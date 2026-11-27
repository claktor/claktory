//! The Database extension: an agent's own SQLite database, and Factory's
//! records (factory.db) to read for admin agents.

use crate::common::{Client, MODEL, View, apply_request, create_agent, create_project, send_message, start, temp_dir};
use factory_protocol::v1::{self, AgentStatus, client_message, entry::Kind};

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

fn turn(agent: &str, extension: &str, on: bool) -> client_message::Body {
    let change = v1::ExtensionChange { id: extension.into(), enabled: Some(on), ..Default::default() };
    client_message::Body::UpdateAgent(v1::UpdateAgent { agent_id: agent.into(), extension: Some(change), ..Default::default() })
}

fn panel(view: &View) -> v1::Database {
    match view.extension_view.as_ref().and_then(|view| view.body.clone()) {
        Some(v1::extension_view::Body::Database(database)) => database,
        other => panic!("expected the Database panel, got {other:?}"),
    }
}

#[tokio::test]
async fn an_agent_keeps_records_in_its_own_database_and_its_panel_follows() {
    let dir = temp_dir();
    let (addr, _factory) = start(&dir).await;
    let (mut client, workspace, _) = Client::connect(addr).await;
    let mut view = View { workspace, ..View::default() };
    apply_request(&mut client, &mut view, create_project("Site", &dir.join("site"))).await;
    apply_request(&mut client, &mut view, create_agent("site", "Scout", "")).await;
    // Off unless turned on, for agents that collect data worth querying.
    let database =
        |view: &View| view.agent("scout").extensions.iter().find(|extension| extension.id == "database").expect("database").enabled;
    assert!(!database(&view));
    apply_request(&mut client, &mut view, turn("scout", "database", true)).await;
    assert!(database(&view));
    apply_request(&mut client, &mut view, select("scout")).await;
    let open = v1::OpenPanel { extension_id: "database".into(), agent_id: "scout".into() };
    apply_request(&mut client, &mut view, client_message::Body::OpenPanel(open)).await;
    let file = dir.join("home").join("agents").join("scout").join("database.sqlite");
    assert!(panel(&view).tables.is_empty());
    assert!(std::path::Path::new(&panel(&view).path).ends_with("agents/scout/database.sqlite"), "{}", panel(&view).path);

    // Nothing is made until the first write.
    assert_eq!(call(&mut client, &mut view, "scout", "db_schema", "{}").await, "No tables yet; create them with db_exec.");
    let empty = call(&mut client, &mut view, "scout", "db_query", r#"{"sql": "SELECT 1"}"#).await;
    assert!(empty.contains("Your database has no tables yet"), "{empty}");
    assert!(!file.exists());

    let made = r#"{"sql": "CREATE TABLE leads (id INTEGER PRIMARY KEY, name TEXT NOT NULL, score REAL); CREATE VIEW good AS SELECT name FROM leads WHERE score > 0.5;"}"#;
    assert_eq!(call(&mut client, &mut view, "scout", "db_exec", made).await, "ok");
    assert!(file.exists());
    let added = r#"{"sql": "INSERT INTO leads (name, score) VALUES (?1, ?2)", "params": ["Ada", 0.9]}"#;
    assert_eq!(call(&mut client, &mut view, "scout", "db_exec", added).await, "ok: 1 row changed, last id 1");
    assert_eq!(
        call(&mut client, &mut view, "scout", "db_query", r#"{"sql": "SELECT name, score FROM leads"}"#).await,
        "name | score\nAda | 0.9"
    );
    let refused = call(&mut client, &mut view, "scout", "db_query", r#"{"sql": "DELETE FROM leads"}"#).await;
    assert!(refused.contains("db_query only reads"), "{refused}");

    // The panel shows its tables and views as they change.
    view.follow(&mut client, |view| panel(view).tables.len() == 2 && panel(view).tables[1].rows == 1).await;
    let tables = panel(&view).tables;
    assert_eq!((tables[0].name.as_str(), tables[0].view, tables[1].name.as_str(), tables[1].view), ("good", true, "leads", false));
    let columns: Vec<(&str, &str)> = tables[1].columns.iter().map(|column| (column.name.as_str(), column.r#type.as_str())).collect();
    assert_eq!(columns, [("id", "INTEGER"), ("name", "TEXT"), ("score", "REAL")]);

    // Turned off, its tools are refused; its data stays.
    apply_request(&mut client, &mut view, turn("scout", "database", false)).await;
    let off = call(&mut client, &mut view, "scout", "db_query", r#"{"sql": "SELECT * FROM leads"}"#).await;
    assert!(off.contains("turned off"), "{off}");
    assert!(file.exists());
}

#[tokio::test]
async fn admin_agents_read_factorys_records_but_not_agents_memory() {
    let dir = temp_dir();
    let (addr, _factory) = start(&dir).await;
    let (mut client, workspace, _) = Client::connect(addr).await;
    let mut view = View { workspace, ..View::default() };
    let rivet = v1::UpdateAgent { agent_id: "rivet".into(), model: Some(MODEL.into()), effort: Some("off".into()), ..Default::default() };
    apply_request(&mut client, &mut view, client_message::Body::UpdateAgent(rivet)).await;

    // An admin agent has Factory's records, and no database of its own: they're what db_query reads, read only.
    let only =
        call(&mut client, &mut view, "rivet", "db_query", r#"{"sql": "SELECT COUNT(*) AS n FROM runs WHERE agent = 'rivet'"}"#).await;
    assert!(only.starts_with("n\n") && only != "n\n0", "{only}");
    let write = call(&mut client, &mut view, "rivet", "db_query", r#"{"sql": "DELETE FROM runs"}"#).await;
    assert!(write.contains("db_query only reads"), "{write}");

    // With its own database on too, `instance` picks the records.
    apply_request(&mut client, &mut view, turn("rivet", "database", true)).await;
    let clear = v1::RunCommand { agent_id: "rivet".into(), name: "clear".into(), args: String::new() };
    apply_request(&mut client, &mut view, client_message::Body::RunCommand(clear)).await;
    let sent = r#"{"sql": "SELECT text FROM messages WHERE agent = ?1 ORDER BY created DESC LIMIT 1", "params": ["rivet"], "instance": "factory"}"#;
    let read = call(&mut client, &mut view, "rivet", "db_query", sent).await;
    assert!(read.starts_with("text\ncall db_query"), "{read}");
    let tables = call(&mut client, &mut view, "rivet", "db_schema", r#"{"instance": "factory"}"#).await;
    assert!(tables.contains("messages (") && tables.contains("runs (") && !tables.contains("memory_"), "{tables}");
    let memory = call(&mut client, &mut view, "rivet", "db_query", r#"{"sql": "SELECT * FROM memory_notes", "instance": "factory"}"#).await;
    assert!(memory.contains("aren't readable"), "{memory}");
    // Without an instance it's Rivet's own database.
    assert_eq!(call(&mut client, &mut view, "rivet", "db_schema", "{}").await, "No tables yet; create them with db_exec.");

    // The records' panel.
    let open = v1::OpenPanel { extension_id: "database:factory".into(), agent_id: "rivet".into() };
    apply_request(&mut client, &mut view, client_message::Body::OpenPanel(open)).await;
    let records = panel(&view);
    assert!(records.path.ends_with("factory.db"), "{}", records.path);
    let names: Vec<&str> = records.tables.iter().map(|table| table.name.as_str()).collect();
    assert!(names.contains(&"messages") && !names.iter().any(|name| name.starts_with("memory_")), "{names:?}");
}
