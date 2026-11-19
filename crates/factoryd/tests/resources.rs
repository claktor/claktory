//! The Resources extension: the machine and Factory's processes sampled,
//! and admin agents told once when memory or disk runs low.

mod common;

use std::time::Duration;

use common::{Client, MODEL, View, apply_request, create_agent, create_project, send_message, start, temp_dir};
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
    assert!(summary.starts_with("Machine: CPU ") && summary.contains("\nFactory: ") && summary.contains("Factory's backend"), "{summary}");
    assert!(summary.contains("\nOthers by memory: ") && summary.contains("No alerts."), "{summary}");
    let processes = call(&mut client, &mut view, "rivet", "resources", r#"{"detail": "processes"}"#).await;
    assert!(processes.contains("- Scout's task-1 `sleep 30`: "), "{processes}");
    let history = call(&mut client, &mut view, "rivet", "resources", r#"{"detail": "history"}"#).await;
    assert!(history.lines().count() >= 2, "{history}");
    let disk = call(&mut client, &mut view, "rivet", "resources", r#"{"detail": "disk"}"#).await;
    assert!(disk.contains("Volume /") && disk.contains("Factory's data"), "{disk}");

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
    assert!(machine.volumes.iter().any(|volume| volume.uses.contains(&"Factory's data".to_owned())));
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
    // …again after it recovered.
    apply_request(&mut client, &mut view, settings(v1::UpdateSettings { ram_percent: Some(100), ..Default::default() })).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
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
