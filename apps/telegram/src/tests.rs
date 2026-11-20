use std::collections::{HashMap, VecDeque};

use serde_json::{Value, json};

use super::*;

/// An event: kind, text, data, dedupe key and scope.
type Emitted = (String, String, Option<Value>, Option<String>, String);

/// A stand-in for Claktory's app host and Telegram: what the app asked
/// for, and answers queued per Bot API method.
#[derive(Default)]
struct Fake {
    secrets: HashMap<String, String>,
    kv: HashMap<String, Vec<u8>>,
    events: Vec<Emitted>,
    timers: Vec<(String, u64)>,
    status: String,
    logs: Vec<String>,
    /// Calls made, with their parameters.
    calls: Vec<Request>,
    /// Answers, by method; `{"ok":true,"result":…}` with `true` otherwise.
    answers: HashMap<String, VecDeque<Result<Response, String>>>,
    poll: Option<Request>,
    poll_answer: Option<Result<Response, String>>,
    clock: u64,
}

fn ok(result: Value) -> Result<Response, String> {
    Ok(Response { status: 200, body: serde_json::to_vec(&json!({"ok": true, "result": result})).unwrap() })
}

fn refused(status: u16, description: &str) -> Result<Response, String> {
    Ok(Response { status, body: serde_json::to_vec(&json!({"ok": false, "description": description})).unwrap() })
}

impl Fake {
    fn with_token() -> Self {
        let mut fake = Fake { clock: 1_795_176_000, ..Fake::default() };
        fake.secrets.insert("bot_token".into(), "123:abc".into());
        fake.answer("getMe", ok(json!({"username": "claktory_bot"})));
        fake
    }

    fn answer(&mut self, method: &str, answer: Result<Response, String>) {
        self.answers.entry(method.into()).or_default().push_back(answer);
    }

    fn sent(&self) -> Vec<&Request> {
        self.calls.iter().filter(|call| call.method == "sendMessage").collect()
    }

    fn timer(&self) -> Option<u64> {
        self.timers.last().map(|(_, ms)| *ms)
    }
}

impl Host for Fake {
    fn secret(&mut self, name: &str) -> Option<String> {
        self.secrets.get(name).cloned()
    }
    fn kv_get(&mut self, key: &str) -> Option<Vec<u8>> {
        self.kv.get(key).cloned()
    }
    fn kv_set(&mut self, key: &str, value: &[u8]) -> Result<(), String> {
        self.kv.insert(key.into(), value.to_vec());
        Ok(())
    }
    fn kv_delete(&mut self, key: &str) {
        self.kv.remove(key);
    }
    fn emit(&mut self, kind: &str, text: &str, data: Option<&Value>, dedupe: Option<&str>, scope: &str) {
        self.events.push((kind.into(), text.into(), data.cloned(), dedupe.map(Into::into), scope.into()));
    }
    fn set_timer(&mut self, name: &str, after_ms: u64) {
        self.timers.push((name.into(), after_ms));
    }
    fn set_status(&mut self, text: &str) {
        self.status = text.into();
    }
    fn log(&mut self, _level: Level, message: &str) {
        self.logs.push(message.into());
    }
    fn call(&mut self, token: &str, request: &Request) -> Result<Response, String> {
        assert_eq!(token, "123:abc");
        self.calls.push(request.clone());
        self.answers.get_mut(&request.method).and_then(VecDeque::pop_front).unwrap_or_else(|| ok(json!(true)))
    }
    fn start_poll(&mut self, token: &str, request: &Request) -> Result<(), String> {
        assert_eq!(token, "123:abc");
        assert!(self.poll.is_none(), "one poll at a time");
        self.poll = Some(request.clone());
        Ok(())
    }
    fn poll_answer(&mut self) -> Option<Result<Response, String>> {
        let answer = self.poll_answer.take()?;
        self.poll = None;
        Some(answer)
    }
    fn polling(&self) -> bool {
        self.poll.is_some()
    }
    fn now(&mut self) -> u64 {
        self.clock
    }
    fn random(&mut self) -> u64 {
        0x1234_5678_9abc
    }
}

fn message(update: i64, user: i64, text: &str) -> Value {
    json!({
        "update_id": update,
        "message": {
            "message_id": update * 10,
            "date": 1_795_176_100,
            "chat": {"id": user, "type": "private"},
            "from": {"id": user, "first_name": "Matt", "username": "matt"},
            "text": text,
        }
    })
}

/// Started, with Telegram answering one poll with [updates].
fn deliver(app: &mut App, fake: &mut Fake, updates: Vec<Value>) {
    if !fake.polling() {
        app.on_timer(fake, "poll");
    }
    fake.poll_answer = Some(ok(Value::Array(updates)));
    app.on_timer(fake, "poll");
}

fn started() -> (App, Fake) {
    let mut fake = Fake::with_token();
    let mut app = App::default();
    app.start(&mut fake);
    (app, fake)
}

fn linked() -> (App, Fake) {
    let (mut app, mut fake) = started();
    let code = link_code(&mut fake);
    deliver(&mut app, &mut fake, vec![message(1, 42, &format!("/start {code}"))]);
    fake.calls.clear();
    (app, fake)
}

#[test]
fn without_a_token_it_asks_for_one_and_doesnt_poll() {
    let mut fake = Fake::default();
    let mut app = App::default();
    app.start(&mut fake);
    assert_eq!(fake.status, "Add the bot token from @BotFather in the app's setup.");
    assert!(fake.calls.is_empty());
    assert!(fake.timers.is_empty());
}

#[test]
fn started_it_says_how_to_link_and_polls() {
    let (mut app, mut fake) = started();
    assert_eq!(fake.calls[0].method, "getMe");
    let code = link_code(&mut fake);
    assert_eq!(code.len(), 8);
    assert!(code.chars().all(|c| c.is_ascii_uppercase() || ('2'..='9').contains(&c)), "{code}");
    assert_eq!(
        fake.status,
        format!("To link your Telegram, open https://t.me/claktory_bot?start={code} and tap Start, or send @claktory_bot /start {code}.")
    );
    // At once, then a long poll from the start.
    assert_eq!(fake.timer(), Some(0));
    app.on_timer(&mut fake, "poll");
    let poll = fake.poll.clone().unwrap();
    assert_eq!(poll.method, "getUpdates");
    assert_eq!(poll.params["offset"], 0);
    assert_eq!(poll.params["timeout"], POLL_SECONDS);
    assert_eq!(fake.timer(), Some(TICK_MS));
    // Still waiting: it looks again without starting another.
    app.on_timer(&mut fake, "poll");
    assert_eq!(fake.poll, Some(poll));
    // The same code after a restart.
    let mut again = App::default();
    again.start(&mut fake);
    assert_eq!(link_code(&mut fake), code);
}

#[test]
fn start_with_the_code_links_that_chat() {
    let (mut app, mut fake) = started();
    let code = link_code(&mut fake);
    deliver(&mut app, &mut fake, vec![message(7, 42, &format!("/start {code}"))]);
    let linked: Linked = serde_json::from_slice(&fake.kv["linked"]).unwrap();
    assert_eq!(linked, Linked { chat_id: 42, user_id: 42, name: "Matt".into(), username: "matt".into() });
    assert_eq!(fake.status, "Linked to Matt (@matt).");
    assert!(!fake.kv.contains_key("link_code"));
    assert_eq!(fake.sent()[0].params["chat_id"], 42);
    assert!(fake.sent()[0].params["text"].as_str().unwrap().starts_with("Linked."));
    // Seen: the next poll starts after it.
    assert_eq!(fake.kv["offset"], b"8");
    app.on_timer(&mut fake, "poll");
    assert_eq!(fake.poll.as_ref().unwrap().params["offset"], 8);
    // Linking makes no event.
    assert!(fake.events.is_empty());
}

#[test]
fn a_wrong_code_links_nothing() {
    let (mut app, mut fake) = started();
    deliver(&mut app, &mut fake, vec![message(1, 42, "/start NOPE"), message(2, 42, "hello")]);
    assert!(!fake.kv.contains_key("linked"));
    assert_eq!(fake.sent().len(), 1, "only the /start is answered");
    assert!(fake.sent()[0].params["text"].as_str().unwrap().starts_with("That code doesn't match."));
    assert!(fake.events.is_empty());
}

#[test]
fn the_persons_messages_become_events_and_nobody_elses() {
    let (mut app, mut fake) = linked();
    let mut stranger = message(3, 99, "let me in");
    stranger["message"]["from"]["first_name"] = json!("Eve");
    let mut group = message(4, 42, "in a group");
    group["message"]["chat"] = json!({"id": -5, "type": "group"});
    deliver(&mut app, &mut fake, vec![message(2, 42, "Can you check the deploy?"), stranger, group]);
    assert_eq!(fake.events.len(), 1);
    let (kind, text, data, dedupe, scope) = &fake.events[0];
    assert_eq!(kind, "message");
    assert_eq!(text, "Matt: Can you check the deploy?");
    assert_eq!(data.as_ref().unwrap()["message_id"], 20);
    assert_eq!(dedupe.as_deref(), Some("update:2"));
    assert_eq!(scope, "read");
    assert!(fake.sent().is_empty(), "strangers get no answer");
    assert_eq!(fake.kv["offset"], b"5");
}

#[test]
fn a_photo_says_what_it_is() {
    let (mut app, mut fake) = linked();
    let mut photo = message(2, 42, "");
    photo["message"].as_object_mut().unwrap().remove("text");
    photo["message"]["photo"] = json!([{"file_id": "x"}]);
    photo["message"]["caption"] = json!("the dashboard");
    deliver(&mut app, &mut fake, vec![photo]);
    assert_eq!(fake.events[0].1, "Matt: [sent a photo, which Claktory can't show yet] the dashboard");
}

#[test]
fn send_needs_a_linked_chat() {
    let (mut app, mut fake) = started();
    let result = app.call(&mut fake, "telegram_send", r#"{"text":"hi"}"#, "rivet");
    assert!(result.is_error);
    assert_eq!(result.text, "Telegram isn't linked to anyone yet, so there's no one to message.");
    let result = app.call(&mut fake, "telegram_send", "{}", "rivet");
    assert_eq!(result.text, "Give the message as text.");
}

#[test]
fn send_messages_the_person_in_html_and_remembers_it() {
    let (mut app, mut fake) = linked();
    let result = app.call(&mut fake, "telegram_send", r#"{"text":"The **build** failed: `cargo test`"}"#, "rivet");
    assert!(!result.is_error, "{}", result.text);
    assert_eq!(result.text, "Sent to Matt on Telegram.");
    let sent = fake.sent()[0];
    assert_eq!(sent.params["chat_id"], 42);
    assert_eq!(sent.params["parse_mode"], "HTML");
    assert_eq!(sent.params["text"], "The <b>build</b> failed: <code>cargo test</code>");

    let history = app.call(&mut fake, "telegram_history", "{}", "rivet");
    assert_eq!(history.text, "[2026-11-20 12:00] agent rivet: The **build** failed: `cargo test`");
}

#[test]
fn html_telegram_cant_read_goes_as_plain_text() {
    let (mut app, mut fake) = linked();
    fake.answer("sendMessage", refused(400, "Bad Request: can't parse entities: unsupported tag"));
    let result = app.call(&mut fake, "telegram_send", r#"{"text":"a <b> c"}"#, "rivet");
    assert!(!result.is_error, "{}", result.text);
    let sent = fake.sent();
    assert_eq!(sent.len(), 2);
    assert_eq!(sent[1].params["text"], "a <b> c");
    assert!(sent[1].params.get("parse_mode").is_none());
}

#[test]
fn a_long_message_goes_in_pieces() {
    let (mut app, mut fake) = linked();
    let paragraph = "word ".repeat(500);
    let text = format!("{paragraph}\n\n{paragraph}\n\n{paragraph}");
    let result = app.call(&mut fake, "telegram_send", &json!({"text": text}).to_string(), "rivet");
    assert!(!result.is_error);
    // One per paragraph: two don't fit in one message.
    assert_eq!(fake.sent().len(), 3);
    for sent in fake.sent() {
        assert!(sent.params["text"].as_str().unwrap().chars().count() <= MESSAGE_MAX);
    }
}

#[test]
fn a_refused_send_says_why() {
    let (mut app, mut fake) = linked();
    fake.answer("sendMessage", refused(403, "Forbidden: bot was blocked by the user"));
    let result = app.call(&mut fake, "telegram_send", r#"{"text":"hi"}"#, "rivet");
    assert!(result.is_error);
    assert_eq!(result.text, "Telegram didn't take the message: Telegram answered 403: Forbidden: bot was blocked by the user");
}

#[test]
fn history_is_both_sides_newest_last_and_kept_short() {
    let (mut app, mut fake) = linked();
    deliver(&mut app, &mut fake, vec![message(2, 42, "first")]);
    app.call(&mut fake, "telegram_send", r#"{"text":"second"}"#, "june");
    let all = app.call(&mut fake, "telegram_history", "", "june");
    assert_eq!(all.text, "[2026-11-20 12:01] Matt: first\n[2026-11-20 12:00] agent june: second");
    let last = app.call(&mut fake, "telegram_history", r#"{"limit":1}"#, "june");
    assert_eq!(last.text, "[2026-11-20 12:00] agent june: second");
    for n in 0..HISTORY_KEPT + 5 {
        app.call(&mut fake, "telegram_send", &json!({"text": format!("m{n}")}).to_string(), "june");
    }
    let kept: Vec<Said> = serde_json::from_slice(&fake.kv["history"]).unwrap();
    assert_eq!(kept.len(), HISTORY_KEPT);
    assert_eq!(kept.last().unwrap().text, format!("m{}", HISTORY_KEPT + 4));
}

#[test]
fn failed_polls_back_off_and_say_why() {
    let (mut app, mut fake) = started();
    app.on_timer(&mut fake, "poll");
    fake.poll_answer = Some(refused(401, "Unauthorized"));
    app.on_timer(&mut fake, "poll");
    assert_eq!(fake.timer(), Some(5_000));
    // One failure can be a blip: the status waits for the second.
    assert!(fake.status.starts_with("To link"));
    app.on_timer(&mut fake, "poll");
    fake.poll_answer = Some(Err("connection reset".into()));
    app.on_timer(&mut fake, "poll");
    assert_eq!(fake.timer(), Some(30_000));
    assert_eq!(fake.status, "connection reset");
    // Working again, it says how to link again.
    app.on_timer(&mut fake, "poll");
    fake.poll_answer = Some(ok(json!([])));
    app.on_timer(&mut fake, "poll");
    assert!(fake.status.starts_with("To link"));
    assert_eq!(fake.timer(), Some(TICK_MS));
}

#[test]
fn stopped_it_stops_polling() {
    let (mut app, mut fake) = started();
    app.stop(&mut fake);
    app.on_timer(&mut fake, "poll");
    assert!(fake.poll.is_none());
}

#[test]
fn unknown_tools_and_bad_arguments_say_so() {
    let (mut app, mut fake) = linked();
    assert_eq!(app.call(&mut fake, "telegram_call", "{}", "x").text, "Telegram has no tool telegram_call.");
    assert!(app.call(&mut fake, "telegram_send", "{", "x").text.starts_with("The arguments aren't JSON"));
}

#[test]
fn markdown_reads_as_telegram_html() {
    assert_eq!(to_telegram_html("a < b & c > d"), "a &lt; b &amp; c &gt; d");
    assert_eq!(to_telegram_html("**bold** and *it* and _it_ and ~~gone~~"), "<b>bold</b> and <i>it</i> and <i>it</i> and <s>gone</s>");
    assert_eq!(to_telegram_html("snake_case_name stays"), "snake_case_name stays");
    assert_eq!(to_telegram_html("2 * 3 * 4"), "2 * 3 * 4");
    assert_eq!(to_telegram_html("`a<b>`"), "<code>a&lt;b&gt;</code>");
    assert_eq!(
        to_telegram_html("[the docs](https://claktory.com/x?a=1&b=\"2\")"),
        "<a href=\"https://claktory.com/x?a=1&amp;b=&quot;2&quot;\">the docs</a>"
    );
    assert_eq!(to_telegram_html("[local](file:///etc)"), "[local](file:///etc)");
    assert_eq!(to_telegram_html("# Title\n- one\n- **two**"), "<b>Title</b>\n• one\n• <b>two</b>");
    assert_eq!(to_telegram_html("```rust\nlet a = b < c;\n```"), "<pre><code class=\"language-rust\">let a = b &lt; c;</code></pre>");
    assert_eq!(to_telegram_html("> quoted\n> more\n\nafter"), "<blockquote>quoted\nmore</blockquote>\n\nafter");
    assert_eq!(to_telegram_html("not **closed"), "not **closed");
    assert_eq!(to_telegram_html(r"\*literal\*"), "*literal*");
}

#[test]
fn pieces_split_between_paragraphs_then_lines() {
    assert_eq!(chunks("short", 10), vec!["short"]);
    assert_eq!(chunks("aaaa\n\nbbbb\ncccc", 12), vec!["aaaa", "bbbb\ncccc"]);
    assert_eq!(chunks("aaaaaaaaaa", 4), vec!["aaaa", "aaaa", "aa"]);
}

#[test]
fn times_read_as_utc_dates() {
    assert_eq!(clock(0), "1970-01-01 00:00");
    assert_eq!(clock(1_795_176_000), "2026-11-20 12:00");
    assert_eq!(clock(951_782_400), "2000-02-29 00:00");
}
