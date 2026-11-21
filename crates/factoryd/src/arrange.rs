//! How agents are arranged in the sidebar: within each project (or the
//! workspace) they come in config.json's order, and an agent can be nested
//! under another one in the same place. Nesting is purely visual.
//!
//! A `parent` that's missing, in another project, or part of a loop is
//! ignored: the agent shows at the top level.

use std::collections::HashSet;

use crate::config::{AgentConfig, Config};
use crate::factory::RequestError;

/// The agent `id` is nested under, if its `parent` is valid.
pub(crate) fn parent_of<'a>(config: &'a Config, id: &str) -> Option<&'a str> {
    let agent = config.agent(id)?;
    let parent = config.agent(agent.parent.as_deref()?)?;
    if parent.project != agent.project {
        return None;
    }
    // Walk up: reaching the agent again means a loop.
    let mut seen = HashSet::from([agent.id.as_str()]);
    let mut current = parent;
    loop {
        if current.id == agent.id {
            return None;
        }
        if !seen.insert(current.id.as_str()) {
            return Some(parent.id.as_str());
        }
        match current.parent.as_deref().and_then(|next| config.agent(next)) {
            Some(next) if next.project == current.project => current = next,
            _ => return Some(parent.id.as_str()),
        }
    }
}

/// `parent` values that are ignored, and why, for the log.
pub(crate) fn ignored_parents(config: &Config) -> Vec<String> {
    let mut ignored = Vec::new();
    for agent in &config.agents {
        let Some(parent) = agent.parent.as_deref() else { continue };
        if parent_of(config, &agent.id).is_none() {
            let why = match config.agent(parent) {
                None => "there's no such agent",
                Some(other) if other.project != agent.project => "it's in another project",
                Some(_) => "it would nest the agent under itself",
            };
            ignored.push(format!("agent \"{}\": parent \"{parent}\" is ignored: {why}", agent.id));
        }
    }
    ignored
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
        return Err(RequestError::Arrange(format!("{} can't go under itself or one of its own agents", agent.name)));
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
                return Err(RequestError::Arrange(format!("{} is in another project", parent.name)));
            }
            project
        }
        (None, Some(parent)) => parent.project.clone(),
        (None, None) => agent.project.clone(),
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
            return Err(RequestError::Arrange(format!("{} isn't among the agents it's going next to", sibling.name)));
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
    taken[0].parent = parent;
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

/// An agent is about to go: the agents nested under it move up to its own
/// parent.
pub(crate) fn lift_children(config: &mut Config, going: &str) {
    let grandparent = parent_of(config, going).map(str::to_owned);
    for agent in &mut config.agents {
        if agent.parent.as_deref() == Some(going) {
            agent.parent = grandparent.clone();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ProjectConfig;

    fn agent(id: &str, project: Option<&str>, parent: Option<&str>) -> AgentConfig {
        AgentConfig {
            id: id.into(),
            color: String::new(),
            project: project.map(str::to_owned),
            parent: parent.map(str::to_owned),
            lead: None,
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
    fn parents_that_cannot_hold_are_ignored() {
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
        assert_eq!(ignored_parents(&config).len(), 4);
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
    }
}
