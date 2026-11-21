//! The Agents extension, shown as "Messaging": agents message each other
//! and see who's working. Its id stays "agents" (config keys, tool names).
//!
//! An agent sees and messages only the agents in its own project (agents in
//! the workspace are one group), plus those its `also` list grants, or every
//! agent if it's an admin agent. An agent that isn't `reachable`, or has the
//! extension off, can't be messaged by anyone, so no one sees it. Seeing is
//! exactly being able to message.

use std::fmt::Write as _;
use std::sync::Arc;

use factory_agent::{Event, EventSource, Tool, ToolResult};
use factory_protocol::v1::{self, server_message::Body};
use serde::Deserialize;
use serde_json::json;

use super::{Extension, Manifest, OptionKind, OptionSpec, PromptKind, PromptSection, agent_list, enabled, switch};
use crate::config::{AgentConfig, Config};
use crate::factory::{Factory, Inner};
use crate::transcript::now_ms;

pub(crate) const ID: &str = "agents";

static MANIFEST: Manifest = Manifest {
    id: ID,
    name: "Messaging",
    icon: "users",
    description: "Lets it message other agents and see who's working.",
    default_enabled: true,
    options: &[
        OptionSpec {
            id: "reachable",
            label: "Other agents can message it",
            description: "When off, it can still message others, but no agent can message it or see it.",
            kind: OptionKind::Switch,
            default_on: true,
            includes_project: false,
        },
        OptionSpec {
            id: "admin",
            label: "Admin: can see and message every agent",
            description: "For an agent like Rivet that works across all projects.",
            kind: OptionKind::Switch,
            default_on: false,
            includes_project: false,
        },
        OptionSpec {
            id: "see_map",
            label: "Can see who can message whom",
            description: "Shows which agents can message each other. Admin agents always see it.",
            kind: OptionKind::Switch,
            default_on: false,
            includes_project: false,
        },
        OptionSpec {
            id: "also",
            label: "Can also see and message",
            description: "Agents in other projects it can message. For them to reply, add it to their list too.",
            kind: OptionKind::Agents,
            default_on: false,
            includes_project: true,
        },
    ],
};

const SEND: &str = "Message an agent by id or name, or several at once with a list (up to 20), each getting the same text and told who else got it. It arrives as an event from you and wakes the agent if it's idle; a working agent reads it after its current step.";
/// The most agents one `agents_send` reaches.
const MAX_RECIPIENTS: usize = 20;
const MAP: &str =
    "Who can message whom, one line per agent: whom it messages, and by whom it's messaged. `agent` (an id or name) shows just that agent.";
const STATUS: &str = "Whether agents are idle or working, one line each: every agent you can message, or only `agent`.";
const INSPECT: &str = "How an agent on your team is set up, read only: you, the agents you lead (directly or through others), or, as an admin agent, every agent. `agent` (an id or name) gives its name, role, project and folder, model and effort, lead, extensions with their options, secrets (by name, never values), monitors, prompt and whether it's working; `agent` \"team\", or none, lists your team, one line each.";
const SUGGEST: &str = "Suggest that the person talk to another agent (`agent`, by id or name, one you can message) themselves: they see a card in your conversation, with `reason`, and a button that opens that agent's conversation with `message` in its message box, drafted and not sent, for them to change or send. Write `message` as the person would send it, in their voice, saying everything that agent needs; `reason` is one line on why they should talk to it.";
/// The prompt's line on suggesting a talk. `{person}`: the user's name, or
/// "the person".
const SUGGEST_PROMPT: &str = "When {person} should give another agent something or settle something with it directly (secrets it will ask them for, a decision that's theirs), rather than you passing it on, suggest it with agents_suggest_talk: they get a button that opens that agent with your message drafted for them.\n";
/// `{person}`: the user's name, or "the person".
const PROPOSE: &str = "Rivet, the admin agent, can't be messaged: only {person} talks to it. When you reach a conclusion Rivet should act on (another agent is needed, say), propose a message with propose_to_rivet, written to stand alone; {person} approves or dismisses it, and you're told which.\n";

pub(crate) struct Agents;

/// Whether `from` may see and message `to`. No agent can message one in
/// an archived project, and one there messages no one (it doesn't run).
pub(crate) fn can_message(config: &Config, from: &AgentConfig, to: &AgentConfig) -> bool {
    if from.id == to.id || !enabled(from, ID) || !enabled(to, ID) || !switch(to, ID, "reachable") {
        return false;
    }
    if config.archived(from) || config.archived(to) {
        return false;
    }
    switch(from, ID, "admin") || from.project == to.project || agent_list(from, ID, "also").contains(&to.id)
}

/// The agents `from` can message, in the settings' order.
fn reachable<'a>(config: &'a Config, from: &AgentConfig) -> Vec<&'a AgentConfig> {
    config.agents.iter().filter(|to| can_message(config, from, to)).collect()
}

/// Whom `from` can message, by id, as its prompt names them; empty with the
/// extension off.
pub(crate) fn roster(config: &Config, from: &AgentConfig) -> std::collections::BTreeMap<String, String> {
    if !enabled(from, ID) {
        return Default::default();
    }
    reachable(config, from).into_iter().map(|to| (to.id.clone(), named(config, from, to))).collect()
}

/// How others see where `agent` is: its project's name, "workspace", or
/// "admin" for an admin agent, wherever it is.
fn project_name<'a>(config: &'a Config, agent: &AgentConfig) -> &'a str {
    if switch(agent, ID, "admin") {
        return "admin";
    }
    agent.project.as_deref().and_then(|project| config.project(project)).map_or("workspace", |project| project.name.as_str())
}

/// Where `agent` is, as `from` needs to know it: None in `from`'s own
/// project, or when its role already says so ("Admin" for an admin agent).
fn place<'a>(config: &'a Config, from: &AgentConfig, agent: &AgentConfig) -> Option<&'a str> {
    let place = project_name(config, agent);
    (agent.project != from.project && !agent.role.eq_ignore_ascii_case(place)).then_some(place)
}

/// "Mike (Coordinator)": its name, with what's known of it in parentheses,
/// its role and then `rest`; just "Mike" with neither.
fn with_role(agent: &AgentConfig, rest: Option<&str>) -> String {
    let parts: Vec<&str> = [Some(agent.role.as_str()).filter(|role| !role.is_empty()), rest].into_iter().flatten().collect();
    if parts.is_empty() { agent.name.clone() } else { format!("{} ({})", agent.name, parts.join(", ")) }
}

/// "Researcher (researcher)", with its role and with its project when it
/// isn't `from`'s: "Mike (mike, Coordinator)", "Researcher (researcher, in
/// Factory Research)", or "Rivet (rivet, Admin)".
fn named(config: &Config, from: &AgentConfig, agent: &AgentConfig) -> String {
    let mut text = format!("{} ({}", agent.name, agent.id);
    if !agent.role.is_empty() {
        let _ = write!(text, ", {}", agent.role);
    }
    match place(config, from, agent) {
        Some("admin") => text.push_str(", admin"),
        Some(place) => {
            let _ = write!(text, ", in {place}");
        }
        None => {}
    }
    text.push(')');
    text
}

/// "scout", or "scout (Lab)" when it's in another project than `from`
/// ("rivet (admin)" for an admin agent).
fn labeled(config: &Config, from: &AgentConfig, agent: &AgentConfig) -> String {
    if agent.project == from.project { agent.id.clone() } else { format!("{} ({})", agent.id, project_name(config, agent)) }
}

/// "Mike (Coordinator)", with its project when it's in another project than
/// `to` ("Ana (Frontend, Web Site)", "Frontend (Web Site)"), or "Rivet
/// (admin)" for an admin agent there: how a message from `agent` is signed,
/// so a reply finds it.
pub(crate) fn signed(config: &Config, agent: &AgentConfig, to: &AgentConfig) -> String {
    with_role(agent, place(config, to, agent))
}

/// The agent among `candidates` that `name` means: its id, its name, its
/// name as messages are signed ("Mike (Coordinator)", "Frontend (Web
/// Site)"), or else its role ("Coordinator"). A name or role several share
/// means the one in `from`'s project, if just one is.
pub(crate) fn find<'a>(
    config: &Config,
    from: &AgentConfig,
    candidates: &[&'a AgentConfig],
    name: &str,
) -> Result<Option<&'a AgentConfig>, String> {
    let name = name.trim();
    if let Some(agent) = candidates.iter().find(|agent| agent.id == name) {
        return Ok(Some(agent));
    }
    let mut named: Vec<&'a AgentConfig> = candidates.iter().copied().filter(|agent| agent.name.eq_ignore_ascii_case(name)).collect();
    if named.is_empty() {
        named = candidates
            .iter()
            .copied()
            .filter(|agent| {
                let project = Some(project_name(config, agent));
                signed(config, agent, from).eq_ignore_ascii_case(name)
                    || with_role(agent, None).eq_ignore_ascii_case(name)
                    || with_role(agent, project).eq_ignore_ascii_case(name)
                    || format!("{} ({})", agent.name, project_name(config, agent)).eq_ignore_ascii_case(name)
            })
            .collect();
    }
    if named.is_empty() {
        named = candidates.iter().copied().filter(|agent| !agent.role.is_empty() && agent.role.eq_ignore_ascii_case(name)).collect();
    }
    if named.len() > 1 {
        let own: Vec<&'a AgentConfig> = named.iter().copied().filter(|agent| agent.project == from.project).collect();
        if own.len() == 1 {
            named = own;
        }
    }
    match named.as_slice() {
        [] => Ok(None),
        [agent] => Ok(Some(agent)),
        several => {
            let ids: Vec<String> = several.iter().map(|agent| format!("{} ({})", agent.id, project_name(config, agent))).collect();
            Err(format!("Several agents are called \"{name}\"; use an id: {}.", ids.join(", ")))
        }
    }
}

impl Extension for Agents {
    fn manifest(&self) -> &'static Manifest {
        &MANIFEST
    }

    fn prompt(&self, _factory: &Factory, inner: &Inner, agent: &AgentConfig) -> Vec<PromptSection> {
        let config = &inner.config;
        let mut text = String::from(
            "## Other agents\n\nMessage agents with agents_send, by id or name (a list sends the same message to several), and see who's idle or working with agents_status. Messages from agents arrive as events from them. Say everything the agent needs in the message, since it can't see your conversation. Don't send messages only to acknowledge or thank.\n\n",
        );
        let others = reachable(config, agent);
        if others.is_empty() {
            text.push_str("No agent can be messaged from here for now.\n");
        } else {
            let names: Vec<String> = others.iter().map(|other| named(config, agent, other)).collect();
            let _ = writeln!(text, "You can message: {}.", names.join(", "));
            text.push_str(&SUGGEST_PROMPT.replace("{person}", config.person()));
        }
        if let Some(lead) = config.lead(agent) {
            let _ = writeln!(text, "You report to {}, which can see how you're set up.", named(config, agent, lead));
        }
        if crate::proposals::is_admin(agent) {
            text.push_str("You lead every agent: agents_inspect shows how any of them is set up, read only.\n");
        } else {
            let team: Vec<String> = led(config, agent).into_iter().map(|other| named(config, agent, other)).collect();
            if !team.is_empty() {
                let _ = writeln!(text, "You lead {}: agents_inspect shows how they're set up, read only.", team.join(", "));
            }
        }
        if sees_map(agent) {
            text.push_str(
                "agents_map shows who can message whom across Claktory, for when you set up or check how agents work together.\n",
            );
        }
        if !crate::proposals::is_admin(agent) {
            text.push_str(&PROPOSE.replace("{person}", config.person()));
        }
        vec![PromptSection { kind: PromptKind::Static, text }]
    }

    fn tools(&self, factory: &Arc<Factory>, agent: &str) -> Vec<Tool> {
        let mut tools =
            vec![send_tool(factory, agent), status_tool(factory, agent), suggest_tool(factory, agent), inspect_tool(factory, agent)];
        let (map, propose) = match factory.lock().config.agent(agent) {
            Some(me) => (sees_map(me), !crate::proposals::is_admin(me)),
            None => (false, false),
        };
        if map {
            tools.push(map_tool(factory, agent));
        }
        if propose {
            tools.push(crate::proposals::tool(factory, agent));
        }
        tools
    }

    fn owns_tool(&self, name: &str) -> bool {
        name == crate::proposals::TOOL || name.strip_prefix(ID).is_some_and(|verb| verb.starts_with('_'))
    }

    /// Whom it can message and who can message it. The view is resent with
    /// every settings change, which is all that changes either.
    fn view(&self, _factory: &Factory, inner: &Inner, agent: &str) -> Option<v1::ExtensionView> {
        let config = &inner.config;
        let me = config.agent(agent)?;
        let reaches = reachable(config, me).into_iter().map(|to| to.id.clone()).collect();
        let reached_by = config.agents.iter().filter(|from| can_message(config, from, me)).map(|from| from.id.clone()).collect();
        Some(v1::ExtensionView {
            extension_id: ID.to_owned(),
            agent_id: agent.to_owned(),
            title: MANIFEST.name.to_owned(),
            body: Some(v1::extension_view::Body::Messaging(v1::Messaging { reaches, reached_by })),
        })
    }
}

/// Whether `agent` may see who can message whom: with `see_map`, or as an admin.
fn sees_map(agent: &AgentConfig) -> bool {
    switch(agent, ID, "see_map") || switch(agent, ID, "admin")
}

/// "builder Builder (Reviewer, Site): messages writer, scout (Lab); messaged by rivet, writer"
fn map_line(config: &Config, asker: &AgentConfig, agent: &AgentConfig) -> String {
    let list = |agents: Vec<&AgentConfig>| {
        if agents.is_empty() {
            "no one".to_owned()
        } else {
            agents.iter().map(|other| labeled(config, asker, other)).collect::<Vec<_>>().join(", ")
        }
    };
    let messages = list(reachable(config, agent));
    let messaged_by = list(config.agents.iter().filter(|from| can_message(config, from, agent)).collect());
    let mut line = agent.id.clone();
    let label = with_role(agent, place(config, asker, agent));
    if label != agent.id {
        let _ = write!(line, " {label}");
    }
    let _ = write!(line, ": messages {messages}; messaged by {messaged_by}");
    line
}

/// The map `asker` asked for: every agent's line, or `agent`'s.
fn map(factory: &Factory, asker: &str, agent: Option<&str>) -> Result<String, String> {
    let inner = factory.lock();
    let config = &inner.config;
    let asker = super::caller(config, asker)?;
    let Some(name) = agent.map(str::trim).filter(|name| !name.is_empty()) else {
        let lines: Vec<String> = config.agents.iter().map(|agent| map_line(config, asker, agent)).collect();
        return Ok(lines.join("\n"));
    };
    let everyone: Vec<&AgentConfig> = config.agents.iter().collect();
    match find(config, asker, &everyone, name)? {
        Some(agent) => Ok(map_line(config, asker, agent)),
        None => Err(format!("No agent has the id or name \"{name}\".")),
    }
}

fn map_tool(factory: &Arc<Factory>, asker: &str) -> Tool {
    let parameters = json!({
        "type": "object",
        "properties": {"agent": {"type": "string"}},
        "additionalProperties": false,
    });
    super::tool(
        "agents_map",
        "Who messages whom",
        MAP,
        parameters,
        super::typed_handler(factory, asker, |factory, asker, args: StatusArgs| {
            let lines = map(factory, asker, args.agent.as_deref())?;
            Ok(ToolResult::text(lines, json!({})))
        }),
    )
}

#[derive(Deserialize)]
struct SendArgs {
    to: To,
    text: String,
}

/// One recipient, as before, or several.
#[derive(Deserialize)]
#[serde(untagged)]
enum To {
    One(String),
    Several(Vec<String>),
}

#[derive(Deserialize)]
struct SuggestArgs {
    agent: String,
    message: String,
    reason: String,
}

#[derive(Deserialize)]
struct StatusArgs {
    #[serde(default)]
    agent: Option<String>,
}

fn send_tool(factory: &Arc<Factory>, sender: &str) -> Tool {
    let parameters = json!({
        "type": "object",
        "required": ["to", "text"],
        "properties": {
            "to": {"anyOf": [
                {"type": "string"},
                {"type": "array", "items": {"type": "string"}, "minItems": 1, "maxItems": MAX_RECIPIENTS},
            ]},
            "text": {"type": "string", "minLength": 1},
        },
        "additionalProperties": false,
    });
    super::tool(
        "agents_send",
        "Message agent",
        SEND,
        parameters,
        super::typed_handler(factory, sender, |factory, sender, args: SendArgs| match args.to {
            // An agent whose context began before lists were allowed has a
            // schema that says `to` is a string, so it writes the list in one.
            To::One(to) if let Some(several) = listed(&to) => send_to_several(factory, sender, &several, args.text),
            To::One(to) => send(factory, sender, &to, args.text),
            To::Several(to) => send_to_several(factory, sender, &to, args.text),
        }),
    )
}

/// The names in `to` when it's a JSON list written as a string.
fn listed(to: &str) -> Option<Vec<String>> {
    let to = to.trim();
    if !to.starts_with('[') {
        return None;
    }
    serde_json::from_str(to).ok()
}

fn status_tool(factory: &Arc<Factory>, asker: &str) -> Tool {
    let parameters = json!({
        "type": "object",
        "properties": {"agent": {"type": "string"}},
        "additionalProperties": false,
    });
    super::tool(
        "agents_status",
        "Agent status",
        STATUS,
        parameters,
        super::typed_handler(factory, asker, |factory, asker, args: StatusArgs| {
            let lines = status(factory, asker, args.agent.as_deref().filter(|agent| !agent.trim().is_empty()))?;
            Ok(ToolResult::text(lines, json!({})))
        }),
    )
}

fn suggest_tool(factory: &Arc<Factory>, proposer: &str) -> Tool {
    let parameters = json!({
        "type": "object",
        "required": ["agent", "message", "reason"],
        "properties": {
            "agent": {"type": "string"},
            "message": {"type": "string", "minLength": 1},
            "reason": {"type": "string", "minLength": 1},
        },
        "additionalProperties": false,
    });
    super::tool(
        "agents_suggest_talk",
        "Suggest talking to an agent",
        SUGGEST,
        parameters,
        super::typed_handler(factory, proposer, |factory, proposer, args: SuggestArgs| suggest(factory, proposer, args)),
    )
}

/// Shows the person a card suggesting they talk to another agent, with a
/// message drafted for them; nothing is sent. Its details: the agent
/// (`agent_id`, `agent_name`, `agent_role`), `message` and `reason`.
fn suggest(factory: &Factory, proposer: &str, args: SuggestArgs) -> Result<ToolResult, String> {
    let (message, reason) = (args.message.trim().to_owned(), args.reason.trim().to_owned());
    if message.is_empty() {
        return Err("Write the message they'd send in `message`.".to_owned());
    }
    if reason.is_empty() {
        return Err("Say in `reason` why they should talk to it.".to_owned());
    }
    let inner = factory.lock();
    let config = &inner.config;
    let from = super::caller(config, proposer)?;
    let agent = resolve(config, from, &args.agent)?;
    let said = format!(
        "Suggested. {} sees a card with a button that opens {}'s conversation with your message drafted; nothing is sent until they send it.",
        config.person_capitalized(),
        agent.name
    );
    Ok(ToolResult::text(
        said,
        json!({"agent_id": agent.id, "agent_name": agent.name, "agent_role": agent.role, "message": message, "reason": reason}),
    ))
}

/// The agent `from` can message called `to` (an id or a name), or why not.
fn resolve<'a>(config: &'a Config, from: &AgentConfig, to: &str) -> Result<&'a AgentConfig, String> {
    let to = to.trim();
    let candidates = reachable(config, from);
    if let Some(agent) = find(config, from, &candidates, to)? {
        return Ok(agent);
    }
    if to == from.id || to.eq_ignore_ascii_case(&from.name) {
        return Err("That's you.".to_owned());
    }
    if candidates.is_empty() {
        return Err(format!("No agent you can message is called \"{to}\"; you can't message any agent."));
    }
    let ids: Vec<String> = candidates.iter().map(|agent| labeled(config, from, agent)).collect();
    Err(format!("No agent you can message is called \"{to}\"; you can message: {}.", ids.join(", ")))
}

/// Delivers one agent's message to another; says what happened, or why not.
/// Its details: whom it went to (`to`, its id, `name` and `role`), and whether it
/// was `working`, so it reads the message after its current step; and, as for
/// several, `text` and `recipients`.
pub(crate) fn send(factory: &Arc<Factory>, sender: &str, to: &str, text: String) -> Result<ToolResult, String> {
    let target = {
        let inner = factory.lock();
        let config = &inner.config;
        let from = super::caller(config, sender)?;
        resolve(config, from, to)?.id.clone()
    };
    let delivered = deliver(factory, sender, &[target], &text);
    let Some(recipient) = delivered.first() else { return Err("That agent is no longer in this Claktory.".to_owned()) };
    let working = recipient.working;
    let said = if working { "Sent; it's working and reads it after its current step." } else { "Sent." };
    let (target, name, role) = (recipient.id.clone(), recipient.name.clone(), recipient.role.clone());
    let recipients = vec![recipient.details(to)];
    Ok(ToolResult::text(
        said,
        json!({"to": target, "name": name, "role": role, "working": working, "text": text, "recipients": recipients}),
    ))
}

/// Delivers one agent's message to several, each told who else got it; says
/// what happened for each. The sender and repeats are left out, and one that
/// can't be messaged doesn't stop the others; with none it fails. Its details:
/// `text` and `recipients`, each with `to` as written, its `id`, `name` and
/// `role`, the `outcome` ("sent", "working": sent, read after its current
/// step, or "failed") and, failed, `why`.
fn send_to_several(factory: &Arc<Factory>, sender: &str, names: &[String], text: String) -> Result<ToolResult, String> {
    if names.is_empty() {
        return Err("Name at least one agent in `to`.".to_owned());
    }
    if names.len() > MAX_RECIPIENTS {
        return Err(format!("Send to at most {MAX_RECIPIENTS} agents at once."));
    }
    // Each name: the agent it means, or why not.
    let mut resolved: Vec<(&str, Result<String, String>)> = Vec::new();
    {
        let inner = factory.lock();
        let config = &inner.config;
        let from = super::caller(config, sender)?;
        for name in names {
            let name = name.as_str();
            if name.trim() == from.id || name.trim().eq_ignore_ascii_case(&from.name) {
                continue;
            }
            let found = resolve(config, from, name).map(|agent| agent.id.clone());
            if let Ok(id) = &found
                && resolved.iter().any(|(_, other)| other.as_ref().is_ok_and(|other| other == id))
            {
                continue;
            }
            resolved.push((name, found));
        }
    }
    let targets: Vec<String> = resolved.iter().filter_map(|(_, found)| found.as_ref().ok().cloned()).collect();
    if targets.is_empty() {
        let reasons: Vec<String> = resolved.iter().filter_map(|(_, found)| found.as_ref().err().cloned()).collect();
        return Err(if reasons.is_empty() { "That's only you.".to_owned() } else { format!("Sent to no one. {}", reasons.join(" ")) });
    }
    let delivered = deliver(factory, sender, &targets, &text);
    let mut lines = Vec::new();
    let mut recipients = Vec::new();
    for (name, found) in &resolved {
        let reached = found.as_ref().ok().and_then(|id| delivered.iter().find(|recipient| &recipient.id == id));
        match (found, reached) {
            (_, Some(recipient)) => {
                let said = if recipient.working { "sent; it's working and reads it after its current step" } else { "sent" };
                lines.push(format!("{}: {said}.", recipient.label));
                recipients.push(recipient.details(name));
            }
            (Ok(_), None) => {
                let why = "That agent is no longer in this Claktory.";
                lines.push(format!("{name}: {why}"));
                recipients.push(json!({"to": name, "id": "", "name": "", "role": "", "outcome": "failed", "why": why}));
            }
            (Err(why), None) => {
                lines.push(format!("{name}: {why}"));
                recipients.push(json!({"to": name, "id": "", "name": "", "role": "", "outcome": "failed", "why": why}));
            }
        }
    }
    Ok(ToolResult::text(lines.join("\n"), json!({"text": text, "recipients": recipients})))
}

/// An agent a message reached.
struct Recipient {
    id: String,
    name: String,
    role: String,
    /// "Lena (backend-2)": how the sender's result names it.
    label: String,
    /// It was working, so it reads the message after its current step.
    working: bool,
}

impl Recipient {
    fn details(&self, to: &str) -> serde_json::Value {
        let outcome = if self.working { "working" } else { "sent" };
        json!({"to": to, "id": self.id, "name": self.name, "role": self.role, "outcome": outcome})
    }
}

/// Delivers `text` from `sender` to each of `targets` (ids), each told who
/// else got it when there are several, and lights up their rows; the agents
/// it reached, in order.
fn deliver(factory: &Arc<Factory>, sender: &str, targets: &[String], text: &str) -> Vec<Recipient> {
    let mut letters = Vec::new();
    let mut reached = Vec::new();
    {
        let inner = factory.lock();
        let config = &inner.config;
        let Some(from) = config.agent(sender) else { return reached };
        let agents: Vec<&AgentConfig> = targets.iter().filter_map(|id| config.agent(id)).collect();
        for agent in &agents {
            let mut text = text.to_owned();
            let others: Vec<String> =
                agents.iter().filter(|other| other.id != agent.id).map(|other| signed(config, other, agent)).collect();
            if !others.is_empty() {
                let _ = write!(text, "\n\nAlso sent to: {}.", others.join(", "));
            }
            // Read a pipe: those it messages know it.
            let mut signature = signed(config, from, agent);
            if !factory.pipes.tainted(sender).is_empty() {
                signature.push_str(crate::pipes::agents::TAINT_LABEL);
            }
            let event =
                Event { source: EventSource::Agent, from: signature, id: sender.to_owned(), summary: String::new(), transient: false };
            letters.push((agent.id.clone(), text, event));
            reached.push(Recipient {
                id: agent.id.clone(),
                name: agent.name.clone(),
                role: agent.role.clone(),
                label: format!("{} ({})", agent.name, agent.id),
                working: inner.agents.get(&agent.id).is_some_and(|runtime| runtime.running),
            });
        }
    }
    for (id, text, event) in letters {
        factory.deliver_event(&id, text, event);
    }
    // The person sees who talks to whom: the rows glow.
    let row = |id: &str| v1::HighlightTarget { kind: "agent".to_owned(), id: id.to_owned() };
    let mut rows = vec![row(sender)];
    rows.extend(reached.iter().map(|recipient| row(&recipient.id)));
    factory.send(Body::Highlight(v1::Highlight {
        targets: rows,
        text: String::new(),
        from_agent_id: sender.to_owned(),
        kind: v1::HighlightKind::Message.into(),
    }));
    reached
}

/// "12m", "3h", "2d": how long ago `since` was.
pub(crate) fn ago(since: i64, now: i64) -> String {
    let minutes = (now - since).max(0) / 60_000;
    match minutes {
        0 => "<1m".to_owned(),
        1..60 => format!("{minutes}m"),
        60..2880 => format!("{}h", minutes / 60),
        _ => format!("{}d", minutes / 1440),
    }
}

/// One line per agent `asker` can message, or for `only`.
pub(crate) fn status(factory: &Factory, asker: &str, only: Option<&str>) -> Result<String, String> {
    let times = factory.store.run_times().map_err(|error| format!("Couldn't read runs: {error}"))?;
    let inner = factory.lock();
    let config = &inner.config;
    let from = super::caller(config, asker)?;
    let agents = match only {
        Some(only) => vec![resolve(config, from, only)?],
        None => reachable(config, from),
    };
    if agents.is_empty() {
        return Ok("No agent can be messaged from here.".to_owned());
    }
    let now = now_ms();
    let mut lines = Vec::new();
    for agent in agents {
        let mut line = format!("{} {}: {}", agent.id, with_role(agent, place(config, from, agent)), state(&inner, &times, agent, now));
        if let Some(lead) = config.lead(agent) {
            let _ = write!(line, "; led by {}", labeled(config, from, lead));
        }
        lines.push(line);
    }
    Ok(lines.join("\n"))
}

/// "working 3m, running bash; 2 queued", "idle 12m; no model": what the
/// agent is doing now.
fn state(inner: &Inner, times: &crate::store::RunTimes, agent: &AgentConfig, now: i64) -> String {
    let runtime = inner.agents.get(&agent.id);
    let (started, ended) = times.get(&agent.id).copied().unwrap_or_default();
    let mut line = String::new();
    if runtime.is_some_and(|runtime| runtime.running) {
        line.push_str("working");
        if let Some(started) = started {
            let _ = write!(line, " {}", ago(started, now));
        }
        if let Some(tool) = runtime.and_then(|runtime| runtime.doing.as_deref()) {
            let _ = write!(line, ", running {tool}");
        }
    } else {
        line.push_str("idle");
        if let Some(ended) = ended {
            let _ = write!(line, " {}", ago(ended, now));
        }
    }
    let queued = runtime.map_or(0, |runtime| runtime.queue.len());
    if queued > 0 {
        let _ = write!(line, "; {queued} queued");
    }
    if !inner.models.iter().any(|model| model.id == agent.model) {
        line.push_str("; no model");
    }
    line
}

/// The agents `lead` leads, directly or through others (every other agent
/// for an admin agent), in the settings' order.
fn led<'a>(config: &'a Config, lead: &AgentConfig) -> Vec<&'a AgentConfig> {
    config.agents.iter().filter(|agent| config.leads(lead, agent)).collect()
}

fn inspect_tool(factory: &Arc<Factory>, asker: &str) -> Tool {
    let parameters = json!({
        "type": "object",
        "properties": {"agent": {"type": "string"}},
        "additionalProperties": false,
    });
    super::tool(
        "agents_inspect",
        "Inspect an agent",
        INSPECT,
        parameters,
        super::typed_handler(factory, asker, |factory, asker, args: StatusArgs| {
            let wanted = args.agent.as_deref().map(str::trim).filter(|agent| !agent.is_empty() && !agent.eq_ignore_ascii_case("team"));
            match wanted {
                None => team_lines(factory, asker),
                Some(wanted) => inspect(factory, asker, wanted),
            }
        }),
    )
}

/// `asker`'s team, one line each: itself, then those it leads.
fn team_lines(factory: &Factory, asker: &str) -> Result<ToolResult, String> {
    let times = factory.store.run_times().map_err(|error| format!("Couldn't read runs: {error}"))?;
    let inner = factory.lock();
    let config = &inner.config;
    let me = super::caller(config, asker)?;
    let now = now_ms();
    let team: Vec<&AgentConfig> = std::iter::once(me).chain(led(config, me)).collect();
    let mut lines = Vec::new();
    for agent in &team {
        let mut line = format!("{} {}", agent.id, with_role(agent, place(config, me, agent)));
        if agent.id == me.id {
            line.push_str(" (you)");
        }
        let _ = write!(line, ": {} {}", agent.model, agent.effort);
        if let Some(lead) = config.lead(agent) {
            let _ = write!(line, "; led by {}", labeled(config, me, lead));
        }
        let _ = write!(line, "; {}", state(&inner, &times, agent, now));
        lines.push(line);
    }
    if team.len() == 1 {
        lines.push("No agent reports to you, so you can inspect only yourself.".to_owned());
    }
    let ids: Vec<&str> = team.iter().map(|agent| agent.id.as_str()).collect();
    Ok(ToolResult::text(lines.join("\n"), json!({"team": ids})))
}

/// How `wanted` (an id or name on `asker`'s team) is set up, read only.
fn inspect(factory: &Factory, asker: &str, wanted: &str) -> Result<ToolResult, String> {
    let times = factory.store.run_times().map_err(|error| format!("Couldn't read runs: {error}"))?;
    let (id, mut text) = {
        let inner = factory.lock();
        let config = &inner.config;
        let me = super::caller(config, asker)?;
        let team: Vec<&AgentConfig> = std::iter::once(me).chain(led(config, me)).collect();
        let agent = match find(config, me, &team, wanted)? {
            Some(agent) => agent,
            None => {
                let everyone: Vec<&AgentConfig> = config.agents.iter().collect();
                return Err(match find(config, me, &everyone, wanted)? {
                    Some(agent) => format!(
                        "{} isn't on your team: you can see how an agent is set up only when you lead it, directly or through others.",
                        agent.name
                    ),
                    None => format!("No agent has the id or name \"{wanted}\"."),
                });
            }
        };
        (agent.id.clone(), setup(factory, &inner, &times, me, agent))
    };
    text.push_str("\nSecrets:");
    if factory.vault.exists() && !factory.vault.is_open() {
        text.push_str(" can't be read while the vault is locked");
    } else {
        // "Name (sets A, B): description", never a value.
        let secrets = factory.vault.granted(&id);
        if secrets.is_empty() {
            text.push_str(" none");
        }
        for label in secrets.values() {
            let _ = write!(text, "\n- {label}");
        }
    }
    text.push_str("\nMonitors:");
    let monitors = super::monitors::lines(factory, &id)?;
    if monitors.is_empty() {
        text.push_str(" none");
    }
    for line in monitors {
        let _ = write!(text, "\n- {line}");
    }
    let prompt = factory.lock().config.agent(&id).map(|agent| agent.prompt.clone()).unwrap_or_default();
    if prompt.is_empty() {
        text.push_str("\nPrompt: none");
    } else {
        let _ = write!(text, "\nPrompt:\n{prompt}");
    }
    Ok(ToolResult::text(text, json!({"agent": id})))
}

/// The first part of what agents_inspect says of `agent`: who and where it
/// is, its model, lead, state and extensions.
fn setup(factory: &Factory, inner: &Inner, times: &crate::store::RunTimes, me: &AgentConfig, agent: &AgentConfig) -> String {
    let config = &inner.config;
    let mut text = format!("{} ({})", agent.name, agent.id);
    if !agent.role.is_empty() {
        let _ = write!(text, ", {}", agent.role);
    }
    if agent.id == me.id {
        text.push_str(" (you)");
    }
    let project = agent.project.as_deref().and_then(|project| config.project(project));
    let folder = crate::runner::places(&factory.home, config, agent, &[]).folder;
    match (project, config.agent_folder(agent)) {
        (Some(project), Some(named)) => {
            let _ = write!(text, "\nProject: {}, folder {} ({})", project.name, named.name, folder.display());
        }
        (Some(project), None) => {
            let _ = write!(text, "\nProject: {} ({})", project.name, folder.display());
        }
        (None, _) => {
            let _ = write!(text, "\nProject: none, the workspace ({})", folder.display());
        }
    }
    if config.archived(agent) {
        text.push_str(", archived");
    }
    let _ = write!(text, "\nModel: {}, effort {}", agent.model, agent.effort);
    match config.lead(agent) {
        Some(lead) => {
            let _ = write!(text, "\nLead: {}", named(config, me, lead));
        }
        None => text.push_str("\nLead: none"),
    }
    if crate::proposals::is_admin(agent) {
        text.push_str(" (an admin agent: it leads every agent)");
    }
    let _ = write!(text, "\nNow: {}", state(inner, times, agent, now_ms()));
    text.push_str("\nExtensions:");
    for unit in super::units() {
        let on = enabled(agent, unit.key());
        let _ = write!(text, "\n- {} ({}): {}", unit.name(), unit.key(), if on { "on" } else { "off" });
        if unit.instance.is_some() {
            continue;
        }
        let manifest = unit.extension.manifest();
        let options: Vec<String> = manifest
            .options
            .iter()
            .map(|spec| match spec.kind {
                super::OptionKind::Switch => format!("{} {}", spec.id, if switch(agent, manifest.id, spec.id) { "on" } else { "off" }),
                super::OptionKind::Agents => {
                    let ids: Vec<String> =
                        agent_list(agent, manifest.id, spec.id).into_iter().filter(|id| config.agent(id).is_some()).collect();
                    format!("{} [{}]", spec.id, ids.join(", "))
                }
            })
            .collect();
        if !options.is_empty() {
            let _ = write!(text, "; {}", options.join(", "));
        }
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn durations_are_short() {
        let now = 10_000_000_000;
        assert_eq!(ago(now - 30_000, now), "<1m");
        assert_eq!(ago(now - 12 * 60_000, now), "12m");
        assert_eq!(ago(now - 3 * 3_600_000, now), "3h");
        assert_eq!(ago(now - 47 * 3_600_000, now), "47h");
        assert_eq!(ago(now - 3 * 86_400_000, now), "3d");
    }
}
