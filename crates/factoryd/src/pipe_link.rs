//! Pipes between two Claktories, the transport (`claktory-pipe/1`): invite
//! codes, the handshake, pinning the peer's key, the four digits, signed
//! messages with acks, de-duplication, the offline queue with retries, and
//! close. What a pipe is, who may use it and what may go through it is the
//! store's (`crate::pipes`): this only carries what the store hands it, to
//! the one Claktory pinned to the pipe, and hands back what arrives.
//!
//! The store works with its own pipe ids; the transport keeps, per pipe, the
//! wire id both sides share (chosen by the inviter), the peer's key and
//! addresses, the queue and what it has seen (factory.db: `pipe_links`,
//! `pipe_outbox`, `pipe_seen`).
//!
//! The store hears back through `Store`, which it registers once with
//! `set_store`; until it does, nothing arrives.

use std::collections::{HashMap, HashSet};
use std::net::{IpAddr, SocketAddr};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicI64, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock, Weak};
use std::time::Duration;

use iroh::endpoint::{Connection, RecvStream, SendStream};
use iroh::{EndpointAddr, EndpointId, RelayUrl, SecretKey, Signature};
use rusqlite::{OptionalExtension, params};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use crate::encoding::{hex, same, unhex};
use crate::factory::Factory;
use crate::transcript::now_ms;

/// The pipes' protocol on the tunnel's endpoint, apart from the devices'.
pub const ALPN_PIPE: &[u8] = b"claktory-pipe/1";

/// How long an invite code works.
pub const INVITE_LIFETIME: std::time::Duration = std::time::Duration::from_secs(10 * 60);
/// At most this much text in one message, in bytes.
pub const MAX_TEXT: usize = 16 * 1024;
/// At most this many bytes of files in one message, all together.
pub const MAX_FILE: u64 = 10 * 1024 * 1024;
/// At most this many files in one message.
pub const MAX_FILES: usize = 20;

/// One side of a pipe, as the other side sees it: exchanged in the
/// handshake, and the inviter's carried in the code too. Owner is what that
/// side says; nothing vouches for it yet ("not verified").
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Side {
    /// Its Claktory's name.
    pub claktory: String,
    /// Its person's name (Settings); empty when it isn't set.
    pub owner_name: String,
    /// Its account's email; empty when it isn't signed in.
    pub owner_email: String,
    /// The project at its end.
    pub project: String,
    /// What it calls the pipe.
    pub pipe_name: String,
    /// One line on what it's for, as that side's person wrote it.
    pub purpose: String,
}

/// The Claktory at the other end, pinned to the pipe.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Peer {
    /// Its tunnel key (iroh endpoint id), lowercase hex.
    pub endpoint_id: String,
    pub side: Side,
    /// An account service vouched for its owner. Always false for now.
    pub verified: bool,
}

/// Who on the sending side wrote a message, as that side says.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Via {
    Person { name: String },
    Agent { name: String, role: String },
}

/// A file to send: its name as the peer sees it, and where it is now. Its
/// bytes are copied into the queue when the message is sent.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OutFile {
    pub name: String,
    pub path: PathBuf,
}

/// A message for the peer, checked and approved by the store already.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Outgoing {
    /// The store's, unique within the pipe; the peer de-duplicates on it.
    pub msg_id: String,
    pub via: Via,
    pub text: String,
    pub files: Vec<OutFile>,
}

/// A file that came with a message: its name as the peer gave it (not
/// sanitised) and a temporary file holding it, under
/// `<home>/pipes/incoming/`, for the store to move; what's left there a day
/// later is deleted.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InFile {
    pub name: String,
    pub path: PathBuf,
}

/// A message from the peer: its signature checked, seen for the first time.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Incoming {
    pub msg_id: String,
    /// Its place in the peer's order, from 1.
    pub seq: u64,
    pub via: Via,
    pub text: String,
    pub files: Vec<InFile>,
    /// When the peer sent it, by its clock, in ms since 1970.
    pub sent_at_ms: i64,
}

/// Whether the peer can be reached, as last seen. Cached: cheap to read.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LinkStatus {
    pub online: bool,
    /// When it was last reached, in ms since 1970; 0 if never.
    pub last_seen_ms: i64,
    /// Messages waiting to reach it.
    pub queued: u32,
    /// Why it can't be reached, in a short line; empty while online, and
    /// before anything has tried.
    pub reason: String,
    /// When reaching it started failing, in ms since 1970; 0 while online.
    pub offline_since_ms: i64,
}

/// Why the transport can't do what the store asked. Each says it so the app
/// can show it.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum PipeError {
    #[error("This code has expired. Ask for a new one.")]
    Expired,
    #[error("This code has been used already. Ask for a new one.")]
    Used,
    #[error("That isn't a pipe code.")]
    BadCode,
    #[error("This pipe is connected here already.")]
    Exists,
    #[error("No such pipe.")]
    NotFound,
    #[error("Can't reach the other Claktory: {0}")]
    Unreachable(String),
    #[error("{0}")]
    TooLarge(String),
    #[error("This pipe isn't open yet.")]
    NotOpen,
    #[error("This pipe is closed.")]
    Closed,
    #[error("{0}")]
    Other(String),
}

/// The transport's tables in factory.db (a migration in `store`).
pub(crate) const SCHEMA: &str = SCHEMA_SQL;

/// What the transport tells the store. Each is called from the transport's
/// own tasks, never with a lock of its held; each takes the store's pipe
/// id. Keep them quick.
pub trait Store: Send + Sync {
    /// The inviter's side: a Claktory used this pipe's code and passed the
    /// handshake. Both people now compare `digits`.
    fn knocked(&self, factory: &Arc<Factory>, pipe: &str, peer: Peer, digits: String);
    /// The peer's person approved. The pipe is open once this side's has too
    /// (`approve`); only then do messages go either way.
    fn peer_approved(&self, factory: &Arc<Factory>, pipe: &str);
    /// A message arrived: called once per msg_id, in the peer's order, and
    /// acked once it returns Ok (stored; held for a limit counts). Err
    /// refuses it, and the peer hears the reason (`refused`).
    fn received(&self, factory: &Arc<Factory>, pipe: &str, message: Incoming) -> Result<(), String>;
    /// The peer stored a message of this side's.
    fn delivered(&self, factory: &Arc<Factory>, pipe: &str, msg_id: &str);
    /// The peer refused a message of this side's.
    fn refused(&self, factory: &Arc<Factory>, pipe: &str, msg_id: &str, reason: &str);
    fn peer_paused(&self, factory: &Arc<Factory>, pipe: &str);
    fn peer_resumed(&self, factory: &Arc<Factory>, pipe: &str);
    /// The peer closed the pipe (or turned it away before approving): it's
    /// unpinned and its queue dropped already.
    fn peer_closed(&self, factory: &Arc<Factory>, pipe: &str);
    /// `status` changed: online, last seen, or the queue.
    fn link_changed(&self, factory: &Arc<Factory>, pipe: &str, status: LinkStatus);
    /// What the peer (pinned to `endpoint`) presents as who it is, on a ping
    /// or its answer: the account service's statement, "" when it says it
    /// has none (signed out); never called for a peer that sends nothing (a
    /// Claktory from before). Unchecked: the store checks it.
    fn identity(&self, factory: &Arc<Factory>, pipe: &str, endpoint: &str, identity: &str);
    /// What this side presents as who it is (as `identity` takes it); None
    /// to send nothing.
    fn own_identity(&self, factory: &Arc<Factory>) -> Option<String>;
    /// Whether agents' messages wait for now (the peer is another account
    /// than the person accepted): queued ones stay, in order, with what's
    /// behind them, until it says otherwise and `wake`s the link.
    fn holds_agents(&self, factory: &Arc<Factory>, pipe: &str) -> bool;
}

const SCHEMA_SQL: &str = "
-- Pipes between Claktories, their transport (pipe_link.rs). Each pipe's
-- link: the id both sides share (wire), its invite's secret while it waits
-- (only its SHA-256), this side as the peer sees it (mine, JSON), the peer's
-- pinned key and how to reach it, who approved and paused, and where each
-- side's numbering stands.
CREATE TABLE pipe_links (
    pipe TEXT PRIMARY KEY,
    wire TEXT NOT NULL UNIQUE,
    inviter INTEGER NOT NULL,
    secret_sha256 TEXT NOT NULL DEFAULT '',
    invite_expires INTEGER NOT NULL DEFAULT 0,
    mine TEXT NOT NULL,
    peer TEXT NOT NULL DEFAULT '',
    peer_side TEXT NOT NULL DEFAULT '',
    relays TEXT NOT NULL DEFAULT '[]',
    addresses TEXT NOT NULL DEFAULT '[]',
    approved INTEGER NOT NULL DEFAULT 0,
    peer_approved INTEGER NOT NULL DEFAULT 0,
    paused INTEGER NOT NULL DEFAULT 0,
    peer_paused INTEGER NOT NULL DEFAULT 0,
    next_seq INTEGER NOT NULL DEFAULT 1,
    next_cseq INTEGER NOT NULL DEFAULT 1,
    received_seq INTEGER NOT NULL DEFAULT 0,
    received_cseq INTEGER NOT NULL DEFAULT 0,
    last_seen INTEGER NOT NULL DEFAULT 0,
    closed INTEGER NOT NULL DEFAULT 0,
    closed_at INTEGER NOT NULL DEFAULT 0
);
-- What waits to reach each pipe's peer, in order: messages, and approve,
-- pause, resume and close (control), each a signed frame.
CREATE TABLE pipe_outbox (
    n INTEGER PRIMARY KEY AUTOINCREMENT,
    pipe TEXT NOT NULL,
    control INTEGER NOT NULL,
    msg_id TEXT NOT NULL DEFAULT '',
    frame BLOB NOT NULL
);
CREATE INDEX pipe_outbox_pipe ON pipe_outbox (pipe, n);
";

/// Which queued messages an agent wrote, which wait while the peer is
/// another account than the person accepted (`Store::holds_agents`).
pub(crate) const OUTBOX_AGENT_SQL: &str = "
ALTER TABLE pipe_outbox ADD COLUMN agent INTEGER NOT NULL DEFAULT 0;
";

static STORE: OnceLock<Box<dyn Store>> = OnceLock::new();

/// Registers the store, once (later calls are ignored).
pub fn set_store(store: Box<dyn Store>) {
    let _ = STORE.set(store);
}

/// The registered store, else Claktory's own (`crate::pipes`): tests
/// register one of their own in place of it.
fn store() -> Option<&'static dyn Store> {
    Some(STORE.get().map_or(crate::pipes::link(), |store| &**store))
}

/// What a signature covers, before the frame's payload.
const LABEL: &[u8] = b"claktory pipe frame v1\n";
/// At most this big, a frame's payload (its text and what describes it).
const MAX_PAYLOAD: usize = 256 * 1024;
/// At most this big, a reply.
const MAX_REPLY: usize = 256 * 1024;
/// How long dialing the peer may take, in ms (15 s; tests shorten it).
static DIAL_MS: AtomicU64 = AtomicU64::new(15_000);

/// Shortens every pipe dial, for every Factory in this process: tests
/// exercising failed dials would otherwise wait the whole 15 s each try.
pub fn set_dial(timeout: Duration) {
    DIAL_MS.store(u64::try_from(timeout.as_millis()).unwrap_or(u64::MAX), Ordering::Relaxed);
}
/// How long the peer may take to answer a frame (its files' time aside).
const ANSWER: Duration = Duration::from_secs(30);
/// How often an idle link checks that its peer is there.
const PING: Duration = Duration::from_secs(20);
/// The longest wait between tries to reach a peer.
const MAX_BACKOFF: Duration = Duration::from_secs(60);
/// How long a closed pipe keeps trying to tell its peer.
const CLOSE_TRIES: i64 = 7 * 24 * 60 * 60 * 1000;
/// Files that came with messages and weren't moved are deleted after this.
const INCOMING_KEPT: Duration = Duration::from_secs(24 * 60 * 60);

/// Who the gate lets onto the pipes' ALPN: the peers pinned to pipes, and
/// any key while an invite waits (it proves the invite's secret first).
#[derive(Debug, Default)]
pub(crate) struct Admits {
    keys: Mutex<HashSet<String>>,
    /// Until when an invite waits, in ms since 1970.
    invite_until: AtomicI64,
}

impl Admits {
    pub(crate) fn admits(&self, key: &str, now: i64) -> bool {
        self.invite_until.load(Ordering::Relaxed) > now || lock(&self.keys).contains(key)
    }

    /// Whether `key` is pinned to a pipe, invite window aside: an unpinned
    /// key is here only to claim a waiting invite, and gets less.
    fn pinned(&self, key: &str) -> bool {
        lock(&self.keys).contains(key)
    }
}

/// What the transport keeps while Factory runs.
#[derive(Default)]
pub(crate) struct State {
    pub(crate) admits: Arc<Admits>,
    /// Links with something to do (open, or a close to tell): remote access
    /// stays on for them.
    active: AtomicUsize,
    /// Joins under way: remote access stays on for them too.
    joining: AtomicUsize,
    /// Their wires: the inviter may already be talking on one before the
    /// join has saved its link here.
    joining_wires: Mutex<HashSet<String>>,
    /// Each pipe's worker, which reaches its peer.
    workers: Mutex<HashMap<String, Arc<Worker>>>,
    /// One frame at a time per pipe from its peer.
    receiving: Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
    key: OnceLock<SecretKey>,
}

impl State {
    /// Whether remote access must stay on for pipes.
    pub(crate) fn wants_tunnel(&self, now: i64) -> bool {
        self.active.load(Ordering::Relaxed) > 0
            || self.joining.load(Ordering::Relaxed) > 0
            || self.admits.invite_until.load(Ordering::Relaxed) > now
    }
}

#[derive(Default)]
struct Worker {
    wake: tokio::sync::Notify,
    status: Mutex<LinkStatus>,
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// One pipe's link to its peer (`pipe_links`).
#[derive(Clone, Debug, Default)]
struct Link {
    pipe: String,
    wire: String,
    inviter: bool,
    secret_sha256: String,
    invite_expires: i64,
    mine: Side,
    peer: String,
    peer_side: Side,
    relays: Vec<String>,
    addresses: Vec<String>,
    approved: bool,
    peer_approved: bool,
    paused: bool,
    peer_paused: bool,
    next_seq: i64,
    next_cseq: i64,
    received_seq: i64,
    received_cseq: i64,
    last_seen: i64,
    closed: bool,
    closed_at: i64,
}

const COLUMNS: &str = "pipe, wire, inviter, secret_sha256, invite_expires, mine, peer, peer_side, relays, addresses, approved, \
     peer_approved, paused, peer_paused, next_seq, next_cseq, received_seq, received_cseq, last_seen, closed, closed_at";

fn json<T: serde::de::DeserializeOwned + Default>(text: &str) -> T {
    serde_json::from_str(text).unwrap_or_default()
}

fn text<T: Serialize>(value: &T) -> String {
    serde_json::to_string(value).unwrap_or_default()
}

impl Link {
    fn read(row: &rusqlite::Row<'_>) -> rusqlite::Result<Link> {
        Ok(Link {
            pipe: row.get(0)?,
            wire: row.get(1)?,
            inviter: row.get(2)?,
            secret_sha256: row.get(3)?,
            invite_expires: row.get(4)?,
            mine: json(&row.get::<_, String>(5)?),
            peer: row.get(6)?,
            peer_side: json(&row.get::<_, String>(7)?),
            relays: json(&row.get::<_, String>(8)?),
            addresses: json(&row.get::<_, String>(9)?),
            approved: row.get(10)?,
            peer_approved: row.get(11)?,
            paused: row.get(12)?,
            peer_paused: row.get(13)?,
            next_seq: row.get(14)?,
            next_cseq: row.get(15)?,
            received_seq: row.get(16)?,
            received_cseq: row.get(17)?,
            last_seen: row.get(18)?,
            closed: row.get(19)?,
            closed_at: row.get(20)?,
        })
    }

    fn save(&self, db: &rusqlite::Connection) -> rusqlite::Result<()> {
        db.execute(
            &format!("INSERT OR REPLACE INTO pipe_links ({COLUMNS}) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21)"),
            params![
                self.pipe,
                self.wire,
                self.inviter,
                self.secret_sha256,
                self.invite_expires,
                text(&self.mine),
                self.peer,
                text(&self.peer_side),
                text(&self.relays),
                text(&self.addresses),
                self.approved,
                self.peer_approved,
                self.paused,
                self.peer_paused,
                self.next_seq,
                self.next_cseq,
                self.received_seq,
                self.received_cseq,
                self.last_seen,
                self.closed,
                self.closed_at,
            ],
        )?;
        Ok(())
    }

    /// Saves only what hearing from the peer changes (`received`): where
    /// it's reached, its approval and pause, how far its frames have come
    /// and when it was seen. The rest of the row is this side's to change
    /// (`control`, `send`) at any moment meanwhile, and a whole-row save
    /// from a copy read earlier would undo it: a ping heard while the
    /// person approved left the link unapproved, so nothing went.
    fn save_received(&self, db: &rusqlite::Connection) -> rusqlite::Result<()> {
        db.execute(
            "UPDATE pipe_links SET relays = ?2, addresses = ?3, peer_approved = ?4, peer_paused = ?5, received_seq = ?6, \
             received_cseq = ?7, last_seen = ?8 WHERE pipe = ?1",
            params![
                self.pipe,
                text(&self.relays),
                text(&self.addresses),
                self.peer_approved,
                self.peer_paused,
                self.received_seq,
                self.received_cseq,
                self.last_seen,
            ],
        )?;
        Ok(())
    }

    /// Both people approved and neither paused: messages go.
    fn flowing(&self) -> bool {
        self.approved && self.peer_approved && !self.paused && !self.peer_paused && !self.closed
    }
}

fn load(factory: &Factory, pipe: &str) -> Option<Link> {
    load_from(&factory.store.db(), pipe)
}

/// The link, read with the database already held: read, changed and saved
/// under one hold, nothing of this side's can come between.
fn load_from(db: &rusqlite::Connection, pipe: &str) -> Option<Link> {
    db.query_row(&format!("SELECT {COLUMNS} FROM pipe_links WHERE pipe = ?1"), [pipe], Link::read).optional().unwrap_or_else(|error| {
        tracing::error!(%error, "could not read a pipe's link");
        None
    })
}

fn by_wire(factory: &Factory, wire: &str) -> Option<Link> {
    let db = factory.store.db();
    db.query_row(&format!("SELECT {COLUMNS} FROM pipe_links WHERE wire = ?1"), [wire], Link::read).optional().unwrap_or_else(|error| {
        tracing::error!(%error, "could not read a pipe's link");
        None
    })
}

fn other(error: impl std::fmt::Display) -> PipeError {
    PipeError::Other(error.to_string())
}

/// What a frame carries, signed.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
struct Payload {
    v: u32,
    wire: String,
    kind: String,
    /// A message's place in its sender's order; a control's, in theirs.
    seq: u64,
    msg_id: String,
    via: Option<Via>,
    text: String,
    files: Vec<FileMeta>,
    sent_at: i64,
    /// join: the invite's secret.
    secret: String,
    /// join: the joining side.
    side: Option<Side>,
    /// join and ping: where the sender can be reached now.
    relays: Vec<String>,
    addresses: Vec<String>,
    /// ping: who the sender is, as the account service vouches
    /// (`Store::identity`); absent from a Claktory from before.
    #[serde(skip_serializing_if = "Option::is_none")]
    identity: Option<String>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
struct FileMeta {
    name: String,
    size: u64,
    sha256: String,
}

/// The answer to a frame.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
struct Reply {
    /// "ok", "refused" (a message the store refused: `reason`), "later"
    /// (try again later), "closed" (the pipe is closed there), "expired",
    /// "used", "bad".
    code: String,
    reason: String,
    /// join: the inviting side, and where it's reached.
    side: Option<Side>,
    relays: Vec<String>,
    addresses: Vec<String>,
    /// ping: who the answering side is, as a ping says it.
    #[serde(skip_serializing_if = "Option::is_none")]
    identity: Option<String>,
}

impl Reply {
    fn code(code: &str) -> Reply {
        Reply { code: code.to_owned(), ..Reply::default() }
    }
}

const KIND_JOIN: &str = "join";
const KIND_PING: &str = "ping";
const KIND_MESSAGE: &str = "message";
const KIND_APPROVE: &str = "approve";
const KIND_PAUSE: &str = "pause";
const KIND_RESUME: &str = "resume";
const KIND_CLOSE: &str = "close";

/// This Claktory's tunnel key, which signs its frames.
fn key(factory: &Factory) -> Result<&SecretKey, PipeError> {
    if let Some(key) = factory.remote.pipes.key.get() {
        return Ok(key);
    }
    let key = crate::remote::secret_key(&factory.home).map_err(other)?;
    Ok(factory.remote.pipes.key.get_or_init(|| key))
}

/// A frame: its payload's length (4 bytes, big-endian), the payload (JSON),
/// its signature (64 bytes) over `LABEL` and the payload, then its files'
/// bytes, one after the other, as the payload lists them.
fn frame(key: &SecretKey, payload: &Payload, files: &[Vec<u8>]) -> Vec<u8> {
    let body = serde_json::to_vec(payload).unwrap_or_default();
    let signature = key.sign(&[LABEL, &body].concat());
    let mut out = Vec::with_capacity(4 + body.len() + 64 + files.iter().map(Vec::len).sum::<usize>());
    out.extend_from_slice(&u32::try_from(body.len()).unwrap_or(u32::MAX).to_be_bytes());
    out.extend_from_slice(&body);
    out.extend_from_slice(&signature.to_bytes());
    for file in files {
        out.extend_from_slice(file);
    }
    out
}

/// Reads a frame's payload from `peer`, checking its signature and sizes;
/// its files follow, read by `read_files` once the payload is allowed.
async fn read_payload(receive: &mut RecvStream, peer: &EndpointId) -> Result<Payload, String> {
    let length = receive.read_u32().await.map_err(|error| error.to_string())? as usize;
    if length > MAX_PAYLOAD {
        return Err("too large".to_owned());
    }
    let mut body = vec![0; length];
    receive.read_exact(&mut body).await.map_err(|error| error.to_string())?;
    let mut signature = [0_u8; 64];
    receive.read_exact(&mut signature).await.map_err(|error| error.to_string())?;
    peer.verify(&[LABEL, &body].concat(), &Signature::from_bytes(&signature)).map_err(|_| "a bad signature".to_owned())?;
    let payload: Payload = serde_json::from_slice(&body).map_err(|error| error.to_string())?;
    let total: u64 = payload.files.iter().map(|file| file.size).sum();
    if payload.files.len() > MAX_FILES || total > MAX_FILE || payload.text.len() > MAX_TEXT {
        return Err("too large".to_owned());
    }
    Ok(payload)
}

/// Reads the files the payload lists, checking each against its sha256.
async fn read_files(receive: &mut RecvStream, payload: &Payload) -> Result<Vec<Vec<u8>>, String> {
    let mut files = Vec::with_capacity(payload.files.len());
    for meta in &payload.files {
        let mut bytes = vec![0; usize::try_from(meta.size).map_err(|error| error.to_string())?];
        receive.read_exact(&mut bytes).await.map_err(|error| error.to_string())?;
        if hex(&Sha256::digest(&bytes)) != meta.sha256 {
            return Err("a file that doesn't match".to_owned());
        }
        files.push(bytes);
    }
    Ok(files)
}

/// Reads a frame, taking only what this peer may send: an unpinned
/// key, admitted only because an invite waits, gets to send one join with
/// no files, and a pinned peer's files are read only on its own pipe's
/// wire — both checked after the signature and before any file byte is
/// buffered, so a stranger can't make this side hold its 10 MB.
async fn read_frame(
    factory: &Weak<Factory>,
    receive: &mut RecvStream,
    peer: &EndpointId,
    pinned: bool,
) -> Result<(Payload, Vec<Vec<u8>>), String> {
    let payload = read_payload(receive, peer).await?;
    if !pinned && (payload.kind != KIND_JOIN || !payload.files.is_empty()) {
        return Err("not a pipe's peer".to_owned());
    }
    if !payload.files.is_empty() {
        let factory = factory.upgrade().ok_or("shutting down")?;
        if !by_wire(&factory, &payload.wire).is_some_and(|link| link.peer == peer.to_string()) {
            return Err("a frame for a pipe that isn't its".to_owned());
        }
    }
    let files = read_files(receive, &payload).await?;
    Ok((payload, files))
}

async fn write_reply(send: &mut SendStream, reply: &Reply) -> std::io::Result<()> {
    let body = serde_json::to_vec(reply).unwrap_or_default();
    send.write_u32(u32::try_from(body.len()).unwrap_or(u32::MAX)).await?;
    send.write_all(&body).await?;
    send.finish().map_err(std::io::Error::other)?;
    Ok(())
}

/// Sends a frame on a stream of its own; the peer's reply. A ping on a
/// connection that died without a word shouldn't wait the whole `ANSWER`:
/// it gets `PING_ANSWER`, and the next try dials afresh.
const PING_ANSWER: Duration = Duration::from_secs(10);

async fn exchange(connection: &Connection, frame: &[u8], patience: Duration) -> Result<Reply, String> {
    let talk = async {
        let (mut send, mut receive) = connection.open_bi().await.map_err(|error| error.to_string())?;
        send.write_all(frame).await.map_err(|error| error.to_string())?;
        send.finish().map_err(|error| error.to_string())?;
        let length = receive.read_u32().await.map_err(|error| error.to_string())? as usize;
        if length > MAX_REPLY {
            return Err("a reply too large".to_owned());
        }
        let mut body = vec![0; length];
        receive.read_exact(&mut body).await.map_err(|error| error.to_string())?;
        serde_json::from_slice(&body).map_err(|error| error.to_string())
    };
    // More time for more bytes: a megabyte a second at least.
    let time = patience + Duration::from_secs(frame.len() as u64 / 1_000_000);
    tokio::time::timeout(time, talk).await.map_err(|_| "no answer in time".to_owned())?
}

/// Dials `peer` at what it's known by.
async fn dial(factory: &Arc<Factory>, peer: &str, relays: &[String], addresses: &[String]) -> Result<Connection, String> {
    let endpoint = factory.remote_endpoint().ok_or("remote access isn't running")?;
    let id: EndpointId = peer.parse().map_err(|_| "a bad key".to_owned())?;
    let mut addr = EndpointAddr::new(id);
    for relay in relays {
        if let Ok(url) = relay.parse::<RelayUrl>() {
            addr = addr.with_relay_url(url);
        }
    }
    for address in addresses {
        if let Ok(address) = address.parse::<SocketAddr>() {
            addr = addr.with_ip_addr(address);
        }
    }
    let connecting = endpoint.connect(addr, ALPN_PIPE);
    match tokio::time::timeout(Duration::from_millis(DIAL_MS.load(Ordering::Relaxed)), connecting).await {
        Ok(Ok(connection)) => Ok(connection),
        Ok(Err(error)) => Err(format!("{error:#}")),
        Err(_) => Err("no answer in time".to_owned()),
    }
}

/// The relays a peer may point this Claktory at: https only. The tunnel
/// is end to end, so a relay never reads pipe traffic; the rule is about
/// where the dial goes: an http relay URL could point it at any host the
/// peer names, this machine's own network included. What the person
/// pasted (an invite code's relays) is taken as it is.
fn peer_relays(relays: Vec<String>) -> Vec<String> {
    relays.into_iter().filter(|relay| relay.parse::<RelayUrl>().is_ok_and(|url| url.scheme() == "https")).collect()
}

/// The addresses a peer may point this Claktory at: never loopback,
/// link-local or unspecified, which would aim its dials at its own
/// machine or network segment. An invite code's addresses, pasted
/// by the person, are taken as they are.
fn peer_addresses(addresses: Vec<String>) -> Vec<String> {
    addresses.into_iter().filter(|address| address.parse::<SocketAddr>().is_ok_and(|address| dialable(address.ip()))).collect()
}

fn dialable(ip: IpAddr) -> bool {
    // An IPv4 address mapped into IPv6 ([::ffff:127.0.0.1]) is judged as
    // its IPv4 self.
    let ip = ip.to_canonical();
    match ip {
        IpAddr::V4(ip) => !(ip.is_loopback() || ip.is_link_local() || ip.is_unspecified() || ip.is_broadcast()),
        IpAddr::V6(ip) => !(ip.is_loopback() || ip.is_unspecified() || (ip.segments()[0] & 0xffc0) == 0xfe80),
    }
}

/// Where this Claktory is reached now, for its peers.
fn here(factory: &Factory) -> (Vec<String>, Vec<String>) {
    match factory.remote_endpoint() {
        Some(endpoint) => {
            let (relays, addresses) = factory.reached_at(&endpoint);
            (relays, addresses.iter().map(ToString::to_string).collect())
        }
        None => (Vec::new(), Vec::new()),
    }
}

fn random_hex() -> Result<String, PipeError> {
    let mut bytes = [0_u8; 32];
    getrandom::fill(&mut bytes).map_err(other)?;
    Ok(hex(&bytes))
}

fn sha256_hex(text: &str) -> String {
    hex(&Sha256::digest(text.as_bytes()))
}

/// The four digits both people compare: as a device's, from the inviter's
/// key, the joiner's and the secret's SHA-256.
fn digits(inviter: &str, joiner: &str, secret_sha256: &str) -> String {
    crate::remote::approval_number(inviter, joiner, secret_sha256)
}

/// What a code holds.
struct Code {
    wire: String,
    secret: String,
    endpoint: String,
    relays: Vec<String>,
    addresses: Vec<String>,
    expires: i64,
    side: Side,
}

fn parse_code(code: &str) -> Result<Code, PipeError> {
    let code = code.trim();
    if !code.starts_with("claktory-pipe://") {
        return Err(PipeError::BadCode);
    }
    let one = |name: &str| crate::remote::code_values(code, name).next().unwrap_or_default();
    let parsed = Code {
        wire: one("w"),
        secret: one("s"),
        endpoint: one("e"),
        relays: crate::remote::code_values(code, "r").collect(),
        addresses: crate::remote::code_values(code, "a").collect(),
        expires: one("x").parse().unwrap_or(0),
        side: Side {
            claktory: one("c"),
            owner_name: one("o"),
            owner_email: one("m"),
            project: one("j"),
            pipe_name: one("n"),
            purpose: one("p"),
        },
    };
    let hex_of = |text: &str, length: usize| text.len() == length && unhex(text).is_some();
    if !hex_of(&parsed.wire, 64) || !hex_of(&parsed.secret, 64) || parsed.endpoint.parse::<EndpointId>().is_err() {
        return Err(PipeError::BadCode);
    }
    Ok(parsed)
}

/// Recomputes who the gate lets in and whether remote access stays on for
/// pipes, and starts a worker for each link with a peer to reach. Called
/// when Factory starts and whenever links change.
pub(crate) fn reload(factory: &Arc<Factory>) {
    let now = now_ms();
    let links: Vec<Link> = {
        let db = factory.store.db();
        let read = db
            .prepare(&format!("SELECT {COLUMNS} FROM pipe_links"))
            .and_then(|mut query| query.query_map([], Link::read)?.collect::<rusqlite::Result<Vec<Link>>>());
        match read {
            Ok(links) => links,
            Err(error) => {
                tracing::error!(%error, "could not read the pipes' links");
                return;
            }
        }
    };
    let state = &factory.remote.pipes;
    let invite_until = links
        .iter()
        .filter(|link| link.inviter && !link.closed && link.invite_expires > now)
        .map(|link| link.invite_expires)
        .max()
        .unwrap_or(0);
    state.admits.invite_until.store(invite_until, Ordering::Relaxed);
    *lock(&state.admits.keys) = links.iter().filter(|link| !link.peer.is_empty()).map(|link| link.peer.clone()).collect();
    state.active.store(links.iter().filter(|link| !link.peer.is_empty()).count(), Ordering::Relaxed);
    let mut workers = lock(&state.workers);
    for link in links.iter().filter(|link| !link.peer.is_empty()) {
        if !workers.contains_key(&link.pipe) {
            let worker = Arc::new(Worker::default());
            workers.insert(link.pipe.clone(), worker.clone());
            tokio::spawn(work(Arc::downgrade(factory), link.pipe.clone(), worker));
        }
    }
}

/// The link's worker looks at its queue again now.
pub(crate) fn wake(factory: &Factory, pipe: &str) {
    if let Some(worker) = lock(&factory.remote.pipes.workers).get(pipe) {
        worker.wake.notify_one();
    }
}

/// A one-time code for `pipe`, working for `INVITE_LIFETIME`:
/// `claktory-pipe://?…` with this Claktory's address, a secret and `mine`.
/// A new code for the same pipe replaces one not used yet. Returns it with
/// when it stops working, in ms since 1970.
pub async fn invite(factory: &Arc<Factory>, pipe: &str, mine: &Side) -> Result<(String, i64), PipeError> {
    if let Some(link) = load(factory, pipe)
        && (!link.peer.is_empty() || !link.inviter || link.closed)
    {
        return Err(PipeError::Exists);
    }
    let (wire, secret) = (random_hex()?, random_hex()?);
    let expires = now_ms() + INVITE_LIFETIME.as_millis() as i64;
    let link = Link {
        pipe: pipe.to_owned(),
        wire: wire.clone(),
        inviter: true,
        secret_sha256: sha256_hex(&secret),
        invite_expires: expires,
        mine: mine.clone(),
        next_seq: 1,
        next_cseq: 1,
        ..Link::default()
    };
    link.save(&factory.store.db()).map_err(other)?;
    reload(factory);
    let (endpoint, relays, addresses) = match factory.reachable().await {
        Ok(reached) => reached,
        Err(error) => {
            cancel_invite(factory, pipe);
            return Err(PipeError::Unreachable(error.to_string()));
        }
    };
    use crate::remote::encode;
    let mut code = format!("claktory-pipe://?w={wire}&s={secret}&e={}&x={expires}", endpoint.id());
    for (name, value) in [
        ("c", &mine.claktory),
        ("o", &mine.owner_name),
        ("m", &mine.owner_email),
        ("j", &mine.project),
        ("n", &mine.pipe_name),
        ("p", &mine.purpose),
    ] {
        code.push_str(&format!("&{name}={}", encode(value)));
    }
    for relay in &relays {
        code.push_str(&format!("&r={}", encode(relay)));
    }
    for address in &addresses {
        code.push_str(&format!("&a={}", encode(&address.to_string())));
    }
    Ok((code, expires))
}

/// What an invite code says of the side that made it, without dialing:
/// for the Accept screen.
pub fn read_invite(code: &str) -> Result<Side, PipeError> {
    let code = parse_code(code)?;
    if code.expires != 0 && code.expires <= now_ms() {
        return Err(PipeError::Expired);
    }
    Ok(code.side)
}

/// Ends `pipe`'s code before it's used (Connect a pipe… closed).
pub fn cancel_invite(factory: &Arc<Factory>, pipe: &str) {
    let deleted = factory.store.db().execute("DELETE FROM pipe_links WHERE pipe = ?1 AND inviter = 1 AND peer = ''", [pipe]);
    if let Err(error) = deleted {
        tracing::error!(%error, "could not cancel a pipe's code");
    }
    reload(factory);
}

/// Keeps remote access on while a join is under way.
struct Joining(Weak<Factory>, String);

impl Joining {
    fn start(factory: &Arc<Factory>, wire: &str) -> Joining {
        factory.remote.pipes.joining.fetch_add(1, Ordering::Relaxed);
        lock(&factory.remote.pipes.joining_wires).insert(wire.to_owned());
        Joining(Arc::downgrade(factory), wire.to_owned())
    }
}

impl Drop for Joining {
    fn drop(&mut self) {
        if let Some(factory) = self.0.upgrade() {
            factory.remote.pipes.joining.fetch_sub(1, Ordering::Relaxed);
            lock(&factory.remote.pipes.joining_wires).remove(&self.1);
        }
    }
}

/// Joins the pipe `code` offers, as the store's `pipe`, with `mine`: dials
/// the inviter, proves the code's secret, exchanges sides and pins its key.
/// Returns it with the four digits both people compare. The code is used
/// up.
pub async fn join(factory: &Arc<Factory>, pipe: &str, code: &str, mine: &Side) -> Result<(Peer, String), PipeError> {
    let code = parse_code(code)?;
    if code.expires != 0 && code.expires <= now_ms() {
        return Err(PipeError::Expired);
    }
    if load(factory, pipe).is_some() || by_wire(factory, &code.wire).is_some() {
        return Err(PipeError::Exists);
    }
    let _joining = Joining::start(factory, &code.wire);
    let (endpoint, relays, addresses) = factory.reachable().await.map_err(|error| PipeError::Unreachable(error.to_string()))?;
    let me = endpoint.id().to_string();
    if me == code.endpoint {
        return Err(PipeError::Other("This code was made on this Claktory. Use it on the other one.".to_owned()));
    }
    let connection = dial(factory, &code.endpoint, &code.relays, &code.addresses).await.map_err(PipeError::Unreachable)?;
    let payload = Payload {
        v: 1,
        wire: code.wire.clone(),
        kind: KIND_JOIN.to_owned(),
        secret: code.secret.clone(),
        side: Some(mine.clone()),
        relays,
        addresses: addresses.iter().map(ToString::to_string).collect(),
        sent_at: now_ms(),
        ..Payload::default()
    };
    let reply = exchange(&connection, &frame(key(factory)?, &payload, &[]), ANSWER).await.map_err(PipeError::Unreachable)?;
    connection.close(0u32.into(), b"joined");
    match reply.code.as_str() {
        "ok" => {}
        "expired" => return Err(PipeError::Expired),
        "used" => return Err(PipeError::Used),
        "closed" => return Err(PipeError::Closed),
        _ => return Err(PipeError::BadCode),
    }
    let side = reply.side.unwrap_or_default();
    let mut known_addresses = peer_addresses(reply.addresses);
    for address in code.addresses {
        if !known_addresses.contains(&address) {
            known_addresses.push(address);
        }
    }
    let link = Link {
        pipe: pipe.to_owned(),
        wire: code.wire,
        inviter: false,
        secret_sha256: sha256_hex(&code.secret),
        mine: mine.clone(),
        peer: code.endpoint.clone(),
        peer_side: side.clone(),
        relays: {
            let relays = peer_relays(reply.relays);
            if relays.is_empty() { code.relays } else { relays }
        },
        addresses: known_addresses,
        next_seq: 1,
        next_cseq: 1,
        last_seen: now_ms(),
        ..Link::default()
    };
    link.save(&factory.store.db()).map_err(other)?;
    reload(factory);
    let digits = digits(&code.endpoint, &me, &link.secret_sha256);
    Ok((Peer { endpoint_id: code.endpoint, side, verified: false }, digits))
}

/// Queues a control for the peer (`kind`), after `change`s the link.
fn control(factory: &Arc<Factory>, pipe: &str, kind: &str, change: impl FnOnce(&mut Link)) -> Result<(), PipeError> {
    let key = key(factory)?;
    {
        let mut db = factory.store.db();
        let mut link = load_from(&db, pipe).ok_or(PipeError::NotFound)?;
        if link.closed {
            return Err(PipeError::Closed);
        }
        if link.peer.is_empty() {
            return Err(PipeError::NotOpen);
        }
        change(&mut link);
        let payload = Payload {
            v: 1,
            wire: link.wire.clone(),
            kind: kind.to_owned(),
            seq: link.next_cseq as u64,
            sent_at: now_ms(),
            ..Payload::default()
        };
        let bytes = frame(key, &payload, &[]);
        link.next_cseq += 1;
        let transaction = db.transaction().map_err(other)?;
        link.save(&transaction).map_err(other)?;
        transaction.execute("INSERT INTO pipe_outbox (pipe, control, frame) VALUES (?1, 1, ?2)", params![pipe, bytes]).map_err(other)?;
        transaction.commit().map_err(other)?;
    }
    wake(factory, pipe);
    Ok(())
}

/// This side's person approved: the peer is told (once it can be reached).
pub fn approve(factory: &Arc<Factory>, pipe: &str) -> Result<(), PipeError> {
    control(factory, pipe, KIND_APPROVE, |link| link.approved = true)
}

/// Signs `message` and queues it for the peer, in order, until it's acked.
/// Only once the pipe is open (both approved). Returns its place in this
/// side's order.
pub fn send(factory: &Arc<Factory>, pipe: &str, message: Outgoing) -> Result<u64, PipeError> {
    // Said before the files are read, as ever; said again under the hold
    // below, where it counts.
    let open = |link: &Link| {
        if link.closed {
            return Err(PipeError::Closed);
        }
        if !(link.approved && link.peer_approved) {
            return Err(PipeError::NotOpen);
        }
        Ok(())
    };
    open(&load(factory, pipe).ok_or(PipeError::NotFound)?)?;
    if message.text.len() > MAX_TEXT {
        return Err(PipeError::TooLarge(format!("A message can hold at most {} KB of text.", MAX_TEXT / 1024)));
    }
    if message.files.len() > MAX_FILES {
        return Err(PipeError::TooLarge(format!("A message can carry at most {MAX_FILES} files.")));
    }
    let mut files = Vec::with_capacity(message.files.len());
    let mut metas = Vec::with_capacity(message.files.len());
    let mut total = 0_u64;
    for file in &message.files {
        let bytes = std::fs::read(&file.path).map_err(|error| other(format!("Can't read {}: {error}", file.path.display())))?;
        total += bytes.len() as u64;
        if total > MAX_FILE {
            return Err(PipeError::TooLarge(format!("A message can carry at most {} MB of files.", MAX_FILE / 1024 / 1024)));
        }
        metas.push(FileMeta { name: file.name.clone(), size: bytes.len() as u64, sha256: hex(&Sha256::digest(&bytes)) });
        files.push(bytes);
    }
    let key = key(factory)?;
    let seq = {
        let mut db = factory.store.db();
        let mut link = load_from(&db, pipe).ok_or(PipeError::NotFound)?;
        open(&link)?;
        let seq = link.next_seq as u64;
        let by_agent = matches!(message.via, Via::Agent { .. });
        let payload = Payload {
            v: 1,
            wire: link.wire.clone(),
            kind: KIND_MESSAGE.to_owned(),
            seq,
            msg_id: message.msg_id.clone(),
            via: Some(message.via),
            text: message.text,
            files: metas,
            sent_at: now_ms(),
            ..Payload::default()
        };
        let bytes = frame(key, &payload, &files);
        link.next_seq += 1;
        let transaction = db.transaction().map_err(other)?;
        link.save(&transaction).map_err(other)?;
        transaction
            .execute(
                "INSERT INTO pipe_outbox (pipe, control, msg_id, frame, agent) VALUES (?1, 0, ?2, ?3, ?4)",
                params![pipe, message.msg_id, bytes, by_agent],
            )
            .map_err(other)?;
        transaction.commit().map_err(other)?;
        seq
    };
    let count = queued(factory, pipe);
    // Not under the workers' lock: the store's link_changed takes Factory's,
    // and status, called under Factory's, takes the workers'.
    let worker = lock(&factory.remote.pipes.workers).get(pipe).cloned();
    if let Some(worker) = worker {
        set_status(factory, pipe, &worker, |status| status.queued = count);
    }
    wake(factory, pipe);
    Ok(seq)
}

/// Nothing goes either way until `resume`; the peer is told.
pub fn pause(factory: &Arc<Factory>, pipe: &str) -> Result<(), PipeError> {
    control(factory, pipe, KIND_PAUSE, |link| link.paused = true)
}

pub fn resume(factory: &Arc<Factory>, pipe: &str) -> Result<(), PipeError> {
    control(factory, pipe, KIND_RESUME, |link| link.paused = false)
}

/// Ends the pipe: the peer is told if it can be (for a week), its key
/// unpinned once it has heard, the queue dropped. Also turns a pipe away
/// before it's approved, and ends a code not used yet.
pub fn close(factory: &Arc<Factory>, pipe: &str) {
    let Some(link) = load(factory, pipe) else { return };
    if link.peer.is_empty() {
        cancel_invite(factory, pipe);
        return;
    }
    if link.closed {
        return;
    }
    let closing = control(factory, pipe, KIND_CLOSE, |link| {
        link.closed = true;
        link.closed_at = now_ms();
    });
    if let Err(error) = closing {
        tracing::error!(%error, "could not close a pipe");
    }
    if let Err(error) = factory.store.db().execute("DELETE FROM pipe_outbox WHERE pipe = ?1 AND control = 0", [pipe]) {
        tracing::error!(%error, "could not drop a closed pipe's queue");
    }
    wake(factory, pipe);
}

/// Whether the peer can be reached, as last seen (cached).
pub fn status(factory: &Factory, pipe: &str) -> LinkStatus {
    lock(&factory.remote.pipes.workers).get(pipe).map(|worker| lock(&worker.status).clone()).unwrap_or_default()
}

/// Forgets a link for good: its row, its queue, its key.
fn forget(factory: &Arc<Factory>, pipe: &str) {
    {
        let db = factory.store.db();
        for statement in ["DELETE FROM pipe_outbox WHERE pipe = ?1", "DELETE FROM pipe_links WHERE pipe = ?1"] {
            if let Err(error) = db.execute(statement, [pipe]) {
                tracing::error!(%error, "could not forget a pipe's link");
            }
        }
    }
    lock(&factory.remote.pipes.workers).remove(pipe);
    reload(factory);
}

/// The next frame to send: the first control, or else, while messages
/// flow, the first message; while agents' are held (`hold_agents`), only
/// those queued before the first of them. (n, a message's id or "", the
/// frame.)
fn next_frame(factory: &Factory, link: &Link, hold_agents: bool) -> Option<(i64, String, Vec<u8>)> {
    let db = factory.store.db();
    let found = if link.flowing() {
        db.query_row(
            "SELECT n, msg_id, frame FROM pipe_outbox WHERE pipe = ?1 AND (control = 1 OR NOT ?2
                 OR n < (SELECT COALESCE(MIN(n), 9223372036854775807) FROM pipe_outbox WHERE pipe = ?1 AND control = 0 AND agent = 1))
             ORDER BY control DESC, n LIMIT 1",
            params![&link.pipe, hold_agents],
            crate::store::row3,
        )
    } else {
        db.query_row("SELECT n, msg_id, frame FROM pipe_outbox WHERE pipe = ?1 AND control = 1 ORDER BY n LIMIT 1", [&link.pipe], crate::store::row3)
    };
    found.optional().unwrap_or_else(|error| {
        tracing::error!(%error, "could not read a pipe's queue");
        None
    })
}

fn queued(factory: &Factory, pipe: &str) -> u32 {
    factory.store.db().query_row("SELECT COUNT(*) FROM pipe_outbox WHERE pipe = ?1 AND control = 0", [pipe], |row| row.get(0)).unwrap_or(0)
}

/// The peer couldn't be reached: why, and since when (kept from the
/// first failure).
fn offline(status: &mut LinkStatus, reason: &str) {
    status.online = false;
    status.reason = reason.lines().next().unwrap_or_default().to_owned();
    if status.offline_since_ms == 0 {
        status.offline_since_ms = now_ms();
    }
}

/// Sets what `status` says, telling the store when online, the queue or
/// the reason changed (not for last seen alone).
fn set_status(factory: &Arc<Factory>, pipe: &str, worker: &Worker, change: impl FnOnce(&mut LinkStatus)) {
    let (before, after) = {
        let mut status = lock(&worker.status);
        let before = status.clone();
        change(&mut status);
        (before, status.clone())
    };
    if (before.online, before.queued, &before.reason) != (after.online, after.queued, &after.reason)
        && let Some(store) = store()
    {
        store.link_changed(factory, pipe, after);
    }
}

/// Reaches one pipe's peer for as long as the link has something to do:
/// sends what's queued, in order, one frame at a time, each until it's
/// answered, and checks every `PING` that the peer is there.
async fn work(factory: Weak<Factory>, pipe: String, worker: Arc<Worker>) {
    let mut connection: Option<Connection> = None;
    // Whether the peer has answered a ping on this connection: until it
    // has, no message goes, so who it is now (its identity, which may hold
    // agents' messages) is heard before anything queued meanwhile.
    let mut pinged = false;
    let mut backoff = Duration::from_secs(1);
    let mut contact = tokio::time::Instant::now() - PING;
    loop {
        let Some(strong) = factory.upgrade() else { return };
        let Some(link) = load(&strong, &pipe) else {
            lock(&strong.remote.pipes.workers).remove(&pipe);
            return;
        };
        let queue = queued(&strong, &pipe);
        set_status(&strong, &pipe, &worker, |status| {
            status.queued = queue;
            status.last_seen_ms = link.last_seen;
        });
        let hold_agents = store().is_some_and(|store| store.holds_agents(&strong, &pipe));
        let next = next_frame(&strong, &link, hold_agents);
        if link.closed && (next.is_none() || now_ms() - link.closed_at > CLOSE_TRIES) {
            // Its close was heard (or never will be).
            forget(&strong, &pipe);
            return;
        }
        let ping_due = contact.elapsed() >= PING;
        if next.is_none() && !ping_due {
            drop(strong);
            let _ = tokio::time::timeout_at(contact + PING, worker.wake.notified()).await;
            continue;
        }
        if connection.as_ref().is_some_and(|connection| connection.close_reason().is_some()) {
            connection = None;
        }
        if connection.is_none() {
            match dial(&strong, &link.peer, &link.relays, &link.addresses).await {
                Ok(dialed) => {
                    connection = Some(dialed);
                    pinged = false;
                }
                Err(error) => {
                    tracing::debug!(pipe, %error, "can't reach a pipe's peer");
                    set_status(&strong, &pipe, &worker, |status| offline(status, &error));
                    drop(strong);
                    let _ = tokio::time::timeout(backoff, worker.wake.notified()).await;
                    backoff = (backoff * 2).min(MAX_BACKOFF);
                    continue;
                }
            }
        }
        let Some(open) = connection.clone() else { continue };
        let next = next.filter(|(_, msg_id, _)| pinged || msg_id.is_empty());
        let bytes = match &next {
            Some((_, _, frame)) => frame.clone(),
            None => {
                let (relays, addresses) = here(&strong);
                let payload = Payload {
                    v: 1,
                    wire: link.wire.clone(),
                    kind: KIND_PING.to_owned(),
                    relays,
                    addresses,
                    sent_at: now_ms(),
                    identity: store().and_then(|store| store.own_identity(&strong)),
                    ..Payload::default()
                };
                match key(&strong) {
                    Ok(key) => frame(key, &payload, &[]),
                    Err(error) => {
                        tracing::error!(%error, "can't sign for a pipe");
                        return;
                    }
                }
            }
        };
        let patience = if next.is_some() { ANSWER } else { PING_ANSWER };
        drop(strong);
        let answered = exchange(&open, &bytes, patience).await;
        let Some(strong) = factory.upgrade() else { return };
        let reply = match answered {
            Ok(reply) => reply,
            Err(error) => {
                tracing::debug!(pipe, %error, "a pipe's peer didn't answer");
                connection = None;
                set_status(&strong, &pipe, &worker, |status| offline(status, &error));
                drop(strong);
                let _ = tokio::time::timeout(backoff, worker.wake.notified()).await;
                backoff = (backoff * 2).min(MAX_BACKOFF);
                continue;
            }
        };
        backoff = Duration::from_secs(1);
        contact = tokio::time::Instant::now();
        let seen = now_ms();
        if let Err(error) = strong.store.db().execute("UPDATE pipe_links SET last_seen = ?2 WHERE pipe = ?1", params![pipe, seen]) {
            tracing::error!(%error, "could not note when a pipe's peer was seen");
        }
        set_status(&strong, &pipe, &worker, |status| {
            status.online = true;
            status.last_seen_ms = seen;
            status.reason.clear();
            status.offline_since_ms = 0;
        });
        if reply.code == "closed" {
            if !link.closed
                && let Some(store) = store()
            {
                store.peer_closed(&strong, &pipe);
            }
            forget(&strong, &pipe);
            return;
        }
        if let (Some(identity), Some(store)) = (reply.identity.as_deref(), store()) {
            store.identity(&strong, &pipe, &link.peer, identity);
        }
        if next.is_none() {
            pinged = true;
        }
        let Some((n, msg_id, _)) = next else { continue };
        match reply.code.as_str() {
            "ok" | "refused" => {
                if let Err(error) = strong.store.db().execute("DELETE FROM pipe_outbox WHERE n = ?1", [n]) {
                    tracing::error!(%error, "could not take a delivered frame off a pipe's queue");
                }
                if !msg_id.is_empty()
                    && let Some(store) = store()
                {
                    if reply.code == "ok" {
                        store.delivered(&strong, &pipe, &msg_id);
                    } else {
                        store.refused(&strong, &pipe, &msg_id, &reply.reason);
                    }
                }
            }
            // Not now (it isn't open or flowing there yet): after a while, or
            // when the peer says something.
            _ => {
                drop(strong);
                let _ = tokio::time::timeout(Duration::from_secs(5), worker.wake.notified()).await;
            }
        }
    }
}

/// Frames read at once per peer connection: enough for a busy pipe, and
/// a bound on what one connection can make this side hold in memory.
const STREAMS_AT_ONCE: usize = 8;
/// Streams a connection whose key isn't pinned to a pipe gets in all: a
/// joiner needs one per try, and it's pinned once it claims the invite.
const UNPINNED_STREAMS: usize = 8;

/// Serves a peer's connection on the pipes' ALPN: each stream one frame,
/// answered.
pub(crate) async fn serve(connection: Connection, factory: Weak<Factory>) {
    let peer = connection.remote_id();
    let reading = Arc::new(tokio::sync::Semaphore::new(STREAMS_AT_ONCE));
    let mut unpinned_streams = 0;
    while let Ok((send, receive)) = connection.accept_bi().await {
        let Some(strong) = factory.upgrade() else { return };
        let pinned = strong.remote.pipes.admits.pinned(&peer.to_string());
        drop(strong);
        if !pinned {
            unpinned_streams += 1;
            if unpinned_streams > UNPINNED_STREAMS {
                // Here only to claim a waiting invite, and that takes one
                // stream, not a stream mill.
                connection.close(0u32.into(), b"too many streams");
                return;
            }
        }
        let Ok(permit) = reading.clone().acquire_owned().await else { return };
        let factory = factory.clone();
        tokio::spawn(async move {
            let _reading = permit;
            if let Err(error) = answer(factory, peer, pinned, send, receive).await {
                tracing::debug!(%error, "a pipe's frame failed");
            }
        });
    }
}

async fn answer(
    factory: Weak<Factory>,
    peer: EndpointId,
    pinned: bool,
    mut send: SendStream,
    mut receive: RecvStream,
) -> std::io::Result<()> {
    let patience = ANSWER + Duration::from_secs(MAX_FILE / 1_000_000);
    let read = tokio::time::timeout(patience, read_frame(&factory, &mut receive, &peer, pinned)).await;
    let (payload, files) = match read {
        Ok(Ok(read)) => read,
        Ok(Err(error)) => {
            tracing::warn!(endpoint = %peer, %error, "turned away a pipe's frame");
            return write_reply(&mut send, &Reply::code("bad")).await;
        }
        Err(_) => return Ok(()),
    };
    let Some(factory) = factory.upgrade() else { return Ok(()) };
    let reply = if payload.kind == KIND_JOIN {
        joined(&factory, &peer.to_string(), payload)
    } else {
        received(&factory, &peer.to_string(), payload, files).await
    };
    write_reply(&mut send, &reply).await
}

/// The inviter's side of a join: the code's secret, then the peer pinned.
fn joined(factory: &Arc<Factory>, peer: &str, payload: Payload) -> Reply {
    let Some(mut link) = by_wire(factory, &payload.wire).filter(|link| link.inviter) else {
        return Reply::code("bad");
    };
    if link.closed {
        return Reply::code("closed");
    }
    if !same(&sha256_hex(&payload.secret), &link.secret_sha256) {
        tracing::warn!(endpoint = %peer, "a pipe's code with the wrong secret");
        return Reply::code("bad");
    }
    let (relays, addresses) = here(factory);
    let accepted = Reply { code: "ok".to_owned(), side: Some(link.mine.clone()), relays, addresses, ..Reply::default() };
    if !link.peer.is_empty() {
        // Its answer was lost, and it asks again.
        return if link.peer == peer { accepted } else { Reply::code("used") };
    }
    if link.invite_expires <= now_ms() {
        return Reply::code("expired");
    }
    link.peer = peer.to_owned();
    link.peer_side = payload.side.unwrap_or_default();
    link.relays = peer_relays(payload.relays);
    link.addresses = peer_addresses(payload.addresses);
    // invite_expires stays: until it passes, a second claimant of the code
    // still gets in far enough to hear "used".
    link.last_seen = now_ms();
    if let Err(error) = link.save(&factory.store.db()) {
        tracing::error!(%error, "could not pin a pipe's peer");
        return Reply::code("later");
    }
    reload(factory);
    let me = match key(factory) {
        Ok(key) => key.public().to_string(),
        Err(_) => String::new(),
    };
    tracing::info!(pipe = %link.pipe, endpoint = %peer, "a Claktory joined a pipe");
    if let Some(store) = store() {
        let digits = digits(&me, peer, &link.secret_sha256);
        store.knocked(factory, &link.pipe, Peer { endpoint_id: peer.to_owned(), side: link.peer_side.clone(), verified: false }, digits);
    }
    accepted
}

/// A frame of a pinned peer's: a ping, a control or a message.
async fn received(factory: &Arc<Factory>, peer: &str, payload: Payload, files: Vec<Vec<u8>>) -> Reply {
    let Some(link) = by_wire(factory, &payload.wire) else {
        // A join of ours whose answer came but whose link isn't saved yet:
        // the inviter's first ping can come first, and "closed" would end
        // the pipe it just joined.
        if lock(&factory.remote.pipes.joining_wires).contains(&payload.wire) {
            return Reply::code("later");
        }
        return Reply::code("closed");
    };
    if link.peer != peer {
        tracing::warn!(endpoint = %peer, "a frame for a pipe that isn't its");
        return Reply::code("bad");
    }
    if link.closed {
        return Reply::code("closed");
    }
    let receiving = lock(&factory.remote.pipes.receiving).entry(link.pipe.clone()).or_default().clone();
    let _one = receiving.lock().await;
    // Afresh, under the lock.
    let Some(mut link) = load(factory, &link.pipe) else { return Reply::code("closed") };
    link.last_seen = now_ms();
    let pipe = link.pipe.clone();
    let reply = match payload.kind.as_str() {
        KIND_PING => {
            let relays = peer_relays(payload.relays);
            let addresses = peer_addresses(payload.addresses);
            if !relays.is_empty() || !addresses.is_empty() {
                link.relays = relays;
                link.addresses = addresses;
            }
            let store = store();
            if let (Some(identity), Some(store)) = (payload.identity.as_deref(), &store) {
                store.identity(factory, &pipe, &link.peer, identity);
            }
            // Ours back, from a Claktory that says who it is.
            let identity = payload.identity.is_some().then(|| store.and_then(|store| store.own_identity(factory))).flatten();
            Reply { identity, ..Reply::code("ok") }
        }
        KIND_APPROVE | KIND_PAUSE | KIND_RESUME | KIND_CLOSE => {
            if payload.seq as i64 <= link.received_cseq {
                Reply::code("ok")
            } else {
                link.received_cseq = payload.seq as i64;
                match payload.kind.as_str() {
                    KIND_APPROVE => link.peer_approved = true,
                    KIND_PAUSE => link.peer_paused = true,
                    KIND_RESUME => link.peer_paused = false,
                    _ => {
                        forget(factory, &pipe);
                        if let Some(store) = store() {
                            store.peer_closed(factory, &pipe);
                        }
                        return Reply::code("ok");
                    }
                }
                if let Err(error) = link.save_received(&factory.store.db()) {
                    tracing::error!(%error, "could not save a pipe's link");
                    return Reply::code("later");
                }
                if let Some(store) = store() {
                    match payload.kind.as_str() {
                        KIND_APPROVE => store.peer_approved(factory, &pipe),
                        KIND_PAUSE => store.peer_paused(factory, &pipe),
                        _ => store.peer_resumed(factory, &pipe),
                    }
                }
                wake(factory, &pipe);
                return Reply::code("ok");
            }
        }
        KIND_MESSAGE => {
            if payload.seq as i64 <= link.received_seq {
                Reply::code("ok")
            } else if payload.seq as i64 != link.received_seq + 1 || !link.approved || !link.peer_approved || link.paused {
                Reply::code("later")
            } else {
                let Some(store) = store() else { return Reply::code("later") };
                let files = match keep_files(factory, &pipe, payload.seq, &payload.files, files).await {
                    Ok(files) => files,
                    Err(error) => {
                        tracing::error!(%error, "could not keep a pipe's files");
                        return Reply::code("later");
                    }
                };
                let message = Incoming {
                    msg_id: payload.msg_id,
                    seq: payload.seq,
                    via: payload.via.unwrap_or(Via::Person { name: String::new() }),
                    text: payload.text,
                    files,
                    sent_at_ms: payload.sent_at,
                };
                let outcome = store.received(factory, &pipe, message);
                link.received_seq = payload.seq as i64;
                match outcome {
                    Ok(()) => Reply::code("ok"),
                    Err(reason) => Reply { code: "refused".to_owned(), reason, ..Reply::default() },
                }
            }
        }
        _ => Reply::code("bad"),
    };
    if let Err(error) = link.save_received(&factory.store.db()) {
        tracing::error!(%error, "could not save a pipe's link");
        return Reply::code("later");
    }
    // The peer is there: what waits for it can go now.
    wake(factory, &pipe);
    reply
}

/// Writes a message's files to `<home>/pipes/incoming/<pipe>/<seq>/`, for
/// the store to move; clears out what was left there long ago.
async fn keep_files(factory: &Factory, pipe: &str, seq: u64, metas: &[FileMeta], files: Vec<Vec<u8>>) -> std::io::Result<Vec<InFile>> {
    if files.is_empty() {
        return Ok(Vec::new());
    }
    let root = factory.home.root().join("pipes/incoming");
    let dir = root.join(sanitize(pipe)).join(seq.to_string());
    let metas = metas.to_vec();
    tokio::task::spawn_blocking(move || {
        clear_old(&root);
        std::fs::create_dir_all(&dir)?;
        let mut kept = Vec::with_capacity(files.len());
        for (index, (meta, bytes)) in metas.iter().zip(files).enumerate() {
            let path = dir.join(format!("{index}"));
            std::fs::write(&path, bytes)?;
            kept.push(InFile { name: meta.name.clone(), path });
        }
        Ok(kept)
    })
    .await
    .map_err(std::io::Error::other)?
}

pub(crate) fn sanitize(text: &str) -> String {
    text.chars().map(|char| if char.is_ascii_alphanumeric() || char == '-' { char } else { '_' }).collect()
}

/// Deletes what's been under `root` longer than `INCOMING_KEPT`.
fn clear_old(root: &Path) {
    let Ok(pipes) = std::fs::read_dir(root) else { return };
    for pipe in pipes.flatten() {
        let Ok(messages) = std::fs::read_dir(pipe.path()) else { continue };
        for message in messages.flatten() {
            let old = message
                .metadata()
                .and_then(|meta| meta.modified())
                .is_ok_and(|modified| modified.elapsed().is_ok_and(|age| age > INCOMING_KEPT));
            if old {
                let _ = std::fs::remove_dir_all(message.path());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // The established-link paths (a dial or exchange that fails) call
    // `offline`; two Factories in one test process always find each
    // other's real addresses once joined, so the recording is pinned here.
    #[test]
    fn offline_keeps_the_first_failure_time_and_one_line() {
        let mut status = LinkStatus { online: true, ..LinkStatus::default() };
        offline(&mut status, "no answer in time\nand a second line");
        assert!(!status.online);
        assert_eq!(status.reason, "no answer in time");
        let since = status.offline_since_ms;
        assert!(since > 0);

        // A later, different failure updates the reason, not the start.
        offline(&mut status, "connection lost");
        assert_eq!(status.reason, "connection lost");
        assert_eq!(status.offline_since_ms, since);
    }

    /// A frame heard from the peer is handled on a copy of the link read
    /// when it came; this side's person approving or an agent sending
    /// meanwhile must survive its save (the pipes_between flake: a ping
    /// heard while the person approved left the link unapproved here, and
    /// the held message they let through never went).
    #[test]
    fn hearing_from_the_peer_never_undoes_what_this_side_changed_meanwhile() {
        let dir = std::env::temp_dir().join(format!("factoryd-pipe-link-{}-{}", std::process::id(), now_ms()));
        std::fs::create_dir_all(&dir).expect("dir");
        let store = crate::store::Store::open(&dir.join("factory.db")).expect("store");
        let link = Link { pipe: "p".into(), wire: "w".into(), peer: "k".into(), next_seq: 1, next_cseq: 1, ..Link::default() };
        link.save(&store.db()).expect("saved");

        let mut heard = load_from(&store.db(), "p").expect("the copy a frame is handled on");
        {
            let db = store.db();
            let mut mine = load_from(&db, "p").expect("the link");
            mine.approved = true;
            mine.next_seq = 2;
            mine.next_cseq = 2;
            mine.save(&db).expect("saved");
        }
        heard.peer_approved = true;
        heard.received_seq = 3;
        heard.last_seen = 5;
        heard.save_received(&store.db()).expect("saved");

        let now = load_from(&store.db(), "p").expect("the link");
        assert!(now.approved && now.peer_approved);
        assert_eq!((now.next_seq, now.next_cseq, now.received_seq, now.last_seen), (2, 2, 3, 5));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
