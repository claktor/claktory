//! Pinned agents: the agents the
//! person keeps under Rivet in the sidebar, from any project, in their
//! order. A pin is a shortcut, never a move: the agent stays in its
//! project, under its lead. Kept in config.json (`pinned_agents`), so it's
//! the same on every device and Rivet can pin when asked; an agent deleted,
//! moved out of projects or archived with its project is unpinned there
//! (`Config::prune_pins`).

use factory_protocol::v1;

use crate::factory::{Factory, RequestError};

/// PinAgent: pins `agent_id` before `before_id` (empty, or not pinned:
/// last), moving it there when it's pinned already.
pub(crate) async fn pin(factory: &Factory, request: &v1::PinAgent) -> Result<(), RequestError> {
    let id = request.agent_id.clone();
    let before = request.before_id.clone();
    factory
        .edit_config(|config| {
            let agent = config.agent(&id).ok_or_else(|| RequestError::UnknownAgent(id.clone()))?;
            if agent.project.is_none() {
                return Err(RequestError::Command(format!("{} is outside projects, so it's at the top already.", agent.name)));
            }
            if config.archived(agent) {
                return Err(RequestError::Command(format!("{}'s project is archived: unarchive it first.", agent.name)));
            }
            if before == id {
                return Ok(());
            }
            config.pinned_agents.retain(|pinned| *pinned != id);
            let at = config.pinned_agents.iter().position(|pinned| *pinned == before).unwrap_or(config.pinned_agents.len());
            config.pinned_agents.insert(at, id.clone());
            Ok(())
        })
        .await
}

/// UnpinAgent: one not pinned is no change.
pub(crate) async fn unpin(factory: &Factory, request: &v1::UnpinAgent) -> Result<(), RequestError> {
    let id = request.agent_id.clone();
    factory
        .edit_config(|config| {
            config.pinned_agents.retain(|pinned| *pinned != id);
            Ok(())
        })
        .await
}
