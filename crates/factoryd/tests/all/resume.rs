//! Resuming the Workspace on reconnect (#378): an app that comes back with
//! the revision it holds gets only what changed while it was away, and ends
//! up with exactly what a fresh app gets. A restart, or a revision the
//! backend doesn't have, gets a whole Workspace, as before.

use std::sync::Arc;

use crate::common::{
    Client, Scripted, View, apply_patch, apply_request, create_agent, create_project, local_token, send_message, start, start_with, temp_dir,
};
use factory_protocol::v1::{self, ClientMessage, ServerMessage, client_message, server_message::Body};
use factory_protocol::PROTOCOL_VERSION;
use factoryd::factory::Factory;

/// Connects as an app that applies patches, holding `holds`: the first
/// message after Welcome.
async fn hello(addr: std::net::SocketAddr, holds: &str) -> (Client, ServerMessage) {
    let mut client = Client::open(addr).await;
    let hello = v1::Hello {
        protocol_version: PROTOCOL_VERSION,
        client_version: "test".into(),
        token: local_token(addr),
        device_kind: v1::DeviceKind::Phone.into(),
        workspace_patches: true,
        workspace_revision: holds.into(),
        ..Default::default()
    };
    client.send(ClientMessage { id: 1, body: Some(client_message::Body::Hello(hello)) }).await;
    assert!(matches!(client.receive().await.body, Some(Body::Welcome(_))));
    let first = client.receive().await;
    (client, first)
}

/// The Workspace a first message brings an app holding `held` to.
fn applied(held: &v1::Workspace, first: &ServerMessage) -> v1::Workspace {
    match first.body.as_ref().expect("a body") {
        Body::Workspace(whole) => whole.clone(),
        Body::WorkspacePatch(patch) => apply_patch(held, patch),
        other => panic!("expected the Workspace first, got {other:?}"),
    }
}

/// What a fresh app gets now.
async fn fresh(addr: std::net::SocketAddr) -> v1::Workspace {
    let (_client, first) = hello(addr, "").await;
    match first.body {
        Some(Body::Workspace(whole)) => whole,
        other => panic!("a fresh app gets a whole Workspace, got {other:?}"),
    }
}

#[tokio::test]
async fn an_app_back_from_away_gets_only_what_changed_and_sees_what_a_fresh_one_does() {
    let dir = temp_dir();
    let (addr, _factory) = start(&dir).await;
    let (app, first) = hello(addr, "").await;
    let Some(Body::Workspace(mut held)) = first.body.clone() else { panic!("whole first: {first:?}") };
    let mut revision = first.workspace_revision.clone();
    assert!(!revision.is_empty() && first.workspace_base_revision.is_empty());
    drop(app);

    // While it's away, another app (not resuming) changes things, a few
    // times over: each comeback is a patch, equal to a fresh app's view.
    let (mut other, workspace, _) = Client::connect_with(addr, v1::DeviceKind::Desktop, false).await;
    let mut view = crate::common::View { workspace, ..Default::default() };
    apply_request(&mut other, &mut view, create_project("Site", &dir.join("site"))).await;
    for (round, name) in ["Builder", "Helper", "Tester"].into_iter().enumerate() {
        apply_request(&mut other, &mut view, create_agent("site", name, "")).await;
        let rename = v1::UpdateSettings { name: Some(format!("Claktory {round}")), ..Default::default() };
        apply_request(&mut other, &mut view, client_message::Body::UpdateSettings(rename)).await;

        let (back, first) = hello(addr, &revision).await;
        assert!(matches!(first.body, Some(Body::WorkspacePatch(_))), "round {round}: {first:?}");
        assert_eq!(first.workspace_base_revision, revision);
        let before = held.clone();
        held = match first.body.as_ref() {
            Some(Body::WorkspacePatch(patch)) => apply_patch(&held, patch),
            _ => unreachable!(),
        };
        let now = fresh(addr).await;
        assert_eq!(held, now, "round {round}");
        // Real resume patches, for the app's tests (Formaggio).
        if let Some(out) = std::env::var_os("CLAKTORY_RESUME_FIXTURES") {
            use prost::Message as _;
            let out = std::path::Path::new(&out).join(format!("round{round}"));
            std::fs::create_dir_all(&out).expect("fixtures");
            std::fs::write(out.join("held_workspace.pb"), before.encode_to_vec()).expect("held");
            std::fs::write(out.join("resume_server_message.pb"), first.encode_to_vec()).expect("resume");
            std::fs::write(out.join("fresh_workspace.pb"), now.encode_to_vec()).expect("fresh");
        }
        revision = first.workspace_revision.clone();
        drop(back);
    }

    // Nothing changed: still a patch, all of it kept.
    let (_back, first) = hello(addr, &revision).await;
    let Some(Body::WorkspacePatch(patch)) = &first.body else { panic!("a patch: {first:?}") };
    assert!(patch.workspace.as_ref().is_some_and(|workspace| workspace.agents.iter().all(|agent| patch.kept_agents.contains(&agent.id))));
    assert_eq!(apply_patch(&held, patch), fresh(addr).await);

    // A revision it doesn't have: whole.
    let (_stale, first) = hello(addr, "0123456789abcdef-1").await;
    assert!(matches!(first.body, Some(Body::Workspace(_))), "{first:?}");
    assert_eq!(applied(&held, &first), fresh(addr).await);
}

#[tokio::test]
async fn after_a_restart_an_app_gets_a_whole_workspace() {
    let dir = temp_dir();
    let (addr, _factory) = start(&dir).await;
    let (app, first) = hello(addr, "").await;
    let revision = first.workspace_revision.clone();
    drop(app);

    // The same home, another process's backend (the first runs on here).
    let again = Factory::open(crate::common::home(&dir), Arc::new(Scripted::signed_in())).await.expect("open again");
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("addr");
    crate::common::serves(addr, crate::common::home(&dir));
    tokio::spawn(factoryd::serve(listener, again.clone()));
    let (_back, first) = hello(addr, &revision).await;
    assert!(matches!(first.body, Some(Body::Workspace(_))), "{first:?}");
    assert!(first.workspace_base_revision.is_empty() && !first.workspace_revision.is_empty());
    assert_ne!(first.workspace_revision, revision);
}

// ---- The conversation shown (#413, #378's second step) ----

/// Connects as an app holding `agent`'s conversation at `revision` (both
/// empty: none), and reads up to its Transcript.
async fn holding(addr: std::net::SocketAddr, agent: &str, revision: &str) -> (Client, ServerMessage) {
    let mut client = Client::open(addr).await;
    let hello = v1::Hello {
        protocol_version: PROTOCOL_VERSION,
        client_version: "test".into(),
        token: local_token(addr),
        device_kind: v1::DeviceKind::Phone.into(),
        transcript_agent_id: agent.into(),
        transcript_revision: revision.into(),
        ..Default::default()
    };
    client.send(ClientMessage { id: 1, body: Some(client_message::Body::Hello(hello)) }).await;
    loop {
        let message = client.receive().await;
        if matches!(message.body, Some(Body::Transcript(_))) {
            return (client, message);
        }
    }
}

fn transcript(message: &ServerMessage) -> &v1::Transcript {
    match message.body.as_ref() {
        Some(Body::Transcript(transcript)) => transcript,
        other => panic!("expected a Transcript, got {other:?}"),
    }
}

/// What an app holds after `message`, from `held`: a whole Transcript
/// replaces it, a resumed one puts each entry in place by seq.
fn apply(held: &mut Vec<v1::Entry>, message: &ServerMessage) {
    let shown = transcript(message);
    if !shown.resumed {
        held.clone_from(&shown.entries);
        return;
    }
    for entry in &shown.entries {
        match held.iter_mut().find(|known| known.seq == entry.seq) {
            Some(known) => *known = entry.clone(),
            None => held.push(entry.clone()),
        }
    }
    held.sort_by_key(|entry| entry.seq);
}

#[tokio::test]
async fn an_app_back_with_the_conversation_gets_only_its_new_entries_and_ends_as_a_fresh_one() {
    use prost::Message as _;
    let dir = temp_dir();
    let (addr, _factory) = start_with(&dir, Scripted::signed_in()).await;
    let (mut other, workspace, _) = Client::connect_with(addr, v1::DeviceKind::Desktop, false).await;
    let mut view = View { workspace, ..Default::default() };
    apply_request(&mut other, &mut view, create_agent("", "Ops", "")).await;
    apply_request(&mut other, &mut view, client_message::Body::SelectAgent(v1::SelectAgent { agent_id: "ops".into() })).await;
    let idle = |view: &View| view.agent("ops").status() == v1::AgentStatus::Idle && view.agent("ops").queued_messages == 0;
    // A conversation with some length to it, as a real one has.
    for round in 0..4 {
        let text = format!("Round {round}: {}", "the plan, at length. ".repeat(40));
        apply_request(&mut other, &mut view, send_message("ops", &text)).await;
        let count = view.entries.len();
        view.follow(&mut other, |view| view.entries.len() > count && idle(view)).await;
    }
    // And a question for the person, waiting.
    let args = serde_json::json!({"title": "Ship it?", "body": "Only you can say."});
    apply_request(&mut other, &mut view, send_message("ops", &format!("call ask_person {args}"))).await;
    view.follow(&mut other, |view| view.entries.iter().any(|entry| matches!(entry.kind, Some(v1::entry::Kind::AskPerson(_)))) && idle(view))
        .await;
    let card = view.entries.iter().find(|entry| matches!(entry.kind, Some(v1::entry::Kind::AskPerson(_)))).expect("the card").seq;

    // An app reads the conversation, and goes away.
    let (app, first) = holding(addr, "", "").await;
    assert!(!transcript(&first).resumed && !first.transcript_revision.is_empty(), "{first:?}");
    let mut held = transcript(&first).entries.clone();
    let mut revision = first.transcript_revision.clone();
    let before = held.len();
    drop(app);

    // Meanwhile the question is answered (its card, an earlier entry,
    // changes) and the agent answers twice more; back, it gets only that.
    let answer = v1::AnswerAsk { agent_id: "ops".into(), seq: card, text: "Yes.".into(), ..Default::default() };
    apply_request(&mut other, &mut view, client_message::Body::AnswerAsk(answer)).await;
    for text in ["Third.", "Fourth."] {
        apply_request(&mut other, &mut view, send_message("ops", text)).await;
        let count = view.entries.len();
        view.follow(&mut other, |view| view.entries.len() > count && idle(view)).await;
    }
    let (_back, resumed) = holding(addr, "ops", &revision).await;
    let shown = transcript(&resumed);
    assert!(shown.resumed, "{resumed:?}");
    let newest = held.last().map_or(0, |entry| entry.seq);
    assert!(shown.entries.iter().any(|entry| entry.seq == card), "the card as answered: {shown:?}");
    assert!(shown.entries.iter().all(|entry| entry.seq > newest || entry.seq == card), "only what changed: {shown:?}");
    assert!(shown.entries.iter().any(|entry| entry.seq > newest), "and what's new");
    apply(&mut held, &resumed);
    assert!(held.len() > before);
    let (_fresh, page) = holding(addr, "", "").await;
    assert_eq!(held, transcript(&page).entries, "what a fresh app holds");
    assert!(
        resumed.encoded_len() * 2 < page.encoded_len(),
        "resumed {} bytes, the page {}",
        resumed.encoded_len(),
        page.encoded_len()
    );
    revision.clone_from(&resumed.transcript_revision);

    // Back with nothing new: nothing, at the same revision.
    let (mut live, again) = holding(addr, "ops", &revision).await;
    assert!(transcript(&again).resumed && transcript(&again).entries.is_empty(), "{again:?}");
    assert_eq!(again.transcript_revision, revision);

    // Changes while connected carry the revision they bring it to; back
    // from the last one, nothing.
    apply_request(&mut other, &mut view, send_message("ops", "Fifth.")).await;
    let count = view.entries.len();
    view.follow(&mut other, |view| view.entries.len() > count && idle(view)).await;
    let mut latest = String::new();
    while held.len() < view.entries.len() || latest.is_empty() {
        let message = live.receive().await;
        match message.body {
            Some(Body::Entry(update)) => {
                let entry = update.entry.expect("the entry");
                assert!(!message.transcript_revision.is_empty(), "an entry's change carries its revision");
                latest = message.transcript_revision.clone();
                match held.iter_mut().find(|known| known.seq == entry.seq) {
                    Some(known) => *known = entry,
                    None => held.push(entry),
                }
            }
            Some(Body::Delta(_)) => latest = message.transcript_revision.clone(),
            _ => {}
        }
    }
    drop(live);
    let (_back, caught_up) = holding(addr, "ops", &latest).await;
    assert!(transcript(&caught_up).resumed, "{caught_up:?}");
    apply(&mut held, &caught_up);
    let (_fresh, page) = holding(addr, "", "").await;
    assert_eq!(held, transcript(&page).entries);

    // The latest page instead: another agent's, a revision it never gave,
    // or once another agent was shown (Ops is shown again, whole).
    for (agent, held) in [("rivet", latest.as_str()), ("ops", "0123456789abcdef-1"), ("ops", "nonsense")] {
        let (_app, first) = holding(addr, agent, held).await;
        assert!(!transcript(&first).resumed, "{agent} {held}");
    }
    apply_request(&mut other, &mut view, create_agent("", "Ana", "")).await;
    apply_request(&mut other, &mut view, client_message::Body::SelectAgent(v1::SelectAgent { agent_id: "ana".into() })).await;
    apply_request(&mut other, &mut view, client_message::Body::SelectAgent(v1::SelectAgent { agent_id: "ops".into() })).await;
    let (_app, first) = holding(addr, "ops", &latest).await;
    assert!(!transcript(&first).resumed, "{first:?}");
    assert_eq!(transcript(&first).entries, transcript(&page).entries);
}
