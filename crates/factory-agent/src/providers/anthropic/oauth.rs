use std::sync::LazyLock;
use std::time::Duration;

use base64::Engine;
use serde_json::{Value, json};
use tokio::net::TcpListener;
use tokio_util::sync::CancellationToken;

use crate::auth::{AuthEvent, AuthFuture, AuthInteraction, OAuthFlow, RequestAuth};
use crate::clock::now_ms;
use crate::credentials::Credential;
use crate::errors::Error;
use crate::oauth::{
    Callback, FixedCallback, bound_port, browser_authorization, callback_host, error_html, finishing_html, fixed_callback, generate_pkce,
    request_auth,
};
use crate::providers::{client, response_text, send_with_timeout};

/// Pi stores the client id base64-encoded; decoding it keeps the constant byte-identical to the source.
pub static CLIENT_ID: LazyLock<String> = LazyLock::new(|| {
    String::from_utf8(
        base64::engine::general_purpose::STANDARD.decode("OWQxYzI1MGEtZTYxYi00NGQ5LTg4ZWQtNTk0NGQxOTYyZjVl").unwrap_or_default(),
    )
    .unwrap_or_default()
});
const AUTHORIZE_URL: &str = "https://claude.ai/oauth/authorize";
const TOKEN_URL: &str = "https://platform.claude.com/v1/oauth/token";
const CALLBACK_PORT: u16 = 53692;
const CALLBACK_PATH: &str = "/callback";
const SCOPES: &str = "org:create_api_key user:profile user:inference user:sessions:claude_code user:mcp_servers user:file_upload";
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
const EARLY_EXPIRY_MS: u64 = 5 * 60 * 1000;

pub struct AnthropicOAuth {
    pub authorize_url: String,
    pub token_url: String,
    pub callback_port: u16,
}

impl Default for AnthropicOAuth {
    fn default() -> AnthropicOAuth {
        AnthropicOAuth::new()
    }
}

fn credential_from(data: &Value) -> Option<Credential> {
    let access = data["access_token"].as_str()?;
    let refresh = data["refresh_token"].as_str()?;
    let expires_in = data["expires_in"].as_f64()?;
    Some(Credential::Oauth {
        access: access.to_owned(),
        refresh: refresh.to_owned(),
        expires: (now_ms() + (expires_in * 1000.0) as u64).saturating_sub(EARLY_EXPIRY_MS),
        account_id: None,
        extra: Default::default(),
    })
}

impl AnthropicOAuth {
    pub fn new() -> AnthropicOAuth {
        AnthropicOAuth { authorize_url: AUTHORIZE_URL.into(), token_url: TOKEN_URL.into(), callback_port: CALLBACK_PORT }
    }

    fn redirect_uri(port: u16) -> String {
        format!("http://localhost:{port}{CALLBACK_PATH}")
    }

    /// `postJson`: a 30 s request whose non-2xx response is an error carrying the status, url and body.
    async fn post_json(&self, body: Value, cancel: &CancellationToken) -> Result<String, Error> {
        let request =
            client().post(&self.token_url).header("Content-Type", "application/json").header("Accept", "application/json").json(&body);
        let response =
            send_with_timeout(request, REQUEST_TIMEOUT, cancel, "The operation was aborted due to timeout", "This operation was aborted")
                .await?;
        let status = response.status().as_u16();
        let text = response_text(response).await;
        if !(200..300).contains(&status) {
            let message = format!("HTTP request failed. status={status}; url={}; body={text}", self.token_url);
            return Err(crate::auth::refusal(crate::auth::refresh_refused(status, &text), message));
        }
        Ok(text)
    }

    async fn token(
        &self,
        body: Value,
        cancel: &CancellationToken,
        request_error: String,
        invalid_error: &str,
    ) -> Result<Credential, Error> {
        let text = self
            .post_json(body, cancel)
            .await
            .map_err(|error| crate::auth::refusal(error.is_refused(), format!("{request_error} details=Error: {error}")))?;
        let invalid =
            |detail: String| Error::Other(format!("{invalid_error} url={}; body={text}; details=Error: {detail}", self.token_url));
        let data = serde_json::from_str(&text).map_err(|error| invalid(error.to_string()))?;
        credential_from(&data).ok_or_else(|| invalid("missing access_token, refresh_token or expires_in".into()))
    }

    fn authorize_url(&self, challenge: &str, state: &str, port: u16) -> String {
        let query = url::form_urlencoded::Serializer::new(String::new())
            .append_pair("code", "true")
            .append_pair("client_id", &CLIENT_ID)
            .append_pair("response_type", "code")
            .append_pair("redirect_uri", &Self::redirect_uri(port))
            .append_pair("scope", SCOPES)
            .append_pair("code_challenge", challenge)
            .append_pair("code_challenge_method", "S256")
            .append_pair("state", state)
            .finish();
        format!("{}?{query}", self.authorize_url)
    }
}

/// Serves `/callback` on the fixed port (0: any free one, for tests), and
/// returns the port the redirect names; the receiver yields the code and state
/// once. When something else holds the port (another sign-in, another app),
/// the person can still paste the redirect URL, as with Codex.
async fn start_callback(port: u16, expected_state: String) -> (u16, Callback) {
    let listener = TcpListener::bind((callback_host().as_str(), port)).await;
    if let Err(error) = &listener {
        tracing::warn!(port, %error, "Anthropic sign-in callback port unavailable; waiting for a pasted redirect URL");
    }
    let port = bound_port(&listener, port);
    let callback = fixed_callback(
        listener,
        expected_state,
        FixedCallback {
            path: CALLBACK_PATH,
            success: finishing_html("Anthropic"),
            internal: "Internal error".into(),
            missing: error_html("Missing code or state parameter.", None),
            oauth_error: Some("Anthropic authentication did not complete."),
            state_first: false,
        },
    );
    (port, callback)
}

impl AnthropicOAuth {
    async fn login_browser(&self, interaction: &dyn AuthInteraction) -> Result<Credential, Error> {
        let cancel = interaction.cancel();
        let (verifier, challenge) = generate_pkce();
        let (port, callback) = start_callback(self.callback_port, verifier.clone()).await;
        let (code, state) = browser_authorization(
            interaction,
            &cancel,
            callback,
            self.authorize_url(&challenge, &verifier, port),
            (
                "Complete login in your browser. If the browser is on another machine, paste the final redirect URL here.",
                "Complete login in your browser, or paste the authorization code / redirect URL here:",
                Self::redirect_uri(port),
            ),
            &verifier,
            "OAuth state mismatch",
        )
        .await?;
        interaction.notify(AuthEvent::Progress { message: "Exchanging authorization code for tokens...".into() });
        let redirect_uri = Self::redirect_uri(port);
        let body = json!({"grant_type": "authorization_code", "client_id": *CLIENT_ID, "code": code, "state": state, "redirect_uri": redirect_uri, "code_verifier": verifier});
        self.token(
            body,
            &cancel,
            format!(
                "Token exchange request failed. url={}; redirect_uri={redirect_uri}; response_type=authorization_code;",
                self.token_url
            ),
            "Token exchange returned invalid JSON.",
        )
        .await
    }
}

impl OAuthFlow for AnthropicOAuth {
    fn name(&self) -> &str {
        "Anthropic (Claude Pro/Max)"
    }

    fn is_subscription(&self) -> bool {
        true
    }

    fn login<'a>(&'a self, interaction: &'a dyn AuthInteraction) -> AuthFuture<'a, Result<Credential, Error>> {
        Box::pin(self.login_browser(interaction))
    }

    fn refresh<'a>(&'a self, credential: Credential, cancel: CancellationToken) -> AuthFuture<'a, Result<Credential, Error>> {
        Box::pin(async move {
            let Credential::Oauth { refresh, .. } = credential else {
                return Err(Error::Other("Anthropic token refresh request failed: not an oauth credential".into()));
            };
            let body = json!({"grant_type": "refresh_token", "client_id": *CLIENT_ID, "refresh_token": refresh});
            self.token(
                body,
                &cancel,
                format!("Anthropic token refresh request failed. url={};", self.token_url),
                "Anthropic token refresh returned invalid JSON.",
            )
            .await
        })
    }

    fn to_auth(&self, credential: &Credential) -> Result<RequestAuth, Error> {
        Ok(request_auth(credential))
    }
}
