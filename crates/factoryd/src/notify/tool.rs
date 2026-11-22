//! `notify_person`: an agent tells the person something that can't wait for
//! them to look (an ops watchdog's "account.claktory.com isn't answering"),
//! as a notification on their devices. Part of the Ask extension, so a
//! liaison quarantined for a pipe keeps it, under the rule that an agent
//! that has read a pipe puts none of its words in a notification.

use std::sync::Arc;

use factory_agent::{Tool, ToolResult};
use factory_protocol::v1::NotifyKind as Kind;
use serde::Deserialize;
use serde_json::json;

use super::{Delivered, Mode, Note, PER_HOUR, Skipped};
use crate::factory::Factory;

pub(crate) const TOOL: &str = "notify_person";
/// The longest title, in characters.
const MAX_TITLE: usize = 120;
/// The longest text, in characters (a notification shows less).
const MAX_TEXT: usize = 2000;

const ABOUT: &str = "Notify the person on their devices (their Mac, their phone's lock screen) of something that can't wait until they next look at Claktory: a service down, a deadline about to pass, work they're waiting on that's done or stuck. Not for routine progress, and never instead of ask_person when you need an answer: an alert is one-way. title: what happened, in a short line (\"account.claktory.com isn't answering\"). text: a sentence or two more, standing alone. urgent: true only when it can't wait for the morning; their settings decide whether it breaks quiet hours. You notify at most 6 times an hour; past that, they're told only how many more you sent. Says whether it reached anyone.";

/// The tool, for the Ask extension's tools.
pub(crate) fn tool(factory: &Arc<Factory>, agent: &str) -> Tool {
    let parameters = json!({
        "type": "object",
        "required": ["title", "text"],
        "properties": {
            "title": {"type": "string", "minLength": 1},
            "text": {"type": "string"},
            "urgent": {"type": "boolean"},
        },
        "additionalProperties": false,
    });
    crate::extensions::tool(TOOL, "Notify the person", ABOUT, parameters, crate::extensions::typed_handler(factory, agent, notify))
}

#[derive(Deserialize)]
struct Args {
    title: String,
    #[serde(default)]
    text: String,
    #[serde(default)]
    urgent: bool,
}

fn notify(factory: &Arc<Factory>, agent: &str, args: Args) -> Result<ToolResult, String> {
    let title = args.title.split_whitespace().collect::<Vec<_>>().join(" ");
    let text = args.text.trim().to_owned();
    if title.is_empty() {
        return Err("Give a title: what happened, in a short line. Nothing was sent.".to_owned());
    }
    if title.chars().count() > MAX_TITLE {
        return Err(format!("The title is a short line (at most {MAX_TITLE} characters); put the rest in text. Nothing was sent."));
    }
    if text.chars().count() > MAX_TEXT {
        return Err(format!("Keep text to a sentence or two (at most {MAX_TEXT} characters). Nothing was sent."));
    }
    let (config, person) = {
        let inner = factory.lock();
        crate::extensions::caller(&inner.config, agent)?;
        (inner.config.clone(), inner.config.person().to_owned())
    };
    if factory.pipes.contained(agent) {
        return Err(format!("You're limited to a pipe, so you can't send notifications. ask_person reaches {person}. Nothing was sent."));
    }
    let settings = super::settings(factory);
    let Some(me) = config.agent(agent) else { return Err("You're no longer in this Claktory.".to_owned()) };
    let mut note = Note::new(Kind::Alert, format!("agent:{agent}"), me.name.clone());
    note.agent_id = agent.to_owned();
    note.mode = settings.mode(agent);
    note.urgent = args.urgent;
    note.what = if text.is_empty() { title.clone() } else { format!("{title}\n{text}") };
    // It read a pipe: what it says may be the other side's words.
    note.tainted = !factory.pipes.tainted(agent).is_empty();
    let delivered = super::deliver(factory, &config, &settings, note);
    let said = match delivered {
        Delivered::Sent { apps: 0, pushed: 0 } => format!(
            "Not delivered: {person} has no Claktory app open and no device that gets notifications pushed. They'll see it in your conversation when they look."
        ),
        Delivered::Sent { .. } => format!("Sent: {person} gets it as a notification."),
        Delivered::Skipped(Skipped::Off) if settings.mode(agent) == Mode::Never => {
            format!("Not sent: {person} turned notifications from you off.")
        }
        Delivered::Skipped(Skipped::Off) => format!("Not sent: {person} turned agents' alerts off."),
        Delivered::Skipped(Skipped::Quiet) => format!(
            "Not sent: it's {person}'s quiet hours{}. They'll see it in your conversation.",
            if args.urgent { ", and they don't let urgent alerts through" } else { "" }
        ),
        Delivered::Skipped(Skipped::Active) => format!("Not sent: {person} is using Claktory right now."),
        Delivered::Skipped(Skipped::Held) => format!(
            "Held: you've already notified {person} {PER_HOUR} times this hour. When the hour allows, they get one notification saying how many more you sent."
        ),
    };
    let sent = matches!(delivered, Delivered::Sent { apps, pushed } if apps + pushed > 0);
    Ok(ToolResult::text(said, json!({"sent": sent})))
}
