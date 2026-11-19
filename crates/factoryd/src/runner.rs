//! Runs agents: one task per working agent, working through its queued
//! messages. Messages that arrive while it works reach it between steps.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use factory_agent::{
    Agent, AgentEvent, AgentOptions, Arrived, Background, Event, EventSource, Finished, Inbox, Message, StopReason, Task, ThinkingLevel,
    Tool, ToolSpec, UserMessage,
};
use factory_protocol::v1::{self, NoticeLevel};

use crate::config::{AgentConfig, Config, ProjectConfig};
use crate::extensions::{Extension, PromptKind};
use crate::factory::{Factory, Inner, Queued, write_atomic};
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
    /// Compact before anything else (/compact).
    compact: bool,
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
}

/// Where an agent works, and what else its file tools may reach: the Factory
/// home, which they may change (so it can change Factory's settings, itself
/// included), and the project's other folders, which they may only read.
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

pub(crate) fn places(home: &Home, config: &Config, agent: &AgentConfig) -> Places {
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
    // With a checkout of its own, the main checkout is only to read.
    let folder = match checkout(home, config, agent) {
        Some(checkout) => {
            read_only.push(checkout.repo);
            checkout.folder
        }
        None => source,
    };
    Places { folder, extra_dirs: vec![home.root().to_path_buf()], read_only }
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
fn settings_key(config: &Config, agent: &AgentConfig, cwd: &Path) -> String {
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
    key.to_string()
}

/// The models the system prompt lists, by id.
fn model_lines(models: &[v1::Model]) -> BTreeMap<String, String> {
    models.iter().map(|model| (model.id.clone(), format!("{} ({})", model.id, model.name))).collect()
}

fn enabled_extensions(agent: &AgentConfig) -> Vec<&'static dyn Extension> {
    crate::extensions::ALL.into_iter().filter(|extension| crate::extensions::active(agent, *extension)).collect()
}

/// What changed that the agent was told otherwise, in one note; None if nothing.
fn changes(
    frame: &Frame,
    roster: &BTreeMap<String, String>,
    models: &BTreeMap<String, String>,
    folders: &BTreeMap<String, String>,
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
    (!said.is_empty()).then(|| said.join("\n"))
}

/// The tools a frame lists, with its specs, in its order. Tools it doesn't
/// list wait for the next segment; those it lists that are gone, such as an
/// extension's that was turned off, are refused, saying why.
fn fit(mut tools: Vec<Tool>, specs: &[ToolSpec], agent: &AgentConfig) -> Vec<Tool> {
    let mut fitted = Vec::with_capacity(specs.len());
    for spec in specs {
        let mut tool = match tools.iter().position(|tool| tool.spec.name == spec.name) {
            Some(index) => tools.swap_remove(index),
            None => refused(spec, agent),
        };
        tool.spec = spec.clone();
        fitted.push(tool);
    }
    fitted
}

/// A tool of the frame that's gone: calling it says why.
fn refused(spec: &ToolSpec, agent: &AgentConfig) -> Tool {
    let name = spec.name.to_string();
    let owner = crate::extensions::ALL.into_iter().find(|extension| extension.owns_tool(&name)).map(|extension| extension.manifest());
    let reason = match owner {
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
fn frame_model(frame: &Frame) -> String {
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
    config: &Config,
    agent: &AgentConfig,
    folder: &Path,
    can_land: bool,
    tools: &[ToolSpec],
) -> Option<(String, Vec<&'static str>)> {
    use crate::extensions::{OptionKind, agent_list, enabled, switch};
    let then: serde_json::Value = serde_json::from_str(told).ok()?;
    let text = |key: &str| then.get(key).and_then(serde_json::Value::as_str).unwrap_or_default().to_owned();
    // Turned on, it can use them now if its frame can call tools it doesn't list.
    let call_now = tools.iter().any(|tool| tool.name == EXTENSION_CALL);
    let mut said = Vec::new();
    let mut turned_on = Vec::new();
    if text("model") != agent.model {
        said.push(format!("your model is now {}", agent.model));
    }
    if text("name") != agent.name {
        said.push(format!("your name is now {}", agent.name));
    }
    if text("role") != agent.role {
        said.push(if agent.role.is_empty() { "you no longer have a role".to_owned() } else { format!("your role is now {}", agent.role) });
    }
    if text("effort") != agent.effort {
        said.push(format!("your effort is now {}", agent.effort));
    }
    let project_then = then.get("project").and_then(serde_json::Value::as_str);
    let moved = project_then != agent.project.as_deref();
    if moved {
        said.push(match agent.project.as_deref().and_then(|project| config.project(project)) {
            Some(project) => format!("you're now in the project \"{}\"", project.name),
            None => "you're now in Factory's workspace, outside any project".to_owned(),
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
                    said.push(format!("the {} option {} is now {now}", manifest.name, option.id));
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
            section.push_str(
                "tree_land arrives with your next context (after compaction or /clear); until then, leave landing to the person.\n",
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
    "Call a tool Factory told you is yours now but isn't in your tool list yet: tool is its name, args its arguments.";

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
    for extension in enabled_extensions(agent) {
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
        loop {
            if let Err(error) = self.ensure_loaded(&id).await {
                tracing::error!(agent = %id, %error, "could not load transcript");
                let mut inner = self.lock();
                if let Some(runtime) = inner.agents.get_mut(&id) {
                    runtime.running = false;
                    let dropped = std::mem::take(&mut runtime.queue);
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
            if !crate::extensions::enabled(agent, crate::extensions::tree::ID) {
                return;
            }
            let Ok(checkout) = crate::trees::plan(&self.home, id, &source_folder(&self.home, &inner.config, agent)) else { return };
            if checkout.exists() || crate::trees::failure(&checkout).is_some() {
                return;
            }
            let Some(runtime) = inner.agents.get(id) else { return };
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
        let compact_at = inner.config.compact_at_tokens;
        let models = inner.models.clone();
        let places = agent.as_ref().map(|agent| places(&self.home, &inner.config, agent));
        let runtime = inner.agents.get_mut(id)?;
        let Some(agent) = agent else {
            runtime.running = false;
            return None;
        };
        if runtime.queue.is_empty() && !runtime.compact {
            runtime.running = false;
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
        let note = if compact { None } else { self.take_changes(&mut inner, id, current) };
        let runtime = inner.agents.get_mut(id)?;
        let mut prompts = std::mem::take(&mut runtime.queue);
        prompts.splice(0..0, note);
        let (epoch, compact) = (runtime.epoch, std::mem::take(&mut runtime.compact));
        let frame = runtime
            .transcript
            .as_ref()
            .and_then(|transcript| transcript.frame())
            .filter(|frame| frame_model(frame) == agent.model)
            .cloned();
        let run = self.begin_run(id, runtime);
        runtime.taken.extend(prompts.iter().map(|queued| (queued.clone(), run)));
        let messages = runtime.transcript.as_ref().map(|transcript| transcript.messages().to_vec()).unwrap_or_default();
        let extensions = enabled_extensions(&agent);
        self.broadcast_workspace(&inner);
        let places = places?;
        Some(Job { prompts, agent, compact_at, messages, models, extensions, frame, places, epoch, compact })
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
        let folder = places(&self.home, &inner.config, &agent).folder;
        let key = settings_key(&inner.config, &agent, &folder);
        let roster = crate::extensions::agents::roster(&inner.config, &agent);
        let folders = folder_lines(&inner.config, &agent);
        let names = model_lines(&inner.models);
        // tree_land in its tools, or reached through extension_call.
        let can_land = frame.tools.iter().any(|tool| tool.name == "tree_land" || tool.name == EXTENSION_CALL);
        let (settings, turned_on) = match settings_note(frame.told(), &self.home, &inner.config, &agent, &folder, can_land, &frame.tools) {
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
        let notes: Vec<String> = [settings, changes(&frame, &roster, &names, &folders)].into_iter().flatten().collect();
        if notes.is_empty() {
            return None;
        }
        frame.told = key;
        frame.roster = roster;
        frame.models = names;
        frame.folders = folders;
        // Left out of compaction: the next frame says it all.
        let event =
            Event { source: EventSource::Host, from: "Factory".to_owned(), id: String::new(), summary: String::new(), transient: true };
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

    /// Writes the segment's frame from Factory as it is now, keeps it with the
    /// segment, and returns it; None if the agent or its conversation is gone.
    /// `announced` are the tools the agent was told it can call through
    /// extension_call, for a frame that keeps the tools it had.
    async fn new_frame(&self, id: &str, epoch: u64, tools: Vec<ToolSpec>, announced: Vec<String>) -> Option<Frame> {
        let (agent, project, compact_at, models, sections, pinned, roster, folders, places, settings, folder_name) = {
            let mut inner = self.lock();
            if inner.agents.get(id).is_none_or(|runtime| runtime.epoch != epoch) {
                return None;
            }
            let agent = inner.config.agent(id)?.clone();
            let project = agent.project.as_deref().and_then(|project| inner.config.project(project).cloned());
            let (sections, pinned) = self.sections(&mut inner, id, &agent, &enabled_extensions(&agent));
            let roster = crate::extensions::agents::roster(&inner.config, &agent);
            let folders = folder_lines(&inner.config, &agent);
            let places = places(&self.home, &inner.config, &agent);
            let settings = settings_key(&inner.config, &agent, &places.folder);
            let folder_name = inner.config.agent_folder(&agent).map(|folder| folder.name.clone());
            let compact_at = inner.config.compact_at_tokens;
            (agent, project, compact_at, inner.models.clone(), sections, pinned, roster, folders, places, settings, folder_name)
        };
        let render = Render {
            agent: &agent,
            project: project.as_ref(),
            compact_at,
            models: &models,
            sections: &sections,
            pinned: &pinned,
            folder_name: folder_name.as_deref(),
            folders: &folders,
        };
        let system_prompt = system_prompt(&render, &places.folder, &self.home, &places.extra_dirs).await;
        let frame = Frame { settings, system_prompt, tools, told: String::new(), roster, models: model_lines(&models), folders, announced };
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
            let problem = format!(
                "No model available: \"{}\" needs its provider signed in. Sign in under Settings → Providers, or choose another model in this agent's settings.",
                job.agent.model
            );
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
            Some(frame) => Ok((frame.system_prompt, fit(tools, &frame.tools, &job.agent))),
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
        let mut agent = Agent::new(AgentOptions {
            system_prompt,
            model,
            thinking_level,
            tools,
            messages: job.messages,
            stream: self.models.stream(),
            session_id: Some(id.to_owned()),
            transform_context: None,
            should_stop_after_turn: None,
            before_tool_call: None,
            compact_at: Some(job.compact_at),
            inbox: Some(self.inbox(id, job.epoch)),
            after_compaction: Some(after_compaction),
        });
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
        if !self.set_abort(id, Some(agent.abort_handle())) {
            self.unsent(id, &job.prompts, "Stopped.".to_owned(), NoticeLevel::Info);
            return;
        }
        let mut prompts = Vec::with_capacity(job.prompts.len());
        for queued in job.prompts {
            prompts.push(user_message(queued));
        }
        if job.compact {
            match agent.compact().await {
                Ok(()) => {}
                Err(factory_agent::Error::Aborted) => prompts.clear(),
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
    fn latch_failure(&self, id: &str, epoch: u64, failed: bool) {
        let mut inner = self.lock();
        if let Some(runtime) = inner.agents.get_mut(id).filter(|runtime| runtime.epoch == epoch) {
            runtime.failed = failed;
        }
    }

    /// The agent's coding tools, where it works now, its extensions' tools,
    /// and extension_call.
    fn build_tools(self: &Arc<Self>, id: &str, places: &Places, extensions: &[&'static dyn Extension]) -> Vec<Tool> {
        let mut tools = factory_agent::coding_tools(&places.folder, &places.extra_dirs, Some(self.background(id)));
        factory_agent::tools::read_only(&mut tools, &places.read_only);
        for extension in extensions {
            tools.extend(extension.tools(self, id));
        }
        tools.push(self.extension_call(id));
        tools
    }

    /// The agent's extensions' tools as its settings are now.
    fn extension_tools(self: &Arc<Self>, id: &str) -> Vec<Tool> {
        let Some(agent) = self.lock().config.agent(id).cloned() else { return Vec::new() };
        enabled_extensions(&agent).into_iter().flat_map(|extension| extension.tools(self, id)).collect()
    }

    fn extension_specs(self: &Arc<Self>, id: &str) -> Vec<ToolSpec> {
        self.extension_tools(id).into_iter().map(|tool| tool.spec).collect()
    }

    /// Calls a tool of the agent's extensions by name, as its settings are
    /// now: how it uses an extension turned on since its context began.
    fn extension_call(self: &Arc<Self>, id: &str) -> Tool {
        let factory = Arc::downgrade(self);
        let agent = id.to_owned();
        let parameters = serde_json::json!({
            "type": "object",
            "required": ["tool"],
            "properties": {"tool": {"type": "string"}, "args": {"type": "object"}},
            "additionalProperties": false,
        });
        let handler: factory_agent::Handler = Arc::new(move |args| {
            let (factory, agent) = (factory.upgrade(), agent.clone());
            Box::pin(async move {
                let Some(factory) = factory else { return Err(factory_agent::Error::Other("Factory is stopping.".into())) };
                let name = args.get("tool").and_then(serde_json::Value::as_str).unwrap_or_default().to_owned();
                let tools = factory.extension_tools(&agent);
                let Some(tool) = tools.into_iter().find(|tool| tool.spec.name == name) else {
                    return Err(factory_agent::Error::Other(format!(
                        "No tool \"{name}\" of an extension is on for you; extension_call reaches only those Factory told you of."
                    )));
                };
                let args = args.get("args").cloned().unwrap_or_else(|| serde_json::json!({}));
                tool.execute(&args, &tokio_util::sync::CancellationToken::new(), &|_| {}).await
            })
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
        let now = {
            let inner = self.lock();
            settings_key(&inner.config, &agent, &places(&self.home, &inner.config, &agent).folder)
        };
        let now: serde_json::Value = serde_json::from_str(&now).unwrap_or_default();
        let changed = |keys: &[&str]| keys.iter().any(|key| then.get(key) != now.get(key));
        // Its project, folder or checkout.
        if changed(&["project", "folder", "cwd"]) {
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
            let places = places(&self.home, &inner.config, &agent);
            (agent, places)
        };
        Some(self.build_tools(id, &places, &enabled_extensions(&agent)))
    }

    /// Records an agent event: updates the transcript, tells clients, saves.
    async fn apply(&self, id: &str, epoch: u64, event: AgentEvent) {
        let persist = {
            let mut inner = self.lock();
            let Some(runtime) = inner.agents.get_mut(id) else { return };
            // A run of a conversation the person cleared.
            if runtime.epoch != epoch {
                return;
            }
            let writes = runtime.writes.clone();
            let doing = match &event {
                AgentEvent::ToolExecutionStart { tool_name, .. } => Some(Some(tool_name.clone())),
                AgentEvent::ToolExecutionEnd { .. } | AgentEvent::AgentEnd { .. } => Some(None),
                _ => None,
            };
            // Its character works while a tool runs, and thinks otherwise.
            let doing_changed = doing.as_ref().is_some_and(|doing| *doing != runtime.doing);
            if let Some(doing) = doing {
                runtime.doing = doing;
            }
            let Some(transcript) = runtime.transcript.as_mut() else { return };
            let applied = match transcript.apply(event) {
                Ok(applied) => applied,
                Err(error) => {
                    tracing::error!(agent = %id, %error, "could not serialize transcript");
                    return;
                }
            };
            for change in applied.changes {
                self.send_change(&inner, id, change);
            }
            if applied.context_changed || doing_changed {
                self.broadcast_workspace(&inner);
            }
            (applied.persist, applied.user_message, writes)
        };
        let (Some(persist), user_message, writes) = persist else { return };
        let _writing = writes.lock().await;
        if self.lock().agents.get(id).is_none_or(|runtime| runtime.epoch != epoch) {
            return;
        }
        if let Err(error) = write_atomic(&persist.path, &persist.bytes).await {
            tracing::error!(agent = %id, path = %persist.path.display(), %error, "could not save transcript");
            return;
        }
        if user_message && let Some(runtime) = self.lock().agents.get_mut(id) {
            self.delivered(runtime, Some(1));
        }
    }

    /// Lets the person stop the run; false when they already did, while it
    /// was still starting.
    fn set_abort(&self, id: &str, abort: Option<factory_agent::AbortHandle>) -> bool {
        let mut inner = self.lock();
        let Some(runtime) = inner.agents.get_mut(id) else { return false };
        if runtime.stopped && abort.is_some() {
            return false;
        }
        runtime.abort = abort;
        true
    }

    fn notice(&self, id: &str, epoch: u64, text: String, level: NoticeLevel) {
        let mut inner = self.lock();
        let Some(runtime) = inner.agents.get_mut(id).filter(|runtime| runtime.epoch == epoch) else { return };
        let Some(transcript) = runtime.transcript.as_mut() else { return };
        let entry = transcript.notice(text, level);
        self.send_change(&inner, id, Change::Entry(entry));
    }

    /// Shows prompts that never reached the model, and why.
    fn unsent(&self, id: &str, prompts: &[Queued], problem: String, level: NoticeLevel) {
        self.settle(prompts, Outcome::Dropped);
        let mut inner = self.lock();
        if let Some(runtime) = inner.agents.get_mut(id) {
            runtime.taken.retain(|(taken, _)| prompts.iter().all(|prompt| prompt.id != taken.id));
        }
        let Some(transcript) = inner.agents.get_mut(id).and_then(|runtime| runtime.transcript.as_mut()) else { return };
        let mut entries = Vec::with_capacity(prompts.len() + 1);
        for queued in prompts {
            entries.push(transcript.unsent(&user_message(queued.clone())));
        }
        entries.push(transcript.notice(problem, level));
        for entry in entries {
            self.send_change(&inner, id, Change::Entry(entry));
        }
    }

    /// What arrived for a working agent since its last step: the person's
    /// messages and events, which it reads before its next model call. None
    /// once the person stopped it.
    fn inbox(self: &Arc<Self>, id: &str, epoch: u64) -> Inbox {
        let factory = Arc::downgrade(self);
        let agent = id.to_owned();
        Arc::new(move || {
            let (factory, agent) = (factory.upgrade(), agent.clone());
            Box::pin(async move {
                let Some(factory) = factory else { return Arrived::default() };
                let current = factory.extension_specs(&agent);
                let messages = {
                    let mut inner = factory.lock();
                    if inner.agents.get(&agent).is_none_or(|runtime| runtime.abort.is_none()) {
                        return Arrived::default();
                    }
                    let note = factory.take_changes(&mut inner, &agent, &current);
                    let Some(runtime) = inner.agents.get_mut(&agent) else { return Arrived::default() };
                    let mut arrived = std::mem::take(&mut runtime.queue);
                    arrived.splice(0..0, note);
                    if !arrived.is_empty() {
                        let run = factory.begin_run(&agent, runtime);
                        runtime.taken.extend(arrived.iter().map(|queued| (queued.clone(), run)));
                        factory.broadcast_workspace(&inner);
                    }
                    arrived.into_iter().map(user_message).collect()
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
        Background {
            dir: self.home.agent_dir(id).join("tasks"),
            started,
            finished: Arc::new(move |done: Finished| {
                if let Some(factory) = factory.upgrade() {
                    factory.task_finished(&agent, done);
                }
            }),
        }
    }
}

async fn system_prompt(render: &Render<'_>, folder: &Path, home: &Home, extra_dirs: &[PathBuf]) -> String {
    let agent = render.agent;
    let mut prompt = factory_agent::coding_prompt(folder, extra_dirs);
    // Writing to a String can't fail.
    let _ = write!(prompt, "\n\n# Factory\n\nYou are {}, a persistent agent in Factory. ", agent.name);
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
            "You work in Factory's workspace, {}, outside any project. Each project is a folder inside it.",
            folder.display()
        ),
    };
    if !render.folders.is_empty() {
        prompt.push_str("The project's folders:\n");
        for line in render.folders.values() {
            let _ = writeln!(prompt, "- {line}");
        }
        prompt.push_str("Read anything in them, but change files only in yours, with bash too.\n");
    }
    if !agent.prompt.is_empty() {
        let _ = write!(prompt, "\n## Your instructions\n\n{}\n", agent.prompt);
    }
    for text in render.sections.iter().chain(render.pinned) {
        let _ = write!(prompt, "\n{text}");
    }
    let _ = write!(
        prompt,
        "\n## How you persist\n\nYour conversation continues across messages and restarts. When your context reaches {compact} tokens, Factory summarizes it and you continue from the summary, so keep important decisions and progress in the conversation or in files.\n",
        compact = render.compact_at,
    );
    let manual = crate::manual::render(&crate::manual::manual().core, home);
    let _ = write!(
        prompt,
        "\n{manual}\n## This Factory now\n\nYour id in the settings is \"{id}\". Models available now, with their efforts:\n",
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

fn user_message(queued: Queued) -> UserMessage {
    let now = factory_agent::clock::now_ms();
    match queued.event {
        Some(event) => UserMessage::event(queued.text, event, now),
        None => UserMessage::text(queued.text, now),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
            announced: Vec::new(),
        };
        assert_eq!(changes(&frame, &frame.roster, &frame.models, &frame.folders), None);
        let said = changes(&frame, &frame.roster, &map(&[("a/one", "a/one (One)"), ("b/three", "b/three (Three)")]), &frame.folders);
        assert_eq!(said.as_deref(), Some("Models now available: b/three (Three).\nModels no longer available: a/two (Two)."));
    }
}
