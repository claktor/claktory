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

static MANIFEST: Manifest = Manifest {
    id: ID,
    name: "Checklist",
    icon: "list-checks",
    description: "Keeps a checklist of everything you ask for, so nothing gets forgotten, even days later.",
    default_enabled: false,
    options: &[OptionSpec {
        id: "nudge",
        label: "Remind it of unfinished items",
        description: "If it stops with items still open and nothing will wake it, Claktory reminds it once.",
        kind: OptionKind::Switch,
        default_on: true,
        includes_project: false,
    }],
};

const PROMPT: &str = "## Checklist

You keep a checklist of everything the person asks you for, so nothing they said is forgotten across days and compactions. The person sees it beside your conversation.
- When they ask for things, record each as an item right away. checklist_add takes many at once.
- Keep items current with checklist_update: in_progress or an owner when work starts, done or dropped with a short note when it ends.
- in_progress means someone is working on it right now; work waiting its turn stays open, with its owner. Hand each worker one or two items at a time, and the next when one is done.
- blocked means it waits on the person or something outside; say on what in a note. If you stop with open items and nothing running will wake you, Claktory reminds you.
- checklist_list shows open items; filter by status, day, owner or text. checklist_item shows one item's history. Both take `agent` to read another agent's checklist.
- Check the list before telling the person work is finished.
";

const ADD: &str = "Add items to your checklist, many at once. Returns their ids.";
const UPDATE: &str = "Change items on your checklist: status (open, in_progress, blocked, done, dropped), owner (an agent id or name; empty to unassign), text, and/or a short note.";
const LIST: &str = "List checklist items, one line each, oldest first: open, in progress and blocked unless `status` says otherwise (open, in_progress, blocked, done, dropped, all). Filter by `day` added (YYYY-MM-DD), `owner` or text `q`. `agent` reads another agent's checklist.";
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
        "SELECT id, text, status, owner, created, updated, strftime('%m-%d', created / 1000, 'unixepoch', 'localtime')
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

/// Adds items; returns their ids.
pub(crate) fn add(store: &Store, agent: &str, actor: &str, texts: &[String], owner: &str) -> rusqlite::Result<Vec<u32>> {
    let mut db = store.db();
    let transaction = db.transaction()?;
    let last: u32 =
        transaction.query_row("SELECT COALESCE(MAX(id), 0) FROM checklist_items WHERE agent = ?1", [agent], |row| row.get(0))?;
    let now = now_ms();
    let mut ids = Vec::new();
    for (offset, text) in texts.iter().enumerate() {
        let id = last + 1 + offset as u32;
        transaction.execute(
            "INSERT INTO checklist_items (agent, id, text, status, owner, created, updated) VALUES (?1, ?2, ?3, 'open', ?4, ?5, ?5)",
            params![agent, id, text, owner, now],
        )?;
        record(&transaction, agent, id, now, actor, "created", text)?;
        if !owner.is_empty() {
            record(&transaction, agent, id, now, actor, "owner", owner)?;
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
}

/// Applies changes, each to an existing item; says what changed, item by item.
pub(crate) fn update(store: &Store, agent: &str, actor: &str, updates: &[Update]) -> rusqlite::Result<Vec<String>> {
    let mut db = store.db();
    let transaction = db.transaction()?;
    let now = now_ms();
    let mut said = Vec::new();
    for update in updates {
        let exists: Option<u32> = transaction
            .query_row("SELECT id FROM checklist_items WHERE agent = ?1 AND id = ?2", params![agent, update.id], |row| row.get(0))
            .optional()?;
        if exists.is_none() {
            said.push(format!("#{}: no such item", update.id));
            continue;
        }
        let mut parts = Vec::new();
        if let Some(status) = update.status {
            transaction
                .execute("UPDATE checklist_items SET status = ?3 WHERE agent = ?1 AND id = ?2", params![agent, update.id, status.name()])?;
            record(&transaction, agent, update.id, now, actor, "status", status.name())?;
            parts.push(status.name().to_owned());
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
    Ok(())
}

/// "#12 [ ] Fix login (frontend) 11-18"
fn line(item: &Item) -> String {
    let mut line = format!("#{} {} {}", item.id, item.status.mark(), item.text);
    if !item.owner.is_empty() {
        let _ = write!(line, " ({})", item.owner);
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
            "properties": {"items": {"type": "array", "items": text(), "minItems": 1}, "owner": {"type": "string"}},
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
        vec![
            factory_tool(factory, agent, "checklist_add", "Add to checklist", ADD, add, run_add),
            factory_tool(factory, agent, "checklist_update", "Update checklist", UPDATE, update, run_update),
            factory_tool(factory, agent, "checklist_list", "List checklist", LIST, list, run_list),
            factory_tool(factory, agent, "checklist_item", "Checklist item", ITEM, item, run_item),
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
        for item in items {
            let mut log = Vec::new();
            for change in item.log {
                log.push(v1::ChecklistUpdate { time: change.time, text: change.text, author: change.actor });
            }
            shown.push(v1::ChecklistItem {
                id: item.id,
                text: item.text,
                status: item.status.proto().into(),
                owner: item.owner,
                created: item.created,
                updated: item.updated,
                log,
                note: item.blocked_note,
            });
        }
        Some(v1::ExtensionView {
            extension_id: ID.to_owned(),
            agent_id: agent.to_owned(),
            title: MANIFEST.name.to_owned(),
            body: Some(v1::extension_view::Body::Checklist(v1::Checklist { items: shown })),
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
    let Some(me) = config.agent(me) else { return Err("You're no longer in this Claktory.".to_owned()) };
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
    let ids = add(&factory.store, agent, agent, &texts, &owner).map_err(failed)?;
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
    owner: Option<String>,
    #[serde(default)]
    text: Option<String>,
    #[serde(default)]
    note: Option<String>,
}

fn run_update(factory: &Factory, agent: &str, args: serde_json::Value) -> Result<ToolResult, String> {
    let args: UpdateArgs = serde_json::from_value(args).map_err(|error| error.to_string())?;
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
        updates.push(Update { id: arg.id, status, owner, text, note });
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
    let (items, total) = query(&factory.store, &request).map_err(failed)?;
    let details = json!({"agent": whose, "shown": items.len(), "total": total});
    if items.is_empty() {
        return Ok(ToolResult::text("No items match.", details));
    }
    let mut lines: Vec<String> = items.iter().map(line).collect();
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

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> Store {
        let dir = std::env::temp_dir().join(format!("factoryd-checklist-{}-{}", std::process::id(), now_ms()));
        std::fs::create_dir_all(&dir).expect("dir");
        Store::open(&dir.join("factory.db")).expect("store")
    }

    #[test]
    fn items_are_added_updated_and_listed_with_their_history() {
        let store = store();
        let texts = ["Fix login".to_owned(), "Dark mode".to_owned(), "Ship release notes".to_owned()];
        assert_eq!(add(&store, "coordinator", "coordinator", &texts, "").expect("add"), [1, 2, 3]);
        assert_eq!(add(&store, "frontend", "frontend", &["Its own".to_owned()], "").expect("add"), [1], "ids are per agent");
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
    fn statuses_read_loosely() {
        assert_eq!(Status::parse("In progress"), Some(Status::InProgress));
        assert_eq!(Status::parse("in-progress"), Some(Status::InProgress));
        assert_eq!(Status::parse("DONE"), Some(Status::Done));
        assert_eq!(Status::parse("finished"), None);
    }
}
