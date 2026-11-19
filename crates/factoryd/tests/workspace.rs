mod common;

use std::path::Path;

use common::{Client, MODEL, View, apply_request, create_agent, create_project, describe, last_workspace, send_message, start, temp_dir};
use factory_protocol::v1::{self, AgentStatus, ErrorCode, client_message, server_message::Body};
use factoryd::config::Config;

#[tokio::test]
async fn created_agents_are_saved_selected_and_shared_by_every_client() {
    let dir = temp_dir();
    let (addr, _factory) = start(&dir).await;
    let (mut client, _, _) = Client::connect(addr).await;

    let events = client.ok(create_project("Web Site", &dir.join("site"))).await;
    assert_eq!(last_workspace(&events).expect("workspace before ack").projects[0].id, "web-site");
    assert!(dir.join("site").is_dir());

    // The new agent and its (empty) transcript arrive before the ack.
    let events = client.ok(create_agent("web-site", "Release Notes", "Write the release notes.")).await;
    let workspace = last_workspace(&events).expect("workspace before ack");
    assert_eq!(workspace.selected_agent_id, "release-notes");
    let agent = workspace.agents.iter().find(|agent| agent.id == "release-notes").expect("new agent");
    assert_eq!((agent.project_id.as_str(), agent.model.as_str(), agent.effort.as_str()), ("web-site", MODEL, "off"));
    assert_eq!(agent.compact_at_tokens, 400_000);
    let transcript = events.iter().find_map(|event| match &event.body {
        Some(Body::Transcript(transcript)) => Some(transcript),
        _ => None,
    });
    assert_eq!(transcript.expect("transcript before ack").agent_id, "release-notes");

    // A second agent with the same name gets its own id.
    client.ok(create_agent("web-site", "Release Notes", "")).await;

    let text = std::fs::read_to_string(dir.join("home/config.json")).expect("config.json");
    let config = Config::parse(&text).expect("valid config");
    let mut ids = Vec::new();
    for agent in &config.agents {
        ids.push(agent.id.as_str());
    }
    assert_eq!(ids, ["rivet", "release-notes", "release-notes-2"]);
    assert_eq!(config.agent("release-notes").expect("saved").prompt, "Write the release notes.");

    // Another device sees the same workspace, selection included.
    let (_other, workspace, transcript) = Client::connect(addr).await;
    assert_eq!(workspace.selected_agent_id, "release-notes-2");
    assert_eq!(transcript.expect("selected transcript").agent_id, "release-notes-2");
}

#[tokio::test]
async fn edits_to_config_json_apply_live_and_broken_edits_keep_the_last_good_settings() {
    let dir = temp_dir();
    let (addr, _factory) = start(&dir).await;
    let (mut client, _, _) = Client::connect(addr).await;
    client.ok(create_project("Docs", &dir.join("docs"))).await;
    let config_path = dir.join("home/config.json");

    // What an agent does to change Factory: edit the file. This one still uses
    // the old name for the prompt, "purpose".
    let edited = format!(
        r#"{{
  "projects": [{{ "id": "docs", "name": "Docs", "path": "{}" }}],
  "agents": [{{ "id": "editor", "project": "docs", "name": "Editor", "purpose": "Edit the docs.", "model": "{MODEL}", "effort": "off" }}]
}}"#,
        dir.join("docs").display()
    );
    std::fs::write(&config_path, &edited).expect("write");
    let mut view = View::default();
    view.follow(&mut client, |view| view.workspace.agents.len() == 1).await;
    assert_eq!(view.agent("editor").prompt, "Edit the docs.");
    let mut config = Config::parse(&edited).expect("parse");

    std::fs::write(&config_path, "{ \"projects\": [").expect("write");
    view.follow(&mut client, |view| !view.workspace.config_error.is_empty()).await;
    assert_eq!(view.workspace.agents.len(), 1, "keeps the last good settings");

    // The app can't save over a file someone is in the middle of fixing.
    let reply = client.request(create_project("Other", &dir.join("other"))).await;
    match reply.body {
        Body::Error(error) => assert_eq!(error.code(), ErrorCode::InvalidRequest),
        other => panic!("expected an error, got {other:?}"),
    }
    assert_eq!(std::fs::read_to_string(&config_path).expect("read"), "{ \"projects\": [");

    // Restoring from the app puts the working settings back and keeps the broken file.
    let restore = client_message::Body::RestoreSettings(v1::RestoreSettings {});
    match apply_request(&mut client, &mut view, restore).await {
        Body::Ack(_) => {}
        other => panic!("expected ack, got {other:?}"),
    }
    assert!(view.workspace.config_error.is_empty());
    assert_eq!(read_config(&dir).agents[0].prompt, "Edit the docs.");
    assert_eq!(std::fs::read_to_string(dir.join("home/config.broken.json")).expect("kept"), "{ \"projects\": [");

    // And hand edits keep applying.
    config.agents[0].name = "Copy Editor".into();
    std::fs::write(&config_path, config.to_json().expect("json")).expect("write");
    view.follow(&mut client, |view| view.agent("editor").name == "Copy Editor").await;
}

#[tokio::test]
async fn projects_need_only_a_name_and_agents_move_between_projects_and_the_workspace() {
    let dir = temp_dir();
    let (addr, _factory) = start(&dir).await;
    let (mut client, _, _) = Client::connect(addr).await;
    let mut view = View::default();

    apply_request(&mut client, &mut view, create_project("Web Site", Path::new(""))).await;
    let folder = dir.join("home/workspace/web-site");
    assert!(folder.is_dir(), "the folder is named after the project");
    assert_eq!(view.workspace.projects[0].path, folder.display().to_string());

    // An agent in the workspace itself, with a conversation.
    apply_request(&mut client, &mut view, create_agent("", "Builder", "")).await;
    assert_eq!(view.agent("builder").project_id, "");
    apply_request(&mut client, &mut view, send_message("builder", "hello")).await;
    view.follow(&mut client, |view| view.entries.len() == 2 && view.agent("builder").status() == AgentStatus::Idle).await;

    // Dragged into the project: the conversation comes along.
    let reply = apply_request(&mut client, &mut view, move_agent("builder", "web-site")).await;
    assert!(matches!(reply, Body::Ack(_)), "{reply:?}");
    assert_eq!(view.agent("builder").project_id, "web-site");
    let config = read_config(&dir);
    assert_eq!(config.agent("builder").expect("builder").project.as_deref(), Some("web-site"));
    let reply = apply_request(&mut client, &mut view, select("builder")).await;
    assert!(matches!(reply, Body::Ack(_)), "{reply:?}");
    assert_eq!(describe(&view.entries), ["user: hello", "assistant: echo: hello"]);

    // And back out.
    apply_request(&mut client, &mut view, move_agent("builder", "")).await;
    assert_eq!(view.agent("builder").project_id, "");
    assert_eq!(read_config(&dir).agent("builder").expect("builder").project, None);
    match apply_request(&mut client, &mut view, move_agent("builder", "nowhere")).await {
        Body::Error(error) => assert_eq!(error.code(), ErrorCode::NotFound),
        other => panic!("expected an error, got {other:?}"),
    }

    // Deleting the project deletes its agents and their conversations, never the folder's files.
    apply_request(&mut client, &mut view, move_agent("builder", "web-site")).await;
    std::fs::write(folder.join("notes.txt"), "keep me").expect("write");
    let delete = client_message::Body::DeleteProject(v1::DeleteProject { project_id: "web-site".into() });
    apply_request(&mut client, &mut view, delete).await;
    assert!(view.workspace.projects.is_empty());
    assert!(view.workspace.agents.iter().all(|agent| agent.id != "builder"));
    assert_eq!(view.workspace.selected_agent_id, "");
    assert!(!dir.join("home/agents/builder").exists());
    assert_eq!(std::fs::read_to_string(folder.join("notes.txt")).expect("kept"), "keep me");

    // The name is free again, and the new agent starts fresh.
    apply_request(&mut client, &mut view, create_agent("", "Builder", "")).await;
    assert_eq!(view.workspace.selected_agent_id, "builder");
    assert!(view.entries.is_empty());
}

fn move_agent(agent_id: &str, project_id: &str) -> client_message::Body {
    client_message::Body::UpdateAgent(v1::UpdateAgent {
        agent_id: agent_id.into(),
        project_id: Some(project_id.into()),
        ..Default::default()
    })
}

fn select(agent_id: &str) -> client_message::Body {
    client_message::Body::SelectAgent(v1::SelectAgent { agent_id: agent_id.into() })
}

fn read_config(dir: &Path) -> Config {
    Config::parse(&std::fs::read_to_string(dir.join("home/config.json")).expect("read")).expect("parse")
}

#[tokio::test]
async fn each_new_agent_gets_the_color_least_like_the_others() {
    let dir = temp_dir();
    let (addr, _factory) = start(&dir).await;
    let (mut client, _, _) = Client::connect(addr).await;
    let mut view = View::default();
    apply_request(&mut client, &mut view, create_project("Site", &dir.join("site"))).await;
    for name in ["Builder", "Writer", "Tester"] {
        apply_request(&mut client, &mut view, create_agent("site", name, "")).await;
    }
    let colors = |view: &View| view.workspace.agents.iter().map(|agent| (agent.id.clone(), agent.color.clone())).collect::<Vec<_>>();
    // Each new agent gets the color least like those taken: red is furthest
    // from blue, green from both, and so on.
    let expected = [("rivet", "blue"), ("builder", "red"), ("writer", "green"), ("tester", "slate")];
    assert_eq!(colors(&view), expected.map(|(id, color)| (id.to_owned(), color.to_owned())));
    assert_eq!(view.workspace.next_agent_color, "pink", "the app offers the next one");

    // Colors are kept, so removing an agent doesn't recolor the ones after it.
    let delete = client_message::Body::DeleteAgent(v1::DeleteAgent { agent_id: "builder".into() });
    apply_request(&mut client, &mut view, delete).await;
    let recolor = |color: &str| {
        client_message::Body::UpdateAgent(v1::UpdateAgent { agent_id: "writer".into(), color: Some(color.into()), ..Default::default() })
    };
    apply_request(&mut client, &mut view, recolor("amber")).await;
    let expected = [("rivet", "blue"), ("writer", "amber"), ("tester", "slate")];
    assert_eq!(colors(&view), expected.map(|(id, color)| (id.to_owned(), color.to_owned())));

    match apply_request(&mut client, &mut view, recolor("plaid")).await {
        Body::Error(error) => {
            assert_eq!(error.code(), ErrorCode::InvalidRequest);
            assert!(error.message.contains("must be one of blue, violet"), "{}", error.message);
        }
        other => panic!("expected an error, got {other:?}"),
    }

    let config = std::fs::read_to_string(dir.join("home/config.json")).expect("config");
    assert!(!config.contains("plaid"));
}

/// The agents' colors as config.json has them, once it has them all.
async fn saved_colors(dir: &Path, count: usize) -> Vec<(String, String)> {
    for _ in 0..100 {
        if let Ok(config) = Config::parse(&std::fs::read_to_string(dir.join("home/config.json")).expect("config")) {
            let colors: Vec<_> = config.agents.iter().map(|agent| (agent.id.clone(), agent.color.clone())).collect();
            if colors.len() == count && colors.iter().all(|(_, color)| !color.is_empty()) {
                return colors;
            }
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    panic!("config.json never got every agent's color");
}

#[tokio::test]
async fn colors_belong_to_agents_not_to_their_place() {
    let dir = temp_dir();
    // A config from before colors were saved, when an agent showed its place's color.
    std::fs::create_dir_all(dir.join("home")).expect("home");
    std::fs::create_dir_all(dir.join("site")).expect("site");
    let agent = |id: &str| serde_json::json!({"id": id, "name": id, "prompt": "", "model": MODEL, "effort": "off", "project": "site"});
    let config = serde_json::json!({
        "projects": [{"id": "site", "name": "Site", "path": dir.join("site")}],
        "agents": [{"id": "rivet", "name": "Rivet", "prompt": "", "model": MODEL, "effort": "off"}, agent("builder"), agent("writer"), agent("tester")],
    });
    std::fs::write(dir.join("home/config.json"), config.to_string()).expect("write");
    let (addr, _old) = start(&dir).await;
    let (mut client, workspace, _) = Client::connect(addr).await;
    let mut view = View { workspace, ..View::default() };
    let colors = |view: &View| view.workspace.agents.iter().map(|agent| (agent.id.clone(), agent.color.clone())).collect::<Vec<_>>();
    let pairs = |pairs: &[(&str, &str)]| pairs.iter().map(|(id, color)| (id.to_string(), color.to_string())).collect::<Vec<_>>();
    // Each keeps the color it showed, now saved.
    let before = pairs(&[("rivet", "blue"), ("builder", "violet"), ("writer", "green"), ("tester", "pink")]);
    assert_eq!(colors(&view), before);
    assert_eq!(saved_colors(&dir, 4).await, before);

    // Reordered, every agent keeps its color, after a restart too.
    let arrange = client_message::Body::ArrangeAgent(v1::ArrangeAgent {
        agent_id: "tester".into(),
        parent_id: String::new(),
        before_id: "builder".into(),
        project_id: None,
        folder_id: None,
    });
    apply_request(&mut client, &mut view, arrange).await;
    let reordered = pairs(&[("rivet", "blue"), ("tester", "pink"), ("builder", "violet"), ("writer", "green")]);
    assert_eq!(colors(&view), reordered);
    let (addr, _new) = start(&dir).await;
    let (mut client, workspace, _) = Client::connect(addr).await;
    let mut view = View { workspace, ..View::default() };
    assert_eq!(colors(&view), reordered);

    // Written into config.json without a color, an agent keeps the one it had,
    // and a new one gets the color least like the others; both are saved.
    let next = view.workspace.next_agent_color.clone();
    let mut config = Config::parse(&std::fs::read_to_string(dir.join("home/config.json")).expect("config")).expect("valid");
    config.agents[2].color.clear();
    let mut editor = config.agents[3].clone();
    (editor.id, editor.name, editor.color) = ("editor".into(), "Editor".into(), String::new());
    config.agents.insert(1, editor);
    std::fs::write(dir.join("home/config.json"), config.to_json().expect("json")).expect("write");
    view.follow(&mut client, |view| view.workspace.agents.len() == 5).await;
    let edited = pairs(&[("rivet", "blue"), ("editor", next.as_str()), ("tester", "pink"), ("builder", "violet"), ("writer", "green")]);
    assert_eq!(colors(&view), edited);
    assert_eq!(saved_colors(&dir, 5).await, edited);
}

#[tokio::test]
async fn agents_are_arranged_and_nested_in_the_sidebar_and_folded_for_every_device() {
    let dir = temp_dir();
    let (addr, _old) = start(&dir).await;
    let (mut client, workspace, _) = Client::connect(addr).await;
    let mut view = View { workspace, ..View::default() };
    apply_request(&mut client, &mut view, create_project("Web", &dir.join("web"))).await;
    apply_request(&mut client, &mut view, create_project("App", &dir.join("app"))).await;
    for (project, name) in [("web", "Backend"), ("web", "Frontend"), ("web", "Coordinator"), ("app", "Lead")] {
        apply_request(&mut client, &mut view, create_agent(project, name, "")).await;
    }
    let arrange = |agent: &str, parent: &str, before: &str, project: Option<&str>| {
        client_message::Body::ArrangeAgent(v1::ArrangeAgent {
            agent_id: agent.into(),
            parent_id: parent.into(),
            before_id: before.into(),
            project_id: project.map(str::to_owned),
            folder_id: None,
        })
    };
    let tree = |view: &View| -> Vec<String> {
        view.workspace
            .agents
            .iter()
            .map(|agent| {
                let parent = if agent.parent_id.is_empty() { String::new() } else { format!(">{}", agent.parent_id) };
                format!("{}@{}{parent}", agent.id, agent.project_id)
            })
            .collect()
    };

    // Coordinator to the top of its project, Backend and Frontend under it.
    assert!(matches!(apply_request(&mut client, &mut view, arrange("coordinator", "", "backend", None)).await, Body::Ack(_)));
    apply_request(&mut client, &mut view, arrange("backend", "coordinator", "", None)).await;
    apply_request(&mut client, &mut view, arrange("frontend", "coordinator", "backend", None)).await;
    assert_eq!(tree(&view), ["rivet@", "coordinator@web", "frontend@web>coordinator", "backend@web>coordinator", "lead@app"]);
    let config = Config::parse(&std::fs::read_to_string(dir.join("home/config.json")).expect("config")).expect("valid");
    assert_eq!(config.agent("backend").and_then(|agent| agent.parent.as_deref()), Some("coordinator"));

    // Nonsense is refused with a reason.
    match apply_request(&mut client, &mut view, arrange("coordinator", "frontend", "", None)).await {
        Body::Error(error) => {
            assert_eq!(error.code(), ErrorCode::InvalidRequest);
            assert_eq!(error.message, "Coordinator can't go under itself or one of its own agents");
        }
        other => panic!("expected an error, got {other:?}"),
    }

    // Folded, on every device and after a restart.
    let fold = client_message::Body::SetAgentCollapsed(v1::SetAgentCollapsed { agent_id: "coordinator".into(), collapsed: true });
    apply_request(&mut client, &mut view, fold).await;
    assert!(view.agent("coordinator").collapsed);
    let (addr, _new) = start(&dir).await;
    let (mut client, workspace, _) = Client::connect(addr).await;
    let mut view = View { workspace, ..View::default() };
    assert!(view.agent("coordinator").collapsed && !view.agent("frontend").collapsed);

    // Dropped onto Lead, Coordinator moves to App with everything under it.
    apply_request(&mut client, &mut view, arrange("coordinator", "lead", "", None)).await;
    assert_eq!(tree(&view), ["rivet@", "lead@app", "coordinator@app>lead", "frontend@app>coordinator", "backend@app>coordinator"]);
    // Moved in its settings, it takes them along too, at the top level.
    let to_web = v1::UpdateAgent { agent_id: "coordinator".into(), project_id: Some("web".into()), ..Default::default() };
    apply_request(&mut client, &mut view, client_message::Body::UpdateAgent(to_web)).await;
    assert_eq!(tree(&view), ["rivet@", "lead@app", "coordinator@web", "frontend@web>coordinator", "backend@web>coordinator"]);

    // Deleting a parent lifts its agents up a level.
    apply_request(&mut client, &mut view, arrange("backend", "frontend", "", None)).await;
    apply_request(&mut client, &mut view, client_message::Body::DeleteAgent(v1::DeleteAgent { agent_id: "frontend".into() })).await;
    assert_eq!(tree(&view), ["rivet@", "lead@app", "coordinator@web", "backend@web>coordinator"]);

    // A parent written by hand that can't hold is ignored, not an error.
    let path = dir.join("home/config.json");
    let mut written: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&path).expect("config")).expect("json");
    written["agents"][1]["parent"] = serde_json::json!("coordinator");
    written["agents"][1]["name"] = serde_json::json!("Team Lead");
    std::fs::write(&path, serde_json::to_string_pretty(&written).expect("json")).expect("write");
    view.follow(&mut client, |view| view.agent("lead").name == "Team Lead").await;
    assert_eq!(view.workspace.config_error, "");
    assert_eq!(view.agent("lead").parent_id, "", "Coordinator is in another project");
}

#[tokio::test]
async fn the_sidebar_and_panel_widths_are_kept_for_every_device() {
    let dir = temp_dir();
    let (addr, _factory) = start(&dir).await;
    let (mut client, workspace, _) = Client::connect(addr).await;
    let settings = |workspace: &v1::Workspace| {
        let settings = workspace.settings.clone().expect("settings");
        (settings.sidebar_width, settings.panel_width)
    };
    assert_eq!(settings(&workspace), (0.0, 0.0), "the app's defaults until dragged");
    let config = || std::fs::read_to_string(dir.join("home/config.json")).expect("config.json");
    assert!(!config().contains("\"display\""), "only written once changed");

    let widths = v1::UpdateSettings { sidebar_width: Some(312.5), panel_width: Some(420.0), ..Default::default() };
    let events = client.ok(client_message::Body::UpdateSettings(widths)).await;
    assert_eq!(settings(last_workspace(&events).expect("workspace")), (312.5, 420.0));
    let saved: serde_json::Value = serde_json::from_str(&config()).expect("json");
    assert_eq!(saved["display"], serde_json::json!({"sidebar_width": 312.5, "panel_width": 420.0}));

    // Another device sees the same; one width changes alone.
    let (mut other, workspace, _) = Client::connect(addr).await;
    assert_eq!(settings(&workspace), (312.5, 420.0));
    let events = other.ok(client_message::Body::UpdateSettings(v1::UpdateSettings { panel_width: Some(0.0), ..Default::default() })).await;
    assert_eq!(settings(last_workspace(&events).expect("workspace")), (312.5, 0.0));

    let refused =
        other.request(client_message::Body::UpdateSettings(v1::UpdateSettings { sidebar_width: Some(-1.0), ..Default::default() })).await;
    assert!(matches!(&refused.body, Body::Error(error) if error.message.contains("sidebar_width")), "{:?}", refused.body);
    assert!(Config::parse(r#"{"display": {"panel_width": -3}}"#).is_err());
}
