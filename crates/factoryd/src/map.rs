//! The Map: how this Claktory is arranged, as a graph the app lays out.
//! Where agents are (workspace, projects, folders, nesting), whom they can
//! message, which secrets they have, what data they read and write, and
//! their extensions that grant reach or power. It's computed here from the
//! same rules agents follow (`can_message`, `is_admin`, `enabled`), so the
//! app never repeats them. Sent with every Workspace, so it follows every
//! change to the settings and the vault.

use std::path::Path;

use factory_protocol::v1;

use crate::config::{AgentConfig, Config};
use crate::extensions::agents::can_message;
use crate::extensions::{enabled, switch};
use crate::home::Home;
use crate::proposals::is_admin;

const WORKSPACE: &str = "workspace";
const RECORDS: &str = "data:records";

fn project_node(id: &str) -> String {
    format!("project:{id}")
}

fn folder_node(project: &str, folder: &str) -> String {
    format!("folder:{project}/{folder}")
}

fn agent_node(id: &str) -> String {
    format!("agent:{id}")
}

fn database_node(agent: &str) -> String {
    format!("data:agent:{agent}")
}

/// An agent's extensions that grant reach or power, in this order: key, label, icon.
fn badges(agent: &AgentConfig) -> Vec<v1::MapBadge> {
    let admin = is_admin(agent);
    let tasks = enabled(agent, "tasks");
    let has = [
        ("agents:admin", "Messages every agent", "users", admin),
        ("secrets:give", "Gives secrets to agents", "key-round", admin && enabled(agent, "secrets")),
        ("tasks:see_all", "Stops any agent's tasks", "terminal", tasks && (admin || switch(agent, "tasks", "see_all"))),
        ("web", "Web", "globe", enabled(agent, "web")),
        ("tree", "Own checkout", "git-branch", enabled(agent, "tree")),
        ("database", "Database", "database", enabled(agent, "database")),
        ("database:factory", "Claktory's records", "database", enabled(agent, "database:factory")),
        ("memory:factory", "Claktory manual", "brain", enabled(agent, "memory:factory")),
    ];
    has.into_iter()
        .filter(|(.., on)| *on)
        .map(|(key, label, icon, _)| v1::MapBadge { key: key.to_owned(), label: label.to_owned(), icon: icon.to_owned() })
        .collect()
}

fn path(path: &Path) -> String {
    path.display().to_string()
}

fn edge(from: &str, to: &str, kind: v1::MapEdgeKind) -> v1::MapEdge {
    v1::MapEdge { from_id: from.to_owned(), to_id: to.to_owned(), kind: kind.into(), both_ways: false }
}

/// The map of `config`, with the secrets in `vault` (names and grants only).
pub(crate) fn build(config: &Config, home: &Home, vault: &v1::VaultState) -> v1::ClaktoryMap {
    let mut nodes = vec![v1::MapNode {
        id: WORKSPACE.to_owned(),
        kind: v1::MapNodeKind::Workspace.into(),
        label: "Workspace".to_owned(),
        detail: path(&home.workspace()),
        ..Default::default()
    }];
    let mut edges = Vec::new();
    for project in &config.projects {
        nodes.push(v1::MapNode {
            id: project_node(&project.id),
            kind: v1::MapNodeKind::Project.into(),
            label: project.name.clone(),
            detail: path(&project.path),
            ref_id: project.id.clone(),
            ..Default::default()
        });
        for folder in &project.folders {
            nodes.push(v1::MapNode {
                id: folder_node(&project.id, &folder.id),
                kind: v1::MapNodeKind::Folder.into(),
                label: folder.name.clone(),
                detail: path(&folder.path),
                parent_id: project_node(&project.id),
                ref_id: folder.id.clone(),
                project_id: project.id.clone(),
                ..Default::default()
            });
        }
    }
    let mut records = false;
    for agent in &config.agents {
        let id = agent_node(&agent.id);
        let project = agent.project.as_deref().and_then(|project| config.project(project));
        let folder = config.agent_folder(agent);
        // Where it works, which it writes; the rest of its project it reads.
        let place = match (project, folder) {
            (Some(project), Some(folder)) => folder_node(&project.id, &folder.id),
            (Some(project), None) => project_node(&project.id),
            (None, _) => WORKSPACE.to_owned(),
        };
        let admin = is_admin(agent);
        let parent =
            if admin { String::new() } else { crate::arrange::parent_of(config, &agent.id).map_or_else(|| place.clone(), agent_node) };
        let project_id = project.map(|project| project.id.clone()).unwrap_or_default();
        nodes.push(v1::MapNode {
            id: id.clone(),
            kind: v1::MapNodeKind::Agent.into(),
            label: agent.name.clone(),
            detail: agent.role.clone(),
            parent_id: parent,
            admin,
            color: agent.color.clone(),
            badges: badges(agent),
            ref_id: agent.id.clone(),
            project_id: project_id.clone(),
        });
        edges.push(edge(&id, &place, v1::MapEdgeKind::Writes));
        if let Some(project) = project {
            let others = std::iter::once(project_node(&project.id))
                .chain(project.folders.iter().map(|folder| folder_node(&project.id, &folder.id)))
                .filter(|other| *other != place);
            edges.extend(others.map(|other| edge(&id, &other, v1::MapEdgeKind::Reads)));
        }
        if enabled(agent, "database") {
            let database = database_node(&agent.id);
            nodes.push(v1::MapNode {
                id: database.clone(),
                kind: v1::MapNodeKind::Data.into(),
                label: format!("{}'s database", agent.name),
                detail: path(&home.agent_dir(&agent.id).join("database.sqlite")),
                parent_id: id.clone(),
                ref_id: agent.id.clone(),
                project_id,
                ..Default::default()
            });
            edges.push(edge(&id, &database, v1::MapEdgeKind::Writes));
        }
        if enabled(agent, "database:factory") {
            records = true;
            edges.push(edge(&id, RECORDS, v1::MapEdgeKind::Reads));
        }
        for to in &config.agents {
            if !can_message(agent, to) {
                continue;
            }
            let back = can_message(to, agent);
            // A pair that messages both ways is one edge, from the lower id.
            if back && to.id < agent.id {
                continue;
            }
            edges.push(v1::MapEdge { both_ways: back, ..edge(&id, &agent_node(&to.id), v1::MapEdgeKind::Messages) });
        }
    }
    if records {
        nodes.push(v1::MapNode {
            id: RECORDS.to_owned(),
            kind: v1::MapNodeKind::Data.into(),
            label: "Claktory's records".to_owned(),
            detail: path(&home.db_path()),
            ..Default::default()
        });
    }
    for secret in &vault.secrets {
        let id = format!("secret:{}", secret.id);
        nodes.push(v1::MapNode {
            id: id.clone(),
            kind: v1::MapNodeKind::Secret.into(),
            label: secret.name.clone(),
            detail: secret.description.clone(),
            ref_id: secret.id.clone(),
            ..Default::default()
        });
        let holders = secret.grants.iter().filter(|grant| config.agent(&grant.agent_id).is_some());
        edges.extend(holders.map(|grant| edge(&id, &agent_node(&grant.agent_id), v1::MapEdgeKind::Secret)));
    }
    nodes.sort_by(|a, b| a.id.cmp(&b.id));
    edges.sort_by(|a, b| (&a.from_id, &a.to_id, a.kind).cmp(&(&b.from_id, &b.to_id, b.kind)));
    edges.dedup();
    v1::ClaktoryMap { nodes, edges }
}
