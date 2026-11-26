//! #490 risk 5: factoryd raises its soft limit on open files at start,
//! from launchd's 256 or systemd's 1024 to the hard limit (at most 65,536),
//! and agents' commands inherit it. A file of its own: the limit is the
//! process's.

#[path = "common/mod.rs"]
mod common;

use common::{Client, View, apply_request, create_agent, create_project, send_message, start, temp_dir};
use factory_protocol::v1::{self, AgentStatus, client_message, entry::Kind};
use factoryd::open_files;

/// Has `agent` run `command` with bash and returns what it printed.
async fn run(client: &mut Client, view: &mut View, agent: &str, command: &str) -> String {
    apply_request(client, view, client_message::Body::SelectAgent(v1::SelectAgent { agent_id: agent.into() })).await;
    let before = view.entries.len();
    let args = serde_json::json!({ "command": command });
    apply_request(client, view, send_message(agent, &format!("call bash {args}"))).await;
    view.follow(client, |view| {
        let state = view.agent(agent);
        view.entries.len() >= before + 3 && state.status() == AgentStatus::Idle && state.queued_messages == 0
    })
    .await;
    view.entries[before..]
        .iter()
        .find_map(|entry| match &entry.kind {
            Some(Kind::ToolCall(call)) => Some(call.output.clone()),
            _ => None,
        })
        .expect("a tool call")
}

#[tokio::test]
async fn the_open_files_limit_is_raised_at_start_and_agents_commands_have_it() {
    // As launchd starts a Mac's processes.
    let (_, hard) = open_files::limits().expect("the limits");
    let lowered = libc::rlimit { rlim_cur: 256.min(hard), rlim_max: hard };
    // SAFETY: setrlimit only reads `lowered`.
    assert_eq!(unsafe { libc::setrlimit(libc::RLIMIT_NOFILE, &raw const lowered) }, 0);

    let (before, after) = open_files::raise().expect("raised");
    assert_eq!(before, 256.min(hard));
    assert_eq!(open_files::limits().map(|(soft, _)| soft), Some(after));
    assert!(after <= open_files::MOST, "at most {}: {after}", open_files::MOST);
    // The hard limit, or as near as the system allows (macOS: at least its
    // old OPEN_MAX).
    assert!(after >= hard.min(10_240), "raised from {before} to {after}, hard {hard}");
    // Never lowered, and asking again changes nothing.
    assert_eq!(open_files::raise(), Some((after, after)));

    // Agents' commands inherit it.
    let dir = temp_dir();
    let (addr, _factory) = start(&dir).await;
    let (mut client, workspace, _) = Client::connect(addr).await;
    let mut view = View { workspace, ..View::default() };
    apply_request(&mut client, &mut view, create_project("Site", &dir.join("site"))).await;
    apply_request(&mut client, &mut view, create_agent("site", "Lea", "")).await;
    let said = run(&mut client, &mut view, "lea", "ulimit -n").await;
    assert!(said.lines().any(|line| line.trim() == after.to_string()), "{said}");
}
