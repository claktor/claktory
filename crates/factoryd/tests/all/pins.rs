//! Pinned agents: an ordered
//! list in config.json, shown in Settings, set with PinAgent and
//! UnpinAgent, and pruned of agents that can't be pinned.

use crate::common::{Client, View, apply_request, create_agent, create_project, home_of, start, temp_dir};
use factory_protocol::v1::{self, client_message::Body as Request, server_message::Body};

fn pin(agent: &str, before: &str) -> Request {
    Request::PinAgent(v1::PinAgent { agent_id: agent.into(), before_id: before.into() })
}

fn unpin(agent: &str) -> Request {
    Request::UnpinAgent(v1::UnpinAgent { agent_id: agent.into() })
}

fn pinned(view: &View) -> Vec<String> {
    view.workspace.settings.as_ref().expect("settings").pinned_agents.clone()
}

fn ack(reply: Body) {
    assert!(matches!(reply, Body::Ack(_)), "{reply:?}");
}

fn refused(reply: Body) -> String {
    match reply {
        Body::Error(error) => error.message,
        other => panic!("expected a refusal, got {other:?}"),
    }
}

#[tokio::test]
async fn pins_keep_the_persons_order_and_go_with_what_unpins_them() {
    let dir = temp_dir();
    let (addr, _factory) = start(&dir).await;
    let (mut client, workspace, _) = Client::connect(addr).await;
    let mut view = View { workspace, ..View::default() };
    ack(apply_request(&mut client, &mut view, create_project("Site", &dir.join("site"))).await);
    ack(apply_request(&mut client, &mut view, create_project("Lab", &dir.join("lab"))).await);
    for (project, name) in [("site", "Mike"), ("site", "Sam"), ("lab", "Zoe"), ("lab", "Ada")] {
        ack(apply_request(&mut client, &mut view, create_agent(project, name, "")).await);
    }
    let config = || std::fs::read_to_string(home_of(addr).config_path()).expect("config.json");
    assert!(!config().contains("pinned_agents"), "written only once there's one");

    // Pinned last, before another, and moved by pinning again.
    ack(apply_request(&mut client, &mut view, pin("mike", "")).await);
    ack(apply_request(&mut client, &mut view, pin("sam", "")).await);
    ack(apply_request(&mut client, &mut view, pin("zoe", "sam")).await);
    assert_eq!(pinned(&view), ["mike", "zoe", "sam"]);
    ack(apply_request(&mut client, &mut view, pin("mike", "")).await);
    assert_eq!(pinned(&view), ["zoe", "sam", "mike"]);
    ack(apply_request(&mut client, &mut view, pin("mike", "zoe")).await);
    ack(apply_request(&mut client, &mut view, pin("ada", "nobody")).await);
    assert_eq!(pinned(&view), ["mike", "zoe", "sam", "ada"]);
    let saved: serde_json::Value = serde_json::from_str(&config()).expect("JSON");
    assert_eq!(saved["pinned_agents"], serde_json::json!(["mike", "zoe", "sam", "ada"]));

    // Rivet is at the top already; an agent that isn't there can't be.
    let said = refused(apply_request(&mut client, &mut view, pin("rivet", "")).await);
    assert!(said.contains("outside projects"), "{said}");
    refused(apply_request(&mut client, &mut view, pin("nobody", "")).await);

    // Unpinned, and again harmlessly.
    ack(apply_request(&mut client, &mut view, unpin("sam")).await);
    ack(apply_request(&mut client, &mut view, unpin("sam")).await);
    assert_eq!(pinned(&view), ["mike", "zoe", "ada"]);

    // Moved to another project, it keeps its pin; out of projects, deleted,
    // or archived with its project, it's unpinned.
    let to_lab = v1::UpdateAgent { agent_id: "mike".into(), project_id: Some("lab".into()), ..Default::default() };
    ack(apply_request(&mut client, &mut view, Request::UpdateAgent(to_lab)).await);
    assert_eq!(pinned(&view), ["mike", "zoe", "ada"]);
    let out = v1::UpdateAgent { agent_id: "zoe".into(), project_id: Some(String::new()), ..Default::default() };
    ack(apply_request(&mut client, &mut view, Request::UpdateAgent(out)).await);
    assert_eq!(pinned(&view), ["mike", "ada"]);
    ack(apply_request(&mut client, &mut view, Request::DeleteAgent(v1::DeleteAgent { agent_id: "ada".into() })).await);
    assert_eq!(pinned(&view), ["mike"]);
    let archive = v1::UpdateProject { project_id: "lab".into(), archived: Some(true), ..Default::default() };
    ack(apply_request(&mut client, &mut view, Request::UpdateProject(archive)).await);
    assert!(pinned(&view).is_empty());
    assert!(!config().contains("pinned_agents"));
    // An archived project's agent can't be pinned; another can.
    ack(apply_request(&mut client, &mut view, pin("sam", "")).await);
    assert_eq!(pinned(&view), ["sam"]);
    let said = refused(apply_request(&mut client, &mut view, pin("mike", "")).await);
    assert!(said.contains("archived"), "{said}");
}

/// Rivet pins by editing config.json: what can't be pinned is left out and
/// written back.
#[tokio::test]
async fn rivet_pins_in_the_settings_file() {
    let dir = temp_dir();
    let (addr, _factory) = start(&dir).await;
    let (mut client, workspace, _) = Client::connect(addr).await;
    let mut view = View { workspace, ..View::default() };
    ack(apply_request(&mut client, &mut view, create_project("Site", &dir.join("site"))).await);
    ack(apply_request(&mut client, &mut view, create_agent("site", "Mike", "")).await);
    let path = home_of(addr).config_path();
    let mut config: serde_json::Value = serde_json::from_slice(&std::fs::read(&path).expect("config")).expect("JSON");
    config["pinned_agents"] = serde_json::json!(["mike", "rivet", "ghost", "mike"]);
    std::fs::write(&path, serde_json::to_vec_pretty(&config).expect("JSON")).expect("written");
    view.follow(&mut client, |view| view.workspace.settings.as_ref().is_some_and(|settings| !settings.pinned_agents.is_empty())).await;
    assert_eq!(pinned(&view), ["mike"]);
    let saved: serde_json::Value = serde_json::from_slice(&std::fs::read(&path).expect("config")).expect("JSON");
    assert_eq!(saved["pinned_agents"], serde_json::json!(["mike"]));
}
