//! An agent suggests that the person talk to another agent themselves
//! (agents_suggest_talk): the app shows a card with a button that opens that
//! agent with the suggested message drafted. Nothing is sent to anyone.

use crate::common::{Client, View, apply_request, create_agent, create_project, send_message, start, temp_dir};
use factory_protocol::v1::{self, AgentStatus, client_message, entry::Kind};

fn select(agent: &str) -> client_message::Body {
    client_message::Body::SelectAgent(v1::SelectAgent { agent_id: agent.into() })
}

fn idle(view: &View, agent: &str) -> bool {
    let agent = view.agent(agent);
    agent.status() == AgentStatus::Idle && agent.queued_messages == 0
}

/// A Factory with a project "Site", its agent Builder, and Arjun (DevOps).
async fn setup(dir: &std::path::Path) -> (Client, View) {
    let (addr, _factory) = start(dir).await;
    let (mut client, workspace, _) = Client::connect(addr).await;
    let mut view = View { workspace, ..View::default() };
    apply_request(&mut client, &mut view, create_project("Site", &dir.join("site"))).await;
    apply_request(&mut client, &mut view, create_agent("site", "Builder", "")).await;
    let mut arjun = create_agent("site", "Arjun", "");
    if let client_message::Body::CreateAgent(create) = &mut arjun {
        create.role = "DevOps".into();
    }
    apply_request(&mut client, &mut view, arjun).await;
    (client, view)
}

/// Builder calls agents_suggest_talk with `args`; returns the call's output
/// and details.
async fn suggest(client: &mut Client, view: &mut View, args: serde_json::Value) -> (String, String) {
    apply_request(client, view, select("builder")).await;
    let before = view.entries.len();
    apply_request(client, view, send_message("builder", &format!("call agents_suggest_talk {args}"))).await;
    view.follow(client, |view| view.entries.len() >= before + 3 && idle(view, "builder")).await;
    view.entries[before..]
        .iter()
        .find_map(|entry| match &entry.kind {
            Some(Kind::ToolCall(call)) if call.name == "agents_suggest_talk" => Some((call.output.clone(), call.details.clone())),
            _ => None,
        })
        .expect("the call")
}

const MESSAGE: &str = "Builder needs the Cloudflare token for the site's DNS. Ask me for it and give it to Builder.";

#[tokio::test]
async fn a_suggestion_carries_the_agent_and_the_drafted_message_and_sends_nothing() {
    let dir = temp_dir();
    let (mut client, mut view) = setup(&dir).await;

    let args = serde_json::json!({"agent": "arjun", "message": format!("  {MESSAGE}\n"), "reason": " Arjun sets up secrets. "});
    let (said, details) = suggest(&mut client, &mut view, args).await;
    assert_eq!(
        said,
        "Suggested. The person sees a card with a button that opens Arjun's conversation with your message drafted; nothing is sent until they send it."
    );
    let details: serde_json::Value = serde_json::from_str(&details).expect("details are JSON");
    assert_eq!(
        details,
        serde_json::json!({
            "agent_id": "arjun",
            "agent_name": "Arjun",
            "agent_role": "DevOps",
            "message": MESSAGE,
            "reason": "Arjun sets up secrets.",
        })
    );

    // Arjun got nothing: the person sends it, if they choose to.
    apply_request(&mut client, &mut view, select("arjun")).await;
    assert!(view.entries.is_empty() && idle(&view, "arjun"), "{:?}", view.entries);
}

#[tokio::test]
async fn a_suggestion_names_an_agent_the_proposer_can_message_and_says_something() {
    let dir = temp_dir();
    let (mut client, mut view) = setup(&dir).await;

    let nobody = serde_json::json!({"agent": "Nobody", "message": MESSAGE, "reason": "Why not."});
    let (said, details) = suggest(&mut client, &mut view, nobody).await;
    assert_eq!(said, "No agent you can message is called \"Nobody\"; you can message: arjun.");
    assert!(details.is_empty() || details == "{}", "no card for a refusal: {details}");

    let me = serde_json::json!({"agent": "Builder", "message": MESSAGE, "reason": "Why not."});
    assert_eq!(suggest(&mut client, &mut view, me).await.0, "That's you.");

    let blank = serde_json::json!({"agent": "Arjun", "message": "  ", "reason": "Why not."});
    assert_eq!(suggest(&mut client, &mut view, blank).await.0, "Write the message they'd send in `message`.");
}
