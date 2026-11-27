//! Needs you's history (ListNeedsYouHistory): the cards the person decided
//! in the last 30 days, newest first, so they can find an agent they
//! answered ("the one about emails") and open its conversation at the card.
//!
//! Read only, over the tables the cards are kept in: asks, proposals,
//! pipe_secret_approvals, pipe_starts, app_grant_proposals and
//! pipe_handoff_changes (where reports go, changed) and pipe_member_changes
//! (who only sends to a pipe) in factory.db; requests and
//! connect_proposals in vault.db (names, comments and states only, never a
//! value), and pipe_decisions, kept here, for the decisions on a pipe that
//! leave no other trace (a held message let through or discarded, a loop
//! pause continued). Expired and waiting cards aren't listed, nor
//! withdrawn requests for secrets; questions and proposals their agents
//! withdrew are, with the agent's reason.
//! What the other side of a pipe wrote never becomes a title: a held
//! message's text goes only in `excerpt`, for the app to frame; an ask
//! keeps its taint, as on its card; a pipe start is named by its kind, and
//! an app grant by its app and agent, never by Rivet's reason. A card /clear
//! closed unseen ('cleared') was never the person's decision, and isn't
//! listed.

use factory_protocol::v1::{self, NeedsYouOutcome as Outcome, NeedsYouRecordKind as Kind, entry::Kind as Entry};
use std::collections::{BTreeSet, HashMap};

use rusqlite::{Connection, OpenFlags, OptionalExtension as _, params};
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
/// tables: nothing that forwards to the other side reads outside
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

/// The person's comment kept in decision_notes for `kind` and `reference`;
/// '' for none.
pub(crate) fn note_of(factory: &Factory, kind: &str, reference: &str) -> String {
    factory
        .store
        .db()
        .query_row("SELECT comment FROM decision_notes WHERE kind = ?1 AND ref = ?2", params![kind, reference], |row| row.get(0))
        .optional()
        .ok()
        .flatten()
        .unwrap_or_default()
}

/// Keeps the person's comment on a decision for `kind` and `reference`,
/// apart from the pipe tables.
pub(crate) fn keep_note(factory: &Factory, kind: &str, reference: &str, comment: &str) {
    let kept = factory.store.db().execute(
        "INSERT OR REPLACE INTO decision_notes (kind, ref, comment, created) VALUES (?1, ?2, ?3, ?4)",
        params![kind, reference, comment.trim(), now_ms()],
    );
    if let Err(error) = kept {
        tracing::warn!(%error, kind, "couldn't keep the person's comment on a decision");
    }
}

/// What tells an agent the person dismissed its card (a question, a
/// proposal, a held pipe message) with nothing to say about it: an event
/// from Claktory, sent quietly, as agents_send's `quiet` is. It waits for
/// the agent's next wake rather than waking it, since often the card was
/// settled elsewhere (in its chat, say), and the agent still reads it
/// then, so it doesn't ask again. A dismissal with the person's words, an
/// answer or an approval wakes it as before.
pub(crate) fn dismissed_event(summary: String) -> factory_agent::Event {
    let mut event = crate::factory::host_event(summary);
    event.provenance = Some(factory_agent::Provenance { quiet: true, ..Default::default() });
    event
}

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
    /// CONVERSATION: what History shows of it.
    conversation: ConversationChange,
    /// PIPE_MEMBER: the agent let send to the pipe, or stopped. GIVEN_UP:
    /// the agent it no longer messages.
    member: String,
    /// WITHDRAWN and SECRET_GIVEN_UP: the agent's reason, cut;
    /// SECRET_SAVED: the secret's description.
    reason: String,
    /// QUESTION, REVIEW and PROPOSAL: how many times its agent changed it.
    updates: u32,
    /// APP_ACCESS and PROPOSAL_GRANT made on an approved proposal: its id,
    /// and its agent.
    proposal: String,
    proposer: String,
    /// ALERT: the alert as kept, and where it was read.
    alert: Option<v1::Alert>,
    on: Option<v1::AnsweredOn>,
    /// QUESTION and REVIEW with steps: what History says of them.
    procedure: Procedure,
    /// SECRET_REQUEST: how it was approved.
    secret: SecretOutcome,
    /// COMMAND: the card's command, with how it ran.
    command: Option<v1::AskCommand>,
    /// QUESTION from an errand's handoff: what it was, whether its action
    /// failed, and the secret its recovery codes were kept as.
    handoff: Option<v1::ErrandHandoff>,
    action_error: String,
    codes_kept_as: String,
    /// All the text a search looks in, lowercase.
    search: String,
}

/// A procedure's outcome, as History shows it: the step it's about
/// (1 for the first; 0 without steps), how many there are, and whether its
/// agent stopped it there after Can't do this.
#[derive(Default)]
struct Procedure {
    step: u32,
    steps: u32,
    stopped: bool,
}

impl Procedure {
    /// From an ask's stored steps (JSON, or none), whether the person
    /// replied, and whether it was resolved.
    fn of(steps: Option<&str>, replied: bool, resolved: bool) -> Procedure {
        let steps: Vec<Value> = steps.and_then(|steps| serde_json::from_str(steps).ok()).unwrap_or_default();
        let state = |step: &Value| step.get("state").and_then(Value::as_str).unwrap_or_default().to_owned();
        let count = u32::try_from(steps.len()).unwrap_or(u32::MAX);
        if count == 0 {
            return Procedure::default();
        }
        let cant = steps.iter().position(|step| state(step) == "cant");
        // Their words on a step they did: the one done last.
        let replied_on = || {
            steps
                .iter()
                .enumerate()
                .filter(|(_, step)| state(step) == "done")
                .max_by_key(|(_, step)| step.get("decided").and_then(Value::as_i64).unwrap_or_default())
                .map(|(at, _)| at)
        };
        let at = cant
            .or_else(|| if replied { replied_on() } else { None })
            .or_else(|| steps.iter().position(|step| state(step).is_empty()))
            .unwrap_or(steps.len() - 1);
        Procedure { step: u32::try_from(at + 1).unwrap_or(u32::MAX), steps: count, stopped: resolved && cant.is_some() }
    }
}

/// How a secret request was approved, as History shows it.
#[derive(Default)]
struct SecretOutcome {
    outcome: v1::NeedsYouSecretOutcome,
    /// The secret it got, by name.
    given_name: String,
    /// ADMIN: the admin agent it was left to.
    admin: String,
    /// GENERATED or IMPORTED: given to no one (the vault's "generated"
    /// state), not to the agent that asked.
    to_no_one: bool,
}

impl SecretOutcome {
    /// From a request's row: its state, its new secret's JSON (what the card
    /// asked to make, and whether the person added it there), the name of
    /// the secret given, and the admin agent it went to ('' when the person
    /// decided it themselves).
    fn of(state: &str, new_secret: &str, given_name: String, admin: String) -> SecretOutcome {
        use v1::NeedsYouSecretOutcome as Secret;
        let new: Value = serde_json::from_str(new_secret).unwrap_or_default();
        let filled = |key: &str| match new.get(key) {
            Some(Value::String(text)) => !text.is_empty(),
            Some(Value::Array(list)) => !list.is_empty(),
            Some(Value::Object(map)) => !map.is_empty(),
            _ => false,
        };
        let outcome = match state {
            "granted" | "generated" | "approved" if state == "approved" || !admin.is_empty() => Secret::Admin,
            "granted" | "generated" if filled("generate") => Secret::Generated,
            "granted" | "generated" if filled("imports") => Secret::Imported,
            "granted" | "generated" if new.get("added").and_then(Value::as_bool).unwrap_or(false) => Secret::Added,
            "granted" | "generated" => Secret::Gave,
            _ => return SecretOutcome::default(),
        };
        let admin = if outcome == Secret::Admin { admin } else { String::new() };
        let given_name = if outcome == Secret::Admin { String::new() } else { given_name };
        let to_no_one = matches!(outcome, Secret::Generated | Secret::Imported) && state == "generated";
        SecretOutcome { outcome, given_name, admin, to_no_one }
    }
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
    /// The model and effort it set: while the subject still has them, it
    /// can be undone (`superseded`).
    after: (String, String),
}

/// A change an agent made to another's conversation (CONVERSATION), as
/// History shows it.
#[derive(Default)]
struct ConversationChange {
    /// Its id in conversation_changes, to find its tool call.
    change: String,
    /// Whose conversation it was.
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
            conversation: ConversationChange::default(),
            member: String::new(),
            reason: String::new(),
            updates: 0,
            proposal: String::new(),
            proposer: String::new(),
            alert: None,
            on: None,
            procedure: Procedure::default(),
            secret: SecretOutcome::default(),
            command: None,
            handoff: None,
            action_error: String::new(),
            codes_kept_as: String::new(),
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

/// A replied card wins over its stored state. Read the state first even
/// then, so its conversion error still precedes the reply flag's.
fn outcome(
    row: &rusqlite::Row<'_>,
    state: usize,
    replied: usize,
    when: (&str, Outcome),
    otherwise: Outcome,
) -> rusqlite::Result<Outcome> {
    let state: String = row.get(state)?;
    Ok(if row.get::<_, bool>(replied)? { Outcome::Replied } else if state == when.0 { when.1 } else { otherwise })
}

/// How many times a card was changed, from its version.
fn updates(revision: i64) -> u32 {
    u32::try_from(revision - 1).unwrap_or(0)
}

/// A card its agent withdrew (state 'withdrawn') wins over `decided`,
/// the outcome its other states give.
fn withdrawn(row: &rusqlite::Row<'_>, state: usize, decided: Outcome) -> rusqlite::Result<Outcome> {
    Ok(if row.get::<_, String>(state)? == "withdrawn" { Outcome::Withdrawn } else { decided })
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
    /// Map a table's changes up to the scan cap, keeping whether it had more.
    fn changes<T>(&mut self, changes: Vec<T>, make: impl FnMut(T) -> Row) {
        let capped = changes.len() > MAX_SCAN as usize;
        let rows = changes.into_iter().take(MAX_SCAN as usize).map(make).collect();
        self.add((rows, capped));
    }

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
        "SELECT id, agent, title, answer, state, created, decided, taint, review, body, replied, reason, revision, resolved, steps, action, action_error FROM asks
         WHERE state IN ('answered', 'dismissed', 'withdrawn') AND decided >= ?1 AND decided < ?2 ORDER BY decided DESC LIMIT ?3",
        window,
        |row| {
            let review: String = row.get(8)?;
            let id: i64 = row.get(0)?;
            // An Execute card: Claktory's heading, its command, how it ran.
            let action = row.get::<_, Option<String>>(15)?.unwrap_or_default();
            let command = crate::command_cards::in_history(db, id, &action);
            let kind = match (&command, review.is_empty()) {
                (Some(_), _) => Kind::Command,
                (None, true) => Kind::Question,
                (None, false) => Kind::Review,
            };
            let answered = if command.is_some() { Outcome::Approved } else { Outcome::Answered };
            let outcome = withdrawn(row, 4, outcome(row, 4, 10, ("answered", answered), Outcome::Dismissed)?)?;
            // Talked through on its card and ended with Resolve.
            let outcome = if row.get::<_, bool>(13)? { Outcome::Resolved } else { outcome };
            let mut found = Row::new(kind, id.to_string(), row.get(1)?, outcome, row.get(5)?, row.get(6)?);
            let agents_title: String = row.get(2)?;
            found.title = cut(&command.as_ref().map_or_else(|| agents_title.clone(), crate::command_cards::heading), MAX_TITLE);
            found.reply = cut(&row.get::<_, String>(3)?, MAX_REPLY);
            found.taint = row.get(7)?;
            let reason: String = row.get(11)?;
            found.reason = cut(&reason, MAX_REPLY);
            found.updates = updates(row.get(12)?);
            found.procedure = Procedure::of(row.get::<_, Option<String>>(14)?.as_deref(), row.get(10)?, row.get(13)?);
            let words = command.as_ref().map(|command| command.command.clone()).unwrap_or_default();
            found.command = command;
            // An errand's handoff: what it was, as History says it.
            if let Ok(crate::card_actions::Action::Handoff { handoff }) = serde_json::from_str(&action) {
                if handoff.kind == v1::ErrandHandoffKind::RecoveryCodes as i32 {
                    found.codes_kept_as = crate::extensions::ask::kept_as(&found.reply).unwrap_or_default().to_owned();
                }
                found.handoff = Some(v1::ErrandHandoff { agreement: String::new(), ..handoff.wire() });
            }
            found.action_error = cut(&row.get::<_, String>(16)?, MAX_REPLY);
            Ok(found.searched(&[&row.get::<_, String>(9)?, &row.get::<_, String>(3)?, &reason, &agents_title, &words]))
        },
    )?);
    found.add(rows(
        db,
        "SELECT id, agent, text, comment, state, created, decided, replied, reason, revision FROM proposals
         WHERE state IN ('approved', 'dismissed', 'withdrawn') AND decided >= ?1 AND decided < ?2 ORDER BY decided DESC LIMIT ?3",
        window,
        |row| {
            let text: String = row.get(2)?;
            let outcome = withdrawn(row, 4, outcome(row, 4, 7, ("approved", Outcome::Approved), Outcome::Dismissed)?)?;
            let mut found = Row::new(Kind::Proposal, row.get::<_, i64>(0)?.to_string(), row.get(1)?, outcome, row.get(5)?, row.get(6)?);
            found.title = cut(text.lines().find(|line| !line.trim().is_empty()).unwrap_or_default(), MAX_TITLE);
            found.reply = cut(&row.get::<_, String>(3)?, MAX_REPLY);
            let reason: String = row.get(8)?;
            found.reason = cut(&reason, MAX_REPLY);
            found.updates = updates(row.get(9)?);
            Ok(found.searched(&[&text, &row.get::<_, String>(3)?, &reason]))
        },
    )?);
    found.add(rows(
        db,
        "SELECT a.id, a.agent, a.secrets, a.state, a.created, a.decided, a.command, a.replied, COALESCE(n.comment, '')
         FROM pipe_secret_approvals a
         LEFT JOIN decision_notes n ON a.replied = 1 AND n.kind = 'secret_use' AND n.ref = CAST(a.id AS TEXT)
         WHERE a.state IN ('allowed', 'denied') AND a.decided >= ?1 AND a.decided < ?2 ORDER BY a.decided DESC LIMIT ?3",
        window,
        |row| {
            let names: Vec<String> = serde_json::from_str(&row.get::<_, String>(2)?).unwrap_or_default();
            let outcome = outcome(row, 3, 7, ("allowed", Outcome::Approved), Outcome::Denied)?;
            let mut found = Row::new(Kind::SecretUse, row.get::<_, i64>(0)?.to_string(), row.get(1)?, outcome, row.get(4)?, row.get(5)?);
            found.title = cut(&names.join(", "), MAX_TITLE);
            found.reply = cut(&row.get::<_, String>(8)?, MAX_REPLY);
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
        "SELECT d.agent, d.kind, d.allowed, d.asked, d.decided, d.pipe, d.seq, COALESCE(m.text, ''), d.id, COALESCE(n.comment, ''),
                COALESCE(m.replied, 0), COALESCE(w.comment, '')
         FROM pipe_decisions d LEFT JOIN pipe_messages m ON d.seq > 0 AND m.pipe = d.pipe AND m.seq = d.seq
         LEFT JOIN decision_notes n ON d.seq > 0 AND n.kind = 'pipe_message' AND n.ref = d.pipe || ':' || d.seq
         LEFT JOIN decision_notes w ON d.seq > 0 AND d.kind = 'held' AND w.kind = 'pipe_withdrawn' AND w.ref = d.pipe || ':' || d.seq
         WHERE d.decided >= ?1 AND d.decided < ?2 ORDER BY d.decided DESC LIMIT ?3",
        window,
        |row| {
            let kind: String = row.get(1)?;
            let (kind, title) = match kind.as_str() {
                "held" => (Kind::PipeHeld, "Your agent's message to the other side"),
                "held_incoming" => (Kind::PipeHeld, "A message from the other side"),
                _ => (Kind::PipeLoop, "Your agents and theirs kept replying to each other"),
            };
            // A held message sent back with a reply was neither
            // sent nor simply discarded; one its writer took back
            // (pipe_withdraw) was withdrawn, with its reason.
            let withdrawn: String = row.get(11)?;
            let outcome = match row.get::<_, bool>(2)? {
                _ if !withdrawn.is_empty() => Outcome::Withdrawn,
                _ if row.get::<_, bool>(10)? => Outcome::Replied,
                true => Outcome::Approved,
                false => Outcome::Denied,
            };
            let mut found = Row::new(kind, row.get::<_, i64>(8)?.to_string(), row.get(0)?, outcome, row.get(3)?, row.get(4)?);
            found.title = title.to_owned();
            found.reply = cut(&row.get::<_, String>(9)?, MAX_REPLY);
            found.pipe = row.get(5)?;
            found.seq = row.get::<_, i64>(6)?.try_into().unwrap_or_default();
            let text: String = row.get(7)?;
            found.excerpt = cut(&text, MAX_EXCERPT);
            found.reason = cut(&withdrawn, MAX_REPLY);
            Ok(found.searched(&[&text, &withdrawn]))
        },
    )?);
    found.add(rows(
        db,
        "SELECT id, agent, grantee, app, state, created, decided, comment, reason, replied FROM app_grant_proposals
         WHERE state IN ('granted', 'declined', 'failed') AND decided >= ?1 AND decided < ?2 ORDER BY decided DESC LIMIT ?3",
        window,
        |row| {
            let outcome = match row.get::<_, String>(4)?.as_str() {
                _ if row.get::<_, bool>(9)? => Outcome::Replied,
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
    from_proposal_grants(db, window, found)?;
    from_model_changes(db, window, names, found)?;
    from_conversation_changes(db, window, names, found)?;
    from_member_changes(db, window, names, found)?;
    from_given_up(db, window, names, found)?;
    from_access_given_up(db, window, names, found)?;
    from_saved(db, window, names, found)?;
    from_alerts(db, window, found)?;
    from_protected(db, window, found)?;
    from_recovery_key_changes(db, window, found)?;
    from_browser_switches(db, window, names, found)?;
    Ok(())
}

/// Agents' browsers turned on or off (`browser_switches`), with no card:
/// Claktory's sentence, whose browser as the subject.
fn from_browser_switches(db: &Connection, window: &Window, names: &HashMap<String, String>, found: &mut Found) -> rusqlite::Result<()> {
    let switches = crate::browser_switches::between(db, window.since, window.before, i64::from(MAX_SCAN) + 1)?;
    found.changes(switches, |switch| {
        let mut row = Row::new(Kind::Browser, format!("browser:{}", switch.id), switch.admin().to_owned(), Outcome::Changed, switch.at, switch.at);
        row.title = cut(&switch.title(names), MAX_TITLE);
        row.member = switch.agent.clone();
        let subject = names.get(&switch.agent).cloned().unwrap_or_default();
        row.searched(&[&subject])
    });
    Ok(())
}

/// The places macOS refused whose Needs you item ended (`protected`): the
/// item's words when it was raised, dismissed with its OK or ended by a
/// check that found the place let in.
fn from_protected(db: &Connection, window: &Window, found: &mut Found) -> rusqlite::Result<()> {
    let ended = crate::protected::between(db, window.since, window.before, i64::from(MAX_SCAN) + 1)?;
    found.changes(ended, |ended| {
        let outcome = if ended.dismissed { Outcome::Dismissed } else { Outcome::Changed };
        let mut row = Row::new(Kind::ProtectedPlace, format!("protected:{}", ended.id), ended.agent.clone(), outcome, ended.raised, ended.ended);
        row.title = cut(&ended.title, MAX_TITLE);
        row.excerpt = cut(&ended.text, MAX_EXCERPT);
        row.searched(&[&ended.text])
    });
    Ok(())
}

/// The alerts the person read (`notify::alerts`): its words as sent (none
/// when withheld), and the device it was read on.
fn from_alerts(db: &Connection, window: &Window, found: &mut Found) -> rusqlite::Result<()> {
    let read = crate::notify::alerts::between(db, window.since, window.before, i64::from(MAX_SCAN) + 1)?;
    found.changes(read, |read| {
        let alert = read.alert;
        let mut row = Row::new(Kind::Alert, alert.id.clone(), alert.agent_id.clone(), Outcome::Read, alert.sent_ms, read.read);
        row.title = cut(&alert.title, MAX_TITLE);
        let searched = [alert.text.clone(), alert.from_name.clone()];
        row.on = Some(read.on);
        row.alert = Some(alert);
        row.searched(&[&searched[0], &searched[1]])
    });
    Ok(())
}

/// The account's recovery key changes this Claktory sealed its vault to
/// (`account::recovery`): Claktory's sentence, asked when the
/// check-in saw the new key.
fn from_recovery_key_changes(db: &Connection, window: &Window, found: &mut Found) -> rusqlite::Result<()> {
    let changes = crate::account::recovery::between(db, window.since, window.before, i64::from(MAX_SCAN) + 1)?;
    found.changes(changes, |change| {
        let mut row = Row::new(Kind::RecoveryKeyChanged, format!("recovery:{}", change.id), String::new(), Outcome::Changed, change.seen, change.at);
        row.title = cut(&change.title, MAX_TITLE);
        row.searched(&[])
    });
    Ok(())
}

/// The agents an admin agent let only send to a pipe, or stopped
/// (`pipes::senders`): Claktory's sentence, the pipe's record.
fn from_member_changes(db: &Connection, window: &Window, names: &HashMap<String, String>, found: &mut Found) -> rusqlite::Result<()> {
    let changes = crate::pipes::senders::between(db, window.since, window.before, i64::from(MAX_SCAN) + 1)?;
    found.changes(changes, |change| {
        let mut row =
            Row::new(Kind::PipeMember, format!("member:{}", change.id), change.by.clone(), Outcome::Changed, change.at, change.at);
        row.title = cut(&change.title(names), MAX_TITLE);
        row.pipe = change.pipe.clone();
        row.member = change.agent.clone();
        let subject = names.get(&change.agent).cloned().unwrap_or_default();
        row.searched(&[&subject])
    });
    Ok(())
}

/// The secrets agents gave up on their own (`vault::given_up`): Claktory's
/// sentence, the agent's reason apart.
fn from_given_up(db: &Connection, window: &Window, names: &HashMap<String, String>, found: &mut Found) -> rusqlite::Result<()> {
    let given = crate::vault::given_up::between(db, window.since, window.before, i64::from(MAX_SCAN) + 1)?;
    found.changes(given, |given| {
        let mut row = Row::new(Kind::SecretGivenUp, given.id.to_string(), given.agent.clone(), Outcome::Changed, given.at, given.at);
        row.title = cut(&given.title(names), MAX_TITLE);
        row.reason = cut(&given.reason, MAX_REPLY);
        row.searched(&[&given.reason])
    });
    Ok(())
}

/// Access agents gave up on their own (`crate::given_up`):
/// Claktory's sentence, the agent's reason apart, and what it gave up.
fn from_access_given_up(db: &Connection, window: &Window, names: &HashMap<String, String>, found: &mut Found) -> rusqlite::Result<()> {
    use crate::given_up::What;
    let given = crate::given_up::between(db, window.since, window.before, i64::from(MAX_SCAN) + 1)?;
    found.changes(given, |given| {
        let mut row = Row::new(Kind::GivenUp, given.id.to_string(), given.agent.clone(), Outcome::Changed, given.at, given.at);
        row.title = cut(&given.title(names), MAX_TITLE);
        row.reason = cut(&given.reason, MAX_REPLY);
        match given.what {
            what if what.pipe() => row.pipe = given.target.clone(),
            What::App => row.app.app = given.target.clone(),
            _ => row.member = given.target.clone(),
        }
        row.searched(&[&given.reason, &given.label])
    });
    Ok(())
}

/// The secrets agents saved from a command's output (`vault::saved`):
/// Claktory's sentence, the secret's description apart.
fn from_saved(db: &Connection, window: &Window, names: &HashMap<String, String>, found: &mut Found) -> rusqlite::Result<()> {
    let saved = crate::vault::saved::between(db, window.since, window.before, i64::from(MAX_SCAN) + 1)?;
    found.changes(saved, |saved| {
        let mut row = Row::new(Kind::SecretSaved, saved.id.to_string(), saved.agent.clone(), Outcome::Changed, saved.at, saved.at);
        row.title = cut(&saved.title(names), MAX_TITLE);
        row.reason = cut(&saved.description, MAX_REPLY);
        let parents = saved.parents.join(" ");
        row.searched(&[&saved.description, &saved.name, &parents])
    });
    Ok(())
}

/// What agents did to other agents' conversations with the Conversations
/// extension (`fork::history`): Claktory's sentence, the agent's reason apart.
fn from_conversation_changes(db: &Connection, window: &Window, names: &HashMap<String, String>, found: &mut Found) -> rusqlite::Result<()> {
    let changes = crate::fork::history::between(db, window.since, window.before, i64::from(MAX_SCAN) + 1)?;
    found.changes(changes, |change| {
        let mut row = Row::new(Kind::Conversation, change.id.to_string(), change.by.clone(), Outcome::Changed, change.at, change.at);
        row.title = cut(&change.title(names), MAX_TITLE);
        row.conversation =
            ConversationChange { change: change.id.to_string(), subject: change.agent.clone(), reason: cut(&change.reason, MAX_REPLY) };
        let subject = names.get(&change.agent).cloned().unwrap_or_default();
        row.searched(&[&change.reason, &subject])
    });
    Ok(())
}

/// The model changes agents made with models_set
/// (`extensions::models`): Claktory's sentence, the agent's reason apart.
fn from_model_changes(db: &Connection, window: &Window, names: &HashMap<String, String>, found: &mut Found) -> rusqlite::Result<()> {
    let changes = crate::extensions::models::between(db, window.since, window.before, i64::from(MAX_SCAN) + 1)?;
    found.changes(changes, |change| {
        let mut row = Row::new(Kind::Model, change.id.to_string(), change.by.clone(), Outcome::Changed, change.at, change.at);
        row.title = cut(&change.title(names), MAX_TITLE);
        row.model = ModelChange {
            change: change.id.to_string(),
            undone: change.undone.is_some(),
            subject: change.agent.clone(),
            reason: cut(&change.reason, MAX_REPLY),
            after: (change.model_after.clone(), change.effort_after.clone()),
        };
        row.searched(&[&change.reason, &change.model_after, &change.name_after])
    });
    Ok(())
}

/// Changes to what an app reads and who uses it (apps::changes), and
/// Rivet's cards for them that the person declined or that failed: titles
/// are Claktory's sentences, and Rivet's reason stays apart, as its words.
fn from_app_access(db: &Connection, window: &Window, names: &HashMap<String, String>, found: &mut Found) -> rusqlite::Result<()> {
    let names: std::collections::BTreeMap<String, String> = names.iter().map(|(id, name)| (id.clone(), name.clone())).collect();
    let changes = crate::apps::changes::between(db, window.since, window.before, i64::from(MAX_SCAN) + 1)?;
    let capped = changes.len() > MAX_SCAN as usize;
    // An agent's giving up its own shows as GIVEN_UP (`from_access_given_up`).
    let changes = changes.into_iter().take(MAX_SCAN as usize).filter(|change| change.via != crate::apps::changes::Via::Agent);
    let rows = changes.map(|change| {
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
        // Made on an approved proposal (`crate::proposal_grants`).
        if !card && let Some((proposal, _)) = change.reference.strip_prefix("proposal:").and_then(|grant| grant.split_once(':')) {
            row.proposal = proposal.to_owned();
            row.proposer = db.query_row("SELECT agent FROM proposals WHERE id = ?1", [proposal], |row| row.get(0)).unwrap_or_default();
        }
        row.app = AppAccess {
            change: change.id.to_string(),
            undone: change.undone_by.is_some(),
            app: change.app.clone(),
            reason: cut(&change.reason, MAX_REPLY),
            card: if card { change.reference.clone() } else { String::new() },
        };
        row.searched(&[&change.labels.app, &change.reason])
    });
    found.add((rows.collect(), capped));
    found.add(self::rows(
        db,
        "SELECT id, agent, app, card, state, created, decided, comment, replied FROM app_access_proposals
         WHERE state IN ('declined', 'failed') AND decided >= ?1 AND decided < ?2 ORDER BY decided DESC LIMIT ?3",
        window,
        |row| {
            let outcome = outcome(row, 4, 8, ("declined", Outcome::Denied), Outcome::Failed)?;
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

/// Changes an admin agent made with no card of their own, approved in an
/// agent's proposal (`crate::proposal_grants`), other than an app's
/// access (APP_ACCESS says so).
fn from_proposal_grants(db: &Connection, window: &Window, found: &mut Found) -> rusqlite::Result<()> {
    found.add(rows(
        db,
        "SELECT g.proposal, g.idx, g.summary, g.used, g.used_by, p.agent FROM proposal_grants g JOIN proposals p ON p.id = g.proposal
         WHERE g.used IS NOT NULL AND json_extract(g.what, '$.kind') != 'app' AND g.used >= ?1 AND g.used < ?2 ORDER BY g.used DESC LIMIT ?3",
        window,
        |row| {
            let (proposal, idx, used): (i64, i64, i64) = (row.get(0)?, row.get(1)?, row.get(3)?);
            let mut found = Row::new(Kind::ProposalGrant, format!("{proposal}:{idx}"), row.get(4)?, Outcome::Changed, used, used);
            found.title = cut(&row.get::<_, String>(2)?, MAX_TITLE);
            found.proposal = proposal.to_string();
            found.proposer = row.get(5)?;
            Ok(found.searched(&[]))
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
    factory_agent::text::list_and(&names)
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

/// Whether `table` has `column`: vault.db is read here as it is, and a
/// vault not opened since an update doesn't have the newer columns yet.
fn has_column(db: &Connection, table: &str, column: &str) -> bool {
    db.prepare(&format!("SELECT {column} FROM {table} LIMIT 0")).is_ok()
}

fn from_vault(db: &Connection, window: &Window, found: &mut Found) -> rusqlite::Result<()> {
    // Answered with a reply: 0 in a vault from before it.
    let replied = |table: &str| if has_column(db, table, "replied") { "replied" } else { "0 AS replied" };
    found.add(rows(
        db,
        &format!(
            "SELECT id, agent, secret_name, new_secret, comment, state, created, MAX(decided, approved) AS at, reason, {}, {}, {} FROM requests
             WHERE state IN ('granted', 'generated', 'approved', 'dismissed') AND at >= ?1 AND at < ?2 ORDER BY at DESC LIMIT ?3",
            replied("requests"),
            // Added later: '' in a vault from before them.
            if has_column(db, "requests", "given_name") { "given_name" } else { "'' AS given_name" },
            // The admin agent only when the request passed to it (its
            // approval stamped): every request names the one it would.
            if has_column(db, "requests", "admin") { "CASE WHEN approved > 0 THEN admin ELSE '' END AS admin" } else { "'' AS admin" },
        ),
        window,
        |row| {
            let mut name: String = row.get(2)?;
            if name.is_empty() {
                let new: Value = serde_json::from_str(&row.get::<_, String>(3)?).unwrap_or_default();
                name = new.get("name").and_then(Value::as_str).unwrap_or_default().to_owned();
            }
            let outcome = outcome(row, 5, 9, ("dismissed", Outcome::Dismissed), Outcome::Approved)?;
            let mut found = Row::new(Kind::SecretRequest, row.get(0)?, row.get(1)?, outcome, row.get(6)?, row.get(7)?);
            if outcome == Outcome::Approved {
                found.secret = SecretOutcome::of(&row.get::<_, String>(5)?, &row.get::<_, String>(3)?, row.get(10)?, row.get(11)?);
            }
            found.title = cut(&name, MAX_TITLE);
            found.reply = cut(&row.get::<_, String>(4)?, MAX_REPLY);
            Ok(found.searched(&[&row.get::<_, String>(8)?]))
        },
    )?);
    found.add(rows(
        db,
        &format!(
            "SELECT id, agent, connector, purpose, comment, state, created, decided, reason, {} FROM connect_proposals
             WHERE state IN ('connected', 'dismissed') AND decided >= ?1 AND decided < ?2 ORDER BY decided DESC LIMIT ?3",
            replied("connect_proposals")
        ),
        window,
        |row| {
            let (connector, purpose): (String, String) = (row.get(2)?, row.get(3)?);
            let outcome = outcome(row, 5, 9, ("connected", Outcome::Approved), Outcome::Dismissed)?;
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
fn card<'a>(entries: impl DoubleEndedIterator<Item = &'a v1::Entry>, row: &Row) -> Option<(u64, Vec<v1::PipeTaint>)> {
    entries.rev().find_map(|entry| {
        let taint = match (&entry.kind, row.kind) {
            (Some(Entry::AskPerson(ask)), Kind::Question | Kind::Review | Kind::Command) if ask.id == row.id => ask.taint.clone(),
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
            (Some(Entry::ToolCall(call)), Kind::ProposalGrant)
                if crate::proposal_grants::grant_of(call).as_deref() == Some(row.id.as_str()) =>
            {
                Vec::new()
            }
            (Some(Entry::ToolCall(call)), Kind::Alert) if crate::notify::alerts::alert_of(call).as_deref() == Some(row.id.as_str()) => {
                Vec::new()
            }
            (Some(Entry::ToolCall(call)), Kind::SecretGivenUp)
                if crate::vault::given_up::given_up_of(call).as_deref() == Some(row.id.as_str()) =>
            {
                Vec::new()
            }
            (Some(Entry::ToolCall(call)), Kind::GivenUp) if crate::given_up::given_up_of(call).as_deref() == Some(row.id.as_str()) => {
                Vec::new()
            }
            (Some(Entry::ToolCall(call)), Kind::SecretSaved) if crate::vault::saved::saved_of(call).as_deref() == Some(row.id.as_str()) => {
                Vec::new()
            }
            (Some(Entry::ToolCall(call)), Kind::Conversation)
                if crate::fork::history::change_of(call).as_deref() == Some(row.conversation.change.as_str()) =>
            {
                Vec::new()
            }
            _ => return None,
        };
        Some((entry.seq, taint))
    })
}

/// A pipe's record, which says where it is itself: no card shows it.
fn piped(kind: Kind) -> bool {
    matches!(kind, Kind::PipeHeld | Kind::PipeLoop | Kind::PipeForward | Kind::PipeMember)
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
    /// Reads the conversations the rows' cards are in that aren't held (not
    /// opened since a restart, or unloaded while idle: `crate::unload`), so
    /// each record says where its card is.
    async fn hold_cards_of(&self, rows: &[Row]) {
        let unheld: BTreeSet<String> = {
            let inner = self.lock();
            let chats = self.chats.ids();
            rows.iter()
                .filter(|row| !piped(row.kind))
                .flat_map(|row| [row.agent.clone(), row.proposer.clone()])
                .filter(|agent| !agent.is_empty())
                .filter(|agent| inner.config.agent(agent).is_some() || chats.contains(agent))
                .filter(|agent| inner.agents.get(agent).is_none_or(|runtime| runtime.transcript.is_none()))
                .collect()
        };
        let reads = futures_util::future::join_all(unheld.iter().map(|agent| self.ensure_loaded(agent))).await;
        for (agent, read) in unheld.iter().zip(reads) {
            if let Err(error) = read {
                tracing::warn!(%agent, %error, "could not read a conversation to find where its cards are");
            }
        }
    }

    /// ListNeedsYouHistory.
    pub(crate) async fn needs_you_history(&self, request: &v1::ListNeedsYouHistory) -> Result<v1::NeedsYouHistory, RequestError> {
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
        drop(vault);
        self.hold_cards_of(&rows).await;

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
                    inner.agents.get(&row.agent).and_then(|runtime| runtime.transcript.as_ref()).map(|transcript| transcript.in_memory());
                let piped = piped(row.kind);
                let found = if piped { None } else { entries.and_then(|entries| card(entries, &row)) };
                // The device it was decided from, as its card says.
                let answered_on = found.as_ref().and_then(|(seq, _)| {
                    let transcript = inner.agents.get(&row.agent).and_then(|runtime| runtime.transcript.as_ref())?;
                    transcript.entry(*seq)?.answered_on.clone()
                });
                let (seq, mut taint) = if piped { (row.seq, Vec::new()) } else { found.unwrap_or_default() };
                if taint.is_empty()
                    && let Some(stored) = row.taint.as_deref().and_then(|text| serde_json::from_str::<Value>(text).ok())
                {
                    taint = crate::pipes::agents::routes_from_json(&stored);
                }
                // The proposal it was approved in, in its agent's conversation.
                let proposal_seq = inner
                    .agents
                    .get(&row.proposer)
                    .and_then(|runtime| runtime.transcript.as_ref())
                    .and_then(|transcript| {
                        transcript.in_memory().rev().find_map(|entry| match &entry.kind {
                            Some(Entry::Proposal(proposal)) if !row.proposal.is_empty() && proposal.id == row.proposal => Some(entry.seq),
                            _ => None,
                        })
                    })
                    .unwrap_or_default();
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
                    agent_reason: match row.kind {
                        _ if row.outcome == Outcome::Withdrawn => row.reason,
                        // Closed by its agent as settled in the talk.
                        _ if row.outcome == Outcome::Resolved => row.reason,
                        Kind::Model => row.model.reason,
                        Kind::Conversation => row.conversation.reason,
                        Kind::SecretGivenUp | Kind::GivenUp | Kind::SecretSaved => row.reason,
                        _ => row.app.reason,
                    },
                    model_change_id: row.model.change,
                    // As UndoModelChange checks it.
                    superseded: row.kind == Kind::Model
                        && !row.model.undone
                        && inner
                            .config
                            .agent(&row.model.subject)
                            .is_none_or(|agent| (agent.model.clone(), agent.effort.clone()) != row.model.after),
                    subject_agent_id: match row.kind {
                        Kind::Conversation => row.conversation.subject,
                        Kind::PipeMember | Kind::GivenUp | Kind::Browser => row.member,
                        Kind::SecretRequest => row.secret.admin,
                        _ => row.model.subject,
                    },
                    updates: row.updates,
                    proposal_id: row.proposal,
                    proposal_agent_id: row.proposer,
                    proposal_seq,
                    // An alert's is where it was read.
                    answered_on: answered_on.or(row.on),
                    alert: row.alert,
                    step: row.procedure.step,
                    steps: row.procedure.steps,
                    step_stopped: row.procedure.stopped,
                    secret_outcome: row.secret.outcome.into(),
                    secret_given_name: row.secret.given_name,
                    secret_to_no_one: row.secret.to_no_one,
                    command: row.command,
                    handoff: row.handoff,
                    action_error: row.action_error,
                    codes_kept_as: row.codes_kept_as,
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
