//! Comments on files: the person selects text in a viewer pane (or a region
//! of an image) and comments on it, and the agent it's addressed to reads the
//! comments, answers and revises the file.
//!
//! A thread belongs to a file, not a pane: every pane showing the file shows
//! it. Its place is a range of the file's text, kept with a quote of that
//! text, and follows the text as the file is rewritten (`anchor`): through
//! the diff from the text it was placed on, kept for that (`thread_files`),
//! or by finding its quote again; when neither works, it's detached, never
//! dropped. Images' threads keep the region they were made with.
//!
//! The person's comments are drafts until sent: SendComments gives the agent
//! all of them on a file as one event (or a comment goes at once, alone).
//! Agents answer with the Comments extension's tools, and never see drafts.
//!
//! Files in panes are watched (every second, and at once after an agent's
//! write_file or edit_file): when one changes, every client hears of it
//! (FileChanged), and its threads are placed again and sent (ThreadList).

pub(crate) mod anchor;
mod region;

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fmt::Write as _;
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, Weak};
use std::time::{Duration, SystemTime};

use factory_protocol::v1::{self, server_message::Body};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use self::anchor::{Doc, Mapping, Quote, Space};
use crate::factory::{Factory, RequestError, host_event};
use crate::locking::lock as guard;
use crate::transcript::now_ms;

pub(crate) const SCHEMA: &str = "
-- Comment threads on files (crate::annotations).
CREATE TABLE threads (
    id INTEGER PRIMARY KEY,
    -- The agent it's addressed to.
    agent TEXT NOT NULL,
    -- The file, absolute.
    path TEXT NOT NULL,
    -- text, markdown or image: the space its quote is in.
    kind TEXT NOT NULL,
    -- The file's SHA-256 (hex) its place refers to.
    version TEXT NOT NULL,
    -- Its range of the file's text, in UTF-16 code units; null when it's
    -- detached, and for images.
    range_start INTEGER,
    range_end INTEGER,
    -- Its quote as placed now, JSON {exact, prefix, suffix}, in its kind's space.
    quote TEXT NOT NULL,
    -- An image's selectors, as made (a JSON array); '[]' for text.
    selectors TEXT NOT NULL,
    -- open or resolved
    status TEXT NOT NULL,
    -- anchored, moved, detached or outdated
    anchor TEXT NOT NULL,
    -- The text it was made on.
    original TEXT NOT NULL,
    created INTEGER NOT NULL,
    updated INTEGER NOT NULL
);
CREATE INDEX threads_by_path ON threads (path);
CREATE INDEX threads_by_agent ON threads (agent);
CREATE TABLE thread_comments (
    id INTEGER PRIMARY KEY,
    thread INTEGER NOT NULL,
    -- The agent who wrote it; '' for the person.
    author TEXT NOT NULL,
    body TEXT NOT NULL,
    created INTEGER NOT NULL,
    -- When the person last changed it.
    edited INTEGER,
    -- 1 for the person's, until it's sent to the agent.
    draft INTEGER NOT NULL
);
CREATE INDEX thread_comments_by_thread ON thread_comments (thread, id);
-- The text of each file with text threads, as their places refer to it.
CREATE TABLE thread_files (path TEXT PRIMARY KEY, sha TEXT NOT NULL, text TEXT NOT NULL);
";

/// Suggestions: a change to a thread's text, proposed (slice 2).
pub(crate) const SUGGESTIONS: &str = "
-- A change to a thread's text, proposed: the replacement (null for none),
-- the agent who suggested it ('' for the person), and what became of it
-- ('' until decided, accepted or rejected).
ALTER TABLE threads ADD COLUMN suggestion TEXT;
ALTER TABLE threads ADD COLUMN suggested_by TEXT;
ALTER TABLE threads ADD COLUMN decision TEXT;
";

/// Text files are commented on up to this size, as viewers read them.
const TEXT_LIMIT: u64 = 2_000_000;
/// How often files shown in panes are checked for changes.
const POLL_EVERY: Duration = Duration::from_secs(1);
/// A quote in an event is cut to this many characters.
const QUOTE_SHOWN: usize = 160;

pub(crate) const CHANGED: &str = "The text changed; select it again.";

/// What the poller last saw of the files in panes, and a lock that keeps
/// changes to threads one at a time.
#[derive(Default)]
pub(crate) struct State {
    seen: Mutex<HashMap<String, Seen>>,
    work: Mutex<()>,
}

#[derive(Clone, Debug, PartialEq)]
struct Seen {
    stamp: Option<(SystemTime, u64)>,
    /// Empty when it couldn't be read.
    sha: String,
}

/// A file's content, as threads need it.
enum Content {
    Text {
        sha: String,
        text: String,
    },
    /// Not text (an image), or too big to be.
    Bytes {
        sha: String,
    },
    /// Missing, unreadable or too big.
    Gone,
}

impl Content {
    fn sha(&self) -> Option<&str> {
        match self {
            Content::Text { sha, .. } | Content::Bytes { sha } => Some(sha),
            Content::Gone => None,
        }
    }
}

fn sha256(bytes: &[u8]) -> String {
    crate::encoding::hex(&Sha256::digest(bytes))
}

fn stamp(path: &Path) -> Option<(SystemTime, u64)> {
    let metadata = std::fs::metadata(path).ok()?;
    Some((metadata.modified().ok()?, metadata.len()))
}

fn read(path: &Path) -> Content {
    let Ok(metadata) = std::fs::metadata(path) else { return Content::Gone };
    if !metadata.is_file() {
        return Content::Gone;
    }
    // Too big to be text (an image, a video): only its hash, kept while
    // the file is unchanged; files over 2 GB are gone.
    if metadata.len() > TEXT_LIMIT {
        return match crate::extensions::viewer::whole_sha256(path) {
            Ok(sha) => Content::Bytes { sha },
            Err(_) => Content::Gone,
        };
    }
    let Ok(bytes) = std::fs::read(path) else { return Content::Gone };
    let sha = sha256(&bytes);
    if bytes.contains(&0) {
        return Content::Bytes { sha };
    }
    match String::from_utf8(bytes) {
        Ok(text) => Content::Text { sha, text },
        Err(error) => {
            // A few bad bytes show as � in viewers, so they're placed so too.
            let bytes = error.into_bytes();
            let text = String::from_utf8_lossy(&bytes).into_owned();
            Content::Text { sha, text }
        }
    }
}

fn is_markdown(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    lower.ends_with(".md") || lower.ends_with(".markdown")
}

/// `path` as threads keep it: "~" expanded, links resolved.
fn absolute(path: &str) -> Result<String, RequestError> {
    let path = path.trim();
    if path.is_empty() {
        return Err(RequestError::Empty("path"));
    }
    let home = std::env::home_dir().unwrap_or_else(|| PathBuf::from("/"));
    let expanded = match path {
        "~" => home,
        _ => path.strip_prefix("~/").map_or_else(|| PathBuf::from(path), |rest| home.join(rest)),
    };
    if !expanded.is_absolute() {
        return Err(RequestError::RelativePath(path.to_owned()));
    }
    Ok(std::fs::canonicalize(&expanded).unwrap_or(expanded).display().to_string())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Text,
    Markdown,
    Image,
    Video,
}

impl Kind {
    fn parse(kind: &str) -> Result<Kind, RequestError> {
        match kind {
            "text" => Ok(Kind::Text),
            "markdown" => Ok(Kind::Markdown),
            "image" => Ok(Kind::Image),
            "video" => Ok(Kind::Video),
            other => Err(RequestError::Comments(format!("kind is \"text\", \"markdown\", \"image\" or \"video\"; got \"{other}\""))),
        }
    }

    fn name(self) -> &'static str {
        match self {
            Kind::Text => "text",
            Kind::Markdown => "markdown",
            Kind::Image => "image",
            Kind::Video => "video",
        }
    }

    fn is_text(self) -> bool {
        matches!(self, Kind::Text | Kind::Markdown)
    }

    fn space(self) -> Space {
        if self == Kind::Markdown { Space::Rendered } else { Space::Source }
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
struct QuoteJson {
    exact: String,
    #[serde(default)]
    prefix: String,
    #[serde(default)]
    suffix: String,
}

impl QuoteJson {
    fn of(quote: &Quote) -> QuoteJson {
        QuoteJson { exact: anchor::string(&quote.exact), prefix: anchor::string(&quote.prefix), suffix: anchor::string(&quote.suffix) }
    }

    fn quote(&self) -> Quote {
        Quote::new(&self.exact, &self.prefix, &self.suffix)
    }
}

/// A thread as kept.
#[derive(Clone, Debug)]
struct Thread {
    id: i64,
    agent: String,
    path: String,
    kind: Kind,
    version: String,
    range: Option<Range<usize>>,
    quote: QuoteJson,
    selectors: String,
    resolved: bool,
    anchor: v1::AnchorState,
    original: String,
    created: i64,
    updated: i64,
    suggestion: Option<Suggestion>,
}

/// A replacement proposed for a thread's text.
#[derive(Clone, Debug)]
struct Suggestion {
    replacement: String,
    /// An agent's id; empty for the person.
    by: String,
    decision: v1::SuggestionDecision,
}

const THREAD_COLUMNS: &str = "id, agent, path, kind, version, range_start, range_end, quote, selectors, status, anchor, original, created, updated, suggestion, suggested_by, decision";

fn anchor_name(anchor: v1::AnchorState) -> &'static str {
    match anchor {
        v1::AnchorState::Moved => "moved",
        v1::AnchorState::Detached => "detached",
        v1::AnchorState::Outdated => "outdated",
        v1::AnchorState::Anchored | v1::AnchorState::Unspecified => "anchored",
    }
}

fn anchor_of(name: &str) -> v1::AnchorState {
    match name {
        "moved" => v1::AnchorState::Moved,
        "detached" => v1::AnchorState::Detached,
        "outdated" => v1::AnchorState::Outdated,
        _ => v1::AnchorState::Anchored,
    }
}

impl Thread {
    fn read(row: &rusqlite::Row<'_>) -> rusqlite::Result<Thread> {
        let start: Option<i64> = row.get(5)?;
        let end: Option<i64> = row.get(6)?;
        let quote: String = row.get(7)?;
        let kind: String = row.get(3)?;
        let status: String = row.get(9)?;
        let anchor: String = row.get(10)?;
        Ok(Thread {
            id: row.get(0)?,
            agent: row.get(1)?,
            path: row.get(2)?,
            kind: Kind::parse(&kind).unwrap_or(Kind::Text),
            version: row.get(4)?,
            range: start.zip(end).map(|(start, end)| start.max(0) as usize..end.max(start).max(0) as usize),
            quote: serde_json::from_str(&quote).unwrap_or_default(),
            selectors: row.get(8)?,
            resolved: status == "resolved",
            anchor: anchor_of(&anchor),
            original: row.get(11)?,
            created: row.get(12)?,
            updated: row.get(13)?,
            suggestion: row.get::<_, Option<String>>(14)?.map(|replacement| Suggestion {
                replacement,
                by: row.get::<_, Option<String>>(15).ok().flatten().unwrap_or_default(),
                decision: match row.get::<_, Option<String>>(16).ok().flatten().as_deref() {
                    Some("accepted") => v1::SuggestionDecision::Accepted,
                    Some("rejected") => v1::SuggestionDecision::Rejected,
                    _ => v1::SuggestionDecision::Unspecified,
                },
            }),
        })
    }

    fn name(&self) -> String {
        format!("t-{}", self.id)
    }

    fn is_text(&self) -> bool {
        self.kind.is_text()
    }
}

#[derive(Clone, Debug)]
struct Comment {
    id: i64,
    thread: i64,
    author: String,
    body: String,
    created: i64,
    edited: Option<i64>,
    draft: bool,
}

impl Comment {
    fn read(row: &rusqlite::Row<'_>) -> rusqlite::Result<Comment> {
        Ok(Comment {
            id: row.get(0)?,
            thread: row.get(1)?,
            author: row.get(2)?,
            body: row.get(3)?,
            created: row.get(4)?,
            edited: row.get(5)?,
            draft: row.get::<_, i64>(6)? != 0,
        })
    }

    fn to_proto(&self) -> v1::ThreadComment {
        v1::ThreadComment {
            id: format!("c-{}", self.id),
            author_agent_id: self.author.clone(),
            body: self.body.clone(),
            created_ms: self.created,
            edited_ms: self.edited.unwrap_or(0),
            draft: self.draft,
        }
    }
}

const COMMENT_COLUMNS: &str = "id, thread, author, body, created, edited, draft";

/// "t-41" (or "41") as a thread's row id.
fn thread_number(id: &str) -> Option<i64> {
    let id = id.trim();
    id.strip_prefix("t-").unwrap_or(id).parse().ok()
}

fn comment_number(id: &str) -> Option<i64> {
    let id = id.trim();
    id.strip_prefix("c-").unwrap_or(id).parse().ok()
}

fn thread(db: &Connection, id: i64) -> rusqlite::Result<Option<Thread>> {
    db.query_row(&format!("SELECT {THREAD_COLUMNS} FROM threads WHERE id = ?1"), [id], Thread::read).optional()
}

fn threads_on(db: &Connection, path: &str) -> rusqlite::Result<Vec<Thread>> {
    let mut statement = db.prepare(&format!("SELECT {THREAD_COLUMNS} FROM threads WHERE path = ?1 ORDER BY id"))?;
    statement.query_map([path], Thread::read)?.collect()
}

fn comments_of(db: &Connection, thread: i64) -> rusqlite::Result<Vec<Comment>> {
    let mut statement = db.prepare(&format!("SELECT {COMMENT_COLUMNS} FROM thread_comments WHERE thread = ?1 ORDER BY id"))?;
    statement.query_map([thread], Comment::read)?.collect()
}

fn has_threads(db: &Connection, path: &str) -> rusqlite::Result<bool> {
    db.query_row("SELECT EXISTS (SELECT 1 FROM threads WHERE path = ?1)", [path], |row| row.get(0))
}

fn snapshot(db: &Connection, path: &str) -> rusqlite::Result<Option<(String, String)>> {
    db.query_row("SELECT sha, text FROM thread_files WHERE path = ?1", [path], crate::store::row2).optional()
}

fn save_snapshot(db: &Connection, path: &str, sha: &str, text: &str) -> rusqlite::Result<()> {
    db.execute(
        "INSERT INTO thread_files (path, sha, text) VALUES (?1, ?2, ?3) ON CONFLICT (path) DO UPDATE SET sha = excluded.sha, text = excluded.text",
        params![path, sha, text],
    )?;
    Ok(())
}

fn save_place(db: &Connection, thread: &Thread) -> rusqlite::Result<()> {
    let (start, end) = thread.range.clone().map_or((None, None), |range| (Some(range.start as i64), Some(range.end as i64)));
    db.execute(
        "UPDATE threads SET version = ?2, range_start = ?3, range_end = ?4, quote = ?5, anchor = ?6, original = ?7 WHERE id = ?1",
        params![
            thread.id,
            thread.version,
            start,
            end,
            serde_json::to_string(&thread.quote).unwrap_or_default(),
            anchor_name(thread.anchor),
            thread.original
        ],
    )?;
    Ok(())
}

fn touch(db: &Connection, thread: i64) -> rusqlite::Result<()> {
    db.execute("UPDATE threads SET updated = ?2 WHERE id = ?1", params![thread, now_ms()])?;
    Ok(())
}

/// Places the file's threads on its text as it is now, if it changed since
/// they were placed. Returns its content, and whether any thread moved.
fn refresh(db: &Connection, path: &str) -> rusqlite::Result<(Content, bool)> {
    let content = read(Path::new(path));
    let threads = threads_on(db, path)?;
    let mut changed = false;
    if let Some(sha) = content.sha() {
        // An image's or video's thread is on the version it was made on:
        // outdated while the file is another, and anchored again when it's
        // the same once more (rewritten identically, or caught mid-write).
        for mut thread in threads.iter().filter(|thread| !thread.is_text()).cloned() {
            let anchor = if thread.version == sha { v1::AnchorState::Anchored } else { v1::AnchorState::Outdated };
            if thread.anchor != anchor {
                thread.anchor = anchor;
                save_place(db, &thread)?;
                changed = true;
            }
        }
    }
    let Content::Text { sha, text } = &content else { return Ok((content, changed)) };
    let texts: Vec<Thread> = threads.into_iter().filter(Thread::is_text).collect();
    if texts.is_empty() {
        db.execute("DELETE FROM thread_files WHERE path = ?1", [path])?;
        return Ok((content, changed));
    }
    let Some((old_sha, old_text)) = snapshot(db, path)? else {
        save_snapshot(db, path, sha, text)?;
        return Ok((content, changed));
    };
    if old_sha == *sha {
        return Ok((content, changed));
    }
    let markdown = is_markdown(path);
    let old = Doc::new(&old_text, markdown);
    let new = Doc::new(text, markdown);
    let mapping = Mapping::new(&old.source, &new.source);
    for mut thread in texts {
        let space = thread.kind.space();
        let quote = thread.quote.quote();
        match new.follow(&mapping, space, thread.range.as_ref(), &quote) {
            Some((range, _)) => {
                let now = new.quote(space, &range).unwrap_or(quote);
                thread.anchor =
                    if anchor::units(&thread.original) == now.exact { v1::AnchorState::Anchored } else { v1::AnchorState::Moved };
                thread.quote = QuoteJson::of(&now);
                thread.range = Some(range);
            }
            None => {
                thread.anchor = v1::AnchorState::Detached;
                thread.range = None;
            }
        }
        thread.version.clone_from(sha);
        save_place(db, &thread)?;
    }
    save_snapshot(db, path, sha, text)?;
    Ok((content, true))
}

fn text_anchor(text: &[u16], range: &Range<usize>, lines: Option<(usize, usize)>) -> v1::TextAnchor {
    let quote = Quote::of(text, range);
    let (first_line, last_line) = lines.unwrap_or((0, 0));
    v1::TextAnchor {
        start: range.start as i32,
        end: range.end as i32,
        exact: anchor::string(&quote.exact),
        prefix: anchor::string(&quote.prefix),
        suffix: anchor::string(&quote.suffix),
        first_line: first_line as i32,
        last_line: last_line as i32,
    }
}

fn selector(json: Value) -> v1::Selector {
    v1::Selector { json: json.to_string() }
}

/// A thread for clients, placed on `doc` (the file's text now, which its
/// place refers to; None when the file is gone).
fn to_proto(db: &Connection, thread: &Thread, doc: Option<&Doc>) -> rusqlite::Result<v1::Thread> {
    let comments = comments_of(db, thread.id)?.iter().map(Comment::to_proto).collect();
    let mut proto = v1::Thread {
        id: thread.name(),
        agent_id: thread.agent.clone(),
        path: thread.path.clone(),
        kind: thread.kind.name().to_owned(),
        version_sha256: thread.version.clone(),
        status: if thread.resolved { v1::ThreadStatus::Resolved } else { v1::ThreadStatus::Open }.into(),
        anchor: thread.anchor.into(),
        original_exact: thread.original.clone(),
        comments,
        created_ms: thread.created,
        updated_ms: thread.updated,
        suggestion: thread.suggestion.as_ref().map(|suggestion| v1::Suggestion {
            replacement: suggestion.replacement.clone(),
            author_agent_id: suggestion.by.clone(),
            decision: suggestion.decision.into(),
        }),
        ..v1::Thread::default()
    };
    if !thread.is_text() {
        let selectors: Vec<Value> = serde_json::from_str(&thread.selectors).unwrap_or_default();
        proto.selectors = selectors.into_iter().map(selector).collect();
        return Ok(proto);
    }
    let placed = doc.zip(thread.range.clone()).filter(|(doc, range)| range.end <= doc.source.len());
    let Some((doc, range)) = placed else {
        proto.anchor = v1::AnchorState::Detached.into();
        proto.selectors = vec![selector(
            json!({"type": "TextQuoteSelector", "exact": thread.quote.exact, "prefix": thread.quote.prefix, "suffix": thread.quote.suffix}),
        )];
        return Ok(proto);
    };
    proto.source = Some(text_anchor(&doc.source, &range, Some(doc.lines(&range))));
    if let Some(projection) = &doc.projection
        && let Some(rendered) = projection.shown_from(&range)
    {
        proto.rendered = Some(text_anchor(&projection.text, &rendered, None));
    }
    let space = thread.kind.space();
    let own = if space == Space::Rendered { proto.rendered.clone() } else { proto.source.clone() };
    if let Some(own) = own {
        proto.selectors = vec![
            selector(json!({"type": "TextQuoteSelector", "exact": own.exact, "prefix": own.prefix, "suffix": own.suffix})),
            selector(json!({"type": "TextPositionSelector", "start": own.start, "end": own.end})),
        ];
    }
    Ok(proto)
}

/// Every thread on a file, placed on its text as it is now.
fn list(db: &Connection, path: &str) -> rusqlite::Result<(v1::ThreadList, bool)> {
    let (content, changed) = refresh(db, path)?;
    let doc = match &content {
        Content::Text { text, .. } => Some(Doc::new(text, is_markdown(path))),
        _ => None,
    };
    let threads = threads_on(db, path)?.iter().map(|thread| to_proto(db, thread, doc.as_ref())).collect::<rusqlite::Result<_>>()?;
    let list = v1::ThreadList { path: path.to_owned(), version_sha256: content.sha().unwrap_or_default().to_owned(), threads };
    Ok((list, changed))
}

impl Factory {
    fn comments_lock(&self) -> MutexGuard<'_, ()> {
        guard(&self.annotations.work)
    }

    /// Tells every client of the threads on `path` as they are now.
    fn broadcast_threads(&self, db: &Connection, path: &str) -> rusqlite::Result<()> {
        let (list, _) = list(db, path)?;
        self.send(Body::ThreadList(list));
        Ok(())
    }

    /// ListThreads.
    pub(crate) fn list_threads(&self, path: &str) -> Result<v1::ThreadList, RequestError> {
        let path = absolute(path)?;
        let _work = self.comments_lock();
        let db = self.store.db();
        let (list, changed) = list(&db, &path)?;
        if changed {
            self.send(Body::ThreadList(list.clone()));
        }
        Ok(list)
    }

    /// CreateThread: the person starts a thread.
    pub(crate) fn create_thread(self: &Arc<Self>, request: &v1::CreateThread) -> Result<v1::Thread, RequestError> {
        self.known_agent(&request.agent_id)?;
        let body = request.body.trim();
        // A suggested change needs no words: its comment may be empty.
        if body.is_empty() && request.suggestion.is_none() {
            return Err(RequestError::Empty("body"));
        }
        let kind = Kind::parse(&request.kind)?;
        if request.suggestion.is_some() && !kind.is_text() {
            return Err(RequestError::Comments("Changes can be suggested only for text.".to_owned()));
        }
        let path = absolute(&request.path)?;
        let (thread, comment) = {
            let _work = self.comments_lock();
            let db = self.store.db();
            let now = now_ms();
            let place = place(&db, &path, kind, &request.version_sha256, &request.selectors)?;
            let (start, end) = place.range.clone().map_or((None, None), |range| (Some(range.start as i64), Some(range.end as i64)));
            db.execute(
                "INSERT INTO threads (agent, path, kind, version, range_start, range_end, quote, selectors, status, anchor, original, created, updated)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 'open', ?9, ?10, ?11, ?11)",
                params![
                    request.agent_id,
                    path,
                    kind.name(),
                    place.version,
                    start,
                    end,
                    serde_json::to_string(&place.quote).unwrap_or_default(),
                    place.selectors,
                    anchor_name(place.anchor),
                    place.quote.exact,
                    now
                ],
            )?;
            let thread = db.last_insert_rowid();
            if let Some(suggestion) = &request.suggestion {
                db.execute(
                    "UPDATE threads SET suggestion = ?2, suggested_by = '', decision = '' WHERE id = ?1",
                    params![thread, suggestion],
                )?;
            }
            db.execute(
                "INSERT INTO thread_comments (thread, author, body, created, draft) VALUES (?1, '', ?2, ?3, ?4)",
                params![thread, body, now, !request.send_now],
            )?;
            let comment = db.last_insert_rowid();
            let (list, _) = list(&db, &path)?;
            self.send(Body::ThreadList(list.clone()));
            let made = list.threads.into_iter().find(|made| made.id == format!("t-{thread}"));
            (made.unwrap_or_default(), comment)
        };
        if request.send_now {
            self.send_comments_now(&request.agent_id, &[comment])?;
        }
        Ok(thread)
    }

    /// ReattachThread: the person puts a thread on new text (or a region).
    pub(crate) fn reattach_thread(&self, request: &v1::ReattachThread) -> Result<(), RequestError> {
        let kind = Kind::parse(&request.kind)?;
        let _work = self.comments_lock();
        let db = self.store.db();
        let mut thread = find_thread(&db, &request.thread_id)?;
        let place = place(&db, &thread.path, kind, &request.version_sha256, &request.selectors)?;
        thread.kind = kind;
        thread.version = place.version;
        thread.range = place.range;
        thread.original.clone_from(&place.quote.exact);
        thread.quote = place.quote;
        thread.anchor = place.anchor;
        save_place(&db, &thread)?;
        db.execute("UPDATE threads SET kind = ?2, selectors = ?3 WHERE id = ?1", params![thread.id, kind.name(), place.selectors])?;
        touch(&db, thread.id)?;
        self.broadcast_threads(&db, &thread.path)?;
        Ok(())
    }

    /// ReplyThread: the person replies (and so reopens it).
    pub(crate) fn reply_thread(self: &Arc<Self>, request: &v1::ReplyThread) -> Result<(), RequestError> {
        let body = request.body.trim();
        if body.is_empty() {
            return Err(RequestError::Empty("body"));
        }
        let (agent, comment) = {
            let _work = self.comments_lock();
            let db = self.store.db();
            let thread = find_thread(&db, &request.thread_id)?;
            db.execute(
                "INSERT INTO thread_comments (thread, author, body, created, draft) VALUES (?1, '', ?2, ?3, ?4)",
                params![thread.id, body, now_ms(), !request.send_now],
            )?;
            let comment = db.last_insert_rowid();
            db.execute("UPDATE threads SET status = 'open' WHERE id = ?1", [thread.id])?;
            touch(&db, thread.id)?;
            self.broadcast_threads(&db, &thread.path)?;
            (thread.agent, comment)
        };
        if request.send_now {
            self.send_comments_now(&agent, &[comment])?;
        }
        Ok(())
    }

    /// One of the person's comments, and its thread.
    fn own_comment(db: &Connection, id: &str) -> Result<(Comment, Thread), RequestError> {
        let unknown = || RequestError::UnknownComment(id.to_owned());
        let number = comment_number(id).ok_or_else(unknown)?;
        let comment = db
            .query_row(&format!("SELECT {COMMENT_COLUMNS} FROM thread_comments WHERE id = ?1"), [number], Comment::read)
            .optional()?
            .ok_or_else(unknown)?;
        if !comment.author.is_empty() {
            return Err(RequestError::Comments("Only your own comments can be changed.".to_owned()));
        }
        let thread = thread(db, comment.thread)?.ok_or_else(unknown)?;
        Ok((comment, thread))
    }

    /// EditComment.
    pub(crate) fn edit_comment(&self, request: &v1::EditComment) -> Result<(), RequestError> {
        let body = request.body.trim();
        if body.is_empty() {
            return Err(RequestError::Empty("body"));
        }
        let _work = self.comments_lock();
        let db = self.store.db();
        let (comment, thread) = Self::own_comment(&db, &request.comment_id)?;
        db.execute("UPDATE thread_comments SET body = ?2, edited = ?3 WHERE id = ?1", params![comment.id, body, now_ms()])?;
        touch(&db, thread.id)?;
        self.broadcast_threads(&db, &thread.path)?;
        Ok(())
    }

    /// DeleteComment: a thread left with none goes too.
    pub(crate) fn delete_comment(&self, request: &v1::DeleteComment) -> Result<(), RequestError> {
        let _work = self.comments_lock();
        let db = self.store.db();
        let (comment, thread) = Self::own_comment(&db, &request.comment_id)?;
        db.execute("DELETE FROM thread_comments WHERE id = ?1", [comment.id])?;
        if comments_of(&db, thread.id)?.is_empty() {
            db.execute("DELETE FROM threads WHERE id = ?1", [thread.id])?;
        } else {
            touch(&db, thread.id)?;
        }
        self.broadcast_threads(&db, &thread.path)?;
        Ok(())
    }

    /// SetThreadStatus.
    pub(crate) fn set_thread_status(&self, request: &v1::SetThreadStatus) -> Result<(), RequestError> {
        let _work = self.comments_lock();
        let db = self.store.db();
        let thread = find_thread(&db, &request.thread_id)?;
        let status = if request.resolved { "resolved" } else { "open" };
        db.execute("UPDATE threads SET status = ?2 WHERE id = ?1", params![thread.id, status])?;
        touch(&db, thread.id)?;
        self.broadcast_threads(&db, &thread.path)?;
        Ok(())
    }

    /// ReaddressThread.
    pub(crate) fn readdress_thread(&self, request: &v1::ReaddressThread) -> Result<(), RequestError> {
        self.known_agent(&request.agent_id)?;
        let _work = self.comments_lock();
        let db = self.store.db();
        let thread = find_thread(&db, &request.thread_id)?;
        db.execute("UPDATE threads SET agent = ?2 WHERE id = ?1", params![thread.id, request.agent_id])?;
        touch(&db, thread.id)?;
        self.broadcast_threads(&db, &thread.path)?;
        Ok(())
    }

    /// DecideSuggestion: the person accepts a suggested change (Claktory
    /// makes it in the file) or rejects it; either resolves the thread.
    pub(crate) fn decide_suggestion(self: &Arc<Self>, request: &v1::DecideSuggestion) -> Result<(), RequestError> {
        let person = self.lock().config.person_capitalized().to_owned();
        let tell = {
            let _work = self.comments_lock();
            let db = self.store.db();
            let found = find_thread(&db, &request.thread_id)?;
            let Some(suggestion) = found.suggestion.clone() else {
                return Err(RequestError::Comments("This comment doesn't suggest a change.".to_owned()));
            };
            if suggestion.decision != v1::SuggestionDecision::Unspecified {
                return Err(RequestError::Comments("This change was already accepted or rejected.".to_owned()));
            }
            let (content, _) = refresh(&db, &found.path)?;
            let thread = thread(&db, found.id)?.unwrap_or(found);
            let before = thread.quote.exact.clone();
            if request.accept {
                self.make_change(&db, &thread, content, &suggestion.replacement)?;
            }
            let sent = comments_of(&db, thread.id)?.iter().any(|comment| !comment.draft);
            let decision = if request.accept { "accepted" } else { "rejected" };
            db.execute("UPDATE threads SET decision = ?2, status = 'resolved' WHERE id = ?1", params![thread.id, decision])?;
            // Its unsent comments are kept with it, never sent: the change
            // is decided, so they'd only tell the agent of it as if new.
            db.execute("UPDATE thread_comments SET draft = 0 WHERE thread = ?1 AND draft = 1", [thread.id])?;
            touch(&db, thread.id)?;
            self.broadcast_threads(&db, &thread.path)?;
            let change = format!("“{}” → “{}”", shown(&before), shown(&suggestion.replacement));
            let file = file_name(&thread.path);
            match (suggestion.by.is_empty(), request.accept) {
                // The agent's suggestion: it hears what became of it.
                (false, true) => Some((
                    suggestion.by.clone(),
                    format!("{person} accepted your suggested change {} on {file}.", thread.name()),
                    format!("{change}\nThe file has it now: {}", thread.path),
                )),
                (false, false) => Some((
                    suggestion.by.clone(),
                    format!("{person} rejected your suggested change {} on {file}.", thread.name()),
                    format!("{change}\nThe file is as it was: {}", thread.path),
                )),
                // Their own, already sent: the agent mustn't make it again.
                (true, true) if sent => Some((
                    thread.agent.clone(),
                    format!("{person} made the change they suggested in {} on {file} themselves.", thread.name()),
                    format!("{change}\nThe file has it now: {}", thread.path),
                )),
                _ => None,
            }
        };
        if let Some((agent, summary, details)) = tell
            && self.lock().config.agent(&agent).is_some()
        {
            let text = format!("{summary}\n{details}");
            let event = host_event(summary);
            self.deliver_event(&agent, text, event);
        }
        Ok(())
    }

    /// Replaces a thread's text in its file with `replacement`, if the text
    /// is still the one it was made on; the thread then holds the new text.
    fn make_change(&self, db: &Connection, thread: &Thread, content: Content, replacement: &str) -> Result<(), RequestError> {
        let changed = || RequestError::Comments("The text changed after this was suggested.".to_owned());
        let Content::Text { text, .. } = content else { return Err(changed()) };
        let source = anchor::units(&text);
        let range = match (&thread.range, thread.anchor) {
            (Some(range), v1::AnchorState::Anchored) if range.end <= source.len() => range.clone(),
            _ => return Err(changed()),
        };
        let added = anchor::units(replacement);
        let mut new = source[..range.start].to_vec();
        new.extend_from_slice(&added);
        new.extend_from_slice(&source[range.end..]);
        let new = anchor::string(&new);
        write_atomic(Path::new(&thread.path), &new)?;
        // The other threads follow the change; this one is on the new text.
        refresh(db, &thread.path)?;
        let mut placed = self::thread(db, thread.id)?.unwrap_or_else(|| thread.clone());
        let doc = Doc::new(&new, is_markdown(&thread.path));
        let range = range.start..range.start + added.len();
        match doc.quote(placed.kind.space(), &range) {
            Some(quote) => {
                placed.original = anchor::string(&quote.exact);
                placed.quote = QuoteJson::of(&quote);
                placed.range = Some(range);
                placed.anchor = v1::AnchorState::Anchored;
            }
            // Deleted: nothing left to show it on.
            None => {
                placed.range = None;
                placed.anchor = v1::AnchorState::Detached;
            }
        }
        save_place(db, &placed)?;
        let sha = sha256(new.as_bytes());
        guard(&self.annotations.seen).entry(thread.path.clone()).and_modify(|seen| {
            seen.stamp = stamp(Path::new(&thread.path));
            seen.sha.clone_from(&sha);
        });
        self.send(Body::FileChanged(v1::FileChanged { path: thread.path.clone(), sha256: sha, by_agent_id: String::new() }));
        Ok(())
    }

    /// SendComments: the person's drafts on a file (or every file) to the
    /// agent they're addressed to.
    pub(crate) fn send_comments(self: &Arc<Self>, request: &v1::SendComments) -> Result<(), RequestError> {
        self.known_agent(&request.agent_id)?;
        let path = if request.path.trim().is_empty() { None } else { Some(absolute(&request.path)?) };
        let drafts: Vec<i64> = {
            let db = self.store.db();
            let mut statement = db.prepare(
                "SELECT c.id FROM thread_comments c JOIN threads t ON t.id = c.thread
                 WHERE c.draft = 1 AND t.agent = ?1 AND (?2 IS NULL OR t.path = ?2) ORDER BY c.id",
            )?;
            statement.query_map(params![request.agent_id, path], |row| row.get(0))?.collect::<rusqlite::Result<_>>()?
        };
        if drafts.is_empty() {
            return Err(RequestError::Comments("There are no comments to send.".to_owned()));
        }
        self.send_comments_now(&request.agent_id, &drafts)
    }

    /// Gives `agent` the person's comments `ids` as one event, and marks
    /// them sent.
    fn send_comments_now(self: &Arc<Self>, agent: &str, ids: &[i64]) -> Result<(), RequestError> {
        let (person, tools) = {
            let inner = self.lock();
            let Some(config) = inner.config.agent(agent) else { return Err(RequestError::UnknownAgent(agent.to_owned())) };
            (inner.config.person_capitalized().to_owned(), crate::extensions::enabled(config, crate::extensions::comments::ID))
        };
        let (text, summary) = {
            let _work = self.comments_lock();
            let db = self.store.db();
            let mut by_thread: BTreeMap<i64, Vec<Comment>> = BTreeMap::new();
            for id in ids {
                let comment = db.query_row(&format!("SELECT {COMMENT_COLUMNS} FROM thread_comments WHERE id = ?1"), [id], Comment::read)?;
                by_thread.entry(comment.thread).or_default().push(comment);
            }
            let mut paths = BTreeSet::new();
            let mut docs: HashMap<String, Option<Doc>> = HashMap::new();
            let mut sections = Vec::new();
            for (number, comments) in &by_thread {
                let Some(thread) = thread(&db, *number)? else { continue };
                if !docs.contains_key(&thread.path) {
                    let (content, _) = refresh(&db, &thread.path)?;
                    let doc = match content {
                        Content::Text { text, .. } => Some(Doc::new(&text, is_markdown(&thread.path))),
                        _ => None,
                    };
                    docs.insert(thread.path.clone(), doc);
                }
                let thread = self::thread(&db, *number)?.unwrap_or(thread);
                let first = comments_of(&db, thread.id)?.first().map(|comment| comment.id);
                sections.push(event_section(&thread, docs.get(&thread.path).and_then(Option::as_ref), comments, first));
                paths.insert(thread.path.clone());
            }
            for id in ids {
                db.execute("UPDATE thread_comments SET draft = 0 WHERE id = ?1", [id])?;
            }
            for path in &paths {
                self.broadcast_threads(&db, path)?;
            }
            let count = ids.len();
            let names: Vec<String> = paths.iter().map(|path| file_name(path)).collect();
            let place = match names.as_slice() {
                [one] => one.clone(),
                many => format!("{} files", many.len()),
            };
            let summary = match count {
                1 => format!("{person} commented on {place}."),
                _ => format!("{person} left {count} comments on {place}."),
            };
            let mut text = format!("{summary}\n");
            for section in sections {
                text.push('\n');
                text.push_str(&section);
            }
            if tools {
                text.push_str("\nAnswer on a thread with comments_reply and its id, and close it with comments_resolve once it's dealt with. Edit the file with your usual tools: the comments stay with their text.");
            }
            (text, summary)
        };
        let event = host_event(summary);
        self.deliver_event(agent, text, event);
        Ok(())
    }
}

/// Writes a file whole or not at all, keeping its permissions.
fn write_atomic(path: &Path, text: &str) -> std::io::Result<()> {
    let name = path.file_name().map_or_else(|| "file".to_owned(), |name| name.to_string_lossy().into_owned());
    let temporary = path.with_file_name(format!(".{name}.claktory-{}", std::process::id()));
    std::fs::write(&temporary, text)?;
    if let Ok(metadata) = std::fs::metadata(path) {
        let _ = std::fs::set_permissions(&temporary, metadata.permissions());
    }
    std::fs::rename(&temporary, path).inspect_err(|_| {
        let _ = std::fs::remove_file(&temporary);
    })
}

fn file_name(path: &str) -> String {
    Path::new(path).file_name().map_or_else(|| path.to_owned(), |name| name.to_string_lossy().into_owned())
}

/// A quote as an event shows it: on one line, cut short.
fn shown(quote: &str) -> String {
    let line = anchor::collapse(quote);
    let line = line.trim();
    if line.chars().count() <= QUOTE_SHOWN {
        return line.to_owned();
    }
    let cut: String = line.chars().take(QUOTE_SHOWN).collect();
    format!("{}…", cut.trim_end())
}

/// Where a thread is, as agents read it: "essay.md, lines 12–14".
fn location(thread: &Thread, doc: Option<&Doc>) -> String {
    let lines = doc.zip(thread.range.clone()).filter(|(doc, range)| range.end <= doc.source.len()).map(|(doc, range)| doc.lines(&range));
    match lines {
        Some((first, last)) if first == last => format!("{}, line {first}", thread.path),
        Some((first, last)) => format!("{}, lines {first}–{last}", thread.path),
        None if thread.is_text() => format!("{} (no longer in the file)", thread.path),
        None => thread.path.clone(),
    }
}

/// Where an image's or a video's thread is, in words: "a region of the
/// image, 30% wide and 20% tall, …" or "a spot on the image, 62% across and
/// 30% down", with pixels while the image is the one it was made on; "at
/// 0:31–0:45, in a region of the frame, …" (or "on a spot in the frame") in
/// a video.
fn media_place(thread: &Thread) -> String {
    if thread.kind == Kind::Video {
        let region = region::describe(&thread.selectors, None).map(|place| match place.spot {
            true => format!(", on a spot in the frame, {}", place.words),
            false => format!(", in a region of the frame, {}", place.words),
        });
        return match region::when(&thread.selectors) {
            Some(when) => format!("{when}{}", region.unwrap_or_default()),
            None => format!("a part of the video: {}", thread.selectors),
        };
    }
    let size = match thread.anchor {
        v1::AnchorState::Outdated => None,
        _ => std::fs::read(&thread.path).ok().and_then(|bytes| region::size(&bytes)),
    };
    match region::describe(&thread.selectors, size) {
        Some(place) if place.spot => format!("a spot on the image, {}", place.words),
        Some(place) => format!("a region of the image, {}", place.words),
        None => format!("a region of the image: {}", thread.selectors),
    }
}

/// What a thread that isn't on text is on: "image" or "video".
fn media(thread: &Thread) -> &'static str {
    if thread.kind == Kind::Video { "video" } else { "image" }
}

/// One thread's part of the event: where it is, what it quotes, and the
/// person's new comments on it.
fn event_section(thread: &Thread, doc: Option<&Doc>, comments: &[Comment], first: Option<i64>) -> String {
    let mut section = format!("[{}] {}", thread.name(), location(thread, doc));
    if thread.is_text() {
        let _ = write!(section, ": “{}”", shown(&thread.quote.exact));
        if thread.anchor == v1::AnchorState::Moved {
            let _ = write!(section, " (the text changed since; it was “{}”)", shown(&thread.original));
        }
    } else {
        let _ = write!(section, ": {}", media_place(thread));
        if thread.anchor == v1::AnchorState::Outdated {
            let _ = write!(section, " (on an earlier version of the {})", media(thread));
        }
    }
    if let Some(suggestion) = thread.suggestion.as_ref().filter(|suggestion| suggestion.by.is_empty()) {
        let _ = write!(section, ", suggesting “{}” instead", shown(&suggestion.replacement));
    }
    section.push('\n');
    for comment in comments.iter().filter(|comment| !comment.body.trim().is_empty()) {
        if Some(comment.id) != first {
            section.push_str("Reply: ");
        }
        section.push_str(comment.body.trim());
        section.push('\n');
    }
    section
}

/// Where a new or reattached thread goes.
struct Place {
    version: String,
    range: Option<Range<usize>>,
    quote: QuoteJson,
    selectors: String,
    anchor: v1::AnchorState,
}

fn find_thread(db: &Connection, id: &str) -> Result<Thread, RequestError> {
    let unknown = || RequestError::UnknownThread(id.to_owned());
    let number = thread_number(id).ok_or_else(unknown)?;
    thread(db, number)?.ok_or_else(unknown)
}

#[derive(Deserialize)]
struct SelectorJson {
    #[serde(rename = "type")]
    kind: String,
    #[serde(default)]
    exact: String,
    #[serde(default)]
    prefix: String,
    #[serde(default)]
    suffix: String,
    #[serde(default)]
    start: Option<usize>,
    #[serde(default)]
    end: Option<usize>,
}

/// Places a selection made on the file's version `version` on its text as
/// it is now: where it was made if the file is the same and it's there,
/// else wherever its quote is.
fn place(db: &Connection, path: &str, kind: Kind, version: &str, selectors: &[v1::Selector]) -> Result<Place, RequestError> {
    let parsed: Vec<Value> = selectors
        .iter()
        .map(|selector| serde_json::from_str(&selector.json))
        .collect::<Result<_, _>>()
        .map_err(|error| RequestError::Comments(format!("a selector isn't JSON: {error}")))?;
    let (content, _) = refresh(db, path)?;
    if !kind.is_text() {
        let Some(sha) = content.sha() else { return Err(RequestError::Comments(format!("Can't read {path}."))) };
        if parsed.is_empty() {
            let needs = if kind == Kind::Video { "A video's comment needs a time." } else { "An image's comment needs a region." };
            return Err(RequestError::Comments(needs.to_owned()));
        }
        // Kept on the version it was made on, which may be an earlier one.
        let anchor = if version == sha { v1::AnchorState::Anchored } else { v1::AnchorState::Outdated };
        return Ok(Place {
            version: version.to_owned(),
            range: None,
            quote: QuoteJson::default(),
            selectors: Value::Array(parsed).to_string(),
            anchor,
        });
    }
    let Content::Text { sha, text } = content else {
        return Err(RequestError::Comments("Comments can be left on text files up to 2 MB.".to_owned()));
    };
    let mut quote = None;
    let mut position = None;
    for value in parsed {
        let Ok(selector) = serde_json::from_value::<SelectorJson>(value) else { continue };
        match selector.kind.as_str() {
            "TextQuoteSelector" | "TextQuote" => quote = Some(Quote::new(&selector.exact, &selector.prefix, &selector.suffix)),
            "TextPositionSelector" | "TextPosition" => position = selector.start.zip(selector.end).map(|(start, end)| start..end),
            _ => {}
        }
    }
    let Some(quote) = quote.filter(|quote| !quote.exact.is_empty()) else {
        return Err(RequestError::Comments("A comment on text needs a TextQuoteSelector.".to_owned()));
    };
    let space = kind.space();
    let quote = if space == Space::Rendered && is_markdown(path) { quote.collapsed() } else { quote };
    let doc = Doc::new(&text, is_markdown(path));
    // Made on this very text: its position, if the quote's there.
    let hint = position.filter(|_| version == sha);
    let Some((range, _)) = doc.place(space, &quote, hint.as_ref()) else { return Err(RequestError::Comments(CHANGED.to_owned())) };
    let placed = doc.quote(space, &range).unwrap_or(quote);
    if snapshot(db, path)?.is_none_or(|(old, _)| old != sha) {
        save_snapshot(db, path, &sha, &text)?;
    }
    Ok(Place {
        version: sha,
        range: Some(range),
        quote: QuoteJson::of(&placed),
        selectors: "[]".to_owned(),
        anchor: v1::AnchorState::Anchored,
    })
}

/// An agent wrote `path` (as its write_file or edit_file call gave it,
/// from `folder`): clients showing it read it again, and its threads follow
/// the text at once rather than at the next check.
pub(crate) fn written(factory: &Factory, agent: &str, folder: &Path, path: &str) {
    let path = Path::new(path);
    let path = if path.is_absolute() { path.to_path_buf() } else { folder.join(path) };
    let path = std::fs::canonicalize(&path).unwrap_or(path).display().to_string();
    let shown = ["text", "image", "video", "pdf"].iter().any(|kind| factory.viewer.paths(kind).contains(&path));
    let _work = factory.comments_lock();
    let db = factory.store.db();
    let threaded = has_threads(&db, &path).unwrap_or(false);
    if !shown && !threaded {
        return;
    }
    let stamp = stamp(Path::new(&path));
    let sha = match read(Path::new(&path)).sha() {
        Some(sha) => sha.to_owned(),
        None => String::new(),
    };
    {
        let mut seen = guard(&factory.annotations.seen);
        let now = Seen { stamp, sha: sha.clone() };
        if seen.get(&path).is_some_and(|before| before.sha == sha) {
            return;
        }
        if shown {
            seen.insert(path.clone(), now);
        }
    }
    factory.send(Body::FileChanged(v1::FileChanged { path: path.clone(), sha256: sha, by_agent_id: agent.to_owned() }));
    if threaded && let Err(error) = factory.broadcast_threads(&db, &path) {
        tracing::warn!(%error, %path, "could not place the comments on a file again");
    }
}

/// Checks the files shown in panes every second, and tells clients of the
/// ones that changed, with their threads placed again.
pub(crate) async fn monitor(factory: Weak<Factory>) {
    loop {
        tokio::time::sleep(POLL_EVERY).await;
        let Some(strong) = factory.upgrade() else { return };
        if tokio::task::spawn_blocking(move || poll(&strong)).await.is_err() {
            return;
        }
    }
}

fn poll(factory: &Factory) {
    let mut paths = factory.viewer.paths("text");
    paths.extend(factory.viewer.paths("image"));
    paths.extend(factory.viewer.paths("video"));
    paths.extend(factory.viewer.paths("pdf"));
    let mut changed = Vec::new();
    {
        let mut seen = guard(&factory.annotations.seen);
        seen.retain(|path, _| paths.contains(path));
        for path in paths {
            let stamp = stamp(Path::new(&path));
            let before = seen.get(&path);
            if before.is_some_and(|before| before.stamp == stamp) {
                continue;
            }
            let sha = read(Path::new(&path)).sha().unwrap_or_default().to_owned();
            if before.is_some_and(|before| before.sha != sha) {
                changed.push((path.clone(), sha.clone()));
            }
            seen.insert(path, Seen { stamp, sha });
        }
    }
    for (path, sha) in changed {
        factory.send(Body::FileChanged(v1::FileChanged { path: path.clone(), sha256: sha, by_agent_id: String::new() }));
        let _work = factory.comments_lock();
        let db = factory.store.db();
        if has_threads(&db, &path).unwrap_or(false)
            && let Err(error) = factory.broadcast_threads(&db, &path)
        {
            tracing::warn!(%error, %path, "could not place the comments on a file again");
        }
    }
}

// ---- For the Comments extension's tools ----

/// The threads addressed to `agent` that it can see (those with a comment
/// that isn't a draft), as it reads them.
pub(crate) fn agent_list(factory: &Factory, agent: &str, path: Option<&str>, status: &str) -> Result<String, String> {
    let (person, names) = {
        let inner = factory.lock();
        let names: HashMap<String, String> = inner.config.agents.iter().map(|agent| (agent.id.clone(), agent.name.clone())).collect();
        (inner.config.person_capitalized().to_owned(), names)
    };
    let _work = factory.comments_lock();
    let db = factory.store.db();
    let paths: Vec<String> = {
        let mut statement = db
            .prepare("SELECT DISTINCT path FROM threads WHERE agent = ?1 AND (?2 IS NULL OR path = ?2) ORDER BY path")
            .map_err(|error| error.to_string())?;
        let rows = statement.query_map(params![agent, path], |row| row.get(0)).map_err(|error| error.to_string())?;
        rows.collect::<rusqlite::Result<_>>().map_err(|error| error.to_string())?
    };
    let mut out = String::new();
    let mut count = 0;
    for path in paths {
        let (content, _) = refresh(&db, &path).map_err(|error| error.to_string())?;
        let doc = match content {
            Content::Text { text, .. } => Some(Doc::new(&text, is_markdown(&path))),
            _ => None,
        };
        for thread in threads_on(&db, &path).map_err(|error| error.to_string())? {
            if thread.agent != agent || (status == "open" && thread.resolved) || (status == "resolved" && !thread.resolved) {
                continue;
            }
            let comments: Vec<Comment> =
                comments_of(&db, thread.id).map_err(|error| error.to_string())?.into_iter().filter(|comment| !comment.draft).collect();
            if comments.is_empty() && !suggested_by(&thread, agent) {
                continue;
            }
            count += 1;
            let state = match (thread.resolved, thread.anchor) {
                (true, _) => "resolved",
                (false, v1::AnchorState::Moved) => "open, the text changed since",
                (false, v1::AnchorState::Detached) => "open, no longer in the file",
                (false, v1::AnchorState::Outdated) if thread.kind == Kind::Video => "open, the video changed since",
                (false, v1::AnchorState::Outdated) => "open, the image changed since",
                _ => "open",
            };
            let _ = write!(out, "[{}] {} ({state})", thread.name(), location(&thread, doc.as_ref()));
            if thread.is_text() {
                let _ = write!(out, ": “{}”", shown(&thread.quote.exact));
            } else {
                let _ = write!(out, ": {}", media_place(&thread));
            }
            out.push('\n');
            if let Some(suggestion) = &thread.suggestion {
                let who = if suggestion.by.is_empty() { person.as_str() } else { "You" };
                let state = match suggestion.decision {
                    v1::SuggestionDecision::Accepted => "accepted",
                    v1::SuggestionDecision::Rejected => "rejected",
                    v1::SuggestionDecision::Unspecified => "not decided yet",
                };
                let _ = writeln!(out, "  {who} suggested “{}” instead ({state})", shown(&suggestion.replacement));
            }
            for comment in comments.iter().filter(|comment| !comment.body.trim().is_empty()) {
                let who = if comment.author.is_empty() {
                    person.clone()
                } else if comment.author == agent {
                    "You".to_owned()
                } else {
                    names.get(&comment.author).cloned().unwrap_or_else(|| comment.author.clone())
                };
                let _ = writeln!(out, "  {who}: {}", comment.body.trim());
            }
        }
    }
    if count == 0 {
        return Ok(match status {
            "resolved" => "No resolved comment threads.".to_owned(),
            "all" => "No comment threads.".to_owned(),
            _ => "No open comment threads.".to_owned(),
        });
    }
    Ok(out.trim_end().to_owned())
}

/// The agent suggested the change the thread holds.
fn suggested_by(thread: &Thread, agent: &str) -> bool {
    thread.suggestion.as_ref().is_some_and(|suggestion| suggestion.by == agent)
}

/// A thread addressed to `agent` that it can see.
fn agents_thread(db: &Connection, agent: &str, id: &str) -> Result<Thread, String> {
    let found = find_thread(db, id).map_err(|_| format!("There's no thread {id}."))?;
    if found.agent != agent {
        return Err(format!("{} isn't addressed to you.", found.name()));
    }
    let sent = comments_of(db, found.id).map_err(|error| error.to_string())?.iter().any(|comment| !comment.draft);
    if !sent && !suggested_by(&found, agent) {
        return Err(format!("There's no thread {id}."));
    }
    Ok(found)
}

/// The agent's comment on a thread, and optionally resolves it.
pub(crate) fn agent_reply(factory: &Factory, agent: &str, id: &str, body: &str, resolve: bool) -> Result<String, String> {
    let person = factory.lock().config.person().to_owned();
    let _work = factory.comments_lock();
    let db = factory.store.db();
    let thread = agents_thread(&db, agent, id)?;
    let fail = |error: rusqlite::Error| error.to_string();
    let body = body.trim();
    if !body.is_empty() {
        db.execute(
            "INSERT INTO thread_comments (thread, author, body, created, draft) VALUES (?1, ?2, ?3, ?4, 0)",
            params![thread.id, agent, body, now_ms()],
        )
        .map_err(fail)?;
    }
    if resolve {
        db.execute("UPDATE threads SET status = 'resolved' WHERE id = ?1", [thread.id]).map_err(fail)?;
    }
    touch(&db, thread.id).map_err(fail)?;
    factory.broadcast_threads(&db, &thread.path).map_err(fail)?;
    Ok(match (resolve, body.is_empty()) {
        (true, true) => format!("Resolved {}.", thread.name()),
        (true, false) => format!("Resolved {}, with your note for {person}.", thread.name()),
        _ => format!("Replied on {}; {person} sees it with the file.", thread.name()),
    })
}

/// The agent leaves a comment on `path` (relative to `folder`), on the text
/// `quote` quotes, for the person.
pub(crate) fn agent_add(
    factory: &Factory,
    agent: &str,
    folder: &Path,
    path: &str,
    quote: &Quote,
    body: &str,
    suggestion: Option<&str>,
) -> Result<String, String> {
    let body = body.trim();
    if body.is_empty() && suggestion.is_none() {
        return Err("body is empty: say what the comment is.".to_owned());
    }
    if quote.exact.is_empty() {
        return Err("quote.exact is empty: give the text the comment is on.".to_owned());
    }
    let asked = Path::new(path.trim());
    let path = if asked.is_absolute() { asked.to_path_buf() } else { folder.join(asked) };
    let path = std::fs::canonicalize(&path).unwrap_or(path).display().to_string();
    let person = factory.lock().config.person().to_owned();
    let _work = factory.comments_lock();
    let db = factory.store.db();
    let fail = |error: rusqlite::Error| error.to_string();
    let (content, _) = refresh(&db, &path).map_err(fail)?;
    let Content::Text { sha, text } = content else { return Err(format!("{path} isn't a text file of up to 2 MB.")) };
    let doc = Doc::new(&text, is_markdown(&path));
    let Some(found) = anchor::find(&doc.source, quote, None).filter(|found| found.similarity >= 1.0) else {
        return Err(format!("That text isn't in {path}: quote it exactly as the file has it."));
    };
    let range = found.range;
    let placed = Quote::of(&doc.source, &range);
    if snapshot(&db, &path).map_err(fail)?.is_none_or(|(old, _)| old != sha) {
        save_snapshot(&db, &path, &sha, &text).map_err(fail)?;
    }
    let now = now_ms();
    let exact = anchor::string(&placed.exact);
    db.execute(
        "INSERT INTO threads (agent, path, kind, version, range_start, range_end, quote, selectors, status, anchor, original, created, updated)
         VALUES (?1, ?2, 'text', ?3, ?4, ?5, ?6, '[]', 'open', 'anchored', ?7, ?8, ?8)",
        params![
            agent,
            path,
            sha,
            range.start as i64,
            range.end as i64,
            serde_json::to_string(&QuoteJson::of(&placed)).unwrap_or_default(),
            exact,
            now
        ],
    )
    .map_err(fail)?;
    let thread = db.last_insert_rowid();
    if let Some(suggestion) = suggestion {
        db.execute(
            "UPDATE threads SET suggestion = ?2, suggested_by = ?3, decision = '' WHERE id = ?1",
            params![thread, suggestion, agent],
        )
        .map_err(fail)?;
    }
    if !body.is_empty() {
        db.execute(
            "INSERT INTO thread_comments (thread, author, body, created, draft) VALUES (?1, ?2, ?3, ?4, 0)",
            params![thread, agent, body, now],
        )
        .map_err(fail)?;
    }
    factory.broadcast_threads(&db, &path).map_err(fail)?;
    let (first, last) = doc.lines(&range);
    let lines = if first == last { format!("line {first}") } else { format!("lines {first}–{last}") };
    Ok(match suggestion {
        Some(_) => format!("Suggested the change on {path}, {lines} (t-{thread}); {person} accepts or rejects it, and you'll hear which."),
        None => format!("Commented on {path}, {lines} (t-{thread}); {person} sees it with the file."),
    })
}
