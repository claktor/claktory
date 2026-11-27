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
/// Answers the person's words on one of the agent's cards, where they are (#430).
pub(crate) const REPLY_TOOL: &str = "ask_reply";
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
    description: "Lets it ask you when it's blocked on something only you can give: a request you answer or dismiss, instead of a question lost in its chat.",
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

/// The migration that let a review card follow its file (Bruno 46189):
/// the file's content when it was asked or its agent last changed it, or
/// Claktory last marked it (`crate::review_watch::fingerprint`; '' for a
/// card from before), how many of its changes were Claktory's for the file
/// (they don't count against its agent's), and Claktory's words for the
/// last change when it was one ('' when its agent made it).
pub(crate) const FOLLOWS_FILE: &str = "
ALTER TABLE asks ADD COLUMN review_seen TEXT NOT NULL DEFAULT '';
ALTER TABLE asks ADD COLUMN file_updates INTEGER NOT NULL DEFAULT 0;
ALTER TABLE asks ADD COLUMN update_note TEXT NOT NULL DEFAULT '';
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

/// The migration that let the person and the agent talk on a card (#430):
/// `talk`, the talk so far (a JSON list of `ThreadMessage`; null until the agent
/// first answers on it with ask_reply), and 1 when the person ended it with
/// Resolve (state 'answered'), talked through without a decision.
pub(crate) const THREAD: &str = "
ALTER TABLE asks ADD COLUMN talk TEXT;
ALTER TABLE asks ADD COLUMN resolved INTEGER NOT NULL DEFAULT 0;
";

/// The migration that kept why an approved card's change couldn't be made
/// (its action, or clearing a conversation); '' when it was, or wasn't
/// approved.
pub(crate) const ACTION_ERROR: &str = "
ALTER TABLE asks ADD COLUMN action_error TEXT NOT NULL DEFAULT '';
";

/// One message of the talk on a card (#430), as kept.
#[derive(Clone, Debug, Deserialize, Serialize)]
struct ThreadMessage {
    /// The agent's; false: the person's.
    agent: bool,
    text: String,
    at: i64,
    /// The person's files on that turn (#511), uploads of the agent's.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    attachments: Vec<factory_agent::Attachment>,
    /// On a procedure, the step that turn is about (1 for the first, #533);
    /// None for an ordinary card, and for turns before it was kept.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    step: Option<u32>,
}

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
    /// The files the person answered it with (#511).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    comment_attachments: Vec<factory_agent::Attachment>,
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
        v1::AskStep {
            text: self.text.clone(),
            state: state.into(),
            comment: self.comment.clone(),
            decided_ms: self.decided,
            comment_attachments: self.comment_attachments.iter().map(crate::transcript::attachment).collect(),
        }
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

const PROMPT: &str = "# Asking the person\nWhen you're genuinely blocked on something only the person can give you (a decision that's theirs, an approval, access or information you can't get any other way), ask them with ask_person: they see a request they answer or dismiss, and you're told which in an event (a dismissal without a comment quietly, with whatever next wakes you). Never use it for questions your best judgment can settle: decide, carry on, and say what you chose in your report. Don't ask in your chat text instead: the person may never see it.\nOne question or one action per request, kept short: what to do, where, and how long. Several decisions are several requests, never a list in one, and a checklist item that waits on them is one concern: if it bundles several, split it into items first, each with its own request. Something that takes them several actions goes as steps in one request: they see one step at a time, the next only once the one before is done. When it's about an item on a checklist, give item.\nWhen they ask to be sent, shown or notified of a report, a write-up, a file or anything else to read, send it as a review request (ask_person with review): it waits in their Needs you, where a notification would disappear. show only opens it beside your conversation, in addition, never instead; notify_person is for news with nothing to read, and points at the request when there is one.\nWhen a question stops applying before they answer (you found the answer, the plan changed, it was settled elsewhere), take it back at once with ask_withdraw and a short reason, rather than leaving it to waste their time. That includes a question they answer in your chat instead of in its request: take the request back then, so they don't have to dismiss it too. When what you need changes but still needs them, change the request in place with ask_update instead of withdrawing it and asking again. When they answer a request with words that need an answer from you (a question back, a doubt), answer in the request with ask_reply, where they are, not in your chat.\nApprove always means the request as it reads when they tap it. So when your answer in a request changes what you ask, give the new title or body in that same ask_reply, and the request shows what they'd be approving. Say a request asks \"OK to spend up to $20 on a paid detector?\", they reply \"no paid tools\", and you propose a free one instead: answer with the new title \"OK to use the free detector?\" and a body that says so, never only the words, or their Approve still reads as the $20. If they approve or answer a request after your reply without the request changing, you're told the talk with it: act on where the talk ended, and ask in the request if that's unclear. A request that makes a change when approved (one that changes settings, a pipe, an app, clears a conversation) does exactly what it shows: it can't be changed or answered on, so to ask for something else, ask again.\nWhen their words in a request settle it (they told you what to do, and nothing waits on them any more), answer with ask_reply and resolve: true: your words close the request as talked through, instead of sending it back to wait in their Needs you.\nOn a procedure, their words on a step, or Can't do this, mean that step needs fixing or an answer, with nothing marked done: fix it in the same request with ask_reply and steps, or answer with text, rather than asking again. After Can't do this, find a way without them if they can't; if they won't, respect it and stop the request there with ask_reply's resolve.";

const ABOUT: &str = "Ask the person something, as a request they answer or dismiss. Use it only when you're genuinely blocked on something only the person can give you: a decision that's theirs, an approval, access or information you can't get any other way. Never use it for questions your best judgment can settle: decide, carry on, and mention the choice in your report. One question or one action per request, never a list of decisions: several decisions are several requests, and at most 3 can wait at once. title: what you need, in a short line. body: what you need and why, standing alone (the person may read it hours later, away from your conversation); a command they should run on this computer goes in command instead; one for somewhere else goes in a fenced code block, which the request gives a Copy button. wants_text: true if you need a written answer; false (the default) if a yes, an approval, is enough. review: when the person asked to review or comment on a document, or to be sent, shown or notified of a report, a write-up or a file to read (text or Markdown up to 2 MB, an image, a video or a PDF up to 64 MB), ask with review: its path (absolute, or relative to your working folder), never a notification or show alone, and never having someone else show it; their request opens the file beside your conversation, their comments on it come to you, and the answer says how many they left. If you change the file while it waits, Claktory marks it updated by itself, so they review the file as it reads now; no ask_update needed for that. item: the checklist item it's about (12 for #12, or Ana#12): give it whenever the question is about an item on a checklist, since that's how it shows with the item in their Needs you; if they dismiss it, you're told what to do with the item, and Claktory can have you ask again later. steps: when they have to do several things in order (a procedure), the steps, each one short action (2 to 12 of them): the request shows one step at a time with Done and Can't do this, you're told of each in an event, and the next shows only once the one before is done; a step they can't do stops it. title says what it's all for, body introduces it; not with wants_text or review. command: a command for the person to run on this computer, as sh -c takes it (at most 4000 characters), in cwd (an absolute folder; yours unless given), stopped after timeout seconds (120 unless given, at most 600); title and body say why. Their request shows it exactly, with warnings where it's risky; Execute runs it once, as them, with no terminal (a password or host-key prompt fails), and you're told its exit code and what it printed. It goes alone, without wants_text, review, steps or item, and can't be changed: withdraw it and ask again. You're told their answer in an event, and a dismissal without a comment in a quiet one, which comes with whatever next wakes you; meanwhile carry on with whatever doesn't depend on it.";

const UPDATE_ABOUT: &str = "Change one of your questions still waiting for the person, in place, when what you need changes but still needs them: a detail was wrong, the plan moved, a step changed. A change to the file a review question shows needs none: Claktory marks the question updated by itself once the file stops changing. id: the question's id, which its result gave; then only what changes: title, body, wants_text or review (\"\" for none) on a question, steps on a procedure (one with steps stays one, and one without can't get them); reason: optional, why it changed, in a short line they read in the request (at most 300 characters). Their request redraws where it is, marked updated, without counting again or notifying them again; an answer they gave to the old version is refused, so they look again. On a procedure, the steps already done stay done while their text is unchanged; from the first changed step on, they're asked again. At most 10 changes per card: change it only when it matters, and take back one that no longer applies with ask_withdraw instead. You can change only your own questions.";

const REPLY_ABOUT: &str = "Answer what the person wrote in one of your requests (a reply, or their comment on approving or dismissing it), in the request, where they are: in Needs you or your conversation, rather than in your chat, which they may not be reading. id: the question's id; text: your answer, short and standing alone (Markdown, at most 4000 characters); title, body or steps: the request's new ones whenever your answer changes what you ask, since Approve always means the request as it reads: a request left unchanged still asks its old words (a procedure keeps the steps already done while unchanged, and asks again one they couldn't do). If they then approve or answer it unchanged, you're told the talk with it: act on where it ended. On a procedure they replied on, or couldn't do a step of, steps with that step rewritten fixes it, and the request opens again at that step; text alone answers them there. The request keeps the talk (their words, then yours), opens again in their Needs you as a new question does (a procedure on its way, answered on a step's comment, stays at its step and comes back to the top), and they answer it, reply again, or resolve it (talked through, with no decision); you're told which. Only after they wrote something, or couldn't do a step, never in a request that makes a change when approved: it does exactly what it shows, so ask again instead. resolve: true when their words settle it, so nothing waits on them any more: your answer closes the request as talked through (Resolved, as their own Resolve does) instead of opening it again; it stays in the request, History shows you closed it, and nothing is approved or run (in a request they already approved or dismissed, the decision stands and your words are only added). Not with title, body or steps. If the talk started other work, say so in your answer (\"Started: a separate plan for X.\"). It shares a request's 10 changes with ask_update.";

const WITHDRAW_ABOUT: &str = "Take back one of your questions still waiting for the person (a request ask_person or another tool of yours opened), as soon as it no longer applies: you found the answer, the plan changed, or it was settled elsewhere. id: the question's id, which its result gave; reason: why, in a short line they read in Needs you's History (at most 300 characters). Without id, lists your questions waiting, with their ids. It leaves their Needs you at once and frees its place; a checklist item it was about stays as it is, so update the item yourself. You can take back only your own questions.";

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
                "command": {"type": "string", "minLength": 1},
                "cwd": {"type": "string"},
                "timeout": {"type": "integer", "minimum": 1, "maximum": crate::command_cards::MAX_TIMEOUT},
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
        let reply_parameters = json!({
            "type": "object",
            "required": ["id", "text"],
            "properties": {
                "id": {"type": "string"},
                "text": {"type": "string", "minLength": 1},
                "title": {"type": "string"},
                "body": {"type": "string"},
                "steps": {"type": "array", "items": {"type": "string", "minLength": 1}, "minItems": MIN_STEPS, "maxItems": MAX_STEPS},
                "resolve": {"type": "boolean"},
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
            tools.insert(2, super::tool(REPLY_TOOL, "Answer in a request", REPLY_ABOUT, reply_parameters, super::typed_handler(factory, agent, reply)));
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
    /// A command for the person to run on this computer (Execute,
    /// `crate::command_cards`), its folder and its time limit in seconds.
    #[serde(default)]
    command: Option<String>,
    #[serde(default)]
    cwd: Option<String>,
    #[serde(default)]
    timeout: Option<u32>,
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
    if let Some(command) = args.command.clone() {
        return ask_to_run(factory, agent, args, &title, &body, &command);
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
    // What the file reads as now: the card follows it (`file_changed`).
    let seen = crate::review_watch::fingerprint(&review);
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
            "INSERT INTO asks (agent, title, body, wants_text, created, state, answer, taint, review, item, steps, review_seen) VALUES (?1, ?2, ?3, ?4, ?5, 'pending', '', ?6, ?7, ?8, ?9, ?10)",
            params![
                agent,
                title,
                body,
                args.wants_text,
                now_ms(),
                taint.as_ref().map(serde_json::Value::to_string),
                review,
                item,
                (!steps.is_empty()).then(|| serde_json::to_string(&steps).unwrap_or_default()),
                seen
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
            "Asked {person} to go through {} steps: their request shows one at a time, the next once the one before is done. You're told of each in an event, and if they can't do one, which stops it; meanwhile carry on with whatever doesn't depend on it.",
            steps.len()
        )
    } else if review.is_empty() {
        format!(
            "Asked {person}: they see it as a request to answer or dismiss. You'll be told which in an event; meanwhile carry on with whatever doesn't depend on it."
        )
    } else {
        format!(
            "Asked {person} to review {review}: their request opens it beside your conversation, and their comments on it come to you. You'll be told when they answer or dismiss it; meanwhile carry on with whatever doesn't depend on it."
        )
    };
    let said = format!(
        "{said} Its id is {id}: if it stops applying before they answer in the request (they answer it in your chat, say), take it back with ask_withdraw, or change it with ask_update if what you need changes.{}",
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

/// An Execute card (`crate::command_cards`): the person runs `command` on
/// this computer with one tap, or dismisses it. Refused from an agent that
/// has read from outside, or is limited to a pipe.
fn ask_to_run(factory: &Arc<Factory>, agent: &str, args: AskArgs, title: &str, body: &str, command: &str) -> Result<ToolResult, String> {
    if args.wants_text || args.review.is_some() || args.steps.is_some() || args.item.is_some() {
        return Err("command goes alone: without wants_text, review, steps or item. Nothing was asked.".to_owned());
    }
    let (person, folder) = {
        let inner = factory.lock();
        let config = super::caller(&inner.config, agent)?;
        let folder = crate::runner::places(&factory.home, &inner.config, config, &[]).folder;
        (inner.config.person().to_owned(), folder)
    };
    if factory.pipes.contained(agent) {
        return Err(format!("You can't ask {person} to run a command. Nothing was asked."));
    }
    if !factory.pipes.tainted(agent).is_empty() {
        let read = crate::pipes::agents::what_was_read(factory, agent);
        return Err(format!(
            "You've read {read}, which may hold someone else's instructions, so you can't ask {person} to run a command until your conversation starts afresh. Nothing was asked."
        ));
    }
    let cwd = args.cwd.as_deref().map(str::trim).filter(|cwd| !cwd.is_empty()).map_or(folder, std::path::PathBuf::from);
    let prepared = crate::command_cards::prepare(command, &cwd, args.timeout).map_err(|why| format!("{why} Nothing was asked."))?;
    let action = serde_json::to_string(&crate::card_actions::Action::Execute(Box::new(prepared.clone())))
        .map_err(|error| format!("Couldn't record the question: {error}"))?;
    let id = {
        let db = factory.store.db();
        let waiting: i64 = db
            .query_row("SELECT COUNT(*) FROM asks WHERE agent = ?1 AND state = 'pending'", [agent], |row| row.get(0))
            .map_err(|error| format!("Couldn't record the question: {error}"))?;
        if waiting >= MAX_PENDING {
            return Err(format!(
                "You already have {waiting} questions waiting for {person}, the most you can: wait for an answer before asking more. Nothing was asked."
            ));
        }
        db.execute(
            "INSERT INTO asks (agent, title, body, wants_text, created, state, answer, action) VALUES (?1, ?2, ?3, 0, ?4, 'pending', '', ?5)",
            params![agent, title, body, now_ms(), action],
        )
        .map_err(|error| format!("Couldn't record the question: {error}"))?;
        db.last_insert_rowid().to_string()
    };
    let warned = if prepared.warnings.is_empty() { "" } else { ", its warnings," };
    let said = format!(
        "Asked {person} to run it on {}: their request shows the command exactly, where it runs{warned} and your title and body as your reason. If they tap Execute, it runs once, as them, and you're told its exit code and what it printed; if they dismiss it, you're told that. It can't be changed: to change it, take it back with ask_withdraw (id {id}) and ask again.",
        prepared.machine
    );
    let details = json!({"ask": id, "title": title, "body": body, "wants_text": false, "command": prepared});
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
    if !(matches!(
        tool,
        TOOL | CLEAR_TOOL | "extension_call" | crate::pipes::approval_card::TOOL | crate::pipes::trust_card::TOOL
    ) || crate::errands::handoff::TOOLS.contains(&tool)
        || crate::card_actions::TOOLS.contains(&tool))
        || is_error
    {
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
        update_note: String::new(),
        action_description: text("action_description"),
        action_reason: text("action_reason"),
        thread: Vec::new(),
        resolved: false,
        action_error: String::new(),
        // A pipe's approval card (#471 G1).
        pipe_approval: crate::pipes::approval_card::proposal_in(details),
        command: details
            .get("command")
            .and_then(|command| serde_json::from_value::<crate::command_cards::Command>(command.clone()).ok())
            .map(|command| crate::command_cards::to_proto(&command, None)),
        pipe_trust: crate::pipes::trust_card::proposal_in(details),
        approve_follows_talk: false,
        // An errand's handoff card (#435), and whether it's a code card (W8).
        handoff: crate::errands::handoff::in_details(details),
        code: details.get("code").and_then(serde_json::Value::as_bool).unwrap_or(false),
    })
}

/// An ask as recorded.
#[derive(Clone)]
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
    /// The talk on it so far (#430); empty until its agent answers on it.
    thread: Vec<ThreadMessage>,
    /// Ended with Resolve (#430).
    resolved: bool,
    /// Approved, but its change couldn't be made: why; '' otherwise.
    action_error: String,
    /// An Execute card's run, once read (`command_cards::run_of`).
    run: Option<crate::command_cards::Run>,
    /// Of `revision`'s changes, those Claktory made because its review file
    /// changed (`file_changed`): they don't count against its agent's.
    file_updates: i64,
    /// Claktory's words for its last change, when it was one; ''.
    update_note: String,
}

const COLUMNS: &str = "id, title, body, wants_text, state, answer, decided, taint, review, created, answer_attachments, item, steps, replied, clears, clears_incarnation, reason, revision, updated, update_reason, action, action_description, action_reason, talk, resolved, action_error, file_updates, update_note";
/// The column after COLUMNS, for a query that reads one more.
const AFTER_COLUMNS: usize = 28;

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
        thread: row.get::<_, Option<String>>(23)?.and_then(|thread| serde_json::from_str(&thread).ok()).unwrap_or_default(),
        resolved: row.get(24)?,
        action_error: row.get(25)?,
        run: None,
        file_updates: row.get(26)?,
        update_note: row.get(27)?,
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

    /// The changes its agent made (ask_update, answers on it), not counting
    /// Claktory's for its file.
    fn agent_changes(&self) -> i64 {
        self.revision - self.file_updates
    }

    /// Approving or answering it makes no change of its own (#423's
    /// actions, clearing a conversation): what it means can be talked over.
    fn plain(&self) -> bool {
        self.action.is_empty() && self.clears.is_empty()
    }

    /// Their answer, or the step they last answered, came with words or
    /// files (#511): something of theirs its agent can answer on the card.
    fn person_said(&self) -> bool {
        !self.answer.trim().is_empty() || !self.answer_attachments.is_empty()
    }

    /// On a procedure, the step the person's last words are about (1 for the
    /// first, #533): the step they last did, when they did it with words
    /// (`on_step`); otherwise the one waiting, or the one they couldn't do.
    fn talk_step(&self, on_step: bool) -> Option<u32> {
        let at = if on_step {
            self.steps.iter().enumerate().filter(|(_, step)| step.state == "done").max_by_key(|(_, step)| step.decided).map(|(at, _)| at)
        } else {
            self.steps.iter().position(|step| step.state.is_empty() || step.state == "cant")
        }?;
        u32::try_from(at + 1).ok()
    }

    /// Its agent closed it (`close`), rather than the person resolving it:
    /// its words came last, at or after the decision (a person's Resolve
    /// adds none, and comes after the agent's last reply).
    fn closed_by_agent(&self) -> bool {
        self.resolved && self.thread.last().is_some_and(|last| last.agent && self.decided.is_none_or(|decided| last.at >= decided))
    }

    /// A procedure stopped at a step they couldn't do (#533 N3): its agent
    /// may fix that step on the card, words or not.
    fn stopped(&self) -> bool {
        self.steps.iter().any(|step| step.state == "cant")
    }

    /// Approving or answering it now agrees with where its talk ended (#492,
    /// AskPerson.approve_follows_talk): a plain card waiting, with no steps,
    /// whose talk ends with its agent's reply made after its words last
    /// changed (a reply that gave a new title or body changed them itself).
    fn follows_talk(&self) -> bool {
        self.state == "pending"
            && self.plain()
            && self.steps.is_empty()
            && self.thread.last().is_some_and(|last| last.agent && self.updated.is_none_or(|changed| last.at > changed))
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
            // `reason` is also an agent's words closing it (#496): History's.
            withdraw_reason: if self.state == "withdrawn" { self.reason.clone() } else { String::new() },
            revision: u32::try_from(self.revision).unwrap_or(1),
            updated_ms: self.updated.unwrap_or_default(),
            update_reason: self.update_reason.clone(),
            update_note: self.update_note.clone(),
            action_description: self.action_description.clone(),
            action_reason: self.action_reason.clone(),
            thread: self
                .thread
                .iter()
                .map(|message| v1::AskThreadMessage {
                    from_agent: message.agent,
                    text: message.text.clone(),
                    at_ms: message.at,
                    attachments: message.attachments.iter().map(crate::transcript::attachment).collect(),
                    step: message.step.unwrap_or_default(),
                })
                .collect(),
            resolved: self.resolved,
            action_error: self.action_error.clone(),
            pipe_approval: crate::pipes::approval_card::proposal_of(&self.action),
            pipe_trust: crate::pipes::trust_card::proposal_of(&self.action),
            approve_follows_talk: self.follows_talk(),
            command: self.execute().map(|command| crate::command_cards::to_proto(&command, self.run.as_ref())),
            handoff: self.handoff().map(|handoff| handoff.wire()),
            code: self.handoff().is_some_and(|handoff| handoff.kind == v1::ErrandHandoffKind::Code as i32),
        }
    }

    /// What it runs, when it's an Execute card.
    fn execute(&self) -> Option<crate::command_cards::Command> {
        match serde_json::from_str(&self.action) {
            Ok(crate::card_actions::Action::Execute(command)) => Some(*command),
            _ => None,
        }
    }

    /// What it hands the person, when it's an errand's handoff card.
    fn handoff(&self) -> Option<crate::errands::HandoffRow> {
        match serde_json::from_str(&self.action) {
            Ok(crate::card_actions::Action::Handoff { handoff }) => Some(*handoff),
            _ => None,
        }
    }

    /// With its run, when it's an Execute card that ran.
    fn with_run(mut self, runs: &crate::command_cards::Runs, db: &rusqlite::Connection) -> Row {
        if self.execute().is_some() {
            self.run = crate::command_cards::run_of(runs, db, self.id);
        }
        self
    }
}

/// Shows card `id` of `agent`'s as it is now in its conversation; and in
/// the workspace (Needs you) too unless `only_entry`, for a run's output
/// as it comes.
pub(crate) fn show(factory: &Arc<Factory>, agent: &str, id: i64, only_entry: bool) {
    let found = {
        let db = factory.store.db();
        db.query_row(&format!("SELECT {COLUMNS} FROM asks WHERE id = ?1 AND agent = ?2"), params![id, agent], row)
            .optional()
            .ok()
            .flatten()
            .map(|ask| ask.with_run(&factory.command_runs, &db))
    };
    let Some(ask) = found else { return };
    // Its conversation as loaded; one not loaded shows it when it is (`restore`).
    let mut inner = factory.lock();
    let entry = inner.agents.get_mut(agent).and_then(|runtime| runtime.transcript.as_mut()).and_then(|transcript| transcript.set_ask(ask.to_proto()));
    if let Some(entry) = entry {
        factory.send_change(&inner, agent, crate::transcript::Change::Entry(entry));
        if !only_entry {
            factory.broadcast_workspace(&inner);
        }
    }
}

/// What Execute card `id` runs.
pub(crate) fn command_of(factory: &Factory, id: i64) -> Option<crate::command_cards::Command> {
    let db = factory.store.db();
    db.query_row(&format!("SELECT {COLUMNS} FROM asks WHERE id = ?1"), [id], row).optional().ok().flatten()?.execute()
}

/// Shows the agent's decided asks as decided in its just-loaded
/// conversation, its procedures as far as they've gone, and the questions
/// it changed as they are now.
pub(crate) fn restore(store: &Store, runs: &crate::command_cards::Runs, agent: &str, transcript: &mut Transcript) -> rusqlite::Result<()> {
    let db = store.db();
    let mut statement = db
        .prepare(&format!("SELECT {COLUMNS} FROM asks WHERE agent = ?1 AND (state != 'pending' OR steps IS NOT NULL OR revision > 1) ORDER BY id"))?;
    for decided in statement.query_map([agent], row)? {
        transcript.set_ask(decided?.with_run(runs, &db).to_proto());
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
    let db = store.db();
    db.execute("DELETE FROM asks WHERE agent = ?1", [agent])?;
    // Execute cards' runs go with them.
    crate::command_cards::forget(&db)?;
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
/// `device`: the device they answered on, which an Execute card's run records.
pub(crate) async fn answer(factory: &Arc<Factory>, request: &v1::AnswerAsk, device: &str) -> Result<(), RequestError> {
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
    if let Err(changed) = seen(request.revision, ask.revision, ask.updated) {
        // Every ask_reply makes a new version, so a reply they hadn't seen
        // is what changed it: say that (row 1268), not that it changed.
        if unseen_reply(&ask, request.talk_seen) {
            return Err(RequestError::Ask(replied_since(factory, agent)));
        }
        return Err(RequestError::Ask(changed));
    }
    // An approval or answer of a review card whose file changed since the
    // card was last marked (Abbacchio 1570): the watch marks it only once
    // the file has been quiet a while, so one given meanwhile would mean a
    // version that's no longer the file. It's marked now, and refused.
    if !request.dismiss && !request.reply && !request.resolve && !ask.review.is_empty() && review_moved(factory, agent, &ask).await {
        return Err(RequestError::Ask(CHANGED.to_owned()));
    }
    // Talked over on the card (#430): Resolve ends it, and Reply answers
    // the agent again, whatever kind of card it is.
    let threaded = !ask.thread.is_empty();
    if request.resolve {
        return resolve(factory, agent, request, ask);
    }
    if request.reply && !ask.steps.is_empty() {
        // Send on a step (#533): their words on the step waiting, which
        // stays waiting with nothing marked; the card waits on its agent to
        // fix the step or answer, as a reply does.
        let Some(current) = ask.steps.iter().position(|step| step.state.is_empty()) else {
            return Err(RequestError::Ask("Every step was already answered.".to_owned()));
        };
        if request.step.is_some_and(|given| given as usize != current) {
            return Err(RequestError::Ask("That isn't the step waiting: look at the request again.".to_owned()));
        }
    } else if !ask.steps.is_empty() || request.step.is_some() {
        if request.reply {
            return Err(RequestError::Ask("This question has no steps.".to_owned()));
        }
        return step(factory, agent, request, ask);
    }
    // Approved or answered with a reply of its agent's on it that the
    // person hadn't seen (#492): the card shows it now, and they answer
    // again, having read it.
    let deciding = !request.dismiss && !request.reply;
    if deciding && ask.plain() && unseen_reply(&ask, request.talk_seen) {
        return Err(RequestError::Ask(replied_since(factory, agent)));
    }
    // Its talk, when approving or answering it agrees with where it ended.
    let follows = deciding && ask.follows_talk();
    let text = request.text.trim();
    // A reply (#370 Submit): the person's words, neither a yes nor a no.
    if request.reply {
        if ask.handoff().is_some_and(|handoff| handoff.kind == v1::ErrandHandoffKind::Code as i32) {
            return Err(RequestError::Ask("Type the code and press Fill in, or Can't do this.".to_owned()));
        }
        if ask.handoff().is_some_and(|handoff| handoff.kind == v1::ErrandHandoffKind::RecoveryCodes as i32) {
            return Err(RequestError::Ask(CODES_CHOICE.to_owned()));
        }
        if ask.wants_text && !threaded {
            return Err(RequestError::Ask("This question wants a written answer: send it as the answer.".to_owned()));
        }
        // Words, files (#511) or both.
        if text.is_empty() && request.attachments.is_empty() {
            return Err(RequestError::Ask("Write a reply or attach a file, or approve or dismiss the question.".to_owned()));
        }
    }
    let dismiss = request.dismiss && !request.reply;
    // An Execute card (`crate::command_cards`): Execute or Dismiss, nothing
    // else. Execute comes `confirmed` from an app that showed the command
    // (with the confirm's second tap when it has warnings); an older app
    // shows only the agent's title and body, so its approval runs nothing.
    let execute = ask.execute();
    if execute.is_some() {
        if request.reply {
            return Err(RequestError::Ask("This request runs a command: Execute it or dismiss it.".to_owned()));
        }
        if !dismiss && !request.confirmed {
            return Err(RequestError::Ask(
                "This request runs a command, which this version of the app can't show: update Claktory to run it, or dismiss it.".to_owned(),
            ));
        }
        // Its agent hears only how the run went (#511): files go with a
        // dismissal, where it hears their words.
        if !dismiss && !request.attachments.is_empty() {
            return Err(RequestError::Ask("Execute runs the command and nothing else: execute it without files, or dismiss it with them.".to_owned()));
        }
        // It runs exactly what it shows, and words with it would read as
        // instructions it doesn't follow: they go with a dismissal (#533 N5).
        if !dismiss && !text.is_empty() {
            return Err(RequestError::Ask("Execute runs the command and nothing else: execute it without a comment, or dismiss it with one.".to_owned()));
        }
    }
    // A trust card's words go to the pipe's lead, never to its asker (#471
    // G7): files can't follow them there, so none go (Fugo 37176).
    if !request.attachments.is_empty()
        && matches!(serde_json::from_str::<crate::card_actions::Action>(&ask.action), Ok(crate::card_actions::Action::TrustPipe(_)))
    {
        return Err(RequestError::Ask("Files can't go with this answer: its comment goes to the pipe's lead, not the asker.".to_owned()));
    }
    // An errand's handoff (#435): Done, or Can't do this with a comment.
    let handoff = ask.handoff();
    // An agreement card (#435 W8b): Accept ticks the box, a reply ticks
    // nothing and goes to the agent, Can't do this ends the errand.
    let ticks = handoff.as_ref().filter(|handoff| handoff.kind == v1::ErrandHandoffKind::Agreement as i32 && !handoff.agreement.is_empty());
    if handoff.is_some() && ticks.is_none() && request.reply {
        return Err(RequestError::Ask("Press Done when you've finished, or Can't do this.".to_owned()));
    }
    if ticks.is_some() && deciding && !text.is_empty() {
        return Err(RequestError::Ask("Accept ticks the box and takes no words: send them as a reply instead, and nothing is ticked.".to_owned()));
    }
    // A recovery-codes card (#435 W8b, Fugo 42966): I saved them, with no
    // words, or Keep in Secrets (its own request); never dismissed, so its
    // codes go only when the person says.
    let codes = handoff.as_ref().filter(|handoff| handoff.kind == v1::ErrandHandoffKind::RecoveryCodes as i32);
    if codes.is_some() && dismiss {
        return Err(RequestError::Ask(CODES_CHOICE.to_owned()));
    }
    if codes.is_some() && (!text.is_empty() || !request.attachments.is_empty()) {
        return Err(RequestError::Ask("I saved them doesn't take a reply.".to_owned()));
    }
    // A code card (#435 W8): the code goes into the page, never into the
    // card, the conversation or the agent.
    let code = handoff.as_ref().filter(|handoff| handoff.kind == v1::ErrandHandoffKind::Code as i32);
    if code.is_some() && !dismiss && !request.attachments.is_empty() {
        return Err(RequestError::Ask("This request takes only the code.".to_owned()));
    }
    if code.is_some() && !dismiss && text.is_empty() {
        return Err(RequestError::Ask("Type the code, or press Can't do this.".to_owned()));
    }
    // Done on any other handoff takes nothing with it: its agent hears only
    // that the person finished (`crate::errands::handoff::answered`), so
    // words or files an older app sends with it are dropped, never kept as
    // an answer the agent didn't get (Abbacchio 1640). Words go with Can't
    // do this, or an agreement card's reply.
    let wordless = handoff.is_some() && code.is_none() && deciding;
    let text = if wordless { "" } else { text };
    // An answer needs words; a reply on its talk can be files alone (#511).
    if !dismiss && !request.reply && ask.wants_text && text.is_empty() {
        return Err(RequestError::Ask("Write an answer, or dismiss the question.".to_owned()));
    }
    // Asked to clear another agent's conversation (agents_clear): approved,
    // it's cleared, so that agent must still be here.
    let clearing = !ask.clears.is_empty() && !dismiss && !request.reply;
    if clearing {
        let Some(name) = factory.lock().config.agent(&ask.clears).filter(|_| ask.clears != agent).map(|target| target.name.clone())
        else {
            return Err(RequestError::Ask(
                "That agent is no longer in this Claktory, so there's nothing to clear: dismiss this request.".to_owned(),
            ));
        };
        // The agent it was asked for, not another made since with its id.
        let now = crate::user_apps::current_incarnation(&factory.home, &ask.clears);
        if ask.clears_incarnation.is_empty() || now.as_deref() != Some(ask.clears_incarnation.as_str()) {
            return Err(RequestError::Ask(format!(
                "The agent this request asked to clear was deleted, and {name} is another agent made since, so nothing was cleared: dismiss this request."
            )));
        }
    }
    // A change an admin agent asked the person to approve (#423): approved,
    // it's made, so it must still be one that can be made.
    let acting = !ask.action.is_empty() && !dismiss && !request.reply;
    if acting {
        crate::card_actions::check(factory, &ask.action).map_err(RequestError::Ask)?;
    }
    // Filled before the card is kept, so a code that isn't one leaves it
    // waiting; a fill that didn't take ends it all the same, saying why.
    let mut not_filled = None;
    let acted = if let Some(code) = code.filter(|_| !dismiss) {
        Some(crate::errands::code::fill(factory, code, text).await)
    } else if let Some(agreement) = ticks.filter(|_| deciding) {
        Some(crate::errands::code::tick(factory, agreement).await)
    } else {
        None
    };
    match acted {
        Some(Err(crate::errands::code::NotFilled::Retry(why))) => return Err(RequestError::Ask(why)),
        Some(Err(crate::errands::code::NotFilled::Gone)) => not_filled = Some(crate::errands::code::LOST.to_owned()),
        Some(Ok(Err(why))) => not_filled = Some(why),
        Some(Ok(Ok(()))) | None => {}
    }
    // Files with any answer (#511), and drawings on a PDF's pages: uploads
    // of this agent's, as a message's attachments are.
    let attachments = if wordless { Vec::new() } else { factory.attachments(agent, &request.attachments)? };
    let stored = Some(&attachments).filter(|list| !list.is_empty()).and_then(|list| serde_json::to_string(list).ok());
    // Dismissing, the text is the person's optional comment.
    let state = if dismiss { "dismissed" } else { "answered" };
    // A code card keeps no code: it's in the page now.
    let answer = if code.is_some() && !dismiss { "" } else { text };
    let decided = now_ms();
    // Kept only if nothing answered or changed it meanwhile.
    let claimed = factory.store.db().execute(
        "UPDATE asks SET state = ?2, answer = ?3, decided = ?4, answer_attachments = ?5, replied = ?6 WHERE id = ?1 AND state = 'pending' AND revision = ?7",
        params![number, state, answer, decided, stored, request.reply, ask.revision],
    )?;
    if claimed == 0 {
        return Err(RequestError::Ask(lost_claim(factory, number)));
    }
    // Saved: Claktory's copy goes.
    if codes.is_some() {
        crate::errands::code::drop_codes(factory, number);
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
        let name = factory.agent_name(&ask.clears);
        let cleared = if clearing {
            let cleared = factory.clear_conversation(&ask.clears).await;
            if let Err(error) = &cleared {
                tracing::error!(%agent, clears = %ask.clears, %error, "could not clear the conversation the person approved clearing");
            }
            Some(cleared.map_err(|error| error.to_string()))
        } else {
            None
        };
        if let Some(Err(why)) = &cleared {
            not_made(factory, agent, &mut ask, why);
        }
        let (summary, mut text) = told_clear(&person, &ask, &name, cleared);
        text.push_str(&files_said(&person, &ask.review, &ask.answer_attachments));
        factory.deliver_event(agent, text, told_event(&ask, None, summary));
        return Ok(());
    }
    if let Some(why) = &not_filled {
        let what = if ticks.is_some() { "ticked" } else { "filled in" };
        not_made(factory, agent, &mut ask, &format!("It wasn't {what}: {why}."));
    }
    if let Some(handoff) = handoff {
        crate::errands::handoff::answered(factory, agent, &handoff, !dismiss, &ask.answer).await;
        return Ok(());
    }
    if let Some(command) = execute {
        if acting {
            crate::command_cards::start(factory, agent, ask.id, command, device);
        } else {
            let (summary, mut text) = crate::card_actions::told(&person, &ask.title, &ask.answer, false, false, None);
            text.push_str(&files_said(&person, &ask.review, &ask.answer_attachments));
            factory.deliver_event(agent, text, told_event(&ask, None, summary));
        }
        return Ok(());
    }
    if !ask.action.is_empty() {
        let made = if acting { Some(crate::card_actions::apply(factory, &ask.action).await) } else { None };
        if let Some(Err(why)) = &made {
            not_made(factory, agent, &mut ask, why);
        }
        // A trust card's comment goes to the pipe's lead, never word for
        // word to the agent that asked (#471 G7, Fugo 34521).
        let comment = crate::pipes::trust_card::comment_for_asker(factory, &ask.action, &ask.answer, ask.replied);
        let trust = crate::pipes::trust_card::is_trust(&ask.action);
        let (summary, mut text) = crate::card_actions::told(&person, &ask.title, &comment, ask.replied, trust, made);
        text.push_str(&files_said(&person, &ask.review, &ask.answer_attachments));
        factory.deliver_event(agent, text, told_event(&ask, None, summary));
        return Ok(());
    }
    let comments = (!ask.review.is_empty()).then(|| reviewed(factory, agent, &ask));
    let talk = follows.then(|| talk_quoted(&person, &ask.thread));
    let (summary, mut text) = told(&person, &ask, comments, &answered_on(factory, agent, &ask, request.from()), talk.as_deref());
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

/// What a recovery-codes card takes.
const CODES_CHOICE: &str = "These codes go only when you say: choose I saved them once you have, or Keep in Secrets.";

/// The recovery-codes card at `seq` in `agent`'s conversation, waiting:
/// its row and handoff; or why not, for the person.
fn codes_card(factory: &Factory, agent: &str, seq: u64) -> Result<(Row, crate::errands::HandoffRow), RequestError> {
    factory.known_agent(agent)?;
    let none = || RequestError::Ask("There's no such recovery-codes request in this agent's conversation.".to_owned());
    let id = {
        let inner = factory.lock();
        let entry =
            inner.agents.get(agent).and_then(|runtime| runtime.transcript.as_ref()).and_then(|transcript| transcript.entry(seq).cloned());
        match entry.and_then(|entry| entry.kind) {
            Some(v1::entry::Kind::AskPerson(ask)) => ask.id,
            _ => return Err(none()),
        }
    };
    let number: i64 = id.parse().map_err(|_| none())?;
    let ask = factory
        .store
        .db()
        .query_row(&format!("SELECT {COLUMNS} FROM asks WHERE id = ?1 AND agent = ?2"), params![number, agent], row)
        .optional()?
        .ok_or_else(none)?;
    let handoff = ask.handoff().filter(|handoff| handoff.kind == v1::ErrandHandoffKind::RecoveryCodes as i32).ok_or_else(none)?;
    if ask.state != "pending" {
        return Err(RequestError::Ask("These codes are gone: the request was already closed.".to_owned()));
    }
    Ok((ask, handoff))
}

/// ShowRecoveryCodes (#435 W8b): a waiting recovery-codes card's codes, for
/// the person's device that asked only.
pub(crate) async fn show_recovery_codes(factory: &Arc<Factory>, request: &v1::ShowRecoveryCodes) -> Result<v1::RecoveryCodes, RequestError> {
    factory.ensure_loaded(&request.agent_id).await?;
    let (ask, _) = codes_card(factory, &request.agent_id, request.seq)?;
    let codes = crate::errands::code::held_codes(factory, ask.id).map_err(RequestError::Ask)?.ok_or_else(|| {
        RequestError::Ask("These codes are gone: Claktory restarted while Secrets was locked. Choose I saved them to close the request.".to_owned())
    })?;
    Ok(v1::RecoveryCodes { agent_id: request.agent_id.clone(), seq: request.seq, codes: codes.to_vec() })
}

/// A recovery-codes card's answer once the codes were kept in Secrets as
/// the secret `name` (`keep_recovery_codes`).
fn kept_answer(name: &str) -> String {
    format!("Kept in Secrets as {name}, given to no agent.")
}

/// The secret a recovery-codes card's codes were kept as, from its answer
/// (`kept_answer`); None when the person chose I saved them.
pub(crate) fn kept_as(answer: &str) -> Option<&str> {
    answer.strip_prefix("Kept in Secrets as ")?.strip_suffix(", given to no agent.").filter(|name| !name.is_empty())
}

/// KeepRecoveryCodes (#435 W8b): the codes become a secret given to no
/// agent, Claktory's copy goes, and the card closes; the agent is told.
pub(crate) async fn keep_recovery_codes(factory: &Arc<Factory>, request: &v1::KeepRecoveryCodes) -> Result<(), RequestError> {
    let agent = request.agent_id.as_str();
    factory.ensure_loaded(agent).await?;
    let (mut ask, handoff) = codes_card(factory, agent, request.seq)?;
    let codes = crate::errands::code::held_codes(factory, ask.id).map_err(RequestError::Ask)?.ok_or_else(|| {
        RequestError::Ask("These codes are gone: Claktory restarted while Secrets was locked, so there's nothing to keep. Choose I saved them to close the request.".to_owned())
    })?;
    let host = url::Url::parse(&handoff.origin).ok().and_then(|url| url.host_str().map(str::to_owned)).unwrap_or_default();
    let why = format!("Kept from the recovery-codes request of errand {}.", handoff.errand);
    let name = factory.vault.keep_codes(&host, &codes, &why).map_err(|error| match error {
        crate::vault::VaultError::Locked => RequestError::Ask(
            "Secrets is locked, so the codes can't go there: unlock it in Settings → Secrets, or save them yourself and choose I saved them."
                .to_owned(),
        ),
        error => RequestError::Ask(format!("The codes weren't kept: {error}")),
    })?;
    let answer = kept_answer(&name);
    let decided = now_ms();
    let claimed = factory.store.db().execute(
        "UPDATE asks SET state = 'answered', answer = ?2, decided = ?3 WHERE id = ?1 AND state = 'pending'",
        params![ask.id, answer, decided],
    )?;
    if claimed == 0 {
        // Saved meanwhile on another device: the secret stays, as asked.
        return Err(RequestError::Ask(lost_claim(factory, ask.id)));
    }
    crate::errands::code::drop_codes(factory, ask.id);
    (ask.state, ask.answer, ask.decided) = ("answered".to_owned(), answer, Some(decided));
    {
        let mut inner = factory.lock();
        let entry =
            inner.agents.get_mut(agent).and_then(|runtime| runtime.transcript.as_mut()).and_then(|transcript| transcript.set_ask(ask.to_proto()));
        if let Some(entry) = entry {
            factory.send_change(&inner, agent, crate::transcript::Change::Entry(entry));
        }
        factory.broadcast_workspace(&inner);
    }
    factory.broadcast_workspace(&factory.lock());
    factory.forget_cached(&handoff.errand, &handoff.origin).await;
    crate::errands::code::codes_saved(factory, agent, &handoff, Some(&name)).await;
    Ok(())
}

/// A review card waiting, as `crate::review_watch` follows its file.
pub(crate) struct WaitingReview {
    pub(crate) id: i64,
    pub(crate) agent: String,
    /// The file it shows.
    pub(crate) review: String,
    /// What the file held when it was asked or last changed
    /// (`crate::review_watch::fingerprint`); '' for a card from before.
    pub(crate) seen: String,
    pub(crate) revision: i64,
}

/// The review cards waiting for the person.
pub(crate) fn waiting_reviews(factory: &Factory) -> Result<Vec<WaitingReview>, String> {
    crate::store::query(
        &factory.store.db(),
        "SELECT id, agent, review, review_seen, revision FROM asks WHERE state = 'pending' AND review != ''",
        [],
        |row| Ok(WaitingReview { id: row.get(0)?, agent: row.get(1)?, review: row.get(2)?, seen: row.get(3)?, revision: row.get(4)? }),
    )
    .map_err(|error| error.to_string())
}

/// Whether review card `ask`'s file no longer holds what the card was last
/// marked at; if so, it's marked updated now (`file_changed`).
async fn review_moved(factory: &Arc<Factory>, agent: &str, ask: &Row) -> bool {
    let seen = factory
        .store
        .db()
        .query_row("SELECT review_seen FROM asks WHERE id = ?1", [ask.id], |row| row.get::<_, String>(0))
        .unwrap_or_default();
    // A card from before this keeps nothing of its file to compare.
    if seen.is_empty() {
        return false;
    }
    let path = ask.review.clone();
    let Ok(print) = tokio::task::spawn_blocking(move || crate::review_watch::fingerprint(&path)).await else { return false };
    if print.is_empty() || print == seen {
        return false;
    }
    let card = WaitingReview { id: ask.id, agent: agent.to_owned(), review: ask.review.clone(), seen, revision: ask.revision };
    file_changed(factory, &card, &print);
    true
}

/// The file review card `card` shows changed while it waited, and now
/// holds `print` (Bruno 46189): Claktory marks the card updated by itself,
/// as an ask_update would, in its own words (AskPerson.update_note). It
/// redraws in place without notifying again, an answer to the version
/// before is refused, and its agent is told quietly. A card from before
/// this (nothing kept of its file) only keeps what the file holds now.
pub(crate) fn file_changed(factory: &Arc<Factory>, card: &WaitingReview, print: &str) {
    let db = factory.store.db();
    if card.seen.is_empty() {
        if let Err(error) =
            db.execute("UPDATE asks SET review_seen = ?3 WHERE id = ?1 AND revision = ?2 AND review_seen = ''", params![card.id, card.revision, print])
        {
            tracing::warn!(card = card.id, %error, "couldn't keep what a review card's file holds");
        }
        return;
    }
    let name = factory.agent_name(&card.agent);
    let note = format!("This file changed since {name} asked. Approving means it as it reads now.");
    let claimed = db.execute(
        "UPDATE asks SET revision = revision + 1, updated = ?3, update_reason = '', update_note = ?4, review_seen = ?5,
         file_updates = file_updates + 1
         WHERE id = ?1 AND state = 'pending' AND revision = ?2",
        params![card.id, card.revision, now_ms(), note, print],
    );
    if !matches!(claimed, Ok(1)) {
        // Answered or changed meanwhile: the next look sees it as it is.
        return;
    }
    let ask = db.query_row(&format!("SELECT {COLUMNS} FROM asks WHERE id = ?1"), [card.id], row);
    drop(db);
    let Ok(ask) = ask else { return };
    {
        let mut inner = factory.lock();
        let entry = inner
            .agents
            .get_mut(&card.agent)
            .and_then(|runtime| runtime.transcript.as_mut())
            .and_then(|transcript| transcript.set_ask(ask.to_proto()));
        if let Some(entry) = entry {
            factory.send_change(&inner, &card.agent, crate::transcript::Change::Entry(entry));
        }
        factory.broadcast_workspace(&inner);
    }
    if let Some((whose, _)) = ask.item.rsplit_once('#') {
        factory.refresh_view(whose);
    }
    let person = factory.lock().config.person().to_owned();
    let text = format!(
        "{} changed while question {} waited, so {person} sees it marked updated and reviews it as it reads now. An approval of the earlier version is refused.",
        card.review, card.id
    );
    factory.deliver_event(&card.agent, text, crate::needs_you::dismissed_event(format!("Question {}: its file changed", card.id)));
}

/// Keeps why the change an approved card asked for couldn't be made, and
/// shows it on the card, so it doesn't read as done.
fn not_made(factory: &Arc<Factory>, agent: &str, ask: &mut Row, why: &str) {
    ask.action_error = why.to_owned();
    if let Err(error) = factory.store.db().execute("UPDATE asks SET action_error = ?2 WHERE id = ?1", params![ask.id, why]) {
        tracing::error!(%agent, %error, "could not keep why an approved card's change wasn't made");
    }
    let mut inner = factory.lock();
    let entry =
        inner.agents.get_mut(agent).and_then(|runtime| runtime.transcript.as_mut()).and_then(|transcript| transcript.set_ask(ask.to_proto()));
    if let Some(entry) = entry {
        factory.send_change(&inner, agent, crate::transcript::Change::Entry(entry));
        factory.broadcast_workspace(&inner);
    }
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
        let summary = format!("Your request \"{}\" closed: that agent was deleted.", ask.title);
        let text = format!("The agent your request \"{}\" was for was deleted, so the request is closed and nothing was cleared.", ask.title);
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
        "Asked {person}: they see a request to approve or dismiss clearing {name}'s conversation, and only their approval clears it. You'll be told which in an event. Its id is {id}: if it stops applying before they answer in the request (they answer it in your chat, say), take it back with ask_withdraw."
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
    ask_to_approve_with(factory, asker, title, &format!("{description} {reason}"), (description, reason), action)
}

/// `ask_to_approve` with its own `body`, for older apps.
pub(crate) fn ask_to_approve_with(
    factory: &Factory,
    asker: &str,
    title: &str,
    body: &str,
    (description, reason): (&str, &str),
    action: &str,
) -> Result<String, String> {
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
    let name = factory.agent_name(whose);
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
    // A handoff waits for the person's Done or Can't do this: the browser
    // is theirs until then (`crate::errands::handoff`).
    if ask.handoff().is_some() {
        return Err(format!("Question {number} hands {person} the browser: it waits for their Done or Can't do this. Nothing was withdrawn."));
    }
    // Claktory's own card about the browser's sign-in (#435, Fugo 41236):
    // the person's, never the agent's to take back.
    if matches!(serde_json::from_str::<crate::card_actions::Action>(&ask.action), Ok(crate::card_actions::Action::BrowserSignOut { .. })) {
        return Err(format!(
            "Question {number} is Claktory's request for {person} about the browser's sign-in, not yours to take back. Nothing was withdrawn."
        ));
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

/// Whether the card's talk has a reply of its agent's past the `talk_seen`
/// messages the person's app showed them (#492); never for an app that
/// doesn't say.
fn unseen_reply(ask: &Row, talk_seen: Option<u32>) -> bool {
    talk_seen.is_some_and(|seen| ask.thread.iter().skip(usize::try_from(seen).unwrap_or(usize::MAX)).any(|message| message.agent))
}

fn replied_since(factory: &Factory, agent: &str) -> String {
    let name = factory.agent_name(agent);
    format!("{name} replied in this request since you looked at it. Read the reply, then answer again.")
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
        "You've changed {what} {number} {MAX_UPDATES} times, the most one request can be changed, so it wasn't changed again. If it no longer says what you need, take it back and make a new one. Nothing was changed."
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
    if ask.agent_changes() > MAX_UPDATES {
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
    // The card shows its file as it reads now.
    let seen = crate::review_watch::fingerprint(&changed.review);
    // Kept only if the person didn't answer it, nor the agent change it, meanwhile.
    let claimed = factory
        .store
        .db()
        .execute(
            "UPDATE asks SET title = ?3, body = ?4, wants_text = ?5, review = ?6, steps = ?7, taint = ?8,
             revision = revision + 1, updated = ?9, update_reason = ?10, review_seen = ?12, update_note = ''
             WHERE id = ?1 AND agent = ?2 AND state = 'pending' AND revision = ?11",
            params![number, agent, changed.title, changed.body, changed.wants_text, changed.review, steps, ask.taint, now, reason, ask.revision, seen],
        )
        .map_err(|error| format!("Couldn't change it: {error}"))?;
    if claimed == 0 {
        return Err(format!("{person} just answered question {number}, so it wasn't changed; you'll be told their answer."));
    }
    let kept = changed.steps.iter().take_while(|step| step.state == "done").count();
    let moved_back = procedure && kept < ask.steps.iter().take_while(|step| step.state == "done").count();
    (ask.title, ask.body, ask.wants_text, ask.review, ask.steps) = (changed.title, changed.body, changed.wants_text, changed.review, changed.steps);
    (ask.revision, ask.updated, ask.update_reason, ask.update_note) = (ask.revision + 1, Some(now), reason, String::new());
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
    let left = MAX_UPDATES - (ask.agent_changes() - 1);
    let mut said = format!(
        "Changed question {number}, \"{}\": {person}'s request shows it as it is now, marked updated{}, without notifying them again. An answer they give to the version before is refused, so they look again.",
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
        0 => " That was the last change this request can take.".to_owned(),
        1 => " It can be changed once more.".to_owned(),
        _ => format!(" It can be changed {left} more times."),
    });
    Ok(ToolResult::text(said, json!({})))
}

/// The longest answer an agent gives on a card (ask_reply), in characters.
const MAX_THREAD_TEXT: usize = 4000;

/// What the agent is told of where the person answered `ask` from (#430),
/// and, when they wrote something it may need to answer, to answer it on
/// the card, where they are: the event's lines before "You asked".
fn answered_on(factory: &Factory, agent: &str, ask: &Row, from: v1::AskAnsweredFrom) -> String {
    let mut said = Vec::new();
    match from {
        v1::AskAnsweredFrom::NeedsYou => said.push("They answered from Needs you, not in your conversation.".to_owned()),
        v1::AskAnsweredFrom::Conversation => said.push("They answered in the request in your conversation.".to_owned()),
        v1::AskAnsweredFrom::Unspecified => {}
    }
    if ask.person_said() && can_reply(factory, agent, ask) {
        // Files alone (#511) are what they sent.
        let what = if ask.answer.trim().is_empty() { "sent" } else { "wrote" };
        said.push(format!(
            "If what they {what} needs an answer from you, reply in the request with {REPLY_TOOL} (id {}) so it reaches them where they are, rather than in your chat.",
            ask.id
        ));
    }
    if said.is_empty() { String::new() } else { format!("\n\n{}", said.join(" ")) }
}

/// Whether the agent may answer the person on `ask` (#430): an ordinary
/// card, with changes left, from an agent not limited to a pipe.
fn can_reply(factory: &Factory, agent: &str, ask: &Row) -> bool {
    ask.clears.is_empty() && ask.action.is_empty() && ask.agent_changes() <= MAX_UPDATES && !factory.pipes.contained(agent)
}

/// Resolve (#430): the person ends a card they talked over with its agent,
/// without a decision. Its agent is told, quietly unless they commented.
fn resolve(factory: &Arc<Factory>, agent: &str, request: &v1::AnswerAsk, mut ask: Row) -> Result<(), RequestError> {
    if ask.thread.is_empty() {
        return Err(RequestError::Ask("Only a request talked over with its agent can be resolved: answer or dismiss it.".to_owned()));
    }
    let comment = request.text.trim().to_owned();
    // Files with it (#511), kept with their comment.
    let attachments = factory.attachments(agent, &request.attachments)?;
    let stored = Some(&attachments).filter(|list| !list.is_empty()).and_then(|list| serde_json::to_string(list).ok());
    let decided = now_ms();
    let claimed = factory.store.db().execute(
        "UPDATE asks SET state = 'answered', answer = ?2, decided = ?3, resolved = 1, replied = 0, answer_attachments = ?5
         WHERE id = ?1 AND state = 'pending' AND revision = ?4",
        params![ask.id, comment, decided, ask.revision, stored],
    )?;
    if claimed == 0 {
        return Err(RequestError::Ask(lost_claim(factory, ask.id)));
    }
    (ask.state, ask.answer, ask.decided, ask.resolved, ask.replied) = ("answered".to_owned(), comment.clone(), Some(decided), true, false);
    ask.answer_attachments = attachments;
    let person = {
        let mut inner = factory.lock();
        let entry =
            inner.agents.get_mut(agent).and_then(|runtime| runtime.transcript.as_mut()).and_then(|transcript| transcript.set_ask(ask.to_proto()));
        if let Some(entry) = entry {
            factory.send_change(&inner, agent, crate::transcript::Change::Entry(entry));
            factory.broadcast_workspace(&inner);
        }
        inner.config.person_capitalized().to_owned()
    };
    let title = &ask.title;
    let summary = format!("{person} resolved \"{title}\".");
    let said = if comment.is_empty() { String::new() } else { format!(" Their comment:\n\n{comment}\n\n") };
    let files = files_said(&person, &ask.review, &ask.answer_attachments);
    let mut text = format!(
        "{person} resolved your question \"{title}\" in its request: you talked it through, so it ends there, without a decision.{}Carry on with what you agreed.{files}\n\nYou asked: {title}\n{}",
        if said.is_empty() { " ".to_owned() } else { said },
        ask.body
    );
    if !ask.item.is_empty() {
        match super::checklist::decided(factory, &ask.item, false, "ask") {
            Ok(Some(more)) => text.push_str(&more),
            Ok(None) => {}
            Err(error) => tracing::error!(%agent, %error, "could not note a resolved question on its item"),
        }
        if let Some((whose, _)) = ask.item.rsplit_once('#') {
            factory.refresh_view(whose);
        }
        factory.broadcast_workspace(&factory.lock());
    }
    let event = if comment.is_empty() && files.is_empty() { crate::needs_you::dismissed_event(summary) } else { host_event(summary) };
    factory.deliver_event(agent, text, event);
    Ok(())
}

#[derive(Deserialize)]
struct ReplyArgs {
    id: String,
    text: String,
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    body: Option<String>,
    #[serde(default)]
    steps: Option<Vec<String>>,
    /// The talk settled it (#496): close the card with these words.
    #[serde(default)]
    resolve: bool,
}

/// ask_reply (#430): answers the person's words on one of the agent's own
/// cards, where they are. The card keeps the talk (`thread`: their words,
/// then the agent's), opens again in Needs you as a new version, with a new
/// title, body or steps if given, and notifies as a new question does.
fn reply(factory: &Arc<Factory>, agent: &str, args: ReplyArgs) -> Result<ToolResult, String> {
    let (person, named) = {
        let inner = factory.lock();
        super::caller(&inner.config, agent)?;
        (inner.config.person().to_owned(), inner.config.person_capitalized().to_owned())
    };
    if factory.pipes.contained(agent) {
        return Err("You can't answer in a request: ask a new question if it still matters. Nothing was sent.".to_owned());
    }
    let given = args.id.trim();
    let unknown = || format!("You have no question {given}: ask_withdraw without id lists those waiting. Nothing was sent.");
    let number = card_number(given).ok_or_else(unknown)?;
    let text = args.text.trim().to_owned();
    if text.is_empty() {
        return Err(format!("Give text: your answer to what {person} wrote. Nothing was sent."));
    }
    if text.chars().count() > MAX_THREAD_TEXT {
        return Err(format!("Keep your answer to {MAX_THREAD_TEXT} characters; it's a reply in a request. Nothing was sent."));
    }
    let found = factory
        .store
        .db()
        .query_row(&format!("SELECT {COLUMNS} FROM asks WHERE id = ?1 AND agent = ?2"), params![number, agent], row)
        .optional()
        .map_err(|error| format!("Couldn't read your questions: {error}"))?;
    let Some(mut ask) = found else { return Err(unknown()) };
    // A procedure on its way, whose last step done came with their words
    // or files: answered where it waits, at the step it's on.
    let on_step = ask.state == "pending" && !ask.steps.is_empty() && ask.person_said();
    if args.resolve {
        if args.title.is_some() || args.body.is_some() || args.steps.is_some() {
            return Err("Closing a request changes nothing on it: give only id, text and resolve. Nothing was sent.".to_owned());
        }
        return close(factory, agent, (&person, &named), ask, on_step, text);
    }
    match ask.state.as_str() {
        "pending" if on_step => {}
        "pending" => {
            return Err(format!(
                "Question {number} still waits for {person}, so there's nothing of theirs to answer: change it with ask_update if it needs to. Nothing was sent."
            ));
        }
        "answered" | "dismissed" if ask.resolved => {
            let who = if ask.closed_by_agent() { format!("You closed question {number}") } else { format!("{named} resolved question {number}") };
            return Err(format!("{who}: it's over. If something still needs them, ask a new question. Nothing was sent."));
        }
        "answered" | "dismissed" => {}
        "withdrawn" => return Err(format!("You withdrew question {number}. Nothing was sent.")),
        _ => return Err(format!("Question {number} was closed without an answer. Nothing was sent.")),
    }
    if !ask.person_said() && !ask.stopped() {
        return Err(format!(
            "{named} answered question {number} without words or files, so there's nothing of theirs to answer on it. Nothing was sent."
        ));
    }
    // What the person approves is what happens (Fugo, #492): a card that
    // makes a change is never talked on, so no reply can stand beside its
    // words, nor open it again under the same action.
    if !ask.plain() {
        return Err(format!(
            "Question {number} does exactly what it shows when {person} approves it, so it can't be answered on, opened again or changed in a reply. To ask for something else, ask again with the tool that made it. Nothing was sent."
        ));
    }
    if ask.agent_changes() > MAX_UPDATES {
        return Err(format!(
            "Question {number} was changed or answered on {MAX_UPDATES} times, the most one request can take: ask a new question if it still needs {person}. Nothing was sent."
        ));
    }
    let procedure = !ask.steps.is_empty();
    if !procedure && args.steps.is_some() {
        return Err(format!("Question {number} has no steps, so it can't be given any. Nothing was sent."));
    }
    let mut changed = ask.clone_fields();
    if let Some(title) = &args.title {
        let title = title.split_whitespace().collect::<Vec<_>>().join(" ");
        if title.is_empty() || title.chars().count() > MAX_TITLE {
            return Err(format!("The title is a short line, at most {MAX_TITLE} characters. Nothing was sent."));
        }
        changed.title = title;
    }
    if let Some(body) = &args.body {
        let body = body.trim().to_owned();
        if body.is_empty() {
            return Err("The body can't be empty. Nothing was sent.".to_owned());
        }
        changed.body = body;
    }
    if let Some(steps) = &args.steps {
        changed.steps = restepped(&ask.steps, steps).map_err(|error| error.replace("Nothing was changed.", "Nothing was sent."))?;
    } else if procedure {
        // A step they couldn't do is asked again.
        for step in changed.steps.iter_mut().filter(|step| step.state == "cant") {
            *step = Step { text: std::mem::take(&mut step.text), ..Step::default() };
        }
        if changed.steps.iter().all(|step| step.state == "done") {
            return Err(format!(
                "{named} did every step of question {number}, so it would open again with nothing to do: give steps with what's left, or ask a new question. Nothing was sent."
            ));
        }
    }
    let edited = changed.title != ask.title
        || changed.body != ask.body
        || serde_json::to_string(&changed.steps).ok() != serde_json::to_string(&ask.steps).ok();
    let now = now_ms();
    let mut thread = ask.thread.clone();
    let said_at = ask.decided.or_else(|| ask.steps.iter().filter(|step| step.state == "done").map(|step| step.decided).max());
    let about = ask.talk_step(on_step);
    // Can't do this without words (#533 N3) leaves nothing of theirs to show.
    if ask.person_said() {
        let (text, attachments) = (ask.answer.clone(), ask.answer_attachments.clone());
        thread.push(ThreadMessage { agent: false, text, at: said_at.unwrap_or(now), attachments, step: about });
    }
    thread.push(ThreadMessage { agent: true, text: text.clone(), at: now, attachments: Vec::new(), step: about });
    // Read a pipe or a web page since it asked: the card says so now.
    let routes = crate::pipes::agents::routes(factory, agent);
    if !routes.is_empty() {
        ask.taint = Some(crate::pipes::agents::routes_json(&routes).to_string());
    }
    let steps = (!changed.steps.is_empty()).then(|| serde_json::to_string(&changed.steps).unwrap_or_default());
    let updated = if edited { Some(now) } else { ask.updated };
    let update_reason = if edited { String::new() } else { ask.update_reason.clone() };
    let update_note = if edited { String::new() } else { ask.update_note.clone() };
    // Kept only if nothing else opened or changed it meanwhile.
    let claimed = factory
        .store
        .db()
        .execute(
            "UPDATE asks SET title = ?3, body = ?4, steps = ?5, talk = ?6, taint = ?7, state = 'pending', answer = '', decided = NULL,
             answer_attachments = NULL, replied = 0, revision = revision + 1, updated = ?8, update_reason = ?9, update_note = ?12
             WHERE id = ?1 AND agent = ?2 AND state = ?10 AND revision = ?11",
            params![
                number,
                agent,
                changed.title,
                changed.body,
                steps,
                serde_json::to_string(&thread).unwrap_or_default(),
                ask.taint,
                updated,
                update_reason,
                ask.state,
                ask.revision,
                update_note
            ],
        )
        .map_err(|error| format!("Couldn't answer on it: {error}"))?;
    if claimed == 0 {
        return Err(format!("Question {number} changed meanwhile, so nothing was sent: look at it again."));
    }
    (ask.title, ask.body, ask.steps, ask.thread) = (changed.title, changed.body, changed.steps, thread);
    (ask.state, ask.answer, ask.decided, ask.replied, ask.revision) = ("pending".to_owned(), String::new(), None, false, ask.revision + 1);
    (ask.answer_attachments, ask.updated, ask.update_reason, ask.update_note) = (Vec::new(), updated, update_reason, update_note);
    // Its card waits again, at the same seq, and notifies as a new one does;
    // one that still waited notifies again.
    {
        let mut inner = factory.lock();
        let entry =
            inner.agents.get_mut(agent).and_then(|runtime| runtime.transcript.as_mut()).and_then(|transcript| transcript.set_ask(ask.to_proto()));
        if let Some(entry) = entry {
            if on_step {
                factory.notify.waits_again(agent, entry.seq);
            }
            factory.send_change(&inner, agent, crate::transcript::Change::Entry(entry));
        }
        factory.broadcast_workspace(&inner);
    }
    if let Some((whose, _)) = ask.item.rsplit_once('#') {
        factory.refresh_view(whose);
    }
    let left = MAX_UPDATES - (ask.agent_changes() - 1);
    let mut said = if on_step {
        let at = ask.steps.iter().position(|step| step.state.is_empty()).map_or(0, |at| at + 1);
        format!(
            "Answered on question {number}, \"{}\": your answer shows under their comment, in the request that still waits at step {at} of {} in {person}'s Needs you, and they're notified. You're told as each step is done, or when they reply or resolve it; meanwhile carry on with whatever doesn't depend on it.",
            ask.title,
            ask.steps.len()
        )
    } else if let Some(at) = ask.steps.iter().position(|step| step.state.is_empty()) {
        format!(
            "Answered on question {number}, \"{}\": it's open again in {person}'s Needs you at step {} of {}, with your answer under their words, and they're notified as for a new question. You're told as each step is done, or when they reply again or resolve it; meanwhile carry on with whatever doesn't depend on it.",
            ask.title,
            at + 1,
            ask.steps.len()
        )
    } else {
        format!(
            "Answered on question {number}, \"{}\": it's open again in {person}'s Needs you, with your answer under their words, and they're notified as for a new question. You're told when they answer, reply again or resolve it; meanwhile carry on with whatever doesn't depend on it.",
            ask.title
        )
    };
    said.push_str(&match left {
        0 => " That was the last change or answer this request can take.".to_owned(),
        1 => " It can take one more change or answer.".to_owned(),
        _ => format!(" It can take {left} more changes or answers."),
    });
    Ok(ToolResult::text(said, json!({})))
}

/// ask_reply with resolve (#496): the talk on the card settled it, so its
/// agent closes it with its last words, which stay on the card. A card that
/// waits on nothing but the talk (the person's reply, or the agent's own
/// answer still waiting, or a procedure's comment) ends Resolved, as the
/// person's Resolve ends it, and leaves Needs you; History shows the
/// agent's words as its own. One the person already decided keeps its
/// decision and only gains the words. It never approves, answers or runs
/// anything, and a card that makes a change is never talked on.
fn close(factory: &Arc<Factory>, agent: &str, (person, named): (&str, &str), mut ask: Row, on_step: bool, text: String) -> Result<ToolResult, String> {
    let number = ask.id;
    if !ask.plain() {
        return Err(format!(
            "Question {number} does exactly what it shows when {person} approves it, so it can't be answered on, opened again or changed in a reply. To ask for something else, ask again with the tool that made it. Nothing was sent."
        ));
    }
    let mut thread = ask.thread.clone();
    // A procedure stopped at a step they couldn't do (Can't do this, #533):
    // its agent gives up on it, with words of theirs or none. It stays as
    // it was, stopped there for good: the card shows "Stopped at step N".
    let giving_up = ask.stopped() && !ask.resolved && matches!(ask.state.as_str(), "answered" | "dismissed");
    // Their words that closed the round, when they're not on the card yet.
    let theirs = match ask.state.as_str() {
        _ if giving_up => ask.person_said(),
        "pending" if on_step => true,
        "pending" if ask.thread.last().is_some_and(|last| last.agent) => false,
        "pending" => {
            return Err(format!(
                "Question {number} still waits for {person}, and nothing was said on it: take it back with ask_withdraw if it no longer applies. Nothing was sent."
            ));
        }
        "answered" | "dismissed" if ask.resolved => {
            let who = if ask.closed_by_agent() { format!("You closed question {number}") } else { format!("{named} resolved question {number}") };
            return Err(format!("{who}: it's over already. Nothing was sent."));
        }
        "answered" | "dismissed" if !ask.person_said() => {
            return Err(format!("{named} decided question {number} without words: it's over already, with nothing to close. Nothing was sent."));
        }
        "answered" | "dismissed" => true,
        "withdrawn" => return Err(format!("You withdrew question {number}. Nothing was sent.")),
        _ => return Err(format!("Question {number} was closed without an answer. Nothing was sent.")),
    };
    let now = now_ms();
    let about = ask.talk_step(on_step);
    if theirs {
        let said_at = ask.decided.or_else(|| ask.steps.iter().filter(|step| step.state == "done").map(|step| step.decided).max());
        let (said, attachments) = (ask.answer.clone(), ask.answer_attachments.clone());
        thread.push(ThreadMessage { agent: false, text: said, at: said_at.unwrap_or(now), attachments, step: about });
    }
    thread.push(ThreadMessage { agent: true, text: text.clone(), at: now, attachments: Vec::new(), step: about });
    let talk = serde_json::to_string(&thread).unwrap_or_default();
    // Approved, answered or dismissed: that decision stands.
    let decided_already = !on_step && ask.state != "pending" && !ask.replied && !giving_up;
    let claimed = if giving_up {
        factory.store.db().execute(
            "UPDATE asks SET talk = ?3, resolved = 1 WHERE id = ?1 AND agent = ?2 AND state = ?4 AND revision = ?5",
            params![number, agent, talk, ask.state, ask.revision],
        )
    } else if decided_already {
        factory.store.db().execute(
            "UPDATE asks SET talk = ?3 WHERE id = ?1 AND agent = ?2 AND state = ?4 AND revision = ?5",
            params![number, agent, talk, ask.state, ask.revision],
        )
    } else {
        factory.store.db().execute(
            "UPDATE asks SET talk = ?3, state = 'answered', answer = '', decided = ?4, resolved = 1, replied = 0, answer_attachments = NULL, reason = ?5
             WHERE id = ?1 AND agent = ?2 AND state = ?6 AND revision = ?7",
            params![number, agent, talk, now, text, ask.state, ask.revision],
        )
    }
    .map_err(|error| format!("Couldn't close it: {error}"))?;
    if claimed == 0 {
        return Err(format!("Question {number} changed meanwhile, so nothing was sent: look at it again."));
    }
    ask.thread = thread;
    if giving_up {
        ask.resolved = true;
    } else if !decided_already {
        (ask.state, ask.answer, ask.decided, ask.resolved, ask.replied) = ("answered".to_owned(), String::new(), Some(now), true, false);
        (ask.answer_attachments, ask.reason) = (Vec::new(), text);
    }
    {
        let mut inner = factory.lock();
        let entry =
            inner.agents.get_mut(agent).and_then(|runtime| runtime.transcript.as_mut()).and_then(|transcript| transcript.set_ask(ask.to_proto()));
        if let Some(entry) = entry {
            factory.send_change(&inner, agent, crate::transcript::Change::Entry(entry));
        }
        factory.broadcast_workspace(&inner);
    }
    let title = &ask.title;
    if decided_already {
        let decision = if ask.state == "dismissed" { "dismissal" } else { "answer" };
        return Ok(ToolResult::text(
            format!("Added your words to question {number}, \"{title}\", under theirs. {named} had decided it already, so its {decision} stands and it doesn't come back to their Needs you."),
            json!({}),
        ));
    }
    let mut said = match ask.steps.iter().position(|step| step.state == "cant") {
        Some(at) if giving_up => format!(
            "Stopped question {number}, \"{title}\", at step {} of {}, for good: your words stay in the request, which shows it stopped there, without notifying {person}. Nothing more is asked of them on it.",
            at + 1,
            ask.steps.len()
        ),
        _ => format!(
            "Closed question {number}, \"{title}\", as settled in the talk: your words stay in the request, it's left {person}'s Needs you without notifying them, and History shows you closed it. Nothing was approved or run."
        ),
    };
    if !ask.item.is_empty() {
        match super::checklist::decided(factory, &ask.item, false, "ask") {
            Ok(Some(more)) => said.push_str(&more),
            Ok(None) => {}
            Err(error) => tracing::error!(%agent, %error, "could not note a closed question on its item"),
        }
        if let Some((whose, _)) = ask.item.rsplit_once('#') {
            factory.refresh_view(whose);
        }
        factory.broadcast_workspace(&factory.lock());
    }
    Ok(ToolResult::text(said, json!({})))
}

/// The person answers a procedure's current step: Done, or Can't do this
/// (`dismiss`), which stops it. The agent is told; the card shows the next
/// step, or that it's over.
fn step(factory: &Arc<Factory>, agent: &str, request: &v1::AnswerAsk, mut ask: Row) -> Result<(), RequestError> {
    let Some(answered) = request.step else {
        return Err(RequestError::Ask("Answer the step the request shows.".to_owned()));
    };
    if ask.steps.is_empty() {
        return Err(RequestError::Ask("This question has no steps.".to_owned()));
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
    // Files with the step (#511), kept on it, and as the card's latest
    // answer, as its comment is, for its agent to answer on.
    let attachments = factory.attachments(agent, &request.attachments)?;
    let stored = Some(&attachments).filter(|list| !list.is_empty()).and_then(|list| serde_json::to_string(list).ok());
    let done = if request.dismiss { "cant" } else { "done" };
    ask.steps[current] = Step {
        text: std::mem::take(&mut ask.steps[current].text),
        state: done.to_owned(),
        comment: comment.clone(),
        comment_attachments: attachments.clone(),
        decided: now,
    };
    let (state, decided) = match (request.dismiss, current + 1 == ask.steps.len()) {
        (true, _) => ("dismissed", Some(now)),
        (false, true) => ("answered", Some(now)),
        (false, false) => ("pending", None),
    };
    // Kept only if nothing answered it meanwhile (another device, a second
    // tap), and its agent didn't change it.
    let claimed = factory.store.db().execute(
        "UPDATE asks SET steps = ?2, state = ?3, answer = ?4, decided = ?5, answer_attachments = ?8
         WHERE id = ?1 AND state = 'pending' AND steps = ?6 AND revision = ?7",
        params![ask.id, serde_json::to_string(&ask.steps).unwrap_or_default(), state, comment, decided, before, ask.revision, stored],
    )?;
    if claimed == 0 {
        let changed = factory
            .store
            .db()
            .query_row("SELECT revision FROM asks WHERE id = ?1", [ask.id], |row| row.get::<_, i64>(0))
            .is_ok_and(|now| now != ask.revision);
        return Err(RequestError::Ask(if changed { CHANGED.to_owned() } else { "That step was already answered.".to_owned() }));
    }
    (ask.state, ask.answer, ask.decided, ask.answer_attachments) = (state.to_owned(), comment, decided, attachments);
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
    // Where they answered from and, with their words, to answer them on the
    // card (#430): before "You asked", as in the other events.
    let on = answered_on(factory, agent, &ask, request.from());
    let (summary, mut text) = told_step(&person, &ask, current, &on);
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
/// summary, and the event's text, with `on` (where they answered from, once
/// it's over) before "You asked".
fn told_step(person: &str, ask: &Row, at: usize, on: &str) -> (String, String) {
    let count = ask.steps.len();
    let step = &ask.steps[at];
    let number = at + 1;
    let title = &ask.title;
    let comment = if step.comment.is_empty() { String::new() } else { format!("\n\nTheir comment:\n\n{}", step.comment) };
    let comment = format!("{comment}{}", files_said(person, &ask.review, &step.comment_attachments));
    let asked = format!("{on}\n\nYou asked: {title}\n{}", ask.body);
    if step.state == "cant" {
        let rest = match count - number {
            0 => String::new(),
            1 => format!(" Step {count} wasn't shown to them."),
            _ => format!(" Steps {} to {count} weren't shown to them.", number + 1),
        };
        return (
            format!("{person} couldn't do step {number} of {count} of \"{title}\"."),
            format!(
                "{person} can't or won't do step {number} of {count} of \"{title}\": {}{comment}\n\nIt stops there, waiting for you, and nothing was marked done.{rest} If they can't, find a way without them and rewrite the steps in this request: {REPLY_TOOL} (id {id}) with steps, step {number} rewritten, and a short answer, and it opens again at step {number}. If their words say they won't, respect it: stop the request, and don't do that step another way. To stop: {REPLY_TOOL} (id {id}) with resolve: true and a short word for them, and the request shows it stopped at step {number}; then use your best judgment and say what you chose in your report.{asked}",
                step.text,
                id = ask.id
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
            "{person} did step {number} of {count} of \"{title}\": {}{comment}{on}\n\nThey see step {} next: {}\n\nYou're told as each step is done; meanwhile carry on with whatever doesn't depend on it.",
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

/// The page of the PDF a drawing of the person's is of: the app names each
/// one "<pdf stem>-p<page>.png"; None for any other file.
fn drawing_page(attachment: &factory_agent::Attachment) -> Option<u32> {
    let stem = attachment.name.strip_suffix(".png").or_else(|| attachment.name.strip_suffix(".PNG"))?;
    // A second upload of that name the same day is kept as
    // "<stem>-p<page> 2.png" (Sorbet's iPad pass).
    let stem = match stem.rsplit_once(' ') {
        Some((name, copy)) if !copy.is_empty() && copy.bytes().all(|b| b.is_ascii_digit()) => name,
        _ => stem,
    };
    let (_, page) = stem.rsplit_once("-p")?;
    page.parse().ok().filter(|page| *page > 0)
}

/// The lines naming the files the person answered with, by path, so its
/// agent reads them; empty for none. On a PDF's review card their drawings
/// on its pages say which pages (`drawings`); any other file (#511) is
/// listed with its name, type and size, as a message's attachments are.
fn files_said(person: &str, review: &str, attached: &[factory_agent::Attachment]) -> String {
    let pdf = review.to_ascii_lowercase().ends_with(".pdf");
    let (drawn, other): (Vec<_>, Vec<_>) = attached.iter().cloned().partition(|attachment| pdf && drawing_page(attachment).is_some());
    let mut said = drawings(person, &drawn);
    if other.is_empty() {
        return said;
    }
    let (count, them) = if other.len() == 1 { ("a file".to_owned(), "it") } else { (format!("{} files", other.len()), "them") };
    said.push_str(&format!("\n\n{person} attached {count}; read {them} at {}:", if other.len() == 1 { "its path" } else { "their paths" }));
    for file in &other {
        let kind = if file.mime.is_empty() { String::new() } else { format!(", {}", file.mime) };
        said.push_str(&format!("\n- {} (\"{}\"{kind}, {} bytes)", file.path, file.name, file.size));
    }
    if other.iter().any(|file| file.image_type().is_some()) {
        said.push_str("\nLook at images with read_image.");
    }
    said
}

/// The line naming the person's drawings on a PDF's pages, by path, so the
/// agent looks at them; empty for none.
fn drawings(person: &str, attached: &[factory_agent::Attachment]) -> String {
    if attached.is_empty() {
        return String::new();
    }
    let count = attached.len();
    let pages: Option<Vec<String>> = attached.iter().map(|attachment| drawing_page(attachment).map(|page| page.to_string())).collect();
    let which = match pages {
        Some(pages) if count == 1 => format!(" (page {})", pages[0]),
        Some(pages) => format!(" (pages {} and {})", pages[..count - 1].join(", "), pages[count - 1]),
        None => String::new(),
    };
    let paths: Vec<&str> = attached.iter().map(|attachment| attachment.path.as_str()).collect();
    let (pages, them) = if count == 1 { ("1 page".to_owned(), "it") } else { (format!("{count} pages"), "them") };
    format!("\n\n{person} drew on {pages}{which}; look at {them} with read_image: {}", paths.join(", "))
}

/// The most of a card's talk its agent's event quotes (#492), in characters.
const TALK_QUOTED: usize = 3000;

/// The end of a card's talk, for its agent's event when the person approved
/// or answered after it (#492): oldest first, each message under whose it
/// is, the latest ones as far as TALK_QUOTED goes, fenced as the talk.
fn talk_quoted(person: &str, thread: &[ThreadMessage]) -> String {
    let mut kept = Vec::new();
    let mut left = TALK_QUOTED;
    for message in thread.iter().rev() {
        if left == 0 {
            break;
        }
        let text = message.text.trim();
        let count = text.chars().count();
        let shown = if count <= left { text.to_owned() } else { format!("…{}", text.chars().skip(count - left).collect::<String>()) };
        left = left.saturating_sub(count);
        // Their files on that turn (#511), by path.
        let files = if message.attachments.is_empty() {
            String::new()
        } else {
            let paths: Vec<&str> = message.attachments.iter().map(|attachment| attachment.path.as_str()).collect();
            format!("\n(Attached: {})", paths.join(", "))
        };
        kept.push(format!("{}: {shown}{files}", if message.agent { "You" } else { person }));
    }
    let earlier = if kept.len() < thread.len() { "(Earlier messages left out.)\n" } else { "" };
    kept.reverse();
    let talk = kept.join("\n\n");
    // Longer than any run of backticks in it, so nothing in it closes it.
    let longest = talk.split(|c| c != '`').map(str::len).max().unwrap_or(0);
    let fence = "`".repeat((longest + 1).max(3));
    format!("{earlier}{fence}\n{talk}\n{fence}")
}

/// What its agent is told of a decided card: a summary, and the event's
/// text. `talk`: the talk it was approved or answered after (#492), quoted.
fn told(person: &str, ask: &Row, comments: Option<i64>, on: &str, talk: Option<&str>) -> (String, String) {
    let left = match comments {
        Some(0) => format!("\n\nThey left no comments on {}.", ask.review),
        Some(1) => format!("\n\nThey left 1 comment on {}; it came to you as a comment (comments_list shows it).", ask.review),
        Some(count) => {
            format!("\n\nThey left {count} comments on {}; they came to you as comments (comments_list shows them).", ask.review)
        }
        None => String::new(),
    };
    let drew = files_said(person, &ask.review, &ask.answer_attachments);
    let asked = format!("{left}{drew}{on}\n\nYou asked: {}\n{}", ask.title, ask.body);
    // A reply (#370): their words for what they want, never a yes or a no.
    if ask.replied {
        // Files alone (#511): they're named below.
        let said = if ask.answer.is_empty() { ", with files only.".to_owned() } else { format!(":\n\n{}", ask.answer) };
        // Send on a step (#533): the step still waits, nothing marked.
        if let Some(at) = ask.steps.iter().position(|step| step.state.is_empty()) {
            let (number, count, title, id) = (at + 1, ask.steps.len(), &ask.title, ask.id);
            let words = if ask.answer.is_empty() { "They sent files only.".to_owned() } else { format!("Their words:\n\n{}", ask.answer) };
            return (
                format!("{person} replied on step {number} of {count} of \"{title}\"."),
                format!(
                    "{person} replied on step {number} of {count} of \"{title}\": {}\n\n{words}\n\nStep {number} still waits; nothing was marked done. If the step is wrong, fix it in the request with {REPLY_TOOL} (id {id}, steps: the steps, step {number} rewritten) and a short answer; if it only needs an answer, {REPLY_TOOL} with text. They're shown step {number} again either way.{asked}",
                    ask.steps[at].text
                ),
            );
        }
        return (format!("{person} replied to \"{}\".", ask.title), format!("{person} replied to your question \"{}\"{said}{asked}", ask.title));
    }
    let title = &ask.title;
    if let Some(talk) = talk
        && ask.state == "answered"
    {
        let talked = format!("The talk in the request, oldest first:\n\n{talk}");
        if ask.wants_text {
            return (
                format!("{person} answered \"{title}\" after your reply on it."),
                format!(
                    "{person} answered your question \"{title}\" after your last reply in its request, so read their answer with the talk it follows.\n\n{talked}\n\nTheir answer:\n\n{}{asked}",
                    ask.answer
                ),
            );
        }
        let comment = if ask.answer.is_empty() { String::new() } else { format!("\n\nTheir comment:\n\n{}", ask.answer) };
        return (
            format!("{person} approved \"{title}\" after your reply on it."),
            format!(
                "{person} approved your question \"{title}\" after your last reply in its request, without the request changing: they agreed to where your talk there ended, not only to the request's words. Act on where it ended; if it's unclear what they agreed to, ask in the request with {REPLY_TOOL} (id {}).\n\n{talked}{comment}{asked}",
                ask.id
            ),
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

// ---- Cards Claktory settles itself (#471) ----

/// The action cards still waiting, every agent's: id, agent and action.
pub(crate) fn pending_actions(factory: &Factory) -> Vec<(i64, String, String)> {
    crate::store::query(
        &factory.store.db(),
        "SELECT id, agent, action FROM asks WHERE state = 'pending' AND action != '' ORDER BY id",
        [],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
    )
    .unwrap_or_else(|error| {
        tracing::error!(%error, "could not read the cards waiting");
        Vec::new()
    })
}

/// The action card `id` (as the app gives it), while it waits: its id,
/// agent and action.
pub(crate) fn pending_action(factory: &Factory, id: &str) -> Option<(i64, String, String)> {
    let number = card_number(id)?;
    factory
        .store
        .db()
        .query_row("SELECT id, agent, action FROM asks WHERE id = ?1 AND state = 'pending' AND action != ''", [number], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?))
        })
        .optional()
        .unwrap_or_else(|error| {
            tracing::error!(%error, "could not read a card");
            None
        })
}

/// Ask `id` as kept now, and its agent.
fn kept(factory: &Factory, id: i64) -> Option<(String, Row)> {
    let db = factory.store.db();
    let agent: String = db.query_row("SELECT agent FROM asks WHERE id = ?1", [id], |row| row.get(0)).ok()?;
    let ask = db.query_row(&format!("SELECT {COLUMNS} FROM asks WHERE id = ?1"), [id], row).ok()?;
    Some((agent, ask))
}

/// Shows ask `id` as it is now, in its agent's conversation and Needs you.
fn redraw(factory: &Factory, id: i64) -> Option<(String, Row)> {
    let (agent, ask) = kept(factory, id)?;
    let inner = &mut *factory.lock();
    let entry = inner
        .agents
        .get_mut(&agent)
        .and_then(|runtime| runtime.transcript.as_mut())
        .and_then(|transcript| transcript.set_ask(ask.to_proto()));
    if let Some(entry) = entry {
        factory.send_change(inner, &agent, crate::transcript::Change::Entry(entry));
    }
    factory.broadcast_workspace(inner);
    Some((agent, ask))
}

/// Withdraws a waiting card for Claktory, `reason` in Claktory's words: it
/// shows withdrawn, with it, in the conversation and in History.
pub(crate) fn withdraw_for(factory: &Factory, id: i64, reason: &str) {
    let withdrawn = factory.store.db().execute(
        "UPDATE asks SET state = 'withdrawn', decided = ?2, reason = ?3 WHERE id = ?1 AND state = 'pending'",
        params![id, now_ms(), reason],
    );
    match withdrawn {
        Ok(0) => {}
        Ok(_) => {
            redraw(factory, id);
        }
        Err(error) => tracing::error!(%error, "could not withdraw a card that's void"),
    }
}

/// Changes a waiting action card of `agent`'s in place, as ask_update
/// changes a question: a new revision, its agent's `reason` as the update's.
pub(crate) fn change_action(
    factory: &Factory,
    agent: &str,
    id: i64,
    (title, body): (&str, &str),
    (description, reason): (&str, &str),
    action: &str,
) -> Result<(), String> {
    let person = factory.lock().config.person().to_owned();
    let revision: i64 = factory
        .store
        .db()
        .query_row("SELECT revision FROM asks WHERE id = ?1 AND agent = ?2 AND state = 'pending'", params![id, agent], |row| row.get(0))
        .map_err(|_| format!("{person} just answered request {id}, so it wasn't changed; you'll be told their answer."))?;
    if revision > MAX_UPDATES {
        return Err(format!(
            "You've changed request {id} {MAX_UPDATES} times, the most: take it back with ask_withdraw, and ask again. Nothing was changed."
        ));
    }
    let changed = factory
        .store
        .db()
        .execute(
            "UPDATE asks SET title = ?3, body = ?4, action = ?5, action_description = ?6, action_reason = ?7,
             revision = revision + 1, updated = ?8, update_reason = ?7
             WHERE id = ?1 AND agent = ?2 AND state = 'pending' AND revision = ?9",
            params![id, agent, title, body, action, description, reason, now_ms(), revision],
        )
        .map_err(|error| format!("Couldn't change it: {error}"))?;
    if changed == 0 {
        return Err(format!("{person} just answered request {id}, so it wasn't changed; you'll be told their answer."));
    }
    redraw(factory, id);
    Ok(())
}

/// Marks a waiting action card answered, approved by the person through
/// the request it stands for (ApprovePipe, AcceptPipeIdentity): false when
/// it no longer waits.
pub(crate) fn claim_answered(factory: &Factory, id: i64) -> bool {
    factory
        .store
        .db()
        .execute("UPDATE asks SET state = 'answered', decided = ?2 WHERE id = ?1 AND state = 'pending'", params![id, now_ms()])
        .is_ok_and(|claimed| claimed == 1)
}

/// A claimed card waits again: what its approval was to make couldn't be
/// made, and the person was told why.
pub(crate) fn unclaim(factory: &Factory, id: i64) {
    if let Err(error) =
        factory.store.db().execute("UPDATE asks SET state = 'pending', decided = NULL WHERE id = ?1 AND state = 'answered'", [id])
    {
        tracing::error!(%error, "could not set a card waiting again");
    }
}

/// A claimed card shows answered, and its agent is told `text`.
pub(crate) fn told_answered(factory: &Arc<Factory>, id: i64, text: String, summary: String) {
    if let Some((agent, _)) = redraw(factory, id) {
        factory.deliver_event(&agent, text, host_event(summary));
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
            thread: Vec::new(),
            resolved: false,
            action_error: String::new(),
            run: None,
            file_updates: 0,
            update_note: String::new(),
        }
    }

    #[test]
    fn a_changed_procedure_keeps_the_steps_done_up_to_the_first_change() {
        let step = |text: &str, state: &str| Step { text: text.to_owned(), state: state.to_owned(), ..Step::default() };
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
        let (summary, text) = told("Matt", &replied, None, "", None);
        assert_eq!(summary, "Matt replied to \"Deploy to production?\".");
        assert_eq!(
            text,
            "Matt replied to your question \"Deploy to production?\":\n\nNo, staging first.\n\nYou asked: Deploy to production?\nThe release is ready."
        );
        assert!(!text.contains("yes") && !text.contains("dismissed"), "{text}");
    }

    #[test]
    fn the_agent_is_told_what_the_person_decided() {
        let (summary, text) = told("Matt", &ask("dismissed", false, ""), None, "", None);
        assert_eq!(summary, "Matt dismissed your question \"Deploy to production?\".");
        assert!(
            text.contains("Use your best judgment") && text.ends_with("You asked: Deploy to production?\nThe release is ready."),
            "{text}"
        );
        assert!(
            told("Matt", &ask("answered", false, ""), None, "", None)
                .1
                .starts_with("Matt answered your question \"Deploy to production?\": yes, go ahead.")
        );
        assert!(told("Matt", &ask("answered", false, "After 5 pm."), None, "", None).1.contains("yes, go ahead. Their comment:\n\nAfter 5 pm."));
        let (_, text) = told("Matt", &ask("dismissed", false, "Not this week."), None, "", None);
        assert!(
            text.starts_with(
                "Matt dismissed your question \"Deploy to production?\": they won't answer it. Their comment:\n\nNot this week.\n\nUse your best judgment"
            ),
            "{text}"
        );
        let (summary, text) = told("Matt", &ask("answered", true, "Use the blue one."), None, "", None);
        assert_eq!(summary, "Matt answered \"Deploy to production?\".");
        assert!(text.starts_with("Matt answered your question \"Deploy to production?\":\n\nUse the blue one."), "{text}");
    }

    #[test]
    fn approving_after_a_reply_on_an_unchanged_card_follows_the_talk_quoted_fenced_and_capped() {
        let said = |agent: bool, text: &str, at: i64| ThreadMessage { agent, text: text.to_owned(), at, attachments: Vec::new(), step: None };
        let talk = vec![said(false, "No paid detectors.", 10), said(true, "Then the free plan: ```GPTZero``` only.", 20)];
        let pending = Row { thread: talk.clone(), ..ask("pending", false, "") };
        assert!(pending.follows_talk());
        // The reply gave the card new words, or the card changed since: it
        // reads as the ask itself.
        assert!(!Row { updated: Some(20), ..pending.clone() }.follows_talk());
        assert!(!Row { updated: Some(30), ..pending.clone() }.follows_talk());
        assert!(Row { updated: Some(15), ..pending.clone() }.follows_talk());
        // A card that makes a change, a procedure or a decided card never does.
        assert!(!Row { action: "{}".to_owned(), ..pending.clone() }.follows_talk());
        assert!(!Row { clears: "kenji".to_owned(), ..pending.clone() }.follows_talk());
        assert!(!Row { steps: vec![Step::default()], ..pending.clone() }.follows_talk());
        assert!(!Row { thread: talk.clone(), ..ask("answered", false, "") }.follows_talk());
        assert!(pending.to_proto().approve_follows_talk);

        let quoted = talk_quoted("Matt", &talk);
        assert_eq!(quoted, "````\nMatt: No paid detectors.\n\nYou: Then the free plan: ```GPTZero``` only.\n````");
        let approved = Row { thread: talk.clone(), ..ask("answered", false, "") };
        let (summary, text) = told("Matt", &approved, None, "", Some(&quoted));
        assert_eq!(summary, "Matt approved \"Deploy to production?\" after your reply on it.");
        assert!(text.contains("they agreed to where your talk there ended") && text.contains(&quoted), "{text}");
        assert!(!text.contains("yes, go ahead"), "{text}");

        // A long talk: the latest words, at most TALK_QUOTED of them.
        let long = vec![said(false, &"a".repeat(5000), 1), said(true, &"b".repeat(2500), 2), said(false, &"c".repeat(600), 3)];
        let quoted = talk_quoted("Matt", &long);
        assert!(quoted.starts_with("(Earlier messages left out.)\n```\nYou: …bbb"), "{}", quoted.chars().take(80).collect::<String>());
        assert_eq!(quoted.matches('b').count(), TALK_QUOTED - 600);
        assert!(quoted.ends_with(&format!("Matt: {}\n```", "c".repeat(600))));
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
        // The card gives fenced code blocks a Copy button, not inline code
        // (#238); one to run here is an Execute card (#494).
        assert!(ABOUT.contains("a command they should run on this computer goes in command instead"), "{ABOUT}");
        assert!(ABOUT.contains("one for somewhere else goes in a fenced code block, which the request gives a Copy button"), "{ABOUT}");
    }
}
