//! A project's brief: written once, in the system prompt of every agent in
//! the project, with a short note to each when it changes.

use crate::common::{Client, Scripted, View, apply_request, create_agent, create_project, describe, send_message, start_with, temp_dir};
use factory_protocol::v1::{self, AgentStatus, client_message, server_message::Body};

fn brief(project: &str, text: &str) -> client_message::Body {
    client_message::Body::UpdateProject(v1::UpdateProject { project_id: project.into(), brief: Some(text.into()), ..Default::default() })
}

fn idle(view: &View, agent: &str) -> bool {
    let agent = view.agent(agent);
    agent.status() == AgentStatus::Idle && agent.queued_messages == 0
}

async fn talk(client: &mut Client, view: &mut View, agent: &str, text: &str) -> Vec<String> {
    apply_request(client, view, client_message::Body::SelectAgent(v1::SelectAgent { agent_id: agent.into() })).await;
    let before = view.entries.len();
    apply_request(client, view, send_message(agent, text)).await;
    view.follow(client, |view| view.entries.len() >= before + 2 && idle(view, agent)).await;
    describe(&view.entries[before..])
}

fn ack(body: Body) {
    assert!(matches!(body, Body::Ack(_)), "{body:?}");
}

#[tokio::test]
async fn a_brief_reaches_every_agent_in_its_project_from_its_next_step() {
    let dir = temp_dir();
    let models = Scripted::signed_in();
    let prompt = models.system_prompt.clone();
    let sent = || prompt.lock().expect("lock").clone();
    let (addr, _factory) = start_with(&dir, models).await;
    let (mut client, workspace, _) = Client::connect(addr).await;
    let mut view = View { workspace, ..View::default() };
    apply_request(&mut client, &mut view, create_project("Site", &dir.join("site"))).await;
    apply_request(&mut client, &mut view, create_project("Shop", &dir.join("shop"))).await;
    apply_request(&mut client, &mut view, create_agent("site", "Lead", "Lead the site.")).await;
    apply_request(&mut client, &mut view, create_agent("site", "Builder", "")).await;
    apply_request(&mut client, &mut view, create_agent("shop", "Clerk", "")).await;
    for agent in ["lead", "builder", "clerk"] {
        talk(&mut client, &mut view, agent, "hello").await;
        assert!(!sent().contains("## Your project's brief"), "none by default: {}", sent());
    }
    let clerk = sent();

    ack(apply_request(&mut client, &mut view, brief("site", "  Ship the shop by Friday.\n")).await);
    assert_eq!(view.workspace.projects.iter().find(|project| project.id == "site").expect("site").brief, "Ship the shop by Friday.");
    let saved = std::fs::read_to_string(dir.join("home/config.json")).expect("config");
    assert!(saved.contains("\"brief\": \"Ship the shop by Friday.\""), "{saved}");

    let said = talk(&mut client, &mut view, "lead", "now?").await;
    assert_eq!(
        said[0],
        "event from Claktory: Your settings changed. Your project's brief changed; the new one is in your system prompt from now on."
    );
    assert!(sent().contains("## Your project's brief\n\nShip the shop by Friday.\n\n## Your instructions\n\nLead the site."), "{}", sent());
    let said = talk(&mut client, &mut view, "builder", "now?").await;
    assert!(said[0].contains("Your project's brief changed"), "{said:?}");
    assert!(sent().contains("## Your project's brief\n\nShip the shop by Friday.\n"), "{}", sent());
    assert_eq!(talk(&mut client, &mut view, "builder", "again").await, ["user: again", "assistant: echo: again"], "told once");
    assert_eq!(talk(&mut client, &mut view, "clerk", "and you?").await, ["user: and you?", "assistant: echo: and you?"]);
    assert_eq!(sent(), clerk, "another project's agents are untouched");

    ack(apply_request(&mut client, &mut view, brief("site", "")).await);
    let said = talk(&mut client, &mut view, "lead", "and now?").await;
    assert!(said[0].ends_with("Your project's brief was removed: it has none from now on."), "{said:?}");
    assert!(!sent().contains("## Your project's brief"), "{}", sent());
    assert!(!std::fs::read_to_string(dir.join("home/config.json")).expect("config").contains("\"brief\""));
}
