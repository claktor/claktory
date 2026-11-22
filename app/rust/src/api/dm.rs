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
use claktory_dm_crypto::group::{Group, Incoming, LeafKey, Me};
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
    pub leaves: Vec<DmListed>,
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
    /// (never dropped: every commit after it would fail).
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
}

impl DmProcessed {
    fn of(outcome: DmOutcome) -> DmProcessed {
        DmProcessed { outcome, message: None, epoch: 0, why: String::new(), handshake: false }
    }

    fn message(message: DmMessage) -> DmProcessed {
        DmProcessed { message: Some(message), ..DmProcessed::of(DmOutcome::Message) }
    }

    fn committed(epoch: u64) -> DmProcessed {
        DmProcessed { epoch, ..DmProcessed::of(DmOutcome::Committed) }
    }
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

    fn save_pins(&self, tx: &Transaction<'_>, pins: &Pins) -> Result<(), Error> {
        self.save_row(tx, PINS, &Zeroizing::new(json(pins)?))
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
            self.save_pins(tx, &pins)
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
        let mut state = self.state();
        if state.me.is_some() {
            bail!("this device is set up already");
        }
        let identity = IdentityKey::from_secret(&bytes32(&identity_key)?);
        let public = identity.public().0.to_vec();
        let certificate = self.become_leaf(&mut state, identity, &account_id, &leaf_id, &name, now_ms)?;
        let listed = DmListed { kind: DmLeafKind::Device, leaf_id, signature_key: state.leaf.public().to_vec() };
        drop(state);
        let device_list = self.sign_device_list(1, now_ms, vec![listed])?;
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
            comparison: offer.comparison(),
        })
    }

    /// On the new device: opens what the scanning device sent ([sealed])
    /// and becomes the leaf its QR code named. The identity's secret comes
    /// back for the platform key store.
    pub fn finish_handoff(&self, handoff: &DmHandoff, sealed: Vec<u8>, now_ms: u64) -> anyhow::Result<DmHandoffDone> {
        let waiting = handoff
            .waiting
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take()
            .ok_or_else(|| anyhow!("this hand-off was used already"))?;
        let (leaf_id, name) = (waiting.offer.device.leaf_id.clone(), waiting.offer.device.name.clone());
        let received = waiting.open(&sealed)?;
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
    /// signed, and taken into this device's own pins.
    pub fn sign_device_list(&self, version: u64, created_ms: u64, leaves: Vec<DmListed>) -> anyhow::Result<Vec<u8>> {
        let mut state = self.state();
        let identity = state.identity.as_ref().ok_or_else(|| anyhow!("this device isn't set up"))?;
        let leaves = leaves
            .into_iter()
            .map(|leaf| Ok(Listed { kind: leaf.kind.into(), leaf_id: leaf.leaf_id, signature_key: bytes32(&leaf.signature_key)? }))
            .collect::<anyhow::Result<Vec<_>>>()?;
        let list = DeviceList { account_id: state.account_id.clone(), version, created_ms, leaves };
        let bytes = list.sign(identity)?.to_bytes()?;
        let account = state.account_id.clone();
        self.change_pins(&mut state, |pins| pins.accept_list(&account, &bytes))?;
        Ok(bytes)
    }

    fn change_pins(&self, state: &mut State, change: impl FnOnce(&mut Pins) -> Result<(), Error>) -> anyhow::Result<()> {
        let mut pins = state.pins.clone();
        change(&mut pins)?;
        self.store.with_tx(|_, tx| self.save_pins(tx, &pins))?;
        state.pins = pins;
        Ok(())
    }

    /// Pins [account]'s identity at first contact; a different key than
    /// the one pinned is refused (that's [key_changed]).
    pub fn first_contact(&self, account: String, identity_public: Vec<u8>) -> anyhow::Result<()> {
        let identity = IdentityPublic::from_slice(&identity_public)?;
        self.change_pins(&mut self.state(), |pins| pins.first_contact(&account, identity))
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
            leaves: signed
                .list
                .leaves
                .iter()
                .map(|leaf| DmListed { kind: leaf.kind.into(), leaf_id: leaf.leaf_id.clone(), signature_key: leaf.signature_key.to_vec() })
                .collect(),
        }))
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
            let (_, out) = Group::rejoin(provider, me, &state.pins, group_kind(&kind)?, info.bytes())?;
            Self::add_group(tx, &group_id, &kind, seq)?;
            Self::pending_commit(tx, &group_id, &out.commit)?;
            Ok::<_, Error>(out)
        })?;
        commit(out)
    }

    fn pending_commit(tx: &Transaction<'_>, group_id: &[u8], commit: &[u8]) -> Result<(), Error> {
        tx.execute("INSERT OR REPLACE INTO commits (grp, digest) VALUES (?1, ?2)", params![group_id, envelope::digest(commit).to_vec()])
            .map_err(sql)?;
        Ok(())
    }

    fn change_group(
        &self,
        group_id: &[u8],
        change: impl FnOnce(&mut Group, &Provider<'_>, &Me, &Pins) -> Result<claktory_dm_crypto::group::Outgoing, Error>,
    ) -> anyhow::Result<DmCommit> {
        let state = self.state();
        let me = state.me.as_ref().ok_or_else(|| anyhow!("this device isn't set up"))?;
        let out = self.store.with_tx(|provider, tx| {
            let (mut group, _) = Self::load(provider, tx, group_id)?;
            let out = change(&mut group, provider, me, &state.pins)?;
            Self::pending_commit(tx, group_id, &out.commit)?;
            Ok::<_, Error>(out)
        })?;
        commit(out)
    }

    /// Adds the leaves of [key_packages] (envelopes' JSON) to [group_id].
    pub fn add_members(&self, group_id: Vec<u8>, key_packages: Vec<String>) -> anyhow::Result<DmCommit> {
        let packages = key_packages.iter().map(|package| Ok(read_envelope(package)?.into_bytes())).collect::<anyhow::Result<Vec<_>>>()?;
        self.change_group(&group_id, |group, provider, me, pins| group.add(provider, me, pins, &packages))
    }

    /// Removes the leaves [leaf_ids] from [group_id].
    pub fn remove_members(&self, group_id: Vec<u8>, leaf_ids: Vec<String>) -> anyhow::Result<DmCommit> {
        let ids: Vec<&str> = leaf_ids.iter().map(String::as_str).collect();
        self.change_group(&group_id, |group, provider, me, _| group.remove(provider, me, &ids))
    }

    /// New keys for this device's leaf in [group_id].
    pub fn update_group(&self, group_id: Vec<u8>) -> anyhow::Result<DmCommit> {
        self.change_group(&group_id, |group, provider, me, _| group.update(provider, me))
    }

    /// Moves this device's leaf in [group_id] to its certificate under the
    /// person's new security key (same leaf, same signature key; dms-e2ee
    /// §2.1): the one commit a leaf under a replaced key may make there.
    pub fn move_group(&self, group_id: Vec<u8>) -> anyhow::Result<DmCommit> {
        self.change_group(&group_id, |group, provider, me, _| group.update_certificate(provider, me))
    }

    /// The worker took this device's commit to [group_id]: merge it.
    pub fn commit_taken(&self, group_id: Vec<u8>) -> anyhow::Result<()> {
        let _state = self.state();
        self.store.with_tx(|provider, tx| Self::merge_own(provider, tx, &group_id))?;
        Ok(())
    }

    fn merge_own(provider: &Provider<'_>, tx: &Transaction<'_>, group_id: &[u8]) -> Result<(), Error> {
        if tx.execute("DELETE FROM commits WHERE grp = ?1", [group_id]).map_err(sql)? == 0 {
            return Ok(());
        }
        let (mut group, _) = Self::load(provider, tx, group_id)?;
        group.merge_pending(provider)
    }

    /// The worker refused this device's commit (another came first): drop
    /// it; the change can be made again on the new epoch.
    pub fn commit_refused(&self, group_id: Vec<u8>) -> anyhow::Result<()> {
        let _state = self.state();
        self.store.with_tx(|provider, tx| {
            tx.execute("DELETE FROM commits WHERE grp = ?1", [&group_id]).map_err(sql)?;
            let (mut group, _) = Self::load(provider, tx, &group_id)?;
            group.clear_pending(provider)
        })?;
        Ok(())
    }

    /// Forgets [group_id]: its MLS state, history and what waits to go
    /// (revoked, unfriended, signed out).
    pub fn forget_group(&self, group_id: Vec<u8>) -> anyhow::Result<()> {
        let _state = self.state();
        self.store.with_tx(|_, tx| {
            claktory_dm_crypto::storage::forget_group(tx, &openmls_group_id(&group_id))?;
            tx.execute("DELETE FROM groups WHERE id = ?1", [&group_id]).map_err(sql)?;
            Ok::<_, Error>(())
        })?;
        Ok(())
    }

    /// Every group this device is in.
    pub fn groups(&self) -> anyhow::Result<Vec<DmGroup>> {
        let state = self.state();
        Ok(self.store.with_tx(|provider, tx| {
            let rows: Vec<(Vec<u8>, String, i64)> = tx
                .prepare("SELECT id, kind, seq FROM groups ORDER BY id")
                .map_err(sql)?
                .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
                .map_err(sql)?
                .collect::<Result<_, _>>()
                .map_err(sql)?;
            rows.into_iter()
                .map(|(group_id, kind, seq)| {
                    let kind: DmGroupKind = serde_json::from_str(&kind).map_err(|e| Error::Format(e.to_string()))?;
                    let group = Group::load(provider, group_kind(&kind)?, &group_id)?;
                    let epoch = group.as_ref().map(Group::epoch).unwrap_or(0);
                    let moved = match (&group, &state.me) {
                        (Some(group), Some(me)) => group.holds(me).unwrap_or(true),
                        _ => true,
                    };
                    Ok(DmGroup { group_id, kind, seq, epoch, moved })
                })
                .collect::<Result<Vec<_>, Error>>()
        })?)
    }

    /// Encrypts [content] for [group_id], once: asked again for the same
    /// retry id, it gives the same envelope (JSON) until that comes back.
    /// Refused in a conversation while the friend's security key changed
    /// and the person hasn't seen it ([seen]).
    pub fn send(&self, group_id: Vec<u8>, content: DmContent, now_ms: i64) -> anyhow::Result<String> {
        let id = content.client_message_id.clone();
        if id.is_empty() {
            bail!("a message has a retry id");
        }
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
                return Ok::<_, Error>(kept);
            }
            let (kind, _) = Self::group_row(tx, &group_id)?.ok_or_else(|| Error::Storage("a group this device isn't in".to_owned()))?;
            // A friend's security key changed and the person hasn't seen the
            // notice: nothing is encrypted to it (Fugo 2b), whatever calls.
            if kind.kind == DmGroupType::Conversation {
                for account in kind.accounts.iter().filter(|account| **account != state.account_id) {
                    if !state.pins.can_send(account) {
                        return Err(Error::Refused(format!("{account}'s security key changed and that hasn't been seen yet")));
                    }
                }
            }
            let (mut group, _) = Self::load(provider, tx, &group_id)?;
            let bytes = group.send(provider, me, &encoded)?;
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
        envelope_json(bytes)
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
                let before = snapshot(&Self::load(provider, tx, &group_id)?.0, &state.pins);
                Self::merge_own(provider, tx, &group_id)?;
                advance(tx)?;
                let (group, _) = Self::load(provider, tx, &group_id)?;
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
            let own: Option<(String, Vec<u8>)> = tx
                .query_row("SELECT client_id, value FROM outgoing WHERE grp = ?1 AND digest = ?2", params![group_id, digest], |row| {
                    Ok((row.get(0)?, row.get(1)?))
                })
                .optional()
                .map_err(sql)?;
            if let Some((id, sealed)) = own {
                let pending = self.store.open_row(&place(b"outgoing", &group_id, id.as_bytes(), b"value"), &sealed)?;
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
                };
                tx.execute("DELETE FROM outgoing WHERE grp = ?1 AND client_id = ?2", params![group_id, id]).map_err(sql)?;
                self.take(tx, state, &kind, &group_id, &message)?;
                advance(tx)?;
                return Ok(Ok(DmProcessed::message(message)));
            }
            let (mut group, _) = Self::load(provider, tx, &group_id)?;
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
                Err(error) if refusal(&error) => return Ok(Err(error.to_string())),
                Err(error) => return Err(error),
            };
            let processed = match incoming {
                Incoming::Application { from, content } => match Content::decode(&content) {
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
                        };
                        if kind.kind == DmGroupType::SelfGroup && message.content.kind == ROTATION_KIND {
                            let name = from.certificate.leaf.name.clone();
                            self.keep_rotation(tx, state, &group, &message, &name)?;
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
        if !message.from_kind_device || message.from_account != state.account_id {
            return Ok(());
        }
        let content = &message.content;
        let extra: serde_json::Map<String, serde_json::Value> = serde_json::from_str(&content.extra_json).unwrap_or_default();
        if content.kind == claktory_dm_crypto::content::kind::PINS {
            let Some(exported) = extra.get("pins") else { return Ok(()) };
            let mut theirs: Pins = serde_json::from_value(exported.clone()).map_err(|e| Error::Format(e.to_string()))?;
            theirs.reparse()?;
            state.pins.import(&theirs)?;
            return self.save_pins(tx, &state.pins);
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
        let missing = |from: &[DmMember], other: &Snapshot| -> Vec<DmMember> {
            from.iter().filter(|member| member.leaf_id != own_leaf && !other.ids.contains(&member.leaf_id)).cloned().collect()
        };
        // Another's leaves pass now and none did before, though they had
        // leaves here (one that moved, or one that no longer passes and
        // isn't the person's own: in a chat of two, theirs): their new key,
        // said at the first commit under it. Never for someone joining.
        let theirs_failed =
            before.ids.iter().any(|id| id != own_leaf && !own_listed.contains(id) && !before.roster.iter().any(|old| &old.leaf_id == id));
        let mut key_changed: Vec<String> = Vec::new();
        for member in &after.roster {
            let passed = before.roster.iter().any(|old| old.account == member.account);
            let had = before.ids.contains(&member.leaf_id) || theirs_failed;
            if had && !passed && member.account != own_account && !key_changed.contains(&member.account) {
                key_changed.push(member.account.clone());
            }
        }
        // Who came: those that pass. Who went: every leaf that was there,
        // checked or not, since one taken off its list fails the checks
        // before the commit removing it.
        let members = DmMembers { added: missing(&after.roster, &before), removed: missing(&before.all, &after), key_changed };
        if members.added.is_empty() && members.removed.is_empty() && members.key_changed.is_empty() {
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
        };
        self.keep(tx, group_id, &message)?;
        Ok(Some(message))
    }

    /// Who's in [group_id] now, as their certificates say (those that
    /// pass the checks now).
    pub fn members(&self, group_id: Vec<u8>) -> anyhow::Result<Vec<DmMember>> {
        let state = self.state();
        Ok(self.store.with_tx(|provider, tx| Ok::<_, Error>(roster(&Self::load(provider, tx, &group_id)?.0, &state.pins)))?)
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
        Ok(self.store.with_tx(|provider, tx| Ok::<_, Error>(unchecked(&Self::load(provider, tx, &group_id)?.0)))?)
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
            let Incoming::Application { from, content } = group.process(provider, &state.pins, envelope.bytes())? else {
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
            };
            let sealed = self.store.seal_row(&at, &Zeroizing::new(json(&message)?))?;
            tx.execute("INSERT INTO side (grp, digest, value) VALUES (?1, ?2, ?3)", params![group_id, digest, sealed]).map_err(sql)?;
            Ok(message)
        })?)
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
        self.store.with_tx(|_, tx| self.save_row(tx, REMOVAL, &json(&removal)?))?;
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
            self.save_pins(tx, &pins)?;
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
        if pin.identity == identity || pin.retired.contains(&identity) {
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
            self.save_pins(tx, &pins)?;
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
/// (to name), and every leaf's id (to tell who came and went).
#[frb(ignore)]
#[derive(Default)]
struct Snapshot {
    roster: Vec<DmMember>,
    ids: Vec<String>,
    /// Every leaf with what it claims, checked or not: for who went, since
    /// a leaf taken off its list fails the checks before its removal
    /// commits (Fugo).
    all: Vec<DmMember>,
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
    Snapshot { roster: roster(group, pins), ids: group.leaf_ids(), all: unchecked(group) }
}

/// Every leaf of [group] with what its certificate claims, unchecked: for
/// removal decisions and who went, never for trust.
fn unchecked(group: &Group) -> Vec<DmMember> {
    group
        .leaves_unchecked()
        .into_iter()
        .map(|leaf| DmMember {
            account: leaf.account_id,
            leaf_id: leaf.leaf_id,
            kind: leaf.kind.into(),
            name: leaf.name,
            since_ms: leaf.created_ms,
        })
        .collect()
}

/// The members of [group] that pass the checks now.
fn roster(group: &Group, pins: &Pins) -> Vec<DmMember> {
    group
        .members(pins)
        .into_iter()
        .flatten()
        .map(|member| DmMember {
            account: member.account().to_owned(),
            leaf_id: member.leaf_id().to_owned(),
            kind: member.kind().into(),
            name: member.certificate.leaf.name.clone(),
            since_ms: member.certificate.leaf.created_ms,
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

    #[test]
    fn two_people_talk_each_message_once_and_in_order() {
        let dir = folder();
        let (phone, ipad) = two(&dir);
        let id = b"conversation-1".to_vec();
        let info = read_envelope(&phone.create_group(id.clone(), chat()).unwrap()).unwrap();
        assert_eq!(info.parsed().kind, claktory_dm_crypto::envelope::Kind::GroupInfo);
        assert!(read_envelope(&ipad.last_resort_key_package().unwrap()).is_ok());
        let package = ipad.key_packages(1).unwrap().remove(0);
        let added = phone.add_members(id.clone(), vec![package]).unwrap();
        // The worker took the Add at seq 1; it comes back to the phone.
        let back = phone.process(id.clone(), 1, added.commit.clone(), 10).unwrap();
        assert_eq!((back.outcome, back.epoch), (DmOutcome::Committed, 1));
        // Who came, as a line of its own, from Ana's certificate.
        let came = back.message.unwrap().members.unwrap();
        assert_eq!((came.added[0].account.as_str(), came.added[0].name.as_str()), ("ana", "iPad"));
        assert!(came.removed.is_empty());
        assert_eq!(phone.members(id.clone()).unwrap().len(), 2);
        ipad.join_group(id.clone(), chat(), added.welcome.unwrap(), 1).unwrap();

        // Encrypted once: a retry gives the same bytes.
        let said = phone.send(id.clone(), text("m1", "hi Ana"), 20).unwrap();
        assert_eq!(phone.send(id.clone(), text("m1", "hi Ana"), 21).unwrap(), said);
        assert_eq!(phone.unsent().unwrap()[0].content.text.as_deref(), Some("hi Ana"));

        // At seq 2 for both: Ana reads it, and the phone knows it as its own.
        let heard = ipad.process(id.clone(), 2, said.clone(), 30).unwrap().message.unwrap();
        assert_eq!((heard.from_account.as_str(), heard.mine, heard.content.text.as_deref()), ("matt", false, Some("hi Ana")));
        let mine = phone.process(id.clone(), 2, said.clone(), 30).unwrap().message.unwrap();
        assert!(mine.mine);
        assert!(phone.unsent().unwrap().is_empty());
        // Again: already.
        assert_eq!(ipad.process(id.clone(), 2, said, 31).unwrap().outcome, DmOutcome::Already);

        // Ana answers; both keep the history, sealed, across a reopen.
        let answer = ipad.send(id.clone(), text("a1", "hi Matt"), 40).unwrap();
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
        assert!(phone.send(id.clone(), text("m2", "wait"), 22).is_err());
        phone.seen("ana".into()).unwrap();
        assert!(phone.send(id.clone(), text("m2", "now"), 23).is_ok());
        phone.drop_unsent(id.clone(), "m2".into()).unwrap();
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_bad_message_is_dropped_and_the_group_goes_on() {
        let dir = folder();
        let (phone, ipad) = two(&dir);
        let id = b"conversation-1".to_vec();
        phone.create_group(id.clone(), chat()).unwrap();
        let added = phone.add_members(id.clone(), vec![ipad.key_packages(1).unwrap().remove(0)]).unwrap();
        phone.commit_taken(id.clone()).unwrap();
        ipad.join_group(id.clone(), chat(), added.welcome.unwrap(), 1).unwrap();

        let junk = Envelope::new(vec![0, 1, 2, 3]).map(|e| e.to_json().to_string()).unwrap_or_else(|_| "{}".into());
        assert_eq!(ipad.process(id.clone(), 2, junk, 1).unwrap().outcome, DmOutcome::Dropped);
        assert_eq!(ipad.process(id.clone(), 3, "not json".into(), 1).unwrap().outcome, DmOutcome::Dropped);
        let said = phone.send(id.clone(), text("m1", "still here"), 2).unwrap();
        let heard = ipad.process(id.clone(), 4, said, 3).unwrap().message.unwrap();
        assert_eq!(heard.content.text.as_deref(), Some("still here"));
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
        phone.key_changed("ana".into(), new_key).unwrap();
        let pin = phone.pin("ana".into()).unwrap();
        assert!(pin.changed_unseen && !pin.can_send && !pin.verified);
        phone.seen("ana".into()).unwrap();
        assert!(phone.pin("ana".into()).unwrap().can_send);
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
        tablet.accept_list("matt".into(), list).unwrap();

        let me = b"self".to_vec();
        let mine = DmGroupKind { kind: DmGroupType::SelfGroup, accounts: vec!["matt".into()], claktory_id: String::new() };
        phone.create_group(me.clone(), mine.clone()).unwrap();
        let added = phone.add_members(me.clone(), vec![tablet.key_packages(1).unwrap().remove(0)]).unwrap();
        phone.process(me.clone(), 1, added.commit, 1).unwrap();
        tablet.join_group(me.clone(), mine, added.welcome.unwrap(), 1).unwrap();

        // A conversation with history on the tablet.
        let chat_id = b"conversation-1".to_vec();
        tablet.create_group(chat_id.clone(), chat()).unwrap();
        let with_ana = tablet.add_members(chat_id.clone(), vec![ipad.key_packages(1).unwrap().remove(0)]).unwrap();
        tablet.process(chat_id.clone(), 1, with_ana.commit, 1).unwrap();
        let said = tablet.send(chat_id.clone(), text("t1", "hello"), 2).unwrap();
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
            let envelope = phone.send(me.clone(), content, seq).unwrap();
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
        both(about("pins", None, serde_json::json!({"pins": pins})));
        assert_eq!(tablet.pin("bea".into()).unwrap().identity_public, phone.pin("bea".into()).unwrap().identity_public);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_proposal_is_decrypted_once_and_kept() {
        let dir = folder();
        let (phone, ipad) = two(&dir);
        let id = b"conversation-1".to_vec();
        phone.create_group(id.clone(), chat()).unwrap();
        let added = phone.add_members(id.clone(), vec![ipad.key_packages(1).unwrap().remove(0)]).unwrap();
        phone.commit_taken(id.clone()).unwrap();
        ipad.join_group(id.clone(), chat(), added.welcome.unwrap(), 1).unwrap();
        // Not through the feed: the side route, decrypted once.
        let said = phone.send(id.clone(), text("p1", "proposed"), 2).unwrap();
        let first = ipad.open_side(id.clone(), said.clone(), 3).unwrap();
        assert_eq!(first.content.text.as_deref(), Some("proposed"));
        assert_eq!(ipad.open_side(id.clone(), said, 4).unwrap(), first);
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
