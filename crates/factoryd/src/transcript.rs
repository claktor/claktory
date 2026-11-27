//! An agent's conversation: the messages the model sees (its context) and the
//! entries the app shows.
//!
//! Each context window is one JSON file, `segment-NNNN.json`. Compaction starts
//! a new segment whose first message is the summary, so older segments stay as
//! read-only history and only the current one is rewritten. A restart reads
//! only the newest; the older ones' entries are read back when asked for
//! (`older`). A segment is written a message at a time, as JSON Lines
//! (`log`).

mod images;
pub(crate) mod log;
pub(crate) mod resume;
mod older;

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use factory_agent::{
    AgentEvent, AssistantMessage, AssistantPart, EventSource, Message, StopReason, StreamEvent, ToolResult, ToolResultPart, UserContent,
    UserMessage, UserPart,
};
use factory_protocol::v1::{self, Entry, NoticeLevel, ToolCallState, entry::Kind};
use serde::{Deserialize, Serialize};

pub use older::Older;
use older::{Index, Next, Section, Stamp};

/// Live tool output reaches clients at most this often.
const TOOL_OUTPUT_INTERVAL: Duration = Duration::from_millis(100);

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct CompactionInfo {
    summary: String,
    tokens_before: u64,
    /// Compacted while the agent was idle (`crate::idle`).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    idle: bool,
    /// What the request that wrote the summary took; None in segments
    /// written before it was kept.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    usage: Option<factory_agent::Usage>,
}

/// A segment as read.
type Segment = log::SegmentFile<Message>;

/// What a segment's model calls start with: the system prompt and tools,
/// written when the segment starts and sent byte for byte until the next, so
/// the provider's cache of the conversation holds. A change of the agent's
/// model writes a new one at its next model call, tools included; a change of
/// what its system prompt says of it (its instructions, name or project)
/// writes a new system prompt then, keeping the tools, which come first in
/// the cache.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Frame {
    /// The agent's own settings it was written for.
    pub settings: String,
    pub system_prompt: String,
    pub tools: Vec<factory_agent::ToolSpec>,
    /// The agent's own settings as it was last told of them, when that's
    /// since the frame was written; empty for `settings`.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub told: String,
    /// What the agent was told of since: agents it can message (id → how
    /// it's named) and models (id → name), so it hears of changes.
    #[serde(default)]
    pub roster: BTreeMap<String, String>,
    #[serde(default)]
    pub models: BTreeMap<String, String>,
    /// Its project's folders (id → "Name: /path (yours)").
    #[serde(default)]
    pub folders: BTreeMap<String, String>,
    /// The vault's secrets it may use (id → "Name (sets A, B): description").
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub secrets: BTreeMap<String, String>,
    /// The apps it was given scopes of (id → "Telegram (send, read)").
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub apps: BTreeMap<String, String>,
    /// The pipes it may use (id → what its system prompt says of each).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub pipes: BTreeMap<String, String>,
    /// Tools the agent was told it can call through extension_call, since
    /// they're not among `tools` (an extension turned on meanwhile).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub announced: Vec<String>,
}

impl Frame {
    /// The agent's own settings as it knows them.
    pub fn told(&self) -> &str {
        if self.told.is_empty() { &self.settings } else { &self.told }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum TranscriptError {
    #[error("could not read the transcript: {0}")]
    Io(#[from] std::io::Error),
    #[error("transcript file {path} is corrupt: {source}")]
    Corrupt { path: PathBuf, source: serde_json::Error },
}

/// A change clients need to hear about.
// Short-lived: made and sent at once, a few at a time, so an entry inline
// costs less than boxing every one.
#[allow(clippy::large_enum_variant)]
pub enum Change {
    Entry(Entry),
    Delta { seq: u64, text: String },
}

/// A save of the conversation's current segment, to make outside any lock
/// (`write`): what's new since the last save, appended, or the segment
/// whole. One that isn't made has the next save write the segment whole.
pub struct Persist {
    path: PathBuf,
    save: Save,
    /// Where its images go as files (`images`); None keeps them inline, as
    /// the version an update on trial rolls back to reads them.
    images: Option<PathBuf>,
    unsaved: Arc<AtomicBool>,
    made: bool,
}

enum Save {
    Append(Vec<log::Line<Message>>),
    /// As JSON Lines, or as JSON (`Transcript::old_form`).
    Whole {
        lines: bool,
        segment: Box<OwnedSegment>,
    },
}

/// A segment's parts, taken to write it whole.
struct OwnedSegment {
    compaction: Option<CompactionInfo>,
    cleared: Option<i64>,
    messages: Vec<Message>,
    pinned: BTreeMap<String, String>,
    frame: Option<Frame>,
}

impl Persist {
    /// Makes the save, off the async threads.
    pub async fn write(self) -> std::io::Result<()> {
        let mut persist = self;
        tokio::task::spawn_blocking(move || persist.write_now()).await.map_err(std::io::Error::other)?
    }

    /// Makes the save here.
    pub fn write_now(&mut self) -> std::io::Result<()> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        if let Some(dir) = &self.images {
            let messages: Vec<&mut Message> = match &mut self.save {
                Save::Append(lines) => lines
                    .iter_mut()
                    .filter_map(|line| match line {
                        log::Line::Message(message) => Some(message),
                        _ => None,
                    })
                    .collect(),
                Save::Whole { segment, .. } => segment.messages.iter_mut().collect(),
            };
            for message in messages {
                images::store(message, dir)?;
            }
        }
        // Stored images by name, but on trial: there, their base64.
        let write = || match &self.save {
            Save::Append(lines) if lines.is_empty() => Ok(()),
            Save::Append(lines) => {
                let mut bytes = Vec::new();
                for line in lines {
                    log::push_line(line, &mut bytes).map_err(std::io::Error::other)?;
                }
                log::append(&self.path, &bytes)
            }
            Save::Whole { lines, segment } => {
                let whole = log::WholeSegment {
                    compaction: segment.compaction.as_ref(),
                    cleared: segment.cleared,
                    messages: &segment.messages,
                    pinned: &segment.pinned,
                    frame: segment.frame.as_ref(),
                };
                log::replace(&self.path, &whole.bytes(*lines).map_err(std::io::Error::other)?)
            }
        };
        if self.images.is_some() { factory_agent::types::naming_stored_images(write) } else { write() }?;
        self.made = true;
        Ok(())
    }

    /// The file it writes.
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for Persist {
    fn drop(&mut self) {
        if !self.made {
            self.unsaved.store(true, Ordering::Relaxed);
        }
    }
}

/// What the current segment's file holds, as of the last save.
#[derive(Default)]
struct Saved {
    /// Its number; 0 for no file yet.
    segment: u32,
    /// JSON Lines, appended to.
    lines: bool,
    messages: usize,
    pinned: BTreeMap<String, String>,
    /// Its frame is the segment's.
    frame: bool,
}

#[derive(Default)]
pub struct Applied {
    pub changes: Vec<Change>,
    pub persist: Option<Persist>,
    /// The context size changed, or what waits for the person, so the
    /// workspace snapshot is stale.
    pub context_changed: bool,
    /// A message the agent was given (the person's or an event) is now in the
    /// conversation, and in `persist`.
    pub user_message: bool,
}

/// Maps the message being streamed to the entries showing it.
#[derive(Default)]
struct Live {
    /// Assistant content index → entry seq.
    parts: Vec<Option<u64>>,
    /// Tool call id → entry seq.
    tools: HashMap<String, u64>,
    /// When each running tool's output was last pushed.
    tool_pushed: HashMap<u64, Instant>,
    /// The seq of the first entry of the response being shown, which all
    /// its entries share as `reply`.
    reply: Option<u64>,
}

pub struct Transcript {
    dir: PathBuf,
    /// Whose conversation this is (the agent's id): a secret request's
    /// entries show in both the asker's and the admin agent's conversations,
    /// and which one waits for the person depends on which this is.
    owner: String,
    /// The seq of `entries`' first: the ones before are on disk (`older`),
    /// in `sections`.
    base: u64,
    /// The entries held, from `base` on: the newest segment's, and the ones
    /// before it a first page needs. An entry's seq is `base` + its index.
    entries: Vec<Entry>,
    /// The entries before `base` still held: those that can change or are
    /// looked up by what they show (`keeps`). They win over what's on disk.
    kept: BTreeMap<u64, Entry>,
    /// The older segments' entries on disk, before `base`.
    sections: std::sync::Arc<[Section]>,
    segment: u32,
    compaction: Option<CompactionInfo>,
    messages: Vec<Message>,
    /// Prompt sections fixed for this segment, by extension id (see
    /// `PromptKind::SegmentPinned`).
    pinned: BTreeMap<String, String>,
    /// When the person cleared the conversation, if that started this segment.
    cleared: Option<i64>,
    frame: Option<Frame>,
    /// Each segment's file, by number, as read.
    files: BTreeMap<u32, PathBuf>,
    /// What the current segment's file holds.
    saved: Saved,
    /// A save wasn't made: the next writes the segment whole.
    unsaved: Arc<AtomicBool>,
    /// Save a new segment, or one still JSON, as JSON: an update is on
    /// trial, and the version it rolls back to may not read JSON Lines.
    old_form: Arc<AtomicBool>,
    context_tokens: u64,
    /// The entries waiting for the person's answer (see `waits`), each with
    /// its fingerprint as held now (`fingerprint`).
    waiting: BTreeMap<u64, u64>,
    /// The person's message the next assistant text answers (its entry seq):
    /// set by their message, cleared by an event (the text then answers the
    /// event, which quotes nothing) and taken by the first assistant text.
    awaiting_reply: Option<u64>,
    live: Live,
    /// The model that wrote each finished response ("anthropic/claude-…"),
    /// as runs: from the response whose first entry is this seq on, until
    /// the next. For reporting a reply (`report`).
    models: Vec<(u64, String)>,
    /// Where each message the agent was given (the person's or an event,
    /// never a tool's result) is kept, by its entry's seq. A conversation
    /// goes back to right before one of these (`crate::fork`).
    origins: BTreeMap<u64, Origin>,
    /// It shows entries a reload wouldn't: notices, messages that never
    /// reached the model, what a response streamed before it failed. It
    /// stays in memory while it does (`reads_back_the_same`).
    shown_only: bool,
}

/// Where a message the agent was given is kept: its segment, and its index
/// in that segment's messages; and when it was given (its entry's time).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct Origin {
    segment: u32,
    index: u32,
    ms: i64,
}

/// A page of entries (`Transcript::page`): at hand, or to read from disk
/// first, off the async threads (`OlderPage::read`).
pub enum Page {
    Here(Vec<Entry>, bool),
    Older(OlderPage),
}

/// A page that starts before the entries held.
pub struct OlderPage {
    older: Older,
    /// The page's entries held in memory.
    held: Vec<Entry>,
    before: u64,
    limit: u32,
    from: Option<u64>,
}

impl OlderPage {
    /// The page's entries, and whether there are earlier ones.
    pub fn read(self) -> std::io::Result<(Vec<Entry>, bool)> {
        self.older.page(self.held, self.before, self.limit, self.from)
    }
}

/// Where a conversation can go back to (`crate::fork`): right before the
/// message at `index` of segment `segment`, given at `at_ms`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ForkPoint {
    pub segment: u32,
    pub index: u32,
    pub at_ms: i64,
}

/// What a snapshot of the conversation holds at most: the latest entries,
/// this many, and about this many bytes (at least one entry, however big).
const PAGE_ENTRIES: usize = 200;
const PAGE_BYTES: usize = 256 * 1024;
/// The most entries one page may be asked for.
const MAX_PAGE_ENTRIES: usize = 1000;

/// What a cleared conversation starts with.
const CLEARED: &str = "Conversation cleared";

impl Transcript {
    /// Reads the agent's conversation from its files, off the async threads:
    /// a long one is megabytes of JSON.
    pub async fn load(dir: PathBuf, owner: String) -> Result<Transcript, TranscriptError> {
        match tokio::task::spawn_blocking(move || Transcript::read(dir, owner)).await {
            Ok(result) => result,
            Err(error) => Err(TranscriptError::Io(std::io::Error::other(error))),
        }
    }

    fn empty(dir: PathBuf, owner: String, cleared: Option<i64>) -> Transcript {
        Transcript {
            dir,
            owner,
            base: 0,
            entries: Vec::new(),
            kept: BTreeMap::new(),
            sections: std::sync::Arc::new([]),
            segment: 1,
            compaction: None,
            messages: Vec::new(),
            pinned: BTreeMap::new(),
            cleared,
            frame: None,
            files: BTreeMap::new(),
            saved: Saved::default(),
            unsaved: Arc::new(AtomicBool::new(false)),
            old_form: Arc::new(AtomicBool::new(false)),
            context_tokens: 0,
            waiting: BTreeMap::new(),
            awaiting_reply: None,
            live: Live::default(),
            models: Vec::new(),
            origins: BTreeMap::new(),
            shown_only: false,
        }
    }

    /// Reads the conversation: its older segments' entries from the index
    /// of them (`older`) as far as it still matches them, the rest from the
    /// segments, which then go into the index; the newest segment, the
    /// model's context, always whole. Holds the newest segment's entries
    /// and the ones before it a first page needs.
    fn read(dir: PathBuf, owner: String) -> Result<Transcript, TranscriptError> {
        let mut transcript = Transcript::empty(dir, owner, None);
        transcript.files = log::files(&transcript.dir)?;
        let numbers: Vec<u32> = transcript.files.keys().copied().collect();
        let Some((&newest, older)) = numbers.split_last() else {
            older::forget(&transcript.dir, true);
            return Ok(transcript);
        };
        if older.is_empty() {
            older::forget(&transcript.dir, true);
        }
        let stamps =
            older.iter().map(|number| Ok((*number, Stamp::of(&transcript.files[number])?))).collect::<std::io::Result<Vec<_>>>()?;
        let index = if older.is_empty() { None } else { Index::read(&transcript.dir, &stamps) };
        // What's kept in memory of the older segments once they're on disk.
        let mut keep = BTreeSet::new();
        let mut sections = Vec::new();
        match &index {
            Some(index) => {
                transcript.resume(index, &mut keep);
                sections.clone_from(&index.sections);
            }
            None if !older.is_empty() => older::forget(&transcript.dir, false),
            None => {}
        }
        let cached = sections.len();
        let next = index.as_ref().and_then(|index| index.next);
        // The segment before the first one read, whose messages weren't.
        let mut before = cached.checked_sub(1).map(|at| older[at]);
        for (number, stamp) in &stamps[cached..] {
            let first = transcript.len();
            // An older segment is only shown, and images show as "[image]":
            // their data, often most of its bytes, isn't kept (#377).
            let segment = transcript.parse(*number, false)?;
            transcript.restore_after(*number, segment, next, before.take())?;
            sections.push(Section { number: *number, file: *stamp, first, count: transcript.len() - first, offset: 0, bytes: 0 });
        }
        // The index keeps the state after the older segments.
        let added = (sections.len() > cached).then(|| transcript.snapshot(&mut sections, cached, &mut keep, index.as_ref()));
        let newest_first = transcript.len();
        let segment = transcript.parse(newest, true)?;
        // Saves append to it, unless it must be written whole first: a crash
        // cut its last save off, it's JSON, or another name links to it.
        let path = &transcript.files[&newest];
        transcript.saved = Saved {
            segment: newest,
            lines: log::is_lines(path),
            messages: segment.messages.len(),
            pinned: segment.pinned.clone(),
            frame: true,
        };
        if segment.torn || (log::is_lines(path) && !log::appendable(path)) {
            transcript.unsaved.store(true, Ordering::Relaxed);
        }
        let summary_ms = summary_ms(&segment);
        let shown_from = transcript.restore_after(newest, segment, next, before.take())?;
        let next_now = summary_ms.map(|summary_ms| Next { number: newest, summary_ms, shown_from: u32::try_from(shown_from).unwrap_or(0) });
        let mut on_disk = cached;
        let written = match (added, index) {
            (Some((mut index, from, bytes)), _) => {
                index.next = next_now;
                index.write(&transcript.dir, from, &bytes).map(|()| on_disk = sections.len())
            }
            (None, Some(mut index)) if index.next != next_now => {
                index.next = next_now;
                index.write(&transcript.dir, index.end(), &[])
            }
            _ => Ok(()),
        };
        if let Err(error) = written {
            tracing::warn!(dir = %transcript.dir.display(), %error, "could not write a conversation's index; its older entries stay in memory");
        }
        // Nothing is running yet, so a call still without a result was cut off
        // by a restart. The model is told the same ("No result provided").
        for entry in transcript.kept.values_mut().chain(&mut transcript.entries) {
            if let Some(Kind::ToolCall(call)) = &mut entry.kind
                && matches!(call.state(), ToolCallState::Pending | ToolCallState::Running)
            {
                call.state = ToolCallState::Failed.into();
                call.output = "Interrupted: Claktory restarted before this finished.".to_owned();
            }
        }
        transcript.hold(sections, on_disk, newest_first, &keep);
        Ok(transcript)
    }

    /// A segment of the conversation, read whole or without its images' data.
    fn parse(&self, number: u32, whole: bool) -> Result<Segment, TranscriptError> {
        let path = self.files.get(&number).cloned().unwrap_or_else(|| log::path(&self.dir, number, true));
        let bytes = std::fs::read(&path)?;
        let parse = || log::parse::<Message>(&path, &bytes);
        if !whole {
            return factory_agent::types::without_image_data(parse).map_err(|source| TranscriptError::Corrupt { path, source });
        }
        let mut segment = parse().map_err(|source| TranscriptError::Corrupt { path: path.clone(), source })?;
        images::load(&mut segment.messages, &path);
        Ok(segment)
    }

    /// Picks up after the older segments the index has, from the state it
    /// keeps; `keep` gets the seqs of the entries it keeps.
    fn resume(&mut self, index: &Index, keep: &mut BTreeSet<u64>) {
        self.base = index.sections.last().map_or(0, |last| last.first + last.count);
        self.segment = index.sections.last().map_or(1, |last| last.number);
        for entry in index.kept() {
            keep.insert(entry.seq);
            self.track(&entry);
            self.kept.insert(entry.seq, entry);
        }
        self.awaiting_reply = index.awaiting_reply;
        self.live.tools = index.tools.iter().cloned().collect();
        self.models.clone_from(&index.models);
        self.origins = index.origins.iter().copied().collect();
    }

    /// Restores a segment read after the index's (`resume`): where its shown
    /// messages start is known from the index when it's the segment the
    /// index last knew as the newest (`next`); else, when it carries
    /// messages over from `before`, whose messages weren't read, from those.
    fn restore_after(&mut self, number: u32, segment: Segment, next: Option<Next>, before: Option<u32>) -> Result<usize, TranscriptError> {
        let summary_ms = summary_ms(&segment);
        let shown_from =
            next.filter(|next| next.number == number && Some(next.summary_ms) == summary_ms).map(|next| next.shown_from as usize);
        if shown_from.is_none()
            && summary_ms.is_some()
            && let Some(before) = before
        {
            self.messages = self.parse(before, false)?.messages;
        }
        Ok(self.restore(number, segment, shown_from))
    }

    /// The index of the older segments just read, `sections[from..]`: their
    /// entries, encoded, where they go in the entries file, and the state
    /// after them. `keep` gains what of them is kept in memory.
    fn snapshot(&self, sections: &mut [Section], from: usize, keep: &mut BTreeSet<u64>, index: Option<&Index>) -> (Index, u64, Vec<u8>) {
        let end = index.map_or(0, Index::end);
        let mut bytes = Vec::new();
        for section in &mut sections[from..] {
            let start = usize::try_from(section.first - self.base).unwrap_or(usize::MAX).min(self.entries.len());
            let entries = &self.entries[start..(start + section.count as usize).min(self.entries.len())];
            section.offset = end + bytes.len() as u64;
            older::encode(entries, &mut bytes);
            section.bytes = end + bytes.len() as u64 - section.offset;
            keep.extend(entries.iter().filter(|entry| keeps(entry)).map(|entry| entry.seq));
        }
        let index = Index {
            version: 0,
            sections: sections.to_vec(),
            awaiting_reply: self.awaiting_reply,
            tools: self.live.tools.iter().filter(|(_, seq)| keep.contains(seq)).map(|(id, seq)| (id.clone(), *seq)).collect(),
            models: self.models.clone(),
            origins: self.origins.iter().map(|(seq, origin)| (*seq, *origin)).collect(),
            kept: Index::encode_kept(keep.iter().filter_map(|seq| self.entry(*seq))),
            next: None,
        };
        (index, end, bytes)
    }

    /// Holds the newest segment's entries and the older sections' a first
    /// page needs; lets go of the rest, but for what's kept (`keep`).
    /// `sections[..on_disk]` are on disk; the rest stay held.
    fn hold(&mut self, sections: Vec<Section>, on_disk: usize, newest_first: u64, keep: &BTreeSet<u64>) {
        // sections[at..] are held.
        let mut at = sections.len();
        let mut from = newest_first;
        loop {
            if from < self.base {
                let older = Older::new(self.dir.clone(), sections[..=at].into(), self.kept.clone());
                match older.section(at) {
                    Ok(mut entries) => {
                        for entry in &entries {
                            self.kept.remove(&entry.seq);
                        }
                        entries.append(&mut self.entries);
                        self.entries = entries;
                        self.base = from;
                    }
                    Err(error) => {
                        tracing::warn!(dir = %self.dir.display(), %error, "could not read a conversation's older entries");
                        at += 1;
                        from = sections.get(at).map_or(newest_first, |section| section.first);
                        break;
                    }
                }
            }
            let held = &self.entries[usize::try_from(from - self.base).unwrap_or(usize::MAX).min(self.entries.len())..];
            if at == 0 || page_start(held, from, self.len(), 0, None).is_some() {
                break;
            }
            at -= 1;
            from = sections[at].first;
        }
        if at > on_disk {
            at = on_disk;
            from = sections.get(at).map_or(newest_first, |section| section.first);
        }
        let from = from.max(self.base);
        let gone = usize::try_from(from - self.base).unwrap_or(usize::MAX).min(self.entries.len());
        for entry in self.entries.drain(..gone) {
            if keep.contains(&entry.seq) {
                self.kept.insert(entry.seq, entry);
            }
        }
        self.base = from;
        self.sections = sections[..at].into();
    }

    /// A new, empty conversation the person started with /clear, and its
    /// first file. The old conversation's files must be out of `dir`.
    /// `old_form` as `save_old_form` takes it.
    pub fn cleared(dir: PathBuf, owner: String, old_form: Arc<AtomicBool>) -> (Transcript, Persist) {
        let now = now_ms();
        let mut transcript = Transcript::empty(dir, owner, Some(now));
        transcript.old_form = old_form;
        transcript.push(now, notice(CLEARED.to_owned(), NoticeLevel::Info));
        let persist = transcript.persist();
        (transcript, persist)
    }

    /// The latest entries, as a snapshot sends them, and whether there are
    /// earlier ones. Always held (`hold`).
    pub fn recent(&self) -> (Vec<Entry>, bool) {
        match self.page(self.len(), 0, None) {
            Some(Page::Here(entries, more)) => (entries, more),
            Some(Page::Older(page)) => (page.held, true),
            None => Default::default(),
        }
    }

    /// Entries before `before` (a seq, exclusive): from `from` on when set,
    /// else the latest, at most `limit` (0 for `PAGE_ENTRIES`) and about
    /// `PAGE_BYTES`, and never starting partway through a model response
    /// unless that response alone is bigger. Whether there are earlier ones.
    /// None when `before` is beyond the end; to read from disk first when it
    /// starts before the entries held.
    pub fn page(&self, before: u64, limit: u32, from: Option<u64>) -> Option<Page> {
        if before > self.len() {
            return None;
        }
        let offset = self.base.min(before);
        let held = if before > self.base { &self.entries[..usize::try_from(before - self.base).ok()?] } else { &[] };
        Some(match page_start(held, offset, before, limit, from) {
            Some(start) => Page::Here(held[usize::try_from(start - offset).ok()?..].to_vec(), start > 0),
            None => Page::Older(OlderPage { older: self.older(), held: held.to_vec(), before, limit, from }),
        })
    }

    /// The older entries on disk, to read off the async threads.
    pub fn older(&self) -> Older {
        Older::new(self.dir.clone(), self.sections.clone(), self.kept.clone())
    }

    /// How many entries the conversation has: the next one's seq.
    pub fn len(&self) -> u64 {
        self.base + self.entries.len() as u64
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// The entries held from `base` on (`window`'s first): the newest
    /// segment's and the ones before it a first page needs.
    pub fn window(&self) -> (u64, &[Entry]) {
        (self.base, &self.entries)
    }

    /// The held entries from `seq` on.
    pub fn since(&self, seq: u64) -> &[Entry] {
        let from = usize::try_from(seq.saturating_sub(self.base)).unwrap_or(usize::MAX).min(self.entries.len());
        &self.entries[from..]
    }

    /// The entry `seq`, if it's held: every card and call still waiting
    /// is, older ones too; read others with `older`.
    pub fn entry(&self, seq: u64) -> Option<&Entry> {
        match seq.checked_sub(self.base) {
            Some(index) => self.entries.get(usize::try_from(index).ok()?),
            None => self.kept.get(&seq),
        }
    }

    /// The entries held, oldest first: the older ones kept, then the window.
    pub fn in_memory(&self) -> impl DoubleEndedIterator<Item = &Entry> + '_ {
        self.kept.values().chain(&self.entries)
    }

    fn in_memory_mut(&mut self) -> impl DoubleEndedIterator<Item = &mut Entry> + '_ {
        self.kept.values_mut().chain(&mut self.entries)
    }

    /// The held entry `seq`, from the fields, so other fields stay free.
    fn at<'a>(entries: &'a mut [Entry], kept: &'a mut BTreeMap<u64, Entry>, base: u64, seq: u64) -> Option<&'a mut Entry> {
        match seq.checked_sub(base) {
            Some(index) => entries.get_mut(usize::try_from(index).ok()?),
            None => kept.get_mut(&seq),
        }
    }

    /// The seqs of the messages the agent was given, oldest first: where the
    /// conversation can go back to (`fork_point`).
    pub fn origin_seqs(&self) -> impl DoubleEndedIterator<Item = u64> + '_ {
        self.origins.keys().copied()
    }

    /// The model that wrote the response whose first entry is `reply`
    /// (Entry.reply), as "provider/model"; None while it's still being
    /// written.
    pub fn response_model(&self, reply: u64) -> Option<&str> {
        if self.live.reply == Some(reply) {
            return None;
        }
        let run = self.models.partition_point(|(first, _)| *first <= reply);
        run.checked_sub(1).map(|run| self.models[run].1.as_str())
    }

    /// The model's current context.
    pub fn messages(&self) -> &[Message] {
        &self.messages
    }

    /// This segment's pinned prompt sections, by extension id. They're saved
    /// with the segment's next write.
    pub fn pinned_mut(&mut self) -> &mut BTreeMap<String, String> {
        &mut self.pinned
    }

    /// This segment's frame; None until its first run.
    pub fn frame(&self) -> Option<&Frame> {
        self.frame.as_ref()
    }

    /// Sets the segment's frame; it's saved with the segment's next write.
    pub fn set_frame(&mut self, frame: Frame) {
        self.frame = Some(frame);
        self.saved.frame = false;
    }

    /// Has saves keep a new segment, or one still JSON, as JSON while `old`
    /// says so (an update on trial).
    pub fn save_old_form(&mut self, old: Arc<AtomicBool>) {
        self.old_form = old;
    }

    pub fn context_tokens(&self) -> u64 {
        self.context_tokens
    }

    /// The seqs of the entries waiting for the person's answer, oldest
    /// first: requests for secrets and proposals still pending.
    pub fn waiting_seqs(&self) -> Vec<u64> {
        self.waiting.keys().copied().collect()
    }

    /// Each waiting entry's fingerprint (Agent.waiting_versions, Squalo's
    /// B3): it changes whenever anything in the entry does, so the app
    /// fetches again only the cards that changed.
    pub fn waiting_versions(&self) -> BTreeMap<u64, u64> {
        self.waiting.clone()
    }

    /// Notes whether `entry` waits for the person now, and as what.
    fn track(&mut self, entry: &Entry) {
        if waits(entry.kind.as_ref(), &self.owner) {
            self.waiting.insert(entry.seq, fingerprint(entry));
        } else {
            self.waiting.remove(&entry.seq);
        }
    }

    /// Replaces the latest shown item of the same kind and id, keeping its entry metadata.
    fn replace_shown(&mut self, kind: Kind) -> Option<Entry> {
        let entry = self.in_memory_mut().rev().find(|entry| match (&entry.kind, &kind) {
            (Some(Kind::Proposal(shown)), Kind::Proposal(updated)) => shown.id == updated.id,
            (Some(Kind::SecretRequest(shown)), Kind::SecretRequest(updated)) => shown.id == updated.id,
            (Some(Kind::ConnectProposal(shown)), Kind::ConnectProposal(updated)) => shown.id == updated.id,
            (Some(Kind::DevicePairing(shown)), Kind::DevicePairing(updated)) => shown.id == updated.id,
            (Some(Kind::AskPerson(shown)), Kind::AskPerson(updated)) => shown.id == updated.id,
            (Some(Kind::PipeStart(shown)), Kind::PipeStart(updated)) => shown.id == updated.id,
            (Some(Kind::AppGrant(shown)), Kind::AppGrant(updated)) => shown.id == updated.id,
            (Some(Kind::AppAccess(shown)), Kind::AppAccess(updated)) => shown.id == updated.id,
            _ => false,
        })?;
        entry.kind = Some(kind);
        let entry = entry.clone();
        self.track(&entry);
        Some(entry)
    }

    /// Shows a proposal to Rivet as it is now (decided, say); the changed
    /// entry, or None if the conversation doesn't show it.
    pub fn set_proposal(&mut self, proposal: v1::Proposal) -> Option<Entry> {
        self.replace_shown(Kind::Proposal(proposal))
    }

    /// Marks the add-secret cards shown for request `id` closed; the
    /// entries that changed.
    fn close_collect_cards(&mut self, id: &str) -> Vec<Entry> {
        let mut closed = Vec::new();
        for entry in self.in_memory_mut() {
            if let Some(Kind::SecretRequest(request)) = &mut entry.kind
                && request.id == id
                && request.collecting
            {
                request.collecting = false;
                closed.push(entry.clone());
            }
        }
        for entry in &closed {
            self.track(entry);
        }
        closed
    }

    /// Shows an agent's request for a secret as it is now (decided, say); the
    /// changed entry, or None if the conversation doesn't show it.
    pub fn set_secret_request(&mut self, request: v1::SecretRequest) -> Option<Entry> {
        self.replace_shown(Kind::SecretRequest(request))
    }

    /// Shows an agent's proposal to connect a service as it is now; the
    /// changed entry, or None if the conversation doesn't show it.
    pub fn set_connect_proposal(&mut self, proposal: v1::ConnectProposal) -> Option<Entry> {
        self.replace_shown(Kind::ConnectProposal(proposal))
    }

    /// Shows a device an agent adds as it is now (added, say); the changed
    /// entry, or None if the conversation doesn't show it. An update that
    /// doesn't say what the device is keeps what the card says: the kind
    /// comes from the devices_add call, and factory.db doesn't record it.
    pub fn set_device_pairing(&mut self, mut pairing: v1::DevicePairing) -> Option<Entry> {
        if pairing.kind() == v1::DeviceKind::Unspecified {
            let shown = self.in_memory().rev().find_map(|entry| match &entry.kind {
                Some(Kind::DevicePairing(shown)) if shown.id == pairing.id => Some(shown.kind),
                _ => None,
            });
            if let Some(kind) = shown {
                pairing.kind = kind;
            }
        }
        self.replace_shown(Kind::DevicePairing(pairing))
    }

    /// Shows an agent's card for starting a pipe as it is now (done, say);
    /// the changed entry, or None if the conversation doesn't show it.
    pub fn set_pipe_start(&mut self, start: v1::PipeStart) -> Option<Entry> {
        self.replace_shown(Kind::PipeStart(start))
    }

    /// Shows Rivet's proposal to give an agent access to an app as it is
    /// now (decided, say); the changed entry, or None if the conversation
    /// doesn't show it.
    pub fn set_app_grant(&mut self, proposal: v1::AppGrantProposal) -> Option<Entry> {
        self.replace_shown(Kind::AppGrant(proposal))
    }

    /// Shows Rivet's proposal of what an app reads and who uses it as it is
    /// now (decided, say); the changed entry, or None if the conversation
    /// doesn't show it.
    pub fn set_app_access(&mut self, proposal: v1::AppAccessProposal) -> Option<Entry> {
        self.replace_shown(Kind::AppAccess(proposal))
    }

    /// Shows a pipe secret approval as decided; None if it isn't shown.
    /// `reply`: the person's reply when they answered it with a comment
    /// (#370), else None.
    pub fn set_secret_approval(&mut self, id: &str, state: v1::SecretApprovalState, decided_ms: i64, reply: Option<&str>) -> Option<Entry> {
        let entry = self.in_memory_mut().find(|entry| matches!(&entry.kind, Some(Kind::SecretApproval(shown)) if shown.id == id))?;
        if let Some(Kind::SecretApproval(approval)) = &mut entry.kind {
            approval.state = state.into();
            approval.decided_ms = decided_ms;
            approval.replied = reply.is_some();
            approval.comment = reply.unwrap_or_default().to_owned();
        }
        let entry = entry.clone();
        self.track(&entry);
        Some(entry)
    }

    /// Shows an agent's question for the person as it is now (answered,
    /// say); the changed entry, or None if the conversation doesn't show it.
    pub fn set_ask(&mut self, ask: v1::AskPerson) -> Option<Entry> {
        self.replace_shown(Kind::AskPerson(ask))
    }

    /// Says which device the card at `seq` was decided from (#514,
    /// `crate::answered_on`); the changed entry, or None if it isn't shown.
    pub fn set_answered_on(&mut self, seq: u64, on: v1::AnsweredOn) -> Option<Entry> {
        let entry = self.in_memory_mut().find(|entry| entry.seq == seq)?;
        entry.answered_on = Some(on);
        let entry = entry.clone();
        self.track(&entry);
        Some(entry)
    }

    /// Adds a harness notice. Notices are for display only and aren't saved.
    pub fn notice(&mut self, text: impl Into<String>, level: NoticeLevel) -> Entry {
        self.shown_only = true;
        self.push(now_ms(), notice(text.into(), level))
    }

    /// Shows a message that never reached the model (e.g. the run couldn't
    /// start). Display only.
    pub fn unsent(&mut self, message: &UserMessage) -> Entry {
        self.shown_only = true;
        self.push(now_ms(), user_kind(message))
    }

    /// Whether reading its files again gives what it holds: everything it
    /// shows is saved, and nothing it shows is for display only. Then it
    /// can be unloaded (`crate::unload`).
    pub fn reads_back_the_same(&self) -> bool {
        !self.shown_only
            && self.live.reply.is_none()
            && self.live.parts.iter().all(Option::is_none)
            && !self.unsaved.load(Ordering::Relaxed)
            // No save of it is still to be written: each holds `unsaved`.
            && Arc::strong_count(&self.unsaved) == 1
            && self.saved.segment == self.segment
            && self.saved.messages == self.messages.len()
            && self.saved.pinned == self.pinned
            && (self.saved.frame || self.frame.is_none())
    }

    /// Drops what the response being shown streamed into, which stays
    /// shown, for display only, when no complete message took it over.
    fn end_live(&mut self) {
        self.shown_only |= self.live.reply.is_some() || self.live.parts.iter().any(Option::is_some);
        self.live.parts.clear();
        self.live.reply = None;
    }

    pub fn apply(&mut self, event: AgentEvent) -> Result<Applied, serde_json::Error> {
        let mut applied = Applied::default();
        match event {
            AgentEvent::MessageStart { message: Message::Assistant(_) } => self.end_live(),
            AgentEvent::MessageUpdate { assistant_message_event, .. } => self.stream(*assistant_message_event, &mut applied.changes),
            AgentEvent::MessageEnd { message } => {
                let waiting = self.waiting.len();
                let at = (self.segment, u32::try_from(self.messages.len()).unwrap_or(u32::MAX));
                for entry in self.show(&message, at) {
                    applied.changes.push(Change::Entry(entry));
                }
                // A request or proposal now waits for the person.
                applied.context_changed |= self.waiting.len() != waiting;
                if let Message::Assistant(assistant) = &message {
                    let tokens = context_tokens(assistant);
                    if tokens > 0 && tokens != self.context_tokens {
                        self.context_tokens = tokens;
                        applied.context_changed = true;
                    }
                }
                applied.user_message = matches!(message, Message::User(_));
                self.messages.push(message);
                applied.persist = Some(self.persist());
            }
            AgentEvent::ToolExecutionStart { tool_call_id, .. } => {
                let times = Times { started: Some(now_ms()), ended: None };
                if let Some(entry) = self.update_tool(&tool_call_id, None, ToolCallState::Running, times) {
                    applied.changes.push(Change::Entry(entry));
                }
            }
            AgentEvent::ToolExecutionUpdate { tool_call_id, partial_result, .. } => {
                let Some(&seq) = self.live.tools.get(&tool_call_id) else { return Ok(applied) };
                let now = Instant::now();
                let due = self.live.tool_pushed.get(&seq).is_none_or(|last| now.duration_since(*last) >= TOOL_OUTPUT_INTERVAL);
                if due {
                    self.live.tool_pushed.insert(seq, now);
                    let output = ToolOutput { output: result_text(&partial_result), details: String::new() };
                    if let Some(entry) = self.update_tool(&tool_call_id, Some(output), ToolCallState::Running, Times::default()) {
                        applied.changes.push(Change::Entry(entry));
                    }
                }
            }
            AgentEvent::ToolExecutionEnd { tool_call_id, result, is_error, .. } => {
                let state = if is_error { ToolCallState::Failed } else { ToolCallState::Done };
                let times = Times { started: None, ended: Some(now_ms()) };
                let output = ToolOutput { output: result_text(&result), details: details_json(&result.details) };
                if let Some(entry) = self.update_tool(&tool_call_id, Some(output), state, times) {
                    applied.changes.push(Change::Entry(entry));
                }
            }
            // The response failed partway and will be requested again. What it
            // streamed stays visible; the retry streams into new entries.
            AgentEvent::Retrying { attempt, delay_ms, .. } => {
                self.end_live();
                let seconds = delay_ms.div_ceil(1000);
                let text = format!("The model request failed. Retrying in {seconds}s (attempt {attempt}).");
                applied.changes.push(Change::Entry(self.notice(text, NoticeLevel::Info)));
            }
            // The response failed for want of an account with capacity: the
            // run waits (`crate::waits` says so) and asks again.
            AgentEvent::Waiting { .. } => self.end_live(),
            AgentEvent::Compacted { summary, tokens_before, messages, usage, reason } => {
                let idle = reason == factory_agent::compaction::Reason::Idle;
                self.segment += 1;
                // Stamped like the summary message, so a reload shows the same entry.
                let timestamp = messages.first().map_or_else(now_ms, |message| message.timestamp() as i64);
                let entry = self.push(timestamp, Kind::Compaction(v1::Compaction { summary: summary.clone(), tokens_before, idle }));
                applied.changes.push(Change::Entry(entry));
                self.compaction = Some(CompactionInfo { summary, tokens_before, idle, usage: Some(usage) });
                self.messages = messages;
                self.pinned.clear();
                self.cleared = None;
                self.frame = None;
                self.context_tokens = 0;
                applied.context_changed = true;
                applied.persist = Some(self.persist());
            }
            _ => {}
        }
        Ok(applied)
    }

    /// Shows a segment read back after the ones before it. Messages
    /// compaction carried over are shown where they appeared live, at the end
    /// of the segment before: those before `shown_from` when it's given, else
    /// as many as end that segment. Where the messages shown start.
    fn restore(&mut self, number: u32, segment: Segment, shown_from: Option<usize>) -> usize {
        self.segment = number;
        if let Some(time) = segment.cleared {
            self.push(time, notice(CLEARED.to_owned(), NoticeLevel::Info));
        }
        let mut messages = &segment.messages[..];
        if let Some(info) = &segment.compaction
            && let Some((summary, kept)) = messages.split_first()
        {
            // The summary message shows as the divider. Messages compaction kept
            // after it (the prompt that triggered it) were shown at the end of
            // the previous segment, which is where they appeared live.
            let kind =
                Kind::Compaction(v1::Compaction { summary: info.summary.clone(), tokens_before: info.tokens_before, idle: info.idle });
            self.push(summary.timestamp() as i64, kind);
            let carried = match shown_from {
                Some(from) => from.saturating_sub(1).min(kept.len()),
                None => {
                    let mut carried = kept.len().min(self.messages.len());
                    // The previous segment's were read without their images' data.
                    let ends_with = |carried: usize| {
                        let tail = &self.messages[self.messages.len() - carried..];
                        tail.iter().zip(&kept[..carried]).all(|(before, kept)| same_but_images(before, kept))
                    };
                    while carried > 0 && !ends_with(carried) {
                        carried -= 1;
                    }
                    carried
                }
            };
            messages = &kept[carried..];
        }
        self.context_tokens = 0;
        // Where the messages shown start in the segment's own.
        let offset = segment.messages.len() - messages.len();
        for (index, message) in messages.iter().enumerate() {
            self.show(message, (number, u32::try_from(offset + index).unwrap_or(u32::MAX)));
            if let Message::Assistant(assistant) = message {
                let tokens = context_tokens(assistant);
                if tokens > 0 {
                    self.context_tokens = tokens;
                }
            }
        }
        self.compaction = segment.compaction;
        self.messages = segment.messages;
        self.pinned = segment.pinned;
        self.cleared = segment.cleared;
        self.frame = segment.frame;
        offset
    }

    /// Shows a complete message, reusing the entries its stream already
    /// created; `at` is where it's kept (its segment and index there).
    fn show(&mut self, message: &Message, at: (u32, u32)) -> Vec<Entry> {
        match message {
            Message::User(user) => {
                let entry = self.push(user.timestamp as i64, user_kind(user));
                self.origins.insert(entry.seq, Origin { segment: at.0, index: at.1, ms: entry.timestamp_ms });
                // The next assistant text answers the latest prompt: the
                // person's message is quoted by it; an event's reply isn't.
                self.awaiting_reply = user.event.is_none().then_some(entry.seq);
                vec![entry]
            }
            Message::Assistant(assistant) => self.show_assistant(assistant),
            Message::ToolResult(result) => {
                let state = if result.is_error { ToolCallState::Failed } else { ToolCallState::Done };
                let output = parts_text(&result.content);
                // The result's own times, so a reload shows what was shown live.
                let times = Times { started: result.started.map(|time| time as i64), ended: Some(result.timestamp as i64) };
                let output = ToolOutput { output, details: details_json(&result.details) };
                let mut changed: Vec<Entry> = self.update_tool(&result.tool_call_id, Some(output), state, times).into_iter().collect();
                // A proposal to Rivet shows after the call that made it.
                if let Some(proposal) = crate::proposals::proposed(&result.tool_name, result.is_error, &result.details) {
                    changed.push(self.push(result.timestamp as i64, Kind::Proposal(proposal)));
                }
                // So does a request for a secret.
                if let Some(request) = crate::vault::asks::requested(&result.tool_name, result.is_error, &result.details) {
                    changed.push(self.push(result.timestamp as i64, Kind::SecretRequest(request)));
                }
                // And the add-secret card an admin agent opens for one.
                if let Some(request) = crate::vault::asks::collected(&result.tool_name, result.is_error, &result.details) {
                    // A card opened again for the same request: the earlier one
                    // is closed, so only the new one waits.
                    changed.extend(self.close_collect_cards(&request.id));
                    changed.push(self.push(result.timestamp as i64, Kind::SecretRequest(request)));
                }
                // And a proposal to connect a service.
                if let Some(proposal) = crate::connectors::proposals::proposed(&result.tool_name, result.is_error, &result.details) {
                    changed.push(self.push(result.timestamp as i64, Kind::ConnectProposal(proposal)));
                }
                // And a secret a tainted agent asked the person for.
                if let Some(approval) = crate::pipes::agents::asked(&result.tool_name, &result.details) {
                    changed.push(self.push(result.timestamp as i64, Kind::SecretApproval(approval)));
                }
                // And a question for the person.
                if let Some(ask) = crate::extensions::ask::asked(&result.tool_name, result.is_error, &result.details) {
                    changed.push(self.push(result.timestamp as i64, Kind::AskPerson(ask)));
                }
                // And a device the agent adds, with its code.
                if let Some(pairing) = crate::extensions::devices::started(&result.tool_name, result.is_error, &result.details) {
                    changed.push(self.push(result.timestamp as i64, Kind::DevicePairing(pairing)));
                }
                // And a card for the person to start a pipe.
                if let Some(start) = crate::pipes::start::started(&result.tool_name, result.is_error, &result.details) {
                    changed.push(self.push(result.timestamp as i64, Kind::PipeStart(start)));
                }
                // And Rivet's proposal to give an agent access to an app.
                if let Some(proposal) = crate::user_apps::proposed(&result.tool_name, result.is_error, &result.details) {
                    changed.push(self.push(result.timestamp as i64, Kind::AppGrant(proposal)));
                }
                // And its proposal of what one of Claktory's apps reads.
                if let Some(proposal) = crate::apps::access::proposed(&result.tool_name, result.is_error, &result.details) {
                    changed.push(self.push(result.timestamp as i64, Kind::AppAccess(proposal)));
                }
                changed
            }
            Message::System(_) => Vec::new(),
        }
    }

    fn show_assistant(&mut self, message: &AssistantMessage) -> Vec<Entry> {
        let timestamp = message.timestamp as i64;
        let mut changed = Vec::new();
        for (index, part) in message.content.iter().enumerate() {
            let streamed = self.live.parts.get(index).copied().flatten();
            let empty = match part {
                AssistantPart::Text { text, .. } => text.is_empty(),
                AssistantPart::Thinking { thinking, .. } => thinking.is_empty(),
                AssistantPart::ToolCall { .. } => false,
            };
            // Failed runs end with an empty text part; there's nothing to show.
            if empty && streamed.is_none() {
                continue;
            }
            let kind = match part {
                AssistantPart::Text { text, .. } => Kind::Assistant(v1::AssistantText { text: text.clone() }),
                AssistantPart::Thinking { thinking, .. } => Kind::Thinking(v1::Thinking { text: thinking.clone() }),
                AssistantPart::ToolCall { id, name, arguments, .. } => Kind::ToolCall(v1::ToolCall {
                    call_id: id.clone(),
                    name: name.clone(),
                    input: pretty(arguments),
                    output: String::new(),
                    state: ToolCallState::Pending.into(),
                    timeout_ms: timeout_ms(name, arguments),
                    ..v1::ToolCall::default()
                }),
            };
            let seq = match streamed {
                Some(seq) => {
                    let Some(entry) = Self::at(&mut self.entries, &mut self.kept, self.base, seq) else { continue };
                    if entry.kind.as_ref() != Some(&kind) {
                        entry.kind = Some(kind);
                        changed.push(entry.clone());
                    }
                    seq
                }
                None => {
                    let mut entry = self.push_part(timestamp, kind);
                    // Shown whole (a reload, a response that didn't stream):
                    // the first text after the person's message answers it.
                    if matches!(part, AssistantPart::Text { .. })
                        && let Some(from) = self.awaiting_reply.take()
                    {
                        entry.replied_to = Some(from);
                        if let Some(held) = Self::at(&mut self.entries, &mut self.kept, self.base, entry.seq) {
                            held.replied_to = Some(from);
                        }
                    }
                    let seq = entry.seq;
                    changed.push(entry);
                    seq
                }
            };
            if let AssistantPart::ToolCall { id, .. } = part {
                self.live.tools.insert(id.clone(), seq);
            }
        }
        if let Some(reply) = self.live.reply {
            let model = format!("{}/{}", message.provider, message.model);
            if self.models.last().is_none_or(|(_, last)| *last != model) {
                self.models.push((reply, model));
            }
        }
        self.live.parts.clear();
        self.live.reply = None;
        match message.stop_reason {
            // A refusal is itself, not a failure: the model's safety
            // classifiers declined the step, and the card says so.
            StopReason::Error if let Some(refusal) = factory_agent::refusal::of(message) => {
                let text = message.error_message.clone().unwrap_or_else(|| "The model declined this request.".to_owned());
                changed.push(self.push(
                    timestamp,
                    Kind::Notice(v1::Notice {
                        text,
                        level: NoticeLevel::Error.into(),
                        refused: None,
                        refusal_category: refusal.shown_category().to_owned(),
                    }),
                ));
            }
            StopReason::Error => {
                let text = message.error_message.clone().unwrap_or_else(|| "The model request failed.".to_owned());
                // The provider refused the credentials: whose, so the app can offer to change them.
                let refused = factory_agent::accounts::refused(message).map(|refused| v1::CredentialsRefused {
                    provider_id: message.provider.clone(),
                    account_id: refused.account,
                    method_id: refused.method,
                });
                changed.push(self.push(
                    timestamp,
                    Kind::Notice(v1::Notice { text, level: NoticeLevel::Error.into(), refused, refusal_category: String::new() }),
                ));
            }
            StopReason::Aborted => changed.push(self.push(timestamp, notice("Stopped.".to_owned(), NoticeLevel::Info))),
            _ => {}
        }
        // The provider's own fallback answered after the model declined:
        // say who, since the person picked the model that didn't answer.
        if let Some((from, to, category)) = factory_agent::refusal::fallback_served(message) {
            let text = factory_agent::refusal::handoff_text(&message.provider, &from, &to, &category);
            changed.push(self.push(timestamp, notice(text, NoticeLevel::Info)));
        }
        changed
    }

    fn stream(&mut self, event: StreamEvent, changes: &mut Vec<Change>) {
        match event {
            StreamEvent::TextStart { content_index, partial } => {
                let kind = Kind::Assistant(v1::AssistantText::default());
                let mut entry = self.start_part(content_index, partial.timestamp as i64, kind);
                // The first text after the person's message answers it, from
                // its first streamed word on.
                if let Some(from) = self.awaiting_reply.take() {
                    entry.replied_to = Some(from);
                    if let Some(held) = Self::at(&mut self.entries, &mut self.kept, self.base, entry.seq) {
                        held.replied_to = Some(from);
                    }
                }
                changes.push(Change::Entry(entry));
            }
            StreamEvent::ThinkingStart { content_index, partial } => {
                let kind = Kind::Thinking(v1::Thinking::default());
                changes.push(Change::Entry(self.start_part(content_index, partial.timestamp as i64, kind)));
            }
            StreamEvent::ToolcallStart { content_index, partial } => {
                let name = match partial.content.get(content_index) {
                    Some(AssistantPart::ToolCall { name, .. }) => name.clone(),
                    _ => String::new(),
                };
                let kind = Kind::ToolCall(v1::ToolCall { name, state: ToolCallState::Pending.into(), ..v1::ToolCall::default() });
                changes.push(Change::Entry(self.start_part(content_index, partial.timestamp as i64, kind)));
            }
            StreamEvent::TextDelta { content_index, delta, .. } | StreamEvent::ThinkingDelta { content_index, delta, .. } => {
                let Some(seq) = self.live.parts.get(content_index).copied().flatten() else { return };
                let Some(entry) = Self::at(&mut self.entries, &mut self.kept, self.base, seq) else { return };
                match &mut entry.kind {
                    Some(Kind::Assistant(v1::AssistantText { text })) | Some(Kind::Thinking(v1::Thinking { text })) => {
                        text.push_str(&delta);
                        changes.push(Change::Delta { seq, text: delta });
                    }
                    _ => {}
                }
            }
            // The arguments as the model writes them, raw JSON so far, so the
            // call shows what it does early; replaced with the whole at the end.
            StreamEvent::ToolcallDelta { content_index, delta, .. } => {
                let Some(seq) = self.live.parts.get(content_index).copied().flatten() else { return };
                let now = Instant::now();
                let due = self.live.tool_pushed.get(&seq).is_none_or(|last| now.duration_since(*last) >= TOOL_OUTPUT_INTERVAL);
                let Some(entry) = Self::at(&mut self.entries, &mut self.kept, self.base, seq) else { return };
                if let Some(Kind::ToolCall(call)) = &mut entry.kind {
                    call.input.push_str(&delta);
                    if due {
                        self.live.tool_pushed.insert(seq, now);
                        changes.push(Change::Entry(entry.clone()));
                    }
                }
            }
            StreamEvent::ToolcallEnd { content_index, tool_call: AssistantPart::ToolCall { id, name, arguments, .. }, .. } => {
                let Some(seq) = self.live.parts.get(content_index).copied().flatten() else { return };
                let Some(entry) = Self::at(&mut self.entries, &mut self.kept, self.base, seq) else { return };
                if let Some(Kind::ToolCall(call)) = &mut entry.kind {
                    call.call_id = id.clone();
                    call.input = pretty(&arguments);
                    call.timeout_ms = timeout_ms(&name, &arguments);
                    changes.push(Change::Entry(entry.clone()));
                }
                self.live.tools.insert(id, seq);
            }
            _ => {}
        }
    }

    fn start_part(&mut self, index: usize, timestamp: i64, kind: Kind) -> Entry {
        let entry = self.push_part(timestamp, kind);
        if self.live.parts.len() <= index {
            self.live.parts.resize(index + 1, None);
        }
        self.live.parts[index] = Some(entry.seq);
        entry
    }

    /// `output` is the call's output so far (with no details) while it runs,
    /// and its result with details once it's done.
    fn update_tool(&mut self, call_id: &str, output: Option<ToolOutput>, state: ToolCallState, times: Times) -> Option<Entry> {
        let seq = *self.live.tools.get(call_id)?;
        let entry = Self::at(&mut self.entries, &mut self.kept, self.base, seq)?;
        let Some(Kind::ToolCall(call)) = &mut entry.kind else { return None };
        if matches!(state, ToolCallState::Done | ToolCallState::Failed) {
            self.live.tool_pushed.remove(&seq);
        }
        let unchanged = call.state == i32::from(state)
            && output.as_ref().is_none_or(|output| output.output == call.output && output.details == call.details)
            && times.started.is_none_or(|started| started == call.started)
            && times.ended.is_none_or(|ended| ended == call.ended);
        if unchanged {
            return None;
        }
        if let Some(output) = output {
            call.output = output.output;
            call.details = output.details;
        }
        if let Some(started) = times.started {
            call.started = started;
        }
        if let Some(ended) = times.ended {
            call.ended = ended;
        }
        call.state = state.into();
        Some(entry.clone())
    }

    fn push(&mut self, timestamp_ms: i64, kind: Kind) -> Entry {
        let entry = Entry { seq: self.len(), timestamp_ms, kind: Some(kind), reply: 0, replied_to: None, answered_on: None };
        self.entries.push(entry.clone());
        self.track(&entry);
        entry
    }

    /// Adds an entry of the model response being shown.
    fn push_part(&mut self, timestamp_ms: i64, kind: Kind) -> Entry {
        let seq = self.len();
        let reply = *self.live.reply.get_or_insert(seq);
        let entry = Entry { seq, timestamp_ms, kind: Some(kind), reply, replied_to: None, answered_on: None };
        self.entries.push(entry.clone());
        entry
    }

    /// Where the conversation goes back to when the person (or an agent)
    /// takes it back to the entry `seq` (`entry`, read back if it isn't
    /// held), which must still have `timestamp_ms`: right before it, for a
    /// message the agent was given (the person's or an event); right before
    /// the next one, for the agent's reply, so the whole run it's part of
    /// stays. Else why not, in the person's words.
    pub fn fork_point(&self, seq: u64, entry: Option<&Entry>, timestamp_ms: i64) -> Result<ForkPoint, &'static str> {
        let Some(entry) = entry.filter(|entry| entry.seq == seq && entry.timestamp_ms == timestamp_ms && seq < self.len()) else {
            return Err("The conversation changed. Try again.");
        };
        let origin = match entry.kind {
            Some(Kind::User(_) | Kind::Event(_)) => match self.origins.get(&seq) {
                Some(origin) => *origin,
                None => return Err("This message never reached the agent, so it can't go back to it."),
            },
            Some(Kind::Assistant(_) | Kind::Thinking(_)) => match self.origins.range(seq + 1..).next() {
                Some((_, origin)) => *origin,
                None => return Err("Nothing came after this yet, so there's nothing to go back from."),
            },
            _ => return Err("A conversation goes back only to a message or a reply."),
        };
        // Before the first message there's nothing to keep.
        let first = self.origins.values().map(|origin| (origin.segment, origin.index)).min();
        if origin.index == 0 && first == Some((origin.segment, origin.index)) {
            return Err("This is where the conversation starts, so there's nothing before it to go back to.");
        }
        Ok(ForkPoint { segment: origin.segment, index: origin.index, at_ms: origin.ms })
    }

    /// The save of the current segment: the messages, frame and pinned
    /// sections new since the last, as lines to append; or the segment whole,
    /// for a new one, one still JSON, one a crash cut off or another name
    /// links to, and after a save that wasn't made.
    pub(crate) fn persist(&mut self) -> Persist {
        let unsaved = self.unsaved.swap(false, Ordering::Relaxed);
        let same = self.saved.segment == self.segment;
        let append = !unsaved && same && self.saved.lines && self.saved.messages <= self.messages.len();
        let (lines, save) = if append {
            let mut lines: Vec<log::Line<Message>> = self.messages[self.saved.messages..].iter().cloned().map(log::Line::Message).collect();
            if self.pinned != self.saved.pinned {
                lines.push(log::Line::Pinned(self.pinned.clone()));
            }
            if !self.saved.frame
                && let Some(frame) = &self.frame
            {
                lines.push(log::Line::Frame(frame.clone()));
            }
            (true, Save::Append(lines))
        } else {
            let lines = !(self.old_form.load(Ordering::Relaxed) && !(same && self.saved.lines));
            let segment = OwnedSegment {
                compaction: self.compaction.clone(),
                cleared: self.cleared,
                messages: self.messages.clone(),
                pinned: self.pinned.clone(),
                frame: self.frame.clone(),
            };
            (lines, Save::Whole { lines, segment: Box::new(segment) })
        };
        let path = log::path(&self.dir, self.segment, lines);
        self.files.insert(self.segment, path.clone());
        self.saved = Saved { segment: self.segment, lines, messages: self.messages.len(), pinned: self.pinned.clone(), frame: true };
        let images = (!self.old_form.load(Ordering::Relaxed)).then(|| self.dir.join(images::DIR));
        Persist { path, save, images, unsaved: self.unsaved.clone(), made: false }
    }
}

/// A page of `id`'s conversation (`Transcript::page`), read from disk when
/// it starts before the entries held. None when `before` is beyond its end
/// or what's on disk can't be read.
pub(crate) async fn page_of(
    factory: &crate::factory::Factory,
    id: &str,
    before: u64,
    limit: u32,
    from: Option<u64>,
) -> Option<(Vec<Entry>, bool)> {
    let page = {
        let inner = factory.lock();
        inner.agents.get(id)?.transcript.as_ref()?.page(before, limit, from)?
    };
    match page {
        Page::Here(entries, more) => Some((entries, more)),
        Page::Older(page) => match tokio::task::spawn_blocking(move || page.read()).await {
            Ok(Ok(page)) => Some(page),
            Ok(Err(error)) => {
                tracing::warn!(agent = %id, %error, "could not read a conversation's older entries");
                None
            }
            Err(_) => None,
        },
    }
}

/// The entries `seqs` of `id`'s conversation there are: held, or read from
/// disk.
pub(crate) async fn entries_of(factory: &crate::factory::Factory, id: &str, seqs: &[u64]) -> HashMap<u64, Entry> {
    let mut found = HashMap::new();
    let older = {
        let inner = factory.lock();
        let Some(transcript) = inner.agents.get(id).and_then(|runtime| runtime.transcript.as_ref()) else { return found };
        let mut missing = Vec::new();
        for &seq in seqs {
            match transcript.entry(seq) {
                Some(entry) => {
                    found.insert(seq, entry.clone());
                }
                None if seq < transcript.len() => missing.push(seq),
                None => {}
            }
        }
        if missing.is_empty() {
            return found;
        }
        (transcript.older(), missing)
    };
    let (older, missing) = older;
    match tokio::task::spawn_blocking(move || older.entries(&missing)).await {
        Ok(Ok(read)) => found.extend(read),
        Ok(Err(error)) => tracing::warn!(agent = %id, %error, "could not read a conversation's older entries"),
        Err(_) => {}
    }
    found
}

/// Tests: the file of segment `number` of the conversation in `dir`, in
/// whichever form it is.
#[doc(hidden)]
pub fn segment_file_for_test(dir: &Path, number: u32) -> Option<PathBuf> {
    log::files(dir).ok()?.remove(&number)
}

/// Tests: segment `number` of the conversation in `dir`, whichever form it's
/// in, as the one JSON object a segment was before JSON Lines (`messages`,
/// `frame`, `pinned`…).
#[doc(hidden)]
pub fn segment_for_test(dir: &Path, number: u32) -> Option<serde_json::Value> {
    let segment = log::read::<serde_json::Value>(&segment_file_for_test(dir, number)?).ok()?;
    let whole = log::WholeSegment {
        compaction: segment.compaction.as_ref(),
        cleared: segment.cleared,
        messages: &segment.messages,
        pinned: &segment.pinned,
        frame: segment.frame.as_ref(),
    };
    serde_json::from_slice(&whole.bytes(false).ok()?).ok()
}

/// Goes through `id`'s entries before `before` (all of them for None),
/// newest first, until `visit` says to stop (true): those held a slice at a
/// time under the lock, then the older ones read back from disk a section at
/// a time, off it. A section that can't be read ends it, logged.
pub(crate) async fn scan_back(factory: &crate::factory::Factory, id: &str, before: Option<u64>, mut visit: impl FnMut(&Entry) -> bool) {
    /// Held entries looked at per hold of the lock.
    const SLICE: u64 = 2000;
    let mut end = before.unwrap_or(u64::MAX);
    let older = loop {
        let start = {
            let inner = factory.lock();
            let Some(transcript) = inner.agents.get(id).and_then(|runtime| runtime.transcript.as_ref()) else { return };
            let (base, entries) = transcript.window();
            end = end.min(transcript.len());
            if end <= base {
                break transcript.older();
            }
            let start = end.saturating_sub(SLICE).max(base);
            for entry in entries[(start - base) as usize..(end - base) as usize].iter().rev() {
                if visit(entry) {
                    return;
                }
            }
            start
        };
        end = start;
        tokio::task::yield_now().await;
    };
    let sections: Vec<usize> = older.before(end).collect();
    for at in sections {
        let read = older.clone();
        let entries = match tokio::task::spawn_blocking(move || read.section(at)).await {
            Ok(Ok(entries)) => entries,
            Ok(Err(error)) => {
                tracing::warn!(agent = %id, %error, "could not read a conversation's older entries");
                return;
            }
            Err(_) => return,
        };
        for entry in entries.iter().rev().filter(|entry| entry.seq < end) {
            if visit(entry) {
                return;
            }
        }
    }
}

/// Where a page (`Transcript::page`) of the entries before `before` starts:
/// `entries` are those from `offset` to `before`. None when it starts before
/// them, or needs one of the earlier entries to know.
fn page_start(entries: &[Entry], offset: u64, before: u64, limit: u32, from: Option<u64>) -> Option<u64> {
    let end = before;
    let at = |seq: u64| entries.get(usize::try_from(seq.checked_sub(offset)?).ok()?);
    if let Some(from) = from {
        let start = from.min(end);
        return (start >= offset).then_some(start);
    }
    let limit = match limit {
        0 => PAGE_ENTRIES,
        limit => (limit as usize).min(MAX_PAGE_ENTRIES),
    } as u64;
    let mut start = end;
    let mut bytes = 0usize;
    while start > 0 && end - start < limit {
        let size = prost::Message::encoded_len(at(start - 1)?);
        if start < end && bytes + size > PAGE_BYTES {
            break;
        }
        bytes += size;
        start -= 1;
    }
    // The whole model response the first entry is part of.
    if start < end
        && let Some(first) = at(start)
        && first.reply != 0
        && first.reply < start
    {
        let response = first.reply;
        let mut extra = 0;
        for seq in response..start {
            extra += prost::Message::encoded_len(at(seq)?);
        }
        if bytes + extra <= PAGE_BYTES * 2 {
            start = response;
        }
    }
    Some(start)
}

/// Whether an older segment's entry stays in memory once its entries are on
/// disk (`older`): a card, which changes as it's decided and is looked up
/// by what it shows; a call still without a result, which a later segment
/// or a restart changes; and a call Needs you shows a change by.
fn keeps(entry: &Entry) -> bool {
    match &entry.kind {
        Some(
            Kind::Proposal(_)
            | Kind::SecretRequest(_)
            | Kind::ConnectProposal(_)
            | Kind::DevicePairing(_)
            | Kind::AskPerson(_)
            | Kind::PipeStart(_)
            | Kind::AppGrant(_)
            | Kind::AppAccess(_)
            | Kind::SecretApproval(_),
        ) => true,
        Some(Kind::ToolCall(call)) => {
            matches!(call.state(), ToolCallState::Pending | ToolCallState::Running)
                || crate::apps::access::change_of(call).is_some()
                || crate::extensions::models::change_of(call).is_some()
                || crate::fork::history::change_of(call).is_some()
        }
        _ => false,
    }
}

/// When a segment's summary was written, if it starts with one.
fn summary_ms(segment: &Segment) -> Option<u64> {
    segment.compaction.as_ref()?;
    segment.messages.first().map(Message::timestamp)
}

/// Tokens the model held after producing `message`.
fn context_tokens(message: &AssistantMessage) -> u64 {
    let usage = &message.usage;
    usage.input + usage.cache_read + usage.cache_write + usage.output
}

/// Whether an entry waits for the person's answer: a request for a secret,
/// a proposal (to connect a service, or to Rivet), a device being added, a
/// pipe secret approval or a question still pending.
/// An entry's fingerprint: FNV-1a over its encoding, the same for the same
/// entry from one start to the next.
fn fingerprint(entry: &Entry) -> u64 {
    prost::Message::encode_to_vec(entry).iter().fold(0xcbf2_9ce4_8422_2325, |hash: u64, byte| (hash ^ u64::from(*byte)).wrapping_mul(0x0100_0000_01b3))
}

fn waits(kind: Option<&Kind>, owner: &str) -> bool {
    match kind {
        // An approved request waits on its admin agent, not the person, so
        // it leaves Needs you until there's something for them to do: the
        // admin's add-secret card while it collects, or the asker's card
        // again when no admin holds it (it was deleted, say).
        Some(Kind::SecretRequest(request)) => match request.state() {
            v1::SecretRequestState::Pending => true,
            v1::SecretRequestState::Approved => {
                if request.admin_agent_id.is_empty() {
                    true
                } else {
                    request.collecting && owner == request.admin_agent_id
                }
            }
            _ => false,
        },
        Some(Kind::ConnectProposal(proposal)) => proposal.state() == v1::ConnectProposalState::Pending,
        Some(Kind::Proposal(proposal)) => proposal.state() == v1::ProposalState::Pending,
        Some(Kind::SecretApproval(approval)) => approval.state() == v1::SecretApprovalState::Pending,
        Some(Kind::AskPerson(ask)) => ask.state() == v1::AskPersonState::Pending,
        Some(Kind::DevicePairing(pairing)) => {
            matches!(pairing.state(), v1::DevicePairingState::Waiting | v1::DevicePairingState::Asking)
        }
        Some(Kind::PipeStart(start)) => start.state() == v1::PipeStartState::Waiting,
        Some(Kind::AppGrant(proposal)) => proposal.state() == v1::AppGrantState::Pending,
        Some(Kind::AppAccess(proposal)) => proposal.state() == v1::AppGrantState::Pending,
        _ => false,
    }
}

/// Finds the files of the images stored as files (`images`) of the segment
/// at `segment`.
pub(crate) fn load_images(messages: &mut [Message], segment: &Path) {
    images::load(messages, segment);
}

/// Whether two messages are the same but for their images' data, which an
/// older segment is read without.
fn same_but_images(a: &Message, b: &Message) -> bool {
    fn blank(message: &Message) -> Message {
        let mut message = message.clone();
        match &mut message {
            Message::User(UserMessage { content: UserContent::Parts(parts), .. }) => {
                for part in parts {
                    if let UserPart::Image { data, .. } = part {
                        data.clear();
                    }
                }
            }
            Message::ToolResult(result) => {
                for part in &mut result.content {
                    if let ToolResultPart::Image { data, .. } = part {
                        data.clear();
                    }
                }
            }
            _ => {}
        }
        message
    }
    a == b || blank(a) == blank(b)
}

fn notice(text: String, level: NoticeLevel) -> Kind {
    Kind::Notice(v1::Notice { text, level: level.into(), ..Default::default() })
}

/// A voice message sent with typed text (#425): its two parts apart, so
/// the app shows what was typed, then the recording with the words heard.
fn voice_parts(message: &UserMessage, text: &str) -> v1::UserText {
    let Some(voice) = message.voice.as_ref() else { return v1::UserText::default() };
    match crate::speech::voice::heard_part(text, &voice.typed) {
        Some(heard) => v1::UserText { typed: voice.typed.clone(), heard: heard.to_owned(), ..Default::default() },
        None => v1::UserText::default(),
    }
}

/// The person's message, or an event shown as one, never as theirs.
fn user_kind(message: &UserMessage) -> Kind {
    let text = user_text(&message.content);
    let Some(event) = &message.event else {
        // Attached images are shown as attachments, not as "[image]".
        let text = if message.attachments.is_empty() { text } else { text_parts(&message.content) };
        return Kind::User(v1::UserText {
            text: text.clone(),
            sent_from: message.sent_from.as_ref().map(sent_from),
            client_id: message.client_id.clone(),
            attachments: message.attachments.iter().map(attachment).collect(),
            voice: message.voice.as_ref().map(|voice| v1::Transcription {
                state: v1::TranscriptionState::Done.into(),
                duration_ms: voice.duration_ms,
                language: voice.language.clone(),
                ..Default::default()
            }),
            checklist_item: message.checklist_item.as_ref().map(checklist_item),
            ..voice_parts(message, &text)
        });
    };
    let source = match event.source {
        EventSource::Host => v1::EventSource::Factory,
        EventSource::Agent => v1::EventSource::Agent,
        EventSource::Task => v1::EventSource::Task,
        EventSource::Monitor => v1::EventSource::Monitor,
        EventSource::App => v1::EventSource::App,
        EventSource::Pipe => v1::EventSource::Pipe,
    };
    Kind::Event(v1::Event {
        source: source.into(),
        from: event.from.clone(),
        from_id: event.id.clone(),
        summary: event.summary.clone(),
        text,
        pipe_seqs: event.pipe_seqs.clone(),
        message_id: event.provenance.as_ref().map_or(0, |provenance| provenance.message),
        reply_to_ids: event.provenance.as_ref().map(|provenance| provenance.reply_to.clone()).unwrap_or_default(),
        intent: event.provenance.as_ref().map(|provenance| provenance.intent.clone()).unwrap_or_default(),
        quiet: event.provenance.as_ref().is_some_and(|provenance| provenance.quiet),
    })
}

/// A file attached to the person's message, for clients.
/// A checklist item a message of the person's replies on, as clients see it.
pub(crate) fn checklist_item(item: &factory_agent::ChecklistItemRef) -> v1::ChecklistItemRef {
    v1::ChecklistItemRef { agent_id: item.agent.clone(), id: item.id, text: item.text.clone() }
}

pub(crate) fn attachment(attached: &factory_agent::Attachment) -> v1::Attachment {
    v1::Attachment { path: attached.path.clone(), name: attached.name.clone(), size: attached.size, mime: attached.mime.clone() }
}

/// The message's text alone, without its images.
fn text_parts(content: &UserContent) -> String {
    match content {
        UserContent::Text(text) => text.clone(),
        UserContent::Parts(parts) => {
            let texts: Vec<&str> = parts
                .iter()
                .filter_map(|part| match part {
                    UserPart::Text { text } => Some(text.as_str()),
                    UserPart::Image { .. } => None,
                })
                .collect();
            texts.join("\n")
        }
    }
}

/// Where the person sent a message from, for clients.
pub(crate) fn sent_from(from: &factory_agent::SentFrom) -> v1::SentFrom {
    let kind = match from.kind.as_str() {
        "desktop" => v1::DeviceKind::Desktop,
        "laptop" => v1::DeviceKind::Laptop,
        "phone" => v1::DeviceKind::Phone,
        "tablet" => v1::DeviceKind::Tablet,
        _ => v1::DeviceKind::Unspecified,
    };
    v1::SentFrom { device_name: from.device.clone(), kind: kind.into() }
}

/// The device a message comes from, as the model reads it: `kind` as the
/// app said it, none when it didn't.
pub(crate) fn device(name: &str, kind: v1::DeviceKind) -> factory_agent::SentFrom {
    let kind = match kind {
        v1::DeviceKind::Desktop => "desktop",
        v1::DeviceKind::Laptop => "laptop",
        v1::DeviceKind::Phone => "phone",
        v1::DeviceKind::Tablet => "tablet",
        v1::DeviceKind::Unspecified => "",
    };
    factory_agent::SentFrom { device: name.to_owned(), kind: kind.to_owned() }
}

fn user_text(content: &UserContent) -> String {
    match content {
        UserContent::Text(text) => text.clone(),
        UserContent::Parts(parts) => {
            let mut text = String::new();
            for part in parts {
                if !text.is_empty() {
                    text.push('\n');
                }
                match part {
                    UserPart::Text { text: part } => text.push_str(part),
                    UserPart::Image { .. } => text.push_str("[image]"),
                }
            }
            text
        }
    }
}

/// A tool call's output and details (`v1::ToolCall`'s).
struct ToolOutput {
    output: String,
    details: String,
}

/// Owned text fields for a stored frame or tool-result card; non-strings are empty.
pub(crate) fn text_fields(value: &serde_json::Value) -> impl Fn(&str) -> String + '_ {
    move |key| value.get(key).and_then(serde_json::Value::as_str).unwrap_or_default().to_owned()
}

/// A result's details for the app: empty for none (null or `{}`).
fn details_json(details: &serde_json::Value) -> String {
    match details {
        serde_json::Value::Null => String::new(),
        serde_json::Value::Object(map) if map.is_empty() => String::new(),
        details => details.to_string(),
    }
}

fn result_text(result: &ToolResult) -> String {
    parts_text(&result.content)
}

fn parts_text(parts: &[ToolResultPart]) -> String {
    let mut text = String::new();
    for part in parts {
        if !text.is_empty() {
            text.push('\n');
        }
        match part {
            ToolResultPart::Text { text: part } => text.push_str(part),
            ToolResultPart::Image { .. } => text.push_str("[image]"),
        }
    }
    text
}

/// When a tool call began and ended, to set on its entry; None leaves it.
#[derive(Default)]
struct Times {
    started: Option<i64>,
    ended: Option<i64>,
}

/// How long a call may run before it's stopped, in ms: bash's timeout; 0 for
/// a background command and for other tools.
fn timeout_ms(name: &str, arguments: &serde_json::Value) -> u64 {
    if name != "bash" {
        return 0;
    }
    factory_agent::tools::bash::call_timeout(arguments).map_or(0, |timeout| u64::try_from(timeout.as_millis()).unwrap_or(u64::MAX))
}

fn pretty(value: &serde_json::Value) -> String {
    serde_json::to_string_pretty(value).unwrap_or_default()
}

/// A millisecond timestamp in this computer's time zone; out-of-range timestamps are empty.
pub(crate) fn local_time(ms: i64, format: &str) -> String {
    jiff::Timestamp::from_millisecond(ms)
        .map(|time| time.to_zoned_datetime(jiff::tz::TimeZone::system()).strftime(format).to_string())
        .unwrap_or_default()
}

pub fn now_ms() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |elapsed| elapsed.as_millis() as i64)
}
