use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use serde_json::{Value, json};
use tokio::net::TcpListener;
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;

use crate::auth::{AuthEvent, AuthFuture, AuthInteraction, AuthPrompt, OAuthFlow, RequestAuth};
use crate::credentials::Credential;
use crate::errors::Error;
use crate::oauth::{
    CANCEL_MESSAGE, CallbackRequests, callback_host, check_cancel, error_html, generate_pkce, request_auth, respond, success_html,
};
use crate::providers::{client, response_text, send_with_timeout};

const AUTHORIZE_URL: &str = "https://openrouter.ai/auth";
const TOKEN_URL: &str = "https://openrouter.ai/api/v1/auth/keys";
const LOGIN_TIMEOUT: Duration = Duration::from_secs(5 * 60);
const EXCHANGE_TIMEOUT: Duration = Duration::from_secs(30);
const NEVER_EXPIRES: u64 = 9_007_199_254_740_991;

/// OpenRouter exchanges the code for a permanent key, stored as an `oauth` credential that never expires.
pub struct OpenRouterOAuth {
    pub authorize_url: String,
    pub token_url: String,
}

impl Default for OpenRouterOAuth {
    fn default() -> OpenRouterOAuth {
        OpenRouterOAuth::new()
    }
}

fn error_detail(body: &Value) -> Option<String> {
    if let Some(text) = body["error_description"].as_str() {
        return Some(text.to_owned());
    }
    if let Some(text) = body["message"].as_str() {
        return Some(text.to_owned());
    }
    match &body["error"] {
        Value::String(text) => Some(text.clone()),
        Value::Object(error) => error.get("message").and_then(Value::as_str).map(str::to_owned),
        _ => None,
    }
}

fn query(url: &reqwest::Url, name: &str) -> Option<String> {
    url.query_pairs().find(|(key, _)| key == name).map(|(_, value)| value.into_owned())
}

fn parse_code(input: &str) -> Option<String> {
    let value = input.trim();
    if value.is_empty() {
        return None;
    }
    if let Ok(url) = reqwest::Url::parse(value) {
        return query(&url, "code");
    }
    if value.contains("code=") {
        return url::form_urlencoded::parse(value.as_bytes()).find(|(key, _)| key == "code").map(|(_, code)| code.into_owned());
    }
    Some(value.to_owned())
}

impl OpenRouterOAuth {
    pub fn new() -> OpenRouterOAuth {
        OpenRouterOAuth { authorize_url: AUTHORIZE_URL.into(), token_url: TOKEN_URL.into() }
    }

    async fn exchange(&self, code: &str, verifier: &str, cancel: &CancellationToken) -> Result<Credential, Error> {
        check_cancel(cancel)?;
        let request = client()
            .post(&self.token_url)
            .header("accept", "application/json")
            .json(&json!({"code": code, "code_verifier": verifier, "code_challenge_method": "S256"}));
        let response =
            send_with_timeout(request, EXCHANGE_TIMEOUT, cancel, "OpenRouter OAuth token exchange timed out", CANCEL_MESSAGE).await?;
        let status = response.status();
        let text = response_text(response).await;
        let body = match serde_json::from_str::<Value>(&text) {
            Ok(body @ Value::Object(_)) => body,
            Ok(_) => json!({}),
            Err(_) if status.is_success() => return Err(Error::Other("OpenRouter OAuth returned invalid JSON".into())),
            Err(_) => json!({}),
        };
        if !status.is_success() {
            let detail = error_detail(&body).map(|detail| format!(": {detail}")).unwrap_or_default();
            return Err(Error::Other(format!("OpenRouter OAuth key exchange failed (HTTP {}){detail}", status.as_u16())));
        }
        let key = body["key"]
            .as_str()
            .filter(|key| !key.is_empty())
            .ok_or_else(|| Error::Other("OpenRouter OAuth response carries no \"key\"".into()))?;
        Ok(Credential::Oauth {
            access: key.to_owned(),
            refresh: String::new(),
            expires: NEVER_EXPIRES,
            account_id: None,
            extra: Default::default(),
        })
    }
}

struct Callback {
    url: String,
    claimed: Arc<AtomicBool>,
    settled: oneshot::Receiver<Result<Credential, Error>>,
    task: tokio::task::JoinHandle<()>,
}

/// A one-shot loopback server on an ephemeral port that exchanges the code itself.
async fn start_callback(flow: &OpenRouterOAuth, path: String, verifier: String, cancel: CancellationToken) -> Result<Callback, Error> {
    let host = callback_host();
    let listener = TcpListener::bind((host.as_str(), 0)).await?;
    let port = listener.local_addr()?.port();
    let url = format!("http://{host}:{port}{path}");
    let claimed = Arc::new(AtomicBool::new(false));
    let (tx, settled) = oneshot::channel();
    let flow = OpenRouterOAuth { authorize_url: flow.authorize_url.clone(), token_url: flow.token_url.clone() };
    let claims = claimed.clone();
    let task = tokio::spawn(async move {
        let mut settle = Some(tx);
        let mut requests = CallbackRequests::new(listener);
        while let Some((mut socket, method, target)) = requests.next().await {
            let Ok(request) = reqwest::Url::parse(&format!("http://{host}{target}")) else { continue };
            if method != "GET" || request.path() != path {
                respond(&mut socket, 404, "Not Found", &error_html("OAuth callback route not found.", None)).await;
                continue;
            }
            if claims.load(Ordering::SeqCst) || settle.is_none() {
                respond(&mut socket, 409, "Conflict", &error_html("This OAuth callback has already been used.", None)).await;
                continue;
            }
            if let Some(error) = query(&request, "error") {
                let description = query(&request, "error_description").unwrap_or(error);
                respond(&mut socket, 400, "Bad Request", &error_html("OpenRouter authorization was denied.", Some(&description))).await;
                if let Some(settle) = settle.take() {
                    let _ = settle.send(Err(Error::Other(format!("OpenRouter authorization failed: {description}"))));
                }
                continue;
            }
            let Some(code) = query(&request, "code").filter(|code| !code.is_empty()) else {
                respond(&mut socket, 400, "Bad Request", &error_html("OpenRouter returned no authorization code.", None)).await;
                continue;
            };
            claims.store(true, Ordering::SeqCst);
            let outcome = flow.exchange(&code, &verifier, &cancel).await;
            match &outcome {
                Ok(_) => respond(&mut socket, 200, "OK", &success_html("Signed in to OpenRouter. You may now close this page.")).await,
                Err(error) => {
                    respond(&mut socket, 502, "Bad Gateway", &error_html("OpenRouter key exchange failed.", Some(&error.to_string()))).await
                }
            }
            if let Some(settle) = settle.take() {
                let _ = settle.send(outcome);
            }
        }
    });
    Ok(Callback { url, claimed, settled, task })
}

impl OAuthFlow for OpenRouterOAuth {
    fn name(&self) -> &str {
        "OpenRouter OAuth"
    }

    fn is_subscription(&self) -> bool {
        false
    }

    fn login<'a>(&'a self, interaction: &'a dyn AuthInteraction) -> AuthFuture<'a, Result<Credential, Error>> {
        Box::pin(async move {
            let cancel = interaction.cancel();
            check_cancel(&cancel)?;
            let (verifier, challenge) = generate_pkce();
            let path = format!("/oauth/callback/{}", uuid::Uuid::new_v4());
            let callback = start_callback(self, path, verifier.clone(), cancel.clone()).await?;
            self.drive(interaction, callback, &verifier, &challenge, &cancel).await
        })
    }

    fn refresh<'a>(&'a self, credential: Credential, _cancel: CancellationToken) -> AuthFuture<'a, Result<Credential, Error>> {
        Box::pin(async move { Ok(credential) })
    }

    fn to_auth(&self, credential: &Credential) -> Result<RequestAuth, Error> {
        Ok(request_auth(credential))
    }
}

impl OpenRouterOAuth {
    /// The callback and a pasted code race; a claimed callback is already exchanging, so its result settles the login.
    async fn drive(
        &self,
        interaction: &dyn AuthInteraction,
        callback: Callback,
        verifier: &str,
        challenge: &str,
        cancel: &CancellationToken,
    ) -> Result<Credential, Error> {
        let Callback { url, claimed, mut settled, task } = callback;
        let query = url::form_urlencoded::Serializer::new(String::new())
            .append_pair("callback_url", &url)
            .append_pair("code_challenge", challenge)
            .append_pair("code_challenge_method", "S256")
            .finish();
        interaction.notify(AuthEvent::Progress { message: format!("Listening for OpenRouter OAuth callback on {url}") });
        interaction.notify(AuthEvent::AuthUrl {
            url: format!("{}?{query}", self.authorize_url),
            instructions: Some(
                "Complete sign-in in your browser. If the browser is on another machine, paste the final redirect URL here.".into(),
            ),
        });
        let manual = interaction.prompt(AuthPrompt::ManualCode {
            message: "Complete sign-in in your browser, or paste the authorization code / redirect URL here:".into(),
            placeholder: Some(url.clone()),
        });
        tokio::pin!(manual);
        let outcome = async {
            tokio::select! {
                received = &mut settled => received.unwrap_or_else(|_| Err(Error::Other(CANCEL_MESSAGE.into()))),
                typed = &mut manual => {
                    let input = typed?;
                    if claimed.load(Ordering::SeqCst) {
                        return settled.await.unwrap_or_else(|_| Err(Error::Other(CANCEL_MESSAGE.into())));
                    }
                    let code = parse_code(&input).ok_or_else(|| Error::Other("Missing authorization code".into()))?;
                    interaction.notify(AuthEvent::Progress { message: "Exchanging authorization code for an API key...".into() });
                    self.exchange(&code, verifier, cancel).await
                }
                () = tokio::time::sleep(LOGIN_TIMEOUT) => Err(Error::Other("OpenRouter OAuth login timed out".into())),
                () = cancel.cancelled() => Err(Error::Other(CANCEL_MESSAGE.into())),
            }
        }
        .await;
        task.abort();
        let _ = task.await;
        outcome
    }
}
