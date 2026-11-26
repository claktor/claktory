//! An admin agent's card asking the person to accept a change in who a
//! pipe's other side is (#471 G7; Fugo 34521, Polnareff 34518):
//! `pipes_trust` opens it, and only the person's AcceptPipeIdentity from it
//! (or in the pipe) accepts the change. It holds no email, account or name
//! (the app shows those from the Workspace's Pipe), only the change it was
//! asked for and a salted hash of what the other side presented then, so
//! no one who reads it can learn or confirm who that is. Its agent, which a
//! pipe agent may have steered, hears only that the person accepted it or
//! dismissed it. It's withdrawn as void once the change is accepted in the
//! pipe, clears or becomes another, or the pipe closes.

use std::sync::Arc;

use factory_protocol::v1;
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest, Sha256};

use super::{Record, State};
use crate::card_actions::Action;
use crate::factory::{Factory, RequestError};

pub(crate) const TOOL: &str = "pipes_trust";

/// What the card is bound to.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct Trust {
    pub pipe: String,
    /// The change it was asked for (`v1::PipeIdentityChange`).
    pub change: i32,
    /// 16 random bytes, hex, and the sha256 of them with what the other
    /// side presented then (`super::identity::presented`).
    pub salt: String,
    pub bound: String,
}

impl Trust {
    pub(crate) fn proposal(&self) -> v1::PipeTrustProposal {
        v1::PipeTrustProposal { pipe_id: self.pipe.clone(), identity_change: self.change }
    }

    /// Whether the pipe still shows what it was asked under.
    fn holds(&self, factory: &Factory, record: &Record) -> bool {
        let (change, presented) = super::identity::presented(factory, record);
        change == self.change && change != 0 && bound(&self.salt, &presented) == self.bound
    }
}

fn bound(salt: &str, presented: &str) -> String {
    crate::encoding::hex(&Sha256::digest(format!("{salt}\n{presented}").as_bytes()))
}

/// A card's AskPerson.pipe_trust, from its action; None for any other.
pub(crate) fn proposal_of(action: &str) -> Option<v1::PipeTrustProposal> {
    match serde_json::from_str::<Action>(action) {
        Ok(Action::TrustPipe(trust)) => Some(trust.proposal()),
        _ => None,
    }
}

/// The same, from the tool result that asked it (`details.pipe_trust`).
pub(crate) fn proposal_in(details: &serde_json::Value) -> Option<v1::PipeTrustProposal> {
    let trust = details.get("pipe_trust")?;
    let pipe_id = trust.get("pipe_id")?.as_str()?.to_owned();
    let identity_change = i32::try_from(trust.get("identity_change")?.as_i64()?).ok()?;
    Some(v1::PipeTrustProposal { pipe_id, identity_change })
}

/// The cards about `pipe` still waiting: id, agent and binding.
fn waiting(factory: &Factory, pipe: &str) -> Vec<(i64, String, Trust)> {
    crate::extensions::ask::pending_actions(factory)
        .into_iter()
        .filter_map(|(id, agent, action)| match serde_json::from_str::<Action>(&action) {
            Ok(Action::TrustPipe(trust)) if trust.pipe == pipe => Some((id, agent, trust)),
            _ => None,
        })
        .collect()
}

/// "the pipe "Website"", or "the pipe p1" when the other side named it.
fn called(record: &Record) -> String {
    format!("the pipe {}", super::handoff::label(record).trim_start_matches("pipe "))
}

/// pipes_trust: opens the card for the person.
pub(crate) fn ask(factory: &Arc<Factory>, agent: &str, record: &Record, reason: &str) -> Result<factory_agent::ToolResult, String> {
    let person = factory.lock().config.person().to_owned();
    if record.state == State::Closed {
        return Err("That pipe is closed. Nothing was asked.".to_owned());
    }
    let (change, presented) = super::identity::presented(factory, record);
    if change == 0 {
        return Err(format!(
            "Who's at the other end of that pipe hasn't changed since {person} last accepted it, so there's nothing to accept. Nothing was asked."
        ));
    }
    if let Some((id, whose, _)) = waiting(factory, &record.id).first() {
        return Err(if whose == agent {
            format!("Your card {id} about that pipe is still waiting for {person}. Nothing was asked.")
        } else {
            format!(
                "{} already asked {person} about that pipe on a card that's still waiting. Nothing was asked.",
                factory.agent_name(whose)
            )
        });
    }
    let salt = crate::encoding::hex(&crate::vault::crypto::random::<16>());
    let trust = Trust { pipe: record.id.clone(), change, bound: bound(&salt, &presented), salt };
    let me = factory.agent_name(agent);
    let called = called(record);
    let (title, description) = match v1::PipeIdentityChange::try_from(change) {
        Ok(v1::PipeIdentityChange::AccountChanged) => (
            format!("Trust the Claktory at the other end of {called}"),
            format!(
                "{me} asks you to trust the Claktory at the other end of {called}: it now signs in with a different account. Check with the other person yourself that this change is theirs; an agent can't know it. Until you trust it, what it sends waits for you, and your agents can't send to it."
            ),
        ),
        Ok(v1::PipeIdentityChange::EmailChanged) => (
            format!("Accept the new email on {called}"),
            format!(
                "{me} asks you to accept that the Claktory at the other end of {called} signs in with another email of the same account. Check with the other person yourself that this change is theirs."
            ),
        ),
        _ => (
            format!("Accept that {called} is no longer verified"),
            format!(
                "{me} asks you to accept that claktory.com no longer vouches for the Claktory at the other end of {called}. It shows Not verified until the same account vouches for it again."
            ),
        ),
    };
    // Older apps show only the body.
    let body = format!(
        "{me} asks you to accept a change in who's at the other end of {called}. Update Claktory to accept it here, or accept it in the pipe."
    );
    let action = serde_json::to_string(&Action::TrustPipe(trust.clone())).map_err(|error| format!("Couldn't ask: {error}"))?;
    let id = crate::extensions::ask::ask_to_approve_with(factory, agent, &title, &body, (&description, reason), &action)?;
    let details = json!({
        "ask": id,
        "title": title,
        "body": body,
        "wants_text": false,
        "action_description": description,
        "action_reason": reason,
        "pipe_trust": {"pipe_id": trust.pipe, "identity_change": change},
    });
    Ok(factory_agent::ToolResult::text(
        format!(
            "Asked {person}: their card shows who the other side was and is now, which you never see, and asks them to check the change with the other person themselves. Only their tap accepts it, and you'll be told only whether they accepted it or dismissed it. Its id is {id}; it's withdrawn by itself if the change is accepted in the pipe, clears or becomes another, or the pipe closes."
        ),
        details,
    ))
}

/// AcceptPipeIdentity from a card (`card_id`): the card must still wait,
/// for this pipe and the change it shows now, as it was asked. It's
/// claimed (answered) before the change is accepted: its id, or None
/// without a card.
pub(crate) fn claim(factory: &Factory, record: &Record, card_id: &str) -> Result<Option<i64>, RequestError> {
    let card = card_id.trim();
    if card.is_empty() {
        return Ok(None);
    }
    let void = || {
        RequestError::Pipe(
            "This card no longer waits for you: what the other side shows changed since it was asked. Look at the pipe itself.".to_owned(),
        )
    };
    let (id, _, action) = crate::extensions::ask::pending_action(factory, card).ok_or_else(void)?;
    let Ok(Action::TrustPipe(trust)) = serde_json::from_str::<Action>(&action) else { return Err(void()) };
    if trust.pipe != record.id || !trust.holds(factory, record) {
        return Err(void());
    }
    if !crate::extensions::ask::claim_answered(factory, id) {
        return Err(void());
    }
    Ok(Some(id))
}

/// After accepting from a card: its agent hears only that the person
/// accepted it, or the card waits again when it couldn't be.
pub(crate) fn settled(factory: &Arc<Factory>, id: i64, record: &Record, accepted: bool) {
    if !accepted {
        crate::extensions::ask::unclaim(factory, id);
        return;
    }
    let person = factory.person_capitalized();
    let summary = format!("{person} accepted the change on {}", called(record));
    crate::extensions::ask::told_answered(factory, id, format!("{summary}."), summary);
}

/// The cards about `pipe` that no longer hold (the change was accepted,
/// cleared or became another; the pipe closed or was deleted: `why`) are
/// withdrawn as void.
pub(crate) fn settle(factory: &Factory, pipe: &str, why: &str) {
    let record = factory.pipes.get(pipe).filter(|record| record.state != State::Closed);
    for (id, _, trust) in waiting(factory, pipe) {
        if record.as_ref().is_some_and(|record| trust.holds(factory, record)) {
            continue;
        }
        crate::extensions::ask::withdraw_for(factory, id, &format!("This card is void: {why}."));
    }
}

/// Whether a card's action is a pipe's trust card.
pub(crate) fn is_trust(action: &str) -> bool {
    matches!(serde_json::from_str::<Action>(action), Ok(Action::TrustPipe(_)))
}

/// The person's comment on deciding a card, as its agent is told it: a
/// trust card's (not a reply) goes to the pipe's lead instead, so a pipe
/// agent that steered the asker never reads it word for word.
pub(crate) fn comment_for_asker(factory: &Arc<Factory>, action: &str, comment: &str, replied: bool) -> String {
    let Ok(Action::TrustPipe(trust)) = serde_json::from_str::<Action>(action) else { return comment.to_owned() };
    if replied || comment.trim().is_empty() {
        return comment.to_owned();
    }
    if let Some(record) = factory.pipes.get(&trust.pipe).filter(|record| !record.lead.is_empty()) {
        let person = factory.person_capitalized();
        let summary = format!("{person} decided about who's at the other end of {}", called(&record));
        let text = format!("{summary}.{}", crate::decisions::note(&person, comment));
        factory.deliver_event(&record.lead, text, crate::factory::host_event(summary));
    }
    String::new()
}
