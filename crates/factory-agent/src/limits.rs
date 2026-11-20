//! How much of a subscription's usage limits is used, and which plan it is:
//! Claude Pro/Max through Anthropic's OAuth usage and profile endpoints,
//! ChatGPT through Codex's usage endpoint and its sign-in's token.

use std::time::Duration;

use futures_util::future::BoxFuture;
use serde_json::Value;

use crate::auth::RequestAuth;
use crate::clock::{now_ms, parse_iso_ms};
use crate::errors::Error;
use crate::providers::{client, user_agent};

const TIMEOUT: Duration = Duration::from_secs(20);
const WEEK_SECONDS: u64 = 7 * 24 * 3600;
const FIVE_HOURS_SECONDS: u64 = 5 * 3600;

/// One usage limit.
#[derive(Clone, Debug, PartialEq)]
pub struct UsageLimit {
    /// "weekly" (all models), "five_hour", "weekly_opus", ...
    pub id: String,
    /// "Weekly", "5-hour", "Weekly (Opus)", ...
    pub label: String,
    /// 0-100.
    pub used_percent: f64,
    /// Milliseconds since 1970; 0 if unknown.
    pub resets_at: u64,
}

/// Reads a provider's limits with its signed-in credential, against its base url.
pub type LimitsFn = fn(base_url: String, auth: RequestAuth) -> BoxFuture<'static, Result<Vec<UsageLimit>, Error>>;

/// Reads which plan a subscription sign-in has, such as "Claude Max 20x" or
/// "ChatGPT Pro"; empty when the provider doesn't say.
pub type PlanFn = fn(base_url: String, auth: RequestAuth) -> BoxFuture<'static, Result<String, Error>>;

/// Who a sign-in is.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Identity {
    /// Tells the account apart from the provider's others (its user id, say);
    /// empty when the provider doesn't say.
    pub key: String,
    /// What people know it by: its email, or else its name.
    pub label: String,
}

/// Reads who a subscription sign-in is.
pub type IdentityFn = fn(base_url: String, auth: RequestAuth) -> BoxFuture<'static, Result<Identity, Error>>;

fn limit(id: impl Into<String>, label: impl Into<String>, used_percent: f64, resets_at: u64) -> UsageLimit {
    UsageLimit { id: id.into(), label: label.into(), used_percent: used_percent.clamp(0.0, 100.0), resets_at }
}

/// "claude_max" -> "Claude Max", "free_workspace" -> "Free Workspace".
fn title(words: &str) -> String {
    let mut title = Vec::new();
    for word in words.split(['_', '-', ' ']).filter(|word| !word.is_empty()) {
        let mut characters = word.chars();
        if let Some(first) = characters.next() {
            title.push(first.to_uppercase().chain(characters).collect::<String>());
        }
    }
    title.join(" ")
}

/// "Fable" -> "fable", "GPT-5.3 Codex Spark" -> "gpt_5_3_codex_spark".
fn slug(name: &str) -> String {
    let mut slug = String::new();
    for character in name.chars() {
        if character.is_ascii_alphanumeric() {
            slug.push(character.to_ascii_lowercase());
        } else if !slug.is_empty() && !slug.ends_with('_') {
            slug.push('_');
        }
    }
    slug.trim_end_matches('_').to_owned()
}

/// `what` names the request in errors: "Anthropic usage", "Anthropic plan".
async fn get_json(what: &str, request: reqwest::RequestBuilder) -> Result<Value, Error> {
    let response = request.timeout(TIMEOUT).send().await.map_err(|error| Error::Other(format!("{what}: {error}")))?;
    let status = response.status();
    let text = response.text().await.unwrap_or_default();
    if !status.is_success() {
        let parsed: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
        let message = parsed["error"]["message"]
            .as_str()
            .or_else(|| parsed["error"].as_str())
            .or_else(|| parsed["detail"].as_str())
            .or_else(|| parsed["message"].as_str())
            .or(status.canonical_reason())
            .unwrap_or_default();
        return Err(Error::Other(format!("{what}: HTTP {}: {message}", status.as_u16())));
    }
    serde_json::from_str(&text).map_err(|error| Error::Other(format!("{what}: unreadable answer: {error}")))
}

// Anthropic (Claude Pro/Max).

const ANTHROPIC_WINDOWS: [(&str, &str, &str); 5] = [
    ("seven_day", "weekly", "Weekly"),
    ("five_hour", "five_hour", "5-hour"),
    ("seven_day_opus", "weekly_opus", "Weekly (Opus)"),
    ("seven_day_sonnet", "weekly_sonnet", "Weekly (Sonnet)"),
    ("seven_day_oauth_apps", "weekly_oauth_apps", "Weekly (OAuth apps)"),
];

/// GETs one of Anthropic's OAuth account endpoints, `/api/oauth/<path>`.
async fn anthropic_get(what: &str, path: &str, base_url: &str, auth: RequestAuth) -> Result<Value, Error> {
    let token = auth.api_key.ok_or_else(|| Error::Other(format!("{what}: not signed in")))?;
    let request = client()
        .get(format!("{}/api/oauth/{path}", base_url.trim_end_matches('/')))
        .bearer_auth(token)
        .header("anthropic-beta", "oauth-2025-04-20")
        .header("user-agent", format!("claude-cli/{}", crate::providers::anthropic::CLAUDE_CODE_VERSION));
    get_json(what, request).await
}

pub fn anthropic(base_url: String, auth: RequestAuth) -> BoxFuture<'static, Result<Vec<UsageLimit>, Error>> {
    Box::pin(async move { Ok(parse_anthropic(&anthropic_get("Anthropic usage", "usage", &base_url, auth).await?)) })
}

pub fn anthropic_plan(base_url: String, auth: RequestAuth) -> BoxFuture<'static, Result<String, Error>> {
    Box::pin(async move { Ok(parse_anthropic_plan(&anthropic_get("Anthropic plan", "profile", &base_url, auth).await?)) })
}

pub fn anthropic_identity(base_url: String, auth: RequestAuth) -> BoxFuture<'static, Result<Identity, Error>> {
    Box::pin(async move { Ok(parse_anthropic_identity(&anthropic_get("Anthropic account", "profile", &base_url, auth).await?)) })
}

/// The account in Anthropic's OAuth profile: its `uuid`, and its `email`
/// (or else its `display_name` or `full_name`).
pub fn parse_anthropic_identity(profile: &Value) -> Identity {
    let account = &profile["account"];
    let text = |key: &str| account[key].as_str().map(str::trim).filter(|text| !text.is_empty()).map(str::to_owned);
    let label = text("email").or_else(|| text("email_address")).or_else(|| text("display_name")).or_else(|| text("full_name"));
    Identity { key: text("uuid").or_else(|| label.clone()).unwrap_or_default(), label: label.unwrap_or_default() }
}

/// The plan in Anthropic's OAuth profile: the organization's
/// `plan_display_name` ("Max" -> "Claude Max"), or else its
/// `organization_type` ("claude_max", with the 5x or 20x of its
/// `rate_limit_tier`), or else the account's `has_claude_max`/`has_claude_pro`.
pub fn parse_anthropic_plan(profile: &Value) -> String {
    let organization = &profile["organization"];
    if let Some(name) = organization["plan_display_name"].as_str().map(str::trim).filter(|name| !name.is_empty()) {
        return if name.starts_with("Claude") { name.to_owned() } else { format!("Claude {name}") };
    }
    let max = || {
        let tier = organization["rate_limit_tier"].as_str().unwrap_or_default();
        match tier.rsplit('_').next() {
            Some(multiple @ ("5x" | "20x")) => format!("Claude Max {multiple}"),
            _ => "Claude Max".to_owned(),
        }
    };
    match organization["organization_type"].as_str() {
        Some("claude_max") => max(),
        Some(kind) if kind.starts_with("claude_") => title(kind),
        _ if profile["account"]["has_claude_max"].as_bool() == Some(true) => max(),
        _ if profile["account"]["has_claude_pro"].as_bool() == Some(true) => "Claude Pro".to_owned(),
        _ => String::new(),
    }
}

/// `seven_day`, `five_hour` and the per-model weekly windows, each
/// `{"utilization": <percent>, "resets_at": "<RFC 3339>"}` or null, then any
/// model-scoped weekly limit in `limits` not already among them.
pub fn parse_anthropic(usage: &Value) -> Vec<UsageLimit> {
    let mut limits = Vec::new();
    for (key, id, label) in ANTHROPIC_WINDOWS {
        let window = &usage[key];
        let Some(used) = window["utilization"].as_f64() else { continue };
        limits.push(limit(id, label, used, window["resets_at"].as_str().and_then(parse_iso_ms).unwrap_or(0)));
    }
    for entry in usage["limits"].as_array().into_iter().flatten() {
        let Some(used) = entry["percent"].as_f64() else { continue };
        let resets_at = entry["resets_at"].as_str().and_then(parse_iso_ms).unwrap_or(0);
        let (id, label) = match entry["kind"].as_str() {
            Some("weekly_all") => ("weekly".to_owned(), "Weekly".to_owned()),
            Some("weekly_scoped") => {
                let Some(model) = entry["scope"]["model"]["display_name"].as_str().filter(|name| !slug(name).is_empty()) else {
                    continue;
                };
                (format!("weekly_{}", slug(model)), format!("Weekly ({model})"))
            }
            _ => continue,
        };
        if limits.iter().all(|known: &UsageLimit| known.id != id) {
            limits.push(limit(id, label, used, resets_at));
        }
    }
    limits
}

// OpenAI Codex (ChatGPT Plus/Pro).

/// GETs Codex's usage endpoint.
async fn codex_usage(what: &str, base_url: &str, auth: RequestAuth) -> Result<Value, Error> {
    let token = auth.api_key.ok_or_else(|| Error::Other(format!("{what}: not signed in")))?;
    let account = crate::providers::openai_codex::oauth::account_id(&token)
        .ok_or_else(|| Error::Other(format!("{what}: the sign-in has no ChatGPT account")))?;
    let base = base_url.trim_end_matches('/');
    // Codex's own client: ChatGPT's backend serves it at /wham/usage.
    let url = if base.contains("/backend-api") { format!("{base}/wham/usage") } else { format!("{base}/api/codex/usage") };
    let request = client()
        .get(url)
        .bearer_auth(token)
        .header("chatgpt-account-id", account)
        .header("originator", "pi")
        .header("User-Agent", user_agent());
    get_json(what, request).await
}

pub fn codex(base_url: String, auth: RequestAuth) -> BoxFuture<'static, Result<Vec<UsageLimit>, Error>> {
    Box::pin(async move { Ok(parse_codex(&codex_usage("OpenAI Codex usage", &base_url, auth).await?, now_ms())) })
}

/// The plan in the access token's `chatgpt_plan_type` claim; when the token
/// has none, the `plan_type` of Codex's usage endpoint.
pub fn codex_plan(base_url: String, auth: RequestAuth) -> BoxFuture<'static, Result<String, Error>> {
    Box::pin(async move {
        if let Some(plan) = auth.api_key.as_deref().and_then(crate::providers::openai_codex::oauth::plan_type) {
            return Ok(chatgpt_plan(&plan));
        }
        let usage = codex_usage("OpenAI Codex plan", &base_url, auth).await?;
        Ok(usage["plan_type"].as_str().map(chatgpt_plan).unwrap_or_default())
    })
}

/// Who the access token is: its ChatGPT user and account (one person can be
/// in several, each with its own limits), and its email.
pub fn codex_identity(_base_url: String, auth: RequestAuth) -> BoxFuture<'static, Result<Identity, Error>> {
    Box::pin(async move { Ok(auth.api_key.as_deref().map(crate::providers::openai_codex::oauth::identity).unwrap_or_default()) })
}

/// "plus" -> "ChatGPT Plus", "pro" -> "ChatGPT Pro", "edu" -> "ChatGPT Edu";
/// empty for "unknown" or nothing.
pub fn chatgpt_plan(plan_type: &str) -> String {
    match plan_type.trim() {
        "" | "unknown" => String::new(),
        "education" => "ChatGPT Edu".to_owned(),
        plan => format!("ChatGPT {}", title(plan)),
    }
}

/// "five_hour"/"5-hour" and "weekly"/"Weekly" by the window's length; others
/// by their length in days or hours.
fn codex_window(seconds: u64) -> (String, String) {
    match seconds {
        FIVE_HOURS_SECONDS => ("five_hour".into(), "5-hour".into()),
        WEEK_SECONDS => ("weekly".into(), "Weekly".into()),
        _ if seconds >= 86_400 && seconds.is_multiple_of(86_400) => {
            (format!("{}_day", seconds / 86_400), format!("{}-day", seconds / 86_400))
        }
        _ => {
            let hours = seconds.div_ceil(3600).max(1);
            (format!("{hours}_hour"), format!("{hours}-hour"))
        }
    }
}

fn push_codex_windows(limits: &mut Vec<UsageLimit>, rate_limit: &Value, scope: Option<&str>, now: u64) {
    for key in ["primary_window", "secondary_window"] {
        let window = &rate_limit[key];
        let Some(used) = window["used_percent"].as_f64() else { continue };
        let Some(seconds) = window["limit_window_seconds"].as_u64() else { continue };
        let (mut id, mut label) = codex_window(seconds);
        if let Some(name) = scope {
            id = format!("{id}_{}", slug(name));
            label = format!("{label} ({name})");
        }
        let resets_at = match (window["reset_at"].as_u64(), window["reset_after_seconds"].as_u64()) {
            (Some(at), _) => at * 1000,
            (None, Some(after)) => now + after * 1000,
            (None, None) => 0,
        };
        if limits.iter().all(|known| known.id != id) {
            limits.push(limit(id, label, used, resets_at));
        }
    }
}

/// `rate_limit.{primary,secondary}_window`, each `{"used_percent",
/// "limit_window_seconds", "reset_at" (seconds since 1970), "reset_after_seconds"}`,
/// then `additional_rate_limits[]`, the per-model ones named by `limit_name`.
pub fn parse_codex(usage: &Value, now: u64) -> Vec<UsageLimit> {
    let mut limits = Vec::new();
    push_codex_windows(&mut limits, &usage["rate_limit"], None, now);
    for extra in usage["additional_rate_limits"].as_array().into_iter().flatten() {
        let name = extra["limit_name"].as_str().or_else(|| extra["metered_feature"].as_str()).filter(|name| !slug(name).is_empty());
        if let Some(name) = name {
            push_codex_windows(&mut limits, &extra["rate_limit"], Some(name), now);
        }
    }
    // The headline window first, then shortest to longest.
    limits.sort_by_key(|limit| limit.id != "weekly");
    limits
}
