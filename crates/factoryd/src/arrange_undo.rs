//! The sidebar's Undo for a drag, and what a move to another project would
//! do to its pipes (items 15 and 17b).
//!
//! An undoable ArrangeAgent arranges as ArrangeAgent does (`arrange`), and
//! answers what it moved with a token. RestoreArrangement puts exactly that
//! back, project, folder, lead and place, for 10 seconds (the app offers
//! 8), from the same
//! device only: kept in memory, one per device, so a newer drag from it
//! replaces the older one, using it spends it, and a phone that reconnects
//! meanwhile can still undo. It's refused, with nothing changed, when any
//! of those agents changed since, or where they were is gone.

use std::collections::HashMap;
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant};

use factory_protocol::v1;

use crate::config::{AgentConfig, Config};
use crate::factory::{Factory, RequestError};

/// How long a drag can be undone: the app's 8 seconds, and a little for the
/// round trip.
const KEPT: Duration = Duration::from_secs(10);

/// Refused: the list isn't what the drag left.
const CHANGED: &str = "The list changed since, so this can't be undone.";
/// Refused: spent, from another device, or too old.
const GONE: &str = "This can't be undone any more.";

/// Where an agent is.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Place {
    project: Option<String>,
    folder: String,
    lead: Option<String>,
}

impl Place {
    fn of(agent: &AgentConfig) -> Place {
        Place { project: agent.project.clone(), folder: agent.folder.clone(), lead: agent.lead.clone() }
    }
}

/// An agent a drag moved.
#[derive(Clone, Debug)]
struct Moved {
    id: String,
    /// Where it was, and where the drag left it.
    before: Place,
    after: Place,
    /// Its place in config.json's order before: its index, and the first
    /// agent after it that didn't move (None: it came after all of them).
    index: usize,
    anchor: Option<String>,
    /// The agent just before it, for the app to show.
    after_id: String,
}

struct Kept {
    token: String,
    at: Instant,
    moved: Vec<Moved>,
}

/// The last undoable drag of each device, by device id.
#[derive(Default)]
pub(crate) struct Undos(Mutex<HashMap<String, Kept>>);

impl Undos {
    fn keep(&self, device: &str, kept: Kept) {
        let mut undos = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        undos.retain(|_, kept| kept.at.elapsed() < KEPT);
        undos.insert(device.to_owned(), kept);
    }

    /// `device`'s drag with `token`, spent, if it's still there.
    fn take(&self, device: &str, token: &str) -> Option<Vec<Moved>> {
        let mut undos = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        let kept = undos.remove(device)?;
        if kept.token != token {
            undos.insert(device.to_owned(), kept);
            return None;
        }
        (kept.at.elapsed() < KEPT).then_some(kept.moved)
    }
}

/// An undoable ArrangeAgent from `device`.
pub(crate) async fn arrange(
    factory: &Factory,
    device: &str,
    id: &str,
    parent: Option<&str>,
    before: Option<&str>,
    project: Option<&str>,
    folder: Option<&str>,
) -> Result<v1::Arranged, RequestError> {
    let mut moved = factory
        .edit_config(|config| {
            let moving = crate::arrange::subtree(config, id);
            let was: Vec<(String, Place, usize, Option<String>, String)> = moving
                .iter()
                .filter_map(|member| {
                    let index = config.agents.iter().position(|agent| &agent.id == member)?;
                    let anchor = config.agents[index + 1..].iter().find(|agent| !moving.contains(&agent.id)).map(|agent| agent.id.clone());
                    let after_id = index.checked_sub(1).map(|at| config.agents[at].id.clone()).unwrap_or_default();
                    Some((member.clone(), Place::of(&config.agents[index]), index, anchor, after_id))
                })
                .collect();
            crate::arrange::arrange(config, id, parent, before, project, folder)?;
            Ok(was
                .into_iter()
                .filter_map(|(id, before, index, anchor, after_id)| {
                    let after = Place::of(config.agent(&id)?);
                    Some(Moved { id, before, after, index, anchor, after_id })
                })
                .collect::<Vec<_>>())
        })
        .await?;
    let mut token = [0_u8; 16];
    getrandom::fill(&mut token).map_err(|error| RequestError::Arrange(error.to_string()))?;
    let token = crate::encoding::hex(&token);
    let answer = v1::Arranged {
        token: token.clone(),
        moved: moved
            .iter()
            .map(|moved| v1::ArrangedAgent {
                agent_id: moved.id.clone(),
                project_id: moved.before.project.clone().unwrap_or_default(),
                folder_id: moved.before.folder.clone(),
                lead_id: moved.before.lead.clone().unwrap_or_default(),
                after_id: moved.after_id.clone(),
            })
            .collect(),
    };
    // Put back in the order they stood in.
    moved.sort_by_key(|moved| moved.index);
    factory.arrange_undos.keep(device, Kept { token, at: Instant::now(), moved });
    Ok(answer)
}

/// RestoreArrangement from `device`.
pub(crate) async fn restore(factory: &Factory, device: &str, token: &str) -> Result<(), RequestError> {
    let Some(moved) = factory.arrange_undos.take(device, token) else {
        return Err(RequestError::Arrange(GONE.to_owned()));
    };
    factory.edit_config(|config| put_back(config, &moved)).await
}

/// Puts `moved` back where they were, or refuses with nothing changed.
fn put_back(config: &mut Config, moved: &[Moved]) -> Result<(), RequestError> {
    let mut next = config.clone();
    restored(&mut next, moved)?;
    *config = next;
    Ok(())
}

/// `put_back` on a copy, which a refusal leaves half done.
fn restored(config: &mut Config, moved: &[Moved]) -> Result<(), RequestError> {
    let changed = || RequestError::Arrange(CHANGED.to_owned());
    for moved in moved {
        let agent = config.agent(&moved.id).ok_or_else(changed)?;
        if Place::of(agent) != moved.after {
            return Err(changed());
        }
        let Place { project, folder, .. } = &moved.before;
        if let Some(project) = project
            && config.project(project).is_none()
        {
            return Err(changed());
        }
        if !folder.is_empty() && project.as_deref().and_then(|project| config.project(project)?.folder(folder)).is_none() {
            return Err(changed());
        }
    }
    let mut taken = Vec::new();
    for moved in moved {
        if let Some(index) = config.agents.iter().position(|agent| agent.id == moved.id) {
            let mut agent = config.agents.remove(index);
            agent.project.clone_from(&moved.before.project);
            agent.folder.clone_from(&moved.before.folder);
            agent.lead.clone_from(&moved.before.lead);
            taken.push((agent, moved.anchor.clone()));
        }
    }
    // Each just before the agent that followed it, in its old order: those
    // sharing an anchor keep theirs.
    for (agent, anchor) in taken {
        let index = anchor.and_then(|anchor| config.agents.iter().position(|agent| agent.id == anchor)).unwrap_or(config.agents.len());
        config.agents.insert(index, agent);
    }
    // Their leads as they were, if they can still be (a lead gone, now in
    // another project, or a loop): arrange's own rule.
    for moved in moved {
        if let (Some(agent), Some(lead)) = (config.agent(&moved.id), moved.before.lead.as_deref())
            && config.lead_refused(agent, lead).is_some()
        {
            return Err(changed());
        }
    }
    Ok(())
}

/// PreviewArrange: for the move's confirm, which of the old project's pipes
/// a moving agent stops answering, and who'd answer each after (17b).
pub(crate) fn preview(factory: &Factory, request: &v1::PreviewArrange) -> Result<v1::ArrangePreview, RequestError> {
    let before = factory.lock().config.clone();
    let mut after = before.clone();
    let some = |text: &str| (!text.is_empty()).then(|| text.to_owned());
    let (parent, sibling) = (some(&request.parent_id), some(&request.before_id));
    let moving = crate::arrange::subtree(&before, &request.agent_id);
    crate::arrange::arrange(&mut after, &request.agent_id, parent.as_deref(), sibling.as_deref(), request.project_id.as_deref(), None)?;
    let handovers = crate::pipes::lead::handovers(factory, &before, &after, &moving)
        .into_iter()
        .map(|(pipe_id, agent_id, next_lead_id)| v1::PipeHandover { pipe_id, agent_id, next_lead_id })
        .collect();
    Ok(v1::ArrangePreview { handovers })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ProjectConfig;

    fn agent(id: &str, project: Option<&str>, lead: Option<&str>, folder: &str) -> AgentConfig {
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
            folder: folder.into(),
            extensions: Default::default(),
            avatar: Default::default(),
            fallback_model: String::new(),
            compact_at_tokens: None,
            other: Default::default(),
        }
    }

    fn config() -> Config {
        let mut config = Config { agents: Vec::new(), ..Config::default() };
        for id in ["web", "app"] {
            let mut project = ProjectConfig {
                id: id.into(),
                name: id.into(),
                path: format!("/{id}").into(),
                folders: Vec::new(),
                archived: false,
                other: Default::default(),
            };
            project.folders.push(crate::config::FolderConfig {
                id: "ui".into(),
                name: "UI".into(),
                path: format!("/{id}/ui").into(),
                other: Default::default(),
            });
            config.projects.push(project);
        }
        config.agents = vec![
            agent("coordinator", Some("web"), None, ""),
            agent("backend", Some("web"), Some("coordinator"), ""),
            agent("frontend", Some("web"), Some("coordinator"), "ui"),
            agent("writer", Some("web"), None, ""),
            agent("tester", Some("web"), Some("frontend"), ""),
            agent("lead", Some("app"), None, ""),
        ];
        config
    }

    /// What `arrange` keeps, made without a Factory.
    fn dragged(config: &mut Config, id: &str, parent: Option<&str>, project: Option<&str>) -> Vec<Moved> {
        let moving = crate::arrange::subtree(config, id);
        let was: Vec<_> = moving
            .iter()
            .map(|member| {
                let index = config.agents.iter().position(|agent| &agent.id == member).expect("there");
                let anchor = config.agents[index + 1..].iter().find(|agent| !moving.contains(&agent.id)).map(|agent| agent.id.clone());
                (member.clone(), Place::of(&config.agents[index]), index, anchor)
            })
            .collect();
        crate::arrange::arrange(config, id, parent, None, project, None).expect("arranged");
        let mut moved: Vec<Moved> = was
            .into_iter()
            .map(|(id, before, index, anchor)| {
                let after = Place::of(config.agent(&id).expect("there"));
                Moved { id, before, after, index, anchor, after_id: String::new() }
            })
            .collect();
        moved.sort_by_key(|moved| moved.index);
        moved
    }

    #[test]
    fn a_move_to_another_project_goes_back_exactly() {
        let mut config = config();
        let original = config.clone();
        // Frontend (in UI) and Tester (in the project's own), into App
        // under Lead: both lose their folders, and they're placed together.
        let moved = dragged(&mut config, "frontend", Some("lead"), None);
        assert_eq!(config.agent("tester").expect("tester").project.as_deref(), Some("app"));
        put_back(&mut config, &moved).expect("put back");
        assert_eq!(config, original, "project, folder, lead and order");
    }

    #[test]
    fn a_reorder_and_a_new_lead_go_back() {
        let mut config = config();
        let original = config.clone();
        let moved = dragged(&mut config, "writer", Some("backend"), None);
        put_back(&mut config, &moved).expect("put back");
        assert_eq!(config, original);
    }

    #[test]
    fn nothing_goes_back_once_the_list_changed() {
        let mut config = config();
        let moved = dragged(&mut config, "frontend", Some("lead"), None);
        // Tester moved since, on another device.
        config.agent_mut("tester").expect("tester").lead = Some("lead".into());
        let before = config.clone();
        assert_eq!(put_back(&mut config, &moved).expect_err("refused").to_string(), CHANGED);
        assert_eq!(config, before, "nothing changed");
        // Its old lead deleted since.
        let mut config = self::config();
        let moved = dragged(&mut config, "writer", Some("backend"), None);
        let moved: Vec<Moved> = moved
            .into_iter()
            .map(|mut moved| {
                moved.before.lead = Some("gone".into());
                moved
            })
            .collect();
        let before = config.clone();
        assert!(put_back(&mut config, &moved).is_err());
        assert_eq!(config, before);
    }

    #[test]
    fn only_the_device_that_dragged_undoes_and_only_for_a_while() {
        let undos = Undos::default();
        let kept = |token: &str, at: Instant| Kept { token: token.into(), at, moved: Vec::new() };
        undos.keep("mac", kept("one", Instant::now()));
        assert!(undos.take("phone", "one").is_none(), "another device");
        assert!(undos.take("mac", "two").is_none(), "another token");
        assert!(undos.take("mac", "one").is_some(), "kept through both");
        assert!(undos.take("mac", "one").is_none(), "spent");
        // A newer drag replaces it.
        undos.keep("mac", kept("one", Instant::now()));
        undos.keep("mac", kept("two", Instant::now()));
        assert!(undos.take("mac", "one").is_none());
        assert!(undos.take("mac", "two").is_some());
        // Too old.
        if let Some(then) = Instant::now().checked_sub(KEPT) {
            undos.keep("mac", kept("old", then));
            assert!(undos.take("mac", "old").is_none());
        }
    }
}
