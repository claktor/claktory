//! Extensions: capabilities agents have on top of their coding tools, on or
//! off per agent. Agents (on by default) lets agents message each other within
//! their project, plus whom they're granted; Checklist (off by default) keeps
//! what the person asked for, with a panel the app shows.

use crate::common::{
    Client, MODEL, OTHER_MODEL, Scripted, View, apply_request, create_agent, create_project, describe, send_message, start, start_with,
    temp_dir,
};
use factory_protocol::v1::{self, AgentStatus, client_message, entry::Kind, server_message::Body};

fn select(agent: &str) -> client_message::Body {
    client_message::Body::SelectAgent(v1::SelectAgent { agent_id: agent.into() })
}

fn idle(view: &View, agent: &str) -> bool {
    let agent = view.agent(agent);
    agent.status() == AgentStatus::Idle && agent.queued_messages == 0
}

fn change(agent: &str, change: v1::ExtensionChange) -> client_message::Body {
    client_message::Body::UpdateAgent(v1::UpdateAgent { agent_id: agent.into(), extension: Some(change), ..Default::default() })
}

fn turn(agent: &str, extension: &str, on: bool) -> client_message::Body {
    change(agent, v1::ExtensionChange { id: extension.into(), enabled: Some(on), ..Default::default() })
}

fn set_switch(agent: &str, option: &str, on: bool) -> client_message::Body {
    let value = v1::OptionValue { on, agent_ids: Vec::new() };
    change(agent, v1::ExtensionChange { id: "agents".into(), option_id: option.into(), value: Some(value), ..Default::default() })
}

fn grant(agent: &str, others: &[&str]) -> client_message::Body {
    let value = v1::OptionValue { on: false, agent_ids: others.iter().map(|other| (*other).to_owned()).collect() };
    change(agent, v1::ExtensionChange { id: "agents".into(), option_id: "also".into(), value: Some(value), ..Default::default() })
}

fn ack(reply: Body) {
    assert!(matches!(reply, Body::Ack(_)), "expected ack, got {reply:?}");
}

/// Has `agent` call `tool` with `args` (the scripted model does what it's
/// told) and returns what the tool answered.
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

/// Starts the agent on a new conversation (/clear), so its next context
/// has its settings as they are now.
async fn fresh(client: &mut Client, view: &mut View, agent: &str) {
    let clear = v1::RunCommand { agent_id: agent.into(), name: "clear".into(), args: String::new() };
    ack(apply_request(client, view, client_message::Body::RunCommand(clear)).await);
}

fn config(dir: &std::path::Path) -> serde_json::Value {
    serde_json::from_str(&std::fs::read_to_string(dir.join("home/config.json")).expect("config.json")).expect("json")
}

#[tokio::test]
async fn agents_see_and_message_their_own_project_plus_whom_they_are_granted() {
    let dir = temp_dir();
    let models = Scripted::signed_in();
    let system_prompt = models.system_prompt.clone();
    let (addr, _factory) = start_with(&dir, models).await;
    let (mut client, workspace, _) = Client::connect(addr).await;
    let mut view = View { workspace, ..View::default() };
    apply_request(&mut client, &mut view, create_project("Site", &dir.join("site"))).await;
    apply_request(&mut client, &mut view, create_project("Lab", &dir.join("lab"))).await;
    for (project, name) in [("site", "Builder"), ("site", "Writer"), ("lab", "Scout"), ("", "Helper")] {
        apply_request(&mut client, &mut view, create_agent(project, name, "")).await;
    }
    let rivet = v1::UpdateAgent { agent_id: "rivet".into(), model: Some(MODEL.into()), effort: Some("off".into()), ..Default::default() };
    apply_request(&mut client, &mut view, client_message::Body::UpdateAgent(rivet)).await;

    // Every agent has every extension, with its effective values.
    let ids: Vec<&str> = view.workspace.extensions.iter().map(|extension| extension.id.as_str()).collect();
    assert_eq!(
        ids,
        [
            "agents",
            "checklist",
            "database",
            "database:factory",
            "highlight",
            "memory",
            "memory:factory",
            "monitors",
            "resources",
            "secrets",
            "tasks",
            "tree",
            "viewer",
            "web"
        ]
    );
    let manual = &view.workspace.extensions[6];
    assert_eq!((manual.name.as_str(), manual.base.as_str(), manual.admin_default), ("Claktory manual", "memory", true));
    let records = &view.workspace.extensions[3];
    assert_eq!((records.name.as_str(), records.base.as_str(), records.admin_default), ("Claktory's records", "database", true));
    let builder = view.agent("builder").extensions.clone();
    assert_eq!((builder[0].id.as_str(), builder[0].enabled, builder[1].enabled), ("agents", true, false));
    assert!(builder[0].options["reachable"].on && !builder[0].options["admin"].on && builder[0].options["also"].agent_ids.is_empty());
    let rivet = &view.agent("rivet").extensions[0];
    assert!(rivet.options["admin"].on && !rivet.options["reachable"].on, "Rivet is the admin agent, and no one messages it");
    assert!(view.agent("rivet").extensions[4].enabled && !builder[4].enabled, "a new Factory's Rivet points at things");
    assert!(
        !builder[2].enabled
            && !view.agent("rivet").extensions[2].enabled
            && !builder[3].enabled
            && view.agent("rivet").extensions[3].enabled,
        "a database of its own only when turned on; admins Factory's records"
    );

    // Builder sees only its project, and is told whom it can message instead.
    assert_eq!(call(&mut client, &mut view, "builder", "agents_status", "{}").await, "writer Writer: idle");
    let prompt = system_prompt.lock().expect("lock").clone();
    assert!(prompt.contains("## Other agents") && prompt.contains("You can message: Writer (writer)."), "{prompt}");
    let refused = call(&mut client, &mut view, "builder", "agents_send", r#"{"to":"scout","text":"hi"}"#).await;
    assert_eq!(refused, "No agent you can message is called \"scout\"; you can message: writer.");

    // A grant lets Builder reach Scout, one way.
    ack(apply_request(&mut client, &mut view, grant("builder", &["scout"])).await);
    assert_eq!(config(&dir)["agents"][1]["extensions"], serde_json::json!({"agents": {"also": ["scout"]}}));
    assert_eq!(view.agent("builder").extensions[0].options["also"].agent_ids, ["scout"]);
    assert_eq!(call(&mut client, &mut view, "builder", "agents_status", "{}").await, "writer Writer: idle\nscout Scout (Lab): idle");
    assert_eq!(call(&mut client, &mut view, "builder", "agents_send", r#"{"to":"Scout","text":"hi"}"#).await, "Sent.");
    view.follow(&mut client, |view| idle(view, "scout")).await;

    // The Messaging panel shows both directions, by the same rule.
    assert_eq!(view.workspace.extensions[0].name, "Messaging");
    let messaging = |view: &View| match view.extension_view.as_ref().and_then(|view| view.body.clone()) {
        Some(v1::extension_view::Body::Messaging(messaging)) => (messaging.reaches, messaging.reached_by),
        other => panic!("expected the messaging panel, got {other:?}"),
    };
    // Each agent keeps its own panel open.
    for agent in ["scout", "builder"] {
        let open = v1::OpenPanel { extension_id: "agents".into(), agent_id: agent.into() };
        ack(apply_request(&mut client, &mut view, client_message::Body::OpenPanel(open)).await);
    }
    assert_eq!((view.agent("scout").open_panel.as_str(), view.agent("writer").open_panel.as_str()), ("agents", ""));
    apply_request(&mut client, &mut view, select("scout")).await;
    assert_eq!(messaging(&view), (Vec::<String>::new(), vec!["rivet".to_owned(), "builder".to_owned()]));
    apply_request(&mut client, &mut view, select("builder")).await;
    assert_eq!(messaging(&view), (vec!["writer".to_owned(), "scout".to_owned()], vec!["rivet".to_owned(), "writer".to_owned()]));
    // A grant changing while it's open shows at once.
    ack(apply_request(&mut client, &mut view, grant("builder", &[])).await);
    view.follow(&mut client, |view| messaging(view).0 == ["writer"]).await;
    ack(apply_request(&mut client, &mut view, grant("builder", &["scout"])).await);
    view.follow(&mut client, |view| messaging(view).0.len() == 2).await;
    ack(apply_request(
        &mut client,
        &mut view,
        client_message::Body::OpenPanel(v1::OpenPanel { extension_id: String::new(), agent_id: String::new() }),
    )
    .await);
    assert_eq!(call(&mut client, &mut view, "scout", "agents_status", "{}").await, "No agent can be messaged from here.");

    // The admin agent sees everyone who can be messaged; no one can message it.
    let everyone = call(&mut client, &mut view, "rivet", "agents_status", r#"{"agent":null}"#).await;
    let lines: Vec<&str> = everyone.lines().map(|line| line.split(':').next().expect("line")).collect();
    assert_eq!(lines, ["builder Builder (Site)", "writer Writer (Site)", "scout Scout (Lab)", "helper Helper"]);
    let refused = call(&mut client, &mut view, "helper", "agents_send", r#"{"to":"rivet","text":"done"}"#).await;
    assert!(refused.starts_with("No agent you can message is called \"rivet\""), "{refused}");

    // An admin agent signs as one, with its id for the app.
    assert_eq!(call(&mut client, &mut view, "rivet", "agents_send", r#"{"to":"scout","text":"how's it going?"}"#).await, "Sent.");
    view.follow(&mut client, |view| idle(view, "scout")).await;
    apply_request(&mut client, &mut view, select("scout")).await;
    let signed = view.entries.iter().find_map(|entry| match &entry.kind {
        Some(Kind::Event(event)) if event.text == "how's it going?" => Some((event.from.clone(), event.from_id.clone())),
        _ => None,
    });
    // Its role says it's the admin.
    assert_eq!(signed, Some(("Rivet (Admin)".to_owned(), "rivet".to_owned())));

    // Who can message whom: admins see the map; others once given it.
    let map = call(&mut client, &mut view, "rivet", "agents_map", "{}").await;
    assert_eq!(
        map.lines().collect::<Vec<_>>(),
        [
            "rivet Rivet (Admin): messages builder (Site), writer (Site), scout (Lab), helper; messaged by no one",
            "builder Builder (Site): messages writer (Site), scout (Lab); messaged by rivet, writer (Site)",
            "writer Writer (Site): messages builder (Site); messaged by rivet, builder (Site)",
            "scout Scout (Lab): messages no one; messaged by rivet, builder (Site)",
            "helper Helper: messages no one; messaged by rivet",
        ]
    );
    assert!(call(&mut client, &mut view, "builder", "agents_map", "{}").await.contains("not found"));
    ack(apply_request(&mut client, &mut view, set_switch("builder", "see_map", true)).await);
    fresh(&mut client, &mut view, "builder").await;
    let one = call(&mut client, &mut view, "builder", "agents_map", r#"{"agent":"Scout"}"#).await;
    assert_eq!(one, "scout Scout (Lab): messages no one; messaged by rivet (admin), builder");
    ack(apply_request(&mut client, &mut view, set_switch("builder", "see_map", false)).await);

    // Unreachable, or with the extension off, an agent disappears for others.
    ack(apply_request(&mut client, &mut view, set_switch("writer", "reachable", false)).await);
    assert!(call(&mut client, &mut view, "builder", "agents_status", "{}").await.starts_with("scout Scout (Lab): idle"));
    ack(apply_request(&mut client, &mut view, turn("scout", "agents", false)).await);
    assert_eq!(call(&mut client, &mut view, "builder", "agents_status", "{}").await, "No agent can be messaged from here.");
    assert_eq!(config(&dir)["agents"][3]["extensions"], serde_json::json!({"agents": {"enabled": false}}));
    // Its tools are refused; from its next context, it has neither them nor the prompt section.
    assert!(call(&mut client, &mut view, "scout", "agents_status", "{}").await.contains("turned off for you"));
    fresh(&mut client, &mut view, "scout").await;
    let unknown = call(&mut client, &mut view, "scout", "agents_status", "{}").await;
    assert!(unknown.contains("not found"), "{unknown}");
    let prompt = system_prompt.lock().expect("lock").clone();
    assert!(!prompt.contains("## Other agents"), "{prompt}");

    // A deleted agent leaves no grants behind.
    ack(apply_request(&mut client, &mut view, client_message::Body::DeleteAgent(v1::DeleteAgent { agent_id: "scout".into() })).await);
    assert_eq!(config(&dir)["agents"][1].get("extensions"), None, "nothing left to say");
    assert!(view.agent("builder").extensions[0].options["also"].agent_ids.is_empty());
}

#[tokio::test]
async fn extension_settings_are_checked_and_unknown_ones_kept() {
    let dir = temp_dir();
    let (addr, _factory) = start(&dir).await;
    let (mut client, workspace, _) = Client::connect(addr).await;
    let mut view = View { workspace, ..View::default() };
    apply_request(&mut client, &mut view, create_agent("", "Builder", "")).await;

    for (bad, code) in [
        (grant("builder", &["nobody"]), v1::ErrorCode::NotFound),
        (turn("builder", "nope", true), v1::ErrorCode::NotFound),
        (set_switch("builder", "loud", true), v1::ErrorCode::NotFound),
    ] {
        match apply_request(&mut client, &mut view, bad).await {
            Body::Error(error) => assert_eq!(error.code(), code, "{}", error.message),
            other => panic!("expected an error, got {other:?}"),
        }
    }

    // In config.json: an extension Factory doesn't have is kept and ignored,
    // a value of the wrong kind is an error until fixed.
    let path = dir.join("home/config.json");
    let mut written = config(&dir);
    written["agents"][1]["extensions"] = serde_json::json!({"later": {"enabled": true}, "agents": {"admin": true}});
    std::fs::write(&path, serde_json::to_string_pretty(&written).expect("json")).expect("write");
    view.follow(&mut client, |view| view.agent("builder").extensions[0].options["admin"].on).await;
    assert_eq!(view.workspace.config_error, "");
    written["agents"][1]["extensions"]["agents"]["admin"] = serde_json::json!("yes");
    std::fs::write(&path, serde_json::to_string_pretty(&written).expect("json")).expect("write");
    view.follow(&mut client, |view| !view.workspace.config_error.is_empty()).await;
    assert!(view.workspace.config_error.contains("\"agents.admin\" must be true or false"), "{}", view.workspace.config_error);
}

#[tokio::test]
async fn a_checklist_keeps_what_the_person_asked_for_and_its_panel_follows_live() {
    let dir = temp_dir();
    let (addr, _old) = start(&dir).await;
    let (mut client, workspace, _) = Client::connect(addr).await;
    let mut view = View { workspace, ..View::default() };
    apply_request(&mut client, &mut view, create_project("Site", &dir.join("site"))).await;
    apply_request(&mut client, &mut view, create_agent("site", "Lead", "")).await;
    apply_request(&mut client, &mut view, create_agent("site", "Builder", "")).await;

    // Off by default: no tools until it's turned on.
    assert!(call(&mut client, &mut view, "lead", "checklist_list", "{}").await.contains("not found"));
    ack(apply_request(&mut client, &mut view, turn("lead", "checklist", true)).await);
    assert!(view.agent("lead").extensions[1].enabled);
    // Its tools come with its next context.
    assert!(call(&mut client, &mut view, "lead", "checklist_list", "{}").await.contains("not found"));
    fresh(&mut client, &mut view, "lead").await;

    // The panel opens beside the selected agent and shows its checklist.
    let open = |id: &str| client_message::Body::OpenPanel(v1::OpenPanel { extension_id: id.into(), agent_id: String::new() });
    ack(apply_request(&mut client, &mut view, open("checklist")).await);
    assert_eq!(view.workspace.open_panel, "checklist");
    let shown = view.extension_view.clone().expect("the panel's content");
    assert_eq!((shown.extension_id.as_str(), shown.agent_id.as_str(), shown.title.as_str()), ("checklist", "lead", "Checklist"));

    let added = call(&mut client, &mut view, "lead", "checklist_add", r#"{"items":["Fix login","Dark mode","Release notes"]}"#).await;
    assert_eq!(added, "Added #1-#3.");
    let updated = r#"{"updates":[{"id":1,"status":"done","note":"shipped"},{"id":2,"owner":"Builder","status":"in_progress"},{"id":7,"status":"done"}]}"#;
    let said = call(&mut client, &mut view, "lead", "checklist_update", updated).await;
    assert_eq!(said, "#1 done, noted; #2 in_progress, owner builder; #7: no such item.");

    let items = |view: &View| match view.extension_view.as_ref().and_then(|view| view.body.clone()) {
        Some(v1::extension_view::Body::Checklist(checklist)) => checklist.items,
        _ => Vec::new(),
    };
    let shown = items(&view);
    assert_eq!(shown.len(), 3);
    assert_eq!(
        (shown[0].status(), shown[1].status(), shown[1].owner.as_str()),
        (v1::ChecklistStatus::Done, v1::ChecklistStatus::InProgress, "builder")
    );
    let log: Vec<(&str, &str)> = shown[0].log.iter().map(|update| (update.author.as_str(), update.text.as_str())).collect();
    assert_eq!(log, [("lead", "Added"), ("lead", "Done: shipped")]);
    assert!(shown[0].updated >= shown[0].created && shown[0].created > 0);

    // Open items by default, one terse line each.
    let open_items = call(&mut client, &mut view, "lead", "checklist_list", "{}").await;
    let lines: Vec<String> = open_items.lines().map(|line| line.rsplit_once(' ').expect("day").0.to_owned()).collect();
    assert_eq!(lines, ["#2 [~] Dark mode (builder)", "#3 [ ] Release notes"]);
    let limited = call(&mut client, &mut view, "lead", "checklist_list", r#"{"status":"all","limit":1}"#).await;
    assert!(limited.ends_with("\n+2 more; narrow with q, day, owner or status, or raise limit."), "{limited}");
    let history = call(&mut client, &mut view, "lead", "checklist_item", r#"{"id":1}"#).await;
    assert!(history.lines().nth(2).expect("second change").ends_with(" lead: Done: shipped"), "{history}");

    // Another agent with a checklist reads Lead's.
    ack(apply_request(&mut client, &mut view, turn("builder", "checklist", true)).await);
    fresh(&mut client, &mut view, "builder").await;
    let read = call(&mut client, &mut view, "builder", "checklist_list", r#"{"agent":"lead","q":"release"}"#).await;
    assert!(read.starts_with("#3 [ ] Release notes"), "{read}");
    // Builder is selected now, with no panel open; opened, its own (empty) checklist shows.
    assert_eq!((view.workspace.open_panel.as_str(), view.agent("lead").open_panel.as_str()), ("", "checklist"));
    ack(apply_request(&mut client, &mut view, open("checklist")).await);
    assert_eq!(view.extension_view.as_ref().map(|shown| shown.agent_id.as_str()), Some("builder"));
    assert!(items(&view).is_empty());
    // A panel whose extension is turned off counts as closed.
    ack(apply_request(&mut client, &mut view, turn("builder", "checklist", false)).await);
    assert_eq!(view.agent("builder").open_panel, "");

    // What's open, and the checklist, survive a restart.
    apply_request(&mut client, &mut view, select("lead")).await;
    let (addr, _new) = start(&dir).await;
    let (mut client, workspace, _) = Client::connect(addr).await;
    let mut view = View { workspace, ..View::default() };
    assert_eq!(view.workspace.open_panel, "checklist");
    view.follow(&mut client, |view| view.extension_view.is_some()).await;
    assert_eq!(items(&view).len(), 3);

    // Closing the panel.
    ack(apply_request(&mut client, &mut view, open("")).await);
    assert_eq!(view.workspace.open_panel, "");
}

#[tokio::test]
async fn agents_with_the_same_name_in_different_projects_are_told_apart() {
    let dir = temp_dir();
    let (addr, _factory) = start(&dir).await;
    let (mut client, workspace, _) = Client::connect(addr).await;
    let mut view = View { workspace, ..View::default() };
    apply_request(&mut client, &mut view, create_project("Web", &dir.join("web"))).await;
    apply_request(&mut client, &mut view, create_project("App", &dir.join("app"))).await;
    for (project, name) in [("web", "Frontend"), ("app", "Frontend"), ("app", "Lead")] {
        apply_request(&mut client, &mut view, create_agent(project, name, "")).await;
    }
    let rivet = v1::UpdateAgent { agent_id: "rivet".into(), model: Some(MODEL.into()), effort: Some("off".into()), ..Default::default() };
    apply_request(&mut client, &mut view, client_message::Body::UpdateAgent(rivet)).await;
    // The second Frontend's id says which project it's in.
    let ids: Vec<&str> = view.workspace.agents.iter().map(|agent| agent.id.as_str()).collect();
    assert_eq!(ids, ["rivet", "frontend", "app-frontend", "lead"]);

    // Lead, granted Web's Frontend too, sees both, each with its id.
    ack(apply_request(&mut client, &mut view, grant("lead", &["frontend"])).await);
    let status = call(&mut client, &mut view, "lead", "agents_status", "{}").await;
    let lines: Vec<&str> = status.lines().map(|line| line.split(':').next().expect("line")).collect();
    assert_eq!(lines, ["frontend Frontend (Web)", "app-frontend Frontend"]);
    // By name, its own project's Frontend.
    assert_eq!(call(&mut client, &mut view, "lead", "agents_send", r#"{"to":"Frontend","text":"Yours"}"#).await, "Sent.");
    view.follow(&mut client, |view| idle(view, "app-frontend")).await;
    apply_request(&mut client, &mut view, select("app-frontend")).await;
    let Some(Kind::Event(event)) = &view.entries[0].kind else { panic!("an event") };
    assert_eq!((event.from.as_str(), event.text.as_str()), ("Lead", "Yours"));
    // From another project, a message names it, and replies can use that.
    assert_eq!(call(&mut client, &mut view, "lead", "agents_send", r#"{"to":"frontend","text":"Theirs"}"#).await, "Sent.");
    view.follow(&mut client, |view| idle(view, "frontend")).await;
    apply_request(&mut client, &mut view, select("frontend")).await;
    let Some(Kind::Event(event)) = &view.entries[0].kind else { panic!("an event") };
    assert_eq!((event.from.as_str(), event.from_id.as_str()), ("Lead (App)", "lead"));
    let Some(Kind::Assistant(read)) = &view.entries[1].kind else { panic!("a reply") };
    assert!(read.text.contains("<event source=\"agent\" from=\"Lead (App)\" id=\"lead\">"), "{}", read.text);

    // The admin agent, in neither project, must say which.
    let ambiguous = call(&mut client, &mut view, "rivet", "agents_send", r#"{"to":"frontend ","text":"hi"}"#).await;
    assert_eq!(ambiguous, "Sent.", "an id is exact");
    let ambiguous = call(&mut client, &mut view, "rivet", "agents_send", r#"{"to":"Frontend","text":"hi"}"#).await;
    assert_eq!(ambiguous, "Several agents are called \"Frontend\"; use an id: frontend (Web), app-frontend (App).");
    assert_eq!(call(&mut client, &mut view, "rivet", "agents_send", r#"{"to":"Frontend (App)","text":"hi"}"#).await, "Sent.");
    let missing = call(&mut client, &mut view, "app-frontend", "agents_send", r#"{"to":"Nobody","text":"hi"}"#).await;
    assert_eq!(missing, "No agent you can message is called \"Nobody\"; you can message: lead.");
    let missing = call(&mut client, &mut view, "lead", "agents_send", r#"{"to":"Nobody","text":"hi"}"#).await;
    assert_eq!(missing, "No agent you can message is called \"Nobody\"; you can message: frontend (Web), app-frontend.");
}

#[tokio::test]
async fn memory_is_the_agents_own_and_its_prompt_holds_still_until_the_next_compaction() {
    let dir = temp_dir();
    let models = Scripted::signed_in();
    let system_prompt = models.system_prompt.clone();
    let (addr, _factory) = start_with(&dir, models).await;
    let (mut client, workspace, _) = Client::connect(addr).await;
    let mut view = View { workspace, ..View::default() };
    apply_request(&mut client, &mut view, create_agent("", "Ops", "")).await;
    let prompt = || system_prompt.lock().expect("lock").clone();

    // Off by default.
    assert!(call(&mut client, &mut view, "ops", "memory_read", "{}").await.contains("not found"));
    assert!(!prompt().contains("## Memory"));
    ack(apply_request(&mut client, &mut view, turn("ops", "memory", true)).await);
    assert_eq!(config(&dir)["agents"][1]["extensions"], serde_json::json!({"memory": {"enabled": true}}));
    fresh(&mut client, &mut view, "ops").await;

    let open = client_message::Body::OpenPanel(v1::OpenPanel { extension_id: "memory".into(), agent_id: String::new() });
    ack(apply_request(&mut client, &mut view, open).await);
    let learned = r#"{"ops":[{"op":"pref_add","rule":"Don't propose paid CI","why":"everything runs locally"},{"op":"note_write","slug":"deploy","summary":"How releases ship","body":"Run scripts/release.sh.\nNever install."}]}"#;
    assert_eq!(call(&mut client, &mut view, "ops", "memory_update", learned).await, "ok: pref #1 added; note deploy written");
    // The section was written when the context began, before anything was learned…
    let before = prompt();
    assert!(before.contains("## Memory\n") && before.contains("## Your memory"), "{before}");
    assert!(before.contains("Preferences:\n(none yet)\n"), "{before}");
    // …and holds still for the rest of it, so the prompt cache does too.
    assert_eq!(
        call(&mut client, &mut view, "ops", "memory_read", "{}").await.lines().nth(1),
        Some("#1 Don't propose paid CI. Why: everything runs locally")
    );
    assert_eq!(prompt(), before);
    assert_eq!(call(&mut client, &mut view, "ops", "memory_read", r#"{"q":"INSTALL"}"#).await, "note deploy:2: Never install.");
    let note = call(&mut client, &mut view, "ops", "memory_read", r#"{"note":"deploy"}"#).await;
    assert!(
        note.starts_with("deploy: How releases ship (updated ") && note.ends_with("\n\nRun scripts/release.sh.\nNever install."),
        "{note}"
    );

    // The panel shows it all, with who changed what.
    let memory = |view: &View| match view.extension_view.as_ref().and_then(|view| view.body.clone()) {
        Some(v1::extension_view::Body::Memory(memory)) => memory,
        other => panic!("expected the memory panel, got {other:?}"),
    };
    let shown = memory(&view);
    assert_eq!((shown.preferences[0].id, shown.preferences[0].rule.as_str()), (1, "Don't propose paid CI"));
    assert_eq!((shown.notes[0].slug.as_str(), shown.notes[0].body.as_str()), ("deploy", "Run scripts/release.sh.\nNever install."));
    let changes: Vec<(v1::MemoryChangeKind, &str)> = shown.changes.iter().map(|change| (change.kind(), change.actor.as_str())).collect();
    assert_eq!(changes, [(v1::MemoryChangeKind::NoteWritten, "ops"), (v1::MemoryChangeKind::PreferenceAdded, "ops")]);

    // A compaction starts a new context, whose prompt has what was learned.
    let compact_at =
        |tokens| client_message::Body::UpdateSettings(v1::UpdateSettings { compact_at_tokens: Some(tokens), ..Default::default() });
    ack(apply_request(&mut client, &mut view, compact_at(1_000)).await);
    apply_request(&mut client, &mut view, send_message("ops", "carry on")).await;
    view.follow(&mut client, |view| view.entries.iter().any(|entry| matches!(entry.kind, Some(Kind::Compaction(_)))) && idle(view, "ops"))
        .await;
    ack(apply_request(&mut client, &mut view, compact_at(400_000)).await);
    let after = prompt();
    assert!(after.contains("Preferences:\n#1 Don't propose paid CI. Why: everything runs locally ("), "{after}");
    assert!(after.contains("Knowledge notes:\n- deploy: How releases ship\n"), "{after}");
    let segment: serde_json::Value =
        serde_json::from_slice(&std::fs::read(dir.join("home/agents/ops/segment-0002.json")).expect("segment")).expect("json");
    assert!(segment["pinned"]["memory"].as_str().expect("kept with the segment").contains("#1 Don't propose paid CI"));

    // Kept with the segment, it's the same after a restart, though memory changed since.
    let more = r#"{"ops":[{"op":"pref_add","rule":"Answer briefly","why":"he skims"}]}"#;
    assert_eq!(call(&mut client, &mut view, "ops", "memory_update", more).await, "ok: pref #2 added");
    let models = Scripted::signed_in();
    let system_prompt = models.system_prompt.clone();
    let (addr, _new) = start_with(&dir, models).await;
    let (mut client, workspace, _) = Client::connect(addr).await;
    let mut view = View { workspace, ..View::default() };
    call(&mut client, &mut view, "ops", "memory_read", "{}").await;
    let restarted = system_prompt.lock().expect("lock").clone();
    assert!(restarted.contains("#1 Don't propose paid CI") && !restarted.contains("Answer briefly"), "{restarted}");

    // Deleting the agent deletes its memory.
    ack(apply_request(&mut client, &mut view, client_message::Body::DeleteAgent(v1::DeleteAgent { agent_id: "ops".into() })).await);
    let db = rusqlite::Connection::open(dir.join("home/factory.db")).expect("db");
    for table in ["memory_preferences", "memory_notes", "memory_events"] {
        let left: i64 = db.query_row(&format!("SELECT COUNT(*) FROM {table} WHERE agent = 'ops'"), [], |row| row.get(0)).expect("count");
        assert_eq!(left, 0, "{table}");
    }
}

#[tokio::test]
async fn an_agents_system_prompt_and_tools_hold_still_and_changes_are_appended() {
    let dir = temp_dir();
    let models = Scripted::signed_in();
    let (system_prompt, tools) = (models.system_prompt.clone(), models.tools.clone());
    let (addr, _factory) = start_with(&dir, models).await;
    let (mut client, workspace, _) = Client::connect(addr).await;
    let mut view = View { workspace, ..View::default() };
    apply_request(&mut client, &mut view, create_project("Site", &dir.join("site"))).await;
    for name in ["Lead", "Builder", "Helper"] {
        apply_request(&mut client, &mut view, create_agent("site", name, "")).await;
    }
    let sent = || (system_prompt.lock().expect("lock").clone(), tools.lock().expect("lock").clone());
    let talk = async |client: &mut Client, view: &mut View, text: &str| {
        apply_request(client, view, select("lead")).await;
        let before = view.entries.len();
        apply_request(client, view, send_message("lead", text)).await;
        view.follow(client, |view| view.entries.len() >= before + 2 && idle(view, "lead")).await;
        describe(&view.entries[before..])
    };
    talk(&mut client, &mut view, "hello").await;
    let first = sent();
    assert!(first.0.contains("You can message: Builder (builder), Helper (helper)."), "{}", first.0);

    // Others come, go and change: Lead's prompt and tools don't move…
    apply_request(&mut client, &mut view, create_agent("site", "Designer", "")).await;
    let rename = v1::UpdateAgent { agent_id: "builder".into(), name: Some("Maker".into()), ..Default::default() };
    ack(apply_request(&mut client, &mut view, client_message::Body::UpdateAgent(rename)).await);
    ack(apply_request(&mut client, &mut view, set_switch("helper", "reachable", false)).await);
    let said = talk(&mut client, &mut view, "again").await;
    assert_eq!(sent(), first);
    // …and it hears what changed, once, before what it was sent.
    assert_eq!(
        said,
        [
            "event from Claktory: You can now message: Maker (builder), Designer (designer).\nYou can no longer message: Helper (helper).",
            "user: again",
            "assistant: echo: again",
        ]
    );
    assert_eq!(talk(&mut client, &mut view, "and again").await, ["user: and again", "assistant: echo: and again"]);
    assert_eq!(sent(), first);

    // Across a restart too.
    let models = Scripted::signed_in();
    let (system_prompt, tools, compacted) = (models.system_prompt.clone(), models.tools.clone(), models.compacted.clone());
    let (addr, _new) = start_with(&dir, models).await;
    let (mut client, workspace, _) = Client::connect(addr).await;
    let mut view = View { workspace, ..View::default() };
    talk(&mut client, &mut view, "still there?").await;
    let sent = || (system_prompt.lock().expect("lock").clone(), tools.lock().expect("lock").clone());
    assert_eq!(sent(), first);

    // New instructions apply at once: a new system prompt, with the tools
    // kept. An extension turned on comes as a note with its instructions,
    // its tools called through extension_call until the next context.
    let instructed = v1::UpdateAgent { agent_id: "lead".into(), prompt: Some("Lead the site.".into()), ..Default::default() };
    ack(apply_request(&mut client, &mut view, client_message::Body::UpdateAgent(instructed)).await);
    ack(apply_request(&mut client, &mut view, turn("lead", "checklist", true)).await);
    let said = talk(&mut client, &mut view, "now?").await;
    let (prompt, now_tools) = sent();
    assert!(prompt.contains("## Your instructions\n\nLead the site."), "{prompt}");
    assert_eq!(now_tools, first.1, "the tools hold still");
    assert!(
        said[0].starts_with("event from Claktory: Your settings changed: the Checklist extension is now on, and yours to use now (below). Your instructions changed; the new ones are in your system prompt from now on."),
        "{said:?}"
    );
    assert!(
        said[0].contains(
            "\n\nIts instructions, which your system prompt has from your next context (after compaction or /clear):\n\n## Checklist"
        ),
        "{said:?}"
    );
    assert!(said[0].contains("call them through extension_call") && said[0].contains("\n- checklist_add: "), "{said:?}");
    assert_eq!(talk(&mut client, &mut view, "and now?").await.len(), 2, "told once");
    let added = talk(&mut client, &mut view, r#"call extension_call {"tool": "checklist_add", "args": {"items": ["Ship it"]}}"#).await;
    assert!(added.iter().any(|line| line.contains("Added #1.")), "{added:?}");
    let first = sent();
    // A default that comes with other settings is news too: an admin agent
    // has the Factory manual.
    ack(apply_request(&mut client, &mut view, set_switch("lead", "admin", true)).await);
    let said = talk(&mut client, &mut view, "admin now?").await;
    assert!(said[0].contains("the Claktory manual extension is now on, and yours to use now (below)"), "{said:?}");
    assert!(said[0].contains("## The Claktory manual") && said[0].contains("\n- memory_read: "), "{said:?}");
    ack(apply_request(&mut client, &mut view, set_switch("lead", "admin", false)).await);
    let said = talk(&mut client, &mut view, "not now?").await;
    assert!(said[0].contains("the Claktory manual extension is now off"), "{said:?}");
    assert_eq!(sent(), first);
    // Turned off, its tools are refused until the next context.
    ack(apply_request(&mut client, &mut view, turn("lead", "agents", false)).await);
    let said = talk(&mut client, &mut view, r#"call agents_status {}"#).await;
    assert!(said[0].contains("the Messaging extension is now off; stop using its tools."), "{said:?}");
    assert!(said[2].contains("The Messaging extension is turned off for you"), "{said:?}");
    assert_eq!(sent(), first);
    ack(apply_request(&mut client, &mut view, turn("lead", "agents", true)).await);

    // A compaction starts the next context, with its settings as they are now.
    // The summary is asked for after the context as it was sent, notes and all,
    // so the prompt cache holds; the model is told to leave the notes out.
    let compact = v1::RunCommand { agent_id: "lead".into(), name: "compact".into(), args: String::new() };
    ack(apply_request(&mut client, &mut view, client_message::Body::RunCommand(compact)).await);
    view.follow(&mut client, |view| view.entries.iter().any(|entry| matches!(entry.kind, Some(Kind::Compaction(_)))) && idle(view, "lead"))
        .await;
    let summarized = compacted.lock().expect("lock").clone();
    assert!(summarized.contains("now?") && summarized.contains("Your settings changed"), "{summarized}");
    assert!(summarized.contains("Leave out the notes from the program running you"), "{summarized}");
    talk(&mut client, &mut view, "after").await;
    let (prompt, tools) = sent();
    assert!(prompt.contains("Lead the site.") && tools.contains("checklist_add"), "{prompt}");

    // A new model can't use the old one's cache: a new frame, tools and all.
    let after = sent();
    let model = v1::UpdateAgent { agent_id: "lead".into(), model: Some(OTHER_MODEL.into()), ..Default::default() };
    ack(apply_request(&mut client, &mut view, client_message::Body::UpdateAgent(model)).await);
    let renamed = v1::UpdateAgent { agent_id: "lead".into(), name: Some("Captain".into()), ..Default::default() };
    ack(apply_request(&mut client, &mut view, client_message::Body::UpdateAgent(renamed)).await);
    let said = talk(&mut client, &mut view, "new model").await;
    assert!(said[0].contains(&format!("your model is now {OTHER_MODEL}; your name is now Captain")), "{said:?}");
    assert!(sent().0.contains("You are Captain") && sent() != after);
}

/// A working agent's model, effort and instructions apply from its next model
/// call, as the person changes them; an extension turned on meanwhile comes
/// as one note, with the frame as it was.
#[tokio::test]
async fn a_working_agents_settings_apply_from_its_next_model_call() {
    let dir = temp_dir();
    let models = Scripted::signed_in();
    let requests = models.requests.clone();
    let (addr, _factory) = start_with(&dir, models).await;
    let (mut client, workspace, _) = Client::connect(addr).await;
    let mut view = View { workspace, ..View::default() };
    apply_request(&mut client, &mut view, create_project("Site", &dir.join("site"))).await;
    apply_request(&mut client, &mut view, create_agent("site", "Lead", "")).await;
    apply_request(&mut client, &mut view, select("lead")).await;
    let working = async |client: &mut Client, view: &mut View, change: client_message::Body| {
        let before = view.entries.len();
        apply_request(client, view, send_message("lead", r#"call bash {"command": "sleep 1"}"#)).await;
        view.follow(client, |view| view.entries[before..].iter().any(|entry| matches!(entry.kind, Some(Kind::ToolCall(_))))).await;
        ack(apply_request(client, view, change).await);
        view.follow(client, |view| view.entries.len() >= before + 3 && idle(view, "lead")).await;
        describe(&view.entries[before..])
    };

    let change = v1::UpdateAgent {
        agent_id: "lead".into(),
        model: Some(OTHER_MODEL.into()),
        effort: Some("high".into()),
        prompt: Some("Be brief.".into()),
        ..Default::default()
    };
    let said = working(&mut client, &mut view, client_message::Body::UpdateAgent(change)).await;
    let sent = requests.lock().expect("lock").clone();
    let (before, after) = (&sent[sent.len() - 2], &sent[sent.len() - 1]);
    assert_eq!((before.model.as_str(), before.effort.as_str()), ("echo", "off"));
    assert_eq!((after.model.as_str(), after.effort.as_str()), ("parrot", "high"), "the next call, mid-run");
    assert!(!before.system_prompt.contains("Be brief.") && after.system_prompt.contains("## Your instructions\n\nBe brief."));
    let note = said.iter().find(|line| line.starts_with("event from Claktory")).expect("a note");
    assert_eq!(
        note,
        "event from Claktory: Your settings changed: your model is now scripted/parrot; your effort is now high. Your instructions changed; the new ones are in your system prompt from now on."
    );

    // An extension turned on mid-run: the same system prompt and tools, and
    // one note, after which its tools work through extension_call.
    let said = working(&mut client, &mut view, turn("lead", "checklist", true)).await;
    let sent = requests.lock().expect("lock").clone();
    let (before, after) = (&sent[sent.len() - 2], &sent[sent.len() - 1]);
    assert_eq!((&before.system_prompt, &before.tools), (&after.system_prompt, &after.tools));
    assert!(after.tools.contains(&"extension_call".to_owned()) && !after.tools.contains(&"checklist_add".to_owned()));
    let notes: Vec<&String> = said.iter().filter(|line| line.starts_with("event from Claktory")).collect();
    assert_eq!(notes.len(), 1, "{said:?}");
    assert!(notes[0].contains("the Checklist extension is now on, and yours to use now") && notes[0].contains("- checklist_add: "));
    let added = working(&mut client, &mut view, client_message::Body::SelectAgent(v1::SelectAgent { agent_id: "lead".into() })).await;
    assert!(!added.iter().any(|line| line.starts_with("event from Claktory")), "told once: {added:?}");

    // Moved to another project mid-run: it works there from the next call,
    // with a system prompt and tools for it (the checklist's among them now).
    apply_request(&mut client, &mut view, create_project("Shop", &dir.join("shop"))).await;
    let moved = v1::UpdateAgent { agent_id: "lead".into(), project_id: Some("shop".into()), ..Default::default() };
    let said = working(&mut client, &mut view, client_message::Body::UpdateAgent(moved)).await;
    let sent = requests.lock().expect("lock").clone();
    let (before, after) = (&sent[sent.len() - 2], &sent[sent.len() - 1]);
    assert!(before.system_prompt.contains("You work in the project \"Site\""), "{}", before.system_prompt);
    assert!(after.system_prompt.contains("You work in the project \"Shop\""), "{}", after.system_prompt);
    assert!(!before.tools.contains(&"checklist_add".to_owned()) && after.tools.contains(&"checklist_add".to_owned()));
    assert!(said.iter().any(|line| line.contains("you're now in the project \"Shop\"")), "{said:?}");
}

#[tokio::test]
async fn an_idle_agent_with_work_outstanding_is_reminded_once_until_something_changes() {
    let dir = temp_dir();
    let (addr, factory) = start(&dir).await;
    factory.set_nudge_after(std::time::Duration::from_millis(150));
    let (mut client, workspace, _) = Client::connect(addr).await;
    let mut view = View { workspace, ..View::default() };
    apply_request(&mut client, &mut view, create_project("Site", &dir.join("site"))).await;
    apply_request(&mut client, &mut view, create_agent("site", "Lead", "")).await;
    apply_request(&mut client, &mut view, create_agent("site", "Worker", "")).await;
    ack(apply_request(&mut client, &mut view, turn("lead", "checklist", true)).await);
    let reminders = |view: &View| {
        view.entries.iter().filter(|entry| matches!(&entry.kind, Some(Kind::Event(event)) if event.text.starts_with("You have"))).count()
    };
    let quiet = async |client: &mut Client, view: &mut View| {
        // Long enough for a reminder to come, if one would.
        let until = tokio::time::Instant::now() + std::time::Duration::from_millis(700);
        while let Ok(message) = tokio::time::timeout_at(until, client.receive()).await {
            if let Some(body) = &message.body {
                view.apply(body);
            }
        }
    };

    // The worker it gave #1 is working: nothing to remind it of yet.
    apply_request(&mut client, &mut view, send_message("worker", "hang")).await;
    let added = r#"{"items":["Fix login"],"owner":"worker"}"#;
    assert_eq!(call(&mut client, &mut view, "lead", "checklist_add", added).await, "Added #1.");
    quiet(&mut client, &mut view).await;
    assert_eq!(reminders(&view), 0);

    // It stops: Lead is reminded, once.
    ack(apply_request(&mut client, &mut view, client_message::Body::StopAgent(v1::StopAgent { agent_id: "worker".into() })).await);
    view.follow(&mut client, |view| reminders(view) == 1 && idle(view, "lead")).await;
    let reminder = view.entries.iter().rev().find_map(|entry| match &entry.kind {
        Some(Kind::Event(event)) if event.text.starts_with("You have") => Some(event.clone()),
        _ => None,
    });
    let reminder = reminder.expect("a reminder");
    assert_eq!(
        reminder.text,
        "You have 1 outstanding item and nothing running will wake you: #1 Fix login. Carry on, or mark what's waiting with status blocked and a note saying on what."
    );
    assert_eq!((reminder.from.as_str(), reminder.summary.as_str()), ("Claktory", "Reminder: 1 outstanding checklist item"));
    quiet(&mut client, &mut view).await;
    assert_eq!(reminders(&view), 1, "no repeat while nothing changes");

    // A background task will wake it, so no reminder while it runs; its end
    // is news, and then it's reminded again.
    let background = r#"{"command":"sleep 1","run_in_background":true}"#;
    call(&mut client, &mut view, "lead", "bash", background).await;
    quiet(&mut client, &mut view).await;
    assert_eq!(reminders(&view), 1);
    view.follow(&mut client, |view| reminders(view) == 2 && idle(view, "lead")).await;

    // Blocked items wait on someone else: no reminder for them.
    let blocked = r#"{"updates":[{"id":1,"status":"blocked","note":"waiting on the person's API key"}]}"#;
    assert_eq!(call(&mut client, &mut view, "lead", "checklist_update", blocked).await, "#1 blocked, noted.");
    quiet(&mut client, &mut view).await;
    assert_eq!(reminders(&view), 2);
    assert!(call(&mut client, &mut view, "lead", "checklist_list", "{}").await.starts_with("#1 [!] Fix login (worker)"));
    let open = client_message::Body::OpenPanel(v1::OpenPanel { extension_id: "checklist".into(), agent_id: String::new() });
    ack(apply_request(&mut client, &mut view, open).await);
    let note = match view.extension_view.as_ref().and_then(|view| view.body.clone()) {
        Some(v1::extension_view::Body::Checklist(checklist)) => checklist.items[0].note.clone(),
        other => panic!("expected the checklist, got {other:?}"),
    };
    assert_eq!(note, "waiting on the person's API key", "what it waits on, for the panel");

    // And it can be turned off.
    let reopen = r#"{"updates":[{"id":1,"status":"open"}]}"#;
    let off = v1::ExtensionChange {
        id: "checklist".into(),
        option_id: "nudge".into(),
        value: Some(v1::OptionValue { on: false, agent_ids: Vec::new() }),
        ..Default::default()
    };
    ack(apply_request(&mut client, &mut view, change("lead", off)).await);
    call(&mut client, &mut view, "lead", "checklist_update", reopen).await;
    quiet(&mut client, &mut view).await;
    assert_eq!(reminders(&view), 2);
}

#[tokio::test]
async fn a_working_agent_hears_of_its_settings_changing_at_its_next_step() {
    let dir = temp_dir();
    let models = Scripted::signed_in();
    let system_prompt = models.system_prompt.clone();
    let (addr, _factory) = start_with(&dir, models).await;
    let (mut client, workspace, _) = Client::connect(addr).await;
    let mut view = View { workspace, ..View::default() };
    apply_request(&mut client, &mut view, create_project("Site", &dir.join("site"))).await;
    apply_request(&mut client, &mut view, create_agent("site", "Lead", "")).await;
    apply_request(&mut client, &mut view, select("lead")).await;
    apply_request(&mut client, &mut view, send_message("lead", r#"call bash {"command":"sleep 1"}"#)).await;
    view.follow(&mut client, |view| view.agent("lead").status() == AgentStatus::Working).await;
    ack(apply_request(&mut client, &mut view, turn("lead", "checklist", true)).await);
    view.follow(&mut client, |view| view.entries.len() >= 4 && idle(view, "lead")).await;
    let said = describe(&view.entries);
    assert!(said[1].starts_with("tool bash:"), "{said:?}");
    assert!(said[2].starts_with("event from Claktory: Your settings changed: the Checklist extension is now on"), "{said:?}");
    let prompt = system_prompt.lock().expect("lock").clone();
    assert!(prompt.contains("## Other agents") && !prompt.contains("## Checklist"), "the prompt holds still");
}

#[tokio::test]
async fn agents_have_a_role_beside_their_name_that_other_agents_see() {
    let dir = temp_dir();
    let models = Scripted::signed_in();
    let system_prompt = models.system_prompt.clone();
    let (addr, _factory) = start_with(&dir, models).await;
    let (mut client, workspace, _) = Client::connect(addr).await;
    let mut view = View { workspace, ..View::default() };
    assert_eq!((view.agent("rivet").name.as_str(), view.agent("rivet").role.as_str()), ("Rivet", "Admin"));
    let rivet = v1::UpdateAgent { agent_id: "rivet".into(), model: Some(MODEL.into()), effort: Some("off".into()), ..Default::default() };
    apply_request(&mut client, &mut view, client_message::Body::UpdateAgent(rivet)).await;
    apply_request(&mut client, &mut view, create_project("Site", &dir.join("site"))).await;
    apply_request(&mut client, &mut view, create_project("Lab", &dir.join("lab"))).await;
    let hire = |project: &str, name: &str, role: &str| {
        let client_message::Body::CreateAgent(mut agent) = create_agent(project, name, "") else { unreachable!() };
        agent.role = role.into();
        client_message::Body::CreateAgent(agent)
    };
    for (project, name, role) in [("site", "Mike", " Coordinator "), ("site", "Ana", "Backend"), ("lab", "Zoe", "Researcher")] {
        ack(apply_request(&mut client, &mut view, hire(project, name, role)).await);
    }
    assert_eq!(view.agent("mike").role, "Coordinator");
    assert_eq!(config(&dir)["agents"][1]["role"], "Coordinator");

    // Other agents see it with the name, and the agent knows its own.
    assert_eq!(call(&mut client, &mut view, "ana", "agents_status", "{}").await, "mike Mike (Coordinator): idle");
    let prompt = system_prompt.lock().expect("lock").clone();
    assert!(prompt.contains("You are Ana, a persistent agent in Claktory. Your role: Backend."), "{prompt}");
    assert!(prompt.contains("You can message: Mike (mike, Coordinator)."), "{prompt}");
    let everyone = call(&mut client, &mut view, "rivet", "agents_status", "{}").await;
    assert!(everyone.contains("zoe Zoe (Researcher, Lab): idle"), "{everyone}");

    // A message finds an agent by its role too, and is signed with the sender's.
    assert_eq!(call(&mut client, &mut view, "ana", "agents_send", r#"{"to":"coordinator","text":"done"}"#).await, "Sent.");
    view.follow(&mut client, |view| idle(view, "mike")).await;
    apply_request(&mut client, &mut view, select("mike")).await;
    let signed = view.entries.iter().find_map(|entry| match &entry.kind {
        Some(Kind::Event(event)) if event.text == "done" => Some(event.from.clone()),
        _ => None,
    });
    assert_eq!(signed.as_deref(), Some("Ana (Backend)"));
    assert_eq!(call(&mut client, &mut view, "zoe", "agents_status", "{}").await, "No agent can be messaged from here.");

    // A new role: the agent hears it, and so do those who message it.
    let role = |agent: &str, role: &str| {
        client_message::Body::UpdateAgent(v1::UpdateAgent { agent_id: agent.into(), role: Some(role.into()), ..Default::default() })
    };
    ack(apply_request(&mut client, &mut view, role("mike", "Lead")).await);
    assert_eq!(view.agent("mike").role, "Lead");
    let told = |view: &View, text: &str| {
        view.entries
            .iter()
            .any(|entry| matches!(&entry.kind, Some(Kind::Event(event)) if event.from == "Claktory" && event.text.contains(text)))
    };
    call(&mut client, &mut view, "mike", "agents_status", "{}").await;
    assert!(told(&view, "your role is now Lead"), "{:?}", describe(&view.entries));
    assert!(system_prompt.lock().expect("lock").contains("Your role: Lead."), "a new role is a new system prompt");
    call(&mut client, &mut view, "ana", "agents_status", "{}").await;
    assert!(told(&view, "You can now message: Mike (mike, Lead)."), "{:?}", describe(&view.entries));

    // Cleared, it's left out of the settings file.
    ack(apply_request(&mut client, &mut view, role("mike", "")).await);
    assert_eq!(view.agent("mike").role, "");
    assert!(config(&dir)["agents"][1].get("role").is_none());
}

#[tokio::test]
async fn one_message_reaches_several_agents_each_told_who_else_got_it() {
    let dir = temp_dir();
    let (addr, _factory) = start(&dir).await;
    let (mut client, workspace, _) = Client::connect(addr).await;
    let mut view = View { workspace, ..View::default() };
    let rivet = v1::UpdateAgent { agent_id: "rivet".into(), model: Some(MODEL.into()), effort: Some("off".into()), ..Default::default() };
    apply_request(&mut client, &mut view, client_message::Body::UpdateAgent(rivet)).await;
    apply_request(&mut client, &mut view, create_project("Site", &dir.join("site"))).await;
    apply_request(&mut client, &mut view, create_project("Lab", &dir.join("lab"))).await;
    let hire = |project: &str, name: &str, role: &str| {
        let client_message::Body::CreateAgent(mut agent) = create_agent(project, name, "") else { unreachable!() };
        agent.role = role.into();
        client_message::Body::CreateAgent(agent)
    };
    for (project, name, role) in
        [("site", "Mike", "Coordinator"), ("site", "Ana", "Frontend"), ("site", "Sam", "Backend"), ("lab", "Zoe", "Researcher")]
    {
        ack(apply_request(&mut client, &mut view, hire(project, name, role)).await);
    }
    // The tool's answer and its details.
    async fn send(
        client: &mut Client,
        view: &mut View,
        agent: &str,
        args: serde_json::Value,
    ) -> (String, serde_json::Value, v1::ToolCallState) {
        apply_request(client, view, select(agent)).await;
        let before = view.entries.len();
        apply_request(client, view, send_message(agent, &format!("call agents_send {args}"))).await;
        view.follow(client, |view| view.entries.len() >= before + 3 && view.agent(agent).status() == AgentStatus::Idle).await;
        let call = view.entries[before..]
            .iter()
            .find_map(|entry| match &entry.kind {
                Some(Kind::ToolCall(call)) => Some(call.clone()),
                _ => None,
            })
            .expect("a tool call");
        let details = if call.details.is_empty() { serde_json::Value::Null } else { serde_json::from_str(&call.details).expect("JSON") };
        (call.output.clone(), details, call.state())
    }
    async fn received(client: &mut Client, view: &mut View, agent: &str, starts: &str) -> String {
        view.follow(client, |view| idle(view, agent)).await;
        apply_request(client, view, select(agent)).await;
        view.entries
            .iter()
            .rev()
            .find_map(|entry| match &entry.kind {
                Some(Kind::Event(event)) if event.text.starts_with(starts) => Some(event.text.clone()),
                _ => None,
            })
            .unwrap_or_else(|| panic!("{agent} got no message starting {starts:?}"))
    }

    // Mike is busy; Ana sends to him, Sam, herself, Mike again by role, and
    // someone who isn't there. She and the repeat are left out, the one
    // missing is said, and the others get it.
    apply_request(&mut client, &mut view, send_message("mike", r#"call bash {"command": "sleep 4"}"#)).await;
    let to = serde_json::json!({"to": ["mike", "Sam", "ana", "Coordinator", "Nobody"], "text": "Standup at 10"});
    let (said, details, state) = send(&mut client, &mut view, "ana", to).await;
    assert_eq!(state, v1::ToolCallState::Done);
    assert_eq!(
        said,
        "Mike (mike): sent; it's working and reads it after its current step.\nSam (sam): sent.\nNobody: No agent you can message is called \"Nobody\"; you can message: mike, sam."
    );
    assert_eq!(
        details,
        serde_json::json!({"text": "Standup at 10", "recipients": [
            {"to": "mike", "id": "mike", "name": "Mike", "role": "Coordinator", "outcome": "working"},
            {"to": "Sam", "id": "sam", "name": "Sam", "role": "Backend", "outcome": "sent"},
            {"to": "Nobody", "id": "", "name": "", "role": "", "outcome": "failed",
             "why": "No agent you can message is called \"Nobody\"; you can message: mike, sam."},
        ]})
    );
    assert_eq!(received(&mut client, &mut view, "mike", "Standup").await, "Standup at 10\n\nAlso sent to: Sam (Backend).");
    assert_eq!(received(&mut client, &mut view, "sam", "Standup").await, "Standup at 10\n\nAlso sent to: Mike (Coordinator).");

    // Others are named as the recipient knows them: from another project, with it.
    let to = serde_json::json!({"to": ["mike", "zoe"], "text": "Plans?"});
    assert_eq!(send(&mut client, &mut view, "rivet", to).await.0, "Mike (mike): sent.\nZoe (zoe): sent.");
    assert_eq!(received(&mut client, &mut view, "zoe", "Plans").await, "Plans?\n\nAlso sent to: Mike (Coordinator, Site).");
    // A list of one is a plain message.
    let to = serde_json::json!({"to": ["sam"], "text": "Just you"});
    assert_eq!(send(&mut client, &mut view, "ana", to).await.0, "Sam (sam): sent.");
    assert_eq!(received(&mut client, &mut view, "sam", "Just").await, "Just you");

    // An agent whose tools are from before lists writes one as a string.
    let to = serde_json::json!({"to": r#"["mike", "sam"]"#, "text": "Old schema"});
    assert_eq!(send(&mut client, &mut view, "ana", to).await.0, "Mike (mike): sent.\nSam (sam): sent.");
    assert_eq!(received(&mut client, &mut view, "sam", "Old").await, "Old schema\n\nAlso sent to: Mike (Coordinator).");

    // Reaching no one fails, with why; so does too long a list.
    let to = serde_json::json!({"to": ["ana", "Nobody", "zoe"], "text": "Hello?"});
    let (said, details, state) = send(&mut client, &mut view, "ana", to).await;
    assert_eq!(state, v1::ToolCallState::Failed);
    assert!(said.starts_with("Sent to no one. No agent you can message is called \"Nobody\""), "{said}");
    assert!(said.contains("called \"zoe\""), "{said}");
    assert_eq!(details, serde_json::Value::Null);
    let many: Vec<String> = (0..21).map(|n| format!("agent-{n}")).collect();
    let (said, _, state) = send(&mut client, &mut view, "ana", serde_json::json!({"to": many, "text": "Hi"})).await;
    assert_eq!(state, v1::ToolCallState::Failed);
    assert!(said.contains("to: must not have more than 20 items"), "{said}");
}
