//! #414's boundary (Fugo 31931, 32063): a pipe's liaison, made by Claktory
//! for that pipe alone, uses no other pipe in any role (member, sending
//! only, lead or liaison), and an agent on another pipe is never a pipe's
//! liaison. Whoever asks (the person in the app, an agent's card, a
//! proposal's grant), it's refused with why; the app shows who's held
//! (Pipe.held_agents). Agents in two ordinary pipes stay allowed, and so
//! does an agent limited to several (quarantine is the person's
//! containment, built to span pipes); closed pipes don't count. A Claktory
//! from before has a liaison on another pipe taken off it at start, saying
//! so there.

use std::sync::Arc;

use factory_protocol::v1;

use super::{Record, State, change, lead, line};
use crate::config::Config;
use crate::factory::{Factory, RequestError};

/// Whether `agent` is `record`'s liaison (not closed).
fn liaison_of(record: &Record, agent: &str) -> bool {
    on(record, agent) && record.bound == agent
}

/// Whether `agent` is on `record` (not closed): one of its agents, which
/// every lead and liaison is.
fn on(record: &Record, agent: &str) -> bool {
    record.state != State::Closed && !agent.is_empty() && record.members.iter().any(|id| id == agent)
}

/// The pipe as the person knows it, and as agents may read it: its name
/// only when this side chose it.
fn named(record: &Record) -> String {
    if record.named_here.unwrap_or(record.peer.is_none()) && !record.name.trim().is_empty() {
        record.name.trim().to_owned()
    } else {
        format!("pipe {}", record.id)
    }
}

fn name(config: &Config, agent: &str) -> String {
    config.agent(agent).map_or_else(|| agent.to_owned(), |agent| agent.name.clone())
}

/// Why `agent`, `other`'s liaison, can't use another pipe.
fn held_reason(config: &Config, agent: &str, other: &Record) -> String {
    let pipe = named(other);
    let who = name(config, agent);
    let owner = if pipe.ends_with('s') { format!("{pipe}'") } else { format!("{pipe}'s") };
    format!("{who} is {owner} liaison: it reads that pipe, so it can't use another.")
}

/// Why `agent`, on `other`, can't be this pipe's liaison.
fn on_reason(config: &Config, agent: &str, other: &Record) -> String {
    let pipe = named(other);
    let who = name(config, agent);
    format!("{who} is on {pipe}: a pipe's liaison can't use another pipe, so take {who} off {pipe} first.")
}

/// Why `agent` can't have a role in `pipe`: it's another pipe's liaison;
/// None when it may.
fn held(config: &Config, records: &[Record], pipe: &str, agent: &str) -> Option<String> {
    records.iter().filter(|other| other.id != pipe).find(|other| liaison_of(other, agent)).map(|other| held_reason(config, agent, other))
}

/// Why `agent` can't be `pipe`'s liaison: it's on another pipe; None when
/// it may.
fn elsewhere(config: &Config, records: &[Record], pipe: &str, agent: &str) -> Option<String> {
    records.iter().filter(|other| other.id != pipe).find(|other| on(other, agent)).map(|other| on_reason(config, agent, other))
}

/// A pipe's roles after a change, as checked.
pub(crate) struct Roles<'a> {
    pub members: &'a [String],
    pub lead: &'a str,
    /// Its liaison, when it's one of `members`.
    pub bound: &'a str,
}

/// Refused, saying why, when `pipe` with `roles` would have another pipe's
/// liaison, or a liaison that's on another pipe.
pub(crate) fn check(factory: &Factory, pipe: &str, roles: &Roles<'_>) -> Result<(), RequestError> {
    let records = factory.pipes.all();
    let config = factory.lock().config.clone();
    let everyone = roles.members.iter().map(String::as_str).chain([roles.lead, roles.bound]);
    for agent in everyone.filter(|agent| !agent.is_empty()) {
        if let Some(why) = held(&config, &records, pipe, agent) {
            return Err(RequestError::Pipe(why));
        }
    }
    if !roles.bound.is_empty()
        && let Some(why) = elsewhere(&config, &records, pipe, roles.bound)
    {
        return Err(RequestError::Pipe(why));
    }
    Ok(())
}

/// Pipe.held_agents for `record`: every other pipe's liaison. Empty for a
/// closed pipe. (Pipe.cant_limit stays empty: a liaison is only ever a new
/// agent, and limiting an agent on several pipes is the person's to do.)
pub(crate) fn view(config: &Config, records: &[Record], record: &Record) -> Vec<v1::PipeAgentHeld> {
    if record.state == State::Closed {
        return Vec::new();
    }
    config
        .agents
        .iter()
        .filter_map(|agent| {
            held(config, records, &record.id, &agent.id).map(|reason| v1::PipeAgentHeld { agent_id: agent.id.clone(), reason })
        })
        .collect()
}

/// At start: a pipe's liaison on other pipes, from before the boundary, is
/// taken off every other, each saying why.
pub(crate) fn upgrade(factory: &Arc<Factory>) {
    let mut records = factory.pipes.all();
    records.sort_by(|a, b| a.created.cmp(&b.created).then_with(|| a.id.cmp(&b.id)));
    let config = factory.lock().config.clone();
    for agent in &config.agents {
        let Some(kept) = records.iter().find(|record| liaison_of(record, &agent.id)) else { continue };
        for other in records.iter().filter(|other| other.id != kept.id && on(other, &agent.id)) {
            let lead_before = lead::effective(&config, other).unwrap_or_default();
            change(factory, &other.id, |record| {
                record.members.retain(|id| *id != agent.id);
                record.send_only.retain(|id| *id != agent.id);
                record.quarantined.retain(|id| *id != agent.id);
                if record.lead == agent.id {
                    record.lead.clear();
                }
                if record.bound == agent.id {
                    record.bound.clear();
                }
            });
            let why = held_reason(&config, &agent.id, kept);
            tracing::info!(agent = %agent.id, pipe = %other.id, kept = %kept.id, "took an agent off a pipe: it reads another");
            line(factory, &other.id, format!("{} left this pipe. {why}", agent.name));
            lead::settle(factory, &other.id, &lead_before);
        }
    }
}
