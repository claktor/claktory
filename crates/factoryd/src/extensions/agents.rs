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
    description: "Message other agents and see whether they're idle or working.",
    default_enabled: true,
    options: &[
        OptionSpec {
            id: "reachable",
            label: "Other agents can message it",
            description: "Off: it can still message others, but no agent can message it or see it.",
            kind: OptionKind::Switch,
            default_on: true,
            includes_project: false,
        },
        OptionSpec {
            id: "admin",
            label: "Admin: can see and message every agent",
            description: "For an admin agent like Rivet, which works across every project.",
            kind: OptionKind::Switch,
            default_on: false,
            includes_project: false,
        },
        OptionSpec {
            id: "see_map",
            label: "Can see who can message whom",
            description: "Gives it agents_map, which shows who every agent can message. Admin agents have it anyway.",
            kind: OptionKind::Switch,
            default_on: false,
            includes_project: false,
        },
        OptionSpec {
            id: "also",
            label: "Can also see and message",
            description: "Agents outside its project it may message. They can't message it back unless they're given it too.",
            kind: OptionKind::Agents,
            default_on: false,
            includes_project: true,
        },
    ],
};

const SEND: &str = "Message an agent by id or name. It arrives as an event from you and wakes the agent if it's idle; a working agent reads it after its current step.";
const MAP: &str =
    "Who can message whom, one line per agent: whom it messages, and by whom it's messaged. `agent` (an id or name) shows just that agent.";
const STATUS: &str = "Whether agents are idle or working, one line each: every agent you can message, or only `agent`.";
const PROPOSE: &str = "Rivet, the admin agent, can't be messaged: only the person talks to it. When you reach a conclusion Rivet should act on (another agent is needed, say), propose a message with propose_to_rivet, written to stand alone; the person approves or dismisses it, and you're told which.\n";

pub(crate) struct Agents;

/// Whether `from` may see and message `to`.
pub(crate) fn can_message(from: &AgentConfig, to: &AgentConfig) -> bool {
    if from.id == to.id || !enabled(from, ID) || !enabled(to, ID) || !switch(to, ID, "reachable") {
        return false;
    }
    switch(from, ID, "admin") || from.project == to.project || agent_list(from, ID, "also").contains(&to.id)
}

/// The agents `from` can message, in the settings' order.
fn reachable<'a>(config: &'a Config, from: &AgentConfig) -> Vec<&'a AgentConfig> {
    config.agents.iter().filter(|to| can_message(from, to)).collect()
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
            "## Other agents\n\nMessage agents with agents_send, by id or name, and see who's idle or working with agents_status. Messages from agents arrive as events from them. Say everything the agent needs in the message, since it can't see your conversation. Don't send messages only to acknowledge or thank.\n\n",
        );
        let others = reachable(config, agent);
        if others.is_empty() {
            text.push_str("No agent can be messaged from here for now.\n");
        } else {
            let names: Vec<String> = others.iter().map(|other| named(config, agent, other)).collect();
            let _ = writeln!(text, "You can message: {}.", names.join(", "));
        }
        if sees_map(agent) {
            text.push_str("agents_map shows who can message whom across Factory, for when you set up or check how agents work together.\n");
        }
        if !crate::proposals::is_admin(agent) {
            text.push_str(PROPOSE);
        }
        vec![PromptSection { kind: PromptKind::Static, text }]
    }

    fn tools(&self, factory: &Arc<Factory>, agent: &str) -> Vec<Tool> {
        let mut tools = vec![send_tool(factory, agent), status_tool(factory, agent)];
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
        let reached_by = config.agents.iter().filter(|from| can_message(from, me)).map(|from| from.id.clone()).collect();
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
    let messaged_by = list(config.agents.iter().filter(|from| can_message(from, agent)).collect());
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
    let Some(asker) = config.agent(asker) else { return Err("You're no longer in this Factory.".to_owned()) };
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
    let factory = Arc::downgrade(factory);
    let asker = asker.to_owned();
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
        Arc::new(move |args| {
            let factory = factory.upgrade();
            let asker = asker.clone();
            Box::pin(async move {
                let args: StatusArgs = serde_json::from_value(args)?;
                let Some(factory) = factory else { return Err(factory_agent::Error::Other("Factory is stopping.".into())) };
                let lines = map(&factory, &asker, args.agent.as_deref()).map_err(factory_agent::Error::Other)?;
                Ok(ToolResult::text(lines, json!({})))
            })
        }),
    )
}

#[derive(Deserialize)]
struct SendArgs {
    to: String,
    text: String,
}

#[derive(Deserialize)]
struct StatusArgs {
    #[serde(default)]
    agent: Option<String>,
}

fn send_tool(factory: &Arc<Factory>, sender: &str) -> Tool {
    let factory = Arc::downgrade(factory);
    let sender = sender.to_owned();
    let parameters = json!({
        "type": "object",
        "required": ["to", "text"],
        "properties": {"to": {"type": "string"}, "text": {"type": "string", "minLength": 1}},
        "additionalProperties": false,
    });
    super::tool(
        "agents_send",
        "Message agent",
        SEND,
        parameters,
        Arc::new(move |args| {
            let factory = factory.upgrade();
            let sender = sender.clone();
            Box::pin(async move {
                let args: SendArgs = serde_json::from_value(args)?;
                let Some(factory) = factory else { return Err(factory_agent::Error::Other("Factory is stopping.".into())) };
                let sent = send(&factory, &sender, &args.to, args.text).map_err(factory_agent::Error::Other)?;
                Ok(ToolResult::text(sent, json!({})))
            })
        }),
    )
}

fn status_tool(factory: &Arc<Factory>, asker: &str) -> Tool {
    let factory = Arc::downgrade(factory);
    let asker = asker.to_owned();
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
        Arc::new(move |args| {
            let factory = factory.upgrade();
            let asker = asker.clone();
            Box::pin(async move {
                let args: StatusArgs = serde_json::from_value(args)?;
                let Some(factory) = factory else { return Err(factory_agent::Error::Other("Factory is stopping.".into())) };
                let lines = status(&factory, &asker, args.agent.as_deref().filter(|agent| !agent.trim().is_empty()))
                    .map_err(factory_agent::Error::Other)?;
                Ok(ToolResult::text(lines, json!({})))
            })
        }),
    )
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
pub(crate) fn send(factory: &Arc<Factory>, sender: &str, to: &str, text: String) -> Result<String, String> {
    let (from, target, working) = {
        let inner = factory.lock();
        let config = &inner.config;
        let Some(from) = config.agent(sender) else { return Err("You're no longer in this Factory.".to_owned()) };
        let target = resolve(config, from, to)?;
        let working = inner.agents.get(&target.id).is_some_and(|runtime| runtime.running);
        (signed(config, from, target), target.id.clone(), working)
    };
    let event = Event { source: EventSource::Agent, from, id: sender.to_owned(), summary: String::new(), transient: false };
    factory.deliver_event(&target, text, event);
    // The person sees who talks to whom: both rows glow.
    let row = |id: &str| v1::HighlightTarget { kind: "agent".to_owned(), id: id.to_owned() };
    factory.send(Body::Highlight(v1::Highlight {
        targets: vec![row(sender), row(&target)],
        text: String::new(),
        from_agent_id: sender.to_owned(),
        kind: v1::HighlightKind::Message.into(),
    }));
    Ok(if working { "Sent; it's working and reads it after its current step.".to_owned() } else { "Sent.".to_owned() })
}

/// "12m", "3h", "2d": how long ago `since` was.
fn ago(since: i64, now: i64) -> String {
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
    let Some(from) = config.agent(asker) else { return Err("You're no longer in this Factory.".to_owned()) };
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
        let runtime = inner.agents.get(&agent.id);
        let (started, ended) = times.get(&agent.id).copied().unwrap_or_default();
        let mut line = format!("{} {}", agent.id, with_role(agent, place(config, from, agent)));
        if runtime.is_some_and(|runtime| runtime.running) {
            line.push_str(": working");
            if let Some(started) = started {
                let _ = write!(line, " {}", ago(started, now));
            }
            if let Some(tool) = runtime.and_then(|runtime| runtime.doing.as_deref()) {
                let _ = write!(line, ", running {tool}");
            }
        } else {
            line.push_str(": idle");
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
        lines.push(line);
    }
    Ok(lines.join("\n"))
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
