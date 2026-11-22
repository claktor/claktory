//! The Checklist extension: everything the person asks an agent for, kept as
//! items the agent works through over days, so nothing they said is lost.
//!
//! Items live in factory.db (`checklist_items`), and every change is also an
//! event in `checklist_events`, which is only ever added to, so how work went
//! (such as items done per hour) can be read back later. Everything reads
//! items through `query`, so a permission check can go there later.

use std::fmt::Write as _;
use std::sync::Arc;

use factory_agent::{Tool, ToolResult};
use factory_protocol::v1;
use rusqlite::{OptionalExtension, params, params_from_iter};
use serde::Deserialize;
use serde_json::json;

use super::{Extension, Manifest, OptionKind, OptionSpec, PromptKind, PromptSection, factory_tool};
use crate::config::AgentConfig;
use crate::factory::{Factory, Inner};
use crate::store::Store;
use crate::transcript::now_ms;

pub(crate) const ID: &str = "checklist";
const DEFAULT_LIMIT: usize = 50;
const MAX_LIMIT: usize = 500;

/// The migration that added what an item is blocked on: `blocked_on`
/// ("person" when it waits on the person, else a short free text; ''
/// while not blocked) and `blocked_since` (ms; null while not blocked),
/// and when each agent was last reminded of person-blocked items with
/// no question waiting (agent '' records the one-time sweep).
pub(crate) const BLOCKED_ON: &str = "
ALTER TABLE checklist_items ADD COLUMN blocked_on TEXT NOT NULL DEFAULT '';
ALTER TABLE checklist_items ADD COLUMN blocked_since INTEGER;
CREATE TABLE checklist_nudges (agent TEXT PRIMARY KEY, last INTEGER NOT NULL);
";

/// The migration that added goals (#270): outcomes the person wants, per
/// agent (G1, G2…, apart from items' ids), each with its own history; and
/// the goal an item serves (0 for none).
pub(crate) const GOALS: &str = "
CREATE TABLE checklist_goals (
    agent TEXT NOT NULL,
    id INTEGER NOT NULL,
    text TEXT NOT NULL,
    -- open, done or dropped
    status TEXT NOT NULL,
    created INTEGER NOT NULL,
    updated INTEGER NOT NULL,
    PRIMARY KEY (agent, id)
);
CREATE TABLE checklist_goal_events (
    seq INTEGER PRIMARY KEY,
    agent TEXT NOT NULL,
    goal INTEGER NOT NULL,
    time INTEGER NOT NULL,
    -- an agent id; '' for the person
    actor TEXT NOT NULL,
    -- created, status, edited or removed
    kind TEXT NOT NULL,
    value TEXT NOT NULL
);
CREATE INDEX checklist_goal_events_by_goal ON checklist_goal_events (agent, goal, seq);
ALTER TABLE checklist_items ADD COLUMN goal INTEGER NOT NULL DEFAULT 0;
";

/// The longest goal.
const GOAL_LENGTH: usize = 500;

/// How often `nudge_every` checks in: each choice's value, label and
/// minutes.
pub(crate) const EVERY: [(&str, &str, i64); 6] = [
    ("off", "Off", 0),
    ("15m", "Every 15 minutes", 15),
    ("30m", "Every 30 minutes", 30),
    ("1h", "Every hour", 60),
    ("2h", "Every 2 hours", 120),
    ("1d", "Once a day", 24 * 60),
];

static MANIFEST: Manifest = Manifest {
    id: ID,
    name: "Checklist",
    icon: "list-checks",
    description: "Keeps a checklist of everything you ask for, so nothing gets forgotten, even days later.",
    default_enabled: false,
    options: &[
        OptionSpec {
            id: "nudge",
            label: "Remind it of unfinished items",
            description: "If it stops with items still open and nothing will wake it, Claktory reminds it once.",
            kind: OptionKind::Switch,
            default_on: true,
            includes_project: false,
        },
        OptionSpec {
            id: "nudge_every",
            label: "Check in on its goals and items",
            description: "While it has open goals or unfinished items, Claktory reminds it this often, once it's idle, to see whether anything can move forward.",
            kind: OptionKind::Choice {
                choices: &[
                    (EVERY[0].0, EVERY[0].1),
                    (EVERY[1].0, EVERY[1].1),
                    (EVERY[2].0, EVERY[2].1),
                    (EVERY[3].0, EVERY[3].1),
                    (EVERY[4].0, EVERY[4].1),
                    (EVERY[5].0, EVERY[5].1),
                ],
                default: "off",
            },
            default_on: false,
            includes_project: false,
        },
    ],
};

const PROMPT: &str = "## Checklist

You keep a checklist of everything the person asks you for, so nothing they said is forgotten across days and compactions. The person sees it beside your conversation.
- When they ask for things, record each as an item right away. checklist_add takes many at once.
- Keep items current with checklist_update: in_progress or an owner when work starts, done or dropped with a short note when it ends.
- in_progress means someone is working on it right now; work waiting its turn stays open, with its owner. Hand each worker one or two items at a time, and the next when one is done.
- blocked means it waits on the person or something outside; set blocked_on with it: \"person\" when it waits on the person, else a few words naming the blocker, and say more in a note. A decision that gates work is asked the day it blocks: when an item blocks on the person, file ask_person then and there, one card per real decision; only pure preferences batch into one question. While a person-blocked item has no question waiting, Claktory reminds the item's owner daily.
- Goals are the person's: outcomes they want (\"a link to the Android app I can share\"). Record one with checklist_goals only when the person states it, in their words, never your own plans; the person also adds and changes goals in the app, and you're told when they do. Link the items that serve a goal with `goal` on checklist_add or checklist_update. Mark a goal done only when the person's outcome is reached, or dropped when they say so.
- checklist_list shows open goals, then open items; filter by status, day, owner or text. checklist_item shows one item's history. Both take `agent` to read another agent's checklist.
- Check the list before telling the person work is finished.
";

const ADD: &str = "Add items to your checklist, many at once. goal: the id of the goal they serve (2 for G2). Returns their ids.";
const UPDATE: &str = "Change items on your checklist: status (open, in_progress, blocked, done, dropped), blocked_on with status blocked (\"person\" when it waits on the person, else a few words naming the blocker), goal (the id of the goal it serves, 2 for G2; 0 for none), owner (an agent id or name; empty to unassign), text, and/or a short note.";
const LIST: &str = "List the checklist: its open goals (all of them with status all), then its items, one line each, oldest first: open, in progress and blocked unless `status` says otherwise (open, in_progress, blocked, done, dropped, all). Filter items by `day` added (YYYY-MM-DD), `owner` or text `q`. `agent` reads another agent's checklist.";
const GOALS_TOOL: &str = "The person's goals on your checklist: outcomes they want, recorded in their words, only from what they said. add: new goals' texts. updates: changes by id (1 for G1): text, status (open, done, dropped), or remove. Link items to a goal with `goal` on checklist_add or checklist_update.";
const ITEM: &str = "One checklist item with everything that happened to it. `agent` reads another agent's checklist.";

pub(crate) struct Checklist;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Status {
    Open,
    InProgress,
    Done,
    Dropped,
    /// Waiting on the person or something outside; still outstanding.
    Blocked,
}

/// What's still to do: everything not done or dropped.
const OUTSTANDING: [Status; 3] = [Status::Open, Status::InProgress, Status::Blocked];

impl Status {
    const ALL: [Status; 5] = [Status::Open, Status::InProgress, Status::Done, Status::Dropped, Status::Blocked];

    fn name(self) -> &'static str {
        match self {
            Status::Open => "open",
            Status::InProgress => "in_progress",
            Status::Done => "done",
            Status::Dropped => "dropped",
            Status::Blocked => "blocked",
        }
    }

    fn parse(name: &str) -> Option<Status> {
        Status::ALL.into_iter().find(|status| status.name() == name.trim().to_ascii_lowercase().replace([' ', '-'], "_"))
    }

    fn mark(self) -> &'static str {
        match self {
            Status::Open => "[ ]",
            Status::InProgress => "[~]",
            Status::Done => "[x]",
            Status::Dropped => "[-]",
            Status::Blocked => "[!]",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Status::Open => "Open",
            Status::InProgress => "In progress",
            Status::Done => "Done",
            Status::Dropped => "Dropped",
            Status::Blocked => "Blocked",
        }
    }

    fn proto(self) -> v1::ChecklistStatus {
        match self {
            Status::Open => v1::ChecklistStatus::Open,
            Status::InProgress => v1::ChecklistStatus::InProgress,
            Status::Done => v1::ChecklistStatus::Done,
            Status::Dropped => v1::ChecklistStatus::Dropped,
            Status::Blocked => v1::ChecklistStatus::Blocked,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Item {
    pub id: u32,
    pub text: String,
    pub status: Status,
    pub owner: String,
    /// What it waits on while blocked: "person" when it waits on the
    /// person, else a short free text; empty while not blocked (and on
    /// blocked items from before it was kept).
    pub blocked_on: String,
    /// When it became blocked, in ms; None while not blocked.
    pub blocked_since: Option<i64>,
    /// The goal it serves; 0 for none.
    pub goal: u32,
    pub created: i64,
    pub updated: i64,
    /// The local day it was added, "11-18".
    day: String,
    /// What happened to it, oldest first; only when asked for.
    pub log: Vec<Change>,
    /// While it's blocked, the latest note since it became so; only with the log.
    pub blocked_note: String,
}

/// One line of an item's history.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Change {
    pub time: i64,
    /// Local time, "11-18 14:02".
    when: String,
    /// An agent id; empty for the person.
    pub actor: String,
    pub text: String,
}

/// Which items to read. Every read of checklist data goes through `query`.
#[derive(Default)]
pub(crate) struct Query<'a> {
    pub agent: &'a str,
    /// None for every status.
    pub statuses: Option<Vec<Status>>,
    /// A local day, "2026-11-18".
    pub day: Option<&'a str>,
    pub owner: Option<&'a str>,
    /// Text the item contains.
    pub text: Option<&'a str>,
    pub limit: Option<usize>,
    pub with_log: bool,
}

/// Whether the agent's checklist has item `id`.
pub(crate) fn has_item(store: &Store, agent: &str, id: u32) -> rusqlite::Result<bool> {
    let db = store.db();
    db.query_row("SELECT EXISTS (SELECT 1 FROM checklist_items WHERE agent = ?1 AND id = ?2)", rusqlite::params![agent, id], |row| {
        row.get(0)
    })
}

/// The items `query` asks for, oldest first, and how many matched in all.
pub(crate) fn query(store: &Store, query: &Query) -> rusqlite::Result<(Vec<Item>, usize)> {
    let db = store.db();
    let mut filter = String::from("agent = ?1");
    let mut values = vec![query.agent.to_owned()];
    if let Some(statuses) = &query.statuses {
        let mut marks = Vec::new();
        for status in statuses {
            values.push(status.name().to_owned());
            marks.push(format!("?{}", values.len()));
        }
        let _ = write!(filter, " AND status IN ({})", marks.join(", "));
    }
    if let Some(day) = query.day {
        values.push(day.to_owned());
        let _ = write!(filter, " AND strftime('%Y-%m-%d', created / 1000, 'unixepoch', 'localtime') = ?{}", values.len());
    }
    if let Some(owner) = query.owner {
        values.push(owner.to_owned());
        let _ = write!(filter, " AND owner = ?{}", values.len());
    }
    if let Some(text) = query.text {
        values.push(format!("%{}%", text.replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_")));
        let _ = write!(filter, " AND text LIKE ?{} ESCAPE '\\'", values.len());
    }
    let total: i64 =
        db.query_row(&format!("SELECT COUNT(*) FROM checklist_items WHERE {filter}"), params_from_iter(&values), |row| row.get(0))?;
    let total = usize::try_from(total).unwrap_or(0);
    let limit = query.limit.map_or(-1, |limit| limit as i64);
    let mut statement = db.prepare(&format!(
        "SELECT id, text, status, owner, created, updated, strftime('%m-%d', created / 1000, 'unixepoch', 'localtime'), blocked_on, blocked_since, goal
         FROM checklist_items WHERE {filter} ORDER BY id LIMIT {limit}"
    ))?;
    let mut rows = statement.query(params_from_iter(&values))?;
    let mut items = Vec::new();
    while let Some(row) = rows.next()? {
        let status: String = row.get(2)?;
        items.push(Item {
            id: row.get(0)?,
            text: row.get(1)?,
            status: Status::parse(&status).unwrap_or(Status::Open),
            owner: row.get(3)?,
            created: row.get(4)?,
            updated: row.get(5)?,
            day: row.get(6)?,
            blocked_on: row.get(7)?,
            blocked_since: row.get(8)?,
            goal: row.get(9)?,
            log: Vec::new(),
            blocked_note: String::new(),
        });
    }
    if query.with_log && !items.is_empty() {
        let mut statement = db.prepare(
            "SELECT item, time, strftime('%m-%d %H:%M', time / 1000, 'unixepoch', 'localtime'), actor, kind, value
             FROM checklist_events WHERE agent = ?1 ORDER BY seq",
        )?;
        let mut rows = statement.query([query.agent])?;
        let mut events: std::collections::HashMap<u32, Vec<Row>> = std::collections::HashMap::new();
        while let Some(row) = rows.next()? {
            events.entry(row.get(0)?).or_default().push((row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?, row.get(5)?));
        }
        for item in &mut items {
            let events = events.remove(&item.id).unwrap_or_default();
            if item.status == Status::Blocked {
                item.blocked_note = blocked_note(&events);
            }
            item.log = changes(events);
        }
    }
    Ok((items, total))
}

/// The latest note made since the item last became blocked.
fn blocked_note(events: &[Row]) -> String {
    let since = events.iter().rposition(|(_, _, _, kind, value)| kind == "status" && value == Status::Blocked.name()).unwrap_or(0);
    let note = events[since..].iter().rev().find(|(_, _, _, kind, _)| kind == "note");
    note.map(|(_, _, _, _, value)| value.clone()).unwrap_or_default()
}

/// An event as read: time, local time, actor, kind and value.
type Row = (i64, String, String, String, String);

/// An item's events as lines to show: a note made with a status or owner
/// change reads as one line, "Done: shipped".
fn changes(events: Vec<Row>) -> Vec<Change> {
    let mut log: Vec<Change> = Vec::new();
    let mut joinable = false;
    for (time, when, actor, kind, value) in events {
        let text = match kind.as_str() {
            "created" => "Added".to_owned(),
            "status" => Status::parse(&value).map_or(value, |status| status.label().to_owned()),
            "owner" if value.is_empty() => "Unassigned".to_owned(),
            "owner" => format!("Owner: {value}"),
            "edited" => format!("Edited: {value}"),
            "blocked_on" => format!("Blocks on: {value}"),
            "goal" if value.is_empty() => "No goal".to_owned(),
            "goal" => format!("Serves {value}"),
            _ => value,
        };
        if kind == "note"
            && joinable
            && let Some(last) = log.last_mut()
            && last.time == time
            && last.actor == actor
        {
            let _ = write!(last.text, ": {text}");
            joinable = false;
            continue;
        }
        joinable = matches!(kind.as_str(), "status" | "owner");
        log.push(Change { time, when, actor, text });
    }
    log
}

/// Adds items, serving `goal` unless it's 0 (which must exist); returns their ids.
pub(crate) fn add(store: &Store, agent: &str, actor: &str, texts: &[String], owner: &str, goal: u32) -> rusqlite::Result<Vec<u32>> {
    let mut db = store.db();
    let transaction = db.transaction()?;
    let last: u32 =
        transaction.query_row("SELECT COALESCE(MAX(id), 0) FROM checklist_items WHERE agent = ?1", [agent], |row| row.get(0))?;
    let now = now_ms();
    let mut ids = Vec::new();
    for (offset, text) in texts.iter().enumerate() {
        let id = last + 1 + offset as u32;
        transaction.execute(
            "INSERT INTO checklist_items (agent, id, text, status, owner, created, updated, goal) VALUES (?1, ?2, ?3, 'open', ?4, ?5, ?5, ?6)",
            params![agent, id, text, owner, now, goal],
        )?;
        record(&transaction, agent, id, now, actor, "created", text)?;
        if !owner.is_empty() {
            record(&transaction, agent, id, now, actor, "owner", owner)?;
        }
        if goal != 0 {
            record(&transaction, agent, id, now, actor, "goal", &format!("G{goal}"))?;
        }
        ids.push(id);
    }
    transaction.commit()?;
    Ok(ids)
}

fn record(db: &rusqlite::Connection, agent: &str, item: u32, time: i64, actor: &str, kind: &str, value: &str) -> rusqlite::Result<()> {
    db.execute(
        "INSERT INTO checklist_events (agent, item, time, actor, kind, value) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![agent, item, time, actor, kind, value],
    )?;
    Ok(())
}

/// One change to an item; `owner` is already an agent id, or empty to unassign.
#[derive(Default)]
pub(crate) struct Update {
    pub id: u32,
    pub status: Option<Status>,
    pub owner: Option<String>,
    pub text: Option<String>,
    pub note: Option<String>,
    /// What it waits on: "person", or a short free text. Only on an item
    /// that is (or becomes, in this update) blocked.
    pub blocked_on: Option<String>,
    /// The goal it serves; 0 for none.
    pub goal: Option<u32>,
}

/// Applies changes, each to an existing item; says what changed, item by item.
pub(crate) fn update(store: &Store, agent: &str, actor: &str, updates: &[Update]) -> rusqlite::Result<Vec<String>> {
    let mut db = store.db();
    let transaction = db.transaction()?;
    let now = now_ms();
    let mut said = Vec::new();
    for update in updates {
        let was: Option<String> = transaction
            .query_row("SELECT status FROM checklist_items WHERE agent = ?1 AND id = ?2", params![agent, update.id], |row| row.get(0))
            .optional()?;
        let Some(was) = was else {
            said.push(format!("#{}: no such item", update.id));
            continue;
        };
        let blocked = update.status.map_or(Status::parse(&was) == Some(Status::Blocked), |status| status == Status::Blocked);
        if update.blocked_on.is_some() && !blocked {
            said.push(format!("#{}: blocked_on goes with status blocked", update.id));
            continue;
        }
        if let Some(goal) = update.goal
            && goal != 0
            && !has_goal(&transaction, agent, goal)?
        {
            said.push(format!("#{}: no goal G{goal}", update.id));
            continue;
        }
        let mut parts = Vec::new();
        if let Some(status) = update.status {
            if status == Status::Blocked {
                // Becoming blocked starts the clock; already blocked keeps it.
                transaction.execute(
                    "UPDATE checklist_items SET status = ?3, blocked_since = COALESCE(blocked_since, ?4) WHERE agent = ?1 AND id = ?2",
                    params![agent, update.id, status.name(), now],
                )?;
            } else {
                transaction.execute(
                    "UPDATE checklist_items SET status = ?3, blocked_on = '', blocked_since = NULL WHERE agent = ?1 AND id = ?2",
                    params![agent, update.id, status.name()],
                )?;
            }
            record(&transaction, agent, update.id, now, actor, "status", status.name())?;
            parts.push(status.name().to_owned());
        }
        if let Some(on) = &update.blocked_on {
            transaction
                .execute("UPDATE checklist_items SET blocked_on = ?3 WHERE agent = ?1 AND id = ?2", params![agent, update.id, on])?;
            record(&transaction, agent, update.id, now, actor, "blocked_on", on)?;
            parts.push(format!("blocks on {on}"));
        }
        if let Some(goal) = update.goal {
            transaction.execute("UPDATE checklist_items SET goal = ?3 WHERE agent = ?1 AND id = ?2", params![agent, update.id, goal])?;
            let name = if goal == 0 { String::new() } else { format!("G{goal}") };
            record(&transaction, agent, update.id, now, actor, "goal", &name)?;
            parts.push(if goal == 0 { "no goal".to_owned() } else { format!("serves G{goal}") });
        }
        if let Some(owner) = &update.owner {
            transaction.execute("UPDATE checklist_items SET owner = ?3 WHERE agent = ?1 AND id = ?2", params![agent, update.id, owner])?;
            record(&transaction, agent, update.id, now, actor, "owner", owner)?;
            parts.push(if owner.is_empty() { "unassigned".to_owned() } else { format!("owner {owner}") });
        }
        if let Some(text) = &update.text {
            transaction.execute("UPDATE checklist_items SET text = ?3 WHERE agent = ?1 AND id = ?2", params![agent, update.id, text])?;
            record(&transaction, agent, update.id, now, actor, "edited", text)?;
            parts.push("edited".to_owned());
        }
        if let Some(note) = &update.note {
            record(&transaction, agent, update.id, now, actor, "note", note)?;
            parts.push("noted".to_owned());
        }
        if parts.is_empty() {
            said.push(format!("#{}: nothing to change", update.id));
            continue;
        }
        transaction.execute("UPDATE checklist_items SET updated = ?3 WHERE agent = ?1 AND id = ?2", params![agent, update.id, now])?;
        said.push(format!("#{} {}", update.id, parts.join(", ")));
    }
    transaction.commit()?;
    Ok(said)
}

/// An agent is deleted, and its checklist with it.
pub(crate) fn forget(store: &Store, agent: &str) -> rusqlite::Result<()> {
    let db = store.db();
    db.execute("DELETE FROM checklist_items WHERE agent = ?1", [agent])?;
    db.execute("DELETE FROM checklist_events WHERE agent = ?1", [agent])?;
    db.execute("DELETE FROM checklist_goals WHERE agent = ?1", [agent])?;
    db.execute("DELETE FROM checklist_goal_events WHERE agent = ?1", [agent])?;
    Ok(())
}

/// A goal: an outcome the person wants.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Goal {
    pub id: u32,
    pub text: String,
    /// Open, done or dropped.
    pub status: Status,
    pub created: i64,
    pub updated: i64,
    /// What happened to it, oldest first; only when asked for.
    pub log: Vec<Change>,
}

fn has_goal(db: &rusqlite::Connection, agent: &str, id: u32) -> rusqlite::Result<bool> {
    db.query_row("SELECT EXISTS (SELECT 1 FROM checklist_goals WHERE agent = ?1 AND id = ?2)", params![agent, id], |row| row.get(0))
}

/// The agent's goals, oldest first, every status.
pub(crate) fn goals(store: &Store, agent: &str, with_log: bool) -> rusqlite::Result<Vec<Goal>> {
    let db = store.db();
    let mut statement = db.prepare("SELECT id, text, status, created, updated FROM checklist_goals WHERE agent = ?1 ORDER BY id")?;
    let rows = statement.query_map([agent], |row| {
        let status: String = row.get(2)?;
        Ok(Goal {
            id: row.get(0)?,
            text: row.get(1)?,
            status: Status::parse(&status).unwrap_or(Status::Open),
            created: row.get(3)?,
            updated: row.get(4)?,
            log: Vec::new(),
        })
    })?;
    let mut goals: Vec<Goal> = rows.collect::<Result<_, _>>()?;
    if with_log && !goals.is_empty() {
        let mut statement = db.prepare(
            "SELECT goal, time, strftime('%m-%d %H:%M', time / 1000, 'unixepoch', 'localtime'), actor, kind, value
             FROM checklist_goal_events WHERE agent = ?1 ORDER BY seq",
        )?;
        let mut rows = statement.query([agent])?;
        let mut events: std::collections::HashMap<u32, Vec<Row>> = std::collections::HashMap::new();
        while let Some(row) = rows.next()? {
            events.entry(row.get(0)?).or_default().push((row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?, row.get(5)?));
        }
        for goal in &mut goals {
            goal.log = changes(events.remove(&goal.id).unwrap_or_default());
        }
    }
    Ok(goals)
}

/// How far each goal's items are: done, and all but the dropped.
pub(crate) fn progress(store: &Store, agent: &str) -> rusqlite::Result<std::collections::HashMap<u32, (usize, usize)>> {
    let db = store.db();
    let mut statement = db.prepare("SELECT goal, status FROM checklist_items WHERE agent = ?1 AND goal != 0")?;
    let mut rows = statement.query([agent])?;
    let mut progress: std::collections::HashMap<u32, (usize, usize)> = std::collections::HashMap::new();
    while let Some(row) = rows.next()? {
        let status: String = row.get(1)?;
        let entry = progress.entry(row.get(0)?).or_default();
        match Status::parse(&status) {
            Some(Status::Dropped) => {}
            Some(Status::Done) => {
                entry.0 += 1;
                entry.1 += 1;
            }
            _ => entry.1 += 1,
        }
    }
    Ok(progress)
}

fn record_goal(db: &rusqlite::Connection, agent: &str, goal: u32, time: i64, actor: &str, kind: &str, value: &str) -> rusqlite::Result<()> {
    db.execute(
        "INSERT INTO checklist_goal_events (agent, goal, time, actor, kind, value) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![agent, goal, time, actor, kind, value],
    )?;
    Ok(())
}

/// Adds goals; returns their ids. A removed goal's id isn't given again,
/// so its history stays its own.
pub(crate) fn add_goals(store: &Store, agent: &str, actor: &str, texts: &[String]) -> rusqlite::Result<Vec<u32>> {
    let mut db = store.db();
    let transaction = db.transaction()?;
    let last: u32 = transaction.query_row(
        "SELECT MAX(COALESCE((SELECT MAX(id) FROM checklist_goals WHERE agent = ?1), 0),
                    COALESCE((SELECT MAX(goal) FROM checklist_goal_events WHERE agent = ?1), 0))",
        [agent],
        |row| row.get(0),
    )?;
    let now = now_ms();
    let mut ids = Vec::new();
    for (offset, text) in texts.iter().enumerate() {
        let id = last + 1 + offset as u32;
        transaction.execute(
            "INSERT INTO checklist_goals (agent, id, text, status, created, updated) VALUES (?1, ?2, ?3, 'open', ?4, ?4)",
            params![agent, id, text, now],
        )?;
        record_goal(&transaction, agent, id, now, actor, "created", text)?;
        ids.push(id);
    }
    transaction.commit()?;
    Ok(ids)
}

/// One change to a goal.
#[derive(Default)]
pub(crate) struct GoalUpdate {
    pub id: u32,
    pub text: Option<String>,
    /// Open, done or dropped.
    pub status: Option<Status>,
    /// Removes it; the items that served it serve none.
    pub remove: bool,
}

/// Applies changes, each to an existing goal; says what changed, goal by goal.
pub(crate) fn update_goals(store: &Store, agent: &str, actor: &str, updates: &[GoalUpdate]) -> rusqlite::Result<Vec<String>> {
    let mut db = store.db();
    let transaction = db.transaction()?;
    let now = now_ms();
    let mut said = Vec::new();
    for update in updates {
        let id = update.id;
        if !has_goal(&transaction, agent, id)? {
            said.push(format!("G{id}: no such goal"));
            continue;
        }
        if update.status.is_some_and(|status| !matches!(status, Status::Open | Status::Done | Status::Dropped)) {
            said.push(format!("G{id}: a goal is open, done or dropped"));
            continue;
        }
        if update.remove {
            let items: Vec<u32> = {
                let mut statement = transaction.prepare("SELECT id FROM checklist_items WHERE agent = ?1 AND goal = ?2")?;
                statement.query_map(params![agent, id], |row| row.get(0))?.collect::<Result<_, _>>()?
            };
            for item in items {
                transaction
                    .execute("UPDATE checklist_items SET goal = 0, updated = ?3 WHERE agent = ?1 AND id = ?2", params![agent, item, now])?;
                record(&transaction, agent, item, now, actor, "goal", "")?;
            }
            let text: String =
                transaction
                    .query_row("SELECT text FROM checklist_goals WHERE agent = ?1 AND id = ?2", params![agent, id], |row| row.get(0))?;
            transaction.execute("DELETE FROM checklist_goals WHERE agent = ?1 AND id = ?2", params![agent, id])?;
            record_goal(&transaction, agent, id, now, actor, "removed", &text)?;
            said.push(format!("G{id} removed"));
            continue;
        }
        let mut parts = Vec::new();
        if let Some(status) = update.status {
            transaction
                .execute("UPDATE checklist_goals SET status = ?3 WHERE agent = ?1 AND id = ?2", params![agent, id, status.name()])?;
            record_goal(&transaction, agent, id, now, actor, "status", status.name())?;
            parts.push(status.name().to_owned());
        }
        if let Some(text) = &update.text {
            transaction.execute("UPDATE checklist_goals SET text = ?3 WHERE agent = ?1 AND id = ?2", params![agent, id, text])?;
            record_goal(&transaction, agent, id, now, actor, "edited", text)?;
            parts.push("edited".to_owned());
        }
        if parts.is_empty() {
            said.push(format!("G{id}: nothing to change"));
            continue;
        }
        transaction.execute("UPDATE checklist_goals SET updated = ?3 WHERE agent = ?1 AND id = ?2", params![agent, id, now])?;
        said.push(format!("G{id} {}", parts.join(", ")));
    }
    transaction.commit()?;
    Ok(said)
}

/// "G2 [ ] Ship the Android app (1/3 items done)"
fn goal_line(goal: &Goal, progress: Option<&(usize, usize)>) -> String {
    let mut line = format!("G{} {} {}", goal.id, goal.status.mark(), goal.text);
    if let Some((done, all)) = progress.filter(|(_, all)| *all > 0) {
        let _ = write!(line, " ({done}/{all} item{} done)", if *all == 1 { "" } else { "s" });
    }
    line
}

/// "#12 [ ] Fix login (frontend) 11-18"
fn line(item: &Item) -> String {
    let mut line = format!("#{} {} {}", item.id, item.status.mark(), item.text);
    if !item.owner.is_empty() {
        let _ = write!(line, " ({})", item.owner);
    }
    if item.goal != 0 {
        let _ = write!(line, " \u{2192} G{}", item.goal);
    }
    let _ = write!(line, " {}", item.day);
    line
}

impl Extension for Checklist {
    fn manifest(&self) -> &'static Manifest {
        &MANIFEST
    }

    fn prompt(&self, _factory: &Factory, _inner: &Inner, _agent: &AgentConfig) -> Vec<PromptSection> {
        vec![PromptSection { kind: PromptKind::Static, text: PROMPT.to_owned() }]
    }

    fn tools(&self, factory: &Arc<Factory>, agent: &str) -> Vec<Tool> {
        let text = || json!({"type": "string", "minLength": 1});
        // Optional parameters are the ones left out of `required` (see `super::tool`).
        let add = json!({
            "type": "object",
            "required": ["items"],
            "properties": {
                "items": {"type": "array", "items": text(), "minItems": 1},
                "owner": {"type": "string"},
                "goal": {"type": "integer", "minimum": 1},
            },
            "additionalProperties": false,
        });
        let status = json!({"type": "string", "enum": ["open", "in_progress", "blocked", "done", "dropped"]});
        let update = json!({
            "type": "object",
            "required": ["updates"],
            "properties": {"updates": {"type": "array", "minItems": 1, "items": {
                "type": "object",
                "required": ["id"],
                "properties": {
                    "id": {"type": "integer", "minimum": 1},
                    "status": status,
                    "blocked_on": {"type": "string", "minLength": 1, "maxLength": 200},
                    "goal": {"type": "integer", "minimum": 0},
                    "owner": {"type": "string"},
                    "text": text(),
                    "note": text(),
                },
                "additionalProperties": false,
            }}},
            "additionalProperties": false,
        });
        let list = json!({
            "type": "object",
            "properties": {
                "agent": {"type": "string"},
                "status": {"type": "string", "enum": ["open", "in_progress", "blocked", "done", "dropped", "all"]},
                "day": {"type": "string"},
                "owner": {"type": "string"},
                "q": {"type": "string"},
                "limit": {"type": "integer", "minimum": 1, "maximum": MAX_LIMIT},
            },
            "additionalProperties": false,
        });
        let item = json!({
            "type": "object",
            "required": ["id"],
            "properties": {"id": {"type": "integer", "minimum": 1}, "agent": {"type": "string"}},
            "additionalProperties": false,
        });
        let goal_text = json!({"type": "string", "minLength": 1, "maxLength": GOAL_LENGTH});
        let goals = json!({
            "type": "object",
            "properties": {
                "add": {"type": "array", "items": goal_text, "minItems": 1},
                "updates": {"type": "array", "minItems": 1, "items": {
                    "type": "object",
                    "required": ["id"],
                    "properties": {
                        "id": {"type": "integer", "minimum": 1},
                        "text": goal_text,
                        "status": {"type": "string", "enum": ["open", "done", "dropped"]},
                        "remove": {"type": "boolean"},
                    },
                    "additionalProperties": false,
                }},
            },
            "additionalProperties": false,
        });
        vec![
            factory_tool(factory, agent, "checklist_add", "Add to checklist", ADD, add, run_add),
            factory_tool(factory, agent, "checklist_update", "Update checklist", UPDATE, update, run_update),
            factory_tool(factory, agent, "checklist_list", "List checklist", LIST, list, run_list),
            factory_tool(factory, agent, "checklist_item", "Checklist item", ITEM, item, run_item),
            factory_tool(factory, agent, "checklist_goals", "Checklist goals", GOALS_TOOL, goals, run_goals),
        ]
    }

    fn view(&self, factory: &Factory, _inner: &Inner, agent: &str) -> Option<v1::ExtensionView> {
        let items = match query(&factory.store, &Query { agent, with_log: true, ..Query::default() }) {
            Ok((items, _)) => items,
            Err(error) => {
                tracing::error!(%agent, %error, "could not read the checklist");
                return None;
            }
        };
        // Whether each item's owning agent has a question waiting for the
        // person, asked once per owner.
        let mut pending: std::collections::HashMap<&str, bool> = std::collections::HashMap::new();
        let mut shown = Vec::new();
        for item in &items {
            let has_pending_ask = item.blocked_on == "person" && {
                let owning = if item.owner.is_empty() { agent } else { item.owner.as_str() };
                *pending.entry(owning).or_insert_with(|| super::ask::pending_of(&factory.store, owning))
            };
            let mut log = Vec::new();
            for change in &item.log {
                log.push(v1::ChecklistUpdate { time: change.time, text: change.text.clone(), author: change.actor.clone() });
            }
            shown.push(v1::ChecklistItem {
                id: item.id,
                text: item.text.clone(),
                status: item.status.proto().into(),
                owner: item.owner.clone(),
                created: item.created,
                updated: item.updated,
                log,
                note: item.blocked_note.clone(),
                blocked_on: item.blocked_on.clone(),
                blocked_since_ms: item.blocked_since.unwrap_or_default(),
                has_pending_ask,
                goal: item.goal,
            });
        }
        let goals = match goals(&factory.store, agent, true) {
            Ok(goals) => goals,
            Err(error) => {
                tracing::error!(%agent, %error, "could not read the checklist's goals");
                return None;
            }
        };
        let goals = goals
            .into_iter()
            .map(|goal| v1::Goal {
                id: goal.id,
                text: goal.text,
                status: goal.status.proto().into(),
                created: goal.created,
                updated: goal.updated,
                log: goal
                    .log
                    .into_iter()
                    .map(|change| v1::ChecklistUpdate { time: change.time, text: change.text, author: change.actor })
                    .collect(),
            })
            .collect();
        Some(v1::ExtensionView {
            extension_id: ID.to_owned(),
            agent_id: agent.to_owned(),
            title: MANIFEST.name.to_owned(),
            body: Some(v1::extension_view::Body::Checklist(v1::Checklist { items: shown, goals })),
        })
    }
}

fn failed(error: impl std::fmt::Display) -> String {
    format!("The checklist couldn't be read or saved: {error}")
}

/// The id of the agent `name` means (an id or a name; a name several agents
/// share means the one in `me`'s project).
fn agent_id(factory: &Factory, me: &str, name: &str) -> Result<String, String> {
    let inner = factory.lock();
    let config = &inner.config;
    let me = super::caller(config, me)?;
    let everyone: Vec<&AgentConfig> = config.agents.iter().collect();
    match super::agents::find(config, me, &everyone, name)? {
        Some(agent) => Ok(agent.id.clone()),
        None => Err(format!("No agent has the id or name \"{}\".", name.trim())),
    }
}

/// Whose checklist to read: `asked` (an id or name), or the agent's own.
fn whose(factory: &Factory, agent: &str, asked: Option<&str>) -> Result<String, String> {
    match asked.map(str::trim).filter(|asked| !asked.is_empty()) {
        Some(asked) => agent_id(factory, agent, asked),
        None => Ok(agent.to_owned()),
    }
}

#[derive(Deserialize)]
struct AddArgs {
    items: Vec<String>,
    #[serde(default)]
    owner: Option<String>,
    #[serde(default)]
    goal: Option<u32>,
}

fn run_add(factory: &Factory, agent: &str, args: serde_json::Value) -> Result<ToolResult, String> {
    let args: AddArgs = serde_json::from_value(args).map_err(|error| error.to_string())?;
    let texts: Vec<String> = args.items.iter().map(|text| text.trim().to_owned()).filter(|text| !text.is_empty()).collect();
    if texts.is_empty() {
        return Err("Nothing to add: every item was empty.".to_owned());
    }
    let owner = match args.owner.as_deref().map(str::trim).filter(|owner| !owner.is_empty()) {
        Some(owner) => agent_id(factory, agent, owner)?,
        None => String::new(),
    };
    let goal = args.goal.unwrap_or(0);
    if goal != 0 && !has_goal(&factory.store.db(), agent, goal).map_err(failed)? {
        return Err(format!("No goal G{goal} on your checklist: add it with checklist_goals first, if the person asked for it."));
    }
    let ids = add(&factory.store, agent, agent, &texts, &owner, goal).map_err(failed)?;
    factory.refresh_view(agent);
    let said = match ids.as_slice() {
        [id] => format!("Added #{id}."),
        [first, .., last] => format!("Added #{first}-#{last}."),
        [] => "Added nothing.".to_owned(),
    };
    let items: Vec<_> = ids.iter().zip(&texts).map(|(id, text)| json!({"id": id, "text": text, "owner": owner})).collect();
    Ok(ToolResult::text(said, json!({"agent": agent, "items": items})))
}

#[derive(Deserialize)]
struct UpdateArgs {
    updates: Vec<UpdateArg>,
}

#[derive(Deserialize)]
struct UpdateArg {
    id: u32,
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    blocked_on: Option<String>,
    #[serde(default)]
    goal: Option<u32>,
    #[serde(default)]
    owner: Option<String>,
    #[serde(default)]
    text: Option<String>,
    #[serde(default)]
    note: Option<String>,
}

fn run_update(factory: &Factory, agent: &str, args: serde_json::Value) -> Result<ToolResult, String> {
    let args: UpdateArgs = serde_json::from_value(args).map_err(|error| error.to_string())?;
    let person = factory.lock().config.person().to_ascii_lowercase();
    let mut updates = Vec::new();
    for arg in args.updates {
        let status = match arg.status.as_deref() {
            Some(name) => Some(
                Status::parse(name).ok_or_else(|| format!("#{}: status must be open, in_progress, blocked, done or dropped", arg.id))?,
            ),
            None => None,
        };
        let owner = match arg.owner.as_deref().map(str::trim) {
            Some("") => Some(String::new()),
            Some(owner) => Some(agent_id(factory, agent, owner).map_err(|problem| format!("#{}: {problem}", arg.id))?),
            None => None,
        };
        let text = arg.text.map(|text| text.trim().to_owned()).filter(|text| !text.is_empty());
        let note = arg.note.map(|note| note.trim().to_owned()).filter(|note| !note.is_empty());
        // The person however named ("Person", their name) keys the daily
        // reminder, so it's kept as the one word.
        let blocked_on = arg.blocked_on.map(|on| on.trim().to_owned()).filter(|on| !on.is_empty()).map(|on| {
            let lower = on.to_ascii_lowercase();
            if lower == "person" || lower == "the person" || lower == person { "person".to_owned() } else { on }
        });
        updates.push(Update { id: arg.id, status, owner, text, note, blocked_on, goal: arg.goal });
    }
    let said = update(&factory.store, agent, agent, &updates).map_err(failed)?;
    factory.refresh_view(agent);
    // Each item as it is now, with what changed.
    let (now, _) = query(&factory.store, &Query { agent, ..Query::default() }).map_err(failed)?;
    let items: Vec<_> = updates
        .iter()
        .map(|update| {
            let changed: Vec<&str> = [
                (update.status.is_some(), "status"),
                (update.blocked_on.is_some(), "blocked_on"),
                (update.goal.is_some(), "goal"),
                (update.owner.is_some(), "owner"),
                (update.text.is_some(), "text"),
                (update.note.is_some(), "note"),
            ]
            .into_iter()
            .filter_map(|(changed, what)| changed.then_some(what))
            .collect();
            match now.iter().find(|item| item.id == update.id) {
                None => json!({"id": update.id, "problem": "no such item"}),
                Some(_) if changed.is_empty() => json!({"id": update.id, "problem": "nothing to change"}),
                Some(item) => {
                    let mut shown =
                        json!({"id": item.id, "text": item.text, "status": item.status.name(), "owner": item.owner, "changed": changed});
                    if let Some(note) = &update.note {
                        shown["note"] = json!(note);
                    }
                    shown
                }
            }
        })
        .collect();
    Ok(ToolResult::text(format!("{}.", said.join("; ")), json!({"agent": agent, "items": items})))
}

#[derive(Deserialize)]
struct ListArgs {
    #[serde(default)]
    agent: Option<String>,
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    day: Option<String>,
    #[serde(default)]
    owner: Option<String>,
    #[serde(default)]
    q: Option<String>,
    #[serde(default)]
    limit: Option<usize>,
}

fn run_list(factory: &Factory, agent: &str, args: serde_json::Value) -> Result<ToolResult, String> {
    let args: ListArgs = serde_json::from_value(args).map_err(|error| error.to_string())?;
    let whose = whose(factory, agent, args.agent.as_deref())?;
    let statuses = match args.status.as_deref().map(str::trim).filter(|status| !status.is_empty()) {
        None => Some(OUTSTANDING.to_vec()),
        Some("all") => None,
        Some(name) => Some(vec![Status::parse(name).ok_or("status must be open, in_progress, blocked, done, dropped or all")?]),
    };
    let owner = match args.owner.as_deref().map(str::trim).filter(|owner| !owner.is_empty()) {
        Some(owner) => Some(agent_id(factory, agent, owner)?),
        None => None,
    };
    let day = args.day.as_deref().map(str::trim).filter(|day| !day.is_empty());
    if let Some(day) = day
        && factory_agent::clock::parse_iso_ms(&format!("{day}T00:00:00Z")).is_none()
    {
        return Err(format!("day \"{day}\" must look like 2026-11-18"));
    }
    let limit = args.limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT);
    let request = Query {
        agent: &whose,
        statuses,
        day,
        owner: owner.as_deref(),
        text: args.q.as_deref().map(str::trim).filter(|q| !q.is_empty()),
        limit: Some(limit),
        with_log: false,
    };
    let every_status = request.statuses.is_none();
    let filtered = request.day.is_some() || request.owner.is_some() || request.text.is_some();
    let (items, total) = query(&factory.store, &request).map_err(failed)?;
    // Goals first: the open ones, or all with status all; not when
    // narrowing items by day, owner or text.
    let mut lines: Vec<String> = Vec::new();
    if !filtered {
        let progress = progress(&factory.store, &whose).map_err(failed)?;
        for goal in goals(&factory.store, &whose, false).map_err(failed)? {
            if every_status || goal.status == Status::Open {
                lines.push(goal_line(&goal, progress.get(&goal.id)));
            }
        }
    }
    let goals_shown = lines.len();
    let details = json!({"agent": whose, "shown": items.len(), "total": total, "goals": goals_shown});
    if items.is_empty() {
        lines.push("No items match.".to_owned());
        return Ok(ToolResult::text(lines.join("\n"), details));
    }
    lines.extend(items.iter().map(line));
    if total > items.len() {
        lines.push(format!("+{} more; narrow with q, day, owner or status, or raise limit.", total - items.len()));
    }
    Ok(ToolResult::text(lines.join("\n"), details))
}

#[derive(Deserialize)]
struct ItemArgs {
    id: u32,
    #[serde(default)]
    agent: Option<String>,
}

fn run_item(factory: &Factory, agent: &str, args: serde_json::Value) -> Result<ToolResult, String> {
    let args: ItemArgs = serde_json::from_value(args).map_err(|error| error.to_string())?;
    let whose = whose(factory, agent, args.agent.as_deref())?;
    let (items, _) = query(&factory.store, &Query { agent: &whose, with_log: true, ..Query::default() }).map_err(failed)?;
    let Some(item) = items.into_iter().find(|item| item.id == args.id) else {
        return Err(format!("No item #{} on {whose}'s checklist.", args.id));
    };
    let mut lines = vec![line(&item)];
    for change in &item.log {
        let actor = if change.actor.is_empty() { "person" } else { change.actor.as_str() };
        lines.push(format!("{} {actor}: {}", change.when, change.text));
    }
    let details = json!({"agent": whose, "id": item.id, "text": item.text, "status": item.status.name(), "owner": item.owner});
    Ok(ToolResult::text(lines.join("\n"), details))
}

#[derive(Deserialize)]
struct GoalsArgs {
    #[serde(default)]
    add: Vec<String>,
    #[serde(default)]
    updates: Vec<GoalArg>,
}

#[derive(Deserialize)]
struct GoalArg {
    id: u32,
    #[serde(default)]
    text: Option<String>,
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    remove: bool,
}

/// A goal's status, as a goal has them: open, done or dropped.
fn goal_status(name: &str) -> Option<Status> {
    Status::parse(name).filter(|status| matches!(status, Status::Open | Status::Done | Status::Dropped))
}

fn run_goals(factory: &Factory, agent: &str, args: serde_json::Value) -> Result<ToolResult, String> {
    let args: GoalsArgs = serde_json::from_value(args).map_err(|error| error.to_string())?;
    let texts: Vec<String> = args.add.iter().map(|text| text.trim().to_owned()).filter(|text| !text.is_empty()).collect();
    if texts.is_empty() && args.updates.is_empty() {
        return Err("Nothing to do: give goals to add, or updates.".to_owned());
    }
    if let Some(long) = texts.iter().find(|text| text.chars().count() > GOAL_LENGTH) {
        return Err(format!("A goal is at most {GOAL_LENGTH} characters; \"{}…\" is longer.", long.chars().take(40).collect::<String>()));
    }
    let mut updates = Vec::new();
    for arg in args.updates {
        let status = match arg.status.as_deref() {
            Some(name) => Some(goal_status(name).ok_or_else(|| format!("G{}: status must be open, done or dropped", arg.id))?),
            None => None,
        };
        let text = arg.text.map(|text| text.trim().to_owned()).filter(|text| !text.is_empty());
        updates.push(GoalUpdate { id: arg.id, text, status, remove: arg.remove });
    }
    let mut said = Vec::new();
    let added = add_goals(&factory.store, agent, agent, &texts).map_err(failed)?;
    match added.as_slice() {
        [] => {}
        [id] => said.push(format!("Added G{id}")),
        [first, .., last] => said.push(format!("Added G{first}-G{last}")),
    }
    said.extend(update_goals(&factory.store, agent, agent, &updates).map_err(failed)?);
    factory.refresh_view(agent);
    let details = json!({"agent": agent, "added": added});
    Ok(ToolResult::text(format!("{}.", said.join("; ")), details))
}

/// The person adds a goal to `agent`'s checklist (AddGoal); the agent is told.
/// The person's goals go only to an agent with its Checklist on: one
/// without it has no tools to act on them.
fn checklist_on(factory: &Factory, agent: &str) -> Result<(), crate::factory::RequestError> {
    factory.known_agent(agent)?;
    let inner = factory.lock();
    match inner.config.agent(agent) {
        Some(config) if !super::enabled(config, ID) => {
            Err(crate::factory::RequestError::Checklist(format!("{}'s Checklist is off.", config.name)))
        }
        _ => Ok(()),
    }
}

pub(crate) fn person_add_goal(factory: &Arc<Factory>, request: &v1::AddGoal) -> Result<(), crate::factory::RequestError> {
    use crate::factory::RequestError;
    checklist_on(factory, &request.agent_id)?;
    let text = request.text.trim();
    if text.is_empty() {
        return Err(RequestError::Empty("A goal"));
    }
    if text.chars().count() > GOAL_LENGTH {
        return Err(RequestError::Checklist(format!("A goal is at most {GOAL_LENGTH} characters.")));
    }
    let ids = add_goals(&factory.store, &request.agent_id, "", &[text.to_owned()])?;
    factory.refresh_view(&request.agent_id);
    let person = factory.lock().config.person_capitalized().to_owned();
    let id = ids.first().copied().unwrap_or_default();
    let said = format!(
        "{person} added a goal to your checklist: G{id} \"{text}\". Link the items that serve it with goal: {id} on checklist_add or checklist_update, and add items for what it still needs."
    );
    factory.deliver_event(&request.agent_id, said, crate::factory::host_event(format!("Goal G{id} added")));
    Ok(())
}

/// The person changes one of `agent`'s goals (UpdateGoal); the agent is told.
pub(crate) fn person_update_goal(factory: &Arc<Factory>, request: &v1::UpdateGoal) -> Result<(), crate::factory::RequestError> {
    use crate::factory::RequestError;
    checklist_on(factory, &request.agent_id)?;
    let id = request.goal_id;
    let status = match request.status() {
        v1::ChecklistStatus::Unspecified => None,
        v1::ChecklistStatus::Open => Some(Status::Open),
        v1::ChecklistStatus::Done => Some(Status::Done),
        v1::ChecklistStatus::Dropped => Some(Status::Dropped),
        _ => return Err(RequestError::Checklist("A goal is open, done or dropped.".to_owned())),
    };
    let text = match request.text.as_deref().map(str::trim) {
        Some("") => return Err(RequestError::Empty("A goal")),
        Some(text) if text.chars().count() > GOAL_LENGTH => {
            return Err(RequestError::Checklist(format!("A goal is at most {GOAL_LENGTH} characters.")));
        }
        other => other.map(str::to_owned),
    };
    let before = goals(&factory.store, &request.agent_id, false)?.into_iter().find(|goal| goal.id == id);
    let Some(before) = before else { return Err(RequestError::Checklist(format!("There's no goal G{id}."))) };
    let update = GoalUpdate { id, text: text.clone(), status, remove: request.remove };
    let said = update_goals(&factory.store, &request.agent_id, "", &[update])?;
    factory.refresh_view(&request.agent_id);
    if said.iter().any(|said| said.ends_with("nothing to change")) {
        return Ok(());
    }
    let person = factory.lock().config.person_capitalized().to_owned();
    let mut what = Vec::new();
    if request.remove {
        what.push(format!("removed goal G{id} \"{}\"; the items that served it serve no goal now", before.text));
    } else {
        if let Some(text) = &text {
            what.push(format!("changed goal G{id} to \"{text}\""));
        }
        if let Some(status) = status {
            let goal = if text.is_some() { "it".to_owned() } else { format!("goal G{id}") };
            what.push(match status {
                Status::Done => format!("marked {goal} done"),
                Status::Dropped => format!("dropped {goal}"),
                _ => format!("reopened {goal}"),
            });
        }
    }
    let said = format!("{person} {} on your checklist.", what.join(" and "));
    factory.deliver_event(&request.agent_id, said, crate::factory::host_event(format!("Goal G{id} changed")));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> Store {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("factoryd-checklist-{}-{}-{n}", std::process::id(), now_ms()));
        std::fs::create_dir_all(&dir).expect("dir");
        Store::open(&dir.join("factory.db")).expect("store")
    }

    #[test]
    fn items_are_added_updated_and_listed_with_their_history() {
        let store = store();
        let texts = ["Fix login".to_owned(), "Dark mode".to_owned(), "Ship release notes".to_owned()];
        assert_eq!(add(&store, "coordinator", "coordinator", &texts, "", 0).expect("add"), [1, 2, 3]);
        assert_eq!(add(&store, "frontend", "frontend", &["Its own".to_owned()], "", 0).expect("add"), [1], "ids are per agent");
        let updates = [
            Update { id: 1, status: Some(Status::Done), note: Some("shipped".into()), ..Update::default() },
            Update { id: 2, owner: Some("frontend".into()), ..Update::default() },
            Update { id: 9, status: Some(Status::Done), ..Update::default() },
            Update { id: 3, ..Update::default() },
        ];
        let said = update(&store, "coordinator", "coordinator", &updates).expect("update");
        assert_eq!(said, ["#1 done, noted", "#2 owner frontend", "#9: no such item", "#3: nothing to change"]);

        let open = Query { agent: "coordinator", statuses: Some(vec![Status::Open, Status::InProgress]), ..Query::default() };
        let (items, total) = query(&store, &open).expect("query");
        assert_eq!(total, 2);
        let lines: Vec<String> = items.iter().map(|item| line(item).rsplit_once(' ').expect("day").0.to_owned()).collect();
        assert_eq!(lines, ["#2 [ ] Dark mode (frontend)", "#3 [ ] Ship release notes"]);

        let limited = Query { agent: "coordinator", limit: Some(1), text: Some("dark"), ..Query::default() };
        let (items, total) = query(&store, &limited).expect("query");
        assert_eq!((items.len(), total, items[0].id), (1, 1, 2), "text search ignores case");
        let none = Query { agent: "coordinator", text: Some("%"), ..Query::default() };
        assert_eq!(query(&store, &none).expect("query").1, 0, "wildcards are plain text");

        let (items, _) = query(&store, &Query { agent: "coordinator", with_log: true, ..Query::default() }).expect("query");
        let log: Vec<(&str, &str)> = items[0].log.iter().map(|change| (change.actor.as_str(), change.text.as_str())).collect();
        assert_eq!(log, [("coordinator", "Added"), ("coordinator", "Done: shipped")]);
        assert_eq!(items[1].log.last().map(|change| change.text.as_str()), Some("Owner: frontend"));
        assert_eq!(items[0].status, Status::Done);

        // Every change is kept as an event, with when it happened.
        let events: i64 =
            store.db().query_row("SELECT COUNT(*) FROM checklist_events WHERE agent = 'coordinator'", [], |row| row.get(0)).expect("count");
        assert_eq!(events, 3 + 2 + 1);

        forget(&store, "coordinator").expect("forget");
        assert_eq!(query(&store, &Query { agent: "coordinator", ..Query::default() }).expect("query").1, 0);
        assert_eq!(query(&store, &Query { agent: "frontend", ..Query::default() }).expect("query").1, 1);
    }

    #[test]
    fn blocked_on_rides_with_blocked_and_the_clock_starts_once() {
        let store = store();
        add(&store, "lead", "lead", &["Ship it".to_owned()], "", 0).expect("add");

        // blocked_on without a blocked item is turned away whole.
        let said =
            update(&store, "lead", "lead", &[Update { id: 1, blocked_on: Some("person".into()), ..Update::default() }]).expect("update");
        assert_eq!(said, ["#1: blocked_on goes with status blocked"]);

        // Becoming blocked starts the clock and keeps what it blocks on.
        let both = Update { id: 1, status: Some(Status::Blocked), blocked_on: Some("person".into()), ..Update::default() };
        assert_eq!(update(&store, "lead", "lead", &[both]).expect("update"), ["#1 blocked, blocks on person"]);
        let (items, _) = query(&store, &Query { agent: "lead", ..Query::default() }).expect("query");
        let since = items[0].blocked_since.expect("a clock");
        assert_eq!(items[0].blocked_on, "person");

        // Still blocked, the clock holds; the reason can change alone.
        let again = Update { id: 1, status: Some(Status::Blocked), blocked_on: Some("the CI queue".into()), ..Update::default() };
        update(&store, "lead", "lead", &[again]).expect("update");
        let (items, _) = query(&store, &Query { agent: "lead", ..Query::default() }).expect("query");
        assert_eq!((items[0].blocked_since, items[0].blocked_on.as_str()), (Some(since), "the CI queue"));

        // Unblocking clears both.
        update(&store, "lead", "lead", &[Update { id: 1, status: Some(Status::InProgress), ..Update::default() }]).expect("update");
        let (items, _) = query(&store, &Query { agent: "lead", ..Query::default() }).expect("query");
        assert_eq!((items[0].blocked_since, items[0].blocked_on.as_str()), (None, ""));

        // The history kept each turn.
        let (items, _) = query(&store, &Query { agent: "lead", with_log: true, ..Query::default() }).expect("query");
        let texts: Vec<&str> = items[0].log.iter().map(|change| change.text.as_str()).collect();
        assert!(texts.contains(&"Blocks on: person") && texts.contains(&"Blocks on: the CI queue"), "{texts:?}");
    }

    #[test]
    fn goals_are_kept_apart_from_items_and_items_can_serve_them() {
        let store = store();
        let texts = ["A link to the Android app I can share".to_owned(), "Fewer search outages".to_owned()];
        assert_eq!(add_goals(&store, "lead", "", &texts).expect("goals"), [1, 2]);
        assert_eq!(add(&store, "lead", "lead", &["Build the APK".to_owned(), "Host it".to_owned()], "", 1).expect("add"), [1, 2]);
        add(&store, "lead", "lead", &["Unrelated".to_owned()], "", 0).expect("add");

        // An item serves a goal that exists, and can stop serving it.
        let link = |id, goal| Update { id, goal: Some(goal), ..Update::default() };
        let said = update(&store, "lead", "lead", &[link(3, 7), link(3, 2), link(2, 0)]).expect("update");
        assert_eq!(said, ["#3: no goal G7", "#3 serves G2", "#2 no goal"]);
        let done = Update { id: 1, status: Some(Status::Done), ..Update::default() };
        update(&store, "lead", "lead", &[done]).expect("update");
        let progress = progress(&store, "lead").expect("progress");
        assert_eq!((progress.get(&1), progress.get(&2)), (Some(&(1, 1)), Some(&(0, 1))));

        // A goal is open, done or dropped; changes say what changed.
        let changes = [
            GoalUpdate { id: 1, status: Some(Status::Blocked), ..GoalUpdate::default() },
            GoalUpdate { id: 1, status: Some(Status::Done), text: Some("A link anyone can open".into()), ..GoalUpdate::default() },
            GoalUpdate { id: 9, status: Some(Status::Done), ..GoalUpdate::default() },
            GoalUpdate { id: 2, ..GoalUpdate::default() },
        ];
        let said = update_goals(&store, "lead", "lead", &changes).expect("goals");
        assert_eq!(said, ["G1: a goal is open, done or dropped", "G1 done, edited", "G9: no such goal", "G2: nothing to change"]);
        let kept = goals(&store, "lead", true).expect("goals");
        assert_eq!((kept[0].status, kept[0].text.as_str()), (Status::Done, "A link anyone can open"));
        let log: Vec<(&str, &str)> = kept[0].log.iter().map(|change| (change.actor.as_str(), change.text.as_str())).collect();
        assert_eq!(log, [("", "Added"), ("lead", "Done"), ("lead", "Edited: A link anyone can open")]);

        // Removed, its items serve none, and its id isn't given again.
        assert_eq!(
            update_goals(&store, "lead", "", &[GoalUpdate { id: 2, remove: true, ..GoalUpdate::default() }]).expect("goals"),
            ["G2 removed"]
        );
        let (items, _) = query(&store, &Query { agent: "lead", with_log: true, ..Query::default() }).expect("query");
        assert_eq!(items[2].goal, 0);
        assert_eq!(items[2].log.last().map(|change| change.text.as_str()), Some("No goal"));
        assert_eq!(add_goals(&store, "lead", "lead", &["Next".to_owned()]).expect("goals"), [3]);
        assert_eq!(add_goals(&store, "other", "other", &["Its own".to_owned()]).expect("goals"), [1], "ids are per agent");
    }

    #[test]
    fn statuses_read_loosely() {
        assert_eq!(Status::parse("In progress"), Some(Status::InProgress));
        assert_eq!(Status::parse("in-progress"), Some(Status::InProgress));
        assert_eq!(Status::parse("DONE"), Some(Status::Done));
        assert_eq!(Status::parse("finished"), None);
    }
}
