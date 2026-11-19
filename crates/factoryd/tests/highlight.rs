//! Highlights: an agent makes parts of the app glow so the person sees what
//! it means, and both agents glow when one messages another. They're only
//! sent, never stored; the person can turn them off.

mod common;

use common::{Client, View, apply_request, create_agent, create_project, describe, send_message, start, temp_dir};
use factory_protocol::v1::{self, AgentStatus, HighlightKind, client_message, server_message::Body};

fn select(agent: &str) -> client_message::Body {
    client_message::Body::SelectAgent(v1::SelectAgent { agent_id: agent.into() })
}

fn idle(view: &View, agent: &str) -> bool {
    view.workspace.agents.iter().any(|candidate| candidate.id == agent && candidate.status() == AgentStatus::Idle)
}

fn targets(highlight: &v1::Highlight) -> Vec<(String, String)> {
    highlight.targets.iter().map(|target| (target.kind.clone(), target.id.clone())).collect()
}

fn pair(kind: &str, id: &str) -> (String, String) {
    (kind.to_owned(), id.to_owned())
}

/// What `agent`'s tool call said.
async fn call(client: &mut Client, view: &mut View, agent: &str, text: &str) -> String {
    let before = view.entries.len();
    apply_request(client, view, send_message(agent, text)).await;
    view.follow(client, |view| view.entries.len() >= before + 3 && idle(view, agent)).await;
    describe(&view.entries[before + 1..before + 2]).remove(0)
}

#[tokio::test]
async fn agents_highlight_parts_of_the_app_and_messages_light_up_both_agents() {
    let dir = temp_dir();
    let (addr, _factory) = start(&dir).await;
    let (mut client, workspace, _) = Client::connect(addr).await;
    let mut view = View { workspace, ..View::default() };
    assert!(view.workspace.highlights, "on unless turned off");
    let catalog = view.workspace.extensions.iter().find(|extension| extension.id == "highlight").expect("highlight");
    assert_eq!((catalog.name.as_str(), catalog.icon.as_str(), catalog.default_enabled), ("Highlights", "sparkles", false));
    apply_request(&mut client, &mut view, create_project("Site", &dir.join("site"))).await;
    apply_request(&mut client, &mut view, create_agent("site", "Lead", "")).await;
    apply_request(&mut client, &mut view, create_agent("site", "Writer", "")).await;
    let on = v1::ExtensionChange { id: "highlight".into(), enabled: Some(true), ..Default::default() };
    let turn_on = v1::UpdateAgent { agent_id: "lead".into(), extension: Some(on), ..Default::default() };
    apply_request(&mut client, &mut view, client_message::Body::UpdateAgent(turn_on)).await;
    apply_request(&mut client, &mut view, select("lead")).await;

    // A message lights up sender and recipient.
    let said = call(&mut client, &mut view, "lead", r#"call agents_send {"to":"writer","text":"hi"}"#).await;
    assert_eq!(said, "tool agents_send: Sent.");
    let message = view.highlights.iter().find(|highlight| highlight.kind() == HighlightKind::Message).expect("message highlight");
    assert_eq!(targets(message), [pair("agent", "lead"), pair("agent", "writer")]);
    assert_eq!(message.from_agent_id, "lead");
    view.follow(&mut client, |view| idle(view, "writer")).await;

    // The tool shows what exists, with a caption.
    view.highlights.clear();
    let show = r#"call highlight {"targets":[{"kind":"agent","id":"writer"},{"kind":"project","id":"site"},{"kind":"extension","id":"lead/highlight"},{"kind":"settings"}],"text":"Here"}"#;
    assert_eq!(call(&mut client, &mut view, "lead", show).await, "tool highlight: Shown.");
    let shown = view.highlights.iter().find(|highlight| highlight.kind() == HighlightKind::Agent).expect("agent highlight");
    assert_eq!(
        targets(shown),
        [pair("agent", "writer"), pair("project", "site"), pair("extension", "lead/highlight"), pair("settings", "")]
    );
    assert_eq!((shown.text.as_str(), shown.from_agent_id.as_str()), ("Here", "lead"));

    // Nothing is sent for targets that don't exist; the agent hears why.
    view.highlights.clear();
    for (target, why) in [
        (r#"{"kind":"agent","id":"nobody"}"#, "No agent has the id \"nobody\"."),
        (r#"{"kind":"checklist_item","id":"lead#3"}"#, "lead's checklist has no item #3."),
        (r#"{"kind":"extension","id":"writer/memory"}"#, "Writer doesn't have Memory turned on"),
    ] {
        let said = call(&mut client, &mut view, "lead", &format!(r#"call highlight {{"targets":[{target}]}}"#)).await;
        assert!(said.contains(why), "{said}");
    }
    assert!(view.highlights.is_empty(), "{:?}", view.highlights);

    // Turned off, the app is told; the agent hears it isn't seen.
    let off = client_message::Body::SetHighlights(v1::SetHighlights { on: false });
    assert!(matches!(apply_request(&mut client, &mut view, off).await, Body::Ack(_)));
    assert!(!view.workspace.highlights);
    let config: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(dir.join("home/config.json")).expect("config")).expect("json");
    assert_eq!(config["highlights"], false);
    let said = call(&mut client, &mut view, "lead", r#"call highlight {"targets":[{"kind":"marketplace"}]}"#).await;
    assert!(said.starts_with("tool highlight: Highlights are off"), "{said}");
}
