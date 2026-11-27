//! An admin agent's card to approve a new pipe (#471 G1; Fugo 33994 and
//! 34010, Polnareff 34001 and 34261): `pipes_approve` opens it, filled in
//! with the agents, lead and reviews the person asked for, checked as
//! ApprovePipe checks them. The person compares the pipe's digits with the
//! other person themselves and approves it with their own choices
//! (ApprovePipe with its card_id): no tool approves a pipe. The card never
//! holds the digits, nor anything the other side said (the app shows those
//! from the Workspace's Pipe), and its agent hears only that the person
//! approved it or dismissed it. It's withdrawn as void once the pipe stops
//! waiting for approval, or its other side's key changes.

use std::sync::Arc;

use factory_protocol::v1;
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest, Sha256};

use super::{Record, State};
use crate::card_actions::Action;
use crate::factory::{Factory, RequestError};

pub(crate) const TOOL: &str = "pipes_approve";

/// The pipe's own settings when the card was asked (Polnareff 34261), for
/// the app to tell what narrowed since.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct Baseline {
    pub members: Vec<String>,
    pub quarantined: Vec<String>,
    pub send_only: Vec<String>,
    pub lead: String,
    pub review_incoming: bool,
    pub review_outgoing: bool,
}

impl Baseline {
    fn of(record: &Record) -> Baseline {
        Baseline {
            members: record.members.clone(),
            quarantined: record.quarantined.clone(),
            send_only: record.send_only.clone(),
            lead: record.lead.clone(),
            review_incoming: record.review_incoming,
            review_outgoing: record.review,
        }
    }
}

/// What the card proposes, and what it's bound to.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct Approval {
    pub pipe: String,
    /// The other side's pinned key, hashed (`key_of`).
    pub key: String,
    pub members: Vec<String>,
    /// Among `members`, those that can run commands on this computer.
    pub commands: Vec<String>,
    /// The member that answers it; '' with a liaison.
    pub lead: String,
    /// A new liaison agent of this name, which answers it; '' for none.
    pub liaison: String,
    pub review_incoming: bool,
    pub review_outgoing: bool,
    /// Among `members`, those that only send to it.
    pub send_only: Vec<String>,
    pub asked_from: Baseline,
}

impl Approval {
    pub(crate) fn proposal(&self) -> v1::PipeApprovalProposal {
        let was = &self.asked_from;
        v1::PipeApprovalProposal {
            pipe_id: self.pipe.clone(),
            member_ids: self.members.clone(),
            can_run_commands_ids: self.commands.clone(),
            lead_id: self.lead.clone(),
            liaison_name: self.liaison.clone(),
            review_incoming: self.review_incoming,
            review_outgoing: self.review_outgoing,
            send_only_ids: self.send_only.clone(),
            asked_from: Some(v1::PipeApprovalBaseline {
                member_ids: was.members.clone(),
                quarantined_ids: was.quarantined.clone(),
                send_only_ids: was.send_only.clone(),
                lead_id: was.lead.clone(),
                review_incoming: was.review_incoming,
                review_outgoing: was.review_outgoing,
            }),
        }
    }

    /// The ApprovePipe its choices make, checked as the person's own are.
    fn request(&self) -> v1::ApprovePipe {
        let quarantined = self.members.iter().filter(|id| !self.commands.contains(id)).cloned().collect();
        v1::ApprovePipe {
            pipe_id: self.pipe.clone(),
            approve: true,
            member_ids: self.members.clone(),
            lead_id: self.lead.clone(),
            liaison_name: self.liaison.clone(),
            quarantined: Some(v1::PipeMembers { agent_ids: quarantined }),
            review_incoming: Some(self.review_incoming),
            review_outgoing: Some(self.review_outgoing),
            send_only: Some(v1::PipeMembers { agent_ids: self.send_only.clone() }),
            ..Default::default()
        }
    }
}

/// The other side's pinned key, hashed (the card's action is in a table
/// agents with Claktory's records read; the key is public anyway, Fugo
/// 34521); '' when none is pinned.
pub(crate) fn key_of(record: &Record) -> String {
    let key = record.peer.as_ref().map(|peer| peer.endpoint_id.as_str()).unwrap_or_default();
    if key.is_empty() { String::new() } else { crate::encoding::hex(&Sha256::digest(key.as_bytes())) }
}

/// A card's AskPerson.pipe_approval, from its action; None for any other.
pub(crate) fn proposal_of(action: &str) -> Option<v1::PipeApprovalProposal> {
    match serde_json::from_str::<Action>(action) {
        Ok(Action::ApprovePipe(approval)) => Some(approval.proposal()),
        _ => None,
    }
}

/// The same, from the tool result that asked it (`details.pipe_approval`).
pub(crate) fn proposal_in(details: &serde_json::Value) -> Option<v1::PipeApprovalProposal> {
    serde_json::from_value::<Approval>(details.get("pipe_approval")?.clone()).ok().map(|approval| approval.proposal())
}

/// The cards to approve `pipe` still waiting: id, agent and proposal.
fn waiting(factory: &Factory, pipe: &str) -> Vec<(i64, String, Approval)> {
    crate::extensions::ask::pending_actions(factory)
        .into_iter()
        .filter_map(|(id, agent, action)| match serde_json::from_str::<Action>(&action) {
            Ok(Action::ApprovePipe(approval)) if approval.pipe == pipe => Some((id, agent, *approval)),
            _ => None,
        })
        .collect()
}

/// "the pipe "Website"", or "the pipe p1" when the other side named it.
fn called(record: &Record) -> String {
    format!("the pipe {}", super::handoff::label(record).trim_start_matches("pipe "))
}

fn said(error: RequestError) -> String {
    match error {
        RequestError::Pipe(text) => text,
        other => other.to_string(),
    }
}

/// pipes_approve: opens the card for the person, or changes the one
/// `agent` has waiting for this pipe (a new revision). `approval` holds
/// the choices; the rest is filled in here, from the pipe.
pub(crate) fn ask(
    factory: &Arc<Factory>,
    agent: &str,
    record: &Record,
    mut approval: Approval,
    reason: &str,
) -> Result<factory_agent::ToolResult, String> {
    let person = factory.lock().config.person().to_owned();
    if record.state != State::Approval {
        return Err(format!("That pipe isn't waiting for {person}'s approval, so there's nothing to approve. Nothing was asked."));
    }
    let key = key_of(record);
    if key.is_empty() {
        return Err(
            "That pipe's other side hasn't connected yet: ask once it waits for approval with its digits. Nothing was asked.".to_owned()
        );
    }
    // The agent's own words in Claktory's: one short, plain name (Fugo 34864).
    if !approval.liaison.is_empty() && !plain_name(&approval.liaison) {
        return Err(format!(
            "A liaison's name is one line of at most {NAME_MAX} characters, without quotes or control characters. Nothing was asked."
        ));
    }
    (approval.pipe, approval.key, approval.asked_from) = (record.id.clone(), key, Baseline::of(record));
    super::choices_checked(factory, record, &approval.request()).map_err(|error| format!("{} Nothing was asked.", said(error)))?;
    let me = factory.agent_name(agent);
    let called = called(record);
    let title = format!("Approve {called}");
    // Older apps show only the body (Polnareff 34001).
    let body = format!("{me} asks you to approve {called}. Update Claktory to approve it here, or approve it in the pipe.");
    let description = describe(factory, &me, &called, &approval);
    let action =
        serde_json::to_string(&Action::ApprovePipe(Box::new(approval.clone()))).map_err(|error| format!("Couldn't ask: {error}"))?;
    let cards = waiting(factory, &record.id);
    match cards.first() {
        Some((_, whose, _)) if whose != agent => Err(format!(
            "{} already asked {person} to approve this pipe in a request that's still waiting. Nothing was asked.",
            factory.agent_name(whose)
        )),
        Some((id, _, was)) if *was == approval => Err(format!("Your request {id} already proposes that. Nothing was changed.")),
        Some((id, _, _)) => {
            crate::extensions::ask::change_action(factory, agent, *id, (&title, &body), (&description, reason), &action)?;
            Ok(factory_agent::ToolResult::text(
                format!(
                    "Changed your request {id} to approve {called}: {person}'s request shows what you propose now, marked updated, without notifying them again."
                ),
                json!({}),
            ))
        }
        None => {
            let id = crate::extensions::ask::ask_to_approve_with(factory, agent, &title, &body, (&description, reason), &action)?;
            let details = json!({
                "ask": id,
                "title": title,
                "body": body,
                "wants_text": false,
                "action_description": description,
                "action_reason": reason,
                "pipe_approval": approval,
            });
            Ok(factory_agent::ToolResult::text(
                format!(
                    "Asked {person}: their request shows what you propose, each choice theirs to change, and the pipe's digits for them to compare with the other person themselves. Only their tap approves it, and you'll be told only whether they approved it or dismissed it. Its id is {id}; calling pipes_approve again while it waits changes it, and it's withdrawn by itself if the pipe is approved or closed some other way first, or its other side changes."
                ),
                details,
            ))
        }
    }
}

/// The longest liaison name a card proposes.
const NAME_MAX: usize = 40;

/// A name fit to quote in Claktory's words: one line, short, with no
/// quotes, control characters or text-direction marks.
fn plain_name(name: &str) -> bool {
    let bidi = |c: char| matches!(c, '\u{061C}' | '\u{200E}' | '\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}');
    name.chars().count() <= NAME_MAX
        && name.trim() == name
        && !name.chars().any(|c| c.is_control() || bidi(c) || matches!(c, '"' | '\u{2028}' | '\u{2029}'))
}

/// The card's words for what it proposes, Claktory's own.
fn describe(factory: &Factory, me: &str, called: &str, approval: &Approval) -> String {
    let name = |id: &String| factory.agent_name(id);
    let names = |ids: &[String]| crate::card_actions::listed(&ids.iter().map(name).collect::<Vec<_>>());
    let readers: Vec<String> = approval.members.iter().filter(|id| !approval.send_only.contains(id)).cloned().collect();
    let mut parts = Vec::new();
    if !readers.is_empty() {
        parts.push(format!("{} reading it", names(&readers)));
    }
    parts.push(if approval.liaison.is_empty() {
        format!("{} answering it", factory.agent_name(&approval.lead))
    } else {
        format!("a new agent, \"{}\", answering it", super::agents::escape(&approval.liaison))
    });
    if !approval.send_only.is_empty() {
        parts.push(format!("{} only sending to it", names(&approval.send_only)));
    }
    let commands = if approval.commands.is_empty() {
        "None of its agents can run commands on this computer".to_owned()
    } else {
        format!("{} can run commands on this computer", names(&approval.commands))
    };
    let on = |on: bool| if on { "on" } else { "off" };
    format!(
        "{me} asks you to approve {called}, with {}. {commands}. Review incoming is {}, Review outgoing {}. Compare its digits with the other person yourself, by voice or in person, before you approve.",
        crate::card_actions::listed(&parts),
        on(approval.review_incoming),
        on(approval.review_outgoing),
    )
}

/// ApprovePipe from a card (`card_id`): the card must still wait, for this
/// pipe and the key it was asked under. It's claimed (answered) before the
/// pipe is approved, so the withdrawal approving brings passes it by: its
/// id and agent, or None without a card.
pub(crate) fn claim(factory: &Factory, record: &Record, request: &v1::ApprovePipe) -> Result<Option<(i64, String)>, RequestError> {
    let card = request.card_id.trim();
    if card.is_empty() {
        return Ok(None);
    }
    let void = || RequestError::Pipe("This request no longer waits for you: approve the pipe in its own view.".to_owned());
    let (id, agent, action) = crate::extensions::ask::pending_action(factory, card).ok_or_else(void)?;
    let Ok(Action::ApprovePipe(approval)) = serde_json::from_str::<Action>(&action) else { return Err(void()) };
    if approval.pipe != record.id || approval.key != key_of(record) || record.state != State::Approval {
        return Err(void());
    }
    if !crate::extensions::ask::claim_answered(factory, id) {
        return Err(void());
    }
    Ok(Some((id, agent)))
}

/// After approving from a card: its agent hears only that the person
/// approved it (never their comment or choices), or the card waits again
/// when the pipe couldn't be approved.
pub(crate) fn settled(factory: &Arc<Factory>, id: i64, record: &Record, approved: bool) {
    if !approved {
        crate::extensions::ask::unclaim(factory, id);
        return;
    }
    let person = factory.person_capitalized();
    let summary = format!("{person} approved {}", called(record));
    crate::extensions::ask::told_answered(factory, id, format!("{summary}."), summary);
}

/// A pipe changed (`super::change`, or deleted: `after` None): while it
/// waited for approval, its cards are void once it doesn't, or its other
/// side's key changed.
pub(crate) fn changed(factory: &Factory, before: &Record, after: Option<&Record>) {
    if before.state != State::Approval {
        return;
    }
    let why = match after {
        None => "the pipe was deleted",
        Some(after) if after.state == State::Closed => "the pipe was closed",
        Some(after) if after.state != State::Approval => "the pipe was approved in its own view",
        Some(after) if key_of(after) != key_of(before) => "its other side's Claktory changed",
        Some(_) => return,
    };
    for (id, _, _) in waiting(factory, &before.id) {
        crate::extensions::ask::withdraw_for(factory, id, &format!("This request is void: {why}."));
    }
}
