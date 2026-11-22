//! Message provenance and quiet copies (#285, middleman.md H1 and H3): a
//! message says its number and what it answers; a quiet copy reaches the
//! sender's lead without waking it, and hands on the sender's taint as it
//! was when it was sent.

use rusqlite::Connection;

use crate::common::{Client, Scripted, View, apply_request, create_agent, create_project, send_message, start_with, temp_dir};
use crate::pipes::{call, idle, select};
use factory_protocol::v1::{self, client_message, entry::Kind, server_message::Body};
use factoryd::pipes;

type Requests = std::sync::Arc<std::sync::Mutex<Vec<crate::common::Request>>>;

/// Site with Lea, Bo (who reports to Lea) and Max.
async fn setup() -> (std::path::PathBuf, std::sync::Arc<factoryd::Factory>, Client, View, Requests) {
    let dir = temp_dir();
    let models = Scripted::signed_in();
    let requests = models.requests.clone();
    let (addr, factory) = start_with(&dir, models).await;
    let (mut client, workspace, _) = Client::connect(addr).await;
    let mut view = View { workspace, ..View::default() };
    apply_request(&mut client, &mut view, create_project("Site", &dir.join("site"))).await;
    for name in ["Lea", "Bo", "Max"] {
        apply_request(&mut client, &mut view, create_agent("site", name, "")).await;
    }
    let lead = v1::UpdateAgent { agent_id: "bo".into(), lead_id: Some("lea".into()), ..Default::default() };
    apply_request(&mut client, &mut view, client_message::Body::UpdateAgent(lead)).await;
    (dir, factory, client, view, requests)
}

fn db(dir: &std::path::Path) -> Connection {
    Connection::open(dir.join("home/factory.db")).expect("factory.db")
}

/// The agent messages `agent` received, oldest first: id, sender, send_id,
/// sender_run, reply_to, intent, taint.
#[allow(clippy::type_complexity)]
fn received(dir: &std::path::Path, agent: &str) -> Vec<(i64, String, String, Option<i64>, String, String, String)> {
    let db = db(dir);
    let mut statement = db
        .prepare("SELECT id, sender_id, send_id, sender_run, reply_to, intent, taint FROM messages WHERE agent = ?1 AND source = 'agent' ORDER BY id")
        .expect("select");
    statement
        .query_map([agent], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?, row.get(5)?, row.get(6)?)))
        .expect("rows")
        .collect::<Result<_, _>>()
        .expect("rows")
}

async fn settle(client: &mut Client, view: &mut View, agents: &[&str]) {
    view.follow(client, |view| agents.iter().all(|agent| idle(view, agent))).await;
}

#[tokio::test]
async fn a_message_says_its_number_and_a_reply_what_it_answers() {
    let (dir, _factory, mut client, mut view, requests) = setup().await;
    let sent = call(&mut client, &mut view, "bo", "agents_send", r#"{"to": "max", "text": "Ready for review.", "intent": "handoff"}"#).await;
    assert_eq!(sent, "Sent.");
    settle(&mut client, &mut view, &["max"]).await;
    let rows = received(&dir, "max");
    let (number, sender, send_id, run, reply_to, intent, taint) = rows.last().cloned().expect("a row");
    assert_eq!((sender.as_str(), send_id.len(), reply_to.as_str(), intent.as_str(), taint.as_str()), ("bo", 32, "[]", "handoff", "{}"));
    assert!(run.is_some(), "the sender's run is recorded");
    // Max read its number on the event.
    let read = requests.lock().expect("lock").iter().any(|request| {
        request.prompt.contains("Ready for review.") && request.prompt.contains(&format!("from=\"Bo\" id=\"bo\" msg=\"{number}\""))
    });
    assert!(read, "the event tag has msg=\"{number}\"");

    // Max answers it; the answer says so, to Bo and to the app.
    let args = format!(r#"{{"to": "bo", "text": "Looks good.", "reply_to": [{number}], "intent": "decision"}}"#);
    assert_eq!(call(&mut client, &mut view, "max", "agents_send", &args).await, "Sent.");
    settle(&mut client, &mut view, &["bo"]).await;
    let (answer, _, _, _, reply_to, intent, _) = received(&dir, "bo").last().cloned().expect("a row");
    assert_eq!((reply_to, intent.as_str()), (format!("[{number}]"), "decision"));
    apply_request(&mut client, &mut view, select("bo")).await;
    let shown = view
        .entries
        .iter()
        .find_map(|entry| match &entry.kind {
            Some(Kind::Event(event)) if event.text == "Looks good." => Some(event.clone()),
            _ => None,
        })
        .expect("the event");
    assert_eq!((shown.message_id, shown.reply_to_ids.as_slice(), shown.intent.as_str()), (answer as u64, [number as u64].as_slice(), "decision"));

    // Only messages an agent sent the sender: not another's, not a made-up one.
    let before = received(&dir, "bo").len();
    for wrong in [answer + 1000, answer] {
        let args = format!(r#"{{"to": "bo", "text": "Again.", "reply_to": [{wrong}]}}"#);
        let refused = call(&mut client, &mut view, "max", "agents_send", &args).await;
        assert!(refused.contains(&format!("{wrong} isn't a message an agent sent you")), "{refused}");
    }
    assert_eq!(received(&dir, "bo").len(), before, "nothing was sent");

    // A fanout is one send.
    call(&mut client, &mut view, "bo", "agents_send", r#"{"to": ["max", "lea"], "text": "Both of you."}"#).await;
    settle(&mut client, &mut view, &["max", "lea"]).await;
    let to_max = received(&dir, "max").last().cloned().expect("max").2;
    let to_lea = received(&dir, "lea").last().cloned().expect("lea").2;
    assert_eq!(to_max, to_lea);
}

#[tokio::test]
async fn a_quiet_copy_reaches_the_lead_without_waking_it() {
    let (dir, _factory, mut client, mut view, _) = setup().await;
    // Lea has a conversation, so it can be told before its next step.
    apply_request(&mut client, &mut view, send_message("lea", "hello")).await;
    settle(&mut client, &mut view, &["lea"]).await;
    let runs = |dir: &std::path::Path| -> i64 {
        db(dir).query_row("SELECT COUNT(*) FROM runs WHERE agent = 'lea'", [], |row| row.get(0)).expect("runs")
    };
    let before = runs(&dir);

    let args = r#"{"to": "max", "text": "Built; over to you.", "intent": "handoff", "copy_lead": true}"#;
    let sent = call(&mut client, &mut view, "bo", "agents_send", args).await;
    assert_eq!(sent, "Sent.\nCopied quietly to Lea.");
    settle(&mut client, &mut view, &["max"]).await;
    view.follow(&mut client, |view| view.agent("lea").unread_copies == 1).await;
    assert_eq!(runs(&dir), before, "the copy didn't wake Lea");
    assert!(received(&dir, "lea").is_empty(), "a copy is never a message");
    // Nor after a restart: still there, still unread, still asleep.
    let (restarted, _again) = crate::common::start(&dir).await;
    let (_other, workspace, _) = Client::connect(restarted).await;
    let lea = workspace.agents.iter().find(|agent| agent.id == "lea").expect("lea");
    assert_eq!(lea.unread_copies, 1);
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    assert_eq!(runs(&dir), before, "the restart didn't wake Lea");

    // The person sees it.
    let list = v1::ListQuietCopies { agent_id: "lea".into(), ..Default::default() };
    let Body::QuietCopies(listed) = apply_request(&mut client, &mut view, client_message::Body::ListQuietCopies(list)).await else {
        panic!("no copies")
    };
    let copy = listed.copies.first().expect("a copy").clone();
    assert_eq!(
        (copy.from.as_str(), copy.from_id.as_str(), copy.text.as_str(), copy.to_ids.as_slice(), copy.intent.as_str(), copy.read),
        ("Bo", "bo", "Built; over to you.", ["max".to_owned()].as_slice(), "handoff", false)
    );
    assert!(!listed.more);

    // Lea hears of it when it works anyway, once, then reads it.
    apply_request(&mut client, &mut view, send_message("lea", "What's new?")).await;
    settle(&mut client, &mut view, &["lea"]).await;
    apply_request(&mut client, &mut view, select("lea")).await;
    let told: Vec<String> = view
        .entries
        .iter()
        .filter_map(|entry| match &entry.kind {
            Some(Kind::Event(event)) if event.text.contains("new quiet copy") => Some(event.text.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(told.len(), 1, "{told:?}");
    assert!(told[0].contains("1 new quiet copy is available, from Bo (1): agents_copies reads them."), "{told:?}");
    let read = call(&mut client, &mut view, "lea", "agents_copies", "{}").await;
    assert!(read.starts_with(&format!("Copy {} from Bo, to Max", copy.id)) && read.contains("; handoff:\nBuilt; over to you."), "{read}");
    view.follow(&mut client, |view| view.agent("lea").unread_copies == 0).await;
    let again = call(&mut client, &mut view, "lea", "agents_copies", "{}").await;
    assert!(again.starts_with("No unread quiet copies"), "{again}");
    assert!(call(&mut client, &mut view, "lea", "agents_copies", r#"{"all": true}"#).await.contains("Built; over to you."));

    // The lead among the recipients gets the message itself.
    let both = call(&mut client, &mut view, "bo", "agents_send", r#"{"to": ["lea", "max"], "text": "FYI.", "copy_lead": true}"#).await;
    assert!(both.ends_with("Your lead is a recipient, so it got the message itself, not a quiet copy."), "{both}");
    // No lead, no copy, and nothing sent.
    let before = received(&dir, "bo").len();
    let refused = call(&mut client, &mut view, "max", "agents_send", r#"{"to": "bo", "text": "Hi.", "copy_lead": true}"#).await;
    assert!(refused.starts_with("You have no lead"), "{refused}");
    assert_eq!(received(&dir, "bo").len(), before);
}

#[tokio::test]
async fn a_copy_hands_on_the_senders_taint_as_it_was() {
    let (dir, factory, mut client, mut view, _) = setup().await;
    pipes::taint_with_for_test(&factory, "max", "bo", r#"{"p-secret": []}"#);
    let sent = call(&mut client, &mut view, "bo", "agents_send", r#"{"to": "max", "text": "From the pipe.", "copy_lead": true}"#).await;
    assert!(sent.ends_with("Copied quietly to Lea."), "{sent}");
    settle(&mut client, &mut view, &["max"]).await;
    let signature: String = db(&dir).query_row("SELECT sender FROM quiet_copies", [], |row| row.get(0)).expect("a copy");
    assert!(signature.starts_with("Bo") && signature.len() > "Bo".len(), "the taint label is kept: {signature}");
    assert_eq!(pipes::taint_of_for_test(&factory, "lea"), "{}", "not tainted until it reads it");

    // Bo starts over and is renamed: the copy keeps what Bo had read.
    let clear = v1::RunCommand { agent_id: "bo".into(), name: "clear".into(), args: String::new() };
    apply_request(&mut client, &mut view, client_message::Body::RunCommand(clear)).await;
    let rename = v1::UpdateAgent { agent_id: "bo".into(), name: Some("Bob".into()), ..Default::default() };
    apply_request(&mut client, &mut view, client_message::Body::UpdateAgent(rename)).await;
    view.follow(&mut client, |_| pipes::taint_of_for_test(&factory, "bo") == "{}").await;
    call(&mut client, &mut view, "lea", "agents_copies", "{}").await;
    assert!(pipes::taint_of_for_test(&factory, "lea").contains("p-secret"), "{}", pipes::taint_of_for_test(&factory, "lea"));

    // A copy whose taint can't be read isn't given.
    call(&mut client, &mut view, "bo", "agents_send", r#"{"to": "max", "text": "UNCHECKED text", "copy_lead": true}"#).await;
    db(&dir).execute("UPDATE quiet_copies SET taint = 'not json' WHERE text = 'UNCHECKED text'", []).expect("spoiled");
    let read = call(&mut client, &mut view, "lea", "agents_copies", "{}").await;
    assert!(read.contains("couldn't be checked and was left out") && !read.contains("UNCHECKED"), "{read}");
}
