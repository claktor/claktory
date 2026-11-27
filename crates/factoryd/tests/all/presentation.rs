//! Where each device was reading: kept per device and
//! agent, sent only to that device's connections, with panes a layout no
//! longer has, deleted agents and unpaired devices dropped.

use crate::common::{Client, View, apply_request, create_agent, create_project, home, start, temp_dir};
use factory_protocol::v1::{self, client_message::Body as Request, server_message::Body, view_node::Node};

fn leaf(id: &str, kind: &str, path: &str) -> v1::ViewNode {
    v1::ViewNode { id: id.into(), node: Some(Node::Leaf(v1::ViewLeaf { kind: kind.into(), path: path.into(), ..Default::default() })) }
}

fn layout(agent: &str, panes: Vec<v1::ViewNode>) -> Request {
    let mut children = vec![leaf("chat", "chat", "")];
    children.extend(panes);
    let split = v1::ViewSplit { axis: v1::ViewAxis::Row.into(), children, weights: Vec::new() };
    Request::SetLayout(v1::SetLayout {
        agent_id: agent.into(),
        root: Some(v1::ViewNode { id: "split".into(), node: Some(Node::Split(split)) }),
        focus: None,
    })
}

fn place(leaf: &str, page: u32) -> v1::LeafPlace {
    v1::LeafPlace { leaf_id: leaf.into(), page_1based: page }
}

fn set(agent: &str, focused: &str, places: Vec<v1::LeafPlace>, sidebar_hidden: Option<bool>) -> Request {
    Request::SetPresentation(v1::SetPresentation { agent_id: agent.into(), focused_leaf_id: focused.into(), places, sidebar_hidden })
}

async fn refused(client: &mut Client, request: Request) -> String {
    match client.request(request).await.body {
        Body::Error(error) => error.message,
        other => panic!("expected a refusal, got {other:?}"),
    }
}

/// The next Presentation this client hears that `wanted` accepts.
async fn heard(client: &mut Client, wanted: impl Fn(&v1::Presentation) -> bool) -> v1::Presentation {
    client
        .wait_for(
            |body| if let Body::Presentation(presentation) = body { wanted(presentation).then(|| presentation.clone()) } else { None },
        )
        .await
}

#[tokio::test]
async fn each_device_keeps_where_it_was_reading_for_itself() {
    let dir = temp_dir();
    let (addr, _factory) = start(&dir).await;
    let (mut ipad, workspace, _) = Client::connect(addr).await;
    let mut view = View { workspace, ..View::default() };
    let (mut window, _, _) = Client::connect(addr).await;
    apply_request(&mut ipad, &mut view, create_project("Site", &dir.join("site"))).await;
    apply_request(&mut ipad, &mut view, create_agent("site", "Writer", "")).await;
    apply_request(&mut ipad, &mut view, create_agent("site", "Editor", "")).await;
    apply_request(
        &mut ipad,
        &mut view,
        layout("writer", vec![leaf("paper", "pdf", "/tmp/paper.pdf"), leaf("notes", "text", "/tmp/notes.md")]),
    )
    .await;

    // Kept: only the panes the layout has, and pages that say something.
    let places = vec![place("paper", 3), place("gone", 2), place("notes", 0)];
    assert!(matches!(ipad.request(set("writer", "paper", places, Some(true))).await.body, Body::Ack(_)));
    let shown = heard(&mut window, |shown| !shown.agents.is_empty()).await;
    let writer = v1::AgentPresentation { agent_id: "writer".into(), focused_leaf_id: "paper".into(), places: vec![place("paper", 3)] };
    assert_eq!(shown, v1::Presentation { agents: vec![writer.clone()], sidebar_hidden: true });
    // A pane it doesn't have isn't focused; the sidebar alone can change.
    assert!(matches!(ipad.request(set("editor", "paper", vec![], None)).await.body, Body::Ack(_)));
    assert!(matches!(ipad.request(set("", "", vec![], Some(false))).await.body, Body::Ack(_)));
    let shown = heard(&mut window, |shown| !shown.sidebar_hidden).await;
    assert_eq!(shown.agents, vec![writer.clone()], "nothing to keep for the editor");

    // Refused, with a reason.
    for (request, why) in [
        (set("nobody", "", vec![], None), "nobody"),
        (set("", "", vec![], None), "agent_id"),
        (set("writer", "", vec![place("paper", 1), place("paper", 2)], None), "different pane"),
        (set("writer", "", vec![place("", 1)], None), "different pane"),
        (set("writer", "", vec![place("paper", 100_001)], None), "page"),
        (set("writer", &"x".repeat(129), vec![], None), "characters"),
    ] {
        let said = refused(&mut ipad, request).await;
        assert!(said.contains(why), "{said}");
    }

    // A new connection of this device hears it after its Welcome.
    let (mut again, _, _) = Client::connect(addr).await;
    let shown = heard(&mut again, |_| true).await;
    assert_eq!(shown, v1::Presentation { agents: vec![writer.clone()], sidebar_hidden: false });

    // Another device's record stays its own, and goes when it's unpaired.
    let (_, laptop) = factoryd::remote::pair(&home(&dir), "Laptop").expect("paired");
    let records = rusqlite::Connection::open(home(&dir).db_path()).expect("factory.db");
    records
        .execute(
            "INSERT INTO presentations (device, agent, focused_leaf, places, updated) VALUES (?1, 'writer', 'notes', '[]', 1)",
            [&laptop.id],
        )
        .expect("the laptop's");
    records.execute("INSERT INTO presentation_sidebars (device, hidden, updated) VALUES (?1, 1, 1)", [&laptop.id]).expect("the laptop's");
    let (mut another, _, _) = Client::connect(addr).await;
    assert_eq!(heard(&mut another, |_| true).await.agents, vec![writer.clone()]);
    assert!(matches!(ipad.request(Request::UnpairDevice(v1::UnpairDevice { device_id: laptop.id.clone() })).await.body, Body::Ack(_)));
    let count = |table: &str| -> i64 {
        records.query_row(&format!("SELECT COUNT(*) FROM {table} WHERE device = ?1"), [&laptop.id], |row| row.get(0)).expect("count")
    };
    assert_eq!((count("presentations"), count("presentation_sidebars")), (0, 0));

    // A pane the layout loses leaves the record.
    apply_request(
        &mut ipad,
        &mut view,
        layout("writer", vec![leaf("notes", "text", "/tmp/notes.md"), leaf("more", "text", "/tmp/more.md")]),
    )
    .await;
    let shown = heard(&mut window, |shown| shown.agents.is_empty()).await;
    assert!(!shown.sidebar_hidden);
    assert!(matches!(ipad.request(set("writer", "notes", vec![place("notes", 4)], None)).await.body, Body::Ack(_)));
    heard(&mut window, |shown| shown.agents.len() == 1).await;
    apply_request(&mut ipad, &mut view, layout("writer", vec![leaf("notes", "text", "/tmp/notes.md"), leaf("other", "text", "/tmp/o.md")]))
        .await;
    assert!(matches!(ipad.request(set("writer", "other", vec![place("notes", 5)], None)).await.body, Body::Ack(_)));
    heard(&mut window, |shown| shown.agents.first().is_some_and(|agent| agent.focused_leaf_id == "other")).await;

    // A deleted agent's record goes with it.
    apply_request(&mut ipad, &mut view, Request::DeleteAgent(v1::DeleteAgent { agent_id: "writer".into() })).await;
    let shown = heard(&mut window, |shown| shown.agents.is_empty()).await;
    assert_eq!(shown, v1::Presentation::default());
    let left: i64 = records.query_row("SELECT COUNT(*) FROM presentations", [], |row| row.get(0)).expect("count");
    assert_eq!(left, 0);
}
