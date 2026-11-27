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
/// anything longer is for ask_person with review.
pub(crate) const MAX_TEXT: usize = 300;

const ABOUT: &str = "Notify the person with a one-line ping on their devices (their Mac, their phone's lock screen) when something can't wait until they next look at Claktory: work they're waiting on is done or stuck, a service is down, a deadline is about to pass. title: what happened, in a short line (\"account.claktory.com isn't answering\"). text: a sentence more at most (300 characters), or nothing. Anything to explain, or for them to read (a report, a summary, findings, a file), goes as ask_person with review, never in an alert, even when they said \"notify me\": an alert about it only points at the request. Never instead of ask_person when you need an answer: an alert is one-way. Not for routine progress. Every alert also waits in their Needs you until they read it. urgent: true only when it can't wait for the morning; their settings decide whether it breaks quiet hours, and only the agents they allow send urgent alerts (Rivet and agents that lead others unless they changed it): anyone else's goes as a normal one. An alert about the same thing as another agent's still unread from the last 30 minutes folds into theirs, naming you on it, without a notification of its own. You notify at most 6 times an hour; past that, they're told only how many more you sent. Says whether it reached anyone.";

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
    // Only agents the person lets send urgent alerts do;
    // another's goes as a normal one, and it's told who may.
    let downgraded = args.urgent && !super::urgent::allowed(factory, &config, agent);
    note.urgent = args.urgent && !downgraded;
    // It read a web page: what it says may be a stranger's words. The same
    // test as a waiting card's notification (quarantined ones are refused
    // above, as contained).
    note.tainted = !factory.pipes.tainted(agent).is_empty() || factory.pipes.quarantined(agent);
    let mut said = Vec::new();
    if downgraded {
        let senders = super::urgent::senders(&config, &super::urgent::chosen(factory));
        let names: Vec<String> = senders.iter().map(|agent| agent.name.clone()).collect();
        said.push(match names.as_slice() {
            [] => "Sent as a normal alert: no agent may send urgent ones.".to_owned(),
            names => format!("Sent as a normal alert: only {} may send urgent ones.", factory_agent::text::list_and(names)),
        });
    }
    let kept = "It waits in their Needs you until they read it.";
    // About the same thing as another agent's alert still unread: folded
    // into it, with no notification of its own but once, when it makes that
    // one urgent. Never one whose words may be a stranger's.
    let folded = if note.tainted { None } else { super::alerts::fold(factory, agent, &me.name, &title, note.urgent) };
    if let Some(folded) = folded {
        let into = folded.into;
        said.push(format!(
            "Folded into {}'s alert about the same thing, \"{}\", still unread: {person} sees you named on it, and no notification of its own was sent.",
            into.from_name, into.title
        ));
        let mut sent = false;
        if folded.upgraded {
            // The alert as it reads, from whom it's from, urgent now.
            let mut upgrade = Note::new(Kind::Alert, note.source.clone(), into.from_name.clone());
            upgrade.agent_id = into.agent_id.clone();
            upgrade.mode = settings.mode(&into.agent_id);
            upgrade.urgent = true;
            upgrade.seq = into.seq;
            upgrade.alert_id = into.id.clone();
            upgrade.waits = format!("alert:{}", into.id);
            upgrade.what = if into.text.is_empty() { into.title.clone() } else { format!("{}\n{}", into.title, into.text) };
            let delivered = super::deliver(factory, &config, &settings, upgrade);
            sent = matches!(delivered, Delivered::Sent { apps, pushed } if apps + pushed > 0);
            said.push(format!("Your alert made it urgent. {}", told(factory, &settings, agent, &person, true, &delivered)));
        } else {
            said.push(kept.to_owned());
        }
        return Ok(ToolResult::text(said.join(" "), json!({"sent": sent, "alert": into.id, "folded": true})));
    }
    // Its call's entry, which the alert and its notification open.
    note.seq = super::alerts::call_seq(factory, agent, &title);
    let urgent = note.urgent;
    let (alert, delivered) = super::alert(factory, &config, &settings, note, &title, &text);
    said.push(told(factory, &settings, agent, &person, urgent, &delivered));
    let sent = matches!(delivered, Delivered::Sent { apps, pushed } if apps + pushed > 0);
    Ok(ToolResult::text(said.join(" "), json!({"sent": sent, "alert": alert})))
}

/// What became of an alert's notification (`urgent` as it was sent), in
/// words for its agent.
fn told(factory: &Factory, settings: &super::NotificationsConfig, agent: &str, person: &str, urgent: bool, delivered: &Delivered) -> String {
    let kept = "It waits in their Needs you until they read it.";
    match delivered {
        Delivered::Sent { apps: 0, pushed: 0 } => match factory.notify.off_on(factory).as_slice() {
            [] => format!(
                "Not delivered as a notification: {person} has no Claktory app open that shows notifications, and no device that gets them pushed. {kept}"
            ),
            off => format!(
                "Not delivered as a notification: notifications are turned off on {}, and no other device of {person}'s shows them. {kept}",
                factory_agent::text::list_and(off)
            ),
        },
        Delivered::Sent { .. } => format!("Sent: {person} gets it as a notification, and it waits in their Needs you until they read it."),
        Delivered::Skipped(Skipped::Off) if settings.mode(agent) == Mode::Never => {
            format!("Not sent as a notification: {person} turned notifications from you off. {kept}")
        }
        Delivered::Skipped(Skipped::Off) => format!("Not sent as a notification: {person} turned agents' alerts off. {kept}"),
        Delivered::Skipped(Skipped::Quiet) => format!(
            "Not sent as a notification: it's {person}'s quiet hours{}. {kept} When quiet hours end, they're told what came meanwhile.",
            if urgent { ", and they don't let urgent alerts through" } else { "" }
        ),
        Delivered::Skipped(Skipped::Active) => format!("Not sent as a notification: {person} is using Claktory right now. {kept}"),
        Delivered::Skipped(Skipped::Held) => format!(
            "Held: you've already notified {person} {PER_HOUR} times this hour. When the hour allows, they get one notification saying how many more you sent. {kept}"
        ),
    }
}
