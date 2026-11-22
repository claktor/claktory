//! Agents in a person's direct messages (D4a, dms.md §7): what a Claktory's
//! backend may do for one of its agents the person gave access to some of
//! their conversations, and only that. Every call checks the agent's grant
//! inside the same IMMEDIATE transaction as what it reads or writes
//! (`grants::check`, which counts it against the scope's quota), so a
//! revoke lands before it or after it, never between.
//!
//! - **Conversations** (`GET agent/conversations`, any dms scope): only the
//!   ones its grant names, with the other person as the person's devices
//!   show them (an ended one as they were when it ended), and nothing of
//!   the person's own view (read, drafts).
//! - **History** (`GET agent/conversations/<id>/messages`, dm.read): the
//!   person's history pager, from the grant's start (`granted_ms`) and at
//!   most 64 KiB a page. Their deletions apply; reading never moves their
//!   read position.
//! - **Drafts** (`PUT conversations/<id>/agent-drafts/<client id>`,
//!   dm.draft): a suggestion for the person, apart from their own composer
//!   draft, sealed like a message (`Place::agent_draft`), for an active
//!   friendship of the generation the agent saw. The same id with the same
//!   text answers the same; other text revises it while it's open. At most
//!   three open a conversation an agent; the grant's quota keeps it to 20 an
//!   hour. Each change adds an `agent_draft` event for the person's devices,
//!   which list them (`GET …/agent-drafts`) and close one when the person
//!   uses or dismisses it (`DELETE …/agent-drafts/<id>`). An agent never
//!   sends one: the person does, as their own message.
//! - **Following** (`PUT`/`DELETE agent/subscriptions/<id>`, dm.subscribe
//!   with dm.read).
//! - **Events** (`GET agent/events?since=&wait=`, the backend itself, no
//!   agent): the account's own outbox, filtered to messages from the other
//!   person in conversations one of the calling Claktory's agents follows
//!   with a live grant to read and follow them, sent after the grant
//!   began. Ids only, never text; each delivery is counted against the
//!   grant's dm.subscribe quota. The backend long-polls it, authenticated
//!   afresh by the root every time, so no delivery outlives the
//!   registration it was checked against.

use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};
use serde::Deserialize;
use serde_json::{Value, json};

use super::bodies::{Stored, Submitted, merge};
use super::groups;
use super::identity::{Refusal, body, hex, invalid, random};
use super::keys::Keys;
use super::messages::{self, Bounds};
use super::sync::{self, Kind};
use crate::grants::{self, Resource};
use crate::ipc::{Actor, Caller};

pub const SCHEMA: &str = "
-- What an agent drafted for the person in a conversation: its id for it
-- (unique for the agent and conversation), the body sealed like a
-- message's, and when the person used or dismissed it (closed).
CREATE TABLE IF NOT EXISTS dm_agent_drafts (
    id TEXT PRIMARY KEY,
    account TEXT NOT NULL,
    actor TEXT NOT NULL,
    conversation_id TEXT NOT NULL,
    client_id TEXT NOT NULL,
    agent_name TEXT NOT NULL,
    revision INTEGER NOT NULL,
    format INTEGER NOT NULL,
    body BLOB NOT NULL,
    digest BLOB NOT NULL,
    created INTEGER NOT NULL,
    updated INTEGER NOT NULL,
    closed INTEGER,
    UNIQUE (account, actor, conversation_id, client_id)
);
CREATE INDEX IF NOT EXISTS dm_agent_drafts_by_conversation ON dm_agent_drafts (account, conversation_id);
-- The conversations each agent follows: a message there may wake it.
CREATE TABLE IF NOT EXISTS dm_agent_subscriptions (
    account TEXT NOT NULL,
    claktory_id TEXT NOT NULL,
    agent_id TEXT NOT NULL,
    incarnation TEXT NOT NULL,
    conversation_id TEXT NOT NULL,
    created INTEGER NOT NULL,
    PRIMARY KEY (account, claktory_id, conversation_id, agent_id, incarnation)
);
";

const APP: &str = "dms";
/// A page an agent reads: at most 64 KiB of bodies.
const AGENT_PAGE_BYTES: usize = 64 * 1024;
/// Drafts an agent may have open in one conversation.
const OPEN_DRAFTS: i64 = 3;
/// Open drafts the sync snapshot lists, at most.
const OPEN_LISTED: i64 = 200;
/// An event page: outbox rows looked at, and events answered, at most.
const SCAN: i64 = 500;
const EVENTS: usize = 100;
/// The longest name an agent's draft is shown under.
const MAX_AGENT_NAME: usize = 64;

/// Whether `route` is this module's.
pub fn knows(method: &str, route: &str) -> bool {
    let parts: Vec<&str> = route.split('/').collect();
    matches!(
        (method, parts.as_slice()),
        ("GET", ["agent", "conversations"] | ["agent", "events"] | ["agent", "conversations", _, "messages"])
            | ("PUT" | "DELETE", ["agent", "subscriptions", _])
            | ("GET", ["conversations", _, "agent-drafts"])
            | ("PUT" | "DELETE", ["conversations", _, "agent-drafts", _])
    )
}

/// Answers one of this module's routes.
pub fn answer(
    db: &mut Connection,
    keys: &Keys,
    method: &str,
    (route, query): (&str, &str),
    caller: &Caller,
    bytes: &[u8],
    now_ms: i64,
) -> Result<(u16, Value), Refusal> {
    let parts: Vec<&str> = route.split('/').collect();
    let transaction = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let answer = match (method, parts.as_slice(), caller) {
        // The person's devices: the drafts agents left them.
        ("GET", ["conversations", conversation, "agent-drafts"], Caller::Device { account_id, .. }) => {
            (200, drafts(&transaction, account_id, conversation)?)
        }
        ("DELETE", ["conversations", conversation, "agent-drafts", id], Caller::Device { account_id, .. }) => {
            close(&transaction, account_id, conversation, id, now_ms)?;
            (204, Value::Null)
        }
        // The backend itself: the events its agents follow.
        ("GET", ["agent", "events"], Caller::Backend { agent: None, .. }) => (200, events(&transaction, caller, query, now_ms)?),
        // The backend for one of its agents.
        (_, _, Caller::Backend { agent: Some(_), .. }) => match (method, parts.as_slice()) {
            ("GET", ["agent", "conversations"]) => (200, conversations(&transaction, keys, caller, now_ms)?),
            ("GET", ["agent", "conversations", conversation, "messages"]) => {
                (200, history(&transaction, caller, conversation, query, now_ms)?)
            }
            ("PUT", ["conversations", conversation, "agent-drafts", client_id]) => {
                draft(&transaction, caller, conversation, client_id, bytes, now_ms)?
            }
            ("PUT", ["agent", "subscriptions", conversation]) => {
                follow(&transaction, caller, conversation, now_ms)?;
                (204, Value::Null)
            }
            ("DELETE", ["agent", "subscriptions", conversation]) => {
                unfollow(&transaction, caller, conversation)?;
                (204, Value::Null)
            }
            _ => return Err(device_only()),
        },
        (_, ["agent", ..], _) => return Err(Refusal(403, "not_granted", "Only an agent given access can do this.".to_owned())),
        _ => return Err(device_only()),
    };
    transaction.commit()?;
    Ok(answer)
}

fn device_only() -> Refusal {
    Refusal(403, "device_only", "Only your own signed-in devices can do this.".to_owned())
}

fn not_found() -> Refusal {
    Refusal(404, "not_found", "There's nothing here.".to_owned())
}

fn not_granted() -> Refusal {
    Refusal(403, "not_granted", "This agent hasn't been given that.".to_owned())
}

fn refused_by(refused: grants::Refused) -> Refusal {
    match refused {
        grants::Refused::Quota(wait) => {
            Refusal(429, "rate_limited", format!("Too many for now. Try again in {} minutes.", (wait + 59) / 60))
        }
        grants::Refused::Db(error) => Refusal::from(super::keys::Refused::Db(error)),
        _ => not_granted(),
    }
}

fn conversation(id: &str) -> Resource {
    Resource { kind: "conversation".into(), id: id.to_owned() }
}

/// A live dms grant, as kept: its scopes, what it names and when it began.
type Held = (Vec<String>, Vec<Resource>, i64);

/// The calling agent's live dms grant; None when it has none.
fn grant_of(transaction: &Transaction<'_>, caller: &Caller, now_ms: i64) -> Result<Option<Held>, Refusal> {
    let Caller::Backend { account_id, claktory_id, epoch, agent: Some(agent) } = caller else { return Ok(None) };
    let found: Option<(String, String, i64, i64)> = transaction
        .query_row(
            "SELECT scopes, resources, granted, expires FROM grants
             WHERE app = ?1 AND account_id = ?2 AND claktory_id = ?3 AND epoch = ?4 AND agent_id = ?5 AND incarnation = ?6",
            params![APP, account_id, claktory_id, *epoch as i64, agent.id, agent.incarnation],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .optional()?;
    Ok(found.filter(|(_, _, _, expires)| *expires == 0 || *expires > now_ms).map(|(scopes, resources, granted, _)| {
        (serde_json::from_str(&scopes).unwrap_or_default(), serde_json::from_str(&resources).unwrap_or_default(), granted)
    }))
}

/// Checks and counts `scope` on `conversation` for the calling agent; when
/// its grant began.
fn allowed(transaction: &Transaction<'_>, caller: &Caller, scope: &str, on: &str, now_ms: i64) -> Result<i64, Refusal> {
    grants::check(transaction, caller, APP, scope, Some(&conversation(on)), now_ms).map_err(refused_by)?;
    let (_, _, granted) = grant_of(transaction, caller, now_ms)?.ok_or_else(not_granted)?;
    Ok(granted)
}

/// `GET agent/conversations`: the ones the agent's grant names.
fn conversations(transaction: &Transaction<'_>, keys: &Keys, caller: &Caller, now_ms: i64) -> Result<Value, Refusal> {
    let (scopes, resources, granted) = grant_of(transaction, caller, now_ms)?.ok_or_else(not_granted)?;
    // Counted against one of its scopes: listing what it was given.
    let scope = ["dm.read", "dm.draft", "dm.subscribe", "dm.send"]
        .into_iter()
        .find(|scope| scopes.iter().any(|held| held == scope))
        .ok_or_else(not_granted)?;
    grants::check(transaction, caller, APP, scope, None, now_ms).map_err(refused_by)?;
    let Caller::Backend { account_id, claktory_id, agent: Some(agent), .. } = caller else { return Err(not_granted()) };
    let named: Vec<&str> =
        resources.iter().filter(|resource| resource.kind == "conversation").map(|resource| resource.id.as_str()).collect();
    let all = messages::conversations(transaction, keys, account_id, now_ms)?;
    let mut shown = Vec::new();
    for listed in all["conversations"].as_array().into_iter().flatten() {
        let Some(id) = listed["id"].as_str() else { continue };
        if !named.contains(&id) {
            continue;
        }
        let subscribed: bool = transaction.query_row(
            "SELECT EXISTS (SELECT 1 FROM dm_agent_subscriptions
             WHERE account = ?1 AND claktory_id = ?2 AND conversation_id = ?3 AND agent_id = ?4 AND incarnation = ?5)",
            params![account_id, claktory_id, id, agent.id, agent.incarnation],
            |row| row.get(0),
        )?;
        // Its group, and the friend, for the Claktory's own leaf (E2).
        let generation = listed["contact_generation"].as_i64().unwrap_or_default();
        let group = groups::of_conversation(transaction, id, generation)?;
        let friend: Option<String> = transaction
            .query_row(
                "SELECT CASE WHEN low = ?2 THEN high ELSE low END FROM dm_contacts WHERE conversation_id = ?1",
                params![id, account_id],
                |row| row.get(0),
            )
            .optional()?;
        shown.push(json!({
            "id": id,
            "group_id": group.map(|group| super::leaves::b64(&group.id)),
            "account_id": friend,
            "contact_generation": listed["contact_generation"],
            "with": {"name": listed["with"]["name"], "tag": listed["with"]["tag"]},
            "active": listed["active"],
            "last_seq": listed["last_seq"],
            "from_ms": granted,
            "subscribed": subscribed,
        }));
    }
    Ok(json!({"conversations": shown}))
}

/// `GET agent/conversations/<id>/messages`: the person's history from the
/// grant's start, smaller pages.
fn history(transaction: &Transaction<'_>, caller: &Caller, on: &str, query: &str, now_ms: i64) -> Result<Value, Refusal> {
    let granted = allowed(transaction, caller, "dm.read", on, now_ms)?;
    let asked = Bounds::asked(query)?;
    // Times are whole seconds: a message in the grant's own second may have
    // come before it, so it isn't shown.
    let bounds = Bounds { max_bytes: AGENT_PAGE_BYTES.min(asked.max_bytes), from_ms: granted + 1, ..asked };
    messages::page(transaction, caller.account_id(), on, &bounds)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DraftAsk {
    contact_generation: i64,
    /// The draft as an MLS application message in the Claktory's channel
    /// with the person's devices.
    envelope: Value,
    #[serde(default)]
    agent_name: String,
    #[serde(default)]
    #[allow(dead_code)]
    request_nonce: Option<String>,
}

fn valid_client_id(id: &str) -> bool {
    (22..=64).contains(&id.len()) && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

/// The agent in its account's namespace, as messages name it.
fn actor(caller: &Caller) -> Option<String> {
    let Caller::Backend { claktory_id, epoch, agent: Some(agent), .. } = caller else { return None };
    Some(json!({"claktory": claktory_id, "epoch": epoch, "agent": agent.id, "incarnation": agent.incarnation}).to_string())
}

/// A name to show a draft under: one line, no hidden or
/// direction-changing characters, at most 64 characters.
fn shown_name(name: &str) -> String {
    name.chars()
        .filter(|c| !matches!(c, '\u{200b}'..='\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}' | '\u{feff}'))
        .map(|c| if c.is_control() { ' ' } else { c })
        .take(MAX_AGENT_NAME)
        .collect::<String>()
        .trim()
        .to_owned()
}

/// `PUT conversations/<id>/agent-drafts/<client id>`.
fn draft(
    transaction: &Transaction<'_>,
    caller: &Caller,
    on: &str,
    client_id: &str,
    bytes: &[u8],
    now_ms: i64,
) -> Result<(u16, Value), Refusal> {
    allowed(transaction, caller, "dm.draft", on, now_ms)?;
    let ask: DraftAsk = body(bytes)?;
    if !valid_client_id(client_id) {
        return Err(invalid("A draft's id has to be 22 to 64 letters, digits, - and _."));
    }
    let submitted = Submitted::from_json(&ask.envelope)?;
    let account = caller.account_id();
    let actor = actor(caller).ok_or_else(not_granted)?;
    let found: Option<(i64, Option<i64>)> = transaction
        .query_row(
            "SELECT generation, ended FROM dm_contacts WHERE conversation_id = ?1 AND (low = ?2 OR high = ?2)",
            params![on, account],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?;
    let (generation, ended) = found.ok_or_else(not_found)?;
    if ended.is_some() {
        return Err(Refusal(410, "closed", "This conversation has ended.".to_owned()));
    }
    if generation != ask.contact_generation {
        return Err(Refusal(409, "stale", "The friendship's generation changed. Look again, then try again.".to_owned()));
    }
    // In the Claktory's channel with the person's devices, at an epoch it
    // still takes.
    let Caller::Backend { claktory_id, .. } = caller else { return Err(not_granted()) };
    let channel = groups::channel_of(transaction, account, claktory_id)?
        .ok_or_else(|| Refusal(409, "not_set_up", "This Claktory has no channel with the person's devices yet.".to_owned()))?;
    match groups::fits(&channel, &submitted) {
        Ok(_) => {}
        Err(Refusal(409, "epoch", _)) => return Ok(groups::epoch_answer(channel.epoch)),
        Err(refused) => return Err(refused),
    }
    let digest = submitted.digest();
    let name = shown_name(&ask.agent_name);
    let earlier: Option<(String, i64, Vec<u8>, Option<i64>)> = transaction
        .query_row(
            "SELECT id, revision, digest, closed FROM dm_agent_drafts
             WHERE account = ?1 AND actor = ?2 AND conversation_id = ?3 AND client_id = ?4",
            params![account, actor, on, client_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .optional()?;
    let (id, revision) = match earlier {
        Some((_, _, _, Some(_))) => {
            return Err(Refusal(410, "closed", "The person already used or dismissed that draft. Make a new one.".to_owned()));
        }
        // A retry: the same answer.
        Some((id, revision, was, None)) if was == digest => {
            return Ok((200, json!({"draft_id": id, "revision": revision, "accepted_ms": now_ms})));
        }
        Some((id, revision, _, None)) => {
            let stored = submitted.store();
            transaction.execute(
                "UPDATE dm_agent_drafts SET revision = ?2, format = ?3, body = ?4, digest = ?5, agent_name = ?6, updated = ?7 WHERE id = ?1",
                params![id, revision + 1, stored.format, stored.bytes, digest, name, now_ms],
            )?;
            (id, revision + 1)
        }
        None => {
            let open: i64 = transaction.query_row(
                "SELECT COUNT(*) FROM dm_agent_drafts WHERE account = ?1 AND actor = ?2 AND conversation_id = ?3 AND closed IS NULL",
                params![account, actor, on],
                |row| row.get(0),
            )?;
            if open >= OPEN_DRAFTS {
                return Err(Refusal(
                    429,
                    "too_many_drafts",
                    format!(
                        "{OPEN_DRAFTS} of this agent's drafts already wait in this conversation. Change one of them, or wait for the person."
                    ),
                ));
            }
            let id = format!("ad{}", hex(&random::<16>()));
            let stored = submitted.store();
            transaction.execute(
                "INSERT INTO dm_agent_drafts (id, account, actor, conversation_id, client_id, agent_name, revision, format, body, digest, created, updated)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, 1, ?7, ?8, ?9, ?10, ?10)",
                params![id, account, actor, on, client_id, name, stored.format, stored.bytes, digest, now_ms],
            )?;
            (id, 1)
        }
    };
    sync::emit(transaction, account, Kind::AgentDraft, on, &id, revision, now_ms)?;
    Ok((200, json!({"draft_id": id, "revision": revision, "accepted_ms": now_ms})))
}

/// `GET conversations/<id>/agent-drafts`: the open drafts agents left the
/// person there, newest first.
fn drafts(transaction: &Transaction<'_>, account: &str, on: &str) -> Result<Value, Refusal> {
    let theirs: bool = transaction.query_row(
        "SELECT COUNT(*) > 0 FROM dm_contacts WHERE conversation_id = ?1 AND (low = ?2 OR high = ?2)",
        params![on, account],
        |row| row.get(0),
    )?;
    if !theirs {
        return Err(not_found());
    }
    type Kept = (String, String, String, i64, i64, Vec<u8>, i64, i64);
    let rows: Vec<Kept> = {
        let mut statement = transaction.prepare(
            "SELECT id, actor, agent_name, revision, format, body, created, updated FROM dm_agent_drafts
             WHERE account = ?1 AND conversation_id = ?2 AND closed IS NULL ORDER BY updated DESC",
        )?;
        statement
            .query_map(params![account, on], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?, row.get(5)?, row.get(6)?, row.get(7)?))
            })?
            .collect::<Result<_, _>>()?
    };
    let mut shown = Vec::new();
    for (id, actor, name, revision, format, bytes, created, updated) in rows {
        let actor: Value = serde_json::from_str(&actor).unwrap_or_default();
        let mut draft = json!({
            "id": id,
            "agent": {"name": name, "claktory_id": actor["claktory"]},
            "revision": revision,
            "created_ms": created,
            "updated_ms": updated,
        });
        merge(&mut draft, Stored { format, bytes }.shown()?);
        shown.push(draft);
    }
    Ok(json!({"drafts": shown}))
}

/// The open drafts agents left the person, in every conversation that's
/// still theirs, newest first, without their text: for the sync snapshot,
/// so a device shows who left one where (People's friend rows).
pub fn open(transaction: &Transaction<'_>, account: &str) -> Result<Value, Refusal> {
    type Kept = (String, String, String, String, i64, i64);
    let rows: Vec<Kept> = {
        let mut statement = transaction.prepare(
            "SELECT d.id, d.conversation_id, d.actor, d.agent_name, d.revision, d.updated FROM dm_agent_drafts d
             WHERE d.account = ?1 AND d.closed IS NULL
               AND EXISTS (SELECT 1 FROM dm_contacts c WHERE c.conversation_id = d.conversation_id AND (c.low = ?1 OR c.high = ?1))
             ORDER BY d.updated DESC LIMIT ?2",
        )?;
        statement
            .query_map(params![account, OPEN_LISTED], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?, row.get(5)?))
            })?
            .collect::<Result<_, _>>()?
    };
    let open: Vec<Value> = rows
        .into_iter()
        .map(|(id, conversation, actor, name, revision, updated)| {
            let actor: Value = serde_json::from_str(&actor).unwrap_or_default();
            json!({
                "id": id,
                "conversation_id": conversation,
                "agent": {"name": name, "claktory_id": actor["claktory"]},
                "revision": revision,
                "updated_ms": updated,
            })
        })
        .collect();
    Ok(Value::from(open))
}

/// `DELETE conversations/<id>/agent-drafts/<id>`: the person used or
/// dismissed it; their other devices hear so.
fn close(transaction: &Transaction<'_>, account: &str, on: &str, id: &str, now_ms: i64) -> Result<(), Refusal> {
    let revision: Option<i64> = transaction
        .query_row(
            "SELECT revision FROM dm_agent_drafts WHERE id = ?1 AND account = ?2 AND conversation_id = ?3 AND closed IS NULL",
            params![id, account, on],
            |row| row.get(0),
        )
        .optional()?;
    let Some(revision) = revision else { return Ok(()) };
    transaction.execute("UPDATE dm_agent_drafts SET closed = ?2, body = x'', format = 0 WHERE id = ?1", params![id, now_ms])?;
    sync::emit(transaction, account, Kind::AgentDraft, on, id, revision + 1, now_ms)?;
    Ok(())
}

/// `PUT agent/subscriptions/<id>`: needs dm.read too, since a wake is only
/// worth reading.
fn follow(transaction: &Transaction<'_>, caller: &Caller, on: &str, now_ms: i64) -> Result<(), Refusal> {
    allowed(transaction, caller, "dm.subscribe", on, now_ms)?;
    let (scopes, _, _) = grant_of(transaction, caller, now_ms)?.ok_or_else(not_granted)?;
    if !scopes.iter().any(|scope| scope == "dm.read") {
        return Err(not_granted());
    }
    let Caller::Backend { account_id, claktory_id, agent: Some(agent), .. } = caller else { return Err(not_granted()) };
    transaction.execute(
        "INSERT OR IGNORE INTO dm_agent_subscriptions (account, claktory_id, agent_id, incarnation, conversation_id, created)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![account_id, claktory_id, agent.id, agent.incarnation, on, now_ms],
    )?;
    Ok(())
}

/// `DELETE agent/subscriptions/<id>`: always allowed, for its own.
fn unfollow(transaction: &Transaction<'_>, caller: &Caller, on: &str) -> Result<(), Refusal> {
    let Caller::Backend { account_id, claktory_id, agent: Some(agent), .. } = caller else { return Err(not_granted()) };
    transaction.execute(
        "DELETE FROM dm_agent_subscriptions WHERE account = ?1 AND claktory_id = ?2 AND agent_id = ?3 AND incarnation = ?4 AND conversation_id = ?5",
        params![account_id, claktory_id, agent.id, agent.incarnation, on],
    )?;
    Ok(())
}

/// `GET agent/events`: where the account's outbox is now (no `since`), or
/// the messages after it that wake one of the calling Claktory's agents.
fn events(transaction: &Transaction<'_>, caller: &Caller, query: &str, now_ms: i64) -> Result<Value, Refusal> {
    let Caller::Backend { account_id, claktory_id, epoch, .. } = caller else { return Err(not_granted()) };
    let mut since_text = None;
    for pair in query.split('&').filter(|pair| !pair.is_empty()) {
        let (name, value) = pair.split_once('=').unwrap_or((pair, ""));
        match name {
            "since" => since_text = Some(value.to_owned()),
            "wait" | "request_nonce" => {}
            _ => return Err(invalid("Events take since and wait.")),
        }
    }
    let Some(since_text) = since_text else {
        let last: i64 =
            transaction
                .query_row("SELECT COALESCE(MAX(seq), 0) FROM dm_account_events WHERE account = ?1", [account_id], |row| row.get(0))?;
        return Ok(json!({"events": [], "next_cursor": sync::cursor(last)}));
    };
    let since = sync::since(&since_text)?;
    sync::within(transaction, account_id, since)?;
    type Row = (i64, String, String, String);
    let rows: Vec<Row> = {
        let mut statement = transaction.prepare(
            "SELECT seq, kind, conversation_id, object_id FROM dm_account_events WHERE account = ?1 AND seq > ?2 ORDER BY seq LIMIT ?3",
        )?;
        statement
            .query_map(params![account_id, since, SCAN], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)))?
            .collect::<Result<_, _>>()?
    };
    let mut events = Vec::new();
    let mut through = since;
    for (seq, kind, on, object) in rows {
        if events.len() >= EVENTS {
            break;
        }
        through = seq;
        if kind != "message" {
            continue;
        }
        let message: Option<(String, i64, i64)> = transaction
            .query_row("SELECT author, seq, accepted FROM dm_messages WHERE id = ?1 AND conversation_id = ?2", params![object, on], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?))
            })
            .optional()?;
        // Only the other person's words wake anyone.
        let Some((author, message_seq, accepted)) = message.filter(|(author, _, _)| author != account_id) else { continue };
        let _ = author;
        let followers: Vec<(String, String)> = {
            let mut statement = transaction.prepare(
                "SELECT agent_id, incarnation FROM dm_agent_subscriptions WHERE account = ?1 AND claktory_id = ?2 AND conversation_id = ?3",
            )?;
            statement.query_map(params![account_id, claktory_id, on], |row| Ok((row.get(0)?, row.get(1)?)))?.collect::<Result<_, _>>()?
        };
        for (agent_id, incarnation) in followers {
            let agent = Actor { id: agent_id, incarnation };
            let as_agent = Caller::Backend {
                account_id: account_id.clone(),
                claktory_id: claktory_id.clone(),
                epoch: *epoch,
                agent: Some(agent.clone()),
            };
            // Its grant, now: live, naming the conversation, to follow and
            // read it, and begun before the message.
            let Some((scopes, _, granted)) = grant_of(transaction, &as_agent, now_ms)? else { continue };
            if !scopes.iter().any(|scope| scope == "dm.read") || accepted <= granted {
                continue;
            }
            if grants::check(transaction, &as_agent, APP, "dm.subscribe", Some(&conversation(&on)), now_ms).is_err() {
                continue;
            }
            events.push(json!({"agent": {"id": agent.id, "incarnation": agent.incarnation}, "conversation_id": on, "seq": message_seq}));
        }
    }
    Ok(json!({"events": events, "next_cursor": sync::cursor(through)}))
}

/// An account is deleted: its agents' drafts and subscriptions go.
pub fn purge(transaction: &Transaction<'_>, account: &str) -> rusqlite::Result<()> {
    transaction.execute("DELETE FROM dm_agent_drafts WHERE account = ?1", [account])?;
    transaction.execute("DELETE FROM dm_agent_subscriptions WHERE account = ?1", [account])?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::super::bodies::tests::{opened, sealed};
    use super::super::contacts::tests::World;
    use super::super::messages::tests::group_of;
    use super::*;
    use crate::grants::Put;

    const MAC: &str = "mac";

    fn june() -> Actor {
        Actor { id: "june".into(), incarnation: "inc-1".into() }
    }

    fn as_agent(agent: Actor) -> Caller {
        Caller::Backend { account_id: "ana".into(), claktory_id: MAC.into(), epoch: 1, agent: Some(agent) }
    }

    fn backend() -> Caller {
        Caller::Backend { account_id: "ana".into(), claktory_id: MAC.into(), epoch: 1, agent: None }
    }

    fn call(world: &mut World, caller: &Caller, method: &str, route: &str, body: Value) -> (u16, Value) {
        let bytes = if body.is_null() { Vec::new() } else { body.to_string().into_bytes() };
        super::super::identity::route(&mut world.db, &world.keys, method, route, caller, &bytes, world.now).expect("a route")
    }

    /// Ana and Bo, friends: their conversation and its generation.
    fn friends(world: &mut World, a: &str, b: &str) -> (String, i64) {
        let code = world.code(b);
        let request = world.send(a, &code);
        let (status, contact) = world.accept(b, &request);
        assert_eq!(status, 200, "{contact}");
        (contact["conversation_id"].as_str().expect("id").to_owned(), contact["generation"].as_i64().expect("generation"))
    }

    fn post(world: &mut World, account: &str, conversation: &str, generation: i64, text: &str) -> i64 {
        world.actions += 1;
        let (group, epoch) = group_of(world, conversation, generation);
        let send = json!({"client_message_id": format!("message-{:018}", world.actions), "contact_generation": generation, "envelope": sealed(&group, epoch, text)});
        let (status, receipt) = world.call(account, "POST", &format!("conversations/{conversation}/messages"), send);
        assert_eq!(status, 200, "{receipt}");
        receipt["seq"].as_i64().expect("seq")
    }

    fn grant(world: &mut World, scopes: &[&str], conversations: &[&str]) {
        let put = Put {
            app: APP.into(),
            agent: june(),
            scopes: scopes.iter().map(|scope| (*scope).to_owned()).collect(),
            resources: conversations.iter().map(|id| conversation(id)).collect(),
            expires_ms: 0,
        };
        grants::put(&mut world.db, &backend(), put, world.now).expect("granted");
    }

    fn texts(page: &Value) -> Vec<String> {
        page["messages"].as_array().expect("messages").iter().map(|m| opened(&m["envelope"])).collect()
    }

    /// An agent reads only what it was given: the conversations its grant
    /// names, from when it was given, in small pages, never moving what
    /// the person has read.
    #[test]
    fn an_agent_reads_only_its_conversations_from_its_grant_on() {
        let mut world = World::new();
        world.name("bo", "Bo");
        let (c1, generation) = friends(&mut world, "ana", "bo");
        let (c2, _) = friends(&mut world, "ana", "cy");
        post(&mut world, "bo", &c1, generation, "before the grant");
        let june = as_agent(june());

        // No grant: nothing.
        assert_eq!(call(&mut world, &june, "GET", "agent/conversations", Value::Null).0, 403);
        assert_eq!(call(&mut world, &june, "GET", &format!("agent/conversations/{c1}/messages"), Value::Null).0, 403);
        // A device or the backend itself can't use an agent's routes.
        assert_eq!(world.call("ana", "GET", "agent/conversations", Value::Null).0, 403);
        assert_eq!(call(&mut world, &backend(), "GET", "agent/conversations", Value::Null).0, 403);

        world.now += 1_000;
        grant(&mut world, &["dm.read"], &[&c1]);
        world.now += 1_000;
        post(&mut world, "bo", &c1, generation, "after it");
        post(&mut world, "ana", &c1, generation, "and Ana's");

        let (status, listed) = call(&mut world, &june, "GET", "agent/conversations", Value::Null);
        assert_eq!(status, 200, "{listed}");
        let listed = listed["conversations"].as_array().expect("a list").clone();
        assert_eq!(listed.len(), 1, "only the one granted: {listed:?}");
        assert_eq!(listed[0]["id"], json!(c1));
        assert_eq!(listed[0]["with"]["name"], json!("Bo"));
        assert!(listed[0]["with"]["tag"].as_str().is_some_and(|tag| !tag.is_empty()), "{listed:?}");
        assert_eq!(listed[0]["from_ms"], json!(world.now - 1_000));
        assert!(listed[0].get("view").is_none(), "nothing of the person's own view");

        let (status, page) = call(&mut world, &june, "GET", &format!("agent/conversations/{c1}/messages"), Value::Null);
        assert_eq!(status, 200, "{page}");
        assert_eq!(texts(&page), ["after it", "and Ana's"]);
        assert_eq!(call(&mut world, &june, "GET", &format!("agent/conversations/{c2}/messages"), Value::Null).0, 403);
        // Reading moved nothing the person has read.
        let read: Option<i64> = world
            .db
            .query_row("SELECT read_seq FROM dm_views WHERE account = 'ana' AND conversation_id = ?1", [&c1], |row| row.get(0))
            .optional()
            .expect("sql");
        assert!(read.unwrap_or(0) == 0, "read_seq {read:?}");

        // A page stays under 64 KiB.
        for _ in 0..6 {
            post(&mut world, "bo", &c1, generation, &"x".repeat(16 * 1024));
        }
        let (_, page) = call(&mut world, &june, "GET", &format!("agent/conversations/{c1}/messages?after=0"), Value::Null);
        let bytes: usize = texts(&page).iter().map(String::len).sum();
        assert!(bytes <= AGENT_PAGE_BYTES && page["has_more"] == json!(true), "{bytes}");

        // Taken back: refused at once.
        let id: String = world.db.query_row("SELECT id FROM grants", [], |row| row.get(0)).expect("the grant");
        grants::revoke(&mut world.db, &backend(), &id, world.now).expect("revoked");
        assert_eq!(call(&mut world, &june, "GET", &format!("agent/conversations/{c1}/messages"), Value::Null).0, 403);
    }

    /// A draft waits for the person: kept once per id, revised while open,
    /// a few at a time, only in an ongoing friendship of the generation the
    /// agent saw, and closed when the person uses or dismisses it.
    #[test]
    fn an_agents_draft_waits_for_the_person() {
        let mut world = World::new();
        let (c1, generation) = friends(&mut world, "ana", "bo");
        let june = as_agent(june());
        // Ana's Claktory's channel: the group its agents' drafts are sealed in.
        let channel = vec![4u8; 16];
        world
            .db
            .execute(
                "INSERT INTO dm_groups (group_id, kind, subject, account_id, claktory_id, epoch, last_seq, group_info, created)
                 VALUES (?1, 'channel', 'channel:ana:mac', 'ana', 'mac', 0, 0, x'00', 0)",
                [&channel],
            )
            .expect("a channel");
        let route = |id: &str| format!("conversations/{c1}/agent-drafts/{id}");
        let ask = |text: &str, generation: i64| json!({"contact_generation": generation, "envelope": sealed(&channel, 0, text), "agent_name": "June\u{202e}", "request_nonce": "n"});
        let first = "draft-000000000000000000001";

        assert_eq!(call(&mut world, &june, "PUT", &route(first), ask("Hi", generation)).0, 403, "no grant");
        grant(&mut world, &["dm.read"], &[&c1]);
        assert_eq!(call(&mut world, &june, "PUT", &route(first), ask("Hi", generation)).0, 403, "reading isn't drafting");
        grant(&mut world, &["dm.draft"], &[&c1]);

        let (status, kept) = call(&mut world, &june, "PUT", &route(first), ask("Hi Bo", generation));
        assert_eq!(status, 200, "{kept}");
        assert_eq!(kept["revision"], json!(1));
        let draft_id = kept["draft_id"].as_str().expect("an id").to_owned();
        // A retry is the same; other text revises it.
        assert_eq!(call(&mut world, &june, "PUT", &route(first), ask("Hi Bo", generation)).1["revision"], json!(1));
        let (_, revised) = call(&mut world, &june, "PUT", &route(first), ask("Hi Bo!", generation));
        assert_eq!((revised["draft_id"].as_str(), revised["revision"].as_i64()), (Some(draft_id.as_str()), Some(2)));
        assert_eq!(call(&mut world, &june, "PUT", &route(first), ask("Hi", generation + 1)).0, 409, "another generation");

        // The person's devices see it, under the agent's name as shown.
        let (status, listed) = world.call("ana", "GET", &format!("conversations/{c1}/agent-drafts"), Value::Null);
        assert_eq!(status, 200, "{listed}");
        assert_eq!(opened(&listed["drafts"][0]["envelope"]), "Hi Bo!");
        assert_eq!(listed["drafts"][0]["agent"]["name"], json!("June"));
        assert_eq!(world.call("bo", "GET", &format!("conversations/{c1}/agent-drafts"), Value::Null).1["drafts"], json!([]));
        // And the sync snapshot says who left one where, without its text.
        let (status, snapshot) = world.call("ana", "GET", "sync", Value::Null);
        assert_eq!(status, 200, "{snapshot}");
        let open = &snapshot["agent_drafts"];
        assert_eq!(open.as_array().map(Vec::len), Some(1), "{snapshot}");
        assert_eq!((open[0]["id"].as_str(), open[0]["conversation_id"].as_str()), (Some(draft_id.as_str()), Some(c1.as_str())));
        assert_eq!((open[0]["agent"]["name"].as_str(), open[0]["revision"].as_i64()), (Some("June"), Some(2)));
        assert!(open[0].get("body").is_none() && !snapshot["agent_drafts"].to_string().contains("Hi Bo"), "{open}");
        assert_eq!(world.call("bo", "GET", "sync", Value::Null).1["agent_drafts"], json!([]));
        let events: i64 = world
            .db
            .query_row("SELECT COUNT(*) FROM dm_account_events WHERE account = 'ana' AND kind = 'agent_draft'", [], |row| row.get(0))
            .expect("sql");
        assert_eq!(events, 2, "one for each change");

        // Three open at most (a minute on: five a minute is the quota).
        world.now += 60_000;
        for n in 2..=3 {
            assert_eq!(call(&mut world, &june, "PUT", &route(&format!("draft-00000000000000000000{n}")), ask("More", generation)).0, 200);
        }
        assert_eq!(call(&mut world, &june, "PUT", &route("draft-000000000000000000004"), ask("More", generation)).0, 429);

        // The person dismisses one: it's closed for the agent too.
        world.now += 60_000;
        assert_eq!(world.call("ana", "DELETE", &format!("conversations/{c1}/agent-drafts/{draft_id}"), Value::Null).0, 204);
        assert_eq!(call(&mut world, &june, "PUT", &route(first), ask("Again", generation)).0, 410);
        assert_eq!(call(&mut world, &june, "PUT", &route("draft-000000000000000000004"), ask("More", generation)).0, 200);
        // An agent can't list or close the person's drafts.
        assert_eq!(call(&mut world, &june, "GET", &format!("conversations/{c1}/agent-drafts"), Value::Null).0, 403);

        // Ana's account deleted: its agents' drafts go with it.
        super::super::identity::purge(&mut world.db, &world.keys, "ana", world.now).expect("purged");
        let left: i64 = world.db.query_row("SELECT COUNT(*) FROM dm_agent_drafts", [], |row| row.get(0)).expect("sql");
        assert_eq!(left, 0);
    }

    /// Only the other person's messages, in a conversation an agent follows
    /// with a live grant to read and follow it, sent after the grant began,
    /// reach the backend's events: ids, never text.
    #[test]
    fn events_wake_only_followers_with_a_live_grant() {
        let mut world = World::new();
        let (c1, generation) = friends(&mut world, "ana", "bo");
        let june = as_agent(june());
        let (status, start) = call(&mut world, &backend(), "GET", "agent/events", Value::Null);
        assert_eq!(status, 200, "{start}");
        let cursor = start["next_cursor"].as_str().expect("a cursor").to_owned();
        let events =
            |world: &mut World, cursor: &str| call(world, &backend(), "GET", &format!("agent/events?since={cursor}&wait=1"), Value::Null);

        grant(&mut world, &["dm.subscribe"], &[&c1]);
        assert_eq!(call(&mut world, &june, "PUT", &format!("agent/subscriptions/{c1}"), json!({})).0, 403, "following needs reading");
        grant(&mut world, &["dm.read", "dm.subscribe"], &[&c1]);
        assert_eq!(call(&mut world, &june, "PUT", &format!("agent/subscriptions/{c1}"), json!({})).0, 204);
        world.now += 1_000;

        let theirs = post(&mut world, "bo", &c1, generation, "Are you there?");
        post(&mut world, "ana", &c1, generation, "Yes");
        let (status, got) = events(&mut world, &cursor);
        assert_eq!(status, 200, "{got}");
        assert_eq!(got["events"], json!([{"agent": {"id": "june", "incarnation": "inc-1"}, "conversation_id": c1, "seq": theirs}]));
        assert!(!got.to_string().contains("Are you there"), "ids only");
        let cursor = got["next_cursor"].as_str().expect("a cursor").to_owned();

        // Another Claktory of the account hears nothing of June's.
        let other = Caller::Backend { account_id: "ana".into(), claktory_id: "server".into(), epoch: 1, agent: None };
        assert_eq!(
            call(&mut world, &other, "GET", &format!("agent/events?since={}", start["next_cursor"].as_str().expect("c")), Value::Null).1["events"],
            json!([])
        );

        // Not following, or the grant taken back: nothing.
        assert_eq!(call(&mut world, &june, "DELETE", &format!("agent/subscriptions/{c1}"), Value::Null).0, 204);
        post(&mut world, "bo", &c1, generation, "Hello?");
        let (_, got) = events(&mut world, &cursor);
        assert_eq!(got["events"], json!([]));
        let cursor = got["next_cursor"].as_str().expect("a cursor").to_owned();
        assert_eq!(call(&mut world, &june, "PUT", &format!("agent/subscriptions/{c1}"), json!({})).0, 204);
        let id: String = world.db.query_row("SELECT id FROM grants", [], |row| row.get(0)).expect("the grant");
        grants::revoke(&mut world.db, &backend(), &id, world.now).expect("revoked");
        post(&mut world, "bo", &c1, generation, "Still?");
        assert_eq!(events(&mut world, &cursor).1["events"], json!([]));
        assert_eq!(events(&mut world, "c1.garbage").0, 410, "a cursor never handed out");
    }
}
