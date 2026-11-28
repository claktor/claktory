use std::path::Path;

use crate::common::{
    Client, MODEL, View, apply_request, create_agent, create_project, describe, last_workspace, send_message, start, temp_dir,
};
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
    let config = Config::parse(&text, &dir.join("home")).expect("valid config");
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
        crate::common::json_text(dir.join("docs").display())
    );
    std::fs::write(&config_path, &edited).expect("write");
    let mut view = View::default();
    view.follow(&mut client, |view| view.workspace.agents.len() == 1 && view.workspace.agents[0].id == "editor").await;
    assert_eq!(view.agent("editor").prompt, "Edit the docs.");
    let mut config = Config::parse(&edited, &dir.join("home")).expect("parse");

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
    std::fs::write(&config_path, config.to_json(&dir.join("home")).expect("json")).expect("write");
    view.follow(&mut client, |view| view.agent("editor").name == "Copy Editor").await;
}

#[tokio::test]
async fn projects_need_only_a_name_and_agents_move_between_projects_and_the_workspace() {
    let dir = temp_dir();
    let (addr, _factory) = start(&dir).await;
    let (mut client, _, _) = Client::connect(addr).await;
    let mut view = View::default();

    apply_request(&mut client, &mut view, create_project("Web Site", Path::new(""))).await;
    let folder = dir.join("home").join("workspace").join("web-site");
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
    Config::parse(&std::fs::read_to_string(dir.join("home/config.json")).expect("read"), &dir.join("home")).expect("parse")
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
        if let Ok(config) = Config::parse(&std::fs::read_to_string(dir.join("home/config.json")).expect("config"), &dir.join("home")) {
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
        ..Default::default()
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
    let mut config =
        Config::parse(&std::fs::read_to_string(dir.join("home/config.json")).expect("config"), &dir.join("home")).expect("valid");
    config.agents[2].color.clear();
    let mut editor = config.agents[3].clone();
    (editor.id, editor.name, editor.color) = ("editor".into(), "Editor".into(), String::new());
    config.agents.insert(1, editor);
    std::fs::write(dir.join("home/config.json"), config.to_json(&dir.join("home")).expect("json")).expect("write");
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
            ..Default::default()
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
    let config = Config::parse(&std::fs::read_to_string(dir.join("home/config.json")).expect("config"), &dir.join("home")).expect("valid");
    // Nested is led: the nesting is the lead, kept as `lead`.
    assert_eq!(config.agent("backend").and_then(|agent| agent.lead.as_deref()), Some("coordinator"));
    assert_eq!(view.agent("backend").lead_id, "coordinator");

    // Nonsense is refused with a reason.
    match apply_request(&mut client, &mut view, arrange("coordinator", "frontend", "", None)).await {
        Body::Error(error) => {
            assert_eq!(error.code(), ErrorCode::InvalidRequest);
            assert_eq!(error.message, "Frontend reports to Coordinator, so Coordinator can't report to Frontend.");
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
    assert!(Config::parse(r#"{"display": {"panel_width": -3}}"#, std::path::Path::new("/claktory")).is_err());
}

#[tokio::test]
async fn each_device_keeps_its_own_zoom() {
    let dir = temp_dir();
    let (addr, _factory) = start(&dir).await;
    let (mut client, workspace, _) = Client::connect(addr).await;
    let zoom = |workspace: &v1::Workspace| workspace.settings.clone().expect("settings").zoom;
    assert!(zoom(&workspace).is_empty(), "actual size everywhere");
    let config = || std::fs::read_to_string(dir.join("home/config.json")).expect("config.json");
    let set = |zoom: f64| client_message::Body::SetZoom(v1::SetZoom { zoom });

    // The loopback port is this machine's own app.
    let events = client.ok(set(1.25)).await;
    assert_eq!(zoom(last_workspace(&events).expect("workspace")), [("local".to_owned(), 1.25)].into());
    let saved: serde_json::Value = serde_json::from_str(&config()).expect("json");
    assert_eq!(saved["display"], serde_json::json!({"zoom": {"local": 1.25}}));
    let (_, workspace, _) = Client::connect(addr).await;
    assert_eq!(zoom(&workspace)["local"], 1.25, "restored when it connects");

    for wrong in [0.4, 2.5, f64::NAN, f64::INFINITY, -1.0] {
        let refused = client.request(set(wrong)).await;
        assert!(
            matches!(&refused.body, Body::Error(error) if error.code() == v1::ErrorCode::InvalidRequest),
            "{wrong}: {:?}",
            refused.body
        );
    }
    // Actual size isn't written.
    client.ok(set(0.5)).await;
    let events = client.ok(set(1.0)).await;
    assert!(zoom(last_workspace(&events).expect("workspace")).is_empty());
    assert!(!config().contains("\"display\""), "{}", config());
    client.ok(set(0.0)).await;

    assert!(Config::parse(r#"{"display": {"zoom": {"phone": 1.5}}}"#, std::path::Path::new("/claktory")).is_ok());
    assert!(Config::parse(r#"{"display": {"zoom": {"phone": 3}}}"#, std::path::Path::new("/claktory")).is_err());
}

/// How much of the files each device keeps, its own, as zoom is.
#[tokio::test]
async fn each_device_keeps_its_own_file_cache() {
    let dir = temp_dir();
    let (addr, _factory) = start(&dir).await;
    let (mut client, workspace, _) = Client::connect(addr).await;
    let caches = |workspace: &v1::Workspace| workspace.settings.clone().expect("settings").file_cache;
    assert!(caches(&workspace).is_empty(), "1 GB everywhere");
    let config = || std::fs::read_to_string(dir.join("home/config.json")).expect("config.json");
    let set = |bytes: u64| client_message::Body::SetFileCache(v1::SetFileCache { bytes });

    let events = client.ok(set(20_000_000_000)).await;
    assert_eq!(caches(last_workspace(&events).expect("workspace")), [("local".to_owned(), 20_000_000_000)].into());
    let saved: serde_json::Value = serde_json::from_str(&config()).expect("json");
    assert_eq!(saved["display"], serde_json::json!({"file_cache": {"local": 20_000_000_000_u64}}));
    let (_, workspace, _) = Client::connect(addr).await;
    assert_eq!(caches(&workspace)["local"], 20_000_000_000, "restored when it connects");

    for wrong in [1, 99_999_999, 100_000_000_001] {
        let refused = client.request(set(wrong)).await;
        assert!(
            matches!(&refused.body, Body::Error(error) if error.code() == v1::ErrorCode::InvalidRequest),
            "{wrong}: {:?}",
            refused.body
        );
    }
    for edge in [100_000_000, 100_000_000_000] {
        let events = client.ok(set(edge)).await;
        assert_eq!(caches(last_workspace(&events).expect("workspace"))["local"], edge);
    }
    // The default isn't written, whether set as 0 or as 1 GB.
    let events = client.ok(set(0)).await;
    assert!(caches(last_workspace(&events).expect("workspace")).is_empty());
    assert!(!config().contains("\"display\""), "{}", config());
    client.ok(set(300_000_000)).await;
    client.ok(set(1_000_000_000)).await;
    assert!(!config().contains("\"display\""), "{}", config());
    client.ok(set(0)).await;

    let parse = |text: &str| Config::parse(text, std::path::Path::new("/claktory"));
    assert!(parse(r#"{"display": {"file_cache": {"phone": 5000000000}}}"#).is_ok());
    for wrong in [r#"{"display": {"file_cache": {"phone": 5}}}"#, r#"{"display": {"file_cache": {"phone": -1}}}"#, r#"{"display": {"file_cache": {"phone": 1.5e9}}}"#] {
        assert!(parse(wrong).is_err(), "{wrong}");
    }
}

#[tokio::test]
async fn the_vaults_unlock_window_is_a_setting() {
    let dir = temp_dir();
    let (addr, _factory) = start(&dir).await;
    let (mut client, workspace, _) = Client::connect(addr).await;
    assert_eq!(workspace.settings.expect("settings").vault_unlock_minutes, 5, "5 minutes unless changed");
    let config = || std::fs::read_to_string(dir.join("home/config.json")).expect("config.json");
    assert!(!config().contains("\"vault\""), "only written once changed");

    let events =
        client.ok(client_message::Body::UpdateSettings(v1::UpdateSettings { vault_unlock_minutes: Some(15), ..Default::default() })).await;
    assert_eq!(last_workspace(&events).and_then(|workspace| workspace.settings.clone()).expect("settings").vault_unlock_minutes, 15);
    let saved: serde_json::Value = serde_json::from_str(&config()).expect("json");
    assert_eq!(saved["vault"], serde_json::json!({"unlock_minutes": 15}));
    for minutes in [0, 61] {
        let refused = client
            .request(client_message::Body::UpdateSettings(v1::UpdateSettings { vault_unlock_minutes: Some(minutes), ..Default::default() }))
            .await;
        assert!(matches!(&refused.body, Body::Error(error) if error.message.contains("unlock_minutes")), "{:?}", refused.body);
    }
}

/// The sidebar's Projects and Chats sections fold away the same on
/// every device and after a restart, without touching config.json.
#[tokio::test]
async fn the_sidebar_sections_fold_on_every_device() {
    let dir = temp_dir();
    let (addr, _factory) = start(&dir).await;
    let (mut client, workspace, _) = Client::connect(addr).await;
    let mut view = View { workspace, ..View::default() };
    assert!(!view.workspace.projects_collapsed && !view.workspace.chats_collapsed, "open by default");
    let settings = dir.join("home/config.json");
    let before = std::fs::read_to_string(&settings).expect("config");
    let fold = |projects: Option<bool>, chats: Option<bool>| {
        client_message::Body::UpdateSettings(v1::UpdateSettings {
            projects_collapsed: projects,
            chats_collapsed: chats,
            ..Default::default()
        })
    };

    assert!(matches!(apply_request(&mut client, &mut view, fold(Some(true), None)).await, Body::Ack(_)));
    assert!(view.workspace.projects_collapsed && !view.workspace.chats_collapsed);
    assert_eq!(std::fs::read_to_string(&settings).expect("config"), before, "a fold isn't a setting");
    let (mut other, workspace, _) = Client::connect(addr).await;
    assert!(workspace.projects_collapsed, "another device sees it folded");

    // With a setting in the same request, both hold.
    let both = v1::UpdateSettings { chats_collapsed: Some(true), user_name: Some("Matt".into()), ..Default::default() };
    let mut other_view = View { workspace, ..View::default() };
    apply_request(&mut other, &mut other_view, client_message::Body::UpdateSettings(both)).await;
    assert!(other_view.workspace.chats_collapsed && other_view.workspace.projects_collapsed);
    assert_eq!(other_view.workspace.settings.as_ref().map(|settings| settings.user_name.as_str()), Some("Matt"));

    // Kept after a restart; opened again.
    let (addr, _again) = start(&dir).await;
    let (mut client, workspace, _) = Client::connect(addr).await;
    assert!(workspace.projects_collapsed && workspace.chats_collapsed);
    let mut view = View { workspace, ..View::default() };
    apply_request(&mut client, &mut view, fold(Some(false), Some(false))).await;
    assert!(!view.workspace.projects_collapsed && !view.workspace.chats_collapsed);
}

#[tokio::test]
async fn a_file_with_relative_paths_keeps_that_form_and_old_absolute_settings_stay_absolute() {
    let dir = temp_dir();
    std::fs::create_dir_all(dir.join("home/workspace/docs")).expect("home");
    // A file in the new form, as a move writes it: territory paths say
    // where they are without naming the machine.
    std::fs::write(dir.join("home/config.json"), r#"{"projects":[{"id":"docs","name":"Docs","path":"workspace/docs"}]}"#).expect("write");
    let (addr, _factory) = start(&dir).await;
    let (mut client, workspace, _) = Client::connect(addr).await;
    let mut view = View { workspace, ..View::default() };
    assert_eq!(read_config(&dir).project("docs").expect("docs").path, dir.join("home/workspace/docs"));

    // Its new projects follow: one in the workspace relative, an external
    // one absolute. All read back absolute.
    apply_request(&mut client, &mut view, create_project("Site", &dir.join("home/workspace/site"))).await;
    apply_request(&mut client, &mut view, create_project("Code", &dir.join("repo"))).await;
    let text = std::fs::read_to_string(dir.join("home/config.json")).expect("config.json");
    assert!(text.contains(r#""path": "workspace/docs""#), "{text}");
    assert!(text.contains(r#""path": "workspace/site""#), "{text}");
    assert!(text.contains(&format!(r#""path": "{}""#, crate::common::json_text(dir.join("repo").display()))), "{text}");
    let config = read_config(&dir);
    assert_eq!(config.project("site").expect("site").path, dir.join("home/workspace/site"));
    assert_eq!(config.project("code").expect("code").path, dir.join("repo"));

    // A settings file from before relative forms (absolute paths for
    // everything, this machine's included) reads as it always did, and
    // stays so: the supervisor may go back to a backend from before, which
    // reads only that, after any write.
    let old = format!(r#"{{"projects":[{{"id":"docs","name":"Docs","path":"{}"}}]}}"#, crate::common::json_text(dir.join("home/workspace/docs").display()));
    std::fs::write(dir.join("home/config.json"), &old).expect("write");
    view.follow(&mut client, |view| view.workspace.projects.len() == 1 && view.workspace.projects[0].id == "docs").await;
    assert_eq!(read_config(&dir).project("docs").expect("docs").path, dir.join("home/workspace/docs"));
    apply_request(&mut client, &mut view, create_project("More", &dir.join("home/workspace/more"))).await;
    let text = std::fs::read_to_string(dir.join("home/config.json")).expect("config.json");
    assert!(text.contains(&format!(r#""path": "{}""#, crate::common::json_text(dir.join("home/workspace/docs").display()))), "{text}");
    assert!(text.contains(&format!(r#""path": "{}""#, crate::common::json_text(dir.join("home/workspace/more").display()))), "{text}");
}

#[tokio::test]
async fn claktorys_own_writes_after_an_update_keep_old_settings_readable_by_the_version_before() {
    // An updated Claktory with a pipe from an older version. Its first start
    // fills in colors and renames the pipe's "Liaison", two settings
    // writes; the person changes a setting too. If any of them flipped
    // paths to the relative form, going back to the version before (the
    // supervisor's rollback, during the update's trial or after) would
    // leave it a file it can't read.
    let dir = temp_dir();
    std::fs::create_dir_all(dir.join("home/workspace/site")).expect("home");
    std::fs::create_dir_all(dir.join("home/bin")).expect("bin");
    let site = dir.join("home/workspace/site");
    let old = format!(
        r#"{{"projects":[{{"id":"site","name":"Site","path":"{}","folders":[{{"id":"notes","name":"Notes","path":"{}"}}]}}],
            "agents":[{{"id":"ana","name":"Ana","model":"{MODEL}","effort":"off","prompt":"","project":"site"}},
                      {{"id":"liaison","name":"Liaison","color":"green","model":"{MODEL}","effort":"off",
                        "prompt":"You are the liaison for the pipe \"Launch\": you read what comes through it.","project":"site"}}]}}"#,
        // As JSON strings: a Windows path's backslashes escaped.
        crate::common::json_text(site.display()),
        crate::common::json_text(site.join("notes").display())
    );
    std::fs::write(dir.join("home/config.json"), &old).expect("write");
    let trial = r#"{"from":"v0.1.15","to":"v0.1.16","started":true}"#;
    std::fs::write(dir.join("home/bin/trial.json"), trial).expect("trial");
    let (addr, factory) = start(&dir).await;
    let pipe = factoryd::pipes::open_for_test(&factory, "site", "Launch", &["ana", "liaison"], "ana", false);
    factoryd::pipes::unbound_for_test(&factory, &pipe);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    let text = loop {
        let text = std::fs::read_to_string(dir.join("home/config.json")).expect("config.json");
        if !text.contains(r#""name": "Liaison""#) && text.contains(r#""id": "liaison""#) {
            break text;
        }
        assert!(std::time::Instant::now() < deadline, "the liaison was never renamed: {text}");
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    };
    assert!(text.contains(r#""color""#), "Ana's color was filled in: {text}");
    let absolute = |text: &str| {
        text.contains(&format!(r#""path": "{}""#, crate::common::json_text(site.display())))
            && text.contains(&format!(r#""path": "{}""#, crate::common::json_text(site.join("notes").display())))
    };
    assert!(absolute(&text), "the paths kept their absolute form: {text}");

    // The trial passed; the person's own changes keep it readable too.
    std::fs::remove_file(dir.join("home/bin/trial.json")).expect("pass");
    let (mut client, workspace, _) = Client::connect(addr).await;
    let mut view = View { workspace, ..View::default() };
    apply_request(&mut client, &mut view, create_project("More", &dir.join("home/workspace/more"))).await;
    let text = std::fs::read_to_string(dir.join("home/config.json")).expect("config.json");
    assert!(absolute(&text), "{text}");
    assert!(text.contains(&format!(r#""path": "{}""#, crate::common::json_text(dir.join("home/workspace/more").display()))), "{text}");
}

#[tokio::test]
async fn a_newer_claktorys_settings_still_read_and_survive_changes() {
    // After the supervisor goes back from an update, the file may hold
    // settings only the newer version knows: they're kept, not refused.
    let dir = temp_dir();
    std::fs::create_dir_all(dir.join("home/workspace/site")).expect("home");
    let config = format!(
        r#"{{"projects":[{{"id":"site","name":"Site","path":"{}","pinned":true}}],
            "display":{{"max_open_panes":3}},"notifications":{{"agents":{{"ed":"always"}}}},
            "agents":[{{"id":"ed","name":"Ed","model":"{MODEL}","effort":"off","prompt":"","project":"site","notify":"always"}}]}}"#,
        crate::common::json_text(dir.join("home/workspace/site").display())
    );
    std::fs::write(dir.join("home/config.json"), &config).expect("write");
    let (addr, _factory) = start(&dir).await;
    let (mut client, workspace, _) = Client::connect(addr).await;
    assert!(workspace.config_error.is_empty(), "{}", workspace.config_error);
    assert_eq!(workspace.agents.iter().find(|agent| agent.id == "ed").expect("ed").project_id, "site");
    let mut view = View { workspace, ..View::default() };
    apply_request(&mut client, &mut view, create_project("More", &dir.join("repo"))).await;
    let written: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(dir.join("home/config.json")).expect("config")).expect("json");
    assert_eq!(written["display"]["max_open_panes"], 3, "{written}");
    assert_eq!(written["notifications"]["agents"]["ed"], "always");
    assert_eq!(written["projects"][0]["pinned"], true);
    assert_eq!(written["agents"][0]["notify"], "always");
    assert_eq!(written["projects"][1]["id"], "more");
}
