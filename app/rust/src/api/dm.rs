//! End-to-end direct messages, for Dart (dms-e2ee.md §5.3, E4): this
//! device's leaf, the person's identity, the pins of who they talk to, and
//! every group the device is in, over the sealed store (dm/store.rs).
//!
//! Dart does the talking: it fetches from the worker and sends to it, and
//! keeps the keys in the platform's key store (people/dm_keys.dart). This
//! makes and checks everything: keys, certificates, device lists, the
//! hand-off, safety numbers, and MLS. What it's given it checks, and what
//! fails the checks is dropped with why, never used.
//!
//! - **In order, one transaction per message** ([`DmDevice::process`]):
//!   OpenMLS's state, the history row and the group's position commit
//!   together, so a crash never loses a message (it decrypts again) and
//!   never shows one twice.
//! - **Encrypted once** ([`DmDevice::send`]): the envelope is kept until it
//!   comes back in the feed, so a retry sends the same bytes; a member
//!   can't decrypt its own message, so it's recognised by its digest and
//!   its history row comes from what was kept.
//! - **Commits** go out as [`DmCommit`]; the device merges its own once the
//!   worker took it ([`DmDevice::commit_taken`], or seeing it come back),
//!   and drops it when another came first ([`DmDevice::commit_refused`]).
//! - **Who's in a group** changes only by commits: each one that adds or
//!   removes leaves is kept in the history as its own line
//!   ([`DmMessage::members`]), from the certificates, never from the service.
//! - **The self group** carries the person's drafts, read positions,
//!   deletions and pins between their devices: those apply to the
//!   conversation they name ([`DmDevice::view`]) and to the pins, and aren't
//!   kept as history.
//! - **Proposals and agents' drafts** come by their own routes, not the
//!   feed: [`DmDevice::open_side`] decrypts each once and keeps it by its
//!   digest.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use anyhow::{Context as _, anyhow, bail};
use claktory_dm_crypto::certificate::{Leaf, LeafCertificate, LeafKind};
use claktory_dm_crypto::content::{ByAgent, Content};
use claktory_dm_crypto::device_list::{DeviceList, Listed, SignedDeviceList};
use claktory_dm_crypto::envelope::{self, Envelope};
use claktory_dm_crypto::group::{Group, Incoming, LeafKey, Me, Window};
use claktory_dm_crypto::handoff::{self, Offer, OfferFor, Waiting};
use claktory_dm_crypto::identity::{IdentityKey, IdentityPublic};
use claktory_dm_crypto::safety::SafetyNumber;
use claktory_dm_crypto::validation::{GroupKind, Pins};
use claktory_dm_crypto::{Error, provider::Provider};
use flutter_rust_bridge::frb;
use rusqlite::{OptionalExtension, Transaction, params};
use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

use crate::dm::store::{Opened, Store};

/// 32 new random bytes: a key for the platform key store (the identity,
/// this device's leaf, the store's seal key).
#[frb(sync)]
pub fn dm_new_secret() -> anyhow::Result<Vec<u8>> {
    Ok(IdentityKey::generate()?.secret().to_vec())
}

/// The public half of an identity or leaf key's 32 secret bytes.
#[frb(sync)]
pub fn dm_public_key(secret: Vec<u8>) -> anyhow::Result<Vec<u8>> {
    Ok(IdentityKey::from_secret(&bytes32(&secret)?).public().0.to_vec())
}

/// Deletes the store at [path]: starting over, after it was made with
/// another key, or on signing out.
pub fn dm_remove_store(path: String) -> anyhow::Result<()> {
    Ok(Store::remove(&PathBuf::from(path))?)
}

fn bytes32(bytes: &[u8]) -> anyhow::Result<[u8; 32]> {
    bytes.try_into().map_err(|_| anyhow!("a key is 32 bytes"))
}

/// What kind of leaf: one of the person's devices, or one of their
/// Claktories.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DmLeafKind {
    Device,
    Claktory,
}

impl From<DmLeafKind> for LeafKind {
    fn from(kind: DmLeafKind) -> LeafKind {
        match kind {
            DmLeafKind::Device => LeafKind::Device,
            DmLeafKind::Claktory => LeafKind::Claktory,
        }
    }
}

impl From<LeafKind> for DmLeafKind {
    fn from(kind: LeafKind) -> DmLeafKind {
        match kind {
            LeafKind::Device => DmLeafKind::Device,
            LeafKind::Claktory => DmLeafKind::Claktory,
        }
    }
}

/// One leaf on a device list.
#[derive(Debug, Clone)]
pub struct DmListed {
    pub kind: DmLeafKind,
    pub leaf_id: String,
    pub signature_key: Vec<u8>,
}

/// An account's device list, as this device last took it.
#[derive(Debug, Clone)]
pub struct DmDeviceList {
    pub version: u64,
    /// When it was signed, by the signing device's clock (ms).
    pub created_ms: u64,
    pub leaves: Vec<DmListed>,
    /// The account's home Claktory (#451): its iroh endpoint id (32 bytes),
    /// signed with the rest; None while the list names none.
    pub home: Option<Vec<u8>>,
}

/// What kind of group.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DmGroupType {
    /// A conversation between two people.
    Conversation,
    /// The person's own devices.
    SelfGroup,
    /// The person's devices and one of their Claktories.
    Channel,
}

/// Which group, from the person's own records (the worker's group list as
/// this device knows it), never from the group's own say: a conversation's
/// two people's account ids; a self group's or a channel's one person, and
/// a channel's Claktory.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DmGroupKind {
    pub kind: DmGroupType,
    pub accounts: Vec<String>,
    pub claktory_id: String,
}

impl TryFrom<&DmGroupKind> for GroupKind {
    type Error = Error;

    fn try_from(kind: &DmGroupKind) -> Result<GroupKind, Error> {
        let bad = || Error::Format("a group kind needs its people".to_owned());
        Ok(match (kind.kind, kind.accounts.as_slice()) {
            (DmGroupType::Conversation, [a, b]) => GroupKind::Conversation { people: [a.clone(), b.clone()] },
            (DmGroupType::SelfGroup, [account]) => GroupKind::SelfGroup { account: account.clone() },
            (DmGroupType::Channel, [account]) if !kind.claktory_id.is_empty() => {
                GroupKind::Channel { account: account.clone(), claktory_id: kind.claktory_id.clone() }
            }
            _ => return Err(bad()),
        })
    }
}

fn group_kind(kind: &DmGroupKind) -> Result<GroupKind, Error> {
    kind.try_into()
}

/// How this device stands.
#[derive(Debug, Clone)]
pub struct DmStatus {
    /// It has the person's identity and its own certificate: it can talk.
    pub set_up: bool,
    pub account_id: String,
    pub leaf_id: String,
    pub name: String,
    /// This device's MLS signature key (its leaf's public half).
    pub leaf_public: Vec<u8>,
    /// The person's identity key, once known here.
    pub identity_public: Option<Vec<u8>>,
}

/// What a set-up gives the worker: the identity's public half, this
/// device's certificate and the person's device list.
#[derive(Debug, Clone)]
pub struct DmSetUp {
    pub identity_public: Vec<u8>,
    pub certificate: Vec<u8>,
    pub device_list: Vec<u8>,
}

/// What the new device got in the hand-off: the person's identity (its
/// secret, for the platform key store at once) and this device's
/// certificate, which the person's device list must name before others
/// accept it.
pub struct DmHandoffDone {
    pub account_id: String,
    pub identity_key: Vec<u8>,
    pub identity_public: Vec<u8>,
    pub certificate: Vec<u8>,
}

/// What the scanning device sends back through the worker, and the new
/// device's leaf key, to add to the device list.
#[derive(Debug, Clone)]
pub struct DmHandoffSealed {
    pub sealed: Vec<u8>,
    pub leaf_signature_key: Vec<u8>,
    /// The new device as its QR code says: its leaf id (its account key),
    /// name and platform, to ask the person and to list it, and its iroh
    /// endpoint id, the only one history transfer dials.
    pub leaf_id: String,
    pub name: String,
    pub platform: String,
    pub endpoint_id: Vec<u8>,
    /// The digits the new device shows beside its code: for a pasted code,
    /// the person checks they match before adding it.
    pub comparison: String,
}

/// What this device knows of an account's security key.
#[derive(Debug, Clone)]
pub struct DmPin {
    pub identity_public: Vec<u8>,
    /// The person compared safety numbers for this key.
    pub verified: bool,
    /// The key changed and the person hasn't seen the notice: sending waits.
    pub changed_unseen: bool,
    pub can_send: bool,
    /// The latest device list's version under this key; 0 before any.
    pub device_list_version: u64,
    /// The key this one replaced at its last change, while there's still
    /// that key to go back to: what "The numbers don't match" goes back to
    /// ([DmDevice::undo_key_change], E11-3). None otherwise.
    pub changed_from: Option<Vec<u8>>,
    /// This key is one the person said didn't match before, back again:
    /// the notice says so. Cleared once they verify it.
    pub mismatched: bool,
}

/// Two people's safety number: 60 digits in twelve groups, the same on
/// both sides, and this side's QR code.
#[derive(Debug, Clone)]
pub struct DmSafety {
    pub digits: String,
    pub qr: Vec<u8>,
}

/// A change to a group, for the worker: the commit; for an Add, the
/// Welcome; and the new epoch's GroupInfo, which the worker keeps.
#[derive(Debug, Clone)]
pub struct DmCommit {
    /// Each as an envelope's JSON (`{"format":2,"bytes":…}`).
    pub commit: String,
    pub welcome: Option<String>,
    pub group_info: Option<String>,
}

/// A group this device is in.
#[derive(Debug, Clone)]
pub struct DmGroup {
    pub group_id: Vec<u8>,
    pub kind: DmGroupKind,
    /// The last seq processed.
    pub seq: i64,
    pub epoch: u64,
    /// This device's leaf here carries its current certificate: false
    /// until it moves to the person's new security key ([DmDevice::move_group]).
    pub moved: bool,
    /// A commit of this device's waits to be merged or given up: one the
    /// app stopped before it heard of (row 553), settled against the
    /// worker's log ([DmDevice::take_worker_commit]).
    pub pending: bool,
}

/// How this device's state of a group stands against the worker's
/// ([DmDevice::fork_check], E11-8).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DmForkState {
    /// Same epoch, same confirmation tag.
    InStep,
    /// The worker is past it: catch up from the feed.
    Behind,
    /// Past the worker, or at its epoch with another confirmation tag, or
    /// with no state at all: it joins again ([DmDevice::resync_group]).
    /// Nothing changes there, nor is sent, until then.
    Forked,
    /// A commit of this device's made under [PENDING_FOR] ago waits for
    /// the worker's answer: nothing changed; ask again later.
    Pending,
    /// A commit of this device's waited longer, and the worker is at the
    /// epoch it was made at, so never took it: given up (a rejoin's state
    /// dropped). Ask again: it may be in step now, or need to rejoin.
    PendingCleared,
}

/// What a rejoin to get back in step with the worker made
/// ([DmDevice::resync_group]).
#[derive(Debug, Clone)]
pub struct DmResync {
    /// The external commit, for the worker; once taken, take out what's
    /// dead at once ([DmDevice::remove_dead_leaves]).
    pub commit: DmCommit,
    /// The retry ids of this device's messages to the group that never
    /// went: send each again through the unsent path, encrypted anew.
    pub resend: Vec<String>,
}

/// One account's device list version, as the worker has it (GET groups'
/// `list_versions`), for [DmDevice::send].
#[derive(Debug, Clone)]
pub struct DmListVersion {
    pub account: String,
    pub version: u64,
}

/// One device list this device took (E11-8): whose, which version, when.
/// No keys.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DmListAdopted {
    pub account: String,
    pub version: u64,
    pub at_ms: i64,
}

/// Why a change to a group or a send was refused (E11-8).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DmGroupErrorKind {
    /// This device's state of the group is forked from the worker's
    /// ([DmForkState::Forked]): resync first.
    Forked,
    /// A commit of this device's waits for the worker: settle it first
    /// ([DmDevice::fork_check]).
    Pending,
    /// A member's device list here is older than the worker's: fetch those
    /// [DmGroupError::accounts]' lists, then send again.
    ListsStale,
    /// Anything else, as [DmGroupError::message] says.
    Failed,
}

/// A refusal from a change to a group or a send, typed (E11-8).
#[derive(Debug, Clone)]
pub struct DmGroupError {
    pub kind: DmGroupErrorKind,
    /// The group it's about (empty for [DmGroupErrorKind::Failed]).
    pub group_id: Vec<u8>,
    /// Forked: it can't join again from the worker's GroupInfo, which is
    /// below the highest epoch this device reached. It says so to the
    /// group's devices (`POST groups/<id>/out-of-step`), one makes an
    /// empty commit, and it resyncs past it.
    pub stuck: bool,
    /// ListsStale: whose lists to fetch.
    pub accounts: Vec<String>,
    /// What went wrong, as words (for Failed, as before).
    pub message: String,
}

impl DmGroupError {
    fn of(kind: DmGroupErrorKind, group_id: &[u8], message: &str) -> DmGroupError {
        DmGroupError { kind, group_id: group_id.to_vec(), stuck: false, accounts: vec![], message: message.to_owned() }
    }
}

impl std::fmt::Display for DmGroupError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl From<anyhow::Error> for DmGroupError {
    fn from(error: anyhow::Error) -> DmGroupError {
        DmGroupError::of(DmGroupErrorKind::Failed, &[], &format!("{error:#}"))
    }
}

impl From<Error> for DmGroupError {
    fn from(error: Error) -> DmGroupError {
        DmGroupError::of(DmGroupErrorKind::Failed, &[], &error.to_string())
    }
}

/// What [DmDevice::forked_place] keeps.
#[derive(Serialize, Deserialize)]
struct Forked {
    stuck: bool,
}

/// Which agent wrote a message the person approved.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DmByAgent {
    pub claktory_id: String,
    pub agent_id: String,
    pub incarnation: String,
    pub name: String,
}

/// What a message says: its kind ("text", "draft", "read", "delete",
/// "proposal"), its retry id, its text, who of the person's agents wrote it,
/// and the kind's other fields as a JSON object.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DmContent {
    pub kind: String,
    pub client_message_id: String,
    pub text: Option<String>,
    pub by_agent: Option<DmByAgent>,
    pub extra_json: String,
}

impl DmContent {
    fn to_content(&self) -> anyhow::Result<Content> {
        let extra = if self.extra_json.trim().is_empty() {
            serde_json::Map::new()
        } else {
            match serde_json::from_str(&self.extra_json).context("a content's extra fields")? {
                serde_json::Value::Object(map) => map,
                _ => bail!("a content's extra fields are a JSON object"),
            }
        };
        Ok(Content {
            v: claktory_dm_crypto::content::VERSION,
            kind: self.kind.clone(),
            client_message_id: self.client_message_id.clone(),
            text: self.text.clone(),
            by_agent: self.by_agent.as_ref().map(|by| ByAgent {
                claktory_id: by.claktory_id.clone(),
                agent_id: by.agent_id.clone(),
                incarnation: by.incarnation.clone(),
                name: by.name.clone(),
            }),
            extra,
        })
    }

    /// With the sender's clock [now_ms] inside, as "sent_ms" among the
    /// extra fields (row 556): shown with the message, never its order.
    /// One that says already keeps its own.
    fn sent_at(mut self, now_ms: i64) -> anyhow::Result<DmContent> {
        let mut content = self.to_content()?;
        if content.sent_ms().is_none() && now_ms > 0 {
            content.set_sent_ms(now_ms);
            self.extra_json = serde_json::Value::Object(content.extra).to_string();
        }
        Ok(self)
    }

    fn from_content(content: Content) -> DmContent {
        DmContent {
            kind: content.kind,
            client_message_id: content.client_message_id,
            text: content.text,
            by_agent: content.by_agent.map(|by| DmByAgent {
                claktory_id: by.claktory_id,
                agent_id: by.agent_id,
                incarnation: by.incarnation,
                name: by.name,
            }),
            extra_json: serde_json::Value::Object(content.extra).to_string(),
        }
    }
}

/// A message in a group's history, as this device keeps it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DmMessage {
    pub seq: i64,
    /// This device sent it.
    pub mine: bool,
    /// Who sent it, from its certified leaf.
    pub from_account: String,
    pub from_leaf: String,
    pub from_kind_device: bool,
    pub content: DmContent,
    pub arrived_ms: i64,
    /// A commit that changed who's in the group, instead of something
    /// said ([content] is then empty): never from anyone's content.
    #[serde(default)]
    pub members: Option<DmMembers>,
    /// Said by a device its person had taken out by the time this device
    /// read it (row 561, Fugo): taken in a window, as an online member took
    /// it then, and shown as from that device.
    #[serde(default)]
    pub departed: bool,
}

/// A leaf in a group, as its certificate says.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DmMember {
    pub account: String,
    pub leaf_id: String,
    pub kind: DmLeafKind,
    /// The name its certificate gives (a device's, or a Claktory's).
    pub name: String,
    /// When its certificate was made: when it was added, or the person's
    /// last new security key.
    #[serde(default)]
    pub since_ms: u64,
}

/// A conversation the other person isn't in yet (row 676): none of their
/// devices passes the checks there, so nothing is sent until one does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DmJoining {
    /// Whose devices it waits for.
    pub account: String,
    /// This device adds them first: it's the person's member with the
    /// lowest leaf index there (the creator, unless it left), so the
    /// person's devices don't each spend the friend's key packages (Fugo).
    pub adds_first: bool,
}

/// Who a commit added and removed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DmMembers {
    pub added: Vec<DmMember>,
    pub removed: Vec<DmMember>,
    /// The accounts (not this device's) whose leaves moved to a new
    /// security key in this commit: the key-changed line goes here, before
    /// anything said under it (Fugo).
    #[serde(default)]
    pub key_changed: Vec<String>,
    /// What was sent here between the last message this device read and
    /// this line was skipped: it joined the group again past it, and
    /// can't read it (row 654, Fugo: never silent). Some(account) when
    /// that account's security key changed meanwhile ("" for any other
    /// cause); None for a line about members.
    #[serde(default)]
    pub skipped: Option<String>,
    /// Leaves back under their id with a new signature key: the device
    /// installed again (row 202). One line each ("Ana set up a device
    /// again: iPhone", Trish); its former install leaving under that id
    /// says nothing.
    #[serde(default)]
    pub set_up_again: Vec<DmMember>,
    /// The first devices of an account that had no leaf here before: the
    /// friend joining ("Ana joined on Pixel", Bruno 33634), still said for
    /// each leaf (Fugo); not in [added].
    #[serde(default)]
    pub joined: Vec<DmMember>,
}

/// What the person's own devices keep of a conversation, through the self
/// group: the draft (and when it was written), how far they've read, and
/// what they deleted.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DmView {
    pub draft: String,
    pub draft_ms: i64,
    pub read_seq: i64,
    pub deleted_seq: i64,
}

/// What one message from the feed was.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DmOutcome {
    /// Said by a member: now in the history ([DmProcessed::message]).
    Message,
    /// A commit: the group is at [DmProcessed::epoch].
    Committed,
    /// A proposal, kept until a commit covers it.
    Proposal,
    /// Processed already (at or before the group's position).
    Already,
    /// Refused by the checks, or not readable here: dropped, with
    /// [DmProcessed::why] (for the log, not the person).
    Dropped,
    /// A commit under a security key this device hasn't pinned for the
    /// account in [DmProcessed::why] (a rotation, dms-e2ee §2.1): left
    /// unread, the position kept, to take once that account's keys are in
    /// (never dropped: every commit after it would fail). Or, with
    /// [DmProcessed::behind], something from an epoch past this device's.
    Held,
}

/// One message from the feed, processed.
#[derive(Debug, Clone)]
pub struct DmProcessed {
    pub outcome: DmOutcome,
    pub message: Option<DmMessage>,
    pub epoch: u64,
    pub why: String,
    /// Dropped, and it was a commit or proposal: this device may have
    /// fallen out of step and should rejoin.
    pub handshake: bool,
    /// Held, and never to be taken here: a commit or proposal from a leaf
    /// under a security key [why]'s account replaced (row 654). This
    /// device rejoins the group past it.
    pub replaced: bool,
    /// Held, from an epoch past this device's (row 551): it missed a commit
    /// the home never carried (its own, taken by the worker just as the
    /// app stopped, say). The worker's log has it ([take_worker_commit]);
    /// then this is taken again. Never passed over unread meanwhile.
    pub behind: bool,
    /// Held: a leaf of [why]'s that the device list taken here doesn't
    /// name (row 561), such as a friend's device added while this one was
    /// away. Their list is fetched and it's taken again; one still unnamed
    /// then is refused as before.
    pub unlisted: bool,
}

impl DmProcessed {
    fn of(outcome: DmOutcome) -> DmProcessed {
        DmProcessed {
            outcome,
            message: None,
            epoch: 0,
            why: String::new(),
            handshake: false,
            replaced: false,
            behind: false,
            unlisted: false,
        }
    }

    fn unlisted(account: String, handshake: bool) -> DmProcessed {
        DmProcessed { why: account, handshake, unlisted: true, ..DmProcessed::of(DmOutcome::Held) }
    }

    fn message(message: DmMessage) -> DmProcessed {
        DmProcessed { message: Some(message), ..DmProcessed::of(DmOutcome::Message) }
    }

    fn committed(epoch: u64) -> DmProcessed {
        DmProcessed { epoch, ..DmProcessed::of(DmOutcome::Committed) }
    }
}

/// What a group without MLS state here does with an item of its feed
/// ([DmDevice::stateless]).
enum Stateless {
    /// From before: passed over.
    Old,
    /// It joins again: dropped, why.
    Out(String),
}

/// One item of a group's feed for [DmDevice::process_window]: its seq,
/// envelope (JSON) and when it arrived.
#[derive(Debug, Clone)]
pub struct DmWindowItem {
    pub seq: i64,
    pub envelope: String,
    pub arrived_ms: i64,
}

/// How a window ended ([DmDevice::process_window]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DmWindowEnd {
    /// Closed by the removal: [DmWindowOutcome::processed] is each item
    /// from the refused commit to the removal, taken; the position is past
    /// the removal.
    Closed,
    /// The items ran out before the removal: nothing changed. Again with
    /// more, while more commits are to come; else refused as before.
    Open,
    /// It can't close ([DmWindowOutcome::why]): nothing changed, and the
    /// commit stays refused.
    Aborted,
    /// No window opens here (not a conversation, or refused for something
    /// else): nothing changed, refused as before.
    None,
}

/// What [DmDevice::process_window] came to.
#[derive(Debug, Clone)]
pub struct DmWindowOutcome {
    pub end: DmWindowEnd,
    pub why: String,
    pub processed: Vec<DmProcessed>,
}

/// A message encrypted here that hasn't come back yet.
#[derive(Debug, Clone)]
pub struct DmUnsent {
    pub group_id: Vec<u8>,
    pub client_message_id: String,
    pub envelope: String,
    pub created_ms: i64,
    /// What it says, to encrypt again as a new message when the group
    /// moved on before it went (409 epoch).
    pub content: DmContent,
}

/// This device's identity and leaf, as the store keeps them (`rows/me`).
#[derive(Serialize, Deserialize)]
struct Kept {
    account_id: String,
    certificate: Vec<u8>,
}

/// A message this device sent, kept until it comes back.
#[derive(Serialize, Deserialize)]
struct Pending {
    content: DmContent,
}

struct State {
    leaf: LeafKey,
    identity: Option<IdentityKey>,
    me: Option<Me>,
    account_id: String,
    pins: Pins,
}

/// The new device's offer while it waits for the hand-off: single use,
/// in memory only.
#[frb(opaque)]
pub struct DmHandoff {
    waiting: Mutex<Option<Waiting>>,
    qr: String,
    comparison: String,
}

impl DmHandoff {
    /// The QR code's text.
    #[frb(sync)]
    pub fn qr(&self) -> String {
        self.qr.clone()
    }

    /// The digits both devices show when the code is pasted rather than
    /// scanned, as "51550 99913 70850 18796".
    #[frb(sync)]
    pub fn comparison(&self) -> String {
        self.comparison.clone()
    }
}

/// This device's direct messages: its leaf, the person's identity once
/// set up, the pins, and its groups over the sealed store.
#[frb(opaque)]
pub struct DmDevice {
    store: Arc<Store>,
    state: Mutex<State>,
    /// History-transfer keys (E8, dm/transfer.rs), in memory only.
    pub(crate) transfers: Arc<crate::dm::transfer::Keys>,
}

const ME: &[&[u8]] = &[b"rows", b"me"];
const PINS: &[&[u8]] = &[b"rows", b"pins"];
/// Set once this device read what the worker still held and went over to
/// its home (#451): the worker's feed is never read again.
const HOME_DRAINED: &[&[u8]] = &[b"rows", b"home_drained"];

/// Where the home's seqs start (#451): above every worker's.
const HOME_SEQ_BASE: i64 = 1 << 32;

/// A group's row for where this device is in the worker's feed once it
/// reads the home ([DmDevice::proposal_seq]).
struct ProposalPlace(Vec<u8>);

impl ProposalPlace {
    fn each(&self) -> [&[u8]; 2] {
        [b"rows", &self.0]
    }
}

fn proposal_place(group_id: &[u8]) -> ProposalPlace {
    ProposalPlace([b"proposal_seq:".as_slice(), group_id].concat())
}

/// A group's row for when this device made the commit to it the worker
/// hasn't taken yet (unix ms; E11-8): one older than [PENDING_FOR] the
/// worker never will ([DmDevice::fork_check]).
fn commit_place(group_id: &[u8]) -> ProposalPlace {
    ProposalPlace([b"commit_at:".as_slice(), group_id].concat())
}

/// A group's row once [DmDevice::fork_check] found its state forked from
/// the worker's (E11-8): nothing changes there, nor is sent, until it
/// joins again ([DmDevice::resync_group]). Whether that's stuck below the
/// floor.
fn forked_place(group_id: &[u8]) -> ProposalPlace {
    ProposalPlace([b"forked:".as_slice(), group_id].concat())
}

/// How long a commit of this device's may wait for the worker before it's
/// taken as never to be taken (E11-8): the worker answers in seconds.
const PENDING_FOR: i64 = 2 * 60 * 1000;

fn now_ms() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|now| now.as_millis() as i64).unwrap_or(0)
}

pub(crate) fn place<'a>(table: &'a [u8], group: &'a [u8], id: &'a [u8], column: &'a [u8]) -> [&'a [u8]; 4] {
    [table, group, id, column]
}

fn sql(error: rusqlite::Error) -> Error {
    Error::Storage(error.to_string())
}

fn json<T: Serialize>(value: &T) -> Result<Vec<u8>, Error> {
    serde_json::to_vec(value).map_err(|error| Error::Format(error.to_string()))
}

fn envelope_json(bytes: Vec<u8>) -> anyhow::Result<String> {
    Ok(Envelope::new(bytes)?.to_json().to_string())
}

fn read_envelope(text: &str) -> anyhow::Result<Envelope> {
    let value: serde_json::Value = serde_json::from_str(text).context("an envelope is JSON")?;
    Ok(Envelope::from_json(&value)?)
}

/// Whether `error` means the message itself is bad (drop it) rather than
/// this device (try again).
fn refusal(error: &Error) -> bool {
    !matches!(error, Error::Storage(_) | Error::Sealed)
}

impl DmDevice {
    /// Opens the store at [path] with its seal key [store_key], as the leaf
    /// [leaf_key], with the person's identity [identity_key] once this device
    /// has it. None when the store was made with another seal key: start
    /// over ([dm_remove_store]).
    pub fn open(path: String, store_key: Vec<u8>, leaf_key: Vec<u8>, identity_key: Option<Vec<u8>>) -> anyhow::Result<Option<DmDevice>> {
        let store = match Store::open(&PathBuf::from(path), bytes32(&store_key)?)? {
            Opened::Ready(store) => store,
            Opened::OtherKey => return Ok(None),
        };
        let leaf = LeafKey::from_secret(&bytes32(&leaf_key)?);
        let identity = identity_key.map(|key| bytes32(&key).map(|key| IdentityKey::from_secret(&key))).transpose()?;
        let (kept, pins) = store.with_tx(|_, tx| -> Result<_, Error> {
            let row = |place: &[&[u8]]| -> Result<Option<Zeroizing<Vec<u8>>>, Error> {
                let sealed: Option<Vec<u8>> =
                    tx.query_row("SELECT value FROM rows WHERE name = ?1", [place[1]], |row| row.get(0)).optional().map_err(sql)?;
                sealed.map(|sealed| store.open_row(place, &sealed)).transpose()
            };
            let kept: Option<Kept> =
                row(ME)?.map(|bytes| serde_json::from_slice(&bytes)).transpose().map_err(|e| Error::Format(e.to_string()))?;
            let mut pins: Pins = row(PINS)?
                .map(|bytes| serde_json::from_slice(&bytes))
                .transpose()
                .map_err(|e| Error::Format(e.to_string()))?
                .unwrap_or_default();
            pins.reparse()?;
            Ok((kept, pins))
        })?;
        let mut state = State { leaf, identity, me: None, account_id: String::new(), pins };
        if let Some(kept) = kept {
            let certificate = LeafCertificate::from_bytes(&kept.certificate)?;
            state.me = Some(Me::new(LeafKey::from_secret(&state.leaf.secret()), certificate)?);
            state.account_id = kept.account_id;
        }
        Ok(Some(DmDevice { store: Arc::new(store), state: Mutex::new(state), transfers: Arc::default() }))
    }

    /// The sealed store, for history transfer (E8).
    pub(crate) fn store(&self) -> Arc<Store> {
        self.store.clone()
    }

    fn state(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn save_row(&self, tx: &Transaction<'_>, place: &[&[u8]], value: &[u8]) -> Result<(), Error> {
        let sealed = self.store.seal_row(place, value)?;
        tx.execute("INSERT OR REPLACE INTO rows (name, value) VALUES (?1, ?2)", params![place[1], sealed]).map_err(sql)?;
        Ok(())
    }

    /// Keeps [pins] in place of [before]; the person's devices ([account]'s
    /// leaves) its list no longer names are kept as removed, so a code for
    /// one is never taken again ([DmDevice::seal_handoff]).
    fn save_pins(&self, tx: &Transaction<'_>, account: &str, before: &Pins, pins: &Pins) -> Result<(), Error> {
        let listed = |pins: &Pins| -> Vec<String> {
            pins.get(account)
                .and_then(|pin| pin.device_list.as_ref())
                .and_then(|bytes| SignedDeviceList::from_bytes(bytes).ok())
                .map(|signed| signed.list.leaves.into_iter().map(|leaf| leaf.leaf_id).collect())
                .unwrap_or_default()
        };
        let now = listed(pins);
        let gone: Vec<String> = listed(before).into_iter().filter(|leaf| !now.contains(leaf)).collect();
        self.note_removed(tx, &gone)?;
        // Each list taken, by version (E11-8): which one this device sent
        // on is told after the fact.
        let adopted: Vec<DmListAdopted> = pins
            .accounts()
            .filter_map(|account| {
                let version = pins.list_version(account)?;
                (before.list_version(account) != Some(version)).then(|| DmListAdopted { account: account.to_owned(), version, at_ms: now_ms() })
            })
            .collect();
        if !adopted.is_empty() {
            let mut log: Vec<DmListAdopted> = self.read_row(tx, LISTS_ADOPTED)?.unwrap_or_default();
            log.extend(adopted);
            let over = log.len().saturating_sub(LISTS_ADOPTED_KEPT);
            log.drain(..over);
            self.save_row(tx, LISTS_ADOPTED, &json(&log)?)?;
        }
        self.save_row(tx, PINS, &Zeroizing::new(json(pins)?))
    }

    /// Adds [leaves] to the person's devices removed here or off a list
    /// this device took: their codes are refused from then on.
    fn note_removed(&self, tx: &Transaction<'_>, leaves: &[String]) -> Result<(), Error> {
        if leaves.is_empty() {
            return Ok(());
        }
        let mut removed: Vec<String> = self.read_row(tx, REMOVED)?.unwrap_or_default();
        let before = removed.len();
        for leaf in leaves {
            if !removed.contains(leaf) {
                removed.push(leaf.clone());
            }
        }
        if removed.len() == before {
            return Ok(());
        }
        self.save_row(tx, REMOVED, &json(&removed)?)
    }

    #[frb(sync)]
    pub fn status(&self) -> DmStatus {
        let state = self.state();
        let leaf = state.me.as_ref().map(|me| &me.certificate.leaf);
        DmStatus {
            set_up: state.me.is_some() && state.identity.is_some(),
            account_id: state.account_id.clone(),
            leaf_id: leaf.map(|leaf| leaf.leaf_id.clone()).unwrap_or_default(),
            name: leaf.map(|leaf| leaf.name.clone()).unwrap_or_default(),
            leaf_public: state.leaf.public().to_vec(),
            identity_public: state.identity.as_ref().map(|identity| identity.public().0.to_vec()),
        }
    }

    /// Certifies this device with the identity, pins the person's own
    /// identity and accepts their device list: for the first device (a new
    /// identity) and for the one the hand-off just brought it.
    fn become_leaf(
        &self,
        state: &mut State,
        identity: IdentityKey,
        account_id: &str,
        leaf_id: &str,
        name: &str,
        now_ms: u64,
    ) -> anyhow::Result<Vec<u8>> {
        let leaf = Leaf {
            kind: LeafKind::Device,
            account_id: account_id.to_owned(),
            leaf_id: leaf_id.to_owned(),
            signature_key: state.leaf.public(),
            name: name.to_owned(),
            created_ms: now_ms,
        };
        let certificate = LeafCertificate::issue(&identity, leaf)?;
        let bytes = certificate.to_bytes()?;
        let me = Me::new(LeafKey::from_secret(&state.leaf.secret()), certificate)?;
        let mut pins = state.pins.clone();
        pins.first_contact(account_id, identity.public())?;
        let kept = Kept { account_id: account_id.to_owned(), certificate: bytes.clone() };
        self.store.with_tx(|_, tx| -> Result<(), Error> {
            self.save_row(tx, ME, &json(&kept)?)?;
            self.save_pins(tx, account_id, &state.pins, &pins)
        })?;
        state.pins = pins;
        state.me = Some(me);
        state.identity = Some(identity);
        state.account_id = account_id.to_owned();
        Ok(bytes)
    }

    /// Sets up the person's first device: [identity_key] is a new identity
    /// (kept in the platform key store first), this device is
    /// [leaf_id] ("its account key") called [name], and the device list
    /// names it alone, version 1.
    pub fn set_up_first(
        &self,
        identity_key: Vec<u8>,
        account_id: String,
        leaf_id: String,
        name: String,
        now_ms: u64,
    ) -> anyhow::Result<DmSetUp> {
        self.set_up(identity_key, None, account_id, leaf_id, name, now_ms)
    }

    /// No other device (dms-e2ee §2, "All devices lost"; row 630): sets
    /// up this device as [set_up_first] does, under a new identity
    /// [identity_key] that replaces the account's [old_identity_public]
    /// (as the worker serves it), held as retired here from the start, as
    /// a key change would leave it (Ghiaccio): leaves under the old key
    /// are the dead ones, and a Claktory enrolled under it can be enrolled
    /// again. [replaced_identity_keys]: the keys the account replaced
    /// before that one, oldest first (GET keys' list), retired too, so
    /// leaves and Claktories under any of them are known after more than
    /// one start-over (row 654; Fugo: only here, for the person's own
    /// account, and refused whole if the list names the new key).
    #[allow(clippy::too_many_arguments)]
    pub fn set_up_new_key(
        &self,
        identity_key: Vec<u8>,
        old_identity_public: Vec<u8>,
        replaced_identity_keys: Vec<Vec<u8>>,
        account_id: String,
        leaf_id: String,
        name: String,
        now_ms: u64,
    ) -> anyhow::Result<DmSetUp> {
        let mut keys = replaced_identity_keys;
        keys.push(old_identity_public);
        self.set_up(identity_key, Some(keys), account_id, leaf_id, name, now_ms)
    }

    fn set_up(
        &self,
        identity_key: Vec<u8>,
        old_identity_publics: Option<Vec<Vec<u8>>>,
        account_id: String,
        leaf_id: String,
        name: String,
        now_ms: u64,
    ) -> anyhow::Result<DmSetUp> {
        let mut state = self.state();
        if state.me.is_some() {
            bail!("this device is set up already");
        }
        let identity = IdentityKey::from_secret(&bytes32(&identity_key)?);
        if let Some(old) = old_identity_publics {
            // Oldest first, the one the worker serves now last: each retired
            // by the next, as key changes would have left them.
            let old = old.iter().map(|key| IdentityPublic::from_slice(key)).collect::<Result<Vec<_>, _>>()?;
            if old.contains(&identity.public()) {
                bail!("that's the key the account has");
            }
            let Some((first, rest)) = old.split_first() else { bail!("no key to replace") };
            let mut pins = state.pins.clone();
            pins.first_contact(&account_id, *first)?;
            for key in rest {
                pins.key_changed(&account_id, *key)?;
            }
            pins.key_changed(&account_id, identity.public())?;
            // The person's own start-over: what it replaced, for good (L5).
            pins.retire_previous(&account_id);
            pins.seen(&account_id);
            // Kept with the leaf below, in one transaction.
            state.pins = pins;
        }
        let public = identity.public().0.to_vec();
        let certificate = self.become_leaf(&mut state, identity, &account_id, &leaf_id, &name, now_ms)?;
        let listed = DmListed { kind: DmLeafKind::Device, leaf_id, signature_key: state.leaf.public().to_vec() };
        drop(state);
        let device_list = self.sign_device_list(1, now_ms, vec![listed])?;
        // The first list, this device's own: pinned at once (nothing on
        // the worker to step out of line with; a refused setup starts over).
        self.accept_list(account_id, device_list.clone())?;
        Ok(DmSetUp { identity_public: public, certificate, device_list })
    }

    /// Starts adding this device, [leaf_id] (its account key) called
    /// [name] on [platform], reachable for history at the iroh endpoint
    /// [endpoint_id], to the person's: shows the returned hand-off's QR code
    /// to one of their devices that's set up.
    pub fn start_handoff(&self, leaf_id: String, name: String, platform: String, endpoint_id: Vec<u8>) -> anyhow::Result<DmHandoff> {
        let waiting = Waiting::new(OfferFor {
            leaf_signature_key: self.state().leaf.public(),
            leaf_id,
            name,
            platform,
            endpoint_id: bytes32(&endpoint_id)?,
        })?;
        let qr = waiting.offer.qr();
        let comparison = waiting.offer.comparison();
        Ok(DmHandoff { waiting: Mutex::new(Some(waiting)), qr, comparison })
    }

    /// On the device that scanned [qr]: the person's identity and pins,
    /// sealed for the new device only, and its leaf key for the device list.
    pub fn seal_handoff(&self, qr: String) -> anyhow::Result<DmHandoffSealed> {
        let state = self.state();
        let identity = state.identity.as_ref().ok_or_else(|| anyhow!("this device isn't set up"))?;
        let offer = Offer::from_qr(&qr)?;
        // A code works once, and never for a device the person removed
        // (Abbacchio's row 612: an old code put a locked-out device back
        // on the list). Checked before the person compares the numbers.
        let comparison = offer.comparison();
        let (removed, used) = self.store.with_tx(|_, tx| -> Result<(Vec<String>, Vec<String>), Error> {
            Ok((self.read_row(tx, REMOVED)?.unwrap_or_default(), self.read_row(tx, USED_CODES)?.unwrap_or_default()))
        })?;
        if removed.contains(&offer.device.leaf_id) || used.contains(&comparison) {
            bail!(CODE_USED);
        }
        let sealed = handoff::seal(&offer, &state.account_id, identity, &state.pins)?;
        let device = &offer.device;
        // The history-transfer key (E8), for the endpoint this QR code named
        // only, for 30 minutes.
        self.transfers.keep_sending(&device.leaf_id, device.endpoint_id, &sealed.transfer_key);
        Ok(DmHandoffSealed {
            sealed: sealed.bytes.clone(),
            leaf_signature_key: device.leaf_signature_key.to_vec(),
            leaf_id: device.leaf_id.clone(),
            name: device.name.clone(),
            platform: device.platform.clone(),
            endpoint_id: device.endpoint_id.to_vec(),
            comparison,
        })
    }

    /// The code whose digits are [comparison] was handed off to: it's
    /// refused from now on.
    pub fn code_used(&self, comparison: String) -> anyhow::Result<()> {
        let _state = self.state();
        self.store.with_tx(|_, tx| -> Result<(), Error> {
            let mut used: Vec<String> = self.read_row(tx, USED_CODES)?.unwrap_or_default();
            if used.contains(&comparison) {
                return Ok(());
            }
            used.push(comparison);
            let over = used.len().saturating_sub(USED_CODES_KEPT);
            used.drain(..over);
            self.save_row(tx, USED_CODES, &json(&used)?)
        })?;
        Ok(())
    }

    /// On the new device: opens what the scanning device sent ([sealed])
    /// and becomes the leaf its QR code named, only when what it gives is
    /// [account_id]'s identity as their account publishes it
    /// ([identity_public], from the worker's keys): knowing the QR code's
    /// secret alone (a photo of it, the clipboard) never plants another
    /// identity and pins on this device (Melone F3). Either way the offer
    /// is spent. The identity's secret comes back for the platform key
    /// store.
    pub fn finish_handoff_for(
        &self,
        handoff: &DmHandoff,
        sealed: Vec<u8>,
        now_ms: u64,
        account_id: String,
        identity_public: Vec<u8>,
    ) -> anyhow::Result<DmHandoffDone> {
        self.open_handoff(handoff, sealed, now_ms, Some((&account_id, &identity_public)))
    }

    /// [finish_handoff_for] with nothing to compare it with: tests of what
    /// comes after.
    #[cfg(test)]
    pub(crate) fn finish_handoff(&self, handoff: &DmHandoff, sealed: Vec<u8>, now_ms: u64) -> anyhow::Result<DmHandoffDone> {
        self.open_handoff(handoff, sealed, now_ms, None)
    }

    fn open_handoff(
        &self,
        handoff: &DmHandoff,
        sealed: Vec<u8>,
        now_ms: u64,
        expected: Option<(&str, &[u8])>,
    ) -> anyhow::Result<DmHandoffDone> {
        let waiting = handoff
            .waiting
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take()
            .ok_or_else(|| anyhow!("this hand-off was used already"))?;
        let (leaf_id, name) = (waiting.offer.device.leaf_id.clone(), waiting.offer.device.name.clone());
        let received = waiting.open(&sealed)?;
        if let Some((account_id, identity_public)) = expected
            && (received.account_id != account_id || received.identity.public().0.as_slice() != identity_public)
        {
            bail!("this hand-off isn't from the person's account as it's published");
        }
        let mut state = self.state();
        // The history-transfer key (E8), for 30 minutes.
        self.transfers.keep_receiving(&received.transfer_key);
        if state.me.is_some() {
            bail!("this device is set up already");
        }
        let mut pins = state.pins.clone();
        pins.import(&received.pins)?;
        state.pins = pins;
        let identity_key = received.identity.secret().to_vec();
        let identity_public = received.identity.public().0.to_vec();
        let certificate = self.become_leaf(&mut state, received.identity, &received.account_id, &leaf_id, &name, now_ms)?;
        Ok(DmHandoffDone { account_id: received.account_id, identity_key, identity_public, certificate })
    }

    /// A certificate for one of the person's leaves (a Claktory enrolling,
    /// H2), signed by their identity.
    pub fn issue_certificate(
        &self,
        kind: DmLeafKind,
        leaf_id: String,
        signature_key: Vec<u8>,
        name: String,
        created_ms: u64,
    ) -> anyhow::Result<Vec<u8>> {
        let state = self.state();
        let identity = state.identity.as_ref().ok_or_else(|| anyhow!("this device isn't set up"))?;
        let leaf = Leaf {
            kind: kind.into(),
            account_id: state.account_id.clone(),
            leaf_id,
            signature_key: bytes32(&signature_key)?,
            name,
            created_ms,
        };
        Ok(LeafCertificate::issue(identity, leaf)?.to_bytes()?)
    }

    /// The person's device list at [version] (one more than the last),
    /// signed. Not pinned here: that's [accept_list] once the worker took
    /// it, so a list it refused (or never heard) never counts as seen, and
    /// the next one another device puts at that version is taken
    /// (Abbacchio's row 615).
    pub fn sign_device_list(&self, version: u64, created_ms: u64, leaves: Vec<DmListed>) -> anyhow::Result<Vec<u8>> {
        self.sign_device_list_with_home(version, created_ms, leaves, None)
    }

    /// [sign_device_list], naming [home] (an iroh endpoint id, 32 bytes) as
    /// the person's home Claktory (#451); None for none.
    pub fn sign_device_list_with_home(
        &self,
        version: u64,
        created_ms: u64,
        leaves: Vec<DmListed>,
        home: Option<Vec<u8>>,
    ) -> anyhow::Result<Vec<u8>> {
        let home = home.map(|home| bytes32(&home)).transpose()?;
        let state = self.state();
        let identity = state.identity.as_ref().ok_or_else(|| anyhow!("this device isn't set up"))?;
        let leaves = leaves
            .into_iter()
            .map(|leaf| Ok(Listed { kind: leaf.kind.into(), leaf_id: leaf.leaf_id, signature_key: bytes32(&leaf.signature_key)? }))
            .collect::<anyhow::Result<Vec<_>>>()?;
        let list = DeviceList { account_id: state.account_id.clone(), version, created_ms, leaves };
        Ok(list.sign_with(identity, home)?.to_bytes()?)
    }

    fn change_pins(&self, state: &mut State, change: impl FnOnce(&mut Pins) -> Result<(), Error>) -> anyhow::Result<()> {
        let mut pins = state.pins.clone();
        change(&mut pins)?;
        self.store.with_tx(|_, tx| self.save_pins(tx, &state.account_id, &state.pins, &pins))?;
        state.pins = pins;
        Ok(())
    }

    /// Pins [account]'s identity at first contact; a different key than
    /// the one pinned is refused (that's [key_changed]).
    pub fn first_contact(&self, account: String, identity_public: Vec<u8>) -> anyhow::Result<()> {
        let identity = IdentityPublic::from_slice(&identity_public)?;
        self.change_pins(&mut self.state(), |pins| pins.first_contact(&account, identity))
    }

    /// The commit or proposal [envelope] held in [group_id] proves that
    /// [account] replaced the key its leaf is under, which the worker lists
    /// in [listed] (GET keys' replaced_identity_keys, with [served] its
    /// current key): that key is retired here, so the commit is known as
    /// one this device can never take (row 654). Only what a held commit
    /// proves, and nothing when the list names a current key (Fugo: no
    /// wedge against the friend's next key). Whether a key was retired.
    pub fn retire_replaced(
        &self,
        group_id: Vec<u8>,
        envelope: String,
        account: String,
        served: Vec<u8>,
        listed: Vec<Vec<u8>>,
    ) -> anyhow::Result<bool> {
        let served = IdentityPublic::from_slice(&served)?.0;
        let listed = listed.iter().map(|key| IdentityPublic::from_slice(key).map(|key| key.0)).collect::<Result<Vec<_>, _>>()?;
        let envelope = read_envelope(&envelope)?;
        let mut state = self.state();
        let signer = self.store.with_tx(|provider, tx| {
            let (mut group, _) = Self::load(provider, tx, &group_id)?;
            group.held_signer(provider, envelope.bytes())
        })?;
        let Some(signer) = signer else { return Ok(false) };
        let mut retired = false;
        self.change_pins(&mut state, |pins| {
            retired = pins.replaced(&account, served, &listed, &signer)?;
            Ok(())
        })?;
        Ok(retired)
    }

    /// [account]'s security key changed: sending waits until [seen].
    pub fn key_changed(&self, account: String, identity_public: Vec<u8>) -> anyhow::Result<()> {
        let identity = IdentityPublic::from_slice(&identity_public)?;
        let mut state = self.state();
        // The person's own key changes only through their self group
        // (take_rotation), never because the worker serves another.
        if account == state.account_id {
            bail!("the person's own security key changes only through their devices");
        }
        self.change_pins(&mut state, |pins| pins.key_changed(&account, identity))
    }

    /// Takes [account]'s signed device list, as the worker serves it.
    pub fn accept_list(&self, account: String, device_list: Vec<u8>) -> anyhow::Result<()> {
        self.change_pins(&mut self.state(), |pins| pins.accept_list(&account, &device_list))
    }

    /// The person saw that [account]'s key changed.
    pub fn seen(&self, account: String) -> anyhow::Result<()> {
        self.change_pins(&mut self.state(), |pins| {
            pins.seen(&account);
            Ok(())
        })
    }

    /// The person compared safety numbers with [account] after its key
    /// changed and they didn't match (E11-3): the key it changed from
    /// ([back_to], the pin's `changed_from`) is pinned again, its list,
    /// version and verified flag with it, and the one that didn't match
    /// can come back only as another change, which says so.
    pub fn undo_key_change(&self, account: String, back_to: Vec<u8>) -> anyhow::Result<()> {
        let back_to = IdentityPublic::from_slice(&back_to)?;
        if account == self.state().account_id {
            bail!("the person's own security key changes only through their devices");
        }
        self.change_pins(&mut self.state(), |pins| pins.undo_change(&account, &back_to))
    }

    /// The person compared safety numbers with [account], for the key
    /// pinned now.
    pub fn verify(&self, account: String) -> anyhow::Result<()> {
        let mut state = self.state();
        let identity = state.pins.get(&account).map(|pin| pin.identity).ok_or_else(|| anyhow!("nothing pinned for {account}"))?;
        self.change_pins(&mut state, |pins| pins.verified(&account, &identity))
    }

    /// [account]'s device list as this device last took it (checked then):
    /// to add or remove one of the person's own leaves, the next version
    /// lists these with the change.
    #[frb(sync)]
    pub fn device_list(&self, account: String) -> anyhow::Result<Option<DmDeviceList>> {
        let state = self.state();
        let Some(bytes) = state.pins.get(&account).and_then(|pin| pin.device_list.as_ref()) else { return Ok(None) };
        let signed = SignedDeviceList::from_bytes(bytes)?;
        Ok(Some(DmDeviceList {
            version: signed.list.version,
            created_ms: signed.list.created_ms,
            leaves: signed
                .list
                .leaves
                .iter()
                .map(|leaf| DmListed { kind: leaf.kind.into(), leaf_id: leaf.leaf_id.clone(), signature_key: leaf.signature_key.to_vec() })
                .collect(),
            home: signed.home.map(|home| home.to_vec()),
        }))
    }

    /// Whether this device went over to its home (#451): it read what the
    /// worker still held, once, and reads the home only since.
    #[frb(sync)]
    pub fn home_drained(&self) -> anyhow::Result<bool> {
        let _state = self.state();
        let sealed: Option<Vec<u8>> = self.store.with_tx(|_, tx| {
            tx.query_row("SELECT value FROM rows WHERE name = ?1", [HOME_DRAINED[1]], |row| row.get(0)).optional().map_err(sql)
        })?;
        Ok(sealed.is_some())
    }

    /// Marks this device as gone over to its home ([home_drained]).
    pub fn set_home_drained(&self) -> anyhow::Result<()> {
        let _state = self.state();
        self.store.with_tx(|_, tx| self.save_row(tx, HOME_DRAINED, b"1"))?;
        Ok(())
    }

    #[frb(sync)]
    pub fn pin(&self, account: String) -> Option<DmPin> {
        let state = self.state();
        let pin = state.pins.get(&account)?;
        Some(DmPin {
            identity_public: pin.identity.0.to_vec(),
            verified: pin.verified,
            changed_unseen: pin.changed_unseen,
            can_send: state.pins.can_send(&account),
            device_list_version: pin.highest_version,
            changed_from: pin.changed_from.map(|key| key.0.to_vec()),
            mismatched: pin.mismatched,
        })
    }

    /// The pins as JSON, for the person's other devices (the self group).
    pub fn export_pins(&self) -> anyhow::Result<Vec<u8>> {
        Ok(json(&self.state().pins)?)
    }

    /// Pins from another of the person's own devices (never the service).
    pub fn import_pins(&self, exported: Vec<u8>) -> anyhow::Result<()> {
        let mut theirs: Pins = serde_json::from_slice(&exported).context("pins")?;
        theirs.reparse()?;
        self.change_pins(&mut self.state(), |pins| pins.import(&theirs))
    }

    /// The safety number with [account], for the key pinned now.
    #[frb(sync)]
    pub fn safety_number(&self, account: String) -> anyhow::Result<DmSafety> {
        let number = self.safety(&account)?;
        Ok(DmSafety { digits: number.digits(), qr: number.qr() })
    }

    /// Whether [account]'s scanned QR code shows the same two keys.
    #[frb(sync)]
    pub fn safety_matches(&self, account: String, scanned: Vec<u8>) -> anyhow::Result<bool> {
        Ok(self.safety(&account)?.matches_scanned(&scanned)?)
    }

    fn safety(&self, account: &str) -> anyhow::Result<SafetyNumber> {
        let state = self.state();
        let ours = state.identity.as_ref().ok_or_else(|| anyhow!("this device isn't set up"))?.public();
        let theirs = state.pins.get(account).ok_or_else(|| anyhow!("nothing pinned for {account}"))?.identity;
        Ok(SafetyNumber::new((&ours, &state.account_id), (&theirs, account)))
    }

    /// [count] one-time key packages for the worker's supply.
    pub fn key_packages(&self, count: u32) -> anyhow::Result<Vec<String>> {
        let state = self.state();
        let me = state.me.as_ref().ok_or_else(|| anyhow!("this device isn't set up"))?;
        let packages = self.store.with_tx(|provider, _| (0..count).map(|_| me.key_package(provider)).collect::<Result<Vec<_>, Error>>())?;
        packages.into_iter().map(envelope_json).collect()
    }

    /// The last-resort key package, which the worker hands out again once
    /// the one-time ones are gone.
    pub fn last_resort_key_package(&self) -> anyhow::Result<String> {
        let state = self.state();
        let me = state.me.as_ref().ok_or_else(|| anyhow!("this device isn't set up"))?;
        envelope_json(self.store.with_tx(|provider, _| me.last_resort_key_package(provider))?)
    }

    /// Whether the last resort the worker keeps for this device, whose
    /// lifetime ends at [last_resort_ends] (seconds since the epoch, the
    /// worker's GET key-packages; 0 when it doesn't say), is close enough
    /// to its end at [now_secs] to be replaced now (Melone's F4).
    #[frb(sync)]
    pub fn last_resort_due(&self, last_resort_ends: i64, now_secs: i64) -> bool {
        last_resort_ends != 0 && last_resort_ends <= now_secs.saturating_add(claktory_dm_crypto::group::LAST_RESORT_RENEW as i64)
    }

    /// Deletes this device's key packages whose lifetime ended by
    /// [now_secs], their private keys with them (Melone's F4): one never
    /// used, or a last resort replaced since, opens no Welcome after. How
    /// many went.
    pub fn sweep_key_packages(&self, now_secs: i64) -> anyhow::Result<u32> {
        let now = u64::try_from(now_secs).map_err(|_| anyhow!("a time before 1970"))?;
        let gone = self.store.with_tx(|provider, _| provider.sweep_key_packages(now))?;
        Ok(u32::try_from(gone).unwrap_or(u32::MAX))
    }

    fn group_row(tx: &Transaction<'_>, group_id: &[u8]) -> Result<Option<(DmGroupKind, i64)>, Error> {
        let row: Option<(String, i64)> = tx
            .query_row("SELECT kind, seq FROM groups WHERE id = ?1", [group_id], |row| Ok((row.get(0)?, row.get(1)?)))
            .optional()
            .map_err(sql)?;
        row.map(|(kind, seq)| Ok((serde_json::from_str(&kind).map_err(|e| Error::Format(e.to_string()))?, seq))).transpose()
    }

    fn load(provider: &Provider<'_>, tx: &Transaction<'_>, group_id: &[u8]) -> Result<(Group, i64), Error> {
        let (kind, seq) = Self::group_row(tx, group_id)?.ok_or_else(|| Error::Storage("a group this device isn't in".to_owned()))?;
        let group =
            Group::load(provider, group_kind(&kind)?, group_id)?.ok_or_else(|| Error::Storage("a group whose state is gone".to_owned()))?;
        Ok((group, seq))
    }

    fn add_group(tx: &Transaction<'_>, group_id: &[u8], kind: &DmGroupKind, seq: i64) -> Result<(), Error> {
        let kind = serde_json::to_string(kind).map_err(|e| Error::Format(e.to_string()))?;
        // Never REPLACE: that deletes the row, and its history with it.
        tx.execute(
            "INSERT INTO groups (id, kind, seq) VALUES (?1, ?2, ?3) \
             ON CONFLICT (id) DO UPDATE SET kind = excluded.kind, seq = excluded.seq",
            params![group_id, kind, seq],
        )
        .map_err(sql)?;
        // History another device sent before this one joined (E8).
        crate::dm::transfer::adopt(tx, group_id).map_err(sql)
    }

    /// Starts the group [group_id] of [kind], with only this device in it.
    /// Gives its GroupInfo at epoch 0 (an envelope's JSON), for the worker.
    pub fn create_group(&self, group_id: Vec<u8>, kind: DmGroupKind) -> anyhow::Result<String> {
        let state = self.state();
        let me = state.me.as_ref().ok_or_else(|| anyhow!("this device isn't set up"))?;
        let info = self.store.with_tx(|provider, tx| {
            let group = Group::create(provider, me, group_kind(&kind)?, &group_id)?;
            Self::add_group(tx, &group_id, &kind, 0)?;
            group.group_info(provider, me)
        })?;
        envelope_json(info)
    }

    /// Joins [group_id] of [kind] from a [welcome] (an envelope's JSON);
    /// the feed goes on after [seq].
    pub fn join_group(&self, group_id: Vec<u8>, kind: DmGroupKind, welcome: String, seq: i64) -> anyhow::Result<()> {
        let welcome = read_envelope(&welcome)?;
        let state = self.state();
        self.store.with_tx(|provider, tx| {
            Group::join(provider, &state.pins, group_kind(&kind)?, &group_id, welcome.bytes())?;
            Self::add_group(tx, &group_id, &kind, seq)
        })?;
        Ok(())
    }

    /// A line in [group_id]'s history at [seq] (the first one this device
    /// didn't read): what was sent from there until it joined again was
    /// skipped, because [because]'s security key changed ("" for another
    /// cause). Row 654: a device that can't read messages says so.
    pub fn note_skipped(&self, group_id: Vec<u8>, seq: i64, arrived_ms: i64, because: String) -> anyhow::Result<()> {
        let _state = self.state();
        let message = DmMessage {
            seq,
            mine: false,
            from_account: String::new(),
            from_leaf: String::new(),
            from_kind_device: false,
            content: DmContent {
                kind: String::new(),
                client_message_id: String::new(),
                text: None,
                by_agent: None,
                extra_json: String::new(),
            },
            arrived_ms,
            members: Some(DmMembers {
                added: vec![],
                removed: vec![],
                key_changed: vec![],
                skipped: Some(because),
                set_up_again: vec![],
                joined: vec![],
            }),
            departed: false,
        };
        self.store.with_tx(|_, tx| self.keep(tx, &group_id, &message))?;
        Ok(())
    }

    /// Joins [group_id] again from the worker's GroupInfo ([group_info],
    /// an envelope's JSON), with an external commit for the worker; the
    /// feed goes on after [seq]. For a device whose state broke or that
    /// was away too long.
    pub fn rejoin_group(&self, group_id: Vec<u8>, kind: DmGroupKind, group_info: String, seq: i64) -> anyhow::Result<DmCommit> {
        let info = read_envelope(&group_info)?;
        let state = self.state();
        let me = state.me.as_ref().ok_or_else(|| anyhow!("this device isn't set up"))?;
        let out = self.store.with_tx(|provider, tx| {
            claktory_dm_crypto::storage::forget_group(tx, &openmls_group_id(&group_id)).map_err(|e| Error::Storage(e.to_string()))?;
            let floor = claktory_dm_crypto::storage::epoch_floor(tx, &group_id)?;
            let (_, out) = Group::rejoin(provider, me, &state.pins, group_kind(&kind)?, &group_id, info.bytes(), floor)?;
            // OpenMLS stored the new epoch as it made the commit: aside
            // until the worker takes it, so a commit it never takes leaves no
            // epoch of this device's own, nor its floor raised (E11-8).
            claktory_dm_crypto::storage::stash_rejoin(tx, &group_id, floor, &out.commit)?;
            Self::add_group(tx, &group_id, &kind, seq)?;
            self.pending_commit(tx, &group_id, &out.commit)?;
            Ok::<_, Error>(out)
        })?;
        commit(out)
    }

    fn pending_commit(&self, tx: &Transaction<'_>, group_id: &[u8], commit: &[u8]) -> Result<(), Error> {
        tx.execute("INSERT OR REPLACE INTO commits (grp, digest) VALUES (?1, ?2)", params![group_id, envelope::digest(commit).to_vec()])
            .map_err(sql)?;
        self.save_row(tx, &commit_place(group_id).each(), &json(&now_ms())?)
    }

    /// This device's commit to [group_id] is settled (taken, refused, or
    /// another came first): no longer waited for. Its digest, if one
    /// waited.
    fn settle(&self, tx: &Transaction<'_>, group_id: &[u8]) -> Result<Option<Vec<u8>>, Error> {
        let digest: Option<Vec<u8>> =
            tx.query_row("SELECT digest FROM commits WHERE grp = ?1", [group_id], |row| row.get(0)).optional().map_err(sql)?;
        tx.execute("DELETE FROM commits WHERE grp = ?1", [group_id]).map_err(sql)?;
        Self::delete_row(tx, &commit_place(group_id).each())?;
        Ok(digest)
    }

    /// This device's commit to [group_id] will never be taken: its state
    /// goes back to the epoch before (a rejoin's, put aside, is dropped,
    /// leaving the group without one here, to join again).
    fn give_up(&self, provider: &Provider<'_>, tx: &Transaction<'_>, group_id: &[u8]) -> Result<(), Error> {
        self.settle(tx, group_id)?;
        if claktory_dm_crypto::storage::drop_rejoin(tx, group_id)? {
            return Ok(());
        }
        match Self::load(provider, tx, group_id) {
            Ok((mut group, _)) => group.clear_pending(provider),
            Err(_) => Ok(()),
        }
    }

    /// A commit of this device's to [group_id], unless its state is
    /// forked or gone or one of its commits waits (E11-8). None when
    /// [change] has nothing to commit.
    fn change_group(
        &self,
        group_id: &[u8],
        change: impl FnOnce(&mut Group, &Provider<'_>, &Me, &Pins) -> Result<Option<claktory_dm_crypto::group::Outgoing>, Error>,
    ) -> Result<Option<DmCommit>, DmGroupError> {
        let state = self.state();
        let me = state.me.as_ref().ok_or_else(|| anyhow!("this device isn't set up"))?;
        let out = self.store.with_tx(|provider, tx| {
            self.guard(provider, tx, group_id, true)?;
            let (mut group, _) = Self::load(provider, tx, group_id)?;
            let Some(out) = change(&mut group, provider, me, &state.pins)? else { return Ok::<_, DmGroupError>(None) };
            self.pending_commit(tx, group_id, &out.commit)?;
            Ok(Some(out))
        })?;
        Ok(out.map(commit).transpose()?)
    }

    /// Refuses a change to [group_id] ([commit]) or a send while its state
    /// here is forked from the worker's ([DmDevice::fork_check]) or gone, or
    /// (a commit, or with no state) while a commit of its own waits for the
    /// worker: loudly, never as if it went (E11-8).
    fn guard(&self, provider: &Provider<'_>, tx: &Transaction<'_>, group_id: &[u8], commit: bool) -> Result<(), DmGroupError> {
        if let Some(forked) = self.read_row::<Forked>(tx, &forked_place(group_id).each())? {
            return Err(DmGroupError {
                stuck: forked.stuck,
                ..DmGroupError::of(DmGroupErrorKind::Forked, group_id, "this device is out of step with the group: it joins again first")
            });
        }
        let (kind, _) = Self::group_row(tx, group_id)?.ok_or_else(|| Error::Storage("a group this device isn't in".to_owned()))?;
        let pending: bool = tx.query_row("SELECT count(*) > 0 FROM commits WHERE grp = ?1", [group_id], |row| row.get(0)).map_err(sql)?;
        let stateless = Group::load(provider, group_kind(&kind)?, group_id)?.is_none();
        if pending && (commit || stateless) {
            return Err(DmGroupError::of(DmGroupErrorKind::Pending, group_id, "a change of this device's to the group waits for the service"));
        }
        if stateless {
            return Err(DmGroupError::of(DmGroupErrorKind::Forked, group_id, "this device has no state of the group: it joins again first"));
        }
        Ok(())
    }

    /// Adds the leaves of [key_packages] (envelopes' JSON) to [group_id].
    /// None when there's nothing to add: every package's key is in the
    /// group already, or repeated (`Error::NothingToAdd`, told by its kind,
    /// never its words: Fugo).
    pub fn add_members(&self, group_id: Vec<u8>, key_packages: Vec<String>) -> Result<Option<DmCommit>, DmGroupError> {
        let packages = key_packages.iter().map(|package| Ok(read_envelope(package)?.into_bytes())).collect::<anyhow::Result<Vec<_>>>()?;
        self.change_group(&group_id, |group, provider, me, pins| match group.add(provider, me, pins, &packages) {
            Ok(out) => Ok(Some(out)),
            Err(Error::NothingToAdd) => Ok(None),
            Err(error) => Err(error),
        })
    }

    /// Removes the leaves [leaf_ids] from [group_id].
    pub fn remove_members(&self, group_id: Vec<u8>, leaf_ids: Vec<String>) -> Result<DmCommit, DmGroupError> {
        let ids: Vec<&str> = leaf_ids.iter().map(String::as_str).collect();
        made(self.change_group(&group_id, |group, provider, me, _| group.remove(provider, me, &ids).map(Some)))
    }

    /// After a start-over's rejoin (row 630): one commit taking every dead
    /// leaf of the person's out of [group_id] (under a key other than this
    /// device's, on no list of the new one), so none holds the new epoch's
    /// secrets. None when there's none.
    ///
    /// [list_versions] are GET groups' (the worker's): a device of the
    /// person's own taken off their list goes too (E11-8), but only once
    /// this device's own list is at the worker's version, so one on an
    /// older list never takes out a device added since
    /// ([DmGroupErrorKind::ListsStale]).
    pub fn remove_dead_leaves(&self, group_id: Vec<u8>, list_versions: Vec<DmListVersion>) -> Result<Option<DmCommit>, DmGroupError> {
        let state = self.state();
        let me = state.me.as_ref().ok_or_else(|| anyhow!("this device isn't set up"))?;
        let behind = list_versions
            .iter()
            .any(|each| each.account == state.account_id && state.pins.list_version(&each.account).is_none_or(|here| here < each.version));
        if behind {
            return Err(DmGroupError {
                accounts: vec![state.account_id.clone()],
                ..DmGroupError::of(DmGroupErrorKind::ListsStale, &group_id, "this device's own device list is older than the service's")
            });
        }
        let out = self.store.with_tx(|provider, tx| {
            self.guard(provider, tx, &group_id, true)?;
            let (mut group, _) = Self::load(provider, tx, &group_id)?;
            // The person's own devices among them, kept by name: Your devices
            // lists them ("Still on your old security key") until the person
            // removes them, signing them out (Fugo). Not this device's own
            // leaf from before the start-over (same leaf id, old key): that's
            // this device, not another (Abbacchio's row 640).
            let here = &me.certificate.leaf.leaf_id;
            let own: Vec<DmMember> = group
                .dead(&state.pins, me)
                .into_iter()
                .filter(|dead| dead.leaf.account_id == state.account_id && dead.leaf.kind == LeafKind::Device && &dead.leaf.leaf_id != here)
                .map(|dead| DmMember {
                    account: dead.leaf.account_id,
                    leaf_id: dead.leaf.leaf_id,
                    kind: DmLeafKind::Device,
                    name: dead.leaf.name,
                    since_ms: dead.leaf.created_ms,
                })
                .collect();
            let Some(out) = group.remove_dead(provider, me, &state.pins)? else { return Ok::<_, DmGroupError>(None) };
            if !own.is_empty() {
                let mut kept: Vec<DmMember> = self.read_row(tx, REPLACED)?.unwrap_or_default();
                for each in own {
                    if !kept.iter().any(|known| known.leaf_id == each.leaf_id) {
                        kept.push(each);
                    }
                }
                self.save_row(tx, REPLACED, &json(&kept)?)?;
            }
            self.pending_commit(tx, &group_id, &out.commit)?;
            Ok(Some(out))
        })?;
        Ok(out.map(commit).transpose()?)
    }

    /// The person's devices a start-over took out of their groups (under
    /// the key it replaced), until the person removes them.
    pub fn replaced_devices(&self) -> anyhow::Result<Vec<DmMember>> {
        let state = self.state();
        let here = state.me.as_ref().map(|me| me.certificate.leaf.leaf_id.as_str());
        let kept: Vec<DmMember> = self.store.with_tx(|_, tx| self.read_row(tx, REPLACED))?.unwrap_or_default();
        // Never this device (kept by a build before row 640's fix).
        Ok(kept.into_iter().filter(|each| Some(each.leaf_id.as_str()) != here).collect())
    }

    /// [leaf_id] was removed (signed out): no longer listed as replaced.
    pub fn forget_replaced(&self, leaf_id: String) -> anyhow::Result<()> {
        let _state = self.state();
        self.store.with_tx(|_, tx| -> Result<(), Error> {
            let mut kept: Vec<DmMember> = self.read_row(tx, REPLACED)?.unwrap_or_default();
            kept.retain(|each| each.leaf_id != leaf_id);
            self.save_row(tx, REPLACED, &json(&kept)?)
        })?;
        Ok(())
    }

    /// The person's own leaves in [group_id] under a security key other
    /// than this device's (a key replaced since), by leaf id, unchecked:
    /// never signed out by a sweep (Fugo: a surviving old device is the
    /// person's alarm when someone else started over), and shown apart.
    pub fn replaced_leaves(&self, group_id: Vec<u8>) -> anyhow::Result<Vec<String>> {
        let state = self.state();
        let current = state.identity.as_ref().map(|identity| identity.public());
        Ok(self.store.with_tx(|provider, tx| {
            let (group, _) = Self::load(provider, tx, &group_id)?;
            Ok::<_, Error>(
                group
                    .certificates_unchecked()
                    .into_iter()
                    .filter(|certificate| {
                        certificate.leaf.account_id == state.account_id
                            && Some(certificate.identity) != current
                            && state.me.as_ref().is_none_or(|me| me.certificate.leaf.leaf_id != certificate.leaf.leaf_id)
                    })
                    .map(|certificate| certificate.leaf.leaf_id)
                    .collect(),
            )
        })?)
    }

    /// The other person in the conversation [group_id] when none of their
    /// devices is in yet (row 676), and whether this device adds them
    /// first. None for any other group, or once they're in.
    pub fn not_joined(&self, group_id: Vec<u8>) -> anyhow::Result<Option<DmJoining>> {
        let state = self.state();
        let me = state.me.as_ref().ok_or_else(|| anyhow!("this device isn't set up"))?;
        Ok(self.store.with_tx(|provider, tx| {
            let Some((kind, _)) = Self::group_row(tx, &group_id)? else { return Ok::<_, Error>(None) };
            let (group, _) = Self::load(provider, tx, &group_id)?;
            // Waiting for a new security key or a reinstall is said as that
            // ([waiting_for]).
            if group.waiting(&state.pins, me).is_some() {
                return Ok(None);
            }
            let Some(account) = not_joined(&kind, &group, &state.pins, &state.account_id) else { return Ok(None) };
            // Members in tree order: the first of the person's devices adds.
            let first = roster(&group, &state.pins).into_iter().find(|(member, _)| member.account == state.account_id);
            let mine = &me.certificate.leaf;
            let adds_first = first.is_some_and(|(member, key)| member.leaf_id == mine.leaf_id && key == mine.signature_key);
            Ok(Some(DmJoining { account, adds_first }))
        })?)
    }

    /// The account whose device under a replaced key, on no list of the new
    /// one, is still in [group_id]: nothing is sent there until it's out
    /// (Fugo, row 630). None when nothing holds it.
    pub fn waiting_for(&self, group_id: Vec<u8>) -> anyhow::Result<Option<String>> {
        let state = self.state();
        let me = state.me.as_ref().ok_or_else(|| anyhow!("this device isn't set up"))?;
        Ok(self.store.with_tx(|provider, tx| {
            let (group, _) = Self::load(provider, tx, &group_id)?;
            Ok::<_, Error>(group.waiting(&state.pins, me))
        })?)
    }

    /// The device whose former install holds [group_id] (row 202): its
    /// name, when what [waiting_for] waits for is a reinstall; None
    /// otherwise.
    pub fn reinstalling(&self, group_id: Vec<u8>) -> anyhow::Result<Option<String>> {
        let state = self.state();
        let me = state.me.as_ref().ok_or_else(|| anyhow!("this device isn't set up"))?;
        Ok(self.store.with_tx(|provider, tx| {
            let (group, _) = Self::load(provider, tx, &group_id)?;
            Ok::<_, Error>(match group.held(&state.pins, me) {
                Some((_, claktory_dm_crypto::validation::Held::Reinstalled)) => {
                    group.stale(&state.pins).into_iter().next().map(|stale| stale.leaf.name)
                }
                _ => None,
            })
        })?)
    }

    /// The device its person removed from their account whose leaf is
    /// still in [group_id] (Held::Removed, E11-8): its name ("" when it has
    /// none), when that's what [waiting_for] waits for, until one of that
    /// person's devices takes it out (remove_dead_leaves); None otherwise.
    pub fn removed_held(&self, group_id: Vec<u8>) -> anyhow::Result<Option<String>> {
        let state = self.state();
        let me = state.me.as_ref().ok_or_else(|| anyhow!("this device isn't set up"))?;
        Ok(self.store.with_tx(|provider, tx| {
            let (group, _) = Self::load(provider, tx, &group_id)?;
            Ok::<_, Error>(match group.held(&state.pins, me) {
                Some((account, claktory_dm_crypto::validation::Held::Removed)) => Some(
                    group
                        .taken_off(&state.pins)
                        .into_iter()
                        .find(|removed| removed.leaf.account_id == account)
                        .map(|removed| removed.leaf.name)
                        .unwrap_or_default(),
                ),
                _ => None,
            })
        })?)
    }

    /// New keys for this device's leaf in [group_id].
    pub fn update_group(&self, group_id: Vec<u8>) -> Result<DmCommit, DmGroupError> {
        made(self.change_group(&group_id, |group, provider, me, _| group.update(provider, me).map(Some)))
    }

    /// Moves this device's leaf in [group_id] to its certificate under the
    /// person's new security key (same leaf, same signature key; dms-e2ee
    /// §2.1): the one commit a leaf under a replaced key may make there.
    pub fn move_group(&self, group_id: Vec<u8>) -> Result<DmCommit, DmGroupError> {
        made(self.change_group(&group_id, |group, provider, me, _| group.update_certificate(provider, me).map(Some)))
    }

    /// The worker took this device's commit to [group_id]: merge it.
    pub fn commit_taken(&self, group_id: Vec<u8>) -> anyhow::Result<()> {
        let _state = self.state();
        self.store.with_tx(|provider, tx| self.merge_own(provider, tx, &group_id))?;
        Ok(())
    }

    /// The worker took this device's commit: merged, or a rejoin's state
    /// put aside taken back, by the commit's digest, its floor raised only
    /// now (E11-8).
    fn merge_own(&self, provider: &Provider<'_>, tx: &Transaction<'_>, group_id: &[u8]) -> Result<(), Error> {
        let Some(digest) = self.settle(tx, group_id)? else { return Ok(()) };
        if claktory_dm_crypto::storage::pending_rejoin(tx, group_id)?.is_some() {
            claktory_dm_crypto::storage::take_rejoin(tx, group_id, &digest)?;
            // Back in step: changes and sends go on.
            Self::delete_row(tx, &forked_place(group_id).each())?;
            return Ok(());
        }
        let (mut group, _) = Self::load(provider, tx, group_id)?;
        group.merge_pending(provider)
    }

    /// The worker refused this device's commit (another came first): drop
    /// it; the change can be made again on the new epoch.
    pub fn commit_refused(&self, group_id: Vec<u8>) -> anyhow::Result<()> {
        let _state = self.state();
        self.store.with_tx(|provider, tx| self.give_up(provider, tx, &group_id))?;
        Ok(())
    }

    /// How this device's state of [group_id] stands against the worker's
    /// (GET groups' `epoch` and `confirmation_tag`, base64url), E11-8:
    /// - a commit of its own made under [PENDING_FOR] ago: [DmForkState::Pending],
    ///   nothing changed;
    /// - one older, with the worker past the epoch it was made at: Behind
    ///   (the feed settles it: its own back, merged, or another's first);
    ///   at that epoch still: given up, [DmForkState::PendingCleared];
    /// - no state here (a rejoin refused): Forked;
    /// - past the worker, or at its epoch with another confirmation tag:
    ///   Forked, kept so changes and sends are refused
    ///   ([DmGroupErrorKind::Forked]) until [DmDevice::resync_group]'s
    ///   commit is taken;
    /// - behind it: Behind; otherwise InStep.
    pub fn fork_check(&self, group_id: Vec<u8>, worker_epoch: u64, confirmation_tag: String) -> anyhow::Result<DmForkState> {
        use base64::Engine as _;
        let tag = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(confirmation_tag.trim_end_matches('='))
            .or_else(|_| base64::engine::general_purpose::STANDARD.decode(&confirmation_tag))
            .context("a confirmation tag that isn't base64")?;
        let _state = self.state();
        Ok(self.store.with_tx(|provider, tx| {
            let (kind, _) = Self::group_row(tx, &group_id)?.ok_or_else(|| Error::Storage("a group this device isn't in".to_owned()))?;
            let pending: bool =
                tx.query_row("SELECT count(*) > 0 FROM commits WHERE grp = ?1", [&group_id], |row| row.get(0)).map_err(sql)?;
            let group = Group::load(provider, group_kind(&kind)?, &group_id)?;
            if pending {
                // Unknown (made before this was kept): long ago.
                let made: i64 = self.read_row(tx, &commit_place(&group_id).each())?.unwrap_or(0);
                if now_ms() - made < PENDING_FOR {
                    return Ok::<_, Error>(DmForkState::Pending);
                }
                // The epoch it was made at: a rejoin's is the one before
                // the epoch it reaches.
                let made_at = match &group {
                    Some(group) => Some(group.epoch()),
                    None => claktory_dm_crypto::storage::pending_rejoin(tx, &group_id)?.map(|reached| reached.saturating_sub(1)),
                };
                if made_at.is_some_and(|epoch| worker_epoch > epoch) {
                    return Ok(DmForkState::Behind);
                }
                self.give_up(provider, tx, &group_id)?;
                return Ok(DmForkState::PendingCleared);
            }
            let forked = |tx: &Transaction<'_>| -> Result<DmForkState, Error> {
                let stuck = self.read_row::<Forked>(tx, &forked_place(&group_id).each())?.is_some_and(|forked| forked.stuck);
                self.save_row(tx, &forked_place(&group_id).each(), &json(&Forked { stuck })?)?;
                Ok(DmForkState::Forked)
            };
            let Some(group) = group else { return forked(tx) };
            if group.epoch() > worker_epoch || (group.epoch() == worker_epoch && group.confirmation_tag()? != tag) {
                return forked(tx);
            }
            if group.epoch() < worker_epoch {
                return Ok(DmForkState::Behind);
            }
            Self::delete_row(tx, &forked_place(&group_id).each())?;
            Ok(DmForkState::InStep)
        })?)
    }

    /// Joins [group_id] again from the worker's GroupInfo ([group_info],
    /// an envelope's JSON) once [DmDevice::fork_check] found it forked
    /// (E11-8): an external commit, every member checked as a rejoin
    /// checks them, never a re-add by the home; the feed goes on after
    /// [seq]. Its state waits aside until the worker takes the commit
    /// ([DmDevice::commit_taken], or its coming back in the feed); then
    /// take out what's dead at once ([DmDevice::remove_dead_leaves]) and
    /// send [DmResync::resend] again. A GroupInfo below the highest epoch
    /// this device reached is refused as [DmGroupError::stuck].
    pub fn resync_group(&self, group_id: Vec<u8>, group_info: String, seq: i64) -> Result<DmResync, DmGroupError> {
        let info = read_envelope(&group_info)?;
        let state = self.state();
        let me = state.me.as_ref().ok_or_else(|| anyhow!("this device isn't set up"))?;
        let done = self.store.with_tx(|provider, tx| {
            let (kind, _) = Self::group_row(tx, &group_id)?.ok_or_else(|| Error::Storage("a group this device isn't in".to_owned()))?;
            let pending: bool =
                tx.query_row("SELECT count(*) > 0 FROM commits WHERE grp = ?1", [&group_id], |row| row.get(0)).map_err(sql)?;
            if pending {
                return Err(DmGroupError::of(DmGroupErrorKind::Pending, &group_id, "a change of this device's to the group waits for the service"));
            }
            let floor = claktory_dm_crypto::storage::epoch_floor(tx, &group_id)?;
            if claktory_dm_crypto::group::group_info_epoch(info.bytes())? < floor {
                // Kept (the transaction commits), so changes and sends stay
                // refused, saying why.
                self.save_row(tx, &forked_place(&group_id).each(), &json(&Forked { stuck: true })?)?;
                return Ok(Err(DmGroupError {
                    stuck: true,
                    ..DmGroupError::of(
                        DmGroupErrorKind::Forked,
                        &group_id,
                        "the group's state at the service is from before this device's: it waits for a member to move it on",
                    )
                }));
            }
            claktory_dm_crypto::storage::forget_group(tx, &openmls_group_id(&group_id))?;
            let (_, out) = Group::rejoin(provider, me, &state.pins, group_kind(&kind)?, &group_id, info.bytes(), floor)?;
            claktory_dm_crypto::storage::stash_rejoin(tx, &group_id, floor, &out.commit)?;
            Self::add_group(tx, &group_id, &kind, seq)?;
            self.pending_commit(tx, &group_id, &out.commit)?;
            let resend: Vec<String> = tx
                .prepare("SELECT client_id FROM outgoing WHERE grp = ?1 ORDER BY created_ms")
                .map_err(sql)?
                .query_map([&group_id], |row| row.get(0))
                .map_err(sql)?
                .collect::<Result<_, _>>()
                .map_err(sql)?;
            if !resend.is_empty() {
                self.save_row(tx, &forked_place(&group_id).each(), &json(&Forked { stuck: false })?)?;
            }
            Ok(Ok((out, resend)))
        })?;
        let (out, resend) = done?;
        Ok(DmResync { commit: commit(out)?, resend })
    }

    /// The device lists this device took, oldest first (E11-8): whose,
    /// which version, when. No keys.
    pub fn lists_adopted(&self) -> anyhow::Result<Vec<DmListAdopted>> {
        let _state = self.state();
        Ok(self.store.with_tx(|_, tx| self.read_row(tx, LISTS_ADOPTED))?.unwrap_or_default())
    }

    /// Forgets [group_id]: its MLS state, history and what waits to go
    /// (revoked, unfriended, signed out).
    pub fn forget_group(&self, group_id: Vec<u8>) -> anyhow::Result<()> {
        let _state = self.state();
        self.store.with_tx(|_, tx| {
            claktory_dm_crypto::storage::forget_group(tx, &openmls_group_id(&group_id))?;
            tx.execute("DELETE FROM groups WHERE id = ?1", [&group_id]).map_err(sql)?;
            for place in [proposal_place(&group_id), commit_place(&group_id), forked_place(&group_id)] {
                Self::delete_row(tx, &place.each())?;
            }
            Ok::<_, Error>(())
        })?;
        Ok(())
    }

    /// Every group this device is in.
    pub fn groups(&self) -> anyhow::Result<Vec<DmGroup>> {
        let state = self.state();
        Ok(self.store.with_tx(|provider, tx| {
            let rows: Vec<(Vec<u8>, String, i64, bool)> = tx
                .prepare("SELECT id, kind, seq, EXISTS (SELECT 1 FROM commits WHERE grp = groups.id) FROM groups ORDER BY id")
                .map_err(sql)?
                .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)))
                .map_err(sql)?
                .collect::<Result<_, _>>()
                .map_err(sql)?;
            rows.into_iter()
                .map(|(group_id, kind, seq, pending)| {
                    let kind: DmGroupKind = serde_json::from_str(&kind).map_err(|e| Error::Format(e.to_string()))?;
                    let group = Group::load(provider, group_kind(&kind)?, &group_id)?;
                    let epoch = group.as_ref().map(Group::epoch).unwrap_or(0);
                    let moved = match (&group, &state.me) {
                        (Some(group), Some(me)) => group.holds(me).unwrap_or(true),
                        _ => true,
                    };
                    Ok(DmGroup { group_id, kind, seq, epoch, moved, pending })
                })
                .collect::<Result<Vec<_>, Error>>()
        })?)
    }

    /// Encrypts [content] for [group_id], once: asked again for the same
    /// retry id, it gives the same envelope (JSON) until that comes back.
    /// Refused in a conversation while the friend's security key changed
    /// and the person hasn't seen it ([seen]).
    ///
    /// Refused, typed, while the group's state here is forked or gone
    /// ([DmGroupErrorKind::Forked]), and while a member's device list here
    /// is older than the worker's [list_versions] (GET groups'), so a
    /// device its person took off isn't sent to on an old list
    /// ([DmGroupErrorKind::ListsStale], E11-8).
    pub fn send(&self, group_id: Vec<u8>, content: DmContent, now_ms: i64, list_versions: Vec<DmListVersion>) -> Result<String, DmGroupError> {
        let id = content.client_message_id.clone();
        if id.is_empty() {
            return Err(DmGroupError::of(DmGroupErrorKind::Failed, &group_id, "a message has a retry id"));
        }
        let content = content.sent_at(now_ms)?;
        let encoded = content.to_content()?.encode()?;
        let extra: serde_json::Map<String, serde_json::Value> = serde_json::from_str(&content.extra_json).unwrap_or_default();
        let state = self.state();
        let me = state.me.as_ref().ok_or_else(|| anyhow!("this device isn't set up"))?;
        let pending = json(&Pending { content: content.clone() })?;
        let bytes = self.store.with_tx(|provider, tx| {
            let kept: Option<Vec<u8>> = tx
                .query_row("SELECT envelope FROM outgoing WHERE grp = ?1 AND client_id = ?2", params![group_id, id], |row| row.get(0))
                .optional()
                .map_err(sql)?;
            if let Some(kept) = kept {
                return Ok::<_, DmGroupError>(kept);
            }
            self.guard(provider, tx, &group_id, false)?;
            let stale: Vec<String> = list_versions
                .iter()
                .filter(|each| state.pins.list_version(&each.account).is_none_or(|here| here < each.version))
                .map(|each| each.account.clone())
                .collect();
            if !stale.is_empty() {
                return Err(DmGroupError {
                    accounts: stale,
                    ..DmGroupError::of(DmGroupErrorKind::ListsStale, &group_id, "a member's device list here is older than the service's")
                });
            }
            let (kind, _) = Self::group_row(tx, &group_id)?.ok_or_else(|| Error::Storage("a group this device isn't in".to_owned()))?;
            // A friend's security key changed and the person hasn't seen the
            // notice: nothing is encrypted to it (Fugo 2b), whatever calls.
            if kind.kind == DmGroupType::Conversation {
                for account in kind.accounts.iter().filter(|account| **account != state.account_id) {
                    if !state.pins.can_send(account) {
                        return Err(Error::Refused(format!("{account}'s security key changed and that hasn't been seen yet")).into());
                    }
                }
            }
            let (mut group, _) = Self::load(provider, tx, &group_id)?;
            // The other person isn't in yet (row 676): nothing is encrypted
            // for a group they can never read it in, whatever calls. Every
            // application message goes through here.
            // (A wait for a new security key or a reinstall says so first.)
            if group.waiting(&state.pins, me).is_none()
                && let Some(account) = not_joined(&kind, &group, &state.pins, &state.account_id)
            {
                return Err(Error::Refused(format!("waiting for {account} to join")).into());
            }
            let bytes = group.send(provider, me, &state.pins, &encoded)?;
            let sealed = self.store.seal_row(&place(b"outgoing", &group_id, id.as_bytes(), b"value"), &pending)?;
            tx.execute(
                "INSERT INTO outgoing (grp, client_id, digest, envelope, value, created_ms) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![group_id, id, envelope::digest(&bytes).to_vec(), bytes, sealed, now_ms],
            )
            .map_err(sql)?;
            // The person's own draft, read position or deletion counts here
            // at once; coming back, it changes nothing more.
            if kind.kind == DmGroupType::SelfGroup {
                self.apply_view(tx, &content, &extra)?;
            }
            Ok(bytes)
        })?;
        Ok(envelope_json(bytes)?)
    }

    /// What was encrypted here and hasn't come back: to send again, as it is.
    pub fn unsent(&self) -> anyhow::Result<Vec<DmUnsent>> {
        let _state = self.state();
        let rows = self.store.with_tx(|_, tx| {
            tx.prepare("SELECT grp, client_id, envelope, created_ms, value FROM outgoing ORDER BY created_ms")
                .map_err(sql)?
                .query_map([], |row| {
                    Ok((
                        row.get::<_, Vec<u8>>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, Vec<u8>>(2)?,
                        row.get(3)?,
                        row.get::<_, Vec<u8>>(4)?,
                    ))
                })
                .map_err(sql)?
                .collect::<Result<Vec<_>, _>>()
                .map_err(sql)
        })?;
        rows.into_iter()
            .map(|(group_id, client_message_id, bytes, created_ms, sealed)| {
                let pending = self.store.open_row(&place(b"outgoing", &group_id, client_message_id.as_bytes(), b"value"), &sealed)?;
                let pending: Pending = serde_json::from_slice(&pending)?;
                Ok(DmUnsent { group_id, client_message_id, envelope: envelope_json(bytes)?, created_ms, content: pending.content })
            })
            .collect()
    }

    /// Gives up on sending [client_message_id] to [group_id] (the person
    /// removed it, or the service refused it for good).
    pub fn drop_unsent(&self, group_id: Vec<u8>, client_message_id: String) -> anyhow::Result<()> {
        let _state = self.state();
        self.store.with_tx(|_, tx| {
            tx.execute("DELETE FROM outgoing WHERE grp = ?1 AND client_id = ?2", params![group_id, client_message_id]).map_err(sql)
        })?;
        Ok(())
    }

    /// The feed's next message for [group_id]: [seq] from the worker and its
    /// [envelope] (JSON). In one transaction with what it changes; call in
    /// seq order. A message this device can't take is dropped and the
    /// position moves past it, so one bad message never stops the group.
    pub fn process(&self, group_id: Vec<u8>, seq: i64, envelope: String, arrived_ms: i64) -> anyhow::Result<DmProcessed> {
        let mut guard = self.state();
        let state = &mut *guard;
        let parsed = read_envelope(&envelope);
        let handshake = parsed.as_ref().is_ok_and(|envelope| !matches!(envelope.parsed().kind, envelope::Kind::Application));
        let outcome = self.store.with_tx(|provider, tx| {
            let Some((kind, position)) = Self::group_row(tx, &group_id)? else {
                return Err(Error::Storage("a group this device isn't in".to_owned()));
            };
            let own_leaf = state.me.as_ref().map(|me| me.certificate.leaf.leaf_id.clone()).unwrap_or_default();
            if seq <= position {
                return Ok(Ok(DmProcessed::of(DmOutcome::Already)));
            }
            // The first of the home's: where the worker's feed was is kept,
            // for the proposals that stay there ([take_proposal]).
            if position < HOME_SEQ_BASE && seq > HOME_SEQ_BASE {
                let place = proposal_place(&group_id);
                if self.read_row::<i64>(tx, &place.each())?.is_none() {
                    self.save_row(tx, &place.each(), &json(&position)?)?;
                }
            }
            let advance = |tx: &Transaction<'_>| -> Result<(), Error> {
                tx.execute("UPDATE groups SET seq = ?2 WHERE id = ?1", params![group_id, seq]).map_err(sql)?;
                Ok(())
            };
            let envelope = match &parsed {
                Ok(envelope) => envelope,
                Err(error) => {
                    advance(tx)?;
                    return Ok(Err(error.to_string()));
                }
            };
            let digest = envelope.digest().to_vec();
            // This device's own commit coming back: the worker took it.
            let own_commit: bool = tx
                .query_row("SELECT count(*) FROM commits WHERE grp = ?1 AND digest = ?2", params![group_id, digest], |row| {
                    row.get::<_, i64>(0)
                })
                .map_err(sql)?
                > 0;
            if own_commit {
                // A rejoin's has no state before it here: as after it.
                let before = Group::load(provider, group_kind(&kind)?, &group_id)?.map(|group| snapshot(&group, &state.pins));
                self.merge_own(provider, tx, &group_id)?;
                advance(tx)?;
                let (group, _) = Self::load(provider, tx, &group_id)?;
                let before = before.unwrap_or_else(|| snapshot(&group, &state.pins));
                let line = self.member_line(
                    tx,
                    &group_id,
                    seq,
                    arrived_ms,
                    true,
                    &own_leaf,
                    &state.account_id,
                    &listed_ids(&state.pins, &state.account_id),
                    before,
                    snapshot(&group, &state.pins),
                )?;
                return Ok(Ok(DmProcessed { message: line, ..DmProcessed::committed(group.epoch()) }));
            }
            // This device's own message coming back: from what it kept.
            if let Some(message) = self.own_message(tx, state, &kind, &group_id, envelope, seq, arrived_ms)? {
                advance(tx)?;
                return Ok(Ok(DmProcessed::message(message)));
            }
            let Some(mut group) = Group::load(provider, group_kind(&kind)?, &group_id)? else {
                return match self.stateless(provider, tx, &group_id, envelope)? {
                    Stateless::Old => {
                        advance(tx)?;
                        Ok(Ok(DmProcessed::of(DmOutcome::Already)))
                    }
                    Stateless::Out(why) => Ok(Err(why)),
                };
            };
            // From an epoch past this one: a commit is missing here. Held,
            // the position kept, until it's in (row 551).
            if envelope.parsed().group.as_ref().is_some_and(|(_, epoch)| *epoch > group.epoch()) {
                return Ok(Ok(DmProcessed { handshake, behind: true, epoch: group.epoch(), ..DmProcessed::of(DmOutcome::Held) }));
            }
            let before = handshake.then(|| snapshot(&group, &state.pins));
            let incoming = match group.process(provider, &state.pins, envelope.bytes()) {
                Ok(incoming) => incoming,
                // A commit under a key not pinned yet: held, unread, until
                // that account's keys are in (Ghiaccio: dropping it would
                // leave every later commit failing). What's said under one
                // is dropped as before.
                Err(Error::UnknownIdentity { account }) if handshake => {
                    return Ok(Ok(DmProcessed { why: account, handshake, ..DmProcessed::of(DmOutcome::Held) }));
                }
                // One under a key its account replaced: never taken here, so
                // held for a rejoin past it (row 654). Something said under
                // one (a kept device before it moves) is dropped as before.
                Err(Error::Replaced { account }) if handshake => {
                    return Ok(Ok(DmProcessed { why: account, handshake, replaced: true, ..DmProcessed::of(DmOutcome::Held) }));
                }
                // A leaf the list taken here doesn't name: held, the
                // position kept, for a newer list (row 561; the crate's R5).
                Err(Error::Unlisted { account }) => return Ok(Ok(DmProcessed::unlisted(account, handshake))),
                Err(error) if refusal(&error) => return Ok(Err(error.to_string())),
                Err(error) => return Err(error),
            };
            let processed = match incoming {
                // Something said in a conversation by a Claktory's leaf:
                // never shown (#451, Fugo). An agent's message only ever
                // comes as one of the person's devices sent it, after their
                // approval; a home can't see who sent what, so the check is
                // here, from the sender's signed certificate.
                Incoming::Application { from, .. } if kind.kind == DmGroupType::Conversation && from.kind() == LeafKind::Claktory => {
                    advance(tx)?;
                    return Ok(Err("a Claktory's leaf can't say anything in a conversation".to_owned()));
                }
                Incoming::Application { from, content, .. } => match Content::decode(&content) {
                    Ok(content) => {
                        let message = DmMessage {
                            seq,
                            mine: false,
                            from_account: from.account().to_owned(),
                            from_leaf: from.leaf_id().to_owned(),
                            from_kind_device: from.kind() == LeafKind::Device,
                            content: DmContent::from_content(content),
                            arrived_ms,
                            members: None,
                            departed: false,
                        };
                        if kind.kind == DmGroupType::SelfGroup && message.content.kind == ROTATION_KIND {
                            let name = from.certificate.leaf.name.clone();
                            self.keep_rotation(tx, state, &group, &message, &name)?;
                        } else if kind.kind == DmGroupType::Channel
                            && matches!(
                                message.content.kind.as_str(),
                                claktory_dm_crypto::content::kind::PROPOSAL | claktory_dm_crypto::content::kind::AGENT_DRAFT
                            )
                        {
                            // A proposal or an agent's draft, which its
                            // Claktory sends through the home (#451): kept by
                            // its digest, which the service's record names.
                            self.keep_side(tx, &group_id, &digest, &message)?;
                        } else {
                            self.take(tx, state, &kind, &group_id, &message)?;
                        }
                        DmProcessed::message(message)
                    }
                    // Decrypted, but not a content this reads: the ratchet
                    // moved, so it's kept moved; the message is dropped.
                    Err(error) => {
                        advance(tx)?;
                        return Ok(Err(error.to_string()));
                    }
                },
                Incoming::Proposal => DmProcessed::of(DmOutcome::Proposal),
                Incoming::Committed { epoch } => {
                    let after = snapshot(&group, &state.pins);
                    let line = self.member_line(
                        tx,
                        &group_id,
                        seq,
                        arrived_ms,
                        false,
                        &own_leaf,
                        &state.account_id,
                        &listed_ids(&state.pins, &state.account_id),
                        before.unwrap_or_default(),
                        after,
                    )?;
                    DmProcessed { message: line, ..DmProcessed::committed(epoch) }
                }
            };
            advance(tx)?;
            Ok(Ok(processed))
        })?;
        match outcome {
            Ok(processed) => Ok(processed),
            Err(why) => {
                // Refused before anything changed: the group as it was, the
                // position past it, in a transaction of its own.
                let _ = self
                    .store
                    .with_tx(|_, tx| tx.execute("UPDATE groups SET seq = max(seq, ?2) WHERE id = ?1", params![group_id, seq]).map_err(sql));
                Ok(DmProcessed { why, handshake, ..DmProcessed::of(DmOutcome::Dropped) })
            }
        }
    }

    /// Gives up on the feed's message [seq] in [group_id], held until now
    /// ([DmOutcome::Held]): the position moves past it.
    pub fn skip(&self, group_id: Vec<u8>, seq: i64) -> anyhow::Result<()> {
        let _state = self.state();
        self.store
            .with_tx(|_, tx| tx.execute("UPDATE groups SET seq = max(seq, ?2) WHERE id = ?1", params![group_id, seq]).map_err(sql))?;
        Ok(())
    }

    /// Where this device is in the worker's feed of [group_id] once it
    /// reads the home (#451): the proposals stay there (a Claktory's own
    /// Remove, Mista 32950), and the home's commits take them in by
    /// reference. Before the home, where it is in the feed.
    #[frb(sync)]
    pub fn proposal_seq(&self, group_id: Vec<u8>) -> anyhow::Result<i64> {
        let _state = self.state();
        Ok(self.store.with_tx(|_, tx| Self::proposal_position(self, tx, &group_id))?)
    }

    fn proposal_position(&self, tx: &Transaction<'_>, group_id: &[u8]) -> Result<i64, Error> {
        if let Some(seq) = self.read_row::<i64>(tx, &proposal_place(group_id).each())? {
            return Ok(seq);
        }
        let position = Self::group_row(tx, group_id)?.map(|(_, seq)| seq).unwrap_or(0);
        Ok(if position < HOME_SEQ_BASE { position } else { 0 })
    }

    /// The worker's message [seq] in [group_id] once this device reads the
    /// home: only a proposal is taken (kept for the commit that takes it
    /// in), anything else is passed over; the worker position moves past
    /// it either way, but not past a proposal [DmOutcome::Held] (under a
    /// key not pinned yet). Call in seq order, each before the home's
    /// commit of its epoch.
    pub fn take_proposal(&self, group_id: Vec<u8>, seq: i64, envelope: String) -> anyhow::Result<DmProcessed> {
        let mut guard = self.state();
        let state = &mut *guard;
        let parsed = read_envelope(&envelope);
        let place = proposal_place(&group_id);
        let outcome = self.store.with_tx(|provider, tx| {
            if seq <= self.proposal_position(tx, &group_id)? {
                return Ok(Ok(DmProcessed::of(DmOutcome::Already)));
            }
            let advance = |tx: &Transaction<'_>| self.save_row(tx, &place.each(), &json(&seq)?);
            let envelope = match &parsed {
                Ok(envelope) if matches!(envelope.parsed().kind, envelope::Kind::Proposal) => envelope,
                Ok(_) => {
                    advance(tx)?;
                    return Ok(Ok(DmProcessed::of(DmOutcome::Already)));
                }
                Err(error) => {
                    advance(tx)?;
                    return Ok(Err(error.to_string()));
                }
            };
            let (mut group, _) = Self::load(provider, tx, &group_id)?;
            match group.process(provider, &state.pins, envelope.bytes()) {
                Ok(Incoming::Proposal) => {
                    advance(tx)?;
                    Ok(Ok(DmProcessed { handshake: true, ..DmProcessed::of(DmOutcome::Proposal) }))
                }
                Ok(_) => Err(Error::Format("a proposal that wasn't one".to_owned())),
                Err(Error::UnknownIdentity { account }) => {
                    Ok(Ok(DmProcessed { why: account, handshake: true, ..DmProcessed::of(DmOutcome::Held) }))
                }
                Err(Error::Unlisted { account }) => Ok(Ok(DmProcessed::unlisted(account, true))),
                Err(error) if refusal(&error) => Ok(Err(error.to_string())),
                Err(error) => Err(error),
            }
        })?;
        match outcome {
            Ok(processed) => Ok(processed),
            Err(why) => {
                let _ = self.store.with_tx(|_, tx| {
                    if seq > self.proposal_position(tx, &group_id)? {
                        self.save_row(tx, &place.each(), &json(&seq)?)?;
                    }
                    Ok::<_, Error>(())
                });
                Ok(DmProcessed { why, handshake: true, ..DmProcessed::of(DmOutcome::Dropped) })
            }
        }
    }

    /// The worker's commit [seq] in [group_id] once this device reads the
    /// home, which the home never carried (row 551): this device's own,
    /// taken by the worker just as the app stopped, is merged; another's
    /// is taken as the feed takes one, any change of this device's own
    /// still waiting from before it given up. Anything else is passed over.
    /// The worker position moves past it, but not past a commit
    /// [DmOutcome::Held] (under a key not pinned yet); the home's position
    /// stays. Call in seq order with [take_proposal].
    pub fn take_worker_commit(&self, group_id: Vec<u8>, seq: i64, envelope: String, arrived_ms: i64) -> anyhow::Result<DmProcessed> {
        let mut guard = self.state();
        let state = &mut *guard;
        let parsed = read_envelope(&envelope);
        let place = proposal_place(&group_id);
        let outcome = self.store.with_tx(|provider, tx| {
            if Self::group_row(tx, &group_id)?.is_none() {
                return Err(Error::Storage("a group this device isn't in".to_owned()));
            }
            if seq <= self.proposal_position(tx, &group_id)? {
                return Ok(Ok(DmProcessed::of(DmOutcome::Already)));
            }
            let advance = |tx: &Transaction<'_>| self.save_row(tx, &place.each(), &json(&seq)?);
            let envelope = match &parsed {
                Ok(envelope) if matches!(envelope.parsed().kind, envelope::Kind::Commit) => envelope,
                Ok(_) => {
                    advance(tx)?;
                    return Ok(Ok(DmProcessed::of(DmOutcome::Already)));
                }
                Err(error) => {
                    advance(tx)?;
                    return Ok(Err(error.to_string()));
                }
            };
            let own_leaf = state.me.as_ref().map(|me| me.certificate.leaf.leaf_id.clone()).unwrap_or_default();
            let digest = envelope.digest().to_vec();
            let pending: Option<Vec<u8>> =
                tx.query_row("SELECT digest FROM commits WHERE grp = ?1", [&group_id], |row| row.get(0)).optional().map_err(sql)?;
            let mine = pending.as_deref() == Some(digest.as_slice());
            let kind = Self::group_row(tx, &group_id)?.map(|(kind, _)| kind).ok_or_else(|| Error::Storage("a group this device isn't in".to_owned()))?;
            let loaded = Group::load(provider, group_kind(&kind)?, &group_id)?;
            let before = match loaded {
                Some(mut group) => {
                    // One from before this device's epoch: it has it already.
                    if envelope.parsed().group.as_ref().is_some_and(|(_, epoch)| *epoch < group.epoch()) {
                        advance(tx)?;
                        return Ok(Ok(DmProcessed { epoch: group.epoch(), ..DmProcessed::of(DmOutcome::Already) }));
                    }
                    let before = snapshot(&group, &state.pins);
                    if mine {
                        self.merge_own(provider, tx, &group_id)?;
                    } else {
                        if pending.is_some() {
                            // Another's came first: this device's own never will.
                            self.give_up(provider, tx, &group_id)?;
                            group = Self::load(provider, tx, &group_id)?.0;
                        }
                        if let Some(refused) = Self::take_others(provider, &state.pins, &mut group, envelope)? {
                            return Ok(refused);
                        }
                    }
                    Some(before)
                }
                // Its rejoin's commit, taken: its state back (E11-8).
                None if mine => {
                    self.merge_own(provider, tx, &group_id)?;
                    None
                }
                None => {
                    return match self.stateless(provider, tx, &group_id, envelope)? {
                        Stateless::Old => {
                            advance(tx)?;
                            Ok(Ok(DmProcessed::of(DmOutcome::Already)))
                        }
                        Stateless::Out(why) => Ok(Err(why)),
                    };
                }
            };
            advance(tx)?;
            let (group, _) = Self::load(provider, tx, &group_id)?;
            let before = before.unwrap_or_else(|| snapshot(&group, &state.pins));
            let line = self.member_line(
                tx,
                &group_id,
                seq,
                arrived_ms,
                mine,
                &own_leaf,
                &state.account_id,
                &listed_ids(&state.pins, &state.account_id),
                before,
                snapshot(&group, &state.pins),
            )?;
            Ok(Ok(DmProcessed { message: line, handshake: true, ..DmProcessed::committed(group.epoch()) }))
        })?;
        match outcome {
            Ok(processed) => Ok(processed),
            Err(why) => {
                let _ = self.store.with_tx(|_, tx| {
                    if seq > self.proposal_position(tx, &group_id)? {
                        self.save_row(tx, &place.each(), &json(&seq)?)?;
                    }
                    Ok::<_, Error>(())
                });
                Ok(DmProcessed { why, handshake: true, ..DmProcessed::of(DmOutcome::Dropped) })
            }
        }
    }

    /// A commit [process] held as [DmProcessed::unlisted] (row 561):
    /// [items] is it, then what follows it in the group's feed, in order.
    /// The device it brings in is one its person has since taken off their
    /// list; if one of their listed devices that was there all along takes
    /// it out again within [items] (claktory-dm-crypto's window, Fugo's
    /// rule), everything up to that removal is taken as an online member
    /// took it, the departed device's messages marked as such. Anything
    /// else changes nothing here.
    pub fn process_window(&self, group_id: Vec<u8>, items: Vec<DmWindowItem>) -> anyhow::Result<DmWindowOutcome> {
        let mut guard = self.state();
        let state = &mut *guard;
        let mut envelopes = Vec::with_capacity(items.len());
        for item in &items {
            match read_envelope(&item.envelope) {
                Ok(envelope) => envelopes.push(envelope),
                Err(error) => {
                    return Ok(DmWindowOutcome { end: DmWindowEnd::Aborted, why: error.to_string(), processed: vec![] });
                }
            }
        }
        let bytes: Vec<&[u8]> = envelopes.iter().map(|envelope| envelope.bytes()).collect();
        let outcome = |end, why: String| Ok(DmWindowOutcome { end, why, processed: vec![] });
        // What it comes to, and who's in the group after each commit taken
        // (every leaf, checked or not, since the departed one never passes
        // the latest list): each tried and rolled back.
        let tried = |count: usize| -> Result<Result<(Window, Snapshot), Error>, Error> {
            let mut out = None;
            let _ = self.store.with_tx(|provider, tx| -> Result<(), Error> {
                let (mut group, _) = Self::load(provider, tx, &group_id)?;
                let window = group.process_window(provider, &state.pins, &bytes[..count]);
                out = Some(window.map(|window| (window, every_leaf(&group))));
                Err(Error::Storage("tried only".to_owned()))
            });
            out.ok_or_else(|| Error::Storage("the window couldn't be tried".to_owned()))
        };
        let taken = match tried(bytes.len())? {
            Err(error) => return outcome(DmWindowEnd::None, error.to_string()),
            Ok((Window::Open, _)) => return outcome(DmWindowEnd::Open, String::new()),
            Ok((Window::Aborted { why }, _)) => return outcome(DmWindowEnd::Aborted, why),
            Ok((Window::Closed { taken }, _)) => taken,
        };
        let count = taken.len();
        let mut after: Vec<Option<Snapshot>> = Vec::with_capacity(count);
        for (index, windowed) in taken.iter().enumerate() {
            after.push(match windowed.incoming {
                Ok(Incoming::Committed { .. }) => match tried(index + 1)? {
                    Ok((_, snapshot)) => Some(snapshot),
                    Err(_) => None,
                },
                _ => None,
            });
        }
        let own_leaf = state.me.as_ref().map(|me| me.certificate.leaf.leaf_id.clone()).unwrap_or_default();
        let processed = self.store.with_tx(|provider, tx| -> Result<Vec<DmProcessed>, Error> {
            let (kind, _) = Self::group_row(tx, &group_id)?.ok_or_else(|| Error::Storage("a group this device isn't in".to_owned()))?;
            let (mut group, _) = Self::load(provider, tx, &group_id)?;
            let mut before = every_leaf(&group);
            let Window::Closed { taken } = group.process_window(provider, &state.pins, &bytes[..count])? else {
                return Err(Error::Storage("the window didn't close again".to_owned()));
            };
            let mut processed = Vec::with_capacity(count);
            for ((index, windowed), snapshot) in taken.into_iter().enumerate().zip(after) {
                let item = &items[index];
                let out = match windowed.incoming {
                    Ok(Incoming::Committed { epoch }) => {
                        let mut line = None;
                        if let Some(snapshot) = snapshot {
                            let next = snapshot.clone();
                            line = self.member_line(
                                tx,
                                &group_id,
                                item.seq,
                                item.arrived_ms,
                                false,
                                &own_leaf,
                                &state.account_id,
                                &listed_ids(&state.pins, &state.account_id),
                                std::mem::replace(&mut before, next),
                                snapshot,
                            )?;
                        }
                        DmProcessed { message: line, handshake: true, ..DmProcessed::committed(epoch) }
                    }
                    Ok(Incoming::Proposal) => DmProcessed { handshake: true, ..DmProcessed::of(DmOutcome::Proposal) },
                    Ok(Incoming::Application { from, content, .. }) => {
                        if from.kind() == LeafKind::Claktory {
                            DmProcessed {
                                why: "a Claktory's leaf can't say anything in a conversation".to_owned(),
                                ..DmProcessed::of(DmOutcome::Dropped)
                            }
                        } else {
                            match Content::decode(&content) {
                                Ok(content) => {
                                    let message = DmMessage {
                                        seq: item.seq,
                                        mine: false,
                                        from_account: from.account().to_owned(),
                                        from_leaf: from.leaf_id().to_owned(),
                                        from_kind_device: from.kind() == LeafKind::Device,
                                        content: DmContent::from_content(content),
                                        arrived_ms: item.arrived_ms,
                                        members: None,
                                        departed: windowed.departed,
                                    };
                                    self.take(tx, state, &kind, &group_id, &message)?;
                                    DmProcessed::message(message)
                                }
                                Err(error) => DmProcessed { why: error.to_string(), ..DmProcessed::of(DmOutcome::Dropped) },
                            }
                        }
                    }
                    // This device's own, which it can't decrypt: from what
                    // it kept, as [process] takes it.
                    Err(_) => match self.own_message(tx, state, &kind, &group_id, &envelopes[index], item.seq, item.arrived_ms)? {
                        Some(message) => DmProcessed::message(message),
                        None => DmProcessed::of(DmOutcome::Dropped),
                    },
                };
                processed.push(out);
            }
            let last = items[count - 1].seq;
            tx.execute("UPDATE groups SET seq = max(seq, ?2) WHERE id = ?1", params![group_id, last]).map_err(sql)?;
            Ok(processed)
        })?;
        Ok(DmWindowOutcome { end: DmWindowEnd::Closed, why: String::new(), processed })
    }

    /// This device's own message [envelope] coming back, from what it kept
    /// when it sealed it: taken, and no longer waiting to go. None when it
    /// isn't one of its own.
    #[allow(clippy::too_many_arguments)]
    /// Another's commit from the worker's log, taken as the feed takes one
    /// ([DmDevice::take_worker_commit]): None once taken, or what to answer.
    fn take_others(
        provider: &Provider<'_>,
        pins: &Pins,
        group: &mut Group,
        envelope: &Envelope,
    ) -> Result<Option<Result<DmProcessed, String>>, Error> {
        match group.process(provider, pins, envelope.bytes()) {
            Ok(Incoming::Committed { .. }) => Ok(None),
            // Framed as a commit, read as something else: dropped and
            // passed, never stopping the group (Fugo).
            Ok(_) => Ok(Some(Err("a commit that wasn't one".to_owned()))),
            Err(Error::UnknownIdentity { account }) => {
                Ok(Some(Ok(DmProcessed { why: account, handshake: true, ..DmProcessed::of(DmOutcome::Held) })))
            }
            Err(Error::Replaced { account }) => {
                Ok(Some(Ok(DmProcessed { why: account, handshake: true, replaced: true, ..DmProcessed::of(DmOutcome::Held) })))
            }
            Err(Error::Unlisted { account }) => Ok(Some(Ok(DmProcessed::unlisted(account, true)))),
            Err(error) if refusal(&error) => Ok(Some(Err(error.to_string()))),
            Err(error) => Err(error),
        }
    }

    /// Something of [group_id]'s feed, not this device's own commit, while
    /// it has no MLS state there: its rejoin waits for the worker, or the
    /// state is gone (E11-8). Before the rejoin's epoch, it's old: passed
    /// over. A commit at it is another's the worker took instead, so the
    /// rejoin's state put aside goes, and so does any commit at its epoch
    /// or later without one: this device joins again ([DmOutcome::Dropped]
    /// with `handshake`).
    fn stateless(&self, provider: &Provider<'_>, tx: &Transaction<'_>, group_id: &[u8], envelope: &Envelope) -> Result<Stateless, Error> {
        let commit = matches!(envelope.parsed().kind, envelope::Kind::Commit);
        let epoch = envelope.parsed().group.as_ref().map(|(_, epoch)| *epoch);
        if let Some(reached) = claktory_dm_crypto::storage::pending_rejoin(tx, group_id)? {
            if !commit || epoch.is_some_and(|epoch| epoch + 1 < reached) {
                return Ok(Stateless::Old);
            }
            self.give_up(provider, tx, group_id)?;
            return Ok(Stateless::Out("another commit came before this device's rejoin: it joins again".to_owned()));
        }
        if !commit {
            return Ok(Stateless::Old);
        }
        self.settle(tx, group_id)?;
        Ok(Stateless::Out("this device has no state of the group: it joins again".to_owned()))
    }

    fn own_message(
        &self,
        tx: &Transaction<'_>,
        state: &mut State,
        kind: &DmGroupKind,
        group_id: &[u8],
        envelope: &Envelope,
        seq: i64,
        arrived_ms: i64,
    ) -> Result<Option<DmMessage>, Error> {
        let digest = envelope.digest().to_vec();
        let own: Option<(String, Vec<u8>)> = tx
            .query_row("SELECT client_id, value FROM outgoing WHERE grp = ?1 AND digest = ?2", params![group_id, digest], |row| {
                Ok((row.get(0)?, row.get(1)?))
            })
            .optional()
            .map_err(sql)?;
        let Some((id, sealed)) = own else { return Ok(None) };
        let pending = self.store.open_row(&place(b"outgoing", group_id, id.as_bytes(), b"value"), &sealed)?;
        let pending: Pending = serde_json::from_slice(&pending).map_err(|e| Error::Format(e.to_string()))?;
        let me = state.me.as_ref().ok_or_else(|| Error::Storage("this device isn't set up".to_owned()))?;
        let message = DmMessage {
            seq,
            mine: true,
            from_account: me.certificate.leaf.account_id.clone(),
            from_leaf: me.certificate.leaf.leaf_id.clone(),
            from_kind_device: true,
            content: pending.content,
            arrived_ms,
            members: None,
            departed: false,
        };
        tx.execute("DELETE FROM outgoing WHERE grp = ?1 AND client_id = ?2", params![group_id, id]).map_err(sql)?;
        self.take(tx, state, kind, group_id, &message)?;
        Ok(Some(message))
    }

    /// Moves where this device is in the worker's feed of [group_id] to
    /// [seq]: what was there is gone (the worker pruned it).
    pub fn skip_proposals(&self, group_id: Vec<u8>, seq: i64) -> anyhow::Result<()> {
        let _state = self.state();
        self.store.with_tx(|_, tx| {
            if seq > self.proposal_position(tx, &group_id)? {
                self.save_row(tx, &proposal_place(&group_id).each(), &json(&seq)?)?;
            }
            Ok::<_, Error>(())
        })?;
        Ok(())
    }

    fn keep(&self, tx: &Transaction<'_>, group_id: &[u8], message: &DmMessage) -> Result<(), Error> {
        let seq = message.seq.to_be_bytes();
        let sealed = self.store.seal_row(&place(b"history", group_id, &seq, b"value"), &Zeroizing::new(json(message)?))?;
        tx.execute("INSERT OR REPLACE INTO history (grp, seq, value) VALUES (?1, ?2, ?3)", params![group_id, message.seq, sealed])
            .map_err(sql)?;
        Ok(())
    }

    /// Keeps [message] from [group_id] of [kind]: in a conversation or
    /// channel, as history; in the self group, what it says about a
    /// conversation goes to that conversation's view, and pins to the pins
    /// (Fugo: only from the person's own devices, which the self group's
    /// checks already make sure of).
    fn take(&self, tx: &Transaction<'_>, state: &mut State, kind: &DmGroupKind, group_id: &[u8], message: &DmMessage) -> Result<(), Error> {
        if kind.kind != DmGroupType::SelfGroup {
            return self.keep(tx, group_id, message);
        }
        // Only from the person's own devices, and never from a message
        // marked as a departed device's (Fugo 40397): pins are key-bearing.
        if !message.from_kind_device || message.from_account != state.account_id || message.departed {
            return Ok(());
        }
        let content = &message.content;
        let extra: serde_json::Map<String, serde_json::Value> = serde_json::from_str(&content.extra_json).unwrap_or_default();
        if content.kind == claktory_dm_crypto::content::kind::PINS {
            let Some(exported) = extra.get("pins") else { return Ok(()) };
            let mut theirs: Pins = serde_json::from_value(exported.clone()).map_err(|e| Error::Format(e.to_string()))?;
            theirs.reparse()?;
            let before = state.pins.clone();
            state.pins.import(&theirs)?;
            return self.save_pins(tx, &state.account_id, &before, &state.pins);
        }
        self.apply_view(tx, content, &extra)
    }

    /// A draft, read position or deletion for a conversation, from the self
    /// group (or this device's own, as it sends it).
    fn apply_view(
        &self,
        tx: &Transaction<'_>,
        content: &DmContent,
        extra: &serde_json::Map<String, serde_json::Value>,
    ) -> Result<(), Error> {
        use claktory_dm_crypto::content::kind;
        let Some(conversation) = extra.get("group").and_then(|group| group.as_str()) else { return Ok(()) };
        let Ok(conversation) = base64::Engine::decode(&base64::engine::general_purpose::URL_SAFE_NO_PAD, conversation) else {
            return Ok(());
        };
        let number = |key: &str| extra.get(key).and_then(serde_json::Value::as_i64).unwrap_or(0);
        let mut view = Self::read_view(&self.store, tx, &conversation)?;
        match content.kind.as_str() {
            kind::DRAFT => {
                let at = number("at_ms");
                if at < view.draft_ms {
                    return Ok(());
                }
                view.draft = content.text.clone().unwrap_or_default();
                view.draft_ms = at;
            }
            kind::READ => view.read_seq = view.read_seq.max(number("seq")),
            kind::DELETE => {
                let through = number("seq");
                if through <= view.deleted_seq {
                    return Ok(());
                }
                view.deleted_seq = through;
                view.read_seq = view.read_seq.max(through);
                tx.execute("DELETE FROM history WHERE grp = ?1 AND seq <= ?2", params![conversation, through]).map_err(sql)?;
            }
            _ => return Ok(()),
        }
        let name = view_name(&conversation);
        self.save_row(tx, &[b"rows", name.as_bytes()], &Zeroizing::new(json(&view)?))
    }

    pub(crate) fn read_view(store: &Store, tx: &Transaction<'_>, conversation: &[u8]) -> Result<DmView, Error> {
        let name = view_name(conversation);
        let sealed: Option<Vec<u8>> =
            tx.query_row("SELECT value FROM rows WHERE name = ?1", [name.as_bytes()], |row| row.get(0)).optional().map_err(sql)?;
        let Some(sealed) = sealed else { return Ok(DmView::default()) };
        let plain = store.open_row(&[b"rows", name.as_bytes()], &sealed)?;
        serde_json::from_slice(&plain).map_err(|e| Error::Format(e.to_string()))
    }

    /// What the person's devices keep of the conversation [group_id]: its
    /// draft, read position and deletion, through the self group.
    pub fn view(&self, group_id: Vec<u8>) -> anyhow::Result<DmView> {
        let _state = self.state();
        Ok(self.store.with_tx(|_, tx| Self::read_view(&self.store, tx, &group_id))?)
    }

    /// A line in the history for a commit at [seq] that changed who's in
    /// [group_id], unless only this device's own leaf moved (a rejoin).
    #[allow(clippy::too_many_arguments)]
    fn member_line(
        &self,
        tx: &Transaction<'_>,
        group_id: &[u8],
        seq: i64,
        arrived_ms: i64,
        mine: bool,
        own_leaf: &str,
        own_account: &str,
        own_listed: &[String],
        before: Snapshot,
        after: Snapshot,
    ) -> Result<Option<DmMessage>, Error> {
        // By every leaf in the tree, checked or not: a friend's leaf that
        // moved to their new key was there all along, under the old one.
        // A leaf is its id and its signature key (row 202, Fugo): one that
        // comes back under its id with a new key (a reinstall) went, and a
        // new one came.
        // And whose it is: its certificate's account goes with its id (row
        // 671, Fugo), so a friend's leaf under an id one of the person's
        // own has is the friend's, never taken for this device or theirs.
        let own = |account: &str, id: &str| account == own_account && id == own_leaf;
        let missing = |from: &[(DmMember, [u8; 32])], other: &Snapshot| -> Vec<DmMember> {
            from.iter()
                .filter(|(member, key)| !own(&member.account, &member.leaf_id) && !other.holds(member, key))
                .map(|(member, _)| member.clone())
                .collect()
        };
        // Another's leaves pass now and none did before, though they had
        // leaves here (one that moved, or one that no longer passes and
        // isn't the person's own: in a chat of two, theirs): their new key,
        // said at the first commit under it. Never for someone joining.
        let theirs_failed = before.ids.iter().any(|(account, id)| {
            !own(account, id)
                && !(account == own_account && own_listed.contains(id))
                && !before.roster.iter().any(|(old, _)| &old.account == account && &old.leaf_id == id)
        });
        let mut key_changed: Vec<String> = Vec::new();
        for (member, _) in &after.roster {
            let passed = before.roster.iter().any(|(old, _)| old.account == member.account);
            let had = before.has(&member.account, &member.leaf_id) || theirs_failed;
            if had && !passed && member.account != own_account && !key_changed.contains(&member.account) {
                key_changed.push(member.account.clone());
            }
        }
        // Who came: those that pass. Who went: every leaf that was there,
        // checked or not, since one taken off its list fails the checks
        // before the commit removing it.
        // A leaf back under an id that was here, with a new key, is the
        // device installed again (row 202): said once, as that; the former
        // install going under an id still here says nothing.
        let (set_up_again, added): (Vec<DmMember>, Vec<DmMember>) =
            missing(&after.roster, &before).into_iter().partition(|member| before.has(&member.account, &member.leaf_id));
        let removed = missing(&before.all, &after).into_iter().filter(|member| !after.has(&member.account, &member.leaf_id)).collect();
        // A device of an account with no leaf here before: it joined.
        let (joined, added): (Vec<DmMember>, Vec<DmMember>) = added
            .into_iter()
            .partition(|member| member.kind == DmLeafKind::Device && !before.ids.iter().any(|(account, _)| account == &member.account));
        let members = DmMembers { added, removed, key_changed, skipped: None, set_up_again, joined };
        if members.added.is_empty()
            && members.removed.is_empty()
            && members.key_changed.is_empty()
            && members.set_up_again.is_empty()
            && members.joined.is_empty()
        {
            return Ok(None);
        }
        let message = DmMessage {
            seq,
            mine,
            from_account: String::new(),
            from_leaf: String::new(),
            from_kind_device: false,
            content: DmContent {
                kind: String::new(),
                client_message_id: String::new(),
                text: None,
                by_agent: None,
                extra_json: String::new(),
            },
            arrived_ms,
            members: Some(members),
            departed: false,
        };
        self.keep(tx, group_id, &message)?;
        Ok(Some(message))
    }

    /// Who's in [group_id] now, as their certificates say (those that
    /// pass the checks now).
    pub fn members(&self, group_id: Vec<u8>) -> anyhow::Result<Vec<DmMember>> {
        let state = self.state();
        Ok(self.store.with_tx(|provider, tx| {
            Ok::<_, Error>(roster(&Self::load(provider, tx, &group_id)?.0, &state.pins).into_iter().map(|(member, _)| member).collect())
        })?)
    }

    /// Every leaf in [group_id] by id, checked or not: a Claktory's leaf
    /// still under the person's old key is there, though not a member that
    /// passes the checks until it moves.
    pub fn leaf_ids(&self, group_id: Vec<u8>) -> anyhow::Result<Vec<String>> {
        let _state = self.state();
        Ok(self.store.with_tx(|provider, tx| Ok::<_, Error>(Self::load(provider, tx, &group_id)?.0.leaf_ids()))?)
    }

    /// Every leaf in [group_id] as its certificate claims (account, kind,
    /// id, name), **unchecked**: for removal decisions only. A leaf taken
    /// off its list, a revoked Claktory's or one under a replaced key fails
    /// [members]' checks, yet must still be removed; trusting its claimed
    /// account is fine there, since removing only takes access away. Never
    /// for showing who's in or trusting what a leaf says ([members] does).
    pub fn leaves(&self, group_id: Vec<u8>) -> anyhow::Result<Vec<DmMember>> {
        let _state = self.state();
        Ok(self.store.with_tx(|provider, tx| {
            Ok::<_, Error>(unchecked(&Self::load(provider, tx, &group_id)?.0).into_iter().map(|(member, _)| member).collect())
        })?)
    }

    /// A proposal or an agent's draft in the channel [group_id], which
    /// come by their own routes rather than the feed: decrypted once and
    /// kept by its digest, so asked again it gives what it gave. Refused
    /// unless a member sent it under the group's checks.
    pub fn open_side(&self, group_id: Vec<u8>, envelope: String, arrived_ms: i64) -> anyhow::Result<DmMessage> {
        let envelope = read_envelope(&envelope)?;
        let digest = envelope.digest().to_vec();
        let state = self.state();
        Ok(self.store.with_tx(|provider, tx| {
            let kept: Option<Vec<u8>> = tx
                .query_row("SELECT value FROM side WHERE grp = ?1 AND digest = ?2", params![group_id, digest], |row| row.get(0))
                .optional()
                .map_err(sql)?;
            let at = place(b"side", &group_id, &digest, b"value");
            if let Some(kept) = kept {
                let plain = self.store.open_row(&at, &kept)?;
                return serde_json::from_slice(&plain).map_err(|e| Error::Format(e.to_string()));
            }
            let (mut group, _) = Self::load(provider, tx, &group_id)?;
            let Incoming::Application { from, content, .. } = group.process(provider, &state.pins, envelope.bytes())? else {
                return Err(Error::Refused("not something said".to_owned()));
            };
            let message = DmMessage {
                seq: 0,
                mine: false,
                from_account: from.account().to_owned(),
                from_leaf: from.leaf_id().to_owned(),
                from_kind_device: from.kind() == LeafKind::Device,
                content: DmContent::from_content(Content::decode(&content)?),
                arrived_ms,
                members: None,
                departed: false,
            };
            let sealed = self.store.seal_row(&at, &Zeroizing::new(json(&message)?))?;
            tx.execute("INSERT INTO side (grp, digest, value) VALUES (?1, ?2, ?3)", params![group_id, digest, sealed]).map_err(sql)?;
            Ok(message)
        })?)
    }

    /// The proposal or agent's draft with [digest] (SHA-256 of its MLS
    /// bytes) that came in the channel [group_id]'s feed; None until it
    /// has (#451).
    pub fn side_message(&self, group_id: Vec<u8>, digest: Vec<u8>) -> anyhow::Result<Option<DmMessage>> {
        let _state = self.state();
        Ok(self.store.with_tx(|_, tx| {
            let kept: Option<Vec<u8>> = tx
                .query_row("SELECT value FROM side WHERE grp = ?1 AND digest = ?2", params![group_id, digest], |row| row.get(0))
                .optional()
                .map_err(sql)?;
            let Some(kept) = kept else { return Ok(None) };
            let plain = self.store.open_row(&place(b"side", &group_id, &digest, b"value"), &kept)?;
            Ok::<_, Error>(Some(serde_json::from_slice(&plain).map_err(|e| Error::Format(e.to_string()))?))
        })?)
    }

    /// Keeps [message] as [group_id]'s side message [digest], opened.
    fn keep_side(&self, tx: &Transaction<'_>, group_id: &[u8], digest: &[u8], message: &DmMessage) -> Result<(), Error> {
        let sealed = self.store.seal_row(&place(b"side", group_id, digest, b"value"), &Zeroizing::new(json(message)?))?;
        tx.execute("INSERT OR IGNORE INTO side (grp, digest, value) VALUES (?1, ?2, ?3)", params![group_id, digest, sealed])
            .map_err(sql)?;
        Ok(())
    }

    /// [group_id]'s history, newest last: up to [limit] messages before
    /// [before_seq] (all the newest without it).
    pub fn history(&self, group_id: Vec<u8>, before_seq: Option<i64>, limit: u32) -> anyhow::Result<Vec<DmMessage>> {
        let _state = self.state();
        let rows: Vec<(i64, Vec<u8>)> = self.store.with_tx(|_, tx| {
            tx.prepare("SELECT seq, value FROM history WHERE grp = ?1 AND seq < ?2 ORDER BY seq DESC LIMIT ?3")
                .map_err(sql)?
                .query_map(params![group_id, before_seq.unwrap_or(i64::MAX), limit], |row| Ok((row.get(0)?, row.get(1)?)))
                .map_err(sql)?
                .collect::<Result<Vec<_>, _>>()
                .map_err(sql)
        })?;
        let mut messages = rows
            .into_iter()
            .map(|(seq, sealed)| {
                let plain = self.store.open_row(&place(b"history", &group_id, &seq.to_be_bytes(), b"value"), &sealed)?;
                Ok(serde_json::from_slice(&plain)?)
            })
            .collect::<anyhow::Result<Vec<DmMessage>>>()?;
        messages.reverse();
        Ok(messages)
    }
}

/// What a rotation message says in the person's self group (dms-e2ee
/// §2.1): the key to their new identity's bundle (which comes through
/// `me/handoffs`), that identity, and the devices it took out.
pub const ROTATION_KIND: &str = "rotation";

const ROTATION: &[&[u8]] = &[b"rows", b"rotation"];
const ROTATION_IN: &[&[u8]] = &[b"rows", b"rotation_in"];
const REMOVAL: &[&[u8]] = &[b"rows", b"removal"];
/// The person's devices removed here or off a list this device took, by
/// leaf: a code for one is refused (Abbacchio's row 612, Fugo).
const REMOVED: &[&[u8]] = &[b"rows", b"removed"];
/// The add codes this device has handed off to, by their digits: each
/// works once.
const USED_CODES: &[&[u8]] = &[b"rows", b"used_codes"];
/// The person's devices a start-over took out of their groups (row 630).
const REPLACED: &[&[u8]] = &[b"rows", b"replaced"];
/// The device lists this device took ([DmDevice::lists_adopted]), the
/// latest [LISTS_ADOPTED_KEPT].
const LISTS_ADOPTED: &[&[u8]] = &[b"rows", b"lists_adopted"];
const LISTS_ADOPTED_KEPT: usize = 512;
/// How many used codes are kept: each works for 30 minutes, so a few is
/// plenty.
const USED_CODES_KEPT: usize = 256;
/// Why a code is refused, as the person reads it.
const CODE_USED: &str = "This code was already used, or is for a device you removed.";

/// A removal the person confirmed (Remove…, or Lock out with a new
/// security key, dms-e2ee §2.1), kept in the sealed store from its start
/// until it's finished, so one stopped part-way (a dialog closed, the app
/// quit) carries on at the next launch (Abbacchio's R5b, Fugo).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DmRemoval {
    /// The person's devices being removed, by leaf.
    pub remove: Vec<String>,
    /// Their names when it started, in the same order: once off the list
    /// and out of the self group, nothing else names them.
    pub names: Vec<String>,
    /// The devices kept, for the new key.
    pub keep: Vec<String>,
    /// A new security key follows (Lock out): kept until the new key is
    /// taken here ([DmDevice::finish_rotation]), not only until signed out.
    pub new_key: bool,
}

/// A rotation under way on this device (dms-e2ee §2.1), kept in the sealed
/// store from its start to the new key being taken, so a crash carries on
/// with the same new key.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DmRotation {
    /// The new identity's public half; its secret is in the platform key
    /// store (`identity_next`) until [DmDevice::finish_rotation].
    pub identity_public: Vec<u8>,
    /// The person's device list, version 1, signed by the new key.
    pub device_list: Vec<u8>,
    /// The new identity and pins, sealed for the kept devices.
    pub sealed: Vec<u8>,
    /// The key to [sealed], for the self group.
    pub key: Vec<u8>,
    pub kept: Vec<String>,
    pub removed: Vec<String>,
    /// The rotation message went to the self group.
    pub said: bool,
}

/// A rotation another of the person's devices made, waiting for its
/// bundle and for the worker to serve its key.
#[derive(Debug, Clone)]
pub struct DmPendingRotation {
    pub from_leaf: String,
    /// That device's name, from its certificate.
    pub from_name: String,
    pub identity_public: Vec<u8>,
}

/// [DmPendingRotation] as the store keeps it, with its key.
#[frb(ignore)]
#[derive(Serialize, Deserialize)]
struct PendingRow {
    from_leaf: String,
    from_name: String,
    identity_public: Vec<u8>,
    key: Vec<u8>,
}

impl DmDevice {
    fn read_row<T: serde::de::DeserializeOwned>(&self, tx: &Transaction<'_>, place: &[&[u8]]) -> Result<Option<T>, Error> {
        let sealed: Option<Vec<u8>> =
            tx.query_row("SELECT value FROM rows WHERE name = ?1", [place[1]], |row| row.get(0)).optional().map_err(sql)?;
        let Some(sealed) = sealed else { return Ok(None) };
        let plain = self.store.open_row(place, &sealed)?;
        serde_json::from_slice(&plain).map(Some).map_err(|e| Error::Format(e.to_string()))
    }

    fn delete_row(tx: &Transaction<'_>, place: &[&[u8]]) -> Result<(), Error> {
        tx.execute("DELETE FROM rows WHERE name = ?1", [place[1]]).map_err(sql)?;
        Ok(())
    }

    /// The person's own pins once their key is [identity] with [list]:
    /// the old key retired, the change seen (it's theirs), the list taken.
    fn rotated_pins(state: &State, identity: IdentityPublic, list: &[u8]) -> Result<Pins, Error> {
        let mut pins = state.pins.clone();
        pins.key_changed(&state.account_id, identity)?;
        pins.retire_previous(&state.account_id);
        pins.seen(&state.account_id);
        pins.accept_list(&state.account_id, list)?;
        Ok(pins)
    }

    /// Starts changing the person's security key on this device (Lost or
    /// stolen, dms-e2ee §2.1), once the lost devices are removed: with
    /// [identity_key] (new, kept as `identity_next` first), the person's
    /// devices [kept] (confirmed by name) and [removed]. The self group
    /// must hold exactly this device and [kept], each on the current list
    /// with the signature key its leaf has there (Fugo: never a key only a
    /// list names). Asked again with the same key, the same rotation.
    pub fn prepare_rotation(
        &self,
        identity_key: Vec<u8>,
        kept: Vec<String>,
        removed: Vec<String>,
        now_ms: u64,
    ) -> anyhow::Result<DmRotation> {
        let state = self.state();
        let me = state.me.as_ref().ok_or_else(|| anyhow!("this device isn't set up"))?;
        let identity = IdentityKey::from_secret(&bytes32(&identity_key)?);
        let rotation = self.store.with_tx(|provider, tx| -> anyhow::Result<DmRotation> {
            if let Some(kept_before) = self.read_row::<DmRotation>(tx, ROTATION)? {
                if kept_before.identity_public == identity.public().0.to_vec() {
                    return Ok(kept_before);
                }
                bail!("another change of security key is under way");
            }
            if state.identity.as_ref().is_some_and(|current| current.public() == identity.public()) {
                bail!("that's the security key already");
            }
            let own = &me.certificate.leaf.leaf_id;
            let self_group: Option<Vec<u8>> = {
                let rows: Vec<(Vec<u8>, String)> = tx
                    .prepare("SELECT id, kind FROM groups")
                    .map_err(sql)?
                    .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
                    .map_err(sql)?
                    .collect::<Result<_, _>>()
                    .map_err(sql)?;
                rows.into_iter().find_map(|(id, kind)| {
                    serde_json::from_str::<DmGroupKind>(&kind).ok().filter(|kind| kind.kind == DmGroupType::SelfGroup).map(|_| id)
                })
            };
            let self_group = self_group.ok_or_else(|| anyhow!("no self group to hand the new key over"))?;
            let (group, _) = Self::load(provider, tx, &self_group)?;
            // A device being removed must be out of the self group first
            // (Abbacchio's R5: a removal that never committed).
            let every_leaf = group.leaf_ids();
            if removed.iter().any(|leaf| every_leaf.contains(leaf)) {
                bail!("a device being removed is still in your self group");
            }
            // Every leaf in the tree, checked: one that fails the checks is
            // a leaf this can't vouch for.
            let members = group
                .members(&state.pins)
                .into_iter()
                .collect::<Result<Vec<_>, Error>>()
                .map_err(|_| anyhow!("a device in your self group isn't on your device list any more; remove it first"))?;
            let mut in_tree: Vec<&str> = members.iter().map(|member| member.leaf_id()).collect();
            in_tree.sort_unstable();
            let mut expected: Vec<&str> = kept.iter().map(String::as_str).chain([own.as_str()]).collect();
            expected.sort_unstable();
            expected.dedup();
            if in_tree != expected {
                bail!("your self group doesn't hold exactly the devices you kept");
            }
            if every_leaf.len() != members.len() {
                bail!("a device being removed is still in your self group");
            }
            let list =
                state.pins.get(&state.account_id).and_then(|pin| pin.device_list.as_ref()).ok_or_else(|| anyhow!("no device list"))?;
            let list = SignedDeviceList::from_bytes(list)?;
            let leaves = members
                .iter()
                .map(|member| {
                    let leaf = &member.certificate.leaf;
                    if leaf.kind != LeafKind::Device || leaf.account_id != state.account_id {
                        bail!("a leaf in your self group isn't one of your devices");
                    }
                    if !list.list.leaves.iter().any(|listed| listed.leaf_id == leaf.leaf_id && listed.signature_key == leaf.signature_key) {
                        bail!("a device in your self group isn't on your device list");
                    }
                    Ok(Listed { kind: LeafKind::Device, leaf_id: leaf.leaf_id.clone(), signature_key: leaf.signature_key })
                })
                .collect::<anyhow::Result<Vec<_>>>()?;
            let device_list =
                DeviceList { account_id: state.account_id.clone(), version: 1, created_ms: now_ms, leaves }.sign(&identity)?.to_bytes()?;
            let pins = Self::rotated_pins(&state, identity.public(), &device_list)?;
            let sealed = handoff::seal_rotation(&state.account_id, &identity, &pins)?;
            let rotation = DmRotation {
                identity_public: identity.public().0.to_vec(),
                device_list,
                sealed: sealed.bytes,
                key: sealed.key.to_vec(),
                kept,
                removed,
                said: false,
            };
            self.save_row(tx, ROTATION, &Zeroizing::new(json(&rotation)?))?;
            Ok(rotation)
        })?;
        Ok(rotation)
    }

    /// Keeps [removal] as the one under way, in place of any before.
    pub fn begin_removal(&self, removal: DmRemoval) -> anyhow::Result<()> {
        let _state = self.state();
        self.store.with_tx(|_, tx| -> Result<(), Error> {
            // Removed for good, whatever becomes of the removal.
            self.note_removed(tx, &removal.remove)?;
            self.save_row(tx, REMOVAL, &json(&removal)?)
        })?;
        Ok(())
    }

    /// The removal under way on this device, if any.
    pub fn removal(&self) -> anyhow::Result<Option<DmRemoval>> {
        let _state = self.state();
        Ok(self.store.with_tx(|_, tx| self.read_row(tx, REMOVAL))?)
    }

    /// The removal under way is done (signed out, with no new key to
    /// follow).
    pub fn end_removal(&self) -> anyhow::Result<()> {
        let _state = self.state();
        self.store.with_tx(|_, tx| Self::delete_row(tx, REMOVAL))?;
        Ok(())
    }

    /// The rotation under way on this device, if any.
    pub fn rotation(&self) -> anyhow::Result<Option<DmRotation>> {
        let _state = self.state();
        Ok(self.store.with_tx(|_, tx| self.read_row(tx, ROTATION))?)
    }

    /// The rotation message went to the self group: from now on the same
    /// new key is used, never another.
    pub fn rotation_said(&self) -> anyhow::Result<()> {
        let _state = self.state();
        self.store.with_tx(|_, tx| -> Result<(), Error> {
            let Some(mut rotation) = self.read_row::<DmRotation>(tx, ROTATION)? else { return Ok(()) };
            rotation.said = true;
            self.save_row(tx, ROTATION, &Zeroizing::new(json(&rotation)?))
        })?;
        Ok(())
    }

    /// Gives up on a rotation that never reached the self group.
    pub fn drop_rotation(&self) -> anyhow::Result<()> {
        let _state = self.state();
        self.store.with_tx(|_, tx| Self::delete_row(tx, ROTATION))?;
        Ok(())
    }

    /// The worker took the new key: this device takes it too, with its
    /// leaf certified under it (same leaf, same signature key) and the
    /// person's pins on it. Gives the new certificate.
    pub fn finish_rotation(&self, identity_key: Vec<u8>) -> anyhow::Result<Vec<u8>> {
        let mut state = self.state();
        let identity = IdentityKey::from_secret(&bytes32(&identity_key)?);
        let rotation: DmRotation =
            self.store.with_tx(|_, tx| self.read_row(tx, ROTATION))?.ok_or_else(|| anyhow!("no rotation under way"))?;
        if rotation.identity_public != identity.public().0.to_vec() {
            bail!("not the key this rotation made");
        }
        let pins = Self::rotated_pins(&state, identity.public(), &rotation.device_list)?;
        let (me, certificate) = Self::recertify(&state, &identity)?;
        self.store.with_tx(|_, tx| -> Result<(), Error> {
            self.save_row(tx, ME, &json(&Kept { account_id: state.account_id.clone(), certificate: certificate.clone() })?)?;
            self.save_pins(tx, &state.account_id, &state.pins, &pins)?;
            Self::delete_row(tx, ROTATION)?;
            // The Lock out it finishes, with it.
            Self::delete_row(tx, REMOVAL)
        })?;
        state.pins = pins;
        state.me = Some(me);
        state.identity = Some(identity);
        Ok(certificate)
    }

    /// This device's leaf certified again under [identity]: the same leaf
    /// in every other way.
    fn recertify(state: &State, identity: &IdentityKey) -> anyhow::Result<(Me, Vec<u8>)> {
        let me = state.me.as_ref().ok_or_else(|| anyhow!("this device isn't set up"))?;
        let certificate = LeafCertificate::issue(identity, me.certificate.leaf.clone())?;
        let bytes = certificate.to_bytes()?;
        Ok((me.recertified(certificate)?, bytes))
    }

    /// A rotation message in the self group (Fugo's checks): from one of
    /// the person's own devices, not one it removed, none of which is in
    /// the self group now, for a key that's neither theirs now nor one
    /// they had before. Kept to act on once its bundle comes and the
    /// worker serves its key; anything else is ignored.
    fn keep_rotation(&self, tx: &Transaction<'_>, state: &State, group: &Group, message: &DmMessage, name: &str) -> Result<(), Error> {
        if !message.from_kind_device || message.from_account != state.account_id || message.mine {
            return Ok(());
        }
        let extra: serde_json::Map<String, serde_json::Value> = serde_json::from_str(&message.content.extra_json).unwrap_or_default();
        let b64 = |key: &str| {
            extra
                .get(key)
                .and_then(serde_json::Value::as_str)
                .and_then(|text| base64::Engine::decode(&base64::engine::general_purpose::URL_SAFE_NO_PAD, text).ok())
        };
        let (Some(key), Some(identity)) = (b64("key"), b64("identity")) else { return Ok(()) };
        let removed: Vec<String> = extra
            .get("removed")
            .and_then(serde_json::Value::as_array)
            .map(|list| list.iter().filter_map(|leaf| leaf.as_str().map(str::to_owned)).collect())
            .unwrap_or_default();
        if key.len() != 32 || removed.contains(&message.from_leaf) {
            return Ok(());
        }
        let in_tree = group.leaf_ids();
        if removed.iter().any(|leaf| in_tree.contains(leaf)) {
            return Ok(());
        }
        let Ok(identity) = IdentityPublic::from_slice(&identity) else { return Ok(()) };
        let Some(pin) = state.pins.get(&state.account_id) else { return Ok(()) };
        if pin.identity == identity || pin.replaced(&identity) {
            return Ok(());
        }
        let pending =
            PendingRow { from_leaf: message.from_leaf.clone(), from_name: name.to_owned(), identity_public: identity.0.to_vec(), key };
        self.save_row(tx, ROTATION_IN, &Zeroizing::new(json(&pending)?))
    }

    /// Another of the person's devices changed their security key, and
    /// this one hasn't taken it yet.
    pub fn pending_rotation(&self) -> anyhow::Result<Option<DmPendingRotation>> {
        let _state = self.state();
        let row: Option<PendingRow> = self.store.with_tx(|_, tx| self.read_row(tx, ROTATION_IN))?;
        Ok(row.map(|row| DmPendingRotation { from_leaf: row.from_leaf, from_name: row.from_name, identity_public: row.identity_public }))
    }

    /// Opens the new key's bundle [sealed] (from `me/handoffs`) with the
    /// pending rotation's key: the new identity's secret, for the platform
    /// key store (`identity_next`) before [DmDevice::take_rotation].
    /// Nothing here changes.
    pub fn open_rotation(&self, sealed: Vec<u8>) -> anyhow::Result<Vec<u8>> {
        let state = self.state();
        let pending: PendingRow =
            self.store.with_tx(|_, tx| self.read_row(tx, ROTATION_IN))?.ok_or_else(|| anyhow!("no new key waiting"))?;
        let rotated = handoff::open_rotation(&state.account_id, &bytes32(&pending.key)?, &sealed)?;
        if rotated.identity.public().0.to_vec() != pending.identity_public {
            bail!("the bundle has another key than the message named");
        }
        Ok(rotated.identity.secret().to_vec())
    }

    /// Takes the new key ([identity_key], from [DmDevice::open_rotation],
    /// in the key store already) with the bundle [sealed]'s pins: this
    /// device's leaf certified under it, the old key retired. Gives the new
    /// certificate.
    pub fn take_rotation(&self, identity_key: Vec<u8>, sealed: Vec<u8>) -> anyhow::Result<Vec<u8>> {
        let mut state = self.state();
        let pending: PendingRow =
            self.store.with_tx(|_, tx| self.read_row(tx, ROTATION_IN))?.ok_or_else(|| anyhow!("no new key waiting"))?;
        let rotated = handoff::open_rotation(&state.account_id, &bytes32(&pending.key)?, &sealed)?;
        let identity = IdentityKey::from_secret(&bytes32(&identity_key)?);
        if rotated.identity.public() != identity.public() || identity.public().0.to_vec() != pending.identity_public {
            bail!("the bundle has another key than the message named");
        }
        let mut pins = state.pins.clone();
        pins.import(&rotated.pins)?;
        pins.seen(&state.account_id);
        if pins.get(&state.account_id).map(|pin| pin.identity) != Some(identity.public()) {
            bail!("the bundle's pins don't have your new key");
        }
        let (me, certificate) = Self::recertify(&state, &identity)?;
        self.store.with_tx(|_, tx| -> Result<(), Error> {
            self.save_row(tx, ME, &json(&Kept { account_id: state.account_id.clone(), certificate: certificate.clone() })?)?;
            self.save_pins(tx, &state.account_id, &state.pins, &pins)?;
            Self::delete_row(tx, ROTATION_IN)
        })?;
        state.pins = pins;
        state.me = Some(me);
        state.identity = Some(identity);
        Ok(certificate)
    }

    /// Forgets a pending rotation that can't be taken (its bundle never
    /// came, or didn't open).
    pub fn drop_pending_rotation(&self) -> anyhow::Result<()> {
        let _state = self.state();
        self.store.with_tx(|_, tx| Self::delete_row(tx, ROTATION_IN))?;
        Ok(())
    }
}

/// Who's in a group around a commit: the members that pass the checks
/// (to name), and every leaf as it claims (to tell who came and went).
/// A leaf is its certificate's account, its id and its signature key
/// (rows 202 and 671, Fugo).
#[frb(ignore)]
#[derive(Default)]
#[derive(Clone)]
struct Snapshot {
    /// Those that pass the checks, each with its signature key.
    roster: Vec<(DmMember, [u8; 32])>,
    /// Every leaf's account and id, checked or not.
    ids: Vec<(String, String)>,
    /// Every leaf with what it claims, checked or not: for who went, since
    /// a leaf taken off its list fails the checks before its removal
    /// commits (Fugo).
    all: Vec<(DmMember, [u8; 32])>,
}

impl Snapshot {
    /// Whether a leaf of [account]'s with the id [leaf_id] is here, under
    /// any key.
    fn has(&self, account: &str, leaf_id: &str) -> bool {
        self.ids.iter().any(|(a, id)| a == account && id == leaf_id)
    }

    /// Whether [member]'s leaf is here under [key]: the same leaf.
    fn holds(&self, member: &DmMember, key: &[u8; 32]) -> bool {
        self.all.iter().any(|(m, k)| m.account == member.account && m.leaf_id == member.leaf_id && k == key)
    }
}

/// The leaf ids on [account]'s device list as this device took it.
fn listed_ids(pins: &Pins, account: &str) -> Vec<String> {
    pins.get(account)
        .and_then(|pin| pin.device_list.as_ref())
        .and_then(|bytes| SignedDeviceList::from_bytes(bytes).ok())
        .map(|signed| signed.list.leaves.into_iter().map(|leaf| leaf.leaf_id).collect())
        .unwrap_or_default()
}

fn snapshot(group: &Group, pins: &Pins) -> Snapshot {
    let all = unchecked(group);
    Snapshot {
        roster: roster(group, pins),
        ids: all.iter().map(|(member, _)| (member.account.clone(), member.leaf_id.clone())).collect(),
        all,
    }
}

/// Who's in [group], every leaf as if it passed: for the lines of a
/// window (row 561), whose departed device never passes the latest list
/// yet was there, as an online member saw it.
fn every_leaf(group: &Group) -> Snapshot {
    let all = unchecked(group);
    Snapshot { roster: all.clone(), ids: all.iter().map(|(member, _)| (member.account.clone(), member.leaf_id.clone())).collect(), all }
}

/// Every leaf of [group] with what its certificate claims, unchecked: for
/// removal decisions and who went, never for trust.
fn unchecked(group: &Group) -> Vec<(DmMember, [u8; 32])> {
    group
        .leaves_unchecked()
        .into_iter()
        .map(|leaf| {
            let key = leaf.signature_key;
            (
                DmMember {
                    account: leaf.account_id,
                    leaf_id: leaf.leaf_id,
                    kind: leaf.kind.into(),
                    name: leaf.name,
                    since_ms: leaf.created_ms,
                },
                key,
            )
        })
        .collect()
}

/// The members of [group] that pass the checks now.
/// In a conversation, the other person when none of their devices passes
/// the checks in [group] (row 676): a stale or dead leaf, or one off their
/// list, doesn't count (Fugo). None for any other group.
fn not_joined(kind: &DmGroupKind, group: &Group, pins: &Pins, own: &str) -> Option<String> {
    if kind.kind != DmGroupType::Conversation {
        return None;
    }
    let present = roster(group, pins);
    kind.accounts
        .iter()
        .find(|account| {
            account.as_str() != own && !present.iter().any(|(member, _)| &member.account == *account && member.kind == DmLeafKind::Device)
        })
        .cloned()
}

fn roster(group: &Group, pins: &Pins) -> Vec<(DmMember, [u8; 32])> {
    group
        .members(pins)
        .into_iter()
        .flatten()
        .map(|member| {
            (
                DmMember {
                    account: member.account().to_owned(),
                    leaf_id: member.leaf_id().to_owned(),
                    kind: member.kind().into(),
                    name: member.certificate.leaf.name.clone(),
                    since_ms: member.certificate.leaf.created_ms,
                },
                member.certificate.leaf.signature_key,
            )
        })
        .collect()
}

/// Where a conversation's view is kept in `rows`.
pub(crate) fn view_name(conversation: &[u8]) -> String {
    format!("view:{}", base64::Engine::encode(&base64::engine::general_purpose::URL_SAFE_NO_PAD, conversation))
}

fn openmls_group_id(group_id: &[u8]) -> openmls::prelude::GroupId {
    openmls::prelude::GroupId::from_slice(group_id)
}

/// A change that always commits something ([DmDevice::change_group]).
fn made(commit: Result<Option<DmCommit>, DmGroupError>) -> Result<DmCommit, DmGroupError> {
    commit?.ok_or_else(|| DmGroupError::of(DmGroupErrorKind::Failed, &[], "nothing to commit"))
}

fn commit(out: claktory_dm_crypto::group::Outgoing) -> anyhow::Result<DmCommit> {
    Ok(DmCommit {
        commit: envelope_json(out.commit)?,
        welcome: out.welcome.map(envelope_json).transpose()?,
        group_info: out.group_info.map(envelope_json).transpose()?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn folder() -> PathBuf {
        // Apart for every test: the clock alone can repeat across threads.
        static NEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("claktory-dm-api-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// A device with a store in [dir] named [name].
    fn device(dir: &std::path::Path, name: &str) -> DmDevice {
        DmDevice::open(
            dir.join(format!("{name}.db")).to_string_lossy().into_owned(),
            dm_new_secret().unwrap(),
            dm_new_secret().unwrap(),
            None,
        )
        .unwrap()
        .unwrap()
    }

    /// A new device's leaf id: its 32-byte account key, base64url.
    const TABLET: &str = "dGFibGV0LXRhYmxldC10YWJsZXQtdGFibGV0LXRhYmw";

    fn text(id: &str, text: &str) -> DmContent {
        DmContent { kind: "text".into(), client_message_id: id.into(), text: Some(text.into()), by_agent: None, extra_json: String::new() }
    }

    /// Matt's phone and Ana's iPad, each set up, each pinning the other.
    fn two(dir: &std::path::Path) -> (DmDevice, DmDevice) {
        let (phone, ipad) = (device(dir, "phone"), device(dir, "ipad"));
        let matt = phone.set_up_first(dm_new_secret().unwrap(), "matt".into(), "phone".into(), "iPhone".into(), 1).unwrap();
        let ana = ipad.set_up_first(dm_new_secret().unwrap(), "ana".into(), "ipad".into(), "iPad".into(), 1).unwrap();
        phone.first_contact("ana".into(), ana.identity_public.clone()).unwrap();
        phone.accept_list("ana".into(), ana.device_list).unwrap();
        ipad.first_contact("matt".into(), matt.identity_public.clone()).unwrap();
        ipad.accept_list("matt".into(), matt.device_list).unwrap();
        (phone, ipad)
    }

    fn chat() -> DmGroupKind {
        DmGroupKind { kind: DmGroupType::Conversation, accounts: vec!["ana".into(), "matt".into()], claktory_id: String::new() }
    }

    /// Matt's phone and Ana's iPad in a conversation at epoch 1, the worker
    /// having taken the add at seq 1; the add's commit (its GroupInfo is of
    /// epoch 1).
    fn chatting(dir: &std::path::Path) -> (DmDevice, DmDevice, Vec<u8>, DmCommit) {
        let (phone, ipad) = two(dir);
        let id = b"conversation-1".to_vec();
        phone.create_group(id.clone(), chat()).unwrap();
        let package = ipad.key_packages(1).unwrap().remove(0);
        let added = phone.add_members(id.clone(), vec![package]).unwrap().expect("something to add");
        phone.process(id.clone(), 1, added.commit.clone(), 10).unwrap();
        ipad.join_group(id.clone(), chat(), added.welcome.clone().unwrap(), 1).unwrap();
        (phone, ipad, id, added)
    }

    /// The device's state of the group as GET groups shows the worker's:
    /// its epoch and confirmation tag (base64url).
    fn state_of(device: &DmDevice, id: &[u8]) -> (u64, String) {
        use base64::Engine as _;
        device
            .store
            .with_tx(|provider, tx| {
                let (group, _) = DmDevice::load(provider, tx, id)?;
                Ok::<_, Error>((group.epoch(), base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(group.confirmation_tag()?)))
            })
            .unwrap()
    }

    fn floor(device: &DmDevice, id: &[u8]) -> u64 {
        device.store.with_tx(|_, tx| claktory_dm_crypto::storage::epoch_floor(tx, id)).unwrap()
    }

    /// Ages the device's waiting commit to [id] past [PENDING_FOR].
    fn age_commit(device: &DmDevice, id: &[u8]) {
        device.store.with_tx(|_, tx| device.save_row(tx, &commit_place(id).each(), &json(&0i64)?)).unwrap();
    }

    /// E11-8, the lost answer: the iPad rejoins, the worker restarts and
    /// loses its commit, and the phone's commit is taken at that epoch
    /// instead. The iPad never forks onto an epoch of its own: no state
    /// until the worker takes its commit, its floor where it was; it finds
    /// it's out of step, joins again past the phone's commit, and is in
    /// step.
    #[test]
    fn a_rejoin_whose_answer_is_lost_never_forks_the_device() {
        let dir = folder();
        let (phone, ipad, id, added) = chatting(&dir);
        let lost = ipad.rejoin_group(id.clone(), chat(), added.group_info.clone().unwrap(), 1).unwrap();
        let mine = |device: &DmDevice| device.groups().unwrap().into_iter().find(|group| group.group_id == id).unwrap();
        assert_eq!((mine(&ipad).epoch, mine(&ipad).pending), (0, true), "its state waits aside");
        assert_eq!(floor(&ipad, &id), 1, "not raised before the worker takes it");

        // The worker took the phone's commit at seq 2 instead.
        let update = phone.update_group(id.clone()).unwrap();
        phone.commit_taken(id.clone()).unwrap();
        let (epoch, tag) = state_of(&phone, &id);
        assert_eq!(epoch, 2);
        assert_eq!(ipad.fork_check(id.clone(), epoch, tag.clone()).unwrap(), DmForkState::Pending, "a fresh commit waits");
        let refused = ipad.update_group(id.clone()).unwrap_err();
        assert_eq!(refused.kind, DmGroupErrorKind::Pending);
        age_commit(&ipad, &id);
        assert_eq!(ipad.fork_check(id.clone(), epoch, tag.clone()).unwrap(), DmForkState::Behind, "the feed settles it");

        // The phone's commit at the rejoin's epoch: the rejoin's state goes,
        // and it joins again.
        let taken = ipad.process(id.clone(), 2, update.commit.clone(), 20).unwrap();
        assert_eq!((taken.outcome, taken.handshake), (DmOutcome::Dropped, true), "{taken:?}");
        assert!(!mine(&ipad).pending);
        assert_eq!(floor(&ipad, &id), 1);
        assert_eq!(ipad.fork_check(id.clone(), epoch, tag.clone()).unwrap(), DmForkState::Forked);
        let refused = ipad.send(id.clone(), text("a1", "hello?"), 21, vec![]).unwrap_err();
        assert_eq!(refused.kind, DmGroupErrorKind::Forked);
        // Its first commit, if the worker had it after all, is never taken
        // back now.
        assert!(lost.commit != update.commit);

        let resync = ipad.resync_group(id.clone(), update.group_info.clone().unwrap(), 2).unwrap();
        assert!(resync.resend.is_empty());
        assert_eq!(phone.process(id.clone(), 3, resync.commit.commit.clone(), 30).unwrap().outcome, DmOutcome::Committed);
        let back = ipad.process(id.clone(), 3, resync.commit.commit.clone(), 30).unwrap();
        assert_eq!((back.outcome, back.epoch), (DmOutcome::Committed, 3));
        let (epoch, tag) = state_of(&phone, &id);
        assert_eq!(ipad.fork_check(id.clone(), epoch, tag).unwrap(), DmForkState::InStep);
        assert_eq!(state_of(&ipad, &id), state_of(&phone, &id));
        assert_eq!(floor(&ipad, &id), 3);
        let said = ipad.send(id.clone(), text("a1", "back"), 31, vec![]).unwrap();
        let heard = phone.process(id.clone(), 4, said, 40).unwrap().message.unwrap();
        assert_eq!(heard.content.text.as_deref(), Some("back"));
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// E11-8: the worker took the rejoin's commit but the answer was lost;
    /// it comes back in the feed and is taken by its digest.
    #[test]
    fn a_rejoin_taken_but_unheard_is_taken_from_the_feed() {
        let dir = folder();
        let (phone, ipad, id, added) = chatting(&dir);
        let rejoined = ipad.rejoin_group(id.clone(), chat(), added.group_info.clone().unwrap(), 1).unwrap();
        assert_eq!(phone.process(id.clone(), 2, rejoined.commit.clone(), 20).unwrap().outcome, DmOutcome::Committed);
        age_commit(&ipad, &id);
        let (epoch, tag) = state_of(&phone, &id);
        assert_eq!(ipad.fork_check(id.clone(), epoch, tag.clone()).unwrap(), DmForkState::Behind);
        assert_eq!(ipad.process(id.clone(), 2, rejoined.commit, 20).unwrap().outcome, DmOutcome::Committed);
        assert_eq!(ipad.fork_check(id.clone(), epoch, tag).unwrap(), DmForkState::InStep);
        assert_eq!(floor(&ipad, &id), 2);
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// E11-8: a commit that waited past [PENDING_FOR] with the worker still
    /// at its epoch was never taken: given up, and the device is in step.
    #[test]
    fn a_commit_the_worker_never_took_is_given_up() {
        let dir = folder();
        let (phone, ipad, id, _) = chatting(&dir);
        ipad.update_group(id.clone()).unwrap();
        let (epoch, tag) = state_of(&phone, &id);
        assert_eq!(ipad.fork_check(id.clone(), epoch, tag.clone()).unwrap(), DmForkState::Pending);
        age_commit(&ipad, &id);
        assert_eq!(ipad.fork_check(id.clone(), epoch, tag.clone()).unwrap(), DmForkState::PendingCleared);
        assert_eq!(ipad.fork_check(id.clone(), epoch, tag).unwrap(), DmForkState::InStep);
        assert!(ipad.update_group(id.clone()).is_ok(), "changes go on");
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// E11-8, the floor's way out: the worker lost an epoch
    /// the iPad merged, so its GroupInfo is below the iPad's floor and the
    /// iPad can't rejoin (stuck). Another member's empty commit moves the
    /// group past it, and it rejoins.
    #[test]
    fn a_device_stuck_below_its_floor_rejoins_once_a_member_moves_the_group_on() {
        let dir = folder();
        let (phone, ipad, id, added) = chatting(&dir);
        // The phone's update reached the iPad, then the worker lost it, and
        // the phone heard it refused.
        let update = phone.update_group(id.clone()).unwrap();
        ipad.process(id.clone(), 2, update.commit, 20).unwrap();
        phone.commit_refused(id.clone()).unwrap();
        assert_eq!(floor(&ipad, &id), 2);
        let (epoch, tag) = state_of(&phone, &id);
        assert_eq!(epoch, 1);
        assert_eq!(ipad.fork_check(id.clone(), epoch, tag).unwrap(), DmForkState::Forked);
        let stuck = ipad.resync_group(id.clone(), added.group_info.clone().unwrap(), 1).unwrap_err();
        assert_eq!((stuck.kind, stuck.stuck), (DmGroupErrorKind::Forked, true), "{stuck:?}");
        let refused = ipad.send(id.clone(), text("a1", "hi"), 21, vec![]).unwrap_err();
        assert_eq!((refused.kind, refused.stuck), (DmGroupErrorKind::Forked, true));

        // Told it's out of step, the phone makes an empty commit.
        let moved = phone.update_group(id.clone()).unwrap();
        phone.commit_taken(id.clone()).unwrap();
        let resync = ipad.resync_group(id.clone(), moved.group_info.unwrap(), 2).unwrap();
        phone.process(id.clone(), 3, resync.commit.commit.clone(), 30).unwrap();
        ipad.commit_taken(id.clone()).unwrap();
        let (epoch, tag) = state_of(&phone, &id);
        assert_eq!(ipad.fork_check(id.clone(), epoch, tag).unwrap(), DmForkState::InStep);
        assert!(ipad.send(id.clone(), text("a1", "hi"), 31, vec![]).is_ok());
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// E11-8: a send waits while a member's list here is older than the
    /// worker's, naming whose; each list taken is logged, without keys.
    #[test]
    fn a_send_on_an_old_list_is_refused_and_lists_taken_are_logged() {
        let dir = folder();
        let (phone, _ipad, id, _) = chatting(&dir);
        let newer = vec![DmListVersion { account: "ana".into(), version: 2 }];
        let stale = phone.send(id.clone(), text("m1", "hi"), 20, newer).unwrap_err();
        assert_eq!((stale.kind, stale.accounts.clone()), (DmGroupErrorKind::ListsStale, vec!["ana".to_owned()]));
        let same = vec![DmListVersion { account: "ana".into(), version: 1 }];
        assert!(phone.send(id.clone(), text("m1", "hi"), 21, same).is_ok());
        let ahead = vec![DmListVersion { account: "matt".into(), version: 9 }];
        let refused = phone.remove_dead_leaves(id.clone(), ahead).unwrap_err();
        assert_eq!((refused.kind, refused.accounts), (DmGroupErrorKind::ListsStale, vec!["matt".to_owned()]));
        let adopted = phone.lists_adopted().unwrap();
        assert!(adopted.iter().any(|each| each.account == "ana" && each.version == 1), "{adopted:?}");
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// F4.2: the device's own key packages past their lifetime are deleted,
    /// never one still in date.
    #[test]
    fn key_packages_past_their_lifetime_are_swept() {
        let dir = folder();
        let (_phone, ipad) = two(&dir);
        ipad.key_packages(3).unwrap();
        ipad.last_resort_key_package().unwrap();
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs() as i64;
        assert_eq!(ipad.sweep_key_packages(now).unwrap(), 0, "all in date");
        let lifetime = claktory_dm_crypto::group::KEY_PACKAGE_LIFETIME as i64;
        assert_eq!(ipad.sweep_key_packages(now + lifetime + 3600).unwrap(), 4, "the three and the last resort");
        assert_eq!(ipad.sweep_key_packages(now + lifetime + 3600).unwrap(), 0, "gone already");
        assert!(ipad.sweep_key_packages(-1).is_err());
        let week = 7 * 24 * 60 * 60;
        assert!(!ipad.last_resort_due(0, now), "the worker doesn't say");
        assert!(!ipad.last_resort_due(now + week + 60, now));
        assert!(ipad.last_resort_due(now + week, now));
        assert!(ipad.last_resort_due(now - 60, now), "past its end");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn two_people_talk_each_message_once_and_in_order() {
        let dir = folder();
        let (phone, ipad) = two(&dir);
        let id = b"conversation-1".to_vec();
        let info = read_envelope(&phone.create_group(id.clone(), chat()).unwrap()).unwrap();
        assert_eq!(info.parsed().kind, claktory_dm_crypto::envelope::Kind::GroupInfo);
        assert!(read_envelope(&ipad.last_resort_key_package().unwrap()).is_ok());
        let package = ipad.key_packages(1).unwrap().remove(0);
        let added = phone.add_members(id.clone(), vec![package]).unwrap().expect("something to add");
        // The worker took the Add at seq 1; it comes back to the phone.
        let back = phone.process(id.clone(), 1, added.commit.clone(), 10).unwrap();
        assert_eq!((back.outcome, back.epoch), (DmOutcome::Committed, 1));
        // Who came, as a line of its own, from Ana's certificate: her
        // first device here, so she joined (a later one is added).
        let came = back.message.unwrap().members.unwrap();
        assert_eq!((came.joined[0].account.as_str(), came.joined[0].name.as_str()), ("ana", "iPad"));
        assert!(came.added.is_empty());
        assert!(came.removed.is_empty());
        assert_eq!(phone.members(id.clone()).unwrap().len(), 2);
        ipad.join_group(id.clone(), chat(), added.welcome.unwrap(), 1).unwrap();

        // Encrypted once: a retry gives the same bytes.
        let said = phone.send(id.clone(), text("m1", "hi Ana"), 20, vec![]).unwrap();
        assert_eq!(phone.send(id.clone(), text("m1", "hi Ana"), 21, vec![]).unwrap(), said);
        assert_eq!(phone.unsent().unwrap()[0].content.text.as_deref(), Some("hi Ana"));

        // At seq 2 for both: Ana reads it, and the phone knows it as its own.
        let heard = ipad.process(id.clone(), 2, said.clone(), 30).unwrap().message.unwrap();
        assert_eq!((heard.from_account.as_str(), heard.mine, heard.content.text.as_deref()), ("matt", false, Some("hi Ana")));
        // The sender's clock rides inside (row 556), and their own copy says it too.
        let sent_ms = |message: &DmMessage| -> Option<i64> {
            serde_json::from_str::<serde_json::Value>(&message.content.extra_json).ok()?.get("sent_ms")?.as_i64()
        };
        assert_eq!(sent_ms(&heard), Some(20));
        let mine = phone.process(id.clone(), 2, said.clone(), 30).unwrap().message.unwrap();
        assert!(mine.mine);
        assert_eq!(sent_ms(&mine), Some(20));
        assert!(phone.unsent().unwrap().is_empty());
        // Again: already.
        assert_eq!(ipad.process(id.clone(), 2, said, 31).unwrap().outcome, DmOutcome::Already);

        // Ana answers; both keep the history, sealed, across a reopen.
        let answer = ipad.send(id.clone(), text("a1", "hi Matt"), 40, vec![]).unwrap();
        for device in [&phone, &ipad] {
            assert_eq!(device.process(id.clone(), 3, answer.clone(), 50).unwrap().outcome, DmOutcome::Message);
        }
        let texts = |device: &DmDevice| -> Vec<String> {
            device.history(id.clone(), None, 10).unwrap().into_iter().filter_map(|m| m.content.text).collect()
        };
        assert_eq!(texts(&phone), ["hi Ana", "hi Matt"]);
        assert_eq!(texts(&ipad), ["hi Ana", "hi Matt"]);
        assert_eq!(phone.history(id.clone(), Some(3), 10).unwrap().len(), 2);
        assert_eq!(phone.groups().unwrap()[0].seq, 3);
        // Joined again from a GroupInfo, Ana's device keeps what it had.
        ipad.rejoin_group(id.clone(), chat(), added.group_info.clone().unwrap(), 3).unwrap();
        assert_eq!(texts(&ipad), ["hi Ana", "hi Matt"]);
        // Ana's key changes: nothing is encrypted to it until it's seen.
        phone.key_changed("ana".into(), dm_public_key(dm_new_secret().unwrap()).unwrap()).unwrap();
        assert!(phone.send(id.clone(), text("m2", "wait"), 22, vec![]).is_err());
        phone.seen("ana".into()).unwrap();
        // Seen, but her iPad's leaf is still under the replaced key and on no
        // list of the new one: held until it moves or is taken out (row 630).
        let held = phone.send(id.clone(), text("m2", "now"), 23, vec![]).unwrap_err().to_string();
        assert!(held.contains("waiting for ana's new security key"), "{held}");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn nothing_is_sent_in_a_conversation_until_the_friend_is_in() {
        // Row 676: Matt's phone made the conversation, and his tablet got in
        // before Ana (an external join winning the race with the add).
        // Nothing is encrypted there, of any kind, until a device of Ana's
        // passes the checks; the first of Matt's members adds her.
        let dir = folder();
        let (phone, ipad) = two(&dir);
        let tablet = device(&dir, "tablet");
        let handoff = tablet.start_handoff(TABLET.into(), "iPad".into(), "iPadOS".into(), vec![7; 32]).unwrap();
        tablet.finish_handoff(&handoff, phone.seal_handoff(handoff.qr()).unwrap().sealed, 5).unwrap();
        let listed = |device: &DmDevice, id: &str| DmListed {
            kind: DmLeafKind::Device,
            leaf_id: id.into(),
            signature_key: device.status().leaf_public,
        };
        let list = phone.sign_device_list(2, 6, vec![listed(&phone, "phone"), listed(&tablet, TABLET)]).unwrap();
        for device in [&phone, &tablet, &ipad] {
            device.accept_list("matt".into(), list.clone()).unwrap();
        }
        let id = b"conversation-1".to_vec();
        let info = phone.create_group(id.clone(), chat()).unwrap();
        let joined = tablet.rejoin_group(id.clone(), chat(), info, 0).unwrap();
        tablet.commit_taken(id.clone()).unwrap();
        phone.process(id.clone(), 1, joined.commit, 10).unwrap();

        let waits = |device: &DmDevice| device.not_joined(id.clone()).unwrap();
        assert_eq!(waits(&phone), Some(DmJoining { account: "ana".into(), adds_first: true }));
        assert_eq!(waits(&tablet), Some(DmJoining { account: "ana".into(), adds_first: false }));
        let refused = phone.send(id.clone(), text("m1", "ana 1"), 11, vec![]).unwrap_err().to_string();
        assert!(refused.contains("waiting for ana to join"), "{refused}");
        let reaction = DmContent {
            kind: "reaction".into(),
            client_message_id: "r1".into(),
            text: None,
            by_agent: None,
            extra_json: r#"{"to":"m0","emoji":"+1"}"#.into(),
        };
        assert!(tablet.send(id.clone(), reaction, 12, vec![]).unwrap_err().to_string().contains("to join"));
        assert!(phone.unsent().unwrap().is_empty() && tablet.unsent().unwrap().is_empty(), "nothing sealed to send");

        // A key package of a device of Ana's that isn't on her list: refused,
        // and the chat still waits.
        let rogue = device(&dir, "rogue");
        let offer =
            rogue.start_handoff("cm9ndWUtcm9ndWUtcm9ndWUtcm9ndWUtcm9ndWUtcm8".into(), "Rogue".into(), "macOS".into(), vec![8; 32]).unwrap();
        rogue.finish_handoff(&offer, ipad.seal_handoff(offer.qr()).unwrap().sealed, 5).unwrap();
        let rogue_add = phone.add_members(id.clone(), vec![rogue.key_packages(1).unwrap().remove(0)]).unwrap_err().to_string();
        assert!(rogue_add.contains("list"), "{rogue_add}");
        assert!(phone.send(id.clone(), text("m1", "ana 1"), 13, vec![]).is_err());

        // Ana's listed iPad is added: it goes, at the new epoch.
        let added = phone.add_members(id.clone(), vec![ipad.key_packages(1).unwrap().remove(0)]).unwrap().expect("something to add");
        phone.process(id.clone(), 2, added.commit.clone(), 14).unwrap();
        tablet.process(id.clone(), 2, added.commit, 14).unwrap();
        ipad.join_group(id.clone(), chat(), added.welcome.unwrap(), 2).unwrap();
        assert_eq!((waits(&phone), waits(&tablet)), (None, None));
        let said = phone.send(id.clone(), text("m1", "ana 1"), 15, vec![]).unwrap();
        let heard = ipad.process(id.clone(), 3, said, 16).unwrap().message.unwrap();
        assert_eq!(heard.content.text.as_deref(), Some("ana 1"));

        // Her only device set up again under a new key, the old leaf still
        // in: it doesn't count, and the chat holds.
        let again = device(&dir, "ipad-again");
        let relisted = ipad
            .sign_device_list(
                2,
                20,
                vec![DmListed { kind: DmLeafKind::Device, leaf_id: "ipad".into(), signature_key: again.status().leaf_public }],
            )
            .unwrap();
        phone.accept_list("ana".into(), relisted).unwrap();
        assert!(phone.send(id.clone(), text("m2", "after"), 21, vec![]).is_err());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_commit_the_home_never_carried_is_taken_from_the_worker_and_what_waited_for_it_is_read() {
        // Row 551: Ana accepts Matt, and the worker takes her add at seq 1,
        // but her app stops before it merges it or hands it to her home.
        // Matt joins by its Welcome and says something at epoch 1, which
        // reaches her through her home: held, never passed over, until the
        // worker's commit is in; then read.
        let dir = folder();
        let (phone, ipad) = two(&dir);
        let id = b"conversation-1".to_vec();
        ipad.create_group(id.clone(), chat()).unwrap();
        let added = ipad.add_members(id.clone(), vec![phone.key_packages(1).unwrap().remove(0)]).unwrap().expect("something to add");
        phone.join_group(id.clone(), chat(), added.welcome.unwrap(), 1).unwrap();
        assert_eq!(ipad.not_joined(id.clone()).unwrap().map(|waits| waits.account), Some("matt".into()));
        assert!(ipad.groups().unwrap()[0].pending, "her add waits to be merged");

        let said = phone.send(id.clone(), text("m1", "hi Ana"), 20, vec![]).unwrap();
        let first = HOME_SEQ_BASE + 1;
        let held = ipad.process(id.clone(), first, said.clone(), 30).unwrap();
        assert_eq!((held.outcome, held.behind, held.epoch), (DmOutcome::Held, true, 0));
        assert!(ipad.groups().unwrap()[0].seq < first, "the position stays before it");

        // Something other than a commit at the worker is passed over.
        assert_eq!(ipad.take_worker_commit(id.clone(), 1, said.clone(), 5).unwrap().outcome, DmOutcome::Already);
        assert_eq!(ipad.proposal_seq(id.clone()).unwrap(), 1);
        // Her own add, from the worker's log at seq 2 (as if a proposal
        // came first): merged, Matt in, said as he joined.
        let taken = ipad.take_worker_commit(id.clone(), 2, added.commit.clone(), 10).unwrap();
        assert_eq!((taken.outcome, taken.epoch), (DmOutcome::Committed, 1));
        let came = taken.message.unwrap().members.unwrap();
        assert_eq!(came.joined[0].account, "matt");
        assert_eq!(ipad.not_joined(id.clone()).unwrap(), None);
        assert!(!ipad.groups().unwrap()[0].pending);
        assert_eq!(ipad.take_worker_commit(id.clone(), 2, added.commit.clone(), 10).unwrap().outcome, DmOutcome::Already);

        let heard = ipad.process(id.clone(), first, said, 31).unwrap().message.unwrap();
        assert_eq!(heard.content.text.as_deref(), Some("hi Ana"));
        // The home carrying the same commit later changes nothing.
        let again = ipad.process(id.clone(), first + 1, added.commit, 32).unwrap();
        assert_ne!(again.outcome, DmOutcome::Committed);
        assert_eq!(ipad.groups().unwrap()[0].epoch, 1);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_friends_device_added_while_away_waits_for_their_list_then_goes_on() {
        // Row 561: Matt adds his tablet while Ana's iPad is away. Back, her
        // iPad has his list from before, which doesn't name it: the add is
        // held, the position kept, never dropped (which made it join again
        // past everything said meanwhile); once his newer list is in, it's
        // taken, and what the tablet says reads.
        let dir = folder();
        let (phone, ipad) = two(&dir);
        let id = b"conversation-1".to_vec();
        phone.create_group(id.clone(), chat()).unwrap();
        let added = phone.add_members(id.clone(), vec![ipad.key_packages(1).unwrap().remove(0)]).unwrap().expect("something to add");
        phone.commit_taken(id.clone()).unwrap();
        ipad.join_group(id.clone(), chat(), added.welcome.unwrap(), 1).unwrap();

        let tablet = device(&dir, "tablet");
        let handoff = tablet.start_handoff(TABLET.into(), "iPad".into(), "iPadOS".into(), vec![7; 32]).unwrap();
        tablet.finish_handoff(&handoff, phone.seal_handoff(handoff.qr()).unwrap().sealed, 5).unwrap();
        let listed = |device: &DmDevice, id: &str| DmListed {
            kind: DmLeafKind::Device,
            leaf_id: id.into(),
            signature_key: device.status().leaf_public,
        };
        let list = phone.sign_device_list(2, 6, vec![listed(&phone, "phone"), listed(&tablet, TABLET)]).unwrap();
        phone.accept_list("matt".into(), list.clone()).unwrap();
        tablet.accept_list("matt".into(), list.clone()).unwrap();
        let more = phone.add_members(id.clone(), vec![tablet.key_packages(1).unwrap().remove(0)]).unwrap().expect("something to add");
        phone.commit_taken(id.clone()).unwrap();
        tablet.join_group(id.clone(), chat(), more.welcome.unwrap(), 2).unwrap();

        let held = ipad.process(id.clone(), 2, more.commit.clone(), 10).unwrap();
        assert_eq!((held.outcome, held.unlisted, held.why.as_str()), (DmOutcome::Held, true, "matt"));
        assert_eq!(ipad.groups().unwrap()[0].seq, 1, "the position stays before it");
        ipad.accept_list("matt".into(), list).unwrap();
        let taken = ipad.process(id.clone(), 2, more.commit, 11).unwrap();
        assert_eq!((taken.outcome, taken.epoch), (DmOutcome::Committed, 2));
        let said = tablet.send(id.clone(), text("t1", "from the tablet"), 12, vec![]).unwrap();
        let heard = ipad.process(id.clone(), 3, said, 13).unwrap().message.unwrap();
        assert_eq!(heard.content.text.as_deref(), Some("from the tablet"));
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// Ana's iPad in a chat with Matt's phone; Matt's tablet, set up, on his
    /// list v2 (phone and tablet) as the phone and tablet hold it, and his
    /// v3 (the phone alone) signed: Ana's iPad holds v1 (row 561).
    fn away_while_a_device_came_and_went(dir: &std::path::Path) -> (DmDevice, DmDevice, DmDevice, Vec<u8>, Vec<u8>) {
        let (phone, ipad) = two(dir);
        let id = b"conversation-1".to_vec();
        phone.create_group(id.clone(), chat()).unwrap();
        let added = phone.add_members(id.clone(), vec![ipad.key_packages(1).unwrap().remove(0)]).unwrap().expect("something to add");
        phone.commit_taken(id.clone()).unwrap();
        ipad.join_group(id.clone(), chat(), added.welcome.unwrap(), 1).unwrap();
        let tablet = device(dir, "tablet");
        let handoff = tablet.start_handoff(TABLET.into(), "Tablet".into(), "iPadOS".into(), vec![7; 32]).unwrap();
        tablet.finish_handoff(&handoff, phone.seal_handoff(handoff.qr()).unwrap().sealed, 5).unwrap();
        let listed = |device: &DmDevice, id: &str| DmListed {
            kind: DmLeafKind::Device,
            leaf_id: id.into(),
            signature_key: device.status().leaf_public,
        };
        let v2 = phone.sign_device_list(2, 6, vec![listed(&phone, "phone"), listed(&tablet, TABLET)]).unwrap();
        phone.accept_list("matt".into(), v2.clone()).unwrap();
        tablet.accept_list("matt".into(), v2).unwrap();
        let v3 = phone.sign_device_list(3, 7, vec![listed(&phone, "phone")]).unwrap();
        (phone, ipad, tablet, id, v3)
    }

    fn item(seq: i64, envelope: String) -> DmWindowItem {
        DmWindowItem { seq, envelope, arrived_ms: seq * 10 }
    }

    #[test]
    fn a_device_that_came_and_went_while_away_is_read_through_its_window() {
        // Row 561 (Fugo's rule): Matt's tablet joins, says something, and
        // Matt's phone (there all along, on his latest list) takes it out
        // again, all while Ana's iPad is away. Back, with Matt's newest list,
        // which doesn't name the tablet, her iPad holds the add; the window
        // takes it all, as she'd have seen it online, the tablet's message
        // marked as from a device since removed.
        let dir = folder();
        let (phone, ipad, tablet, id, v3) = away_while_a_device_came_and_went(&dir);
        let more = phone.add_members(id.clone(), vec![tablet.key_packages(1).unwrap().remove(0)]).unwrap().expect("something to add");
        phone.commit_taken(id.clone()).unwrap();
        tablet.join_group(id.clone(), chat(), more.welcome.unwrap(), 2).unwrap();
        let said = tablet.send(id.clone(), text("t1", "from the tablet"), 12, vec![]).unwrap();
        tablet.process(id.clone(), 3, said.clone(), 13).unwrap();
        phone.process(id.clone(), 3, said.clone(), 13).unwrap();
        phone.accept_list("matt".into(), v3.clone()).unwrap();
        let out = phone.remove_members(id.clone(), vec![TABLET.into()]).unwrap();
        phone.commit_taken(id.clone()).unwrap();
        let after = phone.send(id.clone(), text("p1", "tablet's gone"), 14, vec![]).unwrap();

        ipad.accept_list("matt".into(), v3).unwrap();
        let held = ipad.process(id.clone(), 2, more.commit.clone(), 20).unwrap();
        assert_eq!((held.outcome, held.unlisted), (DmOutcome::Held, true));
        // Not there yet: nothing changes.
        let open = ipad.process_window(id.clone(), vec![item(2, more.commit.clone()), item(3, said.clone())]).unwrap();
        assert_eq!(open.end, DmWindowEnd::Open);
        assert!(open.processed.is_empty());
        assert_eq!(ipad.groups().unwrap()[0].seq, 1);

        let closed = ipad
            .process_window(id.clone(), vec![item(2, more.commit.clone()), item(3, said), item(4, out.commit.clone())])
            .unwrap();
        assert_eq!(closed.end, DmWindowEnd::Closed);
        let outcomes: Vec<DmOutcome> = closed.processed.iter().map(|p| p.outcome).collect();
        assert_eq!(outcomes, [DmOutcome::Committed, DmOutcome::Message, DmOutcome::Committed]);
        let came = closed.processed[0].message.as_ref().unwrap().members.as_ref().unwrap();
        assert_eq!(came.added[0].name, "Tablet");
        let heard = closed.processed[1].message.as_ref().unwrap();
        assert_eq!((heard.content.text.as_deref(), heard.departed), (Some("from the tablet"), true));
        let went = closed.processed[2].message.as_ref().unwrap().members.as_ref().unwrap();
        assert_eq!(went.removed[0].name, "Tablet");
        assert_eq!(ipad.groups().unwrap()[0].seq, 4);
        // And on, as ever.
        let next = ipad.process(id.clone(), 5, after, 50).unwrap().message.unwrap();
        assert_eq!((next.content.text.as_deref(), next.departed), (Some("tablet's gone"), false));
        let kept: Vec<(Option<String>, bool)> =
            ipad.history(id.clone(), None, 10).unwrap().into_iter().map(|m| (m.content.text, m.departed)).collect();
        assert!(kept.contains(&(Some("from the tablet".into()), true)), "{kept:?}");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn no_window_past_what_a_listed_device_never_took_out() {
        // The tablet is never taken out: the window stays open, then (no
        // more items) it's refused as before. In a self group none opens.
        let dir = folder();
        let (phone, ipad, tablet, id, v3) = away_while_a_device_came_and_went(&dir);
        let more = phone.add_members(id.clone(), vec![tablet.key_packages(1).unwrap().remove(0)]).unwrap().expect("something to add");
        phone.commit_taken(id.clone()).unwrap();
        ipad.accept_list("matt".into(), v3).unwrap();
        let open = ipad.process_window(id.clone(), vec![item(2, more.commit.clone())]).unwrap();
        assert_eq!(open.end, DmWindowEnd::Open);
        assert_eq!(ipad.groups().unwrap()[0].seq, 1);
        let junk = ipad.process_window(id.clone(), vec![item(2, "not json".into())]).unwrap();
        assert_eq!(junk.end, DmWindowEnd::Aborted);
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// PassOn (Mista cc19e82e7, Fugo's condition 5): after the person's
    /// home moves, the new home may serve a message again at a new seq.
    /// Taken once, it's dropped the second time (its key is gone once
    /// decrypted): one line in the history. The device's own the same.
    #[test]
    fn a_message_served_again_at_another_seq_shows_once() {
        let dir = folder();
        let (phone, ipad) = two(&dir);
        let id = b"conversation-1".to_vec();
        phone.create_group(id.clone(), chat()).unwrap();
        let added = phone.add_members(id.clone(), vec![ipad.key_packages(1).unwrap().remove(0)]).unwrap().expect("something to add");
        phone.commit_taken(id.clone()).unwrap();
        ipad.join_group(id.clone(), chat(), added.welcome.unwrap(), 1).unwrap();

        let said = phone.send(id.clone(), text("m1", "once"), 2, vec![]).unwrap();
        assert_eq!(ipad.process(id.clone(), 2, said.clone(), 3).unwrap().outcome, DmOutcome::Message);
        let again = ipad.process(id.clone(), 7, said.clone(), 4).unwrap();
        assert_eq!((again.outcome, again.message.is_some()), (DmOutcome::Dropped, false));
        let heard = ipad.history(id.clone(), None, 50).unwrap();
        assert_eq!(heard.iter().filter(|m| m.content.text.as_deref() == Some("once")).count(), 1);
        // The sender's own copy, back twice.
        assert_eq!(phone.process(id.clone(), 2, said.clone(), 3).unwrap().outcome, DmOutcome::Message);
        let mine = phone.process(id.clone(), 7, said, 4).unwrap();
        assert_eq!((mine.outcome, mine.message.is_some()), (DmOutcome::Dropped, false));
        let kept = phone.history(id.clone(), None, 50).unwrap();
        assert_eq!(kept.iter().filter(|m| m.content.text.as_deref() == Some("once")).count(), 1);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_bad_message_is_dropped_and_the_group_goes_on() {
        let dir = folder();
        let (phone, ipad) = two(&dir);
        let id = b"conversation-1".to_vec();
        phone.create_group(id.clone(), chat()).unwrap();
        let added = phone.add_members(id.clone(), vec![ipad.key_packages(1).unwrap().remove(0)]).unwrap().expect("something to add");
        phone.commit_taken(id.clone()).unwrap();
        ipad.join_group(id.clone(), chat(), added.welcome.unwrap(), 1).unwrap();

        let junk = Envelope::new(vec![0, 1, 2, 3]).map(|e| e.to_json().to_string()).unwrap_or_else(|_| "{}".into());
        assert_eq!(ipad.process(id.clone(), 2, junk, 1).unwrap().outcome, DmOutcome::Dropped);
        assert_eq!(ipad.process(id.clone(), 3, "not json".into(), 1).unwrap().outcome, DmOutcome::Dropped);
        let said = phone.send(id.clone(), text("m1", "still here"), 2, vec![]).unwrap();
        let heard = ipad.process(id.clone(), 4, said, 3).unwrap().message.unwrap();
        assert_eq!(heard.content.text.as_deref(), Some("still here"));
        std::fs::remove_dir_all(dir).unwrap();
    }

    // Melone F3: a hand-off opens only with the identity the person's
    // account publishes; someone else who knew the QR code's secret can't
    // plant theirs. The offer is spent either way.
    #[test]
    fn a_hand_off_is_taken_only_with_the_published_identity() {
        let dir = folder();
        let (phone, _ipad) = two(&dir);
        let published = phone.status().identity_public.unwrap();
        let tablet = device(&dir, "tablet");
        for (account, identity) in [("matt", vec![9; 32]), ("ana", published.clone())] {
            let handoff = tablet.start_handoff(TABLET.into(), "iPad".into(), "iPadOS".into(), vec![7; 32]).unwrap();
            let sealed = phone.seal_handoff(handoff.qr()).unwrap();
            assert!(tablet.finish_handoff_for(&handoff, sealed.sealed.clone(), 5, account.into(), identity).is_err());
            assert!(!tablet.status().set_up);
            assert!(tablet.pin("matt".into()).is_none());
            let spent = tablet.finish_handoff_for(&handoff, sealed.sealed, 5, "matt".into(), published.clone());
            assert!(spent.is_err(), "spent");
        }
        let handoff = tablet.start_handoff(TABLET.into(), "iPad".into(), "iPadOS".into(), vec![7; 32]).unwrap();
        let sealed = phone.seal_handoff(handoff.qr()).unwrap();
        let done = tablet.finish_handoff_for(&handoff, sealed.sealed, 5, "matt".into(), published.clone()).unwrap();
        assert_eq!(done.identity_public, published);
        assert!(tablet.status().set_up);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_new_device_gets_the_identity_by_qr_and_a_key_change_holds_sending() {
        let dir = folder();
        let (phone, ipad) = two(&dir);
        let tablet = device(&dir, "tablet");
        assert!(!tablet.status().set_up);
        let handoff = tablet.start_handoff(TABLET.into(), "iPad".into(), "iPadOS".into(), vec![7; 32]).unwrap();
        let sealed = phone.seal_handoff(handoff.qr()).unwrap();
        assert_eq!((sealed.leaf_id.as_str(), sealed.name.as_str(), sealed.platform.as_str()), (TABLET, "iPad", "iPadOS"));
        assert_eq!(sealed.endpoint_id, vec![7; 32]);
        // Pasted, with whatever came along, it's the same code and digits.
        assert_eq!(handoff.comparison().len(), 23);
        let pasted = phone.seal_handoff(format!("  {}\n", handoff.qr())).unwrap();
        assert_eq!(
            (pasted.comparison.as_str(), sealed.comparison.as_str()),
            (handoff.comparison().as_str(), handoff.comparison().as_str())
        );
        assert!(phone.seal_handoff("claktory-dm-add:1:nope".into()).is_err());
        let done = tablet.finish_handoff(&handoff, sealed.sealed.clone(), 5).unwrap();
        assert_eq!(done.account_id, "matt");
        assert_eq!(Some(done.identity_public.clone()), phone.status().identity_public);
        // Single use.
        assert!(tablet.finish_handoff(&handoff, sealed.sealed, 5).is_err());
        // It came with Matt's pins: Ana's key is known there too.
        assert_eq!(tablet.pin("ana".into()).unwrap().identity_public, ipad.status().identity_public.unwrap());
        assert_eq!(phone.safety_number("ana".into()).unwrap().digits, tablet.safety_number("ana".into()).unwrap().digits);

        // The phone lists both devices, version 2.
        let listed = |device: &DmDevice, id: &str| DmListed {
            kind: DmLeafKind::Device,
            leaf_id: id.into(),
            signature_key: device.status().leaf_public,
        };
        let list = phone.sign_device_list(2, 6, vec![listed(&phone, "phone"), listed(&tablet, TABLET)]).unwrap();
        // Taken by the worker: pinned here too.
        phone.accept_list("matt".into(), list.clone()).unwrap();
        // The phone took its own new list: the next change starts from it.
        let mine = phone.device_list("matt".into()).unwrap().unwrap();
        assert_eq!((mine.version, mine.leaves.len()), (2, 2));
        assert!(phone.device_list("bea".into()).unwrap().is_none());
        ipad.accept_list("matt".into(), list).unwrap();
        assert_eq!(ipad.pin("matt".into()).unwrap().device_list_version, 2);

        // Safety numbers match across the two people.
        let at_ana = ipad.safety_number("matt".into()).unwrap();
        assert!(phone.safety_matches("ana".into(), at_ana.qr).unwrap());
        phone.verify("ana".into()).unwrap();
        assert!(phone.pin("ana".into()).unwrap().verified);

        // Ana's key changes: sending waits until seen, and the mark goes.
        let new_key = dm_public_key(dm_new_secret().unwrap()).unwrap();
        phone.key_changed("ana".into(), new_key.clone()).unwrap();
        let pin = phone.pin("ana".into()).unwrap();
        assert!(pin.changed_unseen && !pin.can_send && !pin.verified);
        phone.seen("ana".into()).unwrap();
        assert!(phone.pin("ana".into()).unwrap().can_send);

        // E11-3: the numbers don't match: back to the key it changed from,
        // verified as it was; the one that didn't match comes back only as
        // another change, marked.
        let real = ipad.status().identity_public.unwrap();
        let pin = phone.pin("ana".into()).unwrap();
        assert_eq!(pin.changed_from.as_deref(), Some(real.as_slice()));
        assert!(phone.undo_key_change("ana".into(), vec![9; 32]).is_err());
        phone.undo_key_change("ana".into(), real.clone()).unwrap();
        let pin = phone.pin("ana".into()).unwrap();
        assert_eq!(pin.identity_public, real);
        assert!(pin.verified && !pin.changed_unseen && pin.changed_from.is_none() && !pin.mismatched);
        assert_eq!(pin.device_list_version, 1);
        phone.key_changed("ana".into(), new_key.clone()).unwrap();
        let pin = phone.pin("ana".into()).unwrap();
        assert!(pin.mismatched && pin.changed_unseen && !pin.verified);
        assert!(phone.undo_key_change("matt".into(), real).is_err());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_reinstalled_device_comes_back_under_its_leaf_id_with_a_new_key_said_once() {
        let dir = folder();
        let (phone, ipad) = two(&dir);
        let listed = |device: &DmDevice, id: &str| DmListed {
            kind: DmLeafKind::Device,
            leaf_id: id.into(),
            signature_key: device.status().leaf_public,
        };
        let hand_off = |to: &DmDevice| {
            let handoff = to.start_handoff(TABLET.into(), "iPad".into(), "iPadOS".into(), vec![7; 32]).unwrap();
            let sealed = phone.seal_handoff(handoff.qr()).unwrap();
            to.finish_handoff(&handoff, sealed.sealed, 5).unwrap();
        };
        let tablet = device(&dir, "tablet");
        hand_off(&tablet);
        let list = phone.sign_device_list(2, 6, vec![listed(&phone, "phone"), listed(&tablet, TABLET)]).unwrap();
        for device in [&phone, &tablet, &ipad] {
            device.accept_list("matt".into(), list.clone()).unwrap();
        }
        let id = b"conversation-1".to_vec();
        phone.create_group(id.clone(), chat()).unwrap();
        let added = phone
            .add_members(id.clone(), vec![ipad.key_packages(1).unwrap().remove(0), tablet.key_packages(1).unwrap().remove(0)])
            .unwrap()
            .expect("something to add");
        phone.process(id.clone(), 1, added.commit.clone(), 10).unwrap();
        ipad.join_group(id.clone(), chat(), added.welcome.clone().unwrap(), 1).unwrap();

        // Reinstalled: a new store, so a new leaf key, under the same leaf
        // id; handed off again, and the list names its new key.
        let again = device(&dir, "tablet-again");
        assert_ne!(again.status().leaf_public, tablet.status().leaf_public);
        hand_off(&again);
        let list = phone.sign_device_list(3, 7, vec![listed(&phone, "phone"), listed(&again, TABLET)]).unwrap();
        for device in [&phone, &again, &ipad] {
            device.accept_list("matt".into(), list.clone()).unwrap();
        }
        let info = added.group_info.clone().unwrap();
        let rejoined = again.rejoin_group(id.clone(), chat(), info, 1).unwrap();
        again.commit_taken(id.clone()).unwrap();
        // The others take it at seq 2: one line, "set up a device again",
        // never a removal and an add.
        for device in [&phone, &ipad] {
            let line = device.process(id.clone(), 2, rejoined.commit.clone(), 20).unwrap().message.unwrap();
            let members = line.members.unwrap();
            assert_eq!(members.set_up_again.iter().map(|m| (m.leaf_id.as_str(), m.name.as_str())).collect::<Vec<_>>(), [(TABLET, "iPad")]);
            assert!(members.added.is_empty() && members.removed.is_empty());
        }
        // Its former install is still in: nobody sends, Ana included, and
        // the chat can say whose device it waits for.
        assert_eq!(ipad.waiting_for(id.clone()).unwrap().as_deref(), Some("matt"));
        assert_eq!(ipad.reinstalling(id.clone()).unwrap().as_deref(), Some("iPad"));
        assert!(ipad.send(id.clone(), text("a1", "wait"), 21, vec![]).is_err());
        // Ana never takes it out; the reinstalled iPad does, by index.
        assert!(ipad.remove_dead_leaves(id.clone(), vec![]).unwrap().is_none());
        let out = again.remove_dead_leaves(id.clone(), vec![]).unwrap().unwrap();
        again.commit_taken(id.clone()).unwrap();
        for device in [&phone, &ipad] {
            // Its former install going under an id still here says nothing.
            assert!(device.process(id.clone(), 3, out.commit.clone(), 30).unwrap().message.is_none());
        }
        assert_eq!(ipad.reinstalling(id.clone()).unwrap(), None);
        assert_eq!(ipad.waiting_for(id.clone()).unwrap(), None);
        // The new install stays, and hears Ana.
        let said = ipad.send(id.clone(), text("a2", "welcome back"), 40, vec![]).unwrap();
        let heard = again.process(id.clone(), 4, said, 41).unwrap().message.unwrap();
        assert_eq!(heard.content.text.as_deref(), Some("welcome back"));
        assert_eq!(phone.members(id.clone()).unwrap().iter().filter(|m| m.leaf_id == TABLET).count(), 1);
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// A device its person took off their list is still in the chat until
    /// one of their devices takes it out: the chat names it (E11-8),
    /// never as a new key or a reinstall.
    #[test]
    fn a_chat_held_by_a_removed_device_names_it() {
        let dir = folder();
        let (phone, ipad) = two(&dir);
        let listed = |device: &DmDevice, id: &str| DmListed {
            kind: DmLeafKind::Device,
            leaf_id: id.into(),
            signature_key: device.status().leaf_public,
        };
        let tablet = device(&dir, "tablet");
        let handoff = tablet.start_handoff(TABLET.into(), "iPad".into(), "iPadOS".into(), vec![7; 32]).unwrap();
        let sealed = phone.seal_handoff(handoff.qr()).unwrap();
        tablet.finish_handoff(&handoff, sealed.sealed, 5).unwrap();
        let list = phone.sign_device_list(2, 6, vec![listed(&phone, "phone"), listed(&tablet, TABLET)]).unwrap();
        for device in [&phone, &tablet, &ipad] {
            device.accept_list("matt".into(), list.clone()).unwrap();
        }
        let id = b"conversation-1".to_vec();
        phone.create_group(id.clone(), chat()).unwrap();
        let added = phone
            .add_members(id.clone(), vec![ipad.key_packages(1).unwrap().remove(0), tablet.key_packages(1).unwrap().remove(0)])
            .unwrap()
            .expect("something to add");
        phone.process(id.clone(), 1, added.commit.clone(), 10).unwrap();
        ipad.join_group(id.clone(), chat(), added.welcome.clone().unwrap(), 1).unwrap();
        assert_eq!(ipad.removed_held(id.clone()).unwrap(), None);

        // Locked out: off Matt's list, still in the chat.
        let list = phone.sign_device_list(3, 7, vec![listed(&phone, "phone")]).unwrap();
        for device in [&phone, &ipad] {
            device.accept_list("matt".into(), list.clone()).unwrap();
        }
        assert_eq!(ipad.waiting_for(id.clone()).unwrap().as_deref(), Some("matt"));
        assert_eq!(ipad.removed_held(id.clone()).unwrap().as_deref(), Some("iPad"));
        assert_eq!(ipad.reinstalling(id.clone()).unwrap(), None);
        // Matt's phone takes it out; then nothing holds the chat.
        let out = phone.remove_dead_leaves(id.clone(), vec![]).unwrap().expect("the iPad to take out");
        phone.commit_taken(id.clone()).unwrap();
        ipad.process(id.clone(), 2, out.commit.clone(), 20).unwrap();
        assert_eq!(ipad.removed_held(id.clone()).unwrap(), None);
        assert_eq!(ipad.waiting_for(id.clone()).unwrap(), None);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_friends_leaf_under_an_id_of_the_persons_own_is_the_friends() {
        // Row 671 (Fugo): a leaf is its certificate's account with its id
        // and key. Ana lists a device under the id of Matt's phone itself:
        // the phone says Ana added it, never takes it for its own leaf.
        let dir = folder();
        let (phone, ipad) = (device(&dir, "phone"), device(&dir, "ipad"));
        let matt = phone.set_up_first(dm_new_secret().unwrap(), "matt".into(), TABLET.into(), "iPhone".into(), 1).unwrap();
        let ana = ipad.set_up_first(dm_new_secret().unwrap(), "ana".into(), "ipad".into(), "iPad".into(), 1).unwrap();
        phone.first_contact("ana".into(), ana.identity_public.clone()).unwrap();
        phone.accept_list("ana".into(), ana.device_list).unwrap();
        ipad.first_contact("matt".into(), matt.identity_public.clone()).unwrap();
        ipad.accept_list("matt".into(), matt.device_list).unwrap();
        let mini = device(&dir, "mini");
        let handoff = mini.start_handoff(TABLET.into(), "iPad mini".into(), "iPadOS".into(), vec![7; 32]).unwrap();
        let sealed = ipad.seal_handoff(handoff.qr()).unwrap();
        mini.finish_handoff(&handoff, sealed.sealed, 5).unwrap();
        let listed = |device: &DmDevice, id: &str| DmListed {
            kind: DmLeafKind::Device,
            leaf_id: id.into(),
            signature_key: device.status().leaf_public,
        };
        let list = ipad.sign_device_list(2, 6, vec![listed(&ipad, "ipad"), listed(&mini, TABLET)]).unwrap();
        for device in [&ipad, &mini, &phone] {
            device.accept_list("ana".into(), list.clone()).unwrap();
        }
        let id = b"conversation-1".to_vec();
        phone.create_group(id.clone(), chat()).unwrap();
        let added = phone.add_members(id.clone(), vec![ipad.key_packages(1).unwrap().remove(0)]).unwrap().expect("something to add");
        phone.process(id.clone(), 1, added.commit.clone(), 10).unwrap();
        ipad.join_group(id.clone(), chat(), added.welcome.unwrap(), 1).unwrap();

        let more = ipad.add_members(id.clone(), vec![mini.key_packages(1).unwrap().remove(0)]).unwrap().expect("something to add");
        ipad.process(id.clone(), 2, more.commit.clone(), 20).unwrap();
        let line = phone.process(id.clone(), 2, more.commit.clone(), 21).unwrap().message.expect("a line for Ana's device");
        let members = line.members.unwrap();
        let who = |list: &[DmMember]| list.iter().map(|m| (m.account.clone(), m.leaf_id.clone(), m.name.clone())).collect::<Vec<_>>();
        assert_eq!(who(&members.added), [("ana".to_owned(), TABLET.to_owned(), "iPad mini".to_owned())]);
        assert!(members.joined.is_empty(), "her second device: added, not joined");
        assert!(members.removed.is_empty() && members.set_up_again.is_empty() && members.key_changed.is_empty());
        // Both leaves are in, each under its own account.
        let mut both: Vec<(String, String)> =
            phone.members(id.clone()).unwrap().into_iter().filter(|m| m.leaf_id == TABLET).map(|m| (m.account, m.name)).collect();
        both.sort();
        assert_eq!(both, [("ana".to_owned(), "iPad mini".to_owned()), ("matt".to_owned(), "iPhone".to_owned())]);

        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn the_self_group_carries_drafts_reads_deletions_and_pins() {
        let dir = folder();
        let (phone, ipad) = two(&dir);
        let tablet = device(&dir, "tablet");
        let handoff = tablet.start_handoff(TABLET.into(), "iPad".into(), "iPadOS".into(), vec![7; 32]).unwrap();
        let sealed = phone.seal_handoff(handoff.qr()).unwrap();
        tablet.finish_handoff(&handoff, sealed.sealed, 5).unwrap();
        let listed = |device: &DmDevice, id: &str| DmListed {
            kind: DmLeafKind::Device,
            leaf_id: id.into(),
            signature_key: device.status().leaf_public,
        };
        let list = phone.sign_device_list(2, 6, vec![listed(&phone, "phone"), listed(&tablet, TABLET)]).unwrap();
        // Taken by the worker: pinned here too.
        phone.accept_list("matt".into(), list.clone()).unwrap();
        tablet.accept_list("matt".into(), list).unwrap();

        let me = b"self".to_vec();
        let mine = DmGroupKind { kind: DmGroupType::SelfGroup, accounts: vec!["matt".into()], claktory_id: String::new() };
        phone.create_group(me.clone(), mine.clone()).unwrap();
        let added = phone.add_members(me.clone(), vec![tablet.key_packages(1).unwrap().remove(0)]).unwrap().expect("something to add");
        phone.process(me.clone(), 1, added.commit, 1).unwrap();
        tablet.join_group(me.clone(), mine, added.welcome.unwrap(), 1).unwrap();

        // A conversation with history on the tablet.
        let chat_id = b"conversation-1".to_vec();
        tablet.create_group(chat_id.clone(), chat()).unwrap();
        let with_ana = tablet.add_members(chat_id.clone(), vec![ipad.key_packages(1).unwrap().remove(0)]).unwrap().expect("something to add");
        tablet.process(chat_id.clone(), 1, with_ana.commit, 1).unwrap();
        let said = tablet.send(chat_id.clone(), text("t1", "hello"), 2, vec![]).unwrap();
        tablet.process(chat_id.clone(), 2, said, 2).unwrap();
        assert_eq!(tablet.history(chat_id.clone(), None, 10).unwrap().len(), 2);

        let about = |kind: &str, text: Option<&str>, extra: serde_json::Value| DmContent {
            kind: kind.into(),
            client_message_id: format!("{kind}-{}", extra),
            text: text.map(str::to_owned),
            by_agent: None,
            extra_json: extra.to_string(),
        };
        let group = base64::Engine::encode(&base64::engine::general_purpose::URL_SAFE_NO_PAD, &chat_id);
        // Written on the phone: it counts there at once, and on the tablet
        // when it comes; never as history.
        let mut seq = 1;
        let mut both = |content: DmContent| {
            seq += 1;
            let envelope = phone.send(me.clone(), content, seq, vec![]).unwrap();
            phone.process(me.clone(), seq, envelope.clone(), seq).unwrap();
            tablet.process(me.clone(), seq, envelope, seq).unwrap()
        };
        let draft = both(about("draft", Some("half a thought"), serde_json::json!({"group": group, "at_ms": 10})));
        assert_eq!(draft.outcome, DmOutcome::Message);
        // An older draft from elsewhere doesn't replace a newer one.
        both(about("draft", Some("older"), serde_json::json!({"group": group, "at_ms": 5})));
        both(about("read", None, serde_json::json!({"group": group, "seq": 2})));
        for device in [&phone, &tablet] {
            let view = device.view(chat_id.clone()).unwrap();
            assert_eq!((view.draft.as_str(), view.read_seq), ("half a thought", 2));
        }
        both(about("delete", None, serde_json::json!({"group": group, "seq": 2})));
        assert_eq!(tablet.view(chat_id.clone()).unwrap().deleted_seq, 2);
        assert!(tablet.history(chat_id.clone(), None, 10).unwrap().is_empty());
        assert!(tablet.history(me.clone(), None, 10).unwrap().is_empty());

        // Pins: Bea, met on the phone, is pinned on the tablet too.
        phone.first_contact("bea".into(), dm_public_key(dm_new_secret().unwrap()).unwrap()).unwrap();
        let pins: serde_json::Value = serde_json::from_slice(&phone.export_pins().unwrap()).unwrap();
        // Never from a message marked as a departed device's (Fugo 40397).
        let departed = DmMessage {
            seq: 99,
            mine: false,
            from_account: "matt".into(),
            from_leaf: "phone".into(),
            from_kind_device: true,
            content: about("pins", None, serde_json::json!({"pins": pins.clone()})),
            arrived_ms: 99,
            members: None,
            departed: true,
        };
        let self_kind = DmGroupKind { kind: DmGroupType::SelfGroup, accounts: vec!["matt".into()], claktory_id: String::new() };
        {
            let mut state = tablet.state();
            tablet.store.with_tx(|_, tx| tablet.take(tx, &mut state, &self_kind, &me, &departed)).unwrap();
        }
        assert!(tablet.pin("bea".into()).is_none());
        both(about("pins", None, serde_json::json!({"pins": pins})));
        assert_eq!(tablet.pin("bea".into()).unwrap().identity_public, phone.pin("bea".into()).unwrap().identity_public);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_proposal_in_the_channel_feed_is_kept_by_its_digest() {
        // #451 (Mista 32507): a Claktory sends its proposals and drafts into
        // its channel through the home; the service's record names only the
        // digest of the MLS bytes. Taken from the feed, it's found by that
        // digest, and isn't the channel's history.
        let dir = folder();
        let (phone, _) = two(&dir);
        let tablet = device(&dir, "tablet");
        let handoff = tablet.start_handoff(TABLET.into(), "iPad".into(), "iPadOS".into(), vec![7; 32]).unwrap();
        let sealed = phone.seal_handoff(handoff.qr()).unwrap();
        tablet.finish_handoff(&handoff, sealed.sealed, 5).unwrap();
        let listed = |device: &DmDevice, id: &str| DmListed {
            kind: DmLeafKind::Device,
            leaf_id: id.into(),
            signature_key: device.status().leaf_public,
        };
        let list = phone.sign_device_list(2, 6, vec![listed(&phone, "phone"), listed(&tablet, TABLET)]).unwrap();
        phone.accept_list("matt".into(), list.clone()).unwrap();
        tablet.accept_list("matt".into(), list).unwrap();
        let id = b"channel-mini".to_vec();
        let channel = DmGroupKind { kind: DmGroupType::Channel, accounts: vec!["matt".into()], claktory_id: "mini".into() };
        phone.create_group(id.clone(), channel.clone()).unwrap();
        let added = phone.add_members(id.clone(), vec![tablet.key_packages(1).unwrap().remove(0)]).unwrap().expect("something to add");
        phone.process(id.clone(), 1, added.commit, 1).unwrap();
        tablet.join_group(id.clone(), channel, added.welcome.unwrap(), 1).unwrap();

        let proposal = DmContent {
            kind: "proposal".into(),
            client_message_id: "p1".into(),
            text: Some("proposed".into()),
            by_agent: None,
            extra_json: String::new(),
        };
        let said = phone.send(id.clone(), proposal, 2, vec![]).unwrap();
        let digest = read_envelope(&said).unwrap().digest().to_vec();
        assert!(tablet.side_message(id.clone(), digest.clone()).unwrap().is_none());
        let processed = tablet.process(id.clone(), 2, said, 2).unwrap();
        assert_eq!(processed.outcome, DmOutcome::Message);
        let kept = tablet.side_message(id.clone(), digest).unwrap().expect("kept by its digest");
        assert_eq!((kept.content.kind.as_str(), kept.content.text.as_deref()), ("proposal", Some("proposed")));
        assert!(tablet.history(id.clone(), None, 10).unwrap().iter().all(|message| message.content.kind != "proposal"));
        // Something else said there is history as before.
        let note = phone.send(id.clone(), text("n1", "hello"), 3, vec![]).unwrap();
        tablet.process(id.clone(), 3, note, 3).unwrap();
        assert!(tablet.history(id.clone(), None, 10).unwrap().iter().any(|message| message.content.text.as_deref() == Some("hello")));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_proposal_is_decrypted_once_and_kept() {
        let dir = folder();
        let (phone, ipad) = two(&dir);
        let id = b"conversation-1".to_vec();
        phone.create_group(id.clone(), chat()).unwrap();
        let added = phone.add_members(id.clone(), vec![ipad.key_packages(1).unwrap().remove(0)]).unwrap().expect("something to add");
        phone.commit_taken(id.clone()).unwrap();
        ipad.join_group(id.clone(), chat(), added.welcome.unwrap(), 1).unwrap();
        // Not through the feed: the side route, decrypted once.
        let said = phone.send(id.clone(), text("p1", "proposed"), 2, vec![]).unwrap();
        let first = ipad.open_side(id.clone(), said.clone(), 3).unwrap();
        assert_eq!(first.content.text.as_deref(), Some("proposed"));
        assert_eq!(ipad.open_side(id.clone(), said, 4).unwrap(), first);
        std::fs::remove_dir_all(dir).unwrap();
    }

    fn wrap(bytes: Vec<u8>) -> String {
        Envelope::new(bytes).unwrap().to_json().to_string()
    }

    /// The MLS group of a Claktory's leaf joined from [welcome] (an
    /// envelope), with OpenMLS directly, past every check of ours.
    fn joined(provider: &Provider<'_>, welcome: &str) -> openmls::prelude::MlsGroup {
        use openmls::prelude::tls_codec::Deserialize as _;
        use openmls::prelude::*;
        let welcome = read_envelope(welcome).unwrap().into_bytes();
        let MlsMessageBodyIn::Welcome(welcome) = MlsMessageIn::tls_deserialize_exact(&welcome).unwrap().extract() else {
            panic!("not a welcome")
        };
        let config =
            MlsGroupJoinConfig::builder().use_ratchet_tree_extension(true).wire_format_policy(PURE_PLAINTEXT_WIRE_FORMAT_POLICY).build();
        StagedWelcome::new_from_welcome(provider, &config, welcome, None).unwrap().into_group(provider).unwrap()
    }

    #[test]
    fn an_add_of_only_those_in_the_group_already_is_nothing_to_add_by_its_kind() {
        let dir = folder();
        let (phone, ipad) = two(&dir);
        let id = b"conversation-1".to_vec();
        phone.create_group(id.clone(), chat()).unwrap();
        let package = ipad.key_packages(1).unwrap().remove(0);
        let added = phone.add_members(id.clone(), vec![package.clone()]).unwrap().expect("something to add");
        assert_eq!(phone.process(id.clone(), 1, added.commit, 1).unwrap().outcome, DmOutcome::Committed);
        // The iPad's key is in the group now: nothing to add, and no
        // commit waiting, rather than an error told by its words (Fugo).
        assert!(phone.add_members(id.clone(), vec![package]).unwrap().is_none());
        // A refusal of another kind is still an error.
        assert!(phone.add_members(id, vec!["not an envelope".into()]).is_err());
    }

    /// Matt's phone and Ana's iPad in a conversation (seqs 1 and 2 taken),
    /// and Matt's Claktory granted it: its store, its leaf and its Welcome.
    fn claktory_in_a_conversation(dir: &std::path::Path) -> (DmDevice, DmDevice, DmDevice, Me, Vec<u8>, String) {
        use claktory_dm_crypto::certificate::claktory_leaf_id;
        let (phone, ipad) = two(dir);
        let mini = device(dir, "mini");
        let id = b"conversation-1".to_vec();
        phone.create_group(id.clone(), chat()).unwrap();
        let added = phone.add_members(id.clone(), ipad.key_packages(1).unwrap()).unwrap().expect("something to add");
        assert_eq!(phone.process(id.clone(), 1, added.commit, 1).unwrap().outcome, DmOutcome::Committed);
        ipad.join_group(id.clone(), chat(), added.welcome.unwrap(), 1).unwrap();

        // The Claktory's leaf, certified and listed by Matt's phone.
        let key = LeafKey::generate().unwrap();
        let leaf_id = claktory_leaf_id("mini", 1);
        let certificate = phone.issue_certificate(DmLeafKind::Claktory, leaf_id.clone(), key.public().to_vec(), "mini".into(), 1).unwrap();
        let me = Me::new(key, LeafCertificate::from_bytes(&certificate).unwrap()).unwrap();
        let leaves = vec![
            DmListed { kind: DmLeafKind::Device, leaf_id: "phone".into(), signature_key: phone.status().leaf_public },
            DmListed { kind: DmLeafKind::Claktory, leaf_id, signature_key: me.key.public().to_vec() },
        ];
        let v2 = phone.sign_device_list(2, 2, leaves).unwrap();
        phone.accept_list("matt".into(), v2.clone()).unwrap();
        ipad.accept_list("matt".into(), v2).unwrap();
        let package = mini.store().with_tx(|provider, _| me.key_package(provider)).unwrap();
        let granted = phone.add_members(id.clone(), vec![wrap(package)]).unwrap().expect("something to add");
        for device in [&phone, &ipad] {
            assert_eq!(device.process(id.clone(), 2, granted.commit.clone(), 2).unwrap().outcome, DmOutcome::Committed);
        }
        (phone, ipad, mini, me, id, granted.welcome.unwrap())
    }

    #[test]
    fn a_claktorys_leave_from_the_worker_is_taken_before_the_homes_commit_of_it() {
        // #451 (Mista 32950): once a device reads the home, proposals (a
        // Claktory's own Remove) stay at the worker. Each is taken from
        // there, by the worker's own seqs, before the home's commit that
        // takes it in by reference; that commit doesn't go in without it.
        use openmls::prelude::tls_codec::Serialize as _;
        let home = |n: i64| HOME_SEQ_BASE + n;
        let dir = folder();
        let (phone, ipad, mini, me, id, welcome) = claktory_in_a_conversation(&dir);
        assert_eq!(ipad.proposal_seq(id.clone()).unwrap(), 2, "before the home: where it is in the feed");

        // Both go over to the home: the worker's position stays as it was.
        let said = phone.send(id.clone(), text("m1", "from the home"), 3, vec![]).unwrap();
        for device in [&phone, &ipad] {
            assert!(device.process(id.clone(), home(1), said.clone(), 3).unwrap().message.is_some());
            assert_eq!(device.proposal_seq(id.clone()).unwrap(), 2);
        }

        // The Claktory asks to leave, at the worker (its seq 3).
        let leave = mini
            .store()
            .with_tx(|provider, _| -> Result<Vec<u8>, claktory_dm_crypto::Error> {
                let mut group = joined(provider, &welcome);
                Ok(group.leave_group(provider, &me.key).unwrap().tls_serialize_detached().unwrap())
            })
            .unwrap();
        // Said before it came (with a proposal waiting, nothing is said
        // until a commit takes it in).
        let old = phone.send(id.clone(), text("m2", "old"), 5, vec![]).unwrap();
        // Through process, a worker's seq is behind the home's: never taken.
        assert_eq!(ipad.process(id.clone(), 3, wrap(leave.clone()), 4).unwrap().outcome, DmOutcome::Already);
        for device in [&phone, &ipad] {
            assert_eq!(device.take_proposal(id.clone(), 3, wrap(leave.clone())).unwrap().outcome, DmOutcome::Proposal);
            assert_eq!(device.take_proposal(id.clone(), 3, wrap(leave.clone())).unwrap().outcome, DmOutcome::Already);
            assert_eq!(device.proposal_seq(id.clone()).unwrap(), 3);
        }
        // Anything else still at the worker is passed over, its seq too.
        assert_eq!(ipad.take_proposal(id.clone(), 4, old).unwrap().outcome, DmOutcome::Already);
        assert_eq!(ipad.proposal_seq(id.clone()).unwrap(), 4);

        // The phone's next commit takes it in; at the home, the iPad takes
        // that commit, and the Claktory is out.
        let commit = phone.update_group(id.clone()).unwrap().commit;
        assert_eq!(phone.process(id.clone(), home(2), commit.clone(), 6).unwrap().outcome, DmOutcome::Committed);
        assert_eq!(ipad.process(id.clone(), home(2), commit, 6).unwrap().outcome, DmOutcome::Committed);
        for device in [&phone, &ipad] {
            assert!(device.members(id.clone()).unwrap().iter().all(|member| member.kind != DmLeafKind::Claktory));
        }
        // Gone with the group.
        ipad.skip_proposals(id.clone(), 9).unwrap();
        assert_eq!(ipad.proposal_seq(id.clone()).unwrap(), 9);
        ipad.forget_group(id.clone()).unwrap();
        assert_eq!(ipad.proposal_seq(id.clone()).unwrap(), 0);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn something_a_claktory_says_in_a_conversation_is_dropped_and_never_shown() {
        // #451 (Fugo, Mista 32498): a home can't see who sent a message, so
        // the friend's device is the check. Matt's Claktory, granted his
        // conversation with Ana, says something there with OpenMLS
        // directly, past every check on its own side: Ana's iPad drops it,
        // and nothing of it shows.
        use openmls::prelude::tls_codec::Serialize as _;
        let dir = folder();
        let (phone, ipad, mini, me, id, welcome) = claktory_in_a_conversation(&dir);

        // It joins from its Welcome and speaks, no check of ours in the way.
        let content = text("c1", "psst").to_content().unwrap().encode().unwrap();
        let said = mini
            .store()
            .with_tx(|provider, _| -> Result<Vec<u8>, claktory_dm_crypto::Error> {
                let mut group = joined(provider, &welcome);
                Ok(group.create_message(provider, &me.key, &content).unwrap().tls_serialize_detached().unwrap())
            })
            .unwrap();
        let processed = ipad.process(id.clone(), 3, wrap(said), 3).unwrap();
        assert_eq!(processed.outcome, DmOutcome::Dropped);
        assert!(processed.message.is_none());
        let shown: Vec<_> = ipad.history(id.clone(), None, 100).unwrap().into_iter().filter_map(|message| message.content.text).collect();
        assert!(!shown.iter().any(|text| text == "psst"), "{shown:?}");
        // The conversation goes on.
        let next = phone.send(id.clone(), text("m1", "still here"), 4, vec![]).unwrap();
        assert!(ipad.process(id.clone(), 4, next, 4).unwrap().message.is_some());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_list_names_the_home_and_a_friend_reads_it_and_the_store_keeps_going_over() {
        // #451: the person's list names their home Claktory, signed with the
        // rest; a friend reads it from the list they took. A list without
        // one reads as before. Going over to the home is kept in the store.
        let dir = folder();
        let path = dir.join("phone.db").to_string_lossy().into_owned();
        let (store_key, leaf_key, identity) = (dm_new_secret().unwrap(), dm_new_secret().unwrap(), dm_new_secret().unwrap());
        let phone = DmDevice::open(path.clone(), store_key.clone(), leaf_key.clone(), None).unwrap().unwrap();
        let matt = phone.set_up_first(identity.clone(), "matt".into(), "phone".into(), "iPhone".into(), 1).unwrap();
        let ipad = device(&dir, "ipad");
        ipad.set_up_first(dm_new_secret().unwrap(), "ana".into(), "ipad".into(), "iPad".into(), 1).unwrap();
        ipad.first_contact("matt".into(), matt.identity_public).unwrap();
        ipad.accept_list("matt".into(), matt.device_list).unwrap();
        assert_eq!(ipad.device_list("matt".into()).unwrap().unwrap().home, None);

        let leaves = phone.device_list("matt".into()).unwrap().unwrap().leaves;
        assert!(phone.sign_device_list_with_home(2, 2, leaves.clone(), Some(vec![1; 31])).is_err());
        let homed = phone.sign_device_list_with_home(2, 2, leaves, Some(vec![7; 32])).unwrap();
        ipad.accept_list("matt".into(), homed).unwrap();
        let read = ipad.device_list("matt".into()).unwrap().unwrap();
        assert_eq!((read.version, read.home), (2, Some(vec![7; 32])));

        assert!(!phone.home_drained().unwrap());
        phone.set_home_drained().unwrap();
        assert!(phone.home_drained().unwrap());
        drop(phone);
        let again = DmDevice::open(path, store_key, leaf_key, Some(identity)).unwrap().unwrap();
        assert!(again.home_drained().unwrap());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_store_reopens_as_it_was_and_not_with_another_key() {
        let dir = folder();
        let path = dir.join("phone.db").to_string_lossy().into_owned();
        let (store_key, leaf_key, identity) = (dm_new_secret().unwrap(), dm_new_secret().unwrap(), dm_new_secret().unwrap());
        let phone = DmDevice::open(path.clone(), store_key.clone(), leaf_key.clone(), None).unwrap().unwrap();
        phone.set_up_first(identity.clone(), "matt".into(), "phone".into(), "iPhone".into(), 1).unwrap();
        phone
            .create_group(
                b"self".to_vec(),
                DmGroupKind { kind: DmGroupType::SelfGroup, accounts: vec!["matt".into()], claktory_id: String::new() },
            )
            .unwrap();
        drop(phone);
        let again = DmDevice::open(path.clone(), store_key, leaf_key.clone(), Some(identity)).unwrap().unwrap();
        let status = again.status();
        assert!(status.set_up);
        assert_eq!((status.account_id.as_str(), status.leaf_id.as_str()), ("matt", "phone"));
        assert_eq!(again.groups().unwrap().len(), 1);
        assert!(again.pin("matt".into()).is_some());
        drop(again);
        assert!(DmDevice::open(path.clone(), dm_new_secret().unwrap(), leaf_key, None).unwrap().is_none());
        dm_remove_store(path).unwrap();
        std::fs::remove_dir_all(dir).unwrap();
    }
}
