//! What the person can do in the app and Rivet now can too, when they ask
//! (items 8 to 12): stop an agent (agents_stop), start or stop an app
//! (apps_set_running), check for updates (updates_check), refresh a
//! provider's models (models_refresh) and have an agent waiting on usage try
//! now (models_try_now). Deleting a conversation's version is in fork.rs.

use crate::common::{Client, MODEL, Scripted, View, apply_request, create_agent, create_project, send_message, start_with, temp_dir};
use factory_protocol::v1::{self, AgentStatus, client_message, entry::Kind};

fn select(agent: &str) -> client_message::Body {
    client_message::Body::SelectAgent(v1::SelectAgent { agent_id: agent.into() })
}

fn idle(view: &View, agent: &str) -> bool {
    let agent = view.agent(agent);
    agent.status() == AgentStatus::Idle && agent.queued_messages == 0
}

/// `agent` calls `tool`; what it said.
async fn call(client: &mut Client, view: &mut View, agent: &str, tool: &str, args: &str) -> String {
    apply_request(client, view, select(agent)).await;
    let before = view.entries.len();
    apply_request(client, view, send_message(agent, &format!("call {tool} {args}"))).await;
    let output = |view: &View| {
        view.entries.get(before..).unwrap_or_default().iter().find_map(|entry| match &entry.kind {
            Some(Kind::ToolCall(call)) if call.name == tool && !call.output.is_empty() => Some(call.output.clone()),
            _ => None,
        })
    };
    view.follow(client, |view| output(view).is_some() && idle(view, agent)).await;
    output(view).expect("the call's output")
}

#[tokio::test]
async fn rivet_stops_an_agent_checks_for_updates_and_refreshes_models_when_asked() {
    let dir = temp_dir();
    let models = Scripted::signed_in();
    let tools = models.tools.clone();
    let (addr, factory) = start_with(&dir, models).await;
    let (mut client, workspace, _) = Client::connect(addr).await;
    let mut view = View { workspace, ..View::default() };
    apply_request(&mut client, &mut view, create_project("Site", &dir.join("site"))).await;
    apply_request(&mut client, &mut view, create_agent("site", "Mike", "")).await;
    let rivet = v1::UpdateAgent { agent_id: "rivet".into(), model: Some(MODEL.into()), effort: Some("off".into()), ..Default::default() };
    apply_request(&mut client, &mut view, client_message::Body::UpdateAgent(rivet)).await;

    // Only an admin agent stops another.
    apply_request(&mut client, &mut view, select("mike")).await;
    apply_request(&mut client, &mut view, send_message("mike", "hello")).await;
    view.follow(&mut client, |view| view.entries.len() == 2 && idle(view, "mike")).await;
    let mikes = tools.lock().expect("lock").clone();
    assert!(!mikes.contains("agents_stop") && !mikes.contains("apps_set_running"), "{mikes}");

    // Idle, there's nothing to stop; itself, never.
    let said = call(&mut client, &mut view, "rivet", "agents_stop", r#"{"agent":"Mike"}"#).await;
    assert_eq!(said, "Mike isn't working, so there was nothing to stop.");
    let said = call(&mut client, &mut view, "rivet", "agents_stop", r#"{"agent":"rivet"}"#).await;
    assert_eq!(said, "That's you: you stop when you end your reply. Nothing was done.");
    assert!(tools.lock().expect("lock").contains("agents_stop"));

    // Working, it stops where it is.
    apply_request(&mut client, &mut view, select("mike")).await;
    apply_request(&mut client, &mut view, send_message("mike", r#"call bash {"command":"echo started; sleep 20"}"#)).await;
    view.follow(&mut client, |view| view.agent("mike").status() == AgentStatus::Working).await;
    let started = std::time::Instant::now();
    let said = call(&mut client, &mut view, "rivet", "agents_stop", r#"{"agent":"mike"}"#).await;
    assert_eq!(said, "Stopped Mike: its current work ended where it was.");
    apply_request(&mut client, &mut view, select("mike")).await;
    view.follow(&mut client, |view| idle(view, "mike")).await;
    assert!(started.elapsed() < std::time::Duration::from_secs(20), "the command didn't run its 20 seconds");

    // Updates, as Settings' button checks them.
    let said = call(&mut client, &mut view, "rivet", "updates_check", "{}").await;
    assert!(said.starts_with("This Claktory is up to date: "), "{said}");

    // A provider's models, asked for again.
    let said = call(&mut client, &mut view, "rivet", "models_refresh", r#"{"provider":"nowhere"}"#).await;
    assert!(said.starts_with("There's no provider \"nowhere\": the providers are "), "{said}");
    let said = call(&mut client, &mut view, "rivet", "models_refresh", "{}").await;
    assert!(said.starts_with("Asked each provider whose list can be asked for its models again: "), "{said}");

    // Try now, only for an agent that waits on usage.
    let said = call(&mut client, &mut view, "rivet", "models_try_now", r#"{"agent":"Mike"}"#).await;
    assert_eq!(said, "Mike isn't waiting on a provider's usage, so there's nothing to try again. Nothing was done.");

    // An app, only one installed.
    let said = call(&mut client, &mut view, "rivet", "apps_set_running", r#"{"app":"nothing","running":true}"#).await;
    assert!(said.ends_with("Nothing was changed."), "{said}");

    // Never after reading a web page: Mike
    // keeps working, and no app is touched.
    factoryd::pipes::taint_for_test(&factory, "rivet", "browser:https://example.com");
    apply_request(&mut client, &mut view, select("mike")).await;
    apply_request(&mut client, &mut view, send_message("mike", r#"call bash {"command":"echo started; sleep 5"}"#)).await;
    view.follow(&mut client, |view| view.agent("mike").status() == AgentStatus::Working).await;
    let said = call(&mut client, &mut view, "rivet", "agents_stop", r#"{"agent":"mike"}"#).await;
    assert_eq!(
        said,
        "You've read a page on https://example.com, so you can't stop other agents: what you read there may be someone else's instructions. The person can stop one with Stop in its conversation. Nothing was done."
    );
    assert_eq!(view.agent("mike").status(), AgentStatus::Working, "Mike wasn't stopped");
    let said = call(&mut client, &mut view, "rivet", "apps_set_running", r#"{"app":"charts","running":false}"#).await;
    assert_eq!(
        said,
        "You've read a page on https://example.com, so you can't start or stop apps: what you read there may be someone else's instructions. The person can start or stop one with its switch in Marketplace → Apps. Nothing was changed."
    );
    apply_request(&mut client, &mut view, select("mike")).await;
    view.follow(&mut client, |view| idle(view, "mike")).await;
    let ran = view.entries.iter().any(|entry| matches!(&entry.kind, Some(Kind::ToolCall(call)) if call.name == "bash" && call.output.contains("started") && !call.output.contains("Stopped")));
    assert!(ran, "Mike's command ran to its end");
}
