//! An agent changes its own cards while they wait (#428): ask_update for its
//! questions (title, body, wants_text, review, a procedure's steps),
//! proposal_update for its proposals' text. The card redraws in place, at
//! the same seq, as a new version, marked updated with the agent's reason;
//! it doesn't count again in Needs you. An answer to the version before is
//! refused; one that names no version (an app before #428) is refused only
//! just after a change. At most 10 changes a card, never another agent's,
//! and History keeps one record, saying how many times it changed.

use crate::common::{Client, MODEL, View, apply_request, create_agent, create_project, send_message, start, temp_dir};
use factory_protocol::v1::{self, AgentStatus, AskPersonState, AskStepState, NeedsYouRecordKind, ProposalState, client_message, entry::Kind, server_message::Body};

fn select(agent: &str) -> client_message::Body {
    client_message::Body::SelectAgent(v1::SelectAgent { agent_id: agent.into() })
}

fn idle(view: &View, agent: &str) -> bool {
    let agent = view.agent(agent);
    agent.status() == AgentStatus::Idle && agent.queued_messages == 0
}

/// Has `agent` call `tool` with `args` and returns what the tool answered.
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

fn card(view: &View, id: &str) -> (u64, v1::AskPerson) {
    view.entries
        .iter()
        .find_map(|entry| match &entry.kind {
            Some(Kind::AskPerson(ask)) if ask.id == id => Some((entry.seq, ask.clone())),
            _ => None,
        })
        .expect("the card")
}

fn proposal(view: &View, id: &str) -> (u64, v1::Proposal) {
    view.entries
        .iter()
        .find_map(|entry| match &entry.kind {
            Some(Kind::Proposal(proposal)) if proposal.id == id => Some((entry.seq, proposal.clone())),
            _ => None,
        })
        .expect("the proposal")
}

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

fn answer(seq: u64, text: &str, revision: Option<u32>) -> client_message::Body {
    client_message::Body::AnswerAsk(v1::AnswerAsk { agent_id: "builder".into(), seq, text: text.into(), revision, ..Default::default() })
}

async fn reply_to(client: &mut Client, view: &mut View, request: client_message::Body) -> Result<(), String> {
    match apply_request(client, view, request).await {
        Body::Ack(_) => Ok(()),
        Body::Error(error) => Err(error.message),
        other => panic!("expected an ack or a refusal, got {other:?}"),
    }
}

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
async fn an_agent_changes_its_own_question_in_place_and_an_answer_to_the_old_one_is_refused() {
    let dir = temp_dir();
    let (factory, mut client, mut view) = setup(&dir).await;
    let theirs = id_in(&call(&mut client, &mut view, "writer", "ask_person", serde_json::json!({"title": "Tone?", "body": "For the blog."})).await);
    let said = call(&mut client, &mut view, "builder", "ask_person", serde_json::json!({"title": "Deploy Friday?", "body": "The release is ready."})).await;
    assert!(said.contains("or change it with ask_update if what you need changes."), "{said}");
    let id = id_in(&said);
    let (seq, asked) = card(&view, &id);
    assert_eq!((asked.revision, asked.updated_ms, asked.update_reason.as_str()), (1, 0, ""));
    view.follow(&mut client, |view| view.agent("builder").waiting_seqs == [seq]).await;

    // Never another's; something must change.
    let other = call(&mut client, &mut view, "builder", "ask_update", serde_json::json!({"id": theirs, "title": "Mine now?"})).await;
    assert_eq!(other, format!("You have no question {theirs}: ask_withdraw without id lists yours. Nothing was changed."));
    let nothing = call(&mut client, &mut view, "builder", "ask_update", serde_json::json!({"id": id})).await;
    assert_eq!(nothing, "Give what changes: title, body, wants_text, review or steps. Nothing was changed.");
    let same = call(&mut client, &mut view, "builder", "ask_update", serde_json::json!({"id": id, "title": "Deploy  Friday?"})).await;
    assert_eq!(same, format!("That's what question {id} already says. Nothing was changed."));
    let steps = call(&mut client, &mut view, "builder", "ask_update", serde_json::json!({"id": id, "steps": ["A", "B"]})).await;
    assert!(steps.starts_with(&format!("Question {id} has no steps")), "{steps}");

    // Changed: the same card, a new version, marked with the reason; still one waiting.
    let args = serde_json::json!({"id": id, "title": "Deploy Thursday?", "wants_text": true, "reason": "QA moved the freeze a day earlier."});
    let said = call(&mut client, &mut view, "builder", "ask_update", args).await;
    assert_eq!(
        said,
        format!(
            "Changed question {id}, \"Deploy Thursday?\": the person's card shows it as it is now, marked updated with your reason, without notifying them again. An answer they give to the version before is refused, so they look again. It can be changed 9 more times."
        )
    );
    let (seq_now, shown) = card(&view, &id);
    assert_eq!(seq_now, seq, "redrawn in place");
    assert_eq!(
        (shown.title.as_str(), shown.body.as_str(), shown.wants_text, shown.revision, shown.update_reason.as_str(), shown.state()),
        ("Deploy Thursday?", "The release is ready.", true, 2, "QA moved the freeze a day earlier.", AskPersonState::Pending)
    );
    assert!(shown.updated_ms > 0);
    assert_eq!(view.agent("builder").waiting_seqs, [seq], "it doesn't count again");
    apply_request(&mut client, &mut view, select("writer")).await;
    assert_eq!(card(&view, &theirs).1.title, "Tone?", "Writer's is untouched");
    apply_request(&mut client, &mut view, select("builder")).await;

    // An answer to the version before is refused, and so is one naming
    // none just after the change (an app before #428).
    assert_eq!(reply_to(&mut client, &mut view, answer(seq, "Yes.", Some(1))).await, Err("This card changed. Look at it again.".to_owned()));
    assert_eq!(reply_to(&mut client, &mut view, answer(seq, "Yes.", None)).await, Err("This card changed. Look at it again.".to_owned()));
    assert_eq!(card(&view, &id).1.state(), AskPersonState::Pending);

    // It's kept across a restart as changed.
    drop(client);
    drop(factory);
    let (addr, _factory) = start(&dir).await;
    let (mut client, workspace, _) = Client::connect(addr).await;
    let mut view = View { workspace, ..View::default() };
    apply_request(&mut client, &mut view, select("builder")).await;
    let (_, kept) = card(&view, &id);
    assert_eq!((kept.title.as_str(), kept.wants_text, kept.revision), ("Deploy Thursday?", true, 2));
    assert_eq!(view.agent("builder").waiting_seqs, [seq]);

    // The latest version is answered.
    assert_eq!(reply_to(&mut client, &mut view, answer(seq, "Yes, Thursday.", Some(2))).await, Ok(()));
    let (_, answered) = card(&view, &id);
    assert_eq!((answered.state(), answered.answer.as_str()), (AskPersonState::Answered, "Yes, Thursday."));
    let late = call(&mut client, &mut view, "builder", "ask_update", serde_json::json!({"id": id, "title": "Deploy Wednesday?"})).await;
    assert_eq!(late, format!("Question {id} isn't waiting: the person already answered it. Nothing was changed."));

    // History has one record, with its last title and how many changes.
    let records = history(&mut client).await;
    let record = records.iter().find(|record| record.kind() == NeedsYouRecordKind::Question).expect("listed");
    assert_eq!((record.title.as_str(), record.updates), ("Deploy Thursday?", 1));
}

#[tokio::test]
async fn a_changed_procedure_keeps_its_steps_done_and_a_card_takes_ten_changes() {
    let dir = temp_dir();
    let (_factory, mut client, mut view) = setup(&dir).await;
    let steps = serde_json::json!({"title": "Set up signing", "body": "So releases are signed.", "steps": ["Open Settings.", "Sign in.", "Add the key."]});
    let id = id_in(&call(&mut client, &mut view, "builder", "ask_person", steps).await);
    let (seq, _) = card(&view, &id);
    let done = |step: u32, revision: u32| {
        client_message::Body::AnswerAsk(v1::AnswerAsk { agent_id: "builder".into(), seq, step: Some(step), revision: Some(revision), ..Default::default() })
    };
    assert_eq!(reply_to(&mut client, &mut view, done(0, 1)).await, Ok(()));
    assert_eq!(reply_to(&mut client, &mut view, done(1, 1)).await, Ok(()));

    // A later step changes: the two done stay done.
    let args = serde_json::json!({"id": id, "steps": ["Open Settings.", "Sign in.", "Add the signing key."]});
    let said = call(&mut client, &mut view, "builder", "ask_update", args).await;
    assert!(said.contains(" Steps 1 to 2 stay done; they see step 3 of 3 next."), "{said}");
    let states = |view: &View| card(view, &id).1.steps.iter().map(v1::AskStep::state).collect::<Vec<_>>();
    assert_eq!(states(&view), [AskStepState::Done, AskStepState::Done, AskStepState::Pending]);

    // A done step changes: from there on they're asked again.
    let args = serde_json::json!({"id": id, "steps": ["Open Settings.", "Sign in with SSO.", "Add the signing key."], "reason": "SSO is required now."});
    let said = call(&mut client, &mut view, "builder", "ask_update", args).await;
    assert!(said.contains(" Step 1 stays done; they see step 2 of 3 next. That's a step they had already done, changed, so they're asked it again."), "{said}");
    assert_eq!(states(&view), [AskStepState::Done, AskStepState::Pending, AskStepState::Pending]);
    // Answering the step on the version before is refused.
    assert_eq!(reply_to(&mut client, &mut view, done(1, 2)).await, Err("This card changed. Look at it again.".to_owned()));
    let wants = call(&mut client, &mut view, "builder", "ask_update", serde_json::json!({"id": id, "wants_text": true})).await;
    assert!(wants.starts_with(&format!("Question {id} is a procedure")), "{wants}");

    // Ten changes in all, then no more.
    for at in 3..=10 {
        let said = call(&mut client, &mut view, "builder", "ask_update", serde_json::json!({"id": id, "body": format!("So releases are signed ({at}).")})).await;
        assert!(said.starts_with(&format!("Changed question {id}")), "{said}");
        if at == 10 {
            assert!(said.ends_with(" That was the last change this card can take."), "{said}");
        }
    }
    assert_eq!(card(&view, &id).1.revision, 11);
    let more = call(&mut client, &mut view, "builder", "ask_update", serde_json::json!({"id": id, "body": "Once more."})).await;
    assert!(more.starts_with(&format!("You've changed question {id} 10 times, the most one card can be changed")), "{more}");
    assert_eq!(reply_to(&mut client, &mut view, done(1, 11)).await, Ok(()));
}

#[tokio::test]
async fn an_agent_changes_its_own_proposal_to_rivet() {
    let dir = temp_dir();
    let (_factory, mut client, mut view) = setup(&dir).await;
    let rivet = v1::UpdateAgent { agent_id: "rivet".into(), model: Some(MODEL.into()), effort: Some("off".into()), ..Default::default() };
    apply_request(&mut client, &mut view, client_message::Body::UpdateAgent(rivet)).await;
    let theirs = id_in(&call(&mut client, &mut view, "writer", "propose_to_rivet", serde_json::json!({"message": "Add an editor."})).await);
    let said = call(&mut client, &mut view, "builder", "propose_to_rivet", serde_json::json!({"message": "Add a backend agent."})).await;
    assert!(said.contains("or change it with proposal_update if what you propose changes."), "{said}");
    let id = id_in(&said);
    let (seq, _) = proposal(&view, &id);

    let other = call(&mut client, &mut view, "builder", "proposal_update", serde_json::json!({"id": theirs, "message": "Mine."})).await;
    assert_eq!(other, format!("You have no proposal {theirs}: proposal_withdraw without id lists yours. Nothing was changed."));
    let said = call(&mut client, &mut view, "builder", "proposal_update", serde_json::json!({"id": id, "message": "Add two backend agents.", "reason": "The queue doubled."})).await;
    assert_eq!(
        said,
        format!(
            "Changed proposal {id}: the person's card shows it as it is now, marked updated with your reason, without notifying them again. Approving the version before is refused, so they look again. It can be changed 9 more times."
        )
    );
    let (seq_now, shown) = proposal(&view, &id);
    assert_eq!(seq_now, seq);
    assert_eq!((shown.text.as_str(), shown.revision, shown.update_reason.as_str()), ("Add two backend agents.", 2, "The queue doubled."));
    view.follow(&mut client, |view| view.agent("builder").waiting_seqs == [seq]).await;

    let decide = |revision: Option<u32>| {
        client_message::Body::DecideProposal(v1::DecideProposal { agent_id: "builder".into(), proposal_id: id.clone(), approve: true, revision, ..Default::default() })
    };
    assert_eq!(reply_to(&mut client, &mut view, decide(Some(1))).await, Err("This card changed. Look at it again.".to_owned()));
    assert_eq!(reply_to(&mut client, &mut view, decide(None)).await, Err("This card changed. Look at it again.".to_owned()));
    assert_eq!(reply_to(&mut client, &mut view, decide(Some(2))).await, Ok(()));
    assert_eq!(proposal(&view, &id).1.state(), ProposalState::Approved);

    let records = history(&mut client).await;
    let record = records.iter().find(|record| record.kind() == NeedsYouRecordKind::Proposal).expect("listed");
    assert_eq!((record.title.as_str(), record.updates), ("Add two backend agents.", 1));
}

#[tokio::test]
async fn a_review_card_follows_its_file_and_an_approval_of_the_file_before_is_refused() {
    let dir = temp_dir();
    let (factory, mut client, mut view) = setup(&dir).await;
    factoryd::review_watch::set_times_for_test(&factory, std::time::Duration::from_millis(50), std::time::Duration::from_millis(400));
    let file = dir.join("site/report.md");
    std::fs::write(&file, "# Report\n\nFirst draft.\n").expect("the file");
    let said = call(&mut client, &mut view, "builder", "ask_person", serde_json::json!({"title": "Review the report", "body": "B", "review": "report.md"})).await;
    let id = id_in(&said);
    let (seq, asked) = card(&view, &id);
    assert_eq!(asked.revision, 1);
    let settle = || tokio::time::sleep(std::time::Duration::from_millis(1200));

    // Saved again unchanged: nothing marks it.
    std::fs::write(&file, "# Report\n\nFirst draft.\n").expect("saved again");
    settle().await;
    let shown = client.ping(b"after the same save").await;
    assert!(!format!("{shown:?}").contains("This file changed"), "marked for the same content");
    assert_eq!(card(&view, &id).1.revision, 1);

    // Saved ten times in a burst: marked once, when it went quiet, in
    // Claktory's words, not the agent's.
    for at in 0..10 {
        std::fs::write(&file, format!("# Report\n\nDraft {at}.\n")).expect("saved");
        tokio::time::sleep(std::time::Duration::from_millis(60)).await;
    }
    view.follow(&mut client, |view| card(view, &id).1.revision == 2).await;
    settle().await;
    let _ = client.ping(b"after the burst").await;
    let (seq_now, marked) = card(&view, &id);
    assert_eq!(seq_now, seq, "redrawn in place");
    assert_eq!(
        (marked.revision, marked.update_note.as_str(), marked.update_reason.as_str(), marked.state()),
        (2, "This file changed since Builder asked. Approving means it as it reads now.", "", AskPersonState::Pending)
    );
    assert!(marked.updated_ms > 0);
    assert_eq!(view.agent("builder").waiting_seqs, [seq], "it doesn't count again");

    // Its agent's own changes still count from ten: a file change isn't one.
    let said = call(&mut client, &mut view, "builder", "ask_update", serde_json::json!({"id": id, "body": "The second draft."})).await;
    assert!(said.ends_with(" It can be changed 9 more times."), "{said}");
    let (_, updated) = card(&view, &id);
    assert_eq!((updated.revision, updated.update_note.as_str()), (3, ""), "the agent's change is its own");

    // An approval of the file before is refused; of the file now, kept.
    assert_eq!(reply_to(&mut client, &mut view, answer(seq, "", Some(2))).await, Err("This card changed. Look at it again.".to_owned()));
    std::fs::write(&file, "# Report\n\nFinal.\n").expect("saved");
    view.follow(&mut client, |view| card(view, &id).1.revision == 4).await;
    assert_eq!(reply_to(&mut client, &mut view, answer(seq, "", Some(3))).await, Err("This card changed. Look at it again.".to_owned()));
    assert_eq!(reply_to(&mut client, &mut view, answer(seq, "", Some(4))).await, Ok(()));
    // Its agent was told, quietly, with the answer.
    let told = |view: &View| {
        view.entries.iter().any(|entry| matches!(&entry.kind, Some(Kind::Event(event)) if event.text.contains("changed while question") && event.text.contains("reviews it as it reads now")))
    };
    view.follow(&mut client, |view| told(view) && idle(view, "builder")).await;

    // Answered, it follows the file no more.
    std::fs::write(&file, "# Report\n\nAfter.\n").expect("saved");
    settle().await;
    let _ = client.ping(b"after the answer").await;
    assert_eq!(card(&view, &id).1.revision, 4);
}
