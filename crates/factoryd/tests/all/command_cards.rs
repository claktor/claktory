//! Execute on command cards (#494, Fugo 35295 and 35326): an agent asks the
//! person to run a command on this computer with ask_person's `command`;
//! only the person's Execute runs it, once, as them, stopped with everything
//! it started at its timeout or when Claktory stops, and the agent is told
//! how it went, secrets hidden.

use crate::common::{Client, View, apply_request, create_agent, create_project, send_message, start, temp_dir};
use factory_protocol::v1::{self, AgentStatus, AskPersonState, CommandRunState, CommandWarningKind, client_message, entry::Kind, server_message::Body};
use serde_json::json;
use std::path::Path;
use std::time::{Duration, Instant};

const SENTINEL: &str = "demo-0123456789abcdef-SENTINEL";

fn idle(view: &View, agent: &str) -> bool {
    let agent = view.agent(agent);
    agent.status() == AgentStatus::Idle && agent.queued_messages == 0
}

/// A Factory with a project "Site" and its agent Builder, selected.
async fn setup(dir: &Path) -> (std::sync::Arc<factoryd::Factory>, Client, View) {
    let (addr, factory) = start(dir).await;
    factory.set_vault_passphrase_cost("argon2id m=1024 t=1 p=1");
    let (mut client, workspace, _) = Client::connect(addr).await;
    let mut view = View { workspace, ..View::default() };
    apply_request(&mut client, &mut view, create_project("Site", &dir.join("site"))).await;
    apply_request(&mut client, &mut view, create_agent("site", "Builder", "")).await;
    let select = client_message::Body::SelectAgent(v1::SelectAgent { agent_id: "builder".into() });
    apply_request(&mut client, &mut view, select).await;
    (factory, client, view)
}

/// Builder calls `tool` with `args`; what it said, and the card it shows.
async fn call(client: &mut Client, view: &mut View, tool: &str, args: serde_json::Value) -> (String, Option<(u64, v1::AskPerson)>) {
    let before = view.entries.len();
    apply_request(client, view, send_message("builder", &format!("call {tool} {args}"))).await;
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

/// Builder asks to run `command`; the card's seq.
async fn ask_to_run(client: &mut Client, view: &mut View, command: &str, timeout: Option<u32>) -> u64 {
    let mut args = json!({"title": "Restart the web server", "body": "It picks up the new certificate.", "command": command});
    if let Some(timeout) = timeout {
        args["timeout"] = json!(timeout);
    }
    let (said, card) = call(client, view, "ask_person", args).await;
    card.unwrap_or_else(|| panic!("no card: {said}")).0
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

fn run(view: &View, seq: u64) -> Option<v1::CommandRun> {
    card(view, seq).command.and_then(|command| command.run)
}

/// Execute, as the app sends it: confirmed.
fn execute(seq: u64) -> client_message::Body {
    client_message::Body::AnswerAsk(v1::AnswerAsk { agent_id: "builder".into(), seq, confirmed: true, ..Default::default() })
}

fn error(reply: &Body) -> String {
    match reply {
        Body::Error(error) => error.message.clone(),
        other => panic!("expected an error, got {other:?}"),
    }
}

/// Follows card `seq` until its run ended; the run.
async fn ended(client: &mut Client, view: &mut View, seq: u64) -> v1::CommandRun {
    view.follow(client, |view| run(view, seq).is_some_and(|run| run.state() != CommandRunState::Running)).await;
    run(view, seq).expect("a run")
}

/// Events from Claktory Builder ran on.
fn events(view: &View) -> Vec<String> {
    view.entries
        .iter()
        .filter_map(|entry| match &entry.kind {
            Some(Kind::Event(event)) if event.source() == v1::EventSource::Factory => Some(event.text.clone()),
            _ => None,
        })
        .collect()
}

/// The `count`th event from Claktory, once Builder ran on it.
async fn told(client: &mut Client, view: &mut View, count: usize) -> String {
    view.follow(client, |view| events(view).len() >= count && idle(view, "builder")).await;
    events(view)[count - 1].clone()
}

/// Whether process group `pgid` (or process, negative) has any process left.
fn alive(pid: i64) -> bool {
    std::process::Command::new("/bin/kill")
        .args(["-0", "--", &pid.to_string()])
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

/// Waits up to 5 s for `pid` to be gone; whether it is.
fn gone(pid: i64) -> bool {
    let until = Instant::now() + Duration::from_secs(5);
    while alive(pid) {
        if Instant::now() > until {
            return false;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    true
}

/// A copy of /bin/sleep at `path`, which runs under its own name. macOS
/// kills a copy of a system binary that keeps Apple's signature, so it's
/// signed again, ad hoc.
fn sleep_named(path: &Path) {
    std::fs::copy("/bin/sleep", path).expect("copy");
    if cfg!(target_os = "macos") {
        let signed = std::process::Command::new("codesign").args(["-f", "-s", "-"]).arg(path).output().expect("codesign");
        assert!(signed.status.success(), "{signed:?}");
    }
}

fn number_in(file: &Path) -> i64 {
    let until = Instant::now() + Duration::from_secs(10);
    loop {
        if let Some(number) = std::fs::read_to_string(file).ok().and_then(|text| text.trim().parse().ok()) {
            return number;
        }
        assert!(Instant::now() < until, "{} never written", file.display());
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn the_person_runs_a_command_once_and_the_agent_hears_how_it_went_secrets_hidden() {
    let dir = temp_dir();
    let (_factory, mut client, mut view) = setup(&dir).await;
    let secret = v1::CreateSecret {
        name: "Demo".into(),
        entries: vec![v1::SecretEntry { variable: "DEMO_TOKEN".into(), value: SENTINEL.into(), file: false }],
        ..Default::default()
    };
    apply_request(&mut client, &mut view, client_message::Body::CreateSecret(secret)).await;

    // The agent never knows a secret's value: the command prints one from a file.
    std::fs::write(dir.join("site/token.txt"), format!("{SENTINEL}\n")).expect("token");
    let command = "printf 'hello\\n'; pwd; env | grep -cE '^(FACTORY|CLAKTORY)_'; echo \"path=$PATH\"; cat token.txt; exit 3".to_owned();
    let args = json!({"title": "Check the build folder", "body": "I can't see it from here.", "command": command});
    let (said, shown) = call(&mut client, &mut view, "ask_person", args).await;
    assert!(said.starts_with("Asked the person to run it on "), "{said}");
    assert!(said.contains("ask_withdraw (id 1)"), "{said}");
    let (seq, shown) = shown.expect("a card");
    assert_eq!((shown.title.as_str(), shown.state(), shown.wants_text), ("Check the build folder", AskPersonState::Pending, false));
    let asked = shown.command.clone().expect("a command card");
    assert_eq!(asked.command, command, "byte for byte");
    assert!(asked.cwd.ends_with("site"), "the agent's folder: {}", asked.cwd);
    assert!(!asked.machine.is_empty());
    assert_eq!((asked.timeout_seconds, asked.warnings.len(), asked.run.is_none()), (120, 0, true));
    assert_eq!(view.agent("builder").waiting_seqs, [seq]);

    // A reply is no answer to it: Execute or dismiss.
    let reply = v1::AnswerAsk { agent_id: "builder".into(), seq, reply: true, text: "Which folder?".into(), ..Default::default() };
    let refused = apply_request(&mut client, &mut view, client_message::Body::AnswerAsk(reply)).await;
    assert_eq!(error(&refused), "This card runs a command: Execute it or dismiss it.");
    // And the agent can't change what it runs.
    let (said, _) = call(&mut client, &mut view, "ask_update", json!({"id": "1", "body": "Another reason."})).await;
    assert!(said.contains("can't be changed"), "{said}");

    let executed = apply_request(&mut client, &mut view, execute(seq)).await;
    assert!(matches!(executed, Body::Ack(_)), "{executed:?}");
    assert_eq!(card(&view, seq).state(), AskPersonState::Answered);
    let ran = ended(&mut client, &mut view, seq).await;
    assert_eq!((ran.state(), ran.exit_code), (CommandRunState::Exited, 3), "{ran:?}");
    let lines: Vec<&str> = ran.output.lines().collect();
    assert_eq!(lines[0], "hello", "{}", ran.output);
    assert!(lines[1].ends_with("/site"), "it ran in its folder: {}", ran.output);
    assert_eq!(lines[2], "0", "none of Claktory's own variables: {}", ran.output);
    assert!(lines[3].starts_with("path=") && lines[3].len() > 5, "{}", ran.output);
    assert_eq!(lines[4], "[secret Demo/DEMO_TOKEN]", "{}", ran.output);
    assert!(!ran.output.contains(SENTINEL));
    assert!(ran.started_ms > 0 && !ran.output_truncated && ran.error.is_empty(), "{ran:?}");

    let text = told(&mut client, &mut view, 1).await;
    assert!(text.starts_with("The person ran your command on "), "{text}");
    assert!(text.contains(": exit code 3, in ") && text.contains("[secret Demo/DEMO_TOKEN]") && text.contains("hello"), "{text}");
    assert!(!text.contains(SENTINEL), "{text}");
    assert!(view.agent("builder").waiting_seqs.is_empty());

    // Once: a second Execute runs nothing.
    let again = apply_request(&mut client, &mut view, execute(seq)).await;
    assert_eq!(error(&again), "This question was already answered.");
}

#[tokio::test(flavor = "multi_thread")]
async fn what_a_card_cant_run_and_who_cant_ask_is_refused() {
    let dir = temp_dir();
    let (factory, mut client, mut view) = setup(&dir).await;
    for (args, refused) in [
        (json!({"title": "T", "body": "B", "command": "echo \u{202e}txt.exe"}), "U+202E"),
        (json!({"title": "T", "body": "B", "command": "echo a\rb"}), "U+000D"),
        (json!({"title": "T", "body": "B", "command": "x".repeat(4001)}), "at most 4000 characters"),
        (json!({"title": "T", "body": "B", "command": "ls", "cwd": "relative/place"}), "give an absolute folder"),
        (json!({"title": "T", "body": "B", "command": "ls", "cwd": "/tmp/\u{202e}txt"}), "cwd: it holds U+202E"),
        (json!({"title": "T", "body": "B", "command": "r\u{E0072}m x"}), "U+E0072"),
        (json!({"title": "T", "body": "B", "command": "ls", "cwd": "/no/such/folder"}), "isn't a folder"),
        (json!({"title": "T", "body": "B", "command": "ls", "wants_text": true}), "command goes alone"),
        (json!({"title": "T", "body": "B", "command": "ls", "steps": ["a", "b"]}), "command goes alone"),
    ] {
        let (said, card) = call(&mut client, &mut view, "ask_person", args).await;
        assert!(said.contains(refused) && said.contains("Nothing was asked."), "{refused}: {said}");
        assert!(card.is_none());
    }
    let longest = json!({"title": "T", "body": "B", "command": "x".repeat(4000)});
    assert!(call(&mut client, &mut view, "ask_person", longest).await.1.is_some(), "4000 characters is fine");

    // An agent that has read from outside can't ask.
    factoryd::pipes::taint_for_test(&factory, "builder", "browser:https://example.com");
    let (said, card) = call(&mut client, &mut view, "ask_person", json!({"title": "T", "body": "B", "command": "ls"})).await;
    assert!(said.contains("You've read a page on https://example.com") && said.contains("can't ask the person to run a command"), "{said}");
    assert!(card.is_none());

    // Nor one limited to a pipe's work.
    apply_request(&mut client, &mut view, create_agent("site", "Ingrid", "")).await;
    factoryd::pipes::make_curator_for_test(&factory, "ingrid", false).await.expect("a curator");
    let select = client_message::Body::SelectAgent(v1::SelectAgent { agent_id: "ingrid".into() });
    apply_request(&mut client, &mut view, select).await;
    let before = view.entries.len();
    let args = json!({"title": "T", "body": "B", "command": "ls"});
    apply_request(&mut client, &mut view, send_message("ingrid", &format!("call ask_person {args}"))).await;
    view.follow(&mut client, |view| view.entries.len() >= before + 3 && idle(view, "ingrid")).await;
    let said = view.entries[before..].iter().find_map(|entry| match &entry.kind {
        Some(Kind::ToolCall(call)) => Some(call.output.clone()),
        _ => None,
    });
    let said = said.expect("a tool call");
    assert!(said.contains("You can't ask the person to run a command. Nothing was asked."), "{said}");
}

/// Only an app that shows command cards runs one; a command has no
/// terminal and nothing on its input, so a prompt that needs either (a
/// terminal password, a host key) fails at once rather than waiting. (No
/// sudo here: with Touch ID for sudo it would ask the Mac's sensor.)
#[tokio::test(flavor = "multi_thread")]
async fn only_an_app_that_shows_the_command_runs_it_and_nothing_waits_for_a_terminal() {
    let dir = temp_dir();
    let (_factory, mut client, mut view) = setup(&dir).await;
    let seq = ask_to_run(&mut client, &mut view, "ls ~/.ssh/claktory-no-such-key; tty; read line; echo \"read=$?\"", Some(60)).await;
    let warnings = card(&view, seq).command.expect("a command").warnings;
    assert_eq!(warnings.iter().map(v1::CommandWarning::kind).collect::<Vec<_>>(), [CommandWarningKind::Sensitive]);
    assert_eq!(warnings[0].text, "Touches Claktory's data, ssh keys or the keychain.");

    // An app that doesn't know command cards approves without `confirmed`:
    // it showed only the title and body, so nothing runs.
    let approve = v1::AnswerAsk { agent_id: "builder".into(), seq, ..Default::default() };
    let unconfirmed = apply_request(&mut client, &mut view, client_message::Body::AnswerAsk(approve)).await;
    assert_eq!(error(&unconfirmed), "This card runs a command, which this version of the app can't show: update Claktory to run it, or dismiss it.");
    assert_eq!(card(&view, seq).state(), AskPersonState::Pending, "still waiting");
    let executed = apply_request(&mut client, &mut view, execute(seq)).await;
    assert!(matches!(executed, Body::Ack(_)), "{executed:?}");
    let ran = ended(&mut client, &mut view, seq).await;
    assert_eq!(ran.state(), CommandRunState::Exited, "{ran:?}");
    assert!(ran.output.contains("not a tty") && ran.output.contains("read=1"), "{ran:?}");
    assert!(ran.duration_ms < 20_000, "nothing waited: {ran:?}");
    assert!(card(&view, seq).command.expect("a command").warnings.len() == 1, "the warning stays on the card");

    // sudo (never the real one: /bin/sleep, named sudo) shows as waiting
    // for a fingerprint or a password while it runs.
    std::fs::create_dir_all(dir.join("bin")).expect("bin");
    sleep_named(&dir.join("bin/sudo"));
    let seq = ask_to_run(&mut client, &mut view, &format!("'{}' 3; echo done", dir.join("bin/sudo").display()), Some(60)).await;
    let warnings = card(&view, seq).command.expect("a command").warnings;
    assert_eq!(warnings.iter().map(v1::CommandWarning::kind).collect::<Vec<_>>(), [CommandWarningKind::Admin]);
    apply_request(&mut client, &mut view, execute(seq)).await;
    view.follow(&mut client, |view| run(view, seq).is_some_and(|run| !run.waiting.is_empty())).await;
    let waiting = run(&view, seq).expect("a run").waiting;
    assert!(waiting.starts_with("Waiting for a fingerprint or password on ") && waiting.ends_with(" (sudo)."), "{waiting}");
    let ran = ended(&mut client, &mut view, seq).await;
    assert!(ran.waiting.is_empty() && ran.output.contains("done"), "{ran:?}");

    // Dismissed, nothing runs and the agent hears it.
    let seq = ask_to_run(&mut client, &mut view, "rm -rf build", None).await;
    let dismiss = v1::AnswerAsk { agent_id: "builder".into(), seq, dismiss: true, ..Default::default() };
    assert!(matches!(apply_request(&mut client, &mut view, client_message::Body::AnswerAsk(dismiss)).await, Body::Ack(_)));
    assert!(run(&view, seq).is_none());
}

/// At its timeout the command is stopped with everything it started, even a
/// child that shrugs off being asked to stop.
#[tokio::test(flavor = "multi_thread")]
async fn a_command_out_of_time_is_stopped_with_everything_it_started() {
    let dir = temp_dir();
    let (_factory, mut client, mut view) = setup(&dir).await;
    let child = dir.join("child");
    let group = dir.join("group");
    let command = format!("echo $$ > '{}'; (trap '' TERM; exec sleep 300) & echo $! > '{}'; echo waiting; wait", group.display(), child.display());
    let seq = ask_to_run(&mut client, &mut view, &command, Some(1)).await;
    apply_request(&mut client, &mut view, execute(seq)).await;
    let ran = ended(&mut client, &mut view, seq).await;
    assert_eq!(ran.state(), CommandRunState::TimedOut, "{ran:?}");
    assert!(ran.output.contains("waiting"), "{ran:?}");
    let (child, group) = (number_in(&child), number_in(&group));
    assert!(gone(child), "the child that ignored SIGTERM is gone");
    assert!(gone(-group), "nothing is left in its group");
    let text = told(&mut client, &mut view, 1).await;
    assert!(text.contains("it was still running after 1 seconds, so it was stopped, with everything it started"), "{text}");

    // A shell that ends at once leaves nothing behind either (Fugo 36500):
    // what it started is asked to stop, then made to.
    let left = dir.join("left");
    let command = format!("(trap '' TERM; exec sleep 300) & echo $! > '{}'; exit 0", left.display());
    let seq = ask_to_run(&mut client, &mut view, &command, Some(60)).await;
    apply_request(&mut client, &mut view, execute(seq)).await;
    let ran = ended(&mut client, &mut view, seq).await;
    assert_eq!((ran.state(), ran.exit_code), (CommandRunState::Exited, 0), "{ran:?}");
    assert!(gone(number_in(&left)), "what the shell left running is gone");
}

/// Claktory stopping stops what a card runs; one that died with a run going
/// stops it at its next start, only when its process group is still the
/// run's. Either way the card shows it cut short, never runs it again, and
/// the agent is told.
#[tokio::test(flavor = "multi_thread")]
async fn a_run_claktory_stopped_during_is_stopped_and_never_a_strangers_group() {
    let dir = temp_dir();
    let (factory, mut client, mut view) = setup(&dir).await;
    let long = |file: &Path| format!("echo $$ > '{}'; echo started; sleep 300", file.display());
    let started = |view: &View, seq: u64| run(view, seq).is_some_and(|run| run.output.contains("started"));

    // Stopping, as at an update.
    let first = dir.join("first");
    let seq = ask_to_run(&mut client, &mut view, &long(&first), Some(600)).await;
    apply_request(&mut client, &mut view, execute(seq)).await;
    view.follow(&mut client, |view| started(view, seq)).await;
    let first = number_in(&first);
    factory.stopping();
    assert!(gone(-first), "stopped with Claktory");
    drop(client);
    drop(factory);
    let (addr, factory) = start(&dir).await;
    let (mut client, workspace, _) = Client::connect(addr).await;
    let mut view = View { workspace, ..View::default() };
    let select = client_message::Body::SelectAgent(v1::SelectAgent { agent_id: "builder".into() });
    apply_request(&mut client, &mut view, select.clone()).await;
    let ran = run(&view, seq).expect("its run");
    assert_eq!(ran.state(), CommandRunState::Interrupted, "{ran:?}");
    assert_eq!(ran.error, "Claktory stopped while it ran; it was stopped.");
    assert!(ran.output.contains("started"), "what it printed stays: {ran:?}");
    let text = told(&mut client, &mut view, 1).await;
    assert!(text.contains("but Claktory stopped while it ran; it was stopped."), "{text}");
    let again = apply_request(&mut client, &mut view, execute(seq)).await;
    assert_eq!(error(&again), "This question was already answered.");

    // Dying with two going: the one still its own is stopped at the next
    // start; one whose group can't be told to be the run's is left alone.
    let (ours, stranger) = (dir.join("ours"), dir.join("stranger"));
    let ours_seq = ask_to_run(&mut client, &mut view, &long(&ours), Some(600)).await;
    let stranger_seq = ask_to_run(&mut client, &mut view, &long(&stranger), Some(600)).await;
    for seq in [ours_seq, stranger_seq] {
        apply_request(&mut client, &mut view, execute(seq)).await;
        view.follow(&mut client, |view| started(view, seq)).await;
    }
    let (ours, stranger) = (number_in(&ours), number_in(&stranger));
    factoryd::abandon_runs_for_test(&factory);
    let db = rusqlite::Connection::open(dir.join("home/factory.db")).expect("factory.db");
    let stranger_id: i64 = card(&view, stranger_seq).id.parse().expect("an id");
    let changed = db.execute("UPDATE ask_runs SET leader_start = '1.000000' WHERE ask = ?1", [stranger_id]);
    assert_eq!(changed.expect("changed"), 1);
    drop(client);
    drop(factory);
    let (addr, _factory) = start(&dir).await;
    let (mut client, workspace, _) = Client::connect(addr).await;
    let mut view = View { workspace, ..View::default() };
    apply_request(&mut client, &mut view, select).await;
    assert!(gone(-ours), "its own group was stopped");
    assert!(alive(-stranger), "a group that may not be the run's is left alone");
    std::process::Command::new("/bin/kill").args(["-9", "--", &(-stranger).to_string()]).status().expect("kill");
    let (ours, stranger) = (run(&view, ours_seq).expect("a run"), run(&view, stranger_seq).expect("a run"));
    assert_eq!((ours.state(), ours.error.as_str()), (CommandRunState::Interrupted, "Claktory stopped while it ran; it was stopped."));
    assert_eq!(
        (stranger.state(), stranger.error.as_str()),
        (CommandRunState::Interrupted, "Claktory stopped while it ran; it may have finished on its own.")
    );
}
