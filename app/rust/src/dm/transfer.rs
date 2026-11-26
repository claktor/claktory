//! History transfer between the person's own devices (dms-e2ee.md §2, E8;
//! Fugo's six points): right after a device adds a new one, it sends the new
//! one what it keeps of every conversation, straight over iroh, sealed with
//! the key only these two devices got from the add-device hand-off.
//!
//! - **The key** is the hand-off's HPKE exporter (claktory-dm-crypto's
//!   `handoff`): the sender keeps it from `seal_handoff`, by the new leaf
//!   and with the endpoint the QR code named; the new device from
//!   `finish_handoff`. Both in memory only ([`Keys`]), gone after
//!   [`LIFETIME`] or once a transfer completes, zeroized.
//! - **Who talks to whom:** the sender dials only the endpoint the QR code
//!   named, which iroh authenticates. The new device takes one connection at
//!   a time, and only one whose first chunk opens under the key: anything
//!   else is closed and it waits on. Once one has, every other is refused
//!   until it ends.
//! - **Framing:** claktory-dm-crypto's `transfer` seals every chunk (its
//!   session, index and conversation in the associated data) and the end
//!   record, which says how many chunks and messages there were. A transfer
//!   that stops before it, or doesn't add up, is cut short, never complete.
//! - **Resuming** ("Try again", within the key's lifetime) is a new session
//!   from the sender, told what the new device already kept of each
//!   conversation; never by index.
//! - **The receiver's limits:** a cap on the whole and on each message
//!   (16 KiB of text), names one line with nothing hidden, ids short, seqs
//!   only rising: a chunk that isn't so ends the transfer. History can't be
//!   checked against MLS signatures here; it's taken as coming from the
//!   person's own device, which only the key proves.
//! - **Where it goes:** each message into `history`, sealed as dm.rs seals
//!   its own, or, for a group the new device hasn't joined yet (its Welcome
//!   may come later), into `transferred`, which [`adopt`] moves over when it
//!   joins. Read positions, deletions and drafts merge as the self group
//!   merges them. Pins don't travel: the hand-off carried them.
//!
//! The wire: one bidirectional stream, frames of a 4-byte length and a tag.
//! Sender: `HELLO` (the session, then chunk 0: `{"v":1}`); after `RESUME`,
//! chunk 1 (`{"chats":N}`), then each conversation's chunks
//! (`{"view"?, "messages", "last"}`), then `END`. Receiver: `RESUME`
//! (`{"have":{group:seq}}`) once the hello opened, and `DONE` (1: complete)
//! after the end record.

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::{Context as _, anyhow, bail};
use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use claktory_dm_crypto::certificate::{MAX_ACCOUNT_ID, MAX_LEAF_ID, MAX_NAME, one_line};
use claktory_dm_crypto::content::MAX_TEXT;
use claktory_dm_crypto::transfer::{MAX_CHUNK, Opener, Sealer};
use iroh::endpoint::{Connection, ConnectionError, VarInt, presets};
use iroh::{EndpointAddr, EndpointId, SecretKey};
use rusqlite::{Transaction, params};
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncRead, AsyncReadExt as _, AsyncWrite, AsyncWriteExt as _};
use tokio::sync::{mpsc, watch};
use zeroize::Zeroizing;

use crate::api::dm::{DmDevice, DmGroupKind, DmGroupType, DmMessage, DmView, place, view_name};
use crate::dm::store::Store;

/// The transfer's protocol on iroh.
pub const ALPN: &[u8] = b"claktory-dm-transfer/1";
/// How long the key, and with it the transfer and any resume, lasts.
pub const LIFETIME: Duration = Duration::from_secs(30 * 60);
/// The most a transfer brings over, in bytes of what it says.
pub const MOST: usize = 256 * 1024 * 1024;
/// How long the sender keeps dialling a new device that isn't ready yet.
const DIAL_FOR: Duration = Duration::from_secs(3 * 60);
const DIAL_TIMEOUT: Duration = Duration::from_secs(15);
const DIAL_AGAIN: Duration = Duration::from_secs(3);
/// How long a connection has to say who it is.
const HELLO_WITHIN: Duration = Duration::from_secs(20);
/// The longest wait for the next frame.
const IDLE: Duration = Duration::from_secs(120);
/// What a chunk is filled to, under [MAX_CHUNK].
const FILL: usize = MAX_CHUNK - 16 * 1024;
/// History is read this many rows at a time.
const PAGE: i64 = 200;
/// Staged history for a group never joined goes after a day.
const STAGED_FOR_MS: i64 = 24 * 60 * 60 * 1000;
/// The longest group id, in bytes.
const MAX_GROUP_ID: usize = 64;
/// A message's other fields (`extra_json`), and its ids.
const MAX_EXTRA: usize = 4 * 1024;
const MAX_ID: usize = 128;
const MAX_KIND: usize = 32;
/// Who a commit added and removed, at most.
const MAX_MEMBERS: usize = 256;

const HELLO: u8 = 1;
const CHUNK: u8 = 2;
const END: u8 = 3;
const RESUME: u8 = 4;
const DONE: u8 = 5;
/// The most a frame holds: a sealed chunk with its index and conversation.
const MAX_FRAME: usize = MAX_CHUNK + 16 + 1 + 8 + 1 + 255;

/// Why the new device closed a connection.
const CLOSE_DONE: u32 = 0;
const CLOSE_NOT_READY: u32 = 1;
const CLOSE_BUSY: u32 = 2;
const CLOSE_SKIPPED: u32 = 3;
const CLOSE_REFUSED: u32 = 4;

/// How a transfer ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum End {
    /// Everything came, and added up.
    Complete,
    /// The person skipped it, or nothing came.
    Skipped,
    /// Some came, not all.
    CutShort,
}

/// The transfer keys this device holds, in memory only: as the sender, one
/// per new leaf it added (with the endpoint that leaf's QR code named); as a
/// new device, its own. Each lasts [LIFETIME].
pub struct Keys {
    lifetime: Duration,
    inner: Mutex<KeysInner>,
}

#[derive(Default)]
struct KeysInner {
    sending: HashMap<String, Sending>,
    receiving: Option<Receiving>,
}

struct Sending {
    key: Zeroizing<[u8; 32]>,
    endpoint: [u8; 32],
    until: Instant,
}

struct Receiving {
    key: Zeroizing<[u8; 32]>,
    until: Instant,
}

/// What the sender needs: the key, the endpoint to dial and when it ends.
pub struct ToSend {
    pub key: Zeroizing<[u8; 32]>,
    pub endpoint: [u8; 32],
    pub until: Instant,
}

impl Default for Keys {
    fn default() -> Keys {
        Keys::lasting(LIFETIME)
    }
}

impl Keys {
    /// Keys that last [lifetime] (tests make it short).
    pub fn lasting(lifetime: Duration) -> Keys {
        Keys { lifetime, inner: Mutex::new(KeysInner::default()) }
    }

    fn inner(&self) -> std::sync::MutexGuard<'_, KeysInner> {
        let mut inner = self.inner.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let now = Instant::now();
        inner.sending.retain(|_, sending| sending.until > now);
        if inner.receiving.as_ref().is_some_and(|receiving| receiving.until <= now) {
            inner.receiving = None;
        }
        inner
    }

    /// The sender's key for the new leaf [leaf_id], whose QR code named
    /// [endpoint].
    pub fn keep_sending(&self, leaf_id: &str, endpoint: [u8; 32], key: &[u8; 32]) {
        let until = Instant::now() + self.lifetime;
        self.inner().sending.insert(leaf_id.to_owned(), Sending { key: Zeroizing::new(*key), endpoint, until });
    }

    /// The new device's key.
    pub fn keep_receiving(&self, key: &[u8; 32]) {
        self.inner().receiving = Some(Receiving { key: Zeroizing::new(*key), until: Instant::now() + self.lifetime });
    }

    /// Whether this device can still send its history to [leaf_id].
    pub fn can_send(&self, leaf_id: &str) -> bool {
        self.inner().sending.contains_key(leaf_id)
    }

    pub fn sending(&self, leaf_id: &str) -> Option<ToSend> {
        self.inner().sending.get(leaf_id).map(|sending| ToSend {
            key: sending.key.clone(),
            endpoint: sending.endpoint,
            until: sending.until,
        })
    }

    pub fn receiving(&self) -> Option<(Zeroizing<[u8; 32]>, Instant)> {
        self.inner().receiving.as_ref().map(|receiving| (receiving.key.clone(), receiving.until))
    }

    pub fn forget_sending(&self, leaf_id: &str) {
        self.inner().sending.remove(leaf_id);
    }

    pub fn forget_receiving(&self) {
        self.inner().receiving = None;
    }
}

/// History staged before its group was joined: moved into the group's
/// history once it is (dm.rs calls this as it adds a group), and staging
/// for groups never joined dropped after a day.
pub fn adopt(tx: &Transaction<'_>, group_id: &[u8]) -> rusqlite::Result<()> {
    tx.execute("INSERT OR IGNORE INTO history (grp, seq, value) SELECT grp, seq, value FROM transferred WHERE grp = ?1", [group_id])?;
    tx.execute("DELETE FROM transferred WHERE grp = ?1 OR created_ms < ?2", params![group_id, now_ms() - STAGED_FOR_MS])?;
    Ok(())
}

fn now_ms() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|since| since.as_millis() as i64).unwrap_or(0)
}

#[derive(Serialize, Deserialize)]
struct Hello {
    v: u8,
}

#[derive(Serialize, Deserialize)]
struct Plan {
    chats: u64,
}

/// Part of one conversation's history.
#[derive(Serialize, Deserialize)]
struct Part {
    #[serde(default)]
    view: Option<DmView>,
    messages: Vec<DmMessage>,
    last: bool,
}

/// What the new device already kept, by conversation: the last seq.
#[derive(Serialize, Deserialize, Default)]
struct Resume {
    have: BTreeMap<String, i64>,
}

fn json<T: Serialize>(value: &T) -> anyhow::Result<Zeroizing<Vec<u8>>> {
    Ok(Zeroizing::new(serde_json::to_vec(value)?))
}

async fn write_frame<W: AsyncWrite + Unpin>(writer: &mut W, tag: u8, parts: &[&[u8]]) -> anyhow::Result<()> {
    let len: usize = 1 + parts.iter().map(|part| part.len()).sum::<usize>();
    if len > MAX_FRAME {
        bail!("a frame of {len} bytes");
    }
    let mut frame = Vec::with_capacity(4 + len);
    frame.extend_from_slice(&(len as u32).to_be_bytes());
    frame.push(tag);
    for part in parts {
        frame.extend_from_slice(part);
    }
    writer.write_all(&frame).await?;
    writer.flush().await?;
    Ok(())
}

async fn read_frame<R: AsyncRead + Unpin>(reader: &mut R, within: Duration) -> anyhow::Result<(u8, Vec<u8>)> {
    tokio::time::timeout(within, async {
        let mut len = [0u8; 4];
        reader.read_exact(&mut len).await?;
        let len = u32::from_be_bytes(len) as usize;
        if len == 0 || len > MAX_FRAME {
            bail!("a frame of {len} bytes");
        }
        let mut frame = vec![0u8; len];
        reader.read_exact(&mut frame).await?;
        let tag = frame.remove(0);
        Ok((tag, frame))
    })
    .await
    .map_err(|_| anyhow!("nothing came for {} s", within.as_secs()))?
}

fn chunk_frame(index: u64, conversation: &str, sealed: &[u8]) -> anyhow::Result<Vec<u8>> {
    let conversation = conversation.as_bytes();
    let len = u8::try_from(conversation.len()).map_err(|_| anyhow!("a conversation id that long"))?;
    let mut body = Vec::with_capacity(9 + conversation.len() + sealed.len());
    body.extend_from_slice(&index.to_be_bytes());
    body.push(len);
    body.extend_from_slice(conversation);
    body.extend_from_slice(sealed);
    Ok(body)
}

fn read_chunk(body: &[u8]) -> anyhow::Result<(u64, String, &[u8])> {
    let short = || anyhow!("a chunk frame cut short");
    let index = u64::from_be_bytes(body.get(..8).ok_or_else(short)?.try_into().expect("8 bytes"));
    let len = usize::from(*body.get(8).ok_or_else(short)?);
    let conversation = body.get(9..9 + len).ok_or_else(short)?;
    let conversation = String::from_utf8(conversation.to_vec()).context("a conversation id")?;
    Ok((index, conversation, &body[9 + len..]))
}

fn b64(bytes: &[u8]) -> String {
    URL_SAFE_NO_PAD.encode(bytes)
}

/// The conversations this device keeps, each with its view.
fn conversations(store: &Store) -> anyhow::Result<Vec<(Vec<u8>, DmView)>> {
    Ok(store.with_tx(|_, tx| {
        let rows: Vec<(Vec<u8>, String)> = tx
            .prepare("SELECT id, kind FROM groups ORDER BY id")
            .map_err(sql)?
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
            .map_err(sql)?
            .collect::<Result<_, _>>()
            .map_err(sql)?;
        let mut kept = Vec::new();
        for (id, kind) in rows {
            let Ok(kind) = serde_json::from_str::<DmGroupKind>(&kind) else { continue };
            if kind.kind != DmGroupType::Conversation {
                continue;
            }
            let view = DmDevice::read_view(store, tx, &id)?;
            kept.push((id, view));
        }
        Ok::<_, claktory_dm_crypto::Error>(kept)
    })?)
}

/// Up to [PAGE] of [group_id]'s messages after [after], decrypted.
fn page(store: &Store, group_id: &[u8], after: i64) -> anyhow::Result<Vec<(i64, Zeroizing<Vec<u8>>)>> {
    let rows: Vec<(i64, Vec<u8>)> = store.with_tx(|_, tx| {
        tx.prepare("SELECT seq, value FROM history WHERE grp = ?1 AND seq > ?2 ORDER BY seq LIMIT ?3")
            .map_err(sql)?
            .query_map(params![group_id, after, PAGE], |row| Ok((row.get(0)?, row.get(1)?)))
            .map_err(sql)?
            .collect::<Result<_, _>>()
            .map_err(sql)
    })?;
    rows.into_iter()
        .map(|(seq, sealed)| Ok((seq, store.open_row(&place(b"history", group_id, &seq.to_be_bytes(), b"value"), &sealed)?)))
        .collect()
}

fn sql(error: rusqlite::Error) -> claktory_dm_crypto::Error {
    claktory_dm_crypto::Error::Storage(error.to_string())
}

fn alive(until: Instant) -> anyhow::Result<()> {
    if Instant::now() >= until {
        bail!("the transfer's 30 minutes are over");
    }
    Ok(())
}

/// The sender's side, on a stream to the new device: everything this
/// device keeps of its conversations that the new one doesn't have yet.
pub async fn send<R, W>(store: &Store, key: &[u8; 32], until: Instant, mut reader: R, mut writer: W) -> anyhow::Result<End>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    alive(until)?;
    let mut sealer = Sealer::new(key)?;
    let (_, hello) = sealer.seal_chunk("", 0, &json(&Hello { v: 1 })?)?;
    write_frame(&mut writer, HELLO, &[&sealer.session(), &hello]).await?;
    let (tag, body) = read_frame(&mut reader, IDLE).await?;
    if tag != RESUME {
        bail!("the new device answered with {tag}");
    }
    let resume: Resume = serde_json::from_slice(&body).context("what the new device has")?;

    // Only conversations with something it doesn't have.
    let mut plan = Vec::new();
    for (group_id, view) in conversations(store)? {
        let have = resume.have.get(&b64(&group_id)).copied().unwrap_or(0).max(view.deleted_seq);
        if !page(store, &group_id, have)?.is_empty() {
            plan.push((group_id, view, have));
        }
    }
    let (index, sealed) = sealer.seal_chunk("", 0, &json(&Plan { chats: plan.len() as u64 })?)?;
    write_frame(&mut writer, CHUNK, &[&chunk_frame(index, "", &sealed)?]).await?;

    for (group_id, view, have) in plan {
        let conversation = b64(&group_id);
        let mut after = have;
        let mut view = Some(view);
        let mut part: Vec<DmMessage> = Vec::new();
        let mut size = 0;
        loop {
            alive(until)?;
            let rows = page(store, &group_id, after)?;
            let done = (rows.len() as i64) < PAGE;
            for (seq, plain) in rows {
                after = seq;
                let message: DmMessage = serde_json::from_slice(&plain).context("a kept message")?;
                // Comfortably under the chunk with the rest of the part.
                if plain.len() > FILL / 2 {
                    continue;
                }
                if size + plain.len() + 1 > FILL {
                    let full = Part { view: view.take(), messages: std::mem::take(&mut part), last: false };
                    seal_part(&mut sealer, &mut writer, &conversation, &full).await?;
                    size = 0;
                }
                size += plain.len() + 1;
                part.push(message);
            }
            if done {
                break;
            }
        }
        let last = Part { view: view.take(), messages: std::mem::take(&mut part), last: true };
        seal_part(&mut sealer, &mut writer, &conversation, &last).await?;
    }
    let end = sealer.seal_end()?;
    write_frame(&mut writer, END, &[&end]).await?;
    let (tag, body) = read_frame(&mut reader, IDLE).await?;
    if tag != DONE || body.len() != 1 {
        bail!("the new device didn't say how it went");
    }
    Ok(if body[0] == 1 { End::Complete } else { End::CutShort })
}

async fn seal_part<W: AsyncWrite + Unpin>(sealer: &mut Sealer, writer: &mut W, conversation: &str, part: &Part) -> anyhow::Result<()> {
    let plain = json(part)?;
    let (index, sealed) = sealer.seal_chunk(conversation, part.messages.len() as u64, &plain)?;
    write_frame(writer, CHUNK, &[&chunk_frame(index, conversation, &sealed)?]).await
}

/// What the new device has received so far, across sessions: the last
/// seq of each conversation, to resume from.
#[derive(Default)]
pub struct Have(Mutex<BTreeMap<String, i64>>);

impl Have {
    fn get(&self) -> BTreeMap<String, i64> {
        self.0.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).clone()
    }

    fn set(&self, conversation: &str, seq: i64) {
        self.0.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).insert(conversation.to_owned(), seq);
    }
}

/// How a receiving session went.
#[derive(Debug)]
pub enum Received {
    /// The connection never proved it has the key: not the sender.
    Stranger(anyhow::Error),
    /// The sender's, ending so; with how many conversations came.
    Ended(End),
}

/// The new device's side, on a stream from a connection: checks it's the
/// sender, then takes its history into [store]. [progress] hears
/// (conversations done, conversations in all) as they finish.
pub async fn receive<R, W>(
    store: &Store,
    key: &[u8; 32],
    until: Instant,
    have: &Have,
    mut reader: R,
    writer: W,
    progress: &mut (dyn FnMut(u64, u64) + Send),
) -> Received
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    // Who it is: a hello that opens under the key, or nothing.
    let opener = match hello(key, &mut reader).await {
        Ok(opener) => opener,
        Err(error) => return Received::Stranger(error),
    };
    Received::Ended(session(store, until, have, opener, reader, writer, progress).await)
}

/// The rest of a receiving session, once the hello opened.
async fn session<R, W>(
    store: &Store,
    until: Instant,
    have: &Have,
    mut opener: Opener,
    mut reader: R,
    mut writer: W,
    progress: &mut (dyn FnMut(u64, u64) + Send),
) -> End
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let mut got_any = false;
    let result = async {
        write_frame(&mut writer, RESUME, &[&serde_json::to_vec(&Resume { have: have.get() })?]).await?;
        let (tag, body) = read_frame(&mut reader, IDLE).await?;
        let (index, conversation, sealed) = if tag == CHUNK { read_chunk(&body)? } else { bail!("no plan") };
        if !conversation.is_empty() {
            bail!("no plan");
        }
        let plan: Plan = serde_json::from_slice(&opener.open_chunk(index, "", sealed)?).context("the plan")?;
        progress(0, plan.chats);
        let mut done = 0;
        let mut kept: BTreeMap<String, u64> = BTreeMap::new();
        let mut total = 0usize;
        let mut last: BTreeMap<String, i64> = have.get();
        loop {
            alive(until)?;
            let (tag, body) = read_frame(&mut reader, IDLE).await?;
            match tag {
                CHUNK => {
                    let (index, conversation, sealed) = read_chunk(&body)?;
                    if conversation.is_empty() {
                        bail!("a second plan");
                    }
                    let plain = opener.open_chunk(index, &conversation, sealed)?;
                    total += plain.len();
                    if total > MOST {
                        bail!("more than {} MiB of history", MOST / 1024 / 1024);
                    }
                    let group_id = URL_SAFE_NO_PAD.decode(&conversation).context("a conversation id")?;
                    if group_id.is_empty() || group_id.len() > MAX_GROUP_ID {
                        bail!("a group id of {} bytes", group_id.len());
                    }
                    let part: Part = serde_json::from_slice(&plain).context("a part of a conversation")?;
                    let from = last.get(&conversation).copied().unwrap_or(0);
                    let messages = checked(part.messages, from)?;
                    let count = messages.len() as u64;
                    let newest = messages.last().map(|message| message.seq);
                    take(store, &group_id, part.view.as_ref(), messages)?;
                    got_any = true;
                    *kept.entry(conversation.clone()).or_default() += count;
                    if let Some(newest) = newest {
                        last.insert(conversation.clone(), newest);
                        have.set(&conversation, newest);
                    }
                    if part.last {
                        done += 1;
                        progress(done.min(plan.chats), plan.chats);
                    }
                }
                END => {
                    let whole = opener.finish(&body, &kept).is_ok();
                    write_frame(&mut writer, DONE, &[&[u8::from(whole)]]).await?;
                    let _ = writer.shutdown().await;
                    return Ok(if whole { End::Complete } else { End::CutShort });
                }
                other => bail!("a frame tagged {other}"),
            }
        }
    }
    .await;
    match result {
        Ok(end) => end,
        Err(error) => {
            log(&format!("history transfer stopped: {error:#}"));
            if got_any { End::CutShort } else { End::Skipped }
        }
    }
}

async fn hello<R: AsyncRead + Unpin>(key: &[u8; 32], reader: &mut R) -> anyhow::Result<Opener> {
    let (tag, body) = read_frame(reader, HELLO_WITHIN).await?;
    if tag != HELLO || body.len() < 16 {
        bail!("not a hello");
    }
    let session: [u8; 16] = body[..16].try_into().expect("16 bytes");
    let mut opener = Opener::new(key, session);
    let plain = opener.open_chunk(0, "", &body[16..]).map_err(|_| anyhow!("a hello that doesn't open"))?;
    let hello: Hello = serde_json::from_slice(&plain).context("a hello")?;
    if hello.v != 1 {
        bail!("transfer version {}", hello.v);
    }
    Ok(opener)
}

fn log(line: &str) {
    eprintln!("{line}");
}

/// Printable ASCII with no spaces, at most [most] bytes (empty allowed).
fn ascii(text: &str, most: usize) -> bool {
    text.len() <= most && text.bytes().all(|b| b.is_ascii_graphic())
}

/// [messages] as the receiver keeps them, after [from]; refused whole if
/// any isn't well-formed (Fugo 5).
fn checked(messages: Vec<DmMessage>, from: i64) -> anyhow::Result<Vec<DmMessage>> {
    let mut previous = from;
    messages
        .into_iter()
        .map(|mut message| {
            if message.seq <= previous {
                bail!("messages out of order");
            }
            previous = message.seq;
            if message.arrived_ms < 0 {
                bail!("a message from before 1970");
            }
            let content = &message.content;
            let ids_ok = ascii(&message.from_account, MAX_ACCOUNT_ID)
                && ascii(&message.from_leaf, MAX_LEAF_ID)
                && ascii(&content.kind, MAX_KIND)
                && ascii(&content.client_message_id, MAX_ID);
            if !ids_ok {
                bail!("a message with an id that isn't one");
            }
            if content.text.as_ref().is_some_and(|text| text.len() > MAX_TEXT) {
                bail!("a message over {MAX_TEXT} bytes of text");
            }
            if content.extra_json.len() > MAX_EXTRA
                || (!content.extra_json.trim().is_empty()
                    && !matches!(serde_json::from_str(&content.extra_json), Ok(serde_json::Value::Object(_))))
            {
                bail!("a message whose other fields aren't");
            }
            if let Some(by) = &content.by_agent {
                let ok = ascii(&by.claktory_id, MAX_ID)
                    && ascii(&by.agent_id, MAX_ID)
                    && ascii(&by.incarnation, MAX_ID)
                    && one_line(&by.name, MAX_NAME);
                if !ok {
                    bail!("a message by an agent that isn't named in one line");
                }
            }
            if let Some(members) = &message.members {
                if members.added.len() + members.removed.len() + members.set_up_again.len() + members.joined.len() > MAX_MEMBERS {
                    bail!("a change of too many members");
                }
                for member in members.added.iter().chain(&members.removed).chain(&members.set_up_again).chain(&members.joined) {
                    if !ascii(&member.account, MAX_ACCOUNT_ID) || !ascii(&member.leaf_id, MAX_LEAF_ID) || !one_line(&member.name, MAX_NAME)
                    {
                        bail!("a member that isn't named in one line");
                    }
                }
            }
            // From the person's other device, here as from the feed.
            message.mine = false;
            Ok(message)
        })
        .collect()
}

/// Keeps [messages] of [group_id] and merges its [view], in one
/// transaction: into its history if this device is in the group, staged
/// until it joins if not.
fn take(store: &Store, group_id: &[u8], view: Option<&DmView>, messages: Vec<DmMessage>) -> anyhow::Result<()> {
    store.with_tx(|_, tx| {
        let joined: bool =
            tx.query_row("SELECT count(*) FROM groups WHERE id = ?1", [group_id], |row| row.get::<_, i64>(0)).map_err(sql)? > 0;
        let mut deleted = DmDevice::read_view(store, tx, group_id)?.deleted_seq;
        if let Some(theirs) = view {
            deleted = merge_view(store, tx, group_id, theirs)?;
        }
        let now = now_ms();
        for message in messages.iter().filter(|message| message.seq > deleted) {
            let seq = message.seq.to_be_bytes();
            let plain = Zeroizing::new(serde_json::to_vec(message).map_err(|e| claktory_dm_crypto::Error::Format(e.to_string()))?);
            let sealed = store.seal_row(&place(b"history", group_id, &seq, b"value"), &plain)?;
            if joined {
                tx.execute("INSERT OR IGNORE INTO history (grp, seq, value) VALUES (?1, ?2, ?3)", params![group_id, message.seq, sealed])
            } else {
                tx.execute(
                    "INSERT OR IGNORE INTO transferred (grp, seq, value, created_ms) VALUES (?1, ?2, ?3, ?4)",
                    params![group_id, message.seq, sealed, now],
                )
            }
            .map_err(sql)?;
        }
        Ok::<_, claktory_dm_crypto::Error>(())
    })?;
    Ok(())
}

/// Merges the sender's view of [group_id] into this device's, as the self
/// group would: the furthest read and deleted, the newer draft. Gives how
/// far history is deleted now.
fn merge_view(store: &Store, tx: &Transaction<'_>, group_id: &[u8], theirs: &DmView) -> Result<i64, claktory_dm_crypto::Error> {
    let mine = DmDevice::read_view(store, tx, group_id)?;
    let mut merged = mine.clone();
    merged.read_seq = mine.read_seq.max(theirs.read_seq);
    merged.deleted_seq = mine.deleted_seq.max(theirs.deleted_seq);
    if theirs.draft_ms > mine.draft_ms && theirs.draft.len() <= MAX_TEXT {
        merged.draft = theirs.draft.clone();
        merged.draft_ms = theirs.draft_ms;
    }
    if merged == mine {
        return Ok(mine.deleted_seq);
    }
    if merged.deleted_seq > mine.deleted_seq {
        tx.execute("DELETE FROM history WHERE grp = ?1 AND seq <= ?2", params![group_id, merged.deleted_seq]).map_err(sql)?;
        tx.execute("DELETE FROM transferred WHERE grp = ?1 AND seq <= ?2", params![group_id, merged.deleted_seq]).map_err(sql)?;
    }
    let name = view_name(group_id);
    let plain = Zeroizing::new(serde_json::to_vec(&merged).map_err(|e| claktory_dm_crypto::Error::Format(e.to_string()))?);
    let sealed = store.seal_row(&[b"rows", name.as_bytes()], &plain)?;
    tx.execute("INSERT OR REPLACE INTO rows (name, value) VALUES (?1, ?2)", params![name.as_bytes(), sealed]).map_err(sql)?;
    Ok(merged.deleted_seq)
}

/// What a transfer endpoint is doing with connections.
enum Accepting {
    /// Nothing is receiving: connections are told it isn't ready.
    Idle,
    /// A receiver waits: the next connection goes to it.
    Armed(mpsc::Sender<Connection>),
    /// A sender proved itself: every other connection is refused.
    Busy,
}

/// A transfer endpoint: a fresh iroh endpoint for one add-device, the new
/// device's (its id goes in the QR code) or the sender's (it dials from it).
/// The relay admits its key through the device's assertion for it, which
/// Dart asks for ([Endpoint::id]).
pub struct Endpoint {
    endpoint: iroh::Endpoint,
    phase: Arc<Mutex<Accepting>>,
    have: Have,
    cancel: watch::Sender<bool>,
    receiving: Mutex<bool>,
}

/// The device's QUIC settings: on Android without segmentation offload, as
/// the tunnel's (tunnel.rs, `device_transport`).
fn transport() -> iroh::endpoint::QuicTransportConfig {
    iroh::endpoint::QuicTransportConfig::builder().enable_segmentation_offload(!cfg!(target_os = "android")).build()
}

fn phase(phase: &Mutex<Accepting>) -> std::sync::MutexGuard<'_, Accepting> {
    phase.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

impl Endpoint {
    /// A new endpoint on n0's relays, as the tunnel's, with a new key.
    pub async fn bind() -> anyhow::Result<Endpoint> {
        let endpoint = iroh::Endpoint::builder(presets::N0)
            .secret_key(SecretKey::generate())
            .alpns(vec![ALPN.to_vec()])
            .transport_config(transport())
            .bind()
            .await?;
        Ok(Endpoint::on(endpoint))
    }

    /// A transfer endpoint on [endpoint] (which takes [ALPN]); in a runtime.
    pub fn on(endpoint: iroh::Endpoint) -> Endpoint {
        let shared = Arc::new(Mutex::new(Accepting::Idle));
        let accepting = endpoint.clone();
        let phases = shared.clone();
        tokio::spawn(async move {
            while let Some(incoming) = accepting.accept().await {
                let phases = phases.clone();
                tokio::spawn(async move {
                    let Ok(accepting) = incoming.accept() else { return };
                    let Ok(connection) = accepting.await else { return };
                    let refused = {
                        let mut phase = phase(&phases);
                        match &*phase {
                            Accepting::Idle => Some(CLOSE_NOT_READY),
                            Accepting::Busy => Some(CLOSE_BUSY),
                            Accepting::Armed(waiting) => match waiting.try_send(connection.clone()) {
                                Ok(()) => None,
                                Err(mpsc::error::TrySendError::Full(_)) => Some(CLOSE_BUSY),
                                Err(mpsc::error::TrySendError::Closed(_)) => {
                                    *phase = Accepting::Idle;
                                    Some(CLOSE_NOT_READY)
                                }
                            },
                        }
                    };
                    if let Some(code) = refused {
                        connection.close(VarInt::from_u32(code), b"");
                    }
                });
            }
        });
        Endpoint { endpoint, phase: shared, have: Have::default(), cancel: watch::channel(false).0, receiving: Mutex::new(false) }
    }

    /// Its id: the endpoint key, 32 bytes.
    pub fn id(&self) -> [u8; 32] {
        *self.endpoint.id().as_bytes()
    }

    /// Where it can be reached (for tests on one machine).
    pub fn addr(&self) -> EndpointAddr {
        self.endpoint.addr()
    }

    /// Ends the receiving that's going on, as skipped.
    pub fn cancel(&self) {
        let _ = self.cancel.send(true);
    }

    pub async fn close(&self) {
        self.cancel();
        self.endpoint.close().await;
    }

    /// The new device's side: waits for the sender (until [until]), takes
    /// its history into [store], and says how it went. One at a time.
    pub async fn receive(
        &self,
        store: &Store,
        key: &[u8; 32],
        until: Instant,
        progress: &mut (dyn FnMut(u64, u64) + Send),
    ) -> anyhow::Result<End> {
        {
            let mut receiving = self.receiving.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
            if *receiving {
                bail!("this device is receiving its history already");
            }
            *receiving = true;
        }
        let _ = self.cancel.send(false);
        let end = self.receive_one(store, key, until, progress).await;
        *phase(&self.phase) = Accepting::Idle;
        *self.receiving.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = false;
        Ok(end)
    }

    async fn receive_one(&self, store: &Store, key: &[u8; 32], until: Instant, progress: &mut (dyn FnMut(u64, u64) + Send)) -> End {
        let (arm, mut connections) = mpsc::channel(1);
        *phase(&self.phase) = Accepting::Armed(arm.clone());
        let mut cancelled = self.cancel.subscribe();
        loop {
            let wait = until.saturating_duration_since(Instant::now());
            let connection = tokio::select! {
                connection = connections.recv() => match connection { Some(connection) => connection, None => return End::Skipped },
                _ = cancelled.wait_for(|cancelled| *cancelled) => return End::Skipped,
                _ = tokio::time::sleep(wait) => return End::Skipped,
            };
            // Only a connection whose hello opens under the key is the
            // sender's; any other is closed, and the wait goes on.
            let proven = tokio::time::timeout(HELLO_WITHIN, async {
                let (writer, mut reader) = connection.accept_bi().await?;
                let opener = hello(key, &mut reader).await?;
                anyhow::Ok((opener, reader, writer))
            })
            .await
            .unwrap_or_else(|_| Err(anyhow!("no hello in time")));
            let (opener, reader, writer) = match proven {
                Ok(proven) => proven,
                Err(error) => {
                    log(&format!("history transfer: a connection that isn't the sender's: {error:#}"));
                    connection.close(VarInt::from_u32(CLOSE_REFUSED), b"");
                    continue;
                }
            };
            // From here it's the only one: every other is refused.
            *phase(&self.phase) = Accepting::Busy;
            drop(connections);
            let end = tokio::select! {
                end = session(store, until, &self.have, opener, reader, writer, progress) => end,
                _ = cancelled.wait_for(|cancelled| *cancelled) => {
                    connection.close(VarInt::from_u32(CLOSE_SKIPPED), b"");
                    return End::Skipped;
                }
            };
            // The sender closes once it has DONE; then it's gone.
            let _ = tokio::time::timeout(Duration::from_secs(5), connection.closed()).await;
            connection.close(VarInt::from_u32(CLOSE_DONE), b"");
            return end;
        }
    }

    /// The sender's side: dials [to] (the endpoint the new device's QR code
    /// named), again while it isn't ready yet, and sends this device's
    /// history from [store] under [key], until [until].
    pub async fn send(&self, store: &Store, key: &[u8; 32], until: Instant, to: EndpointAddr) -> anyhow::Result<End> {
        let dial_until = until.min(Instant::now() + DIAL_FOR);
        loop {
            alive(until)?;
            let connection = match tokio::time::timeout(DIAL_TIMEOUT, self.endpoint.connect(to.clone(), ALPN)).await {
                Ok(Ok(connection)) => connection,
                Ok(Err(error)) if Instant::now() < dial_until => {
                    log(&format!("history transfer: dialling the new device: {error}"));
                    tokio::time::sleep(DIAL_AGAIN).await;
                    continue;
                }
                Err(_) if Instant::now() < dial_until => continue,
                Ok(Err(error)) => bail!("couldn't reach the new device: {error}"),
                Err(_) => bail!("couldn't reach the new device"),
            };
            let sent = async {
                let (writer, reader) = connection.open_bi().await?;
                send(store, key, until, reader, writer).await
            }
            .await;
            if let Ok(end) = sent {
                connection.close(VarInt::from_u32(CLOSE_DONE), b"");
                return Ok(end);
            }
            let why = tokio::time::timeout(Duration::from_secs(2), connection.closed()).await.ok();
            let code = match why {
                Some(ConnectionError::ApplicationClosed(close)) => Some(close.error_code.into_inner()),
                _ => None,
            };
            match code {
                Some(code) if code == u64::from(CLOSE_NOT_READY) && Instant::now() < dial_until => {
                    tokio::time::sleep(DIAL_AGAIN).await;
                }
                Some(code) if code == u64::from(CLOSE_SKIPPED) => return Ok(End::Skipped),
                Some(code) if code == u64::from(CLOSE_BUSY) => bail!("the new device is receiving from elsewhere"),
                _ => {
                    connection.close(VarInt::from_u32(CLOSE_DONE), b"");
                    return Ok(End::CutShort);
                }
            }
        }
    }
}

/// The endpoint id of a new device, from its QR code's 32 bytes.
pub fn endpoint_addr(endpoint: &[u8; 32]) -> anyhow::Result<EndpointAddr> {
    Ok(EndpointAddr::new(EndpointId::from_bytes(endpoint)?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::dm::{DmContent, DmMember, DmMembers, dm_new_secret};
    use iroh::RelayMode;

    fn folder() -> std::path::PathBuf {
        static NEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("claktory-dm-transfer-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn device(dir: &std::path::Path, name: &str) -> DmDevice {
        let path = dir.join(format!("{name}.db")).to_string_lossy().into_owned();
        DmDevice::open(path, dm_new_secret().unwrap(), dm_new_secret().unwrap(), None).unwrap().unwrap()
    }

    fn in_group(store: &Store, group_id: &[u8], kind: DmGroupType) {
        let kind = DmGroupKind { kind, accounts: vec!["ana".into(), "matt".into()], claktory_id: String::new() };
        store
            .with_tx(|_, tx| {
                tx.execute(
                    "INSERT INTO groups (id, kind, seq) VALUES (?1, ?2, 0)",
                    params![group_id, serde_json::to_string(&kind).unwrap()],
                )
                .map_err(sql)?;
                adopt(tx, group_id).map_err(sql)
            })
            .unwrap();
    }

    fn message(seq: i64, text: &str) -> DmMessage {
        DmMessage {
            seq,
            mine: seq % 2 == 0,
            from_account: if seq % 2 == 0 { "matt" } else { "ana" }.into(),
            from_leaf: "leaf".into(),
            from_kind_device: true,
            content: DmContent {
                kind: "text".into(),
                client_message_id: format!("m-{seq}"),
                text: Some(text.into()),
                by_agent: None,
                extra_json: "{}".into(),
            },
            arrived_ms: 1_000 + seq,
            members: None,
            departed: false,
        }
    }

    /// [count] messages said in [group_id], as dm.rs keeps them.
    fn said(store: &Store, group_id: &[u8], count: i64, size: usize) {
        store
            .with_tx(|_, tx| {
                for seq in 1..=count {
                    let text = format!("{seq}:{}", "x".repeat(size));
                    let plain = serde_json::to_vec(&message(seq, &text)).unwrap();
                    let sealed = store.seal_row(&place(b"history", group_id, &seq.to_be_bytes(), b"value"), &plain)?;
                    tx.execute("INSERT INTO history (grp, seq, value) VALUES (?1, ?2, ?3)", params![group_id, seq, sealed]).map_err(sql)?;
                }
                Ok::<_, claktory_dm_crypto::Error>(())
            })
            .unwrap();
    }

    fn seqs(device: &DmDevice, group_id: &[u8]) -> Vec<i64> {
        device.history(group_id.to_vec(), None, 10_000).unwrap().iter().map(|message| message.seq).collect()
    }

    fn staged(store: &Store) -> i64 {
        store.with_tx(|_, tx| tx.query_row("SELECT count(*) FROM transferred", [], |row| row.get(0)).map_err(sql)).unwrap()
    }

    /// Runs a transfer over an in-memory pipe, [relay] seeing every frame
    /// the sender sends and choosing whether it passes.
    async fn over_pipe(
        old: &Store,
        new: &Store,
        keys: ([u8; 32], [u8; 32]),
        have: &Have,
        mut relay: impl FnMut(u8, usize) -> bool + Send,
    ) -> (anyhow::Result<End>, Received, Vec<(u64, u64)>) {
        let mut n = 0;
        over_relay(old, new, keys, have, move |tag, body| {
            n += 1;
            if relay(tag, n) { vec![(tag, body)] } else { Vec::new() }
        })
        .await
    }

    /// The same, [relay] turning each frame the sender sends into the
    /// frames that go on (none, it, others, some held back from before).
    async fn over_relay(
        old: &Store,
        new: &Store,
        keys: ([u8; 32], [u8; 32]),
        have: &Have,
        mut relay: impl FnMut(u8, Vec<u8>) -> Vec<(u8, Vec<u8>)> + Send,
    ) -> (anyhow::Result<End>, Received, Vec<(u64, u64)>) {
        let (sender_side, relay_in) = tokio::io::duplex(1 << 20);
        let (relay_out, receiver_side) = tokio::io::duplex(1 << 20);
        let (mut from_sender, to_sender) = tokio::io::split(relay_in);
        let (from_receiver, mut to_receiver) = tokio::io::split(relay_out);
        let (sender_read, sender_write) = tokio::io::split(sender_side);
        let (receiver_read, receiver_write) = tokio::io::split(receiver_side);
        let until = Instant::now() + Duration::from_secs(60);
        let mut progress = Vec::new();
        // Frames up, as they come; answers down, untouched.
        let up = async move {
            'frames: while let Ok((tag, body)) = read_frame(&mut from_sender, IDLE).await {
                let frames = relay(tag, body);
                // A dropped end record: the way is cut there.
                if frames.is_empty() && tag == END {
                    break;
                }
                for (tag, body) in frames {
                    if write_frame(&mut to_receiver, tag, &[&body]).await.is_err() {
                        break 'frames;
                    }
                }
            }
            let _ = to_receiver.shutdown().await;
        };
        let down = async move {
            let (mut from_receiver, mut to_sender) = (from_receiver, to_sender);
            let _ = tokio::io::copy(&mut from_receiver, &mut to_sender).await;
            let _ = to_sender.shutdown().await;
        };
        let mut record = |done, total| progress.push((done, total));
        let (sent, received, _, _) = tokio::join!(
            send(old, &keys.0, until, sender_read, sender_write),
            receive(new, &keys.1, until, have, receiver_read, receiver_write, &mut record),
            up,
            down,
        );
        (sent, received, progress)
    }

    #[tokio::test]
    async fn history_comes_over_whole_and_waits_for_groups_not_joined_yet() {
        let dir = folder();
        let (old, new) = (device(&dir, "phone"), device(&dir, "tablet"));
        let (a, b, own) = (b"conversation-a".to_vec(), b"conversation-b".to_vec(), b"self-group".to_vec());
        in_group(&old.store(), &a, DmGroupType::Conversation);
        in_group(&old.store(), &b, DmGroupType::Conversation);
        in_group(&old.store(), &own, DmGroupType::SelfGroup);
        // Enough in A for several chunks; a few in B; the self group's
        // never travels.
        said(&old.store(), &a, 60, 12 * 1024);
        said(&old.store(), &b, 3, 10);
        said(&old.store(), &own, 2, 10);
        // Ana read A to 50 on the old device, and deleted B through 1.
        old.store()
            .with_tx(|_, tx| {
                merge_view(&old.store(), tx, &a, &DmView { read_seq: 50, ..DmView::default() })?;
                merge_view(&old.store(), tx, &b, &DmView { read_seq: 1, deleted_seq: 1, ..DmView::default() })
            })
            .unwrap();
        // The new device has joined A already, not B yet.
        in_group(&new.store(), &a, DmGroupType::Conversation);

        let key = [9u8; 32];
        let (sent, received, progress) = over_pipe(&old.store(), &new.store(), (key, key), &Have::default(), |_, _| true).await;
        assert_eq!(sent.unwrap(), End::Complete);
        assert!(matches!(received, Received::Ended(End::Complete)), "{received:?}");
        assert_eq!(progress.first(), Some(&(0, 2)));
        assert_eq!(progress.last(), Some(&(2, 2)));

        assert_eq!(seqs(&new, &a), (1..=60).collect::<Vec<_>>());
        let first = &new.history(a.clone(), None, 1_000).unwrap()[1];
        assert_eq!((first.mine, first.from_account.as_str()), (false, "matt"), "from the person's other device, as from the feed");
        assert_eq!(new.view(a.clone()).unwrap().read_seq, 50, "read on the old device, read here");
        // B waits until the new device joins it; then it's there.
        assert!(seqs(&new, &b).is_empty());
        assert_eq!(staged(&new.store()), 2, "B's 2 and 3: 1 was deleted");
        in_group(&new.store(), &b, DmGroupType::Conversation);
        assert_eq!(seqs(&new, &b), [2, 3]);
        assert_eq!(staged(&new.store()), 0);
        assert!(seqs(&new, &own).is_empty());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn without_the_key_nothing_opens_and_nothing_is_kept() {
        let dir = folder();
        let (old, new) = (device(&dir, "phone"), device(&dir, "tablet"));
        let a = b"conversation-a".to_vec();
        in_group(&old.store(), &a, DmGroupType::Conversation);
        in_group(&new.store(), &a, DmGroupType::Conversation);
        said(&old.store(), &a, 3, 10);
        // Another key: the hello doesn't open (test 20, with a real
        // hand-off, below).
        let (sent, received, _) = over_pipe(&old.store(), &new.store(), ([1; 32], [2; 32]), &Have::default(), |_, _| true).await;
        assert!(sent.is_err());
        assert!(matches!(received, Received::Stranger(_)), "{received:?}");
        assert!(seqs(&new, &a).is_empty());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn a_chunk_dropped_on_the_way_cuts_it_short_and_a_new_session_resumes() {
        let dir = folder();
        let (old, new) = (device(&dir, "phone"), device(&dir, "tablet"));
        let a = b"conversation-a".to_vec();
        in_group(&old.store(), &a, DmGroupType::Conversation);
        in_group(&new.store(), &a, DmGroupType::Conversation);
        said(&old.store(), &a, 60, 12 * 1024);
        let key = [5u8; 32];
        let have = Have::default();
        // The relay drops the fourth chunk: the receiver notices at the next.
        let mut chunks = 0;
        let (sent, received, _) = over_pipe(&old.store(), &new.store(), (key, key), &have, |tag, _| {
            if tag == CHUNK {
                chunks += 1;
            }
            !(tag == CHUNK && chunks == 4)
        })
        .await;
        assert!(sent.is_err() || sent.as_ref().is_ok_and(|end| *end == End::CutShort));
        assert!(matches!(received, Received::Ended(End::CutShort)), "{received:?}");
        let kept = seqs(&new, &a);
        assert!(!kept.is_empty() && kept.len() < 60, "{}", kept.len());

        // Try again: a new session, from where it got to.
        let (sent, received, _) = over_pipe(&old.store(), &new.store(), (key, key), &have, |_, _| true).await;
        assert_eq!(sent.unwrap(), End::Complete);
        assert!(matches!(received, Received::Ended(End::Complete)), "{received:?}");
        assert_eq!(seqs(&new, &a), (1..=60).collect::<Vec<_>>());

        // Cut off before the end record: never complete.
        let fresh = device(&dir, "laptop");
        in_group(&fresh.store(), &a, DmGroupType::Conversation);
        let (_, received, _) = over_pipe(&old.store(), &fresh.store(), (key, key), &Have::default(), |tag, _| tag != END).await;
        assert!(matches!(received, Received::Ended(End::CutShort)), "{received:?}");
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// Test 21: chunks swapped on the way, or a chunk sent twice, are
    /// noticed: the transfer is cut short, never complete, and a new
    /// session brings the rest.
    #[tokio::test]
    async fn chunks_reordered_or_repeated_on_the_way_cut_it_short() {
        let dir = folder();
        let (old, new) = (device(&dir, "phone"), device(&dir, "tablet"));
        let a = b"conversation-a".to_vec();
        in_group(&old.store(), &a, DmGroupType::Conversation);
        in_group(&new.store(), &a, DmGroupType::Conversation);
        said(&old.store(), &a, 60, 12 * 1024);
        let key = [6u8; 32];
        let have = Have::default();
        // The third chunk held back and sent after the fourth.
        let (mut chunks, mut held) = (0, None);
        let (_, received, _) = over_relay(&old.store(), &new.store(), (key, key), &have, |tag, body| {
            if tag != CHUNK {
                return vec![(tag, body)];
            }
            chunks += 1;
            match chunks {
                3 => {
                    held = Some(body);
                    Vec::new()
                }
                4 => vec![(tag, body), (CHUNK, held.take().expect("held"))],
                _ => vec![(tag, body)],
            }
        })
        .await;
        assert!(matches!(received, Received::Ended(End::CutShort)), "{received:?}");
        let kept = seqs(&new, &a);
        assert!(kept.len() < 60, "{}", kept.len());
        assert_eq!(kept, (1..=kept.len() as i64).collect::<Vec<_>>(), "only what came in order");

        // A fresh device: the second chunk sent twice.
        let fresh = device(&dir, "laptop");
        in_group(&fresh.store(), &a, DmGroupType::Conversation);
        let mut chunks = 0;
        let (_, received, _) = over_relay(&old.store(), &fresh.store(), (key, key), &Have::default(), |tag, body| {
            if tag == CHUNK {
                chunks += 1;
                if chunks == 2 {
                    return vec![(tag, body.clone()), (tag, body)];
                }
            }
            vec![(tag, body)]
        })
        .await;
        assert!(matches!(received, Received::Ended(End::CutShort)), "{received:?}");

        // Again, untouched: whole.
        let (sent, received, _) = over_pipe(&old.store(), &new.store(), (key, key), &have, |_, _| true).await;
        assert_eq!(sent.unwrap(), End::Complete);
        assert!(matches!(received, Received::Ended(End::Complete)), "{received:?}");
        assert_eq!(seqs(&new, &a), (1..=60).collect::<Vec<_>>());
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// Test 20: someone who recorded the hand-off and the transfer, and
    /// has the QR code's text, but not the new device's one-time key,
    /// opens nothing: sealing for that code again gives another key, and
    /// the transfer under the real one doesn't open with it.
    #[tokio::test]
    async fn the_qr_codes_text_and_a_recording_open_nothing() {
        let dir = folder();
        let (phone, tablet) = (device(&dir, "phone"), device(&dir, "tablet"));
        phone.set_up_first(dm_new_secret().unwrap(), "matt".into(), "phone".into(), "iPhone".into(), 1).unwrap();
        let leaf = "dGFibGV0LXRhYmxldC10YWJsZXQtdGFibGV0LXRhYmw";
        let handoff = tablet.start_handoff(leaf.into(), "iPad".into(), "iPadOS".into(), vec![7; 32]).unwrap();
        let sealed = phone.seal_handoff(handoff.qr()).unwrap();
        tablet.finish_handoff(&handoff, sealed.sealed.clone(), 2).unwrap();
        let real = *tablet.transfers.receiving().unwrap().0;

        // The attacker seals for the same code, under any identity: its own
        // key, not theirs.
        let offer = claktory_dm_crypto::handoff::Offer::from_qr(&handoff.qr()).unwrap();
        let identity = claktory_dm_crypto::identity::IdentityKey::generate().unwrap();
        let theirs = claktory_dm_crypto::handoff::seal(&offer, "matt", &identity, &Default::default()).unwrap();
        assert_ne!(*theirs.transfer_key, real);
        // Nor is it anything the code or the recorded bundle carry.
        let code =
            base64::Engine::decode(&base64::engine::general_purpose::URL_SAFE_NO_PAD, handoff.qr().rsplit(':').next().unwrap()).unwrap();
        assert!(!code.windows(32).any(|window| window == real) && !sealed.sealed.windows(32).any(|window| window == real));

        // The transfer under the real key, opened with the attacker's.
        let a = b"conversation-a".to_vec();
        in_group(&phone.store(), &a, DmGroupType::Conversation);
        said(&phone.store(), &a, 3, 10);
        let eavesdropper = device(&dir, "eavesdropper");
        in_group(&eavesdropper.store(), &a, DmGroupType::Conversation);
        let (sent, received, _) =
            over_pipe(&phone.store(), &eavesdropper.store(), (real, *theirs.transfer_key), &Have::default(), |_, _| true).await;
        assert!(sent.is_err());
        assert!(matches!(received, Received::Stranger(_)), "{received:?}");
        assert!(seqs(&eavesdropper, &a).is_empty());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn what_isnt_well_formed_is_refused() {
        let good = || message(1, "hi");
        assert!(checked(vec![good()], 0).is_ok());
        assert!(checked(vec![good()], 1).is_err(), "not after what it has");
        assert!(checked(vec![message(2, "a"), message(1, "b")], 0).is_err(), "out of order");
        let mut long = good();
        long.content.text = Some("x".repeat(MAX_TEXT + 1));
        assert!(checked(vec![long], 0).is_err());
        let mut odd = good();
        odd.from_account = "ana\nmatt".into();
        assert!(checked(vec![odd], 0).is_err());
        let mut extra = good();
        extra.content.extra_json = "[1]".into();
        assert!(checked(vec![extra], 0).is_err());
        let member = |name: &str| DmMember {
            account: "ana".into(),
            leaf_id: "ipad".into(),
            kind: crate::api::dm::DmLeafKind::Device,
            name: name.into(),
            since_ms: 0,
        };
        let mut added = good();
        added.members = Some(DmMembers {
            added: vec![member("Ana's iPad")],
            removed: vec![],
            key_changed: vec![],
            skipped: None,
            set_up_again: vec![],
            joined: vec![],
        });
        assert!(checked(vec![added.clone()], 0).is_ok());
        added.members = Some(DmMembers {
            added: vec![member("iPad\u{202e}")],
            removed: vec![],
            key_changed: vec![],
            skipped: None,
            set_up_again: vec![],
            joined: vec![],
        });
        assert!(checked(vec![added.clone()], 0).is_err(), "a hidden character in a name");
        added.members = Some(DmMembers {
            added: vec![],
            removed: vec![],
            key_changed: vec![],
            skipped: None,
            set_up_again: vec![],
            joined: vec![member("iPad\u{202e}")],
        });
        assert!(checked(vec![added], 0).is_err(), "a hidden character in a joining device's name");
        assert!(!checked(vec![good()], 0).unwrap()[0].mine);
    }

    #[test]
    fn keys_last_thirty_minutes_and_come_from_the_hand_off() {
        let keys = Keys::lasting(Duration::from_millis(50));
        keys.keep_sending("tablet", [1; 32], &[2; 32]);
        keys.keep_receiving(&[3; 32]);
        assert!(keys.can_send("tablet") && keys.receiving().is_some());
        std::thread::sleep(Duration::from_millis(80));
        assert!(!keys.can_send("tablet") && keys.sending("tablet").is_none() && keys.receiving().is_none(), "test 23");

        // The hand-off leaves both devices the same key, and the sender the
        // endpoint the QR code named.
        let dir = folder();
        let (phone, tablet) = (device(&dir, "phone"), device(&dir, "tablet"));
        phone.set_up_first(dm_new_secret().unwrap(), "matt".into(), "phone".into(), "iPhone".into(), 1).unwrap();
        let leaf = "dGFibGV0LXRhYmxldC10YWJsZXQtdGFibGV0LXRhYmw";
        let handoff = tablet.start_handoff(leaf.into(), "iPad".into(), "iPadOS".into(), vec![7; 32]).unwrap();
        let sealed = phone.seal_handoff(handoff.qr()).unwrap();
        tablet.finish_handoff(&handoff, sealed.sealed, 2).unwrap();
        let to = phone.transfers.sending(leaf).unwrap();
        assert_eq!(to.endpoint, [7; 32]);
        assert_eq!(*to.key, *tablet.transfers.receiving().unwrap().0);
        assert!(phone.has_history_for(leaf.into()) && !phone.has_history_for("another".into()));
        std::fs::remove_dir_all(dir).unwrap();
    }

    async fn local() -> Endpoint {
        let endpoint =
            iroh::Endpoint::builder(presets::Minimal).relay_mode(RelayMode::Disabled).alpns(vec![ALPN.to_vec()]).bind().await.unwrap();
        Endpoint::on(endpoint)
    }

    #[tokio::test]
    async fn over_iroh_the_sender_waits_for_the_new_device_and_no_one_else_gets_in() {
        let dir = folder();
        let (old, new) = (device(&dir, "phone"), device(&dir, "tablet"));
        let a = b"conversation-a".to_vec();
        in_group(&old.store(), &a, DmGroupType::Conversation);
        said(&old.store(), &a, 30, 2 * 1024);
        let (sender, receiver) = (local().await, local().await);
        let key = [4u8; 32];
        let until = Instant::now() + Duration::from_secs(60);

        // The sender dials before the new device is ready: it's told so,
        // and dials again until it is.
        let to = receiver.addr();
        let (old_store, new_store) = (old.store(), new.store());
        let sending = async { sender.send(&old_store, &key, until, to).await };
        let receiving = async {
            tokio::time::sleep(Duration::from_millis(1500)).await;
            let mut progress = |_, _| {};
            receiver.receive(&new_store, &key, until, &mut progress).await
        };
        let (sent, received) = tokio::join!(sending, receiving);
        assert_eq!((sent.unwrap(), received.unwrap()), (End::Complete, End::Complete));
        in_group(&new.store(), &a, DmGroupType::Conversation);
        assert_eq!(seqs(&new, &a), (1..=30).collect::<Vec<_>>());

        // While a sender that proved itself is connected, anyone else is
        // refused (test 22); Skip ends it for the sender too.
        let stalled = local().await;
        let other = local().await;
        let receiving = async {
            let mut progress = |_, _| {};
            receiver.receive(&new_store, &key, until, &mut progress).await
        };
        let probe = async {
            let connection = stalled.endpoint.connect(receiver.addr(), ALPN).await.unwrap();
            let (mut writer, mut reader) = connection.open_bi().await.unwrap();
            let mut sealer = Sealer::new(&key).unwrap();
            let (_, hello) = sealer.seal_chunk("", 0, &json(&Hello { v: 1 }).unwrap()).unwrap();
            write_frame(&mut writer, HELLO, &[&sealer.session(), &hello]).await.unwrap();
            assert_eq!(read_frame(&mut reader, IDLE).await.unwrap().0, RESUME);
            // Proven and stalled: a second connection is refused.
            let second = other.endpoint.connect(receiver.addr(), ALPN).await.unwrap();
            let why = tokio::time::timeout(Duration::from_secs(5), second.closed()).await.unwrap();
            assert!(
                matches!(&why, ConnectionError::ApplicationClosed(close) if close.error_code.into_inner() == u64::from(CLOSE_BUSY)),
                "{why:?}"
            );
            receiver.cancel();
            let why = tokio::time::timeout(Duration::from_secs(5), connection.closed()).await.unwrap();
            assert!(
                matches!(&why, ConnectionError::ApplicationClosed(close) if close.error_code.into_inner() == u64::from(CLOSE_SKIPPED)),
                "{why:?}"
            );
        };
        let (received, _) = tokio::join!(receiving, probe);
        assert_eq!(received.unwrap(), End::Skipped);

        // Past the key's lifetime, nothing starts.
        let over = Instant::now();
        assert!(sender.send(&old_store, &key, over, receiver.addr()).await.is_err());
        for endpoint in [sender, receiver, stalled, other] {
            endpoint.close().await;
        }
        std::fs::remove_dir_all(dir).unwrap();
    }
}
