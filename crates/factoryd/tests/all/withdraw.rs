//! An agent takes back its own cards once they no longer apply (#407):
//! ask_withdraw for its questions (procedures, review asks and agents_clear's
//! cards too), proposal_withdraw for its proposals to Rivet. A withdrawn
//! card leaves Needs you at once, frees its place, shows as withdrawn with
//! the agent's reason, and is listed so in Needs you's history; it can't be
//! answered after. Never another agent's.

use crate::common::{Client, MODEL, View, apply_request, create_agent, create_project, send_message, start, temp_dir};
use factory_protocol::v1::{
    self, AgentStatus, AskPersonState, NeedsYouOutcome, NeedsYouRecordKind, ProposalState, client_message, entry::Kind, server_message::Body,
};

fn select(agent: &str) -> client_message::Body {
    client_message::Body::SelectAgent(v1::SelectAgent { agent_id: agent.into() })
}

fn idle(view: &View, agent: &str) -> bool {
    let agent = view.agent(agent);
    agent.status() == AgentStatus::Idle && agent.queued_messages == 0
}

/// Has `agent` call `tool` with `args` (the scripted model does what it's
/// told) and returns what the tool answered.
async fn call(client: &mut Client, view: &mut View, agent: &str, tool: &str, args: serde_json::Value) -> String {
    apply_request(client, view, select(agent)).await;
    let before = view.entries.len();
    apply_request(client, view, send_message(agent, &format!("call {tool} {args}"))).await;
    let output = |view: &View| {
        view.entries.get(before..).unwrap_or_default().iter().find_map(|entry| match &entry.kind {
            Some(Kind::ToolCall(call)) if call.name == tool && !call.output.is_empty() => Some(call.output.clone()),
            _ => None,
        })
    };
    view.follow(client, |view| output(view).is_some() && idle(view, agent)).await;
    output(view).expect("the call's output")
}

/// The card with id `id` in the selected agent's conversation, and its seq.
fn card(view: &View, id: &str) -> (u64, v1::AskPerson) {
    view.entries
        .iter()
        .find_map(|entry| match &entry.kind {
            Some(Kind::AskPerson(ask)) if ask.id == id => Some((entry.seq, ask.clone())),
            _ => None,
        })
        .expect("the card")
}

/// The id an ask_person (or propose_to_rivet) result gives.
fn id_in(said: &str) -> String {
    let (_, rest) = said.split_once("Its id is ").expect("an id");
    rest.split(':').next().expect("the id").to_owned()
}

async fn history(client: &mut Client) -> Vec<v1::NeedsYouRecord> {
    let list = client_message::Body::ListNeedsYouHistory(v1::ListNeedsYouHistory { query: String::new(), before_ms: 0, limit: 0 });
    match client.request(list).await.body {
        Body::NeedsYouHistory(history) => history.records,
        other => panic!("expected NeedsYouHistory, got {other:?}"),
    }
}

fn answer(agent: &str, seq: u64, text: &str) -> client_message::Body {
    client_message::Body::AnswerAsk(v1::AnswerAsk { agent_id: agent.into(), seq, text: text.into(), ..Default::default() })
}

async fn refused(client: &mut Client, view: &mut View, request: client_message::Body) -> String {
    match apply_request(client, view, request).await {
        Body::Error(error) => error.message,
        other => panic!("expected a refusal, got {other:?}"),
    }
}

/// A Factory with a project "Site" and its agents Builder and Writer.
async fn setup(dir: &std::path::Path) -> (std::sync::Arc<factoryd::Factory>, Client, View) {
    let (addr, factory) = start(dir).await;
    let (mut client, workspace, _) = Client::connect(addr).await;
    let mut view = View { workspace, ..View::default() };
    apply_request(&mut client, &mut view, create_project("Site", &dir.join("site"))).await;
    apply_request(&mut client, &mut view, create_agent("site", "Builder", "")).await;
    apply_request(&mut client, &mut view, create_agent("site", "Writer", "")).await;
    (factory, client, view)
}

#[tokio::test]
async fn an_agent_withdraws_its_own_question_which_leaves_needs_you_and_frees_its_place() {
    let dir = temp_dir();
    let (_factory, mut client, mut view) = setup(&dir).await;
    let checklist = v1::ExtensionChange { id: "checklist".into(), enabled: Some(true), ..Default::default() };
    let on = v1::UpdateAgent { agent_id: "builder".into(), extension: Some(checklist), ..Default::default() };
    apply_request(&mut client, &mut view, client_message::Body::UpdateAgent(on)).await;
    call(&mut client, &mut view, "builder", "checklist_add", serde_json::json!({"items": ["Pick the logo"]})).await;
    call(
        &mut client,
        &mut view,
        "builder",
        "checklist_update",
        serde_json::json!({"updates": [{"id": 1, "status": "blocked", "waits_on": "person_decision"}]}),
    )
    .await;

    // Writer's question, which Builder can't touch.
    let theirs = call(&mut client, &mut view, "writer", "ask_person", serde_json::json!({"title": "Tone?", "body": "For the blog."})).await;
    let theirs = id_in(&theirs);

    // Builder's three: a question about its item, a procedure, and one more.
    let about = serde_json::json!({"title": "Round or square?", "body": "For the logo.", "item": "1"});
    let said = call(&mut client, &mut view, "builder", "ask_person", about).await;
    assert!(said.contains("take it back with ask_withdraw"), "{said}");
    let logo = id_in(&said);
    let steps = serde_json::json!({"title": "Set up signing", "body": "So releases are signed.", "steps": ["Open Settings.", "Add the key."]});
    let signing = id_in(&call(&mut client, &mut view, "builder", "ask_person", steps).await);
    let third = id_in(&call(&mut client, &mut view, "builder", "ask_person", serde_json::json!({"title": "Ship Friday?", "body": "B"})).await);
    view.follow(&mut client, |view| view.agent("builder").waiting_seqs.len() == 3 && view.agent("builder").waiting_items.len() == 1)
        .await;
    let full = call(&mut client, &mut view, "builder", "ask_person", serde_json::json!({"title": "A fourth?", "body": "B"})).await;
    assert!(full.starts_with("You already have 3 questions waiting"), "{full}");

    // Without id: its own, with their ids.
    let listed = call(&mut client, &mut view, "builder", "ask_withdraw", serde_json::json!({})).await;
    assert!(listed.starts_with("Your questions waiting for the person; take one back with its id and a reason:"), "{listed}");
    assert!(listed.contains(&format!("\n- {logo}: \"Round or square?\", asked ")) && listed.contains(", about #1"), "{listed}");
    assert!(listed.contains(&format!("\n- {signing}: \"Set up signing\", asked ")) && listed.contains("0 of 2 steps done"), "{listed}");
    assert!(!listed.contains("Tone?"), "only its own: {listed}");

    // Never another's, and never without a reason.
    let other = call(&mut client, &mut view, "builder", "ask_withdraw", serde_json::json!({"id": theirs, "reason": "Not needed."})).await;
    assert_eq!(other, format!("You have no question {theirs}: ask_withdraw without id lists yours. Nothing was withdrawn."));
    let bare = call(&mut client, &mut view, "builder", "ask_withdraw", serde_json::json!({"id": logo})).await;
    assert_eq!(bare, "Give reason: why it no longer applies, in a short line the person reads. Nothing was withdrawn.");
    apply_request(&mut client, &mut view, select("writer")).await;
    assert_eq!(card(&view, &theirs).1.state(), AskPersonState::Pending, "Writer's question still waits");

    // Withdrawn: it leaves Needs you, its card says why, and the item is as it was.
    let said = call(&mut client, &mut view, "builder", "ask_withdraw", serde_json::json!({"id": format!("#{logo}"), "reason": "Square was picked in the brand guide."})).await;
    assert_eq!(
        said,
        format!(
            "Withdrew question {logo}, \"Round or square?\": it's gone from the person's Needs you, and their History shows it withdrawn by you, with your reason. It was about #1, which stays as it was: update it if it no longer waits on the person."
        )
    );
    let (seq, shown) = card(&view, &logo);
    assert_eq!(
        (shown.state(), shown.withdrawn, shown.withdraw_reason.as_str(), shown.answer.as_str()),
        (AskPersonState::Dismissed, true, "Square was picked in the brand guide.", "")
    );
    assert!(shown.decided_ms > 0);
    view.follow(&mut client, |view| view.agent("builder").waiting_seqs.len() == 2 && view.agent("builder").waiting_items.is_empty()).await;
    assert!(!view.agent("builder").waiting_seqs.contains(&seq));
    let items = call(&mut client, &mut view, "builder", "checklist_list", serde_json::json!({})).await;
    assert!(items.contains("#1 [!] Pick the logo [waits on person decision]"), "the item stays blocked: {items}");

    // The person can't answer it now.
    assert_eq!(refused(&mut client, &mut view, answer("builder", seq, "Round.")).await, "This question was withdrawn.");
    let again = call(&mut client, &mut view, "builder", "ask_withdraw", serde_json::json!({"id": logo, "reason": "Again."})).await;
    assert_eq!(again, format!("Question {logo} isn't waiting: You already withdrew it. Nothing was withdrawn."));

    // Its place is free.
    let fourth = call(&mut client, &mut view, "builder", "ask_person", serde_json::json!({"title": "A fourth?", "body": "B"})).await;
    assert!(fourth.starts_with("Asked the person"), "{fourth}");

    // A procedure half done is withdrawn the same way.
    let (step_seq, _) = card(&view, &signing);
    let done = v1::AnswerAsk { agent_id: "builder".into(), seq: step_seq, step: Some(0), ..Default::default() };
    assert!(matches!(apply_request(&mut client, &mut view, client_message::Body::AnswerAsk(done)).await, Body::Ack(_)));
    let said = call(&mut client, &mut view, "builder", "ask_withdraw", serde_json::json!({"id": signing, "reason": "Signing moved to CI."})).await;
    assert!(said.starts_with(&format!("Withdrew question {signing}, \"Set up signing\"")), "{said}");
    let (_, procedure) = card(&view, &signing);
    assert_eq!((procedure.state(), procedure.withdrawn), (AskPersonState::Dismissed, true));
    assert_eq!(procedure.steps[0].state(), v1::AskStepState::Done);

    // History lists them as withdrawn, with the agent's reason, never as the person's.
    let records = history(&mut client).await;
    let withdrawn: Vec<_> = records.iter().filter(|record| record.outcome() == NeedsYouOutcome::Withdrawn).collect();
    assert_eq!(withdrawn.len(), 2, "{records:?}");
    let logo_record = withdrawn.iter().find(|record| record.title == "Round or square?").expect("listed");
    assert_eq!(
        (logo_record.kind(), logo_record.agent_id.as_str(), logo_record.agent_reason.as_str(), logo_record.reply.as_str(), logo_record.seq),
        (NeedsYouRecordKind::Question, "builder", "Square was picked in the brand guide.", "", seq)
    );
    assert!(!records.iter().any(|record| record.title == "Ship Friday?"), "a question still waiting isn't listed");

    // The list has only what still waits.
    let listed = call(&mut client, &mut view, "builder", "ask_withdraw", serde_json::json!({})).await;
    assert!(!listed.contains("Round or square?") && listed.contains(&format!("- {third}: \"Ship Friday?\"")), "{listed}");
    assert!(listed.contains("A fourth?"), "{listed}");
}

#[tokio::test]
async fn an_agent_withdraws_its_own_proposal_to_rivet() {
    let dir = temp_dir();
    let (_factory, mut client, mut view) = setup(&dir).await;
    let rivet = v1::UpdateAgent { agent_id: "rivet".into(), model: Some(MODEL.into()), effort: Some("off".into()), ..Default::default() };
    apply_request(&mut client, &mut view, client_message::Body::UpdateAgent(rivet)).await;

    let theirs = call(&mut client, &mut view, "writer", "propose_to_rivet", serde_json::json!({"message": "Add an editor."})).await;
    let theirs = id_in(&theirs);
    let said = call(&mut client, &mut view, "builder", "propose_to_rivet", serde_json::json!({"message": "Add a backend agent.\nWe're saturated."})).await;
    assert!(said.starts_with("Proposed; the person will approve or dismiss it. Its id is "), "{said}");
    let mine = id_in(&said);
    view.follow(&mut client, |view| view.agent("builder").waiting_seqs.len() == 1).await;

    let listed = call(&mut client, &mut view, "builder", "proposal_withdraw", serde_json::json!({})).await;
    assert!(listed.starts_with("Your proposals waiting for the person; take one back with its id and a reason:"), "{listed}");
    assert!(listed.contains(&format!("\n- {mine}: \"Add a backend agent.\", proposed ")) && !listed.contains("editor"), "{listed}");
    let other = call(&mut client, &mut view, "builder", "proposal_withdraw", serde_json::json!({"id": theirs, "reason": "No."})).await;
    assert_eq!(other, format!("You have no proposal {theirs}: proposal_withdraw without id lists yours. Nothing was withdrawn."));

    let said = call(&mut client, &mut view, "builder", "proposal_withdraw", serde_json::json!({"id": mine, "reason": "Backend 4 was added today."})).await;
    assert_eq!(
        said,
        format!("Withdrew proposal {mine}: it's gone from the person's Needs you, and their History shows it withdrawn by you, with your reason.")
    );
    let shown = view
        .entries
        .iter()
        .find_map(|entry| match &entry.kind {
            Some(Kind::Proposal(proposal)) if proposal.id == mine => Some(proposal.clone()),
            _ => None,
        })
        .expect("the proposal");
    assert_eq!(
        (shown.state(), shown.withdrawn, shown.withdraw_reason.as_str(), shown.comment.as_str()),
        (ProposalState::Dismissed, true, "Backend 4 was added today.", "")
    );
    view.follow(&mut client, |view| view.agent("builder").waiting_seqs.is_empty()).await;

    // The person can't approve it now, and Rivet never gets it.
    let decide = v1::DecideProposal { agent_id: "builder".into(), proposal_id: mine.clone(), approve: true, ..Default::default() };
    assert_eq!(refused(&mut client, &mut view, client_message::Body::DecideProposal(decide)).await, "This proposal was withdrawn.");

    let records = history(&mut client).await;
    let record = records.iter().find(|record| record.kind() == NeedsYouRecordKind::Proposal).expect("listed");
    assert_eq!(
        (record.outcome(), record.title.as_str(), record.agent_reason.as_str()),
        (NeedsYouOutcome::Withdrawn, "Add a backend agent.", "Backend 4 was added today.")
    );
    // Writer's still waits.
    assert_eq!(view.agent("writer").waiting_seqs.len(), 1);
}
