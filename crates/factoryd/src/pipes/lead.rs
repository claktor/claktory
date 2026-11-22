//! A pipe's lead, the agent told of each message from the other side
//! (pipe-edo-findings.md, Bruno's B3): a pipe with agents always has one.
//! When its lead goes (unticked, cleared, deleted, moved out of the pipe's
//! project), its bound liaison leads it, or else its first agent limited to
//! the pipe; the pipe says who answers now, and that agent is told at once
//! of what came after the last it was told of. Never an agent that can run
//! commands unless the person chose it (Fugo): the other side wakes its
//! lead at will. With none to choose, the pipe has no lead: what arrives
//! waits for the person (Pipe.unanswered_seqs), and the pipe says so once.

use std::sync::Arc;

use super::{Record, State, agents, change, line};
use crate::config::Config;
use crate::factory::Factory;

/// The pipe's agents now: its members that are still agents of its project.
fn present<'a>(config: &'a Config, record: &'a Record) -> impl Iterator<Item = &'a String> {
    record.members.iter().filter(|id| config.agent(id).is_some_and(|agent| agent.project.as_deref() == Some(record.project.as_str())))
}

/// Its lead, if it's one of its agents now.
pub(crate) fn effective(config: &Config, record: &Record) -> Option<String> {
    present(config, record).find(|id| **id == record.lead).cloned()
}

/// Who leads it: its lead, else its bound liaison, else its first agent
/// limited to the pipe; None when it has neither (the person chooses).
fn chosen(config: &Config, record: &Record) -> Option<String> {
    let limited = |id: &&String| record.quarantined.contains(id);
    effective(config, record)
        .or_else(|| present(config, record).filter(limited).find(|id| **id == record.bound).cloned())
        .or_else(|| present(config, record).find(limited).cloned())
}

/// After `pipe` changed: gives it a lead if it has an agent limited to it
/// and none leads it. When who leads it differs from `before` (its lead before the change,
/// "" for none), the pipe says so, and a new lead is told at once of what
/// arrived since the last message a lead was told of.
pub(crate) fn settle(factory: &Arc<Factory>, pipe: &str, before: &str) {
    let Some(record) = factory.pipes.get(pipe) else { return };
    if !record.granted() {
        return;
    }
    let (now, name) = {
        let inner = factory.lock();
        let now = chosen(&inner.config, &record).unwrap_or_default();
        let name = inner.config.agent(&now).map(|agent| agent.name.clone()).unwrap_or_default();
        (now, name)
    };
    if record.lead != now || (record.no_lead_said && !now.is_empty()) {
        change(factory, pipe, |record| {
            record.lead.clone_from(&now);
            if !now.is_empty() {
                record.no_lead_said = false;
            }
        });
    }
    if now == before {
        return;
    }
    let who = record.peer_who();
    if now.is_empty() {
        change(factory, pipe, |record| record.no_lead_said = true);
        line(factory, pipe, no_lead_line(&who));
    } else {
        line(factory, pipe, format!("{name} answers this pipe now: it's told of each message from {who}."));
        agents::catch_up(factory, pipe);
    }
    factory.broadcast_workspace(&factory.lock());
}

/// What the pipe says when no agent of this side is told of what arrives.
pub(crate) fn no_lead_line(who: &str) -> String {
    format!(
        "No agent answers this pipe now: messages from {who} wait for you here. Choose its lead in the pipe's settings."
    )
}

/// Every pipe, at start and after agents were deleted or moved: one whose
/// lead isn't one of its agents now gets one if it can (`settle`).
pub(crate) fn settle_all(factory: &Arc<Factory>) {
    let config = factory.lock().config.clone();
    for record in factory.pipes.all() {
        if record.granted() && effective(&config, &record).is_none() {
            settle(factory, &record.id, "");
        }
    }
}

/// The messages from the other side no agent was told of, while `record`
/// has no lead: the person's to answer (Pipe.unanswered_seqs).
pub(crate) fn unanswered(factory: &Factory, config: &Config, record: &Record) -> Vec<u64> {
    if !record.granted() || record.state == State::Closed || effective(config, record).is_some() {
        return Vec::new();
    }
    agents::untold(factory, record).into_iter().map(|message| message.seq).collect()
}
