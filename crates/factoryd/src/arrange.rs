//! How agents are arranged in the sidebar: within each project (or the
//! workspace) they come in config.json's order, and an agent is nested under
//! its lead (`lead`, #234) when the lead is in the same place. One led by an
//! admin agent elsewhere, such as Rivet, shows at the top level.
//!
//! The nesting is the hierarchy: dropping an agent onto another makes it
//! that agent's report. config.json's `parent`, which nested agents purely
//! visually before, is read and kept but no longer used, so an older
//! Claktory (a rollback) still shows what it did.

use crate::config::{AgentConfig, Config};
use crate::factory::RequestError;

/// A drop the app wouldn't make from what it shows: the list changed under
/// the drag (on another device, or Rivet moved an agent).
const CHANGED: &str = "The list changed while you were dragging. Try again.";

/// The agent `id` is nested under: its lead, when that's in its place.
pub(crate) fn parent_of<'a>(config: &'a Config, id: &str) -> Option<&'a str> {
    let agent = config.agent(id)?;
    let lead = config.lead(agent)?;
    (lead.project == agent.project).then_some(lead.id.as_str())
}

/// The agent and everything nested under it, in config order.
fn subtree(config: &Config, id: &str) -> Vec<String> {
    let mut members = vec![id.to_owned()];
    // Each pass adds the children of what's in already, until nothing new.
    loop {
        let before = members.len();
        for agent in &config.agents {
            if !members.contains(&agent.id)
                && let Some(parent) = parent_of(config, &agent.id)
                && members.iter().any(|member| member == parent)
            {
                members.push(agent.id.clone());
            }
        }
        if members.len() == before {
            break;
        }
    }
    let mut ordered: Vec<String> = config.agents.iter().map(|agent| agent.id.clone()).filter(|id| members.contains(id)).collect();
    // The agent itself first, so it keeps leading its subtree.
    ordered.retain(|member| member != id);
    ordered.insert(0, id.to_owned());
    ordered
}

/// Moves `id` (with everything nested under it) under `parent` (None for the
/// top level), before the sibling `before` (None for last), in `project` (None
/// to keep its project or take the parent's; Some("") for the workspace), and
/// into `folder` of that project (Some("") for the project's own folder; None
/// keeps each agent's folder, or the project's own when the project changes).
///
/// `parent` becomes its lead, as UpdateAgent's `lead_id` would; the top
/// level clears a lead in the same place, and keeps an admin agent's from
/// elsewhere. Its reports move with it, and keep reporting to it.
pub(crate) fn arrange(
    config: &mut Config,
    id: &str,
    parent: Option<&str>,
    before: Option<&str>,
    project: Option<&str>,
    folder: Option<&str>,
) -> Result<(), RequestError> {
    let agent = config.agent(id).ok_or_else(|| RequestError::UnknownAgent(id.to_owned()))?;
    let moving = subtree(config, id);
    let parent = match parent {
        Some(parent) => Some(config.agent(parent).ok_or_else(|| RequestError::UnknownAgent(parent.to_owned()))?),
        None => None,
    };
    if let Some(parent) = parent
        && moving.contains(&parent.id)
    {
        let why = if parent.id == agent.id {
            format!("{} can't report to itself.", agent.name)
        } else {
            format!("{} reports to {}, so {} can't report to {}.", parent.name, agent.name, agent.name, parent.name)
        };
        return Err(RequestError::Lead(why));
    }
    let destination: Option<String> = match (project, parent) {
        (Some(project), parent) => {
            let project = (!project.is_empty()).then(|| project.to_owned());
            if let Some(id) = &project
                && config.project(id).is_none()
            {
                return Err(RequestError::UnknownProject(id.clone()));
            }
            if let Some(parent) = parent
                && parent.project != project
            {
                return Err(RequestError::Arrange(CHANGED.to_owned()));
            }
            project
        }
        (None, Some(parent)) => parent.project.clone(),
        (None, None) => agent.project.clone(),
    };
    if let Some(parent) = parent {
        let placed = AgentConfig { project: destination.clone(), ..agent.clone() };
        if let Some(why) = config.lead_refused(&placed, &parent.id) {
            return Err(RequestError::Lead(why));
        }
    }
    // At the top level, a lead that would nest it there goes; an admin
    // agent's from elsewhere stays.
    let lead = match parent {
        Some(parent) => Some(parent.id.clone()),
        None => agent.lead.clone().filter(|lead| config.agent(lead).is_some_and(|lead| lead.project != destination)),
    };
    let parent = parent.map(|parent| parent.id.clone());
    if let Some(folder) = folder
        && !folder.is_empty()
        && destination.as_deref().and_then(|project| config.project(project)?.folder(folder)).is_none()
    {
        return Err(RequestError::NotItsFolder(folder.to_owned()));
    }
    if let Some(before) = before {
        let sibling = config.agent(before).ok_or_else(|| RequestError::UnknownAgent(before.to_owned()))?;
        if moving.iter().any(|member| member == before)
            || sibling.project != destination
            || parent_of(config, before).map(str::to_owned) != parent
        {
            return Err(RequestError::Arrange(CHANGED.to_owned()));
        }
    }

    // Take the subtree out, move it, and put it back in its place.
    let mut taken: Vec<AgentConfig> = Vec::new();
    for member in &moving {
        if let Some(index) = config.agents.iter().position(|agent| &agent.id == member) {
            taken.push(config.agents.remove(index));
        }
    }
    for agent in &mut taken {
        if agent.project != destination {
            agent.folder.clear();
        }
        if let Some(folder) = folder {
            folder.clone_into(&mut agent.folder);
        }
        agent.project = destination.clone();
    }
    taken[0].lead = lead;
    let index = match before {
        Some(before) => config.agents.iter().position(|agent| agent.id == before).unwrap_or(config.agents.len()),
        // Last: after the last agent already where it's going.
        None => config.agents.iter().rposition(|agent| agent.project == destination).map_or(config.agents.len(), |last| last + 1),
    };
    for (offset, agent) in taken.into_iter().enumerate() {
        config.agents.insert(index + offset, agent);
    }
    Ok(())
}

/// `folder` is one the agent's project has, or empty for the project's own.
pub(crate) fn check_folder(config: &Config, id: &str, folder: &str) -> Result<(), RequestError> {
    let agent = config.agent(id).ok_or_else(|| RequestError::UnknownAgent(id.to_owned()))?;
    if folder.is_empty() || agent.project.as_deref().and_then(|project| config.project(project)?.folder(folder)).is_some() {
        Ok(())
    } else {
        Err(RequestError::NotItsFolder(folder.to_owned()))
    }
}

/// An agent is about to go: those that report to it report to its own lead
/// instead, which led them through it already, so nothing more is granted.
pub(crate) fn lift_children(config: &mut Config, going: &str) {
    let grandparent = config.agent(going).and_then(|agent| config.lead(agent)).map(|lead| lead.id.clone());
    let reports: Vec<String> =
        config.agents.iter().filter(|agent| agent.lead.as_deref() == Some(going)).map(|agent| agent.id.clone()).collect();
    for id in reports {
        let lead = grandparent.clone().filter(|lead| {
            let agent = config.agent(&id);
            agent.is_some_and(|agent| lead != &agent.id && config.lead_refused(agent, lead).is_none())
        });
        if let Some(agent) = config.agent_mut(&id) {
            agent.lead = lead;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ProjectConfig;

    fn agent(id: &str, project: Option<&str>, lead: Option<&str>) -> AgentConfig {
        AgentConfig {
            id: id.into(),
            color: String::new(),
            project: project.map(str::to_owned),
            parent: None,
            lead: lead.map(str::to_owned),
            name: id.to_uppercase(),
            role: String::new(),
            prompt: String::new(),
            model: "a/b".into(),
            effort: "high".into(),
            placeholder: String::new(),
            folder: String::new(),
            extensions: Default::default(),
            avatar: Default::default(),
            fallback_model: String::new(),
        }
    }

    fn factory() -> Config {
        let mut config = Config { agents: Vec::new(), ..Config::default() };
        for id in ["web", "app"] {
            config.projects.push(ProjectConfig {
                id: id.into(),
                name: id.into(),
                path: format!("/{id}").into(),
                folders: Vec::new(),
                archived: false,
            });
        }
        config.agents = vec![
            agent("rivet", None, None),
            agent("coordinator", Some("web"), None),
            agent("backend", Some("web"), Some("coordinator")),
            agent("frontend", Some("web"), Some("coordinator")),
            agent("tester", Some("web"), Some("frontend")),
            agent("writer", Some("web"), None),
            agent("lead", Some("app"), None),
        ];
        config
    }

    fn ids(config: &Config) -> Vec<String> {
        config
            .agents
            .iter()
            .map(|agent| {
                let parent = parent_of(config, &agent.id).map_or(String::new(), |parent| format!(">{parent}"));
                format!("{}@{}{parent}", agent.id, agent.project.as_deref().unwrap_or("-"))
            })
            .collect()
    }

    #[test]
    fn agents_nest_under_their_lead_in_their_place_only() {
        let mut config = factory();
        config.agents.push(agent("ghost", Some("web"), Some("nobody")));
        config.agents.push(agent("far", Some("app"), Some("coordinator")));
        config.agents.push(agent("a", None, Some("b")));
        config.agents.push(agent("b", None, Some("a")));
        config.agents.push(agent("c", None, Some("a")));
        assert_eq!(parent_of(&config, "tester"), Some("frontend"));
        for loose in ["ghost", "far", "a", "b"] {
            assert_eq!(parent_of(&config, loose), None, "{loose}");
        }
        assert_eq!(parent_of(&config, "c"), Some("a"), "under an agent in a loop, which itself shows at the top");
        // Led by an admin agent elsewhere: its lead, at the top level.
        config.agents[0].extensions = serde_json::from_value(serde_json::json!({"agents": {"admin": true}})).unwrap();
        config.agents.push(agent("ops", Some("web"), Some("rivet")));
        assert_eq!(parent_of(&config, "ops"), None);
        // `parent` is no longer read.
        let mut visual = agent("old", Some("web"), None);
        visual.parent = Some("coordinator".into());
        config.agents.push(visual);
        assert_eq!(parent_of(&config, "old"), None);
    }

    #[test]
    fn dropping_onto_an_agent_makes_it_the_lead_and_the_top_level_clears_one_in_the_same_place() {
        let mut config = factory();
        arrange(&mut config, "writer", Some("backend"), None, None, None).expect("arrange");
        assert_eq!(config.agent("writer").unwrap().lead.as_deref(), Some("backend"));
        // A reorder among its siblings keeps it.
        arrange(&mut config, "writer", Some("backend"), None, None, None).expect("arrange");
        assert_eq!(config.agent("writer").unwrap().lead.as_deref(), Some("backend"));
        arrange(&mut config, "writer", None, Some("coordinator"), None, None).expect("arrange");
        assert_eq!(config.agent("writer").unwrap().lead, None);
        // An admin agent's lead from elsewhere stays at the top level.
        config.agents[0].extensions = serde_json::from_value(serde_json::json!({"agents": {"admin": true}})).unwrap();
        config.agent_mut("writer").unwrap().lead = Some("rivet".into());
        arrange(&mut config, "writer", None, None, None, None).expect("arrange");
        assert_eq!(config.agent("writer").unwrap().lead.as_deref(), Some("rivet"));
        // Cycles are refused in UpdateAgent's words.
        let refused = arrange(&mut config, "coordinator", Some("tester"), None, None, None).expect_err("a loop");
        assert_eq!(refused.to_string(), "TESTER reports to COORDINATOR, so COORDINATOR can't report to TESTER.");
    }

    #[test]
    fn moving_an_agent_takes_its_subtree_and_keeps_projects_together() {
        let mut config = factory();
        // Writer to the top of its project.
        arrange(&mut config, "writer", None, Some("coordinator"), None, None).expect("arrange");
        assert_eq!(ids(&config)[1..3], ["writer@web", "coordinator@web"]);
        // Frontend (with Tester) under Writer, last.
        arrange(&mut config, "frontend", Some("writer"), None, None, None).expect("arrange");
        assert_eq!(parent_of(&config, "frontend"), Some("writer"));
        assert_eq!(parent_of(&config, "tester"), Some("frontend"));
        // Writer's subtree into App, dropped onto Lead.
        arrange(&mut config, "writer", Some("lead"), None, None, None).expect("arrange");
        assert_eq!(
            ids(&config),
            [
                "rivet@-",
                "coordinator@web",
                "backend@web>coordinator",
                "lead@app",
                "writer@app>lead",
                "frontend@app>writer",
                "tester@app>frontend",
            ]
        );
        // Back to the workspace's top level.
        arrange(&mut config, "writer", None, None, Some(""), None).expect("arrange");
        assert_eq!(
            ids(&config)[1..],
            ["writer@-", "frontend@->writer", "tester@->frontend", "coordinator@web", "backend@web>coordinator", "lead@app"]
        );
    }

    #[test]
    fn a_moved_subtree_takes_the_folder_it_is_given_and_leaves_its_project_s_behind() {
        let mut config = factory();
        for project in &mut config.projects {
            let path = project.path.join("ui");
            project.folders.push(crate::config::FolderConfig { id: "ui".into(), name: "UI".into(), path });
        }
        arrange(&mut config, "frontend", None, None, None, Some("ui")).expect("arrange");
        let folder = |config: &Config, id: &str| config.agent(id).expect("agent").folder.clone();
        assert_eq!((folder(&config, "frontend"), folder(&config, "tester")), ("ui".to_owned(), "ui".to_owned()));
        // Unset: each keeps its folder in its project…
        arrange(&mut config, "frontend", Some("writer"), None, None, None).expect("arrange");
        assert_eq!(folder(&config, "tester"), "ui");
        // …and leaves it in another, even one with a folder of that id.
        arrange(&mut config, "frontend", None, None, Some("app"), None).expect("arrange");
        assert_eq!((folder(&config, "frontend"), folder(&config, "tester")), (String::new(), String::new()));
        assert!(arrange(&mut config, "lead", None, None, None, Some("nowhere")).is_err());
    }

    #[test]
    fn arrangements_that_make_no_sense_are_refused() {
        let mut config = factory();
        let before = config.clone();
        let refused = [
            arrange(&mut config, "coordinator", Some("tester"), None, None, None),
            arrange(&mut config, "frontend", Some("frontend"), None, None, None),
            arrange(&mut config, "writer", Some("lead"), None, Some("web"), None),
            arrange(&mut config, "writer", None, Some("backend"), None, None),
            arrange(&mut config, "nobody", None, None, None, None),
            arrange(&mut config, "writer", None, None, Some("nowhere"), None),
        ];
        for result in refused {
            assert!(result.is_err());
        }
        assert_eq!(config, before, "nothing changed");
    }

    #[test]
    fn a_deleted_parents_children_move_up() {
        let mut config = factory();
        lift_children(&mut config, "frontend");
        config.agents.retain(|agent| agent.id != "frontend");
        assert_eq!(parent_of(&config, "tester"), Some("coordinator"));
        // A top-level agent's reports go to the top level.
        lift_children(&mut config, "coordinator");
        config.agents.retain(|agent| agent.id != "coordinator");
        assert_eq!((config.agent("tester").unwrap().lead.clone(), config.agent("backend").unwrap().lead.clone()), (None, None));
    }
}
