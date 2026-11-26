//! This Claktory as the person's direct-message mailbox, their "home"
//! (#451; dms-no-server-storage.md §2–§3; the seams in Mista's
//! dms-home-design.md §A). claktory.com keeps only the control plane
//! (commits, GroupInfo, KeyPackages, device lists, contacts): the messages
//! themselves go from the sender's device to its home, from that home to
//! each friend's home (`claktory-dm/1`, H2), and each home serves its own
//! person's leaves (their devices over the tunnel, H3; this Claktory's own
//! leaf, H5).
//!
//! - **Ciphertext only.** The home is a mailbox, never a member: it holds
//!   MLS messages as sent and reads only their public framing (group,
//!   epoch, kind) and the groups' public state (an OpenMLS `PublicGroup`
//!   each), as the worker did. So it works while the vault is locked.
//! - **Its own file,** `<home>/dm/home.db`, never factory.db: it can grow to
//!   gigabytes, and `dm/` stays out of moves and backups. A home that moves
//!   forwards what it holds instead.
//! - **One order a home.** Each group's log is numbered here, as it
//!   arrives. A home's person's devices all see the same order; the two
//!   people may see two messages sent at once in different orders.
//! - **Commits are the server's.** A commit is appended only from the
//!   person's own device (which hands it over once claktory.com took it) or
//!   pulled from claktory.com's log; one a friend's home passes on only
//!   makes this home look there (`wanted`). A message from an epoch this
//!   home has no commit for yet is held, unserved, until it has.
//! - **Scoped.** A friend's home is taken from only for the groups the
//!   person's device listed for it (`set_routes`, never an agent's or
//!   Rivet's), within its caps: 1 GiB held per friend, 5 GiB in all, and
//!   nothing while the disk has under 5 GiB free.
//! - **Kept until read.** An entry goes once every one of the person's
//!   leaves in its group has acked it, or after 90 days; a leaf that comes
//!   back after that rejoins, as a device away too long did before.
//! - **The person's other Claktories** read through it too: each one's
//!   leaf, once its signed hello bound it to its endpoint (`bind`, H2), is
//!   sent every entry of the groups it's in, and what it took counts as
//!   read. A Claktory that isn't the home has one route, to the home its
//!   person's signed device list names (`follow`), for every group its leaf
//!   is in.
//! - **One writer:** every call is one transaction on the file under one
//!   lock, never held across an await.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};

use claktory_dm_crypto::certificate::{LeafCertificate, LeafKind};
use claktory_dm_crypto::envelope::{Envelope, Kind as MlsKind};
use claktory_dm_crypto::provider::{Crypto, Provider};
use claktory_dm_crypto::storage::{self as mls_storage, SealKey};
use openmls::prelude::{GroupId, MlsMessageBodyIn, OpenMlsProvider, ProcessedMessageContent, ProposalStore, PublicGroup};
use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};
use tokio::sync::broadcast;

use crate::encoding::hex;
use crate::factory::Factory;
use crate::home::Home;
use crate::transcript::now_ms;

/// The file's format: a newer one is left alone.
pub(crate) const FORMAT: i64 = 1;
const FILE: &str = "home.db";

/// How long an entry, a held message or a leaf's place is kept.
pub(crate) const KEEP_MS: i64 = 90 * 86_400_000;
/// How long a Claktory that stopped being the home hands what it had queued
/// for friends' homes on to the new one (Fugo 33779, condition 3).
pub(crate) const HANDOVER_MS: i64 = 7 * 86_400_000;
/// How many epochs behind an application message may be: members keep the
/// secrets of this many past epochs (`claktory_dm_crypto::group`).
pub(crate) const EPOCHS_BEHIND: u64 = 5;
/// What one friend may have held here, and everyone together.
pub(crate) const FRIEND_BYTES: i64 = 1 << 30;
pub(crate) const TOTAL_BYTES: i64 = 5 << 30;
/// Intake stops while the disk has less than this free.
pub(crate) const FREE_FLOOR: u64 = 5 << 30;
/// When a refused friend or device should try again.
const FULL_RETRY_MS: i64 = 15 * 60_000;
/// A page: entries by default and at most, and bytes.
const PAGE: usize = 50;
const PAGE_MOST: usize = 100;
const PAGE_BYTES: usize = 256 * 1024;

const SCHEMA: &str = "
CREATE TABLE meta (
    name TEXT PRIMARY KEY,
    value BLOB NOT NULL
);
-- Each group the home holds: its kind (conversation, self or channel; ''
-- while it isn't known), the friend of a conversation, its epoch as the
-- commits here have it, the log's last seq and how far it was pruned, and
-- from which epoch both people have been in it (conversations; NULL while
-- one isn't). public_ok: its PublicGroup took every commit; when one
-- didn't, the home asks for the group's GroupInfo again (`wanted`).
CREATE TABLE groups (
    group_id BLOB PRIMARY KEY,
    kind TEXT NOT NULL,
    friend TEXT,
    epoch INTEGER,
    -- Home seqs start past 2^32, above any seq the worker gave, so a device
    -- that read the worker's log never takes one for already read.
    last_seq INTEGER NOT NULL DEFAULT 4294967296,
    pruned_seq INTEGER NOT NULL DEFAULT 0,
    both_since INTEGER,
    public_ok INTEGER NOT NULL DEFAULT 0,
    -- A friend's home said there's a commit past this epoch.
    hinted INTEGER,
    created INTEGER NOT NULL
);
-- The log: MLS messages as sent, in the order this home took them.
-- account: whose leaf sent it; leaf_id and client_message_id: the
-- person's own leaf's, for its receipts.
CREATE TABLE log (
    group_id BLOB NOT NULL,
    seq INTEGER NOT NULL,
    kind TEXT NOT NULL,
    epoch INTEGER NOT NULL,
    envelope BLOB NOT NULL,
    digest BLOB NOT NULL,
    account TEXT NOT NULL,
    mine INTEGER NOT NULL,
    leaf_id TEXT,
    client_message_id TEXT,
    accepted INTEGER NOT NULL,
    PRIMARY KEY (group_id, seq)
) WITHOUT ROWID;
CREATE UNIQUE INDEX log_digest ON log (group_id, digest);
CREATE UNIQUE INDEX log_client ON log (group_id, leaf_id, client_message_id) WHERE client_message_id IS NOT NULL;
CREATE INDEX log_accepted ON log (accepted);
CREATE INDEX log_account ON log (account) WHERE mine = 0;
-- What left the log, for 90 days, so a retry isn't taken twice.
CREATE TABLE gone (
    group_id BLOB NOT NULL,
    digest BLOB NOT NULL,
    seq INTEGER NOT NULL,
    at INTEGER NOT NULL,
    PRIMARY KEY (group_id, digest)
) WITHOUT ROWID;
-- Messages from an epoch the home has no commit for yet, or for a group it
-- doesn't know yet, in the order they came.
CREATE TABLE held (
    id INTEGER PRIMARY KEY,
    group_id BLOB NOT NULL,
    kind TEXT NOT NULL,
    epoch INTEGER NOT NULL,
    envelope BLOB NOT NULL,
    digest BLOB NOT NULL,
    account TEXT NOT NULL,
    mine INTEGER NOT NULL,
    leaf_id TEXT,
    client_message_id TEXT,
    received INTEGER NOT NULL
);
CREATE UNIQUE INDEX held_digest ON held (group_id, digest);
-- The person's own leaves in each group, as its PublicGroup has them.
CREATE TABLE members (
    group_id BLOB NOT NULL,
    leaf_id TEXT NOT NULL,
    PRIMARY KEY (group_id, leaf_id)
) WITHOUT ROWID;
-- How far each of the person's leaves has kept each group, and when it
-- last said so.
CREATE TABLE cursors (
    leaf_id TEXT NOT NULL,
    group_id BLOB NOT NULL,
    acked INTEGER NOT NULL,
    touched INTEGER NOT NULL,
    PRIMARY KEY (leaf_id, group_id)
) WITHOUT ROWID;
-- The routing table the person's device gave: whose home (or the person's
-- other Claktory) is at each endpoint, and for which groups.
CREATE TABLE routes (
    endpoint TEXT NOT NULL,
    account TEXT NOT NULL,
    group_id BLOB NOT NULL,
    PRIMARY KEY (endpoint, group_id)
) WITHOUT ROWID;
CREATE INDEX routes_group ON routes (group_id);
-- What goes to each endpoint, until it took it (H2 sends it).
CREATE TABLE outbox (
    endpoint TEXT NOT NULL,
    message_id TEXT NOT NULL,
    group_id BLOB NOT NULL,
    envelope BLOB NOT NULL,
    leaf_id TEXT,
    client_message_id TEXT,
    queued INTEGER NOT NULL,
    PRIMARY KEY (endpoint, message_id)
) WITHOUT ROWID;
-- What went, for 90 days: a bounce is taken only for an id sent there.
CREATE TABLE sent (
    endpoint TEXT NOT NULL,
    message_id TEXT NOT NULL,
    group_id BLOB NOT NULL,
    leaf_id TEXT,
    client_message_id TEXT,
    at INTEGER NOT NULL,
    PRIMARY KEY (endpoint, message_id)
) WITHOUT ROWID;
";

/// Run at every open: tables added since a store's first version, and the
/// views over the routes (a connection's own).
const EVERY_OPEN: &str = "
-- The person's other Claktories this home serves: each one's leaf, at the
-- endpoint its latest signed hello came from (H2 checks the hello). Apart
-- from the device's routes, which set_routes replaces whole.
CREATE TABLE IF NOT EXISTS bound (
    leaf_id TEXT PRIMARY KEY,
    endpoint TEXT NOT NULL UNIQUE,
    at_ms INTEGER NOT NULL
);
-- Not the home: the person's home, as their signed device list names it,
-- for each group this Claktory's leaf is in.
CREATE TABLE IF NOT EXISTS toward (
    endpoint TEXT NOT NULL,
    group_id BLOB NOT NULL,
    PRIMARY KEY (endpoint, group_id)
) WITHOUT ROWID;
-- The person's leaves a commit took out of a group: each reads on to that
-- commit (through), so it learns it was taken out, and no further.
CREATE TABLE IF NOT EXISTS left_at (
    group_id BLOB NOT NULL,
    leaf_id TEXT NOT NULL,
    through INTEGER NOT NULL,
    PRIMARY KEY (group_id, leaf_id)
) WITHOUT ROWID;
-- What this Claktory had queued for friends' homes (each friend's account)
-- when it stopped being the home: handed on to the person's new home until
-- `until`, or queued for those friends again if it's the home again first.
CREATE TABLE IF NOT EXISTS handover (
    message_id TEXT NOT NULL,
    account TEXT NOT NULL,
    group_id BLOB NOT NULL,
    envelope BLOB NOT NULL,
    leaf_id TEXT NOT NULL,
    client_message_id TEXT NOT NULL,
    queued INTEGER NOT NULL,
    until INTEGER NOT NULL,
    PRIMARY KEY (message_id, account)
) WITHOUT ROWID;
-- What this Claktory had taken that one of the person's leaves hadn't read
-- yet when it stopped being the home (application messages only): passed
-- on to the person's new home until `until`, each kept until that home
-- took it (Zucchero's row 567, Fugo 39876). `account`: whose, as a hint.
CREATE TABLE IF NOT EXISTS pass_on (
    message_id TEXT PRIMARY KEY,
    group_id BLOB NOT NULL,
    envelope BLOB NOT NULL,
    account TEXT NOT NULL,
    queued INTEGER NOT NULL,
    until INTEGER NOT NULL
) WITHOUT ROWID;
-- What the person's former home passed on here: never passed on again
-- (Fugo 39876, condition 3).
CREATE TABLE IF NOT EXISTS passed_in (
    group_id BLOB NOT NULL,
    digest BLOB NOT NULL,
    at INTEGER NOT NULL,
    PRIMARY KEY (group_id, digest)
) WITHOUT ROWID;
-- A friend's new home proved itself with the friend's signed device list
-- (row 595, Fugo 41473): the highest version taken from such a list for
-- that friend's account, and the home it named, so an older list never
-- moves the route back. The device's routes still replace the routes.
CREATE TABLE IF NOT EXISTS friend_homes (
    account TEXT PRIMARY KEY,
    version INTEGER NOT NULL,
    endpoint TEXT NOT NULL,
    at INTEGER NOT NULL
);
-- Where the person's own messages go as they come in: friends' homes, and
-- the person's home from a Claktory that isn't it.
CREATE TEMP VIEW IF NOT EXISTS onward (endpoint, account, group_id) AS
    SELECT endpoint, account, group_id FROM main.routes
    UNION SELECT endpoint, CAST((SELECT value FROM main.meta WHERE name = 'account') AS TEXT), group_id FROM main.toward;
-- Every endpoint the home talks to, for which groups: those, and the
-- person's other Claktories bound here, for their leaf's groups.
CREATE TEMP VIEW IF NOT EXISTS reach (endpoint, account, group_id) AS
    SELECT endpoint, account, group_id FROM temp.onward
    UNION SELECT b.endpoint, CAST((SELECT value FROM main.meta WHERE name = 'account') AS TEXT), m.group_id
        FROM main.bound b JOIN main.members m ON m.leaf_id = b.leaf_id;
";

/// Where a message came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Source {
    /// One of the person's own devices, over the tunnel (H3): only while
    /// this Claktory is their home.
    Device { leaf_id: String, client_message_id: String },
    /// This Claktory's own leaf (H5): its channel's drafts and proposals,
    /// and its commits once the server took them, whether or not it's the
    /// home (it goes on to the home through the routes).
    Local { leaf_id: String, client_message_id: String },
    /// A home (or the person's other Claktory) at this endpoint (H2).
    Home { endpoint: String, message_id: String },
    /// A commit pulled from claktory.com's log.
    Server,
    /// The person's former home, bound here by its hello, handing on what
    /// it had queued for friends' homes when it stopped being the home
    /// (`hand_over`): a message of one of the person's leaves (`leaf_id`)
    /// in a group it's in now, taken as that leaf's device's would be. Only
    /// while this Claktory is the home.
    HandOver { endpoint: String, message_id: String, leaf_id: String, client_message_id: String },
    /// The person's former home, bound here by its hello, passing on what
    /// it had taken that their leaves hadn't all read (`hand_over`): an
    /// application message only, for a group the person is in now, for
    /// their leaves only (never queued for a friend's home, never passed
    /// on again). `account` is a hint only. Only while this Claktory is
    /// the home.
    PassOn { endpoint: String, message_id: String, account: String },
}

/// A message to take.
#[derive(Debug, Clone)]
pub(crate) struct Intake {
    pub group_id: Vec<u8>,
    /// The MLS message, as sent.
    pub envelope: Vec<u8>,
    pub source: Source,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Taken {
    /// In the log, at `seq` (`duplicate`: it already was).
    Stored { seq: i64, accepted_ms: i64, duplicate: bool },
    /// Kept until the home has its epoch's commit (or its group).
    Held,
    /// More than 5 epochs behind this home's `epoch`: the sender
    /// re-encrypts it (a friend's home's, the scoped bounce).
    Bounce { epoch: u64 },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Refusal {
    /// That endpoint isn't in the routing table for that group.
    NotRouted,
    /// A conversation the other person has no device in yet.
    Waiting,
    /// The same id from the same leaf with another message.
    Conflict,
    /// Not an MLS message this home takes, or not this group's.
    Invalid(String),
    /// Over a cap, or the disk is nearly full.
    Full { retry_after_ms: i64 },
    /// This Claktory can't be a home now (signed out, a newer file).
    Unready(String),
    /// The person's signed device list doesn't name this Claktory as their
    /// home (yet).
    NotHome,
}

impl From<rusqlite::Error> for Refusal {
    fn from(error: rusqlite::Error) -> Refusal {
        Refusal::Unready(format!("the home's store failed: {error}"))
    }
}

/// What H2 sends to an endpoint.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Outgoing {
    pub message_id: String,
    pub group_id: Vec<u8>,
    pub envelope: Vec<u8>,
    /// Handed on to the person's new home (`hand_over`): the leaf that sent
    /// it and its client id, for `Source::HandOver` there.
    pub handover: Option<(String, String)>,
    /// Passed on to the person's new home (`hand_over`): whose it is, a
    /// hint, for `Source::PassOn` there.
    pub pass_on: Option<String>,
}

/// One entry of the routing table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Route {
    pub account: String,
    /// Its iroh endpoint id, lowercase hex.
    pub endpoint: String,
    pub group_ids: Vec<Vec<u8>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct GroupView {
    pub group_id: Vec<u8>,
    pub kind: String,
    pub epoch: u64,
    pub last_seq: i64,
    /// Where the leaf is: its kept place, or the seq of the commit that
    /// added it. DmFollow reads on from here for a group it has no place in.
    pub place: i64,
    /// How far the log was let go of: a reader whose own place is before
    /// it is behind.
    pub pruned: i64,
    /// Pruned past where this leaf is: it rejoins.
    pub behind: bool,
}

/// An entry as a leaf reads it (the worker's shape).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Entry {
    pub group_id: Vec<u8>,
    pub seq: i64,
    /// "application" or "commit".
    pub kind: String,
    pub epoch: u64,
    pub envelope: Vec<u8>,
    pub accepted_ms: i64,
    /// From the person's own leaf.
    pub from_me: bool,
    /// The reading leaf's own messages only.
    pub client_message_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Page {
    pub entries: Vec<Entry>,
    pub has_more: bool,
}

/// Pruned past where the leaf asked to read from: it rejoins.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TooFarBehind {
    pub epoch: u64,
}

/// What happened, for H3's pushes and H5's wakes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Arrival {
    Entry { group_id: Vec<u8>, seq: i64 },
    /// An endpoint took one of the person's messages.
    Delivered { leaf_id: String, client_message_id: String, group_id: Vec<u8>, endpoint: String },
    /// A friend's home bounced it: re-encrypt at `epoch`.
    Bounced { leaf_id: String, client_message_id: String, group_id: Vec<u8>, epoch: u64, endpoint: String },
}

/// What the home needs from claktory.com: a group's GroupInfo (a group it
/// holds messages for but has no state of, or whose state fell out of step),
/// or its commits after `after_epoch`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Want {
    pub group_id: Vec<u8>,
    pub group_info: bool,
    pub after_epoch: Option<u64>,
}

/// The home's place on the Factory.
pub(crate) struct State {
    store: Mutex<Option<Store>>,
    arrivals: broadcast::Sender<Arrival>,
    /// The person's current signed device list names this Claktory as their
    /// home (H3 sets it from the list): until it does, nothing but the
    /// server's commits is taken.
    is_home: std::sync::atomic::AtomicBool,
    /// Not the home, and the list names one: this Claktory's leaf reads
    /// through its own home.db, which the person's home fills.
    away: std::sync::atomic::AtomicBool,
    /// Wakes the pull from claktory.com (`monitor`).
    pull: std::sync::Arc<tokio::sync::Notify>,
}

impl Default for State {
    fn default() -> State {
        State {
            store: Mutex::new(None),
            arrivals: broadcast::channel(1024).0,
            is_home: std::sync::atomic::AtomicBool::new(false),
            away: std::sync::atomic::AtomicBool::new(false),
            pull: Default::default(),
        }
    }
}

/// Whether this Claktory is the person's home now (DmHome.is_home).
pub(crate) fn is_home(factory: &Factory) -> bool {
    factory.dm_home.is_home.load(std::sync::atomic::Ordering::SeqCst)
}

/// Whether the person's current signed device list names this Claktory as
/// their home (the list watcher, from the list it verified).
pub(crate) fn set_is_home(factory: &Factory, is_home: bool) {
    factory.dm_home.is_home.store(is_home, std::sync::atomic::Ordering::SeqCst);
}

struct Store {
    db: Connection,
    account: String,
    key: SealKey,
    crypto: Crypto,
}

fn dir(home: &Home) -> PathBuf {
    super::store::dir(home)
}

fn path(home: &Home) -> PathBuf {
    dir(home).join(FILE)
}

fn connect(path: &Path) -> rusqlite::Result<Connection> {
    let db = Connection::open(path)?;
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
    }
    db.busy_timeout(std::time::Duration::from_secs(5))?;
    // The PublicGroups' storage needs it, though nothing here is secret.
    db.pragma_update(None, "secure_delete", "ON")?;
    let _: String = db.query_row("PRAGMA journal_mode = WAL", [], |row| row.get(0))?;
    Ok(db)
}

fn meta(db: &Connection, name: &str) -> rusqlite::Result<Option<Vec<u8>>> {
    db.query_row("SELECT value FROM meta WHERE name = ?1", [name], |row| row.get(0)).optional()
}

/// Forgets the whole home: signed out, or another account.
fn forget_all(home: &Home) {
    let path = path(home);
    for file in [path.clone(), path.with_extension("db-wal"), path.with_extension("db-shm")] {
        if let Err(error) = std::fs::remove_file(&file)
            && error.kind() != std::io::ErrorKind::NotFound
        {
            tracing::warn!(%error, file = %file.display(), "couldn't delete the direct-message home");
        }
    }
}

impl Store {
    fn open(home: &Home, account: &str) -> Result<Store, Refusal> {
        let failed = |error: rusqlite::Error| Refusal::Unready(format!("the home's store doesn't open: {error}"));
        std::fs::create_dir_all(dir(home)).map_err(|error| Refusal::Unready(format!("no folder for the home: {error}")))?;
        let db = connect(&path(home)).map_err(failed)?;
        let made: bool = db
            .query_row("SELECT EXISTS (SELECT 1 FROM sqlite_master WHERE name = 'meta')", [], |row| row.get(0))
            .map_err(failed)?;
        if made {
            let format: Option<i64> = db
                .query_row("SELECT CAST(value AS INTEGER) FROM meta WHERE name = 'format'", [], |row| row.get(0))
                .optional()
                .map_err(failed)?;
            if format.is_some_and(|format| format > FORMAT) {
                return Err(Refusal::Unready("a newer Claktory's direct-message home: left alone".to_owned()));
            }
            let kept = meta(&db, "account").map_err(failed)?;
            if kept.as_deref() == Some(account.as_bytes()) {
                let key = meta(&db, "seal").map_err(failed)?.and_then(|bytes| <[u8; 32]>::try_from(bytes).ok());
                if let Some(key) = key {
                    db.execute_batch(EVERY_OPEN).map_err(failed)?;
                    let crypto = Crypto::new().map_err(|error| Refusal::Unready(error.to_string()))?;
                    return Ok(Store { db, account: account.to_owned(), key: SealKey::from_bytes(key), crypto });
                }
            }
            // Another account's, or broken: start over.
            drop(db);
            forget_all(home);
            return Store::open(home, account);
        }
        // The PublicGroups' rows are sealed like the worker's, though public:
        // a key of the file's own.
        let mut key = [0u8; 32];
        getrandom::fill(&mut key).map_err(|error| Refusal::Unready(format!("no randomness: {error}")))?;
        db.execute_batch(&format!("BEGIN; {SCHEMA} {} COMMIT;", mls_storage::SCHEMA)).map_err(failed)?;
        db.execute("INSERT INTO meta (name, value) VALUES ('format', ?1), ('account', ?2), ('seal', ?3)", params![FORMAT, account.as_bytes(), key.to_vec()])
            .map_err(failed)?;
        db.execute_batch(EVERY_OPEN).map_err(failed)?;
        let crypto = Crypto::new().map_err(|error| Refusal::Unready(error.to_string()))?;
        Ok(Store { db, account: account.to_owned(), key: SealKey::from_bytes(key), crypto })
    }
}

/// The person's account, while this Claktory is signed in.
fn account_of(factory: &Factory) -> Option<String> {
    let config = factory.lock().config.account.clone();
    factory.account.registration(&config).map(|(account, ..)| account)
}

fn lock(factory: &Factory) -> MutexGuard<'_, Option<Store>> {
    factory.dm_home.store.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// One transaction on the home, as the account is now.
fn with<T>(factory: &Factory, f: impl FnOnce(&mut Step<'_>) -> Result<T, Refusal>) -> Result<T, Refusal> {
    let Some(account) = account_of(factory) else {
        let mut guard = lock(factory);
        if guard.take().is_some() || path(&factory.home).exists() {
            forget_all(&factory.home);
        }
        return Err(Refusal::Unready("this Claktory is signed out".to_owned()));
    };
    let mut guard = lock(factory);
    if guard.as_ref().is_some_and(|store| store.account != account) {
        *guard = None;
    }
    if guard.is_none() {
        *guard = Some(Store::open(&factory.home, &account)?);
    }
    let is_home = factory.dm_home.is_home.load(std::sync::atomic::Ordering::SeqCst);
    let (out, arrivals) = run_on(guard.as_mut().expect("opened above"), &factory.home, is_home, f)?;
    drop(guard);
    for arrival in arrivals {
        let _ = factory.dm_home.arrivals.send(arrival);
    }
    Ok(out)
}

/// `f` as one transaction on `store`: what it returned, and what arrived.
fn run_on<T>(store: &mut Store, home: &Home, is_home: bool, f: impl FnOnce(&mut Step<'_>) -> Result<T, Refusal>) -> Result<(T, Vec<Arrival>), Refusal> {
    let Store { db, account, key, crypto } = store;
    let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let mut step = Step { tx: &tx, account, key, crypto, home, is_home, now: now_ms(), arrivals: Vec::new() };
    let out = f(&mut step)?;
    let arrivals = std::mem::take(&mut step.arrivals);
    drop(step);
    tx.commit()?;
    Ok((out, arrivals))
}

/// Where the home runs: the Factory, or a test's own store.
pub(crate) trait Host {
    fn run<T>(&self, f: impl FnOnce(&mut Step<'_>) -> Result<T, Refusal>) -> Result<T, Refusal>;
    fn home_dir(&self) -> &Home;
    /// Something to pull from claktory.com now.
    fn wake_pull(&self) {}
}

impl Host for Factory {
    fn run<T>(&self, f: impl FnOnce(&mut Step<'_>) -> Result<T, Refusal>) -> Result<T, Refusal> {
        with(self, f)
    }

    fn home_dir(&self) -> &Home {
        &self.home
    }

    fn wake_pull(&self) {
        self.dm_home.pull.notify_one();
    }
}

/// What happens next here, as it happens.
pub(crate) fn arrivals(factory: &Factory) -> broadcast::Receiver<Arrival> {
    factory.dm_home.arrivals.subscribe()
}

pub(crate) struct Step<'a> {
    tx: &'a Transaction<'a>,
    account: &'a str,
    key: &'a SealKey,
    crypto: &'a Crypto,
    home: &'a Home,
    is_home: bool,
    now: i64,
    arrivals: Vec<Arrival>,
}

/// A group as kept.
#[derive(Debug, Clone)]
struct Group {
    id: Vec<u8>,
    kind: String,
    friend: Option<String>,
    /// None until it has its state (a GroupInfo).
    epoch: Option<u64>,
    last_seq: i64,
    pruned_seq: i64,
    both_since: Option<i64>,
}

fn certified(credential: &openmls::prelude::Credential, signature_key: &[u8]) -> Option<LeafCertificate> {
    let certificate = LeafCertificate::from_credential(credential).ok()?;
    (certificate.leaf.signature_key.as_slice() == signature_key).then_some(certificate)
}

impl Step<'_> {
    fn group(&self, id: &[u8]) -> rusqlite::Result<Option<Group>> {
        self.tx
            .query_row(
                "SELECT group_id, kind, friend, epoch, last_seq, pruned_seq, both_since FROM groups WHERE group_id = ?1",
                [id],
                |row| {
                    Ok(Group {
                        id: row.get(0)?,
                        kind: row.get(1)?,
                        friend: row.get(2)?,
                        epoch: row.get::<_, Option<i64>>(3)?.map(|epoch| epoch as u64),
                        last_seq: row.get(4)?,
                        pruned_seq: row.get(5)?,
                        both_since: row.get(6)?,
                    })
                },
            )
            .optional()
    }

    /// The group's row, made (without state) if it isn't there: its seqs
    /// start where this Claktory's do since it last became the home
    /// (`seq_base`), or at `FIRST_SEQ` if it never did.
    fn group_or_new(&self, id: &[u8]) -> rusqlite::Result<Group> {
        self.tx.execute(
            "INSERT INTO groups (group_id, kind, created, last_seq)
             VALUES (?1, '', ?2, MAX(?3, COALESCE((SELECT CAST(value AS INTEGER) FROM meta WHERE name = 'seq_base'), 0) - 1))
             ON CONFLICT (group_id) DO NOTHING",
            params![id, self.now, FIRST_SEQ - 1],
        )?;
        Ok(self.group(id)?.expect("made above"))
    }

    fn provider(&self) -> Result<Provider<'_>, Refusal> {
        Provider::public(self.crypto, self.tx, self.key).map_err(|error| Refusal::Unready(error.to_string()))
    }

    /// The friend the routing table names for a group, if any endpoint of
    /// another account lists it.
    fn friend_of(&self, id: &[u8]) -> rusqlite::Result<Option<String>> {
        self.tx
            .query_row("SELECT account FROM routes WHERE group_id = ?1 AND account != ?2 LIMIT 1", params![id, self.account], |row| row.get(0))
            .optional()
    }

    /// Reads the group's members after its state changed: the person's own
    /// leaves (new ones start reading after `seq`, the commit that added
    /// them), its kind when it wasn't known, and since when both people of
    /// a conversation have been in it.
    fn members_changed(&mut self, group: &Group, public: &PublicGroup, seq: i64) -> rusqlite::Result<()> {
        let mut own = BTreeSet::new();
        let mut accounts = BTreeSet::new();
        let mut claktory = false;
        for member in public.members() {
            let Some(certificate) = certified(&member.credential, &member.signature_key) else { continue };
            let leaf = certificate.leaf;
            if leaf.kind == LeafKind::Device {
                accounts.insert(leaf.account_id.clone());
            }
            if leaf.account_id == self.account {
                claktory |= leaf.kind == LeafKind::Claktory;
                own.insert(leaf.leaf_id);
            }
        }
        let before: BTreeSet<String> = {
            let mut statement = self.tx.prepare("SELECT leaf_id FROM members WHERE group_id = ?1")?;
            statement.query_map([&group.id], |row| row.get(0))?.collect::<Result<_, _>>()?
        };
        // A leaf taken out still reads on to the commit that took it out
        // (`left_at`), its place kept; it's no longer waited for.
        for gone in before.difference(&own) {
            self.tx.execute("DELETE FROM members WHERE group_id = ?1 AND leaf_id = ?2", params![group.id, gone])?;
            self.tx.execute(
                "INSERT OR REPLACE INTO left_at (group_id, leaf_id, through) VALUES (?1, ?2, ?3)",
                params![group.id, gone, seq],
            )?;
        }
        for new in own.difference(&before) {
            self.tx.execute("INSERT INTO members (group_id, leaf_id) VALUES (?1, ?2)", params![group.id, new])?;
            self.tx.execute("DELETE FROM left_at WHERE group_id = ?1 AND leaf_id = ?2", params![group.id, new])?;
            // In again (a rejoin): it reads on from the commit that added it.
            self.tx.execute(
                "INSERT INTO cursors (leaf_id, group_id, acked, touched) VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT (leaf_id, group_id) DO UPDATE SET acked = MAX(acked, excluded.acked), touched = excluded.touched",
                params![new, group.id, seq, self.now],
            )?;
        }
        let friend = match &group.friend {
            Some(friend) => Some(friend.clone()),
            None => self.friend_of(&group.id)?.or_else(|| accounts.iter().find(|account| *account != self.account).cloned()),
        };
        // A channel starts as the device alone, then adds the Claktory's
        // leaf: "self" until then.
        let kind = if !group.kind.is_empty() && !(group.kind == "self" && claktory) {
            group.kind.clone()
        } else if friend.is_some() {
            "conversation".to_owned()
        } else if claktory {
            "channel".to_owned()
        } else {
            "self".to_owned()
        };
        let epoch = public.group_context().epoch().as_u64() as i64;
        let both_since = if kind == "conversation" {
            let both = friend.as_ref().is_some_and(|friend| accounts.contains(friend)) && accounts.contains(self.account);
            both.then(|| group.both_since.unwrap_or(epoch))
        } else {
            None
        };
        self.tx.execute(
            "UPDATE groups SET kind = ?2, friend = ?3, both_since = ?4, public_ok = 1 WHERE group_id = ?1",
            params![group.id, kind, friend, both_since],
        )?;
        Ok(())
    }

    /// Appends to a group's log: its seq.
    #[allow(clippy::too_many_arguments)]
    fn append(
        &mut self,
        group: &mut Group,
        kind: &str,
        epoch: u64,
        envelope: &[u8],
        digest: &[u8],
        account: &str,
        mine: bool,
        leaf: Option<(&str, &str)>,
    ) -> rusqlite::Result<i64> {
        let seq = group.last_seq + 1;
        let (leaf_id, client_message_id) = match leaf {
            Some((leaf, client)) => (Some(leaf), (!client.is_empty()).then_some(client)),
            None => (None, None),
        };
        self.tx.execute(
            "INSERT INTO log (group_id, seq, kind, epoch, envelope, digest, account, mine, leaf_id, client_message_id, accepted)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            params![group.id, seq, kind, epoch as i64, envelope, digest, account, mine, leaf_id, client_message_id, self.now],
        )?;
        self.tx.execute("UPDATE groups SET last_seq = ?2 WHERE group_id = ?1", params![group.id, seq])?;
        group.last_seq = seq;
        self.forward(&group.id, envelope, digest, leaf_id, self.now)?;
        self.arrivals.push(Arrival::Entry { group_id: group.id.clone(), seq });
        Ok(seq)
    }

    /// The seq of a message already here (in the log, or gone from it).
    fn known(&self, group: &[u8], digest: &[u8]) -> rusqlite::Result<Option<(i64, i64)>> {
        let logged = self
            .tx
            .query_row("SELECT seq, accepted FROM log WHERE group_id = ?1 AND digest = ?2", params![group, digest], |row| {
                Ok((row.get(0)?, row.get(1)?))
            })
            .optional()?;
        if logged.is_some() {
            return Ok(logged);
        }
        self.tx
            .query_row("SELECT seq, at FROM gone WHERE group_id = ?1 AND digest = ?2", params![group, digest], |row| Ok((row.get(0)?, row.get(1)?)))
            .optional()
    }

    /// What the person's former home passed on here: never passed on again.
    fn passed_in(&self, group: &[u8], digest: &[u8]) -> rusqlite::Result<()> {
        self.tx.execute(
            "INSERT INTO passed_in (group_id, digest, at) VALUES (?1, ?2, ?3) ON CONFLICT (group_id, digest) DO UPDATE SET at = ?3",
            params![group, digest, self.now],
        )?;
        Ok(())
    }

    fn is_held(&self, group: &[u8], digest: &[u8]) -> rusqlite::Result<bool> {
        self.tx.query_row("SELECT COUNT(*) > 0 FROM held WHERE group_id = ?1 AND digest = ?2", params![group, digest], |row| row.get(0))
    }

    #[allow(clippy::too_many_arguments)]
    fn hold(
        &self,
        group: &[u8],
        kind: &str,
        epoch: u64,
        envelope: &[u8],
        digest: &[u8],
        account: &str,
        mine: bool,
        leaf: Option<(&str, &str)>,
    ) -> rusqlite::Result<()> {
        let (leaf_id, client_message_id) = match leaf {
            Some((leaf, client)) => (Some(leaf), (!client.is_empty()).then_some(client)),
            None => (None, None),
        };
        self.tx.execute(
            "INSERT INTO held (group_id, kind, epoch, envelope, digest, account, mine, leaf_id, client_message_id, received)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10) ON CONFLICT (group_id, digest) DO NOTHING",
            params![group, kind, epoch as i64, envelope, digest, account, mine, leaf_id, client_message_id, self.now],
        )?;
        Ok(())
    }

    /// The caps on what a friend's home may leave here.
    fn room(&self, friend: &str, bytes: usize) -> Result<(), Refusal> {
        let full = Refusal::Full { retry_after_ms: FULL_RETRY_MS };
        let theirs: i64 = self.tx.query_row(
            "SELECT COALESCE((SELECT SUM(LENGTH(envelope)) FROM log WHERE mine = 0 AND account = ?1), 0)
                  + COALESCE((SELECT SUM(LENGTH(envelope)) FROM held WHERE mine = 0 AND account = ?1), 0)",
            [friend],
            |row| row.get(0),
        )?;
        let (friend_most, total_most) = caps();
        if theirs + bytes as i64 > friend_most {
            return Err(full);
        }
        let all: i64 = self.tx.query_row(
            "SELECT COALESCE((SELECT SUM(LENGTH(envelope)) FROM log), 0) + COALESCE((SELECT SUM(LENGTH(envelope)) FROM held), 0)",
            [],
            |row| row.get(0),
        )?;
        if all + bytes as i64 > total_most {
            return Err(full);
        }
        if free_bytes(self.home).is_some_and(|free| free < FREE_FLOOR) {
            return Err(full);
        }
        Ok(())
    }

    /// Queues an entry of the log for the person's other Claktories bound
    /// here whose leaf is in its group, but the leaf that sent it.
    fn forward(&self, group: &[u8], envelope: &[u8], digest: &[u8], from_leaf: Option<&str>, queued: i64) -> rusqlite::Result<()> {
        self.tx.execute(
            "INSERT INTO outbox (endpoint, message_id, group_id, envelope, leaf_id, client_message_id, queued)
             SELECT b.endpoint, ?2, ?1, ?3, NULL, NULL, ?5 FROM bound b JOIN members m ON m.leaf_id = b.leaf_id
             WHERE m.group_id = ?1 AND b.leaf_id IS NOT ?4
             ON CONFLICT (endpoint, message_id) DO NOTHING",
            params![group, hex(digest), envelope, from_leaf, queued],
        )?;
        Ok(())
    }

    /// Queues a message of the person's for every endpoint the routing
    /// table lists for its group.
    fn queue(&self, group: &[u8], envelope: &[u8], digest: &[u8], leaf: Option<(&str, &str)>) -> rusqlite::Result<()> {
        let (leaf_id, client_message_id) = match leaf {
            Some((leaf, client)) => (Some(leaf), (!client.is_empty()).then_some(client)),
            None => (None, None),
        };
        self.tx.execute(
            "INSERT INTO outbox (endpoint, message_id, group_id, envelope, leaf_id, client_message_id, queued)
             SELECT endpoint, ?2, ?1, ?3, ?4, ?5, ?6 FROM onward WHERE group_id = ?1
             ON CONFLICT (endpoint, message_id) DO NOTHING",
            params![group, hex(digest), envelope, leaf_id, client_message_id, self.now],
        )?;
        Ok(())
    }

    /// Takes a commit the server took: the group's next epoch. Its
    /// PublicGroup follows it; one it can't (a proposal it never saw, a
    /// state out of step) asks for the GroupInfo again, and the log goes on.
    fn commit(&mut self, group: &mut Group, parsed: claktory_dm_crypto::envelope::Parsed, envelope: &[u8], digest: &[u8], account: &str, leaf: Option<(&str, &str)>) -> Result<i64, Refusal> {
        let epoch = group.epoch.expect("only for a group with state");
        let seq = self.append(group, "commit", epoch, envelope, digest, account, account == self.account, leaf)?;
        let next = epoch + 1;
        self.tx.execute("UPDATE groups SET epoch = ?2 WHERE group_id = ?1", params![group.id, next as i64])?;
        group.epoch = Some(next);
        let followed = (|| -> Result<PublicGroup, String> {
            let provider = Provider::public(self.crypto, self.tx, self.key).map_err(|error| error.to_string())?;
            let id = GroupId::from_slice(&group.id);
            let mut public = PublicGroup::load(provider.storage(), &id).map_err(|error| error.to_string())?.ok_or("no state")?;
            let message = parsed.into_protocol_message().map_err(|error| error.to_string())?;
            let processed = public.process_message(provider.crypto(), message).map_err(|error| error.to_string())?;
            let ProcessedMessageContent::StagedCommitMessage(staged) = processed.into_content() else {
                return Err("not a commit".to_owned());
            };
            public.merge_commit(provider.storage(), *staged).map_err(|error| error.to_string())?;
            Ok(public)
        })();
        match followed {
            Ok(public) => self.members_changed(group, &public, seq)?,
            Err(error) => {
                tracing::info!(%error, "a commit the home's public state couldn't follow: it asks for the group's GroupInfo");
                self.tx.execute("UPDATE groups SET public_ok = 0 WHERE group_id = ?1", [&group.id])?;
            }
        }
        self.release(group)?;
        Ok(seq)
    }

    /// Moves what was held for epochs the group has reached into its log,
    /// in the order it came.
    fn release(&mut self, group: &mut Group) -> rusqlite::Result<()> {
        let Some(epoch) = group.epoch else { return Ok(()) };
        type Row = (i64, String, i64, Vec<u8>, Vec<u8>, String, bool, Option<String>, Option<String>);
        let rows: Vec<Row> = {
            let mut statement = self.tx.prepare(
                "SELECT id, kind, epoch, envelope, digest, account, mine, leaf_id, client_message_id FROM held
                 WHERE group_id = ?1 AND kind = 'application' AND epoch <= ?2 ORDER BY id",
            )?;
            statement
                .query_map(params![group.id, epoch as i64], |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                        row.get(5)?,
                        row.get(6)?,
                        row.get(7)?,
                        row.get(8)?,
                    ))
                })?
                .collect::<Result<_, _>>()?
        };
        for (id, kind, at, envelope, digest, account, mine, leaf_id, client) in rows {
            self.tx.execute("DELETE FROM held WHERE id = ?1", [id])?;
            if self.known(&group.id, &digest)?.is_some() {
                continue;
            }
            let leaf = leaf_id.as_deref().map(|leaf| (leaf, client.as_deref().unwrap_or("")));
            self.append(group, &kind, at as u64, &envelope, &digest, &account, mine, leaf)?;
        }
        Ok(())
    }

    fn take(&mut self, intake: Intake) -> Result<Taken, Refusal> {
        let invalid = |what: &str| Refusal::Invalid(what.to_owned());
        let envelope = Envelope::new(intake.envelope).map_err(|error| Refusal::Invalid(error.to_string()))?;
        let parsed = envelope.parsed();
        let (group_id, epoch) = parsed.group.clone().ok_or_else(|| invalid("Only application messages and commits go through a home."))?;
        if group_id.as_slice() != intake.group_id.as_slice() {
            return Err(invalid("That message is another group's."));
        }
        let kind = match parsed.kind {
            MlsKind::Application => "application",
            MlsKind::Commit => "commit",
            _ => return Err(invalid("Only application messages and commits go through a home.")),
        };
        if !(16..=32).contains(&intake.group_id.len()) {
            return Err(invalid("A group's id is 16 to 32 bytes."));
        }
        let digest = envelope.digest();
        let bytes = envelope.bytes();
        if !self.is_home && matches!(intake.source, Source::Device { .. } | Source::HandOver { .. } | Source::PassOn { .. }) {
            return Err(Refusal::NotHome);
        }

        // Who sent it, and as which account.
        let mut handed_by_claktory = false;
        let (account, leaf) = match &intake.source {
            Source::Device { leaf_id, client_message_id } | Source::Local { leaf_id, client_message_id } => {
                if kind == "application" && (client_message_id.is_empty() || client_message_id.len() > 64 || !client_message_id.bytes().all(|b| b.is_ascii_graphic())) {
                    return Err(invalid("client_message_id is 1 to 64 visible characters."));
                }
                (self.account.to_owned(), Some((leaf_id.clone(), client_message_id.clone())))
            }
            Source::Home { endpoint, message_id } => {
                let endpoint = endpoint.to_ascii_lowercase();
                let account: Option<String> = self
                    .tx
                    .query_row("SELECT account FROM reach WHERE endpoint = ?1 AND group_id = ?2", params![endpoint, intake.group_id], |row| row.get(0))
                    .optional()?;
                let Some(account) = account else { return Err(Refusal::NotRouted) };
                // Not the home: only the person's own home forwarding to
                // this Claktory's leaf, never a friend's.
                if !self.is_home && account != self.account {
                    return Err(Refusal::NotHome);
                }
                if *message_id != hex(&digest) {
                    return Err(invalid("A message's id is its envelope's SHA-256."));
                }
                // Another Claktory of the person's, bound here: its leaf
                // sent it, as this Claktory's own leaf's would be.
                let bound: Option<String> = if account == self.account {
                    self.tx.query_row("SELECT leaf_id FROM bound WHERE endpoint = ?1", [&endpoint], |row| row.get(0)).optional()?
                } else {
                    None
                };
                (account, bound.map(|leaf_id| (leaf_id, String::new())))
            }
            Source::Server => {
                if kind != "commit" {
                    return Err(invalid("Only commits come from the server."));
                }
                (String::new(), None)
            }
            Source::HandOver { endpoint, message_id, leaf_id, client_message_id } => {
                // From the person's Claktory bound here now (on their
                // current signed list: `unbind_missing`), read again at
                // every take.
                let endpoint = endpoint.to_ascii_lowercase();
                let bound: Option<String> =
                    self.tx.query_row("SELECT leaf_id FROM bound WHERE endpoint = ?1", [&endpoint], |row| row.get(0)).optional()?;
                if bound.is_none() {
                    return Err(Refusal::NotRouted);
                }
                if *message_id != hex(&digest) {
                    return Err(invalid("A message's id is its envelope's SHA-256."));
                }
                // Only a group the person is in now, from a leaf of theirs
                // in it.
                let member: bool = self.tx.query_row(
                    "SELECT COUNT(*) > 0 FROM members WHERE group_id = ?1 AND leaf_id = ?2",
                    params![intake.group_id, leaf_id],
                    |row| row.get(0),
                )?;
                if !member {
                    return Err(Refusal::NotRouted);
                }
                if kind == "application" && (client_message_id.is_empty() || client_message_id.len() > 64 || !client_message_id.bytes().all(|b| b.is_ascii_graphic())) {
                    return Err(invalid("client_message_id is 1 to 64 visible characters."));
                }
                handed_by_claktory = self.tx.query_row("SELECT COUNT(*) > 0 FROM bound WHERE leaf_id = ?1", [leaf_id], |row| row.get(0))?;
                (self.account.to_owned(), Some((leaf_id.clone(), client_message_id.clone())))
            }
            Source::PassOn { endpoint, message_id, account } => {
                // From the person's Claktory bound here now (on their
                // current signed list), read again at every take (Fugo
                // 39876, condition 1).
                let endpoint = endpoint.to_ascii_lowercase();
                let bound: bool = self.tx.query_row("SELECT COUNT(*) > 0 FROM bound WHERE endpoint = ?1", [&endpoint], |row| row.get(0))?;
                if !bound {
                    return Err(Refusal::NotRouted);
                }
                if *message_id != hex(&digest) {
                    return Err(invalid("A message's id is its envelope's SHA-256."));
                }
                // Application messages only (condition 2).
                if kind != "application" {
                    return Err(invalid("Only application messages are passed on."));
                }
                if account.is_empty() || account.len() > 64 || !account.bytes().all(|b| b.is_ascii_graphic()) {
                    return Err(invalid("An account is 1 to 64 visible characters."));
                }
                // The person's own too (row 591, Fugo 41196), but never as
                // theirs: every passed message lands as a friend's would
                // (`mine` false, no leaf or client id, queued to no one,
                // within its account's cap), so the hint steers nothing.
                // A group the person is in now. One this home has no state
                // of yet (nothing of it came here since the move): it asks
                // claktory.com for the group, and the former home tries
                // again shortly.
                let stated: bool = self
                    .tx
                    .query_row("SELECT epoch IS NOT NULL FROM groups WHERE group_id = ?1", [&intake.group_id], |row| row.get(0))
                    .optional()?
                    .unwrap_or(false);
                if !stated {
                    return Err(Refusal::Unready(STATELESS.to_owned()));
                }
                let member: bool =
                    self.tx.query_row("SELECT COUNT(*) > 0 FROM members WHERE group_id = ?1", [&intake.group_id], |row| row.get(0))?;
                if !member {
                    return Err(Refusal::NotRouted);
                }
                // A hint only (condition 4): who sent it is in its MLS
                // credential, which only the devices read.
                (account.clone(), None)
            }
        };
        let passed_on = matches!(intake.source, Source::PassOn { .. });
        // The person's own, unless a former home passed it on: then it
        // lands as a friend's would, whatever the hint says (Fugo 41196).
        let mine = account == self.account && !passed_on;
        let leaf_ref = leaf.as_ref().map(|(leaf, client)| (leaf.as_str(), client.as_str()));

        // The same message again.
        if let Some((seq, accepted_ms)) = self.known(&intake.group_id, &digest)? {
            return Ok(Taken::Stored { seq, accepted_ms, duplicate: true });
        }
        if let Some((leaf_id, client)) = &leaf
            && !client.is_empty()
        {
            let earlier: Option<i64> = self.tx.query_row(
                "SELECT COUNT(*) FROM log WHERE group_id = ?1 AND leaf_id = ?2 AND client_message_id = ?3",
                params![intake.group_id, leaf_id, client],
                |row| row.get(0),
            ).optional()?;
            let held_other: bool = self.tx.query_row(
                "SELECT COUNT(*) > 0 FROM held WHERE group_id = ?1 AND leaf_id = ?2 AND client_message_id = ?3 AND digest != ?4",
                params![intake.group_id, leaf_id, client, digest.to_vec()],
                |row| row.get(0),
            )?;
            if earlier.is_some_and(|count| count > 0) || held_other {
                return Err(Refusal::Conflict);
            }
        }
        if self.is_held(&intake.group_id, &digest)? {
            return Ok(Taken::Held);
        }

        let mut group = self.group_or_new(&intake.group_id)?;
        let from_friend = matches!(intake.source, Source::Home { .. });
        // Within the caps (condition 6), refused for now when full: the
        // former home keeps it and tries again.
        if from_friend || passed_on {
            self.room(&account, bytes.len())?;
        }

        // A friend's home's commit only says there's one to fetch.
        if kind == "commit" && from_friend {
            if group.epoch.is_none_or(|ours| epoch >= ours) {
                self.tx.execute(
                    "UPDATE groups SET hinted = MAX(COALESCE(hinted, -1), ?2) WHERE group_id = ?1",
                    params![group.id, epoch as i64],
                )?;
            }
            return Ok(Taken::Held);
        }

        let Some(ours) = group.epoch else {
            // No state yet: kept until the home has the group's GroupInfo.
            self.hold(&group.id, kind, epoch, bytes, &digest, &account, mine, leaf_ref)?;
            if leaf.is_some() {
                self.queue(&group.id, bytes, &digest, leaf_ref)?;
            }
            if passed_on {
                self.passed_in(&group.id, &digest)?;
            }
            return Ok(Taken::Held);
        };

        if kind == "commit" {
            if epoch < ours {
                // Not the server's for that epoch (it would be here).
                return Err(invalid("That commit is for an epoch this group is past."));
            }
            if epoch > ours {
                self.hold(&group.id, kind, epoch, bytes, &digest, &account, mine, leaf_ref)?;
                self.tx.execute(
                    "UPDATE groups SET hinted = MAX(COALESCE(hinted, -1), ?2) WHERE group_id = ?1",
                    params![group.id, epoch as i64],
                )?;
                return Ok(Taken::Held);
            }
            let seq = self.commit(&mut group, parsed, bytes, &digest, &account, leaf_ref)?;
            if leaf.is_some() {
                self.queue(&group.id, bytes, &digest, leaf_ref)?;
            }
            // Any held commits for the epochs after, in order.
            self.held_commits(&mut group)?;
            return Ok(Taken::Stored { seq, accepted_ms: self.now, duplicate: false });
        }

        // An application message.
        if epoch + EPOCHS_BEHIND < ours {
            return Ok(Taken::Bounce { epoch: ours });
        }
        // Passed on: the former home took it under the rules below, for the
        // person's own leaves only (no queue to anyone, condition 3).
        if passed_on {
            self.passed_in(&group.id, &digest)?;
            if epoch > ours {
                self.hold(&group.id, kind, epoch, bytes, &digest, &account, false, None)?;
                self.tx.execute(
                    "UPDATE groups SET hinted = MAX(COALESCE(hinted, -1), ?2) WHERE group_id = ?1",
                    params![group.id, ours as i64],
                )?;
                return Ok(Taken::Held);
            }
            let seq = self.append(&mut group, kind, epoch, bytes, &digest, &account, false, None)?;
            return Ok(Taken::Stored { seq, accepted_ms: self.now, duplicate: false });
        }
        // A Claktory's leaf never sends in a conversation (only the person's
        // device does, after they approved it). Who sent a friend's message
        // is encrypted, so the devices drop that; this Claktory's own leaf,
        // and another of the person's Claktories bound here, are known.
        let from_claktory = matches!(intake.source, Source::Local { .. }) || (from_friend && leaf.is_some()) || handed_by_claktory;
        if group.kind == "conversation" && from_claktory {
            return Err(invalid("A Claktory doesn't send messages in a conversation."));
        }
        if group.kind == "conversation" && group.both_since.is_none_or(|since| (epoch as i64) < since) {
            return Err(Refusal::Waiting);
        }
        if leaf.is_some() {
            self.queue(&group.id, bytes, &digest, leaf_ref)?;
        }
        if epoch > ours {
            self.hold(&group.id, kind, epoch, bytes, &digest, &account, mine, leaf_ref)?;
            self.tx.execute(
                "UPDATE groups SET hinted = MAX(COALESCE(hinted, -1), ?2) WHERE group_id = ?1",
                params![group.id, ours as i64],
            )?;
            return Ok(Taken::Held);
        }
        let seq = self.append(&mut group, kind, epoch, bytes, &digest, &account, mine, leaf_ref)?;
        Ok(Taken::Stored { seq, accepted_ms: self.now, duplicate: false })
    }

    /// Commits held for the epochs after the group's, taken in order while
    /// the next one is there (from the person's device or the server).
    fn held_commits(&mut self, group: &mut Group) -> Result<(), Refusal> {
        loop {
            let Some(epoch) = group.epoch else { return Ok(()) };
            type Held = (i64, Vec<u8>, String, Option<String>, Option<String>);
            let next: Option<Held> = self
                .tx
                .query_row(
                    "SELECT id, envelope, account, leaf_id, client_message_id FROM held WHERE group_id = ?1 AND kind = 'commit' AND epoch = ?2
                     ORDER BY id LIMIT 1",
                    params![group.id, epoch as i64],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?)),
                )
                .optional()?;
            let Some((id, bytes, account, leaf_id, client)) = next else { return Ok(()) };
            self.tx.execute("DELETE FROM held WHERE id = ?1", [id])?;
            let Ok(envelope) = Envelope::new(bytes) else { continue };
            let digest = envelope.digest();
            let leaf = leaf_id.as_deref().map(|leaf| (leaf, client.as_deref().unwrap_or("")));
            self.commit(group, envelope.parsed(), envelope.bytes(), &digest, &account, leaf)?;
        }
    }

    /// Gives a group its state from a GroupInfo with its ratchet tree: a
    /// group it had none of, or one whose state fell out of step, or that
    /// is behind it. The log goes on from there; held messages that now fit
    /// go in.
    fn seed(&mut self, group_info: &[u8]) -> Result<(), Refusal> {
        let envelope = Envelope::new(group_info.to_vec()).map_err(|error| Refusal::Invalid(error.to_string()))?;
        let parsed = envelope.parsed();
        let MlsMessageBodyIn::GroupInfo(info) = parsed.message.extract() else {
            return Err(Refusal::Invalid("That isn't a GroupInfo.".to_owned()));
        };
        let id = info.group_id().as_slice().to_vec();
        if !(16..=32).contains(&id.len()) {
            return Err(Refusal::Invalid("A group's id is 16 to 32 bytes.".to_owned()));
        }
        let epoch = info.epoch().as_u64();
        let mut group = self.group_or_new(&id)?;
        let ok: bool = self.tx.query_row("SELECT public_ok FROM groups WHERE group_id = ?1", [&id], |row| row.get(0))?;
        if group.epoch.is_some_and(|ours| ours > epoch || (ours == epoch && ok)) {
            return Ok(());
        }
        let tree = info
            .extensions()
            .ratchet_tree()
            .ok_or_else(|| Refusal::Invalid("A GroupInfo here carries its ratchet tree.".to_owned()))?
            .ratchet_tree()
            .clone();
        let provider = self.provider()?;
        mls_storage::forget_group(self.tx, &GroupId::from_slice(&id)).map_err(|error| Refusal::Unready(error.to_string()))?;
        let (public, _) = PublicGroup::from_external(provider.crypto(), provider.storage(), tree, info, ProposalStore::default())
            .map_err(|error| Refusal::Invalid(format!("That GroupInfo doesn't check: {error}.")))?;
        self.tx.execute("UPDATE groups SET epoch = ?2 WHERE group_id = ?1", params![id, epoch as i64])?;
        group.epoch = Some(epoch);
        let seq = group.last_seq;
        self.members_changed(&group, &public, seq)?;
        let mut group = self.group(&id)?.expect("there");
        self.held_commits(&mut group)?;
        self.release(&mut group)?;
        // Commits from before the GroupInfo's epoch never fit now.
        self.tx.execute("DELETE FROM held WHERE group_id = ?1 AND kind = 'commit' AND epoch < ?2", params![id, epoch as i64])?;
        Ok(())
    }
}

fn free_bytes(home: &Home) -> Option<u64> {
    #[cfg(test)]
    if let Some(free) = TEST_FREE.with(|free| free.get()) {
        return Some(free);
    }
    crate::uploads::available(home.root())
}

#[cfg(test)]
thread_local! {
    static TEST_FREE: std::cell::Cell<Option<u64>> = const { std::cell::Cell::new(None) };
    static TEST_CAPS: std::cell::Cell<Option<(i64, i64)>> = const { std::cell::Cell::new(None) };
}

/// What one friend, and everyone, may have held here.
fn caps() -> (i64, i64) {
    #[cfg(test)]
    if let Some(caps) = TEST_CAPS.with(|caps| caps.get()) {
        return caps;
    }
    (FRIEND_BYTES, TOTAL_BYTES)
}

/// Why a message passed on for a group this home has no state of yet waits
/// (`Source::PassOn`).
const STATELESS: &str = "this home doesn't have the group's state yet";

/// Takes a message for the log (H2, H3, H5, the server pull).
pub(crate) fn take(factory: &impl Host, intake: Intake) -> Result<Taken, Refusal> {
    let group_id = intake.group_id.clone();
    let taken = factory.run(|step| step.take(intake));
    // Passed on for a group nothing else brought here since the move: the
    // home asks claktory.com for it, and the former home tries again.
    if matches!(&taken, Err(Refusal::Unready(why)) if why == STATELESS) {
        factory.run(|step| Ok(step.group_or_new(&group_id).map(|_| ())?))?;
        factory.wake_pull();
    }
    if taken == Ok(Taken::Held) {
        factory.wake_pull();
    }
    taken
}

/// A group's state from a GroupInfo with its ratchet tree (claktory.com's,
/// pulled for `wanted`).
pub(crate) fn seed(factory: &impl Host, group_info: &[u8]) -> Result<(), Refusal> {
    factory.run(|step| step.seed(group_info))
}

/// What the home needs from claktory.com now.
pub(crate) fn wanted(factory: &impl Host) -> Result<Vec<Want>, Refusal> {
    factory.run(|step| {
        type Row = (Vec<u8>, Option<i64>, bool, Option<i64>, bool);
        let rows: Vec<Row> = {
            let mut statement = step.tx.prepare(
                "SELECT g.group_id, g.epoch, g.public_ok, g.hinted, EXISTS (SELECT 1 FROM held h WHERE h.group_id = g.group_id) FROM groups g",
            )?;
            statement
                .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?)))?
                .collect::<Result<_, _>>()?
        };
        let mut wants = Vec::new();
        for (group_id, epoch, public_ok, hinted, held) in rows {
            match epoch {
                None => wants.push(Want { group_id, group_info: true, after_epoch: None }),
                Some(epoch) => {
                    let behind = hinted.is_some_and(|hinted| hinted >= epoch) || held;
                    if !public_ok || behind {
                        wants.push(Want { group_id, group_info: !public_ok, after_epoch: behind.then_some(epoch as u64) });
                    }
                }
            }
        }
        Ok(wants)
    })
}

/// The server had nothing after `epoch` for a group a hint named: the hint
/// is spent.
pub(crate) fn caught_up(factory: &impl Host, group_id: &[u8]) -> Result<(), Refusal> {
    factory.run(|step| {
        step.tx.execute("UPDATE groups SET hinted = NULL WHERE group_id = ?1", [group_id])?;
        Ok(())
    })
}

/// Replaces the routing table (H3's DmRoutes, from the person's device
/// only: friends' homes). Groups newly routed to an endpoint get nothing old
/// queued: the table says where new messages go. A friend whose home moved
/// gets what waited for the old one at the new one (`repoint`). The person's
/// other Claktories bound here (`bind`) and the route to the home (`follow`)
/// stay.
pub(crate) fn set_routes(factory: &impl Host, routes: Vec<Route>) -> Result<(), Refusal> {
    factory.run(|step| {
        let before = onward_now(step)?;
        step.tx.execute("DELETE FROM routes", [])?;
        for route in &routes {
            let endpoint = route.endpoint.to_ascii_lowercase();
            for group in &route.group_ids {
                step.tx.execute(
                    "INSERT INTO routes (endpoint, account, group_id) VALUES (?1, ?2, ?3) ON CONFLICT (endpoint, group_id) DO UPDATE SET account = ?2",
                    params![endpoint, route.account, group],
                )?;
                // A conversation's friend, once known, stays its friend.
                if route.account != step.account {
                    step.tx.execute("UPDATE groups SET friend = COALESCE(friend, ?2) WHERE group_id = ?1", params![group, route.account])?;
                }
            }
        }
        repoint(step, &before)?;
        rehome(step, true)?;
        Ok(())
    })
}

/// Why a friend's new home wasn't taken (`friend_moved`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum NotMoved {
    /// The person's own account: their devices route it.
    Own,
    /// No group is routed to that account here.
    NotRouted,
    /// Older than a list taken for that account before.
    Older,
}

/// The highest version taken for `account` from a list its new home showed
/// (`friend_moved`): the floor for the next.
pub(crate) fn friend_floor(factory: &impl Host, account: &str) -> Result<u64, Refusal> {
    factory.run(|step| {
        let floor: Option<i64> =
            step.tx.query_row("SELECT version FROM friend_homes WHERE account = ?1", [account], |row| row.get(0)).optional()?;
        Ok(floor.unwrap_or(0) as u64)
    })
}

/// A friend's new home showed `account`'s signed device list, verified
/// under the person's pins at `version`, naming `endpoint` its home (row
/// 595, Fugo 41473): the groups routed here to that account (only those,
/// never another account or a new group) now go to `endpoint`, and what
/// waited for its old home goes there too, as when the person's device
/// gives new routes. Persists `version` as the floor for that account.
/// The endpoints the routes moved off.
pub(crate) fn friend_moved(
    factory: &impl Host,
    account: &str,
    endpoint: &str,
    version: u64,
) -> Result<Result<Vec<String>, NotMoved>, Refusal> {
    let endpoint = endpoint.to_ascii_lowercase();
    factory.run(|step| {
        if account == step.account {
            return Ok(Err(NotMoved::Own));
        }
        let floor: Option<i64> =
            step.tx.query_row("SELECT version FROM friend_homes WHERE account = ?1", [account], |row| row.get(0)).optional()?;
        if floor.is_some_and(|floor| (version as i64) < floor) {
            return Ok(Err(NotMoved::Older));
        }
        let old: Vec<String> = {
            let mut statement = step.tx.prepare("SELECT DISTINCT endpoint FROM routes WHERE account = ?1 AND endpoint != ?2")?;
            statement.query_map(params![account, endpoint], |row| row.get(0))?.collect::<Result<_, _>>()?
        };
        let routed: bool = step.tx.query_row("SELECT COUNT(*) > 0 FROM routes WHERE account = ?1", [account], |row| row.get(0))?;
        if !routed {
            return Ok(Err(NotMoved::NotRouted));
        }
        let before = onward_now(step)?;
        step.tx.execute(
            "INSERT OR IGNORE INTO routes (endpoint, account, group_id) SELECT ?2, account, group_id FROM routes WHERE account = ?1",
            params![account, endpoint],
        )?;
        step.tx.execute("DELETE FROM routes WHERE account = ?1 AND endpoint != ?2", params![account, endpoint])?;
        step.tx.execute(
            "INSERT INTO friend_homes (account, version, endpoint, at) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT (account) DO UPDATE SET version = ?2, endpoint = ?3, at = ?4",
            params![account, version as i64, endpoint, step.now],
        )?;
        repoint(step, &before)?;
        Ok(Ok(old))
    })
}

/// Where the person's messages go now (`onward`): (endpoint, account,
/// group), before the routes change.
fn onward_now(step: &Step<'_>) -> rusqlite::Result<Vec<(String, String, Vec<u8>)>> {
    let mut statement = step.tx.prepare("SELECT endpoint, account, group_id FROM onward")?;
    let rows = statement.query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?.collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

/// After the routes changed: what waited for an endpoint that no longer
/// reaches its group goes to the one that does now for the same account (a
/// friend's home moved, or the person's), so a move loses nothing queued
/// (dms-no-server-storage.md §3). What has nowhere to go is let go.
fn repoint(step: &Step<'_>, before: &[(String, String, Vec<u8>)]) -> rusqlite::Result<()> {
    for (old, account, group) in before {
        step.tx.execute(
            "UPDATE OR IGNORE outbox
             SET endpoint = (SELECT o.endpoint FROM onward o WHERE o.account = ?2 AND o.group_id = ?3 ORDER BY o.endpoint LIMIT 1)
             WHERE endpoint = ?1 AND group_id = ?3
               AND NOT EXISTS (SELECT 1 FROM reach r WHERE r.endpoint = ?1 AND r.group_id = ?3)
               AND EXISTS (SELECT 1 FROM onward o WHERE o.account = ?2 AND o.group_id = ?3)",
            params![old, account, group],
        )?;
    }
    step.tx.execute(
        "DELETE FROM outbox WHERE NOT EXISTS (SELECT 1 FROM reach r WHERE r.endpoint = outbox.endpoint AND r.group_id = outbox.group_id)",
        [],
    )?;
    Ok(())
}

/// The route at an endpoint: H2's gate, before any frame.
pub(crate) fn route_of(factory: &impl Host, endpoint: &str) -> Result<Option<Route>, Refusal> {
    factory.run(|step| {
        let rows: Vec<(String, Vec<u8>)> = {
            let mut statement = step.tx.prepare("SELECT account, group_id FROM reach WHERE endpoint = ?1 ORDER BY group_id")?;
            statement.query_map([endpoint.to_ascii_lowercase()], |row| Ok((row.get(0)?, row.get(1)?)))?.collect::<Result<_, _>>()?
        };
        let Some((account, _)) = rows.first().cloned() else { return Ok(None) };
        Ok(Some(Route { account, endpoint: endpoint.to_ascii_lowercase(), group_ids: rows.into_iter().map(|(_, group)| group).collect() }))
    })
}

/// Binds another of the person's Claktories' leaf to the endpoint its
/// signed hello came from at `at_ms` (H2 checked the hello against the
/// person's current signed device list). An older hello than the leaf's
/// last never binds it again (a replay after it moved): false, nothing
/// changed. Moved to a new endpoint, what waited for the old one goes to the
/// new one; the first time, what its groups hold past where it kept them.
pub(crate) fn bind(factory: &impl Host, leaf_id: &str, endpoint: &str, at_ms: i64) -> Result<bool, Refusal> {
    let endpoint = endpoint.to_ascii_lowercase();
    factory.run(|step| {
        let before: Option<(String, i64)> = step
            .tx
            .query_row("SELECT endpoint, at_ms FROM bound WHERE leaf_id = ?1", [leaf_id], |row| Ok((row.get(0)?, row.get(1)?)))
            .optional()?;
        if before.as_ref().is_some_and(|(_, at)| at_ms <= *at) {
            return Ok(false);
        }
        // One leaf an endpoint: a newer leaf there (the Claktory enrolled
        // again) takes its place.
        step.tx.execute("DELETE FROM bound WHERE endpoint = ?1 AND leaf_id != ?2", params![endpoint, leaf_id])?;
        step.tx.execute(
            "INSERT INTO bound (leaf_id, endpoint, at_ms) VALUES (?1, ?2, ?3)
             ON CONFLICT (leaf_id) DO UPDATE SET endpoint = ?2, at_ms = ?3",
            params![leaf_id, endpoint, at_ms],
        )?;
        match before {
            Some((old, _)) if old != endpoint => {
                step.tx.execute("UPDATE OR IGNORE outbox SET endpoint = ?2 WHERE endpoint = ?1", params![old, endpoint])?;
            }
            Some(_) => {}
            None => backfill(step, leaf_id)?,
        }
        step.tx.execute(
            "DELETE FROM outbox WHERE NOT EXISTS (SELECT 1 FROM reach r WHERE r.endpoint = outbox.endpoint AND r.group_id = outbox.group_id)",
            [],
        )?;
        Ok(true)
    })
}

/// Queues for a newly bound leaf what its groups' logs hold past where it
/// kept them, in order.
fn backfill(step: &Step<'_>, leaf_id: &str) -> rusqlite::Result<()> {
    type Row = (Vec<u8>, Vec<u8>, Vec<u8>, i64);
    let rows: Vec<Row> = {
        let mut statement = step.tx.prepare(
            "SELECT l.group_id, l.envelope, l.digest, l.accepted FROM members m
             JOIN log l ON l.group_id = m.group_id
             LEFT JOIN cursors c ON c.leaf_id = m.leaf_id AND c.group_id = m.group_id
             WHERE m.leaf_id = ?1 AND l.seq > COALESCE(c.acked, 0) AND l.leaf_id IS NOT ?1
             ORDER BY l.group_id, l.seq",
        )?;
        statement.query_map([leaf_id], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)))?.collect::<Result<_, _>>()?
    };
    for (group, envelope, digest, accepted) in rows {
        step.forward(&group, &envelope, &digest, None, accepted)?;
    }
    Ok(())
}

/// The leaf bound at an endpoint, if any: H2's gate lets it in without a
/// hello.
pub(crate) fn bound_leaf(factory: &impl Host, endpoint: &str) -> Result<Option<String>, Refusal> {
    factory.run(|step| Ok(step.tx.query_row("SELECT leaf_id FROM bound WHERE endpoint = ?1", [endpoint.to_ascii_lowercase()], |row| row.get(0)).optional()?))
}

/// Lets go of the bindings of leaves no longer on the person's signed
/// device list (`listed`: the Claktory leaves it names), and what waited
/// for them.
pub(crate) fn unbind_missing(factory: &impl Host, listed: &[String]) -> Result<(), Refusal> {
    factory.run(|step| {
        let bound: Vec<String> = {
            let mut statement = step.tx.prepare("SELECT leaf_id FROM bound")?;
            statement.query_map([], |row| row.get(0))?.collect::<Result<_, _>>()?
        };
        for leaf in bound.iter().filter(|leaf| !listed.contains(leaf)) {
            step.tx.execute("DELETE FROM bound WHERE leaf_id = ?1", [leaf])?;
        }
        step.tx.execute(
            "DELETE FROM outbox WHERE NOT EXISTS (SELECT 1 FROM reach r WHERE r.endpoint = outbox.endpoint AND r.group_id = outbox.group_id)",
            [],
        )?;
        Ok(())
    })
}

/// Where this Claktory stands, by the person's current signed device list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Placed {
    /// The list names this Claktory as their home.
    Home,
    /// It names another (its endpoint, lowercase hex), or none.
    Away(Option<String>),
}

/// The list watcher: this Claktory's place, by the person's current signed
/// device list (`follow`). Whether its place or its routes changed (H2
/// reloads its links then).
pub(crate) fn follow_list(factory: &Factory, placed: &Placed, groups: &[Vec<u8>]) -> Result<bool, Refusal> {
    use std::sync::atomic::Ordering;
    let was_home = factory.dm_home.is_home.swap(*placed == Placed::Home, Ordering::SeqCst);
    let was_away = factory.dm_home.away.swap(matches!(placed, Placed::Away(Some(_))), Ordering::SeqCst);
    let moved = was_home != (*placed == Placed::Home) || was_away != matches!(placed, Placed::Away(Some(_)));
    let followed = follow(factory, placed, groups)?;
    if !was_home && *placed == Placed::Home {
        factory.wake_pull();
    }
    Ok(followed || moved)
}

/// Where the list watcher last placed this Claktory, until it reads the
/// list again (at start, maybe with the vault locked).
fn restore(factory: &Factory) {
    let said = with(factory, |step| Ok(meta(step.tx, "placed")?));
    let Ok(Some(said)) = said else { return };
    let said = String::from_utf8_lossy(&said);
    set_is_home(factory, said == "home");
    factory.dm_home.away.store(said.strip_prefix("away:").is_some_and(|home| !home.is_empty()), std::sync::atomic::Ordering::SeqCst);
}

/// Where `list` puts this Claktory, at endpoint `ours`.
pub(crate) fn placed(list: &claktory_dm_crypto::device_list::SignedDeviceList, ours: &[u8; 32]) -> Placed {
    match list.home {
        Some(home) if home == *ours => Placed::Home,
        home => Placed::Away(home.map(|home| hex(&home))),
    }
}

/// Follows the person's current signed device list, as this Claktory's leaf
/// verified it (their pinned identity's, never older than one seen before):
/// the home has no route of its own (the device gives its routes); another
/// Claktory has one, to the home, for `groups` (its leaf's). The home that
/// stops being it hands on what it had queued (`hand_over`). Whether its
/// place or its routes changed.
pub(crate) fn follow(factory: &impl Host, placed: &Placed, groups: &[Vec<u8>]) -> Result<bool, Refusal> {
    let wanted: Vec<(String, Vec<u8>)> = match placed {
        Placed::Away(Some(home)) => groups.iter().map(|group| (home.clone(), group.clone())).collect(),
        _ => Vec::new(),
    };
    let said = match placed {
        Placed::Home => "home".to_owned(),
        Placed::Away(home) => format!("away:{}", home.as_deref().unwrap_or("")),
    };
    factory.run(|step| {
        let was = meta(step.tx, "placed")?;
        let was_home = was.as_deref() == Some(b"home".as_slice());
        // Its place alone changes where what it hands on goes.
        let moved = was.as_deref() != Some(said.as_bytes());
        // Kept, so a restart with the vault locked (the leaf can't read the
        // list then) still knows.
        step.tx.execute("INSERT OR REPLACE INTO meta (name, value) VALUES ('placed', ?1)", [said.as_bytes()])?;
        if was_home && *placed != Placed::Home {
            hand_over(step)?;
        }
        // The home now: a device may have committed while another was it,
        // or with no home to send to (row 570). Each group it knows is
        // checked at claktory.com for commits it never carried, as a hint
        // would have it; one with none has the mark spent (`caught_up`).
        if !was_home && *placed == Placed::Home {
            step.tx.execute("UPDATE groups SET hinted = MAX(COALESCE(hinted, -1), epoch) WHERE epoch IS NOT NULL", [])?;
            // The person's devices may have read another home past the
            // seqs this one gives (row 574): from now on its seqs start
            // past every seq a home gave before, each group's and any it
            // learns of later.
            let kept = meta(step.tx, "seq_base")?.and_then(|value| String::from_utf8_lossy(&value).parse::<i64>().ok());
            let base = seq_base(step.now).max(kept.unwrap_or(0));
            step.tx.execute("INSERT OR REPLACE INTO meta (name, value) VALUES ('seq_base', ?1)", [base.to_string().as_bytes()])?;
            step.tx.execute("UPDATE groups SET last_seq = MAX(last_seq, ?1)", [base - 1])?;
            // When it became the home (DmHome.home_since_ms): what the
            // person's devices read before it came from another home.
            step.tx.execute("INSERT OR REPLACE INTO meta (name, value) VALUES ('home_since', ?1)", [step.now.to_string().as_bytes()])?;
        }
        rehome(step, false)?;
        let mut kept: Vec<(String, Vec<u8>)> = {
            let mut statement = step.tx.prepare("SELECT endpoint, group_id FROM toward")?;
            statement.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?.collect::<Result<_, _>>()?
        };
        let mut sorted = wanted.clone();
        sorted.sort();
        kept.sort();
        if kept == sorted {
            return Ok(moved);
        }
        let before = onward_now(step)?;
        step.tx.execute("DELETE FROM toward", [])?;
        for (endpoint, group) in &wanted {
            step.tx.execute("INSERT OR IGNORE INTO toward (endpoint, group_id) VALUES (?1, ?2)", params![endpoint, group])?;
        }
        // The home moved: what waited for the old one goes to the new one.
        repoint(step, &before)?;
        Ok(true)
    })
}

/// This Claktory stopped being the home: what it had queued for friends'
/// homes goes to the person's new home instead, for `HANDOVER_MS`
/// (dms-no-server-storage.md §3, Fugo 33779), and their routes go, so
/// nothing new is queued toward them. A second move while away keeps the
/// rows and their time.
fn hand_over(step: &Step<'_>) -> rusqlite::Result<()> {
    step.tx.execute(
        "INSERT OR IGNORE INTO handover (message_id, account, group_id, envelope, leaf_id, client_message_id, queued, until)
         SELECT o.message_id, r.account, o.group_id, o.envelope, o.leaf_id, COALESCE(o.client_message_id, ''), o.queued, ?1
         FROM outbox o JOIN routes r ON r.endpoint = o.endpoint AND r.group_id = o.group_id
         WHERE o.leaf_id IS NOT NULL",
        [step.now + HANDOVER_MS],
    )?;
    step.tx.execute("DELETE FROM outbox WHERE endpoint IN (SELECT endpoint FROM routes)", [])?;
    step.tx.execute("DELETE FROM routes", [])?;
    // And what it took that one of the person's leaves in the group hasn't
    // read yet (a leaf that never followed it: all of it), so the new home
    // serves it (Zucchero's rows 567 and 591, Fugo 39876 and 41196):
    // application messages only, friends' and the person's own, in the log
    // or held, never what a home before passed on here, nor what's still
    // queued for friends' homes (handed over above).
    let until = step.now + HANDOVER_MS;
    step.tx.execute(
        "INSERT OR IGNORE INTO pass_on (message_id, group_id, envelope, account, queued, until)
         SELECT lower(hex(l.digest)), l.group_id, l.envelope, l.account, l.accepted, ?1 FROM log l
         WHERE l.kind = 'application'
         AND NOT EXISTS (SELECT 1 FROM passed_in p WHERE p.group_id = l.group_id AND p.digest = l.digest)
         AND NOT EXISTS (SELECT 1 FROM handover h WHERE h.message_id = lower(hex(l.digest)))
         AND EXISTS (SELECT 1 FROM members m LEFT JOIN cursors c ON c.group_id = m.group_id AND c.leaf_id = m.leaf_id
                     WHERE m.group_id = l.group_id AND COALESCE(c.acked, 0) < l.seq)",
        [until],
    )?;
    step.tx.execute(
        "INSERT OR IGNORE INTO pass_on (message_id, group_id, envelope, account, queued, until)
         SELECT lower(hex(h.digest)), h.group_id, h.envelope, h.account, h.received, ?1 FROM held h
         WHERE h.kind = 'application'
         AND NOT EXISTS (SELECT 1 FROM passed_in p WHERE p.group_id = h.group_id AND p.digest = h.digest)
         AND NOT EXISTS (SELECT 1 FROM handover o WHERE o.message_id = lower(hex(h.digest)))",
        [until],
    )?;
    Ok(())
}

/// The home again before its handed rows went (or their week was up): they
/// go to what `onward` routes now for the same friend's account and group
/// (`repoint`'s rule, Fugo 33818). `settle`: the device said its routes, so
/// the rest goes; until then, it waits for them.
fn rehome(step: &Step<'_>, settle: bool) -> rusqlite::Result<()> {
    if meta(step.tx, "placed")?.as_deref() != Some(b"home".as_slice()) {
        return Ok(());
    }
    // The home again: it serves what it would have passed on itself.
    step.tx.execute("DELETE FROM pass_on", [])?;
    step.tx.execute(
        "INSERT OR IGNORE INTO outbox (endpoint, message_id, group_id, envelope, leaf_id, client_message_id, queued)
         SELECT o.endpoint, h.message_id, h.group_id, h.envelope, h.leaf_id, NULLIF(h.client_message_id, ''), h.queued
         FROM handover h JOIN onward o ON o.account = h.account AND o.group_id = h.group_id
         WHERE h.until >= ?1",
        [step.now],
    )?;
    if settle {
        step.tx.execute("DELETE FROM handover", [])?;
    } else {
        step.tx.execute(
            "DELETE FROM handover WHERE EXISTS (SELECT 1 FROM onward o WHERE o.account = handover.account AND o.group_id = handover.group_id)",
            [],
        )?;
    }
    Ok(())
}

/// The person's home, while this Claktory is away and has something to
/// hand on to it (`hand_over`).
fn handing_to(step: &Step<'_>) -> rusqlite::Result<Option<String>> {
    let Some(placed) = meta(step.tx, "placed")? else { return Ok(None) };
    let home = match String::from_utf8_lossy(&placed).strip_prefix("away:") {
        Some(home) if !home.is_empty() => home.to_owned(),
        _ => return Ok(None),
    };
    let any: bool = step.tx.query_row(
        "SELECT EXISTS (SELECT 1 FROM handover WHERE until >= ?1) OR EXISTS (SELECT 1 FROM pass_on WHERE until >= ?1)",
        [step.now],
        |row| row.get(0),
    )?;
    Ok(any.then_some(home))
}

/// A friend's home (or the person's) says `group` has a commit at `epoch`:
/// the pull looks (Fugo's condition 4: never the commit itself).
pub(crate) fn hint(factory: &impl Host, group_id: &[u8], epoch: u64) -> Result<(), Refusal> {
    factory.run(|step| {
        step.tx.execute(
            "UPDATE groups SET hinted = MAX(COALESCE(hinted, -1), ?2) WHERE group_id = ?1 AND (epoch IS NULL OR epoch <= ?2)",
            params![group_id, epoch as i64],
        )?;
        Ok(())
    })?;
    factory.wake_pull();
    Ok(())
}

/// How many messages wait for each endpoint (DmPeer.outbox_count).
pub(crate) fn outbox_counts(factory: &impl Host) -> Result<BTreeMap<String, i64>, Refusal> {
    factory.run(|step| {
        let mut statement = step.tx.prepare("SELECT endpoint, COUNT(*) FROM outbox GROUP BY endpoint")?;
        let rows = statement.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?.collect::<Result<_, _>>()?;
        Ok(rows)
    })
}

/// The whole routing table, one Route an endpoint (H2's gate and peers).
/// Away from the home, only the route to the home: friends' homes admit
/// only the person's home, and the person's other Claktories are bound
/// there, so a Claktory that was the home never knocks on them again
/// (Zucchero, row 612). The table stays, for when it's the home again.
pub(crate) fn routes(factory: &impl Host) -> Result<Vec<Route>, Refusal> {
    factory.run(|step| {
        let mut rows: Vec<(String, String, Vec<u8>)> = {
            let mut statement = step.tx.prepare("SELECT endpoint, account, group_id FROM reach ORDER BY endpoint, group_id")?;
            statement.query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?.collect::<Result<_, _>>()?
        };
        if let Some(placed) = meta(step.tx, "placed")?
            && let Some(home) = String::from_utf8_lossy(&placed).strip_prefix("away:")
        {
            rows.retain(|(endpoint, _, _)| !home.is_empty() && endpoint.eq_ignore_ascii_case(home));
        }
        // And the person's home, for what this Claktory hands on to it.
        if let Some(home) = handing_to(step)? {
            let groups: Vec<Vec<u8>> = {
                let mut statement = step
                    .tx
                    .prepare("SELECT group_id FROM handover WHERE until >= ?1 UNION SELECT group_id FROM pass_on WHERE until >= ?1")?;
                statement.query_map([step.now], |row| row.get(0))?.collect::<Result<_, _>>()?
            };
            for group in groups {
                if !rows.iter().any(|(endpoint, _, kept)| *endpoint == home && *kept == group) {
                    rows.push((home.clone(), step.account.to_owned(), group));
                }
            }
            rows.sort();
        }
        let mut routes: Vec<Route> = Vec::new();
        for (endpoint, account, group) in rows {
            match routes.last_mut() {
                Some(route) if route.endpoint == endpoint => route.group_ids.push(group),
                _ => routes.push(Route { account, endpoint, group_ids: vec![group] }),
            }
        }
        Ok(routes)
    })
}

/// What waits for an endpoint, oldest first.
pub(crate) fn outbox(factory: &impl Host, endpoint: &str, limit: usize) -> Result<Vec<Outgoing>, Refusal> {
    factory.run(|step| {
        let mut statement =
            // In each group's order: entries queued at once (a backfill, held
            // messages let go) by their seq.
            step.tx.prepare(
                "SELECT o.message_id, o.group_id, o.envelope FROM outbox o WHERE o.endpoint = ?1
                 ORDER BY o.queued, COALESCE((SELECT l.seq FROM log l WHERE l.group_id = o.group_id AND l.digest = unhex(o.message_id)), 0), o.message_id
                 LIMIT ?2",
            )?;
        let mut rows: Vec<Outgoing> = statement
            .query_map(params![endpoint.to_ascii_lowercase(), limit as i64], |row| {
                Ok(Outgoing { message_id: row.get(0)?, group_id: row.get(1)?, envelope: row.get(2)?, handover: None, pass_on: None })
            })?
            .collect::<Result<_, _>>()?;
        // The person's home: what this Claktory hands on to it, after.
        if rows.len() < limit && handing_to(step)?.is_some_and(|home| home == endpoint.to_ascii_lowercase()) {
            let mut statement = step.tx.prepare(
                "SELECT message_id, group_id, envelope, leaf_id, client_message_id FROM handover WHERE until >= ?1
                 GROUP BY message_id ORDER BY MIN(queued), message_id LIMIT ?2",
            )?;
            let handed = statement
                .query_map(params![step.now, (limit - rows.len()) as i64], |row| {
                    Ok(Outgoing {
                        message_id: row.get(0)?,
                        group_id: row.get(1)?,
                        envelope: row.get(2)?,
                        handover: Some((row.get(3)?, row.get(4)?)),
                        pass_on: None,
                    })
                })?
                .collect::<Result<Vec<_>, _>>()?;
            rows.extend(handed);
            // Then what it passes on, in the order it took it.
            if rows.len() < limit {
                let mut statement = step.tx.prepare(
                    "SELECT message_id, group_id, envelope, account FROM pass_on WHERE until >= ?1 ORDER BY queued, message_id LIMIT ?2",
                )?;
                let passed = statement
                    .query_map(params![step.now, (limit - rows.len()) as i64], |row| {
                        Ok(Outgoing {
                            message_id: row.get(0)?,
                            group_id: row.get(1)?,
                            envelope: row.get(2)?,
                            handover: None,
                            pass_on: Some(row.get(3)?),
                        })
                    })?
                    .collect::<Result<Vec<_>, _>>()?;
                rows.extend(passed);
            }
        }
        Ok(rows)
    })
}

/// An endpoint took these: out of its queue, and the person's devices are
/// told their messages were delivered. Another Claktory of the person's
/// bound there kept them: its leaf's place moves up to the first entry it
/// still waits for.
pub(crate) fn sent(factory: &impl Host, endpoint: &str, message_ids: &[String]) -> Result<(), Refusal> {
    let endpoint = endpoint.to_ascii_lowercase();
    factory.run(|step| {
        // What was handed on to the person's home: done with.
        if handing_to(step)?.is_some_and(|home| home == endpoint) {
            for message_id in message_ids {
                step.tx.execute("DELETE FROM handover WHERE message_id = ?1", [message_id])?;
                step.tx.execute("DELETE FROM pass_on WHERE message_id = ?1", [message_id])?;
            }
        }
        let mut touched: BTreeSet<Vec<u8>> = BTreeSet::new();
        for message_id in message_ids {
            let row: Option<(Vec<u8>, Option<String>, Option<String>)> = step
                .tx
                .query_row(
                    "SELECT group_id, leaf_id, client_message_id FROM outbox WHERE endpoint = ?1 AND message_id = ?2",
                    params![endpoint, message_id],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                )
                .optional()?;
            let Some((group_id, leaf_id, client)) = row else { continue };
            step.tx.execute("DELETE FROM outbox WHERE endpoint = ?1 AND message_id = ?2", params![endpoint, message_id])?;
            step.tx.execute(
                "INSERT INTO sent (endpoint, message_id, group_id, leaf_id, client_message_id, at) VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                 ON CONFLICT (endpoint, message_id) DO UPDATE SET at = ?6",
                params![endpoint, message_id, group_id, leaf_id, client, step.now],
            )?;
            if let (Some(leaf_id), Some(client_message_id)) = (leaf_id, client) {
                step.arrivals.push(Arrival::Delivered { leaf_id, client_message_id, group_id: group_id.clone(), endpoint: endpoint.clone() });
            }
            touched.insert(group_id);
        }
        let bound: Option<String> =
            step.tx.query_row("SELECT leaf_id FROM bound WHERE endpoint = ?1", [&endpoint], |row| row.get(0)).optional()?;
        if let Some(leaf) = bound {
            for group in &touched {
                let through: i64 = step.tx.query_row(
                    "SELECT COALESCE(
                         (SELECT MIN(l.seq) - 1 FROM outbox o JOIN log l ON l.group_id = o.group_id AND l.digest = unhex(o.message_id)
                          WHERE o.endpoint = ?1 AND o.group_id = ?2),
                         (SELECT last_seq FROM groups WHERE group_id = ?2), 0)",
                    params![endpoint, group],
                    |row| row.get(0),
                )?;
                keep_place(step, &leaf, group, through)?;
                prune(step, group)?;
            }
        }
        Ok(())
    })
}

/// One of the person's messages a friend's home took.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Delivered {
    pub group_id: Vec<u8>,
    pub client_message_id: String,
    pub endpoint: String,
    pub at_ms: i64,
}

/// What a friends' home took of `leaf`'s messages, newest first, at most
/// `limit` (the last 90 days): DmFollow's, so a device that restarts still
/// knows. A message that endpoint bounced since isn't one.
pub(crate) fn deliveries(factory: &impl Host, leaf: &str, limit: usize) -> Result<Vec<Delivered>, Refusal> {
    factory.run(|step| {
        let mut statement = step.tx.prepare(
            "SELECT group_id, client_message_id, endpoint, at FROM sent
             WHERE leaf_id = ?1 AND client_message_id IS NOT NULL ORDER BY at DESC, message_id LIMIT ?2",
        )?;
        let rows = statement
            .query_map(params![leaf, limit as i64], |row| {
                Ok(Delivered { group_id: row.get(0)?, client_message_id: row.get(1)?, endpoint: row.get(2)?, at_ms: row.get(3)? })
            })?
            .collect::<Result<_, _>>()?;
        Ok(rows)
    })
}

/// A friend's home bounced one of the person's messages (more than 5
/// epochs behind its own, `epoch`): taken only for an id sent or queued to
/// that endpoint (Fugo's scoped bounce). Whether it was.
pub(crate) fn bounced(factory: &impl Host, endpoint: &str, message_id: &str, epoch: u64) -> Result<bool, Refusal> {
    let endpoint = endpoint.to_ascii_lowercase();
    factory.run(|step| {
        // Handed on and too old at the person's home: let go, never sealed
        // again here (Fugo 33779, condition 5).
        if handing_to(step)?.is_some_and(|home| home == endpoint)
            && step.tx.execute("DELETE FROM handover WHERE message_id = ?1", [message_id])?
                + step.tx.execute("DELETE FROM pass_on WHERE message_id = ?1", [message_id])?
                > 0
        {
            return Ok(true);
        }
        let mut row: Option<(Vec<u8>, Option<String>, Option<String>)> = None;
        for table in ["outbox", "sent"] {
            if row.is_none() {
                row = step
                    .tx
                    .query_row(
                        &format!("SELECT group_id, leaf_id, client_message_id FROM {table} WHERE endpoint = ?1 AND message_id = ?2"),
                        params![endpoint, message_id],
                        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                    )
                    .optional()?;
            }
        }
        let Some((group_id, leaf_id, client)) = row else { return Ok(false) };
        step.tx.execute("DELETE FROM outbox WHERE endpoint = ?1 AND message_id = ?2", params![endpoint, message_id])?;
        // Not delivered after all (`deliveries`).
        step.tx.execute("DELETE FROM sent WHERE endpoint = ?1 AND message_id = ?2", params![endpoint, message_id])?;
        if let (Some(leaf_id), Some(client_message_id)) = (leaf_id, client) {
            step.arrivals.push(Arrival::Bounced { leaf_id, client_message_id, group_id, epoch, endpoint: endpoint.clone() });
        }
        Ok(true)
    })
}

/// The groups a leaf of the person's reads here.
pub(crate) fn groups(factory: &impl Host, leaf: &str) -> Result<Vec<GroupView>, Refusal> {
    factory.run(|step| {
        let mut statement = step.tx.prepare(
            "SELECT g.group_id, g.kind, g.epoch, g.last_seq, g.pruned_seq, c.acked FROM groups g JOIN members m ON m.group_id = g.group_id
             LEFT JOIN cursors c ON c.group_id = g.group_id AND c.leaf_id = m.leaf_id
             WHERE m.leaf_id = ?1 AND g.epoch IS NOT NULL ORDER BY g.created, g.group_id",
        )?;
        let rows = statement
            .query_map([leaf], |row| {
                let pruned: i64 = row.get(4)?;
                let acked: Option<i64> = row.get(5)?;
                Ok(GroupView {
                    group_id: row.get(0)?,
                    kind: row.get(1)?,
                    epoch: row.get::<_, i64>(2)? as u64,
                    last_seq: row.get(3)?,
                    place: acked.unwrap_or(pruned),
                    pruned,
                    behind: acked.is_none_or(|acked| acked < pruned),
                })
            })?
            .collect::<Result<_, _>>()?;
        Ok(rows)
    })
}

/// A page of a group's log after `after`, for one of the person's leaves.
/// Reading after a seq keeps the leaf's place there too.
pub(crate) fn page(factory: &impl Host, leaf: &str, group_id: &[u8], after: i64, limit: usize) -> Result<Result<Page, TooFarBehind>, Refusal> {
    factory.run(|step| {
        let group = step.group(group_id)?.filter(|group| group.epoch.is_some());
        let reach = reach_of(step, leaf, group_id)?;
        let (Some(group), Some(through)) = (group, reach) else {
            return Err(Refusal::Invalid("This home doesn't hold that group for that leaf.".to_owned()));
        };
        if after < group.pruned_seq {
            return Ok(Err(TooFarBehind { epoch: group.epoch.unwrap_or_default() }));
        }
        keep_place(step, leaf, group_id, after)?;
        let limit = limit.clamp(1, PAGE_MOST);
        type Row = (i64, String, i64, Vec<u8>, bool, Option<String>, Option<String>, i64);
        let rows: Vec<Row> = {
            let mut statement = step.tx.prepare(
                "SELECT seq, kind, epoch, envelope, mine, leaf_id, client_message_id, accepted FROM log
                 WHERE group_id = ?1 AND seq > ?2 AND seq <= ?4 ORDER BY seq LIMIT ?3",
            )?;
            statement
                .query_map(params![group_id, after, limit as i64 + 1, through], |row| {
                    Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?, row.get(5)?, row.get(6)?, row.get(7)?))
                })?
                .collect::<Result<_, _>>()?
        };
        let mut has_more = rows.len() > limit;
        let mut bytes = 0;
        let mut entries = Vec::new();
        for (seq, kind, epoch, envelope, mine, from_leaf, client, accepted) in rows.into_iter().take(limit) {
            bytes += envelope.len();
            if bytes > PAGE_BYTES && !entries.is_empty() {
                has_more = true;
                break;
            }
            entries.push(Entry {
                group_id: group_id.to_vec(),
                seq,
                kind,
                epoch: epoch as u64,
                envelope,
                accepted_ms: accepted,
                from_me: mine,
                client_message_id: client.filter(|_| from_leaf.as_deref() == Some(leaf)),
            });
        }
        Ok(Ok(Page { entries, has_more }))
    })
}

/// How far a leaf may read a group here: to the end while it's in it, to
/// the commit that took it out once one did; None if neither.
fn reach_of(step: &Step<'_>, leaf: &str, group_id: &[u8]) -> rusqlite::Result<Option<i64>> {
    let member: bool =
        step.tx.query_row("SELECT COUNT(*) > 0 FROM members WHERE group_id = ?1 AND leaf_id = ?2", params![group_id, leaf], |row| row.get(0))?;
    if member {
        return Ok(Some(i64::MAX));
    }
    step.tx
        .query_row("SELECT through FROM left_at WHERE group_id = ?1 AND leaf_id = ?2", params![group_id, leaf], |row| row.get(0))
        .optional()
}

/// Where a leaf of the person's is in a group here (from its join on),
/// if the home holds the group for it (or a commit took it out).
pub(crate) fn place(factory: &impl Host, leaf: &str, group_id: &[u8]) -> Result<Option<i64>, Refusal> {
    factory.run(|step| Ok(step.tx.query_row(
        "SELECT c.acked FROM cursors c JOIN groups g ON g.group_id = c.group_id
         WHERE c.leaf_id = ?1 AND c.group_id = ?2 AND g.epoch IS NOT NULL
         AND (EXISTS (SELECT 1 FROM members m WHERE m.group_id = c.group_id AND m.leaf_id = c.leaf_id)
              OR EXISTS (SELECT 1 FROM left_at l WHERE l.group_id = c.group_id AND l.leaf_id = c.leaf_id))",
        params![leaf, group_id],
        |row| row.get(0),
    ).optional()?))
}

/// The first seq a home gives a group: above every seq the worker gave.
pub(crate) const FIRST_SEQ: i64 = (1 << 32) + 1;

/// When this Claktory last became the person's home, in ms
/// (DmHome.home_since_ms); 0 if it never did. One that became it before
/// the time was kept reads it from where its seqs start (`seq_base`).
pub(crate) fn home_since(factory: &impl Host) -> Result<i64, Refusal> {
    factory.run(|step| {
        let number = |name| -> Result<Option<i64>, Refusal> {
            Ok(meta(step.tx, name)?.and_then(|value| String::from_utf8_lossy(&value).parse::<i64>().ok()))
        };
        if let Some(since) = number("home_since")? {
            return Ok(since);
        }
        Ok(number("seq_base")?.map_or(0, |base| ((base - FIRST_SEQ) / 1000).max(0)))
    })
}

/// Where a Claktory's seqs start once it becomes the person's home at
/// `now_ms` (row 574): above the worker's, and, by the clock, above every
/// seq a home that was the home before gave (it would have to have taken
/// more than a thousand messages a millisecond in one group). A device's
/// place from the old home is then always before what this one gives, so
/// it reads everything here. Needs the homes' clocks roughly right: one
/// behind the old home's by longer than the old one was the home could
/// still number below a device's place there.
fn seq_base(now_ms: i64) -> i64 {
    FIRST_SEQ + now_ms.max(0) * 1000
}

/// A leaf kept a group through `through`: the log may let go of it.
pub(crate) fn ack(factory: &impl Host, leaf: &str, group_id: &[u8], through: i64) -> Result<(), Refusal> {
    factory.run(|step| {
        keep_place(step, leaf, group_id, through)?;
        prune(step, group_id)?;
        Ok(())
    })
}

fn keep_place(step: &Step<'_>, leaf: &str, group_id: &[u8], through: i64) -> rusqlite::Result<()> {
    step.tx.execute(
        "UPDATE cursors SET acked = MAX(acked, MIN(?3, (SELECT last_seq FROM groups WHERE group_id = ?2))), touched = ?4
         WHERE leaf_id = ?1 AND group_id = ?2",
        params![leaf, group_id, through, step.now],
    )?;
    Ok(())
}

/// Lets go of what every one of the person's leaves in the group has kept
/// (leaves silent for 90 days aren't waited for).
fn prune(step: &Step<'_>, group_id: &[u8]) -> rusqlite::Result<()> {
    let through: Option<i64> = step.tx.query_row(
        "SELECT MIN(c.acked) FROM members m JOIN cursors c ON c.group_id = m.group_id AND c.leaf_id = m.leaf_id
         WHERE m.group_id = ?1 AND c.touched >= ?2",
        params![group_id, step.now - KEEP_MS],
        |row| row.get(0),
    )?;
    if let Some(through) = through {
        let_go(step, group_id, through)?;
    }
    Ok(())
}

/// Moves a group's log past `through`, remembering what went.
fn let_go(step: &Step<'_>, group_id: &[u8], through: i64) -> rusqlite::Result<()> {
    step.tx.execute(
        "INSERT INTO gone (group_id, digest, seq, at) SELECT group_id, digest, seq, ?3 FROM log WHERE group_id = ?1 AND seq <= ?2
         ON CONFLICT (group_id, digest) DO NOTHING",
        params![group_id, through, step.now],
    )?;
    step.tx.execute("DELETE FROM log WHERE group_id = ?1 AND seq <= ?2", params![group_id, through])?;
    step.tx.execute("UPDATE groups SET pruned_seq = MAX(pruned_seq, ?2) WHERE group_id = ?1", params![group_id, through])?;
    Ok(())
}

/// Forgets what's past its 90 days: log entries, held messages, what went,
/// and the places of leaves silent that long. How many rows went.
pub(crate) fn sweep(factory: &impl Host) -> Result<usize, Refusal> {
    factory.run(|step| {
        let cutoff = step.now - KEEP_MS;
        let old: Vec<(Vec<u8>, i64)> = {
            let mut statement = step.tx.prepare("SELECT group_id, MAX(seq) FROM log WHERE accepted < ?1 GROUP BY group_id")?;
            statement.query_map([cutoff], |row| Ok((row.get(0)?, row.get(1)?)))?.collect::<Result<_, _>>()?
        };
        let mut gone = 0;
        for (group, through) in &old {
            let_go(step, group, *through)?;
            gone += 1;
        }
        gone += step.tx.execute("DELETE FROM held WHERE received < ?1", [cutoff])?;
        gone += step.tx.execute("DELETE FROM gone WHERE at < ?1", [cutoff])?;
        gone += step.tx.execute("DELETE FROM sent WHERE at < ?1", [cutoff])?;
        gone += step.tx.execute("DELETE FROM outbox WHERE queued < ?1", [cutoff])?;
        gone += step.tx.execute("DELETE FROM handover WHERE until < ?1", [step.now])?;
        gone += step.tx.execute("DELETE FROM pass_on WHERE until < ?1", [step.now])?;
        gone += step.tx.execute("DELETE FROM passed_in WHERE at < ?1", [cutoff])?;
        gone += step.tx.execute("DELETE FROM cursors WHERE touched < ?1", [cutoff])?;
        gone += step.tx.execute(
            "DELETE FROM left_at WHERE NOT EXISTS (SELECT 1 FROM cursors c WHERE c.group_id = left_at.group_id AND c.leaf_id = left_at.leaf_id)",
            [],
        )?;
        let groups: Vec<Vec<u8>> = {
            let mut statement = step.tx.prepare("SELECT group_id FROM groups")?;
            statement.query_map([], |row| row.get(0))?.collect::<Result<_, _>>()?
        };
        for group in groups {
            prune(step, &group)?;
        }
        Ok(gone)
    })
}

/// How much each friend has held here, in bytes (DmHome's held_bytes): not
/// the person's own a former home passed on, though their cap counts it.
pub(crate) fn held_bytes(factory: &impl Host) -> Result<BTreeMap<String, i64>, Refusal> {
    factory.run(|step| {
        let mut statement = step.tx.prepare(
            "SELECT account, SUM(bytes) FROM (SELECT account, LENGTH(envelope) AS bytes FROM log WHERE mine = 0
             UNION ALL SELECT account, LENGTH(envelope) FROM held WHERE mine = 0) WHERE account != ?1 GROUP BY account",
        )?;
        let rows = statement.query_map([step.account], |row| Ok((row.get(0)?, row.get(1)?)))?.collect::<Result<_, _>>()?;
        Ok(rows)
    })
}

/// Whether intake is paused for the disk.
pub(crate) fn intake_paused(factory: &impl Host) -> bool {
    free_bytes(factory.home_dir()).is_some_and(|free| free < FREE_FLOOR)
}

/// Tests only: the disk as `free` bytes free, on this thread.
#[cfg(test)]
pub(crate) fn set_test_free(free: Option<u64>) {
    TEST_FREE.with(|cell| cell.set(free));
}

/// Tests only: smaller caps, on this thread.
#[cfg(test)]
fn set_test_caps(caps: Option<(i64, i64)>) {
    TEST_CAPS.with(|cell| cell.set(caps));
}

/// How often the pull looks again with nothing to wake it, and the most it
/// backs off after failing.
const PULL_IDLE: std::time::Duration = std::time::Duration::from_secs(600);
const PULL_MOST: std::time::Duration = std::time::Duration::from_secs(900);
/// How often the 90-day sweep runs.
const SWEEP_EVERY: std::time::Duration = std::time::Duration::from_secs(3600);

/// Pulls from claktory.com what the home needs (`wanted`): GroupInfos for
/// groups it has no state of, and commits after an epoch for groups a hint
/// or a held message says are behind. Every commit it takes this way is the
/// server's. Also runs the 90-day sweep. Only while this Claktory is the
/// person's home, or another of their Claktories that the home fills.
pub(crate) async fn monitor(factory: std::sync::Weak<Factory>) {
    if let Some(strong) = factory.upgrade() {
        restore(&strong);
    }
    crate::network::ready().await;
    let mut backoff = std::time::Duration::from_secs(30);
    let mut swept: Option<tokio::time::Instant> = None;
    loop {
        let Some(strong) = factory.upgrade() else { return };
        let wake = strong.dm_home.pull.clone();
        let serves = strong.dm_home.is_home.load(std::sync::atomic::Ordering::SeqCst)
            || strong.dm_home.away.load(std::sync::atomic::Ordering::SeqCst);
        let pause = if !serves {
            PULL_IDLE
        } else {
            if swept.is_none_or(|at| at.elapsed() >= SWEEP_EVERY) {
                match sweep(&*strong) {
                    Ok(_) => swept = Some(tokio::time::Instant::now()),
                    Err(error) => tracing::debug!(?error, "the direct-message home wasn't swept"),
                }
            }
            match pull(&strong).await {
                Ok(()) => {
                    backoff = std::time::Duration::from_secs(30);
                    PULL_IDLE
                }
                Err(problem) => {
                    tracing::debug!(?problem, "the direct-message home couldn't pull from claktory.com");
                    let pause = backoff;
                    backoff = (backoff * 2).min(PULL_MOST);
                    pause
                }
            }
        };
        drop(strong);
        tokio::select! {
            () = tokio::time::sleep(pause) => {}
            () = wake.notified() => {}
        }
    }
}

#[derive(serde::Deserialize)]
struct PulledInfo {
    group_info: serde_json::Value,
}

#[derive(serde::Deserialize)]
struct PulledCommits {
    #[serde(default)]
    commits: Vec<PulledCommit>,
    #[serde(default)]
    has_more: bool,
}

#[derive(serde::Deserialize)]
struct PulledCommit {
    commit: serde_json::Value,
}

/// One pass over what the home wants.
async fn pull(factory: &Factory) -> Result<(), crate::account::apps::Problem> {
    use base64::Engine as _;
    use base64::engine::general_purpose::URL_SAFE_NO_PAD;
    use crate::account::apps::Problem;
    let unreadable = |error: String| Problem::ForNow(format!("an unreadable answer: {error}"));
    let wants = match wanted(factory) {
        Ok(wants) => wants,
        Err(_) => return Ok(()),
    };
    for want in wants {
        let id = URL_SAFE_NO_PAD.encode(&want.group_id);
        if want.group_info {
            let answer = factory.account.dms("GET", &format!("groups/{id}/group-info"), &[], None, None, None).await?;
            let info: PulledInfo = serde_json::from_slice(&answer).map_err(|error| unreadable(error.to_string()))?;
            let envelope = Envelope::from_json(&info.group_info).map_err(|error| unreadable(error.to_string()))?;
            if let Err(error) = seed(factory, envelope.bytes()) {
                tracing::info!(?error, "a pulled GroupInfo wasn't taken");
            }
        }
        let Some(mut after) = want.after_epoch else { continue };
        let mut any = false;
        loop {
            let query = [("after_epoch", after.to_string())];
            let answer = match factory.account.dms("GET", &format!("groups/{id}/commits"), &query, None, None, None).await {
                // claktory.com let go of the commit after that epoch: the
                // group starts again from its GroupInfo.
                Err(Problem::Refused(410, _)) => {
                    let answer = factory.account.dms("GET", &format!("groups/{id}/group-info"), &[], None, None, None).await?;
                    let info: PulledInfo = serde_json::from_slice(&answer).map_err(|error| unreadable(error.to_string()))?;
                    let envelope = Envelope::from_json(&info.group_info).map_err(|error| unreadable(error.to_string()))?;
                    if let Err(error) = seed(factory, envelope.bytes()) {
                        tracing::info!(?error, "a pulled GroupInfo wasn't taken");
                    }
                    any = true;
                    break;
                }
                answer => answer?,
            };
            let page: PulledCommits = serde_json::from_slice(&answer).map_err(|error| unreadable(error.to_string()))?;
            for commit in &page.commits {
                let envelope = Envelope::from_json(&commit.commit).map_err(|error| unreadable(error.to_string()))?;
                if let Some((_, epoch)) = envelope.parsed().group {
                    after = after.max(epoch + 1);
                }
                any = true;
                let intake = Intake { group_id: want.group_id.clone(), envelope: envelope.into_bytes(), source: Source::Server };
                if let Err(error) = take(factory, intake) {
                    tracing::info!(?error, "a pulled commit wasn't taken");
                }
            }
            if !page.has_more || page.commits.is_empty() {
                break;
            }
        }
        if !any {
            let _ = caught_up(factory, &want.group_id);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use claktory_dm_crypto::CIPHERSUITE;
    use claktory_dm_crypto::certificate::Leaf;
    use claktory_dm_crypto::group::LeafKey;
    use claktory_dm_crypto::identity::IdentityKey;
    use openmls::prelude::{
        CredentialWithKey, KeyPackage, MlsGroup, MlsGroupJoinConfig, MlsMessageIn, MlsMessageOut, PURE_PLAINTEXT_WIRE_FORMAT_POLICY,
        SignaturePublicKey, StagedWelcome,
    };

    use super::*;

    /// One of a person's devices: its own MLS store and leaf.
    struct Device {
        db: Connection,
        key: SealKey,
        crypto: Crypto,
        leaf: LeafKey,
        credential: CredentialWithKey,
        id: String,
    }

    impl Device {
        fn new(identity: &IdentityKey, account: &str, id: &str) -> Device {
            Device::of_kind(identity, account, id, LeafKind::Device)
        }

        fn of_kind(identity: &IdentityKey, account: &str, id: &str, kind: LeafKind) -> Device {
            let leaf = LeafKey::generate().unwrap();
            let certificate = LeafCertificate::issue(
                identity,
                Leaf {
                    kind,
                    account_id: account.to_owned(),
                    leaf_id: id.to_owned(),
                    signature_key: leaf.public(),
                    name: id.to_owned(),
                    created_ms: 1,
                },
            )
            .unwrap();
            let credential = CredentialWithKey {
                credential: certificate.to_credential().unwrap(),
                signature_key: SignaturePublicKey::from(leaf.public().to_vec()),
            };
            let db = Connection::open_in_memory().unwrap();
            db.pragma_update(None, "secure_delete", "ON").unwrap();
            db.execute_batch(mls_storage::SCHEMA).unwrap();
            db.execute_batch(mls_storage::FLOOR_SCHEMA).unwrap();
            Device { db, key: SealKey::from_bytes([7; 32]), crypto: Crypto::new().unwrap(), leaf, credential, id: id.to_owned() }
        }

        fn provider(&self) -> Provider<'_> {
            Provider::new(&self.crypto, &self.db, &self.key).unwrap()
        }

        fn key_package(&self) -> KeyPackage {
            KeyPackage::builder().build(CIPHERSUITE, &self.provider(), &self.leaf, self.credential.clone()).unwrap().key_package().clone()
        }

        fn create(&self, id: &[u8]) -> MlsGroup {
            MlsGroup::builder()
                .with_group_id(GroupId::from_slice(id))
                .ciphersuite(CIPHERSUITE)
                .with_wire_format_policy(PURE_PLAINTEXT_WIRE_FORMAT_POLICY)
                .use_ratchet_tree_extension(true)
                .max_past_epochs(5)
                .build(&self.provider(), &self.leaf, self.credential.clone())
                .unwrap()
        }

        fn join(&self, welcome: MlsMessageOut) -> MlsGroup {
            let MlsMessageBodyIn::Welcome(welcome) = MlsMessageIn::from(welcome).extract() else { panic!("not a Welcome") };
            let config = MlsGroupJoinConfig::builder()
                .wire_format_policy(PURE_PLAINTEXT_WIRE_FORMAT_POLICY)
                .use_ratchet_tree_extension(true)
                .max_past_epochs(5)
                .build();
            StagedWelcome::new_from_welcome(&self.provider(), &config, welcome, None).unwrap().into_group(&self.provider()).unwrap()
        }

        fn info(&self, group: &MlsGroup) -> Vec<u8> {
            group.export_group_info(self.provider().crypto(), &self.leaf, true).unwrap().to_bytes().unwrap()
        }

        fn say(&self, group: &mut MlsGroup, text: &str) -> Vec<u8> {
            group.create_message(&self.provider(), &self.leaf, text.as_bytes()).unwrap().to_bytes().unwrap()
        }

        /// An empty commit (an Update of its own leaf), merged: the next epoch.
        fn update(&self, group: &mut MlsGroup) -> Vec<u8> {
            let bundle = group.self_update(&self.provider(), &self.leaf, Default::default()).unwrap();
            let commit = bundle.commit().clone().to_bytes().unwrap();
            group.merge_pending_commit(&self.provider()).unwrap();
            commit
        }

        fn read(&self, group: &mut MlsGroup, bytes: &[u8]) {
            let message = claktory_dm_crypto::envelope::parse(bytes).unwrap().into_protocol_message().unwrap();
            let processed = group.process_message(&self.provider(), message).unwrap();
            if let ProcessedMessageContent::StagedCommitMessage(staged) = processed.into_content() {
                group.merge_staged_commit(&self.provider(), *staged).unwrap();
            }
        }
    }

    /// A home on a folder of its own, as if this Claktory were the person's.
    struct TestHost {
        store: Mutex<Store>,
        home: Home,
        is_home: bool,
        arrivals: Mutex<Vec<Arrival>>,
    }

    impl Host for TestHost {
        fn run<T>(&self, f: impl FnOnce(&mut Step<'_>) -> Result<T, Refusal>) -> Result<T, Refusal> {
            let mut store = self.store.lock().unwrap();
            let (out, arrivals) = run_on(&mut store, &self.home, self.is_home, f)?;
            self.arrivals.lock().unwrap().extend(arrivals);
            Ok(out)
        }

        fn home_dir(&self) -> &Home {
            &self.home
        }
    }

    impl TestHost {
        fn new(account: &str) -> TestHost {
            static N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
            let n = N.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            let root = std::env::temp_dir().join(format!("factoryd-dm-home-{}-{n}-{}", std::process::id(), now_ms()));
            std::fs::create_dir_all(&root).unwrap();
            let home = Home::new(root);
            let store = Store::open(&home, account).unwrap();
            TestHost { store: Mutex::new(store), home, is_home: true, arrivals: Mutex::new(Vec::new()) }
        }

        fn arrived(&self) -> Vec<Arrival> {
            std::mem::take(&mut *self.arrivals.lock().unwrap())
        }

        fn epoch(&self, group: &[u8]) -> Option<i64> {
            self.store.lock().unwrap().db.query_row("SELECT epoch FROM groups WHERE group_id = ?1", [group], |row| row.get(0)).unwrap()
        }

        fn log(&self, group: &[u8]) -> Vec<(i64, String)> {
            let store = self.store.lock().unwrap();
            let mut statement = store.db.prepare("SELECT seq, kind FROM log WHERE group_id = ?1 ORDER BY seq").unwrap();
            statement.query_map([group], |row| Ok((row.get(0)?, row.get(1)?))).unwrap().collect::<Result<_, _>>().unwrap()
        }
    }

    impl Drop for TestHost {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(self.home.root());
        }
    }

    const GROUP: [u8; 16] = [9; 16];
    const BO_HOME: &str = "b0b0";
    const FIRST: i64 = (1 << 32) + 1;

    fn device(intake_from: &Device, client: &str) -> Source {
        Source::Device { leaf_id: intake_from.id.clone(), client_message_id: client.to_owned() }
    }

    fn from_bo(envelope: &[u8]) -> Source {
        Source::Home { endpoint: BO_HOME.to_owned(), message_id: hex(&claktory_dm_crypto::envelope::digest(envelope)) }
    }

    fn take_(host: &TestHost, envelope: Vec<u8>, source: Source) -> Result<Taken, Refusal> {
        take(host, Intake { group_id: GROUP.to_vec(), envelope, source })
    }

    /// Ann's home, holding Ann and Bo's conversation at epoch 1 (Ann's
    /// phone made it and added Bo's phone), with Bo's home routed.
    struct World {
        host: TestHost,
        ann: Device,
        bo: Device,
        ann_group: MlsGroup,
        bo_group: MlsGroup,
    }

    fn world() -> World {
        let ann_id = IdentityKey::generate().unwrap();
        let bo_id = IdentityKey::generate().unwrap();
        let ann = Device::new(&ann_id, "ann", "ann-phone");
        let bo = Device::new(&bo_id, "bo", "bo-phone");
        let host = TestHost::new("ann");
        let mut ann_group = ann.create(&GROUP);
        set_routes(&host, vec![Route { account: "bo".to_owned(), endpoint: BO_HOME.to_owned(), group_ids: vec![GROUP.to_vec()] }]).unwrap();
        seed(&host, &ann.info(&ann_group)).unwrap();
        let (commit, welcome, _) = ann_group.add_members(&ann.provider(), &ann.leaf, &[bo.key_package()]).unwrap();
        ann_group.merge_pending_commit(&ann.provider()).unwrap();
        let bo_group = bo.join(welcome);
        let taken = take_(&host, commit.to_bytes().unwrap(), device(&ann, "")).unwrap();
        assert!(matches!(taken, Taken::Stored { seq: FIRST, duplicate: false, .. }), "{taken:?}");
        assert_eq!(host.epoch(&GROUP), Some(1));
        host.arrived();
        // The commit is queued for Bo's home too; it took it.
        let queued = outbox(&host, BO_HOME, 10).unwrap();
        sent(&host, BO_HOME, &queued.iter().map(|out| out.message_id.clone()).collect::<Vec<_>>()).unwrap();
        World { host, ann, bo, ann_group, bo_group }
    }

    #[test]
    fn a_friends_message_is_logged_and_served_to_the_persons_leaves() {
        let mut w = world();
        let hello = w.bo.say(&mut w.bo_group, "hello Ann");
        let taken = take_(&w.host, hello.clone(), from_bo(&hello)).unwrap();
        let Taken::Stored { seq, duplicate: false, .. } = taken else { panic!("{taken:?}") };
        assert_eq!(seq, FIRST + 1);
        assert_eq!(w.host.arrived(), vec![Arrival::Entry { group_id: GROUP.to_vec(), seq }]);
        // Again: the same seq, nothing new.
        assert!(matches!(take_(&w.host, hello.clone(), from_bo(&hello)), Ok(Taken::Stored { duplicate: true, .. })));

        let views = groups(&w.host, "ann-phone").unwrap();
        assert_eq!(views.len(), 1);
        assert_eq!((views[0].kind.as_str(), views[0].epoch, views[0].place, views[0].behind), ("conversation", 1, FIRST - 1, false));
        let read = page(&w.host, "ann-phone", &GROUP, FIRST, 50).unwrap().unwrap();
        assert_eq!(read.entries.len(), 1);
        assert!(!read.entries[0].from_me);
        w.ann.read(&mut w.ann_group, &read.entries[0].envelope);

        // Ann's own message: logged, queued for Bo's home, delivered.
        let reply = w.ann.say(&mut w.ann_group, "hi Bo");
        let taken = take_(&w.host, reply.clone(), device(&w.ann, "c1")).unwrap();
        assert!(matches!(taken, Taken::Stored { seq, .. } if seq == FIRST + 2));
        let queued = outbox(&w.host, BO_HOME, 10).unwrap();
        assert_eq!(queued.len(), 1);
        assert_eq!(queued[0].envelope, reply);
        w.host.arrived();
        sent(&w.host, BO_HOME, &[queued[0].message_id.clone()]).unwrap();
        assert_eq!(
            w.host.arrived(),
            vec![Arrival::Delivered {
                leaf_id: "ann-phone".to_owned(),
                client_message_id: "c1".to_owned(),
                group_id: GROUP.to_vec(),
                endpoint: BO_HOME.to_owned()
            }]
        );
        assert!(outbox(&w.host, BO_HOME, 10).unwrap().is_empty());
        // The same id with other bytes is a conflict, never a second message.
        let other = w.ann.say(&mut w.ann_group, "hi again");
        assert_eq!(take_(&w.host, other, device(&w.ann, "c1")), Err(Refusal::Conflict));
        let read = page(&w.host, "ann-phone", &GROUP, FIRST + 1, 50).unwrap().unwrap();
        assert_eq!(read.entries[0].client_message_id.as_deref(), Some("c1"));
        assert!(read.entries[0].from_me);
    }

    #[test]
    fn only_routed_endpoints_and_groups_are_taken_from() {
        let mut w = world();
        let hello = w.bo.say(&mut w.bo_group, "hello");
        let stranger = Source::Home { endpoint: "dead".to_owned(), message_id: hex(&claktory_dm_crypto::envelope::digest(&hello)) };
        assert_eq!(take_(&w.host, hello.clone(), stranger), Err(Refusal::NotRouted));
        let mislabeled = take(&w.host, Intake { group_id: [8; 16].to_vec(), envelope: hello.clone(), source: from_bo(&hello) });
        assert!(matches!(mislabeled, Err(Refusal::Invalid(_))));
        let wrong_id = Source::Home { endpoint: BO_HOME.to_owned(), message_id: "00".to_owned() };
        assert!(matches!(take_(&w.host, hello.clone(), wrong_id), Err(Refusal::Invalid(_))));
        assert_eq!(route_of(&w.host, BO_HOME).unwrap().map(|route| route.account), Some("bo".to_owned()));
        assert_eq!(route_of(&w.host, "dead").unwrap(), None);
        assert_eq!(routes(&w.host).unwrap(), vec![Route { account: "bo".to_owned(), endpoint: BO_HOME.to_owned(), group_ids: vec![GROUP.to_vec()] }]);
        // Bo's home routed for another group only: this one isn't taken.
        set_routes(&w.host, vec![Route { account: "bo".to_owned(), endpoint: BO_HOME.to_owned(), group_ids: vec![[8; 16].to_vec()] }]).unwrap();
        assert_eq!(take_(&w.host, hello.clone(), from_bo(&hello)), Err(Refusal::NotRouted));
    }

    #[test]
    fn a_friends_commit_only_wakes_the_pull_and_the_servers_copy_is_taken() {
        let mut w = world();
        // Bo's home passes on a commit (Fugo's condition 4): even one that
        // would verify, it changes nothing until claktory.com has it.
        let commit = w.bo.update(&mut w.bo_group);
        let before = w.host.log(&GROUP);
        assert_eq!(take_(&w.host, commit.clone(), from_bo(&commit)), Ok(Taken::Held));
        assert_eq!(w.host.epoch(&GROUP), Some(1));
        assert_eq!(w.host.log(&GROUP), before);
        assert_eq!(wanted(&w.host).unwrap(), vec![Want { group_id: GROUP.to_vec(), group_info: false, after_epoch: Some(1) }]);
        // A message from that epoch waits for it.
        let later = w.bo.say(&mut w.bo_group, "after the commit");
        assert_eq!(take_(&w.host, later.clone(), from_bo(&later)), Ok(Taken::Held));
        assert_eq!(w.host.log(&GROUP), before);
        // The server's copy: logged, the epoch moves, the held message follows.
        assert!(matches!(take_(&w.host, commit.clone(), Source::Server), Ok(Taken::Stored { duplicate: false, .. })));
        assert_eq!(w.host.epoch(&GROUP), Some(2));
        let log = w.host.log(&GROUP);
        assert_eq!(&log[log.len() - 2..], &[(FIRST + 1, "commit".to_owned()), (FIRST + 2, "application".to_owned())]);
        let read = page(&w.host, "ann-phone", &GROUP, FIRST, 50).unwrap().unwrap();
        for entry in &read.entries {
            w.ann.read(&mut w.ann_group, &entry.envelope);
        }
        assert_eq!(w.ann_group.epoch().as_u64(), 2);
    }

    #[test]
    fn a_forged_commit_from_a_friend_is_never_stored() {
        let mut w = world();
        // A commit Bo's device made but the server never took: Bo's home
        // sends it anyway; the server's log then has another for epoch 1.
        let fork = w.bo.update(&mut w.bo_group);
        assert_eq!(take_(&w.host, fork.clone(), from_bo(&fork)), Ok(Taken::Held));
        let real = w.ann.update(&mut w.ann_group);
        assert!(matches!(take_(&w.host, real, Source::Server), Ok(Taken::Stored { .. })));
        assert_eq!(w.host.epoch(&GROUP), Some(2));
        // Sent again, now behind: still never stored.
        assert!(take_(&w.host, fork.clone(), from_bo(&fork)).is_ok());
        let digests: i64 = w
            .host
            .store
            .lock()
            .unwrap()
            .db
            .query_row("SELECT COUNT(*) FROM log WHERE digest = ?1", [claktory_dm_crypto::envelope::digest(&fork).to_vec()], |row| row.get(0))
            .unwrap();
        assert_eq!(digests, 0);
    }

    #[test]
    fn a_message_too_many_epochs_behind_is_bounced_and_bounces_are_scoped() {
        let mut w = world();
        let stale = w.bo.say(&mut w.bo_group, "old");
        for _ in 0..6 {
            let commit = w.ann.update(&mut w.ann_group);
            assert!(matches!(take_(&w.host, commit, device(&w.ann, "")), Ok(Taken::Stored { .. })));
        }
        assert_eq!(take_(&w.host, stale.clone(), from_bo(&stale)), Ok(Taken::Bounce { epoch: 7 }));
        // A bounce from Bo's home counts only for what was sent there.
        let mine = w.ann.say(&mut w.ann_group, "mine");
        take_(&w.host, mine.clone(), device(&w.ann, "m1")).unwrap();
        let id = hex(&claktory_dm_crypto::envelope::digest(&mine));
        assert!(!bounced(&w.host, "dead", &id, 9).unwrap());
        assert!(!bounced(&w.host, BO_HOME, "beef", 9).unwrap());
        // Taken there first: a delivery, until the bounce.
        sent(&w.host, BO_HOME, std::slice::from_ref(&id)).unwrap();
        let delivered = deliveries(&w.host, "ann-phone", 1000).unwrap();
        assert_eq!(
            delivered.iter().map(|d| (d.client_message_id.as_str(), d.endpoint.as_str())).collect::<Vec<_>>(),
            vec![("m1", BO_HOME)]
        );
        assert!(deliveries(&w.host, "bo-phone", 1000).unwrap().is_empty());
        w.host.arrived();
        assert!(bounced(&w.host, BO_HOME, &id, 9).unwrap());
        assert!(deliveries(&w.host, "ann-phone", 1000).unwrap().is_empty());
        assert_eq!(
            w.host.arrived(),
            vec![Arrival::Bounced {
                leaf_id: "ann-phone".to_owned(),
                client_message_id: "m1".to_owned(),
                group_id: GROUP.to_vec(),
                epoch: 9,
                endpoint: BO_HOME.to_owned()
            }]
        );
    }

    #[test]
    fn caps_and_the_free_space_floor_refuse_intake() {
        let mut w = world();
        set_test_free(Some(FREE_FLOOR - 1));
        let hello = w.bo.say(&mut w.bo_group, "hello");
        assert_eq!(take_(&w.host, hello.clone(), from_bo(&hello)), Err(Refusal::Full { retry_after_ms: FULL_RETRY_MS }));
        set_test_free(None);
        assert!(matches!(take_(&w.host, hello.clone(), from_bo(&hello)), Ok(Taken::Stored { .. })));
        // Bo at his cap: refused, as the bytes held say; then all of them.
        let more = w.bo.say(&mut w.bo_group, "more");
        set_test_caps(Some((hello.len() as i64 + 10, 1 << 30)));
        assert_eq!(take_(&w.host, more.clone(), from_bo(&more)), Err(Refusal::Full { retry_after_ms: FULL_RETRY_MS }));
        set_test_caps(Some((1 << 30, 10)));
        assert_eq!(take_(&w.host, more.clone(), from_bo(&more)), Err(Refusal::Full { retry_after_ms: FULL_RETRY_MS }));
        set_test_caps(None);
        // Ann's own devices aren't held to a friend's cap.
        let mine = w.ann.say(&mut w.ann_group, "mine");
        assert!(matches!(take_(&w.host, mine, device(&w.ann, "m1")), Ok(Taken::Stored { .. })));
    }

    #[test]
    fn acked_entries_go_and_a_leaf_behind_them_rejoins() {
        let mut w = world();
        for n in 0..3 {
            let text = w.bo.say(&mut w.bo_group, &format!("m{n}"));
            take_(&w.host, text.clone(), from_bo(&text)).unwrap();
        }
        assert_eq!(w.host.log(&GROUP).len(), 4);
        ack(&w.host, "ann-phone", &GROUP, FIRST + 2).unwrap();
        assert_eq!(w.host.log(&GROUP).iter().map(|(seq, _)| *seq).collect::<Vec<_>>(), vec![FIRST + 3]);
        assert_eq!(page(&w.host, "ann-phone", &GROUP, FIRST, 50).unwrap(), Err(TooFarBehind { epoch: 1 }));
        assert_eq!(page(&w.host, "ann-phone", &GROUP, FIRST + 2, 50).unwrap().unwrap().entries.len(), 1);
        // A message that went is still known: a retry isn't taken twice.
        let again = w.bo.say(&mut w.bo_group, "m3");
        take_(&w.host, again.clone(), from_bo(&again)).unwrap();
        ack(&w.host, "ann-phone", &GROUP, FIRST + 4).unwrap();
        assert!(matches!(take_(&w.host, again.clone(), from_bo(&again)), Ok(Taken::Stored { duplicate: true, seq, .. }) if seq == FIRST + 4));
        assert!(w.host.log(&GROUP).is_empty());
    }

    #[test]
    fn nothing_but_the_servers_commits_is_taken_until_this_claktory_is_home() {
        let mut w = world();
        w.host.is_home = false;
        let hello = w.bo.say(&mut w.bo_group, "hello");
        assert_eq!(take_(&w.host, hello.clone(), from_bo(&hello)), Err(Refusal::NotHome));
        let mine = w.ann.say(&mut w.ann_group, "mine");
        assert_eq!(take_(&w.host, mine, device(&w.ann, "m1")), Err(Refusal::NotHome));
        let commit = w.ann.update(&mut w.ann_group);
        assert!(matches!(take_(&w.host, commit, Source::Server), Ok(Taken::Stored { .. })));
        // This Claktory's own leaf, though, goes on: its commits (through
        // the routes), never a message in a conversation.
        let local = || Source::Local { leaf_id: "ann-phone".to_owned(), client_message_id: "l1".to_owned() };
        let own = w.ann.say(&mut w.ann_group, "from this Claktory's leaf");
        assert!(matches!(take_(&w.host, own, local()), Err(Refusal::Invalid(_))));
        let commit = w.ann.update(&mut w.ann_group);
        let local_commit = Source::Local { leaf_id: "ann-phone".to_owned(), client_message_id: String::new() };
        assert!(matches!(take_(&w.host, commit, local_commit), Ok(Taken::Stored { .. })));
        assert_eq!(outbox(&w.host, BO_HOME, 10).unwrap().len(), 1);
    }

    #[test]
    fn a_message_for_a_group_with_no_state_waits_for_its_group_info() {
        let ann_id = IdentityKey::generate().unwrap();
        let ann = Device::new(&ann_id, "ann", "ann-phone");
        let host = TestHost::new("ann");
        let mut group = ann.create(&GROUP);
        let info = ann.info(&group);
        let note = ann.say(&mut group, "a draft");
        assert_eq!(take_(&host, note.clone(), device(&ann, "d1")), Ok(Taken::Held));
        assert_eq!(wanted(&host).unwrap(), vec![Want { group_id: GROUP.to_vec(), group_info: true, after_epoch: None }]);
        seed(&host, &info).unwrap();
        assert_eq!(host.log(&GROUP), vec![(FIRST, "application".to_owned())]);
        let views = groups(&host, "ann-phone").unwrap();
        assert_eq!(views[0].kind, "self");
        assert!(wanted(&host).unwrap().is_empty());
    }

    #[test]
    fn another_claktory_of_the_persons_is_bound_by_its_hello_and_sent_every_entry() {
        const CHANNEL: [u8; 16] = [5; 16];
        const MAC: &str = "a11ce0";
        let ann_id = IdentityKey::generate().unwrap();
        let ann = Device::new(&ann_id, "ann", "ann-phone");
        let mac = Device::of_kind(&ann_id, "ann", "mac#1", LeafKind::Claktory);
        let host = TestHost::new("ann");
        let mut channel = ann.create(&CHANNEL);
        seed(&host, &ann.info(&channel)).unwrap();
        let (commit, welcome, _) = channel.add_members(&ann.provider(), &ann.leaf, &[mac.key_package()]).unwrap();
        channel.merge_pending_commit(&ann.provider()).unwrap();
        let mut mac_channel = mac.join(welcome);
        let in_channel = |envelope: Vec<u8>, source: Source| take(&host, Intake { group_id: CHANNEL.to_vec(), envelope, source });
        assert!(matches!(in_channel(commit.to_bytes().unwrap(), device(&ann, "")), Ok(Taken::Stored { .. })));
        assert_eq!(groups(&host, "mac#1").unwrap()[0].kind, "channel");

        // Not bound yet: nothing taken from it, nothing for it.
        let draft = mac.say(&mut mac_channel, "a draft");
        let from_mac = |envelope: &[u8]| Source::Home { endpoint: MAC.to_owned(), message_id: hex(&claktory_dm_crypto::envelope::digest(envelope)) };
        assert_eq!(in_channel(draft.clone(), from_mac(&draft)), Err(Refusal::NotRouted));
        assert_eq!(bound_leaf(&host, MAC).unwrap(), None);

        // Its hello binds it: a route of the person's own for its groups,
        // with nothing old to send (it was added by the last entry).
        assert!(bind(&host, "mac#1", MAC, 10).unwrap());
        assert_eq!(bound_leaf(&host, MAC).unwrap().as_deref(), Some("mac#1"));
        assert_eq!(route_of(&host, MAC).unwrap(), Some(Route { account: "ann".to_owned(), endpoint: MAC.to_owned(), group_ids: vec![CHANNEL.to_vec()] }));
        assert!(outbox(&host, MAC, 10).unwrap().is_empty());

        // The phone's message goes to it; its own isn't sent back.
        let hi = ann.say(&mut channel, "hi Mac");
        assert!(matches!(in_channel(hi.clone(), device(&ann, "c1")), Ok(Taken::Stored { seq, .. }) if seq == FIRST + 1));
        assert!(matches!(in_channel(draft.clone(), from_mac(&draft)), Ok(Taken::Stored { seq, .. }) if seq == FIRST + 2));
        let queued = outbox(&host, MAC, 10).unwrap();
        assert_eq!(queued.iter().map(|out| out.envelope.clone()).collect::<Vec<_>>(), vec![hi.clone()]);
        let read = page(&host, "ann-phone", &CHANNEL, FIRST, 50).unwrap().unwrap();
        assert!(read.entries[1].from_me);
        // What it took counts as kept: its place moves past it.
        host.arrived();
        sent(&host, MAC, &[queued[0].message_id.clone()]).unwrap();
        assert!(host.arrived().is_empty(), "no receipt for the person's own Claktory");
        assert_eq!(place(&host, "mac#1", &CHANNEL).unwrap(), Some(FIRST + 2));

        // The person's device's routes leave it alone.
        set_routes(&host, Vec::new()).unwrap();
        assert_eq!(bound_leaf(&host, MAC).unwrap().as_deref(), Some("mac#1"));

        // An older hello never binds it again; a newer one from elsewhere
        // moves it, and what waited goes with it.
        let more = ann.say(&mut channel, "more");
        in_channel(more.clone(), device(&ann, "c2")).unwrap();
        assert!(!bind(&host, "mac#1", "beef", 5).unwrap());
        assert!(!bind(&host, "mac#1", "beef", 10).unwrap());
        assert_eq!(bound_leaf(&host, "beef").unwrap(), None);
        assert!(bind(&host, "mac#1", "BEEF", 20).unwrap());
        assert_eq!(bound_leaf(&host, MAC).unwrap(), None);
        assert_eq!(outbox(&host, "beef", 10).unwrap().iter().map(|out| out.envelope.clone()).collect::<Vec<_>>(), vec![more]);
        assert_eq!(outbox_counts(&host).unwrap(), BTreeMap::from([("beef".to_owned(), 1)]));

        // Off the person's device list: unbound, and nothing waits for it.
        unbind_missing(&host, &["mac#1".to_owned()]).unwrap();
        assert!(route_of(&host, "beef").unwrap().is_some());
        unbind_missing(&host, &[]).unwrap();
        assert_eq!(route_of(&host, "beef").unwrap(), None);
        assert!(outbox_counts(&host).unwrap().is_empty());
    }

    #[test]
    fn a_valid_but_older_device_list_leaves_the_route_unchanged() {
        use claktory_dm_crypto::device_list::{DeviceList, Listed};
        use claktory_dm_crypto::validation::{Pins, Trust as _};
        let ann_id = IdentityKey::generate().unwrap();
        let mut host = TestHost::new("ann");
        host.is_home = false;
        let list = |version: u64, home: [u8; 32]| {
            DeviceList {
                account_id: "ann".to_owned(),
                version,
                created_ms: version,
                leaves: vec![Listed { kind: LeafKind::Claktory, leaf_id: "ann-mac".to_owned(), signature_key: [1; 32] }],
            }
            .sign_with(&ann_id, Some(home))
            .unwrap()
            .to_bytes()
            .unwrap()
        };
        let ours = [3; 32];
        let groups = vec![GROUP.to_vec()];
        let current = |pins: &Pins| placed(pins.current("ann").unwrap().1.unwrap(), &ours);
        let mut pins = Pins::default();
        pins.first_contact("ann", ann_id.public()).unwrap();

        pins.accept_list("ann", &list(2, [4; 32])).unwrap();
        assert_eq!(current(&pins), Placed::Away(Some(hex(&[4; 32]))));
        assert!(follow(&host, &current(&pins), &groups).unwrap());
        let to_home = vec![Route { account: "ann".to_owned(), endpoint: hex(&[4; 32]), group_ids: groups.clone() }];
        assert_eq!(routes(&host).unwrap(), to_home);

        // Validly signed, but older (or the same version, other bytes),
        // naming another home: refused, and the route stays.
        assert!(pins.accept_list("ann", &list(1, [5; 32])).is_err());
        assert!(pins.accept_list("ann", &list(2, [5; 32])).is_err());
        assert!(!follow(&host, &current(&pins), &groups).unwrap());
        assert_eq!(routes(&host).unwrap(), to_home);
        // The device's routes leave it alone too.
        set_routes(&host, Vec::new()).unwrap();
        assert_eq!(routes(&host).unwrap(), to_home);

        // A newer one naming this Claktory: it's the home, with no route of
        // its own.
        pins.accept_list("ann", &list(3, ours)).unwrap();
        assert_eq!(current(&pins), Placed::Home);
        assert!(follow(&host, &Placed::Home, &groups).unwrap());
        assert!(routes(&host).unwrap().is_empty());
    }

    #[test]
    fn a_friends_moved_home_gets_what_waited_for_the_old_one() {
        let mut w = world();
        let reply = w.ann.say(&mut w.ann_group, "hi Bo");
        take_(&w.host, reply.clone(), device(&w.ann, "c1")).unwrap();
        assert_eq!(outbox(&w.host, BO_HOME, 10).unwrap().len(), 1);
        // Bo made another Claktory his home: Ann's device routes there now.
        let moved = "b1b1";
        set_routes(&w.host, vec![Route { account: "bo".to_owned(), endpoint: moved.to_owned(), group_ids: vec![GROUP.to_vec()] }]).unwrap();
        assert!(outbox(&w.host, BO_HOME, 10).unwrap().is_empty());
        let queued = outbox(&w.host, moved, 10).unwrap();
        assert_eq!(queued.len(), 1);
        assert_eq!(queued[0].envelope, reply);
        w.host.arrived();
        sent(&w.host, moved, &[queued[0].message_id.clone()]).unwrap();
        assert_eq!(
            w.host.arrived(),
            vec![Arrival::Delivered {
                leaf_id: "ann-phone".to_owned(),
                client_message_id: "c1".to_owned(),
                group_id: GROUP.to_vec(),
                endpoint: moved.to_owned()
            }]
        );
        // Routed elsewhere for another group only, and someone else's home
        // for this one: nothing of Bo's reaches this group any more, so what
        // waited goes, never to another account's endpoint.
        let again = w.ann.say(&mut w.ann_group, "still there?");
        take_(&w.host, again, device(&w.ann, "c2")).unwrap();
        set_routes(
            &w.host,
            vec![
                Route { account: "bo".to_owned(), endpoint: "b2b2".to_owned(), group_ids: vec![[8; 16].to_vec()] },
                Route { account: "cy".to_owned(), endpoint: "c0c0".to_owned(), group_ids: vec![GROUP.to_vec()] },
            ],
        )
        .unwrap();
        assert!(outbox_counts(&w.host).unwrap().is_empty());
    }

    #[test]
    fn a_claktory_whose_home_moved_sends_what_waited_to_the_new_one() {
        let mut w = world();
        w.host.is_home = false;
        let groups = vec![GROUP.to_vec()];
        let (old, new) = ("a0a0", "a1a1");
        assert!(follow(&w.host, &Placed::Away(Some(old.to_owned())), &groups).unwrap());
        let commit = w.ann.update(&mut w.ann_group);
        let local_commit = Source::Local { leaf_id: "ann-phone".to_owned(), client_message_id: String::new() };
        assert!(matches!(take_(&w.host, commit.clone(), local_commit), Ok(Taken::Stored { .. })));
        assert_eq!(outbox(&w.host, old, 10).unwrap().len(), 1);
        // The person named another home.
        assert!(follow(&w.host, &Placed::Away(Some(new.to_owned())), &groups).unwrap());
        assert!(outbox(&w.host, old, 10).unwrap().is_empty());
        let queued = outbox(&w.host, new, 10).unwrap();
        assert_eq!(queued.len(), 1);
        assert_eq!(queued[0].envelope, commit);
        assert_eq!(outbox(&w.host, BO_HOME, 10).unwrap().len(), 1, "Bo's home keeps its own");
        // No home at all: nowhere to go.
        assert!(follow(&w.host, &Placed::Away(None), &groups).unwrap());
        assert!(outbox(&w.host, new, 10).unwrap().is_empty());
    }

    #[test]
    fn a_home_that_stops_being_it_hands_what_waited_on_to_the_new_one() {
        let mut w = world();
        assert!(follow(&w.host, &Placed::Home, &[]).unwrap());
        assert!(!follow(&w.host, &Placed::Home, &[]).unwrap());
        let reply = w.ann.say(&mut w.ann_group, "hi Bo");
        take_(&w.host, reply.clone(), device(&w.ann, "c1")).unwrap();
        assert_eq!(outbox(&w.host, BO_HOME, 10).unwrap().len(), 1);

        // The person's list names another home: what waited for Bo's home
        // goes to it, and Bo's route goes (nothing new is queued there).
        let (new_home, later) = ("a1a1", "a2a2");
        w.host.is_home = false;
        assert!(follow(&w.host, &Placed::Away(Some(new_home.to_owned())), &[]).unwrap());
        assert!(outbox(&w.host, BO_HOME, 10).unwrap().is_empty());
        assert_eq!(routes(&w.host).unwrap(), vec![Route { account: "ann".to_owned(), endpoint: new_home.to_owned(), group_ids: vec![GROUP.to_vec()] }]);
        let handed = outbox(&w.host, new_home, 10).unwrap();
        assert_eq!(handed.len(), 1);
        assert_eq!(handed[0].envelope, reply);
        assert_eq!(handed[0].handover, Some(("ann-phone".to_owned(), "c1".to_owned())));

        // No home named for a while, then another: it waits, then goes there.
        assert!(follow(&w.host, &Placed::Away(None), &[]).unwrap());
        assert!(routes(&w.host).unwrap().is_empty());
        assert!(follow(&w.host, &Placed::Away(Some(later.to_owned())), &[]).unwrap());
        assert!(outbox(&w.host, new_home, 10).unwrap().is_empty());
        assert_eq!(outbox(&w.host, later, 10).unwrap(), handed);

        // Taken there: done with, and no route left.
        sent(&w.host, later, &[handed[0].message_id.clone()]).unwrap();
        assert!(outbox(&w.host, later, 10).unwrap().is_empty());
        assert!(routes(&w.host).unwrap().is_empty());

        // Too old there: let go, never sealed again.
        let again = w.ann.say(&mut w.ann_group, "again");
        w.host.is_home = true;
        assert!(follow(&w.host, &Placed::Home, &[]).unwrap());
        set_routes(&w.host, vec![Route { account: "bo".to_owned(), endpoint: BO_HOME.to_owned(), group_ids: vec![GROUP.to_vec()] }]).unwrap();
        take_(&w.host, again, device(&w.ann, "c2")).unwrap();
        w.host.is_home = false;
        assert!(follow(&w.host, &Placed::Away(Some(later.to_owned())), &[]).unwrap());
        // (The reply, which no leaf of Ann's read here, is passed on too.)
        let handed: Vec<_> = outbox(&w.host, later, 10).unwrap().into_iter().filter(|out| out.handover.is_some()).collect();
        assert_eq!(handed.len(), 1);
        assert!(bounced(&w.host, later, &handed[0].message_id, 9).unwrap());
        assert!(outbox(&w.host, later, 10).unwrap().iter().all(|out| out.handover.is_none()));

        // Past its week: gone.
        let last = w.ann.say(&mut w.ann_group, "last");
        w.host.is_home = true;
        assert!(follow(&w.host, &Placed::Home, &[]).unwrap());
        set_routes(&w.host, vec![Route { account: "bo".to_owned(), endpoint: BO_HOME.to_owned(), group_ids: vec![GROUP.to_vec()] }]).unwrap();
        take_(&w.host, last, device(&w.ann, "c3")).unwrap();
        w.host.is_home = false;
        assert!(follow(&w.host, &Placed::Away(Some(later.to_owned())), &[]).unwrap());
        assert_eq!(outbox(&w.host, later, 10).unwrap().iter().filter(|out| out.handover.is_some()).count(), 1);
        w.host
            .run(|step| {
                step.tx.execute("UPDATE handover SET until = ?1", [step.now - 1])?;
                Ok(step.tx.execute("UPDATE pass_on SET until = ?1", [step.now - 1])?)
            })
            .unwrap();
        assert!(outbox(&w.host, later, 10).unwrap().is_empty());
        assert!(routes(&w.host).unwrap().is_empty());
        sweep(&w.host).unwrap();
        assert_eq!(w.host.run(|step| Ok(step.tx.query_row("SELECT COUNT(*) FROM handover", [], |row| row.get::<_, i64>(0))?)).unwrap(), 0);
    }

    /// Zucchero's row 612: a Claktory that isn't the home links to nothing
    /// but the home, whatever routes a device gives it (friends' homes take
    /// only the person's home), and they're there again once it's the home.
    #[test]
    fn away_from_the_home_it_links_only_to_the_home() {
        let mut w = world();
        let home = "a1a1";
        let reply = w.ann.say(&mut w.ann_group, "hi Bo");
        assert!(follow(&w.host, &Placed::Home, &[]).unwrap());
        take_(&w.host, reply, device(&w.ann, "c1")).unwrap();
        w.host.is_home = false;
        assert!(follow(&w.host, &Placed::Away(Some(home.to_owned())), &[]).unwrap());
        let bo = || vec![Route { account: "bo".to_owned(), endpoint: BO_HOME.to_owned(), group_ids: vec![GROUP.to_vec()] }];
        set_routes(&w.host, bo()).unwrap();
        let linked: Vec<String> = routes(&w.host).unwrap().into_iter().map(|route| route.endpoint).collect();
        assert_eq!(linked, [home]);
        assert!(follow(&w.host, &Placed::Away(None), &[]).unwrap());
        assert!(routes(&w.host).unwrap().is_empty(), "no home named: no links");
        w.host.is_home = true;
        assert!(follow(&w.host, &Placed::Home, &[]).unwrap());
        set_routes(&w.host, bo()).unwrap();
        assert!(routes(&w.host).unwrap().iter().any(|route| route.endpoint == BO_HOME));
    }

    /// Row 595 (Fugo 41473): a friend's new home, proven by their signed
    /// list, takes the groups routed to that friend here, and what waited
    /// for the old one; nothing else moves, an older list never moves it
    /// back, and the person's device's routes still replace it all.
    #[test]
    fn a_friends_proven_new_home_takes_only_their_groups() {
        let mut w = world();
        const NEW: &str = "b1b1";
        const OTHER_GROUP: [u8; 16] = [6; 16];
        let both = || {
            vec![
                Route { account: "bo".to_owned(), endpoint: BO_HOME.to_owned(), group_ids: vec![GROUP.to_vec()] },
                Route { account: "cy".to_owned(), endpoint: "c0c0".to_owned(), group_ids: vec![OTHER_GROUP.to_vec()] },
            ]
        };
        set_routes(&w.host, both()).unwrap();
        let reply = w.ann.say(&mut w.ann_group, "hi Bo");
        take_(&w.host, reply.clone(), device(&w.ann, "c1")).unwrap();
        assert_eq!(outbox(&w.host, BO_HOME, 10).unwrap().len(), 1);

        assert_eq!(friend_moved(&w.host, "bo", NEW, 5).unwrap(), Ok(vec![BO_HOME.to_owned()]));
        let mut now = routes(&w.host).unwrap();
        now.sort_by(|a, b| a.account.cmp(&b.account));
        assert_eq!(
            now,
            vec![
                Route { account: "bo".to_owned(), endpoint: NEW.to_owned(), group_ids: vec![GROUP.to_vec()] },
                Route { account: "cy".to_owned(), endpoint: "c0c0".to_owned(), group_ids: vec![OTHER_GROUP.to_vec()] },
            ],
            "only Bo's group moved"
        );
        assert!(outbox(&w.host, BO_HOME, 10).unwrap().is_empty());
        assert_eq!(outbox(&w.host, NEW, 10).unwrap().iter().map(|out| out.envelope.clone()).collect::<Vec<_>>(), vec![reply]);
        assert_eq!(friend_floor(&w.host, "bo").unwrap(), 5);

        // An older list, the person's own account, or one routed nowhere.
        assert_eq!(friend_moved(&w.host, "bo", BO_HOME, 4).unwrap(), Err(NotMoved::Older));
        assert_eq!(friend_moved(&w.host, "ann", NEW, 9).unwrap(), Err(NotMoved::Own));
        assert_eq!(friend_moved(&w.host, "dee", NEW, 9).unwrap(), Err(NotMoved::NotRouted));
        assert_eq!(routes(&w.host).unwrap().iter().find(|route| route.account == "bo").unwrap().endpoint, NEW);

        // The person's device gives routes again: they're what stands.
        set_routes(&w.host, both()).unwrap();
        assert_eq!(routes(&w.host).unwrap().iter().find(|route| route.account == "bo").unwrap().endpoint, BO_HOME);
    }

    /// Row 574: a device's place is in the numbering of the home it read.
    /// A Claktory that becomes the home numbers past every seq a home gave
    /// before, so a place from the old home (past this one's old seqs) still
    /// reads what comes here next: in the groups it knew, in ones it first
    /// hears of later, and each time it becomes the home again.
    #[test]
    fn a_claktory_that_becomes_the_home_numbers_past_every_seq_a_home_gave_before() {
        let mut w = world();
        let bo = || vec![Route { account: "bo".to_owned(), endpoint: BO_HOME.to_owned(), group_ids: vec![GROUP.to_vec()] }];
        // Ann's phone read another home's log well past this one's seqs.
        let elsewhere = FIRST + 50_000;
        let before = now_ms();
        w.host.is_home = false;
        assert!(follow(&w.host, &Placed::Away(Some("a1a1".to_owned())), &[]).unwrap());
        w.host.is_home = true;
        assert!(follow(&w.host, &Placed::Home, &[]).unwrap());
        set_routes(&w.host, bo()).unwrap();
        let hello = w.bo.say(&mut w.bo_group, "hello Ann");
        let Taken::Stored { seq, .. } = take_(&w.host, hello.clone(), from_bo(&hello)).unwrap() else { panic!("not stored") };
        assert!(seq >= seq_base(before) && seq > elsewhere, "{seq}");
        let read = page(&w.host, "ann-phone", &GROUP, elsewhere, 50).unwrap().unwrap();
        assert_eq!(read.entries.iter().map(|entry| entry.seq).collect::<Vec<_>>(), vec![seq], "read from the old home's place");

        // A group it first hears of now starts there too.
        let other = w.host.run(|step| Ok(step.group_or_new(&[7; 16])?.last_seq)).unwrap();
        assert!(other >= seq_base(before) - 1, "{other}");

        // The home again later: past what it gave the time before.
        let later = now_ms();
        w.host.is_home = false;
        assert!(follow(&w.host, &Placed::Away(Some("a1a1".to_owned())), &[]).unwrap());
        w.host.is_home = true;
        assert!(follow(&w.host, &Placed::Home, &[]).unwrap());
        set_routes(&w.host, bo()).unwrap();
        let again = w.bo.say(&mut w.bo_group, "again");
        let Taken::Stored { seq: next, .. } = take_(&w.host, again.clone(), from_bo(&again)).unwrap() else { panic!("not stored") };
        assert!(next > seq && next >= seq_base(later), "{next} after {seq}");

        // When it last became the home (DmHome.home_since_ms), and for one
        // that became it before that was kept, where its seqs start says.
        let since = home_since(&w.host).unwrap();
        assert!(since >= later && since <= now_ms(), "{since} from {later}");
        w.host.run(|step| Ok(step.tx.execute("DELETE FROM meta WHERE name = 'home_since'", [])?)).unwrap();
        assert_eq!(home_since(&w.host).unwrap(), since, "from seq_base");
    }

    #[test]
    fn a_home_again_within_the_week_sends_what_it_had_handed_to_its_friends() {
        let mut w = world();
        let bo = || vec![Route { account: "bo".to_owned(), endpoint: BO_HOME.to_owned(), group_ids: vec![GROUP.to_vec()] }];
        assert!(follow(&w.host, &Placed::Home, &[]).unwrap());
        let reply = w.ann.say(&mut w.ann_group, "hi Bo");
        take_(&w.host, reply.clone(), device(&w.ann, "c1")).unwrap();
        w.host.is_home = false;
        assert!(follow(&w.host, &Placed::Away(Some("a1a1".to_owned())), &[]).unwrap());
        assert_eq!(outbox(&w.host, "a1a1", 10).unwrap().len(), 1);

        // The home again: it waits for the device's routes, then goes to Bo's
        // home with its receipt.
        w.host.is_home = true;
        assert!(follow(&w.host, &Placed::Home, &[]).unwrap());
        assert!(outbox_counts(&w.host).unwrap().is_empty());
        set_routes(&w.host, bo()).unwrap();
        let queued = outbox(&w.host, BO_HOME, 10).unwrap();
        assert_eq!(queued.len(), 1);
        assert_eq!((queued[0].envelope.clone(), queued[0].handover.clone()), (reply, None));
        w.host.arrived();
        sent(&w.host, BO_HOME, &[queued[0].message_id.clone()]).unwrap();
        assert!(matches!(&w.host.arrived()[..], [Arrival::Delivered { client_message_id, endpoint, .. }] if client_message_id == "c1" && endpoint == BO_HOME));

        // Routes that no longer reach Bo for this group: it goes, never to
        // another account's home.
        let again = w.ann.say(&mut w.ann_group, "again");
        take_(&w.host, again, device(&w.ann, "c2")).unwrap();
        w.host.is_home = false;
        assert!(follow(&w.host, &Placed::Away(Some("a1a1".to_owned())), &[]).unwrap());
        w.host.is_home = true;
        assert!(follow(&w.host, &Placed::Home, &[]).unwrap());
        set_routes(&w.host, vec![Route { account: "cy".to_owned(), endpoint: "c0c0".to_owned(), group_ids: vec![GROUP.to_vec()] }]).unwrap();
        assert!(outbox_counts(&w.host).unwrap().is_empty());
        assert_eq!(w.host.run(|step| Ok(step.tx.query_row("SELECT COUNT(*) FROM handover", [], |row| row.get::<_, i64>(0))?)).unwrap(), 0);
    }

    #[test]
    fn the_new_home_takes_a_hand_over_only_from_the_persons_bound_claktory() {
        let mut w = world();
        const OLD_HOME: &str = "a0a0";
        let reply = w.ann.say(&mut w.ann_group, "hi Bo");
        let id = hex(&claktory_dm_crypto::envelope::digest(&reply));
        let handed = |endpoint: &str, leaf: &str| Source::HandOver {
            endpoint: endpoint.to_owned(),
            message_id: id.clone(),
            leaf_id: leaf.to_owned(),
            client_message_id: "c1".to_owned(),
        };
        // Not bound (dropped from the list, or never on it): refused.
        assert_eq!(take_(&w.host, reply.clone(), handed(OLD_HOME, "ann-phone")), Err(Refusal::NotRouted));
        assert!(bind(&w.host, "ann-mac", OLD_HOME, 10).unwrap());
        // Only for a leaf of the person's in the group now.
        assert_eq!(take_(&w.host, reply.clone(), handed(OLD_HOME, "ann-tablet")), Err(Refusal::NotRouted));
        let wrong_id = Source::HandOver { endpoint: OLD_HOME.to_owned(), message_id: "00".to_owned(), leaf_id: "ann-phone".to_owned(), client_message_id: "c1".to_owned() };
        assert!(matches!(take_(&w.host, reply.clone(), wrong_id), Err(Refusal::Invalid(_))));
        // Not the home: refused.
        w.host.is_home = false;
        assert_eq!(take_(&w.host, reply.clone(), handed(OLD_HOME, "ann-phone")), Err(Refusal::NotHome));
        w.host.is_home = true;

        // As the phone's own: logged once, queued for Bo's home with its
        // receipt.
        let taken = take_(&w.host, reply.clone(), handed(OLD_HOME, "ann-phone")).unwrap();
        assert!(matches!(taken, Taken::Stored { seq, duplicate: false, .. } if seq == FIRST + 1), "{taken:?}");
        assert!(matches!(take_(&w.host, reply.clone(), handed(OLD_HOME, "ann-phone")), Ok(Taken::Stored { duplicate: true, .. })));
        let read = page(&w.host, "ann-phone", &GROUP, FIRST, 50).unwrap().unwrap();
        assert_eq!((read.entries.len(), read.entries[0].from_me, read.entries[0].client_message_id.as_deref()), (1, true, Some("c1")));
        let queued = outbox(&w.host, BO_HOME, 10).unwrap();
        assert_eq!(queued.len(), 1);
        w.host.arrived();
        sent(&w.host, BO_HOME, &[queued[0].message_id.clone()]).unwrap();
        assert!(matches!(&w.host.arrived()[..], [Arrival::Delivered { leaf_id, client_message_id, .. }] if leaf_id == "ann-phone" && client_message_id == "c1"));
        // Unbound again (off the list): refused at the next take.
        unbind_missing(&w.host, &[]).unwrap();
        let more = w.ann.say(&mut w.ann_group, "more");
        let more_id = hex(&claktory_dm_crypto::envelope::digest(&more));
        let source = Source::HandOver { endpoint: OLD_HOME.to_owned(), message_id: more_id, leaf_id: "ann-phone".to_owned(), client_message_id: "c2".to_owned() };
        assert_eq!(take_(&w.host, more, source), Err(Refusal::NotRouted));
    }

    /// What the person's former home passes on (Zucchero's row 567, Fugo
    /// 39876): only from their Claktory bound here, only an application
    /// message, only while this is the home, for a group the person is in;
    /// for their leaves only, never queued for a friend's home, and never
    /// passed on again.
    #[test]
    fn the_new_home_takes_what_is_passed_on_only_from_the_persons_bound_claktory() {
        const OLD_HOME: &str = "a0a0";
        let mut w = world();
        let passed = |endpoint: &str, envelope: &[u8]| Source::PassOn {
            endpoint: endpoint.to_owned(),
            message_id: hex(&claktory_dm_crypto::envelope::digest(envelope)),
            account: "bo".to_owned(),
        };
        let hello = w.bo.say(&mut w.bo_group, "hello Ann");
        // Not bound (dropped from the list, or never on it): refused.
        assert_eq!(take_(&w.host, hello.clone(), passed(OLD_HOME, &hello)), Err(Refusal::NotRouted));
        assert!(bind(&w.host, "ann-mac", OLD_HOME, 10).unwrap());
        // Ann's own, sealed before her commit below (row 591).
        let mine = w.ann.say(&mut w.ann_group, "sent while the iPad was off");
        // Not an application message, or not under its own id: refused.
        let commit = w.ann.update(&mut w.ann_group);
        assert!(matches!(take_(&w.host, commit.clone(), passed(OLD_HOME, &commit)), Err(Refusal::Invalid(_))));
        let wrong_id = Source::PassOn { endpoint: OLD_HOME.to_owned(), message_id: "00".to_owned(), account: "bo".to_owned() };
        assert!(matches!(take_(&w.host, hello.clone(), wrong_id), Err(Refusal::Invalid(_))));
        // A commit under the person's own account too.
        let as_ann = |envelope: &[u8]| Source::PassOn {
            endpoint: OLD_HOME.to_owned(),
            message_id: hex(&claktory_dm_crypto::envelope::digest(envelope)),
            account: "ann".to_owned(),
        };
        assert!(matches!(take_(&w.host, commit.clone(), as_ann(&commit)), Err(Refusal::Invalid(_))));
        // Not the home: refused.
        w.host.is_home = false;
        assert_eq!(take_(&w.host, hello.clone(), passed(OLD_HOME, &hello)), Err(Refusal::NotHome));
        w.host.is_home = true;
        // A group a home has no state of yet: it asks claktory.com for it,
        // and the former home tries again.
        let fresh = TestHost::new("ann");
        assert!(bind(&fresh, "ann-mac", OLD_HOME, 10).unwrap());
        assert_eq!(take_(&fresh, hello.clone(), passed(OLD_HOME, &hello)), Err(Refusal::Unready(STATELESS.to_owned())));
        assert_eq!(wanted(&fresh).unwrap(), vec![Want { group_id: GROUP.to_vec(), group_info: true, after_epoch: None }]);

        // Taken: logged for Ann's leaves, queued for no one, once.
        w.host.arrived();
        let taken = take_(&w.host, hello.clone(), passed(OLD_HOME, &hello)).unwrap();
        assert!(matches!(taken, Taken::Stored { duplicate: false, .. }), "{taken:?}");
        assert!(matches!(take_(&w.host, hello.clone(), passed(OLD_HOME, &hello)), Ok(Taken::Stored { duplicate: true, .. })));
        assert!(outbox_counts(&w.host).unwrap().is_empty(), "never to a friend's home");
        let read = page(&w.host, "ann-phone", &GROUP, FIRST, 50).unwrap().unwrap();
        assert_eq!(read.entries.iter().filter(|entry| entry.envelope == hello).count(), 1);
        assert!(!read.entries.iter().find(|entry| entry.envelope == hello).unwrap().from_me);

        // The person's own (row 591, Fugo 41196): taken, but as a friend's
        // passed message would be, whatever the hint says: not from_me, no
        // client id, queued for no one, within the account's cap, once.
        set_test_caps(Some((mine.len() as i64 - 1, 1 << 30)));
        assert_eq!(take_(&w.host, mine.clone(), as_ann(&mine)), Err(Refusal::Full { retry_after_ms: FULL_RETRY_MS }), "within the cap");
        set_test_caps(None);
        let taken = take_(&w.host, mine.clone(), as_ann(&mine)).unwrap();
        assert!(matches!(taken, Taken::Stored { duplicate: false, .. }), "{taken:?}");
        assert!(matches!(take_(&w.host, mine.clone(), as_ann(&mine)), Ok(Taken::Stored { duplicate: true, .. })));
        assert!(outbox_counts(&w.host).unwrap().is_empty(), "never to Bo's home");
        let read = page(&w.host, "ann-phone", &GROUP, FIRST, 50).unwrap().unwrap();
        let entry = read.entries.iter().find(|entry| entry.envelope == mine).expect("served");
        assert_eq!((entry.from_me, entry.client_message_id.as_deref()), (false, None));
        let counted: i64 = w
            .host
            .run(|step| Ok(step.tx.query_row("SELECT COUNT(*) FROM log WHERE account = 'ann' AND mine = 0", [], |row| row.get(0))?))
            .unwrap();
        assert_eq!(counted, 1, "in the account's cap");
        assert!(!held_bytes(&w.host).unwrap().contains_key("ann"), "not shown as a friend's");

        // This home moves on before Ann read them: never passed on again.
        w.host.is_home = false;
        assert!(follow(&w.host, &Placed::Away(Some("a2a2".to_owned())), &[]).unwrap());
        assert!(outbox(&w.host, "a2a2", 10).unwrap().iter().all(|out| out.envelope != hello && out.envelope != mine));
    }

    /// A home that stops being it passes on what one of the person's
    /// leaves hadn't read (Zucchero's rows 567 and 591): a friend's message
    /// or the person's own, not what every leaf read, never a commit; done
    /// with once the new home took it, and let go if it's the home again
    /// first.
    #[test]
    fn a_home_that_stops_being_it_passes_on_what_the_persons_leaves_hadnt_read() {
        let mut w = world();
        let new_home = "a1a1";
        assert!(follow(&w.host, &Placed::Home, &[]).unwrap());
        let read = w.bo.say(&mut w.bo_group, "read");
        let Ok(Taken::Stored { seq, .. }) = take_(&w.host, read.clone(), from_bo(&read)) else { panic!("not taken") };
        ack(&w.host, "ann-phone", &GROUP, seq).unwrap();
        let unread = w.bo.say(&mut w.bo_group, "unread");
        take_(&w.host, unread.clone(), from_bo(&unread)).unwrap();
        // Ann's own, which Bo's home took, from a device: her other devices
        // haven't read it either (row 591).
        set_routes(&w.host, vec![Route { account: "bo".to_owned(), endpoint: BO_HOME.to_owned(), group_ids: vec![GROUP.to_vec()] }])
            .unwrap();
        let own = w.ann.say(&mut w.ann_group, "own");
        take_(&w.host, own.clone(), device(&w.ann, "o1")).unwrap();
        let to_bo = outbox(&w.host, BO_HOME, 10).unwrap();
        sent(&w.host, BO_HOME, &to_bo.iter().map(|out| out.message_id.clone()).collect::<Vec<_>>()).unwrap();

        w.host.is_home = false;
        assert!(follow(&w.host, &Placed::Away(Some(new_home.to_owned())), &[]).unwrap());
        assert_eq!(
            routes(&w.host).unwrap(),
            vec![Route { account: "ann".to_owned(), endpoint: new_home.to_owned(), group_ids: vec![GROUP.to_vec()] }]
        );
        let mut passed = outbox(&w.host, new_home, 10).unwrap();
        passed.sort_by_key(|out| out.pass_on.clone());
        assert_eq!(passed.len(), 2, "{passed:?}");
        let shown: Vec<_> = passed.iter().map(|out| (out.envelope.clone(), out.pass_on.clone(), out.handover.clone())).collect();
        assert_eq!(shown, vec![(own, Some("ann".to_owned()), None), (unread, Some("bo".to_owned()), None)]);
        // Taken there: done with, and no route left.
        sent(&w.host, new_home, &passed.iter().map(|out| out.message_id.clone()).collect::<Vec<_>>()).unwrap();
        assert!(outbox(&w.host, new_home, 10).unwrap().is_empty());
        assert!(routes(&w.host).unwrap().is_empty());

        // The home again before it was taken: it serves it itself.
        let more = w.bo.say(&mut w.bo_group, "more");
        w.host.is_home = true;
        assert!(follow(&w.host, &Placed::Home, &[]).unwrap());
        set_routes(&w.host, vec![Route { account: "bo".to_owned(), endpoint: BO_HOME.to_owned(), group_ids: vec![GROUP.to_vec()] }])
            .unwrap();
        take_(&w.host, more.clone(), from_bo(&more)).unwrap();
        w.host.is_home = false;
        assert!(follow(&w.host, &Placed::Away(Some(new_home.to_owned())), &[]).unwrap());
        assert!(outbox(&w.host, new_home, 10).unwrap().iter().any(|out| out.envelope == more));
        w.host.is_home = true;
        assert!(follow(&w.host, &Placed::Home, &[]).unwrap());
        let left: i64 = w.host.run(|step| Ok(step.tx.query_row("SELECT COUNT(*) FROM pass_on", [], |row| row.get(0))?)).unwrap();
        assert_eq!(left, 0);
    }

    #[test]
    fn a_hint_marks_the_group_for_the_pull() {
        let w = world();
        hint(&w.host, &GROUP, 0).unwrap();
        assert!(wanted(&w.host).unwrap().is_empty(), "a commit the home already has");
        hint(&w.host, &GROUP, 1).unwrap();
        assert_eq!(wanted(&w.host).unwrap(), vec![Want { group_id: GROUP.to_vec(), group_info: false, after_epoch: Some(1) }]);
    }

    /// Row 570: a Claktory that becomes the home looks at claktory.com for
    /// commits made while another was the home, in every group it knows.
    #[test]
    fn a_claktory_that_becomes_the_home_looks_for_commits_it_never_carried() {
        let mut w = world();
        w.host.is_home = false;
        assert!(follow(&w.host, &Placed::Away(Some("a1a1".to_owned())), &[]).unwrap());
        assert!(wanted(&w.host).unwrap().is_empty());
        w.host.is_home = true;
        assert!(follow(&w.host, &Placed::Home, &[]).unwrap());
        let wants = wanted(&w.host).unwrap();
        assert_eq!(wants.len(), 1, "{wants:?}");
        assert_eq!((&wants[0].group_id, wants[0].group_info), (&GROUP.to_vec(), false));
        assert!(wants[0].after_epoch.is_some());
        // Nothing there: the mark is spent, and staying the home sets none.
        caught_up(&w.host, &GROUP).unwrap();
        assert!(!follow(&w.host, &Placed::Home, &[]).unwrap());
        assert!(wanted(&w.host).unwrap().is_empty());
    }

    #[test]
    fn another_claktory_bound_here_never_sends_in_a_conversation() {
        let mut w = world();
        let mac_id = IdentityKey::generate().unwrap();
        // Its certificate's identity doesn't matter here: the home reads
        // only who its leaf is from the binding.
        let mac = Device::of_kind(&mac_id, "ann", "mac#1", LeafKind::Claktory);
        let (commit, welcome, _) = w.ann_group.add_members(&w.ann.provider(), &w.ann.leaf, &[mac.key_package()]).unwrap();
        w.ann_group.merge_pending_commit(&w.ann.provider()).unwrap();
        assert!(matches!(take_(&w.host, commit.to_bytes().unwrap(), device(&w.ann, "")), Ok(Taken::Stored { .. })));
        let mut mac_group = mac.join(welcome);
        assert!(bind(&w.host, "mac#1", "a11ce0", 1).unwrap());
        let said = mac.say(&mut mac_group, "an agent's words");
        let source = Source::Home { endpoint: "a11ce0".to_owned(), message_id: hex(&claktory_dm_crypto::envelope::digest(&said)) };
        assert!(matches!(take_(&w.host, said, source), Err(Refusal::Invalid(_))));
    }

    #[test]
    fn a_leaf_taken_out_reads_on_to_the_commit_that_took_it_out_and_no_further() {
        let mut w = world();
        let tablet = Device::new(&IdentityKey::generate().unwrap(), "ann", "ann-tablet");
        let (commit, _, _) = w.ann_group.add_members(&w.ann.provider(), &w.ann.leaf, &[tablet.key_package()]).unwrap();
        w.ann_group.merge_pending_commit(&w.ann.provider()).unwrap();
        let Ok(Taken::Stored { seq: added, .. }) = take_(&w.host, commit.to_bytes().unwrap(), device(&w.ann, "")) else { panic!("not added") };
        let before = w.ann.say(&mut w.ann_group, "before");
        take_(&w.host, before, device(&w.ann, "b1")).unwrap();
        let index = w.ann_group.members().find(|member| member.credential == tablet.credential.credential).unwrap().index;
        let (commit, _, _) = w.ann_group.remove_members(&w.ann.provider(), &w.ann.leaf, &[index]).unwrap();
        w.ann_group.merge_pending_commit(&w.ann.provider()).unwrap();
        let Ok(Taken::Stored { seq: removed, .. }) = take_(&w.host, commit.to_bytes().unwrap(), device(&w.ann, "")) else { panic!("not removed") };
        let after = w.ann.say(&mut w.ann_group, "after");
        take_(&w.host, after, device(&w.ann, "a1")).unwrap();

        // It reads to the commit that took it out, so it learns it's out.
        assert_eq!(place(&w.host, "ann-tablet", &GROUP).unwrap(), Some(added));
        let read = page(&w.host, "ann-tablet", &GROUP, added, 50).unwrap().unwrap();
        assert_eq!(read.entries.iter().map(|entry| entry.seq).collect::<Vec<_>>(), vec![added + 1, removed]);
        assert!(!read.has_more);
        assert!(groups(&w.host, "ann-tablet").unwrap().is_empty(), "not in it any more");
        // Added again: it reads on from the commit that added it.
        let again = Device::new(&IdentityKey::generate().unwrap(), "ann", "ann-tablet");
        let (commit, _, _) = w.ann_group.add_members(&w.ann.provider(), &w.ann.leaf, &[again.key_package()]).unwrap();
        w.ann_group.merge_pending_commit(&w.ann.provider()).unwrap();
        let Ok(Taken::Stored { seq: back, .. }) = take_(&w.host, commit.to_bytes().unwrap(), device(&w.ann, "")) else { panic!("not added") };
        assert_eq!(place(&w.host, "ann-tablet", &GROUP).unwrap(), Some(back));
        assert_eq!(groups(&w.host, "ann-tablet").unwrap().len(), 1);
    }
}
