//! GetChecklist and ListTasks: an agent's checklist and tasks as their
//! panels show them, read without opening the panel.

use crate::common::{Client, View, apply_request, create_agent, create_project, send_message, start, temp_dir};
use factory_protocol::v1::{self, AgentStatus, client_message, entry::Kind, extension_view, server_message::Body};

fn select(agent: &str) -> client_message::Body {
    client_message::Body::SelectAgent(v1::SelectAgent { agent_id: agent.into() })
}

async fn call(client: &mut Client, view: &mut View, agent: &str, tool: &str, args: &str) -> String {
    apply_request(client, view, select(agent)).await;
    let before = view.entries.len();
    apply_request(client, view, send_message(agent, &format!("call {tool} {args}"))).await;
    let idle = |view: &View| view.agent(agent).status() == AgentStatus::Idle && view.agent(agent).queued_messages == 0;
    view.follow(client, |view| view.entries.len() >= before + 3 && idle(view)).await;
    view.entries[before..]
        .iter()
        .find_map(|entry| match &entry.kind {
            Some(Kind::ToolCall(call)) => Some(call.output.clone()),
            _ => None,
        })
        .expect("a tool call")
}

async fn panel(client: &mut Client, view: &mut View, request: client_message::Body) -> v1::ExtensionView {
    match apply_request(client, view, request).await {
        Body::ExtensionView(view) => view,
        other => panic!("expected an ExtensionView, got {other:?}"),
    }
}

fn checklist(agent: &str) -> client_message::Body {
    client_message::Body::GetChecklist(v1::GetChecklist { agent_id: agent.into() })
}

fn tasks(agent: &str) -> client_message::Body {
    client_message::Body::ListTasks(v1::ListTasks { agent_id: agent.into() })
}

#[tokio::test]
async fn an_agents_checklist_and_tasks_read_as_their_panels_show_them_without_opening_one() {
    let dir = temp_dir();
    let (addr, _factory) = start(&dir).await;
    let (mut client, workspace, _) = Client::connect(addr).await;
    let mut view = View { workspace, ..View::default() };
    apply_request(&mut client, &mut view, create_project("Site", &dir.join("site"))).await;
    apply_request(&mut client, &mut view, create_agent("site", "Builder", "")).await;

    // Checklist is off: nothing to show.
    let off = panel(&mut client, &mut view, checklist("builder")).await;
    assert_eq!((off.extension_id.as_str(), off.agent_id.as_str(), off.body), ("checklist", "builder", None));

    let on = v1::ExtensionChange { id: "checklist".into(), enabled: Some(true), ..Default::default() };
    let on = v1::UpdateAgent { agent_id: "builder".into(), extension: Some(on), ..Default::default() };
    apply_request(&mut client, &mut view, client_message::Body::UpdateAgent(on)).await;
    call(&mut client, &mut view, "builder", "checklist_add", r#"{"items":["Fix login"]}"#).await;
    let read = panel(&mut client, &mut view, checklist("builder")).await;
    let Some(extension_view::Body::Checklist(list)) = read.body else { panic!("expected a checklist, got {read:?}") };
    let titles: Vec<&str> = list.items.iter().map(|item| item.text.as_str()).collect();
    assert_eq!(titles, ["Fix login"]);

    let started = call(&mut client, &mut view, "builder", "bash", r#"{"command":"sleep 30","run_in_background":true}"#).await;
    assert!(started.starts_with("Started task-1"), "{started}");
    let read = panel(&mut client, &mut view, tasks("builder")).await;
    let Some(extension_view::Body::Tasks(listed)) = read.body else { panic!("expected tasks, got {read:?}") };
    let running: Vec<(&str, &str)> = listed.tasks.iter().map(|task| (task.id.as_str(), task.command.as_str())).collect();
    assert_eq!(running, [("task-1", "sleep 30")]);

    // Neither opened a panel.
    assert!(view.agent("builder").open_panel.is_empty(), "{:?}", view.agent("builder").open_panel);

    let unknown = apply_request(&mut client, &mut view, checklist("nobody")).await;
    assert!(matches!(unknown, Body::Error(_)), "{unknown:?}");
}
