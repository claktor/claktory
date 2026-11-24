//! Sends the app makes again (#401): on a weak connection it can't tell a
//! message that never arrived from one whose answer was lost, so it sends
//! again with the same `client_id` until it hears back. Whatever the person
//! sent arrives once: a repeat answers as a success and delivers nothing
//! twice, so their words are never lost and never doubled.
//!
//! The person's messages carry their app's `client_id` into factory.db's
//! `messages`, for an agent or a chat alike (`chat:<n>`); that's what a
//! repeat is told by, across connections and restarts.

use std::sync::Mutex;

use rusqlite::OptionalExtension;

use crate::factory::{Factory, RequestError};

/// Finding a message by its `client_id` without reading every message.
pub(crate) const INDEX: &str = "
-- The person's messages by the id their app gave them (crate::repeats).
CREATE INDEX messages_by_client_id ON messages (client_id) WHERE client_id IS NOT NULL;
";

/// One of the person's sends at a time: a repeat that arrives while the
/// first is still being recorded waits for it, then finds it.
static SENDING: Mutex<()> = Mutex::new(());

/// Whether the person's message `client_id` is already recorded for `agent`
/// (an agent's id, or a chat's `chat:<n>`).
pub(crate) fn sent(factory: &Factory, agent: &str, client_id: &str) -> bool {
    let db = factory.store.db();
    let found = db
        .query_row("SELECT 1 FROM messages WHERE client_id = ?1 AND agent = ?2 LIMIT 1", [client_id, agent], |_| Ok(()))
        .optional();
    match found {
        Ok(found) => found.is_some(),
        Err(error) => {
            tracing::error!(%error, "could not look for a message sent before");
            false
        }
    }
}

/// The chat whose messages include the person's `client_id`, if any.
pub(crate) fn chat_of(factory: &Factory, client_id: &str) -> Option<String> {
    let db = factory.store.db();
    db.query_row(
        "SELECT agent FROM messages WHERE client_id = ?1 AND agent LIKE 'chat:%' ORDER BY id LIMIT 1",
        [client_id],
        |row| row.get(0),
    )
    .optional()
    .ok()
    .flatten()
}

/// Sends the person's message `client_id` to `agent` with `send`, unless
/// it's already there: then it's a repeat, answered as a success.
pub(crate) fn once(
    factory: &Factory,
    agent: &str,
    client_id: &str,
    send: impl FnOnce() -> Result<(), RequestError>,
) -> Result<(), RequestError> {
    if client_id.is_empty() {
        return send();
    }
    let _one = SENDING.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    if sent(factory, agent, client_id) {
        tracing::info!(agent, client_id, "a message sent again (its answer was lost); it was delivered once already");
        return Ok(());
    }
    send()
}
