//! Apps: integrations Claktory runs once, for every agent (a Telegram bot,
//! say). Each is a WebAssembly component the apps host, claktory-apps, runs
//! in its own sandbox (`host`); factoryd holds what's installed and approved
//! (config.json's `apps`), starts and stops them, keeps their secrets in the
//! vault given to the grantee "app:<id>", offers their tools to the agents
//! given their scopes, and delivers their events to the agents subscribed.
//!
//! The host is mechanism; the policy is here: an app runs when the person
//! wants it running and its setup is done; a crash restarts it after 1 s,
//! 5 s, 30 s, then 5 min, and three in ten minutes pause it. An app can
//! never give itself anything: grants, subscriptions and secrets come only
//! from the person.

pub(crate) mod a2ui;
pub(crate) mod access;
pub(crate) mod bundle;
pub mod bundled;
pub(crate) mod changes;
pub(crate) mod files;
pub(crate) mod data;
mod events;
pub(crate) mod host;
pub(crate) mod pins;
pub(crate) mod surfaces;

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, Weak};
use std::time::{Duration, Instant};

use factory_agent::{Tool, ToolResult};
use factory_protocol::apps::{self as wire, frame::Body};
use factory_protocol::v1;
use serde::{Deserialize, Serialize};

pub use host::{AppsLink, Launcher};
pub(crate) use surfaces::Viewer;

use crate::config::Config;
use crate::factory::{Factory, RequestError, host_event};
use bundle::{Bundle, Manifest, Tier};
use host::Host;

/// How long a tool call may take: the host's 60 s, and time to say so.
const CALL_DEADLINE: Duration = Duration::from_secs(70);
/// How long the host may take to load, start or stop an app.
const LIFECYCLE_DEADLINE: Duration = Duration::from_secs(90);
/// Waits after a crash, then the host's failures, one after another.
const BACKOFF: [Duration; 4] = [Duration::from_secs(1), Duration::from_secs(5), Duration::from_secs(30), Duration::from_secs(300)];
/// Three crashes in this long pause an app.
const CRASH_WINDOW: Duration = Duration::from_secs(600);
const CRASHES_TO_PAUSE: usize = 3;

/// An installed app, in config.json's `apps`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AppConfig {
    pub id: String,
    pub version: String,
    /// The person wants it running. Only written when off.
    #[serde(default = "yes", skip_serializing_if = "is_true")]
    pub running: bool,
    /// Its publisher's key (Ed25519, base64), pinned at its first install.
    pub key: String,
    /// Its file's SHA-256, hex.
    pub sha256: String,
    pub installed_ms: i64,
    /// The scopes each agent was given.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub grants: BTreeMap<String, Vec<String>>,
    /// The agents that hear each event kind.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub subscriptions: BTreeMap<String, Vec<String>>,
    /// Its secrets' ids in the vault, by the manifest's names.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub secrets: BTreeMap<String, String>,
    /// The data sources the person let it read (`data`'s ids).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sources: Vec<String>,
    /// The version it was updated from, until it has started once (its
    /// upgrade() runs first).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub upgrade_from: String,
    /// Settings this version doesn't know, kept and written back as they
    /// were (`crate::config::Unknown`).
    #[serde(flatten)]
    pub other: crate::config::Unknown,
}

fn yes() -> bool {
    true
}

fn is_true(value: &bool) -> bool {
    *value
}

/// Who the vault gives an app's secrets to.
pub(crate) fn grantee(app: &str) -> String {
    format!("app:{app}")
}

/// Apps are offered in every Claktory: Claktory's own (Charts, shipped with
/// each release: `bundled`) install by themselves and run and update from
/// the Marketplace's Apps tab (#201, Zucchero's gate passed; #333). Off, only the dev Claktory (`factoryd
/// --dev`) would have them. Telegram isn't shipped until it's proven.
const SHIPPED: bool = true;

/// Apps' state while Claktory runs (what's installed is in config.json).
pub(crate) struct Apps {
    /// Apps are offered here (`SHIPPED`, the dev Claktory, or a test that
    /// set an apps host). When not, there's no trace of them: no apps in
    /// the workspace or the Map, no host, and their requests are refused.
    available: std::sync::atomic::AtomicBool,
    state: Mutex<State>,
    /// Wakes `monitor` to bring the host in line with what's wanted.
    wake: Arc<tokio::sync::Notify>,
    /// One pass of `reconcile` at a time.
    busy: tokio::sync::Mutex<()>,
    /// How the host starts; a stand-in in tests.
    launcher: Mutex<Option<Launcher>>,
    pub(crate) events: events::Events,
    /// The views open now.
    surfaces: surfaces::Surfaces,
    /// The dock's pins.
    pins: pins::Pins,
    /// Claktory's own apps this release ships.
    pub(crate) bundled: bundled::Bundled,
}

#[derive(Default)]
struct State {
    /// The installed version's manifest, by app id.
    manifests: BTreeMap<String, Arc<Manifest>>,
    apps: BTreeMap<String, Runtime>,
    host: Option<Arc<Host>>,
    /// Why there's no host, for the person; empty when there is.
    host_problem: String,
    host_failures: usize,
    host_retry_at: Option<Instant>,
}

#[derive(Default)]
struct Runtime {
    /// The version the host has loaded; empty for none.
    loaded: String,
    /// A version the host couldn't load: not tried again until it changes.
    failed: String,
    running: bool,
    starting: bool,
    /// About its state, for the person: the last error.
    detail: String,
    /// The app's own line.
    status: String,
    crashes: Vec<Instant>,
    paused: bool,
    retry_at: Option<Instant>,
}

impl Default for Apps {
    fn default() -> Apps {
        let dev = std::env::args().any(|arg| arg == "--dev");
        Apps {
            available: std::sync::atomic::AtomicBool::new(SHIPPED || dev),
            state: Mutex::default(),
            wake: Arc::default(),
            busy: tokio::sync::Mutex::default(),
            launcher: Mutex::default(),
            events: events::Events::default(),
            surfaces: surfaces::Surfaces::default(),
            pins: pins::Pins::default(),
            bundled: bundled::Bundled::default(),
        }
    }
}

impl Apps {
    /// Whether apps are offered in this Claktory.
    pub(crate) fn available(&self) -> bool {
        self.available.load(std::sync::atomic::Ordering::Relaxed)
    }

    fn state(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().expect("apps state")
    }

    /// Brings the host in line with what's wanted, soon.
    pub(crate) fn poke(&self) {
        self.wake.notify_one();
    }

    pub(crate) fn manifest(&self, id: &str) -> Option<Arc<Manifest>> {
        self.state().manifests.get(id).cloned()
    }
}

/// The installed app `id`'s manifest, read from its folder.
fn read_manifest(factory: &Factory, app: &AppConfig) -> Result<Manifest, String> {
    let path = factory.home.root().join("apps").join(&app.id).join(&app.version).join("manifest.toml");
    let text = std::fs::read_to_string(&path).map_err(|_| "This app's files are missing. Install it again.".to_owned())?;
    Manifest::parse(&text)
}

/// Whether each of the app's secrets is in the vault, given to it.
fn setup_done(app: &AppConfig, manifest: &Manifest, vault: &v1::VaultState) -> Vec<(String, bool)> {
    let grantee = grantee(&app.id);
    manifest
        .secrets
        .iter()
        .map(|secret| {
            let done = app.secrets.get(&secret.name).is_some_and(|id| {
                vault.secrets.iter().any(|kept| &kept.id == id && kept.grants.iter().any(|grant| grant.agent_id == grantee))
            });
            (secret.name.clone(), done)
        })
        .collect()
}

fn tier(tier: Tier) -> v1::AppTier {
    match tier {
        Tier::Claktory => v1::AppTier::Claktory,
        Tier::Local => v1::AppTier::Local,
    }
}

fn scopes(manifest: &Manifest, key: &str) -> Vec<v1::AppScopeSpec> {
    manifest
        .scopes
        .iter()
        .map(|scope| v1::AppScopeSpec {
            id: scope.id.clone(),
            label: scope.label.clone(),
            tools: scope.tools.clone(),
            reads_data: scope_reads(manifest, key, scope),
        })
        .collect()
}

fn event_kinds(manifest: &Manifest) -> Vec<v1::AppEventKind> {
    manifest
        .events
        .iter()
        .map(|event| v1::AppEventKind { kind: event.kind.clone(), label: event.label.clone(), scope: event.scope.clone() })
        .collect()
}

fn secret_items(manifest: &Manifest, done: &[(String, bool)]) -> Vec<v1::AppSetupItem> {
    manifest
        .secrets
        .iter()
        .map(|secret| v1::AppSetupItem {
            kind: v1::AppSetupKind::Secret.into(),
            name: secret.name.clone(),
            label: secret.label.clone(),
            description: secret.description.clone(),
            done: done.iter().any(|(name, done)| name == &secret.name && *done),
        })
        .collect()
}

/// The apps installed, for clients.
pub(crate) fn infos(factory: &Factory, config: &Config, vault: &v1::VaultState) -> Vec<v1::AppInfo> {
    if !factory.apps.available() {
        return Vec::new();
    }
    let state = factory.apps.state();
    let mut infos = Vec::new();
    for app in &config.apps {
        let runtime = state.apps.get(&app.id);
        let Some(manifest) = state.manifests.get(&app.id) else {
            infos.push(v1::AppInfo {
                id: app.id.clone(),
                name: app.id.clone(),
                version: app.version.clone(),
                state: v1::AppState::Unavailable.into(),
                state_detail: runtime.map(|runtime| runtime.detail.clone()).unwrap_or_default(),
                installed_ms: app.installed_ms,
                publisher_key: bundle::short_key(&app.key),
                tier: tier(tier_of(&app.key)).into(),
                bundled: bundled::is_bundled(factory, &app.id, &app.key),
                ..Default::default()
            });
            continue;
        };
        let done = setup_done(app, manifest, vault);
        let (state_now, detail) = match runtime {
            _ if !app.running => (v1::AppState::Stopped, String::new()),
            Some(runtime) if runtime.paused => (v1::AppState::PausedCrashed, runtime.detail.clone()),
            _ if state.host.is_none() && !state.host_problem.is_empty() => (v1::AppState::Unavailable, state.host_problem.clone()),
            _ if done.iter().any(|(_, done)| !done) => (v1::AppState::NeedsSetup, String::new()),
            Some(runtime) if !runtime.failed.is_empty() && runtime.failed == app.version => {
                (v1::AppState::Unavailable, runtime.detail.clone())
            }
            Some(runtime) if runtime.running => (v1::AppState::Running, String::new()),
            Some(runtime) if runtime.retry_at.is_some() => (v1::AppState::Restarting, runtime.detail.clone()),
            _ => (v1::AppState::Starting, String::new()),
        };
        // Who gave each what it has, and when.
        let (sources_given, agents_given) = changes::given(factory, app);
        let mut grants: Vec<v1::AppGrant> = app
            .grants
            .iter()
            .filter(|(agent, _)| config.agent(agent).is_some())
            .map(|(agent, scopes)| v1::AppGrant {
                agent_id: agent.clone(),
                scopes: scopes.clone(),
                given: agents_given.get(agent).cloned(),
            })
            .collect();
        grants.sort_by(|a, b| a.agent_id.cmp(&b.agent_id));
        let subscriptions = app
            .subscriptions
            .iter()
            .map(|(kind, agents)| v1::AppSubscription {
                event_kind: kind.clone(),
                agent_ids: agents.iter().filter(|agent| config.agent(agent).is_some()).cloned().collect(),
            })
            .filter(|subscription| !subscription.agent_ids.is_empty())
            .collect();
        infos.push(v1::AppInfo {
            id: app.id.clone(),
            name: manifest.name.clone(),
            version: app.version.clone(),
            publisher: manifest.publisher.clone(),
            tier: tier(tier_of(&app.key)).into(),
            icon: manifest.icon.clone(),
            state: state_now.into(),
            state_detail: detail,
            scopes: scopes(manifest, &app.key),
            events: event_kinds(manifest),
            hosts: manifest.hosts.clone(),
            grants,
            subscriptions,
            setup: secret_items(manifest, &done),
            status: runtime.map(|runtime| runtime.status.clone()).unwrap_or_default(),
            description: manifest.description.clone(),
            installed_ms: app.installed_ms,
            publisher_key: bundle::short_key(&app.key),
            views: manifest.views.iter().map(|view| v1::AppViewSpec { id: view.id.clone(), title: view.title.clone() }).collect(),
            sources: data::infos(config, manifest, app)
                .into_iter()
                .map(|source| v1::AppDataSource {
                    given: if source.granted { sources_given.get(&source.id).cloned() } else { None },
                    ..source
                })
                .collect(),
            bundled: bundled::is_bundled(factory, &app.id, &app.key),
            reads_files: manifest.data.iter().any(|data| data.kind == bundle::FILE),
        });
    }
    infos.sort_by_key(|info| info.name.to_lowercase());
    infos
}

fn tier_of(key: &str) -> Tier {
    bundle::tier_of(key)
}

// ---- What agents get ----

/// The apps `agent` was given scopes of, as it's told of them: "Telegram
/// (send, read)", by app id.
pub(crate) fn granted(factory: &Factory, config: &Config, agent: &str) -> BTreeMap<String, String> {
    if !factory.apps.available() {
        return BTreeMap::new();
    }
    let state = factory.apps.state();
    config
        .apps
        .iter()
        .filter_map(|app| {
            let scopes = app.grants.get(agent).filter(|scopes| !scopes.is_empty())?;
            let manifest = state.manifests.get(&app.id)?;
            Some((app.id.clone(), format!("{} ({})", manifest.name, scopes.join(", "))))
        })
        .collect()
}

/// The apps' sections of `agent`'s system prompt: each app it was given,
/// with its instructions.
pub(crate) fn sections(factory: &Factory, config: &Config, agent: &str) -> Vec<String> {
    let by_app = sections_by_app(factory, config, agent);
    config.apps.iter().filter_map(|app| by_app.get(&app.id).cloned()).collect()
}

/// The same, by app id.
pub(crate) fn sections_by_app(factory: &Factory, config: &Config, agent: &str) -> BTreeMap<String, String> {
    if !factory.apps.available() {
        return BTreeMap::new();
    }
    let state = factory.apps.state();
    let mut sections = BTreeMap::new();
    for app in &config.apps {
        let (Some(scopes), Some(manifest)) = (app.grants.get(agent).filter(|scopes| !scopes.is_empty()), state.manifests.get(&app.id))
        else {
            continue;
        };
        sections.insert(app.id.clone(), section(manifest, scopes));
    }
    sections
}

/// One app's section: what the agent may do with it, and its instructions.
pub(crate) fn section(manifest: &Manifest, scopes: &[String]) -> String {
    let labels: Vec<String> =
        scopes.iter().filter_map(|scope| manifest.scope(scope)).map(|scope| format!("{} ({})", scope.label, scope.id)).collect();
    let mut text = format!(
        "# The {} app\n\nClaktory runs the {} app for {{everyone}}; the person gave you: {}. Its events reach you as `<event source=\"app\" from=\"{}\">`.",
        manifest.name,
        manifest.name,
        labels.join("; "),
        manifest.name
    )
    .replace(" for {everyone}", "");
    let instructions = manifest.instructions.trim();
    if !instructions.is_empty() {
        text.push_str("\n\n");
        text.push_str(instructions);
    }
    text
}

/// The tools of the apps `agent` was given, as its grants are now.
pub(crate) fn tools(factory: &Arc<Factory>, agent: &str) -> Vec<Tool> {
    if !factory.apps.available() {
        return Vec::new();
    }
    let grants: Vec<(String, Vec<String>)> = {
        let inner = factory.lock();
        inner.config.apps.iter().filter_map(|app| Some((app.id.clone(), app.grants.get(agent)?.clone()))).collect()
    };
    let mut tools = Vec::new();
    for (app, scopes) in grants {
        let Some(manifest) = factory.apps.manifest(&app) else { continue };
        for tool in &manifest.tools {
            if !scopes.iter().any(|scope| manifest.scope(scope).is_some_and(|spec| spec.tools.contains(&tool.name))) {
                continue;
            }
            tools.push(app_tool(factory, agent, &manifest, tool));
        }
    }
    // Giving an app up (#236), for an agent that has any.
    if !tools.is_empty() {
        tools.push(crate::given_up::apps_tool(factory, agent));
    }
    tools
}

/// Whether `name` is an app's tool, for saying why it's gone.
pub(crate) fn tool_app(factory: &Factory, name: &str) -> Option<String> {
    let state = factory.apps.state();
    state.manifests.values().find(|manifest| manifest.tools.iter().any(|tool| tool.name == name)).map(|manifest| manifest.name.clone())
}

fn app_tool(factory: &Arc<Factory>, agent: &str, manifest: &Manifest, spec: &bundle::ToolSpec) -> Tool {
    let weak = Arc::downgrade(factory);
    let (agent, app, name) = (agent.to_owned(), manifest.id.clone(), spec.name.clone());
    let handler: factory_agent::Handler = Arc::new(move |args| {
        let (weak, agent, app, name) = (weak.clone(), agent.clone(), app.clone(), name.clone());
        Box::pin(async move {
            let Some(factory) = weak.upgrade() else { return Err(factory_agent::Error::Other("Claktory is stopping.".into())) };
            call(&factory, &app, &name, &args, &agent).await.map_err(factory_agent::Error::Other)
        })
    });
    let mut tool = Tool::custom(spec.name.clone(), manifest.name.clone(), spec.description.clone(), spec.parameters.clone(), handler);
    tool.spec.constrained_sampling = serde_json::Value::Null;
    tool
}

/// Whether `scope`'s tools hand an agent what the app reads: as the
/// manifest says for Claktory's own apps; always for another publisher's,
/// whose word on it isn't taken.
pub(crate) fn scope_reads(manifest: &Manifest, key: &str, scope: &bundle::ScopeSpec) -> bool {
    !matches!(bundle::tier_of(key), bundle::Tier::Claktory) || manifest.reads_data(scope)
}

/// An agent's call of an app's tool: checked against its grant now, then
/// made in the host.
async fn call(factory: &Arc<Factory>, app: &str, tool: &str, args: &serde_json::Value, agent: &str) -> Result<ToolResult, String> {
    let manifest = factory.apps.manifest(app).ok_or_else(|| format!("The app that had {tool} isn't installed any more."))?;
    let allowed = {
        let inner = factory.lock();
        let installed = inner.config.apps.iter().find(|installed| installed.id == app);
        let granted: Vec<&bundle::ScopeSpec> = installed
            .and_then(|installed| installed.grants.get(agent))
            .map(|scopes| {
                scopes.iter().filter_map(|scope| manifest.scope(scope)).filter(|spec| spec.tools.iter().any(|t| t == tool)).collect()
            })
            .unwrap_or_default();
        !granted.is_empty()
    };
    if !allowed {
        return Err(format!("You no longer have {tool}: the person took that part of the {} app back.", manifest.name));
    }
    let host = {
        let state = factory.apps.state();
        let running = state.apps.get(app).is_some_and(|runtime| runtime.running);
        match (&state.host, running) {
            (Some(host), true) => host.clone(),
            _ => return Err(format!("The {} app isn't running now, so {tool} can't work. Tell the person if it matters.", manifest.name)),
        }
    };
    let asked = wire::Call { app_id: app.to_owned(), tool: tool.to_owned(), args: args.to_string(), caller: agent.to_owned() };
    match host.ask(Body::Call(asked), CALL_DEADLINE).await? {
        Body::CallResult(result) if result.is_error => Err(result.text),
        Body::CallResult(result) => {
            let data = serde_json::from_str(&result.data).unwrap_or_else(|_| serde_json::json!({}));
            Ok(ToolResult::text(result.text, serde_json::json!({"app": app, "data": data})))
        }
        Body::Done(done) => Err(if done.error.is_empty() { format!("The {} app didn't answer.", manifest.name) } else { done.error }),
        _ => Err(format!("The {} app answered something Claktory can't read.", manifest.name)),
    }
}

// ---- Running them ----

impl Factory {
    /// Tests: apps aren't offered, as where they don't ship (`SHIPPED`
    /// off), until `set_apps_host`.
    #[doc(hidden)]
    pub fn set_apps_unavailable_for_test(&self) {
        self.apps.available.store(false, std::sync::atomic::Ordering::Relaxed);
    }

    /// How the apps host starts, instead of its program beside factoryd's
    /// (tests use a stand-in).
    pub fn set_apps_host(&self, launcher: Launcher) {
        self.apps.available.store(true, std::sync::atomic::Ordering::Relaxed);
        *self.apps.launcher.lock().expect("launcher") = Some(launcher);
        {
            let mut state = self.apps.state();
            state.host_retry_at = None;
            state.host_failures = 0;
        }
        self.apps.poke();
    }
}

/// Keeps the host in line with what's wanted, whenever something changes
/// and when a retry is due.
pub(crate) async fn monitor(factory: Weak<Factory>) {
    let Some(wake) = factory.upgrade().map(|factory| factory.apps.wake.clone()) else { return };
    loop {
        let next = {
            let Some(factory) = factory.upgrade() else { return };
            let next = reconcile(&factory).await;
            // Claktory's own apps installed (or updated) by themselves,
            // once the installed ones' manifests are read.
            bundled::keep_installed(&factory).await;
            next
        };
        tokio::select! {
            () = wake.notified() => {}
            () = tokio::time::sleep(next.unwrap_or(Duration::from_secs(3600))) => {}
        }
    }
}

/// One pass: the host up while any app is installed, each loaded at its
/// version, and running exactly when it should. Returns when to look again.
async fn reconcile(factory: &Arc<Factory>) -> Option<Duration> {
    if !factory.apps.available() {
        return None;
    }
    let _busy = factory.apps.busy.lock().await;
    let (installed, vault) = {
        let inner = factory.lock();
        (inner.config.apps.clone(), factory.vault.state(|_, _| false))
    };
    // What's installed, read from disk once.
    let mut changed = false;
    for app in &installed {
        let have = factory.apps.state().manifests.get(&app.id).is_some_and(|manifest| manifest.version == app.version);
        if have {
            continue;
        }
        let read = read_manifest(factory, app);
        let mut state = factory.apps.state();
        match read {
            Ok(manifest) => {
                state.manifests.insert(app.id.clone(), Arc::new(manifest));
            }
            Err(problem) => {
                state.manifests.remove(&app.id);
                state.apps.entry(app.id.clone()).or_default().detail = problem;
            }
        }
        changed = true;
    }
    {
        let mut state = factory.apps.state();
        let gone: Vec<String> = state.manifests.keys().filter(|id| !installed.iter().any(|app| &app.id == *id)).cloned().collect();
        for id in gone {
            state.manifests.remove(&id);
            changed = true;
        }
    }
    if installed.is_empty() {
        let host = {
            let mut state = factory.apps.state();
            state.apps.clear();
            state.host_problem.clear();
            state.host.take()
        };
        if let Some(host) = host {
            host.close();
        }
        if changed {
            factory.apps_changed();
        }
        return None;
    }
    let host = match ensure_host(factory).await {
        Ok(host) => host,
        Err(wait) => {
            factory.apps_changed();
            return Some(wait);
        }
    };
    let now = Instant::now();
    let mut next: Option<Duration> = None;
    for app in &installed {
        let Some(manifest) = factory.apps.manifest(&app.id) else { continue };
        let done = setup_done(app, &manifest, &vault).iter().all(|(_, done)| *done);
        let (loaded, failed, running, paused, retry_at) = {
            let state = factory.apps.state();
            let runtime = state.apps.get(&app.id);
            (
                runtime.map(|runtime| runtime.loaded.clone()).unwrap_or_default(),
                runtime.map(|runtime| runtime.failed.clone()).unwrap_or_default(),
                runtime.is_some_and(|runtime| runtime.running),
                runtime.is_some_and(|runtime| runtime.paused),
                runtime.and_then(|runtime| runtime.retry_at),
            )
        };
        if loaded != app.version {
            if failed == app.version {
                continue;
            }
            let load = wire::Load {
                app_id: app.id.clone(),
                version: app.version.clone(),
                component: folder(factory, &app.id).join(&app.version).join("app.wasm").display().to_string(),
                data_dir: folder(factory, &app.id).join("data").display().to_string(),
                hosts: manifest.hosts.clone(),
                memory_mb: manifest.memory_mb,
                storage_mb: manifest.storage_mb,
                upgrade_from: app.upgrade_from.clone(),
                world: manifest.world.clone(),
            };
            let _ = std::fs::create_dir_all(folder(factory, &app.id).join("data"));
            surfaces::app_not_running(factory, &app.id);
            let answer = done_of(host.ask(Body::Load(load), LIFECYCLE_DEADLINE).await);
            let mut state = factory.apps.state();
            let runtime = state.apps.entry(app.id.clone()).or_default();
            runtime.running = false;
            match answer {
                Ok(()) => {
                    runtime.loaded = app.version.clone();
                    runtime.failed.clear();
                    runtime.detail.clear();
                }
                Err(problem) => {
                    tracing::warn!(app = %app.id, %problem, "the apps host couldn't load an app");
                    runtime.loaded.clear();
                    runtime.failed = app.version.clone();
                    runtime.detail = problem;
                    changed = true;
                    continue;
                }
            }
            changed = true;
        }
        let due = retry_at.is_none_or(|at| at <= now);
        if let Some(at) = retry_at.filter(|at| *at > now) {
            let wait = at - now;
            next = Some(next.map_or(wait, |next| next.min(wait)));
        }
        let want = app.running && done && !paused && due;
        if want && !(running && loaded == app.version) {
            {
                let mut state = factory.apps.state();
                let runtime = state.apps.entry(app.id.clone()).or_default();
                runtime.starting = true;
                runtime.retry_at = None;
            }
            factory.apps_changed();
            let answer = done_of(host.ask(Body::Start(wire::Start { app_id: app.id.clone() }), LIFECYCLE_DEADLINE).await);
            {
                let mut state = factory.apps.state();
                let runtime = state.apps.entry(app.id.clone()).or_default();
                runtime.starting = false;
                runtime.running = answer.is_ok();
            }
            match answer {
                Ok(()) => {
                    tracing::info!(app = %app.id, version = %app.version, "app started");
                    surfaces::app_running(factory, &app.id);
                    if !app.upgrade_from.is_empty() {
                        let id = app.id.clone();
                        let _ = factory
                            .edit_config(|config| {
                                if let Some(app) = config.apps.iter_mut().find(|app| app.id == id) {
                                    app.upgrade_from.clear();
                                }
                                Ok(())
                            })
                            .await;
                    }
                }
                Err(problem) => {
                    if let Some(wait) = crashed(factory, &app.id, &problem) {
                        next = Some(next.map_or(wait, |next| next.min(wait)));
                    }
                }
            }
            changed = true;
        } else if !want && running {
            let answer = done_of(host.ask(Body::Stop(wire::Stop { app_id: app.id.clone() }), LIFECYCLE_DEADLINE).await);
            if let Err(problem) = answer {
                tracing::warn!(app = %app.id, %problem, "the apps host couldn't stop an app");
            }
            factory.apps.state().apps.entry(app.id.clone()).or_default().running = false;
            surfaces::app_not_running(factory, &app.id);
            changed = true;
        }
    }
    // Uninstalled: forgotten by the host too.
    let gone: Vec<String> = {
        let state = factory.apps.state();
        state.apps.keys().filter(|id| !installed.iter().any(|app| &app.id == *id)).cloned().collect()
    };
    for id in gone {
        let _ = host.ask(Body::Unload(wire::Unload { app_id: id.clone() }), LIFECYCLE_DEADLINE).await;
        factory.apps.state().apps.remove(&id);
        changed = true;
    }
    if changed {
        factory.apps_changed();
    }
    next
}

fn folder(factory: &Factory, app: &str) -> std::path::PathBuf {
    factory.home.root().join("apps").join(app)
}

/// A lifecycle answer: Done, empty or with why not.
fn done_of(answer: Result<Body, String>) -> Result<(), String> {
    match answer? {
        Body::Done(done) if done.error.is_empty() => Ok(()),
        Body::Done(done) => Err(done.error),
        _ => Err("Claktory's apps host answered something it can't read.".to_owned()),
    }
}

/// The host as it runs, started if it isn't (and isn't waiting to retry):
/// or how long until it's tried again.
async fn ensure_host(factory: &Arc<Factory>) -> Result<Arc<Host>, Duration> {
    {
        let state = factory.apps.state();
        if let Some(host) = state.host.as_ref().filter(|host| !host.gone.is_cancelled()) {
            return Ok(host.clone());
        }
        if let Some(at) = state.host_retry_at.filter(|at| *at > Instant::now()) {
            return Err(at - Instant::now());
        }
    }
    let launcher = factory.apps.launcher.lock().expect("launcher").clone();
    let launched = match launcher {
        Some(launch) => launch(),
        None => host::launch_process(),
    };
    let started = match launched {
        Ok(link) => {
            let host = Host::connect(link, handler(Arc::downgrade(factory)));
            match host.ask(Body::Hello(wire::Hello { version: crate::update::VERSION.to_owned() }), LIFECYCLE_DEADLINE).await {
                Ok(Body::Ready(ready)) if ready.worlds.iter().any(|world| world == bundle::WORLD) => {
                    tracing::info!(version = %ready.version, "the apps host started");
                    Ok(host)
                }
                Ok(Body::Ready(_)) => {
                    host.close();
                    Err("Update Claktory to run this app.".to_owned())
                }
                Ok(_) => {
                    host.close();
                    Err("Claktory's apps host answered something it can't read.".to_owned())
                }
                Err(problem) => {
                    host.close();
                    Err(problem)
                }
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            Err("This Claktory can't run apps. Update it to the latest version.".to_owned())
        }
        Err(error) => Err(format!("Apps couldn't start: {error}")),
    };
    let mut state = factory.apps.state();
    match started {
        Ok(host) => {
            state.host = Some(host.clone());
            state.host_problem.clear();
            state.host_failures = 0;
            state.host_retry_at = None;
            for runtime in state.apps.values_mut() {
                runtime.loaded.clear();
                runtime.failed.clear();
                runtime.running = false;
            }
            // When it goes, everything it ran goes with it.
            let weak = Arc::downgrade(factory);
            let gone = host.gone.clone();
            let watched = Arc::downgrade(&host);
            tokio::spawn(async move {
                gone.cancelled().await;
                let Some(factory) = weak.upgrade() else { return };
                host_gone(&factory, &watched);
            });
            Ok(host)
        }
        Err(problem) => {
            tracing::warn!(%problem, "the apps host isn't running");
            let wait = BACKOFF[state.host_failures.min(BACKOFF.len() - 1)];
            state.host_failures += 1;
            state.host_retry_at = Some(Instant::now() + wait);
            state.host_problem = problem;
            state.host = None;
            Err(wait)
        }
    }
}

/// The host stopped by itself: it's started again after a wait.
fn host_gone(factory: &Arc<Factory>, which: &Weak<Host>) {
    {
        let mut state = factory.apps.state();
        let current = state.host.as_ref().is_some_and(|host| Weak::ptr_eq(&Arc::downgrade(host), which));
        if !current {
            return;
        }
        state.host = None;
        let wait = BACKOFF[state.host_failures.min(BACKOFF.len() - 1)];
        state.host_failures += 1;
        state.host_retry_at = Some(Instant::now() + wait);
        state.host_problem = "Apps stopped. They'll start again in a moment.".to_owned();
        for runtime in state.apps.values_mut() {
            runtime.loaded.clear();
            runtime.running = false;
        }
    }
    tracing::warn!("the apps host stopped");
    surfaces::app_stopped(factory, None, &|_| "Apps stopped. They'll start again in a moment.".to_owned());
    factory.apps_changed();
    factory.apps.poke();
}

/// An app crashed (or failed to start): it starts again after a wait, or,
/// the third time in ten minutes, it's paused and Rivet hears of it.
/// Returns the wait.
fn crashed(factory: &Arc<Factory>, app: &str, problem: &str) -> Option<Duration> {
    let now = Instant::now();
    let (paused, wait) = {
        let mut state = factory.apps.state();
        let runtime = state.apps.entry(app.to_owned()).or_default();
        runtime.running = false;
        runtime.starting = false;
        runtime.crashes.retain(|at| now.duration_since(*at) < CRASH_WINDOW);
        runtime.crashes.push(now);
        runtime.detail = problem.to_owned();
        if runtime.crashes.len() >= CRASHES_TO_PAUSE {
            runtime.paused = true;
            runtime.retry_at = None;
            (true, None)
        } else {
            let wait = BACKOFF[(runtime.crashes.len() - 1).min(BACKOFF.len() - 1)];
            runtime.retry_at = Some(now + wait);
            (false, Some(wait))
        }
    };
    tracing::warn!(app, problem, paused, "an app crashed");
    let problem_text = problem.to_owned();
    surfaces::app_stopped(factory, Some(app), &|name| {
        if paused {
            format!("The {name} app crashed, so Claktory paused it. {problem_text}")
        } else {
            format!("The {name} app stopped: {problem_text} It starts again in a moment.")
        }
    });
    if paused {
        let name = factory.apps.manifest(app).map_or_else(|| app.to_owned(), |manifest| manifest.name.clone());
        let last = format!("The last error: {problem}");
        let text = format!(
            "The {name} app crashed {CRASHES_TO_PAUSE} times in 10 minutes, so Claktory paused it. {last} \
             It stays off until the person starts it again in the Marketplace's Apps tab."
        );
        let event = host_event(format!("The {name} app was paused after crashing"));
        if factory.lock().config.agent(crate::config::RIVET).is_some() {
            factory.deliver_event(crate::config::RIVET, text, event);
        }
    }
    factory.apps_changed();
    wait
}

/// What the host sends besides answers.
fn handler(factory: Weak<Factory>) -> host::Handler {
    Arc::new(move |host, id, body| {
        let Some(factory) = factory.upgrade() else { return };
        match body {
            Body::GetSecret(asked) => {
                let value = secret(&factory, &asked.app_id, &asked.name);
                let answer = match value {
                    Some(value) => wire::SecretValue { found: true, value: value.to_string() },
                    None => wire::SecretValue { found: false, value: String::new() },
                };
                host.answer(id, Body::SecretValue(answer));
            }
            Body::Emit(emitted) => events::emit(&factory, emitted),
            Body::PushView(pushed) => {
                let error = surfaces::push(&factory, &pushed).err().unwrap_or_default();
                host.answer(id, Body::Done(wire::Done { error }));
            }
            Body::GetDataSources(asked) => host.answer(id, Body::DataSourceList(data::list(&factory, &asked.app_id))),
            Body::DataQuery(asked) => {
                tokio::spawn(async move {
                    let answer = data::query(&factory, asked).await;
                    host.answer(id, Body::DataAnswer(answer));
                });
            }
            Body::SetStatus(status) => {
                let installed = factory.lock().config.apps.iter().any(|app| app.id == status.app_id);
                if installed {
                    let text: String = status.text.trim().chars().take(500).collect();
                    factory.apps.state().apps.entry(status.app_id.clone()).or_default().status = text;
                    factory.apps_changed();
                }
            }
            Body::Crashed(crash) => {
                // A crash while starting is counted from Start's answer.
                let running = factory.apps.state().apps.get(&crash.app_id).is_some_and(|runtime| runtime.running && !runtime.starting);
                if !running {
                    tracing::info!(app = %crash.app_id, error = %crash.error, "an app crashed while starting");
                } else if crashed(&factory, &crash.app_id, &crash.error).is_some() {
                    factory.apps.poke();
                }
            }
            other => tracing::warn!(?other, "the apps host sent something unexpected"),
        }
    })
}

/// One of the app's secrets, as the manifest names it: only if the vault
/// keeps it given to the app.
fn secret(factory: &Factory, app: &str, name: &str) -> Option<zeroize::Zeroizing<String>> {
    let id = {
        let inner = factory.lock();
        inner.config.apps.iter().find(|installed| installed.id == app)?.secrets.get(name)?.clone()
    };
    match factory.vault.value_for(&id, &grantee(app)) {
        Ok(value) => value,
        Err(error) => {
            tracing::warn!(app, %error, "could not read an app's secret");
            None
        }
    }
}

impl Factory {
    /// What clients see of apps changed.
    pub(crate) fn apps_changed(&self) {
        self.broadcast_workspace(&self.lock());
    }
}

// ---- The person's requests ----

impl Factory {
    /// Reads the .clapp asked for; without `approve`, says what it can
    /// reach; with it, installs it (or updates the installed one).
    pub(crate) async fn install_local_app(
        self: &Arc<Self>,
        request: &v1::InstallLocalApp,
    ) -> Result<Option<v1::AppPermissions>, RequestError> {
        let refused = |text: String| Err(RequestError::Apps(text));
        let bundled = request.bundled.trim();
        let bytes = if !bundled.is_empty() {
            // One of Claktory's own, from this release: read and checked
            // again now, and held to Claktory's key, then reviewed and
            // installed like any file.
            let Some(path) = bundled::path(self, bundled) else {
                return refused(format!("Claktory doesn't come with an app called {bundled}."));
            };
            tokio::task::spawn_blocking(move || bundled::read_bytes(&path))
                .await
                .map_err(|_| RequestError::Apps("Claktory couldn't read this app.".to_owned()))?
                .map_err(RequestError::Apps)?
                .1
        } else if request.content.is_empty() {
            let path = request.path.trim();
            if path.is_empty() {
                return refused("Choose an app file (.clapp) to install.".to_owned());
            }
            if !std::path::Path::new(path).is_absolute() {
                return Err(RequestError::RelativePath(path.to_owned()));
            }
            match tokio::fs::metadata(path).await {
                Ok(meta) if meta.len() > bundle::MAX_BYTES as u64 => {
                    return refused(format!("This file is too big for an app (over {} MB).", bundle::MAX_BYTES / 1024 / 1024));
                }
                Ok(_) => {}
                Err(_) => return refused(format!("There's no file at {path}.")),
            }
            tokio::fs::read(path).await.map_err(|error| RequestError::Apps(format!("{path} can't be read: {error}")))?
        } else {
            request.content.clone()
        };
        let bundle = tokio::task::spawn_blocking(move || Bundle::read(&bytes))
            .await
            .map_err(|_| RequestError::Apps("Claktory couldn't read this file.".to_owned()))?
            .map_err(RequestError::Apps)?;
        let installed = self.lock().config.apps.iter().find(|app| app.id == bundle.manifest.id).cloned();
        if let Some(installed) = &installed
            && installed.key != bundle.key
        {
            return refused(format!(
                "This {} comes from a different publisher than the one installed. To install it, uninstall that one first.",
                bundle.manifest.name
            ));
        }
        if !request.approve {
            let from_path = request.content.is_empty() && bundled.is_empty();
            return Ok(Some(self.permissions(&bundle, request.path.trim(), from_path, installed.as_ref())));
        }
        if request.sha256.trim() != bundle.sha256 {
            return refused("This file isn't the one you reviewed. Review it again before installing.".to_owned());
        }
        if installed.as_ref().is_some_and(|installed| installed.sha256 == bundle.sha256) {
            // The same file again: nothing to do.
            return Ok(None);
        }
        let id = bundle.manifest.id.clone();
        let version = bundle.manifest.version.clone();
        let dir = folder(self, &id).join(&version);
        let (manifest_text, component) = (bundle.manifest_text.clone(), bundle.component.clone());
        let written = {
            let dir = dir.clone();
            tokio::task::spawn_blocking(move || -> std::io::Result<()> {
                let staging = dir.with_extension("new");
                let _ = std::fs::remove_dir_all(&staging);
                std::fs::create_dir_all(&staging)?;
                std::fs::write(staging.join("manifest.toml"), manifest_text)?;
                std::fs::write(staging.join("app.wasm"), component)?;
                let _ = std::fs::remove_dir_all(&dir);
                std::fs::rename(&staging, &dir)?;
                std::fs::create_dir_all(dir.parent().expect("app folder").join("data"))
            })
            .await
            .map_err(|_| RequestError::Apps("Couldn't install it.".to_owned()))?
        };
        written.map_err(|error| RequestError::Apps(format!("Couldn't install it: {error}")))?;
        let (key, sha256) = (bundle.key.clone(), bundle.sha256.clone());
        let before = installed.as_ref().map(|installed| installed.version.clone());
        self.edit_config(|config| {
            let now = crate::transcript::now_ms();
            match config.apps.iter_mut().find(|app| app.id == id) {
                Some(app) => {
                    if app.version != version {
                        app.upgrade_from = app.version.clone();
                    }
                    app.version = version.clone();
                    app.sha256 = sha256;
                }
                None => config.apps.push(AppConfig {
                    id: id.clone(),
                    version: version.clone(),
                    running: true,
                    key,
                    sha256,
                    installed_ms: now,
                    grants: BTreeMap::new(),
                    subscriptions: BTreeMap::new(),
                    secrets: BTreeMap::new(),
                    sources: Vec::new(),
                    upgrade_from: String::new(),
                    other: crate::config::Unknown::new(),
                }),
            }
            Ok(())
        })
        .await?;
        {
            let mut state = self.apps.state();
            state.manifests.insert(id.clone(), Arc::new(bundle.manifest.clone()));
            let runtime = state.apps.entry(id.clone()).or_default();
            // The same version again (a developer's new build): loaded afresh.
            runtime.loaded.clear();
            runtime.failed.clear();
            runtime.paused = false;
            runtime.crashes.clear();
            runtime.retry_at = None;
        }
        pins::installed(self, &id, installed.is_none(), !bundle.manifest.views.is_empty());
        // Installed again: one of Claktory's own that the person had
        // uninstalled is theirs to keep again.
        bundled::note_uninstalled(self, &id, &bundle.key, false);
        // An older version's files go once this one is in place.
        if let Some(before) = before.filter(|before| *before != version) {
            let _ = tokio::fs::remove_dir_all(folder(self, &id).join(before)).await;
        }
        tracing::info!(app = %id, %version, "app installed");
        self.apps_changed();
        self.apps.poke();
        Ok(None)
    }

    fn permissions(&self, bundle: &Bundle, path: &str, from_path: bool, installed: Option<&AppConfig>) -> v1::AppPermissions {
        let manifest = &bundle.manifest;
        let mut added = Vec::new();
        let installed_version = installed.map(|installed| installed.version.clone()).unwrap_or_default();
        if let Some(before) = installed.and_then(|installed| self.apps.manifest(&installed.id)) {
            for host in manifest.hosts.iter().filter(|host| !before.hosts.contains(host)) {
                added.push(format!("Reach {host}"));
            }
            for scope in manifest.scopes.iter().filter(|scope| before.scope(&scope.id).is_none()) {
                added.push(format!("Agents you give it to can: {}", lower_first(&scope.label)));
            }
            for event in manifest.events.iter().filter(|event| before.event(&event.kind).is_none()) {
                added.push(format!("Agents you choose hear: {}", lower_first(&event.label)));
            }
            for secret in manifest.secrets.iter().filter(|secret| before.secret(&secret.name).is_none()) {
                added.push(format!("Setting it up asks for: {}", lower_first(&secret.label)));
            }
            if manifest.memory_mb > before.memory_mb {
                added.push(format!("Use up to {} MB of memory", manifest.memory_mb));
            }
            if manifest.storage_mb > before.storage_mb {
                added.push(format!("Store up to {} MB", manifest.storage_mb));
            }
            added.extend(data::added(&before, manifest));
        }
        let sources = data::requested(&self.lock().config, manifest);
        v1::AppPermissions {
            path: if from_path { path.to_owned() } else { String::new() },
            sha256: bundle.sha256.clone(),
            id: manifest.id.clone(),
            name: manifest.name.clone(),
            version: manifest.version.clone(),
            publisher: manifest.publisher.clone(),
            tier: tier(bundle.tier).into(),
            publisher_key: bundle::short_key(&bundle.key),
            description: manifest.description.clone(),
            icon: manifest.icon.clone(),
            hosts: manifest.hosts.clone(),
            scopes: scopes(manifest, &bundle.key),
            events: event_kinds(manifest),
            secrets: secret_items(manifest, &[]),
            memory_mb: manifest.memory_mb,
            storage_mb: manifest.storage_mb,
            installed_version,
            added,
            sources,
        }
    }

    pub(crate) async fn set_app_running(&self, request: &v1::SetAppRunning) -> Result<(), RequestError> {
        let id = request.id.clone();
        self.edit_config(|config| {
            let app = config.apps.iter_mut().find(|app| app.id == id).ok_or_else(|| no_app(&id))?;
            app.running = request.running;
            Ok(())
        })
        .await?;
        if request.running {
            // Started by the person: crashes before don't count.
            let mut state = self.apps.state();
            let runtime = state.apps.entry(id).or_default();
            runtime.paused = false;
            runtime.crashes.clear();
            runtime.retry_at = None;
            runtime.failed.clear();
            state.host_retry_at = None;
        }
        self.apps_changed();
        self.apps.poke();
        Ok(())
    }

    pub(crate) async fn uninstall_app(&self, request: &v1::UninstallApp) -> Result<(), RequestError> {
        let id = request.id.clone();
        let removed = self
            .edit_config(|config| {
                let index = config.apps.iter().position(|app| app.id == id).ok_or_else(|| no_app(&id))?;
                Ok(config.apps.remove(index))
            })
            .await?;
        // One of Claktory's own stays uninstalled: it isn't put back.
        bundled::note_uninstalled(self, &id, &removed.key, true);
        // The host forgets it before its files go.
        surfaces::app_uninstalled(self, &id);
        pins::uninstalled(self, &id);
        self.apps.poke();
        let _busy = self.apps.busy.lock().await;
        let grantee = grantee(&id);
        for secret in removed.secrets.values() {
            // Its secrets stay in the vault, for a later install; only the app's grant goes.
            let _ = self.vault.revoke(secret, &grantee, "");
        }
        self.vault_changed();
        let dir = folder(self, &id);
        let keep = request.keep_data;
        let _ = tokio::task::spawn_blocking(move || {
            let Ok(entries) = std::fs::read_dir(&dir) else { return };
            for entry in entries.flatten() {
                if keep && entry.file_name() == "data" {
                    continue;
                }
                let _ = std::fs::remove_dir_all(entry.path()).or_else(|_| std::fs::remove_file(entry.path()));
            }
            if !keep {
                let _ = std::fs::remove_dir(&dir);
            }
        })
        .await;
        tracing::info!(app = %id, keep_data = keep, "app uninstalled");
        self.apps_changed();
        Ok(())
    }

    /// GrantApp: the person gives the agent these scopes of the app in place
    /// of what it had (none takes it away), kept as a change of theirs.
    pub(crate) async fn grant_app(self: &Arc<Self>, request: &v1::GrantApp) -> Result<(), RequestError> {
        let manifest = self.apps.manifest(&request.app_id).ok_or_else(|| no_app(&request.app_id))?;
        for scope in &request.scopes {
            if manifest.scope(scope).is_none() {
                return Err(RequestError::Apps(format!("The {} app has no scope \"{scope}\".", manifest.name)));
            }
        }
        let held = {
            let inner = self.lock();
            if inner.config.agent(&request.agent_id).is_none() {
                return Err(RequestError::UnknownAgent(request.agent_id.clone()));
            }
            let app = inner.config.apps.iter().find(|app| app.id == request.app_id).ok_or_else(|| no_app(&request.app_id))?;
            changes::held(app, &request.agent_id)
        };
        // It no longer hears what it no longer has the scope for.
        let hears =
            held.hears.into_iter().filter(|kind| manifest.event(kind).is_some_and(|event| request.scopes.contains(&event.scope))).collect();
        let to = changes::Access {
            agents: [(request.agent_id.clone(), changes::Held { scopes: request.scopes.clone(), hears })].into(),
            ..Default::default()
        };
        changes::apply(self, &request.app_id, &to, changes::How::YOU).await?;
        Ok(())
    }

    /// SubscribeApp: the agents that hear the app's events of a kind, in
    /// place of those that did; kept as a change of the person's.
    pub(crate) async fn subscribe_app(self: &Arc<Self>, request: &v1::SubscribeApp) -> Result<(), RequestError> {
        let manifest = self.apps.manifest(&request.app_id).ok_or_else(|| no_app(&request.app_id))?;
        let Some(event) = manifest.event(&request.event_kind) else {
            return Err(RequestError::Apps(format!("The {} app has no event \"{}\".", manifest.name, request.event_kind)));
        };
        let to = {
            let inner = self.lock();
            let config = &inner.config;
            let app = config.apps.iter().find(|app| app.id == request.app_id).ok_or_else(|| no_app(&request.app_id))?;
            let mut agents: Vec<&String> = Vec::new();
            for id in &request.agent_ids {
                let Some(agent) = config.agent(id) else { return Err(RequestError::UnknownAgent(id.clone())) };
                if !app.grants.get(id).is_some_and(|scopes| scopes.contains(&event.scope)) {
                    let scope = manifest.scope(&event.scope).map_or(event.scope.as_str(), |scope| scope.label.as_str());
                    let heard = if event.label.is_empty() { event.kind.clone() } else { lower_first(&event.label) };
                    return Err(RequestError::Apps(format!(
                        "{} can't hear {heard}. Turn on \u{201c}{scope}\u{201d} for {} first.",
                        agent.name, agent.name
                    )));
                }
                if !agents.contains(&id) {
                    agents.push(id);
                }
            }
            let had: Vec<String> = app.subscriptions.get(&request.event_kind).cloned().unwrap_or_default();
            let mut to = changes::Access::default();
            // Each agent that starts or stops hearing it.
            for agent in agents
                .iter()
                .map(|id| id.as_str())
                .filter(|id| !had.iter().any(|old| old == id))
                .chain(had.iter().map(String::as_str).filter(|old| !agents.iter().any(|id| id == old) && config.agent(old).is_some()))
            {
                let mut held = changes::held(app, agent);
                held.hears.retain(|kind| kind != &request.event_kind);
                if agents.iter().any(|id| id.as_str() == agent) {
                    held.hears.push(request.event_kind.clone());
                    held.hears.sort();
                }
                to.agents.insert(agent.to_owned(), held);
            }
            to
        };
        if !to.is_empty() {
            changes::apply(self, &request.app_id, &to, changes::How::YOU).await?;
        }
        Ok(())
    }

    pub(crate) async fn set_app_secret(&self, request: &v1::SetAppSecret) -> Result<(), RequestError> {
        let manifest = self.apps.manifest(&request.app_id).ok_or_else(|| no_app(&request.app_id))?;
        let Some(spec) = manifest.secret(&request.name) else {
            return Err(RequestError::Apps(format!("The {} app doesn't ask for \"{}\".", manifest.name, request.name)));
        };
        let value = request.value.trim();
        if value.is_empty() {
            let what = if spec.label.is_empty() { spec.name.clone() } else { lower_first(&spec.label) };
            return Err(RequestError::Apps(format!("Paste the {what} to save it.")));
        }
        let kept = self.lock().config.apps.iter().find(|app| app.id == request.app_id).and_then(|app| app.secrets.get(&spec.name).cloned());
        let values = [crate::vault::Value {
            variable: spec.name.to_uppercase(),
            value: zeroize::Zeroizing::new(value.to_owned()),
            file: false,
            binary: false,
        }];
        let label = if spec.label.is_empty() { spec.name.clone() } else { lower_first(&spec.label) };
        let description = format!("The {name} app's {label}. Only the {name} app uses it.", name = manifest.name);
        let exists = kept.as_ref().is_some_and(|id| self.vault.state(|_, _| false).secrets.iter().any(|secret| &secret.id == id));
        let id = match kept.filter(|_| exists) {
            Some(id) => {
                // As the person may have renamed or described it.
                let kept = self.vault.state(|_, _| false).secrets.into_iter().find(|secret| secret.id == id);
                let (name, description) = kept.map(|secret| (secret.name, secret.description)).unwrap_or_default();
                self.vault.update_secret(&id, &name, &description, Some(&values))?;
                id
            }
            None => {
                let taken: Vec<String> =
                    self.vault.state(|_, _| false).secrets.into_iter().map(|secret| secret.name.to_lowercase()).collect();
                let base: String = format!("{} {label}", manifest.name)
                    .chars()
                    .filter(|c| c.is_alphanumeric() || matches!(c, ' ' | '-' | '_' | '.' | '@' | '(' | ')'))
                    .take(56)
                    .collect();
                let base = base.trim().to_owned();
                let name = (1..)
                    .map(|n| if n == 1 { base.clone() } else { format!("{base} ({n})") })
                    .find(|name| !taken.contains(&name.to_lowercase()))
                    .unwrap_or(base);
                self.vault.create_secret(&name, &description, &values, "")?
            }
        };
        self.vault.grant(&id, &grantee(&request.app_id), "")?;
        let (app_id, name) = (request.app_id.clone(), spec.name.clone());
        self.edit_config(|config| {
            let app = config.apps.iter_mut().find(|app| app.id == app_id).ok_or_else(|| no_app(&app_id))?;
            app.secrets.insert(name, id);
            Ok(())
        })
        .await?;
        self.vault_changed();
        Ok(())
    }
}

/// A label inside a sentence: "Bot token" → "bot token"; "API key" stays.
pub(super) fn lower_first(label: &str) -> String {
    let mut chars = label.chars();
    match (chars.next(), chars.next()) {
        (Some(first), Some(second)) if first.is_uppercase() && !second.is_uppercase() => {
            first.to_lowercase().chain(label.chars().skip(1)).collect()
        }
        _ => label.to_owned(),
    }
}

fn no_app(id: &str) -> RequestError {
    RequestError::Apps(format!("There's no app \"{id}\" installed."))
}

/// An agent is gone: no app's grant or subscription names it.
pub(crate) fn forget_agent(config: &mut Config, gone: &str) {
    for app in &mut config.apps {
        app.grants.remove(gone);
        for agents in app.subscriptions.values_mut() {
            agents.retain(|agent| agent != gone);
        }
        app.subscriptions.retain(|_, agents| !agents.is_empty());
        data::forget_agent(app, gone);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_apps_settings_from_a_newer_claktory_are_kept() {
        let text =
            r#"{"id":"charts","version":"0.2.0","key":"k","sha256":"s","installed_ms":1,"sources":["factory-records"],"pinned":{"a":1}}"#;
        let app: AppConfig = serde_json::from_str(text).expect("read despite the key it doesn't know");
        assert_eq!(app.sources, ["factory-records"]);
        assert_eq!(app.other["pinned"], serde_json::json!({"a": 1}));
        let written: serde_json::Value = serde_json::to_value(&app).expect("written");
        assert_eq!(written, serde_json::from_str::<serde_json::Value>(text).expect("JSON"));
    }
}
