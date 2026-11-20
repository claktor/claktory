//! Tasks: agents see and stop background tasks, their own or, with
//! see_all, everyone's; the owner hears who stopped one. Admin agents hear
//! of tasks that run long, once.

use crate::common::{Client, View, apply_request, create_agent, create_project, describe, send_message, start, temp_dir};
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
