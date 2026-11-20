//! The Factory: settings, agents and their conversations, shared by every
//! connected client and by the agents themselves.
//!
//! Every broadcast happens while `inner` is locked, and clients subscribe under
//! the same lock (`connect`). So a client's snapshot plus the events after it
//! are exactly the current state, with nothing missed or applied twice.
//!
//! Events are encoded once and the same bytes go to every client.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, Weak};
use std::time::{Duration, SystemTime};

use bytes::Bytes;
use factory_agent::{AbortHandle, DetachHandle, EventSource};
use factory_protocol::v1::{self, server_message::Body};
use prost::Message as _;
use serde::Deserialize;
use tokio::sync::broadcast;

use crate::config::{self, AgentConfig, Config, FolderConfig, Invalid, ProjectConfig, State};
use crate::home::Home;
use crate::login::Flow;
use crate::models::Models;
use crate::transcript::{Change, Transcript, TranscriptError, now_ms};

const WATCH_INTERVAL: Duration = Duration::from_millis(500);
const EVENT_BUFFER: usize = 4096;
/// How long a bash command runs before another agent's message moves it to
/// the background (the person's does at once).
const AGENT_MESSAGE_WAIT: Duration = Duration::from_secs(60);

/// An encoded `ServerMessage` with `reply_to = 0`.
pub type Event = Bytes;

#[derive(Debug, thiserror::Error)]
pub enum OpenError {
    #[error("could not prepare the Claktory home: {0}")]
    Io(#[from] std::io::Error),
    #[error("could not write the default config: {0}")]
    Json(#[from] serde_json::Error),
    #[error("could not open factory.db: {0}")]
    Db(#[from] rusqlite::Error),
}

#[derive(Debug, thiserror::Error)]
pub enum RequestError {
    #[error("no project with id \"{0}\"")]
    UnknownProject(String),
    #[error("no agent with id \"{0}\"")]
    UnknownAgent(String),
    #[error("{0} can't be empty")]
    Empty(&'static str),
    #[error("path must be absolute: {0}")]
    RelativePath(String),
    #[error("config.json has an error; fix it before changing settings here")]
    ConfigBroken,
    #[error(transparent)]
    Invalid(#[from] Invalid),
    #[error("could not save: {0}")]
    Io(#[from] std::io::Error),
    #[error("could not save: {0}")]
    Json(#[from] serde_json::Error),
    #[error("could not save: {0}")]
    Db(#[from] rusqlite::Error),
    #[error(transparent)]
    Transcript(#[from] TranscriptError),
    #[error("no provider with id \"{0}\"")]
    UnknownProvider(String),
    #[error("{provider} has no sign-in method \"{method}\"")]
    UnknownLoginMethod { provider: String, method: String },
    #[error("{provider} has no account \"{account}\"")]
    UnknownAccount { provider: String, account: String },
    #[error("no sign-in is waiting for an answer")]
    NoLoginQuestion,
    #[error(transparent)]
    Provider(#[from] factory_agent::Error),
    #[error("no release has been published")]
    NoRelease,
    #[error("\"{0}\" isn't a folder inside the project")]
    NotInProject(String),
    #[error("can't update the app at {0}: it isn't an app on Claktory's computer, or the release has no such app")]
    NotAnApp(String),
    #[error("no extension with id \"{0}\"")]
    UnknownExtension(String),
    #[error("{0}")]
    Arrange(String),
    #[error("project \"{project}\" has no folder \"{folder}\"")]
    UnknownFolder { project: String, folder: String },
    #[error("folder \"{0}\" isn't one of the agent's project's folders")]
    NotItsFolder(String),
    /// A slash command that can't run now, and why.
    #[error("{0}")]
    Command(String),
    #[error("extension \"{extension}\" has no option \"{option}\"")]
    UnknownOption { extension: String, option: String },
    /// A background task that can't be stopped, and why.
    #[error("{0}")]
    NoTask(String),
    /// A monitor that can't be changed so, and why.
    #[error("{0}")]
    Monitor(String),
    #[error("no paired device \"{0}\"")]
    UnknownDevice(String),
    /// Remote access can't do that now, and why.
    #[error("{0}")]
    Remote(String),
    /// A release's app can't be sent to an app on another machine, and why.
    #[error("can't send the app of {version}: {why}")]
    NoApp { version: String, why: String },
    #[error("no proposal \"{0}\" from this agent")]
    UnknownProposal(String),
    /// A proposal to Rivet that can't be decided so, and why.
    #[error("{0}")]
    Proposal(String),
    /// A build cache that can't be cleared, and why.
    #[error("{0}")]
    BuildCache(String),
    /// A layout that can't be used, and why.
    #[error("{0}")]
    Layout(String),
    #[error(transparent)]
    Vault(#[from] crate::vault::VaultError),
    /// The account service's answer, or why it can't be asked (`crate::account`).
    #[error("{1}")]
    Account(v1::ErrorCode, String),
}

impl RequestError {
    pub fn code(&self) -> v1::ErrorCode {
        match self {
            RequestError::UnknownProject(_)
            | RequestError::UnknownAgent(_)
            | RequestError::UnknownProvider(_)
            | RequestError::UnknownAccount { .. }
            | RequestError::UnknownExtension(_)
            | RequestError::UnknownFolder { .. }
            | RequestError::UnknownOption { .. }
            | RequestError::NoTask(_)
            | RequestError::UnknownDevice(_)
            | RequestError::NoApp { .. }
            | RequestError::UnknownProposal(_) => v1::ErrorCode::NotFound,
            RequestError::Empty(_)
            | RequestError::RelativePath(_)
            | RequestError::ConfigBroken
            | RequestError::Invalid(_)
            | RequestError::UnknownLoginMethod { .. }
            | RequestError::NoLoginQuestion
            | RequestError::NoRelease
            | RequestError::NotInProject(_)
            | RequestError::NotAnApp(_)
            | RequestError::Arrange(_)
            | RequestError::NotItsFolder(_)
            | RequestError::Command(_)
            | RequestError::Monitor(_)
            | RequestError::Remote(_)
            | RequestError::Proposal(_)
            | RequestError::BuildCache(_)
            | RequestError::Layout(_) => v1::ErrorCode::InvalidRequest,
            RequestError::Account(code, _) => *code,
            RequestError::Io(_) | RequestError::Json(_) | RequestError::Db(_) | RequestError::Transcript(_) | RequestError::Provider(_) => {
                v1::ErrorCode::Internal
            }
            RequestError::Vault(error) => {
                use crate::vault::VaultError;
                match error {
                    VaultError::Locked => v1::ErrorCode::VaultLocked,
                    VaultError::ProofRequired => v1::ErrorCode::ProofRequired,
                    VaultError::WrongProof(_) => v1::ErrorCode::WrongProof,
                    VaultError::NotGranted { .. } => v1::ErrorCode::NotGranted,
                    VaultError::NoSecret(_) | VaultError::NoRequest(_) => v1::ErrorCode::NotFound,
                    VaultError::NoVault | VaultError::Exists | VaultError::Invalid(_) => v1::ErrorCode::InvalidRequest,
                    VaultError::Db(_) | VaultError::Io(_) => v1::ErrorCode::Internal,
                }
            }
        }
    }
}

/// Fields of an agent to change; `None` leaves one as it is. An empty
/// `project` moves the agent to the workspace.
#[derive(Default)]
pub struct AgentChange<'a> {
    pub name: Option<&'a str>,
    pub prompt: Option<&'a str>,
    pub model: Option<&'a str>,
    pub effort: Option<&'a str>,
    pub project: Option<&'a str>,
    pub color: Option<&'a str>,
    pub extension: Option<&'a v1::ExtensionChange>,
    /// Display only, like color: it doesn't change the agent's prompt.
    pub placeholder: Option<&'a str>,
    /// The id of the project folder it works in; empty for the project's own.
    pub folder: Option<&'a str>,
    /// Its character's shape (display only); empty for automatic.
    pub avatar_shape: Option<&'a str>,
    /// What it does; empty for none.
    pub role: Option<&'a str>,
}

/// What a client starts from: the state now, and the events that follow it.
pub struct Snapshot {
    pub events: broadcast::Receiver<Event>,
    pub workspace: v1::Workspace,
    pub transcript: Option<v1::Transcript>,
    pub explorer: Option<v1::Explorer>,
    pub view: Option<v1::ExtensionView>,
    pub queued: Option<v1::QueuedMessages>,
}

pub struct Factory {
    pub(crate) home: Home,
    pub(crate) models: Arc<dyn Models>,
    inner: Mutex<Inner>,
    events: broadcast::Sender<Event>,
    pub(crate) store: crate::store::Store,
    /// Serializes writes of config.json, and reloads.
    pub(crate) files: tokio::sync::Mutex<()>,
    /// Serializes listings of the explorer.
    pub(crate) explorer_lock: tokio::sync::Mutex<()>,
    pub(crate) health: crate::update::Health,
    /// The selected agent's queue as last sent to clients.
    queued_sent: Mutex<Option<v1::QueuedMessages>>,
    /// How long an agent sits idle with work outstanding before it's reminded, in ms.
    nudge_after: std::sync::atomic::AtomicU64,
    /// How often background tasks are checked for running long, in ms.
    task_check_every: std::sync::atomic::AtomicU64,
    /// Wakes the usage monitor, so a new sign-in's usage and plan are read at once.
    pub(crate) usage_wake: Arc<tokio::sync::Notify>,
    /// The Monitors extension's scheduler.
    pub(crate) monitors: crate::extensions::monitors::State,
    /// Web search and fetch, shared by every agent (`extensions::web`).
    pub(crate) web: crate::extensions::web::State,
    /// Agents' characters: presence and moods (`crate::characters`).
    pub(crate) characters: crate::characters::State,
    /// Remote access: devices shown and connected (`crate::remote`).
    pub(crate) remote: crate::remote::State,
    /// The machine's resources as last sampled (`extensions::resources`).
    pub(crate) resources: crate::extensions::resources::State,
    /// Build caches found, and cleared ones agents are to hear of
    /// (`extensions::tree::caches`).
    pub(crate) build_caches: crate::extensions::tree::caches::State,
    /// When idle agents compact (`crate::idle`).
    pub(crate) idle: crate::idle::State,
    /// Agents' layouts (`extensions::viewer`).
    pub(crate) viewer: crate::extensions::viewer::State,
    /// The secrets agents use (`crate::vault`).
    pub(crate) vault: crate::vault::Vault,
    /// Hides the vault's values in what agents and the app are shown.
    pub(crate) redaction: factory_agent::Redaction,
    /// This Claktory's account at claktory.com (`crate::account`).
    pub(crate) account: crate::account::State,
}

pub(crate) struct Inner {
    pub(crate) config: Config,
    config_error: Option<String>,
    pub(crate) state: State,
    /// Models agents can run on now.
    pub(crate) models: Vec<v1::Model>,
    pub(crate) providers: Vec<v1::Provider>,
    /// The usage of accounts signed in with a subscription, by provider and account id.
    pub(crate) usage: HashMap<crate::usage::AccountKey, v1::ProviderUsage>,
    /// The plan of accounts signed in with a subscription, by provider and account id.
    pub(crate) plans: HashMap<crate::usage::AccountKey, String>,
    pub(crate) login: Option<Flow>,
    pub(crate) login_generation: u64,
    pub(crate) agents: HashMap<String, Runtime>,
    pub(crate) latest_release: crate::update::Release,
    /// Why the last update was undone; "" when it wasn't.
    pub(crate) update_error: String,
    /// The focused project's explorer as last sent.
    pub(crate) explorer: Option<v1::Explorer>,
    /// The open extension panel's view as last sent.
    pub(crate) view: Option<v1::ExtensionView>,
    /// Background tasks being stopped, by (agent, task): who stopped them
    /// (see `extensions::tasks`).
    pub(crate) stopping: HashMap<(String, String), String>,
}

/// A message waiting for an agent: one the person sent, or an event.
#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(from = "Saved")]
pub(crate) struct Queued {
    /// Its id in the store's messages.
    pub(crate) id: i64,
    pub(crate) text: String,
    pub(crate) event: Option<factory_agent::Event>,
    /// When it was sent, in ms since 1970.
    pub(crate) created: i64,
    /// The person's: the device they sent it from, when known.
    pub(crate) sent_from: Option<factory_agent::SentFrom>,
}

impl Queued {
    pub(crate) fn from_person(text: impl Into<String>, sent_from: Option<factory_agent::SentFrom>) -> Queued {
        Queued { id: 0, text: text.into(), event: None, created: now_ms(), sent_from }
    }

    pub(crate) fn event(text: String, event: factory_agent::Event) -> Queued {
        Queued { id: 0, text, event: Some(event), created: now_ms(), sent_from: None }
    }
}

/// How runs.json kept a queue: plain text before events.
#[derive(Deserialize)]
#[serde(untagged)]
enum Saved {
    Text(String),
    Queued {
        text: String,
        #[serde(default)]
        event: Option<factory_agent::Event>,
    },
}

impl From<Saved> for Queued {
    fn from(saved: Saved) -> Queued {
        match saved {
            Saved::Text(text) => Queued::from_person(text, None),
            Saved::Queued { text, event } => Queued { id: 0, text, event, created: now_ms(), sent_from: None },
        }
    }
}

#[derive(Default)]
pub(crate) struct Runtime {
    pub(crate) transcript: Option<Transcript>,
    pub(crate) queue: Vec<Queued>,
    /// A run task is alive for this agent.
    pub(crate) running: bool,
    /// The person stopped it; it's only winding down.
    pub(crate) stopped: bool,
    /// Cancels the run in progress; taken when the person stops it.
    pub(crate) abort: Option<AbortHandle>,
    /// The run in progress in the store.
    pub(crate) run: Option<i64>,
    /// Messages handed to the agent that its saved conversation doesn't hold
    /// yet, oldest first, with their run. They count as delivered once saved,
    /// so a Factory that stops before then gives them to the agent again.
    pub(crate) taken: std::collections::VecDeque<(Queued, Option<i64>)>,
    /// The tool it's running now.
    pub(crate) doing: Option<String>,
    /// When the tool it's running now started, in ms since 1970; 0 when none.
    pub(crate) doing_since: i64,
    /// Moves the bash command it's running now to the background, so a
    /// message reaches it sooner.
    pub(crate) detach: Option<DetachHandle>,
    /// Which conversation runs belong to: /clear starts a new one, and
    /// whatever a run of the old one still reports is dropped.
    pub(crate) epoch: u64,
    /// Held while the conversation's files are written, so none is written
    /// after /clear moved them away.
    pub(crate) writes: Arc<tokio::sync::Mutex<()>>,
    /// The next run compacts first: the person asked for /compact, or it's
    /// idle with its prompt cache about to expire (`crate::idle`).
    pub(crate) compact: Option<factory_agent::compaction::Reason>,
    /// The run in progress only compacts an idle agent's conversation: new
    /// work for it cancels that, and it ends with no news of a run.
    pub(crate) idle: bool,
    /// The model response an idle compaction was last tried after (its
    /// timestamp), so it's tried once until the agent works again.
    pub(crate) idle_tried: u64,
    /// The run hit an error it didn't recover from (its model call or
    /// compaction failed for good, or its folder was unusable); it ends as
    /// "failed" unless a later prompt in it succeeds. Cleared when it ends.
    pub(crate) failed: bool,
}

impl Runtime {
    /// Messages wait for a busy agent: a bash command it's running moves to
    /// the background (as at its timeout) so the agent reads them sooner: at
    /// once for the person's, once the command has run a minute for another
    /// agent's. Task and monitor events wait for the step to end. Called when
    /// a message comes and when a command starts.
    pub(crate) fn make_way(&self) {
        let (Some(detach), Some("bash")) = (&self.detach, self.doing.as_deref()) else { return };
        let person = self.queue.iter().any(|queued| queued.event.is_none());
        let agent = self.queue.iter().any(|queued| queued.event.as_ref().is_some_and(|event| event.source == EventSource::Agent));
        let wait = if person {
            Duration::ZERO
        } else if agent {
            let ran = Duration::from_millis(now_ms().saturating_sub(self.doing_since).max(0) as u64);
            AGENT_MESSAGE_WAIT.saturating_sub(ran)
        } else {
            return;
        };
        let detach = detach.this_call();
        if wait.is_zero() {
            detach.detach();
            return;
        }
        // Only this command: one that ended meanwhile has nothing to move.
        tokio::spawn(async move {
            tokio::time::sleep(wait).await;
            detach.detach();
        });
    }
}

impl Factory {
    pub async fn open(home: Home, models: Arc<dyn Models>) -> Result<Arc<Factory>, OpenError> {
        tokio::fs::create_dir_all(home.workspace()).await?;
        let mut created = false;
        let (config, config_error) = match tokio::fs::read_to_string(home.config_path()).await {
            Ok(text) => match Config::parse(&text) {
                Ok(mut config) => {
                    // Agents from before colors were saved keep theirs from now
                    // on, and a default prompt nobody edited becomes this
                    // version's.
                    if config.keep_colors(None) | config.follow_default_prompts() {
                        write_atomic(&home.config_path(), config.to_json()?.as_bytes()).await?;
                    }
                    (config, None)
                }
                // Start with defaults but never overwrite the file: it gets fixed or restored, then it reloads.
                Err(error) => {
                    tracing::warn!(%error, "config.json rejected");
                    (Config::default(), Some(error.to_string()))
                }
            },
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                let config = Config::default();
                write_atomic(&home.config_path(), config.to_json()?.as_bytes()).await?;
                created = true;
                (config, None)
            }
            Err(error) => return Err(error.into()),
        };
        let store = crate::store::Store::open(&home.db_path())?;
        crate::runs::import_files(&home, &store).await?;
        let mut state: State = store.state()?;
        if state.selected_agent.as_deref().is_some_and(|id| config.agent(id).is_none()) {
            state.selected_agent = None;
        }
        if state.focused_project.as_deref().is_some_and(|id| config.project(id).is_none()) {
            state.focused_project = None;
        }
        state.unseen.retain(|id| config.agent(id).is_some());
        state.collapsed.retain(|id| config.agent(id).is_some());
        state.open_panels.retain(|id, _| config.agent(id).is_some());
        // A new Factory opens on Rivet, who can show the person around.
        if created && state.selected_agent.is_none() {
            state.selected_agent = Some(config::RIVET.to_owned());
        }
        let inner = Inner {
            config,
            config_error,
            state,
            models: models.list().await,
            providers: models.providers().await,
            usage: HashMap::new(),
            plans: HashMap::new(),
            login: None,
            login_generation: 0,
            latest_release: crate::update::latest(&home).await,
            update_error: crate::update::failure_message(&home),
            explorer: None,
            view: None,
            stopping: HashMap::new(),
            agents: HashMap::new(),
        };
        let (events, _) = broadcast::channel(EVENT_BUFFER);
        let presences = crate::characters::presences(&store.db())?;
        let viewer = crate::extensions::viewer::State::load(&store);
        let remote = crate::remote::State::new(&home);
        let vault = crate::vault::Vault::open_or_make(home.root().join("vault"));
        let account = crate::account::State::new(&home);
        let factory = Arc::new(Factory {
            home,
            models,
            inner: Mutex::new(inner),
            events,
            store,
            files: tokio::sync::Mutex::new(()),
            explorer_lock: tokio::sync::Mutex::new(()),
            health: crate::update::Health::default(),
            queued_sent: Mutex::new(None),
            nudge_after: std::sync::atomic::AtomicU64::new(30_000),
            task_check_every: std::sync::atomic::AtomicU64::new(60_000),
            usage_wake: Arc::default(),
            monitors: crate::extensions::monitors::State::default(),
            web: crate::extensions::web::State::default(),
            characters: crate::characters::State::new(presences),
            remote,
            resources: crate::extensions::resources::State::default(),
            build_caches: crate::extensions::tree::caches::State::default(),
            idle: crate::idle::State::default(),
            viewer,
            redaction: factory_agent::Redaction::new(factory_agent::Redactor::new(&vault.every_variable())),
            vault,
            account,
        });
        // Before runs Factory stopped in are ended, so they don't count as the last.
        factory.add_presences(&factory.lock().config.agents.clone());
        factory.watch_trial();
        factory.refresh_explorer().await;
        {
            let mut inner = factory.lock();
            inner.view = factory.current_view(&inner);
        }
        if let Err(error) = factory.resume() {
            tracing::error!(%error, "could not resume what agents were doing");
        }
        tokio::spawn(watch(Arc::downgrade(&factory)));
        tokio::spawn(crate::usage::monitor(Arc::downgrade(&factory)));
        tokio::spawn(crate::extensions::nudge::monitor(Arc::downgrade(&factory)));
        tokio::spawn(crate::extensions::tasks::monitor(Arc::downgrade(&factory)));
        tokio::spawn(crate::extensions::monitors::monitor(Arc::downgrade(&factory)));
        tokio::spawn(crate::characters::monitor(Arc::downgrade(&factory)));
        tokio::spawn(crate::remote::monitor(Arc::downgrade(&factory)));
        tokio::spawn(crate::extensions::resources::monitor(Arc::downgrade(&factory)));
        tokio::spawn(crate::extensions::tree::caches::monitor(Arc::downgrade(&factory)));
        tokio::spawn(crate::idle::monitor(Arc::downgrade(&factory)));
        tokio::spawn(crate::account::monitor(Arc::downgrade(&factory)));
        Ok(factory)
    }

    /// How long an idle agent with outstanding checklist items waits before
    /// it's reminded (30s); tests make it shorter.
    pub fn set_nudge_after(&self, after: Duration) {
        self.nudge_after.store(u64::try_from(after.as_millis()).unwrap_or(u64::MAX), std::sync::atomic::Ordering::Relaxed);
    }

    pub(crate) fn nudge_after(&self) -> Duration {
        Duration::from_millis(self.nudge_after.load(std::sync::atomic::Ordering::Relaxed))
    }

    /// How often background tasks are checked for running long (a minute);
    /// tests make it shorter.
    pub fn set_task_check_every(&self, every: Duration) {
        self.task_check_every.store(u64::try_from(every.as_millis()).unwrap_or(u64::MAX), std::sync::atomic::Ordering::Relaxed);
    }

    pub(crate) fn task_check_every(&self) -> Duration {
        Duration::from_millis(self.task_check_every.load(std::sync::atomic::Ordering::Relaxed))
    }

    pub(crate) fn lock(&self) -> MutexGuard<'_, Inner> {
        // A panic while holding the lock leaves data that's still consistent
        // enough to keep serving; recover instead of poisoning every client.
        self.inner.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Subscribes to events and returns the state they apply on top of.
    pub async fn connect(&self) -> Snapshot {
        loop {
            let selected = self.lock().state.selected_agent.clone();
            let mut problem = None;
            if let Some(id) = &selected
                && let Err(error) = self.ensure_loaded(id).await
            {
                tracing::error!(agent = %id, %error, "could not load transcript");
                problem = Some(error);
            }
            let inner = self.lock();
            // Selection may have moved while loading; retry until the snapshot is complete.
            if inner.state.selected_agent != selected {
                continue;
            }
            let receiver = self.events.subscribe();
            let transcript = selected.as_deref().map(|id| {
                // An unreadable transcript mustn't keep clients out; show why it's missing.
                transcript_of(&inner, id).unwrap_or_else(|| unreadable(id, problem.as_ref()))
            });
            return Snapshot {
                events: receiver,
                workspace: self.workspace(&inner),
                transcript,
                explorer: inner.explorer.clone(),
                view: inner.view.clone(),
                // A client starts out with nothing queued.
                queued: self.queued_of(&inner).filter(|queued| !queued.messages.is_empty()),
            };
        }
    }

    /// Creates a project. Without a `path`, its folder is named after it inside
    /// the workspace; an existing folder of that name is adopted.
    pub async fn create_project(&self, name: &str, path: &str) -> Result<(), RequestError> {
        let name = name.trim();
        if name.is_empty() {
            return Err(RequestError::Empty("name"));
        }
        let chosen = match path.trim() {
            "" => None,
            path => {
                let path = expand_home(path);
                if !path.is_absolute() {
                    return Err(RequestError::RelativePath(path.display().to_string()));
                }
                Some(path)
            }
        };
        let workspace = self.home.workspace();
        let path = self
            .edit_config(|config| {
                let id = config::unique_id(name, "project", |id| config.project(id).is_some());
                let path = chosen.unwrap_or_else(|| workspace.join(&id));
                config.projects.push(ProjectConfig { id, name: name.to_owned(), path: path.clone(), folders: Vec::new() });
                Ok(path)
            })
            .await?;
        if !tokio::fs::try_exists(&path).await? {
            tokio::fs::create_dir_all(&path).await?;
            // New folders say what the explorer hides, so it's easy to change.
            write_atomic(&path.join(crate::explorer::IGNORE_FILE), crate::explorer::DEFAULT_IGNORE.as_bytes()).await?;
        }
        Ok(())
    }

    pub async fn update_project(&self, id: &str, name: Option<&str>) -> Result<(), RequestError> {
        self.edit_config(|config| {
            let Some(project) = config.projects.iter_mut().find(|project| project.id == id) else {
                return Err(RequestError::UnknownProject(id.to_owned()));
            };
            if let Some(name) = name {
                project.name = name.trim().to_owned();
            }
            Ok(())
        })
        .await
    }

    /// Removes a project and deletes its agents. Its folder stays.
    pub async fn delete_project(&self, id: &str) -> Result<(), RequestError> {
        let removed = self
            .edit_config(|config| {
                if config.project(id).is_none() {
                    return Err(RequestError::UnknownProject(id.to_owned()));
                }
                config.projects.retain(|project| project.id != id);
                let mut removed = Vec::new();
                for agent in &config.agents {
                    if agent.project.as_deref() == Some(id) {
                        removed.push(agent.id.clone());
                    }
                }
                config.agents.retain(|agent| agent.project.as_deref() != Some(id));
                for agent in &removed {
                    crate::extensions::forget_agent(config, agent);
                }
                Ok(removed)
            })
            .await?;
        {
            let mut inner = self.lock();
            inner.state.explorer.remove(id);
            if inner.state.focused_project.as_deref() == Some(id) {
                inner.state.focused_project = None;
            }
        }
        for agent in removed {
            self.delete_agent_data(&agent).await?;
        }
        Ok(())
    }

    /// Creates an agent, in a project or (with an empty `project`) the workspace.
    pub async fn create_agent(&self, request: &v1::CreateAgent) -> Result<(), RequestError> {
        let v1::CreateAgent { project_id: project, name, prompt, model, effort, color, folder_id: folder, role } = request;
        let (project, model, effort, folder) = (project.as_str(), model.as_str(), effort.as_str(), folder.as_str());
        let name = name.trim();
        if name.is_empty() {
            return Err(RequestError::Empty("name"));
        }
        let home = &self.home;
        let id = self
            .edit_config(|config| {
                let project = match project {
                    "" => None,
                    id if config.project(id).is_some() => Some(id.to_owned()),
                    id => return Err(RequestError::UnknownProject(id.to_owned())),
                };
                if !folder.is_empty() && project.as_deref().and_then(|id| config.project(id)?.folder(folder)).is_none() {
                    return Err(RequestError::NotItsFolder(folder.to_owned()));
                }
                let model = if model.is_empty() { config.default_model.clone() } else { model.to_owned() };
                let effort = if effort.is_empty() { config.default_effort.clone() } else { effort.to_owned() };
                // Leftover data under an id would show up as this agent's history.
                let taken = |id: &str| config.agent(id).is_some() || home.agent_dir(id).exists();
                let same_project = |id: &str| config.agent(id).is_some_and(|other| other.project == project);
                let id = config::agent_id(name, project.as_deref(), taken, same_project);
                // Picked now, so the color stays when agents before it are removed.
                let color = if color.is_empty() { config.next_color().to_owned() } else { color.to_owned() };
                let agent = AgentConfig {
                    id: id.clone(),
                    parent: None,
                    project,
                    name: name.to_owned(),
                    role: role.trim().to_owned(),
                    prompt: prompt.trim().to_owned(),
                    model,
                    effort,
                    color,
                    placeholder: String::new(),
                    folder: folder.to_owned(),
                    extensions: Default::default(),
                    avatar: Default::default(),
                };
                config.agents.push(agent);
                Ok(id)
            })
            .await?;
        self.select_agent(&id).await
    }

    /// Deletes an agent and its conversation, stopping it if it's running.
    pub async fn delete_agent(&self, id: &str) -> Result<(), RequestError> {
        self.edit_config(|config| {
            if config.agent(id).is_none() {
                return Err(RequestError::UnknownAgent(id.to_owned()));
            }
            crate::arrange::lift_children(config, id);
            config.agents.retain(|agent| agent.id != id);
            crate::extensions::forget_agent(config, id);
            Ok(())
        })
        .await?;
        self.delete_agent_data(id).await
    }

    async fn delete_agent_data(&self, id: &str) -> Result<(), RequestError> {
        crate::extensions::checklist::forget(&self.store, id)?;
        crate::extensions::memory::forget(&self.store, id)?;
        crate::extensions::monitors::forget(self, id)?;
        crate::proposals::forget(&self.store, id)?;
        crate::extensions::viewer::forget(self, id)?;
        self.forget_presence(id)?;
        match tokio::fs::remove_dir_all(self.home.agent_dir(id)).await {
            Err(error) if error.kind() != std::io::ErrorKind::NotFound => Err(error.into()),
            _ => Ok(()),
        }
    }

    pub async fn update_settings(&self, request: v1::UpdateSettings) -> Result<(), RequestError> {
        let v1::UpdateSettings {
            default_model: model,
            default_effort: effort,
            compact_at_tokens: compact_at,
            ram_percent,
            disk_free_gb,
            disk_free_percent,
            cpu_percent,
            build_cache_gb,
            sidebar_width,
            panel_width,
            user_name,
            vault_unlock_minutes,
            idle_compaction,
            idle_compaction_short_cache,
            cache_ttl,
        } = request;
        let (model, effort) = (model.as_deref(), effort.as_deref());
        let models = self.lock().models.clone();
        self.edit_config(|config| {
            // Empty: whatever suits the (new) default model.
            let effort = match effort {
                Some("") => Some(fitting_effort(&models, model.unwrap_or(&config.default_model))),
                other => other,
            };
            if let Some(model) = model {
                config.default_model = model.to_owned();
            }
            if let Some(effort) = effort {
                config.default_effort = effort.to_owned();
            }
            if let Some(compact_at) = compact_at {
                config.compact_at_tokens = compact_at;
            }
            let resources = &mut config.resources;
            for (value, setting) in [
                (ram_percent, &mut resources.ram_percent),
                (disk_free_gb, &mut resources.disk_free_gb),
                (disk_free_percent, &mut resources.disk_free_percent),
                (cpu_percent, &mut resources.cpu_percent),
            ] {
                if let Some(value) = value {
                    *setting = value;
                }
            }
            if let Some(budget) = build_cache_gb {
                config.resources.build_cache_gb = budget;
            }
            if let Some(name) = user_name {
                config.user.name = name.trim().to_owned();
            }
            if let Some(minutes) = vault_unlock_minutes {
                config.vault.unlock_minutes = minutes;
            }
            if let Some(on) = idle_compaction {
                config.compaction.idle = on;
            }
            if let Some(on) = idle_compaction_short_cache {
                config.compaction.idle_short_cache = on;
            }
            if let Some(v1::CacheTtl { key, ttl }) = cache_ttl {
                let key = key.trim().to_owned();
                match ttl.trim() {
                    "" => {
                        config.prompt_cache.remove(&key);
                    }
                    ttl => {
                        let Some(ttl) = factory_agent::CacheTtl::parse(ttl) else {
                            return Err(RequestError::Invalid(crate::config::Invalid::BadPromptCache(format!(
                                "{key}: ttl \"{ttl}\" isn't 5m or 1h"
                            ))));
                        };
                        config.prompt_cache.insert(key, ttl);
                    }
                }
            }
            for (value, setting) in [(sidebar_width, &mut config.display.sidebar_width), (panel_width, &mut config.display.panel_width)] {
                if let Some(value) = value {
                    *setting = value;
                }
            }
            Ok(())
        })
        .await
    }

    /// Overwrites a broken config.json with the settings in effect, keeping
    /// the broken file as config.broken.json.
    pub async fn restore_settings(&self) -> Result<(), RequestError> {
        let _files = self.files.lock().await;
        let text = self.lock().config.to_json()?;
        let path = self.home.config_path();
        match tokio::fs::copy(&path, path.with_file_name("config.broken.json")).await {
            Err(error) if error.kind() != std::io::ErrorKind::NotFound => return Err(error.into()),
            _ => {}
        }
        write_atomic(&path, text.as_bytes()).await?;
        let mut inner = self.lock();
        inner.config_error = None;
        self.broadcast_workspace(&inner);
        Ok(())
    }

    pub async fn update_agent(&self, id: &str, change: AgentChange<'_>) -> Result<(), RequestError> {
        self.edit_config(|config| {
            if let Some(project) = change.project
                && !project.is_empty()
                && config.project(project).is_none()
            {
                return Err(RequestError::UnknownProject(project.to_owned()));
            }
            let AgentChange { name, prompt, model, effort, project, color, extension, placeholder, folder, avatar_shape, role } = change;
            if let Some(extension) = extension {
                crate::extensions::apply_change(config, id, extension)?;
            }
            let Some(agent) = config.agent_mut(id) else { return Err(RequestError::UnknownAgent(id.to_owned())) };
            if let Some(color) = color {
                agent.color = color.to_owned();
            }
            if let Some(placeholder) = placeholder {
                agent.placeholder = placeholder.trim().to_owned();
            }
            if let Some(role) = role {
                agent.role = role.trim().to_owned();
            }
            if let Some(shape) = avatar_shape {
                agent.avatar.shape = shape.trim().to_owned();
            }
            if let Some(project) = project
                && agent.project.as_deref().unwrap_or_default() != project
            {
                // Its nested agents go with it, to the new project's top level.
                crate::arrange::arrange(config, id, None, None, Some(project), None)?;
            }
            if let Some(folder) = folder {
                crate::arrange::check_folder(config, id, folder)?;
            }
            let Some(agent) = config.agent_mut(id) else { return Err(RequestError::UnknownAgent(id.to_owned())) };
            if let Some(folder) = folder {
                agent.folder = folder.to_owned();
            }
            if let Some(name) = name {
                agent.name = name.trim().to_owned();
            }
            if let Some(prompt) = prompt {
                agent.prompt = prompt.trim().to_owned();
            }
            if let Some(model) = model {
                agent.model = model.to_owned();
            }
            if let Some(effort) = effort {
                agent.effort = effort.to_owned();
            }
            Ok(())
        })
        .await?;
        self.refresh_view(id);
        Ok(())
    }

    pub async fn select_agent(&self, id: &str) -> Result<(), RequestError> {
        if self.lock().config.agent(id).is_none() {
            return Err(RequestError::UnknownAgent(id.to_owned()));
        }
        self.ensure_loaded(id).await?;
        let mut inner = self.lock();
        inner.state.selected_agent = Some(id.to_owned());
        // Opening the agent is seeing what it did.
        inner.state.unseen.remove(id);
        let view = self.current_view(&inner);
        self.set_view(&mut inner, view);
        // Workspace first: clients learn the selection, then receive its transcript.
        self.broadcast_workspace(&inner);
        if let Some(transcript) = transcript_of(&inner, id) {
            self.send(Body::Transcript(transcript));
        }
        self.store.save_state(&inner.state)?;
        Ok(())
    }

    /// The person's message for an agent, sent from the device `sent_from`
    /// names, when known.
    pub fn send_message(self: &Arc<Self>, id: &str, text: &str, sent_from: Option<factory_agent::SentFrom>) -> Result<(), RequestError> {
        let text = text.trim();
        if text.is_empty() {
            return Err(RequestError::Empty("message"));
        }
        let mut inner = self.lock();
        if inner.config.agent(id).is_none() {
            return Err(RequestError::UnknownAgent(id.to_owned()));
        }
        self.enqueue(&mut inner, id, Queued::from_person(text, sent_from))?;
        Ok(())
    }

    /// Gives an agent an event, waking it if it's idle. Gone agents miss it.
    pub(crate) fn deliver_event(self: &Arc<Self>, id: &str, text: String, event: factory_agent::Event) {
        let mut inner = self.lock();
        if inner.config.agent(id).is_none() {
            tracing::info!(agent = %id, from = %event.from, "event for an agent that's gone");
            return;
        }
        let from = event.from.clone();
        if let Err(error) = self.enqueue(&mut inner, id, Queued::event(text, event)) {
            tracing::error!(agent = %id, %from, %error, "could not record an event, so it's lost");
        }
    }

    /// Records a message for an existing agent and queues it: a working
    /// agent reads it after its current step, an idle one starts on it.
    fn enqueue(self: &Arc<Self>, inner: &mut Inner, id: &str, mut queued: Queued) -> Result<(), rusqlite::Error> {
        // No secret's value reaches an agent, or factory.db, in a message:
        // another agent's, a task's output, a monitor's, or one pasted by the person.
        let redactor = self.redaction.current();
        if !redactor.is_empty() {
            queued.text = redactor.redact(&queued.text).into_owned();
            if let Some(event) = queued.event.as_mut() {
                event.summary = redactor.redact(&event.summary).into_owned();
            }
        }
        queued.id = self.store.add_message(id, &queued)?;
        let runtime = inner.agents.entry(id.to_owned()).or_default();
        runtime.queue.push(queued);
        runtime.stopped = false;
        // New work wins over compacting while idle: the conversation stays
        // as it was, and the run goes on with the message.
        if runtime.idle
            && let Some(abort) = &runtime.abort
        {
            abort.abort();
        }
        if !runtime.running {
            runtime.running = true;
            tokio::spawn(self.clone().run(id.to_owned()));
        } else {
            runtime.make_way();
        }
        self.broadcast_workspace(inner);
        Ok(())
    }

    pub fn stop_agent(&self, id: &str) -> Result<(), RequestError> {
        let mut inner = self.lock();
        if inner.config.agent(id).is_none() {
            return Err(RequestError::UnknownAgent(id.to_owned()));
        }
        if let Some(runtime) = inner.agents.get_mut(id) {
            // What's queued stays: it reaches the agent after the stop, as a
            // new run (/clear drops it).
            runtime.stopped = true;
            if let Some(abort) = runtime.abort.take() {
                abort.abort();
            }
            self.end_run(runtime, "stopped");
        }
        self.broadcast_workspace(&inner);
        Ok(())
    }

    pub(crate) async fn ensure_loaded(&self, id: &str) -> Result<(), TranscriptError> {
        if self.lock().agents.get(id).is_some_and(|runtime| runtime.transcript.is_some()) {
            return Ok(());
        }
        let mut transcript = Transcript::load(self.home.agent_dir(id)).await?;
        if let Err(error) = crate::proposals::restore(&self.store, id, &mut transcript) {
            tracing::error!(agent = %id, %error, "could not read proposals; decided ones show as pending");
        }
        match self.vault.requests_of(id) {
            Ok(requests) => {
                for request in requests.into_iter().filter(|request| request.state() != v1::SecretRequestState::Pending) {
                    transcript.set_secret_request(request);
                }
            }
            Err(error) => tracing::error!(agent = %id, %error, "could not read secret requests; decided ones show as pending"),
        }
        let mut inner = self.lock();
        let runtime = inner.agents.entry(id.to_owned()).or_default();
        if runtime.transcript.is_none() {
            runtime.transcript = Some(transcript);
        }
        Ok(())
    }

    /// Changes the config in memory and on disk, or changes nothing.
    pub(crate) async fn edit_config<R>(&self, edit: impl FnOnce(&mut Config) -> Result<R, RequestError>) -> Result<R, RequestError> {
        let _files = self.files.lock().await;
        // Start from the file, not memory: an agent may have edited it since the last poll.
        self.sync_config().await;
        let (result, text) = {
            let mut inner = self.lock();
            if inner.config_error.is_some() {
                return Err(RequestError::ConfigBroken);
            }
            let mut config = inner.config.clone();
            let result = edit(&mut config)?;
            config.keep_colors(Some(&inner.config));
            config.follow_default_prompts();
            config.validate()?;
            let text = config.to_json()?;
            self.apply_config(&mut inner, config);
            self.broadcast_workspace(&inner);
            (result, text)
        };
        write_atomic(&self.home.config_path(), text.as_bytes()).await?;
        Ok(result)
    }

    async fn reload_config(&self) {
        let _files = self.files.lock().await;
        self.sync_config().await;
    }

    /// Applies config.json if it changed. Callers hold `files`.
    async fn sync_config(&self) {
        let text = match tokio::fs::read_to_string(self.home.config_path()).await {
            Ok(text) => text,
            Err(error) => {
                tracing::warn!(%error, "could not read config.json");
                return;
            }
        };
        let mut parsed = Config::parse(&text);
        // An agent written in without a color gets one, and a shipped prompt
        // written in becomes this version's, saved.
        if let Ok(config) = &mut parsed {
            let colorless = config.agents.iter().any(|agent| agent.color.is_empty());
            if colorless {
                let before = self.lock().config.clone();
                config.keep_colors(Some(&before));
            }
            if config.follow_default_prompts() | colorless {
                self.save_filled_in(&text, config).await;
            }
        }
        let mut inner = self.lock();
        match parsed {
            Ok(config) => {
                let had_error = inner.config_error.take().is_some();
                if config == inner.config && !had_error {
                    return;
                }
                tracing::info!("config.json reloaded");
                for unknown in crate::extensions::unknown_settings(&config) {
                    tracing::warn!(%unknown, "config.json: ignoring an extension setting");
                }
                for ignored in crate::arrange::ignored_parents(&config) {
                    tracing::warn!(%ignored, "config.json: ignoring a parent");
                }
                self.apply_config(&mut inner, config);
            }
            Err(error) => {
                let message = error.to_string();
                if inner.config_error.as_deref() == Some(message.as_str()) {
                    return;
                }
                tracing::warn!(%error, "config.json rejected");
                inner.config_error = Some(message);
            }
        }
        self.broadcast_workspace(&inner);
    }

    /// Writes `config` with the colors it was given over `read`, the file it
    /// came from, unless the file has changed since (that's read next).
    /// Writes back a config with what was filled in (colors, prompts), unless
    /// the file changed since `read`.
    async fn save_filled_in(&self, read: &str, config: &Config) {
        let path = self.home.config_path();
        let saved = match config.to_json() {
            Ok(text) if tokio::fs::read_to_string(&path).await.is_ok_and(|now| now == read) => {
                write_atomic(&path, text.as_bytes()).await.map_err(|error| error.to_string())
            }
            Ok(_) => Ok(()),
            Err(error) => Err(error.to_string()),
        };
        if let Err(error) = saved {
            tracing::warn!(%error, "could not save config.json");
        }
    }

    /// Re-reads which providers are signed in and so which models are available.
    pub(crate) async fn refresh_models(&self) {
        let models = self.models.list().await;
        let providers = self.models.providers().await;
        let mut inner = self.lock();
        if models != inner.models || providers != inner.providers {
            // Usage and plan belong to a sign-in; a new one's are read afresh, now.
            for provider in &providers {
                let before = inner.providers.iter().find(|old| old.id == provider.id);
                let signed_in_again = before.is_none_or(|old| old.signed_in_with != provider.signed_in_with);
                let known = before.map(crate::usage::account_ids).unwrap_or_default();
                for account in crate::usage::account_ids(provider) {
                    if signed_in_again || !known.contains(&account) {
                        let key = (provider.id.clone(), account);
                        inner.usage.remove(&key);
                        inner.plans.remove(&key);
                        self.usage_wake.notify_one();
                    }
                }
            }
            // Those of accounts signed out of go with them.
            let listed = |(id, account): &crate::usage::AccountKey| {
                providers.iter().any(|provider| provider.id == *id && crate::usage::account_ids(provider).contains(account))
            };
            inner.usage.retain(|key, _| listed(key));
            inner.plans.retain(|key, _| listed(key));
            inner.models = models;
            inner.providers = providers;
            self.broadcast_workspace(&inner);
        }
    }

    /// Swaps in a valid config, stopping agents it removed.
    fn apply_config(&self, inner: &mut Inner, config: Config) {
        let mut removed = Vec::new();
        for id in inner.agents.keys() {
            if config.agent(id).is_none() {
                removed.push(id.clone());
            }
        }
        for id in removed {
            if let Some(mut runtime) = inner.agents.remove(&id) {
                if let Some(abort) = runtime.abort.take() {
                    abort.abort();
                }
                self.settle(&runtime.queue, crate::store::Outcome::Dropped);
                self.end_run(&mut runtime, "deleted");
            }
        }
        let selected_gone = inner.state.selected_agent.as_deref().is_some_and(|id| config.agent(id).is_none());
        let marks = |state: &crate::config::State| {
            state.unseen.len() + state.collapsed.len() + state.explorer.len() + state.collapsed_folders.len() + state.open_panels.len()
        };
        let before = marks(&inner.state);
        inner.state.unseen.retain(|id| config.agent(id).is_some());
        inner.state.collapsed.retain(|id| config.agent(id).is_some());
        inner.state.open_panels.retain(|id, _| config.agent(id).is_some());
        let folder_exists = |project: &str, folder: &str| config.project(project).is_some_and(|project| project.folder(folder).is_some());
        // Views of folders (and agents' checkouts) that are gone.
        inner.state.explorer.retain(|key, _| crate::explorer::view_exists(&config, key));
        inner.state.collapsed_folders.retain(|(project, folder)| folder_exists(project, folder));
        let pruned = before != marks(&inner.state);
        self.retire_checkouts(&inner.config, &config);
        // A removed agent's grants go with it, so one added later with its id
        // gets none (deleted in the app, or edited out of config.json).
        for agent in inner.config.agents.iter().filter(|agent| config.agent(&agent.id).is_none()) {
            if let Err(error) = self.vault.forget_agent(&agent.id) {
                tracing::error!(%error, agent = %agent.id, "could not take a removed agent's grants");
            }
        }
        self.add_presences(&config.agents);
        inner.config = config;
        if selected_gone {
            // The workspace broadcast that follows tells clients nothing is selected.
            inner.state.selected_agent = None;
        }
        // Otherwise an agent added later with a removed one's id would inherit its marks.
        if (pruned || selected_gone)
            && let Err(error) = self.store.save_state(&inner.state)
        {
            tracing::error!(%error, "could not forget removed agents' sidebar marks");
        }
        // An extension may have been turned on or off for the selected agent.
        let view = self.current_view(inner);
        self.set_view(inner, view);
    }

    /// Removes the own checkouts of agents that were deleted or turned the
    /// "tree" extension off, where nothing would be lost (`trees::retire`);
    /// ones turned on again are tried again if making them failed.
    fn retire_checkouts(&self, before: &Config, after: &Config) {
        let wants = |agent: &AgentConfig| crate::extensions::enabled(agent, crate::extensions::tree::ID);
        for agent in &before.agents {
            let now = after.agent(&agent.id);
            if wants(agent) && now.is_none_or(|now| !wants(now)) {
                let (home, id) = (self.home.clone(), agent.id.clone());
                tokio::task::spawn_blocking(move || crate::trees::retire(&home, &id));
            } else if !wants(agent) && now.is_some_and(wants) {
                crate::trees::retry(&self.home, &agent.id);
            }
        }
    }

    /// The selected agent's open panel's content.
    pub(crate) fn current_view(&self, inner: &Inner) -> Option<v1::ExtensionView> {
        let agent = inner.state.selected_agent.as_deref()?;
        crate::extensions::view(self, inner, agent, open_panel(inner, agent)?)
    }

    /// Sends the open panel's content to clients if it changed.
    pub(crate) fn set_view(&self, inner: &mut Inner, view: Option<v1::ExtensionView>) {
        if view != inner.view {
            if let Some(view) = &view {
                self.send(Body::ExtensionView(view.clone()));
            }
            inner.view = view;
        }
    }

    /// An extension's data for the agent changed: its open panel shows it.
    pub(crate) fn refresh_view(&self, agent: &str) {
        let mut inner = self.lock();
        if inner.state.selected_agent.as_deref() != Some(agent) {
            return;
        }
        let view = self.current_view(&inner);
        self.set_view(&mut inner, view);
    }

    /// Moves an agent (and the agents nested under it) in the sidebar; see `arrange::arrange`.
    pub async fn arrange_agent(
        &self,
        id: &str,
        parent: &str,
        before: &str,
        project: Option<&str>,
        folder: Option<&str>,
    ) -> Result<(), RequestError> {
        let some = |text: &str| (!text.is_empty()).then(|| text.to_owned());
        let (parent, before) = (some(parent), some(before));
        self.edit_config(|config| crate::arrange::arrange(config, id, parent.as_deref(), before.as_deref(), project, folder)).await
    }

    /// Adds a folder to a project. Without a `path`, it's named after it
    /// inside the project's folder. The folder is created if missing.
    pub async fn create_folder(&self, project: &str, name: &str, path: &str) -> Result<(), RequestError> {
        let name = name.trim();
        if name.is_empty() {
            return Err(RequestError::Empty("name"));
        }
        let chosen = folder_path(path)?;
        let path = self
            .edit_config(|config| {
                let Some(project) = config.projects.iter_mut().find(|candidate| candidate.id == project) else {
                    return Err(RequestError::UnknownProject(project.to_owned()));
                };
                let id = config::unique_id(name, "folder", |id| project.folder(id).is_some());
                let path = chosen.unwrap_or_else(|| project.path.join(&id));
                project.folders.push(FolderConfig { id, name: name.to_owned(), path: path.clone() });
                Ok(path)
            })
            .await?;
        tokio::fs::create_dir_all(&path).await?;
        self.refresh_explorer().await;
        Ok(())
    }

    /// Renames a project's folder, or points it at another folder on disk.
    pub async fn update_folder(&self, project: &str, id: &str, name: Option<&str>, path: Option<&str>) -> Result<(), RequestError> {
        let name = name.map(str::trim);
        if name == Some("") {
            return Err(RequestError::Empty("name"));
        }
        let chosen = path.map(folder_path).transpose()?.flatten();
        let path = self
            .edit_config(|config| {
                let folder = folder_mut(config, project, id)?;
                if let Some(name) = name {
                    name.clone_into(&mut folder.name);
                }
                if let Some(path) = chosen {
                    folder.path = path;
                }
                Ok(folder.path.clone())
            })
            .await?;
        tokio::fs::create_dir_all(&path).await?;
        self.refresh_explorer().await;
        Ok(())
    }

    /// Removes a project's folder: its agents go to the project's own folder.
    /// No files are deleted.
    pub async fn delete_folder(&self, project: &str, id: &str) -> Result<(), RequestError> {
        self.edit_config(|config| {
            folder_mut(config, project, id)?;
            if let Some(project) = config.projects.iter_mut().find(|candidate| candidate.id == project) {
                project.folders.retain(|folder| folder.id != id);
            }
            for agent in &mut config.agents {
                if agent.project.as_deref() == Some(project) && agent.folder == id {
                    agent.folder.clear();
                }
            }
            Ok(())
        })
        .await?;
        self.refresh_explorer().await;
        Ok(())
    }

    /// Whether the app shows highlights (see `extensions::highlight`).
    pub async fn set_highlights(&self, on: bool) -> Result<(), RequestError> {
        self.edit_config(|config| {
            config.highlights = on;
            Ok(())
        })
        .await
    }

    /// Stops an agent's running background task for the person; the agent
    /// gets the event that it finished, stopped by the person.
    pub fn stop_task(&self, agent: &str, task: &str) -> Result<(), RequestError> {
        if self.lock().config.agent(agent).is_none() {
            return Err(RequestError::UnknownAgent(agent.to_owned()));
        }
        let person = self.lock().config.person().to_owned();
        crate::extensions::tasks::stop(self, agent, task, &person).map_err(RequestError::NoTask)
    }

    /// Folds a project folder's agents away in the sidebar, or shows them.
    pub fn set_folder_collapsed(&self, project: &str, id: &str, collapsed: bool) -> Result<(), RequestError> {
        let mut inner = self.lock();
        let Some(config) = inner.config.project(project) else { return Err(RequestError::UnknownProject(project.to_owned())) };
        if config.folder(id).is_none() {
            return Err(RequestError::UnknownFolder { project: project.to_owned(), folder: id.to_owned() });
        }
        let key = (project.to_owned(), id.to_owned());
        let changed = if collapsed { inner.state.collapsed_folders.insert(key) } else { inner.state.collapsed_folders.remove(&key) };
        if changed {
            self.broadcast_workspace(&inner);
            self.store.save_state(&inner.state)?;
        }
        Ok(())
    }

    /// Folds an agent's nested agents away in the sidebar, or shows them.
    pub fn set_agent_collapsed(&self, id: &str, collapsed: bool) -> Result<(), RequestError> {
        let mut inner = self.lock();
        if inner.config.agent(id).is_none() {
            return Err(RequestError::UnknownAgent(id.to_owned()));
        }
        let changed = if collapsed { inner.state.collapsed.insert(id.to_owned()) } else { inner.state.collapsed.remove(id) };
        if changed {
            self.broadcast_workspace(&inner);
            self.store.save_state(&inner.state)?;
        }
        Ok(())
    }

    /// Opens an extension's panel beside an agent (empty: the selected one), or closes it.
    pub async fn open_panel(&self, agent: &str, extension: &str) -> Result<(), RequestError> {
        if !extension.is_empty() && crate::extensions::unit(extension).is_none() {
            return Err(RequestError::UnknownExtension(extension.to_owned()));
        }
        let mut inner = self.lock();
        let agent = match agent {
            "" => inner.state.selected_agent.clone().ok_or(RequestError::Empty("agent_id"))?,
            agent => agent.to_owned(),
        };
        if inner.config.agent(&agent).is_none() {
            return Err(RequestError::UnknownAgent(agent));
        }
        if extension.is_empty() {
            inner.state.open_panels.remove(&agent);
        } else {
            inner.state.open_panels.insert(agent, extension.to_owned());
        }
        self.broadcast_workspace(&inner);
        let view = self.current_view(&inner);
        self.set_view(&mut inner, view);
        self.store.save_state(&inner.state)?;
        Ok(())
    }

    /// Sends the workspace, and the selected agent's queue if it changed:
    /// every change to a queue is followed by this, for the sidebar's count.
    pub(crate) fn broadcast_workspace(&self, inner: &Inner) {
        self.send(Body::Workspace(self.workspace(inner)));
        let queued = self.queued_of(inner);
        let mut sent = self.queued_sent.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        if *sent != queued {
            if let Some(queued) = &queued {
                self.send(Body::QueuedMessages(queued.clone()));
            }
            *sent = queued;
        }
    }

    /// What waits for the selected agent, for clients.
    fn queued_of(&self, inner: &Inner) -> Option<v1::QueuedMessages> {
        let id = inner.state.selected_agent.as_deref()?;
        let queue = inner.agents.get(id).map_or(&[][..], |runtime| &runtime.queue[..]);
        let messages = queue
            .iter()
            .map(|queued| {
                let event = queued.event.as_ref();
                v1::QueuedMessage {
                    id: queued.id,
                    source: crate::store::source_of(event).to_owned(),
                    sender: event.map(|event| event.from.clone()).unwrap_or_default(),
                    sender_id: event.map(|event| event.id.clone()).unwrap_or_default(),
                    text: queued.text.clone(),
                    created: queued.created,
                    summary: event.map(|event| event.summary.clone()).unwrap_or_default(),
                    sent_from: queued.sent_from.as_ref().map(crate::transcript::sent_from),
                }
            })
            .collect();
        Some(v1::QueuedMessages { agent_id: id.to_owned(), messages })
    }

    /// Sends a transcript change to clients if the agent is the selected one.
    pub(crate) fn send_change(&self, inner: &Inner, agent_id: &str, change: Change) {
        if inner.state.selected_agent.as_deref() != Some(agent_id) {
            return;
        }
        let agent_id = agent_id.to_owned();
        let body = match change {
            Change::Entry(entry) => Body::Entry(v1::EntryUpdate { agent_id, entry: Some(entry) }),
            Change::Delta { seq, text } => Body::Delta(v1::EntryDelta { agent_id, seq, text }),
        };
        self.send(body);
    }

    pub(crate) fn send(&self, body: Body) {
        if self.events.receiver_count() == 0 {
            return;
        }
        let message = v1::ServerMessage { reply_to: 0, body: Some(body) };
        // An error only means the last client left just now.
        let _ = self.events.send(Bytes::from(message.encode_to_vec()));
    }

    fn workspace(&self, inner: &Inner) -> v1::Workspace {
        let mut projects = Vec::new();
        for project in &inner.config.projects {
            let folders = project
                .folders
                .iter()
                .map(|folder| v1::ProjectFolder {
                    id: folder.id.clone(),
                    name: folder.name.clone(),
                    path: folder.path.display().to_string(),
                    collapsed: inner.state.collapsed_folders.contains(&(project.id.clone(), folder.id.clone())),
                })
                .collect();
            projects.push(v1::Project {
                id: project.id.clone(),
                name: project.name.clone(),
                path: project.path.display().to_string(),
                folders,
            });
        }
        let mut agents = Vec::new();
        let now = crate::transcript::now_ms();
        for agent in &inner.config.agents {
            let runtime = inner.agents.get(&agent.id);
            let character = self.character(inner, agent, now);
            let status = if runtime.is_some_and(|runtime| runtime.running) { v1::AgentStatus::Working } else { v1::AgentStatus::Idle };
            agents.push(v1::Agent {
                id: agent.id.clone(),
                project_id: agent.project.clone().unwrap_or_default(),
                name: agent.name.clone(),
                prompt: agent.prompt.clone(),
                model: agent.model.clone(),
                effort: agent.effort.clone(),
                status: status.into(),
                context_tokens: runtime.and_then(|runtime| runtime.transcript.as_ref()).map_or(0, Transcript::context_tokens),
                compact_at_tokens: inner.config.compact_at_tokens,
                queued_messages: runtime.map_or(0, |runtime| runtime.queue.len() as u32),
                model_available: inner.models.iter().any(|model| model.id == agent.model),
                color: agent.color.clone(),
                unseen: inner.state.unseen.contains(&agent.id),
                extensions: crate::extensions::of_agent(&inner.config, agent),
                parent_id: crate::arrange::parent_of(&inner.config, &agent.id).unwrap_or_default().to_owned(),
                collapsed: inner.state.collapsed.contains(&agent.id),
                commands: crate::commands::of_agent(agent),
                placeholder: agent.placeholder.clone(),
                folder: String::new(),
                folder_id: inner.config.agent_folder(agent).map(|folder| folder.id.clone()).unwrap_or_default(),
                open_panel: open_panel(inner, &agent.id).unwrap_or_default().to_owned(),
                avatar_shape: character.shape,
                // Faces are gone: every agent has the same eyes.
                avatar_face: String::new(),
                character_seed: character.seed,
                character_mood: character.mood.into(),
                character_mood_since_ms: character.since,
                avatar_shape_override: character.shape_override,
                avatar_face_override: None,
                role: agent.role.clone(),
                layout: self.viewer.layout(&agent.id),
                default_prompt: Config::default_prompt(agent).unwrap_or_default().to_owned(),
            });
        }
        let config = &inner.config;
        let vault = self.vault.state(|agent, task| self.store.task_running(agent, task));
        v1::Workspace {
            map: Some(crate::map::build(config, &self.home, &vault)),
            projects,
            agents,
            models: inner.models.clone(),
            version: crate::update::VERSION.to_owned(),
            focused_project_id: inner.state.focused_project.clone().filter(|id| inner.config.project(id).is_some()).unwrap_or_default(),
            latest_release: inner.latest_release.version.clone(),
            latest_backend: inner.latest_release.backend.clone(),
            changelog: inner.latest_release.changelog.iter().map(crate::update::ChangelogVersion::to_proto).collect(),
            remote: Some(self.remote_view(inner)),
            vault: Some(vault),
            account: self.account.view(&config.account),
            update_error: inner.update_error.clone(),
            next_agent_color: inner.config.next_color().to_owned(),
            extensions: crate::extensions::catalog(),
            open_panel: inner.state.selected_agent.as_deref().and_then(|agent| open_panel(inner, agent)).unwrap_or_default().to_owned(),
            highlights: config.highlights,
            selected_agent_id: inner.state.selected_agent.clone().unwrap_or_default(),
            config_error: inner.config_error.clone().unwrap_or_default(),
            providers: inner
                .providers
                .iter()
                .map(|provider| {
                    let signed_in = !provider.signed_in_with.is_empty();
                    let key = |account: &str| (provider.id.clone(), account.to_owned());
                    let first = key(provider.accounts.first().map_or("", |account| account.id.as_str()));
                    let accounts = provider
                        .accounts
                        .iter()
                        .map(|account| v1::Account {
                            usage: inner.usage.get(&key(&account.id)).cloned().or_else(|| account.usage.clone()),
                            plan: inner.plans.get(&key(&account.id)).cloned().unwrap_or_else(|| account.plan.clone()),
                            ..account.clone()
                        })
                        .collect();
                    v1::Provider {
                        // Until the first read, what the sign-in says: empty for a subscription.
                        usage: if signed_in { inner.usage.get(&first).cloned().or_else(|| provider.usage.clone()) } else { None },
                        plan: if signed_in { inner.plans.get(&first).cloned().unwrap_or_default() } else { String::new() },
                        accounts,
                        cache_ttl: config.cache_ttl(&provider.id).map(|ttl| ttl.as_str().to_owned()).unwrap_or_default(),
                        ..provider.clone()
                    }
                })
                .collect(),
            login: inner.login.as_ref().map(|flow| flow.view.clone()),
            settings: Some(v1::Settings {
                default_model: config.default_model.clone(),
                default_effort: config.default_effort.clone(),
                compact_at_tokens: config.compact_at_tokens,
                ram_percent: config.resources.ram_percent,
                disk_free_gb: config.resources.disk_free_gb,
                disk_free_percent: config.resources.disk_free_percent,
                cpu_percent: config.resources.cpu_percent,
                build_cache_gb: config.resources.build_cache_gb,
                sidebar_width: config.display.sidebar_width,
                panel_width: config.display.panel_width,
                user_name: config.user.name.clone(),
                vault_unlock_minutes: config.vault.unlock_minutes,
                idle_compaction: config.compaction.idle,
                idle_compaction_short_cache: config.compaction.idle_short_cache,
                cache_ttls: config
                    .prompt_cache
                    .iter()
                    .map(|(key, ttl)| v1::CacheTtl { key: key.clone(), ttl: ttl.as_str().to_owned() })
                    .collect(),
            }),
        }
    }
}

/// The extension whose panel is open beside the agent, if the agent has it on.
fn open_panel<'a>(inner: &'a Inner, agent: &str) -> Option<&'a str> {
    let extension = inner.state.open_panels.get(agent)?;
    let config = inner.config.agent(agent)?;
    crate::extensions::enabled(config, extension).then_some(extension.as_str())
}

/// A folder path from a request: None when empty, else absolute.
fn folder_path(path: &str) -> Result<Option<PathBuf>, RequestError> {
    match path.trim() {
        "" => Ok(None),
        path => {
            let path = expand_home(path.trim_end_matches('/'));
            if !path.is_absolute() {
                return Err(RequestError::RelativePath(path.display().to_string()));
            }
            Ok(Some(path))
        }
    }
}

fn folder_mut<'a>(config: &'a mut Config, project: &str, id: &str) -> Result<&'a mut FolderConfig, RequestError> {
    let Some(found) = config.projects.iter_mut().find(|candidate| candidate.id == project) else {
        return Err(RequestError::UnknownProject(project.to_owned()));
    };
    found
        .folders
        .iter_mut()
        .find(|folder| folder.id == id)
        .ok_or_else(|| RequestError::UnknownFolder { project: project.to_owned(), folder: id.to_owned() })
}

/// "high" if the model takes it, otherwise its strongest effort.
fn fitting_effort<'a>(models: &'a [v1::Model], model: &str) -> &'a str {
    let Some(model) = models.iter().find(|candidate| candidate.id == model) else { return config::DEFAULT_EFFORT };
    if model.efforts.iter().any(|effort| effort == config::DEFAULT_EFFORT) {
        return config::DEFAULT_EFFORT;
    }
    model.efforts.last().map_or("off", String::as_str)
}

fn unreadable(id: &str, problem: Option<&TranscriptError>) -> v1::Transcript {
    let text = match problem {
        Some(problem) => format!("This conversation couldn't be loaded: {problem}"),
        None => "This conversation couldn't be loaded.".to_owned(),
    };
    let notice = v1::Notice { text, level: v1::NoticeLevel::Error.into() };
    let entry = v1::Entry { seq: 0, timestamp_ms: crate::transcript::now_ms(), kind: Some(v1::entry::Kind::Notice(notice)), reply: 0 };
    v1::Transcript { agent_id: id.to_owned(), entries: vec![entry] }
}

pub(crate) fn transcript_of(inner: &Inner, id: &str) -> Option<v1::Transcript> {
    let transcript = inner.agents.get(id)?.transcript.as_ref()?;
    Some(v1::Transcript { agent_id: id.to_owned(), entries: transcript.entries().to_vec() })
}

/// Picks up edits to config.json, new logins in auth.json and newly published
/// releases, whoever made them.
async fn watch(factory: Weak<Factory>) {
    let (config_path, auth_path, release_path) = match factory.upgrade() {
        Some(factory) => (factory.home.config_path(), factory.home.auth_path(), factory.home.releases().join("latest")),
        None => return,
    };
    let mut config_stamp = stamp(&config_path).await;
    let mut auth_stamp = stamp(&auth_path).await;
    let mut release_stamp = stamp(&release_path).await;
    // An edit since Factory read the files, before the stamps above, counts
    // too: a sign-in or a release as Factory started is noticed, not missed
    // until the next.
    match factory.upgrade() {
        Some(factory) => {
            factory.reload_config().await;
            factory.refresh_models().await;
            factory.refresh_release().await;
        }
        None => return,
    }
    loop {
        tokio::time::sleep(WATCH_INTERVAL).await;
        let Some(factory) = factory.upgrade() else { return };
        let config_now = stamp(&config_path).await;
        if config_now != config_stamp {
            config_stamp = config_now;
            factory.reload_config().await;
        }
        let auth_now = stamp(&auth_path).await;
        if auth_now != auth_stamp {
            auth_stamp = auth_now;
            factory.refresh_models().await;
        }
        factory.refresh_explorer().await;
        let release_now = stamp(&release_path).await;
        if release_now != release_stamp {
            release_stamp = release_now;
            factory.refresh_release().await;
        }
    }
}

async fn stamp(path: &Path) -> Option<(SystemTime, u64)> {
    let metadata = tokio::fs::metadata(path).await.ok()?;
    Some((metadata.modified().ok()?, metadata.len()))
}

fn expand_home(path: &str) -> PathBuf {
    if let Some(rest) = path.strip_prefix("~/")
        && let Some(home) = std::env::home_dir()
    {
        return home.join(rest);
    }
    PathBuf::from(path)
}

/// Readers see the old file or the new one, never a mix.
pub(crate) async fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".tmp");
    tokio::fs::write(&tmp, bytes).await?;
    tokio::fs::rename(&tmp, path).await
}

impl Factory {
    /// The vault changed: what's hidden follows its values, and clients see
    /// it as it is now.
    /// Opens the vault with the key the backend before this one handed over
    /// at an update (see `vault::handoff`).
    pub fn take_vault_key(&self, handed: crate::vault::handoff::Handed) {
        if self.vault.take(handed) {
            tracing::info!("the vault stays open across the update");
            self.vault_changed();
        } else {
            tracing::warn!("a key handed over at the update doesn't open the vault; it opens as after a restart");
        }
    }

    pub(crate) fn vault_changed(&self) {
        self.redaction.set(factory_agent::Redactor::new(&self.vault.every_variable()));
        self.broadcast_workspace(&self.lock());
    }

    /// Makes new vault passphrases cheap to check; for tests.
    pub fn set_vault_passphrase_cost(&self, params: &'static str) {
        self.vault.set_passphrase_params(params);
    }
}
