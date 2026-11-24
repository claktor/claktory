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

/// What a key has to spend, as its provider says (OpenRouter's GET /key),
/// in the provider's currency (USD for OpenRouter). Numbers parsed from its
/// answer, never its text.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct KeyCredit {
    /// The key's spending limit; None when it has none.
    pub limit: Option<f64>,
    /// What's left of that limit (zero or less: used up); None with no limit.
    pub left: Option<f64>,
    /// When the limit refills: "daily", "weekly" or "monthly"; empty when
    /// it never does.
    pub limit_reset: String,
    /// Spent today (since 00:00 UTC).
    pub spent_today: f64,
    /// The account never bought credit: only free models run on it.
    pub never_paid: bool,
}

/// Reads what the key in `auth` has to spend from the provider at
/// `base_url`; why not, in a short sentence, when it can't.
pub type CreditFn = fn(base_url: String, auth: RequestAuth) -> BoxFuture<'static, Result<KeyCredit, String>>;

/// How long the check waits for the provider.
const CHECK_WAIT: Duration = Duration::from_secs(15);

/// Anthropic: its models, one of them.
pub fn anthropic(base_url: String, auth: RequestAuth) -> BoxFuture<'static, KeyCheck> {
    Box::pin(async move {
        let Some(key) = auth.api_key.clone() else { return KeyCheck::Unchecked("no key".into()) };
        let request = client()
            .get(format!("{}/v1/models?limit=1", base_url.trim_end_matches('/')))
            .header("x-api-key", key.clone())
            .header("anthropic-version", "2023-06-01");
        check(with_headers(request, &auth), &key).await
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

/// OpenRouter: the key's limit, what's left of it and today's spend, from
/// GET /key. The key goes only in the Authorization header, and nothing of
/// the answer is kept but the numbers.
pub fn openrouter_credit(base_url: String, auth: RequestAuth) -> BoxFuture<'static, Result<KeyCredit, String>> {
    Box::pin(async move {
        let Some(key) = auth.api_key.clone() else { return Err("There's no key to ask about.".into()) };
        let request = client().get(format!("{}/key", base_url.trim_end_matches('/'))).bearer_auth(&key);
        let response = match tokio::time::timeout(CHECK_WAIT, with_headers(request, &auth).send()).await {
            Err(_) => return Err("OpenRouter didn't answer in time.".to_owned()),
            Ok(Err(_)) => return Err("OpenRouter couldn't be reached.".into()),
            Ok(Ok(response)) => response,
        };
        let status = response.status();
        let body = tokio::time::timeout(CHECK_WAIT, bounded_text(response)).await.unwrap_or_default();
        if !status.is_success() {
            return Err(format!("OpenRouter answered {}{}.", status.as_u16(), said(&body, &key)));
        }
        let value: serde_json::Value = serde_json::from_str(&body).map_err(|_| "OpenRouter's answer couldn't be read.".to_owned())?;
        parse_credit(&value).ok_or_else(|| "OpenRouter didn't say how much this key can spend.".to_owned())
    })
}

/// The most of an answer's body read.
const MOST_BODY: usize = 64 * 1024;

/// An answer's body as text, at most `MOST_BODY` bytes of it; what was read
/// before a failure.
async fn bounded_text(mut response: reqwest::Response) -> String {
    let mut body = Vec::new();
    while let Ok(Some(chunk)) = response.chunk().await {
        let room = MOST_BODY - body.len();
        body.extend_from_slice(&chunk[..chunk.len().min(room)]);
        if body.len() >= MOST_BODY {
            break;
        }
    }
    String::from_utf8_lossy(&body).into_owned()
}

/// GET /key's `data`, its numbers only.
fn parse_credit(value: &serde_json::Value) -> Option<KeyCredit> {
    let data = value.get("data")?.as_object()?;
    let amount = |name: &str| data.get(name).and_then(serde_json::Value::as_f64).filter(|amount| amount.is_finite());
    let limit = amount("limit");
    let reset = data.get("limit_reset").and_then(serde_json::Value::as_str).unwrap_or_default();
    Some(KeyCredit {
        limit,
        left: limit.and_then(|_| amount("limit_remaining")),
        limit_reset: if limit.is_some() && ["daily", "weekly", "monthly"].contains(&reset) { reset.to_owned() } else { String::new() },
        spent_today: amount("usage_daily").unwrap_or(0.0).max(0.0),
        never_paid: data.get("is_free_tier").and_then(serde_json::Value::as_bool).unwrap_or(false),
    })
}

/// What a refusal said, for a log or an error: its JSON `error.message`
/// alone, cut short and with the key taken out, never the whole body.
pub(crate) fn said(body: &str, key: &str) -> String {
    let message = serde_json::from_str::<serde_json::Value>(body)
        .ok()
        .and_then(|value| value.pointer("/error/message").and_then(serde_json::Value::as_str).map(str::to_owned))
        .unwrap_or_default();
    let message = if key.trim().is_empty() { message } else { message.replace(key.trim(), "[key]") };
    let message: String = message.chars().filter(|char| !char.is_control()).take(200).collect();
    if message.trim().is_empty() { String::new() } else { format!(": {}", message.trim()) }
}

fn bearer(base_url: String, auth: RequestAuth, path: &'static str) -> BoxFuture<'static, KeyCheck> {
    Box::pin(async move {
        let Some(key) = auth.api_key.clone() else { return KeyCheck::Unchecked("no key".into()) };
        let request = client().get(format!("{}/{path}", base_url.trim_end_matches('/'))).bearer_auth(&key);
        check(with_headers(request, &auth), &key).await
    })
}

fn with_headers(mut request: reqwest::RequestBuilder, auth: &RequestAuth) -> reqwest::RequestBuilder {
    for (name, value) in &auth.headers {
        request = request.header(name, value);
    }
    request
}

async fn check(request: reqwest::RequestBuilder, key: &str) -> KeyCheck {
    let response = match tokio::time::timeout(CHECK_WAIT, request.send()).await {
        Err(_) => return KeyCheck::Unchecked(format!("no answer within {} s", CHECK_WAIT.as_secs())),
        Ok(Err(error)) => return KeyCheck::Unchecked(error.to_string()),
        Ok(Ok(response)) => response,
    };
    let status = response.status();
    if status.is_success() {
        return KeyCheck::Accepted;
    }
    let body = tokio::time::timeout(CHECK_WAIT, bounded_text(response)).await.unwrap_or_default();
    // Logged: the status and the provider's own message, never the body.
    let detail = format!("{}{}", status.as_u16(), said(&body, key));
    if refused_error(status.as_u16(), &body) { KeyCheck::Refused(detail) } else { KeyCheck::Unchecked(detail) }
}

#[cfg(test)]
mod tests {
    use super::{KeyCredit, parse_credit, said};
    use serde_json::json;

    #[test]
    fn credit_is_read_from_the_numbers_alone() {
        let limited = json!({"data": {"label": "sk-or-v1-abc...6ed", "limit": 10, "limit_remaining": 4.2, "limit_reset": "weekly",
            "usage": 40.5, "usage_daily": 0.8, "is_free_tier": false}});
        assert_eq!(
            parse_credit(&limited),
            Some(KeyCredit { limit: Some(10.0), left: Some(4.2), limit_reset: "weekly".into(), spent_today: 0.8, never_paid: false })
        );
        let unlimited =
            json!({"data": {"limit": null, "limit_remaining": null, "limit_reset": "weekly", "usage_daily": 0, "is_free_tier": true}});
        assert_eq!(parse_credit(&unlimited), Some(KeyCredit { never_paid: true, ..KeyCredit::default() }));
        assert_eq!(parse_credit(&json!({"error": {"message": "nope"}})), None);
    }

    #[test]
    fn a_refusal_says_only_its_message_without_the_key() {
        let body = json!({"error": {"code": 401, "message": "Key sk-or-v1-secret is not valid"}, "user_id": "u1"}).to_string();
        assert_eq!(said(&body, "sk-or-v1-secret"), ": Key [key] is not valid");
        assert_eq!(said("<html>whatever sk-or-v1-secret</html>", "sk-or-v1-secret"), "");
    }
}
