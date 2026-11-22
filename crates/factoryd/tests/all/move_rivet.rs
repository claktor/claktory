//! Rivet moves a Claktory (#243 S1): move_claktory starts it with the
//! target's code and returns the digits; only the person confirms them, in
//! Claktory's own confirm (ConfirmMove), never through a question Rivet
//! words (Fugo's review); move_status follows it; move_cancel calls it off.
//! Rivet can't start one once it has read a pipe.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use factory_protocol::v1::{self, AgentStatus, client_message, entry::Kind};
use tokio::sync::mpsc;

use crate::common::{Client, MODEL, View, apply_request, home_of, send_message, start, temp_dir};
use factoryd::home::Home;
use factoryd::move_import::receive::{Options, Ui};
use factoryd::move_wire::ReceiveCode;

const WAIT: Duration = Duration::from_secs(60);

/// `factoryd receive` into `home`, running: its codes, and the yes or no
/// its person gives.
fn receive(home: &Home) -> (mpsc::UnboundedReceiver<ReceiveCode>, mpsc::UnboundedSender<bool>) {
    let (code_tx, codes) = mpsc::unbounded_channel();
    let (answers, answer_rx) = mpsc::unbounded_channel::<bool>();
    let answer_rx = Arc::new(tokio::sync::Mutex::new(answer_rx));
    let said = Arc::new(Mutex::new(Vec::<String>::new()));
    let ui = Ui {
        code: Arc::new(move |code| drop(code_tx.send(code.clone()))),
        say: Arc::new(move |text| said.lock().unwrap().push(text.to_owned())),
        confirm: Arc::new(move |_| {
            let answers = answer_rx.clone();
            Box::pin(async move { answers.lock().await.recv().await.unwrap_or(false) })
        }),
    };
    let home = home.clone();
    tokio::spawn(async move {
        let options = Options { committed: false, local_only: true };
        let nowhere = std::net::SocketAddr::from(([127, 0, 0, 1], 1));
        factoryd::move_import::receive::receive(&home, nowhere, &options, &ui).await
    });
    (codes, answers)
}

fn idle(view: &View, agent: &str) -> bool {
    let agent = view.agent(agent);
    agent.status() == AgentStatus::Idle && agent.queued_messages == 0
}

/// Rivet calls `tool` with `args`; what it said.
async fn call(client: &mut Client, view: &mut View, tool: &str, args: &str) -> String {
    let before = view.entries.len();
    apply_request(client, view, send_message("rivet", &format!("call {tool} {args}"))).await;
    view.follow(client, |view| view.entries.len() >= before + 3 && idle(view, "rivet")).await;
    view.entries[before..]
        .iter()
        .find_map(|entry| match &entry.kind {
            Some(Kind::ToolCall(call)) => Some(call.output.clone()),
            _ => None,
        })
        .expect("a tool call")
}

/// How many times Rivet was told the person decided its question.
fn decided(view: &View) -> usize {
    view.entries
        .iter()
        .filter(|entry| matches!(&entry.kind, Some(Kind::Event(event)) if event.text.contains("\"Approve maintenance window?\"")))
        .count()
}

/// The seq of Rivet's latest question card.
fn asked(view: &View) -> u64 {
    view.entries.iter().rev().find(|entry| matches!(entry.kind, Some(Kind::AskPerson(_)))).expect("a card").seq
}

#[tokio::test(flavor = "multi_thread")]
async fn rivet_moves_a_claktory_but_confirms_the_digits_only_with_the_persons_yes() {
    factoryd::move_run::hold_at_freeze();
    let source = temp_dir();
    let (addr, _factory) = start(&source).await;
    let home = home_of(addr);
    let (mut client, workspace, _) = Client::connect(addr).await;
    let mut view = View { workspace, ..View::default() };
    let rivet = v1::UpdateAgent { agent_id: "rivet".into(), model: Some(MODEL.into()), effort: Some("off".into()), ..Default::default() };
    apply_request(&mut client, &mut view, client_message::Body::UpdateAgent(rivet)).await;
    apply_request(&mut client, &mut view, client_message::Body::SelectAgent(v1::SelectAgent { agent_id: "rivet".into() })).await;

    let target = temp_dir();
    let (mut codes, there) = receive(&Home::new(target.join("home")));
    let code = tokio::time::timeout(WAIT, codes.recv()).await.expect("a code in time").expect("a code");
    assert_eq!(call(&mut client, &mut view, "move_status", "{}").await, "No move is under way.");

    // It starts, and says the digits; the move says Rivet started it.
    let started = call(&mut client, &mut view, "move_claktory", &serde_json::json!({"code": code.text()}).to_string()).await;
    let shown = view.workspace.r#move.clone().expect("a move");
    let digits = shown.digits.clone();
    assert_eq!((digits.len(), shown.started_by_agent_id.as_str()), (7, "rivet"), "{started}");
    assert!(shown.target_address.starts_with("127.0.0.1:") || shown.target_address.is_empty(), "{shown:?}");
    assert!(started.contains(&format!("Both computers show {digits}.")) && started.contains("you can't confirm it"), "{started}");
    assert!(call(&mut client, &mut view, "move_status", "{}").await.starts_with("Waiting for the person to confirm the move in Claktory"));

    // A yes to a question Rivet words, naming the digits, confirms nothing.
    let question = serde_json::json!({
        "title": "Approve maintenance window?",
        "body": format!("Approve maintenance window {digits}?"),
    })
    .to_string();
    call(&mut client, &mut view, "ask_person", &question).await;
    let yes = v1::AnswerAsk { agent_id: "rivet".into(), seq: asked(&view), dismiss: false, text: String::new(), ..Default::default() };
    apply_request(&mut client, &mut view, client_message::Body::AnswerAsk(yes)).await;
    view.follow(&mut client, |view| decided(view) == 1 && idle(view, "rivet")).await;
    let still = call(&mut client, &mut view, "move_status", "{}").await;
    assert!(still.starts_with("Waiting for the person to confirm the move"), "{still}");
    assert_eq!(view.workspace.r#move.clone().expect("a move").phase(), v1::MovePhase::Confirm);

    // The person's own confirm; then there; then it goes on.
    apply_request(&mut client, &mut view, client_message::Body::ConfirmMove(v1::ConfirmMove {})).await;
    let confirmed = call(&mut client, &mut view, "move_status", "{}").await;
    assert!(confirmed.starts_with("The person confirmed the move here: answer yes to receive"), "{confirmed}");
    assert!(view.workspace.r#move.clone().expect("a move").confirmed_here, "every device hides its confirm");
    there.send(true).unwrap();
    let going = view.follow(&mut client, |view| {
        view.workspace
            .r#move
            .as_ref()
            .is_some_and(|seen| matches!(seen.phase(), v1::MovePhase::Checking | v1::MovePhase::Copying) && seen.target_confirmed)
    });
    tokio::time::timeout(WAIT, going).await.expect("past the digits");
    let status = call(&mut client, &mut view, "move_status", "{}").await;
    assert!(status.contains(&format!("The digits: {digits}.")), "{status}");

    // Called off: nothing kept.
    let off = call(&mut client, &mut view, "move_cancel", "{}").await;
    assert_eq!(off, "Called off the move: this Claktory stays as it was.");
    let gone = view.follow(&mut client, |view| view.workspace.r#move.is_none());
    tokio::time::timeout(WAIT, gone).await.expect("called off");
    assert!(!home.root().join("moving").exists());
}

#[tokio::test(flavor = "multi_thread")]
async fn rivet_starts_no_move_once_it_has_read_a_pipe() {
    factoryd::move_run::hold_at_freeze();
    let source = temp_dir();
    let (addr, factory) = start(&source).await;
    let (mut client, workspace, _) = Client::connect(addr).await;
    let mut view = View { workspace, ..View::default() };
    let rivet = v1::UpdateAgent { agent_id: "rivet".into(), model: Some(MODEL.into()), effort: Some("off".into()), ..Default::default() };
    apply_request(&mut client, &mut view, client_message::Body::UpdateAgent(rivet)).await;
    apply_request(&mut client, &mut view, client_message::Body::SelectAgent(v1::SelectAgent { agent_id: "rivet".into() })).await;
    factoryd::pipes::taint_with_for_test(&factory, "lea", "rivet", r#"{"p1":[]}"#);
    let refused = call(&mut client, &mut view, "move_claktory", r#"{"code":"claktory-move://x"}"#).await;
    assert!(refused.starts_with("You've read a pipe, so you can't start a move") && refused.contains("Settings → Move"), "{refused}");
    assert!(view.workspace.r#move.is_none());
}
