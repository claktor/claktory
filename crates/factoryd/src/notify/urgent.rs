//! Who may send urgent alerts: admin agents
//! always, and either the person's own choice or, by default, every agent
//! that leads at least one other. Another agent's urgent alert goes as a
//! normal one, never refused, and the agent is told who may.
//!
//! The person's choice is kept in factory.db (notify_settings' row
//! 'urgent_from'), apart from the settings' own row, which a backend rolled
//! back to reads with no fields it doesn't know.

use std::sync::PoisonError;

use factory_protocol::v1;
use serde::{Deserialize, Serialize};

use crate::config::{AgentConfig, Config};
use crate::factory::{Factory, RequestError};

/// notify_settings' row with the person's choice.
const ROW: &str = "urgent_from";

/// The person's choice, as kept.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct UrgentFrom {
    /// Their own list of agent ids; None for the default.
    pub custom: Option<Vec<String>>,
}

/// The person's choice, once read from factory.db.
pub(crate) fn chosen(factory: &Factory) -> UrgentFrom {
    let mut cached = factory.notify.urgent_from.lock().unwrap_or_else(PoisonError::into_inner);
    cached.get_or_insert_with(|| super::read_row(factory, ROW)).clone()
}

/// Whether `agent` may send urgent alerts under `chosen`.
fn may(config: &Config, chosen: &UrgentFrom, agent: &AgentConfig) -> bool {
    if crate::proposals::is_admin(agent) {
        return true;
    }
    match &chosen.custom {
        Some(ids) => ids.contains(&agent.id),
        None => leads_another(config, agent),
    }
}

/// Whether some other agent reports to `agent`.
fn leads_another(config: &Config, agent: &AgentConfig) -> bool {
    config.agents.iter().any(|other| other.id != agent.id && config.lead(other).is_some_and(|lead| lead.id == agent.id))
}

/// The agents who may send urgent alerts now, in the sidebar's order.
pub(crate) fn senders<'a>(config: &'a Config, chosen: &UrgentFrom) -> Vec<&'a AgentConfig> {
    config.agents.iter().filter(|agent| may(config, chosen, agent)).collect()
}

/// Whether `agent` (an id) may send urgent alerts now.
pub(crate) fn allowed(factory: &Factory, config: &Config, agent: &str) -> bool {
    config.agent(agent).is_some_and(|agent| may(config, &chosen(factory), agent))
}

/// NotificationSettings.urgent_from and urgent_from_custom.
pub(crate) fn fill(factory: &Factory, config: &Config, settings: &mut v1::NotificationSettings) {
    let chosen = chosen(factory);
    settings.urgent_from = senders(config, &chosen).into_iter().map(|agent| agent.id.clone()).collect();
    settings.urgent_from_custom = chosen.custom.is_some();
}

/// NotificationSettingsChange.urgent_from: the person's own list, or the
/// default again. An id that isn't an agent's is refused, nothing changed.
/// Whether it changed.
pub(crate) fn change(factory: &Factory, config: &Config, senders: &v1::UrgentSenders) -> Result<bool, RequestError> {
    let new = if senders.use_default {
        UrgentFrom { custom: None }
    } else {
        let mut ids: Vec<String> = Vec::new();
        for id in &senders.agent_ids {
            if config.agent(id).is_none() {
                return Err(RequestError::UnknownAgent(id.clone()));
            }
            if !ids.contains(id) {
                ids.push(id.clone());
            }
        }
        UrgentFrom { custom: Some(ids) }
    };
    if chosen(factory) == new {
        return Ok(false);
    }
    factory.store.db().execute(
        "INSERT INTO notify_settings (key, value) VALUES (?1, ?2) ON CONFLICT (key) DO UPDATE SET value = excluded.value",
        [ROW, serde_json::to_string(&new)?.as_str()],
    )?;
    *factory.notify.urgent_from.lock().unwrap_or_else(PoisonError::into_inner) = Some(new);
    Ok(true)
}
