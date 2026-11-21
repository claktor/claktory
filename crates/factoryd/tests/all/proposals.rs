//! No agent can message Rivet, the admin agent; an agent proposes a message
//! for it instead (propose_to_rivet), which the person approves, with a
//! comment, or dismisses. Approved, Rivet gets it as the person's message;
//! either way the agent is told.

use crate::common::{Client, MODEL, View, apply_request, create_agent, create_project, describe, send_message, start, temp_dir};
use factory_protocol::v1::{self, AgentStatus, ProposalState, client_message, entry::Kind, server_message::Body};

fn select(agent: &str) -> client_message::Body {
    client_message::Body::SelectAgent(v1::SelectAgent { agent_id: agent.into() })
}

fn idle(view: &View, agent: &str) -> bool {
    let agent = view.agent(agent);
    agent.status() == AgentStatus::Idle && agent.queued_messages == 0
}

fn decide(agent: &str, proposal: &str, approve: bool, comment: &str) -> client_message::Body {
    client_message::Body::DecideProposal(v1::DecideProposal {
        agent_id: agent.into(),
        proposal_id: proposal.into(),
        approve,
        comment: comment.into(),
        ..Default::default()
    })
}

/// Rivet runs on the scripted model, so it can act on what it's sent.
async fn give_rivet_a_model(client: &mut Client, view: &mut View) {
    let rivet = v1::UpdateAgent { agent_id: "rivet".into(), model: Some(MODEL.into()), effort: Some("off".into()), ..Default::default() };
    apply_request(client, view, client_message::Body::UpdateAgent(rivet)).await;
}

/// A Factory with a project "Site" and its agent Builder.
async fn setup(dir: &std::path::Path) -> (std::sync::Arc<factoryd::Factory>, Client, View) {
    let (addr, factory) = start(dir).await;
    let (mut client, workspace, _) = Client::connect(addr).await;
    let mut view = View { workspace, ..View::default() };
    apply_request(&mut client, &mut view, create_project("Site", &dir.join("site"))).await;
    apply_request(&mut client, &mut view, create_agent("site", "Builder", "")).await;
    (factory, client, view)
}

/// Builder proposes `text`; returns what the tool said and the proposal
/// entry it shows, if any.
async fn propose(client: &mut Client, view: &mut View, text: &str) -> (String, Option<v1::Proposal>) {
    apply_request(client, view, select("builder")).await;
    let before = view.entries.len();
    let args = serde_json::json!({"message": text}).to_string();
    apply_request(client, view, send_message("builder", &format!("call propose_to_rivet {args}"))).await;
    view.follow(client, |view| view.entries.len() >= before + 3 && idle(view, "builder")).await;
    let new = &view.entries[before..];
    let output = new.iter().find_map(|entry| match &entry.kind {
        Some(Kind::ToolCall(call)) => Some(call.output.clone()),
        _ => None,
    });
    let proposal = new.iter().find_map(|entry| match &entry.kind {
        Some(Kind::Proposal(proposal)) => Some(proposal.clone()),
        _ => None,
    });
    (output.expect("a tool call"), proposal)
}

fn proposal_entry(entries: &[v1::Entry], id: &str) -> v1::Proposal {
    entries
        .iter()
        .find_map(|entry| match &entry.kind {
            Some(Kind::Proposal(proposal)) if proposal.id == id => Some(proposal.clone()),
            _ => None,
        })
        .expect("the proposal's entry")
}

const NEEDED: &str = "The backend agents are saturated: add a fourth backend agent to Site.";

#[tokio::test]
async fn an_approved_proposal_reaches_rivet_as_the_persons_message_with_their_comment() {
    let dir = temp_dir();
    let (_factory, mut client, mut view) = setup(&dir).await;
    give_rivet_a_model(&mut client, &mut view).await;

    let (said, proposal) = propose(&mut client, &mut view, NEEDED).await;
    assert_eq!(said, "Proposed; the person will approve or dismiss it.");
    let proposal = proposal.expect("the proposal shows after the call");
    assert_eq!((proposal.text.as_str(), proposal.state()), (NEEDED, ProposalState::Pending));
    let shown = describe(&view.entries);
    let at = shown.iter().position(|entry| entry.starts_with("tool propose_to_rivet")).expect("the call");
    assert!(shown[at + 1].starts_with("proposal "), "right after the call: {shown:?}");
    // The call carries its details for the app's view of it.
    let details = view
        .entries
        .iter()
        .find_map(|entry| match &entry.kind {
            Some(Kind::ToolCall(call)) if call.name == "propose_to_rivet" => Some(call.details.clone()),
            _ => None,
        })
        .expect("the call");
    let details: serde_json::Value = serde_json::from_str(&details).expect("details are JSON");
    assert_eq!((details["proposal"].as_str(), details["text"].as_str()), (Some(proposal.id.as_str()), Some(NEEDED)));

    // Approved with a comment: the entry shows it, and Builder is told.
    let reply = apply_request(&mut client, &mut view, decide("builder", &proposal.id, true, "Yes, call it Backend 4.")).await;
    assert!(matches!(reply, Body::Ack(_)), "{reply:?}");
    let decided = proposal_entry(&view.entries, &proposal.id);
    assert_eq!((decided.state(), decided.comment.as_str()), (ProposalState::Approved, "Yes, call it Backend 4."));
    assert!(decided.decided_at_ms > 0);
    view.follow(&mut client, |view| {
        view.entries.iter().any(|entry| matches!(&entry.kind, Some(Kind::Event(event)) if event.source() == v1::EventSource::Factory))
            && idle(view, "builder")
    })
    .await;
    let told = view
        .entries
        .iter()
        .find_map(|entry| match &entry.kind {
            Some(Kind::Event(event)) if event.source() == v1::EventSource::Factory => Some(event.clone()),
            _ => None,
        })
        .expect("Builder is told");
    assert_eq!(told.summary, "The person approved your proposal to Rivet (comment: Yes, call it Backend 4.).");

    // Rivet got it as the person's own message, not an agent's event.
    apply_request(&mut client, &mut view, select("rivet")).await;
    view.follow(&mut client, |view| view.entries.len() >= 2 && idle(view, "rivet")).await;
    let Some(Kind::User(message)) = &view.entries[0].kind else { panic!("the person's message: {:?}", view.entries[0]) };
    assert_eq!(message.text, format!("Yes, call it Backend 4.\n\nBuilder (Site) proposes:\n\n{NEEDED}"));

    // It's decided once.
    let again = apply_request(&mut client, &mut view, decide("builder", &proposal.id, false, "")).await;
    assert!(matches!(again, Body::Error(_)), "{again:?}");
}

#[tokio::test]
async fn a_dismissed_proposal_tells_the_agent_and_rivet_gets_nothing() {
    let dir = temp_dir();
    let (_factory, mut client, mut view) = setup(&dir).await;
    give_rivet_a_model(&mut client, &mut view).await;
    let (_, proposal) = propose(&mut client, &mut view, NEEDED).await;
    let proposal = proposal.expect("proposed");

    let reply = apply_request(&mut client, &mut view, decide("builder", &proposal.id, false, "")).await;
    assert!(matches!(reply, Body::Ack(_)), "{reply:?}");
    assert_eq!(proposal_entry(&view.entries, &proposal.id).state(), ProposalState::Dismissed);
    view.follow(&mut client, |view| view.entries.iter().any(|entry| matches!(&entry.kind, Some(Kind::Event(_)))) && idle(view, "builder"))
        .await;
    let summaries: Vec<String> = view
        .entries
        .iter()
        .filter_map(|entry| match &entry.kind {
            Some(Kind::Event(event)) => Some(event.summary.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(summaries, ["The person dismissed your proposal to Rivet."]);

    let db = rusqlite::Connection::open(dir.join("home/factory.db")).expect("factory.db");
    let to_rivet: i64 = db.query_row("SELECT count(*) FROM messages WHERE agent = 'rivet'", [], |row| row.get(0)).expect("count");
    assert_eq!(to_rivet, 0, "Rivet hears nothing of a dismissed proposal");
}

#[tokio::test]
async fn without_a_rivet_that_can_run_nothing_is_proposed() {
    let dir = temp_dir();
    let (_factory, mut client, mut view) = setup(&dir).await;

    // A new Factory's Rivet has no model here.
    let (said, proposal) = propose(&mut client, &mut view, NEEDED).await;
    assert_eq!(said, "Rivet has no model available, so it can't act on a proposal; nothing was proposed.");
    assert!(proposal.is_none());

    let delete = client_message::Body::DeleteAgent(v1::DeleteAgent { agent_id: "rivet".into() });
    apply_request(&mut client, &mut view, delete).await;
    let (said, proposal) = propose(&mut client, &mut view, NEEDED).await;
    assert_eq!(said, "This Claktory has no Rivet (admin agent) to propose to, so nothing was proposed.");
    assert!(proposal.is_none());

    let db = rusqlite::Connection::open(dir.join("home/factory.db")).expect("factory.db");
    let recorded: i64 = db.query_row("SELECT count(*) FROM proposals", [], |row| row.get(0)).expect("count");
    assert_eq!(recorded, 0);
}

#[tokio::test]
async fn proposals_survive_a_restart_pending_or_decided() {
    let dir = temp_dir();
    let (factory, mut client, mut view) = setup(&dir).await;
    give_rivet_a_model(&mut client, &mut view).await;
    let (_, first) = propose(&mut client, &mut view, "First: add a reviewer.").await;
    let (_, second) = propose(&mut client, &mut view, NEEDED).await;
    let (first, second) = (first.expect("first"), second.expect("second"));
    apply_request(&mut client, &mut view, decide("builder", &first.id, false, "Not now.")).await;
    view.follow(&mut client, |view| idle(view, "builder")).await;
    drop(client);
    drop(factory);

    let (addr, _factory) = start(&dir).await;
    let (mut client, workspace, transcript) = Client::connect(addr).await;
    let mut view = View { workspace, entries: transcript.expect("Builder's conversation").entries, ..View::default() };
    let shown = proposal_entry(&view.entries, &first.id);
    assert_eq!((shown.state(), shown.comment.as_str()), (ProposalState::Dismissed, "Not now."));
    assert_eq!(proposal_entry(&view.entries, &second.id).state(), ProposalState::Pending);
    // Calls read back from disk keep their details.
    let details: Vec<&str> = view
        .entries
        .iter()
        .filter_map(|entry| match &entry.kind {
            Some(Kind::ToolCall(call)) => Some(call.details.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(details.len(), 2);
    assert!(details[1].contains(&second.id), "{details:?}");

    // The pending one can still be approved.
    let reply = apply_request(&mut client, &mut view, decide("builder", &second.id, true, "")).await;
    assert!(matches!(reply, Body::Ack(_)), "{reply:?}");
    assert_eq!(proposal_entry(&view.entries, &second.id).state(), ProposalState::Approved);
    apply_request(&mut client, &mut view, select("rivet")).await;
    view.follow(&mut client, |view| view.entries.len() >= 2 && idle(view, "rivet")).await;
    let Some(Kind::User(message)) = &view.entries[0].kind else { panic!("the person's message: {:?}", view.entries[0]) };
    assert_eq!(message.text, format!("Builder (Site) proposes:\n\n{NEEDED}"));
}

fn user_name(name: &str) -> client_message::Body {
    client_message::Body::UpdateSettings(v1::UpdateSettings { user_name: Some(name.into()), ..Default::default() })
}

#[tokio::test]
async fn agents_call_the_user_by_name_once_it_is_given() {
    let dir = temp_dir();
    let models = crate::common::Scripted::signed_in();
    let system_prompt = models.system_prompt.clone();
    let (addr, _factory) = crate::common::start_with(&dir, models).await;
    let (mut client, workspace, _) = Client::connect(addr).await;
    let mut view = View { workspace, ..View::default() };
    apply_request(&mut client, &mut view, create_project("Site", &dir.join("site"))).await;
    apply_request(&mut client, &mut view, create_agent("site", "Builder", "")).await;
    give_rivet_a_model(&mut client, &mut view).await;

    // Without a name, it's "the person", as ever.
    let (said, _) = propose(&mut client, &mut view, NEEDED).await;
    assert_eq!(said, "Proposed; the person will approve or dismiss it.");
    let prompt = system_prompt.lock().expect("lock").clone();
    assert!(!prompt.contains("\nThe person you work with is") && prompt.contains("where the person approves it"), "{prompt}");

    // Named mid-context: Builder hears it now, and its system prompt stays
    // as it is until its next context.
    apply_request(&mut client, &mut view, user_name("  Matt ")).await;
    assert_eq!(view.workspace.settings.as_ref().map(|settings| settings.user_name.as_str()), Some("Matt"));
    let saved: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(dir.join("home/config.json")).expect("config")).expect("json");
    assert_eq!(saved["user"], serde_json::json!({"name": "Matt"}));
    let (said, proposal) = propose(&mut client, &mut view, NEEDED).await;
    assert_eq!(said, "Proposed; Matt will approve or dismiss it.");
    let told = view.entries.iter().rev().find_map(|entry| match &entry.kind {
        Some(Kind::Event(event)) if event.source() == v1::EventSource::Factory => Some(event.text.clone()),
        _ => None,
    });
    assert!(told.as_deref().is_some_and(|text| text.contains("the person you work with is Matt")), "{told:?}");
    assert!(!system_prompt.lock().expect("lock").contains("Matt"), "the frame holds until the next context");

    apply_request(&mut client, &mut view, decide("builder", &proposal.expect("proposed").id, false, "")).await;
    view.follow(&mut client, |view| {
        view.entries.iter().any(|entry| matches!(&entry.kind, Some(Kind::Event(event)) if event.summary.starts_with("Matt dismissed")))
    })
    .await;

    // Its next context names Matt, in the prompt and the manual.
    let clear = v1::RunCommand { agent_id: "builder".into(), name: "clear".into(), args: String::new() };
    apply_request(&mut client, &mut view, client_message::Body::RunCommand(clear)).await;
    view.follow(&mut client, |view| idle(view, "builder")).await;
    apply_request(&mut client, &mut view, send_message("builder", "hello")).await;
    view.follow(&mut client, |view| view.entries.len() >= 3 && idle(view, "builder")).await;
    let prompt = system_prompt.lock().expect("lock").clone();
    assert!(prompt.contains("\nThe person you work with is Matt.\n") && prompt.contains("where Matt approves it"), "{prompt}");
    assert!(!prompt.contains("{{user}}") && !prompt.contains("{{User}}"));

    // Removed: "the person" again.
    apply_request(&mut client, &mut view, user_name("")).await;
    assert!(!std::fs::read_to_string(dir.join("home/config.json")).expect("config").contains("\"user\""));
}
