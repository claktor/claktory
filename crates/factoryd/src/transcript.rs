//! An agent's conversation: the messages the model sees (its context) and the
//! entries the app shows.
//!
//! Each context window is one JSON file, `segment-NNNN.json`. Compaction starts
//! a new segment whose first message is the summary, so older segments stay as
//! read-only history and only the current one is rewritten.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::ffi::OsStr;
use std::path::PathBuf;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use factory_agent::{
    AgentEvent, AssistantMessage, AssistantPart, EventSource, Message, StopReason, StreamEvent, ToolResult, ToolResultPart, UserContent,
    UserMessage, UserPart,
};
use factory_protocol::v1::{self, Entry, NoticeLevel, ToolCallState, entry::Kind};
use serde::{Deserialize, Serialize};

/// Live tool output reaches clients at most this often.
const TOOL_OUTPUT_INTERVAL: Duration = Duration::from_millis(100);

#[derive(Clone, Debug, Serialize, Deserialize)]
struct CompactionInfo {
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

#[derive(Deserialize)]
struct Segment {
    #[serde(default)]
    compaction: Option<CompactionInfo>,
    messages: Vec<Message>,
    #[serde(default)]
    pinned: BTreeMap<String, String>,
    /// When the person cleared the conversation, starting this segment.
    #[serde(default)]
    cleared: Option<i64>,
    #[serde(default)]
    frame: Option<Frame>,
}

#[derive(Serialize)]
struct SegmentRef<'a> {
    #[serde(skip_serializing_if = "Option::is_none")]
    compaction: Option<&'a CompactionInfo>,
    messages: &'a [Message],
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pinned: &'a BTreeMap<String, String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    cleared: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    frame: Option<&'a Frame>,
}

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

/// A file write the caller must perform, outside any lock.
pub struct Persist {
    pub path: PathBuf,
    pub bytes: Vec<u8>,
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
    /// An entry's seq is its index.
    entries: Vec<Entry>,
    segment: u32,
    compaction: Option<CompactionInfo>,
    messages: Vec<Message>,
    /// Prompt sections fixed for this segment, by extension id (see
    /// `PromptKind::SegmentPinned`).
    pinned: BTreeMap<String, String>,
    /// When the person cleared the conversation, if that started this segment.
    cleared: Option<i64>,
    frame: Option<Frame>,
    context_tokens: u64,
    /// The entries waiting for the person's answer (see `waits`).
    waiting: BTreeSet<u64>,
    live: Live,
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

    fn read(dir: PathBuf, owner: String) -> Result<Transcript, TranscriptError> {
        let mut transcript = Transcript {
            dir,
            owner,
            entries: Vec::new(),
            segment: 1,
            compaction: None,
            messages: Vec::new(),
            pinned: BTreeMap::new(),
            cleared: None,
            frame: None,
            context_tokens: 0,
            waiting: BTreeSet::new(),
            live: Live::default(),
        };
        let mut numbers = Vec::new();
        match std::fs::read_dir(&transcript.dir) {
            Ok(files) => {
                for file in files {
                    if let Some(number) = segment_number(&file?.file_name()) {
                        numbers.push(number);
                    }
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(transcript),
            Err(error) => return Err(error.into()),
        }
        numbers.sort_unstable();
        for number in numbers {
            let path = transcript.segment_path(number);
            let bytes = std::fs::read(&path)?;
            let segment = serde_json::from_slice(&bytes).map_err(|source| TranscriptError::Corrupt { path, source })?;
            transcript.restore(number, segment);
        }
        // Nothing is running yet, so a call still without a result was cut off
        // by a restart. The model is told the same ("No result provided").
        for entry in &mut transcript.entries {
            if let Some(Kind::ToolCall(call)) = &mut entry.kind
                && matches!(call.state(), ToolCallState::Pending | ToolCallState::Running)
            {
                call.state = ToolCallState::Failed.into();
                call.output = "Interrupted: Claktory restarted before this finished.".to_owned();
            }
        }
        Ok(transcript)
    }

    /// A new, empty conversation the person started with /clear, and its
    /// first file. The old conversation's files must be out of `dir`.
    pub fn cleared(dir: PathBuf, owner: String) -> Result<(Transcript, Persist), serde_json::Error> {
        let now = now_ms();
        let mut transcript = Transcript {
            dir,
            owner,
            entries: Vec::new(),
            segment: 1,
            compaction: None,
            messages: Vec::new(),
            pinned: BTreeMap::new(),
            cleared: Some(now),
            frame: None,
            context_tokens: 0,
            waiting: BTreeSet::new(),
            live: Live::default(),
        };
        transcript.push(now, notice(CLEARED.to_owned(), NoticeLevel::Info));
        let persist = transcript.persist()?;
        Ok((transcript, persist))
    }

    /// The latest entries, as a snapshot sends them, and whether there are
    /// earlier ones.
    pub fn recent(&self) -> (Vec<Entry>, bool) {
        self.page(self.entries.len() as u64, 0, None).unwrap_or_default()
    }

    /// Entries before `before` (a seq, exclusive): from `from` on when set,
    /// else the latest, at most `limit` (0 for `PAGE_ENTRIES`) and about
    /// `PAGE_BYTES`, and never starting partway through a model response
    /// unless that response alone is bigger. Whether there are earlier ones.
    /// None when `before` is beyond the end.
    pub fn page(&self, before: u64, limit: u32, from: Option<u64>) -> Option<(Vec<Entry>, bool)> {
        let end = usize::try_from(before).ok().filter(|end| *end <= self.entries.len())?;
        let start = match from {
            Some(from) => usize::try_from(from).unwrap_or(usize::MAX).min(end),
            None => {
                let limit = match limit {
                    0 => PAGE_ENTRIES,
                    limit => (limit as usize).min(MAX_PAGE_ENTRIES),
                };
                let mut start = end;
                let mut bytes = 0usize;
                while start > 0 && end - start < limit {
                    let size = prost::Message::encoded_len(&self.entries[start - 1]);
                    if start < end && bytes + size > PAGE_BYTES {
                        break;
                    }
                    bytes += size;
                    start -= 1;
                }
                // The whole model response the first entry is part of.
                if let Some(first) = self.entries.get(start)
                    && first.reply != 0
                    && (first.reply as usize) < start
                {
                    let response = first.reply as usize;
                    let extra: usize = self.entries[response..start].iter().map(prost::Message::encoded_len).sum();
                    if bytes + extra <= PAGE_BYTES * 2 {
                        start = response;
                    }
                }
                start
            }
        };
        Some((self.entries[start..end].to_vec(), start > 0))
    }

    pub fn entries(&self) -> &[Entry] {
        &self.entries
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
    }

    pub fn context_tokens(&self) -> u64 {
        self.context_tokens
    }

    /// The seqs of the entries waiting for the person's answer, oldest
    /// first: requests for secrets and proposals still pending.
    pub fn waiting_seqs(&self) -> Vec<u64> {
        self.waiting.iter().copied().collect()
    }

    /// Notes whether `entry` waits for the person now.
    fn track(&mut self, entry: &Entry) {
        if waits(entry.kind.as_ref(), &self.owner) {
            self.waiting.insert(entry.seq);
        } else {
            self.waiting.remove(&entry.seq);
        }
    }

    /// Replaces the latest shown item of the same kind and id, keeping its entry metadata.
    fn replace_shown(&mut self, kind: Kind) -> Option<Entry> {
        let entry = self.entries.iter_mut().rev().find(|entry| match (&entry.kind, &kind) {
            (Some(Kind::Proposal(shown)), Kind::Proposal(updated)) => shown.id == updated.id,
            (Some(Kind::SecretRequest(shown)), Kind::SecretRequest(updated)) => shown.id == updated.id,
            (Some(Kind::ConnectProposal(shown)), Kind::ConnectProposal(updated)) => shown.id == updated.id,
            (Some(Kind::DevicePairing(shown)), Kind::DevicePairing(updated)) => shown.id == updated.id,
            (Some(Kind::AskPerson(shown)), Kind::AskPerson(updated)) => shown.id == updated.id,
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
    /// entry, or None if the conversation doesn't show it.
    pub fn set_device_pairing(&mut self, pairing: v1::DevicePairing) -> Option<Entry> {
        self.replace_shown(Kind::DevicePairing(pairing))
    }

    /// Shows a pipe secret approval as decided; None if it isn't shown.
    pub fn set_secret_approval(&mut self, id: &str, state: v1::SecretApprovalState, decided_ms: i64) -> Option<Entry> {
        let entry = self.entries.iter_mut().find(|entry| matches!(&entry.kind, Some(Kind::SecretApproval(shown)) if shown.id == id))?;
        if let Some(Kind::SecretApproval(approval)) = &mut entry.kind {
            approval.state = state.into();
            approval.decided_ms = decided_ms;
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

    /// Adds a harness notice. Notices are for display only and aren't saved.
    pub fn notice(&mut self, text: impl Into<String>, level: NoticeLevel) -> Entry {
        self.push(now_ms(), notice(text.into(), level))
    }

    /// Shows a message that never reached the model (e.g. the run couldn't
    /// start). Display only.
    pub fn unsent(&mut self, message: &UserMessage) -> Entry {
        self.push(now_ms(), user_kind(message))
    }

    pub fn apply(&mut self, event: AgentEvent) -> Result<Applied, serde_json::Error> {
        let mut applied = Applied::default();
        match event {
            AgentEvent::MessageStart { message: Message::Assistant(_) } => {
                self.live.parts.clear();
                self.live.reply = None;
            }
            AgentEvent::MessageUpdate { assistant_message_event, .. } => self.stream(*assistant_message_event, &mut applied.changes),
            AgentEvent::MessageEnd { message } => {
                let waiting = self.waiting.len();
                for entry in self.show(&message) {
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
                applied.persist = Some(self.persist()?);
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
                self.live.parts.clear();
                self.live.reply = None;
                let seconds = delay_ms.div_ceil(1000);
                let text = format!("The model request failed. Retrying in {seconds}s (attempt {attempt}).");
                applied.changes.push(Change::Entry(self.push(now_ms(), notice(text, NoticeLevel::Info))));
            }
            // The response failed for want of an account with capacity: the
            // run waits (`crate::waits` says so) and asks again.
            AgentEvent::Waiting { .. } => {
                self.live.parts.clear();
                self.live.reply = None;
            }
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
                applied.persist = Some(self.persist()?);
            }
            _ => {}
        }
        Ok(applied)
    }

    fn restore(&mut self, number: u32, segment: Segment) {
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
            let mut carried = kept.len().min(self.messages.len());
            while carried > 0 && !self.messages.ends_with(&kept[..carried]) {
                carried -= 1;
            }
            messages = &kept[carried..];
        }
        self.context_tokens = 0;
        for message in messages {
            self.show(message);
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
    }

    /// Shows a complete message, reusing the entries its stream already created.
    fn show(&mut self, message: &Message) -> Vec<Entry> {
        match message {
            Message::User(user) => vec![self.push(user.timestamp as i64, user_kind(user))],
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
                    let entry = &mut self.entries[seq as usize];
                    if entry.kind.as_ref() != Some(&kind) {
                        entry.kind = Some(kind);
                        changed.push(entry.clone());
                    }
                    seq
                }
                None => {
                    let entry = self.push_part(timestamp, kind);
                    let seq = entry.seq;
                    changed.push(entry);
                    seq
                }
            };
            if let AssistantPart::ToolCall { id, .. } = part {
                self.live.tools.insert(id.clone(), seq);
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
                changes.push(Change::Entry(self.start_part(content_index, partial.timestamp as i64, kind)));
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
                match &mut self.entries[seq as usize].kind {
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
                let entry = &mut self.entries[seq as usize];
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
                let entry = &mut self.entries[seq as usize];
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
        let entry = &mut self.entries[seq as usize];
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
        let entry = Entry { seq: self.entries.len() as u64, timestamp_ms, kind: Some(kind), reply: 0 };
        self.entries.push(entry.clone());
        self.track(&entry);
        entry
    }

    /// Adds an entry of the model response being shown.
    fn push_part(&mut self, timestamp_ms: i64, kind: Kind) -> Entry {
        let seq = self.entries.len() as u64;
        let reply = *self.live.reply.get_or_insert(seq);
        let entry = Entry { seq, timestamp_ms, kind: Some(kind), reply };
        self.entries.push(entry.clone());
        entry
    }

    fn persist(&self) -> Result<Persist, serde_json::Error> {
        let segment = SegmentRef {
            compaction: self.compaction.as_ref(),
            messages: &self.messages,
            pinned: &self.pinned,
            cleared: self.cleared,
            frame: self.frame.as_ref(),
        };
        Ok(Persist { path: self.segment_path(self.segment), bytes: serde_json::to_vec(&segment)? })
    }

    fn segment_path(&self, number: u32) -> PathBuf {
        self.dir.join(format!("segment-{number:04}.json"))
    }
}

fn segment_number(name: &OsStr) -> Option<u32> {
    name.to_str()?.strip_prefix("segment-")?.strip_suffix(".json")?.parse().ok()
}

/// Tokens the model held after producing `message`.
fn context_tokens(message: &AssistantMessage) -> u64 {
    let usage = &message.usage;
    usage.input + usage.cache_read + usage.cache_write + usage.output
}

/// Whether an entry waits for the person's answer: a request for a secret,
/// a proposal (to connect a service, or to Rivet), a device being added, a
/// pipe secret approval or a question still pending.
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
        _ => false,
    }
}

fn notice(text: String, level: NoticeLevel) -> Kind {
    Kind::Notice(v1::Notice { text, level: level.into(), ..Default::default() })
}

/// The person's message, or an event shown as one, never as theirs.
fn user_kind(message: &UserMessage) -> Kind {
    let text = user_text(&message.content);
    let Some(event) = &message.event else {
        // Attached images are shown as attachments, not as "[image]".
        let text = if message.attachments.is_empty() { text } else { text_parts(&message.content) };
        return Kind::User(v1::UserText {
            text,
            sent_from: message.sent_from.as_ref().map(sent_from),
            client_id: message.client_id.clone(),
            attachments: message.attachments.iter().map(attachment).collect(),
            voice: message.voice.as_ref().map(|voice| v1::Transcription {
                state: v1::TranscriptionState::Done.into(),
                duration_ms: voice.duration_ms,
                language: voice.language.clone(),
                ..Default::default()
            }),
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
    })
}

/// A file attached to the person's message, for clients.
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

pub fn now_ms() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |elapsed| elapsed.as_millis() as i64)
}
