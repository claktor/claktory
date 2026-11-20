//! Viewers: the app reads files on Factory's machine through the protocol
//! (ReadFile), each agent's main area is a layout of panes kept by the
//! backend (SetLayout), and agents open files there for the person (show).

use crate::common::{Client, View, apply_request, create_agent, create_project, send_message, start, temp_dir};
use factory_protocol::v1::{self, AgentStatus, client_message, entry::Kind, server_message::Body, view_node::Node};

fn idle(view: &View, agent: &str) -> bool {
    let agent = view.agent(agent);
    agent.status() == AgentStatus::Idle && agent.queued_messages == 0
}

fn read(path: &std::path::Path, max_bytes: u64) -> client_message::Body {
    client_message::Body::ReadFile(v1::ReadFile { path: path.display().to_string(), max_bytes })
}

fn leaf(id: &str, kind: &str, path: &str) -> v1::ViewNode {
    v1::ViewNode { id: id.into(), node: Some(Node::Leaf(v1::ViewLeaf { kind: kind.into(), path: path.into(), title: String::new() })) }
}

fn row(id: &str, children: Vec<v1::ViewNode>) -> v1::ViewNode {
    v1::ViewNode { id: id.into(), node: Some(Node::Split(v1::ViewSplit { axis: v1::ViewAxis::Row.into(), children, weights: Vec::new() })) }
}

fn set_layout(agent: &str, root: Option<v1::ViewNode>) -> client_message::Body {
    client_message::Body::SetLayout(v1::SetLayout { agent_id: agent.into(), root })
}

/// The layout's panes, depth first, as "kind path".
fn panes(node: Option<&v1::ViewNode>) -> Vec<String> {
    fn walk(node: &v1::ViewNode, out: &mut Vec<String>) {
        match &node.node {
            Some(Node::Leaf(leaf)) => out.push(format!("{} {}", leaf.kind, leaf.path).trim().to_owned()),
            Some(Node::Split(split)) => split.children.iter().for_each(|child| walk(child, out)),
            None => {}
        }
    }
    let mut out = Vec::new();
    if let Some(node) = node {
        walk(node, &mut out);
    }
    out
}

#[tokio::test]
async fn the_app_reads_files_through_the_protocol() {
    let dir = temp_dir();
    let (addr, _factory) = start(&dir).await;
    let (mut client, _, _) = Client::connect(addr).await;
    std::fs::write(dir.join("notes.md"), "# Plan\nShip it.\n").expect("write");
    std::fs::write(dir.join("dot.png"), b"\x89PNG\r\n\x1a\n\0\0\0\rIHDRpixels").expect("write");

    let content = |body: Body| match body {
        Body::FileContent(content) => content,
        other => panic!("expected file content, got {other:?}"),
    };
    let text = content(client.request(read(&dir.join("notes.md"), 0)).await.body);
    assert_eq!(
        (text.mime.as_str(), text.data.as_slice(), text.size, text.truncated),
        ("text/markdown", &b"# Plan\nShip it.\n"[..], 16, false)
    );
    assert!(text.modified_ms > 0 && text.error.is_empty());
    let image = content(client.request(read(&dir.join("dot.png"), 0)).await.body);
    assert_eq!((image.mime.as_str(), image.data.len()), ("image/png", 22));
    let cut = content(client.request(read(&dir.join("notes.md"), 6)).await.body);
    assert_eq!((cut.data.as_slice(), cut.truncated, cut.size), (&b"# Plan"[..], true, 16));
    let missing = content(client.request(read(&dir.join("gone.md"), 0)).await.body);
    assert_eq!(missing.error, format!("No such file: {}", dir.join("gone.md").display()));
    let folder = content(client.request(read(&dir, 0)).await.body);
    assert_eq!(folder.error, format!("{} is a folder", dir.display()));
}

#[tokio::test]
async fn layouts_are_checked_kept_for_every_device_and_survive_a_restart() {
    let dir = temp_dir();
    let (addr, factory) = start(&dir).await;
    let (mut client, workspace, _) = Client::connect(addr).await;
    let mut view = View { workspace, ..View::default() };
    apply_request(&mut client, &mut view, create_project("Site", &dir.join("site"))).await;
    apply_request(&mut client, &mut view, create_agent("site", "Builder", "")).await;
    assert!(view.agent("builder").layout.is_none(), "just its conversation");

    for (bad, why) in [
        (row("s", vec![leaf("a", "text", "/a"), leaf("b", "image", "/b")]), "exactly once"),
        (row("s", vec![leaf("chat", "chat", ""), leaf("chat", "text", "/a")]), "Two panes"),
        (row("s", (0..13).map(|n| leaf(&format!("p{n}"), if n == 0 { "chat" } else { "text" }, "/a")).collect()), "at most 12"),
    ] {
        let reply = apply_request(&mut client, &mut view, set_layout("builder", Some(bad))).await;
        assert!(matches!(&reply, Body::Error(error) if error.message.contains(why)), "{why}: {reply:?}");
    }

    let layout = row("s", vec![leaf("chat", "chat", ""), leaf("p1", "text", "/notes.md")]);
    let reply = apply_request(&mut client, &mut view, set_layout("builder", Some(layout.clone()))).await;
    assert!(matches!(reply, Body::Ack(_)), "{reply:?}");
    assert_eq!(view.agent("builder").layout.as_ref(), Some(&layout));
    let (_, other, _) = Client::connect(addr).await;
    let builder = other.agents.iter().find(|agent| agent.id == "builder").expect("builder");
    assert_eq!(builder.layout.as_ref(), Some(&layout), "every device sees it");

    drop(client);
    drop(factory);
    let (addr, _factory) = start(&dir).await;
    let (mut client, workspace, _) = Client::connect(addr).await;
    let mut view = View { workspace, ..View::default() };
    assert_eq!(view.agent("builder").layout.as_ref(), Some(&layout), "kept across a restart");
    // Back to just the conversation.
    apply_request(&mut client, &mut view, set_layout("builder", None)).await;
    assert!(view.agent("builder").layout.is_none());
}

#[tokio::test]
async fn an_agent_shows_the_person_a_file_once() {
    let dir = temp_dir();
    let (addr, _factory) = start(&dir).await;
    let (mut client, workspace, _) = Client::connect(addr).await;
    let mut view = View { workspace, ..View::default() };
    let site = dir.join("site");
    apply_request(&mut client, &mut view, create_project("Site", &site)).await;
    apply_request(&mut client, &mut view, create_agent("site", "Builder", "")).await;
    std::fs::write(site.join("report.md"), "# Report\n").expect("write");
    std::fs::write(site.join("blob.bin"), [0u8, 159, 146, 150]).expect("write");
    let report = std::fs::canonicalize(site.join("report.md")).expect("path").display().to_string();

    let show = |args: &str| send_message("builder", &format!("call show {args}"));
    let said = |view: &View| {
        view.entries.iter().rev().find_map(|entry| match &entry.kind {
            Some(Kind::ToolCall(call)) => Some(call.output.clone()),
            _ => None,
        })
    };
    // Relative to its folder; text, inferred.
    apply_request(&mut client, &mut view, show(r#"{"path":"report.md"}"#)).await;
    view.follow(&mut client, |view| idle(view, "builder")).await;
    assert_eq!(said(&view).as_deref(), Some(format!("Showing {report} to the person.").as_str()));
    assert_eq!(panes(view.agent("builder").layout.as_ref()), ["chat", &format!("text {report}")]);
    let lit = view.highlights.last().expect("the pane glows");
    assert_eq!((lit.targets[0].kind.as_str(), lit.from_agent_id.as_str()), ("pane", "builder"));
    // The call's details name the pane, for the app's view of it.
    let details = view.entries.iter().rev().find_map(|entry| match &entry.kind {
        Some(Kind::ToolCall(call)) => Some(call.details.clone()),
        _ => None,
    });
    let details: serde_json::Value = serde_json::from_str(&details.expect("the call")).expect("details are JSON");
    let pane = lit.targets[0].id.strip_prefix("builder:").expect("the agent's pane");
    assert_eq!(details, serde_json::json!({"path": report, "kind": "text", "pane": pane, "agent": "builder"}));

    // Again: the same pane, glowing again, not a second one.
    let glows = view.highlights.len();
    apply_request(&mut client, &mut view, show(&format!(r#"{{"path":"{report}"}}"#))).await;
    view.follow(&mut client, |view| idle(view, "builder") && view.highlights.len() > glows).await;
    assert_eq!(panes(view.agent("builder").layout.as_ref()).len(), 2);

    // What it can't show, it says why.
    apply_request(&mut client, &mut view, show(r#"{"path":"blob.bin"}"#)).await;
    view.follow(&mut client, |view| idle(view, "builder")).await;
    let refused = said(&view).expect("said");
    assert!(refused.contains("is neither text nor an image"), "{refused}");
    apply_request(&mut client, &mut view, show(r#"{"path":"missing.md"}"#)).await;
    view.follow(&mut client, |view| idle(view, "builder")).await;
    assert!(said(&view).expect("said").starts_with("No such file: "));
}
