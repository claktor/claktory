//! Rivet and moves: no agent starts or confirms a move. move_claktory says
//! how one goes with the account (the person approves it on their device),
//! or that the person moves it themselves without one; move_status follows
//! a move; move_report checks the moved Claktory.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use factory_protocol::v1::{self, AgentStatus, client_message, entry::Kind};
use tokio::sync::mpsc;

use crate::common::{Client, MODEL, Scripted, View, apply_request, create_project, home_of, send_message, start, start_with, temp_dir};
use factoryd::home::Home;
use factoryd::move_import::receive::{Options, Ui};
use factoryd::move_wire::ReceiveCode;

const WAIT: Duration = Duration::from_secs(60);
/// How long a step of a move may take under load: the server may be silent
/// for longer than a client waits for a message (connecting, checking).
const PATIENCE: Duration = Duration::from_secs(300);

/// Follows `view` until `done`, however long the server stays silent
/// meanwhile, up to PATIENCE. Each wait stops before the client's own
/// timeout and starts again; what arrived is already in `view`.
async fn patiently(client: &mut Client, view: &mut View, mut done: impl FnMut(&View) -> bool) {
    let until = std::time::Instant::now() + PATIENCE;
    loop {
        match tokio::time::timeout(Duration::from_secs(30), view.follow(client, &mut done)).await {
            Ok(()) => return,
            Err(_) if std::time::Instant::now() < until => continue,
            Err(_) => panic!("still waiting after {PATIENCE:?}"),
        }
    }
}

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
        let options = Options { committed: false, local_only: true, ..Options::default() };
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
    patiently(client, view, |view| view.entries.len() >= before + 3 && idle(view, "rivet")).await;
    view.entries[before..]
        .iter()
        .find_map(|entry| match &entry.kind {
            Some(Kind::ToolCall(call)) => Some(call.output.clone()),
            _ => None,
        })
        .expect("a tool call")
}

/// Rivet can't start a move, with a code or otherwise. Without an
/// account, move_claktory says the person moves it themselves; a code given
/// it is refused as an argument it doesn't take; nothing starts.
#[tokio::test(flavor = "multi_thread")]
async fn rivet_starts_no_move_and_says_the_person_moves_it() {
    factoryd::move_run::hold_at_freeze();
    let source = temp_dir();
    let (addr, _factory) = start(&source).await;
    let (mut client, workspace, _) = Client::connect(addr).await;
    let mut view = View { workspace, ..View::default() };
    let rivet = v1::UpdateAgent { agent_id: "rivet".into(), model: Some(MODEL.into()), effort: Some("off".into()), ..Default::default() };
    apply_request(&mut client, &mut view, client_message::Body::UpdateAgent(rivet)).await;
    apply_request(&mut client, &mut view, client_message::Body::SelectAgent(v1::SelectAgent { agent_id: "rivet".into() })).await;

    let target = temp_dir();
    let (mut codes, _there) = receive(&Home::new(target.join("home")));
    let code = tokio::time::timeout(WAIT, codes.recv()).await.expect("a code in time").expect("a code");
    assert_eq!(call(&mut client, &mut view, "move_status", "{}").await, "No move is under way.");
    let how = call(&mut client, &mut view, "move_claktory", "{}").await;
    assert!(how.contains("isn't signed in to a Claktory account") && how.contains("moves it themselves"), "{how}");
    let with_code = call(&mut client, &mut view, "move_claktory", &serde_json::json!({"code": code.text()}).to_string()).await;
    assert!(!with_code.contains("Connected"), "{with_code}");
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert!(view.workspace.r#move.is_none(), "a move started: {:?}", view.workspace.r#move);
    assert_eq!(call(&mut client, &mut view, "move_status", "{}").await, "No move is under way.");
}

/// On the new computer, move_report says how the move went (#243 S3):
/// where from, the vault, the account, devices, folders, each sign-in
/// (a usage read), missing tools, and what of each project needs a Mac.
#[tokio::test(flavor = "multi_thread")]
async fn rivet_reports_how_the_move_went_on_the_new_computer() {
    let dir = temp_dir();
    let (addr, _factory) = start_with(&dir, Scripted::subscribed()).await;
    let home = home_of(addr);
    let (mut client, workspace, _) = Client::connect(addr).await;
    let mut view = View { workspace, ..View::default() };
    let rivet = v1::UpdateAgent { agent_id: "rivet".into(), model: Some(MODEL.into()), effort: Some("off".into()), ..Default::default() };
    apply_request(&mut client, &mut view, client_message::Body::UpdateAgent(rivet)).await;
    apply_request(&mut client, &mut view, client_message::Body::SelectAgent(v1::SelectAgent { agent_id: "rivet".into() })).await;

    // Nothing moved here yet.
    let none = call(&mut client, &mut view, "move_report", "{}").await;
    assert!(none.starts_with("This Claktory hasn't moved here"), "{none}");

    // A move recorded as import leaves it, a project with an iOS app, and a
    // task that used a tool this computer hasn't got.
    let site = dir.join("site");
    std::fs::create_dir_all(site.join("ios")).expect("an iOS app");
    apply_request(&mut client, &mut view, create_project("Site", &site)).await;
    apply_request(&mut client, &mut view, client_message::Body::SelectAgent(v1::SelectAgent { agent_id: "rivet".into() })).await;
    let db = rusqlite::Connection::open(home.db_path()).expect("the records");
    let moved = factoryd::transcript::now_ms() - 60_000;
    // As import keeps it: the longest first, the old home to this user's.
    let user_home = std::env::home_dir().expect("a home folder").display().to_string();
    let map = serde_json::json!([
        ["/Users/me/.factory", format!("{user_home}/.factory")],
        ["/Volumes/Work", format!("{user_home}/moved-from-old-mac/Volumes/Work")],
        ["/Users/me", user_home],
    ]);
    db.execute(
        "INSERT INTO moves (export_id, machine, version, epoch, map, imported, told, finished) VALUES ('x1', 'Old Mac', 'v0.1.16', 1, ?1, ?2, ?2, ?2)",
        rusqlite::params![map.to_string(), moved],
    )
    .expect("a move");
    db.execute(
        "INSERT INTO tasks (agent, task, command, pid, log, started) VALUES ('rivet', 'task-1', 'notarytool submit app.zip', 1, '', 0)",
        [],
    )
    .expect("a task");
    drop(db);

    let report = call(&mut client, &mut view, "move_report", "{}").await;
    assert!(report.starts_with("Claktory moved here from Old Mac (v0.1.16) at "), "{report}");
    assert!(report.contains("\nSecrets: "), "{report}");
    assert!(report.contains("\nAccount: this Claktory isn't signed in to claktory.com. Sign in from Settings → Account."), "{report}");
    assert!(report.contains("\nDevices: none added."), "{report}");
    assert!(
        report.contains(&format!(
            "\nFolders: all 1 of the projects' folders are in place.\nHome folder: /Users/me is now {user_home}.\nAlso moved: /Volumes/Work is now {user_home}/moved-from-old-mac/Volumes/Work.\n"
        )),
        "{report}"
    );
    assert!(report.contains("works here."), "a usage read: {report}");
    assert!(report.contains("\nNot installed here: notarytool (Rivet uses it)."), "{report}");
    assert!(report.contains("\n\nFor most projects this just works"), "{report}");
    let mac = std::env::consts::OS == "macos";
    assert_eq!(report.contains("\nSite has an iOS app, which build only on a Mac"), !mac, "{report}");

    // A folder that isn't where the move put it is named.
    std::fs::remove_dir_all(&site).expect("gone");
    let report = call(&mut client, &mut view, "move_report", "{}").await;
    assert!(report.contains(&format!("\nFolders: 1 of 1 isn't here: Site ({}).", site.display())), "{report}");
}

/// A sign-in the provider refuses on the new computer (#243 S8, row 627):
/// move_report's usage read finds it refused and says so on its own line,
/// in words that say what to do, and the others still report.
#[tokio::test(flavor = "multi_thread")]
async fn rivet_reports_a_sign_in_refused_after_the_move() {
    let dir = temp_dir();
    let (addr, _factory) = start_with(&dir, Scripted::subscribed().refusing_usage()).await;
    let home = home_of(addr);
    let (mut client, workspace, _) = Client::connect(addr).await;
    let mut view = View { workspace, ..View::default() };
    let rivet = v1::UpdateAgent { agent_id: "rivet".into(), model: Some(MODEL.into()), effort: Some("off".into()), ..Default::default() };
    apply_request(&mut client, &mut view, client_message::Body::UpdateAgent(rivet)).await;
    apply_request(&mut client, &mut view, client_message::Body::SelectAgent(v1::SelectAgent { agent_id: "rivet".into() })).await;
    let db = rusqlite::Connection::open(home.db_path()).expect("the records");
    let moved = factoryd::transcript::now_ms() - 60_000;
    db.execute(
        "INSERT INTO moves (export_id, machine, version, epoch, map, imported, told, finished) VALUES ('x1', 'Old Mac', 'v0.1.16', 1, '[]', ?1, ?1, ?1)",
        [moved],
    )
    .expect("a move");
    drop(db);

    let report = call(&mut client, &mut view, "move_report", "{}").await;
    assert!(report.starts_with("Claktory moved here from Old Mac (v0.1.16) at "), "{report}");
    assert!(
        report.contains("\nSign-in: Scripted was refused here: sign in again in Settings → Providers (Needs you shows it too).\n"),
        "{report}"
    );
    assert!(!report.contains("works here"), "{report}");
    assert!(report.contains("\n\nFor most projects this just works"), "{report}");
}

/// A folder outside the home folder doesn't stop a move (Abbacchio's S8):
/// the plan says where it goes before the person confirms, and the other
/// computer takes the plan. The person starts it now.
#[tokio::test(flavor = "multi_thread")]
async fn a_move_takes_folders_outside_the_home_folder_under_the_other_computers_home() {
    factoryd::move_run::hold_at_freeze();
    let source = temp_dir();
    let (addr, _factory) = start(&source).await;
    let (mut client, workspace, _) = Client::connect(addr).await;
    let mut view = View { workspace, ..View::default() };
    // Outside this user's home folder, as a temporary folder is.
    let assets = source.join("assets");
    std::fs::create_dir_all(&assets).expect("assets");
    std::fs::write(assets.join("logo.svg"), "<svg/>").expect("a file");
    let user_home = std::env::home_dir().expect("a home folder");
    assert!(!assets.starts_with(&user_home), "the test needs a folder outside {}", user_home.display());
    apply_request(&mut client, &mut view, create_project("Web", &assets)).await;

    let target = temp_dir();
    let (mut codes, there) = receive(&Home::new(target.join("home")));
    let code = tokio::time::timeout(WAIT, codes.recv()).await.expect("a code in time").expect("a code");
    apply_request(&mut client, &mut view, client_message::Body::StartMove(v1::StartMove { code: code.text() })).await;
    patiently(&mut client, &mut view, |view| view.workspace.r#move.as_ref().is_some_and(|seen| seen.phase() == v1::MovePhase::Confirm))
        .await;
    let planned = view.workspace.r#move.clone().expect("a move").map;
    let to = |map: &[v1::MovePath]| map.iter().find(|path| path.from == assets.display().to_string()).map(|path| path.to.clone());
    let goes = to(&planned).expect("where assets goes");
    assert!(goes.starts_with(&format!("{}/moved-from-", user_home.display())) && goes.ends_with(&assets.display().to_string()), "{goes}");

    // Confirmed on both sides, the other computer takes the plan: no
    // problem, and the same place for the folder.
    apply_request(&mut client, &mut view, client_message::Body::ConfirmMove(v1::ConfirmMove {})).await;
    there.send(true).unwrap();
    patiently(&mut client, &mut view, |view| {
        view.workspace
            .r#move
            .as_ref()
            .is_some_and(|seen| seen.phase() == v1::MovePhase::Failed || (seen.phase() == v1::MovePhase::Copying && seen.plan_bytes > 0))
    })
    .await;
    let copying = view.workspace.r#move.clone().expect("a move");
    assert_eq!(copying.phase(), v1::MovePhase::Copying, "{copying:?}");
    assert!(copying.problems.is_empty(), "{copying:?}");
    assert_eq!(to(&copying.map), Some(goes));
    // It holds at the freeze (hold_at_freeze), before anything goes in place.
}
