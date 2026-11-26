//! Tasks: agents see and stop background tasks, their own or, with
//! see_all, everyone's; the owner hears who stopped one. Admin agents hear
//! of tasks that run long, once.

use crate::common::{Client, View, apply_request, create_agent, create_project, describe, home, send_message, start, temp_dir};
use factory_protocol::v1::{self, AgentStatus, client_message, entry::Kind};

fn select(agent: &str) -> client_message::Body {
    client_message::Body::SelectAgent(v1::SelectAgent { agent_id: agent.into() })
}

fn idle(view: &View, agent: &str) -> bool {
    let agent = view.agent(agent);
    agent.status() == AgentStatus::Idle && agent.queued_messages == 0
}

/// What `agent`'s call of `tool` with `args` answered.
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

fn events(view: &View) -> Vec<String> {
    describe(&view.entries).into_iter().filter(|line| line.starts_with("event from")).collect()
}

#[tokio::test]
async fn agents_list_and_stop_background_tasks_and_admins_hear_of_long_ones() {
    let dir = temp_dir();
    let (addr, factory) = start(&dir).await;
    let (mut client, workspace, _) = Client::connect(addr).await;
    let mut view = View { workspace, ..View::default() };
    apply_request(&mut client, &mut view, create_project("Site", &dir.join("site"))).await;
    apply_request(&mut client, &mut view, create_agent("site", "Builder", "")).await;
    apply_request(&mut client, &mut view, create_agent("site", "Writer", "")).await;
    let rivet = v1::UpdateAgent {
        agent_id: "rivet".into(),
        model: Some(crate::common::MODEL.into()),
        effort: Some("off".into()),
        ..Default::default()
    };
    apply_request(&mut client, &mut view, client_message::Body::UpdateAgent(rivet)).await;

    // On by default: Builder starts a server and sees it running.
    assert_eq!(call(&mut client, &mut view, "builder", "tasks_list", "{}").await, "No background tasks are running.");
    let started = call(&mut client, &mut view, "builder", "bash", r#"{"command":"sleep 30","run_in_background":true}"#).await;
    assert!(started.starts_with("Started task-1"), "{started}");
    assert_eq!(call(&mut client, &mut view, "builder", "tasks_list", "{}").await, "task-1 builder: sleep 30 — running <1m");
    // Its header counts it (area 4 §8.9); no one else's does.
    view.follow(&mut client, |view| view.agent("builder").tasks_running == 1).await;
    assert_eq!(view.agent("writer").tasks_running, 0);

    // Others see only their own, unless they see all.
    assert_eq!(call(&mut client, &mut view, "writer", "tasks_list", r#"{"agent":"builder"}"#).await, "You can see only your own tasks.");
    assert_eq!(call(&mut client, &mut view, "rivet", "tasks_list", r#"{"agent":"*"}"#).await, "task-1 builder: sleep 30 — running <1m");

    // Admin agents hear of a task that runs long, once.
    let db = rusqlite::Connection::open(dir.join("home/factory.db")).expect("db");
    db.execute("UPDATE tasks SET started = started - 25 * 3600000", []).expect("backdate");
    factory.set_task_check_every(std::time::Duration::from_millis(50));
    apply_request(&mut client, &mut view, select("rivet")).await;
    view.follow(&mut client, |view| events(view).iter().any(|line| line.contains("has been running")) && idle(view, "rivet")).await;
    let told = events(&view);
    assert_eq!(told.len(), 1, "{told:?}");
    assert!(told[0].starts_with("event from Claktory: Builder's task-1 `sleep 30` has been running 25h (log: "), "{told:?}");
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    assert_eq!(events(&view).len(), 1, "once");

    // Rivet stops it; Builder hears who did.
    assert_eq!(
        call(&mut client, &mut view, "rivet", "tasks_stop", r#"{"task":"task-1","agent":"builder"}"#).await,
        "Stopping builder's task-1; builder gets an event when it ends."
    );
    apply_request(&mut client, &mut view, select("builder")).await;
    view.follow(&mut client, |view| events(view).iter().any(|line| line.contains("task-1 finished")) && idle(view, "builder")).await;
    assert!(events(&view).iter().any(|line| line.contains("task-1 finished (stopped by Rivet, ")), "{:?}", events(&view));
    view.follow(&mut client, |view| view.agent("builder").tasks_running == 0).await;
    let all = call(&mut client, &mut view, "builder", "tasks_list", r#"{"all":true}"#).await;
    assert!(all.starts_with("task-1 builder: sleep 30 — stopped by Rivet after"), "{all}");
    assert!(call(&mut client, &mut view, "builder", "tasks_stop", r#"{"task":"task-1"}"#).await.contains("no running task"));
}

#[tokio::test]
async fn the_tasks_panel_shows_tasks_with_their_output_and_the_person_stops_one() {
    let dir = temp_dir();
    let (addr, _factory) = start(&dir).await;
    let (mut client, workspace, _) = Client::connect(addr).await;
    let mut view = View { workspace, ..View::default() };
    apply_request(&mut client, &mut view, create_project("Site", &dir.join("site"))).await;
    apply_request(&mut client, &mut view, create_agent("site", "Builder", "")).await;
    let command = r#"{"command":"echo one; echo two; sleep 30","run_in_background":true}"#;
    call(&mut client, &mut view, "builder", "bash", command).await;
    let open = v1::OpenPanel { extension_id: "tasks".into(), agent_id: "builder".into() };
    apply_request(&mut client, &mut view, client_message::Body::OpenPanel(open)).await;
    let tasks = |view: &View| match view.extension_view.as_ref().and_then(|view| view.body.clone()) {
        Some(v1::extension_view::Body::Tasks(tasks)) => tasks.tasks,
        _ => Vec::new(),
    };

    // Running, with its output so far.
    view.follow(&mut client, |view| tasks(view).first().is_some_and(|task| task.log_tail == "one\ntwo")).await;
    let task = &tasks(&view)[0];
    assert_eq!((task.id.as_str(), task.agent_id.as_str(), task.running, task.ended), ("task-1", "builder", true, 0));
    assert!(task.command.starts_with("echo one") && task.log_path.ends_with("task-1.log"), "{task:?}");

    // The person stops it: the panel shows how it ended, and Builder hears.
    apply_request(
        &mut client,
        &mut view,
        client_message::Body::StopTask(v1::StopTask { agent_id: "builder".into(), task_id: "task-1".into() }),
    )
    .await;
    view.follow(&mut client, |view| tasks(view).first().is_some_and(|task| !task.running)).await;
    let task = &tasks(&view)[0];
    assert_eq!(task.status, "stopped by the person");
    assert!(task.ended >= task.started && task.ended > 0, "{task:?}");
    view.follow(&mut client, |view| events(view).iter().any(|line| line.contains("task-1 finished")) && idle(view, "builder")).await;
    assert!(events(&view).iter().any(|line| line.contains("task-1 finished (stopped by the person, ")), "{:?}", events(&view));

    // One that isn't running can't be stopped.
    let again = v1::StopTask { agent_id: "builder".into(), task_id: "task-1".into() };
    let reply = client.request(client_message::Body::StopTask(again)).await;
    assert!(matches!(reply.body, v1::server_message::Body::Error(error) if error.message.contains("no running task")));
}

#[tokio::test]
async fn an_agents_commands_and_background_tasks_know_its_id() {
    let dir = temp_dir();
    let (addr, _factory) = start(&dir).await;
    let (mut client, workspace, _) = Client::connect(addr).await;
    let mut view = View { workspace, ..View::default() };
    apply_request(&mut client, &mut view, create_project("Site", &dir.join("site"))).await;
    apply_request(&mut client, &mut view, create_agent("site", "Builder", "")).await;
    apply_request(&mut client, &mut view, create_agent("site", "Writer", "")).await;

    let shown = call(&mut client, &mut view, "builder", "bash", r#"{"command":"echo \"id=$FACTORY_AGENT_ID\""}"#).await;
    assert!(shown.starts_with("id=builder\n"), "{shown}");
    let shown = call(&mut client, &mut view, "writer", "bash", r#"{"command":"echo \"id=$FACTORY_AGENT_ID\""}"#).await;
    assert!(shown.starts_with("id=writer\n"), "{shown}");

    let started = call(&mut client, &mut view, "builder", "bash", r#"{"command":"echo $FACTORY_AGENT_ID > background-id","run_in_background":true}"#).await;
    assert!(started.starts_with("Started task-"), "{started}");
    let file = dir.join("site/background-id");
    for _ in 0..100 {
        if std::fs::read_to_string(&file).is_ok_and(|text| text.ends_with('\n')) {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    assert_eq!(std::fs::read_to_string(&file).expect("the task wrote its id"), "builder\n");
}

/// ripgrep for every agent: `<home>/tools` is first on the PATH of its
/// commands and background tasks; while it isn't there (no release, as here
/// at first), it's passed over and the rest of the PATH works as before.
#[tokio::test]
async fn an_agents_commands_find_claktorys_own_tools_first() {
    let dir = temp_dir();
    let (addr, _factory) = start(&dir).await;
    let (mut client, workspace, _) = Client::connect(addr).await;
    let mut view = View { workspace, ..View::default() };
    apply_request(&mut client, &mut view, create_project("Site", &dir.join("site"))).await;
    apply_request(&mut client, &mut view, create_agent("site", "Builder", "")).await;
    let tools = home(&dir).root().join("tools");

    let shown = call(&mut client, &mut view, "builder", "bash", r#"{"command":"ls / > /dev/null && echo listed"}"#).await;
    assert!(shown.starts_with("listed\n"), "no tools folder yet, the rest of the PATH as before: {shown}");

    std::fs::create_dir_all(&tools).expect("tools");
    std::fs::write(tools.join("rg"), "#!/bin/sh\necho \"Claktory's rg $*\"\n").expect("rg");
    std::fs::set_permissions(tools.join("rg"), std::os::unix::fs::PermissionsExt::from_mode(0o755)).expect("mode");
    let shown = call(&mut client, &mut view, "builder", "bash", r#"{"command":"rg --version; echo \"path=$PATH\""}"#).await;
    assert!(shown.starts_with("Claktory's rg --version\n"), "{shown}");
    assert!(shown.contains(&format!("path={}:", tools.display())), "first, then the rest: {shown}");

    let started =
        call(&mut client, &mut view, "builder", "bash", r#"{"command":"rg in-task > background-rg","run_in_background":true}"#).await;
    assert!(started.starts_with("Started task-"), "{started}");
    let file = dir.join("site/background-rg");
    for _ in 0..100 {
        if std::fs::read_to_string(&file).is_ok_and(|text| text.ends_with('\n')) {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    assert_eq!(std::fs::read_to_string(&file).expect("the task ran rg"), "Claktory's rg in-task\n");
}

/// Friend bug #44: the computer restarted, ending two of an idle agent's
/// background tasks. Nothing starts them again; the agent hears of them
/// once, together, with each one's command, folder, start and log, and
/// decides itself. A task that's still running isn't listed, and stays
/// watched.
#[tokio::test]
async fn after_a_restart_an_agent_hears_once_of_every_task_it_ended() {
    let dir = temp_dir();
    let (addr, _old) = start(&dir).await;
    let (mut client, workspace, _) = Client::connect(addr).await;
    let mut view = View { workspace, ..View::default() };
    apply_request(&mut client, &mut view, create_project("Site", &dir.join("site"))).await;
    apply_request(&mut client, &mut view, create_agent("site", "Builder", "")).await;
    for command in ["sleep 300", "printf 'listening on 3000\\n'; sleep 300", "sleep 301"] {
        let args = serde_json::json!({"command": command, "run_in_background": true}).to_string();
        let started = call(&mut client, &mut view, "builder", "bash", &args).await;
        assert!(started.starts_with("Started task-"), "{started}");
    }
    // The first two began before the computer last started: its restart
    // ended them. The third survived.
    let db = rusqlite::Connection::open(dir.join("home/factory.db")).expect("db");
    db.execute("UPDATE tasks SET started = 1000 WHERE task IN ('task-1', 'task-2')", []).expect("backdate");
    let pids: Vec<i64> =
        db.prepare("SELECT pid FROM tasks").unwrap().query_map([], |row| row.get(0)).unwrap().map(Result::unwrap).collect();

    let (addr, _new) = start(&dir).await;
    let (mut client, workspace, _) = Client::connect(addr).await;
    let mut view = View { workspace, ..View::default() };
    apply_request(&mut client, &mut view, select("builder")).await;
    view.follow(&mut client, |view| events(view).iter().any(|line| line.contains("restarted")) && idle(view, "builder")).await;
    let notes: Vec<&v1::Event> = view
        .entries
        .iter()
        .filter_map(|entry| match &entry.kind {
            Some(Kind::Event(event)) if event.source() == v1::EventSource::Factory => Some(event),
            _ => None,
        })
        .collect();
    assert_eq!(notes.len(), 1, "one note: {notes:?}");
    let note = notes[0];
    assert_eq!(note.summary, "The computer restarted, which ended 2 background tasks");
    assert!(note.text.starts_with("The computer restarted, which ended these background tasks you were running. Claktory started none of them again."), "{}", note.text);
    assert!(note.text.contains("isn't stale"), "{}", note.text);
    let site = dir.join("site").display().to_string();
    assert!(note.text.contains("\n- task-1, started ") && note.text.contains(&format!(" in {site}")), "{}", note.text);
    assert!(note.text.contains("\n  Command: sleep 300\n") && note.text.contains("Command: printf 'listening on 3000\\n'; sleep 300"), "{}", note.text);
    assert!(note.text.contains("task-2.log") && !note.text.contains("task-3"), "{}", note.text);
    let ended: Vec<(String, Option<String>)> = db
        .prepare("SELECT task, status FROM tasks ORDER BY id")
        .unwrap()
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    assert_eq!(
        ended,
        [
            ("task-1".to_owned(), Some("the computer restarted".to_owned())),
            ("task-2".to_owned(), Some("the computer restarted".to_owned())),
            ("task-3".to_owned(), None)
        ]
    );
    for pid in pids {
        // SAFETY: ends the test's own sleeps, by their process groups.
        unsafe { libc::kill(-(pid as i32), libc::SIGKILL) };
    }
}
