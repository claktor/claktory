//! Factory's settings: one plain JSON file that people, agents and the app all
//! edit. The backend watches it and applies valid changes immediately.

use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};

use factory_agent::CacheTtl;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

pub const DEFAULT_COMPACT_AT: u64 = 400_000;
pub const DEFAULT_MODEL: &str = "anthropic/claude-opus-5-5";
pub const DEFAULT_EFFORT: &str = "high";
/// Admin agents hear of a background task running longer than this.
pub const DEFAULT_TASK_ALERT_HOURS: u64 = 24;
pub const EFFORTS: [&str; 7] = ["off", "minimal", "low", "medium", "high", "xhigh", "max"];

/// The agent every Factory starts with.
pub const RIVET: &str = "rivet";
/// Rivet's prompt as shipped; the file's trailing newline isn't part of it.
/// Its `{{user}}`-style placeholders are filled in when the system prompt is
/// written (`crate::manual::render`), so the saved copy stays this text.
const RIVET_PROMPT: &str = include_str!("rivet-prompt.md");
/// SHA-256 (hex) of every Rivet prompt Claktory has shipped, this one
/// included, each without trailing whitespace. A Rivet whose prompt is one
/// of them was never edited, so it follows the default: it's given this
/// version's (`Config::follow_default_prompts`). Changing rivet-prompt.md
/// means adding its new hash here, never removing one (a test checks).
const SHIPPED_RIVET_PROMPTS: &[&str] = &[
    "ed5aab63acd5bfe29585aa8e7c05a7d8bb26ec96e5dd1335c5ac5a5a631c51a1",
    "bb08afb70c45a65884f0d7e6579839a6840d9ea772340bde8ea48c7c9d7e7c20",
    "5a60c84c032044246db90d6cf9ed775368b9a614a5323ab514b8071b4bbecc8a",
    "7bec0e2d35f7d6689af81cdc0d570c5dba134b22c5b348b9a18d470f6e3a52e9",
    "64894df788d2ada2c1d61feadc84b92fe7d4aae70209c482e46b4b75e640613d",
    "13667721780258db83e69eaeaf656b9d93dc512073ab17b54967bdf2473edc23",
    "e2ff2ec11f50a36790e2565154cdd4fb3898bafb4fa533ff31c60beacd5518e0",
    "3a08dce43408450cf07f61168441c96f43567cf9e3e624ef212183368f3f9fa4",
    "807640ce610ae87fd1a51f72b439d93cef8fbf6e814eace0b3a76ba9e46e21f0",
    // {{user}} for "the person".
    "7d935762bff903785cbbc4d3b57f605b213ddbd4628b6074fa641978c4c88bb7",
    // Connected services.
    "d36d20a71ddcb3df22e8bc47fcd49b3e3e663af41041aef7167b6514924507c8",
    // Approved secret requests (#223).
    "7d84d2855daa56e7a63dae5da9571ee5dd12213651d1541dd0cc773eb47857ae",
];
/// Rivet is interactive, so it gets medium effort, the Opus 5.5 default.
const RIVET_EFFORT: &str = "medium";
/// What Rivet does, beside its name.
const RIVET_ROLE: &str = "Admin";
/// What Rivet's message box offers in a new Factory, so a new person knows
/// what it's for: examples of what to ask, one per line, which the app shows
/// in turn. Each asks for agents that do a job, not for the job itself.
const RIVET_PLACEHOLDER: &str = "Set up a team that works on my codebase while I sleep\n\
Build me a team that builds an iPhone app for my running club\n\
Set up agents that build and run the website for my bakery\n\
Give me an AI accountant that keeps my books and does my taxes\n\
Set up a team that makes images and short videos for my product launch\n\
Add a reviewer that checks every pull request and fixes failing tests\n\
Set up an agent that drafts my social posts for me to approve\n\
Give me an assistant that reads my inbox every morning and tells me what matters\n\
Set up a travel agent that plans my Japan trip and keeps the bookings together\n\
Set up a researcher that briefs me on my competitors every Monday\n\
Give me an agent that answers my customers' emails in my voice\n\
Set up an agent that tracks my spending and tells me where it goes\n\
Set up a team that finds me a new job and preps me for interviews\n\
Set up a team that turns my podcast episodes into blog posts and clips";

/// Settings in config.json this version doesn't know, by key, in any part
/// of it: a newer Claktory's, read after the supervisor went back to this
/// one (or a typo in a hand edit). They're kept and written back as they
/// were, so a downgrade never makes the file unreadable nor loses a newer
/// setting, and only logged (`Config::unknown_keys`). Versions before this
/// refused a file with any.
pub type Unknown = BTreeMap<String, Value>;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Config {
    /// This Claktory's name, the same on every device; unset, this
    /// computer's (`Config::claktory_name`). Only written once set, so a
    /// backend from before it can still read the file.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub name: String,
    /// This Claktory's color, one of `AGENT_COLORS`; unset, one chosen from
    /// its key (`remote::key_color`). Only written once set.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub color: String,
    /// Agents' contexts are summarized when they reach this many tokens.
    #[serde(default = "default_compact_at")]
    pub compact_at_tokens: u64,
    /// Model for new agents that don't name one: "<provider>/<model id>".
    #[serde(default = "default_model")]
    pub default_model: String,
    #[serde(default = "default_effort")]
    pub default_effort: String,
    /// The model a refused step is retried on for agents that name no
    /// `fallback_model` of their own; empty for none.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub default_fallback_model: String,
    #[serde(default)]
    pub projects: Vec<ProjectConfig>,
    #[serde(default)]
    pub agents: Vec<AgentConfig>,
    /// The person wants highlights shown in the app. Only written when off.
    #[serde(default = "yes", skip_serializing_if = "is_true")]
    pub highlights: bool,
    /// Admin agents hear of a background task once it has run this many hours.
    #[serde(default = "default_task_alert_hours", skip_serializing_if = "is_default_task_alert_hours")]
    pub task_alert_hours: u64,
    /// How paired devices on other machines reach Factory (`crate::remote`):
    /// the relays of its private tunnel. Whether they may is decided by the
    /// devices paired, not by this.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remote: Option<RemoteConfig>,
    /// When agents with the Resources extension hear the machine runs low.
    /// Only written when changed.
    #[serde(default, skip_serializing_if = "ResourcesConfig::is_default")]
    pub resources: ResourcesConfig,
    /// How the app is laid out, the same on every device. Only written when
    /// changed.
    #[serde(default, skip_serializing_if = "DisplayConfig::is_default")]
    pub display: DisplayConfig,
    /// The person agents work for. Only written when set.
    #[serde(default, skip_serializing_if = "UserConfig::is_default")]
    pub user: UserConfig,
    /// The vault's settings (`crate::vault`). Only written when changed.
    #[serde(default, skip_serializing_if = "VaultConfig::is_default")]
    pub vault: VaultConfig,
    /// When agents compact besides a full context (`crate::idle`). Only
    /// written when changed.
    #[serde(default, skip_serializing_if = "CompactionConfig::is_default")]
    pub compaction: CompactionConfig,
    /// How long prompt caches are asked to last, by provider id or model id
    /// (a model's wins); what isn't listed asks for the provider's default.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub prompt_cache: BTreeMap<String, CacheTtl>,
    /// Signing in to claktory.com (`crate::account`). Only written when
    /// changed.
    #[serde(default, skip_serializing_if = "AccountConfig::is_default")]
    pub account: AccountConfig,
    /// The apps installed (`crate::apps`). Only written once there's one.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub apps: Vec<crate::apps::AppConfig>,
    /// Settings this version doesn't know, kept and written back as they
    /// were (`Unknown`).
    #[serde(flatten)]
    pub other: Unknown,
}

/// Claktory's account at claktory.com, or at a service of the person's own.
/// What it's signed in with lives in `<home>/account`, never here.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct AccountConfig {
    /// Signing in is on (the default): the app asks for it, and a signed-in
    /// Claktory calls the service. Off, nothing calls the service and the app
    /// doesn't ask. Every Claktory with it on must sign in (`crate::account`).
    pub enabled: bool,
    /// The service to sign in at next; empty for claktory.com's
    /// (`crate::account::DEFAULT_SERVICE`).
    #[serde(skip_serializing_if = "String::is_empty")]
    pub service: String,
    /// Settings this version doesn't know, kept and written back as they
    /// were (`Unknown`).
    #[serde(flatten)]
    pub other: Unknown,
}

impl Default for AccountConfig {
    fn default() -> AccountConfig {
        AccountConfig { enabled: true, service: String::new(), other: Default::default() }
    }
}

impl AccountConfig {
    fn is_default(&self) -> bool {
        *self == AccountConfig::default()
    }
}

/// When agents compact besides a full context.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct CompactionConfig {
    /// An idle agent compacts just before its prompt cache expires.
    pub idle: bool,
    /// Also when the cache lasts only 5 minutes (after 4 idle minutes).
    pub idle_short_cache: bool,
    /// Settings this version doesn't know, kept and written back as they
    /// were (`Unknown`).
    #[serde(flatten)]
    pub other: Unknown,
}

impl Default for CompactionConfig {
    fn default() -> CompactionConfig {
        CompactionConfig { idle: true, idle_short_cache: false, other: Default::default() }
    }
}

impl CompactionConfig {
    fn is_default(&self) -> bool {
        *self == CompactionConfig::default()
    }
}

/// The vault's settings. Which secrets exist and who may use them live in
/// the vault itself, never here: agents can edit this file.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct VaultConfig {
    /// Minutes a connection stays unlocked after the person gave the vault
    /// passphrase on it, 1 to 60.
    pub unlock_minutes: u32,
    /// Settings this version doesn't know, kept and written back as they
    /// were (`Unknown`).
    #[serde(flatten)]
    pub other: Unknown,
}

impl Default for VaultConfig {
    fn default() -> VaultConfig {
        VaultConfig { unlock_minutes: 5, other: Default::default() }
    }
}

impl VaultConfig {
    fn is_default(&self) -> bool {
        *self == VaultConfig::default()
    }
}

/// The person agents work for. Unlike the rest, it keeps what it doesn't
/// know (`other`): a later backend's profile fields (a picture, say) survive
/// a rollback to this one, and a rollback can't refuse the file over them.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct UserConfig {
    /// Their first name, which agents call them by; empty when not given.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub name: String,
    /// Their avatar's colours (`crate::person`). Only written once they
    /// shuffle them.
    #[serde(skip_serializing_if = "AvatarConfig::is_default")]
    pub avatar: AvatarConfig,
    #[serde(flatten)]
    pub other: BTreeMap<String, serde_json::Value>,
}

/// The person's avatar: the seed its colours come from. Keeps what it
/// doesn't know, as `UserConfig` does.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct AvatarConfig {
    /// Random, made when the person shuffles; empty until then, and the
    /// colours come from this Claktory's key.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub seed: String,
    /// The pair the person picked from the palette (`crate::person::PAIRS`,
    /// an index), used over the seed while set. A plain shuffle clears it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub choice: Option<u32>,
    #[serde(flatten)]
    pub other: BTreeMap<String, serde_json::Value>,
}

impl AvatarConfig {
    fn is_default(&self) -> bool {
        *self == AvatarConfig::default()
    }
}

impl UserConfig {
    fn is_default(&self) -> bool {
        *self == UserConfig::default()
    }
}

/// The widths the person dragged the sidebar and the right panel to, in
/// logical pixels; 0 for the app's default. The app keeps them in its range.
/// And how much each device zooms the app.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct DisplayConfig {
    #[serde(skip_serializing_if = "is_zero")]
    pub sidebar_width: f64,
    #[serde(skip_serializing_if = "is_zero")]
    pub panel_width: f64,
    /// Each device's zoom by its id ("local" for this machine's own app),
    /// `ZOOM_MIN` to `ZOOM_MAX`; actual size (1) isn't written. Written only
    /// once someone zooms, so an untouched file stays readable by a backend
    /// from before it.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub zoom: BTreeMap<String, f64>,
    /// Settings this version doesn't know, kept and written back as they
    /// were (`Unknown`).
    #[serde(flatten)]
    pub other: Unknown,
}

/// How far a device can zoom the app out and in.
pub const ZOOM_MIN: f64 = 0.5;
pub const ZOOM_MAX: f64 = 2.0;

/// Panes `show` keeps open besides the conversation, until the person
/// changes it…
pub const DEFAULT_OPEN_PANES: u32 = 4;
/// …and at most: a layout's 12 panes, less the conversation.
pub const MAX_OPEN_PANES: u32 = 11;

/// The most panes `show` keeps open besides the conversation, as the person
/// set it (`State::max_open_panes`; 0 for the default).
pub fn open_panes(chosen: u32) -> u32 {
    if chosen == 0 { DEFAULT_OPEN_PANES } else { chosen.min(MAX_OPEN_PANES) }
}

impl DisplayConfig {
    fn is_default(&self) -> bool {
        *self == DisplayConfig::default()
    }

    fn check(&self) -> Result<(), Invalid> {
        for (name, value) in [("sidebar_width", self.sidebar_width), ("panel_width", self.panel_width)] {
            if !value.is_finite() || value < 0.0 {
                return Err(Invalid::BadDisplay(format!("{name} must be a width of 0 or more")));
            }
        }
        for (device, zoom) in &self.zoom {
            if !(ZOOM_MIN..=ZOOM_MAX).contains(zoom) {
                return Err(Invalid::BadDisplay(format!("zoom.{device} must be from {ZOOM_MIN} to {ZOOM_MAX}")));
            }
        }
        Ok(())
    }
}

fn is_zero(value: &f64) -> bool {
    *value == 0.0
}

fn is_zero_count(value: &u32) -> bool {
    *value == 0
}

/// The Resources extension's alerts (`crate::extensions::resources`), and
/// the build caches' budget (`crate::extensions::tree::caches`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ResourcesConfig {
    /// Memory used above this percentage for a few minutes.
    pub ram_percent: u32,
    /// A volume Factory uses with less free than this many GB or
    /// `disk_free_percent` of it, whichever is smaller; 0 turns it off.
    pub disk_free_gb: u32,
    pub disk_free_percent: u32,
    /// The processors busier than this percentage for several minutes; 0, as
    /// unless set, turns it off.
    pub cpu_percent: u32,
    /// GB the build caches of one checkout or project folder may take before
    /// they're cleared, when nothing works there; 0 never clears them. Only
    /// written when changed, so a backend from before it reads the rest.
    #[serde(skip_serializing_if = "is_default_build_cache")]
    pub build_cache_gb: f64,
    /// Settings this version doesn't know, kept and written back as they
    /// were (`Unknown`).
    #[serde(flatten)]
    pub other: Unknown,
}

const DEFAULT_BUILD_CACHE_GB: f64 = 25.0;

fn is_default_build_cache(gb: &f64) -> bool {
    *gb == DEFAULT_BUILD_CACHE_GB
}

impl Default for ResourcesConfig {
    fn default() -> ResourcesConfig {
        ResourcesConfig {
            ram_percent: 90,
            disk_free_gb: 10,
            disk_free_percent: 10,
            cpu_percent: 0,
            build_cache_gb: DEFAULT_BUILD_CACHE_GB,
            other: Default::default(),
        }
    }
}

impl ResourcesConfig {
    fn is_default(&self) -> bool {
        *self == ResourcesConfig::default()
    }

    fn check(&self) -> Result<(), Invalid> {
        for (name, value, least) in
            [("ram_percent", self.ram_percent, 1), ("disk_free_percent", self.disk_free_percent, 1), ("cpu_percent", self.cpu_percent, 0)]
        {
            if !(least..=100).contains(&value) {
                return Err(Invalid::BadResources(format!("{name} must be from {least} to 100")));
            }
        }
        if !self.build_cache_gb.is_finite() || self.build_cache_gb < 0.0 {
            return Err(Invalid::BadResources("build_cache_gb must be 0 (never cleared) or more".to_owned()));
        }
        Ok(())
    }
}

/// The private tunnel's settings. `port` and `listen` were the TLS
/// listener's, which is gone: still read, so older files load, and kept, but
/// unused.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemoteConfig {
    #[serde(default, skip_serializing_if = "is_zero_port")]
    pub port: u16,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub listen: String,
    /// The relays devices reach Factory through when there's no direct path,
    /// as URLs; empty for the default ones (n0's).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub relays: Vec<String>,
    /// Settings this version doesn't know, kept and written back as they
    /// were (`Unknown`).
    #[serde(flatten)]
    pub other: Unknown,
}

fn is_zero_port(port: &u16) -> bool {
    *port == 0
}

impl RemoteConfig {
    pub fn check(&self) -> Result<(), Invalid> {
        for relay in &self.relays {
            let url: iroh::RelayUrl = relay.parse().map_err(|_| Invalid::BadRemote(format!("relay \"{relay}\" isn't a URL")))?;
            if !matches!(url.scheme(), "http" | "https") {
                return Err(Invalid::BadRemote(format!("relay \"{relay}\" isn't an http or https URL")));
            }
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ProjectConfig {
    pub id: String,
    pub name: String,
    pub path: PathBuf,
    /// Folders some of its agents work in, in the person's order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub folders: Vec<FolderConfig>,
    /// Put away, with nothing deleted: its agents don't run (what's sent to
    /// them waits), their monitors pause, no agent can message them, and the
    /// Map leaves it out, until it's unarchived.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub archived: bool,
    /// Settings this version doesn't know, kept and written back as they
    /// were (`Unknown`).
    #[serde(flatten)]
    pub other: Unknown,
}

/// A folder inside a project, where some of its agents work. It only scopes
/// where they work: messaging and everything else stay project-wide.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FolderConfig {
    /// Unique within the project.
    pub id: String,
    pub name: String,
    /// In memory always absolute. In the file: relative when inside
    /// Claktory's home (its territory, resolved against the home it's
    /// read from), absolute anywhere else, such as a repository outside
    /// the workspace.
    pub path: PathBuf,
    /// Settings this version doesn't know, kept and written back as they
    /// were (`Unknown`).
    #[serde(flatten)]
    pub other: Unknown,
}

impl ProjectConfig {
    pub fn folder(&self, id: &str) -> Option<&FolderConfig> {
        self.folders.iter().find(|folder| folder.id == id)
    }
}

/// A project or folder path as read: relative in the file means under
/// the Claktory home it was read from.
fn resolve(path: &mut PathBuf, root: &Path) {
    if path.is_relative() {
        *path = root.join(&path);
    }
}

/// A project or folder path as written: inside the root it's emitted
/// relative, so the file doesn't name the machine. The root itself
/// stays as it is.
fn relativize(path: &mut PathBuf, root: &Path) {
    if let Ok(rest) = path.strip_prefix(root)
        && !rest.as_os_str().is_empty()
    {
        *path = rest.to_path_buf();
    }
}

/// Whether `read`, a settings file's text, writes any project or folder
/// path relative.
fn any_relative(read: &str) -> bool {
    let Ok(value) = serde_json::from_str::<Value>(read) else { return false };
    let relative = |path: &Value| path.as_str().is_some_and(|path| std::path::Path::new(path).is_relative());
    value["projects"].as_array().into_iter().flatten().any(|project| {
        relative(&project["path"]) || project["folders"].as_array().into_iter().flatten().any(|folder| relative(&folder["path"]))
    })
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AgentConfig {
    pub id: String,
    /// One of `AGENT_COLORS`. Factory gives every agent one and saves it
    /// (`Config::keep_colors`), so it never follows the agent's place.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub color: String,
    /// The id of the project the agent works in; none for the workspace itself.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
    /// The agent it's nested under in the sidebar, in the same project.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<String>,
    /// The agent it reports to, which sees how it's set up (agents_inspect):
    /// one in its project, or an admin agent. One that isn't, or that would
    /// close a loop, is ignored (`Config::lead`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lead: Option<String>,
    pub name: String,
    /// What it does ("Coordinator"), shown with its name; empty for none.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub role: String,
    /// Instructions added to the agent's system prompt. Older configs call it "purpose".
    #[serde(default, alias = "purpose")]
    pub prompt: String,
    pub model: String,
    pub effort: String,
    /// The model a step `model` refused is retried on, once per refused
    /// turn; empty uses `default_fallback_model`, and with both empty a
    /// refusal just ends the step.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub fallback_model: String,
    /// What its message box says when empty; empty for "Message <name>".
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub placeholder: String,
    /// The id of the project folder it works in; empty for the project's own
    /// folder. One its project doesn't have is ignored, like a bad `parent`.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub folder: String,
    /// Its extensions' settings by extension id, e.g. `{"agents": {"reachable": false}}`.
    /// Anything left out takes the extension's default.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub extensions: BTreeMap<String, Map<String, Value>>,
    /// Its character's shape as chosen; left out, automatic.
    #[serde(default, skip_serializing_if = "Avatar::is_automatic")]
    pub avatar: Avatar,
    /// Settings this version doesn't know, kept and written back as they
    /// were (`Unknown`).
    #[serde(flatten)]
    pub other: Unknown,
}

/// An agent's character as chosen in its settings: `shape` empty for
/// automatic (`crate::characters`). Only a choice is kept, never what
/// automatic picks; one this Factory doesn't know is kept too, and drawn as
/// automatic.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Avatar {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub shape: String,
    /// Characters had faces once: still read, so older files stay valid,
    /// and ignored (dropped when the file is next saved).
    #[serde(default, skip_serializing)]
    pub face: String,
    /// Settings this version doesn't know, kept and written back as they
    /// were (`Unknown`).
    #[serde(flatten)]
    pub other: Unknown,
}

impl Avatar {
    pub fn is_automatic(&self) -> bool {
        self.shape.is_empty()
    }
}

fn default_compact_at() -> u64 {
    DEFAULT_COMPACT_AT
}

fn default_model() -> String {
    DEFAULT_MODEL.to_owned()
}

fn default_effort() -> String {
    DEFAULT_EFFORT.to_owned()
}

fn default_task_alert_hours() -> u64 {
    DEFAULT_TASK_ALERT_HOURS
}

#[allow(clippy::trivially_copy_pass_by_ref)] // serde's skip_serializing_if passes a reference
fn is_default_task_alert_hours(hours: &u64) -> bool {
    *hours == DEFAULT_TASK_ALERT_HOURS
}

fn yes() -> bool {
    true
}

#[allow(clippy::trivially_copy_pass_by_ref)] // serde's skip_serializing_if passes a reference
fn is_true(value: &bool) -> bool {
    *value
}

/// Rivet is the admin agent: it sees and messages every agent, and hands out
/// work without hearing back, so no agent can message it. It points the
/// person at what it sets up with highlights.
fn rivet_extensions() -> BTreeMap<String, Map<String, Value>> {
    let mut agents = Map::new();
    agents.insert("reachable".to_owned(), Value::Bool(false));
    agents.insert("admin".to_owned(), Value::Bool(true));
    let mut highlight = Map::new();
    highlight.insert("enabled".to_owned(), Value::Bool(true));
    BTreeMap::from([("agents".to_owned(), agents), ("highlight".to_owned(), highlight)])
}

/// A new Factory: no projects, and Rivet in the workspace.
impl Default for Config {
    fn default() -> Config {
        let rivet = AgentConfig {
            id: RIVET.to_owned(),
            project: None,
            parent: None,
            lead: None,
            name: "Rivet".to_owned(),
            role: RIVET_ROLE.to_owned(),
            prompt: RIVET_PROMPT.trim_end().to_owned(),
            model: default_model(),
            effort: RIVET_EFFORT.to_owned(),
            color: AGENT_COLORS[0].to_owned(),
            placeholder: RIVET_PLACEHOLDER.to_owned(),
            folder: String::new(),
            extensions: rivet_extensions(),
            avatar: Avatar { shape: "hexagon".to_owned(), face: String::new(), other: Default::default() },
            fallback_model: String::new(),
            other: Default::default(),
        };
        Config {
            name: String::new(),
            color: String::new(),
            compact_at_tokens: DEFAULT_COMPACT_AT,
            default_model: default_model(),
            default_effort: default_effort(),
            projects: Vec::new(),
            agents: vec![rivet],
            highlights: true,
            task_alert_hours: DEFAULT_TASK_ALERT_HOURS,
            remote: None,
            resources: ResourcesConfig::default(),
            display: DisplayConfig::default(),
            user: UserConfig::default(),
            vault: VaultConfig::default(),
            compaction: CompactionConfig::default(),
            prompt_cache: BTreeMap::new(),
            account: AccountConfig::default(),
            apps: Vec::new(),
            default_fallback_model: String::new(),
            other: Default::default(),
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("config.json is not valid: {0}")]
    Parse(#[from] serde_json::Error),
    #[error(transparent)]
    Invalid(#[from] Invalid),
}

#[derive(Debug, thiserror::Error)]
pub enum Invalid {
    #[error("id \"{0}\" must be non-empty and use only a-z, 0-9 and -")]
    BadId(String),
    #[error("two entries have the id \"{0}\"")]
    DuplicateId(String),
    #[error("\"{0}\" has an empty name")]
    EmptyName(String),
    #[error("project \"{0}\": path must be absolute")]
    RelativePath(String),
    #[error("project \"{project}\": folder \"{folder}\" needs an absolute path")]
    RelativeFolder { project: String, folder: String },
    #[error("agent \"{agent}\" is in project \"{project}\", which doesn't exist")]
    UnknownProject { agent: String, project: String },
    #[error("model \"{0}\" must look like \"provider/model-id\"")]
    BadModel(String),
    #[error("effort \"{0}\" must be one of off, minimal, low, medium, high, xhigh, max")]
    BadEffort(String),
    #[error("compact_at_tokens must be positive")]
    ZeroCompaction,
    #[error("task_alert_hours must be positive")]
    ZeroTaskAlert,
    #[error("remote: {0}")]
    BadRemote(String),
    #[error("resources: {0}")]
    BadResources(String),
    #[error("display: {0}")]
    BadDisplay(String),
    #[error("vault: unlock_minutes must be 1 to 60")]
    BadVault,
    #[error("account: service \"{0}\" must be an https:// address (http:// only on this computer)")]
    BadAccountService(String),
    #[error("prompt_cache: \"{0}\" must be a provider whose cache lifetime can be chosen ({providers}) or one of their models (\"anthropic/<model id>\")", providers = CACHE_TTL_PROVIDERS.join(", "))]
    BadPromptCache(String),
    #[error("prompt_cache: \"{0}\" can't be \"{1}\": it's {2}")]
    BadPromptCacheTtl(String, String, String),
    #[error("prompt_cache: \"{0}\"'s cache lifetime can't be chosen yet")]
    PromptCacheNotOffered(String),
    #[error("color \"{0}\" must be one of blue, violet, green, pink, teal, amber, red, slate")]
    BadColor(String),
    #[error("name \"{0}\" can't have line breaks or other control characters")]
    BadName(String),
    #[error("agent \"{agent}\": extensions setting \"{setting}\" must be {expected}")]
    BadExtensionSetting { agent: String, setting: String, expected: String },
    #[error("notifications: {0}")]
    BadNotifications(String),
}

impl Config {
    /// Reads the settings as kept at `root` (the Claktory home the file
    /// belongs to): a project or folder path written relative is under
    /// the root — Claktory's own territory, the same wherever the home
    /// moves — and is resolved against it here. Absolute paths (external
    /// folders, and every path a config written before relative forms
    /// holds) are kept as they are, so old files read forever.
    pub fn parse(text: &str, root: &Path) -> Result<Config, ConfigError> {
        let mut config: Config = serde_json::from_str(text)?;
        for project in &mut config.projects {
            resolve(&mut project.path, root);
            for folder in &mut project.folders {
                resolve(&mut folder.path, root);
            }
        }
        config.validate()?;
        Ok(config)
    }

    /// This Claktory's name: as set, or this computer's.
    pub fn claktory_name(&self) -> String {
        match self.name.trim() {
            "" => crate::remote::machine_name().to_owned(),
            name => name.to_owned(),
        }
    }

    /// This Claktory's color: as set, or `default` (`remote::key_color`).
    pub fn claktory_color<'a>(&'a self, default: &'a str) -> &'a str {
        if self.color.is_empty() { default } else { &self.color }
    }

    /// Where the file holds settings this version doesn't know
    /// (`Unknown`), such as "display.pane_rules" or "agents[ed].notify", to
    /// log.
    pub fn unknown_keys(&self) -> Vec<String> {
        let mut keys = Vec::new();
        let mut add = |at: &str, other: &Unknown| {
            keys.extend(other.keys().map(|key| if at.is_empty() { key.clone() } else { format!("{at}.{key}") }))
        };
        add("", &self.other);
        add("account", &self.account.other);
        add("compaction", &self.compaction.other);
        add("vault", &self.vault.other);
        add("user", &self.user.other);
        add("user.avatar", &self.user.avatar.other);
        add("display", &self.display.other);
        add("resources", &self.resources.other);
        if let Some(remote) = &self.remote {
            add("remote", &remote.other);
        }
        for project in &self.projects {
            add(&format!("projects[{}]", project.id), &project.other);
            for folder in &project.folders {
                add(&format!("projects[{}].folders[{}]", project.id, folder.id), &folder.other);
            }
        }
        for agent in &self.agents {
            add(&format!("agents[{}]", agent.id), &agent.other);
            add(&format!("agents[{}].avatar", agent.id), &agent.avatar.other);
        }
        for app in &self.apps {
            add(&format!("apps[{}]", app.id), &app.other);
        }
        keys
    }

    /// The user's name, when it's set.
    pub fn user_name(&self) -> Option<&str> {
        Some(self.user.name.trim()).filter(|name| !name.is_empty())
    }

    /// How agents and Factory's texts call the user: their name, or "the
    /// person" when it isn't set.
    pub fn person(&self) -> &str {
        self.user_name().unwrap_or("the person")
    }

    /// `person` at the start of a sentence: their name, or "The person".
    pub fn person_capitalized(&self) -> &str {
        self.user_name().unwrap_or("The person")
    }

    /// The settings as written at `root`, in the new form: a project or
    /// folder path inside the root (Claktory's own territory) is emitted
    /// relative to it, so the file stays true when the home moves to
    /// another machine; paths outside stay absolute. A backend from before
    /// relative forms can't read a file that holds one, so only a move
    /// (`move_import`), into a home with no older backend to go back to,
    /// writes the file this way; every other write keeps the file's form
    /// (`to_json_keeping_form`).
    pub fn to_json(&self, root: &Path) -> Result<String, serde_json::Error> {
        let mut config = self.clone();
        for project in &mut config.projects {
            relativize(&mut project.path, root);
            for folder in &mut project.folders {
                relativize(&mut folder.path, root);
            }
        }
        let mut text = serde_json::to_string_pretty(&config)?;
        text.push('\n');
        Ok(text)
    }

    /// The settings in the form `read`, the file as it is now, has: the new
    /// form (`to_json`) once it writes any path relative (a move put it
    /// there, or the person did), else every path absolute, as every
    /// version reads it. The supervisor can hand the file back to the
    /// backend before an update, during the update's trial or any time
    /// after when the new one keeps failing to start, so no write of
    /// Claktory's, nor the person's settings changes, may move a file an
    /// older backend reads into a form it can't.
    pub fn to_json_keeping_form(&self, root: &Path, read: &str) -> Result<String, serde_json::Error> {
        if any_relative(read) {
            return self.to_json(root);
        }
        let mut text = serde_json::to_string_pretty(self)?;
        text.push('\n');
        Ok(text)
    }

    pub fn validate(&self) -> Result<(), Invalid> {
        if self.compact_at_tokens == 0 {
            return Err(Invalid::ZeroCompaction);
        }
        if self.task_alert_hours == 0 {
            return Err(Invalid::ZeroTaskAlert);
        }
        if let Some(remote) = &self.remote {
            remote.check()?;
        }
        self.resources.check()?;
        self.display.check()?;
        if !(1..=60).contains(&self.vault.unlock_minutes) {
            return Err(Invalid::BadVault);
        }
        if !self.account.service.is_empty() && crate::account::service_url(&self.account.service).is_none() {
            return Err(Invalid::BadAccountService(self.account.service.clone()));
        }
        for (key, ttl) in &self.prompt_cache {
            let provider = key.split_once('/').map_or(key.as_str(), |(provider, model)| if model.is_empty() { "" } else { provider });
            if !CACHE_TTL_PROVIDERS.contains(&provider) {
                return Err(Invalid::BadPromptCache(key.clone()));
            }
            if !cache_ttl_choices(provider).contains(ttl) {
                return Err(Invalid::BadPromptCacheTtl(key.clone(), ttl.as_str().to_owned(), choices_text(provider)));
            }
        }
        check_model(&self.default_model)?;
        check_effort(&self.default_effort)?;
        if !self.default_fallback_model.is_empty() {
            check_model(&self.default_fallback_model)?;
        }
        let mut ids = HashSet::new();
        for project in &self.projects {
            check_entry(&project.id, &project.name, &mut ids)?;
            if !project.path.is_absolute() {
                return Err(Invalid::RelativePath(project.id.clone()));
            }
            let mut folder_ids = HashSet::new();
            for folder in &project.folders {
                check_entry(&folder.id, &folder.name, &mut folder_ids)?;
                if !folder.path.is_absolute() {
                    return Err(Invalid::RelativeFolder { project: project.id.clone(), folder: folder.id.clone() });
                }
            }
        }
        if self.name.chars().any(char::is_control) {
            return Err(Invalid::BadName(self.name.clone()));
        }
        if !self.color.is_empty() && !AGENT_COLORS.contains(&self.color.as_str()) {
            return Err(Invalid::BadColor(self.color.clone()));
        }
        let mut agent_ids = HashSet::new();
        for agent in &self.agents {
            check_entry(&agent.id, &agent.name, &mut agent_ids)?;
            if let Some(project) = &agent.project
                && self.project(project).is_none()
            {
                return Err(Invalid::UnknownProject { agent: agent.id.clone(), project: project.clone() });
            }
            check_model(&agent.model)?;
            check_effort(&agent.effort)?;
            if !agent.fallback_model.is_empty() {
                check_model(&agent.fallback_model)?;
            }
            if !agent.color.is_empty() && !AGENT_COLORS.contains(&agent.color.as_str()) {
                return Err(Invalid::BadColor(agent.color.clone()));
            }
            crate::extensions::validate(agent)?;
        }
        Ok(())
    }

    pub fn project(&self, id: &str) -> Option<&ProjectConfig> {
        self.projects.iter().find(|project| project.id == id)
    }

    pub fn agent(&self, id: &str) -> Option<&AgentConfig> {
        self.agents.iter().find(|agent| agent.id == id)
    }

    pub fn agent_mut(&mut self, id: &str) -> Option<&mut AgentConfig> {
        self.agents.iter_mut().find(|agent| agent.id == id)
    }

    /// The agent `agent` reports to: its `lead`, when that's another agent,
    /// in its project or an admin agent, whose own leads don't come back to
    /// it. So leads never make a loop.
    pub fn lead(&self, agent: &AgentConfig) -> Option<&AgentConfig> {
        let lead = self.lead_as_written(agent)?;
        let mut seen = HashSet::new();
        let mut next = Some(lead);
        while let Some(current) = next {
            if current.id == agent.id {
                return None;
            }
            if !seen.insert(current.id.as_str()) {
                // A loop further up, which doesn't go through `agent`.
                break;
            }
            next = self.lead_as_written(current);
        }
        Some(lead)
    }

    /// `agent`'s `lead`, if it's another agent in its project or an admin agent.
    fn lead_as_written(&self, agent: &AgentConfig) -> Option<&AgentConfig> {
        let lead = self.agent(agent.lead.as_deref()?)?;
        (lead.id != agent.id && (lead.project == agent.project || crate::proposals::is_admin(lead))).then_some(lead)
    }

    /// Whether `lead` leads `agent`: an admin agent leads every other one;
    /// any other agent, those that report to it, directly or through others.
    pub fn leads(&self, lead: &AgentConfig, agent: &AgentConfig) -> bool {
        if lead.id == agent.id {
            return false;
        }
        if crate::proposals::is_admin(lead) {
            return true;
        }
        let mut seen = HashSet::new();
        let mut next = self.lead(agent);
        while let Some(current) = next {
            if current.id == lead.id {
                return true;
            }
            if !seen.insert(current.id.as_str()) {
                return false;
            }
            next = self.lead(current);
        }
        false
    }

    /// Why `agent` can't report to `lead` (an id), if it can't: it must be
    /// another agent, in its project or an admin agent, and not one that
    /// reports to `agent`.
    pub fn lead_refused(&self, agent: &AgentConfig, lead: &str) -> Option<String> {
        let Some(lead) = self.agent(lead) else { return Some("That agent no longer exists.".to_owned()) };
        if lead.id == agent.id {
            return Some(format!("{} can't report to itself.", agent.name));
        }
        if lead.project != agent.project && !crate::proposals::is_admin(lead) {
            return Some(format!(
                "{} is in another project, so {} can't report to {}. Choose an agent in {}'s project, or an admin agent.",
                lead.name, agent.name, lead.name, agent.name
            ));
        }
        let mut seen = HashSet::new();
        let mut next = Some(lead);
        while let Some(current) = next {
            if current.id == agent.id {
                return Some(format!("{} reports to {}, so {} can't report to {}.", lead.name, agent.name, agent.name, lead.name));
            }
            if !seen.insert(current.id.as_str()) {
                break;
            }
            next = self.lead(current);
        }
        None
    }

    /// Leads that can't be any more, because the agent is gone or now in
    /// another project, are forgotten.
    pub fn forget_stale_leads(&mut self) {
        let stale: Vec<String> = self
            .agents
            .iter()
            .filter(|agent| agent.lead.as_deref().is_some_and(|lead| self.lead_as_written(agent).is_none_or(|found| found.id != lead)))
            .map(|agent| agent.id.clone())
            .collect();
        for agent in &mut self.agents {
            if stale.contains(&agent.id) {
                agent.lead = None;
            }
        }
    }

    /// The agent's project is archived: it doesn't run, and no agent can
    /// message it.
    pub fn archived(&self, agent: &AgentConfig) -> bool {
        agent.project.as_deref().and_then(|project| self.project(project)).is_some_and(|project| project.archived)
    }

    /// The project folder the agent works in, if it's in one its project has.
    pub fn agent_folder(&self, agent: &AgentConfig) -> Option<&FolderConfig> {
        let project = self.project(agent.project.as_deref()?)?;
        project.folder(&agent.folder)
    }
}

fn check_entry(id: &str, name: &str, seen: &mut HashSet<String>) -> Result<(), Invalid> {
    // Ids name folders on disk, so they must not be able to escape them.
    if id.is_empty() || !id.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-') {
        return Err(Invalid::BadId(id.to_owned()));
    }
    if !seen.insert(id.to_owned()) {
        return Err(Invalid::DuplicateId(id.to_owned()));
    }
    if name.trim().is_empty() {
        return Err(Invalid::EmptyName(id.to_owned()));
    }
    Ok(())
}

/// Whether `prompt` is a Rivet prompt some version of Claktory shipped.
fn shipped_default(prompt: &str) -> bool {
    use sha2::{Digest, Sha256};
    let hash = Sha256::digest(prompt.trim_end().as_bytes());
    let hex: String = hash.iter().map(|byte| format!("{byte:02x}")).collect();
    SHIPPED_RIVET_PROMPTS.contains(&hex.as_str())
}

pub fn check_model(model: &str) -> Result<(), Invalid> {
    match model.split_once('/') {
        Some((provider, id)) if !provider.is_empty() && !id.is_empty() => Ok(()),
        _ => Err(Invalid::BadModel(model.to_owned())),
    }
}

/// The colors an agent can have, in the palette's order.
pub const AGENT_COLORS: [&str; 8] = ["blue", "violet", "green", "pink", "teal", "amber", "red", "slate"];

/// How each of `AGENT_COLORS` looks, as the app draws it (`agentColors` in
/// app/lib/identity.dart).
const AGENT_RGB: [[u8; 3]; 8] = [
    [0x2F, 0x6F, 0xEB],
    [0x7B, 0x52, 0xE0],
    [0x1F, 0x85, 0x49],
    [0xC7, 0x35, 0x8A],
    [0x11, 0x80, 0x8A],
    [0xA8, 0x60, 0x09],
    [0xD0, 0x3C, 0x3C],
    [0x5A, 0x63, 0x77],
];

/// Providers whose requests can choose how long the prompt cache lasts, and
/// the lifetimes each offers, its default first.
pub const CACHE_TTL_PROVIDERS: &[&str] = &["anthropic", "openai"];

/// `provider`'s choices, as words: "5m or 1h".
fn choices_text(provider: &str) -> String {
    cache_ttl_choices(provider).iter().map(|ttl| ttl.as_str()).collect::<Vec<_>>().join(" or ")
}

/// The lifetimes `provider`'s cache can be asked to last; none for one whose
/// requests can't choose.
pub fn cache_ttl_choices(provider: &str) -> &'static [CacheTtl] {
    match provider {
        "anthropic" => &[CacheTtl::FiveMinutes, CacheTtl::OneHour],
        "openai" => &[CacheTtl::ThirtyMinutes, CacheTtl::TwelveHours],
        _ => &[],
    }
}

/// The choices the app offers, and UpdateSettings takes: OpenAI's only from
/// the release after the one that first reads them, so that a rollback never
/// meets a config.json it refuses (#281).
pub fn cache_ttl_offered(provider: &str) -> &'static [CacheTtl] {
    match provider {
        "openai" => &[],
        provider => cache_ttl_choices(provider),
    }
}

/// How long a cache is asked to last unless set (Matt's call): an hour, so
/// an idle conversation is compacted just before it expires (`crate::idle`)
/// rather than paying for the whole context again after 5 minutes. Only the
/// default: config.json holds nothing new, and a set "5m" still wins.
pub const DEFAULT_CACHE_TTL: CacheTtl = CacheTtl::OneHour;

/// OpenAI's own lifetime from GPT-5.6 on, which its requests ask for by
/// sending nothing: at least 30 minutes. The ChatGPT subscription (Codex)
/// lets no request choose, and keeps it as long (measured, #281).
pub const OPENAI_CACHE_TTL: CacheTtl = CacheTtl::ThirtyMinutes;

impl Config {
    /// How long the prompt cache of `model` ("provider/model id", or a
    /// provider alone) lasts: its own setting, its provider's, or the
    /// provider's default (`DEFAULT_CACHE_TTL` on Anthropic,
    /// `OPENAI_CACHE_TTL` on OpenAI's models that have one); None where
    /// Claktory can neither choose it nor know it.
    pub fn cache_ttl(&self, model: &str) -> Option<CacheTtl> {
        let (provider, id) = model.split_once('/').unwrap_or((model, ""));
        let set = || self.prompt_cache.get(model).or_else(|| self.prompt_cache.get(provider)).copied();
        // A model that keeps its cache as OpenAI likes: unknown.
        let known = id.is_empty() || factory_agent::providers::catalog::has_cache_lifetime(provider, id);
        match provider {
            "anthropic" => Some(set().unwrap_or(DEFAULT_CACHE_TTL)),
            "openai" if known => Some(set().unwrap_or(OPENAI_CACHE_TTL)),
            "openai-codex" if known => Some(OPENAI_CACHE_TTL),
            _ => None,
        }
    }

    /// The default prompt an agent can go back to, with its placeholders:
    /// Rivet's, as this version ships it. None for any other agent.
    pub fn default_prompt(agent: &AgentConfig) -> Option<&'static str> {
        (agent.id == RIVET).then(|| RIVET_PROMPT.trim_end())
    }

    /// Gives an agent whose prompt is still a default Claktory shipped (any
    /// version, trailing whitespace aside) this version's, so a prompt nobody
    /// edited follows the default; a prompt someone edited stays theirs. Says
    /// whether any changed, so the file is written back.
    pub fn follow_default_prompts(&mut self) -> bool {
        let mut changed = false;
        for agent in &mut self.agents {
            if let Some(default) = Config::default_prompt(agent)
                && agent.prompt != default
                && shipped_default(&agent.prompt)
            {
                agent.prompt = default.to_owned();
                changed = true;
            }
        }
        changed
    }

    /// Gives every agent without a color one, and says whether any needed
    /// it, so the file is written back. An agent keeps the color it had in
    /// `before`, the config this one replaces, and one new since gets
    /// `next_color`. With no `before` (the file read at start, maybe from
    /// before colors were saved) an agent gets the color its place in the
    /// list used to give it, so nothing looks different.
    pub fn keep_colors(&mut self, before: Option<&Config>) -> bool {
        let mut filled = false;
        for index in 0..self.agents.len() {
            if !self.agents[index].color.is_empty() {
                continue;
            }
            let color = match before {
                None => AGENT_COLORS[index % AGENT_COLORS.len()].to_owned(),
                Some(before) => match before.agent(&self.agents[index].id) {
                    Some(old) if !old.color.is_empty() => old.color.clone(),
                    _ => self.next_color().to_owned(),
                },
            };
            self.agents[index].color = color;
            filled = true;
        }
        filled
    }

    /// The color a new agent gets: the one that looks least like any color
    /// the agents have, so they're all as easy to tell apart as they can be.
    /// Once every color is taken, the one fewest agents have.
    pub fn next_color(&self) -> &'static str {
        let mut used = Vec::with_capacity(self.agents.len());
        for agent in &self.agents {
            if let Some(color) = AGENT_COLORS.iter().position(|color| *color == agent.color) {
                used.push(color);
            }
        }
        let mut best = (0, f32::MIN, usize::MAX);
        for (candidate, rgb) in AGENT_RGB.iter().enumerate() {
            // How far it is from the closest color in use, and how many have it.
            let mut nearest = f32::INFINITY;
            let mut uses = 0;
            for &color in &used {
                nearest = nearest.min(oklab_distance(*rgb, AGENT_RGB[color]));
                uses += usize::from(color == candidate);
            }
            let better = match nearest.total_cmp(&best.1) {
                std::cmp::Ordering::Greater => true,
                std::cmp::Ordering::Equal => uses < best.2,
                std::cmp::Ordering::Less => false,
            };
            if better {
                best = (candidate, nearest, uses);
            }
        }
        AGENT_COLORS[best.0]
    }
}

/// How different two colors look: their distance in Oklab, a color space
/// where equal distances look about equally different.
fn oklab_distance(a: [u8; 3], b: [u8; 3]) -> f32 {
    let (a, b) = (oklab(a), oklab(b));
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
}

fn oklab(rgb: [u8; 3]) -> [f32; 3] {
    let [r, g, b] = rgb.map(|channel| {
        let c = f32::from(channel) / 255.0;
        if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) }
    });
    let l = (0.412_221_46 * r + 0.536_332_55 * g + 0.051_445_995 * b).cbrt();
    let m = (0.211_903_5 * r + 0.680_699_5 * g + 0.107_396_96 * b).cbrt();
    let s = (0.088_302_46 * r + 0.281_718_85 * g + 0.629_978_7 * b).cbrt();
    [
        0.210_454_26 * l + 0.793_617_8 * m - 0.004_072_047 * s,
        1.977_998_5 * l - 2.428_592_2 * m + 0.450_593_7 * s,
        0.025_904_037 * l + 0.782_771_77 * m - 0.808_675_77 * s,
    ]
}

pub fn check_effort(effort: &str) -> Result<(), Invalid> {
    if EFFORTS.contains(&effort) { Ok(()) } else { Err(Invalid::BadEffort(effort.to_owned())) }
}

/// A readable id derived from `name` that `taken` doesn't already use:
/// "Release Notes" → "release-notes", then "release-notes-2", ...
/// An id for a new agent named `name` in `project`: its name ("frontend");
/// when an agent in another project has that id, the project's id and its
/// name ("web-site-frontend"); otherwise a number ("frontend-2").
/// `same_project` says whether the agent with an id is in `project`.
pub fn agent_id(name: &str, project: Option<&str>, taken: impl Fn(&str) -> bool, same_project: impl Fn(&str) -> bool) -> String {
    let base = unique_id(name, "agent", |_| false);
    if let Some(project) = project
        && taken(&base)
        && !same_project(&base)
    {
        let flavored = format!("{project}-{base}");
        if !taken(&flavored) {
            return flavored;
        }
    }
    unique_id(&base, "agent", taken)
}

pub fn unique_id(name: &str, fallback: &str, taken: impl Fn(&str) -> bool) -> String {
    let mut base = String::new();
    for c in name.trim().chars() {
        if c.is_ascii_alphanumeric() {
            base.push(c.to_ascii_lowercase());
        } else if !base.is_empty() && !base.ends_with('-') {
            base.push('-');
        }
    }
    while base.ends_with('-') {
        base.pop();
    }
    if base.is_empty() {
        base = fallback.to_owned();
    }
    if !taken(&base) {
        return base;
    }
    let mut n = 2;
    loop {
        let candidate = format!("{base}-{n}");
        if !taken(&candidate) {
            return candidate;
        }
        n += 1;
    }
}

/// UI state that belongs to the workspace rather than to a device (tenet 7).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct State {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selected_agent: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub focused_project: Option<String>,
    /// Each project's explorer, by project id, and each of its folders' by
    /// "<project id>/<folder id>".
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub explorer: std::collections::BTreeMap<String, FolderView>,
    /// Agents with a run that ended since the person last opened them.
    #[serde(default, skip_serializing_if = "std::collections::BTreeSet::is_empty")]
    pub unseen: std::collections::BTreeSet<String>,
    /// The extension whose panel is open beside each agent, by agent id.
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub open_panels: std::collections::BTreeMap<String, String>,
    /// Agents whose nested agents are folded away in the sidebar.
    #[serde(default, skip_serializing_if = "std::collections::BTreeSet::is_empty")]
    pub collapsed: std::collections::BTreeSet<String>,
    /// Project folders (project id, folder id) whose agents are folded away in the sidebar.
    #[serde(default, skip_serializing_if = "std::collections::BTreeSet::is_empty")]
    pub collapsed_folders: std::collections::BTreeSet<(String, String)>,
    /// The sidebar's Projects and Chats sections, folded away.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub projects_collapsed: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub chats_collapsed: bool,
    /// The most panes `show` keeps open besides the conversation (Settings
    /// → Display), 1 to `MAX_OPEN_PANES`; 0 for `DEFAULT_OPEN_PANES`. Kept
    /// here rather than in config.json, which a backend from before it
    /// would refuse whole after a rollback.
    #[serde(default, skip_serializing_if = "is_zero_count")]
    pub max_open_panes: u32,
}

/// How a project's explorer is opened up.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct FolderView {
    /// Expanded folders, relative to the project folder.
    #[serde(default, skip_serializing_if = "std::collections::BTreeSet::is_empty")]
    pub expanded: std::collections::BTreeSet<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub show_all: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// How long each model's cache lasts: Anthropic's an hour unless set;
    /// OpenAI's from GPT-5.6 on 30 minutes unless set, the subscription's
    /// always (it can't be asked for more), and earlier models' unknown.
    #[test]
    fn each_provider_s_cache_lifetime_is_known_or_set_or_none() {
        let mut config = Config::default();
        let ttl = |config: &Config, model: &str| config.cache_ttl(model).map(CacheTtl::as_str);
        assert_eq!(ttl(&config, "anthropic/claude-opus-4-8"), Some("1h"));
        assert_eq!(ttl(&config, "openai/gpt-5.6-sol"), Some("30m"));
        assert_eq!(ttl(&config, "openai"), Some("30m"));
        assert_eq!(ttl(&config, "openai/gpt-5.5"), None, "kept as OpenAI likes");
        assert_eq!(ttl(&config, "openai-codex/gpt-6.1-sol"), Some("30m"));
        assert_eq!(ttl(&config, "openai-codex"), Some("30m"));
        assert_eq!(ttl(&config, "openrouter/anthropic/claude-opus-4.5"), None);
        config.prompt_cache.insert("openai".into(), CacheTtl::TwelveHours);
        assert_eq!(ttl(&config, "openai/gpt-5.6-sol"), Some("12h"));
        assert_eq!(ttl(&config, "openai/gpt-5.5"), None);
        assert!(config.validate().is_ok());
        config.prompt_cache.insert("openai".into(), CacheTtl::OneHour);
        assert!(config.validate().is_err(), "an hour isn't OpenAI's");
        config.prompt_cache.clear();
        config.prompt_cache.insert("openai-codex".into(), CacheTtl::ThirtyMinutes);
        assert!(config.validate().is_err(), "the subscription can't choose");
        // This release offers no choice for OpenAI yet; it reads one.
        assert!(cache_ttl_offered("openai").is_empty());
        assert_eq!(cache_ttl_offered("anthropic"), [CacheTtl::FiveMinutes, CacheTtl::OneHour]);
    }

    /// A newer Claktory's settings, read after the supervisor went back to
    /// this one: every part of the file reads, and keeps them for when the
    /// newer one comes back.
    #[test]
    fn settings_this_version_doesnt_know_are_kept_wherever_they_are() {
        let root = Path::new("/tmp/claktory-home");
        let text = r#"{
            "pane_rules": {"max": 3},
            "display": {"sidebar_width": 300, "max_open_panes": 5},
            "account": {"enabled": false, "region": "eu"},
            "compaction": {"idle": true, "later": 1},
            "vault": {"unlock_minutes": 5, "touch_id": true},
            "resources": {"gpu_percent": 80},
            "remote": {"relays": [], "mesh": "on"},
            "user": {"name": "Matt", "avatar": {"seed": "ab", "hue": 3}},
            "projects": [{"id": "site", "name": "Site", "path": "/tmp/site", "pinned": true,
                          "folders": [{"id": "ui", "name": "UI", "path": "/tmp/site/ui", "color": "red"}]}],
            "agents": [{"id": "ed", "name": "Ed", "model": "anthropic/claude-sonnet-4-5", "effort": "off",
                        "purpose": "Old name for the prompt.", "notify": "always", "avatar": {"shape": "circle", "ring": 2}}]
        }"#;
        let config = Config::parse(text, root).expect("a newer Claktory's settings read");
        assert_eq!(config.agents[0].prompt, "Old name for the prompt.", "the old alias still reads");
        assert_eq!(config.display.sidebar_width, 300.0);
        let mut keys = config.unknown_keys();
        keys.sort();
        assert_eq!(
            keys,
            [
                "account.region",
                "agents[ed].avatar.ring",
                "agents[ed].notify",
                "compaction.later",
                "display.max_open_panes",
                "pane_rules",
                "projects[site].folders[ui].color",
                "projects[site].pinned",
                "remote.mesh",
                "resources.gpu_percent",
                "user.avatar.hue",
                "vault.touch_id",
            ]
        );
        // Written back as they were, whichever form the file takes.
        for written in [config.to_json(root).expect("json"), config.to_json_keeping_form(root, text).expect("json")] {
            let value: Value = serde_json::from_str(&written).expect("json");
            assert_eq!(value["pane_rules"]["max"], 3, "{written}");
            assert_eq!(value["display"]["max_open_panes"], 5);
            assert_eq!(value["vault"]["touch_id"], true);
            assert_eq!(value["remote"]["mesh"], "on");
            assert_eq!(value["user"]["avatar"]["hue"], 3);
            assert_eq!(value["projects"][0]["pinned"], true);
            assert_eq!(value["projects"][0]["folders"][0]["color"], "red");
            assert_eq!(value["agents"][0]["notify"], "always");
            assert_eq!(value["agents"][0]["avatar"]["ring"], 2);
            assert_eq!(Config::parse(&written, root).expect("reads again"), config);
        }
    }

    /// A changed rivet-prompt.md needs its hash in SHIPPED_RIVET_PROMPTS, or
    /// the next version wouldn't know it as a default.
    #[test]
    fn this_versions_rivet_prompt_is_a_shipped_default() {
        assert!(
            shipped_default(RIVET_PROMPT),
            "add rivet-prompt.md's SHA-256 (without trailing whitespace) to SHIPPED_RIVET_PROMPTS, keeping the others"
        );
        assert!(shipped_default(&format!("{}\n\n \t", RIVET_PROMPT.trim_end())), "trailing whitespace aside");
        assert!(!shipped_default(&format!("{} Be brief.", RIVET_PROMPT.trim_end())));
        assert!(!Config::default().follow_default_prompts(), "a new Claktory's is this version's");
    }

    /// The fallback-model fields (#225) are checked like any other model
    /// name, and empty means none.
    #[test]
    fn fallback_models_are_validated_and_optional() {
        let mut config = Config::default();
        assert!(config.validate().is_ok(), "empty fallbacks are fine");
        config.default_fallback_model = "not-a-model".into();
        assert!(config.validate().is_err());
        config.default_fallback_model = "anthropic/claude-haiku-4-5".into();
        assert!(config.validate().is_ok());
        config.agents[0].fallback_model = "nope".into();
        assert!(config.validate().is_err());
        config.agents[0].fallback_model = "anthropic/claude-sonnet-4-5".into();
        assert!(config.validate().is_ok());
    }
}
