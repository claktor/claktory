//! History's backfill (#458 B2, history-replay.md §3): what the Map was
//! before Claktory began recording it, reconstructed once from what lies
//! around, and kept in `map_history` as rows the replay draws dotted
//! (`inferred`).
//!
//! Recording begins with the whole Map at `since` (rows with why 'start').
//! Before that, this infers:
//! - agents appearing at the earliest of their folder's birth, their first
//!   message and their first run; deleted ones (not in the start) for their
//!   active span, named as they last signed a message;
//! - their names, roles, projects and folders from each transcript
//!   segment's frame, dated when the segment before it was last written (its
//!   compaction), the first at the agent's appearance; and from Claktory's
//!   "Your settings changed" notes between them;
//! - whom they could message from the frames' rosters, and from the "You can
//!   now message" notes between them;
//! - projects and their folders when their folder was made or their first
//!   agent appeared, the workspace with the first of all;
//! - pipes from when they were made.
//!
//! Leads, secrets and apps before `since` aren't known here. Whatever it
//! inferred that the start doesn't have ends just before `since`.
//!
//! It reads only factory.db and files on this computer, never the network
//! (Fugo), and never a whole transcript: a segment's frame is near the top
//! of a JSON Lines segment and the last key of an older JSON one, so only
//! that part is read.

use std::collections::{BTreeMap, BTreeSet};
use std::io::{BufRead, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use regex::Regex;
use rusqlite::{Connection, OptionalExtension, params};
use serde::Deserialize;
use serde_json::{Value, json};

const WHY: &str = "backfill";
const WORKSPACE: &str = "workspace";
/// How much of a segment's top is read for its frame (JSON Lines).
const HEAD_READ: u64 = 4 << 20;
/// How much of its end, at most, in steps (JSON, and JSON Lines written whole).
const TAIL_READS: [u64; 3] = [1 << 20, 8 << 20, 32 << 20];

/// A row for `map_history`, as recording writes them.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Row {
    pub at: i64,
    pub kind: &'static str,
    pub node: Option<String>,
    pub edge: Option<String>,
    pub data: Value,
}

/// How long after Claktory starts the backfill waits: recording has written
/// its start by then, and agents resuming have had their turn.
const AFTER_START: std::time::Duration = std::time::Duration::from_secs(60);

/// Runs the backfill in the background a while after start, on a connection
/// of its own; it does nothing once it has run.
pub(crate) fn spawn(home: &crate::home::Home) {
    let (db, agents) = (home.db_path(), home.root().join("agents"));
    tokio::spawn(async move {
        tokio::time::sleep(AFTER_START).await;
        let done = tokio::task::spawn_blocking(move || {
            let mut db = Connection::open(&db)?;
            db.busy_timeout(std::time::Duration::from_secs(30))?;
            run(&mut db, &agents, crate::transcript::now_ms())
        })
        .await;
        if let Ok(Err(error)) = done {
            tracing::warn!(%error, "History: couldn't reconstruct the Map from before recording began");
        }
    });
}

/// Reconstructs the Map before recording began, once: nothing until
/// recording has begun (its start rows written), and nothing after it ran.
/// The rows written.
pub(crate) fn run(db: &mut Connection, agents: &Path, now: i64) -> rusqlite::Result<usize> {
    let meta: Option<(i64, i64)> = db
        .query_row("SELECT since, backfilled FROM map_history_meta WHERE one = 1", [], |row| Ok((row.get(0)?, row.get(1)?)))
        .optional()?;
    let Some((since, 0)) = meta.filter(|(since, _)| *since > 0) else { return Ok(0) };
    let start = Start::read(db)?;
    if start.nodes.is_empty() {
        return Ok(0);
    }
    let rows = infer(db, agents, since, &start)?;
    let transaction = db.transaction()?;
    for row in &rows {
        transaction.execute(
            "INSERT INTO map_history (at, kind, node, edge, data, why, inferred) VALUES (?1, ?2, ?3, ?4, ?5, ?6, 1)",
            params![row.at, row.kind, row.node, row.edge, row.data.to_string(), WHY],
        )?;
    }
    transaction.execute(
        "INSERT INTO map_history_meta (one, backfilled) VALUES (1, ?1) ON CONFLICT(one) DO UPDATE SET backfilled = excluded.backfilled",
        [now.max(1)],
    )?;
    transaction.commit()?;
    tracing::info!(rows = rows.len(), since, "History: reconstructed the Map from before recording began");
    Ok(rows.len())
}

/// The Map as recording began: its start rows.
#[derive(Default)]
struct Start {
    nodes: BTreeMap<String, Value>,
    edges: BTreeMap<String, Value>,
}

impl Start {
    fn read(db: &Connection) -> rusqlite::Result<Start> {
        let mut start = Start::default();
        let mut statement =
            db.prepare("SELECT kind, node, edge, data FROM map_history WHERE why = 'start' AND inferred = 0 ORDER BY seq")?;
        let rows = statement.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?, row.get::<_, Option<String>>(2)?, row.get::<_, String>(3)?))
        })?;
        for row in rows {
            let (kind, node, edge, data) = row?;
            let data = serde_json::from_str(&data).unwrap_or(Value::Null);
            match (kind.as_str(), node, edge) {
                ("node_add", Some(node), _) => {
                    start.nodes.insert(node, data);
                }
                ("edge_add", _, Some(edge)) => {
                    start.edges.insert(edge, data);
                }
                _ => {}
            }
        }
        Ok(start)
    }

    fn kind(&self, node: &str) -> &str {
        text(self.nodes.get(node), "kind")
    }

    /// A project's folder at `path`, as the start has it.
    fn folder(&self, project: &str, path: &str) -> Option<&str> {
        let parent = format!("project:{project}");
        self.nodes
            .iter()
            .find(|(id, data)| self.kind(id) == "folder" && text(Some(data), "parent_id") == parent && text(Some(data), "detail") == path)
            .map(|(id, _)| id.as_str())
    }
}

fn text<'a>(data: Option<&'a Value>, field: &str) -> &'a str {
    data.and_then(|data| data.get(field)).and_then(Value::as_str).unwrap_or_default()
}

/// What an agent's settings were, as far as the Map shows them.
#[derive(Clone, Debug, Default, PartialEq)]
struct Settings {
    name: String,
    role: String,
    project: String,
    /// Its folder's path, or empty for its project's own.
    folder: String,
    admin: bool,
}

/// One thing known to have happened, at its time.
#[derive(Debug)]
enum Change {
    /// The workspace, a project, a folder or a pipe: as the start has it.
    Node(String),
    Agent(String),
    Settings(String, Settings),
    /// Its settings as a note told it: a new name or role.
    Renamed(String, Option<String>, Option<String>),
    Admin(String, bool),
    Roster(String, BTreeSet<String>),
    Reach(String, Vec<String>, Vec<String>),
    Gone(String),
}

impl Change {
    /// What goes first at one time: places, then agents, then what changed of them, then removals.
    fn order(&self) -> u8 {
        match self {
            Change::Node(_) => 0,
            Change::Agent(_) => 1,
            Change::Gone(_) => 3,
            _ => 2,
        }
    }
}

/// What's known of each agent's activity: first and last times, and the
/// name it last signed a message with.
#[derive(Default)]
struct Active {
    first: Option<i64>,
    last: Option<i64>,
    signed: Option<(i64, String)>,
}

impl Active {
    fn saw(&mut self, first: i64, last: i64) {
        self.first = Some(self.first.map_or(first, |known| known.min(first)));
        self.last = Some(self.last.map_or(last, |known| known.max(last)));
    }
}

/// The rows for before `since`, in time order.
fn infer(db: &Connection, agents_dir: &Path, since: i64, start: &Start) -> rusqlite::Result<Vec<Row>> {
    let active = activity(db)?;
    let mut ids: BTreeSet<String> = active.keys().cloned().collect();
    ids.extend(
        start.nodes.keys().filter(|node| start.kind(node) == "agent").filter_map(|node| node.strip_prefix("agent:")).map(str::to_owned),
    );
    let mut changes: Vec<(i64, Change)> = Vec::new();
    let mut seeds: BTreeMap<String, Settings> = BTreeMap::new();
    let mut appeared: BTreeMap<String, i64> = BTreeMap::new();
    for id in &ids {
        let node = format!("agent:{id}");
        let dir = agents_dir.join(id);
        let found = active.get(id);
        let at = [born(&dir), found.and_then(|active| active.first)].into_iter().flatten().min();
        let Some(at) = at.filter(|at| *at < since) else { continue };
        let in_start = start.nodes.get(&node);
        let signed = found.and_then(|active| active.signed.as_ref()).map(|(_, name)| name.clone());
        seeds.insert(id.clone(), seed(start, in_start, id, signed));
        appeared.insert(id.clone(), at);
        changes.push((at, Change::Agent(id.clone())));
        for (number, (dated, frame)) in frames(&dir).into_iter().enumerate() {
            let at = if number == 0 { at } else { dated.unwrap_or(at) };
            if let Some(settings) = frame.settings() {
                changes.push((at, Change::Settings(id.clone(), settings)));
            }
            if let Some(roster) = frame.roster {
                changes.push((at, Change::Roster(id.clone(), roster.into_keys().collect())));
            }
        }
        if in_start.is_none() {
            let last = found.and_then(|active| active.last).unwrap_or(at).max(at).min(since - 1);
            changes.push((last, Change::Gone(id.clone())));
        }
    }
    notes(db, &appeared, &mut changes)?;
    places(start, &appeared, &seeds, &mut changes);
    pipes(db, start, &mut changes)?;
    changes.retain(|(at, _)| *at < since);
    changes.sort_by_key(|(at, change)| (*at, change.order()));

    let mut state = State { start, nodes: BTreeSet::new(), agents: BTreeMap::new(), reach: BTreeMap::new() };
    let mut sent = Map::default();
    let mut rows = Vec::new();
    let mut index = 0;
    while index < changes.len() {
        let at = changes[index].0;
        while index < changes.len() && changes[index].0 == at {
            state.apply(&changes[index].1, &seeds);
            index += 1;
        }
        let now = state.map();
        diff(at, &sent, &now, &mut rows);
        sent = now;
    }
    // What the start doesn't have ended before it.
    let kept = Map {
        nodes: sent.nodes.iter().filter(|(id, _)| start.nodes.contains_key(*id)).map(|(id, data)| (id.clone(), data.clone())).collect(),
        edges: sent.edges.iter().filter(|(key, _)| start.edges.contains_key(*key)).map(|(key, data)| (key.clone(), data.clone())).collect(),
    };
    diff(since - 1, &sent, &kept, &mut rows);
    Ok(rows)
}

/// An agent's settings before any frame says them: as the start has them,
/// or, for one deleted, the name it last signed with.
fn seed(start: &Start, node: Option<&Value>, id: &str, signed: Option<String>) -> Settings {
    let Some(node) = node else { return Settings { name: signed.unwrap_or_else(|| id.to_owned()), ..Settings::default() } };
    let parent = text(Some(node), "parent_id");
    let folder = if start.kind(parent) == "folder" { text(start.nodes.get(parent), "detail").to_owned() } else { String::new() };
    Settings {
        name: text(Some(node), "label").to_owned(),
        role: text(Some(node), "detail").to_owned(),
        project: text(Some(node), "project_id").to_owned(),
        folder,
        admin: node.get("admin").and_then(Value::as_bool).unwrap_or(false),
    }
}

/// Each agent's first and last message, sent or received, and run.
fn activity(db: &Connection) -> rusqlite::Result<BTreeMap<String, Active>> {
    let mut active: BTreeMap<String, Active> = BTreeMap::new();
    let queries = [
        "SELECT agent, MIN(created), MAX(created) FROM messages GROUP BY agent",
        "SELECT sender_id, MIN(created), MAX(created) FROM messages WHERE source = 'agent' AND sender_id != '' GROUP BY sender_id",
        "SELECT agent, MIN(started), MAX(COALESCE(ended, started)) FROM runs GROUP BY agent",
    ];
    for query in queries {
        let mut statement = db.prepare(query)?;
        let rows = statement.query_map([], |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?, row.get::<_, i64>(2)?)))?;
        for row in rows {
            let (id, first, last) = row?;
            if !crate::chats::is_chat(&id) {
                active.entry(id).or_default().saw(first, last);
            }
        }
    }
    let mut statement = db.prepare(
        "SELECT sender_id, sender, created FROM messages WHERE id IN
            (SELECT MAX(id) FROM messages WHERE source = 'agent' AND sender_id != '' GROUP BY sender_id)",
    )?;
    let rows = statement.query_map([], |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?, row.get::<_, i64>(2)?)))?;
    for row in rows {
        let (id, name, at) = row?;
        if let Some(active) = active.get_mut(&id) {
            active.signed = Some((at, name));
        }
    }
    Ok(active)
}

/// When a file or folder was made, in ms; None where the file system doesn't say.
fn born(path: &Path) -> Option<i64> {
    let created = std::fs::metadata(path).ok()?.created().ok()?;
    ms(created)
}

fn modified(path: &Path) -> Option<i64> {
    ms(std::fs::metadata(path).ok()?.modified().ok()?)
}

fn ms(time: std::time::SystemTime) -> Option<i64> {
    i64::try_from(time.duration_since(std::time::UNIX_EPOCH).ok()?.as_millis()).ok()
}

/// What a frame says, of what the Map shows.
#[derive(Deserialize, Default)]
struct Frame {
    #[serde(default)]
    settings: String,
    /// None in frames from before rosters: unknown, not no one.
    #[serde(default)]
    roster: Option<BTreeMap<String, String>>,
}

impl Frame {
    fn settings(&self) -> Option<Settings> {
        let settings: Value = serde_json::from_str(&self.settings).ok()?;
        let field = |name: &str| settings.get(name).and_then(Value::as_str).unwrap_or_default().to_owned();
        Some(Settings {
            name: field("name"),
            role: field("role"),
            project: field("project"),
            folder: field("folder"),
            admin: settings.pointer("/extensions/agents/admin").and_then(Value::as_bool).unwrap_or(false),
        })
        .filter(|settings| !settings.name.is_empty())
    }
}

/// An agent's segments' frames, in order, each with when the segment before
/// it was last written (None for the first).
fn frames(dir: &Path) -> Vec<(Option<i64>, Frame)> {
    static SEGMENT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^segment-(\d+)\.(json|jsonl)$").expect("pattern"));
    let Ok(read) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut segments: BTreeMap<u32, PathBuf> = BTreeMap::new();
    for entry in read.flatten() {
        let name = entry.file_name();
        let Some(found) = name.to_str().and_then(|name| SEGMENT.captures(name)) else { continue };
        let Ok(number) = found[1].parse::<u32>() else { continue };
        // Both forms for a moment while one replaces the other: JSON Lines is the newer.
        let lines = &found[2] == "jsonl";
        if lines || !segments.contains_key(&number) {
            segments.insert(number, entry.path());
        }
    }
    let mut before = None;
    let mut frames = Vec::new();
    for path in segments.values() {
        let frame = if crate::transcript::log::is_lines(path) { lines_frame(path) } else { whole_frame(path) };
        if let Some(frame) = frame {
            frames.push((before, frame));
        }
        before = modified(path);
    }
    frames
}

#[derive(Deserialize)]
struct FrameLine {
    frame: Frame,
}

/// A JSON Lines segment's frame: its line, near the top as the segment
/// began, or near the end when it was written whole again.
fn lines_frame(path: &Path) -> Option<Frame> {
    let file = std::fs::File::open(path).ok()?;
    let mut head = std::io::BufReader::new(file.take(HEAD_READ));
    let mut line = Vec::new();
    loop {
        line.clear();
        if head.read_until(b'\n', &mut line).ok()? == 0 {
            break;
        }
        if line.starts_with(b"{\"frame\":")
            && let Ok(found) = serde_json::from_slice::<FrameLine>(&line)
        {
            return Some(found.frame);
        }
    }
    let marker = b"\n{\"frame\":";
    tail(path, |bytes| {
        let at = bytes.windows(marker.len()).rposition(|window| window == marker)? + 1;
        let end = bytes[at..].iter().position(|byte| *byte == b'\n').map_or(bytes.len(), |end| at + end);
        serde_json::from_slice::<FrameLine>(&bytes[at..end]).ok().map(|found| found.frame)
    })
}

/// A JSON segment's frame: its last key, so read from the end.
fn whole_frame(path: &Path) -> Option<Frame> {
    let key = b"\"frame\":";
    tail(path, |bytes| {
        let end = bytes.iter().rposition(|byte| !byte.is_ascii_whitespace())?;
        if bytes[end] != b'}' {
            return None;
        }
        let mut search = end;
        while let Some(at) = bytes[..search].windows(key.len()).rposition(|window| window == key) {
            search = at;
            if at > 0 && bytes[at - 1] == b'\\' {
                continue;
            }
            if let Ok(frame) = serde_json::from_slice::<Option<Frame>>(&bytes[at + key.len()..end]) {
                return frame;
            }
        }
        None
    })
}

/// `find` on the end of the file, read a larger part at a time until found.
fn tail<T>(path: &Path, find: impl Fn(&[u8]) -> Option<T>) -> Option<T> {
    let mut file = std::fs::File::open(path).ok()?;
    let length = file.metadata().ok()?.len();
    for most in TAIL_READS {
        let read = most.min(length);
        file.seek(SeekFrom::Start(length - read)).ok()?;
        let mut bytes = Vec::with_capacity(usize::try_from(read).ok()?);
        (&mut file).take(read).read_to_end(&mut bytes).ok()?;
        if let Some(found) = find(&bytes) {
            return Some(found);
        }
        if read == length {
            break;
        }
    }
    None
}

/// Claktory's notes to agents between frames: whom they can message now or
/// no longer, a new name or role, the admin option. Dated when they were
/// queued, which is when the agent was next about to run.
fn notes(db: &Connection, agents: &BTreeMap<String, i64>, changes: &mut Vec<(i64, Change)>) -> rusqlite::Result<()> {
    static REACH: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"You can (now|no longer) message: (.+?)\.(?:\s|$)").expect("pattern"));
    static SETTINGS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"Your settings changed: (.+?)\.(?:\s|$)").expect("pattern"));
    static ID: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\(([a-z0-9][a-z0-9_-]*)[,)]").expect("pattern"));
    let mut statement = db.prepare(
        "SELECT agent, created, text FROM messages WHERE source = 'factory'
            AND (text LIKE '%You can now message:%' OR text LIKE '%You can no longer message:%' OR text LIKE 'Your settings changed:%')",
    )?;
    let rows = statement.query_map([], |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?, row.get::<_, String>(2)?)))?;
    for row in rows {
        let (agent, at, text) = row?;
        if !agents.contains_key(&agent) {
            continue;
        }
        let (mut gained, mut lost) = (Vec::new(), Vec::new());
        for found in REACH.captures_iter(&text) {
            let ids = ID.captures_iter(&found[2]).map(|id| id[1].to_owned()).filter(|id| agents.contains_key(id));
            if &found[1] == "now" { gained.extend(ids) } else { lost.extend(ids) }
        }
        if !gained.is_empty() || !lost.is_empty() {
            changes.push((at, Change::Reach(agent.clone(), gained, lost)));
        }
        let Some(said) = SETTINGS.captures(&text) else { continue };
        let (mut name, mut role) = (None, None);
        for item in said[1].split("; ") {
            if let Some(new) = item.strip_prefix("your name is now ") {
                name = Some(new.to_owned());
            } else if let Some(new) = item.strip_prefix("your role is now ") {
                role = Some(new.to_owned());
            } else if item == "you no longer have a role" {
                role = Some(String::new());
            } else if let Some(on) = item.strip_prefix("the Messaging option admin is now ") {
                changes.push((at, Change::Admin(agent.clone(), on == "on")));
            }
        }
        if name.is_some() || role.is_some() {
            changes.push((at, Change::Renamed(agent.clone(), name, role)));
        }
    }
    Ok(())
}

/// The workspace with the first agent; each project in the start, and its
/// folders, when its folder was made or its first agent appeared.
fn places(start: &Start, appeared: &BTreeMap<String, i64>, seeds: &BTreeMap<String, Settings>, changes: &mut Vec<(i64, Change)>) {
    if let Some(first) = appeared.values().min()
        && start.nodes.contains_key(WORKSPACE)
    {
        changes.push((*first, Change::Node(WORKSPACE.to_owned())));
    }
    // Projects each agent was ever in, by its frames and the start.
    let mut first_in: BTreeMap<String, i64> = BTreeMap::new();
    for (at, change) in changes.iter() {
        if let Change::Settings(_, settings) = change
            && !settings.project.is_empty()
        {
            let known = first_in.entry(settings.project.clone()).or_insert(*at);
            *known = (*known).min(*at);
        }
    }
    for (id, settings) in seeds {
        if let (Some(at), false) = (appeared.get(id), settings.project.is_empty()) {
            let known = first_in.entry(settings.project.clone()).or_insert(*at);
            *known = (*known).min(*at);
        }
    }
    for (node, data) in &start.nodes {
        let Some(project) = node.strip_prefix("project:").filter(|_| start.kind(node) == "project") else { continue };
        let made = born(Path::new(text(Some(data), "detail")));
        let Some(at) = [made, first_in.get(project).copied()].into_iter().flatten().min() else { continue };
        changes.push((at, Change::Node(node.clone())));
        for (folder, data) in &start.nodes {
            if start.kind(folder) == "folder" && text(Some(data), "parent_id") == node {
                changes.push((at, Change::Node(folder.clone())));
            }
        }
    }
}

/// The start's pipes, from when each was made (closed ones aren't in it).
fn pipes(db: &Connection, start: &Start, changes: &mut Vec<(i64, Change)>) -> rusqlite::Result<()> {
    let exists: bool =
        db.query_row("SELECT EXISTS (SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'pipes')", [], |row| row.get(0))?;
    if !exists {
        return Ok(());
    }
    let mut statement = db.prepare("SELECT id, data FROM pipes")?;
    let rows = statement.query_map([], |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)))?;
    for row in rows {
        let (id, data) = row?;
        let node = format!("pipe:{id}");
        if !start.nodes.contains_key(&node) {
            continue;
        }
        let created = serde_json::from_str::<Value>(&data).ok().and_then(|data| data.get("created").and_then(Value::as_i64));
        if let Some(created) = created.filter(|created| *created > 0) {
            changes.push((created, Change::Node(node)));
        }
    }
    Ok(())
}

/// What's known at a time, to draw the Map from.
struct State<'a> {
    start: &'a Start,
    /// The workspace, projects, folders and pipes there.
    nodes: BTreeSet<String>,
    agents: BTreeMap<String, Settings>,
    /// Whom each agent can message.
    reach: BTreeMap<String, BTreeSet<String>>,
}

#[derive(Default, Clone)]
struct Map {
    nodes: BTreeMap<String, Value>,
    edges: BTreeMap<String, Value>,
}

impl State<'_> {
    fn apply(&mut self, change: &Change, seeds: &BTreeMap<String, Settings>) {
        match change {
            Change::Node(node) => {
                self.nodes.insert(node.clone());
            }
            Change::Agent(id) => {
                self.agents.insert(id.clone(), seeds.get(id).cloned().unwrap_or_default());
            }
            Change::Settings(id, settings) => {
                if let Some(known) = self.agents.get_mut(id) {
                    *known = settings.clone();
                }
            }
            Change::Renamed(id, name, role) => {
                if let Some(known) = self.agents.get_mut(id) {
                    known.name = name.clone().unwrap_or_else(|| known.name.clone());
                    known.role = role.clone().unwrap_or_else(|| known.role.clone());
                }
            }
            Change::Admin(id, admin) => {
                if let Some(known) = self.agents.get_mut(id) {
                    known.admin = *admin;
                }
            }
            Change::Roster(id, roster) => {
                self.reach.insert(id.clone(), roster.clone());
            }
            Change::Reach(id, gained, lost) => {
                let reach = self.reach.entry(id.clone()).or_default();
                reach.extend(gained.iter().cloned());
                for gone in lost {
                    reach.remove(gone);
                }
            }
            Change::Gone(id) => {
                self.agents.remove(id);
                self.reach.remove(id);
            }
        }
    }

    /// Where an agent works: its folder, its project, or the workspace.
    fn place(&self, settings: &Settings) -> String {
        let project = format!("project:{}", settings.project);
        if settings.project.is_empty() || !self.nodes.contains(&project) {
            return WORKSPACE.to_owned();
        }
        match self.start.folder(&settings.project, &settings.folder).filter(|_| !settings.folder.is_empty()) {
            Some(folder) if self.nodes.contains(folder) => folder.to_owned(),
            _ => project,
        }
    }

    /// The Map as `crate::map` would draw it from what's known.
    fn map(&self) -> Map {
        let mut map = Map::default();
        for node in &self.nodes {
            map.nodes.insert(node.clone(), self.start.nodes.get(node).cloned().unwrap_or(Value::Null));
        }
        for (id, settings) in &self.agents {
            let node = format!("agent:{id}");
            let place = self.place(settings);
            let known = self.start.nodes.get(&node);
            let project = if place == WORKSPACE { String::new() } else { settings.project.clone() };
            map.nodes.insert(
                node.clone(),
                json!({
                    "kind": "agent",
                    "label": settings.name,
                    "detail": settings.role,
                    "parent_id": if settings.admin { String::new() } else { place.clone() },
                    "project_id": project,
                    "ref_id": id,
                    "color": text(known, "color"),
                    "admin": settings.admin,
                    "badges": known.and_then(|known| known.get("badges")).cloned().unwrap_or_else(|| json!([])),
                    "icon": "",
                }),
            );
            let edge = |to: &str, kind: &str| (format!("{node}|{to}|{kind}"), json!({"both_ways": false, "label": ""}));
            let (writes, data) = edge(&place, "writes");
            map.edges.insert(writes, data);
            if !project.is_empty() {
                let project_node = format!("project:{project}");
                let others = std::iter::once(project_node.clone())
                    .chain(
                        self.nodes
                            .iter()
                            .filter(|other| {
                                text(self.start.nodes.get(*other), "parent_id") == project_node && self.start.kind(other) == "folder"
                            })
                            .cloned(),
                    )
                    .filter(|other| *other != place)
                    .collect::<Vec<_>>();
                map.edges.extend(others.iter().map(|other| edge(other, "reads")));
            }
        }
        for (from, reach) in &self.reach {
            if !self.agents.contains_key(from) {
                continue;
            }
            for to in reach.iter().filter(|to| self.agents.contains_key(*to) && *to != from) {
                let back = self.reach.get(to).is_some_and(|theirs| theirs.contains(from));
                // A pair that messages both ways is one edge, from the lower id.
                if back && to < from {
                    continue;
                }
                map.edges.insert(format!("agent:{from}|agent:{to}|messages"), json!({"both_ways": back, "label": ""}));
            }
        }
        map
    }
}

/// The rows that take `before` to `after` at `at`: edges and nodes gone,
/// then nodes and edges new or changed.
fn diff(at: i64, before: &Map, after: &Map, rows: &mut Vec<Row>) {
    let row = |kind: &'static str, node: Option<&String>, edge: Option<&String>, data: Value| Row {
        at,
        kind,
        node: node.cloned(),
        edge: edge.cloned(),
        data,
    };
    for key in before.edges.keys().filter(|key| !after.edges.contains_key(*key)) {
        rows.push(row("edge_remove", None, Some(key), json!({})));
    }
    for node in before.nodes.keys().filter(|node| !after.nodes.contains_key(*node)) {
        rows.push(row("node_remove", Some(node), None, json!({})));
    }
    for (node, data) in &after.nodes {
        match before.nodes.get(node) {
            None => rows.push(row("node_add", Some(node), None, data.clone())),
            Some(was) if was != data => rows.push(row("node_change", Some(node), None, changed(was, data))),
            Some(_) => {}
        }
    }
    for (key, data) in &after.edges {
        match before.edges.get(key) {
            None => rows.push(row("edge_add", None, Some(key), data.clone())),
            Some(was) if was != data => rows.push(row("edge_change", None, Some(key), data.clone())),
            Some(_) => {}
        }
    }
}

/// Only the fields of `now` that differ from `was`.
fn changed(was: &Value, now: &Value) -> Value {
    let (Some(was), Some(now)) = (was.as_object(), now.as_object()) else { return now.clone() };
    Value::Object(
        now.iter().filter(|(field, value)| was.get(*field) != Some(*value)).map(|(field, value)| (field.clone(), value.clone())).collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The pipes' table, which the pipes keep apart from the store's.
    const PIPES: &str = "CREATE TABLE IF NOT EXISTS pipes (id TEXT PRIMARY KEY, data TEXT NOT NULL);";

    const SINCE: i64 = 10_000;

    /// A folder of its own for each test, removed after.
    struct Dir(PathBuf);

    impl Dir {
        fn new(name: &str) -> Dir {
            let dir = std::env::temp_dir().join(format!("history-backfill-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).expect("dir");
            Dir(dir)
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for Dir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn start(db: &Connection, node: Option<&str>, edge: Option<&str>, data: Value) {
        let kind = if node.is_some() { "node_add" } else { "edge_add" };
        db.execute(
            "INSERT INTO map_history (at, kind, node, edge, data, why, inferred) VALUES (?1, ?2, ?3, ?4, ?5, 'start', 0)",
            params![SINCE, kind, node, edge, data.to_string()],
        )
        .expect("start row");
    }

    fn message(db: &Connection, to: &str, source: &str, from: (&str, &str), text: &str, at: i64) {
        db.execute(
            "INSERT INTO messages (agent, source, sender, sender_id, summary, text, created) VALUES (?1, ?2, ?3, ?4, '', ?5, ?6)",
            params![to, source, from.0, from.1, text, at],
        )
        .expect("message");
    }

    fn agent(id: &str, label: &str, role: &str, parent: &str) -> Value {
        json!({"kind": "agent", "label": label, "detail": role, "parent_id": parent, "project_id": "site", "ref_id": id,
            "color": "blue", "admin": false, "badges": [], "icon": ""})
    }

    fn frame(name: &str, role: &str, roster: &[&str]) -> Value {
        let settings = json!({"name": name, "role": role, "project": "site", "folder": null, "model": "m"}).to_string();
        let roster: BTreeMap<&str, String> = roster.iter().map(|id| (*id, format!("{id} ({id})"))).collect();
        json!({"settings": settings, "system_prompt": "You are… \"frame\": not this", "tools": [], "roster": roster})
    }

    /// A fixture home: Mike, whose two segments rename him, and who could
    /// message Ana and later Zed; Ana, renamed by a note that also lets her
    /// message Mike; Zed, deleted, active for a while; a pipe.
    fn fixture(dir: &Path) -> Connection {
        let store = crate::store::Store::open(&dir.join("factory.db")).expect("store");
        drop(store);
        let db = Connection::open(dir.join("factory.db")).expect("db");
        db.execute_batch(PIPES).expect("pipes");
        db.execute("INSERT INTO map_history_meta (one, since) VALUES (1, ?1)", [SINCE]).expect("meta");
        start(&db, Some("workspace"), None, json!({"kind": "workspace", "label": "Workspace"}));
        start(&db, Some("project:site"), None, json!({"kind": "project", "label": "Site", "detail": dir.join("no-such-site")}));
        start(
            &db,
            Some("folder:site/docs"),
            None,
            json!({"kind": "folder", "label": "Docs", "detail": "/x/docs", "parent_id": "project:site"}),
        );
        start(&db, Some("agent:mike"), None, agent("mike", "Mike", "Coordinator", "project:site"));
        start(&db, Some("agent:ana"), None, agent("ana", "Ana", "", "folder:site/docs"));
        start(
            &db,
            Some("pipe:p1"),
            None,
            json!({"kind": "pipe", "label": "Pipe", "detail": "Another Claktory", "parent_id": "project:site"}),
        );
        let both = json!({"both_ways": true, "label": ""});
        start(&db, None, Some("agent:ana|agent:mike|messages"), both);
        let plain = json!({"both_ways": false, "label": ""});
        for edge in ["agent:mike|project:site|writes", "agent:mike|folder:site/docs|reads", "agent:ana|folder:site/docs|writes"] {
            start(&db, None, Some(edge), plain.clone());
        }
        db.execute("INSERT INTO pipes (id, data) VALUES ('p1', ?1)", [json!({"id": "p1", "created": 4500}).to_string()]).expect("pipe");
        db.execute("INSERT INTO runs (agent, version, started, ended) VALUES ('mike', 'v', 1000, 1500)", []).expect("run");
        db.execute("INSERT INTO runs (agent, version, started, ended) VALUES ('zed', 'v', 2500, 6000)", []).expect("run");
        message(&db, "ana", "agent", ("Mike", "mike"), "hi", 2000);
        message(&db, "mike", "agent", ("Zed", "zed"), "hello", 3000);
        let note = "Your settings changed: your name is now Ana B.\nYou can now message: Mike (mike, Coordinator).";
        message(&db, "ana", "factory", ("Factory", ""), note, 4000);

        let mike = dir.join("agents/mike");
        std::fs::create_dir_all(&mike).expect("folder");
        // An older JSON segment: its frame is its last key, after messages
        // that hold the word too.
        let first = json!({"messages": [{"role": "user", "content": [{"type": "text", "text": "\"frame\": {}"}]},
            {"role": "assistant", "content": [{"type": "toolCall", "arguments": {"frame": 1}}]}],
            "frame": frame("Mikey", "Planner", &["ana"])});
        std::fs::write(mike.join("segment-0001.json"), first.to_string()).expect("segment");
        let file = std::fs::File::options().write(true).open(mike.join("segment-0001.json")).expect("open");
        file.set_modified(std::time::UNIX_EPOCH + std::time::Duration::from_millis(5000)).expect("dated");
        // A JSON Lines one: its frame's line near the top.
        let lines = [
            json!({"compaction": {"summary": "s", "tokens_before": 1}}),
            json!({"message": {"role": "user", "content": "go"}}),
            json!({"frame": frame("Mike", "Coordinator", &["ana", "zed"])}),
            json!({"message": {"role": "user", "content": "more"}}),
        ];
        let text: String = lines.iter().map(|line| format!("{line}\n")).collect();
        std::fs::write(mike.join("segment-0002.jsonl"), text).expect("segment");
        db
    }

    fn rows(db: &Connection) -> Vec<(i64, String, String, Value)> {
        let mut statement = db
            .prepare("SELECT at, kind, COALESCE(node, edge), data FROM map_history WHERE why = 'backfill' AND inferred = 1 ORDER BY seq")
            .expect("query");
        statement
            .query_map([], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?, serde_json::from_str(&row.get::<_, String>(3)?).expect("json")))
            })
            .expect("rows")
            .map(Result::unwrap)
            .collect()
    }

    fn at(rows: &[(i64, String, String, Value)], kind: &str, what: &str) -> Vec<i64> {
        rows.iter().filter(|row| row.1 == kind && row.2 == what).map(|row| row.0).collect()
    }

    #[test]
    fn the_map_before_recording_is_reconstructed_once_from_frames_notes_and_activity() {
        let dir = Dir::new("once");
        let mut db = fixture(dir.path());
        let written = run(&mut db, &dir.path().join("agents"), 20_000).expect("backfill");
        let rows = rows(&db);
        assert_eq!(written, rows.len());
        assert!(rows.iter().all(|row| row.0 < SINCE), "{rows:#?}");
        assert!(rows.windows(2).all(|pair| pair[0].0 <= pair[1].0), "in time order");

        // Places with the first agent; Mike as his first frame named him, renamed by his second.
        for node in ["workspace", "project:site", "folder:site/docs", "agent:mike"] {
            assert_eq!(at(&rows, "node_add", node), [1000], "{node}");
        }
        let mike = rows.iter().find(|row| row.1 == "node_add" && row.2 == "agent:mike").expect("mike");
        assert_eq!(
            (&mike.3["label"], &mike.3["detail"], &mike.3["parent_id"]),
            (&json!("Mikey"), &json!("Planner"), &json!("project:site"))
        );
        let renamed = rows.iter().find(|row| row.1 == "node_change" && row.2 == "agent:mike").expect("renamed");
        assert_eq!((renamed.0, renamed.3.clone()), (5000, json!({"label": "Mike", "detail": "Coordinator"})));
        assert_eq!(at(&rows, "edge_add", "agent:mike|project:site|writes"), [1000]);

        // Ana when she was first sent something, in her folder; renamed by a note.
        assert_eq!(at(&rows, "node_add", "agent:ana"), [2000]);
        let ana = rows.iter().find(|row| row.1 == "node_add" && row.2 == "agent:ana").expect("ana");
        assert_eq!(ana.3["parent_id"], json!("folder:site/docs"));
        let renamed = rows.iter().find(|row| row.1 == "node_change" && row.2 == "agent:ana").expect("renamed");
        assert_eq!((renamed.0, renamed.3.clone()), (4000, json!({"label": "Ana B"})));

        // Mike could message Ana from her start; the note made it both ways,
        // one edge from the lower id.
        assert_eq!(at(&rows, "edge_add", "agent:mike|agent:ana|messages"), [2000]);
        assert_eq!(at(&rows, "edge_remove", "agent:mike|agent:ana|messages"), [4000]);
        let both = rows.iter().find(|row| row.1 == "edge_add" && row.2 == "agent:ana|agent:mike|messages").expect("both");
        assert_eq!((both.0, both.3["both_ways"].clone()), (4000, json!(true)));

        // Zed, deleted: from his first run to his last, under his signed name,
        // and Mike's edge to him only while both were there.
        assert_eq!(at(&rows, "node_add", "agent:zed"), [2500]);
        assert_eq!(at(&rows, "node_remove", "agent:zed"), [6000]);
        let zed = rows.iter().find(|row| row.1 == "node_add" && row.2 == "agent:zed").expect("zed");
        assert_eq!((&zed.3["label"], &zed.3["parent_id"]), (&json!("Zed"), &json!("workspace")));
        assert_eq!(at(&rows, "edge_add", "agent:mike|agent:zed|messages"), [5000]);
        assert_eq!(at(&rows, "edge_remove", "agent:mike|agent:zed|messages"), [6000]);

        // The pipe from when it was made.
        assert_eq!(at(&rows, "node_add", "pipe:p1"), [4500]);

        // What the start lacks ends just before it; what it has goes on.
        assert_eq!(at(&rows, "edge_remove", "agent:ana|project:site|reads"), [SINCE - 1]);
        assert!(at(&rows, "edge_remove", "agent:ana|agent:mike|messages").is_empty());
        assert!(at(&rows, "node_remove", "agent:mike").is_empty());

        // Once only.
        assert_eq!(run(&mut db, &dir.path().join("agents"), 30_000).expect("again"), 0);
        let backfilled: i64 = db.query_row("SELECT backfilled FROM map_history_meta", [], |row| row.get(0)).expect("meta");
        assert_eq!(backfilled, 20_000);
    }

    #[test]
    fn nothing_is_reconstructed_before_recording_has_begun() {
        let dir = Dir::new("not-begun");
        let mut db = fixture(dir.path());
        db.execute("DELETE FROM map_history", []).expect("no start");
        assert_eq!(run(&mut db, &dir.path().join("agents"), 20_000).expect("backfill"), 0);
        db.execute("UPDATE map_history_meta SET since = 0", []).expect("not begun");
        assert_eq!(run(&mut db, &dir.path().join("agents"), 20_000).expect("backfill"), 0);
        let backfilled: i64 = db.query_row("SELECT backfilled FROM map_history_meta", [], |row| row.get(0)).expect("meta");
        assert_eq!(backfilled, 0, "it runs once it can");
    }
}
