//! What agents did to other agents' conversations with the Conversations
//! extension, kept for the person's History (NEEDS_YOU_RECORD_KIND_CONVERSATION):
//! "Rivet took Mike's conversation back to 14:02", with Rivet's reason
//! apart. The person's own changes aren't kept: they made them.

use std::collections::HashMap;

use crate::factory::Factory;
use crate::transcript::{local_time, now_ms};

/// The table of changes agents made to conversations.
pub(crate) const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS conversation_changes (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    -- The agent whose conversation changed, and the agent that changed it.
    agent TEXT NOT NULL,
    by TEXT NOT NULL,
    -- back (to an earlier message) or switched (to another version).
    kind TEXT NOT NULL,
    -- Why, in the changing agent's words.
    reason TEXT NOT NULL DEFAULT '',
    -- back: when the message it went back to was given.
    point INTEGER NOT NULL DEFAULT 0,
    at INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS conversation_changes_at ON conversation_changes (at);
";

/// What happened to a conversation: its `kind` in the table, which can also
/// be "deleted" (a version set aside, deleted).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Kind {
    Back,
    Switched,
    Deleted,
}

impl Kind {
    fn key(self) -> &'static str {
        match self {
            Kind::Back => "back",
            Kind::Switched => "switched",
            Kind::Deleted => "deleted",
        }
    }
}

/// Keeps that `by` changed `agent`'s conversation: its id; None when it
/// couldn't be kept (logged).
pub(crate) fn record(factory: &Factory, agent: &str, by: &str, kind: Kind, reason: &str, point: i64) -> Option<i64> {
    let db = factory.store.db();
    let kept = db.execute(
        "INSERT INTO conversation_changes (agent, by, kind, reason, point, at) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        rusqlite::params![agent, by, kind.key(), reason, point, now_ms()],
    );
    match kept {
        Ok(_) => Some(db.last_insert_rowid()),
        Err(error) => {
            tracing::error!(%error, "couldn't keep a conversation change for the history");
            None
        }
    }
}

/// A change an agent made to a conversation, as kept.
#[derive(Clone, Debug)]
pub(crate) struct ConversationChange {
    pub id: i64,
    pub agent: String,
    pub by: String,
    pub kind: String,
    pub reason: String,
    pub point: i64,
    pub at: i64,
}

impl ConversationChange {
    fn read(row: &rusqlite::Row<'_>) -> rusqlite::Result<ConversationChange> {
        Ok(ConversationChange {
            id: row.get(0)?,
            agent: row.get(1)?,
            by: row.get(2)?,
            kind: row.get(3)?,
            reason: row.get(4)?,
            point: row.get(5)?,
            at: row.get(6)?,
        })
    }

    /// Claktory's sentence for it, with each agent's name now (`names`):
    /// "Rivet took Mike's conversation back to 14:02", "Rivet switched Mike to
    /// another version".
    pub(crate) fn title(&self, names: &HashMap<String, String>) -> String {
        let name = |id: &str| names.get(id).cloned().unwrap_or_else(|| id.to_owned());
        let (by, agent) = (name(&self.by), name(&self.agent));
        if self.kind == Kind::Switched.key() {
            return format!("{by} switched {agent} to another version");
        }
        if self.kind == Kind::Deleted.key() {
            return format!("{by} deleted one of {} other versions", crate::apps::changes::possessive(&agent));
        }
        let day = |ms: i64| local_time(ms, "%Y-%m-%d");
        let when = if day(self.point) == day(self.at) { local_time(self.point, "%H:%M") } else { local_time(self.point, "%-d %b %H:%M") };
        format!("{by} took {} conversation back to {when}", crate::apps::changes::possessive(&agent))
    }
}

/// The changes made from `since` to before `before`, newest first, at most
/// `limit`.
pub(crate) fn between(db: &rusqlite::Connection, since: i64, before: i64, limit: i64) -> rusqlite::Result<Vec<ConversationChange>> {
    let mut statement = db.prepare(
        "SELECT id, agent, by, kind, reason, point, at FROM conversation_changes
         WHERE at >= ?1 AND at < ?2 ORDER BY at DESC, id DESC LIMIT ?3",
    )?;
    statement.query_map(rusqlite::params![since, before, limit], ConversationChange::read)?.collect()
}

/// The change a conversation_fork or conversation_switch call made, from
/// its entry.
pub(crate) fn change_of(call: &factory_protocol::v1::ToolCall) -> Option<String> {
    if !matches!(call.name.as_str(), "conversation_fork" | "conversation_switch" | "conversation_delete" | "extension_call") {
        return None;
    }
    let details: serde_json::Value = serde_json::from_str(&call.details).ok()?;
    details.get("conversation_change")?.as_str().map(str::to_owned)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn titles_say_who_did_what_to_whom() {
        let names: HashMap<String, String> =
            [("rivet", "Rivet"), ("mike", "Mike"), ("james", "James")].map(|(id, name)| (id.to_owned(), name.to_owned())).into();
        let at = 1_764_000_000_000;
        let change = |agent: &str, kind: Kind, point: i64| ConversationChange {
            id: 1,
            agent: agent.to_owned(),
            by: "rivet".to_owned(),
            kind: kind.key().to_owned(),
            reason: String::new(),
            point,
            at,
        };
        let same_day = at - 60_000;
        assert_eq!(
            change("mike", Kind::Back, same_day).title(&names),
            format!("Rivet took Mike's conversation back to {}", local_time(same_day, "%H:%M"))
        );
        let days_before = at - 3 * 24 * 60 * 60 * 1000;
        assert_eq!(
            change("james", Kind::Back, days_before).title(&names),
            format!("Rivet took James' conversation back to {}", local_time(days_before, "%-d %b %H:%M"))
        );
        assert_eq!(change("mike", Kind::Switched, 0).title(&names), "Rivet switched Mike to another version");
        assert_eq!(change("mike", Kind::Deleted, 0).title(&names), "Rivet deleted one of Mike's other versions");
        assert_eq!(change("gone", Kind::Switched, 0).title(&names), "Rivet switched gone to another version");
    }
}
