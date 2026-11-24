//! An app that applies WorkspacePatch ends up with exactly the Workspace an
//! app sent whole ones has, through any change; and nothing is sent when
//! nothing changed.

use crate::common::{Client, View, apply_request, create_agent, create_project, send_message, start, temp_dir};
use factory_protocol::v1::{self, AgentStatus, client_message, server_message::Body};

#[tokio::test]
async fn patches_make_the_same_workspace_as_whole_ones() {
    let dir = temp_dir();
    let (addr, _factory) = start(&dir).await;
    let (mut whole, workspace, _) = Client::connect_with(addr, v1::DeviceKind::Laptop, false).await;
    let mut whole_view = View { workspace, ..View::default() };
    let (mut patched, workspace, _) = Client::connect_with(addr, v1::DeviceKind::Laptop, true).await;
    let mut patched_view = View { workspace, ..View::default() };

    let steps = vec![
        create_project("Site", &dir.join("site")),
        create_agent("site", "Builder", "Builds the site."),
        create_agent("site", "Writer", "Writes it."),
        create_agent("", "Helper", ""),
        client_message::Body::UpdateAgent(v1::UpdateAgent {
            agent_id: "writer".into(),
            prompt: Some("Writes every page.".into()),
            ..Default::default()
        }),
        client_message::Body::SelectAgent(v1::SelectAgent { agent_id: "builder".into() }),
        send_message("builder", "hello"),
        client_message::Body::UpdateProject(v1::UpdateProject { project_id: "site".into(), name: None, archived: Some(true), before_id: None }),
        client_message::Body::DeleteAgent(v1::DeleteAgent { agent_id: "helper".into() }),
        client_message::Body::UpdateProject(v1::UpdateProject { project_id: "site".into(), name: None, archived: Some(false), before_id: None }),
        client_message::Body::SelectAgent(v1::SelectAgent { agent_id: "rivet".into() }),
    ];
    let mut patches = 0;
    for (n, step) in steps.into_iter().enumerate() {
        apply_request(&mut whole, &mut whole_view, step).await;
        patches += same(&mut whole, &mut whole_view, &mut patched, &mut patched_view, &format!("step {n}")).await;
    }
    // The agent's run ends the same way on both.
    whole_view.follow(&mut whole, |view| view.agent("builder").status == AgentStatus::Idle as i32).await;
    patches += same(&mut whole, &mut whole_view, &mut patched, &mut patched_view, "after the run").await;
    assert!(patches >= 10, "{patches} patches");

    // A new connection starts from a whole Workspace, the same one.
    let (_, fresh, _) = Client::connect_with(addr, v1::DeviceKind::Laptop, true).await;
    assert_eq!(fresh, patched_view.workspace);
}

/// Checks both apps show the same Workspace once each has everything sent so
/// far (an agent's run may still change it in between, so it tries again).
async fn same(whole: &mut Client, whole_view: &mut View, patched: &mut Client, patched_view: &mut View, when: &str) -> usize {
    let mut patches = 0;
    for _ in 0..20 {
        patches += catch_up(patched, patched_view, whole_view).await;
        let reply = whole.request(select(whole_view)).await;
        reply.events.iter().for_each(|event| whole_view.apply(event.body.as_ref().expect("body")));
        if patched_view.workspace == whole_view.workspace {
            return patches;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    panic!("{when}: the patched Workspace differs");
}

/// Selecting what's selected: a request that changes nothing.
fn select(view: &View) -> client_message::Body {
    client_message::Body::SelectAgent(v1::SelectAgent { agent_id: view.workspace.selected_agent_id.clone() })
}

/// Brings `view` up to everything sent before now: a request that changes
/// nothing, answered after what came before it. Returns how many patches
/// came, and checks no whole Workspace did.
async fn catch_up(client: &mut Client, view: &mut View, other: &View) -> usize {
    let reply = client.request(select(other)).await;
    let mut patches = 0;
    for event in &reply.events {
        let body = event.body.as_ref().expect("body");
        assert!(!matches!(body, Body::Workspace(_)), "only patches after the first Workspace");
        patches += usize::from(matches!(body, Body::WorkspacePatch(_)));
        view.apply(body);
    }
    patches
}

#[tokio::test]
async fn a_request_that_changes_nothing_sends_no_workspace() {
    let dir = temp_dir();
    let (addr, _factory) = start(&dir).await;
    for patches in [false, true] {
        let (mut client, _, _) = Client::connect_with(addr, v1::DeviceKind::Laptop, patches).await;
        // Rivet is selected already.
        let unchanged = client_message::Body::SelectAgent(v1::SelectAgent { agent_id: "rivet".into() });
        let events = client.ok(unchanged).await;
        let workspaces = events.iter().filter(|event| matches!(event.body, Some(Body::Workspace(_) | Body::WorkspacePatch(_))));
        assert_eq!(workspaces.count(), 0);
    }
}
