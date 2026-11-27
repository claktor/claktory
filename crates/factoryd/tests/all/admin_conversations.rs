//! An admin agent compacts another agent's conversation when the person
//! asks (agents_compact), as their /compact does, and asks them to clear one
//! (agents_clear): a card, and only their approval runs /clear. Other
//! agents have neither tool.

use crate::common::{
    Client, MODEL, Scripted, View, apply_request, create_agent, create_project, describe, send_message, start_with, temp_dir,
};
use factory_protocol::v1::{self, AgentStatus, client_message, entry::Kind, server_message::Body};

fn select(agent: &str) -> client_message::Body {
    client_message::Body::SelectAgent(v1::SelectAgent { agent_id: agent.into() })
}

fn idle(view: &View, agent: &str) -> bool {
    let agent = view.agent(agent);
    agent.status() == AgentStatus::Idle && agent.queued_messages == 0
}

/// `agent` calls `tool`; what it said.
async fn call(client: &mut Client, view: &mut View, agent: &str, tool: &str, args: &str) -> String {
    apply_request(client, view, select(agent)).await;
    let before = view.entries.len();
    apply_request(client, view, send_message(agent, &format!("call {tool} {args}"))).await;
    view.follow(client, |view| view.entries.len() >= before + 3 && idle(view, agent)).await;
    view.entries[before..]
        .iter()
        .find_map(|entry| match &entry.kind {
            Some(Kind::ToolCall(call)) => Some(call.output.clone()),
            _ => None,
        })
        .expect("a tool call")
}

/// The clear cards in the selected agent's conversation, with their seqs.
fn cards(view: &View) -> Vec<(u64, v1::AskPerson)> {
    view.entries
        .iter()
        .filter_map(|entry| match &entry.kind {
            Some(Kind::AskPerson(ask)) => Some((entry.seq, ask.clone())),
            _ => None,
        })
        .collect()
}

/// The events from Claktory in Rivet's conversation, once it has run on `count` of them.
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
    view.follow(client, |view| told(view).len() >= count && idle(view, "rivet")).await;
    told(view).pop().expect("an event")
}

fn answer(seq: u64, dismiss: bool) -> client_message::Body {
    client_message::Body::AnswerAsk(v1::AnswerAsk { agent_id: "rivet".into(), seq, dismiss, ..Default::default() })
}

#[tokio::test]
async fn rivet_compacts_an_agent_when_asked_and_clears_one_only_with_the_persons_approval() {
    let dir = temp_dir();
    let models = Scripted::signed_in();
    let tools = models.tools.clone();
    let (addr, _factory) = start_with(&dir, models).await;
    let (mut client, workspace, _) = Client::connect(addr).await;
    let mut view = View { workspace, ..View::default() };
    apply_request(&mut client, &mut view, create_project("Site", &dir.join("site"))).await;
    apply_request(&mut client, &mut view, create_agent("site", "Tiziano", "")).await;
    apply_request(&mut client, &mut view, create_agent("site", "Mike", "")).await;
    let rivet = v1::UpdateAgent { agent_id: "rivet".into(), model: Some(MODEL.into()), effort: Some("off".into()), ..Default::default() };
    apply_request(&mut client, &mut view, client_message::Body::UpdateAgent(rivet)).await;

    // Only an admin agent has the tools.
    apply_request(&mut client, &mut view, select("mike")).await;
    apply_request(&mut client, &mut view, send_message("mike", "hello")).await;
    view.follow(&mut client, |view| view.entries.len() == 2 && idle(view, "mike")).await;
    let mikes = tools.lock().expect("lock").clone();
    assert!(!mikes.contains("agents_compact") && !mikes.contains("agents_clear"), "{mikes}");

    // Nothing to compact yet, nor itself.
    let said = call(&mut client, &mut view, "rivet", "agents_compact", r#"{"agent":"Tiziano"}"#).await;
    assert_eq!(said, "Tiziano's conversation has nothing to compact yet. Nothing was done.");
    let rivets = tools.lock().expect("lock").clone();
    assert!(rivets.contains("agents_compact") && rivets.contains("agents_clear"), "{rivets}");
    let said = call(&mut client, &mut view, "rivet", "agents_compact", r#"{"agent":"rivet"}"#).await;
    assert!(said.starts_with("That's you"), "{said}");
    let said = call(&mut client, &mut view, "rivet", "agents_clear", r#"{"agent":"nobody"}"#).await;
    assert_eq!(said, "No agent has the id or name \"nobody\".");

    // Idle, it's compacted now, as /compact does.
    apply_request(&mut client, &mut view, select("tiziano")).await;
    apply_request(&mut client, &mut view, send_message("tiziano", "hello")).await;
    view.follow(&mut client, |view| view.entries.len() == 2 && idle(view, "tiziano")).await;
    let said = call(&mut client, &mut view, "rivet", "agents_compact", r#"{"agent":"Tiziano"}"#).await;
    assert_eq!(said, "Compacting Tiziano's conversation now; it carries on from the summary.");
    apply_request(&mut client, &mut view, select("tiziano")).await;
    view.follow(&mut client, |view| view.entries.len() == 3 && idle(view, "tiziano")).await;
    assert_eq!(describe(&view.entries), ["user: hello", "assistant: echo: hello", "compaction: 2 earlier messages."]);

    // Working, it's compacted once its current work ends.
    apply_request(&mut client, &mut view, send_message("tiziano", r#"call bash {"command":"sleep 4"}"#)).await;
    view.follow(&mut client, |view| view.agent("tiziano").status() == AgentStatus::Working).await;
    let said = call(&mut client, &mut view, "rivet", "agents_compact", r#"{"agent":"tiziano"}"#).await;
    assert_eq!(said, "Tiziano is working: its conversation is compacted as soon as its current work ends, before it reads anything new.");
    apply_request(&mut client, &mut view, select("tiziano")).await;
    view.follow(&mut client, |view| {
        idle(view, "tiziano") && describe(&view.entries).last().is_some_and(|last| last.starts_with("compaction:"))
    })
    .await;
    let compactions = describe(&view.entries).iter().filter(|entry| entry.starts_with("compaction:")).count();
    assert_eq!(compactions, 2, "{:?}", describe(&view.entries));

    // Clearing is a card for the person; nothing is cleared until they approve.
    let said = call(&mut client, &mut view, "rivet", "agents_clear", r#"{"agent":"Tiziano"}"#).await;
    assert!(said.starts_with("Asked "), "{said}");
    let shown = cards(&view);
    assert_eq!(shown.len(), 1);
    let (seq, card) = &shown[0];
    assert_eq!(card.title, "Clear Tiziano's conversation");
    assert!(card.body.starts_with("Rivet asks to clear Tiziano's conversation."), "{}", card.body);
    assert!(!card.wants_text);
    let said = call(&mut client, &mut view, "rivet", "agents_clear", r#"{"agent":"Tiziano"}"#).await;
    assert!(said.contains("it's waiting for them"), "{said}");
    assert_eq!(cards(&view).len(), 1, "one request per agent at a time");

    // Dismissed, it stays as it is.
    // Without a comment, Rivet is told quietly (friend bug #19), with
    // whatever next wakes it.
    apply_request(&mut client, &mut view, answer(*seq, true)).await;
    view.follow(&mut client, |view| view.agent("rivet").quiet_waiting == 1 && idle(view, "rivet")).await;
    apply_request(&mut client, &mut view, select("tiziano")).await;
    assert!(describe(&view.entries).starts_with(&["user: hello".to_owned()]), "{:?}", describe(&view.entries));

    // Withdrawn by Rivet (#407), it closes and nothing is cleared.
    let said = call(&mut client, &mut view, "rivet", "agents_clear", r#"{"agent":"Tiziano"}"#).await;
    let text = told(&mut client, &mut view, 1).await;
    assert!(text.contains("dismissed your request to clear Tiziano's conversation, so it stays as it is."), "{text}");
    let id = said.split_once("Its id is ").and_then(|(_, rest)| rest.split(':').next()).expect("its id").to_owned();
    let withdrawn = call(&mut client, &mut view, "rivet", "ask_withdraw", &serde_json::json!({"id": id, "reason": "Tiziano was compacted instead."}).to_string()).await;
    assert!(withdrawn.starts_with(&format!("Withdrew question {id}, \"Clear Tiziano's conversation\"")), "{withdrawn}");
    let (seq, card) = cards(&view).pop().expect("the card");
    assert_eq!((card.state(), card.withdrawn), (v1::AskPersonState::Dismissed, true));
    match apply_request(&mut client, &mut view, answer(seq, false)).await {
        Body::Error(error) => assert_eq!(error.message, "This question was withdrawn."),
        other => panic!("expected a refusal, got {other:?}"),
    }
    apply_request(&mut client, &mut view, select("tiziano")).await;
    assert!(describe(&view.entries).starts_with(&["user: hello".to_owned()]), "{:?}", describe(&view.entries));

    // Approved, it's cleared as /clear does, and Rivet is told.
    call(&mut client, &mut view, "rivet", "agents_clear", r#"{"agent":"tiziano"}"#).await;
    let (seq, _) = cards(&view).pop().expect("a new card");
    apply_request(&mut client, &mut view, answer(seq, false)).await;
    let text = told(&mut client, &mut view, 2).await;
    assert!(text.contains("approved, so Tiziano's conversation is cleared"), "{text}");
    assert_eq!(cards(&view).last().expect("the card").1.state(), v1::AskPersonState::Answered);
    apply_request(&mut client, &mut view, select("tiziano")).await;
    view.follow(&mut client, |view| describe(&view.entries) == ["notice: Conversation cleared"]).await;
    assert_eq!(std::fs::read_dir(dir.join("home/agents/tiziano/archive")).expect("archive").count(), 1);
}

#[tokio::test]
async fn a_clear_card_closes_when_its_agent_is_deleted_and_never_clears_another_made_with_its_id() {
    let dir = temp_dir();
    let (addr, _factory) = start_with(&dir, Scripted::signed_in()).await;
    let (mut client, workspace, _) = Client::connect(addr).await;
    let mut view = View { workspace, ..View::default() };
    apply_request(&mut client, &mut view, create_project("Site", &dir.join("site"))).await;
    apply_request(&mut client, &mut view, create_agent("site", "Tiziano", "")).await;
    let rivet = v1::UpdateAgent { agent_id: "rivet".into(), model: Some(MODEL.into()), effort: Some("off".into()), ..Default::default() };
    apply_request(&mut client, &mut view, client_message::Body::UpdateAgent(rivet)).await;
    call(&mut client, &mut view, "rivet", "agents_clear", r#"{"agent":"Tiziano"}"#).await;
    let (seq, _) = cards(&view).pop().expect("the card");

    // Tiziano is deleted: the card closes, and Rivet is told.
    apply_request(&mut client, &mut view, client_message::Body::DeleteAgent(v1::DeleteAgent { agent_id: "tiziano".into() })).await;
    let text = told(&mut client, &mut view, 1).await;
    assert_eq!(
        text,
        "The agent your request \"Clear Tiziano's conversation\" was for was deleted, so the request is closed and nothing was cleared."
    );
    assert_eq!(cards(&view).pop().expect("the card").1.state(), v1::AskPersonState::Dismissed);

    // Made again with the same id, it isn't cleared by the old card.
    apply_request(&mut client, &mut view, create_agent("site", "Tiziano", "")).await;
    apply_request(&mut client, &mut view, select("tiziano")).await;
    apply_request(&mut client, &mut view, send_message("tiziano", "hello")).await;
    view.follow(&mut client, |view| view.entries.len() == 2 && idle(view, "tiziano")).await;
    apply_request(&mut client, &mut view, select("rivet")).await;
    assert!(matches!(apply_request(&mut client, &mut view, answer(seq, false)).await, Body::Error(_)));

    // Nor by a card for the agent it was, whatever else closed it: approving
    // clears only the agent it was asked for.
    call(&mut client, &mut view, "rivet", "agents_clear", r#"{"agent":"Tiziano"}"#).await;
    let (seq, _) = cards(&view).pop().expect("a new card");
    let incarnation = dir.join("home/agents/tiziano/incarnation");
    assert!(incarnation.exists(), "asking gave it an incarnation");
    std::fs::write(&incarnation, "0123456789abcdef0123456789abcdef\n").expect("another incarnation");
    match apply_request(&mut client, &mut view, answer(seq, false)).await {
        Body::Error(error) => assert_eq!(
            error.message,
            "The agent this request asked to clear was deleted, and Tiziano is another agent made since, so nothing was cleared: dismiss this request."
        ),
        other => panic!("expected a refusal, got {other:?}"),
    }
    assert_eq!(cards(&view).pop().expect("the card").1.state(), v1::AskPersonState::Pending);
    apply_request(&mut client, &mut view, select("tiziano")).await;
    assert_eq!(describe(&view.entries), ["user: hello", "assistant: echo: hello"]);
    assert!(!dir.join("home/agents/tiziano/archive").exists());

    // A card asked for it as it is now clears it.
    apply_request(&mut client, &mut view, select("rivet")).await;
    apply_request(&mut client, &mut view, answer(seq, true)).await;
    told(&mut client, &mut view, 3).await;
    call(&mut client, &mut view, "rivet", "agents_clear", r#"{"agent":"Tiziano"}"#).await;
    let (seq, _) = cards(&view).pop().expect("a new card");
    apply_request(&mut client, &mut view, answer(seq, false)).await;
    let text = told(&mut client, &mut view, 4).await;
    assert!(text.contains("approved, so Tiziano's conversation is cleared"), "{text}");
}

#[tokio::test]
async fn a_clear_card_closes_when_its_agent_is_taken_out_of_the_settings() {
    let dir = temp_dir();
    let (addr, _factory) = start_with(&dir, Scripted::signed_in()).await;
    let (mut client, workspace, _) = Client::connect(addr).await;
    let mut view = View { workspace, ..View::default() };
    apply_request(&mut client, &mut view, create_project("Site", &dir.join("site"))).await;
    apply_request(&mut client, &mut view, create_agent("site", "Tiziano", "")).await;
    let rivet = v1::UpdateAgent { agent_id: "rivet".into(), model: Some(MODEL.into()), effort: Some("off".into()), ..Default::default() };
    apply_request(&mut client, &mut view, client_message::Body::UpdateAgent(rivet)).await;
    call(&mut client, &mut view, "rivet", "agents_clear", r#"{"agent":"Tiziano"}"#).await;
    let (seq, _) = cards(&view).pop().expect("the card");

    // Taken out of config.json, as Rivet deletes agents.
    let path = dir.join("home/config.json");
    let mut config: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&path).expect("config")).expect("json");
    config["agents"].as_array_mut().expect("agents").retain(|agent| agent["id"] != "tiziano");
    std::fs::write(&path, serde_json::to_string_pretty(&config).expect("json")).expect("write");

    let text = told(&mut client, &mut view, 1).await;
    assert_eq!(
        text,
        "The agent your request \"Clear Tiziano's conversation\" was for was deleted, so the request is closed and nothing was cleared."
    );
    let card = cards(&view).into_iter().find(|(shown, _)| *shown == seq).expect("the card").1;
    assert_eq!(card.state(), v1::AskPersonState::Dismissed);
    assert!(matches!(apply_request(&mut client, &mut view, answer(seq, false)).await, Body::Error(_)));
    // Told once, however many times the settings are read since.
    tokio::time::sleep(std::time::Duration::from_millis(1500)).await;
    apply_request(&mut client, &mut view, select("rivet")).await;
    let closing = |view: &View| {
        view.entries.iter().filter(|entry| matches!(&entry.kind, Some(Kind::Event(event)) if event.text.contains("the request is closed"))).count()
    };
    assert_eq!(closing(&view), 1);
}
