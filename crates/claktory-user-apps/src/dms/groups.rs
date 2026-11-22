//! Groups, end to end (E2; README "End to end"): the worker as MLS's
//! delivery service. It keeps each group's public state (OpenMLS
//! `PublicGroup`, never a secret), one mailbox per group in the order every
//! member processes it, the GroupInfo of its current epoch for rejoins, and
//! the Welcomes waiting for their leaves.
//!
//! It checks what it can from the framing and the public state, and never
//! reads an application message: a commit must apply to the current epoch
//! and come from the caller's own leaf; what it adds must be KeyPackages the
//! worker handed out, of leaves that belong in that group; its Welcome must
//! go to exactly those; its GroupInfo must be the new epoch's.

use std::collections::BTreeSet;

use claktory_dm_crypto::certificate::{LeafCertificate, LeafKind, claktory_leaf_id};
use claktory_dm_crypto::envelope::{Envelope, Kind as MlsKind, Parsed};
use claktory_dm_crypto::provider::Provider;
use claktory_dm_crypto::storage::{self as mls_storage, SealKey};
use openmls::messages::group_info::VerifiableGroupInfo;
use openmls::prelude::{GroupId, MlsMessageBodyIn, OpenMlsProvider, ProcessedMessageContent, Proposal, ProposalStore, PublicGroup, Sender};
use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};
use serde::Deserialize;
use serde_json::{Value, json};

use super::identity::{Refusal, body, invalid, take};
use super::keys::{Keys, Refused};
use super::leaves::{Leaf, b64, crypto, keys_of, leaf_of, unb64};
use super::sync::{self, Kind};
use crate::grants;
use crate::ipc::Caller;

pub const SCHEMA: &str = "
-- Each group: its kind and subject (one current group per subject), its
-- owner (a self group's or channel's account; a conversation's creator),
-- its epoch, its mailbox's last seq and how far it was pruned, and the
-- GroupInfo of its current epoch (an MLS message), for rejoins.
CREATE TABLE IF NOT EXISTS dm_groups (
    group_id BLOB PRIMARY KEY,
    kind TEXT NOT NULL,
    subject TEXT NOT NULL UNIQUE,
    account_id TEXT NOT NULL,
    conversation_id TEXT,
    contact_generation INTEGER,
    claktory_id TEXT,
    epoch INTEGER NOT NULL,
    last_seq INTEGER NOT NULL,
    pruned_seq INTEGER NOT NULL DEFAULT 0,
    group_info BLOB NOT NULL,
    created INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS dm_groups_account ON dm_groups (account_id);
CREATE INDEX IF NOT EXISTS dm_groups_conversation ON dm_groups (conversation_id);
-- Each group's mailbox: application messages, commits and proposals, as
-- sent (MLS messages), numbered in the order the worker took them.
CREATE TABLE IF NOT EXISTS dm_mailbox (
    group_id BLOB NOT NULL,
    seq INTEGER NOT NULL,
    kind TEXT NOT NULL,
    epoch INTEGER NOT NULL,
    envelope BLOB NOT NULL,
    digest BLOB NOT NULL,
    account_id TEXT NOT NULL,
    leaf_id TEXT NOT NULL,
    client_message_id TEXT,
    by_agent TEXT,
    accepted INTEGER NOT NULL,
    PRIMARY KEY (group_id, seq)
) WITHOUT ROWID;
CREATE UNIQUE INDEX IF NOT EXISTS dm_mailbox_client ON dm_mailbox (group_id, leaf_id, client_message_id)
    WHERE client_message_id IS NOT NULL;
CREATE INDEX IF NOT EXISTS dm_mailbox_accepted ON dm_mailbox (accepted);
-- Welcomes waiting for their leaves: one row per leaf a commit added.
CREATE TABLE IF NOT EXISTS dm_welcomes (
    id TEXT PRIMARY KEY,
    group_id BLOB NOT NULL,
    account_id TEXT NOT NULL,
    leaf_id TEXT NOT NULL,
    welcome BLOB NOT NULL,
    -- The seq of the commit that made it: the leaf reads on after it.
    seq INTEGER NOT NULL,
    accepted INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS dm_welcomes_leaf ON dm_welcomes (account_id, leaf_id);
CREATE INDEX IF NOT EXISTS dm_welcomes_accepted ON dm_welcomes (accepted);
";

/// How long a mailbox keeps what it took; a self group, whose traffic is
/// read positions and drafts, a week.
const MAILBOX_KEEP_MS: i64 = 30 * 86_400_000;
const SELF_KEEP_MS: i64 = 7 * 86_400_000;
/// How long a Welcome waits for its leaf.
const WELCOME_KEEP_MS: i64 = 30 * 86_400_000;
/// How many epochs behind an application message may be.
pub const EPOCHS_BEHIND: u64 = 5;
/// Commits an hour, per leaf and per account; proposals an hour per leaf;
/// application messages a minute per leaf (self and channel groups).
const COMMITS_PER_LEAF: i64 = 120;
const COMMITS_PER_ACCOUNT: i64 = 600;
/// A budget of their own for the commits a new security key or a removed
/// device takes (dms-e2ee §2.1): each leaf's move to the new key, and the
/// Removes of the account's own leaves its key no longer vouches for, one
/// per group each, so a person with many chats isn't held up.
const ROTATION_COMMITS_PER_LEAF: i64 = 1000;
const ROTATION_COMMITS_PER_ACCOUNT: i64 = 5000;
const PROPOSALS_PER_LEAF: i64 = 120;
const MESSAGES_PER_LEAF: i64 = 60;
/// A page of a mailbox: messages by default and at most, and bytes.
const PAGE: usize = 50;
const PAGE_MOST: usize = 100;
const PAGE_BYTES: usize = 256 * 1024;
/// The Claktory's event feed: rows looked at, and events answered, a call.
const SCAN: i64 = 500;
const EVENTS: usize = 100;

/// A group's kind, as the routes name it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GroupKind {
    Conversation,
    SelfGroup,
    Channel,
}

impl GroupKind {
    pub fn name(self) -> &'static str {
        match self {
            GroupKind::Conversation => "conversation",
            GroupKind::SelfGroup => "self",
            GroupKind::Channel => "channel",
        }
    }

    fn parse(name: &str) -> Option<GroupKind> {
        match name {
            "conversation" => Some(GroupKind::Conversation),
            "self" => Some(GroupKind::SelfGroup),
            "channel" => Some(GroupKind::Channel),
            _ => None,
        }
    }
}

/// A group as the worker keeps it.
pub struct Group {
    pub id: Vec<u8>,
    pub kind: GroupKind,
    pub account: String,
    pub conversation: Option<String>,
    pub generation: Option<i64>,
    pub claktory: Option<String>,
    pub epoch: u64,
    pub last_seq: i64,
    pub pruned_seq: i64,
}

const COLUMNS: &str = "group_id, kind, account_id, conversation_id, contact_generation, claktory_id, epoch, last_seq, pruned_seq";

fn group_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Group> {
    let kind: String = row.get(1)?;
    Ok(Group {
        id: row.get(0)?,
        kind: GroupKind::parse(&kind).unwrap_or(GroupKind::SelfGroup),
        account: row.get(2)?,
        conversation: row.get(3)?,
        generation: row.get(4)?,
        claktory: row.get(5)?,
        epoch: row.get::<_, i64>(6)? as u64,
        last_seq: row.get(7)?,
        pruned_seq: row.get(8)?,
    })
}

pub fn load(db: &Connection, id: &[u8]) -> rusqlite::Result<Option<Group>> {
    db.query_row(&format!("SELECT {COLUMNS} FROM dm_groups WHERE group_id = ?1"), [id], group_row).optional()
}

/// A conversation's group at its generation, if one was made.
pub fn of_conversation(db: &Connection, conversation: &str, generation: i64) -> rusqlite::Result<Option<Group>> {
    db.query_row(
        &format!("SELECT {COLUMNS} FROM dm_groups WHERE subject = ?1"),
        [subject_of_conversation(conversation, generation)],
        group_row,
    )
    .optional()
}

/// An account's channel with one of its Claktories, if one was made.
pub fn channel_of(db: &Connection, account: &str, claktory: &str) -> rusqlite::Result<Option<Group>> {
    db.query_row(&format!("SELECT {COLUMNS} FROM dm_groups WHERE subject = ?1"), [format!("channel:{account}:{claktory}")], group_row)
        .optional()
}

/// Whether an application message goes in `group` now: its epoch, or
/// `400` for another group's and `409 epoch` (no more said: the caller
/// answers it with [`epoch_answer`]) for one too old.
pub fn fits(group: &Group, submitted: &super::bodies::Submitted) -> Result<u64, Refusal> {
    let (id, epoch) = submitted.group();
    if id != group.id {
        return Err(invalid("That message is for another group."));
    }
    if !epoch_fits(group, epoch) {
        return Err(Refusal(409, "epoch", "The conversation moved on meanwhile. Catch up, then try again.".to_owned()));
    }
    Ok(epoch)
}

fn subject_of_conversation(conversation: &str, generation: i64) -> String {
    format!("conversation:{conversation}:{generation}")
}

fn not_found() -> Refusal {
    Refusal(404, "not_found", "There's nothing here.".to_owned())
}

fn device_only() -> Refusal {
    Refusal(403, "device_only", "Only your own signed-in devices can do this.".to_owned())
}

fn not_set_up() -> Refusal {
    Refusal(409, "not_set_up", "Set up messages on one of your devices first.".to_owned())
}

fn broken(what: &str) -> Refusal {
    tracing::error!("dms: {what}");
    Refusal(503, "unavailable", "Something went wrong. Try again later.".to_owned())
}

/// `409 epoch`, with the group's current one: fetch, process, try again.
pub fn epoch_answer(epoch: u64) -> (u16, Value) {
    (409, json!({"error": "epoch", "message": "The conversation moved on meanwhile. Catch up, then try again.", "epoch": epoch}))
}

/// A group id from a path: 16 to 32 bytes, base64url.
fn group_id_of(text: &str) -> Result<Vec<u8>, Refusal> {
    let id = unb64(text, "A group id").map_err(|_| not_found())?;
    if !(16..=32).contains(&id.len()) {
        return Err(not_found());
    }
    Ok(id)
}

/// An envelope from a body, of `kind`: or why not.
fn envelope_of(value: &Value, kind: MlsKind, what: &str) -> Result<(Envelope, Parsed), Refusal> {
    let envelope = Envelope::from_json(value).map_err(|error| invalid(&format!("{what} isn't an envelope: {error}.")))?;
    let parsed = envelope.parsed();
    if parsed.kind != kind {
        return Err(invalid(&format!("{what} isn't what this takes.")));
    }
    Ok((envelope, parsed))
}

/// The key the group's public state is sealed with at rest, a data key of
/// its own: forgetting it forgets the group.
fn seal_key(keys: &Keys, db: &Connection, id: &[u8], now_ms: i64) -> Result<SealKey, Refusal> {
    Ok(SealKey::from_bytes(*keys.hash_key(db, &purpose(id), now_ms)?))
}

fn purpose(id: &[u8]) -> String {
    format!("group:{}", b64(id))
}

/// The certificate a leaf carries, when it's one and for the leaf's own
/// signature key.
fn certified(credential: &openmls::prelude::Credential, signature_key: &[u8]) -> Option<LeafCertificate> {
    let certificate = LeafCertificate::from_credential(credential).ok()?;
    (certificate.leaf.signature_key.as_slice() == signature_key).then_some(certificate)
}

fn is_leaf(certificate: &LeafCertificate, leaf: &Leaf) -> bool {
    certificate.leaf.account_id == leaf.account && certificate.leaf.leaf_id == leaf.leaf_id && certificate.leaf.kind == leaf.kind
}

/// Whether the account's current identity certified `certificate` and its
/// current list names it.
fn current(db: &Connection, certificate: &LeafCertificate) -> Result<bool, Refusal> {
    let Some(keys) = keys_of(db, &certificate.leaf.account_id)? else { return Ok(false) };
    Ok(certificate.identity == keys.identity && keys.list.names(certificate))
}

/// Whether the account's current identity made `certificate`, listed or
/// not. A leaf under any other (one its account replaced) is inert: it
/// moves itself to the new key, and does nothing else (dms-e2ee §2.1).
fn under_current(db: &Connection, certificate: &LeafCertificate) -> Result<bool, Refusal> {
    Ok(keys_of(db, &certificate.leaf.account_id)?.is_some_and(|keys| keys.identity == certificate.identity))
}

fn replaced() -> Refusal {
    Refusal(
        403,
        "replaced_key",
        "This leaf's certificate is from a security key its account replaced: it only moves itself to the new one.".to_owned(),
    )
}

/// The people whose devices a group reaches: a conversation's two, or the
/// owner.
pub fn people(db: &Connection, group: &Group) -> rusqlite::Result<Vec<String>> {
    match (&group.kind, &group.conversation) {
        (GroupKind::Conversation, Some(conversation)) => {
            let pair: Option<(String, String)> = db
                .query_row("SELECT low, high FROM dm_contacts WHERE conversation_id = ?1", [conversation], |row| {
                    Ok((row.get(0)?, row.get(1)?))
                })
                .optional()?;
            Ok(pair.map(|(low, high)| vec![low, high]).unwrap_or_default())
        }
        _ => Ok(vec![group.account.clone()]),
    }
}

/// The earliest a Claktory's agents hold a live DM grant on `conversation`
/// from, under its registration now, if any does.
fn granted_from(db: &Connection, caller: &Caller, conversation: &str, now_ms: i64) -> rusqlite::Result<Option<i64>> {
    let Caller::Backend { account_id, claktory_id, epoch, agent: None } = caller else { return Ok(None) };
    Ok(grants::of_claktory(db, account_id, claktory_id, *epoch, "dms", now_ms)?
        .into_iter()
        .filter(|grant| grant.resources.iter().any(|resource| resource.kind == "conversation" && resource.id == conversation))
        .map(|grant| grant.granted_ms)
        .min())
}

/// Whether `caller` fetches `group`, and from when (Unix ms): a Claktory
/// reads a conversation only from its agents' grant on.
pub fn reach(db: &Connection, caller: &Caller, group: &Group, now_ms: i64) -> Result<Option<i64>, Refusal> {
    match (caller, group.kind) {
        (Caller::Device { account_id, .. }, GroupKind::Conversation) => Ok(people(db, group)?.contains(account_id).then_some(0)),
        (Caller::Device { account_id, .. }, _) => Ok((*account_id == group.account).then_some(0)),
        (Caller::Backend { agent: None, .. }, GroupKind::Conversation) => {
            let Some(conversation) = &group.conversation else { return Ok(None) };
            if !people(db, group)?.contains(&caller.account_id().to_owned()) {
                return Ok(None);
            }
            Ok(granted_from(db, caller, conversation, now_ms)?)
        }
        (Caller::Backend { account_id, claktory_id, agent: None, .. }, GroupKind::Channel) => {
            Ok((*account_id == group.account && group.claktory.as_ref() == Some(claktory_id)).then_some(0))
        }
        _ => Ok(None),
    }
}

/// Whether `caller` may rejoin `group` by an external commit: a device a
/// group of its own account's or a conversation it's in, a Claktory only
/// its own channel.
fn may_rejoin(db: &Connection, caller: &Caller, group: &Group, now_ms: i64) -> Result<bool, Refusal> {
    Ok(match (caller, group.kind) {
        (Caller::Device { .. }, _) => reach(db, caller, group, now_ms)?.is_some(),
        (Caller::Backend { agent: None, .. }, GroupKind::Channel) => reach(db, caller, group, now_ms)?.is_some(),
        _ => false,
    })
}

/// The group a path names, when the caller reaches it (or may rejoin it):
/// and from when.
fn reached(db: &Connection, caller: &Caller, id: &str, now_ms: i64) -> Result<(Group, i64), Refusal> {
    let group = load(db, &group_id_of(id)?)?.ok_or_else(not_found)?;
    let from = reach(db, caller, &group, now_ms)?.ok_or_else(not_found)?;
    Ok((group, from))
}

/// What a group shows its members.
fn shown(db: &Connection, caller: &Caller, group: &Group, now_ms: i64) -> Result<Value, Refusal> {
    let mut shown = json!({"group_id": b64(&group.id), "kind": group.kind.name(), "epoch": group.epoch, "last_seq": group.last_seq});
    describe(db, group, &mut shown)?;
    if let (Caller::Backend { account_id, claktory_id, epoch, .. }, Some(conversation)) = (caller, &group.conversation) {
        let grants: Vec<Value> = grants::of_claktory(db, account_id, claktory_id, *epoch, "dms", now_ms)?
            .into_iter()
            .filter(|grant| grant.resources.iter().any(|resource| resource.kind == "conversation" && resource.id == *conversation))
            .map(|grant| json!({"agent": {"id": grant.agent.id, "incarnation": grant.agent.incarnation}, "from_ms": grant.granted_ms}))
            .collect();
        shown["grants"] = json!(grants);
    }
    Ok(shown)
}

/// A group's subject, into `shown`.
fn describe(db: &Connection, group: &Group, shown: &mut Value) -> rusqlite::Result<()> {
    if let (Some(conversation), Some(generation)) = (&group.conversation, group.generation) {
        shown["conversation_id"] = json!(conversation);
        shown["contact_generation"] = json!(generation);
        shown["accounts"] = json!(people(db, group)?);
    }
    if let Some(claktory) = &group.claktory {
        shown["claktory_id"] = json!(claktory);
    }
    Ok(())
}

/// Adds to every account the group reaches that something happened.
fn tell(
    transaction: &Transaction<'_>,
    group: &Group,
    kind: impl Fn() -> Kind,
    object: &str,
    seq: i64,
    now_ms: i64,
) -> rusqlite::Result<()> {
    for account in people(transaction, group)? {
        sync::emit(transaction, &account, kind(), "", object, seq, now_ms)?;
    }
    Ok(())
}

/// One message into a group's mailbox: its seq.
pub struct Entry<'a> {
    pub kind: &'static str,
    pub epoch: u64,
    pub envelope: &'a Envelope,
    pub leaf: &'a Leaf,
    pub client_message_id: Option<&'a str>,
    pub by_agent: Option<&'a str>,
}

/// Appends to `group`'s mailbox and tells its accounts: the new seq.
pub fn append(transaction: &Transaction<'_>, group: &mut Group, entry: Entry<'_>, now_ms: i64) -> rusqlite::Result<i64> {
    let seq = group.last_seq + 1;
    transaction.execute(
        "INSERT INTO dm_mailbox (group_id, seq, kind, epoch, envelope, digest, account_id, leaf_id, client_message_id, by_agent, accepted)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
        params![
            group.id,
            seq,
            entry.kind,
            entry.epoch as i64,
            entry.envelope.bytes(),
            entry.envelope.digest().to_vec(),
            entry.leaf.account,
            entry.leaf.leaf_id,
            entry.client_message_id,
            entry.by_agent,
            now_ms
        ],
    )?;
    transaction.execute("UPDATE dm_groups SET last_seq = ?2 WHERE group_id = ?1", params![group.id, seq])?;
    group.last_seq = seq;
    tell(transaction, group, || Kind::GroupMessage, &b64(&group.id), seq, now_ms)?;
    Ok(seq)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Create {
    kind: String,
    conversation_id: Option<String>,
    contact_generation: Option<i64>,
    claktory_id: Option<String>,
    group_info: Value,
    #[serde(default)]
    #[allow(dead_code)]
    request_nonce: Option<String>,
}

/// `POST groups`: a device starts a group of a subject of its own account's,
/// with only its own leaf in it.
fn create(transaction: &Transaction<'_>, keys: &Keys, caller: &Caller, create: Create, now_ms: i64) -> Result<Value, Refusal> {
    let Caller::Device { account_id, registrations, .. } = caller else { return Err(device_only()) };
    let leaf = leaf_of(caller)?;
    let kind = GroupKind::parse(&create.kind).ok_or_else(|| invalid("kind is conversation, self or channel."))?;
    let (subject, conversation, generation, claktory) =
        match (kind, &create.conversation_id, create.contact_generation, &create.claktory_id) {
            (GroupKind::Conversation, Some(conversation), Some(generation), None) => {
                let active: bool = transaction.query_row(
                    "SELECT COUNT(*) > 0 FROM dm_contacts WHERE conversation_id = ?1 AND (low = ?2 OR high = ?2) AND generation = ?3
                 AND ended IS NULL",
                    params![conversation, account_id, generation],
                    |row| row.get(0),
                )?;
                if !active {
                    return Err(not_found());
                }
                (subject_of_conversation(conversation, generation), Some(conversation.clone()), Some(generation), None)
            }
            (GroupKind::SelfGroup, None, None, None) => (format!("self:{account_id}"), None, None, None),
            (GroupKind::Channel, None, None, Some(claktory)) => {
                if !registrations.iter().any(|registration| registration.claktory_id == *claktory) {
                    return Err(not_found());
                }
                (format!("channel:{account_id}:{claktory}"), None, None, Some(claktory.clone()))
            }
            _ => {
                return Err(invalid(
                    "A conversation's group names conversation_id and contact_generation, a channel claktory_id, a self group neither.",
                ));
            }
        };
    let (envelope, parsed) = envelope_of(&create.group_info, MlsKind::GroupInfo, "group_info")?;
    let MlsMessageBodyIn::GroupInfo(info) = parsed.message.extract() else { return Err(invalid("group_info isn't a GroupInfo.")) };
    let id = info.group_id().as_slice().to_vec();
    if !(16..=32).contains(&id.len()) {
        return Err(invalid("A group's id is 16 to 32 random bytes."));
    }
    if info.epoch().as_u64() != 0 || info.ciphersuite() != claktory_dm_crypto::CIPHERSUITE {
        return Err(invalid("A new group's GroupInfo is at epoch 0, for MLS_128_DHKEMX25519_AES128GCM_SHA256_Ed25519."));
    }
    let taken: bool =
        transaction
            .query_row("SELECT COUNT(*) > 0 FROM dm_groups WHERE subject = ?1 OR group_id = ?2", params![subject, id], |row| row.get(0))?;
    if taken {
        return Err(Refusal(409, "exists", "There's a group for this already.".to_owned()));
    }
    let key = seal_key(keys, transaction, &id, now_ms)?;
    let provider = Provider::public(crypto(), transaction, &key).map_err(|error| broken(&error.to_string()))?;
    if PublicGroup::load(provider.storage(), &GroupId::from_slice(&id)).map_err(|error| broken(&error.to_string()))?.is_some() {
        return Err(Refusal(409, "exists", "There's a group for this already.".to_owned()));
    }
    let public = from_group_info(&provider, info)?;
    let members: Vec<_> = public.members().collect();
    let only = match members.as_slice() {
        [only] => certified(&only.credential, &only.signature_key),
        _ => None,
    };
    let Some(certificate) = only.filter(|certificate| is_leaf(certificate, &leaf)) else {
        return Err(invalid("A new group's only member is the device making it."));
    };
    if keys_of(transaction, account_id)?.is_none() {
        return Err(not_set_up());
    }
    if !current(transaction, &certificate)? {
        return Err(invalid("That leaf isn't on the account's device list."));
    }
    transaction.execute(
        "INSERT INTO dm_groups (group_id, kind, subject, account_id, conversation_id, contact_generation, claktory_id, epoch, last_seq,
         group_info, created) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 0, 0, ?8, ?9)",
        params![id, kind.name(), subject, account_id, conversation, generation, claktory, envelope.bytes(), now_ms],
    )?;
    let group = load(transaction, &id)?.ok_or_else(|| broken("a group just made isn't there"))?;
    tell(transaction, &group, || Kind::Groups, "", 0, now_ms)?;
    Ok(json!({"group_id": b64(&id), "epoch": 0, "last_seq": 0}))
}

/// A `PublicGroup` from a GroupInfo with its ratchet tree, stored in
/// `provider`: OpenMLS checks the GroupInfo's signature by its signer's
/// leaf and that the tree is the one its context names.
fn from_group_info(provider: &Provider<'_>, info: VerifiableGroupInfo) -> Result<PublicGroup, Refusal> {
    let tree =
        info.extensions().ratchet_tree().ok_or_else(|| invalid("A GroupInfo here carries its ratchet tree."))?.ratchet_tree().clone();
    let (public, _) = PublicGroup::from_external(provider.crypto(), provider.storage(), tree, info, ProposalStore::default())
        .map_err(|error| invalid(&format!("That GroupInfo doesn't check: {error}.")))?;
    Ok(public)
}

/// Whether `info` is exactly `public`'s state: built apart (nothing of it
/// kept), the same context and confirmation tag.
fn is_state_of(info: VerifiableGroupInfo, public: &PublicGroup) -> Result<bool, Refusal> {
    let scratch = Connection::open_in_memory()?;
    scratch.pragma_update(None, "secure_delete", "ON")?;
    scratch.execute_batch(mls_storage::SCHEMA)?;
    // Public state only, thrown away with the connection.
    let key = SealKey::from_bytes([0; 32]);
    let provider = Provider::public(crypto(), &scratch, &key).map_err(|error| broken(&error.to_string()))?;
    let theirs = from_group_info(&provider, info)?;
    Ok(theirs.group_context() == public.group_context() && theirs.confirmation_tag() == public.confirmation_tag())
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Commit {
    commit: Value,
    welcome: Option<Value>,
    group_info: Value,
    #[serde(default)]
    #[allow(dead_code)]
    request_nonce: Option<String>,
}

/// Where a leaf may be added: a conversation's people's devices and the
/// Claktories its agents are granted on; a self group's owner's devices; a
/// channel's owner's devices and its Claktory.
fn may_add(db: &Connection, group: &Group, people: &[String], certificate: &LeafCertificate, now_ms: i64) -> Result<bool, Refusal> {
    let leaf = &certificate.leaf;
    let claktory = || {
        let (id, epoch) = leaf.leaf_id.rsplit_once('#')?;
        Some((id.to_owned(), epoch.parse::<u64>().ok()?))
    };
    Ok(match group.kind {
        GroupKind::Conversation => {
            let Some(conversation) = &group.conversation else { return Ok(false) };
            let active: bool =
                db.query_row("SELECT COUNT(*) > 0 FROM dm_contacts WHERE conversation_id = ?1 AND ended IS NULL", [conversation], |row| {
                    row.get(0)
                })?;
            if !active || !people.contains(&leaf.account_id) {
                return Ok(false);
            }
            match (leaf.kind, claktory()) {
                (LeafKind::Device, _) => true,
                (LeafKind::Claktory, Some((id, epoch))) => {
                    let backend = Caller::Backend { account_id: leaf.account_id.clone(), claktory_id: id, epoch, agent: None };
                    granted_from(db, &backend, conversation, now_ms)?.is_some()
                }
                _ => false,
            }
        }
        GroupKind::SelfGroup => leaf.account_id == group.account && leaf.kind == LeafKind::Device,
        GroupKind::Channel => {
            leaf.account_id == group.account
                && match leaf.kind {
                    LeafKind::Device => true,
                    LeafKind::Claktory => claktory()
                        .is_some_and(|(id, epoch)| Some(&id) == group.claktory.as_ref() && claktory_leaf_id(&id, epoch) == leaf.leaf_id),
                }
        }
    })
}

/// `POST groups/<id>/commits`: the next epoch, from a member's own leaf,
/// or a rejoin.
fn commit(
    transaction: &Transaction<'_>,
    keys: &Keys,
    caller: &Caller,
    id: &str,
    commit: Commit,
    now_ms: i64,
) -> Result<(u16, Value), Refusal> {
    let leaf = leaf_of(caller)?;
    let mut group = load(transaction, &group_id_of(id)?)?.ok_or_else(not_found)?;
    let reaches = reach(transaction, caller, &group, now_ms)?.is_some();
    if !reaches && !may_rejoin(transaction, caller, &group, now_ms)? {
        return Err(not_found());
    }
    let (envelope, parsed) = envelope_of(&commit.commit, MlsKind::Commit, "commit")?;
    let (group_id, epoch) = parsed.group.clone().ok_or_else(|| invalid("commit isn't a commit."))?;
    if group_id.as_slice() != group.id.as_slice() {
        return Err(invalid("That commit is another group's."));
    }
    if epoch != group.epoch {
        return Ok(epoch_answer(group.epoch));
    }
    let message = parsed.into_protocol_message().map_err(|error| invalid(&format!("That commit isn't one: {error}.")))?;
    let key = seal_key(keys, transaction, &group.id, now_ms)?;
    let provider = Provider::public(crypto(), transaction, &key).map_err(|error| broken(&error.to_string()))?;
    let mut public = PublicGroup::load(provider.storage(), &group_id)
        .map_err(|error| broken(&error.to_string()))?
        .ok_or_else(|| broken("a group's public state is missing"))?;
    let processed =
        public.process_message(provider.crypto(), message).map_err(|error| invalid(&format!("That commit doesn't apply: {error}.")))?;
    let sender = processed.sender().clone();
    let ProcessedMessageContent::StagedCommitMessage(staged) = processed.into_content() else {
        return Err(invalid("commit isn't a commit."));
    };

    // Who: the caller's own leaf, as a member or rejoining. A member's new
    // leaf, when it has one, is its own under the account's current key and
    // list; under a key its account replaced, a member only moves itself to
    // the new one, keeping its signature key, with nothing else in the
    // commit (R1's rule, with Fugo's three tightenings).
    let mut rotation = false;
    let committer = match sender {
        Sender::Member(index) => Some(index),
        _ => None,
    };
    let external = match sender {
        Sender::Member(index) => {
            let mine = public.leaf(index).and_then(|node| certified(node.credential(), node.signature_key().as_slice()));
            let Some(old) = mine.filter(|certificate| is_leaf(certificate, &leaf)) else {
                return Err(invalid("A leaf commits only as itself."));
            };
            let path = staged.update_path_leaf_node().map(|node| certified(node.credential(), node.signature_key().as_slice()));
            if let Some(new) = &path {
                let fine = match new {
                    Some(new) => is_leaf(new, &leaf) && current(transaction, new)?,
                    None => false,
                };
                if !fine {
                    return Err(Refusal(
                        403,
                        "not_current",
                        "A leaf's new certificate is its own, from its account's current security key and on its device list.".to_owned(),
                    ));
                }
            }
            if !under_current(transaction, &old)? {
                let moved = staged.queued_proposals().next().is_none()
                    && path.flatten().is_some_and(|new| new.leaf.signature_key == old.leaf.signature_key);
                if !moved {
                    return Err(replaced());
                }
                rotation = true;
            }
            false
        }
        Sender::NewMemberCommit => {
            let joining = staged.update_path_leaf_node().and_then(|node| certified(node.credential(), node.signature_key().as_slice()));
            let Some(certificate) = joining.filter(|certificate| is_leaf(certificate, &leaf)) else {
                return Err(invalid("A leaf rejoins only as itself."));
            };
            if !may_rejoin(transaction, caller, &group, now_ms)? {
                return Err(not_found());
            }
            if !current(transaction, &certificate)? {
                return Err(invalid("That leaf isn't on the account's device list."));
            }
            true
        }
        _ => return Err(invalid("Commits come from members, or from a leaf rejoining.")),
    };

    // What: adds, removes and updates; a Claktory only its own Update; a
    // rejoin only removes its own old leaf.
    let people = people(transaction, &group)?;
    let mut added = Vec::new();
    // Only Removes of the account's own leaves its current key and list no
    // longer vouch for: a removed device's, or one a new key left behind.
    let mut cleanup = staged.queued_proposals().next().is_some();
    for queued in staged.queued_proposals() {
        if matches!(leaf.kind, LeafKind::Claktory) && !external {
            return Err(Refusal(403, "claktory_update_only", "A Claktory commits only its own update.".to_owned()));
        }
        // A proposal another member made, taken in by reference: never
        // one from a leaf under a replaced key.
        if let Sender::Member(by) = queued.sender()
            && Some(*by) != committer
        {
            let proposer = public.leaf(*by).and_then(|node| certified(node.credential(), node.signature_key().as_slice()));
            match proposer {
                Some(proposer) if under_current(transaction, &proposer)? => {}
                _ => return Err(replaced()),
            }
        }
        let leaving = match queued.proposal() {
            Proposal::Remove(remove) if !external => {
                public.leaf(remove.removed()).and_then(|node| certified(node.credential(), node.signature_key().as_slice()))
            }
            _ => None,
        };
        cleanup = cleanup
            && match leaving {
                Some(gone) => gone.leaf.account_id == leaf.account && !current(transaction, &gone)?,
                None => false,
            };
        match queued.proposal() {
            Proposal::Add(add) => {
                if external {
                    return Err(invalid("A rejoin adds no one."));
                }
                added.push(add.key_package().clone());
            }
            Proposal::Remove(remove) => {
                if external {
                    let old = public.leaf(remove.removed()).and_then(|node| certified(node.credential(), node.signature_key().as_slice()));
                    let own = old.is_some_and(|old| {
                        old.leaf.account_id == leaf.account
                            && (old.leaf.leaf_id == leaf.leaf_id
                                || (old.leaf.kind == LeafKind::Claktory
                                    && leaf.kind == LeafKind::Claktory
                                    && old.leaf.leaf_id.rsplit_once('#').map(|(id, _)| id)
                                        == leaf.leaf_id.rsplit_once('#').map(|(id, _)| id)))
                    });
                    if !own {
                        return Err(invalid("A rejoin removes only the leaf's own old self."));
                    }
                } else {
                    // A member removes only its own account's leaves, but
                    // takes in any leaf's own request to leave.
                    let asked = matches!(queued.sender(), Sender::Member(by) if *by == remove.removed());
                    let removed =
                        public.leaf(remove.removed()).and_then(|node| certified(node.credential(), node.signature_key().as_slice()));
                    if !asked && !removed.is_some_and(|removed| removed.leaf.account_id == leaf.account) {
                        return Err(Refusal(403, "not_allowed", "A device removes only its own account's leaves.".to_owned()));
                    }
                }
            }
            Proposal::Update(_) | Proposal::SelfRemove => {}
            Proposal::ExternalInit(_) if external => {}
            _ => return Err(invalid("Commits here add, remove and update, nothing else.")),
        }
    }

    // A new key's moves and the Removes it leaves behind have their own
    // budget; everything else the usual one.
    let (budget, per_leaf, per_account) = if rotation || cleanup {
        ("rotation-commit", ROTATION_COMMITS_PER_LEAF, ROTATION_COMMITS_PER_ACCOUNT)
    } else {
        ("commit", COMMITS_PER_LEAF, COMMITS_PER_ACCOUNT)
    };
    take(transaction, &format!("{budget}:{}", leaf.leaf_id), 3600, per_leaf, now_ms)?;
    take(transaction, &format!("{budget}s:{}", leaf.account), 3600, per_account, now_ms)?;

    // Whom it adds: KeyPackages the worker handed out, of leaves that
    // belong here, and the Welcome to exactly them.
    let mut welcomed = Vec::new();
    for package in &added {
        let reference = package.hash_ref(crypto().backend()).map_err(|_| invalid("An added KeyPackage has no reference."))?;
        let reference = reference.as_slice().to_vec();
        let claimed: Option<(String, String)> = transaction
            .query_row("SELECT account_id, leaf_id FROM dm_claims WHERE ref = ?1", [&reference], |row| Ok((row.get(0)?, row.get(1)?)))
            .optional()?;
        let node = package.leaf_node();
        let certificate = certified(node.credential(), node.signature_key().as_slice())
            .ok_or_else(|| invalid("An added KeyPackage's credential isn't its leaf's certificate."))?;
        let Some((account, leaf_id)) =
            claimed.filter(|(account, leaf_id)| *account == certificate.leaf.account_id && *leaf_id == certificate.leaf.leaf_id)
        else {
            return Err(invalid("A commit adds only KeyPackages claimed here."));
        };
        if !current(transaction, &certificate)? || !may_add(transaction, &group, &people, &certificate, now_ms)? {
            return Err(Refusal(403, "not_allowed", "That leaf can't be added to this group.".to_owned()));
        }
        welcomed.push((reference, account, leaf_id));
    }
    let welcome = match (&commit.welcome, welcomed.is_empty()) {
        (None, true) => None,
        (Some(_), true) => return Err(invalid("A commit that adds no one has no Welcome.")),
        (None, false) => return Err(invalid("A commit that adds someone comes with their Welcome.")),
        (Some(value), false) => {
            let (welcome, parsed) = envelope_of(value, MlsKind::Welcome, "welcome")?;
            let MlsMessageBodyIn::Welcome(inner) = parsed.message.extract() else { return Err(invalid("welcome isn't a Welcome.")) };
            let to: BTreeSet<Vec<u8>> = inner.secrets().iter().map(|secret| secret.new_member().as_slice().to_vec()).collect();
            let adds: BTreeSet<Vec<u8>> = welcomed.iter().map(|(reference, ..)| reference.clone()).collect();
            if to != adds || inner.secrets().len() != welcomed.len() {
                return Err(invalid("The Welcome goes to exactly the KeyPackages the commit adds."));
            }
            Some(welcome)
        }
    };

    // The new epoch, and its GroupInfo.
    public.merge_commit(provider.storage(), *staged).map_err(|error| broken(&format!("merging a commit: {error}")))?;
    let next = group.epoch + 1;
    let (info_envelope, parsed) = envelope_of(&commit.group_info, MlsKind::GroupInfo, "group_info")?;
    let MlsMessageBodyIn::GroupInfo(info) = parsed.message.extract() else { return Err(invalid("group_info isn't a GroupInfo.")) };
    if info.epoch().as_u64() != next || info.group_id().as_slice() != group.id.as_slice() || !is_state_of(info, &public)? {
        return Err(invalid("The GroupInfo is the new epoch's."));
    }
    let seq = append(
        transaction,
        &mut group,
        Entry { kind: "commit", epoch, envelope: &envelope, leaf: &leaf, client_message_id: None, by_agent: None },
        now_ms,
    )?;
    transaction.execute(
        "UPDATE dm_groups SET epoch = ?2, group_info = ?3 WHERE group_id = ?1",
        params![group.id, next as i64, info_envelope.bytes()],
    )?;
    if let Some(welcome) = welcome {
        for (reference, account, leaf_id) in welcomed {
            let id = hex(&rand_id());
            transaction.execute(
                "INSERT INTO dm_welcomes (id, group_id, account_id, leaf_id, welcome, seq, accepted) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![id, group.id, account, leaf_id, welcome.bytes(), seq, now_ms],
            )?;
            transaction.execute("DELETE FROM dm_claims WHERE ref = ?1", [reference])?;
            sync::emit(transaction, &account, Kind::Welcome, "", &id, 0, now_ms)?;
        }
    }
    Ok((200, json!({"seq": seq, "epoch": next})))
}

fn rand_id() -> [u8; 16] {
    let mut bytes = [0; 16];
    getrandom::fill(&mut bytes).expect("the system's random source");
    bytes
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Propose {
    proposal: Value,
    #[serde(default)]
    #[allow(dead_code)]
    request_nonce: Option<String>,
}

/// `POST groups/<id>/proposals`: a member asks to leave (its own Remove),
/// for the next commit to take in.
fn propose(
    transaction: &Transaction<'_>,
    keys: &Keys,
    caller: &Caller,
    id: &str,
    propose: Propose,
    now_ms: i64,
) -> Result<(u16, Value), Refusal> {
    let leaf = leaf_of(caller)?;
    let mut group = match reached(transaction, caller, id, now_ms) {
        Ok((group, _)) => group,
        // A Claktory whose grant on a conversation ended asks to leave it
        // (Fugo L6): no grant is needed for that, only its leaf still in
        // the tree, which processing the proposal below checks, and it
        // being its own Remove, the only proposal taken.
        Err(refused) => match (caller, load(transaction, &group_id_of(id)?)?) {
            (Caller::Backend { agent: None, .. }, Some(group))
                if group.kind == GroupKind::Conversation && people(transaction, &group)?.contains(&leaf.account) =>
            {
                group
            }
            _ => return Err(refused),
        },
    };
    let (envelope, parsed) = envelope_of(&propose.proposal, MlsKind::Proposal, "proposal")?;
    let (group_id, epoch) = parsed.group.clone().ok_or_else(|| invalid("proposal isn't a proposal."))?;
    if group_id.as_slice() != group.id.as_slice() {
        return Err(invalid("That proposal is another group's."));
    }
    if epoch != group.epoch {
        return Ok(epoch_answer(group.epoch));
    }
    take(transaction, &format!("proposal:{}", leaf.leaf_id), 3600, PROPOSALS_PER_LEAF, now_ms)?;
    let message = parsed.into_protocol_message().map_err(|error| invalid(&format!("That proposal isn't one: {error}.")))?;
    let key = seal_key(keys, transaction, &group.id, now_ms)?;
    let provider = Provider::public(crypto(), transaction, &key).map_err(|error| broken(&error.to_string()))?;
    let mut public = PublicGroup::load(provider.storage(), &group_id)
        .map_err(|error| broken(&error.to_string()))?
        .ok_or_else(|| broken("a group's public state is missing"))?;
    let processed =
        public.process_message(provider.crypto(), message).map_err(|error| invalid(&format!("That proposal doesn't apply: {error}.")))?;
    let Sender::Member(index) = *processed.sender() else { return Err(invalid("Proposals here come from members.")) };
    let mine = public.leaf(index).and_then(|node| certified(node.credential(), node.signature_key().as_slice()));
    let Some(mine) = mine.filter(|certificate| is_leaf(certificate, &leaf)) else {
        return Err(invalid("A leaf proposes only as itself."));
    };
    if !under_current(transaction, &mine)? {
        return Err(replaced());
    }
    let ProcessedMessageContent::ProposalMessage(queued) = processed.into_content() else {
        return Err(invalid("proposal isn't a proposal."));
    };
    let own_remove = match queued.proposal() {
        Proposal::Remove(remove) => remove.removed() == index,
        Proposal::SelfRemove => true,
        _ => false,
    };
    if !own_remove {
        return Err(invalid("The only proposal here is a leaf's own Remove."));
    }
    public.add_proposal(provider.storage(), *queued).map_err(|error| broken(&format!("keeping a proposal: {error}")))?;
    let seq = append(
        transaction,
        &mut group,
        Entry { kind: "proposal", epoch, envelope: &envelope, leaf: &leaf, client_message_id: None, by_agent: None },
        now_ms,
    )?;
    Ok((200, json!({"seq": seq})))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Send {
    client_message_id: String,
    envelope: Value,
    #[serde(default)]
    #[allow(dead_code)]
    request_nonce: Option<String>,
}

/// Whether an application message's `epoch` may still go into `group`.
pub fn epoch_fits(group: &Group, epoch: u64) -> bool {
    epoch <= group.epoch && group.epoch - epoch <= EPOCHS_BEHIND
}

/// `POST groups/<id>/messages`: an application message in a self group or
/// a channel. A Claktory sends only in its own channel.
fn send(transaction: &Transaction<'_>, caller: &Caller, id: &str, send: Send, now_ms: i64) -> Result<(u16, Value), Refusal> {
    let leaf = leaf_of(caller)?;
    let (mut group, _) = reached(transaction, caller, id, now_ms)?;
    match (group.kind, caller) {
        (GroupKind::Conversation, _) => return Err(invalid("A conversation's messages go through conversations/<id>/messages.")),
        (GroupKind::SelfGroup, Caller::Backend { .. }) => return Err(not_found()),
        _ => {}
    }
    if send.client_message_id.is_empty()
        || send.client_message_id.len() > 64
        || !send.client_message_id.bytes().all(|b| b.is_ascii_graphic())
    {
        return Err(invalid("client_message_id is 1 to 64 visible characters."));
    }
    let (envelope, parsed) = envelope_of(&send.envelope, MlsKind::Application, "envelope")?;
    let (group_id, epoch) = parsed.group.ok_or_else(|| invalid("envelope isn't an application message."))?;
    if group_id.as_slice() != group.id.as_slice() {
        return Err(invalid("That message is another group's."));
    }
    let earlier: Option<(i64, Vec<u8>, i64)> = transaction
        .query_row(
            "SELECT seq, digest, accepted FROM dm_mailbox WHERE group_id = ?1 AND leaf_id = ?2 AND client_message_id = ?3",
            params![group.id, leaf.leaf_id, send.client_message_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()?;
    if let Some((seq, digest, accepted)) = earlier {
        if digest == envelope.digest() {
            return Ok((200, json!({"seq": seq, "accepted_ms": accepted})));
        }
        return Err(Refusal(409, "conflict", "That message id was used for another message.".to_owned()));
    }
    if !epoch_fits(&group, epoch) {
        return Ok(epoch_answer(group.epoch));
    }
    super::identity::take_as(transaction, &format!("group-message:{}", leaf.leaf_id), 60, MESSAGES_PER_LEAF, now_ms, "Too many messages.")?;
    let seq = append(
        transaction,
        &mut group,
        Entry {
            kind: "application",
            epoch,
            envelope: &envelope,
            leaf: &leaf,
            client_message_id: Some(&send.client_message_id),
            by_agent: None,
        },
        now_ms,
    )?;
    Ok((200, json!({"seq": seq, "accepted_ms": now_ms})))
}

/// `GET groups/<id>/messages?after=&limit=`: the mailbox after a seq, in
/// order.
fn messages(db: &Connection, caller: &Caller, id: &str, query: &str, now_ms: i64) -> Result<(u16, Value), Refusal> {
    let (group, from) = reached(db, caller, id, now_ms)?;
    let (mut after, mut limit) = (0i64, PAGE);
    for pair in query.split('&').filter(|pair| !pair.is_empty()) {
        let (name, value) = pair.split_once('=').unwrap_or((pair, ""));
        match name {
            "after" => after = value.parse().ok().filter(|after| *after >= 0).ok_or_else(|| invalid("after is a seq."))?,
            "limit" => {
                limit = value.parse().ok().filter(|limit| (1..=PAGE_MOST).contains(limit)).ok_or_else(|| invalid("limit is 1 to 100."))?
            }
            // A Claktory's calls without a body carry their nonce here.
            "request_nonce" => {}
            _ => return Err(invalid("Messages take after and limit.")),
        }
    }
    if after < group.pruned_seq {
        return Ok((
            410,
            json!({"error": "too_far_behind", "message": "This device was away too long for this conversation. It joins again.", "epoch": group.epoch}),
        ));
    }
    type Row = (i64, String, i64, Vec<u8>, String, Option<String>, Option<String>, i64);
    let rows: Vec<Row> = {
        let mut statement = db.prepare(
            "SELECT seq, kind, epoch, envelope, account_id, client_message_id, by_agent, accepted FROM dm_mailbox
             WHERE group_id = ?1 AND seq > ?2 AND accepted >= ?3 ORDER BY seq LIMIT ?4",
        )?;
        statement
            .query_map(params![group.id, after, from, limit as i64 + 1], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?, row.get(5)?, row.get(6)?, row.get(7)?))
            })?
            .collect::<Result<_, _>>()?
    };
    let mut has_more = rows.len() > limit;
    let mut bytes = 0;
    let mut shown = Vec::new();
    for (seq, kind, epoch, envelope, account, client_message_id, by_agent, accepted) in rows.into_iter().take(limit) {
        bytes += envelope.len();
        if bytes > PAGE_BYTES && !shown.is_empty() {
            has_more = true;
            break;
        }
        let envelope = Envelope::new(envelope).map_err(|_| broken("a kept message doesn't parse"))?;
        let mut message = json!({"seq": seq, "kind": kind, "envelope": envelope.to_json(), "accepted_ms": accepted, "epoch": epoch});
        let own = account == caller.account_id();
        if group.kind == GroupKind::Conversation {
            message["from"] = json!(if own { "you" } else { "them" });
        }
        if let (true, Some(client_message_id)) = (own, client_message_id) {
            message["client_message_id"] = json!(client_message_id);
        }
        if let Some(by_agent) = by_agent {
            message["by_agent"] = serde_json::from_str(&by_agent).unwrap_or(Value::Null);
        }
        shown.push(message);
    }
    Ok((200, json!({"messages": shown, "has_more": has_more})))
}

/// `GET groups`: the groups the caller fetches.
fn list(db: &Connection, caller: &Caller, now_ms: i64) -> Result<Value, Refusal> {
    let account = caller.account_id();
    let candidates: Vec<Group> = {
        let mut statement = db.prepare(&format!(
            "SELECT {COLUMNS} FROM dm_groups WHERE (account_id = ?1 AND kind != 'conversation')
                 OR (kind = 'conversation' AND conversation_id IN (SELECT conversation_id FROM dm_contacts WHERE low = ?1 OR high = ?1))
             ORDER BY created, group_id"
        ))?;
        statement.query_map([account], group_row)?.collect::<Result<_, _>>()?
    };
    let mut groups = Vec::new();
    for group in candidates {
        if reach(db, caller, &group, now_ms)?.is_some() {
            groups.push(shown(db, caller, &group, now_ms)?);
        }
    }
    Ok(json!({"groups": groups}))
}

/// `GET groups/<id>/group-info`: the current epoch's, for a rejoin.
fn group_info(db: &Connection, caller: &Caller, id: &str, now_ms: i64) -> Result<Value, Refusal> {
    let group = load(db, &group_id_of(id)?)?.ok_or_else(not_found)?;
    if reach(db, caller, &group, now_ms)?.is_none() && !may_rejoin(db, caller, &group, now_ms)? {
        return Err(not_found());
    }
    let info: Vec<u8> = db.query_row("SELECT group_info FROM dm_groups WHERE group_id = ?1", [&group.id], |row| row.get(0))?;
    let envelope = Envelope::new(info).map_err(|_| broken("a kept GroupInfo doesn't parse"))?;
    Ok(json!({"epoch": group.epoch, "group_info": envelope.to_json()}))
}

/// `GET welcomes`: the Welcomes waiting for the caller's leaf.
fn welcomes(db: &Connection, caller: &Caller) -> Result<Value, Refusal> {
    let leaf = leaf_of(caller)?;
    type Row = (String, Vec<u8>, Vec<u8>, i64, i64);
    let rows: Vec<Row> = {
        let mut statement = db.prepare(
            "SELECT id, group_id, welcome, seq, accepted FROM dm_welcomes WHERE account_id = ?1 AND leaf_id = ?2 ORDER BY accepted, id",
        )?;
        statement
            .query_map(params![leaf.account, leaf.leaf_id], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?)))?
            .collect::<Result<_, _>>()?
    };
    let mut welcomes = Vec::new();
    for (id, group_id, welcome, seq, accepted) in rows {
        let Some(group) = load(db, &group_id)? else { continue };
        let envelope = Envelope::new(welcome).map_err(|_| broken("a kept Welcome doesn't parse"))?;
        let mut shown = json!({"id": id, "group_id": b64(&group.id), "kind": group.kind.name(), "welcome": envelope.to_json(), "seq": seq, "accepted_ms": accepted});
        describe(db, &group, &mut shown)?;
        welcomes.push(shown);
    }
    Ok(json!({"welcomes": welcomes}))
}

/// `GET claktory/events?since=&wait=`: what the Claktory's leaf follows, from
/// its account's outbox: messages in the groups it fetches, Welcomes to it,
/// and its groups changing.
fn events(db: &Connection, caller: &Caller, query: &str, now_ms: i64) -> Result<Value, Refusal> {
    let leaf = leaf_of(caller)?;
    if !matches!(caller, Caller::Backend { .. }) {
        return Err(Refusal(403, "claktory_only", "Only a Claktory follows these.".to_owned()));
    }
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
            db.query_row("SELECT COALESCE(MAX(seq), 0) FROM dm_account_events WHERE account = ?1", [&leaf.account], |row| row.get(0))?;
        return Ok(json!({"events": [], "next_cursor": sync::cursor(last)}));
    };
    let since = sync::since(&since_text)?;
    sync::within(db, &leaf.account, since)?;
    let rows: Vec<(i64, String, String, i64)> = {
        let mut statement = db.prepare(
            "SELECT seq, kind, object_id, object_seq FROM dm_account_events WHERE account = ?1 AND seq > ?2
             AND kind IN ('group_message', 'welcome', 'groups') ORDER BY seq LIMIT ?3",
        )?;
        statement
            .query_map(params![leaf.account, since, SCAN], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)))?
            .collect::<Result<_, _>>()?
    };
    // Past every row looked at; when they were all there were, past the
    // account's other events too.
    let all = (rows.len() as i64) < SCAN;
    let mut through = since;
    let mut shown = Vec::new();
    let mut complete = true;
    for (seq, kind, object, object_seq) in rows {
        if shown.len() >= EVENTS {
            complete = false;
            break;
        }
        through = seq;
        match kind.as_str() {
            "group_message" => {
                let Ok(id) = unb64(&object, "a group") else { continue };
                let Some(group) = load(db, &id)? else { continue };
                let Some(from) = reach(db, caller, &group, now_ms)? else { continue };
                let accepted: Option<i64> = db
                    .query_row("SELECT accepted FROM dm_mailbox WHERE group_id = ?1 AND seq = ?2", params![id, object_seq], |row| {
                        row.get(0)
                    })
                    .optional()?;
                if accepted.is_some_and(|accepted| accepted >= from) {
                    shown.push(json!({"kind": "group_message", "group_id": object, "seq": object_seq}));
                }
            }
            "welcome" => {
                let mine: bool = db.query_row(
                    "SELECT COUNT(*) > 0 FROM dm_welcomes WHERE id = ?1 AND account_id = ?2 AND leaf_id = ?3",
                    params![object, leaf.account, leaf.leaf_id],
                    |row| row.get(0),
                )?;
                if mine {
                    shown.push(json!({"kind": "welcome", "id": object}));
                }
            }
            _ => shown.push(json!({"kind": "groups"})),
        }
    }
    if all && complete {
        through = db.query_row(
            "SELECT COALESCE(MAX(seq), ?2) FROM dm_account_events WHERE account = ?1",
            params![leaf.account, through],
            |row| row.get(0),
        )?;
    }
    Ok(json!({"events": shown, "next_cursor": sync::cursor(through)}))
}

/// Forgets what's past its time: mailboxes after 30 days (a self group's
/// after 7), Welcomes after 30. What it deleted.
pub fn sweep(db: &mut Connection, now_ms: i64) -> Result<usize, Refused> {
    let transaction = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
    // A conversation's group outlives its friendship by as long as its
    // mailbox: then its public state and key go.
    let ended: Vec<Vec<u8>> = {
        let mut statement = transaction.prepare(
            "SELECT g.group_id FROM dm_groups g LEFT JOIN dm_contacts c ON c.conversation_id = g.conversation_id
             WHERE g.kind = 'conversation' AND (c.id IS NULL OR c.ended < ?1)",
        )?;
        statement.query_map([now_ms - MAILBOX_KEEP_MS], |row| row.get(0))?.collect::<Result<_, _>>()?
    };
    for id in &ended {
        forget(&transaction, id)?;
    }
    transaction.execute(
        "UPDATE dm_groups SET pruned_seq = MAX(pruned_seq, COALESCE((SELECT MAX(seq) FROM dm_mailbox m WHERE m.group_id = dm_groups.group_id
             AND m.accepted < CASE dm_groups.kind WHEN 'self' THEN ?2 ELSE ?1 END), 0))",
        params![now_ms - MAILBOX_KEEP_MS, now_ms - SELF_KEEP_MS],
    )?;
    let mut deleted = transaction
        .execute("DELETE FROM dm_mailbox WHERE seq <= (SELECT pruned_seq FROM dm_groups g WHERE g.group_id = dm_mailbox.group_id)", [])?;
    deleted += transaction.execute("DELETE FROM dm_welcomes WHERE accepted < ?1", [now_ms - WELCOME_KEEP_MS])?;
    transaction.commit()?;
    Ok(deleted + ended.len())
}

/// Forgets a group: its public state, its key, its mailbox and Welcomes.
fn forget(transaction: &Transaction<'_>, id: &[u8]) -> Result<(), Refused> {
    mls_storage::forget_group(transaction, &GroupId::from_slice(id)).map_err(|error| Refused::Db(error.to_string()))?;
    super::keys::forget(transaction, &purpose(id))?;
    transaction.execute("DELETE FROM dm_mailbox WHERE group_id = ?1", [id])?;
    transaction.execute("DELETE FROM dm_welcomes WHERE group_id = ?1", [id])?;
    transaction.execute("DELETE FROM dm_groups WHERE group_id = ?1", [id])?;
    Ok(())
}

/// Deleting an account: its own groups (self, channels) go with their
/// public state and key; Welcomes to its leaves go.
pub fn purge(transaction: &Transaction<'_>, account: &str) -> Result<(), Refused> {
    let ids: Vec<Vec<u8>> = {
        let mut statement = transaction.prepare("SELECT group_id FROM dm_groups WHERE account_id = ?1 AND kind != 'conversation'")?;
        statement.query_map([account], |row| row.get(0))?.collect::<Result<_, _>>()?
    };
    for id in ids {
        forget(transaction, &id)?;
    }
    transaction.execute("DELETE FROM dm_welcomes WHERE account_id = ?1", [account])?;
    Ok(())
}

pub fn knows(method: &str, route: &str) -> bool {
    let parts: Vec<&str> = route.split('/').collect();
    matches!(
        (method, parts.as_slice()),
        ("POST" | "GET", ["groups"])
            | ("GET" | "POST", ["groups", _, "messages"])
            | ("POST", ["groups", _, "commits" | "proposals"])
            | ("GET", ["groups", _, "group-info"])
            | ("GET", ["welcomes"])
            | ("DELETE", ["welcomes", _])
            | ("GET", ["claktory", "events"])
    )
}

pub fn answer(
    db: &mut Connection,
    keys: &Keys,
    method: &str,
    (route, query): (&str, &str),
    caller: &Caller,
    bytes: &[u8],
    now_ms: i64,
) -> Result<(u16, Value), Refusal> {
    let transaction = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let parts: Vec<&str> = route.split('/').collect();
    let (status, answer) = match (method, parts.as_slice()) {
        ("POST", ["groups"]) => (200, create(&transaction, keys, caller, body(bytes)?, now_ms)?),
        ("GET", ["groups"]) => (200, list(&transaction, caller, now_ms)?),
        ("GET", ["groups", id, "messages"]) => messages(&transaction, caller, id, query, now_ms)?,
        ("POST", ["groups", id, "messages"]) => send(&transaction, caller, id, body(bytes)?, now_ms)?,
        ("POST", ["groups", id, "commits"]) => commit(&transaction, keys, caller, id, body(bytes)?, now_ms)?,
        ("POST", ["groups", id, "proposals"]) => propose(&transaction, keys, caller, id, body(bytes)?, now_ms)?,
        ("GET", ["groups", id, "group-info"]) => (200, group_info(&transaction, caller, id, now_ms)?),
        ("GET", ["welcomes"]) => (200, welcomes(&transaction, caller)?),
        ("DELETE", ["welcomes", id]) => {
            let leaf = leaf_of(caller)?;
            let gone = transaction.execute(
                "DELETE FROM dm_welcomes WHERE id = ?1 AND account_id = ?2 AND leaf_id = ?3",
                params![id, leaf.account, leaf.leaf_id],
            )?;
            if gone == 0 {
                return Err(not_found());
            }
            (204, Value::Null)
        }
        ("GET", ["claktory", "events"]) => (200, events(&transaction, caller, query, now_ms)?),
        _ => return Err(not_found()),
    };
    // A refusal answered as such (409 epoch, 410) keeps nothing it did.
    if status < 400 {
        transaction.commit()?;
    }
    Ok((status, answer))
}

#[cfg(test)]
mod tests {
    use super::super::contacts::tests::World;
    use super::super::leaves::tests::{Person, call, claktory, device, set_up, upload};
    use super::super::messages::tests::friends;
    use super::*;
    use crate::grants::{Put, Resource};
    use crate::ipc::{Actor, Registration};
    use claktory_dm_crypto::group::{Group as Mls, Outgoing};
    use claktory_dm_crypto::validation::{GroupKind as Says, Pins};

    fn env(bytes: Vec<u8>) -> Value {
        Envelope::new(bytes).expect("an envelope").to_json()
    }

    fn bytes_of(value: &Value) -> Vec<u8> {
        Envelope::from_json(value).expect("an envelope").into_bytes()
    }

    /// What every endpoint here trusts: each person's identity and list.
    fn pins(people: &[&Person]) -> Pins {
        let mut pins = Pins::default();
        for person in people {
            pins.first_contact(&person.account, person.identity.public()).expect("pinned");
            pins.accept_list(&person.account, &person.signed.as_ref().expect("a list").to_bytes().expect("bytes")).expect("accepted");
        }
        pins
    }

    /// Makes a group on `person`'s leaf and registers it with `body`.
    fn start(world: &mut World, person: &Person, leaf: usize, says: Says, id: [u8; 16], mut body: Value) -> Mls {
        let (group, info) = person.with(leaf, |provider, me| {
            let group = Mls::create(provider, me, says, &id).expect("made");
            let info = group.group_info(provider, me).expect("a GroupInfo");
            (group, info)
        });
        body["group_info"] = env(info);
        let (status, made) = call(world, &person.caller(leaf), "POST", "groups", body);
        assert_eq!((status, made), (200, json!({"group_id": b64(&id), "epoch": 0, "last_seq": 0})));
        group
    }

    fn claim_one(world: &mut World, caller: &Caller, account: &str, leaf_id: &str) -> Vec<u8> {
        let (status, claimed) = call(world, caller, "POST", "key-packages/claim", json!({"account_id": account, "leaf_ids": [leaf_id]}));
        assert_eq!(status, 200, "{claimed}");
        bytes_of(&claimed["key_packages"][0]["key_package"])
    }

    fn post_commit(world: &mut World, caller: &Caller, id: &[u8], out: &Outgoing) -> (u16, Value) {
        let mut body = json!({"commit": env(out.commit.clone()), "group_info": env(out.group_info.clone().expect("a GroupInfo"))});
        if let Some(welcome) = &out.welcome {
            body["welcome"] = env(welcome.clone());
        }
        call(world, caller, "POST", &format!("groups/{}/commits", b64(id)), body)
    }

    fn mailbox(world: &mut World, caller: &Caller, id: &[u8], after: i64) -> (u16, Value) {
        call(world, caller, "GET", &format!("groups/{}/messages?after={after}&request_nonce=n{after}", b64(id)), Value::Null)
    }

    fn seqs(page: &Value) -> Vec<(i64, String)> {
        page["messages"]
            .as_array()
            .expect("messages")
            .iter()
            .map(|message| (message["seq"].as_i64().expect("seq"), message["kind"].as_str().expect("kind").to_owned()))
            .collect()
    }

    fn events(world: &World, account: &str, kind: &str) -> Vec<Value> {
        let changes = sync::changes(&world.db, account, 0, 100).ok().expect("changes");
        changes["events"].as_array().expect("events").iter().filter(|event| event["kind"] == kind).cloned().collect()
    }

    fn phone_of(account: &str, claktories: &[&str]) -> Caller {
        let registrations = claktories.iter().map(|id| Registration { claktory_id: (*id).into(), epoch: 1 }).collect();
        Caller::Device { account_id: account.into(), device_key: format!("{account}-phone"), registrations }
    }

    /// Only a device makes a group, of a subject of its own account's, with
    /// only itself in it, once per subject.
    #[test]
    fn a_device_starts_a_group_of_its_own_once() {
        let mut world = World::new();
        let ana = set_up(&mut world, "ana");
        let phone = ana.caller(0);
        let id = [1; 16];
        let (group, info) = ana.with(0, |provider, me| {
            let group = Mls::create(provider, me, Says::SelfGroup { account: "ana".into() }, &id).expect("made");
            let info = group.group_info(provider, me).expect("a GroupInfo");
            (group, info)
        });
        let body = json!({"kind": "self", "group_info": env(info.clone())});

        // Not an account page, not a Claktory, not someone else's device
        // with Ana's GroupInfo, not without keys.
        let page = Caller::Account { account_id: "ana".into(), registrations: Vec::new() };
        assert_eq!(call(&mut world, &page, "POST", "groups", body.clone()).0, 403);
        assert_eq!(call(&mut world, &claktory("ana", "mac"), "POST", "groups", body.clone()).0, 403);
        assert_eq!(call(&mut world, &device("bo", "bo-phone"), "POST", "groups", body.clone()).0, 400, "Bo isn't its member");
        let mut cy = Person::new("cy");
        cy.add(device("cy", "cy-phone"));
        let cy_info = cy.with(0, |provider, me| {
            Mls::create(provider, me, Says::SelfGroup { account: "cy".into() }, &[2; 16])
                .expect("made")
                .group_info(provider, me)
                .expect("info")
        });
        let (status, refused) = call(&mut world, &cy.caller(0), "POST", "groups", json!({"kind": "self", "group_info": env(cy_info)}));
        assert_eq!((status, &refused["error"]), (409, &json!("not_set_up")), "{refused}");

        // Subjects that aren't Ana's: a conversation she isn't in, a
        // Claktory that isn't hers; and fields that don't fit the kind.
        let mut other = body.clone();
        other["kind"] = json!("conversation");
        other["conversation_id"] = json!("nope");
        other["contact_generation"] = json!(1);
        assert_eq!(call(&mut world, &phone, "POST", "groups", other).0, 404);
        let mut channel = body.clone();
        channel["kind"] = json!("channel");
        channel["claktory_id"] = json!("mac");
        assert_eq!(call(&mut world, &phone, "POST", "groups", channel).0, 404, "no Claktory of hers is called mac");
        let mut mixed = body.clone();
        mixed["claktory_id"] = json!("mac");
        assert_eq!(call(&mut world, &phone, "POST", "groups", mixed).0, 400);

        let (status, made) = call(&mut world, &phone, "POST", "groups", body.clone());
        assert_eq!((status, made), (200, json!({"group_id": b64(&id), "epoch": 0, "last_seq": 0})));
        let (status, refused) = call(&mut world, &phone, "POST", "groups", body);
        assert_eq!((status, &refused["error"]), (409, &json!("exists")));
        assert_eq!(events(&world, "ana", "groups").len(), 1);

        let (_, listed) = call(&mut world, &phone, "GET", "groups", Value::Null);
        assert_eq!(listed, json!({"groups": [{"group_id": b64(&id), "kind": "self", "epoch": 0, "last_seq": 0}]}));
        assert_eq!(call(&mut world, &device("bo", "bo-phone"), "GET", "groups", Value::Null).1, json!({"groups": []}));
        let (_, info) = call(&mut world, &phone, "GET", &format!("groups/{}/group-info", b64(&id)), Value::Null);
        assert_eq!(info["epoch"], 0);
        assert_eq!(call(&mut world, &device("bo", "bo-phone"), "GET", &format!("groups/{}/group-info", b64(&id)), Value::Null).0, 404);
        drop(group);
    }

    /// A conversation: Ana adds Bo from a KeyPackage she claimed, with the
    /// Welcome to exactly it and the new epoch's GroupInfo; one commit per
    /// epoch, first come; each in the mailbox in order.
    #[test]
    fn a_conversation_takes_one_commit_an_epoch_with_its_welcome() {
        let mut world = World::new();
        let (conversation, generation) = friends(&mut world, "ana", "bo");
        let mut ana = set_up(&mut world, "ana");
        let bo = set_up(&mut world, "bo");
        let packages = bo.key_packages(0, 2);
        assert_eq!(upload(&mut world, &bo.caller(0), packages, None).0, 200);
        let says = Says::Conversation { people: ["ana".into(), "bo".into()] };
        let id = [3; 16];
        let body = json!({"kind": "conversation", "conversation_id": conversation, "contact_generation": generation});
        let mut group = start(&mut world, &ana, 0, says.clone(), id, body);
        let (phone, bo_phone) = (ana.caller(0), bo.caller(0));
        let trust = pins(&[&ana, &bo]);

        let package = claim_one(&mut world, &phone, "bo", "bo-phone");
        let out = ana.with(0, |provider, me| group.add(provider, me, &trust, &[package]).expect("added"));

        // Without its Welcome; with the old GroupInfo; from Bo; for another
        // group: refused, and nothing kept.
        let mut bare = out.clone();
        bare.welcome = None;
        assert_eq!(post_commit(&mut world, &phone, &id, &bare).0, 400);
        let mut stale = out.clone();
        stale.group_info = Some(ana.with(0, |provider, me| group.group_info(provider, me).expect("info")));
        assert_eq!(post_commit(&mut world, &phone, &id, &stale).0, 400, "the GroupInfo is the new epoch's");
        assert_eq!(post_commit(&mut world, &bo_phone, &id, &out).0, 400, "Bo isn't the leaf that made it");
        assert_eq!(post_commit(&mut world, &phone, &[9; 16], &out).0, 404);

        let (status, taken) = post_commit(&mut world, &phone, &id, &out);
        assert_eq!((status, taken), (200, json!({"seq": 1, "epoch": 1})));
        let (status, again) = post_commit(&mut world, &phone, &id, &out);
        assert_eq!((status, &again["error"], &again["epoch"]), (409, &json!("epoch"), &json!(1)));
        ana.with(0, |provider, _| group.merge_pending(provider).expect("merged"));

        // Bo's Welcome, for his phone alone.
        assert_eq!(call(&mut world, &device("bo", "bo-mac"), "GET", "welcomes", Value::Null).1, json!({"welcomes": []}));
        let (_, welcomes) = call(&mut world, &bo_phone, "GET", "welcomes", Value::Null);
        let welcome = welcomes["welcomes"][0].clone();
        assert_eq!(welcome["group_id"], b64(&id));
        assert_eq!(welcome["kind"], "conversation");
        assert_eq!(welcome["conversation_id"], conversation.as_str());
        assert_eq!(welcome["accounts"], json!(["ana", "bo"]));
        assert_eq!(welcome["seq"], 1, "the commit that added him: he reads on after it");
        assert_eq!(events(&world, "bo", "welcome"), vec![json!({"kind": "welcome", "id": welcome["id"]})]);
        let mut theirs =
            bo.with(0, |provider, _| Mls::join(provider, &trust, says.clone(), &id, &bytes_of(&welcome["welcome"])).expect("joined"));
        let path = format!("welcomes/{}", welcome["id"].as_str().expect("id"));
        assert_eq!(call(&mut world, &device("bo", "bo-mac"), "DELETE", &path, Value::Null).0, 404);
        assert_eq!(call(&mut world, &bo_phone, "DELETE", &path, Value::Null).0, 204);
        assert_eq!(call(&mut world, &bo_phone, "GET", "welcomes", Value::Null).1, json!({"welcomes": []}));

        // Both see the commit; Bo's Update comes first, Ana's is too late.
        let (_, page) = mailbox(&mut world, &bo_phone, &id, 0);
        assert_eq!(seqs(&page), vec![(1, "commit".to_owned())]);
        assert_eq!(page["messages"][0]["from"], "them");
        assert_eq!(page["messages"][0]["epoch"], 0);
        assert_eq!(mailbox(&mut world, &phone, &id, 0).1["messages"][0]["from"], "you");
        let theirs_out = bo.with(0, |provider, me| theirs.update(provider, me).expect("updated"));
        let ours_out = ana.with(0, |provider, me| group.update(provider, me).expect("updated"));
        assert_eq!(post_commit(&mut world, &bo_phone, &id, &theirs_out), (200, json!({"seq": 2, "epoch": 2})));
        assert_eq!(post_commit(&mut world, &phone, &id, &ours_out).1["epoch"], 2);
        ana.with(0, |provider, _| {
            group.clear_pending(provider).expect("cleared");
            group.process(provider, &trust, &out_bytes(&theirs_out)).expect("processed");
        });
        assert_eq!(group.epoch(), 2);
        assert_eq!(events(&world, "ana", "group_message").len(), 2);
        assert_eq!(
            events(&world, "bo", "group_message"),
            vec![
                json!({"kind": "group_message", "group_id": b64(&id), "seq": 1}),
                json!({"kind": "group_message", "group_id": b64(&id), "seq": 2})
            ]
        );

        // Ana removes only her own account's leaves, never Bo's.
        let theirs_removed = ana.with(0, |provider, me| group.remove(provider, me, &["bo-phone"]).expect("made"));
        let (status, refused) = post_commit(&mut world, &phone, &id, &theirs_removed);
        assert_eq!((status, &refused["error"]), (403, &json!("not_allowed")), "{refused}");
        ana.with(0, |provider, _| group.clear_pending(provider).expect("cleared"));

        // A conversation's messages don't come this way.
        let sent = ana.with(0, |provider, me| group.send(provider, me, b"hi").expect("sent"));
        let send = json!({"client_message_id": "m-1", "envelope": env(sent)});
        assert_eq!(call(&mut world, &phone, "POST", &format!("groups/{}/messages", b64(&id)), send).0, 400);

        // Only KeyPackages handed out here: Ana's Mac, unclaimed, then claimed.
        let mac = ana.add(device("ana", "ana-mac"));
        assert_eq!(call(&mut world, &phone, "PUT", "me/keys", ana.put(false)).0, 200);
        let packages = ana.key_packages(mac, 1);
        assert_eq!(upload(&mut world, &ana.caller(mac), packages.clone(), None).0, 200);
        let trust = pins(&[&ana, &bo]);
        let unclaimed = ana.with(0, |provider, me| group.add(provider, me, &trust, &[bytes_of(&packages[0])]).expect("added"));
        assert_eq!(post_commit(&mut world, &phone, &id, &unclaimed).0, 400);
        ana.with(0, |provider, _| group.clear_pending(provider).expect("cleared"));
        let package = claim_one(&mut world, &phone, "ana", "ana-mac");
        let claimed = ana.with(0, |provider, me| group.add(provider, me, &trust, &[package]).expect("added"));
        assert_eq!(post_commit(&mut world, &phone, &id, &claimed), (200, json!({"seq": 3, "epoch": 3})));
        let left: i64 = world.db.query_row("SELECT COUNT(*) FROM dm_claims", [], |row| row.get(0)).expect("count");
        assert_eq!(left, 0, "a claim is used once");

        // Bo asks to leave; Ana's next commit takes his own Remove in.
        ana.with(0, |provider, _| group.merge_pending(provider).expect("merged"));
        bo.with(0, |provider, _| {
            theirs.merge_pending(provider).expect("merged");
            theirs.process(provider, &trust, &claimed.commit).expect("processed");
        });
        let leave = bo.with(0, |provider, me| theirs.leave(provider, me).expect("proposed"));
        let path = format!("groups/{}/proposals", b64(&id));
        assert_eq!(call(&mut world, &bo_phone, "POST", &path, json!({"proposal": env(leave.clone())})), (200, json!({"seq": 4})));
        ana.with(0, |provider, _| group.process(provider, &trust, &leave).expect("processed"));
        let taken = ana.with(0, |provider, me| group.update(provider, me).expect("committed"));
        assert_eq!(post_commit(&mut world, &phone, &id, &taken), (200, json!({"seq": 5, "epoch": 4})));
        ana.with(0, |provider, _| group.merge_pending(provider).expect("merged"));
        assert_eq!(group.members(&trust).len(), 2, "Bo's phone is out");
    }

    fn out_bytes(out: &Outgoing) -> Vec<u8> {
        out.commit.clone()
    }

    /// A Claktory reads its own channel and the conversations its agents
    /// are granted on, from the grant on; it sends only in its channel,
    /// commits only its own Update, and asks to leave by proposal.
    #[test]
    fn a_claktory_reaches_its_channel_and_granted_conversations_only() {
        let mut world = World::new();
        let (conversation, generation) = friends(&mut world, "ana", "bo");
        let mut ana = Person::new("ana");
        ana.add(phone_of("ana", &["mac"]));
        let mac = ana.add(claktory("ana", "mac"));
        assert_eq!(call(&mut world, &ana.caller(0), "PUT", "me/keys", ana.put(true)).0, 200);
        let bo = set_up(&mut world, "bo");
        let packages = bo.key_packages(0, 1);
        assert_eq!(upload(&mut world, &bo.caller(0), packages, None).0, 200);
        let packages = ana.key_packages(mac, 3);
        assert_eq!(upload(&mut world, &ana.caller(mac), packages, None).0, 200);
        let trust = pins(&[&ana, &bo]);
        let (phone, machine) = (ana.caller(0), ana.caller(mac));
        let (start_cursor, cursor) = {
            let (status, start) = call(&mut world, &machine, "GET", "claktory/events", Value::Null);
            assert_eq!(status, 200, "{start}");
            (start.clone(), start["next_cursor"].as_str().expect("a cursor").to_owned())
        };
        assert_eq!(start_cursor["events"], json!([]));

        // The channel, with the Claktory added.
        let says = Says::Channel { account: "ana".into(), claktory_id: "mac".into() };
        let channel_id = [4; 16];
        let mut channel = start(&mut world, &ana, 0, says.clone(), channel_id, json!({"kind": "channel", "claktory_id": "mac"}));
        let package = claim_one(&mut world, &phone, "ana", "mac#1");
        let out = ana.with(0, |provider, me| channel.add(provider, me, &trust, &[package]).expect("added"));
        assert_eq!(post_commit(&mut world, &phone, &channel_id, &out).0, 200);
        ana.with(0, |provider, _| channel.merge_pending(provider).expect("merged"));
        let (_, welcomes) = call(&mut world, &machine, "GET", "welcomes", Value::Null);
        assert_eq!(welcomes["welcomes"][0]["claktory_id"], "mac");
        let mut its = ana.with(mac, |provider, _| {
            Mls::join(provider, &trust, says.clone(), &channel_id, &bytes_of(&welcomes["welcomes"][0]["welcome"])).expect("joined")
        });

        // It sends there, not in Ana's self group.
        let sent = ana.with(mac, |provider, me| its.send(provider, me, b"done").expect("sent"));
        let send = json!({"client_message_id": "c-1", "envelope": env(sent.clone())});
        let path = format!("groups/{}/messages", b64(&channel_id));
        assert_eq!(call(&mut world, &machine, "POST", &path, send.clone()).1["seq"], 2);
        assert_eq!(call(&mut world, &machine, "POST", &path, send).1["seq"], 2, "a retry is the same message");
        let other = ana.with(mac, |provider, me| its.send(provider, me, b"other").expect("sent"));
        let (status, refused) = call(&mut world, &machine, "POST", &path, json!({"client_message_id": "c-1", "envelope": env(other)}));
        assert_eq!((status, &refused["error"]), (409, &json!("conflict")));
        let self_id = [5; 16];
        let _own = start(&mut world, &ana, 0, Says::SelfGroup { account: "ana".into() }, self_id, json!({"kind": "self"}));
        let in_self = json!({"client_message_id": "c-2", "envelope": env(sent)});
        assert_eq!(call(&mut world, &machine, "POST", &format!("groups/{}/messages", b64(&self_id)), in_self).0, 404);
        assert_eq!(mailbox(&mut world, &machine, &self_id, 0).0, 404);

        // Its own Update; nothing else.
        let update = ana.with(mac, |provider, me| its.update(provider, me).expect("updated"));
        assert_eq!(post_commit(&mut world, &machine, &channel_id, &update).1["epoch"], 2);
        ana.with(mac, |provider, _| its.merge_pending(provider).expect("merged"));
        ana.with(0, |provider, _| channel.process(provider, &trust, &update.commit).expect("processed"));
        let remove = ana.with(mac, |provider, me| its.remove(provider, me, &["ana-phone"]).expect("made"));
        let (status, refused) = post_commit(&mut world, &machine, &channel_id, &remove);
        assert_eq!((status, &refused["error"]), (403, &json!("claktory_update_only")), "{refused}");
        ana.with(mac, |provider, _| its.clear_pending(provider).expect("cleared"));

        // The conversation: nothing for it before a grant, then from the
        // grant on; adding it takes the grant too.
        let says = Says::Conversation { people: ["ana".into(), "bo".into()] };
        let chat_id = [6; 16];
        let body = json!({"kind": "conversation", "conversation_id": conversation, "contact_generation": generation});
        let mut chat = start(&mut world, &ana, 0, says.clone(), chat_id, body);
        let package = claim_one(&mut world, &phone, "bo", "bo-phone");
        let out = ana.with(0, |provider, me| chat.add(provider, me, &trust, &[package]).expect("added"));
        assert_eq!(post_commit(&mut world, &phone, &chat_id, &out).0, 200);
        ana.with(0, |provider, _| chat.merge_pending(provider).expect("merged"));
        assert_eq!(mailbox(&mut world, &machine, &chat_id, 0).0, 404);
        let package = claim_one(&mut world, &phone, "ana", "mac#1");
        let early = ana.with(0, |provider, me| chat.add(provider, me, &trust, &[package]).expect("added"));
        let (status, refused) = post_commit(&mut world, &phone, &chat_id, &early);
        assert_eq!((status, &refused["error"]), (403, &json!("not_allowed")), "no agent of the Mac's is granted on it yet");
        ana.with(0, |provider, _| chat.clear_pending(provider).expect("cleared"));

        world.now += 1_000;
        let june = Actor { id: "june".into(), incarnation: "1".into() };
        let put = Put {
            app: "dms".into(),
            agent: june.clone(),
            scopes: vec!["dm.read".into()],
            resources: vec![Resource { kind: "conversation".into(), id: conversation.clone() }],
            expires_ms: 0,
        };
        crate::grants::put(&mut world.db, &machine, put, world.now).expect("granted");
        let package = claim_one(&mut world, &phone, "ana", "mac#1");
        let out = ana.with(0, |provider, me| chat.add(provider, me, &trust, &[package]).expect("added"));
        assert_eq!(post_commit(&mut world, &phone, &chat_id, &out), (200, json!({"seq": 2, "epoch": 2})));
        let (status, page) = mailbox(&mut world, &machine, &chat_id, 0);
        assert_eq!((status, seqs(&page)), (200, vec![(2, "commit".to_owned())]), "only from the grant on");
        let as_june = Caller::Backend { account_id: "ana".into(), claktory_id: "mac".into(), epoch: 1, agent: Some(june.clone()) };
        assert_eq!(mailbox(&mut world, &as_june, &chat_id, 0).0, 404, "the Claktory, not its agent");

        // What it lists, and follows.
        let (_, listed) = call(&mut world, &machine, "GET", "groups", Value::Null);
        let kinds: Vec<&str> =
            listed["groups"].as_array().expect("groups").iter().map(|group| group["kind"].as_str().expect("kind")).collect();
        assert_eq!(kinds, ["channel", "conversation"]);
        assert_eq!(listed["groups"][1]["grants"], json!([{"agent": {"id": "june", "incarnation": "1"}, "from_ms": world.now}]));
        let (status, followed) = call(&mut world, &machine, "GET", &format!("claktory/events?since={cursor}"), Value::Null);
        assert_eq!(status, 200, "{followed}");
        let kinds: Vec<(String, Value)> = followed["events"]
            .as_array()
            .expect("events")
            .iter()
            .map(|event| (event["kind"].as_str().expect("kind").to_owned(), event["group_id"].clone()))
            .collect();
        let next = followed["next_cursor"].as_str().expect("a cursor").to_owned();
        assert_eq!(call(&mut world, &machine, "GET", &format!("claktory/events?since={next}"), Value::Null).1["events"], json!([]));
        let channel_b64 = json!(b64(&channel_id));
        let chat_b64 = json!(b64(&chat_id));
        assert_eq!(
            kinds,
            vec![
                // The channel made, the Claktory added, its message.
                ("groups".to_owned(), Value::Null),
                ("group_message".to_owned(), channel_b64.clone()),
                ("welcome".to_owned(), Value::Null),
                ("group_message".to_owned(), channel_b64.clone()),
                // Ana's self group (a group of the account's, not one it
                // fetches), its own Update, the conversation made.
                ("groups".to_owned(), Value::Null),
                ("group_message".to_owned(), channel_b64),
                ("groups".to_owned(), Value::Null),
                // The grant, then added to the conversation.
                ("groups".to_owned(), Value::Null),
                ("group_message".to_owned(), chat_b64),
                ("welcome".to_owned(), Value::Null),
            ]
        );

        // The grant ends: its feed says so at once, and the conversation
        // leaves its list.
        let (_, welcomes) = call(&mut world, &machine, "GET", "welcomes", Value::Null);
        let grant: String = world.db.query_row("SELECT id FROM grants", [], |row| row.get(0)).expect("the grant");
        world.now += 1_000;
        crate::grants::revoke(&mut world.db, &phone, &grant, world.now).expect("revoked");
        let (_, followed) = call(&mut world, &machine, "GET", &format!("claktory/events?since={next}"), Value::Null);
        assert_eq!(followed["events"], json!([{"kind": "groups"}]));
        let (_, listed) = call(&mut world, &machine, "GET", "groups", Value::Null);
        assert_eq!(listed["groups"].as_array().map(Vec::len), Some(1), "the channel only: {listed}");
        assert_eq!(mailbox(&mut world, &machine, &chat_id, 0).0, 404);

        // It still asks to leave by proposal, with no grant (Fugo L6): its
        // own Remove only, and only the Claktory itself.
        let welcome = welcomes["welcomes"]
            .as_array()
            .expect("welcomes")
            .iter()
            .find(|welcome| welcome["group_id"] == json!(b64(&chat_id)))
            .cloned()
            .expect("one");
        let mut in_chat =
            ana.with(mac, |provider, _| Mls::join(provider, &trust, says, &chat_id, &bytes_of(&welcome["welcome"])).expect("joined"));
        let leave = ana.with(mac, |provider, me| in_chat.leave(provider, me).expect("proposed"));
        let path = format!("groups/{}/proposals", b64(&chat_id));
        assert_eq!(call(&mut world, &as_june, "POST", &path, json!({"proposal": env(leave.clone())})).0, 403);
        let (status, proposed) = call(&mut world, &machine, "POST", &path, json!({"proposal": env(leave)}));
        assert_eq!((status, proposed), (200, json!({"seq": 3})));
        assert_eq!(seqs(&mailbox(&mut world, &phone, &chat_id, 2).1), vec![(3, "proposal".to_owned())]);
    }

    /// A device of the account joins its groups by external commit from the
    /// GroupInfo the worker keeps; no one else does.
    #[test]
    fn a_device_of_the_account_rejoins_from_the_group_info() {
        let mut world = World::new();
        let mut ana = set_up(&mut world, "ana");
        let says = Says::SelfGroup { account: "ana".into() };
        let id = [7; 16];
        let mut group = start(&mut world, &ana, 0, says.clone(), id, json!({"kind": "self"}));
        let mac = ana.add(device("ana", "ana-mac"));
        assert_eq!(call(&mut world, &ana.caller(0), "PUT", "me/keys", ana.put(false)).0, 200);
        let trust = pins(&[&ana]);
        let path = format!("groups/{}/group-info", b64(&id));
        let (status, info) = call(&mut world, &ana.caller(mac), "GET", &path, Value::Null);
        assert_eq!(status, 200, "{info}");
        let (mut joined, out) = ana
            .with(mac, |provider, me| Mls::rejoin(provider, me, &trust, says.clone(), &bytes_of(&info["group_info"])).expect("rejoined"));
        let bo = set_up(&mut world, "bo");
        assert_eq!(post_commit(&mut world, &bo.caller(0), &id, &out).0, 404, "Bo can't join Ana's self group");
        assert_eq!(post_commit(&mut world, &ana.caller(mac), &id, &out), (200, json!({"seq": 1, "epoch": 1})));
        ana.with(mac, |provider, _| joined.merge_pending(provider).expect("merged"));
        ana.with(0, |provider, _| group.process(provider, &trust, &out.commit).expect("processed"));
        assert_eq!(group.epoch(), 1);

        // In a self group, the devices talk.
        let sent = ana.with(mac, |provider, me| joined.send(provider, me, b"read 4").expect("sent"));
        let body = json!({"client_message_id": "r-1", "envelope": env(sent)});
        assert_eq!(call(&mut world, &ana.caller(mac), "POST", &format!("groups/{}/messages", b64(&id)), body).1["seq"], 2);
        let (_, page) = mailbox(&mut world, &ana.caller(0), &id, 1);
        assert_eq!(seqs(&page), vec![(2, "application".to_owned())]);
        assert_eq!(page["messages"][0]["client_message_id"], "r-1");
        assert!(page["messages"][0].get("from").is_none());

        // A week on, the self group's mailbox is gone: rejoin.
        world.now += 8 * 86_400_000;
        assert!(sweep(&mut world.db, world.now).expect("swept") >= 2);
        let (status, behind) = mailbox(&mut world, &ana.caller(0), &id, 1);
        assert_eq!((status, &behind["error"]), (410, &json!("too_far_behind")));
        assert_eq!(mailbox(&mut world, &ana.caller(0), &id, 2), (200, json!({"messages": [], "has_more": false})));
    }

    /// Deleting an account deletes its own groups, their state and key.
    #[test]
    fn deleting_an_account_deletes_its_groups() {
        let mut world = World::new();
        let ana = set_up(&mut world, "ana");
        let _group = start(&mut world, &ana, 0, Says::SelfGroup { account: "ana".into() }, [8; 16], json!({"kind": "self"}));
        assert!(super::super::identity::purge(&mut world.db, &world.keys, "ana", world.now).is_ok());
        for table in ["dm_groups", "mls_values"] {
            let left: i64 = world.db.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| row.get(0)).expect("count");
            assert_eq!(left, 0, "{table}");
        }
        let keys: i64 =
            world.db.query_row("SELECT COUNT(*) FROM dm_keys WHERE purpose LIKE 'group:%'", [], |row| row.get(0)).expect("count");
        assert_eq!(keys, 0);
    }
}
