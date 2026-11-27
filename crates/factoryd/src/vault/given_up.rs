//! Secrets agents gave up on their own (`secrets_give_up`), kept for
//! the person's History (NEEDS_YOU_RECORD_KIND_SECRET_GIVEN_UP): "Mista
//! gave up Cloudflare", with its reason apart. Giving one up only takes
//! access away, so it asks no one; getting it back takes a request.

use std::collections::HashMap;

use crate::factory::Factory;
use crate::transcript::now_ms;

/// The table of secrets agents gave up.
pub(crate) const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS secrets_given_up (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    agent TEXT NOT NULL,
    -- The secret, by id, and its name then.
    secret TEXT NOT NULL,
    name TEXT NOT NULL,
    -- Why, in the agent's words.
    reason TEXT NOT NULL DEFAULT '',
    at INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS secrets_given_up_at ON secrets_given_up (at);
";

/// Keeps that `agent` gave up the secret: its id; None when it couldn't be
/// kept (logged).
pub(crate) fn record(factory: &Factory, agent: &str, secret: &str, name: &str, reason: &str) -> Option<i64> {
    let db = factory.store.db();
    let kept = db.execute(
        "INSERT INTO secrets_given_up (agent, secret, name, reason, at) VALUES (?1, ?2, ?3, ?4, ?5)",
        rusqlite::params![agent, secret, name, reason, now_ms()],
    );
    match kept {
        Ok(_) => Some(db.last_insert_rowid()),
        Err(error) => {
            tracing::error!(%error, "couldn't keep a secret given up for the history");
            None
        }
    }
}

/// A secret an agent gave up, as kept.
#[derive(Clone, Debug)]
pub(crate) struct GivenUp {
    pub id: i64,
    pub agent: String,
    pub name: String,
    pub reason: String,
    pub at: i64,
}

impl GivenUp {
    /// Claktory's sentence for it, with the agent's name now: "Mista gave up
    /// Cloudflare".
    pub(crate) fn title(&self, names: &HashMap<String, String>) -> String {
        let agent = names.get(&self.agent).cloned().unwrap_or_else(|| self.agent.clone());
        format!("{agent} gave up {}", self.name)
    }
}

/// The secrets given up from `since` to before `before`, newest first, at
/// most `limit`.
pub(crate) fn between(db: &rusqlite::Connection, since: i64, before: i64, limit: i64) -> rusqlite::Result<Vec<GivenUp>> {
    let mut statement = db.prepare(
        "SELECT id, agent, name, reason, at FROM secrets_given_up
         WHERE at >= ?1 AND at < ?2 ORDER BY at DESC, id DESC LIMIT ?3",
    )?;
    statement
        .query_map(rusqlite::params![since, before, limit], |row| {
            Ok(GivenUp { id: row.get(0)?, agent: row.get(1)?, name: row.get(2)?, reason: row.get(3)?, at: row.get(4)? })
        })?
        .collect()
}

/// The record a secrets_give_up call made, from its entry.
pub(crate) fn given_up_of(call: &factory_protocol::v1::ToolCall) -> Option<String> {
    if !matches!(call.name.as_str(), TOOL | "extension_call") {
        return None;
    }
    let details: serde_json::Value = serde_json::from_str(&call.details).ok()?;
    details.get(DETAIL)?.as_str().map(str::to_owned)
}

/// The tool, and the key of its result's details naming the record.
pub(crate) const TOOL: &str = "secrets_give_up";
pub(crate) const DETAIL: &str = "secret_given_up";
