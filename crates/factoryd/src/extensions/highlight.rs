//! Highlights: the agent makes parts of the app glow briefly, so the person
//! finds what it's talking about, such as an agent it just set up or where
//! to click. Highlights are transient: sent to the clients connected now and
//! never stored. Factory also highlights both agents when one messages
//! another (see `agents::send`).

use std::sync::Arc;

use factory_agent::{Tool, ToolResult};
use factory_protocol::v1::{self, server_message::Body};
use serde::Deserialize;
use serde_json::json;

use super::{Extension, Manifest, PromptKind, PromptSection};
use crate::config::{AgentConfig, Config};
use crate::factory::{Factory, Inner};

pub(crate) const ID: &str = "highlight";

static MANIFEST: Manifest = Manifest {
    id: ID,
    name: "Highlights",
    icon: "sparkles",
    description: "Lets it make parts of the app glow for a moment, to show you what it means.",
    default_enabled: false,
    options: &[],
};

const PROMPT: &str = "## Highlights

The person sees Claktory in its app. `highlight` makes parts of the app glow for a moment, so they find what you mean: after you set something up, or when you tell them where to click. Targets, by kind and id:
- agent: an agent's id (its row in the sidebar)
- project: a project's id
- checklist_item: \"<agent id>#<number>\", e.g. \"backend#4\"
- extension: \"<agent id>/<extension id>\", the extension's icon for that agent, e.g. \"backend/checklist\"
- settings, marketplace: the buttons at the bottom of the sidebar (no id)
Add a short `text` to say what to look at. It changes nothing; use it sparingly.
";

const DESCRIPTION: &str = "Make parts of the app glow for a moment so the person sees them. targets: [{kind, id}], kind agent, project, checklist_item (\"<agent>#<n>\"), extension (\"<agent>/<extension>\"), settings or marketplace (no id). text: an optional short caption.";

const KINDS: [&str; 6] = ["agent", "project", "checklist_item", "extension", "settings", "marketplace"];

pub(crate) struct Highlight;

impl Extension for Highlight {
    fn manifest(&self) -> &'static Manifest {
        &MANIFEST
    }

    fn prompt(&self, _factory: &Factory, _inner: &Inner, _agent: &AgentConfig) -> Vec<PromptSection> {
        vec![PromptSection { kind: PromptKind::Static, text: PROMPT.to_owned() }]
    }

    fn tools(&self, factory: &Arc<Factory>, agent: &str) -> Vec<Tool> {
        // Optional parameters are the ones left out of `required` (see `super::tool`).
        let parameters = json!({
            "type": "object",
            "required": ["targets"],
            "properties": {
                "targets": {"type": "array", "minItems": 1, "items": {
                    "type": "object",
                    "required": ["kind"],
                    "properties": {"kind": {"type": "string", "enum": KINDS}, "id": {"type": "string"}},
                    "additionalProperties": false,
                }},
                "text": {"type": "string"},
            },
            "additionalProperties": false,
        });
        let factory = Arc::downgrade(factory);
        let agent = agent.to_owned();
        let handler: factory_agent::Handler = Arc::new(move |args| {
            let factory = factory.upgrade();
            let agent = agent.clone();
            Box::pin(async move {
                let Some(factory) = factory else { return Err(factory_agent::Error::Other("Claktory is stopping.".into())) };
                let args: Args = serde_json::from_value(args)?;
                let text = show(&factory, &agent, args).map_err(factory_agent::Error::Other)?;
                Ok(ToolResult::text(text, json!({})))
            })
        });
        vec![super::tool("highlight", "Highlight", DESCRIPTION, parameters, handler)]
    }
}

#[derive(Deserialize)]
struct Args {
    targets: Vec<Target>,
    #[serde(default)]
    text: Option<String>,
}

#[derive(Deserialize)]
struct Target {
    kind: String,
    #[serde(default)]
    id: Option<String>,
}

/// Sends the highlight if every target exists; says whether the person sees it.
fn show(factory: &Factory, agent: &str, args: Args) -> Result<String, String> {
    let (targets, on, person) = {
        let inner = factory.lock();
        let mut targets = Vec::new();
        for target in args.targets {
            let id = target.id.unwrap_or_default().trim().to_owned();
            check(factory, &inner.config, &target.kind, &id)?;
            targets.push(v1::HighlightTarget { kind: target.kind, id });
        }
        (targets, inner.config.highlights, inner.config.person_capitalized().to_owned())
    };
    let text = args.text.unwrap_or_default().trim().to_owned();
    let kind = v1::HighlightKind::Agent.into();
    factory.send(Body::Highlight(v1::Highlight { targets, text, from_agent_id: agent.to_owned(), kind }));
    Ok(if on { "Shown.".to_owned() } else { format!("Highlights are off: {person} turned them off in Settings, so they don't see it.") })
}

/// An error saying what's wrong with the target, if anything.
fn check(factory: &Factory, config: &Config, kind: &str, id: &str) -> Result<(), String> {
    let agent = |id: &str| config.agent(id).map(|_| ()).ok_or_else(|| format!("No agent has the id \"{id}\"."));
    match kind {
        "agent" => agent(id),
        "project" => config.project(id).map(|_| ()).ok_or_else(|| format!("No project has the id \"{id}\".")),
        "checklist_item" => {
            let Some((owner, number)) = id.split_once('#') else {
                return Err(format!("A checklist item is \"<agent id>#<number>\", not \"{id}\"."));
            };
            agent(owner)?;
            let number: u32 = number.parse().map_err(|_| format!("A checklist item is \"<agent id>#<number>\", not \"{id}\"."))?;
            match crate::extensions::checklist::has_item(&factory.store, owner, number) {
                Ok(true) => Ok(()),
                Ok(false) => Err(format!("{owner}'s checklist has no item #{number}.")),
                Err(error) => Err(format!("Couldn't read the checklist: {error}")),
            }
        }
        "extension" => {
            let Some((owner, extension)) = id.split_once('/') else {
                return Err(format!("An extension is \"<agent id>/<extension id>\", not \"{id}\"."));
            };
            let Some(owner) = config.agent(owner) else { return Err(format!("No agent has the id \"{owner}\".")) };
            let Some(found) = super::find(extension) else { return Err(format!("No extension has the id \"{extension}\".")) };
            if super::enabled(owner, found.manifest().id) {
                Ok(())
            } else {
                Err(format!("{} doesn't have {} turned on, so the app shows no icon for it.", owner.name, found.manifest().name))
            }
        }
        "settings" | "marketplace" => Ok(()),
        other => Err(format!("No kind \"{other}\"; use one of {}.", KINDS.join(", "))),
    }
}
