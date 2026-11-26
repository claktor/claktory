//! The Map: Workspace.map, how this Claktory is arranged, computed by the
//! backend from the same rules agents follow.

use serde_json::{Value, json};

use crate::common::{Client, MODEL, last_workspace, start, temp_dir};
use factory_protocol::v1::{self, MapEdgeKind, MapNodeKind, client_message::Body as Request};
use factoryd::config::{AgentConfig, Config, FolderConfig, ProjectConfig};

fn agent(rivet: &AgentConfig, id: &str, project: &str, folder: &str, role: &str, extensions: Value) -> AgentConfig {
    let mut name = id.to_owned();
    name[..1].make_ascii_uppercase();
    AgentConfig {
        id: id.into(),
        project: Some(project.into()),
        parent: None,
        name,
        role: role.into(),
        prompt: String::new(),
        folder: folder.into(),
        placeholder: String::new(),
        color: String::new(),
        extensions: serde_json::from_value(extensions).expect("extensions"),
        ..rivet.clone()
    }
}

/// A Claktory like Matt's: Rivet, a project with folders (one agent nested
/// under another, one nobody can message), and a second project whose
/// agents reach into the first through `also` lists.
fn config(dir: &std::path::Path) -> Config {
    let mut config = Config::default();
    config.agents[0].model = MODEL.into();
    let rivet = config.agents[0].clone();
    let folder = |id: &str, name: &str| FolderConfig { id: id.into(), name: name.into(), path: dir.join(id), other: Default::default() };
    config.projects = vec![
        ProjectConfig {
            id: "factory".into(),
            name: "Claktory".into(),
            path: dir.join("factory"),
            folders: vec![folder("app", "App"), folder("research", "Research")],
            archived: false,
            other: Default::default(),
        },
        ProjectConfig {
            id: "site".into(),
            name: "Site".into(),
            path: dir.join("site"),
            folders: Vec::new(),
            archived: false,
            other: Default::default(),
        },
    ];
    let mut lena = agent(&rivet, "lena", "factory", "app", "Backend", json!({}));
    lena.lead = Some("sam".into());
    config.agents.extend([
        agent(&rivet, "mike", "factory", "", "Coordinator", json!({"agents": {"also": ["ana"]}})),
        agent(&rivet, "sam", "factory", "app", "Backend", json!({"database": {"enabled": true}, "web": {"enabled": false}})),
        lena,
        agent(&rivet, "iris", "factory", "research", "", json!({"agents": {"reachable": false}, "tree": {"enabled": false}})),
        agent(&rivet, "ana", "site", "", "Frontend", json!({"agents": {"also": ["mike"]}})),
        agent(&rivet, "scout", "site", "", "", json!({"agents": {"also": ["sam"]}, "tasks": {"see_all": true}})),
    ]);
    config
}

fn node<'a>(map: &'a v1::ClaktoryMap, id: &str) -> &'a v1::MapNode {
    map.nodes.iter().find(|node| node.id == id).unwrap_or_else(|| panic!("no node {id}"))
}

fn badges(map: &v1::ClaktoryMap, id: &str) -> Vec<String> {
    node(map, id).badges.iter().map(|badge| badge.key.clone()).collect()
}

/// Its edges of a kind, as (from, to, both ways).
fn edges(map: &v1::ClaktoryMap, kind: MapEdgeKind) -> Vec<(String, String, bool)> {
    map.edges.iter().filter(|edge| edge.kind() == kind).map(|edge| (edge.from_id.clone(), edge.to_id.clone(), edge.both_ways)).collect()
}

fn of(edges: &[(String, String, bool)], from: &str) -> Vec<(String, bool)> {
    edges.iter().filter(|(edge_from, ..)| edge_from == from).map(|(_, to, both)| (to.clone(), *both)).collect()
}

fn to(ids: &[&str]) -> Vec<(String, bool)> {
    ids.iter().map(|id| ((*id).to_owned(), false)).collect()
}

#[tokio::test]
async fn the_map_shows_where_agents_are_whom_they_message_and_what_they_reach() {
    let dir = temp_dir();
    std::fs::create_dir_all(dir.join("home")).expect("home");
    std::fs::write(dir.join("home/config.json"), config(&dir).to_json(&dir.join("home")).expect("json")).expect("config");
    let (addr, _factory) = start(&dir).await;
    let (mut client, workspace, _) = Client::connect(addr).await;
    let map = workspace.map.clone().expect("a map");

    let ids: Vec<&str> = map.nodes.iter().map(|node| node.id.as_str()).collect();
    let mut sorted = ids.clone();
    sorted.sort_unstable();
    assert_eq!(ids, sorted, "sorted by id");
    assert_eq!(
        ids,
        [
            "agent:ana",
            "agent:iris",
            "agent:lena",
            "agent:mike",
            "agent:rivet",
            "agent:sam",
            "agent:scout",
            "data:agent:sam",
            "data:records",
            "folder:factory/app",
            "folder:factory/research",
            "project:factory",
            "project:site",
            "workspace",
        ]
    );

    // The hierarchy, as in the sidebar, with the admin agent at the top.
    let parent = |id: &str| node(&map, id).parent_id.clone();
    assert_eq!(parent("agent:rivet"), "");
    assert!(node(&map, "agent:rivet").admin && !node(&map, "agent:mike").admin);
    assert_eq!(parent("agent:mike"), "project:factory");
    assert_eq!(parent("agent:sam"), "folder:factory/app");
    assert_eq!(parent("agent:lena"), "agent:sam", "nested under Sam, its lead");
    assert_eq!(parent("agent:iris"), "folder:factory/research");
    assert_eq!(parent("agent:ana"), "project:site");
    assert_eq!(parent("folder:factory/app"), "project:factory");
    assert_eq!(parent("data:agent:sam"), "agent:sam");
    for top in ["project:factory", "project:site", "workspace", "data:records"] {
        assert_eq!(parent(top), "", "{top}");
    }
    let sam = node(&map, "agent:sam");
    assert_eq!(
        (sam.kind(), sam.label.as_str(), sam.detail.as_str(), sam.ref_id.as_str(), sam.project_id.as_str()),
        (MapNodeKind::Agent, "Sam", "Backend", "sam", "factory")
    );
    assert_eq!(sam.color, workspace.agents.iter().find(|agent| agent.id == "sam").expect("sam").color);
    let app = node(&map, "folder:factory/app");
    assert_eq!((app.kind(), app.label.as_str(), app.detail.clone()), (MapNodeKind::Folder, "App", dir.join("app").display().to_string()));
    assert_eq!(node(&map, "data:agent:sam").label, "Sam's database");
    assert_eq!(node(&map, "data:records").label, "Claktory's records");

    // Extensions that grant reach or power.
    assert_eq!(
        badges(&map, "agent:rivet"),
        ["agents:admin", "secrets:give", "tasks:see_all", "web", "tree", "database:factory", "memory:factory"]
    );
    assert_eq!(badges(&map, "agent:sam"), ["tree", "database"]);
    assert_eq!(badges(&map, "agent:iris"), ["web"]);
    assert_eq!(badges(&map, "agent:scout"), ["tasks:see_all", "web", "tree"]);

    // Who can message whom: Rivet everyone it can reach, one way (nobody
    // messages Rivet); a project's agents each other, both ways, as one
    // edge; `also` lists across projects; Iris nobody can message.
    let messages = edges(&map, MapEdgeKind::Messages);
    assert_eq!(of(&messages, "agent:rivet"), to(&["agent:ana", "agent:lena", "agent:mike", "agent:sam", "agent:scout"]));
    assert_eq!(of(&messages, "agent:ana"), [("agent:mike".to_owned(), true), ("agent:scout".to_owned(), true)]);
    assert_eq!(of(&messages, "agent:lena"), [("agent:mike".to_owned(), true), ("agent:sam".to_owned(), true)]);
    assert_eq!(of(&messages, "agent:mike"), [("agent:sam".to_owned(), true)]);
    assert_eq!(of(&messages, "agent:scout"), to(&["agent:sam"]), "Sam can't message back");
    assert_eq!(of(&messages, "agent:iris"), to(&["agent:lena", "agent:mike", "agent:sam"]));
    assert!(messages.iter().all(|(_, to, _)| to != "agent:iris" && to != "agent:rivet"));

    // Where agents write, and what else of their project they read.
    let writes = edges(&map, MapEdgeKind::Writes);
    assert_eq!(of(&writes, "agent:sam"), to(&["data:agent:sam", "folder:factory/app"]));
    assert_eq!(of(&writes, "agent:mike"), to(&["project:factory"]));
    assert_eq!(of(&writes, "agent:rivet"), to(&["workspace"]));
    let reads = edges(&map, MapEdgeKind::Reads);
    assert_eq!(of(&reads, "agent:sam"), to(&["folder:factory/research", "project:factory"]));
    assert_eq!(of(&reads, "agent:mike"), to(&["folder:factory/app", "folder:factory/research"]));
    assert_eq!(of(&reads, "agent:rivet"), to(&["data:records"]));
    assert!(of(&reads, "agent:ana").is_empty());

    // A secret given to an agent shows at once, by name, never its value.
    let secret = v1::CreateSecret {
        name: "Cloudflare".into(),
        description: "The claktory.com zone.".into(),
        entries: vec![v1::SecretEntry { variable: "TOKEN".into(), value: "cf-SECRETVALUE".into(), file: false }],
        ..Default::default()
    };
    let events = client.ok(Request::CreateSecret(secret)).await;
    let map = last_workspace(&events).and_then(|workspace| workspace.map.clone()).expect("a new map");
    let secret = map.nodes.iter().find(|node| node.kind() == MapNodeKind::Secret).expect("the secret").clone();
    assert_eq!((secret.label.as_str(), secret.detail.as_str()), ("Cloudflare", "The claktory.com zone."));
    assert!(edges(&map, MapEdgeKind::Secret).is_empty(), "nobody has it yet");
    let grant = v1::GrantSecret { id: secret.ref_id.clone(), agent_id: "sam".into(), proof: None, fill_only: false };
    let events = client.request(Request::GrantSecret(grant)).await.events;
    let map = last_workspace(&events).and_then(|workspace| workspace.map.clone()).expect("a new map");
    assert_eq!(edges(&map, MapEdgeKind::Secret), [(format!("secret:{}", secret.ref_id), "agent:sam".to_owned(), false)]);
    assert!(!format!("{map:?}").contains("SECRETVALUE"));

    // The same Claktory gives the same map.
    let (_, again, _) = Client::connect(addr).await;
    assert_eq!(again.map, Some(map));
}
