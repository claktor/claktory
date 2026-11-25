//! Reports (pipes-automation.md §2, H1): a pipe's liaison submits a finished
//! report in the fields of one of the report types the pipe forwards
//! (`types`, §11), one way, to the one agent the person chose to read them
//! (the pipe's forward, its recipient a report reader).
//!
//! This is the store, in factory.db (`pipe_handoff_*`, which the Database
//! extension's SQL never reaches): each pipe's forward and each project's
//! default, the reports with where they came from (the liaison, what it had
//! read, the pipe's messages it came after), the per-pipe aliases for the
//! reader's issues, and every change of where reports go, for Needs you's
//! history. No agent reads a report but through `handoff_read`, which marks
//! it as having read the pipe; the app reads them through ListPipeReports,
//! which marks no one.
//!
//! A report goes only to the recipient of the forward revision it was taken
//! under, while that agent still reads reports: otherwise it waits for the
//! person (DecidePipeReports), never redirected.

pub(crate) mod attachments;
pub(crate) mod schema;
pub(crate) mod tools;
pub(crate) mod types;
pub(crate) mod wake;

use std::collections::BTreeMap;
use std::sync::Arc;

use factory_protocol::v1;
use rusqlite::{OptionalExtension, params};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use super::{Record, Taint};
use crate::factory::{Factory, RequestError};
use crate::transcript::now_ms;

pub(crate) const SCHEMA: &str = "
-- Where a pipe's reports go ('pipe', its id), or a project's default for its
-- new pipes ('project', its id): the person's or an admin agent's choice.
CREATE TABLE pipe_handoff_forwards (
    kind TEXT NOT NULL,
    target TEXT NOT NULL,
    -- The agent that reads them; '' for no one.
    recipient TEXT NOT NULL,
    schema TEXT NOT NULL,
    -- Rises with each change of a pipe's.
    revision INTEGER NOT NULL,
    -- '' for the person, else the admin agent's id.
    set_by TEXT NOT NULL,
    set_ms INTEGER NOT NULL,
    -- JSON: ReportLimits, 0 for the default.
    limits TEXT NOT NULL,
    PRIMARY KEY (kind, target)
);
-- Reports a liaison submitted, and those refused (with no values).
CREATE TABLE pipe_handoff_reports (
    id INTEGER PRIMARY KEY,
    pipe TEXT NOT NULL,
    -- Its number in the pipe; 0 for a refusal.
    number INTEGER NOT NULL,
    liaison TEXT NOT NULL,
    schema TEXT NOT NULL,
    -- The forward revision and recipient it was taken under.
    revision INTEGER NOT NULL,
    recipient TEXT NOT NULL,
    -- JSON object, as checked; '{}' for a refusal.
    fields TEXT NOT NULL,
    text_bytes INTEGER NOT NULL,
    -- The liaison's idempotency token; '' for none.
    token TEXT NOT NULL,
    -- The pipe's last message when it was submitted.
    after_seq INTEGER NOT NULL,
    -- JSON: what the liaison had read then (pipes::Taint), its pipe included.
    taint TEXT NOT NULL,
    received INTEGER NOT NULL,
    -- new, accepted, done, declined, known, fixed, wont_fix, needs_info, refused, waiting, discarded
    state TEXT NOT NULL,
    alias TEXT NOT NULL DEFAULT '',
    -- The reader's own key for the issue: never shown to the pipe's side.
    issue TEXT NOT NULL DEFAULT '',
    fixed_in TEXT NOT NULL DEFAULT '',
    needs TEXT NOT NULL DEFAULT '',
    refused_code TEXT NOT NULL DEFAULT '',
    refused_field TEXT NOT NULL DEFAULT '',
    -- When its recipient was woken for it, and first read its words.
    delivered INTEGER,
    read INTEGER
);
CREATE INDEX pipe_handoff_reports_by_pipe ON pipe_handoff_reports (pipe, number);
CREATE INDEX pipe_handoff_reports_by_recipient ON pipe_handoff_reports (recipient, state);
CREATE INDEX pipe_handoff_reports_by_time ON pipe_handoff_reports (received);
-- Each pipe's own name for a reader's issue ('A3'), made by Claktory.
CREATE TABLE pipe_handoff_aliases (
    pipe TEXT NOT NULL,
    issue TEXT NOT NULL,
    alias TEXT NOT NULL,
    PRIMARY KEY (pipe, issue)
);
-- Every change of where reports go, for Needs you's history.
CREATE TABLE pipe_handoff_changes (
    id INTEGER PRIMARY KEY,
    -- pipe, project or reader
    kind TEXT NOT NULL,
    -- The pipe's, the project's or the agent's id.
    target TEXT NOT NULL,
    -- The recipient before and after (pipe, project); the agent (reader).
    old TEXT NOT NULL,
    new TEXT NOT NULL,
    -- '' for the person, else the admin agent's id.
    by TEXT NOT NULL,
    at INTEGER NOT NULL,
    -- reader: 1 when its conversation was kept (not started fresh).
    kept INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX pipe_handoff_changes_by_time ON pipe_handoff_changes (at);
-- A report's files, copied into <home>/pipes/reports/<pipe>/ under the
-- names Claktory made (`file`, their SHA-256); `name` is as shown, cleaned.
CREATE TABLE pipe_handoff_files (
    report INTEGER NOT NULL,
    idx INTEGER NOT NULL,
    pipe TEXT NOT NULL,
    name TEXT NOT NULL,
    file TEXT NOT NULL,
    size INTEGER NOT NULL,
    -- Its image type, or ''.
    image TEXT NOT NULL,
    received INTEGER NOT NULL,
    PRIMARY KEY (report, idx)
);
CREATE INDEX pipe_handoff_files_by_pipe ON pipe_handoff_files (pipe, received);
";

/// How long reports are kept: the reader's database can be rebuilt from them.
const KEEP_MS: i64 = 90 * 24 * 3_600_000;
const HOUR_MS: i64 = 3_600_000;
const DAY_MS: i64 = 24 * HOUR_MS;
/// The most refusals kept for a pipe in a day; past it they're only refused.
const MAX_REFUSALS_A_DAY: i64 = 100;

/// How much a pipe may forward in the last 24 hours (ReportLimits): 0 is
/// the default.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct Limits {
    pub per_hour: u32,
    pub per_day: u32,
    pub text_kib_per_day: u32,
    pub attachments_per_report: u32,
    pub attachment_mb: u32,
    pub attachments_mb_per_day: u32,
}

impl Limits {
    /// Each one in effect.
    pub(crate) fn effective(self) -> Limits {
        let or = |value: u32, default: u32| if value == 0 { default } else { value };
        Limits {
            per_hour: or(self.per_hour, 6),
            per_day: or(self.per_day, 30),
            text_kib_per_day: or(self.text_kib_per_day, 64),
            attachments_per_report: or(self.attachments_per_report, 3),
            attachment_mb: or(self.attachment_mb, 2),
            attachments_mb_per_day: or(self.attachments_mb_per_day, 20),
        }
    }

    fn from_proto(limits: &v1::ReportLimits) -> Limits {
        Limits {
            per_hour: limits.per_hour,
            per_day: limits.per_day,
            text_kib_per_day: limits.text_kib_per_day,
            attachments_per_report: limits.attachments_per_report,
            attachment_mb: limits.attachment_mb,
            attachments_mb_per_day: limits.attachments_mb_per_day,
        }
    }

    fn to_proto(self) -> v1::ReportLimits {
        let limits = self.effective();
        v1::ReportLimits {
            per_hour: limits.per_hour,
            per_day: limits.per_day,
            text_kib_per_day: limits.text_kib_per_day,
            attachments_per_report: limits.attachments_per_report,
            attachment_mb: limits.attachment_mb,
            attachments_mb_per_day: limits.attachments_mb_per_day,
        }
    }
}

/// Where reports go, as kept.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Forward {
    pub recipient: String,
    /// The report types its liaison may forward, in the order chosen.
    pub types: Vec<String>,
    pub revision: u32,
    pub set_by: String,
    pub set_ms: i64,
    pub limits: Limits,
}

impl Forward {
    /// What a pipe or project with none kept forwards, as the app shows
    /// it: to no one, bug reports.
    fn unset() -> Forward {
        Forward { types: vec![schema::BUG_REPORT.to_owned()], ..Forward::default() }
    }

    fn to_proto(&self) -> v1::PipeForward {
        v1::PipeForward {
            recipient_id: self.recipient.clone(),
            schema_id: self.types.first().cloned().unwrap_or_default(),
            revision: self.revision,
            set_by_id: self.set_by.clone(),
            set_ms: self.set_ms,
            limits: Some(self.limits.to_proto()),
            type_ids: self.types.clone(),
        }
    }
}

/// What a forward is set on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Target<'a> {
    Pipe(&'a str),
    /// A project's default for its new pipes.
    Project(&'a str),
}

impl Target<'_> {
    fn kind(self) -> &'static str {
        match self {
            Target::Pipe(_) => "pipe",
            Target::Project(_) => "project",
        }
    }

    fn id(self) -> String {
        match self {
            Target::Pipe(id) | Target::Project(id) => id.to_owned(),
        }
    }
}

const FORWARD_COLUMNS: &str = "recipient, schema, revision, set_by, set_ms, limits, types";

fn forward_of(row: &rusqlite::Row<'_>) -> rusqlite::Result<Forward> {
    // Kept before report types were data: its one schema.
    let types: Vec<String> = serde_json::from_str(&row.get::<_, String>(6)?).unwrap_or_default();
    Ok(Forward {
        recipient: row.get(0)?,
        types: if types.is_empty() { vec![row.get(1)?] } else { types },
        revision: row.get(2)?,
        set_by: row.get(3)?,
        set_ms: row.get(4)?,
        limits: serde_json::from_str(&row.get::<_, String>(5)?).unwrap_or_default(),
    })
}

/// The forward set on `target`, if any.
pub(crate) fn forward(factory: &Factory, target: Target<'_>) -> Option<Forward> {
    let db = factory.store.db();
    db.query_row(
        &format!("SELECT {FORWARD_COLUMNS} FROM pipe_handoff_forwards WHERE kind = ?1 AND target = ?2"),
        params![target.kind(), target.id()],
        forward_of,
    )
    .optional()
    .unwrap_or_else(|error| {
        tracing::error!(%error, "could not read where a pipe's reports go");
        None
    })
}

/// Every forward, as (kind, target, forward).
pub(crate) fn forwards(factory: &Factory) -> Vec<(String, String, Forward)> {
    let db = factory.store.db();
    let result = db.prepare(&format!("SELECT {FORWARD_COLUMNS}, kind, target FROM pipe_handoff_forwards")).and_then(|mut statement| {
        statement.query_map([], |row| Ok((row.get(7)?, row.get(8)?, forward_of(row)?)))?.collect::<rusqlite::Result<Vec<_>>>()
    });
    result.unwrap_or_else(|error| {
        tracing::error!(%error, "could not read where reports go");
        Vec::new()
    })
}

/// `types` as asked, as kept: each a current type, none twice, 1 to
/// PER_PIPE of them.
fn chosen_types(factory: &Factory, types: &[String]) -> Result<Vec<String>, String> {
    let mut kept: Vec<String> = Vec::new();
    for id in types.iter().map(|id| id.trim()) {
        if types::current(factory, id).is_none() {
            return Err(format!("There's no report type \"{id}\"."));
        }
        if !kept.iter().any(|known| known == id) {
            kept.push(id.to_owned());
        }
    }
    if kept.is_empty() {
        return Err("Choose at least one type of report it can forward.".to_owned());
    }
    if kept.len() > schema::PER_PIPE {
        return Err(format!("A pipe can forward at most {} types of report.", schema::PER_PIPE));
    }
    Ok(kept)
}

/// Whether `agent` reads reports now (a curator, `pipes::curator`).
pub(crate) fn reader(factory: &Factory, agent: &str) -> bool {
    factory.pipes.curator(agent)
}

/// Sets where `target`'s reports go: `recipient` ("" for no one), who must
/// read reports and be in the pipe's (or project's) project, and the report
/// types its liaison may forward (None keeps them: "bug_report" for a new
/// forward). `by` is "" for the person, else the admin agent. A pipe's
/// revision rises when its recipient changes; its reports not yet read then
/// wait for the person. A change of types holds nothing: each report keeps
/// its own type. Kept in the changes, for the history.
pub(crate) fn set_forward(
    factory: &Arc<Factory>,
    target: Target<'_>,
    recipient: &str,
    types: Option<&[String]>,
    limits: Option<Limits>,
    by: &str,
) -> Result<(), String> {
    let before = forward(factory, target);
    let types = match types {
        Some(types) => chosen_types(factory, types)?,
        None => before.clone().unwrap_or_else(Forward::unset).types,
    };
    let project = match target {
        Target::Pipe(id) => {
            let record = factory.pipes.get(id).ok_or_else(|| "That pipe is gone.".to_owned())?;
            if record.state == super::State::Closed {
                return Err("That pipe is closed.".to_owned());
            }
            record.project
        }
        Target::Project(id) => {
            factory.lock().config.project(id).ok_or_else(|| "That project is gone.".to_owned())?;
            id.to_owned()
        }
    };
    let recipient = recipient.trim();
    if !recipient.is_empty() {
        let (name, in_project) = {
            let inner = factory.lock();
            let agent = inner.config.agent(recipient).ok_or_else(|| "That agent is gone.".to_owned())?;
            (agent.name.clone(), agent.project.as_deref() == Some(project.as_str()))
        };
        if !in_project {
            return Err(format!("{name} isn't in this pipe's project, so it can't read its reports."));
        }
        if !reader(factory, recipient) {
            return Err(format!(
                "{name} doesn't read reports yet: an agent that reads them can't run commands, use the web or secrets, or message other agents."
            ));
        }
    }
    write_forward(factory, target, before, recipient, &types, limits, by, false)
}

/// The types as a change keeps them: each with its version then.
fn versions(factory: &Factory, types: &[String]) -> String {
    let kept: Vec<(String, u32)> =
        types.iter().map(|id| (id.clone(), types::current(factory, id).map_or(0, |schema| schema.version))).collect();
    serde_json::to_string(&kept).unwrap_or_default()
}

/// Keeps `target`'s forward (`before` it was this), raising a pipe's revision
/// when the recipient changed and making its reports not yet read wait for
/// the person; keeps each change for the history (`deleted`: its recipient
/// was deleted, so no one, `by` "", and `before`'s recipient is the name it
/// had), and tells the pipe's liaisons.
#[allow(clippy::too_many_arguments)]
fn write_forward(
    factory: &Arc<Factory>,
    target: Target<'_>,
    before: Option<Forward>,
    recipient: &str,
    types: &[String],
    limits: Option<Limits>,
    by: &str,
    deleted: bool,
) -> Result<(), String> {
    // Never kept, it's compared with what the app showed: a first save
    // changes only what the person changed.
    let shown = before.clone().unwrap_or_else(Forward::unset);
    let changed = shown.recipient != recipient;
    let types_changed = shown.types != types;
    let limits = limits.or_else(|| before.as_ref().map(|before| before.limits)).unwrap_or_default();
    if !changed && !types_changed && before.as_ref().is_some_and(|before| before.limits == limits) {
        return Ok(());
    }
    let types_before = versions(factory, &shown.types);
    let types_now = versions(factory, types);
    let revision = match target {
        Target::Pipe(_) => before.as_ref().map_or(0, |before| before.revision) + u32::from(changed),
        Target::Project(_) => 0,
    }
    .max(u32::from(matches!(target, Target::Pipe(_))));
    let now = now_ms();
    {
        let mut db = factory.store.db();
        let transaction = db.transaction().map_err(|error| error.to_string())?;
        transaction
            .execute(
                "INSERT OR REPLACE INTO pipe_handoff_forwards (kind, target, recipient, schema, revision, set_by, set_ms, limits, types)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                params![
                    target.kind(),
                    target.id(),
                    recipient,
                    types.first().cloned().unwrap_or_default(),
                    revision,
                    by,
                    now,
                    serde_json::to_string(&limits).unwrap_or_default(),
                    serde_json::to_string(types).unwrap_or_default()
                ],
            )
            .map_err(|error| error.to_string())?;
        if changed {
            if let Target::Pipe(pipe) = target {
                // Not yet read: they wait for the person, never redirected.
                transaction
                    .execute(
                        "UPDATE pipe_handoff_reports SET state = 'waiting' WHERE pipe = ?1 AND state = 'new' AND read IS NULL AND revision != ?2",
                        params![pipe, revision],
                    )
                    .map_err(|error| error.to_string())?;
            }
            let old = before.as_ref().map(|before| before.recipient.clone()).unwrap_or_default();
            // `kept` on a pipe's or project's change: its recipient was deleted.
            transaction
                .execute(
                    "INSERT INTO pipe_handoff_changes (kind, target, old, new, by, at, kept) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                    params![target.kind(), target.id(), old, recipient, by, now, deleted],
                )
                .map_err(|error| error.to_string())?;
        }
        if types_changed {
            // 'pipe_types' or 'project_types': old and new as [[id, version]…].
            transaction
                .execute(
                    "INSERT INTO pipe_handoff_changes (kind, target, old, new, by, at, kept) VALUES (?1, ?2, ?3, ?4, ?5, ?6, 0)",
                    params![format!("{}_types", target.kind()), target.id(), types_before, types_now, by, now],
                )
                .map_err(|error| error.to_string())?;
        }
        transaction.commit().map_err(|error| error.to_string())?;
    }
    if let Target::Pipe(pipe) = target {
        if changed {
            tell_liaisons(factory, pipe, recipient);
        } else if types_changed && !recipient.is_empty() {
            tell_types(factory, pipe, types);
        }
    }
    factory.broadcast_workspace(&factory.lock());
    Ok(())
}

/// After agents were deleted or moved: forwards to a deleted agent go to no
/// one (an agent made later with its id must not inherit them), and reports
/// not yet read that can't go to their recipient now (deleted, moved out of
/// the pipe's project, no longer a reader) wait for the person, told or not.
pub(crate) fn recheck(factory: &Arc<Factory>) {
    let forwards: Vec<(String, String)> = {
        let db = factory.store.db();
        let result = db.prepare("SELECT kind, target FROM pipe_handoff_forwards WHERE recipient != ''").and_then(|mut statement| {
            statement.query_map([], crate::store::row2)?.collect::<rusqlite::Result<Vec<_>>>()
        });
        result.unwrap_or_else(|error| {
            tracing::error!(%error, "could not read where reports go");
            Vec::new()
        })
    };
    for (kind, id) in forwards {
        let target = if kind == "project" { Target::Project(&id) } else { Target::Pipe(&id) };
        let Some(before) = forward(factory, target) else { continue };
        if before.recipient.is_empty() || factory.lock().config.agent(&before.recipient).is_some() {
            continue;
        }
        // The history names it as it was called: its id names no one now.
        let types = before.types.clone();
        let mut before = before;
        if let Some(name) = wake::deleted_name(factory, &before.recipient) {
            before.recipient = name;
        }
        if let Err(error) = write_forward(factory, target, Some(before), "", &types, None, "", true) {
            tracing::error!(%kind, target = %id, %error, "could not stop forwarding reports to a deleted agent");
        }
    }
    let stuck: Vec<i64> = reports_where(factory, "state = 'new' AND read IS NULL", &[])
        .into_iter()
        .filter(|report| !deliverable(factory, report))
        .map(|report| report.id)
        .collect();
    if !stuck.is_empty() {
        hold(factory, &stuck);
        factory.broadcast_workspace(&factory.lock());
    }
}

/// Tells the pipe's liaisons it forwards its reports now, or no longer.
fn tell_liaisons(factory: &Arc<Factory>, pipe: &str, recipient: &str) {
    let Some(record) = factory.pipes.get(pipe) else { return };
    let (person, name) = {
        let inner = factory.lock();
        let name = inner.config.agent(recipient).map(|agent| agent.name.clone()).unwrap_or_default();
        (inner.config.person_capitalized().to_owned(), name)
    };
    let pipe = label(&record);
    let (summary, text) = if recipient.is_empty() {
        (
            format!("The pipe {pipe} no longer forwards reports"),
            format!("{person} set the pipe {pipe} to forward its reports to no one: handoff_submit is refused until they choose someone again."),
        )
    } else {
        (
            format!("The pipe {pipe} forwards its reports to {name}"),
            format!(
                "The pipe {pipe} now forwards finished reports to {name}, who reads them. When the other side has given you everything a report needs and confirmed your summary, forward it with handoff_submit and tell them its number; handoff_status says where each stands."
            ),
        )
    };
    for liaison in &record.quarantined {
        let text = if recipient.is_empty() { text.clone() } else { format!("{text}{}", submit_now(factory, liaison)) };
        factory.deliver_event(liaison, text, crate::factory::host_event(summary.clone()));
    }
}

/// handoff_submit as it is now for `liaison`, to call through
/// extension_call: a tool reaches the agent's tool list only with its next
/// context, and is announced mid-context once, by name, so a change of its
/// types (and their fields) would otherwise never reach it.
fn submit_now(factory: &Arc<Factory>, liaison: &str) -> String {
    match tools::liaison_tools(factory, liaison).into_iter().find(|tool| tool.spec.name == "handoff_submit") {
        Some(tool) => format!(
            "\n\nUntil your next context, call handoff_submit through extension_call {{\"tool\": \"handoff_submit\", \"args\": {{…}}}}, with these parameters (the ones in your tool list, if it has it, are older): {}",
            tool.spec.parameters
        ),
        None => String::new(),
    }
}

/// The names of `types`, as their authors wrote them, escaped and quoted:
/// `"Bug report" and "Task request"`.
pub(crate) fn type_names(factory: &Factory, types: &[String]) -> String {
    let names: Vec<String> = types
        .iter()
        .filter_map(|id| types::current(factory, id))
        .map(|schema| format!("\"{}\"", super::agents::escape(&schema.name)))
        .collect();
    if names.is_empty() { "no report types".to_owned() } else { factory_agent::text::list_and(&names) }
}

/// Tells the pipe's liaisons which report types it forwards now: their
/// handoff_submit takes those from their next step.
fn tell_types(factory: &Arc<Factory>, pipe: &str, types: &[String]) {
    let Some(record) = factory.pipes.get(pipe) else { return };
    let label = label(&record);
    let names = type_names(factory, types);
    let summary = format!("The pipe {label} forwards other types of report now");
    let text = format!("The pipe {label} now forwards {names}: handoff_submit takes these types, and their fields, from now on.");
    for liaison in &record.quarantined {
        factory.deliver_event(liaison, format!("{text}{}", submit_now(factory, liaison)), crate::factory::host_event(summary.clone()));
    }
}

/// A type was edited: the liaisons of pipes that forward it hear that its
/// fields changed.
pub(crate) fn types_changed(factory: &Arc<Factory>, id: &str) {
    for (kind, pipe, forward) in forwards(factory) {
        if kind == "pipe" && !forward.recipient.is_empty() && forward.types.iter().any(|type_id| type_id == id) {
            tell_types(factory, &pipe, &forward.types);
        }
    }
}

/// Keeps that `agent` was made a report reader, or let go, by `by` ("" for
/// the person), for the history; `kept`: its conversation stayed.
pub(crate) fn reader_changed(factory: &Factory, agent: &str, reads: bool, by: &str, kept: bool) {
    let (old, new) = if reads { ("", agent) } else { (agent, "") };
    if let Err(error) = factory.store.db().execute(
        "INSERT INTO pipe_handoff_changes (kind, target, old, new, by, at, kept) VALUES ('reader', ?1, ?2, ?3, ?4, ?5, ?6)",
        params![agent, old, new, by, now_ms(), kept],
    ) {
        tracing::error!(%agent, %error, "could not keep a change of report readers");
    }
}

/// A new pipe takes its project's default forward, at revision 1.
pub(crate) fn adopt_default(factory: &Factory, record: &Record) {
    let Some(default) = forward(factory, Target::Project(&record.project)).filter(|default| !default.recipient.is_empty()) else { return };
    if let Err(error) = factory.store.db().execute(
        "INSERT OR IGNORE INTO pipe_handoff_forwards (kind, target, recipient, schema, revision, set_by, set_ms, limits, types)
         VALUES ('pipe', ?1, ?2, ?3, 1, ?4, ?5, ?6, ?7)",
        params![
            record.id,
            default.recipient,
            default.types.first().cloned().unwrap_or_default(),
            default.set_by,
            now_ms(),
            serde_json::to_string(&default.limits).unwrap_or_default(),
            serde_json::to_string(&default.types).unwrap_or_default()
        ],
    ) {
        tracing::error!(pipe = %record.id, %error, "could not give a new pipe its project's forward");
    }
}

/// The pipes whose reports go to `agent`.
pub(crate) fn reads_reports(factory: &Factory, agent: &str) -> Vec<String> {
    let db = factory.store.db();
    let mut statement = match db.prepare("SELECT target FROM pipe_handoff_forwards WHERE kind = 'pipe' AND recipient = ?1 ORDER BY target") {
        Ok(statement) => statement,
        Err(error) => {
            tracing::error!(%error, "could not read where reports go");
            return Vec::new();
        }
    };
    statement.query_map([agent], |row| row.get(0)).map(|rows| rows.flatten().collect()).unwrap_or_default()
}

/// Where reports still go to `agent`, for the person: the pipes (by their
/// names, as the app shows them) and projects' new pipes whose forward
/// names it, pipes first. A
/// closed or deleted pipe forwards nothing, nor a deleted project's
/// default, so they don't count. None when it can't be read.
pub(crate) fn forwarding_to(factory: &Factory, agent: &str) -> Option<Vec<String>> {
    let rows: Vec<(String, String)> = {
        let db = factory.store.db();
        let result = db.prepare("SELECT kind, target FROM pipe_handoff_forwards WHERE recipient = ?1 ORDER BY target").and_then(
            |mut statement| statement.query_map([agent], crate::store::row2)?.collect::<rusqlite::Result<Vec<_>>>(),
        );
        match result {
            Ok(rows) => rows,
            Err(error) => {
                tracing::error!(%error, "could not read where reports go");
                return None;
            }
        }
    };
    let (mut pipes, mut projects) = (Vec::new(), Vec::new());
    for (kind, target) in rows {
        if kind == "pipe" {
            if let Some(record) = factory.pipes.get(&target).filter(|record| record.state != super::State::Closed) {
                pipes.push(record.name.clone());
            }
        } else if let Some(project) = factory.lock().config.project(&target).map(|project| project.name.clone()) {
            projects.push(format!("{project}'s new pipes"));
        }
    }
    pipes.sort();
    projects.sort();
    pipes.extend(projects);
    Some(pipes)
}

/// How an agent is told of a pipe: the person's own name for it, else its
/// id, never a name the other side chose (Fugo's change 2).
pub(crate) fn label(record: &Record) -> String {
    if record.named_here.unwrap_or(record.peer.is_none()) && !record.name.trim().is_empty() {
        format!("\"{}\"", super::agents::escape(&record.name))
    } else {
        format!("pipe {}", record.id)
    }
}

fn label_of(factory: &Factory, pipe: &str) -> String {
    factory.pipes.get(pipe).map_or_else(|| format!("pipe {pipe}"), |record| label(&record))
}

// ---- Reports ----

/// A report as kept.
#[derive(Clone, Debug, Default)]
pub(crate) struct Report {
    pub id: i64,
    pub pipe: String,
    pub number: u32,
    pub liaison: String,
    /// Its type's id; "" for a refusal whose type wasn't one the pipe forwards.
    pub schema: String,
    pub type_version: u32,
    pub revision: u32,
    pub recipient: String,
    pub fields: Map<String, Value>,
    pub taint: Taint,
    pub received: i64,
    pub state: String,
    pub alias: String,
    pub fixed_in: String,
    pub needs: String,
    pub refused_code: String,
    pub refused_field: String,
    pub read: Option<i64>,
}

const COLUMNS: &str =
    "id, pipe, number, liaison, schema, revision, recipient, fields, taint, received, state, alias, fixed_in, needs, refused_code, refused_field, read, type_version";

fn report_of(row: &rusqlite::Row<'_>) -> rusqlite::Result<Report> {
    Ok(Report {
        id: row.get(0)?,
        pipe: row.get(1)?,
        number: row.get(2)?,
        liaison: row.get(3)?,
        schema: row.get(4)?,
        revision: row.get(5)?,
        recipient: row.get(6)?,
        fields: serde_json::from_str(&row.get::<_, String>(7)?).unwrap_or_default(),
        taint: serde_json::from_str(&row.get::<_, String>(8)?).unwrap_or_default(),
        received: row.get(9)?,
        state: row.get(10)?,
        alias: row.get(11)?,
        fixed_in: row.get(12)?,
        needs: row.get(13)?,
        refused_code: row.get(14)?,
        refused_field: row.get(15)?,
        read: row.get(16)?,
        type_version: row.get(17)?,
    })
}

pub(crate) fn reports_where(factory: &Factory, condition: &str, values: &[&dyn rusqlite::ToSql]) -> Vec<Report> {
    let db = factory.store.db();
    let query = format!("SELECT {COLUMNS} FROM pipe_handoff_reports WHERE {condition}");
    let result = db.prepare(&query).and_then(|mut statement| statement.query_map(values, report_of)?.collect::<rusqlite::Result<Vec<_>>>());
    result.unwrap_or_else(|error| {
        tracing::error!(%error, "could not read reports");
        Vec::new()
    })
}

/// What a liaison sent, as `handoff_submit` takes it.
pub(crate) struct Submission<'a> {
    pub pipe: &'a Record,
    pub liaison: &'a str,
    /// The report's type; None for the pipe's only one.
    pub type_id: Option<&'a str>,
    pub fields: &'a Map<String, Value>,
    /// Keys of `fields` that aren't the type's (a tool from before types,
    /// whose fields came beside its own parameters).
    pub ignore: &'a [&'a str],
    pub token: &'a str,
    pub revises: Option<u32>,
}

/// Takes a report, or refuses it: what the liaison is told ("accepted #4",
/// "refused: too_long title"), never anything it sent. Its type must be one
/// the pipe forwards, checked against that type's current version.
pub(crate) fn submit(factory: &Arc<Factory>, submission: &Submission<'_>) -> String {
    let record = submission.pipe;
    let Some(forward) = forward(factory, Target::Pipe(&record.id)).filter(|forward| !forward.recipient.is_empty()) else {
        return "refused: no_forward".to_owned();
    };
    let now = now_ms();
    let type_id = match submission.type_id {
        Some(type_id) => forward.types.iter().find(|enabled| enabled.as_str() == type_id),
        None if forward.types.len() == 1 => forward.types.first(),
        None => None,
    };
    let Some(schema) = type_id.and_then(|type_id| types::current(factory, type_id)) else {
        return refuse(factory, record, submission.liaison, &forward, None, schema::Refusal::new("type"), now);
    };
    let schema = schema.as_ref();
    prune(factory, now);
    let token = submission.token.trim();
    if !token.is_empty() {
        let token = token.chars().take(128).collect::<String>();
        if let Some(number) = factory
            .store
            .db()
            .query_row(
                "SELECT number FROM pipe_handoff_reports WHERE pipe = ?1 AND liaison = ?2 AND token = ?3 AND number > 0",
                params![record.id, submission.liaison, token],
                |row| row.get::<_, u32>(0),
            )
            .optional()
            .ok()
            .flatten()
        {
            return format!("accepted #{number}");
        }
    }
    let refused = |refusal: schema::Refusal| refuse(factory, record, submission.liaison, &forward, Some(schema), refusal, now);
    let checked = match schema::check(schema, submission.fields, submission.ignore) {
        Ok(checked) => checked,
        Err(refusal) => return refused(refusal),
    };
    if crate::pipes::agents::secrets_in(factory, &checked.text(schema), &[]).is_some() {
        return refused(schema::Refusal::new("secret"));
    }
    let revised = match submission.revises {
        Some(number) => {
            let found = reports_where(
                factory,
                "pipe = ?1 AND number = ?2 AND liaison = ?3",
                &[&record.id, &number, &submission.liaison.to_owned()],
            );
            match found.into_iter().next() {
                Some(report) if report.state == "new" && report.read.is_none() && report.revision == forward.revision && report.schema == schema.id => {
                    Some(report)
                }
                _ => return refused(schema::Refusal::new("cannot_revise")),
            }
        }
        None => None,
    };
    if let Some(refusal) = over_budget(factory, &record.id, &forward, checked.text_bytes, revised.is_none(), now) {
        return refused(refusal);
    }
    let mut checked = checked;
    let files_field = schema.attachments().map(|field| field.name.clone()).unwrap_or_default();
    let handles: Vec<String> = match checked.fields.remove(&files_field) {
        Some(Value::Array(items)) => items.into_iter().filter_map(|item| item.as_str().map(str::to_owned)).collect(),
        _ => Vec::new(),
    };
    let files = match attachments::take(factory, record, &handles, &files_field, forward.limits, now) {
        Ok(files) => files,
        Err(refusal) => return refused(refusal),
    };
    let fields = serde_json::to_string(&checked.fields).unwrap_or_default();
    // Reading a report marks no one since #346; the column stays, empty.
    let taint = "{}".to_owned();
    let after_seq = i64::try_from(record.next_seq.saturating_sub(1)).unwrap_or(0);
    let text_bytes = i64::try_from(checked.text_bytes).unwrap_or(i64::MAX);
    let kept = {
        let db = factory.store.db();
        let kept = match &revised {
            Some(report) => db
                .execute(
                    "UPDATE pipe_handoff_reports SET fields = ?1, text_bytes = ?2, taint = ?3, after_seq = ?4, received = ?5, delivered = NULL, type_version = ?7
                     WHERE id = ?6",
                    params![fields, text_bytes, taint, after_seq, now, report.id, schema.version],
                )
                .map(|_| (report.id, report.number)),
            None => db
                .query_row("SELECT COALESCE(MAX(number), 0) + 1 FROM pipe_handoff_reports WHERE pipe = ?1", [&record.id], |row| row.get::<_, u32>(0))
                .and_then(|number| {
                    db.execute(
                        "INSERT INTO pipe_handoff_reports
                         (pipe, number, liaison, schema, revision, recipient, fields, text_bytes, token, after_seq, taint, received, state, type_version)
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, 'new', ?13)",
                        params![
                            record.id,
                            number,
                            submission.liaison,
                            schema.id,
                            forward.revision,
                            forward.recipient,
                            fields,
                            text_bytes,
                            token,
                            after_seq,
                            taint,
                            now,
                            schema.version
                        ],
                    )
                    .map(|_| (db.last_insert_rowid(), number))
                }),
        };
        match kept {
            Ok(kept) => kept,
            Err(error) => {
                tracing::error!(pipe = %record.id, %error, "could not keep a report");
                return "refused: not_kept".to_owned();
            }
        }
    };
    let (id, number) = kept;
    if !files.is_empty() || revised.is_some() {
        attachments::keep(factory, &record.id, id, &files, now);
    }
    wake::wake(factory, &forward.recipient);
    factory.broadcast_workspace(&factory.lock());
    format!("accepted #{number}")
}

/// Keeps a refusal (its code and field only, and its type when it was one
/// the pipe forwards), a hundred a day at most, and says it.
fn refuse(
    factory: &Arc<Factory>,
    record: &Record,
    liaison: &str,
    forward: &Forward,
    schema: Option<&schema::Schema>,
    refusal: schema::Refusal,
    now: i64,
) -> String {
    let kept: i64 = factory
        .store
        .db()
        .query_row(
            "SELECT COUNT(*) FROM pipe_handoff_reports WHERE pipe = ?1 AND state = 'refused' AND received > ?2",
            params![record.id, now - DAY_MS],
            |row| row.get(0),
        )
        .unwrap_or(MAX_REFUSALS_A_DAY);
    if kept < MAX_REFUSALS_A_DAY {
        if let Err(error) = factory.store.db().execute(
            "INSERT INTO pipe_handoff_reports
             (pipe, number, liaison, schema, revision, recipient, fields, text_bytes, token, after_seq, taint, received, state, refused_code, refused_field, type_version)
             VALUES (?1, 0, ?2, ?3, ?4, ?5, '{}', 0, '', ?6, '{}', ?7, 'refused', ?8, ?9, ?10)",
            params![
                record.id,
                liaison,
                schema.map(|schema| schema.id.clone()).unwrap_or_default(),
                forward.revision,
                forward.recipient,
                i64::try_from(record.next_seq.saturating_sub(1)).unwrap_or(0),
                now,
                refusal.code,
                refusal.field.clone().unwrap_or_default(),
                schema.map_or(0, |schema| schema.version)
            ],
        ) {
            tracing::error!(pipe = %record.id, %error, "could not keep a refused report");
        }
        factory.broadcast_workspace(&factory.lock());
    }
    refusal.said()
}

/// The limit a new report (or a revision, `new` false) would pass.
fn over_budget(factory: &Factory, pipe: &str, forward: &Forward, text_bytes: usize, new: bool, now: i64) -> Option<schema::Refusal> {
    let limits = forward.limits.effective();
    let db = factory.store.db();
    let count = |since: i64| -> i64 {
        db.query_row(
            "SELECT COUNT(*) FROM pipe_handoff_reports WHERE pipe = ?1 AND number > 0 AND received > ?2",
            params![pipe, since],
            |row| row.get(0),
        )
        .unwrap_or(i64::MAX)
    };
    let bytes: i64 = db
        .query_row(
            "SELECT COALESCE(SUM(text_bytes), 0) FROM pipe_handoff_reports WHERE pipe = ?1 AND number > 0 AND received > ?2",
            params![pipe, now - DAY_MS],
            |row| row.get(0),
        )
        .unwrap_or(i64::MAX);
    let over = (new && (count(now - HOUR_MS) >= i64::from(limits.per_hour) || count(now - DAY_MS) >= i64::from(limits.per_day)))
        || bytes + i64::try_from(text_bytes).unwrap_or(i64::MAX) > i64::from(limits.text_kib_per_day) * 1024;
    over.then(|| schema::Refusal::new("over_limit"))
}

/// Reports past 90 days go.
fn prune(factory: &Factory, now: i64) {
    match factory.store.db().execute("DELETE FROM pipe_handoff_reports WHERE received < ?1", [now - KEEP_MS]) {
        Ok(0) => return,
        Ok(_) => {}
        Err(error) => return tracing::error!(%error, "could not forget old reports"),
    }
    attachments::prune(factory);
}

/// Whether `report` may go to its recipient now: the forward it was taken
/// under is still the pipe's, and its recipient still exists, is in the
/// pipe's project and reads reports. Otherwise it waits for the person.
pub(crate) fn deliverable(factory: &Factory, report: &Report) -> bool {
    let Some(record) = factory.pipes.get(&report.pipe) else { return false };
    let current = forward(factory, Target::Pipe(&report.pipe));
    current.is_some_and(|forward| forward.revision == report.revision && forward.recipient == report.recipient)
        && factory.lock().config.agent(&report.recipient).is_some_and(|agent| agent.project.as_deref() == Some(record.project.as_str()))
        && reader(factory, &report.recipient)
}

/// Its reports not yet read go to the person instead of `report`'s
/// recipient: the forward changed, or its recipient no longer reads reports.
pub(crate) fn hold(factory: &Factory, ids: &[i64]) {
    let db = factory.store.db();
    for id in ids {
        if let Err(error) = db.execute("UPDATE pipe_handoff_reports SET state = 'waiting' WHERE id = ?1 AND state = 'new' AND read IS NULL", [id]) {
            tracing::error!(%error, "could not hold a report");
        }
    }
}

/// The person sends a pipe's waiting reports to its current recipient, or
/// discards them.
pub(crate) fn decide_waiting(factory: &Arc<Factory>, pipe: &str, send: bool) -> Result<(), String> {
    factory.pipes.get(pipe).ok_or_else(|| "That pipe is gone.".to_owned())?;
    if send {
        let forward = forward(factory, Target::Pipe(pipe)).filter(|forward| !forward.recipient.is_empty());
        let Some(forward) = forward.filter(|forward| reader(factory, &forward.recipient)) else {
            return Err("Choose who reads this pipe's reports first.".to_owned());
        };
        factory
            .store
            .db()
            .execute(
                "UPDATE pipe_handoff_reports SET state = 'new', revision = ?2, recipient = ?3, delivered = NULL WHERE pipe = ?1 AND state = 'waiting'",
                params![pipe, forward.revision, forward.recipient],
            )
            .map_err(|error| error.to_string())?;
        wake::wake(factory, &forward.recipient);
    } else {
        factory
            .store
            .db()
            .execute("UPDATE pipe_handoff_reports SET state = 'discarded' WHERE pipe = ?1 AND state = 'waiting'", [pipe])
            .map_err(|error| error.to_string())?;
    }
    factory.broadcast_workspace(&factory.lock());
    Ok(())
}

/// What the liaison is told of a report's state.
pub(crate) fn status_words(report: &Report) -> String {
    match report.state.as_str() {
        "accepted" => "accepted".to_owned(),
        "done" => "done".to_owned(),
        "declined" => "declined".to_owned(),
        "known" => format!("known as {}", report.alias),
        "fixed" => format!("fixed in {}", report.fixed_in),
        "wont_fix" => "won't fix".to_owned(),
        "needs_info" => format!("needs more: {}", report.needs),
        "discarded" => "closed".to_owned(),
        // Waiting is the person's business: to the pipe's side it's new.
        _ => "new".to_owned(),
    }
}

/// The states a report its reader can still mark is in.
const MARKABLE: &str = "('new', 'accepted', 'done', 'declined', 'known', 'fixed', 'wont_fix', 'needs_info')";

/// The reader marks a report of its own: what its pipe's liaison will be
/// told, one of the outcomes its type's version allows. `issue` is its own
/// key, which never leaves: the pipe gets its own alias for it, made here.
pub(crate) fn mark(factory: &Arc<Factory>, reader_id: &str, id: i64, state: &str, issue: &str, fixed_in: &str, needs: &str) -> Result<String, String> {
    let report = reports_where(factory, &format!("id = ?1 AND recipient = ?2 AND state IN {MARKABLE}"), &[&id, &reader_id.to_owned()])
        .into_iter()
        .next()
        .ok_or_else(|| format!("You have no report {id}."))?;
    let schema = types::version(factory, &report.schema, report.type_version).ok_or_else(|| "That report's type can't be read.".to_owned())?;
    let allowed = || {
        let ids: Vec<&str> = schema.outcomes.iter().map(|outcome| outcome.id()).collect();
        format!("A report of this type can be marked {}.", ids.join(", "))
    };
    let outcome = schema::Outcome::of(state).filter(|outcome| schema.allows(*outcome)).ok_or_else(allowed)?;
    let (state, alias, fixed_in, needs, issue) = match outcome {
        schema::Outcome::Accepted | schema::Outcome::Done | schema::Outcome::Declined | schema::Outcome::WontFix => {
            (outcome.id(), String::new(), String::new(), String::new(), String::new())
        }
        schema::Outcome::Known => {
            let issue = schema::cleaned(issue, true).chars().take(64).collect::<String>();
            if issue.is_empty() {
                return Err("known needs issue: your own name for the issue (the pipe gets its own for it).".to_owned());
            }
            ("known", alias(factory, &report.pipe, &issue)?, String::new(), String::new(), issue)
        }
        schema::Outcome::Fixed => {
            let version = schema::known_version(fixed_in.trim()).filter(|version| *version != "unknown");
            let version = version.ok_or_else(|| "fixed needs fixed_in: a Claktory release, such as v0.1.17.".to_owned())?;
            ("fixed", String::new(), version.to_owned(), String::new(), String::new())
        }
        schema::Outcome::NeedsInfo => {
            let field = schema.field(needs.trim()).ok_or_else(|| {
                let names: Vec<&str> = schema.fields.iter().map(|field| field.name.as_str()).collect();
                format!("needs_info needs needs: one of {}.", names.join(", "))
            })?;
            ("needs_info", String::new(), String::new(), field.name.clone(), String::new())
        }
    };
    factory
        .store
        .db()
        .execute(
            "UPDATE pipe_handoff_reports SET state = ?2, alias = ?3, fixed_in = ?4, needs = ?5, issue = ?6 WHERE id = ?1",
            params![id, state, alias, fixed_in, needs, issue],
        )
        .map_err(|error| error.to_string())?;
    factory.broadcast_workspace(&factory.lock());
    let pipe = label_of(factory, &report.pipe);
    let updated = Report { state: state.to_owned(), alias, fixed_in, needs, ..report };
    Ok(format!("Report {id} ({pipe} #{}): {}.", updated.number, status_words(&updated)))
}

/// The pipe's alias for `issue`: the one it has, or the next ("A1" … "A9999",
/// then "B1").
fn alias(factory: &Factory, pipe: &str, issue: &str) -> Result<String, String> {
    let db = factory.store.db();
    if let Some(alias) =
        db.query_row("SELECT alias FROM pipe_handoff_aliases WHERE pipe = ?1 AND issue = ?2", params![pipe, issue], |row| row.get(0)).optional().map_err(|error| error.to_string())?
    {
        return Ok(alias);
    }
    let count: u32 =
        db.query_row("SELECT COUNT(*) FROM pipe_handoff_aliases WHERE pipe = ?1", [pipe], |row| row.get(0)).map_err(|error| error.to_string())?;
    let letter = count / 9999;
    if letter >= 26 {
        return Err("This pipe has run out of issue names.".to_owned());
    }
    let alias = format!("{}{}", char::from(b'A' + u8::try_from(letter).unwrap_or(0)), count % 9999 + 1);
    db.execute("INSERT INTO pipe_handoff_aliases (pipe, issue, alias) VALUES (?1, ?2, ?3)", params![pipe, issue, alias]).map_err(|error| error.to_string())?;
    Ok(alias)
}

// ---- For the app ----

/// The report types, for Workspace.report_schemas.
pub(crate) fn schemas(factory: &Factory) -> Vec<v1::ReportSchema> {
    types::currents(factory).iter().map(|schema| schema::to_proto(schema)).collect()
}

/// Pipe.forward and Pipe.reports.
pub(crate) fn pipe_view(factory: &Factory, record: &Record) -> (Option<v1::PipeForward>, Option<v1::PipeReportCounts>) {
    let forward = forward(factory, Target::Pipe(&record.id));
    let now = now_ms();
    let db = factory.store.db();
    let count = |condition: &str, since: i64| -> u32 {
        db.query_row(
            &format!("SELECT COUNT(*) FROM pipe_handoff_reports WHERE pipe = ?1 AND received > ?2 AND {condition}"),
            params![record.id, since],
            |row| row.get(0),
        )
        .unwrap_or(0)
    };
    let today = count("number > 0", now - DAY_MS);
    let hour = count("number > 0", now - HOUR_MS);
    let refused_today = count("state = 'refused'", now - DAY_MS);
    let over_limit_today = count("state = 'refused' AND refused_code = 'over_limit'", now - DAY_MS);
    let waiting = count("state = 'waiting'", 0);
    let limits = forward.as_ref().map(|forward| forward.limits).unwrap_or_default().effective();
    let room_today = limits.per_day.saturating_sub(today).min(limits.per_hour.saturating_sub(hour));
    let counts = (today + refused_today + waiting > 0 || forward.is_some()).then_some(v1::PipeReportCounts {
        today,
        room_today,
        refused_today,
        over_limit_today,
        waiting,
    });
    (forward.map(|forward| forward.to_proto()), counts)
}

/// Project.report_forward.
pub(crate) fn project_view(factory: &Factory, project: &str) -> Option<v1::PipeForward> {
    forward(factory, Target::Project(project)).map(|forward| forward.to_proto())
}

fn state_proto(state: &str) -> v1::PipeReportState {
    match state {
        "new" => v1::PipeReportState::New,
        "accepted" => v1::PipeReportState::Accepted,
        "known" => v1::PipeReportState::Known,
        "fixed" => v1::PipeReportState::Fixed,
        "wont_fix" => v1::PipeReportState::WontFix,
        "needs_info" => v1::PipeReportState::NeedsInfo,
        "refused" => v1::PipeReportState::Refused,
        "waiting" => v1::PipeReportState::Waiting,
        "discarded" => v1::PipeReportState::Discarded,
        "done" => v1::PipeReportState::Done,
        "declined" => v1::PipeReportState::Declined,
        _ => v1::PipeReportState::Unspecified,
    }
}

/// ListPipeReports: a page, newest first. Reading it marks no agent.
pub(crate) fn list(factory: &Factory, request: &v1::ListPipeReports) -> Result<v1::PipeReportPage, RequestError> {
    let limit = match request.limit {
        0 => 50,
        limit => limit.min(200),
    };
    let before = if request.before_id == 0 { i64::MAX } else { i64::try_from(request.before_id).unwrap_or(i64::MAX) };
    let fetch = i64::from(limit) + 1;
    let mut reports = if !request.pipe_id.is_empty() {
        reports_where(factory, "pipe = ?1 AND id < ?2 ORDER BY id DESC LIMIT ?3", &[&request.pipe_id, &before, &fetch])
    } else if !request.recipient_id.is_empty() {
        reports_where(
            factory,
            "recipient = ?1 AND number > 0 AND id < ?2 ORDER BY id DESC LIMIT ?3",
            &[&request.recipient_id, &before, &fetch],
        )
    } else {
        return Err(RequestError::Remote("Say which pipe's reports, or whose.".to_owned()));
    };
    let more = reports.len() > limit as usize;
    reports.truncate(limit as usize);
    let routes = |taint: &Taint| -> Vec<v1::PipeTaint> {
        let inner = factory.lock();
        let name = |id: &String| inner.config.agent(id).map_or_else(|| id.clone(), |agent| agent.name.clone());
        taint
            .iter()
            .map(|(key, via)| {
                let kind = super::agents::Kind::of(key);
                v1::PipeTaint {
                    pipe_name: super::agents::taint_name(factory, key),
                    pipe_id: if let super::agents::Kind::Pipe(id) = kind { id.to_owned() } else { String::new() },
                    via_names: via.iter().map(name).collect(),
                    kind: match kind {
                        super::agents::Kind::Pipe(_) => String::new(),
                        super::agents::Kind::Browser(_) => "browser".to_owned(),
                        super::agents::Kind::Dm(_) => "dm".to_owned(),
                    },
                }
            })
            .collect()
    };
    // The versions its reports use that aren't current, for the app to show
    // each against its own.
    let mut schemas: Vec<v1::ReportSchema> = Vec::new();
    for report in reports.iter().filter(|report| !report.schema.is_empty()) {
        let current = types::current(factory, &report.schema).is_some_and(|schema| schema.version == report.type_version);
        let listed = schemas.iter().any(|schema| schema.id == report.schema && schema.version == report.type_version);
        if !current && !listed && let Some(schema) = types::version(factory, &report.schema, report.type_version) {
            schemas.push(schema::to_proto(&schema));
        }
    }
    let reports = reports
        .into_iter()
        .map(|report| {
            let schema = types::version(factory, &report.schema, report.type_version);
            let order: Vec<&str> = schema.as_ref().map(|schema| schema.fields.iter().map(|field| field.name.as_str()).collect()).unwrap_or_default();
            let mut values: Vec<v1::ReportValue> =
                order.iter().filter_map(|name| report.fields.get(*name).map(|value| schema::value_proto(name, value))).collect();
            values.extend(report.fields.iter().filter(|(name, _)| !order.contains(&name.as_str())).map(|(name, value)| schema::value_proto(name, value)));
            v1::PipeReport {
                id: u64::try_from(report.id).unwrap_or_default(),
                pipe_id: report.pipe.clone(),
                number: report.number,
                state: state_proto(&report.state) as i32,
                alias: report.alias.clone(),
                fixed_in: report.fixed_in.clone(),
                needs_field: report.needs.clone(),
                refused_code: report.refused_code.clone(),
                refused_field: report.refused_field.clone(),
                schema_id: report.schema.clone(),
                values,
                liaison_id: report.liaison.clone(),
                recipient_id: report.recipient.clone(),
                received_ms: report.received,
                read: report.read.is_some(),
                read_ms: report.read.unwrap_or_default(),
                taint: routes(&report.taint),
                attachments: attachments::of(factory, report.id)
                    .into_iter()
                    .map(|file| v1::ReportAttachment {
                        name: file.name,
                        size: file.size,
                        image: file.image.is_some(),
                        path: attachments::store(factory, &report.pipe).join(&file.file).display().to_string(),
                    })
                    .collect(),
                type_version: report.type_version,
            }
        })
        .collect();
    Ok(v1::PipeReportPage { pipe_id: request.pipe_id.clone(), recipient_id: request.recipient_id.clone(), reports, more, schemas })
}

/// At start: reports past 90 days go, those that can't go to their
/// recipient now wait (`recheck`, again whenever agents are deleted or
/// moved), and readers with reports waiting for them are woken.
pub(crate) fn start(factory: &Arc<Factory>) {
    prune(factory, now_ms());
    attachments::prune(factory);
    recheck(factory);
    wake::watch_agents(factory);
    let recipients: Vec<String> = {
        let db = factory.store.db();
        let result = db.prepare("SELECT DISTINCT recipient FROM pipe_handoff_reports WHERE state = 'new' AND delivered IS NULL").and_then(|mut statement| {
            statement.query_map([], |row| row.get::<_, String>(0))?.collect::<rusqlite::Result<Vec<_>>>()
        });
        result.unwrap_or_default()
    };
    for recipient in recipients {
        wake::wake(factory, &recipient);
    }
}

/// The reports `agent` may read: its own, forwarded to it, not refused,
/// waiting or discarded.
pub(crate) fn readable(factory: &Factory, agent: &str, condition: &str, values: &[&dyn rusqlite::ToSql]) -> Vec<Report> {
    let agent = agent.to_owned();
    let mut all: Vec<&dyn rusqlite::ToSql> = vec![&agent];
    all.extend_from_slice(values);
    reports_where(
        factory,
        &format!("recipient = ?1 AND state IN {MARKABLE} AND {condition}"),
        &all,
    )
}

/// Pipes by id, for naming them.
pub(crate) fn pipes_by_id(factory: &Factory) -> BTreeMap<String, Record> {
    factory.pipes.all().into_iter().map(|record| (record.id.clone(), record)).collect()
}

/// Makes `agent` a report reader (`by`: "" for the person, else the admin
/// agent), and keeps the change for the history.
pub(crate) async fn make_reader(factory: &Arc<Factory>, agent: &str, by: &str, fresh: bool) -> Result<(), String> {
    if reader(factory, agent) {
        return Ok(());
    }
    super::curator::make(factory, agent, by, fresh).await?;
    reader_changed(factory, agent, true, by, !fresh);
    factory.broadcast_workspace(&factory.lock());
    Ok(())
}

/// SetPipeForward, from the person.
pub(crate) async fn set_forward_request(factory: &Arc<Factory>, request: &v1::SetPipeForward) -> Result<(), RequestError> {
    let target = match (request.pipe_id.trim(), request.project_id.trim()) {
        ("", "") => return Err(RequestError::Remote("Say which pipe, or which project's new pipes.".to_owned())),
        ("", project) => Target::Project(project),
        (pipe, _) => Target::Pipe(pipe),
    };
    let recipient = request.recipient_id.trim();
    if request.make_reader && !recipient.is_empty() && !reader(factory, recipient) {
        make_reader(factory, recipient, "", request.fresh).await.map_err(RequestError::Remote)?;
    }
    let limits = request.limits.as_ref().map(Limits::from_proto);
    // An older app names one type: a change only when it isn't on already.
    let schema_id = request.schema_id.trim();
    let types = if !request.type_ids.is_empty() {
        Some(request.type_ids.clone())
    } else if schema_id.is_empty() || forward(factory, target).is_some_and(|before| before.types.iter().any(|id| id == schema_id)) {
        None
    } else {
        Some(vec![schema_id.to_owned()])
    };
    set_forward(factory, target, recipient, types.as_deref(), limits, "").map_err(RequestError::Remote)
}

/// SaveReportType, from the person.
pub(crate) fn save_type_request(factory: &Arc<Factory>, request: &v1::SaveReportType) -> Result<(), RequestError> {
    types::save_request(factory, request)
}

/// DeleteReportType, from the person.
pub(crate) fn delete_type_request(factory: &Arc<Factory>, request: &v1::DeleteReportType) -> Result<(), RequestError> {
    types::delete_request(factory, request)
}

/// The ceilings a report type stays within, for Workspace.report_type_rules.
pub(crate) fn type_rules() -> v1::ReportTypeRules {
    schema::rules_proto()
}

/// SetReportReader: the person makes an agent a reader, or lets it go.
pub(crate) async fn set_reader_request(factory: &Arc<Factory>, request: &v1::SetReportReader) -> Result<(), RequestError> {
    let agent = request.agent_id.trim();
    if request.reads {
        return make_reader(factory, agent, "", request.fresh).await.map_err(RequestError::Remote);
    }
    if !reader(factory, agent) {
        return Ok(());
    }
    super::curator::release(factory, agent, request.fresh).await.map_err(RequestError::Remote)?;
    reader_changed(factory, agent, false, "", !request.fresh);
    factory.broadcast_workspace(&factory.lock());
    Ok(())
}

#[doc(hidden)]
pub fn set_wake_every(factory: &Factory, every: std::time::Duration) {
    factory.pipes.handoff.set_every(every);
}

/// Tests: `agent`'s handoff_submit parameters, as JSON, if it has the tool.
#[doc(hidden)]
pub fn submit_parameters_for_test(factory: &Arc<Factory>, agent: &str) -> Option<String> {
    tools::liaison_tools(factory, agent).into_iter().find(|tool| tool.spec.name == "handoff_submit").map(|tool| tool.spec.parameters.to_string())
}

/// Tests: a pipe's forward, as (recipient, revision, set by).
#[doc(hidden)]
pub fn forward_for_test(factory: &Factory, pipe: &str) -> Option<(String, u32, String)> {
    forward(factory, Target::Pipe(pipe)).map(|forward| (forward.recipient, forward.revision, forward.set_by))
}
