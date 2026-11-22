//! A project's folders: some of its agents work in one, changing files only
//! there and reading the rest of the project; the explorer shows each folder,
//! and removing one never deletes files.

use crate::common::{Client, Scripted, View, apply_request, create_agent, create_project, describe, send_message, start_with, temp_dir};
use factory_protocol::v1::{self, AgentStatus, ErrorCode, client_message, server_message::Body};

fn select(agent: &str) -> client_message::Body {
    client_message::Body::SelectAgent(v1::SelectAgent { agent_id: agent.into() })
}

fn create_folder(project: &str, name: &str, path: &str) -> client_message::Body {
    client_message::Body::CreateFolder(v1::CreateFolder { project_id: project.into(), name: name.into(), path: path.into() })
}

fn set_folder(agent: &str, folder: &str) -> client_message::Body {
    client_message::Body::UpdateAgent(v1::UpdateAgent { agent_id: agent.into(), folder_id: Some(folder.into()), ..Default::default() })
}

fn idle(view: &View, agent: &str) -> bool {
    let agent = view.agent(agent);
    agent.status() == AgentStatus::Idle && agent.queued_messages == 0
}

/// What `agent` says to `text`, entry by entry.
async fn talk(client: &mut Client, view: &mut View, agent: &str, text: &str) -> Vec<String> {
    apply_request(client, view, select(agent)).await;
    let before = view.entries.len();
    apply_request(client, view, send_message(agent, text)).await;
    view.follow(client, |view| view.entries.len() >= before + 2 && idle(view, agent)).await;
    describe(&view.entries[before..])
}

fn folders(view: &View, project: &str) -> Vec<v1::ProjectFolder> {
    view.workspace.projects.iter().find(|candidate| candidate.id == project).expect("project").folders.clone()
}

fn write(path: &std::path::Path, content: &str) -> String {
    format!(r#"call write_file {{"path":"{}","content":"{content}"}}"#, path.display())
}

#[tokio::test]
async fn agents_work_in_their_folder_and_read_the_whole_project() {
    let dir = temp_dir();
    let models = Scripted::signed_in();
    let system_prompt = models.system_prompt.clone();
    let (addr, _factory) = start_with(&dir, models).await;
    let (mut client, workspace, _) = Client::connect(addr).await;
    let mut view = View { workspace, ..View::default() };
    let site = dir.join("site");
    apply_request(&mut client, &mut view, create_project("Site", &site)).await;
    apply_request(&mut client, &mut view, create_agent("site", "Lead", "")).await;
    apply_request(&mut client, &mut view, create_agent("site", "Researcher", "")).await;
    talk(&mut client, &mut view, "lead", "hello").await;
    let lead_prompt = system_prompt.lock().expect("lock").clone();
    assert!(lead_prompt.contains(&format!("in its folder {}.", site.display())), "{lead_prompt}");

    // A folder anywhere, and one inside the project's folder, named after it.
    let research = dir.join("research");
    let reply = apply_request(&mut client, &mut view, create_folder("site", "Research", research.to_str().expect("path"))).await;
    assert!(matches!(reply, Body::Ack(_)), "{reply:?}");
    apply_request(&mut client, &mut view, create_folder("site", "Docs", "")).await;
    let docs = site.join("docs");
    assert!(research.is_dir() && docs.is_dir());
    let listed: Vec<(String, String, String)> =
        folders(&view, "site").into_iter().map(|folder| (folder.id, folder.name, folder.path)).collect();
    assert_eq!(
        listed,
        [
            ("research".to_owned(), "Research".to_owned(), research.display().to_string()),
            ("docs".to_owned(), "Docs".to_owned(), docs.display().to_string())
        ]
    );

    // Researcher works in Research.
    let reply = apply_request(&mut client, &mut view, set_folder("researcher", "research")).await;
    assert!(matches!(reply, Body::Ack(_)), "{reply:?}");
    assert_eq!(view.agent("researcher").folder_id, "research");
    std::fs::write(research.join("notes.md"), "# Notes").expect("write");
    let said = talk(&mut client, &mut view, "researcher", r#"call bash {"command":"pwd; cat notes.md"}"#).await;
    let canonical = research.canonicalize().expect("canonical");
    assert!(
        said[1].contains(&format!("{}\n# Notes", canonical.display())) || said[1].contains(&format!("{}\n# Notes", research.display())),
        "{said:?}"
    );
    let own = system_prompt.lock().expect("lock").clone();
    assert!(own.contains(&format!("working in {}. Your shell starts there; don't cd there first.", research.display())), "{own}");
    assert!(own.contains(&format!("You work in the project \"Site\", in its folder Research, {}.", research.display())), "{own}");
    let listing = format!(
        "The project's folders:\n- The project's own folder: {} (read only)\n- Docs: {} (read only)\n- Research: {} (yours)\n",
        site.display(),
        docs.display(),
        research.display()
    );
    assert!(own.contains(&listing), "{own}");

    // It reads the whole project, and changes files only in its folder.
    std::fs::write(site.join("plan.md"), "the plan").expect("write");
    let read = format!(r#"call read_file {{"path":"{}"}}"#, site.join("plan.md").display());
    assert!(talk(&mut client, &mut view, "researcher", &read).await[1].contains("the plan"));
    let said = talk(&mut client, &mut view, "researcher", &write(&site.join("plan.md"), "mine")).await;
    assert!(said[1].contains("read only"), "{said:?}");
    assert_eq!(std::fs::read_to_string(site.join("plan.md")).expect("read"), "the plan");
    talk(&mut client, &mut view, "researcher", r#"call write_file {"path":"found.md","content":"found"}"#).await;
    assert_eq!(std::fs::read_to_string(research.join("found.md")).expect("read"), "found");

    // Lead, in the project's own folder, hears of the folders without its
    // prompt changing, and can't change files in Docs, inside its folder.
    let said = talk(&mut client, &mut view, "lead", "and now?").await;
    assert_eq!(
        said[0],
        format!(
            "event from Claktory: New or changed project folders: The project's own folder: {} (yours); Docs: {} (read only); Research: {} (read only).",
            site.display(),
            docs.display(),
            research.display()
        )
    );
    assert_eq!(*system_prompt.lock().expect("lock"), lead_prompt);
    let said = talk(&mut client, &mut view, "lead", r#"call write_file {"path":"docs/a.md","content":"a"}"#).await;
    assert!(said[1].contains("read only"), "{said:?}");

    // The explorer lists each folder as a root of its own, which opens on its own.
    std::fs::create_dir_all(research.join("papers")).expect("dir");
    std::fs::write(research.join("papers/a.md"), "a").expect("write");
    apply_request(&mut client, &mut view, client_message::Body::FocusProject(v1::FocusProject { project_id: "site".into() })).await;
    let roots = |view: &View| view.explorer.as_ref().map(|explorer| explorer.folders.clone()).unwrap_or_default();
    view.follow(&mut client, |view| roots(view).len() == 2).await;
    let root = &roots(&view)[0];
    assert_eq!((root.folder_id.as_str(), root.path.as_str()), ("research", research.to_str().expect("path")));
    let names: Vec<&str> = root.entries.iter().map(|entry| entry.path.as_str()).collect();
    assert_eq!(names, ["papers", "found.md", "notes.md"]);
    let open = client_message::Body::ExpandFolder(v1::ExpandFolder {
        project_id: "site".into(),
        path: "papers".into(),
        expanded: true,
        folder_id: "research".into(),
        agent_id: String::new(),
    });
    assert!(matches!(apply_request(&mut client, &mut view, open).await, Body::Ack(_)));
    view.follow(&mut client, |view| roots(view).first().is_some_and(|root| root.entries.len() == 4)).await;
    assert!(view.explorer.as_ref().expect("explorer").entries.iter().all(|entry| !entry.expanded), "the project's tree is its own");

    // Renamed and folded away in the sidebar.
    let rename = v1::UpdateFolder {
        project_id: "site".into(),
        folder_id: "research".into(),
        name: Some("Papers".into()),
        path: None,
        before_id: None,
    };
    apply_request(&mut client, &mut view, client_message::Body::UpdateFolder(rename)).await;
    let fold = v1::SetFolderCollapsed { project_id: "site".into(), folder_id: "research".into(), collapsed: true };
    apply_request(&mut client, &mut view, client_message::Body::SetFolderCollapsed(fold)).await;
    let research_folder = folders(&view, "site").into_iter().find(|folder| folder.id == "research").expect("folder");
    assert_eq!((research_folder.name.as_str(), research_folder.collapsed), ("Papers", true));

    // Into Docs by dragging; a folder of another project isn't one.
    let drag = v1::ArrangeAgent { agent_id: "researcher".into(), folder_id: Some("docs".into()), ..Default::default() };
    assert!(matches!(apply_request(&mut client, &mut view, client_message::Body::ArrangeAgent(drag)).await, Body::Ack(_)));
    assert_eq!(view.agent("researcher").folder_id, "docs");
    let refused = apply_request(&mut client, &mut view, set_folder("lead", "nowhere")).await;
    assert!(matches!(&refused, Body::Error(error) if error.code() == ErrorCode::InvalidRequest), "{refused:?}");
    apply_request(&mut client, &mut view, set_folder("researcher", "research")).await;

    // Removing a folder puts its agents in the project's own folder and keeps the files.
    let remove = v1::DeleteFolder { project_id: "site".into(), folder_id: "research".into() };
    assert!(matches!(apply_request(&mut client, &mut view, client_message::Body::DeleteFolder(remove)).await, Body::Ack(_)));
    assert_eq!(view.agent("researcher").folder_id, "");
    assert_eq!(folders(&view, "site").len(), 1);
    assert!(research.join("notes.md").exists());
    view.follow(&mut client, |view| roots(view).len() == 1).await;
    let said = talk(&mut client, &mut view, "researcher", r#"call bash {"command":"pwd"}"#).await;
    // It's told first, and its shell starts there at once.
    assert!(
        said[0].starts_with(&format!(
            "event from Claktory: Your settings changed: your shell and file tools now start in {}.",
            site.display()
        )),
        "{said:?}"
    );
    let canonical = site.canonicalize().expect("canonical");
    let pwd = said.iter().find(|line| line.starts_with("tool bash")).expect("bash");
    assert!(pwd.contains(&canonical.display().to_string()) || pwd.contains(&site.display().to_string()), "{said:?}");

    // Moving an agent to another project leaves its folder behind.
    apply_request(&mut client, &mut view, set_folder("researcher", "docs")).await;
    let to_workspace = v1::UpdateAgent { agent_id: "researcher".into(), project_id: Some(String::new()), ..Default::default() };
    apply_request(&mut client, &mut view, client_message::Body::UpdateAgent(to_workspace)).await;
    assert_eq!(view.agent("researcher").folder_id, "");

    // A folder's path is absolute.
    let refused = apply_request(&mut client, &mut view, create_folder("site", "Notes", "notes")).await;
    assert!(matches!(&refused, Body::Error(error) if error.message.contains("absolute")), "{refused:?}");
}

#[tokio::test]
async fn a_folder_the_project_lacks_is_ignored_and_bad_folders_are_refused() {
    let dir = temp_dir();
    let text = format!(
        r#"{{"projects":[{{"id":"site","name":"Site","path":"{site}","folders":[{{"id":"app","name":"App","path":"{app}"}}]}}],
            "agents":[{{"id":"lead","name":"Lead","model":"a/b","effort":"high","project":"site","folder":"gone"}},
                      {{"id":"dev","name":"Dev","model":"a/b","effort":"high","project":"site","folder":"app"}}]}}"#,
        site = dir.join("site").display(),
        app = dir.join("app").display()
    );
    let config = factoryd::config::Config::parse(&text, &dir.join("home")).expect("valid");
    assert!(config.agent_folder(config.agent("lead").expect("lead")).is_none());
    assert_eq!(config.agent_folder(config.agent("dev").expect("dev")).map(|folder| folder.name.as_str()), Some("App"));
    for bad in [
        r#"{"id":"Bad Id","name":"App","path":"/app"}"#,
        r#"{"id":"app","name":" ","path":"/app"}"#,
        r#"{"id":"app","name":"App","path":"/app"},{"id":"app","name":"Again","path":"/again"}"#,
    ] {
        let text = format!(r#"{{"projects":[{{"id":"site","name":"Site","path":"/site","folders":[{bad}]}}]}}"#);
        assert!(factoryd::config::Config::parse(&text, std::path::Path::new("/claktory")).is_err(), "{text}");
    }

    // A relative folder path is Claktory's own territory: under the home
    // the file was read from (and written back relative; see config.rs).
    let text = r#"{"projects":[{"id":"site","name":"Site","path":"workspace/site","folders":[{"id":"notes","name":"Notes","path":"workspace/site/notes"}]}]}"#;
    let config = factoryd::config::Config::parse(text, std::path::Path::new("/claktory")).expect("territory paths resolve");
    assert_eq!(config.projects[0].path, std::path::Path::new("/claktory/workspace/site"));
    assert_eq!(config.projects[0].folders[0].path, std::path::Path::new("/claktory/workspace/site/notes"));
}

#[tokio::test]
async fn the_explorer_never_shows_a_directory_twice() {
    let dir = temp_dir();
    let (addr, _factory) = start_with(&dir, Scripted::signed_in()).await;
    let (mut client, workspace, _) = Client::connect(addr).await;
    let mut view = View { workspace, ..View::default() };
    let repo = dir.join("repo");
    let research = dir.join("research");
    for folder in [repo.join("docs"), research.clone()] {
        std::fs::create_dir_all(&folder).expect("dir");
    }
    std::fs::write(repo.join("main.rs"), "fn main() {}").expect("write");
    std::fs::write(repo.join("docs/guide.md"), "guide").expect("write");
    std::fs::write(research.join("found.md"), "found").expect("write");
    apply_request(&mut client, &mut view, create_project("Factory", &repo)).await;
    // App is the project's own folder; Docs is inside it, twice over; Research is elsewhere.
    for (name, path) in [("App", &repo), ("Docs", &repo.join("docs")), ("Docs again", &repo.join("docs")), ("Research", &research)] {
        apply_request(&mut client, &mut view, create_folder("factory", name, path.to_str().expect("path"))).await;
    }
    assert_eq!(folders(&view, "factory").len(), 4);
    apply_request(&mut client, &mut view, client_message::Body::FocusProject(v1::FocusProject { project_id: "factory".into() })).await;
    let roots = |view: &View| view.explorer.as_ref().map(|explorer| explorer.folders.clone()).unwrap_or_default();
    view.follow(&mut client, |view| !roots(view).is_empty()).await;

    let explorer = view.explorer.clone().expect("explorer");
    assert!(explorer.entries.is_empty() && explorer.error.is_empty(), "the project's own root is App's: {:?}", explorer.entries);
    let shown: Vec<(&str, Vec<&str>)> = explorer
        .folders
        .iter()
        .map(|root| {
            (root.folder_id.as_str(), root.entries.iter().map(|entry| entry.path.as_str()).filter(|path| !path.starts_with('.')).collect())
        })
        .collect();
    assert_eq!(shown, [("app", vec!["main.rs"]), ("docs", vec!["guide.md"]), ("research", vec!["found.md"])]);
}

/// #248: a project's folders come in the order the person drags them to,
/// kept in config.json, the same on every device and after a restart.
#[tokio::test]
async fn folders_are_dragged_into_the_order_the_person_wants() {
    let dir = temp_dir();
    let (addr, _factory) = start_with(&dir, Scripted::signed_in()).await;
    let (mut client, workspace, _) = Client::connect(addr).await;
    let mut view = View { workspace, ..View::default() };
    apply_request(&mut client, &mut view, create_project("Claktory", &dir.join("claktory"))).await;
    for name in ["App", "Research", "Ops"] {
        apply_request(&mut client, &mut view, create_folder("claktory", name, "")).await;
    }
    let order = |view: &View| folders(view, "claktory").into_iter().map(|folder| folder.id).collect::<Vec<_>>();
    assert_eq!(order(&view), ["app", "research", "ops"], "new ones go last");
    let drag = |folder: &str, before: &str| {
        client_message::Body::UpdateFolder(v1::UpdateFolder {
            project_id: "claktory".into(),
            folder_id: folder.into(),
            before_id: Some(before.into()),
            ..Default::default()
        })
    };

    // Ops to the top, so its pipe is at hand; then App last.
    assert!(matches!(apply_request(&mut client, &mut view, drag("ops", "app")).await, Body::Ack(_)));
    assert_eq!(order(&view), ["ops", "app", "research"]);
    apply_request(&mut client, &mut view, drag("app", "")).await;
    assert_eq!(order(&view), ["ops", "research", "app"]);
    // Before itself changes nothing; a name and a place in one request both hold.
    apply_request(&mut client, &mut view, drag("research", "research")).await;
    assert_eq!(order(&view), ["ops", "research", "app"]);
    let both = v1::UpdateFolder {
        project_id: "claktory".into(),
        folder_id: "app".into(),
        name: Some("Code".into()),
        path: None,
        before_id: Some("ops".into()),
    };
    apply_request(&mut client, &mut view, client_message::Body::UpdateFolder(both)).await;
    assert_eq!(order(&view), ["app", "ops", "research"]);
    assert_eq!(folders(&view, "claktory")[0].name, "Code");

    // A drag from a list that changed meanwhile is refused, and nothing moves.
    for (folder, before, said) in
        [("gone", "", "That folder no longer exists."), ("ops", "gone", "The list changed while you were dragging. Try again.")]
    {
        match apply_request(&mut client, &mut view, drag(folder, before)).await {
            Body::Error(error) => assert_eq!((error.code(), error.message.as_str()), (ErrorCode::InvalidRequest, said)),
            other => panic!("expected an error, got {other:?}"),
        }
    }
    assert_eq!(order(&view), ["app", "ops", "research"]);

    // Kept in config.json's order, and so after a restart.
    let config: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(dir.join("home/config.json")).expect("config")).expect("json");
    let written: Vec<&str> =
        config["projects"][0]["folders"].as_array().expect("folders").iter().filter_map(|folder| folder["id"].as_str()).collect();
    assert_eq!(written, ["app", "ops", "research"]);
    let (addr, _again) = start_with(&dir, Scripted::signed_in()).await;
    let (_client, workspace, _) = Client::connect(addr).await;
    assert_eq!(order(&View { workspace, ..View::default() }), ["app", "ops", "research"]);
}
