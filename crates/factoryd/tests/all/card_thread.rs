//! A conversation on a card (#430). The person answers a card with words
//! (a reply, or a comment on approving or dismissing it); the agent is told
//! where they answered from and to answer on the card (ask_reply), which
//! keeps the talk on it (`thread`) and opens it again in Needs you as a new
//! version, notifying as a new question does. The person answers it, replies
//! again, or ends it with Resolve, without a decision. Never on a card whose
//! approval makes a change.

use crate::common::{Client, View, apply_request, create_agent, create_project, send_message, start, temp_dir};
use factory_protocol::v1::{self, AgentStatus, AskAnsweredFrom, AskPersonState, AskStepState, NeedsYouOutcome, client_message, entry::Kind, server_message::Body};

fn select(agent: &str) -> client_message::Body {
    client_message::Body::SelectAgent(v1::SelectAgent { agent_id: agent.into() })
}

fn idle(view: &View, agent: &str) -> bool {
    let agent = view.agent(agent);
    agent.status() == AgentStatus::Idle && agent.queued_messages == 0
}

async fn call(client: &mut Client, view: &mut View, tool: &str, args: serde_json::Value) -> String {
    apply_request(client, view, select("builder")).await;
    let before = view.entries.len();
    apply_request(client, view, send_message("builder", &format!("call {tool} {args}"))).await;
    let output = |view: &View| {
        view.entries.get(before..).unwrap_or_default().iter().find_map(|entry| match &entry.kind {
            Some(Kind::ToolCall(call)) if call.name == tool && !call.output.is_empty() => Some(call.output.clone()),
            _ => None,
        })
    };
    view.follow(client, |view| output(view).is_some() && idle(view, "builder")).await;
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

fn id_in(said: &str) -> String {
    let (_, rest) = said.split_once("Its id is ").expect("an id");
    rest.split(':').next().expect("the id").to_owned()
}

/// Claktory's events Builder got, oldest first.
fn events(view: &View) -> Vec<(String, bool)> {
    view.entries
        .iter()
        .filter_map(|entry| match &entry.kind {
            Some(Kind::Event(event)) if event.source() == v1::EventSource::Factory => Some((event.text.clone(), event.quiet)),
            _ => None,
        })
        .collect()
}

async fn answered(client: &mut Client, view: &mut View, request: v1::AnswerAsk) -> Result<(), String> {
    match apply_request(client, view, client_message::Body::AnswerAsk(request)).await {
        Body::Ack(_) => Ok(()),
        Body::Error(error) => Err(error.message),
        other => panic!("expected an ack or a refusal, got {other:?}"),
    }
}

#[tokio::test]
async fn the_person_and_the_agent_talk_on_a_card_until_it_is_resolved() {
    let dir = temp_dir();
    let (addr, _factory) = start(&dir).await;
    let (mut client, workspace, _) = Client::connect(addr).await;
    let mut view = View { workspace, ..View::default() };
    apply_request(&mut client, &mut view, create_project("Site", &dir.join("site"))).await;
    apply_request(&mut client, &mut view, create_agent("site", "Builder", "")).await;

    let id = id_in(&call(&mut client, &mut view, "ask_person", serde_json::json!({"title": "Deploy Friday?", "body": "The release is ready."})).await);
    let (seq, _) = card(&view, &id);
    // Nothing of theirs to answer yet.
    let early = call(&mut client, &mut view, "ask_reply", serde_json::json!({"id": id, "text": "Hello."})).await;
    assert!(early.starts_with(&format!("Question {id} still waits for the person")), "{early}");

    // They reply from Needs you with a question: the agent is told where,
    // and to answer on the card.
    let question = v1::AnswerAsk {
        agent_id: "builder".into(),
        seq,
        reply: true,
        text: "Is QA done?".into(),
        revision: Some(1),
        from: AskAnsweredFrom::NeedsYou.into(),
        ..Default::default()
    };
    assert_eq!(answered(&mut client, &mut view, question).await, Ok(()));
    view.follow(&mut client, |view| events(view).len() == 1 && idle(view, "builder")).await;
    let (told, _) = events(&view).pop().expect("told");
    assert!(told.contains("Is QA done?"), "{told}");
    assert!(
        told.contains(&format!(
            "They answered from Needs you, not in your conversation. If what they wrote needs an answer from you, reply on the card with ask_reply (id {id}) so it reaches them where they are, rather than in your chat."
        )),
        "{told}"
    );
    assert!(view.agent("builder").waiting_seqs.is_empty());

    // The agent answers on the card: the talk shows, and it waits again, as a new version.
    let said = call(&mut client, &mut view, "ask_reply", serde_json::json!({"id": id, "text": "Yes, QA signed off this morning.", "title": "Deploy Friday? QA is done"})).await;
    assert!(said.starts_with(&format!("Answered on question {id}, \"Deploy Friday? QA is done\": it's open again in the person's Needs you")), "{said}");
    let (seq_now, open) = card(&view, &id);
    assert_eq!(seq_now, seq);
    assert_eq!((open.state(), open.revision, open.title.as_str(), open.answer.as_str(), open.decided_ms), (AskPersonState::Pending, 2, "Deploy Friday? QA is done", "", 0));
    let thread: Vec<_> = open.thread.iter().map(|message| (message.from_agent, message.text.as_str())).collect();
    assert_eq!(thread, [(false, "Is QA done?"), (true, "Yes, QA signed off this morning.")]);
    assert!(open.thread[1].at_ms >= open.thread[0].at_ms && open.thread[0].at_ms > 0);
    view.follow(&mut client, |view| view.agent("builder").waiting_seqs == [seq]).await;
    // Once only: it's their turn now.
    let twice = call(&mut client, &mut view, "ask_reply", serde_json::json!({"id": id, "text": "Also…"})).await;
    assert!(twice.starts_with(&format!("Question {id} still waits")), "{twice}");

    // Resolve needs the revision they saw, and ends it with no decision.
    let resolve = |revision: u32, text: &str| v1::AnswerAsk {
        agent_id: "builder".into(),
        seq,
        resolve: true,
        text: text.into(),
        revision: Some(revision),
        ..Default::default()
    };
    assert_eq!(answered(&mut client, &mut view, resolve(1, "")).await, Err("This card changed. Look at it again.".to_owned()));
    let events_before = events(&view).len();
    assert_eq!(answered(&mut client, &mut view, resolve(2, "")).await, Ok(()));
    let (_, ended) = card(&view, &id);
    assert_eq!((ended.state(), ended.resolved, ended.replied, ended.thread.len()), (AskPersonState::Answered, true, false, 2));
    view.follow(&mut client, |view| view.agent("builder").quiet_waiting == 1 && view.agent("builder").waiting_seqs.is_empty()).await;
    apply_request(&mut client, &mut view, send_message("builder", "Carry on.")).await;
    view.follow(&mut client, |view| events(view).len() > events_before && idle(view, "builder")).await;
    let (told, quiet) = events(&view).pop().expect("told");
    assert!(quiet, "a bare Resolve is told quietly");
    assert!(told.starts_with("The person resolved your question \"Deploy Friday? QA is done\" on its card: you talked it through"), "{told}");
    let over = call(&mut client, &mut view, "ask_reply", serde_json::json!({"id": id, "text": "Thanks."})).await;
    assert!(over.starts_with(&format!("The person resolved question {id}")), "{over}");

    // History: one record, resolved.
    let list = client_message::Body::ListNeedsYouHistory(v1::ListNeedsYouHistory { query: String::new(), before_ms: 0, limit: 0 });
    let Body::NeedsYouHistory(history) = client.request(list).await.body else { panic!("history") };
    let record = history.records.iter().find(|record| record.title == "Deploy Friday? QA is done").expect("listed");
    assert_eq!((record.outcome(), record.updates), (NeedsYouOutcome::Resolved, 1));

    // A card without a thread can't be resolved; one answered without words can't be answered on.
    let plain = id_in(&call(&mut client, &mut view, "ask_person", serde_json::json!({"title": "Ship it?", "body": "B"})).await);
    let (plain_seq, _) = card(&view, &plain);
    let bare = v1::AnswerAsk { agent_id: "builder".into(), seq: plain_seq, resolve: true, revision: Some(1), ..Default::default() };
    assert_eq!(
        answered(&mut client, &mut view, bare).await,
        Err("Only a card talked over with its agent can be resolved: answer or dismiss it.".to_owned())
    );
    let yes = v1::AnswerAsk { agent_id: "builder".into(), seq: plain_seq, revision: Some(1), ..Default::default() };
    assert_eq!(answered(&mut client, &mut view, yes).await, Ok(()));
    let wordless = call(&mut client, &mut view, "ask_reply", serde_json::json!({"id": plain, "text": "Great."})).await;
    assert!(wordless.starts_with(&format!("The person answered question {plain} without words")), "{wordless}");
}

#[tokio::test]
async fn a_procedure_they_couldnt_finish_is_talked_over_and_asked_again() {
    let dir = temp_dir();
    let (addr, _factory) = start(&dir).await;
    let (mut client, workspace, _) = Client::connect(addr).await;
    let mut view = View { workspace, ..View::default() };
    apply_request(&mut client, &mut view, create_project("Site", &dir.join("site"))).await;
    apply_request(&mut client, &mut view, create_agent("site", "Builder", "")).await;
    let steps = serde_json::json!({"title": "Set up signing", "body": "So releases are signed.", "steps": ["Open Settings.", "Add the key."]});
    let id = id_in(&call(&mut client, &mut view, "ask_person", steps).await);
    let (seq, _) = card(&view, &id);
    let step = |at: u32, cant: bool, text: &str, revision: u32| v1::AnswerAsk {
        agent_id: "builder".into(),
        seq,
        step: Some(at),
        dismiss: cant,
        text: text.into(),
        revision: Some(revision),
        from: AskAnsweredFrom::Conversation.into(),
        ..Default::default()
    };
    assert_eq!(answered(&mut client, &mut view, step(0, false, "", 1)).await, Ok(()));
    assert_eq!(answered(&mut client, &mut view, step(1, true, "Which key? There are two.", 1)).await, Ok(()));
    view.follow(&mut client, |view| events(view).len() == 2 && idle(view, "builder")).await;
    let (told, _) = events(&view).pop().expect("told");
    assert!(told.contains("They answered on the card in your conversation. If what they wrote needs an answer from you, reply on the card with ask_reply"), "{told}");
    // Before "You asked", as in the other events.
    assert!(told.contains("rather than in your chat.\n\nYou asked: Set up signing"), "{told}");

    // Answered on, the step they couldn't do is asked again; the one done stays done.
    let said = call(&mut client, &mut view, "ask_reply", serde_json::json!({"id": id, "text": "The one named release."})).await;
    assert!(said.starts_with(&format!("Answered on question {id}")), "{said}");
    let (_, open) = card(&view, &id);
    let states: Vec<_> = open.steps.iter().map(v1::AskStep::state).collect();
    assert_eq!((open.state(), states), (AskPersonState::Pending, vec![AskStepState::Done, AskStepState::Pending]));
    // A reply on it answers the agent again, rather than the step.
    let again = v1::AnswerAsk { agent_id: "builder".into(), seq, reply: true, text: "Found it, thanks.".into(), revision: Some(2), ..Default::default() };
    assert_eq!(answered(&mut client, &mut view, again).await, Ok(()));
    assert!(card(&view, &id).1.replied);
}

#[tokio::test]
async fn a_comment_on_a_step_is_answered_on_the_card_while_it_waits() {
    let dir = temp_dir();
    let (addr, _factory) = start(&dir).await;
    let (mut client, workspace, _) = Client::connect_as(addr, v1::DeviceKind::Desktop).await;
    let mut view = View { workspace, ..View::default() };
    apply_request(&mut client, &mut view, create_project("Site", &dir.join("site"))).await;
    apply_request(&mut client, &mut view, create_agent("site", "Builder", "")).await;
    let mut notes = Vec::new();
    let steps = serde_json::json!({"title": "Rotate the key", "body": "The old one leaked.", "steps": ["Open Keychain.", "Delete the old key.", "Add the new one."]});
    let id = id_in(&crate::notify::call(&mut client, &mut view, &mut notes, "builder", "ask_person", steps).await);
    let (seq, _) = card(&view, &id);
    let step = |at: u32, text: &str, revision: u32, from: AskAnsweredFrom| v1::AnswerAsk {
        agent_id: "builder".into(),
        seq,
        step: Some(at),
        text: text.into(),
        revision: Some(revision),
        from: from.into(),
        ..Default::default()
    };

    // Done with a comment: the agent is told where, and to answer on the card.
    assert_eq!(answered(&mut client, &mut view, step(0, "Done, but it asked me for a password.", 1, AskAnsweredFrom::NeedsYou)).await, Ok(()));
    view.follow(&mut client, |view| events(view).len() == 1 && idle(view, "builder")).await;
    let (told, _) = events(&view).pop().expect("told");
    assert!(
        told.contains(&format!(
            "Done, but it asked me for a password.\n\nThey answered from Needs you, not in your conversation. If what they wrote needs an answer from you, reply on the card with ask_reply (id {id}) so it reaches them where they are, rather than in your chat.\n\nThey see step 2 next"
        )),
        "{told}"
    );

    // The answer goes on the card, which still waits at its step, and notifies.
    notes.clear();
    let said =
        crate::notify::call(&mut client, &mut view, &mut notes, "builder", "ask_reply", serde_json::json!({"id": id, "text": "That's your login password."})).await;
    assert!(
        said.starts_with(&format!("Answered on question {id}, \"Rotate the key\": your answer shows under their comment, on the card that still waits at step 2 of 3")),
        "{said}"
    );
    let (_, open) = card(&view, &id);
    let states: Vec<_> = open.steps.iter().map(v1::AskStep::state).collect();
    assert_eq!((open.state(), open.revision, states), (AskPersonState::Pending, 2, vec![AskStepState::Done, AskStepState::Pending, AskStepState::Pending]));
    let thread: Vec<_> = open.thread.iter().map(|message| (message.from_agent, message.text.as_str())).collect();
    assert_eq!(thread, [(false, "Done, but it asked me for a password."), (true, "That's your login password.")]);
    assert!(open.thread[0].at_ms > 0 && open.thread[1].at_ms >= open.thread[0].at_ms);
    assert_eq!(view.agent("builder").waiting_seqs, [seq]);
    let needs: Vec<_> = notes.iter().filter(|note| note.kind() == v1::NotifyKind::NeedsYou).map(|note| note.seq).collect();
    assert_eq!(needs, [seq], "{notes:?}");
    // Once only: it's their turn now.
    let twice = call(&mut client, &mut view, "ask_reply", serde_json::json!({"id": id, "text": "Also…"})).await;
    assert!(twice.starts_with(&format!("Question {id} still waits")), "{twice}");

    // The next step, done without words: nothing to answer.
    assert_eq!(answered(&mut client, &mut view, step(1, "", 2, AskAnsweredFrom::Conversation)).await, Ok(()));
    view.follow(&mut client, |view| events(view).len() == 2 && idle(view, "builder")).await;
    let (told, _) = events(&view).pop().expect("told");
    assert!(told.contains("They answered on the card in your conversation.\n\nThey see step 3 next") && !told.contains("ask_reply"), "{told}");
    let wordless = call(&mut client, &mut view, "ask_reply", serde_json::json!({"id": id, "text": "Good."})).await;
    assert!(wordless.starts_with(&format!("Question {id} still waits")), "{wordless}");
}
