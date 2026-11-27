//! pipe_leave: an agent gives up a pipe of its own, or only reading
//! it (it only sends then), or only leading it, at once and with no
//! card; the person's History keeps it (`crate::given_up`). Only the person
//! gives any of it back, in the pipe's settings.
//!
//! What it holds is checked again as it changes, under the pipes' lock
//! that the person's changes take too (`change`), so a call that comes
//! after the person took it off changes nothing. Its liaison and an agent
//! limited to the pipe are the person's containment, so they can't leave
//! . An agent that gives up leading is never made the lead
//! again but by the person (`Record::declined_lead`); whoever leads after
//! is chosen as always (`lead::settle`): never one that can run commands.
//! The last agent leaving leaves the pipe as it is: nothing is closed or
//! paused, and nothing goes to the other side.

use std::sync::Arc;

use factory_agent::{Tool, ToolResult};
use serde::Deserialize;
use serde_json::json;

use super::{agents, change, lead, line};
use crate::factory::Factory;
use crate::given_up::{self, What};

pub(crate) const TOOL: &str = "pipe_leave";
const DESCRIPTION: &str = "Give up a pipe of yours, on your own, when you no longer need it or decide you shouldn't have it: it's taken from you at once, without asking the person, and they see it in History with your reason. pipe: its name or id, left out when you have one; reason: why, for them (at most 500 characters); only: \"reading\" to stop reading it and only send to it, or \"leading\" to stop being told of each message from the other side, staying one of its agents. Getting it back is their choice, in the pipe's settings.";

#[derive(Deserialize)]
struct Args {
    #[serde(default)]
    pipe: Option<String>,
    #[serde(default)]
    reason: String,
    #[serde(default)]
    only: Option<String>,
}

/// pipe_leave, for an agent with any pipe.
pub(crate) fn tool(factory: &Arc<Factory>, agent: &str) -> Tool {
    crate::extensions::tool(
        TOOL,
        "Give up a pipe",
        DESCRIPTION,
        json!({
            "type": "object",
            "required": ["reason"],
            "properties": {
                "pipe": {"type": "string", "description": "The pipe's name or id."},
                "reason": {"type": "string"},
                "only": {"type": "string", "enum": ["reading", "leading"]},
            },
            "additionalProperties": false,
        }),
        crate::extensions::typed_handler(factory, agent, leave),
    )
}

/// The pipe as History names it: ""Launch"" when this side
/// named it; else who it's with, "with Fred"; else "" ("a pipe"). Never
/// its id, nor a name the other side chose.
fn history_label(record: &super::Record) -> String {
    let label = super::handoff::label(record);
    if label.starts_with('"') {
        return label;
    }
    match record.peer.as_ref().map(|peer| peer.owner_name.trim()).filter(|owner| !owner.is_empty()) {
        Some(owner) => format!("with {owner}"),
        None => String::new(),
    }
}

fn leave(factory: &Arc<Factory>, agent: &str, args: Args) -> Result<ToolResult, String> {
    let reason = given_up::reason(&args.reason)?;
    let what = match args.only.as_deref().map(str::trim) {
        None | Some("") => What::Pipe,
        Some("reading") => What::PipeReading,
        Some("leading") => What::PipeLeading,
        Some(other) => return Err(format!("only is \"reading\" or \"leading\", not \"{other}\". Nothing was changed.")),
    };
    let record = agents::resolve(factory, agent, args.pipe.as_deref()).map_err(|error| format!("{error} Nothing was changed."))?;
    given_up::refuse_contained(factory, agent)?;
    let person = factory.person_capitalized();
    let pipe_called = agents::called(&record, agent);
    let lead_before = lead::effective(&factory.lock().config, &record).unwrap_or_default();
    let changed = change(factory, &record.id, |record| {
        if !record.granted() || !record.members.iter().any(|id| id == agent) {
            return Err(format!("You're no longer one of the agents of {pipe_called}."));
        }
        if record.bound == agent || record.quarantined.iter().any(|id| id == agent) {
            return Err(format!(
                "{person} limited you to {pipe_called}, so only they take you off it: ask {person}, in the pipe or your conversation."
            ));
        }
        let leads = record.lead == agent;
        match what {
            What::PipeReading if record.sends_only(agent) => Err(format!("You already only send to {pipe_called}.")),
            What::PipeReading if leads => {
                Err(format!("You lead {pipe_called}: give up leading it first (only: \"leading\"), then reading it."))
            }
            What::PipeReading => {
                record.send_only.push(agent.to_owned());
                Ok(())
            }
            What::PipeLeading if !leads => Err(format!("You don't lead {pipe_called}.")),
            What::PipeLeading => {
                record.lead.clear();
                if !record.declined_lead.iter().any(|id| id == agent) {
                    record.declined_lead.push(agent.to_owned());
                }
                Ok(())
            }
            _ => {
                record.members.retain(|id| id != agent);
                record.send_only.retain(|id| id != agent);
                record.declined_lead.retain(|id| id != agent);
                if leads {
                    record.lead.clear();
                }
                Ok(())
            }
        }
    });
    match changed {
        None => return Err("That pipe is gone. Nothing was changed.".to_owned()),
        Some(Err(error)) => return Err(format!("{error} Nothing was changed.")),
        Some(Ok(())) => {}
    }
    let name = factory.agent_name(agent);
    line(
        factory,
        &record.id,
        match what {
            What::PipeReading => format!("{name} stopped reading this pipe: it only sends to it now."),
            What::PipeLeading => format!("{name} stopped leading this pipe."),
            _ => format!("{name} left this pipe."),
        },
    );
    // Someone else leads it now, if anyone can.
    lead::settle(factory, &record.id, &lead_before);
    factory.broadcast_workspace(&factory.lock());
    let kept = given_up::record(factory, agent, what, &record.id, &history_label(&record), &reason);
    let back = format!("{person} sees this in History, with your reason; getting it back is their choice, in the pipe's settings.");
    let said = match what {
        What::PipeReading => format!(
            "You now only send to {pipe_called}: you can't read it, and aren't told what the other side says, from your next step on. {back}"
        ),
        What::PipeLeading => format!(
            "You no longer lead {pipe_called}: you aren't told of each message from the other side, and you're never made its lead again unless {person} chooses you. You're still one of its agents. {back}"
        ),
        _ => format!("Left {pipe_called}: you can't send to it or read it, from your next step on. {back}"),
    };
    Ok(ToolResult::text(said, json!({"pipe": record.id, given_up::DETAIL: kept.map(|id| id.to_string())})))
}
