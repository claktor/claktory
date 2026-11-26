//! Alerts kept in Needs you (Bruno 44235): every alert, an agent's
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
        "SELECT id, agent, from_name, title, text, urgent, withheld, sent, seq FROM alerts WHERE read IS NULL ORDER BY sent, rowid",
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
        "INSERT INTO alerts (id, agent, from_name, title, text, urgent, withheld, sent, seq) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        params![
            alert.id,
            alert.agent_id,
            alert.from_name,
            alert.title,
            alert.text,
            alert.urgent,
            alert.withheld,
            alert.sent_ms,
            i64::try_from(alert.seq).unwrap_or(i64::MAX)
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
/// hello said). Ids already read or unknown are skipped.
pub(crate) fn read(factory: &Factory, ids: &[String], device_id: &str, device_name: &str) -> Result<(), RequestError> {
    let now = now_ms();
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
        "SELECT id, agent, from_name, title, text, urgent, withheld, sent, seq, read, read_device, read_device_name FROM alerts
         WHERE read IS NOT NULL AND read >= ?1 AND read < ?2 ORDER BY read DESC, rowid DESC LIMIT ?3",
    )?;
    let rows = statement.query_map(params![since, before, limit], |row| {
        let read: i64 = row.get(9)?;
        Ok(Read {
            alert: read_alert(row)?,
            read,
            on: v1::AnsweredOn { device_id: row.get(10)?, device_name: row.get(11)?, at_ms: read },
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
