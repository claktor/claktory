//! History's events (#458 B3): every message, card, proposal, request for a
//! secret and pipe message, and when agents worked, read from where each
//! is kept, merged in time order and paged by a cursor. What's said is
//! never read, but for telling a list send's copies apart in messages from
//! before `send_id` (its length and first words, compared, never sent).

use factory_protocol::v1::{self, HistoryEventKind as Kind};
use rusqlite::Connection;

/// Where events come from, each read in its own order (ids rise with time).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum Source {
    Messages = 0,
    Asks = 1,
    Proposals = 2,
    Requests = 3,
    PipeOut = 4,
    Runs = 5,
}

/// A row of an agent's, not of a chat's (`crate::chats::PREFIX`): chats
/// are the person's own conversations, never part of History (#473).
const NOT_A_CHAT: &str = "substr(agent, 1, 5) != 'chat:'";

const SOURCES: [Source; 6] = [Source::Messages, Source::Asks, Source::Proposals, Source::Requests, Source::PipeOut, Source::Runs];

/// How far each source has been read: the last id taken of each.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct Cursor([i64; 6]);

impl Cursor {
    pub(super) fn encode(self) -> Vec<u8> {
        self.0.iter().flat_map(|id| id.to_le_bytes()).collect()
    }

    /// Empty, or one this backend made; anything else starts over.
    pub(super) fn decode(bytes: &[u8]) -> Cursor {
        let mut ids = [0; 6];
        if bytes.len() == 48 {
            for (at, chunk) in bytes.chunks_exact(8).enumerate() {
                ids[at] = i64::from_le_bytes(chunk.try_into().unwrap_or_default());
            }
        }
        Cursor(ids)
    }
}

/// One event: who to whom ("" is the person; otherwise a MapNode id).
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Event {
    pub(super) at: i64,
    pub(super) kind: Kind,
    pub(super) from: String,
    pub(super) to: Vec<String>,
    pub(super) flags: u32,
}

/// A stretch of an agent's work.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Run {
    pub(super) agent: String,
    pub(super) started: i64,
    pub(super) ended: i64,
}

#[derive(Clone, Debug, PartialEq)]
pub(super) enum Item {
    Event(Event),
    Run(Run),
}

impl Item {
    pub(super) fn at(&self) -> i64 {
        match self {
            Item::Event(event) => event.at,
            Item::Run(run) => run.started,
        }
    }
}

/// An item and the last id of its source it covers (a list send covers
/// several messages).
struct Read {
    last: i64,
    item: Item,
}

/// Up to `limit` items after `cursor` in time order, none after `until`
/// (0: no end), runs only when `runs`; with the cursor after them, and
/// whether more may follow. `requests` reads the vault's requests for
/// secrets after a rowid.
pub(super) fn read(
    db: &Connection,
    requests: &dyn Fn(i64, usize) -> Vec<(i64, String, i64)>,
    cursor: Cursor,
    until: i64,
    limit: usize,
    runs: bool,
) -> rusqlite::Result<(Vec<Item>, Cursor, bool)> {
    // Enough of each that a list send isn't cut short.
    let batch = limit + 64;
    let mut reads = Vec::new();
    let mut full = Vec::new();
    for source in SOURCES {
        if source == Source::Runs && !runs {
            reads.push(Vec::new());
            full.push(false);
            continue;
        }
        let (items, was_full) = source_after(db, requests, source, cursor.0[source as usize], batch)?;
        reads.push(items);
        full.push(was_full);
    }
    let mut taken = Vec::new();
    let mut next = cursor;
    let mut heads = [0usize; 6];
    let after_until = |at: i64| until > 0 && at > until;
    while taken.len() < limit {
        let first = SOURCES
            .iter()
            .filter_map(|source| reads[*source as usize].get(heads[*source as usize]).map(|read| (read.item.at(), *source)))
            .min();
        let Some((at, source)) = first else { break };
        if after_until(at) {
            break;
        }
        let read = &reads[source as usize][heads[source as usize]];
        next.0[source as usize] = read.last;
        taken.push(read.item.clone());
        heads[source as usize] += 1;
    }
    // More: something read but not taken (and not past until), or a source
    // whose batch was full and is all taken.
    let more = SOURCES.iter().any(|source| {
        let (read, head) = (&reads[*source as usize], heads[*source as usize]);
        match read.get(head) {
            Some(left) => !after_until(left.item.at()),
            None => full[*source as usize],
        }
    });
    Ok((taken, next, more))
}

/// The cursor after everything there is now.
pub(super) fn end(db: &Connection, requests: &dyn Fn(i64, usize) -> Vec<(i64, String, i64)>) -> rusqlite::Result<Cursor> {
    let max = |sql: &str| db.query_row(sql, [], |row| row.get::<_, Option<i64>>(0)).map(Option::unwrap_or_default);
    let mut last_request = 0;
    loop {
        let more = requests(last_request, 1000);
        let Some(last) = more.last() else { break };
        last_request = last.0;
    }
    Ok(Cursor([
        max("SELECT max(id) FROM messages")?,
        max("SELECT max(id) FROM asks")?,
        max("SELECT max(id) FROM proposals")?,
        last_request,
        max("SELECT max(rowid) FROM pipe_messages")?,
        max("SELECT max(id) FROM runs")?,
    ]))
}

/// A source's items after `after`, from at most `batch` rows; and whether
/// the batch was full (more may follow).
fn source_after(
    db: &Connection,
    requests: &dyn Fn(i64, usize) -> Vec<(i64, String, i64)>,
    source: Source,
    after: i64,
    batch: usize,
) -> rusqlite::Result<(Vec<Read>, bool)> {
    let limit = i64::try_from(batch).unwrap_or(i64::MAX);
    let asked = |table: &str, kind: Kind| -> rusqlite::Result<(Vec<Read>, bool)> {
        let rows: Vec<(i64, String, i64)> = crate::store::query(
            db,
            &format!("SELECT id, agent, created FROM {table} WHERE id > ?1 AND {NOT_A_CHAT} ORDER BY id LIMIT ?2"),
            [after, limit],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )?;
        let full = rows.len() == batch;
        Ok((rows.into_iter().map(|(id, agent, at)| asked_of(id, &agent, at, kind)).collect(), full))
    };
    match source {
        Source::Messages => messages(db, after, batch),
        Source::Asks => asked("asks", Kind::Card),
        Source::Proposals => asked("proposals", Kind::Proposal),
        Source::Requests => {
            let rows = requests(after, batch);
            let full = rows.len() == batch;
            let reads = rows
                .into_iter()
                .filter(|(_, agent, _)| !crate::chats::is_chat(agent))
                .map(|(id, agent, at)| asked_of(id, &agent, at, Kind::SecretRequest))
                .collect();
            Ok((reads, full))
        }
        Source::PipeOut => {
            // What this side sent on a pipe: its agents' and the person's;
            // not what was refused or thrown away before it went.
            let rows: Vec<(i64, String, i64, String, String)> = crate::store::query(
                db,
                "SELECT rowid, pipe, created, author, agent FROM pipe_messages
                 WHERE rowid > ?1 AND author IN ('my_agent', 'me') AND state NOT IN ('refused', 'discarded') ORDER BY rowid LIMIT ?2",
                [after, limit],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?)),
            )?;
            let full = rows.len() == batch;
            let reads = rows
                .into_iter()
                .map(|(id, pipe, at, author, agent)| Read {
                    last: id,
                    item: Item::Event(Event {
                        at,
                        kind: Kind::PipeOut,
                        from: if author == "me" || agent.is_empty() { String::new() } else { format!("agent:{agent}") },
                        to: vec![format!("pipe:{pipe}")],
                        flags: 0,
                    }),
                })
                .collect();
            Ok((reads, full))
        }
        Source::Runs => {
            let rows: Vec<(i64, String, i64, i64)> = crate::store::query(
                db,
                &format!("SELECT id, agent, started, coalesce(ended, 0) FROM runs WHERE id > ?1 AND {NOT_A_CHAT} ORDER BY id LIMIT ?2"),
                [after, limit],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )?;
            let full = rows.len() == batch;
            let reads = rows
                .into_iter()
                .map(|(id, agent, started, ended)| Read { last: id, item: Item::Run(Run { agent: format!("agent:{agent}"), started, ended }) })
                .collect();
            Ok((reads, full))
        }
    }
}

/// An agent asking the person something.
fn asked_of(id: i64, agent: &str, at: i64, kind: Kind) -> Read {
    Read { last: id, item: Item::Event(Event { at, kind, from: format!("agent:{agent}"), to: vec![String::new()], flags: 0 }) }
}

/// A message row, as read.
struct Message {
    id: i64,
    agent: String,
    source: String,
    sender_id: String,
    created: i64,
    send_id: String,
    quiet: bool,
    reply: bool,
    intent: String,
    /// For telling copies apart before `send_id`: its length and first words.
    same: String,
}

/// Messages to agents, a list send's copies as one event.
fn messages(db: &Connection, after: i64, batch: usize) -> rusqlite::Result<(Vec<Read>, bool)> {
    let limit = i64::try_from(batch).unwrap_or(i64::MAX);
    let rows: Vec<Message> = crate::store::query(
        db,
        &format!(
            "SELECT id, agent, source, sender_id, created, coalesce(send_id, ''), coalesce(quiet, 0), coalesce(reply_to, ''),
                coalesce(intent, ''),
                CASE WHEN source = 'agent' AND coalesce(send_id, '') = '' THEN length(text) || ':' || substr(text, 1, 200) ELSE '' END
         FROM messages WHERE id > ?1 AND {NOT_A_CHAT} ORDER BY id LIMIT ?2"
        ),
        [after, limit],
        |row| {
            Ok(Message {
                id: row.get(0)?,
                agent: row.get(1)?,
                source: row.get(2)?,
                sender_id: row.get(3)?,
                created: row.get(4)?,
                send_id: row.get(5)?,
                quiet: row.get::<_, i64>(6)? != 0,
                reply: !row.get::<_, String>(7)?.is_empty(),
                intent: row.get(8)?,
                same: row.get(9)?,
            })
        },
    )?;
    let full = rows.len() == batch;
    let mut reads: Vec<Read> = Vec::new();
    let mut group: Option<&Message> = None;
    for row in &rows {
        let to = format!("agent:{}", row.agent);
        // Another copy of the same list send: the same sender, and the same
        // send_id, or (before it) the same words within a second.
        let copy = group.is_some_and(|first| {
            row.source == "agent"
                && first.source == "agent"
                && row.sender_id == first.sender_id
                && if first.send_id.is_empty() {
                    row.send_id.is_empty() && row.same == first.same && (row.created - first.created).abs() <= 1000
                } else {
                    row.send_id == first.send_id
                }
        });
        if copy && let Some(Read { last, item: Item::Event(event) }) = reads.last_mut() {
            event.to.push(to);
            *last = row.id;
            continue;
        }
        group = Some(row);
        let (kind, from) = match row.source.as_str() {
            "person" => (Kind::Person, String::new()),
            "agent" => (Kind::Agent, format!("agent:{}", row.sender_id)),
            "task" => (Kind::Task, String::new()),
            "monitor" => (Kind::Monitor, String::new()),
            "app" => (Kind::App, format!("app:{}", row.sender_id)),
            "pipe" if row.sender_id.starts_with("pipe:") => (Kind::PipeIn, row.sender_id.clone()),
            "pipe" => (Kind::PipeIn, format!("pipe:{}", row.sender_id)),
            _ => (Kind::Claktory, String::new()),
        };
        let intent = match row.intent.as_str() {
            "handoff" => 1,
            "decision_request" => 2,
            "decision" => 3,
            "status" => 4,
            _ => 0,
        };
        let flags = u32::from(row.quiet) | (u32::from(row.reply) << 1) | (intent << 2);
        reads.push(Read { last: row.id, item: Item::Event(Event { at: row.created, kind, from, to: vec![to], flags }) });
    }
    // A list send the batch may have cut short comes whole in the next.
    if full && reads.len() > 1 {
        reads.pop();
    }
    Ok((reads, full))
}

/// Items as HistoryEvents' columns, nodes by `index` (0 for the person).
pub(super) fn columns(items: &[Item], index: &mut dyn FnMut(&str) -> u32, next: Option<Cursor>) -> v1::HistoryEvents {
    let mut events = v1::HistoryEvents { base_ms: items.first().map_or(0, Item::at), ..Default::default() };
    let (mut last_event, mut last_run) = (events.base_ms, events.base_ms);
    for item in items {
        match item {
            Item::Event(event) => {
                events.dt.push(event.at - last_event);
                last_event = event.at;
                events.from.push(index(&event.from));
                events.kind.push(event.kind.into());
                events.flags.push(event.flags);
                events.to_count.push(u32::try_from(event.to.len()).unwrap_or(u32::MAX));
                events.to.extend(event.to.iter().map(|to| index(to)));
            }
            Item::Run(run) => {
                events.run_node.push(index(&run.agent));
                events.run_start.push(run.started - last_run);
                last_run = run.started;
                // 0 is still running: a run over in no time lasted a ms.
                events.run_ms.push(if run.ended > 0 { u64::try_from(run.ended - run.started).unwrap_or_default().max(1) } else { 0 });
            }
        }
    }
    events.next_cursor = next.map(Cursor::encode).unwrap_or_default();
    events
}
