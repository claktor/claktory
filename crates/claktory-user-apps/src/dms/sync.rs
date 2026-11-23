//! Sync (D2b): how every device of a person keeps up with their DMs, and
//! what each person keeps of a conversation (read, draft, deleted).
//!
//! - **Outbox.** Every change a person's devices need to know of adds an
//!   event to that account's own outbox (`dm_account_events`), numbered in
//!   its own order, in the same transaction as the change: a message, a
//!   deletion, a read position or draft, a conversation begun or ended,
//!   requests, contacts or blocks changed, their own identity changed.
//! - **Snapshot and cursor.** A device starts with `GET sync`: everything
//!   it shows, and a cursor. Then `GET sync?since=<cursor>` answers what
//!   happened after it, at most 100 events (or 256 KiB), with the next
//!   cursor; `wait` (up to 25 seconds) waits for something to happen when
//!   nothing has. Fetching consumes nothing: every device reads the same
//!   outbox. Events go after 30 days; a cursor from before that, or one
//!   that was never handed out, is `410 resync_required`: start again from
//!   a snapshot, then page back through history.
//! - **Per person, per conversation** (`dm_views`): how far they've read
//!   (only ever forward), how far they've deleted their copy (only ever
//!   forward; the other person's copy stays), and their draft, kept by the
//!   service (compare and swap on its revision, so two devices never
//!   silently overwrite each other).
//! - **Space.** Each account keeps at most 100 MiB of messages (what's in
//!   its conversations after its deletion cutoff, its own and the other's):
//!   past it, sending is refused until it deletes some. A message both
//!   people deleted (or one of them is gone) is gone from the store.

use rusqlite::{Connection, OptionalExtension, Transaction, params};
use serde::Deserialize;
use serde_json::{Value, json};

use super::identity::{Refusal, invalid};
use super::keys::Refused;
use super::messages::Kept;

pub const SCHEMA: &str = "
-- What a person keeps of a conversation: how far they've read and deleted
-- their copy (sequence numbers, only ever forward) and their draft (a body
-- as `dms::bodies` keeps it; revision 0 is none yet).
CREATE TABLE IF NOT EXISTS dm_views (
    account TEXT NOT NULL,
    conversation_id TEXT NOT NULL,
    read_seq INTEGER NOT NULL DEFAULT 0,
    deleted_seq INTEGER NOT NULL DEFAULT 0,
    draft_format INTEGER,
    draft BLOB,
    draft_revision INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (account, conversation_id)
);
-- Bytes of messages each account keeps.
CREATE TABLE IF NOT EXISTS dm_usage (
    account TEXT PRIMARY KEY,
    bytes INTEGER NOT NULL
);
-- How far each account's outbox was pruned: a cursor before it can't be
-- answered.
CREATE TABLE IF NOT EXISTS dm_cursors (
    account TEXT PRIMARY KEY,
    pruned_through INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS dm_account_events_created ON dm_account_events (created);
";

/// What each account may keep.
pub const QUOTA: i64 = 100 * 1024 * 1024;
/// How long events are kept for devices to catch up with.
const EVENTS_KEEP_MS: i64 = 30 * 86_400_000;
/// A page of events at most, and how long a device may wait for one.
const PAGE: usize = 100;
const PAGE_BYTES: usize = 256 * 1024;
pub const MAX_WAIT: u64 = 25;

/// What changed, for an account's devices: the kinds of event.
pub enum Kind {
    /// A message in a conversation (`object`: its id, `seq`: its number).
    Message,
    /// The person deleted their copy through `seq`.
    Deleted,
    /// A conversation began or ended.
    Conversation,
    /// Their requests, in or out, changed.
    Requests,
    /// Their friends changed.
    Contacts,
    /// Whom they blocked changed.
    Blocks,
    /// Their own profile, privacy or friend ID changed.
    Me,
    /// One of their agents suggested a draft, or its suggestion changed
    /// (`object`: its id, `seq`: its revision; agents.rs).
    AgentDraft,
    /// A message an agent proposed was proposed, revised or decided
    /// (`object`: its id, `seq`: its revision; review.rs).
    Proposal,
    /// An account's identity key or device list changed (`object`: the
    /// account; leaves.rs): fetch its keys again.
    Keys,
    /// A group took a message (`object`: the group's id, base64url; `seq`:
    /// its seq; groups.rs).
    GroupMessage,
    /// A Welcome waits for one of the account's leaves (`object`: its id).
    Welcome,
    /// The account's groups changed: list them again.
    Groups,
    /// A device hand-off waits (`object`: its id; handoffs.rs).
    Handoff,
}

impl Kind {
    fn name(&self) -> &'static str {
        match self {
            Kind::Message => "message",
            Kind::Deleted => "deleted",
            Kind::Conversation => "conversation",
            Kind::Requests => "requests",
            Kind::Contacts => "contacts",
            Kind::Blocks => "blocks",
            Kind::Me => "me",
            Kind::AgentDraft => "agent_draft",
            Kind::Proposal => "proposal",
            Kind::Keys => "keys",
            Kind::GroupMessage => "group_message",
            Kind::Welcome => "welcome",
            Kind::Groups => "groups",
            Kind::Handoff => "handoff",
        }
    }
}

thread_local! {
    /// The accounts this thread added events for since `touched` last
    /// took them: whose waiting devices to wake.
    static TOUCHED: std::cell::RefCell<Vec<String>> = const { std::cell::RefCell::new(Vec::new()) };
}

/// The accounts this thread added events for since it last asked, each
/// once (a transaction that then rolled back included: waking one for
/// nothing only makes it look again).
pub fn touched() -> Vec<String> {
    let mut accounts = TOUCHED.with(|touched| std::mem::take(&mut *touched.borrow_mut()));
    accounts.sort();
    accounts.dedup();
    accounts
}

/// Adds an event to `account`'s outbox, in the caller's transaction,
/// numbered after every event it had, pruned ones too.
pub fn emit(
    transaction: &Transaction<'_>,
    account: &str,
    kind: Kind,
    conversation: &str,
    object: &str,
    seq: i64,
    now_ms: i64,
) -> rusqlite::Result<()> {
    emit_if(transaction, true, account, kind, conversation, object, seq, now_ms)
}

/// The same, only when `when`: the statement runs either way, so whether it
/// added one shows in neither the time it takes nor the queries made (a
/// request reaching someone, or not).
#[allow(clippy::too_many_arguments)]
pub fn emit_if(
    transaction: &Transaction<'_>,
    when: bool,
    account: &str,
    kind: Kind,
    conversation: &str,
    object: &str,
    seq: i64,
    now_ms: i64,
) -> rusqlite::Result<()> {
    transaction.execute(
        "INSERT INTO dm_account_events (account, seq, kind, conversation_id, object_id, object_seq, created)
         SELECT ?1, MAX(
             (SELECT COALESCE(MAX(seq), 0) FROM dm_account_events WHERE account = ?1),
             (SELECT COALESCE(MAX(pruned_through), 0) FROM dm_cursors WHERE account = ?1)
         ) + 1, ?2, ?3, ?4, ?5, ?6 WHERE ?7",
        params![account, kind.name(), conversation, object, seq, now_ms, when],
    )?;
    if when {
        TOUCHED.with(|touched| touched.borrow_mut().push(account.to_owned()));
    }
    Ok(())
}

/// The person's own view of a conversation: read and deleted through, and
/// their draft.
pub fn view(db: &Connection, account: &str, conversation: &str) -> Result<Value, Refused> {
    let found: Option<(i64, i64, i64)> = db
        .query_row(
            "SELECT read_seq, deleted_seq, draft_revision FROM dm_views WHERE account = ?1 AND conversation_id = ?2",
            params![account, conversation],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()?;
    let (read, deleted, revision) = found.unwrap_or((0, 0, 0));
    // Read positions and drafts travel in the person's self group now (E2):
    // the draft here stays empty.
    Ok(json!({"read_seq": read, "deleted_seq": deleted, "draft": {"text": "", "revision": revision}}))
}

/// How far the person deleted their copy of a conversation.
pub fn deleted_through(db: &Connection, account: &str, conversation: &str) -> rusqlite::Result<i64> {
    Ok(db
        .query_row("SELECT deleted_seq FROM dm_views WHERE account = ?1 AND conversation_id = ?2", params![account, conversation], |row| {
            row.get(0)
        })
        .optional()?
        .unwrap_or(0))
}

fn ensure_view(transaction: &Transaction<'_>, account: &str, conversation: &str) -> rusqlite::Result<()> {
    transaction.execute(
        "INSERT INTO dm_views (account, conversation_id) VALUES (?1, ?2) ON CONFLICT (account, conversation_id) DO NOTHING",
        params![account, conversation],
    )?;
    Ok(())
}

/// Bytes the account keeps now.
pub fn usage(db: &Connection, account: &str) -> rusqlite::Result<i64> {
    Ok(db.query_row("SELECT bytes FROM dm_usage WHERE account = ?1", [account], |row| row.get(0)).optional()?.unwrap_or(0))
}

/// Adds `bytes` to what `account` keeps (less, when negative).
pub fn add_usage(transaction: &Transaction<'_>, account: &str, bytes: i64) -> rusqlite::Result<()> {
    transaction.execute(
        "INSERT INTO dm_usage (account, bytes) VALUES (?1, MAX(?2, 0))
         ON CONFLICT (account) DO UPDATE SET bytes = MAX(bytes + ?2, 0)",
        params![account, bytes],
    )?;
    Ok(())
}

/// A conversation of `account`'s: its last sequence number and whether it's
/// still on. None if it isn't theirs.
fn conversation_of(transaction: &Transaction<'_>, account: &str, conversation: &str) -> rusqlite::Result<Option<(i64, bool)>> {
    transaction
        .query_row(
            "SELECT COALESCE(v.last_seq, 0), c.ended IS NULL FROM dm_contacts c LEFT JOIN dm_conversations v ON v.id = c.conversation_id
             WHERE c.conversation_id = ?1 AND (c.low = ?2 OR c.high = ?2)",
            params![conversation, account],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
}

fn not_found() -> Refusal {
    Refusal(404, "not_found", "There's nothing here.".to_owned())
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Delete {
    /// Delete the person's copy of every message up to this one.
    through_seq: i64,
    #[serde(default)]
    #[allow(dead_code)]
    request_nonce: Option<String>,
}

/// Deletes the person's copy of a conversation's messages through a
/// sequence number (no further than the last): their history starts after
/// it on every device, for good, and what they kept no longer counts
/// against their space. The other person's copy stays. Messages both have
/// deleted (or whose other person is gone) leave the store.
pub fn delete(transaction: &Transaction<'_>, account: &str, conversation: &str, bytes: &[u8], now_ms: i64) -> Result<Value, Refusal> {
    let delete: Delete = super::identity::body(bytes)?;
    let (last, _) = conversation_of(transaction, account, conversation)?.ok_or_else(not_found)?;
    ensure_view(transaction, account, conversation)?;
    let was = deleted_through(transaction, account, conversation)?;
    let now = was.max(delete.through_seq.clamp(0, last));
    if now != was {
        let freed: i64 = transaction.query_row(
            "SELECT COALESCE(SUM(LENGTH(body)), 0) FROM dm_messages WHERE conversation_id = ?1 AND seq > ?2 AND seq <= ?3",
            params![conversation, was, now],
            |row| row.get(0),
        )?;
        transaction.execute(
            "UPDATE dm_views SET deleted_seq = ?3, read_seq = MAX(read_seq, ?3) WHERE account = ?1 AND conversation_id = ?2",
            params![account, conversation, now],
        )?;
        add_usage(transaction, account, -freed)?;
        emit(transaction, account, Kind::Deleted, conversation, "", now, now_ms)?;
        collect(transaction, conversation)?;
    }
    Ok(json!({"deleted_seq": now}))
}

/// Removes from the store the messages of a conversation that neither of
/// its two people keeps: each deleted them, or is gone.
pub fn collect(transaction: &Transaction<'_>, conversation: &str) -> rusqlite::Result<usize> {
    let pair: Option<(String, String)> = transaction
        .query_row("SELECT low, high FROM dm_contacts WHERE conversation_id = ?1", [conversation], |row| Ok((row.get(0)?, row.get(1)?)))
        .optional()?;
    let Some((low, high)) = pair else { return Ok(0) };
    let mut kept = Vec::new();
    for account in [&low, &high] {
        let here: bool = transaction.query_row("SELECT COUNT(*) > 0 FROM dm_people WHERE account_id = ?1", [account], |row| row.get(0))?;
        kept.push(if here { deleted_through(transaction, account, conversation)? } else { i64::MAX });
    }
    let through = kept.into_iter().min().unwrap_or(0);
    transaction.execute("DELETE FROM dm_messages WHERE conversation_id = ?1 AND seq <= ?2", params![conversation, through])
}

/// Where a device is: an opaque cursor, after the account's event `seq`.
pub fn cursor(seq: i64) -> String {
    format!("c1.{seq}")
}

fn parse_cursor(text: &str) -> Option<i64> {
    text.strip_prefix("c1.")?.parse().ok().filter(|seq| *seq >= 0)
}

/// The event a cursor is after, refusing one that isn't a cursor.
pub fn since(text: &str) -> Result<i64, Refusal> {
    parse_cursor(text).ok_or_else(resync)
}

/// Whether events after `since` are all still kept (and `since` isn't
/// past the last): a reader further behind starts again.
pub fn within(db: &Connection, account: &str, since: i64) -> Result<(), Refusal> {
    let (last, pruned) = bounds(db, account)?;
    if since < pruned || since > last {
        return Err(resync());
    }
    Ok(())
}

fn resync() -> Refusal {
    Refusal(410, "resync_required", "This device missed too much. Start again from a snapshot.".to_owned())
}

/// What `GET sync` asks: a snapshot (no `since`), or what happened after a
/// cursor, waiting up to `wait` seconds for something.
pub struct Ask {
    pub since: Option<i64>,
    pub limit: usize,
    pub wait: u64,
}

pub fn ask(query: &str) -> Result<Ask, Refusal> {
    let mut ask = Ask { since: None, limit: PAGE, wait: 0 };
    for pair in query.split('&').filter(|pair| !pair.is_empty()) {
        let (name, value) = pair.split_once('=').unwrap_or((pair, ""));
        match name {
            "since" => ask.since = Some(since(value)?),
            "limit" => ask.limit = value.parse().ok().filter(|n| (1..=PAGE).contains(n)).ok_or_else(|| invalid("limit is 1 to 100."))?,
            "wait" => ask.wait = value.parse().ok().filter(|n| *n <= MAX_WAIT).ok_or_else(|| invalid("wait is 0 to 25 seconds."))?,
            "request_nonce" => {}
            _ => return Err(invalid("Sync takes since, limit and wait.")),
        }
    }
    Ok(ask)
}

/// The account's last event, and how far its outbox was pruned.
pub fn bounds(db: &Connection, account: &str) -> rusqlite::Result<(i64, i64)> {
    let last: i64 = db.query_row("SELECT COALESCE(MAX(seq), 0) FROM dm_account_events WHERE account = ?1", [account], |row| row.get(0))?;
    let pruned: i64 =
        db.query_row("SELECT pruned_through FROM dm_cursors WHERE account = ?1", [account], |row| row.get(0)).optional()?.unwrap_or(0);
    Ok((last.max(pruned), pruned))
}

/// The snapshot: the cursor it's as of, and everything a device shows.
pub fn snapshot(db: &Connection, account: &str, parts: Value) -> Result<Value, Refusal> {
    let (last, _) = bounds(db, account)?;
    let mut snapshot = parts;
    snapshot["cursor"] = json!(cursor(last));
    snapshot["usage"] = json!({"bytes": usage(db, account)?, "quota": QUOTA});
    Ok(snapshot)
}

/// What happened after `since`, each event as the device needs it: a
/// message as it now is to this person (or that they deleted it), their
/// read position, draft or deletion as they now are, and for the rest,
/// what to fetch again.
pub fn changes(db: &Connection, account: &str, since: i64, limit: usize) -> Result<Value, Refusal> {
    within(db, account, since)?;
    let rows: Vec<(i64, String, String, String, i64)> = {
        let mut statement = db.prepare(
            "SELECT seq, kind, conversation_id, object_id, object_seq FROM dm_account_events WHERE account = ?1 AND seq > ?2
             ORDER BY seq LIMIT ?3",
        )?;
        statement
            .query_map(params![account, since, limit as i64 + 1], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?))
            })?
            .collect::<Result<_, _>>()?
    };
    let mut has_more = rows.len() > limit;
    let mut events = Vec::new();
    let mut next = since;
    let mut bytes = 0;
    for (seq, kind, conversation, object, object_seq) in rows.into_iter().take(limit) {
        let mut event = json!({"kind": kind});
        if !conversation.is_empty() {
            event["conversation_id"] = json!(conversation);
        }
        match kind.as_str() {
            "message" => {
                let found =
                    db.query_row(&format!("SELECT {} FROM dm_messages WHERE id = ?1", Kept::COLUMNS), [&object], Kept::row).optional()?;
                let deleted = deleted_through(db, account, &conversation)?;
                match found {
                    Some(kept) if object_seq > deleted => {
                        bytes += kept.size();
                        if bytes > PAGE_BYTES && !events.is_empty() {
                            has_more = true;
                            break;
                        }
                        event["message"] = kept.shown(account)?;
                    }
                    _ => event["message"] = json!({"id": object, "seq": object_seq, "deleted": true}),
                }
            }
            "read" | "draft" | "deleted" => {
                let view = view(db, account, &conversation)?;
                event["view"] = view;
            }
            "agent_draft" => event["agent_draft"] = json!({"id": object, "revision": object_seq}),
            "proposal" => event["proposal"] = json!({"id": object, "revision": object_seq}),
            "keys" => event["account_id"] = json!(object),
            "group_message" => {
                event["group_id"] = json!(object);
                event["seq"] = json!(object_seq);
            }
            "welcome" | "handoff" => event["id"] = json!(object),
            _ => {}
        }
        events.push(event);
        next = seq;
    }
    Ok(json!({"events": events, "next_cursor": cursor(next), "has_more": has_more}))
}

/// Deletes events past their 30 days, remembering how far each account's
/// outbox was pruned. How many went.
pub fn prune(db: &mut Connection, now_ms: i64) -> Result<usize, Refused> {
    let transaction = db.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    let before = now_ms - EVENTS_KEEP_MS;
    transaction.execute(
        "INSERT INTO dm_cursors (account, pruned_through)
         SELECT account, MAX(seq) FROM dm_account_events WHERE created < ?1 GROUP BY account
         ON CONFLICT (account) DO UPDATE SET pruned_through = MAX(pruned_through, excluded.pruned_through)",
        [before],
    )?;
    let pruned = transaction.execute("DELETE FROM dm_account_events WHERE created < ?1", [before])?;
    transaction.commit()?;
    Ok(pruned)
}

/// An account is deleted (its person row already gone): what it kept of its
/// conversations, its space and its cursor go, and messages no one keeps
/// any more leave the store.
pub fn purge(transaction: &Transaction<'_>, account: &str) -> rusqlite::Result<()> {
    let conversations: Vec<String> = {
        let mut statement = transaction.prepare("SELECT conversation_id FROM dm_contacts WHERE low = ?1 OR high = ?1")?;
        statement.query_map([account], |row| row.get(0))?.collect::<Result<_, _>>()?
    };
    transaction.execute("DELETE FROM dm_views WHERE account = ?1", [account])?;
    transaction.execute("DELETE FROM dm_usage WHERE account = ?1", [account])?;
    transaction.execute("DELETE FROM dm_cursors WHERE account = ?1", [account])?;
    for conversation in conversations {
        collect(transaction, &conversation)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::super::contacts::tests::World;
    use super::super::messages::tests::{client_id, friends, history, post, texts};
    use super::*;

    fn sync(world: &mut World, account: &str, query: &str) -> (u16, Value) {
        let route = if query.is_empty() { "sync".to_owned() } else { format!("sync?{query}") };
        world.call(account, "GET", &route, Value::Null)
    }

    /// What happened after `cursor`: the events and the next cursor.
    fn since(world: &mut World, account: &str, cursor: &str) -> (Vec<Value>, String) {
        let (status, changes) = sync(world, account, &format!("since={cursor}"));
        assert_eq!(status, 200, "{changes}");
        (changes["events"].as_array().expect("events").clone(), changes["next_cursor"].as_str().expect("a cursor").to_owned())
    }

    fn kinds(events: &[Value]) -> Vec<&str> {
        events.iter().map(|event| event["kind"].as_str().expect("a kind")).collect()
    }

    fn delete(world: &mut World, account: &str, conversation: &str, through: i64) -> (u16, Value) {
        world.call(account, "POST", &format!("conversations/{conversation}/delete"), json!({"through_seq": through}))
    }

    fn stored(world: &World) -> i64 {
        world.db.query_row("SELECT COUNT(*) FROM dm_messages", [], |row| row.get(0)).expect("count")
    }

    #[test]
    fn a_snapshot_then_changes_reach_every_device_and_fetching_uses_nothing_up() {
        let mut world = World::new();
        let (conversation, generation) = friends(&mut world, "ana", "bo");
        let (status, snapshot) = sync(&mut world, "ana", "");
        assert_eq!(status, 200, "{snapshot}");
        assert_eq!(snapshot["conversations"][0]["id"], conversation.as_str());
        assert_eq!(snapshot["contacts"].as_array().map(Vec::len), Some(1));
        assert_eq!(snapshot["usage"], json!({"bytes": 0, "quota": QUOTA}));
        let cursor = snapshot["cursor"].as_str().expect("a cursor").to_owned();

        post(&mut world, "bo", &conversation, generation, &client_id(1), "hi Ana");
        post(&mut world, "ana", &conversation, generation, &client_id(2), "hi Bo");
        // Her phone and her Mac, from the same cursor, see the same.
        let (phone, next) = since(&mut world, "ana", &cursor);
        let (mac, _) = since(&mut world, "ana", &cursor);
        assert_eq!(phone, mac);
        // Each in the group's mailbox, and as a message of the conversation.
        assert_eq!(kinds(&phone), ["group_message", "message", "group_message", "message"]);
        assert_eq!(phone[0]["seq"], phone[1]["message"]["seq"]);
        let phone: Vec<Value> = phone.into_iter().filter(|event| event["kind"] == "message").collect();
        assert_eq!(phone[0]["message"]["from"], "them");
        assert_eq!(super::super::bodies::tests::opened(&phone[0]["message"]["envelope"]), "hi Ana");
        assert_eq!(phone[0]["conversation_id"], conversation.as_str());
        assert!(phone[0]["message"].get("client_message_id").is_none(), "the other's id is theirs");
        assert_eq!(phone[1]["message"]["client_message_id"], client_id(2), "hers, to match a send whose answer was lost");
        assert_eq!(history(&mut world, "ana", &conversation, "").1["messages"][1]["client_message_id"], client_id(2));
        let (nothing, again) = since(&mut world, "ana", &next);
        assert!(nothing.is_empty());
        assert_eq!(again, next, "nothing new, the same cursor");

        // A page at a time.
        let (status, page) = sync(&mut world, "ana", &format!("since={cursor}&limit=1"));
        assert_eq!(status, 200);
        assert_eq!((page["events"].as_array().map(Vec::len), page["has_more"].as_bool()), (Some(1), Some(true)));
    }

    /// Read positions and drafts travel in the person's self group now:
    /// their routes are gone.
    #[test]
    fn reading_and_drafts_are_no_routes_of_the_worker_s() {
        let mut world = World::new();
        let (conversation, _) = friends(&mut world, "ana", "bo");
        let caller = crate::ipc::Caller::Device { account_id: "ana".into(), device_key: "k".into(), registrations: Vec::new(), removed_devices: Vec::new(), signed_in_at: None, bound_devices: None };
        for what in ["read", "draft"] {
            let route = format!("conversations/{conversation}/{what}");
            let body = json!({"seq": 1, "text": "x", "revision": 0}).to_string();
            let answered = super::super::identity::route(&mut world.db, &world.keys, "PUT", &route, &caller, body.as_bytes(), world.now);
            assert!(answered.is_none(), "{what}");
        }
    }

    /// End to end began: the worker-sealed bodies go, with their numbering
    /// and the space they took.
    #[test]
    fn worker_sealed_bodies_are_forgotten_once() {
        let mut world = World::new();
        let (conversation, generation) = friends(&mut world, "ana", "bo");
        for seq in 1..=3 {
            world
                .db
                .execute(
                    "INSERT INTO dm_messages (id, conversation_id, seq, author, actor, client_id, contact_generation, format, body, digest,
                     accepted) VALUES (?1, ?2, ?3, 'ana', 'person', ?1, ?4, 1, x'0102030405', x'00', 0)",
                    params![format!("old-{seq}"), conversation, seq, generation],
                )
                .expect("an old message");
        }
        world.db.execute("INSERT INTO dm_conversations (id, last_seq) VALUES (?1, 3)", [&conversation]).expect("numbered");
        world
            .db
            .execute(
                "INSERT INTO dm_views (account, conversation_id, read_seq, deleted_seq, draft_format, draft, draft_revision)
                 VALUES ('bo', ?1, 3, 2, 1, x'01', 4)",
                [&conversation],
            )
            .expect("a view");
        world.db.execute("INSERT INTO dm_usage (account, bytes) VALUES ('ana', 15), ('bo', 5)", []).expect("usage");

        assert_eq!(super::super::bodies::forget_format_1_in(&mut world.db).expect("forgot"), 4);
        assert_eq!(stored(&world), 0);
        assert_eq!((usage(&world.db, "ana").expect("usage"), usage(&world.db, "bo").expect("usage")), (0, 0));
        assert_eq!(
            view(&world.db, "bo", &conversation).expect("a view"),
            json!({"read_seq": 0, "deleted_seq": 0, "draft": {"text": "", "revision": 4}})
        );
        assert_eq!(super::super::bodies::forget_format_1_in(&mut world.db).expect("again"), 0, "once");

        // What follows is numbered by the conversation's group.
        assert_eq!(post(&mut world, "bo", &conversation, generation, &client_id(1), "new").1["seq"], 1);
        assert_eq!(texts(&history(&mut world, "bo", &conversation, "").1), ["new"]);
        assert_eq!(usage(&world.db, "ana").expect("usage"), usage(&world.db, "bo").expect("usage"));
    }

    #[test]
    fn a_cursor_never_handed_out_or_pruned_starts_again_from_a_snapshot() {
        let mut world = World::new();
        let (conversation, generation) = friends(&mut world, "ana", "bo");
        let cursor = sync(&mut world, "ana", "").1["cursor"].as_str().expect("a cursor").to_owned();
        for forged in ["garbage", "c1.-1", "c1.999", "c2.1", ""] {
            let (status, refused) = sync(&mut world, "ana", &format!("since={forged}"));
            assert_eq!((status, refused["error"].as_str()), (410, Some("resync_required")), "{forged}");
        }
        assert_eq!(sync(&mut world, "ana", "since=c1.0&wait=26").0, 400);
        assert_eq!(sync(&mut world, "ana", "since=c1.0&limit=101").0, 400);
        assert_eq!(sync(&mut world, "ana", "since=c1.0&what=1").0, 400);

        // 30 days on, what's older is pruned: a cursor from before what
        // went starts again, and the outbox counts on from where it was.
        post(&mut world, "bo", &conversation, generation, &client_id(0), "a month ago");
        world.now += EVENTS_KEEP_MS + 1;
        assert!(prune(&mut world.db, world.now).expect("pruned") > 0);
        let (status, refused) = sync(&mut world, "ana", &format!("since={cursor}"));
        assert_eq!((status, refused["error"].as_str()), (410, Some("resync_required")));
        let fresh = sync(&mut world, "ana", "").1["cursor"].as_str().expect("a cursor").to_owned();
        assert!(parse_cursor(&fresh) > parse_cursor(&cursor), "the snapshot's cursor is where the outbox was");
        assert!(since(&mut world, "ana", &fresh).0.is_empty());
        post(&mut world, "bo", &conversation, generation, &client_id(1), "still here");
        let (events, next) = since(&mut world, "ana", &fresh);
        assert_eq!(kinds(&events), ["group_message", "message"]);
        assert!(parse_cursor(&next) > parse_cursor(&fresh));
    }

    #[test]
    fn deleting_history_hides_it_everywhere_frees_space_and_leaves_the_store_once_both_have() {
        let mut world = World::new();
        let (conversation, generation) = friends(&mut world, "ana", "bo");
        for n in 0..3 {
            post(&mut world, "ana", &conversation, generation, &client_id(n), &format!("m{n}"));
        }
        let cursor = sync(&mut world, "ana", "").1["cursor"].as_str().expect("a cursor").to_owned();
        let before = sync(&mut world, "ana", "").1["usage"]["bytes"].as_i64().expect("bytes");
        assert!(before > 0);
        assert_eq!(usage(&world.db, "bo").expect("usage"), before, "both keep both");

        assert_eq!(delete(&mut world, "ana", &conversation, 2).1, json!({"deleted_seq": 2}));
        assert_eq!(delete(&mut world, "ana", &conversation, 1).1, json!({"deleted_seq": 2}), "never back");
        assert_eq!(texts(&history(&mut world, "ana", &conversation, "").1), ["m2"]);
        assert_eq!(texts(&history(&mut world, "bo", &conversation, "").1), ["m0", "m1", "m2"], "his copy stays");
        let after = usage(&world.db, "ana").expect("usage");
        assert!(after < before && after > 0, "{after} of {before}");
        assert_eq!(stored(&world), 3, "Bo still keeps them");
        let (events, _) = since(&mut world, "ana", &cursor);
        assert_eq!(kinds(&events), ["deleted"]);
        assert_eq!(events[0]["view"]["deleted_seq"], 2);
        assert_eq!(events[0]["view"]["read_seq"], 2, "what's deleted is read");
        // A device further behind sees the deleted messages as deleted.
        let (events, _) = since(&mut world, "ana", "c1.0");
        let first = events.iter().find(|event| event["kind"] == "message").expect("a message");
        assert_eq!(first["message"], json!({"id": first["message"]["id"], "seq": 1, "deleted": true}));

        assert_eq!(delete(&mut world, "bo", &conversation, 1).0, 200);
        assert_eq!(stored(&world), 2, "neither keeps the first: it's gone");
        assert_eq!(delete(&mut world, "carol", &conversation, 1).0, 404);

        // An ended conversation is listed while there's something of it left.
        let (_, contacts) = world.call("bo", "GET", "contacts", Value::Null);
        let contact = contacts["contacts"][0]["id"].as_str().expect("id").to_owned();
        world.call("bo", "DELETE", &format!("contacts/{contact}"), Value::Null);
        assert_eq!(world.call("ana", "GET", "conversations", Value::Null).1["conversations"].as_array().map(Vec::len), Some(1));
        delete(&mut world, "ana", &conversation, 3);
        assert_eq!(world.call("ana", "GET", "conversations", Value::Null).1["conversations"], json!([]));
        assert_eq!(usage(&world.db, "ana").expect("usage"), 0);
        assert_eq!(world.call("bo", "GET", "conversations", Value::Null).1["conversations"].as_array().map(Vec::len), Some(1));
    }

    #[test]
    fn a_person_whose_space_is_full_sends_again_once_they_delete() {
        let mut world = World::new();
        let (conversation, generation) = friends(&mut world, "ana", "bo");
        post(&mut world, "ana", &conversation, generation, &client_id(1), "first");
        world.db.execute("UPDATE dm_usage SET bytes = ?1 WHERE account = 'ana'", [QUOTA - 10]).expect("full");
        let (status, refused) = post(&mut world, "ana", &conversation, generation, &client_id(2), "second");
        assert_eq!((status, refused["error"].as_str()), (413, Some("storage_full")), "{refused}");
        assert_eq!(post(&mut world, "bo", &conversation, generation, &client_id(2), "his").0, 200, "his space is his");
        delete(&mut world, "ana", &conversation, 2);
        world.db.execute("UPDATE dm_usage SET bytes = 0 WHERE account = 'ana'", []).expect("freed");
        assert_eq!(post(&mut world, "ana", &conversation, generation, &client_id(2), "second").0, 200);
    }

    #[test]
    fn a_request_is_an_event_for_its_recipient_only_when_it_reached_them() {
        let mut world = World::new();
        let code = world.code("bo");
        let bo = sync(&mut world, "bo", "").1["cursor"].as_str().expect("a cursor").to_owned();
        let ana = sync(&mut world, "ana", "").1["cursor"].as_str().expect("a cursor").to_owned();
        world.send("ana", &code);
        assert_eq!(kinds(&since(&mut world, "bo", &bo).0), ["requests"]);
        assert_eq!(kinds(&since(&mut world, "ana", &ana).0), ["requests"]);

        // Requests off: Ana's next request reaches no one, and Bo's outbox
        // says nothing of it.
        let (_, me) = world.call("carol", "GET", "me", Value::Null);
        world.call("carol", "PUT", "me/privacy", json!({"requests_on": false, "revision": me["privacy"]["revision"]}));
        let code = world.code("carol");
        let carol = sync(&mut world, "carol", "").1["cursor"].as_str().expect("a cursor").to_owned();
        world.send("ana", &code);
        assert!(since(&mut world, "carol", &carol).0.is_empty());
    }

    #[test]
    fn an_account_s_outbox_and_what_it_kept_go_with_it() {
        let mut world = World::new();
        let (conversation, generation) = friends(&mut world, "ana", "bo");
        post(&mut world, "ana", &conversation, generation, &client_id(1), "bye");
        delete(&mut world, "bo", &conversation, 1);
        assert_eq!(stored(&world), 1, "Ana keeps it");
        let Ok(()) = crate::dms::identity::purge(&mut world.db, &world.keys, "ana", world.now) else { panic!("purged") };
        assert_eq!(stored(&world), 0, "no one keeps it now");
        for table in ["dm_account_events", "dm_views", "dm_usage"] {
            let left: i64 =
                world.db.query_row(&format!("SELECT COUNT(*) FROM {table} WHERE account = 'ana'"), [], |row| row.get(0)).expect("count");
            assert_eq!(left, 0, "{table}");
        }
    }
}
