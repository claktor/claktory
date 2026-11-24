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

use super::{Extension, Manifest, OptionKind, OptionSpec, PromptSection, factory_tool};
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

/// The migration that typed blocks (#279, #282): what a blocked item waits
/// on (`blocked_kind`: person_decision, person_action, item, agent or
/// external; '' for untyped, as before), whom or what (`blocked_ref`: an
/// agent id, or "agent#12" for an item), when an outside wait is checked
/// again or a parked item resumes (`retry_at`; `retries` so far, each
/// twice as long as the last), when the person is asked again after
/// dismissing its question (`reask_at`, `dismissed_at`), when its owner was
/// last asked whether it's still blocked (`prompted_at`), and when its
/// owner's lead was told it's overdue (`overdue_told`). The item an ask is
/// for ("agent#12"; '' for none). And Settings → Checklist's intervals, in
/// minutes, by name; a missing one is its default.
pub(crate) const WAITS: &str = "
ALTER TABLE checklist_items ADD COLUMN blocked_kind TEXT NOT NULL DEFAULT '';
ALTER TABLE checklist_items ADD COLUMN blocked_ref TEXT NOT NULL DEFAULT '';
ALTER TABLE checklist_items ADD COLUMN retry_at INTEGER;
ALTER TABLE checklist_items ADD COLUMN retries INTEGER NOT NULL DEFAULT 0;
ALTER TABLE checklist_items ADD COLUMN reask_at INTEGER;
ALTER TABLE checklist_items ADD COLUMN dismissed_at INTEGER;
ALTER TABLE checklist_items ADD COLUMN prompted_at INTEGER;
ALTER TABLE checklist_items ADD COLUMN overdue_told INTEGER;
ALTER TABLE asks ADD COLUMN item TEXT NOT NULL DEFAULT '';
CREATE TABLE checklist_settings (name TEXT PRIMARY KEY, minutes INTEGER NOT NULL);
UPDATE checklist_items SET blocked_kind = 'person_decision' WHERE status = 'blocked' AND blocked_on = 'person';
";

/// Settings → Checklist (#282): how Claktory follows up on blocked items,
/// in minutes. Kept in factory.db, not config.json, which an older backend
/// would refuse whole after a rollback.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Times {
    /// A block on another agent or item, or of no kind: its owner is asked
    /// whether it's still blocked this long after, and as often again.
    pub agent_blocked: i64,
    /// The same for a wait on the person.
    pub person_blocked: i64,
    /// An outside wait with no retry_at is checked again this long after,
    /// then each time twice as long, a day at most (or this, if longer).
    pub external_retry: i64,
    /// After the person dismisses a question about an item that still
    /// waits on them, its owner asks again this long after, as often.
    pub reask: i64,
    /// A block this old is told to its owner's lead, once.
    pub overdue: i64,
}

impl Times {
    const DEFAULT: Times =
        Times { agent_blocked: 4 * 60, person_blocked: 24 * 60, external_retry: 2 * 60, reask: 24 * 60, overdue: 24 * 60 };
    /// Each can be from 15 minutes to a week.
    const RANGE: std::ops::RangeInclusive<u32> = 15..=7 * 24 * 60;

    fn names(&mut self) -> [(&'static str, &mut i64); 5] {
        [
            ("agent_blocked", &mut self.agent_blocked),
            ("person_blocked", &mut self.person_blocked),
            ("external_retry", &mut self.external_retry),
            ("reask", &mut self.reask),
            ("overdue", &mut self.overdue),
        ]
    }

    /// The intervals as set, each its default until changed.
    pub(crate) fn read(store: &Store) -> Times {
        let mut times = Times::DEFAULT;
        let db = store.db();
        for (name, value) in times.names() {
            let set: Option<i64> =
                db.query_row("SELECT minutes FROM checklist_settings WHERE name = ?1", [name], |row| row.get(0)).optional().ok().flatten();
            if let Some(set) = set {
                *value = set;
            }
        }
        times
    }

    pub(crate) fn proto(self) -> v1::ChecklistSettings {
        let minutes = |value: i64| u32::try_from(value).unwrap_or_default();
        v1::ChecklistSettings {
            agent_blocked_minutes: minutes(self.agent_blocked),
            person_blocked_minutes: minutes(self.person_blocked),
            external_retry_minutes: minutes(self.external_retry),
            reask_minutes: minutes(self.reask),
            overdue_minutes: minutes(self.overdue),
        }
    }

    /// Sets what `request` gives (0 leaves one as it is), or nothing when
    /// any is out of range. Says whether anything changed.
    pub(crate) fn set(store: &Store, request: &v1::ChecklistSettings) -> Result<bool, String> {
        let given = [
            ("agent_blocked", request.agent_blocked_minutes),
            ("person_blocked", request.person_blocked_minutes),
            ("external_retry", request.external_retry_minutes),
            ("reask", request.reask_minutes),
            ("overdue", request.overdue_minutes),
        ];
        if given.iter().any(|(_, minutes)| *minutes != 0 && !Times::RANGE.contains(minutes)) {
            return Err(format!("Choose from {} minutes to {} days.", Times::RANGE.start(), Times::RANGE.end() / (24 * 60)));
        }
        let before = Times::read(store);
        let db = store.db();
        for (name, minutes) in given.into_iter().filter(|(_, minutes)| *minutes != 0) {
            db.execute("INSERT OR REPLACE INTO checklist_settings (name, minutes) VALUES (?1, ?2)", params![name, minutes])
                .map_err(|error| error.to_string())?;
        }
        drop(db);
        Ok(Times::read(store) != before)
    }
}

/// An outside wait's retries: each twice as long as the last, from `base`
/// minutes, at most a day (or `base`, if that's longer).
fn retry_minutes(base: i64, retries: i64) -> i64 {
    let most = base.max(24 * 60);
    base.saturating_mul(1_i64 << retries.clamp(0, 20)).min(most)
}

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
- blocked means nothing can move until something comes; say what with waits_on: person_decision (a decision only the person can make), person_action (something only they can do), item (another item, ref 12 or Ana#12: it opens again by itself when that one is done or dropped), agent (ref, its id or name: it's told you're waiting) or external (something outside; you're told to check again at retry_at, such as 2h or \"2026-11-23 14:00\", or after a while if you give none, then less and less often). blocked_on adds a few words if useful; say more in a note.
- An item waiting on the person shows in their Needs you only as your question about it: blocked on a decision of theirs or something only they can do, file ask_person then and there with item naming it, one card per decision or action (steps for a procedure), never several on one card. A question without item covers no item. An item that waits on them is one concern: if it bundles several, split it into separate items first, each with its own card. Never asked, dismissed, or waiting to be asked again, it shows only on your checklist, not to them. When they answer, clear the block. If they dismiss it, decide yourself if you can and note what you chose; if you truly can't move without them, leave it blocked with a note why, and Claktory has you ask again later.
- Now and then Claktory asks whether a blocked item is still blocked: try it again, since it may not be. A block that lasts too long (a day, unless the person changed it) is told to your lead.
- parked means set aside for later (after a release, say) or ongoing, with nothing to wait on: never mark that blocked. Parked items aren't nudged; retry_at sets when one resumes, if you know.
- Goals are the person's: outcomes they want (\"a link to the Android app I can share\"). Record one with checklist_goals only when the person states it, in their words, never your own plans; the person also adds and changes goals in the app, and you're told when they do. Link the items that serve a goal with `goal` on checklist_add or checklist_update. Mark a goal done only when the person's outcome is reached, or dropped when they say so.
- checklist_list shows open goals, then open items; filter by status, day, owner or text. checklist_item shows one item's history. Both take `agent` to read another agent's checklist.
- The person can reply on an item from the app: it comes as their message, its `<sent>` tag naming the item (about=\"checklist item #12: …\", or Ana#12 on another agent's checklist), and the item's history keeps it. It changes nothing by itself: act on it, then update the item.
- Check the list before telling the person work is finished.
";

const ADD: &str = "Add items to your checklist, many at once. goal: the id of the goal they serve (2 for G2). Returns their ids.";
const UPDATE: &str = "Change items on your checklist: status (open, in_progress, blocked, parked, done, dropped); with status blocked, waits_on (person_decision, person_action, item, agent or external), ref (item: 12 or Ana#12; agent: its id or name) and blocked_on (a few words naming the blocker; \"person\" alone means a decision of the person's); retry_at (2h, 1d or \"2026-11-23 14:00\": when you're told to check again, with waits_on external; when it resumes, with status parked), goal (the id of the goal it serves, 2 for G2; 0 for none), owner (an agent id or name; empty to unassign), text, and/or a short note.";
const LIST: &str = "List the checklist: its open goals (all of them with status all), then its items, one line each, oldest first: open, in progress, blocked and parked unless `status` says otherwise (open, in_progress, blocked, parked, done, dropped, all). Filter items by `day` added (YYYY-MM-DD), `owner` or text `q`. `agent` reads another agent's checklist.";
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
    /// Ongoing or waiting its turn, with nothing to wait on: still
    /// outstanding, never nudged.
    Parked,
}

/// What's still to do: everything not done or dropped.
const OUTSTANDING: [Status; 4] = [Status::Open, Status::InProgress, Status::Blocked, Status::Parked];

impl Status {
    const ALL: [Status; 6] = [Status::Open, Status::InProgress, Status::Done, Status::Dropped, Status::Blocked, Status::Parked];

    fn name(self) -> &'static str {
        match self {
            Status::Open => "open",
            Status::InProgress => "in_progress",
            Status::Done => "done",
            Status::Dropped => "dropped",
            Status::Blocked => "blocked",
            Status::Parked => "parked",
        }
    }

    pub(crate) fn parse(name: &str) -> Option<Status> {
        Status::ALL.into_iter().find(|status| status.name() == name.trim().to_ascii_lowercase().replace([' ', '-'], "_"))
    }

    fn mark(self) -> &'static str {
        match self {
            Status::Open => "[ ]",
            Status::InProgress => "[~]",
            Status::Done => "[x]",
            Status::Dropped => "[-]",
            Status::Blocked => "[!]",
            Status::Parked => "[=]",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Status::Open => "Open",
            Status::InProgress => "In progress",
            Status::Done => "Done",
            Status::Dropped => "Dropped",
            Status::Blocked => "Blocked",
            Status::Parked => "Parked",
        }
    }

    fn proto(self) -> v1::ChecklistStatus {
        match self {
            Status::Open => v1::ChecklistStatus::Open,
            Status::InProgress => v1::ChecklistStatus::InProgress,
            Status::Done => v1::ChecklistStatus::Done,
            Status::Dropped => v1::ChecklistStatus::Dropped,
            Status::Blocked => v1::ChecklistStatus::Blocked,
            Status::Parked => v1::ChecklistStatus::Parked,
        }
    }
}

/// What a blocked item waits on (#279).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum Wait {
    /// Untyped: a blocked item from before, or one given only blocked_on words.
    #[default]
    Untyped,
    /// A decision only the person can make.
    PersonDecision,
    /// Something only the person can do (sign in, pay, plug in).
    PersonAction,
    /// Another checklist item: it opens again when that one is done or dropped.
    Item,
    /// Another agent.
    Agent,
    /// Something outside, checked again at its retry time.
    External,
}

impl Wait {
    const ALL: [Wait; 5] = [Wait::PersonDecision, Wait::PersonAction, Wait::Item, Wait::Agent, Wait::External];

    pub(crate) fn name(self) -> &'static str {
        match self {
            Wait::Untyped => "",
            Wait::PersonDecision => "person_decision",
            Wait::PersonAction => "person_action",
            Wait::Item => "item",
            Wait::Agent => "agent",
            Wait::External => "external",
        }
    }

    pub(crate) fn parse(name: &str) -> Option<Wait> {
        let name = name.trim().to_ascii_lowercase().replace([' ', '-'], "_");
        Wait::ALL.into_iter().find(|wait| wait.name() == name)
    }

    fn read(name: &str) -> Wait {
        Wait::parse(name).unwrap_or_default()
    }

    pub(crate) fn is_person(self) -> bool {
        matches!(self, Wait::PersonDecision | Wait::PersonAction)
    }

    fn proto(self) -> v1::BlockedKind {
        match self {
            Wait::Untyped => v1::BlockedKind::Unspecified,
            Wait::PersonDecision => v1::BlockedKind::PersonDecision,
            Wait::PersonAction => v1::BlockedKind::PersonAction,
            Wait::Item => v1::BlockedKind::Item,
            Wait::Agent => v1::BlockedKind::Agent,
            Wait::External => v1::BlockedKind::External,
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
    /// While blocked: what kind of thing it waits on, and whom or what (an
    /// agent id, or "agent#12" for an item).
    pub wait: Wait,
    pub wait_ref: String,
    /// While it waits on something outside: when it's checked again.
    pub retry_at: Option<i64>,
    /// While it waits on the person after they dismissed its question: when
    /// Claktory has it asked again; None when it won't be.
    pub reask_at: Option<i64>,
    /// When the person last dismissed a question about it while blocked.
    pub dismissed_at: Option<i64>,
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
        "SELECT id, text, status, owner, created, updated, strftime('%m-%d', created / 1000, 'unixepoch', 'localtime'), blocked_on, blocked_since, goal,
                blocked_kind, blocked_ref, retry_at, reask_at, dismissed_at
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
            wait: Wait::read(&row.get::<_, String>(10)?),
            wait_ref: row.get(11)?,
            retry_at: row.get(12)?,
            reask_at: row.get(13)?,
            dismissed_at: row.get(14)?,
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
            "waits_on" => format!("Waits on: {value}"),
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
    /// What it waits on, typed (#279): its kind, and whom or what (an
    /// agent id, or "agent#12"; already checked). Only on an item that is
    /// (or becomes) blocked.
    pub wait: Option<(Wait, String)>,
    /// When to check again, in ms: an outside wait's next retry, or when a
    /// parked item resumes. Only with waits_on external or status parked.
    pub retry_at: Option<i64>,
    /// An outside wait's first check when no retry_at was given (Settings →
    /// Checklist's retry interval): only for a new wait, since the same one
    /// set again keeps its next check.
    pub first_retry: Option<i64>,
}

/// The blocked_on words a typed wait shows to older apps: "person" for the
/// person, else whom or what it waits on.
fn words_for(wait: Wait, reference: &str, words: Option<&str>) -> String {
    match (wait, words) {
        (Wait::PersonDecision | Wait::PersonAction, _) => "person".to_owned(),
        (_, Some(words)) => words.to_owned(),
        (Wait::Item, None) => match reference.split_once('#') {
            Some((agent, id)) => format!("{agent} #{id}"),
            None => reference.to_owned(),
        },
        (Wait::Agent, None) => reference.to_owned(),
        (Wait::External, None) => "something outside".to_owned(),
        (Wait::Untyped, None) => String::new(),
    }
}

/// Applies changes, each to an existing item; says what changed, item by item.
pub(crate) fn update(store: &Store, agent: &str, actor: &str, updates: &[Update]) -> rusqlite::Result<Vec<String>> {
    let mut db = store.db();
    let transaction = db.transaction()?;
    let now = now_ms();
    let mut said = Vec::new();
    for update in updates {
        let was: Option<(String, String, String)> = transaction
            .query_row(
                "SELECT status, blocked_kind, blocked_ref FROM checklist_items WHERE agent = ?1 AND id = ?2",
                params![agent, update.id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .optional()?;
        let Some((was, was_kind, was_ref)) = was else {
            said.push(format!("#{}: no such item", update.id));
            continue;
        };
        let blocked = update.status.map_or(Status::parse(&was) == Some(Status::Blocked), |status| status == Status::Blocked);
        if update.blocked_on.is_some() && !blocked {
            said.push(format!("#{}: blocked_on goes with status blocked", update.id));
            continue;
        }
        if update.wait.is_some() && !blocked {
            said.push(format!("#{}: waits_on goes with status blocked", update.id));
            continue;
        }
        let parked = update.status.map_or(Status::parse(&was) == Some(Status::Parked), |status| status == Status::Parked);
        let external = match &update.wait {
            Some((wait, _)) => *wait == Wait::External,
            None => blocked && update.blocked_on.is_none() && Wait::read(&was_kind) == Wait::External,
        };
        if update.retry_at.is_some() && !parked && !external {
            said.push(format!("#{}: retry_at goes with waits_on external, or status parked", update.id));
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
                    "UPDATE checklist_items SET status = ?3, blocked_on = '', blocked_since = NULL, blocked_kind = '', blocked_ref = '',
                     retry_at = NULL, retries = 0, reask_at = NULL, dismissed_at = NULL, prompted_at = NULL, overdue_told = NULL
                     WHERE agent = ?1 AND id = ?2",
                    params![agent, update.id, status.name()],
                )?;
            }
            record(&transaction, agent, update.id, now, actor, "status", status.name())?;
            parts.push(status.name().to_owned());
        }
        // The same wait set again (new words, say) keeps where its retries,
        // re-asks and prompts stand; a new one starts them afresh.
        let same = |wait: Wait, reference: &str| {
            Status::parse(&was) == Some(Status::Blocked)
                && update.status.is_none_or(|status| status == Status::Blocked)
                && Wait::read(&was_kind) == wait
                && was_ref == reference
        };
        if let Some((wait, reference)) = &update.wait
            && same(*wait, reference)
        {
            // Its words and next check stay unless new ones are given.
            let on = update.blocked_on.as_deref().map(|words| words_for(*wait, reference, Some(words)));
            transaction.execute(
                "UPDATE checklist_items SET blocked_on = COALESCE(?3, blocked_on), retry_at = COALESCE(?4, retry_at) WHERE agent = ?1 AND id = ?2",
                params![agent, update.id, on, update.retry_at],
            )?;
            let mut shown = wait.name().replace('_', " ");
            if !reference.is_empty() {
                let _ = write!(shown, " {reference}");
            }
            record(&transaction, agent, update.id, now, actor, "waits_on", &shown)?;
            parts.push(format!("waits on {shown}"));
        } else if let Some((wait, reference)) = &update.wait {
            let on = words_for(*wait, reference, update.blocked_on.as_deref());
            transaction.execute(
                "UPDATE checklist_items SET blocked_kind = ?3, blocked_ref = ?4, retry_at = ?5, blocked_on = ?6,
                 retries = 0, reask_at = NULL, dismissed_at = NULL, prompted_at = NULL WHERE agent = ?1 AND id = ?2",
                params![agent, update.id, wait.name(), reference, update.retry_at.or(update.first_retry), on],
            )?;
            let mut shown = wait.name().replace('_', " ");
            if !reference.is_empty() {
                let _ = write!(shown, " {reference}");
            }
            record(&transaction, agent, update.id, now, actor, "waits_on", &shown)?;
            parts.push(format!("waits on {shown}"));
        } else if let Some(on) = &update.blocked_on {
            // Words alone, as before: "person" is a decision of the person's;
            // anything else is untyped.
            let wait = if on == "person" { Wait::PersonDecision } else { Wait::Untyped };
            transaction.execute(
                "UPDATE checklist_items SET blocked_on = ?3, blocked_kind = ?4, blocked_ref = '', retry_at = NULL,
                 retries = 0, reask_at = NULL, dismissed_at = NULL, prompted_at = NULL WHERE agent = ?1 AND id = ?2",
                params![agent, update.id, on, wait.name()],
            )?;
            record(&transaction, agent, update.id, now, actor, "blocked_on", on)?;
            parts.push(format!("blocks on {on}"));
        }
        // A parked item's resume time, or a new retry for an outside wait
        // already set.
        if let Some(at) = update.retry_at
            && (parked || update.wait.is_none())
        {
            transaction.execute("UPDATE checklist_items SET retry_at = ?3 WHERE agent = ?1 AND id = ?2", params![agent, update.id, at])?;
            let when = super::monitors::when::local(at, now);
            let kind = if parked { "resumes" } else { "retry" };
            record(&transaction, agent, update.id, now, actor, kind, &when)?;
            parts.push(if parked { format!("resumes {when}") } else { format!("checked again {when}") });
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

/// "agent#12": how a wait on an item, and an ask about one, name it.
pub(crate) fn item_key(agent: &str, id: u32) -> String {
    format!("{agent}#{id}")
}

/// An item's text, by its key; None when it's gone.
fn text_of(store: &Store, key: &str) -> Option<String> {
    let (agent, id) = key.rsplit_once('#')?;
    let id: u32 = id.parse().ok()?;
    store
        .db()
        .query_row("SELECT text FROM checklist_items WHERE agent = ?1 AND id = ?2", params![agent, id], |row| row.get(0))
        .optional()
        .ok()
        .flatten()
}

/// The migration that added links from cards other than ask_person's to
/// the checklist items they're about (secrets_request's and
/// propose_to_rivet's item).
pub(crate) const CARD_ITEMS: &str = "
-- Cards that name the checklist item they're about, besides ask_person's
-- (kept in asks.item): kind 'secret' (a vault request's id) or 'proposal'
-- (a proposal's id), the agent that asked, and the item, 'agent#12'.
CREATE TABLE card_items (kind TEXT NOT NULL, card TEXT NOT NULL, agent TEXT NOT NULL, item TEXT NOT NULL, PRIMARY KEY (kind, card));
CREATE INDEX card_items_by_item ON card_items (agent, item);
";

/// What the person last dismissed about a blocked item, so asking again
/// uses the same tool: 'ask' (a question), 'secret' (a request for a
/// secret) or 'proposal' (a proposal to Rivet); '' before this was kept,
/// read as a question.
pub(crate) const DISMISSED_CARD: &str = "
ALTER TABLE checklist_items ADD COLUMN dismissed_card TEXT NOT NULL DEFAULT '';
";

/// A card waiting for the person that names a checklist item.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Card {
    /// An ask_person question (a procedure too), by its id.
    Ask(i64),
    /// A request for a secret, by its id.
    Secret(String),
    /// A proposal to Rivet, by its id.
    Proposal(i64),
}

impl Card {
    /// The kind, as card_items and dismissed_card keep it.
    pub(crate) fn kind(&self) -> &'static str {
        match self {
            Card::Ask(_) => "ask",
            Card::Secret(_) => "secret",
            Card::Proposal(_) => "proposal",
        }
    }
}

/// Links `agent`'s card to the item it names ("agent#12").
pub(crate) fn link(store: &Store, card: &Card, agent: &str, item: &str) {
    let id = match card {
        Card::Ask(_) => return,
        Card::Secret(id) => id.clone(),
        Card::Proposal(id) => id.to_string(),
    };
    let kind = card.kind();
    let linked = store.db().execute(
        "INSERT OR REPLACE INTO card_items (kind, card, agent, item) VALUES (?1, ?2, ?3, ?4)",
        params![kind, id, agent, item],
    );
    if let Err(error) = linked {
        tracing::error!(%agent, %item, %error, "could not link a card to its checklist item");
    }
}

/// `agent`'s cards waiting for the person about item `key` ("agent#12"),
/// oldest first by kind: its questions, its requests for secrets still
/// waiting on the person, and its proposals.
pub(crate) fn cards_about(factory: &Factory, agent: &str, key: &str) -> Vec<Card> {
    let mut cards: Vec<Card> = super::ask::pending_ids_about(&factory.store, agent, key).into_iter().map(Card::Ask).collect();
    let linked: Vec<(String, String)> = crate::store::query(
        &factory.store.db(),
        "SELECT kind, card FROM card_items WHERE agent = ?1 AND item = ?2 ORDER BY rowid",
        params![agent, key],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )
    .unwrap_or_default();
    for (kind, id) in linked {
        match kind.as_str() {
            // Waiting on the person: pending, or approved with no admin
            // agent to find it (as `Agent.waiting_seqs` counts it).
            "secret" => {
                let waits = factory.vault.pending_request(agent, &id).is_ok_and(|request| {
                    request.state() == v1::SecretRequestState::Pending
                        || (request.state() == v1::SecretRequestState::Approved && request.admin_agent_id.is_empty())
                });
                if waits {
                    cards.push(Card::Secret(id));
                }
            }
            "proposal" => {
                let Ok(number) = id.parse::<i64>() else { continue };
                let waits = factory
                    .store
                    .db()
                    .query_row(
                        "SELECT EXISTS (SELECT 1 FROM proposals WHERE id = ?1 AND agent = ?2 AND state = 'pending')",
                        params![number, agent],
                        |row| row.get(0),
                    )
                    .unwrap_or(false);
                if waits {
                    cards.push(Card::Proposal(number));
                }
            }
            _ => {}
        }
    }
    cards
}

/// The person decided `card` (a request for a secret or a proposal):
/// what the event telling its agent adds about the item it's linked to,
/// as for a question (`decided`); "" for none, or an item that no longer
/// waits on them. Its checklist and Needs you show it afresh.
pub(crate) fn card_decided(factory: &Factory, card: &Card, dismissed: bool) -> String {
    let id = match card {
        Card::Ask(_) => return String::new(),
        Card::Secret(id) => id.clone(),
        Card::Proposal(id) => id.to_string(),
    };
    let kind = card.kind();
    let item: Option<String> = factory
        .store
        .db()
        .query_row("SELECT item FROM card_items WHERE kind = ?1 AND card = ?2", params![kind, id], |row| row.get(0))
        .optional()
        .unwrap_or_default();
    let Some(item) = item else { return String::new() };
    let said = decided(factory, &item, dismissed, kind).unwrap_or_else(|error| {
        tracing::error!(%item, %error, "could not note a decided card on its item");
        None
    });
    if let Some((whose, _)) = item.rsplit_once('#') {
        factory.refresh_view(whose);
    }
    factory.broadcast_workspace(&factory.lock());
    said.unwrap_or_default()
}

/// Whether `agent` has a card waiting for the person about item `key`.
pub(crate) fn asked_about(factory: &Factory, agent: &str, key: &str) -> bool {
    !cards_about(factory, agent, key).is_empty()
}

/// The seq of `card`'s entry in `transcript`, if it shows it.
pub(crate) fn seq_of(transcript: &crate::transcript::Transcript, card: &Card) -> Option<u64> {
    use v1::entry::Kind;
    transcript.in_memory().rev().find_map(|entry| {
        let shows = match (&entry.kind, card) {
            (Some(Kind::AskPerson(ask)), Card::Ask(id)) => ask.id == id.to_string(),
            (Some(Kind::SecretRequest(request)), Card::Secret(id)) => request.id == *id,
            (Some(Kind::Proposal(proposal)), Card::Proposal(id)) => proposal.id == id.to_string(),
            _ => false,
        };
        shows.then_some(entry.seq)
    })
}

/// The checklist item a card `agent` is about to file is about: the one it
/// names (`given`: 12, or Ana#12), or with none, the one item it just
/// blocked on the person and hasn't asked about (`just_blocked`). Its key,
/// "agent#12", and when it was taken from the block rather than named, how
/// the tool's result names it and the item's text; ("", None) for none.
pub(crate) fn item_for(factory: &Factory, agent: &str, given: Option<&str>) -> Result<(String, Option<(String, String)>), String> {
    if let Some(given) = given.map(str::trim).filter(|given| !given.is_empty()) {
        let key = resolve_item(factory, agent, given)?;
        // Only the agent that acts on an item asks about it: its owner, or
        // with none its checklist's agent, as replies about items go.
        let (whose, id) = key.rsplit_once('#').unwrap_or_default();
        let owner: String = factory
            .store
            .db()
            .query_row("SELECT owner FROM checklist_items WHERE agent = ?1 AND id = ?2", params![whose, id], |row| row.get(0))
            .map_err(failed)?;
        let acting = owning(whose, &owner);
        if acting != agent {
            let (checklist, acts) = (name_of(factory, whose), name_of(factory, acting));
            let whose_it_is = if owner.is_empty() { format!("on {checklist}'s checklist") } else { format!("{acts}'s") };
            return Err(format!(
                "{checklist}#{id} is {whose_it_is} and isn't yours: ask {acts} to ask, or have it assigned to you"
            ));
        }
        return Ok((key, None));
    }
    let Some((whose, id, text)) = just_blocked(factory, agent, now_ms()) else { return Ok((String::new(), None)) };
    let label = if whose == agent {
        format!("#{id}")
    } else {
        format!("{}#{id}", factory.lock().config.agent(&whose).map_or_else(|| whose.clone(), |owner| owner.name.clone()))
    };
    Ok((item_key(&whose, id), Some((label, text))))
}

/// What a tool's result adds when its card was taken to be about the item
/// just blocked: so the agent can tell when that's wrong.
pub(crate) fn taken_for(linked: Option<(String, String)>, person: &str) -> String {
    match linked {
        Some((label, text)) => format!(
            " It's taken to be about {label} \"{text}\", the item you just blocked on {person}, and shows with it in their Needs you; if it's about something else, give item when you ask."
        ),
        None => String::new(),
    }
}

/// How soon after an item is blocked on the person a question that names
/// no item is taken to be about it (`just_blocked`).
const JUST_BLOCKED_MS: i64 = 10 * 60 * 1000;

/// The item a question `agent` asks without naming one is about, when
/// that's certain: the one item blocked on the person, in the past few
/// minutes, that `agent` acts on and hasn't asked about. "agent#12" and its
/// text; None when there's none, or more than one.
pub(crate) fn just_blocked(factory: &Factory, agent: &str, now: i64) -> Option<(String, u32, String)> {
    let rows: Vec<(String, u32, String)> = crate::store::query(
        &factory.store.db(),
        "SELECT agent, id, text FROM checklist_items
         WHERE status = 'blocked' AND blocked_kind IN ('person_decision', 'person_action') AND updated >= ?2
         AND ((agent = ?1 AND owner IN ('', ?1)) OR owner = ?1)",
        params![agent, now - JUST_BLOCKED_MS],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
    )
    .ok()?;
    let mut unasked = rows.into_iter().filter(|(whose, id, _)| !asked_about(factory, agent, &item_key(whose, *id)));
    let only = unasked.next()?;
    unasked.next().is_none().then_some(only)
}

/// The agent that acts on an item: its owner, or the checklist's agent.
fn owning<'a>(agent: &'a str, owner: &'a str) -> &'a str {
    if owner.is_empty() { agent } else { owner }
}

/// The person replies to `receiver` on item `id` of `checklist`'s checklist
/// ("" for the receiver's own): the item as the message names it. Refused
/// for an item that isn't there, or a receiver that's neither the
/// checklist's agent nor the item's owner.
pub(crate) fn reply_target(factory: &Factory, receiver: &str, checklist: &str, id: u32) -> Result<factory_agent::ChecklistItemRef, String> {
    let checklist = if checklist.is_empty() { receiver } else { checklist };
    let found: Option<(String, String)> = factory
        .store
        .db()
        .query_row("SELECT text, owner FROM checklist_items WHERE agent = ?1 AND id = ?2", params![checklist, id], |row| {
            Ok((row.get(0)?, row.get(1)?))
        })
        .optional()
        .map_err(failed)?;
    let name = |agent: &str| factory.lock().config.agent(agent).map_or_else(|| agent.to_owned(), |agent| agent.name.clone());
    let Some((text, owner)) = found else {
        return Err(format!("That item is no longer on {}'s checklist.", name(checklist)));
    };
    if receiver != checklist && receiver != owner {
        return Err(if owner.is_empty() {
            format!("That item is on {0}'s checklist, so reply to {0}.", name(checklist))
        } else {
            format!("That item is {0}'s, so reply to {0}.", name(&owner))
        });
    }
    let label = if receiver == checklist { format!("#{id}") } else { format!("{}#{id}", name(checklist)) };
    Ok(factory_agent::ChecklistItemRef { agent: checklist.to_owned(), id, label, text })
}

/// The person's reply on `item` went: its history keeps it as theirs (an
/// empty actor). Apart from the agent's notes, so a blocked item still
/// shows what it asked.
pub(crate) fn replied(factory: &Factory, item: &factory_agent::ChecklistItemRef, text: &str) {
    let recorded = record(&factory.store.db(), &item.agent, item.id, now_ms(), "", "reply", text);
    if let Err(error) = recorded {
        tracing::warn!(agent = %item.agent, item = item.id, %error, "a reply wasn't kept in the item's history");
    }
    factory.refresh_view(&item.agent);
}

/// Items that wait on the person with a card about them waiting too (a
/// question, a request for a secret or a proposal), by checklist
/// (Agent.waiting_items): one for each such card, with its place,
/// `card_seq` giving the seq of the card's entry in `agent`'s conversation
/// (0 when it isn't loaded). An item with no card about it is the agent's
/// own business, not the person's.
pub(crate) fn waiting_items(
    factory: &Factory,
    card_seq: impl Fn(&str, &Card) -> u64,
) -> rusqlite::Result<std::collections::HashMap<String, Vec<v1::WaitingItem>>> {
    let store = &factory.store;
    /// Agent, id, text, owner, kind, blocked since, re-ask and dismissal times.
    type Waiting = (String, u32, String, String, String, Option<i64>, Option<i64>, Option<i64>);
    let rows: Vec<Waiting> = {
        let db = store.db();
        let mut statement = db.prepare(
            "SELECT agent, id, text, owner, blocked_kind, blocked_since, reask_at, dismissed_at FROM checklist_items
             WHERE status = 'blocked' AND blocked_kind IN ('person_decision', 'person_action') ORDER BY blocked_since, agent, id",
        )?;
        let rows = statement.query_map([], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?, row.get(5)?, row.get(6)?, row.get(7)?))
        })?;
        rows.collect::<Result<_, _>>()?
    };
    let mut waiting: std::collections::HashMap<String, Vec<v1::WaitingItem>> = std::collections::HashMap::new();
    for (agent, id, text, owner, kind, since, reask, dismissed) in rows {
        let asker = owning(&agent, &owner).to_owned();
        for card in cards_about(factory, &asker, &item_key(&agent, id)) {
            waiting.entry(agent.clone()).or_default().push(v1::WaitingItem {
                item: id,
                text: text.clone(),
                kind: Wait::read(&kind).proto().into(),
                since_ms: since.unwrap_or_default(),
                reask_at_ms: reask.unwrap_or_default(),
                has_pending_ask: true,
                owner: owner.clone(),
                dismissed_ms: dismissed.unwrap_or_default(),
                card_agent_id: if asker == agent { String::new() } else { asker.clone() },
                card_seq: card_seq(&asker, &card),
            });
        }
    }
    Ok(waiting)
}

/// Items `agent`'s #`id` was blocking open again now it's done or dropped,
/// each recorded as `actor`'s doing; their owners are told, and their
/// checklists shown afresh.
pub(crate) fn finished(factory: &Arc<Factory>, agent: &str, id: u32, status: Status, actor: &str) -> rusqlite::Result<()> {
    let key = item_key(agent, id);
    let waiting: Vec<(String, u32, String, String)> = {
        let db = factory.store.db();
        let mut statement = db.prepare(
            "SELECT agent, id, text, owner FROM checklist_items WHERE status = 'blocked' AND blocked_kind = 'item' AND blocked_ref = ?1",
        )?;
        let rows = statement.query_map([&key], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)))?;
        rows.collect::<Result<_, _>>()?
    };
    if waiting.is_empty() {
        return Ok(());
    }
    let (text, name) = {
        let inner = factory.lock();
        let name = inner.config.agent(agent).map_or_else(|| agent.to_owned(), |config| config.name.clone());
        (text_of(&factory.store, &key).unwrap_or_default(), name)
    };
    let how = if status == Status::Done { "done" } else { "dropped" };
    for (whose, item, item_text, owner) in waiting {
        let note = format!("{name}'s #{id} is {how}");
        update(&factory.store, &whose, actor, &[Update { id: item, status: Some(Status::Open), note: Some(note), ..Update::default() }])?;
        let on = if whose == owning(&whose, &owner) { String::new() } else { format!(" on {}'s checklist", name_of(factory, &whose)) };
        let said = format!(
            "#{item}{on} \"{item_text}\" was waiting on {name}'s #{id} \"{text}\", which is {how}: it's open again. Carry on with it, or mark it again if it still can't move."
        );
        factory.deliver_event(owning(&whose, &owner), said, crate::factory::host_event(format!("#{item} is open again")));
        factory.refresh_view(&whose);
    }
    factory.broadcast_workspace(&factory.lock());
    Ok(())
}

/// Tells agent `on` that `agent`'s #`id` waits on it: no reply needed.
fn waited_on(factory: &Arc<Factory>, agent: &str, id: u32, on: &str) {
    let Some(text) = text_of(&factory.store, &item_key(agent, id)) else { return };
    let name = factory.lock().config.agent(agent).map_or_else(|| agent.to_owned(), |config| config.name.clone());
    let said = format!(
        "{name} is waiting on you for its #{id} \"{text}\". Nothing to answer if you already know; when what it needs is ready, tell {name}."
    );
    factory.deliver_event(on, said, crate::factory::host_event(format!("{name} is waiting on you")));
}

/// The person decided a question about item `key` ("agent#12"): what the
/// event telling its agent adds, while the item still waits on them; None
/// when it no longer does. Answered, the agent clears the block or says
/// what's missing. Dismissed, it decides itself if it can; if it truly
/// can't, it stays blocked, out of Needs you, and Claktory has it ask again after
/// Settings → Checklist's re-ask interval, as often as it takes, with the
/// same kind of card (`kind`, as `Card::kind`).
pub(crate) fn decided(factory: &Factory, key: &str, dismissed: bool, kind: &str) -> rusqlite::Result<Option<String>> {
    let Some((agent, id)) = key.rsplit_once('#') else { return Ok(None) };
    let Ok(id) = id.parse::<u32>() else { return Ok(None) };
    let row: Option<(String, String)> = factory
        .store
        .db()
        .query_row(
            "SELECT text, blocked_kind FROM checklist_items WHERE agent = ?1 AND id = ?2 AND status = 'blocked'",
            params![agent, id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?;
    let Some((text, wait)) = row else { return Ok(None) };
    if !Wait::read(&wait).is_person() {
        return Ok(None);
    }
    let person = factory.lock().config.person().to_owned();
    if !dismissed {
        // Answered: whatever an earlier dismissal set going is over.
        factory
            .store
            .db()
            .execute("UPDATE checklist_items SET dismissed_at = NULL, reask_at = NULL WHERE agent = ?1 AND id = ?2", params![agent, id])?;
        return Ok(Some(format!(
            "\n\nIt was about #{id} \"{text}\", still blocked on {person}. If this settles it, update #{id} now; if something is still missing, say what in a note."
        )));
    }
    let now = now_ms();
    let reask_at = now + Times::read(&factory.store).reask.saturating_mul(factory.nudge_minute());
    factory.store.db().execute(
        "UPDATE checklist_items SET dismissed_at = ?3, reask_at = ?4, dismissed_card = ?5 WHERE agent = ?1 AND id = ?2",
        params![agent, id, now, reask_at, kind],
    )?;
    let when = super::monitors::when::local(reask_at, now);
    let again = match kind {
        "secret" => format!("ask them again {when}, as a new request for it"),
        "proposal" => format!("propose it again {when}, as a new proposal"),
        _ => format!("ask them again {when}, as a new question"),
    };
    Ok(Some(format!(
        "\n\nIt was about #{id} \"{text}\", which still waits on {person}. Decide it yourself if you can, and update #{id}; note what you chose. If you truly can't move without them, leave #{id} blocked and say why in a note: it's yours to settle meanwhile, out of their Needs you, and Claktory has you {again}."
    )))
}

/// Whether Claktory follows up with `owning` about its blocked items: not
/// when it's archived or gone, or has its Checklist's reminders off.
fn followed_up(factory: &Factory, owning: &str) -> bool {
    let inner = factory.lock();
    let Some(config) = inner.config.agent(owning) else { return false };
    !inner.config.archived(config) && !(super::enabled(config, ID) && !super::switch(config, ID, "nudge"))
}

/// A blocked or parked item as `check_waits` reads it.
struct Due {
    agent: String,
    id: u32,
    text: String,
    owner: String,
    kind: Wait,
    on: String,
    since: i64,
    retries: i64,
    /// What the person last dismissed about it (`DISMISSED_CARD`).
    dismissed: String,
}

impl Due {
    fn owning(&self) -> &str {
        owning(&self.agent, &self.owner)
    }

    /// " on Ana's checklist" when it isn't `owning`'s own.
    fn whose(&self, factory: &Factory) -> String {
        if self.owning() == self.agent { String::new() } else { format!(" on {}'s checklist", name_of(factory, &self.agent)) }
    }
}

/// Typed waits and parked items coming due (#279, #282), checked now and
/// then by the nudge loop, each interval from Settings → Checklist:
/// - a parked item's resume time opens it again, its owner told;
/// - an outside wait's retry time tells its owner to check again, and sets
///   the next, twice as long after;
/// - a dismissed question's re-ask time has the owner ask the person again,
///   and sets the next;
/// - a block on an agent or item, or one waiting on the person, has its
///   owner asked whether it's still blocked, and again as often;
/// - a block older than the overdue interval is told to its owner's lead,
///   once (never to the person).
pub(crate) fn check_waits(factory: &Arc<Factory>) -> rusqlite::Result<()> {
    let now = now_ms();
    let minute = factory.nudge_minute();
    let times = Times::read(&factory.store);
    let after = |minutes: i64| minutes.saturating_mul(minute);
    let due = |condition: &str, at: i64| -> rusqlite::Result<Vec<Due>> {
        let db = factory.store.db();
        let mut statement = db.prepare(&format!(
            "SELECT agent, id, text, owner, blocked_kind, blocked_on, COALESCE(blocked_since, updated), retries, dismissed_card
             FROM checklist_items WHERE {condition} ORDER BY agent, id"
        ))?;
        let rows = statement.query_map([at], |row| {
            Ok(Due {
                agent: row.get(0)?,
                id: row.get(1)?,
                text: row.get(2)?,
                owner: row.get(3)?,
                kind: Wait::read(&row.get::<_, String>(4)?),
                on: row.get(5)?,
                since: row.get(6)?,
                retries: row.get(7)?,
                dismissed: row.get(8)?,
            })
        })?;
        rows.collect()
    };
    let (person, person_capitalized) = {
        let inner = factory.lock();
        (inner.config.person().to_owned(), inner.config.person_capitalized().to_owned())
    };
    let mut changed = false;

    for item in due("status = 'parked' AND retry_at <= ?1", now)? {
        update(
            &factory.store,
            &item.agent,
            &item.agent,
            &[Update { id: item.id, status: Some(Status::Open), note: Some("Resumed at its set time".to_owned()), ..Update::default() }],
        )?;
        let said =
            format!("#{}{} \"{}\" was parked until now: it's open again. Pick it up, or park it again.", item.id, item.whose(factory), item.text);
        factory.deliver_event(item.owning(), said, crate::factory::host_event(format!("#{} resumes", item.id)));
        factory.refresh_view(&item.agent);
        changed = true;
    }

    for item in due("status = 'blocked' AND blocked_kind = 'external' AND retry_at <= ?1", now)? {
        let next = now + after(retry_minutes(times.external_retry, item.retries + 1));
        factory.store.db().execute(
            "UPDATE checklist_items SET retry_at = ?3, retries = retries + 1 WHERE agent = ?1 AND id = ?2",
            params![item.agent, item.id, next],
        )?;
        let said = format!(
            "#{}{} \"{}\" waits on {}, and it's time to check again. If it can move now, carry on and update it; if not, leave it: you're told again {}, or set your own retry_at.",
            item.id,
            item.whose(factory),
            item.text,
            item.on,
            super::monitors::when::local(next, now),
        );
        factory.deliver_event(item.owning(), said, crate::factory::host_event(format!("Check again: #{}", item.id)));
        factory.refresh_view(&item.agent);
    }

    for item in due("status = 'blocked' AND blocked_kind IN ('person_decision', 'person_action') AND reask_at <= ?1", now)? {
        let next = now + after(times.reask);
        factory
            .store
            .db()
            .execute("UPDATE checklist_items SET reask_at = ?3 WHERE agent = ?1 AND id = ?2", params![item.agent, item.id, next])?;
        factory.refresh_view(&item.agent);
        changed = true;
        let owning = item.owning();
        if asked_about(factory, owning, &item_key(&item.agent, item.id)) {
            continue;
        }
        let name = if owning == item.agent { item.id.to_string() } else { format!("{}#{}", name_of(factory, &item.agent), item.id) };
        // Again with what was dismissed: a secret with secrets_request, a
        // proposal with propose_to_rivet; anything else is a question.
        let (id, whose, text) = (item.id, item.whose(factory), &item.text);
        let (said, summary) = match item.dismissed.as_str() {
            "secret" => (
                format!(
                    "{person_capitalized} dismissed your request for a secret for #{id}{whose} \"{text}\", and it still waits on them. If you can do without it now, do, and update #{id}. If not, ask again with secrets_request (item: \"{name}\"), saying in a line why only they can give it."
                ),
                format!("Ask {person} again: #{id}"),
            ),
            "proposal" => (
                format!(
                    "{person_capitalized} dismissed your proposal about #{id}{whose} \"{text}\", and it still waits on them. If you can do without it now, do, and update #{id}. If not, propose again with propose_to_rivet (item: \"{name}\"), saying in a line why it's needed."
                ),
                format!("Propose again: #{id}"),
            ),
            _ => (
                format!(
                    "{person_capitalized} dismissed your question about #{id}{whose} \"{text}\", and it still waits on them. If you can decide it yourself now, do, and update #{id}. If not, ask them again with ask_person (item: \"{name}\"), saying in a line why only they can settle it."
                ),
                format!("Ask {person} again: #{id}"),
            ),
        };
        factory.deliver_event(owning, said, crate::factory::host_event(summary));
    }

    // Still blocked? On agents and items (and untyped) after the agent
    // interval; on the person after the person interval, unless a dismissal
    // has it re-asked or a decision has no card yet (the daily reminder
    // covers that). Each owner hears of its items together.
    let mut still: std::collections::BTreeMap<String, Vec<(Due, i64)>> = std::collections::BTreeMap::new();
    for (kinds, minutes) in [("'', 'item', 'agent'", times.agent_blocked), ("'person_decision', 'person_action'", times.person_blocked)] {
        let condition = format!(
            "status = 'blocked' AND blocked_kind IN ({kinds}) AND dismissed_at IS NULL AND COALESCE(prompted_at, blocked_since, updated) <= ?1"
        );
        for item in due(&condition, now - after(minutes))? {
            let key = item_key(&item.agent, item.id);
            if item.kind.is_person() && !asked_about(factory, item.owning(), &key) {
                continue;
            }
            factory
                .store
                .db()
                .execute("UPDATE checklist_items SET prompted_at = ?3 WHERE agent = ?1 AND id = ?2", params![item.agent, item.id, now])?;
            still.entry(item.owning().to_owned()).or_default().push((item, minutes));
        }
    }
    for (owning, items) in still {
        if !followed_up(factory, &owning) {
            continue;
        }
        let lines: Vec<String> = items
            .iter()
            .map(|(item, _)| {
                let on = if item.on.is_empty() { String::new() } else { format!(", waiting on {}", item.on) };
                let how_long = super::monitors::when::duration(now - item.since);
                format!("#{}{} \"{}\"{on}, blocked {how_long}", item.id, item.whose(factory), item.text)
            })
            .collect();
        let person_waits = items.iter().any(|(item, _)| item.kind.is_person());
        let mut said = format!(
            "Still blocked? {}. It may not be any more: try it again. If it can move, carry on and update it; if it's set aside for later or ongoing rather than blocked, mark it parked.",
            lines.join("; ")
        );
        if person_waits {
            let _ = write!(
                said,
                " One waiting on {person} shows in their Needs you only once you ask: ask_person with item, one question."
            );
        }
        let count = items.len();
        let summary = format!("Still blocked? {count} item{}", if count == 1 { "" } else { "s" });
        factory.deliver_event(&owning, said, crate::factory::host_event(summary));
    }

    for item in due("status = 'blocked' AND blocked_since <= ?1 AND overdue_told IS NULL", now - after(times.overdue))? {
        factory
            .store
            .db()
            .execute("UPDATE checklist_items SET overdue_told = ?3 WHERE agent = ?1 AND id = ?2", params![item.agent, item.id, now])?;
        let (lead, name) = {
            let inner = factory.lock();
            let Some(config) = inner.config.agent(item.owning()) else { continue };
            (inner.config.lead(config).map(|lead| lead.id.clone()), config.name.clone())
        };
        let Some(lead) = lead else { continue };
        let on = if item.on.is_empty() { String::new() } else { format!(", waiting on {}", item.on) };
        let how_long = super::monitors::when::duration(now - item.since);
        let whose =
            if item.owning() == item.agent { String::new() } else { format!(" (on {}'s checklist)", name_of(factory, &item.agent)) };
        let said = format!(
            "{name}'s #{}{whose} \"{}\" has been blocked for {how_long}{on}. See whether you can unblock it, or settle with {name} what happens next.",
            item.id, item.text,
        );
        factory.deliver_event(&lead, said, crate::factory::host_event(format!("Overdue: {name}'s #{}", item.id)));
    }
    if changed {
        factory.broadcast_workspace(&factory.lock());
    }
    Ok(())
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
    if item.status == Status::Blocked {
        match (item.wait, item.wait_ref.as_str()) {
            (Wait::Untyped, _) if item.blocked_on.is_empty() => {}
            (Wait::Untyped, _) => {
                let _ = write!(line, " [on {}]", item.blocked_on);
            }
            (wait, "") => {
                let _ = write!(line, " [waits on {}]", wait.name().replace('_', " "));
            }
            (wait, reference) => {
                let _ = write!(line, " [waits on {} {reference}]", wait.name().replace('_', " "));
            }
        }
    }
    if let Some(at) = item.retry_at {
        let when = super::monitors::when::local(at, now_ms());
        match item.status {
            Status::Parked => {
                let _ = write!(line, " [resumes {when}]");
            }
            Status::Blocked => {
                let _ = write!(line, " [checked again {when}]");
            }
            _ => {}
        }
    }
    let _ = write!(line, " {}", item.day);
    line
}

impl Extension for Checklist {
    fn manifest(&self) -> &'static Manifest {
        &MANIFEST
    }

    fn prompt(&self, _factory: &Factory, _inner: &Inner, _agent: &AgentConfig) -> Vec<PromptSection> {
        vec![PromptSection::static_text(PROMPT)]
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
        let status = json!({"type": "string", "enum": ["open", "in_progress", "blocked", "parked", "done", "dropped"]});
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
                    "waits_on": {"type": "string", "enum": ["person_decision", "person_action", "item", "agent", "external"]},
                    "ref": {"type": "string"},
                    "retry_at": {"type": "string"},
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
                "status": {"type": "string", "enum": ["open", "in_progress", "blocked", "parked", "done", "dropped", "all"]},
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
            super::tool("checklist_update", "Update checklist", UPDATE, update, super::typed_handler(factory, agent, run_update)),
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
        let mut shown = Vec::new();
        for item in &items {
            // A card waiting about it: one its owning agent filed naming it.
            let has_pending_ask = (item.blocked_on == "person" || item.wait.is_person())
                && asked_about(factory, owning(agent, &item.owner), &item_key(agent, item.id));
            // An item waited on reads by its text, as it is now.
            let blocked_on = match item.wait {
                Wait::Item => text_of(&factory.store, &item.wait_ref).unwrap_or_else(|| item.blocked_on.clone()),
                _ => item.blocked_on.clone(),
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
                blocked_on,
                blocked_since_ms: item.blocked_since.unwrap_or_default(),
                has_pending_ask,
                goal: item.goal,
                blocked_kind: item.wait.proto().into(),
                blocked_ref: item.wait_ref.clone(),
                retry_at_ms: item.retry_at.unwrap_or_default(),
                reask_at_ms: item.reask_at.unwrap_or_default(),
                dismissed_ms: item.dismissed_at.unwrap_or_default(),
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
    waits_on: Option<String>,
    #[serde(default, rename = "ref")]
    reference: Option<String>,
    #[serde(default)]
    retry_at: Option<String>,
    #[serde(default)]
    goal: Option<u32>,
    #[serde(default)]
    owner: Option<String>,
    #[serde(default)]
    text: Option<String>,
    #[serde(default)]
    note: Option<String>,
}

/// How texts name `agent`: its name, or its id once it's gone.
pub(crate) fn name_of(factory: &Factory, agent: &str) -> String {
    factory.lock().config.agent(agent).map_or_else(|| agent.to_owned(), |config| config.name.clone())
}

/// The item `reference` names, as its key ("agent#12"): "12" or "#12" on
/// `agent`'s own checklist, "Ana#12" on Ana's (an id or name).
pub(crate) fn resolve_item(factory: &Factory, agent: &str, reference: &str) -> Result<String, String> {
    let (whose, id) = reference.trim().rsplit_once('#').unwrap_or(("", reference.trim()));
    let id: u32 =
        id.trim().parse().map_err(|_| format!("\"{reference}\" isn't an item: write 12 for #12, or Ana#12 on Ana's checklist"))?;
    let whose = if whose.trim().is_empty() { agent.to_owned() } else { agent_id(factory, agent, whose)? };
    if !has_item(&factory.store, &whose, id).map_err(failed)? {
        return Err(format!("there's no #{id} on {}'s checklist", name_of(factory, &whose)));
    }
    Ok(item_key(&whose, id))
}

/// A typed wait as the agent gave it, checked: its kind and whom or what
/// (an agent id, or "agent#12"); and when to check again (retry_at: an
/// outside wait's, or a parked item's resume time).
type Given = (Option<(Wait, String)>, Option<i64>);

fn typed_wait(factory: &Factory, agent: &str, item: u32, arg: &UpdateArg) -> Result<Given, String> {
    let retry_at = match arg.retry_at.as_deref().map(str::trim).filter(|at| !at.is_empty()) {
        None => None,
        Some(at) => {
            let now = now_ms();
            let at = super::monitors::when::parse_duration(at)
                .map(|wait| now + wait)
                .or_else(|_| super::monitors::when::parse_time(at, now))
                .map_err(|_| format!("#{item}: retry_at \"{at}\" isn't a time: write e.g. 2h, 1d or \"2026-11-23 14:00\""))?;
            Some(at)
        }
    };
    let Some(kind) = arg.waits_on.as_deref().map(str::trim).filter(|kind| !kind.is_empty()) else {
        if arg.reference.is_some() {
            return Err(format!("#{item}: ref goes with waits_on item or agent"));
        }
        return Ok((None, retry_at));
    };
    let wait =
        Wait::parse(kind).ok_or_else(|| format!("#{item}: waits_on must be person_decision, person_action, item, agent or external"))?;
    let reference = arg.reference.as_deref().map(str::trim).filter(|reference| !reference.is_empty());
    let reference = match (wait, reference) {
        (Wait::Item, None) => {
            return Err(format!("#{item}: waits_on item needs ref, the item it waits on: 12 for #12, or Ana#12 on Ana's checklist"));
        }
        (Wait::Item, Some(reference)) => {
            let key = resolve_item(factory, agent, reference).map_err(|problem| format!("#{item}: {problem}"))?;
            waitable(factory, &item_key(agent, item), &key).map_err(|problem| format!("#{item}: {problem}"))?;
            key
        }
        (Wait::Agent, None) => return Err(format!("#{item}: waits_on agent needs ref, the agent's id or name")),
        (Wait::Agent, Some(reference)) => agent_id(factory, agent, reference).map_err(|problem| format!("#{item}: {problem}"))?,
        (_, Some(_)) => return Err(format!("#{item}: ref goes with waits_on item or agent")),
        (_, None) => String::new(),
    };
    Ok((Some((wait, reference)), retry_at))
}

/// Whether item `from` can wait on item `on` (both "agent#12"): not on
/// itself, not on one that's already done or dropped, and not in a loop
/// of items each waiting on the next.
fn waitable(factory: &Factory, from: &str, on: &str) -> Result<(), String> {
    let store = &factory.store;
    if on == from {
        return Err("an item can't wait on itself".to_owned());
    }
    let read = |key: &str| -> Option<(String, String, String)> {
        let (agent, id) = key.rsplit_once('#')?;
        let id: u32 = id.parse().ok()?;
        store
            .db()
            .query_row(
                "SELECT status, blocked_kind, blocked_ref FROM checklist_items WHERE agent = ?1 AND id = ?2",
                params![agent, id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .optional()
            .ok()
            .flatten()
    };
    // Named as people read it: "Ana #12".
    let shown = match on.rsplit_once('#') {
        Some((agent, id)) => format!("{} #{id}", name_of(factory, agent)),
        None => on.to_owned(),
    };
    if let Some((status, _, _)) = read(on)
        && let Some(status @ (Status::Done | Status::Dropped)) = Status::parse(&status)
    {
        return Err(format!("{shown} is already {}: there's nothing to wait on", status.name()));
    }
    // A short walk: a loop is rare, and a long chain is cut off.
    let mut next = on.to_owned();
    for _ in 0..50 {
        let Some((status, kind, reference)) = read(&next) else { break };
        if status != Status::Blocked.name() || Wait::read(&kind) != Wait::Item {
            break;
        }
        if reference == from {
            return Err(format!("{shown} already waits, through items, on this one: they'd wait on each other forever"));
        }
        next = reference;
    }
    Ok(())
}

fn run_update(factory: &Arc<Factory>, agent: &str, args: serde_json::Value) -> Result<ToolResult, String> {
    let args: UpdateArgs = serde_json::from_value(args).map_err(|error| error.to_string())?;
    let person = factory.lock().config.person().to_ascii_lowercase();
    let mut updates = Vec::new();
    for arg in args.updates {
        let status = match arg.status.as_deref() {
            Some(name) => Some(
                Status::parse(name)
                    .ok_or_else(|| format!("#{}: status must be open, in_progress, blocked, parked, done or dropped", arg.id))?,
            ),
            None => None,
        };
        let owner = match arg.owner.as_deref().map(str::trim) {
            Some("") => Some(String::new()),
            Some(owner) => Some(agent_id(factory, agent, owner).map_err(|problem| format!("#{}: {problem}", arg.id))?),
            None => None,
        };
        let (wait, retry_at) = typed_wait(factory, agent, arg.id, &arg)?;
        let text = arg.text.map(|text| text.trim().to_owned()).filter(|text| !text.is_empty());
        let note = arg.note.map(|note| note.trim().to_owned()).filter(|note| !note.is_empty());
        // The person however named ("Person", their name) keys the daily
        // reminder, so it's kept as the one word.
        let blocked_on = arg.blocked_on.map(|on| on.trim().to_owned()).filter(|on| !on.is_empty()).map(|on| {
            let lower = on.to_ascii_lowercase();
            if lower == "person" || lower == "the person" || lower == person { "person".to_owned() } else { on }
        });
        // An outside wait is always checked again: by default after
        // Settings → Checklist's retry interval.
        let first_retry = match (&wait, retry_at) {
            (Some((Wait::External, _)), None) => {
                Some(now_ms() + Times::read(&factory.store).external_retry.saturating_mul(factory.nudge_minute()))
            }
            _ => None,
        };
        updates.push(Update { id: arg.id, status, owner, text, note, blocked_on, goal: arg.goal, wait, retry_at, first_retry });
    }
    // Agents newly waited on: not those this item already waited on.
    let newly: Vec<(u32, String)> = updates
        .iter()
        .filter_map(|update| match &update.wait {
            Some((Wait::Agent, on)) if on != agent => Some((update.id, on.clone())),
            _ => None,
        })
        .filter(|(id, on)| {
            let before: Option<(String, String, String)> = factory
                .store
                .db()
                .query_row(
                    "SELECT status, blocked_kind, blocked_ref FROM checklist_items WHERE agent = ?1 AND id = ?2",
                    params![agent, id],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                )
                .optional()
                .ok()
                .flatten();
            !before.is_some_and(|(status, kind, reference)| {
                status == Status::Blocked.name() && Wait::read(&kind) == Wait::Agent && reference == *on
            })
        })
        .collect();
    let said = update(&factory.store, agent, agent, &updates).map_err(failed)?;
    factory.refresh_view(agent);
    // Items waiting on one that's now done or dropped open again.
    for update in &updates {
        if let Some(status @ (Status::Done | Status::Dropped)) = update.status {
            finished(factory, agent, update.id, status, agent).map_err(failed)?;
        }
    }
    // An agent waited on is told, once, so a wait never sits on someone
    // who doesn't know.
    for (id, on) in &newly {
        waited_on(factory, agent, *id, on);
    }
    // What waits on the person shows in Needs you (Agent.waiting_items).
    if updates.iter().any(|update| update.status.is_some() || update.wait.is_some() || update.blocked_on.is_some() || update.text.is_some())
    {
        factory.broadcast_workspace(&factory.lock());
    }
    // Each item as it is now, with what changed.
    let (now, _) = query(&factory.store, &Query { agent, ..Query::default() }).map_err(failed)?;
    let items: Vec<_> = updates
        .iter()
        .map(|update| {
            let changed: Vec<&str> = [
                (update.status.is_some(), "status"),
                (update.blocked_on.is_some(), "blocked_on"),
                (update.wait.is_some(), "waits_on"),
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
    // Blocked on the person with no question naming it: nothing shows in
    // their Needs you until one does, so the agent is told how.
    let person = factory.lock().config.person().to_owned();
    let mut text = format!("{}.", said.join("; "));
    for update in updates.iter().filter(|update| update.status.is_some() || update.wait.is_some() || update.blocked_on.is_some()) {
        let Some(item) = now.iter().find(|item| item.id == update.id && item.status == Status::Blocked && item.wait.is_person()) else {
            continue;
        };
        let asker = owning(agent, &item.owner);
        if asked_about(factory, asker, &item_key(agent, item.id)) {
            continue;
        }
        let _ = if asker == agent {
            write!(
                text,
                " #{0} doesn't show in {person}'s Needs you until you ask: ask_person with item {0}, one question (or secrets_request or propose_to_rivet with item {0}, if it waits on a secret or on Rivet).",
                item.id
            )
        } else {
            let name = factory.lock().config.agent(asker).map_or_else(|| asker.to_owned(), |owner| owner.name.clone());
            let mine = factory.lock().config.agent(agent).map_or_else(|| agent.to_owned(), |me| me.name.clone());
            write!(
                text,
                " #{0} doesn't show in {person}'s Needs you until {name}, its owner, asks: ask_person with item {mine}#{0}, one question (or secrets_request or propose_to_rivet with that item, if it waits on a secret or on Rivet).",
                item.id
            )
        };
    }
    // Done or dropped while cards about it still wait for the person: they
    // may no longer apply, which only the agent can tell, so it's told
    // which, and nothing is closed for it (friend bug #19).
    for update in &updates {
        let Some(status @ (Status::Done | Status::Dropped)) = update.status else { continue };
        let Some(item) = now.iter().find(|item| item.id == update.id && item.status == status) else { continue };
        let key = item_key(agent, item.id);
        let owner = owning(agent, &item.owner);
        for asker in std::iter::once(agent).chain((owner != agent).then_some(owner)) {
            let cards = cards_about(factory, asker, &key);
            if cards.is_empty() {
                continue;
            }
            let (named, tools) = waiting_cards(&cards);
            let (wait, they, them, apply) = if cards.len() == 1 { ("waits", "it", "it", "applies") } else { ("wait", "they", "them", "apply") };
            let _ = if asker == agent {
                write!(
                    text,
                    " #{} is {}, yet your {named} about it still {wait} for {person}: if {they} no longer {apply}, take {them} back with {tools}.",
                    item.id,
                    status.name()
                )
            } else {
                let name = factory.lock().config.agent(asker).map_or_else(|| asker.to_owned(), |owner| owner.name.clone());
                write!(
                    text,
                    " #{} is {}, yet {name}'s {named} about it still {wait} for {person}: tell {name}, so it takes {them} back if {they} no longer {apply}.",
                    item.id,
                    status.name()
                )
            };
        }
    }
    Ok(ToolResult::text(text, json!({"agent": agent, "items": items})))
}

/// Cards waiting for the person, as the agent reads them ("question 12 and
/// proposal 3"), and the tools that take them back.
fn waiting_cards(cards: &[Card]) -> (String, String) {
    let named: Vec<String> = cards
        .iter()
        .map(|card| match card {
            Card::Ask(id) => format!("question {id}"),
            Card::Secret(id) => format!("request for a secret {id}"),
            Card::Proposal(id) => format!("proposal {id}"),
        })
        .collect();
    let mut tools: Vec<&str> = Vec::new();
    for card in cards {
        let tool = match card {
            Card::Ask(_) => "ask_withdraw",
            Card::Secret(_) => "secrets_request_cancel",
            Card::Proposal(_) => "proposal_withdraw",
        };
        if !tools.contains(&tool) {
            tools.push(tool);
        }
    }
    (listed(&named, "and"), listed(&tools, "or"))
}

/// "a", "a and b", "a, b and c" (with `and`, or another word).
fn listed(parts: &[impl AsRef<str>], and: &str) -> String {
    let parts: Vec<&str> = parts.iter().map(AsRef::as_ref).collect();
    match parts.as_slice() {
        [] => String::new(),
        [one] => (*one).to_owned(),
        [rest @ .., last] => format!("{} {and} {last}", rest.join(", ")),
    }
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
        Some(name) => Some(vec![Status::parse(name).ok_or("status must be open, in_progress, blocked, parked, done, dropped or all")?]),
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
        return Err(format!("No item #{} on {}'s checklist.", args.id, name_of(factory, &whose)));
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
    let person = factory.person_capitalized();
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
    let person = factory.person_capitalized();
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

    #[test]
    fn an_outside_wait_is_retried_twice_as_long_after_each_time_a_day_at_most() {
        let minutes: Vec<i64> = (0..6).map(|retries| retry_minutes(120, retries)).collect();
        assert_eq!(minutes, [120, 240, 480, 960, 1440, 1440]);
        assert_eq!(retry_minutes(2000, 3), 2000, "a base over a day stays as it is");
    }

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
