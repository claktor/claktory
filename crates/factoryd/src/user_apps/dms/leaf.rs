//! This Claktory's leaf in the person's direct messages, end to end
//! (dms-e2ee.md §2 "Claktories", §5.2): its own MLS signature key, certified
//! by one of the person's devices, and through it membership in the
//! person's channel with this Claktory and in each conversation the person
//! gave one of its agents.
//!
//! - **Enrolled only over the person's own connection** (H2): a device asks
//!   for the leaf's key (`offer`, DmLeafOfferRequest) and gives back the
//!   certificate it signed with the person's identity key, with its pins
//!   (`enroll`, DmEnrollLeaf). The certificate must name this Claktory's
//!   registration and this key, and verify against the identity the
//!   person's pins hold for their own account. Never through claktory.com,
//!   which could otherwise swap in its own key.
//! - **One leaf a registration:** signing in again or moving (a new epoch)
//!   starts the store over; the person's device enrolls the new leaf.
//! - **A new security key** (dms-e2ee.md §2.1): a device enrolls the same
//!   leaf key again, under a certificate from the new identity and with pins
//!   that retired the old one; the loop then moves the leaf to it in every
//!   group (`process::rotate`), and it sends nothing from under the old one.
//! - **Trust comes from the person's devices,** never the service: the pins
//!   given at enrollment, and later ones sent in the channel (`pins`).
//! - **One writer:** every use is a `step`, one transaction on the store
//!   under one lock, never held across an await; the network is outside.

use std::sync::{Arc, Mutex, MutexGuard};

use claktory_dm_crypto::certificate::{LeafCertificate, LeafKind, claktory_leaf_id};
use claktory_dm_crypto::content::Content;
use claktory_dm_crypto::envelope::Envelope;
use claktory_dm_crypto::group::{Group, LeafKey, Me};
use claktory_dm_crypto::provider::Provider;
use claktory_dm_crypto::validation::{GroupKind, Pins};
use factory_protocol::v1;
use rusqlite::{OptionalExtension, Transaction, params};

pub(crate) use super::process::catch_up_channel;
use super::store::{self, OpenError, Store};
use crate::factory::Factory;
use crate::transcript::now_ms;

/// The leaf's store, opened on first use while the vault is open.
#[derive(Default)]
pub(crate) struct State {
    store: Mutex<Option<Store>>,
    /// Tests only: this Factory stands for a process that has ended, so
    /// its leaf never opens the store again (`dms::stop_leaf`).
    stopped: std::sync::atomic::AtomicBool,
    /// A full round is wanted now (an enrollment), not at the next one.
    pub(super) round_now: std::sync::atomic::AtomicBool,
    /// Why the channel stopped at a commit, if it did (`process`): one under
    /// the person's new security key, which this Claktory hasn't been given
    /// yet, or one with a leaf the newest device list doesn't name. Nothing
    /// is sent there until it reads on. Known again after a restart at the
    /// first catch-up.
    pub(super) channel_waits: Mutex<Option<Unready>>,
    /// When the channel may next be rejoined for a commit that stays
    /// refused (`process::unlisted`), and the wait after that one fails.
    pub(super) rejoin: Mutex<Rejoin>,
}

/// The throttle on rejoining the channel for a commit that stays refused
/// (Fugo, R5): one attempt, then a wait that doubles after each one that
/// fails, so a service that flaps can't make the leaf churn epochs.
#[derive(Default)]
pub(crate) struct Rejoin {
    pub next: Option<std::time::Instant>,
    pub wait: std::time::Duration,
}

impl State {
    fn lock(&self) -> MutexGuard<'_, Option<Store>> {
        self.store.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Why the channel waits, if it does.
    pub(crate) fn channel_waits(&self) -> Option<Unready> {
        self.channel_waits.lock().unwrap_or_else(std::sync::PoisonError::into_inner).clone()
    }

    pub(super) fn set_channel_waits(&self, why: Option<Unready>) {
        *self.channel_waits.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = why;
    }

    pub(super) fn rejoin(&self) -> MutexGuard<'_, Rejoin> {
        self.rejoin.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

/// Why the leaf can't act now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Unready {
    /// Signed out of the person's account.
    SignedOut,
    /// No vault to keep its keys with.
    NoVault,
    /// The vault is locked.
    Locked,
    /// A newer Claktory's store: left alone.
    Newer,
    /// No device of the person's has enrolled this Claktory yet.
    NotEnrolled,
    /// Enrolled, but not in its channel yet.
    NoChannel,
    /// The person made a new security key (dms-e2ee.md §2.1): this
    /// Claktory waits until a device of theirs enrolls it under the new one,
    /// then moves to it in every group, and sends nothing meanwhile.
    Rotating,
    /// A friend's security key changed in a conversation, and the person's
    /// devices haven't passed the new one on yet: the conversation waits.
    FriendsNewKey,
    /// A commit names a leaf that the newest device list this Claktory
    /// could fetch doesn't (R5): the group waits there, never skipping it,
    /// until a newer list names it (or, in the channel, it rejoins).
    Unlisted,
}

impl Unready {
    /// In words for an agent.
    pub(crate) fn words(&self, person: &str) -> String {
        match self {
            Unready::SignedOut => format!("This Claktory is signed out of {person}'s account, so their direct messages can't be reached."),
            Unready::NoVault => "This Claktory has no vault to keep its message keys in, so direct messages can't be used here.".to_owned(),
            Unready::Locked => format!("{person}'s secrets are locked on this Claktory, so direct messages wait until they're unlocked."),
            Unready::Newer => {
                "A newer version of Claktory set up direct messages here; this one can't use them. Update Claktory to use them again.".to_owned()
            }
            Unready::NotEnrolled | Unready::NoChannel => format!("{person} hasn't finished giving this Claktory the chat yet."),
            Unready::Rotating => format!("{person} changed their security key, and this Claktory is catching up. Try again in a few minutes."),
            Unready::FriendsNewKey => {
                format!("Their security key changed, and this Claktory waits until {person}'s devices pass the new one on. Try again later.")
            }
            Unready::Unlisted => format!("{person}'s devices changed, and this Claktory is catching up. Try again in a few minutes."),
        }
    }
}

#[derive(Debug)]
pub(crate) enum StepError {
    Unready(Unready),
    Failed(String),
}

impl From<String> for StepError {
    fn from(error: String) -> StepError {
        StepError::Failed(error)
    }
}

impl From<rusqlite::Error> for StepError {
    fn from(error: rusqlite::Error) -> StepError {
        StepError::Failed(error.to_string())
    }
}

impl From<claktory_dm_crypto::Error> for StepError {
    fn from(error: claktory_dm_crypto::Error) -> StepError {
        StepError::Failed(error.to_string())
    }
}

impl StepError {
    /// In words for an agent; a failure is logged and said plainly.
    pub(crate) fn words(&self, person: &str) -> String {
        match self {
            StepError::Unready(unready) => unready.words(person),
            StepError::Failed(error) => {
                tracing::warn!(%error, "a direct-message step failed");
                "Direct messages failed on this Claktory just now. Try again in a little while.".to_owned()
            }
        }
    }
}

/// A pending proposal's text, kept sealed until it's decided (`sends`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PendingProposal {
    pub client_message_id: String,
    /// Empty until the service has answered with it.
    pub proposal_id: Option<String>,
    pub agent: String,
    pub conversation: String,
    pub generation: i64,
    pub text: String,
}

/// The enrolled leaf: who it is and whom it trusts.
pub(crate) struct Leaf {
    pub account_id: String,
    pub claktory_id: String,
    pub me: Me,
    pub pins: Pins,
}

/// One step on the leaf's store: one transaction.
pub(crate) struct Step<'a> {
    tx: &'a Transaction<'a>,
    store: &'a StoreKeys<'a>,
    registration: (String, String, u64),
    channel_waits: Option<Unready>,
}

/// The store's key and cryptography, apart from its connection.
struct StoreKeys<'a> {
    key: &'a claktory_dm_crypto::storage::SealKey,
    crypto: &'a claktory_dm_crypto::provider::Crypto,
}

/// Opens the store if it isn't, as this Claktory's registration is now.
fn opened<'a>(factory: &Factory, guard: &'a mut MutexGuard<'_, Option<Store>>) -> Result<(&'a mut Store, (String, String, u64)), StepError> {
    if factory.dm_leaf.stopped.load(std::sync::atomic::Ordering::SeqCst) {
        **guard = None;
        return Err(StepError::Unready(Unready::Locked));
    }
    let config = factory.lock().config.account.clone();
    let Some(registration) = factory.account.registration(&config) else {
        // Signed out, or removed at the service: the leaf is gone for good,
        // so what it kept goes too (signing in again makes a new one).
        if guard.is_some() || store::dir(&factory.home).join("leaf.db").exists() {
            **guard = None;
            store::forget_all(&factory.home);
        }
        return Err(StepError::Unready(Unready::SignedOut));
    };
    if !factory.vault.is_open() {
        // Locked since it was opened: its key goes from memory too.
        **guard = None;
        return Err(StepError::Unready(Unready::Locked));
    }
    if guard.is_none() {
        let store = Store::open(&factory.home, &factory.vault).map_err(|error| match error {
            OpenError::NoVault => StepError::Unready(Unready::NoVault),
            OpenError::Locked => StepError::Unready(Unready::Locked),
            OpenError::Newer(format) => {
                tracing::warn!(format, "the direct-message store is a newer Claktory's: left alone");
                StepError::Unready(Unready::Newer)
            }
            OpenError::Failed(error) => StepError::Failed(error),
        })?;
        **guard = Some(store);
    }
    let store = guard.as_mut().expect("opened above");
    // A leaf of another registration (signed in again, moved): start over.
    let kept: Option<(String, String, i64)> = store
        .db
        .query_row("SELECT account_id, claktory_id, registration_epoch FROM leaf WHERE id = 1", [], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?))
        })
        .optional()?;
    if let Some((account, claktory, epoch)) = kept
        && (account != registration.0 || claktory != registration.1 || epoch as u64 != registration.2)
    {
        tracing::info!("a new registration: this Claktory's direct-message leaf starts over");
        **guard = None;
        store::forget_all(&factory.home);
        let store = Store::open(&factory.home, &factory.vault).map_err(|error| StepError::Failed(format!("{error:?}")))?;
        **guard = Some(store);
    }
    Ok((guard.as_mut().expect("opened above"), registration))
}

/// Runs `f` as one step: committed when it returns Ok, rolled back on Err.
/// Synchronous: never await inside it.
pub(crate) fn step<T>(factory: &Factory, f: impl FnOnce(&mut Step<'_>) -> Result<T, StepError>) -> Result<T, StepError> {
    let mut guard = factory.dm_leaf.lock();
    let (store, registration) = opened(factory, &mut guard)?;
    let Store { db, key, crypto } = store;
    let tx = db.transaction()?;
    let keys = StoreKeys { key, crypto };
    let channel_waits = factory.dm_leaf.channel_waits();
    let mut step = Step { tx: &tx, store: &keys, registration, channel_waits };
    let out = f(&mut step)?;
    drop(step);
    tx.commit()?;
    Ok(out)
}

const CHANNEL: &str = "channel";
const CONVERSATION: &str = "conversation";

impl Step<'_> {
    /// OpenMLS over this step's transaction.
    pub(crate) fn provider(&self) -> Result<Provider<'_>, StepError> {
        // The provider's randomness is a DRBG seeded once: Claktory never
        // forks without exec while one is live, so no child repeats it
        // (dms-e2ee.md §3, Fugo's condition 3).
        Ok(Provider::new(self.store.crypto, self.tx, self.store.key)?)
    }

    pub(crate) fn db(&self) -> &rusqlite::Connection {
        self.tx
    }

    pub(crate) fn seal(&self, place: &[&[u8]], plain: &[u8]) -> Result<Vec<u8>, StepError> {
        Ok(self.store.key.seal_row(place, plain)?)
    }

    pub(crate) fn open_sealed(&self, place: &[&[u8]], sealed: &[u8]) -> Result<zeroize::Zeroizing<Vec<u8>>, StepError> {
        Ok(self.store.key.open_row(place, sealed)?)
    }

    /// The account, Claktory id and registration epoch it acts for.
    pub(crate) fn registration(&self) -> &(String, String, u64) {
        &self.registration
    }

    /// The enrolled leaf.
    pub(crate) fn leaf(&self) -> Result<Leaf, StepError> {
        // Account, Claktory, sealed key, certificate, sealed pins.
        type Row = (String, String, Vec<u8>, Vec<u8>, Vec<u8>);
        let row: Option<Row> = self
            .tx
            .query_row("SELECT account_id, claktory_id, sealed_key, certificate, sealed_pins FROM leaf WHERE id = 1 AND enrolled > 0", [], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?))
            })
            .optional()?;
        let Some((account_id, claktory_id, sealed_key, certificate, sealed_pins)) = row else {
            return Err(StepError::Unready(Unready::NotEnrolled));
        };
        let key = self.leaf_key(&sealed_key)?;
        let me = Me::new(key, LeafCertificate::from_bytes(&certificate)?)?;
        let pins = self.open_pins(&sealed_pins)?;
        Ok(Leaf { account_id, claktory_id, me, pins })
    }

    fn leaf_key(&self, sealed: &[u8]) -> Result<LeafKey, StepError> {
        let secret = self.open_sealed(&[b"leaf", b"key"], sealed)?;
        let secret: [u8; 32] = secret.as_slice().try_into().map_err(|_| StepError::Failed("a leaf key isn't 32 bytes".to_owned()))?;
        Ok(LeafKey::from_secret(&secret))
    }

    /// The pins kept, if any yet.
    fn kept_pins(&self) -> Result<Option<Pins>, StepError> {
        let sealed: Option<Vec<u8>> =
            self.tx.query_row("SELECT sealed_pins FROM leaf WHERE id = 1", [], |row| row.get(0)).optional()?;
        match sealed {
            Some(sealed) if !sealed.is_empty() => Ok(Some(self.open_pins(&sealed)?)),
            _ => Ok(None),
        }
    }

    /// Takes `account`'s signed device list, if its pinned identity signed
    /// it and it's no older than the one kept.
    pub(crate) fn accept_list(&mut self, account: &str, list: &[u8]) -> Result<(), StepError> {
        let Some(mut pins) = self.kept_pins()? else { return Err(StepError::Unready(Unready::NotEnrolled)) };
        pins.accept_list(account, list)?;
        self.keep_pins(&pins)
    }

    /// The version of `account`'s device list kept, 0 for none.
    pub(crate) fn list_version(&self, account: &str) -> Result<u64, StepError> {
        Ok(self.kept_pins()?.and_then(|pins| pins.get(account).map(|pin| pin.highest_version)).unwrap_or(0))
    }

    /// The identity pinned for `account`, if any.
    pub(crate) fn pinned(&self, account: &str) -> Result<Option<[u8; 32]>, StepError> {
        Ok(self.kept_pins()?.and_then(|pins| pins.get(account).map(|pin| pin.identity.0)))
    }

    /// Takes pins from the person's own device (in the channel), merged
    /// as at enrollment: what disagrees is refused and nothing changes.
    pub(crate) fn import_pins(&mut self, given: &Pins) -> Result<(), StepError> {
        let mut pins = self.kept_pins()?.unwrap_or_default();
        pins.import(given)?;
        self.keep_pins(&pins)
    }

    fn open_pins(&self, sealed: &[u8]) -> Result<Pins, StepError> {
        let plain = self.open_sealed(&[b"leaf", b"pins"], sealed)?;
        let mut pins: Pins = serde_json::from_slice(&plain).map_err(|error| StepError::Failed(format!("pins: {error}")))?;
        pins.reparse()?;
        Ok(pins)
    }

    /// Keeps `pins` as what the leaf trusts now.
    pub(crate) fn keep_pins(&mut self, pins: &Pins) -> Result<(), StepError> {
        let json = zeroize::Zeroizing::new(serde_json::to_vec(pins).map_err(|error| StepError::Failed(error.to_string()))?);
        let sealed = self.seal(&[b"leaf", b"pins"], &json)?;
        self.tx.execute("UPDATE leaf SET sealed_pins = ?1 WHERE id = 1", [sealed])?;
        Ok(())
    }

    /// The person's channel with this Claktory, and its group id.
    fn channel_kind(&self, leaf: &Leaf) -> GroupKind {
        GroupKind::Channel { account: leaf.account_id.clone(), claktory_id: leaf.claktory_id.clone() }
    }

    /// The channel's group, loaded.
    pub(crate) fn channel(&self) -> Result<(Leaf, Group), StepError> {
        let leaf = self.leaf()?;
        let group_id: Option<Vec<u8>> =
            self.tx.query_row("SELECT group_id FROM groups WHERE kind = ?1", [CHANNEL], |row| row.get(0)).optional()?;
        let Some(group_id) = group_id else { return Err(StepError::Unready(Unready::NoChannel)) };
        let kind = self.channel_kind(&leaf);
        let group = Group::load(&self.provider()?, kind, &group_id)?.ok_or(StepError::Unready(Unready::NoChannel))?;
        Ok((leaf, group))
    }

    /// Encrypts `content` as one application message in this Claktory's
    /// channel, at its current epoch.
    pub(crate) fn channel_send(&mut self, content: &Content) -> Result<Envelope, StepError> {
        let (leaf, mut group) = self.channel()?;
        // Nothing goes from its place under a replaced key (the person's
        // devices would drop it), nor while the channel waits at a commit.
        if let Some(why) = &self.channel_waits {
            return Err(StepError::Unready(why.clone()));
        }
        if !group.holds(&leaf.me)? {
            return Err(StepError::Unready(Unready::Rotating));
        }
        // Nor while a dead device of the person's is still in it, after they
        // started over (row 630): it could read it until a device of theirs
        // takes it out.
        let bytes = match group.send(&self.provider()?, &leaf.me, &leaf.pins, &content.encode()?) {
            Err(claktory_dm_crypto::Error::Waiting { .. }) => return Err(StepError::Unready(Unready::Unlisted)),
            sent => sent?,
        };
        Ok(Envelope::new(bytes)?)
    }

    /// Keeps a pending proposal's text, by its client_message_id.
    pub(crate) fn keep_proposal(&mut self, proposal: &PendingProposal) -> Result<(), StepError> {
        let id = proposal.client_message_id.as_bytes();
        let sealed = self.seal(&[b"proposals", id, b"text"], proposal.text.as_bytes())?;
        self.tx.execute(
            "INSERT OR REPLACE INTO proposals (client_message_id, proposal_id, agent, conversation, generation, sealed_text)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                proposal.client_message_id,
                proposal.proposal_id.clone().unwrap_or_default(),
                proposal.agent,
                proposal.conversation,
                proposal.generation,
                sealed
            ],
        )?;
        Ok(())
    }

    pub(crate) fn proposal(&self, client_message_id: &str) -> Result<Option<PendingProposal>, StepError> {
        Ok(self.proposals_where("WHERE client_message_id = ?1", [client_message_id])?.pop())
    }

    pub(crate) fn proposals(&self) -> Result<Vec<PendingProposal>, StepError> {
        self.proposals_where("", [])
    }

    fn proposals_where<P: rusqlite::Params>(&self, filter: &str, params: P) -> Result<Vec<PendingProposal>, StepError> {
        let rows: Vec<(String, String, String, String, i64, Vec<u8>)> = self
            .tx
            .prepare(&format!(
                "SELECT client_message_id, proposal_id, agent, conversation, generation, sealed_text FROM proposals {filter} ORDER BY client_message_id"
            ))?
            .query_map(params, |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?, row.get(5)?)))?
            .collect::<rusqlite::Result<_>>()?;
        rows.into_iter()
            .map(|(client_message_id, proposal_id, agent, conversation, generation, sealed)| {
                let text = self.open_sealed(&[b"proposals", client_message_id.as_bytes(), b"text"], &sealed)?;
                let text = String::from_utf8(text.to_vec()).map_err(|_| StepError::Failed("a proposal's text isn't UTF-8".to_owned()))?;
                Ok(PendingProposal {
                    proposal_id: (!proposal_id.is_empty()).then_some(proposal_id),
                    client_message_id,
                    agent,
                    conversation,
                    generation,
                    text,
                })
            })
            .collect()
    }
}

/// Tests only: see `State::stopped`.
pub(crate) fn stop(factory: &Factory) {
    let mut guard = factory.dm_leaf.lock();
    factory.dm_leaf.stopped.store(true, std::sync::atomic::Ordering::SeqCst);
    *guard = None;
}

/// Forgets a proposal's text; needs no key.
pub(crate) fn forget_proposal(factory: &Factory, client_message_id: &str) {
    let _guard = factory.dm_leaf.lock();
    store::forget_proposal(&factory.home, client_message_id);
}

/// Forgets an agent's proposals' texts; needs no key.
pub(crate) fn forget_agent_proposals(factory: &Factory, agent: &str) {
    let _guard = factory.dm_leaf.lock();
    store::forget_agent_proposals(&factory.home, agent);
}

/// The leaf's key for the person's device to certify: made on the first
/// ask, the same until the registration changes.
pub(crate) fn offer(factory: &Arc<Factory>) -> Result<v1::DmLeafOffer, String> {
    let person = factory.lock().config.person_capitalized().to_owned();
    let name = one_line_name(&factory.identity().name);
    step(factory, |step| {
        let (_, claktory_id, epoch) = step.registration().clone();
        let account_id = step.registration().0.clone();
        let kept: Option<(Vec<u8>, i64)> =
            step.db().query_row("SELECT sealed_key, enrolled FROM leaf WHERE id = 1", [], |row| Ok((row.get(0)?, row.get(1)?))).optional()?;
        let (key, enrolled) = match kept {
            Some((sealed, enrolled)) => (step.leaf_key(&sealed)?, enrolled > 0),
            None => {
                let key = LeafKey::generate()?;
                let sealed = step.seal(&[b"leaf", b"key"], key.secret().as_slice())?;
                step.db().execute(
                    "INSERT INTO leaf (id, account_id, claktory_id, registration_epoch, sealed_key, created) VALUES (1, ?1, ?2, ?3, ?4, ?5)",
                    params![account_id, claktory_id, epoch as i64, sealed, now_ms()],
                )?;
                (key, false)
            }
        };
        Ok(v1::DmLeafOffer {
            leaf_signature_key: key.public().to_vec(),
            leaf_id: claktory_leaf_id(&claktory_id, epoch),
            claktory_id,
            registration_epoch: epoch,
            name: name.clone(),
            enrolled,
        })
    })
    .map_err(|error| error.words(&person))
}

/// The Claktory's name as a certificate takes it: one line, 64 characters.
fn one_line_name(name: &str) -> String {
    let name: String = name.chars().filter(|c| !claktory_dm_crypto::certificate::hidden(*c)).take(64).collect();
    let name = name.trim().to_owned();
    if name.is_empty() { "Claktory".to_owned() } else { name }
}

/// Takes the certificate the person's device signed for the offered key,
/// and the device's pins.
pub(crate) fn enroll(factory: &Arc<Factory>, request: &v1::DmEnrollLeaf) -> Result<(), String> {
    let person = factory.lock().config.person_capitalized().to_owned();
    let enrolled = step(factory, |step| {
        let (account_id, claktory_id, epoch) = step.registration().clone();
        let sealed_key: Option<Vec<u8>> = step.db().query_row("SELECT sealed_key FROM leaf WHERE id = 1", [], |row| row.get(0)).optional()?;
        let Some(sealed_key) = sealed_key else {
            return Err(StepError::Failed("no leaf was offered: ask for it first".to_owned()));
        };
        let key = step.leaf_key(&sealed_key)?;
        let certificate = LeafCertificate::from_bytes(&request.certificate)
            .map_err(|error| StepError::Failed(format!("the certificate doesn't read: {error}")))?;
        let leaf = &certificate.leaf;
        let refuse = |why: &str| Err(StepError::Failed(why.to_owned()));
        if leaf.kind != LeafKind::Claktory {
            return refuse("the certificate isn't for a Claktory");
        }
        if leaf.account_id != account_id {
            return refuse("the certificate is for another account");
        }
        if leaf.leaf_id != claktory_leaf_id(&claktory_id, epoch) {
            return refuse("the certificate is for another Claktory or registration");
        }
        if leaf.signature_key != key.public() {
            return refuse("the certificate is for another key");
        }
        let mut given: Pins = serde_json::from_slice(&request.pins).map_err(|_| StepError::Failed("the pins don't read".to_owned()))?;
        given.reparse().map_err(|error| StepError::Failed(format!("the pins don't read: {error}")))?;
        // Merged into what it has, never replacing it: the device may be
        // behind (a stale iPad), and a key retired stays retired (L5).
        let pins = match step.kept_pins()? {
            Some(mut kept) => {
                kept.import(&given).map_err(|_| {
                    StepError::Failed(
                        "that device disagrees with what this Claktory was given about a friend's security key; let it catch up with the person's other devices first".to_owned(),
                    )
                })?;
                kept
            }
            None => given,
        };
        let Some(own) = pins.get(&account_id) else {
            return refuse("the pins don't hold the person's own identity");
        };
        if certificate.identity != own.identity {
            return refuse("the certificate isn't signed with the person's identity");
        }
        // from_bytes verified the signature against the identity it names.
        step.keep_pins(&pins)?;
        step.db().execute(
            "UPDATE leaf SET certificate = ?1, enrolled = ?2 WHERE id = 1",
            params![request.certificate, now_ms()],
        )?;
        Ok(())
    });
    match enrolled {
        Ok(()) => {
            // A new certificate (the person's new security key) is moved to
            // in every group at once: a full round now.
            super::process::wake(factory);
            Ok(())
        }
        Err(StepError::Failed(why)) => Err(format!("This Claktory couldn't take the certificate: {why}.")),
        Err(error) => Err(error.words(&person)),
    }
}

/// Group bookkeeping for the processing loop.
impl Step<'_> {
    /// The groups the leaf is in: (group id, kind, conversation, generation, peer, position).
    pub(crate) fn groups(&self) -> Result<Vec<KeptGroup>, StepError> {
        let rows = self
            .tx
            .prepare("SELECT group_id, kind, conversation, generation, peer, position FROM groups ORDER BY joined")?
            .query_map([], |row| {
                Ok(KeptGroup {
                    group_id: row.get(0)?,
                    channel: row.get::<_, String>(1)? == CHANNEL,
                    conversation: row.get(2)?,
                    generation: row.get(3)?,
                    peer: row.get(4)?,
                    position: row.get(5)?,
                })
            })?
            .collect::<rusqlite::Result<_>>()?;
        Ok(rows)
    }

    /// Keeps a group the leaf joined, at `position`.
    pub(crate) fn keep_group(&mut self, group: &KeptGroup) -> Result<(), StepError> {
        self.tx.execute(
            "INSERT OR REPLACE INTO groups (group_id, kind, conversation, generation, peer, position, joined) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                group.group_id,
                if group.channel { CHANNEL } else { CONVERSATION },
                group.conversation,
                group.generation,
                group.peer,
                group.position,
                now_ms()
            ],
        )?;
        Ok(())
    }

    pub(crate) fn kind_of(&self, leaf: &Leaf, group: &KeptGroup) -> GroupKind {
        if group.channel {
            self.channel_kind(leaf)
        } else {
            let mut people = [leaf.account_id.clone(), group.peer.clone()];
            people.sort();
            GroupKind::Conversation { people }
        }
    }
}

/// A group the leaf is in, as the store keeps it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct KeptGroup {
    pub group_id: Vec<u8>,
    pub channel: bool,
    pub conversation: String,
    pub generation: i64,
    pub peer: String,
    pub position: i64,
}
