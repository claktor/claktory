//! Several accounts per provider: how a provider says a request failed on
//! the account's usage limit (rather than a passing rate limit), and what
//! Factory says when it carries on with the next account or has none left.

use serde_json::{Value, json};

use crate::clock::now_ms;
use crate::types::AssistantMessage;

/// The key a provider sets in a failed response's `extra` when the account
/// reached its usage limit: `{"resetsAt": <ms since 1970, or null>}`.
pub const USAGE_LIMIT: &str = "usageLimit";

/// Marks `output` as failed on the account's usage limit, which resets at
/// `resets_at` (ms since 1970) when the provider says.
pub fn mark_usage_limit(output: &mut AssistantMessage, resets_at: Option<u64>) {
    output.extra.insert(USAGE_LIMIT.into(), json!({ "resetsAt": resets_at }));
}

/// The key a failed response's `extra` has when none of the provider's
/// accounts can take a request now: `{"until": <ms since 1970>}`, when the
/// first to reset does, or `{"until": null}` when none resets by itself (out
/// of credit), so it waits for the person.
pub const NONE_LEFT: &str = "noAccountLeft";

/// Marks `output` as failed because no account can take a request until
/// `until` (ms since 1970), or until the person does something (None).
pub fn mark_none_left(output: &mut AssistantMessage, until: Option<u64>) {
    output.extra.insert(NONE_LEFT.into(), json!({ "until": until }));
}

/// Whether the response failed because no account can take a request now:
/// Some, with until when (None: until the person does something).
pub fn none_left(message: &AssistantMessage) -> Option<Option<u64>> {
    let mark = message.extra.get(NONE_LEFT)?;
    Some(mark.get("until").and_then(Value::as_u64).filter(|at| *at > 0))
}

/// The key in an answer's `extra` naming the account that gave it, when the
/// provider has several: the account's id.
pub const ANSWERED_BY: &str = "account";

/// Marks `output` as given by the account `id`.
pub fn mark_answered_by(output: &mut AssistantMessage, id: &str) {
    output.extra.insert(ANSWERED_BY.into(), json!(id));
}

/// The account that gave the answer, when it's known.
pub fn answered_by(message: &AssistantMessage) -> Option<&str> {
    message.extra.get(ANSWERED_BY).and_then(Value::as_str)
}

/// The key a provider sets in a failed response's `extra` when the API key
/// ran out of credit.
pub const OUT_OF_CREDIT: &str = "outOfCredit";

/// Marks `output` as failed because the API key ran out of credit.
pub fn mark_out_of_credit(output: &mut AssistantMessage) {
    output.extra.insert(OUT_OF_CREDIT.into(), json!(true));
}

/// Whether the response failed because the API key ran out of credit.
pub fn out_of_credit(message: &AssistantMessage) -> bool {
    message.extra.get(OUT_OF_CREDIT).and_then(Value::as_bool) == Some(true)
}

/// Whether a provider's error body says the key ran out of credit:
/// Anthropic's "credit balance is too low", OpenAI's `insufficient_quota`,
/// OpenRouter's 402 (insufficient credits).
pub fn credit_error(status: u16, body: &str) -> bool {
    let parsed: Value = serde_json::from_str(body).unwrap_or(Value::Null);
    let error = &parsed["error"];
    let code = error["code"].as_str().or_else(|| error["type"].as_str()).unwrap_or_default();
    let message = error["message"].as_str().unwrap_or(body).to_ascii_lowercase();
    status == 402
        || code == "insufficient_quota"
        || message.contains("credit balance is too low")
        || message.contains("insufficient credits")
        || message.contains("requires more credits")
}

/// The key a failed response's `extra` has when the provider refused its
/// credentials (a key it doesn't know or revoked, a sign-in it ended):
/// `{"account": <the account's id>, "method": "api_key" | "oauth"}`. The
/// provider marks it; the registry, which knows the account, fills them in.
pub const REFUSED: &str = "credentialsRefused";

/// Whose credentials a provider refused.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Refused {
    /// The account's id among the provider's; empty when unknown.
    pub account: String,
    /// How it signed in: "api_key" or "oauth"; empty when unknown.
    pub method: String,
}

/// Marks `output` as failed because the provider refused its credentials.
pub fn mark_refused(output: &mut AssistantMessage, refused: &Refused) {
    output.extra.insert(REFUSED.into(), json!({ "account": refused.account, "method": refused.method }));
}

/// Whose credentials the provider refused, when that's why the response failed.
pub fn refused(message: &AssistantMessage) -> Option<Refused> {
    let mark = message.extra.get(REFUSED)?;
    let text = |key: &str| mark.get(key).and_then(Value::as_str).unwrap_or_default().to_owned();
    Some(Refused { account: text("account"), method: text("method") })
}

/// Whether a provider's error answer refuses the credentials themselves,
/// rather than the request: any 401, or a 403 that says the key or token is
/// invalid or revoked (a 403 is otherwise a permission the key lacks).
pub fn refused_error(status: u16, body: &str) -> bool {
    if status == 401 {
        return true;
    }
    if status != 403 {
        return false;
    }
    let parsed: Value = serde_json::from_str(body).unwrap_or(Value::Null);
    let error = &parsed["error"];
    let code = error["code"].as_str().or_else(|| error["type"].as_str()).or_else(|| error.as_str()).unwrap_or_default();
    let message = error["message"].as_str().unwrap_or(body).to_ascii_lowercase();
    matches!(code, "authentication_error" | "invalid_api_key" | "invalid_token" | "token_revoked" | "token_expired")
        || message.contains("revoked")
        || message.contains("invalid api key")
        || message.contains("invalid x-api-key")
        || message.contains("invalid bearer token")
}

/// An API key as people tell keys apart: "sk-…a1b2".
pub fn key_label(key: &str) -> String {
    let characters: Vec<char> = key.trim().chars().collect();
    let end: String = characters[characters.len().saturating_sub(4)..].iter().collect();
    let start = key.trim().split_once('-').map_or(String::new(), |(start, _)| format!("{start}-"));
    if characters.len() <= 8 { format!("…{end}") } else { format!("{start}…{end}") }
}

/// Whether the response failed on the account's usage limit: Some, with
/// when it resets if the provider said.
pub fn usage_limit(message: &AssistantMessage) -> Option<Option<u64>> {
    let mark = message.extra.get(USAGE_LIMIT)?;
    Some(mark.get("resetsAt").and_then(Value::as_u64).filter(|at| *at > 0))
}

/// What people call a provider's accounts: "Claude", "ChatGPT".
pub fn account_noun(provider_id: &str, provider_name: &str) -> String {
    match provider_id {
        "anthropic" => "Claude".to_owned(),
        "openai-codex" => "ChatGPT".to_owned(),
        _ => provider_name.to_owned(),
    }
}

/// An account as a note names it: its label, or its place in the order.
pub fn account_name(label: &str, position: usize) -> String {
    if label.is_empty() { format!("#{}", position + 1) } else { label.to_owned() }
}

/// "Claude account a@x.com reached its limit; continuing on b@x.com.", or
/// with `credit`, "… ran out of credit; …".
pub fn switch_note(noun: &str, from: &str, to: &str, credit: bool) -> String {
    let what = if credit { "ran out of credit" } else { "reached its limit" };
    format!("{noun} account {from} {what}; continuing on {to}.")
}

/// The error a request ends with when every account is out: when the first
/// to reset does (`first`: its name and when, ms since 1970); `credit`, that
/// some ran out of credit, which doesn't reset by itself. Says "usage limit"
/// or "out of credit", so it isn't retried.
pub fn exhausted_message(noun: &str, accounts: usize, first: Option<(&str, u64)>, credit: bool) -> String {
    let when = first.map(|(name, at)| (name, when(at)));
    let spent = match (first.is_some(), credit) {
        (true, true) => "reached their usage limit or ran out of credit",
        (false, true) => "ran out of credit",
        _ => "reached their usage limit",
    };
    match (accounts, when) {
        (1, Some((_, when))) => format!("Your {noun} account reached its usage limit; it resets {when}."),
        (1, None) if credit => format!("Your {noun} account ran out of credit."),
        (1, None) => format!("Your {noun} account reached its usage limit."),
        (_, Some((name, when))) => format!("All your {noun} accounts {spent}; the first to reset is {name}, {when}."),
        (_, None) => format!("All your {noun} accounts {spent}."),
    }
}

/// "Tue 14:05 (in 2 h 10 min)", in this machine's time zone.
pub fn when(at: u64) -> String {
    let minutes = at.saturating_sub(now_ms()).div_ceil(60_000);
    let left = match (minutes / 60, minutes % 60) {
        (0, minutes) => format!("{minutes} min"),
        (hours, 0) => format!("{hours} h"),
        (hours, minutes) => format!("{hours} h {minutes} min"),
    };
    match local_time(at) {
        Some(time) => format!("{time} (in {left})"),
        None => format!("in {left}"),
    }
}

/// "Tue 14:05": the weekday and time of `at` (ms since 1970) here.
// libc deprecates musl's `time_t` alias ahead of a change to 64 bits; it's
// 64 bits already on the targets Claktory builds (x86_64, aarch64).
#[allow(deprecated)]
pub fn local_time(at: u64) -> Option<String> {
    let seconds = libc::time_t::try_from(at / 1000).ok()?;
    // SAFETY: localtime_r and strftime write only into the buffers given, which outlive the calls.
    unsafe {
        let mut parts: libc::tm = std::mem::zeroed();
        if libc::localtime_r(&seconds, &mut parts).is_null() {
            return None;
        }
        let mut buffer = [0u8; 32];
        let written = libc::strftime(buffer.as_mut_ptr().cast(), buffer.len(), c"%a %H:%M".as_ptr(), &parts);
        (written > 0).then(|| String::from_utf8_lossy(&buffer[..written]).into_owned())
    }
}
