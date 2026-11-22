//! Monitors: prompts that wake an agent later. A monitor is a prompt the
//! agent wrote plus a condition (`when`): a time, a schedule, a command's
//! result, files changing, or a running command's output. When it fires, the
//! agent gets the prompt as an event from the monitor, with what happened.
//!
//! Monitors live in factory.db, so they last across restarts, compactions
//! and /clear, until they're done or cancelled. They can't loop: a monitor
//! never has more than one fire waiting to be read (what happens meanwhile is
//! carried into the next), every kind has a shortest period, and an agent's
//! monitors fire at most `BUDGET` times an hour before the one going over is
//! paused, with one notice.
//!
//! `when` reads what a monitor waits for; `run` is the scheduler that fires
//! them.

mod run;
pub(crate) mod when;

use std::fmt::Write as _;
use std::sync::Arc;

use factory_agent::{Tool, ToolResult};
use factory_protocol::v1;
use rusqlite::{OptionalExtension, params};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

pub(crate) use run::{State, monitor};
use when::When;

use super::{Extension, Manifest, PromptKind, PromptSection};
use crate::config::{AgentConfig, Config};
use crate::factory::{Factory, Inner, RequestError};
use crate::store::Store;

pub(crate) const ID: &str = "monitors";

/// Monitors an agent may have active at once…
const MAX_ACTIVE: usize = 20;
/// …of which `watch` ones, each a running process.
const MAX_WATCHES: usize = 5;
const MAX_PROMPT: usize = 2_000;
const MAX_NAME: usize = 60;
/// Ended monitors `all` lists, for this long.
const ENDED_LISTED_MS: i64 = 7 * 24 * 60 * 60 * 1000;

static MANIFEST: Manifest = Manifest {
    id: ID,
    name: "Monitors",
    icon: "alarm-clock",
    description: "Lets it wake itself later: at a set time, on a schedule, or when something changes.",
    default_enabled: true,
    options: &[],
};

const PROMPT: &str = "## Monitors

A monitor wakes you later with a prompt you write: at a time (at), on an interval since its last fire (every), on a cron schedule (cron), when a command you run periodically succeeds, fails, matches or changes (check), when files change (files), or when a long-running command prints lines (watch). Monitors last until done or cancelled, across restarts and compactions. Each fire arrives as an event from the monitor with your prompt and what happened, so write the prompt so it makes sense on its own. Every fire makes you read your whole context again, so use the slowest schedule that works and prefer check, files or watch to polling on a short interval. Background tasks already wake you when they end; don't add monitors for them. Cancel monitors you no longer need.
";

const ADD: &str = "Add monitors, all or none. Each: name (short), prompt (what to do when it fires; self-contained), when, and optionally until (a time), max_fires, idle_only (wait until you're not working; default true for at/every/cron), agent (set it for another agent you can message). when is one of: {\"at\": \"2026-11-19 09:00\"} or {\"in\": \"45m\"}; {\"every\": \"24h\", \"start\"?: time}; {\"cron\": \"0 9 * * 1-5\"} (local time); {\"check\": \"<command>\", \"every\": \"5m\", \"fire_on\": \"success|failure|match|changed\", \"match\"?: regex, \"repeat\"?: true (each time it becomes true again) or \"always\" (at every check it holds)} (the command gets $FACTORY_MONITOR_LAST_FIRED: unix seconds of the last fire, or of when it was added); {\"files\": [paths], \"glob\"?: \"*.rs\"}; {\"watch\": \"<long-running command>\", \"match\"?: regex, \"gap\"?: \"30s\"}. Commands run in your folder.";
const LIST: &str = "Your monitors, one line each, and what else can wake you. all: monitors that ended in the last 7 days too. agent: another agent's, if you can message it.";
const CHANGE: &str = "Change monitors: changes is a list of {id (\"m3\"), action: pause, resume, cancel or fire (fire now, to try the prompt)}, and agent for another agent's.";

/// The migration that added `monitors.taint`: the pipes a monitor's
/// creator had read when it set it, as JSON ({"<pipe id>": [route]}),
/// spread to the agent at each fire, even if the creator's conversation
/// was cleared meanwhile. Null when the creator had read none, and for
/// monitors from before it was kept.
pub(crate) const TAINT: &str = "
ALTER TABLE monitors ADD COLUMN taint TEXT;
";

pub(crate) const SCHEMA: &str = "
-- The Monitors extension: each agent's monitors as they are now…
CREATE TABLE monitors (
    agent TEXT NOT NULL,
    -- m1, m2… for each agent
    id INTEGER NOT NULL,
    name TEXT NOT NULL,
    prompt TEXT NOT NULL,
    -- at, every, cron, check, files or watch
    kind TEXT NOT NULL,
    -- What it waits for, as JSON.
    spec TEXT NOT NULL,
    -- When it ends by itself, and after how many fires; null if never.
    until INTEGER,
    max_fires INTEGER,
    -- Its fires wait until the agent isn't working.
    idle_only INTEGER NOT NULL,
    -- Who set it: an agent id; '' for the person.
    created_by TEXT NOT NULL,
    created INTEGER NOT NULL,
    -- active, paused, done, cancelled or error
    state TEXT NOT NULL,
    -- Why it's paused, done or in error.
    reason TEXT NOT NULL,
    -- When it's next due: a time monitor's next fire, a check's next run.
    next_due INTEGER,
    last_fired INTEGER,
    fires INTEGER NOT NULL,
    -- What happened while its last fire waited to be read, for the next one.
    carry TEXT NOT NULL,
    -- What it keeps between runs, as JSON: a check's last result, a watch's
    -- process and how far its log was read.
    cursor TEXT NOT NULL,
    updated INTEGER NOT NULL,
    PRIMARY KEY (agent, id)
);
-- …and everything that happened to them, never changed afterwards.
CREATE TABLE monitor_events (
    seq INTEGER PRIMARY KEY,
    agent TEXT NOT NULL,
    monitor INTEGER NOT NULL,
    time INTEGER NOT NULL,
    -- Who did it: an agent id; '' for the person or for Factory.
    actor TEXT NOT NULL,
    -- created, fired, paused, resumed, cancelled, done or error
    kind TEXT NOT NULL,
    -- What fired it, or why it changed.
    value TEXT NOT NULL
);
CREATE INDEX monitor_events_by_monitor ON monitor_events (agent, monitor, seq);
CREATE INDEX monitor_events_by_time ON monitor_events (agent, kind, time);
";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Status {
    Active,
    Paused,
    Done,
    Cancelled,
    Error,
}

impl Status {
    pub(crate) fn name(self) -> &'static str {
        match self {
            Status::Active => "active",
            Status::Paused => "paused",
            Status::Done => "done",
            Status::Cancelled => "cancelled",
            Status::Error => "error",
        }
    }

    fn parse(name: &str) -> Status {
        match name {
            "active" => Status::Active,
            "paused" => Status::Paused,
            "done" => Status::Done,
            "cancelled" => Status::Cancelled,
            _ => Status::Error,
        }
    }

    /// It won't fire again unless resumed.
    fn ended(self) -> bool {
        matches!(self, Status::Done | Status::Cancelled)
    }
}

/// What a monitor keeps between runs.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct Cursor {
    /// A check's condition at its last run.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_true: Option<bool>,
    /// A check's output at its last run, hashed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hash: Option<String>,
    /// A check's runs in a row that couldn't run.
    #[serde(default)]
    pub failures: u32,
    /// A watch's process group while it runs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pid: Option<u32>,
    /// When the watch's process started.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started: Option<i64>,
    /// How far the watch's log has been read into fires.
    #[serde(default)]
    pub offset: u64,
    /// The watch's runs in a row that ended within a minute.
    #[serde(default)]
    pub quick_runs: u32,
    /// When to start the watch's process again after it ended.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub restart_at: Option<i64>,
    /// What's carried includes a fire that should have been its last.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub last_after_carry: bool,
}

/// A monitor as it is now.
#[derive(Clone, Debug)]
pub(crate) struct Monitor {
    pub agent: String,
    pub id: u32,
    pub name: String,
    pub prompt: String,
    pub when: When,
    pub until: Option<i64>,
    pub max_fires: Option<u32>,
    pub idle_only: bool,
    pub created_by: String,
    /// The creator's taint when it was set (`Pipes::taint_of`, as JSON);
    /// None when the creator had read no pipe, and for old rows.
    pub taint: Option<String>,
    pub created: i64,
    pub status: Status,
    pub reason: String,
    pub next_due: Option<i64>,
    pub last_fired: Option<i64>,
    pub fires: u32,
    pub carry: String,
    pub cursor: Cursor,
}

/// Which monitors to read. Every read of monitor data goes through `query`.
#[derive(Default)]
pub(crate) struct Query<'a> {
    /// None for every agent's.
    pub agent: Option<&'a str>,
    pub id: Option<u32>,
    /// Only active ones (the scheduler's).
    pub active: bool,
    /// Ended ones too, if they ended since then.
    pub ended_since: Option<i64>,
}

const COLUMNS: &str = "agent, id, name, prompt, spec, until, max_fires, idle_only, created_by, created, state, reason, next_due, last_fired, fires, carry, cursor, taint";

/// The monitors `query` asks for, by agent and id. Not-yet-ended ones
/// always; ended ones only with `ended_since`.
pub(crate) fn query(store: &Store, query: &Query) -> rusqlite::Result<Vec<Monitor>> {
    let db = store.db();
    let states = if query.active { "state = 'active'" } else { "(state NOT IN ('done', 'cancelled') OR updated >= ?3)" };
    let mut statement = db.prepare(&format!(
        "SELECT {COLUMNS} FROM monitors WHERE (?1 IS NULL OR agent = ?1) AND (?2 IS NULL OR id = ?2) AND {states} ORDER BY agent, id"
    ))?;
    let since = query.ended_since.unwrap_or(i64::MAX);
    let rows = if query.active {
        statement.query_map(params![query.agent, query.id], read)?.collect::<Result<Vec<_>, _>>()?
    } else {
        statement.query_map(params![query.agent, query.id, since], read)?.collect::<Result<Vec<_>, _>>()?
    };
    Ok(rows.into_iter().flatten().collect())
}

/// A row; None (logged) when its spec can't be read, such as one from a newer Factory.
fn read(row: &rusqlite::Row<'_>) -> rusqlite::Result<Option<Monitor>> {
    let spec: String = row.get(4)?;
    let Ok(when) = serde_json::from_str::<When>(&spec) else {
        tracing::warn!(agent = row.get::<_, String>(0)?, id = row.get::<_, u32>(1)?, "a monitor's condition can't be read; skipped");
        return Ok(None);
    };
    let cursor: String = row.get(16)?;
    Ok(Some(Monitor {
        agent: row.get(0)?,
        id: row.get(1)?,
        name: row.get(2)?,
        prompt: row.get(3)?,
        when,
        until: row.get(5)?,
        max_fires: row.get(6)?,
        idle_only: row.get(7)?,
        created_by: row.get(8)?,
        taint: row.get(17)?,
        created: row.get(9)?,
        status: Status::parse(&row.get::<_, String>(10)?),
        reason: row.get(11)?,
        next_due: row.get(12)?,
        last_fired: row.get(13)?,
        fires: row.get(14)?,
        carry: row.get(15)?,
        cursor: serde_json::from_str(&cursor).unwrap_or_default(),
    }))
}

/// A monitor to add.
pub(crate) struct New {
    pub agent: String,
    pub name: String,
    pub prompt: String,
    pub when: When,
    pub next_due: Option<i64>,
    pub until: Option<i64>,
    pub max_fires: Option<u32>,
    pub idle_only: bool,
    /// The creator's taint now, kept with the monitor (see `Monitor::taint`).
    pub taint: Option<String>,
}

/// Adds monitors, all or none; returns their ids.
pub(crate) fn add(store: &Store, actor: &str, monitors: &[New], now: i64) -> rusqlite::Result<Vec<u32>> {
    let mut db = store.db();
    let transaction = db.transaction()?;
    let mut ids = Vec::new();
    for monitor in monitors {
        let id: u32 =
            transaction.query_row("SELECT COALESCE(MAX(id), 0) + 1 FROM monitors WHERE agent = ?1", [&monitor.agent], |row| row.get(0))?;
        let spec = serde_json::to_string(&monitor.when).unwrap_or_default();
        transaction.execute(
            &format!(
                "INSERT INTO monitors ({COLUMNS}, kind, updated) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, 'active', '', ?11, NULL, 0, '', '{{}}', ?13, ?12, ?10)"
            ),
            params![
                monitor.agent,
                id,
                monitor.name,
                monitor.prompt,
                spec,
                monitor.until,
                monitor.max_fires,
                monitor.idle_only,
                actor,
                now,
                monitor.next_due,
                monitor.when.kind(),
                monitor.taint
            ],
        )?;
        record(&transaction, &monitor.agent, id, now, actor, "created", &monitor.when.describe(now))?;
        ids.push(id);
    }
    transaction.commit()?;
    Ok(ids)
}

fn record(db: &rusqlite::Connection, agent: &str, monitor: u32, time: i64, actor: &str, kind: &str, value: &str) -> rusqlite::Result<()> {
    db.execute(
        "INSERT INTO monitor_events (agent, monitor, time, actor, kind, value) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![agent, monitor, time, actor, kind, value],
    )?;
    Ok(())
}

/// Saves the monitor as it is now, and records `event` (kind, value) with
/// it. With `while_active` (the scheduler's saves), only if it's still
/// active, so a change made meanwhile (cancelled, paused, deleted) stands.
/// Says whether it saved.
pub(crate) fn save(
    store: &Store,
    monitor: &Monitor,
    now: i64,
    actor: &str,
    event: Option<(&str, &str)>,
    while_active: bool,
) -> rusqlite::Result<bool> {
    let mut db = store.db();
    let transaction = db.transaction()?;
    let saved = transaction.execute(
        "UPDATE monitors SET state = ?3, reason = ?4, next_due = ?5, last_fired = ?6, fires = ?7, carry = ?8, cursor = ?9, updated = ?10
         WHERE agent = ?1 AND id = ?2 AND (?11 = 0 OR state = 'active')",
        params![
            monitor.agent,
            monitor.id,
            monitor.status.name(),
            monitor.reason,
            monitor.next_due,
            monitor.last_fired,
            monitor.fires,
            monitor.carry,
            serde_json::to_string(&monitor.cursor).unwrap_or_default(),
            now,
            while_active
        ],
    )? > 0;
    if saved && let Some((kind, value)) = event {
        record(&transaction, &monitor.agent, monitor.id, now, actor, kind, value)?;
    }
    transaction.commit()?;
    Ok(saved)
}

/// How many times the agent's monitors fired since `since`.
pub(crate) fn fired_since(store: &Store, agent: &str, since: i64) -> rusqlite::Result<u32> {
    store.db().query_row(
        "SELECT COUNT(*) FROM monitor_events WHERE agent = ?1 AND kind = 'fired' AND time >= ?2",
        params![agent, since],
        |row| row.get(0),
    )
}

/// Whether the monitor's last fire is still waiting to be read.
pub(crate) fn waiting(store: &Store, agent: &str, id: u32) -> rusqlite::Result<bool> {
    store
        .db()
        .query_row(
            "SELECT 1 FROM messages WHERE agent = ?1 AND source = 'monitor' AND sender_id = ?2 AND delivered IS NULL AND dropped IS NULL LIMIT 1",
            params![agent, format!("m{id}")],
            |_| Ok(()),
        )
        .optional()
        .map(|found| found.is_some())
}

/// An agent is gone: its monitors go, and their processes stop.
pub(crate) fn forget(factory: &Factory, agent: &str) -> rusqlite::Result<()> {
    let _processes = factory.monitors.processes();
    for monitor in query(&factory.store, &Query { agent: Some(agent), ..Query::default() })? {
        run::stop_process(factory, &monitor);
    }
    let db = factory.store.db();
    db.execute("DELETE FROM monitors WHERE agent = ?1", [agent])?;
    db.execute("DELETE FROM monitor_events WHERE agent = ?1", [agent])?;
    Ok(())
}

/// Whether something the agent's monitors wait for will wake it: an active
/// check, files or watch monitor, or a time monitor due within the hour.
pub(crate) fn will_wake(factory: &Factory, agent: &AgentConfig, now: i64) -> rusqlite::Result<bool> {
    if !super::enabled(agent, ID) {
        return Ok(false);
    }
    let soon = now + 60 * when::MINUTE;
    let monitors = query(&factory.store, &Query { agent: Some(&agent.id), active: true, ..Query::default() })?;
    Ok(monitors.iter().any(|monitor| !monitor.when.is_time() || monitor.next_due.is_some_and(|due| due <= soon)))
}

pub(crate) struct Monitors;

impl Extension for Monitors {
    fn manifest(&self) -> &'static Manifest {
        &MANIFEST
    }

    fn prompt(&self, _factory: &Factory, _inner: &Inner, _agent: &AgentConfig) -> Vec<PromptSection> {
        vec![PromptSection { kind: PromptKind::Static, text: PROMPT.to_owned() }]
    }

    fn tools(&self, factory: &Arc<Factory>, agent: &str) -> Vec<Tool> {
        // Optional parameters are the ones left out of `required` (see `super::tool`).
        let add = json!({
            "type": "object",
            "required": ["monitors"],
            "properties": {"monitors": {"type": "array", "items": {
                "type": "object",
                "required": ["name", "prompt", "when"],
                "properties": {
                    "name": {"type": "string"},
                    "prompt": {"type": "string"},
                    "when": {"type": "object"},
                    "until": {"type": "string"},
                    "max_fires": {"type": "integer"},
                    "idle_only": {"type": "boolean"},
                    "agent": {"type": "string"},
                },
            }}},
            "additionalProperties": false,
        });
        let list = json!({
            "type": "object",
            "properties": {"all": {"type": "boolean"}, "agent": {"type": "string"}},
            "additionalProperties": false,
        });
        let change = json!({
            "type": "object",
            "required": ["changes"],
            "properties": {
                "changes": {"type": "array", "items": {
                    "type": "object",
                    "required": ["id", "action"],
                    "properties": {"id": {"type": "string"}, "action": {"type": "string"}},
                }},
                "agent": {"type": "string"},
            },
            "additionalProperties": false,
        });
        vec![
            tool(factory, agent, "monitors_add", "Add monitors", ADD, add, run_add),
            tool(factory, agent, "monitors_list", "List monitors", LIST, list, run_list),
            tool(factory, agent, "monitors_change", "Change monitors", CHANGE, change, run_change),
        ]
    }

    fn view(&self, factory: &Factory, inner: &Inner, agent: &str) -> Option<v1::ExtensionView> {
        inner.config.agent(agent)?;
        let now = factory.monitors.now();
        let read = || -> rusqlite::Result<(Vec<Monitor>, Vec<String>)> {
            let monitors =
                query(&factory.store, &Query { agent: Some(agent), ended_since: Some(now - ENDED_LISTED_MS), ..Query::default() })?;
            Ok((monitors, builtins(&factory.store, &inner.config, agent)?))
        };
        let (monitors, builtins) = read().unwrap_or_else(|error| {
            tracing::error!(%error, "could not read monitors for the panel");
            Default::default()
        });
        let monitors = monitors.iter().map(|monitor| to_proto(monitor, now)).collect();
        Some(v1::ExtensionView {
            extension_id: ID.to_owned(),
            agent_id: agent.to_owned(),
            title: MANIFEST.name.to_owned(),
            body: Some(v1::extension_view::Body::Monitors(v1::Monitors { monitors, builtins })),
        })
    }
}

fn to_proto(monitor: &Monitor, now: i64) -> v1::Monitor {
    let state = match monitor.status {
        Status::Active => v1::MonitorState::Active,
        Status::Paused => v1::MonitorState::Paused,
        Status::Done => v1::MonitorState::Done,
        Status::Cancelled => v1::MonitorState::Cancelled,
        Status::Error => v1::MonitorState::Error,
    };
    let active = monitor.status == Status::Active;
    v1::Monitor {
        id: monitor.id,
        name: monitor.name.clone(),
        prompt: monitor.prompt.clone(),
        when: monitor.when.describe(now),
        kind: monitor.when.kind().to_owned(),
        state: state.into(),
        reason: monitor.reason.clone(),
        next_due: monitor.next_due.filter(|_| active).unwrap_or_default(),
        last_fired: monitor.last_fired.unwrap_or_default(),
        fires: monitor.fires,
        until: monitor.until.unwrap_or_default(),
        max_fires: monitor.max_fires.unwrap_or_default(),
        idle_only: monitor.idle_only,
        created_by: monitor.created_by.clone(),
        created: monitor.created,
    }
}

/// Shows the open Monitors panel as it is now, if one is open; sent only when
/// it changed.
pub(crate) fn refresh_view(factory: &Factory) {
    let mut inner = factory.lock();
    if inner.view.as_ref().is_none_or(|view| view.extension_id != ID) {
        return;
    }
    let view = factory.current_view(&inner);
    factory.set_view(&mut inner, view);
}

impl Factory {
    /// Pauses, resumes or cancels one of an agent's monitors for the person
    /// (the Monitors panel's buttons).
    pub fn change_monitor(self: &Arc<Self>, agent: &str, id: u32, action: v1::MonitorAction) -> Result<(), RequestError> {
        self.known_agent(agent)?;
        let action = match action {
            v1::MonitorAction::Pause => "pause",
            v1::MonitorAction::Resume => "resume",
            v1::MonitorAction::Cancel => "cancel",
            v1::MonitorAction::Unspecified => return Err(RequestError::Monitor("No action given.".to_owned())),
        };
        change(self, agent, "", &[ChangeOne { id: id.to_string(), action: action.to_owned() }]).map(drop).map_err(RequestError::Monitor)
    }
}

fn tool<R: super::Reply>(
    factory: &Arc<Factory>,
    agent: &str,
    name: &'static str,
    label: &'static str,
    description: &'static str,
    parameters: Value,
    run: fn(&Arc<Factory>, &str, Value) -> Result<R, String>,
) -> Tool {
    super::tool(
        name,
        label,
        description,
        parameters,
        super::agent_handler(factory, agent, move |factory, agent, args| async move {
            let factory = super::live(factory)?;
            run(&factory, &agent, args).map(super::Reply::result).map_err(factory_agent::Error::Other)
        }),
    )
}

fn db_error(error: rusqlite::Error) -> String {
    format!("Couldn't read or write monitors: {error}")
}

/// Whose monitors `asker` means: its own, or (with `agent`) another agent
/// it can message. The person (`asker` "") may name anyone.
fn whose(config: &Config, asker: &str, agent: Option<&str>) -> Result<String, String> {
    let agent = agent.map(str::trim).filter(|agent| !agent.is_empty());
    if asker.is_empty() {
        let id = agent.ok_or("Say whose monitors.")?;
        return config.agent(id).map(|agent| agent.id.clone()).ok_or_else(|| format!("No agent has the id \"{id}\"."));
    }
    let me = super::caller(config, asker)?;
    let Some(wanted) = agent.filter(|wanted| *wanted != asker && !me.name.eq_ignore_ascii_case(wanted)) else {
        return Ok(asker.to_owned());
    };
    let reachable: Vec<&AgentConfig> = config.agents.iter().filter(|to| super::agents::can_message(config, me, to)).collect();
    match super::agents::find(config, me, &reachable, wanted)? {
        Some(found) if super::enabled(found, ID) => Ok(found.id.clone()),
        Some(found) => Err(format!("{} has monitors turned off.", found.id)),
        None => {
            let ids: Vec<&str> = reachable.iter().map(|agent| agent.id.as_str()).collect();
            Err(if ids.is_empty() {
                format!("No agent you can message is called \"{wanted}\"; you can set monitors only for yourself.")
            } else {
                format!("No agent you can message is called \"{wanted}\"; you can message: {}.", ids.join(", "))
            })
        }
    }
}

#[derive(Deserialize)]
struct AddArgs {
    monitors: Vec<AddOne>,
}

#[derive(Deserialize)]
struct AddOne {
    name: String,
    prompt: String,
    when: Value,
    #[serde(default)]
    until: Option<String>,
    #[serde(default)]
    max_fires: Option<i64>,
    #[serde(default)]
    idle_only: Option<bool>,
    #[serde(default)]
    agent: Option<String>,
}

fn run_add(factory: &Arc<Factory>, asker: &str, args: Value) -> Result<ToolResult, String> {
    let args: AddArgs = serde_json::from_value(args).map_err(|error| format!("{error}; monitors is a list of {{name, prompt, when}}."))?;
    add_monitors(factory, asker, args.monitors)
}

/// Strips what would break the event tag the name goes in.
fn clean_name(name: &str) -> String {
    let name: String = name.chars().filter(|c| !matches!(c, '"' | '<' | '>' | '\n' | '\r')).collect();
    name.trim().to_owned()
}

fn add_monitors(factory: &Arc<Factory>, asker: &str, wanted: Vec<AddOne>) -> Result<ToolResult, String> {
    if wanted.is_empty() {
        return Err("monitors is empty; give at least one {name, prompt, when}.".to_owned());
    }
    let now = factory.monitors.now();
    // What the creator has read of pipes, kept with the monitor: each fire
    // spreads it to the monitor's agent, even if the creator's conversation
    // is cleared meanwhile — the prompt was written under that influence.
    let taint = {
        let taint = factory.pipes.taint_of(asker);
        (!taint.is_empty()).then(|| serde_json::to_string(&taint).unwrap_or_default())
    };
    let mut new = Vec::new();
    {
        let inner = factory.lock();
        for (index, one) in wanted.into_iter().enumerate() {
            let which = if index == 0 { String::new() } else { format!("monitor {}: ", index + 1) };
            let fail = |error: String| format!("{which}{error} Nothing was added.");
            let agent = whose(&inner.config, asker, one.agent.as_deref()).map_err(fail)?;
            let Some(config) = inner.config.agent(&agent) else { return Err(fail(format!("No agent has the id \"{agent}\"."))) };
            let name = clean_name(&one.name);
            if name.is_empty() || name.chars().count() > MAX_NAME {
                return Err(fail(format!("name must be 1 to {MAX_NAME} characters.")));
            }
            let prompt = one.prompt.trim().to_owned();
            if prompt.is_empty() || prompt.chars().count() > MAX_PROMPT {
                return Err(fail(format!("prompt must be 1 to {MAX_PROMPT} characters; got {}.", prompt.chars().count())));
            }
            let folder = crate::runner::places(&factory.home, &inner.config, config, &[]).folder;
            let (when, next_due) = when::parse(&one.when, &folder, now).map_err(fail)?;
            let until = one.until.as_deref().map(|until| when::parse_time(until, now)).transpose().map_err(fail)?;
            if until.is_some_and(|until| until <= now) {
                return Err(fail("until is in the past.".to_owned()));
            }
            let max_fires = match one.max_fires {
                None => None,
                Some(fires) if fires >= 1 => Some(u32::try_from(fires).unwrap_or(u32::MAX)),
                Some(_) => return Err(fail("max_fires must be at least 1.".to_owned())),
            };
            let idle_only = one.idle_only.unwrap_or(when.is_time());
            new.push(New { agent, name, prompt, when, next_due, until, max_fires, idle_only, taint: taint.clone() });
        }
    }
    // Room for them, agent by agent.
    let mut agents: Vec<&str> = new.iter().map(|monitor| monitor.agent.as_str()).collect();
    agents.sort_unstable();
    agents.dedup();
    for agent in agents {
        let active = query(&factory.store, &Query { agent: Some(agent), active: true, ..Query::default() }).map_err(db_error)?;
        let adding: Vec<&New> = new.iter().filter(|monitor| monitor.agent == agent).collect();
        let whose = if agent == asker { "You have".to_owned() } else { format!("{agent} has") };
        if active.len() + adding.len() > MAX_ACTIVE {
            return Err(format!(
                "{whose} {} active monitors; at most {MAX_ACTIVE}. Cancel some with monitors_change first. Nothing was added.",
                active.len()
            ));
        }
        let watches = |kind: bool| kind as usize;
        let running = active.iter().map(|monitor| watches(matches!(monitor.when, When::Watch { .. }))).sum::<usize>();
        let more = adding.iter().map(|monitor| watches(matches!(monitor.when, When::Watch { .. }))).sum::<usize>();
        if running + more > MAX_WATCHES {
            return Err(format!("{whose} {running} watch monitors; at most {MAX_WATCHES}, since each runs a process. Nothing was added."));
        }
    }
    let ids = add(&factory.store, asker, &new, now).map_err(db_error)?;
    let mut lines = Vec::new();
    let mut added = Vec::new();
    for (monitor, id) in new.iter().zip(&ids) {
        run::added(factory, &monitor.agent, *id);
        let whose = if monitor.agent == asker { String::new() } else { format!(" for {}", monitor.agent) };
        let first = match (&monitor.when, monitor.next_due) {
            (When::Check { .. }, _) => "; first check now".to_owned(),
            (_, Some(due)) => format!("; first fire {}", when::local(due, now)),
            (When::Files { .. }, _) => "; watching from now".to_owned(),
            _ => "; starting it now".to_owned(),
        };
        let schedule = monitor.when.describe(now);
        lines.push(format!("Added m{id}{whose} \"{}\": {schedule}{first}.", monitor.name));
        let mut shown = json!({"id": format!("m{id}"), "agent": monitor.agent, "name": monitor.name, "schedule": schedule});
        if let Some(due) = monitor.next_due {
            shown["next"] = json!(due);
        }
        added.push(shown);
    }
    refresh_view(factory);
    Ok(ToolResult::text(lines.join("\n"), json!({"monitors": added})))
}

#[derive(Deserialize)]
struct ListArgs {
    #[serde(default)]
    all: Option<bool>,
    #[serde(default)]
    agent: Option<String>,
}

fn run_list(factory: &Arc<Factory>, asker: &str, args: Value) -> Result<ToolResult, String> {
    let args: ListArgs = serde_json::from_value(args).map_err(|error| error.to_string())?;
    let agent = whose(&factory.lock().config, asker, args.agent.as_deref())?;
    let (text, count) = list(factory, &agent, args.all.unwrap_or(false))?;
    Ok(ToolResult::text(text, json!({"agent": agent, "count": count})))
}

/// One line per monitor, then what else can wake the agent; and how many
/// monitors it listed.
fn list(factory: &Factory, agent: &str, all: bool) -> Result<(String, usize), String> {
    let now = factory.monitors.now();
    let ended_since = all.then_some(now - ENDED_LISTED_MS);
    let monitors = query(&factory.store, &Query { agent: Some(agent), ended_since, ..Query::default() }).map_err(db_error)?;
    let person = factory.lock().config.person().to_owned();
    let mut lines: Vec<String> = monitors.iter().map(|monitor| line(monitor, agent, &person, now)).collect();
    lines.extend(builtins(&factory.store, &factory.lock().config, agent).map_err(db_error)?);
    if lines.is_empty() {
        return Ok(("No monitors.".to_owned(), 0));
    }
    Ok((lines.join("\n"), monitors.len()))
}

/// The agent's monitors that haven't ended, one line each, as monitors_list
/// shows them: for an agent that leads it (agents_inspect).
pub(crate) fn lines(factory: &Factory, agent: &str) -> Result<Vec<String>, String> {
    let now = factory.monitors.now();
    let monitors = query(&factory.store, &Query { agent: Some(agent), ..Query::default() }).map_err(db_error)?;
    let person = factory.lock().config.person().to_owned();
    Ok(monitors.iter().map(|monitor| line(monitor, agent, &person, now)).collect())
}

/// "m3 deploy check — check `gh run view…` every 5m, fires on match \"success\" · next 14:10 · fired 0".
fn line(monitor: &Monitor, agent: &str, person: &str, now: i64) -> String {
    let mut line = format!("m{} {} — {}", monitor.id, monitor.name, monitor.when.describe(now));
    match monitor.status {
        Status::Active => {
            if let Some(due) = monitor.next_due {
                let _ = write!(line, " · next {}", when::local(due, now));
            }
        }
        status => {
            let _ = write!(line, " · {}", status.name());
            if !monitor.reason.is_empty() {
                let _ = write!(line, ": {}", monitor.reason);
            }
        }
    }
    let _ = write!(line, " · fired {}", monitor.fires);
    if let Some(until) = monitor.until {
        let _ = write!(line, " · until {}", when::local(until, now));
    }
    if let Some(max) = monitor.max_fires {
        let _ = write!(line, " · at most {max}");
    }
    if monitor.created_by != agent {
        let by = if monitor.created_by.is_empty() { person } else { monitor.created_by.as_str() };
        let _ = write!(line, " · set by {by}");
    }
    line
}

/// What else wakes the agent: its running background tasks, and the
/// Checklist's reminder.
fn builtins(store: &Store, config: &Config, agent: &str) -> rusqlite::Result<Vec<String>> {
    let mut lines: Vec<String> = store
        .tasks(Some(agent), i64::MAX)?
        .into_iter()
        .filter(|task| task.ended.is_none())
        .map(|task| format!("{} (background task) — wakes you when it exits", task.task))
        .collect();
    if let Some(config) = config.agent(agent)
        && super::enabled(config, super::checklist::ID)
        && super::switch(config, super::checklist::ID, "nudge")
    {
        lines.push("checklist reminder — on: reminds you of outstanding items when nothing else will wake you".to_owned());
    }
    Ok(lines)
}

#[derive(Deserialize)]
struct ChangeArgs {
    changes: Vec<ChangeOne>,
    #[serde(default)]
    agent: Option<String>,
}

#[derive(Deserialize)]
pub(crate) struct ChangeOne {
    pub id: String,
    pub action: String,
}

fn run_change(factory: &Arc<Factory>, asker: &str, args: Value) -> Result<ToolResult, String> {
    let args: ChangeArgs = serde_json::from_value(args).map_err(|error| format!("{error}; changes is a list of {{id, action}}."))?;
    let agent = whose(&factory.lock().config, asker, args.agent.as_deref())?;
    change(factory, &agent, asker, &args.changes)
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Action {
    Pause,
    Resume,
    Cancel,
    Fire,
}

/// Pauses, resumes, cancels or fires the agent's monitors, all or none;
/// `actor` is who asked (an agent id, or '' for the person). Says what
/// changed, with each monitor's {id, name, action} as details.
pub(crate) fn change(factory: &Arc<Factory>, agent: &str, actor: &str, changes: &[ChangeOne]) -> Result<ToolResult, String> {
    if changes.is_empty() {
        return Err("changes is empty; give at least one {id, action}.".to_owned());
    }
    let by = if actor.is_empty() { format!("by {}", factory.lock().config.person()) } else { format!("by {actor}") };
    let mut planned = Vec::new();
    for change in changes {
        let wanted = change.id.trim();
        let id: u32 =
            wanted.trim_start_matches(['m', 'M']).parse().map_err(|_| format!("\"{wanted}\" isn't a monitor id; ids are m1, m2…"))?;
        let action = match change.action.trim() {
            "pause" => Action::Pause,
            "resume" => Action::Resume,
            "cancel" => Action::Cancel,
            "fire" => Action::Fire,
            other => return Err(format!("m{id}: action must be pause, resume, cancel or fire; got \"{other}\". Nothing changed.")),
        };
        let found = query(&factory.store, &Query { agent: Some(agent), id: Some(id), ended_since: Some(0), ..Query::default() })
            .map_err(db_error)?;
        let Some(monitor) = found.into_iter().next() else {
            return Err(format!("There's no monitor m{id}; monitors_list shows them. Nothing changed."));
        };
        let fine = match action {
            Action::Pause | Action::Fire => monitor.status == Status::Active,
            Action::Resume => matches!(monitor.status, Status::Paused | Status::Error),
            Action::Cancel => !monitor.status.ended(),
        };
        if !fine {
            return Err(format!("m{id} is {}, so it can't be {}. Nothing changed.", monitor.status.name(), past(action)));
        }
        planned.push((monitor.id, action));
    }
    let now = factory.monitors.now();
    let mut said = Vec::new();
    let mut changed = Vec::new();
    for (id, action) in planned {
        // Read again, with processes held still: the scheduler may have
        // started or stopped one since.
        let processes = factory.monitors.processes();
        let found = query(&factory.store, &Query { agent: Some(agent), id: Some(id), ended_since: Some(0), ..Query::default() })
            .map_err(db_error)?;
        let Some(mut monitor) = found.into_iter().next() else { continue };
        match action {
            Action::Fire => {
                drop(processes);
                run::fire_by_hand(factory, &mut monitor, actor, now).map_err(db_error)?;
                said.push(format!("m{} fired", monitor.id));
                changed.push(json!({"id": format!("m{}", monitor.id), "name": monitor.name, "action": "fired"}));
                continue;
            }
            Action::Pause => {
                run::stop_process(factory, &monitor);
                monitor.status = Status::Paused;
                monitor.reason = by.clone();
                monitor.cursor.pid = None;
            }
            Action::Cancel => {
                run::stop_process(factory, &monitor);
                monitor.status = Status::Cancelled;
                monitor.reason = by.clone();
                monitor.cursor.pid = None;
            }
            Action::Resume => {
                monitor.status = Status::Active;
                monitor.reason.clear();
                monitor.cursor.failures = 0;
                monitor.cursor.quick_runs = 0;
                monitor.cursor.restart_at = None;
                // A time monitor that came due meanwhile fires once, not once per time it missed.
                if monitor.when.is_time() && monitor.next_due.is_none_or(|due| due < now) {
                    monitor.next_due = Some(now);
                }
                if matches!(monitor.when, When::Check { .. }) {
                    monitor.next_due = Some(now);
                }
            }
        }
        let kind = past(action);
        save(&factory.store, &monitor, now, actor, Some((kind, &monitor.reason)), false).map_err(db_error)?;
        drop(processes);
        run::forget_live(factory, &monitor);
        said.push(format!("m{} {kind}", monitor.id));
        changed.push(json!({"id": format!("m{}", monitor.id), "name": monitor.name, "action": kind}));
    }
    refresh_view(factory);
    Ok(ToolResult::text(format!("{}.", said.join("; ")), json!({"monitors": changed})))
}

fn past(action: Action) -> &'static str {
    match action {
        Action::Pause => "paused",
        Action::Resume => "resumed",
        Action::Cancel => "cancelled",
        Action::Fire => "fired",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_cant_break_the_event_tag() {
        assert_eq!(clean_name(" deploy \"check\" <now>\n"), "deploy check now");
    }
}
