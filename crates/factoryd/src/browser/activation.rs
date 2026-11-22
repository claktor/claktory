//! Browser control turns itself off (B3b; browser-control.md "State machine"):
//! when the backend sees it turned on for an agent, it keeps that
//! activation, one generation of it, in factory.db. Once a session the
//! agent asked for has ended and left the list (a minute later, so it can
//! ask again at once: for assist instead of read, say), or 5 minutes pass
//! with none asked for, it
//! writes `extensions.browser.enabled = false` for that agent, the only
//! config it ever writes, and only if the agent's browser settings are still
//! exactly as they were turned on (compare and swap, under the config's own
//! lock): a later choice of the person's is never overwritten, and the
//! activation is dropped instead. A restart cleans up the same way; nothing
//! about a session is ever restored.

use std::sync::Arc;

use factory_protocol::v1;
use rusqlite::{OptionalExtension, params};
use serde_json::Value;

use super::ms;
use crate::config::{AgentConfig, Config};
use crate::extensions::browser::ID;
use crate::factory::{Factory, RequestError};
use crate::transcript::now_ms;

/// The agent's browser settings as config.json has them.
fn settings(agent: &AgentConfig) -> Value {
    agent.extensions.get(ID).map_or(Value::Null, |settings| Value::Object(settings.clone()))
}

fn on(config: &Config, agent: &str) -> bool {
    config.agent(agent).is_some_and(|agent| crate::extensions::enabled(agent, ID))
}

/// From `apply_config`, before `after` replaces `before`: an agent the
/// extension was just turned on for gets a new activation; one it's off for
/// (or gone) has none.
pub(crate) fn observe(factory: &Factory, before: &Config, after: &Config) {
    let mut added = false;
    let db = factory.store.db();
    for agent in &before.agents {
        if on(before, &agent.id)
            && !on(after, &agent.id)
            && let Err(error) = db.execute("DELETE FROM browser_activations WHERE agent = ?1", params![agent.id])
        {
            tracing::warn!(%error, agent = %agent.id, "couldn't drop a browser activation");
        }
    }
    for agent in &after.agents {
        if on(after, &agent.id) && !on(before, &agent.id) {
            // A new row, so a new generation: AUTOINCREMENT never reuses one.
            let written = db.execute(
                "INSERT OR REPLACE INTO browser_activations (agent, settings, activated, used) VALUES (?1, ?2, ?3, 0)",
                params![agent.id, settings(agent).to_string(), now_ms()],
            );
            match written {
                Ok(_) => added = true,
                Err(error) => tracing::warn!(%error, agent = %agent.id, "couldn't keep a browser activation"),
            }
        }
    }
    drop(db);
    if added {
        factory.browser.wake.notify_one();
    }
}

/// The agent asked for a session: its activation ends when no session of
/// its is left.
pub(super) fn used(factory: &Factory, agent: &str) {
    if let Err(error) = factory.store.db().execute("UPDATE browser_activations SET used = 1 WHERE agent = ?1", params![agent]) {
        tracing::warn!(%error, %agent, "couldn't mark a browser activation used");
    }
}

/// Whether any activation is waiting to end.
pub(super) fn any(factory: &Factory) -> bool {
    factory.store.db().query_row("SELECT EXISTS (SELECT 1 FROM browser_activations)", [], |row| row.get(0)).unwrap_or(false)
}

/// Whether `agent` has a session listed: pending, active, or ended in the
/// last minute (`Limits::keep_ended`).
fn live(factory: &Factory, agent: &str) -> bool {
    factory.browser.state().sessions.iter().any(|session| session.owner.agent == agent)
}

/// Turns the extension off for agents whose activation is over.
pub(super) async fn sweep(factory: &Arc<Factory>) {
    if !factory.browser.limits().auto_off {
        return;
    }
    let rows: Vec<(String, i64, i64, bool)> = {
        let db = factory.store.db();
        let rows = db.prepare("SELECT agent, generation, activated, used FROM browser_activations").and_then(|mut statement| {
            statement.query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)))?.collect::<Result<Vec<_>, _>>()
        });
        match rows {
            Ok(rows) => rows,
            Err(error) => {
                tracing::warn!(%error, "couldn't read browser activations");
                return;
            }
        }
    };
    let wait = ms(factory.browser.limits().activation);
    let now = now_ms();
    for (agent, generation, activated, used) in rows {
        if live(factory, &agent) || (!used && now - activated < wait) {
            continue;
        }
        turn_off(factory, &agent, generation).await;
    }
}

/// What the compare and swap found.
enum Found {
    /// Off now.
    Done,
    /// A session started meanwhile: later.
    Live,
    /// Changed since (or gone): left as the person has it.
    Changed,
}

async fn turn_off(factory: &Arc<Factory>, agent: &str, generation: i64) {
    let mut found = Found::Changed;
    let result = factory
        .edit_config(|config| {
            let kept: Option<String> = factory
                .store
                .db()
                .query_row("SELECT settings FROM browser_activations WHERE agent = ?1 AND generation = ?2", params![agent, generation], |row| {
                    row.get(0)
                })
                .optional()?;
            let same = kept.and_then(|kept| serde_json::from_str::<Value>(&kept).ok()).is_some_and(|kept| {
                config.agent(agent).is_some_and(|now| crate::extensions::enabled(now, ID) && settings(now) == kept)
            });
            if !same {
                return Err(RequestError::Remote("changed".to_owned()));
            }
            // Config's lock comes before the browser's.
            if live(factory, agent) {
                found = Found::Live;
                return Err(RequestError::Remote("live".to_owned()));
            }
            let change = v1::ExtensionChange { id: ID.to_owned(), enabled: Some(false), ..Default::default() };
            crate::extensions::apply_change(config, agent, &change)?;
            found = Found::Done;
            Ok(())
        })
        .await;
    match (result, found) {
        (Ok(()), _) => tracing::info!(%agent, "browser control turned itself off"),
        (Err(_), Found::Live) => {}
        (Err(error), _) => {
            // Changed since, or the config can't be written: it stays as it
            // is, on, and isn't tried again.
            tracing::info!(%agent, %error, "browser control stays on: its settings changed since it was turned on");
            let dropped = factory.store.db().execute(
                "DELETE FROM browser_activations WHERE agent = ?1 AND generation = ?2",
                params![agent, generation],
            );
            if let Err(error) = dropped {
                tracing::warn!(%error, %agent, "couldn't drop a browser activation");
            }
        }
    }
}
