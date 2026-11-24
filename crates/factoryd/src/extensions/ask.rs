//! The Ask extension (#208): an agent blocked on something only the person
//! can give (a decision, an approval, access) asks them with `ask_person`,
//! instead of burying the question in its chat, where the person may never
//! see it. The question shows as a card in its conversation
//! (`v1::AskPerson`) and counts in the agent's waiting seqs, so the app's
//! Needs-you shows it; the person answers it or dismisses it (AnswerAsk),
//! and the agent is told in an event: the answer, or to use its best
//! judgment. On for every agent unless it's turned off.
//!
//! Asks live in factory.db. The conversation shows each after the
//! `ask_person` call that made it (its result names it), so it reloads where
//! it was; its state comes from here. A pending ask waits until it's
//! decided, across restarts: the person may not look for hours. An agent has
//! at most `MAX_PENDING` waiting at once. Clearing its conversation
//! dismisses them, since the person can no longer see them.

use std::sync::Arc;

use factory_agent::{Tool, ToolResult};
use factory_protocol::v1::{self, AskPersonState as State};
use rusqlite::{OptionalExtension, params};
use serde::{Deserialize, Serialize};
use serde_json::json;

use super::{Extension, Manifest, PromptKind, PromptSection};
use crate::config::AgentConfig;
use crate::factory::{Factory, Inner, RequestError, host_event};
use crate::store::Store;
use crate::transcript::{Transcript, now_ms};

pub(crate) const ID: &str = "ask";
pub(crate) const TOOL: &str = "ask_person";
/// The Messaging extension's tool for admin agents that asks the person to
/// clear another agent's conversation, with a card of this extension's.
pub(crate) const CLEAR_TOOL: &str = "agents_clear";

/// Questions an agent may have waiting at once.
pub(crate) const MAX_PENDING: i64 = 3;
/// The longest title, in characters.
const MAX_TITLE: usize = 120;

static MANIFEST: Manifest = Manifest {
    id: ID,
    name: "Ask",
    icon: "message-circle-question",
    description: "Lets it ask you when it's blocked on something only you can give: a card you answer or dismiss, instead of a question lost in its chat.",
    default_enabled: true,
    options: &[],
};

pub(crate) const SCHEMA: &str = "
-- Questions agents ask the person (the Ask extension's ask_person).
CREATE TABLE asks (
    id INTEGER PRIMARY KEY,
    -- The agent that asked.
    agent TEXT NOT NULL,
    title TEXT NOT NULL,
    body TEXT NOT NULL,
    -- 1: it wants a written answer.
    wants_text INTEGER NOT NULL,
    created INTEGER NOT NULL,
    -- pending, answered, dismissed, or cleared (/clear closed it unseen)
    state TEXT NOT NULL,
    -- The person's answer or comment; '' for none.
    answer TEXT NOT NULL,
    -- When the person answered or dismissed it; null while pending.
    decided INTEGER
);
CREATE INDEX asks_by_agent ON asks (agent, state);
";

/// Review asks (#275): the file the agent asks the person to review,
/// absolute; '' for an ordinary ask.
pub(crate) const REVIEW: &str = "
ALTER TABLE asks ADD COLUMN review TEXT NOT NULL DEFAULT '';
";

/// The migration that added what the person answered a question with
/// besides words: their drawings on a PDF's pages, as uploads (a JSON list
/// of attachments; null for none).
pub(crate) const DRAWINGS: &str = "
ALTER TABLE asks ADD COLUMN answer_attachments TEXT;
";

/// The migration that added procedures (ask_person's steps): the steps in
/// order, as JSON (`Step`); null for an ordinary ask.
pub(crate) const STEPS: &str = "
ALTER TABLE asks ADD COLUMN steps TEXT;
";

/// The migration that added replies (#370 Submit): 1 when the person
/// answered with a comment, neither a yes nor a no (state 'answered').
pub(crate) const REPLIED: &str = "
ALTER TABLE asks ADD COLUMN replied INTEGER NOT NULL DEFAULT 0;
";

/// The migration that added asking to clear another agent's conversation
/// (agents_clear, an admin agent's tool): the id of the agent the person's
/// approval clears; '' for an ordinary ask.
pub(crate) const CLEARS: &str = "
ALTER TABLE asks ADD COLUMN clears TEXT NOT NULL DEFAULT '';
";

/// The migration that tied a card asking to clear an agent's conversation
/// to that agent (Fugo's follow-up to agents_clear): its incarnation
/// (`user_apps::incarnation`) when it was asked. Approving clears only that
/// agent, never one deleted and made again with its id since.
pub(crate) const CLEARS_INCARNATION: &str = "
ALTER TABLE asks ADD COLUMN clears_incarnation TEXT NOT NULL DEFAULT '';
";

/// A procedure's steps: at least, at most, and the longest one, in characters.
const MIN_STEPS: usize = 2;
const MAX_STEPS: usize = 12;
const MAX_STEP: usize = 1000;

/// One step of a procedure, as kept.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
struct Step {
    text: String,
    /// "" while pending, "done", or "cant" (the person couldn't).
    #[serde(default)]
    state: String,
    #[serde(default)]
    comment: String,
    /// ms; 0 while pending.
    #[serde(default)]
    decided: i64,
}

impl Step {
    fn to_proto(&self) -> v1::AskStep {
        let state = match self.state.as_str() {
            "done" => v1::AskStepState::Done,
            "cant" => v1::AskStepState::Cant,
            _ => v1::AskStepState::Pending,
        };
        v1::AskStep { text: self.text.clone(), state: state.into(), comment: self.comment.clone(), decided_ms: self.decided }
    }
}

/// Questions a contained agent (one limited to a pipe, or a curator) has
/// had refused because it already asked `CONTAINED_PER_HOUR` in the past
/// hour (pipes-automation H4, Fugo's change 5): what it asks comes from
/// someone outside, so the person sees a count of these, never their cards.
pub(crate) const HELD: &str = "
CREATE TABLE asks_held (
    agent TEXT NOT NULL,
    created INTEGER NOT NULL
);
CREATE INDEX asks_held_by_agent ON asks_held (agent, created);
";

/// Questions a contained agent may ask in an hour.
const CONTAINED_PER_HOUR: i64 = 3;
const HOUR_MS: i64 = 3_600_000;

/// The questions Claktory refused `agent` today (since local midnight),
/// past its hourly limit: the person's quiet count (Agent.held_asks).
pub(crate) fn held_today(store: &Store, agent: &str) -> u32 {
    store
        .db()
        .query_row(
            "SELECT COUNT(*) FROM asks_held WHERE agent = ?1
             AND created >= CAST(strftime('%s', 'now', 'localtime', 'start of day', 'utc') AS INTEGER) * 1000",
            [agent],
            |row| row.get::<_, u32>(0),
        )
        .unwrap_or_else(|error| {
            tracing::error!(%agent, %error, "could not count the questions held back");
            0
        })
}

/// The largest text file a review asks about: what comments work on.
const REVIEW_TEXT_LIMIT: u64 = 2_000_000;

const PROMPT: &str = "# Asking the person\nWhen you're genuinely blocked on something only the person can give you (a decision that's theirs, an approval, access or information you can't get any other way), ask them with ask_person: they see a card they answer or dismiss, and you're told which in an event. Never use it for questions your best judgment can settle: decide, carry on, and say what you chose in your report. Don't ask in your chat text instead: the person may never see it.\nOne question or one action per card, kept short: what to do, where, and how long. Several decisions are several cards, never a list in one, and a checklist item that waits on them is one concern: if it bundles several, split it into items first, each with its own card. Something that takes them several actions goes as steps on one card: they see one step at a time, the next only once the one before is done. When it's about an item on a checklist, give item.";

const ABOUT: &str = "Ask the person something, as a card they answer or dismiss. Use it only when you're genuinely blocked on something only the person can give you: a decision that's theirs, an approval, access or information you can't get any other way. Never use it for questions your best judgment can settle: decide, carry on, and mention the choice in your report. One question or one action per card, never a list of decisions: several decisions are several cards, and at most 3 can wait at once. title: what you need, in a short line. body: what you need and why, standing alone (the person may read it hours later, away from your conversation); a command they should run goes in a fenced code block, which the card gives a Copy button. wants_text: true if you need a written answer; false (the default) if a yes, an approval, is enough. review: when the person asked to review or comment on a document (text or Markdown up to 2 MB, an image, a video or a PDF up to 64 MB), ask with review: its path (absolute, or relative to your working folder) instead of having someone else show it; their card opens the file beside your conversation, their comments on it come to you, and the answer says how many they left. item: the checklist item it's about (12 for #12, or Ana#12): give it whenever the question is about an item on a checklist, since that's how it shows with the item in their Needs you; if they dismiss it, you're told what to do with the item, and Claktory can have you ask again later. steps: when they have to do several things in order (a procedure), the steps, each one short action (2 to 12 of them): the card shows one step at a time with Done and Can't do this, you're told of each in an event, and the next shows only once the one before is done; a step they can't do stops it. title says what it's all for, body introduces it; not with wants_text or review. You're told their answer, or that they dismissed it, in an event; meanwhile carry on with whatever doesn't depend on it.";

pub(crate) struct Ask;

impl Extension for Ask {
    fn manifest(&self) -> &'static Manifest {
        &MANIFEST
    }

    fn prompt(&self, _factory: &Factory, _inner: &Inner, _agent: &AgentConfig) -> Vec<PromptSection> {
        vec![PromptSection { kind: PromptKind::Static, text: PROMPT.to_owned() }]
    }

    fn tools(&self, factory: &Arc<Factory>, agent: &str) -> Vec<Tool> {
        let parameters = json!({
            "type": "object",
            "required": ["title", "body"],
            "properties": {
                "title": {"type": "string", "minLength": 1},
                "body": {"type": "string", "minLength": 1},
                "wants_text": {"type": "boolean"},
                "review": {"type": "string"},
                "item": {"type": "string"},
                "steps": {"type": "array", "items": {"type": "string", "minLength": 1}, "minItems": MIN_STEPS, "maxItems": MAX_STEPS},
            },
            "additionalProperties": false,
        });
        let mut tools = vec![super::tool(TOOL, "Ask the person", ABOUT, parameters, super::typed_handler(factory, agent, ask))];
        // Alerts that can't wait (`crate::notify`); not for an agent limited
        // to a pipe, whose asks already reach the person.
        if !factory.pipes.contained(agent) {
            tools.push(crate::notify::tool::tool(factory, agent));
        }
        tools
    }
}

#[derive(Deserialize)]
struct AskArgs {
    title: String,
    body: String,
    #[serde(default)]
    wants_text: bool,
    #[serde(default)]
    review: Option<String>,
    #[serde(default)]
    item: Option<String>,
    #[serde(default)]
    steps: Option<Vec<String>>,
}

/// Records the agent's question, or says why not. The result names it, so
/// its conversation shows it after the call.
fn ask(factory: &Arc<Factory>, agent: &str, args: AskArgs) -> Result<ToolResult, String> {
    let title = args.title.split_whitespace().collect::<Vec<_>>().join(" ");
    let body = args.body.trim().to_owned();
    if title.is_empty() || body.is_empty() {
        return Err("Give a title and a body: what you need, and why. Nothing was asked.".to_owned());
    }
    if title.chars().count() > MAX_TITLE {
        return Err(format!("The title is a short line (at most {MAX_TITLE} characters); put the rest in the body. Nothing was asked."));
    }
    let steps: Vec<Step> =
        args.steps.unwrap_or_default().iter().map(|step| Step { text: step.trim().to_owned(), ..Step::default() }).collect();
    if !steps.is_empty() {
        if !(MIN_STEPS..=MAX_STEPS).contains(&steps.len()) {
            return Err(format!(
                "steps: give {MIN_STEPS} to {MAX_STEPS} of them, each one action; a single action is an ordinary question, without steps. Nothing was asked."
            ));
        }
        if let Some(at) = steps.iter().position(|step| step.text.is_empty() || step.text.chars().count() > MAX_STEP) {
            return Err(format!(
                "steps: step {} must say what to do in at most {MAX_STEP} characters; split a longer one in two. Nothing was asked.",
                at + 1
            ));
        }
        if args.wants_text || args.review.as_deref().is_some_and(|path| !path.trim().is_empty()) {
            return Err(
                "steps go without wants_text and review: each step is done or not, with a comment if they like. Nothing was asked."
                    .to_owned(),
            );
        }
    }
    let person = {
        let inner = factory.lock();
        super::caller(&inner.config, agent)?;
        inner.config.person().to_owned()
    };
    let review = match args.review.as_deref().map(str::trim).filter(|path| !path.is_empty()) {
        Some(path) => review_file(factory, agent, path)?,
        None => String::new(),
    };
    // The checklist item it's about (#279): a dismissal can have it asked
    // again, and the person sees the item with the card in Needs you. A
    // question naming none, right after the one item it blocked on the
    // person (and hasn't asked about), is about that one.
    let (item, linked) = super::checklist::item_for(factory, agent, args.item.as_deref())
        .map_err(|problem| format!("item: {problem}. Nothing was asked."))?;
    // Read a pipe: the card says so, and where from.
    let routes = crate::pipes::agents::routes(factory, agent);
    let taint = (!routes.is_empty()).then(|| crate::pipes::agents::routes_json(&routes));
    // A liaison or a curator: what it asks comes from someone outside.
    let contained = factory.pipes.contained(agent);
    // Ok: the question's id; Err: held back, and when it may ask again.
    let asked: Result<String, i64> = 'asked: {
        let db = factory.store.db();
        let waiting: i64 = db
            .query_row("SELECT COUNT(*) FROM asks WHERE agent = ?1 AND state = 'pending'", [agent], |row| row.get(0))
            .map_err(|error| format!("Couldn't record the question: {error}"))?;
        if waiting >= MAX_PENDING {
            return Err(format!(
                "You already have {waiting} questions waiting for {person}, the most you can: wait for an answer before asking more, and settle what your best judgment can. Nothing was asked."
            ));
        }
        if contained {
            let now = now_ms();
            let recent: Vec<i64> = crate::store::query(
                &db,
                "SELECT created FROM asks WHERE agent = ?1 AND created > ?2 ORDER BY created DESC LIMIT ?3",
                params![agent, now - HOUR_MS, CONTAINED_PER_HOUR],
                |row| row.get(0),
            )
            .map_err(|error| format!("Couldn't record the question: {error}"))?;
            if recent.len() as i64 >= CONTAINED_PER_HOUR {
                db.execute("INSERT INTO asks_held (agent, created) VALUES (?1, ?2)", params![agent, now])
                    .map_err(|error| format!("Couldn't record the question: {error}"))?;
                break 'asked Err(recent.last().copied().unwrap_or(now) + HOUR_MS);
            }
        }
        db.execute(
            "INSERT INTO asks (agent, title, body, wants_text, created, state, answer, taint, review, item, steps) VALUES (?1, ?2, ?3, ?4, ?5, 'pending', '', ?6, ?7, ?8, ?9)",
            params![
                agent,
                title,
                body,
                args.wants_text,
                now_ms(),
                taint.as_ref().map(serde_json::Value::to_string),
                review,
                item,
                (!steps.is_empty()).then(|| serde_json::to_string(&steps).unwrap_or_default())
            ],
        )
        .map_err(|error| format!("Couldn't record the question: {error}"))?;
        Ok(db.last_insert_rowid().to_string())
    };
    let id = match asked {
        Ok(id) => id,
        Err(until) => {
            factory.broadcast_workspace(&factory.lock());
            let at = factory_agent::accounts::local_time(u64::try_from(until).unwrap_or_default())
                .unwrap_or_else(|| "an hour from your first of them".to_owned());
            return Err(format!(
                "You've asked {person} {CONTAINED_PER_HOUR} questions in the past hour, the most an agent that reads what comes through pipes can, so this one wasn't shown to them: they see only how many were held back. If it still matters, ask again after {at}; meanwhile settle what your best judgment can. Nothing was asked."
            ));
        }
    };
    let said = if !steps.is_empty() {
        format!(
            "Asked {person} to go through {} steps: their card shows one at a time, the next once the one before is done. You're told of each in an event, and if they can't do one, which stops it; meanwhile carry on with whatever doesn't depend on it.",
            steps.len()
        )
    } else if review.is_empty() {
        format!(
            "Asked {person}: they see it as a card to answer or dismiss. You'll be told which in an event; meanwhile carry on with whatever doesn't depend on it."
        )
    } else {
        format!(
            "Asked {person} to review {review}: their card opens it beside your conversation, and their comments on it come to you. You'll be told when they answer or dismiss it; meanwhile carry on with whatever doesn't depend on it."
        )
    };
    let said = format!("{said}{}", super::checklist::taken_for(linked, &person));
    let mut details = json!({"ask": id, "title": title, "body": body, "wants_text": args.wants_text});
    if let Some(taint) = taint {
        details["taint"] = taint;
    }
    if !review.is_empty() {
        details["review"] = review.into();
    }
    if !steps.is_empty() {
        details["steps"] = steps.iter().map(|step| step.text.clone()).collect::<Vec<_>>().into();
    }
    Ok(ToolResult::text(said, details))
}

/// The file a review ask names, absolute, or why not: one the agent may
/// read (its file tools reach it) and comments work on.
fn review_file(factory: &Arc<Factory>, agent: &str, asked: &str) -> Result<String, String> {
    let config = factory.lock().config.clone();
    let me = super::caller(&config, agent)?;
    let places = crate::runner::placed(factory, &config, me);
    let home = std::env::home_dir().unwrap_or_else(|| std::path::PathBuf::from("/"));
    let path = match asked {
        "~" => home,
        _ => asked.strip_prefix("~/").map_or_else(|| std::path::PathBuf::from(asked), |rest| home.join(rest)),
    };
    let path = if path.is_absolute() { path } else { places.folder.join(path) };
    let Ok(path) = std::fs::canonicalize(&path) else {
        return Err(format!("There's no file {}. Nothing was asked.", path.display()));
    };
    let real = |dir: &std::path::PathBuf| std::fs::canonicalize(dir).unwrap_or_else(|_| dir.clone());
    let roots: Vec<std::path::PathBuf> =
        std::iter::once(&places.folder).chain(&places.extra_dirs).chain(&places.read_only).map(real).collect();
    if !roots.iter().any(|root| path.starts_with(root)) {
        return Err(format!(
            "{} isn't in your folders, so you can't read it: ask about a file you can read. Nothing was asked.",
            path.display()
        ));
    }
    let shown = path.display().to_string();
    let content = crate::extensions::viewer::read_file(&shown, 0, 512);
    if !content.error.is_empty() {
        return Err(format!("{}. Nothing was asked.", content.error.trim_end_matches('.')));
    }
    let kinds = "text or Markdown up to 2 MB, an image, a video (MP4 or QuickTime) or a PDF up to 64 MB";
    match content.mime.as_str() {
        // A PDF's size is checked as it's read: up to 64 MB.
        "image/png" | "image/jpeg" | "image/gif" | "image/webp" | "image/bmp" | "video/mp4" | "video/quicktime" | "application/pdf" => {
            Ok(shown)
        }
        "application/octet-stream" | "video/webm" => Err(format!("{shown} isn't a file comments work on ({kinds}). Nothing was asked.")),
        _ if content.size > REVIEW_TEXT_LIMIT => Err(format!("{shown} is too big to comment on ({kinds}). Nothing was asked.")),
        _ => Ok(shown),
    }
}

/// The entry an ask_person result shows (called directly, or through
/// extension_call), pending; None for any other result, or one that asked
/// nothing.
pub(crate) fn asked(tool: &str, is_error: bool, details: &serde_json::Value) -> Option<v1::AskPerson> {
    if !matches!(tool, TOOL | CLEAR_TOOL | "extension_call") || is_error {
        return None;
    }
    let text = crate::transcript::text_fields(details);
    let id = details.get("ask")?.as_str()?.to_owned();
    Some(v1::AskPerson {
        id,
        title: text("title"),
        body: text("body"),
        wants_text: details.get("wants_text").and_then(serde_json::Value::as_bool).unwrap_or(false),
        state: State::Pending.into(),
        answer: String::new(),
        decided_ms: 0,
        taint: details.get("taint").map(crate::pipes::agents::routes_from_json).unwrap_or_default(),
        review_path: text("review"),
        answer_attachments: Vec::new(),
        steps: details
            .get("steps")
            .and_then(serde_json::Value::as_array)
            .map(|steps| {
                steps
                    .iter()
                    .filter_map(serde_json::Value::as_str)
                    .map(|text| Step { text: text.to_owned(), ..Step::default() }.to_proto())
                    .collect()
            })
            .unwrap_or_default(),
        replied: false,
    })
}

/// An ask as recorded.
struct Row {
    id: i64,
    title: String,
    body: String,
    wants_text: bool,
    state: String,
    answer: String,
    decided: Option<i64>,
    /// JSON (`pipes::agents::routes_json`); null for none.
    taint: Option<String>,
    /// The file it asks to review; '' for none.
    review: String,
    created: i64,
    /// What the person answered or dismissed it with besides words (their
    /// drawings on a PDF's pages), as uploaded.
    answer_attachments: Vec<factory_agent::Attachment>,
    /// The checklist item it's about, "agent#12"; '' for none.
    item: String,
    /// A procedure's steps; empty for an ordinary ask.
    steps: Vec<Step>,
    /// Answered with a reply (#370), not an approval.
    replied: bool,
    /// The agent whose conversation approving it clears (agents_clear); ''
    /// for an ordinary ask.
    clears: String,
    /// That agent's incarnation when it was asked.
    clears_incarnation: String,
}

const COLUMNS: &str =
    "id, title, body, wants_text, state, answer, decided, taint, review, created, answer_attachments, item, steps, replied, clears, clears_incarnation";

fn row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Row> {
    Ok(Row {
        id: row.get(0)?,
        title: row.get(1)?,
        body: row.get(2)?,
        wants_text: row.get(3)?,
        state: row.get(4)?,
        answer: row.get(5)?,
        decided: row.get(6)?,
        taint: row.get(7)?,
        review: row.get(8)?,
        created: row.get(9)?,
        answer_attachments: row.get::<_, Option<String>>(10)?.and_then(|list| serde_json::from_str(&list).ok()).unwrap_or_default(),
        item: row.get(11)?,
        steps: row.get::<_, Option<String>>(12)?.and_then(|steps| serde_json::from_str(&steps).ok()).unwrap_or_default(),
        replied: row.get(13)?,
        clears: row.get(14)?,
        clears_incarnation: row.get(15)?,
    })
}

impl Row {
    fn to_proto(&self) -> v1::AskPerson {
        let state = match self.state.as_str() {
            "answered" => State::Answered,
            "dismissed" | "cleared" => State::Dismissed,
            _ => State::Pending,
        };
        v1::AskPerson {
            id: self.id.to_string(),
            title: self.title.clone(),
            body: self.body.clone(),
            wants_text: self.wants_text,
            state: state.into(),
            answer: self.answer.clone(),
            decided_ms: self.decided.unwrap_or_default(),
            taint: self
                .taint
                .as_deref()
                .and_then(|taint| serde_json::from_str(taint).ok())
                .map(|taint| crate::pipes::agents::routes_from_json(&taint))
                .unwrap_or_default(),
            review_path: self.review.clone(),
            answer_attachments: self.answer_attachments.iter().map(crate::transcript::attachment).collect(),
            steps: self.steps.iter().map(Step::to_proto).collect(),
            replied: self.replied,
        }
    }
}

/// Shows the agent's decided asks as decided in its just-loaded
/// conversation, and its procedures as far as they've gone.
pub(crate) fn restore(store: &Store, agent: &str, transcript: &mut Transcript) -> rusqlite::Result<()> {
    let db = store.db();
    let mut statement = db
        .prepare(&format!("SELECT {COLUMNS} FROM asks WHERE agent = ?1 AND (state != 'pending' OR steps IS NOT NULL) ORDER BY id"))?;
    for decided in statement.query_map([agent], row)? {
        transcript.set_ask(decided?.to_proto());
    }
    Ok(())
}

/// The ids of the agent's questions waiting for the person about
/// checklist item `item` ("agent#12"), oldest first.
pub(crate) fn pending_ids_about(store: &Store, agent: &str, item: &str) -> Vec<i64> {
    crate::store::query(
        &store.db(),
        "SELECT id FROM asks WHERE agent = ?1 AND state = 'pending' AND item = ?2 ORDER BY id",
        params![agent, item],
        |row| row.get(0),
    )
    .unwrap_or_default()
}

/// Whether the agent has a question waiting for the person.
pub(crate) fn pending_of(store: &Store, agent: &str) -> bool {
    store
        .db()
        .query_row("SELECT EXISTS (SELECT 1 FROM asks WHERE agent = ?1 AND state = 'pending')", [agent], |row| row.get(0))
        .unwrap_or(false)
}

/// An agent is deleted, and its asks with it.
pub(crate) fn forget(store: &Store, agent: &str) -> rusqlite::Result<()> {
    store.db().execute("DELETE FROM asks WHERE agent = ?1", [agent])?;
    Ok(())
}

/// The agent's conversation was cleared: its waiting questions can't be
/// seen any more, so they're closed as cleared (shown as dismissed, but
/// not by the person, so Needs you's history leaves them out), and don't
/// count against new ones.
pub(crate) fn cleared(store: &Store, agent: &str) -> rusqlite::Result<()> {
    store
        .db()
        .execute("UPDATE asks SET state = 'cleared', decided = ?2 WHERE agent = ?1 AND state = 'pending'", params![agent, now_ms()])?;
    Ok(())
}

/// `agent`'s conversation went back to before `since` (`crate::fork`): its
/// questions asked since then close as /clear's do. How many.
pub(crate) fn cleared_since(store: &Store, agent: &str, since: i64) -> rusqlite::Result<usize> {
    store.db().execute(
        "UPDATE asks SET state = 'cleared', decided = ?3 WHERE agent = ?1 AND state = 'pending' AND created >= ?2",
        params![agent, since, now_ms()],
    )
}

/// The person answers the ask shown at `seq` in `agent`'s conversation, or
/// dismisses it. Its entry shows the decision, and the agent is told.
pub(crate) async fn answer(factory: &Arc<Factory>, request: &v1::AnswerAsk) -> Result<(), RequestError> {
    let agent = request.agent_id.as_str();
    factory.known_agent(agent)?;
    factory.ensure_loaded(agent).await?;
    let id = {
        let inner = factory.lock();
        let entry = inner
            .agents
            .get(agent)
            .and_then(|runtime| runtime.transcript.as_ref())
            .and_then(|transcript| transcript.entry(request.seq).cloned());
        match entry.and_then(|entry| entry.kind) {
            Some(v1::entry::Kind::AskPerson(ask)) => ask.id,
            _ => return Err(RequestError::Ask("There's no such question in this agent's conversation.".to_owned())),
        }
    };
    let number: i64 = id.parse().map_err(|_| RequestError::Ask("There's no such question.".to_owned()))?;
    let found = factory
        .store
        .db()
        .query_row(&format!("SELECT {COLUMNS} FROM asks WHERE id = ?1 AND agent = ?2"), params![number, agent], row)
        .optional()?;
    let Some(mut ask) = found else { return Err(RequestError::Ask("There's no such question.".to_owned())) };
    if ask.state == "cleared" {
        return Err(RequestError::Ask("This question was closed without an answer.".to_owned()));
    }
    if ask.state != "pending" {
        return Err(RequestError::Ask(format!("This question was already {}.", ask.state)));
    }
    if !ask.steps.is_empty() || request.step.is_some() {
        if request.reply {
            return Err(RequestError::Ask("Answer the step with Done or Can't do this, and a comment if you like.".to_owned()));
        }
        return step(factory, agent, request, ask);
    }
    let text = request.text.trim();
    // A reply (#370 Submit): the person's words, neither a yes nor a no.
    if request.reply {
        if ask.wants_text {
            return Err(RequestError::Ask("This question wants a written answer: send it as the answer.".to_owned()));
        }
        if text.is_empty() {
            return Err(RequestError::Ask("Write a reply, or approve or dismiss the question.".to_owned()));
        }
    }
    let dismiss = request.dismiss && !request.reply;
    if !dismiss && ask.wants_text && text.is_empty() {
        return Err(RequestError::Ask("Write an answer, or dismiss the question.".to_owned()));
    }
    // Asked to clear another agent's conversation (agents_clear): approved,
    // it's cleared, so that agent must still be here.
    let clearing = !ask.clears.is_empty() && !dismiss && !request.reply;
    if clearing {
        let Some(name) = factory.lock().config.agent(&ask.clears).filter(|_| ask.clears != agent).map(|target| target.name.clone())
        else {
            return Err(RequestError::Ask(
                "That agent is no longer in this Claktory, so there's nothing to clear: dismiss this card.".to_owned(),
            ));
        };
        // The agent it was asked for, not another made since with its id.
        let now = crate::user_apps::current_incarnation(&factory.home, &ask.clears);
        if ask.clears_incarnation.is_empty() || now.as_deref() != Some(ask.clears_incarnation.as_str()) {
            return Err(RequestError::Ask(format!(
                "The agent this card asked to clear was deleted, and {name} is another agent made since, so nothing was cleared: dismiss this card."
            )));
        }
    }
    // Drawings on a PDF's pages, answering or dismissing alike: uploads of
    // this agent's, as a message's attachments are.
    let attachments = factory.attachments(agent, &request.attachments)?;
    let stored = Some(&attachments).filter(|list| !list.is_empty()).and_then(|list| serde_json::to_string(list).ok());
    // Dismissing, the text is the person's optional comment.
    let state = if dismiss { "dismissed" } else { "answered" };
    let answer = text;
    let decided = now_ms();
    let claimed = factory.store.db().execute(
        "UPDATE asks SET state = ?2, answer = ?3, decided = ?4, answer_attachments = ?5, replied = ?6 WHERE id = ?1 AND state = 'pending'",
        params![number, state, answer, decided, stored, request.reply],
    )?;
    if claimed == 0 {
        return Err(RequestError::Ask("This question was already answered.".to_owned()));
    }
    (ask.state, ask.answer, ask.decided, ask.answer_attachments, ask.replied) =
        (state.to_owned(), answer.to_owned(), Some(decided), attachments, request.reply);
    // Its entry shows the decision, and no longer waits for the person.
    let person = {
        let mut inner = factory.lock();
        let entry = inner
            .agents
            .get_mut(agent)
            .and_then(|runtime| runtime.transcript.as_mut())
            .and_then(|transcript| transcript.set_ask(ask.to_proto()));
        if let Some(entry) = entry {
            factory.send_change(&inner, agent, crate::transcript::Change::Entry(entry));
            factory.broadcast_workspace(&inner);
        }
        inner.config.person_capitalized().to_owned()
    };
    if !ask.clears.is_empty() {
        let name = factory.lock().config.agent(&ask.clears).map_or_else(|| ask.clears.clone(), |target| target.name.clone());
        let cleared = if clearing {
            let cleared = factory.clear_conversation(&ask.clears).await;
            if let Err(error) = &cleared {
                tracing::error!(%agent, clears = %ask.clears, %error, "could not clear the conversation the person approved clearing");
            }
            Some(cleared.map_err(|error| error.to_string()))
        } else {
            None
        };
        let (summary, text) = told_clear(&person, &ask, &name, cleared);
        factory.deliver_event(agent, text, host_event(summary));
        return Ok(());
    }
    let comments = (!ask.review.is_empty()).then(|| reviewed(factory, agent, &ask));
    let (summary, mut text) = told(&person, &ask, comments);
    // Decided, a question about an item that still waits on the person
    // says what to do with it now (#282): clear the block, or decide; and
    // when dismissed, when it's asked again.
    if !ask.item.is_empty() {
        match super::checklist::decided(factory, &ask.item, ask.state == "dismissed", "ask") {
            Ok(Some(more)) => text.push_str(&more),
            Ok(None) => {}
            Err(error) => tracing::error!(%agent, %error, "could not note a decided question on its item"),
        }
        if let Some((whose, _)) = ask.item.rsplit_once('#') {
            factory.refresh_view(whose);
        }
        factory.broadcast_workspace(&factory.lock());
    }
    let event = host_event(summary);
    factory.deliver_event(agent, text, event);
    Ok(())
}

/// What the agent that asked to clear `name`'s conversation is told: a
/// summary, and the event's text. `cleared`: how clearing it went, approved;
/// None when it wasn't.
fn told_clear(person: &str, ask: &Row, name: &str, cleared: Option<Result<(), String>>) -> (String, String) {
    let comment = if ask.answer.is_empty() { String::new() } else { format!("\n\nTheir comment:\n\n{}", ask.answer) };
    match cleared {
        Some(Ok(())) => (
            format!("{person} approved clearing {name}'s conversation."),
            format!(
                "{person} approved, so {name}'s conversation is cleared: the old one is archived, and {name} starts on a new, empty one, with its checklist and memory.{comment}"
            ),
        ),
        Some(Err(error)) => (
            format!("{name}'s conversation couldn't be cleared."),
            format!("{person} approved clearing {name}'s conversation, but it couldn't be cleared: {error}{comment}"),
        ),
        None if ask.replied => (
            format!("{person} replied to your request to clear {name}'s conversation."),
            format!(
                "{person} replied to your request to clear {name}'s conversation instead of approving it, so nothing was cleared:\n\n{}",
                ask.answer
            ),
        ),
        None => (
            format!("{person} dismissed clearing {name}'s conversation."),
            format!("{person} dismissed your request to clear {name}'s conversation, so it stays as it is.{comment}"),
        ),
    }
}

/// `target` was deleted: the cards asking to clear its conversation close
/// unseen, as /clear closes questions (shown as dismissed, but not by the
/// person), and each agent that asked is told.
pub(crate) fn target_deleted(factory: &Arc<Factory>, target: &str) {
    let closed = (|| -> rusqlite::Result<Vec<(String, Row)>> {
        let db = factory.store.db();
        let asked = crate::store::query(
            &db,
            &format!("SELECT {COLUMNS}, agent FROM asks WHERE clears = ?1 AND state = 'pending' ORDER BY id"),
            [target],
            // `agent` comes after COLUMNS, which `row` reads from the start.
            |found| Ok((found.get::<_, String>(16)?, row(found)?)),
        )?;
        // Each one closed here, not by another caller at the same time, so
        // its agent is told once.
        let mut closed = Vec::new();
        for (asker, ask) in asked {
            let claimed =
                db.execute("UPDATE asks SET state = 'cleared', decided = ?2 WHERE id = ?1 AND state = 'pending'", params![ask.id, now_ms()])?;
            if claimed == 1 {
                closed.push((asker, ask));
            }
        }
        Ok(closed)
    })();
    let closed = closed.unwrap_or_else(|error| {
        tracing::error!(%target, %error, "could not close the cards asking to clear a deleted agent's conversation");
        Vec::new()
    });
    for (asker, mut ask) in closed {
        (ask.state, ask.decided) = ("cleared".to_owned(), Some(now_ms()));
        {
            let mut inner = factory.lock();
            let entry =
                inner.agents.get_mut(&asker).and_then(|runtime| runtime.transcript.as_mut()).and_then(|transcript| transcript.set_ask(ask.to_proto()));
            if let Some(entry) = entry {
                factory.send_change(&inner, &asker, crate::transcript::Change::Entry(entry));
            }
            factory.broadcast_workspace(&inner);
        }
        let summary = format!("Your card \"{}\" closed: that agent was deleted.", ask.title);
        let text = format!("The agent your card \"{}\" was for was deleted, so the card is closed and nothing was cleared.", ask.title);
        factory.deliver_event(&asker, text, host_event(summary));
    }
}

/// Closes the cards asking to clear the conversations of agents the
/// settings no longer list, as `target_deleted` does: deleted in the app
/// (which closed them already), or taken out of config.json.
pub(crate) fn close_for_gone(factory: &Arc<Factory>) {
    let targets: Vec<String> = crate::store::query(
        &factory.store.db(),
        "SELECT DISTINCT clears FROM asks WHERE clears != '' AND state = 'pending'",
        [],
        |row| row.get(0),
    )
    .unwrap_or_else(|error| {
        tracing::error!(%error, "could not read the cards asking to clear conversations");
        Vec::new()
    });
    let gone: Vec<String> = {
        let inner = factory.lock();
        targets.into_iter().filter(|target| inner.config.agent(target).is_none()).collect()
    };
    for target in gone {
        target_deleted(factory, &target);
    }
}

/// Opens the card asking the person to clear `target`'s conversation
/// (agents_clear, an admin agent's tool; `name` is the target's): their
/// approval runs /clear for it, and nothing else does. It's one of `asker`'s
/// questions waiting, and asks once per agent at a time.
pub(crate) fn ask_to_clear(factory: &Factory, asker: &str, target: &str, name: &str) -> Result<ToolResult, String> {
    let (person, me) = {
        let inner = factory.lock();
        let me = super::caller(&inner.config, asker)?;
        (inner.config.person().to_owned(), me.name.clone())
    };
    let title = format!("Clear {name}'s conversation");
    let body = format!(
        "{me} asks to clear {name}'s conversation. It's archived, and {name} starts on a new, empty one, stopping first if it's working; its checklist and memory stay."
    );
    // Approving clears this agent, never one made again with its id.
    let incarnation = crate::user_apps::incarnation(&factory.home, target).map_err(|error| format!("Couldn't ask: {error}"))?;
    // Read a pipe or a web page: the card says so, and where from.
    let routes = crate::pipes::agents::routes(factory, asker);
    let taint = (!routes.is_empty()).then(|| crate::pipes::agents::routes_json(&routes));
    let id = {
        let db = factory.store.db();
        let couldnt = |error: rusqlite::Error| format!("Couldn't ask: {error}");
        let waiting: i64 =
            db.query_row("SELECT COUNT(*) FROM asks WHERE agent = ?1 AND state = 'pending'", [asker], |row| row.get(0)).map_err(couldnt)?;
        if waiting >= MAX_PENDING {
            return Err(format!(
                "You already have {waiting} questions waiting for {person}, the most you can: wait for an answer before asking more. Nothing was asked."
            ));
        }
        let asked: bool = db
            .query_row(
                "SELECT EXISTS (SELECT 1 FROM asks WHERE agent = ?1 AND state = 'pending' AND clears = ?2)",
                params![asker, target],
                |row| row.get(0),
            )
            .map_err(couldnt)?;
        if asked {
            return Err(format!("You already asked {person} to clear {name}'s conversation; it's waiting for them. Nothing new was asked."));
        }
        db.execute(
            "INSERT INTO asks (agent, title, body, wants_text, created, state, answer, taint, clears, clears_incarnation) VALUES (?1, ?2, ?3, 0, ?4, 'pending', '', ?5, ?6, ?7)",
            params![asker, title, body, now_ms(), taint.as_ref().map(serde_json::Value::to_string), target, incarnation],
        )
        .map_err(couldnt)?;
        db.last_insert_rowid().to_string()
    };
    let said = format!(
        "Asked {person}: they see a card to approve or dismiss clearing {name}'s conversation, and only their approval clears it. You'll be told which in an event."
    );
    let mut details = json!({"ask": id, "title": title, "body": body, "wants_text": false, "clears": target});
    if let Some(taint) = taint {
        details["taint"] = taint;
    }
    Ok(ToolResult::text(said, details))
}

/// The person answers a procedure's current step: Done, or Can't do this
/// (`dismiss`), which stops it. The agent is told; the card shows the next
/// step, or that it's over.
fn step(factory: &Arc<Factory>, agent: &str, request: &v1::AnswerAsk, mut ask: Row) -> Result<(), RequestError> {
    let Some(answered) = request.step else {
        return Err(RequestError::Ask("Answer the step the card shows.".to_owned()));
    };
    if ask.steps.is_empty() {
        return Err(RequestError::Ask("This question has no steps.".to_owned()));
    }
    if !request.attachments.is_empty() {
        return Err(RequestError::Ask("Answer this step in words.".to_owned()));
    }
    let Some(current) = ask.steps.iter().position(|step| step.state.is_empty()) else {
        return Err(RequestError::Ask("Every step was already answered.".to_owned()));
    };
    match (answered as usize).cmp(&current) {
        std::cmp::Ordering::Less => return Err(RequestError::Ask("That step was already answered.".to_owned())),
        std::cmp::Ordering::Greater => return Err(RequestError::Ask("You haven't reached that step yet.".to_owned())),
        std::cmp::Ordering::Equal => {}
    }
    let before = serde_json::to_string(&ask.steps).unwrap_or_default();
    let now = now_ms();
    let comment = request.text.trim().to_owned();
    let done = if request.dismiss { "cant" } else { "done" };
    ask.steps[current] = Step { text: std::mem::take(&mut ask.steps[current].text), state: done.to_owned(), comment: comment.clone(), decided: now };
    let (state, decided) = match (request.dismiss, current + 1 == ask.steps.len()) {
        (true, _) => ("dismissed", Some(now)),
        (false, true) => ("answered", Some(now)),
        (false, false) => ("pending", None),
    };
    // Kept only if nothing answered it meanwhile (another device, a second tap).
    let claimed = factory.store.db().execute(
        "UPDATE asks SET steps = ?2, state = ?3, answer = ?4, decided = ?5 WHERE id = ?1 AND state = 'pending' AND steps = ?6",
        params![ask.id, serde_json::to_string(&ask.steps).unwrap_or_default(), state, comment, decided, before],
    )?;
    if claimed == 0 {
        return Err(RequestError::Ask("That step was already answered.".to_owned()));
    }
    (ask.state, ask.answer, ask.decided) = (state.to_owned(), comment, decided);
    // Its entry shows the next step, or that it's over.
    let person = {
        let mut inner = factory.lock();
        let entry = inner
            .agents
            .get_mut(agent)
            .and_then(|runtime| runtime.transcript.as_mut())
            .and_then(|transcript| transcript.set_ask(ask.to_proto()));
        if let Some(entry) = entry {
            factory.send_change(&inner, agent, crate::transcript::Change::Entry(entry));
            factory.broadcast_workspace(&inner);
        }
        inner.config.person_capitalized().to_owned()
    };
    let (summary, mut text) = told_step(&person, &ask, current);
    // Over, a procedure about a checklist item says what to do with it, as
    // a decided question does.
    if ask.state != "pending" && !ask.item.is_empty() {
        match super::checklist::decided(factory, &ask.item, ask.state == "dismissed", "ask") {
            Ok(Some(more)) => text.push_str(&more),
            Ok(None) => {}
            Err(error) => tracing::error!(%agent, %error, "could not note a finished procedure on its item"),
        }
        if let Some((whose, _)) = ask.item.rsplit_once('#') {
            factory.refresh_view(whose);
        }
        factory.broadcast_workspace(&factory.lock());
    }
    factory.deliver_event(agent, text, host_event(summary));
    Ok(())
}

/// What the agent is told of step `at` of a procedure, just answered: a
/// summary, and the event's text.
fn told_step(person: &str, ask: &Row, at: usize) -> (String, String) {
    let count = ask.steps.len();
    let step = &ask.steps[at];
    let number = at + 1;
    let title = &ask.title;
    let comment = if step.comment.is_empty() { String::new() } else { format!("\n\nTheir comment:\n\n{}", step.comment) };
    let asked = format!("\n\nYou asked: {title}\n{}", ask.body);
    if step.state == "cant" {
        let rest = match count - number {
            0 => String::new(),
            1 => format!(" Step {count} wasn't shown to them."),
            _ => format!(" Steps {} to {count} weren't shown to them.", number + 1),
        };
        return (
            format!("{person} couldn't do step {number} of {count} of \"{title}\"."),
            format!(
                "{person} couldn't do step {number} of {count} of \"{title}\": {}{comment}\n\nThat stops it there.{rest} Work out what to do next: fix what stopped them and ask again with the steps left, or use your best judgment and say what you chose in your report.{asked}",
                step.text
            ),
        );
    }
    if number == count {
        return (
            format!("{person} did all {count} steps of \"{title}\"."),
            format!("{person} did the last step, {number} of {count}, of \"{title}\": {}{comment}\n\nAll {count} steps are done.{asked}", step.text),
        );
    }
    (
        format!("{person} did step {number} of {count} of \"{title}\"."),
        format!(
            "{person} did step {number} of {count} of \"{title}\": {}{comment}\n\nThey see step {} next: {}\n\nYou're told as each step is done; meanwhile carry on with whatever doesn't depend on it.",
            step.text,
            number + 1,
            ask.steps[at + 1].text
        ),
    )
}

/// A review ask is decided: the person's comments on the file still unsent
/// go to the agent first (they're done with it), and how many they left
/// there since it asked.
fn reviewed(factory: &Arc<Factory>, agent: &str, ask: &Row) -> i64 {
    // None unsent is the usual case: the app sends each comment as it's written.
    let unsent = v1::SendComments { agent_id: agent.to_owned(), path: ask.review.clone(), ..Default::default() };
    let _ = factory.send_comments(&unsent);
    factory
        .store
        .db()
        .query_row(
            "SELECT COUNT(*) FROM thread_comments c JOIN threads t ON t.id = c.thread
             WHERE c.author = '' AND c.draft = 0 AND t.agent = ?1 AND t.path = ?2 AND c.created >= ?3",
            params![agent, ask.review, ask.created],
            |row| row.get(0),
        )
        .unwrap_or(0)
}

/// What the agent is told of a decided ask: a summary, and the event's
/// text; with how many comments the person left on a review ask's file.
/// The line naming the person's drawings on a PDF's pages, by path, so the
/// agent looks at them; empty for none. The app names each one
/// "<pdf stem>-p<page>.png", which gives its page.
fn drawings(person: &str, attached: &[factory_agent::Attachment]) -> String {
    if attached.is_empty() {
        return String::new();
    }
    let page = |attachment: &factory_agent::Attachment| -> Option<u32> {
        let stem = attachment.name.strip_suffix(".png").or_else(|| attachment.name.strip_suffix(".PNG"))?;
        // A second upload of that name the same day is kept as
        // "<stem>-p<page> 2.png" (Sorbet's iPad pass).
        let stem = match stem.rsplit_once(' ') {
            Some((name, copy)) if !copy.is_empty() && copy.bytes().all(|b| b.is_ascii_digit()) => name,
            _ => stem,
        };
        let (_, page) = stem.rsplit_once("-p")?;
        page.parse().ok().filter(|page| *page > 0)
    };
    let count = attached.len();
    let pages: Option<Vec<String>> = attached.iter().map(|attachment| page(attachment).map(|page| page.to_string())).collect();
    let which = match pages {
        Some(pages) if count == 1 => format!(" (page {})", pages[0]),
        Some(pages) => format!(" (pages {} and {})", pages[..count - 1].join(", "), pages[count - 1]),
        None => String::new(),
    };
    let paths: Vec<&str> = attached.iter().map(|attachment| attachment.path.as_str()).collect();
    let (pages, them) = if count == 1 { ("1 page".to_owned(), "it") } else { (format!("{count} pages"), "them") };
    format!("\n\n{person} drew on {pages}{which}; look at {them} with read_image: {}", paths.join(", "))
}

fn told(person: &str, ask: &Row, comments: Option<i64>) -> (String, String) {
    let left = match comments {
        Some(0) => format!("\n\nThey left no comments on {}.", ask.review),
        Some(1) => format!("\n\nThey left 1 comment on {}; it came to you as a comment (comments_list shows it).", ask.review),
        Some(count) => {
            format!("\n\nThey left {count} comments on {}; they came to you as comments (comments_list shows them).", ask.review)
        }
        None => String::new(),
    };
    let drew = drawings(person, &ask.answer_attachments);
    let asked = format!("{left}{drew}\n\nYou asked: {}\n{}", ask.title, ask.body);
    // A reply (#370): their words for what they want, never a yes or a no.
    if ask.replied {
        return (
            format!("{person} replied to \"{}\".", ask.title),
            format!("{person} replied to your question \"{}\":\n\n{}{asked}", ask.title, ask.answer),
        );
    }
    match (ask.state.as_str(), ask.answer.as_str()) {
        ("dismissed", comment) => {
            let judge = "Use your best judgment and carry on; say what you chose in your report.";
            let said = if comment.is_empty() {
                format!("{person} dismissed your question \"{}\": they won't answer it. {judge}{asked}", ask.title)
            } else {
                format!(
                    "{person} dismissed your question \"{}\": they won't answer it. Their comment:\n\n{comment}\n\n{judge}{asked}",
                    ask.title
                )
            };
            (format!("{person} dismissed your question \"{}\".", ask.title), said)
        }
        (_, "") => (
            format!("{person} approved \"{}\".", ask.title),
            format!("{person} answered your question \"{}\": yes, go ahead.{asked}", ask.title),
        ),
        (_, answer) if ask.wants_text => (
            format!("{person} answered \"{}\".", ask.title),
            format!("{person} answered your question \"{}\":\n\n{answer}{asked}", ask.title),
        ),
        (_, comment) => (
            format!("{person} approved \"{}\".", ask.title),
            format!("{person} answered your question \"{}\": yes, go ahead. Their comment:\n\n{comment}{asked}", ask.title),
        ),
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_drawing_kept_under_a_second_name_still_names_its_page() {
        let drawn =
            |name: &str| factory_agent::Attachment { path: format!("/u/{name}"), name: name.to_owned(), size: 1, mime: String::new() };
        let said = super::drawings("Matt", &[drawn("field-guide-p1 2.png"), drawn("field-guide-p3.png")]);
        assert!(said.starts_with("\n\nMatt drew on 2 pages (pages 1 and 3)"), "{said}");
    }

    use super::*;

    fn ask(state: &str, wants_text: bool, answer: &str) -> Row {
        Row {
            id: 1,
            title: "Deploy to production?".to_owned(),
            body: "The release is ready.".to_owned(),
            wants_text,
            state: state.to_owned(),
            answer: answer.to_owned(),
            decided: Some(1),
            taint: None,
            review: String::new(),
            created: 0,
            answer_attachments: Vec::new(),
            item: String::new(),
            steps: Vec::new(),
            replied: false,
            clears: String::new(),
            clears_incarnation: String::new(),
        }
    }

    #[test]
    fn a_reply_is_told_as_the_persons_words_with_no_yes_or_no() {
        let replied = Row { replied: true, ..ask("answered", false, "No, staging first.") };
        let (summary, text) = told("Matt", &replied, None);
        assert_eq!(summary, "Matt replied to \"Deploy to production?\".");
        assert_eq!(
            text,
            "Matt replied to your question \"Deploy to production?\":\n\nNo, staging first.\n\nYou asked: Deploy to production?\nThe release is ready."
        );
        assert!(!text.contains("yes") && !text.contains("dismissed"), "{text}");
    }

    #[test]
    fn the_agent_is_told_what_the_person_decided() {
        let (summary, text) = told("Matt", &ask("dismissed", false, ""), None);
        assert_eq!(summary, "Matt dismissed your question \"Deploy to production?\".");
        assert!(
            text.contains("Use your best judgment") && text.ends_with("You asked: Deploy to production?\nThe release is ready."),
            "{text}"
        );
        assert!(
            told("Matt", &ask("answered", false, ""), None)
                .1
                .starts_with("Matt answered your question \"Deploy to production?\": yes, go ahead.")
        );
        assert!(told("Matt", &ask("answered", false, "After 5 pm."), None).1.contains("yes, go ahead. Their comment:\n\nAfter 5 pm."));
        let (_, text) = told("Matt", &ask("dismissed", false, "Not this week."), None);
        assert!(
            text.starts_with(
                "Matt dismissed your question \"Deploy to production?\": they won't answer it. Their comment:\n\nNot this week.\n\nUse your best judgment"
            ),
            "{text}"
        );
        let (summary, text) = told("Matt", &ask("answered", true, "Use the blue one."), None);
        assert_eq!(summary, "Matt answered \"Deploy to production?\".");
        assert!(text.starts_with("Matt answered your question \"Deploy to production?\":\n\nUse the blue one."), "{text}");
    }

    #[test]
    fn only_an_ask_person_result_shows_a_card() {
        let details = json!({"ask": "7", "title": "T", "body": "B", "wants_text": true});
        let shown = asked(TOOL, false, &details).expect("a card");
        assert_eq!((shown.id.as_str(), shown.wants_text, shown.state()), ("7", true, State::Pending));
        assert!(asked(TOOL, true, &details).is_none());
        assert!(asked("bash", false, &details).is_none());
        assert!(asked("extension_call", false, &json!({})).is_none());
    }

    #[test]
    fn a_command_to_run_goes_in_a_fenced_block_for_its_copy_button() {
        // The card gives fenced code blocks a Copy button, not inline code (#238).
        assert!(ABOUT.contains("a command they should run goes in a fenced code block"), "{ABOUT}");
    }
}
