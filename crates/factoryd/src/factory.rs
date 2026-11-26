//! The Factory: settings, agents and their conversations, shared by every
//! connected client and by the agents themselves.
//!
//! Every broadcast happens while `inner` is locked, and clients subscribe under
//! the same lock (`connect`). So a client's snapshot plus the events after it
//! are exactly the current state, with nothing missed or applied twice.
//!
//! Events are encoded once and the same bytes go to every client.

use std::collections::{BTreeMap, HashMap};
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

/// How often the explorer's folders are listed while an app is connected,
/// and the watched files read while they can't be watched.
const WATCH_INTERVAL: Duration = Duration::from_millis(500);
/// How often the watched files are read anyway, in case a change went
/// unreported.
const WATCH_SAFETY: Duration = Duration::from_secs(30);
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
    #[error(
        "Claktory can't update the app in {0}, because your account on this Mac isn't allowed to change that folder. Update from an administrator's account, or move Claktory to the Applications folder in your home folder."
    )]
    AppNotWritable(String),
    #[error("no extension with id \"{0}\"")]
    UnknownExtension(String),
    #[error("{0}")]
    Arrange(String),
    #[error("{0}")]
    Lead(String),
    #[error("project \"{project}\" has no folder \"{folder}\"")]
    UnknownFolder { project: String, folder: String },
    #[error("folder \"{0}\" isn't one of the agent's project's folders")]
    NotItsFolder(String),
    /// A slash command that can't run now, and why.
    #[error("{0}")]
    Command(String),
    /// A conversation can't go back there (`crate::fork`).
    #[error("{0}")]
    Fork(String),
    /// No such other version of the conversation (`crate::fork`).
    #[error("This version of the conversation is gone.")]
    NoBranch(String),
    #[error("extension \"{extension}\" has no option \"{option}\"")]
    UnknownOption { extension: String, option: String },
    /// A background task that can't be stopped, and why.
    #[error("{0}")]
    NoTask(String),
    /// A monitor that can't be changed so, and why.
    #[error("{0}")]
    Monitor(String),
    /// A monitor the agent doesn't have.
    #[error("{0}")]
    NoMonitor(String),
    /// Access an agent can't give up (`crate::given_up`), and why.
    #[error("{0}")]
    GiveUp(String),
    #[error("no paired device \"{0}\"")]
    UnknownDevice(String),
    /// The code of an agent's pairing (ShowPairingCode) that no longer
    /// waits for a device, and why.
    #[error("{0}")]
    NoPairingCode(String),
    /// Remote access can't do that now, and why.
    #[error("{0}")]
    Remote(String),
    /// A release's app can't be sent to an app on another machine, and why.
    #[error("can't send the app of {version}: {why}")]
    NoApp { version: String, why: String },
    /// A question from an agent that can't be answered so, and why.
    #[error("{0}")]
    Ask(String),
    #[error("no proposal \"{0}\" from this agent")]
    UnknownProposal(String),
    /// A proposal to Rivet that can't be decided so, and why.
    #[error("{0}")]
    Proposal(String),
    /// A build cache that can't be cleared, and why.
    #[error("{0}")]
    BuildCache(String),
    /// Installing or setting up an app (`crate::apps`), and why not.
    #[error("{0}")]
    Apps(String),
    #[error("before_seq {0} is beyond the end of the transcript")]
    NoEntries(u64),
    /// A layout that can't be used, and why.
    #[error("{0}")]
    Layout(String),
    #[error("no comment thread \"{0}\"")]
    UnknownThread(String),
    #[error("no comment \"{0}\"")]
    UnknownComment(String),
    /// A comment that can't be made or changed so, and why.
    #[error("{0}")]
    Comments(String),
    /// A checklist goal that can't be added or changed so, and why.
    #[error("{0}")]
    Checklist(String),
    /// A usage breakdown that can't be made so, and why.
    #[error("{0}")]
    Usage(String),
    #[error(transparent)]
    Vault(#[from] crate::vault::VaultError),
    /// The account service's answer, or why it can't be asked (`crate::account`).
    #[error("{1}")]
    Account(v1::ErrorCode, String),
    /// A file that can't be uploaded or attached, and why (`crate::uploads`).
    #[error("{0}")]
    Upload(String),
    /// A service that can't be connected, or a connection that can't do
    /// what was asked, and why (`crate::connectors`).
    #[error("{0}")]
    Connect(String),
    /// A service that couldn't be reached, or answered with an error.
    #[error("{0}")]
    Unreachable(String),
    /// A usage reset that isn't there (any more), and why (`crate::usage`).
    #[error("{0}")]
    NoReset(String),
    /// A usage reset that can't be used now, and why (`crate::usage`).
    #[error("{0}")]
    Reset(String),
    #[error("no pipe \"{0}\"")]
    NoPipe(String),
    /// A pipe group that isn't there (any more): deleted, maybe from
    /// another device (`crate::pipes::groups`).
    #[error("That group was deleted.")]
    NoPipeGroup,
    /// A pipe that can't be made or changed so, and why (`crate::pipes`).
    #[error("{0}")]
    Pipe(String),
    /// An avatar pair that can't be picked, and why.
    #[error("{0}")]
    Avatar(String),
    /// Ink, a sketch or a reading place that can't be saved so, and why
    /// (`crate::ink`, `crate::sketch`, `crate::presentation`).
    #[error("{0}")]
    Ink(String),
    #[error("{what} is too long: {most} characters at most.")]
    TooLong { what: &'static str, most: usize },
    /// A search that can't be made so, and why (`crate::find`).
    #[error("{0}")]
    Find(String),
    /// A usage wait that can't be ended so, and why (`crate::waits`).
    #[error("{0}")]
    Wait(String),
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
            | RequestError::NoPairingCode(_)
            | RequestError::NoApp { .. }
            | RequestError::UnknownProposal(_)
            | RequestError::UnknownThread(_)
            | RequestError::UnknownComment(_)
            | RequestError::NoReset(_)
            | RequestError::NoPipe(_)
            | RequestError::NoPipeGroup
            | RequestError::NoMonitor(_)
            | RequestError::NoBranch(_) => v1::ErrorCode::NotFound,
            RequestError::Empty(_)
            | RequestError::RelativePath(_)
            | RequestError::ConfigBroken
            | RequestError::Invalid(_)
            | RequestError::UnknownLoginMethod { .. }
            | RequestError::NoLoginQuestion
            | RequestError::NoRelease
            | RequestError::NotInProject(_)
            | RequestError::NotAnApp(_)
            | RequestError::AppNotWritable(_)
            | RequestError::Arrange(_)
            | RequestError::Lead(_)
            | RequestError::NotItsFolder(_)
            | RequestError::Command(_)
            | RequestError::Fork(_)
            | RequestError::Monitor(_)
            | RequestError::GiveUp(_)
            | RequestError::Remote(_)
            | RequestError::Proposal(_)
            | RequestError::Ask(_)
            | RequestError::BuildCache(_)
            | RequestError::Apps(_)
            | RequestError::Layout(_)
            | RequestError::NoEntries(_)
            | RequestError::Upload(_)
            | RequestError::Connect(_)
            | RequestError::Comments(_)
            | RequestError::Checklist(_)
            | RequestError::Usage(_)
            | RequestError::Reset(_)
            | RequestError::Pipe(_)
            | RequestError::Avatar(_)
            | RequestError::Ink(_)
            | RequestError::TooLong { .. }
            | RequestError::Find(_)
            | RequestError::Wait(_) => v1::ErrorCode::InvalidRequest,
            RequestError::Unreachable(_) => v1::ErrorCode::Unreachable,
            RequestError::Account(code, _) => *code,
            RequestError::Io(_) | RequestError::Json(_) | RequestError::Db(_) | RequestError::Transcript(_) | RequestError::Provider(_) => {
                v1::ErrorCode::Internal
            }
            RequestError::Vault(error) => {
                use crate::vault::VaultError;
                match error {
                    VaultError::Locked => v1::ErrorCode::VaultLocked,
                    VaultError::ProofRequired => v1::ErrorCode::ProofRequired,
                    VaultError::WrongProof(_) | VaultError::WrongGroup(_) => v1::ErrorCode::WrongProof,
                    VaultError::TooManyTries(_) => v1::ErrorCode::RateLimited,
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
    /// The agent it reports to, by id; empty for none.
    pub lead: Option<&'a str>,
    /// The model a refused step is retried on; empty for the Claktory-wide
    /// default.
    pub fallback_model: Option<&'a str>,
    /// Whether it notifies the person (`crate::notify`).
    pub notify: Option<v1::NotifyMode>,
    /// Who or what changed its model, and why, as the agent's "Your model is
    /// now …" says it (friend bug #43): "Matt changed it on Matt's Mac".
    /// None says only the model.
    pub why: Option<&'a str>,
}

/// What a client starts from: the state now, and the events that follow it.
pub struct Snapshot {
    pub events: broadcast::Receiver<Event>,
    pub workspace: v1::Workspace,
    pub transcript: Option<v1::Transcript>,
    /// The revision `transcript` brings the app to (#413); empty when it
    /// isn't the conversation (one that can't be read).
    pub transcript_revision: String,
    /// The explorers of the projects whose files are open, the focused
    /// project's last.
    pub explorers: Vec<v1::Explorer>,
    pub view: Option<v1::ExtensionView>,
    pub queued: Option<v1::QueuedMessages>,
    /// The chat shown, if one: its conversation and queue.
    pub chat: Option<(v1::ChatTranscript, v1::ChatQueuedMessages)>,
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
    /// The projects connections show the files of (`crate::explorer`).
    pub(crate) file_watches: crate::explorer::Watches,
    pub(crate) health: crate::update::Health,
    /// While this backend is an update on trial, conversations' new
    /// segments, and ones still JSON, are saved as JSON: the version it
    /// would roll back to may not read JSON Lines (`transcript::log`).
    pub(crate) old_segments: Arc<std::sync::atomic::AtomicBool>,
    /// The release channel and Check for updates (`crate::update`).
    pub(crate) updates: crate::update::Updates,
    /// The selected agent's queue as last sent to clients.
    queued_sent: Mutex<Option<v1::QueuedMessages>>,
    /// How long an agent sits idle with work outstanding before it's reminded, in ms.
    nudge_after: std::sync::atomic::AtomicU64,
    /// How long a minute of the Checklist's `nudge_every` lasts, in ms.
    nudge_minute: std::sync::atomic::AtomicU64,
    /// How often background tasks are checked for running long, in ms.
    task_check_every: std::sync::atomic::AtomicU64,
    /// Wakes the usage monitor, so a new sign-in's usage and plan are read at once.
    pub(crate) usage_wake: Arc<tokio::sync::Notify>,
    /// Counts up when what apps look at may need a loop that waits while
    /// nothing does (#454): an app connected, the open panel or the panes
    /// changed, or a test changed an interval (`wake_loops`).
    loops_wake: tokio::sync::watch::Sender<u64>,
    /// Counts up at each change to the Workspace (`broadcast_workspace`):
    /// what wakes the loops that look for work only after something
    /// happened, such as the Checklist's reminders.
    activity: tokio::sync::watch::Sender<u64>,
    /// Providers whose accounts' usage and plan are read again at the
    /// monitor's next look, whenever they were last read: signed in to
    /// again, or Try now (`crate::usage`).
    pub(crate) usage_reread: Mutex<std::collections::HashSet<String>>,
    /// When each key's credit was last tried (`crate::credit`).
    pub(crate) credit_tried: Mutex<HashMap<crate::usage::AccountKey, i64>>,
    /// Wakes agents waiting out their accounts' usage limits (`crate::waits`)
    /// when the accounts or the settings change.
    pub(crate) capacity: Arc<tokio::sync::Notify>,
    /// Tests: whether the network is up for waits on it (`crate::waits`),
    /// in place of looking: 0 looks, 1 up, 2 down.
    pub(crate) network_says: std::sync::atomic::AtomicU8,
    /// How long a provider can't be reached while the network works before
    /// a run waiting on it gives up, in ms (`crate::waits`).
    pub(crate) network_give_up_ms: std::sync::atomic::AtomicI64,
    /// How long a provider's answers can keep failing before the run gives
    /// up (`crate::waits`, #490); tests make it shorter.
    pub(crate) provider_give_up_ms: std::sync::atomic::AtomicI64,
    /// The least a wait for such a provider lasts, doubling with each in a
    /// row (`crate::waits`); tests make it shorter.
    pub(crate) provider_wait_ms: std::sync::atomic::AtomicU64,
    /// The accounts a usage reset is being used on now (`crate::usage`).
    pub(crate) resetting: Mutex<std::collections::HashSet<crate::usage::AccountKey>>,
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
    /// When idle agents' conversations leave memory (`crate::unload`).
    pub(crate) unload: crate::unload::State,
    /// Files the person is uploading for agents (`crate::uploads`).
    pub(crate) uploads: crate::uploads::Uploads,
    /// Connecting services (`crate::connectors`).
    pub(crate) broker: crate::connectors::Broker,
    /// Agents' layouts (`extensions::viewer`).
    pub(crate) viewer: crate::extensions::viewer::State,
    /// Comments on files: the files in panes as last checked (`crate::annotations`).
    pub(crate) annotations: crate::annotations::State,
    /// The secrets agents use (`crate::vault`).
    pub(crate) vault: crate::vault::Vault,
    /// Told whenever the vault changes, so the sign-ins in it are taken up
    /// when it opens (`vault::signins::monitor`).
    pub(crate) vault_moved: Arc<tokio::sync::Notify>,
    /// This Claktory's direct-message leaf: its sealed store, one writer at
    /// a time (`user_apps::dms::leaf`).
    pub(crate) dm_leaf: crate::user_apps::dms::leaf::State,
    /// This Claktory as the person's direct-message mailbox (`user_apps::dms::home`).
    pub(crate) dm_home: crate::user_apps::dms::home::State,
    /// What wakes its direct-message loops (`user_apps::dms::Wakes`).
    pub(crate) dm_wakes: crate::user_apps::dms::Wakes,
    /// Direct messages between home Claktories (#451, `claktory-dm/1`).
    pub(crate) dm_link: crate::dm_link::State,
    /// The person's devices' sends to their home (#451 H3, `crate::dm_device`).
    pub(crate) dm_device: crate::dm_device::State,
    /// Hellos from the person's other Claktories (#451 H2, `crate::dm_hello`).
    pub(crate) dm_hello: crate::dm_hello::State,
    /// The apps installed, as they run (`crate::apps`).
    pub(crate) apps: crate::apps::Apps,
    /// Websites pipes' peers share, as this side listens for them
    /// (`pipes::share_listener`).
    pub(crate) share_listeners: crate::pipes::share_listener::Listeners,
    /// Hides the vault's values in what agents and the app are shown.
    pub(crate) redaction: factory_agent::Redaction,
    /// This Claktory's color until the person picks one (`remote::key_color`).
    pub(crate) key_color: &'static str,
    /// The seed of the person's avatar until they shuffle it (`crate::person`).
    pub(crate) key_seed: String,
    /// This Claktory's account at claktory.com (`crate::account`).
    pub(crate) account: crate::account::State,
    /// A new Claktory's first run (`crate::onboarding`).
    pub(crate) onboarding: crate::onboarding::State,
    /// The speech model voice messages need (`crate::speech`).
    pub(crate) speech: crate::speech::model::Model,
    /// Moving this Claktory to another computer (`crate::move_run`).
    pub(crate) moving: crate::move_run::Moving,
    /// Pipes to other Claktories (`crate::pipes`).
    pub(crate) pipes: crate::pipes::Pipes,
    /// Websites shared over pipes: the streams this side serves, and their
    /// rates (`crate::pipes::shares`).
    pub(crate) pipe_shares: crate::pipes::shares::State,
    /// The Map last recorded for History (#458).
    pub(crate) map_history: crate::history::record::Recorder,
    /// What History's overview counted of the events, kept between
    /// overviews (`crate::history::Kept`).
    pub(crate) history: crate::history::Kept,
    /// Transcribes voice messages (`crate::speech::voice`).
    pub(crate) voices: crate::speech::voice::Voices,
    /// The person's chats (`crate::chats`).
    pub(crate) chats: crate::chats::Chats,
    /// Agents and chats that continued on another provider while theirs
    /// was at its usage limit (`crate::usage_switch`).
    pub(crate) usage_switches: crate::usage_switch::Switches,
    /// What each device's app was last sent, to resume from (#378).
    pub(crate) resumes: Arc<crate::patch::Resumes>,
    /// The revisions of the conversation shown, to resume it on reconnect
    /// (#413).
    pub(crate) transcript_log: crate::transcript::resume::Log,
    /// Keeping the Mac awake while agents work (P1).
    pub(crate) keep_awake: crate::keep_awake::KeepAwake,
    /// Commands Execute cards are running (`crate::command_cards`).
    pub(crate) command_runs: crate::command_cards::Runs,
    /// Errands in Claktory's browser (`crate::errands`).
    pub(crate) errands: crate::errands::Errands,
    /// Whether its background service runs it, on a Mac (friend bug #24).
    pub(crate) service: crate::service::Service,
    /// How each provider's model list was last read (friend bug #14).
    pub(crate) catalog: crate::catalog::Catalog,
    /// Notifications: who's using Claktory where, and what was seen
    /// (`crate::notify`).
    pub(crate) notify: crate::notify::State,
    /// Browser control's sessions and the connections brokering them
    /// (`crate::browser`).
    pub(crate) browser: crate::browser::Browser,
    /// Claktory's browser for errands (#435, `crate::browser_host`).
    pub(crate) agent_browser: crate::browser_host::Host,
    /// Each device's connections, waiting for where it was reading
    /// (`crate::presentation`).
    pub(crate) presentations: crate::presentation::Hub,
    /// The devices in Needs you's focus mode (#514).
    pub(crate) focus_mode: crate::focus_mode::FocusMode,
    /// The conversations connections watch without selecting them (#514).
    pub(crate) watches: crate::watch::Hub,
    /// The backend's own diagnostic events, on their way to factory.db
    /// (`crate::diagnostics`).
    pub(crate) diagnostics: crate::diagnostics::Recorder,
    /// Notified when a project is unarchived: its agents read what waited.
    unarchived: tokio::sync::Notify,
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
    /// What API keys have to spend, as their providers say (`crate::credit`).
    pub(crate) credit: HashMap<crate::usage::AccountKey, v1::AccountCredit>,
    /// The plan of accounts signed in with a subscription, by provider and account id.
    pub(crate) plans: HashMap<crate::usage::AccountKey, String>,
    pub(crate) login: Option<Flow>,
    pub(crate) login_generation: u64,
    pub(crate) agents: HashMap<String, Runtime>,
    pub(crate) latest_release: crate::update::Release,
    /// Why the last update was undone; "" when it wasn't.
    pub(crate) update_error: String,
    /// What the last Check for updates found (Workspace.update_check).
    pub(crate) update_check: v1::UpdateCheck,
    /// The explorers of the projects whose files are open, by project id,
    /// as last sent.
    pub(crate) explorers: BTreeMap<String, v1::Explorer>,
    /// The open extension panel's view as last sent.
    pub(crate) view: Option<v1::ExtensionView>,
    /// Background tasks being stopped, by (agent, task): who stopped them
    /// (see `extensions::tasks`).
    pub(crate) stopping: HashMap<(String, String), String>,
    /// Who or what last changed each agent's model, by agent id: the model
    /// it changed to and why (`AgentChange::why`), for its "Your model is
    /// now …" (friend bug #43). In memory only: after a restart the note
    /// says only the model.
    pub(crate) model_why: HashMap<String, (String, String)>,
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
    /// The person's: the id their app made up for this send; empty without one.
    pub(crate) client_id: String,
    /// The person's: the files they attached.
    pub(crate) attachments: Vec<factory_agent::Attachment>,
    /// The person's voice message (`crate::speech::voice`): its
    /// transcription; `text` is empty until it's done.
    pub(crate) voice: Option<crate::speech::voice::Voice>,
    /// The person's reply on a checklist item (`extensions::checklist::reply`).
    pub(crate) checklist_item: Option<factory_agent::ChecklistItemRef>,
    /// An agent's message: what Claktory records of its send, with it.
    #[serde(skip)]
    pub(crate) sent: Option<crate::extensions::agents::provenance::Sent>,
    /// Quiet, past a batch's cap: it waits for the agent's next wake, or
    /// for agents_quiet (`crate::extensions::agents::quiet`).
    #[serde(skip)]
    pub(crate) held: bool,
}

/// A lasting event from Claktory, with no sender id.
pub(crate) fn host_event(summary: String) -> factory_agent::Event {
    factory_agent::Event {
        source: factory_agent::EventSource::Host,
        from: "Claktory".to_owned(),
        id: String::new(),
        summary,
        transient: false,
        pipe_seqs: Vec::new(),
        provenance: None,
    }
}

impl Queued {
    pub(crate) fn from_person(text: impl Into<String>, sent_from: Option<factory_agent::SentFrom>) -> Queued {
        Queued {
            id: 0,
            text: text.into(),
            event: None,
            created: now_ms(),
            sent_from,
            client_id: String::new(),
            attachments: Vec::new(),
            voice: None,
            checklist_item: None,
            sent: None,
            held: false,
        }
    }

    /// Sent quietly: it never wakes the agent (`crate::extensions::agents::quiet`).
    pub(crate) fn quiet(&self) -> bool {
        self.event.as_ref().and_then(|event| event.provenance.as_ref()).is_some_and(|provenance| provenance.quiet)
    }

    pub(crate) fn event(text: String, event: factory_agent::Event) -> Queued {
        Queued { event: Some(event), ..Queued::from_person(text, None) }
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
            Saved::Queued { text, event } => Queued { event, ..Queued::from_person(text, None) },
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
    /// Its conversation is being taken back or switched (`crate::fork`):
    /// no run takes what's queued until that's done.
    pub(crate) forking: bool,
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
    /// What it waits for while none of its model's accounts can take a
    /// request (`crate::waits`); None when it doesn't.
    pub(crate) usage_wait: Option<crate::waits::UsageWait>,
    /// The reset time its conversation last said it waits for, so a wait
    /// that starts again for the same one isn't said twice.
    pub(crate) wait_said: Option<Option<u64>>,
    /// The provider whose answer its run waits after (its Waiting event):
    /// its model may move to another before the wait starts.
    pub(crate) asked_on: String,
    /// Since when it has waited for the vault to open, to use sign-ins kept
    /// in it (`crate::waits`), in ms since 1970; 0 when it doesn't.
    pub(crate) locked_since: i64,
    /// Since when its model calls can't connect while the network is up
    /// (`crate::waits`, #364 O2), in ms since 1970 (0 while it's down), and
    /// how many times it has waited for the network since its last answer;
    /// 0 once a call gets an answer.
    pub(crate) network_since: i64,
    pub(crate) network_waits: u32,
    /// Since when its provider's answers keep failing in a way that passes
    /// (an overload, a 5xx; `crate::waits`, #490), in ms since 1970, and
    /// how many times it has waited them out since its last answer; 0 once
    /// a call gets an answer.
    pub(crate) provider_since: i64,
    pub(crate) provider_waits: u32,
    /// When its conversation was last needed: read, or a run of it ended,
    /// in ms since 1970. An idle one is unloaded a while after (`crate::unload`).
    pub(crate) used_ms: i64,
    /// Its context size when its conversation was unloaded, shown until
    /// it's read again.
    pub(crate) held_tokens: u64,
}

impl Runtime {
    /// Something queued can go to the agent now (a voice message being
    /// transcribed, and the person's after it, can't: `crate::speech::voice`).
    /// Quiet messages never count: they wait for something else to wake it.
    pub(crate) fn has_ready(&self) -> bool {
        self.queue.iter().zip(crate::speech::voice::deliverable(&self.queue)).any(|(queued, ready)| ready && !queued.quiet())
    }

    /// Takes what's queued that can go to the agent now, oldest first, but
    /// quiet messages (`crate::extensions::agents::quiet::take` gives those);
    /// the rest stays.
    pub(crate) fn take_ready(&mut self) -> Vec<Queued> {
        let ready = crate::speech::voice::deliverable(&self.queue);
        let (taken, kept) =
            std::mem::take(&mut self.queue).into_iter().zip(ready).partition::<Vec<_>, _>(|(queued, ready)| *ready && !queued.quiet());
        self.queue = kept.into_iter().map(|(queued, _)| queued).collect();
        taken.into_iter().map(|(queued, _)| queued).collect()
    }

    /// How many quiet messages wait for it.
    pub(crate) fn quiet_waiting(&self) -> usize {
        self.queue.iter().filter(|queued| queued.quiet()).count()
    }

    /// Messages wait for a busy agent: a bash command it's running moves to
    /// the background (as at its timeout) so the agent reads them sooner: at
    /// once for the person's, once the command has run a minute for another
    /// agent's. Task and monitor events wait for the step to end. Called when
    /// a message comes and when a command starts.
    pub(crate) fn make_way(&self) {
        let (Some(detach), Some("bash")) = (&self.detach, self.doing.as_deref()) else { return };
        let ready: Vec<&Queued> = self
            .queue
            .iter()
            .zip(crate::speech::voice::deliverable(&self.queue))
            .filter(|(queued, ready)| *ready && !queued.quiet())
            .map(|(queued, _)| queued)
            .collect();
        let person = ready.iter().any(|queued| queued.event.is_none());
        let agent = ready.iter().any(|queued| queued.event.as_ref().is_some_and(|event| event.source == EventSource::Agent));
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
    /// Where this Factory keeps its files.
    pub fn home(&self) -> &Home {
        &self.home
    }

    pub async fn open(home: Home, models: Arc<dyn Models>) -> Result<Arc<Factory>, OpenError> {
        tokio::fs::create_dir_all(home.workspace()).await?;
        // Only this user may open the home, and the loopback port wants the
        // local pairing code's token from the first client on.
        let local = home.clone();
        match tokio::task::spawn_blocking(move || crate::remote::prepare_local(&local)).await {
            Ok(Ok(())) => {}
            Ok(Err(error)) => tracing::error!(%error, "could not write this machine's pairing code"),
            Err(error) => tracing::error!(%error, "could not write this machine's pairing code"),
        }
        let mut created = false;
        let (config, config_error) = match tokio::fs::read_to_string(home.config_path()).await {
            Ok(text) => match Config::parse(&text, home.root()) {
                Ok(mut config) => {
                    // After going back from an update, most likely: say so at once.
                    for unknown in config.unknown_keys() {
                        tracing::warn!(%unknown, "config.json: keeping a setting this version doesn't know");
                    }
                    // Agents from before colors were saved keep theirs from now
                    // on, and a default prompt nobody edited becomes this
                    // version's.
                    if config.keep_colors(None) | config.follow_default_prompts() {
                        // Like every write, in the file's form: a rollback
                        // must find a file the older backend still reads.
                        write_atomic(&home.config_path(), config.to_json_keeping_form(home.root(), &text)?.as_bytes()).await?;
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
                write_atomic(&home.config_path(), config.to_json(home.root())?.as_bytes()).await?;
                created = true;
                (config, None)
            }
            Err(error) => return Err(error.into()),
        };
        let store = crate::store::Store::open(&home.db_path())?;
        crate::runs::import_files(&home, &store).await?;
        let chats = crate::chats::Chats::load(&store);
        let usage_switches = crate::usage_switch::Switches::load(&store);
        // Only the real Claktory (~/.factory) keeps a closed lid from
        // sleeping the Mac: Claktory Dev's clearing would undo its hold.
        let real = home.root().file_name().is_some_and(|name| name == ".factory");
        let keep_awake = crate::keep_awake::KeepAwake::load(&store, crate::keep_awake::Machine::this(), real);
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
        // The providers' models as last fetched, and those added by their
        // id, before they're first listed.
        let catalog = crate::catalog::load(&home, &store, models.as_ref());
        let inner = Inner {
            config,
            config_error,
            state,
            models: models.list().await,
            providers: models.providers().await,
            usage: HashMap::new(),
            credit: HashMap::new(),
            plans: HashMap::new(),
            login: None,
            login_generation: 0,
            latest_release: crate::update::latest(&home).await,
            update_error: crate::update::failure_message(&home),
            update_check: v1::UpdateCheck::default(),
            explorers: BTreeMap::new(),
            view: None,
            stopping: HashMap::new(),
            model_why: HashMap::new(),
            agents: HashMap::new(),
        };
        let (events, _) = broadcast::channel(EVENT_BUFFER);
        let presences = crate::characters::presences(&store.db())?;
        let viewer = crate::extensions::viewer::State::load(&store);
        let web = crate::extensions::web::State::new(&home);
        let remote = crate::remote::State::new(&home);
        let key_color = crate::remote::key_color(&home);
        let key_seed = crate::remote::key_seed(&home);
        let vault = crate::vault::Vault::open_or_make(home.root().join("vault"));
        let account = crate::account::State::new(&home, inner.config.account.enabled);
        let home_for_speech = home.clone();
        let onboarding = crate::onboarding::State::load(&store, created, &inner.models, crate::service::runs_here(&home))?;
        let service = crate::service::Service::load(&store);
        let old_segments = Arc::new(std::sync::atomic::AtomicBool::new(crate::update::on_trial(&home)));
        let factory = Arc::new(Factory {
            old_segments,
            home,
            models,
            inner: Mutex::new(inner),
            events,
            store,
            files: tokio::sync::Mutex::new(()),
            explorer_lock: tokio::sync::Mutex::new(()),
            file_watches: crate::explorer::Watches::default(),
            health: crate::update::Health::default(),
            updates: crate::update::Updates::default(),
            queued_sent: Mutex::new(None),
            nudge_after: std::sync::atomic::AtomicU64::new(30_000),
            nudge_minute: std::sync::atomic::AtomicU64::new(60_000),
            task_check_every: std::sync::atomic::AtomicU64::new(60_000),
            usage_wake: Arc::default(),
            loops_wake: tokio::sync::watch::channel(0).0,
            activity: tokio::sync::watch::channel(0).0,
            usage_reread: Mutex::default(),
            credit_tried: Mutex::default(),
            capacity: Arc::default(),
            network_says: std::sync::atomic::AtomicU8::new(0),
            network_give_up_ms: std::sync::atomic::AtomicI64::new(crate::waits::NETWORK_GIVE_UP_MS),
            provider_give_up_ms: std::sync::atomic::AtomicI64::new(crate::waits::PROVIDER_GIVE_UP_MS),
            provider_wait_ms: std::sync::atomic::AtomicU64::new(crate::waits::PROVIDER_LEAST_MS),
            resetting: Mutex::default(),
            monitors: crate::extensions::monitors::State::default(),
            web,
            characters: crate::characters::State::new(presences),
            remote,
            resources: crate::extensions::resources::State::default(),
            build_caches: crate::extensions::tree::caches::State::default(),
            idle: crate::idle::State::default(),
            unload: crate::unload::State::default(),
            uploads: crate::uploads::Uploads::default(),
            broker: crate::connectors::Broker::default(),
            viewer,
            annotations: crate::annotations::State::default(),
            redaction: factory_agent::Redaction::new(factory_agent::Redactor::new(&vault.every_variable())),
            vault,
            vault_moved: Arc::default(),
            dm_leaf: crate::user_apps::dms::leaf::State::default(),
            dm_home: crate::user_apps::dms::home::State::default(),
            dm_wakes: crate::user_apps::dms::Wakes::default(),
            dm_link: crate::dm_link::State::default(),
            dm_device: crate::dm_device::State::default(),
            dm_hello: crate::dm_hello::State::default(),
            apps: crate::apps::Apps::default(),
            share_listeners: crate::pipes::share_listener::Listeners::default(),
            account,
            onboarding,
            speech: crate::speech::model::Model::new(&home_for_speech),
            moving: crate::move_run::Moving::new(&home_for_speech),
            pipes: crate::pipes::Pipes::default(),
            pipe_shares: crate::pipes::shares::State::default(),
            map_history: crate::history::record::Recorder::default(),
            history: crate::history::Kept::default(),
            voices: crate::speech::voice::Voices::new(),
            chats,
            usage_switches,
            resumes: Arc::default(),
            transcript_log: crate::transcript::resume::Log::default(),
            keep_awake,
            command_runs: crate::command_cards::Runs::default(),
            errands: crate::errands::Errands::default(),
            service,
            catalog,
            notify: crate::notify::State::default(),
            browser: crate::browser::Browser::default(),
            agent_browser: crate::browser_host::Host::default(),
            presentations: crate::presentation::Hub::default(),
            focus_mode: crate::focus_mode::FocusMode::default(),
            watches: crate::watch::Hub::default(),
            diagnostics: crate::diagnostics::Recorder::default(),
            unarchived: tokio::sync::Notify::new(),
            key_color,
            key_seed,
        });
        // The sign-ins are kept in the vault (rivet-move.md §3, P2).
        let sign_ins = factory
            .models
            .auth_path()
            .map(|auth| Arc::new(crate::vault::signins::SignIns::new(Arc::downgrade(&factory), &factory.home, auth)));
        if let Some(sign_ins) = &sign_ins {
            factory.models.keep_sign_ins_with(sign_ins.clone());
            // The models were read from `auth.json`; once the sign-ins moved
            // into the vault, they're there.
            factory.refresh_models().await;
        }
        // Before runs Factory stopped in are ended, so they don't count as the last.
        factory.add_presences(&factory.lock().config.agents.clone());
        crate::pipes::start(&factory);
        // History: the Map from before recording began, reconstructed once.
        crate::history::backfill::spawn(&factory.home);
        // And its overview's counts, ahead of the first time it opens.
        crate::history::spawn_warm(&factory);
        factory.watch_trial();
        factory.refresh_explorer().await;
        {
            let mut inner = factory.lock();
            inner.view = factory.current_view(&inner);
        }
        if let Err(error) = factory.resume() {
            tracing::error!(%error, "could not resume what agents were doing");
        }
        // Commands a card ran when Claktory last stopped: stopped, never run again.
        crate::command_cards::recover(&factory);
        crate::errands::recover(&factory);
        // Errands drive Claktory's browser, once it's on here (#435).
        if crate::browser_host::enabled() {
            crate::errands::install(&factory, Arc::new(crate::browser_host::Handle::new(&factory)));
        }
        tokio::spawn(watch(Arc::downgrade(&factory)));
        // factoryd.log, rotated past 50 MB (#525).
        tokio::spawn(crate::log_file::watch(factory.home.root().join("factoryd.log")));
        tokio::spawn(explore(Arc::downgrade(&factory)));
        tokio::spawn(crate::usage::monitor(Arc::downgrade(&factory)));
        tokio::spawn(crate::catalog::monitor(Arc::downgrade(&factory)));
        tokio::spawn(crate::credit::monitor(Arc::downgrade(&factory)));
        tokio::spawn(crate::keep_awake::monitor(Arc::downgrade(&factory)));
        crate::service::check(&factory);
        tokio::spawn(crate::extensions::nudge::monitor(Arc::downgrade(&factory)));
        tokio::spawn(crate::extensions::swarm::monitor(Arc::downgrade(&factory)));
        tokio::spawn(crate::extensions::tasks::monitor(Arc::downgrade(&factory)));
        tokio::spawn(crate::extensions::monitors::monitor(Arc::downgrade(&factory)));
        tokio::spawn(crate::characters::monitor(Arc::downgrade(&factory)));
        tokio::spawn(crate::remote::monitor(Arc::downgrade(&factory)));
        tokio::spawn(crate::extensions::devices::monitor(Arc::downgrade(&factory)));
        tokio::spawn(crate::onboarding::monitor(Arc::downgrade(&factory)));
        tokio::spawn(crate::extensions::resources::monitor(Arc::downgrade(&factory)));
        tokio::spawn(crate::extensions::tree::caches::monitor(Arc::downgrade(&factory)));
        tokio::spawn(crate::idle::monitor(Arc::downgrade(&factory)));
        tokio::spawn(crate::vault::requests::monitor(Arc::downgrade(&factory)));
        tokio::spawn(crate::user_apps::dms::watch::monitor(Arc::downgrade(&factory)));
        if let Some(sign_ins) = sign_ins {
            tokio::spawn(crate::vault::signins::monitor(Arc::downgrade(&factory), sign_ins));
        }
        tokio::spawn(crate::account::monitor(Arc::downgrade(&factory)));
        tokio::spawn(crate::annotations::monitor(Arc::downgrade(&factory)));
        // Review cards follow their file (Bruno 46189).
        tokio::spawn(crate::review_watch::monitor(Arc::downgrade(&factory)));
        tokio::spawn(crate::apps::monitor(Arc::downgrade(&factory)));
        tokio::spawn(crate::speech::model::check(Arc::downgrade(&factory)));
        tokio::spawn(crate::speech::voice::work(Arc::downgrade(&factory)));
        tokio::spawn(crate::notify::monitor(Arc::downgrade(&factory)));
        tokio::spawn(crate::disk::monitor(Arc::downgrade(&factory)));
        tokio::spawn(crate::browser::monitor(Arc::downgrade(&factory)));
        tokio::spawn(crate::pipes::spend::monitor(Arc::downgrade(&factory)));
        tokio::spawn(crate::calls::monitor(Arc::downgrade(&factory)));
        // The agent the app opens on, read before an app asks for it.
        if let Some(selected) = factory.lock().state.selected_agent.clone() {
            let factory = Arc::downgrade(&factory);
            tokio::spawn(async move {
                if let Some(factory) = factory.upgrade()
                    && let Err(error) = factory.ensure_loaded(&selected).await
                {
                    tracing::warn!(agent = %selected, %error, "could not read the selected agent's transcript");
                }
            });
        }
        tokio::spawn(resume_unarchived(Arc::downgrade(&factory)));
        tokio::spawn(load_waiting(Arc::downgrade(&factory)));
        Ok(factory)
    }

    /// Where speech recognition comes from: the `claktory-speech` program
    /// (None: not installed) and the model it downloads. Tests and mirrors
    /// change it; the rest uses Claktory's own.
    pub fn set_speech_source(self: &Arc<Self>, program: Option<PathBuf>, artifact: crate::speech::Artifact) {
        self.speech.set_source(program, artifact);
        self.broadcast_workspace(&self.lock());
        tokio::spawn(crate::speech::model::check(Arc::downgrade(self)));
    }

    /// How long an idle agent with outstanding checklist items waits before
    /// it's reminded (30s); tests make it shorter.
    pub fn set_nudge_after(&self, after: Duration) {
        self.nudge_after.store(u64::try_from(after.as_millis()).unwrap_or(u64::MAX), std::sync::atomic::Ordering::Relaxed);
        self.wake_loops();
    }

    pub(crate) fn nudge_after(&self) -> Duration {
        Duration::from_millis(self.nudge_after.load(std::sync::atomic::Ordering::Relaxed))
    }

    /// How long a minute of the Checklist's periodic check-in
    /// (`nudge_every`) lasts; tests make it shorter.
    pub fn set_nudge_minute(&self, minute: Duration) {
        self.nudge_minute.store(u64::try_from(minute.as_millis()).unwrap_or(u64::MAX), std::sync::atomic::Ordering::Relaxed);
        self.wake_loops();
    }

    pub(crate) fn nudge_minute(&self) -> i64 {
        i64::try_from(self.nudge_minute.load(std::sync::atomic::Ordering::Relaxed)).unwrap_or(i64::MAX)
    }

    /// How often background tasks are checked for running long (a minute);
    /// tests make it shorter.
    pub fn set_task_check_every(&self, every: Duration) {
        self.task_check_every.store(u64::try_from(every.as_millis()).unwrap_or(u64::MAX), std::sync::atomic::Ordering::Relaxed);
        self.wake_loops();
    }

    pub(crate) fn task_check_every(&self) -> Duration {
        Duration::from_millis(self.task_check_every.load(std::sync::atomic::Ordering::Relaxed))
    }

    /// Wakes the loops that wait while nothing needs them (`loops_wake`).
    pub(crate) fn wake_loops(&self) {
        self.loops_wake.send_modify(|count| *count = count.wrapping_add(1));
    }

    /// What wakes them: changed() resolves at the next `wake_loops`.
    pub(crate) fn loops_woken(&self) -> tokio::sync::watch::Receiver<u64> {
        self.loops_wake.subscribe()
    }

    /// changed() resolves at the Workspace's next change (`activity`).
    pub(crate) fn activity(&self) -> tokio::sync::watch::Receiver<u64> {
        self.activity.subscribe()
    }

    pub(crate) fn lock(&self) -> MutexGuard<'_, Inner> {
        // A panic while holding the lock leaves data that's still consistent
        // enough to keep serving; recover instead of poisoning every client.
        self.inner.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// The person's sentence-start name, copied while the settings are locked.
    pub(crate) fn person_capitalized(&self) -> String {
        self.lock().config.person_capitalized().to_owned()
    }

    /// The agent's current name, or its unchanged id if gone, copied under one settings lock.
    pub(crate) fn agent_name(&self, id: &str) -> String {
        self.lock().config.agent(id).map_or_else(|| id.to_owned(), |agent| agent.name.clone())
    }

    /// Subscribes to events and returns the state they apply on top of.
    pub async fn connect(&self) -> Snapshot {
        self.connect_resuming(None).await
    }

    /// `connect`, for an app that holds `held`'s conversation (its agent and
    /// revision, Hello's): the entries added or changed since, when that
    /// agent is still the one shown and that can be told (#413).
    pub(crate) async fn connect_resuming(&self, held: Option<(&str, &str)>) -> Snapshot {
        loop {
            let selected = self.lock().state.selected_agent.clone();
            let mut problem = None;
            if let Some(id) = &selected
                && let Err(error) = self.ensure_loaded(id).await
            {
                tracing::error!(agent = %id, %error, "could not load transcript");
                problem = Some(error);
            }
            let chat = self.chats.selected();
            if let Some(id) = &chat
                && let Err(error) = self.ensure_loaded(id).await
            {
                tracing::error!(chat = %id, %error, "could not load a chat's conversation");
            }
            let inner = self.lock();
            // Selection may have moved while loading; retry until the snapshot is complete.
            if inner.state.selected_agent != selected || self.chats.selected() != chat {
                continue;
            }
            let receiver = self.events.subscribe();
            // The explorer, the panes and the open panel follow it now.
            self.wake_loops();
            let (transcript, transcript_revision) = match selected.as_deref() {
                None => (None, String::new()),
                Some(id) => match self.resumed(&inner, id, held) {
                    Some((resumed, revision)) => (Some(resumed), revision),
                    None => match transcript_of(&inner, id) {
                        Some(page) => (Some(page), self.transcript_log.now(id)),
                        // An unreadable transcript mustn't keep clients out; show why it's missing.
                        None => (Some(unreadable(id, problem.as_ref())), String::new()),
                    },
                },
            };
            return Snapshot {
                events: receiver,
                workspace: self.workspace(&inner),
                transcript,
                transcript_revision,
                explorers: self.explorers(&inner),
                view: inner.view.clone(),
                // A client starts out with nothing queued.
                queued: self.queued_of(&inner).filter(|queued| !queued.messages.is_empty()),
                chat: crate::chats::shown(self, &inner),
            };
        }
    }

    /// The conversation of `id` an app holding `held` catches up with, and
    /// the revision it brings it to; None when it gets the latest page.
    fn resumed(&self, inner: &Inner, id: &str, held: Option<(&str, &str)>) -> Option<(v1::Transcript, String)> {
        let (agent, revision) = held?;
        if agent != id {
            return None;
        }
        let resumed = self.transcript_log.since(id, revision)?;
        let transcript = inner.agents.get(id)?.transcript.as_ref()?;
        let entries = resumed.seqs.iter().map(|seq| transcript.entry(*seq).cloned()).collect::<Option<Vec<_>>>()?;
        let shown = v1::Transcript { agent_id: id.to_owned(), entries, has_earlier: false, resumed: true };
        Some((shown, resumed.revision))
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
                config.projects.push(ProjectConfig {
                    id,
                    name: name.to_owned(),
                    path: path.clone(),
                    folders: Vec::new(),
                    archived: false,
                    other: Default::default(),
                });
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

    /// Renames a project, or archives it or brings it back (`archived`).
    /// UpdateProject: `before` moves it in the sidebar's order (friend bug
    /// #12), before that project, or last with "".
    pub async fn update_project(
        &self,
        id: &str,
        name: Option<&str>,
        archived: Option<bool>,
        before: Option<&str>,
    ) -> Result<(), RequestError> {
        self.edit_config(|config| {
            if config.project(id).is_none() {
                return Err(RequestError::UnknownProject(id.to_owned()));
            }
            if let Some(before) = before {
                crate::arrange::move_project(&mut config.projects, id, before)?;
            }
            let Some(project) = config.projects.iter_mut().find(|project| project.id == id) else {
                return Err(RequestError::UnknownProject(id.to_owned()));
            };
            if let Some(name) = name {
                project.name = name.trim().to_owned();
            }
            if let Some(archived) = archived {
                project.archived = archived;
            }
            Ok(())
        })
        .await
    }

    /// Removes a project and deletes its agents. Its folder stays.
    pub async fn delete_project(self: &Arc<Self>, id: &str) -> Result<(), RequestError> {
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
                config.forget_stale_leads();
                for agent in &removed {
                    crate::extensions::forget_agent(config, agent);
                    crate::apps::forget_agent(config, agent);
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
        let v1::CreateAgent { project_id: project, name, prompt, model, effort, color, folder_id: folder, role, lead_id: lead } = request;
        let (project, model, effort, folder) = (project.as_str(), model.as_str(), effort.as_str(), folder.as_str());
        let name = name.trim();
        if name.is_empty() {
            return Err(RequestError::Empty("name"));
        }
        let home = &self.home;
        // On the default model while others are switched away from its
        // provider, it starts where they are (U19, #343).
        let started = {
            let inner = self.lock();
            let config = &inner.config;
            let chosen = if model.is_empty() { config.default_model.as_str() } else { model };
            let chosen_effort = if effort.is_empty() { config.default_effort.as_str() } else { effort };
            (chosen == config.default_model)
                .then(|| {
                    crate::usage_switch::start_on(self, &inner, chosen, chosen_effort)
                        .map(|to| (chosen.to_owned(), chosen_effort.to_owned(), to))
                })
                .flatten()
        };
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
                let (model, effort) = match &started {
                    Some((from, _, (to, to_effort))) if *from == model => (to.clone(), to_effort.clone()),
                    _ => (model, effort),
                };
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
                    lead: None,
                    fallback_model: String::new(),
                    compact_at_tokens: None,
                    other: Default::default(),
                };
                let lead = lead.trim();
                if !lead.is_empty()
                    && let Some(why) = config.lead_refused(&agent, lead)
                {
                    return Err(RequestError::Lead(why));
                }
                let agent = AgentConfig { lead: Some(lead.to_owned()).filter(|lead| !lead.is_empty()), ..agent };
                config.agents.push(agent);
                Ok(id)
            })
            .await?;
        if let Some((from, from_effort, _)) = &started {
            crate::usage_switch::started_switched(self, &id, from, from_effort);
            self.broadcast_workspace(&self.lock());
        }
        self.select_agent(&id).await
    }

    /// Deletes an agent and its conversation, stopping it if it's running.
    pub async fn delete_agent(self: &Arc<Self>, id: &str) -> Result<(), RequestError> {
        self.edit_config(|config| {
            if config.agent(id).is_none() {
                return Err(RequestError::UnknownAgent(id.to_owned()));
            }
            crate::arrange::lift_children(config, id);
            config.agents.retain(|agent| agent.id != id);
            // No agent reports to it any more.
            config.forget_stale_leads();
            crate::extensions::forget_agent(config, id);
            crate::apps::forget_agent(config, id);
            Ok(())
        })
        .await?;
        self.delete_agent_data(id).await
    }

    async fn delete_agent_data(self: &Arc<Self>, id: &str) -> Result<(), RequestError> {
        self.browser_forget(id, "agent_changed");
        // Cards asking to clear its conversation have nothing left to clear.
        crate::extensions::ask::target_deleted(self, id);
        crate::vault::requests::forget_admin(self, id);
        crate::extensions::checklist::forget(&self.store, id)?;
        crate::extensions::memory::forget(&self.store, id)?;
        crate::extensions::monitors::forget(self, id)?;
        crate::proposals::forget(&self.store, id)?;
        crate::extensions::ask::forget(&self.store, id)?;
        crate::extensions::devices::forget(self, id).await?;
        crate::extensions::viewer::forget(self, id)?;
        crate::presentation::forget_agent(self, id)?;
        crate::pipes::start::forget(&self.store, id)?;
        crate::pipes::curator::forget(self, id)?;
        crate::extensions::agents::provenance::forget(&self.store, id)?;
        crate::user_apps::forget(self, id)?;
        crate::apps::access::forget(&self.store, id)?;
        crate::answered_on::forget(&self.store, id)?;
        self.watches.gone(&crate::watch::agent_key(id));
        self.usage_switches.forget(&self.store, id);
        self.forget_presence(id)?;
        match tokio::fs::remove_dir_all(self.home.agent_dir(id)).await {
            Err(error) if error.kind() != std::io::ErrorKind::NotFound => Err(error.into()),
            _ => Ok(()),
        }
    }

    /// What this Claktory is called and its color, as every device shows it.
    pub(crate) fn identity(&self) -> crate::remote::Identity {
        let config = &self.lock().config;
        crate::remote::Identity { name: config.claktory_name(), color: config.claktory_color(self.key_color).to_owned() }
    }

    /// New colours for the person's avatar: the pair they chose, or a
    /// shuffled one, never the one it had (a plain shuffle also forgets a
    /// chosen pair, so shuffling keeps working after a choice).
    pub(crate) async fn shuffle_avatar(&self, request: v1::ShuffleAvatar) -> Result<(), RequestError> {
        let key_seed = self.key_seed.clone();
        if let Some(chosen) = request.choice {
            let count = crate::person::PAIRS.len();
            if chosen < 0 || chosen as usize >= count {
                return Err(RequestError::Avatar(format!("choice must be 0 to {}", count - 1)));
            }
            return self
                .edit_config(|config| {
                    config.user.avatar.choice = Some(u32::try_from(chosen).expect("checked above"));
                    Ok(())
                })
                .await;
        }
        self.edit_config(|config| {
            let shown = crate::person::choice(config, &key_seed);
            config.user.avatar.choice = None;
            config.user.avatar.seed = crate::person::shuffled(shown);
            Ok(())
        })
        .await
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
            name,
            color,
            default_fallback_model,
            projects_collapsed,
            chats_collapsed,
            max_open_panes,
            notifications,
            checklist,
            notification_change,
            when_app_quits,
            resource_alerts_to_admins,
            resource_alert_agents,
        } = request;
        // The sidebar's sections, folded or open: the workspace's UI state
        // (#249), not settings, so a fold alone leaves config.json alone.
        let folds = projects_collapsed.is_some() || chats_collapsed.is_some();
        if folds {
            self.set_sections_collapsed(projects_collapsed, chats_collapsed)?;
        }
        // So is the pane limit: in factory.db, as an older backend after a
        // rollback would refuse a config.json that held it.
        if let Some(panes) = max_open_panes {
            self.set_max_open_panes(panes)?;
        }
        // And the notification settings (`crate::notify`).
        let notified = notifications.is_some() || notification_change.is_some();
        if let Some(notifications) = &notifications {
            crate::notify::update(self, notifications)?;
        }
        if let Some(change) = &notification_change {
            crate::notify::change(self, change)?;
        }
        // And the checklist's intervals (#282).
        if let Some(times) = &checklist
            && crate::extensions::checklist::Times::set(&self.store, times).map_err(RequestError::Checklist)?
        {
            self.broadcast_workspace(&self.lock());
        }
        // And what happens when the Mac app quits (friend bug #27).
        if let Some(mode) = when_app_quits {
            crate::service::set_when_app_quits(self, v1::QuitMode::try_from(mode).unwrap_or_default()).await?;
        }
        let only_folds = (folds || max_open_panes.is_some() || notified || checklist.is_some() || when_app_quits.is_some())
            && model.is_none()
            && effort.is_none()
            && compact_at.is_none()
            && ram_percent.is_none()
            && disk_free_gb.is_none()
            && disk_free_percent.is_none()
            && cpu_percent.is_none()
            && build_cache_gb.is_none()
            && sidebar_width.is_none()
            && panel_width.is_none()
            && user_name.is_none()
            && vault_unlock_minutes.is_none()
            && idle_compaction.is_none()
            && idle_compaction_short_cache.is_none()
            && cache_ttl.is_none()
            && name.is_none()
            && color.is_none()
            && default_fallback_model.is_none()
            && resource_alerts_to_admins.is_none()
            && resource_alert_agents.is_none();
        if only_folds {
            return Ok(());
        }
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
            if let Some(fallback) = &default_fallback_model {
                config.default_fallback_model = fallback.trim().to_owned();
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
            // Who hears the Resources alerts (friend bug #54).
            if let Some(on) = resource_alerts_to_admins {
                config.resources.alerts_to_admins = on;
            }
            if let Some(chosen) = &resource_alert_agents {
                let mut ids: Vec<String> = Vec::new();
                for id in chosen.agent_ids.iter().map(|id| id.trim()) {
                    if config.agent(id).is_none() {
                        return Err(RequestError::UnknownAgent(id.to_owned()));
                    }
                    if !ids.iter().any(|kept| kept == id) {
                        ids.push(id.to_owned());
                    }
                }
                // Chosen, it can look into what it hears: Resources goes on.
                for agent in config.agents.iter_mut().filter(|agent| ids.contains(&agent.id)) {
                    if !crate::extensions::enabled(agent, crate::extensions::resources::ID) {
                        agent
                            .extensions
                            .entry(crate::extensions::resources::ID.to_owned())
                            .or_default()
                            .insert("enabled".to_owned(), serde_json::Value::Bool(true));
                    }
                }
                config.resources.alert_agents = ids;
            }
            if let Some(name) = user_name {
                config.user.name = name.trim().to_owned();
            }
            // Empty goes back to this computer's name and the key's color.
            if let Some(name) = name {
                config.name = name.trim().to_owned();
            }
            if let Some(color) = color {
                config.color = color.trim().to_owned();
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
                                "{key}: ttl \"{ttl}\" isn't 5m, 30m, 1h or 12h"
                            ))));
                        };
                        // What config.json can hold but this version doesn't
                        // write: the backend it rolls back to would refuse it.
                        let provider = key.split_once('/').map_or(key.as_str(), |(provider, _)| provider);
                        if crate::config::cache_ttl_choices(provider).contains(&ttl)
                            && !crate::config::cache_ttl_offered(provider).contains(&ttl)
                        {
                            return Err(RequestError::Invalid(crate::config::Invalid::PromptCacheNotOffered(key)));
                        }
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

    /// SetZoom: how much `device` (a connection's own) zooms the app; 1 or 0
    /// for actual size, which leaves it out of config.json.
    pub async fn set_zoom(&self, device: &str, zoom: f64) -> Result<(), RequestError> {
        let actual = zoom == 0.0 || (zoom - 1.0).abs() < 1e-9;
        if !actual && !(crate::config::ZOOM_MIN..=crate::config::ZOOM_MAX).contains(&zoom) {
            let why = format!("zoom must be from {} to {}", crate::config::ZOOM_MIN, crate::config::ZOOM_MAX);
            return Err(RequestError::Invalid(crate::config::Invalid::BadDisplay(why)));
        }
        if actual && !self.lock().config.display.zoom.contains_key(device) {
            return Ok(());
        }
        self.edit_config(|config| {
            if actual {
                config.display.zoom.remove(device);
            } else {
                config.display.zoom.insert(device.to_owned(), zoom);
            }
            Ok(())
        })
        .await
    }

    /// SetFileCache: how many bytes of files `device` (a connection's own)
    /// keeps for its viewers (#446); 0 or the default (1 GB) leaves it out
    /// of config.json.
    pub async fn set_file_cache(&self, device: &str, bytes: u64) -> Result<(), RequestError> {
        use crate::config::{FILE_CACHE_DEFAULT, FILE_CACHE_MAX, FILE_CACHE_MIN};
        let default = bytes == 0 || bytes == FILE_CACHE_DEFAULT;
        if !default && !(FILE_CACHE_MIN..=FILE_CACHE_MAX).contains(&bytes) {
            let why = format!("file_cache must be from {FILE_CACHE_MIN} to {FILE_CACHE_MAX} bytes");
            return Err(RequestError::Invalid(crate::config::Invalid::BadDisplay(why)));
        }
        if default && !self.lock().config.display.file_cache.contains_key(device) {
            return Ok(());
        }
        self.edit_config(|config| {
            if default {
                config.display.file_cache.remove(device);
            } else {
                config.display.file_cache.insert(device.to_owned(), bytes);
            }
            Ok(())
        })
        .await
    }

    /// Overwrites a broken config.json with the settings in effect, keeping
    /// the broken file as config.broken.json.
    pub async fn restore_settings(&self) -> Result<(), RequestError> {
        let _files = self.files.lock().await;
        let read = self.config_text().await;
        let text = self.lock().config.to_json_keeping_form(self.home.root(), &read)?;
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
        let before = change.model.and_then(|_| self.lock().config.agent(id).map(|agent| agent.model.clone()));
        let model = change.model.map(str::to_owned);
        self.update_agent_keeping_switch(id, change).await?;
        // Its model chosen anew: a switch to another provider (#341) is over.
        if let (Some(before), Some(model)) = (before, model)
            && before != model
        {
            self.usage_switches.forget(&self.store, id);
            self.broadcast_workspace(&self.lock());
        }
        Ok(())
    }

    /// UpdateAgent's change, keeping a switch to another provider: what
    /// `crate::usage_switch` switches it with.
    pub(crate) async fn update_agent_keeping_switch(&self, id: &str, change: AgentChange<'_>) -> Result<(), RequestError> {
        let notify = change.notify;
        // Why, before the agent can see the change; said only while its model
        // is still the one this change gave it.
        if let Some(model) = change.model {
            let mut inner = self.lock();
            match change.why {
                Some(why) => inner.model_why.insert(id.to_owned(), (model.to_owned(), why.to_owned())),
                None => inner.model_why.remove(id),
            };
        }
        self.edit_config(|config| {
            if let Some(project) = change.project
                && !project.is_empty()
                && config.project(project).is_none()
            {
                return Err(RequestError::UnknownProject(project.to_owned()));
            }
            let AgentChange {
                name,
                prompt,
                model,
                effort,
                project,
                color,
                extension,
                placeholder,
                folder,
                avatar_shape,
                role,
                lead,
                fallback_model,
                // Kept in factory.db, after the change (`crate::notify`).
                notify: _,
                // Kept above, before the change.
                why: _,
            } = change;
            if let Some(extension) = extension {
                // Errands only for an agent the person contained for them.
                if extension.id == crate::errands::ERRANDS && extension.enabled == Some(true) && !self.pipes.errand_agent(id) {
                    return Err(RequestError::Remote(
                        "Errands is only for an agent contained for errands: contain it first, in its settings.".to_owned(),
                    ));
                }
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
            if project.is_some() {
                // A lead elsewhere now, and those reporting to it from there, go.
                config.forget_stale_leads();
            }
            if let Some(lead) = lead.map(str::trim).filter(|lead| !lead.is_empty()) {
                let Some(agent) = config.agent(id) else { return Err(RequestError::UnknownAgent(id.to_owned())) };
                if let Some(why) = config.lead_refused(agent, lead) {
                    return Err(RequestError::Lead(why));
                }
            }
            let Some(agent) = config.agent_mut(id) else { return Err(RequestError::UnknownAgent(id.to_owned())) };
            if let Some(lead) = lead {
                agent.lead = Some(lead.trim().to_owned()).filter(|lead| !lead.is_empty());
            }
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
            if let Some(fallback) = fallback_model {
                agent.fallback_model = fallback.trim().to_owned();
            }
            Ok(())
        })
        .await?;
        if let Some(mode) = notify {
            crate::notify::set_mode(self, id, crate::notify::Mode::from_proto(mode))?;
        }
        self.refresh_view(id);
        Ok(())
    }

    pub(crate) fn known_agent(&self, agent: &str) -> Result<(), RequestError> {
        if self.lock().config.agent(agent).is_none() {
            return Err(RequestError::UnknownAgent(agent.to_owned()));
        }
        Ok(())
    }

    pub async fn select_agent(&self, id: &str) -> Result<(), RequestError> {
        self.show_agent(id, None).await
    }

    /// A pipe's bound agent (#250), shown with its pipe: its conversation
    /// streams as the selected agent's while the pipe is what's shown.
    pub(crate) async fn show_with_pipe(&self, id: &str, pipe: &str) -> Result<(), RequestError> {
        self.show_agent(id, Some(pipe)).await
    }

    /// Shows the agent's conversation: alone (in place of a pipe's or a
    /// chat's), or with its pipe's.
    async fn show_agent(&self, id: &str, pipe: Option<&str>) -> Result<(), RequestError> {
        self.known_agent(id)?;
        self.ensure_loaded(id).await?;
        let mut inner = self.lock();
        inner.state.selected_agent = Some(id.to_owned());
        match pipe {
            Some(pipe) => self.pipes.show(pipe),
            None => self.pipes.deselect(),
        }
        crate::chats::leave(self, &mut inner);
        // Opening the agent is seeing what it did.
        inner.state.unseen.remove(id);
        let view = self.current_view(&inner);
        self.set_view(&mut inner, view);
        // Workspace first: clients learn the selection, then receive its transcript.
        self.broadcast_workspace(&inner);
        if let Some(transcript) = transcript_of(&inner, id) {
            self.send_transcript(&inner, transcript);
        }
        self.store.save_state(&inner.state)?;
        Ok(())
    }

    /// Earlier entries of an agent's transcript (LoadEntries), read from its
    /// files first if it isn't loaded.
    pub async fn load_entries(&self, request: &v1::LoadEntries) -> Result<v1::EntryPage, RequestError> {
        let id = request.agent_id.as_str();
        self.known_agent(id)?;
        self.ensure_loaded(id).await?;
        let page = crate::transcript::page_of(self, id, request.before_seq, request.limit, request.from_seq).await;
        let Some((entries, has_earlier)) = page else {
            return Err(RequestError::NoEntries(request.before_seq));
        };
        Ok(v1::EntryPage { agent_id: id.to_owned(), entries, has_earlier })
    }

    /// The person's message for an agent, sent from the device `sent_from`
    /// names, when known, with the id their app made up for it (`client_id`)
    /// and the files they attached (uploaded to the agent's uploads folder).
    pub fn send_message(
        self: &Arc<Self>,
        id: &str,
        text: &str,
        sent_from: Option<factory_agent::SentFrom>,
        client_id: &str,
        attached: &[v1::Attachment],
    ) -> Result<(), RequestError> {
        self.send_message_about(id, text, sent_from, client_id, attached, None)
    }

    /// `send_message`, as a reply on a checklist item when `item` names one
    /// (whose checklist, "" for the agent's own, and its id): the agent
    /// reads which, and the item's history keeps the reply.
    pub(crate) fn send_message_about(
        self: &Arc<Self>,
        id: &str,
        text: &str,
        sent_from: Option<factory_agent::SentFrom>,
        client_id: &str,
        attached: &[v1::Attachment],
        item: Option<(&str, u32)>,
    ) -> Result<(), RequestError> {
        // Sent again after a lost answer: delivered once (`crate::repeats`).
        crate::repeats::once(self, id, client_id, || self.send_message_once(id, text, sent_from, client_id, attached, item))
    }

    fn send_message_once(
        self: &Arc<Self>,
        id: &str,
        text: &str,
        sent_from: Option<factory_agent::SentFrom>,
        client_id: &str,
        attached: &[v1::Attachment],
        item: Option<(&str, u32)>,
    ) -> Result<(), RequestError> {
        let text = text.trim();
        if item.is_some() && text.is_empty() {
            return Err(RequestError::Checklist("Write your reply first.".to_owned()));
        }
        if text.is_empty() && attached.is_empty() {
            return Err(RequestError::Empty("message"));
        }
        self.known_agent(id)?;
        let checklist_item = match item {
            Some((checklist, item)) => {
                Some(crate::extensions::checklist::reply_target(self, id, checklist, item).map_err(RequestError::Checklist)?)
            }
            None => None,
        };
        let attachments = self.attachments(id, attached)?;
        let mut inner = self.lock();
        let queued = Queued {
            client_id: client_id.to_owned(),
            attachments,
            checklist_item: checklist_item.clone(),
            ..Queued::from_person(text, sent_from)
        };
        self.enqueue(&mut inner, id, queued)?;
        drop(inner);
        if let Some(item) = checklist_item {
            crate::extensions::checklist::replied(self, &item, text);
        }
        Ok(())
    }

    /// Gives an agent an event, waking it if it's idle. Gone agents miss it.
    pub(crate) fn deliver_event(self: &Arc<Self>, id: &str, text: String, event: factory_agent::Event) {
        self.deliver_queued(id, Queued::event(text, event));
    }

    /// An agent's message (`crate::extensions::agents`), with what Claktory
    /// records of its send; whether it was recorded.
    pub(crate) fn deliver_sent(
        self: &Arc<Self>,
        id: &str,
        text: String,
        event: factory_agent::Event,
        sent: crate::extensions::agents::provenance::Sent,
    ) -> bool {
        self.deliver_queued(id, Queued { sent: Some(sent), ..Queued::event(text, event) })
    }

    fn deliver_queued(self: &Arc<Self>, id: &str, queued: Queued) -> bool {
        let mut inner = self.lock();
        let from = queued.event.as_ref().map(|event| event.from.clone()).unwrap_or_default();
        if inner.config.agent(id).is_none() && !crate::chats::kept(self, id) {
            tracing::info!(agent = %id, %from, "event for an agent that's gone");
            return false;
        }
        match self.enqueue(&mut inner, id, queued) {
            Ok(()) => true,
            Err(error) => {
                tracing::error!(agent = %id, %from, %error, "could not record an event, so it's lost");
                false
            }
        }
    }

    /// Records a message for an existing agent and queues it: a working
    /// agent reads it after its current step, an idle one starts on it.
    pub(crate) fn enqueue(self: &Arc<Self>, inner: &mut Inner, id: &str, mut queued: Queued) -> Result<(), rusqlite::Error> {
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
        // An agent's message: its number, which the agent reads.
        if let Some(provenance) = queued.event.as_mut().and_then(|event| event.provenance.as_mut()) {
            provenance.message = u64::try_from(queued.id).unwrap_or_default();
        }
        let person = queued.event.is_none();
        inner.agents.entry(id.to_owned()).or_default().queue.push(queued);
        // The person's message to an agent waiting out a usage limit doesn't
        // queue dead: it makes the agent ask the provider again now, since
        // the person may just have brought usage back (`probe_accounts`).
        if person && inner.agents.get(id).is_some_and(|runtime| runtime.usage_wait.is_some()) {
            let model =
                inner.config.agent(id).map(|agent| agent.model.clone()).or_else(|| self.chats.model_and_name(id).map(|(model, _)| model));
            if let Some(provider) = model.and_then(|model| model.split_once('/').map(|(provider, _)| provider.to_owned())) {
                tokio::spawn(self.clone().probe_accounts(provider));
            }
        }
        self.wake(inner, id);
        Ok(())
    }

    /// Gives the agent what's queued for it that can go now: a working
    /// agent reads it after its current step, an idle one starts on it. (A
    /// voice message being transcribed waits, and the person's after it.)
    pub(crate) fn wake(self: &Arc<Self>, inner: &mut Inner, id: &str) {
        let archived = inner.config.agent(id).is_some_and(|agent| inner.config.archived(agent));
        if let Some(runtime) = inner.agents.get_mut(id)
            && runtime.has_ready()
        {
            runtime.stopped = false;
            // New work wins over compacting while idle: the conversation stays
            // as it was, and the run goes on with the message.
            if runtime.idle
                && let Some(abort) = &runtime.abort
            {
                abort.abort();
            }
            if archived {
                // Its project is archived: it waits for it to come back.
            } else if !runtime.running {
                runtime.running = true;
                tokio::spawn(self.clone().run(id.to_owned()));
            } else {
                runtime.make_way();
            }
        }
        self.broadcast_workspace(inner);
    }

    pub fn stop_agent(&self, id: &str) -> Result<(), RequestError> {
        let mut inner = self.lock();
        if inner.config.agent(id).is_none() {
            return Err(RequestError::UnknownAgent(id.to_owned()));
        }
        if let Some(runtime) = inner.agents.get_mut(id) {
            // What's queued stays: it reaches the agent after the stop, as a
            // new run (/clear drops it).
            self.stop_run(runtime);
        }
        self.broadcast_workspace(&inner);
        Ok(())
    }

    pub(crate) async fn ensure_loaded(&self, id: &str) -> Result<(), TranscriptError> {
        if let Some(runtime) = self.lock().agents.get_mut(id)
            && runtime.transcript.is_some()
        {
            runtime.used_ms = crate::transcript::now_ms();
            return Ok(());
        }
        let folder = if crate::chats::is_chat(id) { crate::chats::dir(&self.home, id) } else { self.home.agent_dir(id) };
        let mut transcript = Transcript::load(folder, id.to_owned()).await?;
        transcript.save_old_form(self.old_segments.clone());
        if let Err(error) = crate::proposals::restore(&self.store, id, &mut transcript) {
            tracing::error!(agent = %id, %error, "could not read proposals; decided ones show as pending");
        }
        if let Err(error) = crate::pipes::agents::restore(self, id, &mut transcript) {
            tracing::error!(agent = %id, %error, "could not read pipe secret approvals; decided ones show as waiting");
        }
        if let Err(error) = crate::extensions::ask::restore(&self.store, &self.command_runs, id, &mut transcript) {
            tracing::error!(agent = %id, %error, "could not read its questions; answered ones show as waiting");
        }
        if let Err(error) = crate::extensions::devices::restore(&self.store, id, &mut transcript) {
            tracing::error!(agent = %id, %error, "could not read the devices it adds; they show as waiting");
        }
        if let Err(error) = crate::pipes::start::restore(&self.store, id, &mut transcript) {
            tracing::error!(agent = %id, %error, "could not read its cards for starting pipes; they show as waiting");
        }
        if let Err(error) = crate::user_apps::restore(&self.store, id, &mut transcript) {
            tracing::error!(agent = %id, %error, "could not read its proposals of app access; decided ones show as waiting");
        }
        if let Err(error) = crate::apps::access::restore(&self.store, id, &mut transcript) {
            tracing::error!(agent = %id, %error, "could not read its proposals of what apps read; decided ones show as waiting");
        }
        match self.vault.requests_of(id) {
            Ok(requests) => {
                for request in requests {
                    // A waiting one with what fits it now (#360); the rest
                    // as decided.
                    if request.state() == v1::SecretRequestState::Pending {
                        transcript.set_secret_request(crate::vault::requests::fitted(self, request));
                    } else {
                        transcript.set_secret_request(request);
                    }
                }
            }
            Err(error) => tracing::error!(agent = %id, %error, "could not read secret requests; decided ones show as pending"),
        }
        // The add-secret cards this agent opened for others' requests
        // (secrets_collect) show as those requests are now.
        match self.vault.requests_with_admin(id) {
            Ok(requests) => {
                for request in requests.into_iter().filter(|request| request.agent_id != id) {
                    transcript.set_secret_request(request);
                }
            }
            Err(error) => tracing::error!(agent = %id, %error, "could not read the requests it collects; decided ones show as open"),
        }
        // And those whose asker was deleted, its requests with it, close.
        crate::vault::requests::close_missing(self, &mut transcript);
        match self.vault.proposals_of(id) {
            Ok(proposals) => {
                for proposal in proposals.into_iter().filter(|proposal| proposal.state() != v1::ConnectProposalState::Pending) {
                    transcript.set_connect_proposal(proposal);
                }
            }
            Err(error) => {
                tracing::error!(agent = %id, %error, "could not read proposals to connect services; decided ones show as pending")
            }
        }
        // Which device each decided card was decided from (#514).
        if let Err(error) = crate::answered_on::restore(&self.store, id, &mut transcript) {
            tracing::error!(agent = %id, %error, "could not read which devices its cards were decided from");
        }
        let mut inner = self.lock();
        let runtime = inner.agents.entry(id.to_owned()).or_default();
        runtime.used_ms = crate::transcript::now_ms();
        if runtime.transcript.is_none() {
            let waiting = !transcript.waiting_seqs().is_empty();
            runtime.transcript = Some(transcript);
            // What waits in it shows with the agent.
            if waiting {
                self.broadcast_workspace(&inner);
            }
        }
        Ok(())
    }

    /// Whether something of the agent's waits for the person: a request for
    /// a secret, a proposal to connect a service or to Rivet, a question.
    pub(crate) fn has_waiting(&self, id: &str) -> bool {
        let requests = self.vault.requests_of(id).unwrap_or_default();
        let connects = self.vault.proposals_of(id).unwrap_or_default();
        let collecting = self.vault.requests_with_admin(id).unwrap_or_default();
        // An approved request with an admin agent waits on it, not the
        // person (`crate::transcript`'s waits agrees).
        requests.iter().any(|request| {
            request.state() == v1::SecretRequestState::Pending
                || (request.state() == v1::SecretRequestState::Approved && request.admin_agent_id.is_empty())
        }) || collecting
            .iter()
            .any(|request| request.agent_id != id && request.collecting && request.state() == v1::SecretRequestState::Approved)
            || connects.iter().any(|proposal| proposal.state() == v1::ConnectProposalState::Pending)
            || crate::proposals::pending_of(&self.store, id)
            || crate::extensions::ask::pending_of(&self.store, id)
            || crate::extensions::devices::waiting_of(&self.store, id)
            || crate::pipes::start::waiting_of(&self.store, id)
            || crate::user_apps::pending_of(&self.store, id)
            || crate::apps::access::pending_of(&self.store, id)
    }

    /// Changes the config in memory and on disk, or changes nothing.
    pub(crate) async fn edit_config<R>(&self, edit: impl FnOnce(&mut Config) -> Result<R, RequestError>) -> Result<R, RequestError> {
        let _files = self.files.lock().await;
        // Start from the file, not memory: an agent may have edited it since the last poll.
        self.sync_config().await;
        let read = self.config_text().await;
        let (result, text) = {
            let mut inner = self.lock();
            if inner.config_error.is_some() {
                return Err(RequestError::ConfigBroken);
            }
            let mut config = inner.config.clone();
            let result = edit(&mut config)?;
            config.keep_colors(Some(&inner.config));
            config.follow_default_prompts();
            // An agent deleted, moved out of its project or archived with
            // it is unpinned (`crate::pins`).
            config.prune_pins();
            config.validate()?;
            let text = config.to_json_keeping_form(self.home.root(), &read)?;
            self.apply_config(&mut inner, config);
            self.broadcast_workspace(&inner);
            (result, text)
        };
        write_atomic(&self.home.config_path(), text.as_bytes()).await?;
        Ok(result)
    }

    /// config.json as it is now, for the form the next write keeps
    /// (`Config::to_json_keeping_form`); empty, so all absolute, when it
    /// can't be read. Callers hold `files`.
    async fn config_text(&self) -> String {
        tokio::fs::read_to_string(self.home.config_path()).await.unwrap_or_default()
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
        let mut parsed = Config::parse(&text, self.home.root());
        // An agent written in without a color gets one, and a shipped prompt
        // written in becomes this version's, saved.
        if let Ok(config) = &mut parsed {
            let colorless = config.agents.iter().any(|agent| agent.color.is_empty());
            if colorless {
                let before = self.lock().config.clone();
                config.keep_colors(Some(&before));
            }
            if config.follow_default_prompts() | config.prune_pins() | colorless {
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
                for unknown in config.unknown_keys() {
                    tracing::warn!(%unknown, "config.json: keeping a setting this version doesn't know");
                }
                for unknown in crate::extensions::unknown_settings(&config) {
                    tracing::warn!(%unknown, "config.json: ignoring an extension setting");
                }
                // A model written into config.json: the agent hears where it came from.
                for agent in &config.agents {
                    if inner.config.agent(&agent.id).is_some_and(|before| before.model != agent.model) {
                        let why = "it was changed in Claktory's settings file".to_owned();
                        inner.model_why.insert(agent.id.clone(), (agent.model.clone(), why));
                    }
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
        let saved = match config.to_json_keeping_form(self.home.root(), read) {
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
            self.capacity_changed();
            self.onboarding.models_changed.notify_one();
        }
    }

    /// Swaps in a valid config, stopping agents it removed.
    fn apply_config(&self, inner: &mut Inner, config: Config) {
        // An agent waiting for an account may have another model now.
        self.capacity_changed();
        let mut removed = Vec::new();
        for id in inner.agents.keys() {
            if config.agent(id).is_none() && !crate::chats::is_chat(id) {
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
        let gone: Vec<String> =
            inner.config.agents.iter().filter(|agent| config.agent(&agent.id).is_none()).map(|agent| agent.id.clone()).collect();
        for agent in gone {
            if let Err(error) = self.vault.forget_agent(&agent) {
                tracing::error!(%error, agent = %agent, "could not take a removed agent's grants");
            }
            // Its requests went too: their cards in other conversations close.
            crate::vault::requests::close_cards_in(self, inner, &agent);
        }
        self.add_presences(&config.agents);
        // Agents whose project was archived stop, keeping what's queued for
        // them; those whose project came back read it.
        let mut unarchived = false;
        for agent in &config.agents {
            let (was, is) = (inner.config.agent(&agent.id).is_some_and(|before| inner.config.archived(before)), config.archived(agent));
            if is
                && !was
                && let Some(runtime) = inner.agents.get_mut(&agent.id)
            {
                self.stop_run(runtime);
            }
            unarchived |= was && !is;
        }
        // An archived project's out of view: it isn't focused any more.
        let unfocused = inner.state.focused_project.as_deref().is_some_and(|id| config.project(id).is_some_and(|project| project.archived));
        if unfocused {
            inner.state.focused_project = None;
        }
        // Browser control just turned on turns itself off again later.
        crate::browser::activation::observe(self, &inner.config, &config);
        // Reports for an agent deleted or moved away wait for the person.
        self.pipes.handoff.agents_changed(&inner.config, &config);
        inner.config = config;
        if unarchived {
            self.unarchived.notify_one();
        }
        if selected_gone {
            // The workspace broadcast that follows tells clients nothing is selected.
            inner.state.selected_agent = None;
        }
        // Otherwise an agent added later with a removed one's id would inherit its marks.
        if (pruned || selected_gone || unfocused)
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
        let panel = |view: &Option<v1::ExtensionView>| view.as_ref().map(|view| view.extension_id.clone());
        if panel(&view) != panel(&inner.view) {
            // Another panel: what follows it wakes (the Tasks panel's output).
            self.wake_loops();
        }
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
                project.folders.push(FolderConfig { id, name: name.to_owned(), path: path.clone(), other: Default::default() });
                Ok(path)
            })
            .await?;
        tokio::fs::create_dir_all(&path).await?;
        self.refresh_explorer().await;
        Ok(())
    }

    /// Renames a project's folder, or points it at another folder on disk.
    /// Renames a project's folder, points it elsewhere, or moves it in the
    /// project's list (`before`: the folder it goes before, "" for last).
    pub async fn update_folder(
        &self,
        project: &str,
        id: &str,
        name: Option<&str>,
        path: Option<&str>,
        before: Option<&str>,
    ) -> Result<(), RequestError> {
        let name = name.map(str::trim);
        if name == Some("") {
            return Err(RequestError::Empty("name"));
        }
        let chosen = path.map(folder_path).transpose()?.flatten();
        let path = self
            .edit_config(|config| {
                if let Some(before) = before {
                    let Some(found) = config.projects.iter_mut().find(|candidate| candidate.id == project) else {
                        return Err(RequestError::UnknownProject(project.to_owned()));
                    };
                    crate::arrange::move_folder(&mut found.folders, id, before)?;
                }
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
        self.known_agent(agent)?;
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

    /// Folds the sidebar's Projects or Chats section away, or opens it; None
    /// leaves one as it is.
    fn set_sections_collapsed(&self, projects: Option<bool>, chats: Option<bool>) -> Result<(), RequestError> {
        let mut inner = self.lock();
        let before = (inner.state.projects_collapsed, inner.state.chats_collapsed);
        inner.state.projects_collapsed = projects.unwrap_or(before.0);
        inner.state.chats_collapsed = chats.unwrap_or(before.1);
        if (inner.state.projects_collapsed, inner.state.chats_collapsed) != before {
            self.broadcast_workspace(&inner);
            self.store.save_state(&inner.state)?;
        }
        Ok(())
    }

    /// The most panes `show` keeps open besides the conversation; 0 for the
    /// default.
    fn set_max_open_panes(&self, panes: u32) -> Result<(), RequestError> {
        if panes > config::MAX_OPEN_PANES {
            let why = format!("max_open_panes must be from 1 to {}, or 0 for the default", config::MAX_OPEN_PANES);
            return Err(RequestError::Invalid(Invalid::BadDisplay(why)));
        }
        let mut inner = self.lock();
        if inner.state.max_open_panes != panes {
            inner.state.max_open_panes = panes;
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
        self.activity.send_modify(|count| *count = count.wrapping_add(1));
        let workspace = self.workspace(inner);
        // What newly waits for the person, or answers them, notifies.
        crate::notify::observe(self, inner, &workspace);
        // A change to the Map is kept for History (#458).
        self.map_history.observe(self, &workspace);
        self.send(Body::Workspace(workspace));
        let queued = self.queued_of(inner);
        let mut sent = self.queued_sent.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        if *sent != queued {
            if let Some(queued) = &queued {
                self.send(Body::QueuedMessages(queued.clone()));
            }
            *sent = queued;
        }
        drop(sent);
        crate::chats::broadcast_queue(self, inner);
        self.watches.queues(inner);
    }

    /// What waits for the selected agent, for clients.
    fn queued_of(&self, inner: &Inner) -> Option<v1::QueuedMessages> {
        let id = inner.state.selected_agent.as_deref()?;
        Some(queued_for(inner, id))
    }

    /// Sends a transcript change to clients if the agent is the selected one.
    pub(crate) fn send_change(&self, inner: &Inner, agent_id: &str, change: Change) {
        if crate::chats::is_chat(agent_id) {
            return crate::chats::send_change(self, agent_id, change);
        }
        // Followed even while another agent is shown, so a resume of it is
        // right whatever was shown meanwhile.
        let seq = match &change {
            Change::Entry(entry) => entry.seq,
            Change::Delta { seq, .. } => *seq,
        };
        let revision = self.transcript_log.changed(agent_id, seq).unwrap_or_default();
        // To connections watching it without selecting it (#514).
        let key = crate::watch::agent_key(agent_id);
        if self.watches.watched(&key) {
            let agent_id = agent_id.to_owned();
            let body = match &change {
                Change::Entry(entry) => Body::Entry(v1::EntryUpdate { agent_id, entry: Some(entry.clone()) }),
                Change::Delta { seq, text } => Body::Delta(v1::EntryDelta { agent_id, seq: *seq, text: text.clone() }),
            };
            self.watches.send(&key, body);
        }
        if inner.state.selected_agent.as_deref() != Some(agent_id) {
            return;
        }
        let agent_id = agent_id.to_owned();
        let body = match change {
            Change::Entry(entry) => Body::Entry(v1::EntryUpdate { agent_id, entry: Some(entry) }),
            Change::Delta { seq, text } => Body::Delta(v1::EntryDelta { agent_id, seq, text }),
        };
        self.send_revised(body, revision);
    }

    /// Sends the conversation shown, whole: apps replace what they held, and
    /// resume from its revision (#413). With the lock held, as every change
    /// to it is.
    pub(crate) fn send_transcript(&self, _inner: &Inner, transcript: v1::Transcript) {
        let revision = self.transcript_log.whole(&transcript.agent_id);
        self.send_revised(Body::Transcript(transcript), revision);
    }

    /// `send`, with the conversation's revision (ServerMessage.transcript_revision).
    fn send_revised(&self, body: Body, transcript_revision: String) {
        if self.events.receiver_count() == 0 {
            return;
        }
        let message = v1::ServerMessage { reply_to: 0, body: Some(body), transcript_revision, ..Default::default() };
        let _ = self.events.send(Bytes::from(message.encode_to_vec()));
    }

    /// How many apps are connected now.
    pub(crate) fn clients(&self) -> usize {
        self.events.receiver_count()
    }

    /// Whether Settings shows Apps (Workspace.apps_available).
    pub(crate) fn apps_available(&self) -> bool {
        self.apps.available()
    }

    pub(crate) fn send(&self, body: Body) {
        if self.events.receiver_count() == 0 {
            return;
        }
        let message = v1::ServerMessage { reply_to: 0, body: Some(body), ..Default::default() };
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
                archived: project.archived,
                report_forward: crate::pipes::handoff::project_view(self, &project.id),
            });
        }
        let mut agents = Vec::new();
        let now = crate::transcript::now_ms();
        let unread_copies = crate::extensions::agents::provenance::unread(&self.store);
        // Checklist items waiting on the person (#279), by agent.
        let card_seq = |agent: &str, card: &crate::extensions::checklist::Card| {
            let transcript = inner.agents.get(agent).and_then(|runtime| runtime.transcript.as_ref());
            transcript.and_then(|transcript| crate::extensions::checklist::seq_of(transcript, card)).unwrap_or_default()
        };
        let mut waiting_items = crate::extensions::checklist::waiting_items(self, card_seq).unwrap_or_else(|error| {
            tracing::error!(%error, "could not read the checklist items waiting on the person");
            Default::default()
        });
        // Its header's counts (area 4 §8.9).
        let checklist_open = crate::extensions::checklist::open_counts(&self.store).unwrap_or_else(|error| {
            tracing::error!(%error, "could not count the checklists' open items");
            Default::default()
        });
        let tasks_running = self.store.running_counts().unwrap_or_else(|error| {
            tracing::error!(%error, "could not count the running tasks");
            Default::default()
        });
        let switching = crate::usage_switch::view(self, inner);
        let pools_in_use = crate::usage_switch::pools_in_use(self, inner);
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
                context_tokens: runtime
                    .map_or(0, |runtime| runtime.transcript.as_ref().map_or(runtime.held_tokens, Transcript::context_tokens)),
                compact_at_tokens: inner.config.compact_at(agent),
                queued_messages: runtime.map_or(0, |runtime| (runtime.queue.len() - runtime.quiet_waiting()) as u32),
                quiet_waiting: runtime.map_or(0, |runtime| runtime.quiet_waiting() as u32),
                reads_reports: self.pipes.curator(&agent.id),
                spend: crate::pipes::spend::proto(self, &agent.id),
                held_asks: crate::extensions::ask::held_today(&self.store, &agent.id),
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
                layout_focus: self.viewer.focus(&agent.id),
                waiting_items: waiting_items.remove(&agent.id).unwrap_or_default(),
                default_prompt: Config::default_prompt(agent).unwrap_or_default().to_owned(),
                waiting_seqs: runtime.and_then(|runtime| runtime.transcript.as_ref()).map(Transcript::waiting_seqs).unwrap_or_default(),
                lead_id: inner.config.lead(agent).map(|lead| lead.id.clone()).unwrap_or_default(),
                usage_wait: runtime.and_then(|runtime| runtime.usage_wait.as_ref()).map(|wait| wait.text.clone()).unwrap_or_default(),
                usage_wait_until_ms: runtime.and_then(|runtime| runtime.usage_wait.as_ref()).map_or(0, |wait| wait.until_ms),
                tainted_by_pipes: self.pipes.tainted(&agent.id),
                quarantined: self.pipes.quarantined(&agent.id),
                contained: self.pipes.errand_agent(&agent.id),
                pipe_id: self.pipes.bound_pipe(&agent.id),
                fallback_model: agent.fallback_model.clone(),
                checklist_open: crate::extensions::enabled(agent, crate::extensions::checklist::ID)
                    .then(|| checklist_open.get(&agent.id).copied())
                    .flatten()
                    .unwrap_or(0),
                tasks_running: crate::extensions::enabled(agent, crate::extensions::tasks::ID)
                    .then(|| tasks_running.get(&agent.id).copied())
                    .flatten()
                    .unwrap_or(0),
                notify: crate::notify::settings(self).mode(&agent.id).to_proto().into(),
                unread_copies: unread_copies.get(&agent.id).copied().unwrap_or_default(),
                usage_wait_alternative: switching.alternative(&agent.id),
                usage_wait_no_other_provider: switching.no_other_provider(&agent.id),
                usage_switch: switching.switch(&agent.id),
                branches: crate::fork::count(&self.home.agent_dir(&agent.id)),
            });
        }
        let config = &inner.config;
        // Which tasks run, read once for the whole vault (#402), and only
        // when a grant has tasks that got its secret lately.
        let running = std::cell::OnceCell::new();
        let mut vault = self.vault.state(|agent, task| {
            running
                .get_or_init(|| {
                    self.store.running_set().unwrap_or_else(|error| {
                        tracing::error!(%error, "could not read which background tasks run");
                        Default::default()
                    })
                })
                .contains(&(agent.to_owned(), task.to_owned()))
        });
        if vault.exists && !vault.unlocked {
            vault.agents_waiting = self.agents_waiting_for_vault(inner);
            let problem = self.account.view(&inner.config.account).map(|account| account.problem).unwrap_or_default();
            vault.locked_why = crate::vault::signins::locked_why(&self.vault, problem);
        }
        // Given to an agent quarantined for a pipe, a secret isn't usable by it.
        for secret in &mut vault.secrets {
            for grant in &secret.grants {
                if self.pipes.contained(&grant.agent_id) {
                    let name = config.agent(&grant.agent_id).map_or(grant.agent_id.as_str(), |agent| agent.name.as_str());
                    secret.warnings.push(format!("{name} can't use it while it's limited to a pipe."));
                }
            }
        }
        if vault.exists {
            vault.connectors = self.broker.catalog();
            vault.flows = self.broker.flows();
        }
        let apps = crate::apps::infos(self, config, &vault);
        let (pipes, selected_pipe_id) = crate::pipes::view(self, inner);
        let (chats, selected_chat_id) = crate::chats::view(self, inner, &switching);
        let provider_waits = crate::provider_waits::view(inner, &agents, &chats);
        let (alerts, alerts_unread) = self.notify.alerts.view(self);
        v1::Workspace {
            provider_waits,
            alerts,
            alerts_unread,
            os: std::env::consts::OS.to_owned(),
            map: Some(crate::map::build(config, &self.home, &vault, &apps)),
            apps,
            apps_available: self.apps.available(),
            pinned_apps: crate::apps::pins::view(self, config),
            bundled_apps: crate::apps::bundled::view(self, config),
            pipe_groups: crate::pipes::groups::view(self, inner),
            pipe_shares: crate::pipes::shares::view(self),
            browser_sessions: self.browser.view(),
            dm_sends: crate::user_apps::dms::sends::waiting(self),
            pipes,
            speech_model: Some(self.speech.view()),
            chats,
            selected_chat_id,
            r#move: self.moving.view(),
            projects_collapsed: inner.state.projects_collapsed,
            chats_collapsed: inner.state.chats_collapsed,
            selected_pipe_id,
            person: Some(crate::person::person(config, &self.key_seed, self.account.signed_in_as(&config.account))),
            projects,
            agents,
            models: crate::extensions::images::listed(inner),
            version: crate::update::VERSION.to_owned(),
            focused_project_id: inner.state.focused_project.clone().filter(|id| inner.config.project(id).is_some()).unwrap_or_default(),
            latest_release: inner.latest_release.version.clone(),
            latest_backend: inner.latest_release.backend.clone(),
            changelog: inner.latest_release.changelog.iter().map(crate::update::ChangelogVersion::to_proto).collect(),
            remote: Some(self.remote_view(inner)),
            vault: Some(vault),
            account: self.account.view(&config.account),
            update_error: inner.update_error.clone(),
            update_check: Some(inner.update_check.clone()),
            keep_awake: Some(self.keep_awake.view(&config.claktory_name())),
            backend_service: Some(self.service.view()),
            // Claktory's browser (#435): unset while it's off here.
            agent_browser: self.agent_browser.view(self, &config.browser),
            errands: self.errands.listed(inner),
            diagnostics: Some(crate::diagnostics::view(self)),
            report_schemas: crate::pipes::handoff::schemas(self),
            report_type_rules: Some(crate::pipes::handoff::type_rules()),
            usage_auto_continue: self.usage_switches.auto_continue(),
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
                            in_use: account.in_use && pools_in_use.contains_key(&(provider.id.clone(), false)),
                            fable_in_use: account.fable_in_use && pools_in_use.contains_key(&(provider.id.clone(), true)),
                            usage: inner.usage.get(&key(&account.id)).cloned().or_else(|| account.usage.clone()),
                            plan: inner.plans.get(&key(&account.id)).cloned().unwrap_or_else(|| account.plan.clone()),
                            credit: inner.credit.get(&key(&account.id)).cloned(),
                            ..account.clone()
                        })
                        .collect();
                    v1::Provider {
                        // Until the first read, what the sign-in says: empty for a subscription.
                        usage: if signed_in { inner.usage.get(&first).cloned().or_else(|| provider.usage.clone()) } else { None },
                        plan: if signed_in { inner.plans.get(&first).cloned().unwrap_or_default() } else { String::new() },
                        accounts,
                        cache_ttl: config
                            .cache_ttl(&provider.id)
                            .filter(|_| crate::config::cache_ttl_choices(&provider.id).contains(&factory_agent::CacheTtl::OneHour))
                            .map(|ttl| ttl.as_str().to_owned())
                            .unwrap_or_default(),
                        cache_lifetime: config.cache_ttl(&provider.id).map(|ttl| ttl.as_str().to_owned()).unwrap_or_default(),
                        cache_ttl_choices: crate::config::cache_ttl_offered(&provider.id)
                            .iter()
                            .map(|ttl| ttl.as_str().to_owned())
                            .collect(),
                        agents: pools_in_use.iter().filter(|((on, _), _)| *on == provider.id).map(|(_, count)| count).sum(),
                        ..provider.clone()
                    }
                })
                .map(|mut provider| {
                    let fetchable = !provider.signed_in_with.is_empty() && self.models.fetches_models(&provider.id);
                    self.catalog.view(&mut provider, fetchable);
                    provider
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
                zoom: config.display.zoom.iter().map(|(device, zoom)| (device.clone(), *zoom)).collect(),
                file_cache: config.display.file_cache.clone(),
                focus_mode: self.focus_mode.devices(self),
                pinned_agents: config.pinned_agents.clone(),
                user_name: config.user.name.clone(),
                vault_unlock_minutes: config.vault.unlock_minutes,
                idle_compaction: config.compaction.idle,
                idle_compaction_short_cache: config.compaction.idle_short_cache,
                cache_ttls: config
                    .prompt_cache
                    .iter()
                    .map(|(key, ttl)| v1::CacheTtl { key: key.clone(), ttl: ttl.as_str().to_owned() })
                    .collect(),
                name: config.claktory_name(),
                color: config.claktory_color(self.key_color).to_owned(),
                onboarding: Some(self.onboarding.view()),
                default_fallback_model: config.default_fallback_model.clone(),
                max_open_panes: config::open_panes(inner.state.max_open_panes),
                notifications: Some(crate::notify::settings(self).to_proto()),
                checklist: Some(crate::extensions::checklist::Times::read(&self.store).proto()),
                when_app_quits: self.service.when_app_quits().into(),
                resource_alerts_to_admins: config.resources.alerts_to_admins,
                resource_alert_agents: crate::extensions::resources::hearers(config, &inner.models),
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
pub(crate) fn fitting_effort<'a>(models: &'a [v1::Model], model: &str) -> &'a str {
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
    let notice = v1::Notice { text, level: v1::NoticeLevel::Error.into(), ..Default::default() };
    let entry = v1::Entry {
        seq: 0,
        timestamp_ms: crate::transcript::now_ms(),
        kind: Some(v1::entry::Kind::Notice(notice)),
        reply: 0,
        replied_to: None,
        answered_on: None,
    };
    v1::Transcript { agent_id: id.to_owned(), entries: vec![entry], has_earlier: false, ..Default::default() }
}

/// A queued message as clients see it.
pub(crate) fn queued_message(queued: &Queued) -> v1::QueuedMessage {
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
        client_id: queued.client_id.clone(),
        attachments: queued.attachments.iter().map(crate::transcript::attachment).collect(),
        transcription: queued.voice.as_ref().map(crate::speech::voice::Voice::view),
        checklist_item: queued.checklist_item.as_ref().map(crate::transcript::checklist_item),
        quiet: queued.quiet(),
        typed: queued.voice.as_ref().map(|voice| voice.typed.clone()).unwrap_or_default(),
    }
}

/// The agent's transcript as a snapshot sends it: its latest entries.
/// What waits for agent `id`, for clients.
pub(crate) fn queued_for(inner: &Inner, id: &str) -> v1::QueuedMessages {
    let queue = inner.agents.get(id).map_or(&[][..], |runtime| &runtime.queue[..]);
    let messages = queue.iter().map(queued_message).collect();
    v1::QueuedMessages { agent_id: id.to_owned(), messages }
}

pub(crate) fn transcript_of(inner: &Inner, id: &str) -> Option<v1::Transcript> {
    let transcript = inner.agents.get(id)?.transcript.as_ref()?;
    let (entries, has_earlier) = transcript.recent();
    Some(v1::Transcript { agent_id: id.to_owned(), entries, has_earlier, ..Default::default() })
}

/// Reads the conversations of agents with something waiting for the
/// person when Claktory starts, so every device sees what waits (each
/// agent's `waiting_seqs`) without opening them.
async fn load_waiting(factory: Weak<Factory>) {
    let Some(factory) = factory.upgrade() else { return };
    let mut ids: Vec<String> = factory.lock().config.agents.iter().map(|agent| agent.id.clone()).collect();
    // Chats' proposals to Rivet too.
    ids.extend(factory.chats.ids());
    for id in ids {
        if !factory.has_waiting(&id) {
            continue;
        }
        if let Err(error) = factory.ensure_loaded(&id).await {
            tracing::warn!(agent = %id, %error, "could not read the transcript of an agent with something waiting");
        }
    }
}

/// Starts the agents of a project brought back from the archive on what
/// waited for them.
async fn resume_unarchived(factory: Weak<Factory>) {
    loop {
        let Some(notified) = factory.upgrade() else { return };
        notified.unarchived.notified().await;
        drop(notified);
        let Some(factory) = factory.upgrade() else { return };
        let mut inner = factory.lock();
        let ready: Vec<String> = inner
            .config
            .agents
            .iter()
            .filter(|agent| !inner.config.archived(agent))
            .filter(|agent| inner.agents.get(&agent.id).is_some_and(|runtime| !runtime.running && runtime.has_ready()))
            .map(|agent| agent.id.clone())
            .collect();
        for id in ready {
            if let Some(runtime) = inner.agents.get_mut(&id) {
                tracing::info!(agent = %id, waiting = runtime.queue.len(), "its project is back: reading what waited");
                runtime.running = true;
                runtime.stopped = false;
                tokio::spawn(factory.clone().run(id));
            }
        }
        factory.broadcast_workspace(&inner);
    }
}

/// Picks up edits to config.json, new logins in auth.json and newly published
/// releases, whoever made them.
async fn watch(factory: Weak<Factory>) {
    let (config_path, auth_path, release_path) = match factory.upgrade() {
        Some(factory) => (
            factory.home.config_path(),
            factory.models.auth_path().unwrap_or_else(|| factory.home.auth_path()),
            factory.home.releases().join("latest"),
        ),
        None => return,
    };
    // Watched before they're first read, so no change falls between.
    let mut files = crate::file_watch::FileWatch::new(vec![config_path.clone(), auth_path.clone(), release_path.clone()]);
    let mut config_stamp = stamp(&config_path).await;
    let mut auth_stamp = stamp(&auth_path).await;
    let mut release_stamp = stamp(&release_path).await;
    // An edit since Factory read the files, before the stamps above, counts
    // too: a sign-in or a release as Factory started is noticed, not missed
    // until the next.
    match factory.upgrade() {
        Some(factory) => {
            factory.reload_config().await;
            crate::extensions::ask::close_for_gone(&factory);
            factory.refresh_models().await;
            factory.refresh_release().await;
        }
        None => return,
    }
    loop {
        let wait = if files.ensure() { WATCH_SAFETY } else { WATCH_INTERVAL };
        // Looked at once the watches are armed again, and only then waited
        // on: a change made while they were being armed raises no event,
        // and would wait for the safety look (Abbacchio 34947).
        {
            let Some(factory) = factory.upgrade() else { return };
            let config_now = stamp(&config_path).await;
            if config_now != config_stamp {
                config_stamp = config_now;
                factory.reload_config().await;
                // An agent taken out of the settings, however (Rivet edits them).
                crate::extensions::ask::close_for_gone(&factory);
            }
            let auth_now = stamp(&auth_path).await;
            if auth_now != auth_stamp {
                // A sign-in made elsewhere (`factoryd login`), into the vault.
                if let Err(error) = factory.models.take_in_sign_ins().await {
                    tracing::warn!(%error, "could not take in the sign-ins in auth.json");
                }
                // What was taken in is written there again, or emptied.
                auth_stamp = stamp(&auth_path).await;
                factory.refresh_models().await;
                // Signed in to again, perhaps, which the listing may not show:
                // waiting agents ask again (friend bug #3). One whose accounts
                // are all still spent asks nothing.
                factory.capacity_changed();
            }
            let release_now = stamp(&release_path).await;
            if release_now != release_stamp {
                release_stamp = release_now;
                factory.refresh_release().await;
            }
        }
        tokio::select! {
            () = files.changed.notified() => {}
            () = tokio::time::sleep(wait) => {}
        }
    }
}

/// Lists the explorers' folders again every half second while an app is
/// connected (it's for apps only); with none, waits for one.
async fn explore(factory: Weak<Factory>) {
    let Some(mut woken) = factory.upgrade().map(|factory| factory.loops_woken()) else { return };
    loop {
        let Some(connected) = factory.upgrade().map(|factory| factory.clients() > 0) else { return };
        if connected {
            tokio::time::sleep(WATCH_INTERVAL).await;
        } else if woken.changed().await.is_err() {
            return;
        }
        let Some(factory) = factory.upgrade() else { return };
        factory.refresh_explorer().await;
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
        let mut hidden = self.vault.every_variable();
        hidden.extend(self.broker.hidden(&self.vault));
        self.redaction.set(factory_agent::Redactor::new(&hidden));
        self.vault_moved.notify_one();
        // A secret added or gone may fit a waiting request now, or no more.
        crate::vault::requests::refresh_fitting(self);
        self.broadcast_workspace(&self.lock());
        // An app's secret given or taken back may start or stop it.
        self.apps.poke();
    }

    /// Checks the secret requests waiting as if `later_ms` had passed; for
    /// tests.
    pub fn expire_secret_requests_after(self: &Arc<Self>, later_ms: i64) {
        crate::vault::requests::expire_requests(self, crate::transcript::now_ms() + later_ms);
    }

    /// A stand-in for the kernel's lid sleep, on a laptop as far as
    /// Claktory knows; for tests.
    pub fn set_lid_closed_clamshell(&self, clamshell: Arc<dyn crate::lid_closed::Clamshell>) {
        self.keep_awake.lid.set_clamshell(clamshell);
        self.keep_awake.runs_changed();
    }

    /// Has keeping the Mac awake looked at again now, rather than at its
    /// next 30-second look; for tests.
    pub fn keep_awake_changed(&self) {
        self.keep_awake.runs_changed();
    }

    /// Before the backend stops: lets a closed lid sleep the Mac again, if
    /// Claktory kept it from it (#371). The next start would otherwise.
    pub fn stopping(&self) {
        self.keep_awake.lid.stop(&self.store);
        self.write_calls();
        crate::command_cards::stop_all(self);
    }

    /// Writes the model calls counted so far (`crate::calls`), as it does a
    /// minute after the first; for tests that read factory.db themselves.
    pub fn write_calls(&self) {
        self.store.write_calls();
    }

    /// Makes new vault passphrases cheap to check; for tests.
    pub fn set_vault_passphrase_cost(&self, params: &'static str) {
        self.vault.set_passphrase_params(params);
    }
}
