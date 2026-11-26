//! The vault's running-task counts (#402, Polpo's factoryd-cpu.md): each
//! grant shows how many of the background tasks that got its secret lately
//! still run. Claktory reads which tasks run once per Workspace, not once
//! per task; the counts are the same as the per-task lookup gave.

use std::path::Path;

use crate::common::{Client, View, apply_request, create_agent, create_project, send_message, start, temp_dir};
use factory_protocol::v1::{self, client_message::Body as Request, entry::Kind};

fn idle(view: &View, agent: &str) -> bool {
    let agent = view.agent(agent);
    agent.status() == v1::AgentStatus::Idle && agent.queued_messages == 0
}

async fn call(client: &mut Client, view: &mut View, agent: &str, tool: &str, args: &str) -> String {
    apply_request(client, view, Request::SelectAgent(v1::SelectAgent { agent_id: agent.into() })).await;
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

/// Each agent's running-task count on the secret's grants, as the
/// Workspace shows them.
fn shown(view: &View) -> Vec<(String, u32)> {
    let vault = view.workspace.vault.clone().expect("vault state");
    let mut counts: Vec<(String, u32)> =
        vault.secrets[0].grants.iter().map(|grant| (grant.agent_id.clone(), grant.running_tasks)).collect();
    counts.sort();
    counts
}

/// The counts the way Claktory read them before: each task that got the
/// secret looked up on its own in factory.db.
fn one_by_one(dir: &Path, used: &[(&str, &[&str])]) -> Vec<(String, u32)> {
    let flags = rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY;
    let db = rusqlite::Connection::open_with_flags(dir.join("home/factory.db"), flags).expect("factory.db");
    let running = |agent: &str, task: &str| {
        db.query_row("SELECT 1 FROM tasks WHERE agent = ?1 AND task = ?2 AND ended IS NULL", [agent, task], |_| Ok(())).is_ok()
    };
    let mut counts: Vec<(String, u32)> = used
        .iter()
        .map(|(agent, tasks)| ((*agent).to_owned(), tasks.iter().filter(|task| running(agent, task)).count() as u32))
        .collect();
    counts.sort();
    counts
}

#[tokio::test]
async fn the_vaults_running_task_counts_match_looking_up_each_task() {
    let dir = temp_dir();
    let (addr, factory) = start(&dir).await;
    factory.set_vault_passphrase_cost("argon2id m=1024 t=1 p=1");
    let (mut client, workspace, _) = Client::connect(addr).await;
    let mut view = View { workspace, ..View::default() };
    apply_request(&mut client, &mut view, create_project("Site", &dir.join("site"))).await;
    apply_request(&mut client, &mut view, create_agent("site", "Builder", "")).await;
    apply_request(&mut client, &mut view, create_agent("site", "Writer", "")).await;
    let secret = v1::CreateSecret {
        name: "Cloudflare".into(),
        entries: vec![v1::SecretEntry { variable: "CLOUDFLARE_API_TOKEN".into(), value: "cf-0123456789abcdef".into(), file: false }],
        ..Default::default()
    };
    apply_request(&mut client, &mut view, Request::CreateSecret(secret)).await;
    let id = view.workspace.vault.clone().expect("vault").secrets[0].id.clone();
    for agent in ["builder", "writer"] {
        let grant = v1::GrantSecret { id: id.clone(), agent_id: agent.into(), proof: None, fill_only: false };
        apply_request(&mut client, &mut view, Request::GrantSecret(grant)).await;
    }

    // Builder: two long tasks with the secret, one that ends at once, and
    // one long task without it; Writer: one long task with it.
    let long = r#"{"command":"sleep 60","run_in_background":true,"secrets":["Cloudflare"]}"#;
    let quick = r#"{"command":"true","run_in_background":true,"secrets":["Cloudflare"]}"#;
    let without = r#"{"command":"sleep 60","run_in_background":true}"#;
    for (agent, args) in [("builder", long), ("builder", quick), ("builder", long), ("builder", without), ("writer", long)] {
        assert!(call(&mut client, &mut view, agent, "bash", args).await.starts_with("Started task-"));
    }
    let used: [(&str, &[&str]); 2] = [("builder", &["task-1", "task-2", "task-3"]), ("writer", &["task-1"])];
    let expected = vec![("builder".to_owned(), 2), ("writer".to_owned(), 1)];
    view.follow(&mut client, |view| shown(view) == expected).await;
    assert_eq!(one_by_one(&dir, &used), expected);

    // One of Builder's ends, and Writer's: the counts follow, the same both ways.
    for (agent, task) in [("builder", "task-1"), ("writer", "task-1")] {
        apply_request(&mut client, &mut view, Request::StopTask(v1::StopTask { agent_id: agent.into(), task_id: task.into() })).await;
    }
    let expected = vec![("builder".to_owned(), 1), ("writer".to_owned(), 0)];
    view.follow(&mut client, |view| shown(view) == expected).await;
    assert_eq!(one_by_one(&dir, &used), expected);
    for task in ["task-3", "task-4"] {
        apply_request(&mut client, &mut view, Request::StopTask(v1::StopTask { agent_id: "builder".into(), task_id: task.into() })).await;
    }
    let expected = vec![("builder".to_owned(), 0), ("writer".to_owned(), 0)];
    view.follow(&mut client, |view| shown(view) == expected).await;
    assert_eq!(one_by_one(&dir, &used), expected);

    // Both reads go through the index of running tasks, never the whole table.
    let db = rusqlite::Connection::open_with_flags(dir.join("home/factory.db"), rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY).expect("db");
    for sql in [
        "SELECT agent, task FROM tasks WHERE ended IS NULL",
        "SELECT 1 FROM tasks WHERE agent = 'builder' AND task = 'task-1' AND ended IS NULL",
    ] {
        let plan: String = db.query_row(&format!("EXPLAIN QUERY PLAN {sql}"), [], |row| row.get(3)).expect("plan");
        assert!(plan.contains("USING INDEX tasks_running") || plan.contains("USING COVERING INDEX tasks_running"), "{sql}: {plan}");
    }
}
