//! Leads: an agent reports to another (its lead), in its project or an
//! admin agent, which can then read how it's set up with agents_inspect,
//! read only. Admin agents lead everyone.

use crate::common::{Client, MODEL, Scripted, View, apply_request, create_agent, create_project, send_message, start_with, temp_dir};
use factory_protocol::v1::{self, AgentStatus, client_message, entry::Kind, server_message::Body};

fn select(agent: &str) -> client_message::Body {
    client_message::Body::SelectAgent(v1::SelectAgent { agent_id: agent.into() })
}

fn idle(view: &View, agent: &str) -> bool {
    let agent = view.agent(agent);
    agent.status() == AgentStatus::Idle && agent.queued_messages == 0
}

fn lead(agent: &str, lead: &str) -> client_message::Body {
    client_message::Body::UpdateAgent(v1::UpdateAgent { agent_id: agent.into(), lead_id: Some(lead.into()), ..Default::default() })
}

fn refused(reply: Body) -> String {
    match reply {
        Body::Error(error) => {
            assert_eq!(error.code(), v1::ErrorCode::InvalidRequest);
            error.message
        }
        other => panic!("expected an error, got {other:?}"),
    }
}

fn ack(reply: Body) {
    assert!(matches!(reply, Body::Ack(_)), "expected ack, got {reply:?}");
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

fn config(dir: &std::path::Path) -> serde_json::Value {
    serde_json::from_str(&std::fs::read_to_string(dir.join("home/config.json")).expect("config.json")).expect("json")
}

fn lead_in_config(dir: &std::path::Path, agent: &str) -> Option<String> {
    let config = config(dir);
    let agents = config["agents"].as_array().expect("agents").clone();
    let found = agents.into_iter().find(|found| found["id"] == agent).expect("agent");
    found.get("lead").and_then(|lead| lead.as_str()).map(str::to_owned)
}

/// (from, to) of the Map's lead edges.
fn lead_edges(view: &View) -> Vec<(String, String)> {
    let map = view.workspace.map.clone().expect("map");
    map.edges
        .iter()
        .filter(|edge| edge.kind() == v1::MapEdgeKind::Lead)
        .map(|edge| (edge.from_id.trim_start_matches("agent:").to_owned(), edge.to_id.trim_start_matches("agent:").to_owned()))
        .collect()
}

#[tokio::test]
async fn a_lead_sees_how_its_team_is_set_up_and_no_one_else_does() {
    let dir = temp_dir();
    let models = Scripted::signed_in();
    let system_prompt = models.system_prompt.clone();
    let (addr, _factory) = start_with(&dir, models).await;
    let (mut client, workspace, _) = Client::connect(addr).await;
    let mut view = View { workspace, ..View::default() };
    apply_request(&mut client, &mut view, create_project("Site", &dir.join("site"))).await;
    apply_request(&mut client, &mut view, create_project("Lab", &dir.join("lab"))).await;
    for (project, name, prompt) in [("site", "Mike", ""), ("site", "Sam", "Build the backend."), ("site", "Lena", ""), ("lab", "Scout", "")]
    {
        apply_request(&mut client, &mut view, create_agent(project, name, prompt)).await;
    }
    let rivet = v1::UpdateAgent { agent_id: "rivet".into(), model: Some(MODEL.into()), effort: Some("off".into()), ..Default::default() };
    apply_request(&mut client, &mut view, client_message::Body::UpdateAgent(rivet)).await;

    // Sam reports to Mike, Lena to Sam; Mike to Rivet, an admin agent in another project.
    ack(apply_request(&mut client, &mut view, lead("sam", "mike")).await);
    ack(apply_request(&mut client, &mut view, lead("lena", " sam ")).await);
    ack(apply_request(&mut client, &mut view, lead("mike", "rivet")).await);
    assert_eq!(view.agent("sam").lead_id, "mike");
    assert_eq!(view.agent("lena").lead_id, "sam");
    assert_eq!(view.agent("mike").lead_id, "rivet");
    assert_eq!(view.agent("scout").lead_id, "");
    assert_eq!(lead_in_config(&dir, "lena").as_deref(), Some("sam"));
    let mut edges = lead_edges(&view);
    edges.sort();
    assert_eq!(edges, [("lena".into(), "sam".into()), ("mike".into(), "rivet".into()), ("sam".into(), "mike".into())]);

    // Leads that can't be: in another project (not an admin), itself, a loop, no one.
    assert_eq!(
        refused(apply_request(&mut client, &mut view, lead("sam", "scout")).await),
        "Scout is in another project; a lead is in the same project, or an admin agent"
    );
    assert_eq!(refused(apply_request(&mut client, &mut view, lead("sam", "sam")).await), "Sam can't lead itself");
    assert_eq!(refused(apply_request(&mut client, &mut view, lead("mike", "lena")).await), "Lena reports to Mike, so it can't lead it");
    assert_eq!(refused(apply_request(&mut client, &mut view, lead("mike", "nobody")).await), "no agent with id \"nobody\"");
    assert_eq!(view.agent("mike").lead_id, "rivet", "unchanged");

    // Mike's team: itself, Sam, and Lena through Sam.
    let team = call(&mut client, &mut view, "mike", "agents_inspect", "{}").await;
    let ids: Vec<&str> = team.lines().map(|line| line.split(' ').next().expect("id")).collect();
    assert_eq!(ids, ["mike", "sam", "lena"], "{team}");
    assert!(team.lines().nth(2).expect("lena").contains("; led by sam;"), "{team}");
    assert_eq!(call(&mut client, &mut view, "mike", "agents_inspect", r#"{"agent":"team"}"#).await, team);

    // How Lena is set up, through Sam; nothing changes.
    let before = config(&dir);
    let lena = call(&mut client, &mut view, "mike", "agents_inspect", r#"{"agent":"Lena"}"#).await;
    assert!(lena.starts_with("Lena (lena)\nProject: Site ("), "{lena}");
    for line in [
        &format!("Model: {MODEL}, effort off"),
        "Lead: Sam (sam)",
        "Now: idle",
        "- Messaging (agents): on; reachable on, admin off, see_map off, also []",
        "- Checklist (checklist): off",
        "Secrets: none",
        "Monitors: none",
        "Prompt: none",
    ] {
        assert!(lena.contains(line), "{line} in {lena}");
    }
    assert_eq!(config(&dir), before, "inspecting changes nothing");
    let sam = call(&mut client, &mut view, "mike", "agents_inspect", r#"{"agent":"sam"}"#).await;
    assert!(sam.ends_with("Prompt:\nBuild the backend."), "{sam}");

    // Not up the chain, nor sideways, nor across projects.
    let no = call(&mut client, &mut view, "sam", "agents_inspect", r#"{"agent":"mike"}"#).await;
    assert_eq!(no, "Mike isn't on your team: you can see how an agent is set up only when you lead it, directly or through others.");
    let no = call(&mut client, &mut view, "scout", "agents_inspect", r#"{"agent":"lena"}"#).await;
    assert!(no.starts_with("Lena isn't on your team"), "{no}");
    let alone = call(&mut client, &mut view, "scout", "agents_inspect", "{}").await;
    assert!(alone.ends_with("No agent reports to you, so you can inspect only yourself."), "{alone}");
    assert_eq!(
        call(&mut client, &mut view, "sam", "agents_inspect", r#"{"agent":"nobody"}"#).await,
        "No agent has the id or name \"nobody\"."
    );

    // The admin agent leads everyone, in every project.
    let scout = call(&mut client, &mut view, "rivet", "agents_inspect", r#"{"agent":"scout"}"#).await;
    assert!(scout.starts_with("Scout (scout)\nProject: Lab ("), "{scout}");
    let everyone = call(&mut client, &mut view, "rivet", "agents_inspect", "{}").await;
    assert_eq!(everyone.lines().count(), 5, "{everyone}");

    // A secret by name and variables, a monitor by name and schedule.
    let secret = v1::CreateSecret {
        name: "Cloudflare".into(),
        description: "DNS for the site".into(),
        entries: vec![v1::SecretEntry { variable: "CF_TOKEN".into(), value: "cf-SECRETVALUE".into(), file: false }],
    };
    apply_request(&mut client, &mut view, client_message::Body::CreateSecret(secret)).await;
    let id = view.workspace.vault.clone().expect("vault").secrets[0].id.clone();
    let grant = v1::GrantSecret { id, agent_id: "lena".into(), proof: None };
    apply_request(&mut client, &mut view, client_message::Body::GrantSecret(grant)).await;
    let added = call(
        &mut client,
        &mut view,
        "lena",
        "monitors_add",
        r#"{"monitors":[{"name":"deploy check","prompt":"Look at the deploy.","when":{"in":"2h"}}]}"#,
    )
    .await;
    assert!(added.starts_with("Added m1"), "{added}");
    let lena = call(&mut client, &mut view, "mike", "agents_inspect", r#"{"agent":"lena"}"#).await;
    assert!(lena.contains("Secrets:\n- Cloudflare (sets CF_TOKEN): DNS for the site\n"), "{lena}");
    assert!(!lena.contains("cf-SECRETVALUE"));
    assert!(lena.contains("Monitors:\n- m1 deploy check — "), "{lena}");

    // agents_status says who leads whom.
    let status = call(&mut client, &mut view, "mike", "agents_status", "{}").await;
    assert!(status.lines().any(|line| line.starts_with("lena Lena: idle") && line.ends_with("; led by sam")), "{status}");

    // Their system prompts say so, from their next context.
    for (agent, says) in [
        ("mike", "You lead Sam (sam), Lena (lena): agents_inspect shows how they're set up, read only."),
        ("lena", "You report to Sam (sam), which can see how you're set up."),
        ("rivet", "You lead every agent: agents_inspect shows how any of them is set up, read only."),
    ] {
        let clear = v1::RunCommand { agent_id: agent.into(), name: "clear".into(), args: String::new() };
        ack(apply_request(&mut client, &mut view, client_message::Body::RunCommand(clear)).await);
        call(&mut client, &mut view, agent, "agents_status", "{}").await;
        let prompt = system_prompt.lock().expect("lock").clone();
        assert!(prompt.contains(says), "{agent}: {says} in {prompt}");
    }

    // None takes the lead away.
    ack(apply_request(&mut client, &mut view, lead("mike", "")).await);
    assert_eq!(view.agent("mike").lead_id, "");
    assert_eq!(lead_in_config(&dir, "mike"), None);
}

#[tokio::test]
async fn leads_that_no_longer_fit_go_when_agents_move_or_are_deleted() {
    let dir = temp_dir();
    let (addr, _factory) = start_with(&dir, Scripted::signed_in()).await;
    let (mut client, workspace, _) = Client::connect(addr).await;
    let mut view = View { workspace, ..View::default() };
    apply_request(&mut client, &mut view, create_project("Site", &dir.join("site"))).await;
    apply_request(&mut client, &mut view, create_project("Lab", &dir.join("lab"))).await;
    for name in ["Mike", "Sam", "Lena"] {
        apply_request(&mut client, &mut view, create_agent("site", name, "")).await;
    }
    // Created with a lead.
    let mut omar = create_agent("site", "Omar", "");
    if let client_message::Body::CreateAgent(create) = &mut omar {
        create.lead_id = "mike".into();
    }
    ack(apply_request(&mut client, &mut view, omar).await);
    assert_eq!(view.agent("omar").lead_id, "mike");
    let mut bad = create_agent("lab", "Scout", "");
    if let client_message::Body::CreateAgent(create) = &mut bad {
        create.lead_id = "mike".into();
    }
    refused(apply_request(&mut client, &mut view, bad).await);
    assert!(view.workspace.agents.iter().all(|agent| agent.id != "scout"), "not created");

    ack(apply_request(&mut client, &mut view, lead("sam", "mike")).await);
    ack(apply_request(&mut client, &mut view, lead("lena", "sam")).await);

    // Sam moves to Lab: its lead stays in Site, and Lena's lead left it.
    let moved = v1::UpdateAgent { agent_id: "sam".into(), project_id: Some("lab".into()), ..Default::default() };
    ack(apply_request(&mut client, &mut view, client_message::Body::UpdateAgent(moved)).await);
    assert_eq!((view.agent("sam").lead_id.as_str(), view.agent("lena").lead_id.as_str()), ("", ""));
    assert_eq!((lead_in_config(&dir, "sam"), lead_in_config(&dir, "lena")), (None, None));
    assert_eq!(view.agent("omar").lead_id, "mike", "the rest stay");

    // Mike is deleted: no one reports to it any more.
    ack(apply_request(&mut client, &mut view, client_message::Body::DeleteAgent(v1::DeleteAgent { agent_id: "mike".into() })).await);
    assert_eq!(view.agent("omar").lead_id, "");
    assert_eq!(lead_in_config(&dir, "omar"), None);
}

#[tokio::test]
async fn a_lead_written_by_hand_that_cant_be_is_ignored_and_kept() {
    let dir = temp_dir();
    let (addr, _factory) = start_with(&dir, Scripted::signed_in()).await;
    let (mut client, workspace, _) = Client::connect(addr).await;
    let mut view = View { workspace, ..View::default() };
    apply_request(&mut client, &mut view, create_project("Site", &dir.join("site"))).await;
    for name in ["Mike", "Sam", "Lena"] {
        apply_request(&mut client, &mut view, create_agent("site", name, "")).await;
    }
    // A loop between Mike and Sam, and Lena under Sam, as someone might write it.
    let path = dir.join("home/config.json");
    let mut config = config(&dir);
    for agent in config["agents"].as_array_mut().expect("agents") {
        let lead = match agent["id"].as_str() {
            Some("mike") => "sam",
            Some("sam") => "mike",
            Some("lena") => "sam",
            _ => continue,
        };
        agent["lead"] = lead.into();
    }
    std::fs::write(&path, serde_json::to_string_pretty(&config).expect("json")).expect("write");
    view.follow(&mut client, |view| view.agent("lena").lead_id == "sam").await;
    assert_eq!((view.agent("mike").lead_id.as_str(), view.agent("sam").lead_id.as_str()), ("", ""), "a loop leads nowhere");
    assert_eq!(lead_edges(&view), [("lena".to_owned(), "sam".to_owned())]);
    // Neither leads the other, and the file keeps what was written.
    let no = call(&mut client, &mut view, "mike", "agents_inspect", r#"{"agent":"sam"}"#).await;
    assert!(no.starts_with("Sam isn't on your team"), "{no}");
    assert_eq!(lead_in_config(&dir, "mike").as_deref(), Some("sam"));
}
