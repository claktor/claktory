//! The Monitors overview (friend bug #10): every agent's monitors in one
//! list, each monitor's history a page at a time, and a signal to clients
//! when any of them changed, so a list on screen stays true.

use std::hash::{DefaultHasher, Hash, Hasher};

use factory_protocol::v1;
use rusqlite::{OptionalExtension, params};

use super::{ENDED_LISTED_MS, ID, Query, Status, query, to_proto};
use crate::config::Config;
use crate::factory::{Factory, RequestError};

/// A history page's events unless asked for fewer…
const HISTORY_PAGE: u32 = 50;
/// …and at most.
const HISTORY_MAX: u32 = 200;

impl Factory {
    /// ListMonitors: every agent's monitors, by agent and then id, but those
    /// that ended over a week ago.
    pub(crate) fn list_monitors(&self, request: &v1::ListMonitors) -> Result<v1::MonitorList, RequestError> {
        let now = self.monitors.now();
        let agent = Some(request.agent_id.trim()).filter(|agent| !agent.is_empty());
        let project = Some(request.project_id.trim()).filter(|project| !project.is_empty());
        let states: Vec<v1::MonitorState> = request.states().collect();
        let monitors = query(&self.store, &Query { agent, ended_since: Some(now - ENDED_LISTED_MS), ..Query::default() })?;
        let inner = self.lock();
        let config = &inner.config;
        let monitors = monitors
            .iter()
            .filter_map(|monitor| {
                let agent = config.agent(&monitor.agent)?;
                if project.is_some_and(|project| agent.project.as_deref() != Some(project)) {
                    return None;
                }
                let shown = to_proto(monitor, now);
                if !states.is_empty() && !states.contains(&shown.state()) {
                    return None;
                }
                let held = match monitor.status {
                    Status::Active => held(config, &monitor.agent),
                    _ => v1::MonitorHeld::Unspecified,
                };
                Some(v1::AgentMonitor { agent_id: monitor.agent.clone(), monitor: Some(shown), held: held.into() })
            })
            .collect();
        Ok(v1::MonitorList { monitors })
    }

    /// ListMonitorHistory: what happened to one monitor, newest first.
    pub(crate) fn monitor_history(&self, request: &v1::ListMonitorHistory) -> Result<v1::MonitorHistory, RequestError> {
        let agent = request.agent_id.trim();
        self.known_agent(agent)?;
        let db = self.store.db();
        let exists = db
            .query_row("SELECT 1 FROM monitors WHERE agent = ?1 AND id = ?2", params![agent, request.monitor_id], |_| Ok(()))
            .optional()?;
        if exists.is_none() {
            return Err(RequestError::NoMonitor(format!("{agent} has no monitor m{}.", request.monitor_id)));
        }
        let limit = match request.limit {
            0 => HISTORY_PAGE,
            limit => limit.min(HISTORY_MAX),
        };
        let before = i64::try_from(request.before_seq).unwrap_or(i64::MAX);
        let mut statement = db.prepare(
            "SELECT seq, time, actor, kind, value FROM monitor_events
             WHERE agent = ?1 AND monitor = ?2 AND (?3 = 0 OR seq < ?3) ORDER BY seq DESC LIMIT ?4",
        )?;
        let mut events = statement
            .query_map(params![agent, request.monitor_id, before, limit + 1], |row| {
                Ok(v1::MonitorEvent {
                    seq: row.get::<_, i64>(0)?.try_into().unwrap_or_default(),
                    time_ms: row.get(1)?,
                    actor: row.get(2)?,
                    kind: event_kind(&row.get::<_, String>(3)?).into(),
                    detail: row.get(4)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        let more = events.len() > limit as usize;
        events.truncate(limit as usize);
        Ok(v1::MonitorHistory { agent_id: agent.to_owned(), monitor_id: request.monitor_id, events, more })
    }
}

/// Why the agent's active monitors don't run now, if they don't: as the
/// scheduler decides (`run::tick_once`).
fn held(config: &Config, agent: &str) -> v1::MonitorHeld {
    match config.agent(agent) {
        Some(agent) if !crate::extensions::enabled(agent, ID) => v1::MonitorHeld::ExtensionOff,
        Some(agent) if config.archived(agent) => v1::MonitorHeld::ProjectArchived,
        _ => v1::MonitorHeld::Unspecified,
    }
}

fn event_kind(kind: &str) -> v1::MonitorEventKind {
    match kind {
        "created" => v1::MonitorEventKind::Created,
        "fired" => v1::MonitorEventKind::Fired,
        "paused" => v1::MonitorEventKind::Paused,
        "resumed" => v1::MonitorEventKind::Resumed,
        "cancelled" => v1::MonitorEventKind::Cancelled,
        "done" => v1::MonitorEventKind::Done,
        "error" => v1::MonitorEventKind::Error,
        _ => v1::MonitorEventKind::Unspecified,
    }
}

/// Tells every client that monitors changed, if they did since it last
/// said: any monitor saved (added, fired, checked, paused, ended), deleted,
/// or held or let go by its agent's settings. Called after each of the
/// scheduler's ticks and each change, so it costs a small query.
pub(crate) fn signal(factory: &Factory) {
    let read = || -> rusqlite::Result<Vec<(String, i64, i64)>> {
        let db = factory.store.db();
        let mut statement = db.prepare("SELECT agent, COUNT(*), MAX(updated) FROM monitors GROUP BY agent ORDER BY agent")?;
        statement.query_map([], crate::store::row3)?.collect()
    };
    let rows = match read() {
        Ok(rows) => rows,
        Err(error) => {
            tracing::error!(%error, "could not read monitors for the overview");
            return;
        }
    };
    let mut hasher = DefaultHasher::new();
    {
        let inner = factory.lock();
        for (agent, count, updated) in &rows {
            (agent, count, updated, held(&inner.config, agent) as i32).hash(&mut hasher);
        }
    }
    let now = hasher.finish();
    let mut said = factory.monitors.signalled();
    match *said {
        // The first look only sets where it stands: nothing has changed yet.
        None => *said = Some(now),
        Some(before) if before != now => {
            *said = Some(now);
            drop(said);
            factory.send(v1::server_message::Body::MonitorsChanged(v1::MonitorsChanged {}));
        }
        Some(_) => {}
    }
}
