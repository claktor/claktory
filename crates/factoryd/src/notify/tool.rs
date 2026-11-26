//! `notify_person`: an agent tells the person something that can't wait for
//! them to look (an ops watchdog's "account.claktory.com isn't answering"),
//! as a one-line notification on their devices, kept in their Needs you
//! until they read it (`super::alerts`). Part of the Ask extension, so a
//! liaison quarantined for a pipe keeps it, under the rule that an agent
//! that has read a web page puts none of its words in a notification, nor
//! in Needs you.

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
/// The longest text, in characters: a notification is a one-line ping, and
/// anything longer is for ask_person with review (Bruno 44235).
pub(crate) const MAX_TEXT: usize = 300;

const ABOUT: &str = "Notify the person with a one-line ping on their devices (their Mac, their phone's lock screen) when something can't wait until they next look at Claktory: work they're waiting on is done or stuck, a service is down, a deadline is about to pass. title: what happened, in a short line (\"account.claktory.com isn't answering\"). text: a sentence more at most (300 characters), or nothing. Anything to explain, or for them to read (a report, a summary, findings, a file), goes as ask_person with review, never in an alert, even when they said \"notify me\": an alert about it only points at the card. Never instead of ask_person when you need an answer: an alert is one-way. Not for routine progress. Every alert also waits in their Needs you until they read it. urgent: true only when it can't wait for the morning; their settings decide whether it breaks quiet hours. You notify at most 6 times an hour; past that, they're told only how many more you sent. Says whether it reached anyone.";

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
        return Err(format!(
            "An alert is a one-line ping: keep text to a sentence (at most {MAX_TEXT} characters). Anything to explain or read goes as ask_person with review. Nothing was sent."
        ));
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
    // It read a web page: what it says may be a stranger's words. The same
    // test as a waiting card's notification (quarantined ones are refused
    // above, as contained).
    note.tainted = !factory.pipes.tainted(agent).is_empty() || factory.pipes.quarantined(agent);
    // Its call's entry, which the alert and its notification open.
    note.seq = super::alerts::call_seq(factory, agent, &title);
    let (alert, delivered) = super::alert(factory, &config, &settings, note, &title, &text);
    let kept = "It waits in their Needs you until they read it.";
    let said = match delivered {
        Delivered::Sent { apps: 0, pushed: 0 } => format!(
            "Not delivered as a notification: {person} has no Claktory app open that shows notifications, and no device that gets them pushed. {kept}"
        ),
        Delivered::Sent { .. } => format!("Sent: {person} gets it as a notification, and it waits in their Needs you until they read it."),
        Delivered::Skipped(Skipped::Off) if settings.mode(agent) == Mode::Never => {
            format!("Not sent as a notification: {person} turned notifications from you off. {kept}")
        }
        Delivered::Skipped(Skipped::Off) => format!("Not sent as a notification: {person} turned agents' alerts off. {kept}"),
        Delivered::Skipped(Skipped::Quiet) => format!(
            "Not sent as a notification: it's {person}'s quiet hours{}. {kept} When quiet hours end, they're told what came meanwhile.",
            if args.urgent { ", and they don't let urgent alerts through" } else { "" }
        ),
        Delivered::Skipped(Skipped::Active) => format!("Not sent as a notification: {person} is using Claktory right now. {kept}"),
        Delivered::Skipped(Skipped::Held) => format!(
            "Held: you've already notified {person} {PER_HOUR} times this hour. When the hour allows, they get one notification saying how many more you sent. {kept}"
        ),
    };
    let sent = matches!(delivered, Delivered::Sent { apps, pushed } if apps + pushed > 0);
    Ok(ToolResult::text(said, json!({"sent": sent, "alert": alert})))
}
