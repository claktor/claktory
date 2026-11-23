//! An agent's message, sent only once the person approved exactly it
//! (D4b; dms.md §7, "Rivet path and exact-content review").
//!
//! - **Proposal.** An agent with a live `dm.send` grant on a conversation
//!   proposes a message (`POST conversations/<id>/proposals`, from its
//!   Claktory's backend, for the agent): an MLS application message in the
//!   Claktory's channel with the person's devices (E2), which the worker
//!   keeps as it came and can't read, bound to one revision, the conversation and its generation, and the
//!   agent (its Claktory, registration epoch and incarnation) and grant.
//!   Each proposal counts as a use of the grant's `dm.send`. Proposing again
//!   with the same id and the same bytes is a retry; with others, while it
//!   waits, it's the next revision, and an approval of an earlier one no
//!   longer holds.
//! - **Review.** Only the person, from one of their signed-in devices,
//!   approves (`POST proposals/<id>/approve`), naming the revision and the
//!   digest (SHA-256) of the exact proposal it decrypted and showed, and
//!   bringing the message itself, encrypted by the device in the
//!   conversation's group: the backend never says "approved". The
//!   approval's transaction checks the digest against the proposal kept, that the agent's Claktory is
//!   still registered under the epoch it proposed under, that the same
//!   grant still holds `dm.send` on the conversation, and that the
//!   friendship is still on at that generation; then it appends the
//!   device's message, once, as the agent's. It works with the agent's Claktory off.
//!   What can never be sent any more (the grant revoked, the Claktory signed
//!   out, the friendship ended) is `closed`.
//! - **Declining** ends it; the agent can withdraw it while it waits; it
//!   expires after 3 days. Using its text as a draft is the person's own
//!   message, sent with their own Send: nothing here.

use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest as _, Sha256};

use super::bodies::{Stored, Submitted, merge};
use super::groups;
use super::identity::{Refusal, body, clean_name, hex, invalid, random};
use super::leaves::leaf_of;
use super::messages::{Outgoing, append};
use super::sync::{self, Kind};
use crate::grants::{self, Resource};
use crate::ipc::{Actor, Caller};

pub const SCHEMA: &str = "
-- Messages agents proposed, waiting for the person's review or decided:
-- the text (sealed, as `dms::bodies` keeps it) at its latest revision, and
-- everything an approval is bound to. `state`: pending, sent, declined,
-- withdrawn or closed (it can't be sent any more); a pending one past
-- `expires` has expired.
CREATE TABLE IF NOT EXISTS dm_proposals (
    id TEXT PRIMARY KEY,
    account TEXT NOT NULL,
    conversation_id TEXT NOT NULL,
    contact_generation INTEGER NOT NULL,
    claktory_id TEXT NOT NULL,
    epoch INTEGER NOT NULL,
    agent_id TEXT NOT NULL,
    incarnation TEXT NOT NULL,
    agent_name TEXT NOT NULL,
    grant_id TEXT NOT NULL,
    client_id TEXT NOT NULL,
    revision INTEGER NOT NULL,
    format INTEGER NOT NULL,
    body BLOB NOT NULL,
    digest BLOB NOT NULL,
    state TEXT NOT NULL,
    created INTEGER NOT NULL,
    updated INTEGER NOT NULL,
    expires INTEGER NOT NULL,
    message_id TEXT,
    seq INTEGER,
    UNIQUE (account, claktory_id, epoch, agent_id, incarnation, conversation_id, client_id)
);
CREATE INDEX IF NOT EXISTS dm_proposals_account ON dm_proposals (account, state);
";

/// How long a proposal waits for the person.
pub const WAITS_MS: i64 = 3 * 86_400_000;
/// Proposals waiting at once per grant and conversation.
pub const WAITING_MOST: i64 = 5;
/// How long a decided or expired proposal is kept, for its agent to learn
/// what became of it.
const KEPT_MS: i64 = 7 * 86_400_000;
/// A proposal as kept.
struct Proposal {
    id: String,
    account: String,
    conversation: String,
    contact_generation: i64,
    claktory_id: String,
    epoch: i64,
    agent: Actor,
    agent_name: String,
    grant_id: String,
    client_id: String,
    revision: i64,
    stored: Stored,
    digest: Vec<u8>,
    state: String,
    created: i64,
    expires: i64,
    message_id: Option<String>,
    seq: Option<i64>,
}

const COLUMNS: &str = "id, account, conversation_id, contact_generation, claktory_id, epoch, agent_id, incarnation, agent_name, grant_id,
                       client_id, revision, format, body, digest, state, created, expires, message_id, seq";

fn read(row: &rusqlite::Row<'_>) -> rusqlite::Result<Proposal> {
    Ok(Proposal {
        id: row.get(0)?,
        account: row.get(1)?,
        conversation: row.get(2)?,
        contact_generation: row.get(3)?,
        claktory_id: row.get(4)?,
        epoch: row.get(5)?,
        agent: Actor { id: row.get(6)?, incarnation: row.get(7)? },
        agent_name: row.get(8)?,
        grant_id: row.get(9)?,
        client_id: row.get(10)?,
        revision: row.get(11)?,
        stored: Stored { format: row.get(12)?, bytes: row.get(13)? },
        digest: row.get(14)?,
        state: row.get(15)?,
        created: row.get(16)?,
        expires: row.get(17)?,
        message_id: row.get(18)?,
        seq: row.get(19)?,
    })
}

impl Proposal {
    /// Its state now: a pending one past its time has expired.
    fn state(&self, now_ms: i64) -> &str {
        if self.state == "pending" && self.expires <= now_ms { "expired" } else { &self.state }
    }

    /// The agent, as the caller it proposed as.
    fn caller(&self) -> Caller {
        Caller::Backend {
            account_id: self.account.clone(),
            claktory_id: self.claktory_id.clone(),
            epoch: self.epoch as u64,
            agent: Some(self.agent.clone()),
        }
    }

    /// What the person's device approves by: SHA-256 of the proposal's
    /// bytes, in lowercase hex.
    fn digest(&self) -> String {
        hex(&Sha256::digest(&self.stored.bytes))
    }

    /// What its agent learns of it: never the person's words, only what
    /// became of its own.
    fn status(&self, now_ms: i64) -> Value {
        let state = self.state(now_ms);
        let mut status = json!({
            "proposal_id": self.id,
            "revision": self.revision,
            "state": state,
            "expires_ms": self.expires,
            "waiting": state == "pending",
        });
        if let (Some(message_id), Some(seq)) = (&self.message_id, self.seq) {
            status["message_id"] = json!(message_id);
            status["seq"] = json!(seq);
        }
        status
    }

    /// What the person reviews: the proposal as the agent's Claktory
    /// encrypted it, and what it's bound to.
    fn shown(&self, now_ms: i64) -> Result<Value, Refusal> {
        let mut shown = json!({
            "id": self.id,
            "revision": self.revision,
            "state": self.state(now_ms),
            "conversation_id": self.conversation,
            "contact_generation": self.contact_generation,
            "from": {
                "claktory_id": self.claktory_id,
                "agent_id": self.agent.id,
                "agent_incarnation": self.agent.incarnation,
                "agent_name": self.agent_name,
            },
            "proposal_digest": self.digest(),
            "created_ms": self.created,
            "expires_ms": self.expires,
        });
        merge(&mut shown, self.stored.shown()?);
        Ok(shown)
    }
}

fn not_found() -> Refusal {
    Refusal(404, "not_found", "There's nothing here.".to_owned())
}

fn not_granted() -> Refusal {
    Refusal(403, "not_granted", "This agent hasn't been given that.".to_owned())
}

fn granted(refused: grants::Refused) -> Refusal {
    match refused {
        grants::Refused::Quota(wait) => {
            Refusal(429, "rate_limited", format!("Too many messages. Try again {}.", super::identity::later(wait)))
        }
        grants::Refused::Db(error) => Refusal::from(super::keys::Refused::Db(error)),
        _ => not_granted(),
    }
}

/// The agent a caller acts for, or a refusal: only a Claktory's backend,
/// for one of its agents, proposes.
fn agent_of(caller: &Caller) -> Result<(&str, &str, u64, &Actor), Refusal> {
    match caller {
        Caller::Backend { account_id, claktory_id, epoch, agent: Some(agent) } => Ok((account_id, claktory_id, *epoch, agent)),
        _ => Err(not_granted()),
    }
}

fn device_only() -> Refusal {
    Refusal(403, "device_only", "Only your own signed-in devices can do this.".to_owned())
}

fn find(db: &Connection, id: &str, account: &str) -> Result<Proposal, Refusal> {
    db.query_row(&format!("SELECT {COLUMNS} FROM dm_proposals WHERE id = ?1 AND account = ?2"), params![id, account], read)
        .optional()?
        .ok_or_else(not_found)
}

/// Whether the agent is the caller: its own proposal.
fn owns(proposal: &Proposal, caller: &Caller) -> bool {
    matches!(caller, Caller::Backend { account_id, claktory_id, epoch, agent: Some(agent) }
        if *account_id == proposal.account && *claktory_id == proposal.claktory_id
            && *epoch as i64 == proposal.epoch && *agent == proposal.agent)
}

fn decide(transaction: &Transaction<'_>, proposal: &Proposal, state: &str, now_ms: i64) -> rusqlite::Result<()> {
    transaction.execute("UPDATE dm_proposals SET state = ?2, updated = ?3 WHERE id = ?1", params![proposal.id, state, now_ms])?;
    sync::emit(transaction, &proposal.account, Kind::Proposal, &proposal.conversation, &proposal.id, proposal.revision, now_ms)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Propose {
    client_message_id: String,
    contact_generation: i64,
    /// The proposal: an MLS application message in the Claktory's channel.
    envelope: Value,
    /// The agent's name, as the person knows it (it rides in the content
    /// too).
    #[serde(default)]
    agent_name: String,
    #[serde(default)]
    #[allow(dead_code)]
    request_nonce: Option<String>,
}

/// An agent proposes a message, or retries, or revises one still waiting.
fn propose(transaction: &Transaction<'_>, caller: &Caller, conversation: &str, bytes: &[u8], now_ms: i64) -> Result<(u16, Value), Refusal> {
    let (account, claktory_id, epoch, agent) = agent_of(caller)?;
    let propose: Propose = body(bytes)?;
    if !super::messages::valid_client_id(&propose.client_message_id) {
        return Err(invalid("client_message_id has to be 22 to 64 letters, digits, - and _."));
    }
    let agent_name = match propose.agent_name.as_str() {
        "" => String::new(),
        name => clean_name(name).ok_or_else(|| invalid("agent_name is 1 to 80 characters, all of them visible."))?,
    };
    let submitted = Submitted::from_json(&propose.envelope)?;
    let resource = Resource { kind: "conversation".into(), id: conversation.to_owned() };
    let digest = submitted.digest();
    let earlier = transaction
        .query_row(
            &format!(
                "SELECT {COLUMNS} FROM dm_proposals WHERE account = ?1 AND claktory_id = ?2 AND epoch = ?3 AND agent_id = ?4
                 AND incarnation = ?5 AND conversation_id = ?6 AND client_id = ?7"
            ),
            params![account, claktory_id, epoch as i64, agent.id, agent.incarnation, conversation, propose.client_message_id],
            read,
        )
        .optional()?;
    if let Some(earlier) = &earlier {
        if earlier.digest == digest {
            // A retry: what became of it, the grant still checked.
            grants::live(transaction, caller, "dms", "dm.send", Some(&resource), now_ms).map_err(granted)?;
            return Ok((200, earlier.status(now_ms)));
        }
        if earlier.state(now_ms) != "pending" {
            return Err(Refusal(409, "conflict", "This proposal was already decided. Propose the new text under a new id.".to_owned()));
        }
    }
    // In the Claktory's channel with the person's devices, at an epoch it
    // still takes.
    let channel = groups::channel_of(transaction, account, claktory_id)?
        .ok_or_else(|| Refusal(409, "not_set_up", "This Claktory has no channel with the person's devices yet.".to_owned()))?;
    match groups::fits(&channel, &submitted) {
        Ok(_) => {}
        Err(Refusal(409, "epoch", _)) => return Ok(groups::epoch_answer(channel.epoch)),
        Err(refused) => return Err(refused),
    }
    let grant_id = grants::check(transaction, caller, "dms", "dm.send", Some(&resource), now_ms).map_err(granted)?;
    let contact: Option<(i64, Option<i64>)> = transaction
        .query_row(
            "SELECT generation, ended FROM dm_contacts WHERE conversation_id = ?1 AND (low = ?2 OR high = ?2)",
            params![conversation, account],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?;
    let (generation, ended) = contact.ok_or_else(not_found)?;
    if ended.is_some() {
        return Err(Refusal(410, "closed", "You can't send messages in this conversation anymore.".to_owned()));
    }
    if generation != propose.contact_generation {
        return Err(Refusal(409, "stale", "This conversation changed. Look again, then try again.".to_owned()));
    }
    let (id, revision) = match &earlier {
        Some(earlier) => (earlier.id.clone(), earlier.revision + 1),
        None => {
            let waiting: i64 = transaction.query_row(
                "SELECT COUNT(*) FROM dm_proposals WHERE grant_id = ?1 AND conversation_id = ?2 AND state = 'pending' AND expires > ?3",
                params![grant_id, conversation, now_ms],
                |row| row.get(0),
            )?;
            if waiting >= WAITING_MOST {
                return Err(Refusal(
                    429,
                    "too_many_waiting",
                    "Too many messages are waiting for review in this conversation. Wait for the person, or withdraw one.".to_owned(),
                ));
            }
            (hex(&random::<16>()), 1)
        }
    };
    let stored = submitted.store();
    let expires = now_ms + WAITS_MS;
    transaction.execute(
        "INSERT INTO dm_proposals (id, account, conversation_id, contact_generation, claktory_id, epoch, agent_id, incarnation,
                                   agent_name, grant_id, client_id, revision, format, body, digest, state, created, updated, expires)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, 'pending', ?16, ?16, ?17)
         ON CONFLICT (id) DO UPDATE SET contact_generation = ?4, agent_name = ?9, grant_id = ?10, revision = ?12, format = ?13,
                                        body = ?14, digest = ?15, updated = ?16, expires = ?17",
        params![
            id,
            account,
            conversation,
            generation,
            claktory_id,
            epoch as i64,
            agent.id,
            agent.incarnation,
            agent_name,
            grant_id,
            propose.client_message_id,
            revision,
            stored.format,
            stored.bytes,
            digest,
            now_ms,
            expires
        ],
    )?;
    sync::emit(transaction, account, Kind::Proposal, conversation, &id, revision, now_ms)?;
    Ok((200, find(transaction, &id, account)?.status(now_ms)))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Approve {
    revision: i64,
    /// SHA-256 of the proposal the device decrypted, in hex.
    proposal_digest: String,
    /// The approved message, encrypted by the device in the conversation's
    /// group.
    envelope: Value,
    #[serde(default)]
    #[allow(dead_code)]
    request_nonce: Option<String>,
}

/// The person approves exactly what their device showed them: see the
/// module's notes. Something that can't be sent any more is `410
/// unsendable`, and stays so; a change since is `409 changed`.
fn approve(transaction: &Transaction<'_>, caller: &Caller, id: &str, bytes: &[u8], now_ms: i64) -> Result<(u16, Value), Refusal> {
    let Caller::Device { account_id, registrations, .. } = caller else { return Err(device_only()) };
    let approve: Approve = body(bytes)?;
    let proposal = find(transaction, id, account_id)?;
    let changed = || Refusal(409, "changed", "This message changed since you looked. Look again.".to_owned());
    match proposal.state(now_ms) {
        "pending" => {}
        "sent" if proposal.revision == approve.revision => return Ok((200, proposal.status(now_ms))),
        "sent" => return Err(changed()),
        state => {
            let message = match state {
                "declined" => "You declined this message.",
                "withdrawn" => "The agent took this message back.",
                "expired" => "This message waited too long. Ask the agent again.",
                _ => "This message can't be sent anymore.",
            };
            return Err(Refusal(410, "unsendable", message.to_owned()));
        }
    }
    if proposal.revision != approve.revision {
        return Err(changed());
    }
    if proposal.digest() != approve.proposal_digest.to_ascii_lowercase() {
        return Err(changed());
    }
    let submitted = Submitted::from_json(&approve.envelope)?;
    let leaf = leaf_of(caller)?;
    let unsendable = |transaction: &Transaction<'_>| -> Result<(u16, Value), Refusal> {
        decide(transaction, &proposal, "closed", now_ms)?;
        Ok((410, json!({"error": "unsendable", "message": "This message can't be sent anymore.", "state": "closed"})))
    };
    // Its Claktory still registered as it was, and the same grant still
    // holding: a new registration, incarnation or grant never inherits it.
    let registered = registrations.iter().any(|known| known.claktory_id == proposal.claktory_id && known.epoch as i64 == proposal.epoch);
    let agent = proposal.caller();
    let resource = Resource { kind: "conversation".into(), id: proposal.conversation.clone() };
    let holds = grants::live(transaction, &agent, "dms", "dm.send", Some(&resource), now_ms);
    match holds {
        Ok(grant) if registered && grant == proposal.grant_id => {}
        Ok(_) | Err(grants::Refused::NotGranted) => return unsendable(transaction),
        Err(refused) => return Err(granted(refused)),
    }
    let outgoing = Outgoing {
        conversation: &proposal.conversation,
        contact_generation: proposal.contact_generation,
        client_id: &proposal.client_id,
        body: &submitted,
        leaf: &leaf,
    };
    let receipt = match append(transaction, &agent, &outgoing, now_ms) {
        Ok(receipt) => receipt,
        // The device's message is behind, or not yet possible: nothing
        // decided.
        Err(Refusal(409, "epoch", _)) => {
            return super::messages::epoch_of(transaction, &proposal.conversation, proposal.contact_generation);
        }
        // Not yet: the friend isn't in the conversation (row 676).
        Err(refused @ Refusal(400 | 409, "invalid_request" | "not_set_up" | "conflict" | "waiting_for_friend", _)) => return Err(refused),
        // The friendship ended or began again since: never this one.
        Err(Refusal(410 | 409 | 403, ..)) => return unsendable(transaction),
        Err(refused) => return Err(refused),
    };
    transaction.execute(
        "UPDATE dm_proposals SET state = 'sent', updated = ?2, message_id = ?3, seq = ?4 WHERE id = ?1",
        params![proposal.id, now_ms, receipt.message_id, receipt.seq],
    )?;
    sync::emit(transaction, account_id, Kind::Proposal, &proposal.conversation, &proposal.id, proposal.revision, now_ms)?;
    Ok((200, find(transaction, id, account_id)?.status(now_ms)))
}

/// The person's proposals waiting for them, that could still be sent:
/// newest first.
pub fn waiting(db: &Connection, caller: &Caller, now_ms: i64) -> Result<Value, Refusal> {
    let Caller::Device { account_id, registrations, .. } = caller else { return Err(device_only()) };
    let rows: Vec<Proposal> = {
        let mut statement = db.prepare(&format!(
            "SELECT {} FROM dm_proposals p JOIN dm_contacts c ON c.conversation_id = p.conversation_id
             WHERE p.account = ?1 AND p.state = 'pending' AND p.expires > ?2 AND c.ended IS NULL
             AND c.generation = p.contact_generation ORDER BY p.updated DESC, p.id",
            COLUMNS.split(',').map(|column| format!("p.{}", column.trim())).collect::<Vec<_>>().join(", ")
        ))?;
        statement.query_map(params![account_id, now_ms], read)?.collect::<Result<_, _>>()?
    };
    let mut shown = Vec::new();
    for proposal in rows {
        let registered =
            registrations.iter().any(|known| known.claktory_id == proposal.claktory_id && known.epoch as i64 == proposal.epoch);
        let resource = Resource { kind: "conversation".into(), id: proposal.conversation.clone() };
        let holds = grants::live(db, &proposal.caller(), "dms", "dm.send", Some(&resource), now_ms);
        if registered && holds.is_ok_and(|grant| grant == proposal.grant_id) {
            shown.push(proposal.shown(now_ms)?);
        }
    }
    Ok(json!(shown))
}

/// Whether `route` is this module's.
pub fn knows(method: &str, route: &str) -> bool {
    let parts: Vec<&str> = route.split('/').collect();
    matches!(
        (method, parts.as_slice()),
        ("POST", ["conversations", _, "proposals"])
            | ("GET", ["proposals"])
            | ("GET" | "DELETE", ["proposals", _])
            | ("POST", ["proposals", _, "approve" | "decline"])
    )
}

/// Answers one of this module's routes, for the caller each allows.
pub fn answer(
    db: &mut Connection,
    method: &str,
    (route, query): (&str, &str),
    caller: &Caller,
    bytes: &[u8],
    now_ms: i64,
) -> Result<(u16, Value), Refusal> {
    let account = caller.account_id().to_owned();
    let transaction = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let parts: Vec<&str> = route.split('/').collect();
    let answer = match (method, parts.as_slice()) {
        ("POST", ["conversations", conversation, "proposals"]) => {
            let (status, answer) = propose(&transaction, caller, conversation, bytes, now_ms)?;
            if status != 200 {
                return Ok((status, answer));
            }
            answer
        }
        ("GET", ["proposals"]) => json!({"proposals": waiting(&transaction, caller, now_ms)?}),
        ("GET", ["proposals", id]) => {
            for pair in query.split('&').filter(|pair| !pair.is_empty()) {
                let (name, value) = pair.split_once('=').unwrap_or((pair, ""));
                match name {
                    "wait" if value.parse::<u64>().is_ok_and(|wait| wait <= sync::MAX_WAIT) => {}
                    "request_nonce" => {}
                    _ => return Err(invalid("A proposal's status takes wait (0 to 25 seconds).")),
                }
            }
            let proposal = find(&transaction, id, &account)?;
            match caller {
                Caller::Device { .. } => proposal.shown(now_ms)?,
                _ if owns(&proposal, caller) => {
                    let resource = Resource { kind: "conversation".into(), id: proposal.conversation.clone() };
                    grants::live(&transaction, caller, "dms", "dm.send", Some(&resource), now_ms).map_err(granted)?;
                    proposal.status(now_ms)
                }
                _ => return Err(not_found()),
            }
        }
        ("DELETE", ["proposals", id]) => {
            agent_of(caller)?;
            let proposal = find(&transaction, id, &account)?;
            if !owns(&proposal, caller) {
                return Err(not_found());
            }
            if proposal.state(now_ms) == "pending" {
                decide(&transaction, &proposal, "withdrawn", now_ms)?;
            }
            find(&transaction, id, &account)?.status(now_ms)
        }
        ("POST", ["proposals", id, "approve"]) => {
            let (status, answer) = approve(&transaction, caller, id, bytes, now_ms)?;
            transaction.commit()?;
            return Ok((status, answer));
        }
        ("POST", ["proposals", id, "decline"]) => {
            if !matches!(caller, Caller::Device { .. }) {
                return Err(device_only());
            }
            #[derive(Deserialize)]
            #[serde(deny_unknown_fields)]
            struct Decline {
                #[serde(default)]
                #[allow(dead_code)]
                request_nonce: Option<String>,
            }
            body::<Decline>(bytes)?;
            let proposal = find(&transaction, id, &account)?;
            match proposal.state(now_ms) {
                "pending" => decide(&transaction, &proposal, "declined", now_ms)?,
                "declined" | "withdrawn" | "expired" | "closed" => {}
                _ => return Err(Refusal(409, "conflict", "This message was already sent.".to_owned())),
            }
            find(&transaction, id, &account)?.status(now_ms)
        }
        _ => return Err(not_found()),
    };
    transaction.commit()?;
    Ok((200, answer))
}

/// Deletes proposals decided, or expired, a week ago. How many went.
pub fn sweep(db: &mut Connection, now_ms: i64) -> Result<usize, Refusal> {
    let before = now_ms - KEPT_MS;
    Ok(db.execute("DELETE FROM dm_proposals WHERE (state != 'pending' AND updated < ?1) OR expires < ?1", [before])?)
}

/// An account is deleted: its agents' proposals go.
pub fn purge(transaction: &Transaction<'_>, account: &str) -> rusqlite::Result<()> {
    transaction.execute("DELETE FROM dm_proposals WHERE account = ?1", [account])?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::super::bodies::tests::{opened, sealed};
    use super::super::contacts::tests::World;
    use super::super::messages::tests::{client_id, friends, group_of, history, texts};
    use super::*;
    use crate::ipc::Registration;

    /// Ana and Bo, friends; Ana's agent June, on her Mac mini (registered
    /// under epoch 1), may send in their conversation.
    struct Review {
        world: World,
        conversation: String,
        generation: i64,
        june: Caller,
        /// The Mac's channel with Ana's devices, where proposals come.
        channel: Vec<u8>,
    }

    fn june() -> Actor {
        Actor { id: "june".into(), incarnation: "inc-1".into() }
    }

    fn setup() -> Review {
        let mut world = World::new();
        let (conversation, generation) = friends(&mut world, "ana", "bo");
        let backend = Caller::Backend { account_id: "ana".into(), claktory_id: "mac".into(), epoch: 1, agent: None };
        let put = grants::Put {
            app: "dms".into(),
            agent: june(),
            scopes: vec!["dm.send".into()],
            resources: vec![Resource { kind: "conversation".into(), id: conversation.clone() }],
            expires_ms: 0,
        };
        grants::put(&mut world.db, &backend, put, world.now).expect("granted");
        let june = Caller::Backend { account_id: "ana".into(), claktory_id: "mac".into(), epoch: 1, agent: Some(june()) };
        let channel = vec![4; 16];
        world
            .db
            .execute(
                "INSERT INTO dm_groups (group_id, kind, subject, account_id, claktory_id, epoch, last_seq, group_info, created)
                 VALUES (?1, 'channel', 'channel:ana:mac', 'ana', 'mac', 0, 0, x'00', 0)",
                [&channel],
            )
            .expect("a channel");
        group_of(&mut world, &conversation, generation);
        Review { world, conversation, generation, june, channel }
    }

    fn device(account: &str, epoch: u64) -> Caller {
        Caller::Device {
            account_id: account.into(),
            device_key: "k".into(),
            registrations: vec![Registration { claktory_id: "mac".into(), epoch }],
            removed_devices: Vec::new(),
            signed_in_at: None,
            bound_devices: None,
        }
    }

    fn call(world: &mut World, caller: &Caller, method: &str, route: &str, body: Value) -> (u16, Value) {
        let bytes = if body.is_null() { Vec::new() } else { body.to_string().into_bytes() };
        super::super::identity::route(&mut world.db, &world.keys, method, route, caller, &bytes, world.now).expect("a route")
    }

    impl Review {
        fn propose(&mut self, n: u32, text: &str) -> (u16, Value) {
            let envelope = sealed(&self.channel, 0, text);
            let body = json!({"client_message_id": client_id(n), "contact_generation": self.generation, "envelope": envelope,
                              "agent_name": "June", "request_nonce": "x"});
            let (june, route) = (self.june.clone(), format!("conversations/{}/proposals", self.conversation));
            call(&mut self.world, &june, "POST", &route, body)
        }

        fn waiting(&mut self, account: &str, epoch: u64) -> Vec<Value> {
            let (status, listed) = call(&mut self.world, &device(account, epoch), "GET", "proposals", Value::Null);
            assert_eq!(status, 200, "{listed}");
            listed["proposals"].as_array().expect("proposals").clone()
        }

        /// What the person's device sends: the digest of the proposal it
        /// decrypted and showed, computed itself, and the message encrypted
        /// again in the conversation's group.
        fn approval(&mut self, shown: &Value) -> Value {
            let bytes = claktory_dm_crypto::envelope::Envelope::from_json(&shown["envelope"]).expect("an envelope").into_bytes();
            let digest = hex(&Sha256::digest(&bytes));
            let (group, epoch) = group_of(&mut self.world, &self.conversation, self.generation);
            let envelope = sealed(&group, epoch, &opened(&shown["envelope"]));
            json!({"revision": shown["revision"], "proposal_digest": digest, "envelope": envelope, "request_nonce": "y"})
        }

        fn approve(&mut self, id: &str, approval: Value, epoch: u64) -> (u16, Value) {
            call(&mut self.world, &device("ana", epoch), "POST", &format!("proposals/{id}/approve"), approval)
        }
    }

    #[test]
    fn an_agent_s_message_is_sent_only_once_the_person_approves_exactly_it() {
        let mut review = setup();
        let (status, proposed) = review.propose(1, "See you at 6");
        assert_eq!(status, 200, "{proposed}");
        assert_eq!(
            (proposed["state"].as_str(), proposed["revision"].as_i64(), proposed["waiting"].as_bool()),
            (Some("pending"), Some(1), Some(true))
        );
        assert!(proposed.get("envelope").is_none(), "its agent learns only what became of it");
        assert_eq!(review.propose(1, "See you at 6").1, proposed, "a retry");
        let id = proposed["proposal_id"].as_str().expect("id").to_owned();
        assert!(
            history(&mut review.world, "bo", &review.conversation, "").1["messages"].as_array().is_some_and(Vec::is_empty),
            "nothing sent yet"
        );

        // Ana's devices see it, all of it; Bo's don't.
        let waiting = review.waiting("ana", 1);
        assert_eq!(waiting.len(), 1);
        assert_eq!((opened(&waiting[0]["envelope"]).as_str(), waiting[0]["from"]["agent_name"].as_str()), ("See you at 6", Some("June")));
        assert_eq!(waiting[0]["epoch"], 0);
        assert!(review.waiting("bo", 1).is_empty());
        let (_, snapshot) = call(&mut review.world, &device("ana", 1), "GET", "sync", Value::Null);
        assert_eq!(snapshot["proposals"][0]["id"], id.as_str());
        let approval = review.approval(&waiting[0]);
        assert_eq!(call(&mut review.world, &device("bo", 1), "POST", &format!("proposals/{id}/approve"), approval.clone()).0, 404);
        // The backend never approves, for itself or as the agent.
        let june = review.june.clone();
        let (status, refused) = call(&mut review.world, &june, "POST", &format!("proposals/{id}/approve"), approval.clone());
        assert_eq!((status, refused["error"].as_str()), (403, Some("device_only")));

        // A digest of something else than what it showed, or an earlier
        // revision, is refused.
        let mut wrong = approval.clone();
        wrong["proposal_digest"] = json!("0".repeat(64));
        assert_eq!(review.approve(&id, wrong, 1).1["error"], "changed");
        let (status, revised) = review.propose(1, "See you at 7");
        assert_eq!((status, revised["revision"].as_i64()), (200, Some(2)));
        assert_eq!(review.approve(&id, approval, 1).1["error"], "changed", "the text changed since");

        // The device's message, behind the conversation's group: nothing
        // decided, try again.
        let waiting = review.waiting("ana", 1);
        let approval = review.approval(&waiting[0]);
        let (group, _) = group_of(&mut review.world, &review.conversation, review.generation);
        review.world.db.execute("UPDATE dm_groups SET epoch = 6 WHERE group_id = ?1", [&group]).expect("moved on");
        let (status, behind) = review.approve(&id, approval, 1);
        assert_eq!((status, &behind["epoch"]), (409, &json!(6)), "{behind}");
        assert_eq!(review.waiting("ana", 1).len(), 1, "still waiting");
        // Bo's devices not in the conversation's group yet (row 676): not
        // yet, nothing decided either.
        review.world.db.execute("INSERT INTO dm_group_people (group_id, both_since) VALUES (?1, NULL)", [&group]).expect("not both");
        let approval = review.approval(&waiting[0]);
        let (status, refused) = review.approve(&id, approval, 1);
        assert_eq!((status, refused["error"].as_str()), (409, Some("waiting_for_friend")), "{refused}");
        assert_eq!(review.waiting("ana", 1).len(), 1, "still waiting");
        review.world.db.execute("UPDATE dm_group_people SET both_since = 0 WHERE group_id = ?1", [&group]).expect("both in");
        let approval = review.approval(&waiting[0]);
        let (status, sent) = review.approve(&id, approval.clone(), 1);
        assert_eq!(status, 200, "{sent}");
        assert_eq!((sent["state"].as_str(), sent["seq"].as_i64()), (Some("sent"), Some(1)));
        let (_, page) = history(&mut review.world, "bo", &review.conversation, "");
        assert_eq!(texts(&page), ["See you at 7"]);
        assert_eq!((page["messages"][0]["by_agent"].as_bool(), page["messages"][0]["from"].as_str()), (Some(true), Some("them")));
        assert_eq!(review.approve(&id, approval, 1).1, sent, "approving again sends nothing more");
        assert_eq!(texts(&history(&mut review.world, "bo", &review.conversation, "").1).len(), 1);
        let (_, status) = call(&mut review.world, &june, "GET", &format!("proposals/{id}"), Value::Null);
        assert_eq!((status["state"].as_str(), status["waiting"].as_bool()), (Some("sent"), Some(false)));
        assert!(review.waiting("ana", 1).is_empty());
        let (status, _) = call(&mut review.world, &device("ana", 1), "POST", &format!("proposals/{id}/decline"), json!({}));
        assert_eq!(status, 409, "sent is sent");
        assert_eq!(review.propose(1, "See you at 8").1["error"], "conflict", "decided: a new one has a new id");

        // Ana's devices heard of each step; another agent doesn't see it.
        let kinds: i64 = review
            .world
            .db
            .query_row("SELECT COUNT(*) FROM dm_account_events WHERE account = 'ana' AND kind = 'proposal'", [], |row| row.get(0))
            .expect("count");
        assert_eq!(kinds, 3, "proposed, revised, sent");
        let other = Caller::Backend {
            account_id: "ana".into(),
            claktory_id: "mac".into(),
            epoch: 1,
            agent: Some(Actor { id: "june".into(), incarnation: "inc-2".into() }),
        };
        assert_eq!(call(&mut review.world, &other, "GET", &format!("proposals/{id}"), Value::Null).0, 404, "June made again isn't June");
    }

    #[test]
    fn what_can_t_be_sent_anymore_is_closed_and_isn_t_shown() {
        // The grant taken back.
        let mut review = setup();
        let id = review.propose(1, "hi").1["proposal_id"].as_str().expect("id").to_owned();
        let waiting = review.waiting("ana", 1);
        let approval = review.approval(&waiting[0]);
        let grant: String = review.world.db.query_row("SELECT id FROM grants", [], |row| row.get(0)).expect("grant");
        let Ok(()) = grants::revoke(&mut review.world.db, &device("ana", 1), &grant, review.world.now) else { panic!("revoked") };
        assert!(review.waiting("ana", 1).is_empty());
        let (status, closed) = review.approve(&id, approval.clone(), 1);
        assert_eq!((status, closed["error"].as_str(), closed["state"].as_str()), (410, Some("unsendable"), Some("closed")));
        assert_eq!(review.approve(&id, approval, 1).0, 410, "for good");

        // The grant replaced, even by one like it: approved under another.
        let mut review = setup();
        let id = review.propose(1, "hi").1["proposal_id"].as_str().expect("id").to_owned();
        let waiting = review.waiting("ana", 1);
        let approval = review.approval(&waiting[0]);
        let backend = Caller::Backend { account_id: "ana".into(), claktory_id: "mac".into(), epoch: 1, agent: None };
        let put = grants::Put {
            app: "dms".into(),
            agent: june(),
            scopes: vec!["dm.send".into()],
            resources: vec![Resource { kind: "conversation".into(), id: review.conversation.clone() }],
            expires_ms: 0,
        };
        grants::put(&mut review.world.db, &backend, put, review.world.now).expect("granted again");
        assert!(review.waiting("ana", 1).is_empty());
        assert_eq!(review.approve(&id, approval, 1).1["error"], "unsendable");

        // The Mac signed in again: a new registration inherits nothing.
        let mut review = setup();
        let id = review.propose(1, "hi").1["proposal_id"].as_str().expect("id").to_owned();
        let waiting = review.waiting("ana", 1);
        let approval = review.approval(&waiting[0]);
        assert!(review.waiting("ana", 2).is_empty());
        assert_eq!(review.approve(&id, approval, 2).0, 410);

        // Bo blocked Ana.
        let mut review = setup();
        let id = review.propose(1, "hi").1["proposal_id"].as_str().expect("id").to_owned();
        let waiting = review.waiting("ana", 1);
        let approval = review.approval(&waiting[0]);
        let (_, contacts) = review.world.call("bo", "GET", "contacts", Value::Null);
        let contact = contacts["contacts"][0]["id"].clone();
        assert_eq!(review.world.call("bo", "POST", "blocks", json!({"contact_id": contact})).0, 200);
        assert!(review.waiting("ana", 1).is_empty());
        assert_eq!(review.approve(&id, approval, 1).1["error"], "unsendable");
        assert!(texts(&history(&mut review.world, "bo", &review.conversation, "").1).is_empty());

        // Three days pass.
        let mut review = setup();
        let id = review.propose(1, "hi").1["proposal_id"].as_str().expect("id").to_owned();
        let waiting = review.waiting("ana", 1);
        let approval = review.approval(&waiting[0]);
        review.world.now += WAITS_MS;
        assert!(review.waiting("ana", 1).is_empty());
        let (status, expired) = review.approve(&id, approval, 1);
        assert_eq!((status, expired["message"].as_str()), (410, Some("This message waited too long. Ask the agent again.")));
        let june = review.june.clone();
        assert_eq!(call(&mut review.world, &june, "GET", &format!("proposals/{id}"), Value::Null).1["state"], "expired");
        // A week after, it's gone.
        review.world.now += KEPT_MS + 1;
        assert_eq!(sweep(&mut review.world.db, review.world.now).ok(), Some(1));
    }

    #[test]
    fn the_person_declines_the_agent_withdraws_and_only_a_few_wait() {
        let mut review = setup();
        let id = review.propose(1, "hi").1["proposal_id"].as_str().expect("id").to_owned();
        let (status, declined) = call(&mut review.world, &device("ana", 1), "POST", &format!("proposals/{id}/decline"), json!({}));
        assert_eq!((status, declined["state"].as_str()), (200, Some("declined")));
        let june = review.june.clone();
        assert_eq!(call(&mut review.world, &june, "GET", &format!("proposals/{id}"), Value::Null).1["state"], "declined");
        assert_eq!(call(&mut review.world, &june, "POST", &format!("proposals/{id}/decline"), json!({})).0, 403, "only the person");
        assert_eq!(review.propose(1, "hi again").1["error"], "conflict");

        let id = review.propose(2, "hi").1["proposal_id"].as_str().expect("id").to_owned();
        let waiting = review.waiting("ana", 1);
        let approval = review.approval(&waiting[0]);
        let (status, withdrawn) = call(&mut review.world, &june, "DELETE", &format!("proposals/{id}"), Value::Null);
        assert_eq!((status, withdrawn["state"].as_str()), (200, Some("withdrawn")));
        assert_eq!(review.approve(&id, approval, 1).1["message"], "The agent took this message back.");
        assert_eq!(call(&mut review.world, &device("ana", 1), "DELETE", &format!("proposals/{id}"), Value::Null).0, 403);

        for n in 3..3 + WAITING_MOST as u32 {
            assert_eq!(review.propose(n, "one of many").0, 200);
        }
        let (status, refused) = review.propose(99, "one too many");
        assert_eq!((status, refused["error"].as_str()), (429, Some("too_many_waiting")));
        // Names that hide what they say are refused.
        let body = json!({"client_message_id": client_id(100), "contact_generation": review.generation,
                          "envelope": sealed(&review.channel, 0, "x"), "agent_name": "June\u{202E}"});
        let route = format!("conversations/{}/proposals", review.conversation);
        assert_eq!(call(&mut review.world, &june, "POST", &route, body).0, 400);
        // A device proposes nothing.
        let body = json!({"client_message_id": client_id(101), "contact_generation": review.generation,
                          "envelope": sealed(&review.channel, 0, "x")});
        assert_eq!(call(&mut review.world, &device("ana", 1), "POST", &route, body.clone()).0, 403);
        // Format 1, or another group than the Mac's channel, or a channel
        // epoch too old.
        let text = json!({"client_message_id": client_id(102), "contact_generation": review.generation, "text": "x"});
        assert_eq!(call(&mut review.world, &june, "POST", &route, text).0, 400);
        let elsewhere = json!({"client_message_id": client_id(102), "contact_generation": review.generation,
                               "envelope": sealed(&[9; 16], 0, "x")});
        assert_eq!(call(&mut review.world, &june, "POST", &route, elsewhere).0, 400);
        let channel = review.channel.clone();
        review.world.db.execute("UPDATE dm_groups SET epoch = 6 WHERE group_id = ?1", [&channel]).expect("moved on");
        let (status, behind) = call(&mut review.world, &june, "POST", &route, body);
        assert_eq!((status, &behind["error"], &behind["epoch"]), (409, &json!("epoch"), &json!(6)));
    }

    #[test]
    fn an_agent_without_send_proposes_nothing() {
        let mut review = setup();
        let backend = Caller::Backend { account_id: "ana".into(), claktory_id: "mac".into(), epoch: 1, agent: None };
        let put = grants::Put {
            app: "dms".into(),
            agent: june(),
            scopes: vec!["dm.read".into()],
            resources: vec![Resource { kind: "conversation".into(), id: review.conversation.clone() }],
            expires_ms: 0,
        };
        grants::put(&mut review.world.db, &backend, put, review.world.now).expect("granted");
        assert_eq!(review.propose(1, "hi").0, 403);
        assert_eq!(call(&mut review.world, &backend, "GET", "proposals", Value::Null).0, 403);
    }
}
