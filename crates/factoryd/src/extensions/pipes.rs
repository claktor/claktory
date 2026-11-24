//! The Pipes extension: an agent starts a pipe for the person (#244), the
//! way devices_add adds a device. `pipes_invite` and `pipes_accept` open a
//! card in its conversation for the person to make the invite, or paste the
//! code they were given, themselves (`crate::pipes::start`); `pipes_list`
//! says where the pipes stand. Admin agents such as Rivet have it unless
//! it's turned off. No tool takes or returns a code, approves a pipe or
//! chooses the agents that read it.
//!
//! `pipes_forward` (pipes-automation.md §7.1) sets where a pipe's reports go,
//! or a project's default, and can make the recipient a report reader: only
//! for an admin agent, checked on every call, and never while it has read
//! anything from outside (a pipe, the web, direct messages). It only moves
//! toward containment: nothing in it lifts a reader or a liaison, and it
//! never releases reports waiting for the person.
//!
//! `pipes_group` (#359) arranges a project's pipes into groups in the
//! sidebar, as the person does from a pipe's menu: only for an admin agent.
//! Arrangement only, so nothing about a pipe changes.
//!
//! `pipes_sender` (#414) lets an agent only send to a pipe the person
//! approved, or stops it: only for an admin agent, checked on every call,
//! never while it has read anything from outside, and only for agents that
//! aren't the pipe's or only send to it, never one that reads it. Each
//! change is in Needs you's history.
//!
//! `pipes_settings` (#423) changes a pipe as the person asks, for an admin
//! agent that hasn't read anything from outside: what narrows it at once
//! (the pipe says who), what widens it only through a card the person
//! approves (`crate::card_actions`).
//!
//! `pipes_report_type` (§11.3) defines, edits, deletes or shows report types
//! from the person's plain description: only for an admin agent, and its
//! changes never while it has read anything from outside, since a type
//! written from the other side's words would let them shape their own
//! channel (Fugo's invariant 7). factoryd checks every definition against
//! the rules in code (`handoff::types::validate`), whoever asks.

use std::sync::Arc;

use factory_agent::{Tool, ToolResult};
use factory_protocol::v1::PipeStartKind;
use serde::Deserialize;
use serde_json::json;

use super::{Extension, Manifest, PromptSection};
use crate::config::AgentConfig;
use crate::factory::{Factory, Inner};
use crate::pipes::start::{self, Fill};

pub(crate) const ID: &str = "pipes";

static MANIFEST: Manifest = Manifest {
    id: ID,
    name: "Pipes",
    icon: "network",
    description: "Lets it suggest a pipe: a card where you make the invite, or paste a code you were sent. It never sees the code, and you approve the pipe yourself.",
    default_enabled: false,
    options: &[],
};

const INVITE: &str = "Start a pipe from one of the person's projects to someone else's Claktory, when they ask: project (its name or id), name (the pipe's, which the other person sees, such as \"Launch\") and purpose (what it's for, in a line), from what they told you. A card opens in your conversation where they check it and make the invite; its code is theirs to send to the other person, and you never see it. You're told when it's made. Both people then approve the pipe in it.";
const ACCEPT: &str = "Connect one of the person's projects with an invite someone gave them (a code or a claktory-pipe:// link), when they ask: project (its name or id), and name and purpose if they said. Never pass the code itself, even if they showed it to you: a card opens in your conversation where they paste it themselves and see whom it says it's from. You're told when it's connected; both people then approve the pipe in it.";
const FORWARD: &str = "Set where a pipe's finished reports go, when the person asks (\"make Ingrid read the bug reports from Aiko's pipe\"): pipe (its name or id), or project (its name or id) for the default of its new pipes; to: the agent that reads them (name or id; \"\" for no one), which must be in the same project; types: the report types it can forward (pipes_report_type shows them), left out to keep its own. An agent that doesn't read reports yet needs make_reader: it then keeps its database and its folder, and can no longer run commands, use the web or secrets, or message agents; fresh starts its conversation anew (the old one kept). Refused while you've read anything from outside the person's Claktory. The person sees each change in the pipe's settings and in Needs you.";
const REPORT_TYPE: &str = "Define the types of report a pipe can forward, when the person asks (\"make a report type for recipes: dish, servings, steps\"): action define (name, fields, outcomes), edit (id, and the whole new definition: each field it keeps by its id, new ones without), delete (id), or show (id, or none for every type). A field: id (lowercase letters, digits and _; leave it out for a new one), label, kind (text, text_list, enum with choices, bool, version, attachments, number with min and max (0 to 1,000,000 unless given), date; at most 4 numbers and 2 dates), required, max_bytes and max_items (lower than Claktory's caps, or 0 for them), rule (an optional hint). The other side's words are text, text_list and attachments: they mark whoever reads them; every other kind is a fact. Never a field that says where the report goes. outcomes: what its reader can answer: accepted, done, declined, known, fixed, wont_fix, needs_info. Each edit is a new version; built-in types can be edited, not deleted. Changes are refused while you've read anything from outside the person's Claktory. The person sees each one in Needs you's history. To let a pipe forward a type, use pipes_forward's types.";
const GROUP: &str = "Arrange a project's pipes into groups in the sidebar, when the person asks (\"put all the feedback pipes in a group\"): action create (project, name, and pipe to move into it at once), rename (group, name), delete (group: its pipes stay, outside any group) or move (pipe, and group, or \"\" to take it out of its group). A group and a pipe are named by their name or id; pipes_list shows each pipe's group. A pipe can only be in a group of its own project. Arrangement only: pipes work the same in a group or out of one.";
const SENDER: &str = "Let an agent send to one of the person's pipes without reading it, when they ask (\"let Rivet post our release notes to Aiko's pipe\"): pipe (its name or id), agent (name or id; any agent of the person's, you included), send: true to let it, false to stop one that only sends. It gets pipe_send and pipe_status for that pipe, never pipe_read: it isn't told what arrives, what the other side answers, or why they refused a message. Only for a pipe the person approved and that isn't closed, and only for an agent that isn't one of its agents yet; the person changes agents that read it, in the pipe's settings. Refused while you've read anything from outside the person's Claktory. The person sees each change in Needs you's history.";
const SETTINGS: &str = "Change one of the person's pipes, when they ask: pipe (its name or id) and reason (why, in a sentence they read), then what changes. Narrowing it is made at once, and the pipe says you did: pause (true), lower limits (messages_per_hour, burst, file_mb_per_day; those left out stay), remove (agents that stop being its agents), quarantine (agents that can no longer run commands on this computer), review_outgoing or review_incoming true. Widening it opens a card in your conversation (and their Needs you) for the person to approve, with the whole change, and nothing changes until they do: resume (true), close (true), raised limits, add (agents that become its agents, unable to run commands unless also in commands), commands (agents that can run commands), lead (an agent), liaison (a new liaison agent's name, \"\" for the one Claktory suggests), or a review turned off. Agents by name or id. Refused while you've read anything from outside the person's Claktory. Only the person deletes a pipe, or changes its name, what it's for and its instructions.";
const LIST: &str = "The person's pipes, or one project's (project, its name or id): each one's name, id and where it stands. Never codes, digits or what the other side says about itself.";

pub(crate) struct Pipes;

impl Extension for Pipes {
    fn manifest(&self) -> &'static Manifest {
        &MANIFEST
    }

    fn prompt(&self, _factory: &Factory, inner: &Inner, _agent: &AgentConfig) -> Vec<PromptSection> {
        let person = inner.config.person();
        let text = format!(
            "## Starting pipes\n\nWhen {person} wants one of their projects to talk with someone else's Claktory, start the pipe for them: pipes_invite when they're inviting someone, pipes_accept when someone gave them a code or link. Either opens a card in your conversation, filled in from what they told you, where they make the invite or paste the code themselves. You never see a code (never ask for one, and never pass one on), you don't approve the pipe, and you don't choose the agents that read it: {person} does, in the pipe. When they ask, pipes_sender lets an agent only send to a pipe they approved, without reading it, and pipes_settings changes a pipe: what narrows it at once, what widens it on a card they approve. pipes_list says where their pipes stand.\n"
        );
        vec![PromptSection::static_text(text)]
    }

    fn tools(&self, factory: &Arc<Factory>, agent: &str) -> Vec<Tool> {
        let fill = |required: &[&str]| {
            json!({
                "type": "object",
                "required": required,
                "properties": {
                    "project": {"type": "string"},
                    "name": {"type": "string"},
                    "purpose": {"type": "string"},
                },
                "additionalProperties": false,
            })
        };
        let list = json!({"type": "object", "properties": {"project": {"type": "string"}}, "additionalProperties": false});
        let group = json!({
            "type": "object",
            "required": ["action"],
            "properties": {
                "action": {"type": "string", "enum": ["create", "rename", "delete", "move"]},
                "project": {"type": "string"},
                "group": {"type": "string"},
                "name": {"type": "string"},
                "pipe": {"type": "string"},
            },
            "additionalProperties": false,
        });
        let forward = json!({
            "type": "object",
            "required": ["to"],
            "properties": {
                "pipe": {"type": "string"},
                "project": {"type": "string"},
                "to": {"type": "string"},
                "types": {"type": "array", "items": {"type": "string"}},
                "make_reader": {"type": "boolean"},
                "fresh": {"type": "boolean"},
            },
            "additionalProperties": false,
        });
        vec![
            super::tool(
                "pipes_invite",
                "Invite to a pipe",
                INVITE,
                fill(&["project", "name"]),
                super::typed_handler(factory, agent, invite),
            ),
            super::tool("pipes_accept", "Join a pipe", ACCEPT, fill(&["project"]), super::typed_handler(factory, agent, accept)),
            super::tool("pipes_list", "List pipes", LIST, list, super::typed_handler(factory, agent, list_pipes)),
            super::tool("pipes_forward", "Forward a pipe's reports", FORWARD, forward, forward_handler(factory, agent)),
            super::tool("pipes_group", "Group pipes", GROUP, group, super::typed_handler(factory, agent, group_pipes)),
            super::tool("pipes_sender", "Send to a pipe", SENDER, sender_parameters(), super::typed_handler(factory, agent, sender)),
            super::tool("pipes_report_type", "Report types", REPORT_TYPE, report_type_parameters(), super::typed_handler(factory, agent, report_type)),
            super::async_tool(factory, agent, crate::card_actions::PIPE_TOOL, "Change a pipe", SETTINGS, settings_parameters(), settings),
        ]
    }

    fn admin_default(&self) -> bool {
        true
    }
}

#[derive(Deserialize)]
struct FillArgs {
    project: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    purpose: String,
}

#[derive(Deserialize)]
struct ListArgs {
    #[serde(default)]
    project: Option<String>,
}

fn opened(factory: &Arc<Factory>, agent: &str, kind: PipeStartKind, args: FillArgs) -> Result<ToolResult, String> {
    super::caller(&factory.lock().config, agent)?;
    let (said, details) = start::open(factory, agent, Fill { kind, project: args.project, name: args.name, purpose: args.purpose })?;
    Ok(ToolResult::text(said, details))
}

fn invite(factory: &Arc<Factory>, agent: &str, args: FillArgs) -> Result<ToolResult, String> {
    opened(factory, agent, PipeStartKind::Invite, args)
}

fn accept(factory: &Arc<Factory>, agent: &str, args: FillArgs) -> Result<ToolResult, String> {
    opened(factory, agent, PipeStartKind::Accept, args)
}

#[derive(Deserialize)]
struct ForwardArgs {
    #[serde(default)]
    pipe: String,
    #[serde(default)]
    project: String,
    to: String,
    #[serde(default)]
    types: Option<Vec<String>>,
    /// From a tool made before report types were data: one type.
    #[serde(default)]
    schema: String,
    #[serde(default)]
    make_reader: bool,
    #[serde(default)]
    fresh: bool,
}

fn forward_handler(factory: &Arc<Factory>, agent: &str) -> factory_agent::Handler {
    super::agent_handler(factory, agent, |factory, agent, args| async move {
        let args: ForwardArgs = serde_json::from_value(args)?;
        let factory = super::live(factory)?;
        forward(&factory, &agent, args).await.map(|said| ToolResult::text(said, json!({}))).map_err(factory_agent::Error::Other)
    })
}

/// The caller is an admin agent (`what`: what only it can do), and has
/// read nothing from outside (`where_else`: where the person can do it).
fn untainted_admin(factory: &Factory, agent: &str, what: &str, where_else: &str) -> Result<(), String> {
    {
        let inner = factory.lock();
        let caller = super::caller(&inner.config, agent)?;
        if !crate::proposals::is_admin(caller) {
            return Err(format!("Only an admin agent can {what}; the person can, {where_else}."));
        }
    }
    if !factory.pipes.tainted(agent).is_empty() {
        let read = crate::pipes::agents::what_was_read(factory, agent);
        return Err(format!(
            "Not while you've read {read}: what you read there could have steered this. The person can do it {where_else}, or clear your conversation."
        ));
    }
    Ok(())
}

fn settings_parameters() -> serde_json::Value {
    let agents = json!({"type": "array", "items": {"type": "string"}});
    let count = json!({"type": "integer", "minimum": 1});
    json!({
        "type": "object",
        "required": ["pipe", "reason"],
        "properties": {
            "pipe": {"type": "string", "minLength": 1},
            "reason": {"type": "string", "minLength": 1, "maxLength": 500},
            "pause": {"type": "boolean"},
            "resume": {"type": "boolean"},
            "close": {"type": "boolean"},
            "messages_per_hour": count,
            "burst": count,
            "file_mb_per_day": count,
            "add": agents,
            "remove": agents,
            "quarantine": agents,
            "commands": agents,
            "lead": {"type": "string", "minLength": 1},
            "liaison": {"type": "string"},
            "review_outgoing": {"type": "boolean"},
            "review_incoming": {"type": "boolean"},
        },
        "additionalProperties": false,
    })
}

#[derive(Deserialize)]
struct SettingsArgs {
    pipe: String,
    reason: String,
    #[serde(default)]
    pause: bool,
    #[serde(default)]
    resume: bool,
    #[serde(default)]
    close: bool,
    messages_per_hour: Option<u32>,
    burst: Option<u32>,
    file_mb_per_day: Option<u32>,
    #[serde(default)]
    add: Vec<String>,
    #[serde(default)]
    remove: Vec<String>,
    #[serde(default)]
    quarantine: Vec<String>,
    #[serde(default)]
    commands: Vec<String>,
    lead: Option<String>,
    liaison: Option<String>,
    review_outgoing: Option<bool>,
    review_incoming: Option<bool>,
}

/// An agent of the person's by id, or by name (a name two share: the one in
/// the pipe's project).
fn find_agent(factory: &Factory, wanted: &str, project: &str) -> Result<String, String> {
    let wanted = wanted.trim();
    let inner = factory.lock();
    let agents = &inner.config.agents;
    agents
        .iter()
        .find(|candidate| candidate.id == wanted)
        .or_else(|| {
            let named: Vec<_> = agents.iter().filter(|candidate| candidate.name.eq_ignore_ascii_case(wanted)).collect();
            named.iter().find(|candidate| candidate.project.as_deref() == Some(project)).or(named.first()).copied()
        })
        .map(|found| found.id.clone())
        .ok_or_else(|| format!("There's no agent \"{wanted}\"."))
}

/// pipes_settings (#423, Bruno 28259): narrows a pipe at once, or asks the
/// person on a card to approve a change that widens it.
async fn settings(factory: Arc<Factory>, agent: String, args: SettingsArgs) -> Result<ToolResult, String> {
    use crate::card_actions::{Action, PipeChange, PipeLimits, listed};
    untainted_admin(&factory, &agent, "change a pipe's settings", "in the pipe's settings")?;
    let record = find_pipe(&factory, args.pipe.trim())?;
    let pipe_called = format!("the pipe {}", crate::pipes::handoff::label(&record).trim_start_matches("pipe "));
    if record.state == crate::pipes::State::Closed {
        return Err("That pipe is closed: only the person can delete it. Nothing was changed.".to_owned());
    }
    let reason = args.reason.split_whitespace().collect::<Vec<_>>().join(" ");
    if reason.is_empty() {
        return Err("Give reason: why, in a sentence the person reads. Nothing was changed.".to_owned());
    }
    let ids = |list: &[String]| list.iter().map(|wanted| find_agent(&factory, wanted, &record.project)).collect::<Result<Vec<_>, _>>();
    let limits = (args.messages_per_hour.is_some() || args.burst.is_some() || args.file_mb_per_day.is_some()).then(|| PipeLimits {
        messages_per_hour: args.messages_per_hour.unwrap_or(record.limits.per_hour),
        burst: args.burst.unwrap_or(record.limits.burst),
        file_mb_per_day: args.file_mb_per_day.unwrap_or(record.limits.file_mb),
    });
    let change = PipeChange {
        pipe: record.id.clone(),
        pause: args.pause,
        resume: args.resume,
        close: args.close,
        limits,
        add: ids(&args.add)?,
        remove: ids(&args.remove)?,
        quarantine: ids(&args.quarantine)?,
        commands: ids(&args.commands)?,
        lead: args.lead.as_deref().map(|lead| find_agent(&factory, lead, &record.project)).transpose()?,
        liaison: args.liaison.map(|name| name.trim().to_owned()),
        review_outgoing: args.review_outgoing,
        review_incoming: args.review_incoming,
    };
    if change.pause && change.resume {
        return Err("Pause or resume it, not both. Nothing was changed.".to_owned());
    }
    if let Some(both) = change.add.iter().find(|id| change.remove.contains(id)) {
        return Err(format!("{both} is both added and removed. Nothing was changed."));
    }
    if let Some(both) = change.commands.iter().find(|id| change.quarantine.contains(id)) {
        return Err(format!("{both} is both let run commands and stopped. Nothing was changed."));
    }
    let named = |id: &str| factory.lock().config.agent(id).map_or_else(|| id.to_owned(), |agent| agent.name.clone());
    let said = change.describe(&named);
    if said.is_empty() {
        return Err("Say what to change. Nothing was changed.".to_owned());
    }
    let me = named(&agent);
    let wider = change.widens(&record, &named);
    if wider.is_empty() {
        let done =
            crate::card_actions::apply_pipe(&factory, &change, Some(&me)).await.map_err(|why| format!("{why} Nothing was changed."))?;
        return Ok(ToolResult::text(format!("{done} The person sees it in the pipe."), json!({})));
    }
    let title = format!("Change {pipe_called}");
    let description = format!("{me} asks to {} on {pipe_called}.", listed(&said));
    crate::card_actions::ask(&factory, &agent, &title, &description, &reason, &Action::Pipe(change))
}

fn sender_parameters() -> serde_json::Value {
    json!({
        "type": "object",
        "required": ["pipe", "agent", "send"],
        "properties": {
            "pipe": {"type": "string"},
            "agent": {"type": "string"},
            "send": {"type": "boolean"},
        },
        "additionalProperties": false,
    })
}

#[derive(Deserialize)]
struct SenderArgs {
    pipe: String,
    agent: String,
    send: bool,
}

/// pipes_sender: the admin agent lets an agent only send to a pipe, or
/// stops it, as the person asked (`pipes::senders`).
fn sender(factory: &Arc<Factory>, agent: &str, args: SenderArgs) -> Result<ToolResult, String> {
    untainted_admin(factory, agent, "let an agent send to a pipe", "in the pipe's settings")?;
    let pipe = find_pipe(factory, args.pipe.trim())?;
    let wanted = args.agent.trim();
    let member = {
        let inner = factory.lock();
        let agents = &inner.config.agents;
        agents
            .iter()
            .find(|candidate| candidate.id == wanted)
            .or_else(|| {
                let named: Vec<_> = agents.iter().filter(|candidate| candidate.name.eq_ignore_ascii_case(wanted)).collect();
                // A name two agents share: the one in the pipe's project.
                named.iter().find(|candidate| candidate.project.as_deref() == Some(pipe.project.as_str())).or(named.first()).copied()
            })
            .map(|found| found.id.clone())
            .ok_or_else(|| format!("There's no agent \"{wanted}\"."))?
    };
    let said = crate::pipes::senders::set(factory, agent, &pipe.id, &member, args.send)?;
    Ok(ToolResult::text(said, json!({})))
}

#[derive(Deserialize)]
struct GroupArgs {
    action: String,
    #[serde(default)]
    project: String,
    #[serde(default)]
    group: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    pipe: String,
}

/// A pipe by its id, or by the name this side gave it.
fn find_pipe(factory: &Factory, wanted: &str) -> Result<crate::pipes::Record, String> {
    let pipes = factory.pipes.all();
    pipes
        .iter()
        .find(|record| record.id == wanted)
        .or_else(|| pipes.iter().find(|record| record.named_here.unwrap_or(record.peer.is_none()) && record.name.eq_ignore_ascii_case(wanted)))
        .cloned()
        .ok_or_else(|| format!("There's no pipe \"{wanted}\" (pipes_list names them)."))
}

/// A group by its id, or by its name (in `project` when given).
fn find_group(factory: &Factory, wanted: &str, project: &str) -> Result<crate::pipes::groups::Group, String> {
    let wanted = wanted.trim();
    if wanted.is_empty() {
        return Err("Say which group.".to_owned());
    }
    let groups = crate::pipes::groups::all(factory);
    if let Some(group) = groups.iter().find(|group| group.id == wanted) {
        return Ok(group.clone());
    }
    let named: Vec<&crate::pipes::groups::Group> =
        groups.iter().filter(|group| group.name.eq_ignore_ascii_case(wanted) && (project.is_empty() || group.project == project)).collect();
    match named.as_slice() {
        [group] => Ok((*group).clone()),
        [] => Err(format!("There's no group \"{wanted}\" (pipes_list shows each pipe's group).")),
        _ => Err(format!("More than one project has a group \"{wanted}\": say which project.")),
    }
}

/// pipes_group: the admin agent arranges pipes into groups, as the person asked.
fn group_pipes(factory: &Arc<Factory>, agent: &str, args: GroupArgs) -> Result<ToolResult, String> {
    use crate::pipes::groups;
    use crate::pipes::handoff::label;
    let (project, project_name) = {
        let inner = factory.lock();
        let caller = super::caller(&inner.config, agent)?;
        if !crate::proposals::is_admin(caller) {
            return Err("Only an admin agent can arrange pipes into groups; the person can, from a pipe's … menu.".to_owned());
        }
        let wanted = args.project.trim();
        if wanted.is_empty() {
            (String::new(), String::new())
        } else {
            let project = inner
                .config
                .projects
                .iter()
                .find(|project| project.id == wanted || project.name.eq_ignore_ascii_case(wanted))
                .ok_or_else(|| format!("There's no project \"{wanted}\"."))?;
            (project.id.clone(), project.name.clone())
        }
    };
    let error = |error: crate::factory::RequestError| error.to_string();
    let said = match args.action.as_str() {
        "create" => {
            let pipe = if args.pipe.trim().is_empty() { None } else { Some(find_pipe(factory, args.pipe.trim())?) };
            let project = match (&pipe, project.as_str()) {
                (Some(pipe), "") => pipe.project.clone(),
                (_, "") => return Err("Say which project the group is in.".to_owned()),
                (_, project) => project.to_owned(),
            };
            let made = groups::create(factory, &project, &args.name, pipe.as_ref().map_or("", |pipe| pipe.id.as_str())).map_err(error)?;
            let place = factory.lock().config.project(&made.project).map(|project| project.name.clone()).unwrap_or(project_name);
            match pipe {
                Some(pipe) => format!("Made the group {} in {place}, and moved {} to it.", made.name, label(&pipe)),
                None => format!("Made the group {} in {place}. Move pipes to it with action move.", made.name),
            }
        }
        "rename" => {
            let group = find_group(factory, &args.group, &project)?;
            let renamed = groups::rename(factory, &group.id, &args.name).map_err(error)?;
            format!("Renamed the group {} to {}.", group.name, renamed.name)
        }
        "delete" => {
            let group = find_group(factory, &args.group, &project)?;
            let (group, pipes) = groups::delete(factory, &group.id).map_err(error)?;
            match pipes {
                0 => format!("Deleted the group {}; it had no pipes.", group.name),
                1 => format!("Deleted the group {}; its pipe stays, outside any group.", group.name),
                pipes => format!("Deleted the group {}; its {pipes} pipes stay, outside any group.", group.name),
            }
        }
        "move" => {
            let pipe = find_pipe(factory, args.pipe.trim())?;
            if args.group.trim().is_empty() {
                groups::set(factory, &pipe.id, "").map_err(error)?;
                format!("Moved {} out of its group.", label(&pipe))
            } else {
                let group = find_group(factory, &args.group, &pipe.project)?;
                groups::set(factory, &pipe.id, &group.id).map_err(error)?;
                format!("Moved {} to the group {}.", label(&pipe), group.name)
            }
        }
        other => return Err(format!("There's no action \"{other}\": create, rename, delete or move.")),
    };
    Ok(ToolResult::text(said, json!({})))
}

/// pipes_forward: the admin agent sets where reports go, as the person asked.
async fn forward(factory: &Arc<Factory>, agent: &str, args: ForwardArgs) -> Result<String, String> {
    use crate::pipes::handoff::{self, Target};
    untainted_admin(factory, agent, "set where a pipe's reports go", "in the pipe's settings")?;
    let (target, project, place) = match (args.pipe.trim(), args.project.trim()) {
        ("", "") => return Err("Say which pipe, or which project's new pipes.".to_owned()),
        ("", wanted) => {
            let inner = factory.lock();
            let project = inner
                .config
                .projects
                .iter()
                .find(|project| project.id == wanted || project.name.eq_ignore_ascii_case(wanted))
                .ok_or_else(|| format!("There's no project \"{wanted}\"."))?;
            (project.id.clone(), project.id.clone(), format!("{}'s new pipes", project.name))
        }
        (wanted, _) => {
            let pipes = factory.pipes.all();
            let record = pipes
                .iter()
                .find(|record| record.id == wanted)
                .or_else(|| pipes.iter().find(|record| record.named_here.unwrap_or(record.peer.is_none()) && record.name.eq_ignore_ascii_case(wanted)))
                .ok_or_else(|| format!("There's no pipe \"{wanted}\" (pipes_list names them)."))?;
            (record.id.clone(), record.project.clone(), format!("the pipe {}", handoff::label(record)))
        }
    };
    let to = args.to.trim();
    let (recipient, name) = if to.is_empty() {
        (String::new(), "no one".to_owned())
    } else {
        let inner = factory.lock();
        let found = inner
            .config
            .agents
            .iter()
            .filter(|candidate| candidate.project.as_deref() == Some(project.as_str()))
            .find(|candidate| candidate.id == to)
            .or_else(|| inner.config.agents.iter().filter(|candidate| candidate.project.as_deref() == Some(project.as_str())).find(|candidate| candidate.name.eq_ignore_ascii_case(to)))
            .ok_or_else(|| format!("There's no agent \"{to}\" in that project."))?;
        (found.id.clone(), found.name.clone())
    };
    let target = if args.pipe.trim().is_empty() { Target::Project(&target) } else { Target::Pipe(&target) };
    let mut made = false;
    if !recipient.is_empty() && !handoff::reader(factory, &recipient) {
        if !args.make_reader {
            return Err(format!(
                "{name} doesn't read reports yet. With make_reader it will: it keeps its database and folder, and can no longer run commands, use the web or secrets, or message agents. Ask the person first if they haven't said so."
            ));
        }
        handoff::make_reader(factory, &recipient, agent, args.fresh).await?;
        made = true;
    }
    let types = match (args.types, args.schema.trim()) {
        (Some(types), _) => Some(types),
        (None, "") => None,
        (None, schema) => Some(vec![schema.to_owned()]),
    };
    handoff::set_forward(factory, target, &recipient, types.as_deref(), None, agent)?;
    let kinds = handoff::forward(factory, target).map(|forward| handoff::type_names(factory, &forward.types)).unwrap_or_default();
    let mut said = format!("Reports from {place} now go to {name}, of the types {kinds}.");
    if made {
        said.push_str(&format!(" {name} now reads reports{}.", if args.fresh { ", from a fresh conversation" } else { "" }));
    }
    said.push_str(" The person sees this in the pipe's settings and in Needs you.");
    Ok(said)
}

fn report_type_parameters() -> serde_json::Value {
    json!({
        "type": "object",
        "required": ["action"],
        "properties": {
            "action": {"type": "string", "enum": ["define", "edit", "delete", "show"]},
            "id": {"type": "string"},
            "name": {"type": "string"},
            "fields": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {
                        "id": {"type": "string"},
                        "label": {"type": "string"},
                        "kind": {"type": "string", "enum": ["text", "text_list", "enum", "bool", "version", "attachments", "number", "date"]},
                        "choices": {"type": "array", "items": {"type": "string"}},
                        "required": {"type": "boolean"},
                        "max_bytes": {"type": "integer"},
                        "max_items": {"type": "integer"},
                        "min": {"type": "integer"},
                        "max": {"type": "integer"},
                        "rule": {"type": "string"},
                    },
                },
            },
            "outcomes": {"type": "array", "items": {"type": "string", "enum": ["accepted", "done", "declined", "known", "fixed", "wont_fix", "needs_info"]}},
        },
        "additionalProperties": false,
    })
}

#[derive(Deserialize)]
struct ReportTypeArgs {
    action: String,
    #[serde(flatten)]
    draft: crate::pipes::handoff::types::Draft,
}

/// pipes_report_type: the admin agent defines, edits, deletes or shows report
/// types, as the person described them.
fn report_type(factory: &Arc<Factory>, agent: &str, args: ReportTypeArgs) -> Result<ToolResult, String> {
    use crate::pipes::handoff::types;
    let id = args.draft.id.trim().to_owned();
    let said = match args.action.as_str() {
        "show" => {
            super::caller(&factory.lock().config, agent)?;
            if id.is_empty() {
                let all: Vec<String> = types::currents(factory).iter().map(|schema| types::described(schema)).collect();
                all.join("\n")
            } else {
                let schema = types::current(factory, &id).ok_or_else(|| format!("There's no report type \"{id}\"."))?;
                types::described(&schema)
            }
        }
        "define" | "edit" => {
            untainted_admin(factory, agent, "define report types", "in Settings → Pipes")?;
            let mut draft = args.draft;
            if args.action == "define" {
                draft.id = String::new();
            } else if id.is_empty() {
                return Err("edit needs id: the type to change.".to_owned());
            }
            let made = types::save(factory, &draft, None, agent)?;
            let widened = if made.widened.is_empty() { String::new() } else { format!(" It lets more cross than before: {}.", made.widened.join(", ")) };
            format!("Kept as version {}.{widened} The person sees it in Needs you's history.\n{}", made.version, types::described(&made))
        }
        "delete" => {
            untainted_admin(factory, agent, "delete report types", "in Settings → Pipes")?;
            types::delete(factory, &id, agent)?;
            format!("Deleted \"{id}\": its reports keep it. The person sees this in Needs you's history.")
        }
        _ => return Err("action is define, edit, delete or show.".to_owned()),
    };
    Ok(ToolResult::text(said, json!({})))
}

fn list_pipes(factory: &Arc<Factory>, _agent: &str, args: ListArgs) -> Result<ToolResult, String> {
    Ok(ToolResult::text(start::list(factory, args.project.as_deref())?, json!({})))
}
