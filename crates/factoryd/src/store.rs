//! Factory's records, in `<home>/factory.db` (SQLite): every message an agent
//! was sent and what became of it, every run, every background task, and the
//! app's state. Settings stay in config.json, which people and agents edit,
//! and conversations stay in each agent's folder.
//!
//! What's waiting for an agent and which agents are working are read back
//! from here when Factory starts, so work carries on across restarts.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::sync::{Mutex, MutexGuard};

use factory_agent::{Event, EventSource, Finished, Task};
use rusqlite::{Connection, OptionalExtension, Row, params, types::FromSql};

use crate::characters::Presence;
use crate::config::{FolderView, State};
use crate::factory::Queued;
use crate::transcript::now_ms;

const SCHEMA: &str = "
-- Everything an agent was sent: the person's messages and events.
CREATE TABLE messages (
    id INTEGER PRIMARY KEY,
    agent TEXT NOT NULL,
    -- person, factory, agent or task
    source TEXT NOT NULL,
    -- Who sent it: an agent's name, a task's id, Factory; '' for the person.
    sender TEXT NOT NULL,
    sender_id TEXT NOT NULL,
    -- One line for people; '' for the person's messages.
    summary TEXT NOT NULL,
    -- What the agent reads.
    text TEXT NOT NULL,
    -- Milliseconds since 1970.
    created INTEGER NOT NULL,
    -- When the agent got it, and in which run.
    delivered INTEGER,
    run INTEGER,
    -- When it was dropped instead: the person stopped the agent, the agent
    -- was deleted, or its model couldn't run.
    dropped INTEGER
);
CREATE INDEX messages_by_agent ON messages (agent, id);

-- Each stretch of work: from an agent taking a message until it has nothing
-- left to do.
CREATE TABLE runs (
    id INTEGER PRIMARY KEY,
    agent TEXT NOT NULL,
    -- The version of Factory that ran it.
    version TEXT NOT NULL,
    started INTEGER NOT NULL,
    ended INTEGER,
    -- finished, stopped (by the person), deleted (the agent was), or
    -- interrupted (Factory stopped; the agent carried on in a new run)
    outcome TEXT
);
CREATE INDEX runs_by_agent ON runs (agent, id);

-- Commands agents ran in the background.
CREATE TABLE tasks (
    id INTEGER PRIMARY KEY,
    agent TEXT NOT NULL,
    -- task-1, task-2… for each agent
    task TEXT NOT NULL,
    command TEXT NOT NULL,
    -- Its process group.
    pid INTEGER NOT NULL,
    log TEXT NOT NULL,
    started INTEGER NOT NULL,
    ended INTEGER,
    -- exit 0, signal SIGTERM, timed out, stopped…
    status TEXT
);

-- What the app shows, the same on every device: selected_agent,
-- focused_project and open_panel.
CREATE TABLE state (key TEXT PRIMARY KEY, value TEXT NOT NULL);
-- Open folders in each project's explorer.
CREATE TABLE expanded (project TEXT NOT NULL, folder TEXT NOT NULL, PRIMARY KEY (project, folder));
-- Projects whose explorer shows everything, hidden files included.
CREATE TABLE show_all (project TEXT PRIMARY KEY);
";

/// Changes to the schema since `SCHEMA`, in order: database version n+1 has
/// the first n+1 of them. Older backends (after a rollback) ignore tables they
/// don't know.
const MIGRATIONS: [&str; 26] = [
    "
-- Agents with a run that finished or failed since the person last opened them.
CREATE TABLE unseen (agent TEXT PRIMARY KEY);
",
    "
-- The Checklist extension: each agent's items as they are now…
CREATE TABLE checklist_items (
    agent TEXT NOT NULL,
    -- #1, #2… for each agent
    id INTEGER NOT NULL,
    text TEXT NOT NULL,
    -- open, in_progress, done or dropped
    status TEXT NOT NULL,
    -- The agent on it; '' when unassigned.
    owner TEXT NOT NULL,
    created INTEGER NOT NULL,
    updated INTEGER NOT NULL,
    PRIMARY KEY (agent, id)
);
-- …and everything that happened to them, never changed afterwards.
CREATE TABLE checklist_events (
    seq INTEGER PRIMARY KEY,
    agent TEXT NOT NULL,
    item INTEGER NOT NULL,
    time INTEGER NOT NULL,
    -- Who did it: an agent id; '' for the person.
    actor TEXT NOT NULL,
    -- created, status, owner, edited or note
    kind TEXT NOT NULL,
    -- The new status, owner or text, or the note.
    value TEXT NOT NULL
);
CREATE INDEX checklist_events_by_item ON checklist_events (agent, item, seq);
CREATE INDEX checklist_events_by_time ON checklist_events (time);
",
    "
-- Agents whose nested agents are folded away in the sidebar.
CREATE TABLE collapsed (agent TEXT PRIMARY KEY);
",
    crate::extensions::memory::SCHEMA,
    "
-- Project folders whose agents are folded away in the sidebar.
CREATE TABLE collapsed_folders (project TEXT NOT NULL, folder TEXT NOT NULL, PRIMARY KEY (project, folder));
",
    "
-- The extension whose panel is open beside each agent. The panel that was
-- open for everyone stays open beside the agent that was selected.
CREATE TABLE open_panels (agent TEXT PRIMARY KEY, extension TEXT NOT NULL);
INSERT INTO open_panels (agent, extension)
    SELECT selected.value, panel.value FROM state AS selected, state AS panel
    WHERE selected.key = 'selected_agent' AND panel.key = 'open_panel';
",
    "
-- When admin agents were told a task had run long; null until then.
ALTER TABLE tasks ADD COLUMN alerted INTEGER;
",
    crate::extensions::monitors::SCHEMA,
    crate::characters::SCHEMA,
    crate::proposals::SCHEMA,
    crate::extensions::viewer::SCHEMA,
    crate::account::analytics::SCHEMA,
    "
-- The device the person sent a message from, as JSON ({\"device\", \"kind\"});
-- null for events, and for messages from before it.
ALTER TABLE messages ADD COLUMN sent_from TEXT;
",
    "
-- The id the person's app made up for the message; null without one.
ALTER TABLE messages ADD COLUMN client_id TEXT;
",
    "
-- The files the person attached to the message, as JSON; null without any.
ALTER TABLE messages ADD COLUMN attachments TEXT;
",
    crate::annotations::SCHEMA,
    crate::annotations::SUGGESTIONS,
    crate::usage_report::SCHEMA,
    crate::extensions::devices::SCHEMA,
    crate::onboarding::SCHEMA,
    crate::pipe_link::SCHEMA,
    crate::pipes::SCHEMA,
    crate::speech::voice::SCHEMA,
    crate::extensions::ask::SCHEMA,
    crate::chats::SCHEMA,
    crate::move_import::SCHEMA,
];

/// A background task as recorded.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct TaskRow {
    pub agent: String,
    pub task: String,
    pub command: String,
    pub pid: u32,
    pub log: String,
    pub started: i64,
    /// None while it runs.
    pub ended: Option<i64>,
    /// "exit 0", "signal SIGTERM", "stopped by Rivet"…; None while it runs.
    pub status: Option<String>,
}

impl TaskRow {
    fn read(row: &rusqlite::Row<'_>) -> Result<TaskRow, rusqlite::Error> {
        Ok(TaskRow {
            agent: row.get(0)?,
            task: row.get(1)?,
            command: row.get(2)?,
            pid: row.get(3)?,
            log: row.get(4)?,
            started: row.get(5)?,
            ended: row.get(6)?,
            status: row.get(7)?,
        })
    }
}

/// By agent: when its latest run started, and when its latest run ended.
pub(crate) type RunTimes = std::collections::HashMap<String, (Option<i64>, Option<i64>)>;

/// How a message reached the agent, or didn't.
pub(crate) enum Outcome {
    Delivered { run: Option<i64> },
    Dropped,
}

pub(crate) struct Store {
    db: Mutex<Connection>,
}

/// The schema version this backend writes: a database above it was made by
/// a newer one.
pub(crate) fn schema_version() -> i64 {
    MIGRATIONS.len() as i64 + 1
}

/// Creates version 1 when needed, then applies later schemas in order, each in its own transaction.
pub(crate) fn migrate(db: &Connection, schema: &str, migrations: &[&str]) -> rusqlite::Result<()> {
    let version: i64 = db.pragma_query_value(None, "user_version", |row| row.get(0))?;
    if version == 0 {
        db.execute_batch(&format!("BEGIN; {schema} PRAGMA user_version = 1; COMMIT;"))?;
    }
    for (index, migration) in migrations.iter().enumerate() {
        let target = index as i64 + 2;
        if version < target {
            db.execute_batch(&format!("BEGIN; {migration} PRAGMA user_version = {target}; COMMIT;"))?;
        }
    }
    Ok(())
}

/// Positional row readers: convert columns in order, returning the first error.
pub(crate) fn row2<A: FromSql, B: FromSql>(row: &Row<'_>) -> rusqlite::Result<(A, B)> {
    Ok((row.get(0)?, row.get(1)?))
}

pub(crate) fn row3<A: FromSql, B: FromSql, C: FromSql>(row: &Row<'_>) -> rusqlite::Result<(A, B, C)> {
    Ok((row.get(0)?, row.get(1)?, row.get(2)?))
}

pub(crate) fn row4<A: FromSql, B: FromSql, C: FromSql, D: FromSql>(row: &Row<'_>) -> rusqlite::Result<(A, B, C, D)> {
    Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
}

impl Store {
    pub(crate) fn open(path: &Path) -> Result<Store, rusqlite::Error> {
        let db = Connection::open(path)?;
        db.pragma_update(None, "journal_mode", "WAL")?;
        db.pragma_update(None, "synchronous", "NORMAL")?;
        db.busy_timeout(std::time::Duration::from_secs(5))?;
        migrate(&db, SCHEMA, &MIGRATIONS)?;
        Ok(Store { db: Mutex::new(db) })
    }

    pub(crate) fn db(&self) -> MutexGuard<'_, Connection> {
        self.db.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Records a message for an agent; returns its id.
    pub(crate) fn add_message(&self, agent: &str, queued: &Queued) -> Result<i64, rusqlite::Error> {
        let source = source_of(queued.event.as_ref());
        let (sender, sender_id, summary) = match &queued.event {
            None => ("", "", ""),
            Some(event) => (event.from.as_str(), event.id.as_str(), event.summary.as_str()),
        };
        let sent_from = queued.sent_from.as_ref().and_then(|from| serde_json::to_string(from).ok());
        let client_id = Some(queued.client_id.as_str()).filter(|id| !id.is_empty());
        let attachments = Some(&queued.attachments).filter(|list| !list.is_empty()).and_then(|list| serde_json::to_string(list).ok());
        let voice = queued.voice.as_ref().and_then(|voice| serde_json::to_string(voice).ok());
        let db = self.db();
        db.execute(
            "INSERT INTO messages (agent, source, sender, sender_id, summary, text, created, sent_from, client_id, attachments, voice) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            params![agent, source, sender, sender_id, summary, queued.text, queued.created, sent_from, client_id, attachments, voice],
        )?;
        Ok(db.last_insert_rowid())
    }

    /// A voice message's transcription changed: what was heard, and how.
    pub(crate) fn set_voice(&self, id: i64, text: &str, voice: &crate::speech::voice::Voice) -> Result<(), rusqlite::Error> {
        let voice = serde_json::to_string(voice).unwrap_or_default();
        self.db().execute("UPDATE messages SET text = ?2, voice = ?3 WHERE id = ?1", params![id, text, voice])?;
        Ok(())
    }

    pub(crate) fn settle(&self, messages: &[Queued], outcome: Outcome) -> Result<(), rusqlite::Error> {
        let mut db = self.db();
        let transaction = db.transaction()?;
        let now = now_ms();
        for queued in messages {
            match outcome {
                Outcome::Delivered { run } => transaction
                    .execute("UPDATE messages SET delivered = ?2, run = ?3, dropped = NULL WHERE id = ?1", params![queued.id, now, run])?,
                Outcome::Dropped => {
                    transaction.execute("UPDATE messages SET dropped = ?2, delivered = NULL WHERE id = ?1", params![queued.id, now])?
                }
            };
        }
        transaction.commit()
    }

    /// Messages not yet delivered or dropped, oldest first, by agent.
    pub(crate) fn waiting(&self) -> Result<BTreeMap<String, Vec<Queued>>, rusqlite::Error> {
        let db = self.db();
        let mut statement = db.prepare(
            "SELECT id, agent, source, sender, sender_id, summary, text, created, sent_from, client_id, attachments, voice FROM messages WHERE delivered IS NULL AND dropped IS NULL ORDER BY id",
        )?;
        let mut rows = statement.query([])?;
        let mut waiting: BTreeMap<String, Vec<Queued>> = BTreeMap::new();
        while let Some(row) = rows.next()? {
            let source: String = row.get(2)?;
            let event = match source_named(&source) {
                None => None,
                Some(source) => Some(Event { source, from: row.get(3)?, id: row.get(4)?, summary: row.get(5)?, transient: false }),
            };
            let sent_from: Option<String> = row.get(8)?;
            let sent_from = sent_from.and_then(|from| serde_json::from_str(&from).ok());
            waiting.entry(row.get(1)?).or_default().push(Queued {
                id: row.get(0)?,
                text: row.get(6)?,
                event,
                created: row.get(7)?,
                sent_from,
                client_id: row.get::<_, Option<String>>(9)?.unwrap_or_default(),
                attachments: row.get::<_, Option<String>>(10)?.and_then(|list| serde_json::from_str(&list).ok()).unwrap_or_default(),
                voice: row.get::<_, Option<String>>(11)?.and_then(|voice| serde_json::from_str(&voice).ok()),
            });
        }
        Ok(waiting)
    }

    pub(crate) fn start_run(&self, agent: &str, version: &str) -> Result<i64, rusqlite::Error> {
        let db = self.db();
        db.execute("INSERT INTO runs (agent, version, started) VALUES (?1, ?2, ?3)", params![agent, version, now_ms()])?;
        Ok(db.last_insert_rowid())
    }

    /// Ends the run, and with it its agent's presence: idle from now, failed
    /// or not. Returns the agent and its presence, unless the run had ended
    /// already or ended because the agent is deleted.
    pub(crate) fn end_run(&self, run: i64, outcome: &str) -> Result<Option<(String, Presence)>, rusqlite::Error> {
        let mut db = self.db();
        let transaction = db.transaction()?;
        let now = now_ms();
        let ended =
            transaction.execute("UPDATE runs SET ended = ?2, outcome = ?3 WHERE id = ?1 AND ended IS NULL", params![run, now, outcome])?;
        let mut presence = None;
        if ended == 1 && outcome != "deleted" {
            let agent: String = transaction.query_row("SELECT agent FROM runs WHERE id = ?1", [run], |row| row.get(0))?;
            let now = Presence { idle_since: now, failed: outcome == "failed" };
            crate::characters::set_presence(&transaction, &agent, now)?;
            presence = Some((agent, now));
        }
        transaction.commit()?;
        Ok(presence)
    }

    /// Runs that never ended because Factory stopped: by agent, the version
    /// that was running them. Marks them interrupted.
    /// Runs the last Factory left unfinished, ended now as interrupted: for
    /// each agent, the version that ran it and when it started.
    pub(crate) fn interrupted_runs(&self) -> Result<BTreeMap<String, (String, i64)>, rusqlite::Error> {
        let mut db = self.db();
        let transaction = db.transaction()?;
        let mut runs = BTreeMap::new();
        {
            let mut statement = transaction.prepare("SELECT agent, version, started FROM runs WHERE ended IS NULL ORDER BY id")?;
            let mut rows = statement.query([])?;
            while let Some(row) = rows.next()? {
                runs.insert(row.get(0)?, (row.get(1)?, row.get(2)?));
            }
        }
        transaction.execute("UPDATE runs SET ended = ?1, outcome = 'interrupted' WHERE ended IS NULL", [now_ms()])?;
        transaction.commit()?;
        Ok(runs)
    }

    pub(crate) fn start_task(&self, agent: &str, task: &Task) -> Result<(), rusqlite::Error> {
        self.db().execute(
            "INSERT INTO tasks (agent, task, command, pid, log, started) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![agent, task.id, task.command, task.pid, task.log.to_string_lossy(), now_ms()],
        )?;
        Ok(())
    }

    pub(crate) fn end_task(&self, agent: &str, finished: &Finished) -> Result<(), rusqlite::Error> {
        self.db().execute(
            "UPDATE tasks SET ended = ?3, status = ?4 WHERE agent = ?1 AND task = ?2 AND ended IS NULL",
            params![agent, finished.id, now_ms(), finished.status],
        )?;
        Ok(())
    }

    /// Background tasks that hadn't ended when Factory last stopped, by agent.
    /// Whether the agent's background task runs now.
    pub(crate) fn task_running(&self, agent: &str, task: &str) -> bool {
        self.db()
            .query_row("SELECT 1 FROM tasks WHERE agent = ?1 AND task = ?2 AND ended IS NULL", [agent, task], |_| Ok(()))
            .optional()
            .is_ok_and(|found| found.is_some())
    }

    pub(crate) fn running_tasks(&self) -> Result<Vec<(String, Task)>, rusqlite::Error> {
        let db = self.db();
        let mut statement = db.prepare("SELECT agent, task, command, pid, log, started FROM tasks WHERE ended IS NULL ORDER BY id")?;
        let mut rows = statement.query([])?;
        let mut tasks = Vec::new();
        while let Some(row) = rows.next()? {
            let log: String = row.get(4)?;
            let started: i64 = row.get(5)?;
            let task = Task {
                id: row.get(1)?,
                command: row.get(2)?,
                pid: row.get(3)?,
                log: log.into(),
                started: u64::try_from(started).unwrap_or(0),
            };
            tasks.push((row.get(0)?, task));
        }
        Ok(tasks)
    }

    /// Background tasks, newest first: those of `agent` (every agent's
    /// with None) that are running, or ended since `ended_since`.
    pub(crate) fn tasks(&self, agent: Option<&str>, ended_since: i64) -> Result<Vec<TaskRow>, rusqlite::Error> {
        let db = self.db();
        let mut statement = db.prepare(
            "SELECT agent, task, command, pid, log, started, ended, status FROM tasks
             WHERE (?1 IS NULL OR agent = ?1) AND (ended IS NULL OR ended >= ?2) ORDER BY id DESC",
        )?;
        let rows = statement.query_map(params![agent, ended_since], TaskRow::read)?;
        rows.collect()
    }

    /// Running background tasks that started before `before` and that admin
    /// agents weren't told of yet, oldest first.
    pub(crate) fn long_tasks(&self, before: i64) -> Result<Vec<TaskRow>, rusqlite::Error> {
        let db = self.db();
        let mut statement = db.prepare(
            "SELECT agent, task, command, pid, log, started, ended, status FROM tasks
             WHERE ended IS NULL AND alerted IS NULL AND started < ?1 ORDER BY id",
        )?;
        let rows = statement.query_map([before], TaskRow::read)?;
        rows.collect()
    }

    pub(crate) fn task_alerted(&self, agent: &str, task: &str) -> Result<(), rusqlite::Error> {
        self.db()
            .execute("UPDATE tasks SET alerted = ?3 WHERE agent = ?1 AND task = ?2 AND ended IS NULL", params![agent, task, now_ms()])?;
        Ok(())
    }

    /// When each agent's latest run started, and when its latest run ended.
    pub(crate) fn run_times(&self) -> Result<RunTimes, rusqlite::Error> {
        let db = self.db();
        let mut statement = db.prepare("SELECT agent, MAX(started), MAX(ended) FROM runs GROUP BY agent")?;
        let mut rows = statement.query([])?;
        let mut times = std::collections::HashMap::new();
        while let Some(row) = rows.next()? {
            times.insert(row.get(0)?, (row.get(1)?, row.get(2)?));
        }
        Ok(times)
    }

    pub(crate) fn state(&self) -> Result<State, rusqlite::Error> {
        let db = self.db();
        let value = |key: &str| db.query_row("SELECT value FROM state WHERE key = ?1", [key], |row| row.get::<_, String>(0)).optional();
        let mut state = State {
            selected_agent: value("selected_agent")?,
            focused_project: value("focused_project")?,
            explorer: BTreeMap::new(),
            unseen: BTreeSet::new(),
            open_panels: BTreeMap::new(),
            collapsed: BTreeSet::new(),
            collapsed_folders: BTreeSet::new(),
        };
        let mut statement = db.prepare("SELECT project, folder FROM expanded ORDER BY project, folder")?;
        let mut rows = statement.query([])?;
        while let Some(row) = rows.next()? {
            state.explorer.entry(row.get(0)?).or_default().expanded.insert(row.get(1)?);
        }
        let mut statement = db.prepare("SELECT project FROM show_all")?;
        let mut rows = statement.query([])?;
        while let Some(row) = rows.next()? {
            state.explorer.entry(row.get(0)?).or_default().show_all = true;
        }
        let mut statement = db.prepare("SELECT agent FROM unseen")?;
        let mut rows = statement.query([])?;
        while let Some(row) = rows.next()? {
            state.unseen.insert(row.get(0)?);
        }
        let mut statement = db.prepare("SELECT agent FROM collapsed")?;
        let mut rows = statement.query([])?;
        while let Some(row) = rows.next()? {
            state.collapsed.insert(row.get(0)?);
        }
        let mut statement = db.prepare("SELECT agent, extension FROM open_panels")?;
        let mut rows = statement.query([])?;
        while let Some(row) = rows.next()? {
            state.open_panels.insert(row.get(0)?, row.get(1)?);
        }
        let mut statement = db.prepare("SELECT project, folder FROM collapsed_folders")?;
        let mut rows = statement.query([])?;
        while let Some(row) = rows.next()? {
            state.collapsed_folders.insert((row.get(0)?, row.get(1)?));
        }
        Ok(state)
    }

    pub(crate) fn save_state(&self, state: &State) -> Result<(), rusqlite::Error> {
        let mut db = self.db();
        let transaction = db.transaction()?;
        transaction
            .execute_batch("DELETE FROM state; DELETE FROM expanded; DELETE FROM show_all; DELETE FROM unseen; DELETE FROM collapsed; DELETE FROM collapsed_folders; DELETE FROM open_panels;",
        )?;
        // `open_panel` keeps the selected agent's, for an older Factory after a rollback.
        let open_panel = state.selected_agent.as_ref().and_then(|agent| state.open_panels.get(agent)).cloned();
        for (key, value) in
            [("selected_agent", &state.selected_agent), ("focused_project", &state.focused_project), ("open_panel", &open_panel)]
        {
            if let Some(value) = value {
                transaction.execute("INSERT INTO state (key, value) VALUES (?1, ?2)", [key, value])?;
            }
        }
        for (project, FolderView { expanded, show_all }) in &state.explorer {
            for folder in expanded {
                transaction.execute("INSERT INTO expanded (project, folder) VALUES (?1, ?2)", [project, folder])?;
            }
            if *show_all {
                transaction.execute("INSERT INTO show_all (project) VALUES (?1)", [project])?;
            }
        }
        for agent in &state.unseen {
            transaction.execute("INSERT INTO unseen (agent) VALUES (?1)", [agent])?;
        }
        for agent in &state.collapsed {
            transaction.execute("INSERT INTO collapsed (agent) VALUES (?1)", [agent])?;
        }
        for (agent, extension) in &state.open_panels {
            transaction.execute("INSERT INTO open_panels (agent, extension) VALUES (?1, ?2)", [agent, extension])?;
        }
        for (project, folder) in &state.collapsed_folders {
            transaction.execute("INSERT INTO collapsed_folders (project, folder) VALUES (?1, ?2)", [project, folder])?;
        }
        transaction.commit()
    }
}

/// How messages.source names who a message is from.
pub(crate) fn source_of(event: Option<&Event>) -> &'static str {
    event.map_or("person", |event| source_name(event.source))
}

fn source_name(source: EventSource) -> &'static str {
    match source {
        EventSource::Host => "factory",
        EventSource::Agent => "agent",
        EventSource::Task => "task",
        EventSource::Monitor => "monitor",
        EventSource::App => "app",
        EventSource::Pipe => "pipe",
    }
}

/// None for the person.
fn source_named(name: &str) -> Option<EventSource> {
    match name {
        "factory" => Some(EventSource::Host),
        "agent" => Some(EventSource::Agent),
        "task" => Some(EventSource::Task),
        "monitor" => Some(EventSource::Monitor),
        "app" => Some(EventSource::App),
        "pipe" => Some(EventSource::Pipe),
        _ => None,
    }
}
