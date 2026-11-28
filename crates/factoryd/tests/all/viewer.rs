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
    client_message::Body::ReadFile(v1::ReadFile { path: path.display().to_string(), max_bytes, ..Default::default() })
}

fn content(body: Body) -> v1::FileContent {
    match body {
        Body::FileContent(content) => content,
        other => panic!("expected file content, got {other:?}"),
    }
}

fn leaf(id: &str, kind: &str, path: &str) -> v1::ViewNode {
    v1::ViewNode {
        id: id.into(),
        node: Some(Node::Leaf(v1::ViewLeaf { kind: kind.into(), path: path.into(), title: String::new(), ..Default::default() })),
    }
}

fn row(id: &str, children: Vec<v1::ViewNode>) -> v1::ViewNode {
    v1::ViewNode { id: id.into(), node: Some(Node::Split(v1::ViewSplit { axis: v1::ViewAxis::Row.into(), children, weights: Vec::new() })) }
}

fn set_layout(agent: &str, root: Option<v1::ViewNode>) -> client_message::Body {
    client_message::Body::SetLayout(v1::SetLayout { agent_id: agent.into(), root, focus: None })
}

fn focused(agent: &str, root: Option<v1::ViewNode>, focus: &str) -> client_message::Body {
    client_message::Body::SetLayout(v1::SetLayout { agent_id: agent.into(), root, focus: Some(focus.into()) })
}

/// The layout without when each pane was opened.
fn unstamped(node: Option<&v1::ViewNode>) -> Option<v1::ViewNode> {
    fn clear(node: &mut v1::ViewNode) {
        match &mut node.node {
            Some(Node::Leaf(leaf)) => leaf.opened_ms = 0,
            Some(Node::Split(split)) => split.children.iter_mut().for_each(clear),
            None => {}
        }
    }
    node.cloned().map(|mut node| {
        clear(&mut node);
        node
    })
}

/// When the pane `id` of the layout was opened.
fn opened(node: Option<&v1::ViewNode>, id: &str) -> i64 {
    fn find(node: &v1::ViewNode, id: &str) -> Option<i64> {
        match &node.node {
            Some(Node::Leaf(leaf)) if node.id == id => Some(leaf.opened_ms),
            Some(Node::Split(split)) => split.children.iter().find_map(|child| find(child, id)),
            _ => None,
        }
    }
    node.and_then(|node| find(node, id)).expect("the pane")
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
    let before = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).expect("now").as_millis() as i64;
    let reply = apply_request(&mut client, &mut view, set_layout("builder", Some(layout.clone()))).await;
    assert!(matches!(reply, Body::Ack(_)), "{reply:?}");
    assert_eq!(unstamped(view.agent("builder").layout.as_ref()), Some(layout.clone()));
    // Each pane says when it was opened: a new one, now.
    let p1 = opened(view.agent("builder").layout.as_ref(), "p1");
    assert!(p1 >= before, "{p1} >= {before}");
    let (_, other, _) = Client::connect(addr).await;
    let builder = other.agents.iter().find(|agent| agent.id == "builder").expect("builder");
    assert_eq!(builder.layout.as_ref(), view.agent("builder").layout.as_ref(), "every device sees it");

    // A device's layout sent back keeps when its panes were opened, whatever
    // it says: the backend keeps that.
    let mut sent = view.agent("builder").layout.clone().expect("a layout");
    if let Some(Node::Split(split)) = &mut sent.node
        && let Some(Node::Leaf(leaf)) = &mut split.children[1].node
    {
        leaf.opened_ms = 5;
    }
    assert!(matches!(apply_request(&mut client, &mut view, set_layout("builder", Some(sent))).await, Body::Ack(_)));
    assert_eq!(opened(view.agent("builder").layout.as_ref(), "p1"), p1);

    drop(client);
    drop(factory);
    let (addr, factory) = start(&dir).await;
    let (client, workspace, _) = Client::connect(addr).await;
    let view = View { workspace, ..View::default() };
    assert_eq!(unstamped(view.agent("builder").layout.as_ref()), Some(layout.clone()), "kept across a restart");
    assert_eq!(opened(view.agent("builder").layout.as_ref(), "p1"), p1, "and when it was opened");
    drop(client);
    drop(factory);

    // A layout kept before Claktory kept when its panes were opened: 0.
    let db = rusqlite::Connection::open(dir.join("home/factory.db")).expect("db");
    db.execute("UPDATE layouts SET opened = NULL", []).expect("from before");
    drop(db);
    let (addr, _factory) = start(&dir).await;
    let (mut client, workspace, _) = Client::connect(addr).await;
    let mut view = View { workspace, ..View::default() };
    assert_eq!(unstamped(view.agent("builder").layout.as_ref()), Some(layout));
    assert_eq!(opened(view.agent("builder").layout.as_ref(), "p1"), 0);
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
    let pane = lit.targets[0].id.strip_prefix("builder:").expect("the agent's pane").to_owned();
    assert_eq!(details, serde_json::json!({"path": report, "kind": "text", "pane": pane, "agent": "builder"}));

    // Again: the same pane, glowing again, not a second one, and opened
    // now, on every device.
    let first = opened(view.agent("builder").layout.as_ref(), &pane);
    assert!(first > 0);
    tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    let glows = view.highlights.len();
    apply_request(&mut client, &mut view, show(&format!(r#"{{"path":"{}"}}"#, crate::common::json_text(&report)))).await;
    view.follow(&mut client, |view| idle(view, "builder") && view.highlights.len() > glows).await;
    assert_eq!(panes(view.agent("builder").layout.as_ref()).len(), 2);
    let again = opened(view.agent("builder").layout.as_ref(), &pane);
    assert!(again > first, "{again} > {first}");
    let (_, other, _) = Client::connect(addr).await;
    let builder = other.agents.iter().find(|agent| agent.id == "builder").expect("builder");
    assert_eq!(opened(builder.layout.as_ref(), &pane), again);

    // What it can't show, it says why.
    apply_request(&mut client, &mut view, show(r#"{"path":"blob.bin"}"#)).await;
    view.follow(&mut client, |view| idle(view, "builder")).await;
    let refused = said(&view).expect("said");
    assert!(refused.contains("isn't text, an image, a video or a PDF the app can show"), "{refused}");
    apply_request(&mut client, &mut view, show(r#"{"path":"missing.md"}"#)).await;
    view.follow(&mut client, |view| idle(view, "builder")).await;
    assert!(said(&view).expect("said").starts_with("No such file: "));
    // A named pipe is never opened: that would wait for a writer, and the
    // agent with it.
    #[cfg(unix)]
    {
        assert!(std::process::Command::new("mkfifo").arg(site.join("slow.txt")).status().expect("mkfifo").success());
        apply_request(&mut client, &mut view, show(r#"{"path":"slow.txt"}"#)).await;
        view.follow(&mut client, |view| idle(view, "builder")).await;
        let refused = said(&view).expect("said");
        assert!(refused.contains("slow.txt isn't a file the app can show: it's a named pipe, a socket or a device."), "{refused}");
    }
}

/// show opens panes up to the person's setting (4 until changed), counting
/// every pane but the conversation, whoever opened it. Past it, the new
/// file takes the place of the pane opened longest ago, never the one the
/// person expanded, and the agent is told which; a file already open just
/// glows; the order is kept across a restart; a higher setting makes room.
#[tokio::test]
async fn a_full_layout_replaces_its_oldest_pane() {
    let dir = temp_dir();
    let (addr, factory) = start(&dir).await;
    let (mut client, workspace, _) = Client::connect(addr).await;
    let mut view = View { workspace, ..View::default() };
    let site = dir.join("site");
    apply_request(&mut client, &mut view, create_project("Site", &site)).await;
    apply_request(&mut client, &mut view, create_agent("site", "Builder", "")).await;
    for number in 1..=8 {
        std::fs::write(site.join(format!("f{number}.md")), "notes\n").expect("write");
    }
    let path = |number: usize| std::fs::canonicalize(site.join(format!("f{number}.md"))).expect("path").display().to_string();
    let file = |number: usize| format!("text {}", path(number));
    let show = |number: usize| send_message("builder", &format!(r#"call show {{"path":"f{number}.md"}}"#));
    let said = |view: &View| {
        view.entries.iter().rev().find_map(|entry| match &entry.kind {
            Some(Kind::ToolCall(call)) => Some(call.output.clone()),
            _ => None,
        })
    };
    let calls = |view: &View| view.entries.iter().filter(|entry| matches!(entry.kind, Some(Kind::ToolCall(_)))).count();
    let settings = |view: &View| view.workspace.settings.clone().expect("settings");
    assert_eq!(settings(&view).max_open_panes, 4, "the default");

    // The person has Settings open beside the conversation: it counts.
    let page = row("s", vec![leaf("chat", "chat", ""), leaf("set", "settings", "")]);
    assert!(matches!(apply_request(&mut client, &mut view, set_layout("builder", Some(page))).await, Body::Ack(_)));
    for number in 1..=3 {
        apply_request(&mut client, &mut view, show(number)).await;
        view.follow(&mut client, |view| idle(view, "builder") && panes(view.agent("builder").layout.as_ref()).len() == number + 2).await;
    }

    // The fourth file is one too many: it takes the place of Settings,
    // opened first, and the agent is told.
    let before = calls(&view);
    apply_request(&mut client, &mut view, show(4)).await;
    view.follow(&mut client, |view| idle(view, "builder") && calls(view) > before).await;
    assert_eq!(said(&view).expect("said"), format!("Showing {} to the person, in place of Settings (the oldest pane).", path(4)));
    assert_eq!(panes(view.agent("builder").layout.as_ref()), ["chat", &file(4), &file(1), &file(2), &file(3)]);

    // A file already open just glows: its pane, nothing replaced.
    let glows = view.highlights.len();
    let before = calls(&view);
    apply_request(&mut client, &mut view, show(3)).await;
    view.follow(&mut client, |view| idle(view, "builder") && calls(view) > before && view.highlights.len() > glows).await;
    assert_eq!(said(&view).expect("said"), format!("Showing {} to the person.", path(3)));
    assert_eq!(panes(view.agent("builder").layout.as_ref()).len(), 5);

    // The person expands f1, now the oldest: it stays, and the next oldest goes.
    let layout = view.agent("builder").layout.clone();
    let id = |view: &View, number: usize| {
        fn find(node: &v1::ViewNode, path: &str) -> Option<String> {
            match &node.node {
                Some(Node::Leaf(leaf)) if leaf.path == path => Some(node.id.clone()),
                Some(Node::Split(split)) => split.children.iter().find_map(|child| find(child, path)),
                _ => None,
            }
        }
        find(view.agent("builder").layout.as_ref().expect("a layout"), &path(number)).expect("its pane")
    };
    let first = id(&view, 1);
    assert!(matches!(apply_request(&mut client, &mut view, focused("builder", layout, &first)).await, Body::Ack(_)));
    let before = calls(&view);
    apply_request(&mut client, &mut view, show(5)).await;
    view.follow(&mut client, |view| idle(view, "builder") && calls(view) > before).await;
    assert!(said(&view).expect("said").ends_with(", in place of f2.md (the oldest pane)."), "{said:?}", said = said(&view));
    assert_eq!(panes(view.agent("builder").layout.as_ref()), ["chat", &file(4), &file(1), &file(5), &file(3)]);
    assert_eq!(view.agent("builder").layout_focus, first, "still expanded");
    drop(client);
    drop(factory);

    // Across a restart the order holds: f3, shown again after f4 opened,
    // counts as opened then, so f4 is the oldest left to replace.
    let (addr, _factory) = start(&dir).await;
    let (mut client, workspace, _) = Client::connect(addr).await;
    let mut view = View { workspace, ..View::default() };
    apply_request(&mut client, &mut view, client_message::Body::SelectAgent(v1::SelectAgent { agent_id: "builder".into() })).await;
    let before = calls(&view);
    apply_request(&mut client, &mut view, show(6)).await;
    view.follow(&mut client, |view| idle(view, "builder") && calls(view) > before).await;
    assert!(said(&view).expect("said").ends_with(", in place of f4.md (the oldest pane)."), "{said:?}", said = said(&view));
    assert_eq!(panes(view.agent("builder").layout.as_ref()), ["chat", &file(6), &file(1), &file(5), &file(3)]);

    // The setting is from 1 to 11, or 0 for the default.
    let set = |panes: u32| client_message::Body::UpdateSettings(v1::UpdateSettings { max_open_panes: Some(panes), ..Default::default() });
    let refused = apply_request(&mut client, &mut view, set(12)).await;
    assert!(matches!(&refused, Body::Error(error) if error.message.contains("max_open_panes must be from 1 to 11")), "{refused:?}");

    // Raised, there's room for one more, added beside the others.
    assert!(matches!(apply_request(&mut client, &mut view, set(5)).await, Body::Ack(_)));
    view.follow(&mut client, |view| settings(view).max_open_panes == 5).await;
    // Kept in factory.db: a config.json that held it would be refused whole
    // by a backend from before it, after a rollback.
    let config = std::fs::read_to_string(dir.join("home/config.json")).expect("config");
    assert!(!config.contains("max_open_panes"), "{config}");
    let db = rusqlite::Connection::open(dir.join("home/factory.db")).expect("db");
    let kept: String = db.query_row("SELECT value FROM state WHERE key = 'max_open_panes'", [], |row| row.get(0)).expect("kept");
    assert_eq!(kept, "5");
    apply_request(&mut client, &mut view, show(7)).await;
    view.follow(&mut client, |view| idle(view, "builder") && panes(view.agent("builder").layout.as_ref()).len() == 6).await;
    assert_eq!(said(&view).expect("said"), format!("Showing {} to the person.", path(7)));

    // With only the expanded pane to give, show says so and changes nothing.
    assert!(matches!(apply_request(&mut client, &mut view, set(1)).await, Body::Ack(_)));
    let only = row("s", vec![leaf("chat", "chat", ""), leaf("p1", "text", &path(1))]);
    assert!(matches!(apply_request(&mut client, &mut view, focused("builder", Some(only), "p1")).await, Body::Ack(_)));
    let before = calls(&view);
    apply_request(&mut client, &mut view, show(8)).await;
    view.follow(&mut client, |view| idle(view, "builder") && calls(view) > before).await;
    assert!(said(&view).expect("said").starts_with("The only pane beside this conversation is the one the person expanded"));
    assert_eq!(panes(view.agent("builder").layout.as_ref()), ["chat", &file(1)]);
}

/// Focus: the person fills the window with one pane; it's kept with
/// the layout, for every device and across a restart, and an agent's show
/// leaves it in place.
#[tokio::test]
async fn a_pane_in_focus_is_kept_with_the_layout_and_show_leaves_it_there() {
    let dir = temp_dir();
    let (addr, _factory) = start(&dir).await;
    let (mut client, workspace, _) = Client::connect(addr).await;
    let mut view = View { workspace, ..View::default() };
    let site = dir.join("site");
    apply_request(&mut client, &mut view, create_project("Site", &site)).await;
    apply_request(&mut client, &mut view, create_agent("site", "Builder", "")).await;
    std::fs::write(site.join("notes.md"), "notes\n").expect("write");
    let layout = || row("s", vec![leaf("chat", "chat", ""), leaf("set", "settings", "")]);
    assert_eq!(view.agent("builder").layout_focus, "");

    // Only a pane of the layout can be in focus.
    let refused = apply_request(&mut client, &mut view, focused("builder", Some(layout()), "nope")).await;
    assert!(matches!(&refused, Body::Error(error) if error.message.contains("no pane \"nope\"")), "{refused:?}");
    let refused = apply_request(&mut client, &mut view, focused("builder", None, "chat")).await;
    assert!(matches!(refused, Body::Error(_)), "just the conversation has nothing to focus");
    assert!(matches!(apply_request(&mut client, &mut view, focused("builder", Some(layout()), "set")).await, Body::Ack(_)));
    assert_eq!(view.agent("builder").layout_focus, "set");

    // The agent shows a file: its pane joins the layout, the focus stays.
    apply_request(&mut client, &mut view, send_message("builder", r#"call show {"path":"notes.md"}"#)).await;
    view.follow(&mut client, |view| idle(view, "builder") && panes(view.agent("builder").layout.as_ref()).len() == 3).await;
    assert_eq!(view.agent("builder").layout_focus, "set");

    // Kept across a restart, as every device sees it.
    let (addr, _again) = start(&dir).await;
    let (mut client, workspace, _) = Client::connect(addr).await;
    let mut view = View { workspace, ..View::default() };
    assert_eq!((view.agent("builder").layout_focus.as_str(), panes(view.agent("builder").layout.as_ref()).len()), ("set", 3));

    // A SetLayout that leaves focus out keeps it while its pane is there,
    // and ends it when the pane closes.
    let rearranged = row("s", vec![leaf("set", "settings", ""), leaf("chat", "chat", "")]);
    assert!(matches!(apply_request(&mut client, &mut view, set_layout("builder", Some(rearranged))).await, Body::Ack(_)));
    assert_eq!(view.agent("builder").layout_focus, "set");
    assert!(matches!(apply_request(&mut client, &mut view, set_layout("builder", Some(layout()))).await, Body::Ack(_)));
    assert_eq!(view.agent("builder").layout_focus, "set");
    let closed = row("s", vec![leaf("chat", "chat", ""), leaf("other", "usage", "")]);
    assert!(matches!(apply_request(&mut client, &mut view, set_layout("builder", Some(closed))).await, Body::Ack(_)));
    assert_eq!(view.agent("builder").layout_focus, "", "its pane closed");
    assert!(matches!(apply_request(&mut client, &mut view, focused("builder", Some(layout()), "set")).await, Body::Ack(_)));

    // Back: the same layout with no focus.
    let back = view.agent("builder").layout.clone();
    assert!(matches!(apply_request(&mut client, &mut view, focused("builder", back, "")).await, Body::Ack(_)));
    assert_eq!(view.agent("builder").layout_focus, "");
    assert_eq!(panes(view.agent("builder").layout.as_ref()), ["chat", "settings"], "the arrangement is as it was");
}

#[tokio::test]
async fn a_video_is_shown_and_played_a_range_at_a_time() {
    const CHUNK: u64 = 8 * 1024 * 1024;
    let dir = temp_dir();
    let (addr, _factory) = start(&dir).await;
    let (mut client, workspace, _) = Client::connect(addr).await;
    let mut view = View { workspace, ..View::default() };
    let site = dir.join("site");
    apply_request(&mut client, &mut view, create_project("Site", &site)).await;
    apply_request(&mut client, &mut view, create_agent("site", "Builder", "")).await;
    let mut bytes = b"\0\0\0\x18ftypisom\0\0\0\0isommp41\0\0\0\x08mdat".to_vec();
    bytes.extend((0..CHUNK + 4096).map(|at| (at % 253) as u8));
    std::fs::write(site.join("clip.mp4"), &bytes).expect("write");
    let clip = std::fs::canonicalize(site.join("clip.mp4")).expect("path");

    // Inferred from what it is.
    apply_request(&mut client, &mut view, send_message("builder", r#"call show {"path":"clip.mp4"}"#)).await;
    view.follow(&mut client, |view| idle(view, "builder")).await;
    assert_eq!(panes(view.agent("builder").layout.as_ref()), ["chat", &format!("video {}", clip.display())]);

    // Played as the app does: 8 MB at a time, each range with the whole file's hash.
    let ranged = |offset: u64| client_message::Body::ReadFile(v1::ReadFile { path: clip.display().to_string(), max_bytes: CHUNK, offset, ..Default::default() });
    let first = content(client.request(ranged(0)).await.body);
    assert_eq!((first.mime.as_str(), first.offset, first.data.len() as u64, first.truncated), ("video/mp4", 0, CHUNK, true));
    let last = content(client.request(ranged(CHUNK)).await.body);
    assert_eq!((last.offset, last.offset + last.data.len() as u64), (CHUNK, last.size));
    assert_eq!(first.sha256.len(), 64);
    assert_eq!(first.sha256, last.sha256, "one version, one hash");
    assert_eq!([first.data, last.data].concat(), bytes);

    // A new version, a new hash.
    let mut changed = bytes.clone();
    changed[64] ^= 0xff;
    std::fs::write(&clip, &changed).expect("rewrite");
    let later = std::time::SystemTime::now() + std::time::Duration::from_secs(5);
    std::fs::File::options().write(true).open(&clip).and_then(|file| file.set_modified(later)).expect("touch");
    let again = content(client.request(ranged(CHUNK)).await.body);
    assert_ne!(again.sha256, last.sha256);
}

/// A PDF is read whole, a range at a time, from one version (iPad phase 3
/// §2): ranges of 8 MB up to 64 MB, each with the whole file's hash, and a
/// read that names the version it expects says when the file changed.
#[tokio::test]
async fn a_pdf_is_shown_and_read_whole_from_one_version() {
    const CHUNK: u64 = 8 * 1024 * 1024;
    let dir = temp_dir();
    let (addr, _factory) = start(&dir).await;
    let (mut client, workspace, _) = Client::connect(addr).await;
    let mut view = View { workspace, ..View::default() };
    let site = dir.join("site");
    apply_request(&mut client, &mut view, create_project("Site", &site)).await;
    apply_request(&mut client, &mut view, create_agent("site", "Builder", "")).await;
    // Over text's 2 MB, which a PDF read from the start fell under before.
    let mut bytes = b"%PDF-1.7\n".to_vec();
    bytes.extend((0..CHUNK + 4096).map(|at| (at % 251) as u8));
    std::fs::write(site.join("paper.pdf"), &bytes).expect("write");
    let paper = std::fs::canonicalize(site.join("paper.pdf")).expect("path");

    apply_request(&mut client, &mut view, send_message("builder", r#"call show {"path":"paper.pdf"}"#)).await;
    view.follow(&mut client, |view| idle(view, "builder")).await;
    assert_eq!(panes(view.agent("builder").layout.as_ref()), ["chat", &format!("pdf {}", paper.display())]);

    let ranged = |offset: u64, expected: &str| {
        client_message::Body::ReadFile(v1::ReadFile {
            path: paper.display().to_string(),
            offset,
            expected_sha256: expected.into(),
            ..Default::default()
        })
    };
    let first = content(client.request(ranged(0, "")).await.body);
    assert_eq!((first.mime.as_str(), first.offset, first.data.len() as u64, first.truncated), ("application/pdf", 0, CHUNK, true));
    assert_eq!(first.sha256.len(), 64, "the whole file's, from the first range");
    let last = content(client.request(ranged(CHUNK, &first.sha256)).await.body);
    assert_eq!((last.error.as_str(), last.offset + last.data.len() as u64, last.sha256.as_str()), ("", last.size, first.sha256.as_str()));
    assert_eq!([first.data, last.data].concat(), bytes);

    // Changed between ranges: the next one says so, with nothing of the new version.
    let mut changed = bytes.clone();
    changed[64] ^= 0xff;
    std::fs::write(&paper, &changed).expect("rewrite");
    let later = std::time::SystemTime::now() + std::time::Duration::from_secs(5);
    std::fs::File::options().write(true).open(&paper).and_then(|file| file.set_modified(later)).expect("touch");
    let stale = content(client.request(ranged(CHUNK, &first.sha256)).await.body);
    assert_eq!((stale.error.as_str(), stale.data.len(), stale.sha256.as_str()), ("changed", 0, ""));
    let fresh = content(client.request(ranged(0, "")).await.body);
    assert_ne!(fresh.sha256, first.sha256);

    // A small file read whole is checked against what was read.
    std::fs::write(site.join("notes.md"), "# Notes\n").expect("write");
    let notes = site.join("notes.md").display().to_string();
    let whole = content(client.request(client_message::Body::ReadFile(v1::ReadFile { path: notes.clone(), ..Default::default() })).await.body);
    let same = v1::ReadFile { path: notes.clone(), expected_sha256: whole.sha256.to_uppercase(), ..Default::default() };
    assert_eq!(content(client.request(client_message::Body::ReadFile(same)).await.body).data, b"# Notes\n");
    let other = v1::ReadFile { path: notes, expected_sha256: first.sha256.clone(), ..Default::default() };
    assert_eq!(content(client.request(client_message::Body::ReadFile(other)).await.body).error, "changed");

    // Over 64 MB: refused, with a sentence for the person.
    let file = std::fs::File::options().write(true).open(&paper).expect("open");
    file.set_len(80_000_000).expect("grow");
    let big = content(client.request(ranged(0, "")).await.body);
    assert_eq!((big.error.as_str(), big.data.len()), ("This PDF is too big to open here (80 MB).", 0));
}

/// The person opening a pane (on the phone, from its list of open
/// files) counts as opening it now, on every device, as an agent's re-show
/// does.
#[tokio::test]
async fn the_person_opening_a_pane_makes_it_the_latest() {
    let dir = temp_dir();
    let (addr, _factory) = start(&dir).await;
    let (mut client, workspace, _) = Client::connect(addr).await;
    let mut view = View { workspace, ..View::default() };
    apply_request(&mut client, &mut view, create_project("Site", &dir.join("site"))).await;
    apply_request(&mut client, &mut view, create_agent("site", "Builder", "")).await;
    let layout = row("s", vec![leaf("chat", "chat", ""), leaf("a", "text", "/a.md"), leaf("b", "text", "/b.md")]);
    assert!(matches!(apply_request(&mut client, &mut view, set_layout("builder", Some(layout))).await, Body::Ack(_)));
    let a = opened(view.agent("builder").layout.as_ref(), "a");
    tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    let opened_pane = |agent: &str, pane: &str| {
        client_message::Body::PaneOpened(v1::PaneOpened { agent_id: agent.into(), pane_id: pane.into() })
    };
    assert!(matches!(apply_request(&mut client, &mut view, opened_pane("builder", "a")).await, Body::Ack(_)));
    view.follow(&mut client, |view| opened(view.agent("builder").layout.as_ref(), "a") > a).await;
    assert!(opened(view.agent("builder").layout.as_ref(), "a") > opened(view.agent("builder").layout.as_ref(), "b"));

    let refused = apply_request(&mut client, &mut view, opened_pane("builder", "nope")).await;
    assert!(matches!(&refused, Body::Error(error) if error.message == "The layout has no pane \"nope\"."), "{refused:?}");
    let refused = apply_request(&mut client, &mut view, opened_pane("nobody", "a")).await;
    assert!(matches!(&refused, Body::Error(error) if error.code() == v1::ErrorCode::NotFound), "{refused:?}");
}

/// A file the app kept is opened again without reading it again: a read
/// naming the kept copy's hash gets `unchanged` and no data while the file
/// is that version, and the new version, hashed whole, once it isn't.
#[tokio::test]
async fn a_kept_copy_is_opened_again_without_reading_it_again() {
    let dir = temp_dir();
    let (addr, _factory) = start(&dir).await;
    let (mut client, _, _) = Client::connect(addr).await;
    let notes = dir.join("notes.md");
    let text = "# Notes\n".repeat(1000);
    std::fs::write(&notes, &text).expect("write");
    let ask = |max_bytes: u64, offset: u64, unless: &str| {
        client_message::Body::ReadFile(v1::ReadFile {
            path: notes.display().to_string(),
            max_bytes,
            offset,
            unless_sha256: unless.into(),
            ..Default::default()
        })
    };

    // The first piece of a long text has no hash unless a kept copy is named.
    let first = content(client.request(ask(1000, 0, "")).await.body);
    assert_eq!((first.truncated, first.sha256.as_str()), (true, ""));
    let whole = content(client.request(ask(0, 0, "")).await.body);
    assert_eq!((whole.truncated, whole.sha256.len(), whole.unchanged), (false, 64, false));
    let named = content(client.request(ask(1000, 0, &"0".repeat(64))).await.body);
    assert_eq!((named.data.len(), named.sha256.as_str(), named.unchanged), (1000, whole.sha256.as_str(), false));

    // Still that version: nothing read, the rest as it is.
    let kept = content(client.request(ask(1000, 0, &whole.sha256.to_uppercase())).await.body);
    assert!(kept.unchanged);
    assert_eq!((kept.data.len(), kept.truncated, kept.error.as_str()), (0, false, ""));
    assert_eq!((kept.mime.as_str(), kept.size, kept.modified_ms, kept.sha256.as_str()), ("text/markdown", whole.size, whole.modified_ms, whole.sha256.as_str()));

    // Changed: read as asked, a range of the new version too, with its hash.
    let changed = text.replacen("Notes", "Plans", 1);
    std::fs::write(&notes, &changed).expect("rewrite");
    let later = std::time::SystemTime::now() + std::time::Duration::from_secs(5);
    std::fs::File::options().write(true).open(&notes).and_then(|file| file.set_modified(later)).expect("touch");
    let fresh = content(client.request(ask(1000, 0, &whole.sha256)).await.body);
    assert!(!fresh.unchanged);
    assert_eq!((fresh.data.as_slice(), fresh.truncated), (&changed.as_bytes()[..1000], true));
    assert_eq!(fresh.sha256.len(), 64);
    assert_ne!(fresh.sha256, whole.sha256);
    let range = content(client.request(ask(100, 4000, &whole.sha256)).await.body);
    assert_eq!((range.data.as_slice(), range.unchanged, range.sha256.as_str()), (&changed.as_bytes()[4000..4100], false, fresh.sha256.as_str()));

    // A file that's gone says so, as a plain read does.
    std::fs::remove_file(&notes).expect("remove");
    let gone = content(client.request(ask(0, 0, &fresh.sha256)).await.body);
    assert_eq!((gone.error, gone.unchanged), (format!("No such file: {}", notes.display()), false));
}
