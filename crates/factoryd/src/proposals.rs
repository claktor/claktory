//! Proposals to Rivet. No agent can message Rivet, the admin agent: only the
//! person can. An agent that reaches a conclusion Rivet should act on
//! (another agent is needed, say) proposes a message with propose_to_rivet
//! instead. The person sees it in that agent's conversation and approves it,
//! with a comment, or dismisses it. Approved, Rivet gets it as the person's
//! own message; either way, the agent is told.
//!
//! Proposals live in factory.db. The conversation shows each as an entry
//! after the propose_to_rivet call that made it (its result names it), so it
//! reloads where it was; its state comes from here.

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

/// The tool, one of the Messaging extension's, for agents that aren't admins.
pub(crate) const TOOL: &str = "propose_to_rivet";

const ABOUT: &str = "Propose a message for Rivet, the admin agent, which only the person can message: for a conclusion Rivet should act on, such as adding an agent. The person approves it, with a comment, or dismisses it, and you're told which; approved, Rivet gets it from the person. Write it to stand alone, with all the context Rivet needs: Rivet can't see your conversation.";

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
}

pub(crate) fn tool(factory: &Arc<Factory>, agent: &str) -> Tool {
    let parameters = json!({
        "type": "object",
        "required": ["message"],
        "properties": {"message": {"type": "string", "minLength": 1}},
        "additionalProperties": false,
    });
    crate::extensions::tool(
        TOOL,
        "Propose to Rivet",
        ABOUT,
        parameters,
        crate::extensions::typed_handler(factory, agent, |factory, agent, args: ProposeArgs| propose(factory, agent, args.message)),
    )
}

/// Records the agent's proposal, or says why not. The result names it, so
/// its conversation shows it after the call.
fn propose(factory: &Factory, agent: &str, text: String) -> Result<ToolResult, String> {
    let text = text.trim().to_owned();
    if text.is_empty() {
        return Err("The message is empty; nothing was proposed.".to_owned());
    }
    let person = {
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
        inner.config.person().to_owned()
    };
    let id = {
        let db = factory.store.db();
        db.execute(
            "INSERT INTO proposals (agent, text, created, state, comment) VALUES (?1, ?2, ?3, 'pending', '')",
            params![agent, text, now_ms()],
        )
        .map_err(|error| format!("Couldn't record the proposal: {error}"))?;
        db.last_insert_rowid().to_string()
    };
    let said = format!("Proposed; {person} will approve or dismiss it.");
    Ok(ToolResult::text(said, json!({"proposal": id, "text": text})))
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
    Some(v1::Proposal { id, text, state: ProposalState::Pending.into(), comment: String::new(), decided_at_ms: 0 })
}

/// A proposal as recorded.
struct Row {
    id: i64,
    text: String,
    state: String,
    comment: String,
    decided: Option<i64>,
}

impl Row {
    fn to_proto(&self) -> v1::Proposal {
        let state = match self.state.as_str() {
            "approved" => ProposalState::Approved,
            "dismissed" => ProposalState::Dismissed,
            _ => ProposalState::Pending,
        };
        v1::Proposal {
            id: self.id.to_string(),
            text: self.text.clone(),
            state: state.into(),
            comment: self.comment.clone(),
            decided_at_ms: self.decided.unwrap_or_default(),
        }
    }
}

/// Shows the agent's decided proposals as decided in its just-loaded conversation.
pub(crate) fn restore(store: &Store, agent: &str, transcript: &mut Transcript) -> rusqlite::Result<()> {
    let db = store.db();
    let mut statement =
        db.prepare("SELECT id, text, state, comment, decided FROM proposals WHERE agent = ?1 AND state != 'pending' ORDER BY id")?;
    let rows = statement.query_map([agent], |row| {
        Ok(Row { id: row.get(0)?, text: row.get(1)?, state: row.get(2)?, comment: row.get(3)?, decided: row.get(4)? })
    })?;
    for row in rows {
        transcript.set_proposal(row?.to_proto());
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

/// The person approves (with `comment`) or dismisses `agent`'s proposal.
/// Approved, Rivet gets it as the person's message; it stays pending when
/// there's no Rivet, or Rivet has no model. Either way the agent is told.
pub(crate) fn decide(factory: &Arc<Factory>, agent: &str, id: &str, approve: bool, comment: &str) -> Result<(), RequestError> {
    let comment = comment.trim();
    let unknown = || RequestError::UnknownProposal(id.to_owned());
    let number: i64 = id.trim().parse().map_err(|_| unknown())?;
    let found = factory
        .store
        .db()
        .query_row("SELECT id, text, state, comment, decided FROM proposals WHERE id = ?1 AND agent = ?2", params![number, agent], |row| {
            Ok(Row { id: row.get(0)?, text: row.get(1)?, state: row.get(2)?, comment: row.get(3)?, decided: row.get(4)? })
        })
        .optional()?;
    let Some(mut row) = found else { return Err(unknown()) };
    if row.state != "pending" {
        return Err(RequestError::Proposal(format!("This proposal was already {}.", row.state)));
    }
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
                let mut message = String::new();
                if !comment.is_empty() {
                    message.push_str(comment);
                    message.push_str("\n\n");
                }
                let signed = match from {
                    Some(from) => agents::signed(config, from, rivet),
                    None => crate::chats::signed(factory, agent),
                };
                let _ = write!(message, "{signed} proposes:\n\n{}", row.text);
                Some((rivet.id.clone(), message))
            }
        };
        let rivet_name = rivet.map_or_else(|| "Rivet".to_owned(), |rivet| rivet.name.clone());
        (rivet_name, config.person_capitalized().to_owned(), delivery)
    };
    let state = if approve { "approved" } else { "dismissed" };
    let decided = now_ms();
    let claimed = factory.store.db().execute(
        "UPDATE proposals SET state = ?2, comment = ?3, decided = ?4 WHERE id = ?1 AND state = 'pending'",
        params![number, state, comment, decided],
    )?;
    if claimed == 0 {
        return Err(RequestError::Proposal("This proposal was already decided.".to_owned()));
    }
    if let Some((rivet, message)) = &delivery
        && let Err(error) = factory.send_message(rivet, message, None, "", &[])
    {
        factory.store.db().execute("UPDATE proposals SET state = 'pending', comment = '', decided = NULL WHERE id = ?1", [number])?;
        return Err(error);
    }
    (row.state, row.comment, row.decided) = (state.to_owned(), comment.to_owned(), Some(decided));
    // Its entry shows the decision.
    {
        let mut inner = factory.lock();
        let entry = inner
            .agents
            .get_mut(agent)
            .and_then(|runtime| runtime.transcript.as_mut())
            .and_then(|transcript| transcript.set_proposal(row.to_proto()));
        if let Some(entry) = entry {
            factory.send_change(&inner, agent, crate::transcript::Change::Entry(entry));
            // It no longer waits for the person (Agent.waiting_seqs).
            factory.broadcast_workspace(&inner);
        }
    }
    let mut summary = format!("{person} {state} your proposal to {rivet_name}");
    if !comment.is_empty() {
        let _ = write!(summary, " (comment: {comment})");
    }
    summary.push('.');
    let text = format!("{summary}\n\nYour proposal:\n{}", row.text);
    let event = host_event(summary);
    factory.deliver_event(agent, text, event);
    Ok(())
}
