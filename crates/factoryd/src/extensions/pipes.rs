//! The Pipes extension: an agent starts a pipe for the person (#244), the
//! way devices_add adds a device. `pipes_invite` and `pipes_accept` open a
//! card in its conversation for the person to make the invite, or paste the
//! code they were given, themselves (`crate::pipes::start`); `pipes_list`
//! says where the pipes stand. Admin agents such as Rivet have it unless
//! it's turned off. No tool takes or returns a code, approves a pipe or
//! chooses its agents.

use std::sync::Arc;

use factory_agent::{Tool, ToolResult};
use factory_protocol::v1::PipeStartKind;
use serde::Deserialize;
use serde_json::json;

use super::{Extension, Manifest, PromptKind, PromptSection};
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
const LIST: &str = "The person's pipes, or one project's (project, its name or id): each one's name, id and where it stands. Never codes, digits or what the other side says about itself.";

pub(crate) struct Pipes;

impl Extension for Pipes {
    fn manifest(&self) -> &'static Manifest {
        &MANIFEST
    }

    fn prompt(&self, _factory: &Factory, inner: &Inner, _agent: &AgentConfig) -> Vec<PromptSection> {
        let person = inner.config.person();
        let text = format!(
            "## Starting pipes\n\nWhen {person} wants one of their projects to talk with someone else's Claktory, start the pipe for them: pipes_invite when they're inviting someone, pipes_accept when someone gave them a code or link. Either opens a card in your conversation, filled in from what they told you, where they make the invite or paste the code themselves. You never see a code (never ask for one, and never pass one on), you don't approve the pipe, and you don't choose its agents: {person} does, in the pipe. pipes_list says where their pipes stand.\n"
        );
        vec![PromptSection { kind: PromptKind::Static, text }]
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

fn list_pipes(factory: &Arc<Factory>, _agent: &str, args: ListArgs) -> Result<ToolResult, String> {
    Ok(ToolResult::text(start::list(factory, args.project.as_deref())?, json!({})))
}
