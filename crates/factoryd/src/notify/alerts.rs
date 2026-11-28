//! Alerts kept in Needs you: every alert, an agent's
//! notify_person or Claktory's own (its disk, the machine, a newer model),
//! waits in the person's Needs you (Workspace.alerts) until they open it or
//! tap Got it (ReadAlerts), then goes to History as read, with when and on
//! which device. Kept whatever became of its notification: quiet hours, the
//! hourly cap and the person's settings only decide what's pushed.
//!
//! An alert from an agent that had read a web page or another Claktory's
//! words keeps no words of its own (`withheld`), as its notification says
//! only who. Reading is the person's (rivet_reach: Person): no agent clears
//! one.
//!
//! Kept in factory.db, and in memory while they wait, so one Claktory tells
//! while its disk can't be written still shows until a restart.

use std::sync::{Mutex, MutexGuard, PoisonError};

use factory_protocol::v1;
use rusqlite::params;

use crate::factory::{Factory, RequestError};
use crate::transcript::now_ms;

pub(crate) const SCHEMA: &str = "
-- Alerts kept in Needs you until the person reads them (notify/alerts.rs).
CREATE TABLE IF NOT EXISTS alerts (
    id TEXT PRIMARY KEY,
    -- The agent that sent it; '' for Claktory's own.
    agent TEXT NOT NULL DEFAULT '',
    -- Who it's from as it was sent: the agent's name then, or the
    -- Claktory's.
    from_name TEXT NOT NULL,
    -- Its words: both '' when withheld.
    title TEXT NOT NULL DEFAULT '',
    text TEXT NOT NULL DEFAULT '',
    urgent INTEGER NOT NULL DEFAULT 0,
    -- Its agent had read a web page or another Claktory's words.
    withheld INTEGER NOT NULL DEFAULT 0,
    sent INTEGER NOT NULL,
    -- The notify_person call's entry in its agent's conversation; 0 for none.
    seq INTEGER NOT NULL DEFAULT 0,
    -- When the person read it, and on which device; null while it waits.
    read INTEGER,
    read_device TEXT NOT NULL DEFAULT '',
    read_device_name TEXT NOT NULL DEFAULT ''
);
CREATE INDEX IF NOT EXISTS alerts_waiting ON alerts (sent) WHERE read IS NULL;
CREATE INDEX IF NOT EXISTS alerts_read ON alerts (read) WHERE read IS NOT NULL;
";

/// Other agents' alerts folded into one (`fold`): their names, a JSON list.
/// A backend rolled back to names its columns, so never reads it.
pub(crate) const ALSO_FROM: &str = "
ALTER TABLE alerts ADD COLUMN also_from TEXT NOT NULL DEFAULT '';
";

/// Claktory's own alert about a device (one just allowed): its id, which
/// may not read it (`read`). '' for every other alert.
pub(crate) const DEVICE_ID: &str = "
ALTER TABLE alerts ADD COLUMN device_id TEXT NOT NULL DEFAULT '';
";

/// How long an unread alert takes other agents' alerts about the same thing
/// into it (`fold`).
pub(crate) const FOLD_MS: i64 = 30 * 60_000;

/// How many waiting alerts the Workspace lists, the newest.
pub(crate) const LISTED: usize = 100;

/// The alerts waiting, read from factory.db at the first look.
#[derive(Default)]
pub(crate) struct Alerts {
    waiting: Mutex<Option<Vec<v1::Alert>>>,
}

impl Alerts {
    fn waiting<'a>(&'a self, factory: &Factory) -> MutexGuard<'a, Option<Vec<v1::Alert>>> {
        let mut waiting = self.waiting.lock().unwrap_or_else(PoisonError::into_inner);
        if waiting.is_none() {
            *waiting = Some(load(factory).unwrap_or_else(|error| {
                tracing::error!(%error, "could not read the alerts waiting for the person");
                Vec::new()
            }));
        }
        waiting
    }

    /// Workspace.alerts and alerts_unread: the newest `LISTED`, oldest
    /// first, and how many wait.
    pub(crate) fn view(&self, factory: &Factory) -> (Vec<v1::Alert>, u32) {
        let waiting = self.waiting(factory);
        let all = waiting.as_deref().unwrap_or_default();
        let listed = all[all.len().saturating_sub(LISTED)..].to_vec();
        (listed, u32::try_from(all.len()).unwrap_or(u32::MAX))
    }
}

fn load(factory: &Factory) -> rusqlite::Result<Vec<v1::Alert>> {
    let db = factory.store.db();
    let mut statement = db.prepare(
        "SELECT id, agent, from_name, title, text, urgent, withheld, sent, seq, also_from, device_id FROM alerts WHERE read IS NULL ORDER BY sent, rowid",
    )?;
    let rows = statement.query_map([], read_alert)?;
    rows.collect()
}

fn read_alert(row: &rusqlite::Row<'_>) -> rusqlite::Result<v1::Alert> {
    Ok(v1::Alert {
        id: row.get(0)?,
        agent_id: row.get(1)?,
        from_name: row.get(2)?,
        title: row.get(3)?,
        text: row.get(4)?,
        urgent: row.get(5)?,
        withheld: row.get(6)?,
        sent_ms: row.get(7)?,
        seq: u64::try_from(row.get::<_, i64>(8)?).unwrap_or_default(),
        also_from: names(&row.get::<_, String>(9)?),
        device_id: row.get("device_id")?,
    })
}

/// A kept `also_from`: '' (none) or a JSON list of names.
fn names(kept: &str) -> Vec<String> {
    if kept.is_empty() {
        return Vec::new();
    }
    serde_json::from_str(kept).unwrap_or_else(|error| {
        tracing::warn!(%error, "an alert's also_from can't be read");
        Vec::new()
    })
}

/// Keeps `alert` (its id and time are given here; its words dropped when
/// withheld) in Needs you, and sends the Workspace: its id. Kept in memory
/// even when factory.db can't take it.
pub(crate) fn keep(factory: &Factory, mut alert: v1::Alert) -> String {
    let now = now_ms();
    alert.id = format!("alert-{now}-{}", super::rand_suffix());
    alert.sent_ms = now;
    if alert.withheld {
        (alert.title, alert.text) = (String::new(), String::new());
    }
    let kept = factory.store.db().execute(
        "INSERT INTO alerts (id, agent, from_name, title, text, urgent, withheld, sent, seq, device_id) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
        params![
            alert.id,
            alert.agent_id,
            alert.from_name,
            alert.title,
            alert.text,
            alert.urgent,
            alert.withheld,
            alert.sent_ms,
            i64::try_from(alert.seq).unwrap_or(i64::MAX),
            alert.device_id
        ],
    );
    if let Err(error) = kept {
        tracing::error!(%error, id = alert.id, "could not keep an alert: it waits in Needs you until Claktory restarts");
    }
    let id = alert.id.clone();
    factory.notify.alerts.waiting(factory).get_or_insert_default().push(alert);
    factory.broadcast_workspace(&factory.lock());
    id
}

/// ReadAlerts: the person read `ids` on `device_id` (`device_name`, as its
/// hello said). Ids already read or unknown are skipped. Refused, reading
/// none, when one is about `device_id` itself: a device just allowed can't
/// hide the alert that tells the person's other devices of it.
pub(crate) fn read(factory: &Factory, ids: &[String], device_id: &str, device_name: &str) -> Result<(), RequestError> {
    let now = now_ms();
    let own = factory
        .notify
        .alerts
        .waiting(factory)
        .as_deref()
        .unwrap_or_default()
        .iter()
        .any(|alert| ids.contains(&alert.id) && !alert.device_id.is_empty() && alert.device_id == device_id);
    if own {
        tracing::warn!(device = device_id, "a device tried to mark read the alert about itself");
        return Err(RequestError::Account(
            v1::ErrorCode::InvalidRequest,
            "This alert is about this device. Mark it read on another of your devices.".to_owned(),
        ));
    }
    let read: Vec<v1::Alert> = {
        let mut waiting = factory.notify.alerts.waiting(factory);
        let waiting = waiting.get_or_insert_default();
        let (read, still) = std::mem::take(waiting).into_iter().partition(|alert| ids.contains(&alert.id));
        *waiting = still;
        read
    };
    if read.is_empty() {
        return Ok(());
    }
    let db = factory.store.db();
    for alert in &read {
        let kept = db.execute(
            "UPDATE alerts SET read = ?2, read_device = ?3, read_device_name = ?4 WHERE id = ?1 AND read IS NULL",
            params![alert.id, now, device_id, device_name],
        );
        if let Err(error) = kept {
            tracing::error!(%error, id = alert.id, "could not keep that the person read an alert");
        }
    }
    drop(db);
    tracing::info!(alerts = read.len(), device = device_id, "the person read alerts");
    factory.broadcast_workspace(&factory.lock());
    Ok(())
}

/// A read alert, for History.
pub(crate) struct Read {
    pub alert: v1::Alert,
    pub read: i64,
    pub on: v1::AnsweredOn,
}

/// The alerts read from `since` to before `before`, newest first, at most
/// `limit`.
pub(crate) fn between(db: &rusqlite::Connection, since: i64, before: i64, limit: i64) -> rusqlite::Result<Vec<Read>> {
    let mut statement = db.prepare(
        "SELECT id, agent, from_name, title, text, urgent, withheld, sent, seq, also_from, read, read_device, read_device_name, device_id FROM alerts
         WHERE read IS NOT NULL AND read >= ?1 AND read < ?2 ORDER BY read DESC, rowid DESC LIMIT ?3",
    )?;
    let rows = statement.query_map(params![since, before, limit], |row| {
        let read: i64 = row.get(10)?;
        Ok(Read {
            alert: read_alert(row)?,
            read,
            on: v1::AnsweredOn { device_id: row.get(11)?, device_name: row.get(12)?, at_ms: read },
        })
    })?;
    rows.collect()
}

/// The alert a notify_person call kept, from its result's details.
pub(crate) fn alert_of(call: &v1::ToolCall) -> Option<String> {
    if call.name != super::tool::TOOL && call.name != "extension_call" {
        return None;
    }
    let details: serde_json::Value = serde_json::from_str(&call.details).ok()?;
    details.get("alert")?.as_str().map(str::to_owned)
}

/// The entry of `agent`'s notify_person call running now that says
/// `title`: its seq, or 0 when it isn't found. The call's entry is in its
/// conversation before its tool runs.
pub(crate) fn call_seq(factory: &Factory, agent: &str, title: &str) -> u64 {
    use factory_protocol::v1::{ToolCallState, entry::Kind};
    let inner = factory.lock();
    let Some(transcript) = inner.agents.get(agent).and_then(|runtime| runtime.transcript.as_ref()) else { return 0 };
    transcript
        .in_memory()
        .rev()
        .take(200)
        .find_map(|entry| {
            let Some(Kind::ToolCall(call)) = &entry.kind else { return None };
            if !matches!(call.state(), ToolCallState::Pending | ToolCallState::Running) {
                return None;
            }
            let input: serde_json::Value = serde_json::from_str(&call.input).ok()?;
            let args = match call.name.as_str() {
                super::tool::TOOL => &input,
                "extension_call" if input.get("tool").and_then(serde_json::Value::as_str) == Some(super::tool::TOOL) => input.get("args")?,
                _ => return None,
            };
            let said = args.get("title")?.as_str()?.split_whitespace().collect::<Vec<_>>().join(" ");
            (said == title).then_some(entry.seq)
        })
        .unwrap_or(0)
}

// ---- Folding ----

/// An alert folded into another agent's (`fold`).
pub(crate) struct Folded {
    /// The alert it folded into, as it is now.
    pub into: v1::Alert,
    /// It was urgent and made that alert urgent: it notifies once.
    pub upgraded: bool,
}

/// Folds `agent`'s alert (`name`, saying `title`, `urgent` as it may send it)
/// into another agent's about the same thing (`same_concern`), still unread
/// and sent less than `FOLD_MS` ago, the closest by its title and the newest
/// of equals: that alert lists `name` in also_from, once, and turns urgent
/// when this one is and it wasn't. Nothing folds into a withheld alert or
/// Claktory's own; the caller folds no withheld one either. None when no
/// alert is about the same thing.
pub(crate) fn fold(factory: &Factory, agent: &str, name: &str, title: &str, urgent: bool) -> Option<Folded> {
    let words = concern(title);
    let now = now_ms();
    let (into, upgraded) = {
        let mut waiting = factory.notify.alerts.waiting(factory);
        let into = waiting
            .get_or_insert_default()
            .iter_mut()
            .filter(|alert| !alert.agent_id.is_empty() && alert.agent_id != agent && !alert.withheld)
            .filter(|alert| now - alert.sent_ms < FOLD_MS)
            .filter_map(|alert| same_concern(&words, &concern(&alert.title)).map(|overlap| (overlap, alert)))
            .max_by(|(one, first), (other, second)| one.cmp(other).then(first.sent_ms.cmp(&second.sent_ms)))
            .map(|(_, alert)| alert)?;
        if !into.also_from.iter().any(|already| already == name) {
            into.also_from.push(name.to_owned());
        }
        let upgraded = urgent && !into.urgent;
        into.urgent |= urgent;
        (into.clone(), upgraded)
    };
    let kept = factory.store.db().execute(
        "UPDATE alerts SET also_from = ?2, urgent = ?3 WHERE id = ?1",
        params![into.id, serde_json::to_string(&into.also_from).unwrap_or_default(), into.urgent],
    );
    if let Err(error) = kept {
        tracing::error!(%error, id = into.id, "could not keep an alert folded into another: it shows until Claktory restarts");
    }
    tracing::info!(into = into.id, from = agent, upgraded, "an alert folded into another agent's");
    factory.broadcast_workspace(&factory.lock());
    Some(Folded { into, upgraded })
}

/// Words that say nothing of what an alert is about.
const STOP_WORDS: &[&str] = &[
    "the", "and", "for", "are", "but", "you", "your", "yours", "with", "this", "that", "these", "those", "there", "their", "them",
    "they", "then", "than", "from", "into", "onto", "over", "about", "after", "before", "during", "since", "until", "while", "what",
    "which", "who", "whom", "whose", "when", "where", "why", "how", "all", "any", "both", "each", "few", "more", "most", "other",
    "some", "such", "only", "own", "same", "too", "very", "can", "will", "just", "should", "would", "could", "may", "might", "must",
    "shall", "now", "again", "once", "here", "also", "its", "itself", "myself", "yourself", "himself", "herself", "themselves",
    "has", "have", "had", "having", "was", "were", "been", "being", "does", "did", "doing", "done", "per", "via", "out", "our",
    "ours", "mine", "his", "her", "hers", "him", "she", "yes", "get", "got", "let", "use",
];

/// What an alert's title is about: its words, in lower case, but those of
/// fewer than three letters, numbers (times, counts) and `STOP_WORDS`, each
/// cut to a simple stem ("answering" and "answered" as "answer", "tests" as
/// "test"), so two agents' words for one thing meet.
pub(crate) fn concern(title: &str) -> std::collections::BTreeSet<String> {
    title
        .to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|word| word.chars().count() >= 3 && !word.chars().all(|c| c.is_ascii_digit()) && !STOP_WORDS.contains(word))
        .map(stem)
        .collect()
}

fn stem(word: &str) -> String {
    let long = |at_least: usize| word.chars().count() >= at_least;
    let cut = if long(6) && word.ends_with("ing") {
        "ing"
    } else if long(5) && word.ends_with("ed") {
        "ed"
    } else if long(4) && word.ends_with('s') && !word.ends_with("ss") {
        "s"
    } else {
        ""
    };
    word[..word.len() - cut.len()].to_owned()
}

/// Whether two alerts' titles' words (`concern`) say the same thing: at
/// least two words in common, and those at least 60% of all their words
/// together. Leans towards not folding: a title's text, with its times,
/// names and who's looking, made alerts that "it's down" and "it's back"
/// look alike, so only titles count. How much they share, as a fraction
/// to compare, when they do.
fn same_concern(one: &std::collections::BTreeSet<String>, other: &std::collections::BTreeSet<String>) -> Option<Overlap> {
    let shared = one.intersection(other).count();
    let all = one.union(other).count();
    (shared >= 2 && shared * 5 >= all * 3).then_some(Overlap { shared, all })
}

/// `shared` of `all` words, ordered by that fraction.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Overlap {
    shared: usize,
    all: usize,
}

impl Ord for Overlap {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        (self.shared * other.all).cmp(&(other.shared * self.all))
    }
}

impl PartialOrd for Overlap {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

#[cfg(test)]
mod tests {
    use super::{concern, same_concern};

    fn folds(one: &str, other: &str) -> bool {
        same_concern(&concern(one), &concern(other)).is_some()
    }

    /// Real alerts of 2026-11-27: two agents' on tprime2 going down fold; the
    /// low disk folds with neither; and "it's back" never folds into "it's
    /// down", nor a correction.
    #[test]
    fn alerts_about_the_same_thing_fold_and_others_dont() {
        let fugo = "tprime2 stopped answering after my memory test";
        let bruno = "tprime2 stopped answering during Fugo's memory test";
        let disk = "Low disk is holding up the team's tests";
        assert!(folds(fugo, bruno) && folds(bruno, fugo));
        assert!(!folds(disk, fugo) && !folds(disk, bruno));
        assert!(!folds("tprime2 is reachable again", "tprime2 is back up by itself"), "leans towards not folding");
        for back in ["tprime2 is reachable again", "tprime2 is back up by itself", "Correction: tprime2 is down again"] {
            assert!(!folds(back, fugo) && !folds(back, bruno), "{back}");
        }
        assert!(!folds("Tests are failing on main", "Tests are passing on main"), "opposites with one word apart");
        assert!(folds("account.claktory.com isn't answering", "account.claktory.com isn't answering since 10:02"));
        assert!(folds("The site is down", "The site is down"));
        assert!(folds("The website is down", "Website down"));
        assert!(!folds("Down", "Down"), "one word says too little");
    }

    #[test]
    fn a_title_is_its_words_stemmed_without_numbers_or_stop_words() {
        let words: Vec<String> = concern("Fugo's 3 tests: answering, answered at 09:10 UTC, STOPPED").into_iter().collect();
        assert_eq!(words, ["answer", "fugo", "stopp", "test", "utc"]);
    }
}
