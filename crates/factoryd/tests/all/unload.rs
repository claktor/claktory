//! Unloading idle agents' conversations: one nothing has
//! needed for a while leaves memory, and reads back as it was when
//! something needs it again. Never one that's shown, has something waiting
//! for the person, may still compact while idle, or shows something a
//! reload wouldn't.

use std::sync::Arc;
use std::time::Duration;

use crate::common::{Client, LONG_MODEL, Scripted, View, apply_request, create_agent, send_message, start_with, temp_dir};
use factory_protocol::v1::{self, AgentStatus, client_message, entry::Kind};
use factoryd::Factory;
use factoryd::transcript::Transcript;

fn idle(view: &View, agent: &str) -> bool {
    let agent = view.agent(agent);
    agent.status() == AgentStatus::Idle && agent.queued_messages == 0
}

fn select(agent: &str) -> client_message::Body {
    client_message::Body::SelectAgent(v1::SelectAgent { agent_id: agent.into() })
}

/// A Factory whose agent Builder is shown, on `model` when given.
async fn setup(model: Option<&str>) -> (Arc<Factory>, Client, View) {
    let dir = temp_dir();
    let (addr, factory) = start_with(&dir, Scripted::signed_in().with_long_model()).await;
    let (mut client, workspace, _) = Client::connect(addr).await;
    let mut view = View { workspace, ..View::default() };
    apply_request(&mut client, &mut view, create_agent("", "Builder", "")).await;
    if let Some(model) = model {
        let update =
            v1::UpdateAgent { agent_id: "builder".into(), model: Some(model.into()), effort: Some("off".into()), ..Default::default() };
        apply_request(&mut client, &mut view, client_message::Body::UpdateAgent(update)).await;
    }
    apply_request(&mut client, &mut view, select("builder")).await;
    (factory, client, view)
}

/// Builder answers `text`, adding `more` entries at least.
async fn talk(client: &mut Client, view: &mut View, text: &str, more: usize) {
    let before = view.entries.len();
    apply_request(client, view, send_message("builder", text)).await;
    view.follow(client, |view| view.entries.len() >= before + more && idle(view, "builder")).await;
}

/// Applies what the server sends for `time`.
async fn drain(client: &mut Client, view: &mut View, time: Duration) {
    let until = tokio::time::Instant::now() + time;
    while tokio::time::Instant::now() < until {
        tokio::select! {
            message = client.receive() => if let Some(body) = message.body { view.apply(&body) },
            () = tokio::time::sleep_until(until) => {}
        }
    }
}

/// Waits until Builder's conversation is in memory or not, as `held` says.
async fn until_held(factory: &Factory, client: &mut Client, view: &mut View, held: bool) {
    until_held_of(factory, client, view, "builder", held).await;
}

async fn until_held_of(factory: &Factory, client: &mut Client, view: &mut View, agent: &str, held: bool) {
    for _ in 0..250 {
        if factory.conversation_held(agent) == held {
            return;
        }
        drain(client, view, Duration::from_millis(20)).await;
    }
    panic!("{agent}'s conversation never became {}", if held { "held" } else { "unloaded" });
}

/// Builder's conversation as showing it sends it.
async fn shown(client: &mut Client, view: &mut View) -> Vec<v1::Entry> {
    apply_request(client, view, select("builder")).await;
    view.entries.clone()
}

/// Unloads quickly, checking often.
fn soon(factory: &Factory) {
    factory.set_unload_after(Duration::from_millis(50));
    factory.set_idle_check_every(Duration::from_millis(20));
}

#[tokio::test]
async fn an_idle_conversation_leaves_memory_and_reads_back_as_it_was() {
    let (factory, mut client, mut view) = setup(None).await;
    talk(&mut client, &mut view, "hello", 2).await;
    talk(&mut client, &mut view, r#"call bash {"command": "echo step"}"#, 3).await;
    talk(&mut client, &mut view, "and after", 2).await;
    let before = shown(&mut client, &mut view).await;
    assert!(before.iter().any(|entry| matches!(&entry.kind, Some(Kind::ToolCall(_)))), "{before:?}");
    let tokens = view.agent("builder").context_tokens;
    assert!(tokens > 0);

    // Shown, it stays.
    soon(&factory);
    drain(&mut client, &mut view, Duration::from_millis(300)).await;
    assert!(factory.conversation_held("builder"));

    // Not shown and unused, it goes; its context size still shows.
    apply_request(&mut client, &mut view, select("rivet")).await;
    until_held(&factory, &mut client, &mut view, false).await;
    apply_request(&mut client, &mut view, create_agent("", "Other", "")).await;
    assert_eq!(view.agent("builder").context_tokens, tokens);

    // Shown again, it's read back as it was, and carries on.
    factory.set_unload_after(Duration::from_secs(3600));
    assert_eq!(shown(&mut client, &mut view).await, before);
    assert!(factory.conversation_held("builder"));
    talk(&mut client, &mut view, "still here", 2).await;
    let Some(Kind::Assistant(reply)) = view.entries.last().and_then(|entry| entry.kind.as_ref()) else { panic!("a reply") };
    assert!(reply.text.contains("still here"), "{reply:?}");
}

#[tokio::test]
async fn a_conversation_with_a_card_waiting_stays_until_it_s_answered() {
    let (factory, mut client, mut view) = setup(None).await;
    let args = serde_json::json!({"title": "Ship it?", "body": "Only you can say.", "wants_text": false});
    talk(&mut client, &mut view, &format!("call ask_person {args}"), 3).await;
    let seq = view.entries.iter().find_map(|entry| matches!(&entry.kind, Some(Kind::AskPerson(_))).then_some(entry.seq)).expect("the card");
    apply_request(&mut client, &mut view, select("rivet")).await;
    soon(&factory);
    drain(&mut client, &mut view, Duration::from_millis(300)).await;
    assert!(factory.conversation_held("builder"), "a request waits in it");

    let answer = v1::AnswerAsk { agent_id: "builder".into(), seq, dismiss: false, text: String::new(), ..Default::default() };
    apply_request(&mut client, &mut view, client_message::Body::AnswerAsk(answer)).await;
    view.follow(&mut client, |view| idle(view, "builder")).await;
    until_held(&factory, &mut client, &mut view, false).await;
}

/// An hour's cache: the conversation stays while it may still compact
/// before the cache expires, and goes once that moment's past.
#[tokio::test]
async fn a_conversation_that_may_compact_while_idle_stays_until_it_can_t() {
    let (factory, mut client, mut view) = setup(Some(LONG_MODEL)).await;
    factory.set_idle_clock_ahead(Duration::ZERO);
    talk(&mut client, &mut view, "hello", 2).await;
    apply_request(&mut client, &mut view, select("rivet")).await;
    soon(&factory);
    drain(&mut client, &mut view, Duration::from_millis(300)).await;
    assert!(factory.conversation_held("builder"), "it compacts at 58 minutes");

    // Its moment came and went while the machine slept.
    factory.set_idle_clock_ahead(Duration::from_secs(61 * 60));
    until_held(&factory, &mut client, &mut view, false).await;
}

/// A quiet message waits for something else to wake its agent: it doesn't
/// keep the conversation in memory meanwhile,
/// and still comes with what wakes it.
#[tokio::test]
async fn a_quiet_message_waiting_doesn_t_keep_a_conversation_in_memory() {
    let dir = temp_dir();
    let (addr, factory) = start_with(&dir, Scripted::signed_in()).await;
    let (mut client, workspace, _) = Client::connect(addr).await;
    let mut view = View { workspace, ..View::default() };
    apply_request(&mut client, &mut view, crate::common::create_project("Site", &dir.join("site"))).await;
    for name in ["Bo", "Max"] {
        apply_request(&mut client, &mut view, create_agent("site", name, "")).await;
    }
    apply_request(&mut client, &mut view, send_message("max", "hello")).await;
    view.follow(&mut client, |view| idle(view, "max")).await;
    crate::pipes::call(&mut client, &mut view, "bo", "agents_send", r#"{"to": "max", "text": "FYI: the build is green.", "quiet": true}"#)
        .await;
    view.follow(&mut client, |view| view.agent("max").quiet_waiting == 1).await;
    apply_request(&mut client, &mut view, select("rivet")).await;
    soon(&factory);
    until_held_of(&factory, &mut client, &mut view, "max", false).await;

    factory.set_unload_after(Duration::from_secs(3600));
    apply_request(&mut client, &mut view, send_message("max", "and now?")).await;
    apply_request(&mut client, &mut view, select("max")).await;
    view.follow(&mut client, |view| {
        idle(view, "max")
            && view.entries.iter().any(|entry| matches!(&entry.kind, Some(Kind::Event(event)) if event.text == "FYI: the build is green."))
    })
    .await;
}

/// What's shown for display only (a notice, what a failed response
/// streamed) wouldn't read back, so a conversation showing it stays.
#[tokio::test]
async fn a_notice_keeps_a_conversation_in_memory() {
    let dir = temp_dir();
    let mut transcript = Transcript::load(dir.clone(), "builder".into()).await.expect("load");
    let hello: factory_agent::Message =
        serde_json::from_value(serde_json::json!({"role": "user", "content": [{"type": "text", "text": "hello"}], "timestamp": 1}))
            .expect("a message");
    let applied = transcript.apply(factory_agent::AgentEvent::MessageEnd { message: hello }).expect("applied");
    let mut persist = applied.persist.expect("a save");
    assert!(!transcript.reads_back_the_same(), "not saved yet");
    persist.write_now().expect("saved");
    drop(persist);
    assert!(transcript.reads_back_the_same());
    transcript.notice("Lost the connection.", v1::NoticeLevel::Info);
    assert!(!transcript.reads_back_the_same());
}
