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
use serde::Deserialize;
use serde_json::json;

use super::{Extension, Manifest, PromptKind, PromptSection};
use crate::config::AgentConfig;
use crate::factory::{Factory, Inner, RequestError, host_event};
use crate::store::Store;
use crate::transcript::{Transcript, now_ms};

pub(crate) const ID: &str = "ask";
pub(crate) const TOOL: &str = "ask_person";

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
    -- pending, answered or dismissed
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

/// The largest text file a review asks about: what comments work on.
const REVIEW_TEXT_LIMIT: u64 = 2_000_000;

const PROMPT: &str = "# Asking the person\nWhen you're genuinely blocked on something only the person can give you (a decision that's theirs, an approval, access or information you can't get any other way), ask them with ask_person: they see a card they answer or dismiss, and you're told which in an event. Never use it for questions your best judgment can settle: decide, carry on, and say what you chose in your report. Don't ask in your chat text instead: the person may never see it.";

const ABOUT: &str = "Ask the person something, as a card they answer or dismiss. Use it only when you're genuinely blocked on something only the person can give you: a decision that's theirs, an approval, access or information you can't get any other way. Never use it for questions your best judgment can settle: decide, carry on, and mention the choice in your report. Ask one real question at a time, not a batch of small ones; at most 3 can wait at once. title: what you need, in a short line. body: what you need and why, standing alone (the person may read it hours later, away from your conversation); a command they should run goes in a fenced code block, which the card gives a Copy button. wants_text: true if you need a written answer; false (the default) if a yes, an approval, is enough. review: when the person asked to review or comment on a document (text or Markdown up to 2 MB, an image, a video or a PDF up to 64 MB), ask with review: its path (absolute, or relative to your working folder) instead of having someone else show it; their card opens the file beside your conversation, their comments on it come to you, and the answer says how many they left. You're told their answer, or that they dismissed it, in an event; meanwhile carry on with whatever doesn't depend on it.";

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
            },
            "additionalProperties": false,
        });
        let mut tools = vec![super::tool(TOOL, "Ask the person", ABOUT, parameters, super::typed_handler(factory, agent, ask))];
        // Alerts that can't wait (`crate::notify`); not for an agent limited
        // to a pipe, whose asks already reach the person.
        if !factory.pipes.quarantined(agent) {
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
    let person = {
        let inner = factory.lock();
        super::caller(&inner.config, agent)?;
        inner.config.person().to_owned()
    };
    let review = match args.review.as_deref().map(str::trim).filter(|path| !path.is_empty()) {
        Some(path) => review_file(factory, agent, path)?,
        None => String::new(),
    };
    // Read a pipe: the card says so, and where from.
    let routes = crate::pipes::agents::routes(factory, agent);
    let taint = (!routes.is_empty()).then(|| crate::pipes::agents::routes_json(&routes));
    let id = {
        let db = factory.store.db();
        let waiting: i64 = db
            .query_row("SELECT COUNT(*) FROM asks WHERE agent = ?1 AND state = 'pending'", [agent], |row| row.get(0))
            .map_err(|error| format!("Couldn't record the question: {error}"))?;
        if waiting >= MAX_PENDING {
            return Err(format!(
                "You already have {waiting} questions waiting for {person}, the most you can: wait for an answer before asking more, and settle what your best judgment can. Nothing was asked."
            ));
        }
        db.execute(
            "INSERT INTO asks (agent, title, body, wants_text, created, state, answer, taint, review) VALUES (?1, ?2, ?3, ?4, ?5, 'pending', '', ?6, ?7)",
            params![agent, title, body, args.wants_text, now_ms(), taint.as_ref().map(serde_json::Value::to_string), review],
        )
        .map_err(|error| format!("Couldn't record the question: {error}"))?;
        db.last_insert_rowid().to_string()
    };
    let said = if review.is_empty() {
        format!(
            "Asked {person}: they see it as a card to answer or dismiss. You'll be told which in an event; meanwhile carry on with whatever doesn't depend on it."
        )
    } else {
        format!(
            "Asked {person} to review {review}: their card opens it beside your conversation, and their comments on it come to you. You'll be told when they answer or dismiss it; meanwhile carry on with whatever doesn't depend on it."
        )
    };
    let mut details = json!({"ask": id, "title": title, "body": body, "wants_text": args.wants_text});
    if let Some(taint) = taint {
        details["taint"] = taint;
    }
    if !review.is_empty() {
        details["review"] = review.into();
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
    if !matches!(tool, TOOL | "extension_call") || is_error {
        return None;
    }
    let text = |key: &str| details.get(key).and_then(serde_json::Value::as_str).unwrap_or_default().to_owned();
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
}

const COLUMNS: &str = "id, title, body, wants_text, state, answer, decided, taint, review, created, answer_attachments";

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
    })
}

impl Row {
    fn to_proto(&self) -> v1::AskPerson {
        let state = match self.state.as_str() {
            "answered" => State::Answered,
            "dismissed" => State::Dismissed,
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
        }
    }
}

/// Shows the agent's decided asks as decided in its just-loaded conversation.
pub(crate) fn restore(store: &Store, agent: &str, transcript: &mut Transcript) -> rusqlite::Result<()> {
    let db = store.db();
    let mut statement = db.prepare(&format!("SELECT {COLUMNS} FROM asks WHERE agent = ?1 AND state != 'pending' ORDER BY id"))?;
    for decided in statement.query_map([agent], row)? {
        transcript.set_ask(decided?.to_proto());
    }
    Ok(())
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
/// seen any more, so they're dismissed, and don't count against new ones.
pub(crate) fn cleared(store: &Store, agent: &str) -> rusqlite::Result<()> {
    store
        .db()
        .execute("UPDATE asks SET state = 'dismissed', decided = ?2 WHERE agent = ?1 AND state = 'pending'", params![agent, now_ms()])?;
    Ok(())
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
            .and_then(|transcript| transcript.entries().iter().find(|entry| entry.seq == request.seq).cloned());
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
    if ask.state != "pending" {
        return Err(RequestError::Ask(format!("This question was already {}.", ask.state)));
    }
    let text = request.text.trim();
    if !request.dismiss && ask.wants_text && text.is_empty() {
        return Err(RequestError::Ask("Write an answer, or dismiss the question.".to_owned()));
    }
    // Drawings on a PDF's pages, answering or dismissing alike: uploads of
    // this agent's, as a message's attachments are.
    let attachments = factory.attachments(agent, &request.attachments)?;
    let stored = Some(&attachments).filter(|list| !list.is_empty()).and_then(|list| serde_json::to_string(list).ok());
    // Dismissing, the text is the person's optional comment.
    let state = if request.dismiss { "dismissed" } else { "answered" };
    let answer = text;
    let decided = now_ms();
    let claimed = factory.store.db().execute(
        "UPDATE asks SET state = ?2, answer = ?3, decided = ?4, answer_attachments = ?5 WHERE id = ?1 AND state = 'pending'",
        params![number, state, answer, decided, stored],
    )?;
    if claimed == 0 {
        return Err(RequestError::Ask("This question was already answered.".to_owned()));
    }
    (ask.state, ask.answer, ask.decided, ask.answer_attachments) = (state.to_owned(), answer.to_owned(), Some(decided), attachments);
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
    let comments = (!ask.review.is_empty()).then(|| reviewed(factory, agent, &ask));
    let (summary, text) = told(&person, &ask, comments);
    let event = host_event(summary);
    factory.deliver_event(agent, text, event);
    Ok(())
}

/// A review ask is decided: the person's comments on the file still unsent
/// go to the agent first (they're done with it), and how many they left
/// there since it asked.
fn reviewed(factory: &Arc<Factory>, agent: &str, ask: &Row) -> i64 {
    // None unsent is the usual case: the app sends each comment as it's written.
    let unsent = v1::SendComments { agent_id: agent.to_owned(), path: ask.review.clone() };
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
    match (ask.state.as_str(), ask.answer.as_str()) {
        ("dismissed", comment) => {
            let judge = "Use your best judgment and carry on; say what you chose in your report.";
            let said = if comment.is_empty() {
                format!("{person} dismissed your question \"{}\": they won't answer it. {judge}{asked}", ask.title)
            } else {
                format!("{person} dismissed your question \"{}\": they won't answer it. Their comment:\n\n{comment}\n\n{judge}{asked}", ask.title)
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
        }
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
