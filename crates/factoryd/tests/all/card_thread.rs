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
            "They answered from Needs you, not in your conversation. If what they wrote needs an answer from you, reply in the request with ask_reply (id {id}) so it reaches them where they are, rather than in your chat."
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
    assert!(told.starts_with("The person resolved your question \"Deploy Friday? QA is done\" in its request: you talked it through"), "{told}");
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
        Err("Only a request talked over with its agent can be resolved: answer or dismiss it.".to_owned())
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
    assert!(told.contains("They answered in the request in your conversation. If what they wrote needs an answer from you, reply in the request with ask_reply"), "{told}");
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

/// The steps' talk, as the card keeps it: who, what, and the step it's on.
fn talk_steps(dir: &std::path::Path, id: &str) -> Vec<(bool, String, Option<u64>)> {
    let db = rusqlite::Connection::open(dir.join("home/factory.db")).expect("factory.db");
    let talk: String = db.query_row("SELECT talk FROM asks WHERE id = ?1", [id], |row| row.get(0)).expect("talk");
    let talk: Vec<serde_json::Value> = serde_json::from_str(&talk).expect("json");
    talk.iter()
        .map(|turn| (turn["agent"].as_bool().unwrap_or(false), turn["text"].as_str().unwrap_or("").to_owned(), turn["step"].as_u64()))
        .collect()
}

/// #533: words sent on a step leave it waiting, with nothing marked done;
/// the card waits on its agent, which fixes the step or answers, and it
/// opens again at that step. Can't do this is fixed on the same card too.
#[tokio::test]
async fn words_on_a_step_leave_it_waiting_for_the_agent_to_fix_or_answer() {
    let dir = temp_dir();
    let (addr, _factory) = start(&dir).await;
    let (mut client, workspace, _) = Client::connect(addr).await;
    let mut view = View { workspace, ..View::default() };
    apply_request(&mut client, &mut view, create_project("Site", &dir.join("site"))).await;
    apply_request(&mut client, &mut view, create_agent("site", "Builder", "")).await;
    let steps = serde_json::json!({"title": "Rotate the key", "body": "The old one leaked.", "steps": ["Open Keychain.", "Delete the old key.", "Add the new one."]});
    let id = id_in(&call(&mut client, &mut view, "ask_person", steps).await);
    let (seq, _) = card(&view, &id);
    let done = |at: u32, revision: u32| v1::AnswerAsk { agent_id: "builder".into(), seq, step: Some(at), revision: Some(revision), ..Default::default() };
    let send = |text: &str, revision: u32| v1::AnswerAsk {
        agent_id: "builder".into(),
        seq,
        reply: true,
        text: text.into(),
        revision: Some(revision),
        ..Default::default()
    };
    assert_eq!(answered(&mut client, &mut view, done(0, 1)).await, Ok(()));
    view.follow(&mut client, |view| events(view).len() == 1 && idle(view, "builder")).await;

    // Said of a step that isn't the one waiting: refused.
    let elsewhere = v1::AnswerAsk { step: Some(2), ..send("Wrong one.", 1) };
    assert_eq!(answered(&mut client, &mut view, elsewhere).await, Err("That isn't the step waiting: look at the request again.".to_owned()));

    // Sent on step 2, with no talk yet: nothing marked, and it waits on Builder.
    assert_eq!(answered(&mut client, &mut view, send("The menu isn't there.", 1)).await, Ok(()));
    let (_, sent) = card(&view, &id);
    let states: Vec<_> = sent.steps.iter().map(v1::AskStep::state).collect();
    assert_eq!((sent.state(), sent.replied, states), (AskPersonState::Answered, true, vec![AskStepState::Done, AskStepState::Pending, AskStepState::Pending]));
    assert!(view.agent("builder").waiting_seqs.is_empty(), "it waits on its agent now");
    view.follow(&mut client, |view| events(view).len() == 2 && idle(view, "builder")).await;
    let (told, quiet) = events(&view).pop().expect("told");
    assert!(!quiet);
    assert!(
        told.starts_with(&format!(
            "The person replied on step 2 of 3 of \"Rotate the key\": Delete the old key.\n\nTheir words:\n\nThe menu isn't there.\n\nStep 2 still waits; nothing was marked done. If the step is wrong, fix it in the request with ask_reply (id {id}, steps: the steps, step 2 rewritten) and a short answer; if it only needs an answer, ask_reply with text. They're shown step 2 again either way."
        )),
        "{told}"
    );

    // Fixed with steps: open again at step 2, rewritten, the talk on it.
    let fixed = serde_json::json!({"id": id, "text": "It moved to the sidebar.", "steps": ["Open Keychain.", "Delete the old key from the sidebar.", "Add the new one."]});
    let said = call(&mut client, &mut view, "ask_reply", fixed).await;
    assert!(said.starts_with(&format!("Answered on question {id}, \"Rotate the key\": it's open again in the person's Needs you at step 2 of 3")), "{said}");
    let (_, open) = card(&view, &id);
    let states: Vec<_> = open.steps.iter().map(v1::AskStep::state).collect();
    assert_eq!((open.state(), states), (AskPersonState::Pending, vec![AskStepState::Done, AskStepState::Pending, AskStepState::Pending]));
    assert_eq!(open.steps[1].text, "Delete the old key from the sidebar.");
    assert_eq!(open.thread.iter().map(|turn| turn.step).collect::<Vec<_>>(), [2, 2], "each turn says its step");
    assert_eq!(view.agent("builder").waiting_seqs, [seq]);
    assert_eq!(
        talk_steps(&dir, &id),
        [(false, "The menu isn't there.".to_owned(), Some(2)), (true, "It moved to the sidebar.".to_owned(), Some(2))]
    );

    // Answered with words only: open again at the same step, unchanged.
    assert_eq!(answered(&mut client, &mut view, send("Which sidebar?", open.revision)).await, Ok(()));
    view.follow(&mut client, |view| events(view).len() == 3 && idle(view, "builder")).await;
    call(&mut client, &mut view, "ask_reply", serde_json::json!({"id": id, "text": "Keychain Access's, on the left."})).await;
    let (_, open) = card(&view, &id);
    assert_eq!((open.state(), open.steps[1].text.as_str(), open.steps[1].state()), (AskPersonState::Pending, "Delete the old key from the sidebar.", AskStepState::Pending));
    // And Done goes on as ever.
    assert_eq!(answered(&mut client, &mut view, done(1, open.revision)).await, Ok(()));
    view.follow(&mut client, |view| events(view).len() == 4 && idle(view, "builder")).await;
    assert!(events(&view).pop().expect("told").0.starts_with("The person did step 2 of 3"));

    // Can't do this, without words: fixed on the same card, at that step.
    let cant = v1::AnswerAsk { dismiss: true, ..done(2, card(&view, &id).1.revision) };
    assert_eq!(answered(&mut client, &mut view, cant).await, Ok(()));
    view.follow(&mut client, |view| events(view).len() == 5 && idle(view, "builder")).await;
    let (told, _) = events(&view).pop().expect("told");
    assert!(told.contains(&format!("If they can't, find a way without them and rewrite the steps in this request: ask_reply (id {id}) with steps, step 3 rewritten")), "{told}");
    let again = serde_json::json!({"id": id, "text": "Use the new key from 1Password.", "steps": ["Open Keychain.", "Delete the old key from the sidebar.", "Add the new key from 1Password."]});
    let said = call(&mut client, &mut view, "ask_reply", again).await;
    assert!(said.contains("at step 3 of 3"), "{said}");
    let (_, open) = card(&view, &id);
    let states: Vec<_> = open.steps.iter().map(v1::AskStep::state).collect();
    assert_eq!((open.state(), states), (AskPersonState::Pending, vec![AskStepState::Done, AskStepState::Done, AskStepState::Pending]));
    assert_eq!(talk_steps(&dir, &id).last().cloned(), Some((true, "Use the new key from 1Password.".to_owned(), Some(3))));

    // Can't do this again, and this time the agent stops: the card stays
    // as it was, stopped at step 3 for good, the agent's words on it.
    let cant = v1::AnswerAsk { dismiss: true, ..done(2, card(&view, &id).1.revision) };
    assert_eq!(answered(&mut client, &mut view, cant).await, Ok(()));
    view.follow(&mut client, |view| events(view).len() == 6 && idle(view, "builder")).await;
    let stop = serde_json::json!({"id": id, "text": "I'll do it from the server instead.", "resolve": true});
    let said = call(&mut client, &mut view, "ask_reply", stop.clone()).await;
    assert!(said.starts_with(&format!("Stopped question {id}, \"Rotate the key\", at step 3 of 3, for good")), "{said}");
    let (_, stopped) = card(&view, &id);
    let states: Vec<_> = stopped.steps.iter().map(v1::AskStep::state).collect();
    assert_eq!((stopped.state(), stopped.resolved), (AskPersonState::Dismissed, true));
    assert_eq!(states, [AskStepState::Done, AskStepState::Done, AskStepState::Cant]);
    assert_eq!(talk_steps(&dir, &id).last().cloned(), Some((true, "I'll do it from the server instead.".to_owned(), Some(3))));
    // Over: neither stopped nor opened again a second time, and it was the
    // agent that stopped it (Abbacchio's NIT 1580).
    let again = call(&mut client, &mut view, "ask_reply", stop).await;
    assert!(again.starts_with(&format!("You closed question {id}: it's over")), "{again}");
    let reopen = serde_json::json!({"id": id, "text": "Try again.", "steps": ["Open Keychain.", "Delete the old key from the sidebar.", "Add it."]});
    let again = call(&mut client, &mut view, "ask_reply", reopen).await;
    assert!(again.starts_with(&format!("You closed question {id}: it's over")), "{again}");
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
            "Done, but it asked me for a password.\n\nThey answered from Needs you, not in your conversation. If what they wrote needs an answer from you, reply in the request with ask_reply (id {id}) so it reaches them where they are, rather than in your chat.\n\nThey see step 2 next"
        )),
        "{told}"
    );

    // The answer goes on the card, which still waits at its step, and notifies.
    notes.clear();
    let said =
        crate::notify::call(&mut client, &mut view, &mut notes, "builder", "ask_reply", serde_json::json!({"id": id, "text": "That's your login password."})).await;
    assert!(
        said.starts_with(&format!("Answered on question {id}, \"Rotate the key\": your answer shows under their comment, in the request that still waits at step 2 of 3")),
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
    assert!(told.contains("They answered in the request in your conversation.\n\nThey see step 3 next") && !told.contains("ask_reply"), "{told}");
    let wordless = call(&mut client, &mut view, "ask_reply", serde_json::json!({"id": id, "text": "Good."})).await;
    assert!(wordless.starts_with(&format!("Question {id} still waits")), "{wordless}");
}

#[tokio::test]
async fn approving_after_the_agents_reply_agrees_with_where_the_talk_ended() {
    let dir = temp_dir();
    let (addr, _factory) = start(&dir).await;
    let (mut client, workspace, _) = Client::connect(addr).await;
    let mut view = View { workspace, ..View::default() };
    apply_request(&mut client, &mut view, create_project("Site", &dir.join("site"))).await;
    apply_request(&mut client, &mut view, create_agent("site", "Builder", "")).await;
    let answer = |seq: u64, revision: u32, talk_seen: Option<u32>| v1::AnswerAsk {
        agent_id: "builder".into(),
        seq,
        revision: Some(revision),
        talk_seen,
        ..Default::default()
    };

    // Lisa's card 182 (Bruno 35010): the person turns down the paid plan in
    // a reply, and the agent answers with a free one, leaving the card as it was.
    let paid = serde_json::json!({"title": "OK to spend up to $20 on Pangram?", "body": "A paid detector."});
    let id = id_in(&call(&mut client, &mut view, "ask_person", paid).await);
    let (seq, asked) = card(&view, &id);
    assert!(!asked.approve_follows_talk, "no talk yet");
    let no = v1::AnswerAsk { agent_id: "builder".into(), seq, reply: true, text: "No paid detectors.".into(), revision: Some(1), ..Default::default() };
    assert_eq!(answered(&mut client, &mut view, no).await, Ok(()));
    view.follow(&mut client, |view| events(view).len() == 1 && idle(view, "builder")).await;
    call(&mut client, &mut view, "ask_reply", serde_json::json!({"id": id, "text": "Then GPTZero's free plan, at no cost."})).await;
    let (_, open) = card(&view, &id);
    assert_eq!((open.revision, open.title.as_str(), open.thread.len()), (2, "OK to spend up to $20 on Pangram?", 2));
    assert!(open.approve_follows_talk, "approving it now agrees with the reply");

    // An approval made without seeing the reply is refused; dismissing never is.
    // From the version before the reply too, which the reply replaced: it's
    // the reply they're told about (row 1268); an app that doesn't count
    // the talk is told the card changed.
    assert_eq!(
        answered(&mut client, &mut view, answer(seq, 1, Some(0))).await,
        Err("Builder replied in this request since you looked at it. Read the reply, then answer again.".to_owned())
    );
    assert_eq!(answered(&mut client, &mut view, answer(seq, 1, None)).await, Err("This card changed. Look at it again.".to_owned()));
    assert_eq!(
        answered(&mut client, &mut view, answer(seq, 2, Some(0))).await,
        Err("Builder replied in this request since you looked at it. Read the reply, then answer again.".to_owned())
    );
    assert_eq!(card(&view, &id).1.state(), AskPersonState::Pending);
    // Approved having seen it: the agent is told the talk, fenced, and to
    // act on where it ended rather than on the card's $20.
    assert_eq!(answered(&mut client, &mut view, answer(seq, 2, Some(2))).await, Ok(()));
    view.follow(&mut client, |view| events(view).len() == 2 && idle(view, "builder")).await;
    let (told, quiet) = events(&view).pop().expect("told");
    assert!(!quiet);
    assert!(
        told.starts_with("The person approved your question \"OK to spend up to $20 on Pangram?\" after your last reply in its request, without the request changing: they agreed to where your talk there ended, not only to the request's words."),
        "{told}"
    );
    assert!(
        told.contains("The talk in the request, oldest first:\n\n```\nThe person: No paid detectors.\n\nYou: Then GPTZero's free plan, at no cost.\n```"),
        "{told}"
    );
    assert!(!told.contains("yes, go ahead"), "{told}");
    assert!(!card(&view, &id).1.approve_follows_talk, "decided");

    // A reply that changes what's asked gives the card its new words: the
    // card is the ask, and Approve means it as it reads.
    let paid = serde_json::json!({"title": "OK to spend $5 on hosting?", "body": "A paid plan."});
    let id = id_in(&call(&mut client, &mut view, "ask_person", paid).await);
    let (seq, _) = card(&view, &id);
    let no = v1::AnswerAsk { agent_id: "builder".into(), seq, reply: true, text: "Only free plans.".into(), revision: Some(1), ..Default::default() };
    assert_eq!(answered(&mut client, &mut view, no).await, Ok(()));
    view.follow(&mut client, |view| events(view).len() == 3 && idle(view, "builder")).await;
    let changed = serde_json::json!({"id": id, "text": "Free tier then.", "title": "OK to use the free hosting tier?", "body": "No cost."});
    call(&mut client, &mut view, "ask_reply", changed).await;
    let (_, open) = card(&view, &id);
    assert_eq!((open.title.as_str(), open.body.as_str()), ("OK to use the free hosting tier?", "No cost."));
    assert!(!open.approve_follows_talk, "the request itself asks it now");
    // Dismissed with a stale count: not refused.
    let dismiss = v1::AnswerAsk { dismiss: true, text: "Not now.".into(), ..answer(seq, 2, Some(0)) };
    assert_eq!(answered(&mut client, &mut view, dismiss).await, Ok(()));
}

#[tokio::test]
async fn the_agent_closes_a_card_the_talk_settled_with_its_last_words() {
    let dir = temp_dir();
    let (addr, _factory) = start(&dir).await;
    let (mut client, workspace, _) = Client::connect(addr).await;
    let mut view = View { workspace, ..View::default() };
    apply_request(&mut client, &mut view, create_project("Site", &dir.join("site"))).await;
    apply_request(&mut client, &mut view, create_agent("site", "Builder", "")).await;
    let reply = |seq: u64, text: &str, revision: u32| v1::AnswerAsk {
        agent_id: "builder".into(),
        seq,
        reply: true,
        text: text.into(),
        revision: Some(revision),
        ..Default::default()
    };

    // Their reply settles it (Bruno's card 186): the agent closes it with its
    // words, and it leaves Needs you instead of waiting again.
    let id = id_in(&call(&mut client, &mut view, "ask_person", serde_json::json!({"title": "Deploy to production?", "body": "Ready."})).await);
    let (seq, _) = card(&view, &id);
    assert_eq!(answered(&mut client, &mut view, reply(seq, "Staging first, then production tomorrow.", 1)).await, Ok(()));
    view.follow(&mut client, |view| events(view).len() == 1 && idle(view, "builder")).await;
    let said = call(&mut client, &mut view, "ask_reply", serde_json::json!({"id": id, "text": "Staging today, production tomorrow.", "resolve": true})).await;
    assert!(said.starts_with(&format!("Closed question {id}, \"Deploy to production?\", as settled in the talk")), "{said}");
    let (_, closed) = card(&view, &id);
    assert_eq!((closed.state(), closed.resolved, closed.replied, closed.answer.as_str(), closed.revision), (AskPersonState::Answered, true, false, "", 1));
    let thread: Vec<_> = closed.thread.iter().map(|message| (message.from_agent, message.text.as_str())).collect();
    assert_eq!(thread, [(false, "Staging first, then production tomorrow."), (true, "Staging today, production tomorrow.")]);
    assert!(closed.withdraw_reason.is_empty() && !closed.approve_follows_talk);
    assert!(view.agent("builder").waiting_seqs.is_empty());
    let list = client_message::Body::ListNeedsYouHistory(v1::ListNeedsYouHistory { query: String::new(), before_ms: 0, limit: 0 });
    let Body::NeedsYouHistory(history) = client.request(list).await.body else { panic!("history") };
    let record = history.records.iter().find(|record| record.title == "Deploy to production?").expect("listed");
    assert_eq!((record.outcome(), record.agent_reason.as_str(), record.reply.as_str()), (NeedsYouOutcome::Resolved, "Staging today, production tomorrow.", ""));
    let again = call(&mut client, &mut view, "ask_reply", serde_json::json!({"id": id, "text": "Done.", "resolve": true})).await;
    assert!(again.starts_with(&format!("You closed question {id}: it's over already")), "{again}");

    // One that waits again after the agent's answer can be closed too.
    let id = id_in(&call(&mut client, &mut view, "ask_person", serde_json::json!({"title": "Rename the repo?", "body": "B"})).await);
    let (seq, _) = card(&view, &id);
    assert_eq!(answered(&mut client, &mut view, reply(seq, "To what?", 1)).await, Ok(()));
    view.follow(&mut client, |view| events(view).len() == 2 && idle(view, "builder")).await;
    call(&mut client, &mut view, "ask_reply", serde_json::json!({"id": id, "text": "To claktory."})).await;
    assert_eq!(view.agent("builder").waiting_seqs, [seq]);
    call(&mut client, &mut view, "ask_reply", serde_json::json!({"id": id, "text": "Never mind: keeping the name.", "resolve": true})).await;
    let (_, closed) = card(&view, &id);
    assert_eq!((closed.state(), closed.resolved, closed.thread.len()), (AskPersonState::Answered, true, 3));
    view.follow(&mut client, |view| view.agent("builder").waiting_seqs.is_empty()).await;

    // Approved with a comment: the approval stands, and the words are added.
    let id = id_in(&call(&mut client, &mut view, "ask_person", serde_json::json!({"title": "Ship it?", "body": "B"})).await);
    let (seq, _) = card(&view, &id);
    let yes = v1::AnswerAsk { agent_id: "builder".into(), seq, text: "After 5 pm.".into(), revision: Some(1), ..Default::default() };
    assert_eq!(answered(&mut client, &mut view, yes).await, Ok(()));
    view.follow(&mut client, |view| events(view).len() == 3 && idle(view, "builder")).await;
    let said = call(&mut client, &mut view, "ask_reply", serde_json::json!({"id": id, "text": "Shipping at 5.", "resolve": true})).await;
    assert!(said.contains("had decided it already, so its answer stands"), "{said}");
    let (_, kept) = card(&view, &id);
    assert_eq!((kept.state(), kept.resolved, kept.answer.as_str(), kept.thread.len()), (AskPersonState::Answered, false, "After 5 pm.", 2));

    // Nothing said on it yet: that's ask_withdraw's. New words with it: refused.
    let id = id_in(&call(&mut client, &mut view, "ask_person", serde_json::json!({"title": "Lunch?", "body": "B"})).await);
    let early = call(&mut client, &mut view, "ask_reply", serde_json::json!({"id": id, "text": "Never mind.", "resolve": true})).await;
    assert!(early.contains("take it back with ask_withdraw"), "{early}");
    let worded = call(&mut client, &mut view, "ask_reply", serde_json::json!({"id": id, "text": "x", "resolve": true, "title": "T"})).await;
    assert!(worded.starts_with("Closing a request changes nothing on it"), "{worded}");
}

/// One of Builder's uploads, as the app makes it before answering with it.
fn upload(dir: &std::path::Path, name: &str, mime: &str) -> v1::Attachment {
    let folder = dir.join("home/agents/builder/uploads/2026-11-26");
    std::fs::create_dir_all(&folder).expect("uploads");
    let path = folder.join(name);
    std::fs::write(&path, b"\x89PNG\r\n\x1a\n").expect("file");
    v1::Attachment { path: path.display().to_string(), name: name.into(), size: 8, mime: mime.into() }
}

#[tokio::test]
async fn files_go_with_every_answer_on_a_card_and_stay_on_its_talk() {
    let dir = temp_dir();
    let (addr, _factory) = start(&dir).await;
    let (mut client, workspace, _) = Client::connect(addr).await;
    let mut view = View { workspace, ..View::default() };
    apply_request(&mut client, &mut view, create_project("Site", &dir.join("site"))).await;
    apply_request(&mut client, &mut view, create_agent("site", "Builder", "")).await;
    let shot = upload(&dir, "error.png", "image/png");
    let log = upload(&dir, "build.log", "text/plain");

    // A reply needs words or a file; a file alone will do (#511).
    let id = id_in(&call(&mut client, &mut view, "ask_person", serde_json::json!({"title": "Why does the build fail?", "body": "B"})).await);
    let (seq, _) = card(&view, &id);
    let reply = |attachments: Vec<v1::Attachment>, revision: u32| v1::AnswerAsk {
        agent_id: "builder".into(),
        seq,
        reply: true,
        attachments,
        revision: Some(revision),
        from: AskAnsweredFrom::NeedsYou.into(),
        ..Default::default()
    };
    assert_eq!(
        answered(&mut client, &mut view, reply(Vec::new(), 1)).await,
        Err("Write a reply or attach a file, or approve or dismiss the question.".to_owned())
    );
    // Only Builder's uploads.
    let stray = v1::Attachment { path: dir.join("site/x.png").display().to_string(), name: "x.png".into(), ..Default::default() };
    std::fs::write(dir.join("site/x.png"), b"x").expect("stray");
    let refused = answered(&mut client, &mut view, reply(vec![stray.clone()], 1)).await.expect_err("refused");
    assert!(refused.ends_with("isn't one of this agent's uploads."), "{refused}");
    assert_eq!(answered(&mut client, &mut view, reply(vec![shot.clone(), log.clone()], 1)).await, Ok(()));
    let (_, replied) = card(&view, &id);
    assert_eq!(replied.answer_attachments.iter().map(|file| file.name.as_str()).collect::<Vec<_>>(), ["error.png", "build.log"]);
    view.follow(&mut client, |view| events(view).len() == 1 && idle(view, "builder")).await;
    let (told, quiet) = events(&view).pop().expect("told");
    assert!(!quiet);
    assert!(told.starts_with("The person replied to your question \"Why does the build fail?\", with files only."), "{told}");
    assert!(
        told.contains(&format!(
            "The person attached 2 files; read them at their paths:\n- {} (\"error.png\", image/png, 8 bytes)\n- {} (\"build.log\", text/plain, 8 bytes)\nLook at images with read_image.",
            shot.path, log.path
        )),
        "{told}"
    );
    assert!(told.contains(&format!("If what they sent needs an answer from you, reply in the request with ask_reply (id {id})")), "{told}");

    // Answered on, the files stay on the talk, with the turn they came with.
    call(&mut client, &mut view, "ask_reply", serde_json::json!({"id": id, "text": "The log says the disk is full."})).await;
    let (_, open) = card(&view, &id);
    assert!(open.answer_attachments.is_empty());
    let files: Vec<Vec<&str>> = open.thread.iter().map(|message| message.attachments.iter().map(|file| file.path.as_str()).collect()).collect();
    assert_eq!(files, [vec![shot.path.as_str(), log.path.as_str()], vec![]]);

    // Resolved with a file and no words: kept, named, and not quiet.
    let resolve = v1::AnswerAsk { agent_id: "builder".into(), seq, resolve: true, attachments: vec![log.clone()], revision: Some(2), ..Default::default() };
    assert_eq!(answered(&mut client, &mut view, resolve).await, Ok(()));
    let (_, ended) = card(&view, &id);
    assert_eq!((ended.resolved, ended.answer_attachments.len(), ended.thread[0].attachments.len()), (true, 1, 2));
    view.follow(&mut client, |view| events(view).len() == 2 && idle(view, "builder")).await;
    let (told, quiet) = events(&view).pop().expect("told");
    assert!(!quiet, "a Resolve with a file wakes the agent");
    assert!(told.contains(&format!("Carry on with what you agreed.\n\nThe person attached a file; read it at its path:\n- {}", log.path)), "{told}");

    // Approved with a picture: named, after their comment.
    let id = id_in(&call(&mut client, &mut view, "ask_person", serde_json::json!({"title": "Ship this design?", "body": "B"})).await);
    let (seq, _) = card(&view, &id);
    let yes = v1::AnswerAsk { agent_id: "builder".into(), seq, text: "With this logo.".into(), attachments: vec![shot.clone()], revision: Some(1), ..Default::default() };
    assert_eq!(answered(&mut client, &mut view, yes).await, Ok(()));
    view.follow(&mut client, |view| events(view).len() == 3 && idle(view, "builder")).await;
    let (told, _) = events(&view).pop().expect("told");
    assert!(told.contains(&format!("With this logo.\n\nThe person attached a file; read it at its path:\n- {} (", shot.path)), "{told}");

    // A step answered with a file: kept on the step, named, and answered on.
    let steps = serde_json::json!({"title": "Set up signing", "body": "B", "steps": ["Open Settings.", "Add the key."]});
    let id = id_in(&call(&mut client, &mut view, "ask_person", steps).await);
    let (seq, _) = card(&view, &id);
    let done = v1::AnswerAsk { agent_id: "builder".into(), seq, step: Some(0), attachments: vec![shot.clone()], revision: Some(1), ..Default::default() };
    assert_eq!(answered(&mut client, &mut view, v1::AnswerAsk { attachments: vec![stray], ..done.clone() }).await.map_err(|_| ()), Err(()));
    assert_eq!(answered(&mut client, &mut view, done).await, Ok(()));
    let (_, waiting) = card(&view, &id);
    assert_eq!((waiting.steps[0].state(), waiting.steps[0].comment_attachments.len()), (AskStepState::Done, 1));
    view.follow(&mut client, |view| events(view).len() == 4 && idle(view, "builder")).await;
    let (told, _) = events(&view).pop().expect("told");
    assert!(told.contains(&format!("did step 1 of 2 of \"Set up signing\": Open Settings.\n\nThe person attached a file; read it at its path:\n- {}", shot.path)), "{told}");
    let said = call(&mut client, &mut view, "ask_reply", serde_json::json!({"id": id, "text": "That's the right screen."})).await;
    assert!(said.starts_with(&format!("Answered on question {id}")), "{said}");
    let (_, open) = card(&view, &id);
    assert_eq!((open.thread[0].attachments.len(), open.steps[0].comment_attachments.len()), (1, 1));

    // A written-answer card with a talk: a reply can be files alone, an
    // answer still needs words (Abbacchio 45289).
    let id = id_in(&call(&mut client, &mut view, "ask_person", serde_json::json!({"title": "Which branch?", "body": "B", "wants_text": true})).await);
    let (seq, _) = card(&view, &id);
    let answer = v1::AnswerAsk { agent_id: "builder".into(), seq, text: "The one in this log?".into(), revision: Some(1), ..Default::default() };
    assert_eq!(answered(&mut client, &mut view, answer).await, Ok(()));
    view.follow(&mut client, |view| events(view).len() == 5 && idle(view, "builder")).await;
    call(&mut client, &mut view, "ask_reply", serde_json::json!({"id": id, "text": "Which log?"})).await;
    let (_, talking) = card(&view, &id);
    assert!(!talking.thread.is_empty());
    let files = v1::AnswerAsk { agent_id: "builder".into(), seq, attachments: vec![log.clone()], revision: Some(talking.revision), ..Default::default() };
    assert_eq!(
        answered(&mut client, &mut view, files.clone()).await,
        Err("Write an answer, or dismiss the question.".to_owned())
    );
    assert_eq!(answered(&mut client, &mut view, v1::AnswerAsk { reply: true, from: AskAnsweredFrom::NeedsYou.into(), ..files }).await, Ok(()));
    view.follow(&mut client, |view| events(view).len() == 6 && idle(view, "builder")).await;
    let (told, _) = events(&view).pop().expect("told");
    assert!(told.contains(&format!("The person attached a file; read it at its path:\n- {}", log.path)), "{told}");
}
