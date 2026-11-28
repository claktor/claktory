//! Workspaces on one connection: the first goes whole, each later one only
//! as what changed (WorkspacePatch) to apps that asked for that, and one
//! with nothing new not at all. A busy Claktory changes one agent at a time,
//! and its Workspace (every agent's prompt, the changelog, the map) is far
//! bigger than the change, which matters most over the tunnel.
//!
//! A device on another machine (the iPhone, over the tunnel) gets them at
//! most once a second (Polpo's iphone-efficiency.md, fix 2): the first change
//! goes at once, and those in the second after it go together, as one patch,
//! when it ends. A busy team changes the Workspace several times a second,
//! each its own packet and a wake for the phone. What the person waits on
//! isn't held: a request's answer comes after what it changed (`release`),
//! and a change in what waits on them (a question, say) or in whether an
//! agent is working goes at once (`waiting_changed`), so a reply that has
//! come doesn't show as still being written (Zucchero's #453).
//!
//! What a model writes (EntryDelta, ChatEntryDelta) goes to such a device
//! the same way, at most every 0.1 s: the first piece at once, the pieces
//! after it within the while as one (`Deltas`).
//!
//! It works on the encoded bytes, field by field, so a field added to
//! Workspace needs nothing here: prost always encodes the same state to the
//! same bytes (maps are BTreeMaps for that).

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use bytes::Bytes;
use factory_protocol::v1::{self, ServerMessage, server_message::Body};
use prost::Message as _;

/// ServerMessage.workspace.
const WORKSPACE: u32 = 5;
/// Workspace.agents.
const AGENTS: u32 = 2;
/// Workspace.pipes, dm_sends, alerts and protected_places: they hold what
/// waits on the person (a pipe message to review, a direct message to
/// approve, an alert to read, a place macOS refused).
const WAITING_WORKSPACE: [u32; 4] = [26, 37, 51, 53];
/// Agent.id.
const AGENT_ID: u32 = 1;
/// Agent.status, waiting_seqs, waiting_items and waiting_versions: whether
/// it's working (it changes only as a run starts or ends) and its questions
/// for the person, and whether one of them changed (Squalo's B3).
const WAITING_AGENT: [u32; 4] = [7, 32, 42, 55];

/// How often a device on another machine gets the Workspace at most.
pub(crate) const REMOTE_EVERY: std::time::Duration = std::time::Duration::from_secs(1);
/// How often it gets what a model writes at most: as it shows reading, not
/// per piece (fix 3). A model sends 19–47 pieces a second (measured on
/// Sonnet 4.6, Haiku 4.5 and Haiku 5.5), each a frame of its own.
pub(crate) const REMOTE_TEXT_EVERY: std::time::Duration = std::time::Duration::from_millis(100);
/// ServerMessage.delta and chat_delta: what a model writes, piece by piece.
const DELTAS: [u32; 2] = [8, 47];
/// ServerMessage.entry and chat_entry: an entry going into a conversation.
const ENTRIES: [u32; 2] = [7, 46];
const LENGTH_DELIMITED: u8 = 2;

/// ServerMessage.workspace_revision and workspace_base_revision (#378).
const REVISION: u32 = 75;
const BASE_REVISION: u32 = 76;
/// How many Workspaces a device's resume can start from, its latest.
const RESUMES_KEPT: usize = 4;
/// How long one already replaced stays kept too: an app iOS suspends stops
/// reading, but its connection takes writes until the tunnel finds it gone
/// (about 30 s), so the one it holds may be many back (Zucchero row 423).
const RESUME_STALL: std::time::Duration = std::time::Duration::from_secs(120);
/// The most a device keeps however fast its Workspace changes.
const RESUMES_MOST: usize = 256;
/// How long a device's Workspaces are kept for it to resume from.
const RESUME_FOR: std::time::Duration = std::time::Duration::from_secs(6 * 3600);

/// What each device was last sent (#378): its latest few Workspaces by
/// revision, so an app that comes back with one gets only what changed
/// since. In memory only: a restart gives every app a whole Workspace.
pub(crate) struct Resumes {
    /// This process's: no other's revision ever matches.
    nonce: u64,
    next: std::sync::atomic::AtomicU64,
    held: std::sync::Mutex<HashMap<String, std::collections::VecDeque<Held>>>,
}

struct Held {
    revision: String,
    workspace: Arc<Parsed>,
    at: std::time::Instant,
}

impl Default for Resumes {
    fn default() -> Self {
        let mut nonce = [0u8; 8];
        let _ = getrandom::fill(&mut nonce);
        Self { nonce: u64::from_le_bytes(nonce), next: std::sync::atomic::AtomicU64::new(1), held: std::sync::Mutex::default() }
    }
}

impl Resumes {
    fn held(&self) -> std::sync::MutexGuard<'_, HashMap<String, std::collections::VecDeque<Held>>> {
        self.held.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Keeps `workspace` as sent to `device`: its new revision.
    fn keep(&self, device: &str, workspace: &Parsed) -> String {
        self.keep_at(device, workspace, std::time::Instant::now())
    }

    /// Keeps the latest few, and those replaced less than `RESUME_STALL`
    /// ago, at most `RESUMES_MOST`. Each shares what didn't change with the
    /// one before it, so keeping many costs only what changed.
    fn keep_at(&self, device: &str, workspace: &Parsed, now: std::time::Instant) -> String {
        let revision = format!("{:016x}-{}", self.nonce, self.next.fetch_add(1, std::sync::atomic::Ordering::Relaxed));
        let mut held = self.held();
        held.retain(|_, kept| kept.back().is_some_and(|last| now.saturating_duration_since(last.at) < RESUME_FOR));
        let kept = held.entry(device.to_owned()).or_default();
        let workspace = Arc::new(workspace.sharing(kept.back().map(|last| &*last.workspace)));
        kept.push_back(Held { revision: revision.clone(), workspace, at: now });
        while kept.len() > RESUMES_KEPT
            && (kept.len() > RESUMES_MOST || now.saturating_duration_since(kept[1].at) >= RESUME_STALL)
        {
            kept.pop_front();
        }
        revision
    }

    /// The Workspace `device` was sent at `revision`, if still kept.
    fn find(&self, device: &str, revision: &str) -> Option<Arc<Parsed>> {
        let held = self.held();
        let kept = held.get(device)?;
        kept.iter().find(|kept| kept.revision == revision && kept.at.elapsed() < RESUME_FOR).map(|kept| kept.workspace.clone())
    }
}

/// What one connection was last sent.
pub(crate) struct Workspaces {
    /// The app applies patches (Hello.workspace_patches).
    patches: bool,
    last: Option<Arc<Parsed>>,
    /// The revision of `last`, when it has one (#378).
    revision: String,
    /// Where revisions are kept, and for which device.
    resumes: Option<(Arc<Resumes>, String)>,
    /// What the app holds (Hello.workspace_revision), for the first
    /// Workspace to patch.
    resume: Option<(Arc<Parsed>, String)>,
    /// The next Workspace goes out even if unchanged: the first after
    /// Welcome, as a patch.
    send_even_unchanged: bool,
    /// At most one a while, for a device on another machine.
    pace: Option<Pace>,
    /// What a model writes, the same way.
    deltas: Deltas,
    /// What went out, by kind.
    pub(crate) tally: Tally,
    /// Each Workspace without the codes and digits, for an agent's command
    /// or the Windows app, by its authority
    /// (`authority::strip_workspace_for`).
    strip: Option<crate::authority::Authority>,
}

/// Workspaces held to go together.
struct Pace {
    every: std::time::Duration,
    /// When the last one went.
    sent_at: Option<std::time::Instant>,
    /// The latest since, to go when the while is up.
    held: Option<Bytes>,
}

impl Workspaces {
    pub(crate) fn new(patches: bool) -> Self {
        Self {
            patches,
            last: None,
            revision: String::new(),
            resumes: None,
            resume: None,
            send_even_unchanged: false,
            pace: None,
            deltas: Deltas::default(),
            tally: Tally::default(),
            strip: None,
        }
    }

    /// Sends each Workspace without the codes and digits a connection with
    /// `authority` mustn't see.
    pub(crate) fn stripped(mut self, authority: crate::authority::Authority) -> Self {
        self.strip = Some(authority);
        self
    }

    /// Sends a Workspace at most once `every`, and what a model writes at
    /// most once `text_every`, the rest held to go together.
    pub(crate) fn paced(mut self, every: std::time::Duration, text_every: std::time::Duration) -> Self {
        self.pace = Some(Pace { every, sent_at: None, held: None });
        self.deltas.every = Some(text_every);
        self
    }

    /// What goes now for `event`, an encoded ServerMessage, in order: the
    /// model's writing held first when `event` can't wait behind it.
    pub(crate) fn frames(&mut self, event: Bytes) -> Vec<Bytes> {
        self.frames_at(event, std::time::Instant::now())
    }

    /// A message of the person's going into a conversation brings the
    /// Workspace held before it, which shows what it did (the agent
    /// working, the queue taken), rather than a while after it.
    fn frames_at(&mut self, event: Bytes, now: std::time::Instant) -> Vec<Bytes> {
        let event = match self.strip {
            Some(authority) => stripped(event, authority),
            None => event,
        };
        let [held, event] = self.deltas.offer(event, now);
        let workspace = event.as_deref().filter(|event| persons_entry(event)).and_then(|_| self.release_at(now));
        let event = event.and_then(|event| self.outgoing_at(event, now));
        held.into_iter().chain(workspace).chain(event).collect()
    }

    /// When what's held goes, if anything is.
    pub(crate) fn due(&self) -> Option<std::time::Instant> {
        [self.workspace_due(), self.deltas.due()].into_iter().flatten().min()
    }

    fn workspace_due(&self) -> Option<std::time::Instant> {
        let pace = self.pace.as_ref()?;
        pace.held.as_ref()?;
        Some(pace.sent_at? + pace.every)
    }

    /// Everything held, to go now: the model's writing, and the Workspace as
    /// what changed since the last one sent, one patch.
    pub(crate) fn release(&mut self) -> Vec<Bytes> {
        let now = std::time::Instant::now();
        let held = self.deltas.take(now);
        held.into_iter().chain(self.release_at(now)).collect()
    }

    /// What's held and due now.
    pub(crate) fn release_due(&mut self) -> Vec<Bytes> {
        let now = std::time::Instant::now();
        let due = |at: Option<std::time::Instant>| at.is_some_and(|at| at <= now);
        let held = if due(self.deltas.due()) { self.deltas.take(now) } else { None };
        let workspace = if due(self.workspace_due()) { self.release_at(now) } else { None };
        held.into_iter().chain(workspace).collect()
    }

    fn release_at(&mut self, now: std::time::Instant) -> Option<Bytes> {
        let frame = self.pace.as_mut()?.held.take()?;
        let workspace = workspace_in(&frame)?;
        let next = Parsed::new(&workspace).map(Arc::new);
        self.send_now(frame, next, now)
    }

    /// Workspaces for `device`'s app, with revisions, resuming from the
    /// one it holds (`holds`, Hello.workspace_revision) if it's still
    /// kept. Only for apps that apply patches.
    pub(crate) fn resuming(patches: bool, resumes: Arc<Resumes>, device: &str, holds: &str) -> Self {
        let mut sent = Self::new(patches);
        if !patches {
            return sent;
        }
        if !holds.is_empty() {
            sent.resume = resumes.find(device, holds).map(|workspace| (workspace, holds.to_owned()));
            tracing::debug!(device, resumed = sent.resume.is_some(), "an app came back holding a Workspace");
        }
        sent.resumes = Some((resumes, device.to_owned()));
        sent
    }

    /// Starts over: the next Workspace goes whole (after a resync), or, the
    /// first time, as a patch against what the app holds.
    pub(crate) fn reset(&mut self) {
        // What follows is the whole state, at once, the conversation too.
        if let Some(pace) = &mut self.pace {
            pace.held = None;
            pace.sent_at = None;
        }
        self.deltas.held = None;
        self.deltas.sent_at = None;
        match self.resume.take() {
            Some((workspace, revision)) => {
                self.last = Some(workspace);
                self.revision = revision;
                self.send_even_unchanged = true;
            }
            None => {
                self.last = None;
                self.revision.clear();
            }
        }
    }

    /// What to send for `frame`, an encoded ServerMessage: itself, a patch
    /// in its place, or nothing when it's a Workspace the app already has.
    /// Held instead when paced and one went less than a while ago, unless
    /// what waits on the person changed.
    #[cfg(test)]
    fn outgoing(&mut self, frame: Bytes) -> Option<Bytes> {
        self.outgoing_at(frame, std::time::Instant::now())
    }

    fn outgoing_at(&mut self, frame: Bytes, now: std::time::Instant) -> Option<Bytes> {
        let Some(workspace) = workspace_in(&frame) else { return Some(frame) };
        let next = Parsed::new(&workspace).map(Arc::new);
        if let Some(pace) = &mut self.pace {
            let recent = pace.sent_at.is_some_and(|at| now.duration_since(at) < pace.every);
            if recent && !waiting_changed(self.last.as_deref(), next.as_deref()) {
                pace.held = Some(frame);
                return None;
            }
            pace.held = None;
        }
        self.send_now(frame, next, now)
    }

    /// What to send for the Workspace `frame` (`next`, parsed) now.
    fn send_now(&mut self, frame: Bytes, next: Option<Arc<Parsed>>, now: std::time::Instant) -> Option<Bytes> {
        let Some(next) = next else {
            // Not something this can compare; whole is always right.
            self.last = None;
            self.revision.clear();
            return Some(frame);
        };
        let sent = self.compared(frame, next);
        if sent.is_some()
            && let Some(pace) = &mut self.pace
        {
            pace.sent_at = Some(now);
        }
        sent
    }

    fn compared(&mut self, frame: Bytes, next: Arc<Parsed>) -> Option<Bytes> {
        let unchanged_is_news = std::mem::take(&mut self.send_even_unchanged);
        let base = std::mem::take(&mut self.revision);
        let Some(last) = self.last.replace(next.clone()) else { return Some(self.revised(frame, &next, "")) };
        if last == next && !unchanged_is_news {
            self.revision = base;
            return None;
        }
        if !self.patches {
            return Some(frame);
        }
        match patch(&last, &next) {
            Some(patch) => Some(self.revised(patch, &next, &base)),
            None => Some(self.revised(frame, &next, "")),
        }
    }

    /// `message` with the revision `workspace` gets (and, for a patch, the
    /// one it applies to), when this connection keeps revisions.
    fn revised(&mut self, message: Bytes, workspace: &Arc<Parsed>, base: &str) -> Bytes {
        let Some((resumes, device)) = &self.resumes else { return message };
        let revision = resumes.keep(device, workspace);
        let mut message = message.to_vec();
        append_string(&mut message, REVISION, &revision);
        if !base.is_empty() {
            append_string(&mut message, BASE_REVISION, base);
        }
        self.revision = revision;
        message.into()
    }
}

/// What a model writes, to a device on another machine: the first piece at
/// once, those after it within the while held as one.
#[derive(Default)]
struct Deltas {
    /// None: each goes as it comes.
    every: Option<std::time::Duration>,
    sent_at: Option<std::time::Instant>,
    held: Option<ServerMessage>,
}

impl Deltas {
    /// What goes now for `frame`: the piece held, when `frame` can't follow
    /// it otherwise (anything but a Workspace, which goes its own way, or
    /// another entry's piece), then `frame` unless it's held.
    fn offer(&mut self, frame: Bytes, now: std::time::Instant) -> [Option<Bytes>; 2] {
        let Some(every) = self.every else { return [None, Some(frame)] };
        let body = body_number(&frame);
        if body == Some(WORKSPACE) {
            return [None, Some(frame)];
        }
        let next = body.filter(|body| DELTAS.contains(body)).and_then(|_| ServerMessage::decode(frame.clone()).ok());
        let Some(next) = next else { return [self.take(now), Some(frame)] };
        if let Some(held) = &mut self.held
            && merged(held, &next)
        {
            return [None, None];
        }
        let held = self.take(now);
        if held.is_none() && self.sent_at.is_none_or(|at| now.duration_since(at) >= every) {
            self.sent_at = Some(now);
            return [None, Some(frame)];
        }
        self.held = Some(next);
        [held, None]
    }

    fn due(&self) -> Option<std::time::Instant> {
        self.held.as_ref()?;
        Some(self.sent_at? + self.every?)
    }

    /// The piece held, to go now.
    fn take(&mut self, now: std::time::Instant) -> Option<Bytes> {
        let held = self.held.take()?;
        self.sent_at = Some(now);
        Some(held.encode_to_vec().into())
    }
}

/// `frame`, a Workspace without the codes and digits; anything else as it
/// is. One that doesn't decode goes as an empty message, never whole.
fn stripped(frame: Bytes, authority: crate::authority::Authority) -> Bytes {
    if body_number(&frame) != Some(WORKSPACE) {
        return frame;
    }
    let Ok(mut message) = ServerMessage::decode(frame) else { return Bytes::new() };
    if let Some(Body::Workspace(workspace)) = &mut message.body {
        crate::authority::strip_workspace_for(workspace, authority);
    }
    message.encode_to_vec().into()
}

/// Appends `next`'s text to `held`'s when it's more of the same entry, with
/// the conversation's revision it brings.
fn merged(held: &mut ServerMessage, next: &ServerMessage) -> bool {
    // A watched conversation's (#514) never joins the selected one's.
    if held.watched != next.watched {
        return false;
    }
    match (&mut held.body, &next.body) {
        (Some(Body::Delta(held)), Some(Body::Delta(next))) if held.agent_id == next.agent_id && held.seq == next.seq => {
            held.text.push_str(&next.text);
        }
        (Some(Body::ChatDelta(held)), Some(Body::ChatDelta(next))) if held.chat_id == next.chat_id && held.seq == next.seq => {
            held.text.push_str(&next.text);
        }
        _ => return false,
    }
    held.transcript_revision.clone_from(&next.transcript_revision);
    true
}

/// Whether an encoded ServerMessage is an entry of the person's going into
/// the conversation shown, an agent's or a chat's.
fn persons_entry(frame: &[u8]) -> bool {
    if !body_number(frame).is_some_and(|body| ENTRIES.contains(&body)) {
        return false;
    }
    let entry = match ServerMessage::decode(frame).map(|message| message.body) {
        Ok(Some(Body::Entry(update))) => update.entry,
        Ok(Some(Body::ChatEntry(update))) => update.entry,
        _ => None,
    };
    matches!(entry, Some(v1::Entry { kind: Some(v1::entry::Kind::User(_)), .. }))
}

/// The body an encoded ServerMessage carries: its field's number.
fn body_number(frame: &[u8]) -> Option<u32> {
    Field::all(frame)?.into_iter().map(|field| field.number).find(|number| (2..REVISION).contains(number))
}

/// Whether what waits on the person changed from `last` to `next`: an
/// agent's status or questions, the pipes (their messages held for review)
/// or direct messages to approve. Unknown counts as yes.
fn waiting_changed(last: Option<&Parsed>, next: Option<&Parsed>) -> bool {
    let (Some(last), Some(next)) = (last, next) else { return true };
    if WAITING_WORKSPACE.iter().any(|number| last.fields.get(number) != next.fields.get(number)) {
        return true;
    }
    let before: HashMap<&str, &Bytes> = last.agents.iter().map(|(id, agent)| (id.as_str(), agent)).collect();
    let waiting = |agent: &Bytes| {
        let fields = fields_of(agent).unwrap_or_default();
        WAITING_AGENT.map(|number| fields.get(&number).cloned())
    };
    next.agents.iter().any(|(id, agent)| match before.get(id.as_str()) {
        Some(&old) if old == agent => false,
        Some(&old) => waiting(old) != waiting(agent),
        None => waiting(agent).iter().any(Option::is_some),
    })
}

/// What a connection sent, by kind: frames and bytes (before compression),
/// for its `disconnected` diagnostics event (Polpo's iphone-efficiency.md
/// §4), to measure what pacing saves.
#[derive(Default)]
pub(crate) struct Tally {
    counts: [(u64, u64); KINDS.len()],
    /// Until when a whole Transcript counts as asked for by this
    /// connection (its SelectAgent, Hello or resync); any other is
    /// `transcript_unasked` (Polpo 50160): sent because another device
    /// selected an agent (selection is shared), or the conversation was
    /// compacted or cleared.
    asked_until: Option<std::time::Instant>,
}

/// How long after this connection asked a whole Transcript still counts as
/// its own.
const ASKED_FOR: std::time::Duration = std::time::Duration::from_secs(2);

/// The kinds counted, and the ServerMessage bodies each is (Polpo 47790):
/// a Transcript whole or resumed (#413), an agent's earlier entries (page),
/// a chat's transcript and pages; and, before any of them, whatever came
/// for the conversation the connection watches (#514).
const KINDS: [(&str, &[u32]); 12] = [
    ("workspace", &[WORKSPACE]),
    ("patch", &[27]),
    ("entry", &[7, 46]),
    ("delta", &[8, 47]),
    ("transcript", &[TRANSCRIPT]),
    (TRANSCRIPT_RESUMED, &[]),
    (TRANSCRIPT_UNASKED, &[]),
    ("page", &[26]),
    ("chat", &[45, 48]),
    (WATCHED_KIND, &[]),
    ("file", &[14, 16, 59, 70]),
    ("other", &[]),
];
/// ServerMessage.transcript, and Transcript.resumed (#413).
const TRANSCRIPT: u32 = 6;
const RESUMED: u32 = 4;
const TRANSCRIPT_RESUMED: &str = "transcript_resumed";
const TRANSCRIPT_UNASKED: &str = "transcript_unasked";
/// ServerMessage.watched (#514).
const WATCHED: u32 = 90;
const WATCHED_KIND: &str = "watched";

/// Whether `message` has the bool (or other varint) field `number` set.
fn flag(message: &[u8], fields: &[Field], number: u32) -> bool {
    fields.iter().any(|field| field.number == number && field.wire_type == 0 && message[field.value.clone()].iter().any(|byte| byte & 0x7f != 0))
}

impl Tally {
    /// This connection asked for a conversation: the whole Transcript that
    /// follows within `ASKED_FOR` is its own.
    pub(crate) fn asked(&mut self) {
        self.asked_until = Some(std::time::Instant::now() + ASKED_FOR);
    }

    /// Counts `frame`, an encoded ServerMessage.
    pub(crate) fn count(&mut self, frame: &[u8]) {
        let fields = Field::all(frame).unwrap_or_default();
        let body = fields.iter().find(|field| (2..REVISION).contains(&field.number));
        let named = |name: &str| KINDS.iter().position(|(kind, _)| *kind == name).unwrap_or(KINDS.len() - 1);
        let resumed = body.filter(|body| body.number == TRANSCRIPT).is_some_and(|body| {
            let transcript = &frame[body.value.clone()];
            flag(transcript, &Field::all(transcript).unwrap_or_default(), RESUMED)
        });
        let kind = if flag(frame, &fields, WATCHED) {
            named(WATCHED_KIND)
        } else if resumed {
            named(TRANSCRIPT_RESUMED)
        } else if body.is_some_and(|body| body.number == TRANSCRIPT) {
            let asked = self.asked_until.take().is_some_and(|until| std::time::Instant::now() <= until);
            named(if asked { "transcript" } else { TRANSCRIPT_UNASKED })
        } else {
            let body = body.map(|body| body.number);
            KINDS.iter().position(|(_, bodies)| body.is_some_and(|body| bodies.contains(&body))).unwrap_or(KINDS.len() - 1)
        };
        let (frames, bytes) = &mut self.counts[kind];
        *frames += 1;
        *bytes += frame.len() as u64;
    }

    /// "frames=patch:120,delta:900 bytes=patch:4100,delta:90000", the
    /// kinds sent only.
    pub(crate) fn words(&self) -> String {
        let list = |pick: fn(&(u64, u64)) -> u64| {
            let parts: Vec<String> = KINDS
                .iter()
                .zip(&self.counts)
                .filter(|(_, count)| count.0 > 0)
                .map(|((kind, _), count)| format!("{kind}:{}", pick(count)))
                .collect();
            if parts.is_empty() { "0".to_owned() } else { parts.join(",") }
        };
        format!("frames={} bytes={}", list(|count| count.0), list(|count| count.1))
    }
}

/// Appends a string field to an encoded message.
fn append_string(message: &mut Vec<u8>, number: u32, value: &str) {
    let mut put = |mut value: u64| {
        while value >= 0x80 {
            message.push((value as u8) | 0x80);
            value >>= 7;
        }
        message.push(value as u8);
    };
    put(u64::from(number) << 3 | u64::from(LENGTH_DELIMITED));
    put(value.len() as u64);
    message.extend_from_slice(value.as_bytes());
}

/// A message's encoded fields: each field's occurrences (several for a
/// repeated one), keys included.
type Fields = BTreeMap<u32, Vec<Bytes>>;

/// A Workspace's encoded fields, and its agents in order: each one's id and
/// encoded Agent.
#[derive(PartialEq)]
struct Parsed {
    fields: Fields,
    agents: Vec<(String, Bytes)>,
}

impl Parsed {
    fn new(workspace: &Bytes) -> Option<Self> {
        let mut fields = Fields::new();
        let mut agents = Vec::new();
        for field in Field::all(workspace)? {
            if field.number == AGENTS && field.wire_type == LENGTH_DELIMITED {
                let agent = workspace.slice(field.value.clone());
                let id = Field::all(&agent)?
                    .into_iter()
                    .find(|field| field.number == AGENT_ID && field.wire_type == LENGTH_DELIMITED)
                    .map(|field| String::from_utf8_lossy(&agent[field.value]).into_owned())
                    .unwrap_or_default();
                agents.push((id, agent));
            } else {
                fields.entry(field.number).or_default().push(workspace.slice(field.whole));
            }
        }
        Some(Self { fields, agents })
    }

    /// The same Workspace with each piece `before` has unchanged taken from
    /// it, and the rest copied out of the frame it was read from, so that
    /// frame isn't held for it.
    fn sharing(&self, before: Option<&Parsed>) -> Parsed {
        let same = |was: Option<&Bytes>, now: &Bytes| match was {
            Some(was) if was == now => was.clone(),
            _ => Bytes::copy_from_slice(now),
        };
        let fields = self
            .fields
            .iter()
            .map(|(number, occurrences)| {
                let was = before.and_then(|before| before.fields.get(number));
                let occurrences = occurrences.iter().enumerate().map(|(at, now)| same(was.and_then(|was| was.get(at)), now)).collect();
                (*number, occurrences)
            })
            .collect();
        let was: HashMap<&str, &Bytes> =
            before.map(|before| before.agents.iter().map(|(id, agent)| (id.as_str(), agent)).collect()).unwrap_or_default();
        let agents = self.agents.iter().map(|(id, agent)| (id.clone(), same(was.get(id.as_str()).copied(), agent))).collect();
        Parsed { fields, agents }
    }
}

/// A message's fields by number.
fn fields_of(message: &Bytes) -> Option<Fields> {
    let mut fields = Fields::new();
    for field in Field::all(message)? {
        fields.entry(field.number).or_default().push(message.slice(field.whole));
    }
    Some(fields)
}

/// The numbers of the fields `next` has as `last` had them, and the encoded
/// rest of `next`. A field `next` doesn't have is in neither: it's empty now.
fn diff(last: &Fields, next: &Fields) -> (Vec<u32>, Vec<u8>) {
    let mut kept = Vec::new();
    let mut changed = Vec::new();
    for (number, occurrences) in next {
        if last.get(number) == Some(occurrences) {
            kept.push(*number);
        } else {
            occurrences.iter().for_each(|occurrence| changed.extend_from_slice(occurrence));
        }
    }
    (kept, changed)
}

/// `next` as a ServerMessage with a WorkspacePatch against `last`.
fn patch(last: &Parsed, next: &Parsed) -> Option<Bytes> {
    let (kept, changed) = diff(&last.fields, &next.fields);
    let mut workspace = v1::Workspace::decode(changed.as_slice()).ok()?;
    let before: HashMap<&str, &Bytes> = last.agents.iter().map(|(id, agent)| (id.as_str(), agent)).collect();
    let mut kept_agents = Vec::new();
    let mut kept_agent_fields = Vec::new();
    for (id, agent) in &next.agents {
        match before.get(id.as_str()) {
            Some(&old) if old == agent => {
                kept_agents.push(id.clone());
                workspace.agents.push(v1::Agent { id: id.clone(), ..Default::default() });
            }
            // Only what changed (its status, mostly), not its prompt again.
            Some(&old) if !id.is_empty() => {
                let (mut kept, changed) = diff(&fields_of(old)?, &fields_of(agent)?);
                kept.retain(|number| *number != AGENT_ID);
                let mut changed = v1::Agent::decode(changed.as_slice()).ok()?;
                changed.id.clone_from(id);
                workspace.agents.push(changed);
                kept_agent_fields.push(v1::KeptFields { id: id.clone(), kept });
            }
            _ => workspace.agents.push(v1::Agent::decode(agent.clone()).ok()?),
        }
    }
    let patch = v1::WorkspacePatch { workspace: Some(workspace), kept, kept_agents, kept_agent_fields };
    let message = ServerMessage { reply_to: 0, body: Some(Body::WorkspacePatch(patch)), ..Default::default() };
    Some(message.encode_to_vec().into())
}

/// The Workspace in an encoded ServerMessage, if that's what it carries.
fn workspace_in(frame: &Bytes) -> Option<Bytes> {
    Field::all(frame)?
        .into_iter()
        .find(|field| field.number == WORKSPACE && field.wire_type == LENGTH_DELIMITED)
        .map(|field| frame.slice(field.value))
}

/// One field of an encoded message: where it is, key and all, and where its
/// value is (a length-delimited one's without its length).
struct Field {
    number: u32,
    wire_type: u8,
    whole: std::ops::Range<usize>,
    value: std::ops::Range<usize>,
}

impl Field {
    /// A message's top-level fields, in order; None if it isn't well formed.
    fn all(message: &[u8]) -> Option<Vec<Field>> {
        let mut fields = Vec::new();
        let mut at = 0;
        while at < message.len() {
            let start = at;
            let key = varint(message, &mut at)?;
            let number = u32::try_from(key >> 3).ok()?;
            let wire_type = (key & 7) as u8;
            let value_start;
            match wire_type {
                0 => {
                    value_start = at;
                    varint(message, &mut at)?;
                }
                1 => {
                    value_start = at;
                    at = at.checked_add(8)?;
                }
                LENGTH_DELIMITED => {
                    let length = usize::try_from(varint(message, &mut at)?).ok()?;
                    value_start = at;
                    at = at.checked_add(length)?;
                }
                5 => {
                    value_start = at;
                    at = at.checked_add(4)?;
                }
                // Groups (3, 4) aren't in the protocol.
                _ => return None,
            }
            if at > message.len() || number == 0 {
                return None;
            }
            fields.push(Field { number, wire_type, whole: start..at, value: value_start..at });
        }
        Some(fields)
    }
}

fn varint(bytes: &[u8], at: &mut usize) -> Option<u64> {
    let mut value = 0u64;
    for shift in (0..64).step_by(7) {
        let byte = *bytes.get(*at)?;
        *at += 1;
        value |= u64::from(byte & 0x7f) << shift;
        if byte & 0x80 == 0 {
            return Some(value);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn agent(id: &str, prompt: &str) -> v1::Agent {
        v1::Agent { id: id.into(), name: id.to_uppercase(), prompt: prompt.into(), ..Default::default() }
    }

    fn workspace() -> v1::Workspace {
        v1::Workspace {
            agents: vec![agent("mike", "plans"), agent("sam", "builds"), agent("zoe", "draws")],
            version: "v0.1.4".into(),
            selected_agent_id: "sam".into(),
            changelog: vec![v1::ChangelogVersion { version: "v0.1.4".into(), ..Default::default() }],
            ..Default::default()
        }
    }

    fn frame(workspace: &v1::Workspace) -> Bytes {
        ServerMessage { reply_to: 0, body: Some(Body::Workspace(workspace.clone())), ..Default::default() }.encode_to_vec().into()
    }

    /// The encoded occurrences of `message`'s fields numbered in `kept`.
    fn kept_fields(message: &impl prost::Message, kept: &[u32]) -> Vec<u8> {
        let bytes = Bytes::from(message.encode_to_vec());
        let mut fields = Vec::new();
        for field in Field::all(&bytes).unwrap() {
            if kept.contains(&field.number) {
                fields.extend_from_slice(&bytes[field.whole]);
            }
        }
        fields
    }

    /// What an app does with a patch (the contract in the protocol).
    fn apply(last: &v1::Workspace, patch: v1::WorkspacePatch) -> v1::Workspace {
        let mut next = patch.workspace.unwrap();
        assert!(!patch.kept.contains(&AGENTS));
        next.merge(kept_fields(last, &patch.kept).as_slice()).unwrap();
        for agent in &mut next.agents {
            let old = last.agents.iter().find(|old| old.id == agent.id);
            if patch.kept_agents.contains(&agent.id) {
                *agent = old.unwrap().clone();
            } else if let Some(fields) = patch.kept_agent_fields.iter().find(|fields| fields.id == agent.id) {
                agent.merge(kept_fields(old.unwrap(), &fields.kept).as_slice()).unwrap();
            }
        }
        next
    }

    fn decode(frame: Bytes) -> Body {
        ServerMessage::decode(frame).unwrap().body.unwrap()
    }

    #[test]
    fn the_first_workspace_goes_whole_and_an_unchanged_one_not_at_all() {
        for patches in [false, true] {
            let mut sent = Workspaces::new(patches);
            assert!(matches!(decode(sent.outgoing(frame(&workspace())).unwrap()), Body::Workspace(_)));
            assert!(sent.outgoing(frame(&workspace())).is_none());
            sent.reset();
            assert!(matches!(decode(sent.outgoing(frame(&workspace())).unwrap()), Body::Workspace(_)));
        }
    }

    #[test]
    fn other_messages_pass_as_they_are() {
        let mut sent = Workspaces::new(true);
        let delta = ServerMessage {
            reply_to: 7,
            body: Some(Body::Delta(v1::EntryDelta { agent_id: "sam".into(), ..Default::default() })),
            ..Default::default()
        };
        let bytes = Bytes::from(delta.encode_to_vec());
        assert_eq!(sent.outgoing(bytes.clone()), Some(bytes));
    }

    #[test]
    fn an_app_without_patches_gets_each_change_whole() {
        let mut sent = Workspaces::new(false);
        sent.outgoing(frame(&workspace()));
        let mut next = workspace();
        next.agents[1].prompt = "builds the backend".into();
        assert_eq!(decode(sent.outgoing(frame(&next)).unwrap()), Body::Workspace(next));
    }

    #[test]
    fn a_patch_carries_only_what_changed_and_applies_to_the_same_workspace() {
        let mut sent = Workspaces::new(true);
        let first = workspace();
        sent.outgoing(frame(&first));

        let mut next = first.clone();
        next.agents[1].prompt = "builds the backend".into();
        next.agents.push(agent("omar", "pages"));
        next.agents.remove(0);
        next.selected_agent_id = String::new();
        next.config_error = "bad".into();
        let Body::WorkspacePatch(patch) = decode(sent.outgoing(frame(&next)).unwrap()) else { panic!("not a patch") };
        assert_eq!(patch.kept, vec![9, 18]);
        assert_eq!(patch.kept_agents, vec!["zoe".to_owned()]);
        // Sam's prompt changed; its name didn't.
        assert_eq!(patch.kept_agent_fields, vec![v1::KeptFields { id: "sam".into(), kept: vec![3] }]);
        let carried = patch.workspace.as_ref().unwrap();
        assert!(carried.changelog.is_empty() && carried.version.is_empty());
        assert_eq!(carried.agents[0], v1::Agent { id: "sam".into(), prompt: "builds the backend".into(), ..Default::default() });
        assert_eq!(carried.agents[1], v1::Agent { id: "zoe".into(), ..Default::default() });
        assert_eq!(carried.agents[2], agent("omar", "pages"));
        assert_eq!(apply(&first, patch), next);

        // And the next patch is against what was just sent.
        let mut third = next.clone();
        third.changelog.clear();
        third.agents[2].name = String::new();
        let Body::WorkspacePatch(patch) = decode(sent.outgoing(frame(&third)).unwrap()) else { panic!("not a patch") };
        assert_eq!(patch.kept, vec![5, 9]);
        assert_eq!(patch.kept_agents, vec!["sam".to_owned(), "zoe".to_owned()]);
        // A field emptied is neither carried nor kept.
        assert_eq!(patch.kept_agent_fields, vec![v1::KeptFields { id: "omar".into(), kept: vec![4] }]);
        assert_eq!(apply(&next, patch), third);
    }

    /// The revisions a message carries (workspace_revision, its base).
    fn revisions(frame: &Bytes) -> (String, String) {
        let message = ServerMessage::decode(frame.clone()).unwrap();
        (message.workspace_revision, message.workspace_base_revision)
    }

    #[test]
    fn an_app_resumes_from_the_workspace_it_holds_and_only_that() {
        let resumes = Arc::new(Resumes::default());
        let first = workspace();
        let mut sent = Workspaces::resuming(true, resumes.clone(), "phone", "");
        sent.reset();
        let whole = sent.outgoing(frame(&first)).unwrap();
        let (held, base) = revisions(&whole);
        assert!(!held.is_empty() && base.is_empty());
        assert!(matches!(decode(whole), Body::Workspace(_)));

        // Back with what it holds: what changed meanwhile, as a patch
        // against it, naming it.
        let mut next = first.clone();
        next.agents[1].prompt = "builds the backend".into();
        let mut back = Workspaces::resuming(true, resumes.clone(), "phone", &held);
        back.reset();
        let resumed = back.outgoing(frame(&next)).unwrap();
        let (now, base) = revisions(&resumed);
        assert_eq!(base, held);
        assert!(!now.is_empty() && now != held);
        let Body::WorkspacePatch(patch) = decode(resumed) else { panic!("not a patch") };
        assert_eq!(apply(&first, patch), next);
        // The next patch names the one before.
        let mut third = next.clone();
        third.config_error = "bad".into();
        let later = back.outgoing(frame(&third)).unwrap();
        assert_eq!(revisions(&later).1, now);
        // Nothing new: nothing sent, the revision stays.
        assert!(back.outgoing(frame(&third)).is_none());

        // Nothing changed while away: still a patch, all of it kept.
        let (held, _) = revisions(&later);
        let mut again = Workspaces::resuming(true, resumes.clone(), "phone", &held);
        again.reset();
        let same = again.outgoing(frame(&third)).unwrap();
        assert_eq!(revisions(&same).1, held);
        let Body::WorkspacePatch(patch) = decode(same) else { panic!("not a patch") };
        assert_eq!(patch.kept_agents.len(), 3);
        assert_eq!(apply(&third, patch), third);

        // Another device's revision, an unknown one, another process's, or
        // an app without patches: whole.
        let unknown = format!("{:016x}-999", resumes.nonce);
        let elsewhere = Arc::new(Resumes::default());
        for (resumes, device, holds, patches) in
            [(&resumes, "mac", held.as_str(), true), (&resumes, "phone", unknown.as_str(), true), (&elsewhere, "phone", held.as_str(), true), (&resumes, "phone", held.as_str(), false)]
        {
            let mut sent = Workspaces::resuming(patches, (*resumes).clone(), device, holds);
            sent.reset();
            let out = sent.outgoing(frame(&third)).unwrap();
            assert!(matches!(decode(out.clone()), Body::Workspace(_)), "{device} {holds}");
            assert_eq!(revisions(&out).1, "");
            assert_eq!(revisions(&out).0.is_empty(), !patches);
        }
    }

    #[test]
    fn a_device_keeps_its_latest_few_and_those_replaced_a_little_while_ago() {
        let resumes = Resumes::default();
        let parsed = Parsed::new(&workspace_in(&frame(&workspace())).unwrap()).unwrap();
        let start = std::time::Instant::now();
        let at = |secs: u64| start + std::time::Duration::from_secs(secs);
        // A suspended app's connection takes a patch a second until the
        // tunnel finds it gone: the one it holds is still there (row 423).
        let revisions: Vec<String> = (0..40).map(|n| resumes.keep_at("phone", &parsed, at(n))).collect();
        assert!(revisions.iter().all(|revision| resumes.find("phone", revision).is_some()));
        assert!(resumes.find("mac", revisions.last().unwrap()).is_none());
        // Replaced longer ago than that, gone; the latest few always stay.
        let later = resumes.keep_at("phone", &parsed, at(40 + RESUME_STALL.as_secs() + 1));
        assert!(revisions[..revisions.len() - (RESUMES_KEPT - 1)].iter().all(|revision| resumes.find("phone", revision).is_none()));
        assert!(revisions[revisions.len() - (RESUMES_KEPT - 1)..].iter().all(|revision| resumes.find("phone", revision).is_some()));
        assert!(resumes.find("phone", &later).is_some());
        // However fast it changes, at most so many.
        let burst: Vec<String> = (0..RESUMES_MOST + 10).map(|_| resumes.keep_at("phone", &parsed, at(1000))).collect();
        assert!(resumes.find("phone", &burst[9]).is_none());
        assert!(resumes.find("phone", &burst[10]).is_some());
        assert_eq!(resumes.held()["phone"].len(), RESUMES_MOST);
    }

    #[test]
    fn a_kept_workspace_shares_what_did_not_change_with_the_one_before() {
        let resumes = Resumes::default();
        let first = workspace();
        let mut second = first.clone();
        second.agents[0].prompt = "plans the week".into();
        let one = Parsed::new(&workspace_in(&frame(&first)).unwrap()).unwrap();
        let two = Parsed::new(&workspace_in(&frame(&second)).unwrap()).unwrap();
        let (a, b) = (resumes.keep("phone", &one), resumes.keep("phone", &two));
        let (a, b) = (resumes.find("phone", &a).unwrap(), resumes.find("phone", &b).unwrap());
        assert!(*a == one && *b == two, "the same Workspaces");
        let pointer = |bytes: &Bytes| bytes.as_ptr();
        assert_ne!(pointer(&a.agents[0].1), pointer(&b.agents[0].1), "the agent that changed");
        assert_eq!(pointer(&a.agents[1].1), pointer(&b.agents[1].1), "one that didn't");
        for (number, occurrences) in &b.fields {
            for (at, field) in occurrences.iter().enumerate() {
                assert_eq!(pointer(field), pointer(&a.fields[number][at]), "field {number}");
                assert_ne!(pointer(field), pointer(&two.fields[number][at]), "not the frame's");
            }
        }
    }

    #[test]
    fn paced_the_first_goes_at_once_and_a_burst_after_it_as_one_patch() {
        let every = std::time::Duration::from_secs(1);
        let start = std::time::Instant::now();
        let at = |ms: u64| start + std::time::Duration::from_millis(ms);
        let mut sent = Workspaces::new(true).paced(every, every);
        let first = workspace();
        assert!(matches!(decode(sent.outgoing_at(frame(&first), at(0)).unwrap()), Body::Workspace(_)));
        assert_eq!(sent.due(), None);

        // A change goes at once a while after the last.
        let mut second = first.clone();
        second.agents[0].prompt = "plans the week".into();
        let Body::WorkspacePatch(patch) = decode(sent.outgoing_at(frame(&second), at(1500)).unwrap()) else { panic!("not a patch") };
        assert_eq!(apply(&first, patch), second);

        // Within the while: held, the latest kept, and due when it's up.
        let mut latest = second.clone();
        for n in 0..5 {
            latest.agents[1].prompt = format!("builds {n}");
            assert!(sent.outgoing_at(frame(&latest), at(1600 + n * 100)).is_none());
        }
        assert_eq!(sent.due(), Some(at(2500)));
        let Body::WorkspacePatch(patch) = decode(sent.release_at(at(2500)).unwrap()) else { panic!("not a patch") };
        assert_eq!(apply(&second, patch), latest, "every change in the one patch");
        assert_eq!(sent.due(), None);
        assert!(sent.release_at(at(2600)).is_none(), "nothing more held");

        // Held, then nothing new after all: nothing goes.
        let mut back = latest.clone();
        back.config_error = "bad".into();
        assert!(sent.outgoing_at(frame(&back), at(2700)).is_none());
        assert!(sent.outgoing_at(frame(&latest), at(2800)).is_none());
        assert!(sent.release_at(at(3500)).is_none());

        // After a resync, the whole state goes at once.
        assert!(sent.outgoing_at(frame(&back), at(3600)).is_some());
        assert!(sent.outgoing_at(frame(&latest), at(3700)).is_none());
        sent.reset();
        assert_eq!(sent.due(), None);
        assert!(matches!(decode(sent.outgoing_at(frame(&latest), at(3800)).unwrap()), Body::Workspace(_)));
    }

    #[test]
    fn paced_a_change_in_what_waits_on_the_person_goes_at_once() {
        let start = std::time::Instant::now();
        let at = |ms: u64| start + std::time::Duration::from_millis(ms);
        let mut sent = Workspaces::new(true).paced(std::time::Duration::from_secs(1), std::time::Duration::from_secs(1));
        let first = workspace();
        sent.outgoing_at(frame(&first), at(0));
        let mut busy = first.clone();
        busy.agents[0].context_tokens = 1234;
        assert!(sent.outgoing_at(frame(&busy), at(100)).is_none(), "its context: held");

        // A question of an agent's, with what was held.
        let mut asks = busy.clone();
        asks.agents[2].waiting_seqs = vec![12];
        let Body::WorkspacePatch(patch) = decode(sent.outgoing_at(frame(&asks), at(200)).unwrap()) else { panic!("not a patch") };
        assert_eq!(apply(&first, patch), asks);
        assert_eq!(sent.due(), None, "nothing left held");
        // Answered (on another device, say): its card goes at once too.
        let mut answered = asks.clone();
        answered.agents[2].waiting_seqs.clear();
        assert!(sent.outgoing_at(frame(&answered), at(300)).is_some());

        // A new agent already asking, an item it names, a pipe, a direct
        // message, an agent starting or ending its work.
        let changes: [fn(&mut v1::Workspace); 6] = [
            |workspace| workspace.agents.push(v1::Agent { id: "omar".into(), waiting_seqs: vec![3], ..Default::default() }),
            |workspace| workspace.agents[1].waiting_items = vec![v1::WaitingItem::default()],
            |workspace| workspace.pipes = vec![v1::Pipe { id: "p".into(), ..Default::default() }],
            |workspace| workspace.dm_sends = vec![v1::DmSendWaiting::default()],
            |workspace| workspace.agents[0].status = v1::AgentStatus::Working.into(),
            |workspace| workspace.agents[0].status = v1::AgentStatus::Idle.into(),
        ];
        let mut now = answered;
        for (n, change) in changes.iter().enumerate() {
            change(&mut now);
            assert!(sent.outgoing_at(frame(&now), at(400 + n as u64 * 10)).is_some(), "change {n}");
        }
    }

    #[test]
    fn paced_a_message_of_the_persons_brings_the_workspace_held_before_it() {
        let start = std::time::Instant::now();
        let at = |ms: u64| start + std::time::Duration::from_millis(ms);
        let mut sent = Workspaces::new(true).paced(std::time::Duration::from_secs(1), std::time::Duration::from_millis(100));
        let first = workspace();
        sent.frames_at(frame(&first), at(0));
        let mut busy = first.clone();
        busy.agents[1].context_tokens = 1234;
        assert!(sent.frames_at(frame(&busy), at(100)).is_empty(), "its context: held");

        // Another's entry (an agent's reply, an event) leaves it held.
        let entry = |kind: v1::entry::Kind| -> Bytes {
            let entry = v1::Entry { seq: 7, kind: Some(kind), ..Default::default() };
            let update = v1::EntryUpdate { agent_id: "sam".into(), entry: Some(entry) };
            ServerMessage { body: Some(Body::Entry(update)), ..Default::default() }.encode_to_vec().into()
        };
        let reply = entry(v1::entry::Kind::Assistant(v1::AssistantText::default()));
        assert_eq!(sent.frames_at(reply.clone(), at(200)), vec![reply]);
        assert_eq!(sent.due(), Some(at(1000)));

        // The person's: the Workspace goes first, then the entry.
        let theirs = entry(v1::entry::Kind::User(v1::UserText { text: "hi".into(), ..Default::default() }));
        let frames = sent.frames_at(theirs.clone(), at(300));
        assert_eq!(frames.len(), 2, "{frames:?}");
        let Body::WorkspacePatch(patch) = decode(frames[0].clone()) else { panic!("not a patch") };
        assert_eq!(apply(&first, patch), busy);
        assert_eq!(frames[1], theirs);
        assert_eq!(sent.due(), None, "nothing left held");
        // Nothing held: it goes alone.
        assert_eq!(sent.frames_at(theirs.clone(), at(400)), vec![theirs]);

        // In a chat too.
        let mut idle = busy.clone();
        idle.agents[1].context_tokens = 5678;
        assert!(sent.frames_at(frame(&idle), at(500)).is_empty());
        let chat = v1::ChatEntryUpdate {
            chat_id: "c".into(),
            entry: Some(v1::Entry { kind: Some(v1::entry::Kind::User(v1::UserText::default())), ..Default::default() }),
        };
        let chat: Bytes = ServerMessage { body: Some(Body::ChatEntry(chat)), ..Default::default() }.encode_to_vec().into();
        assert_eq!(sent.frames_at(chat, at(600)).len(), 2);
    }

    fn delta(agent: &str, seq: u64, text: &str, revision: &str) -> Bytes {
        let delta = v1::EntryDelta { agent_id: agent.into(), seq, text: text.into() };
        ServerMessage { body: Some(Body::Delta(delta)), transcript_revision: revision.into(), ..Default::default() }.encode_to_vec().into()
    }

    /// A frame's delta: its entry, text and revision.
    fn text(frame: &Bytes) -> (u64, String, String) {
        let message = ServerMessage::decode(frame.clone()).unwrap();
        let Some(Body::Delta(delta)) = message.body else { panic!("not a delta: {message:?}") };
        (delta.seq, delta.text, message.transcript_revision)
    }

    #[test]
    fn paced_what_a_model_writes_goes_a_while_at_a_time_in_order() {
        let start = std::time::Instant::now();
        let at = |ms: u64| start + std::time::Duration::from_millis(ms);
        let mut deltas = Deltas { every: Some(std::time::Duration::from_millis(100)), ..Deltas::default() };
        // The first piece at once; the next within the while, held as one.
        let [held, now] = deltas.offer(delta("sam", 4, "Hel", "r1"), at(0));
        assert_eq!((held, now.map(|frame| text(&frame))), (None, Some((4, "Hel".into(), "r1".into()))));
        for (n, piece) in ["lo", " wor", "ld"].iter().enumerate() {
            assert_eq!(deltas.offer(delta("sam", 4, piece, &format!("r{}", n + 2)), at(10 + n as u64 * 10)), [None, None]);
        }
        assert_eq!(deltas.due(), Some(at(100)));
        assert_eq!(text(&deltas.take(at(100)).unwrap()), (4, "lo world".into(), "r4".into()), "appended, the last revision");
        assert_eq!(deltas.due(), None);

        // Another entry's piece: what's held goes first.
        deltas.offer(delta("sam", 4, "!", "r5"), at(150));
        let [held, now] = deltas.offer(delta("sam", 6, "Next", "r6"), at(160));
        assert_eq!((text(&held.unwrap()), now), ((4, "!".into(), "r5".into()), None));
        // Anything else too, but a Workspace goes its own way.
        assert_eq!(deltas.offer(frame(&workspace()), at(170)), [None, Some(frame(&workspace()))]);
        let entry: Bytes =
            ServerMessage { body: Some(Body::Entry(v1::EntryUpdate::default())), ..Default::default() }.encode_to_vec().into();
        let [held, now] = deltas.offer(entry.clone(), at(180));
        assert_eq!((text(&held.unwrap()), now), ((6, "Next".into(), "r6".into()), Some(entry)));
        // A while after the last: at once again.
        let [held, now] = deltas.offer(delta("sam", 8, "Done", "r7"), at(400));
        assert!(held.is_none() && now.is_some());

        // Unpaced (this computer's app): each as it comes.
        let mut each = Deltas::default();
        for n in 0..3 {
            assert!(matches!(each.offer(delta("sam", 4, "x", ""), at(n)), [None, Some(_)]));
        }
    }

    #[test]
    fn a_tally_counts_by_kind() {
        let mut tally = Tally::default();
        assert_eq!(tally.words(), "frames=0 bytes=0");
        let whole = frame(&workspace());
        tally.count(&whole);
        let delta =
            ServerMessage { body: Some(Body::Delta(v1::EntryDelta { text: "hi".into(), ..Default::default() })), ..Default::default() };
        let delta = delta.encode_to_vec();
        tally.count(&delta);
        tally.count(&delta);
        let ack = ServerMessage { reply_to: 3, body: Some(Body::Ack(v1::Ack::default())), ..Default::default() }.encode_to_vec();
        tally.count(&ack);
        assert_eq!(
            tally.words(),
            format!("frames=workspace:1,delta:2,other:1 bytes=workspace:{},delta:{},other:{}", whole.len(), 2 * delta.len(), ack.len())
        );
    }

    /// Polpo 50160: a whole Transcript this connection didn't ask for (by
    /// its SelectAgent, Hello or resync within two seconds before) is
    /// counted apart, as `transcript_unasked`; one ask covers one.
    #[test]
    fn a_tally_tells_a_transcript_this_connection_asked_for_from_one_it_didnt() {
        let whole = ServerMessage {
            body: Some(Body::Transcript(v1::Transcript { agent_id: "sam".into(), ..Default::default() })),
            ..Default::default()
        }
        .encode_to_vec();
        let mut tally = Tally::default();
        tally.count(&whole);
        tally.asked();
        tally.count(&whole);
        tally.count(&whole);
        tally.asked_until = Some(std::time::Instant::now() - std::time::Duration::from_millis(1));
        tally.count(&whole);
        assert_eq!(
            tally.words(),
            format!("frames=transcript:1,transcript_unasked:3 bytes=transcript:{},transcript_unasked:{}", whole.len(), 3 * whole.len())
        );
    }

    /// Polpo 47790: what used to be one "transcript" kind, apart: a whole
    /// Transcript, a resumed one (#413), an agent's earlier entries, a
    /// chat's, and anything for the watched conversation (#514).
    #[test]
    fn a_tally_tells_transcripts_pages_chats_and_watched_apart() {
        let encode = |body: Body, watched: bool| ServerMessage { body: Some(body), watched, ..Default::default() }.encode_to_vec();
        let whole = encode(Body::Transcript(v1::Transcript { agent_id: "sam".into(), ..Default::default() }), false);
        let resumed = encode(Body::Transcript(v1::Transcript { agent_id: "sam".into(), resumed: true, ..Default::default() }), false);
        let page = encode(Body::EntryPage(v1::EntryPage { agent_id: "sam".into(), ..Default::default() }), false);
        let chat = encode(Body::ChatTranscript(v1::ChatTranscript::default()), false);
        let chat_page = encode(Body::ChatEntryPage(v1::ChatEntryPage::default()), false);
        let watched = encode(Body::Transcript(v1::Transcript { agent_id: "ana".into(), resumed: true, ..Default::default() }), true);
        let watched_delta = encode(Body::Delta(v1::EntryDelta { text: "hi".into(), ..Default::default() }), true);
        let mut tally = Tally::default();
        tally.asked();
        for frame in [&whole, &resumed, &resumed, &page, &chat, &chat_page, &watched, &watched_delta] {
            tally.count(frame);
        }
        assert_eq!(
            tally.words(),
            format!(
                "frames=transcript:1,transcript_resumed:2,page:1,chat:2,watched:2 bytes=transcript:{},transcript_resumed:{},page:{},chat:{},watched:{}",
                whole.len(),
                2 * resumed.len(),
                page.len(),
                chat.len() + chat_page.len(),
                watched.len() + watched_delta.len()
            )
        );
    }

    #[test]
    fn maps_encode_the_same_every_time() {
        let mut options = BTreeMap::new();
        for id in ["a", "b", "c", "d", "e", "f"] {
            options.insert(id.to_owned(), v1::OptionValue { on: true, ..Default::default() });
        }
        let extension = v1::AgentExtension { id: "x".into(), enabled: true, options };
        let mut sent = Workspaces::new(true);
        let mut workspace = workspace();
        workspace.agents[0].extensions = vec![extension];
        sent.outgoing(frame(&workspace));
        assert!(sent.outgoing(frame(&workspace.clone())).is_none());
    }
}
