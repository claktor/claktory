//! Changes an admin agent asks the person to approve (#423, Bruno 28259):
//! Rivet turns Continue automatically on or off, uninstalls an app, or
//! widens a pipe only through a card in its conversation (and Needs you),
//! the way agents_clear clears a conversation. The card keeps the change
//! (`Action`, as JSON in `asks.action`); only the person's Approve makes
//! it, checked again then, and the agent is told how it went. Dismissing or
//! replying makes nothing.
//!
//! Narrowing a pipe (pausing it, lowering its limits, taking agents out or
//! off commands, turning a review on) needs no card: Rivet makes it at once
//! with the same tool, and the pipe says who did.

use std::sync::Arc;

use factory_protocol::v1;
use serde::{Deserialize, Serialize};

use crate::factory::Factory;

/// The tools whose result can be such a card.
pub(crate) const TOOLS: [&str; 4] = [AUTO_CONTINUE_TOOL, UNINSTALL_TOOL, PIPE_TOOL, GROUP_TOOL];
pub(crate) const AUTO_CONTINUE_TOOL: &str = "models_auto_continue";
pub(crate) const UNINSTALL_TOOL: &str = "apps_uninstall";
pub(crate) const PIPE_TOOL: &str = "pipes_settings";
pub(crate) const GROUP_TOOL: &str = "pipes_group";

/// What a card's approval does.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(crate) enum Action {
    /// Settings → Providers' Continue automatically.
    AutoContinue { on: bool },
    /// Uninstalls an app, keeping its stored data or not.
    UninstallApp { app: String, keep_data: bool },
    /// Changes a pipe.
    Pipe(PipeChange),
    /// Moves a pipe into a group that shares websites, which it then gets
    /// too (`pipes::shares`): their names and ids when it was asked.
    GroupPipe {
        pipe: String,
        group: String,
        websites: Vec<String>,
        #[serde(default)]
        shares: Vec<String>,
    },
}

/// A change to a pipe, as pipes_settings asks for it: agents by id.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct PipeChange {
    pub pipe: String,
    pub pause: bool,
    pub resume: bool,
    pub close: bool,
    /// The whole new limits, when they change.
    pub limits: Option<PipeLimits>,
    /// Agents that become the pipe's, and those that stop being.
    pub add: Vec<String>,
    pub remove: Vec<String>,
    /// Agents that can no longer run commands on this computer, and those
    /// that can. One added can't unless it's in `commands`.
    pub quarantine: Vec<String>,
    pub commands: Vec<String>,
    pub lead: Option<String>,
    /// A new liaison agent, by the name it gets ("" for the one Claktory
    /// suggests).
    pub liaison: Option<String>,
    pub review_outgoing: Option<bool>,
    pub review_incoming: Option<bool>,
    /// The agents' names when it was asked, by id: a card names an agent
    /// deleted since as the person knew it.
    pub names: std::collections::BTreeMap<String, String>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct PipeLimits {
    pub messages_per_hour: u32,
    pub burst: u32,
    pub file_mb_per_day: u32,
}

impl PipeChange {
    /// What in it widens the pipe, in words: what needs the person's card.
    /// Empty when it only narrows it.
    pub(crate) fn widens(&self, record: &crate::pipes::Record, name: &dyn Fn(&str) -> String) -> Vec<String> {
        let mut wider = Vec::new();
        if self.resume {
            wider.push("resume it".to_owned());
        }
        if self.close {
            wider.push("close it".to_owned());
        }
        if !self.add.is_empty() {
            wider.push(format!("add {}", names(&self.add, name)));
        }
        if !self.commands.is_empty() {
            wider.push(format!("let {} run commands on this computer", names(&self.commands, name)));
        }
        if let Some(lead) = &self.lead {
            wider.push(format!("make {} its lead", name(lead)));
        }
        if let Some(liaison) = &self.liaison {
            wider.push(if liaison.is_empty() { "add a liaison agent".to_owned() } else { format!("add a liaison agent, {liaison}") });
        }
        if let Some(limits) = self.limits
            && (limits.messages_per_hour > record.limits.per_hour
                || limits.burst > record.limits.burst
                || limits.file_mb_per_day > record.limits.file_mb)
        {
            wider.push(format!("raise its limits to {}", limits_words(limits)));
        }
        if self.review_outgoing == Some(false) && record.review {
            wider.push("turn Review outgoing off".to_owned());
        }
        if self.review_incoming == Some(false) && record.review_incoming {
            wider.push("turn Review incoming off".to_owned());
        }
        wider
    }

    /// Everything it does, in words (Abbacchio's LOWs): asked for ("pause
    /// the pipe "Launch" and add Mia to it") or done ("Paused the pipe
    /// "Launch""), naming the pipe the first time (`pipe`, "the pipe
    /// "Launch"" or "this pipe") and "it" after.
    pub(crate) fn describe(&self, tense: Tense, pipe: &str, name: &dyn Fn(&str) -> String) -> Vec<String> {
        let done = tense == Tense::Done;
        let mut said: Vec<String> = Vec::new();
        let mut first = true;
        let mut it = || if std::mem::take(&mut first) { pipe.to_owned() } else { "it".to_owned() };
        let verb = |ask: &str, past: &str| if done { past.to_owned() } else { ask.to_owned() };
        if self.pause {
            said.push(format!("{} {}", verb("pause", "paused"), it()));
        }
        if self.resume {
            said.push(format!("{} {}", verb("resume", "resumed"), it()));
        }
        if !self.add.is_empty() {
            said.push(format!("{} {} to {}", verb("add", "added"), self.names(&self.add, name), it()));
        }
        if !self.remove.is_empty() {
            said.push(format!("{} {} out of {}", verb("take", "took"), self.names(&self.remove, name), it()));
        }
        if !self.commands.is_empty() {
            said.push(format!("let {} run commands on this computer in {}", self.names(&self.commands, name), it()));
        }
        if !self.quarantine.is_empty() {
            said.push(format!(
                "{} {} running commands on this computer in {}",
                verb("stop", "stopped"),
                self.names(&self.quarantine, name),
                it()
            ));
        }
        if let Some(lead) = &self.lead {
            said.push(format!("{} {} the lead of {}", verb("make", "made"), self.names(std::slice::from_ref(lead), name), it()));
        }
        if let Some(liaison) = &self.liaison {
            let who = if liaison.is_empty() { "a liaison agent".to_owned() } else { format!("a liaison agent, {liaison},") };
            said.push(format!("{} {who} to {}", verb("add", "added"), it()));
        }
        if let Some(limits) = self.limits {
            let whose = match it().as_str() {
                "it" => "its limits".to_owned(),
                named => format!("the limits of {named}"),
            };
            said.push(format!("set {whose} to {}", limits_words(limits)));
        }
        for (review, label) in [(self.review_outgoing, "Review outgoing"), (self.review_incoming, "Review incoming")] {
            if let Some(on) = review {
                let state = if on { "on" } else { "off" };
                said.push(format!("{} {label} {state} for {}", verb("turn", "turned"), it()));
            }
        }
        if self.close {
            said.push(format!("{} {}", verb("close", "closed"), it()));
        }
        said
    }

    /// Agents by the names they had when it was asked (an agent deleted
    /// since still reads as it did), else as they are now.
    fn names(&self, ids: &[String], name: &dyn Fn(&str) -> String) -> String {
        names(ids, &|id| self.names.get(id).cloned().unwrap_or_else(|| name(id)))
    }
}

/// How a change is told: asked for, or done.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Tense {
    Ask,
    Done,
}

/// "Paused the pipe "Launch" and added Mia to it": the first word capital.
pub(crate) fn sentence(parts: &[String]) -> String {
    let said = listed(parts);
    let mut chars = said.chars();
    chars.next().map_or_else(String::new, |first| first.to_uppercase().chain(chars).collect())
}

/// Opens the card asking the person to approve `action` for `asker` (an
/// admin agent, checked by the tool): the tool's result, which shows it.
/// `description` is what approving it does, in Claktory's words; `reason`
/// the agent's, shown apart.
pub(crate) fn ask(
    factory: &Factory,
    asker: &str,
    title: &str,
    description: &str,
    reason: &str,
    action: &Action,
) -> Result<factory_agent::ToolResult, String> {
    ask_saying(factory, asker, (title, description, reason), action, "")
}

/// `ask`, its answer starting with `before`.
fn ask_saying(
    factory: &Factory,
    asker: &str,
    (title, description, reason): (&str, &str, &str),
    action: &Action,
    before: &str,
) -> Result<factory_agent::ToolResult, String> {
    let json = serde_json::to_string(action).map_err(|error| format!("Couldn't ask: {error}"))?;
    let id = crate::extensions::ask::ask_to_approve(factory, asker, title, (description, reason), &json)?;
    let body = format!("{description} {reason}");
    let person = factory.lock().config.person().to_owned();
    let said = format!(
        "{before}Asked {person}: they see a card to approve or dismiss it, and only their approval makes the change. You'll be told which in an event. Its id is {id}: if it stops applying before they answer on the card (they answer it in your chat, say), take it back with ask_withdraw."
    );
    let mut details = serde_json::json!({
        "ask": id,
        "title": title,
        "body": body,
        "wants_text": false,
        "action_description": description,
        "action_reason": reason,
    });
    let routes = crate::pipes::agents::routes(factory, asker);
    if !routes.is_empty() {
        details["taint"] = crate::pipes::agents::routes_json(&routes);
    }
    Ok(factory_agent::ToolResult::text(said, details))
}

/// `ask`, unless `proposal` (an approved proposal's id) covers `action`
/// (`crate::proposal_grants`): then the change is made now, with no card,
/// and History says it was approved in that proposal. Not covered, the card
/// opens as without one, and the answer says why.
pub(crate) async fn ask_or_proposed(
    factory: &Arc<Factory>,
    asker: &str,
    title: &str,
    description: &str,
    reason: &str,
    action: &Action,
    proposal: Option<&str>,
) -> Result<factory_agent::ToolResult, String> {
    let Some(proposal) = proposal.map(str::trim).filter(|proposal| !proposal.is_empty()) else {
        return ask(factory, asker, title, description, reason, action);
    };
    let fits = |grant: &crate::proposal_grants::Grant| crate::proposal_grants::fits_action(factory, grant, action);
    let claimed = match crate::proposal_grants::claim(factory, asker, proposal, fits) {
        Ok(claimed) => claimed,
        Err(why) => {
            let said = format!("Proposal {proposal} doesn't cover it: {why}. ");
            return ask_saying(factory, asker, (title, description, reason), action, &said);
        }
    };
    let (me, person) = {
        let inner = factory.lock();
        (inner.config.agent(asker).map_or_else(|| asker.to_owned(), |me| me.name.clone()), inner.config.person().to_owned())
    };
    let made = match action {
        Action::Pipe(change) => {
            let by = format!("{me}, as {person} approved in {}'s proposal,", claimed.proposer_name);
            apply_pipe(factory, change, Some(&by)).await
        }
        other => match serde_json::to_string(other) {
            Ok(json) => apply(factory, &json).await,
            Err(error) => Err(error.to_string()),
        },
    };
    match made {
        Ok(done) => {
            crate::proposals::refresh(factory, claimed.proposal);
            Ok(factory_agent::ToolResult::text(
                format!("{done} {}", claimed.said(&person)),
                serde_json::json!({"proposal_grant": claimed.reference()}),
            ))
        }
        Err(why) => {
            crate::proposal_grants::unclaim(factory, &claimed);
            Err(format!("{why} Nothing was changed."))
        }
    }
}

/// "20 messages an hour, 5 in a row and 50 MB of files a day".
fn limits_words(limits: PipeLimits) -> String {
    format!("{} messages an hour, {} in a row and {} MB of files a day", limits.messages_per_hour, limits.burst, limits.file_mb_per_day)
}

/// "Ana", "Ana and Bo", "Ana, Bo and Cy".
fn names(ids: &[String], name: &dyn Fn(&str) -> String) -> String {
    let named: Vec<String> = ids.iter().map(|id| name(id)).collect();
    factory_agent::text::list_and(&named)
}

/// "pause it, add Bo and close it", for a sentence.
pub(crate) fn listed(parts: &[String]) -> String {
    factory_agent::text::list_and(parts)
}

/// The card's action, read back.
fn parse(action: &str) -> Result<Action, String> {
    serde_json::from_str(action).map_err(|_| "This card's change can't be read by this version of Claktory: dismiss it.".to_owned())
}

/// Whether the change is still one to make, checked when the person
/// approves it; refused, the card stays for them to dismiss.
pub(crate) fn check(factory: &Factory, action: &str) -> Result<(), String> {
    match parse(action)? {
        Action::AutoContinue { .. } => Ok(()),
        Action::UninstallApp { app, .. } => {
            if factory.lock().config.apps.iter().any(|installed| installed.id == app) {
                Ok(())
            } else {
                Err("That app isn't installed any more, so there's nothing to uninstall: dismiss this card.".to_owned())
            }
        }
        Action::GroupPipe { pipe, group, shares, .. } => {
            let record = factory.pipes.get(&pipe).ok_or_else(|| "That pipe is gone: dismiss this card.".to_owned())?;
            if record.state == crate::pipes::State::Closed {
                return Err("That pipe is closed now: dismiss this card.".to_owned());
            }
            crate::pipes::groups::get(factory, &group).ok_or_else(|| "That group is gone: dismiss this card.".to_owned())?;
            // Only what the card showed (Fugo 31072).
            if crate::pipes::shares::of_group(factory, &group).iter().any(|(id, _)| !shares.contains(id)) {
                return Err("The group shares more websites now: dismiss this card and ask again.".to_owned());
            }
            Ok(())
        }
        Action::Pipe(change) => {
            let record = factory.pipes.get(&change.pipe).ok_or_else(|| "That pipe is gone: dismiss this card.".to_owned())?;
            if record.state == crate::pipes::State::Closed {
                return Err("That pipe is closed now: dismiss this card.".to_owned());
            }
            let inner = factory.lock();
            let gone = change.add.iter().chain(&change.commands).chain(&change.lead).find(|id| inner.config.agent(id).is_none());
            match gone {
                Some(id) => Err(format!(
                    "{} is no longer in this Claktory, so this change can't be made: dismiss this card.",
                    change.names.get(id).unwrap_or(id)
                )),
                None => Ok(()),
            }
        }
    }
}

/// Makes the change the person approved: what it did, or why it couldn't.
pub(crate) async fn apply(factory: &Arc<Factory>, action: &str) -> Result<String, String> {
    match parse(action)? {
        Action::AutoContinue { on } => {
            crate::usage_switch::set_auto_continue(factory, on).map_err(|error| error.to_string())?;
            Ok(if on {
                "Continue automatically is on: an agent or chat waiting on a usage limit now continues on another signed-in provider by itself, when one can run its model.".to_owned()
            } else {
                "Continue automatically is off: agents and chats waiting on a usage limit wait, unless the person continues them on another provider.".to_owned()
            })
        }
        Action::UninstallApp { app, keep_data } => {
            let name = factory.apps.manifest(&app).map_or_else(|| app.clone(), |manifest| manifest.name.clone());
            factory.uninstall_app(&v1::UninstallApp { id: app, keep_data }).await.map_err(|error| error.to_string())?;
            Ok(if keep_data {
                format!("{name} is uninstalled; what it stored is kept for a later install.")
            } else {
                format!("{name} is uninstalled, with what it stored.")
            })
        }
        Action::Pipe(change) => apply_pipe(factory, &change, None).await,
        Action::GroupPipe { pipe, group, .. } => {
            let moved = crate::pipes::groups::set(factory, &pipe, &group).map_err(|error| error.to_string())?;
            let label = factory.pipes.get(&pipe).map_or_else(|| "The pipe".to_owned(), |record| crate::pipes::handoff::label(&record));
            let websites = crate::pipes::shares::reaching(factory, &pipe);
            let group = moved.map_or_else(|| "no group".to_owned(), |group| format!("the group {}", group.name));
            Ok(if websites.is_empty() {
                format!("Moved {label} to {group}.")
            } else {
                format!("Moved {label} to {group}; it now gets {}.", listed(&websites))
            })
        }
    }
}

/// Makes a change to a pipe: approved by the person (`by` None), or a
/// narrowing an admin agent makes itself (`by`, its name). What it did.
pub(crate) async fn apply_pipe(factory: &Arc<Factory>, change: &PipeChange, by: Option<&str>) -> Result<String, String> {
    let record = factory.pipes.get(&change.pipe).ok_or_else(|| "That pipe is gone.".to_owned())?;
    let error = |error: crate::factory::RequestError| match error {
        crate::factory::RequestError::Pipe(text) => text,
        other => other.to_string(),
    };
    let named = |id: &str| factory.agent_name(id);
    let pipe_called = format!("the pipe {}", crate::pipes::handoff::label(&record).trim_start_matches("pipe "));
    let done = sentence(&change.describe(Tense::Done, &pipe_called, &named));
    // The pipe's own line for its settings: pausing, resuming and closing
    // write theirs.
    let settings_only = PipeChange { pause: false, resume: false, close: false, ..change.clone() };
    let said = listed(&settings_only.describe(Tense::Done, "this pipe", &named));
    let members_change = !change.add.is_empty() || !change.remove.is_empty();
    let commands_change = members_change || !change.quarantine.is_empty() || !change.commands.is_empty();
    let mut request = v1::UpdatePipe { pipe_id: record.id.clone(), ..Default::default() };
    if members_change {
        let mut members: Vec<String> = record.members.iter().filter(|id| !change.remove.contains(id)).cloned().collect();
        for id in &change.add {
            if !members.contains(id) {
                members.push(id.clone());
            }
        }
        request.members = Some(v1::PipeMembers { agent_ids: members });
    }
    if commands_change {
        let members = request.members.as_ref().map_or_else(|| record.members.clone(), |members| members.agent_ids.clone());
        // Agents added can't run commands unless the change says so.
        let quarantined = members
            .iter()
            .filter(|id| {
                let was = record.quarantined.contains(id) || (change.add.contains(id) && !record.members.contains(id));
                (was || change.quarantine.contains(id)) && !change.commands.contains(id)
            })
            .cloned()
            .collect();
        request.quarantined = Some(v1::PipeMembers { agent_ids: quarantined });
    }
    request.lead_id = change.lead.clone();
    request.limits = change.limits.map(|limits| v1::PipeLimits {
        messages_per_hour: limits.messages_per_hour,
        burst: limits.burst,
        file_mb_per_day: limits.file_mb_per_day,
        ..Default::default()
    });
    request.review_outgoing = change.review_outgoing;
    request.review_incoming = change.review_incoming;
    let settings = request.members.is_some()
        || request.quarantined.is_some()
        || request.lead_id.is_some()
        || request.limits.is_some()
        || request.review_outgoing.is_some()
        || request.review_incoming.is_some();
    if settings {
        crate::pipes::update(factory, &request).map_err(error)?;
    }
    // Approved on a card, who joined or left shows in the pipe (Rivet's own
    // narrowing says so in its line below).
    if by.is_none() {
        for id in change.add.iter().filter(|id| !record.members.contains(id)) {
            crate::pipes::line(factory, &record.id, format!("{} joined the pipe.", named(id)));
        }
        for id in change.remove.iter().filter(|id| record.members.contains(id)) {
            crate::pipes::line(factory, &record.id, format!("{} left the pipe.", named(id)));
        }
    }
    if let Some(liaison) = &change.liaison {
        let request = v1::AddPipeLiaison { pipe_id: record.id.clone(), name: liaison.clone() };
        crate::pipes::add_liaison(factory, &request).await.map_err(error)?;
    }
    let state = |action: v1::PipeAction| v1::SetPipeState { pipe_id: record.id.clone(), action: action.into() };
    if change.pause {
        crate::pipes::set_pipe_state_by(factory, &state(v1::PipeAction::Pause), by).map_err(error)?;
    }
    if change.resume {
        crate::pipes::set_pipe_state_by(factory, &state(v1::PipeAction::Resume), by).map_err(error)?;
    }
    if settings && let Some(by) = by {
        crate::pipes::line(factory, &record.id, format!("{by} {said}."));
    }
    if change.close {
        crate::pipes::set_pipe_state_by(factory, &state(v1::PipeAction::Close), by).map_err(error)?;
    }
    factory.broadcast_workspace(&factory.lock());
    Ok(format!("{done}."))
}

/// What the agent that asked is told once the person decided: a summary,
/// and the event's text. `made`: how making it went, approved; None when it
/// wasn't (dismissed, or a reply).
pub(crate) fn told(person: &str, title: &str, comment: &str, replied: bool, made: Option<Result<String, String>>) -> (String, String) {
    let their = if comment.is_empty() { String::new() } else { format!("\n\nTheir comment:\n\n{comment}") };
    match made {
        Some(Ok(done)) => (format!("{person} approved \"{title}\"."), format!("{person} approved \"{title}\". {done}{their}")),
        Some(Err(why)) => {
            (format!("\"{title}\" couldn't be done."), format!("{person} approved \"{title}\", but it couldn't be done: {why}{their}"))
        }
        None if replied => (
            format!("{person} replied to \"{title}\"."),
            format!("{person} replied to \"{title}\" instead of approving it, so nothing was changed:\n\n{comment}"),
        ),
        None => (format!("{person} dismissed \"{title}\"."), format!("{person} dismissed \"{title}\", so nothing was changed.{their}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn named(id: &str) -> String {
        let mut name = id.to_owned();
        name[..1].make_ascii_uppercase();
        name
    }

    #[test]
    fn a_change_reads_as_asked_and_as_done() {
        // Abbacchio's LOWs (row 1054).
        let pipe = "the pipe \"Launch\"";
        let resume = PipeChange { resume: true, ..Default::default() };
        assert_eq!(listed(&resume.describe(Tense::Ask, pipe, &named)), "resume the pipe \"Launch\"");
        let both = PipeChange { pause: true, add: vec!["mia".into()], ..Default::default() };
        assert_eq!(listed(&both.describe(Tense::Ask, pipe, &named)), "pause the pipe \"Launch\" and add Mia to it");
        let pause = PipeChange { pause: true, ..Default::default() };
        assert_eq!(sentence(&pause.describe(Tense::Done, pipe, &named)), "Paused the pipe \"Launch\"");
        let out = PipeChange { remove: vec!["lea".into()], ..Default::default() };
        assert_eq!(sentence(&out.describe(Tense::Done, pipe, &named)), "Took Lea out of the pipe \"Launch\"");
        // Named as it was asked, once that agent is gone.
        let kept = PipeChange { add: vec!["mia".into()], names: [("mia".into(), "Mía".into())].into(), ..Default::default() };
        assert_eq!(listed(&kept.describe(Tense::Ask, pipe, &|id| id.to_owned())), "add Mía to the pipe \"Launch\"");
    }
}
