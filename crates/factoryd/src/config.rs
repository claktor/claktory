//! Factory's settings: one plain JSON file that people, agents and the app all
//! edit. The backend watches it and applies valid changes immediately.

use std::collections::{BTreeMap, HashSet};
use std::path::PathBuf;

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
/// Rivet's prompt; the file's trailing newline isn't part of it.
const RIVET_PROMPT: &str = include_str!("rivet-prompt.md");
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

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    /// Agents' contexts are summarized when they reach this many tokens.
    #[serde(default = "default_compact_at")]
    pub compact_at_tokens: u64,
    /// Model for new agents that don't name one: "<provider>/<model id>".
    #[serde(default = "default_model")]
    pub default_model: String,
    #[serde(default = "default_effort")]
    pub default_effort: String,
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
}

/// The vault's settings. Which secrets exist and who may use them live in
/// the vault itself, never here: agents can edit this file.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct VaultConfig {
    /// Minutes a connection stays unlocked after the person gave the vault
    /// passphrase on it, 1 to 60.
    pub unlock_minutes: u32,
}

impl Default for VaultConfig {
    fn default() -> VaultConfig {
        VaultConfig { unlock_minutes: 5 }
    }
}

impl VaultConfig {
    fn is_default(&self) -> bool {
        *self == VaultConfig::default()
    }
}

/// The person agents work for.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct UserConfig {
    /// Their first name, which agents call them by; empty when not given.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub name: String,
}

impl UserConfig {
    fn is_default(&self) -> bool {
        *self == UserConfig::default()
    }
}

/// The widths the person dragged the sidebar and the right panel to, in
/// logical pixels; 0 for the app's default. The app keeps them in its range.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct DisplayConfig {
    #[serde(skip_serializing_if = "is_zero")]
    pub sidebar_width: f64,
    #[serde(skip_serializing_if = "is_zero")]
    pub panel_width: f64,
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
        Ok(())
    }
}

fn is_zero(value: &f64) -> bool {
    *value == 0.0
}

/// The Resources extension's alerts (`crate::extensions::resources`), and
/// the build caches' budget (`crate::extensions::tree::caches`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
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
}

const DEFAULT_BUILD_CACHE_GB: f64 = 25.0;

fn is_default_build_cache(gb: &f64) -> bool {
    *gb == DEFAULT_BUILD_CACHE_GB
}

impl Default for ResourcesConfig {
    fn default() -> ResourcesConfig {
        ResourcesConfig { ram_percent: 90, disk_free_gb: 10, disk_free_percent: 10, cpu_percent: 0, build_cache_gb: DEFAULT_BUILD_CACHE_GB }
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
#[serde(deny_unknown_fields)]
pub struct RemoteConfig {
    #[serde(default, skip_serializing_if = "is_zero_port")]
    pub port: u16,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub listen: String,
    /// The relays devices reach Factory through when there's no direct path,
    /// as URLs; empty for the default ones (n0's).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub relays: Vec<String>,
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
#[serde(deny_unknown_fields)]
pub struct ProjectConfig {
    pub id: String,
    pub name: String,
    pub path: PathBuf,
    /// Folders some of its agents work in, in the person's order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub folders: Vec<FolderConfig>,
}

/// A folder inside a project, where some of its agents work. It only scopes
/// where they work: messaging and everything else stay project-wide.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FolderConfig {
    /// Unique within the project.
    pub id: String,
    pub name: String,
    /// Absolute; anywhere, such as a repository outside the workspace.
    pub path: PathBuf,
}

impl ProjectConfig {
    pub fn folder(&self, id: &str) -> Option<&FolderConfig> {
        self.folders.iter().find(|folder| folder.id == id)
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
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
    pub name: String,
    /// What it does ("Coordinator"), shown with its name; empty for none.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub role: String,
    /// Instructions added to the agent's system prompt. Older configs call it "purpose".
    #[serde(default, alias = "purpose")]
    pub prompt: String,
    pub model: String,
    pub effort: String,
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
            name: "Rivet".to_owned(),
            role: RIVET_ROLE.to_owned(),
            prompt: RIVET_PROMPT.trim_end_matches('\n').to_owned(),
            model: default_model(),
            effort: RIVET_EFFORT.to_owned(),
            color: AGENT_COLORS[0].to_owned(),
            placeholder: RIVET_PLACEHOLDER.to_owned(),
            folder: String::new(),
            extensions: rivet_extensions(),
            avatar: Avatar { shape: "hexagon".to_owned(), face: String::new() },
        };
        Config {
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
    #[error("color \"{0}\" must be one of blue, violet, green, pink, teal, amber, red, slate")]
    BadColor(String),
    #[error("agent \"{agent}\": extensions setting \"{setting}\" must be {expected}")]
    BadExtensionSetting { agent: String, setting: String, expected: &'static str },
}

impl Config {
    pub fn parse(text: &str) -> Result<Config, ConfigError> {
        let config: Config = serde_json::from_str(text)?;
        config.validate()?;
        Ok(config)
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

    pub fn to_json(&self) -> Result<String, serde_json::Error> {
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
        check_model(&self.default_model)?;
        check_effort(&self.default_effort)?;
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

impl Config {
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
