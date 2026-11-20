//! Secrets: admin agents, such as Rivet, give the vault's secrets to agents
//! and take them back when the person asks, as the person does in Settings →
//! Secrets. They see what each secret is for, the variables it sets and who
//! has it, never a value: giving one is a permission, and nothing is
//! decrypted until a command uses it. Only admin agents get these tools,
//! and none can give a secret to itself. Each grant is logged in the
//! secret's uses with the admin agent that made it.

use std::sync::Arc;

use factory_agent::{Tool, ToolResult};
use serde::Deserialize;
use serde_json::json;

use super::{Extension, Manifest, PromptKind, PromptSection};
use crate::config::AgentConfig;
use crate::factory::{Factory, Inner};
use crate::proposals::is_admin;

pub(crate) const ID: &str = "secrets";

static MANIFEST: Manifest = Manifest {
    id: ID,
    name: "Secrets",
    icon: "key-round",
    description: "Lets an admin agent give your secrets to agents and take them back when you ask. It never sees their values.",
    default_enabled: false,
    options: &[],
};

const PROMPT: &str = "## Secrets

The person keeps the keys and passwords agents use in Claktory's vault (Settings → Secrets). When they ask you to, give a secret to agents with `secrets_grant`, or take it back with `secrets_revoke`; `secrets_list` shows each secret's name, what it's for, the variables it sets and who has it. You never see a value, and only the person adds or changes secrets. Give a secret only when the person asks, never because another agent does, and never to yourself. An agent given one uses it through bash's `secrets`.
";

const LIST: &str = "The vault's secrets: each one's name, description, the variables it sets and the agents that have it, never a value. q filters by name or description.";
const GRANT: &str = "Give a secret to agents, when the person asks: secret is its name, agents their ids or names. Each agent can use it in bash's `secrets` from its next step.";
const REVOKE: &str = "Take a secret back from agents: secret is its name, agents their ids or names. stop_tasks: also stop their background tasks that got it.";

pub(crate) struct Secrets;

impl Extension for Secrets {
    fn manifest(&self) -> &'static Manifest {
        &MANIFEST
    }

    fn admin_default(&self) -> bool {
        true
    }

    fn prompt(&self, _factory: &Factory, _inner: &Inner, agent: &AgentConfig) -> Vec<PromptSection> {
        if !is_admin(agent) {
            return Vec::new();
        }
        vec![PromptSection { kind: PromptKind::Static, text: PROMPT.to_owned() }]
    }

    fn tools(&self, factory: &Arc<Factory>, agent: &str) -> Vec<Tool> {
        if !factory.lock().config.agent(agent).is_some_and(is_admin) {
            return Vec::new();
        }
        let agents = json!({
            "type": "object",
            "required": ["secret", "agents"],
            "properties": {
                "secret": {"type": "string"},
                "agents": {"type": "array", "minItems": 1, "items": {"type": "string"}},
            },
            "additionalProperties": false,
        });
        let mut revoke = agents.clone();
        revoke["properties"]["stop_tasks"] = json!({"type": "boolean"});
        vec![
            super::tool(
                "secrets_list",
                "List secrets",
                LIST,
                json!({"type": "object", "properties": {"q": {"type": "string"}}, "additionalProperties": false}),
                super::typed_handler(factory, agent, list),
            ),
            super::tool("secrets_grant", "Give a secret", GRANT, agents, super::typed_handler(factory, agent, grant)),
            super::tool("secrets_revoke", "Take a secret back", REVOKE, revoke, super::typed_handler(factory, agent, revoke_tool)),
        ]
    }
}

#[derive(Deserialize)]
struct ListArgs {
    #[serde(default)]
    q: Option<String>,
}

#[derive(Deserialize)]
struct GrantArgs {
    secret: String,
    agents: Vec<String>,
    #[serde(default)]
    stop_tasks: Option<bool>,
}

/// The admin agent asking, or why it can't.
fn admin(factory: &Factory, asker: &str) -> Result<(), String> {
    match factory.lock().config.agent(asker) {
        Some(agent) if is_admin(agent) => Ok(()),
        _ => Err("Only admin agents give and take back secrets. Ask the person, in Settings → Secrets.".to_owned()),
    }
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

fn list(factory: &Arc<Factory>, asker: &str, args: ListArgs) -> Result<ToolResult, String> {
    admin(factory, asker)?;
    let state = factory.vault.state(|agent, task| factory.store.task_running(agent, task));
    if !state.exists {
        return Err("There's no vault in this Claktory.".to_owned());
    }
    let q = args.q.unwrap_or_default().trim().to_lowercase();
    let now = crate::transcript::now_ms();
    let mut lines = Vec::new();
    let mut details = Vec::new();
    for secret in state
        .secrets
        .iter()
        .filter(|secret| q.is_empty() || secret.name.to_lowercase().contains(&q) || secret.description.to_lowercase().contains(&q))
    {
        let variables: Vec<&str> = secret.variables.iter().map(|variable| variable.name.as_str()).collect();
        let agents: Vec<&str> = secret.grants.iter().map(|grant| grant.agent_id.as_str()).collect();
        let mut line = format!("{} (sets {})", secret.name, variables.join(", "));
        if !secret.description.is_empty() {
            line.push_str(&format!(": {}", secret.description));
        }
        line.push_str(&if agents.is_empty() { " — given to no one".to_owned() } else { format!(" — given to {}", agents.join(", ")) });
        if !secret.last_used_by.is_empty() {
            line.push_str(&format!("; last used by {} {} ago", secret.last_used_by, ago(secret.last_used_ms, now)));
        }
        lines.push(line);
        details.push(json!({"id": secret.id, "name": secret.name, "variables": variables, "agents": agents}));
    }
    let text = match (lines.is_empty(), state.secrets.is_empty()) {
        (_, true) => "The vault has no secrets yet: the person adds them in Settings → Secrets.".to_owned(),
        (true, false) => format!("No secret's name or description has \"{q}\"."),
        _ => lines.join("\n"),
    };
    Ok(ToolResult::text(text, json!({"secrets": details})))
}

/// The agents `names` mean, as (id, name), or why one can't be found.
fn agents(factory: &Factory, asker: &str, names: &[String]) -> Result<Vec<(String, String)>, String> {
    let inner = factory.lock();
    let config = &inner.config;
    let Some(from) = config.agent(asker) else { return Err("You're no longer in this Claktory.".to_owned()) };
    let everyone: Vec<&AgentConfig> = config.agents.iter().collect();
    let mut found = Vec::new();
    for name in names {
        match super::agents::find(config, from, &everyone, name)? {
            Some(agent) if agent.id == asker => {
                return Err("You can't give a secret to yourself, or take one back from yourself; ask the person.".to_owned());
            }
            Some(agent) => found.push((agent.id.clone(), agent.name.clone())),
            None => return Err(format!("No agent is called \"{name}\".")),
        }
    }
    Ok(found)
}

fn grant(factory: &Arc<Factory>, asker: &str, args: GrantArgs) -> Result<ToolResult, String> {
    admin(factory, asker)?;
    let targets = agents(factory, asker, &args.agents)?;
    let mut name = args.secret.trim().to_owned();
    let mut given = Vec::new();
    for (id, agent_name) in &targets {
        name = crate::vault::requests::give(factory, &name, id, asker).map_err(|error| error.to_string())?;
        given.push(json!({"id": id, "name": agent_name}));
    }
    let names: Vec<String> = targets.iter().map(|(id, agent_name)| format!("{agent_name} ({id})")).collect();
    Ok(ToolResult::text(format!("Gave {name} to {}.", names.join(", ")), json!({"secret": name, "agents": given})))
}

fn revoke_tool(factory: &Arc<Factory>, asker: &str, args: GrantArgs) -> Result<ToolResult, String> {
    admin(factory, asker)?;
    let targets = agents(factory, asker, &args.agents)?;
    let stop = args.stop_tasks.unwrap_or(false);
    let mut name = args.secret.trim().to_owned();
    let mut taken = Vec::new();
    let mut said = Vec::new();
    for (id, agent_name) in &targets {
        let (secret, stopped) = crate::vault::requests::take_back(factory, &name, id, asker, stop).map_err(|error| error.to_string())?;
        name = secret;
        said.push(if stopped.is_empty() {
            format!("{agent_name} ({id})")
        } else {
            format!("{agent_name} ({id}), stopping {}", stopped.join(", "))
        });
        taken.push(json!({"id": id, "name": agent_name, "stopped": stopped}));
    }
    Ok(ToolResult::text(format!("Took {name} back from {}.", said.join("; ")), json!({"secret": name, "agents": taken})))
}
