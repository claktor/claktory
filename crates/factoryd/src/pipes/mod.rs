//! Pipes (#196): narrow, deliberate chats between one of this Claktory's
//! projects and one of another Claktory's. Both people approve a pipe, pick
//! the agents granted it (its members) and the one woken by what arrives
//! (its lead), and write their own side's prompt for it. The person can write
//! in it too.
//!
//! This is the store: what a pipe is and who may use it, its messages, and
//! the limits code enforces whatever the model does (an outgoing secret scan,
//! files only from the outbox unless the person approves, Review outgoing,
//! rate limits and a loop breaker, and the person's approval for secrets once
//! an agent has read a pipe: `agents`). The transport (`crate::pipe_link`)
//! only carries what this hands it.
//!
//! Pipes live in factory.db, never in config.json, so no agent (Rivet
//! included) can make, approve or change one: only the person, from the app.

pub(crate) mod agents;
pub(crate) mod bound;
pub(crate) mod quarantine;
pub(crate) mod start;

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use factory_protocol::v1::{self, server_message::Body};
use rusqlite::{OptionalExtension, params};
use serde::{Deserialize, Serialize};

use crate::factory::{Factory, Inner, RequestError};
use crate::pipe_link::{self, Incoming, LinkStatus, Outgoing, Peer, PipeError, Side, Via};
use crate::transcript::now_ms;

/// Where an agent's taint came from: the agents, by id, that what it read
/// came through (JSON list), first the one that read the pipe; null or empty
/// when it read the pipe itself. And the same on the asks and proposals of
/// an agent that had read a pipe (`agents::routes_json`), for the person's
/// card; null for none.
pub(crate) const TAINT_VIA: &str = "
ALTER TABLE pipe_taint ADD COLUMN via TEXT;
ALTER TABLE asks ADD COLUMN taint TEXT;
ALTER TABLE proposals ADD COLUMN taint TEXT;
";

pub(crate) const SCHEMA: &str = "
-- Pipes to other Claktories (crate::pipes): each as JSON, by its id here.
CREATE TABLE pipes (id TEXT PRIMARY KEY, data TEXT NOT NULL);
-- Their messages, and Claktory's lines about them, in each pipe's order.
CREATE TABLE pipe_messages (
    pipe TEXT NOT NULL,
    seq INTEGER NOT NULL,
    -- The id both sides know it by ('' for Claktory's lines).
    msg_id TEXT NOT NULL,
    created INTEGER NOT NULL,
    -- their_agent, their_person, my_agent, me or claktory
    author TEXT NOT NULL,
    name TEXT NOT NULL,
    role TEXT NOT NULL,
    agent TEXT NOT NULL,
    text TEXT NOT NULL,
    -- JSON: [{name, size, path}]
    files TEXT NOT NULL,
    -- held, queued, delivered, refused, discarded, received, held_incoming,
    -- or line
    state TEXT NOT NULL,
    note TEXT NOT NULL,
    PRIMARY KEY (pipe, seq)
);
CREATE INDEX pipe_messages_by_id ON pipe_messages (pipe, msg_id);
-- Agents that have read a pipe in their current context, until /clear.
CREATE TABLE pipe_taint (agent TEXT NOT NULL, pipe TEXT NOT NULL, PRIMARY KEY (agent, pipe));
-- A tainted agent's wish to use secrets, for the person to allow once or deny.
CREATE TABLE pipe_secret_approvals (
    id INTEGER PRIMARY KEY,
    agent TEXT NOT NULL,
    -- JSON lists of names.
    secrets TEXT NOT NULL,
    pipes TEXT NOT NULL,
    command TEXT NOT NULL,
    -- pending, allowed, denied or expired
    state TEXT NOT NULL,
    created INTEGER NOT NULL,
    decided INTEGER,
    -- An allowed one was used (once).
    used INTEGER NOT NULL DEFAULT 0
);
";

/// A pipe's state, as `v1::PipeState`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum State {
    #[default]
    Invited,
    Joining,
    Approval,
    WaitingPeer,
    Open,
    Paused,
    Closed,
}

/// Why a pipe is paused.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Pause {
    #[default]
    None,
    Me,
    Peer,
    Loop,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub(crate) struct Limits {
    pub per_hour: u32,
    pub burst: u32,
    pub file_mb: u32,
}

impl Default for Limits {
    fn default() -> Limits {
        Limits { per_hour: 30, burst: 5, file_mb: 50 }
    }
}

/// The other side, as it describes itself.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub(crate) struct PeerRecord {
    pub claktory: String,
    pub owner_name: String,
    pub owner_email: String,
    pub project: String,
    pub pipe_name: String,
    pub purpose: String,
    pub endpoint_id: String,
}

/// How long the other side's words about itself may be, in characters.
const NAME_CHARS: usize = 80;
const EMAIL_CHARS: usize = 120;
const PURPOSE_CHARS: usize = 300;

/// Text the other side wrote about itself, as kept: one line, no control
/// characters, at most `cap` characters (cut with "…").
pub(crate) fn clean(text: &str, cap: usize) -> String {
    let mut line = String::new();
    for character in text.chars() {
        let character = if character.is_control() || character.is_whitespace() { ' ' } else { character };
        if character == ' ' && (line.is_empty() || line.ends_with(' ')) {
            continue;
        }
        line.push(character);
    }
    let line = line.trim_end();
    if line.chars().count() <= cap {
        return line.to_owned();
    }
    let cut: String = line.chars().take(cap.saturating_sub(1)).collect();
    format!("{}…", cut.trim_end())
}

impl PeerRecord {
    fn from_side(side: &Side, endpoint_id: &str) -> PeerRecord {
        PeerRecord {
            claktory: side.claktory.clone(),
            owner_name: side.owner_name.clone(),
            owner_email: side.owner_email.clone(),
            project: side.project.clone(),
            pipe_name: side.pipe_name.clone(),
            purpose: side.purpose.clone(),
            endpoint_id: endpoint_id.to_owned(),
        }
        .cleaned()
    }

    /// The same, its words one line each and capped (also what was kept
    /// before they were).
    fn cleaned(self) -> PeerRecord {
        PeerRecord {
            claktory: clean(&self.claktory, NAME_CHARS),
            owner_name: clean(&self.owner_name, NAME_CHARS),
            owner_email: clean(&self.owner_email, EMAIL_CHARS),
            project: clean(&self.project, NAME_CHARS),
            pipe_name: clean(&self.pipe_name, NAME_CHARS),
            purpose: clean(&self.purpose, PURPOSE_CHARS),
            endpoint_id: self.endpoint_id,
        }
    }

    fn to_proto(&self) -> v1::PipePeer {
        v1::PipePeer {
            claktory_name: self.claktory.clone(),
            owner_name: self.owner_name.clone(),
            owner_email: self.owner_email.clone(),
            verified: false,
            project_name: self.project.clone(),
            purpose: self.purpose.clone(),
        }
    }

    /// Who's on the other side, for sentences: "Ana", else their Claktory.
    pub(crate) fn who(&self) -> String {
        if !self.owner_name.is_empty() {
            self.owner_name.clone()
        } else if !self.claktory.is_empty() {
            self.claktory.clone()
        } else {
            "the other side".to_owned()
        }
    }
}

/// A pipe as kept.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct Record {
    pub id: String,
    pub name: String,
    pub project: String,
    pub state: State,
    pub paused: Pause,
    pub created: i64,
    pub peer: Option<PeerRecord>,
    pub purpose: String,
    pub prompt: String,
    pub members: Vec<String>,
    pub lead: String,
    pub limits: Limits,
    /// Review outgoing: agents' messages wait for the person's Send.
    pub review: bool,
    /// Review incoming: the other side's messages wait for the person's
    /// Let in before any agent reads them.
    pub review_incoming: bool,
    pub digits: String,
    pub invite_expires: i64,
    /// This side's person approved it; the other side's did.
    pub approved: bool,
    pub peer_approved: bool,
    pub unseen: bool,
    /// Its folder in the project's, `pipes/<folder>`, with outbox and inbox.
    pub folder: String,
    /// When the lead was last woken, and the last incoming message it was told of.
    pub woke: i64,
    pub told: u64,
    /// Incoming messages let in after the lead was told of later ones: it's
    /// told of them at its next wake all the same.
    pub untold: Vec<u64>,
    /// Agents' messages in a row with no person writing on either side.
    pub quiet: u32,
    /// Hashes of the latest agents' messages, to tell repeats.
    pub recent: Vec<u64>,
    pub next_seq: u64,
    /// Members quarantined for it (`quarantine`): the person's choice alone.
    pub quarantined: Vec<String>,
    /// Quarantine came to this pipe: made since, or its liaisons were
    /// quarantined once at start (a person's release after stays).
    pub quarantine_set: bool,
    /// The agent bound to it (`bound`): its liaison, shown in it rather
    /// than beside it; empty when none.
    pub bound: String,
    /// Binding came to this pipe: made since, or its liaison was bound once
    /// at start.
    pub bind_set: bool,
    /// Who chose its name: true when the person typed it (an invite, a join
    /// with a name, a rename), false when it came from the other side (a
    /// join without one). None in pipes from before it: those with a peer
    /// count as named by the other side. Agents that list pipes (Rivet's
    /// pipes_list) never see a name the other side chose.
    pub named_here: Option<bool>,
    /// While JOINING: why the other Claktory can't be reached yet, in a
    /// short line, and since when (the join keeps retrying); empty once
    /// it connects. Runtime state, not kept across restarts.
    #[serde(skip)]
    pub offline_reason: String,
    #[serde(skip)]
    pub offline_since: i64,
}

impl Record {
    /// Agents may use it: approved here, and not closed.
    pub(crate) fn granted(&self) -> bool {
        matches!(self.state, State::WaitingPeer | State::Open | State::Paused)
    }

    pub(crate) fn peer_who(&self) -> String {
        self.peer.as_ref().map_or_else(|| "the other side".to_owned(), PeerRecord::who)
    }

    /// Members that can run commands on this computer: every one not
    /// quarantined.
    pub(crate) fn can_run_commands(&self) -> Vec<String> {
        self.members.iter().filter(|id| !self.quarantined.contains(id)).cloned().collect()
    }

    pub(crate) fn peer_claktory(&self) -> String {
        self.peer
            .as_ref()
            .map(|peer| peer.claktory.clone())
            .filter(|name| !name.is_empty())
            .unwrap_or_else(|| "another Claktory".to_owned())
    }
}

/// A tainted agent's pipes, each with the route it came by: pipe id → the
/// agents, by id, what it read came through (first the one that read the
/// pipe; empty when it read it itself). Its JSON, `{"<pipe id>": ["<agent
/// id>", …]}`, is how it's kept with what carries it (a monitor), and stays
/// as it is; pipe_taint.via is one route of it.
pub(crate) type Taint = BTreeMap<String, Vec<String>>;

#[derive(Default)]
struct Data {
    pipes: BTreeMap<String, Record>,
    selected: Option<String>,
    /// Agents that have read pipes in their current context: agent → pipe
    /// → the agents it came through, by id (empty when it read it itself).
    tainted: BTreeMap<String, Taint>,
    /// Pipes whose lead has a wake coming.
    waking: BTreeSet<String>,
    loaded: bool,
}

/// What the store keeps in memory: every pipe, which is shown, and who's tainted.
pub(crate) struct Pipes {
    data: Mutex<Data>,
    /// How long the lead's wakes are apart, at least (a minute); tests make it shorter.
    wake_every_ms: AtomicU64,
}

impl Default for Pipes {
    fn default() -> Pipes {
        Pipes { data: Mutex::default(), wake_every_ms: AtomicU64::new(60_000) }
    }
}

impl Pipes {
    fn data(&self) -> MutexGuard<'_, Data> {
        self.data.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// The pipe, as it is now.
    pub(crate) fn get(&self, id: &str) -> Option<Record> {
        self.data().pipes.get(id).cloned()
    }

    pub(crate) fn all(&self) -> Vec<Record> {
        self.data().pipes.values().cloned().collect()
    }

    /// The pipes `agent` has read in its current context, by id.
    pub(crate) fn tainted(&self, agent: &str) -> Vec<String> {
        self.data().tainted.get(agent).map(|pipes| pipes.keys().cloned().collect()).unwrap_or_default()
    }

    /// Whether `agent` is quarantined for a pipe (`quarantine`).
    pub(crate) fn quarantined(&self, agent: &str) -> bool {
        self.data().pipes.values().any(|record| record.quarantined.iter().any(|id| id == agent))
    }

    /// The same, with the agents each came through.
    pub(crate) fn taint_of(&self, agent: &str) -> Taint {
        self.data().tainted.get(agent).cloned().unwrap_or_default()
    }

    pub(crate) fn selected(&self) -> Option<String> {
        self.data().selected.clone()
    }

    /// An agent's conversation is shown instead.
    pub(crate) fn deselect(&self) {
        self.data().selected = None;
    }

    /// The pipe is shown.
    pub(crate) fn show(&self, id: &str) {
        self.data().selected = Some(id.to_owned());
    }
}

/// The transport's way back in, for every Factory.
struct Link;

/// The store the transport tells what happens (unless a test registers its own).
pub(crate) fn link() -> &'static dyn pipe_link::Store {
    &Link
}

/// Loads the pipes; at start.
pub(crate) fn start(factory: &Arc<Factory>) {
    let loaded = load(factory);
    let mut data = factory.pipes.data();
    match loaded {
        Ok((pipes, tainted)) => {
            data.pipes = pipes;
            data.tainted = tainted;
        }
        Err(error) => tracing::error!(%error, "could not read the pipes"),
    }
    data.loaded = true;
    // Invites don't outlive a restart's ten minutes for long; those expired meanwhile close.
    let now = now_ms();
    let expired: Vec<String> = data
        .pipes
        .values()
        .filter(|pipe| pipe.state == State::Invited && pipe.invite_expires > 0 && pipe.invite_expires < now)
        .map(|pipe| pipe.id.clone())
        .collect();
    drop(data);
    for id in expired {
        close_with(factory, &id, "The invite expired before anyone used it.");
    }
    quarantine::upgrade(factory);
    bound::upgrade(factory);
    if let Ok(runtime) = tokio::runtime::Handle::try_current() {
        runtime.spawn(start::expire(Arc::downgrade(factory)));
    }
}

type Loaded = (BTreeMap<String, Record>, BTreeMap<String, Taint>);

fn load(factory: &Factory) -> rusqlite::Result<Loaded> {
    let db = factory.store.db();
    let mut pipes = BTreeMap::new();
    let mut statement = db.prepare("SELECT id, data FROM pipes")?;
    for row in statement.query_map([], |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)))? {
        let (id, data) = row?;
        match serde_json::from_str::<Record>(&data) {
            Ok(mut record) => {
                record.peer = record.peer.map(PeerRecord::cleaned);
                pipes.insert(id, record);
            }
            Err(error) => tracing::error!(pipe = %id, %error, "could not read a pipe"),
        }
    }
    let mut tainted: BTreeMap<String, Taint> = BTreeMap::new();
    let mut statement = db.prepare("SELECT agent, pipe, via FROM pipe_taint")?;
    let rows = statement.query_map([], |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?, row.get::<_, Option<String>>(2)?)))?;
    for row in rows {
        let (agent, pipe, via) = row?;
        let via: Vec<String> = via.and_then(|via| serde_json::from_str(&via).ok()).unwrap_or_default();
        tainted.entry(agent).or_default().insert(pipe, via);
    }
    Ok((pipes, tainted))
}

fn save(factory: &Factory, record: &Record) {
    let data = match serde_json::to_string(record) {
        Ok(data) => data,
        Err(error) => return tracing::error!(pipe = %record.id, %error, "could not write a pipe"),
    };
    if let Err(error) = factory.store.db().execute("INSERT OR REPLACE INTO pipes (id, data) VALUES (?1, ?2)", params![record.id, data]) {
        tracing::error!(pipe = %record.id, %error, "could not save a pipe");
    }
}

/// Changes a pipe, keeps it and tells every client; None when it's gone.
pub(crate) fn change<R>(factory: &Factory, id: &str, edit: impl FnOnce(&mut Record) -> R) -> Option<R> {
    let (result, record) = {
        let mut data = factory.pipes.data();
        let record = data.pipes.get_mut(id)?;
        let result = edit(record);
        (result, record.clone())
    };
    save(factory, &record);
    factory.broadcast_workspace(&factory.lock());
    Some(result)
}

fn random_hex(bytes: usize) -> String {
    let mut random = vec![0u8; bytes];
    getrandom::fill(&mut random).expect("the system's random source");
    crate::encoding::hex(&random)
}

/// A name for a folder: letters, digits and dashes.
fn slug(name: &str) -> String {
    let mut slug = String::new();
    for character in name.chars() {
        if character.is_alphanumeric() {
            slug.extend(character.to_lowercase());
        } else if !slug.is_empty() && !slug.ends_with('-') {
            slug.push('-');
        }
    }
    let slug: String = slug.trim_end_matches('-').chars().take(40).collect();
    if slug.is_empty() { "pipe".to_owned() } else { slug }
}

/// The pipe's folder in its project's: `pipes/<folder>`, with `outbox` (what
/// its agents may send) and `inbox` (what arrives).
pub(crate) fn folder(factory: &Factory, record: &Record) -> Option<PathBuf> {
    folder_of(&factory.lock().config, record)
}

pub(crate) fn folder_of(config: &crate::config::Config, record: &Record) -> Option<PathBuf> {
    let project = config.project(&record.project)?;
    Some(project.path.join("pipes").join(&record.folder))
}

// ---- Messages ----

/// One message or Claktory line, as kept.
#[derive(Clone, Debug, Default)]
pub(crate) struct Message {
    pub seq: u64,
    pub msg_id: String,
    pub created: i64,
    pub author: Author,
    pub name: String,
    pub role: String,
    pub agent: String,
    pub text: String,
    pub files: Vec<FileRecord>,
    pub state: MessageState,
    pub note: String,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum Author {
    TheirAgent,
    TheirPerson,
    MyAgent,
    Me,
    #[default]
    Claktory,
}

impl Author {
    fn as_str(self) -> &'static str {
        match self {
            Author::TheirAgent => "their_agent",
            Author::TheirPerson => "their_person",
            Author::MyAgent => "my_agent",
            Author::Me => "me",
            Author::Claktory => "claktory",
        }
    }

    fn parse(text: &str) -> Author {
        match text {
            "their_agent" => Author::TheirAgent,
            "their_person" => Author::TheirPerson,
            "my_agent" => Author::MyAgent,
            "me" => Author::Me,
            _ => Author::Claktory,
        }
    }

    pub(crate) fn incoming(self) -> bool {
        matches!(self, Author::TheirAgent | Author::TheirPerson)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum MessageState {
    Held,
    Queued,
    Delivered,
    Refused,
    Discarded,
    Received,
    HeldIncoming,
    #[default]
    Line,
}

impl MessageState {
    fn as_str(self) -> &'static str {
        match self {
            MessageState::Held => "held",
            MessageState::Queued => "queued",
            MessageState::Delivered => "delivered",
            MessageState::Refused => "refused",
            MessageState::Discarded => "discarded",
            MessageState::Received => "received",
            MessageState::HeldIncoming => "held_incoming",
            MessageState::Line => "line",
        }
    }

    fn parse(text: &str) -> MessageState {
        match text {
            "held" => MessageState::Held,
            "queued" => MessageState::Queued,
            "delivered" => MessageState::Delivered,
            "refused" => MessageState::Refused,
            "discarded" => MessageState::Discarded,
            "received" => MessageState::Received,
            "held_incoming" => MessageState::HeldIncoming,
            _ => MessageState::Line,
        }
    }

    fn to_proto(self) -> v1::PipeMessageState {
        match self {
            MessageState::Held => v1::PipeMessageState::Held,
            MessageState::Queued => v1::PipeMessageState::Queued,
            MessageState::Delivered => v1::PipeMessageState::Delivered,
            MessageState::Refused => v1::PipeMessageState::Refused,
            MessageState::Discarded => v1::PipeMessageState::Discarded,
            MessageState::Received => v1::PipeMessageState::Received,
            MessageState::HeldIncoming => v1::PipeMessageState::HeldIncoming,
            MessageState::Line => v1::PipeMessageState::Unspecified,
        }
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct FileRecord {
    pub name: String,
    pub size: u64,
    pub path: PathBuf,
    /// An outgoing file's SHA-256 when it was scanned (pipe_send): what
    /// goes must be exactly that. Empty for incoming files, and for
    /// messages kept before it was recorded.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub sha256: String,
}

impl Message {
    fn to_proto(&self) -> v1::PipeMessage {
        let kind = match self.author {
            Author::TheirAgent => v1::PipeAuthorKind::TheirAgent,
            Author::TheirPerson => v1::PipeAuthorKind::TheirPerson,
            Author::MyAgent => v1::PipeAuthorKind::MyAgent,
            Author::Me => v1::PipeAuthorKind::Me,
            Author::Claktory => v1::PipeAuthorKind::Claktory,
        };
        v1::PipeMessage {
            seq: self.seq,
            timestamp_ms: self.created,
            author: Some(v1::PipeAuthor {
                kind: kind.into(),
                name: self.name.clone(),
                role: self.role.clone(),
                agent_id: self.agent.clone(),
            }),
            text: self.text.clone(),
            files: self
                .files
                .iter()
                .map(|file| v1::PipeFile { name: file.name.clone(), size: file.size, path: file.path.display().to_string() })
                .collect(),
            state: self.state.to_proto().into(),
            note: self.note.clone(),
        }
    }
}

const COLUMNS: &str = "seq, msg_id, created, author, name, role, agent, text, files, state, note";

fn read_message(row: &rusqlite::Row<'_>) -> rusqlite::Result<Message> {
    let files: String = row.get(8)?;
    Ok(Message {
        seq: row.get::<_, i64>(0)?.try_into().unwrap_or_default(),
        msg_id: row.get(1)?,
        created: row.get(2)?,
        author: Author::parse(&row.get::<_, String>(3)?),
        name: row.get(4)?,
        role: row.get(5)?,
        agent: row.get(6)?,
        text: row.get(7)?,
        files: serde_json::from_str(&files).unwrap_or_default(),
        state: MessageState::parse(&row.get::<_, String>(9)?),
        note: row.get(10)?,
    })
}

/// Adds a message to the pipe (its seq is the pipe's next), keeps it and
/// shows it; returns it as kept.
pub(crate) fn add(factory: &Factory, pipe: &str, mut message: Message) -> Option<Message> {
    let seq = {
        let mut data = factory.pipes.data();
        let record = data.pipes.get_mut(pipe)?;
        if record.next_seq == 0 {
            record.next_seq = 1;
        }
        let seq = record.next_seq;
        record.next_seq += 1;
        // Something new: unless it's being looked at, or it's the person's own.
        let shown = data.selected.as_deref() == Some(pipe);
        let record = data.pipes.get_mut(pipe)?;
        if !shown && message.author != Author::Me {
            record.unseen = true;
        }
        seq
    };
    message.seq = seq;
    if message.created == 0 {
        message.created = now_ms();
    }
    let files = serde_json::to_string(&message.files).unwrap_or_else(|_| "[]".to_owned());
    let stored = factory.store.db().execute(
        &format!("INSERT INTO pipe_messages (pipe, {COLUMNS}) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)"),
        params![
            pipe,
            i64::try_from(seq).unwrap_or(i64::MAX),
            message.msg_id,
            message.created,
            message.author.as_str(),
            message.name,
            message.role,
            message.agent,
            message.text,
            files,
            message.state.as_str(),
            message.note
        ],
    );
    if let Err(error) = stored {
        tracing::error!(%pipe, %error, "could not keep a pipe's message");
        return None;
    }
    if let Some(record) = factory.pipes.get(pipe) {
        save(factory, &record);
    }
    shown(factory, pipe, &message);
    Some(message)
}

/// A line from Claktory in the pipe's chat.
pub(crate) fn line(factory: &Factory, pipe: &str, text: impl Into<String>) {
    add(factory, pipe, Message { text: text.into(), ..Message::default() });
}

/// Changes a message's state (and note); returns it as it is now.
pub(crate) fn set_state(factory: &Factory, pipe: &str, seq: u64, state: MessageState, note: Option<&str>) -> Option<Message> {
    {
        let db = factory.store.db();
        let seq = i64::try_from(seq).unwrap_or(i64::MAX);
        let changed = match note {
            Some(note) => db.execute(
                "UPDATE pipe_messages SET state = ?3, note = ?4 WHERE pipe = ?1 AND seq = ?2",
                params![pipe, seq, state.as_str(), note],
            ),
            None => db.execute("UPDATE pipe_messages SET state = ?3 WHERE pipe = ?1 AND seq = ?2", params![pipe, seq, state.as_str()]),
        };
        if let Err(error) = changed {
            tracing::error!(%pipe, %error, "could not change a pipe's message");
            return None;
        }
    }
    let message = message(factory, pipe, seq)?;
    shown(factory, pipe, &message);
    factory.broadcast_workspace(&factory.lock());
    Some(message)
}

pub(crate) fn message(factory: &Factory, pipe: &str, seq: u64) -> Option<Message> {
    let db = factory.store.db();
    db.query_row(
        &format!("SELECT {COLUMNS} FROM pipe_messages WHERE pipe = ?1 AND seq = ?2"),
        params![pipe, i64::try_from(seq).unwrap_or(i64::MAX)],
        read_message,
    )
    .optional()
    .ok()
    .flatten()
}

fn by_msg_id(factory: &Factory, pipe: &str, msg_id: &str) -> Option<Message> {
    let db = factory.store.db();
    db.query_row(&format!("SELECT {COLUMNS} FROM pipe_messages WHERE pipe = ?1 AND msg_id = ?2"), params![pipe, msg_id], read_message)
        .optional()
        .ok()
        .flatten()
}

/// Messages of the pipe before `before` (0: from the end), at most `limit`,
/// oldest first; and whether there are earlier ones.
pub(crate) fn messages(factory: &Factory, pipe: &str, before: u64, limit: u32) -> (Vec<Message>, bool) {
    let limit = if limit == 0 { 200 } else { limit.min(1000) };
    let before = if before == 0 { i64::MAX } else { i64::try_from(before).unwrap_or(i64::MAX) };
    let db = factory.store.db();
    let read = || -> rusqlite::Result<Vec<Message>> {
        let mut statement =
            db.prepare(&format!("SELECT {COLUMNS} FROM pipe_messages WHERE pipe = ?1 AND seq < ?2 ORDER BY seq DESC LIMIT ?3"))?;
        let rows = statement.query_map(params![pipe, before, i64::from(limit) + 1], read_message)?;
        rows.collect()
    };
    let mut found = read().unwrap_or_else(|error| {
        tracing::error!(%pipe, %error, "could not read a pipe's messages");
        Vec::new()
    });
    let earlier = found.len() > limit as usize;
    found.truncate(limit as usize);
    found.reverse();
    (found, earlier)
}

/// How many of the other side's messages wait for the person to let them in.
pub(crate) fn waiting_incoming(factory: &Factory, pipe: &str) -> u32 {
    counted(factory, pipe, &[Author::TheirAgent, Author::TheirPerson], &[MessageState::HeldIncoming], i64::MAX / 2)
}

/// The pipe's messages that wait for the person, by seq.
fn waiting(factory: &Factory, pipe: &str) -> Vec<u64> {
    let db = factory.store.db();
    let read = || -> rusqlite::Result<Vec<u64>> {
        let mut statement =
            db.prepare("SELECT seq FROM pipe_messages WHERE pipe = ?1 AND state IN ('held', 'held_incoming') ORDER BY seq")?;
        let rows = statement.query_map([pipe], |row| row.get::<_, i64>(0))?;
        rows.map(|seq| seq.map(|seq| u64::try_from(seq).unwrap_or_default())).collect()
    };
    read().unwrap_or_default()
}

/// Sends the message to clients showing the pipe.
fn shown(factory: &Factory, pipe: &str, message: &Message) {
    factory.send(Body::PipeMessageUpdate(v1::PipeMessageUpdate { pipe_id: pipe.to_owned(), message: Some(message.to_proto()) }));
}

// ---- The Workspace ----

/// The pipes, as every client shows them.
pub(crate) fn view(factory: &Factory, inner: &Inner) -> (Vec<v1::Pipe>, String) {
    let (records, selected) = {
        let data = factory.pipes.data();
        (data.pipes.values().cloned().collect::<Vec<_>>(), data.selected.clone().unwrap_or_default())
    };
    let mut pipes: Vec<v1::Pipe> = records
        .iter()
        .filter(|record| inner.config.project(&record.project).is_some())
        .map(|record| {
            let link = if record.state == State::Closed { LinkStatus::default() } else { pipe_link::status(factory, &record.id) };
            let (sent, mb) = used(factory, &record.id);
            let members: Vec<String> = record.members.iter().filter(|id| inner.config.agent(id).is_some()).cloned().collect();
            let lead = if members.contains(&record.lead) { record.lead.clone() } else { String::new() };
            v1::Pipe {
                id: record.id.clone(),
                name: record.name.clone(),
                project_id: record.project.clone(),
                state: state_proto(record.state).into(),
                paused: pause_proto(record.paused).into(),
                created_ms: record.created,
                peer: record.peer.as_ref().map(PeerRecord::to_proto),
                purpose: record.purpose.clone(),
                prompt: record.prompt.clone(),
                lead_id: lead,
                limits: Some(v1::PipeLimits {
                    messages_per_hour: record.limits.per_hour,
                    burst: record.limits.burst,
                    file_mb_per_day: record.limits.file_mb,
                    messages_left: record.limits.per_hour.saturating_sub(sent),
                    file_mb_left: record.limits.file_mb.saturating_sub(mb),
                }),
                review_outgoing: record.review,
                review_incoming: record.review_incoming,
                folder: record.folder.clone(),
                suggested_liaison_name: if record.state == State::Closed { String::new() } else { bound::suggest(&inner.config, record) },
                digits: if record.state == State::Approval { record.digits.clone() } else { String::new() },
                invite_expires_ms: if record.state == State::Invited { record.invite_expires } else { 0 },
                online: link.online,
                last_seen_ms: link.last_seen_ms,
                queued: link.queued,
                unseen: record.unseen,
                offline_reason: if record.state == State::Joining { record.offline_reason.clone() } else { link.reason.clone() },
                offline_since_ms: if record.state == State::Joining { record.offline_since } else { link.offline_since_ms },
                waiting_seqs: waiting(factory, &record.id),
                quarantined_ids: record.quarantined.iter().filter(|id| members.contains(id)).cloned().collect(),
                can_run_commands_ids: members.iter().filter(|id| !record.quarantined.contains(id)).cloned().collect(),
                member_ids: members,
            }
        })
        .collect();
    pipes.sort_by_key(|pipe| pipe.created_ms);
    let selected = if pipes.iter().any(|pipe| pipe.id == selected) { selected } else { String::new() };
    (pipes, selected)
}

fn state_proto(state: State) -> v1::PipeState {
    match state {
        State::Invited => v1::PipeState::Invited,
        State::Joining => v1::PipeState::Joining,
        State::Approval => v1::PipeState::Approval,
        State::WaitingPeer => v1::PipeState::WaitingPeer,
        State::Open => v1::PipeState::Open,
        State::Paused => v1::PipeState::Paused,
        State::Closed => v1::PipeState::Closed,
    }
}

fn pause_proto(pause: Pause) -> v1::PipePause {
    match pause {
        Pause::None => v1::PipePause::Unspecified,
        Pause::Me => v1::PipePause::ByMe,
        Pause::Peer => v1::PipePause::ByPeer,
        Pause::Loop => v1::PipePause::Loop,
    }
}

// ---- Limits ----

/// This side's agents' messages sent in the last hour, and the megabytes of
/// files sent in the last day.
fn used(factory: &Factory, pipe: &str) -> (u32, u32) {
    let now = now_ms();
    let db = factory.store.db();
    let sent: i64 = db
        .query_row(
            "SELECT COUNT(*) FROM pipe_messages WHERE pipe = ?1 AND author = 'my_agent' AND state IN ('queued', 'delivered') AND created > ?2",
            params![pipe, now - 3_600_000],
            |row| row.get(0),
        )
        .unwrap_or(0);
    let mut bytes = 0u64;
    if let Ok(mut statement) = db.prepare(
        "SELECT files FROM pipe_messages WHERE pipe = ?1 AND author IN ('my_agent', 'me') AND state IN ('queued', 'delivered') AND created > ?2",
    ) && let Ok(rows) = statement.query_map(params![pipe, now - 86_400_000], |row| row.get::<_, String>(0))
    {
        for files in rows.flatten() {
            let files: Vec<FileRecord> = serde_json::from_str(&files).unwrap_or_default();
            bytes += files.iter().map(|file| file.size).sum::<u64>();
        }
    }
    (u32::try_from(sent).unwrap_or(u32::MAX), u32::try_from(bytes.div_ceil(1_048_576)).unwrap_or(u32::MAX))
}

/// Messages by `authors` in the last `window_ms` that count against the limits.
fn counted(factory: &Factory, pipe: &str, authors: &[Author], states: &[MessageState], window_ms: i64) -> u32 {
    let authors: Vec<String> = authors.iter().map(|author| format!("'{}'", author.as_str())).collect();
    let states: Vec<String> = states.iter().map(|state| format!("'{}'", state.as_str())).collect();
    let query = format!(
        "SELECT COUNT(*) FROM pipe_messages WHERE pipe = ?1 AND author IN ({}) AND state IN ({}) AND created > ?2",
        authors.join(", "),
        states.join(", ")
    );
    let count: i64 = factory.store.db().query_row(&query, params![pipe, now_ms() - window_ms], |row| row.get(0)).unwrap_or(0);
    u32::try_from(count).unwrap_or(u32::MAX)
}

/// Why an agent's message would go over the pipe's limits; None if it wouldn't.
fn over_outgoing(factory: &Factory, record: &Record, file_bytes: u64) -> Option<String> {
    let sent = [MessageState::Queued, MessageState::Delivered, MessageState::Held];
    if counted(factory, &record.id, &[Author::MyAgent], &sent, 3_600_000) >= record.limits.per_hour {
        return Some(format!("Over the pipe's limit of {} messages an hour.", record.limits.per_hour));
    }
    if counted(factory, &record.id, &[Author::MyAgent], &sent, 60_000) >= record.limits.burst {
        return Some(format!("Over the pipe's limit of {} messages a minute.", record.limits.burst));
    }
    let (_, mb) = used(factory, &record.id);
    if u64::from(mb) * 1_048_576 + file_bytes > u64::from(record.limits.file_mb) * 1_048_576 {
        return Some(format!("Over the pipe's limit of {} MB of files a day.", record.limits.file_mb));
    }
    None
}

fn over_incoming(factory: &Factory, record: &Record) -> Option<String> {
    let arrived = [MessageState::Received, MessageState::HeldIncoming];
    let theirs = [Author::TheirAgent, Author::TheirPerson];
    if counted(factory, &record.id, &theirs, &arrived, 3_600_000) >= record.limits.per_hour {
        return Some(format!("More than {} messages arrived in an hour: let it in, or discard it.", record.limits.per_hour));
    }
    if counted(factory, &record.id, &theirs, &arrived, 60_000) >= record.limits.burst {
        return Some(format!("More than {} messages arrived in a minute: let it in, or discard it.", record.limits.burst));
    }
    None
}

/// How many agents' messages in a row pause the pipe.
const LOOP_AFTER: u32 = 10;

/// Counts a message towards the loop breaker: this side's person's resets
/// it (the other side's can't: it could claim any message is its
/// person's); ten others in a row, or the same text three times lately,
/// pause the pipe.
pub(crate) fn count_for_loop(factory: &Arc<Factory>, pipe: &str, author: Author, text: &str) {
    use std::hash::{Hash, Hasher};
    let tripped = {
        let mut data = factory.pipes.data();
        let Some(record) = data.pipes.get_mut(pipe) else { return };
        if author == Author::Me {
            record.quiet = 0;
            record.recent.clear();
            false
        } else {
            record.quiet += 1;
            let mut hasher = std::collections::hash_map::DefaultHasher::new();
            text.trim().to_lowercase().hash(&mut hasher);
            let hash = hasher.finish();
            let repeats = record.recent.iter().filter(|seen| **seen == hash).count();
            record.recent.push(hash);
            if record.recent.len() > 6 {
                record.recent.remove(0);
            }
            (record.quiet >= LOOP_AFTER || repeats >= 2) && record.state == State::Open
        }
    };
    if tripped {
        let who = factory.pipes.get(pipe).map(|record| record.peer_who()).unwrap_or_default();
        let paused = change(factory, pipe, |record| {
            record.state = State::Paused;
            record.paused = Pause::Loop;
        });
        if paused.is_some() {
            if let Err(error) = pipe_link::pause(factory, pipe) {
                tracing::warn!(%pipe, %error, "could not tell the other side the pipe paused");
            }
            line(
                factory,
                pipe,
                format!(
                    "Your agents and {who}'s have exchanged {LOOP_AFTER} messages on their own, or repeated themselves, so the pipe is paused. Continue?"
                ),
            );
        }
    } else if let Some(record) = factory.pipes.get(pipe) {
        save(factory, &record);
    }
}

// ---- Requests from the app ----

/// This side, as the other sees it.
fn mine(factory: &Factory, project: &str, name: &str, purpose: &str) -> Side {
    let inner = factory.lock();
    let config = &inner.config;
    let email = factory.account.signed_in_as(&config.account).map(|(_, email, _)| email).unwrap_or_default();
    Side {
        claktory: config.claktory_name(),
        owner_name: config.user_name().unwrap_or_default().to_owned(),
        owner_email: email,
        project: config.project(project).map(|project| project.name.clone()).unwrap_or_default(),
        pipe_name: name.to_owned(),
        purpose: purpose.to_owned(),
    }
}

fn new_record(factory: &Factory, project: &str, name: &str, purpose: &str, state: State) -> Result<Record, RequestError> {
    let name = name.trim();
    if name.is_empty() {
        return Err(RequestError::Empty("name"));
    }
    if factory.lock().config.project(project).is_none() {
        return Err(RequestError::UnknownProject(project.to_owned()));
    }
    let id = random_hex(16);
    Ok(Record {
        folder: format!("{}-{}", slug(name), &id[..6]),
        id,
        name: name.to_owned(),
        project: project.to_owned(),
        state,
        created: now_ms(),
        purpose: purpose.trim().to_owned(),
        next_seq: 1,
        quarantine_set: true,
        bind_set: true,
        named_here: Some(true),
        ..Record::default()
    })
}

fn insert(factory: &Factory, record: Record) {
    save(factory, &record);
    factory.pipes.data().pipes.insert(record.id.clone(), record);
    factory.broadcast_workspace(&factory.lock());
}

fn pipe_error(error: PipeError) -> RequestError {
    match error {
        PipeError::Unreachable(_) => RequestError::Unreachable(error.to_string()),
        other => RequestError::Pipe(other.to_string()),
    }
}

fn known(factory: &Factory, id: &str) -> Result<Record, RequestError> {
    factory.pipes.get(id).ok_or_else(|| RequestError::NoPipe(id.to_owned()))
}

/// CreatePipeInvite: a pipe from `project`, INVITED, and its code.
pub(crate) async fn create_invite(factory: &Arc<Factory>, request: &v1::CreatePipeInvite) -> Result<v1::PipeInvite, RequestError> {
    // From an agent's card (#244): only while it waits.
    if !request.start_id.is_empty() {
        start::check(factory, &request.start_id, v1::PipeStartKind::Invite)?;
    }
    let mut record = new_record(factory, &request.project_id, &request.name, &request.purpose, State::Invited)?;
    let side = mine(factory, &record.project, &record.name, &record.purpose);
    let (code, expires) = pipe_link::invite(factory, &record.id, &side).await.map_err(pipe_error)?;
    record.invite_expires = expires;
    let id = record.id.clone();
    let made = record.clone();
    insert(factory, record);
    line(factory, &id, "Invite made. Send its code to the other person; it works once, for 10 minutes.");
    if !request.start_id.is_empty() {
        start::done(factory, &request.start_id, &made, true);
    }
    Ok(v1::PipeInvite { pipe_id: id, code })
}

/// ReadPipeInvite: what a code says, without using it.
pub(crate) fn read_invite(code: &str) -> Result<v1::PipeInvitePreview, RequestError> {
    let side = pipe_link::read_invite(code.trim()).map_err(pipe_error)?;
    let peer = PeerRecord::from_side(&side, "");
    Ok(v1::PipeInvitePreview { code: code.trim().to_owned(), peer: Some(peer.to_proto()), name: peer.pipe_name, expires_ms: 0 })
}

/// AcceptPipeInvite: a pipe to `project`, JOINING and selected, then
/// APPROVAL once the other Claktory answers.
pub(crate) fn accept(factory: &Arc<Factory>, request: &v1::AcceptPipeInvite) -> Result<(), RequestError> {
    let code = request.code.trim().to_owned();
    if !request.start_id.is_empty() {
        start::check(factory, &request.start_id, v1::PipeStartKind::Accept)?;
    }
    let side = pipe_link::read_invite(&code).map_err(pipe_error)?;
    let peer = PeerRecord::from_side(&side, "");
    let name = if request.name.trim().is_empty() { peer.pipe_name.clone() } else { request.name.clone() };
    let mut record = new_record(factory, &request.project_id, &name, &request.purpose, State::Joining)?;
    record.named_here = Some(!request.name.trim().is_empty());
    let reaching = peer.claktory.clone();
    record.peer = Some(peer);
    let id = record.id.clone();
    let mine = mine(factory, &record.project, &record.name, &record.purpose);
    let made = record.clone();
    insert(factory, record);
    if !request.start_id.is_empty() {
        // Named only if this side chose the name: the inviter's isn't repeated to an agent.
        start::done(factory, &request.start_id, &made, !request.name.trim().is_empty());
    }
    factory.pipes.data().selected = Some(id.clone());
    factory.broadcast_workspace(&factory.lock());
    send_transcript(factory, &id);
    line(factory, &id, format!("Reaching {reaching}…"));
    let factory = factory.clone();
    tokio::spawn(async move {
        // The first dial can fail for a passing reason: the other machine's
        // way there isn't up yet, or macOS is still asking its person about
        // the local network. Unreachable retries until the code's own
        // expiry ends it (join refuses an expired code); anything else is
        // definitive and closes the pipe as before.
        let mut backoff = std::time::Duration::from_secs(1);
        let mut said = String::new();
        loop {
            match pipe_link::join(&factory, &id, &code, &mine).await {
                Ok((peer, digits)) => {
                    change(&factory, &id, |record| {
                        record.peer = Some(PeerRecord::from_side(&peer.side, &peer.endpoint_id));
                        record.digits = digits;
                        record.offline_reason.clear();
                        record.offline_since = 0;
                        if record.state == State::Joining {
                            record.state = State::Approval;
                        }
                    });
                    line(&factory, &id, "Connected. Check that both screens show the same four digits, then approve or turn it down.");
                    return;
                }
                Err(PipeError::Unreachable(reason)) => {
                    if !factory.pipes.get(&id).is_some_and(|record| record.state == State::Joining) {
                        // Turned down or deleted meanwhile: nothing to reach.
                        return;
                    }
                    if said != reason {
                        change(&factory, &id, |record| {
                            record.offline_reason.clone_from(&reason);
                            if record.offline_since == 0 {
                                record.offline_since = now_ms();
                            }
                        });
                        line(&factory, &id, format!("Couldn't reach {reaching} yet ({reason}). Trying again…"));
                        said = reason;
                    }
                    tokio::time::sleep(backoff).await;
                    backoff = (backoff * 2).min(std::time::Duration::from_secs(30));
                }
                Err(error) => {
                    close_with(&factory, &id, &format!("Couldn't connect: {error}"));
                    return;
                }
            }
        }
    });
    Ok(())
}

/// ApprovePipe: the person approves the pipe, with its members and lead,
/// or turns it down.
pub(crate) async fn approve(factory: &Arc<Factory>, request: &v1::ApprovePipe) -> Result<(), RequestError> {
    let record = known(factory, &request.pipe_id)?;
    if record.state != State::Approval {
        return Err(RequestError::Pipe("This pipe isn't waiting for your approval.".to_owned()));
    }
    if !request.approve {
        pipe_link::close(factory, &record.id);
        close_with(factory, &record.id, "You turned it down.");
        return Ok(());
    }
    let mut members = checked_members(factory, &record, &request.member_ids)?;
    let mut quarantined = match &request.quarantined {
        Some(chosen) => checked_members(factory, &record, &chosen.agent_ids)?,
        None => Vec::new(),
    };
    if let Some(stray) = quarantined.iter().find(|id| !members.contains(id)) {
        return Err(RequestError::Pipe(format!("{stray} isn't one of this pipe's agents.")));
    }
    let mut lead = request.lead_id.trim().to_owned();
    if !request.liaison_name.trim().is_empty() {
        let liaison = make_liaison(factory, &record, request.liaison_name.trim()).await?;
        if !members.contains(&liaison) {
            members.push(liaison.clone());
        }
        // A liaison is quarantined from the moment it's made.
        quarantined.push(liaison.clone());
        change(factory, &record.id, |record| {
            record.members.push(liaison.clone());
            record.quarantined.push(liaison.clone());
            record.bound.clone_from(&liaison);
        });
        lead = liaison;
    }
    if !lead.is_empty() && !members.contains(&lead) {
        return Err(RequestError::Pipe("The lead must be one of the pipe's agents.".to_owned()));
    }
    if lead.is_empty() {
        return Err(RequestError::Pipe("Choose the agent that answers the pipe, or add a liaison agent.".to_owned()));
    }
    pipe_link::approve(factory, &record.id).map_err(pipe_error)?;
    let opened = change(factory, &record.id, |record| {
        record.members = members;
        record.lead = lead;
        if let Some(review) = request.review_incoming {
            record.review_incoming = review;
        }
        if let Some(review) = request.review_outgoing {
            record.review = review;
        }
        // Its new liaison's already are; the others' are told just below.
        record.quarantined.retain(|id| quarantined.contains(id));
        record.approved = true;
        record.state = if record.peer_approved { State::Open } else { State::WaitingPeer };
        record.state == State::Open
    })
    .unwrap_or(false);
    if let Some(record) = factory.pipes.get(&record.id) {
        make_folders(factory, &record);
    }
    quarantine::set(factory, &record.id, quarantined);
    if request.review_outgoing.is_none() {
        review_with_commands(factory, &record.id, &[]);
    }
    if opened {
        line(factory, &record.id, "You approved it, and so did the other side: the pipe is open.");
    } else {
        line(factory, &record.id, format!("You approved it. Waiting for {} to approve it too.", record.peer_who()));
    }
    Ok(())
}

/// The agents of the pipe's project among `ids`.
fn checked_members(factory: &Factory, record: &Record, ids: &[String]) -> Result<Vec<String>, RequestError> {
    let inner = factory.lock();
    let mut members = Vec::new();
    for id in ids {
        let Some(agent) = inner.config.agent(id) else { return Err(RequestError::UnknownAgent(id.clone())) };
        if agent.project.as_deref() != Some(record.project.as_str()) {
            return Err(RequestError::Pipe(format!("{} isn't in this pipe's project.", agent.name)));
        }
        if !members.contains(id) {
            members.push(id.clone());
        }
    }
    Ok(members)
}

fn make_folders(factory: &Factory, record: &Record) {
    let Some(folder) = folder(factory, record) else { return };
    for sub in ["outbox", "inbox"] {
        if let Err(error) = std::fs::create_dir_all(folder.join(sub)) {
            tracing::warn!(pipe = %record.id, %error, "could not make the pipe's folder");
        }
    }
}

/// A liaison agent for the pipe: in its project, with no messaging, no web,
/// no secrets and no checkout of its own; its id. The caller quarantines it.
async fn make_liaison(factory: &Arc<Factory>, record: &Record, name: &str) -> Result<String, RequestError> {
    let person = factory.lock().config.person().to_owned();
    let prompt = format!(
        "You are the liaison for the pipe \"{}\": you read what comes through it from the other Claktory and answer for this project, within what the pipe's prompt allows. Ask {person} when unsure.",
        record.name
    );
    let before: BTreeSet<String> = factory.lock().config.agents.iter().map(|agent| agent.id.clone()).collect();
    let selected = factory.pipes.selected();
    let request = v1::CreateAgent {
        project_id: record.project.clone(),
        name: name.to_owned(),
        prompt,
        role: "Liaison".to_owned(),
        ..Default::default()
    };
    factory.create_agent(&request).await?;
    let id = factory
        .lock()
        .config
        .agents
        .iter()
        .find(|agent| !before.contains(&agent.id))
        .map(|agent| agent.id.clone())
        .ok_or_else(|| RequestError::Pipe("The liaison agent wasn't made.".to_owned()))?;
    let liaison = id.clone();
    factory
        .edit_config(move |config| {
            for extension in
                [crate::extensions::agents::ID, crate::extensions::web::ID, crate::extensions::secrets::ID, crate::extensions::tree::ID]
            {
                let change = v1::ExtensionChange { id: extension.to_owned(), enabled: Some(false), ..Default::default() };
                crate::extensions::apply_change(config, &liaison, &change)?;
            }
            Ok(())
        })
        .await?;
    // Making it showed its conversation: the pipe stays shown.
    if let Some(selected) = selected {
        select(factory, &selected).await?;
    }
    Ok(id)
}

/// AddPipeLiaison: a liaison agent, the pipe's member and lead.
pub(crate) async fn add_liaison(factory: &Arc<Factory>, request: &v1::AddPipeLiaison) -> Result<(), RequestError> {
    let record = known(factory, &request.pipe_id)?;
    if record.state == State::Closed {
        return Err(RequestError::Pipe("This pipe is closed.".to_owned()));
    }
    let bound = factory.lock().config.agent(&record.bound).map(|agent| agent.name.clone());
    if let Some(name) = bound.filter(|_| record.members.contains(&record.bound)) {
        return Err(RequestError::Pipe(format!("This pipe already has its liaison, {name}.")));
    }
    // No name: the one the app suggests (Pipe.suggested_liaison_name).
    let name = match request.name.trim() {
        "" => bound::suggest(&factory.lock().config, &record),
        name => name.to_owned(),
    };
    let id = make_liaison(factory, &record, &name).await?;
    // Quarantined from the moment it's made: it's never told of it, as it
    // never had more.
    change(factory, &record.id, |record| {
        if !record.members.contains(&id) {
            record.members.push(id.clone());
        }
        if !record.quarantined.contains(&id) {
            record.quarantined.push(id.clone());
        }
        record.lead = id.clone();
        record.bound = id.clone();
    });
    make_folders(factory, &record);
    Ok(())
}

/// UpdatePipe: its name, prompt, purpose, members, lead, limits, review.
pub(crate) fn update(factory: &Arc<Factory>, request: &v1::UpdatePipe) -> Result<(), RequestError> {
    let record = known(factory, &request.pipe_id)?;
    if let Some(name) = &request.name
        && name.trim().is_empty()
    {
        return Err(RequestError::Empty("name"));
    }
    let members = match &request.members {
        Some(members) => Some(checked_members(factory, &record, &members.agent_ids)?),
        None => None,
    };
    let lead = request.lead_id.as_ref().map(|lead| lead.trim().to_owned());
    let after_members = members.clone().unwrap_or_else(|| record.members.clone());
    let quarantined = match &request.quarantined {
        Some(chosen) => Some(checked_members(factory, &record, &chosen.agent_ids)?),
        None => None,
    };
    if let Some(stray) = quarantined.iter().flatten().find(|id| !after_members.contains(id)) {
        return Err(RequestError::Pipe(format!("{stray} isn't one of this pipe's agents.")));
    }
    if let Some(lead) = &lead
        && !lead.is_empty()
        && !after_members.contains(lead)
    {
        return Err(RequestError::Pipe("The lead must be one of the pipe's agents.".to_owned()));
    }
    if let Some(limits) = &request.limits
        && (limits.messages_per_hour == 0 || limits.burst == 0)
    {
        return Err(RequestError::Pipe("A pipe's limits must allow at least one message.".to_owned()));
    }
    let review_off = request.review_outgoing == Some(false) && record.review;
    let incoming = request.review_incoming.filter(|review| *review != record.review_incoming);
    change(factory, &record.id, |record| {
        if let Some(name) = &request.name
            && name.trim() != record.name
        {
            record.name = name.trim().to_owned();
            record.named_here = Some(true);
        }
        if let Some(prompt) = &request.prompt {
            record.prompt = prompt.trim().to_owned();
        }
        if let Some(purpose) = &request.purpose {
            record.purpose = purpose.trim().to_owned();
        }
        if let Some(members) = members {
            record.members = members;
            // A lead that's no longer a member isn't the lead.
            if !record.members.contains(&record.lead) {
                record.lead.clear();
            }
        }
        if let Some(lead) = lead {
            record.lead = lead;
        }
        if let Some(limits) = &request.limits {
            record.limits = Limits { per_hour: limits.messages_per_hour, burst: limits.burst, file_mb: limits.file_mb_per_day.max(1) };
        }
        if let Some(review) = request.review_outgoing {
            record.review = review;
        }
        if let Some(review) = request.review_incoming {
            record.review_incoming = review;
        }
    });
    if review_off {
        line(factory, &record.id, "Review outgoing is off: agents' messages go out without waiting for you.");
    }
    match incoming {
        Some(true) => line(
            factory,
            &record.id,
            "Review incoming is on: each message from the other side waits for you to let it in before your agents read it.",
        ),
        Some(false) => line(
            factory,
            &record.id,
            "Review incoming is off: messages from the other side reach your agents as they arrive. Messages already waiting still wait for you.",
        ),
        None => {}
    }
    // Who's quarantined: as chosen, or as it was among the members left.
    let now = factory.pipes.get(&record.id).map(|record| record.quarantined).unwrap_or_default();
    quarantine::set(factory, &record.id, quarantined.unwrap_or(now));
    bound::unbind_if_gone(factory, &record);
    if request.review_outgoing.is_none() {
        review_with_commands(factory, &record.id, &record.can_run_commands());
    }
    Ok(())
}

/// Whether Review outgoing turns itself on when a member that can run
/// commands joins a pipe (Fugo's suggestion): the person's decision, off
/// until they make it.
const REVIEW_WITH_COMMANDS: bool = false;

/// With `REVIEW_WITH_COMMANDS`, turns Review outgoing on when the pipe has a
/// member that can run commands it didn't have (`before`), saying so.
fn review_with_commands(factory: &Factory, pipe: &str, before: &[String]) {
    if !REVIEW_WITH_COMMANDS {
        return;
    }
    let Some(record) = factory.pipes.get(pipe) else { return };
    if record.review || record.can_run_commands().iter().all(|id| before.contains(id)) {
        return;
    }
    change(factory, pipe, |record| record.review = true);
    line(
        factory,
        pipe,
        "Review outgoing is on because an agent of this pipe can run commands on this computer: each message from your agents waits for you to send it.",
    );
}

/// SetPipeState: pause, resume, close or delete.
pub(crate) fn set_pipe_state(factory: &Arc<Factory>, request: &v1::SetPipeState) -> Result<(), RequestError> {
    let record = known(factory, &request.pipe_id)?;
    let id = record.id.clone();
    match request.action() {
        v1::PipeAction::Pause => {
            if record.state != State::Open {
                return Err(RequestError::Pipe("Only an open pipe can be paused.".to_owned()));
            }
            pipe_link::pause(factory, &id).map_err(pipe_error)?;
            change(factory, &id, |record| {
                record.state = State::Paused;
                record.paused = Pause::Me;
            });
            line(factory, &id, "You paused it: nothing goes either way until you resume it.");
        }
        v1::PipeAction::Resume => {
            if record.state != State::Paused || record.paused == Pause::Peer {
                let why = if record.paused == Pause::Peer {
                    format!("{} paused it: only they can resume it.", record.peer_who())
                } else {
                    "This pipe isn't paused.".to_owned()
                };
                return Err(RequestError::Pipe(why));
            }
            pipe_link::resume(factory, &id).map_err(pipe_error)?;
            change(factory, &id, |record| {
                record.state = State::Open;
                record.paused = Pause::None;
                record.quiet = 0;
                record.recent.clear();
            });
            line(factory, &id, "You resumed it.");
        }
        v1::PipeAction::Close => {
            if record.state == State::Closed {
                return Ok(());
            }
            if record.state == State::Invited {
                pipe_link::cancel_invite(factory, &id);
            } else {
                pipe_link::close(factory, &id);
            }
            close_with(factory, &id, "You closed it.");
        }
        v1::PipeAction::Delete => {
            if record.state != State::Closed {
                return Err(RequestError::Pipe("Close the pipe before deleting it.".to_owned()));
            }
            bound::delete_with(factory, &record);
            delete(factory, &id);
        }
        v1::PipeAction::Unspecified => return Err(RequestError::Pipe("No action given.".to_owned())),
    }
    Ok(())
}

/// Closes the pipe here, with a line saying why.
pub(crate) fn close_with(factory: &Factory, id: &str, why: &str) {
    let closed = change(factory, id, |record| {
        let was = record.state;
        record.state = State::Closed;
        record.paused = Pause::None;
        was != State::Closed
    });
    if closed == Some(true) {
        line(factory, id, why);
    }
}

fn delete(factory: &Factory, id: &str) {
    {
        let mut data = factory.pipes.data();
        data.pipes.remove(id);
        if data.selected.as_deref() == Some(id) {
            data.selected = None;
        }
    }
    let db = factory.store.db();
    for statement in ["DELETE FROM pipes WHERE id = ?1", "DELETE FROM pipe_messages WHERE pipe = ?1"] {
        if let Err(error) = db.execute(statement, [id]) {
            tracing::error!(pipe = %id, %error, "could not delete a pipe");
        }
    }
    drop(db);
    factory.broadcast_workspace(&factory.lock());
}

/// A project is deleted: its pipes close.
pub(crate) fn project_gone(factory: &Arc<Factory>, project: &str) {
    for record in factory.pipes.all().into_iter().filter(|record| record.project == project && record.state != State::Closed) {
        pipe_link::close(factory, &record.id);
        close_with(factory, &record.id, "Its project was deleted.");
    }
}

/// SelectPipe: shown in every client's main pane.
/// With its bound agent's conversation, as the selected agent's.
pub(crate) async fn select(factory: &Arc<Factory>, id: &str) -> Result<(), RequestError> {
    let record = known(factory, id)?;
    let bound = factory.lock().config.agent(&record.bound).filter(|_| record.members.contains(&record.bound)).map(|agent| agent.id.clone());
    match bound {
        Some(bound) => factory.show_with_pipe(&bound, id).await?,
        None => factory.pipes.show(id),
    }
    change(factory, id, |record| record.unseen = false);
    send_transcript(factory, id);
    Ok(())
}

fn send_transcript(factory: &Factory, id: &str) {
    let (messages, has_earlier) = messages(factory, id, 0, 200);
    factory.send(Body::PipeTranscript(v1::PipeTranscript {
        pipe_id: id.to_owned(),
        messages: messages.iter().map(Message::to_proto).collect(),
        has_earlier,
    }));
}

/// LoadPipeMessages.
pub(crate) fn load_messages(factory: &Factory, request: &v1::LoadPipeMessages) -> Result<v1::PipeMessagePage, RequestError> {
    known(factory, &request.pipe_id)?;
    let (messages, has_earlier) = messages(factory, &request.pipe_id, request.before_seq, request.limit);
    Ok(v1::PipeMessagePage { pipe_id: request.pipe_id.clone(), messages: messages.iter().map(Message::to_proto).collect(), has_earlier })
}

/// SendPipeMessage: the person writes in the pipe; it goes out at once.
pub(crate) fn send_person(factory: &Arc<Factory>, request: &v1::SendPipeMessage) -> Result<(), RequestError> {
    let record = known(factory, &request.pipe_id)?;
    let text = request.text.trim().to_owned();
    if text.is_empty() {
        return Err(RequestError::Empty("text"));
    }
    if record.state != State::Open {
        return Err(RequestError::Pipe(not_open(&record)));
    }
    if text.len() > pipe_link::MAX_TEXT {
        return Err(RequestError::Pipe("That's more than a pipe message can hold (16 KB).".to_owned()));
    }
    let name = factory.lock().config.person_capitalized().to_owned();
    if let Some(secrets) = agents::secrets_in(factory, &text, &[]) {
        return Err(RequestError::Pipe(format!("It contains your secret {secrets}, so it wasn't sent.")));
    }
    let message = Message {
        msg_id: random_hex(16),
        author: Author::Me,
        name: name.clone(),
        text: text.clone(),
        state: MessageState::Queued,
        ..Message::default()
    };
    let Some(message) = add(factory, &record.id, message) else { return Err(RequestError::Pipe("Couldn't keep the message.".to_owned())) };
    count_for_loop(factory, &record.id, Author::Me, &text);
    transmit(factory, &record.id, &message, Via::Person { name });
    Ok(())
}

/// Why nothing can be written in the pipe now.
pub(crate) fn not_open(record: &Record) -> String {
    match (record.state, record.paused) {
        (State::Paused, Pause::Loop) => {
            "The pipe is paused: agents exchanged many messages on their own. Only the person can continue it.".to_owned()
        }
        (State::Paused, Pause::Peer) => format!("The pipe is paused by {}.", record.peer_who()),
        (State::Paused, _) => "The pipe is paused.".to_owned(),
        (State::Closed, _) => "The pipe is closed.".to_owned(),
        _ => "The pipe isn't open yet: both people must approve it first.".to_owned(),
    }
}

/// Hands a message that passed every check to the transport, once its
/// files are checked again: each must be what was scanned when it was sent
/// (a held one may have changed since the person saw it), and nothing in
/// it may be a secret now. Refused otherwise, saying why.
pub(crate) fn transmit(factory: &Arc<Factory>, pipe: &str, message: &Message, via: Via) {
    if let Err(why) = unchanged(factory, message) {
        set_state(factory, pipe, message.seq, MessageState::Refused, Some(&why));
        if let Some(record) = factory.pipes.get(pipe) {
            agents::tell(factory, &message.agent, &record, Some("Claktory"), &format!("didn't send your message: {why}"), &message.text);
        }
        return;
    }
    let files = message.files.iter().map(|file| pipe_link::OutFile { name: file.name.clone(), path: file.path.clone() }).collect();
    let outgoing = Outgoing { msg_id: message.msg_id.clone(), via, text: message.text.clone(), files };
    match pipe_link::send(factory, pipe, outgoing) {
        Ok(_) => {
            set_state(factory, pipe, message.seq, MessageState::Queued, None);
        }
        Err(error) => {
            set_state(factory, pipe, message.seq, MessageState::Refused, Some(&error.to_string()));
        }
    }
}

/// DecidePipeMessage: send or discard a held message; let in or discard a
/// held incoming one.
pub(crate) fn decide_message(factory: &Arc<Factory>, request: &v1::DecidePipeMessage) -> Result<(), RequestError> {
    let record = known(factory, &request.pipe_id)?;
    let Some(message) = message(factory, &record.id, request.seq) else {
        return Err(RequestError::Pipe("No such message in this pipe.".to_owned()));
    };
    match (message.state, request.allow) {
        (MessageState::Held, true) => {
            if record.state != State::Open {
                return Err(RequestError::Pipe(not_open(&record)));
            }
            count_for_loop(factory, &record.id, Author::MyAgent, &message.text);
            transmit(factory, &record.id, &message, Via::Agent { name: message.name.clone(), role: message.role.clone() });
        }
        (MessageState::Held, false) => {
            set_state(factory, &record.id, message.seq, MessageState::Discarded, None);
            agents::tell(factory, &message.agent, &record, None, "discarded your message; it wasn't sent", &message.text);
        }
        (MessageState::HeldIncoming, true) => {
            // Its files go to the inbox only now.
            if !message.files.is_empty()
                && let Some(inbox) = folder(factory, &record).map(|folder| folder.join("inbox"))
            {
                let _ = std::fs::create_dir_all(&inbox);
                let moved: Vec<FileRecord> = message
                    .files
                    .iter()
                    .filter_map(|file| into_inbox(&inbox, &pipe_link::InFile { name: file.name.clone(), path: file.path.clone() }))
                    .collect();
                set_files(factory, &record.id, message.seq, &moved);
                forget_held(&message);
            }
            set_state(factory, &record.id, message.seq, MessageState::Received, Some(""));
            // Let in after later ones the lead was told of: told at its next wake.
            change(factory, &record.id, |record| {
                if message.seq <= record.told && !record.untold.contains(&message.seq) {
                    record.untold.push(message.seq);
                }
            });
            agents::wake(factory, &record.id);
        }
        (MessageState::HeldIncoming, false) => {
            forget_held(&message);
            set_files(factory, &record.id, message.seq, &[]);
            set_state(factory, &record.id, message.seq, MessageState::Discarded, None);
        }
        _ => return Err(RequestError::Pipe("This message isn't waiting for you.".to_owned())),
    }
    Ok(())
}

/// The message's files are as they were scanned, and nothing it carries is
/// a secret; the reason it can't go otherwise.
fn unchanged(factory: &Factory, message: &Message) -> Result<(), String> {
    let mut contents = Vec::with_capacity(message.files.len());
    for file in &message.files {
        let bytes = std::fs::read(&file.path).map_err(|_| format!("{} is gone, so it wasn't sent.", file.name))?;
        if !file.sha256.is_empty() && agents::sha256(&bytes) != file.sha256 {
            return Err(format!(
                "{} changed after it was written in, so it wasn't sent: send it again to look at the new version.",
                file.name
            ));
        }
        contents.push(bytes);
    }
    if let Some(secrets) = agents::secrets_in(factory, &message.text, &contents) {
        return Err(format!("It contains your secret {secrets}, so it wasn't sent."));
    }
    Ok(())
}

/// Removes a held incoming message's own folder (where its files waited).
fn forget_held(message: &Message) {
    let folders: BTreeSet<PathBuf> = message.files.iter().filter_map(|file| file.path.parent().map(Path::to_path_buf)).collect();
    for folder in folders {
        // Only the message's own folder under pipes/held, never an inbox.
        if folder.components().rev().nth(2).is_some_and(|part| part.as_os_str() == "held") {
            let _ = std::fs::remove_dir_all(&folder);
        }
    }
}

// ---- From the transport ----

/// How many incoming messages may wait for the person at once; past that,
/// more are refused, not held.
const MAX_HELD_INCOMING: u32 = 20;

/// Where an incoming message's files wait while it's held for the person:
/// in Claktory's home, out of every agent's reach, until it's let in.
fn held_dir(factory: &Factory, pipe: &str) -> PathBuf {
    factory.home.root().join("pipes").join("held").join(pipe).join(random_hex(8))
}

/// The bytes of files that arrived on the pipe in the last day (let in or
/// waiting).
fn incoming_bytes(factory: &Factory, pipe: &str) -> u64 {
    let db = factory.store.db();
    let mut bytes = 0u64;
    if let Ok(mut statement) = db.prepare(
        "SELECT files FROM pipe_messages WHERE pipe = ?1 AND author IN ('their_agent', 'their_person') AND state IN ('received', 'held_incoming') AND created > ?2",
    ) && let Ok(rows) = statement.query_map(params![pipe, now_ms() - 86_400_000], |row| row.get::<_, String>(0))
    {
        for files in rows.flatten() {
            let files: Vec<FileRecord> = serde_json::from_str(&files).unwrap_or_default();
            bytes += files.iter().map(|file| file.size).sum::<u64>();
        }
    }
    bytes
}

/// Changes a message's files.
fn set_files(factory: &Factory, pipe: &str, seq: u64, files: &[FileRecord]) {
    let files = serde_json::to_string(files).unwrap_or_else(|_| "[]".to_owned());
    let seq = i64::try_from(seq).unwrap_or(i64::MAX);
    if let Err(error) =
        factory.store.db().execute("UPDATE pipe_messages SET files = ?3 WHERE pipe = ?1 AND seq = ?2", params![pipe, seq, files])
    {
        tracing::error!(%pipe, %error, "could not change a pipe message's files");
    }
}

/// Moves a received file into the pipe's inbox, under a safe name.
fn into_inbox(inbox: &Path, file: &pipe_link::InFile) -> Option<FileRecord> {
    let mut name: String = file
        .name
        .chars()
        .map(|character| if character.is_alphanumeric() || " .-_()".contains(character) { character } else { '_' })
        .collect();
    name = name.trim_matches(|character: char| character == '.' || character.is_whitespace()).chars().take(120).collect();
    if name.is_empty() {
        name = "file".to_owned();
    }
    let (stem, extension) = match name.rsplit_once('.') {
        Some((stem, extension)) if !stem.is_empty() => (stem.to_owned(), format!(".{extension}")),
        _ => (name.clone(), String::new()),
    };
    let mut target = inbox.join(&name);
    let mut number = 2;
    while target.exists() {
        target = inbox.join(format!("{stem} {number}{extension}"));
        number += 1;
    }
    if std::fs::rename(&file.path, &target).is_err() {
        std::fs::copy(&file.path, &target).ok()?;
        let _ = std::fs::remove_file(&file.path);
    }
    let size = std::fs::metadata(&target).map(|meta| meta.len()).unwrap_or_default();
    Some(FileRecord { name: target.file_name()?.to_string_lossy().into_owned(), size, path: target, sha256: String::new() })
}

impl pipe_link::Store for Link {
    fn knocked(&self, factory: &Arc<Factory>, pipe: &str, peer: Peer, digits: String) {
        let changed = change(factory, pipe, |record| {
            if record.state != State::Invited {
                return false;
            }
            record.peer = Some(PeerRecord::from_side(&peer.side, &peer.endpoint_id));
            record.digits = digits;
            record.state = State::Approval;
            true
        });
        if changed == Some(true) {
            let who = PeerRecord::from_side(&peer.side, "").who();
            line(
                factory,
                pipe,
                format!("{who} used the invite. Check that both screens show the same four digits, then approve or turn it down."),
            );
        }
    }

    fn peer_approved(&self, factory: &Arc<Factory>, pipe: &str) {
        let opened = change(factory, pipe, |record| {
            record.peer_approved = true;
            if record.state == State::WaitingPeer {
                record.state = State::Open;
                return true;
            }
            false
        });
        let who = factory.pipes.get(pipe).map(|record| record.peer_who()).unwrap_or_default();
        match opened {
            Some(true) => line(factory, pipe, format!("{who} approved it too: the pipe is open.")),
            Some(false) => line(factory, pipe, format!("{who} approved it.")),
            None => {}
        }
    }

    fn received(&self, factory: &Arc<Factory>, pipe: &str, message: Incoming) -> Result<(), String> {
        let Some(record) = factory.pipes.get(pipe) else { return Err("No such pipe.".to_owned()) };
        if record.state != State::Open {
            return Err(not_open(&record));
        }
        let arriving: u64 = message
            .files
            .iter()
            .take(pipe_link::MAX_FILES)
            .filter_map(|file| std::fs::metadata(&file.path).ok())
            .map(|meta| meta.len())
            .sum();
        if incoming_bytes(factory, pipe) + arriving > u64::from(record.limits.file_mb) * 1_048_576 {
            return Err(format!("Over this side's limit of {} MB of files a day.", record.limits.file_mb));
        }
        // Over the limits, or Review incoming: held whoever they say wrote it.
        let held = over_incoming(factory, &record)
            .or_else(|| record.review_incoming.then(|| "Waiting for you: let it in, or discard it.".to_owned()));
        if held.is_some() && waiting_incoming(factory, pipe) >= MAX_HELD_INCOMING {
            return Err("Too many messages already wait for this side's person.".to_owned());
        }
        // Held, its files wait out of every agent's reach until it's let in.
        let place =
            if held.is_some() { Some(held_dir(factory, pipe)) } else { folder(factory, &record).map(|folder| folder.join("inbox")) };
        let mut files = Vec::new();
        if let Some(place) = &place {
            let _ = std::fs::create_dir_all(place);
            for file in message.files.iter().take(pipe_link::MAX_FILES) {
                if let Some(kept) = into_inbox(place, file) {
                    files.push(kept);
                }
            }
        }
        let (author, name, role) = match &message.via {
            Via::Person { name } => (Author::TheirPerson, name.clone(), String::new()),
            Via::Agent { name, role } => (Author::TheirAgent, name.clone(), role.clone()),
        };
        let state = if held.is_some() { MessageState::HeldIncoming } else { MessageState::Received };
        let kept = Message {
            msg_id: message.msg_id.clone(),
            created: now_ms(),
            author,
            name,
            role,
            text: message.text.clone(),
            files,
            state,
            note: held.unwrap_or_default(),
            ..Message::default()
        };
        if add(factory, pipe, kept).is_none() {
            return Err("It couldn't be kept.".to_owned());
        }
        count_for_loop(factory, pipe, author, &message.text);
        if state == MessageState::Received {
            agents::wake(factory, pipe);
        } else if let Ok(runtime) = tokio::runtime::Handle::try_current() {
            // It waits for the person: Needs you shows it on every device
            // (Pipe.waiting_seqs), and it notifies. Later, as in
            // link_changed: the transport may hold its own locks.
            let factory = factory.clone();
            runtime.spawn(async move { factory.broadcast_workspace(&factory.lock()) });
        }
        Ok(())
    }

    fn delivered(&self, factory: &Arc<Factory>, pipe: &str, msg_id: &str) {
        if let Some(message) = by_msg_id(factory, pipe, msg_id) {
            set_state(factory, pipe, message.seq, MessageState::Delivered, None);
        }
    }

    fn refused(&self, factory: &Arc<Factory>, pipe: &str, msg_id: &str, reason: &str) {
        let Some(message) = by_msg_id(factory, pipe, msg_id) else { return };
        // Their words: one line, capped, and quoted as theirs.
        let reason = clean(reason, PURPOSE_CHARS);
        let note = format!("The other side refused it. They say: \"{reason}\"");
        set_state(factory, pipe, message.seq, MessageState::Refused, Some(&note));
        if let Some(record) = factory.pipes.get(pipe) {
            agents::tell_refused(factory, &message.agent, &record, &reason, &message.text);
        }
    }

    fn peer_paused(&self, factory: &Arc<Factory>, pipe: &str) {
        let paused = change(factory, pipe, |record| {
            if record.state != State::Open {
                return false;
            }
            record.state = State::Paused;
            record.paused = Pause::Peer;
            true
        });
        if paused == Some(true) {
            let who = factory.pipes.get(pipe).map(|record| record.peer_who()).unwrap_or_default();
            line(factory, pipe, format!("{who} paused it."));
        }
    }

    fn peer_resumed(&self, factory: &Arc<Factory>, pipe: &str) {
        let resumed = change(factory, pipe, |record| {
            if record.state != State::Paused || record.paused != Pause::Peer {
                return false;
            }
            record.state = State::Open;
            record.paused = Pause::None;
            true
        });
        if resumed == Some(true) {
            let who = factory.pipes.get(pipe).map(|record| record.peer_who()).unwrap_or_default();
            line(factory, pipe, format!("{who} resumed it."));
        }
    }

    fn peer_closed(&self, factory: &Arc<Factory>, pipe: &str) {
        let who = factory.pipes.get(pipe).map(|record| record.peer_who()).unwrap_or_default();
        close_with(factory, pipe, &format!("{who} closed it."));
    }

    fn link_changed(&self, factory: &Arc<Factory>, _pipe: &str, _status: LinkStatus) {
        // Later, not here: the transport may hold its own locks, which
        // `status` takes under Factory's.
        let factory = factory.clone();
        if let Ok(runtime) = tokio::runtime::Handle::try_current() {
            runtime.spawn(async move { factory.broadcast_workspace(&factory.lock()) });
        }
    }
}

/// Tests: a pipe that's open, as if both people approved it, with the
/// members and lead given.
#[doc(hidden)]
pub fn open_for_test(factory: &Arc<Factory>, project: &str, name: &str, members: &[&str], lead: &str, review: bool) -> String {
    let mut record = new_record(factory, project, name, "testing", State::Open).expect("a pipe");
    record.peer = Some(PeerRecord {
        claktory: "Ana's Claktory".to_owned(),
        owner_name: "Ana".to_owned(),
        project: "Marketing".to_owned(),
        ..PeerRecord::default()
    });
    record.members = members.iter().map(|id| (*id).to_owned()).collect();
    record.lead = lead.to_owned();
    record.approved = true;
    record.peer_approved = true;
    record.review = review;
    let id = record.id.clone();
    make_folders(factory, &record);
    insert(factory, record);
    id
}

/// Tests: what the transport hands the store, as the transport would.
#[doc(hidden)]
pub fn receive_for_test(factory: &Arc<Factory>, pipe: &str, message: Incoming) -> Result<(), String> {
    pipe_link::Store::received(&Link, factory, pipe, message)
}

/// Tests: the other side said this about itself, as an invite or an
/// accept would carry it.
#[doc(hidden)]
pub fn peer_says_for_test(factory: &Arc<Factory>, pipe: &str, side: &Side) {
    change(factory, pipe, |record| {
        record.peer = Some(PeerRecord::from_side(side, ""));
        true
    });
}

/// Tests: the other side refused a message, as the transport says.
#[doc(hidden)]
pub fn refused_for_test(factory: &Arc<Factory>, pipe: &str, seq: u64, reason: &str) {
    let msg_id = message(factory, pipe, seq).expect("the message").msg_id;
    pipe_link::Store::refused(&Link, factory, pipe, &msg_id, reason);
}

/// Tests: the pipes' section of an agent's system prompt.
#[doc(hidden)]
pub fn prompt_for_test(factory: &Factory, agent: &str) -> Option<String> {
    let config = factory.lock().config.clone();
    agents::prompt_section(factory, &config, agent)
}

/// Tests: the pipe as if kept from before quarantine came, and Claktory
/// starting again.
#[doc(hidden)]
pub fn upgrade_for_test(factory: &Arc<Factory>, pipe: &str) {
    change(factory, pipe, |record| record.quarantine_set = false);
    quarantine::upgrade(factory);
}

/// Tests: what Claktory does with the pipes when it starts again.
#[doc(hidden)]
pub fn start_for_test(factory: &Arc<Factory>) {
    quarantine::upgrade(factory);
    bound::upgrade(factory);
}

/// Tests: the pipe as if kept from before binding, and Claktory starting
/// again.
#[doc(hidden)]
pub fn unbound_for_test(factory: &Arc<Factory>, pipe: &str) {
    change(factory, pipe, |record| {
        record.bind_set = false;
        record.bound.clear();
    });
    bound::upgrade(factory);
}

/// Tests: agents' cards for starting pipes, as if `later_ms` had passed.
#[doc(hidden)]
pub fn expire_starts_for_test(factory: &Arc<Factory>, later_ms: i64) {
    start::expire_before(factory, crate::transcript::now_ms() + later_ms - start::WAITS_MS);
}

/// Tests: whether `agent` is quarantined for a pipe.
#[doc(hidden)]
pub fn quarantined_for_test(factory: &Factory, agent: &str) -> bool {
    factory.pipes.quarantined(agent)
}

/// Tests: `agent`'s taint as a monitor keeps it (JSON).
#[doc(hidden)]
pub fn taint_of_for_test(factory: &Factory, agent: &str) -> String {
    serde_json::to_string(&factory.pipes.taint_of(agent)).expect("JSON")
}

/// Tests: what a monitor `from` set for `to` does when it fires, with the
/// taint it kept (`taint_of_for_test`).
#[doc(hidden)]
pub fn taint_with_for_test(factory: &Arc<Factory>, from: &str, to: &str, taint: &str) {
    agents::taint_with(factory, from, to, &serde_json::from_str(taint).expect("a taint"));
}

/// Tests: the lead's wakes apart by `every` at least.
#[doc(hidden)]
pub fn set_wake_every(factory: &Factory, every: std::time::Duration) {
    factory.pipes.wake_every_ms.store(u64::try_from(every.as_millis()).unwrap_or(u64::MAX), Ordering::Relaxed);
}

impl Pipes {
    pub(crate) fn wake_every_ms(&self) -> u64 {
        self.wake_every_ms.load(Ordering::Relaxed)
    }
}

/// The pipe's messages as tests read them.
#[doc(hidden)]
pub fn messages_for_test(factory: &Factory, pipe: &str) -> Vec<v1::PipeMessage> {
    messages(factory, pipe, 0, 1000).0.iter().map(Message::to_proto).collect()
}
