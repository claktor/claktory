//! The App grants extension (U2): Rivet proposes giving an agent access to
//! an app kept at the person's account, such as their direct messages, and
//! the person decides on the card it opens (`crate::user_apps`). Only Rivet
//! (`proposals::rivet`, the settings' admin agent, never a tool argument)
//! can propose, never while it has read a pipe, and only for agents of this
//! Claktory: never a chat. Nothing is ever given without the person's
//! approval, and the account service enforces what was.

use std::fmt::Write as _;
use std::sync::Arc;

use factory_agent::{Tool, ToolResult};
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
    description: "Lets Rivet propose giving an agent access to an app at your account, such as your direct messages. Nothing is given until you approve it.",
    default_enabled: false,
    options: &[],
};

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
            "## App grants\n\nSome of {person}'s apps live at their account rather than in this Claktory, such as their direct messages. An agent uses one only with a grant {person} approved. When they ask you to give an agent access, propose it with app_grants_propose: it opens a card for them to approve or decline. You never approve it yourself, and nothing is given by default.\n"
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
        vec![super::tool(user_apps::TOOL, "Propose app access", PROPOSE, parameters, super::typed_handler(factory, agent, propose))]
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
    if factory.pipes.quarantined(&grantee) {
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

/// An agent that has read a pipe proposes nothing: what it read may be
/// someone else's instructions.
fn untainted(factory: &Factory, agent: &str) -> Result<(), String> {
    if factory.pipes.tainted(agent).is_empty() {
        Ok(())
    } else {
        let person = factory.lock().config.person_capitalized().to_owned();
        let read = crate::pipes::agents::what_was_read(factory, agent);
        Err(format!(
            "You've read {read}, so you can't propose access to {person}'s apps: what you read there may be someone else's instructions. Nothing was proposed."
        ))
    }
}
