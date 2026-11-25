//! The App grants extension (U2): Rivet proposes giving an agent access to
//! an app kept at the person's account, such as their direct messages, and
//! the person decides on the card it opens (`crate::user_apps`); and Rivet
//! sets up the apps installed in this Claktory (`crate::apps::access`,
//! #339), with a card only where Fugo's rules need the person. Only Rivet
//! (`proposals::rivet`, the settings' admin agent, never a tool argument)
//! can propose, never while it has read a web page, and only for agents of this
//! Claktory: never a chat. Nothing is ever given without the person's
//! approval, and the account service enforces what was.

use std::fmt::Write as _;
use std::sync::Arc;

use factory_agent::{Tool, ToolResult};
use factory_protocol::v1;
use serde::Deserialize;
use serde_json::json;

use super::{Extension, Manifest, PromptKind, PromptSection};
use crate::account::apps::Resource;
use crate::config::AgentConfig;
use crate::factory::{Factory, Inner};
use crate::transcript::now_ms;
use crate::user_apps::{self, Card};

pub(crate) const ID: &str = "app_grants";

static MANIFEST: Manifest = Manifest {
    id: ID,
    name: "App grants",
    icon: "key-round",
    description: "Lets Rivet set up what your apps can read and which agents can use them, such as Charts or your direct messages, when you ask. Rivet asks you first before anything private or anything that can reach the internet, and you can undo any change in Needs you's History.",
    default_enabled: false,
    options: &[],
};

const UNINSTALL: &str = "Ask the person to uninstall an app installed in this Claktory, when they ask: app, its id, as apps_list gives it; keep_data: true to keep what it stored, for a later install; reason: why, in a sentence they read on the card. A card opens in your conversation (and their Needs you), and only their approval uninstalls it; you're told which in an event. To stop an app for a while instead, use apps_set_running. proposal: the id of an approved proposal whose grants name it (keep_data true), once you've reviewed it: then it's made without a card when it's within one of them; otherwise the card opens as usual and the answer says why.";
const RUNNING: &str = "Start or stop an app installed in this Claktory, as the person's switch in Marketplace → Apps does, when they ask (or to start again one Claktory paused after it crashed). app: its id, as apps_list gives it. running: true to start it, false to stop it. Stopped, it stays installed and set up, and its agents can't use it until it starts again.";

const PROPOSE: &str = "Propose giving an agent access to an app at the person's account, when they ask for it: agent, its id or name; app, \"dms\" (direct messages); scopes, what it may do there (dm.read, dm.draft, dm.send, dm.subscribe); resources, what it reaches ([{\"kind\": \"conversation\", \"id\": …}], at least one); days, how long it lasts (left out: until the person takes it back); reason, why, for the person. It opens a card in your conversation where the person approves or declines it, and you're told which. Ask only for what the work needs. Approved, it replaces what that agent had in the app.";

/// The longest reason a card shows, and the longest a grant lasts.
const MAX_REASON: usize = 500;
const MAX_DAYS: i64 = 365;

pub(crate) struct AppGrants;

impl Extension for AppGrants {
    fn manifest(&self) -> &'static Manifest {
        &MANIFEST
    }

    fn prompt(&self, _factory: &Factory, inner: &Inner, _agent: &AgentConfig) -> Vec<PromptSection> {
        let person = inner.config.person();
        let text = format!(
            "## App grants\n\nSome of {person}'s apps live at their account rather than in this Claktory, such as their direct messages. An agent uses one only with a grant {person} approved. When they ask you to give an agent access, propose it with app_grants_propose: it opens a card for them to approve or decline. You never approve it yourself, and nothing is given by default.\n\nThe apps installed in this Claktory, such as Charts, start with nothing: no data to read and no agent to use them. {person} doesn't set them up: they tell you what they want (\"have Ingrid keep a chart of her bugs by severity\"), and you set it up with apps_list and apps_set_access, giving the least that does it: the sources it needs (activity, with no message text, for most charts; conversations or an agent's database only when what they want is in there) and the agents that make or keep up its dashboards, with their scopes; then tell those agents what to keep up. Taking away, and giving what isn't private to an app that reaches no site, is applied at once; private data, an app that reaches the internet, or agents (or you) that have read a web page go on a card for {person} to approve: apps_list says which, so tell them before. Every change shows in their History, where they can undo it. Change what apps read and who uses them only with these tools, never by editing config.json.\n"
        );
        vec![PromptSection { kind: PromptKind::Static, text }]
    }

    fn tools(&self, factory: &Arc<Factory>, agent: &str) -> Vec<Tool> {
        let parameters = json!({
            "type": "object",
            "required": ["agent", "app", "scopes", "reason"],
            "properties": {
                "agent": {"type": "string", "minLength": 1},
                "app": {"type": "string", "enum": user_apps::APPS.iter().map(|app| app.id).collect::<Vec<_>>()},
                "scopes": {"type": "array", "items": {"type": "string"}, "minItems": 1},
                "resources": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "required": ["kind", "id"],
                        "properties": {"kind": {"type": "string"}, "id": {"type": "string"}},
                    },
                },
                "days": {"type": "integer", "minimum": 1, "maximum": MAX_DAYS},
                "reason": {"type": "string", "minLength": 1},
            },
            "additionalProperties": false,
        });
        let list = json!({
            "type": "object",
            "properties": {"app": {"type": "string"}},
            "additionalProperties": false,
        });
        let set = json!({
            "type": "object",
            "required": ["app", "reason"],
            "properties": {
                "app": {"type": "string", "minLength": 1},
                "sources": {"type": "object", "additionalProperties": {"type": "boolean"}},
                "proposal": {"type": "string"},
                "agents": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "required": ["agent", "scopes"],
                        "properties": {
                            "agent": {"type": "string", "minLength": 1},
                            "scopes": {"type": "array", "items": {"type": "string"}},
                            "hears": {"type": "array", "items": {"type": "string"}},
                        },
                        "additionalProperties": false,
                    },
                },
                "reason": {"type": "string", "minLength": 1},
            },
            "additionalProperties": false,
        });
        let running = json!({
            "type": "object",
            "required": ["app", "running"],
            "properties": {"app": {"type": "string", "minLength": 1}, "running": {"type": "boolean"}},
            "additionalProperties": false,
        });
        let uninstall = json!({
            "type": "object",
            "required": ["app", "reason"],
            "properties": {
                "app": {"type": "string", "minLength": 1},
                "keep_data": {"type": "boolean"},
                "reason": {"type": "string", "minLength": 1, "maxLength": MAX_REASON},
                "proposal": {"type": "string"},
            },
            "additionalProperties": false,
        });
        vec![
            super::tool(user_apps::TOOL, "Propose app access", PROPOSE, parameters, super::typed_handler(factory, agent, propose)),
            super::async_tool(factory, agent, RUNNING_TOOL, "Start or stop an app", RUNNING, running, set_running),
            super::async_tool(factory, agent, crate::card_actions::UNINSTALL_TOOL, "Uninstall an app", UNINSTALL, uninstall, uninstall_app),
            super::tool(
                crate::apps::access::LIST_TOOL,
                "Apps",
                crate::apps::access::LIST_DESCRIPTION,
                list,
                super::typed_handler(factory, agent, list_apps),
            ),
            super::async_tool(
                factory,
                agent,
                crate::apps::access::SET_TOOL,
                "Set up an app",
                crate::apps::access::SET_DESCRIPTION,
                set,
                set_access,
            ),
        ]
    }

    fn admin_default(&self) -> bool {
        true
    }
}

#[derive(Deserialize)]
struct ProposeArgs {
    agent: String,
    app: String,
    scopes: Vec<String>,
    #[serde(default)]
    resources: Vec<Resource>,
    #[serde(default)]
    days: Option<i64>,
    reason: String,
}

/// The agent named, by id or else by name, of this Claktory's agents.
fn named(factory: &Factory, wanted: &str) -> Result<(String, String), String> {
    let inner = factory.lock();
    let wanted = wanted.trim();
    if crate::chats::is_chat(wanted) {
        return Err("Only an agent can be given access to an app, not a chat.".to_owned());
    }
    if let Some(agent) = inner.config.agent(wanted) {
        return Ok((agent.id.clone(), agent.name.clone()));
    }
    let matching: Vec<&AgentConfig> = inner.config.agents.iter().filter(|agent| agent.name.eq_ignore_ascii_case(wanted)).collect();
    match matching.as_slice() {
        [agent] => Ok((agent.id.clone(), agent.name.clone())),
        [] => Err(format!("There's no agent \"{wanted}\" in this Claktory.")),
        several => {
            let ids: Vec<&str> = several.iter().map(|agent| agent.id.as_str()).collect();
            Err(format!("More than one agent is called {wanted}: name one by its id ({}).", ids.join(", ")))
        }
    }
}

/// app_grants_propose: checks what Rivet asks for as the service would,
/// and opens the card.
fn propose(factory: &Arc<Factory>, agent: &str, args: ProposeArgs) -> Result<ToolResult, String> {
    only_rivet(factory, agent)?;
    untainted(factory, agent)?;
    let person = {
        let inner = factory.lock();
        if factory.account.signed_in_as(&inner.config.account).is_none() {
            return Err(format!(
                "This Claktory isn't signed in to {}'s account, so its agents can't be given access to apps there. Nothing was proposed.",
                inner.config.person()
            ));
        }
        inner.config.person().to_owned()
    };
    let (grantee, name) = named(factory, &args.agent)?;
    // A liaison, limited to a pipe, would bridge the other Claktory to the
    // person's app (Fugo's review).
    if factory.pipes.contained(&grantee) {
        return Err(format!("{name} is limited to a pipe, so it can't be given access to {person}'s apps. Nothing was proposed."));
    }
    let Some(app) = user_apps::app(args.app.trim()) else {
        let known: Vec<&str> = user_apps::APPS.iter().map(|app| app.id).collect();
        return Err(format!("There's no app \"{}\": the apps are {}.", args.app.trim(), known.join(", ")));
    };
    let mut scopes: Vec<String> = args.scopes.iter().map(|scope| scope.trim().to_owned()).collect();
    scopes.sort();
    scopes.dedup();
    if scopes.is_empty() || scopes.len() > user_apps::MAX_SCOPES {
        return Err(format!("Name 1 to {} scopes.", user_apps::MAX_SCOPES));
    }
    if let Some(unknown) = scopes.iter().find(|scope| !app.scopes.iter().any(|(id, _)| id == scope)) {
        let known: Vec<&str> = app.scopes.iter().map(|(id, _)| *id).collect();
        return Err(format!("{} has no scope \"{unknown}\": its scopes are {}.", app.name, known.join(", ")));
    }
    let mut resources: Vec<Resource> = args
        .resources
        .iter()
        .map(|resource| Resource { kind: resource.kind.trim().to_owned(), id: resource.id.trim().to_owned() })
        .collect();
    resources.sort_by(|a, b| (&a.kind, &a.id).cmp(&(&b.kind, &b.id)));
    resources.dedup();
    if resources.len() > user_apps::MAX_RESOURCES {
        return Err(format!("Name {} resources at most.", user_apps::MAX_RESOURCES));
    }
    if app.needs_resources && resources.is_empty() {
        return Err(format!("Name what it would reach in {} (resources): a grant there that names nothing gives nothing.", app.name));
    }
    if let Some(bad) = resources
        .iter()
        .find(|resource| !app.resource_kinds.contains(&resource.kind.as_str()) || !user_apps::valid_resource_id(&resource.id))
    {
        return Err(format!(
            "\"{}\" \"{}\" isn't something in {}: name {} by their ids.",
            bad.kind,
            bad.id,
            app.name,
            app.resource_kinds.join(", ")
        ));
    }
    let reason = args.reason.trim().to_owned();
    if reason.is_empty() || reason.chars().count() > MAX_REASON {
        return Err(format!("Say why in 1 to {MAX_REASON} characters."));
    }
    let created = now_ms();
    let expires = match args.days {
        None => 0,
        Some(days) if (1..=MAX_DAYS).contains(&days) => created + days * 86_400_000,
        Some(_) => return Err(format!("days is 1 to {MAX_DAYS}, or left out for until {person} takes it back.")),
    };
    if !user_apps::valid_name(&grantee, 64) {
        return Err(format!("{name}'s id can't be named at the account, so it can't be given access."));
    }
    let incarnation = user_apps::incarnation(&factory.home, &grantee).map_err(|error| format!("Couldn't propose it: {error}"))?;
    let card = Card {
        id: format!("g{}", crate::encoding::hex(&crate::vault::crypto::random::<6>())),
        app: app.id.to_owned(),
        agent: grantee.clone(),
        scopes,
        resources,
        expires_ms: expires,
        reason,
        created_ms: created,
        read_pipes: !factory.pipes.tainted(&grantee).is_empty(),
    };
    user_apps::open(&factory.store, agent, &incarnation, &card)?;
    let mut text = format!(
        "Opened a card in your conversation for {person} to approve or decline giving {name} access to {}. You'll be told which.",
        app.name
    );
    for (held_app, held_scopes, held_resources) in user_apps::held(&factory.store, &grantee) {
        if held_app == app.id {
            let _ = write!(
                text,
                " {name} already has {} on {} there; approved, this replaces it.",
                held_scopes.join(", "),
                held_resources.iter().map(|resource| resource.id.as_str()).collect::<Vec<_>>().join(", ")
            );
        }
    }
    let details = json!({"app_grant": card});
    Ok(ToolResult::text(text, details))
}

/// apps_list: only Rivet.
fn list_apps(factory: &Arc<Factory>, agent: &str, args: crate::apps::access::ListArgs) -> Result<ToolResult, String> {
    only_rivet_sets(factory, agent)?;
    crate::apps::access::list(factory, agent, args)
}

/// apps_set_access: only Rivet. A tainted Rivet isn't refused: what it
/// gives goes on a card (`crate::apps::access`).
async fn set_access(factory: Arc<Factory>, agent: String, args: crate::apps::access::SetArgs) -> Result<ToolResult, String> {
    only_rivet_sets(&factory, &agent)?;
    crate::apps::access::set_access(&factory, &agent, args).await
}

/// apps_set_running's name.
pub(crate) const RUNNING_TOOL: &str = "apps_set_running";

#[derive(Deserialize)]
struct RunningArgs {
    app: String,
    running: bool,
}

/// apps_set_running: only Rivet starts or stops an installed app, as the
/// person's switch does (`Factory::set_app_running`), crashes before
/// forgotten when it starts.
async fn set_running(factory: Arc<Factory>, agent: String, args: RunningArgs) -> Result<ToolResult, String> {
    only_rivet_sets(&factory, &agent)?;
    // A page or a pipe it read mustn't turn the person's apps on or off
    // (Fugo, Abbacchio's row 1047).
    super::untainted(&factory, &agent, "start or stop apps", "start or stop one with its switch in Marketplace → Apps. Nothing was changed.")?;
    let wanted = args.app.trim().to_owned();
    if !factory.apps.available() {
        return Err("Apps aren't available in this Claktory yet. Nothing was changed.".to_owned());
    }
    let (installed, was) = {
        let inner = factory.lock();
        let app = inner.config.apps.iter().find(|app| app.id == wanted);
        (app.is_some(), app.is_some_and(|app| app.running))
    };
    if !installed {
        return Err(format!("There's no app \"{wanted}\" installed: apps_list gives their ids. Nothing was changed."));
    }
    let name = factory.apps.manifest(&wanted).map_or_else(|| wanted.clone(), |manifest| manifest.name.clone());
    if !args.running && !was {
        return Ok(ToolResult::text(format!("{name} is stopped already."), json!({"app": wanted, "running": false})));
    }
    let request = v1::SetAppRunning { id: wanted.clone(), running: args.running };
    factory.set_app_running(&request).await.map_err(|error| format!("{error} Nothing was changed."))?;
    let said = if args.running {
        format!("Started {name}: it's starting now, and apps_list shows how it's doing.")
    } else {
        format!("Stopped {name}. It stays installed and set up; its agents can't use it until it starts again.")
    };
    Ok(ToolResult::text(said, json!({"app": wanted, "running": args.running})))
}

#[derive(Deserialize)]
struct UninstallArgs {
    app: String,
    #[serde(default)]
    keep_data: bool,
    reason: String,
    /// An approved proposal that names it (`crate::proposal_grants`).
    #[serde(default)]
    proposal: Option<String>,
}

/// apps_uninstall (#423): asks the person, on a card, to uninstall an app;
/// only their approval does.
async fn uninstall_app(factory: Arc<Factory>, agent: String, args: UninstallArgs) -> Result<ToolResult, String> {
    let (factory, agent) = (&factory, agent.as_str());
    only_rivet_sets(factory, agent)?;
    let wanted = args.app.trim().to_owned();
    let reason = args.reason.split_whitespace().collect::<Vec<_>>().join(" ");
    let (installed, me, person) = {
        let inner = factory.lock();
        let me = inner.config.agent(agent).map(|me| me.name.clone()).unwrap_or_default();
        (inner.config.apps.iter().any(|app| app.id == wanted), me, inner.config.person_capitalized().to_owned())
    };
    if !installed {
        return Err(format!("There's no app \"{wanted}\" installed: apps_list gives their ids. Nothing was asked."));
    }
    if reason.is_empty() {
        return Err(format!("Give reason: why, in a sentence {person} reads on the card. Nothing was asked."));
    }
    let name = factory.apps.manifest(&wanted).map_or_else(|| wanted.clone(), |manifest| manifest.name.clone());
    let data = if args.keep_data { "what it stored is kept for a later install" } else { "what it stored is deleted too" };
    let description = format!(
        "{me} asks to uninstall {name}: it stops, and it's removed with its setup (what it reads and which agents use it); {data}. Its secrets stay in your vault."
    );
    let action = crate::card_actions::Action::UninstallApp { app: wanted, keep_data: args.keep_data };
    crate::card_actions::ask_or_proposed(
        factory,
        agent,
        &format!("Uninstall {name}"),
        &description,
        &reason,
        &action,
        args.proposal.as_deref(),
    )
    .await
}

/// Only Rivet sets apps up: the settings' admin agent, whoever calls.
fn only_rivet_sets(factory: &Factory, agent: &str) -> Result<(), String> {
    let inner = factory.lock();
    super::caller(&inner.config, agent)?;
    if crate::proposals::rivet(&inner.config).is_some_and(|rivet| rivet.id == agent) {
        Ok(())
    } else {
        Err("Only Rivet, the admin agent, sets up the person's apps. Nothing was changed.".to_owned())
    }
}

/// Only Rivet proposes: the settings' admin agent, whoever calls.
fn only_rivet(factory: &Factory, agent: &str) -> Result<(), String> {
    let inner = factory.lock();
    super::caller(&inner.config, agent)?;
    if crate::proposals::rivet(&inner.config).is_some_and(|rivet| rivet.id == agent) {
        Ok(())
    } else {
        Err("Only Rivet, the admin agent, proposes access to the person's apps. Nothing was proposed.".to_owned())
    }
}

/// An agent that has read a web page proposes nothing: what it read may be
/// someone else's instructions.
fn untainted(factory: &Factory, agent: &str) -> Result<(), String> {
    if factory.pipes.tainted(agent).is_empty() {
        Ok(())
    } else {
        let person = factory.person_capitalized();
        let read = crate::pipes::agents::what_was_read(factory, agent);
        Err(format!(
            "You've read {read}, so you can't propose access to {person}'s apps: what you read there may be someone else's instructions. Nothing was proposed."
        ))
    }
}
