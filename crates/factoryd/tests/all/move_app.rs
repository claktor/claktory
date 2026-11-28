//! Moving a Claktory from the app (#217, phase 2), end to end against the
//! real target (`factoryd receive`, in this process): StartMove, the digits
//! on both sides, the copy while it runs, the freeze, then frozen mode's
//! last round, import there, the fence here and the commit.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use factory_protocol::v1;
use factory_protocol::v1::client_message::Body as Request;
use factory_protocol::v1::server_message::Body;
use tokio::sync::mpsc;

use crate::common::{Client, home_of, serves, start, temp_dir};
use factoryd::home::Home;
use factoryd::move_import::receive::{Options, Ui};
use factoryd::move_wire::ReceiveCode;

const WAIT: Duration = Duration::from_secs(60);

/// The person at the target: the codes it shows, and their answers.
struct There {
    codes: mpsc::UnboundedReceiver<ReceiveCode>,
    answers: mpsc::UnboundedSender<bool>,
    said: Arc<Mutex<Vec<String>>>,
}

impl There {
    async fn code(&mut self) -> ReceiveCode {
        tokio::time::timeout(WAIT, self.codes.recv()).await.expect("a code in time").expect("a code")
    }

    fn said(&self) -> String {
        self.said.lock().unwrap().join("\n")
    }
}

type Receiving = tokio::task::JoinHandle<Result<String, factoryd::move_import::ImportError>>;

/// `factoryd receive` into `home`, running.
fn receive(home: &Home) -> (Receiving, There) {
    let (code_tx, codes) = mpsc::unbounded_channel();
    let (answers, answer_rx) = mpsc::unbounded_channel::<bool>();
    let answer_rx = Arc::new(tokio::sync::Mutex::new(answer_rx));
    let said = Arc::new(Mutex::new(Vec::new()));
    let ui = Ui {
        code: Arc::new(move |code| drop(code_tx.send(code.clone()))),
        say: {
            let said = said.clone();
            Arc::new(move |text| said.lock().unwrap().push(text.to_owned()))
        },
        confirm: Arc::new(move |_| {
            let answers = answer_rx.clone();
            Box::pin(async move { answers.lock().await.recv().await.unwrap_or(false) })
        }),
    };
    let home = home.clone();
    let task = tokio::spawn(async move {
        let options = Options { committed: false, local_only: true, ..Options::default() };
        let nowhere = std::net::SocketAddr::from(([127, 0, 0, 1], 1));
        factoryd::move_import::receive::receive(&home, nowhere, &options, &ui).await
    });
    (task, There { codes, answers, said })
}

/// Waits for the move to reach `phase`; the move then.
async fn phase(client: &mut Client, phase: v1::MovePhase) -> v1::Move {
    let waited = client.wait_for(|body| match body {
        Body::Workspace(workspace) => match &workspace.r#move {
            Some(seen) if seen.phase() == phase => Some(Ok(seen.clone())),
            Some(seen) if seen.phase() == v1::MovePhase::Failed && phase != v1::MovePhase::Failed => Some(Err(seen.error.clone())),
            _ => None,
        },
        _ => None,
    });
    match tokio::time::timeout(WAIT, waited).await.expect("in time") {
        Ok(seen) => seen,
        Err(why) => panic!("the move failed waiting for {phase:?}: {why}"),
    }
}

fn start_move(code: &ReceiveCode) -> Request {
    Request::StartMove(v1::StartMove { code: code.text() })
}

#[tokio::test(flavor = "multi_thread")]
async fn a_claktory_moves_from_the_app_through_a_freeze_and_is_fenced_once_the_target_commits() {
    factoryd::move_run::hold_at_freeze();
    let source = temp_dir();
    let (addr, factory) = start(&source).await;
    let home = home_of(addr);
    std::fs::create_dir_all(home.root().join("agents/ana")).unwrap();
    std::fs::write(home.root().join("agents/ana/notes.md"), "written before the move\n").unwrap();
    let (mut client, workspace, _) = Client::connect(addr).await;
    assert!(workspace.r#move.is_none());

    let target = temp_dir();
    let there = Home::new(target.join("home"));
    let (receiving, mut person) = receive(&there);
    let code = person.code().await;
    client.ok(start_move(&code)).await;
    // A second start while one runs is refused.
    assert!(matches!(client.request(start_move(&code)).await.body, Body::Error(_)));

    // Both show the same digits; both confirm.
    let confirm = phase(&mut client, v1::MovePhase::Confirm).await;
    assert_eq!(confirm.digits.len(), 7, "{confirm:?}");
    assert!(confirm.can_cancel);
    person.answers.send(true).unwrap();
    client.ok(Request::ConfirmMove(v1::ConfirmMove {})).await;

    // It copies by itself, showing what's there as blocks go, not only at a
    // round's end; then it freezes once a round sends little.
    let copying = client.wait_for(|body| match body {
        Body::Workspace(workspace) => workspace
            .r#move
            .clone()
            .filter(|seen| seen.phase() == v1::MovePhase::Copying && seen.bytes_total > 0 && seen.bytes_have < seen.bytes_total),
        _ => None,
    });
    let copying = tokio::time::timeout(WAIT, copying).await.expect("progress within the round");
    assert!(copying.step_text.starts_with("Copying to"), "{copying:?}");
    let frozen = phase(&mut client, v1::MovePhase::Frozen).await;
    assert!(frozen.bytes_total > 0 && frozen.bytes_have == frozen.bytes_total, "{frozen:?}");
    assert!(frozen.target_confirmed);
    let saved = factoryd::move_run::frozen(&home).expect("frozen, for the next start");
    assert_eq!(saved.stage, factoryd::move_run::Stage::Frozen);
    assert!(factoryd::move_export::read_moved(&home).is_none(), "nothing fenced before the last round");

    // What changed meanwhile goes in the last round.
    std::fs::write(home.root().join("agents/ana/notes.md"), "written while frozen\n").unwrap();
    drop(client);
    drop(factory);

    // The supervisor starts it again: frozen mode, serving only the move.
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let frozen_addr = listener.local_addr().unwrap();
    serves(frozen_addr, home.clone());
    let serving = {
        let home = home.clone();
        tokio::spawn(async move { factoryd::move_run::serve_frozen(listener, &home, saved, None).await })
    };
    let (mut client, workspace, _) = Client::connect(frozen_addr).await;
    assert!(workspace.r#move.is_some() && workspace.agents.is_empty(), "only the move: {workspace:?}");
    let refused = client.request(Request::CreateProject(v1::CreateProject { name: "x".into(), ..Default::default() })).await;
    assert!(matches!(refused.body, Body::Error(_)), "frozen mode does nothing else");
    // An agent's command follows it with the tool token, as a tool; a
    // token that isn't one is turned away.
    for (token, welcomed) in [(crate::common::tool_token(frozen_addr), true), ("0123abcd".to_owned(), false)] {
        let mut tool = Client::open(frozen_addr).await;
        let hello = v1::Hello { protocol_version: factory_protocol::PROTOCOL_VERSION, token, ..Default::default() };
        tool.send(v1::ClientMessage { id: 1, body: Some(Request::Hello(hello)) }).await;
        match tool.receive().await.body {
            Some(Body::Welcome(welcome)) if welcomed => assert_eq!(welcome.device_id, "tool"),
            Some(Body::Error(error)) if !welcomed => assert_eq!(error.code(), v1::ErrorCode::Unauthorized),
            other => panic!("{other:?}"),
        }
    }
    let done = phase(&mut client, v1::MovePhase::Done).await;
    assert!(!done.can_cancel);
    tokio::time::timeout(WAIT, serving).await.expect("frozen mode ends").unwrap();

    // The target took it and started; this copy is fenced, saying where it went.
    let text = tokio::time::timeout(WAIT, receiving).await.expect("in time").unwrap().expect("received");
    assert!(text.contains("Imported the Claktory"), "{text}\n{}", person.said());
    assert_eq!(std::fs::read_to_string(there.root().join("agents/ana/notes.md")).unwrap(), "written while frozen\n");
    let moved = factoryd::move_export::read_moved(&home).expect("moved.json");
    assert!(!moved.to.is_empty() && moved.to_os == std::env::consts::OS, "{moved:?}");
    assert!(!home.root().join("remote").exists() && home.root().join("moved/remote").exists(), "fenced");
    assert!(!home.root().join("moving").exists());
    assert!(factoryd::move_run::frozen(&home).is_none(), "the next start is moved mode");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_move_called_off_at_the_digits_changes_nothing_and_a_declined_one_says_why() {
    factoryd::move_run::hold_at_freeze();
    let source = temp_dir();
    let (addr, _factory) = start(&source).await;
    let home = home_of(addr);
    let (mut client, _, _) = Client::connect(addr).await;
    let target = temp_dir();
    let there = Home::new(target.join("home"));
    let (_receiving, mut person) = receive(&there);

    // Called off here: no move left, nothing kept.
    let code = person.code().await;
    client.ok(start_move(&code)).await;
    phase(&mut client, v1::MovePhase::Confirm).await;
    client.ok(Request::CancelMove(v1::CancelMove {})).await;
    client
        .wait_for(|body| match body {
            Body::Workspace(workspace) if workspace.r#move.is_none() => Some(()),
            _ => None,
        })
        .await;
    assert!(!home.root().join("moving").exists());

    // Declined there, with the new code it shows: failed, saying why, until dismissed.
    let code = person.code().await;
    client.ok(start_move(&code)).await;
    phase(&mut client, v1::MovePhase::Confirm).await;
    person.answers.send(false).unwrap();
    let failed = phase(&mut client, v1::MovePhase::Failed).await;
    assert!(failed.error.contains("digits weren't confirmed"), "{failed:?}");
    assert!(failed.can_cancel);
    // Kept, so a restart still shows why.
    assert!(home.root().join("move-failed.json").exists());
    client.ok(Request::CancelMove(v1::CancelMove {})).await;
    assert!(!home.root().join("moving").exists() && factoryd::move_export::read_moved(&home).is_none());
    assert!(!home.root().join("move-failed.json").exists());

    // A code that isn't one is refused at once.
    let refused = client.request(Request::StartMove(v1::StartMove { code: "claktory-move://nonsense".into() })).await;
    assert!(matches!(refused.body, Body::Error(error) if error.message.starts_with("That isn't a receive code")));
}

/// Abbacchio's row 625: a move that fails in its last round (in frozen mode)
/// is shown failed after the restart that follows, still saying who started
/// it and what the other computer is; a failure kept before that still reads.
#[tokio::test(flavor = "multi_thread")]
async fn a_move_that_failed_in_its_last_round_still_says_who_started_it_and_where_to() {
    let source = temp_dir();
    let home = Home::new(source.join("home"));
    std::fs::create_dir_all(home.root()).unwrap();
    // Frozen, with a state whose code can't be dialed: the last round fails.
    let saved = factoryd::move_run::Saved {
        stage: factoryd::move_run::Stage::Frozen,
        code: "claktory-move://damaged".into(),
        target_machine: "hetzner-box".into(),
        target_os: "linux".into(),
        target_arch: "x86_64".into(),
        started_by_agent_id: "rivet".into(),
        ..Default::default()
    };
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    tokio::time::timeout(WAIT, factoryd::move_run::serve_frozen(listener, &home, saved, None)).await.expect("frozen mode ends");

    // The supervisor starts the Claktory again here: the move shows failed.
    let (addr, _factory) = start(&source).await;
    let (_client, workspace, _) = Client::connect(addr).await;
    let failed = workspace.r#move.expect("the failed move");
    assert_eq!(failed.phase(), v1::MovePhase::Failed);
    assert_eq!(
        (failed.target_machine.as_str(), failed.target_os.as_str(), failed.target_arch.as_str(), failed.started_by_agent_id.as_str()),
        ("hetzner-box", "linux", "x86_64", "rivet"),
        "{failed:?}"
    );
    assert!(!failed.error.is_empty() && failed.can_cancel, "{failed:?}");

    // A failure kept by an older Claktory reads as before.
    let older = temp_dir();
    let older_home = Home::new(older.join("home"));
    std::fs::create_dir_all(older_home.root()).unwrap();
    let kept = r#"{"at_ms": 1, "why": "Can't reach box.", "target_machine": "box"}"#;
    std::fs::write(older_home.root().join("move-failed.json"), kept).unwrap();
    let (addr, _older_factory) = start(&older).await;
    let (_client, workspace, _) = Client::connect(addr).await;
    let failed = workspace.r#move.expect("the failed move");
    assert_eq!((failed.phase(), failed.target_machine.as_str(), failed.error.as_str()), (v1::MovePhase::Failed, "box", "Can't reach box."));
    assert_eq!((failed.target_os.as_str(), failed.started_by_agent_id.as_str()), ("", ""));
}

/// Nora's P2-F4: the source restarts mid-copy (a crash, which the supervisor
/// answers within a second) while the target still holds the connection
/// that dropped, and turns new ones away. The source dials again until it
/// lets go, and carries on copying instead of failing.
#[tokio::test(flavor = "multi_thread")]
async fn a_restart_while_copying_carries_on_once_the_target_lets_go_of_the_dropped_connection() {
    use factoryd::move_wire::{self, Frame};
    use iroh::{Endpoint, EndpointAddr, RelayMode, SecretKey, endpoint::presets};
    factoryd::move_run::hold_at_freeze();
    let source = temp_dir();
    let (addr, factory) = start(&source).await;
    let home = home_of(addr);
    let (mut client, _, _) = Client::connect(addr).await;
    let target = temp_dir();
    let there = Home::new(target.join("home"));
    let (_receiving, mut person) = receive(&there);
    let code = person.code().await;
    client.ok(start_move(&code)).await;
    phase(&mut client, v1::MovePhase::Confirm).await;
    person.answers.send(true).unwrap();
    client.ok(Request::ConfirmMove(v1::ConfirmMove {})).await;
    phase(&mut client, v1::MovePhase::Frozen).await;
    drop(client);
    drop(factory);
    // As a crash mid-copy leaves it: still copying.
    let mut saved = factoryd::move_run::saved(&home).expect("saved");
    saved.stage = factoryd::move_run::Stage::Copying;
    std::fs::write(home.root().join("moving/state.json"), serde_json::to_vec(&saved).unwrap()).unwrap();

    // The connection the crash dropped, which the target still holds.
    let key = std::fs::read_to_string(home.root().join("moving/key")).unwrap();
    let key: Vec<u8> = (0..64).step_by(2).map(|at| u8::from_str_radix(&key.trim()[at..at + 2], 16).unwrap()).collect();
    let endpoint = Endpoint::builder(presets::Minimal)
        .relay_mode(RelayMode::Disabled)
        .secret_key(SecretKey::from_bytes(&key.try_into().unwrap()))
        .bind()
        .await
        .expect("bind");
    let mut dialed = EndpointAddr::new(code.endpoint.parse().unwrap());
    for address in &code.addresses {
        dialed = dialed.with_ip_addr(*address);
    }
    let stale = endpoint.connect(dialed, move_wire::ALPN).await.expect("connects");
    let (mut send, mut receive) = stale.open_bi().await.expect("stream");
    let hello = Frame::Hello {
        secret: code.secret.clone(),
        version: factoryd::update::VERSION.to_owned(),
        machine: "Old Mac".to_owned(),
        os: std::env::consts::OS.to_owned(),
        arch: std::env::consts::ARCH.to_owned(),
    };
    move_wire::write(&mut send, &hello, &[]).await.unwrap();
    let welcome = tokio::time::timeout(WAIT, move_wire::read(&mut receive)).await.expect("in time").expect("a frame").0;
    assert!(matches!(welcome, Frame::Welcome { resumed: true, .. }), "{welcome:?}");

    // Started again, it carries on: turned away meanwhile (by a target that
    // waits for the dropped connection to time out), but never failed.
    let (addr, _factory) = start(&source).await;
    factoryd::move_run::resume(&_factory);
    tokio::time::sleep(Duration::from_secs(3)).await;
    let (mut client, workspace, _) = Client::connect(addr).await;
    let waiting = workspace.r#move.expect("a move");
    assert_ne!(waiting.phase(), v1::MovePhase::Failed, "{waiting:?}");
    stale.close(0u32.into(), b"gone");
    endpoint.close().await;
    let frozen = if waiting.phase() == v1::MovePhase::Frozen { waiting } else { phase(&mut client, v1::MovePhase::Frozen).await };
    assert!(frozen.bytes_total > 0 && frozen.bytes_have == frozen.bytes_total, "{frozen:?}");
    assert!(!home.root().join("move-failed.json").exists());
}

/// A move with the account: once this Claktory took the person's
/// approval, the move starts with no code and no digits, never shows the
/// confirm, says on every device which device approved it, and the other
/// computer lets it in as the source it named.
#[tokio::test(flavor = "multi_thread")]
async fn an_approved_move_starts_without_digits_and_says_which_device_approved_it() {
    factoryd::move_run::hold_at_freeze();
    let source = temp_dir();
    let (addr, factory) = start(&source).await;
    let home = home_of(addr);
    let (mut client, _, _) = Client::connect(addr).await;

    let target = temp_dir();
    let there = Home::new(target.join("home"));
    let source_key = factoryd::move_run::source_endpoint_for_test(&home);
    let approved = there.root().join("import/receive/approved.json");
    std::fs::create_dir_all(approved.parent().unwrap()).expect("receive folder");
    std::fs::write(&approved, serde_json::json!({"source_key": source_key}).to_string()).expect("approved");
    let (receiving, mut person) = receive(&there);
    let code = person.code().await;
    assert!(code.secret.is_empty(), "no code to show");
    factoryd::move_run::start_approved_for_test(&factory, &code, "phone", "Matt's iPhone").expect("started");

    // Past the handshake with no confirm, nobody asked anything.
    let copying = phase(&mut client, v1::MovePhase::Copying).await;
    assert!(copying.digits.is_empty() || !copying.confirmed_here, "{copying:?}");
    assert_eq!(
        (copying.approved_by_device_id.as_str(), copying.approved_by_name.as_str(), copying.approval_key.len()),
        ("phone", "Matt's iPhone", 43)
    );
    assert!(person.said().contains("as you approved"), "{}", person.said());
    let saved = factoryd::move_run::saved(&home).expect("the move's state");
    assert!(saved.code.is_empty() && saved.approved.is_some_and(|approved| approved.approver_id == "phone"));
    client.ok(Request::CancelMove(v1::CancelMove {})).await;
    receiving.abort();
}
