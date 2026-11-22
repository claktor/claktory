//! What may pass through the helper, checked before it's relayed: the
//! schema agreed with the extension (browser-extension/PROTOCOL.md) and the
//! app (BrowserBridge.swift). Every frame has exactly its fields. The
//! extension's hello pins one tab and one origin for the helper's whole
//! life; one command is in flight at a time, with ids and sequences that
//! only go up, for one backend session; a click, fill or navigation is
//! reviewed by the person before the extension acts, and the app's decision
//! must answer that review, by its hash, once. This
//! is defence in depth: the extension and the app check the same again,
//! and only the person's choice in the app gives a session any authority.

use serde_json::{Map, Value, json};

/// The protocol version on both sides.
pub const VERSION: u64 = 1;
/// A result's object, serialized, at most.
pub const RESULT_LIMIT: usize = 64 * 1024;
/// A command's op, serialized, at most.
pub const COMMAND_LIMIT: usize = 16 * 1024;
/// A review's description of the action, serialized, at most.
pub const ACTION_LIMIT: usize = 64 * 1024;
const FILL_LIMIT: usize = 8192;
const URL_LIMIT: usize = 2048;
const REF_LIMIT: usize = 128;
const TARGET_LIMIT: usize = 64;
const SESSION_LIMIT: usize = 128;
const TITLE_LIMIT: usize = 300;
const ORIGIN_LIMIT: usize = 2048;
const AGENT_LIMIT: usize = 100;
const PURPOSE_LIMIT: usize = 500;
const READ_LIMIT: u64 = 65536;

/// Why the extension paused a session, or ended it.
const PAUSED: &[&str] = &["tab_inactive", "window_blurred", "popup_opened", "dialog_open"];
const ENDED: &[&str] = &["tab_closed", "origin_changed", "stopped", "extension_disabled", "expired", "limit_exceeded", "helper_refused"];

/// Why the helper ends a session itself, as both sides are told.
#[derive(Debug, PartialEq, Eq)]
pub enum Refusal {
    /// The extension named a tab other than its hello's.
    WrongTab,
    /// A frame not in the schema, or out of turn: what it was.
    BadFrame(String),
    /// The app's command broke the rules.
    BadCommand(String),
}

impl Refusal {
    pub fn reason(&self) -> &'static str {
        match self {
            Refusal::WrongTab => "wrong_tab",
            Refusal::BadFrame(_) => "bad_frame",
            Refusal::BadCommand(_) => "bad_command",
        }
    }
}

impl std::fmt::Display for Refusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Refusal::WrongTab => write!(f, "the extension named another tab"),
            Refusal::BadFrame(why) => write!(f, "{why}"),
            Refusal::BadCommand(why) => write!(f, "{why}"),
        }
    }
}

/// What to do with a frame from the extension.
#[derive(Debug, PartialEq, Eq)]
pub enum FromChrome {
    /// Relay it to the app as it is.
    Relay,
    /// Relay it, then end: the extension ended the session.
    Ended,
}

/// What to do with a frame from the app.
#[derive(Debug, PartialEq, Eq)]
pub enum FromApp {
    /// Send this to the extension.
    Send(Value),
    /// Send the extension a stop, then end.
    Stop,
}

/// The tab and origin the extension's hello named.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Pinned {
    pub tab: u64,
    pub origin: String,
}

/// One helper's session, as frames pass.
#[derive(Debug, Default)]
pub struct Session {
    pinned: Option<Pinned>,
    /// The backend's session this helper serves: the first command's.
    session: Option<String>,
    outstanding: Option<InFlight>,
    last_id: Option<u64>,
    last_sequence: Option<u64>,
}

/// The command the extension is working on, and the review it asked for.
#[derive(Debug)]
struct InFlight {
    id: u64,
    sequence: u64,
    op: Map<String, Value>,
    /// The review's hash, and the person's decision on it once made.
    review: Option<(String, Option<bool>)>,
}

fn bad(why: impl Into<String>) -> Refusal {
    Refusal::BadFrame(why.into())
}

/// The frame has `required` and maybe `optional`, nothing else.
fn fields(frame: &Map<String, Value>, kind: &str, required: &[&str], optional: &[&str]) -> Result<(), Refusal> {
    if let Some(missing) = required.iter().find(|name| !frame.contains_key(**name)) {
        return Err(bad(format!("{kind} without {missing}")));
    }
    if frame.keys().any(|name| !required.contains(&name.as_str()) && !optional.contains(&name.as_str())) {
        return Err(bad(format!("{kind} with an unknown field")));
    }
    Ok(())
}

fn text<'a>(frame: &'a Map<String, Value>, name: &str) -> Result<&'a str, Refusal> {
    frame.get(name).and_then(Value::as_str).ok_or_else(|| bad(format!("{name} isn't text")))
}

fn number(frame: &Map<String, Value>, name: &str) -> Result<u64, Refusal> {
    frame.get(name).and_then(Value::as_u64).ok_or_else(|| bad(format!("{name} isn't a whole number")))
}

/// An https origin as the extension reads it from Chrome: lowercase host
/// (punycode), an optional port, nothing else; never an IP literal or a
/// local name.
pub fn valid_origin(origin: &str) -> bool {
    let Some(rest) = origin.strip_prefix("https://") else { return false };
    if origin.len() > ORIGIN_LIMIT {
        return false;
    }
    let (host, port) = match rest.rsplit_once(':') {
        Some((host, port)) => (host, Some(port)),
        None => (rest, None),
    };
    if let Some(port) = port
        && !(!port.is_empty()
            && port.len() <= 5
            && port.bytes().all(|b| b.is_ascii_digit())
            && port.parse::<u32>().is_ok_and(|p| (1..=65535).contains(&p)))
    {
        return false;
    }
    let shaped = !host.is_empty()
        && host.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'.')
        && !host.starts_with('.')
        && !host.ends_with('.')
        && !host.contains("..");
    let ip = host.bytes().all(|b| b.is_ascii_digit() || b == b'.');
    let local = host == "localhost" || host.ends_with(".localhost") || host.ends_with(".local");
    shaped && !ip && !local
}

/// A character the extension never sends raw (it escapes them as
/// ⟦U+XXXX⟧): controls, line breaks, bidi controls and zero-width ones.
fn plain(c: char) -> bool {
    !c.is_control() && !matches!(c, '\u{200B}'..='\u{200F}' | '\u{2028}'..='\u{202E}' | '\u{2060}'..='\u{2069}' | '\u{061C}' | '\u{FEFF}')
}

/// Every string in `value`, keys too, is escaped as the extension escapes
/// it.
fn escaped(value: &Value) -> bool {
    match value {
        Value::String(text) => text.chars().all(plain),
        Value::Array(items) => items.iter().all(escaped),
        Value::Object(object) => object.iter().all(|(key, value)| key.chars().all(plain) && escaped(value)),
        _ => true,
    }
}

/// A navigation's URL: the session's origin exactly, then a path or
/// nothing; no query, fragment or credentials.
fn valid_url(url: &str, origin: &str) -> bool {
    let path = match url.strip_prefix(origin) {
        Some("") => "/",
        Some(path) if path.starts_with('/') => path,
        _ => return false,
    };
    url.len() <= URL_LIMIT && path.chars().all(|c| !c.is_whitespace() && !c.is_control() && !matches!(c, '?' | '#' | '\\'))
}

/// An opaque name the extension gave (a document or an element).
fn valid_ref(value: Option<&Value>, limit: usize) -> bool {
    value.and_then(Value::as_str).is_some_and(|name| !name.is_empty() && name.len() <= limit && name.bytes().all(|b| b.is_ascii_graphic()))
}

/// A command's op: read or scroll, or (B3b) a click, fill or navigation,
/// which the extension shows the person for review before it acts.
fn valid_command(cmd: &Value, origin: &str) -> Result<(), Refusal> {
    let refuse = |why: &str| Err(Refusal::BadCommand(why.to_owned()));
    let Value::Object(cmd) = cmd else { return refuse("a command that isn't an object") };
    if serde_json::to_vec(cmd).map_or(usize::MAX, |bytes| bytes.len()) > COMMAND_LIMIT {
        return refuse("a command past 16 KiB");
    }
    let only = |allowed: &[&str]| cmd.keys().all(|key| allowed.contains(&key.as_str()));
    match cmd.get("op").and_then(Value::as_str) {
        Some("read") => {
            if !only(&["op", "limit"]) {
                return refuse("a read with an unknown field");
            }
            match cmd.get("limit") {
                None => Ok(()),
                Some(limit) if limit.as_u64().is_some_and(|limit| (1..=READ_LIMIT).contains(&limit)) => Ok(()),
                Some(_) => refuse("a read's limit isn't 1 to 65536"),
            }
        }
        Some("scroll") => {
            if !only(&["op", "direction", "pages"]) {
                return refuse("a scroll with an unknown field");
            }
            let direction = matches!(cmd.get("direction").and_then(Value::as_str), Some("up" | "down"));
            let pages = cmd.get("pages").and_then(Value::as_u64).is_some_and(|pages| (1..=3).contains(&pages));
            if direction && pages { Ok(()) } else { refuse("a scroll isn't up or down by 1 to 3 pages") }
        }
        Some(op @ ("click" | "fill")) => {
            let fill = op == "fill";
            let allowed: &[&str] =
                if fill { &["op", "document", "revision", "target", "text"] } else { &["op", "document", "revision", "target"] };
            if !only(allowed) || cmd.len() != allowed.len() {
                return refuse("a click or fill without exactly its fields");
            }
            let refs = valid_ref(cmd.get("document"), REF_LIMIT) && valid_ref(cmd.get("target"), TARGET_LIMIT);
            let revision = cmd.get("revision").and_then(Value::as_u64).is_some();
            let text = !fill
                || cmd.get("text").and_then(Value::as_str).is_some_and(|text| {
                    text.chars().count() <= FILL_LIMIT && text.chars().all(|c| !c.is_control() || c == '\n' || c == '\t')
                });
            if refs && revision && text { Ok(()) } else { refuse("a click or fill with a bad document, target or text") }
        }
        Some("navigate") => {
            if !only(&["op", "url"]) {
                return refuse("a navigation with an unknown field");
            }
            match cmd.get("url").and_then(Value::as_str) {
                Some(url) if valid_url(url, origin) => Ok(()),
                _ => refuse("a navigation that leaves the session's origin, or has a query or fragment"),
            }
        }
        _ => refuse("an op other than read, scroll, click, fill or navigate"),
    }
}

impl Session {
    pub fn pinned(&self) -> Option<&Pinned> {
        self.pinned.as_ref()
    }

    /// Checks a frame from the extension.
    pub fn from_chrome(&mut self, frame: &Map<String, Value>) -> Result<FromChrome, Refusal> {
        let kind = frame.get("type").and_then(Value::as_str).ok_or_else(|| bad("a frame without a type"))?;
        match kind {
            "hello" => fields(frame, kind, &["type", "v", "tab", "origin", "title"], &[])?,
            "result" => fields(frame, kind, &["type", "tab", "id", "result"], &[])?,
            "review" => fields(frame, kind, &["type", "tab", "id", "hash", "expires_ms", "action"], &[])?,
            "paused" | "ended" => fields(frame, kind, &["type", "tab", "reason"], &[])?,
            "resumed" => fields(frame, kind, &["type", "tab"], &[])?,
            _ => return Err(bad("an unknown type from the extension")),
        }
        let tab = number(frame, "tab")?;
        let Some(pinned) = &self.pinned else {
            if kind != "hello" {
                return Err(bad(format!("{kind} before hello")));
            }
            if number(frame, "v")? != VERSION {
                return Err(bad("another protocol version"));
            }
            let origin = text(frame, "origin")?;
            if !valid_origin(origin) {
                return Err(bad("an origin that isn't https on a public name"));
            }
            let title = text(frame, "title")?;
            if title.chars().count() > TITLE_LIMIT || !title.chars().all(plain) {
                return Err(bad("a title that isn't one escaped line of at most 300 characters"));
            }
            self.pinned = Some(Pinned { tab, origin: origin.to_owned() });
            return Ok(FromChrome::Relay);
        };
        if tab != pinned.tab {
            return Err(Refusal::WrongTab);
        }
        match kind {
            "hello" => Err(bad("a second hello")),
            "result" => {
                let id = number(frame, "id")?;
                let Some(command) = self.outstanding.as_ref().filter(|command| command.id == id) else {
                    return Err(bad("a result for no command in flight"));
                };
                // An action that took place must have been allowed (Fugo):
                // it can't stop a compromised extension, but it shows it.
                let acted = matches!(command.op.get("op").and_then(Value::as_str), Some("click" | "fill" | "navigate"))
                    && !frame["result"].get("error").and_then(Value::as_str).is_some_and(|error| !error.is_empty());
                if acted && !matches!(command.review, Some((_, Some(true)))) {
                    return Err(bad("an action done without the person allowing it"));
                }
                let Some(result @ Value::Object(_)) = frame.get("result") else { return Err(bad("a result that isn't an object")) };
                if serde_json::to_vec(result).map_or(usize::MAX, |bytes| bytes.len()) > RESULT_LIMIT {
                    return Err(bad("a result past 64 KiB"));
                }
                self.outstanding = None;
                Ok(FromChrome::Relay)
            }
            "review" => {
                let id = number(frame, "id")?;
                number(frame, "expires_ms")?;
                let session = self.session.clone().unwrap_or_default();
                let Some(command) = self.outstanding.as_mut().filter(|command| command.id == id) else {
                    return Err(bad("a review for no command in flight"));
                };
                if !matches!(command.op.get("op").and_then(Value::as_str), Some("click" | "fill" | "navigate")) {
                    return Err(bad("a review of a command that needs none"));
                }
                if command.review.is_some() {
                    return Err(bad("a second review of one command"));
                }
                let action = &frame["action"];
                if !action.is_object() || serde_json::to_vec(action).map_or(usize::MAX, |bytes| bytes.len()) > ACTION_LIMIT {
                    return Err(bad("a review whose action isn't an object of at most 64 KiB"));
                }
                if !escaped(action) {
                    return Err(bad("a review with a character it should have escaped"));
                }
                let hash = text(frame, "hash")?;
                let expected = crate::canonical::review_hash(&session, id, command.sequence, &command.op, action);
                if expected.as_deref() != Some(hash) {
                    return Err(bad("a review whose hash isn't the command's"));
                }
                command.review = Some((hash.to_owned(), None));
                Ok(FromChrome::Relay)
            }
            "paused" if PAUSED.contains(&text(frame, "reason")?) => Ok(FromChrome::Relay),
            "ended" if ENDED.contains(&text(frame, "reason")?) => Ok(FromChrome::Ended),
            "resumed" => Ok(FromChrome::Relay),
            _ => Err(bad(format!("{kind} with an unknown reason"))),
        }
    }

    /// Checks a frame from the app, and what the extension gets for it.
    pub fn from_app(&mut self, frame: &Map<String, Value>) -> Result<FromApp, Refusal> {
        let Some(kind) = frame.get("type") else {
            fields(frame, "a command", &["id", "session", "sequence", "cmd"], &[])?;
            let id = number(frame, "id")?;
            let sequence = number(frame, "sequence")?;
            let session = text(frame, "session")?;
            let Some(pinned) = &self.pinned else {
                return Err(Refusal::BadCommand("a command before the extension's hello".into()));
            };
            if !valid_ref(frame.get("session"), SESSION_LIMIT) || self.session.as_deref().is_some_and(|first| first != session) {
                return Err(Refusal::BadCommand("a command for another session".into()));
            }
            if self.last_sequence.is_some_and(|last| sequence <= last) {
                return Err(Refusal::BadCommand("a command sequence that doesn't go up".into()));
            }
            if self.outstanding.is_some() {
                return Err(Refusal::BadCommand("a command while another is in flight".into()));
            }
            if self.last_id.is_some_and(|last| id <= last) {
                return Err(Refusal::BadCommand("a command id that doesn't go up".into()));
            }
            valid_command(&frame["cmd"], &pinned.origin)?;
            let Value::Object(op) = frame["cmd"].clone() else { unreachable!("checked") };
            self.session = Some(session.to_owned());
            self.outstanding = Some(InFlight { id, sequence, op, review: None });
            self.last_id = Some(id);
            self.last_sequence = Some(sequence);
            return Ok(FromApp::Send(json!({"type": "command", "id": id, "session": session, "sequence": sequence, "cmd": frame["cmd"]})));
        };
        let kind = kind.as_str().ok_or_else(|| bad("a type that isn't text"))?;
        match kind {
            "pair" => {
                fields(frame, kind, &["type", "code"], &[])?;
                let code = text(frame, "code")?;
                if code.len() != 6 || !code.bytes().all(|b| b.is_ascii_digit()) {
                    return Err(bad("a pairing code that isn't 6 digits"));
                }
                Ok(FromApp::Send(json!({"type": "pairing", "code": code})))
            }
            "bound" => {
                fields(frame, kind, &["type", "agent", "until_ms"], &["purpose"])?;
                if self.pinned.is_none() {
                    return Err(bad("bound before the extension's hello"));
                }
                let agent = text(frame, "agent")?.chars().count();
                number(frame, "until_ms")?;
                let purpose = if frame.contains_key("purpose") { text(frame, "purpose")?.chars().count() } else { 0 };
                if agent == 0 || agent > AGENT_LIMIT || purpose > PURPOSE_LIMIT {
                    return Err(bad("bound with an agent or purpose too long"));
                }
                Ok(FromApp::Send(Value::Object(frame.clone())))
            }
            "stop" => {
                fields(frame, kind, &["type"], &[])?;
                Ok(FromApp::Stop)
            }
            "decision" => {
                fields(frame, kind, &["type", "id", "hash", "allow"], &[])?;
                let id = number(frame, "id")?;
                let hash = text(frame, "hash")?;
                frame["allow"].as_bool().ok_or_else(|| bad("allow isn't true or false"))?;
                let waiting = self.outstanding.as_mut().filter(|command| command.id == id).and_then(|command| command.review.as_mut());
                match waiting {
                    Some((reviewed, decided)) if reviewed == hash && decided.is_none() => {
                        *decided = frame["allow"].as_bool();
                        Ok(FromApp::Send(Value::Object(frame.clone())))
                    }
                    _ => Err(Refusal::BadCommand("a decision on no review waiting for one".into())),
                }
            }
            _ => Err(bad("an unknown type from the app")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn object(value: Value) -> Map<String, Value> {
        match value {
            Value::Object(object) => object,
            _ => unreachable!(),
        }
    }

    fn hello() -> Map<String, Value> {
        object(json!({"type": "hello", "v": 1, "tab": 7, "origin": "https://example.com", "title": "Example"}))
    }

    #[test]
    fn origins_are_https_on_a_public_name() {
        for good in ["https://example.com", "https://a-b.example.co.uk:8443", "https://xn--bcher-kva.example"] {
            assert!(valid_origin(good), "{good}");
        }
        for bad in [
            "http://example.com",
            "https://example.com/",
            "https://example.com/path",
            "https://example.com?q",
            "https://user@example.com",
            "https://Example.com",
            "https://127.0.0.1",
            "https://[::1]",
            "https://localhost",
            "https://printer.local",
            "https://a.localhost:3000",
            "https://example.com:0",
            "https://example.com:99999",
            "https://example.com:",
            "https://",
            "https://.example.com",
            "chrome-extension://abc",
        ] {
            assert!(!valid_origin(bad), "{bad}");
        }
    }

    #[test]
    fn the_hello_pins_one_tab_and_origin_for_good() {
        let mut session = Session::default();
        let early = object(json!({"type": "paused", "tab": 7, "reason": "tab_inactive"}));
        assert!(matches!(session.from_chrome(&early), Err(Refusal::BadFrame(_))), "nothing before hello");
        let mut extra = hello();
        extra.insert("debug".into(), json!(true));
        assert!(matches!(session.from_chrome(&extra), Err(Refusal::BadFrame(_))), "no unknown fields");
        for title in ["two\nlines", "evil\u{202E}txt.exe", "zero\u{200B}width", "bell\u{7}"] {
            let mut raw = hello();
            raw.insert("title".into(), json!(title));
            assert!(matches!(Session::default().from_chrome(&raw), Err(Refusal::BadFrame(_))), "{title:?}");
        }
        let mut escaped = hello();
        escaped.insert("title".into(), json!("evil⟦U+202E⟧txt.exe…"));
        assert_eq!(Session::default().from_chrome(&escaped), Ok(FromChrome::Relay));
        let mut old = hello();
        old.insert("v".into(), json!(2));
        assert!(matches!(session.from_chrome(&old), Err(Refusal::BadFrame(_))));
        assert_eq!(session.from_chrome(&hello()), Ok(FromChrome::Relay));
        assert_eq!(session.pinned(), Some(&Pinned { tab: 7, origin: "https://example.com".into() }));
        assert!(matches!(session.from_chrome(&hello()), Err(Refusal::BadFrame(_))), "only one hello");

        let other_tab = object(json!({"type": "paused", "tab": 8, "reason": "tab_inactive"}));
        assert_eq!(session.from_chrome(&other_tab), Err(Refusal::WrongTab));
        let mut other_hello = hello();
        other_hello.insert("tab".into(), json!(8));
        assert_eq!(session.from_chrome(&other_hello), Err(Refusal::WrongTab));
        let paused = object(json!({"type": "paused", "tab": 7, "reason": "dialog_open"}));
        assert_eq!(session.from_chrome(&paused), Ok(FromChrome::Relay));
        let odd = object(json!({"type": "paused", "tab": 7, "reason": "because"}));
        assert!(matches!(session.from_chrome(&odd), Err(Refusal::BadFrame(_))));
        let ended = object(json!({"type": "ended", "tab": 7, "reason": "origin_changed"}));
        assert_eq!(session.from_chrome(&ended), Ok(FromChrome::Ended));
    }

    fn command(id: u64, sequence: u64, cmd: Value) -> Map<String, Value> {
        object(json!({"id": id, "session": "s-1", "sequence": sequence, "cmd": cmd}))
    }

    #[test]
    fn one_command_at_a_time_with_ids_going_up_and_only_the_known_ops() {
        let mut session = Session::default();
        let read = json!({"op": "read", "limit": 4096});
        assert!(matches!(session.from_app(&command(1, 1, read.clone())), Err(Refusal::BadCommand(_))), "not before hello");
        session.from_chrome(&hello()).unwrap();
        for cmd in [
            json!({"op": "click", "target": "r1"}),
            json!({"op": "click", "document": "D1", "revision": 1, "target": "r1", "text": "x"}),
            json!({"op": "fill", "document": "D1", "revision": 1, "target": "r1", "text": "a\u{1b}b"}),
            json!({"op": "fill", "document": "D1", "revision": 1, "target": "r1", "text": "a".repeat(8193)}),
            json!({"op": "fill", "document": "D 1", "revision": 1, "target": "r1", "text": "a"}),
            json!({"op": "navigate", "url": "https://example.com.evil.net/"}),
            json!({"op": "navigate", "url": "https://example.com/search?q=1"}),
            json!({"op": "navigate", "url": "https://example.com/a#b"}),
            json!({"op": "navigate", "url": "https://other.com/"}),
            json!({"op": "decision", "id": 1, "hash": "0", "allow": true}),
            json!({"op": "read", "limit": 0}),
            json!({"op": "read", "limit": 65537}),
            json!({"op": "read", "selector": "body"}),
            json!({"op": "scroll", "direction": "left", "pages": 1}),
            json!({"op": "scroll", "direction": "down", "pages": 4}),
            json!({"op": "eval", "code": "1"}),
            json!("read"),
        ] {
            assert!(matches!(session.from_app(&command(1, 1, cmd.clone())), Err(Refusal::BadCommand(_))), "{cmd}");
        }
        let sent = session.from_app(&command(5, 10, read.clone())).unwrap();
        assert_eq!(sent, FromApp::Send(json!({"type": "command", "id": 5, "session": "s-1", "sequence": 10, "cmd": read})));
        let scroll = json!({"op": "scroll", "direction": "down", "pages": 2});
        assert!(matches!(session.from_app(&command(6, 11, scroll.clone())), Err(Refusal::BadCommand(_))), "one in flight");

        let result = |id: u64, result: Value| object(json!({"type": "result", "tab": 7, "id": id, "result": result}));
        assert!(matches!(session.from_chrome(&result(4, json!({}))), Err(Refusal::BadFrame(_))), "answers no command");
        let big = json!({"text": "a".repeat(RESULT_LIMIT)});
        assert!(matches!(session.from_chrome(&result(5, big)), Err(Refusal::BadFrame(_))));
        assert_eq!(session.from_chrome(&result(5, json!({"scroll_y": 0}))), Ok(FromChrome::Relay));
        assert!(matches!(session.from_chrome(&result(5, json!({}))), Err(Refusal::BadFrame(_))), "answered once");
        assert!(matches!(session.from_app(&command(5, 11, scroll.clone())), Err(Refusal::BadCommand(_))), "ids go up");
        assert!(matches!(session.from_app(&command(6, 10, scroll.clone())), Err(Refusal::BadCommand(_))), "sequences go up");
        let other = object(json!({"id": 6, "session": "s-2", "sequence": 11, "cmd": scroll.clone()}));
        assert!(matches!(session.from_app(&other), Err(Refusal::BadCommand(_))), "one session");
        assert!(session.from_app(&command(6, 11, scroll)).is_ok());
        for url in ["https://example.com", "https://example.com/", "https://example.com/inbox/42"] {
            let mut session = Session::default();
            session.from_chrome(&hello()).unwrap();
            assert!(session.from_app(&command(1, 1, json!({"op": "navigate", "url": url}))).is_ok(), "{url}");
        }
    }

    #[test]
    fn a_decision_answers_the_review_of_the_command_in_flight_once() {
        let mut session = Session::default();
        session.from_chrome(&hello()).unwrap();
        let op = json!({"op": "fill", "document": "D1", "revision": 3, "target": "r-9f2c", "text": "Hello\nthere"});
        session.from_app(&command(7, 12, op.clone())).unwrap();
        let action = json!({"op": "fill", "origin": "https://example.com", "path": "/compose", "name": "Message", "text": "Hello⏎there"});
        let Value::Object(fields) = op else { unreachable!() };
        let hash = crate::canonical::review_hash("s-1", 7, 12, &fields, &action).unwrap();
        let review = |id: u64, hash: &str, action: Value| {
            object(json!({"type": "review", "tab": 7, "id": id, "hash": hash, "expires_ms": 1, "action": action}))
        };
        let decision = |id: u64, hash: &str| object(json!({"type": "decision", "id": id, "hash": hash, "allow": true}));
        assert!(matches!(session.from_app(&decision(7, &hash)), Err(Refusal::BadCommand(_))), "no review yet");
        assert!(matches!(session.from_chrome(&review(6, &hash, action.clone())), Err(Refusal::BadFrame(_))), "another command");
        assert!(matches!(session.from_chrome(&review(7, &"0".repeat(64), action.clone())), Err(Refusal::BadFrame(_))), "not its hash");
        let mut shown = action.clone();
        shown["text"] = json!("Hello\nthere");
        assert!(matches!(session.from_chrome(&review(7, &hash, shown)), Err(Refusal::BadFrame(_))), "raw newline");
        let mut tampered = action.clone();
        tampered["text"] = json!("Goodbye");
        assert!(matches!(session.from_chrome(&review(7, &hash, tampered)), Err(Refusal::BadFrame(_))), "another description");
        assert_eq!(session.from_chrome(&review(7, &hash, action.clone())), Ok(FromChrome::Relay));
        assert!(matches!(session.from_chrome(&review(7, &hash, action)), Err(Refusal::BadFrame(_))), "one review");
        assert!(matches!(session.from_app(&decision(7, &"0".repeat(64))), Err(Refusal::BadCommand(_))), "another hash");
        assert_eq!(session.from_app(&decision(7, &hash)), Ok(FromApp::Send(Value::Object(decision(7, &hash)))));
        assert!(matches!(session.from_app(&decision(7, &hash)), Err(Refusal::BadCommand(_))), "decided once");
        let result = object(json!({"type": "result", "tab": 7, "id": 7, "result": {"done": true, "took": true}}));
        assert_eq!(session.from_chrome(&result), Ok(FromChrome::Relay));

        // Done without an allow: refused, though a refusal or a denial passes.
        let click = json!({"op": "click", "document": "D1", "revision": 4, "target": "r-1"});
        let done = |id: u64| object(json!({"type": "result", "tab": 7, "id": id, "result": {"done": true}}));
        let failed = |id: u64, error: &str| object(json!({"type": "result", "tab": 7, "id": id, "result": {"error": error}}));
        session.from_app(&command(9, 14, click.clone())).unwrap();
        assert_eq!(session.from_chrome(&failed(9, "sensitive_action")), Ok(FromChrome::Relay), "refused before any review");
        session.from_app(&command(10, 15, click.clone())).unwrap();
        assert!(matches!(session.from_chrome(&done(10)), Err(Refusal::BadFrame(_))), "never reviewed");
        for error in [json!(null), json!(""), json!(false), json!({"why": "x"})] {
            let mut session = Session::default();
            session.from_chrome(&hello()).unwrap();
            session.from_app(&command(10, 15, click.clone())).unwrap();
            let sham = object(json!({"type": "result", "tab": 7, "id": 10, "result": {"done": true, "error": error}}));
            assert!(matches!(session.from_chrome(&sham), Err(Refusal::BadFrame(_))), "an error of {error} isn't one");
        }
        let mut session = Session::default();
        session.from_chrome(&hello()).unwrap();
        session.from_app(&command(11, 16, click.clone())).unwrap();
        let Value::Object(fields) = click else { unreachable!() };
        let action = json!({"op": "click", "name": "Send"});
        let hash = crate::canonical::review_hash("s-1", 11, 16, &fields, &action).unwrap();
        session.from_chrome(&review(11, &hash, action)).unwrap();
        let deny = object(json!({"type": "decision", "id": 11, "hash": hash, "allow": false}));
        session.from_app(&deny).unwrap();
        assert!(matches!(session.from_chrome(&done(11)), Err(Refusal::BadFrame(_))), "denied, yet done");
        assert_eq!(session.from_chrome(&failed(11, "denied")), Ok(FromChrome::Relay));

        // A read needs no review.
        session.from_app(&command(12, 17, json!({"op": "read"}))).unwrap();
        let empty = json!({});
        let Value::Object(read) = json!({"op": "read"}) else { unreachable!() };
        let hash = crate::canonical::review_hash("s-1", 12, 17, &read, &empty).unwrap();
        assert!(matches!(session.from_chrome(&review(12, &hash, empty)), Err(Refusal::BadFrame(_))));
    }

    #[test]
    fn the_app_pairs_binds_and_stops() {
        let mut session = Session::default();
        let pair = object(json!({"type": "pair", "code": "012345"}));
        assert_eq!(session.from_app(&pair), Ok(FromApp::Send(json!({"type": "pairing", "code": "012345"}))));
        for code in ["12345", "1234567", "12345a", "١٢٣٤٥٦"] {
            let pair = object(json!({"type": "pair", "code": code}));
            assert!(session.from_app(&pair).is_err(), "{code}");
        }
        let bound = object(json!({"type": "bound", "agent": "Rivet", "until_ms": 1, "purpose": "Read the launch page"}));
        assert!(session.from_app(&bound).is_err(), "not before hello");
        session.from_chrome(&hello()).unwrap();
        assert_eq!(session.from_app(&bound), Ok(FromApp::Send(Value::Object(bound.clone()))));
        let long = object(json!({"type": "bound", "agent": "R".repeat(101), "until_ms": 1}));
        assert!(session.from_app(&long).is_err());
        let granted = object(json!({"type": "bound", "agent": "Rivet", "until_ms": 1, "grant": "all"}));
        assert!(session.from_app(&granted).is_err());
        assert_eq!(session.from_app(&object(json!({"type": "stop"}))), Ok(FromApp::Stop));
        assert!(session.from_app(&object(json!({"type": "approve"}))).is_err());
    }
}
