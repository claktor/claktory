//! Contacts and consent (D1b): friend requests, friends and blocks. Two
//! people become friends only when one sends a request with the other's
//! friend ID and the other accepts it, each from their own signed-in device;
//! nothing an agent or a Claktory does adds a friend.
//!
//! A request carries no words of the sender's: only who sent it (their own
//! name and avatar, shown to the person it reached) and when. The sender
//! learns nothing about the code they sent: every well-formed request is
//! answered the same way, 202 and a receipt, whether the code is someone's,
//! retired, their own, or its owner has requests off, has blocked them, has
//! one from them already, is already their friend, or sent them one first.
//! The receipt reads "pending" until it's accepted, cancelled or expires (30
//! days on), and a request that didn't reach anyone is never delivered later.
//!
//! A person who declined someone can reopen to them before the 30 days are
//! up (`declines`): their requests reach the person again, and the one
//! declined, if it's still open to them, waits for an answer again.
//!
//! Everything that changes consent (accept, decline, reopen, block, remove,
//! cancel) runs in one IMMEDIATE transaction that reads the rows it depends on inside
//! it, so of two racing changes the first to commit wins and the second sees
//! it.

use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};
use serde::Deserialize;
use serde_json::{Value, json};

use super::identity::{self, BASE32, LOOKUP, Refusal, body, hex, invalid, random, shown_profile, take};
use super::keys::{Field, Keys, Refused};
use super::sync::{self, Kind};
use crate::grants::{self, Resource};

pub const SCHEMA: &str = "
-- Friend requests. `target` is set only for one that reached someone
-- (`delivered`); a request that reached no one keeps no target and is never
-- delivered. state: pending, accepted, cancelled, declined, closed (its
-- target reset their friend ID, turned requests off, blocked, or they became
-- friends another way). The sender sees pending for all but accepted and
-- cancelled, until it expires.
CREATE TABLE IF NOT EXISTS dm_requests (
    id TEXT PRIMARY KEY,
    sender TEXT NOT NULL,
    action_id TEXT NOT NULL,
    target TEXT,
    code_generation INTEGER NOT NULL,
    delivered INTEGER NOT NULL,
    state TEXT NOT NULL,
    contact_id TEXT,
    created INTEGER NOT NULL,
    expires INTEGER NOT NULL,
    UNIQUE (sender, action_id)
);
CREATE INDEX IF NOT EXISTS dm_requests_incoming ON dm_requests (target, state);
CREATE INDEX IF NOT EXISTS dm_requests_expiry ON dm_requests (expires);
-- Friends: two people (`low` < `high`). A pair has one active contact at a
-- time; one removed or blocked has `ended`, and reconnecting makes a new one
-- (the next generation, a new conversation), never the old again.
CREATE TABLE IF NOT EXISTS dm_contacts (
    id TEXT PRIMARY KEY,
    low TEXT NOT NULL,
    high TEXT NOT NULL,
    generation INTEGER NOT NULL,
    conversation_id TEXT NOT NULL UNIQUE,
    request_id TEXT NOT NULL,
    created INTEGER NOT NULL,
    ended INTEGER
);
CREATE UNIQUE INDEX IF NOT EXISTS dm_contacts_active ON dm_contacts (low, high) WHERE ended IS NULL;
CREATE INDEX IF NOT EXISTS dm_contacts_high ON dm_contacts (high);
-- Whom a person blocked: by account, whatever their friend ID.
CREATE TABLE IF NOT EXISTS dm_blocks (
    id TEXT PRIMARY KEY,
    blocker TEXT NOT NULL,
    blocked TEXT NOT NULL,
    created INTEGER NOT NULL,
    UNIQUE (blocker, blocked)
);
CREATE INDEX IF NOT EXISTS dm_blocks_blocked ON dm_blocks (blocked);
-- How each of an ended friendship's two people showed the other when it
-- ended (`shown`: their name and avatar, sealed with the conversation's
-- key): what its conversation shows from then on, never their profile as it
-- changes after.
CREATE TABLE IF NOT EXISTS dm_ended_views (
    conversation_id TEXT NOT NULL,
    viewer TEXT NOT NULL,
    shown BLOB NOT NULL,
    PRIMARY KEY (conversation_id, viewer)
);
-- A declined sender's requests to that person reach no one until `until`.
CREATE TABLE IF NOT EXISTS dm_declines (
    recipient TEXT NOT NULL,
    sender TEXT NOT NULL,
    until INTEGER NOT NULL,
    PRIMARY KEY (recipient, sender)
);
";

const DAY_MS: i64 = 86_400_000;
/// How long a request, and its receipt, lasts.
const EXPIRY_MS: i64 = 30 * DAY_MS;
/// How long a decline keeps that sender's requests from reaching the person.
const DECLINE_MS: i64 = 30 * DAY_MS;
/// Requests a person may send a day: fewer in their first week.
const REQUESTS_NEW: i64 = 5;
const REQUESTS: i64 = 20;
const NEW_FOR_MS: i64 = 7 * DAY_MS;
/// Requests waiting for one person at most; past it, new ones reach no one.
const MAX_INCOMING: i64 = 100;
/// Accepts, declines, blocks, unblocks, removals and cancels an hour.
const ACTIONS_PER_HOUR: i64 = 120;
/// What every well-formed request is answered with.
const SENT: &str = "Request sent. If that's their friend ID, they'll see it.";

fn not_found() -> Refusal {
    Refusal(404, "not_found", "There's nothing here.".to_owned())
}

fn gone() -> Refusal {
    Refusal(410, "gone", "This request isn't open anymore.".to_owned())
}

/// A new id: 128 random bits, in hex.
fn new_id() -> String {
    hex(&random::<16>())
}

/// The two accounts of a pair, in the order contacts keep them.
fn pair<'a>(a: &'a str, b: &'a str) -> (&'a str, &'a str) {
    if a < b { (a, b) } else { (b, a) }
}

/// How `viewer` tells `other` apart from someone with the same name: six
/// base32 letters, the same across their resets and renames, different for
/// every viewer, and nothing anyone can use or look up.
pub fn contact_tag(db: &Connection, keys: &Keys, viewer: &str, other: &str, now_ms: i64) -> Result<String, Refusal> {
    let digest = keyed(db, keys, "claktory-dms contact tag v1", viewer, other, now_ms)?;
    // 30 bits: the first six letters.
    let bits = u32::from_be_bytes([digest[0], digest[1], digest[2], digest[3]]) >> 2;
    Ok((0..6).rev().map(|at| BASE32[((bits >> (at * 5)) & 31) as usize] as char).collect())
}

/// HMAC(lookup key, label, viewer, other), each part length-prefixed.
fn keyed(db: &Connection, keys: &Keys, label: &str, viewer: &str, other: &str, now_ms: i64) -> Result<[u8; 32], Refusal> {
    use hmac::{Hmac, KeyInit, Mac};
    let key = keys.hash_key(db, LOOKUP, now_ms)?;
    let mut mac = <Hmac<sha2::Sha256> as KeyInit>::new_from_slice(key.as_slice()).expect("any key length");
    for part in [label.as_bytes(), viewer.as_bytes(), other.as_bytes()] {
        mac.update(&(part.len() as u64).to_be_bytes());
        mac.update(part);
    }
    Ok(mac.finalize().into_bytes().into())
}

/// The id a person reopens a decline by: who declined whom, as a keyed hash,
/// since the person never sees anyone's account.
fn decline_id(db: &Connection, keys: &Keys, recipient: &str, sender: &str, now_ms: i64) -> Result<String, Refusal> {
    Ok(hex(&keyed(db, keys, "claktory-dms decline v1", recipient, sender, now_ms)?[..16]))
}

/// Someone as a person sees them: their own name and avatar, and their tag.
pub(super) fn person(db: &Connection, keys: &Keys, viewer: &str, other: &str, now_ms: i64) -> Result<Value, Refusal> {
    let (name, avatar_seed) = shown_profile(db, keys, other)?.unwrap_or_default();
    Ok(json!({"name": name, "avatar_seed": avatar_seed, "tag": contact_tag(db, keys, viewer, other, now_ms)?}))
}

/// Closes the requests waiting for `account` (they reset their friend ID or
/// turned requests off): none of them can be accepted any more. Their
/// senders' receipts read as before.
pub fn close_incoming(transaction: &Transaction<'_>, account: &str) -> rusqlite::Result<usize> {
    transaction.execute("UPDATE dm_requests SET state = 'closed' WHERE target = ?1 AND state = 'pending'", [account])
}

/// Closes every pending request between two people, both ways.
fn close_between(transaction: &Transaction<'_>, a: &str, b: &str) -> rusqlite::Result<usize> {
    transaction.execute(
        "UPDATE dm_requests SET state = 'closed'
         WHERE state = 'pending' AND ((sender = ?1 AND target = ?2) OR (sender = ?2 AND target = ?1))",
        params![a, b],
    )
}

/// Ends a contact (removed or blocked): no more of their conversation, and
/// no agent of either keeps access to it. Each keeps how the other showed
/// them then, which is all their conversation shows of the other from now
/// on: a block or removal also ends seeing their name and avatar change.
fn end_contact(transaction: &Transaction<'_>, keys: &Keys, id: &str, now_ms: i64) -> Result<(), Refused> {
    let found: Option<(String, String, String)> = transaction
        .query_row("SELECT low, high, conversation_id FROM dm_contacts WHERE id = ?1 AND ended IS NULL", [id], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?))
        })
        .optional()?;
    let Some((low, high, conversation)) = found else { return Ok(()) };
    transaction.execute("UPDATE dm_contacts SET ended = ?2 WHERE id = ?1", params![id, now_ms])?;
    for (viewer, other) in [(&low, &high), (&high, &low)] {
        let (name, avatar_seed) = shown_profile(transaction, keys, other)?.unwrap_or_default();
        let shown = zeroize::Zeroizing::new(serde_json::to_vec(&json!({"name": name, "avatar_seed": avatar_seed})).unwrap_or_default());
        let record = format!("{conversation}/{viewer}");
        let field = Field { table: "dm_ended_views", record: &record, column: "shown" };
        let sealed = keys.seal(transaction, &super::bodies::purpose(&conversation), field, &shown, now_ms)?;
        transaction.execute(
            "INSERT INTO dm_ended_views (conversation_id, viewer, shown) VALUES (?1, ?2, ?3)
             ON CONFLICT (conversation_id, viewer) DO UPDATE SET shown = excluded.shown",
            params![conversation, viewer, sealed],
        )?;
    }
    for account in [&low, &high] {
        sync::emit(transaction, account, Kind::Contacts, "", id, 0, now_ms)?;
        sync::emit(transaction, account, Kind::Conversation, &conversation, id, 0, now_ms)?;
    }
    grants::drop_resource(transaction, "dms", &[&low, &high], &Resource { kind: "conversation".into(), id: conversation })?;
    Ok(())
}

/// How the other person of an ended friendship showed `viewer` when it
/// ended: their name and avatar seed. None if it isn't kept.
pub(super) fn ended_view(db: &Connection, keys: &Keys, conversation: &str, viewer: &str) -> Result<Option<(String, String)>, Refused> {
    let sealed: Option<Vec<u8>> = db
        .query_row("SELECT shown FROM dm_ended_views WHERE conversation_id = ?1 AND viewer = ?2", params![conversation, viewer], |row| {
            row.get(0)
        })
        .optional()?;
    let Some(sealed) = sealed else { return Ok(None) };
    let record = format!("{conversation}/{viewer}");
    let opened = keys.open(db, Field { table: "dm_ended_views", record: &record, column: "shown" }, &sealed)?;
    let shown: Value = serde_json::from_slice(opened.bytes()).map_err(|_| Refused::DoesntOpen)?;
    let text = |key: &str| shown[key].as_str().unwrap_or_default().to_owned();
    Ok(Some((text("name"), text("avatar_seed"))))
}

fn contact_json(db: &Connection, keys: &Keys, viewer: &str, id: &str, now_ms: i64) -> Result<Value, Refusal> {
    let (low, high, generation, conversation, created): (String, String, i64, String, i64) =
        db.query_row("SELECT low, high, generation, conversation_id, created FROM dm_contacts WHERE id = ?1", [id], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?))
        })?;
    let other = if low == viewer { high } else { low };
    let mut shown = person(db, keys, viewer, &other, now_ms)?;
    // A friend's account: their devices fetch its keys to make the
    // conversation's group (requests and blocks never show it).
    shown["account_id"] = json!(other);
    shown["id"] = json!(id);
    shown["conversation_id"] = json!(conversation);
    shown["generation"] = json!(generation);
    shown["since_ms"] = json!(created);
    Ok(shown)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Send {
    /// The friend ID, as typed or pasted.
    code: String,
    /// The device's id for this request, the same on every retry: 22 to 64
    /// letters, digits, - and _, at least 128 random bits.
    action_id: String,
    #[serde(default)]
    #[allow(dead_code)]
    request_nonce: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Nonce {
    #[serde(default)]
    #[allow(dead_code)]
    request_nonce: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Block {
    /// A request that reached the person, or a contact of theirs: whom to
    /// block, since they never see anyone's account.
    #[serde(default)]
    request_id: Option<String>,
    #[serde(default)]
    contact_id: Option<String>,
    #[serde(default)]
    #[allow(dead_code)]
    request_nonce: Option<String>,
}

fn valid_action_id(id: &str) -> bool {
    (22..=64).contains(&id.len()) && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

/// Whether `route` is this module's.
pub fn knows(method: &str, route: &str) -> bool {
    let parts: Vec<&str> = route.split('/').collect();
    matches!(
        (method, parts.as_slice()),
        ("POST" | "GET", ["requests"])
            | ("POST", ["requests", _, "accept" | "decline" | "cancel"])
            | ("GET" | "POST", ["blocks"])
            | ("DELETE", ["blocks", _])
            | ("GET", ["contacts"])
            | ("DELETE", ["contacts", _])
            | ("GET", ["declines"])
            | ("DELETE", ["declines", _])
    )
}

/// Answers one of this module's routes for `account`, from one of their
/// devices.
pub fn answer(
    db: &mut Connection,
    keys: &Keys,
    method: &str,
    route: &str,
    account: &str,
    bytes: &[u8],
    now_ms: i64,
) -> Result<(u16, Value), Refusal> {
    let transaction = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
    identity::ensure(&transaction, keys, account, now_ms)?;
    let parts: Vec<&str> = route.split('/').collect();
    let answer = match (method, parts.as_slice()) {
        ("POST", ["requests"]) => send(&transaction, keys, account, body(bytes)?, now_ms)?,
        ("GET", ["requests"]) => (200, requests(&transaction, keys, account, now_ms)?),
        ("POST", ["requests", id, action]) => {
            body::<Nonce>(bytes)?;
            take(&transaction, &format!("contacts:{account}"), 3600, ACTIONS_PER_HOUR, now_ms)?;
            match *action {
                "accept" => (200, accept(&transaction, keys, account, id, now_ms)?),
                "decline" => decline(&transaction, account, id, now_ms)?,
                _ => cancel(&transaction, account, id, now_ms)?,
            }
        }
        ("POST", ["blocks"]) => {
            let block: Block = body(bytes)?;
            take(&transaction, &format!("contacts:{account}"), 3600, ACTIONS_PER_HOUR, now_ms)?;
            (200, self::block(&transaction, keys, account, block, now_ms)?)
        }
        ("GET", ["blocks"]) => (200, blocks(&transaction, keys, account, now_ms)?),
        ("DELETE", ["blocks", id]) => {
            take(&transaction, &format!("contacts:{account}"), 3600, ACTIONS_PER_HOUR, now_ms)?;
            if transaction.execute("DELETE FROM dm_blocks WHERE id = ?1 AND blocker = ?2", params![id, account])? == 0 {
                return Err(not_found());
            }
            sync::emit(&transaction, account, Kind::Blocks, "", id, 0, now_ms)?;
            (204, Value::Null)
        }
        ("GET", ["contacts"]) => (200, contacts(&transaction, keys, account, now_ms)?),
        ("DELETE", ["contacts", id]) => {
            take(&transaction, &format!("contacts:{account}"), 3600, ACTIONS_PER_HOUR, now_ms)?;
            let theirs: bool = transaction.query_row(
                "SELECT COUNT(*) > 0 FROM dm_contacts WHERE id = ?1 AND ended IS NULL AND (low = ?2 OR high = ?2)",
                params![id, account],
                |row| row.get(0),
            )?;
            if !theirs {
                return Err(not_found());
            }
            end_contact(&transaction, keys, id, now_ms)?;
            (204, Value::Null)
        }
        ("GET", ["declines"]) => (200, declines(&transaction, keys, account, now_ms)?),
        ("DELETE", ["declines", id]) => {
            take(&transaction, &format!("contacts:{account}"), 3600, ACTIONS_PER_HOUR, now_ms)?;
            let reopened = reopen(&transaction, keys, account, id, now_ms)?;
            sync::emit(&transaction, account, Kind::Requests, "", "", 0, now_ms)?;
            (200, reopened)
        }
        _ => return Err(not_found()),
    };
    transaction.commit()?;
    Ok(answer)
}

/// Whether a request from `sender` would reach `target` now: they take
/// requests, neither blocked the other, they didn't decline `sender` lately,
/// aren't friends, have nothing waiting between them either way, and have
/// fewer than 100 waiting. Every lookup runs whatever the earlier ones found
/// (`target` may be no one), so the time it takes says nothing.
fn open_to(transaction: &Transaction<'_>, sender: &str, target: &str, now_ms: i64) -> rusqlite::Result<bool> {
    let requests_on: bool = transaction
        .query_row("SELECT requests_on FROM dm_people WHERE account_id = ?1", [target], |row| row.get(0))
        .optional()?
        .unwrap_or(false);
    let blocked: bool = transaction.query_row(
        "SELECT COUNT(*) > 0 FROM dm_blocks WHERE (blocker = ?1 AND blocked = ?2) OR (blocker = ?2 AND blocked = ?1)",
        params![sender, target],
        |row| row.get(0),
    )?;
    let declined: bool = transaction.query_row(
        "SELECT COUNT(*) > 0 FROM dm_declines WHERE recipient = ?1 AND sender = ?2 AND until > ?3",
        params![target, sender, now_ms],
        |row| row.get(0),
    )?;
    let (low, high) = pair(sender, target);
    let friends: bool = transaction.query_row(
        "SELECT COUNT(*) > 0 FROM dm_contacts WHERE low = ?1 AND high = ?2 AND ended IS NULL",
        params![low, high],
        |row| row.get(0),
    )?;
    let waiting: bool = transaction.query_row(
        "SELECT COUNT(*) > 0 FROM dm_requests WHERE state = 'pending' AND expires > ?3
         AND ((sender = ?1 AND target = ?2) OR (sender = ?2 AND target = ?1))",
        params![sender, target, now_ms],
        |row| row.get(0),
    )?;
    let incoming: i64 = transaction.query_row(
        "SELECT COUNT(*) FROM dm_requests WHERE target = ?1 AND state = 'pending' AND expires > ?2",
        params![target, now_ms],
        |row| row.get(0),
    )?;
    Ok(requests_on && !blocked && !declined && !friends && !waiting && incoming < MAX_INCOMING)
}

/// Sends a request: always 202 and a receipt, whoever the code is (or
/// isn't). Every one runs the same queries, whatever they find.
fn send(transaction: &Transaction<'_>, keys: &Keys, sender: &str, send: Send, now_ms: i64) -> Result<(u16, Value), Refusal> {
    if !valid_action_id(&send.action_id) {
        return Err(invalid("action_id has to be 22 to 64 letters, digits, - and _."));
    }
    let Some(code) = identity::parse_code(&send.code) else {
        return Err(invalid("That isn't a friend ID. Check it and try again."));
    };
    // A retry: the same receipt.
    let earlier: Option<(String, i64, i64)> = transaction
        .query_row(
            "SELECT id, created, expires FROM dm_requests WHERE sender = ?1 AND action_id = ?2",
            params![sender, send.action_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()?;
    if let Some((id, created, expires)) = earlier {
        return Ok((202, receipt(&id, created, expires)));
    }
    let joined: i64 = transaction.query_row("SELECT created FROM dm_people WHERE account_id = ?1", [sender], |row| row.get(0))?;
    let limit = if now_ms - joined < NEW_FOR_MS { REQUESTS_NEW } else { REQUESTS };
    take(transaction, &format!("requests:{sender}"), 86_400, limit, now_ms)?;
    transaction.execute("DELETE FROM dm_requests WHERE id IN (SELECT id FROM dm_requests WHERE expires <= ?1 LIMIT 100)", [now_ms])?;

    let digest = identity::code_digest(keys, transaction, &code, now_ms)?;
    let found: Option<(String, i64)> = transaction
        .query_row("SELECT account_id, generation FROM dm_friend_codes WHERE digest = ?1 AND retired IS NULL", [&digest], |row| {
            Ok((row.get(0)?, row.get(1)?))
        })
        .optional()?;
    let target = found.as_ref().map(|(account, _)| account.as_str()).unwrap_or("");
    let open = open_to(transaction, sender, target, now_ms)?;
    let reaches = found.is_some() && target != sender && open;
    let (target, generation) = match (&found, reaches) {
        (Some((account, generation)), true) => (Some(account.as_str()), *generation),
        _ => (None, 0),
    };
    let id = new_id();
    let expires = now_ms + EXPIRY_MS;
    transaction.execute(
        "INSERT INTO dm_requests (id, sender, action_id, target, code_generation, delivered, state, created, expires)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'pending', ?7, ?8)",
        params![id, sender, send.action_id, target, generation, reaches, now_ms, expires],
    )?;
    // The sender's devices hear of their receipt; the target's of a request
    // only if it reached them, by a statement that runs either way.
    sync::emit(transaction, sender, Kind::Requests, "", &id, 0, now_ms)?;
    sync::emit_if(transaction, reaches, target.unwrap_or(""), Kind::Requests, "", &id, 0, now_ms)?;
    Ok((202, receipt(&id, now_ms, expires)))
}

fn receipt(id: &str, created: i64, expires: i64) -> Value {
    json!({"receipt": {"id": id, "state": "pending", "created_ms": created, "expires_ms": expires}, "message": SENT})
}

/// The person's requests: those waiting for them (who sent each), and the
/// receipts of their own (never whom they reached).
pub(super) fn requests(transaction: &Transaction<'_>, keys: &Keys, account: &str, now_ms: i64) -> Result<Value, Refusal> {
    let incoming_rows: Vec<(String, String, i64, i64)> = {
        let mut statement = transaction.prepare(
            "SELECT id, sender, created, expires FROM dm_requests
             WHERE target = ?1 AND delivered = 1 AND state = 'pending' AND expires > ?2 ORDER BY created DESC",
        )?;
        statement
            .query_map(params![account, now_ms], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)))?
            .collect::<Result<_, _>>()?
    };
    let mut incoming = Vec::new();
    for (id, sender, created, expires) in incoming_rows {
        incoming.push(
            json!({"id": id, "from": person(transaction, keys, account, &sender, now_ms)?, "created_ms": created, "expires_ms": expires}),
        );
    }
    let outgoing_rows: Vec<(String, String, Option<String>, i64, i64)> = {
        let mut statement = transaction.prepare(
            "SELECT id, state, contact_id, created, expires FROM dm_requests
             WHERE sender = ?1 AND state != 'cancelled' AND expires > ?2 ORDER BY created DESC",
        )?;
        statement
            .query_map(params![account, now_ms], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?)))?
            .collect::<Result<_, _>>()?
    };
    let outgoing: Vec<Value> = outgoing_rows
        .into_iter()
        .map(|(id, state, contact, created, expires)| {
            // Declined, closed or reaching no one: pending, as to the sender
            // they all are.
            let mut shown = json!({"id": id, "state": "pending", "created_ms": created, "expires_ms": expires});
            if state == "accepted" {
                shown["state"] = json!("accepted");
                shown["contact_id"] = json!(contact);
            }
            shown
        })
        .collect();
    Ok(json!({"incoming": incoming, "outgoing": outgoing}))
}

/// The request `id` waiting for `account`, if it's theirs: its sender and
/// state.
fn incoming(transaction: &Transaction<'_>, account: &str, id: &str, now_ms: i64) -> Result<(String, String, Option<String>), Refusal> {
    let found: Option<(String, String, Option<String>, i64)> = transaction
        .query_row(
            "SELECT sender, state, contact_id, expires FROM dm_requests WHERE id = ?1 AND target = ?2 AND delivered = 1",
            params![id, account],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .optional()?;
    let Some((sender, state, contact, expires)) = found else { return Err(not_found()) };
    if state == "pending" && expires <= now_ms {
        return Err(gone());
    }
    Ok((sender, state, contact))
}

/// Accepts a request: the two become friends, with a new conversation.
/// Accepting it again answers the same contact.
fn accept(transaction: &Transaction<'_>, keys: &Keys, account: &str, id: &str, now_ms: i64) -> Result<Value, Refusal> {
    let (sender, state, contact) = incoming(transaction, account, id, now_ms)?;
    match (state.as_str(), contact) {
        ("accepted", Some(contact)) => return contact_json(transaction, keys, account, &contact, now_ms),
        ("pending", _) => {}
        _ => return Err(gone()),
    }
    // Read here, in this transaction: a block, or the sender gone, since.
    let blocked: bool = transaction.query_row(
        "SELECT COUNT(*) > 0 FROM dm_blocks WHERE (blocker = ?1 AND blocked = ?2) OR (blocker = ?2 AND blocked = ?1)",
        params![account, sender],
        |row| row.get(0),
    )?;
    let sender_here: bool =
        transaction.query_row("SELECT COUNT(*) > 0 FROM dm_people WHERE account_id = ?1", [&sender], |row| row.get(0))?;
    if blocked || !sender_here {
        transaction.execute("UPDATE dm_requests SET state = 'closed' WHERE id = ?1", [id])?;
        return Err(gone());
    }
    let (low, high) = pair(account, &sender);
    let active: Option<String> = transaction
        .query_row("SELECT id FROM dm_contacts WHERE low = ?1 AND high = ?2 AND ended IS NULL", params![low, high], |row| row.get(0))
        .optional()?;
    let contact = match active {
        Some(contact) => contact,
        None => {
            let generation: i64 = transaction.query_row(
                "SELECT COALESCE(MAX(generation), 0) + 1 FROM dm_contacts WHERE low = ?1 AND high = ?2",
                params![low, high],
                |row| row.get(0),
            )?;
            let contact = new_id();
            transaction.execute(
                "INSERT INTO dm_contacts (id, low, high, generation, conversation_id, request_id, created) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![contact, low, high, generation, new_id(), id, now_ms],
            )?;
            contact
        }
    };
    // Any other request between them, either way, is done with: the sender
    // of a crossing one sees it pending as before.
    close_between(transaction, account, &sender)?;
    transaction.execute("UPDATE dm_requests SET state = 'accepted', contact_id = ?2 WHERE id = ?1", params![id, contact])?;
    let conversation: String =
        transaction.query_row("SELECT conversation_id FROM dm_contacts WHERE id = ?1", [&contact], |row| row.get(0))?;
    for person in [account, sender.as_str()] {
        sync::emit(transaction, person, Kind::Requests, "", id, 0, now_ms)?;
        sync::emit(transaction, person, Kind::Contacts, "", &contact, 0, now_ms)?;
        sync::emit(transaction, person, Kind::Conversation, &conversation, &contact, 0, now_ms)?;
    }
    contact_json(transaction, keys, account, &contact, now_ms)
}

/// Declines a request: it goes, the sender isn't told, and their requests
/// reach this person again only after 30 days.
fn decline(transaction: &Transaction<'_>, account: &str, id: &str, now_ms: i64) -> Result<(u16, Value), Refusal> {
    let (sender, state, _) = incoming(transaction, account, id, now_ms)?;
    if state != "pending" {
        return Err(gone());
    }
    transaction.execute("UPDATE dm_requests SET state = 'declined' WHERE id = ?1", [id])?;
    sync::emit(transaction, account, Kind::Requests, "", id, 0, now_ms)?;
    transaction.execute(
        "INSERT INTO dm_declines (recipient, sender, until) VALUES (?1, ?2, ?3)
         ON CONFLICT (recipient, sender) DO UPDATE SET until = excluded.until",
        params![account, sender, now_ms + DECLINE_MS],
    )?;
    Ok((204, Value::Null))
}

/// Whom the person declined in the last 30 days, to reopen to.
fn declines(transaction: &Transaction<'_>, keys: &Keys, account: &str, now_ms: i64) -> Result<Value, Refusal> {
    transaction.execute("DELETE FROM dm_declines WHERE recipient = ?1 AND until <= ?2", params![account, now_ms])?;
    let rows: Vec<(String, i64)> = {
        let mut statement = transaction.prepare("SELECT sender, until FROM dm_declines WHERE recipient = ?1 ORDER BY until DESC")?;
        statement.query_map([account], |row| Ok((row.get(0)?, row.get(1)?)))?.collect::<Result<_, _>>()?
    };
    let mut declines = Vec::new();
    for (sender, until) in rows {
        declines.push(json!({
            "id": decline_id(transaction, keys, account, &sender, now_ms)?,
            "from": person(transaction, keys, account, &sender, now_ms)?,
            "until_ms": until,
        }));
    }
    Ok(json!({"declines": declines}))
}

/// Reopens to someone the person declined: their requests reach the person
/// again, and the request declined, if it hasn't expired and would still
/// reach them (the same friend ID, requests on, no block, not friends,
/// nothing else waiting between them), waits for an answer again. The
/// sender is told nothing either way.
fn reopen(transaction: &Transaction<'_>, keys: &Keys, account: &str, id: &str, now_ms: i64) -> Result<Value, Refusal> {
    let senders: Vec<String> = {
        let mut statement = transaction.prepare("SELECT sender FROM dm_declines WHERE recipient = ?1 AND until > ?2")?;
        statement.query_map(params![account, now_ms], |row| row.get(0))?.collect::<Result<_, _>>()?
    };
    let mut found = None;
    for sender in senders {
        if decline_id(transaction, keys, account, &sender, now_ms)? == id {
            found = Some(sender);
            break;
        }
    }
    let sender = found.ok_or_else(not_found)?;
    transaction.execute("DELETE FROM dm_declines WHERE recipient = ?1 AND sender = ?2", params![account, sender])?;
    let declined: Option<(String, i64, i64, i64)> = transaction
        .query_row(
            "SELECT id, code_generation, created, expires FROM dm_requests
             WHERE sender = ?1 AND target = ?2 AND delivered = 1 AND state = 'declined' AND expires > ?3
             ORDER BY created DESC LIMIT 1",
            params![sender, account, now_ms],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .optional()?;
    let Some((request, generation, created, expires)) = declined else { return Ok(json!({"request": null})) };
    let current: Option<i64> = transaction
        .query_row("SELECT generation FROM dm_friend_codes WHERE account_id = ?1 AND retired IS NULL", [account], |row| row.get(0))
        .optional()?;
    if current != Some(generation) || !open_to(transaction, &sender, account, now_ms)? {
        return Ok(json!({"request": null}));
    }
    transaction.execute("UPDATE dm_requests SET state = 'pending' WHERE id = ?1", [&request])?;
    Ok(
        json!({"request": {"id": request, "from": person(transaction, keys, account, &sender, now_ms)?, "created_ms": created, "expires_ms": expires}}),
    )
}

/// Cancels one of the person's own requests, unless it was accepted first.
fn cancel(transaction: &Transaction<'_>, account: &str, id: &str, now_ms: i64) -> Result<(u16, Value), Refusal> {
    let found: Option<(String, Option<String>, bool)> = transaction
        .query_row("SELECT state, target, delivered FROM dm_requests WHERE id = ?1 AND sender = ?2", params![id, account], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?))
        })
        .optional()?;
    let (state, target, delivered) = match found {
        Some((state, target, delivered)) => (Some(state), target, delivered),
        None => (None, None, false),
    };
    match state.as_deref() {
        None | Some("cancelled") => Err(not_found()),
        Some("accepted") => Err(Refusal(409, "accepted", "They accepted before you canceled. You're friends now.".to_owned())),
        Some(_) => {
            transaction.execute("UPDATE dm_requests SET state = 'cancelled' WHERE id = ?1", [id])?;
            sync::emit(transaction, account, Kind::Requests, "", id, 0, now_ms)?;
            let pending = delivered && state.as_deref() == Some("pending");
            sync::emit_if(transaction, pending, target.as_deref().unwrap_or(""), Kind::Requests, "", id, 0, now_ms)?;
            Ok((204, Value::Null))
        }
    }
}

/// Blocks whoever sent a request that reached the person, or a contact of
/// theirs: their requests reach the person no more, what was pending either
/// way closes, and their friendship ends.
fn block(transaction: &Transaction<'_>, keys: &Keys, account: &str, block: Block, now_ms: i64) -> Result<Value, Refusal> {
    let other: String = match (block.request_id, block.contact_id) {
        (Some(request), None) => transaction
            .query_row("SELECT sender FROM dm_requests WHERE id = ?1 AND target = ?2 AND delivered = 1", params![request, account], |row| {
                row.get(0)
            })
            .optional()?
            .ok_or_else(not_found)?,
        (None, Some(contact)) => {
            let found: Option<(String, String)> = transaction
                .query_row(
                    "SELECT low, high FROM dm_contacts WHERE id = ?1 AND ended IS NULL AND (low = ?2 OR high = ?2)",
                    params![contact, account],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .optional()?;
            let (low, high) = found.ok_or_else(not_found)?;
            if low == account { high } else { low }
        }
        _ => return Err(invalid("Name one request_id or contact_id.")),
    };
    transaction.execute(
        "INSERT INTO dm_blocks (id, blocker, blocked, created) VALUES (?1, ?2, ?3, ?4) ON CONFLICT (blocker, blocked) DO NOTHING",
        params![new_id(), account, other, now_ms],
    )?;
    close_between(transaction, account, &other)?;
    let (low, high) = pair(account, &other);
    let active: Option<String> = transaction
        .query_row("SELECT id FROM dm_contacts WHERE low = ?1 AND high = ?2 AND ended IS NULL", params![low, high], |row| row.get(0))
        .optional()?;
    if let Some(contact) = active {
        end_contact(transaction, keys, &contact, now_ms)?;
    }
    sync::emit(transaction, account, Kind::Blocks, "", "", 0, now_ms)?;
    sync::emit(transaction, account, Kind::Requests, "", "", 0, now_ms)?;
    let (id, created): (String, i64) =
        transaction.query_row("SELECT id, created FROM dm_blocks WHERE blocker = ?1 AND blocked = ?2", params![account, other], |row| {
            Ok((row.get(0)?, row.get(1)?))
        })?;
    blocked_json(transaction, keys, account, &id, &other, created, now_ms)
}

fn blocked_json(db: &Connection, keys: &Keys, account: &str, id: &str, other: &str, created: i64, now_ms: i64) -> Result<Value, Refusal> {
    let mut shown = person(db, keys, account, other, now_ms)?;
    shown["id"] = json!(id);
    shown["created_ms"] = json!(created);
    Ok(shown)
}

fn blocks(transaction: &Transaction<'_>, keys: &Keys, account: &str, now_ms: i64) -> Result<Value, Refusal> {
    let rows: Vec<(String, String, i64)> = {
        let mut statement = transaction.prepare("SELECT id, blocked, created FROM dm_blocks WHERE blocker = ?1 ORDER BY created DESC")?;
        statement.query_map([account], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?.collect::<Result<_, _>>()?
    };
    let mut blocks = Vec::new();
    for (id, other, created) in rows {
        blocks.push(blocked_json(transaction, keys, account, &id, &other, created, now_ms)?);
    }
    Ok(json!({"blocks": blocks}))
}

pub(super) fn contacts(transaction: &Transaction<'_>, keys: &Keys, account: &str, now_ms: i64) -> Result<Value, Refusal> {
    let ids: Vec<String> = {
        let mut statement =
            transaction.prepare("SELECT id FROM dm_contacts WHERE (low = ?1 OR high = ?1) AND ended IS NULL ORDER BY created DESC")?;
        statement.query_map([account], |row| row.get(0))?.collect::<Result<_, _>>()?
    };
    let mut contacts = Vec::new();
    for id in ids {
        contacts.push(contact_json(transaction, keys, account, &id, now_ms)?);
    }
    Ok(json!({"contacts": contacts}))
}

/// Forgets an account's part in requests, friends and blocks (it was
/// deleted). Requests others sent it read to them as before, reaching no
/// one; their friendships with it end, and their agents' access to those
/// conversations with them.
pub fn purge(transaction: &Transaction<'_>, keys: &Keys, account: &str, now_ms: i64) -> Result<(), Refused> {
    transaction.execute("DELETE FROM dm_requests WHERE sender = ?1", [account])?;
    transaction.execute(
        "UPDATE dm_requests SET target = NULL, delivered = 0, code_generation = 0,
         state = CASE state WHEN 'pending' THEN 'closed' ELSE state END WHERE target = ?1",
        [account],
    )?;
    let active: Vec<String> = {
        let mut statement = transaction.prepare("SELECT id FROM dm_contacts WHERE (low = ?1 OR high = ?1) AND ended IS NULL")?;
        statement.query_map([account], |row| row.get(0))?.collect::<Result<_, _>>()?
    };
    for contact in active {
        end_contact(transaction, keys, &contact, now_ms)?;
    }
    transaction.execute("DELETE FROM dm_ended_views WHERE viewer = ?1", [account])?;
    transaction.execute("DELETE FROM dm_blocks WHERE blocker = ?1 OR blocked = ?1", [account])?;
    transaction.execute("DELETE FROM dm_declines WHERE recipient = ?1 OR sender = ?1", [account])?;
    for limit in ["requests", "contacts"] {
        transaction.execute("DELETE FROM dm_limits WHERE key = ?1", [format!("{limit}:{account}")])?;
    }
    Ok(())
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;
    use crate::ipc::{Caller, Registration};

    pub(in crate::dms) const NOW: i64 = 1_795_183_320_000;

    pub(in crate::dms) struct World {
        pub db: Connection,
        pub keys: Keys,
        pub now: i64,
        pub actions: u32,
    }

    impl World {
        pub(in crate::dms) fn new() -> World {
            let db = Connection::open_in_memory().expect("db");
            let keys = Keys::from_bytes([7; 32]);
            keys.bind(&db).expect("bound");
            db.execute_batch(crate::grants::SCHEMA).expect("schema");
            db.execute_batch(identity::SCHEMA).expect("schema");
            db.execute_batch(SCHEMA).expect("schema");
            db.execute_batch(super::super::reports::SCHEMA).expect("schema");
            db.execute_batch(super::super::messages::SCHEMA).expect("schema");
            db.execute_batch(super::super::sync::SCHEMA).expect("schema");
            db.execute_batch(super::super::agents::SCHEMA).expect("schema");
            db.execute_batch(super::super::review::SCHEMA).expect("schema");
            db.execute_batch(super::super::leaves::SCHEMA).expect("schema");
            db.execute_batch(super::super::groups::SCHEMA).expect("schema");
            db.execute_batch(super::super::handoffs::SCHEMA).expect("schema");
            db.execute_batch(claktory_dm_crypto::storage::SCHEMA).expect("schema");
            db.pragma_update(None, "secure_delete", "ON").expect("secure_delete");
            World { db, keys, now: NOW, actions: 0 }
        }

        /// The same on a database that's a file, which other connections
        /// can open too.
        pub(in crate::dms) fn on_file(path: &std::path::Path) -> World {
            let keys = Keys::from_bytes([7; 32]);
            let db = crate::worker::open_db(path, &keys).expect("db");
            World { db, keys, now: NOW, actions: 0 }
        }

        pub(in crate::dms) fn call(&mut self, account: &str, method: &str, route: &str, body: Value) -> (u16, Value) {
            let caller = Caller::Device {
                account_id: account.into(),
                device_key: format!("{account}-k"),
                registrations: Vec::<Registration>::new(),
                removed_devices: Vec::new(),
                signed_in_at: None,
                bound_devices: None,
            };
            let bytes = if body.is_null() { Vec::new() } else { body.to_string().into_bytes() };
            identity::route(&mut self.db, &self.keys, method, route, &caller, &bytes, self.now).expect("a route")
        }

        pub(in crate::dms) fn code(&mut self, account: &str) -> String {
            let (status, shown) = self.call(account, "POST", "me/friend-id/reveal", Value::Null);
            assert_eq!(status, 200, "{shown}");
            shown["code"].as_str().expect("a code").to_owned()
        }

        pub(in crate::dms) fn name(&mut self, account: &str, name: &str) {
            let (_, me) = self.call(account, "GET", "me", Value::Null);
            let revision = me["profile"]["revision"].clone();
            assert_eq!(self.call(account, "PUT", "me/profile", json!({"name": name, "revision": revision})).0, 200);
        }

        /// Sends a request with `code`: the receipt, after checking it's the
        /// one answer every request gets.
        pub(in crate::dms) fn send(&mut self, sender: &str, code: &str) -> String {
            self.actions += 1;
            let action_id = format!("action-{:018}", self.actions);
            let (status, sent) = self.call(sender, "POST", "requests", json!({"code": code, "action_id": action_id}));
            assert_eq!(status, 202, "{sent}");
            let mut shape = sent.clone();
            shape["receipt"]["id"] = json!("");
            assert_eq!(
                shape,
                json!({"receipt": {"id": "", "state": "pending", "created_ms": self.now, "expires_ms": self.now + EXPIRY_MS},
                       "message": SENT})
            );
            sent["receipt"]["id"].as_str().expect("an id").to_owned()
        }

        pub(in crate::dms) fn incoming(&mut self, account: &str) -> Vec<Value> {
            let (_, requests) = self.call(account, "GET", "requests", Value::Null);
            requests["incoming"].as_array().cloned().unwrap_or_default()
        }

        pub(in crate::dms) fn outgoing(&mut self, account: &str) -> Vec<Value> {
            let (_, requests) = self.call(account, "GET", "requests", Value::Null);
            requests["outgoing"].as_array().cloned().unwrap_or_default()
        }

        pub(in crate::dms) fn contacts(&mut self, account: &str) -> Vec<Value> {
            let (_, contacts) = self.call(account, "GET", "contacts", Value::Null);
            contacts["contacts"].as_array().cloned().unwrap_or_default()
        }

        pub(in crate::dms) fn accept(&mut self, account: &str, request: &str) -> (u16, Value) {
            self.call(account, "POST", &format!("requests/{request}/accept"), Value::Null)
        }
    }

    #[test]
    fn every_request_is_answered_alike_and_only_an_eligible_one_reaches_anyone() {
        let mut world = World::new();
        world.name("ana", "Ana");
        world.name("bo", "Bo");
        let ana = world.code("ana");
        let bo = world.code("bo");

        // Unknown, their own, a retired code, malformed.
        world.send("bo", "cf1_AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA");
        world.send("ana", &ana);
        let (status, _) = world.call("ana", "POST", "requests", json!({"code": "cf1_nope", "action_id": "a".repeat(22)}));
        assert_eq!(status, 400, "only grammar is told");
        assert_eq!(world.call("ana", "POST", "requests", json!({"code": &bo, "action_id": "short"})).0, 400);

        // One that reaches Bo, once; a second from Ana, and Bo's crossing one,
        // don't.
        let first = world.send("ana", &bo);
        let second = world.send("ana", &bo);
        let crossing = world.send("bo", &ana);
        let incoming = world.incoming("bo");
        assert_eq!(incoming.len(), 1, "{incoming:?}");
        assert_eq!(incoming[0]["id"], first.as_str());
        assert_eq!(incoming[0]["from"]["name"], "Ana");
        assert_eq!(world.incoming("ana"), Vec::<Value>::new(), "the crossing one reaches no one");
        let shown = world.outgoing("ana");
        assert!(shown.iter().all(|receipt| receipt["state"] == "pending"), "{shown:?}");
        assert_eq!(shown.len(), 3, "their own, and the two to Bo: {shown:?}");

        // A retry is the same receipt, and costs nothing.
        let (_, again) = world.call("ana", "POST", "requests", json!({"code": &bo, "action_id": format!("action-{:018}", 3)}));
        assert_eq!(again["receipt"]["id"], first.as_str());

        // Accepted: friends, both see each other, with a tag of their own.
        let (status, contact) = world.accept("bo", &first);
        assert_eq!(status, 200, "{contact}");
        assert_eq!(contact["name"], "Ana");
        assert_eq!(world.accept("bo", &first).1, contact, "the same, accepted again");
        let anas = world.contacts("ana");
        assert_eq!(anas.len(), 1);
        assert_eq!((anas[0]["name"].as_str(), anas[0]["id"].clone()), (Some("Bo"), contact["id"].clone()));
        assert_eq!(anas[0]["conversation_id"], contact["conversation_id"]);
        assert_ne!(anas[0]["tag"], contact["tag"], "each viewer's own");
        assert_eq!(contact["tag"].as_str().map(str::len), Some(6));
        // A friend's account shows, for their keys; a request's sender's never does.
        assert_eq!((anas[0]["account_id"].as_str(), contact["account_id"].as_str()), (Some("bo"), Some("ana")));
        assert!(incoming[0]["from"].get("account_id").is_none());
        // Once accepted, the receipt says so; the others still read pending,
        // and nothing can be accepted through them.
        let shown = world.outgoing("ana");
        let accepted = shown.iter().find(|receipt| receipt["id"] == first.as_str()).expect("the receipt");
        assert_eq!((accepted["state"].as_str(), accepted["contact_id"].clone()), (Some("accepted"), contact["id"].clone()));
        assert_eq!(world.outgoing("bo").iter().find(|receipt| receipt["id"] == crossing.as_str()).expect("crossing")["state"], "pending");
        assert_eq!(world.call("ana", "POST", &format!("requests/{first}/cancel"), Value::Null).0, 409);
        assert_eq!(world.call("ana", "POST", &format!("requests/{second}/cancel"), Value::Null).0, 204);
        // A friend already: another request reaches no one.
        world.send("bo", &ana);
        assert_eq!(world.incoming("ana"), Vec::<Value>::new());

        // Someone else can't see, accept or block any of it.
        world.name("eve", "Eve");
        assert_eq!(world.accept("eve", &first).0, 404);
        assert_eq!(world.call("eve", "POST", "blocks", json!({"contact_id": contact["id"]})).0, 404);
        assert_eq!(world.call("eve", "DELETE", &format!("contacts/{}", contact["id"].as_str().expect("id")), Value::Null).0, 404);
        assert_eq!(world.contacts("eve"), Vec::<Value>::new());
        let backend = Caller::Backend { account_id: "bo".into(), claktory_id: "mac".into(), epoch: 1, agent: None };
        let (status, _) = identity::route(&mut world.db, &world.keys, "GET", "contacts", &backend, b"", NOW).expect("a route");
        assert_eq!(status, 403);
    }

    #[test]
    fn a_tag_stays_through_resets_and_renames() {
        let mut world = World::new();
        world.name("ana", "Ana");
        let bo = world.code("bo");
        let request = world.send("ana", &bo);
        let tag = world.incoming("bo")[0]["from"]["tag"].clone();
        world.accept("bo", &request);
        world.name("ana", "Bo's mum");
        let (_, me) = world.call("ana", "GET", "me", Value::Null);
        world.call("ana", "POST", "me/friend-id/reset", json!({"generation": me["friend_id"]["generation"]}));
        let contact = &world.contacts("bo")[0];
        assert_eq!((contact["tag"].clone(), contact["name"].as_str()), (tag, Some("Bo's mum")));
    }

    #[test]
    fn declining_is_silent_and_holds_off_that_sender_for_30_days() {
        let mut world = World::new();
        let bo = world.code("bo");
        let request = world.send("ana", &bo);
        assert_eq!(world.call("bo", "POST", &format!("requests/{request}/decline"), Value::Null).0, 204);
        assert_eq!(world.outgoing("ana")[0]["state"], "pending", "Ana isn't told");
        assert_eq!(world.accept("bo", &request).0, 410);
        world.send("ana", &bo);
        assert_eq!(world.incoming("bo"), Vec::<Value>::new());
        world.now += DECLINE_MS;
        world.send("ana", &bo);
        assert_eq!(world.incoming("bo").len(), 1, "after 30 days");
        // And a request expires: then it can't be accepted, and the receipt goes.
        let pending = world.incoming("bo")[0]["id"].as_str().expect("id").to_owned();
        world.now += EXPIRY_MS;
        assert_eq!(world.accept("bo", &pending).0, 410);
        assert_eq!(world.outgoing("ana"), Vec::<Value>::new());
    }

    #[test]
    fn reopening_a_decline_lets_that_sender_reach_you_again_and_brings_back_their_request() {
        let mut world = World::new();
        world.name("ana", "Ana");
        let bo = world.code("bo");
        let request = world.send("ana", &bo);
        world.call("bo", "POST", &format!("requests/{request}/decline"), Value::Null);
        let (_, declines) = world.call("bo", "GET", "declines", Value::Null);
        let declines = declines["declines"].as_array().cloned().unwrap_or_default();
        assert_eq!(declines.len(), 1, "{declines:?}");
        assert_eq!((declines[0]["from"]["name"].as_str(), declines[0]["until_ms"].as_i64()), (Some("Ana"), Some(NOW + DECLINE_MS)));
        let id = declines[0]["id"].as_str().expect("id").to_owned();
        assert_eq!(id.len(), 32);
        assert_eq!(world.call("ana", "GET", "declines", Value::Null).1, json!({"declines": []}), "Ana sees nothing");

        // Only Bo can reopen it.
        assert_eq!(world.call("carol", "DELETE", &format!("declines/{id}"), Value::Null).0, 404);
        assert_eq!(world.call("bo", "DELETE", "declines/0000", Value::Null).0, 404);
        let (status, reopened) = world.call("bo", "DELETE", &format!("declines/{id}"), Value::Null);
        assert_eq!(status, 200, "{reopened}");
        assert_eq!(reopened["request"]["id"], request.as_str(), "the request declined is back");
        assert_eq!(reopened["request"]["from"]["name"], "Ana");
        assert_eq!(world.incoming("bo").len(), 1);
        assert_eq!(world.outgoing("ana")[0]["state"], "pending", "Ana saw nothing either way");
        assert_eq!(world.call("bo", "GET", "declines", Value::Null).1, json!({"declines": []}));
        assert_eq!(world.call("bo", "DELETE", &format!("declines/{id}"), Value::Null).0, 404, "once");
        assert_eq!(world.accept("bo", &request).0, 200);

        // A request that wouldn't reach them now stays declined, but the
        // sender's next one reaches them.
        let dan_request = world.send("dan", &bo);
        world.call("bo", "POST", &format!("requests/{dan_request}/decline"), Value::Null);
        let (_, me) = world.call("bo", "GET", "me", Value::Null);
        world.call("bo", "POST", "me/friend-id/reset", json!({"generation": me["friend_id"]["generation"]}));
        let (_, declines) = world.call("bo", "GET", "declines", Value::Null);
        let id = declines["declines"][0]["id"].as_str().expect("id").to_owned();
        assert_eq!(
            world.call("bo", "DELETE", &format!("declines/{id}"), Value::Null).1,
            json!({"request": null}),
            "sent to the old friend ID"
        );
        assert_eq!(world.accept("bo", &dan_request).0, 410);
        let bo = world.code("bo");
        world.send("dan", &bo);
        assert_eq!(world.incoming("bo").len(), 1, "within the 30 days");

        // Past 30 days, a decline is no longer listed.
        let eve_request = world.send("eve", &bo);
        world.call("bo", "POST", &format!("requests/{eve_request}/decline"), Value::Null);
        world.now += DECLINE_MS;
        assert_eq!(world.call("bo", "GET", "declines", Value::Null).1, json!({"declines": []}));
    }

    #[test]
    fn blocking_ends_everything_between_two_people_and_unblocking_restores_nothing() {
        let mut world = World::new();
        let ana = world.code("ana");
        let bo = world.code("bo");
        let request = world.send("ana", &bo);
        let (_, contact) = world.accept("bo", &request);
        let conversation = contact["conversation_id"].as_str().expect("conversation").to_owned();
        // An agent of each may use their conversation.
        for account in ["ana", "bo"] {
            let grant = crate::grants::Put {
                app: "dms".into(),
                agent: crate::ipc::Actor { id: "june".into(), incarnation: "inc-1".into() },
                scopes: vec!["dm.read".into()],
                resources: vec![Resource { kind: "conversation".into(), id: conversation.clone() }],
                expires_ms: 0,
            };
            let backend = Caller::Backend { account_id: account.into(), claktory_id: "mac".into(), epoch: 1, agent: None };
            crate::grants::put(&mut world.db, &backend, grant, NOW).expect("granted");
        }

        let (status, block) = world.call("bo", "POST", "blocks", json!({"contact_id": contact["id"]}));
        assert_eq!(status, 200, "{block}");
        assert_eq!(world.contacts("ana"), Vec::<Value>::new(), "no longer friends");
        assert_eq!(world.contacts("bo"), Vec::<Value>::new());
        let grants: i64 = world.db.query_row("SELECT COUNT(*) FROM grants", [], |row| row.get(0)).expect("count");
        assert_eq!(grants, 0, "no agent keeps the conversation");
        // Neither reaches the other now, whatever they send.
        world.send("ana", &bo);
        world.send("bo", &ana);
        assert_eq!((world.incoming("ana").len(), world.incoming("bo").len()), (0, 0));
        let (_, blocks) = world.call("bo", "GET", "blocks", Value::Null);
        assert_eq!(blocks["blocks"].as_array().map(Vec::len), Some(1));

        // Unblocked: still not friends, and nothing comes back; a fresh
        // request and acceptance make a new friendship, a new conversation.
        let id = block["id"].as_str().expect("id");
        assert_eq!(world.call("ana", "DELETE", &format!("blocks/{id}"), Value::Null).0, 404, "not Ana's");
        assert_eq!(world.call("bo", "DELETE", &format!("blocks/{id}"), Value::Null).0, 204);
        assert_eq!(world.contacts("bo"), Vec::<Value>::new());
        assert_eq!(world.incoming("bo"), Vec::<Value>::new());
        let again = world.send("ana", &bo);
        let (_, renewed) = world.accept("bo", &again);
        assert_eq!(renewed["generation"], 2);
        assert_ne!(renewed["conversation_id"], conversation.as_str());

        // Blocking from a request that reached you.
        let carol = world.code("carol");
        let request = world.send("ana", &carol);
        assert_eq!(world.call("carol", "POST", "blocks", json!({"request_id": request})).0, 200);
        assert_eq!(world.accept("carol", &request).0, 410);
        assert_eq!(world.call("carol", "POST", "blocks", json!({})).0, 400);
    }

    #[test]
    fn a_reset_or_requests_off_closes_what_waits_but_not_what_you_sent() {
        let mut world = World::new();
        let bo = world.code("bo");
        let to_bo = world.send("carol", &bo);
        let (_, me) = world.call("bo", "GET", "me", Value::Null);
        world.call("bo", "POST", "me/friend-id/reset", json!({"generation": me["friend_id"]["generation"]}));
        assert_eq!(world.accept("bo", &to_bo).0, 410, "sent through the old friend ID");
        world.send("carol", &bo);
        assert_eq!(world.incoming("bo"), Vec::<Value>::new(), "the old friend ID reaches no one");

        let bo = world.code("bo");
        let from_ana = world.send("ana", &bo);
        let dan = world.code("dan");
        let bos = world.send("bo", &dan);
        let (_, me) = world.call("bo", "GET", "me", Value::Null);
        world.call("bo", "PUT", "me/privacy", json!({"requests_on": false, "revision": me["privacy"]["revision"]}));
        assert_eq!(world.accept("bo", &from_ana).0, 410);
        assert_eq!(world.accept("dan", &bos).0, 200, "Bo asked, so Dan can accept");
    }

    #[test]
    fn a_new_person_sends_5_requests_a_day_then_20() {
        let mut world = World::new();
        for _ in 0..5 {
            world.send("ana", "cf1_AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA");
        }
        let (status, _) =
            world.call("ana", "POST", "requests", json!({"code": "cf1_AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA", "action_id": "x".repeat(22)}));
        assert_eq!(status, 429);
        world.now += NEW_FOR_MS;
        for _ in 0..20 {
            world.send("ana", "cf1_AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA");
        }
        assert_eq!(
            world.call("ana", "POST", "requests", json!({"code": "cf1_AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA", "action_id": "y".repeat(22)})).0,
            429
        );
    }

    #[test]
    fn a_deleted_account_leaves_its_friends_and_reaches_no_one() {
        let mut world = World::new();
        let bo = world.code("bo");
        let carol = world.code("carol");
        let request = world.send("ana", &bo);
        world.accept("bo", &request);
        let to_carol = world.send("ana", &carol);
        let ana = world.code("ana");
        let from_dan = world.send("dan", &ana);
        identity::purge(&mut world.db, &world.keys, "ana", world.now).expect("purged");
        assert_eq!(world.contacts("bo"), Vec::<Value>::new());
        assert_eq!(world.incoming("carol"), Vec::<Value>::new());
        assert_eq!(world.accept("carol", &to_carol).0, 404);
        assert_eq!(world.outgoing("dan")[0]["id"], from_dan.as_str(), "Dan's receipt reads as before");
        assert_eq!(world.outgoing("dan")[0]["state"], "pending");
        let left: i64 = world
            .db
            .query_row("SELECT COUNT(*) FROM dm_requests WHERE sender = 'ana' OR target = 'ana'", [], |row| row.get(0))
            .expect("count");
        assert_eq!(left, 0);
    }
}
