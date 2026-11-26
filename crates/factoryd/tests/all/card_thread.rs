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
        Err("Builder replied on this card since you looked at it. Read the reply, then answer again.".to_owned())
    );
    assert_eq!(answered(&mut client, &mut view, answer(seq, 1, None)).await, Err("This card changed. Look at it again.".to_owned()));
    assert_eq!(
        answered(&mut client, &mut view, answer(seq, 2, Some(0))).await,
        Err("Builder replied on this card since you looked at it. Read the reply, then answer again.".to_owned())
    );
    assert_eq!(card(&view, &id).1.state(), AskPersonState::Pending);
    // Approved having seen it: the agent is told the talk, fenced, and to
    // act on where it ended rather than on the card's $20.
    assert_eq!(answered(&mut client, &mut view, answer(seq, 2, Some(2))).await, Ok(()));
    view.follow(&mut client, |view| events(view).len() == 2 && idle(view, "builder")).await;
    let (told, quiet) = events(&view).pop().expect("told");
    assert!(!quiet);
    assert!(
        told.starts_with("The person approved your question \"OK to spend up to $20 on Pangram?\" after your last reply on its card, without the card changing: they agreed to where your talk there ended, not only to the card's words."),
        "{told}"
    );
    assert!(
        told.contains("The talk on the card, oldest first:\n\n```\nThe person: No paid detectors.\n\nYou: Then GPTZero's free plan, at no cost.\n```"),
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
    assert!(!open.approve_follows_talk, "the card itself asks it now");
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
    assert!(again.contains("it's over already"), "{again}");

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
    assert!(worded.starts_with("Closing a card changes nothing on it"), "{worded}");
}
