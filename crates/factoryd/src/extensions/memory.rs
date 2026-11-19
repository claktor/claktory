//! The Memory extension: an agent's own memory of what the person wants
//! (preferences, each with why) and of how things work (knowledge notes). It
//! belongs to that agent alone; no other agent reads or writes it.
//!
//! Preferences and the notes' index are in the agent's system prompt as a
//! section pinned to the segment: written when a context starts and kept
//! until the next compaction, so the prompt cache holds. Everything is in
//! factory.db, every change is also an event in `memory_events`, which is
//! only ever added to, and everything reads memory through `query`.
//!
//! Its instance "memory:factory" is Factory's manual as a read-only memory:
//! the admin notes of docs/MANUAL.md (see `crate::manual`), this build's, with
//! their index pinned like the agent's own.

use std::fmt::Write as _;
use std::sync::Arc;

use factory_agent::Tool;
use factory_protocol::v1;
use rusqlite::{Connection, params};
use serde::Deserialize;
use serde_json::json;

use super::{Extension, InstanceDefault, InstanceSpec, Manifest, PromptKind, PromptSection, enabled, factory_tool};
use crate::config::AgentConfig;
use crate::factory::{Factory, Inner};
use crate::store::Store;
use crate::transcript::now_ms;

pub(crate) const ID: &str = "memory";
/// All preferences together, rule and why, in characters (about 4k tokens).
const PREFERENCES_CAP: usize = 16_000;
/// One preference, rule and why.
const PREFERENCE_CAP: usize = 600;
const NOTES_CAP: usize = 100;
const SLUG_CAP: usize = 64;
const SUMMARY_CAP: usize = 150;
const BODY_CAP: usize = 16_000;
const SEARCH_HITS: usize = 20;
/// Changes the panel shows.
const CHANGES_SHOWN: usize = 50;

/// Its tables in factory.db.
pub(crate) const SCHEMA: &str = "
-- The Memory extension: each agent's preferences as they are now…
CREATE TABLE memory_preferences (
    agent TEXT NOT NULL,
    -- #1, #2… for each agent, never reused
    id INTEGER NOT NULL,
    rule TEXT NOT NULL,
    why TEXT NOT NULL,
    learned INTEGER NOT NULL,
    updated INTEGER NOT NULL,
    PRIMARY KEY (agent, id)
);
-- …its knowledge notes…
CREATE TABLE memory_notes (
    agent TEXT NOT NULL,
    -- a-z, 0-9 and -
    slug TEXT NOT NULL,
    summary TEXT NOT NULL,
    body TEXT NOT NULL,
    created INTEGER NOT NULL,
    updated INTEGER NOT NULL,
    PRIMARY KEY (agent, slug)
);
-- …and every change to them, never changed afterwards.
CREATE TABLE memory_events (
    seq INTEGER PRIMARY KEY,
    agent TEXT NOT NULL,
    time INTEGER NOT NULL,
    -- Who made it: an agent id; '' for the person.
    actor TEXT NOT NULL,
    -- pref_added, pref_replaced, pref_removed, note_written, note_edited or note_removed
    kind TEXT NOT NULL,
    -- The preference's id or the note's slug.
    subject TEXT NOT NULL,
    -- The whole entry before and after, as JSON ({rule, why} or {summary, body}); '' for none.
    before TEXT NOT NULL,
    after TEXT NOT NULL
);
CREATE INDEX memory_events_by_agent ON memory_events (agent, seq);
CREATE INDEX memory_events_by_time ON memory_events (time);
";

static MANIFEST: Manifest = Manifest {
    id: ID,
    name: "Memory",
    icon: "brain",
    description: "The agent's own memory: the person's preferences it learns, each with why, and notes on how things work. Only this agent reads and writes it.",
    default_enabled: false,
    options: &[],
};

static INSTANCES: [InstanceSpec; 1] = [InstanceSpec {
    key: "memory:factory",
    name: "factory",
    label: "Factory manual",
    icon: "book-open",
    description: "Factory's own manual as a read-only memory: the app screen by screen, every extension, the settings file and how to change anything. Admin agents like Rivet have it; turn it on for agents that set Factory up or explain it.",
    default: InstanceDefault::Admin,
}];
const FACTORY: &InstanceSpec = &INSTANCES[0];

const MANUAL_PROMPT: &str = "## The Factory manual

The manual above is Factory's core. The rest is your Factory manual, a read-only memory of notes that ship with Factory: the app screen by screen, every extension and its options, the settings file and how to change anything in it, signing in, and where Factory keeps things. Its notes are listed under \"Factory manual notes\". Before you answer a question about Factory or change its settings, open the notes it touches with memory_read (`note`: a slug{instance}), or search them with `q`. They're always the running version's: after Factory updates, read them again rather than relying on what you read before.
";

const PROMPT: &str = "## Memory

You have a memory of your own that lasts across compactions: preferences (what the person wants and doesn't, each with why) and knowledge notes (what you've learned about how things work). Only you read and write it; the person sees it beside your conversation. It's under \"Your memory\" below.
- Add a preference when the person corrects you or turns something down for a general reason (not a one-off), or tells you a lasting preference. Write the rule and why: rule \"Don't propose paid CI services\", why \"everything runs locally\".
- Write a note when something works differently than you expected, or you learn something you'll need again that no file you can read already says.
- Replace or edit the entry that already covers it rather than adding a near-duplicate, and remove what's no longer true. Keep entries short: memory is capped.
- Never record task status or progress, or what's in files, your instructions or the manual.
- Open a note (memory_read) before you act on its topic. memory_read with `q` searches all of it.
";

const UPDATE: &str = "Change your memory: several ops at once, all applied or none. Ops: pref_add{rule, why}, pref_replace{id, rule?, why?}, pref_remove{id}, note_write{slug, summary, body} (creates or replaces), note_edit{slug, old, new, summary?} (old must appear once), note_remove{slug}. Slugs use a-z, 0-9 and -; a summary is one line.";
const READ: &str = "Read your memory: `note` opens a knowledge note; `q` searches preferences and notes (up to 20 lines). With neither, your preferences and notes index as they are now.";
const READ_BOTH: &str = "Read your memory, or with instance \"factory\" the Factory manual: `note` opens a note; `q` searches (up to 20 lines). With neither, the index as it is now.";
const READ_MANUAL: &str = "Read the Factory manual: `note` opens a note by slug; `q` searches every note (up to 20 lines). With neither, the notes' index.";

pub(crate) struct Memory;

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Preference {
    pub id: u32,
    pub rule: String,
    pub why: String,
    pub learned: i64,
    pub updated: i64,
    /// The local day it was learned, "2026-11-18".
    day: String,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Note {
    pub slug: String,
    pub summary: String,
    /// Empty unless asked for.
    pub body: String,
    pub created: i64,
    pub updated: i64,
    /// The local day it last changed.
    day: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Kind {
    PrefAdded,
    PrefReplaced,
    PrefRemoved,
    NoteWritten,
    NoteEdited,
    NoteRemoved,
}

impl Kind {
    const ALL: [Kind; 6] = [Kind::PrefAdded, Kind::PrefReplaced, Kind::PrefRemoved, Kind::NoteWritten, Kind::NoteEdited, Kind::NoteRemoved];

    fn name(self) -> &'static str {
        match self {
            Kind::PrefAdded => "pref_added",
            Kind::PrefReplaced => "pref_replaced",
            Kind::PrefRemoved => "pref_removed",
            Kind::NoteWritten => "note_written",
            Kind::NoteEdited => "note_edited",
            Kind::NoteRemoved => "note_removed",
        }
    }

    fn parse(name: &str) -> Option<Kind> {
        Kind::ALL.into_iter().find(|kind| kind.name() == name)
    }

    fn proto(self) -> v1::MemoryChangeKind {
        match self {
            Kind::PrefAdded => v1::MemoryChangeKind::PreferenceAdded,
            Kind::PrefReplaced => v1::MemoryChangeKind::PreferenceChanged,
            Kind::PrefRemoved => v1::MemoryChangeKind::PreferenceRemoved,
            Kind::NoteWritten => v1::MemoryChangeKind::NoteWritten,
            Kind::NoteEdited => v1::MemoryChangeKind::NoteEdited,
            Kind::NoteRemoved => v1::MemoryChangeKind::NoteRemoved,
        }
    }

    fn is_preference(self) -> bool {
        matches!(self, Kind::PrefAdded | Kind::PrefReplaced | Kind::PrefRemoved)
    }
}

/// One change, as recorded.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Event {
    pub time: i64,
    /// An agent id; empty for the person.
    pub actor: String,
    pub kind: Kind,
    /// The preference's id or the note's slug.
    pub subject: String,
    /// The entry before and after, as JSON; empty for none.
    pub before: String,
    pub after: String,
}

/// What `query` read.
#[derive(Debug, Default)]
pub(crate) struct Contents {
    /// By id.
    pub preferences: Vec<Preference>,
    /// By slug.
    pub notes: Vec<Note>,
    /// Newest first.
    pub changes: Vec<Event>,
}

/// Which parts of an agent's memory to read. Every read of memory goes
/// through `query`.
#[derive(Default)]
pub(crate) struct Query<'a> {
    pub agent: &'a str,
    /// The notes' bodies too.
    pub bodies: bool,
    /// The latest changes, up to this many.
    pub changes: usize,
}

pub(crate) fn query(store: &Store, query: &Query) -> rusqlite::Result<Contents> {
    read(&store.db(), query)
}

fn read(db: &Connection, query: &Query) -> rusqlite::Result<Contents> {
    let mut contents = Contents::default();
    let mut statement = db.prepare(
        "SELECT id, rule, why, learned, updated, strftime('%Y-%m-%d', learned / 1000, 'unixepoch', 'localtime')
         FROM memory_preferences WHERE agent = ?1 ORDER BY id",
    )?;
    let mut rows = statement.query([query.agent])?;
    while let Some(row) = rows.next()? {
        contents.preferences.push(Preference {
            id: row.get(0)?,
            rule: row.get(1)?,
            why: row.get(2)?,
            learned: row.get(3)?,
            updated: row.get(4)?,
            day: row.get(5)?,
        });
    }
    let body = if query.bodies { "body" } else { "''" };
    let mut statement = db.prepare(&format!(
        "SELECT slug, summary, {body}, created, updated, strftime('%Y-%m-%d', updated / 1000, 'unixepoch', 'localtime')
         FROM memory_notes WHERE agent = ?1 ORDER BY slug"
    ))?;
    let mut rows = statement.query([query.agent])?;
    while let Some(row) = rows.next()? {
        contents.notes.push(Note {
            slug: row.get(0)?,
            summary: row.get(1)?,
            body: row.get(2)?,
            created: row.get(3)?,
            updated: row.get(4)?,
            day: row.get(5)?,
        });
    }
    if query.changes > 0 {
        let mut statement =
            db.prepare("SELECT time, actor, kind, subject, before, after FROM memory_events WHERE agent = ?1 ORDER BY seq DESC LIMIT ?2")?;
        let mut rows = statement.query(params![query.agent, query.changes as i64])?;
        while let Some(row) = rows.next()? {
            let kind: String = row.get(2)?;
            let Some(kind) = Kind::parse(&kind) else { continue };
            contents.changes.push(Event {
                time: row.get(0)?,
                actor: row.get(1)?,
                kind,
                subject: row.get(3)?,
                before: row.get(4)?,
                after: row.get(5)?,
            });
        }
    }
    Ok(contents)
}

/// One change an agent (or the person) asks for.
#[derive(Clone, Debug, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub(crate) enum Op {
    PrefAdd {
        rule: String,
        why: String,
    },
    PrefReplace {
        id: u32,
        #[serde(default)]
        rule: Option<String>,
        #[serde(default)]
        why: Option<String>,
    },
    PrefRemove {
        id: u32,
    },
    NoteWrite {
        slug: String,
        summary: String,
        body: String,
    },
    NoteEdit {
        slug: String,
        old: String,
        new: String,
        #[serde(default)]
        summary: Option<String>,
    },
    NoteRemove {
        slug: String,
    },
}

impl Op {
    fn name(&self) -> &'static str {
        match self {
            Op::PrefAdd { .. } => "pref_add",
            Op::PrefReplace { .. } => "pref_replace",
            Op::PrefRemove { .. } => "pref_remove",
            Op::NoteWrite { .. } => "note_write",
            Op::NoteEdit { .. } => "note_edit",
            Op::NoteRemove { .. } => "note_remove",
        }
    }
}

/// Why a change wasn't made; nothing in the call was.
#[derive(Debug, thiserror::Error)]
pub(crate) enum UpdateError {
    #[error("{0}")]
    Refused(String),
    #[error("Memory couldn't be read or saved: {0}")]
    Store(#[from] rusqlite::Error),
}

/// Applies `ops` in order, all or none; says what changed, op by op.
pub(crate) fn update(store: &Store, agent: &str, actor: &str, ops: &[Op]) -> Result<Vec<String>, UpdateError> {
    let mut db = store.db();
    let transaction = db.transaction()?;
    let mut contents = read(&transaction, &Query { agent, bodies: true, changes: 0 })?;
    let mut next_id: u32 = transaction.query_row(
        "SELECT COALESCE(MAX(CAST(subject AS INTEGER)), 0) FROM memory_events WHERE agent = ?1 AND kind LIKE 'pref%'",
        [agent],
        |row| row.get(0),
    )?;
    let now = now_ms();
    let mut events = Vec::new();
    let mut said = Vec::new();
    for (index, op) in ops.iter().enumerate() {
        let refused = |why: String| UpdateError::Refused(format!("Nothing was changed: op {} ({}): {why}", index + 1, op.name()));
        let (kind, subject, before, after) = apply(&mut contents, op, &mut next_id, now).map_err(refused)?;
        said.push(match kind {
            Kind::PrefAdded => format!("pref #{subject} added"),
            Kind::PrefReplaced => format!("pref #{subject} replaced"),
            Kind::PrefRemoved => format!("pref #{subject} removed"),
            Kind::NoteWritten if before.is_empty() => format!("note {subject} written"),
            Kind::NoteWritten => format!("note {subject} rewritten"),
            Kind::NoteEdited => format!("note {subject} edited"),
            Kind::NoteRemoved => format!("note {subject} removed"),
        });
        events.push((kind, subject, before, after));
    }
    check_caps(&contents)?;
    for (kind, subject, before, after) in &events {
        transaction.execute(
            "INSERT INTO memory_events (agent, time, actor, kind, subject, before, after) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![agent, now, actor, kind.name(), subject, before, after],
        )?;
        if kind.is_preference() {
            transaction.execute("DELETE FROM memory_preferences WHERE agent = ?1 AND id = ?2", params![agent, subject])?;
            if let Some(preference) = contents.preferences.iter().find(|preference| preference.id.to_string() == *subject) {
                transaction.execute(
                    "INSERT INTO memory_preferences (agent, id, rule, why, learned, updated) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                    params![agent, preference.id, preference.rule, preference.why, preference.learned, preference.updated],
                )?;
            }
        } else {
            transaction.execute("DELETE FROM memory_notes WHERE agent = ?1 AND slug = ?2", params![agent, subject])?;
            if let Some(note) = contents.notes.iter().find(|note| note.slug == *subject) {
                transaction.execute(
                    "INSERT INTO memory_notes (agent, slug, summary, body, created, updated) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                    params![agent, note.slug, note.summary, note.body, note.created, note.updated],
                )?;
            }
        }
    }
    transaction.commit()?;
    Ok(said)
}

type Applied = (Kind, String, String, String);

/// Applies one op to `contents`; returns its event.
fn apply(contents: &mut Contents, op: &Op, next_id: &mut u32, now: i64) -> Result<Applied, String> {
    match op {
        Op::PrefAdd { rule, why } => {
            let (rule, why) = (filled(rule, "rule")?, filled(why, "why")?);
            fits_preference(&rule, &why)?;
            *next_id += 1;
            let preference = Preference { id: *next_id, rule, why, learned: now, updated: now, day: String::new() };
            let after = preference_json(&preference);
            contents.preferences.push(preference);
            Ok((Kind::PrefAdded, next_id.to_string(), String::new(), after))
        }
        Op::PrefReplace { id, rule, why } => {
            let Some(preference) = contents.preferences.iter_mut().find(|preference| preference.id == *id) else {
                return Err(format!("there's no preference #{id}"));
            };
            if rule.is_none() && why.is_none() {
                return Err("give a new rule, why, or both".to_owned());
            }
            let before = preference_json(preference);
            let rule = rule.as_deref().map(|rule| filled(rule, "rule")).transpose()?.unwrap_or_else(|| preference.rule.clone());
            let why = why.as_deref().map(|why| filled(why, "why")).transpose()?.unwrap_or_else(|| preference.why.clone());
            fits_preference(&rule, &why)?;
            preference.rule = rule;
            preference.why = why;
            preference.updated = now;
            Ok((Kind::PrefReplaced, id.to_string(), before, preference_json(preference)))
        }
        Op::PrefRemove { id } => {
            let Some(index) = contents.preferences.iter().position(|preference| preference.id == *id) else {
                return Err(format!("there's no preference #{id}"));
            };
            let preference = contents.preferences.remove(index);
            Ok((Kind::PrefRemoved, id.to_string(), preference_json(&preference), String::new()))
        }
        Op::NoteWrite { slug, summary, body } => {
            let slug = valid_slug(slug)?;
            let summary = valid_summary(summary)?;
            let body = valid_body(body)?;
            let existing = contents.notes.iter().position(|note| note.slug == slug);
            let (before, created) = match existing {
                Some(index) => {
                    let note = contents.notes.remove(index);
                    (note_json(&note), note.created)
                }
                None => (String::new(), now),
            };
            let note = Note { slug: slug.clone(), summary, body, created, updated: now, day: String::new() };
            let after = note_json(&note);
            let at = contents.notes.partition_point(|other| other.slug < slug);
            contents.notes.insert(at, note);
            Ok((Kind::NoteWritten, slug, before, after))
        }
        Op::NoteEdit { slug, old, new, summary } => {
            let Some(note) = contents.notes.iter_mut().find(|note| note.slug == slug.trim()) else {
                return Err(format!("there's no note \"{}\"", slug.trim()));
            };
            if old.is_empty() {
                return Err("`old` is empty; give the text to replace".to_owned());
            }
            match note.body.matches(old.as_str()).count() {
                1 => {}
                0 => return Err(format!("`old` isn't in note \"{}\"; open it with memory_read to see its text", note.slug)),
                count => return Err(format!("`old` appears {count} times in note \"{}\"; include more around it", note.slug)),
            }
            let before = note_json(note);
            let body = valid_body(&note.body.replacen(old.as_str(), new, 1))?;
            if let Some(summary) = summary {
                note.summary = valid_summary(summary)?;
            }
            note.body = body;
            note.updated = now;
            Ok((Kind::NoteEdited, note.slug.clone(), before, note_json(note)))
        }
        Op::NoteRemove { slug } => {
            let Some(index) = contents.notes.iter().position(|note| note.slug == slug.trim()) else {
                return Err(format!("there's no note \"{}\"", slug.trim()));
            };
            let note = contents.notes.remove(index);
            Ok((Kind::NoteRemoved, note.slug.clone(), note_json(&note), String::new()))
        }
    }
}

fn filled(text: &str, what: &str) -> Result<String, String> {
    let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if text.is_empty() { Err(format!("`{what}` is empty")) } else { Ok(text) }
}

fn fits_preference(rule: &str, why: &str) -> Result<(), String> {
    let size = rule.chars().count() + why.chars().count();
    if size > PREFERENCE_CAP {
        return Err(format!("it's {size} characters; a preference (rule and why) can have at most {PREFERENCE_CAP}"));
    }
    Ok(())
}

fn valid_slug(slug: &str) -> Result<String, String> {
    let slug = slug.trim();
    let fine = !slug.is_empty()
        && slug.len() <= SLUG_CAP
        && slug.bytes().all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-');
    if !fine {
        return Err(format!("\"{slug}\" isn't a slug: use 1 to {SLUG_CAP} of a-z, 0-9 and -"));
    }
    Ok(slug.to_owned())
}

fn valid_summary(summary: &str) -> Result<String, String> {
    let summary = filled(summary, "summary")?;
    let size = summary.chars().count();
    if size > SUMMARY_CAP {
        return Err(format!("the summary is {size} characters; at most {SUMMARY_CAP}"));
    }
    Ok(summary)
}

fn valid_body(body: &str) -> Result<String, String> {
    let body = body.trim();
    if body.is_empty() {
        return Err("`body` is empty".to_owned());
    }
    let size = body.chars().count();
    if size > BODY_CAP {
        return Err(format!("the note would be {size} characters; at most {BODY_CAP}, so split it or cut it down"));
    }
    Ok(body.to_owned())
}

/// Refuses memory over its caps, listing what's there so the agent can merge
/// or remove entries in the same call.
fn check_caps(contents: &Contents) -> Result<(), UpdateError> {
    let sizes: Vec<usize> =
        contents.preferences.iter().map(|preference| preference.rule.chars().count() + preference.why.chars().count()).collect();
    let total: usize = sizes.iter().sum();
    if total > PREFERENCES_CAP {
        let mut text = format!(
            "Nothing was changed: preferences would take {total} characters, over the cap of {PREFERENCES_CAP}. Merge or remove some in the same call. They'd be (characters in brackets):"
        );
        for (preference, size) in contents.preferences.iter().zip(sizes) {
            let _ = write!(text, "\n#{} [{size}] {}", preference.id, preference.rule);
        }
        return Err(UpdateError::Refused(text));
    }
    if contents.notes.len() > NOTES_CAP {
        let mut text = format!(
            "Nothing was changed: that would make {} notes, over the cap of {NOTES_CAP}. Merge or remove some in the same call. They'd be (characters in brackets):",
            contents.notes.len()
        );
        for note in &contents.notes {
            let _ = write!(text, "\n{} [{}] {}", note.slug, note.body.chars().count(), note.summary);
        }
        return Err(UpdateError::Refused(text));
    }
    Ok(())
}

fn preference_json(preference: &Preference) -> String {
    json!({"rule": preference.rule, "why": preference.why}).to_string()
}

fn note_json(note: &Note) -> String {
    json!({"summary": note.summary, "body": note.body}).to_string()
}

/// An agent is deleted, and its memory with it.
pub(crate) fn forget(store: &Store, agent: &str) -> rusqlite::Result<()> {
    let db = store.db();
    db.execute("DELETE FROM memory_preferences WHERE agent = ?1", [agent])?;
    db.execute("DELETE FROM memory_notes WHERE agent = ?1", [agent])?;
    db.execute("DELETE FROM memory_events WHERE agent = ?1", [agent])?;
    Ok(())
}

/// "#3 Don't propose paid CI. Why: everything runs locally."
fn preference_line(preference: &Preference) -> String {
    let mut line = format!("#{} {}", preference.id, preference.rule);
    if !line.ends_with(['.', '!', '?']) {
        line.push('.');
    }
    let _ = write!(line, " Why: {}", preference.why);
    line
}

/// The preferences and the notes' index, as the agent reads them.
fn overview(contents: &Contents, dated: bool) -> String {
    let mut text = String::from("Preferences:\n");
    if contents.preferences.is_empty() {
        text.push_str("(none yet)\n");
    }
    for preference in &contents.preferences {
        text.push_str(&preference_line(preference));
        if dated {
            let _ = write!(text, " ({})", preference.day);
        }
        text.push('\n');
    }
    text.push_str("\nKnowledge notes:\n");
    if contents.notes.is_empty() {
        text.push_str("(none yet)\n");
    }
    for note in &contents.notes {
        let _ = writeln!(text, "- {}: {}", note.slug, note.summary);
    }
    text
}

impl Extension for Memory {
    fn manifest(&self) -> &'static Manifest {
        &MANIFEST
    }

    fn prompt(&self, factory: &Factory, _inner: &Inner, agent: &AgentConfig) -> Vec<PromptSection> {
        let own = enabled(agent, ID);
        let mut sections = Vec::new();
        if enabled(agent, FACTORY.key) {
            let instance = if own { ", and instance: \"factory\"" } else { "" };
            sections.push(PromptSection { kind: PromptKind::Static, text: MANUAL_PROMPT.replace("{instance}", instance) });
            sections.push(PromptSection { kind: PromptKind::InstancePinned(FACTORY.key), text: manual_index("## Factory manual notes\n\n") });
        }
        if !own {
            return sections;
        }
        sections.push(PromptSection { kind: PromptKind::Static, text: PROMPT.to_owned() });
        match query(&factory.store, &Query { agent: &agent.id, ..Query::default() }) {
            Ok(contents) => {
                let text = format!(
                    "## Your memory\n\nAs it was when this context began. What you've changed since is in the conversation; memory_read with no arguments shows it as it is now.\n\n{}",
                    overview(&contents, true)
                );
                sections.push(PromptSection { kind: PromptKind::SegmentPinned, text });
            }
            Err(error) => tracing::error!(agent = %agent.id, %error, "could not read the agent's memory"),
        }
        sections
    }

    fn tools(&self, factory: &Arc<Factory>, agent: &str) -> Vec<Tool> {
        // Optional parameters are the ones left out of `required` (see `super::tool`).
        let update = json!({
            "type": "object",
            "required": ["ops"],
            "properties": {"ops": {"type": "array", "minItems": 1, "items": {
                "type": "object",
                "required": ["op"],
                "properties": {
                    "op": {"type": "string", "enum": ["pref_add", "pref_replace", "pref_remove", "note_write", "note_edit", "note_remove"]},
                    "id": {"type": "integer", "minimum": 1},
                    "rule": {"type": "string"},
                    "why": {"type": "string"},
                    "slug": {"type": "string"},
                    "summary": {"type": "string"},
                    "body": {"type": "string"},
                    "old": {"type": "string"},
                    "new": {"type": "string"},
                },
                "additionalProperties": false,
            }}},
            "additionalProperties": false,
        });
        let (own, manual) = {
            let inner = factory.lock();
            let config = inner.config.agent(agent);
            (config.is_some_and(|config| enabled(config, ID)), config.is_some_and(|config| enabled(config, FACTORY.key)))
        };
        let mut read = json!({
            "type": "object",
            "properties": {"note": {"type": "string"}, "q": {"type": "string"}},
            "additionalProperties": false,
        });
        if own && manual {
            read["properties"]["instance"] = json!({"type": "string", "enum": [FACTORY.name]});
        }
        let mut tools = Vec::new();
        if own {
            tools.push(factory_tool(factory, agent, "memory_update", "Update memory", UPDATE, update, run_update));
        }
        let description = match (own, manual) {
            (true, true) => READ_BOTH,
            (false, true) => READ_MANUAL,
            _ => READ,
        };
        tools.push(factory_tool(factory, agent, "memory_read", "Read memory", description, read, run_read));
        tools
    }

    fn instances(&self) -> &'static [InstanceSpec] {
        &INSTANCES
    }

    fn instance_view(&self, factory: &Factory, _inner: &Inner, agent: &str, instance: &'static InstanceSpec) -> Option<v1::ExtensionView> {
        let notes = crate::manual::manual()
            .notes
            .iter()
            .map(|note| v1::MemoryNote {
                slug: note.slug.clone(),
                summary: note.summary.clone(),
                body: crate::manual::render(&note.body, &factory.home),
                created: 0,
                updated: 0,
            })
            .collect();
        Some(v1::ExtensionView {
            extension_id: instance.key.to_owned(),
            agent_id: agent.to_owned(),
            title: instance.label.to_owned(),
            body: Some(v1::extension_view::Body::Memory(v1::Memory { notes, read_only: true, ..Default::default() })),
        })
    }

    fn view(&self, factory: &Factory, _inner: &Inner, agent: &str) -> Option<v1::ExtensionView> {
        let contents = match query(&factory.store, &Query { agent, bodies: true, changes: CHANGES_SHOWN }) {
            Ok(contents) => contents,
            Err(error) => {
                tracing::error!(%agent, %error, "could not read the agent's memory");
                return None;
            }
        };
        let preferences = contents
            .preferences
            .into_iter()
            .map(|preference| v1::MemoryPreference {
                id: preference.id,
                rule: preference.rule,
                why: preference.why,
                learned: preference.learned,
                updated: preference.updated,
            })
            .collect();
        let notes = contents
            .notes
            .into_iter()
            .map(|note| v1::MemoryNote {
                slug: note.slug,
                summary: note.summary,
                body: note.body,
                created: note.created,
                updated: note.updated,
            })
            .collect();
        let changes = contents.changes.iter().map(change).collect();
        Some(v1::ExtensionView {
            extension_id: ID.to_owned(),
            agent_id: agent.to_owned(),
            title: MANIFEST.name.to_owned(),
            body: Some(v1::extension_view::Body::Memory(v1::Memory { preferences, notes, changes, read_only: false })),
        })
    }
}

/// A change as the panel shows it.
fn change(event: &Event) -> v1::MemoryChange {
    // A removal shows what was removed; anything else, what it became.
    let entry = if event.after.is_empty() { &event.before } else { &event.after };
    let entry: serde_json::Value = serde_json::from_str(entry).unwrap_or_default();
    let field = if event.kind.is_preference() { "rule" } else { "summary" };
    v1::MemoryChange {
        time: event.time,
        actor: event.actor.clone(),
        kind: event.kind.proto().into(),
        preference_id: if event.kind.is_preference() { event.subject.parse().unwrap_or(0) } else { 0 },
        note_slug: if event.kind.is_preference() { String::new() } else { event.subject.clone() },
        text: entry[field].as_str().unwrap_or_default().to_owned(),
    }
}

#[derive(Deserialize)]
struct UpdateArgs {
    ops: Vec<Op>,
}

fn run_update(factory: &Factory, agent: &str, args: serde_json::Value) -> Result<String, String> {
    let args: UpdateArgs = serde_json::from_value(args).map_err(|error| error.to_string())?;
    let said = update(&factory.store, agent, agent, &args.ops).map_err(|error| error.to_string())?;
    factory.refresh_view(agent);
    Ok(format!("ok: {}", said.join("; ")))
}

#[derive(Deserialize)]
struct ReadArgs {
    #[serde(default)]
    note: Option<String>,
    #[serde(default)]
    q: Option<String>,
    #[serde(default)]
    instance: Option<String>,
}

fn run_read(factory: &Factory, agent: &str, args: serde_json::Value) -> Result<String, String> {
    let args: ReadArgs = serde_json::from_value(args).map_err(|error| error.to_string())?;
    let note = args.note.as_deref().map(str::trim).filter(|note| !note.is_empty());
    let q = args.q.as_deref().map(str::trim).filter(|q| !q.is_empty());
    let (own, manual) = {
        let inner = factory.lock();
        let config = inner.config.agent(agent);
        (config.is_some_and(|config| enabled(config, ID)), config.is_some_and(|config| enabled(config, FACTORY.key)))
    };
    match args.instance.as_deref().map(str::trim).filter(|instance| !instance.is_empty()) {
        Some(instance) if instance == FACTORY.name && manual => return read_manual(factory, note, q),
        Some(instance) if instance == FACTORY.name => return Err("The Factory manual isn't on for you.".to_owned()),
        Some(other) => return Err(format!("instance is \"{}\" for the Factory manual, or left out for your own memory; got \"{other}\".", FACTORY.name)),
        None if !own && manual => return read_manual(factory, note, q),
        None => {}
    }
    let failed = |error: rusqlite::Error| format!("Memory couldn't be read: {error}");
    let contents = query(&factory.store, &Query { agent, bodies: note.is_some() || q.is_some(), changes: 0 }).map_err(failed)?;
    if let Some(slug) = note {
        let Some(note) = contents.notes.iter().find(|note| note.slug == slug) else {
            let slugs: Vec<&str> = contents.notes.iter().map(|note| note.slug.as_str()).collect();
            return Err(format!(
                "There's no note \"{slug}\". Notes: {}.",
                if slugs.is_empty() { "none".to_owned() } else { slugs.join(", ") }
            ));
        };
        return Ok(format!("{}: {} (updated {})\n\n{}", note.slug, note.summary, note.day, note.body));
    }
    let Some(q) = q else { return Ok(overview(&contents, false)) };
    let hits = search(&contents, q);
    if hits.is_empty() {
        return Ok(format!("Nothing in your memory mentions \"{q}\"."));
    }
    let mut lines: Vec<String> = hits.iter().take(SEARCH_HITS).cloned().collect();
    if hits.len() > SEARCH_HITS {
        lines.push(format!("+{} more; narrow q.", hits.len() - SEARCH_HITS));
    }
    Ok(lines.join("\n"))
}

/// The Factory manual's notes' index, one line each, after `heading`.
fn manual_index(heading: &str) -> String {
    let mut text = heading.to_owned();
    for note in &crate::manual::manual().notes {
        let _ = writeln!(text, "- {}: {}", note.slug, note.summary);
    }
    text
}

/// memory_read on the Factory manual: a note, a search, or the index.
fn read_manual(factory: &Factory, note: Option<&str>, q: Option<&str>) -> Result<String, String> {
    let notes = &crate::manual::manual().notes;
    if let Some(slug) = note {
        let Some(note) = notes.iter().find(|note| note.slug == slug) else {
            let slugs: Vec<&str> = notes.iter().map(|note| note.slug.as_str()).collect();
            return Err(format!("The Factory manual has no note \"{slug}\". Notes: {}.", slugs.join(", ")));
        };
        return Ok(format!("{}: {}\n\n{}", note.slug, note.summary, crate::manual::render(&note.body, &factory.home)));
    }
    let Some(q) = q else { return Ok(manual_index("Factory manual notes:\n")) };
    let lower = q.to_lowercase();
    let mentions = |text: &str| text.to_lowercase().contains(&lower);
    let mut hits = Vec::new();
    for note in notes {
        if mentions(&note.slug) || mentions(&note.summary) {
            hits.push(format!("{}: {}", note.slug, note.summary));
        }
        for (number, line) in note.body.lines().enumerate() {
            if mentions(line) {
                hits.push(format!("{}:{}: {}", note.slug, number + 1, clip(line.trim(), 200)));
            }
        }
    }
    if hits.is_empty() {
        return Ok(format!("Nothing in the Factory manual mentions \"{q}\"."));
    }
    let mut lines: Vec<String> = hits.iter().take(SEARCH_HITS).cloned().collect();
    if hits.len() > SEARCH_HITS {
        lines.push(format!("+{} more; narrow q.", hits.len() - SEARCH_HITS));
    }
    Ok(lines.join("\n"))
}

/// Lines that mention `q`, ignoring case: "pref #3: …", "note deploy: …"
/// for a note's summary, "note deploy:12: …" for line 12 of its body.
fn search(contents: &Contents, q: &str) -> Vec<String> {
    let q = q.to_lowercase();
    let mentions = |text: &str| text.to_lowercase().contains(&q);
    let mut hits = Vec::new();
    for preference in &contents.preferences {
        if mentions(&preference.rule) || mentions(&preference.why) {
            hits.push(format!("pref {}", preference_line(preference)));
        }
    }
    for note in &contents.notes {
        if mentions(&note.slug) || mentions(&note.summary) {
            hits.push(format!("note {}: {}", note.slug, note.summary));
        }
        for (number, line) in note.body.lines().enumerate() {
            if mentions(line) {
                hits.push(format!("note {}:{}: {}", note.slug, number + 1, clip(line.trim(), 200)));
            }
        }
    }
    hits
}

fn clip(text: &str, chars: usize) -> String {
    match text.char_indices().nth(chars) {
        Some((end, _)) => format!("{}…", &text[..end]),
        None => text.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> Store {
        static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let next = NEXT.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let dir = std::env::temp_dir().join(format!("factoryd-memory-{}-{}-{next}", std::process::id(), now_ms()));
        std::fs::create_dir_all(&dir).expect("dir");
        Store::open(&dir.join("factory.db")).expect("store")
    }

    fn ops(value: serde_json::Value) -> Vec<Op> {
        serde_json::from_value(value).expect("ops")
    }

    fn refused(result: Result<Vec<String>, UpdateError>) -> String {
        match result {
            Err(UpdateError::Refused(text)) => text,
            other => panic!("expected a refusal, got {other:?}"),
        }
    }

    #[test]
    fn preferences_are_added_replaced_and_removed_with_their_history() {
        let store = store();
        let said = update(
            &store,
            "ops",
            "ops",
            &ops(json!([
                {"op": "pref_add", "rule": "Don't propose paid CI", "why": "everything runs locally"},
                {"op": "pref_add", "rule": "Answer  in\nGerman", "why": "he reads faster in it"},
            ])),
        )
        .expect("add");
        assert_eq!(said, ["pref #1 added", "pref #2 added"]);
        assert_eq!(
            update(&store, "other", "other", &ops(json!([{"op": "pref_add", "rule": "R", "why": "W"}]))).expect("add"),
            ["pref #1 added"]
        );

        let said = update(
            &store,
            "ops",
            "ops",
            &ops(json!([{"op": "pref_replace", "id": 2, "rule": "Answer in English"}, {"op": "pref_remove", "id": 1}])),
        )
        .expect("change");
        assert_eq!(said, ["pref #2 replaced", "pref #1 removed"]);
        let contents = query(&store, &Query { agent: "ops", changes: 10, ..Query::default() }).expect("query");
        assert_eq!(contents.preferences.len(), 1);
        assert_eq!(preference_line(&contents.preferences[0]), "#2 Answer in English. Why: he reads faster in it");
        let kinds: Vec<Kind> = contents.changes.iter().map(|event| event.kind).collect();
        assert_eq!(kinds, [Kind::PrefRemoved, Kind::PrefReplaced, Kind::PrefAdded, Kind::PrefAdded], "newest first");
        assert_eq!(contents.changes[1].before, json!({"rule": "Answer in German", "why": "he reads faster in it"}).to_string());
        assert_eq!(change(&contents.changes[0]).text, "Don't propose paid CI", "a removal shows what went");

        // Ids aren't reused, even for the latest one removed.
        update(&store, "ops", "ops", &ops(json!([{"op": "pref_remove", "id": 2}]))).expect("remove");
        assert_eq!(
            update(&store, "ops", "ops", &ops(json!([{"op": "pref_add", "rule": "R", "why": "W"}]))).expect("add"),
            ["pref #3 added"]
        );

        forget(&store, "ops").expect("forget");
        let gone = query(&store, &Query { agent: "ops", changes: 10, ..Query::default() }).expect("query");
        assert!(gone.preferences.is_empty() && gone.changes.is_empty());
        assert_eq!(query(&store, &Query { agent: "other", ..Query::default() }).expect("query").preferences.len(), 1);
    }

    #[test]
    fn a_call_is_applied_whole_or_not_at_all() {
        let store = store();
        let call = ops(json!([
            {"op": "pref_add", "rule": "Keep it", "why": "because"},
            {"op": "pref_replace", "id": 7, "why": "nope"},
        ]));
        assert_eq!(refused(update(&store, "a", "a", &call)), "Nothing was changed: op 2 (pref_replace): there's no preference #7");
        let contents = query(&store, &Query { agent: "a", changes: 10, ..Query::default() }).expect("query");
        assert!(contents.preferences.is_empty() && contents.changes.is_empty());

        // Over the cap: refused, with what's there and how big, so the agent can merge.
        let long = "x".repeat(PREFERENCE_CAP - 1);
        let many: Vec<serde_json::Value> =
            (0..PREFERENCES_CAP / PREFERENCE_CAP).map(|_| json!({"op": "pref_add", "rule": long, "why": "w"})).collect();
        update(&store, "a", "a", &ops(json!(many))).expect("fits");
        let over = refused(update(&store, "a", "a", &ops(json!([{"op": "pref_add", "rule": long, "why": "w"}]))));
        assert!(over.starts_with("Nothing was changed: preferences would take"), "{over}");
        assert!(over.contains(&format!("\n#1 [{PREFERENCE_CAP}] xxx")), "{over}");
        // …and merging in the same call makes it fit.
        let merged = ops(json!([{"op": "pref_remove", "id": 1}, {"op": "pref_add", "rule": long, "why": "w"}]));
        update(&store, "a", "a", &merged).expect("merged");

        let too_long = refused(update(&store, "a", "a", &ops(json!([{"op": "pref_add", "rule": "x".repeat(PREFERENCE_CAP), "why": "w"}]))));
        assert!(too_long.ends_with(&format!("a preference (rule and why) can have at most {PREFERENCE_CAP}")), "{too_long}");
        assert!(refused(update(&store, "a", "a", &ops(json!([{"op": "pref_add", "rule": "R", "why": " "}])))).ends_with("`why` is empty"));
    }

    #[test]
    fn notes_are_written_edited_searched_and_removed() {
        let store = store();
        let body = "Deploys go through scripts/release.sh.\nThe dev Factory uses port 7421.";
        let said = update(
            &store,
            "b",
            "b",
            &ops(json!([
                {"op": "note_write", "slug": "deploy", "summary": "How releases ship", "body": body},
                {"op": "note_write", "slug": "auth", "summary": "Sign-in quirks", "body": "Codex needs a device code."},
            ])),
        )
        .expect("write");
        assert_eq!(said, ["note deploy written", "note auth written"]);
        let said = update(
            &store,
            "b",
            "b",
            &ops(json!([{"op": "note_edit", "slug": "deploy", "old": "7421", "new": "7421 (7420 is the real one)"}])),
        )
        .expect("edit");
        assert_eq!(said, ["note deploy edited"]);
        let duplicate = ops(json!([{"op": "note_edit", "slug": "deploy", "old": "742", "new": "x"}]));
        assert!(
            refused(update(&store, "b", "b", &duplicate)).ends_with("`old` appears 2 times in note \"deploy\"; include more around it")
        );
        let bad = ops(json!([{"op": "note_write", "slug": "Deploy Notes", "summary": "s", "body": "b"}]));
        assert!(refused(update(&store, "b", "b", &bad)).contains("isn't a slug"));
        let rewrite = ops(json!([{"op": "note_write", "slug": "auth", "summary": "Sign-in", "body": "Codex: device code."}]));
        assert_eq!(update(&store, "b", "b", &rewrite).expect("rewrite"), ["note auth rewritten"]);

        let contents = query(&store, &Query { agent: "b", bodies: true, ..Query::default() }).expect("query");
        let slugs: Vec<&str> = contents.notes.iter().map(|note| note.slug.as_str()).collect();
        assert_eq!(slugs, ["auth", "deploy"], "by slug");
        assert!(contents.notes[1].body.ends_with("port 7421 (7420 is the real one)."));
        assert_eq!(
            search(&contents, "PORT"),
            ["note deploy:2: The dev Factory uses port 7421 (7420 is the real one)."],
            "search ignores case and gives the line"
        );
        assert_eq!(search(&contents, "sign"), ["note auth: Sign-in"]);
        assert!(
            query(&store, &Query { agent: "b", ..Query::default() }).expect("query").notes[0].body.is_empty(),
            "no bodies unless asked"
        );

        let overview = overview(&contents, false);
        assert_eq!(overview, "Preferences:\n(none yet)\n\nKnowledge notes:\n- auth: Sign-in\n- deploy: How releases ship\n");

        update(&store, "b", "b", &ops(json!([{"op": "note_remove", "slug": "auth"}]))).expect("remove");
        let contents = query(&store, &Query { agent: "b", changes: 1, ..Query::default() }).expect("query");
        assert_eq!(contents.notes.len(), 1);
        let shown = change(&contents.changes[0]);
        assert_eq!((shown.kind(), shown.note_slug.as_str(), shown.text.as_str()), (v1::MemoryChangeKind::NoteRemoved, "auth", "Sign-in"));
    }
}
