//! Going back in a conversation (friend bug #9; Research's
//! conversation-fork.md). The person, or an agent with the Conversations
//! extension, takes an agent's conversation back to an earlier message:
//! what came before stays byte for byte, so the provider's cache of it
//! still holds, and what came after is set aside as another version, which
//! can be switched back to.
//!
//! A conversation goes back only to right before a message the agent was
//! given (the person's or an event), never between a tool call and its
//! result (`Transcript::fork_point`). Like /clear, it stops the agent, but
//! what's queued for it stays. Each version set aside is a folder,
//! `branches/<id>/` in the agent's own: its segment files, hard-linked (a
//! conversation's files are only ever renamed over, never written in
//! place, so a link keeps what it was), and `branch.json`. Switching sets
//! the conversation as it is aside the same way, from where the two part,
//! and puts the chosen one's files back. /clear archives the versions with
//! the conversation.
//!
//! Cards from the part set aside close (its questions, secret requests and
//! proposals); everything else it did stands (files, messages, tasks,
//! monitors, the checklist), and the agent is told what in a quiet note
//! from Claktory (`note`), which it reads with whatever wakes it next.
//! Changed again before it read that note, the note is replaced by one about
//! the conversation it remembers (the first version set aside since it
//! last ran) and the one it's in now (`forgotten`).

pub(crate) mod history;
mod note;

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

use factory_agent::{AgentEvent, AssistantPart, Message, ToolResultMessage, ToolResultPart, UserContent, UserMessage, UserPart};
use factory_protocol::v1;
use serde::{Deserialize, Serialize};

use crate::factory::{Factory, Queued, RequestError, transcript_of};
use crate::transcript::log;
use crate::transcript::now_ms;

/// The folder in an agent's own where its other versions are kept.
pub(crate) const BRANCHES: &str = "branches";
const BRANCH_FILE: &str = "branch.json";
/// The most a version's label says of the person's message it starts with.
const LABEL_CHARS: usize = 60;

/// Where a version parts from the conversation: right before the message at
/// `index` of segment `segment`, given at `ms`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Point {
    pub segment: u32,
    pub index: u32,
    pub ms: i64,
}

/// Another version of an agent's conversation, as its `branch.json` keeps it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Branch {
    /// Its folder's name.
    pub id: String,
    /// The first words of the first message the person wrote in it, or
    /// "From 14:02".
    pub label: String,
    pub set_aside_ms: i64,
    /// Where it parts from the conversation it was set aside from.
    pub fork: Point,
    /// How many messages it has past that point (the agent's replies and
    /// what it was given; not tools' results).
    #[serde(default)]
    pub messages: u32,
    /// Who set it aside: an agent's id; empty for the person.
    #[serde(default)]
    pub made_by: String,
    /// The note the agent was given when this was set aside (its message's
    /// id): switching back to this version before the agent read it would
    /// make it untrue, so it's dropped then.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<i64>,
}

/// Who takes a conversation back, or switches it: an agent's id, empty for
/// the person, and why, in the agent's words (empty for the person).
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct By<'a> {
    pub agent: &'a str,
    pub reason: &'a str,
}

/// Takes `id`'s conversation back to right before the entry `seq` (which must
/// still have `timestamp_ms`): see `Transcript::fork_point`. Whether
/// anything was set aside (always, once it's taken back).
pub(crate) async fn fork(factory: &Arc<Factory>, id: &str, seq: u64, timestamp_ms: i64, by: By<'_>) -> Result<bool, RequestError> {
    factory.known_agent(id)?;
    factory.ensure_loaded(id).await?;
    let writes = factory.lock().agents.entry(id.to_owned()).or_default().writes.clone();
    let _writing = writes.lock().await;
    // Refused before anything stops. The entry may be read back from disk.
    let entry = crate::transcript::entries_of(factory, id, &[seq]).await.remove(&seq);
    let point = {
        let inner = factory.lock();
        let transcript = inner.agents.get(id).and_then(|runtime| runtime.transcript.as_ref());
        let point = transcript.ok_or_else(|| RequestError::Fork("The conversation changed. Try again.".to_owned()))?;
        point.fork_point(seq, entry.as_ref(), timestamp_ms).map_err(|why| RequestError::Fork(why.to_owned()))?
    };
    let dir = factory.home.agent_dir(id);
    let point = Point { segment: point.segment, index: point.index, ms: point.at_ms };
    let made_by = by.agent.to_owned();
    held(factory, id, async {
        halt(factory, id).await?;
        let unread = unread(factory, id);
        let known = unread.first().cloned();
        let (set_aside, told) = blocking(move || {
            let set_aside = set_aside(&dir, point, &made_by, now_ms())?;
            cut(&dir, point)?;
            let told = forgotten(&dir, known.as_ref().or(set_aside.as_ref()))?;
            Ok((set_aside, told))
        })
        .await?;
        settle(factory, id, set_aside, told, &unread, by, note::Change::Back).await
    })
    .await
}

/// Makes `branch` `id`'s conversation; the conversation as it is now is set
/// aside as another version, from where the two part. Whether anything was
/// set aside: nothing is when the conversation ends where the two part.
pub(crate) async fn switch(factory: &Arc<Factory>, id: &str, branch: &str, by: By<'_>) -> Result<bool, RequestError> {
    factory.known_agent(id)?;
    let dir = factory.home.agent_dir(id);
    let chosen = branch_dir(&dir, branch)?;
    read_branch(&chosen).ok_or_else(|| RequestError::NoBranch(branch.to_owned()))?;
    factory.ensure_loaded(id).await?;
    let writes = factory.lock().agents.entry(id.to_owned()).or_default().writes.clone();
    let _writing = writes.lock().await;
    let made_by = by.agent.to_owned();
    held(factory, id, async {
        halt(factory, id).await?;
        let unread = unread(factory, id);
        let known = unread.first().cloned();
        let (set_aside, told) = blocking(move || {
            let point = parting(&dir, &chosen)?;
            let set_aside = set_aside(&dir, point, &made_by, now_ms())?;
            bring_back(&dir, &chosen)?;
            // Switched back to what it remembers, it's told nothing: that
            // version's folder is gone.
            let told = forgotten(&dir, known.as_ref().or(set_aside.as_ref()))?;
            Ok((set_aside, told))
        })
        .await?;
        settle(factory, id, set_aside, told, &unread, by, note::Change::Switched).await
    })
    .await
}

/// Runs `work` on `id`'s conversation with its queue held: no run takes
/// what's queued until it's done (`Runtime::forking`), not even the stopped
/// run winding down, which would hand it to the conversation as it was.
/// Then what waited goes to the agent, the note first.
async fn held<T>(
    factory: &Arc<Factory>,
    id: &str,
    work: impl std::future::Future<Output = Result<T, RequestError>>,
) -> Result<T, RequestError> {
    factory.lock().agents.entry(id.to_owned()).or_default().forking = true;
    let done = work.await;
    let mut inner = factory.lock();
    if let Some(runtime) = inner.agents.get_mut(id) {
        runtime.forking = false;
    }
    factory.wake(&mut inner, id);
    done
}

/// Deletes the version `branch` of `id`'s conversation.
pub(crate) async fn delete(factory: &Factory, id: &str, branch: &str) -> Result<(), RequestError> {
    factory.known_agent(id)?;
    let folder = branch_dir(&factory.home.agent_dir(id), branch)?;
    if read_branch(&folder).is_none() {
        return Err(RequestError::NoBranch(branch.to_owned()));
    }
    tokio::fs::remove_dir_all(&folder).await?;
    factory.broadcast_workspace(&factory.lock());
    Ok(())
}

/// ListBranches.
pub(crate) fn list(factory: &Factory, id: &str) -> Result<v1::Branches, RequestError> {
    factory.known_agent(id)?;
    let branches = branches(factory, id)
        .into_iter()
        .map(|branch| v1::Branch {
            id: branch.id,
            label: branch.label,
            set_aside_ms: branch.set_aside_ms,
            forked_at_ms: branch.fork.ms,
            messages: branch.messages,
            made_by: branch.made_by,
        })
        .collect();
    Ok(v1::Branches { agent_id: id.to_owned(), branches })
}

/// How many other versions the conversation in `dir` has (Agent.branches),
/// counted again only when its `branches/` folder changed: the Workspace is
/// built often, under the lock.
pub(crate) fn count(dir: &Path) -> u32 {
    static COUNTS: Mutex<Option<HashMap<PathBuf, (SystemTime, u32)>>> = Mutex::new(None);
    let folder = dir.join(BRANCHES);
    let Ok(changed) = std::fs::metadata(&folder).and_then(|meta| meta.modified()) else { return 0 };
    let mut counts = COUNTS.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let counts = counts.get_or_insert_default();
    if let Some((when, count)) = counts.get(&folder)
        && *when == changed
    {
        return *count;
    }
    let count = std::fs::read_dir(&folder)
        .map(|entries| entries.flatten().filter(|entry| made(entry) && entry.path().join(BRANCH_FILE).is_file()).count())
        .unwrap_or(0);
    let count = u32::try_from(count).unwrap_or(u32::MAX);
    counts.insert(folder, (changed, count));
    count
}

/// The other versions of `id`'s conversation, the latest set aside first.
pub(crate) fn branches(factory: &Factory, id: &str) -> Vec<Branch> {
    let folder = factory.home.agent_dir(id).join(BRANCHES);
    let Ok(entries) = std::fs::read_dir(folder) else { return Vec::new() };
    let mut branches: Vec<Branch> = entries.flatten().filter(made).filter_map(|entry| read_branch(&entry.path())).collect();
    branches.sort_by(|a, b| b.set_aside_ms.cmp(&a.set_aside_ms).then_with(|| b.id.cmp(&a.id)));
    branches
}

/// The entry of `branches/` is a version, not one being set aside.
fn made(entry: &std::fs::DirEntry) -> bool {
    !entry.file_name().to_string_lossy().starts_with('.')
}

/// Stops the agent for a fork or a switch, as Stop does, but keeps what's
/// queued: what its run was handed that its conversation doesn't hold yet
/// goes back to the front of the queue. The conversation in memory is
/// written first, so its files hold all of it. The caller holds `writes`.
async fn halt(factory: &Factory, id: &str) -> Result<(), RequestError> {
    let persist = {
        let mut inner = factory.lock();
        let runtime = inner.agents.entry(id.to_owned()).or_default();
        // Its tool calls still running stop here: their results say so, in
        // the part set aside, rather than that Claktory restarted.
        if runtime.running
            && let Some(transcript) = runtime.transcript.as_mut()
        {
            stop_calls(transcript)?;
        }
        let persist = runtime.transcript.as_mut().map(|transcript| transcript.persist());
        let saved = runtime.transcript.as_ref().map_or(0, |transcript| saved(transcript.messages(), &runtime.taken));
        let unsaved: Vec<Queued> = runtime.taken.drain(saved..).map(|(queued, _)| queued).collect();
        runtime.queue.splice(0..0, unsaved);
        // What it was handed and holds reached it.
        factory.stop_run(runtime);
        // What the stopped run still reports belongs to the conversation before.
        runtime.epoch += 1;
        runtime.compact = None;
        runtime.doing = None;
        factory.reset_presence(id);
        persist
    };
    if let Some(persist) = persist {
        persist.write().await?;
    }
    Ok(())
}

/// What a tool call cut off by going back, or by a switch, says.
const STOPPED: &str = "Stopped: the conversation was taken back, or switched to another version, before this finished.";

/// Gives each tool call of the agent's last reply still without a result
/// one saying it stopped, as stopping the agent does.
fn stop_calls(transcript: &mut crate::transcript::Transcript) -> Result<(), serde_json::Error> {
    let messages = transcript.messages();
    let Some(at) = messages.iter().rposition(|message| matches!(message, Message::Assistant(_))) else { return Ok(()) };
    let Message::Assistant(reply) = &messages[at] else { return Ok(()) };
    let answered: Vec<&str> = messages[at + 1..]
        .iter()
        .filter_map(|message| match message {
            Message::ToolResult(result) => Some(result.tool_call_id.as_str()),
            _ => None,
        })
        .collect();
    let open: Vec<(String, String)> = reply
        .content
        .iter()
        .filter_map(|part| match part {
            AssistantPart::ToolCall { id, name, .. } if !answered.contains(&id.as_str()) => Some((id.clone(), name.clone())),
            _ => None,
        })
        .collect();
    let now = u64::try_from(now_ms()).unwrap_or_default();
    for (id, name) in open {
        let result = ToolResultMessage {
            tool_call_id: id,
            tool_name: name,
            content: vec![ToolResultPart::Text { text: STOPPED.to_owned() }],
            details: serde_json::Value::Null,
            usage: None,
            is_error: true,
            timestamp: now,
            started: None,
        };
        transcript.apply(AgentEvent::MessageEnd { message: Message::ToolResult(result) })?;
    }
    Ok(())
}

/// How many of the messages handed to a run (`taken`, oldest first) its
/// conversation holds: they're the last of its prompts, in order.
fn saved(messages: &[Message], taken: &std::collections::VecDeque<(Queued, Option<i64>)>) -> usize {
    let prompts: Vec<&UserMessage> = messages
        .iter()
        .filter_map(|message| match message {
            Message::User(user) => Some(user),
            _ => None,
        })
        .collect();
    (0..=taken.len().min(prompts.len()))
        .rev()
        .find(|&count| taken.iter().take(count).zip(&prompts[prompts.len() - count..]).all(|((queued, _), user)| same(queued, user)))
        .unwrap_or(0)
}

/// `user` is the message `queued` became.
fn same(queued: &Queued, user: &UserMessage) -> bool {
    queued.event.is_some() == user.event.is_some()
        && match &user.content {
            UserContent::Text(text) => *text == queued.text,
            UserContent::Parts(parts) => parts.iter().any(|part| matches!(part, UserPart::Text { text } if *text == queued.text)),
        }
}

/// After the files changed: cards from the part set aside close, the
/// conversation is read again and shown on every device, and the agent
/// gets its note, about what it was `told` it forgot, in place of the notes
/// of `unread` versions it hadn't read yet. Whether anything was set aside.
async fn settle(
    factory: &Arc<Factory>,
    id: &str,
    set_aside: Option<Branch>,
    told: Option<Forgotten>,
    unread: &[Branch],
    by: By<'_>,
    change: note::Change,
) -> Result<bool, RequestError> {
    let kept = set_aside.is_some();
    let closed = match &set_aside {
        Some(branch) => close_cards(factory, id, branch.fork.ms),
        None => 0,
    };
    factory.lock().agents.entry(id.to_owned()).or_default().transcript = None;
    factory.ensure_loaded(id).await?;
    let branch = told.as_ref().map(|told| told.version.id.clone());
    let note = told.map(|told| note::write(factory, id, &told.version, &told.after, closed, by, change, told.change));
    let mut inner = factory.lock();
    // The notes it hadn't read say how the conversation changed from what it
    // remembers to what it was then: this one says it to what it is now.
    if let Some(runtime) = inner.agents.get_mut(id) {
        let stale: Vec<i64> = unread.iter().filter_map(|branch| branch.note).collect();
        let (dropped, queue): (Vec<Queued>, _) = std::mem::take(&mut runtime.queue).into_iter().partition(|queued| stale.contains(&queued.id));
        runtime.queue = queue;
        if !dropped.is_empty() {
            factory.settle(&dropped, crate::store::Outcome::Dropped);
        }
    }
    let mut noted = None;
    if let Some((text, summary)) = note {
        let mut event = crate::factory::host_event(summary);
        event.provenance = Some(factory_agent::Provenance {
            message: 0,
            reply_to: Vec::new(),
            intent: String::new(),
            quiet: true,
            quiet_note: String::new(),
        });
        match factory.enqueue(&mut inner, id, Queued::event(text, event)) {
            // First, before what waited: it says what changed.
            Ok(()) => {
                if let Some(runtime) = inner.agents.get_mut(id)
                    && let Some(note) = runtime.queue.pop()
                {
                    noted = Some(note.id);
                    runtime.queue.insert(0, note);
                }
            }
            Err(error) => tracing::error!(agent = %id, %error, "could not record the note of a conversation taken back"),
        }
    }
    if inner.state.selected_agent.as_deref() == Some(id)
        && let Some(transcript) = transcript_of(&inner, id)
    {
        factory.send_transcript(&inner, transcript);
    }
    // Agent.branches counts the version set aside. What was handed to the
    // stopped run and not saved goes to it once the queue is let go (`held`).
    factory.broadcast_workspace(&inner);
    drop(inner);
    if let (Some(branch), Some(noted)) = (branch, noted) {
        let folder = factory.home.agent_dir(id).join(BRANCHES).join(branch);
        if let Some(mut kept) = read_branch(&folder) {
            kept.note = Some(noted);
            let bytes = serde_json::to_vec_pretty(&kept).map_err(std::io::Error::other)?;
            replace(&folder.join(BRANCH_FILE), &bytes)?;
        }
    }
    Ok(kept)
}

/// Closes `id`'s cards made at `since` or later: its questions, secret
/// requests and proposals of app access, as /clear does. How many questions.
fn close_cards(factory: &Factory, id: &str, since: i64) -> usize {
    let questions = crate::extensions::ask::cleared_since(&factory.store, id, since).unwrap_or_else(|error| {
        tracing::warn!(agent = %id, %error, "could not dismiss its questions");
        0
    });
    if let Err(error) = crate::user_apps::cleared_since(&factory.store, id, since) {
        tracing::warn!(agent = %id, %error, "could not decline its proposals of app access");
    }
    if let Err(error) = crate::apps::access::cleared_since(&factory.store, id, since) {
        tracing::warn!(agent = %id, %error, "could not decline its proposals of what apps read");
    }
    let waiting = factory.vault.waiting_requests().unwrap_or_default();
    let mut withdrew = false;
    for (request, at) in waiting.into_iter().filter(|(request, at)| request.agent_id == id && *at >= since) {
        match factory.vault.withdraw_request(id, &request.id, "Its conversation went back to before it asked.") {
            Ok(request) => {
                crate::vault::requests::show_everywhere(factory, request);
                withdrew = true;
            }
            Err(error) => tracing::warn!(agent = %id, request = %request.id, %error, at, "could not withdraw a secret request"),
        }
    }
    if withdrew {
        factory.vault_changed();
    }
    questions
}

/// The versions set aside whose notes the agent hasn't read yet, the first
/// set aside first: it hasn't run since, so that one is the conversation it
/// remembers. The caller has halted it, so a note handed to it and not
/// saved is queued again.
fn unread(factory: &Factory, id: &str) -> Vec<Branch> {
    let queued: Vec<i64> = factory.lock().agents.get(id).map(|runtime| runtime.queue.iter().map(|queued| queued.id).collect()).unwrap_or_default();
    let mut unread: Vec<Branch> =
        branches(factory, id).into_iter().filter(|branch| branch.note.is_some_and(|note| queued.contains(&note))).collect();
    unread.reverse();
    unread
}

/// What the agent remembers that its conversation no longer holds, for its
/// note: the part of the version `known` (the conversation as it remembers
/// it) from where it parts from the conversation now, and whether that
/// went back (it ends there) or was switched to another version.
struct Forgotten {
    /// `known`, from where the two part.
    version: Branch,
    after: Vec<Message>,
    change: note::Change,
}

/// What the agent forgot of `known` (see `Forgotten`); None when the
/// conversation in `dir` holds all of it, or is it again (switched back to
/// it, its folder is gone).
fn forgotten(dir: &Path, known: Option<&Branch>) -> std::io::Result<Option<Forgotten>> {
    let Some(known) = known else { return Ok(None) };
    let folder = dir.join(BRANCHES).join(&known.id);
    if read_branch(&folder).is_none() {
        return Ok(None);
    }
    let point = parting(&folder, dir)?;
    let forgot = after(&folder, point)?;
    if forgot.is_empty() {
        return Ok(None);
    }
    let change = if after(dir, point)?.is_empty() { note::Change::Back } else { note::Change::Switched };
    let version = Branch { fork: point, messages: counted(&forgot), ..known.clone() };
    Ok(Some(Forgotten { version, after: forgot, change }))
}

/// How many messages `after` has: the agent's replies and what it was given.
fn counted(after: &[Message]) -> u32 {
    u32::try_from(after.iter().filter(|message| matches!(message, Message::User(_) | Message::Assistant(_))).count()).unwrap_or(u32::MAX)
}

/// Runs file work off the async threads.
async fn blocking<T: Send + 'static>(work: impl FnOnce() -> std::io::Result<T> + Send + 'static) -> Result<T, RequestError> {
    match tokio::task::spawn_blocking(work).await {
        Ok(result) => Ok(result?),
        Err(error) => Err(RequestError::Io(std::io::Error::other(error))),
    }
}

/// A version's folder in `dir`; refused for a name that isn't one.
fn branch_dir(dir: &Path, branch: &str) -> Result<PathBuf, RequestError> {
    let plain = !branch.is_empty() && branch.chars().all(|char| char.is_ascii_alphanumeric() || char == '-');
    if !plain {
        return Err(RequestError::NoBranch(branch.to_owned()));
    }
    Ok(dir.join(BRANCHES).join(branch))
}

fn read_branch(folder: &Path) -> Option<Branch> {
    let bytes = std::fs::read(folder.join(BRANCH_FILE)).ok()?;
    serde_json::from_slice(&bytes).ok()
}

/// The conversation's segment files in `dir`, by number (`log::files`).
fn segments(dir: &Path) -> std::io::Result<BTreeMap<u32, PathBuf>> {
    log::files(dir)
}

/// A segment's messages, and whether it started with a summary.
fn messages(path: &Path) -> std::io::Result<(Vec<Message>, bool)> {
    let read = log::read::<Message>(path)?;
    Ok((read.messages, read.compaction.is_some()))
}

/// Writes `bytes` to `path` through a file beside it renamed over it, so a
/// hard link to the old file keeps the old.
fn replace(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".tmp");
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, path)
}

/// The messages of the conversation in `dir` from `point` on, as they
/// happened: a summary isn't one, and the messages a compaction carried
/// into its segment were already counted in the one before.
fn after(dir: &Path, point: Point) -> std::io::Result<Vec<Message>> {
    let mut after: Vec<Message> = Vec::new();
    for (number, path) in segments(dir)?.range(point.segment..) {
        let (messages, summarized) = messages(path)?;
        let start = if *number == point.segment { point.index as usize } else { usize::from(summarized) };
        let mut kept = messages.get(start..).unwrap_or_default();
        if *number != point.segment && summarized {
            let mut carried = kept.len().min(after.len());
            while carried > 0 && !after.ends_with(&kept[..carried]) {
                carried -= 1;
            }
            kept = &kept[carried..];
        }
        after.extend_from_slice(kept);
    }
    Ok(after)
}

/// Sets the conversation in `dir` aside as another version, from `point`
/// on: links its files into a new folder of `branches/` with its
/// `branch.json`. None when nothing comes after `point`.
fn set_aside(dir: &Path, point: Point, made_by: &str, now: i64) -> std::io::Result<Option<Branch>> {
    let after = after(dir, point)?;
    if after.is_empty() {
        return Ok(None);
    }
    let mut id = now.to_string();
    let mut number = 1;
    while dir.join(BRANCHES).join(&id).exists() {
        number += 1;
        id = format!("{now}-{number}");
    }
    // Made beside its place and renamed into it whole, so it's counted
    // (`count`) only once it has its branch.json.
    let folder = dir.join(BRANCHES).join(format!(".new-{id}"));
    if folder.exists() {
        std::fs::remove_dir_all(&folder)?;
    }
    std::fs::create_dir_all(&folder)?;
    for path in segments(dir)?.values() {
        let into = folder.join(path.file_name().unwrap_or_default());
        if std::fs::hard_link(path, &into).is_err() {
            std::fs::copy(path, &into)?;
        }
    }
    let branch = Branch {
        id,
        label: label(&after, point.ms),
        set_aside_ms: now,
        fork: point,
        messages: counted(&after),
        made_by: made_by.to_owned(),
        note: None,
    };
    replace(&folder.join(BRANCH_FILE), &serde_json::to_vec_pretty(&branch).map_err(std::io::Error::other)?)?;
    std::fs::rename(&folder, dir.join(BRANCHES).join(&branch.id))?;
    Ok(Some(branch))
}

/// A version's label: the first words of the first message the person wrote
/// in it, or "From 14:02" (when it parts) when they wrote none.
fn label(after: &[Message], from: i64) -> String {
    let words = after.iter().find_map(|message| match message {
        Message::User(user) if user.event.is_none() => {
            let text = match &user.content {
                UserContent::Text(text) => text.clone(),
                UserContent::Parts(parts) => parts
                    .iter()
                    .filter_map(|part| match part {
                        UserPart::Text { text } => Some(text.as_str()),
                        UserPart::Image { .. } => None,
                    })
                    .collect::<Vec<_>>()
                    .join(" "),
            };
            let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
            (!text.is_empty()).then_some(text)
        }
        _ => None,
    });
    match words {
        Some(text) if text.chars().count() > LABEL_CHARS => {
            let cut: String = text.chars().take(LABEL_CHARS).collect();
            // At a word's end, when there's one.
            let cut = cut.rsplit_once(' ').map_or(cut.as_str(), |(words, _)| words).trim_end();
            format!("{cut}…")
        }
        Some(text) => text,
        None => format!("From {}", crate::transcript::local_time(from, "%H:%M")),
    }
}

/// Cuts the conversation in `dir` right before `point`: its segment keeps
/// what came before, everything else as it was, and later segments go
/// (their copies stay in the version set aside).
fn cut(dir: &Path, point: Point) -> std::io::Result<()> {
    let segments = segments(dir)?;
    if let Some(path) = segments.get(&point.segment) {
        // In the form it has, the rest of it as it was.
        let mut segment = log::read::<serde_json::Value>(path)?;
        segment.messages.truncate(point.index as usize);
        let whole = log::WholeSegment {
            compaction: segment.compaction.as_ref(),
            cleared: segment.cleared,
            messages: &segment.messages,
            pinned: &segment.pinned,
            frame: segment.frame.as_ref(),
        };
        log::replace(path, &whole.bytes(log::is_lines(path)).map_err(std::io::Error::other)?)?;
    }
    for (_, path) in segments.range(point.segment + 1..) {
        std::fs::remove_file(path)?;
    }
    Ok(())
}

/// Where the conversation in `dir` and the version in `branch` part: the
/// first message where they differ, when it was given (now, when the
/// conversation has nothing past it).
fn parting(dir: &Path, branch: &Path) -> std::io::Result<Point> {
    let (ours, theirs) = (segments(dir)?, segments(branch)?);
    let mut last = Point { segment: 1, index: 0, ms: now_ms() };
    for (number, path) in &ours {
        // Read, so a segment saved again as JSON Lines is the same.
        let (mine, _) = messages(path)?;
        let other = match theirs.get(number) {
            Some(path) => messages(path)?.0,
            None => Vec::new(),
        };
        let index = mine.iter().zip(&other).take_while(|(a, b)| a == b).count();
        if index < mine.len() || index < other.len() || !theirs.contains_key(number) {
            let ms = mine.get(index).and_then(|message| i64::try_from(message.timestamp()).ok()).unwrap_or_else(now_ms);
            return Ok(Point { segment: *number, index: u32::try_from(index).unwrap_or(u32::MAX), ms });
        }
        last = Point { segment: *number, index: u32::try_from(index).unwrap_or(u32::MAX), ms: now_ms() };
    }
    Ok(last)
}

/// Makes the version in `branch` the conversation in `dir`: the
/// conversation's files go (set aside already), the version's come back,
/// and its folder goes.
fn bring_back(dir: &Path, branch: &Path) -> std::io::Result<()> {
    for path in segments(dir)?.values() {
        std::fs::remove_file(path)?;
    }
    for (_, path) in segments(branch)? {
        std::fs::rename(&path, dir.join(path.file_name().unwrap_or_default()))?;
    }
    std::fs::remove_dir_all(branch)
}

/// Tests: takes `id`'s conversation back to the entry `seq`, as the person does.
#[doc(hidden)]
pub async fn fork_for_test(factory: &Arc<Factory>, id: &str, seq: u64, timestamp_ms: i64) -> Result<(), String> {
    fork(factory, id, seq, timestamp_ms, By::default()).await.map(drop).map_err(|error| error.to_string())
}

/// Tests: makes the version `branch` `id`'s conversation, as the person does.
#[doc(hidden)]
pub async fn switch_for_test(factory: &Arc<Factory>, id: &str, branch: &str) -> Result<(), String> {
    switch(factory, id, branch, By::default()).await.map(drop).map_err(|error| error.to_string())
}

/// Tests: deletes the version `branch` of `id`'s conversation.
#[doc(hidden)]
pub async fn delete_for_test(factory: &Arc<Factory>, id: &str, branch: &str) -> Result<(), String> {
    delete(factory, id, branch).await.map_err(|error| error.to_string())
}

/// Tests: the other versions of `id`'s conversation, the latest first.
#[doc(hidden)]
pub fn branches_for_test(factory: &Factory, id: &str) -> Vec<Branch> {
    branches(factory, id)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn user(text: &str, at: u64) -> Message {
        Message::User(UserMessage::text(text, at))
    }

    #[test]
    fn a_label_is_the_persons_first_words_or_when_it_parts() {
        let long = "Let's look at the scheduler again, this time with the queue's locks taken in the other order";
        assert_eq!(label(&[user(long, 1)], 0), "Let's look at the scheduler again, this time with the…");
        assert_eq!(label(&[user("  Try   it\nagain ", 1)], 0), "Try it again");
        let event = Message::User(UserMessage::event("done", crate::factory::host_event("done".into()), 1));
        assert!(label(&[event], 0).starts_with("From "));
    }

    fn temp() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("fork-{}-{}", std::process::id(), now_ms()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp");
        dir
    }

    fn write_segment(dir: &Path, number: u32, messages: &[Message], summarized: bool, lines: bool) {
        let frame: crate::transcript::Frame =
            serde_json::from_value(serde_json::json!({"settings": "", "system_prompt": "S", "tools": []})).expect("frame");
        let compaction = serde_json::from_value(serde_json::json!({ "summary": "s", "tokens_before": 1 })).expect("compaction");
        let whole = log::WholeSegment {
            compaction: summarized.then_some(&compaction),
            cleared: None,
            messages,
            pinned: &BTreeMap::new(),
            frame: Some(&frame),
        };
        std::fs::write(log::path(dir, number, lines), whole.bytes(lines).expect("json")).expect("write");
    }

    /// Before a compaction and after it, in segments of either form: what's
    /// set aside counts a carried message once, the cut keeps the rest of
    /// its segment as it was, and a conversation that has nothing past where
    /// it parts sets nothing aside.
    #[test]
    fn a_cut_keeps_what_came_before_and_sets_aside_the_rest() {
        for lines in [false, true] {
            let dir = temp();
            let (one, two, three, four) = (user("one", 1), user("two", 2), user("three", 3), user("four", 4));
            write_segment(&dir, 1, &[one.clone(), two.clone(), three.clone()], false, lines);
            write_segment(&dir, 2, &[user("summary", 3), three.clone(), four.clone()], true, lines);
            let point = Point { segment: 1, index: 1, ms: 2 };
            assert_eq!(after(&dir, point).expect("after"), [two.clone(), three.clone(), four.clone()]);

            let branch = set_aside(&dir, point, "", 10).expect("set aside").expect("a version");
            assert_eq!((branch.id.as_str(), branch.label.as_str(), branch.messages), ("10", "two", 3));
            cut(&dir, point).expect("cut");
            assert_eq!(segments(&dir).expect("segments").keys().copied().collect::<Vec<_>>(), [1]);
            let first = log::path(&dir, 1, lines);
            assert_eq!(messages(&first).expect("read").0, std::slice::from_ref(&one));
            let kept = log::read::<serde_json::Value>(&first).expect("read");
            assert_eq!(kept.frame.map(|frame| frame.system_prompt).as_deref(), Some("S"), "the rest as it was");
            // The version keeps the files as they were.
            let version = dir.join(BRANCHES).join("10");
            assert_eq!(messages(&log::path(&version, 1, lines)).expect("read").0.len(), 3);
            assert!(log::path(&version, 2, lines).is_file());

            // It parts right after "one"; nothing of the conversation is past that.
            let parts = parting(&dir, &version).expect("parting");
            assert_eq!((parts.segment, parts.index), (1, 1));
            assert!(set_aside(&dir, parts, "", 11).expect("set aside").is_none());
            bring_back(&dir, &version).expect("back");
            assert_eq!(after(&dir, point).expect("after"), [two, three, four]);
            assert!(!version.exists());
            let _ = std::fs::remove_dir_all(&dir);
        }
    }

    #[test]
    fn only_what_its_conversation_holds_counts_as_saved() {
        let queued = |text: &str| (Queued::from_person(text, None), None);
        let messages = vec![user("one", 1), user("two", 2)];
        let taken: std::collections::VecDeque<_> = [queued("two"), queued("three")].into();
        assert_eq!(saved(&messages, &taken), 1, "two is saved, three isn't");
        let taken: std::collections::VecDeque<_> = [queued("three")].into();
        assert_eq!(saved(&messages, &taken), 0);
        let taken: std::collections::VecDeque<_> = [queued("one"), queued("two")].into();
        assert_eq!(saved(&messages, &taken), 2);
    }
}
