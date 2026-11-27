//! Who turned an agent's browser on or off, kept for Needs you's History
//! (NEEDS_YOU_RECORD_KIND_BROWSER), with no card. Written where the
//! Browser extension is turned on or off for an agent: by the person, by
//! an admin agent in the settings file, or by Claktory for an agent that
//! can't have it; agents can't turn their own on.

use std::collections::HashMap;

use crate::config::Config;
use crate::transcript::now_ms;

pub(crate) const SCHEMA: &str = "
-- Agents' browsers turned on or off (the Browser extension), for Needs
-- you's history.
CREATE TABLE IF NOT EXISTS browser_switches (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    -- Whose browser.
    agent TEXT NOT NULL,
    -- The admin agent that did it; empty for the person.
    by TEXT NOT NULL,
    -- 1: turned on; 0: turned off.
    turned_on INTEGER NOT NULL,
    at INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS browser_switches_at ON browser_switches (at);
";

/// Who turned a browser on or off.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum By<'a> {
    /// The person, in the app (or factoryd's command line, with their
    /// authority).
    Person,
    /// An admin agent, through the settings file.
    Admin(&'a str),
    /// Claktory itself: a browser an agent can't have, turned off.
    Claktory,
}

/// Kept in `by` for Claktory itself: never an agent's id, which is
/// lowercase.
const CLAKTORY: &str = "Claktory";

/// Keeps that `by` turned `agent`'s browser on (`on`) or off (logged when
/// it can't).
pub(crate) fn record(store: &crate::store::Store, by: By<'_>, agent: &str, on: bool) {
    let by = match by {
        By::Person => "",
        By::Admin(id) => id,
        By::Claktory => CLAKTORY,
    };
    let db = store.db();
    let kept = db.execute(
        "INSERT INTO browser_switches (agent, by, turned_on, at) VALUES (?1, ?2, ?3, ?4)",
        rusqlite::params![agent, by, on, now_ms()],
    );
    if let Err(error) = kept {
        tracing::error!(%error, "couldn't keep a browser turned on or off for the history");
    }
}

/// Keeps each agent in both `before` and `after` whose browser `after`
/// turned on or off, as `by`'s for that agent.
pub(crate) fn record_changes<'a>(store: &crate::store::Store, before: &Config, after: &Config, by: impl Fn(&str) -> By<'a>) {
    for agent in &after.agents {
        let Some(was) = before.agent(&agent.id) else { continue };
        let on = crate::config::browser_on(agent);
        if crate::config::browser_on(was) != on {
            record(store, by(&agent.id), &agent.id, on);
        }
    }
}

/// A switch kept.
#[derive(Clone, Debug)]
pub(crate) struct Switch {
    pub id: i64,
    pub agent: String,
    pub by: String,
    pub on: bool,
    pub at: i64,
}

impl Switch {
    /// The admin agent that did it, for History's row; empty for the
    /// person or Claktory.
    pub(crate) fn admin(&self) -> &str {
        if self.by == CLAKTORY { "" } else { &self.by }
    }

    /// Claktory's sentence for it, with agents' names now: "Rivet turned
    /// on the browser for Yukako", "You turned off the browser for Yukako".
    pub(crate) fn title(&self, names: &HashMap<String, String>) -> String {
        let name = |id: &String| names.get(id).cloned().unwrap_or_else(|| id.clone());
        let by = match self.by.as_str() {
            "" => "You".to_owned(),
            CLAKTORY => CLAKTORY.to_owned(),
            agent => name(&agent.to_owned()),
        };
        let turned = if self.on { "on" } else { "off" };
        format!("{by} turned {turned} the browser for {}", name(&self.agent))
    }
}

/// The switches from `since` to before `before`, newest first, at most
/// `limit`.
pub(crate) fn between(db: &rusqlite::Connection, since: i64, before: i64, limit: i64) -> rusqlite::Result<Vec<Switch>> {
    crate::store::query(
        db,
        "SELECT id, agent, by, turned_on, at FROM browser_switches WHERE at >= ?1 AND at < ?2 ORDER BY at DESC, id DESC LIMIT ?3",
        rusqlite::params![since, before, limit],
        |row| Ok(Switch { id: row.get(0)?, agent: row.get(1)?, by: row.get(2)?, on: row.get(3)?, at: row.get(4)? }),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_sentence_names_who_and_whose() {
        let names: HashMap<String, String> = [("rivet", "Rivet"), ("yukako", "Yukako")].map(|(id, name)| (id.to_owned(), name.to_owned())).into();
        let switch = |by: &str, on: bool| Switch { id: 1, agent: "yukako".to_owned(), by: by.to_owned(), on, at: 0 };
        assert_eq!(switch("rivet", true).title(&names), "Rivet turned on the browser for Yukako");
        assert_eq!(switch("", false).title(&names), "You turned off the browser for Yukako");
        assert_eq!(switch(CLAKTORY, false).title(&names), "Claktory turned off the browser for Yukako");
    }
}
