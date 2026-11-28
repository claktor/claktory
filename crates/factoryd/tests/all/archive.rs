//! Archiving a project puts it away with nothing deleted: its agents stop
//! and don't run, what's sent to them waits, their monitors pause, no agent
//! can message them and the Map leaves them out; unarchived, all of it
//! resumes.

use crate::common::{Client, View, apply_request, create_agent, create_project, home, send_message, start, temp_dir};
use factory_protocol::v1::{self, AgentStatus, client_message};
use factoryd::config::Config;

fn archive(project: &str, archived: bool) -> client_message::Body {
    client_message::Body::UpdateProject(v1::UpdateProject { project_id: project.into(), name: None, archived: Some(archived), before_id: None, brief: None })
}

fn project(view: &View, id: &str) -> v1::Project {
    view.workspace.projects.iter().find(|project| project.id == id).expect("project").clone()
}

fn on_map(view: &View, id: &str) -> bool {
    view.workspace.map.as_ref().expect("map").nodes.iter().any(|node| node.id == id)
}

/// The texts of the person's messages in what's selected.
fn said(view: &View) -> Vec<String> {
    view.entries
        .iter()
        .filter_map(|entry| match &entry.kind {
            Some(v1::entry::Kind::User(user)) => Some(user.text.clone()),
            _ => None,
        })
        .collect()
}

#[tokio::test]
async fn an_archived_projects_agents_stop_and_wait_and_come_back_as_they_were() {
    let dir = temp_dir();
    let (addr, _factory) = start(&dir).await;
    let (mut client, workspace, _) = Client::connect(addr).await;
    let mut view = View { workspace, ..View::default() };
    apply_request(&mut client, &mut view, create_project("Site", &dir.join("site"))).await;
    apply_request(&mut client, &mut view, create_agent("site", "Builder", "")).await;
    apply_request(&mut client, &mut view, create_agent("site", "Scout", "")).await;
    let select = client_message::Body::SelectAgent(v1::SelectAgent { agent_id: "builder".into() });
    apply_request(&mut client, &mut view, select).await;
    let focus = client_message::Body::FocusProject(v1::FocusProject { project_id: "site".into() });
    apply_request(&mut client, &mut view, focus).await;
    std::fs::write(dir.join("site/notes.md"), "kept").expect("a file");
    apply_request(&mut client, &mut view, send_message("builder", "hang")).await;
    view.follow(&mut client, |view| view.agent("builder").status() == AgentStatus::Working).await;
    assert!(on_map(&view, "agent:builder") && on_map(&view, "project:site"));

    // Archived: the working agent stops, and the project leaves the Map and
    // focus, but stays in the Workspace, marked.
    assert!(matches!(apply_request(&mut client, &mut view, archive("site", true)).await, v1::server_message::Body::Ack(_)));
    view.follow(&mut client, |view| project(view, "site").archived && view.agent("builder").status() == AgentStatus::Idle).await;
    assert!(view.workspace.agents.iter().any(|agent| agent.id == "scout"), "its agents are still listed");
    assert!(!on_map(&view, "agent:builder") && !on_map(&view, "agent:scout") && !on_map(&view, "project:site"));
    assert_eq!(view.workspace.focused_project_id, "");
    let saved = Config::parse(&std::fs::read_to_string(dir.join("home/config.json")).expect("config"), &dir.join("home")).expect("valid");
    assert!(saved.project("site").expect("site").archived, "kept in config.json");

    // What's sent to it waits, without waking it.
    apply_request(&mut client, &mut view, send_message("builder", "for later")).await;
    view.follow(&mut client, |view| view.agent("builder").queued_messages == 1).await;
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    view.follow(&mut client, |view| view.agent("builder").status() == AgentStatus::Idle).await;
    assert_eq!(view.agent("builder").queued_messages, 1, "still waiting");
    assert!(!said(&view).contains(&"for later".to_owned()));

    // Unarchived: it reads what waited, and everything is as it was.
    assert!(matches!(apply_request(&mut client, &mut view, archive("site", false)).await, v1::server_message::Body::Ack(_)));
    view.follow(&mut client, |view| {
        said(view).contains(&"for later".to_owned())
            && view.agent("builder").status() == AgentStatus::Idle
            && view.agent("builder").queued_messages == 0
    })
    .await;
    assert!(!project(&view, "site").archived && on_map(&view, "agent:builder"));
    assert_eq!(said(&view), ["hang", "for later"], "its conversation kept");
    assert_eq!(std::fs::read_to_string(dir.join("site/notes.md")).expect("file"), "kept");
    assert!(home(&dir).root().join("config.json").exists());
}

#[tokio::test]
async fn archiving_an_unknown_project_is_refused() {
    let dir = temp_dir();
    let (addr, _factory) = start(&dir).await;
    let (mut client, _, _) = Client::connect(addr).await;
    let reply = client.request(archive("nowhere", true)).await;
    assert!(matches!(&reply.body, v1::server_message::Body::Error(error) if error.code() == v1::ErrorCode::NotFound), "{:?}", reply.body);
}
