//! Needs you's history (ListNeedsYouHistory): the cards the person decided
//! in the last 30 days, newest first, so they can find an agent they
//! answered ("the one about emails") and open its conversation at the card.
//!
//! Read only, over the tables the cards are kept in: asks, proposals,
//! pipe_secret_approvals, pipe_starts and app_grant_proposals in
//! factory.db; requests and
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
    /// All the text a search looks in, lowercase.
    search: String,
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
fn from_factory(db: &Connection, window: &Window, names: &HashMap<String, String>, found: &mut Found) -> rusqlite::Result<()> {
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
    Ok(())
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
        let names: HashMap<String, String> = self.lock().config.agents.iter().map(|agent| (agent.id.clone(), agent.name.clone())).collect();
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
            from_factory(&self.store.db(), window, &names, &mut found)?;
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
                let piped = matches!(row.kind, Kind::PipeHeld | Kind::PipeLoop);
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
}
