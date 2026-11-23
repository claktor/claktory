//! Quiet messages: agents_send's `quiet` never wakes its recipient; the
//! message waits, across a restart too, for whatever next wakes it, and
//! comes then, marked quiet, with Claktory's line on the batch before it.

use std::time::Duration;

use rusqlite::Connection;

use crate::common::{Client, Scripted, View, apply_request, create_agent, create_project, send_message, start_with, temp_dir};
use crate::pipes::{call, idle, select};
use factory_protocol::v1::{self, entry::Kind};
use factoryd::pipes;

/// Site with Bo and Max; Max has a conversation.
async fn setup() -> (std::path::PathBuf, std::sync::Arc<factoryd::Factory>, Client, View) {
    let dir = temp_dir();
    let (addr, factory) = start_with(&dir, Scripted::signed_in()).await;
    let (mut client, workspace, _) = Client::connect(addr).await;
    let mut view = View { workspace, ..View::default() };
    apply_request(&mut client, &mut view, create_project("Site", &dir.join("site"))).await;
    for name in ["Bo", "Max"] {
        apply_request(&mut client, &mut view, create_agent("site", name, "")).await;
    }
    apply_request(&mut client, &mut view, send_message("max", "hello")).await;
    view.follow(&mut client, |view| idle(view, "max")).await;
    (dir, factory, client, view)
}

fn runs(dir: &std::path::Path, agent: &str) -> i64 {
    Connection::open(dir.join("home/factory.db"))
        .expect("factory.db")
        .query_row("SELECT COUNT(*) FROM runs WHERE agent = ?1", [agent], |row| row.get(0))
        .expect("runs")
}

/// The events in `agent`'s conversation whose text is `text`.
async fn events(client: &mut Client, view: &mut View, agent: &str, text: &str) -> Vec<v1::Event> {
    apply_request(client, view, select(agent)).await;
    view.entries
        .iter()
        .filter_map(|entry| match &entry.kind {
            Some(Kind::Event(event)) if event.text == text => Some(event.clone()),
            _ => None,
        })
        .collect()
}

/// The agent's saved conversation, as written.
fn saved(dir: &std::path::Path, agent: &str) -> String {
    let mut text = String::new();
    for file in std::fs::read_dir(dir.join("home/agents").join(agent)).expect("folder").flatten() {
        if file.file_name().to_string_lossy().starts_with("segment-") {
            text.push_str(&std::fs::read_to_string(file.path()).unwrap_or_default());
        }
    }
    text
}

#[tokio::test]
async fn a_quiet_message_waits_for_the_next_wake_and_comes_with_it() {
    let (dir, _factory, mut client, mut view) = setup().await;
    let before = runs(&dir, "max");

    let sent = call(&mut client, &mut view, "bo", "agents_send", r#"{"to": "max", "text": "FYI: the build is green.", "quiet": true}"#).await;
    assert_eq!(sent, "Sent quietly to Max, who reads it when next woken; this doesn't wake it.");
    view.follow(&mut client, |view| view.agent("max").quiet_waiting == 1).await;
    assert_eq!(view.agent("max").queued_messages, 0, "a quiet message isn't queued work");
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(runs(&dir, "max"), before, "it didn't wake Max");
    // Others see it waiting; the person reads it in Max's queue.
    let status = call(&mut client, &mut view, "bo", "agents_status", r#"{"agent": "max"}"#).await;
    assert!(status.contains("idle") && status.ends_with("; 1 quiet waiting"), "{status}");
    apply_request(&mut client, &mut view, select("max")).await;
    view.follow(&mut client, |view| view.queued.as_ref().is_some_and(|queued| queued.messages.len() == 1)).await;
    let waiting = view.queued.as_ref().expect("queued").messages[0].clone();
    assert!(waiting.quiet && waiting.text == "FYI: the build is green.", "{waiting:?}");

    // A restart doesn't wake it either.
    let (restarted, _again) = crate::common::start(&dir).await;
    let (_other, workspace, _) = Client::connect(restarted).await;
    assert_eq!(workspace.agents.iter().find(|agent| agent.id == "max").expect("max").quiet_waiting, 1);
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(runs(&dir, "max"), before, "the restart didn't wake Max");
    drop(_again);

    // The person's message wakes it: the quiet one comes first, marked, with
    // Claktory's line on it for the model.
    apply_request(&mut client, &mut view, send_message("max", "What's new?")).await;
    view.follow(&mut client, |view| idle(view, "max") && view.agent("max").quiet_waiting == 0).await;
    let shown = events(&mut client, &mut view, "max", "FYI: the build is green.").await;
    assert_eq!(shown.len(), 1);
    assert!(shown[0].quiet && shown[0].from_id == "bo" && shown[0].message_id != 0, "{:?}", shown[0]);
    let conversation = saved(&dir, "max");
    assert!(conversation.contains(r#""quietNote":"While you were idle, 1 quiet message came; it follows.""#), "{conversation}");

    // Sent to several, quietly.
    let sent = call(&mut client, &mut view, "bo", "agents_send", r#"{"to": ["max"], "text": "Docs too.", "quiet": true}"#).await;
    assert_eq!(sent, "Max (max): sent quietly; it reads it when next woken.");

    // A plain message is no quiet one.
    assert_eq!(call(&mut client, &mut view, "bo", "agents_send", r#"{"to": "max", "text": "Now, please."}"#).await, "Sent.");
    view.follow(&mut client, |view| idle(view, "max") && view.agent("max").quiet_waiting == 0).await;
    let quiet: Vec<bool> = [
        events(&mut client, &mut view, "max", "Docs too.").await,
        events(&mut client, &mut view, "max", "Now, please.").await,
    ]
    .iter()
    .map(|events| events.first().expect("delivered").quiet)
    .collect();
    assert_eq!(quiet, [true, false]);
}

#[tokio::test]
async fn a_quiet_message_from_a_marked_sender_marks_no_one() {
    let (_dir, factory, mut client, mut view) = setup().await;
    pipes::taint_for_test(&factory, "bo", "browser:https://example.com");
    let sent = call(&mut client, &mut view, "bo", "agents_send", r#"{"to": "max", "text": "From the page.", "quiet": true}"#).await;
    assert!(sent.starts_with("Sent quietly to Max"), "{sent}");
    assert_eq!(pipes::taint_of_for_test(&factory, "max"), "{}");
}
