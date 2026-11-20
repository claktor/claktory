use std::time::Duration;

use base64::Engine;
use serde_json::{Value, json};
use tokio::net::TcpListener;
use tokio_util::sync::CancellationToken;

use crate::auth::AuthFuture;
use crate::auth::{AuthEvent, AuthInteraction, AuthPrompt, OAuthFlow, RequestAuth, SelectOption};
use crate::clock::now_ms;
use crate::credentials::Credential;
use crate::errors::Error;
use crate::oauth::{
    CANCEL_MESSAGE, Callback, FixedCallback, bound_port, browser_authorization, callback_host, check_cancel, error_html, finishing_html,
    fixed_callback, generate_pkce, random_hex, send_cancellable,
};
use crate::providers::{client, response_text};

pub const CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";
pub const JWT_CLAIM_PATH: &str = "https://api.openai.com/auth";
const AUTH_BASE_URL: &str = "https://auth.openai.com";
const CALLBACK_PORT: u16 = 1455;
const SCOPE: &str = "openid profile email offline_access";
const DEVICE_CODE_TIMEOUT_SECONDS: u64 = 15 * 60;
const MINIMUM_POLL_INTERVAL_MS: u64 = 1000;
const DEFAULT_POLL_INTERVAL_SECONDS: f64 = 5.0;
const SLOW_DOWN_INCREMENT_MS: u64 = 5000;
const TIMEOUT_MESSAGE: &str = "Device flow timed out";
const SLOW_DOWN_TIMEOUT_MESSAGE: &str = "Device flow timed out after one or more slow_down responses. This is often caused by clock drift in WSL or VM environments. Please sync or restart the VM clock and try again.";

fn decode_base64_lenient(text: &str) -> Option<Vec<u8>> {
    use base64::engine::general_purpose::{STANDARD, STANDARD_NO_PAD, URL_SAFE, URL_SAFE_NO_PAD};
    [STANDARD, STANDARD_NO_PAD, URL_SAFE, URL_SAFE_NO_PAD].iter().find_map(|engine| engine.decode(text).ok())
}

/// A field of the access token's `https://api.openai.com/auth` claim.
fn auth_claim(token: &str, field: &str) -> Option<String> {
    let parts: Vec<&str> = token.split('.').collect();
    if parts.len() != 3 {
        return None;
    }
    let claims: Value = serde_json::from_slice(&decode_base64_lenient(parts[1])?).ok()?;
    claims[JWT_CLAIM_PATH][field].as_str().filter(|value| !value.is_empty()).map(str::to_owned)
}

/// The `chatgpt_account_id` inside the access token's `https://api.openai.com/auth` claim.
pub fn account_id(token: &str) -> Option<String> {
    auth_claim(token, "chatgpt_account_id")
}

/// Who the access token is: its user and ChatGPT account, and the email of
/// its `https://api.openai.com/profile` claim.
pub fn identity(token: &str) -> crate::limits::Identity {
    let claims = token.split('.').nth(1).and_then(decode_base64_lenient).and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok());
    let email = claims
        .as_ref()
        .and_then(|claims| claims["https://api.openai.com/profile"]["email"].as_str().or_else(|| claims["email"].as_str()))
        .unwrap_or_default()
        .to_owned();
    let user = auth_claim(token, "chatgpt_user_id").or_else(|| auth_claim(token, "user_id")).unwrap_or_else(|| email.clone());
    let key = match account_id(token) {
        Some(account) if !user.is_empty() => format!("{user}/{account}"),
        Some(account) => account,
        None => user,
    };
    crate::limits::Identity { key, label: email }
}

/// The ChatGPT plan ("plus", "pro", ...) in the access token's `chatgpt_plan_type` claim.
pub fn plan_type(token: &str) -> Option<String> {
    auth_claim(token, "chatgpt_plan_type")
}

struct Token {
    access: String,
    refresh: String,
    expires: u64,
}

fn account_id_error() -> Error {
    Error::Other("Failed to extract accountId from token".into())
}

fn credential_from_token(token: Token) -> Result<Credential, Error> {
    let account = account_id(&token.access).ok_or_else(account_id_error)?;
    Ok(Credential::Oauth {
        access: token.access,
        refresh: token.refresh,
        expires: token.expires,
        account_id: Some(account),
        extra: Default::default(),
    })
}

async fn read_token_response(response: reqwest::Response, operation: &str) -> Result<Token, Error> {
    let status = response.status();
    if !status.is_success() {
        let text = response_text(response).await;
        let detail = if text.is_empty() { status.canonical_reason().unwrap_or_default().to_owned() } else { text };
        return Err(Error::Other(format!("OpenAI Codex token {operation} failed ({}): {detail}", status.as_u16())));
    }
    let json: Value = response.json().await.map_err(|error| Error::Other(error.to_string()))?;
    match (json["access_token"].as_str(), json["refresh_token"].as_str(), json["expires_in"].as_f64()) {
        (Some(access), Some(refresh), Some(expires_in)) if !access.is_empty() && !refresh.is_empty() => {
            Ok(Token { access: access.to_owned(), refresh: refresh.to_owned(), expires: now_ms() + (expires_in * 1000.0) as u64 })
        }
        _ => Err(Error::Other(format!("OpenAI Codex token {operation} response missing fields: {json}"))),
    }
}

fn detail_suffix(body: &str) -> String {
    if body.is_empty() { String::new() } else { format!(": {body}") }
}

async fn sleep_cancellable(duration: Duration, cancel: &CancellationToken) -> Result<(), Error> {
    check_cancel(cancel)?;
    tokio::select! {
        () = tokio::time::sleep(duration) => Ok(()),
        () = cancel.cancelled() => Err(Error::Other(CANCEL_MESSAGE.into())),
    }
}

struct Device {
    device_auth_id: String,
    user_code: String,
    interval_seconds: f64,
}

enum Poll {
    Pending,
    SlowDown,
    Complete { code: String, verifier: String },
    Failed(String),
}

/// The ChatGPT OAuth flow: browser PKCE with a local callback, or device code for headless hosts.
pub struct CodexOAuth {
    pub auth_base_url: String,
    pub callback_port: u16,
}

impl Default for CodexOAuth {
    fn default() -> CodexOAuth {
        CodexOAuth::new()
    }
}

impl CodexOAuth {
    pub fn new() -> CodexOAuth {
        CodexOAuth { auth_base_url: AUTH_BASE_URL.into(), callback_port: CALLBACK_PORT }
    }

    fn url(&self, path: &str) -> String {
        format!("{}{path}", self.auth_base_url.trim_end_matches('/'))
    }

    fn redirect_uri(port: u16) -> String {
        format!("http://localhost:{port}/auth/callback")
    }

    async fn exchange(&self, code: &str, verifier: &str, redirect_uri: &str, cancel: &CancellationToken) -> Result<Credential, Error> {
        let request = client().post(self.url("/oauth/token")).form(&[
            ("grant_type", "authorization_code"),
            ("client_id", CLIENT_ID),
            ("code", code),
            ("code_verifier", verifier),
            ("redirect_uri", redirect_uri),
        ]);
        credential_from_token(read_token_response(send_cancellable(request, cancel).await?, "exchange").await?)
    }

    async fn refresh_token(&self, refresh: &str, cancel: &CancellationToken) -> Result<Credential, Error> {
        let request = client().post(self.url("/oauth/token")).form(&[
            ("grant_type", "refresh_token"),
            ("refresh_token", refresh),
            ("client_id", CLIENT_ID),
        ]);
        let response = tokio::select! {
            sent = request.send() => sent.map_err(|error| Error::Other(format!("OpenAI Codex token refresh error: {error}")))?,
            () = cancel.cancelled() => return Err(Error::Other("OpenAI Codex token refresh error: This operation was aborted".into())),
        };
        credential_from_token(read_token_response(response, "refresh").await?)
    }

    async fn start_device_auth(&self, cancel: &CancellationToken) -> Result<Device, Error> {
        let request = client().post(self.url("/api/accounts/deviceauth/usercode")).json(&json!({"client_id": CLIENT_ID}));
        let response = send_cancellable(request, cancel).await?;
        let status = response.status().as_u16();
        if status == 404 {
            return Err(Error::Other(
                "OpenAI Codex device code login is not enabled for this server. Use browser login or verify the server URL.".into(),
            ));
        }
        if !(200..300).contains(&status) {
            let body = response_text(response).await;
            let suffix = detail_suffix(&body);
            return Err(Error::Other(format!("OpenAI Codex device code request failed with status {status}{suffix}")));
        }
        let json: Value = response.json().await.map_err(|error| Error::Other(error.to_string()))?;
        let interval = match &json["interval"] {
            Value::String(text) => text.trim().parse::<f64>().ok(),
            other => other.as_f64(),
        };
        match (json["device_auth_id"].as_str(), json["user_code"].as_str(), interval) {
            (Some(id), Some(code), Some(interval)) if !id.is_empty() && !code.is_empty() && interval.is_finite() && interval >= 0.0 => {
                Ok(Device { device_auth_id: id.to_owned(), user_code: code.to_owned(), interval_seconds: interval })
            }
            _ => Err(Error::Other(format!("Invalid OpenAI Codex device code response: {json}"))),
        }
    }

    async fn poll_device_auth(&self, device: &Device, cancel: &CancellationToken) -> Result<Poll, Error> {
        let request = client()
            .post(self.url("/api/accounts/deviceauth/token"))
            .json(&json!({"device_auth_id": device.device_auth_id, "user_code": device.user_code}));
        let response = send_cancellable(request, cancel).await?;
        let status = response.status().as_u16();
        if (200..300).contains(&status) {
            let json: Value = response.json().await.map_err(|error| Error::Other(error.to_string()))?;
            return Ok(match (json["authorization_code"].as_str(), json["code_verifier"].as_str()) {
                (Some(code), Some(verifier)) if !code.is_empty() && !verifier.is_empty() => {
                    Poll::Complete { code: code.to_owned(), verifier: verifier.to_owned() }
                }
                _ => Poll::Failed(format!("Invalid OpenAI Codex device auth token response: {json}")),
            });
        }
        if status == 403 || status == 404 {
            return Ok(Poll::Pending);
        }
        let body = response_text(response).await;
        let code = serde_json::from_str::<Value>(&body).ok().map(|json| match &json["error"] {
            Value::Object(error) => error.get("code").cloned().unwrap_or(Value::Null),
            other => other.clone(),
        });
        Ok(match code.as_ref().and_then(Value::as_str) {
            Some("deviceauth_authorization_pending") => Poll::Pending,
            Some("slow_down") => Poll::SlowDown,
            _ => {
                let suffix = detail_suffix(&body);
                Poll::Failed(format!("OpenAI Codex device auth failed with status {status}{suffix}"))
            }
        })
    }

    /// RFC 8628 polling: at least one second between polls, five more after each `slow_down`, Pi's timeout texts.
    async fn login_device_code(&self, interaction: &dyn AuthInteraction) -> Result<Credential, Error> {
        let cancel = interaction.cancel();
        let device = self.start_device_auth(&cancel).await?;
        interaction.notify(AuthEvent::DeviceCode {
            user_code: device.user_code.clone(),
            verification_uri: self.url("/codex/device"),
            interval_seconds: Some(device.interval_seconds as u64),
            expires_in_seconds: Some(DEVICE_CODE_TIMEOUT_SECONDS),
        });
        let deadline = tokio::time::Instant::now() + Duration::from_secs(DEVICE_CODE_TIMEOUT_SECONDS);
        let interval = if device.interval_seconds.is_finite() { device.interval_seconds } else { DEFAULT_POLL_INTERVAL_SECONDS };
        let mut interval_ms = MINIMUM_POLL_INTERVAL_MS.max((interval * 1000.0).floor() as u64);
        let mut slow_downs = 0;
        while tokio::time::Instant::now() < deadline {
            check_cancel(&cancel)?;
            match self.poll_device_auth(&device, &cancel).await? {
                Poll::Complete { code, verifier } => {
                    return self.exchange(&code, &verifier, &self.url("/deviceauth/callback"), &cancel).await;
                }
                Poll::Failed(message) => return Err(Error::Other(message)),
                Poll::SlowDown => {
                    slow_downs += 1;
                    interval_ms = MINIMUM_POLL_INTERVAL_MS.max(interval_ms + SLOW_DOWN_INCREMENT_MS);
                }
                Poll::Pending => {}
            }
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            if remaining.is_zero() {
                break;
            }
            sleep_cancellable(Duration::from_millis(interval_ms).min(remaining), &cancel).await?;
        }
        Err(Error::Other(if slow_downs > 0 { SLOW_DOWN_TIMEOUT_MESSAGE } else { TIMEOUT_MESSAGE }.into()))
    }

    fn authorize_url(&self, challenge: &str, state: &str, port: u16) -> String {
        let mut url = reqwest::Url::parse(&self.url("/oauth/authorize"))
            .unwrap_or_else(|_| reqwest::Url::parse(AUTH_BASE_URL).expect("constant url"));
        url.query_pairs_mut()
            .append_pair("response_type", "code")
            .append_pair("client_id", CLIENT_ID)
            .append_pair("redirect_uri", &Self::redirect_uri(port))
            .append_pair("scope", SCOPE)
            .append_pair("code_challenge", challenge)
            .append_pair("code_challenge_method", "S256")
            .append_pair("state", state)
            .append_pair("id_token_add_organizations", "true")
            .append_pair("codex_cli_simplified_flow", "true")
            .append_pair("originator", "pi");
        url.to_string()
    }

    async fn login_browser(&self, interaction: &dyn AuthInteraction) -> Result<Credential, Error> {
        let cancel = interaction.cancel();
        let (verifier, challenge) = generate_pkce();
        let state = random_hex::<16>();
        let (port, callback) = start_callback_server(self.callback_port, state.clone()).await;
        let url = self.authorize_url(&challenge, &state, port);
        let (code, _) = browser_authorization(
            interaction,
            &cancel,
            callback,
            url,
            (
                "A browser window should open. Complete login to finish.",
                "Complete login in your browser, or paste the authorization code / redirect URL here:",
                format!("http://localhost:{CALLBACK_PORT}/auth/callback"),
            ),
            &state,
            "State mismatch",
        )
        .await?;
        self.exchange(&code, &verifier, &Self::redirect_uri(port), &cancel).await
    }
}

/// Serves `/auth/callback` on the port (0: any free one, for tests) until the
/// task is aborted, and returns the port the redirect names; the receiver
/// yields the first code, or `None` when the port cannot be bound.
async fn start_callback_server(port: u16, state: String) -> (u16, Callback) {
    let listener = TcpListener::bind((callback_host().as_str(), port)).await;
    let port = bound_port(&listener, port);
    let callback = fixed_callback(
        listener,
        state,
        FixedCallback {
            path: "/auth/callback",
            success: finishing_html("ChatGPT"),
            internal: error_html("Internal error while processing OAuth callback.", None),
            missing: error_html("Missing authorization code.", None),
            oauth_error: None,
            state_first: true,
        },
    );
    (port, callback)
}

impl OAuthFlow for CodexOAuth {
    fn name(&self) -> &str {
        "OpenAI (ChatGPT Plus/Pro)"
    }

    fn is_subscription(&self) -> bool {
        true
    }

    fn login<'a>(&'a self, interaction: &'a dyn AuthInteraction) -> AuthFuture<'a, Result<Credential, Error>> {
        Box::pin(async move {
            let method = interaction
                .prompt(AuthPrompt::Select {
                    message: "Select OpenAI Codex login method:".into(),
                    options: vec![
                        SelectOption { id: "browser".into(), label: "Browser login (default)".into(), description: None },
                        SelectOption { id: "device_code".into(), label: "Device code login (headless)".into(), description: None },
                    ],
                })
                .await?;
            match method.as_str() {
                "device_code" => self.login_device_code(interaction).await,
                "browser" => self.login_browser(interaction).await,
                other => Err(Error::Other(format!("Unknown OpenAI Codex login method: {other}"))),
            }
        })
    }

    fn refresh<'a>(&'a self, credential: Credential, cancel: CancellationToken) -> AuthFuture<'a, Result<Credential, Error>> {
        Box::pin(async move {
            let Credential::Oauth { refresh, .. } = credential else {
                return Err(account_id_error());
            };
            self.refresh_token(&refresh, &cancel).await
        })
    }

    fn to_auth(&self, credential: &Credential) -> Result<RequestAuth, Error> {
        match credential {
            Credential::Oauth { access, .. } => Ok(RequestAuth { api_key: Some(access.clone()), ..RequestAuth::default() }),
            Credential::ApiKey { .. } => Err(account_id_error()),
        }
    }
}
