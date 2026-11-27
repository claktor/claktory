//! Proposals to Rivet. No agent can message Rivet, the admin agent: only the
//! person can. An agent that reaches a conclusion Rivet should act on
//! (another agent is needed, say) proposes a message with propose_to_rivet
//! instead. The person sees it in that agent's conversation and approves it,
//! with a comment, or dismisses it. Approved, Rivet gets it as the person's
//! own message; either way, the agent is told.
//!
//! Proposals live in factory.db. The conversation shows each as an entry
//! after the propose_to_rivet call that made it (its result names it), so it
//! reloads where it was; its state comes from here. One that no longer
//! applies, its agent takes back with proposal_withdraw (#407).

use std::fmt::Write as _;
use std::sync::Arc;

use factory_agent::{Tool, ToolResult};
use factory_protocol::v1::{self, ProposalState};
use rusqlite::{OptionalExtension, params};
use serde::Deserialize;
use serde_json::json;

use crate::config::{self, AgentConfig, Config};
use crate::extensions::{agents, enabled, switch};
use crate::factory::{Factory, RequestError, host_event};
use crate::store::Store;
use crate::transcript::{Transcript, now_ms};

pub(crate) const SCHEMA: &str = "
-- Messages agents proposed for Rivet, which only the person can message.
CREATE TABLE proposals (
    id INTEGER PRIMARY KEY,
    -- The agent that proposed it.
    agent TEXT NOT NULL,
    text TEXT NOT NULL,
    created INTEGER NOT NULL,
    -- pending, approved or dismissed
    state TEXT NOT NULL,
    -- What the person added when deciding; '' for nothing.
    comment TEXT NOT NULL,
    -- When the person decided; null while pending.
    decided INTEGER
);
CREATE INDEX proposals_by_agent ON proposals (agent, id);
";

/// The migration that added answers (#370 Submit): 1 when the person
/// answered it with a comment rather than approving it (state 'approved':
/// Rivet acts on what they said).
pub(crate) const REPLIED: &str = "
ALTER TABLE proposals ADD COLUMN replied INTEGER NOT NULL DEFAULT 0;
";

/// The migration that let an agent take back its own proposal once it no
/// longer applies (proposal_withdraw, #407): state 'withdrawn', decided
/// when it was, and this, the agent's reason; '' for every other state.
pub(crate) const WITHDRAWN: &str = "
ALTER TABLE proposals ADD COLUMN reason TEXT NOT NULL DEFAULT '';
";

/// The migration that let an agent change its own proposal's text while it
/// waits (proposal_update, #428), as ask::UPDATED does for questions.
pub(crate) const UPDATED: &str = "
ALTER TABLE proposals ADD COLUMN revision INTEGER NOT NULL DEFAULT 1;
ALTER TABLE proposals ADD COLUMN updated INTEGER;
ALTER TABLE proposals ADD COLUMN update_reason TEXT NOT NULL DEFAULT '';
";

/// The tool, one of the Messaging extension's, for agents that aren't admins.
pub(crate) const TOOL: &str = "propose_to_rivet";
/// Takes back one of the agent's own proposals still waiting (#407).
pub(crate) const WITHDRAW_TOOL: &str = "proposal_withdraw";
/// Changes one of the agent's own proposals still waiting (#428).
pub(crate) const UPDATE_TOOL: &str = "proposal_update";

const UPDATE_ABOUT: &str = "Change one of your proposals to Rivet still waiting for the person, in place, when what you propose changes but still needs Rivet: a detail was wrong, or the need moved. id: the proposal's id, which propose_to_rivet's result gave; message: the whole new message, standing alone; reason: optional, why it changed, in a short line they read in the request (at most 300 characters). Their request redraws where it is, marked updated, without counting again or notifying them again; approving the version before is refused, so they look again. At most 10 changes per proposal; take back one that no longer applies with proposal_withdraw instead. You can change only your own proposals.";

const WITHDRAW_ABOUT: &str = "Take back one of your proposals to Rivet still waiting for the person, as soon as it no longer applies: the need went away, or it was settled elsewhere. id: the proposal's id, which propose_to_rivet's result gave; reason: why, in a short line they read in Needs you's History (at most 300 characters). Without id, lists your proposals waiting, with their ids. It leaves their Needs you at once; a checklist item it was for stays as it is, so update the item yourself. You can take back only your own proposals.";

const ABOUT: &str = "Propose a message for Rivet, the admin agent, which only the person can message: for a conclusion Rivet should act on, such as adding an agent. The person approves it, with a comment, or dismisses it, and you're told which; approved, Rivet gets it from the person. Write it to stand alone, with all the context Rivet needs: Rivet can't see your conversation. Say what's needed and why (the job, its scope, who it works with, the access it needs), never which model or effort: Rivet chooses those. item: the checklist item it's for (12 for #12, or Ana#12), whenever one waits on it: the proposal shows as that item in the person's Needs you, so don't also ask them about it. grants: when it asks Rivet to give something it gives only in a request the person approves (an app's access, a pipe widened, an app uninstalled keeping its data, Continue automatically), name each exactly, as that tool's arguments: {tool: \"apps_set_access\", args: {app, sources, agents}}, or pipes_settings, apps_uninstall (keep_data true) or models_auto_continue with theirs, reason left out. The request shows them in Claktory's words, and approving it lets Rivet make each once, within a day, without asking again; name only what the work needs.";

/// Rivet: the admin agent proposals go to. The one with Rivet's id if it's
/// an admin agent, else the first admin agent in the settings.
pub(crate) fn rivet(config: &Config) -> Option<&AgentConfig> {
    let admins = || config.agents.iter().filter(|agent| is_admin(agent));
    admins().find(|agent| agent.id == config::RIVET).or_else(|| admins().next())
}

pub(crate) fn is_admin(agent: &AgentConfig) -> bool {
    enabled(agent, agents::ID) && switch(agent, agents::ID, "admin")
}

#[derive(Deserialize)]
struct ProposeArgs {
    message: String,
    /// The checklist item it's for (12, or Ana#12).
    #[serde(default)]
    item: Option<String>,
    /// What it asks Rivet to give, exactly (`crate::proposal_grants`).
    #[serde(default)]
    grants: Vec<crate::proposal_grants::Asked>,
}

/// The grants parameter, in propose_to_rivet and proposal_update.
fn grants_parameter() -> serde_json::Value {
    json!({
        "type": "array",
        "maxItems": 8,
        "items": {
            "type": "object",
            "required": ["tool", "args"],
            "properties": {"tool": {"type": "string", "enum": crate::proposal_grants::TOOLS}, "args": {"type": "object"}},
            "additionalProperties": false,
        },
    })
}

pub(crate) fn tool(factory: &Arc<Factory>, agent: &str) -> Tool {
    let parameters = json!({
        "type": "object",
        "required": ["message"],
        "properties": {"message": {"type": "string", "minLength": 1}, "item": {"type": "string"}, "grants": grants_parameter()},
        "additionalProperties": false,
    });
    crate::extensions::tool(
        TOOL,
        "Propose to Rivet",
        ABOUT,
        parameters,
        crate::extensions::typed_handler(factory, agent, |factory, agent, args: ProposeArgs| {
            propose(factory, agent, args.message, args.item.as_deref(), &args.grants)
        }),
    )
}

pub(crate) fn update_tool(factory: &Arc<Factory>, agent: &str) -> Tool {
    let parameters = json!({
        "type": "object",
        "required": ["id", "message"],
        "properties": {"id": {"type": "string"}, "message": {"type": "string", "minLength": 1}, "reason": {"type": "string"}, "grants": grants_parameter()},
        "additionalProperties": false,
    });
    crate::extensions::tool(
        UPDATE_TOOL,
        "Change a proposal",
        UPDATE_ABOUT,
        parameters,
        crate::extensions::typed_handler(factory, agent, update),
    )
}

pub(crate) fn withdraw_tool(factory: &Arc<Factory>, agent: &str) -> Tool {
    let parameters = json!({
        "type": "object",
        "properties": {"id": {"type": "string"}, "reason": {"type": "string"}},
        "additionalProperties": false,
    });
    crate::extensions::tool(
        WITHDRAW_TOOL,
        "Withdraw a proposal",
        WITHDRAW_ABOUT,
        parameters,
        crate::extensions::typed_handler(factory, agent, withdraw),
    )
}

/// Records the agent's proposal, or says why not. The result names it, so
/// its conversation shows it after the call.
fn propose(
    factory: &Arc<Factory>,
    agent: &str,
    text: String,
    item: Option<&str>,
    grants: &[crate::proposal_grants::Asked],
) -> Result<ToolResult, String> {
    let text = text.trim().to_owned();
    if text.is_empty() {
        return Err("The message is empty; nothing was proposed.".to_owned());
    }
    let (person, rivet_id) = {
        let inner = factory.lock();
        // A chat proposes as any agent does; it's never an admin.
        if !crate::chats::kept(factory, agent) {
            let me = crate::extensions::caller(&inner.config, agent)?;
            if is_admin(me) {
                return Err("You're an admin agent: message agents directly instead.".to_owned());
            }
        }
        let Some(rivet) = rivet(&inner.config) else {
            return Err("This Claktory has no Rivet (admin agent) to propose to, so nothing was proposed.".to_owned());
        };
        if !inner.models.iter().any(|model| model.id == rivet.model) {
            return Err(format!("{} has no model available, so it can't act on a proposal; nothing was proposed.", rivet.name));
        }
        (inner.config.person().to_owned(), rivet.id.clone())
    };
    // What it asks Rivet to give, resolved now, as the tools would.
    let resolved =
        crate::proposal_grants::resolve_all(factory, agent, &rivet_id, grants).map_err(|why| format!("{why} Nothing was proposed."))?;
    // The checklist item it's for: it shows as that item in Needs you. A
    // chat has no checklist.
    let (item, linked) = if crate::chats::kept(factory, agent) {
        (String::new(), None)
    } else {
        crate::extensions::checklist::item_for(factory, agent, item).map_err(|problem| format!("item: {problem}. Nothing was proposed."))?
    };
    // Read a pipe: the card says so, and where from.
    let routes = crate::pipes::agents::routes(factory, agent);
    let taint = (!routes.is_empty()).then(|| crate::pipes::agents::routes_json(&routes));
    let id = {
        let db = factory.store.db();
        db.execute(
            "INSERT INTO proposals (agent, text, created, state, comment, taint) VALUES (?1, ?2, ?3, 'pending', '', ?4)",
            params![agent, text, now_ms(), taint.as_ref().map(serde_json::Value::to_string)],
        )
        .map_err(|error| format!("Couldn't record the proposal: {error}"))?;
        let id = db.last_insert_rowid();
        crate::proposal_grants::keep(&db, id, grants, &resolved).map_err(|error| format!("Couldn't record the proposal: {error}"))?;
        id
    };
    if !item.is_empty() {
        crate::extensions::checklist::link(&factory.store, &crate::extensions::checklist::Card::Proposal(id), agent, &item);
        factory.refresh_view(agent);
        factory.broadcast_workspace(&factory.lock());
    }
    let id = id.to_string();
    // A chat has no proposal_withdraw.
    let take_back = if crate::chats::kept(factory, agent) {
        String::new()
    } else {
        format!(
            " Its id is {id}: if it stops applying before they decide in the request (they settle it in your chat, say), take it back with proposal_withdraw, or change it with proposal_update if what you propose changes."
        )
    };
    let said =
        format!("Proposed; {person} will approve or dismiss it.{take_back}{}", crate::extensions::checklist::taken_for(linked, &person));
    let mut details = json!({"proposal": id, "text": text});
    if let Some(taint) = taint {
        details["taint"] = taint;
    }
    if !resolved.is_empty() {
        details["grants"] = crate::proposal_grants::details(&resolved);
    }
    Ok(ToolResult::text(said, details))
}

/// The entry a propose_to_rivet result shows (called directly, or through
/// extension_call), pending; None for any other result, or one that
/// proposed nothing.
pub(crate) fn proposed(tool: &str, is_error: bool, details: &serde_json::Value) -> Option<v1::Proposal> {
    if !matches!(tool, TOOL | "extension_call") || is_error {
        return None;
    }
    let id = details.get("proposal")?.as_str()?.to_owned();
    let text = details.get("text").and_then(serde_json::Value::as_str).unwrap_or_default().to_owned();
    let taint = details.get("taint").map(crate::pipes::agents::routes_from_json).unwrap_or_default();
    let grants = details.get("grants").map(crate::proposal_grants::from_details).unwrap_or_default();
    Some(v1::Proposal { id, text, state: ProposalState::Pending.into(), taint, revision: 1, grants, ..Default::default() })
}

/// A proposal as recorded.
struct Row {
    id: i64,
    text: String,
    state: String,
    comment: String,
    decided: Option<i64>,
    /// JSON (`pipes::agents::routes_json`); null for none.
    taint: Option<String>,
    /// Answered with a comment (#370), not approved.
    replied: bool,
    /// Withdrawn: the agent's reason.
    reason: String,
    created: i64,
    /// Which version it is (#428): 1 as proposed, one more each change.
    revision: i64,
    /// When its agent last changed it; None if it never did.
    updated: Option<i64>,
    /// Why, in the agent's words; '' for no reason given.
    update_reason: String,
}

const COLUMNS: &str = "id, text, state, comment, decided, taint, replied, reason, created, revision, updated, update_reason";

fn row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Row> {
    row_at(row, 0)
}

/// A proposal's COLUMNS, from column `at` on.
fn row_at(row: &rusqlite::Row<'_>, at: usize) -> rusqlite::Result<Row> {
    Ok(Row {
        id: row.get(at)?,
        text: row.get(at + 1)?,
        state: row.get(at + 2)?,
        comment: row.get(at + 3)?,
        decided: row.get(at + 4)?,
        taint: row.get(at + 5)?,
        replied: row.get(at + 6)?,
        reason: row.get(at + 7)?,
        created: row.get(at + 8)?,
        revision: row.get(at + 9)?,
        updated: row.get(at + 10)?,
        update_reason: row.get(at + 11)?,
    })
}

impl Row {
    fn to_proto(&self) -> v1::Proposal {
        let state = match self.state.as_str() {
            "approved" => ProposalState::Approved,
            "dismissed" | "withdrawn" => ProposalState::Dismissed,
            _ => ProposalState::Pending,
        };
        v1::Proposal {
            id: self.id.to_string(),
            text: self.text.clone(),
            state: state.into(),
            comment: self.comment.clone(),
            decided_at_ms: self.decided.unwrap_or_default(),
            taint: self
                .taint
                .as_deref()
                .and_then(|taint| serde_json::from_str(taint).ok())
                .map(|taint| crate::pipes::agents::routes_from_json(&taint))
                .unwrap_or_default(),
            replied: self.replied,
            withdrawn: self.state == "withdrawn",
            withdraw_reason: self.reason.clone(),
            revision: u32::try_from(self.revision).unwrap_or(1),
            updated_ms: self.updated.unwrap_or_default(),
            update_reason: self.update_reason.clone(),
            grants: Vec::new(),
        }
    }

    /// The proposal as its card shows it, with its grants.
    fn shown(&self, db: &rusqlite::Connection) -> v1::Proposal {
        v1::Proposal { grants: crate::proposal_grants::shown(db, self.id), ..self.to_proto() }
    }
}

/// The proposal's card redraws as it is now (one of its grants was used).
pub(crate) fn refresh(factory: &Factory, id: i64) {
    let found = {
        let db = factory.store.db();
        db.query_row(&format!("SELECT agent, {COLUMNS} FROM proposals WHERE id = ?1"), [id], |found| {
            let agent: String = found.get(0)?;
            let proposal = self::row_at(found, 1)?;
            Ok((agent, proposal))
        })
        .ok()
        .map(|(agent, proposal)| (agent, proposal.shown(&db)))
    };
    let Some((agent, proposal)) = found else { return };
    let mut inner = factory.lock();
    let entry = inner
        .agents
        .get_mut(&agent)
        .and_then(|runtime| runtime.transcript.as_mut())
        .and_then(|transcript| transcript.set_proposal(proposal));
    if let Some(entry) = entry {
        factory.send_change(&inner, &agent, crate::transcript::Change::Entry(entry));
    }
}

/// Shows the agent's decided proposals as decided in its just-loaded
/// conversation, and those it changed as they are now.
pub(crate) fn restore(store: &Store, agent: &str, transcript: &mut Transcript) -> rusqlite::Result<()> {
    let db = store.db();
    let mut statement =
        db.prepare(&format!("SELECT {COLUMNS} FROM proposals WHERE agent = ?1 AND (state != 'pending' OR revision > 1) ORDER BY id"))?;
    let rows: Vec<Row> = statement.query_map([agent], row)?.collect::<rusqlite::Result<_>>()?;
    for row in rows {
        transcript.set_proposal(row.shown(&db));
    }
    Ok(())
}

/// Whether the agent has a proposal waiting for the person.
pub(crate) fn pending_of(store: &Store, agent: &str) -> bool {
    store
        .db()
        .query_row("SELECT EXISTS (SELECT 1 FROM proposals WHERE agent = ?1 AND state = 'pending')", [agent], |row| row.get(0))
        .unwrap_or(false)
}

/// An agent is deleted, and its proposals with it.
pub(crate) fn forget(store: &Store, agent: &str) -> rusqlite::Result<()> {
    store.db().execute("DELETE FROM proposals WHERE agent = ?1", [agent])?;
    Ok(())
}

/// The person approves (with `comment`) or dismisses `agent`'s proposal,
/// or answers it (`reply`, #370 Submit): Rivet gets it with their comment
/// as what to do. Approved or answered, Rivet gets it as the person's
/// message; it stays pending when there's no Rivet, or Rivet has no model.
/// Either way the agent is told.
pub(crate) fn decide(
    factory: &Arc<Factory>,
    agent: &str,
    id: &str,
    approve: bool,
    reply: bool,
    comment: &str,
    revision: Option<u32>,
) -> Result<(), RequestError> {
    let comment = crate::decisions::comment(comment)?;
    if reply && comment.is_empty() {
        return Err(RequestError::Proposal("Write an answer, or approve or dismiss the proposal.".to_owned()));
    }
    // An answer goes to Rivet as an approval does.
    let approve = approve || reply;
    let unknown = || RequestError::UnknownProposal(id.to_owned());
    let number: i64 = id.trim().parse().map_err(|_| unknown())?;
    let found = factory
        .store
        .db()
        .query_row(&format!("SELECT {COLUMNS} FROM proposals WHERE id = ?1 AND agent = ?2"), params![number, agent], row)
        .optional()?;
    let Some(mut row) = found else { return Err(unknown()) };
    if row.state == "withdrawn" {
        return Err(RequestError::Proposal("This proposal was withdrawn.".to_owned()));
    }
    if row.state != "pending" {
        return Err(RequestError::Proposal(format!("This proposal was already {}.", row.state)));
    }
    crate::extensions::ask::seen(revision, row.revision, row.updated).map_err(RequestError::Proposal)?;
    // What it asks Rivet to give, in Claktory's words: approved (not
    // answered), Rivet reads them apart from the agent's words.
    let grants = if approve && !reply { crate::proposal_grants::shown(&factory.store.db(), number) } else { Vec::new() };
    // Who it's for, and what Rivet reads.
    let (rivet_name, person, delivery) = {
        let inner = factory.lock();
        let config = &inner.config;
        // Signed as in any message to Rivet; a chat by its title.
        let from = match config.agent(agent) {
            Some(from) => Some(from),
            None if crate::chats::kept(factory, agent) => None,
            None => return Err(RequestError::UnknownAgent(agent.to_owned())),
        };
        let rivet = rivet(config);
        let delivery = match rivet {
            _ if !approve => None,
            None => return Err(RequestError::Proposal("There's no Rivet (admin agent) to send it to.".to_owned())),
            Some(rivet) if !inner.models.iter().any(|model| model.id == rivet.model) => {
                return Err(RequestError::Proposal(format!(
                    "{} has no model available: choose one for it, then approve this again.",
                    rivet.name
                )));
            }
            Some(rivet) => {
                let signed = match from {
                    Some(from) => agents::signed(config, from, rivet),
                    None => crate::chats::signed(factory, agent),
                };
                let mut message = String::new();
                if !grants.is_empty() {
                    let proposer = from.map_or_else(|| signed.clone(), |from| from.name.clone());
                    message = crate::proposal_grants::message(
                        config.person_capitalized(),
                        &signed,
                        &proposer,
                        &row.id.to_string(),
                        comment,
                        &row.text,
                        &grants,
                    );
                } else if reply {
                    // Their comment is what to do; the proposal only says
                    // what it was about.
                    let (person, named) = (config.person(), config.person_capitalized());
                    let _ = write!(
                        message,
                        "{named} answered {signed}'s proposal:\n\n{comment}\n\nDo what {person} says; the proposal is for context.\n\n"
                    );
                } else if !comment.is_empty() {
                    message.push_str(comment);
                    message.push_str("\n\n");
                }
                if grants.is_empty() {
                    let _ = write!(message, "{signed} proposes:\n\n{}", row.text);
                }
                Some((rivet.id.clone(), message))
            }
        };
        let rivet_name = rivet.map_or_else(|| "Rivet".to_owned(), |rivet| rivet.name.clone());
        (rivet_name, config.person_capitalized().to_owned(), delivery)
    };
    let state = if approve { "approved" } else { "dismissed" };
    let decided = now_ms();
    // Kept only if nothing decided or changed it meanwhile.
    // Only the admin agent it goes to may use its grants.
    let sent_to = delivery.as_ref().map(|(rivet, _)| rivet.clone()).unwrap_or_default();
    let claimed = factory.store.db().execute(
        "UPDATE proposals SET state = ?2, comment = ?3, decided = ?4, replied = ?5, rivet = ?7 WHERE id = ?1 AND state = 'pending' AND revision = ?6",
        params![number, state, comment, decided, reply, row.revision, sent_to],
    )?;
    if claimed == 0 {
        let now = factory.store.db().query_row("SELECT state FROM proposals WHERE id = ?1", [number], |row| row.get::<_, String>(0));
        return Err(RequestError::Proposal(match now.as_deref() {
            Ok("pending") => crate::extensions::ask::CHANGED.to_owned(),
            Ok("withdrawn") => "This proposal was withdrawn.".to_owned(),
            _ => "This proposal was already decided.".to_owned(),
        }));
    }
    if let Some((rivet, message)) = &delivery
        && let Err(error) = factory.send_message(rivet, message, None, "", &[])
    {
        factory.store.db().execute(
            "UPDATE proposals SET state = 'pending', comment = '', decided = NULL, replied = 0, rivet = '' WHERE id = ?1",
            [number],
        )?;
        return Err(error);
    }
    (row.state, row.comment, row.decided, row.replied) = (state.to_owned(), comment.to_owned(), Some(decided), reply);
    let shown_grants = crate::proposal_grants::shown(&factory.store.db(), number);
    // Its entry shows the decision.
    {
        let mut inner = factory.lock();
        let entry = inner
            .agents
            .get_mut(agent)
            .and_then(|runtime| runtime.transcript.as_mut())
            .and_then(|transcript| transcript.set_proposal(v1::Proposal { grants: shown_grants, ..row.to_proto() }));
        if let Some(entry) = entry {
            factory.send_change(&inner, agent, crate::transcript::Change::Entry(entry));
            // It no longer waits for the person (Agent.waiting_seqs).
            factory.broadcast_workspace(&inner);
        }
    }
    let about = crate::extensions::checklist::card_decided(factory, &crate::extensions::checklist::Card::Proposal(number), !approve);
    let (summary, text) = if reply {
        (
            format!("{person} answered your proposal to {rivet_name}."),
            format!(
                "{person} answered your proposal; {rivet_name} acts on what they said:\n\n{comment}\n\nYour proposal:\n{}{about}",
                row.text
            ),
        )
    } else {
        let summary = format!("{person} {state} your proposal to {rivet_name}.");
        let text = format!("{summary}{}\n\nYour proposal:\n{}{about}", crate::decisions::note(&person, comment), row.text);
        (summary, text)
    };
    // Dismissed without a comment, it's told quietly (friend bug #19).
    let event = if approve || !comment.is_empty() { host_event(summary) } else { crate::needs_you::dismissed_event(summary) };
    factory.deliver_event(agent, text, event);
    Ok(())
}

/// proposal_withdraw (#407): takes back one of the agent's own proposals
/// still waiting, with its reason, or lists them. It leaves Needs you at
/// once and shows as withdrawn; the item it was for stays as it was, for
/// the agent to update.
fn withdraw(factory: &Arc<Factory>, agent: &str, args: crate::extensions::ask::WithdrawArgs) -> Result<ToolResult, String> {
    use crate::extensions::ask::{card_number, item_named, withdraw_reason};
    let person = {
        let inner = factory.lock();
        crate::extensions::caller(&inner.config, agent)?;
        inner.config.person().to_owned()
    };
    let couldnt = |error: rusqlite::Error| format!("Couldn't read your proposals: {error}");
    // The checklist item a proposal is for ("agent#12"), if any.
    let item_of = |id: i64| -> Option<String> {
        factory
            .store
            .db()
            .query_row("SELECT item FROM card_items WHERE kind = 'proposal' AND card = ?1", [id.to_string()], |row| row.get(0))
            .optional()
            .unwrap_or_default()
    };
    let Some(given) = args.id.as_deref().map(str::trim).filter(|id| !id.is_empty()) else {
        let waiting = crate::store::query(
            &factory.store.db(),
            &format!("SELECT {COLUMNS} FROM proposals WHERE agent = ?1 AND state = 'pending' ORDER BY id"),
            [agent],
            row,
        )
        .map_err(couldnt)?;
        if waiting.is_empty() {
            return Ok(ToolResult::text(format!("You have no proposals waiting for {person}."), json!({})));
        }
        let mut said = format!("Your proposals waiting for {person}; take one back with its id and a reason:");
        for proposal in &waiting {
            let when = factory_agent::accounts::local_time(u64::try_from(proposal.created).unwrap_or_default()).unwrap_or_default();
            let first = proposal.text.lines().find(|line| !line.trim().is_empty()).unwrap_or_default().trim();
            let first: String = first.chars().take(120).collect();
            let _ = write!(said, "\n- {}: \"{first}\", proposed {when}", proposal.id);
            if let Some(item) = item_of(proposal.id) {
                let _ = write!(said, ", for {}", item_named(factory, agent, &item));
            }
        }
        return Ok(ToolResult::text(said, json!({})));
    };
    let unknown = || format!("You have no proposal {given}: proposal_withdraw without id lists yours. Nothing was withdrawn.");
    let number = card_number(given).ok_or_else(unknown)?;
    let reason = withdraw_reason(args.reason.as_deref(), &person)?;
    // Only the agent's own: another's is as unknown as one never made.
    let found = factory
        .store
        .db()
        .query_row(&format!("SELECT {COLUMNS} FROM proposals WHERE id = ?1 AND agent = ?2"), params![number, agent], row)
        .optional()
        .map_err(couldnt)?;
    let Some(mut proposal) = found else { return Err(unknown()) };
    if proposal.state != "pending" {
        let was = match proposal.state.as_str() {
            "approved" if proposal.replied => format!("{person} already answered it"),
            "approved" => format!("{person} already approved it"),
            "dismissed" => format!("{person} already dismissed it"),
            "withdrawn" => "You already withdrew it".to_owned(),
            _ => "It's already closed".to_owned(),
        };
        return Err(format!("Proposal {number} isn't waiting: {was}. Nothing was withdrawn."));
    }
    let decided = now_ms();
    // Kept only if the person didn't decide it meanwhile.
    let claimed = factory
        .store
        .db()
        .execute(
            "UPDATE proposals SET state = 'withdrawn', decided = ?3, reason = ?4 WHERE id = ?1 AND agent = ?2 AND state = 'pending'",
            params![number, agent, decided, reason],
        )
        .map_err(|error| format!("Couldn't withdraw it: {error}"))?;
    if claimed == 0 {
        return Err(format!("{person} just decided proposal {number}, so it wasn't withdrawn; you'll be told what they decided."));
    }
    (proposal.state, proposal.decided, proposal.reason) = ("withdrawn".to_owned(), Some(decided), reason);
    // Its card shows it withdrawn, and it no longer waits for the person.
    {
        let shown = proposal.shown(&factory.store.db());
        let mut inner = factory.lock();
        let entry = inner
            .agents
            .get_mut(agent)
            .and_then(|runtime| runtime.transcript.as_mut())
            .and_then(|transcript| transcript.set_proposal(shown));
        if let Some(entry) = entry {
            factory.send_change(&inner, agent, crate::transcript::Change::Entry(entry));
        }
        factory.broadcast_workspace(&inner);
    }
    let mut said = format!(
        "Withdrew proposal {number}: it's gone from {person}'s Needs you, and their History shows it withdrawn by you, with your reason."
    );
    if let Some(item) = item_of(number) {
        if let Some((whose, _)) = item.rsplit_once('#') {
            factory.refresh_view(whose);
        }
        let _ = write!(
            said,
            " It was for {}, which stays as it was: update it if it no longer waits on {person}.",
            item_named(factory, agent, &item)
        );
    }
    Ok(ToolResult::text(said, json!({})))
}

#[derive(Deserialize)]
struct UpdateArgs {
    id: String,
    message: String,
    #[serde(default)]
    reason: Option<String>,
    /// Its grants from now on; left out, those it has, resolved again.
    #[serde(default)]
    grants: Option<Vec<crate::proposal_grants::Asked>>,
}

/// proposal_update (#428): changes the text of one of the agent's own
/// proposals still waiting, in place, as ask_update changes a question:
/// a new version, redrawn where it is, not counted or notified again.
fn update(factory: &Arc<Factory>, agent: &str, args: UpdateArgs) -> Result<ToolResult, String> {
    use crate::extensions::ask::{MAX_UPDATES, card_number, too_many_updates, update_reason};
    let person = {
        let inner = factory.lock();
        crate::extensions::caller(&inner.config, agent)?;
        inner.config.person().to_owned()
    };
    let given = args.id.trim();
    let unknown = || format!("You have no proposal {given}: proposal_withdraw without id lists yours. Nothing was changed.");
    let number = card_number(given).ok_or_else(unknown)?;
    let reason = update_reason(args.reason.as_deref())?;
    let text = args.message.trim().to_owned();
    if text.is_empty() {
        return Err("The message is empty: give the whole new message. Nothing was changed.".to_owned());
    }
    // Only the agent's own: another's is as unknown as one never made.
    let found = factory
        .store
        .db()
        .query_row(&format!("SELECT {COLUMNS} FROM proposals WHERE id = ?1 AND agent = ?2"), params![number, agent], row)
        .optional()
        .map_err(|error| format!("Couldn't read your proposals: {error}"))?;
    let Some(mut proposal) = found else { return Err(unknown()) };
    if proposal.state != "pending" {
        let was = match proposal.state.as_str() {
            "approved" if proposal.replied => format!("{person} already answered it"),
            "approved" => format!("{person} already approved it"),
            "dismissed" => format!("{person} already dismissed it"),
            "withdrawn" => "You withdrew it".to_owned(),
            _ => "It's already closed".to_owned(),
        };
        return Err(format!("Proposal {number} isn't waiting: {was}. Nothing was changed."));
    }
    if proposal.revision > MAX_UPDATES {
        return Err(too_many_updates("proposal", number));
    }
    let kept = crate::proposal_grants::asked_of(&factory.store.db(), number);
    if text == proposal.text && args.grants.as_ref().is_none_or(|grants| grants.is_empty() && kept.is_empty()) {
        return Err(format!("That's what proposal {number} already says. Nothing was changed."));
    }
    // Its grants, resolved again as things are now.
    let asked = args.grants.unwrap_or(kept);
    let rivet_id = rivet(&factory.lock().config).map(|rivet| rivet.id.clone()).unwrap_or_default();
    let resolved =
        crate::proposal_grants::resolve_all(factory, agent, &rivet_id, &asked).map_err(|why| format!("{why} Nothing was changed."))?;
    // Read a pipe or a web page since it proposed: the card says so now.
    let routes = crate::pipes::agents::routes(factory, agent);
    if !routes.is_empty() {
        proposal.taint = Some(crate::pipes::agents::routes_json(&routes).to_string());
    }
    let now = now_ms();
    // Kept only if the person didn't decide it, nor the agent change it, meanwhile.
    let claimed = factory
        .store
        .db()
        .execute(
            "UPDATE proposals SET text = ?3, taint = ?4, revision = revision + 1, updated = ?5, update_reason = ?6
             WHERE id = ?1 AND agent = ?2 AND state = 'pending' AND revision = ?7",
            params![number, agent, text, proposal.taint, now, reason, proposal.revision],
        )
        .map_err(|error| format!("Couldn't change it: {error}"))?;
    if claimed == 0 {
        return Err(format!("{person} just decided proposal {number}, so it wasn't changed; you'll be told what they decided."));
    }
    crate::proposal_grants::keep(&factory.store.db(), number, &asked, &resolved)
        .map_err(|error| format!("Couldn't change its grants: {error}"))?;
    (proposal.text, proposal.revision, proposal.updated, proposal.update_reason) = (text, proposal.revision + 1, Some(now), reason);
    // Its card redraws in place; it still waits, at the same seq.
    {
        let shown = proposal.shown(&factory.store.db());
        let mut inner = factory.lock();
        let entry = inner
            .agents
            .get_mut(agent)
            .and_then(|runtime| runtime.transcript.as_mut())
            .and_then(|transcript| transcript.set_proposal(shown));
        if let Some(entry) = entry {
            factory.send_change(&inner, agent, crate::transcript::Change::Entry(entry));
        }
        factory.broadcast_workspace(&inner);
    }
    let left = MAX_UPDATES - (proposal.revision - 1);
    let mut said = format!(
        "Changed proposal {number}: {person}'s request shows it as it is now, marked updated{}, without notifying them again. Approving the version before is refused, so they look again.",
        if proposal.update_reason.is_empty() { "" } else { " with your reason" }
    );
    said.push_str(&match left {
        0 => " That was the last change this proposal can take.".to_owned(),
        1 => " It can be changed once more.".to_owned(),
        _ => format!(" It can be changed {left} more times."),
    });
    Ok(ToolResult::text(said, json!({})))
}
