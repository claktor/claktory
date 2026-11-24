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

/// Adds to a usage limit's mark (`mark_usage_limit`) which limit the
/// provider says it was: Anthropic's representative claim, such as
/// "seven_day_opus" for the weekly limit of Opus models alone.
pub fn mark_usage_limit_claim(output: &mut AssistantMessage, claim: &str) {
    if let Some(mark) = output.extra.get_mut(USAGE_LIMIT).and_then(Value::as_object_mut) {
        mark.insert("claim".into(), json!(claim));
    }
}

/// The limit a usage limit's mark names (`mark_usage_limit_claim`).
pub fn usage_limit_claim(message: &AssistantMessage) -> Option<&str> {
    message.extra.get(USAGE_LIMIT)?.get("claim")?.as_str()
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

/// How long a key whose in-flight spending is full is skipped when
/// OpenRouter doesn't say (Retry-After).
const IN_FLIGHT_WAIT_MS: u64 = 30_000;

/// Whether an OpenRouter error answer (402 or 403, branching on
/// `error.metadata.limit_source` as its docs say) is a spending cap on the
/// key rather than the account's balance: Some with when the key can
/// spend again, ms since 1970, or Some(None) for a cap that never resets
/// (a lifetime budget), which counts as out of credit. The key's own limit
/// and an org's guardrail or workspace budget reset at `metadata.resets_at`;
/// the in-flight budget (running requests' estimated costs) frees up after
/// `retry_after_ms`. None for anything else, the balance included.
pub fn spending_limit(status: u16, body: &str, retry_after_ms: Option<u64>) -> Option<Option<u64>> {
    if status != 402 && status != 403 {
        return None;
    }
    let parsed: Value = serde_json::from_str(body).unwrap_or(Value::Null);
    let error = &parsed["error"];
    let metadata = &error["metadata"];
    match metadata["limit_source"].as_str() {
        Some("openrouter_in_flight_budget") => Some(Some(now_ms() + retry_after_ms.unwrap_or(IN_FLIGHT_WAIT_MS))),
        Some("openrouter_key_limit" | "openrouter_guardrail_budget" | "openrouter_workspace_budget") => {
            Some(metadata["resets_at"].as_str().and_then(crate::clock::parse_iso_ms))
        }
        Some(_) => None,
        // Without metadata, as OpenRouter answered before saying the source:
        // "Key limit exceeded (daily limit)", "… budget limit exceeded …".
        None => {
            let message = error["message"].as_str().unwrap_or(body).to_ascii_lowercase();
            (status == 403 && (message.contains("key limit exceeded") || message.contains("budget limit exceeded"))).then_some(None)
        }
    }
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
/// some ran out of credit, which doesn't reset by itself; `limit`, the
/// limit reached: "usage limit", or a model's own ("weekly Opus limit").
/// Says "limit" or "out of credit", so it isn't retried.
pub fn exhausted_message(noun: &str, accounts: usize, first: Option<(&str, u64)>, credit: bool, keys: bool, limit: &str) -> String {
    let when = first.map(|(name, at)| (name, when(at)));
    // API keys have no usage limits: theirs is a spending limit set on the
    // key (OpenRouter's), or its in-flight budget.
    if keys && let Some((_, when)) = &when {
        return match (accounts, credit) {
            (1, _) => format!("Your {noun} key reached its spending limit; it resets {when}."),
            (_, true) => {
                format!("All your {noun} keys reached their spending limit or ran out of credit; the first resets {when}.")
            }
            (_, false) => format!("All your {noun} keys reached their spending limit; the first resets {when}."),
        };
    }
    let spent = match (first.is_some(), credit) {
        (true, true) => format!("reached their {limit} or ran out of credit"),
        (false, true) => "ran out of credit".to_owned(),
        _ => format!("reached their {limit}"),
    };
    match (accounts, when) {
        (1, Some((_, when))) => format!("Your {noun} account reached its {limit}; it resets {when}."),
        (1, None) if credit => format!("Your {noun} account ran out of credit."),
        (1, None) => format!("Your {noun} account reached its {limit}."),
        (_, Some((name, when))) => format!("All your {noun} accounts {spent}; the first to reset is {name}, {when}."),
        (_, None) => format!("All your {noun} accounts {spent}."),
    }
}

/// "Tue 14:05", in this machine's time zone; "in 2 h 10 min" only when
/// its time can't be read. Never both: the message is kept (a transcript's
/// notice, an error), and a time from now would be wrong once read later.
pub fn when(at: u64) -> String {
    local_time(at).unwrap_or_else(|| format!("in {}", left(at.saturating_sub(now_ms()))))
}

/// "2 h 10 min": `ms` to the nearest minute, at least 1. Not rounded up:
/// a reset given to the second, set 10 minutes ahead, is a little over 10
/// minutes away when it's read, and reads "10 min", not 11.
fn left(ms: u64) -> String {
    let minutes = (ms.saturating_add(30_000) / 60_000).max(1);
    match (minutes / 60, minutes % 60) {
        (0, minutes) => format!("{minutes} min"),
        (hours, 0) => format!("{hours} h"),
        (hours, minutes) => format!("{hours} h {minutes} min"),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_time_left_reads_to_the_nearest_minute() {
        // A 10-minute limit's reset, rounded up to the second, read at once.
        assert_eq!(left(600_900), "10 min");
        assert_eq!(left(120_999), "2 min");
        assert_eq!(left(569_000), "9 min");
        assert_eq!(left(571_000), "10 min");
        assert_eq!(left(20_000), "1 min");
        assert_eq!(left(0), "1 min");
        assert_eq!(left(3_600_400), "1 h");
        assert_eq!(left((2 * 60 + 10) * 60_000 + 700), "2 h 10 min");
        // Only the time itself, which stays true once kept.
        let at = now_ms() + 600_900;
        assert_eq!(when(at), local_time(at).expect("a time here"));
        assert!(!when(at).contains("(in"));
    }
}
