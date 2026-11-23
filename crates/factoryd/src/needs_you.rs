//! Needs you's history (ListNeedsYouHistory): the cards the person decided
//! in the last 30 days, newest first, so they can find an agent they
//! answered ("the one about emails") and open its conversation at the card.
//!
//! Read only, over the tables the cards are kept in: asks, proposals,
//! pipe_secret_approvals, pipe_starts, app_grant_proposals and
//! pipe_handoff_changes (where reports go, changed) in factory.db; requests and
//! connect_proposals in vault.db (names, comments and states only, never a
//! value), and pipe_decisions, kept here, for the decisions on a pipe that
//! leave no other trace (a held message let through or discarded, a loop
//! pause continued). Expired, withdrawn and waiting cards aren't listed.
//! What the other side of a pipe wrote never becomes a title: a held
//! message's text goes only in `excerpt`, for the app to frame; an ask
//! keeps its taint, as on its card; a pipe start is named by its kind, and
//! an app grant by its app and agent, never by Rivet's reason. A card /clear
//! closed unseen ('cleared') was never the person's decision, and isn't
//! listed.

use factory_protocol::v1::{self, NeedsYouOutcome as Outcome, NeedsYouRecordKind as Kind, entry::Kind as Entry};
use std::collections::HashMap;

use rusqlite::{Connection, OpenFlags, params};
use serde_json::Value;

use crate::factory::{Factory, RequestError};
use crate::transcript::now_ms;

const DAYS: i64 = 30;
const DEFAULT_LIMIT: u32 = 50;
const MAX_LIMIT: u32 = 200;
const MAX_QUERY: usize = 200;
const MAX_REPLY: usize = 300;
const MAX_TITLE: usize = 200;
const MAX_EXCERPT: usize = 200;

pub(crate) const SCHEMA: &str = "
-- The person's decisions on a pipe that leave no other trace, for Needs
-- you's history: a held message let through or discarded, a loop pause
-- continued.
CREATE TABLE pipe_decisions (
    id INTEGER PRIMARY KEY,
    pipe TEXT NOT NULL,
    -- The held message's seq in the pipe; 0 for a loop pause.
    seq INTEGER NOT NULL,
    -- held (to the other side), held_incoming (from it) or loop
    kind TEXT NOT NULL,
    -- The agent whose message it was; '' for the other side's, and a loop.
    agent TEXT NOT NULL,
    -- 1: let through, or continued.
    allowed INTEGER NOT NULL,
    -- Dropped by NOTES: the person's comment is in decision_notes.
    comment TEXT NOT NULL,
    asked INTEGER NOT NULL,
    decided INTEGER NOT NULL
);
CREATE INDEX pipe_decisions_by_time ON pipe_decisions (decided);
";

/// The person's comment on a pipe decision, kept apart from the pipe
/// tables (Fugo): nothing that forwards to the other side reads outside
/// them, so no pipe* table ever holds the person's words.
pub(crate) const NOTES: &str = "
-- The person's comment with a decision, for Needs you's history, apart
-- from the pipe tables: kind 'pipe_message', ref '<pipe>:<seq>'.
CREATE TABLE decision_notes (
    kind TEXT NOT NULL,
    ref TEXT NOT NULL,
    comment TEXT NOT NULL,
    created INTEGER NOT NULL,
    PRIMARY KEY (kind, ref)
);
INSERT INTO decision_notes (kind, ref, comment, created)
    SELECT 'pipe_message', pipe || ':' || seq, comment, decided FROM pipe_decisions WHERE comment != '' AND seq > 0;
ALTER TABLE pipe_decisions DROP COLUMN comment;
";

/// What the person decided on a pipe (`pipe_decided`).
#[derive(Clone, Copy)]
pub(crate) enum PipeDecision {
    /// An agent's message held before it went to the other side.
    Held,
    /// The other side's message, held before an agent read it.
    HeldIncoming,
    /// Agents went back and forth with no person, and the person let them
    /// carry on.
    Loop,
}

/// Keeps a decision on a pipe for the history.
#[allow(clippy::too_many_arguments)]
pub(crate) fn pipe_decided(
    factory: &Factory,
    pipe: &str,
    seq: u64,
    kind: PipeDecision,
    agent: &str,
    allowed: bool,
    comment: &str,
    asked: i64,
) {
    let kind = match kind {
        PipeDecision::Held => "held",
        PipeDecision::HeldIncoming => "held_incoming",
        PipeDecision::Loop => "loop",
    };
    let now = now_ms();
    let db = factory.store.db();
    let mut kept = db.execute(
        "INSERT INTO pipe_decisions (pipe, seq, kind, agent, allowed, asked, decided) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![pipe, seq as i64, kind, agent, allowed, asked, now],
    );
    if kept.is_ok() && !comment.trim().is_empty() && seq > 0 {
        kept = db.execute(
            "INSERT OR REPLACE INTO decision_notes (kind, ref, comment, created) VALUES ('pipe_message', ?1, ?2, ?3)",
            params![format!("{pipe}:{seq}"), comment.trim(), now],
        );
    }
    if let Err(error) = kept {
        tracing::warn!(%error, pipe, "couldn't keep a decision on a pipe for the history");
    }
}

/// The most cards read from one table for a page; past it, `more` says so.
const MAX_SCAN: u32 = 5000;

/// A decided card, as its table has it.
struct Row {
    kind: Kind,
    /// Its id in its table, as its card names it.
    id: String,
    agent: String,
    title: String,
    reply: String,
    outcome: Outcome,
    asked: i64,
    decided: i64,
    /// An ask's stored taint, as JSON.
    taint: Option<String>,
    pipe: String,
    /// A pipe's: its held message's seq in the pipe, and its text.
    seq: u64,
    excerpt: String,
    /// REPORT_TYPE and PIPE_FORWARD: the report types it's about.
    types: Types,
    /// APP_ACCESS: what History shows of it.
    app: AppAccess,
    /// MODEL: what History shows of it.
    model: ModelChange,
    /// All the text a search looks in, lowercase.
    search: String,
}

/// A change of an app's access (APP_ACCESS), as History shows it.
#[derive(Default)]
struct AppAccess {
    /// The change, to undo it; empty for a card that gave nothing.
    change: String,
    undone: bool,
    app: String,
    /// Rivet's words, cut.
    reason: String,
    /// Its card (AppAccessProposal.id), when it was one.
    card: String,
}

/// A model change an agent made (MODEL), as History shows it.
#[derive(Default)]
struct ModelChange {
    change: String,
    undone: bool,
    /// Whose model it was.
    subject: String,
    /// The changing agent's words, cut.
    reason: String,
}

/// A change of report types, as History shows it.
#[derive(Default)]
struct Types {
    made: Option<v1::ReportSchema>,
    before: Option<v1::ReportSchema>,
    widens: bool,
    /// The types a forward's change turned on.
    enabled: Vec<v1::ReportSchema>,
}

impl Row {
    fn new(kind: Kind, id: String, agent: String, outcome: Outcome, asked: i64, decided: i64) -> Row {
        Row {
            kind,
            id,
            agent,
            title: String::new(),
            reply: String::new(),
            outcome,
            asked,
            decided,
            taint: None,
            pipe: String::new(),
            seq: 0,
            excerpt: String::new(),
            types: Types::default(),
            app: AppAccess::default(),
            model: ModelChange::default(),
            search: String::new(),
        }
    }

    /// What a search looks in: `texts`, as well as its title and reply.
    fn searched(mut self, texts: &[&str]) -> Row {
        let mut search = format!("{}\n{}", self.title, self.reply);
        for text in texts {
            search.push('\n');
            search.push_str(text);
        }
        self.search = search.to_lowercase();
        self
    }
}

/// The cards decided from ?1 (since) to before ?2 (before), at most ?3.
struct Window {
    since: i64,
    before: i64,
}

/// `text`, cut to `max` characters.
fn cut(text: &str, max: usize) -> String {
    let text = text.trim();
    match text.char_indices().nth(max) {
        Some((at, _)) => format!("{}…", &text[..at]),
        None => text.to_owned(),
    }
}

/// The rows of one table: `sql` is bound to ?1 (since), ?2 (before) and ?3
/// (the most); whether it had more than that.
fn rows(
    db: &Connection,
    sql: &str,
    window: &Window,
    make: impl Fn(&rusqlite::Row<'_>) -> rusqlite::Result<Row>,
) -> rusqlite::Result<(Vec<Row>, bool)> {
    let found = crate::store::query(db, sql, params![window.since, window.before, MAX_SCAN + 1], make)?;
    let capped = found.len() > MAX_SCAN as usize;
    Ok((found.into_iter().take(MAX_SCAN as usize).collect(), capped))
}

/// Each table's rows in a window, and, when a table had more than it
/// read, the newest time it may have left out: what's decided after it is
/// all there.
#[derive(Default)]
struct Found {
    rows: Vec<Row>,
    cutoff: Option<i64>,
}

impl Found {
    fn add(&mut self, (rows, capped): (Vec<Row>, bool)) {
        if capped && let Some(oldest) = rows.last() {
            self.cutoff = Some(self.cutoff.map_or(oldest.decided, |cutoff| cutoff.max(oldest.decided)));
        }
        self.rows.extend(rows);
    }
}

/// The rows `keep` keeps, newest first, from `window` back: a page of
/// `limit` at least when there are that many, scanning further back past
/// a table's cap as needed; and whether older ones were left unscanned.
fn gather(
    mut window: Window,
    limit: usize,
    keep: impl Fn(&Row) -> bool,
    mut scan: impl FnMut(&Window) -> rusqlite::Result<Found>,
) -> rusqlite::Result<(Vec<Row>, bool)> {
    let mut kept = Vec::new();
    loop {
        let found = scan(&window)?;
        // A table with more than its cap in a single millisecond: take what
        // was read and go on past it, rather than never moving.
        let stuck = found.cutoff.is_some_and(|cutoff| cutoff + 1 >= window.before);
        let complete = if stuck { None } else { found.cutoff };
        let mut rows: Vec<Row> =
            found.rows.into_iter().filter(|row| complete.is_none_or(|cutoff| row.decided > cutoff) && keep(row)).collect();
        rows.sort_by(|a, b| b.decided.cmp(&a.decided).then_with(|| b.asked.cmp(&a.asked)));
        kept.extend(rows);
        let Some(cutoff) = found.cutoff else { return Ok((kept, false)) };
        if kept.len() >= limit {
            return Ok((kept, true));
        }
        window.before = if stuck { cutoff } else { cutoff + 1 };
    }
}

/// `names`: each agent's name now, by id.
fn from_factory(
    db: &Connection,
    window: &Window,
    names: &HashMap<String, String>,
    projects: &HashMap<String, String>,
    found: &mut Found,
) -> rusqlite::Result<()> {
    found.add(rows(
        db,
        "SELECT id, agent, title, answer, state, created, decided, taint, review, body FROM asks
         WHERE state IN ('answered', 'dismissed') AND decided >= ?1 AND decided < ?2 ORDER BY decided DESC LIMIT ?3",
        window,
        |row| {
            let review: String = row.get(8)?;
            let kind = if review.is_empty() { Kind::Question } else { Kind::Review };
            let outcome = if row.get::<_, String>(4)? == "answered" { Outcome::Answered } else { Outcome::Dismissed };
            let mut found = Row::new(kind, row.get::<_, i64>(0)?.to_string(), row.get(1)?, outcome, row.get(5)?, row.get(6)?);
            found.title = cut(&row.get::<_, String>(2)?, MAX_TITLE);
            found.reply = cut(&row.get::<_, String>(3)?, MAX_REPLY);
            found.taint = row.get(7)?;
            Ok(found.searched(&[&row.get::<_, String>(9)?, &row.get::<_, String>(3)?]))
        },
    )?);
    found.add(rows(
        db,
        "SELECT id, agent, text, comment, state, created, decided FROM proposals
         WHERE state IN ('approved', 'dismissed') AND decided >= ?1 AND decided < ?2 ORDER BY decided DESC LIMIT ?3",
        window,
        |row| {
            let text: String = row.get(2)?;
            let outcome = if row.get::<_, String>(4)? == "approved" { Outcome::Approved } else { Outcome::Dismissed };
            let mut found = Row::new(Kind::Proposal, row.get::<_, i64>(0)?.to_string(), row.get(1)?, outcome, row.get(5)?, row.get(6)?);
            found.title = cut(text.lines().find(|line| !line.trim().is_empty()).unwrap_or_default(), MAX_TITLE);
            found.reply = cut(&row.get::<_, String>(3)?, MAX_REPLY);
            Ok(found.searched(&[&text, &row.get::<_, String>(3)?]))
        },
    )?);
    found.add(rows(
        db,
        "SELECT id, agent, secrets, state, created, decided, command FROM pipe_secret_approvals
         WHERE state IN ('allowed', 'denied') AND decided >= ?1 AND decided < ?2 ORDER BY decided DESC LIMIT ?3",
        window,
        |row| {
            let names: Vec<String> = serde_json::from_str(&row.get::<_, String>(2)?).unwrap_or_default();
            let outcome = if row.get::<_, String>(3)? == "allowed" { Outcome::Approved } else { Outcome::Denied };
            let mut found = Row::new(Kind::SecretUse, row.get::<_, i64>(0)?.to_string(), row.get(1)?, outcome, row.get(4)?, row.get(5)?);
            found.title = cut(&names.join(", "), MAX_TITLE);
            Ok(found.searched(&[&row.get::<_, String>(6)?]))
        },
    )?);
    found.add(rows(
        db,
        "SELECT id, agent, kind, state, pipe, created, ended, name, purpose FROM pipe_starts
         WHERE state IN ('done', 'dismissed') AND ended >= ?1 AND ended < ?2 ORDER BY ended DESC LIMIT ?3",
        window,
        |row| {
            let done = row.get::<_, String>(3)? == "done";
            let outcome = if done { Outcome::Approved } else { Outcome::Dismissed };
            let mut found = Row::new(Kind::PipeStart, row.get(0)?, row.get(1)?, outcome, row.get(5)?, row.get(6)?);
            found.title = if row.get::<_, String>(2)? == "accept" { "Accept a pipe invite" } else { "Start a pipe" }.to_owned();
            if done {
                found.pipe = row.get(4)?;
            }
            Ok(found.searched(&[&row.get::<_, String>(7)?, &row.get::<_, String>(8)?]))
        },
    )?);
    found.add(rows(
        db,
        "SELECT d.agent, d.kind, d.allowed, d.asked, d.decided, d.pipe, d.seq, COALESCE(m.text, ''), d.id, COALESCE(n.comment, '')
         FROM pipe_decisions d LEFT JOIN pipe_messages m ON d.seq > 0 AND m.pipe = d.pipe AND m.seq = d.seq
         LEFT JOIN decision_notes n ON d.seq > 0 AND n.kind = 'pipe_message' AND n.ref = d.pipe || ':' || d.seq
         WHERE d.decided >= ?1 AND d.decided < ?2 ORDER BY d.decided DESC LIMIT ?3",
        window,
        |row| {
            let kind: String = row.get(1)?;
            let (kind, title) = match kind.as_str() {
                "held" => (Kind::PipeHeld, "Your agent's message to the other side"),
                "held_incoming" => (Kind::PipeHeld, "A message from the other side"),
                _ => (Kind::PipeLoop, "Your agents and theirs kept replying to each other"),
            };
            let outcome = if row.get::<_, bool>(2)? { Outcome::Approved } else { Outcome::Denied };
            let mut found = Row::new(kind, row.get::<_, i64>(8)?.to_string(), row.get(0)?, outcome, row.get(3)?, row.get(4)?);
            found.title = title.to_owned();
            found.reply = cut(&row.get::<_, String>(9)?, MAX_REPLY);
            found.pipe = row.get(5)?;
            found.seq = row.get::<_, i64>(6)?.try_into().unwrap_or_default();
            let text: String = row.get(7)?;
            found.excerpt = cut(&text, MAX_EXCERPT);
            Ok(found.searched(&[&text]))
        },
    )?);
    found.add(rows(
        db,
        "SELECT id, agent, grantee, app, state, created, decided, comment, reason FROM app_grant_proposals
         WHERE state IN ('granted', 'declined', 'failed') AND decided >= ?1 AND decided < ?2 ORDER BY decided DESC LIMIT ?3",
        window,
        |row| {
            let outcome = match row.get::<_, String>(4)?.as_str() {
                "granted" => Outcome::Approved,
                "declined" => Outcome::Denied,
                _ => Outcome::Failed,
            };
            let mut found = Row::new(Kind::AppGrant, row.get(0)?, row.get(1)?, outcome, row.get(5)?, row.get(6)?);
            let grantee: String = row.get(2)?;
            let grantee = names.get(&grantee).cloned().unwrap_or(grantee);
            let app: String = row.get(3)?;
            let app = crate::user_apps::app(&app).map_or(app, |known| known.name.to_owned());
            found.title = cut(&format!("Give {grantee} access to {app}"), MAX_TITLE);
            found.reply = cut(&row.get::<_, String>(7)?, MAX_REPLY);
            Ok(found.searched(&[&row.get::<_, String>(7)?, &row.get::<_, String>(8)?]))
        },
    )?);
    found.add(rows(
        db,
        "SELECT id, kind, target, old, new, by, at, kept FROM pipe_handoff_changes
         WHERE at >= ?1 AND at < ?2 ORDER BY at DESC, id DESC LIMIT ?3",
        window,
        |row| {
            let (kind, target, old, new, by): (String, String, String, String, String) =
                (row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?, row.get(5)?);
            let at: i64 = row.get(6)?;
            let id = format!("forward:{}", row.get::<_, i64>(0)?);
            let change = Change { kind: &kind, target: &target, old: &old, new: &new, by: &by, kept: row.get(7)? };
            if kind == "type" {
                let mut found = Row::new(Kind::ReportType, id, by.clone(), Outcome::Changed, at, at);
                let version = |text: &str| {
                    text.parse::<u32>().ok().and_then(|version| crate::pipes::handoff::types::load_version(db, &target, version))
                };
                let (made, before) = (version(&new), version(&old));
                found.title = cut(&type_title(&change, made.as_ref(), before.as_ref(), names), MAX_TITLE);
                let searched: Vec<String> = made.iter().chain(before.iter()).map(|schema| schema.name.clone()).collect();
                let proto = |schema: &crate::pipes::handoff::schema::Schema| crate::pipes::handoff::schema::to_proto(schema);
                found.types = Types {
                    widens: !new.is_empty() && made.as_ref().is_some_and(|made| made.widens_words()),
                    // A deletion shows the version it ended at.
                    made: made.as_ref().or(before.as_ref()).map(proto),
                    before: if new.is_empty() { None } else { before.as_ref().map(proto) },
                    enabled: Vec::new(),
                };
                let searched: Vec<&str> = searched.iter().map(String::as_str).collect();
                return Ok(found.searched(&searched));
            }
            let mut found = Row::new(Kind::PipeForward, id, by.clone(), Outcome::Changed, at, at);
            if let Some(place) = kind.strip_suffix("_types") {
                let listed = |text: &str| -> Vec<crate::pipes::handoff::schema::Schema> {
                    let kept: Vec<(String, u32)> = serde_json::from_str(text).unwrap_or_default();
                    kept.into_iter().filter_map(|(id, version)| crate::pipes::handoff::types::load_version(db, &id, version)).collect()
                };
                let (before, now) = (listed(&old), listed(&new));
                let added: Vec<_> = now.iter().filter(|schema| !before.iter().any(|old| old.id == schema.id)).collect();
                let removed: Vec<_> = before.iter().filter(|schema| !now.iter().any(|new| new.id == schema.id)).collect();
                found.title = cut(&types_title(&change, place, &added, &removed, names, projects), MAX_TITLE);
                found.types.enabled = added.iter().map(|schema| crate::pipes::handoff::schema::to_proto(schema)).collect();
                if place == "pipe" {
                    found.pipe = target;
                }
                return Ok(found.searched(&[]));
            }
            found.title = cut(&forward_title(&change, names, projects), MAX_TITLE);
            if kind == "pipe" {
                found.pipe = target;
            }
            Ok(found.searched(&[]))
        },
    )?);
    from_app_access(db, window, names, found)?;
    from_model_changes(db, window, names, found)?;
    Ok(())
}

/// The model changes agents made with models_set
/// (`extensions::models`): Claktory's sentence, the agent's reason apart.
fn from_model_changes(db: &Connection, window: &Window, names: &HashMap<String, String>, found: &mut Found) -> rusqlite::Result<()> {
    let changes = crate::extensions::models::between(db, window.since, window.before, i64::from(MAX_SCAN) + 1)?;
    let capped = changes.len() > MAX_SCAN as usize;
    let rows = changes
        .into_iter()
        .take(MAX_SCAN as usize)
        .map(|change| {
            let mut row = Row::new(Kind::Model, change.id.to_string(), change.by.clone(), Outcome::Changed, change.at, change.at);
            row.title = cut(&change.title(names), MAX_TITLE);
            row.model = ModelChange {
                change: change.id.to_string(),
                undone: change.undone.is_some(),
                subject: change.agent.clone(),
                reason: cut(&change.reason, MAX_REPLY),
            };
            row.searched(&[&change.reason, &change.model_after, &change.name_after])
        })
        .collect();
    found.add((rows, capped));
    Ok(())
}

/// Changes to what an app reads and who uses it (apps::changes), and
/// Rivet's cards for them that the person declined or that failed: titles
/// are Claktory's sentences, and Rivet's reason stays apart, as its words.
fn from_app_access(db: &Connection, window: &Window, names: &HashMap<String, String>, found: &mut Found) -> rusqlite::Result<()> {
    let names: std::collections::BTreeMap<String, String> = names.iter().map(|(id, name)| (id.clone(), name.clone())).collect();
    let changes = crate::apps::changes::between(db, window.since, window.before, i64::from(MAX_SCAN) + 1)?;
    let capped = changes.len() > MAX_SCAN as usize;
    let mut rows = Vec::new();
    for change in changes.into_iter().take(MAX_SCAN as usize) {
        let card = change.via == crate::apps::changes::Via::Card;
        // A card's change shows in Rivet's conversation, at its card.
        let (agent, asked) = if card {
            db.query_row("SELECT agent, created FROM app_access_proposals WHERE id = ?1", [&change.reference], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
            })
            .unwrap_or_else(|_| (String::new(), change.at))
        } else {
            (change.by.clone(), change.at)
        };
        let outcome = if card { Outcome::Approved } else { Outcome::Changed };
        let mut row = Row::new(Kind::AppAccess, change.id.to_string(), agent, outcome, asked, change.at);
        row.title = cut(&change.title(&names), MAX_TITLE);
        row.app = AppAccess {
            change: change.id.to_string(),
            undone: change.undone_by.is_some(),
            app: change.app.clone(),
            reason: cut(&change.reason, MAX_REPLY),
            card: if card { change.reference.clone() } else { String::new() },
        };
        rows.push(row.searched(&[&change.labels.app, &change.reason]));
    }
    found.add((rows, capped));
    found.add(self::rows(
        db,
        "SELECT id, agent, app, card, state, created, decided, comment FROM app_access_proposals
         WHERE state IN ('declined', 'failed') AND decided >= ?1 AND decided < ?2 ORDER BY decided DESC LIMIT ?3",
        window,
        |row| {
            let outcome = if row.get::<_, String>(4)? == "declined" { Outcome::Denied } else { Outcome::Failed };
            let id: String = row.get(0)?;
            let mut found = Row::new(Kind::AppAccess, id.clone(), row.get(1)?, outcome, row.get(5)?, row.get(6)?);
            let card: Vec<u8> = row.get(3)?;
            let card = <v1::AppAccessProposal as prost::Message>::decode(card.as_slice()).unwrap_or_default();
            found.title = cut(&crate::apps::access::title(&card, &names), MAX_TITLE);
            found.reply = cut(&row.get::<_, String>(7)?, MAX_REPLY);
            found.app = AppAccess { app: row.get(2)?, reason: cut(&card.reason, MAX_REPLY), card: id, ..AppAccess::default() };
            Ok(found.searched(&[&card.app_name, &card.reason]))
        },
    )?);
    Ok(())
}

/// A row of pipe_handoff_changes.
struct Change<'a> {
    /// pipe, project or reader.
    kind: &'a str,
    target: &'a str,
    old: &'a str,
    new: &'a str,
    /// "" for the person, else the admin agent's id.
    by: &'a str,
    /// reader: its conversation was kept; pipe or project: the recipient was
    /// deleted, so no one, and `old` is the name it had (its id when
    /// Claktory restarted since).
    kept: bool,
}

/// Claktory's sentence for a change of where reports go: "Rivet set this
/// pipe's reports to go to Ingrid instead of Kim". Never a pipe's name (the
/// app names the pipe, as it names pipes), only agents' and projects' names,
/// which are this side's.
fn forward_title(change: &Change<'_>, names: &HashMap<String, String>, projects: &HashMap<String, String>) -> String {
    let name = |id: &str| names.get(id).cloned().unwrap_or_else(|| id.to_owned());
    let who = if change.by.is_empty() { "You".to_owned() } else { name(change.by) };
    let to = if change.new.is_empty() { "no one".to_owned() } else { name(change.new) };
    let instead = if change.old.is_empty() { String::new() } else { format!(" instead of {}", name(change.old)) };
    let project = || projects.get(change.target).cloned().unwrap_or_else(|| change.target.to_owned());
    match change.kind {
        "pipe" if change.kept => format!("{} was deleted, so this pipe's reports go to no one", change.old),
        "pipe" => format!("{who} set this pipe's reports to go to {to}{instead}"),
        "project" if change.kept => format!("{} was deleted, so {}'s new pipes forward reports to no one", change.old, project()),
        "project" => format!("{who} set {}'s new pipes to forward reports to {to}{instead}", project()),
        _ if change.new.is_empty() => format!("{who} let {} stop reading reports", name(change.target)),
        _ if change.kept => {
            format!("{who} let {} read reports ({}'s earlier conversation is kept)", name(change.target), name(change.target))
        }
        _ => format!("{who} let {} read reports, starting a fresh conversation", name(change.target)),
    }
}

/// "\"Task request\" and \"Question\"": the names their authors wrote.
fn type_names(schemas: &[&crate::pipes::handoff::schema::Schema]) -> String {
    let names: Vec<String> = schemas.iter().map(|schema| format!("\u{201C}{}\u{201D}", schema.name)).collect();
    match names.as_slice() {
        [] => String::new(),
        [one] => one.clone(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}

/// Claktory's sentence for a report type made, changed or deleted: "Rivet
/// changed the report type \u{201C}Recipe\u{201D}". Its name is the person's or
/// an untainted admin agent's words, never the other side's.
fn type_title(
    change: &Change<'_>,
    made: Option<&crate::pipes::handoff::schema::Schema>,
    before: Option<&crate::pipes::handoff::schema::Schema>,
    names: &HashMap<String, String>,
) -> String {
    let who = if change.by.is_empty() { "You".to_owned() } else { names.get(change.by).cloned().unwrap_or_else(|| change.by.to_owned()) };
    let name = |schema: Option<&crate::pipes::handoff::schema::Schema>| {
        schema.map_or_else(|| change.target.to_owned(), |schema| type_names(&[schema]))
    };
    match (change.old.is_empty(), change.new.is_empty()) {
        (true, _) => format!("{who} made the report type {}", name(made)),
        (false, true) => format!("{who} deleted the report type {}", name(before)),
        _ if made.zip(before).is_some_and(|(made, before)| made.name != before.name) => {
            let only_renamed =
                made.zip(before).is_some_and(|(made, before)| made.fields == before.fields && made.outcomes == before.outcomes);
            if only_renamed {
                format!("{who} renamed the report type {} to {}", name(before), name(made))
            } else {
                format!("{who} changed the report type {} and renamed it {}", name(before), name(made))
            }
        }
        _ => format!("{who} changed the report type {}", name(made)),
    }
}

/// Claktory's sentence for a change of the report types a pipe (`place`
/// "pipe") or a project's new pipes can forward.
fn types_title(
    change: &Change<'_>,
    place: &str,
    added: &[&crate::pipes::handoff::schema::Schema],
    removed: &[&crate::pipes::handoff::schema::Schema],
    names: &HashMap<String, String>,
    projects: &HashMap<String, String>,
) -> String {
    let who = if change.by.is_empty() { "You".to_owned() } else { names.get(change.by).cloned().unwrap_or_else(|| change.by.to_owned()) };
    let place = if place == "pipe" {
        "this pipe".to_owned()
    } else {
        format!("{}'s new pipes", projects.get(change.target).cloned().unwrap_or_else(|| change.target.to_owned()))
    };
    match (added.is_empty(), removed.is_empty()) {
        (false, true) => format!("{who} let {place} forward {}", type_names(added)),
        (true, false) => format!("{who} stopped {place} forwarding {}", type_names(removed)),
        (false, false) => format!("{who} let {place} forward {} instead of {}", type_names(added), type_names(removed)),
        (true, true) => format!("{who} changed the order of the report types {place} forwards"),
    }
}

fn from_vault(db: &Connection, window: &Window, found: &mut Found) -> rusqlite::Result<()> {
    found.add(rows(
        db,
        "SELECT id, agent, secret_name, new_secret, comment, state, created, MAX(decided, approved) AS at, reason FROM requests
         WHERE state IN ('granted', 'generated', 'approved', 'dismissed') AND at >= ?1 AND at < ?2 ORDER BY at DESC LIMIT ?3",
        window,
        |row| {
            let mut name: String = row.get(2)?;
            if name.is_empty() {
                let new: Value = serde_json::from_str(&row.get::<_, String>(3)?).unwrap_or_default();
                name = new.get("name").and_then(Value::as_str).unwrap_or_default().to_owned();
            }
            let outcome = if row.get::<_, String>(5)? == "dismissed" { Outcome::Dismissed } else { Outcome::Approved };
            let mut found = Row::new(Kind::SecretRequest, row.get(0)?, row.get(1)?, outcome, row.get(6)?, row.get(7)?);
            found.title = cut(&name, MAX_TITLE);
            found.reply = cut(&row.get::<_, String>(4)?, MAX_REPLY);
            Ok(found.searched(&[&row.get::<_, String>(8)?]))
        },
    )?);
    found.add(rows(
        db,
        "SELECT id, agent, connector, purpose, comment, state, created, decided, reason FROM connect_proposals
         WHERE state IN ('connected', 'dismissed') AND decided >= ?1 AND decided < ?2 ORDER BY decided DESC LIMIT ?3",
        window,
        |row| {
            let (connector, purpose): (String, String) = (row.get(2)?, row.get(3)?);
            let outcome = if row.get::<_, String>(5)? == "connected" { Outcome::Approved } else { Outcome::Dismissed };
            let mut found = Row::new(Kind::Connect, row.get(0)?, row.get(1)?, outcome, row.get(6)?, row.get(7)?);
            found.title = cut(if purpose.trim().is_empty() { &connector } else { &purpose }, MAX_TITLE);
            found.reply = cut(&row.get::<_, String>(4)?, MAX_REPLY);
            Ok(found.searched(&[&connector, &row.get::<_, String>(8)?]))
        },
    )?);
    Ok(())
}

/// The card's entry in its conversation as loaded now, if it's there: its
/// seq, and its taint.
fn card(entries: &[v1::Entry], row: &Row) -> Option<(u64, Vec<v1::PipeTaint>)> {
    entries.iter().rev().find_map(|entry| {
        let taint = match (&entry.kind, row.kind) {
            (Some(Entry::AskPerson(ask)), Kind::Question | Kind::Review) if ask.id == row.id => ask.taint.clone(),
            (Some(Entry::Proposal(proposal)), Kind::Proposal) if proposal.id == row.id => proposal.taint.clone(),
            (Some(Entry::SecretRequest(request)), Kind::SecretRequest) if request.id == row.id => Vec::new(),
            (Some(Entry::ConnectProposal(proposal)), Kind::Connect) if proposal.id == row.id => Vec::new(),
            (Some(Entry::SecretApproval(approval)), Kind::SecretUse) if approval.id == row.id => approval.taint.clone(),
            (Some(Entry::PipeStart(start)), Kind::PipeStart) if start.id == row.id => Vec::new(),
            (Some(Entry::AppGrant(proposal)), Kind::AppGrant) if proposal.id == row.id => Vec::new(),
            (Some(Entry::AppAccess(proposal)), Kind::AppAccess) if !row.app.card.is_empty() && proposal.id == row.app.card => Vec::new(),
            (Some(Entry::ToolCall(call)), Kind::AppAccess)
                if row.app.card.is_empty()
                    && !row.app.change.is_empty()
                    && crate::apps::access::change_of(call).as_deref() == Some(row.app.change.as_str()) =>
            {
                Vec::new()
            }
            (Some(Entry::ToolCall(call)), Kind::Model)
                if crate::extensions::models::change_of(call).as_deref() == Some(row.model.change.as_str()) =>
            {
                Vec::new()
            }
            _ => return None,
        };
        Some((entry.seq, taint))
    })
}

/// The end of a page of `limit` from `rows` (sorted, newest first): never
/// between two decided in the same millisecond, so paging by `before_ms`
/// skips none.
fn page_end(rows: &[Row], limit: usize) -> usize {
    let mut end = limit.min(rows.len());
    while end > 0 && end < rows.len() && rows[end].decided == rows[end - 1].decided {
        end += 1;
    }
    end
}

impl Factory {
    /// ListNeedsYouHistory.
    pub(crate) fn needs_you_history(&self, request: &v1::ListNeedsYouHistory) -> Result<v1::NeedsYouHistory, RequestError> {
        let query: String = request.query.trim().chars().take(MAX_QUERY).collect::<String>().to_lowercase();
        let limit = match request.limit {
            0 => DEFAULT_LIMIT,
            limit => limit.min(MAX_LIMIT),
        } as usize;
        let (names, projects): (HashMap<String, String>, HashMap<String, String>) = {
            let inner = self.lock();
            (
                inner.config.agents.iter().map(|agent| (agent.id.clone(), agent.name.clone())).collect(),
                inner.config.projects.iter().map(|project| (project.id.clone(), project.name.clone())).collect(),
            )
        };
        let agents: Vec<String> = if query.is_empty() {
            Vec::new()
        } else {
            names.iter().filter(|(_, name)| name.to_lowercase().contains(&query)).map(|(id, _)| id.clone()).collect()
        };
        let window = Window {
            since: now_ms() - DAYS * 24 * 60 * 60 * 1000,
            before: if request.before_ms > 0 { request.before_ms } else { i64::MAX },
        };
        let vault = self.home.root().join("vault/vault.db");
        let vault = if vault.exists() {
            let db = Connection::open_with_flags(&vault, OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX)?;
            db.busy_timeout(std::time::Duration::from_secs(5))?;
            Some(db)
        } else {
            None
        };
        let keep = |row: &Row| query.is_empty() || row.search.contains(&query) || agents.contains(&row.agent);
        let (mut rows, unscanned) = gather(window, limit, keep, |window| {
            let mut found = Found::default();
            from_factory(&self.store.db(), window, &names, &projects, &mut found)?;
            if let Some(db) = &vault {
                from_vault(db, window, &mut found)?;
            }
            Ok(found)
        })?;
        let end = page_end(&rows, limit);
        let more = rows.len() > end || unscanned;
        rows.truncate(end);

        let inner = self.lock();
        let records = rows
            .into_iter()
            .map(|row| {
                let chat = crate::chats::is_chat(&row.agent);
                let name = if chat {
                    String::new()
                } else {
                    inner.config.agent(&row.agent).map_or_else(|| row.agent.clone(), |agent| agent.name.clone())
                };
                let entries =
                    inner.agents.get(&row.agent).and_then(|runtime| runtime.transcript.as_ref()).map(|transcript| transcript.entries());
                let piped = matches!(row.kind, Kind::PipeHeld | Kind::PipeLoop | Kind::PipeForward);
                let (seq, mut taint) =
                    if piped { (row.seq, Vec::new()) } else { entries.and_then(|entries| card(entries, &row)).unwrap_or_default() };
                if taint.is_empty()
                    && let Some(stored) = row.taint.as_deref().and_then(|text| serde_json::from_str::<Value>(text).ok())
                {
                    taint = crate::pipes::agents::routes_from_json(&stored);
                }
                v1::NeedsYouRecord {
                    kind: row.kind.into(),
                    agent_id: if chat { String::new() } else { row.agent.clone() },
                    agent_name: name,
                    chat_id: if chat { row.agent.clone() } else { String::new() },
                    pipe_id: row.pipe,
                    seq,
                    title: row.title,
                    excerpt: row.excerpt,
                    outcome: row.outcome.into(),
                    reply: row.reply,
                    asked_ms: row.asked,
                    decided_ms: row.decided,
                    taint,
                    report_type: row.types.made,
                    report_type_before: row.types.before,
                    widens: row.types.widens,
                    report_types: row.types.enabled,
                    app_change_id: row.app.change,
                    undone: row.app.undone || row.model.undone,
                    app_id: row.app.app,
                    agent_reason: if row.kind == Kind::Model { row.model.reason } else { row.app.reason },
                    model_change_id: row.model.change,
                    subject_agent_id: row.model.subject,
                }
            })
            .collect();
        Ok(v1::NeedsYouHistory { records, more })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn decided(at: &[i64]) -> Vec<Row> {
        at.iter().map(|&at| Row::new(Kind::Question, at.to_string(), String::new(), Outcome::Answered, at, at)).collect()
    }

    #[test]
    fn a_page_never_parts_cards_decided_together_and_answers_are_cut() {
        assert_eq!(page_end(&decided(&[9, 8, 7, 6]), 2), 2);
        assert_eq!(page_end(&decided(&[9, 8, 8, 8, 7]), 2), 4);
        assert_eq!(page_end(&decided(&[8, 8, 8]), 1), 3);
        assert_eq!(page_end(&decided(&[9]), 5), 1);
        assert_eq!(page_end(&decided(&[]), 5), 0);
        assert_eq!(cut("  short ", 10), "short");
        assert_eq!(cut("ééééé", 3), "ééé…");
    }

    /// Two tables read at most `cap` rows each, newest first, as `rows` does.
    fn scan(tables: &[Vec<i64>], cap: usize, window: &Window) -> Found {
        let mut found = Found::default();
        for table in tables {
            let mut rows: Vec<Row> =
                decided(table).into_iter().filter(|row| row.decided >= window.since && row.decided < window.before).collect();
            let capped = rows.len() > cap;
            rows.truncate(cap);
            found.add((rows, capped));
        }
        found
    }

    fn times(rows: &[Row]) -> Vec<i64> {
        rows.iter().map(|row| row.decided).collect()
    }

    #[test]
    fn a_table_past_its_cap_never_hides_older_cards_of_another() {
        let tables = [vec![10, 9, 8, 7, 6], vec![9, 5, 1]];
        let all = |_: &Row| true;
        let gathered = |window: Window, limit, keep: &dyn Fn(&Row) -> bool| {
            gather(window, limit, keep, |window| Ok(scan(&tables, 2, window))).expect("gathered")
        };
        // The first table read only 10 and 9: the second's 5 waits for a
        // page that has read the first's 8, 7 and 6.
        let (rows, unscanned) = gathered(Window { since: 0, before: i64::MAX }, 2, &all);
        assert_eq!((times(&rows), unscanned), (vec![10, 9, 9], true));
        let (rows, unscanned) = gathered(Window { since: 0, before: 9 }, 50, &all);
        assert_eq!((times(&rows), unscanned), (vec![8, 7, 6, 5, 1], false));
        // Scanning goes on until a page has its share of what a search keeps.
        let (rows, unscanned) = gathered(Window { since: 0, before: i64::MAX }, 1, &|row: &Row| row.decided < 6);
        assert_eq!((times(&rows), unscanned), (vec![5, 1], false));
        // More than the cap in one millisecond still moves on.
        let tables = [vec![4, 4, 4, 2]];
        let (rows, unscanned) = gather(Window { since: 0, before: 5 }, 9, all, |window| Ok(scan(&tables, 2, window))).expect("gathered");
        assert_eq!((times(&rows), unscanned), (vec![4, 4, 2], false));
    }

    #[test]
    fn a_report_types_title_says_what_happened_to_it() {
        let names = HashMap::from([("rivet".to_owned(), "Rivet".to_owned())]);
        let recipe = crate::pipes::handoff::schema::builtins().remove(1);
        let renamed = crate::pipes::handoff::schema::Schema { name: "Recipe".into(), version: 2, ..recipe.clone() };
        let mut changed = renamed.clone();
        changed.fields.pop();
        let change = |old: &'static str, new: &'static str, by: &'static str| Change {
            kind: "type",
            target: "task_request",
            old,
            new,
            by,
            kept: false,
        };
        let title = |change: Change<'_>, made, before| type_title(&change, made, before, &names);
        assert_eq!(title(change("", "1", ""), Some(&recipe), None), "You made the report type \u{201C}Task request\u{201D}");
        assert_eq!(
            title(change("1", "2", "rivet"), Some(&renamed), Some(&recipe)),
            "Rivet renamed the report type \u{201C}Task request\u{201D} to \u{201C}Recipe\u{201D}"
        );
        assert_eq!(
            title(change("1", "2", ""), Some(&changed), Some(&recipe)),
            "You changed the report type \u{201C}Task request\u{201D} and renamed it \u{201C}Recipe\u{201D}"
        );
        assert_eq!(title(change("2", "", ""), None, Some(&renamed)), "You deleted the report type \u{201C}Recipe\u{201D}");
    }
}
