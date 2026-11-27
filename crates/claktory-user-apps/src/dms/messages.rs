//! Messages (D2a): the conversations' list, and what's left of their
//! history here.
//!
//! A conversation is a friendship's (`dm_contacts.conversation_id`): one per
//! contact generation, so reconnecting starts a new one.
//!
//! claktory.com keeps no messages: a device sends through the
//! person's home Claktory, which delivers to the friend's home, and a send
//! here is refused with `410 moved_to_home`. What it kept from before
//! (`dm_messages`, numbered per conversation, both people's outbox events)
//! stays readable (`GET conversations/<id>/messages`, `sync`) for devices to
//! take once into their own history, until the sweep lets it go.

use rusqlite::{Connection, Transaction, TransactionBehavior, params};
use serde_json::{Value, json};

use super::bodies::{Stored, merge};
use super::contacts::{contact_tag, ended_view, person};
use super::identity::{self, Refusal, invalid};
use super::keys::Keys;
use super::sync;
use crate::ipc::Caller;

pub const SCHEMA: &str = "
-- Each conversation's last sequence number: messages are numbered 1, 2, …
-- in the order they were accepted.
CREATE TABLE IF NOT EXISTS dm_conversations (
    id TEXT PRIMARY KEY,
    last_seq INTEGER NOT NULL
);
-- Messages. `actor` is who sent it for `author`: 'person', or the agent as
-- its Claktory and registration name it. `format` and `body` are the
-- body as `dms::bodies` keeps it; `digest` what a retry is compared by.
CREATE TABLE IF NOT EXISTS dm_messages (
    id TEXT PRIMARY KEY,
    conversation_id TEXT NOT NULL,
    seq INTEGER NOT NULL,
    author TEXT NOT NULL,
    actor TEXT NOT NULL,
    client_id TEXT NOT NULL,
    contact_generation INTEGER NOT NULL,
    format INTEGER NOT NULL,
    body BLOB NOT NULL,
    digest BLOB NOT NULL,
    accepted INTEGER NOT NULL,
    UNIQUE (conversation_id, seq),
    UNIQUE (author, actor, conversation_id, client_id)
);
-- Each account's private outbox of changes, in its own order (`seq`), for
-- its devices to sync from: written in the same transaction as the change.
CREATE TABLE IF NOT EXISTS dm_account_events (
    account TEXT NOT NULL,
    seq INTEGER NOT NULL,
    kind TEXT NOT NULL,
    conversation_id TEXT NOT NULL,
    object_id TEXT NOT NULL,
    object_seq INTEGER NOT NULL,
    created INTEGER NOT NULL,
    PRIMARY KEY (account, seq)
);
";

/// A page of history at most: messages, and bytes of their bodies.
const PAGE: usize = 100;
const PAGE_DEFAULT: usize = 50;
const PAGE_BYTES: usize = 256 * 1024;
/// A send to claktory.com from an app that hasn't moved to
/// the home: the same for a conversation, a self group or a channel.
pub(super) fn moved_to_home() -> Refusal {
    Refusal(410, "moved_to_home", "Messages go through your home Claktory now. Update Claktory to send this.".to_owned())
}

fn not_found() -> Refusal {
    Refusal(404, "not_found", "There's nothing here.".to_owned())
}

/// A conversation as listed: its id, contact, the two people, generation,
/// when it began and ended, its last sequence number.
type Listed = (String, String, String, String, i64, i64, Option<i64>, Option<i64>);
/// A message as kept.
pub struct Kept {
    id: String,
    seq: i64,
    author: String,
    actor: String,
    client_id: String,
    stored: Stored,
    accepted: i64,
}

impl Kept {
    /// The columns `row` reads, in its order.
    pub const COLUMNS: &str = "id, seq, author, actor, client_id, format, body, accepted";

    pub fn row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Kept> {
        Ok(Kept {
            id: row.get(0)?,
            seq: row.get(1)?,
            author: row.get(2)?,
            actor: row.get(3)?,
            client_id: row.get(4)?,
            stored: Stored { format: row.get(5)?, bytes: row.get(6)? },
            accepted: row.get(7)?,
        })
    }

    pub fn size(&self) -> usize {
        self.stored.size()
    }

    /// The message as `account`, one of its two people, sees it: theirs
    /// with the id their device sent it under (so a send whose answer was
    /// lost is matched), or the other's.
    pub fn shown(&self, account: &str) -> Result<Value, Refusal> {
        let mut shown = json!({
            "id": self.id,
            "seq": self.seq,
            "from": if self.author == account { "you" } else { "them" },
            "by_agent": self.actor != "person",
            "accepted_ms": self.accepted,
        });
        merge(&mut shown, self.stored.shown()?);
        if self.author == account && self.actor == "person" {
            shown["client_message_id"] = json!(self.client_id);
        }
        Ok(shown)
    }
}

pub(super) fn valid_client_id(id: &str) -> bool {
    (22..=64).contains(&id.len()) && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

/// Whether `route` is this module's.
pub fn knows(method: &str, route: &str) -> bool {
    let parts: Vec<&str> = route.split('/').collect();
    matches!(
        (method, parts.as_slice()),
        ("GET", ["conversations"] | ["sync"])
            | ("GET" | "POST", ["conversations", _, "messages"])
            | ("POST", ["conversations", _, "delete"])
    )
}

/// Answers one of this module's routes for a person, from one of their
/// devices.
pub fn answer(
    db: &mut Connection,
    keys: &Keys,
    method: &str,
    (route, query): (&str, &str),
    caller: &Caller,
    bytes: &[u8],
    now_ms: i64,
) -> Result<(u16, Value), Refusal> {
    let account = caller.account_id().to_owned();
    let transaction = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
    identity::ensure(&transaction, keys, &account, now_ms)?;
    let parts: Vec<&str> = route.split('/').collect();
    let answer = match (method, parts.as_slice()) {
        ("GET", ["sync"]) => {
            let ask = sync::ask(query)?;
            match ask.since {
                Some(since) => sync::changes(&transaction, &account, since, ask.limit)?,
                None => {
                    let parts = json!({
                        "conversations": conversations(&transaction, keys, &account, now_ms)?["conversations"],
                        "requests": super::contacts::requests(&transaction, keys, &account, now_ms)?,
                        "contacts": super::contacts::contacts(&transaction, keys, &account, now_ms)?["contacts"],
                        "proposals": super::review::waiting(&transaction, caller, now_ms)?,
                        "agent_drafts": super::agents::open(&transaction, &account)?,
                    });
                    sync::snapshot(&transaction, &account, parts)?
                }
            }
        }
        ("POST", ["conversations", conversation, "delete"]) => sync::delete(&transaction, &account, conversation, bytes, now_ms)?,
        ("GET", ["conversations"]) => conversations(&transaction, keys, &account, now_ms)?,
        // Sent through the person's home now; what was kept here
        // before stays readable below until it's gone.
        ("POST", ["conversations", _, "messages"]) => return Err(moved_to_home()),
        ("GET", ["conversations", conversation, "messages"]) => history(&transaction, &account, conversation, query)?,
        _ => return Err(not_found()),
    };
    transaction.commit()?;
    Ok((200, answer))
}

/// The person's conversations: each friendship's, now or before (one that
/// ended is listed while it has messages they haven't deleted), newest
/// first, each with what they keep of it (read, deleted, draft).
pub(super) fn conversations(transaction: &Transaction<'_>, keys: &Keys, account: &str, now_ms: i64) -> Result<Value, Refusal> {
    let rows: Vec<Listed> = {
        let mut statement = transaction.prepare(
            "SELECT c.conversation_id, c.id, c.low, c.high, c.generation, c.created, c.ended, v.last_seq
             FROM dm_contacts c LEFT JOIN dm_conversations v ON v.id = c.conversation_id
             LEFT JOIN dm_views w ON w.account = ?1 AND w.conversation_id = c.conversation_id
             WHERE (c.low = ?1 OR c.high = ?1) AND (c.ended IS NULL OR COALESCE(v.last_seq, 0) > COALESCE(w.deleted_seq, 0))
             ORDER BY c.created DESC",
        )?;
        statement
            .query_map([account], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?, row.get(5)?, row.get(6)?, row.get(7)?))
            })?
            .collect::<Result<_, _>>()?
    };
    let mut shown = Vec::new();
    for (conversation, contact, low, high, generation, since, ended, last_seq) in rows {
        let other = if low == account { high } else { low };
        // An ended one shows the other as they were when it ended, never
        // as they change after.
        let mut with = match ended {
            None => person(transaction, keys, account, &other, now_ms)?,
            Some(_) => {
                let (name, avatar_seed) = ended_view(transaction, keys, &conversation, account)?.unwrap_or_default();
                json!({"name": name, "avatar_seed": avatar_seed, "tag": contact_tag(transaction, keys, account, &other, now_ms)?})
            }
        };
        // The friend's account, whose keys the devices fetch for the group.
        with["account_id"] = json!(other);
        shown.push(json!({
            "id": conversation,
            "contact_id": contact,
            "contact_generation": generation,
            "with": with,
            "active": ended.is_none(),
            "since_ms": since,
            "ended_ms": ended,
            "last_seq": last_seq.unwrap_or(0),
            "view": sync::view(transaction, account, &conversation)?,
        }));
    }
    Ok(json!({"conversations": shown}))
}

/// A page of history: messages after one sequence number (oldest first),
/// or before one (the newest before it, still oldest first), or the latest;
/// at most `limit` (50, up to 100) and `max_bytes` of bodies, and none
/// accepted before `from_ms`.
pub struct Bounds {
    pub after: Option<i64>,
    pub before: Option<i64>,
    pub limit: usize,
    pub max_bytes: usize,
    pub from_ms: i64,
}

impl Bounds {
    /// What a history page's query asks, with a device's bounds.
    pub fn asked(query: &str) -> Result<Bounds, Refusal> {
        let mut bounds = Bounds { after: None, before: None, limit: PAGE_DEFAULT, max_bytes: PAGE_BYTES, from_ms: 0 };
        let number =
            |value: &str| value.parse::<i64>().ok().filter(|n| *n >= 0).ok_or_else(|| invalid("after and before are sequence numbers."));
        for pair in query.split('&').filter(|pair| !pair.is_empty()) {
            let (name, value) = pair.split_once('=').unwrap_or((pair, ""));
            match name {
                "after" => bounds.after = Some(number(value)?),
                "before" => bounds.before = Some(number(value)?),
                "limit" => {
                    bounds.limit = value.parse().ok().filter(|n| (1..=PAGE).contains(n)).ok_or_else(|| invalid("limit is 1 to 100."))?;
                }
                "request_nonce" => {}
                _ => return Err(invalid("A history page takes after, before and limit.")),
            }
        }
        if bounds.after.is_some() && bounds.before.is_some() {
            return Err(invalid("Ask for messages after one, or before one, not both."));
        }
        Ok(bounds)
    }
}

fn history(transaction: &Transaction<'_>, account: &str, conversation: &str, query: &str) -> Result<Value, Refusal> {
    page(transaction, account, conversation, &Bounds::asked(query)?)
}

/// A page of a conversation's history, for one of its two people (or
/// their agent, within its grant's bounds): every message, theirs and the
/// other's, whatever happened to the friendship since, but for what the
/// person deleted. Reading moves nothing.
pub fn page(transaction: &Transaction<'_>, account: &str, conversation: &str, bounds: &Bounds) -> Result<Value, Refusal> {
    let Bounds { after, before, limit, max_bytes, from_ms } = *bounds;
    let theirs: bool = transaction.query_row(
        "SELECT COUNT(*) > 0 FROM dm_contacts WHERE conversation_id = ?1 AND (low = ?2 OR high = ?2)",
        params![conversation, account],
        |row| row.get(0),
    )?;
    if !theirs {
        return Err(not_found());
    }
    let (sql, bound) = match (after, before) {
        (Some(after), _) => ("seq > ?2 ORDER BY seq ASC", after),
        (None, Some(before)) => ("seq < ?2 ORDER BY seq DESC", before),
        (None, None) => ("seq < ?2 ORDER BY seq DESC", i64::MAX),
    };
    // What they deleted is gone from their history.
    let deleted = sync::deleted_through(transaction, account, conversation)?;
    let rows: Vec<Kept> = {
        let mut statement = transaction.prepare(&format!(
            "SELECT {} FROM dm_messages
             WHERE conversation_id = ?1 AND seq > ?4 AND accepted >= ?5 AND {sql} LIMIT ?3",
            Kept::COLUMNS
        ))?;
        statement.query_map(params![conversation, bound, limit as i64 + 1, deleted, from_ms], Kept::row)?.collect::<Result<_, _>>()?
    };
    let mut has_more = rows.len() > limit;
    let mut messages = Vec::new();
    let mut bytes = 0;
    for kept in rows.into_iter().take(limit) {
        bytes += kept.size();
        if bytes > max_bytes && !messages.is_empty() {
            has_more = true;
            break;
        }
        messages.push(kept.shown(account)?);
    }
    if after.is_none() {
        messages.reverse();
    }
    Ok(json!({"messages": messages, "has_more": has_more}))
}

/// An account is deleted: its outbox goes. Its messages stay in its
/// friends' histories, as theirs to keep.
pub fn purge(transaction: &Transaction<'_>, account: &str) -> rusqlite::Result<()> {
    transaction.execute("DELETE FROM dm_account_events WHERE account = ?1", [account])?;
    for limit in ["messages", "messages-hour"] {
        transaction.execute("DELETE FROM dm_limits WHERE key = ?1", [format!("{limit}:{account}")])?;
    }
    Ok(())
}

#[cfg(test)]
pub(super) mod tests {
    use super::super::bodies::{self, Submitted};
    use super::super::contacts::tests::World;
    use super::super::groups;
    use super::super::identity::{hex, later, random, take_as};
    use super::super::leaves::{Leaf, leaf_of};
    use super::super::sync::Kind;
    use super::*;
    use crate::grants::{self, Resource};
    use crate::ipc::Actor;
    use claktory_dm_crypto::envelope::Envelope;
    use rusqlite::OptionalExtension;
    use serde::Deserialize;
    use sha2::Digest as _;

    // How messages were kept when claktory.com took them: what
    // the tests of what's left of them (history, sync, deletes) start from.

    /// Messages a person may send a minute, and an hour.
    const PER_MINUTE: i64 = 60;
    const PER_HOUR: i64 = 1000;

    /// The same words whatever ended it: removed, blocked by either, an account
    /// deleted.
    fn closed() -> Refusal {
        Refusal(410, "closed", "You can't send messages in this conversation anymore.".to_owned())
    }

    /// A send's body.
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Send {
        /// The sender's id for this message, the same on every retry: 22 to 64
        /// letters, digits, - and _, at least 128 random bits.
        client_message_id: String,
        /// The friendship's generation the sender saw.
        contact_generation: i64,
        /// The sender's MLS application message, in the conversation's group.
        envelope: Value,
        #[serde(default)]
        #[allow(dead_code)]
        request_nonce: Option<String>,
    }

    /// A send's answer, once it committed.
    pub struct Receipt {
        pub message_id: String,
        pub seq: i64,
        pub accepted_ms: i64,
    }

    impl Receipt {
        fn json(&self) -> Value {
            json!({"message_id": self.message_id, "seq": self.seq, "accepted_ms": self.accepted_ms})
        }
    }

    /// A message to append: where, the sender's id for it, and its body.
    pub struct Outgoing<'a> {
        pub conversation: &'a str,
        /// The friendship's generation the sender saw.
        pub contact_generation: i64,
        pub client_id: &'a str,
        pub body: &'a Submitted,
        /// The leaf that encrypted it: the sender's device, or the device that
        /// approved an agent's message.
        pub leaf: &'a Leaf,
    }

    /// Who sends: the account, and the actor in its namespace.
    fn actor(caller: &Caller) -> Result<(&str, String), Refusal> {
        match caller {
            Caller::Device { account_id, .. } => Ok((account_id, "person".to_owned())),
            Caller::Backend { account_id, claktory_id, epoch, agent: Some(agent) } => Ok((
                account_id,
                json!({"claktory": claktory_id, "epoch": epoch, "agent": agent.id, "incarnation": agent.incarnation}).to_string(),
            )),
            _ => Err(Refusal(403, "device_only", "Only your own signed-in devices can do this.".to_owned())),
        }
    }

    fn granted(refused: grants::Refused) -> Refusal {
        match refused {
            grants::Refused::Quota(wait) => Refusal(429, "rate_limited", format!("Too many messages. Try again {}.", later(wait))),
            grants::Refused::Db(error) => Refusal::from(super::super::keys::Refused::Db(error)),
            _ => Refusal(403, "not_granted", "This agent hasn't been given that.".to_owned()),
        }
    }

    /// Appends a message to `conversation` from `caller`, in the caller's
    /// IMMEDIATE transaction: see the module's notes. The receipt, to give only
    /// once the transaction commits.
    pub fn append(transaction: &Transaction<'_>, caller: &Caller, outgoing: &Outgoing<'_>, now_ms: i64) -> Result<Receipt, Refusal> {
        let Outgoing { conversation, contact_generation, client_id, body: submitted, leaf } = *outgoing;
        let (account, actor) = actor(caller)?;
        // An agent first needs its grant, checked here, every time: a revoked
        // one is refused even retrying an earlier send.
        if matches!(caller, Caller::Backend { .. }) {
            let resource = Resource { kind: "conversation".into(), id: conversation.to_owned() };
            grants::live(transaction, caller, "dms", "dm.send", Some(&resource), now_ms).map_err(granted)?;
        }
        if !valid_client_id(client_id) {
            return Err(invalid("client_message_id has to be 22 to 64 letters, digits, - and _."));
        }
        let found: Option<(String, String, i64, Option<i64>)> = transaction
            .query_row(
                "SELECT low, high, generation, ended FROM dm_contacts WHERE conversation_id = ?1 AND (low = ?2 OR high = ?2)",
                params![conversation, account],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .optional()?;
        let (low, high, generation, ended) = found.ok_or_else(not_found)?;
        let digest = submitted.digest();

        // A retry: the first receipt, or a conflict.
        let earlier: Option<(String, i64, i64, Vec<u8>)> = transaction
            .query_row(
                "SELECT id, seq, accepted, digest FROM dm_messages WHERE author = ?1 AND actor = ?2 AND conversation_id = ?3 AND client_id = ?4",
                params![account, actor, conversation, client_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .optional()?;
        if let Some((message_id, seq, accepted_ms, was)) = earlier {
            if was != digest {
                return Err(Refusal(409, "conflict", "This message's id was already used for another message.".to_owned()));
            }
            return Ok(Receipt { message_id, seq, accepted_ms });
        }

        if contact_generation != generation {
            return Err(Refusal(409, "stale", "This conversation changed. Look again, then try again.".to_owned()));
        }
        let other = if low == account { &high } else { &low };
        let blocked: bool = transaction.query_row(
            "SELECT COUNT(*) > 0 FROM dm_blocks WHERE (blocker = ?1 AND blocked = ?2) OR (blocker = ?2 AND blocked = ?1)",
            params![account, other],
            |row| row.get(0),
        )?;
        if ended.is_some() || blocked {
            return Err(closed());
        }
        // In the conversation's group, at an epoch it still takes.
        let mut group = groups::of_conversation(transaction, conversation, generation)?
            .ok_or_else(|| Refusal(409, "not_set_up", "This conversation isn't set up for end-to-end encryption yet.".to_owned()))?;
        let epoch = groups::fits(&group, submitted)?;
        // Nothing goes in that the friend can't read: refused before
        // anything is counted or kept.
        if !groups::both_in(transaction, &group, epoch)? {
            return Err(groups::waiting_for_friend());
        }
        if matches!(caller, Caller::Device { .. }) {
            take_as(transaction, &format!("messages:{account}"), 60, PER_MINUTE, now_ms, "Too many messages.")?;
            take_as(transaction, &format!("messages-hour:{account}"), 3600, PER_HOUR, now_ms, "Too many messages.")?;
        }
        let message_id = hex(&random::<16>());
        let stored = submitted.store();
        // Both keep it, so it counts for both; the sender is refused past
        // their space, never told about the other's.
        let size = stored.size() as i64;
        if sync::usage(transaction, account)? + size > sync::QUOTA {
            return Err(Refusal(
                413,
                "storage_full",
                "Your messages use all your space. Delete some conversations' history to send more.".to_owned(),
            ));
        }

        // Numbered in the group's mailbox, with its commits, in the order
        // every member processes them.
        let by_agent = (actor != "person").then_some(actor.as_str());
        let entry = groups::Entry {
            kind: "application",
            epoch,
            envelope: submitted.envelope(),
            leaf,
            client_message_id: Some(client_id),
            by_agent,
            room: super::super::space::Entry::Capped,
        };
        let seq = groups::append(transaction, &mut group, entry, now_ms)?;
        transaction.execute(
            "INSERT INTO dm_conversations (id, last_seq) VALUES (?1, ?2) ON CONFLICT (id) DO UPDATE SET last_seq = ?2",
            params![conversation, seq],
        )?;
        // Its body is a new store too (`groups::append` checked already; kept
        // so if that ever moves).
        super::super::space::guard()?;
        transaction.execute(
            "INSERT INTO dm_messages (id, conversation_id, seq, author, actor, client_id, contact_generation, format, body, digest, accepted)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            params![message_id, conversation, seq, account, actor, client_id, generation, stored.format, stored.bytes, digest, now_ms],
        )?;
        for owner in [&low, &high] {
            sync::add_usage(transaction, owner, size)?;
            sync::emit(transaction, owner, Kind::Message, conversation, &message_id, seq, now_ms)?;
        }
        Ok(Receipt { message_id, seq, accepted_ms: now_ms })
    }

    /// `409 epoch` for a conversation's group, with its epoch now.
    pub fn epoch_of(db: &Connection, conversation: &str, generation: i64) -> Result<(u16, Value), Refusal> {
        let group = groups::of_conversation(db, conversation, generation)?.ok_or_else(not_found)?;
        Ok(groups::epoch_answer(group.epoch))
    }

    /// Ana and Bo, friends: their conversation and its generation.
    pub(in crate::dms) fn friends(world: &mut World, a: &str, b: &str) -> (String, i64) {
        let code = world.code(b);
        let request = world.send(a, &code);
        let (status, contact) = world.accept(b, &request);
        assert_eq!(status, 200, "{contact}");
        (contact["conversation_id"].as_str().expect("id").to_owned(), contact["generation"].as_i64().expect("generation"))
    }

    pub(in crate::dms) fn client_id(n: u32) -> String {
        format!("message-{n:018}")
    }

    /// A person's device as the routes see it.
    pub(in crate::dms) fn device(account: &str) -> Caller {
        Caller::Device {
            account_id: account.into(),
            device_key: format!("{account}-k"),
            registrations: Vec::new(),
            removed_devices: Vec::new(),
            signed_in_at: None,
            bound_devices: None,
        }
    }

    /// A send as claktory.com took it before (`POST
    /// conversations/<id>/messages`, refused now): kept, or refused.
    pub(in crate::dms) fn send_as(world: &mut World, caller: &Caller, conversation: &str, send: Value) -> (u16, Value) {
        let kept = (|| {
            let send: Send = identity::body(send.to_string().as_bytes())?;
            let submitted = Submitted::from_json(&send.envelope)?;
            let leaf = leaf_of(caller)?;
            let transaction = world.db.transaction_with_behavior(TransactionBehavior::Immediate)?;
            identity::ensure(&transaction, &world.keys, caller.account_id(), world.now)?;
            let outgoing = Outgoing {
                conversation,
                contact_generation: send.contact_generation,
                client_id: &send.client_message_id,
                body: &submitted,
                leaf: &leaf,
            };
            let answer = match append(&transaction, caller, &outgoing, world.now) {
                Ok(receipt) => (200, receipt.json()),
                Err(Refusal(409, "epoch", _)) => epoch_of(&transaction, conversation, send.contact_generation)?,
                Err(refused) => return Err(refused),
            };
            transaction.commit()?;
            Ok::<_, Refusal>(answer)
        })();
        kept.unwrap_or_else(|Refusal(status, error, message)| (status, json!({"error": error, "message": message})))
    }

    pub(in crate::dms) fn post(
        world: &mut World,
        account: &str,
        conversation: &str,
        generation: i64,
        id: &str,
        text: &str,
    ) -> (u16, Value) {
        let (group, epoch) = group_of(world, conversation, generation);
        let envelope = super::super::bodies::tests::sealed(&group, epoch, text);
        let send = json!({"client_message_id": id, "contact_generation": generation, "envelope": envelope, "request_nonce": "n"});
        send_as(world, &device(account), conversation, send)
    }

    /// The conversation's group (made here, if there's none, as if its
    /// device had: the worker never opens its messages), and its epoch.
    pub(in crate::dms) fn group_of(world: &mut World, conversation: &str, generation: i64) -> (Vec<u8>, u64) {
        if let Some(group) = groups::of_conversation(&world.db, conversation, generation).expect("groups") {
            return (group.id, group.epoch);
        }
        let id = sha2::Sha256::digest(format!("{conversation}:{generation}"))[..16].to_vec();
        world
            .db
            .execute(
                "INSERT INTO dm_groups (group_id, kind, subject, account_id, conversation_id, contact_generation, epoch, last_seq, group_info,
                 created) SELECT ?1, 'conversation', ?2, low, ?3, ?4, 0, 0, x'00', ?5 FROM dm_contacts WHERE conversation_id = ?3",
                params![id, format!("conversation:{conversation}:{generation}"), conversation, generation, world.now],
            )
            .expect("a group");
        (id, 0)
    }

    pub(in crate::dms) fn history(world: &mut World, account: &str, conversation: &str, query: &str) -> (u16, Value) {
        let route = format!("conversations/{conversation}/messages{}{query}", if query.is_empty() { "" } else { "?" });
        world.call(account, "GET", &route, Value::Null)
    }

    pub(in crate::dms) fn texts(page: &Value) -> Vec<String> {
        page["messages"]
            .as_array()
            .expect("messages")
            .iter()
            .map(|message| super::super::bodies::tests::opened(&message["envelope"]))
            .collect()
    }

    #[test]
    /// What claktory.com kept before it stopped keeping messages stays readable, in order, for
    /// both; a send now is refused, kept nowhere.
    fn what_was_kept_stays_readable_and_a_send_moved_to_the_home() {
        let mut world = World::new();
        world.name("ana", "Ana");
        let (conversation, generation) = friends(&mut world, "ana", "bo");

        let (status, receipt) = post(&mut world, "ana", &conversation, generation, &client_id(1), "Hi Bo 👋");
        assert_eq!(status, 200, "{receipt}");
        assert_eq!((receipt["seq"].as_i64(), receipt["accepted_ms"].as_i64()), (Some(1), Some(world.now)));
        // A retry (a lost answer) is the same message; other text with that
        // id is a conflict, never a second one.
        world.now += 5_000;
        assert_eq!(post(&mut world, "ana", &conversation, generation, &client_id(1), "Hi Bo 👋").1, receipt);
        assert_eq!(post(&mut world, "ana", &conversation, generation, &client_id(1), "Hi Bob").0, 409);
        // Bo's id is his own: the same one is another message.
        let (_, reply) = post(&mut world, "bo", &conversation, generation, &client_id(1), "Hi Ana");
        assert_eq!(reply["seq"], 2);

        // Both see both, in order, from where they stand.
        let (status, page) = history(&mut world, "bo", &conversation, "request_nonce=x");
        assert_eq!(status, 200, "{page}");
        assert_eq!(texts(&page), ["Hi Bo 👋", "Hi Ana"]);
        assert_eq!((page["messages"][0]["from"].as_str(), page["messages"][1]["from"].as_str()), (Some("them"), Some("you")));
        assert_eq!(page["messages"][0]["by_agent"], false);
        assert_eq!(page["has_more"], false);
        assert_eq!(history(&mut world, "ana", &conversation, "").1["messages"][0]["from"], "you");
        let (_, listed) = world.call("bo", "GET", "conversations", Value::Null);
        assert_eq!(listed["conversations"][0]["id"], conversation.as_str());
        assert_eq!(
            (listed["conversations"][0]["with"]["name"].as_str(), listed["conversations"][0]["last_seq"].as_i64()),
            (Some("Ana"), Some(2))
        );

        // Each of them has both in their outbox, in order.
        for account in ["ana", "bo"] {
            let events: Vec<i64> = {
                let mut statement = world
                    .db
                    .prepare("SELECT object_seq FROM dm_account_events WHERE account = ?1 AND kind = 'message' ORDER BY seq")
                    .expect("query");
                statement.query_map([account], |row| row.get(0)).expect("rows").collect::<Result<_, _>>().expect("rows")
            };
            assert_eq!(events, [1, 2], "{account}");
        }
        assert_eq!(listed["conversations"][0]["with"]["account_id"], "ana", "whose keys Bo's devices fetch");
        // Kept as the device sealed it, end to end.
        let (format, body): (i64, Vec<u8>) = world
            .db
            .query_row("SELECT format, body FROM dm_messages WHERE seq = 1", [], |row| Ok((row.get(0)?, row.get(1)?)))
            .expect("body");
        let sent = Envelope::from_json(&page["messages"][0]["envelope"]).expect("an envelope");
        assert_eq!((format, body.as_slice()), (bodies::END_TO_END, sent.bytes()));

        // Nobody else sees it; nor does a Claktory, as itself.
        assert_eq!(history(&mut world, "carol", &conversation, "").0, 404);
        let backend = Caller::Backend { account_id: "ana".into(), claktory_id: "mac".into(), epoch: 1, agent: None };
        let route = format!("conversations/{conversation}/messages");
        let (status, _) = identity::route(&mut world.db, &world.keys, "GET", &route, &backend, b"", world.now).expect("route");
        assert_eq!(status, 403);

        // A send goes through the person's home now: refused here, as it
        // is, and nothing kept.
        let (group, epoch) = group_of(&mut world, &conversation, generation);
        let envelope = super::super::bodies::tests::sealed(&group, epoch, "x");
        let send = json!({"client_message_id": client_id(9), "contact_generation": generation, "envelope": envelope});
        let (status, refused) = world.call("ana", "POST", &route, send);
        assert_eq!((status, refused["error"].as_str()), (410, Some("moved_to_home")), "{refused}");
        let kept: i64 = world.db.query_row("SELECT COUNT(*) FROM dm_messages", [], |row| row.get(0)).expect("count");
        assert_eq!(kept, 2);
    }

    /// Text that still fits an application message.
    const MAX: usize = 16 * 1024;

    #[test]
    fn a_block_or_removal_first_refuses_the_send_and_history_stays() {
        let mut world = World::new();
        let (conversation, generation) = friends(&mut world, "ana", "bo");
        let (_, first) = post(&mut world, "ana", &conversation, generation, &client_id(1), "before");
        let (_, contacts) = world.call("bo", "GET", "contacts", Value::Null);
        let contact = contacts["contacts"][0]["id"].clone();
        assert_eq!(world.call("bo", "POST", "blocks", json!({"contact_id": contact})).0, 200);

        let (status, refused) = post(&mut world, "ana", &conversation, generation, &client_id(2), "after");
        assert_eq!((status, refused["error"].as_str()), (410, Some("closed")), "{refused}");
        assert_eq!(post(&mut world, "bo", &conversation, generation, &client_id(2), "after").0, 410, "either way");
        // What was sent before stays, for both, and a retry of it still
        // answers its receipt.
        assert_eq!(post(&mut world, "ana", &conversation, generation, &client_id(1), "before").1, first);
        assert_eq!(texts(&history(&mut world, "bo", &conversation, "").1), ["before"]);
        assert_eq!(texts(&history(&mut world, "ana", &conversation, "").1), ["before"]);
        let (_, listed) = world.call("ana", "GET", "conversations", Value::Null);
        assert_eq!(
            (listed["conversations"][0]["active"].as_bool(), listed["conversations"][0]["last_seq"].as_i64()),
            (Some(false), Some(1))
        );

        // Removed without a block: the same.
        let (conversation, generation) = friends(&mut world, "carol", "dan");
        let (_, contacts) = world.call("dan", "GET", "contacts", Value::Null);
        let contact = contacts["contacts"][0]["id"].as_str().expect("id").to_owned();
        world.call("dan", "DELETE", &format!("contacts/{contact}"), Value::Null);
        assert_eq!(post(&mut world, "carol", &conversation, generation, &client_id(1), "hi").0, 410);
        let (_, listed) = world.call("carol", "GET", "conversations", Value::Null);
        assert_eq!(listed["conversations"], json!([]), "an ended friendship with no messages isn't listed");
    }

    #[test]
    fn an_ended_conversation_shows_the_other_as_they_were_when_it_ended() {
        let mut world = World::new();
        world.name("ana", "Ana");
        world.name("carol", "Carol");
        for (blocks, other) in [(true, "bo"), (false, "dan")] {
            let (conversation, generation) = friends(&mut world, other, "ana");
            post(&mut world, other, &conversation, generation, &client_id(1), "hi");
            let (_, contacts) = world.call("ana", "GET", "contacts", Value::Null);
            let contact = contacts["contacts"][0]["id"].as_str().expect("id").to_owned();
            if blocks {
                assert_eq!(world.call("ana", "POST", "blocks", json!({"contact_id": contact})).0, 200);
            } else {
                assert_eq!(world.call("ana", "DELETE", &format!("contacts/{contact}"), Value::Null).0, 204);
            }
            let (_, before) = world.call(other, "GET", "conversations", Value::Null);
            let (_, me) = world.call("ana", "GET", "me", Value::Null);
            let revision = me["profile"]["revision"].clone();
            world.call("ana", "PUT", "me/profile", json!({"name": "Ana's new name", "new_avatar": true, "revision": revision}));
            let (_, after) = world.call(other, "GET", "conversations", Value::Null);
            let with = &after["conversations"][0]["with"];
            assert_eq!(with["name"], "Ana", "blocked: {blocks}");
            assert_eq!(with, &before["conversations"][0]["with"], "nothing she changes after reaches them");
            assert_eq!(with["tag"].as_str().map(str::len), Some(6));
            world.name("ana", "Ana");
        }
        // A friend still sees her as she is.
        let (conversation, generation) = friends(&mut world, "carol", "ana");
        post(&mut world, "carol", &conversation, generation, &client_id(1), "hi");
        world.name("ana", "Ana again");
        let (_, listed) = world.call("carol", "GET", "conversations", Value::Null);
        assert_eq!(listed["conversations"][0]["with"]["name"], "Ana again");
    }

    #[test]
    fn history_comes_in_bounded_pages() {
        let mut world = World::new();
        let (conversation, generation) = friends(&mut world, "ana", "bo");
        for n in 1..=120 {
            if n % 50 == 0 {
                world.now += 60_000;
            }
            assert_eq!(post(&mut world, "ana", &conversation, generation, &client_id(n), &format!("m{n}")).0, 200);
        }
        let (_, latest) = history(&mut world, "bo", &conversation, "");
        assert_eq!(texts(&latest).len(), 50);
        assert_eq!((texts(&latest)[0].as_str(), texts(&latest)[49].as_str(), latest["has_more"].as_bool()), ("m71", "m120", Some(true)));
        let (_, earlier) = history(&mut world, "bo", &conversation, "before=71&limit=100");
        assert_eq!((texts(&earlier).len(), earlier["has_more"].as_bool()), (70, Some(false)));
        let (_, after) = history(&mut world, "bo", &conversation, "after=0&limit=100");
        assert_eq!((texts(&after)[0].as_str(), after["has_more"].as_bool()), ("m1", Some(true)));
        let (_, rest) = history(&mut world, "bo", &conversation, "after=100");
        assert_eq!((texts(&rest).len(), rest["has_more"].as_bool()), (20, Some(false)));
        for bad in ["limit=0", "limit=101", "after=-1", "after=1&before=5", "since=3", "after=x"] {
            assert_eq!(history(&mut world, "bo", &conversation, bad).0, 400, "{bad}");
        }

        // 256 KiB at most: sixteen 16 KiB messages and a bit more fill a page.
        let (conversation, generation) = friends(&mut world, "carol", "dan");
        for n in 1..=20 {
            post(&mut world, "carol", &conversation, generation, &client_id(n), &"x".repeat(MAX));
        }
        let (_, page) = history(&mut world, "dan", &conversation, "after=0&limit=100");
        let shown = texts(&page).len();
        assert!((14..=16).contains(&shown) && page["has_more"] == true, "{shown}");
    }

    #[test]
    fn a_person_sends_60_messages_a_minute() {
        let mut world = World::new();
        let (conversation, generation) = friends(&mut world, "ana", "bo");
        for n in 0..PER_MINUTE as u32 {
            assert_eq!(post(&mut world, "ana", &conversation, generation, &client_id(n), "x").0, 200);
        }
        assert_eq!(post(&mut world, "ana", &conversation, generation, &client_id(99), "x").0, 429);
    }

    #[test]
    fn an_agent_appends_only_with_a_live_send_grant_on_the_conversation() {
        let mut world = World::new();
        let (conversation, generation) = friends(&mut world, "ana", "bo");
        let alex = Actor { id: "alex".into(), incarnation: "inc-1".into() };
        let agent = Caller::Backend { account_id: "ana".into(), claktory_id: "mac".into(), epoch: 1, agent: Some(alex.clone()) };
        let backend = Caller::Backend { account_id: "ana".into(), claktory_id: "mac".into(), epoch: 1, agent: None };
        let grant = |scopes: &[&str]| grants::Put {
            app: "dms".into(),
            agent: alex.clone(),
            scopes: scopes.iter().map(|scope| (*scope).to_owned()).collect(),
            resources: vec![Resource { kind: "conversation".into(), id: conversation.clone() }],
            expires_ms: 0,
        };
        let (group, epoch) = group_of(&mut world, &conversation, generation);
        let Ok(text) = Submitted::from_json(&super::super::bodies::tests::sealed(&group, epoch, "From Alex")) else { panic!("text") };
        let leaf = Leaf { account: "ana".into(), leaf_id: "ana-phone".into(), kind: claktory_dm_crypto::certificate::LeafKind::Device };
        let send = |world: &mut World, n: u32| -> Result<Receipt, Refusal> {
            let transaction = world.db.transaction_with_behavior(TransactionBehavior::Immediate).expect("begin");
            let id = client_id(n);
            let outgoing =
                Outgoing { conversation: &conversation, contact_generation: generation, client_id: &id, body: &text, leaf: &leaf };
            let receipt = append(&transaction, &agent, &outgoing, world.now)?;
            transaction.commit().expect("commit");
            Ok(receipt)
        };
        let status = |result: Result<Receipt, Refusal>| result.map(|receipt| receipt.seq).map_err(|Refusal(status, _, _)| status);
        assert_eq!(status(send(&mut world, 1)), Err(403), "no grant");
        grants::put(&mut world.db, &backend, grant(&["dm.read"]), world.now).expect("granted");
        assert_eq!(status(send(&mut world, 1)), Err(403), "reading isn't sending");
        grants::put(&mut world.db, &backend, grant(&["dm.send"]), world.now).expect("granted");
        assert_eq!(status(send(&mut world, 1)), Ok(1));
        let (_, page) = history(&mut world, "bo", &conversation, "");
        assert_eq!((page["messages"][0]["by_agent"].as_bool(), page["messages"][0]["from"].as_str()), (Some(true), Some("them")));
        // Its id is its own: Ana's same id is another message.
        assert_eq!(post(&mut world, "ana", &conversation, generation, &client_id(1), "From Ana").1["seq"], 2);
        // Revoked: even a retry of what it sent is refused.
        let id: String = world.db.query_row("SELECT id FROM grants", [], |row| row.get(0)).expect("grant");
        grants::revoke(&mut world.db, &backend, &id, world.now).expect("revoked");
        assert_eq!(status(send(&mut world, 1)), Err(403));
    }
}
