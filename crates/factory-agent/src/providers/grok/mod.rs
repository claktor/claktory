//! Grok on a SuperGrok or X Premium+ subscription: signed in as
//! xAI's own command-line tool signs in (`oauth`), asked through the chat
//! proxy that tool uses, which speaks OpenAI's Responses API. The proxy
//! answers only clients that say which version of the tool they are
//! (`CLIENT_VERSION`). Its models come from the proxy's own list.

pub mod oauth;

use serde_json::Value;

use crate::errors::Error;
use crate::providers::openai::{Dialect, stream_dialect};
use crate::providers::{Provider, oauth as oauth_flow};
use crate::stream::{EventStream, LlmContext, StreamOptions};
use crate::types::{AssistantMessage, Model};

pub use oauth::GrokOAuth;

pub const ID: &str = "grok";
/// The proxy xAI's command-line tool asks.
pub const BASE_URL: &str = "https://cli-chat-proxy.grok.com/v1";
/// The version of xAI's command-line tool Claktory says it is: the proxy
/// turns away a client that names none, or one older than it takes (426,
/// "Your Grok CLI version (none) is outdated"). Claktory says it's
/// the tool. One fixed version, never fetched, worked out or
/// changed after a refusal; moving it on is a reviewed code change. 1.0.45 is the tool's version on 25 Nov 2026.
pub const CLIENT_VERSION: &str = "1.0.45";

/// The headers every request to the proxy carries besides the token: that
/// it's the tool's sign-in, and the tool's version.
pub fn proxy_headers() -> Vec<(String, String)> {
    vec![("X-XAI-Token-Auth".to_owned(), "xai-grok-cli".to_owned()), ("x-grok-client-version".to_owned(), CLIENT_VERSION.to_owned())]
}

pub fn provider() -> Provider {
    let mut provider = Provider::new(ID, "Grok", BASE_URL, None, oauth_flow(GrokOAuth::default()), stream_simple);
    provider.identity = Some(identity);
    provider.plan = Some(plan);
    provider.limits = Some(crate::limits::grok);
    provider
}

/// What the proxy's failed answers mean: 401 is an
/// access token it no longer takes, renewed once as for any provider;
/// 402 the plan's usage used up, a usage limit until the period resets;
/// 408 and 429 (and 5xx) pass, and are waited out; any other 4xx is xAI
/// turning the sign-in away, said as such and never tried again.
fn failed(status: u16, body: &str, output: &mut AssistantMessage) -> Option<String> {
    match status {
        401 | 408 | 429 => None,
        402 => {
            crate::accounts::mark_usage_limit(output, None);
            Some(format!("Your Grok usage is used up for now (402): {}", oauth::said(body)))
        }
        400..=499 => {
            if crate::accounts::refused_error(status, body) {
                crate::accounts::mark_refused(output, &crate::accounts::Refused::default());
            }
            Some(format!("{} ({status}): {}", oauth::REFUSED, oauth::said(body)))
        }
        _ => None,
    }
}

const DIALECT: Dialect = Dialect { prefix: "xAI error", session_headers: false, failed: Some(failed) };

/// The proxy's host: the only one a Grok sign-in's token is ever sent to,
/// since `api:access` may let it use xAI's API,
/// billed apart from the subscription.
pub const PROXY_HOST: &str = "cli-chat-proxy.grok.com";

/// Whether a Grok token may go to `url`: the proxy over https, or, in a
/// debug build only (tests' stand-ins), this computer, so no override in a
/// release can send it to a local listener.
pub fn allowed(url: &str) -> bool {
    let Ok(url) = url::Url::parse(url) else { return false };
    match (url.scheme(), url.host_str()) {
        ("https", Some(host)) => host == PROXY_HOST,
        ("http", Some(host)) => cfg!(debug_assertions) && host == "127.0.0.1",
        _ => false,
    }
}

/// Refused before anything is sent: an address that isn't the proxy's.
pub fn not_allowed(url: &str) -> Error {
    let host = url::Url::parse(url).ok().and_then(|url| url.host_str().map(str::to_owned)).unwrap_or_default();
    Error::Other(format!("A Grok sign-in is used only with Grok's own service, not {host}."))
}

/// How many conversations' ids are kept before they're all forgotten.
const CONVERSATIONS_KEPT: usize = 4096;

/// The conversation id the proxy keeps a prompt cache by
/// (`x-grok-conv-id`): random, one per agent context, a new one once the
/// context starts again (compaction, /clear, new instructions). Made of
/// nothing of Claktory's: only looked up, on this
/// computer, by the session and the context's start. Forgotten at a
/// restart, which costs the cache once.
fn conversation(session: &str, context: &LlmContext) -> String {
    use sha2::Digest;
    static KNOWN: std::sync::LazyLock<parking_lot::Mutex<hashbrown::HashMap<Vec<u8>, String>>> =
        std::sync::LazyLock::new(Default::default);
    let mut hash = sha2::Sha256::new();
    for part in [session.as_bytes(), context.system_prompt.as_deref().unwrap_or_default().as_bytes()] {
        hash.update((part.len() as u64).to_le_bytes());
        hash.update(part);
    }
    let first = context.messages.first().and_then(|message| serde_json::to_vec(message).ok()).unwrap_or_default();
    hash.update((first.len() as u64).to_le_bytes());
    hash.update(&first);
    let key = hash.finalize().to_vec();
    let mut known = KNOWN.lock();
    if known.len() >= CONVERSATIONS_KEPT && !known.contains_key(&key) {
        known.clear();
    }
    known.entry(key).or_insert_with(|| uuid::Uuid::new_v4().to_string()).clone()
}

/// Through the proxy, which routes by the model it's told in a header and
/// keeps a cache per conversation. The agent's session goes no further:
/// the conversation id stands for it (OpenAI's `prompt_cache_key` too).
pub fn stream_simple(model: &Model, context: LlmContext, mut options: StreamOptions) -> Result<EventStream, Error> {
    if !allowed(&model.base_url) {
        return Err(not_allowed(&model.base_url));
    }
    options.headers.retain(|(name, _)| !name.eq_ignore_ascii_case("x-grok-model-override") && !name.eq_ignore_ascii_case("x-grok-conv-id"));
    options.headers.push(("x-grok-model-override".to_owned(), model.id.clone()));
    options.session_id = options.session_id.map(|session| conversation(&session, &context));
    if let Some(conversation) = &options.session_id {
        options.headers.push(("x-grok-conv-id".to_owned(), conversation.clone()));
    }
    stream_dialect(model, context, options, &DIALECT)
}

/// The access token's claims, read without checking its signature: it came
/// straight from xAI, and only says who and what plan.
fn claims(token: &str) -> Option<Value> {
    use base64::Engine;
    let payload = token.split('.').nth(1)?;
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(payload.trim_end_matches('=')).ok()?;
    serde_json::from_slice(&bytes).ok()
}

/// Who a sign-in is, from its token: its user (`sub`) to tell accounts
/// apart, shown as "Grok" and its last four characters, since the scopes
/// asked for give no email or name (never more scopes for a label).
pub fn identity_of(token: &str) -> crate::limits::Identity {
    let sub = claims(token).and_then(|claims| claims["sub"].as_str().map(str::to_owned)).unwrap_or_default();
    let end: String = sub.chars().rev().take(4).collect::<Vec<_>>().into_iter().rev().collect();
    let label = if end.is_empty() { "Grok".to_owned() } else { format!("Grok …{end}") };
    crate::limits::Identity { key: sub, label }
}

/// The plan a sign-in's token names (its `tier` claim), as xAI words it.
pub fn plan_of(token: &str) -> Option<String> {
    claims(token)
        .and_then(|claims| claims["tier"].as_str().map(str::to_owned))
        .map(|tier| tier.chars().filter(|c| !c.is_control()).take(60).collect::<String>())
        .filter(|tier| !tier.trim().is_empty())
}

fn identity(
    _base_url: String,
    auth: crate::auth::RequestAuth,
) -> futures_util::future::BoxFuture<'static, Result<crate::limits::Identity, Error>> {
    Box::pin(async move { Ok(identity_of(auth.api_key.as_deref().unwrap_or_default())) })
}

fn plan(_base_url: String, auth: crate::auth::RequestAuth) -> futures_util::future::BoxFuture<'static, Result<String, Error>> {
    Box::pin(async move { Ok(plan_of(auth.api_key.as_deref().unwrap_or_default()).unwrap_or_default()) })
}
