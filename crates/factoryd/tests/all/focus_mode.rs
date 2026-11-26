//! Needs you's focus mode (#514): each device's own focus mode, kept in the
//! backend; which device decided each card; and watching a conversation
//! without selecting it (focus mode's Context).

use crate::common::{Client, View, apply_request, create_agent, create_project, send_message, start, temp_dir};
use factory_protocol::v1::{self, AgentStatus, ErrorCode, client_message, entry::Kind, server_message::Body};

fn select(agent: &str) -> client_message::Body {
    client_message::Body::SelectAgent(v1::SelectAgent { agent_id: agent.into() })
}

fn focus(on: bool) -> client_message::Body {
    client_message::Body::SetFocusMode(v1::SetFocusMode { on })
}

fn watch(agent: Option<&str>) -> client_message::Body {
    let target = agent.map(|agent| v1::watch_transcript::Target::AgentId(agent.into()));
    client_message::Body::WatchTranscript(v1::WatchTranscript { target })
}

fn focused(view: &View) -> Vec<String> {
    view.workspace.settings.clone().expect("settings").focus_mode
}

fn idle(view: &View, agent: &str) -> bool {
    let agent = view.agent(agent);
    agent.status() == AgentStatus::Idle && agent.queued_messages == 0
}

/// A Factory with a project "Site" and its agents Builder and Writer, Builder shown.
async fn setup(dir: &std::path::Path) -> (std::sync::Arc<factoryd::Factory>, std::net::SocketAddr, Client, View) {
    let (addr, factory) = start(dir).await;
    let (mut client, workspace, _) = Client::connect(addr).await;
    let mut view = View { workspace, ..View::default() };
    apply_request(&mut client, &mut view, create_project("Site", &dir.join("site"))).await;
    apply_request(&mut client, &mut view, create_agent("site", "Builder", "")).await;
    apply_request(&mut client, &mut view, create_agent("site", "Writer", "")).await;
    apply_request(&mut client, &mut view, select("builder")).await;
    (factory, addr, client, view)
}

#[tokio::test]
async fn focus_mode_is_each_devices_own_and_kept() {
    let dir = temp_dir();
    let (addr, factory) = start(&dir).await;
    let (mut client, workspace, _) = Client::connect(addr).await;
    let mut view = View { workspace, ..View::default() };
    assert!(focused(&view).is_empty());

    // This computer's app is the device "local"; another of its
    // connections sees it too.
    let reply = apply_request(&mut client, &mut view, focus(true)).await;
    assert!(matches!(reply, Body::Ack(_)), "{reply:?}");
    assert_eq!(focused(&view), ["local"]);
    let (_, other, _) = Client::connect(addr).await;
    assert_eq!(other.settings.expect("settings").focus_mode, ["local"]);
    // Twice changes nothing.
    apply_request(&mut client, &mut view, focus(true)).await;
    assert_eq!(focused(&view), ["local"]);
    drop(client);
    drop(factory);

    // After a restart it's still on, and off is off.
    let (addr, _factory) = start(&dir).await;
    let (mut client, workspace, _) = Client::connect(addr).await;
    let mut view = View { workspace, ..View::default() };
    assert_eq!(focused(&view), ["local"]);
    apply_request(&mut client, &mut view, focus(false)).await;
    assert!(focused(&view).is_empty());
}

#[tokio::test]
async fn a_decided_card_says_which_device_decided_it() {
    let dir = temp_dir();
    let (factory, _, mut client, mut view) = setup(&dir).await;
    let args = serde_json::json!({"title": "Ship it?", "body": "Only you can say."});
    apply_request(&mut client, &mut view, send_message("builder", &format!("call ask_person {args}"))).await;
    view.follow(&mut client, |view| !view.agent("builder").waiting_seqs.is_empty() && idle(view, "builder")).await;
    let seq = view.agent("builder").waiting_seqs[0];
    let card = |view: &View| view.entries.iter().find(|entry| entry.seq == seq).cloned().expect("the card");
    assert_eq!(card(&view).answered_on, None, "nothing while it waits");

    let answer = v1::AnswerAsk { agent_id: "builder".into(), seq, text: "Yes.".into(), ..Default::default() };
    apply_request(&mut client, &mut view, client_message::Body::AnswerAsk(answer)).await;
    view.follow(&mut client, |view| card(view).answered_on.is_some()).await;
    let on = card(&view).answered_on.expect("where from");
    assert_eq!(on.device_id, "local");
    assert!(!on.device_name.is_empty());
    assert!(on.at_ms > 0);
    assert!(matches!(card(&view).kind, Some(Kind::AskPerson(ask)) if ask.state() == v1::AskPersonState::Answered));

    // History says it too.
    let history = client.request(client_message::Body::ListNeedsYouHistory(v1::ListNeedsYouHistory::default())).await;
    let Body::NeedsYouHistory(history) = history.body else { panic!("{:?}", history.body) };
    let record = history.records.iter().find(|record| record.title == "Ship it?").expect("the record");
    assert_eq!(record.answered_on.as_ref(), Some(&on));
    drop(client);
    drop(factory);

    // And after a restart.
    let (addr, _factory) = start(&dir).await;
    let (mut client, workspace, _) = Client::connect(addr).await;
    let mut view = View { workspace, ..View::default() };
    apply_request(&mut client, &mut view, select("builder")).await;
    assert_eq!(card(&view).answered_on, Some(on));
}

/// Writer has replied to something new, as a connection watching it sees.
fn replied(view: &View, to: usize) -> bool {
    let replies = view.watched.iter().filter(|body| {
        matches!(body, Body::Entry(update) if update.agent_id == "writer"
            && matches!(update.entry.as_ref().and_then(|entry| entry.kind.as_ref()), Some(Kind::Assistant(_))))
    });
    replies.count() > to && idle(view, "writer")
}

#[tokio::test]
async fn a_connection_watches_a_conversation_without_selecting_it() {
    let dir = temp_dir();
    let (_factory, addr, mut client, mut view) = setup(&dir).await;
    apply_request(&mut client, &mut view, send_message("builder", "Builder's first.")).await;
    view.follow(&mut client, |view| idle(view, "builder")).await;
    let builders = view.entries.clone();
    let (mut other, workspace, _) = Client::connect(addr).await;
    let mut others = View { workspace, ..View::default() };

    // The snapshot first, kept apart, then the answer; nothing else moves.
    let reply = client.request(watch(Some("writer"))).await;
    assert!(matches!(reply.body, Body::Ack(_)), "{:?}", reply.body);
    let watched: Vec<&v1::ServerMessage> = reply.events.iter().filter(|event| event.watched).collect();
    assert!(
        matches!(watched.first().and_then(|event| event.body.as_ref()), Some(Body::Transcript(transcript)) if transcript.agent_id == "writer")
    );
    assert!(watched.iter().any(|event| matches!(&event.body, Some(Body::QueuedMessages(queued)) if queued.agent_id == "writer")));
    for event in reply.events {
        view.take(event);
    }
    assert_eq!(view.workspace.selected_agent_id, "builder");

    // Writer's conversation comes to this connection, tagged; Builder's,
    // shown, is as it was, and so is the selection.
    apply_request(&mut client, &mut view, send_message("writer", "Hello, Writer.")).await;
    view.follow(&mut client, |view| replied(view, 0)).await;
    assert_eq!(view.entries, builders);
    assert_eq!(view.workspace.selected_agent_id, "builder");
    // Another connection, watching nothing, gets none of it.
    for message in other.ping(b"first").await {
        assert!(!message.watched, "{message:?}");
        others.take(message);
    }

    // It stops watching and the other starts: only the other hears of
    // Writer now.
    assert!(matches!(client.request(watch(None)).await.body, Body::Ack(_)));
    let started = apply_request(&mut other, &mut others, watch(Some("writer"))).await;
    assert!(matches!(started, Body::Ack(_)), "{started:?}");
    view.watched.clear();
    let before = others.watched.len();
    apply_request(&mut other, &mut others, send_message("writer", "Again.")).await;
    others.follow(&mut other, |view| view.watched.len() > before && replied(view, 0)).await;
    for message in client.ping(b"stopped").await {
        view.take(message);
    }
    assert!(view.watched.is_empty(), "{:?}", view.watched);

    // Unknown: refused.
    match client.request(watch(Some("nobody"))).await.body {
        Body::Error(error) => assert_eq!(error.code(), ErrorCode::NotFound),
        other => panic!("not refused: {other:?}"),
    }
}
