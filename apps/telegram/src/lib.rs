//! Claktory's Telegram app: a bot that ties itself to one person's chat,
//! turns their messages into `message` events for the agents subscribed to
//! them, and lets agents granted it message them back.
//!
//! The logic here speaks to Claktory's app host only through [`Host`], so
//! it's tested natively with a stand-in ([`tests`]); `wasm.rs` is the real
//! host, through the `claktory:app@1` world and `wasi:http`.

mod markdown;
#[cfg(target_arch = "wasm32")]
mod wasm;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

pub use markdown::to_telegram_html;

/// Telegram's Bot API.
pub const API: &str = "https://api.telegram.org";

/// How long one getUpdates waits for a message, in seconds.
pub const POLL_SECONDS: u64 = 25;

/// How often the app looks at its poll while one is waiting, in ms.
pub const TICK_MS: u64 = 1000;

/// The most messages kept for `telegram_history`.
pub const HISTORY_KEPT: usize = 200;

/// The longest message Telegram takes, in characters.
pub const MESSAGE_MAX: usize = 4096;

/// An HTTP request to Telegram.
#[derive(Debug, Clone, PartialEq)]
pub struct Request {
    /// The Bot API method, such as "sendMessage".
    pub method: String,
    /// Its parameters, sent as JSON.
    pub params: Value,
}

/// Telegram's answer: the HTTP status and body.
#[derive(Debug, Clone, PartialEq)]
pub struct Response {
    pub status: u16,
    pub body: Vec<u8>,
}

/// How much a log line matters.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    Error,
    Warn,
    Info,
    Debug,
}

/// What the app can ask of Claktory's app host.
pub trait Host {
    /// The app's own secret (only those granted to it).
    fn secret(&mut self, name: &str) -> Option<String>;
    fn kv_get(&mut self, key: &str) -> Option<Vec<u8>>;
    /// Refused past the app's storage limit.
    fn kv_set(&mut self, key: &str, value: &[u8]) -> Result<(), String>;
    fn kv_delete(&mut self, key: &str);
    /// An event for the agents subscribed to `kind`.
    fn emit(&mut self, kind: &str, text: &str, data: Option<&Value>, dedupe: Option<&str>, scope: &str);
    fn set_timer(&mut self, name: &str, after_ms: u64);
    /// One line the person sees under the app in Settings → Apps.
    fn set_status(&mut self, text: &str);
    fn log(&mut self, level: Level, message: &str);
    /// Calls the Bot API with the bot's token and waits for the answer.
    fn call(&mut self, token: &str, request: &Request) -> Result<Response, String>;
    /// Starts the long poll, which goes on while the app isn't running.
    fn start_poll(&mut self, token: &str, request: &Request) -> Result<(), String>;
    /// The long poll's answer once it's come, without waiting; None while
    /// it's still waiting or when there's none.
    fn poll_answer(&mut self) -> Option<Result<Response, String>>;
    /// Whether a long poll is under way.
    fn polling(&self) -> bool;
    /// Seconds since 1970.
    fn now(&mut self) -> u64;
    /// A random number, for link codes.
    fn random(&mut self) -> u64;
}

/// The person's chat the bot answers, once linked.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Linked {
    pub chat_id: i64,
    pub user_id: i64,
    pub name: String,
    #[serde(default)]
    pub username: String,
}

/// One message in the chat, for `telegram_history`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Said {
    /// "you" for the person, else the agent's id.
    pub from: String,
    pub text: String,
    /// Seconds since 1970.
    pub at: u64,
}

/// A tool's answer.
#[derive(Debug, Clone, PartialEq)]
pub struct ToolResult {
    pub text: String,
    pub data: Option<Value>,
    pub is_error: bool,
}

impl ToolResult {
    fn ok(text: impl Into<String>) -> Self {
        Self { text: text.into(), data: None, is_error: false }
    }

    fn error(text: impl Into<String>) -> Self {
        Self { text: text.into(), data: None, is_error: true }
    }
}

const KEY_LINKED: &str = "linked";
const KEY_CODE: &str = "link_code";
const KEY_OFFSET: &str = "offset";
const KEY_HISTORY: &str = "history";
const KEY_BOT: &str = "bot";
const TIMER_POLL: &str = "poll";

/// Waits after a failed poll: 5 s, then 30 s, then 5 minutes.
const BACKOFF_MS: [u64; 3] = [5_000, 30_000, 300_000];

/// The app, between calls: everything lasting is in the host's store.
#[derive(Default)]
pub struct App {
    /// Polls that failed in a row.
    failures: usize,
    /// Whether the app is started (polling).
    running: bool,
}

impl App {
    pub fn start(&mut self, host: &mut impl Host) {
        self.running = true;
        self.failures = 0;
        let Some(token) = token(host) else {
            host.set_status("Add the bot token from @BotFather in the app's setup.");
            return;
        };
        match host.call(&token, &Request { method: "getMe".into(), params: json!({}) }) {
            Ok(response) => match answer(&response) {
                Ok(me) => {
                    let username = me["username"].as_str().unwrap_or_default().to_owned();
                    let _ = host.kv_set(KEY_BOT, username.as_bytes());
                }
                Err(problem) => {
                    host.set_status(&problem);
                    host.log(Level::Warn, &format!("getMe: {problem}"));
                    host.set_timer(TIMER_POLL, BACKOFF_MS[0]);
                    return;
                }
            },
            Err(problem) => host.log(Level::Warn, &format!("getMe: {problem}")),
        }
        show_status(host);
        host.set_timer(TIMER_POLL, 0);
    }

    pub fn stop(&mut self, _host: &mut impl Host) {
        self.running = false;
    }

    pub fn on_timer(&mut self, host: &mut impl Host, name: &str) {
        if name != TIMER_POLL || !self.running {
            return;
        }
        let Some(token) = token(host) else {
            host.set_status("Add the bot token from @BotFather in the app's setup.");
            return;
        };
        if host.polling() {
            match host.poll_answer() {
                None => {}
                Some(Ok(response)) => match answer(&response) {
                    Ok(updates) => {
                        if self.failures > 0 {
                            self.failures = 0;
                            show_status(host);
                        }
                        for update in updates.as_array().into_iter().flatten() {
                            self.update(host, &token, update);
                        }
                    }
                    Err(problem) => return self.failed(host, &problem),
                },
                Some(Err(problem)) => return self.failed(host, &problem),
            }
        }
        if !host.polling() {
            let offset = load::<i64>(host, KEY_OFFSET).unwrap_or(0);
            let request = Request {
                method: "getUpdates".into(),
                params: json!({
                    "offset": offset,
                    "timeout": POLL_SECONDS,
                    "allowed_updates": ["message"],
                }),
            };
            if let Err(problem) = host.start_poll(&token, &request) {
                return self.failed(host, &problem);
            }
        }
        host.set_timer(TIMER_POLL, TICK_MS);
    }

    fn failed(&mut self, host: &mut impl Host, problem: &str) {
        let wait = BACKOFF_MS[self.failures.min(BACKOFF_MS.len() - 1)];
        self.failures += 1;
        host.log(Level::Warn, &format!("getUpdates: {problem}"));
        if self.failures >= 2 {
            host.set_status(problem);
        }
        host.set_timer(TIMER_POLL, wait);
    }

    /// One update from getUpdates: a message from the person becomes an
    /// event; "/start <code>" links the chat that sends it.
    fn update(&mut self, host: &mut impl Host, token: &str, update: &Value) {
        let Some(id) = update["update_id"].as_i64() else { return };
        // Seen, whatever comes of it.
        store(host, KEY_OFFSET, &(id + 1));
        let message = &update["message"];
        let (Some(chat), Some(user)) = (message["chat"]["id"].as_i64(), message["from"]["id"].as_i64()) else {
            return;
        };
        if message["chat"]["type"].as_str() != Some("private") {
            return;
        }
        let text = message_text(message);
        let name = display_name(&message["from"]);
        let linked = load::<Linked>(host, KEY_LINKED);
        match linked {
            None => {
                let code = link_code(host);
                let sent = text.trim();
                if sent == format!("/start {code}") {
                    let linked = Linked {
                        chat_id: chat,
                        user_id: user,
                        name: name.clone(),
                        username: message["from"]["username"].as_str().unwrap_or_default().to_owned(),
                    };
                    store(host, KEY_LINKED, &linked);
                    host.kv_delete(KEY_CODE);
                    show_status(host);
                    let _ = send(
                        host,
                        token,
                        chat,
                        "Linked. Messages you send here go to your agents in Claktory, and they can message you here.",
                    );
                } else if sent.starts_with("/start") {
                    let _ = send(
                        host,
                        token,
                        chat,
                        "That code doesn't match. Use the link shown in Claktory, under Settings → Apps → Telegram.",
                    );
                }
            }
            Some(linked) if linked.chat_id == chat && linked.user_id == user => {
                if text.trim() == "/start" || text.trim().starts_with("/start ") {
                    let _ = send(host, token, chat, "This chat is already linked to your Claktory.");
                    return;
                }
                if text.is_empty() {
                    return;
                }
                let at = message["date"].as_u64().unwrap_or_else(|| host.now());
                remember(host, Said { from: "you".into(), text: text.clone(), at });
                let data = json!({
                    "message_id": message["message_id"],
                    "date": message["date"],
                    "from": name,
                });
                host.emit("message", &format!("{name}: {text}"), Some(&data), Some(&format!("update:{id}")), "read");
            }
            // Anyone else: the bot answers only its person.
            Some(_) => {}
        }
    }

    pub fn call(&mut self, host: &mut impl Host, tool: &str, args: &str, caller: &str) -> ToolResult {
        let args: Value = match serde_json::from_str(if args.trim().is_empty() { "{}" } else { args }) {
            Ok(args) => args,
            Err(error) => return ToolResult::error(format!("The arguments aren't JSON: {error}")),
        };
        match tool {
            "telegram_send" => self.telegram_send(host, &args, caller),
            "telegram_history" => telegram_history(host, &args),
            _ => ToolResult::error(format!("Telegram has no tool {tool}.")),
        }
    }

    fn telegram_send(&mut self, host: &mut impl Host, args: &Value, caller: &str) -> ToolResult {
        let Some(text) = args["text"].as_str().filter(|text| !text.trim().is_empty()) else {
            return ToolResult::error("Give the message as text.");
        };
        let Some(token) = token(host) else {
            return ToolResult::error("Telegram isn't set up yet: its bot token is missing.");
        };
        let Some(linked) = load::<Linked>(host, KEY_LINKED) else {
            return ToolResult::error("Telegram isn't linked to anyone yet, so there's no one to message.");
        };
        for chunk in chunks(text, MESSAGE_MAX - 96) {
            if let Err(problem) = send(host, &token, linked.chat_id, &chunk) {
                return ToolResult::error(format!("Telegram didn't take the message: {problem}"));
            }
        }
        let at = host.now();
        remember(host, Said { from: caller.to_owned(), text: text.to_owned(), at });
        ToolResult::ok(format!("Sent to {} on Telegram.", linked.name))
    }
}

/// The recent messages, oldest first.
fn telegram_history(host: &mut impl Host, args: &Value) -> ToolResult {
    let limit = args["limit"].as_u64().unwrap_or(20).clamp(1, HISTORY_KEPT as u64) as usize;
    let history = load::<Vec<Said>>(host, KEY_HISTORY).unwrap_or_default();
    let recent = &history[history.len().saturating_sub(limit)..];
    if recent.is_empty() {
        return ToolResult::ok("No messages yet.");
    }
    let name = load::<Linked>(host, KEY_LINKED).map(|linked| linked.name).unwrap_or_else(|| "You".into());
    let lines: Vec<String> = recent
        .iter()
        .map(|said| {
            let who = if said.from == "you" { name.clone() } else { format!("agent {}", said.from) };
            format!("[{}] {who}: {}", clock(said.at), said.text)
        })
        .collect();
    ToolResult { text: lines.join("\n"), data: Some(json!(recent)), is_error: false }
}

/// Sends one message, as HTML from Markdown, or as plain text if Telegram
/// can't read the HTML.
fn send(host: &mut impl Host, token: &str, chat: i64, text: &str) -> Result<(), String> {
    let html = to_telegram_html(text);
    let request = Request {
        method: "sendMessage".into(),
        params: json!({"chat_id": chat, "text": html, "parse_mode": "HTML", "link_preview_options": {"is_disabled": true}}),
    };
    match host.call(token, &request).and_then(|response| answer(&response)) {
        Ok(_) => Ok(()),
        Err(problem) if problem.contains("can't parse entities") => {
            let plain = Request {
                method: "sendMessage".into(),
                params: json!({"chat_id": chat, "text": text, "link_preview_options": {"is_disabled": true}}),
            };
            host.call(token, &plain).and_then(|response| answer(&response)).map(|_| ())
        }
        Err(problem) => Err(problem),
    }
}

/// The Bot API's `result`, or its description of what went wrong in
/// words the person can act on.
fn answer(response: &Response) -> Result<Value, String> {
    let body: Value = serde_json::from_slice(&response.body).unwrap_or(Value::Null);
    if body["ok"].as_bool() == Some(true) {
        return Ok(body["result"].clone());
    }
    let description = body["description"].as_str().unwrap_or("no answer");
    Err(match response.status {
        401 | 404 => "Telegram refused the bot token. Check it in the app's setup.".to_owned(),
        409 => "Another program is reading this bot's messages. Stop it, or make a new bot for Claktory.".to_owned(),
        status => format!("Telegram answered {status}: {description}"),
    })
}

fn token(host: &mut impl Host) -> Option<String> {
    host.secret("bot_token").map(|token| token.trim().to_owned()).filter(|token| !token.is_empty())
}

/// The code that links a chat: made once, kept until it's used.
pub fn link_code(host: &mut impl Host) -> String {
    if let Some(code) = host.kv_get(KEY_CODE).and_then(|code| String::from_utf8(code).ok()) {
        return code;
    }
    // No 0/O or 1/I, for reading it off a screen.
    const LETTERS: &[u8] = b"ABCDEFGHJKLMNPQRSTUVWXYZ23456789";
    let mut bits = host.random();
    let code: String = (0..8)
        .map(|_| {
            let letter = LETTERS[(bits % LETTERS.len() as u64) as usize] as char;
            bits /= LETTERS.len() as u64;
            letter
        })
        .collect();
    let _ = host.kv_set(KEY_CODE, code.as_bytes());
    code
}

/// Says in Settings → Apps whom it's linked to, or how to link it.
fn show_status(host: &mut impl Host) {
    if let Some(linked) = load::<Linked>(host, KEY_LINKED) {
        let who = if linked.username.is_empty() { linked.name } else { format!("{} (@{})", linked.name, linked.username) };
        host.set_status(&format!("Linked to {who}."));
        return;
    }
    let code = link_code(host);
    let bot = host.kv_get(KEY_BOT).and_then(|bot| String::from_utf8(bot).ok()).unwrap_or_default();
    if bot.is_empty() {
        host.set_status(&format!("To link your Telegram, send the bot /start {code}."));
    } else {
        host.set_status(&format!(
            "To link your Telegram, open https://t.me/{bot}?start={code} and tap Start, or send @{bot} /start {code}."
        ));
    }
}

fn remember(host: &mut impl Host, said: Said) {
    let mut history = load::<Vec<Said>>(host, KEY_HISTORY).unwrap_or_default();
    history.push(said);
    let extra = history.len().saturating_sub(HISTORY_KEPT);
    history.drain(..extra);
    store(host, KEY_HISTORY, &history);
}

/// A message's text: its text, or for a photo, file and the like, what it
/// is and its caption.
fn message_text(message: &Value) -> String {
    if let Some(text) = message["text"].as_str() {
        return text.to_owned();
    }
    let caption = message["caption"].as_str().unwrap_or_default();
    let kind = [
        ("photo", "a photo"),
        ("document", "a file"),
        ("voice", "a voice message"),
        ("audio", "audio"),
        ("video", "a video"),
        ("sticker", "a sticker"),
        ("location", "a location"),
        ("contact", "a contact"),
    ]
    .into_iter()
    .find(|(key, _)| !message[*key].is_null())
    .map(|(_, what)| what);
    match (kind, caption.is_empty()) {
        (Some(what), true) => format!("[sent {what}, which Claktory can't show yet]"),
        (Some(what), false) => format!("[sent {what}, which Claktory can't show yet] {caption}"),
        (None, _) => caption.to_owned(),
    }
}

fn display_name(user: &Value) -> String {
    let first = user["first_name"].as_str().unwrap_or_default();
    let last = user["last_name"].as_str().unwrap_or_default();
    let name = format!("{first} {last}").trim().to_owned();
    if name.is_empty() { user["username"].as_str().unwrap_or("Someone").to_owned() } else { name }
}

/// [text] in pieces Telegram takes, split between paragraphs, then lines,
/// then anywhere.
pub fn chunks(text: &str, max: usize) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = text.trim();
    while rest.chars().count() > max {
        let cut = rest.char_indices().nth(max).map(|(at, _)| at).unwrap_or(rest.len());
        let head = &rest[..cut];
        let at = head.rfind("\n\n").or_else(|| head.rfind('\n')).filter(|at| *at > 0).unwrap_or(cut);
        out.push(rest[..at].trim_end().to_owned());
        rest = rest[at..].trim_start();
    }
    if !rest.is_empty() {
        out.push(rest.to_owned());
    }
    out
}

/// "2026-11-20 14:02" (UTC) for seconds since 1970.
fn clock(seconds: u64) -> String {
    let days = (seconds / 86_400) as i64;
    let minutes = (seconds % 86_400) / 60;
    // Civil date from days since 1970 (Howard Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!("{year:04}-{month:02}-{day:02} {:02}:{:02}", minutes / 60, minutes % 60)
}

fn load<T: for<'de> Deserialize<'de>>(host: &mut impl Host, key: &str) -> Option<T> {
    host.kv_get(key).and_then(|bytes| serde_json::from_slice(&bytes).ok())
}

fn store<T: Serialize>(host: &mut impl Host, key: &str, value: &T) {
    if let Ok(bytes) = serde_json::to_vec(value)
        && let Err(problem) = host.kv_set(key, &bytes)
    {
        host.log(Level::Error, &format!("Couldn't keep {key}: {problem}"));
    }
}

#[cfg(test)]
mod tests;
