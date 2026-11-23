//! Where an agent's message stands among the others (#285, middleman.md
//! H1 and H3).
//!
//! Provenance: every agents_send records, with each recipient's row in
//! `messages`, the send it belongs to (one id for a whole fanout), the
//! sender's run at the time (bound by Claktory: what that run read is
//! context, not a confirmed reply), the messages it answers (`reply_to`,
//! each checked to be an agent's message the sender received), why it was
//! sent as the sender said (`intent`), and the sender's pipe taint then
//! (`{}` since #346: nothing a sender read reaches anyone).
//! Rows from before it keep NULLs there: unknown, never clean. An agent
//! reads each message's number (`msg="…"` on its event) to answer it.
//!
//! Quiet copies: a sender can ask for a copy for its lead (`copy_lead`)
//! that doesn't wake it. A copy is a row of `quiet_copies`, never of
//! `messages`, so nothing queues it, wakes the lead, or delivers it again
//! after a restart. The lead hears how many came only when it runs anyway
//! (in Claktory's note before its next step) and reads them with
//! `agents_copies`. It grants nothing: the sender must be able to message
//! its lead.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::sync::Arc;

use factory_agent::{Tool, ToolResult};
use factory_protocol::v1;
use rusqlite::{OptionalExtension, params};
use serde::Deserialize;
use serde_json::json;

use crate::config::{AgentConfig, Config};
use crate::factory::{Factory, RequestError};
use crate::store::Store;
use crate::transcript::now_ms;

pub(crate) const SCHEMA: &str = "
-- What an agent's message stands on (#285); NULL in rows from before it:
-- unknown. send_id: one id for a whole fanout; sender_run: the sender's run
-- when it sent (context, not a confirmed reply); reply_to: JSON list of the
-- messages to the sender it answers, checked; intent: as the sender said;
-- taint: the sender's pipe taint then (JSON, {} when none).
ALTER TABLE messages ADD COLUMN send_id TEXT;
ALTER TABLE messages ADD COLUMN sender_run INTEGER;
ALTER TABLE messages ADD COLUMN reply_to TEXT;
ALTER TABLE messages ADD COLUMN intent TEXT;
ALTER TABLE messages ADD COLUMN taint TEXT;
-- Quiet copies for a sender's lead: never in messages, so they never wake
-- it. noted: when the lead was told it has them; read: when it read them.
CREATE TABLE quiet_copies (
    id INTEGER PRIMARY KEY,
    agent TEXT NOT NULL,
    send_id TEXT NOT NULL,
    sender_id TEXT NOT NULL,
    sender TEXT NOT NULL,
    text TEXT NOT NULL,
    to_ids TEXT NOT NULL,
    reply_to TEXT NOT NULL,
    intent TEXT NOT NULL,
    taint TEXT NOT NULL,
    created INTEGER NOT NULL,
    noted INTEGER,
    read INTEGER
);
CREATE INDEX quiet_copies_by_agent ON quiet_copies (agent, id);
";

/// Why a message is sent, as its sender may say.
pub(crate) const INTENTS: [&str; 4] = ["handoff", "decision_request", "decision", "status"];
/// The most messages one send can say it answers.
const MAX_REPLY_TO: usize = 20;
/// The most copies agents_copies gives at once, and ListQuietCopies.
const MAX_READ: usize = 20;
const MAX_LIST: usize = 100;
/// The most senders the note names.
const NOTE_SENDERS: usize = 5;

pub(crate) const COPIES_TOOL: &str = "agents_copies";
const COPIES: &str = "The quiet copies agents you lead sent you (they asked for a copy with agents_send's copy_lead; it didn't wake you): the unread ones, oldest first, which then count as read. `all` lists the latest ones instead, read or not, to look again. `limit`: how many (20 at most).";

/// What Claktory records of a send with each row: its id, the sender's
/// run, and the sender's taint then (JSON).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Sent {
    pub send_id: String,
    pub sender_run: Option<i64>,
    pub taint: String,
}

/// What the sender said of its message.
#[derive(Clone, Debug, Default)]
pub(crate) struct Said {
    pub reply_to: Vec<u64>,
    pub intent: String,
    pub copy_lead: bool,
    /// Sent quietly (`quiet`): it waits for the recipient's next wake.
    pub quiet: bool,
}

impl Said {
    /// The event's provenance for a recipient; its number comes when it's
    /// recorded.
    pub(crate) fn provenance(&self) -> factory_agent::Provenance {
        factory_agent::Provenance {
            message: 0,
            reply_to: self.reply_to.clone(),
            intent: self.intent.clone(),
            quiet: self.quiet,
            quiet_note: String::new(),
        }
    }
}

/// A new send's id: 128 random bits.
pub(crate) fn send_id() -> String {
    crate::vault::crypto::random::<16>().iter().map(|byte| format!("{byte:02x}")).collect()
}

/// What the sender records now: the send's id and its run. Its taint is
/// `{}`: since #346 what a sender read never reaches those it messages.
pub(crate) fn sent(factory: &Factory, sender: &str) -> Sent {
    let sender_run = factory.lock().agents.get(sender).and_then(|runtime| runtime.run);
    Sent { send_id: send_id(), sender_run, taint: "{}".to_owned() }
}

/// `said`, checked before anything is sent: a known intent, and each
/// message it answers one from an agent that the sender received.
pub(crate) fn check(factory: &Factory, sender: &str, said: &mut Said) -> Result<(), String> {
    said.intent = said.intent.trim().to_owned();
    if !said.intent.is_empty() && !INTENTS.contains(&said.intent.as_str()) {
        return Err(format!("`intent` is one of {}, or left out.", INTENTS.join(", ")));
    }
    said.reply_to.sort_unstable();
    said.reply_to.dedup();
    if said.reply_to.len() > MAX_REPLY_TO {
        return Err(format!("`reply_to` takes at most {MAX_REPLY_TO} messages."));
    }
    let db = factory.store.db();
    for number in &said.reply_to {
        let theirs = i64::try_from(*number).ok().and_then(|id| {
            db.query_row("SELECT 1 FROM messages WHERE id = ?1 AND agent = ?2 AND source = 'agent'", params![id, sender], |_| Ok(()))
                .optional()
                .ok()
                .flatten()
        });
        if theirs.is_none() {
            return Err(format!(
                "{number} isn't a message an agent sent you: `reply_to` takes the msg numbers of the agents' messages you received. Nothing was sent."
            ));
        }
    }
    Ok(())
}

/// The lead that gets `from`'s quiet copy, or why it can't.
pub(crate) fn lead_for_copy<'a>(config: &'a Config, from: &AgentConfig) -> Result<&'a AgentConfig, String> {
    let Some(lead) = config.lead(from) else {
        return Err("You have no lead, so there's no one to copy: leave copy_lead out. Nothing was sent.".to_owned());
    };
    if !super::can_message(config, from, lead) {
        return Err(format!("You can't message your lead, {}, so it can't get a copy. Nothing was sent.", lead.name));
    }
    Ok(lead)
}

/// A quiet copy for `lead` of what `sender` sent to `to` (ids), signed as
/// the lead would read it.
pub(crate) struct Copy<'a> {
    pub lead: &'a str,
    pub sender: &'a str,
    pub signature: &'a str,
    pub text: &'a str,
    pub to: &'a [String],
    pub said: &'a Said,
    pub sent: &'a Sent,
}

/// Records the copy; its id.
pub(crate) fn record(store: &Store, copy: &Copy<'_>) -> rusqlite::Result<i64> {
    let db = store.db();
    db.execute(
        "INSERT INTO quiet_copies (agent, send_id, sender_id, sender, text, to_ids, reply_to, intent, taint, created)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
        params![
            copy.lead,
            copy.sent.send_id,
            copy.sender,
            copy.signature,
            copy.text,
            json!(copy.to).to_string(),
            json!(copy.said.reply_to).to_string(),
            copy.said.intent,
            copy.sent.taint,
            now_ms()
        ],
    )?;
    Ok(db.last_insert_rowid())
}

/// How many copies each agent hasn't read, by agent.
pub(crate) fn unread(store: &Store) -> BTreeMap<String, u32> {
    let db = store.db();
    let Ok(mut statement) = db.prepare_cached("SELECT agent, COUNT(*) FROM quiet_copies WHERE read IS NULL GROUP BY agent") else {
        return BTreeMap::new();
    };
    statement
        .query_map([], |row| Ok((row.get::<_, String>(0)?, row.get::<_, u32>(1)?)))
        .map(|rows| rows.flatten().collect())
        .unwrap_or_default()
}

/// The agent's copies it wasn't told of yet, in one line, marked told; None
/// without any. `direct`: whether it has agents_copies among its tools
/// (otherwise it reaches it through extension_call).
pub(crate) fn note(store: &Store, agent: &str, direct: bool) -> Option<String> {
    let db = store.db();
    let mut statement =
        db.prepare_cached("SELECT id, sender FROM quiet_copies WHERE agent = ?1 AND noted IS NULL AND read IS NULL ORDER BY id").ok()?;
    let rows: Vec<(i64, String)> =
        statement.query_map([agent], |row| Ok((row.get(0)?, row.get(1)?))).ok()?.flatten().collect();
    let last = rows.last()?.0;
    let mut by: Vec<(String, usize)> = Vec::new();
    for (_, sender) in &rows {
        match by.iter_mut().find(|(name, _)| name == sender) {
            Some((_, count)) => *count += 1,
            None => by.push((sender.clone(), 1)),
        }
    }
    let mut names: Vec<String> = by.iter().take(NOTE_SENDERS).map(|(name, count)| format!("{name} ({count})")).collect();
    if by.len() > NOTE_SENDERS {
        names.push(format!("{} more", by.len() - NOTE_SENDERS));
    }
    db.execute("UPDATE quiet_copies SET noted = ?3 WHERE agent = ?1 AND id <= ?2 AND noted IS NULL", params![agent, last, now_ms()]).ok()?;
    let count = rows.len();
    let copies = if count == 1 { "1 new quiet copy is" } else { &format!("{count} new quiet copies are") };
    let how = if direct { "agents_copies reads them".to_owned() } else { format!("extension_call with tool \"{COPIES_TOOL}\" reads them") };
    Some(format!("{copies} available, from {}: {how}. They didn't wake you; read them when it helps your work.", names.join(", ")))
}

struct Row {
    id: i64,
    sender_id: String,
    sender: String,
    text: String,
    to: Vec<String>,
    reply_to: Vec<u64>,
    intent: String,
    created: i64,
    read: bool,
}

const COLUMNS: &str = "id, sender_id, sender, text, to_ids, reply_to, intent, created, read";

fn row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Row> {
    Ok(Row {
        id: row.get(0)?,
        sender_id: row.get(1)?,
        sender: row.get(2)?,
        text: row.get(3)?,
        to: serde_json::from_str(&row.get::<_, String>(4)?).unwrap_or_default(),
        reply_to: serde_json::from_str(&row.get::<_, String>(5)?).unwrap_or_default(),
        intent: row.get(6)?,
        created: row.get(7)?,
        read: row.get::<_, Option<i64>>(8)?.is_some(),
    })
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ReadArgs {
    #[serde(default)]
    limit: Option<usize>,
    #[serde(default)]
    all: bool,
}

pub(crate) fn tool(factory: &Arc<Factory>, agent: &str) -> Tool {
    let parameters = json!({
        "type": "object",
        "properties": {
            "limit": {"type": "integer", "minimum": 1, "maximum": MAX_READ},
            "all": {"type": "boolean"},
        },
        "additionalProperties": false,
    });
    super::super::tool(
        COPIES_TOOL,
        "Read quiet copies",
        COPIES,
        parameters,
        super::super::typed_handler(factory, agent, |factory, agent, args: ReadArgs| read(factory, agent, &args)),
    )
}

/// The copies `reader` asked for, as text. Unread ones then count as read.
fn read(factory: &Arc<Factory>, reader: &str, args: &ReadArgs) -> Result<ToolResult, String> {
    let limit = args.limit.unwrap_or(MAX_READ).clamp(1, MAX_READ);
    let rows: Vec<Row> = {
        let db = factory.store.db();
        let query = if args.all {
            format!("SELECT {COLUMNS} FROM (SELECT * FROM quiet_copies WHERE agent = ?1 ORDER BY id DESC LIMIT ?2) ORDER BY id")
        } else {
            format!("SELECT {COLUMNS} FROM quiet_copies WHERE agent = ?1 AND read IS NULL ORDER BY id LIMIT ?2")
        };
        let mut statement = db.prepare(&query).map_err(|error| error.to_string())?;
        statement
            .query_map(params![reader, limit as i64], row)
            .map_err(|error| error.to_string())?
            .collect::<Result<_, _>>()
            .map_err(|error| error.to_string())?
    };
    if rows.is_empty() {
        let none = if args.all { "No agent has sent you a quiet copy." } else { "No unread quiet copies. all: true lists the latest ones again." };
        return Ok(ToolResult::text(none, json!({"copies": []})));
    }
    // Every copy is given: since #346 nothing a sender read reaches its
    // readers (its `taint` column is kept, `{}` from then on).
    let checked = vec![true; rows.len()];
    let unread: Vec<i64> = rows.iter().filter(|copy| !copy.read).map(|copy| copy.id).collect();
    if !unread.is_empty() {
        let db = factory.store.db();
        let now = now_ms();
        for id in &unread {
            let _ = db.execute("UPDATE quiet_copies SET read = ?2 WHERE id = ?1 AND read IS NULL", params![id, now]);
        }
    }
    if !unread.is_empty() {
        factory.broadcast_workspace(&factory.lock());
    }
    let inner = factory.lock();
    let config = &inner.config;
    let me = config.agent(reader);
    let name = |id: &str| match (config.agent(id), me) {
        (Some(agent), Some(me)) => super::signed(config, agent, me),
        (Some(agent), None) => agent.name.clone(),
        (None, _) => id.to_owned(),
    };
    let mut text = String::new();
    for (copy, checked) in rows.iter().zip(&checked) {
        if !checked {
            let _ = write!(text, "Copy {} couldn't be checked and was left out.\n\n", copy.id);
            continue;
        }
        let to: Vec<String> = copy.to.iter().map(|id| name(id)).collect();
        let _ = write!(text, "Copy {} from {}, to {}, {} ago", copy.id, copy.sender, to.join(", "), super::ago(copy.created, now_ms()));
        if !copy.reply_to.is_empty() {
            let numbers: Vec<String> = copy.reply_to.iter().map(u64::to_string).collect();
            let _ = write!(text, "; answering their messages {}", numbers.join(", "));
        }
        if !copy.intent.is_empty() {
            let _ = write!(text, "; {}", copy.intent.replace('_', " "));
        }
        let _ = write!(text, ":\n{}\n\n", copy.text);
    }
    let details: Vec<serde_json::Value> =
        rows.iter().zip(&checked).map(|(copy, checked)| json!({"id": copy.id, "from_id": copy.sender_id, "given": checked})).collect();
    Ok(ToolResult::text(text.trim_end().to_owned(), json!({"copies": details})))
}

/// ListQuietCopies: the person's view of an agent's copies, newest first.
/// Listing marks nothing read.
pub(crate) fn list(factory: &Factory, request: &v1::ListQuietCopies) -> Result<v1::QuietCopies, RequestError> {
    factory.known_agent(&request.agent_id)?;
    let limit = match request.limit as usize {
        0 => MAX_READ,
        limit => limit.min(MAX_LIST),
    };
    let before = if request.before_id == 0 { i64::MAX } else { i64::try_from(request.before_id).unwrap_or(i64::MAX) };
    let db = factory.store.db();
    let mut statement =
        db.prepare(&format!("SELECT {COLUMNS} FROM quiet_copies WHERE agent = ?1 AND id < ?2 ORDER BY id DESC LIMIT ?3"))?;
    let mut rows: Vec<Row> = statement.query_map(params![request.agent_id, before, limit as i64 + 1], row)?.collect::<Result<_, _>>()?;
    let more = rows.len() > limit;
    rows.truncate(limit);
    let copies = rows
        .into_iter()
        .map(|copy| v1::QuietCopy {
            id: u64::try_from(copy.id).unwrap_or_default(),
            from: copy.sender,
            from_id: copy.sender_id,
            text: copy.text,
            created_ms: copy.created,
            read: copy.read,
            reply_to_ids: copy.reply_to,
            to_ids: copy.to,
            intent: copy.intent,
        })
        .collect();
    Ok(v1::QuietCopies { agent_id: request.agent_id.clone(), copies, more })
}

/// An agent is deleted: the copies it was sent go.
pub(crate) fn forget(store: &Store, agent: &str) -> rusqlite::Result<()> {
    store.db().execute("DELETE FROM quiet_copies WHERE agent = ?1", [agent])?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use factory_agent::{Event, EventSource, Provenance};

    use super::Sent;
    use crate::factory::Queued;
    use crate::store::Store;

    #[test]
    fn a_waiting_message_keeps_where_it_stands_and_an_old_one_stays_unknown() {
        let path = std::env::temp_dir().join(format!("provenance-test-{}.db", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let store = Store::open(&path).expect("store");
        let provenance = Provenance { message: 0, reply_to: vec![7], intent: "handoff".to_owned(), ..Provenance::default() };
        let event = Event {
            source: EventSource::Agent,
            from: "Bo".to_owned(),
            id: "bo".to_owned(),
            summary: String::new(),
            transient: false,
            pipe_seqs: Vec::new(),
            provenance: Some(provenance),
        };
        let sent = Sent { send_id: "s1".to_owned(), sender_run: Some(3), taint: "{}".to_owned() };
        let queued = Queued { sent: Some(sent), ..Queued::event("Over to you.".to_owned(), event) };
        let id = store.add_message("max", &queued).expect("added");
        store
            .db()
            .execute("INSERT INTO messages (agent, source, sender, sender_id, summary, text, created) VALUES ('max', 'agent', 'Bo', 'bo', '', 'old', 1)", [])
            .expect("an old row");
        let waiting = store.waiting().expect("waiting");
        let back: Vec<Option<Provenance>> =
            waiting["max"].iter().map(|queued| queued.event.as_ref().and_then(|event| event.provenance.clone())).collect();
        let restored = Provenance { message: u64::try_from(id).expect("id"), reply_to: vec![7], intent: "handoff".to_owned(), ..Provenance::default() };
        assert_eq!(back, [Some(restored), None]);
        let (send_id, run, taint): (String, i64, String) = store
            .db()
            .query_row("SELECT send_id, sender_run, taint FROM messages WHERE id = ?1", [id], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
            .expect("row");
        assert_eq!((send_id.as_str(), run, taint.as_str()), ("s1", 3, "{}"));
        let unknown: Option<String> = store.db().query_row("SELECT taint FROM messages WHERE text = 'old'", [], |row| row.get(0)).expect("row");
        assert_eq!(unknown, None, "an old row's taint is unknown, not clean");
        drop(store);
        let _ = std::fs::remove_file(&path);
    }
}
