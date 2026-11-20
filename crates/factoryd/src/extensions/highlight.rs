//! Highlights: the agent makes parts of the app glow briefly, so the person
//! finds what it's talking about, such as an agent it just set up or where
//! to click. Highlights are transient: sent to the clients connected now and
//! never stored. Factory also highlights both agents when one messages
//! another (see `agents::send`). The agent can also open a screen in the
//! app the person is using (`show_screen`): every app is asked, and each
//! opens it only while it's the one in use, answering whether it did.

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

`show_screen` opens a screen in the app on the device the person is using right now, when you're helping them with it (\"Let's pair your phone\": Settings → Devices): `screen` settings (`id` a section: profile, general, account, providers, secrets, apps, defaults, devices, resources, display; none for the top), map, usage, marketplace, changelog (What's new), agent (`id` an agent's id: its conversation) or agent_settings (`id` an agent's id). It never brings the app forward or opens on a device they aren't using; when nothing opened, tell them where to find it instead. It changes what they see, so open only what they need, when they need it.
";

const DESCRIPTION: &str = "Make parts of the app glow for a moment so the person sees them. targets: [{kind, id}], kind agent, project, checklist_item (\"<agent>#<n>\"), extension (\"<agent>/<extension>\"), settings or marketplace (no id). text: an optional short caption.";

const KINDS: [&str; 6] = ["agent", "project", "checklist_item", "extension", "settings", "marketplace"];

const SCREEN_DESCRIPTION: &str = "Open a screen in the app on the device the person is using now (never another device, never bringing the app forward). screen: settings (id: a section, profile, general, account, providers, secrets, apps, defaults, devices, resources or display; none for the top), map, usage, marketplace, changelog, agent (id: an agent's id, its conversation) or agent_settings (id: an agent's id). Says on which device it opened, or that none did.";

const SCREENS: [&str; 7] = ["settings", "map", "usage", "marketplace", "changelog", "agent", "agent_settings"];

/// The sections of Settings an agent can open, as the app names them.
const SECTIONS: [(&str, &str); 10] = [
    ("profile", "Profile"),
    ("general", "General"),
    ("account", "Account"),
    ("providers", "Providers"),
    ("secrets", "Secrets"),
    ("apps", "Apps"),
    ("defaults", "Defaults"),
    ("devices", "Devices"),
    ("resources", "Resources"),
    ("display", "Display"),
];

/// How long `show_screen` waits for the apps to say whether they opened it.
const SCREEN_WAIT: std::time::Duration = std::time::Duration::from_secs(3);

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
        let weak = Arc::downgrade(factory);
        let (factory, agent) = (weak.clone(), agent.to_owned());
        let caller = agent.clone();
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
        let screen_parameters = json!({
            "type": "object",
            "required": ["screen"],
            "properties": {
                "screen": {"type": "string", "enum": SCREENS},
                "id": {"type": "string"},
            },
            "additionalProperties": false,
        });
        let (factory, agent) = (weak, caller);
        let screen: factory_agent::Handler = Arc::new(move |args| {
            let factory = factory.upgrade();
            let agent = agent.clone();
            Box::pin(async move {
                let Some(factory) = factory else { return Err(factory_agent::Error::Other("Claktory is stopping.".into())) };
                let args: ScreenArgs = serde_json::from_value(args)?;
                let text = open_screen(&factory, &agent, args).await.map_err(factory_agent::Error::Other)?;
                Ok(ToolResult::text(text, json!({})))
            })
        });
        vec![
            super::tool("highlight", "Highlight", DESCRIPTION, parameters, handler),
            super::tool("show_screen", "Show screen", SCREEN_DESCRIPTION, screen_parameters, screen),
        ]
    }
}

#[derive(Deserialize)]
struct ScreenArgs {
    screen: String,
    #[serde(default)]
    id: Option<String>,
}

/// The answers to each `show_screen` waiting for them, by its id: the
/// device's name and whether it opened the screen.
type Answers = tokio::sync::mpsc::UnboundedSender<(String, bool)>;
static WAITING: std::sync::Mutex<Option<std::collections::HashMap<String, Answers>>> = std::sync::Mutex::new(None);

fn waiting() -> std::sync::MutexGuard<'static, Option<std::collections::HashMap<String, Answers>>> {
    WAITING.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// An app's answer to OpenScreen `id`: `device` (its name) opened it or not.
pub(crate) fn screen_opened(id: &str, device: &str, opened: bool) {
    if let Some(answers) = waiting().as_ref().and_then(|waiting| waiting.get(id)) {
        let _ = answers.send((device.to_owned(), opened));
    }
}

/// Asks every app to open the screen, and says where it opened.
async fn open_screen(factory: &Factory, agent: &str, args: ScreenArgs) -> Result<String, String> {
    let id = args.id.unwrap_or_default().trim().to_owned();
    let (name, person, persons) = {
        let inner = factory.lock();
        let name = screen_name(factory, &inner.config, &args.screen, &id)?;
        (name, inner.config.person_capitalized().to_owned(), format!("{}'s", inner.config.person()))
    };
    let call = crate::connectors::random_id();
    let (answers, mut answered) = tokio::sync::mpsc::unbounded_channel();
    waiting().get_or_insert_with(Default::default).insert(call.clone(), answers);
    let apps = factory.clients();
    let target = v1::HighlightTarget { kind: args.screen, id };
    factory.send(Body::OpenScreen(v1::OpenScreen { id: call.clone(), target: Some(target), from_agent_id: agent.to_owned() }));
    let mut opened = Vec::new();
    let mut answers = 0;
    let deadline = tokio::time::Instant::now() + SCREEN_WAIT;
    while answers < apps {
        match tokio::time::timeout_at(deadline, answered.recv()).await {
            Ok(Some((device, shown))) => {
                answers += 1;
                if shown && !opened.contains(&device) {
                    opened.push(device);
                }
            }
            Ok(None) | Err(_) => break,
        }
    }
    if let Some(waiting) = waiting().as_mut() {
        waiting.remove(&call);
    }
    Ok(if !opened.is_empty() {
        format!("Opened {name} on {}.", opened.join(" and "))
    } else if apps > 0 && answers == 0 {
        format!(
            "Nothing opened: {persons} app didn't answer (it may be too old to open screens). Tell them where to find it instead: {name}."
        )
    } else {
        format!("{person} isn't using Claktory on any device right now, so nothing opened. Tell them where to find it instead: {name}.")
    })
}

/// What the screen is called, as the person finds it ("Settings → Devices",
/// "Mike's settings"), or an error saying what's wrong with it.
fn screen_name(factory: &Factory, config: &Config, screen: &str, id: &str) -> Result<String, String> {
    let agent = |id: &str| config.agent(id).map(|agent| agent.name.clone()).ok_or_else(|| format!("No agent has the id \"{id}\"."));
    let none = |name: &str| if id.is_empty() { Ok(name.to_owned()) } else { Err(format!("{name} takes no id.")) };
    match screen {
        "settings" if id.is_empty() => Ok("Settings".to_owned()),
        "settings" => {
            let ids = || SECTIONS.iter().map(|(id, _)| *id).collect::<Vec<_>>().join(", ");
            let Some((_, section)) = SECTIONS.iter().find(|(section, _)| *section == id) else {
                return Err(format!("Settings has no section \"{id}\"; use one of {}.", ids()));
            };
            if id == "apps" && !factory.apps_available() {
                return Err("Settings has no Apps section in this Claktory.".to_owned());
            }
            Ok(format!("Settings → {section}"))
        }
        "map" => none("the Map"),
        "usage" => none("Usage"),
        "marketplace" => none("the Marketplace"),
        "changelog" => none("What's new"),
        "agent" => Ok(format!("{}'s conversation", agent(id)?)),
        "agent_settings" => Ok(format!("{}'s settings", agent(id)?)),
        other => Err(format!("No screen \"{other}\"; use one of {}.", SCREENS.join(", "))),
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
