//! Browser control's log (B3b): what happened in each session, kept 7
//! days, for the person: who asked, which device, which site, which kind of
//! command and how it ended. Never what was typed, which element, a path or
//! anything a page said.

use factory_protocol::v1;
use rusqlite::params;

use super::Session;
use crate::factory::{Factory, RequestError};
use crate::transcript::now_ms;

const KEEP_MS: i64 = 7 * 24 * 60 * 60 * 1000;
const DEFAULT_LIMIT: u32 = 100;
const MAX_LIMIT: u32 = 500;

/// An entry about `session`, now.
pub(super) fn entry(session: &Session, device: &str, op: &str, sequence: u64, outcome: &str) -> v1::BrowserLogEntry {
    v1::BrowserLogEntry {
        at_ms: now_ms(),
        session_id: session.id.clone(),
        agent_id: session.owner.agent.clone(),
        device_id: device.to_owned(),
        origin: session.origin.clone(),
        op: op.to_owned(),
        sequence,
        outcome: outcome.to_owned(),
        // The row's, once written.
        id: 0,
    }
}

/// Writes the entries waiting in memory, and forgets those past 7 days.
/// Called outside the browser's lock.
pub(super) fn flush(factory: &Factory) {
    let entries = std::mem::take(&mut factory.browser.state().log);
    if entries.is_empty() {
        return;
    }
    let db = factory.store.db();
    for entry in &entries {
        let written = db.execute(
            "INSERT INTO browser_log (at, session, agent, device, origin, op, sequence, outcome) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                entry.at_ms,
                entry.session_id,
                entry.agent_id,
                entry.device_id,
                entry.origin,
                entry.op,
                i64::try_from(entry.sequence).unwrap_or(i64::MAX),
                entry.outcome
            ],
        );
        if let Err(error) = written {
            tracing::warn!(%error, "couldn't log a browser session's step");
        }
    }
    if let Err(error) = db.execute("DELETE FROM browser_log WHERE at < ?1", params![now_ms() - KEEP_MS]) {
        tracing::warn!(%error, "couldn't forget old browser log entries");
    }
}

fn unavailable() -> RequestError {
    RequestError::Remote("Browser control isn't available yet.".to_owned())
}

impl Factory {
    /// ListBrowserLog: the last 7 days, newest first.
    pub(crate) fn browser_log(&self, request: &v1::ListBrowserLog) -> Result<v1::BrowserLog, RequestError> {
        if !super::preview() {
            return Err(unavailable());
        }
        let limit = match request.limit {
            0 => DEFAULT_LIMIT,
            limit => limit.min(MAX_LIMIT),
        };
        let before = if request.before_ms > 0 { request.before_ms } else { i64::MAX };
        // Entries in the same millisecond as the last page's oldest, after
        // it in the log's order.
        let before_id = if request.before_ms > 0 { request.before_id.max(0) } else { 0 };
        let since = now_ms() - KEEP_MS;
        let db = self.store.db();
        let mut statement = db
            .prepare(
                "SELECT at, session, agent, device, origin, op, sequence, outcome, id FROM browser_log
                 WHERE at >= ?1 AND (at < ?2 OR (at = ?2 AND id < ?5)) AND (?3 = '' OR agent = ?3)
                 ORDER BY at DESC, id DESC LIMIT ?4",
            )?;
        let rows = statement
            .query_map(params![since, before, request.agent_id, limit + 1, before_id], |row| {
                Ok(v1::BrowserLogEntry {
                    at_ms: row.get(0)?,
                    session_id: row.get(1)?,
                    agent_id: row.get(2)?,
                    device_id: row.get(3)?,
                    origin: row.get(4)?,
                    op: row.get(5)?,
                    sequence: u64::try_from(row.get::<_, i64>(6)?).unwrap_or_default(),
                    outcome: row.get(7)?,
                    id: row.get(8)?,
                })
            })
            .and_then(|rows| rows.collect::<Result<Vec<_>, _>>())?;
        let more = rows.len() > limit as usize;
        Ok(v1::BrowserLog { entries: rows.into_iter().take(limit as usize).collect(), more })
    }

    /// ClearBrowserLog: forgets it all.
    pub(crate) fn clear_browser_log(&self) -> Result<(), RequestError> {
        if !super::preview() {
            return Err(unavailable());
        }
        self.store.db().execute("DELETE FROM browser_log", [])?;
        Ok(())
    }
}
