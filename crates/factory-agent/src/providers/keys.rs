//! Checking an API key with its provider before it's kept: one cheap call
//! that costs nothing (listing models, or reading the key itself), which a
//! key the provider doesn't accept fails.

use std::time::Duration;

use futures_util::future::BoxFuture;

use crate::accounts::refused_error;
use crate::auth::RequestAuth;
use crate::providers::client;

/// What the provider made of a key.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum KeyCheck {
    Accepted,
    /// It refused the key, saying this.
    Refused(String),
    /// It couldn't be asked, or its answer says nothing about the key (it's
    /// down, or another server at the base URL doesn't have the call): why.
    Unchecked(String),
}

/// Asks the provider at `base_url` whether it accepts the key in `auth`.
pub type CheckKeyFn = fn(base_url: String, auth: RequestAuth) -> BoxFuture<'static, KeyCheck>;

/// How long the check waits for the provider.
const CHECK_WAIT: Duration = Duration::from_secs(15);

/// Anthropic: its models, one of them.
pub fn anthropic(base_url: String, auth: RequestAuth) -> BoxFuture<'static, KeyCheck> {
    Box::pin(async move {
        let Some(key) = auth.api_key.clone() else { return KeyCheck::Unchecked("no key".into()) };
        let request = client()
            .get(format!("{}/v1/models?limit=1", base_url.trim_end_matches('/')))
            .header("x-api-key", key)
            .header("anthropic-version", "2023-06-01");
        check(with_headers(request, &auth)).await
    })
}

/// OpenAI, and servers that speak its API: the models.
pub fn openai(base_url: String, auth: RequestAuth) -> BoxFuture<'static, KeyCheck> {
    bearer(base_url, auth, "models")
}

/// OpenRouter: the key itself, as its models need none.
pub fn openrouter(base_url: String, auth: RequestAuth) -> BoxFuture<'static, KeyCheck> {
    bearer(base_url, auth, "key")
}

fn bearer(base_url: String, auth: RequestAuth, path: &'static str) -> BoxFuture<'static, KeyCheck> {
    Box::pin(async move {
        let Some(key) = auth.api_key.clone() else { return KeyCheck::Unchecked("no key".into()) };
        let request = client().get(format!("{}/{path}", base_url.trim_end_matches('/'))).bearer_auth(key);
        check(with_headers(request, &auth)).await
    })
}

fn with_headers(mut request: reqwest::RequestBuilder, auth: &RequestAuth) -> reqwest::RequestBuilder {
    for (name, value) in &auth.headers {
        request = request.header(name, value);
    }
    request
}

async fn check(request: reqwest::RequestBuilder) -> KeyCheck {
    let response = match tokio::time::timeout(CHECK_WAIT, request.send()).await {
        Err(_) => return KeyCheck::Unchecked(format!("no answer within {} s", CHECK_WAIT.as_secs())),
        Ok(Err(error)) => return KeyCheck::Unchecked(error.to_string()),
        Ok(Ok(response)) => response,
    };
    let status = response.status();
    if status.is_success() {
        return KeyCheck::Accepted;
    }
    let body = tokio::time::timeout(CHECK_WAIT, response.text()).await.ok().and_then(Result::ok).unwrap_or_default();
    let detail = format!("{}: {}", status.as_u16(), body.chars().take(300).collect::<String>());
    if refused_error(status.as_u16(), &body) { KeyCheck::Refused(detail) } else { KeyCheck::Unchecked(detail) }
}
