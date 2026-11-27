//! Extensions' tools return structured details with their text, for the
//! app's view of each call (docs/EXTENSIONS.md, "Its view in the
//! conversation"): ids, names, counts and what changed, never prose to parse.

use crate::common::{Client, View, apply_request, create_agent, create_project, send_message, start, temp_dir};
use factory_protocol::v1::{self, AgentStatus, client_message, entry::Kind};
use serde_json::{Value, json};

fn idle(view: &View, agent: &str) -> bool {
    let agent = view.agent(agent);
    agent.status() == AgentStatus::Idle && agent.queued_messages == 0
}

fn turn_on(agent: &str, extension: &str) -> client_message::Body {
    let change = v1::ExtensionChange { id: extension.into(), enabled: Some(true), ..Default::default() };
    client_message::Body::UpdateAgent(v1::UpdateAgent { agent_id: agent.into(), extension: Some(change), ..Default::default() })
}

/// Has Builder call `tool` with `args` and returns the call's details.
async fn details(client: &mut Client, view: &mut View, tool: &str, args: Value) -> Value {
    let before = view.entries.len();
    apply_request(client, view, send_message("builder", &format!("call {tool} {args}"))).await;
    view.follow(client, |view| view.entries.len() >= before + 3 && idle(view, "builder")).await;
    let call = view.entries[before..]
        .iter()
        .find_map(|entry| match &entry.kind {
            Some(Kind::ToolCall(call)) => Some(call.clone()),
            _ => None,
        })
        .expect("a tool call");
    assert_eq!(call.state(), v1::ToolCallState::Done, "{tool}: {}", call.output);
    if call.details.is_empty() {
        return Value::Null;
    }
    serde_json::from_str(&call.details).expect("details are JSON")
}

#[tokio::test]
async fn extension_tools_say_what_they_did_in_details() {
    let dir = temp_dir();
    let (addr, _factory) = start(&dir).await;
    let (mut client, workspace, _) = Client::connect(addr).await;
    let mut view = View { workspace, ..View::default() };
    apply_request(&mut client, &mut view, create_project("Site", &dir.join("site"))).await;
    apply_request(&mut client, &mut view, create_agent("site", "Builder", "")).await;
    apply_request(&mut client, &mut view, create_agent("site", "Writer", "")).await;
    for extension in ["checklist", "memory", "database"] {
        apply_request(&mut client, &mut view, turn_on("builder", extension)).await;
    }
    let select = client_message::Body::SelectAgent(v1::SelectAgent { agent_id: "builder".into() });
    apply_request(&mut client, &mut view, select).await;
    let (client, view) = (&mut client, &mut view);

    // Messaging: whom it went to.
    let sent = details(client, view, "agents_send", json!({"to": "Writer", "text": "Hello"})).await;
    assert_eq!(sent["to"], "writer");
    assert_eq!(sent["name"], "Writer");
    assert_eq!(sent["role"], "");
    assert!(sent["working"].is_boolean());
    assert_eq!(sent["text"], "Hello");
    assert_eq!(sent["recipients"][0]["id"], "writer");
    assert_eq!(sent["recipients"][0]["to"], "Writer");
    let several = details(client, view, "agents_send", json!({"to": ["Writer", "nobody"], "text": "Hi"})).await;
    assert_eq!(several["recipients"][0]["name"], "Writer");
    assert_eq!(several["recipients"][1]["outcome"], "failed");
    assert!(several.get("to").is_none());

    // Checklist: the items added, then each one as it is after a change.
    let added = details(client, view, "checklist_add", json!({"items": ["Fix login", "Dark mode"], "owner": "writer"})).await;
    assert_eq!(
        added,
        json!({"agent": "builder", "items": [{"id": 1, "text": "Fix login", "owner": "writer"}, {"id": 2, "text": "Dark mode", "owner": "writer"}]})
    );
    let updates = json!({"updates": [
        {"id": 1, "status": "done", "note": "shipped"},
        {"id": 2, "owner": "", "text": "Dark mode everywhere"},
        {"id": 9, "status": "done"},
        {"id": 2},
    ]});
    let updated = details(client, view, "checklist_update", updates).await;
    assert_eq!(
        updated["items"],
        json!([
            {"id": 1, "text": "Fix login", "status": "done", "owner": "writer", "changed": ["status", "note"], "note": "shipped"},
            {"id": 2, "text": "Dark mode everywhere", "status": "open", "owner": "", "changed": ["owner", "text"]},
            {"id": 9, "problem": "no such item"},
            {"id": 2, "problem": "nothing to change"},
        ])
    );
    let listed = details(client, view, "checklist_list", json!({"status": "all", "limit": 1})).await;
    assert_eq!(listed, json!({"agent": "builder", "shown": 1, "total": 2, "goals": 0}));
    let item = details(client, view, "checklist_item", json!({"id": 1})).await;
    assert_eq!(item, json!({"agent": "builder", "id": 1, "text": "Fix login", "status": "done", "owner": "writer"}));

    // Memory: what changed, by id or slug, with its title.
    let ops = json!({"ops": [
        {"op": "pref_add", "rule": "Short answers", "why": "Matt reads on his phone"},
        {"op": "note_write", "slug": "release", "summary": "How to release", "body": "Stamp, then release."},
    ]});
    let wrote = details(client, view, "memory_update", ops).await;
    assert_eq!(
        wrote["changes"],
        json!([
            {"kind": "preference", "change": "added", "id": 1, "title": "Short answers"},
            {"kind": "note", "change": "written", "slug": "release", "title": "How to release"},
        ])
    );
    let removed = details(client, view, "memory_update", json!({"ops": [{"op": "note_remove", "slug": "release"}]})).await;
    assert_eq!(removed["changes"], json!([{"kind": "note", "change": "removed", "slug": "release", "title": "How to release"}]));
    let overview = details(client, view, "memory_read", json!({})).await;
    assert_eq!(overview, json!({"instance": "", "preferences": 1, "notes": 0}));
    let found = details(client, view, "memory_read", json!({"q": "short"})).await;
    assert_eq!(found, json!({"instance": "", "query": "short", "hits": 1}));

    // Monitors: added with its name and schedule, then changed.
    let monitor = json!({"monitors": [{"name": "Nightly", "prompt": "Check the build.", "when": {"every": "24h"}}]});
    let monitors = details(client, view, "monitors_add", monitor).await;
    let added = &monitors["monitors"][0];
    assert_eq!((&added["id"], &added["agent"], &added["name"]), (&json!("m1"), &json!("builder"), &json!("Nightly")));
    assert_eq!(added["schedule"], "every 1d");
    assert!(added["next"].as_i64().is_some_and(|next| next > 0), "{added}");
    let listed = details(client, view, "monitors_list", json!({})).await;
    assert_eq!(listed, json!({"agent": "builder", "count": 1}));
    let paused = details(client, view, "monitors_change", json!({"changes": [{"id": "m1", "action": "pause"}]})).await;
    assert_eq!(paused, json!({"monitors": [{"id": "m1", "name": "Nightly", "action": "paused"}]}));

    // Tasks: whose, and each task's state.
    let tasks = details(client, view, "tasks_list", json!({})).await;
    assert_eq!(tasks, json!({"agent": "builder", "tasks": []}));

    // Database: counts, and which database.
    let made =
        details(client, view, "db_exec", json!({"sql": "CREATE TABLE leads (name TEXT); INSERT INTO leads VALUES ('Ada'), ('Bo');"})).await;
    assert_eq!(made, json!({"instance": "", "changed": 2, "last_id": 2}));
    let rows = details(client, view, "db_query", json!({"sql": "SELECT name FROM leads"})).await;
    assert_eq!(rows, json!({"instance": "", "columns": ["name"], "rows": 2, "more": 0}));
    let schema = details(client, view, "db_schema", json!({})).await;
    assert_eq!(schema, json!({"instance": "", "tables": 1}));
}

/// Images: what the app draws an image_generate call from, these
/// keys and no others (`crate::image_generate` checks their values).
#[tokio::test]
async fn image_generate_says_what_it_made_in_details() {
    use crate::image_generate::{call, create, painter, setup};
    let (_dir, mut client, mut view) = setup(painter("ChatGPT Plus")).await;
    apply_request(&mut client, &mut view, create("Painter", crate::common::OTHER_MODEL)).await;
    let made = call(&mut client, &mut view, "painter", json!({"prompt": "a lighthouse"})).await;
    assert_eq!(made.state(), v1::ToolCallState::Done, "{}", made.output);
    let details: Value = serde_json::from_str(&made.details).expect("details are JSON");
    let mut keys: Vec<&str> = details.as_object().expect("an object").keys().map(String::as_str).collect();
    keys.sort_unstable();
    assert_eq!(keys, ["bytes", "height", "mime", "model", "path", "prompt", "quality", "size", "width"]);
    for number in ["bytes", "height", "width"] {
        assert!(details[number].is_u64(), "{number}: {details}");
    }
}
