//! The Resources extension: the machine and Factory's processes sampled,
//! and admin agents told once when memory or disk runs low.

use std::time::Duration;

use crate::common::{Client, MODEL, View, apply_request, create_agent, create_project, send_message, start, temp_dir};
use factory_protocol::v1::{self, AgentStatus, client_message, entry::Kind, server_message::Body};

fn select(agent: &str) -> client_message::Body {
    client_message::Body::SelectAgent(v1::SelectAgent { agent_id: agent.into() })
}

fn idle(view: &View, agent: &str) -> bool {
    let agent = view.agent(agent);
    agent.status() == AgentStatus::Idle && agent.queued_messages == 0
}

/// Has `agent` call `tool` with `args` and returns what the tool answered.
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

fn settings(change: v1::UpdateSettings) -> client_message::Body {
    client_message::Body::UpdateSettings(change)
}

/// Events from Factory in the selected agent's conversation that start so.
fn told(view: &View, start: &str) -> usize {
    view.entries.iter().filter(|entry| matches!(&entry.kind, Some(Kind::Event(event)) if event.text.starts_with(start))).count()
}

async fn setup() -> (std::path::PathBuf, std::sync::Arc<factoryd::Factory>, Client, View) {
    let dir = temp_dir();
    let (addr, factory) = start(&dir).await;
    factory.set_resources_every(Duration::from_millis(50), Duration::from_millis(300));
    let (mut client, workspace, _) = Client::connect(addr).await;
    let mut view = View { workspace, ..View::default() };
    // Lines no machine crosses, whatever its real load (no memory is above
    // 100%, no disk has less than 0 bytes free, and the processors' line is
    // off), set before the first sample: an alert comes only from a line a
    // test lowers itself.
    let lines = v1::UpdateSettings { ram_percent: Some(100), disk_free_gb: Some(0), cpu_percent: Some(0), ..Default::default() };
    let reply = apply_request(&mut client, &mut view, settings(lines)).await;
    assert!(matches!(reply, Body::Ack(_)), "{reply:?}");
    let rivet = v1::UpdateAgent { agent_id: "rivet".into(), model: Some(MODEL.into()), effort: Some("off".into()), ..Default::default() };
    apply_request(&mut client, &mut view, client_message::Body::UpdateAgent(rivet)).await;
    apply_request(&mut client, &mut view, create_project("Site", &dir.join("site"))).await;
    apply_request(&mut client, &mut view, create_agent("site", "Scout", "")).await;
    (dir, factory, client, view)
}

#[tokio::test]
async fn an_admin_agent_sees_the_machine_and_factorys_processes() {
    let (_dir, _factory, mut client, mut view) = setup().await;
    // Rivet has it, as an admin agent; others only once turned on.
    let rivet = view.agent("rivet").extensions.iter().find(|extension| extension.id == "resources").expect("listed").enabled;
    let scout = view.agent("scout").extensions.iter().find(|extension| extension.id == "resources").expect("listed").enabled;
    assert!(rivet && !scout);
    let catalog = view.workspace.extensions.iter().find(|extension| extension.id == "resources").expect("in the catalog");
    assert!(catalog.admin_default && !catalog.default_enabled);

    // Scout's background task is Factory's, and Scout's.
    let started = call(&mut client, &mut view, "scout", "bash", r#"{"command": "sleep 30", "run_in_background": true}"#).await;
    assert!(started.contains("task-1"), "{started}");
    tokio::time::sleep(Duration::from_millis(800)).await;
    let summary = call(&mut client, &mut view, "rivet", "resources", "{}").await;
    assert!(summary.starts_with("Machine: CPU ") && summary.contains("\nClaktory: ") && summary.contains("Claktory service"), "{summary}");
    assert!(summary.contains("\nOthers by memory: ") && summary.contains("No alerts."), "{summary}");
    let processes = call(&mut client, &mut view, "rivet", "resources", r#"{"detail": "processes"}"#).await;
    assert!(processes.contains("- Scout's task-1 `sleep 30`: "), "{processes}");
    let history = call(&mut client, &mut view, "rivet", "resources", r#"{"detail": "history"}"#).await;
    assert!(history.lines().count() >= 2, "{history}");
    let disk = call(&mut client, &mut view, "rivet", "resources", r#"{"detail": "disk"}"#).await;
    assert!(disk.contains("Volume /") && disk.contains("Claktory's data"), "{disk}");

    // The panel.
    let open = v1::OpenPanel { extension_id: "resources".into(), agent_id: "rivet".into() };
    apply_request(&mut client, &mut view, client_message::Body::OpenPanel(open)).await;
    let panel = |view: &View| match view.extension_view.as_ref().and_then(|view| view.body.clone()) {
        Some(v1::extension_view::Body::Resources(resources)) => resources,
        other => panic!("expected the Resources panel, got {other:?}"),
    };
    view.follow(&mut client, |view| panel(view).measured > 0).await;
    let resources = panel(&view);
    let machine = resources.machine.expect("the machine");
    assert!(machine.memory_total > 0 && machine.cores > 0 && !machine.volumes.is_empty());
    assert!(machine.volumes.iter().any(|volume| volume.uses.contains(&"Claktory's data".to_owned())));
    assert!(resources.factory.iter().any(|process| process.kind == "backend"));
    let task = resources.factory.iter().find(|process| process.kind == "task").expect("Scout's task");
    assert_eq!((task.agent_id.as_str(), task.label.as_str()), ("scout", "task-1 `sleep 30`"));
    assert!(resources.others.len() <= 5 && !resources.history.is_empty());
    let kinds: Vec<&str> = resources.disk.iter().map(|row| row.kind.as_str()).collect();
    assert_eq!(&kinds[..2], ["home", "releases"]);
    assert!(kinds.contains(&"agent") && kinds.contains(&"project"), "{kinds:?}");
}

#[tokio::test]
async fn admin_agents_are_told_once_when_memory_or_disk_runs_low() {
    let (_dir, _factory, mut client, mut view) = setup().await;
    apply_request(&mut client, &mut view, select("rivet")).await;
    let low_memory = "This machine is running low on memory";
    // Every machine uses more than 1% of its memory.
    let reply = apply_request(&mut client, &mut view, settings(v1::UpdateSettings { ram_percent: Some(1), ..Default::default() })).await;
    assert!(matches!(reply, Body::Ack(_)), "{reply:?}");
    assert_eq!(view.workspace.settings.as_ref().map(|settings| settings.ram_percent), Some(1));
    view.follow(&mut client, |view| told(view, low_memory) == 1).await;
    let event = view
        .entries
        .iter()
        .find_map(|entry| match &entry.kind {
            Some(Kind::Event(event)) if event.text.starts_with(low_memory) => Some(event.text.clone()),
            _ => None,
        })
        .expect("told");
    assert!(event.contains("Don't stop, kill or delete anything yourself."), "{event}");
    // Once per crossing…
    tokio::time::sleep(Duration::from_millis(500)).await;
    view.follow(&mut client, |view| idle(view, "rivet")).await;
    assert_eq!(told(&view, low_memory), 1);
    // …again after it recovered: once a sample has seen it recover, which
    // the panel shows (under load a sample can take longer than any fixed wait).
    let open = v1::OpenPanel { extension_id: "resources".into(), agent_id: "rivet".into() };
    apply_request(&mut client, &mut view, client_message::Body::OpenPanel(open)).await;
    let alerts = |view: &View| match view.extension_view.as_ref().and_then(|view| view.body.as_ref()) {
        Some(v1::extension_view::Body::Resources(resources)) => {
            resources.alerts.iter().filter(|alert| alert.starts_with(low_memory)).count()
        }
        _ => usize::MAX,
    };
    view.follow(&mut client, |view| alerts(view) == 1).await;
    apply_request(&mut client, &mut view, settings(v1::UpdateSettings { ram_percent: Some(100), ..Default::default() })).await;
    view.follow(&mut client, |view| alerts(view) == 0).await;
    apply_request(&mut client, &mut view, settings(v1::UpdateSettings { ram_percent: Some(1), ..Default::default() })).await;
    view.follow(&mut client, |view| told(view, low_memory) == 2).await;

    // Disk: less free than all of it, or than 100,000 GB.
    let disk =
        v1::UpdateSettings { ram_percent: Some(100), disk_free_gb: Some(100_000), disk_free_percent: Some(100), ..Default::default() };
    apply_request(&mut client, &mut view, settings(disk)).await;
    view.follow(&mut client, |view| told(view, "The volume /") >= 1).await;
    // Scout doesn't have it.
    apply_request(&mut client, &mut view, select("scout")).await;
    assert_eq!(told(&view, low_memory) + told(&view, "The volume"), 0);

    // Thresholds out of range are refused.
    let refused = apply_request(&mut client, &mut view, settings(v1::UpdateSettings { ram_percent: Some(0), ..Default::default() })).await;
    assert!(matches!(&refused, Body::Error(error) if error.message.contains("ram_percent must be from 1 to 100")), "{refused:?}");
}

/// Friend bug #13: the machine's health is Rivet's. An agent with Resources
/// on that isn't an admin agent can look, but isn't told; with no admin
/// agent to take it, Claktory notifies the person itself, once per crossing.
#[tokio::test]
async fn the_machine_is_rivets_and_without_rivet_claktory_tells_the_person() {
    let (_dir, _factory, mut client, mut view) = setup().await;
    let resources = |on: bool, agent: &str| {
        let change = v1::ExtensionChange { id: "resources".into(), enabled: Some(on), ..Default::default() };
        client_message::Body::UpdateAgent(v1::UpdateAgent { agent_id: agent.into(), extension: Some(change), ..Default::default() })
    };
    assert!(matches!(apply_request(&mut client, &mut view, resources(true, "scout")).await, Body::Ack(_)));
    let low_memory = "This machine is running low on memory";
    apply_request(&mut client, &mut view, select("rivet")).await;
    apply_request(&mut client, &mut view, settings(v1::UpdateSettings { ram_percent: Some(1), ..Default::default() })).await;
    view.follow(&mut client, |view| told(view, low_memory) == 1).await;
    // Scout has the tool, and wasn't told.
    let looked = call(&mut client, &mut view, "scout", "resources", "{}").await;
    assert!(looked.starts_with("Machine: CPU "), "{looked}");
    assert_eq!(told(&view, low_memory), 0);

    // With Resources off for Rivet, no admin agent can take it: once memory
    // recovers and runs low again, Claktory tells the person itself.
    apply_request(&mut client, &mut view, resources(false, "rivet")).await;
    let open = v1::OpenPanel { extension_id: "resources".into(), agent_id: "scout".into() };
    apply_request(&mut client, &mut view, client_message::Body::OpenPanel(open)).await;
    let alerts = |view: &View| match view.extension_view.as_ref().and_then(|view| view.body.as_ref()) {
        Some(v1::extension_view::Body::Resources(resources)) => resources.alerts.len(),
        _ => usize::MAX,
    };
    apply_request(&mut client, &mut view, settings(v1::UpdateSettings { ram_percent: Some(100), ..Default::default() })).await;
    view.follow(&mut client, |view| alerts(view) == 0).await;
    apply_request(&mut client, &mut view, settings(v1::UpdateSettings { ram_percent: Some(1), ..Default::default() })).await;
    let mut notified = Vec::new();
    while notified.is_empty() {
        let message = client.receive().await;
        let body = message.body.expect("body");
        if let Body::Notify(notify) = &body {
            notified.push(notify.clone());
        }
        view.apply(&body);
    }
    let notify = &notified[0];
    assert_eq!(notify.kind(), v1::NotifyKind::Alert);
    assert!(notify.body.starts_with("This computer is running low on memory: "), "{notify:?}");
    assert!(notify.agent_id.is_empty());
    // Once per crossing, and still no agent is told.
    tokio::time::sleep(Duration::from_millis(500)).await;
    view.follow(&mut client, |view| alerts(view) == 1 && idle(view, "scout")).await;
    assert_eq!(told(&view, low_memory), 0);
    apply_request(&mut client, &mut view, select("rivet")).await;
    assert_eq!(told(&view, low_memory), 1, "Rivet heard only the first");
}

/// #455: a disk that's still short after a restart isn't told again: what
/// was told is kept in factory.db until the disk recovers, and a later drop
/// is told again.
#[tokio::test]
async fn a_low_disk_is_told_once_across_restarts_and_again_after_it_recovers() {
    let (dir, factory, mut client, mut view) = setup().await;
    apply_request(&mut client, &mut view, select("rivet")).await;
    let low_disk = "The volume ";
    let open = v1::OpenPanel { extension_id: "resources".into(), agent_id: "rivet".into() };
    apply_request(&mut client, &mut view, client_message::Body::OpenPanel(open.clone())).await;
    let panel = |view: &View| match view.extension_view.as_ref().and_then(|view| view.body.clone()) {
        Some(v1::extension_view::Body::Resources(resources)) => resources,
        other => panic!("expected the Resources panel, got {other:?}"),
    };
    let alerts = |view: &View| panel(view).alerts.iter().filter(|alert| alert.starts_with(low_disk)).count();
    // Less free than all of it, or than 100,000 GB: every volume Claktory uses.
    let short = v1::UpdateSettings { disk_free_gb: Some(100_000), disk_free_percent: Some(100), ..Default::default() };
    apply_request(&mut client, &mut view, settings(short.clone())).await;
    view.follow(&mut client, |view| alerts(view) >= 1 && told(view, low_disk) == alerts(view) && idle(view, "rivet")).await;
    let volumes = alerts(&view);

    // Restart, the disk still short: nothing new once a few samples are in.
    factory.set_resources_every(Duration::from_secs(3600), Duration::from_secs(3600));
    drop(client);
    drop(factory);
    let (addr, factory) = start(&dir).await;
    factory.set_resources_every(Duration::from_millis(50), Duration::from_millis(300));
    let (mut client, workspace, _) = Client::connect(addr).await;
    let mut view = View { workspace, ..View::default() };
    apply_request(&mut client, &mut view, select("rivet")).await;
    apply_request(&mut client, &mut view, client_message::Body::OpenPanel(open)).await;
    view.follow(&mut client, |view| panel(view).history.len() >= 5).await;
    assert_eq!(told(&view, low_disk), volumes, "told again after a restart");
    assert_eq!(alerts(&view), volumes, "the alerts in effect are kept");

    // Recovered, then short again: told again, once.
    apply_request(&mut client, &mut view, settings(v1::UpdateSettings { disk_free_gb: Some(0), ..Default::default() })).await;
    view.follow(&mut client, |view| alerts(view) == 0).await;
    // Kept just after the panel shows it, and a save that failed (the
    // database busy under load) is made again at the next sample.
    let kept = || -> i64 {
        rusqlite::Connection::open(dir.join("home/factory.db"))
            .and_then(|db| db.query_row("SELECT count(*) FROM resource_alerts", [], |row| row.get(0)))
            .expect("factory.db")
    };
    for _ in 0..400 {
        if kept() == 0 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let rows: Vec<(String, i64)> = rusqlite::Connection::open(dir.join("home/factory.db"))
        .and_then(|db| {
            let mut statement = db.prepare("SELECT about, told FROM resource_alerts")?;
            statement.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?.collect()
        })
        .expect("factory.db");
    assert!(rows.is_empty(), "cleared once it recovered: {rows:?}, panel {:?}", panel(&view).alerts);
    apply_request(&mut client, &mut view, settings(short)).await;
    view.follow(&mut client, |view| told(view, low_disk) == 2 * volumes && idle(view, "rivet")).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    view.follow(&mut client, |view| panel(view).history.len() >= 2).await;
    assert_eq!(told(&view, low_disk), 2 * volumes);
}

/// Kills a process when the test ends, however it ends.
struct Killed(String);

impl Drop for Killed {
    fn drop(&mut self) {
        let _ = std::process::Command::new("kill").arg(&self.0).status();
    }
}

/// Part 2 #4 (#454): a process that uses a full core for an hour, owned by
/// no agent's task (a leftover `cat /dev/zero` did for 37 hours), is told
/// to the admin agents once, a restart included; an agent's task isn't.
#[tokio::test]
async fn a_stray_process_on_a_full_core_is_told_once_and_an_agents_task_never() {
    let (dir, factory, mut client, mut view) = setup().await;
    factory.set_resources_stray(Duration::from_millis(400), 5.0);
    // Scout's task: a full core in its own group, for a few seconds.
    // Named in its command line, so no other test's busy process passes for it.
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |since| since.subsec_nanos());
    let marker = format!("task{}x{nanos}", std::process::id());
    let task = serde_json::json!({
        "command": format!("sh -c ': {marker}; while :; do :; done' & p=$!; sleep 4; kill $p"),
        "run_in_background": true,
    });
    let started = call(&mut client, &mut view, "scout", "bash", &task.to_string()).await;
    assert!(started.contains("task-1"), "{started}");
    // A leftover: started by a command that ended, so its parent is gone.
    let spawned = std::process::Command::new("sh").args(["-c", "yes > /dev/null 2>&1 & echo $!"]).output().expect("sh");
    let pid = String::from_utf8_lossy(&spawned.stdout).trim().to_owned();
    let _killed = Killed(pid.clone());
    apply_request(&mut client, &mut view, select("rivet")).await;
    let about = "A process has used a full processor core for over an hour: ";
    let of = |view: &View, pid: &str| {
        view.entries
            .iter()
            .filter_map(|entry| match &entry.kind {
                Some(Kind::Event(event)) if event.text.starts_with(about) && event.text.contains(&format!("(pid {pid},")) => {
                    Some(event.text.clone())
                }
                _ => None,
            })
            .collect::<Vec<_>>()
    };
    let tasks = |view: &View| {
        view.entries.iter().filter(|entry| matches!(&entry.kind, Some(Kind::Event(event)) if event.text.contains(&marker))).count()
    };
    // Every process counts at 5% of a core here: Rivet hears of others too.
    view.follow(&mut client, |view| of(view, &pid).len() == 1).await;
    let event = of(&view, &pid).remove(0);
    assert!(event.contains("`yes`") && event.contains("No agent's task owns it."), "{event}");
    assert!(event.contains("Don't stop, kill or delete anything yourself."), "{event}");
    // Once: more samples, and Scout's task ran out its time, never told.
    tokio::time::sleep(Duration::from_secs(5)).await;
    view.follow(&mut client, |view| view.agent("rivet").queued_messages == 0).await;
    assert_eq!(of(&view, &pid).len(), 1);
    assert_eq!(tasks(&view), 0, "never Scout's task");

    // A restart doesn't tell it again while it runs.
    factory.set_resources_every(Duration::from_secs(3600), Duration::from_secs(3600));
    drop(client);
    drop(factory);
    let (addr, factory) = start(&dir).await;
    factory.set_resources_every(Duration::from_millis(50), Duration::from_millis(300));
    factory.set_resources_stray(Duration::from_millis(400), 5.0);
    let (mut client, workspace, _) = Client::connect(addr).await;
    let mut view = View { workspace, ..View::default() };
    apply_request(&mut client, &mut view, select("rivet")).await;
    let open = v1::OpenPanel { extension_id: "resources".into(), agent_id: "rivet".into() };
    apply_request(&mut client, &mut view, client_message::Body::OpenPanel(open)).await;
    let samples = |view: &View| match view.extension_view.as_ref().and_then(|view| view.body.as_ref()) {
        Some(v1::extension_view::Body::Resources(resources)) => resources.history.len(),
        _ => 0,
    };
    view.follow(&mut client, |view| samples(view) >= 20).await;
    assert_eq!(of(&view, &pid).len(), 1, "told again after a restart");
}
