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
//! dismisses them, since the person can no longer see them. One that no
//! longer applies, its agent takes back with `ask_withdraw` (#407), with a
//! reason the person reads in Needs you's history.

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
/// Takes back one of the agent's own questions still waiting (#407).
pub(crate) const WITHDRAW_TOOL: &str = "ask_withdraw";
/// Changes one of the agent's own questions still waiting (#428).
pub(crate) const UPDATE_TOOL: &str = "ask_update";
/// How many times an agent may change one card (#428), so it can't churn
/// one the person is reading.
pub(crate) const MAX_UPDATES: i64 = 10;
/// An answer that names no version (from an app before #428) is refused
/// this long after its card changed: the person may have pressed on what
/// it said before.
pub(crate) const UNSEEN_MS: i64 = 30_000;
/// What the person is told when they answer a version of a card that
/// isn't its latest.
pub(crate) const CHANGED: &str = "This card changed. Look at it again.";

/// Questions an agent may have waiting at once.
pub(crate) const MAX_PENDING: i64 = 3;
/// The longest title, in characters.
const MAX_TITLE: usize = 120;
/// The longest reason for withdrawing a question, in characters.
pub(crate) const MAX_REASON: usize = 300;

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

/// The migration that let an agent take back its own question once it no
/// longer applies (ask_withdraw, #407): state 'withdrawn', decided when it
/// was, and this, the agent's reason; '' for every other state.
pub(crate) const WITHDRAWN: &str = "
ALTER TABLE asks ADD COLUMN reason TEXT NOT NULL DEFAULT '';
";

/// The migration that let an agent change its own question while it waits
/// (ask_update, #428): which version it is (1 as asked, one more each
/// change), when it last changed (null if never) and the agent's reason
/// for that ('' for none).
pub(crate) const UPDATED: &str = "
ALTER TABLE asks ADD COLUMN revision INTEGER NOT NULL DEFAULT 1;
ALTER TABLE asks ADD COLUMN updated INTEGER;
ALTER TABLE asks ADD COLUMN update_reason TEXT NOT NULL DEFAULT '';
";

/// The migration that let an admin agent ask the person to approve a change
/// it can't make on its own (#423): what their approval does, as JSON
/// (`crate::card_actions::Action`); '' for an ordinary ask.
pub(crate) const ACTION: &str = "
ALTER TABLE asks ADD COLUMN action TEXT NOT NULL DEFAULT '';
";

/// The migration that kept such a card's two parts apart (Fugo, #423):
/// what Approve does in Claktory's words, and the agent's reason; '' on
/// every other card.
pub(crate) const ACTION_WORDS: &str = "
ALTER TABLE asks ADD COLUMN action_description TEXT NOT NULL DEFAULT '';
ALTER TABLE asks ADD COLUMN action_reason TEXT NOT NULL DEFAULT '';
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

const PROMPT: &str = "# Asking the person\nWhen you're genuinely blocked on something only the person can give you (a decision that's theirs, an approval, access or information you can't get any other way), ask them with ask_person: they see a card they answer or dismiss, and you're told which in an event (a dismissal without a comment quietly, with whatever next wakes you). Never use it for questions your best judgment can settle: decide, carry on, and say what you chose in your report. Don't ask in your chat text instead: the person may never see it.\nOne question or one action per card, kept short: what to do, where, and how long. Several decisions are several cards, never a list in one, and a checklist item that waits on them is one concern: if it bundles several, split it into items first, each with its own card. Something that takes them several actions goes as steps on one card: they see one step at a time, the next only once the one before is done. When it's about an item on a checklist, give item.\nWhen a question stops applying before they answer (you found the answer, the plan changed, it was settled elsewhere), take it back at once with ask_withdraw and a short reason, rather than leaving it to waste their time. That includes a question they answer in your chat instead of on its card: take the card back then, so they don't have to dismiss it too. When what you need changes but still needs them, change the card in place with ask_update instead of withdrawing it and asking again.";

const ABOUT: &str = "Ask the person something, as a card they answer or dismiss. Use it only when you're genuinely blocked on something only the person can give you: a decision that's theirs, an approval, access or information you can't get any other way. Never use it for questions your best judgment can settle: decide, carry on, and mention the choice in your report. One question or one action per card, never a list of decisions: several decisions are several cards, and at most 3 can wait at once. title: what you need, in a short line. body: what you need and why, standing alone (the person may read it hours later, away from your conversation); a command they should run goes in a fenced code block, which the card gives a Copy button. wants_text: true if you need a written answer; false (the default) if a yes, an approval, is enough. review: when the person asked to review or comment on a document (text or Markdown up to 2 MB, an image, a video or a PDF up to 64 MB), ask with review: its path (absolute, or relative to your working folder) instead of having someone else show it; their card opens the file beside your conversation, their comments on it come to you, and the answer says how many they left. item: the checklist item it's about (12 for #12, or Ana#12): give it whenever the question is about an item on a checklist, since that's how it shows with the item in their Needs you; if they dismiss it, you're told what to do with the item, and Claktory can have you ask again later. steps: when they have to do several things in order (a procedure), the steps, each one short action (2 to 12 of them): the card shows one step at a time with Done and Can't do this, you're told of each in an event, and the next shows only once the one before is done; a step they can't do stops it. title says what it's all for, body introduces it; not with wants_text or review. You're told their answer in an event, and a dismissal without a comment in a quiet one, which comes with whatever next wakes you; meanwhile carry on with whatever doesn't depend on it.";

const UPDATE_ABOUT: &str = "Change one of your questions still waiting for the person, in place, when what you need changes but still needs them: a detail was wrong, the plan moved, a step changed. id: the question's id, which its result gave; then only what changes: title, body, wants_text or review (\"\" for none) on a question, steps on a procedure (one with steps stays one, and one without can't get them); reason: optional, why it changed, in a short line they read on the card (at most 300 characters). Their card redraws where it is, marked updated, without counting again or notifying them again; an answer they gave to the old version is refused, so they look again. On a procedure, the steps already done stay done while their text is unchanged; from the first changed step on, they're asked again. At most 10 changes per card: change it only when it matters, and take back one that no longer applies with ask_withdraw instead. You can change only your own questions.";

const WITHDRAW_ABOUT: &str = "Take back one of your questions still waiting for the person (a card ask_person or another tool of yours opened), as soon as it no longer applies: you found the answer, the plan changed, or it was settled elsewhere. id: the question's id, which its result gave; reason: why, in a short line they read in Needs you's History (at most 300 characters). Without id, lists your questions waiting, with their ids. It leaves their Needs you at once and frees its place; a checklist item it was about stays as it is, so update the item yourself. You can take back only your own questions.";

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
        let update_parameters = json!({
            "type": "object",
            "required": ["id"],
            "properties": {
                "id": {"type": "string"},
                "title": {"type": "string"},
                "body": {"type": "string"},
                "wants_text": {"type": "boolean"},
                "review": {"type": "string"},
                "steps": {"type": "array", "items": {"type": "string", "minLength": 1}, "minItems": MIN_STEPS, "maxItems": MAX_STEPS},
                "reason": {"type": "string"},
            },
            "additionalProperties": false,
        });
        let withdraw_parameters = json!({
            "type": "object",
            "properties": {
                "id": {"type": "string"},
                "reason": {"type": "string"},
            },
            "additionalProperties": false,
        });
        let mut tools = vec![
            super::tool(TOOL, "Ask the person", ABOUT, parameters, super::typed_handler(factory, agent, ask)),
            super::tool(WITHDRAW_TOOL, "Withdraw a question", WITHDRAW_ABOUT, withdraw_parameters, super::typed_handler(factory, agent, withdraw)),
        ];
        // Alerts that can't wait (`crate::notify`), and changing a card in
        // place: not for an agent limited to a pipe, whose asks already
        // reach the person, at most CONTAINED_PER_HOUR an hour, in words
        // from outside.
        if !factory.pipes.contained(agent) {
            tools.insert(1, super::tool(UPDATE_TOOL, "Change a question", UPDATE_ABOUT, update_parameters, super::typed_handler(factory, agent, update)));
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
    let said = format!(
        "{said} Its id is {id}: if it stops applying before they answer on the card (they answer it in your chat, say), take it back with ask_withdraw, or change it with ask_update if what you need changes.{}",
        super::checklist::taken_for(linked, &person)
    );
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
    if !(matches!(tool, TOOL | CLEAR_TOOL | "extension_call") || crate::card_actions::TOOLS.contains(&tool)) || is_error {
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
        withdrawn: false,
        withdraw_reason: String::new(),
        revision: 1,
        updated_ms: 0,
        update_reason: String::new(),
        action_description: text("action_description"),
        action_reason: text("action_reason"),
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
    /// Withdrawn: the agent's reason.
    reason: String,
    /// Which version it is (#428): 1 as asked, one more each change.
    revision: i64,
    /// When its agent last changed it; None if it never did.
    updated: Option<i64>,
    /// Why, in the agent's words; '' for no reason given.
    update_reason: String,
    /// What approving it does (`crate::card_actions::Action`, as JSON); ''
    /// for an ordinary ask.
    action: String,
    /// Its two parts: what approving it does, in Claktory's words, and the
    /// agent's reason; '' for an ordinary ask.
    action_description: String,
    action_reason: String,
}

const COLUMNS: &str = "id, title, body, wants_text, state, answer, decided, taint, review, created, answer_attachments, item, steps, replied, clears, clears_incarnation, reason, revision, updated, update_reason, action, action_description, action_reason";
/// The column after COLUMNS, for a query that reads one more.
const AFTER_COLUMNS: usize = 23;

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
        reason: row.get(16)?,
        revision: row.get(17)?,
        updated: row.get(18)?,
        update_reason: row.get(19)?,
        action: row.get(20)?,
        action_description: row.get(21)?,
        action_reason: row.get(22)?,
    })
}

/// What ask_update may change on a question.
struct Changed {
    title: String,
    body: String,
    wants_text: bool,
    review: String,
    steps: Vec<Step>,
}

impl Row {
    fn clone_fields(&self) -> Changed {
        Changed {
            title: self.title.clone(),
            body: self.body.clone(),
            wants_text: self.wants_text,
            review: self.review.clone(),
            steps: self.steps.clone(),
        }
    }

    fn to_proto(&self) -> v1::AskPerson {
        let state = match self.state.as_str() {
            "answered" => State::Answered,
            "dismissed" | "cleared" | "withdrawn" => State::Dismissed,
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
            withdrawn: self.state == "withdrawn",
            withdraw_reason: self.reason.clone(),
            revision: u32::try_from(self.revision).unwrap_or(1),
            updated_ms: self.updated.unwrap_or_default(),
            update_reason: self.update_reason.clone(),
            action_description: self.action_description.clone(),
            action_reason: self.action_reason.clone(),
        }
    }
}

/// Shows the agent's decided asks as decided in its just-loaded
/// conversation, its procedures as far as they've gone, and the questions
/// it changed as they are now.
pub(crate) fn restore(store: &Store, agent: &str, transcript: &mut Transcript) -> rusqlite::Result<()> {
    let db = store.db();
    let mut statement = db
        .prepare(&format!("SELECT {COLUMNS} FROM asks WHERE agent = ?1 AND (state != 'pending' OR steps IS NOT NULL OR revision > 1) ORDER BY id"))?;
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
    if ask.state == "withdrawn" {
        return Err(RequestError::Ask("This question was withdrawn.".to_owned()));
    }
    if ask.state != "pending" {
        return Err(RequestError::Ask(format!("This question was already {}.", ask.state)));
    }
    seen(request.revision, ask.revision, ask.updated).map_err(RequestError::Ask)?;
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
    // A change an admin agent asked the person to approve (#423): approved,
    // it's made, so it must still be one that can be made.
    let acting = !ask.action.is_empty() && !dismiss && !request.reply;
    if acting {
        crate::card_actions::check(factory, &ask.action).map_err(RequestError::Ask)?;
    }
    // Drawings on a PDF's pages, answering or dismissing alike: uploads of
    // this agent's, as a message's attachments are.
    let attachments = factory.attachments(agent, &request.attachments)?;
    let stored = Some(&attachments).filter(|list| !list.is_empty()).and_then(|list| serde_json::to_string(list).ok());
    // Dismissing, the text is the person's optional comment.
    let state = if dismiss { "dismissed" } else { "answered" };
    let answer = text;
    let decided = now_ms();
    // Kept only if nothing answered or changed it meanwhile.
    let claimed = factory.store.db().execute(
        "UPDATE asks SET state = ?2, answer = ?3, decided = ?4, answer_attachments = ?5, replied = ?6 WHERE id = ?1 AND state = 'pending' AND revision = ?7",
        params![number, state, answer, decided, stored, request.reply, ask.revision],
    )?;
    if claimed == 0 {
        return Err(RequestError::Ask(lost_claim(factory, number)));
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
        factory.deliver_event(agent, text, told_event(&ask, None, summary));
        return Ok(());
    }
    if !ask.action.is_empty() {
        let made = if acting { Some(crate::card_actions::apply(factory, &ask.action).await) } else { None };
        let (summary, text) = crate::card_actions::told(&person, &ask.title, &ask.answer, ask.replied, made);
        factory.deliver_event(agent, text, told_event(&ask, None, summary));
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
    factory.deliver_event(agent, text, told_event(&ask, comments, summary));
    Ok(())
}

/// The event that tells the agent what became of its question: quiet when
/// the person dismissed it with nothing of theirs (no comment, drawing or
/// comment on the file it was about), as a card settled elsewhere is
/// (friend bug #19), so it waits for the agent's next wake; otherwise one
/// that wakes it.
fn told_event(ask: &Row, comments: Option<i64>, summary: String) -> factory_agent::Event {
    let bare = ask.state == "dismissed"
        && !ask.replied
        && ask.answer.trim().is_empty()
        && ask.answer_attachments.is_empty()
        && comments.unwrap_or(0) == 0;
    if bare { crate::needs_you::dismissed_event(summary) } else { host_event(summary) }
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
            |found| Ok((found.get::<_, String>(AFTER_COLUMNS)?, row(found)?)),
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
        "Asked {person}: they see a card to approve or dismiss clearing {name}'s conversation, and only their approval clears it. You'll be told which in an event. Its id is {id}: if it stops applying before they answer on the card (they answer it in your chat, say), take it back with ask_withdraw."
    );
    let mut details = json!({"ask": id, "title": title, "body": body, "wants_text": false, "clears": target});
    if let Some(taint) = taint {
        details["taint"] = taint;
    }
    Ok(ToolResult::text(said, details))
}

/// Opens a card asking the person to approve a change `asker` (an admin
/// agent, checked by its caller) can't make on its own (#423): `action`,
/// `crate::card_actions::Action` as JSON, is made when they approve it, and
/// nothing else makes it. It's one of `asker`'s questions waiting, and the
/// same change is asked once at a time. Its id.
pub(crate) fn ask_to_approve(
    factory: &Factory,
    asker: &str,
    title: &str,
    (description, reason): (&str, &str),
    action: &str,
) -> Result<String, String> {
    // Older apps show the body: both parts.
    let body = format!("{description} {reason}");
    let person = factory.lock().config.person().to_owned();
    // Read a pipe or a web page: the card says so, and where from.
    let routes = crate::pipes::agents::routes(factory, asker);
    let taint = (!routes.is_empty()).then(|| crate::pipes::agents::routes_json(&routes));
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
            "SELECT EXISTS (SELECT 1 FROM asks WHERE agent = ?1 AND state = 'pending' AND action = ?2)",
            params![asker, action],
            |row| row.get(0),
        )
        .map_err(couldnt)?;
    if asked {
        return Err(format!("You already asked {person} for this; it's waiting for them. Nothing new was asked."));
    }
    db.execute(
        "INSERT INTO asks (agent, title, body, wants_text, created, state, answer, taint, action, action_description, action_reason) VALUES (?1, ?2, ?3, 0, ?4, 'pending', '', ?5, ?6, ?7, ?8)",
        params![asker, title, body, now_ms(), taint.as_ref().map(serde_json::Value::to_string), action, description, reason],
    )
    .map_err(couldnt)?;
    Ok(db.last_insert_rowid().to_string())
}

#[derive(Deserialize)]
pub(crate) struct WithdrawArgs {
    #[serde(default)]
    pub(crate) id: Option<String>,
    #[serde(default)]
    pub(crate) reason: Option<String>,
}

/// A card's id as an agent gives it: "12", or "#12".
pub(crate) fn card_number(id: &str) -> Option<i64> {
    id.trim().trim_start_matches('#').trim().parse().ok().filter(|id| *id > 0)
}

/// The reason an agent gives for taking a card back, checked: one short
/// line the person reads.
pub(crate) fn withdraw_reason(reason: Option<&str>, person: &str) -> Result<String, String> {
    let reason = reason.unwrap_or_default().split_whitespace().collect::<Vec<_>>().join(" ");
    if reason.is_empty() {
        return Err(format!("Give reason: why it no longer applies, in a short line {person} reads. Nothing was withdrawn."));
    }
    if reason.chars().count() > MAX_REASON {
        return Err(format!("The reason is a short line, at most {MAX_REASON} characters. Nothing was withdrawn."));
    }
    Ok(reason)
}

/// Checklist item `key` ("agent#12") as `me` names it: "#12" on its own
/// checklist, "Ana#12" on another's.
pub(crate) fn item_named(factory: &Factory, me: &str, key: &str) -> String {
    let Some((whose, id)) = key.rsplit_once('#') else { return key.to_owned() };
    if whose == me {
        return format!("#{id}");
    }
    let name = factory.lock().config.agent(whose).map_or_else(|| whose.to_owned(), |agent| agent.name.clone());
    format!("{name}#{id}")
}

/// ask_withdraw (#407): takes back one of the agent's own questions still
/// waiting, with its reason, or lists them. Its card leaves Needs you at
/// once, stops counting against new ones, and shows as withdrawn; the item
/// it was about stays as it was, for the agent to update.
fn withdraw(factory: &Arc<Factory>, agent: &str, args: WithdrawArgs) -> Result<ToolResult, String> {
    let person = {
        let inner = factory.lock();
        super::caller(&inner.config, agent)?;
        inner.config.person().to_owned()
    };
    let couldnt = |error: rusqlite::Error| format!("Couldn't read your questions: {error}");
    let Some(given) = args.id.as_deref().map(str::trim).filter(|id| !id.is_empty()) else {
        let waiting = crate::store::query(
            &factory.store.db(),
            &format!("SELECT {COLUMNS} FROM asks WHERE agent = ?1 AND state = 'pending' ORDER BY id"),
            [agent],
            row,
        )
        .map_err(couldnt)?;
        if waiting.is_empty() {
            return Ok(ToolResult::text(format!("You have no questions waiting for {person}."), json!({})));
        }
        let mut said = format!("Your questions waiting for {person}; take one back with its id and a reason:");
        for ask in &waiting {
            let when = factory_agent::accounts::local_time(u64::try_from(ask.created).unwrap_or_default()).unwrap_or_default();
            said.push_str(&format!("\n- {}: \"{}\", asked {when}", ask.id, ask.title));
            if !ask.steps.is_empty() {
                let done = ask.steps.iter().filter(|step| !step.state.is_empty()).count();
                said.push_str(&format!(", {done} of {} steps done", ask.steps.len()));
            }
            if !ask.item.is_empty() {
                said.push_str(&format!(", about {}", item_named(factory, agent, &ask.item)));
            }
        }
        return Ok(ToolResult::text(said, json!({})));
    };
    let unknown = || format!("You have no question {given}: ask_withdraw without id lists yours. Nothing was withdrawn.");
    let number = card_number(given).ok_or_else(unknown)?;
    let reason = withdraw_reason(args.reason.as_deref(), &person)?;
    // Only the agent's own: another's is as unknown as one never asked.
    let found = factory
        .store
        .db()
        .query_row(&format!("SELECT {COLUMNS} FROM asks WHERE id = ?1 AND agent = ?2"), params![number, agent], row)
        .optional()
        .map_err(couldnt)?;
    let Some(mut ask) = found else { return Err(unknown()) };
    if ask.state != "pending" {
        let was = match ask.state.as_str() {
            "answered" if ask.replied => format!("{person} already replied to it"),
            "answered" => format!("{person} already answered it"),
            "dismissed" => format!("{person} already dismissed it"),
            "withdrawn" => "You already withdrew it".to_owned(),
            _ => "It's already closed".to_owned(),
        };
        return Err(format!("Question {number} isn't waiting: {was}. Nothing was withdrawn."));
    }
    let decided = now_ms();
    // Kept only if the person didn't answer it meanwhile.
    let claimed = factory
        .store
        .db()
        .execute(
            "UPDATE asks SET state = 'withdrawn', decided = ?3, reason = ?4 WHERE id = ?1 AND agent = ?2 AND state = 'pending'",
            params![number, agent, decided, reason],
        )
        .map_err(|error| format!("Couldn't withdraw it: {error}"))?;
    if claimed == 0 {
        return Err(format!("{person} just answered question {number}, so it wasn't withdrawn; you'll be told their answer."));
    }
    (ask.state, ask.decided, ask.reason) = ("withdrawn".to_owned(), Some(decided), reason);
    // Its card shows it withdrawn, and it no longer waits for the person.
    {
        let mut inner = factory.lock();
        let entry =
            inner.agents.get_mut(agent).and_then(|runtime| runtime.transcript.as_mut()).and_then(|transcript| transcript.set_ask(ask.to_proto()));
        if let Some(entry) = entry {
            factory.send_change(&inner, agent, crate::transcript::Change::Entry(entry));
        }
        factory.broadcast_workspace(&inner);
    }
    let mut said = format!(
        "Withdrew question {number}, \"{}\": it's gone from {person}'s Needs you, and their History shows it withdrawn by you, with your reason.",
        ask.title
    );
    if !ask.item.is_empty() {
        if let Some((whose, _)) = ask.item.rsplit_once('#') {
            factory.refresh_view(whose);
        }
        said.push_str(&format!(
            " It was about {}, which stays as it was: update it if it no longer waits on {person}.",
            item_named(factory, agent, &ask.item)
        ));
    }
    Ok(ToolResult::text(said, json!({})))
}

/// Whether an answer was given to the card as it is now (#428): the
/// version it names (AnswerAsk.revision, DecideProposal.revision) must be
/// the latest. One that names none, from an app before #428, is taken for
/// the latest unless the card changed in the last `UNSEEN_MS`. Err: what
/// the person is told.
pub(crate) fn seen(named: Option<u32>, revision: i64, updated: Option<i64>) -> Result<(), String> {
    let fresh = match named {
        // Before #428 a card was always version 1; 0 is how an older backend said so.
        Some(named) => i64::from(named.max(1)) == revision,
        None => revision <= 1 || updated.is_none_or(|at| now_ms() - at >= UNSEEN_MS),
    };
    if fresh { Ok(()) } else { Err(CHANGED.to_owned()) }
}

/// Why an answer's claim on question `id` found nothing to claim: it was
/// changed meanwhile, or decided.
fn lost_claim(factory: &Factory, id: i64) -> String {
    let state = factory.store.db().query_row("SELECT state FROM asks WHERE id = ?1", [id], |row| row.get::<_, String>(0));
    match state.as_deref() {
        Ok("pending") => CHANGED.to_owned(),
        Ok("withdrawn") => "This question was withdrawn.".to_owned(),
        _ => "This question was already answered.".to_owned(),
    }
}

/// The reason an agent gives for changing a card, checked: optional, one
/// short line the person reads on it.
pub(crate) fn update_reason(reason: Option<&str>) -> Result<String, String> {
    let reason = reason.unwrap_or_default().split_whitespace().collect::<Vec<_>>().join(" ");
    if reason.chars().count() > MAX_REASON {
        return Err(format!("The reason is a short line, at most {MAX_REASON} characters. Nothing was changed."));
    }
    Ok(reason)
}

/// What the agent is told when its card can't be changed any more.
pub(crate) fn too_many_updates(what: &str, number: i64) -> String {
    format!(
        "You've changed {what} {number} {MAX_UPDATES} times, the most one card can be changed, so it wasn't changed again. If it no longer says what you need, take it back and make a new one. Nothing was changed."
    )
}

#[derive(Deserialize)]
struct UpdateArgs {
    id: String,
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    body: Option<String>,
    #[serde(default)]
    wants_text: Option<bool>,
    #[serde(default)]
    review: Option<String>,
    #[serde(default)]
    steps: Option<Vec<String>>,
    #[serde(default)]
    reason: Option<String>,
}

/// A procedure's new steps, keeping those already done while they and every
/// step before them are unchanged: from the first changed one on, they're
/// asked again. Err: why they can't be used.
fn restepped(old: &[Step], new: &[String]) -> Result<Vec<Step>, String> {
    if !(MIN_STEPS..=MAX_STEPS).contains(&new.len()) {
        return Err(format!("steps: give {MIN_STEPS} to {MAX_STEPS} of them, each one action. Nothing was changed."));
    }
    let texts: Vec<String> = new.iter().map(|step| step.trim().to_owned()).collect();
    if let Some(at) = texts.iter().position(|text| text.is_empty() || text.chars().count() > MAX_STEP) {
        return Err(format!(
            "steps: step {} must say what to do in at most {MAX_STEP} characters; split a longer one in two. Nothing was changed.",
            at + 1
        ));
    }
    let kept = old.iter().zip(&texts).take_while(|(step, text)| step.state == "done" && step.text == **text).count();
    if kept == texts.len() {
        return Err("steps: the person already did every one of these, so there'd be nothing left for them to do: add the steps still to do, or take the question back. Nothing was changed.".to_owned());
    }
    Ok(texts
        .into_iter()
        .enumerate()
        .map(|(at, text)| if at < kept { old[at].clone() } else { Step { text, ..Step::default() } })
        .collect())
}

/// ask_update (#428): changes one of the agent's own questions still
/// waiting, in place. Its card redraws where it is, as a new version
/// (`revision`), marked updated with the agent's reason; it doesn't count
/// again in Needs you and nothing notifies the person again. An answer to
/// the version before is refused (`seen`).
fn update(factory: &Arc<Factory>, agent: &str, args: UpdateArgs) -> Result<ToolResult, String> {
    let person = {
        let inner = factory.lock();
        super::caller(&inner.config, agent)?;
        inner.config.person().to_owned()
    };
    // What a liaison or a curator asks comes from outside, at most a few an
    // hour: it asks anew rather than rewriting a card the person is reading.
    if factory.pipes.contained(agent) {
        return Err("You can't change a question: take it back with ask_withdraw and ask again if it still matters. Nothing was changed.".to_owned());
    }
    let given = args.id.trim();
    let unknown = || format!("You have no question {given}: ask_withdraw without id lists yours. Nothing was changed.");
    let number = card_number(given).ok_or_else(unknown)?;
    let reason = update_reason(args.reason.as_deref())?;
    // Only the agent's own: another's is as unknown as one never asked.
    let found = factory
        .store
        .db()
        .query_row(&format!("SELECT {COLUMNS} FROM asks WHERE id = ?1 AND agent = ?2"), params![number, agent], row)
        .optional()
        .map_err(|error| format!("Couldn't read your questions: {error}"))?;
    let Some(mut ask) = found else { return Err(unknown()) };
    if ask.state != "pending" {
        let was = match ask.state.as_str() {
            "answered" if ask.replied => format!("{person} already replied to it"),
            "answered" => format!("{person} already answered it"),
            "dismissed" => format!("{person} already dismissed it"),
            "withdrawn" => "You withdrew it".to_owned(),
            _ => "It's already closed".to_owned(),
        };
        return Err(format!("Question {number} isn't waiting: {was}. Nothing was changed."));
    }
    if !ask.clears.is_empty() {
        return Err(format!(
            "Question {number} asks {person} to clear a conversation, which can't be changed: take it back with ask_withdraw if it no longer applies. Nothing was changed."
        ));
    }
    // What the person approves is what happens (Fugo): a card that makes a
    // change keeps the words it was asked with.
    if !ask.action.is_empty() {
        return Err(format!(
            "Question {number} makes a change when {person} approves it, so it can't be changed: take it back with ask_withdraw and ask again. Nothing was changed."
        ));
    }
    if ask.revision > MAX_UPDATES {
        return Err(too_many_updates("question", number));
    }
    if args.title.is_none() && args.body.is_none() && args.wants_text.is_none() && args.review.is_none() && args.steps.is_none() {
        return Err("Give what changes: title, body, wants_text, review or steps. Nothing was changed.".to_owned());
    }
    let procedure = !ask.steps.is_empty();
    if procedure && (args.wants_text.is_some() || args.review.is_some()) {
        return Err(format!(
            "Question {number} is a procedure: change its title, body or steps; it takes no wants_text or review. Nothing was changed."
        ));
    }
    if !procedure && args.steps.is_some() {
        return Err(format!(
            "Question {number} has no steps, so it can't be given any: change its title, body, wants_text or review, or take it back and ask with steps. Nothing was changed."
        ));
    }
    let mut changed = ask.clone_fields();
    if let Some(title) = &args.title {
        let title = title.split_whitespace().collect::<Vec<_>>().join(" ");
        if title.is_empty() {
            return Err("The title can't be empty: say what you need in a short line. Nothing was changed.".to_owned());
        }
        if title.chars().count() > MAX_TITLE {
            return Err(format!("The title is a short line (at most {MAX_TITLE} characters); put the rest in the body. Nothing was changed."));
        }
        changed.title = title;
    }
    if let Some(body) = &args.body {
        let body = body.trim().to_owned();
        if body.is_empty() {
            return Err("The body can't be empty: say what you need, and why. Nothing was changed.".to_owned());
        }
        changed.body = body;
    }
    if let Some(wants_text) = args.wants_text {
        changed.wants_text = wants_text;
    }
    if let Some(review) = &args.review {
        changed.review = match review.trim() {
            "" => String::new(),
            path => review_file(factory, agent, path).map_err(|error| error.replace("Nothing was asked.", "Nothing was changed."))?,
        };
    }
    if let Some(steps) = &args.steps {
        changed.steps = restepped(&ask.steps, steps)?;
    }
    let same = changed.title == ask.title
        && changed.body == ask.body
        && changed.wants_text == ask.wants_text
        && changed.review == ask.review
        && serde_json::to_string(&changed.steps).ok() == serde_json::to_string(&ask.steps).ok();
    if same {
        return Err(format!("That's what question {number} already says. Nothing was changed."));
    }
    // Read a pipe or a web page since it asked: the card says so now.
    let routes = crate::pipes::agents::routes(factory, agent);
    if !routes.is_empty() {
        ask.taint = Some(crate::pipes::agents::routes_json(&routes).to_string());
    }
    let now = now_ms();
    let steps = (!changed.steps.is_empty()).then(|| serde_json::to_string(&changed.steps).unwrap_or_default());
    // Kept only if the person didn't answer it, nor the agent change it, meanwhile.
    let claimed = factory
        .store
        .db()
        .execute(
            "UPDATE asks SET title = ?3, body = ?4, wants_text = ?5, review = ?6, steps = ?7, taint = ?8,
             revision = revision + 1, updated = ?9, update_reason = ?10
             WHERE id = ?1 AND agent = ?2 AND state = 'pending' AND revision = ?11",
            params![number, agent, changed.title, changed.body, changed.wants_text, changed.review, steps, ask.taint, now, reason, ask.revision],
        )
        .map_err(|error| format!("Couldn't change it: {error}"))?;
    if claimed == 0 {
        return Err(format!("{person} just answered question {number}, so it wasn't changed; you'll be told their answer."));
    }
    let kept = changed.steps.iter().take_while(|step| step.state == "done").count();
    let moved_back = procedure && kept < ask.steps.iter().take_while(|step| step.state == "done").count();
    (ask.title, ask.body, ask.wants_text, ask.review, ask.steps) = (changed.title, changed.body, changed.wants_text, changed.review, changed.steps);
    (ask.revision, ask.updated, ask.update_reason) = (ask.revision + 1, Some(now), reason);
    // Its card redraws in place; it still waits, at the same seq.
    {
        let mut inner = factory.lock();
        let entry =
            inner.agents.get_mut(agent).and_then(|runtime| runtime.transcript.as_mut()).and_then(|transcript| transcript.set_ask(ask.to_proto()));
        if let Some(entry) = entry {
            factory.send_change(&inner, agent, crate::transcript::Change::Entry(entry));
        }
        factory.broadcast_workspace(&inner);
    }
    if let Some((whose, _)) = ask.item.rsplit_once('#') {
        factory.refresh_view(whose);
    }
    let left = MAX_UPDATES - (ask.revision - 1);
    let mut said = format!(
        "Changed question {number}, \"{}\": {person}'s card shows it as it is now, marked updated{}, without notifying them again. An answer they give to the version before is refused, so they look again.",
        ask.title,
        if ask.update_reason.is_empty() { String::new() } else { " with your reason".to_owned() }
    );
    if procedure {
        let count = ask.steps.len();
        said.push_str(&match kept {
            0 => format!(" They go through its {count} steps from the first."),
            1 => format!(" Step 1 stays done; they see step 2 of {count} next."),
            _ => format!(" Steps 1 to {kept} stay done; they see step {} of {count} next.", kept + 1),
        });
        if moved_back {
            said.push_str(" That's a step they had already done, changed, so they're asked it again.");
        }
    }
    said.push_str(&match left {
        0 => " That was the last change this card can take.".to_owned(),
        1 => " It can be changed once more.".to_owned(),
        _ => format!(" It can be changed {left} more times."),
    });
    Ok(ToolResult::text(said, json!({})))
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
    // Kept only if nothing answered it meanwhile (another device, a second
    // tap), and its agent didn't change it.
    let claimed = factory.store.db().execute(
        "UPDATE asks SET steps = ?2, state = ?3, answer = ?4, decided = ?5 WHERE id = ?1 AND state = 'pending' AND steps = ?6 AND revision = ?7",
        params![ask.id, serde_json::to_string(&ask.steps).unwrap_or_default(), state, comment, decided, before, ask.revision],
    )?;
    if claimed == 0 {
        let changed = factory
            .store
            .db()
            .query_row("SELECT revision FROM asks WHERE id = ?1", [ask.id], |row| row.get::<_, i64>(0))
            .is_ok_and(|now| now != ask.revision);
        return Err(RequestError::Ask(if changed { CHANGED.to_owned() } else { "That step was already answered.".to_owned() }));
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
            reason: String::new(),
            revision: 1,
            updated: None,
            update_reason: String::new(),
            action: String::new(),
            action_description: String::new(),
            action_reason: String::new(),
        }
    }

    #[test]
    fn a_changed_procedure_keeps_the_steps_done_up_to_the_first_change() {
        let step = |text: &str, state: &str| Step { text: text.to_owned(), state: state.to_owned(), comment: String::new(), decided: 0 };
        let old = [step("Open Settings", "done"), step("Sign in", "done"), step("Copy the key", ""), step("Paste it", "")];
        let texts = |list: &[&str]| list.iter().map(|text| (*text).to_owned()).collect::<Vec<_>>();
        let kept = |steps: &[Step]| steps.iter().map(|step| step.state.clone()).collect::<Vec<_>>();
        // A later step changed: both done steps stay done.
        let new = restepped(&old, &texts(&["Open Settings", "Sign in", "Copy the token", "Paste it"])).expect("steps");
        assert_eq!(kept(&new), ["done", "done", "", ""]);
        // A done step changed: it and every step after it are asked again.
        let new = restepped(&old, &texts(&["Open Settings", "Sign in with SSO", "Copy the key"])).expect("steps");
        assert_eq!(kept(&new), ["done", "", ""]);
        // Nothing left to do is refused, as are too few steps.
        assert!(restepped(&old, &texts(&["Open Settings", "Sign in"])).is_err());
        assert!(restepped(&old, &texts(&["Open Settings"])).is_err());
    }

    #[test]
    fn an_answer_counts_only_for_the_version_it_names() {
        let now = now_ms();
        assert!(seen(Some(1), 1, None).is_ok());
        assert!(seen(Some(0), 1, None).is_ok(), "an app reading an older backend's 0 means 1");
        assert_eq!(seen(Some(1), 2, Some(now - 3_600_000)), Err(CHANGED.to_owned()));
        assert!(seen(Some(2), 2, Some(now)).is_ok());
        // An app before #428 names none: refused only just after a change.
        assert!(seen(None, 1, None).is_ok());
        assert!(seen(None, 3, Some(now - UNSEEN_MS)).is_ok());
        assert_eq!(seen(None, 3, Some(now - 1_000)), Err(CHANGED.to_owned()));
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
