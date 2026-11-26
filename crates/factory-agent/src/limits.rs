//! How much of a subscription's usage limits is used, which usage resets it
//! has left, and which plan it is: Claude Pro/Max through Anthropic's OAuth
//! usage and profile endpoints, ChatGPT through Codex's usage endpoint and
//! its sign-in's token. And using one of those resets, at the person's word.

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

/// Some of a subscription's usage resets, all of one kind: each refills some
/// of its limits now, once.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct UsageReset {
    /// The provider's id for it; empty lets the provider pick one.
    pub id: String,
    /// "Fable reset", "5-hour reset", "Weekly reset", "Full reset", or the
    /// provider's own title.
    pub label: String,
    /// What using it does: "Refills your 5-hour limit."
    pub detail: String,
    /// How many are left, 1 or more.
    pub left: u32,
    /// How many it came with; 0 when the provider doesn't say.
    pub total: u32,
    /// When the first of them expires, in ms since 1970; 0 when they don't.
    pub expires_at: u64,
    /// The provider says it can be used now.
    pub usable: bool,
    /// Why it can't be used now; empty when `usable`.
    pub unavailable: String,
    /// The `UsageLimit` ids it refills.
    pub window_ids: Vec<String>,
}

/// A subscription's usage: its limits and the resets it has left.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct AccountUsage {
    pub limits: Vec<UsageLimit>,
    pub resets: Vec<UsageReset>,
}

/// What came of using a reset.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ResetAnswer {
    /// A limit was reset.
    pub reset: bool,
    /// What happened, to show as is.
    pub message: String,
    /// Where the person can use it on the provider's site instead, when the
    /// provider refused Claktory or couldn't be reached; empty otherwise.
    pub fallback_url: String,
}

/// Reads a provider's limits and resets with its signed-in credential, against its base url.
pub type LimitsFn = fn(base_url: String, auth: RequestAuth) -> BoxFuture<'static, Result<AccountUsage, Error>>;

/// Uses one of a subscription's resets (as its last usage read listed it).
/// Only ever at the person's word: it can't be undone.
pub type ResetFn = fn(base_url: String, auth: RequestAuth, reset: UsageReset) -> BoxFuture<'static, Result<ResetAnswer, Error>>;

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

/// What an error answer says: its `error.message`, `error`, `detail` or
/// `message`, or else its status's name.
fn error_message(status: reqwest::StatusCode, text: &str) -> String {
    let parsed: Value = serde_json::from_str(text).unwrap_or(Value::Null);
    parsed["error"]["message"]
        .as_str()
        .or_else(|| parsed["error"].as_str())
        .or_else(|| parsed["detail"].as_str())
        .or_else(|| parsed["message"].as_str())
        .or(status.canonical_reason())
        .unwrap_or_default()
        .to_owned()
}

/// `what` names the request in errors: "Anthropic usage", "Anthropic plan".
async fn get_json(what: &str, request: reqwest::RequestBuilder) -> Result<Value, Error> {
    let response = request.timeout(TIMEOUT).send().await.map_err(|error| Error::Other(format!("{what}: {error}")))?;
    let status = response.status();
    let text = response.text().await.unwrap_or_default();
    if !status.is_success() {
        let message = format!("{what}: HTTP {}: {}", status.as_u16(), error_message(status, &text));
        // The access token not taken (expired early, or ended): the caller
        // refreshes it once and asks again.
        if crate::accounts::refused_error(status.as_u16(), &text) {
            return Err(Error::Models { code: crate::errors::UNAUTHORIZED, message });
        }
        return Err(Error::Other(message));
    }
    serde_json::from_str(&text).map_err(|error| Error::Other(format!("{what}: unreadable answer: {error}")))
}

/// How a request to use a reset ended.
#[derive(Debug)]
enum Posted {
    /// The provider answered it; `true` when it took a second try, so an
    /// answer that it was already used may be the first try's doing.
    Answer(Value, bool),
    /// The provider refused it: its status and what it said.
    Refused(u16, String),
    /// No answer: it may or may not have gone through.
    Lost(String),
}

/// How long a request to use a reset may take, as Claude Code allows.
const RESET_TIMEOUT: Duration = Duration::from_secs(25);

/// Sends a request to use a reset; one that gets no answer is sent once
/// more, the same, idempotency id and all.
async fn post_reset(request: reqwest::RequestBuilder) -> Posted {
    let mut lost = String::new();
    for again in [false, true] {
        let Some(attempt) = request.try_clone() else { return Posted::Lost("the request can't be sent".into()) };
        let response = match attempt.timeout(RESET_TIMEOUT).send().await {
            Ok(response) => response,
            Err(error) => {
                lost = error.to_string();
                continue;
            }
        };
        let status = response.status();
        let text = match response.text().await {
            Ok(text) => text,
            Err(error) => {
                lost = error.to_string();
                continue;
            }
        };
        if !status.is_success() {
            return Posted::Refused(status.as_u16(), error_message(status, &text));
        }
        return match serde_json::from_str(&text) {
            Ok(answer) => Posted::Answer(answer, again),
            Err(error) => Posted::Lost(format!("unreadable answer: {error}")),
        };
    }
    Posted::Lost(lost)
}

/// "1 five-hour reset left", "2 Fable resets left", "No full resets left".
fn resets_left(left: u64, noun: &str) -> String {
    match left {
        0 => format!("No {noun}s left."),
        1 => format!("1 {noun} left."),
        _ => format!("{left} {noun}s left."),
    }
}

fn answer(reset: bool, message: impl Into<String>) -> ResetAnswer {
    ResetAnswer { reset, message: message.into(), fallback_url: String::new() }
}

fn refused(message: impl Into<String>, fallback_url: &str) -> ResetAnswer {
    ResetAnswer { reset: false, message: message.into(), fallback_url: fallback_url.to_owned() }
}

/// When no answer came back.
const UNCONFIRMED: &str = "Couldn't confirm that the reset went through. Claktory is reading your usage again; if you're still at the limit in a minute, try again.";

// Anthropic (Claude Pro/Max).

const ANTHROPIC_WINDOWS: [(&str, &str, &str); 5] = [
    ("seven_day", "weekly", "Weekly"),
    ("five_hour", "five_hour", "5-hour"),
    ("seven_day_opus", "weekly_opus", "Weekly (Opus)"),
    ("seven_day_sonnet", "weekly_sonnet", "Weekly (Sonnet)"),
    ("seven_day_oauth_apps", "weekly_oauth_apps", "Weekly (OAuth apps)"),
];

/// Claude Code's own user agent: Anthropic tells which app is asking by it
/// (and `x-app`), and offers usage resets only to Claude Code.
fn claude_code_agent() -> String {
    format!("claude-cli/{} (external, cli)", crate::providers::anthropic::CLAUDE_CODE_VERSION)
}

/// GETs one of Anthropic's OAuth account endpoints, `/api/oauth/<path>`.
async fn anthropic_get(what: &str, path: &str, base_url: &str, auth: RequestAuth) -> Result<Value, Error> {
    let token = auth.api_key.ok_or_else(|| Error::Other(format!("{what}: not signed in")))?;
    let request = client()
        .get(format!("{}/api/oauth/{path}", base_url.trim_end_matches('/')))
        .bearer_auth(token)
        .header("anthropic-beta", "oauth-2025-04-20")
        // As Claude Code asks: never a copy kept from before a change of plan.
        .header("cache-control", "no-cache")
        .header("user-agent", claude_code_agent())
        .header("x-app", "cli");
    get_json(what, request).await
}

/// The usage, read as Claude Code reads it for its resets ("cedar ember"):
/// with them, and without the spend it doesn't show.
pub fn anthropic(base_url: String, auth: RequestAuth) -> BoxFuture<'static, Result<AccountUsage, Error>> {
    Box::pin(async move {
        let usage = anthropic_get("Anthropic usage", "usage?cedar_ember=1&skip_spend=1", &base_url, auth).await?;
        Ok(AccountUsage { limits: parse_anthropic(&usage), resets: parse_anthropic_resets(&usage) })
    })
}

/// Where Claude's resets can be used on claude.ai.
const CLAUDE_RESETS: &str = "https://clau.de/reset";

/// A Claude reset's kind, as Claude Code tells them apart: one that gives
/// back what Fable used; else its `type`; else by what it clears.
#[derive(Clone, Copy, Debug, PartialEq)]
enum ResetKind {
    Session,
    Weekly,
    Fable,
    Full,
}

impl ResetKind {
    fn of(grant: &Value, clears: &[String]) -> ResetKind {
        if grant["gives_back_weekly"].as_bool() == Some(true) {
            return ResetKind::Fable;
        }
        match grant["type"].as_str() {
            Some("session") => ResetKind::Session,
            Some("weekly") => ResetKind::Weekly,
            Some("full") => ResetKind::Full,
            _ => {
                let session = clears.iter().any(|window| window == "five_hour");
                let other = clears.iter().any(|window| window != "five_hour");
                match (session, other) {
                    (true, false) => ResetKind::Session,
                    (false, true) => ResetKind::Weekly,
                    _ => ResetKind::Full,
                }
            }
        }
    }

    fn by_label(label: &str) -> ResetKind {
        match label {
            "5-hour reset" => ResetKind::Session,
            "Weekly reset" => ResetKind::Weekly,
            "Fable reset" => ResetKind::Fable,
            _ => ResetKind::Full,
        }
    }

    fn label(self) -> &'static str {
        match self {
            ResetKind::Session => "5-hour reset",
            ResetKind::Weekly => "Weekly reset",
            ResetKind::Fable => "Fable reset",
            ResetKind::Full => "Full reset",
        }
    }

    /// How a count of them reads: "2 five-hour resets".
    fn noun(self) -> &'static str {
        match self {
            ResetKind::Session => "five-hour reset",
            ResetKind::Weekly => "weekly reset",
            ResetKind::Fable => "Fable reset",
            ResetKind::Full => "full reset",
        }
    }

    fn detail(self) -> &'static str {
        match self {
            ResetKind::Session => "Refills your 5-hour limit.",
            ResetKind::Weekly => "Refills your weekly limits.",
            ResetKind::Fable => "Gives back to your weekly limit what Fable used.",
            ResetKind::Full => "Refills all your limits.",
        }
    }

    fn done(self) -> &'static str {
        match self {
            ResetKind::Session => "Your 5-hour limit is reset.",
            ResetKind::Weekly => "Your weekly limits are reset.",
            ResetKind::Fable => "What Fable used is back in your weekly limit.",
            ResetKind::Full => "Your limits are reset.",
        }
    }
}

/// A list of strings in a JSON value; empty when it isn't one.
fn strings(value: &Value) -> Vec<String> {
    value.as_array().into_iter().flatten().filter_map(Value::as_str).map(str::to_owned).collect()
}

/// The `UsageLimit` id of one of Anthropic's windows in `clears`.
fn anthropic_window(key: &str) -> Option<&'static str> {
    match key {
        // Fable's weekly limit.
        "seven_day_overage_included" => Some("weekly_fable"),
        _ => ANTHROPIC_WINDOWS.iter().find(|(window, ..)| *window == key).map(|(_, id, _)| *id),
    }
}

/// Why a grant can't be used now, as Claude Code tells it: what Anthropic
/// says, else a reset used a moment ago, a limit it can't be used at, a
/// limit it doesn't refill, or that it works only at a limit.
fn grant_unavailable(status: &Value, grant: &Value, clears: &[String], fable: bool) -> &'static str {
    if grant["paused"].as_bool() == Some(true) {
        return "It's paused for now.";
    }
    let blocking = strings(&grant["blocking"]);
    let reason = grant["unavailable_reason"].as_str().map(str::to_owned).unwrap_or_else(|| {
        let exhausted = strings(&status["exhausted"]);
        let clears_it = |window: &String| clears.contains(window) || (fable && window == "seven_day");
        if !status["cooldown_until"].is_null() {
            "cooldown".into()
        } else if let Some(window) = blocking.first() {
            match window.as_str() {
                "five_hour" => "blocked_five_hour".into(),
                "seven_day" => "blocked_seven_day".into(),
                "seven_day_opus" => "blocked_opus".into(),
                "seven_day_sonnet" => "blocked_sonnet".into(),
                _ => "blocked_other".into(),
            }
        } else if status["at_limit"].as_bool() == Some(true) && !exhausted.iter().any(clears_it) {
            "cannot_clear".into()
        } else if grant["use_requires_limit"].as_bool() != Some(false) {
            "needs_limit".into()
        } else {
            String::new()
        }
    });
    match reason.as_str() {
        "cooldown" => "A reset was just used on this account. Try again in a minute.",
        "blocked_five_hour" => "It can't be used while you're at your 5-hour limit.",
        "blocked_seven_day" => "It can't be used while you're at your weekly limit.",
        "blocked_opus" => "It can't be used while you're at your weekly Opus limit.",
        "blocked_sonnet" => "It can't be used while you're at your weekly Sonnet limit.",
        "blocked_other" => "It can't be used while you're at one of your other limits.",
        "cannot_clear" => "It doesn't refill the limit you're at.",
        "needs_limit" => "It works only at a usage limit.",
        _ => "Anthropic says it can't be used right now.",
    }
}

/// The resets in the usage's `cedar_ember` block: each grant with resets
/// left, none when the account isn't eligible. Claude Code's own reading.
pub fn parse_anthropic_resets(usage: &Value) -> Vec<UsageReset> {
    let status = &usage["cedar_ember"];
    if status["eligible"].as_bool() != Some(true) {
        return Vec::new();
    }
    let mut resets = Vec::new();
    for grant in status["grants"].as_array().into_iter().flatten() {
        let Some(id) = grant["id"].as_str().filter(|id| grant_id(id)) else { continue };
        let left = grant["resets_left"].as_u64().unwrap_or(0);
        if left == 0 {
            continue;
        }
        let clears = strings(&grant["clears"]);
        let kind = ResetKind::of(grant, &clears);
        let mut window_ids: Vec<String> = Vec::new();
        let fable = kind == ResetKind::Fable;
        let given_back = if fable { vec!["weekly_fable", "weekly"] } else { Vec::new() };
        for id in given_back.into_iter().chain(clears.iter().filter_map(|window| anthropic_window(window))) {
            if !window_ids.iter().any(|known| known == id) {
                window_ids.push(id.to_owned());
            }
        }
        let usable = grant["usable_now"].as_bool() == Some(true) && grant["paused"].as_bool() != Some(true);
        resets.push(UsageReset {
            id: id.to_owned(),
            label: kind.label().to_owned(),
            detail: if fable && clears.iter().any(|window| window == "five_hour") {
                format!("{} It also refills your 5-hour limit.", kind.detail())
            } else {
                kind.detail().to_owned()
            },
            left: u32::try_from(left).unwrap_or(u32::MAX),
            total: grant["resets_total"].as_u64().and_then(|total| u32::try_from(total).ok()).unwrap_or(0),
            expires_at: grant["ends_at"].as_str().and_then(parse_iso_ms).unwrap_or(0),
            usable,
            unavailable: if usable { String::new() } else { grant_unavailable(status, grant, &clears, fable).to_owned() },
            window_ids,
        });
    }
    resets
}

/// A grant id as Anthropic takes it: `[a-z0-9_-]{1,40}`.
fn grant_id(id: &str) -> bool {
    (1..=40).contains(&id.len()) && id.bytes().all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'_' | b'-'))
}

/// Uses one of a Claude account's resets: Claude Code's claim, for the
/// account's organization, once (sent again only when no answer came, with
/// the same request id, so it's never used twice).
pub fn anthropic_reset(base_url: String, auth: RequestAuth, reset: UsageReset) -> BoxFuture<'static, Result<ResetAnswer, Error>> {
    Box::pin(async move {
        if !grant_id(&reset.id) {
            return Ok(answer(false, "That reset isn't one Anthropic offers. Nothing was used."));
        }
        let profile = anthropic_get("Anthropic account", "profile", &base_url, auth.clone()).await?;
        let Some(organization) = profile["organization"]["uuid"].as_str().filter(|uuid| !uuid.is_empty()) else {
            return Ok(refused("Anthropic didn't say which organization this account is in. Nothing was used.", CLAUDE_RESETS));
        };
        let read_auth = auth.clone();
        let token = auth.api_key.ok_or_else(|| Error::Other("Anthropic reset: not signed in".into()))?;
        let url = format!("{}/api/organizations/{organization}/reset_rate_limits", base_url.trim_end_matches('/'));
        let body = serde_json::json!({
            "program": "cedar_ember",
            "grant_id": reset.id,
            "request_id": uuid::Uuid::new_v4().simple().to_string(),
        });
        let request = client()
            .post(url)
            .bearer_auth(token)
            .header("anthropic-beta", "oauth-2025-04-20")
            .header("user-agent", claude_code_agent())
            .header("x-app", "cli")
            .json(&body);
        let posted = post_reset(request).await;
        // The count people see comes from a fresh usage read: the claim's
        // own resets_left counts only the grant it used, and another grant
        // of the same kind may hold more (#230).
        let fresh_left = match &posted {
            Posted::Answer(claim, _) => fresh_resets_left(&base_url, read_auth, &reset, claim).await,
            _ => None,
        };
        Ok(anthropic_reset_answer(&reset, posted, fresh_left))
    })
}

/// How many resets of `reset`'s kind the account still has, read afresh
/// after a claim: Anthropic's count across every grant of the kind, with
/// the used grant itself never above what the claim answered for it (the
/// usage read can lag the claim by a moment). None when the read fails,
/// and the answer falls back to the claim's own count.
async fn fresh_resets_left(base_url: &str, auth: RequestAuth, reset: &UsageReset, claim: &Value) -> Option<u64> {
    let usage = match anthropic_get("Anthropic usage", "usage?cedar_ember=1&skip_spend=1", base_url, auth).await {
        Ok(usage) => usage,
        Err(error) => {
            tracing::warn!(%error, "could not read the usage after a Claude reset");
            return None;
        }
    };
    let claimed = claim["resets_left"].as_u64();
    let mut left = 0;
    for fresh in parse_anthropic_resets(&usage) {
        if fresh.label != reset.label {
            continue;
        }
        let count = u64::from(fresh.left);
        left += if fresh.id == reset.id { count.min(claimed.unwrap_or(count)) } else { count };
    }
    Some(left)
}

/// What Anthropic's answer to a claim means for the person. `fresh_left`
/// is the account's count of this kind from a usage read after the claim
/// (see `fresh_resets_left`); without it, the claim's grant-scoped count.
fn anthropic_reset_answer(reset: &UsageReset, posted: Posted, fresh_left: Option<u64>) -> ResetAnswer {
    let kind = ResetKind::by_label(&reset.label);
    let (claim, again) = match posted {
        Posted::Answer(claim, again) => (claim, again),
        Posted::Refused(429, _) => {
            return answer(false, "Anthropic is getting too many requests right now. Nothing was used; try again in a moment.");
        }
        Posted::Refused(401 | 403, _) => {
            return refused(
                "Anthropic didn't accept Claktory's request to use a reset. Nothing was used; you can use it on claude.ai.",
                CLAUDE_RESETS,
            );
        }
        Posted::Refused(status, message) => {
            let message = message.trim_end_matches('.');
            return refused(
                format!("Anthropic couldn't use the reset ({status}: {message}). Nothing was used; you can use it on claude.ai."),
                CLAUDE_RESETS,
            );
        }
        Posted::Lost(error) => {
            tracing::warn!(%error, "no answer to a Claude reset");
            return refused(UNCONFIRMED, CLAUDE_RESETS);
        }
    };
    let left = fresh_left.or(claim["resets_left"].as_u64());
    let left_line = |left: u64| resets_left(left, kind.noun());
    match claim["result"].as_str().unwrap_or("unavailable") {
        "reset" => {
            let left = left.unwrap_or(u64::from(reset.left.saturating_sub(1)));
            answer(true, format!("{} {}", kind.done(), left_line(left)))
        }
        // A second try finds what the first one did.
        "already_used" if again => answer(true, format!("{} {}", kind.done(), left_line(left.unwrap_or(0)))),
        "already_used" => {
            let left = left.map(|left| format!(" {}", left_line(left))).unwrap_or_default();
            answer(false, format!("That reset was already used, so nothing changed.{left}"))
        }
        "not_limited" => answer(false, "You're not at a limit this reset refills, so nothing was used."),
        "cooldown" => answer(false, "Another reset was just used on this account. This one wasn't; try again in a minute."),
        "ineligible" => refused(
            "Anthropic says this reset isn't available to Claktory now. Nothing was used; you can use it on claude.ai.",
            CLAUDE_RESETS,
        ),
        _ => refused(
            "Anthropic couldn't use the reset right now. Nothing was used; try again in a moment, or use it on claude.ai.",
            CLAUDE_RESETS,
        ),
    }
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

/// The plan in Anthropic's OAuth profile, as Anthropic names it: the
/// organization's `plan_display_name` ("Ultra" -> "Claude Ultra"), as
/// Claude Code shows it; or else its `rate_limit_tier`, which names the plan
/// ("default_claude_max_20x" -> "Claude Max 20x"); or else its
/// `organization_type` ("claude_team" -> "Claude Team"), and last the
/// account's `has_claude_max`/`has_claude_pro`. Never a plan it didn't name:
/// a tier Claktory doesn't know reads as the tier says, not as Max.
pub fn parse_anthropic_plan(profile: &Value) -> String {
    let organization = &profile["organization"];
    if let Some(name) = organization["plan_display_name"].as_str().map(str::trim).filter(|name| plan_name(name)) {
        return if name.starts_with("Claude") { name.to_owned() } else { format!("Claude {name}") };
    }
    let tier = tier_plan(organization["rate_limit_tier"].as_str().unwrap_or_default());
    match organization["organization_type"].as_str().unwrap_or_default() {
        "claude_max" => tier.unwrap_or_else(|| "Claude Max".to_owned()),
        kind if kind.starts_with("claude_") => tier.unwrap_or_else(|| title(kind)),
        _ if profile["account"]["has_claude_max"].as_bool() == Some(true) => tier.unwrap_or_else(|| "Claude Max".to_owned()),
        _ if profile["account"]["has_claude_pro"].as_bool() == Some(true) => "Claude Pro".to_owned(),
        _ => String::new(),
    }
}

/// Whether an Anthropic subscription's organization is the person's own: a
/// Pro or Max plan (`organization_type`, whatever name the plan shows,
/// "Ultra" included). A Team's or an Enterprise's is shared with others,
/// and so is one it doesn't say (#382: images kept as files are reachable
/// to everyone with access to the organization's workspace).
pub fn parse_anthropic_personal(profile: &Value) -> bool {
    matches!(profile["organization"]["organization_type"].as_str(), Some("claude_max" | "claude_pro"))
}

/// Reads whether an Anthropic subscription sign-in is the person's own
/// (`parse_anthropic_personal`).
pub fn anthropic_personal(base_url: String, auth: RequestAuth) -> BoxFuture<'static, Result<bool, Error>> {
    Box::pin(async move { Ok(parse_anthropic_personal(&anthropic_get("Anthropic plan", "profile", &base_url, auth).await?)) })
}

/// Whether Anthropic's `plan_display_name` is a name to show, by the rule
/// Claude Code checks it with: letters, digits, spaces and . + -, at most 40,
/// starting with a letter or digit.
fn plan_name(name: &str) -> bool {
    let mut characters = name.chars();
    characters.next().is_some_and(char::is_alphanumeric)
        && name.chars().count() <= 40
        && name.chars().all(|c| c.is_alphanumeric() || matches!(c, ' ' | '.' | '+' | '-'))
        && !name.ends_with([' ', '.', '-'])
}

/// The plan a subscription's `rate_limit_tier` names: "default_claude_max_20x"
/// -> "Claude Max 20x", "default_claude_ultra" -> "Claude Ultra". None for a
/// tier that names no plan (Pro's "default_claude_ai", or none).
fn tier_plan(tier: &str) -> Option<String> {
    let plan = tier.trim().strip_prefix("default_").unwrap_or(tier.trim());
    if !plan.starts_with("claude_") || matches!(plan, "claude_ai" | "claude_zero") {
        return None;
    }
    let words: Vec<String> = plan
        .split('_')
        .filter(|word| !word.is_empty())
        .map(|word| {
            // "20x" stays as it is.
            if word.trim_end_matches('x').chars().all(|c| c.is_ascii_digit()) { word.to_owned() } else { title(word) }
        })
        .collect();
    (words.len() > 1).then(|| words.join(" "))
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

/// A request to one of Codex's account endpoints (`path` after
/// `/wham/` on ChatGPT's backend, `/api/codex/` elsewhere), as Codex's own
/// client sends it.
fn codex_request(
    what: &str,
    method: reqwest::Method,
    path: &str,
    base_url: &str,
    auth: &RequestAuth,
) -> Result<reqwest::RequestBuilder, Error> {
    let token = auth.api_key.as_deref().ok_or_else(|| Error::Other(format!("{what}: not signed in")))?;
    let account = crate::providers::openai_codex::oauth::account_id(token)
        .ok_or_else(|| Error::Other(format!("{what}: the sign-in has no ChatGPT account")))?;
    let base = base_url.trim_end_matches('/');
    let url = if base.contains("/backend-api") { format!("{base}/wham/{path}") } else { format!("{base}/api/codex/{path}") };
    Ok(client()
        .request(method, url)
        .bearer_auth(token)
        .header("chatgpt-account-id", account)
        .header("originator", "pi")
        .header("cache-control", "no-cache")
        .header("User-Agent", user_agent()))
}

/// GETs Codex's usage endpoint.
async fn codex_usage(what: &str, base_url: &str, auth: RequestAuth) -> Result<Value, Error> {
    get_json(what, codex_request(what, reqwest::Method::GET, "usage", base_url, &auth)?).await
}

/// Where ChatGPT's usage and resets show on its site.
const CHATGPT_USAGE: &str = "https://chatgpt.com/codex/settings/usage";

/// The usage, with the resets it counts (`rate_limit_reset_credits`), told
/// apart by their list when it has any.
pub fn codex(base_url: String, auth: RequestAuth) -> BoxFuture<'static, Result<AccountUsage, Error>> {
    Box::pin(async move {
        let usage = codex_usage("OpenAI Codex usage", &base_url, auth.clone()).await?;
        let limits = parse_codex(&usage, now_ms());
        let available = usage["rate_limit_reset_credits"]["available_count"].as_u64().unwrap_or(0);
        if available == 0 {
            return Ok(AccountUsage { limits, resets: Vec::new() });
        }
        let what = "OpenAI Codex resets";
        let list = match codex_request(what, reqwest::Method::GET, "rate-limit-reset-credits", &base_url, &auth) {
            Ok(request) => get_json(what, request).await,
            Err(error) => Err(error),
        };
        let list = list.unwrap_or_else(|error| {
            // The count alone still says how many there are.
            tracing::warn!(%error, "could not list ChatGPT's resets");
            Value::Null
        });
        let resets = parse_codex_resets(available, &list, &limits);
        Ok(AccountUsage { limits, resets })
    })
}

/// ChatGPT's resets: the `available_count` from the usage, told apart by
/// the list's `available` credits (`{id, title, description, expires_at}`),
/// one entry per title and description, with the id of the one that
/// expires first. Each refills the account's own limits (not a model's).
pub fn parse_codex_resets(available: u64, list: &Value, limits: &[UsageLimit]) -> Vec<UsageReset> {
    let window_ids: Vec<String> = limits.iter().filter(|limit| !limit.label.contains('(')).map(|limit| limit.id.clone()).collect();
    let text = |credit: &Value, key: &str, otherwise: &str| {
        credit[key].as_str().map(str::trim).filter(|text| !text.is_empty()).unwrap_or(otherwise).to_owned()
    };
    let mut credits: Vec<&Value> = list["credits"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|credit| credit["status"].as_str() == Some("available") && credit["id"].as_str().is_some_and(|id| !id.is_empty()))
        .collect();
    credits.sort_by_key(|credit| credit["expires_at"].as_str().and_then(parse_iso_ms).unwrap_or(u64::MAX));
    let mut resets: Vec<UsageReset> = Vec::new();
    for credit in credits {
        let label = text(credit, "title", "Full reset");
        let detail = text(credit, "description", "Refills your current usage limits.");
        if let Some(known) = resets.iter_mut().find(|known| known.label == label && known.detail == detail) {
            known.left += 1;
            continue;
        }
        resets.push(UsageReset {
            id: credit["id"].as_str().unwrap_or_default().to_owned(),
            label,
            detail,
            left: 1,
            total: 0,
            expires_at: credit["expires_at"].as_str().and_then(parse_iso_ms).unwrap_or(0),
            // Only ChatGPT knows: it takes one once a limit is mostly used.
            usable: true,
            unavailable: String::new(),
            window_ids: window_ids.clone(),
        });
    }
    let listed: u64 = resets.iter().map(|reset| u64::from(reset.left)).sum();
    let available = u32::try_from(available).unwrap_or(u32::MAX);
    match resets.as_mut_slice() {
        // None listed: ChatGPT picks one.
        [] => resets.push(UsageReset {
            label: "Full reset".into(),
            detail: "Refills your current usage limits.".into(),
            left: available,
            usable: true,
            window_ids,
            ..UsageReset::default()
        }),
        // The list may stop short of them all.
        [only] if listed < u64::from(available) => only.left = available,
        _ => {}
    }
    resets
}

/// Uses one of a ChatGPT account's resets (the one given, or else one
/// ChatGPT picks), sent again only when no answer came, with the same
/// redeem id, so it's never used twice.
pub fn codex_reset(base_url: String, auth: RequestAuth, reset: UsageReset) -> BoxFuture<'static, Result<ResetAnswer, Error>> {
    Box::pin(async move {
        let what = "OpenAI Codex reset";
        let request = codex_request(what, reqwest::Method::POST, "rate-limit-reset-credits/consume", &base_url, &auth)?;
        let mut body = serde_json::json!({ "redeem_request_id": uuid::Uuid::new_v4().to_string() });
        if !reset.id.is_empty() {
            body["credit_id"] = Value::String(reset.id.clone());
        }
        let posted = post_reset(request.json(&body)).await;
        Ok(codex_reset_answer(posted))
    })
}

/// What ChatGPT's answer to a reset means for the person.
fn codex_reset_answer(posted: Posted) -> ResetAnswer {
    let (consumed, again) = match posted {
        Posted::Answer(consumed, again) => (consumed, again),
        Posted::Refused(429, _) => {
            return answer(false, "ChatGPT is getting too many requests right now. Nothing was used; try again in a moment.");
        }
        Posted::Refused(status, message) => {
            let message = message.trim_end_matches('.');
            return refused(
                format!("ChatGPT couldn't use the reset ({status}: {message}). Nothing was used; you can use it on chatgpt.com."),
                CHATGPT_USAGE,
            );
        }
        Posted::Lost(error) => {
            tracing::warn!(%error, "no answer to a ChatGPT reset");
            return refused(UNCONFIRMED, CHATGPT_USAGE);
        }
    };
    match consumed["code"].as_str().unwrap_or_default() {
        "reset" => answer(true, "Your usage limits are reset."),
        // A second try finds what the first one did.
        "already_redeemed" if again => answer(true, "Your usage limits are reset."),
        "already_redeemed" => answer(false, "That reset was already used, so nothing changed."),
        "nothing_to_reset" => {
            answer(false, "Your usage doesn't need a reset yet: ChatGPT takes one once most of a limit is used. Nothing was used.")
        }
        "no_credit" => answer(false, "This account has no resets left. Nothing was used."),
        _ => refused(
            "ChatGPT couldn't use the reset right now. Nothing was used; try again in a moment, or use it on chatgpt.com.",
            CHATGPT_USAGE,
        ),
    }
}

/// The plan as Codex's usage endpoint says it now (`plan_type`), so an
/// upgrade shows without signing in again; when it can't say, the access
/// token's `chatgpt_plan_type` claim, from when the token was issued.
pub fn codex_plan(base_url: String, auth: RequestAuth) -> BoxFuture<'static, Result<String, Error>> {
    Box::pin(async move {
        let claimed = auth.api_key.as_deref().and_then(crate::providers::openai_codex::oauth::plan_type).map(|plan| chatgpt_plan(&plan));
        let live = match codex_usage("OpenAI Codex plan", &base_url, auth).await {
            Ok(usage) => usage["plan_type"].as_str().map(chatgpt_plan).filter(|plan| !plan.is_empty()),
            Err(error) if claimed.is_none() => return Err(error),
            Err(_) => None,
        };
        Ok(live.or(claimed).unwrap_or_default())
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

/// Grok's plan meter (#493): the proxy's `billing?format=credits`, which
/// xAI's command-line tool reads, asked as the tool's sign-in. Its usage
/// pool for the period (weekly on SuperGrok) and when the period ends.
pub fn grok(base_url: String, auth: RequestAuth) -> BoxFuture<'static, Result<AccountUsage, Error>> {
    Box::pin(async move {
        let what = "Grok usage";
        if !crate::providers::grok::allowed(&base_url) {
            return Err(crate::providers::grok::not_allowed(&base_url));
        }
        let token = auth.api_key.as_deref().ok_or_else(|| Error::Other(format!("{what}: not signed in")))?;
        let mut request = client().get(format!("{}/billing?format=credits", base_url.trim_end_matches('/'))).bearer_auth(token);
        for (name, value) in &auth.headers {
            request = request.header(name, value);
        }
        let usage = get_json(what, request).await?;
        Ok(AccountUsage { limits: parse_grok(&usage), resets: Vec::new() })
    })
}

/// The meter's `config`: `creditUsagePercent` of the `currentPeriod`
/// (`type` USAGE_PERIOD_TYPE_WEEKLY, say), which resets at its `end`. No
/// limits when it says no percent.
pub fn parse_grok(usage: &Value) -> Vec<UsageLimit> {
    let config = &usage["config"];
    let Some(used) = config["creditUsagePercent"].as_f64() else { return Vec::new() };
    let period = &config["currentPeriod"];
    let kind = period["type"].as_str().unwrap_or_default().trim_start_matches("USAGE_PERIOD_TYPE_").to_ascii_lowercase();
    let (id, label) = match kind.as_str() {
        "daily" => ("daily", "Daily"),
        "weekly" => ("weekly", "Weekly"),
        "monthly" => ("monthly", "Monthly"),
        _ => ("usage", "Usage"),
    };
    let resets_at = period["end"].as_str().and_then(parse_iso_ms).unwrap_or(0);
    vec![limit(id, label, used, resets_at)]
}
