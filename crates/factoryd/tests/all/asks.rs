//! Asks (#208): an agent blocked on something only the person can give asks
//! them with ask_person, a card the person answers or dismisses; the agent
//! is told which. Pending asks wait across restarts, at most 3 at once.

use crate::common::{Client, View, apply_request, create_agent, create_project, send_message, start, temp_dir};
use factory_protocol::v1::{self, AgentStatus, AskPersonState, client_message, entry::Kind, server_message::Body};

fn select(agent: &str) -> client_message::Body {
    client_message::Body::SelectAgent(v1::SelectAgent { agent_id: agent.into() })
}

fn idle(view: &View, agent: &str) -> bool {
    let agent = view.agent(agent);
    agent.status() == AgentStatus::Idle && agent.queued_messages == 0
}

fn answer(seq: u64, dismiss: bool, text: &str) -> client_message::Body {
    client_message::Body::AnswerAsk(v1::AnswerAsk { agent_id: "builder".into(), seq, dismiss, text: text.into() })
}

/// A Factory with a project "Site" and its agent Builder.
async fn setup(dir: &std::path::Path) -> (std::sync::Arc<factoryd::Factory>, Client, View) {
    let (addr, factory) = start(dir).await;
    let (mut client, workspace, _) = Client::connect(addr).await;
    let mut view = View { workspace, ..View::default() };
    apply_request(&mut client, &mut view, create_project("Site", &dir.join("site"))).await;
    apply_request(&mut client, &mut view, create_agent("site", "Builder", "")).await;
    apply_request(&mut client, &mut view, select("builder")).await;
    (factory, client, view)
}

/// Builder asks; returns what the tool said and the card it shows (with its
/// seq), if any.
async fn ask(client: &mut Client, view: &mut View, title: &str, wants_text: bool) -> (String, Option<(u64, v1::AskPerson)>) {
    let before = view.entries.len();
    let args = serde_json::json!({"title": title, "body": "The release is **ready**; only you can say when.", "wants_text": wants_text});
    apply_request(client, view, send_message("builder", &format!("call ask_person {args}"))).await;
    view.follow(client, |view| view.entries.len() >= before + 3 && idle(view, "builder")).await;
    let new = &view.entries[before..];
    let output = new.iter().find_map(|entry| match &entry.kind {
        Some(Kind::ToolCall(call)) => Some(call.output.clone()),
        _ => None,
    });
    let card = new.iter().find_map(|entry| match &entry.kind {
        Some(Kind::AskPerson(ask)) => Some((entry.seq, ask.clone())),
        _ => None,
    });
    (output.expect("a tool call"), card)
}

fn card(view: &View, seq: u64) -> v1::AskPerson {
    view.entries
        .iter()
        .find_map(|entry| match &entry.kind {
            Some(Kind::AskPerson(ask)) if entry.seq == seq => Some(ask.clone()),
            _ => None,
        })
        .expect("the card")
}

/// The text of the last event from Claktory, once Builder has run on it.
async fn told(client: &mut Client, view: &mut View, count: usize) -> String {
    let told = |view: &View| {
        view.entries
            .iter()
            .filter_map(|entry| match &entry.kind {
                Some(Kind::Event(event)) if event.source() == v1::EventSource::Factory => Some(event.text.clone()),
                _ => None,
            })
            .collect::<Vec<_>>()
    };
    view.follow(client, |view| told(view).len() >= count && idle(view, "builder")).await;
    told(view).pop().expect("an event")
}

#[tokio::test]
async fn an_ask_waits_for_the_person_who_answers_it() {
    let dir = temp_dir();
    let (_factory, mut client, mut view) = setup(&dir).await;
    let (said, shown) = ask(&mut client, &mut view, "Which color for the launch page?", true).await;
    assert!(said.starts_with("Asked the person: they see it as a card"), "{said}");
    let (seq, shown) = shown.expect("a card after the call");
    assert_eq!(
        (shown.title.as_str(), shown.wants_text, shown.state()),
        ("Which color for the launch page?", true, AskPersonState::Pending)
    );
    assert!(shown.body.contains("**ready**"));
    // It waits for the person, as every card does.
    assert_eq!(view.agent("builder").waiting_seqs, [seq]);

    // A written answer is needed when it wants one.
    let empty = apply_request(&mut client, &mut view, answer(seq, false, " ")).await;
    let Body::Error(error) = empty else { panic!("{empty:?}") };
    assert_eq!(error.message, "Write an answer, or dismiss the question.");
    let reply = apply_request(&mut client, &mut view, answer(seq, false, "Blue, the dark one.")).await;
    assert!(matches!(reply, Body::Ack(_)), "{reply:?}");
    let decided = card(&view, seq);
    assert_eq!((decided.state(), decided.answer.as_str()), (AskPersonState::Answered, "Blue, the dark one."));
    assert!(decided.decided_ms > 0);
    let text = told(&mut client, &mut view, 1).await;
    assert!(text.starts_with("The person answered your question \"Which color for the launch page?\":\n\nBlue, the dark one."), "{text}");
    assert!(view.agent("builder").waiting_seqs.is_empty());

    // Answered once.
    let again = apply_request(&mut client, &mut view, answer(seq, true, "")).await;
    let Body::Error(error) = again else { panic!("{again:?}") };
    assert_eq!(error.message, "This question was already answered.");
    let nothing = apply_request(&mut client, &mut view, answer(1, false, "x")).await;
    assert!(matches!(nothing, Body::Error(_)), "{nothing:?}");
}

#[tokio::test]
async fn a_dismissed_ask_tells_the_agent_to_use_its_judgment_and_a_yes_needs_no_words() {
    let dir = temp_dir();
    let (_factory, mut client, mut view) = setup(&dir).await;
    let (_, first) = ask(&mut client, &mut view, "Deploy to production tonight?", false).await;
    let (seq, _) = first.expect("a card");
    apply_request(&mut client, &mut view, answer(seq, true, "")).await;
    assert_eq!(card(&view, seq).state(), AskPersonState::Dismissed);
    let text = told(&mut client, &mut view, 1).await;
    assert!(text.starts_with("The person dismissed your question \"Deploy to production tonight?\""), "{text}");
    assert!(text.contains("Use your best judgment"), "{text}");

    let (_, second) = ask(&mut client, &mut view, "Delete the old bucket?", false).await;
    let (seq, _) = second.expect("a card");
    let reply = apply_request(&mut client, &mut view, answer(seq, false, "")).await;
    assert!(matches!(reply, Body::Ack(_)), "{reply:?}");
    let text = told(&mut client, &mut view, 2).await;
    assert!(text.starts_with("The person answered your question \"Delete the old bucket?\": yes, go ahead."), "{text}");
}

#[tokio::test]
async fn at_most_three_wait_and_they_survive_a_restart() {
    let dir = temp_dir();
    let (factory, mut client, mut view) = setup(&dir).await;
    let mut seqs = Vec::new();
    for title in ["One?", "Two?", "Three?"] {
        let (_, shown) = ask(&mut client, &mut view, title, true).await;
        seqs.push(shown.expect("a card").0);
    }
    let (said, shown) = ask(&mut client, &mut view, "Four?", true).await;
    assert!(shown.is_none());
    assert!(said.contains("You already have 3 questions waiting for the person"), "{said}");
    assert_eq!(view.agent("builder").waiting_seqs, seqs);
    apply_request(&mut client, &mut view, answer(seqs[0], false, "Yes.")).await;
    told(&mut client, &mut view, 1).await;
    // Room for one more now.
    let (_, shown) = ask(&mut client, &mut view, "Four?", true).await;
    seqs.push(shown.expect("a card").0);
    drop(client);
    drop(factory);

    // After a restart: decided ones show decided, pending ones still wait,
    // without the conversation open, and can be answered.
    let (addr, _factory) = start(&dir).await;
    let (mut client, workspace, _) = Client::connect(addr).await;
    let mut view = View { workspace, ..View::default() };
    view.follow(&mut client, |view| view.agent("builder").waiting_seqs == seqs[1..]).await;
    apply_request(&mut client, &mut view, select("builder")).await;
    let first = card(&view, seqs[0]);
    assert_eq!((first.state(), first.answer.as_str()), (AskPersonState::Answered, "Yes."));
    assert_eq!(card(&view, seqs[1]).state(), AskPersonState::Pending);
    let reply = apply_request(&mut client, &mut view, answer(seqs[1], false, "No.")).await;
    assert!(matches!(reply, Body::Ack(_)), "{reply:?}");
    assert_eq!(view.agent("builder").waiting_seqs, seqs[2..]);

    // /clear dismisses what waits, so new questions have room.
    let clear = v1::RunCommand { agent_id: "builder".into(), name: "clear".into(), args: String::new() };
    apply_request(&mut client, &mut view, client_message::Body::RunCommand(clear)).await;
    view.follow(&mut client, |view| view.agent("builder").waiting_seqs.is_empty()).await;
    for title in ["A?", "B?", "C?"] {
        let (_, shown) = ask(&mut client, &mut view, title, true).await;
        assert!(shown.is_some(), "{title}");
    }
}
