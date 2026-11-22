//! Extensions: capabilities an agent can have besides its coding tools, such
//! as messaging other agents or keeping a checklist. Each is on or off per
//! agent, with options, and contributes a prompt section, tools and possibly a
//! panel. The runner only loops over the agent's enabled extensions.
//!
//! They're built in for now; a manifest says everything Factory needs to show
//! one, so extensions from elsewhere can fit the same shape later.
//!
//! Settings live in config.json under each agent's `extensions`, by extension
//! id. Every key is optional and takes the extension's default when absent.
//!
//! An extension can also declare instances: named second uses of it, each on
//! or off per agent on its own, such as "memory:factory", Factory's manual
//! as a read-only memory. An instance's key, "<extension id>:<name>", stands
//! for it everywhere an extension id does: in config.json, panels and the
//! protocol. The extension serves its instances with the same tools.

pub(crate) mod agents;
pub(crate) mod app_grants;
pub(crate) mod ask;
pub(crate) mod browser;
pub(crate) mod checklist;
pub(crate) mod comments;
pub(crate) mod database;
pub(crate) mod devices;
pub(crate) mod handoff;
pub(crate) mod highlight;
pub(crate) mod memory;
pub(crate) mod monitors;
pub(crate) mod moving;
pub(crate) mod nudge;
pub(crate) mod pipes;
pub(crate) mod resources;
pub(crate) mod secrets;
pub(crate) mod swarm;
pub(crate) mod tasks;
pub(crate) mod tree;
pub(crate) mod viewer;
pub(crate) mod web;

use std::sync::Arc;

use factory_agent::{Tool, ToolResult};
use factory_protocol::v1;
use serde_json::{Map, Value, json};

use crate::config::{AgentConfig, Config, Invalid};
use crate::factory::{Factory, Inner, RequestError};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OptionKind {
    /// On or off.
    Switch,
    /// A list of agent ids.
    Agents,
    /// One of a list: each choice's value (kept in config.json) and label,
    /// in the order shown, and the value in effect until someone chooses.
    Choice { choices: &'static [(&'static str, &'static str)], default: &'static str },
}

pub struct OptionSpec {
    pub id: &'static str,
    pub label: &'static str,
    pub description: &'static str,
    pub kind: OptionKind,
    /// A switch's default; agent lists start empty.
    pub default_on: bool,
    /// An agent list that always counts the agent's own project's agents.
    pub includes_project: bool,
}

pub struct Manifest {
    pub id: &'static str,
    pub name: &'static str,
    /// A Lucide icon name.
    pub icon: &'static str,
    pub description: &'static str,
    pub default_enabled: bool,
    pub options: &'static [OptionSpec],
}

/// When a prompt section is written.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PromptKind {
    /// The same text whenever it's written. It must not hold live data
    /// (statuses, items). Like the whole system prompt, it's written when a
    /// segment starts and kept until the next (see `transcript::Frame`).
    Static,
    /// Written when a segment starts (its first run with the extension on,
    /// and right after each compaction), kept with the segment and reused
    /// until the next compaction, so it can hold data without breaking the
    /// cache. Each extension has at most one.
    SegmentPinned,
    /// Like `SegmentPinned`, for one of the extension's instances, kept by
    /// its key; at most one for each.
    InstancePinned(&'static str),
}

pub struct PromptSection {
    pub kind: PromptKind,
    pub text: String,
}

/// Who has an instance unless it's turned off for them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InstanceDefault {
    /// Admin agents (Messaging's `admin`), such as Rivet.
    Admin,
}

/// A named second use of an extension, on or off per agent on its own.
pub struct InstanceSpec {
    /// "<extension id>:<name>", e.g. "memory:factory": how config.json,
    /// panels and clients name it.
    pub key: &'static str,
    /// e.g. "factory": what tools' `instance` parameter takes.
    pub name: &'static str,
    /// Shown for it, e.g. "Factory manual".
    pub label: &'static str,
    /// A Lucide icon name.
    pub icon: &'static str,
    pub description: &'static str,
    pub default: InstanceDefault,
}

pub(crate) trait Extension: Send + Sync {
    fn manifest(&self) -> &'static Manifest;
    /// What the agent's system prompt says about it.
    fn prompt(&self, factory: &Factory, inner: &Inner, agent: &AgentConfig) -> Vec<PromptSection>;
    /// Its tools, named `<id>_<verb>`.
    fn tools(&self, factory: &Arc<Factory>, agent: &str) -> Vec<Tool>;
    /// Admin agents (Messaging's `admin`) have it unless it's turned off,
    /// whatever `default_enabled` says for others.
    fn admin_default(&self) -> bool {
        false
    }
    /// The tool is one of its own.
    fn owns_tool(&self, name: &str) -> bool {
        let id = self.manifest().id;
        name == id || name.strip_prefix(id).is_some_and(|verb| verb.starts_with('_'))
    }
    /// Not part of this Claktory now: no agent has it, and no one sees it.
    fn hidden(&self) -> bool {
        false
    }
    /// Its panel's content for the agent; None if it has no panel.
    fn view(&self, _factory: &Factory, _inner: &Inner, _agent: &str) -> Option<v1::ExtensionView> {
        None
    }
    /// Its instances. Its prompt and tools serve those the agent has on, as
    /// well as itself (which may be off while an instance is on).
    fn instances(&self) -> &'static [InstanceSpec] {
        &[]
    }
    /// An instance's panel for the agent; None if it has none.
    fn instance_view(
        &self,
        _factory: &Factory,
        _inner: &Inner,
        _agent: &str,
        _instance: &'static InstanceSpec,
    ) -> Option<v1::ExtensionView> {
        None
    }
}

/// An extension, or one of its instances: what an agent has on or off.
#[derive(Clone, Copy)]
pub(crate) struct Unit {
    pub extension: &'static dyn Extension,
    pub instance: Option<&'static InstanceSpec>,
}

impl Unit {
    pub(crate) fn key(&self) -> &'static str {
        self.instance.map_or(self.extension.manifest().id, |instance| instance.key)
    }

    pub(crate) fn name(&self) -> &'static str {
        self.instance.map_or(self.extension.manifest().name, |instance| instance.label)
    }

    /// Whether `agent` has it when its settings don't say.
    fn default_for(&self, agent: &AgentConfig) -> bool {
        match self.instance.map(|instance| instance.default) {
            None => self.extension.manifest().default_enabled || (self.extension.admin_default() && switch(agent, agents::ID, "admin")),
            Some(InstanceDefault::Admin) => switch(agent, agents::ID, "admin"),
        }
    }
}

/// Every extension, each followed by its instances.
pub(crate) fn units() -> impl Iterator<Item = Unit> {
    ALL.into_iter().filter(|extension| !extension.hidden()).flat_map(|extension| {
        std::iter::once(Unit { extension, instance: None })
            .chain(extension.instances().iter().map(move |instance| Unit { extension, instance: Some(instance) }))
    })
}

/// The extension or instance with this key.
pub(crate) fn unit(key: &str) -> Option<Unit> {
    units().find(|unit| unit.key() == key)
}

/// The agent has the extension, or one of its instances, on.
pub(crate) fn active(agent: &AgentConfig, extension: &dyn Extension) -> bool {
    enabled(agent, extension.manifest().id) || extension.instances().iter().any(|instance| enabled(agent, instance.key))
}

/// The panel `key` (an extension or an instance) shows for the agent.
pub(crate) fn view(factory: &Factory, inner: &Inner, agent: &str, key: &str) -> Option<v1::ExtensionView> {
    let unit = unit(key)?;
    match unit.instance {
        None => unit.extension.view(factory, inner, agent),
        Some(instance) => unit.extension.instance_view(factory, inner, agent, instance),
    }
}

/// Every extension, sorted by id, which is also the order of their prompt
/// sections and tools.
pub(crate) const ALL: [&dyn Extension; 22] = [
    &agents::Agents,
    &app_grants::AppGrants,
    &ask::Ask,
    &browser::Browser,
    &checklist::Checklist,
    &comments::Comments,
    &database::Database,
    &devices::Devices,
    &crate::user_apps::dms::Dms,
    &handoff::Handoff,
    &highlight::Highlight,
    &memory::Memory,
    &monitors::Monitors,
    &moving::Move,
    &pipes::Pipes,
    &resources::Resources,
    &secrets::Secrets,
    &swarm::Swarm,
    &tasks::Tasks,
    &tree::Tree,
    &viewer::Viewer,
    &web::Web,
];

/// An extension's tool. Its schema isn't enforced while sampling (strict
/// tools): providers cap how many optional parameters strict tools may have
/// in all, and extensions would soon exceed it. So optional parameters are
/// plain properties left out of `required`.
pub(crate) fn tool(
    name: &'static str,
    label: &'static str,
    description: &'static str,
    parameters: Value,
    handler: factory_agent::Handler,
) -> Tool {
    let mut tool = Tool::custom(name, label, description, parameters, handler);
    tool.spec.constrained_sampling = Value::Null;
    tool
}

/// The tool's calling agent, or the usual error if it was removed.
pub(crate) fn caller<'a>(config: &'a Config, id: &str) -> Result<&'a AgentConfig, String> {
    config.agent(id).ok_or_else(|| "You're no longer in this Claktory.".to_owned())
}

/// A captured Factory, or the usual shutdown refusal at the caller's check.
pub(crate) fn live(factory: Option<Arc<Factory>>) -> Result<Arc<Factory>, factory_agent::Error> {
    factory.ok_or_else(|| factory_agent::Error::Other("Claktory is stopping.".into()))
}

/// Captures Factory weakly, upgrading and cloning the agent at invocation, before polling.
/// Callers keep their own validation and shutdown-check ordering.
pub(crate) fn agent_handler<F>(
    factory: &Arc<Factory>,
    agent: &str,
    run: impl Fn(Option<Arc<Factory>>, String, Value) -> F + Send + Sync + 'static,
) -> factory_agent::Handler
where
    F: std::future::Future<Output = Result<ToolResult, factory_agent::Error>> + Send + 'static,
{
    let factory = Arc::downgrade(factory);
    let agent = agent.to_owned();
    Arc::new(move |args| Box::pin(run(factory.upgrade(), agent.clone(), args)))
}

/// Binds a typed handler without keeping Factory alive; validates before checking shutdown.
pub(crate) fn typed_handler<A: serde::de::DeserializeOwned + 'static>(
    factory: &Arc<Factory>,
    agent: &str,
    run: fn(&Arc<Factory>, &str, A) -> Result<ToolResult, String>,
) -> factory_agent::Handler {
    agent_handler(factory, agent, move |factory, agent, args| async move {
        let args = serde_json::from_value(args)?;
        let factory = live(factory)?;
        run(&factory, &agent, args).map_err(factory_agent::Error::Other)
    })
}

/// What a tool's handler returns: its text alone, or a whole result, with
/// the `details` the app's view of the call draws from.
pub(crate) trait Reply: Send + 'static {
    fn result(self) -> ToolResult;
}

impl Reply for String {
    fn result(self) -> ToolResult {
        ToolResult::text(self, json!({}))
    }
}

impl Reply for ToolResult {
    fn result(self) -> ToolResult {
        self
    }
}

/// Binds a synchronous tool to an agent without keeping Factory alive.
fn factory_tool<R: Reply>(
    factory: &Arc<Factory>,
    agent: &str,
    name: &'static str,
    label: &'static str,
    description: &'static str,
    parameters: Value,
    run: fn(&Factory, &str, Value) -> Result<R, String>,
) -> Tool {
    tool(
        name,
        label,
        description,
        parameters,
        agent_handler(factory, agent, move |factory, agent, args| async move {
            let factory = live(factory)?;
            run(&factory, &agent, args).map(Reply::result).map_err(factory_agent::Error::Other)
        }),
    )
}

pub(crate) fn find(id: &str) -> Option<&'static dyn Extension> {
    ALL.into_iter().find(|extension| extension.manifest().id == id)
}

/// The agent's settings for one extension, as written in config.json.
fn settings<'a>(agent: &'a AgentConfig, extension: &str) -> Option<&'a Map<String, Value>> {
    agent.extensions.get(extension)
}

/// Whether the agent has the extension or instance `key` on.
pub(crate) fn enabled(agent: &AgentConfig, key: &str) -> bool {
    let Some(unit) = unit(key) else { return false };
    settings(agent, key).and_then(|settings| settings.get("enabled")).and_then(Value::as_bool).unwrap_or_else(|| unit.default_for(agent))
}

fn option_spec(extension: &str, option: &str) -> Option<&'static OptionSpec> {
    find(extension)?.manifest().options.iter().find(|spec| spec.id == option)
}

/// A switch's effective value.
pub(crate) fn switch(agent: &AgentConfig, extension: &str, option: &str) -> bool {
    let default = option_spec(extension, option).is_some_and(|spec| spec.default_on);
    settings(agent, extension).and_then(|settings| settings.get(option)).and_then(Value::as_bool).unwrap_or(default)
}

/// A choice's effective value: as written if it's one of the choices, else
/// its default.
pub(crate) fn choice(agent: &AgentConfig, extension: &str, option: &str) -> &'static str {
    let Some(OptionKind::Choice { choices, default }) = option_spec(extension, option).map(|spec| spec.kind) else { return "" };
    let written = settings(agent, extension).and_then(|settings| settings.get(option)).and_then(Value::as_str);
    choices.iter().find(|(value, _)| Some(*value) == written).map_or(default, |(value, _)| value)
}

/// An agent list's ids, as written (ids of agents that are gone included).
pub(crate) fn agent_list(agent: &AgentConfig, extension: &str, option: &str) -> Vec<String> {
    let listed = settings(agent, extension).and_then(|settings| settings.get(option)).and_then(Value::as_array);
    listed.into_iter().flatten().filter_map(Value::as_str).map(str::to_owned).collect()
}

/// Rejects settings Factory can't read, such as a switch that isn't true or false.
pub(crate) fn validate(agent: &AgentConfig) -> Result<(), Invalid> {
    for (id, settings) in &agent.extensions {
        let Some(unit) = unit(id) else { continue };
        for (key, value) in settings {
            let fine = match (key.as_str(), option_spec(id, key).map(|spec| spec.kind)) {
                ("enabled", _) | (_, Some(OptionKind::Switch)) => value.is_boolean(),
                (_, Some(OptionKind::Agents)) => value.as_array().is_some_and(|ids| ids.iter().all(Value::is_string)),
                (_, Some(OptionKind::Choice { choices, .. })) => {
                    value.as_str().is_some_and(|value| choices.iter().any(|(choice, _)| *choice == value))
                }
                (_, None) => true,
            };
            if !fine {
                let expected = match option_spec(id, key).map(|spec| spec.kind) {
                    Some(OptionKind::Agents) => "a list of agent ids".to_owned(),
                    Some(OptionKind::Choice { choices, .. }) => {
                        let values: Vec<String> = choices.iter().map(|(value, _)| format!("\"{value}\"")).collect();
                        format!("one of {}", values.join(", "))
                    }
                    _ => "true or false".to_owned(),
                };
                return Err(Invalid::BadExtensionSetting { agent: agent.id.clone(), setting: format!("{}.{key}", unit.key()), expected });
            }
        }
    }
    Ok(())
}

/// Settings Factory doesn't know, which it keeps but ignores: an extension
/// that isn't installed, or an option it doesn't have.
pub(crate) fn unknown_settings(config: &Config) -> Vec<String> {
    let mut unknown = Vec::new();
    for agent in &config.agents {
        for (id, settings) in &agent.extensions {
            if unit(id).is_none() {
                unknown.push(format!("agent \"{}\": no extension \"{id}\"", agent.id));
                continue;
            }
            for key in settings.keys() {
                if key != "enabled" && option_spec(id, key).is_none() {
                    unknown.push(format!("agent \"{}\": extension \"{id}\" has no option \"{key}\"", agent.id));
                }
            }
        }
    }
    unknown
}

/// Every extension, each followed by its instances, for clients.
pub(crate) fn catalog() -> Vec<v1::Extension> {
    let mut extensions = Vec::new();
    for unit in units() {
        let manifest = unit.extension.manifest();
        if let Some(instance) = unit.instance {
            extensions.push(v1::Extension {
                id: instance.key.to_owned(),
                name: instance.label.to_owned(),
                description: instance.description.to_owned(),
                default_enabled: false,
                options: Vec::new(),
                icon: instance.icon.to_owned(),
                base: manifest.id.to_owned(),
                admin_default: instance.default == InstanceDefault::Admin,
            });
            continue;
        }
        let mut options = Vec::new();
        for spec in manifest.options {
            let (kind, choices, default_choice) = match spec.kind {
                OptionKind::Switch => (v1::ExtensionOptionKind::Switch, Vec::new(), String::new()),
                OptionKind::Agents => (v1::ExtensionOptionKind::Agents, Vec::new(), String::new()),
                OptionKind::Choice { choices, default } => {
                    let choices = choices
                        .iter()
                        .map(|(value, label)| v1::OptionChoice { value: (*value).to_owned(), label: (*label).to_owned() })
                        .collect();
                    (v1::ExtensionOptionKind::Choice, choices, default.to_owned())
                }
            };
            options.push(v1::ExtensionOption {
                id: spec.id.to_owned(),
                label: spec.label.to_owned(),
                description: spec.description.to_owned(),
                default_value: spec.default_on,
                kind: kind.into(),
                includes_project: spec.includes_project,
                choices,
                default_choice,
            });
        }
        extensions.push(v1::Extension {
            id: manifest.id.to_owned(),
            name: manifest.name.to_owned(),
            description: manifest.description.to_owned(),
            default_enabled: manifest.default_enabled,
            options,
            icon: manifest.icon.to_owned(),
            base: String::new(),
            admin_default: unit.extension.admin_default(),
        });
    }
    extensions
}

/// The agent's extensions with their effective values, for clients. Agent
/// lists hold only agents that exist.
pub(crate) fn of_agent(config: &Config, agent: &AgentConfig) -> Vec<v1::AgentExtension> {
    let mut extensions = Vec::new();
    for unit in units() {
        let manifest = unit.extension.manifest();
        let mut options = std::collections::BTreeMap::new();
        for spec in manifest.options.iter().filter(|_| unit.instance.is_none()) {
            let value = match spec.kind {
                OptionKind::Switch => v1::OptionValue { on: switch(agent, manifest.id, spec.id), ..Default::default() },
                OptionKind::Agents => {
                    let mut ids = agent_list(agent, manifest.id, spec.id);
                    ids.retain(|id| config.agent(id).is_some());
                    v1::OptionValue { agent_ids: ids, ..Default::default() }
                }
                OptionKind::Choice { .. } => {
                    v1::OptionValue { choice: choice(agent, manifest.id, spec.id).to_owned(), ..Default::default() }
                }
            };
            options.insert(spec.id.to_owned(), value);
        }
        extensions.push(v1::AgentExtension { id: unit.key().to_owned(), enabled: enabled(agent, unit.key()), options });
    }
    extensions
}

/// Applies a client's change to an agent's settings. A value equal to the
/// default is left out of config.json, so the file only holds choices.
pub(crate) fn apply_change(config: &mut Config, agent_id: &str, change: &v1::ExtensionChange) -> Result<(), RequestError> {
    let Some(unit) = unit(&change.id) else { return Err(RequestError::UnknownExtension(change.id.clone())) };
    let manifest = unit.extension.manifest();
    let spec = if change.option_id.is_empty() {
        None
    } else {
        // Instances have no options.
        let spec = manifest.options.iter().find(|spec| spec.id == change.option_id && unit.instance.is_none());
        Some(spec.ok_or_else(|| RequestError::UnknownOption { extension: change.id.clone(), option: change.option_id.clone() })?)
    };
    let mut list = None;
    if let Some(spec) = spec
        && spec.kind == OptionKind::Agents
    {
        let value = change.value.clone().unwrap_or_default();
        for id in &value.agent_ids {
            if config.agent(id).is_none() {
                return Err(RequestError::UnknownAgent(id.clone()));
            }
        }
        list = Some(value.agent_ids);
    }
    if let Some(spec) = spec
        && let OptionKind::Choice { choices, .. } = spec.kind
    {
        let chosen = change.value.as_ref().map(|value| value.choice.as_str()).unwrap_or_default();
        if !choices.iter().any(|(value, _)| *value == chosen) {
            let values: Vec<&str> = choices.iter().map(|(value, _)| *value).collect();
            return Err(RequestError::Invalid(Invalid::BadExtensionSetting {
                agent: agent_id.to_owned(),
                setting: format!("{}.{}", unit.key(), spec.id),
                expected: format!("one of {}", values.join(", ")),
            }));
        }
    }
    let Some(agent) = config.agent_mut(agent_id) else { return Err(RequestError::UnknownAgent(agent_id.to_owned())) };
    let default = unit.default_for(agent);
    let settings = agent.extensions.entry(unit.key().to_owned()).or_default();
    if let Some(enabled) = change.enabled {
        set(settings, "enabled", Value::Bool(enabled), enabled == default);
    }
    if let Some(spec) = spec {
        match spec.kind {
            OptionKind::Switch => {
                let on = change.value.as_ref().is_some_and(|value| value.on);
                set(settings, spec.id, Value::Bool(on), on == spec.default_on);
            }
            OptionKind::Agents => {
                let ids = list.unwrap_or_default();
                let empty = ids.is_empty();
                set(settings, spec.id, Value::Array(ids.into_iter().map(Value::String).collect()), empty);
            }
            OptionKind::Choice { default, .. } => {
                let chosen = change.value.as_ref().map(|value| value.choice.clone()).unwrap_or_default();
                let is_default = chosen == default;
                set(settings, spec.id, Value::String(chosen), is_default);
            }
        }
    }
    if settings.is_empty() {
        agent.extensions.remove(unit.key());
    }
    Ok(())
}

fn set(settings: &mut Map<String, Value>, key: &str, value: Value, is_default: bool) {
    if is_default {
        settings.remove(key);
    } else {
        settings.insert(key.to_owned(), value);
    }
}

/// An agent is gone: no agent list names it any more.
pub(crate) fn forget_agent(config: &mut Config, gone: &str) {
    for agent in &mut config.agents {
        for (id, settings) in &mut agent.extensions {
            settings.retain(|key, value| {
                if option_spec(id, key).is_some_and(|spec| spec.kind == OptionKind::Agents)
                    && let Some(ids) = value.as_array_mut()
                {
                    ids.retain(|listed| listed.as_str() != Some(gone));
                    return !ids.is_empty();
                }
                true
            });
        }
        agent.extensions.retain(|_, settings| !settings.is_empty());
    }
}
