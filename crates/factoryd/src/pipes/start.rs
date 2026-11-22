//! An agent starts a pipe for the person (#244): `pipes_invite` and
//! `pipes_accept` (the Pipes extension, Rivet's by default) open a card in
//! its conversation (`v1::PipeStart`), filled in from what the person told
//! it. The person makes the invite or pastes the code they were given on
//! the card themselves, through the requests the app's own dialogs send
//! (CreatePipeInvite, AcceptPipeInvite, with the card's id); the agent never
//! sees a code, never approves, and is told only what the person chose or
//! Claktory itself knows: never a word the other side wrote (its names,
//! its purpose), which reaches no agent but through a pipe's fence.
//!
//! Cards live in factory.db; one left waiting a day closes, and the agent
//! is told.

use std::sync::{Arc, Weak};
use std::time::Duration;

use factory_protocol::v1::{self, PipeStartKind as Kind, PipeStartState as State};
use rusqlite::{OptionalExtension, params};

use super::Record;
use crate::factory::{Factory, RequestError, host_event};
use crate::store::Store;
use crate::transcript::{Transcript, now_ms};
use crate::vault::crypto;

pub(crate) const SCHEMA: &str = "
-- Pipes an agent starts for the person (pipes_invite, pipes_accept): the
-- card in its conversation.
CREATE TABLE pipe_starts (
    id TEXT PRIMARY KEY,
    -- The agent that opened it.
    agent TEXT NOT NULL,
    -- invite or accept
    kind TEXT NOT NULL,
    -- What it was filled in with: the project's id, the pipe's name and
    -- what it's for.
    project TEXT NOT NULL,
    name TEXT NOT NULL,
    purpose TEXT NOT NULL,
    -- waiting, done or dismissed
    state TEXT NOT NULL,
    -- done: the pipe made.
    pipe TEXT NOT NULL DEFAULT '',
    created INTEGER NOT NULL,
    ended INTEGER
);
CREATE INDEX pipe_starts_by_agent ON pipe_starts (agent, created);
CREATE INDEX pipe_starts_by_state ON pipe_starts (state);
";

/// How long a card waits for the person before it closes.
pub(crate) const WAITS_MS: i64 = 24 * 3_600_000;

/// The longest name and purpose a card is filled in with.
const MAX_NAME: usize = 80;
const MAX_PURPOSE: usize = 300;

/// A card, as kept.
struct Row {
    id: String,
    agent: String,
    kind: Kind,
    project: String,
    name: String,
    purpose: String,
    state: State,
    pipe: String,
    ended: Option<i64>,
}

const COLUMNS: &str = "id, agent, kind, project, name, purpose, state, pipe, ended";

impl Row {
    fn read(row: &rusqlite::Row<'_>) -> rusqlite::Result<Row> {
        let kind: String = row.get(2)?;
        let state: String = row.get(6)?;
        Ok(Row {
            id: row.get(0)?,
            agent: row.get(1)?,
            kind: if kind == "accept" { Kind::Accept } else { Kind::Invite },
            project: row.get(3)?,
            name: row.get(4)?,
            purpose: row.get(5)?,
            state: match state.as_str() {
                "done" => State::Done,
                "dismissed" => State::Dismissed,
                _ => State::Waiting,
            },
            pipe: row.get(7)?,
            ended: row.get(8)?,
        })
    }

    fn to_proto(&self) -> v1::PipeStart {
        v1::PipeStart {
            id: self.id.clone(),
            agent_id: self.agent.clone(),
            kind: self.kind.into(),
            project_id: self.project.clone(),
            name: self.name.clone(),
            purpose: self.purpose.clone(),
            state: self.state.into(),
            pipe_id: self.pipe.clone(),
            ended_ms: self.ended.unwrap_or_default(),
        }
    }
}

fn kind_word(kind: Kind) -> &'static str {
    if kind == Kind::Accept { "accept" } else { "invite" }
}

/// Text that holds a pipe code, or what looks like one's secret: an agent
/// never carries one onto a card. Best effort only; what keeps codes from
/// agents is that no tool takes one.
fn code_like(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    if lower.contains("claktory-pipe:") {
        return true;
    }
    let mut run = 0;
    for character in text.chars() {
        run = if character.is_ascii_hexdigit() { run + 1 } else { 0 };
        if run >= 32 {
            return true;
        }
    }
    false
}

/// What an agent fills a card in with.
pub(crate) struct Fill {
    pub kind: Kind,
    pub project: String,
    pub name: String,
    pub purpose: String,
}

/// Opens a card in `agent`'s conversation (the tool's result shows it):
/// the project, by id or name, must exist. The result's text and details.
pub(crate) fn open(factory: &Factory, agent: &str, fill: Fill) -> Result<(String, serde_json::Value), String> {
    let (project, person) = {
        let inner = factory.lock();
        let wanted = fill.project.trim();
        let project = inner
            .config
            .projects
            .iter()
            .find(|project| project.id == wanted || project.name.eq_ignore_ascii_case(wanted))
            .map(|project| (project.id.clone(), project.name.clone()));
        (project, inner.config.person().to_owned())
    };
    let Some((project, project_name)) = project else {
        return Err(format!("There's no project \"{}\": name one of the person's projects.", fill.project.trim()));
    };
    let name = fill.name.trim().to_owned();
    let purpose = fill.purpose.trim().to_owned();
    if code_like(&name) || code_like(&purpose) {
        return Err(format!(
            "That looks like a pipe's code. Never pass one on: {person} pastes it on the card themselves, and you never see it."
        ));
    }
    if name.chars().count() > MAX_NAME || purpose.chars().count() > MAX_PURPOSE {
        return Err(format!("Keep the name under {MAX_NAME} characters and what it's for under {MAX_PURPOSE}."));
    }
    if fill.kind == Kind::Invite && name.is_empty() {
        return Err("Give the pipe a name, such as \"Launch\": the other person sees it.".to_owned());
    }
    let id = format!("s{}", crypto::hex(&crypto::random::<6>()));
    let inserted = factory.store.db().execute(
        "INSERT INTO pipe_starts (id, agent, kind, project, name, purpose, state, created) VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'waiting', ?7)",
        params![id, agent, kind_word(fill.kind), project, name, purpose, now_ms()],
    );
    if let Err(error) = inserted {
        return Err(format!("Couldn't open the card: {error}"));
    }
    let said = match fill.kind {
        Kind::Accept => format!(
            "Opened a card in your conversation for {person} to connect {project_name} with the code they were given: they paste it there themselves, see whom it says it's from, and connect; then both people approve the pipe in it. You never see the code. You'll be told when it's connected."
        ),
        _ => format!(
            "Opened a card in your conversation for {person} to make the invite for \"{name}\" in {project_name}: they check it, make it, and send its code to the other person themselves. You never see the code. You'll be told when it's made."
        ),
    };
    let details = serde_json::json!({
        "pipe_start": id,
        "agent": agent,
        "kind": kind_word(fill.kind),
        "project": project,
        "name": name,
        "purpose": purpose,
    });
    Ok((said, details))
}

/// The card a pipes_invite or pipes_accept result shows (called directly,
/// or through extension_call), waiting; None for any other result.
pub(crate) fn started(tool: &str, is_error: bool, details: &serde_json::Value) -> Option<v1::PipeStart> {
    if !matches!(tool, "pipes_invite" | "pipes_accept" | "extension_call") || is_error {
        return None;
    }
    let text = |key: &str| details.get(key).and_then(serde_json::Value::as_str).unwrap_or_default().to_owned();
    let id = details.get("pipe_start")?.as_str()?.to_owned();
    Some(v1::PipeStart {
        id,
        agent_id: text("agent"),
        kind: if text("kind") == "accept" { Kind::Accept } else { Kind::Invite }.into(),
        project_id: text("project"),
        name: text("name"),
        purpose: text("purpose"),
        state: State::Waiting.into(),
        pipe_id: String::new(),
        ended_ms: 0,
    })
}

fn row(store: &Store, id: &str) -> Option<Row> {
    store.db().query_row(&format!("SELECT {COLUMNS} FROM pipe_starts WHERE id = ?1"), [id], Row::read).optional().ok().flatten()
}

/// Shows the agent's cards as they are in its just-loaded conversation.
pub(crate) fn restore(store: &Store, agent: &str, transcript: &mut Transcript) -> rusqlite::Result<()> {
    let db = store.db();
    let mut statement = db.prepare(&format!("SELECT {COLUMNS} FROM pipe_starts WHERE agent = ?1 ORDER BY created"))?;
    for row in statement.query_map([agent], Row::read)? {
        transcript.set_pipe_start(row?.to_proto());
    }
    Ok(())
}

/// Whether a card of the agent's waits for the person.
pub(crate) fn waiting_of(store: &Store, agent: &str) -> bool {
    store
        .db()
        .query_row("SELECT 1 FROM pipe_starts WHERE agent = ?1 AND state = 'waiting' LIMIT 1", [agent], |_| Ok(()))
        .optional()
        .ok()
        .flatten()
        .is_some()
}

/// An agent is deleted: its cards go.
pub(crate) fn forget(store: &Store, agent: &str) -> rusqlite::Result<()> {
    store.db().execute("DELETE FROM pipe_starts WHERE agent = ?1", [agent])?;
    Ok(())
}

/// The card `start` of `kind`, if it's still waiting; the request it came
/// with is refused otherwise, before anything's made.
pub(crate) fn check(factory: &Factory, start: &str, kind: Kind) -> Result<(), RequestError> {
    match row(&factory.store, start.trim()) {
        Some(row) if row.kind == kind && row.state == State::Waiting => Ok(()),
        Some(row) if row.kind == kind => Err(RequestError::Pipe("This card is already closed.".to_owned())),
        _ => Err(RequestError::Pipe("This card no longer exists.".to_owned())),
    }
}

/// The person made the invite, or connected, from card `start`: it shows
/// the pipe, and its agent is told what the person chose (never what the
/// other side says about itself).
pub(crate) fn done(factory: &Arc<Factory>, start: &str, record: &Record, named: bool) {
    let Some(row) = end(factory, start, State::Done, &record.id) else { return };
    let (person, project) = {
        let inner = factory.lock();
        let project = inner.config.project(&record.project).map_or_else(|| record.project.clone(), |project| project.name.clone());
        (inner.config.person_capitalized().to_owned(), project)
    };
    // The name only when this side chose it.
    let pipe = if named { format!("\"{}\"", record.name) } else { "the pipe".to_owned() };
    let summary = match row.kind {
        Kind::Accept => format!("{person} pasted the code and connected {pipe} in {project}"),
        _ => format!("{person} made the invite for {pipe} in {project}"),
    };
    let text = match row.kind {
        Kind::Accept => {
            format!("{summary}: it's connecting, then both people approve it in the pipe. You never see the code or the digits.")
        }
        _ => format!(
            "{summary}: they send its code to the other person themselves. It waits for them to use it; then both people approve the pipe in it."
        ),
    };
    factory.deliver_event(&row.agent, text, host_event(summary));
}

/// DismissPipeStart: the person closes the card; its agent is told.
pub(crate) fn dismiss(factory: &Arc<Factory>, request: &v1::DismissPipeStart) -> Result<(), RequestError> {
    let start = request.start_id.trim();
    match row(&factory.store, start) {
        Some(row) if row.agent == request.agent_id && row.state == State::Waiting => {}
        Some(row) if row.agent == request.agent_id => return Ok(()),
        _ => return Err(RequestError::Pipe("This card no longer exists.".to_owned())),
    }
    let Some(row) = end(factory, start, State::Dismissed, "") else { return Ok(()) };
    let person = factory.lock().config.person_capitalized().to_owned();
    tell_closed(factory, &row, &format!("{person} closed the card"));
    Ok(())
}

fn tell_closed(factory: &Arc<Factory>, row: &Row, summary: &str) {
    let what = if row.kind == Kind::Accept { "connecting a pipe" } else { "making the invite" };
    factory.deliver_event(&row.agent, format!("{summary} for {what}: nothing was made."), host_event(summary.to_owned()));
}

/// Ends a waiting card, shows it as it is now, and returns it; None if it
/// wasn't waiting.
fn end(factory: &Factory, start: &str, state: State, pipe: &str) -> Option<Row> {
    let word = if state == State::Done { "done" } else { "dismissed" };
    let changed = factory
        .store
        .db()
        .execute(
            "UPDATE pipe_starts SET state = ?2, pipe = ?3, ended = ?4 WHERE id = ?1 AND state = 'waiting'",
            params![start, word, pipe, now_ms()],
        )
        .unwrap_or(0);
    if changed == 0 {
        return None;
    }
    let row = row(&factory.store, start)?;
    let mut inner = factory.lock();
    let entry = inner
        .agents
        .get_mut(&row.agent)
        .and_then(|runtime| runtime.transcript.as_mut())
        .and_then(|transcript| transcript.set_pipe_start(row.to_proto()));
    if let Some(entry) = entry {
        factory.send_change(&inner, &row.agent, crate::transcript::Change::Entry(entry));
        // It no longer waits for the person.
        factory.broadcast_workspace(&inner);
    }
    drop(inner);
    Some(row)
}

/// Closes cards left waiting a day, telling their agents; every hour.
pub(crate) async fn expire(factory: Weak<Factory>) {
    loop {
        let Some(strong) = factory.upgrade() else { return };
        expire_before(&strong, now_ms() - WAITS_MS);
        drop(strong);
        tokio::time::sleep(Duration::from_secs(3600)).await;
    }
}

pub(crate) fn expire_before(factory: &Arc<Factory>, before: i64) {
    let ids: Vec<String> = {
        let db = factory.store.db();
        let Ok(mut statement) = db.prepare("SELECT id FROM pipe_starts WHERE state = 'waiting' AND created < ?1") else { return };
        statement.query_map([before], |row| row.get(0)).map(|rows| rows.flatten().collect()).unwrap_or_default()
    };
    for id in ids {
        if let Some(row) = end(factory, &id, State::Dismissed, "") {
            tell_closed(factory, &row, "The card waited a day without being used, so it closed");
        }
    }
}

/// pipes_list: the agent's project's pipes (or every project's), each with
/// what this side chose and where it stands, in Claktory's words: never
/// what the other side says about itself, nor codes or digits.
pub(crate) fn list(factory: &Factory, project: Option<&str>) -> Result<String, String> {
    let (projects, person) = {
        let inner = factory.lock();
        let projects: Vec<(String, String)> =
            inner.config.projects.iter().map(|project| (project.id.clone(), project.name.clone())).collect();
        (projects, inner.config.person().to_owned())
    };
    let wanted = project.map(str::trim).filter(|project| !project.is_empty());
    let projects: Vec<&(String, String)> =
        projects.iter().filter(|(id, name)| wanted.is_none_or(|wanted| id == wanted || name.eq_ignore_ascii_case(wanted))).collect();
    if projects.is_empty() {
        return Err(format!("There's no project \"{}\".", wanted.unwrap_or_default()));
    }
    let mut lines = Vec::new();
    for (id, project_name) in projects {
        for record in factory.pipes.all().into_iter().filter(|record| &record.project == id) {
            lines.push(format!("- {} (id {}), in {project_name}: {}", shown_name(&record), record.id, standing(&record, &person)));
        }
    }
    if lines.is_empty() {
        return Ok("No pipes.".to_owned());
    }
    Ok(lines.join("\n"))
}

/// The pipe's name, when this side chose it; a name the other side gave it
/// (accepted without one of its own) isn't repeated to an agent.
fn shown_name(record: &Record) -> String {
    // By who chose it, never by comparing it with what the other side says
    // now: that can change after the join.
    let theirs = !record.named_here.unwrap_or(record.peer.is_none());
    if theirs { "a pipe named by the other side".to_owned() } else { format!("\"{}\"", record.name) }
}

/// Where a pipe stands, in Claktory's own words.
fn standing(record: &Record, person: &str) -> String {
    use super::{Pause, State as Pipe};
    match (record.state, record.paused) {
        (Pipe::Invited, _) => "its invite waits for the other person to use the code".to_owned(),
        (Pipe::Joining, _) => "connecting".to_owned(),
        (Pipe::Approval, _) => format!("waiting for {person} to approve it"),
        (Pipe::WaitingPeer, _) => "waiting for the other side to approve it".to_owned(),
        (Pipe::Open, _) => "open".to_owned(),
        (Pipe::Paused, Pause::Loop) => format!("paused: the agents talked on their own; {person} can continue it"),
        (Pipe::Paused, Pause::Peer) => "paused by the other side".to_owned(),
        (Pipe::Paused, _) => format!("paused by {person}"),
        (Pipe::Closed, _) => "closed".to_owned(),
    }
}
