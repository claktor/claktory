//! Runs agents: one task per working agent, working through its queued
//! messages. Messages that arrive while it works reach it between steps.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use factory_agent::compaction::Reason;
use factory_agent::{
    Agent, AgentEvent, AgentOptions, Arrived, Background, Event, EventSource, Finished, Inbox, Message, StopReason, Task, ThinkingLevel,
    Tool, ToolSpec, UserContent, UserMessage,
};
use factory_protocol::v1::{self, NoticeLevel};

use crate::config::{AgentConfig, Config, ProjectConfig};
use crate::extensions::{Extension, PromptKind};
use crate::factory::{Factory, Inner, Queued};
use crate::home::Home;
use crate::store::Outcome;
use crate::transcript::{Change, Frame};

/// Files whose contents become an agent's project instructions, first found wins.
const INSTRUCTION_FILES: [&str; 2] = ["AGENTS.md", "CLAUDE.md"];

struct Job {
    prompts: Vec<Queued>,
    agent: AgentConfig,
    compact_at: u64,
    messages: Vec<Message>,
    models: Vec<v1::Model>,
    /// The agent's extensions that are on, in order.
    extensions: Vec<&'static dyn Extension>,
    /// The segment's system prompt and tools, unless there are none yet or
    /// the agent's own settings changed since.
    frame: Option<Frame>,
    /// Where it works and what else it may reach, as things are now.
    places: Places,
    /// The conversation it belongs to (see `Runtime::epoch`).
    epoch: u64,
    /// Compact before anything else: /compact, or idle (`crate::idle`).
    compact: Option<Reason>,
    /// How long its model's prompt cache is asked to last.
    cache_ttl: factory_agent::CacheTtl,
}

/// What a system prompt is written from.
struct Render<'a> {
    agent: &'a AgentConfig,
    project: Option<&'a ProjectConfig>,
    compact_at: u64,
    models: &'a [v1::Model],
    /// The extensions' sections written for every run…
    sections: &'a [String],
    /// …and those fixed for the segment.
    pinned: &'a [String],
    /// The name of the project folder it works in, if it's in one.
    folder_name: Option<&'a str>,
    /// The project's folders, when it has any (see `Frame::folders`).
    folders: &'a BTreeMap<String, String>,
    /// The user's name, when it's set.
    user: Option<&'a str>,
    /// The vault's secrets it may use (see `Frame::secrets`).
    secrets: &'a BTreeMap<String, String>,
    /// Whether what it publishes credits Claktory (`Config::attribution`),
    /// as the settings say when its context starts.
    attribution: bool,
}

/// Where an agent works, and what else its file tools may reach: its own
/// folder in the Factory home (uploads, its tasks' logs, its archives) and,
/// for an admin agent or one given the Claktory manual, `config.json`, which
/// they may change; and, only to read, the project's other folders, the
/// checkouts and what pipes brought in. The rest of the home — `auth.json`,
/// `factory.db`, the vault, other agents' folders — isn't supplied at all,
/// so the file tools refuse it (the same-user shell can still reach it; this
/// is a guardrail, not a sandbox).
pub(crate) struct Places {
    pub(crate) folder: PathBuf,
    pub(crate) extra_dirs: Vec<PathBuf>,
    pub(crate) read_only: Vec<PathBuf>,
}

/// The folder `agent` works in, unless it has a checkout of its own: its
/// project folder, its project's own folder, or the workspace.
pub(crate) fn source_folder(home: &Home, config: &Config, agent: &AgentConfig) -> PathBuf {
    let project = agent.project.as_deref().and_then(|project| config.project(project));
    match (project, config.agent_folder(agent)) {
        (_, Some(folder)) => folder.path.clone(),
        (Some(project), None) => project.path.clone(),
        (None, None) => home.workspace(),
    }
}

/// The agent's own checkout, when its "tree" extension is on and the
/// checkout was made (see `Factory::ensure_checkout`).
pub(crate) fn checkout(home: &Home, config: &Config, agent: &AgentConfig) -> Option<crate::trees::Checkout> {
    if !crate::extensions::enabled(agent, crate::extensions::tree::ID) {
        return None;
    }
    let checkout = crate::trees::plan(home, &agent.id, &source_folder(home, config, agent)).ok()?;
    checkout.exists().then_some(checkout)
}

/// The outboxes of the pipes `agent` may use: it sends a pipe's files from
/// there ("Files you send come from its outbox"), so `places` keeps them
/// writable even when the pipe's project folder is otherwise read only for
/// a folder-scoped agent.
pub(crate) fn pipe_outboxes(factory: &crate::factory::Factory, config: &Config, agent: &str) -> Vec<PathBuf> {
    factory
        .pipes
        .all()
        .into_iter()
        .filter(|record| record.granted() && record.members.iter().any(|member| member == agent))
        .filter_map(|record| crate::pipes::folder_of(config, &record).map(|folder| folder.join("outbox")))
        .collect()
}

/// `pipe_outboxes` doesn't change `folder`: call sites after only that pass none.
pub(crate) fn places(home: &Home, config: &Config, agent: &AgentConfig, pipe_outboxes: &[PathBuf]) -> Places {
    let project = agent.project.as_deref().and_then(|project| config.project(project));
    let source = source_folder(home, config, agent);
    let mut read_only = Vec::new();
    if let Some(project) = project.filter(|project| !project.folders.is_empty()) {
        for path in std::iter::once(&project.path).chain(project.folders.iter().map(|folder| &folder.path)) {
            if *path != source && !read_only.contains(path) {
                read_only.push(path.clone());
            }
        }
    }
    // Other agents' checkouts; its own is more specific, so it stays writable.
    read_only.push(crate::trees::trees_dir(home));
    // Files other Claktories' agents sent through pipes, which pipe_read
    // hands out by path.
    read_only.push(home.root().join("pipes").join("incoming"));
    // With a checkout of its own, the main checkout is only to read.
    let folder = match checkout(home, config, agent) {
        Some(checkout) => {
            read_only.push(checkout.repo);
            checkout.folder
        }
        None => source,
    };
    let mut extra_dirs = vec![home.agent_dir(&agent.id)];
    if crate::proposals::is_admin(agent) || crate::extensions::enabled(agent, "memory:factory") {
        // The manual tells these agents to change Claktory's settings by
        // editing config.json with their file tools.
        extra_dirs.push(home.config_path());
    }
    // More specific than a read-only project folder, so the most-specific
    // rule lets the outbox be written inside it.
    extra_dirs.extend(pipe_outboxes.iter().cloned());
    Places { folder, extra_dirs, read_only }
}

/// Where `agent` works and what its file tools reach: its pipes' folders
/// alone while it's quarantined for one (`pipes::quarantine`), `places`
/// otherwise.
pub(crate) fn placed(factory: &crate::factory::Factory, config: &Config, agent: &AgentConfig) -> Places {
    if factory.pipes.curator(&agent.id) {
        return crate::pipes::curator::places(factory, &agent.id);
    }
    match crate::pipes::quarantine::quarantine(factory, config, &agent.id) {
        Some(quarantine) => quarantine.places(),
        None => places(&factory.home, config, agent, &pipe_outboxes(factory, config, &agent.id)),
    }
}

/// The folders of `agent`'s project, when it has any: folder id ("" for the
/// project's own) → "Research: /path (read only)", or "(yours)" for the
/// agent's.
fn folder_lines(config: &Config, agent: &AgentConfig) -> BTreeMap<String, String> {
    let Some(project) = agent.project.as_deref().and_then(|project| config.project(project)) else { return BTreeMap::new() };
    if project.folders.is_empty() {
        return BTreeMap::new();
    }
    let own = config.agent_folder(agent).map_or("", |folder| folder.id.as_str());
    let whose = |id: &str| if id == own { "yours" } else { "read only" };
    let mut lines = BTreeMap::from([(String::new(), format!("The project's own folder: {} ({})", project.path.display(), whose("")))]);
    for folder in &project.folders {
        lines.insert(folder.id.clone(), format!("{}: {} ({})", folder.name, folder.path.display(), whose(&folder.id)));
    }
    lines
}

/// The agent's own settings that its system prompt and tools follow. When
/// they change, the agent is told (`settings_note`); its frame follows them
/// at the next segment, or the next model call when its model, instructions,
/// name or where it works changed (`settings_now`).
fn settings_key(config: &Config, agent: &AgentConfig, cwd: &Path, quarantined: bool, curator: bool) -> String {
    let mut key = serde_json::json!({
        "name": agent.name,
        "prompt": agent.prompt,
        "model": agent.model,
        "effort": agent.effort,
        "project": agent.project,
        // Where it works: its folder moving to another path is a new frame too.
        "folder": config.agent_folder(agent).map(|folder| &folder.path),
        "extensions": agent.extensions,
        // What it has on, defaults included, so a new default is news too.
        "on": crate::extensions::units().map(|unit| unit.key()).filter(|key| crate::extensions::enabled(agent, key)).collect::<Vec<_>>(),
        // Where its tools start: its folder, or its own checkout.
        "cwd": cwd,
    });
    // Only when it has one, so agents without a role were told all there is.
    if !agent.role.is_empty() {
        key["role"] = agent.role.clone().into();
    }
    // The user's name, only when it's set, the same way.
    if let Some(name) = config.user_name() {
        key["user"] = name.into();
    }
    // Quarantined for a pipe, the same way: its tools and places change.
    if quarantined {
        key["quarantined"] = true.into();
    }
    // A curator, the same way.
    if curator {
        key["curator"] = true.into();
    }
    key.to_string()
}

/// bash, taking `secrets` (see `factory_agent::tools::secrets`).
fn takes_secrets(tool: &ToolSpec) -> bool {
    tool.name == "bash" && tool.parameters.get("properties").and_then(|properties| properties.get("secrets")).is_some()
}

/// The models the system prompt lists, by id.
fn model_lines(models: &[v1::Model]) -> BTreeMap<String, String> {
    models.iter().map(|model| (model.id.clone(), format!("{} ({})", model.id, model.name))).collect()
}

fn enabled_extensions(agent: &AgentConfig) -> Vec<&'static dyn Extension> {
    crate::extensions::ALL.into_iter().filter(|extension| crate::extensions::active(agent, *extension)).collect()
}

/// The agent's extensions that are on, but for a contained agent only those
/// it keeps (a liaison's quarantine: Ask's and the Hand-off's; a curator:
/// those and its own database, always): their tools, extension_call's reach
/// and their prompts.
fn extensions_of(factory: &crate::factory::Factory, agent: &AgentConfig) -> Vec<&'static dyn Extension> {
    if factory.pipes.curator(&agent.id) {
        let kept = crate::pipes::curator::KEPT_EXTENSIONS;
        return crate::extensions::ALL
            .into_iter()
            .filter(|extension| kept.contains(&extension.manifest().id))
            .filter(|extension| {
                extension.manifest().id == crate::extensions::database::ID || crate::extensions::active(agent, *extension)
            })
            .collect();
    }
    let mut extensions = enabled_extensions(agent);
    if factory.pipes.quarantined(&agent.id) {
        extensions.retain(|extension| crate::pipes::quarantine::KEPT_EXTENSIONS.contains(&extension.manifest().id));
    }
    extensions
}

/// What changed that the agent was told otherwise, in one note; None if nothing.
fn changes(
    frame: &Frame,
    roster: &BTreeMap<String, String>,
    models: &BTreeMap<String, String>,
    folders: &BTreeMap<String, String>,
    secrets: &BTreeMap<String, String>,
) -> Option<String> {
    let mut said = Vec::new();
    let list = |now: &BTreeMap<String, String>, then: &BTreeMap<String, String>| -> (Vec<String>, Vec<String>) {
        let gained = now.iter().filter(|(id, label)| then.get(*id) != Some(*label)).map(|(_, label)| label.clone()).collect();
        let lost = then.iter().filter(|(id, _)| !now.contains_key(*id)).map(|(_, label)| label.clone()).collect();
        (gained, lost)
    };
    let (gained, lost) = list(roster, &frame.roster);
    if !gained.is_empty() {
        said.push(format!("You can now message: {}.", gained.join(", ")));
    }
    if !lost.is_empty() {
        said.push(format!("You can no longer message: {}.", lost.join(", ")));
    }
    let (gained, lost) = list(models, &frame.models);
    if !gained.is_empty() {
        said.push(format!("Models now available: {}.", gained.join(", ")));
    }
    if !lost.is_empty() {
        said.push(format!("Models no longer available: {}.", lost.join(", ")));
    }
    let (gained, lost) = list(folders, &frame.folders);
    if !gained.is_empty() {
        said.push(format!("New or changed project folders: {}.", gained.join("; ")));
    }
    if !lost.is_empty() {
        said.push(format!("Project folders removed: {}.", lost.join("; ")));
    }
    let (gained, _) = list(secrets, &frame.secrets);
    if !gained.is_empty() {
        said.push(format!("You can now use the secrets (pass names in bash's `secrets`): {}.", gained.join("; ")));
    }
    // Taken back or deleted: by name, as it was known.
    let lost: Vec<String> = frame
        .secrets
        .iter()
        .filter(|(id, _)| !secrets.contains_key(*id))
        .map(|(_, label)| label.split(" (sets").next().unwrap_or_default().to_owned())
        .collect();
    if !lost.is_empty() {
        said.push(format!("You can no longer use the secrets: {}.", lost.join(", ")));
    }
    (!said.is_empty()).then(|| said.join("\n"))
}

/// The apps given or taken back since the agent was told, in one note; None
/// if none. `sections` are the apps' sections of its system prompt, by id.
fn app_changes(frame: &Frame, apps: &BTreeMap<String, String>, sections: &BTreeMap<String, String>) -> Option<String> {
    let mut said = Vec::new();
    let gained: Vec<&String> = apps.iter().filter(|(id, label)| frame.apps.get(*id) != Some(*label)).map(|(id, _)| id).collect();
    if !gained.is_empty() {
        let labels: Vec<&str> = gained.iter().map(|id| apps[*id].as_str()).collect();
        let mut text = format!(
            "You can now use the apps: {}. What they're for, which your system prompt has from your next context:",
            labels.join("; ")
        );
        for id in gained {
            if let Some(section) = sections.get(id) {
                text.push_str("\n\n");
                text.push_str(section);
            }
        }
        said.push(text);
    }
    let lost: Vec<String> = frame
        .apps
        .iter()
        .filter(|(id, _)| !apps.contains_key(*id))
        .map(|(_, label)| label.split(" (").next().unwrap_or_default().to_owned())
        .collect();
    if !lost.is_empty() {
        said.push(format!("You can no longer use the apps: {}.", lost.join(", ")));
    }
    (!said.is_empty()).then(|| said.join("\n"))
}

/// The tools a frame lists, with its specs, in its order. Tools it doesn't
/// list wait for the next segment; those it lists that are gone, such as an
/// extension's that was turned off, are refused, saying why.
fn fit(factory: &Factory, mut tools: Vec<Tool>, specs: &[ToolSpec], agent: &AgentConfig) -> Vec<Tool> {
    let mut fitted = Vec::with_capacity(specs.len());
    // Still on a pipe, only sending to each (#414, or reading given up).
    let sends = tools.iter().any(|tool| tool.spec.name == "pipe_send");
    for spec in specs {
        let mut tool = match tools.iter().position(|tool| tool.spec.name == spec.name) {
            Some(index) => tools.swap_remove(index),
            None => refused(spec, agent, crate::apps::tool_app(factory, &spec.name), sends),
        };
        tool.spec = spec.clone();
        fitted.push(tool);
    }
    fitted
}

/// A tool of the frame that's gone: calling it says why. `sends`: the
/// agent still sends to a pipe.
fn refused(spec: &ToolSpec, agent: &AgentConfig, app: Option<String>, sends: bool) -> Tool {
    let name = spec.name.to_string();
    let owner = crate::extensions::ALL.into_iter().find(|extension| extension.owns_tool(&name)).map(|extension| extension.manifest());
    let reason = match owner {
        _ if app.is_some() => {
            format!("You no longer have {name} of the {} app, so it doesn't work any more.", app.unwrap_or_default())
        }
        _ if name.starts_with("pipe_") && sends => {
            format!("You only send to your pipes now, so {name} doesn't work any more. pipe_send still does.")
        }
        _ if name.starts_with("pipe_") => format!("You no longer have a pipe, so {name} doesn't work any more."),
        _ if name == crate::given_up::APPS_TOOL => format!("You no longer have any app, so {name} has nothing to give up."),
        _ if name == crate::given_up::REACH_TOOL => {
            format!("You're no longer let message any agent outside your project, so {name} has nothing to give up.")
        }
        Some(manifest) if !crate::extensions::enabled(agent, manifest.id) => {
            format!("The {} extension is turned off for you, so {name} doesn't work any more.", manifest.name)
        }
        _ => format!("{name} isn't available with your current settings."),
    };
    let handler: factory_agent::Handler = Arc::new(move |_| {
        let reason = reason.clone();
        Box::pin(async move { Err(factory_agent::Error::Other(reason)) })
    });
    Tool::custom(spec.name.clone(), spec.label.clone(), spec.description.clone(), spec.parameters.clone(), handler)
}

/// The model a frame was written for.
pub(crate) fn frame_model(frame: &Frame) -> String {
    let settings: serde_json::Value = serde_json::from_str(&frame.settings).unwrap_or_default();
    settings.get("model").and_then(serde_json::Value::as_str).unwrap_or_default().to_owned()
}

/// What changed in the agent's own settings since `told` (a `settings_key`),
/// said to the agent, and the extensions and instances turned on since; None
/// if nothing changed. `folder` is where it works now; `can_land` says its
/// tools have tree_land; `tools` are the frame's.
fn settings_note(
    told: &str,
    home: &Home,
    inner: &Inner,
    agent: &AgentConfig,
    folder: &Path,
    can_land: bool,
    tools: &[ToolSpec],
) -> Option<(String, Vec<&'static str>)> {
    use crate::extensions::{OptionKind, agent_list, enabled, switch};
    let config = &inner.config;
    let then: serde_json::Value = serde_json::from_str(told).ok()?;
    let text = crate::transcript::text_fields(&then);
    // Turned on, it can use them now if its frame can call tools it doesn't list.
    let call_now = tools.iter().any(|tool| tool.name == EXTENSION_CALL);
    let mut said = Vec::new();
    let mut turned_on = Vec::new();
    if text("model") != agent.model {
        // Who or what changed it (friend bug #43), while it's still their model.
        said.push(match inner.model_why.get(&agent.id).filter(|(model, _)| *model == agent.model) {
            Some((_, why)) => format!("your model is now {} ({why})", agent.model),
            None => format!("your model is now {}", agent.model),
        });
    }
    if text("name") != agent.name {
        said.push(format!("your name is now {}", agent.name));
    }
    if text("role") != agent.role {
        said.push(if agent.role.is_empty() { "you no longer have a role".to_owned() } else { format!("your role is now {}", agent.role) });
    }
    if text("user") != config.user_name().unwrap_or_default() {
        said.push(match config.user_name() {
            Some(name) => format!("the person you work with is {name}, and your system prompt says so from your next context"),
            None => "the name of the person you work with was removed".to_owned(),
        });
    }
    if text("effort") != agent.effort {
        said.push(format!("your effort is now {}", agent.effort));
    }
    let project_then = then.get("project").and_then(serde_json::Value::as_str);
    let moved = project_then != agent.project.as_deref();
    if moved {
        said.push(match agent.project.as_deref().and_then(|project| config.project(project)) {
            Some(project) => format!("you're now in the project \"{}\"", project.name),
            None => "you're now in Claktory's workspace, outside any project".to_owned(),
        });
    }
    let mut before = agent.clone();
    before.extensions = then.get("extensions").cloned().and_then(|value| serde_json::from_value(value).ok()).unwrap_or_default();
    let tree = crate::extensions::tree::ID;
    let checkout_switched = enabled(&before, tree) != enabled(agent, tree);
    let folder_now = serde_json::to_value(config.agent_folder(agent).map(|folder| &folder.path)).unwrap_or_default();
    let cwd_moved = match then.get("cwd").and_then(serde_json::Value::as_str) {
        Some(cwd) => Path::new(cwd) != folder,
        // Told before Factory noted where its tools start: they started in
        // its folder then, so a checkout of its own is new.
        None => {
            moved
                || checkout_switched
                || then.get("folder").cloned().unwrap_or_default() != folder_now
                || folder != source_folder(home, config, agent)
        }
    };
    if cwd_moved {
        said.push(format!("your shell and file tools now start in {}", folder.display()));
    }
    // What it had on: as told, or, told before Factory noted it, what the
    // frame's tools show (instances didn't exist then; Own checkout may
    // have no tool, so its settings say).
    let on: Option<Vec<&str>> =
        then.get("on").and_then(serde_json::Value::as_array).map(|on| on.iter().filter_map(serde_json::Value::as_str).collect());
    let had = |unit: crate::extensions::Unit| match (&on, unit.instance) {
        (Some(on), _) => on.contains(&unit.key()),
        (None, Some(_)) => false,
        (None, None) if unit.key() == tree => enabled(&before, tree),
        (None, None) => enabled(&before, unit.key()) && tools.iter().any(|tool| unit.extension.owns_tool(&tool.name)),
    };
    for unit in crate::extensions::units() {
        let (manifest, key) = (unit.extension.manifest(), unit.key());
        match (had(unit), enabled(agent, key)) {
            (false, true) if call_now => {
                turned_on.push(key);
                said.push(format!("the {} extension is now on, and yours to use now (below)", unit.name()));
            }
            (false, true) => said.push(format!(
                "the {} extension is now on; its tools arrive with your next context (after compaction or /clear)",
                unit.name()
            )),
            (true, false) => said.push(format!("the {} extension is now off; stop using its tools", unit.name())),
            (true, true) => {
                for option in manifest.options.iter().filter(|_| unit.instance.is_none()) {
                    let now = match option.kind {
                        OptionKind::Switch if switch(&before, manifest.id, option.id) != switch(agent, manifest.id, option.id) => {
                            if switch(agent, manifest.id, option.id) { "on".to_owned() } else { "off".to_owned() }
                        }
                        OptionKind::Agents if agent_list(&before, manifest.id, option.id) != agent_list(agent, manifest.id, option.id) => {
                            let ids = agent_list(agent, manifest.id, option.id);
                            if ids.is_empty() { "empty".to_owned() } else { ids.join(", ") }
                        }
                        _ => continue,
                    };
                    said.push(format!("the {} extension's “{}” is now {now}", manifest.name, option.label));
                }
            }
            (false, false) => {}
        }
    }
    let new_instructions = text("prompt") != agent.prompt;
    if said.is_empty() && !new_instructions {
        return None;
    }
    // Its checkout applies now, not with the next context: say what it is.
    let in_checkout = cwd_moved && checkout(home, config, agent).is_some();
    let checkout = (in_checkout || (checkout_switched && enabled(agent, tree))).then(|| {
        let mut section = crate::extensions::tree::section(home, &agent.id, &source_folder(home, config, agent));
        if in_checkout && !can_land {
            let _ = writeln!(
                section,
                "tree_land arrives with your next context (after compaction or /clear); until then, leave landing to {}.",
                config.person()
            );
        }
        section
    });
    let mut note =
        if said.is_empty() { "Your settings changed.".to_owned() } else { format!("Your settings changed: {}.", said.join("; ")) };
    if new_instructions && agent.prompt.is_empty() {
        note.push_str(" Your instructions were removed: you have none from now on.");
    } else if new_instructions {
        note.push_str(" Your instructions changed; the new ones are in your system prompt from now on.");
    }
    if let Some(section) = checkout {
        let _ = write!(note, "\n\n{section}");
    }
    Some((note, turned_on))
}

/// The tool through which an agent calls tools its frame doesn't list: those
/// of an extension turned on since its context began.
const EXTENSION_CALL: &str = "extension_call";
const EXTENSION_CALL_ABOUT: &str =
    "Call a tool Claktory told you is yours now but isn't in your tool list yet: tool is its name, args its arguments.";

/// What the agent can use now that an extension came on mid-context: the
/// extensions' instructions not in its system prompt (for `turned_on`), and
/// tools `current` has that its frame neither lists nor announced, which it
/// calls through extension_call. Those it's told of join `frame.announced`.
fn extension_news(
    factory: &Factory,
    inner: &Inner,
    agent: &AgentConfig,
    frame: &mut Frame,
    turned_on: &[&str],
    current: &[ToolSpec],
) -> Option<String> {
    if !frame.tools.iter().any(|tool| tool.name == EXTENSION_CALL) {
        return None;
    }
    let mut news = String::new();
    let mut sections = Vec::new();
    for extension in extensions_of(factory, agent) {
        let keys = std::iter::once(extension.manifest().id).chain(extension.instances().iter().map(|instance| instance.key));
        if !keys.into_iter().any(|key| turned_on.contains(&key)) {
            continue;
        }
        for section in extension.prompt(factory, inner, agent) {
            let text = section.text.trim();
            if !frame.system_prompt.contains(text) {
                sections.push(text.to_owned());
            }
        }
    }
    if !sections.is_empty() {
        let _ = write!(
            news,
            "Its instructions, which your system prompt has from your next context (after compaction or /clear):\n\n{}",
            sections.join("\n\n")
        );
    }
    let listed = |name: &str| frame.tools.iter().any(|tool| tool.name == name) || frame.announced.iter().any(|told| told == name);
    let new: Vec<&ToolSpec> = current.iter().filter(|tool| !listed(&tool.name)).collect();
    if !new.is_empty() {
        if !news.is_empty() {
            news.push_str("\n\n");
        }
        news.push_str("Tools of yours that aren't in your tool list until your next context; call them through extension_call {\"tool\": <name>, \"args\": {…}}:");
        for tool in new {
            let parameters = serde_json::to_string(&tool.parameters).unwrap_or_default();
            let _ = write!(news, "\n- {}: {} Parameters: {parameters}", tool.name, tool.description);
            frame.announced.push(tool.name.to_string());
        }
    }
    (!news.is_empty()).then_some(news)
}

impl Factory {
    /// Works until the agent's queue is empty, then marks it idle.
    pub(crate) async fn run(self: Arc<Self>, id: String) {
        // A chat runs as an agent does, on a frame of its own (`crate::chats`).
        if crate::chats::is_chat(&id) {
            return crate::chats::run(self, id).await;
        }
        loop {
            if let Err(error) = self.ensure_loaded(&id).await {
                tracing::error!(agent = %id, %error, "could not load transcript");
                let mut inner = self.lock();
                if let Some(runtime) = inner.agents.get_mut(&id) {
                    runtime.running = false;
                    let dropped = runtime.take_ready();
                    self.settle(&dropped, Outcome::Dropped);
                    if self.end_run(runtime, "failed") {
                        self.mark_unseen(&mut inner, &id);
                    } else {
                        // No run to record it in: its character fails all the same.
                        self.failed_now(&id);
                    }
                }
                self.broadcast_workspace(&inner);
                return;
            }
            self.ensure_checkout(&id).await;
            let current = self.extension_specs(&id);
            let Some(job) = self.next_job(&id, &current) else { return };
            self.execute(&id, job).await;
        }
    }

    /// Makes the agent's own checkout before it works, when its "tree"
    /// extension is on and there's none yet. It takes a while for a big
    /// folder; the agent waits, and works in its plain folder if it fails.
    async fn ensure_checkout(&self, id: &str) {
        let (checkout, epoch, name) = {
            let inner = self.lock();
            let Some(agent) = inner.config.agent(id) else { return };
            // Contained, it works in its pipe's folder or its own, never a checkout.
            if !crate::extensions::enabled(agent, crate::extensions::tree::ID) || self.pipes.contained(id) {
                return;
            }
            let Ok(checkout) = crate::trees::plan(&self.home, id, &source_folder(&self.home, &inner.config, agent)) else { return };
            if checkout.exists() || crate::trees::failure(&checkout).is_some() {
                return;
            }
            // Compacting while idle works on the conversation, not in a folder.
            let Some(runtime) = inner.agents.get(id).filter(|runtime| runtime.compact != Some(Reason::Idle)) else { return };
            (checkout, runtime.epoch, agent.name.clone())
        };
        let text = format!(
            "Making {name}'s own checkout of {}, on the branch {}, so it works apart from other agents. It's made once, in moments. {name} starts as soon as it's ready.",
            checkout.repo.display(),
            checkout.branch,
        );
        self.notice(id, epoch, text, NoticeLevel::Info);
        let (source, folder) = (checkout.source.clone(), checkout.folder.clone());
        let made = tokio::task::spawn_blocking(move || crate::trees::ensure(&checkout)).await;
        let (text, level) = match made {
            Ok(Ok(crate::trees::Made::Plain)) => (
                format!(
                    "{name}'s own checkout is ready, as a plain one (this disk can't clone copy-on-write, so git wrote its files): {}.",
                    folder.display()
                ),
                NoticeLevel::Info,
            ),
            Ok(Ok(_)) => (format!("{name}'s own checkout is ready: {}.", folder.display()), NoticeLevel::Info),
            Ok(Err(error)) => {
                (format!("Couldn't make {name}'s own checkout ({error}), so it works in {}.", source.display()), NoticeLevel::Error)
            }
            Err(error) => (format!("Couldn't make {name}'s own checkout: {error}."), NoticeLevel::Error),
        };
        self.notice(id, epoch, text, level);
    }

    /// Takes everything queued, or marks the agent idle.
    fn next_job(&self, id: &str, current: &[ToolSpec]) -> Option<Job> {
        let mut inner = self.lock();
        let agent = inner.config.agent(id).cloned();
        let compact_at = agent.as_ref().map_or(inner.config.compact_at_tokens, |agent| inner.config.compact_at(agent));
        let models = inner.models.clone();
        let places = agent.as_ref().map(|agent| placed(self, &inner.config, agent));
        let cache_ttl = agent.as_ref().and_then(|agent| inner.config.cache_ttl(&agent.model)).unwrap_or_default();
        let archived = agent.as_ref().is_some_and(|agent| inner.config.archived(agent));
        let runtime = inner.agents.get_mut(id)?;
        let Some(agent) = agent else {
            runtime.running = false;
            runtime.idle = false;
            return None;
        };
        // Its conversation is being taken back (`crate::fork`): what's queued
        // waits for the conversation as it will be, not as it was.
        if runtime.forking {
            runtime.running = false;
            runtime.idle = false;
            return None;
        }
        // Its project is archived: what's queued waits for it to come back.
        if archived {
            runtime.running = false;
            runtime.idle = false;
            runtime.compact = None;
            self.end_run(runtime, "stopped");
            self.broadcast_workspace(&inner);
            return None;
        }
        // New work came before compacting while idle began: it goes first,
        // on the conversation as it is.
        if runtime.compact == Some(Reason::Idle) && runtime.has_ready() {
            runtime.compact = None;
        }
        if !runtime.has_ready() && runtime.compact.is_none() {
            runtime.running = false;
            runtime.idle = false;
            let outcome = if runtime.failed { "failed" } else { "finished" };
            if self.end_run(runtime, outcome) {
                self.mark_unseen(&mut inner, id);
            }
            self.broadcast_workspace(&inner);
            return None;
        }
        // The frame stays as it is, unless the model changed (then the run
        // starts on a new one) or what the system prompt says of the agent
        // did (then `settings_now` rewrites it); what changed since that the
        // agent should know comes first in this run. A compaction writes a
        // new frame anyway.
        let compact = runtime.compact;
        // Compacting while idle is no run of the agent's: nothing is taken
        // for it, and it ends with no news.
        let idle = compact == Some(Reason::Idle);
        runtime.idle = idle;
        let note = if compact.is_some() { None } else { self.take_changes(&mut inner, id, current) };
        let cleared = if idle { None } else { crate::extensions::tree::caches::take_note(self, id) };
        let runtime = inner.agents.get_mut(id)?;
        // A new run: a stop before now was of the one that ended.
        runtime.stopped = false;
        runtime.wait_said = None;
        (runtime.network_since, runtime.network_waits) = (0, 0);
        (runtime.provider_since, runtime.provider_waits) = (0, 0);
        // Quiet messages come with what wakes it, never on their own.
        let mut prompts = crate::extensions::agents::quiet::take(runtime, crate::extensions::agents::quiet::At::Wake);
        prompts.splice(0..0, note.into_iter().chain(cleared));
        let (epoch, compact) = (runtime.epoch, runtime.compact.take());
        let frame = runtime
            .transcript
            .as_ref()
            .and_then(|transcript| transcript.frame())
            .filter(|frame| frame_model(frame) == agent.model)
            .cloned();
        let run = if idle { None } else { self.begin_run(id, runtime) };
        runtime.taken.extend(prompts.iter().map(|queued| (queued.clone(), run)));
        let messages = runtime.transcript.as_ref().map(|transcript| transcript.messages().to_vec()).unwrap_or_default();
        let extensions = extensions_of(self, &agent);
        self.broadcast_workspace(&inner);
        let places = places?;
        Some(Job { prompts, agent, compact_at, messages, models, extensions, frame, places, epoch, compact, cache_ttl })
    }

    /// What changed that the agent should hear of since it was last told
    /// (its own settings, whom it can message, the models, its project's
    /// folders), as a Factory event, with its frame updated to say it was
    /// told; None if nothing did, or a new frame comes next anyway.
    /// `current` are the specs of the extensions' tools as the agent's
    /// settings are now.
    pub(crate) fn take_changes(&self, inner: &mut Inner, id: &str, current: &[ToolSpec]) -> Option<Queued> {
        let agent = inner.config.agent(id)?.clone();
        let mut frame = inner.agents.get(id)?.transcript.as_ref()?.frame()?.clone();
        let quarantined = self.pipes.contained(id);
        let folder = placed(self, &inner.config, &agent).folder;
        let key = settings_key(&inner.config, &agent, &folder, self.pipes.quarantined(id), self.pipes.curator(id));
        // Quarantined, it messages no one, sees no project folder, secret or app.
        let roster = if quarantined { BTreeMap::new() } else { crate::extensions::agents::roster(&inner.config, &agent) };
        let folders = if quarantined { BTreeMap::new() } else { folder_lines(&inner.config, &agent) };
        let names = model_lines(&inner.models);
        let secrets = if quarantined { BTreeMap::new() } else { self.vault.granted(id) };
        let apps = if quarantined { BTreeMap::new() } else { crate::apps::granted(self, &inner.config, id) };
        let pipes = crate::pipes::agents::granted(self, &inner.config, id);
        // tree_land in its tools, or reached through extension_call.
        let can_land = frame.tools.iter().any(|tool| tool.name == "tree_land" || tool.name == EXTENSION_CALL);
        let (settings, turned_on) = match settings_note(frame.told(), &self.home, inner, &agent, &folder, can_land, &frame.tools) {
            Some((note, turned_on)) => (Some(note), turned_on),
            None => (None, Vec::new()),
        };
        // A new model gets a new frame, with every tool of its own.
        let news =
            if frame_model(&frame) == agent.model { extension_news(self, inner, &agent, &mut frame, &turned_on, current) } else { None };
        let settings = match (settings, news) {
            (Some(settings), Some(news)) => Some(format!("{settings}\n\n{news}")),
            (settings, news) => settings.or(news),
        };
        let apps_note = app_changes(&frame, &apps, &crate::apps::sections_by_app(self, &inner.config, id));
        let pipes_note = crate::pipes::agents::changes(&inner.config, &frame.pipes, &pipes);
        // Quiet copies that came, once each: they never woke it (#285).
        let copies = crate::extensions::agents::provenance::COPIES_TOOL;
        let direct = frame.tools.iter().any(|tool| tool.name == copies);
        let copies_note = if quarantined { None } else { crate::extensions::agents::provenance::note(&self.store, id, direct) };
        let notes: Vec<String> = [settings, changes(&frame, &roster, &names, &folders, &secrets), apps_note, pipes_note, copies_note]
            .into_iter()
            .flatten()
            .collect();
        if notes.is_empty() {
            return None;
        }
        frame.told = key;
        frame.roster = roster;
        frame.models = names;
        frame.folders = folders;
        frame.secrets = secrets;
        frame.apps = apps;
        frame.pipes = pipes;
        // Left out of compaction: the next frame says it all.
        let event = Event {
            source: EventSource::Host,
            from: "Claktory".to_owned(),
            id: String::new(),
            summary: String::new(),
            transient: true,
            pipe_seqs: Vec::new(),
            provenance: None,
        };
        let mut note = Queued::event(notes.join("\n"), event);
        match self.store.add_message(id, &note) {
            Ok(message) => note.id = message,
            Err(error) => {
                tracing::error!(agent = %id, %error, "could not record what changed for the agent");
                return None;
            }
        }
        inner.agents.get_mut(id)?.transcript.as_mut()?.set_frame(frame);
        Some(note)
    }

    /// Notes that `agent` was told it has the secret `secret` (its id) in an
    /// event of its own, such as the answer to its request, so the next
    /// note of what changed doesn't say it again.
    pub(crate) fn told_secret(&self, agent: &str, secret: &str) {
        // Quarantined, its notes list no secret.
        if self.pipes.contained(agent) {
            return;
        }
        let Some(label) = self.vault.granted(agent).remove(secret) else { return };
        let mut inner = self.lock();
        let Some(transcript) = inner.agents.get_mut(agent).and_then(|runtime| runtime.transcript.as_mut()) else { return };
        let Some(mut frame) = transcript.frame().cloned() else { return };
        frame.secrets.insert(secret.to_owned(), label);
        transcript.set_frame(frame);
    }

    /// Writes the segment's frame from Factory as it is now, keeps it with the
    /// segment, and returns it; None if the agent or its conversation is gone.
    /// `announced` are the tools the agent was told it can call through
    /// extension_call, for a frame that keeps the tools it had.
    async fn new_frame(&self, id: &str, epoch: u64, tools: Vec<ToolSpec>, announced: Vec<String>) -> Option<Frame> {
        let (agent, project, compact_at, models, sections, pinned, roster, folders, places, settings, folder_name, user, attribution) = {
            let mut inner = self.lock();
            if inner.agents.get(id).is_none_or(|runtime| runtime.epoch != epoch) {
                return None;
            }
            let agent = inner.config.agent(id)?.clone();
            let project = agent.project.as_deref().and_then(|project| inner.config.project(project).cloned());
            let quarantined = self.pipes.contained(id);
            let (sections, pinned) = self.sections(&mut inner, id, &agent, &extensions_of(self, &agent));
            // Quarantined, it messages no one and sees no project folder.
            let roster = if quarantined { BTreeMap::new() } else { crate::extensions::agents::roster(&inner.config, &agent) };
            let folders = if quarantined { BTreeMap::new() } else { folder_lines(&inner.config, &agent) };
            let places = placed(self, &inner.config, &agent);
            let settings = settings_key(&inner.config, &agent, &places.folder, self.pipes.quarantined(id), self.pipes.curator(id));
            let folder_name = if quarantined { None } else { inner.config.agent_folder(&agent).map(|folder| folder.name.clone()) };
            let compact_at = inner.config.compact_at(&agent);
            let user = inner.config.user_name().map(str::to_owned);
            let attribution = inner.config.attribution;
            (agent, project, compact_at, inner.models.clone(), sections, pinned, roster, folders, places, settings, folder_name, user, attribution)
        };
        // Contained, it has no secret or app.
        let quarantined = self.pipes.contained(id);
        let secrets = if quarantined { BTreeMap::new() } else { self.vault.granted(id) };
        let apps = if quarantined { BTreeMap::new() } else { crate::apps::granted(self, &self.lock().config, id) };
        let pipes = crate::pipes::agents::granted(self, &self.lock().config, id);
        let render = Render {
            agent: &agent,
            project: project.as_ref(),
            compact_at,
            models: &models,
            sections: &sections,
            pinned: &pinned,
            folder_name: folder_name.as_deref(),
            folders: &folders,
            user: user.as_deref(),
            secrets: &secrets,
            attribution,
        };
        let system_prompt = system_prompt(&render, &places.folder, &self.home, &places.extra_dirs).await;
        let frame = Frame {
            settings,
            system_prompt,
            tools,
            told: String::new(),
            roster,
            models: model_lines(&models),
            folders,
            secrets,
            apps,
            pipes,
            announced,
        };
        let mut inner = self.lock();
        let transcript =
            inner.agents.get_mut(id).filter(|runtime| runtime.epoch == epoch).and_then(|runtime| runtime.transcript.as_mut())?;
        transcript.set_frame(frame.clone());
        Some(frame)
    }

    /// The prompt sections of the agent's extensions: static ones as written
    /// now, pinned ones as kept with the current segment, written now and
    /// kept if the segment has none yet.
    fn sections(
        &self,
        inner: &mut Inner,
        id: &str,
        agent: &AgentConfig,
        extensions: &[&'static dyn Extension],
    ) -> (Vec<String>, Vec<String>) {
        let mut sections = Vec::new();
        let mut written = Vec::new();
        for extension in extensions {
            for section in extension.prompt(self, inner, agent) {
                match section.kind {
                    PromptKind::Static => sections.push(section.text),
                    PromptKind::SegmentPinned => written.push((extension.manifest().id, section.text)),
                    PromptKind::InstancePinned(key) => written.push((key, section.text)),
                }
            }
        }
        if !self.pipes.contained(id) {
            sections.extend(crate::apps::sections(self, &inner.config, id));
        }
        sections.extend(crate::pipes::agents::prompt_section(self, &inner.config, id));
        sections.extend(crate::pipes::curator::prompt_section(self, &inner.config, id));
        let Some(transcript) = inner.agents.get_mut(id).and_then(|runtime| runtime.transcript.as_mut()) else {
            return (sections, written.into_iter().map(|(_, text)| text).collect());
        };
        let kept = transcript.pinned_mut();
        // Turned off: turned on again later, it starts from what's current.
        kept.retain(|extension, _| written.iter().any(|(id, _)| id == extension));
        let pinned = written.into_iter().map(|(extension, text)| kept.entry(extension.to_owned()).or_insert(text).clone()).collect();
        (sections, pinned)
    }

    async fn execute(self: &Arc<Self>, id: &str, job: Job) {
        // Only models whose provider is signed in are listed.
        let listed = job.models.iter().any(|model| model.id == job.agent.model);
        let Some(model) = listed.then(|| self.models.resolve(&job.agent.model)).flatten() else {
            let problem = no_model(self, &job.models, &job.agent.model, " in this agent's settings");
            self.unsent(id, &job.prompts, problem, NoticeLevel::Error);
            return;
        };
        let thinking_level = ThinkingLevel::parse(&job.agent.effort).unwrap_or(ThinkingLevel::High);
        let folder = &job.places.folder;
        if let Err(error) = tokio::fs::create_dir_all(folder).await {
            let problem = format!("Can't use the folder {}: {error}", folder.display());
            self.unsent(id, &job.prompts, problem, NoticeLevel::Error);
            self.latch_failure(id, job.epoch, true);
            return;
        }
        let tools = self.build_tools(id, &job.places, &job.extensions);
        // The segment's system prompt and tools, byte for byte, or new ones.
        let reused = match job.frame {
            Some(frame) => Ok((frame.system_prompt, fit(self, tools, &frame.tools, &job.agent))),
            None => Err(tools),
        };
        let (system_prompt, tools) = match reused {
            Ok(reused) => reused,
            Err(tools) => {
                let specs = tools.iter().map(|tool| tool.spec.clone()).collect();
                let Some(frame) = self.new_frame(id, job.epoch, specs, Vec::new()).await else {
                    self.unsent(id, &job.prompts, "Stopped.".to_owned(), NoticeLevel::Info);
                    return;
                };
                (frame.system_prompt, tools)
            }
        };
        // What the system prompt says of the agent may have changed since.
        let now = self.settings_now(id, job.epoch).await;
        let (system_prompt, tools) = (now.system_prompt.unwrap_or(system_prompt), now.tools.unwrap_or(tools));
        // A compaction starts a segment, with a frame of its own: the agent's
        // settings as they are now, tools included.
        let after_compaction: factory_agent::AfterCompaction = {
            let factory = Arc::downgrade(self);
            let (id, epoch) = (id.to_owned(), job.epoch);
            Arc::new(move || {
                let (factory, id) = (factory.upgrade(), id.clone());
                Box::pin(async move {
                    let factory = factory?;
                    let tools = factory.current_tools(&id)?;
                    let specs = tools.iter().map(|tool| tool.spec.clone()).collect();
                    let frame = factory.new_frame(&id, epoch, specs, Vec::new()).await?;
                    Some(factory_agent::NewContext { system_prompt: frame.system_prompt, tools })
                })
            })
        };
        // A request that carried on with another account says so in the
        // transcript, and the providers show the account it left at its limit.
        let notes: factory_agent::providers::models::Notes = {
            let factory = Arc::downgrade(self);
            let (id, epoch) = (id.to_owned(), job.epoch);
            Arc::new(move |note| {
                let Some(factory) = factory.upgrade() else { return };
                if let factory_agent::providers::models::AccountNote::Switched(text) = note {
                    factory.notice(&id, epoch, text, NoticeLevel::Info);
                }
                tokio::spawn(async move { factory.refresh_models().await });
            })
        };
        // The model a refused step is retried on: the agent's own, or the
        // Claktory-wide default; it must be another model whose provider is
        // signed in, or there's none.
        let fallback_model = {
            let named = {
                let config = &self.lock().config;
                let own = config.agent(id).map(|agent| agent.fallback_model.clone()).unwrap_or_default();
                if own.is_empty() { config.default_fallback_model.clone() } else { own }
            };
            (!named.is_empty() && named != job.agent.model && job.models.iter().any(|model| model.id == named))
                .then(|| self.models.resolve(&named))
                .flatten()
        };
        let mut agent = Agent::new(AgentOptions {
            system_prompt,
            model,
            thinking_level,
            tools,
            messages: job.messages,
            stream: self.models.stream_noting(notes),
            session_id: Some(id.to_owned()),
            transform_context: None,
            should_stop_after_turn: None,
            // Once it has read a web page, its secrets wait for the person.
            before_tool_call: Some(crate::pipes::agents::gate(self, id)),
            compact_at: Some(job.compact_at),
            inbox: Some(self.inbox(id, job.epoch)),
            after_compaction: Some(after_compaction),
            // Quarantined for a pipe, it has none.
            secrets: (!self.pipes.contained(id)).then(|| self.secrets(id)),
            fallback_model,
        });
        agent.state.cache_ttl = job.cache_ttl;
        agent.wait_out_with(crate::waits::wait_out(Arc::downgrade(self), id.to_owned(), job.epoch));
        agent.wait_network_with(crate::waits::wait_network(Arc::downgrade(self), id.to_owned(), job.epoch));
        agent.wait_provider_with(crate::waits::wait_provider(Arc::downgrade(self), id.to_owned(), job.epoch));
        let factory = self.clone();
        let agent_id = id.to_owned();
        let epoch = job.epoch;
        agent.subscribe(Arc::new(move |event| {
            let factory = factory.clone();
            let agent_id = agent_id.clone();
            Box::pin(async move {
                factory.apply(&agent_id, epoch, event).await;
                Ok(())
            })
        }));
        if !self.set_abort(id, Some((agent.abort_handle(), agent.detach_handle()))) {
            self.unsent(id, &job.prompts, "Stopped.".to_owned(), NoticeLevel::Info);
            return;
        }
        let images = agent.state.model.input.iter().any(|input| input == "image");
        let mut prompts = Vec::with_capacity(job.prompts.len());
        for queued in job.prompts {
            prompts.push(user_message(queued, images));
        }
        if let Some(reason) = job.compact {
            let idle = reason == Reason::Idle;
            // Work that came meanwhile wins over compacting while idle.
            let skip = idle && self.lock().agents.get(id).is_none_or(|runtime| runtime.has_ready() || runtime.stopped);
            match if skip { Err(factory_agent::Error::Aborted) } else { agent.compact_for(reason).await } {
                Ok(()) => {}
                Err(factory_agent::Error::Aborted) => prompts.clear(),
                // Nobody asked for it: the conversation stays as it was, with no news.
                Err(error) if idle => tracing::warn!(agent = %id, %error, "compacting while idle failed"),
                Err(error) => {
                    tracing::warn!(agent = %id, %error, "compaction failed");
                    self.notice(id, job.epoch, format!("Couldn't compact: {error}"), NoticeLevel::Error);
                    self.latch_failure(id, job.epoch, true);
                }
            }
        }
        if !prompts.is_empty() {
            match agent.prompt_messages(prompts).await {
                // The model failed for good (after its retries) when the run
                // ends on its error; a later prompt that gets through is a
                // recovery.
                Ok(()) => {
                    let failed =
                        matches!(agent.state.messages.last(), Some(Message::Assistant(reply)) if reply.stop_reason == StopReason::Error);
                    self.latch_failure(id, job.epoch, failed);
                }
                // The person stopped it: that's no failure.
                Err(factory_agent::Error::Aborted) => {}
                Err(error) => {
                    tracing::warn!(agent = %id, %error, "run failed");
                    self.notice(id, job.epoch, error.to_string(), NoticeLevel::Error);
                    self.latch_failure(id, job.epoch, true);
                }
            }
        }
        self.set_abort(id, None);
    }

    /// Whether the run, so far, ended in an error it didn't recover from
    /// (see `Runtime::failed`).
    pub(crate) fn latch_failure(&self, id: &str, epoch: u64, failed: bool) {
        let mut inner = self.lock();
        if let Some(runtime) = inner.agents.get_mut(id).filter(|runtime| runtime.epoch == epoch) {
            runtime.failed = failed;
        }
    }

    /// The agent's coding tools, where it works now, its extensions' tools,
    /// and extension_call.
    fn build_tools(self: &Arc<Self>, id: &str, places: &Places, extensions: &[&'static dyn Extension]) -> Vec<Tool> {
        // Quarantined for a pipe: its kept tools only (`extensions` are
        // already those it keeps), and extension_call reaching no more.
        if self.pipes.contained(id) {
            let mut tools = if self.pipes.curator(id) {
                crate::pipes::curator::tools(self, places)
            } else {
                crate::pipes::quarantine::tools(self, id, places)
            };
            for extension in extensions {
                tools.extend(extension.tools(self, id));
            }
            tools.push(self.extension_call(id));
            return tools;
        }
        let mut tools = factory_agent::coding_tools(&places.folder, &places.extra_dirs, Some(self.background(id)));
        factory_agent::tools::read_only(&mut tools, &places.read_only);
        // ripgrep and jq are named only when the agent's commands will find them.
        factory_agent::tools::installed(&mut tools, &crate::update::agents_path(&self.home));
        // Ink and its staging are the person's until they send it, and Apple's
        // container tool's folder holds Claktory's browser's sessions on a Mac.
        let browser_tools = extensions.iter().any(|extension| extension.manifest().id == crate::extensions::browser::ID);
        let denied: Vec<_> = crate::ink::private(&self.home)
            .into_iter()
            .map(Into::into)
            .chain(crate::browser_host::confine::denied(browser_tools))
            .collect();
        factory_agent::tools::denied(&mut tools, &denied);
        // Only agents given a secret see bash's `secrets`.
        if !self.vault.granted(id).is_empty() {
            factory_agent::tools::secrets(&mut tools);
        }
        for extension in extensions {
            tools.extend(extension.tools(self, id));
        }
        tools.extend(crate::apps::tools(self, id));
        tools.extend(crate::pipes::agents::tools(self, id));
        tools.push(self.extension_call(id));
        tools
    }

    /// The agent's extensions' tools as its settings are now.
    fn extension_tools(self: &Arc<Self>, id: &str) -> Vec<Tool> {
        let Some(agent) = self.lock().config.agent(id).cloned() else { return Vec::new() };
        let mut tools: Vec<Tool> = extensions_of(self, &agent).into_iter().flat_map(|extension| extension.tools(self, id)).collect();
        if self.pipes.curator(id) {
            return tools;
        }
        if self.pipes.quarantined(id) {
            tools.extend(crate::pipes::quarantine::pipe_tools(self, id));
            return tools;
        }
        tools.extend(crate::apps::tools(self, id));
        tools.extend(crate::pipes::agents::tools(self, id));
        tools
    }

    fn extension_specs(self: &Arc<Self>, id: &str) -> Vec<ToolSpec> {
        self.extension_tools(id).into_iter().map(|tool| tool.spec).collect()
    }

    /// Calls a tool of the agent's extensions by name, as its settings are
    /// now: how it uses an extension turned on since its context began.
    fn extension_call(self: &Arc<Self>, id: &str) -> Tool {
        let parameters = serde_json::json!({
            "type": "object",
            "required": ["tool"],
            "properties": {"tool": {"type": "string"}, "args": {"type": "object"}},
            "additionalProperties": false,
        });
        let handler: factory_agent::Handler = crate::extensions::agent_handler(self, id, move |factory, agent, args| async move {
            let Some(factory) = factory else { return Err(factory_agent::Error::Other("Claktory is stopping.".into())) };
            let name = args.get("tool").and_then(serde_json::Value::as_str).unwrap_or_default().to_owned();
            let tools = factory.extension_tools(&agent);
            let Some(tool) = tools.into_iter().find(|tool| tool.spec.name == name) else {
                return Err(factory_agent::Error::Other(format!(
                    "No tool \"{name}\" of an extension is on for you; extension_call reaches only those Claktory told you of."
                )));
            };
            let args = args.get("args").cloned().unwrap_or_else(|| serde_json::json!({}));
            tool.execute(&args, &tokio_util::sync::CancellationToken::new(), &|_| {}).await
        });
        let mut tool = Tool::custom(EXTENSION_CALL, "Extension tool", EXTENSION_CALL_ABOUT, parameters, handler);
        tool.spec.constrained_sampling = serde_json::Value::Null;
        tool
    }

    /// The system prompt, model and effort for the agent's next model call,
    /// as its settings are now: a new model gets a new frame, tools and all,
    /// and so does a new project, folder or checkout (its tools work
    /// somewhere else);
    /// new instructions or a new name a new system prompt, with the frame's
    /// tools kept (they come first in the cache); effort goes into the next
    /// request as it is. What stays the same is None.
    async fn settings_now(self: &Arc<Self>, id: &str, epoch: u64) -> factory_agent::Settings {
        if crate::chats::is_chat(id) {
            return crate::chats::settings_now(self, id, epoch);
        }
        let mut settings = factory_agent::Settings::default();
        let (agent, frame, listed) = {
            let inner = self.lock();
            let Some(runtime) = inner.agents.get(id).filter(|runtime| runtime.epoch == epoch) else { return settings };
            let Some(agent) = inner.config.agent(id).cloned() else { return settings };
            let frame = runtime.transcript.as_ref().and_then(|transcript| transcript.frame()).cloned();
            let listed = inner.models.iter().any(|model| model.id == agent.model);
            (agent, frame, listed)
        };
        settings.thinking_level = ThinkingLevel::parse(&agent.effort);
        let Some(frame) = frame else { return settings };
        if frame_model(&frame) != agent.model {
            // A model that can't run leaves it on the one it has until the run ends.
            let Some(model) = listed.then(|| self.models.resolve(&agent.model)).flatten() else { return settings };
            let tools = self.current_tools(id).unwrap_or_default();
            let specs = tools.iter().map(|tool| tool.spec.clone()).collect();
            let Some(frame) = self.new_frame(id, epoch, specs, Vec::new()).await else { return settings };
            settings.system_prompt = Some(frame.system_prompt);
            settings.model = Some(model);
            settings.tools = Some(tools);
            return settings;
        }
        let then: serde_json::Value = serde_json::from_str(&frame.settings).unwrap_or_default();
        let quarantined = self.pipes.contained(id);
        let now = {
            let inner = self.lock();
            settings_key(&inner.config, &agent, &placed(self, &inner.config, &agent).folder, self.pipes.quarantined(id), self.pipes.curator(id))
        };
        let now: serde_json::Value = serde_json::from_str(&now).unwrap_or_default();
        let changed = |keys: &[&str]| keys.iter().any(|key| then.get(key) != now.get(key));
        // Given its first secret, its bash takes `secrets` from now on.
        let needs_secrets = !quarantined && !frame.tools.iter().any(takes_secrets) && !self.vault.granted(id).is_empty();
        // Its project, folder or checkout, or a quarantine that came or went.
        if changed(&["project", "folder", "cwd", "quarantined", "curator"]) || needs_secrets {
            // It works somewhere else now: its tools too.
            let Some(tools) = self.current_tools(id) else { return settings };
            let specs = tools.iter().map(|tool| tool.spec.clone()).collect();
            let Some(frame) = self.new_frame(id, epoch, specs, Vec::new()).await else { return settings };
            settings.system_prompt = Some(frame.system_prompt);
            settings.tools = Some(tools);
        } else if changed(&["name", "role", "prompt"])
            && let Some(frame) = self.new_frame(id, epoch, frame.tools.clone(), frame.announced.clone()).await
        {
            settings.system_prompt = Some(frame.system_prompt);
        }
        settings
    }

    /// The agent's tools as its settings are now; None if it's gone.
    fn current_tools(self: &Arc<Self>, id: &str) -> Option<Vec<Tool>> {
        let (agent, places) = {
            let inner = self.lock();
            let agent = inner.config.agent(id)?.clone();
            let places = placed(self, &inner.config, &agent);
            (agent, places)
        };
        Some(self.build_tools(id, &places, &extensions_of(self, &agent)))
    }

    /// Records an agent event: updates the transcript, tells clients, saves.
    pub(crate) async fn apply(&self, id: &str, epoch: u64, event: AgentEvent) {
        // A file it wrote: apps showing it, and comments on it, follow at once.
        if let AgentEvent::ToolExecutionEnd { tool_name, result, is_error: false, .. } = &event
            && matches!(tool_name.as_str(), "write_file" | "edit_file")
            && let Some(path) = result.details.get("path").and_then(serde_json::Value::as_str)
        {
            let folder = {
                let inner = self.lock();
                inner.config.agent(id).map(|agent| placed(self, &inner.config, agent).folder)
            };
            if let Some(folder) = folder {
                crate::annotations::written(self, id, &folder, path);
            }
        }
        // A download a file tool used: whoever reads it has read the pages
        // it came from (#435 R9).
        if let AgentEvent::ToolExecutionEnd { tool_name, result, is_error: false, .. } = &event
            && matches!(tool_name.as_str(), "read_file" | "read_image" | "write_file" | "edit_file" | "show")
            && let Some(path) = result.details.get("path").and_then(serde_json::Value::as_str)
        {
            let folder = {
                let inner = self.lock();
                inner.config.agent(id).map(|agent| placed(self, &inner.config, agent).folder)
            };
            if let Some(folder) = folder {
                crate::errands::downloads::touched(self, id, &folder, path);
            }
        }
        // macOS refused a tool a protected place (item 12): kept, once
        // checked again, for the person to see.
        if let AgentEvent::ToolExecutionEnd { result, .. } = &event
            && let Some(root) = result.details.get(factory_agent::tools::protected::DETAILS_KEY).and_then(serde_json::Value::as_str)
        {
            crate::protected::refused_by_tool(self, id, root);
        }
        // The images a compaction let go of: their files at the provider go.
        let mut forgotten = Vec::new();
        let persist = {
            let mut inner = self.lock();
            // An answer: the connection is back (`crate::waits`, #364 O2),
            // and the provider with it (#490).
            if let AgentEvent::MessageEnd { message: factory_agent::types::Message::Assistant(answer) } = &event
                && answer.stop_reason != factory_agent::types::StopReason::Error
                && let Some(runtime) = inner.agents.get_mut(id).filter(|runtime| runtime.epoch == epoch)
            {
                (runtime.network_since, runtime.network_waits) = (0, 0);
                (runtime.provider_since, runtime.provider_waits) = (0, 0);
            }
            // A model call's tokens, for the account's check-ins.
            if matches!(event, AgentEvent::MessageEnd { .. } | AgentEvent::Compacted { .. })
                && let Some(runtime) = inner.agents.get(id).filter(|runtime| runtime.epoch == epoch)
            {
                let (model, name) = inner
                    .config
                    .agent(id)
                    .map(|agent| (agent.model.clone(), agent.name.clone()))
                    .or_else(|| self.chats.model_and_name(id))
                    .unwrap_or_default();
                crate::account::analytics::record(&self.store.calls, runtime.run, &model, &event);
                crate::usage_report::record(&self.store.calls, id, &name, &model, &event);
            }
            let Some(runtime) = inner.agents.get_mut(id) else { return };
            // A run of a conversation the person cleared.
            if runtime.epoch != epoch {
                return;
            }
            let writes = runtime.writes.clone();
            if let AgentEvent::Waiting { message: Message::Assistant(reply), .. } = &event {
                runtime.asked_on.clone_from(&reply.provider);
            }
            let doing = match &event {
                AgentEvent::ToolExecutionStart { tool_name, .. } => Some(Some(tool_name.clone())),
                AgentEvent::ToolExecutionEnd { .. } | AgentEvent::AgentEnd { .. } => Some(None),
                _ => None,
            };
            // Its character works while a tool runs, and thinks otherwise.
            let doing_changed = doing.as_ref().is_some_and(|doing| *doing != runtime.doing);
            if let Some(doing) = doing {
                runtime.doing_since = if doing.is_some() { crate::transcript::now_ms() } else { 0 };
                runtime.doing = doing;
                // Messages that came just before it started.
                runtime.make_way();
            }
            let Some(transcript) = runtime.transcript.as_mut() else { return };
            if let AgentEvent::Compacted { messages, .. } = &event {
                let kept = crate::models::image_digests(messages);
                forgotten = crate::models::image_digests(transcript.messages()).into_iter().filter(|digest| !kept.contains(digest)).collect();
            }
            let applied = match transcript.apply(event) {
                Ok(applied) => applied,
                Err(error) => {
                    tracing::error!(agent = %id, %error, "could not serialize transcript");
                    return;
                }
            };
            // A request for a secret shows what in the vault fits it (#360),
            // which its tool result doesn't hold: the agent never sees it.
            let changes: Vec<crate::transcript::Change> = applied
                .changes
                .into_iter()
                .map(|change| match change {
                    crate::transcript::Change::Entry(entry) => match &entry.kind {
                        Some(factory_protocol::v1::entry::Kind::SecretRequest(request))
                            if request.state() == factory_protocol::v1::SecretRequestState::Pending =>
                        {
                            let fitted = crate::vault::requests::fitted(self, request.clone());
                            crate::transcript::Change::Entry(transcript.set_secret_request(fitted).unwrap_or(entry))
                        }
                        _ => crate::transcript::Change::Entry(entry),
                    },
                    other => other,
                })
                .collect();
            for change in changes {
                self.send_change(&inner, id, change);
            }
            if applied.context_changed || doing_changed {
                self.broadcast_workspace(&inner);
            }
            (applied.persist, applied.user_message, writes)
        };
        if !forgotten.is_empty() {
            let models = self.models.clone();
            tokio::spawn(async move { models.forget_images(forgotten).await });
        }
        let (Some(persist), user_message, writes) = persist else { return };
        let _writing = writes.lock().await;
        if self.lock().agents.get(id).is_none_or(|runtime| runtime.epoch != epoch) {
            return;
        }
        let path = persist.path().to_owned();
        if let Err(error) = persist.write().await {
            tracing::error!(agent = %id, path = %path.display(), %error, "could not save transcript");
            return;
        }
        if user_message && let Some(runtime) = self.lock().agents.get_mut(id) {
            self.delivered(runtime, Some(1));
        }
    }

    /// Lets the person stop the run; false when they already did, while it
    /// was still starting.
    /// Also keeps what moves its bash command to the background when a
    /// message comes (see `make_way`).
    pub(crate) fn set_abort(&self, id: &str, handles: Option<(factory_agent::AbortHandle, factory_agent::DetachHandle)>) -> bool {
        let mut inner = self.lock();
        let Some(runtime) = inner.agents.get_mut(id) else { return false };
        if runtime.stopped && handles.is_some() {
            return false;
        }
        (runtime.abort, runtime.detach) = handles.unzip();
        true
    }

    pub(crate) fn notice(&self, id: &str, epoch: u64, text: String, level: NoticeLevel) {
        let mut inner = self.lock();
        let Some(runtime) = inner.agents.get_mut(id).filter(|runtime| runtime.epoch == epoch) else { return };
        let Some(transcript) = runtime.transcript.as_mut() else { return };
        let entry = transcript.notice(text, level);
        self.send_change(&inner, id, Change::Entry(entry));
    }

    /// Shows prompts that never reached the model, and why.
    pub(crate) fn unsent(&self, id: &str, prompts: &[Queued], problem: String, level: NoticeLevel) {
        self.settle(prompts, Outcome::Dropped);
        let mut inner = self.lock();
        if let Some(runtime) = inner.agents.get_mut(id) {
            runtime.taken.retain(|(taken, _)| prompts.iter().all(|prompt| prompt.id != taken.id));
        }
        let Some(transcript) = inner.agents.get_mut(id).and_then(|runtime| runtime.transcript.as_mut()) else { return };
        let mut entries = Vec::with_capacity(prompts.len() + 1);
        for queued in prompts {
            entries.push(transcript.unsent(&user_message(queued.clone(), false)));
        }
        entries.push(transcript.notice(problem, level));
        for entry in entries {
            self.send_change(&inner, id, Change::Entry(entry));
        }
    }

    /// What arrived for a working agent since its last step: the person's
    /// messages and events, which it reads before its next model call. None
    /// once the person stopped it.
    pub(crate) fn inbox(self: &Arc<Self>, id: &str, epoch: u64) -> Inbox {
        let factory = Arc::downgrade(self);
        let agent = id.to_owned();
        Arc::new(move || {
            let (factory, agent) = (factory.upgrade(), agent.clone());
            Box::pin(async move {
                let Some(factory) = factory else { return Arrived::default() };
                let current = factory.extension_specs(&agent);
                let images = factory.takes_images(&agent);
                let messages = {
                    let mut inner = factory.lock();
                    if inner.agents.get(&agent).is_none_or(|runtime| runtime.abort.is_none()) {
                        return Arrived::default();
                    }
                    let note = factory.take_changes(&mut inner, &agent, &current);
                    let Some(runtime) = inner.agents.get_mut(&agent) else { return Arrived::default() };
                    // Quiet messages too, after a step of its work (never
                    // into compacting while idle).
                    let at = if runtime.idle { crate::extensions::agents::quiet::At::Never } else { crate::extensions::agents::quiet::At::Step };
                    let mut arrived = crate::extensions::agents::quiet::take(runtime, at);
                    arrived.splice(0..0, note);
                    if !arrived.is_empty() {
                        let run = factory.begin_run(&agent, runtime);
                        runtime.taken.extend(arrived.iter().map(|queued| (queued.clone(), run)));
                        factory.broadcast_workspace(&inner);
                    }
                    arrived.into_iter().map(|queued| user_message(queued, images)).collect()
                };
                // Its own settings apply from its next model call.
                let settings = factory.settings_now(&agent, epoch).await;
                Arrived { messages, settings }
            })
        })
    }

    /// Lets the agent's bash run commands in the background; it hears when
    /// one finishes as an event. Logs go in the agent's folder.
    fn background(self: &Arc<Self>, id: &str) -> Background {
        let started = {
            let factory = Arc::downgrade(self);
            let agent = id.to_owned();
            Arc::new(move |task: &Task| {
                if let Some(factory) = factory.upgrade() {
                    factory.task_started(&agent, task);
                }
            })
        };
        let factory = Arc::downgrade(self);
        let agent = id.to_owned();
        let mut environment = vec![("FACTORY_AGENT_ID".to_owned(), id.to_owned())];
        // ripgrep and the other tools Claktory gives every agent, first.
        environment.push(("PATH".to_owned(), crate::update::agents_path(&self.home)));
        Background {
            dir: self.home.agent_dir(id).join("tasks"),
            environment,
            started,
            finished: Arc::new(move |done: Finished| {
                if let Some(factory) = factory.upgrade() {
                    factory.task_finished(&agent, done);
                }
            }),
        }
    }

    /// What the agent's bash needs for the vault's secrets: the values of the
    /// ones it's granted, a log of each use, and the redactor for output.
    fn secrets(self: &Arc<Self>, id: &str) -> factory_agent::Secrets {
        let factory = Arc::downgrade(self);
        let resolve: factory_agent::secrets::Resolve = Arc::new(move |agent, names| {
            let factory = factory.clone();
            Box::pin(async move {
                let Some(factory) = factory.upgrade() else {
                    return Err("Claktory is stopping.".to_owned());
                };
                // Locked after a restart: the account service may open it now.
                crate::account::unlock_now(&factory).await;
                // Secrets made from a connection are renewed first when due.
                factory.renew_for(&agent, &names).await?;
                factory.vault.resolve(&agent, &names)
            })
        });
        let factory = Arc::downgrade(self);
        let used: factory_agent::secrets::Used = Arc::new(move |used| {
            if let Some(factory) = factory.upgrade() {
                if let Err(error) = factory.vault.record_use(&used) {
                    tracing::error!(%error, "could not log a secret's use");
                }
                factory.broadcast_workspace(&factory.lock());
            }
        });
        let filter = match FILTER_EXECUTABLE.get() {
            Some(factoryd) => vec![factoryd.clone().into_os_string(), "redact".into()],
            None => Vec::new(),
        };
        let factory = Arc::downgrade(self);
        let check: factory_agent::secrets::CheckSave = Arc::new(move |agent, save, parents| {
            let factory = factory.clone();
            Box::pin(async move {
                let Some(factory) = factory.upgrade() else {
                    return Err("Claktory is stopping.".to_owned());
                };
                crate::account::unlock_now(&factory).await;
                crate::vault::saved::check(&factory, &agent, &save, &parents).map(|_| ())
            })
        });
        let factory = Arc::downgrade(self);
        let store: factory_agent::secrets::StoreSave = Arc::new(move |agent, save, parents, value| {
            let factory = factory.clone();
            Box::pin(async move {
                let Some(factory) = factory.upgrade() else {
                    return Err("Claktory is stopping, so nothing was saved.".to_owned());
                };
                crate::vault::saved::store(&factory, &agent, &save, &parents, &value)
            })
        });
        let save = Some(factory_agent::secrets::Save { check, store });
        factory_agent::Secrets { agent: id.to_owned(), resolve, used, redaction: self.redaction.clone(), filter, save }
    }
}

/// The factoryd that runs a command's log filter (`factoryd redact`), which
/// keeps secrets out of the log on disk; main sets it to itself. Unset, as
/// in a program that runs Factory inside it, such logs aren't filtered.
static FILTER_EXECUTABLE: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();

/// Names the factoryd that filters the logs of commands with secrets.
pub fn set_filter_executable(factoryd: PathBuf) {
    let _ = FILTER_EXECUTABLE.set(factoryd);
}

async fn system_prompt(render: &Render<'_>, folder: &Path, home: &Home, extra_dirs: &[PathBuf]) -> String {
    let agent = render.agent;
    let mut prompt = factory_agent::coding_prompt(folder, extra_dirs);
    // Writing to a String can't fail.
    let _ = write!(prompt, "\n\n# Claktory\n\nYou are {}, a persistent agent in Claktory. ", agent.name);
    if !agent.role.is_empty() {
        let _ = write!(prompt, "Your role: {}. ", agent.role);
    }
    let _ = match (render.project, render.folder_name) {
        (Some(project), Some(name)) => {
            writeln!(prompt, "You work in the project \"{}\", in its folder {name}, {}.", project.name, folder.display())
        }
        (Some(project), None) if !render.folders.is_empty() => {
            writeln!(prompt, "You work in the project \"{}\", in its own folder, {}.", project.name, folder.display())
        }
        (Some(project), None) => writeln!(prompt, "You work in the project \"{}\", in its folder {}.", project.name, folder.display()),
        (None, _) => writeln!(
            prompt,
            "You work in Claktory's workspace, {}, outside any project. Each project is a folder inside it.",
            folder.display()
        ),
    };
    if let Some(name) = render.user {
        let _ = writeln!(prompt, "The person you work with is {name}.");
    }
    if !render.folders.is_empty() {
        prompt.push_str("The project's folders:\n");
        for line in render.folders.values() {
            let _ = writeln!(prompt, "- {line}");
        }
        prompt.push_str("Read anything in them, but change files only in yours, with bash too.\n");
    }
    if !agent.prompt.is_empty() {
        // Placeholders as in the manual ({{user}}, {{home}}…), filled in here
        // so the saved prompt stays as written: a default stays the default.
        let _ = write!(prompt, "\n## Your instructions\n\n{}\n", crate::manual::render(&agent.prompt, home, render.user));
    }
    for text in render.sections.iter().chain(render.pinned) {
        let _ = write!(prompt, "\n{text}");
    }
    if !render.secrets.is_empty() {
        prompt.push_str("\n## Secrets\n\nThe person gave you these secrets from Claktory's vault. To use one, pass its name in bash's `secrets` (`\"secrets\": [\"AWS\"]`): its variables are set for that command only. You never see their values: output that holds one shows `[secret NAME/VARIABLE]` instead. Never print a value, or write one to a file or a commit.\n");
        for line in render.secrets.values() {
            let _ = writeln!(prompt, "- {line}");
        }
    }
    let _ = write!(
        prompt,
        "\n## How you persist\n\nYour conversation continues across messages and restarts. When your context reaches {compact} tokens, Claktory summarizes it and you continue from the summary, so keep important decisions and progress in the conversation or in files.\n",
        compact = render.compact_at,
    );
    // The core for this agent's settings: crediting Claktory, and the
    // co-author trailer only once there's an address for it.
    let core = crate::manual::manual().core_for(|key| match key {
        "attribution" => render.attribution,
        "co_author" => render.attribution && crate::manual::CO_AUTHOR.is_some(),
        _ => true,
    });
    let manual = crate::manual::render(&core, home, render.user);
    let _ = write!(
        prompt,
        "\n{manual}\n## This Claktory now\n\nYour id in the settings is \"{id}\". Models available now, with their efforts:\n",
        id = agent.id,
    );
    if render.models.is_empty() {
        let _ = writeln!(prompt, "- none (no provider is signed in)");
    }
    for model in render.models {
        let _ = writeln!(prompt, "- {} ({}): {}", model.id, model.name, model.efforts.join(", "));
    }
    for file in INSTRUCTION_FILES {
        if let Ok(text) = tokio::fs::read_to_string(folder.join(file)).await {
            let _ = write!(prompt, "\n## Project instructions ({file})\n\n{text}\n");
            break;
        }
    }
    prompt
}

/// A queued message as the agent gets it. The person's attached images go
/// in as images too when its model takes them (`images`) and they're small
/// enough; every attachment is also named by path.
pub(crate) fn user_message(queued: Queued, images: bool) -> UserMessage {
    let now = factory_agent::clock::now_ms();
    match queued.event {
        Some(event) => UserMessage::event(queued.text, event, now),
        None => {
            let mut message = UserMessage::text(queued.text, now);
            if images && let UserContent::Parts(parts) = &mut message.content {
                parts.extend(queued.attachments.iter().filter_map(factory_agent::Attachment::image));
            }
            UserMessage {
                sent_from: queued.sent_from,
                client_id: queued.client_id,
                attachments: queued.attachments,
                voice: queued.voice.as_ref().and_then(crate::speech::voice::Voice::heard),
                checklist_item: queued.checklist_item,
                ..message
            }
        }
    }
}

impl Factory {
    /// Whether the agent's model takes images.
    pub(crate) fn takes_images(&self, agent: &str) -> bool {
        let model = self.lock().config.agent(agent).map(|agent| agent.model.clone()).unwrap_or_default();
        self.models.resolve(&model).is_some_and(|model| model.input.iter().any(|input| input == "image"))
    }
}

/// Why `model` can't run, for the conversation: its provider isn't signed
/// in, or it is and no longer offers that model (its list dropped it;
/// Abbacchio's LOW on #185 U4). `models`: what can run now; `choose`: where
/// to pick another (" in this agent's settings").
pub(crate) fn no_model(factory: &Factory, models: &[v1::Model], model: &str, choose: &str) -> String {
    let provider = model.split_once('/').map_or(model, |(provider, _)| provider);
    let signed_in = models.iter().any(|listed| listed.id.split_once('/').is_some_and(|(id, _)| id == provider));
    let name = factory.lock().providers.iter().find(|found| found.id == provider).map(|found| found.name.clone());
    match name {
        Some(name) if signed_in => format!("No model available: {name} doesn't offer \"{model}\" any more. Choose another model{choose}."),
        _ => format!(
            "No model available: \"{model}\" needs its provider signed in. Sign in under Settings → Providers, or choose another model{choose}."
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The home isn't the agent's: its file tools reach only its own folder
    /// there (and config.json when it's an admin or has the manual), never
    /// auth.json, factory.db or another agent's folder. #209's finding.
    #[tokio::test]
    async fn file_tools_reach_only_the_agents_own_corner_of_the_home() {
        let dir = std::env::temp_dir().join(format!("factoryd-places-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("dir");
        let home = Home::new(dir.clone());
        let mine = home.agent_dir("a");
        std::fs::create_dir_all(mine.join("uploads")).expect("uploads");
        std::fs::create_dir_all(home.workspace()).expect("workspace");
        std::fs::create_dir_all(home.root().join("pipes/incoming/p/1")).expect("pipes");
        std::fs::write(home.root().join("auth.json"), "{\"secret\": true}").expect("auth");
        std::fs::write(home.config_path(), "{}").expect("config");
        std::fs::write(home.root().join("factory.db"), "db").expect("db");
        std::fs::write(mine.join("uploads/u.txt"), "up").expect("upload");
        std::fs::write(home.root().join("pipes/incoming/p/1/f.txt"), "from the pipe").expect("pipe file");
        let project = dir.join("proj");
        std::fs::create_dir_all(project.join("app")).expect("project");
        std::fs::create_dir_all(project.join("pipes/ops/outbox")).expect("outbox");
        let config = Config::parse(
            &serde_json::json!({"projects": [
                {"id": "p", "name": "P", "path": project,
                 "folders": [{"id": "app", "name": "App", "path": project.join("app")}]},
            ], "agents": [
                {"id": "a", "name": "A", "model": "m/one", "effort": "high", "prompt": ""},
                {"id": "rivet", "name": "Rivet", "model": "m/one", "effort": "high", "prompt": "",
                 "extensions": {"agents": {"admin": true}}},
                {"id": "f", "name": "F", "model": "m/one", "effort": "high", "prompt": "",
                 "project": "p", "folder": "app"},
            ]})
            .to_string(),
            home.root(),
        )
        .expect("config");

        let tools_with = |id: &str, outboxes: &[PathBuf]| {
            let placed = places(&home, &config, config.agent(id).expect("agent"), outboxes);
            let mut tools = factory_agent::coding_tools(&placed.folder, &placed.extra_dirs, None);
            factory_agent::tools::read_only(&mut tools, &placed.read_only);
            tools
        };
        let tools = |id: &str| tools_with(id, &[]);
        let run = async |id: &str, tool: &str, args: serde_json::Value| {
            let tool = tools(id).into_iter().find(|candidate| candidate.spec.name == tool).expect("tool");
            tool.execute(&args, &tokio_util::sync::CancellationToken::new(), &|_| {}).await
        };
        let path = |at: PathBuf| at.display().to_string();

        // Its own corner works, reading and writing.
        run("a", "read_file", serde_json::json!({"path": path(mine.join("uploads/u.txt"))})).await.expect("its upload reads");
        run("a", "write_file", serde_json::json!({"path": path(mine.join("notes.txt")), "content": "ok"}))
            .await
            .expect("its folder writes");
        // What a pipe brought in reads, and only reads.
        run("a", "read_file", serde_json::json!({"path": path(home.root().join("pipes/incoming/p/1/f.txt"))}))
            .await
            .expect("pipe files read");
        run("a", "write_file", serde_json::json!({"path": path(home.root().join("pipes/incoming/p/1/f.txt")), "content": "x"}))
            .await
            .expect_err("pipe files don't write");
        // The rest of the home is out of reach, for reading and writing.
        for target in ["auth.json", "factory.db"] {
            run("a", "read_file", serde_json::json!({"path": path(home.root().join(target))})).await.expect_err(target);
            run("a", "write_file", serde_json::json!({"path": path(home.root().join(target)), "content": "x"})).await.expect_err(target);
        }
        run("a", "read_file", serde_json::json!({"path": path(home.config_path())})).await.expect_err("config.json refused");
        run("a", "read_file", serde_json::json!({"path": path(home.agent_dir("rivet").join("x"))}))
            .await
            .expect_err("another agent's folder refused");

        // An admin agent still changes config.json with its file tools, as
        // the manual tells it to, and still can't read auth.json.
        run(
            "rivet",
            "edit_file",
            serde_json::json!({"path": path(home.config_path()), "edits": [{"oldText": "{}", "newText": "{ }", "replaceAll": false}]}),
        )
        .await
        .expect("the admin edits config.json");
        run("rivet", "read_file", serde_json::json!({"path": path(home.root().join("auth.json"))}))
            .await
            .expect_err("auth.json refused for the admin too");

        // A folder-scoped agent granted a pipe writes the pipe's outbox
        // ("Files you send come from its outbox"), inside the project's
        // otherwise read-only own folder, and nothing else of it.
        let outbox = project.join("pipes/ops/outbox");
        let scoped = async |tool: &str, args: serde_json::Value| {
            let tool = tools_with("f", std::slice::from_ref(&outbox)).into_iter().find(|t| t.spec.name == tool).expect("tool");
            tool.execute(&args, &tokio_util::sync::CancellationToken::new(), &|_| {}).await
        };
        scoped("write_file", serde_json::json!({"path": path(outbox.join("report.txt")), "content": "out"}))
            .await
            .expect("the pipe's outbox writes");
        scoped("read_file", serde_json::json!({"path": path(outbox.join("report.txt"))})).await.expect("and reads");
        scoped("write_file", serde_json::json!({"path": path(project.join("notes.txt")), "content": "x"}))
            .await
            .expect_err("the project's own folder stays read only");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn what_changed_is_said_in_one_note() {
        let map = |pairs: &[(&str, &str)]| pairs.iter().map(|(id, label)| ((*id).to_owned(), (*label).to_owned())).collect();
        let frame = Frame {
            settings: String::new(),
            system_prompt: String::new(),
            tools: Vec::new(),
            told: String::new(),
            roster: map(&[("writer", "Writer (writer)")]),
            models: map(&[("a/one", "a/one (One)"), ("a/two", "a/two (Two)")]),
            folders: BTreeMap::new(),
            secrets: BTreeMap::new(),
            apps: BTreeMap::new(),
            pipes: BTreeMap::new(),
            announced: Vec::new(),
        };
        assert_eq!(changes(&frame, &frame.roster, &frame.models, &frame.folders, &frame.secrets), None);
        let said = changes(
            &frame,
            &frame.roster,
            &map(&[("a/one", "a/one (One)"), ("b/three", "b/three (Three)")]),
            &frame.folders,
            &frame.secrets,
        );
        assert_eq!(said.as_deref(), Some("Models now available: b/three (Three).\nModels no longer available: a/two (Two)."));
    }
}
