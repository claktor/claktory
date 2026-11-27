//! What Claktory reads of its browser itself (#435 P4), over the runner's
//! CDP channel while Chrome is driven, with ids of its own so the replies
//! never reach the errand: the sites it's signed in to (the origins its
//! cookies are for, never a cookie's name or value: R8.1, Fugo 36537), and
//! the page shown (its origin and title, for each frame). Everything read
//! is a page's: cleaned and capped before it's kept.

use std::sync::Arc;

use serde_json::{Value, json};

use super::{Inner, view};
use crate::browser_runner::wire::{Channel, MAX_CDP};
use crate::factory::Factory;

/// Ids of the host's own calls: above any an errand uses, within Chrome's
/// 32-bit integers.
pub(crate) const HOST_IDS: u64 = 2_000_000_000;
/// Where the runner's own ids start (Ghiaccio 38278): the host's stay below.
const RUNNER_IDS: u64 = 2_140_000_000;
/// The most sites listed.
const MOST_SITES: usize = 200;
/// How often the page shown is read again, at most, while frames come.
const PAGE_EVERY_MS: i64 = 1000;

/// What a call of the host's was for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Call {
    Sites,
    Page,
}

/// The host's next id: above HOST_IDS, below the runner's, wrapping round.
fn next_id(inner: &mut Inner) -> u64 {
    inner.next_call = inner.next_call.max(HOST_IDS) + 1;
    if inner.next_call >= RUNNER_IDS {
        inner.next_call = HOST_IDS + 1;
    }
    inner.next_call
}

/// Sends one of the host's calls (browser-wide: no session needed).
fn call(inner: &mut Inner, what: Call) {
    let Some(runner) = inner.runner.clone() else { return };
    let method = match what {
        Call::Sites => "Storage.getCookies",
        Call::Page => "Target.getTargets",
    };
    let id = next_id(inner);
    let message = serde_json::to_vec(&json!({"id": id, "method": method, "params": {}})).unwrap_or_default();
    if runner.try_send((Channel::Cdp, message)).is_ok() {
        inner.calls.insert(id, what);
        // An answer that never comes isn't waited for long.
        if inner.calls.len() > 16
            && let Some(oldest) = inner.calls.keys().min().copied()
        {
            inner.calls.remove(&oldest);
        }
    }
}

/// Sends one of the host's calls whose answer is waited for, in CDP session
/// `session` (browser-wide when empty): the whole
/// answer comes on what's returned (None when there's no runner, or its
/// queue is full).
pub(super) fn ask(inner: &mut Inner, session: &str, method: &str, params: Value) -> Option<tokio::sync::oneshot::Receiver<Value>> {
    let runner = inner.runner.clone()?;
    let id = next_id(inner);
    let mut message = json!({"id": id, "method": method, "params": params});
    if !session.is_empty() {
        message["sessionId"] = Value::String(session.to_owned());
    }
    let message = serde_json::to_vec(&message).ok()?;
    if message.len() > MAX_CDP || runner.try_send((Channel::Cdp, message)).is_err() {
        return None;
    }
    let (answer, answered) = tokio::sync::oneshot::channel();
    inner.waiting.insert(id, answer);
    Some(answered)
}

/// Detaches the errand's sessions (Handle::on_release), with the host's
/// own ids: Chrome's answers go nowhere.
pub(super) fn release(inner: &mut Inner) {
    let sessions = std::mem::take(&mut inner.release);
    let Some(runner) = inner.runner.clone() else { return };
    // No download lands in the person's turn (R9, Fugo 38000): the errand
    // allows them again when it connects.
    let deny = json!({"id": next_id(inner), "method": "Browser.setDownloadBehavior", "params": {"behavior": "deny"}});
    let _ = runner.try_send((Channel::Cdp, serde_json::to_vec(&deny).unwrap_or_default()));
    for session in sessions {
        let message = json!({"id": next_id(inner), "method": "Target.detachFromTarget", "params": {"sessionId": session}});
        let _ = runner.try_send((Channel::Cdp, serde_json::to_vec(&message).unwrap_or_default()));
    }
}

/// Reads the signed-in sites again (driven only: an undriven Chrome has
/// no pipe).
pub(super) fn read_sites(inner: &mut Inner) {
    if inner.mode == Some(crate::browser_runner::wire::Mode::Driven) {
        call(inner, Call::Sites);
    }
}

/// Reads the page shown again, at most once a second, driven only.
pub(super) fn read_page(inner: &mut Inner) {
    let now = crate::transcript::now_ms();
    if inner.mode == Some(crate::browser_runner::wire::Mode::Driven) && now - inner.page_read >= PAGE_EVERY_MS {
        inner.page_read = now;
        call(inner, Call::Page);
    }
}

/// Whether a CDP message carries an id in the host's range (one an errand
/// mustn't use).
pub(crate) fn host_id(message: &[u8]) -> bool {
    serde_json::from_slice::<Value>(message).ok().and_then(|message| message["id"].as_u64()).is_some_and(|id| id > HOST_IDS)
}

/// A CDP message from the runner: the host's own answer (taken here, and
/// true), or not (false: it's the errand's).
pub(crate) fn answer(factory: &Arc<Factory>, one: &super::One, payload: &[u8]) -> bool {
    if payload.len() > MAX_CDP {
        return true;
    }
    // Cheap first: only a message naming one of the host's ids is parsed
    // here (an errand's go on as they are).
    let Ok(text) = std::str::from_utf8(payload) else { return false };
    if !text.contains("\"id\":2") {
        return false;
    }
    let Ok(message) = serde_json::from_str::<Value>(text) else { return false };
    let Some(id) = message["id"].as_u64().filter(|id| *id > HOST_IDS) else { return false };
    let changed = {
        let mut inner = one.inner();
        if let Some(answer) = inner.waiting.remove(&id) {
            drop(inner);
            let _ = answer.send(message);
            return true;
        }
        let Some(what) = inner.calls.remove(&id) else { return true };
        let result = &message["result"];
        match what {
            Call::Sites => {
                inner.providers = super::signout::signed_in(result).iter().map(|provider| (*provider).to_owned()).collect();
                let sites = signed_in(result);
                let changed = sites != inner.sites;
                inner.sites = sites;
                changed
            }
            Call::Page => {
                let (origin, title) = page(result);
                inner.origin = origin;
                inner.title = title;
                false
            }
        }
    };
    if changed {
        super::changed(factory);
    }
    true
}

/// The origins its cookies are for, sorted, without a name or value. A
/// name under another listed one is left out: signing out of it alone
/// would leave the cookies its parent holds for it, while signing out of
/// the parent takes its own too.
fn signed_in(result: &Value) -> Vec<String> {
    let mut domains: Vec<String> = result["cookies"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|cookie| cookie["domain"].as_str())
        .map(|domain| domain.trim_start_matches('.').to_ascii_lowercase())
        .filter(|domain| {
            !domain.is_empty() && domain.len() <= 253 && domain.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-')
        })
        .collect();
    domains.sort();
    domains.dedup();
    let under = |domain: &str| domains.iter().any(|parent| domain.strip_suffix(parent.as_str()).is_some_and(|rest| rest.ends_with('.')));
    let mut sites: Vec<String> = domains.iter().filter(|domain| !under(domain)).map(|domain| format!("https://{domain}")).collect();
    sites.sort();
    sites.truncate(MOST_SITES);
    sites
}

/// The page shown, its origin and title cleaned: the page the errand is
/// attached to before one it isn't, and a web page before a blank one (a
/// relaunch can leave an about:blank listed first: Secco 38469).
fn page(result: &Value) -> (String, String) {
    let web = |target: &Value| target["url"].as_str().is_some_and(|url| url.starts_with("http://") || url.starts_with("https://"));
    let shown = result["targetInfos"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|target| target["type"] == "page")
        .min_by_key(|target| (target["attached"] != Value::Bool(true), !web(target)));
    let Some(target) = shown else { return (String::new(), String::new()) };
    let origin = target["url"]
        .as_str()
        .and_then(|url| url::Url::parse(url).ok())
        .filter(|url| matches!(url.scheme(), "http" | "https"))
        .map(|url| url.origin().ascii_serialization())
        .unwrap_or_default();
    let title = view::cleaned(target["title"].as_str().unwrap_or_default(), 200);
    (view::cleaned(&origin, 200), title)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sites_are_origins_only() {
        let cookies = json!({"cookies": [
            {"name": "SID", "value": "secret", "domain": ".google.com"},
            {"name": "x", "value": "y", "domain": "accounts.google.com"},
            {"name": "z", "value": "y", "domain": ".google.com"},
            {"name": "bad", "value": "y", "domain": "evil.com/\\u202e"},
        ]});
        assert_eq!(signed_in(&cookies), ["https://google.com"], "accounts.google.com signs out with google.com");
        assert!(!format!("{:?}", signed_in(&cookies)).contains("secret"));
        let apart = json!({"cookies": [
            {"name": "a", "domain": "api.x.com"},
            {"name": "b", "domain": "www.x.com"},
            {"name": "c", "domain": "notx.com"},
            {"name": "d", "domain": ".com.x.com"},
        ]});
        assert_eq!(signed_in(&apart), ["https://api.x.com", "https://com.x.com", "https://notx.com", "https://www.x.com"]);
    }

    #[test]
    fn leaving_driven_lets_go_of_the_errands_sessions_and_its_connection() {
        use super::super::{One, State, set_state};
        let one = One::new("yukako");
        let mut inner = one.inner();
        let (runner, mut to_runner) = tokio::sync::mpsc::channel(8);
        let (cdp, _from_chrome) = tokio::sync::mpsc::channel(8);
        inner.runner = Some(runner);
        inner.cdp = Some(cdp);
        inner.state = State::Driven;
        inner.release = vec!["S1".to_owned()];
        set_state(&one, &mut inner, State::TakenOver);
        assert!(inner.cdp.is_none());
        assert!(inner.release.is_empty());
        let (_, deny) = to_runner.try_recv().expect("downloads denied");
        let deny: Value = serde_json::from_slice(&deny).unwrap();
        assert_eq!((deny["method"].as_str(), deny["params"]["behavior"].as_str()), (Some("Browser.setDownloadBehavior"), Some("deny")));
        let (channel, message) = to_runner.try_recv().expect("a detach");
        assert_eq!(channel, Channel::Cdp);
        let message: Value = serde_json::from_slice(&message).unwrap();
        assert_eq!(message["method"], "Target.detachFromTarget");
        assert_eq!(message["params"]["sessionId"], "S1");
        assert!(message["id"].as_u64().unwrap() > HOST_IDS && message["id"].as_u64().unwrap() < 1 << 31);
        // Leaving again: downloads denied again, the sessions not detached twice.
        set_state(&one, &mut inner, State::Driven);
        set_state(&one, &mut inner, State::PersonsTurn);
        let (_, deny) = to_runner.try_recv().expect("downloads denied");
        assert!(String::from_utf8_lossy(&deny).contains("\"deny\""));
        assert!(to_runner.try_recv().is_err());
    }

    #[test]
    fn the_hosts_ids_stay_below_the_runners() {
        let one = super::super::One::new("");
        let mut inner = one.inner();
        assert_eq!(next_id(&mut inner), HOST_IDS + 1);
        inner.next_call = RUNNER_IDS - 2;
        assert_eq!(next_id(&mut inner), RUNNER_IDS - 1);
        assert_eq!(next_id(&mut inner), HOST_IDS + 1);
    }

    #[test]
    fn an_errand_cant_use_the_hosts_ids() {
        assert!(host_id(br#"{"id":2000000001,"method":"Storage.getCookies"}"#));
        assert!(!host_id(br#"{"id":7,"method":"Page.navigate"}"#));
        assert!(!host_id(b"not json"));
    }

    #[test]
    fn the_page_is_its_origin_and_a_cleaned_title() {
        let targets = json!({"targetInfos": [
            {"type": "browser", "url": "", "title": "", "attached": true},
            {"type": "page", "url": "https://play.google.com/console/signup?token=abc", "title": "Play\u{202e} Console\u{200b}", "attached": true},
        ]});
        assert_eq!(page(&targets), ("https://play.google.com".to_owned(), "Play Console".to_owned()));
        let file = json!({"targetInfos": [{"type": "page", "url": "file:///etc/passwd", "title": "x", "attached": true}]});
        assert_eq!(page(&file).0, "");
        // A blank page left first by a relaunch isn't the one shown.
        let stray = json!({"targetInfos": [
            {"type": "page", "url": "about:blank", "title": "", "attached": false},
            {"type": "page", "url": "https://example.com/a", "title": "Example", "attached": false},
        ]});
        assert_eq!(page(&stray), ("https://example.com".to_owned(), "Example".to_owned()));
        let driven = json!({"targetInfos": [
            {"type": "page", "url": "https://other.example/", "title": "Other", "attached": false},
            {"type": "page", "url": "https://example.com/a", "title": "Example", "attached": true},
        ]});
        assert_eq!(page(&driven).1, "Example");
    }
}
