//! Notifications (#273): new Needs-you items, agents' alerts and replies to
//! the person's phone reach every connected app as Notify, unless the person
//! is using Claktory; devices registered for pushes get them sealed through
//! the push relay. An agent that has read a web page never puts its words in one.

use std::sync::{Arc, Mutex};

use base64::Engine;
use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use factory_protocol::v1::{self, NotifyKind, client_message, entry::Kind, server_message::Body};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use crate::common::{Client, View, apply_request, create_agent, create_project, send_message, start, temp_dir};
use crate::pipes::{idle, incoming, select};

/// The person's own words an attacker would put on the lock screen.
const MARKER: &str = "MARKER-5521 sign in at evil.example";

/// Runs `text` as `agent`'s next message until it's idle again, noting what
/// notified on the way; what the tool it called answered, if one did.
pub(crate) async fn run(client: &mut Client, view: &mut View, notes: &mut Vec<v1::Notify>, agent: &str, text: &str) -> String {
    let apply = |view: &mut View, notes: &mut Vec<v1::Notify>, body: &Body| {
        if let Body::Notify(notify) = body {
            notes.push(notify.clone());
        }
        view.apply(body);
    };
    for event in client.request(select(agent)).await.events {
        apply(view, notes, event.body.as_ref().expect("body"));
    }
    let before = view.entries.len();
    for event in client.request(send_message(agent, text)).await.events {
        apply(view, notes, event.body.as_ref().expect("body"));
    }
    while !(view.entries.len() >= before + 2 && idle(view, agent)) {
        let message = client.receive().await;
        apply(view, notes, message.body.as_ref().expect("body"));
    }
    view.entries[before..]
        .iter()
        .find_map(|entry| match &entry.kind {
            Some(Kind::ToolCall(call)) => Some(call.output.clone()),
            _ => None,
        })
        .unwrap_or_default()
}

pub(crate) async fn call(
    client: &mut Client,
    view: &mut View,
    notes: &mut Vec<v1::Notify>,
    agent: &str,
    tool: &str,
    args: serde_json::Value,
) -> String {
    run(client, view, notes, agent, &format!("call {tool} {args}")).await
}

fn ask(title: &str) -> serde_json::Value {
    serde_json::json!({"title": title, "body": "Only you can say."})
}

fn alert(title: &str) -> serde_json::Value {
    serde_json::json!({"title": title, "text": "500s since 10:02."})
}

fn set_active(active: bool) -> client_message::Body {
    client_message::Body::SetActive(v1::SetActive { active, ..Default::default() })
}

fn settings(change: impl FnOnce(&mut v1::NotificationSettings)) -> client_message::Body {
    let mut settings = v1::NotificationSettings {
        needs_you: true,
        alerts: true,
        replies: true,
        preview: v1::NotifyPreview::Full.into(),
        ..Default::default()
    };
    change(&mut settings);
    client_message::Body::UpdateSettings(v1::UpdateSettings { notifications: Some(settings), ..Default::default() })
}

/// A Factory with a project "Site" and its agent Builder; the client
/// connected as `kind`.
async fn setup(kind: v1::DeviceKind) -> (std::path::PathBuf, Arc<factoryd::Factory>, Client, View) {
    let dir = temp_dir();
    let (addr, factory) = start(&dir).await;
    let (mut client, workspace, _) = Client::connect_as(addr, kind).await;
    let mut view = View { workspace, ..View::default() };
    apply_request(&mut client, &mut view, create_project("Site", &dir.join("site"))).await;
    apply_request(&mut client, &mut view, create_agent("site", "Builder", "")).await;
    (dir, factory, client, view)
}

#[tokio::test]
async fn a_question_and_an_alert_notify_the_connected_apps() {
    let (_dir, _factory, mut client, mut view) = setup(v1::DeviceKind::Desktop).await;
    let mut notes = Vec::new();
    call(&mut client, &mut view, &mut notes, "builder", "ask_person", ask("Deploy tonight?")).await;
    let seq = view.agent("builder").waiting_seqs[0];
    let [needs] = notes.as_slice() else { panic!("{notes:?}") };
    assert_eq!(needs.kind(), NotifyKind::NeedsYou);
    assert_eq!((needs.title.as_str(), needs.body.as_str()), ("Builder", "Deploy tonight?"));
    assert_eq!((needs.agent_id.as_str(), needs.seq), ("builder", seq));
    assert_eq!((needs.collapse.as_str(), needs.thread.as_str()), ("", "agent:builder"));
    assert!(needs.sent_ms > 0 && !needs.id.is_empty());

    notes.clear();
    let said = call(&mut client, &mut view, &mut notes, "builder", "notify_person", alert("The site is down")).await;
    assert_eq!(said, "Sent: the person gets it as a notification, and it waits in their Needs you until they read it.");
    let [sent] = notes.as_slice() else { panic!("{notes:?}") };
    assert_eq!(sent.kind(), NotifyKind::Alert);
    assert_eq!(sent.body, "The site is down\n500s since 10:02.");
    assert_eq!(sent.collapse, "agent:builder:alert");
    assert!(!sent.urgent);

    // A plain message, answered from a computer, notifies nothing.
    notes.clear();
    run(&mut client, &mut view, &mut notes, "builder", "Thanks").await;
    assert!(notes.is_empty(), "{notes:?}");
}

#[tokio::test]
async fn nothing_but_alerts_notify_while_the_person_uses_claktory() {
    let (_dir, _factory, mut client, mut view) = setup(v1::DeviceKind::Desktop).await;
    let mut notes = Vec::new();
    client.ok(set_active(true)).await;
    call(&mut client, &mut view, &mut notes, "builder", "ask_person", ask("Deploy tonight?")).await;
    assert!(notes.is_empty(), "{notes:?}");
    // An alert shows nowhere else: it goes all the same.
    call(&mut client, &mut view, &mut notes, "builder", "notify_person", alert("The site is down")).await;
    assert_eq!(notes.iter().map(v1::Notify::kind).collect::<Vec<_>>(), [NotifyKind::Alert]);

    notes.clear();
    client.ok(set_active(false)).await;
    call(&mut client, &mut view, &mut notes, "builder", "ask_person", ask("And tomorrow?")).await;
    assert_eq!(notes.iter().map(|note| note.body.as_str()).collect::<Vec<_>>(), ["And tomorrow?"]);
}

#[tokio::test]
async fn a_reply_to_a_message_from_the_phone_notifies() {
    let (_dir, _factory, mut client, mut view) = setup(v1::DeviceKind::Phone).await;
    let mut notes = Vec::new();
    run(&mut client, &mut view, &mut notes, "builder", "Is the build green?").await;
    let [reply] = notes.as_slice() else { panic!("{notes:?}") };
    assert_eq!(reply.kind(), NotifyKind::Reply);
    assert_eq!((reply.title.as_str(), reply.body.as_str()), ("Builder", "echo: Is the build green?"));
    assert_eq!(reply.collapse, "agent:builder:reply");
    let answer = view.entries.iter().rev().find(|entry| matches!(entry.kind, Some(Kind::Assistant(_)))).expect("its answer");
    assert_eq!(reply.seq, answer.seq);
}

#[tokio::test]
async fn the_settings_and_each_agents_choice_decide() {
    let (dir, _factory, mut client, mut view) = setup(v1::DeviceKind::Desktop).await;
    let mut notes = Vec::new();
    // On unless changed, and written only once changed.
    let shown = view.workspace.settings.clone().expect("settings").notifications.expect("notifications");
    assert!(shown.needs_you && shown.alerts && shown.replies && !shown.desktop_replies && !shown.quiet_hours);
    assert!(!std::fs::read_to_string(crate::common::home(&dir).config_path()).expect("config").contains("notifications"));

    apply_request(&mut client, &mut view, settings(|settings| settings.alerts = false)).await;
    let said = call(&mut client, &mut view, &mut notes, "builder", "notify_person", alert("Down")).await;
    assert_eq!(said, "Not sent as a notification: the person turned agents' alerts off. It waits in their Needs you until they read it.");
    let notify = |mode| {
        client_message::Body::UpdateAgent(v1::UpdateAgent { agent_id: "builder".into(), notify: Some(mode as i32), ..Default::default() })
    };
    apply_request(&mut client, &mut view, notify(v1::NotifyMode::Always)).await;
    assert_eq!(view.agent("builder").notify(), v1::NotifyMode::Always);
    let said = call(&mut client, &mut view, &mut notes, "builder", "notify_person", alert("Down")).await;
    assert_eq!(said, "Sent: the person gets it as a notification, and it waits in their Needs you until they read it.");
    // Kept in factory.db: a backend rolled back to would refuse a
    // config.json with keys it doesn't know.
    // Agents' prompts aside, which may name notify_person (Rivet's does).
    let mut config: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(crate::common::home(&dir).config_path()).expect("config")).expect("json");
    for agent in config["agents"].as_array_mut().into_iter().flatten() {
        agent.as_object_mut().expect("an agent").remove("prompt");
    }
    let config = config.to_string();
    assert!(!config.contains("notif") && !config.contains("always"), "{config}");

    apply_request(&mut client, &mut view, notify(v1::NotifyMode::Never)).await;
    notes.clear();
    call(&mut client, &mut view, &mut notes, "builder", "ask_person", ask("Deploy?")).await;
    let said = call(&mut client, &mut view, &mut notes, "builder", "notify_person", alert("Down")).await;
    assert_eq!(said, "Not sent as a notification: the person turned notifications from you off. It waits in their Needs you until they read it.");
    assert!(notes.is_empty(), "{notes:?}");

    // Only who.
    apply_request(&mut client, &mut view, notify(v1::NotifyMode::Default)).await;
    apply_request(&mut client, &mut view, settings(|settings| settings.preview = v1::NotifyPreview::Who.into())).await;
    call(&mut client, &mut view, &mut notes, "builder", "ask_person", ask("Deploy again?")).await;
    assert_eq!(notes.iter().map(|note| note.body.as_str()).collect::<Vec<_>>(), ["Builder needs you"]);

    let refused = apply_request(&mut client, &mut view, settings(|settings| settings.relay = "http://push.example.com".into())).await;
    assert!(matches!(&refused, Body::Error(error) if error.message.contains("relay")), "{refused:?}");

    // They last.
    apply_request(&mut client, &mut view, notify(v1::NotifyMode::Never)).await;
    drop(client);
    drop(_factory);
    let (addr, _factory) = start(&dir).await;
    let (_client, workspace, _) = Client::connect(addr).await;
    let shown = workspace.settings.clone().expect("settings").notifications.expect("notifications");
    assert!(shown.preview() == v1::NotifyPreview::Who && shown.needs_you, "{shown:?}");
    assert_eq!(workspace.agents.iter().find(|agent| agent.id == "builder").expect("builder").notify(), v1::NotifyMode::Never);
}

fn change(change: impl FnOnce(&mut v1::NotificationSettingsChange)) -> client_message::Body {
    let mut fields = v1::NotificationSettingsChange::default();
    change(&mut fields);
    client_message::Body::UpdateSettings(v1::UpdateSettings { notification_change: Some(fields), ..Default::default() })
}

fn shown(workspace: &v1::Workspace) -> v1::NotificationSettings {
    workspace.settings.clone().expect("settings").notifications.expect("notifications")
}

/// What the app sends for one switch changes only that one (B4), so two
/// devices changing different ones at once both stay.
#[tokio::test]
async fn a_change_sets_only_its_own_settings() {
    let dir = temp_dir();
    let (addr, _factory) = start(&dir).await;
    let (mut client, workspace, _) = Client::connect(addr).await;
    let mut view = View { workspace, ..View::default() };
    let before = shown(&view.workspace);
    // Quiet hours' time zone: the computer's, whatever the settings.
    assert_eq!(before.time_zone, jiff::tz::TimeZone::system().iana_name().unwrap_or_default());
    assert_eq!(before.utc_offset_minutes, jiff::ZonedDateTime::now().offset().seconds() / 60);

    apply_request(&mut client, &mut view, change(|change| change.alerts = Some(false))).await;
    assert_eq!(shown(&view.workspace), v1::NotificationSettings { alerts: false, ..before.clone() });

    apply_request(
        &mut client,
        &mut view,
        change(|change| {
            change.preview = Some(v1::NotifyPreview::Who.into());
            change.quiet_hours = Some(true);
            change.quiet_start = Some(22 * 60);
            change.quiet_end = Some(7 * 60);
        }),
    )
    .await;
    let expected = v1::NotificationSettings {
        alerts: false,
        preview: v1::NotifyPreview::Who.into(),
        quiet_hours: true,
        quiet_start: 22 * 60,
        quiet_end: 7 * 60,
        ..before.clone()
    };
    assert_eq!(shown(&view.workspace), expected);

    // Checked as a whole message is: refused, nothing changes.
    let refused = apply_request(&mut client, &mut view, change(|change| change.quiet_end = Some(24 * 60))).await;
    assert!(matches!(&refused, Body::Error(error) if error.message.contains("quiet_end")), "{refused:?}");
    let refused = apply_request(
        &mut client,
        &mut view,
        change(|change| {
            change.needs_you = Some(false);
            change.relay = Some("http://push.example.com".into());
        }),
    )
    .await;
    assert!(matches!(&refused, Body::Error(error) if error.message.contains("relay")), "{refused:?}");
    assert_eq!(shown(&view.workspace), expected);

    // Two devices at once, each with its own switch.
    let (mut other, _, _) = Client::connect(addr).await;
    let (first, second) = tokio::join!(
        client.request(change(|change| change.replies = Some(false))),
        other.request(change(|change| change.desktop_replies = Some(true)))
    );
    assert!(matches!(first.body, Body::Ack(_)) && matches!(second.body, Body::Ack(_)), "{:?} {:?}", first.body, second.body);
    let (_client, workspace, _) = Client::connect(addr).await;
    assert_eq!(shown(&workspace), v1::NotificationSettings { replies: false, desktop_replies: true, ..expected.clone() });

    // The whole message still replaces them all (older apps).
    let mut view = View { workspace, ..View::default() };
    apply_request(&mut client, &mut view, settings(|_| {})).await;
    let replaced = shown(&view.workspace);
    assert!(replaced.alerts && replaced.replies && !replaced.quiet_hours && replaced.preview() == v1::NotifyPreview::Full, "{replaced:?}");
}

#[tokio::test]
async fn an_agent_notifies_six_times_an_hour() {
    let (_dir, _factory, mut client, mut view) = setup(v1::DeviceKind::Desktop).await;
    let mut notes = Vec::new();
    for number in 1..=6 {
        let said = call(&mut client, &mut view, &mut notes, "builder", "notify_person", alert(&format!("Alert {number}"))).await;
        assert!(said.starts_with("Sent"), "{said}");
    }
    let said = call(&mut client, &mut view, &mut notes, "builder", "notify_person", alert("Alert 7")).await;
    assert!(said.starts_with("Held: you've already notified the person 6 times this hour."), "{said}");
    assert_eq!(notes.len(), 6);
}

#[tokio::test]
async fn an_agent_that_read_a_web_page_never_puts_its_words_in_a_notification() {
    let (_dir, factory, mut client, mut view, pipe) = crate::pipes::setup(false).await;
    let mut notes = Vec::new();
    // Reading a pipe marks no one (#346); a web page does.
    factoryd::pipes::receive_for_test(&factory, &pipe, incoming("m1", MARKER)).expect("received");
    call(&mut client, &mut view, &mut notes, "bo", "pipe_read", serde_json::json!({})).await;
    assert!(view.agent("bo").tainted_by_pipes.is_empty());
    factoryd::pipes::taint_for_test(&factory, "bo", "browser:https://example.com");
    view.follow(&mut client, |view| !view.agent("bo").tainted_by_pipes.is_empty()).await;
    notes.clear();
    call(&mut client, &mut view, &mut notes, "bo", "ask_person", ask(MARKER)).await;
    call(&mut client, &mut view, &mut notes, "bo", "notify_person", alert(MARKER)).await;
    let mut urgent = alert(MARKER);
    urgent["urgent"] = true.into();
    call(&mut client, &mut view, &mut notes, "bo", "notify_person", urgent).await;
    assert_eq!(notes.iter().map(v1::Notify::kind).collect::<Vec<_>>(), [NotifyKind::NeedsYou, NotifyKind::Alert, NotifyKind::Alert]);
    for note in &notes {
        assert_eq!((note.title.as_str(), note.body.as_str()), ("Bo", "Bo has something for you"));
        assert!(!format!("{note:?}").contains("MARKER"), "{note:?}");
    }
    // Nor in Needs you, nor in History once read: only who.
    let withheld: Vec<v1::Alert> = view.workspace.alerts.clone();
    assert_eq!(withheld.len(), 2);
    for alert in &withheld {
        assert!(alert.withheld && alert.title.is_empty() && alert.text.is_empty() && alert.from_name == "Bo", "{alert:?}");
    }
    let ids: Vec<&str> = withheld.iter().map(|alert| alert.id.as_str()).collect();
    apply_request(&mut client, &mut view, read_alerts(&ids)).await;
    let history = client.request(client_message::Body::ListNeedsYouHistory(v1::ListNeedsYouHistory::default())).await;
    assert!(!format!("{:?}", history.body).contains("MARKER"), "{:?}", history.body);

    // A message from the other side held for review says which pipe, by
    // the name the person gave it; never what it says, nor the other
    // Claktory's name.
    let review = v1::UpdatePipe { pipe_id: pipe.clone(), review_incoming: Some(true), ..Default::default() };
    apply_request(&mut client, &mut view, client_message::Body::UpdatePipe(review)).await;
    let notified = |body: &Body| if let Body::Notify(notify) = body { Some(notify.clone()) } else { None };
    factoryd::pipes::receive_for_test(&factory, &pipe, incoming("m2", MARKER)).expect("received");
    let held = client.wait_for(notified).await;
    assert_eq!(held.kind(), NotifyKind::NeedsYou);
    assert_eq!(
        (held.title.as_str(), held.body.as_str()),
        ("Website ⇄ Marketing", "A message in Website ⇄ Marketing waits for your review")
    );
    assert_eq!(held.pipe_id, pipe);
    assert!(!format!("{held:?}").contains("MARKER") && !format!("{held:?}").contains("Ana"), "{held:?}");

    // A pipe the other side named: its name is theirs to choose, so it
    // isn't shown either.
    factoryd::notify::pipe_named_by_peer_for_test(&factory, &pipe);
    factoryd::pipes::receive_for_test(&factory, &pipe, incoming("m3", MARKER)).expect("received");
    let held = client.wait_for(notified).await;
    assert_eq!((held.title.as_str(), held.body.as_str()), ("A pipe", "A message in a pipe waits for your review"));
    assert!(!format!("{held:?}").contains("MARKER") && !format!("{held:?}").contains("Website"), "{held:?}");
}

/// A push relay on this computer: what it was sent, and the status it
/// answers with.
struct Relay {
    url: String,
    pushes: Arc<Mutex<Vec<serde_json::Value>>>,
    status: Arc<Mutex<u16>>,
}

async fn relay() -> Relay {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let url = format!("http://127.0.0.1:{}", listener.local_addr().expect("addr").port());
    let pushes = Arc::new(Mutex::new(Vec::new()));
    let status = Arc::new(Mutex::new(200));
    let (seen, answer) = (pushes.clone(), status.clone());
    tokio::spawn(async move {
        loop {
            let Ok((mut stream, _)) = listener.accept().await else { return };
            let mut request = Vec::new();
            let mut buffer = [0u8; 4096];
            let body = loop {
                let read = stream.read(&mut buffer).await.expect("read");
                request.extend_from_slice(&buffer[..read]);
                let text = String::from_utf8_lossy(&request).to_string();
                if let Some((head, body)) = text.split_once("\r\n\r\n") {
                    let length: usize = head
                        .lines()
                        .find_map(|line| line.to_ascii_lowercase().strip_prefix("content-length:").map(|value| value.trim().to_owned()))
                        .and_then(|value| value.parse().ok())
                        .unwrap_or(0);
                    if body.len() >= length {
                        assert!(head.starts_with("POST /v1/push "), "{head}");
                        break body.to_owned();
                    }
                }
                if read == 0 {
                    return;
                }
            };
            seen.lock().expect("lock").push(serde_json::from_str(&body).expect("JSON"));
            let code = *answer.lock().expect("lock");
            let response =
                format!("HTTP/1.1 {code} X\r\ncontent-length: 2\r\ncontent-type: application/json\r\nconnection: close\r\n\r\n{{}}");
            stream.write_all(response.as_bytes()).await.expect("write");
        }
    });
    Relay { url, pushes, status }
}

impl Relay {
    async fn next(&self, count: usize) -> serde_json::Value {
        for _ in 0..500 {
            if let Some(push) = self.pushes.lock().expect("lock").get(count - 1) {
                return push.clone();
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        panic!("no push {count}");
    }
}

/// The plaintext of a push sealed under `key`.
fn open(key: &[u8], push: &serde_json::Value) -> (usize, serde_json::Value) {
    use aws_lc_rs::aead::{AES_256_GCM, Aad, LessSafeKey, Nonce, UnboundKey};
    let bytes = STANDARD.decode(push["ciphertext"].as_str().expect("ciphertext")).expect("base64");
    let (nonce, sealed) = bytes.split_at(12);
    let cipher = LessSafeKey::new(UnboundKey::new(&AES_256_GCM, key).expect("key"));
    let mut buffer = sealed.to_vec();
    let plain = cipher.open_in_place(Nonce::try_assume_unique_for_key(nonce).expect("nonce"), Aad::empty(), &mut buffer).expect("opens");
    (plain.len(), serde_json::from_slice(plain).expect("JSON"))
}

#[tokio::test]
async fn a_registered_device_gets_pushes_sealed_with_its_key_while_it_isnt_in_use() {
    let relay = relay().await;
    let (_dir, factory, mut client, mut view) = setup(v1::DeviceKind::Desktop).await;
    apply_request(&mut client, &mut view, settings(|settings| settings.relay = relay.url.clone())).await;
    let key = [42u8; 32];
    let handle = URL_SAFE_NO_PAD.encode([9u8; 32]);
    let register = |handle: &str, key: &[u8]| {
        client_message::Body::RegisterNotifications(v1::RegisterNotifications {
            handle: handle.into(),
            key: key.to_vec(),
            platform: v1::PushPlatform::Apns.into(),
            ..Default::default()
        })
    };
    let wrong = apply_request(&mut client, &mut view, register(&handle, &key[..16])).await;
    assert!(matches!(wrong, Body::Error(_)), "{wrong:?}");
    assert!(matches!(apply_request(&mut client, &mut view, register("not a handle", &key)).await, Body::Error(_)));
    assert!(matches!(apply_request(&mut client, &mut view, register(&handle, &key)).await, Body::Ack(_)));
    assert!(factoryd::notify::registered_for_test(&factory, "local"));

    // A test goes to it though it's connected.
    client.ok(client_message::Body::SendTestNotification(v1::SendTestNotification {})).await;
    let test = relay.next(1).await;
    assert_eq!(test["handle"], handle.as_str());
    let (size, payload) = open(&key, &test);
    assert_eq!(size, 512);
    assert_eq!((payload["kind"].as_str(), payload["ctr"].as_u64()), (Some("test"), Some(1)));

    // In use, it isn't pushed to: the app shows it.
    client.ok(set_active(true)).await;
    let mut notes = Vec::new();
    call(&mut client, &mut view, &mut notes, "builder", "notify_person", alert("Down")).await;
    assert_eq!(notes.len(), 1);
    assert_eq!(relay.pushes.lock().expect("lock").len(), 1, "nothing pushed while it was in use");

    // Connected but in the background (a phone whose app iOS suspended,
    // its tunnel not yet timed out): pushed, with the Notify's id.
    client.ok(set_active(false)).await;
    notes.clear();
    call(&mut client, &mut view, &mut notes, "builder", "notify_person", alert("Away")).await;
    let away = relay.next(2).await;
    let (_, payload) = open(&key, &away);
    assert_eq!(payload["ctr"], 2);
    assert_eq!(payload["id"], notes[0].id.as_str());

    // The phone sends the same registration on every connect: the counter
    // goes on.
    assert!(matches!(apply_request(&mut client, &mut view, register(&handle, &key)).await, Body::Ack(_)));
    drop(client);
    for _ in 0..200 {
        if factoryd::notify::connections_for_test(&factory) == 0 {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }

    let args = serde_json::json!({"title": "account.claktory.com isn't answering", "text": MARKER, "urgent": true});
    factory.send_message("builder", &format!("call notify_person {args}"), None, "", &[]).expect("sent");
    let push = relay.next(3).await;
    let (_, payload) = open(&key, &push);
    assert_eq!(payload["ctr"], 3);
    assert_eq!(payload["kind"], "alert");
    assert_eq!(payload["title"], "Builder");
    assert_eq!(payload["body"], format!("account.claktory.com isn't answering\n{MARKER}"));
    assert_eq!(payload["target"]["agent"], "builder");
    // Urgent only when the person lets urgent alerts through.
    assert_eq!(push["urgency"], "normal");
    assert_eq!(push["ttl"], 86400);
    assert_eq!(push["collapse"].as_str().map(str::len), Some(22));
    assert!(!push.to_string().contains("builder") && !push.to_string().contains("MARKER"), "{push}");

    // The relay no longer knows the handle: it's forgotten.
    *relay.status.lock().expect("lock") = 410;
    factory.send_message("builder", &format!("call notify_person {}", alert("Again")), None, "", &[]).expect("sent");
    relay.next(4).await;
    for _ in 0..200 {
        if !factoryd::notify::registered_for_test(&factory, "local") {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    panic!("the handle wasn't forgotten");
}

/// Area 5a item 13: a device whose notifications are off counts as reaching
/// no one, gets no pushes, and notify_person says where they're off; once
/// it says they're allowed, both work again.
#[tokio::test]
async fn a_device_with_notifications_off_isnt_counted_or_pushed_to() {
    let relay = relay().await;
    let (_dir, factory, mut client, mut view) = setup(v1::DeviceKind::Desktop).await;
    apply_request(&mut client, &mut view, settings(|settings| settings.relay = relay.url.clone())).await;
    let key = [5u8; 32];
    let register = client_message::Body::RegisterNotifications(v1::RegisterNotifications {
        handle: URL_SAFE_NO_PAD.encode([6u8; 32]),
        key: key.to_vec(),
        platform: v1::PushPlatform::Apns.into(),
        permission: v1::NotificationPermission::Denied.into(),
    });
    assert!(matches!(apply_request(&mut client, &mut view, register).await, Body::Ack(_)));
    let said_off = |off: v1::NotificationPermission| {
        client_message::Body::SetActive(v1::SetActive { active: false, notifications: off.into() })
    };
    client.ok(said_off(v1::NotificationPermission::Denied)).await;

    let mut notes = Vec::new();
    let said = call(&mut client, &mut view, &mut notes, "builder", "notify_person", alert("Down")).await;
    assert_eq!(
        said,
        "Not delivered as a notification: notifications are turned off on this computer, and no other device of the person's shows them. It waits in their Needs you until they read it."
    );
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    assert!(relay.pushes.lock().expect("lock").is_empty(), "nothing pushed to a device that wouldn't show it");

    // Allowed in its Settings: shown by the app, and pushed while it's away.
    client.ok(said_off(v1::NotificationPermission::Allowed)).await;
    notes.clear();
    let said = call(&mut client, &mut view, &mut notes, "builder", "notify_person", alert("Up")).await;
    assert_eq!(said, "Sent: the person gets it as a notification, and it waits in their Needs you until they read it.");
    relay.next(1).await;
    assert!(factoryd::notify::registered_for_test(&factory, "local"));
}

/// Area 5a item 4: the "Get notified…" offer, dismissed on this computer's
/// own app, stays dismissed while its notifications aren't allowed, and is
/// cleared once they are.
#[tokio::test]
async fn the_notifications_offer_dismissed_on_a_device_is_cleared_once_they_are_allowed() {
    let (dir, _factory, mut client, mut view) = setup(v1::DeviceKind::Desktop).await;
    let dismissed = |view: &View| view.workspace.remote.as_ref().is_some_and(|remote| remote.local_notifications_offer_dismissed);
    assert!(!dismissed(&view));
    let dismiss = client_message::Body::DismissNotificationsOffer(v1::DismissNotificationsOffer {});
    assert!(matches!(apply_request(&mut client, &mut view, dismiss.clone()).await, Body::Ack(_)));
    view.follow(&mut client, dismissed).await;
    assert!(matches!(apply_request(&mut client, &mut view, dismiss).await, Body::Ack(_)), "again is no error");

    let said = |permission: v1::NotificationPermission| {
        client_message::Body::SetActive(v1::SetActive { active: true, notifications: permission.into() })
    };
    for still in [v1::NotificationPermission::Denied, v1::NotificationPermission::NotAsked, v1::NotificationPermission::Unspecified] {
        client.ok(said(still)).await;
    }
    let kept = std::fs::read_to_string(dir.join("home/remote/devices.json")).expect("devices.json");
    assert!(kept.contains("notifications_offer_dismissed"), "kept while they aren't allowed: {kept}");
    assert!(matches!(apply_request(&mut client, &mut view, said(v1::NotificationPermission::Allowed)).await, Body::Ack(_)));
    view.follow(&mut client, |view| !dismissed(view)).await;
}

/// Friend bug #36: when the disk turns read-only, the person hears it on
/// every device, from what was saved while it worked: a push to each
/// registered device, with a counter saved ahead (none used twice, after a
/// restart or by an older version), and the connected apps.
// A folder made read-only by its mode; on Windows a DACL would be needed.
#[cfg(unix)]
#[tokio::test]
async fn a_disk_that_cant_be_written_is_told_to_every_device_without_saving_anything() {
    use std::os::unix::fs::PermissionsExt;
    let relay = relay().await;
    let (dir, factory, mut client, mut view) = setup(v1::DeviceKind::Desktop).await;
    apply_request(&mut client, &mut view, settings(|settings| settings.relay = relay.url.clone())).await;
    let key = [7u8; 32];
    let register = client_message::Body::RegisterNotifications(v1::RegisterNotifications {
        handle: URL_SAFE_NO_PAD.encode([3u8; 32]),
        key: key.to_vec(),
        platform: v1::PushPlatform::Apns.into(),
        ..Default::default()
    });
    assert!(matches!(apply_request(&mut client, &mut view, register).await, Body::Ack(_)));
    // Counters are saved ahead from the start.
    let kept = || -> serde_json::Value {
        serde_json::from_slice(&std::fs::read(dir.join("home/remote/notifications.json")).expect("notifications.json")).expect("JSON")
    };
    assert_eq!(kept()["local"]["ctr"], 64, "{}", kept());

    // Nothing can be written any more.
    let remote = dir.join("home/remote");
    std::fs::set_permissions(&remote, std::fs::Permissions::from_mode(0o500)).expect("read-only");
    let pushed = factoryd::disk::tell(&factory, factoryd::disk::Fault::ReadOnly);
    std::fs::set_permissions(&remote, std::fs::Permissions::from_mode(0o700)).expect("writable");
    assert_eq!(pushed, 1);
    let push = relay.next(1).await;
    let (_, payload) = open(&key, &push);
    assert_eq!((payload["kind"].as_str(), payload["ctr"].as_u64()), (Some("alert"), Some(1)));
    assert!(payload["title"].as_str().is_some_and(|title| title.ends_with("can't save anything")), "{payload}");
    assert!(payload["body"].as_str().is_some_and(|body| body.contains("read-only")), "{payload}");
    assert_eq!(push["urgency"], "urgent");
    let told = client
        .wait_for(|body| {
            view.apply(body);
            match body {
                Body::Notify(notify) if notify.collapse == "disk" => Some(notify.clone()),
                _ => None,
            }
        })
        .await;
    assert!(told.urgent && told.kind() == NotifyKind::Alert, "{told:?}");
    assert_eq!(kept()["local"]["ctr"], 64, "nothing saved");
    // Kept in Needs you, which the notification and the push open.
    assert!(!told.alert_id.is_empty());
    assert_eq!(payload["target"]["alert"].as_str(), Some(told.alert_id.as_str()), "{payload}");
    view.follow(&mut client, |view| view.workspace.alerts.iter().any(|alert| alert.id == told.alert_id)).await;
    let alert = view.workspace.alerts.iter().find(|alert| alert.id == told.alert_id).expect("kept");
    assert!(alert.agent_id.is_empty() && alert.urgent && alert.title.ends_with("can't save anything"), "{alert:?}");

    // The next push, as after a restart, starts past what was saved ahead.
    client.ok(client_message::Body::SendTestNotification(v1::SendTestNotification {})).await;
    let (_, payload) = open(&key, &relay.next(2).await);
    assert_eq!(payload["ctr"], 2, "on from memory");
    factoryd::notify::forget_registrations_for_test(&factory);
    client.ok(client_message::Body::SendTestNotification(v1::SendTestNotification {})).await;
    let (_, payload) = open(&key, &relay.next(3).await);
    assert_eq!(payload["ctr"], 65, "read again: past every counter saved ahead");
    assert_eq!(kept()["local"]["ctr"], 128);
}

fn read_alerts(ids: &[&str]) -> client_message::Body {
    client_message::Body::ReadAlerts(v1::ReadAlerts { ids: ids.iter().map(|id| (*id).to_owned()).collect() })
}

/// Bruno 44235: every alert waits in Needs you until the person reads it,
/// then is in History with when and on which device.
#[tokio::test]
async fn an_alert_waits_in_needs_you_until_the_person_reads_it() {
    let (dir, factory, mut client, mut view) = setup(v1::DeviceKind::Desktop).await;
    let mut notes = Vec::new();
    // Builder leads Helper, so its urgent alerts go as urgent (Bruno 52699).
    apply_request(&mut client, &mut view, create_agent("site", "Helper", "")).await;
    let lead = v1::UpdateAgent { agent_id: "helper".into(), lead_id: Some("builder".into()), ..Default::default() };
    apply_request(&mut client, &mut view, client_message::Body::UpdateAgent(lead)).await;
    let mut urgent = alert("The site is down");
    urgent["urgent"] = true.into();
    let said = call(&mut client, &mut view, &mut notes, "builder", "notify_person", urgent).await;
    assert_eq!(said, "Sent: the person gets it as a notification, and it waits in their Needs you until they read it.");
    let call_seq = view
        .entries
        .iter()
        .rev()
        .find_map(|entry| match &entry.kind {
            Some(Kind::ToolCall(call)) if call.name == "notify_person" => Some(entry.seq),
            _ => None,
        })
        .expect("the call");
    let [kept] = view.workspace.alerts.as_slice() else { panic!("{:?}", view.workspace.alerts) };
    assert_eq!((kept.agent_id.as_str(), kept.from_name.as_str()), ("builder", "Builder"));
    assert_eq!((kept.title.as_str(), kept.text.as_str()), ("The site is down", "500s since 10:02."));
    assert!(kept.urgent && !kept.withheld && kept.sent_ms > 0, "{kept:?}");
    assert_eq!(kept.seq, call_seq);
    assert_eq!(view.workspace.alerts_unread, 1);
    // Its notification opens it.
    let [sent] = notes.as_slice() else { panic!("{notes:?}") };
    assert_eq!((sent.alert_id.as_str(), sent.seq), (kept.id.as_str(), call_seq));
    let kept = kept.clone();

    // Kept across a restart.
    drop(client);
    drop(factory);
    let (addr, _factory) = start(&dir).await;
    let (mut client, workspace, _) = Client::connect(addr).await;
    let mut view = View { workspace, ..View::default() };
    assert_eq!(view.workspace.alerts, std::slice::from_ref(&kept));

    // Read: gone from Needs you, in History with when and where.
    assert!(matches!(apply_request(&mut client, &mut view, read_alerts(&["unknown", &kept.id])).await, Body::Ack(_)));
    assert!(view.workspace.alerts.is_empty() && view.workspace.alerts_unread == 0, "{:?}", view.workspace.alerts);
    let history = client.request(client_message::Body::ListNeedsYouHistory(v1::ListNeedsYouHistory::default())).await;
    let Body::NeedsYouHistory(history) = history.body else { panic!("{:?}", history.body) };
    let record = history.records.iter().find(|record| record.kind() == v1::NeedsYouRecordKind::Alert).expect("the record");
    assert_eq!(record.outcome(), v1::NeedsYouOutcome::Read);
    assert_eq!((record.agent_id.as_str(), record.agent_name.as_str(), record.title.as_str()), ("builder", "Builder", "The site is down"));
    assert_eq!(record.alert.as_ref(), Some(&kept));
    assert_eq!((record.asked_ms, record.seq), (kept.sent_ms, call_seq));
    assert!(record.decided_ms >= kept.sent_ms);
    let on = record.answered_on.as_ref().expect("where it was read");
    assert_eq!((on.device_id.as_str(), on.at_ms), ("local", record.decided_ms));
    assert!(!on.device_name.is_empty());
    // A search finds it by its text.
    let search = v1::ListNeedsYouHistory { query: "500s since".into(), ..Default::default() };
    let history = client.request(client_message::Body::ListNeedsYouHistory(search)).await;
    let Body::NeedsYouHistory(history) = history.body else { panic!("{:?}", history.body) };
    assert_eq!(history.records.len(), 1);

    // Reading it again changes nothing.
    assert!(matches!(apply_request(&mut client, &mut view, read_alerts(&[&kept.id])).await, Body::Ack(_)));
}

/// Nothing is lost: an alert held by the hourly cap, or not sent at all,
/// waits in Needs you all the same; and an alert is no question.
#[tokio::test]
async fn alerts_held_or_not_sent_wait_in_needs_you_too() {
    let (_dir, _factory, mut client, mut view) = setup(v1::DeviceKind::Desktop).await;
    let mut notes = Vec::new();
    for number in 1..=7 {
        call(&mut client, &mut view, &mut notes, "builder", "notify_person", alert(&format!("Alert {number}"))).await;
    }
    assert_eq!(notes.len(), 6, "the cap");
    assert_eq!(view.workspace.alerts.len(), 7);
    assert_eq!(view.workspace.alerts_unread, 7);
    assert_eq!(view.workspace.alerts.last().map(|alert| alert.title.as_str()), Some("Alert 7"));
    assert!(view.agent("builder").waiting_seqs.is_empty(), "not a request");

    apply_request(&mut client, &mut view, settings(|settings| settings.alerts = false)).await;
    let said = call(&mut client, &mut view, &mut notes, "builder", "notify_person", alert("Off")).await;
    assert!(said.ends_with("It waits in their Needs you until they read it."), "{said}");
    assert_eq!(view.workspace.alerts_unread, 8);

    // Read together.
    let ids: Vec<String> = view.workspace.alerts.iter().map(|alert| alert.id.clone()).collect();
    let ids: Vec<&str> = ids.iter().map(String::as_str).collect();
    apply_request(&mut client, &mut view, read_alerts(&ids)).await;
    assert_eq!(view.workspace.alerts_unread, 0);
}

/// The tool's text is a line or so: anything longer is for a review card.
#[tokio::test]
async fn an_alerts_text_is_a_sentence_at_most() {
    let (_dir, _factory, mut client, mut view) = setup(v1::DeviceKind::Desktop).await;
    let mut notes = Vec::new();
    let long = serde_json::json!({"title": "Report", "text": "x".repeat(301)});
    let said = call(&mut client, &mut view, &mut notes, "builder", "notify_person", long).await;
    assert!(said.contains("at most 300 characters") && said.contains("ask_person with review") && said.ends_with("Nothing was sent."), "{said}");
    assert!(notes.is_empty() && view.workspace.alerts.is_empty());
    let most = serde_json::json!({"title": "Report", "text": "x".repeat(300)});
    let said = call(&mut client, &mut view, &mut notes, "builder", "notify_person", most).await;
    assert!(said.starts_with("Sent"), "{said}");
}

/// A member quarantined for a pipe sends no alert at all, so nothing it
/// read reaches a notification, Needs you or History (Fugo 45952).
#[tokio::test]
async fn a_quarantined_member_sends_no_alert() {
    let (_dir, _factory, mut client, mut view, pipe) = crate::pipes::setup(false).await;
    let mut notes = Vec::new();
    let quarantine = v1::UpdatePipe { pipe_id: pipe, quarantined: Some(v1::PipeMembers { agent_ids: vec!["bo".into()] }), ..Default::default() };
    apply_request(&mut client, &mut view, client_message::Body::UpdatePipe(quarantine)).await;
    view.follow(&mut client, |view| view.agent("bo").quarantined).await;
    let said = call(&mut client, &mut view, &mut notes, "bo", "notify_person", alert(MARKER)).await;
    assert!(!said.starts_with("Sent") && !said.starts_with("Not"), "{said}");
    assert!(notes.is_empty() && view.workspace.alerts.is_empty(), "{notes:?}");
}
