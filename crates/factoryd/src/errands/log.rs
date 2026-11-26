//! An errand's log (agent-browser.md §5.5), kept 7 days, for the person:
//! who did what, where, and how it went. Never what was typed or read, a
//! query, or anything a page said but the target's role and name, cut to
//! 80 characters, shown as the page's.

use factory_protocol::v1;
use rusqlite::params;

use crate::store::Store;
use crate::transcript::now_ms;

const KEEP_MS: i64 = 7 * 24 * 60 * 60 * 1000;
/// The most entries a log answer holds: the newest.
const MAX_ENTRIES: u32 = 1000;
/// The most characters of a target.
const TARGET_MAX: usize = 80;
/// The most characters of a path.
const PATH_MAX: usize = 300;
/// The most characters of an action or an outcome.
const LINE_MAX: usize = 200;

/// One line with no control or invisible characters, at most `max`
/// characters, cut with "…".
fn line(text: &str, max: usize) -> String {
    let clean: String = text
        .chars()
        .map(|c| if c.is_control() || factory_agent::text::hidden(c) { ' ' } else { c })
        .collect();
    let clean = clean.split_whitespace().collect::<Vec<_>>().join(" ");
    if clean.chars().count() <= max {
        return clean;
    }
    let mut cut: String = clean.chars().take(max - 1).collect();
    cut.push('…');
    cut
}

/// What an entry says.
#[derive(Debug, Default, Clone)]
pub(crate) struct Entry {
    pub errand: String,
    /// The errand's agent, or empty for the person.
    pub agent: String,
    pub origin: String,
    /// Without its query or fragment.
    pub path: String,
    /// In Claktory's words: "Clicked", "Typed 12 characters".
    pub action: String,
    /// Empty when it worked, else why not.
    pub outcome: String,
    /// The element's role and name, read from the page.
    pub target: String,
}

/// Writes `entry` now, and forgets entries past 7 days.
pub(crate) fn write(store: &Store, entry: &Entry) {
    let path = entry.path.split(['?', '#']).next().unwrap_or_default();
    let db = store.db();
    let written = db.execute(
        "INSERT INTO errand_log (at, errand, agent, origin, path, action, outcome, target) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        params![
            now_ms(),
            entry.errand,
            entry.agent,
            line(&entry.origin, PATH_MAX),
            line(path, PATH_MAX),
            line(&entry.action, LINE_MAX),
            line(&entry.outcome, LINE_MAX),
            line(&entry.target, TARGET_MAX),
        ],
    );
    if let Err(error) = written {
        tracing::warn!(%error, "couldn't log an errand's step");
    }
    if let Err(error) = db.execute("DELETE FROM errand_log WHERE at < ?1", params![now_ms() - KEEP_MS]) {
        tracing::warn!(%error, "couldn't forget old errand log entries");
    }
}

/// ListErrandLog: `errand`'s entries (every errand's when empty), the last
/// 7 days, oldest first.
pub(crate) fn list(store: &Store, errand: &str) -> Result<v1::ErrandLog, rusqlite::Error> {
    let db = store.db();
    let mut statement = db.prepare(
        "SELECT at, errand, agent, origin, path, action, outcome, target FROM errand_log
         WHERE at >= ?1 AND (?2 = '' OR errand = ?2) ORDER BY at DESC, id DESC LIMIT ?3",
    )?;
    let mut entries = statement
        .query_map(params![now_ms() - KEEP_MS, errand, MAX_ENTRIES], |row| {
            Ok(v1::ErrandLogEntry {
                at_ms: row.get(0)?,
                errand_id: row.get(1)?,
                agent_id: row.get(2)?,
                origin: row.get(3)?,
                path: row.get(4)?,
                action: row.get(5)?,
                outcome: row.get(6)?,
                target: row.get(7)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    entries.reverse();
    Ok(v1::ErrandLog { errand_id: errand.to_owned(), entries })
}

#[cfg(test)]
mod tests {
    use super::line;

    #[test]
    fn lines_are_plain_and_short() {
        assert_eq!(line("button \"Next\u{202e}\"\n", 80), "button \"Next \"");
        assert_eq!(line(&"a".repeat(90), 80).chars().count(), 80);
        assert!(line(&"a".repeat(90), 80).ends_with('…'));
    }
}
