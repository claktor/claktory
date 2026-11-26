//! Pipes (#196): narrow, deliberate chats between one of this Claktory's
//! projects and one of another Claktory's. Both people approve a pipe, pick
//! the agents granted it (its members) and the one woken by what arrives
//! (its lead), and write their own side's prompt for it. The person can write
//! in it too.
//!
//! This is the store: what a pipe is and who may use it, its messages, and
//! the limits code enforces whatever the model does (an outgoing secret scan,
//! files only from the outbox unless the person approves, Review outgoing,
//! rate limits and a loop breaker: `agents`; since #346 reading a pipe marks
//! no one, and only a web page read through browser control does). The transport (`crate::pipe_link`)
//! only carries what this hands it.
//!
//! Pipes live in factory.db, never in config.json, so no agent (Rivet
//! included) can make, approve or change one: only the person, from the app.

pub(crate) mod agents;
pub(crate) mod approval_card;
pub(crate) mod bound;
pub(crate) mod curator;
pub(crate) mod groups;
pub mod handoff;
pub mod identity;
pub(crate) mod lead;
pub(crate) mod quarantine;
pub(crate) mod sealed;
pub(crate) mod senders;
pub(crate) mod share_listener;
pub mod shares;
pub(crate) mod spend;
pub(crate) mod start;
pub(crate) mod trust_card;
pub(crate) mod waiting;

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
/// came through (JSON list). Since #346 a mark never spreads, so it's null
/// on new rows, kept for old ones; the same on the asks and proposals of an
/// agent that had read a web page (`agents::routes_json`); null for none.
pub(crate) const TAINT_VIA: &str = "
ALTER TABLE pipe_taint ADD COLUMN via TEXT;
ALTER TABLE asks ADD COLUMN taint TEXT;
ALTER TABLE proposals ADD COLUMN taint TEXT;
";

/// The pipe shown, if one, so a restart (an update) shows it again, as it
/// does the selected agent (`Pipes::saved`). At most one row.
pub(crate) const SELECTED: &str = "
CREATE TABLE pipe_selected (pipe TEXT NOT NULL);
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

/// The migration that added replies (#370 Submit): a held message the
/// person sent back to its writer with a comment (state 'discarded'), and a
/// use of a secret they answered with a comment (state 'denied'). The
/// comment itself is in decision_notes, never in a pipe table (Fugo).
pub(crate) const REPLIED: &str = "
ALTER TABLE pipe_messages ADD COLUMN replied INTEGER NOT NULL DEFAULT 0;
ALTER TABLE pipe_secret_approvals ADD COLUMN replied INTEGER NOT NULL DEFAULT 0;
";

/// The migration that tags a Claktory line announcing a website shared
/// with this side with that share's id (W2), so the app can open it there.
pub(crate) const SHARE_LINES: &str = "
ALTER TABLE pipe_messages ADD COLUMN share TEXT NOT NULL DEFAULT '';
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

/// When the loop breaker paused a pipe, and which rule did: a repeated
/// message, or `LOOP_AFTER` in a row.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct Looped {
    pub at: i64,
    pub repeat: bool,
}

impl Looped {
    /// Why it paused, for a card that asks to continue it: in Claktory's
    /// words only, never the other side's (an agent reads the card).
    pub(crate) fn why(looped: Option<Looped>) -> String {
        match looped {
            Some(Looped { repeat: true, .. }) => "the agents on both sides repeated the same message".to_owned(),
            Some(Looped { repeat: false, .. }) => format!("the agents on both sides exchanged {LOOP_AFTER} messages on their own"),
            None => "the agents on both sides talked on their own too long".to_owned(),
        }
    }
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
            project_name: self.project.clone(),
            purpose: self.purpose.clone(),
            // Who vouches for it: `identity::peer`.
            ..v1::PipePeer::default()
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
    /// The pipe said no agent answers it (`lead`), since it last had a lead.
    pub no_lead_said: bool,
    /// Agents' messages in a row with no person writing on either side.
    pub quiet: u32,
    /// Hashes of the latest agents' messages, to tell repeats.
    pub recent: Vec<u64>,
    /// The loop breaker's latest pause (`count_for_loop`), which a card to
    /// continue it is bound to (friend bug #32); None in pipes from before.
    pub looped: Option<Looped>,
    /// The other side says its loop breaker paused it (`Pause::Peer`).
    pub peer_looped: bool,
    pub next_seq: u64,
    /// Members quarantined for it (`quarantine`): the person's choice alone.
    pub quarantined: Vec<String>,
    /// Members that only send to it (#414): pipe_send and pipe_status, never
    /// pipe_read, never woken by what arrives nor told the other side's
    /// words, never its lead. Any agent of this Claktory, its project's or
    /// not (Rivet in the workspace), as it never reads the pipe. Never
    /// quarantined as well.
    pub send_only: Vec<String>,
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
    /// What this side last told the other of its liaison's wait, once
    /// heard (`waiting`); None before anything was.
    pub told_liaison: Option<waiting::Told>,
    /// The other side says its liaison waits, and until when (ms since
    /// 1970, checked; 0: no known end).
    pub peer_waiting: bool,
    pub peer_waiting_until: i64,
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

    /// Members that can run commands on this computer and read the pipe:
    /// every one neither quarantined nor only sending (a misleading message
    /// never reaches those).
    pub(crate) fn can_run_commands(&self) -> Vec<String> {
        self.members.iter().filter(|id| !self.quarantined.contains(id) && !self.send_only.contains(id)).cloned().collect()
    }

    /// Whether `agent` only sends to it (`send_only`).
    pub(crate) fn sends_only(&self, agent: &str) -> bool {
        self.send_only.iter().any(|id| id == agent)
    }

    /// "Ana's Claktory", else who the other side is: for what its
    /// Claktory did by itself.
    pub(crate) fn peer_claktory_owned(&self) -> String {
        match self.peer.as_ref().map(|peer| peer.owner_name.as_str()).filter(|owner| !owner.is_empty()) {
            Some(owner) => format!("{owner}'s Claktory"),
            None => self.peer_who(),
        }
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
    /// What factory.db says is shown (pipe_selected); `view` writes
    /// `selected` there when it differs, whatever changed it.
    saved: Option<String>,
    /// Agents that have read pipes in their current context: agent → pipe
    /// → the agents it came through, by id (empty when it read it itself).
    tainted: BTreeMap<String, Taint>,
    /// Pipes whose lead has a wake coming.
    waking: BTreeSet<String>,
    /// Curators (`curator`), by id.
    curators: BTreeSet<String>,
    /// Errand agents (`crate::errands`), by id: contained too.
    errand_agents: BTreeSet<String>,
    loaded: bool,
}

/// What the store keeps in memory: every pipe, which is shown, and who's tainted.
pub(crate) struct Pipes {
    data: Mutex<Data>,
    /// How long the lead's wakes are apart, at least (a minute); tests make it shorter.
    wake_every_ms: AtomicU64,
    /// How much later than now tests judge the age of a file the person
    /// attached (`agents::attached_by_person`); 0 otherwise.
    attached_later_ms: AtomicU64,
    /// How the other sides' statements are checked (`identity`).
    pub(crate) identity: identity::Identity,
    /// Report readers' wakes (`handoff::wake`).
    pub(crate) handoff: handoff::wake::Wakes,
}

impl Default for Pipes {
    fn default() -> Pipes {
        Pipes {
            data: Mutex::default(),
            wake_every_ms: AtomicU64::new(60_000),
            attached_later_ms: AtomicU64::new(0),
            identity: identity::Identity::default(),
            handoff: handoff::wake::Wakes::default(),
        }
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

    /// Whether `agent` is a curator (`curator`).
    pub(crate) fn curator(&self, agent: &str) -> bool {
        self.data().curators.contains(agent)
    }

    /// Whether `agent` is an errand agent (`crate::errands`).
    pub(crate) fn errand_agent(&self, agent: &str) -> bool {
        self.data().errand_agents.contains(agent)
    }

    /// Whether `agent` is contained: a pipe's liaison, a curator or an
    /// errand agent. Each has no commands, secrets, web, messaging or apps.
    pub(crate) fn contained(&self, agent: &str) -> bool {
        let data = self.data();
        data.curators.contains(agent)
            || data.errand_agents.contains(agent)
            || data.pipes.values().any(|record| record.quarantined.iter().any(|id| id == agent))
    }

    pub(crate) fn set_errand_agent(&self, agent: &str, errand: bool) {
        let mut data = self.data();
        if errand {
            data.errand_agents.insert(agent.to_owned());
        } else {
            data.errand_agents.remove(agent);
        }
    }

    pub(crate) fn set_curator(&self, agent: &str, curator: bool) {
        let mut data = self.data();
        if curator {
            data.curators.insert(agent.to_owned());
        } else {
            data.curators.remove(agent);
        }
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
    let saved = factory.store.db().query_row("SELECT pipe FROM pipe_selected", [], |row| row.get::<_, String>(0)).optional();
    let mut data = factory.pipes.data();
    let mut retired = Vec::new();
    match loaded {
        Ok((pipes, tainted, gone)) => {
            data.pipes = pipes;
            data.tainted = tainted;
            retired = gone;
        }
        Err(error) => tracing::error!(%error, "could not read the pipes"),
    }
    // The pipe shown before the restart, unless it's gone since.
    match saved {
        Ok(saved) => {
            data.selected = saved.clone().filter(|id| data.pipes.contains_key(id));
            data.saved = saved;
        }
        Err(error) => tracing::error!(%error, "could not read which pipe is shown"),
    }
    match curator::load(factory) {
        Ok(curators) => data.curators = curators,
        Err(error) => tracing::error!(%error, "could not read the curators"),
    }
    match crate::errands::load_agents(&factory.store) {
        Ok(agents) => data.errand_agents = agents,
        Err(error) => tracing::error!(%error, "could not read the errand agents"),
    }
    data.loaded = true;
    // Invites expired meanwhile close now; the others when they expire.
    let invites: Vec<String> =
        data.pipes.values().filter(|pipe| pipe.state == State::Invited && pipe.invite_expires > 0).map(|pipe| pipe.id.clone()).collect();
    drop(data);
    for id in invites {
        watch_invite(factory, &id);
    }
    quarantine::upgrade(factory);
    bound::upgrade(factory);
    sealed::upgrade(factory);
    agents::retired(factory, retired);
    // A pipe with agents and no lead (from before every pipe had one) gets one.
    lead::settle_all(factory);
    handoff::start(factory);
    shares::start(factory);
    if let Ok(runtime) = tokio::runtime::Handle::try_current() {
        runtime.spawn(start::expire(Arc::downgrade(factory)));
        runtime.spawn(waiting::watch(Arc::downgrade(factory)));
    }
}

/// The pipes, who's marked, and the secret uses retired at start (#346):
/// each asker and the secrets it asked for.
type Loaded = (BTreeMap<String, Record>, BTreeMap<String, Taint>, Vec<(String, Vec<String>)>);

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
    // #346: reading a pipe or a direct message marks no one any more, so
    // nobody stays marked by one read before, and a secret's use waiting
    // for the person only because of one goes (`expired`, out of Needs
    // you): run again, the command simply runs, or asks afresh if one of its
    // secrets asks each time. Only a web page read through browser control
    // still marks its reader. Every start; a no-op once done.
    db.execute("DELETE FROM pipe_taint WHERE pipe NOT LIKE 'browser:%'", [])?;
    let retiring = "state = 'pending' AND pipes NOT IN ('', '[]') AND agent NOT IN (SELECT agent FROM pipe_taint)";
    let mut retired: Vec<(String, Vec<String>)> = Vec::new();
    let mut statement = db.prepare(&format!("SELECT agent, secrets FROM pipe_secret_approvals WHERE {retiring} ORDER BY id"))?;
    for row in statement.query_map([], |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)))? {
        let (agent, secrets) = row?;
        retired.push((agent, serde_json::from_str(&secrets).unwrap_or_default()));
    }
    drop(statement);
    db.execute(
        &format!("UPDATE pipe_secret_approvals SET state = 'expired', decided = ?1 WHERE {retiring}"),
        [crate::transcript::now_ms()],
    )?;
    let mut tainted: BTreeMap<String, Taint> = BTreeMap::new();
    let mut statement = db.prepare("SELECT agent, pipe, via FROM pipe_taint")?;
    let rows = statement.query_map([], |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?, row.get::<_, Option<String>>(2)?)))?;
    for row in rows {
        let (agent, pipe, via) = row?;
        let via: Vec<String> = via.and_then(|via| serde_json::from_str(&via).ok()).unwrap_or_default();
        tainted.entry(agent).or_default().insert(pipe, via);
    }
    Ok((pipes, tainted, retired))
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
    let (result, record, before, was) = {
        let mut data = factory.pipes.data();
        let record = data.pipes.get_mut(id)?;
        // While it waits for approval, an agent's card to approve it may
        // turn void (`approval_card`).
        let before = (record.state == State::Approval).then(|| record.clone());
        let was = record.state;
        let result = edit(record);
        (result, record.clone(), before, was)
    };
    save(factory, &record);
    factory.broadcast_workspace(&factory.lock());
    if let Some(before) = before {
        approval_card::changed(factory, &before, Some(&record));
    }
    // A card asking to accept who's at the other end is void once it closes.
    if was != State::Closed && record.state == State::Closed {
        trust_card::settle(factory, id, "the pipe was closed");
    }
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
    /// Sent back to its writer by the person's reply (#370): never sent.
    /// The reply is in decision_notes, shown as its note (`for_person`).
    pub replied: bool,
    /// A Claktory line announcing a website shared with this side: its
    /// share's id (`line_for_share`); empty otherwise.
    pub share: String,
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
            replied: self.replied,
            share_id: self.share.clone(),
        }
    }
}

const COLUMNS: &str = "seq, msg_id, created, author, name, role, agent, text, files, state, note, replied, share";

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
        replied: row.get(11)?,
        share: row.get(12)?,
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
        &format!("INSERT INTO pipe_messages (pipe, {COLUMNS}) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)"),
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
            message.note,
            message.replied,
            message.share
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

/// What a message held by Review incoming says.
pub(crate) const REVIEW_NOTE: &str = "Waiting for you: let it in, or discard it.";

/// A line from Claktory in the pipe's chat.
pub(crate) fn line(factory: &Factory, pipe: &str, text: impl Into<String>) {
    add(factory, pipe, Message { text: text.into(), ..Message::default() });
}

/// The line announcing a website the other side shares with this one
/// ("Ana shares Dashboard."), tagged with its `share` so the app offers
/// Open and Copy address on it (W2).
pub(crate) fn line_for_share(factory: &Factory, pipe: &str, text: impl Into<String>, share: &str) {
    add(factory, pipe, Message { text: text.into(), share: share.to_owned(), ..Message::default() });
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

/// A held message goes back to its writer with the person's reply (#370):
/// discarded, never sent. The reply is in decision_notes.
fn sent_back(factory: &Factory, pipe: &str, seq: u64) {
    let changed = factory.store.db().execute(
        "UPDATE pipe_messages SET state = 'discarded', replied = 1 WHERE pipe = ?1 AND seq = ?2",
        params![pipe, i64::try_from(seq).unwrap_or(i64::MAX)],
    );
    if let Err(error) = changed {
        tracing::error!(%pipe, %error, "could not send a pipe's message back");
        return;
    }
    if let Some(message) = message(factory, pipe, seq) {
        shown(factory, pipe, &message);
    }
    factory.broadcast_workspace(&factory.lock());
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

/// Every message of the pipe from seq `from` up to `before` (0: to the
/// end), oldest first; and whether there are earlier ones.
fn messages_from(factory: &Factory, pipe: &str, from: u64, before: u64) -> (Vec<Message>, bool) {
    let from = i64::try_from(from).unwrap_or(i64::MAX);
    let before = if before == 0 { i64::MAX } else { i64::try_from(before).unwrap_or(i64::MAX) };
    let db = factory.store.db();
    let read = || -> rusqlite::Result<(Vec<Message>, bool)> {
        let mut statement =
            db.prepare(&format!("SELECT {COLUMNS} FROM pipe_messages WHERE pipe = ?1 AND seq >= ?2 AND seq < ?3 ORDER BY seq"))?;
        let found = statement.query_map(params![pipe, from, before], read_message)?.collect::<rusqlite::Result<Vec<_>>>()?;
        let earlier = db.query_row("SELECT EXISTS (SELECT 1 FROM pipe_messages WHERE pipe = ?1 AND seq < ?2)", params![pipe, from], |row| row.get(0))?;
        Ok((found, earlier))
    };
    read().unwrap_or_else(|error| {
        tracing::error!(%pipe, %error, "could not read a pipe's messages");
        (Vec::new(), false)
    })
}

/// How many of this side's agents' messages wait for the person to send them.
pub(crate) fn waiting_outgoing(factory: &Factory, pipe: &str) -> u32 {
    counted(factory, pipe, &[Author::MyAgent], &[MessageState::Held], i64::MAX / 2)
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
    factory.send(Body::PipeMessageUpdate(v1::PipeMessageUpdate { pipe_id: pipe.to_owned(), message: Some(for_person(factory, pipe, message)) }));
}

/// A message as the person's app shows it: one sent back with their reply
/// (#370) has the reply as its note, from decision_notes, which nothing
/// that reaches the other side reads.
fn for_person(factory: &Factory, pipe: &str, message: &Message) -> v1::PipeMessage {
    let mut shown = message.to_proto();
    if message.replied {
        shown.note = crate::needs_you::note_of(factory, "pipe_message", &format!("{pipe}:{}", message.seq));
    } else if message.state == MessageState::Discarded && message.author == Author::MyAgent {
        // Taken back by its writer (pipe_withdraw): why, in its words.
        let reason = crate::needs_you::note_of(factory, WITHDRAWN_NOTE, &format!("{pipe}:{}", message.seq));
        if !reason.is_empty() {
            shown.note = format!("{} took it back: {reason}", message.name);
        }
    }
    shown
}

// ---- The Workspace ----

/// The pipes, as every client shows them.
pub(crate) fn view(factory: &Factory, inner: &Inner) -> (Vec<v1::Pipe>, String) {
    let (records, selected, save) = {
        let mut data = factory.pipes.data();
        // Every change of what's shown is broadcast, so it's saved here.
        let save = (data.loaded && data.saved != data.selected).then(|| data.selected.clone());
        if let Some(shown) = &save {
            data.saved.clone_from(shown);
        }
        (data.pipes.values().cloned().collect::<Vec<_>>(), data.selected.clone().unwrap_or_default(), save)
    };
    if let Some(shown) = save {
        save_selected(factory, shown.as_deref());
    }
    let can_check = identity::checks(factory, &inner.config.account);
    let mut grouped = groups::members(factory);
    let mut pipes: Vec<v1::Pipe> = records
        .iter()
        .filter(|record| inner.config.project(&record.project).is_some())
        .map(|record| {
            let link = if record.state == State::Closed { LinkStatus::default() } else { pipe_link::status(factory, &record.id) };
            let (sent, mb) = used(factory, &record.id);
            let members: Vec<String> = record.members.iter().filter(|id| inner.config.agent(id).is_some()).cloned().collect();
            let lead = if members.contains(&record.lead) { record.lead.clone() } else { String::new() };
            let (forward, reports) = handoff::pipe_view(factory, record);
            let (peer_waiting, peer_waiting_until_ms) = waiting::shown(record);
            v1::Pipe {
                held_agents: sealed::view(&inner.config, &records, record),
                cant_limit: Vec::new(),
                peer_waiting,
                peer_waiting_until_ms,
                id: record.id.clone(),
                name: record.name.clone(),
                project_id: record.project.clone(),
                state: state_proto(record.state).into(),
                paused: pause_proto(record.paused).into(),
                created_ms: record.created,
                peer: record.peer.as_ref().map(|peer| identity::peer(factory, &record.id, can_check, peer.to_proto())),
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
                forward,
                reports,
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
                unanswered_seqs: lead::unanswered(factory, &inner.config, record),
                quarantined_ids: record.quarantined.iter().filter(|id| members.contains(id)).cloned().collect(),
                can_run_commands_ids: members
                    .iter()
                    .filter(|id| !record.quarantined.contains(id) && !record.send_only.contains(id))
                    .cloned()
                    .collect(),
                send_only_ids: record.send_only.iter().filter(|id| members.contains(id)).cloned().collect(),
                member_ids: members,
                group_id: grouped.remove(&record.id).unwrap_or_default(),
                reply_agent_id: if record.state == State::Approval {
                    replies_to(factory, &inner.config, &record.id).unwrap_or_default()
                } else {
                    String::new()
                },
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

/// How many agents' messages in a row pause the pipe (#345: 10 tripped on
/// ordinary conversations; the rate limits and the spend cap bound the
/// cost, not this).
const LOOP_AFTER: u32 = 50;
/// The same text this many times among the latest `REPEAT_WINDOW` agents'
/// messages, either side's, is a loop: a message bounced back and forth.
const REPEATS: usize = 3;
const REPEAT_WINDOW: usize = 10;
/// Only a text at least this long counts as repeated: "OK" or "Thanks" said
/// again is no loop (Fugo's #345 rule).
const REPEAT_MIN_CHARS: usize = 40;

/// Which rule paused a pipe.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Tripped {
    Count,
    Repeat,
}

/// A text as the repeat rule compares it: trimmed, lower case, each run of
/// whitespace one space; None when it's too short to count.
fn repeatable(text: &str) -> Option<String> {
    let normal = text.split_whitespace().collect::<Vec<_>>().join(" ").to_lowercase();
    (normal.chars().count() >= REPEAT_MIN_CHARS).then_some(normal)
}

/// Counts a message towards the loop breaker: this side's person's resets
/// it (the other side's can't: it could claim any message is its
/// person's); `LOOP_AFTER` others in a row, or a long text `REPEATS` times
/// among the latest `REPEAT_WINDOW`, pause the pipe. Only exchanges count:
/// a member that only sends never comes here (friend bug #33).
pub(crate) fn count_for_loop(factory: &Arc<Factory>, pipe: &str, author: Author, text: &str) {
    use std::hash::{Hash, Hasher};
    let tripped = {
        let mut data = factory.pipes.data();
        let Some(record) = data.pipes.get_mut(pipe) else { return };
        if author == Author::Me {
            record.quiet = 0;
            record.recent.clear();
            None
        } else {
            record.quiet += 1;
            // Every message takes a place in the window; a short one's hash
            // is never compared, so it can't make a repeat.
            let normal = repeatable(text);
            let mut hasher = std::collections::hash_map::DefaultHasher::new();
            normal.as_deref().unwrap_or(text).hash(&mut hasher);
            normal.is_some().hash(&mut hasher);
            let hash = hasher.finish();
            let repeats = if normal.is_some() { record.recent.iter().filter(|seen| **seen == hash).count() + 1 } else { 0 };
            record.recent.push(hash);
            while record.recent.len() > REPEAT_WINDOW - 1 {
                record.recent.remove(0);
            }
            let tripped = if record.quiet >= LOOP_AFTER {
                Some(Tripped::Count)
            } else if repeats >= REPEATS {
                Some(Tripped::Repeat)
            } else {
                None
            };
            tripped.filter(|_| record.state == State::Open)
        }
    };
    if let Some(tripped) = tripped {
        let who = factory.pipes.get(pipe).map(|record| record.peer_who()).unwrap_or_default();
        let paused = change(factory, pipe, |record| {
            record.state = State::Paused;
            record.paused = Pause::Loop;
            record.looped = Some(Looped { at: now_ms(), repeat: tripped == Tripped::Repeat });
        });
        if paused.is_some() {
            if let Err(error) = pipe_link::pause_looped(factory, pipe) {
                tracing::warn!(%pipe, %error, "could not tell the other side the pipe paused");
            }
            line(
                factory,
                pipe,
                match tripped {
                    Tripped::Count => format!(
                        "Your agents and {who}'s have exchanged {LOOP_AFTER} messages on their own, so the pipe is paused. Continue?"
                    ),
                    Tripped::Repeat => format!(
                        "Your agents and {who}'s have repeated the same message, so the pipe is paused. Continue?"
                    ),
                },
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
    handoff::adopt_default(factory, &record);
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
    invited(factory, record);
    if !request.start_id.is_empty() {
        start::done(factory, &request.start_id, &made, true);
    }
    Ok(v1::PipeInvite { pipe_id: id, code })
}

/// Keeps a new invite's pipe, saying so, and closes it if no one uses the
/// invite before it expires.
fn invited(factory: &Arc<Factory>, record: Record) {
    let id = record.id.clone();
    insert(factory, record);
    line(factory, &id, "Invite made. Send its code to the other person; it works once, for 10 minutes.");
    watch_invite(factory, &id);
}

/// Closes the pipe of an invite no one used once it expires: now if it
/// has, else when it does, while Claktory runs (a restart watches it again).
fn watch_invite(factory: &Arc<Factory>, id: &str) {
    let Some(expires) = factory.pipes.get(id).filter(|record| record.state == State::Invited).map(|record| record.invite_expires) else {
        return;
    };
    if expires <= now_ms() {
        close_expired(factory, id);
        return;
    }
    let Ok(runtime) = tokio::runtime::Handle::try_current() else { return };
    let (weak, id) = (Arc::downgrade(factory), id.to_owned());
    runtime.spawn(async move {
        let wait = u64::try_from(expires - now_ms()).unwrap_or(0);
        tokio::time::sleep(std::time::Duration::from_millis(wait)).await;
        if let Some(factory) = weak.upgrade() {
            close_expired(&factory, &id);
        }
    });
}

/// Closes the pipe if it's still waiting on an invite that has expired:
/// one used meanwhile, or cancelled, stays as it is.
fn close_expired(factory: &Factory, id: &str) {
    let now = now_ms();
    let closed = change(factory, id, |record| {
        let expired = record.state == State::Invited && record.invite_expires > 0 && record.invite_expires <= now;
        if expired {
            record.state = State::Closed;
            record.paused = Pause::None;
        }
        expired
    });
    if closed == Some(true) {
        unsent(factory, id);
        line(factory, id, "The invite expired before anyone used it.");
    }
}

/// ReadPipeInvite: what a code says, without using it.
pub(crate) fn read_invite(code: &str) -> Result<v1::PipeInvitePreview, RequestError> {
    let side = pipe_link::read_invite(code.trim()).map_err(pipe_error)?;
    let peer = PeerRecord::from_side(&side, "");
    let name = offered_name(&peer.pipe_name, now_ms());
    Ok(v1::PipeInvitePreview { code: code.trim().to_owned(), peer: Some(peer.to_proto()), name, expires_ms: 0 })
}

/// The name a pipe joined without naming it takes: the one the other side
/// chose, when Claktory can show it as its own (up to 40 characters: ASCII
/// letters, digits, punctuation and spaces, and ⇄ ↔ → – — ·; never saying
/// Claktory, even spaced out or in letters that read alike: "CIaktory",
/// "C1a-k.tory"), else "Pipe joined 22 Nov" in Claktory's words. Only ASCII
/// letters pass, so letters that merely look Latin (Cyrillic С, Greek Α,
/// fullwidth Ｃ) never do. Names the person types are theirs and stay free.
fn offered_name(theirs: &str, now: i64) -> String {
    let theirs = theirs.trim();
    let plain = !theirs.is_empty()
        && theirs.chars().count() <= 40
        && theirs.chars().all(|c| c.is_ascii_graphic() || c == ' ' || "⇄↔→–—·".contains(c));
    let folded: String = theirs
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .map(|c| match c.to_ascii_lowercase() {
            'i' | '1' => 'l',
            '0' => 'o',
            c => c,
        })
        .collect();
    if plain && !folded.contains("claktory") {
        return theirs.to_owned();
    }
    let day = crate::transcript::local_time(now, "%-d %b");
    format!("Pipe joined {day}").trim().to_owned()
}

/// How many times the other Claktory may turn a joining code away before
/// the pipe closes saying the code no longer works, and for how long after
/// the first time (the tries back off: 1, 2, 4 s…).
const TURNED_AWAY_TRIES: u32 = 4;
const TURNED_AWAY_FOR: std::time::Duration = std::time::Duration::from_secs(60);

/// AcceptPipeInvite: a pipe to `project`, JOINING and selected, then
/// APPROVAL once the other Claktory answers.
pub(crate) fn accept(factory: &Arc<Factory>, request: &v1::AcceptPipeInvite) -> Result<(), RequestError> {
    let code = request.code.trim().to_owned();
    if !request.start_id.is_empty() {
        start::check(factory, &request.start_id, v1::PipeStartKind::Accept)?;
    }
    let side = pipe_link::read_invite(&code).map_err(pipe_error)?;
    let peer = PeerRecord::from_side(&side, "");
    let name = if request.name.trim().is_empty() { offered_name(&peer.pipe_name, now_ms()) } else { request.name.clone() };
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
        // expiry ends it (join refuses an expired code), and at most as long
        // as a code lives (an expiry forged far ahead, or a clock behind,
        // can't keep it waiting). The other Claktory turning the code away
        // ends it after a few tries, or a minute after the first; anything
        // else is definitive and closes the pipe as before.
        let started = std::time::Instant::now();
        let mut backoff = std::time::Duration::from_secs(1);
        let mut said = String::new();
        let mut turned_away: Option<(u32, std::time::Instant)> = None;
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
                Err(error @ (PipeError::Unreachable(_) | PipeError::TurnedAway)) => {
                    if !factory.pipes.get(&id).is_some_and(|record| record.state == State::Joining) {
                        // Turned down or deleted meanwhile: nothing to reach.
                        return;
                    }
                    let reason = match error {
                        PipeError::Unreachable(reason) => reason,
                        _ => {
                            turned_away.get_or_insert((0, std::time::Instant::now())).0 += 1;
                            "it turned the code away".to_owned()
                        }
                    };
                    // Turned away a few times, or a minute after the first:
                    // the code doesn't work there.
                    if turned_away.is_some_and(|(tries, first)| tries >= TURNED_AWAY_TRIES || first.elapsed() >= TURNED_AWAY_FOR) {
                        close_with(&factory, &id, &format!("Couldn't connect: {}", PipeError::TurnedAway));
                        return;
                    }
                    if started.elapsed() >= pipe_link::INVITE_LIFETIME {
                        close_with(&factory, &id, &format!("Couldn't connect: {}", PipeError::Expired));
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
///
/// The person's comment goes to this side's agent only: the lead, or, turned
/// down, the agent whose card started the pipe (else the admin agent with
/// Pipes); never to the other side.
pub(crate) async fn approve(factory: &Arc<Factory>, request: &v1::ApprovePipe) -> Result<(), RequestError> {
    let comment = crate::decisions::comment(&request.comment)?;
    let record = known(factory, &request.pipe_id)?;
    if record.state != State::Approval {
        return Err(RequestError::Pipe("This pipe isn't waiting for your approval.".to_owned()));
    }
    let starter = || replies_to(factory, &factory.lock().config, &record.id);
    // A reply (#370 Submit): the pipe still waits, unchanged; the agent
    // that set it up gets the person's words.
    if request.reply {
        if comment.is_empty() {
            return Err(RequestError::Pipe("Write a reply, or approve or turn it down.".to_owned()));
        }
        let Some(agent) = starter() else {
            return Err(RequestError::Pipe("No agent of yours set up this pipe, so there's no one to reply to.".to_owned()));
        };
        let person = factory.person_capitalized();
        let summary = format!("{person} replied about {}", called(&record));
        let text = format!("{summary}; it still waits for their approval. They say:\n\n{comment}");
        factory.deliver_event(&agent, text, crate::factory::host_event(summary));
        return Ok(());
    }
    // From an agent's card (#471 G1): only approving, with the person's
    // own choices; dismissing or replying is on the card itself.
    if !request.card_id.trim().is_empty() && (!request.approve || request.reply) {
        return Err(RequestError::Pipe("Dismiss the card, or reply on it, on the card itself.".to_owned()));
    }
    if !request.approve {
        pipe_link::close(factory, &record.id);
        close_with(factory, &record.id, "You turned it down.");
        if !comment.is_empty()
            && let Some(agent) = starter()
        {
            noted(factory, &agent, &format!("turned down {}", called(&record)), comment);
        }
        return Ok(());
    }
    choices_checked(factory, &record, request)?;
    let card = approval_card::claim(factory, &record, request)?;
    let from = card.as_ref().map(|(_, agent)| factory.agent_name(agent));
    let approved = approve_as_chosen(factory, &record, request, comment, from.as_deref()).await;
    if let Some((id, _)) = card {
        approval_card::settled(factory, id, &record, approved.is_ok());
    }
    approved
}

/// The person's choices on approving `record` (ApprovePipe), checked
/// before anything is made; an agent's card to approve it is checked the
/// same (`approval_card`).
pub(crate) fn choices_checked(factory: &Factory, record: &Record, request: &v1::ApprovePipe) -> Result<(), RequestError> {
    let send_only = request.send_only.as_ref().map(|chosen| chosen.agent_ids.clone()).unwrap_or_default();
    let members = checked_members_sending(factory, record, &request.member_ids, &send_only)?;
    let quarantined = match &request.quarantined {
        Some(chosen) => checked_members(factory, record, &chosen.agent_ids)?,
        None => Vec::new(),
    };
    if let Some(stray) = quarantined.iter().find(|id| !members.contains(id)) {
        return Err(RequestError::Pipe(format!("{} isn't one of this pipe's agents.", factory.agent_name(stray))));
    }
    let lead = request.lead_id.trim();
    sealed::check(factory, &record.id, &sealed::Roles { members: &members, lead, bound: "" })?;
    // A new liaison answers it; else the lead, one of its agents.
    let liaison = !request.liaison_name.trim().is_empty();
    if !liaison && lead.is_empty() {
        return Err(RequestError::Pipe("Choose the agent that answers the pipe, or add a liaison agent.".to_owned()));
    }
    if !liaison && !members.iter().any(|id| id == lead) {
        return Err(RequestError::Pipe("The lead must be one of the pipe's agents.".to_owned()));
    }
    checked_send_only(factory, &send_only, &members, &quarantined, if liaison { "" } else { lead }, "")?;
    Ok(())
}

/// Approves `record` as the person chose (`approve`), from an agent's card
/// when `from` names it.
async fn approve_as_chosen(
    factory: &Arc<Factory>,
    record: &Record,
    request: &v1::ApprovePipe,
    comment: &str,
    from: Option<&str>,
) -> Result<(), RequestError> {
    let send_only_wanted = request.send_only.as_ref().map(|chosen| chosen.agent_ids.clone()).unwrap_or_default();
    let mut members = checked_members_sending(factory, record, &request.member_ids, &send_only_wanted)?;
    let mut quarantined = match &request.quarantined {
        Some(chosen) => checked_members(factory, record, &chosen.agent_ids)?,
        None => Vec::new(),
    };
    if let Some(stray) = quarantined.iter().find(|id| !members.contains(id)) {
        return Err(RequestError::Pipe(format!("{stray} isn't one of this pipe's agents.")));
    }
    // Before a liaison is made for it: a new one is on no other pipe.
    let roles = sealed::Roles { members: &members, lead: request.lead_id.trim(), bound: "" };
    sealed::check(factory, &record.id, &roles)?;
    let mut lead = request.lead_id.trim().to_owned();
    if !request.liaison_name.trim().is_empty() {
        let liaison = make_liaison(factory, record, request.liaison_name.trim()).await?;
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
    let bound = factory.pipes.get(&record.id).map(|record| record.bound).unwrap_or_default();
    let send_only = checked_send_only(factory, &send_only_wanted, &members, &quarantined, &lead, &bound)?;
    pipe_link::approve(factory, &record.id).map_err(pipe_error)?;
    let opened = change(factory, &record.id, |record| {
        record.members = members;
        record.lead = lead;
        record.send_only = send_only;
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
    let you = from.map_or_else(|| "You approved it".to_owned(), |agent| format!("You approved it, from {agent}'s card"));
    if opened {
        line(factory, &record.id, format!("{you}, and so did the other side: the pipe is open."));
    } else {
        line(factory, &record.id, format!("{you}. Waiting for {} to approve it too.", record.peer_who()));
    }
    if !comment.is_empty()
        && let Some(record) = factory.pipes.get(&record.id)
    {
        noted(factory, &record.lead, &format!("approved {}, with you answering it", called(&record)), comment);
    }
    Ok(())
}

/// The agent the person's comment on approving `pipe` goes to: the one whose
/// card started it, else the admin agent with Pipes.
fn replies_to(factory: &Factory, config: &crate::config::Config, pipe: &str) -> Option<String> {
    start::agent_of(&factory.store, pipe).filter(|agent| config.agent(agent).is_some()).or_else(|| {
        config
            .agents
            .iter()
            .find(|agent| crate::proposals::is_admin(agent) && crate::extensions::enabled(agent, crate::extensions::pipes::ID))
            .map(|agent| agent.id.clone())
    })
}

/// The pipe as this side's agents may read it: by its name only when this
/// side chose it, never a name the other side gave it.
fn called(record: &Record) -> String {
    if record.named_here.unwrap_or(record.peer.is_none()) {
        format!("the pipe \"{}\"", record.name)
    } else {
        format!("the pipe {}", record.id)
    }
}

/// Tells `agent` what the person decided, with their comment as theirs.
fn noted(factory: &Arc<Factory>, agent: &str, what: &str, comment: &str) {
    let person = factory.person_capitalized();
    let summary = format!("{person} {what}");
    let text = format!("{summary}.{}", crate::decisions::note(&person, comment));
    factory.deliver_event(agent, text, crate::factory::host_event(summary));
}

/// The agents of the pipe's project among `ids`.
fn checked_members(factory: &Factory, record: &Record, ids: &[String]) -> Result<Vec<String>, RequestError> {
    checked_members_sending(factory, record, ids, &[])
}

/// The same, where those in `send_only` may be agents outside the pipe's
/// project (#414): they never read it.
fn checked_members_sending(factory: &Factory, record: &Record, ids: &[String], send_only: &[String]) -> Result<Vec<String>, RequestError> {
    let inner = factory.lock();
    let mut members = Vec::new();
    for id in ids {
        let Some(agent) = inner.config.agent(id) else { return Err(RequestError::UnknownAgent(id.clone())) };
        if agent.project.as_deref() != Some(record.project.as_str()) && !send_only.contains(id) {
            return Err(RequestError::Pipe(format!("{} isn't in this pipe's project.", agent.name)));
        }
        // A curator never reaches a pipe (pipes-automation.md §3).
        if factory.pipes.curator(id) {
            return Err(RequestError::Pipe(format!("{} reads reports, so it can't be in a pipe.", agent.name)));
        }
        if !members.contains(id) {
            members.push(id.clone());
        }
    }
    Ok(members)
}

/// The members to only send (#414), checked against the members after the
/// change (`members`), who's quarantined, the lead and the liaison
/// (`bound`): refused, saying why, when one isn't a member, is quarantined,
/// leads or is the liaison.
fn checked_send_only(
    factory: &Factory,
    ids: &[String],
    members: &[String],
    quarantined: &[String],
    lead: &str,
    bound: &str,
) -> Result<Vec<String>, RequestError> {
    let name = |id: &str| factory.agent_name(id);
    let mut chosen = Vec::new();
    for id in ids {
        if !members.contains(id) {
            return Err(RequestError::Pipe(format!("{} isn't one of this pipe's agents.", name(id))));
        }
        if quarantined.contains(id) {
            return Err(RequestError::Pipe(format!("{} is limited to the pipe, so it reads it: it can't only send as well.", name(id))));
        }
        if id == lead {
            return Err(RequestError::Pipe(format!("{} leads the pipe, so it reads what arrives: choose another lead first.", name(id))));
        }
        if id == bound {
            return Err(RequestError::Pipe(format!("{} is the pipe's agent, so it reads what arrives.", name(id))));
        }
        if !chosen.contains(id) {
            chosen.push(id.clone());
        }
    }
    Ok(chosen)
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
    let lead_before = lead::effective(&factory.lock().config, &record).unwrap_or_default();
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
    lead::settle(factory, &record.id, &lead_before);
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
    // Who only sends, as chosen, else as it was: agents outside the
    // project may be members only so.
    let send_only_wanted = request.send_only.as_ref().map(|chosen| chosen.agent_ids.clone());
    let sending = send_only_wanted.clone().unwrap_or_else(|| record.send_only.clone());
    let members = match &request.members {
        Some(members) => Some(checked_members_sending(factory, &record, &members.agent_ids, &sending)?),
        None => None,
    };
    let lead = request.lead_id.as_ref().map(|lead| lead.trim().to_owned());
    let after_members = members.clone().unwrap_or_else(|| record.members.clone());
    let quarantined = match &request.quarantined {
        Some(chosen) => Some(checked_members(factory, &record, &chosen.agent_ids)?),
        None => None,
    };
    let after_quarantined = quarantined.clone().unwrap_or_else(|| record.quarantined.clone());
    let after_lead = match &lead {
        Some(lead) => lead.clone(),
        None if after_members.contains(&record.lead) => record.lead.clone(),
        None => String::new(),
    };
    let send_only = checked_send_only(
        factory,
        &sending.into_iter().filter(|id| after_members.contains(id) || send_only_wanted.is_some()).collect::<Vec<_>>(),
        &after_members,
        &after_quarantined,
        &after_lead,
        &record.bound,
    )?;
    // A member from outside the project that would no longer only send.
    if let Some(stray) = after_members.iter().find(|id| record.send_only.contains(id) && !send_only.contains(id)) {
        let inner = factory.lock();
        if let Some(agent) = inner.config.agent(stray)
            && agent.project.as_deref() != Some(record.project.as_str())
        {
            return Err(RequestError::Pipe(format!("{} isn't in this pipe's project, so it can only send to it.", agent.name)));
        }
    }
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
    let bound = if after_members.contains(&record.bound) { record.bound.as_str() } else { "" };
    let roles = sealed::Roles { members: &after_members, lead: &after_lead, bound };
    sealed::check(factory, &record.id, &roles)?;
    let review_off = request.review_outgoing == Some(false) && record.review;
    let incoming = request.review_incoming.filter(|review| *review != record.review_incoming);
    let lead_before = lead::effective(&factory.lock().config, &record).unwrap_or_default();
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
        record.send_only = send_only;
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
    // A pipe with agents always has a lead: "No lead", or a lead unticked,
    // gives it its liaison, or another of its agents (`lead`).
    lead::settle(factory, &record.id, &lead_before);
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
    set_pipe_state_by(factory, request, None)
}

/// SetPipeState by the person (`by` None), or by an admin agent, by its
/// name (#423's pipes_settings): the pipe's line says who.
pub(crate) fn set_pipe_state_by(factory: &Arc<Factory>, request: &v1::SetPipeState, by: Option<&str>) -> Result<(), RequestError> {
    let who = by.unwrap_or("You");
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
            line(factory, &id, format!("{who} paused it: nothing goes either way until you resume it."));
        }
        v1::PipeAction::Resume => {
            if record.state != State::Paused || record.paused == Pause::Peer {
                let why = if record.paused == Pause::Peer && record.peer_looped {
                    format!(
                        "{} paused it, because the agents were going back and forth: only they can continue it.",
                        record.peer_claktory_owned()
                    )
                } else if record.paused == Pause::Peer {
                    format!("{} paused it: only they can resume it.", record.peer_who())
                } else {
                    "This pipe isn't paused.".to_owned()
                };
                return Err(RequestError::Pipe(why));
            }
            if record.paused == Pause::Loop {
                pipe_link::resume_looped(factory, &id).map_err(pipe_error)?;
            } else {
                pipe_link::resume(factory, &id).map_err(pipe_error)?;
            }
            if record.paused == Pause::Loop {
                let now = crate::transcript::now_ms();
                crate::needs_you::pipe_decided(factory, &id, 0, crate::needs_you::PipeDecision::Loop, "", true, "", now);
            }
            change(factory, &id, |record| {
                record.state = State::Open;
                record.paused = Pause::None;
                record.quiet = 0;
                record.recent.clear();
            });
            line(factory, &id, format!("{who} resumed it."));
            // What its lead wasn't told of meanwhile (a new lead's catch-up).
            agents::wake(factory, &id);
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
            close_with(factory, &id, &format!("{who} closed it."));
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
        unsent(factory, id);
        line(factory, id, why);
    }
}

/// A closed pipe's queue is dropped, on either side: what it still held
/// never goes, so it's shown as not sent rather than waiting for ever.
fn unsent(factory: &Factory, pipe: &str) {
    let note = "The pipe closed before it went.";
    let marked = {
        let db = factory.store.db();
        let mark = || -> rusqlite::Result<Vec<i64>> {
            let mut statement =
                db.prepare("UPDATE pipe_messages SET state = ?2, note = ?3 WHERE pipe = ?1 AND state = ?4 RETURNING seq")?;
            let rows = statement
                .query_map(params![pipe, MessageState::Refused.as_str(), note, MessageState::Queued.as_str()], |row| row.get(0))?;
            rows.collect()
        };
        mark().unwrap_or_else(|error| {
            tracing::error!(%pipe, %error, "could not mark a closed pipe's queue as not sent");
            Vec::new()
        })
    };
    for seq in marked {
        if let Some(message) = message(factory, pipe, u64::try_from(seq).unwrap_or_default()) {
            shown(factory, pipe, &message);
        }
    }
}

/// Records which pipe is shown (none: an agent or a chat is).
fn save_selected(factory: &Factory, shown: Option<&str>) {
    let saved = (|| -> rusqlite::Result<()> {
        let db = factory.store.db();
        db.execute("DELETE FROM pipe_selected", [])?;
        if let Some(pipe) = shown {
            db.execute("INSERT INTO pipe_selected (pipe) VALUES (?1)", [pipe])?;
        }
        Ok(())
    })();
    if let Err(error) = saved {
        tracing::error!(%error, "could not record which pipe is shown");
    }
}

fn delete(factory: &Factory, id: &str) {
    let removed = {
        let mut data = factory.pipes.data();
        let removed = data.pipes.remove(id);
        if data.selected.as_deref() == Some(id) {
            data.selected = None;
        }
        removed
    };
    if let Some(removed) = removed {
        approval_card::changed(factory, &removed, None);
        trust_card::settle(factory, id, "the pipe was deleted");
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
    factory.send(Body::PipeTranscript(transcript(factory, id)));
}

/// The pipe's latest messages, as a client shows them when it's selected:
/// on SelectPipe, and to a client connecting while it's shown.
pub(crate) fn transcript(factory: &Factory, id: &str) -> v1::PipeTranscript {
    let (messages, has_earlier) = messages(factory, id, 0, 200);
    v1::PipeTranscript {
        pipe_id: id.to_owned(),
        messages: messages.iter().map(|message| for_person(factory, id, message)).collect(),
        has_earlier,
    }
}

/// LoadPipeMessages.
pub(crate) fn load_messages(factory: &Factory, request: &v1::LoadPipeMessages) -> Result<v1::PipeMessagePage, RequestError> {
    known(factory, &request.pipe_id)?;
    let (messages, has_earlier) = match request.from_seq {
        Some(from) => messages_from(factory, &request.pipe_id, from, request.before_seq),
        None => messages(factory, &request.pipe_id, request.before_seq, request.limit),
    };
    let messages = messages.iter().map(|message| for_person(factory, &request.pipe_id, message)).collect();
    Ok(v1::PipeMessagePage { pipe_id: request.pipe_id.clone(), messages, has_earlier })
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
    let name = factory.person_capitalized();
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

/// How a message's note starts when the other side refused it; their
/// words follow, so a member that only sends (#414) is never shown the rest.
pub(crate) const PEER_REFUSED: &str = "The other side refused it.";

/// Hands a message that passed every check to the transport, once its
/// files are checked again: each must be what was scanned when it was sent
/// (a held one may have changed since the person saw it), and nothing in
/// it may be a secret now. Refused otherwise, saying why.
pub(crate) fn transmit(factory: &Arc<Factory>, pipe: &str, message: &Message, via: Via) {
    if let Err((why, advice)) = unchanged(factory, message) {
        set_state(factory, pipe, message.seq, MessageState::Refused, Some(&why));
        if let Some(record) = factory.pipes.get(pipe) {
            let note = format!(" {why}{advice}");
            agents::tell_noted(factory, &message.agent, &record, Some("Claktory"), "didn't send your message", &message.text, &note);
        }
        return;
    }
    let files = message.files.iter().map(|file| pipe_link::OutFile { name: file.name.clone(), path: file.path.clone() }).collect();
    // Its writer only sends, as it stands now: the other side's loop
    // breaker leaves it out too (friend bug #33).
    let sends_only = matches!(via, Via::Agent { .. }) && factory.pipes.get(pipe).is_some_and(|record| record.sends_only(&message.agent));
    let outgoing = Outgoing { msg_id: message.msg_id.clone(), via, text: message.text.clone(), files, sends_only };
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
///
/// The person's comment goes only to this side's agent (the message's
/// writer, or the lead for an incoming one), never into the message or
/// anything the other side gets.
pub(crate) fn decide_message(factory: &Arc<Factory>, request: &v1::DecidePipeMessage) -> Result<(), RequestError> {
    let comment = crate::decisions::comment(&request.comment)?;
    let record = known(factory, &request.pipe_id)?;
    let Some(message) = message(factory, &record.id, request.seq) else {
        return Err(RequestError::Pipe("No such message in this pipe.".to_owned()));
    };
    let person = factory.person_capitalized();
    // A reply (#370 Submit), on a message this side's agent wrote: it isn't
    // sent, and goes back to its writer with the person's words.
    if request.reply {
        if comment.is_empty() {
            return Err(RequestError::Pipe("Write a reply, or send or discard it.".to_owned()));
        }
        if message.state != MessageState::Held {
            return Err(RequestError::Pipe("Only a message your agent wrote can go back to it with a reply.".to_owned()));
        }
        // The reply is kept for the history first: the card shows it from there.
        let kind = crate::needs_you::PipeDecision::Held;
        crate::needs_you::pipe_decided(factory, &record.id, message.seq, kind, &message.agent, false, comment, message.created);
        sent_back(factory, &record.id, message.seq);
        let again = format!(" It wasn't sent. They say:\n\n{comment}\n\nWrite it again with that in mind, and send it again.");
        agents::tell_noted(factory, &message.agent, &record, None, "sent your message back", &message.text, &again);
        return Ok(());
    }
    let note = crate::decisions::note(&person, comment);
    // Only with a comment: the lead hears nothing else of it.
    let tell_lead = |what: &str| {
        if !note.is_empty() && !record.lead.is_empty() {
            let summary = format!("{person} {what} on {}", called(&record));
            factory.deliver_event(&record.lead, format!("{summary}.{note}"), crate::factory::host_event(summary));
        }
    };
    match (message.state, request.allow) {
        (MessageState::Held, true) => {
            if record.state != State::Open {
                return Err(RequestError::Pipe(not_open(&record)));
            }
            if !record.sends_only(&message.agent) {
                count_for_loop(factory, &record.id, Author::MyAgent, &message.text);
            }
            transmit(factory, &record.id, &message, Via::Agent { name: message.name.clone(), role: message.role.clone() });
            // The writer hears it went only with a comment.
            if !note.is_empty() {
                agents::tell_noted(factory, &message.agent, &record, None, "approved your held message", &message.text, &note);
            }
        }
        (MessageState::Held, false) => {
            set_state(factory, &record.id, message.seq, MessageState::Discarded, None);
            agents::tell_discarded(factory, &message.agent, &record, &message.text, &note);
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
            tell_lead("let in a message from the other side");
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
            tell_lead("discarded a message from the other side before anyone read it");
        }
        _ => return Err(RequestError::Pipe("This message isn't waiting for you.".to_owned())),
    }
    let (kind, agent) = match message.state {
        MessageState::Held => (crate::needs_you::PipeDecision::Held, message.agent.as_str()),
        _ => (crate::needs_you::PipeDecision::HeldIncoming, ""),
    };
    crate::needs_you::pipe_decided(factory, &record.id, message.seq, kind, agent, request.allow, comment, message.created);
    Ok(())
}

/// pipe_withdraw (friend bug #19): `agent` takes back its own message held
/// for the person, once it no longer applies (they settled it in its chat,
/// say). It's discarded, never sent, and leaves their Needs you at once;
/// `reason` is kept apart from the pipe tables (decision_notes, kind
/// 'pipe_withdrawn'), as the person's words are, and shows as its note.
/// Err: why not, for the agent.
pub(crate) fn withdraw_held(factory: &Factory, pipe: &str, seq: u64, agent: &str, reason: &str) -> Result<(), String> {
    let number = i64::try_from(seq).unwrap_or(i64::MAX);
    let claimed = factory
        .store
        .db()
        .execute(
            "UPDATE pipe_messages SET state = ?5 WHERE pipe = ?1 AND seq = ?2 AND agent = ?3 AND author = ?4 AND state = ?6",
            params![pipe, number, agent, Author::MyAgent.as_str(), MessageState::Discarded.as_str(), MessageState::Held.as_str()],
        )
        .map_err(|error| format!("Couldn't take it back: {error}"))?;
    if claimed == 0 {
        return Err(match message(factory, pipe, seq) {
            Some(found) if found.agent == agent && found.author == Author::MyAgent => match found.state {
                MessageState::Discarded => "It isn't waiting any more: it was discarded or sent back to you.".to_owned(),
                _ => "It isn't waiting any more: it was sent.".to_owned(),
            },
            _ => format!("You have no message {seq} in this pipe. Nothing was taken back."),
        });
    }
    crate::needs_you::keep_note(factory, WITHDRAWN_NOTE, &format!("{pipe}:{seq}"), reason);
    // In the person's History, withdrawn by its writer, with the reason.
    if let Some(message) = set_state(factory, pipe, seq, MessageState::Discarded, None) {
        let kind = crate::needs_you::PipeDecision::Held;
        crate::needs_you::pipe_decided(factory, pipe, seq, kind, agent, false, "", message.created);
    }
    Ok(())
}

/// decision_notes' kind for a held message its writer took back (needs_you's
/// History reads it by name).
const WITHDRAWN_NOTE: &str = "pipe_withdrawn";

/// The message's files are as they were scanned, and nothing it carries is
/// a secret. Otherwise why it can't go, as the app shows it after "Not
/// sent:", and what its writer could do, for the event it gets.
fn unchanged(factory: &Factory, message: &Message) -> Result<(), (String, &'static str)> {
    let mut contents = Vec::with_capacity(message.files.len());
    for file in &message.files {
        let bytes = std::fs::read(&file.path).map_err(|_| (format!("{} is gone.", file.name), ""))?;
        if !file.sha256.is_empty() && agents::sha256(&bytes) != file.sha256 {
            // A file the person attached is sent from their upload (friend bug #15).
            let uploads = factory.home.agent_dir(&message.agent).join("uploads");
            let attached = file.path.starts_with(&uploads) || uploads.canonicalize().is_ok_and(|real| file.path.starts_with(real));
            let when = if attached { "it was attached" } else { "the message was written" };
            return Err((format!("{} changed after {when}.", file.name), " Send it again to send the new version."));
        }
        contents.push(bytes);
    }
    if let Some(secrets) = agents::secrets_in(factory, &message.text, &contents) {
        return Err((format!("It contains your secret {secrets}."), ""));
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
        // Over the limits, another account than the person accepted, or
        // Review incoming: held whoever they say wrote it.
        let held = over_incoming(factory, &record)
            .or_else(|| identity::held_note(factory, &record))
            .or_else(|| record.review_incoming.then(|| REVIEW_NOTE.to_owned()));
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
        // What the other side says an agent that only sends wrote takes no
        // part in the loop breaker, not even a place among the latest
        // (friend bug #33; Fugo 33515); a person's counts whatever it says.
        if !(message.sends_only && author == Author::TheirAgent) {
            count_for_loop(factory, pipe, author, &message.text);
        }
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
        let note = format!("{PEER_REFUSED} They say: \"{reason}\"");
        set_state(factory, pipe, message.seq, MessageState::Refused, Some(&note));
        if let Some(record) = factory.pipes.get(pipe) {
            agents::tell_refused(factory, &message.agent, &record, &reason, &message.text);
        }
    }

    fn peer_paused(&self, factory: &Arc<Factory>, pipe: &str, looped: bool) {
        let paused = change(factory, pipe, |record| {
            if record.state != State::Open {
                return false;
            }
            record.state = State::Paused;
            record.paused = Pause::Peer;
            record.peer_looped = looped;
            true
        });
        if paused == Some(true) {
            let record = factory.pipes.get(pipe);
            let who = record.as_ref().map(|record| record.peer_who()).unwrap_or_default();
            // Their loop breaker, not their person (Abbacchio row 1177):
            // "Ana's Claktory paused it".
            let said = if looped {
                let whose = record.as_ref().map_or(who, Record::peer_claktory_owned);
                format!("{whose} paused it: the agents were going back and forth on their own.")
            } else {
                format!("{who} paused it.")
            };
            line(factory, pipe, said);
        }
    }

    fn peer_resumed(&self, factory: &Arc<Factory>, pipe: &str, looped: bool) {
        let resumed = change(factory, pipe, |record| {
            if record.state != State::Paused || record.paused != Pause::Peer {
                return false;
            }
            record.state = State::Open;
            record.paused = Pause::None;
            record.peer_looped = false;
            // Their person continued their loop breaker's pause: this
            // side's repeats start over too, so the post that waited behind
            // it doesn't trip it here at once (Abbacchio rows 1193-1195).
            // Never the count: their word can't lift that bound, which only
            // this side's person resets (Fugo 33882).
            if looped {
                record.recent.clear();
            }
            true
        });
        if resumed == Some(true) {
            let who = factory.pipes.get(pipe).map(|record| record.peer_who()).unwrap_or_default();
            line(factory, pipe, format!("{who} resumed it."));
            agents::wake(factory, pipe);
        }
    }

    fn peer_closed(&self, factory: &Arc<Factory>, pipe: &str) {
        let who = factory.pipes.get(pipe).map(|record| record.peer_who()).unwrap_or_default();
        close_with(factory, pipe, &format!("{who} closed it."));
    }

    fn identity(&self, factory: &Arc<Factory>, pipe: &str, endpoint: &str, identity: &str) {
        identity::heard(factory, pipe, endpoint, identity);
    }

    fn own_identity(&self, factory: &Arc<Factory>) -> Option<String> {
        identity::own(factory)
    }

    fn holds_agents(&self, factory: &Arc<Factory>, pipe: &str) -> bool {
        identity::changed_account(factory, pipe)
    }

    fn peer_liaison(&self, factory: &Arc<Factory>, pipe: &str, liaison: pipe_link::Liaison) {
        waiting::heard(factory, pipe, liaison);
    }

    fn liaison_told(&self, factory: &Arc<Factory>, pipe: &str, liaison: pipe_link::Liaison) {
        waiting::told(factory, pipe, liaison);
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

/// Tests: a pipe both sides joined, waiting for this side's person to
/// approve it.
#[doc(hidden)]
pub fn approval_for_test(factory: &Arc<Factory>, project: &str, name: &str) -> String {
    let mut record = new_record(factory, project, name, "testing", State::Approval).expect("a pipe");
    record.peer = Some(PeerRecord { claktory: "Ana's Claktory".to_owned(), ..PeerRecord::default() });
    let id = record.id.clone();
    insert(factory, record);
    id
}

/// Tests: a pipe waiting on an invite that expires in `ms`, as
/// CreatePipeInvite makes it.
#[doc(hidden)]
pub fn invited_for_test(factory: &Arc<Factory>, project: &str, name: &str, ms: i64) -> String {
    let mut record = new_record(factory, project, name, "testing", State::Invited).expect("a pipe");
    record.invite_expires = now_ms() + ms;
    let id = record.id.clone();
    invited(factory, record);
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

/// Tests: the pipe pins `endpoint` as the other side's key now.
#[doc(hidden)]
pub fn peer_key_for_test(factory: &Arc<Factory>, pipe: &str, endpoint: &str) {
    change(factory, pipe, |record| {
        if let Some(peer) = record.peer.as_mut() {
            peer.endpoint_id = endpoint.to_owned();
        }
        true
    });
}

/// Tests: the four digits both people compare, as connecting sets them.
#[doc(hidden)]
pub fn digits_for_test(factory: &Arc<Factory>, pipe: &str, digits: &str) {
    change(factory, pipe, |record| record.digits = digits.to_owned());
}

/// Tests: the other side refused a message, as the transport says.
#[doc(hidden)]
pub fn refused_for_test(factory: &Arc<Factory>, pipe: &str, seq: u64, reason: &str) {
    let msg_id = message(factory, pipe, seq).expect("the message").msg_id;
    pipe_link::Store::refused(&Link, factory, pipe, &msg_id, reason);
}

/// Tests: a message of the person's, handed to the transport and waiting in
/// its queue (or delivered); its seq.
#[doc(hidden)]
pub fn sent_for_test(factory: &Arc<Factory>, pipe: &str, text: &str, delivered: bool) -> u64 {
    let state = if delivered { MessageState::Delivered } else { MessageState::Queued };
    let message =
        Message { msg_id: random_hex(16), author: Author::Me, name: "Matt".to_owned(), text: text.to_owned(), state, ..Message::default() };
    add(factory, pipe, message).expect("kept").seq
}

/// Tests: the other side paused the pipe (`true`) or resumed it, as the
/// transport says.
#[doc(hidden)]
pub fn peer_paused_for_test(factory: &Arc<Factory>, pipe: &str, paused: bool) {
    if paused {
        pipe_link::Store::peer_paused(&Link, factory, pipe, false);
    } else {
        pipe_link::Store::peer_resumed(&Link, factory, pipe, false);
    }
}

/// Tests: the other side closed the pipe, as the transport says.
#[doc(hidden)]
pub fn peer_closed_for_test(factory: &Arc<Factory>, pipe: &str) {
    pipe_link::Store::peer_closed(&Link, factory, pipe);
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

/// Tests: `agent` on `pipe` (and limited to it, `limited`) as a Claktory
/// from before #414's boundary could have it, unchecked.
#[doc(hidden)]
pub fn join_unchecked_for_test(factory: &Arc<Factory>, pipe: &str, agent: &str, limited: bool) {
    change(factory, pipe, |record| {
        if !record.members.iter().any(|id| id == agent) {
            record.members.push(agent.to_owned());
        }
        if limited {
            record.quarantined.push(agent.to_owned());
        }
    });
}

/// Tests: what Claktory does with the pipes when it starts again.
#[doc(hidden)]
pub fn start_for_test(factory: &Arc<Factory>) {
    quarantine::upgrade(factory);
    bound::upgrade(factory);
    sealed::upgrade(factory);
    lead::settle_all(factory);
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

/// Tests: makes `agent` a curator, as the person does (`curator::make`).
#[doc(hidden)]
pub async fn make_curator_for_test(factory: &Arc<Factory>, agent: &str, clear: bool) -> Result<(), String> {
    curator::make(factory, agent, "", clear).await
}

/// Tests: the person lifts `agent`'s curator limits (`curator::release`).
#[doc(hidden)]
pub async fn release_curator_for_test(factory: &Arc<Factory>, agent: &str) -> Result<(), String> {
    curator::release(factory, agent, false).await
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

/// Tests: a message counted towards the loop breaker, as `author` wrote it
/// ("their_agent", "their_person", "my_agent" or "me"), without the rate
/// limits that hold what arrives.
#[doc(hidden)]
pub fn count_for_loop_for_test(factory: &Arc<Factory>, pipe: &str, author: &str, text: &str) {
    let author = match author {
        "their_agent" => Author::TheirAgent,
        "their_person" => Author::TheirPerson,
        "my_agent" => Author::MyAgent,
        "me" => Author::Me,
        other => panic!("no author {other}"),
    };
    count_for_loop(factory, pipe, author, text);
}

/// Tests: the other side's loop breaker paused the pipe (`true`), or its
/// person continued that pause, as the transport says.
#[doc(hidden)]
pub fn peer_looped_for_test(factory: &Arc<Factory>, pipe: &str, paused: bool) {
    if paused {
        pipe_link::Store::peer_paused(&Link, factory, pipe, true);
    } else {
        pipe_link::Store::peer_resumed(&Link, factory, pipe, true);
    }
}

/// Tests: `agent` has read `key` (`agents::Kind`, such as
/// "browser:https://example.com"), as a page read through browser control
/// marks it.
#[doc(hidden)]
pub fn taint_for_test(factory: &Arc<Factory>, agent: &str, key: &str) {
    agents::taint(factory, agent, key);
}

/// Tests: `agent` waits on a usage limit until `until_ms` (0: for the
/// person, out of credit), as its run's wait sets it; None: it doesn't.
#[doc(hidden)]
pub fn set_usage_wait_for_test(factory: &Factory, agent: &str, until_ms: Option<i64>) {
    let mut inner = factory.lock();
    let model = inner.config.agent(agent).map(|known| known.model.clone()).unwrap_or_default();
    let provider = model.split_once('/').map_or(model.as_str(), |(provider, _)| provider).to_owned();
    let runtime = inner.agents.get_mut(agent).expect("the agent");
    runtime.usage_wait = until_ms.map(|until_ms| crate::waits::UsageWait {
        text: "Waiting for your Claude usage to reset".to_owned(),
        until_ms,
        switchable: true,
        provider,
    });
}

/// Tests: what the other side of `pipe` is to be told of its liaison now
/// (`waiting::due`).
#[doc(hidden)]
pub fn liaison_due_for_test(factory: &Factory, pipe: &str) -> Option<pipe_link::Liaison> {
    waiting::due(factory, &factory.pipes.get(pipe)?)
}

/// Tests: the other side heard what this side told it of its liaison.
#[doc(hidden)]
pub fn liaison_told_for_test(factory: &Factory, pipe: &str, liaison: pipe_link::Liaison) {
    waiting::told(factory, pipe, liaison);
}

/// Tests: the other side says its liaison waits or is awake, as the
/// transport hands it on.
#[doc(hidden)]
pub fn peer_liaison_for_test(factory: &Factory, pipe: &str, liaison: pipe_link::Liaison) {
    waiting::heard(factory, pipe, liaison);
}

/// Tests: tells each open pipe's other side of its liaison now, as the
/// watch does every 15 seconds.
#[doc(hidden)]
pub fn tell_liaisons_for_test(factory: &Arc<Factory>) {
    waiting::tell(factory);
}

/// Tests: the lead's wakes apart by `every` at least.
#[doc(hidden)]
pub fn set_wake_every(factory: &Factory, every: std::time::Duration) {
    factory.pipes.wake_every_ms.store(u64::try_from(every.as_millis()).unwrap_or(u64::MAX), Ordering::Relaxed);
}

/// Tests: judges how long ago the person attached a file as if `later` had
/// passed since.
#[doc(hidden)]
pub fn attached_later_for_test(factory: &Factory, later: std::time::Duration) {
    factory.pipes.attached_later_ms.store(u64::try_from(later.as_millis()).unwrap_or(u64::MAX), Ordering::Relaxed);
}

impl Pipes {
    pub(crate) fn wake_every_ms(&self) -> u64 {
        self.wake_every_ms.load(Ordering::Relaxed)
    }

    pub(crate) fn attached_later_ms(&self) -> i64 {
        i64::try_from(self.attached_later_ms.load(Ordering::Relaxed)).unwrap_or(i64::MAX)
    }
}

/// The pipe's messages as tests read them.
#[doc(hidden)]
pub fn messages_for_test(factory: &Factory, pipe: &str) -> Vec<v1::PipeMessage> {
    messages(factory, pipe, 0, 1000).0.iter().map(|message| for_person(factory, pipe, message)).collect()
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_name_the_other_side_chose_is_kept_only_when_claktory_can_show_it_as_its_own() {
        let now = 1_795_000_000_000;
        assert_eq!(super::offered_name(" Site and Marketing ", now), "Site and Marketing");
        let fallback = super::offered_name("CIaktory", now);
        assert!(fallback.len() > "Pipe joined ".len() && fallback.chars().last().is_some_and(char::is_alphabetic), "{fallback}");
        for posing in [
            "CIaktory Security",
            "C1aktory",
            "claktory-verify-your-vault",
            "Claktory — verify your vault",
            "Сlaktory",
            "\u{202e}Security",
            "",
        ] {
            assert!(super::offered_name(posing, now).starts_with("Pipe joined "), "{posing}");
        }
        assert!(super::offered_name(&"x".repeat(41), now).starts_with("Pipe joined "));
        assert_eq!(super::offered_name("Site ⇄ Marketing (Q4)", now), "Site ⇄ Marketing (Q4)");
        assert!(super::offered_name("C.l.a.k.t.0.r.y", now).starts_with("Pipe joined "));
        assert!(super::offered_name("Ｃlaktory", now).starts_with("Pipe joined "), "fullwidth");
    }
}
