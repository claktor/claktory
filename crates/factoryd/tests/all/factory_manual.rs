//! Factory's manual is split: every agent's system prompt has the core, and
//! the rest is the Factory manual, a read-only memory (instance
//! "memory:factory") that admin agents such as Rivet have unless it's turned
//! off, and any agent can be given.

use crate::common::{Client, MODEL, Scripted, View, apply_request, create_agent, create_project, send_message, start_with, temp_dir};
use factory_protocol::v1::{self, AgentStatus, client_message, entry::Kind, server_message::Body};

fn select(agent: &str) -> client_message::Body {
    client_message::Body::SelectAgent(v1::SelectAgent { agent_id: agent.into() })
}

fn turn(agent: &str, key: &str, on: bool) -> client_message::Body {
    let change = v1::ExtensionChange { id: key.into(), enabled: Some(on), ..Default::default() };
    client_message::Body::UpdateAgent(v1::UpdateAgent { agent_id: agent.into(), extension: Some(change), ..Default::default() })
}

fn idle(view: &View, agent: &str) -> bool {
    let agent = view.agent(agent);
    agent.status() == AgentStatus::Idle && agent.queued_messages == 0
}

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

fn tool_names(tools: &str) -> Vec<String> {
    let tools: serde_json::Value = serde_json::from_str(tools).expect("tools");
    tools.as_array().expect("list").iter().filter_map(|tool| tool["name"].as_str().map(str::to_owned)).collect()
}

#[tokio::test]
async fn rivet_reads_the_factory_manual_and_other_agents_get_the_core() {
    let dir = temp_dir();
    let models = Scripted::signed_in();
    let (system_prompt, tools) = (models.system_prompt.clone(), models.tools.clone());
    let (addr, _factory) = start_with(&dir, models).await;
    let (mut client, workspace, _) = Client::connect(addr).await;
    let mut view = View { workspace, ..View::default() };
    let rivet = v1::UpdateAgent { agent_id: "rivet".into(), model: Some(MODEL.into()), effort: Some("off".into()), ..Default::default() };
    apply_request(&mut client, &mut view, client_message::Body::UpdateAgent(rivet)).await;
    apply_request(&mut client, &mut view, create_project("Site", &dir.join("site"))).await;
    apply_request(&mut client, &mut view, create_agent("site", "Helper", "")).await;

    // Rivet, the admin agent, has it by default: the core manual, the notes'
    // index, and memory_read to open them (nothing to write: it's read only).
    let rivet_manual = view.agent("rivet").extensions.iter().find(|extension| extension.id == "memory:factory").cloned();
    assert!(rivet_manual.expect("listed").enabled);
    let opened = call(&mut client, &mut view, "rivet", "memory_read", r#"{"note": "settings-file"}"#).await;
    let config = dir.join("home/config.json");
    assert!(opened.starts_with("settings-file: The settings file"), "{opened}");
    assert!(opened.contains(&config.display().to_string()) && !opened.contains("{{"), "with this Factory's paths: {opened}");
    let prompt = system_prompt.lock().expect("lock").clone();
    assert!(prompt.contains("# Claktory manual") && prompt.contains("## Agents"));
    assert!(prompt.contains("## Claktory manual notes\n\n- app-claktories: "), "the index");
    assert!(!prompt.contains("### The settings file"), "notes stay out of the prompt");
    let names = tool_names(&tools.lock().expect("lock"));
    assert!(names.contains(&"memory_read".to_owned()) && !names.contains(&"memory_update".to_owned()), "{names:?}");
    let searched = call(&mut client, &mut view, "rivet", "memory_read", r#"{"q": "compact_at_tokens"}"#).await;
    assert!(searched.lines().any(|line| line.starts_with("settings-file:")), "{searched}");
    let missing = call(&mut client, &mut view, "rivet", "memory_read", r#"{"note": "nope"}"#).await;
    assert!(missing.contains("The Claktory manual has no note \"nope\". Notes: app-claktories, app-sidebar, "), "{missing}");

    // Other agents get the core only…
    call(&mut client, &mut view, "helper", "agents_status", "{}").await;
    let prompt = system_prompt.lock().expect("lock").clone();
    assert!(prompt.contains("## Agents") && !prompt.contains("Claktory manual notes"));
    assert!(!tool_names(&tools.lock().expect("lock")).contains(&"memory_read".to_owned()));

    // …unless it's turned on for them. With their own memory on too, an
    // instance parameter picks which.
    apply_request(&mut client, &mut view, turn("helper", "memory:factory", true)).await;
    apply_request(&mut client, &mut view, turn("helper", "memory", true)).await;
    let clear = v1::RunCommand { agent_id: "helper".into(), name: "clear".into(), args: String::new() };
    apply_request(&mut client, &mut view, client_message::Body::RunCommand(clear)).await;
    let own = call(&mut client, &mut view, "helper", "memory_read", "{}").await;
    assert!(own.starts_with("Preferences:"), "{own}");
    let manual = call(&mut client, &mut view, "helper", "memory_read", r#"{"instance": "factory", "note": "app-updates"}"#).await;
    assert!(manual.starts_with("app-updates: "), "{manual}");
    let tools: serde_json::Value = serde_json::from_str(&tools.lock().expect("lock")).expect("tools");
    let read = tools.as_array().expect("list").iter().find(|tool| tool["name"] == "memory_read").expect("memory_read").clone();
    assert_eq!(read["parameters"]["properties"]["instance"]["enum"], serde_json::json!(["factory"]), "{read}");
    let saved: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&config).expect("config")).expect("json");
    let helper = saved["agents"].as_array().expect("agents").iter().find(|agent| agent["id"] == "helper").expect("helper").clone();
    assert_eq!(helper["extensions"]["memory:factory"], serde_json::json!({"enabled": true}));

    // Its panel: the notes, read only.
    let open = v1::OpenPanel { extension_id: "memory:factory".into(), agent_id: "helper".into() };
    apply_request(&mut client, &mut view, client_message::Body::OpenPanel(open)).await;
    let panel = view.extension_view.clone().expect("a view");
    assert_eq!((panel.extension_id.as_str(), panel.title.as_str()), ("memory:factory", "Claktory manual"));
    match panel.body {
        Some(v1::extension_view::Body::Memory(memory)) => {
            assert!(memory.read_only && memory.preferences.is_empty() && memory.notes.len() >= 15);
            assert!(memory.notes.iter().all(|note| !note.body.contains("{{")));
        }
        other => panic!("expected the manual's notes, got {other:?}"),
    }

    // Turned off for Rivet, it stays off.
    match apply_request(&mut client, &mut view, turn("rivet", "memory:factory", false)).await {
        Body::Ack(_) => {}
        other => panic!("expected an ack, got {other:?}"),
    }
    assert!(!view.agent("rivet").extensions.iter().any(|extension| extension.id == "memory:factory" && extension.enabled));
}
