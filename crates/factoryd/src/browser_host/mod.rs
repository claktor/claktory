//! Claktory's browser, factoryd's side (#435 P4, agent-browser.md Rev 2,
//! SPLIT.md): run by the browser runner (`crate::browser_runner`, C1) as
//! its own user on a Linux server, in a small VM on a Mac (`mac`), or, on a
//! Mac dev backend, by a stand-in over a local headless Chrome (`standin`).
//!
//! Each agent with the Browser extension has a browser of its own: its own profile, so one agent's sign-ins are never
//! another's, and its own Chrome, started when it browses and closed after
//! 10 minutes unused, only while this computer's memory has room for it
//! (`memory`). Its profile is deleted when the agent is, or when its
//! Browser extension is turned off (`Host::config_changed`, and a sweep at
//! start of the agents listed in `<home>/browser-profiles.json`). The
//! person's own browser (no agent: the live view's Open) keeps the profile
//! every agent shared before.
//!
//! The person sees it only when they ask (Show me, Watch, Take over,
//! Open): its frames, and what's copied in it, go only to the one
//! connection watching (`Port`), never in the Workspace, to agents or
//! pipes; input counts only from that connection, only during the person's
//! turn (PERSONS_TURN, TAKEN_OVER, BROWSING). Whatever the runner says is a
//! hostile page's after a Chrome escape: capped and checked before it
//! reaches anything (Fugo 36537, 36598).
//!
//! On in release builds (#435 W12), off in debug builds and tests unless
//! FACTORY_AGENT_BROWSER=1; FACTORY_AGENT_BROWSER=0 turns it off anywhere.
//! Off, the Workspace leaves it unset and every request is refused.

pub(crate) mod confine;
pub(crate) mod downloads;
mod image;
pub(crate) mod linux;
pub(crate) mod locale;
pub(crate) mod mac;
mod memory;
mod profiles;
pub(crate) mod setup;
pub(crate) mod signout;
pub(crate) mod sites;
pub(crate) mod standin;
pub(crate) mod tab_view;
pub(crate) mod uploads;
pub(crate) mod view;

use std::collections::{BTreeMap, HashMap};
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, Weak};
use std::time::{Duration, Instant};

use factory_protocol::v1::{self, AgentBrowserState as State, server_message::Body};
use tokio::sync::{mpsc, watch};
use tokio_util::sync::{CancellationToken, DropGuard};

use crate::browser_runner::wire::{self, Channel, Control, Mode, Report};
use crate::factory::{Factory, RequestError};
use crate::transcript::now_ms;

/// How long it stays open with no one using it (10 minutes, since each
/// browsing agent has a Chrome of its own and memory to give back).
const WARM: Duration = Duration::from_secs(10 * 60);
/// How long a graceful stop may take before the runner is closed.
const STOP_WAIT: Duration = Duration::from_secs(10);
/// How long a runner let go of may take to end (its stdin closed, SIGTERM
/// 2 s later, SIGKILL 2 s after that) before another may start anyway.
const GONE_WAIT: Duration = Duration::from_secs(10);
/// The desktop's screen (Chrome's window maximised on it, Melone 37116).
const DESKTOP: (u16, u16) = (1920, 1080);
/// A connection's queue: frames go one at a time, so a few suffice.
const QUEUE: usize = 8;
/// How long an errand waits for its driven start to be ready.
const STARTING: Duration = Duration::from_secs(60);

/// Turned on by a test, whatever the environment says.
static TURNED_ON: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Whether Claktory's browser is on here: in a release build unless
/// FACTORY_AGENT_BROWSER=0, in a debug build (the dev backend, tests) only
/// with FACTORY_AGENT_BROWSER=1 (#435 W12). Whether it can run on this
/// computer is setup's to say (`setup`), on the browser's card.
pub(crate) fn enabled() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    TURNED_ON.load(Ordering::Relaxed)
        || *ON.get_or_init(|| on_by(std::env::var("FACTORY_AGENT_BROWSER").ok().as_deref(), cfg!(debug_assertions)))
}

/// `enabled`'s rule: what FACTORY_AGENT_BROWSER says, else on unless this
/// is a debug build.
fn on_by(said: Option<&str>, debug: bool) -> bool {
    match said {
        Some("1") => true,
        Some("0") => false,
        _ => !debug,
    }
}

/// Turned on by a test: the stand-in rather than the Mac's VM.
static STANDIN_ON: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Whether a Mac runs the dev stand-in over its own Chrome
/// (FACTORY_AGENT_BROWSER_STANDIN=1, the dev Claktory or a debug build
/// only: `standin` refuses otherwise) rather than the browser's VM (`mac`).
fn standin_asked() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    STANDIN_ON.load(Ordering::Relaxed) || *ON.get_or_init(|| std::env::var("FACTORY_AGENT_BROWSER_STANDIN").is_ok_and(|value| value == "1"))
}

/// Whether this Claktory runs the stand-in: asked for, and allowed here
/// (the dev Claktory or a debug build, as `standin` checks; Fugo 37166).
/// Asked for anywhere else it's ignored, so the real browser runs and its
/// card says how it is, never Off with nothing said (Abbacchio 1782).
pub(crate) fn standin_here(home: &Path) -> bool {
    standin_asked() && (dev_home(home) || cfg!(debug_assertions))
}

/// The Mac's browser image for tests (pinned by digest, as any).
#[doc(hidden)]
pub fn mac_image_for_test(image: String) {
    mac::image_for_test(image);
}

/// The Mac's VMs for tests: `options` before the image, `arguments`
/// after (a stand-in for the runner).
#[doc(hidden)]
pub fn mac_run_for_test(options: Vec<String>, arguments: Vec<String>) {
    mac::run_for_test(options, arguments);
}

/// Turns the stand-in on for tests.
#[doc(hidden)]
pub fn standin_for_test() {
    STANDIN_ON.store(true, Ordering::Relaxed);
}

/// Turns it on for tests.
#[doc(hidden)]
pub fn enable_for_test() {
    TURNED_ON.store(true, Ordering::Relaxed);
}

/// A runner's two directions, at C1's level: payloads by channel. `quit`
/// cancelled ends it (the Linux runner sees its stdin end, and is killed
/// 2 s later), however many senders are still about; so does the last
/// sender closing. The receiver ends when it has.
pub(crate) struct Io {
    pub(crate) to: mpsc::Sender<(Channel, Vec<u8>)>,
    pub(crate) from: mpsc::Receiver<(Channel, Vec<u8>)>,
    pub(crate) quit: CancellationToken,
}

/// Claktory's browsers: one per agent that browses, made when
/// it's first used, and the person's own (no agent).
pub(crate) struct Host {
    browsers: Mutex<BTreeMap<String, Arc<One>>>,
    next_port: AtomicU64,
    /// Every connection's slot, by port id.
    ports: Mutex<HashMap<u64, Slot>>,
    /// The Factory, once it's installed (`install`), for what follows a
    /// change of settings.
    factory: std::sync::OnceLock<Weak<Factory>>,
}

impl Default for Host {
    fn default() -> Host {
        Host {
            browsers: Mutex::new(BTreeMap::new()),
            next_port: AtomicU64::new(1),
            ports: Mutex::new(HashMap::new()),
            factory: std::sync::OnceLock::new(),
        }
    }
}

/// One browser: an agent's, or the person's own (`agent` empty).
pub(crate) struct One {
    agent: String,
    inner: Mutex<Inner>,
    /// Its state, for its agent's browsing (C2) to follow.
    states: watch::Sender<State>,
    /// Waiting sign-outs are applied one run at a time.
    applying: tokio::sync::Mutex<()>,
}

impl One {
    fn new(agent: &str) -> One {
        One {
            agent: agent.to_owned(),
            inner: Mutex::new(Inner::default()),
            states: watch::channel(State::Off).0,
            applying: tokio::sync::Mutex::new(()),
        }
    }

    fn inner(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

struct Inner {
    state: State,
    /// The errand using it (P3 sets it), or empty.
    errand: String,
    /// The running runner's way in; None while off.
    runner: Option<mpsc::Sender<(Channel, Vec<u8>)>>,
    /// Ends the runner when dropped (Io's quit).
    quit: Option<DropGuard>,
    /// A runner let go of hasn't ended yet: no other starts on its
    /// profile meanwhile (Ghiaccio 38462).
    exiting: bool,
    /// Which runner this is: output from an older one is ignored.
    generation: u64,
    /// Its profile is being deleted: once the runner has gone, it's
    /// forgotten (and on a Mac its volume goes).
    deleting: bool,
    /// The runner said it deleted the profile (DataDeleted).
    data_deleted: bool,
    /// The connection watching, if one is.
    viewer: Option<Viewer>,
    /// The screen's size now.
    screen: (u16, u16),
    /// The frame sent to the viewer and not acked yet: its seq.
    in_flight: Option<u32>,
    /// The page shown, as Claktory read it.
    origin: String,
    title: String,
    text_field_focused: bool,
    sites: Vec<String>,
    /// The identity providers its profile is signed in to, by their
    /// session cookies, as last read.
    providers: Vec<String>,
    chrome: String,
    warm_until: i64,
    /// Why the runner itself refused to start (a setup problem it found:
    /// no firewall, no /proc of its own…), cleaned; shown on the setup
    /// card until the next start.
    refused: Option<String>,
    /// Input this second: when it began and how many events.
    input_window: (i64, u32),
    /// How Chrome runs now, once the runner said. The gates key on this.
    mode: Option<Mode>,
    /// How Chrome was last asked to run (`ask_mode`), which a report of
    /// another mode is on its way to: what a turn change decides by, so one
    /// right after another doesn't read a mode about to go.
    asked: Option<Mode>,
    /// The errand's CDP connection (C2): what Chrome says goes there, only
    /// while it's DRIVEN.
    cdp: Option<mpsc::Sender<Vec<u8>>>,
    /// The host's own CDP calls (sites.rs): the last id used, and what
    /// each one waiting is for.
    next_call: u64,
    calls: HashMap<u64, sites::Call>,
    /// When the page shown was last read.
    page_read: i64,
    /// The errand's sessions in Chrome, let go of when it stops driving.
    release: Vec<String>,
    /// The folder the runner's driven Chrome downloads into (R9), as its
    /// Ready said; None without downloads.
    download_dir: Option<String>,
    /// Its profile has no Safe Browsing lists yet, as the runner said:
    /// sign-ins and payments wait for them (`Handle::lists_missing`).
    lists_missing: bool,
    /// The downloads being read over channel 5, by GUID.
    reads: downloads::Reads,
    /// The errand's input lease (R11.3 #5), while one is held: its id and
    /// when it runs out. Input.* reaches the runner only under it, and the
    /// runner holds it too.
    lease: Option<(u64, Instant)>,
    next_lease: u64,
    /// The host's own calls whose answers are waited for (`sites::ask`).
    waiting: HashMap<u64, tokio::sync::oneshot::Sender<serde_json::Value>>,
    /// The sign-outs waiting (`signout`), read from their file when first
    /// needed.
    sign_outs: Option<Vec<signout::Waiting>>,
    /// Why the last driven start couldn't be set up (a waiting sign-out
    /// failed): its errand is told.
    bootstrap_failed: Option<String>,
    /// The uploads on their way to the runner, by id: each told where it
    /// was staged, or why not.
    uploads: HashMap<u64, tokio::sync::oneshot::Sender<Result<String, String>>>,
    next_upload: u64,
    /// The paths the runner staged uploads at since Chrome last started:
    /// the only files the errand's `DOM.setFileInputFiles` may name.
    staged: Vec<String>,
    /// The person's live views of it, and its open checks (`tab_view`).
    views: tab_view::Views,
}

impl Default for Inner {
    fn default() -> Inner {
        Inner {
            state: State::Off,
            errand: String::new(),
            runner: None,
            quit: None,
            exiting: false,
            generation: 0,
            deleting: false,
            data_deleted: false,
            viewer: None,
            screen: DESKTOP,
            in_flight: None,
            origin: String::new(),
            title: String::new(),
            text_field_focused: false,
            sites: Vec::new(),
            providers: Vec::new(),
            chrome: String::new(),
            warm_until: 0,
            refused: None,
            input_window: (0, 0),
            mode: None,
            asked: None,
            cdp: None,
            next_call: sites::HOST_IDS,
            calls: HashMap::new(),
            page_read: 0,
            release: Vec::new(),
            download_dir: None,
            lists_missing: false,
            reads: downloads::Reads::new(),
            lease: None,
            next_lease: 0,
            waiting: HashMap::new(),
            sign_outs: None,
            bootstrap_failed: None,
            uploads: HashMap::new(),
            next_upload: 0,
            staged: Vec::new(),
            views: tab_view::Views::default(),
        }
    }
}

struct Slot {
    device: String,
    sender: mpsc::Sender<Body>,
}

struct Viewer {
    port: u64,
    device: String,
    /// The watching connection's way out.
    sender: mpsc::Sender<Body>,
    max_width: u32,
    /// A phone's shape, in its points; (0, 0) for the desktop's.
    shape: (u32, u32),
}

/// Whether the person's input counts now.
fn persons_turn(state: State) -> bool {
    matches!(state, State::PersonsTurn | State::TakenOver | State::Browsing)
}

pub(super) fn refused(why: impl Into<String>) -> RequestError {
    RequestError::Command(why.into())
}

/// Whether `agent` has Claktory's browser: the Browser extension on.
pub(crate) fn browsing(agent: &crate::config::AgentConfig) -> bool {
    crate::extensions::enabled(agent, BROWSER_EXTENSION)
}

/// The Browser extension's id (`extensions::browser`).
const BROWSER_EXTENSION: &str = "browser";

impl Host {
    fn browsers(&self) -> MutexGuard<'_, BTreeMap<String, Arc<One>>> {
        self.browsers.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn ports(&self) -> MutexGuard<'_, HashMap<u64, Slot>> {
        self.ports.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// `agent`'s browser (the person's own when empty), made the first time.
    fn one(&self, agent: &str) -> Arc<One> {
        self.browsers().entry(agent.to_owned()).or_insert_with(|| Arc::new(One::new(agent))).clone()
    }

    /// `agent`'s browser, if it has been used since Claktory started.
    fn get(&self, agent: &str) -> Option<Arc<One>> {
        self.browsers().get(agent).cloned()
    }

    /// Every browser used since Claktory started.
    fn all(&self) -> Vec<Arc<One>> {
        self.browsers().values().cloned().collect()
    }

    /// What the Workspace shows of this computer's browser, whichever
    /// agent's: its setup, Chrome's version, the time zone and languages,
    /// and a summary of its state (UNAVAILABLE while setup is needed, else
    /// WARM while any agent's Chrome runs, else OFF); None while it's off
    /// here.
    pub(crate) fn view(&self, factory: &Factory, browser: &crate::config::BrowserConfig) -> Option<v1::AgentBrowser> {
        if !enabled() {
            return None;
        }
        let (time_zone, languages) = locale::shown(browser);
        let all = self.all();
        let (mut running, mut refused, mut chrome, mut viewer_device) = (false, None, String::new(), String::new());
        for one in &all {
            let inner = one.inner();
            running |= inner.runner.is_some();
            if refused.is_none() {
                refused.clone_from(&inner.refused);
            }
            if chrome.is_empty() {
                chrome.clone_from(&inner.chrome);
            }
            if let Some(viewer) = &inner.viewer {
                viewer_device.clone_from(&viewer.device);
            }
        }
        let setup =
            setup::cached(factory).or_else(|| refused.map(|problem| v1::AgentBrowserSetup { problem, ..Default::default() }));
        // The person's own browser as it is, when there is one; else a
        // summary of the agents'.
        let own = self.get("").map(|one| one.view()).unwrap_or_else(|| v1::AgentBrowser {
            state: State::Off.into(),
            ..Default::default()
        });
        let state = match (&setup, running, own.state()) {
            (_, _, own) if own != State::Off => own,
            (Some(_), false, _) => State::Unavailable,
            (_, true, _) => State::Warm,
            (None, false, _) => State::Off,
        };
        Some(v1::AgentBrowser {
            agent_id: String::new(),
            state: state.into(),
            setup,
            viewer_device,
            chrome_version: chrome,
            time_zone,
            languages,
            ..own
        })
    }

    /// Each agent's browser, one per agent with the Browser
    /// extension on, in the settings' agent order; none while it's off here.
    pub(crate) fn agent_views(&self, config: &crate::config::Config) -> Vec<v1::AgentBrowser> {
        if !enabled() {
            return Vec::new();
        }
        config
            .agents
            .iter()
            .filter(|agent| browsing(agent))
            .map(|agent| match self.get(&agent.id) {
                Some(one) => one.view(),
                None => v1::AgentBrowser { agent_id: agent.id.clone(), state: State::Off.into(), ..Default::default() },
            })
            .collect()
    }
}

impl One {
    /// What the Workspace shows of this agent's browser.
    fn view(&self) -> v1::AgentBrowser {
        let inner = self.inner();
        v1::AgentBrowser {
            agent_id: self.agent.clone(),
            state: if inner.state == State::Unavailable { State::Off } else { inner.state }.into(),
            errand_id: inner.errand.clone(),
            signed_in_sites: inner.sites.clone(),
            warm_until_ms: if inner.state == State::Warm { inner.warm_until } else { 0 },
            ..Default::default()
        }
    }
}

/// Sets the state, tells errands, and leaves the Workspace to the caller.
fn set_state(one: &One, inner: &mut Inner, state: State) {
    if inner.state != state {
        // Out of the agent's hands: its sessions in Chrome are let go (so
        // a fence it set on one no longer holds the person's page), and
        // its connection ends; it connects again once it drives again.
        if inner.state == State::Driven {
            sites::release(inner);
            inner.cdp = None;
            end_lease(inner);
        }
        inner.state = state;
        one.states.send_replace(state);
    }
}

/// Tells every client, from outside the host's lock.
fn changed(factory: &Factory) {
    factory.broadcast_workspace(&factory.lock());
}

/// Sends `control` to the running runner; false when there's none or its
/// queue is full.
fn control(inner: &Inner, control: &Control) -> bool {
    inner.runner.as_ref().is_some_and(|runner| runner.try_send((Channel::Control, wire::encode(control))).is_ok())
}

/// Asks the runner to run Chrome in `mode` (at `url`), noting it as asked.
fn ask_mode(inner: &mut Inner, mode: Mode, url: Option<String>) {
    inner.asked = Some(mode);
    control(inner, &Control::Mode { mode, url });
}

/// The mode Chrome runs in or is on its way to: the one last asked for,
/// else the one reported.
fn aimed(inner: &Inner) -> Option<Mode> {
    inner.asked.or(inner.mode)
}

/// The kept profile's name: one per Claktory home, so a QA
/// sandbox, a copy or a second Claktory under the same user never drives
/// another's signed-in profile: `c` and the home's id ("dev" for this
/// user's own dev Claktory), for the person's own browser; and one per
/// agent in it, that and `-a` with the agent's id (`agent_key`).
pub(crate) fn profile(home: &Path, agent: &str) -> String {
    let base = if dev_home(home) { "dev".to_owned() } else { format!("c{}", home_id(home)) };
    if agent.is_empty() { base } else { format!("{base}-{}", agent_key(agent)) }
}

/// An agent's key in its browser's names: `a` and the first 8 bytes of the
/// SHA-256 of its id, in hex, so any id makes a name the runner takes
/// ([a-z0-9-], at most 48: with the home's, 17 + 1 + 17) and a Mac VM's
/// name stays within a host name's 63.
pub(crate) fn agent_key(agent: &str) -> String {
    use sha2::Digest;
    let digest = sha2::Sha256::digest(agent.as_bytes());
    format!("a{}", crate::encoding::hex(&digest[..8]))
}

/// Whether `home` is this user's own dev Claktory (`~/.factory-dev`, the
/// home folder as the user database has it, never $HOME, which a sandbox
/// sets): only it keeps the dev names, and takes the dev image overrides.
pub(crate) fn dev_home(home: &Path) -> bool {
    account_home().is_some_and(|account| same_folder(home, &account.join(".factory-dev")))
}

/// A Claktory home's id for its browser: the first 8 bytes of
/// the SHA-256 of its canonical path, in hex.
pub(crate) fn home_id(home: &Path) -> String {
    use sha2::Digest;
    let canonical = std::fs::canonicalize(home).unwrap_or_else(|_| home.to_path_buf());
    let digest = sha2::Sha256::digest(canonical.as_os_str().as_encoded_bytes());
    crate::encoding::hex(&digest[..8])
}

fn same_folder(a: &Path, b: &Path) -> bool {
    let canonical = |path: &Path| std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    canonical(a) == canonical(b)
}

/// This user's home folder, as the user database names it.
#[cfg(unix)]
fn account_home() -> Option<std::path::PathBuf> {
    let mut buffer = vec![0_u8; 16 * 1024];
    // SAFETY: zeroed is a valid passwd (null pointers, zero ids).
    let mut entry: libc::passwd = unsafe { std::mem::zeroed() };
    let mut found: *mut libc::passwd = std::ptr::null_mut();
    // SAFETY: the buffer outlives the entry read from it, and found is
    // either null or points at entry.
    let result = unsafe { libc::getpwuid_r(libc::getuid(), &raw mut entry, buffer.as_mut_ptr().cast(), buffer.len(), &raw mut found) };
    if result != 0 || found.is_null() || entry.pw_dir.is_null() {
        return None;
    }
    // SAFETY: pw_dir points into buffer, a C string.
    let dir = unsafe { std::ffi::CStr::from_ptr(entry.pw_dir) }.to_string_lossy().into_owned();
    (!dir.is_empty()).then(|| std::path::PathBuf::from(dir))
}

#[cfg(not(unix))]
fn account_home() -> Option<std::path::PathBuf> {
    None
}

/// Starts the runner (state STARTING), Chrome in `mode` at `url`.
/// Starts it in `mode`, its pages in `locale`'s time zone and languages
/// (`locale::for_start`), once this computer has the memory for another
/// Chrome (`memory::room`).
fn start(
    factory: &Arc<Factory>,
    one: &Arc<One>,
    inner: &mut Inner,
    mode: Mode,
    url: Option<String>,
    locale: (Option<String>, Vec<String>),
) -> Result<(), RequestError> {
    if let Some(setup) = setup::problem(factory) {
        return Err(refused(setup.problem));
    }
    // The others' locks aren't waited for (one may be starting too): one
    // that's busy counts as open.
    let others = factory
        .agent_browser
        .all()
        .iter()
        .filter(|other| other.agent != one.agent)
        .filter(|other| match other.inner.try_lock() {
            Ok(inner) => inner.runner.is_some(),
            Err(std::sync::TryLockError::Poisoned(poisoned)) => poisoned.into_inner().runner.is_some(),
            Err(std::sync::TryLockError::WouldBlock) => true,
        })
        .count();
    memory::room(others).map_err(refused)?;
    // Listed before its profile is made, so it's deleted however the
    // agent goes (`sweep`).
    if !one.agent.is_empty() {
        profiles::add(factory.home().root(), &one.agent).map_err(|error| refused(format!("Claktory couldn't note its browser: {error}")))?;
    }
    spawn(factory, one, inner)?;
    set_state(one, inner, State::Starting);
    let (time_zone, languages) = locale;
    let profile = profile(factory.home().root(), &one.agent);
    let start = Control::Start { profile, mode, url, width: DESKTOP.0, height: DESKTOP.1, time_zone, languages };
    inner.asked = Some(mode);
    control(inner, &start);
    Ok(())
}

/// Starts a runner, with nothing up in it yet.
fn spawn(factory: &Arc<Factory>, one: &Arc<One>, inner: &mut Inner) -> Result<(), RequestError> {
    if inner.exiting {
        return Err(refused("Claktory's browser is still closing: try again in a moment."));
    }
    let io = if cfg!(target_os = "linux") {
        linux::spawn(factory)
    } else if standin_here(factory.home().root()) {
        standin::spawn(factory, &one.agent)
    } else {
        mac::spawn(factory, &one.agent)
    }
    .map_err(refused)?;
    inner.refused = None;
    inner.generation += 1;
    inner.lease = None;
    inner.runner = Some(io.to);
    inner.quit = Some(io.quit.drop_guard());
    inner.screen = DESKTOP;
    tokio::spawn(read(Arc::downgrade(factory), one.clone(), inner.generation, io.from));
    Ok(())
}

/// What the runner says, until it ends.
async fn read(factory: Weak<Factory>, one: Arc<One>, generation: u64, mut from: mpsc::Receiver<(Channel, Vec<u8>)>) {
    while let Some((channel, payload)) = from.recv().await {
        let Some(factory) = factory.upgrade() else { return };
        let current = {
            let inner = one.inner();
            if inner.generation != generation {
                return;
            }
            inner.runner.is_some()
        };
        // One let go of says nothing more: it's only waited for, to end.
        if !current {
            continue;
        }
        // A download's bytes: to its read, waited for (the runner waits
        // meanwhile), only if factoryd asked for it.
        if channel == Channel::Files {
            downloads::chunk(&one, payload).await;
            continue;
        }
        output(&factory, &one, channel, payload);
    }
    if let Some(factory) = factory.upgrade() {
        gone(&factory, &one, generation);
    }
}

/// The runner of `generation` has ended (its output closed): another may
/// start, and the host is off if it wasn't yet. A profile being deleted is
/// forgotten now.
fn gone(factory: &Arc<Factory>, one: &Arc<One>, generation: u64) {
    let (deleted, confirmed) = {
        let mut inner = one.inner();
        if inner.generation == generation {
            inner.exiting = false;
            (std::mem::take(&mut inner.deleting), std::mem::take(&mut inner.data_deleted))
        } else {
            (false, false)
        }
    };
    ended(factory, one, generation, "Chrome closed unexpectedly.");
    if deleted {
        forget(factory, one, confirmed);
    }
}

/// The runner is gone: off, its errand told through the state.
fn ended(factory: &Factory, one: &One, generation: u64, why: &str) {
    {
        let mut inner = one.inner();
        if inner.generation != generation || inner.state == State::Off {
            return;
        }
        if inner.state == State::Starting && inner.refused.is_none() {
            // It ended as it started, saying nothing on C1 (sudo refused,
            // the Mac's VM couldn't load its firewall): a setup problem.
            inner.refused = Some("Claktory's browser stopped as it started. Check its setup, then try again.".to_owned());
        }
        if inner.state != State::Stopping {
            tracing::warn!(why, "Claktory's browser ended");
        }
        inner.runner = None;
        inner.quit = None;
        inner.in_flight = None;
        inner.warm_until = 0;
        inner.mode = None;
        inner.asked = None;
        inner.cdp = None;
        inner.lease = None;
        inner.waiting.clear();
        inner.uploads.clear();
        inner.staged.clear();
        // Reads under way end, unread.
        inner.reads.clear();
        inner.download_dir = None;
        set_state(one, &mut inner, State::Off);
    }
    changed(factory);
}

/// One payload from the runner: hostile until checked.
fn output(factory: &Arc<Factory>, one: &Arc<One>, channel: Channel, payload: Vec<u8>) {
    match channel {
        Channel::Control => match wire::decode::<Report>(&payload) {
            Some(report) => reported(factory, one, report),
            None => tracing::warn!(bytes = payload.len(), "Claktory's browser sent a report it couldn't read"),
        },
        Channel::Frames => view::frame(one, &payload),
        Channel::Clipboard => view::clipboard(one, payload),
        // To the errand's connection (C2), only while its agent drives.
        Channel::Cdp => {
            // The host's own answers (sites.rs) never reach the errand.
            if sites::answer(factory, one, &payload) {
                return;
            }
            // What comes on the live views' own sessions never does either.
            if tab_view::event(factory, one, &payload) {
                return;
            }
            let inner = one.inner();
            if inner.state == State::Driven
                && let Some(cdp) = &inner.cdp
            {
                let _ = cdp.try_send(payload);
            }
        }
        // Downloads (R9) aren't read yet: a file is taken only once
        // factoryd asked for it (download_read), and it never does yet.
        // Channel 5 goes to `downloads::chunk` before this.
        Channel::Input | Channel::Files => {}
    }
}

fn reported(factory: &Arc<Factory>, one: &Arc<One>, report: Report) {
    let mut stop = false;
    {
        let mut inner = one.inner();
        match report {
            Report::Ready { wire: version, chrome, mode, download_dir, safe_browsing, .. } => {
                inner.lists_missing = safe_browsing == Some(false);
                // Only a plain absolute folder (the errand hands it to
                // Chrome, inside the runner's fence).
                inner.download_dir =
                    download_dir.filter(|dir| dir.starts_with('/') && dir.len() <= 1024 && view::cleaned(dir, 1024) == *dir);
                if version != wire::VERSION {
                    tracing::warn!(version, "Claktory's browser runner speaks another wire: stopped");
                    stop = true;
                } else {
                    // The bare version, as the protocol says ("158.0.8127.88"),
                    // whether the runner said Chrome's product ("Chrome/…").
                    inner.chrome = view::cleaned(chrome.rsplit('/').next().unwrap_or_default(), 40);
                    set_mode(&mut inner, mode);
                    settle(factory, one, &mut inner, mode);
                    sites::read_sites(&mut inner);
                }
            }
            Report::ModeChanged { mode } => {
                set_mode(&mut inner, mode);
                settle(factory, one, &mut inner, mode);
                // Back from the person's turn: what they signed in to.
                sites::read_sites(&mut inner);
            }
            Report::SafeBrowsingReady => inner.lists_missing = false,
            Report::Resized { width, height } => inner.screen = (width.clamp(1, 4096), height.clamp(1, 4096)),
            Report::ChromeExited { .. } => stop = true,
            Report::DataDeleted => {
                inner.data_deleted = true;
                inner.sites.clear();
                inner.providers.clear();
                // Nothing left to sign out of.
                inner.sign_outs = Some(Vec::new());
                if let Err(error) = signout::save(&sign_outs_file(factory, one), &[]) {
                    tracing::warn!(%error, "the browser's waiting sign-outs weren't cleared after its data was deleted");
                }
            }
            Report::Stopped { .. } => {}
            // Downloads (R9): not asked for yet; the runner deletes its own.
            // A hint only: factoryd counts and hashes what it reads itself.
            Report::DownloadDone { .. } => {}
            Report::DownloadCancelled { guid, why } => downloads::cancelled(&mut inner, &guid, &why),
            // Input leases (R11): the runner ended one, or couldn't grant
            // it (Chrome wasn't driven): the host's ends with it.
            Report::Lease { id, granted, why } => {
                if !granted && inner.lease.is_some_and(|(held, _)| held == id) {
                    tracing::warn!(why = %view::cleaned(&why, 200), "Claktory's browser didn't grant an input lease");
                    inner.lease = None;
                }
            }
            Report::LeaseEnded { id, .. } => {
                if inner.lease.is_some_and(|(held, _)| held == id) {
                    inner.lease = None;
                }
            }
            // Only a plain absolute path, as the runner staged it.
            Report::Uploaded { id, path } => {
                if let Some(told) = inner.uploads.remove(&id) {
                    let plain = path.starts_with('/') && path.len() <= 1024 && view::cleaned(&path, 1024) == path;
                    if plain {
                        inner.staged.push(path.clone());
                        let over = inner.staged.len().saturating_sub(wire::MAX_STAGED);
                        inner.staged.drain(..over);
                    }
                    let _ = told.send(if plain { Ok(path) } else { Err("Claktory's browser staged the file somewhere it can't be given from.".to_owned()) });
                }
            }
            Report::UploadFailed { id, why } => {
                if let Some(told) = inner.uploads.remove(&id) {
                    let _ = told.send(Err(format!("Claktory's browser didn't take the file: {}.", view::cleaned(&why, 200))));
                }
            }
            Report::Error { what, fatal } => {
                let what = view::cleaned(&what, 300);
                tracing::warn!(%what, fatal, "Claktory's browser runner");
                // A start it refused is a setup problem: the card says so.
                if fatal && inner.state == State::Starting {
                    inner.refused = Some(what);
                }
                stop = fatal;
            }
        }
    }
    if stop {
        close(factory, one, "it stopped after an error");
    }
    changed(factory);
}

/// Chrome runs in `mode` now. Undriven, Claktory can't read its page, so
/// frames say nothing of it rather than what it was (Ghiaccio 38462).
fn set_mode(inner: &mut Inner, mode: Mode) {
    inner.mode = Some(mode);
    if mode == Mode::Undriven {
        inner.origin.clear();
        inner.title.clear();
    }
}

/// The state a runner settles in once Chrome runs in `mode`: the one a
/// request asked for (STARTING or a turn change keeps its aim).
fn settled(inner: &Inner, mode: Mode) -> State {
    match (inner.state, mode) {
        (State::Starting, Mode::Undriven) if inner.errand.is_empty() => State::Browsing,
        (State::Starting, Mode::Driven) if !inner.errand.is_empty() => State::Driven,
        (State::Starting, _) => State::Warm,
        (state, _) => state,
    }
}

/// Chrome runs in `mode` now, as Ready or ModeChanged said: Chrome started
/// again, so no lease of the one before holds. A driven start settles once
/// the sign-outs waiting are applied, before Chrome runs any page (it
/// starts on about:blank, W1); with none, at once.
fn settle(factory: &Arc<Factory>, one: &Arc<One>, inner: &mut Inner, mode: Mode) {
    inner.lease = None;
    // The runner deleted what it staged.
    inner.staged.clear();
    // A mode asked for before another that's on its way (a turn back right
    // after a turn): its state waits for that one.
    if inner.asked.is_some_and(|asked| asked != mode) {
        return;
    }
    let state = settled(inner, mode);
    if mode == Mode::Driven && inner.state == State::Starting && !waiting(factory, one, inner).is_empty() {
        inner.bootstrap_failed = None;
        tokio::spawn(bootstrap(factory.clone(), one.clone(), inner.generation, state));
        return;
    }
    if mode == Mode::Driven {
        inner.bootstrap_failed = None;
    }
    set_state(one, inner, state);
    after_turn_change(inner);
}

/// The sign-outs waiting (`signout`), read from their file the first time.
fn waiting<'a>(factory: &Factory, one: &One, inner: &'a mut Inner) -> &'a mut Vec<signout::Waiting> {
    let sites = inner.sites.clone();
    inner.sign_outs.get_or_insert_with(|| signout::load(&sign_outs_file(factory, one), || sites))
}

/// The file of `one`'s waiting sign-outs.
fn sign_outs_file(factory: &Factory, one: &One) -> std::path::PathBuf {
    let home = factory.home().root();
    signout::file(home, &one.agent, &profile(home, &one.agent))
}

/// Ends the errand's input lease, if one is held, at the runner too.
fn end_lease(inner: &mut Inner) {
    if let Some((id, _)) = inner.lease.take() {
        control(inner, &Control::EndLease { id });
    }
}

/// Whether the errand's input may reach Chrome now: driven, under a lease
/// not run out.
fn leased(inner: &Inner) -> bool {
    inner.state == State::Driven && inner.lease.is_some_and(|(_, until)| Instant::now() < until)
}

/// A driven start of runner `generation`, before it settles in `state`:
/// the waiting sign-outs applied, or, when one can't be, closed with why
/// (its errand hears it from `bootstrapped`).
async fn bootstrap(factory: Arc<Factory>, one: Arc<One>, generation: u64, state: State) {
    let (_, result) = apply(&factory, &one).await;
    let failed = {
        let mut inner = one.inner();
        if inner.generation != generation || inner.state != State::Starting {
            None
        } else if let Err(why) = &result {
            inner.bootstrap_failed = Some(format!("Claktory's browser couldn't finish signing out ({why}), so it didn't open a page."));
            Some(why.clone())
        } else {
            set_state(&one, &mut inner, state);
            after_turn_change(&inner);
            None
        }
    };
    if let Some(why) = failed {
        tracing::warn!(%why, "Claktory's browser closed: a waiting sign-out failed");
        close(&factory, &one, "a waiting sign-out failed");
    }
    changed(&factory);
}

/// Applies the sign-outs waiting, one at a time, each taken off the list
/// (and its file) once its cookies are gone: those applied, and the first
/// failure, which leaves it and those after it waiting.
async fn apply(factory: &Arc<Factory>, one: &Arc<One>) -> (Vec<signout::Waiting>, Result<(), String>) {
    let _applying = one.applying.lock().await;
    let all: Vec<signout::Waiting> = {
        let mut inner = one.inner();
        waiting(factory, one, &mut inner).clone()
    };
    let mut done = Vec::new();
    for sign_out in all {
        let what = if sign_out.site.is_empty() { sign_out.provider.clone() } else { sign_out.site.clone() };
        if let Err(why) = clear(one, &sign_out).await {
            return (done, Err(format!("signing out of {what}: {why}")));
        }
        let saved = {
            let mut inner = one.inner();
            let file = sign_outs_file(factory, one);
            let list = waiting(factory, one, &mut inner);
            list.retain(|waiting| waiting.provider != sign_out.provider || waiting.site != sign_out.site);
            signout::save(&file, list)
        };
        if let Err(error) = saved {
            tracing::warn!(%error, %what, "the browser signed out, but its waiting sign-outs weren't saved");
        }
        done.push(sign_out);
    }
    (done, Ok(()))
}

/// Deletes the cookies `sign_out` is for (a provider's, or a site's) and
/// clears its origins' storage, then checks none of them is left.
async fn clear(one: &One, sign_out: &signout::Waiting) -> Result<(), String> {
    let origins = signout::origins(sign_out);
    if origins.is_empty() {
        return Err("Claktory doesn't know where that is".to_owned());
    }
    let jar = ask(one, "Storage.getCookies", serde_json::json!({})).await?;
    let expired = signout::expired(&jar, sign_out);
    if !expired.is_empty() {
        ask(one, "Storage.setCookies", serde_json::json!({ "cookies": expired })).await?;
    }
    // Chrome clears an origin's storage only from a page's session (the
    // browser's own answers "Internal error"): a blank page of the host's.
    let made = ask(one, "Target.createTarget", serde_json::json!({"url": "about:blank", "background": true})).await?;
    let target = made["targetId"].as_str().ok_or("Chrome opened no page")?.to_owned();
    let cleared = async {
        let attached = ask(one, "Target.attachToTarget", serde_json::json!({"targetId": target, "flatten": true})).await?;
        let session = attached["sessionId"].as_str().ok_or("Chrome didn't attach to its page")?.to_owned();
        for origin in &origins {
            ask_in(one, &session, "Storage.clearDataForOrigin", serde_json::json!({"origin": origin, "storageTypes": "all"})).await?;
        }
        Ok::<(), String>(())
    }
    .await;
    let closed = ask(one, "Target.closeTarget", serde_json::json!({"targetId": target})).await;
    cleared?;
    closed?;
    let jar = ask(one, "Storage.getCookies", serde_json::json!({})).await?;
    match signout::left(&jar, sign_out) {
        0 => Ok(()),
        left => Err(format!("{left} of its cookies stayed")),
    }
}

/// One of the host's own CDP calls, browser-wide, answered: its result, or
/// why not (not driven, Chrome's error, no answer in 10 s).
async fn ask(one: &One, method: &str, params: serde_json::Value) -> Result<serde_json::Value, String> {
    ask_in(one, "", method, params).await
}

/// `ask`, in CDP session `session`.
async fn ask_in(one: &One, session: &str, method: &str, params: serde_json::Value) -> Result<serde_json::Value, String> {
    let answer = {
        let mut inner = one.inner();
        if inner.mode != Some(Mode::Driven) {
            return Err("Chrome isn't running driven".to_owned());
        }
        sites::ask(&mut inner, session, method, params).ok_or("Claktory's browser isn't open")?
    };
    match tokio::time::timeout(Duration::from_secs(10), answer).await {
        Ok(Ok(message)) => match message.get("error") {
            Some(error) => Err(view::cleaned(error["message"].as_str().unwrap_or("Chrome refused"), 200)),
            None => Ok(message["result"].clone()),
        },
        _ => Err("Chrome didn't answer".to_owned()),
    }
}

/// What follows a change of whose turn it is: the clipboard watched, and
/// the screen in the viewer's shape, only during the person's turn.
fn after_turn_change(inner: &Inner) {
    let turn = persons_turn(inner.state);
    control(inner, &Control::Clipboard { watch: turn && inner.viewer.is_some() });
    let wanted = match &inner.viewer {
        Some(viewer) if turn && viewer.shape.0 > 0 && viewer.shape.1 > 0 => {
            (viewer.shape.0.clamp(240, 2048) as u16, viewer.shape.1.clamp(240, 4096) as u16)
        }
        _ => DESKTOP,
    };
    if inner.screen != wanted {
        control(inner, &Control::Resize { width: wanted.0, height: wanted.1 });
    }
}

/// Lets go of the runner: its stdin ends, and it's killed 2 s later if it
/// hasn't gone. Until it has, none other starts; given up on after
/// GONE_WAIT.
fn let_go(factory: &Arc<Factory>, one: &Arc<One>, inner: &mut Inner) {
    inner.quit = None;
    if inner.runner.take().is_none() {
        return;
    }
    inner.exiting = true;
    let generation = inner.generation;
    let weak = Arc::downgrade(factory);
    let one = one.clone();
    tokio::spawn(async move {
        tokio::time::sleep(GONE_WAIT).await;
        if let Some(factory) = weak.upgrade()
            && let inner = one.inner()
            && inner.exiting
            && inner.generation == generation
        {
            drop(inner);
            tracing::warn!("Claktory's browser runner didn't end after it was let go of");
            gone(&factory, &one, generation);
        }
    });
}

/// Closes the runner at once (a fault, or a stop that took too long):
/// off now, whatever used it told it closed.
fn close(factory: &Arc<Factory>, one: &Arc<One>, why: &str) {
    let generation = {
        let mut inner = one.inner();
        let_go(factory, one, &mut inner);
        inner.generation
    };
    ended(factory, one, generation, why);
}

/// Quits Chrome gracefully (its cookies written), then closes the runner.
fn stop_gracefully(factory: &Arc<Factory>, one: &Arc<One>) {
    let generation = {
        let mut inner = one.inner();
        if inner.runner.is_none() {
            return;
        }
        control(&inner, &Control::Stop);
        set_state(one, &mut inner, State::Stopping);
        inner.generation
    };
    let weak = Arc::downgrade(factory);
    let one = one.clone();
    tokio::spawn(async move {
        tokio::time::sleep(STOP_WAIT).await;
        if let Some(factory) = weak.upgrade()
            && one.inner().generation == generation
        {
            close(&factory, &one, "it didn't stop in time");
        }
    });
}

/// WARM from now: it closes by itself 10 minutes after it was last used.
fn warm(factory: &Arc<Factory>, one: &Arc<One>, inner: &mut Inner) {
    set_state(one, inner, State::Warm);
    let until = now_ms() + WARM.as_millis() as i64;
    inner.warm_until = until;
    let weak = Arc::downgrade(factory);
    let one = one.clone();
    tokio::spawn(async move {
        tokio::time::sleep(WARM).await;
        let Some(factory) = weak.upgrade() else { return };
        let due = {
            let inner = one.inner();
            inner.state == State::Warm && inner.warm_until == until
        };
        if due {
            stop_gracefully(&factory, &one);
            changed(&factory);
        }
    });
}

/// One connection's part: what's sent to it alone. Made for every
/// connection; dropped with it, ending its view.
pub(crate) struct Port {
    id: u64,
    factory: Weak<Factory>,
    receiver: tokio::sync::Mutex<mpsc::Receiver<Body>>,
}

/// Whether `one` is open: a runner up, and neither closing nor closed.
fn open_now(one: &One) -> bool {
    let inner = one.inner();
    inner.runner.is_some() && !matches!(inner.state, State::Off | State::Stopping | State::Unavailable)
}

impl Port {
    pub(crate) fn new(factory: &Arc<Factory>, device: &str) -> Port {
        let host = &factory.agent_browser;
        let id = host.next_port.fetch_add(1, Ordering::Relaxed);
        let (sender, receiver) = mpsc::channel(QUEUE);
        host.ports().insert(id, Slot { device: device.to_owned(), sender });
        Port { id, factory: Arc::downgrade(factory), receiver: tokio::sync::Mutex::new(receiver) }
    }

    /// The next message for this connection alone.
    pub(crate) async fn next(&self) -> Body {
        match self.receiver.lock().await.recv().await {
            Some(body) => body,
            None => std::future::pending().await,
        }
    }

    fn factory(&self) -> Result<Arc<Factory>, RequestError> {
        let factory = self.factory.upgrade().ok_or_else(|| refused("Claktory is stopping."))?;
        if !enabled() {
            return Err(refused("Claktory's browser isn't available yet."));
        }
        Ok(factory)
    }

    /// The browser this connection watches, if it does.
    fn viewed(&self, factory: &Factory) -> Option<Arc<One>> {
        factory.agent_browser.all().into_iter().find(|one| one.inner().viewer.as_ref().is_some_and(|viewer| viewer.port == self.id))
    }

    /// WatchAgentBrowser: the browser in the person's turn, else the
    /// person's own, else any that's open (the live view is QA's).
    pub(crate) fn watch(&self, request: &v1::WatchAgentBrowser) -> Result<(), RequestError> {
        let factory = self.factory()?;
        if request.on {
            live_view(&factory)?;
            locale::offer(&factory, &request.device_time_zone, &request.device_languages);
        }
        let host = &factory.agent_browser;
        let before = self.viewed(&factory);
        if !request.on {
            if let Some(one) = before {
                unwatch(&mut one.inner());
                changed(&factory);
            }
            return Ok(());
        }
        let all = host.all();
        let shown = all
            .iter()
            .find(|one| persons_turn(one.inner().state) && open_now(one))
            .or_else(|| all.iter().find(|one| one.agent.is_empty() && open_now(one)))
            .or_else(|| all.iter().find(|one| open_now(one)))
            .cloned()
            .ok_or_else(|| refused("Claktory's browser isn't open. Open it first."))?;
        let (device, sender) = host
            .ports()
            .get(&self.id)
            .map(|slot| (slot.device.clone(), slot.sender.clone()))
            .ok_or_else(|| refused("This connection is closing."))?;
        if let Some(before) = before.filter(|before| before.agent != shown.agent) {
            unwatch(&mut before.inner());
        }
        {
            let mut inner = shown.inner();
            // The view moves: the connection that had it hears so first,
            // before this one's first frame (Formaggio 36591).
            if let Some(before) = inner.viewer.take().filter(|viewer| viewer.port != self.id) {
                let _ = before.sender.try_send(Body::AgentBrowserViewMoved(v1::AgentBrowserViewMoved { device: device.clone() }));
                // A turn that moves starts again: what was pasted or copied
                // on the device before is emptied (Fugo 36829).
                control(&inner, &Control::Clipboard { watch: false });
            }
            // A frame in flight to the one before is dropped: acked here, so
            // the runner sends the next to this one.
            if let Some(seq) = inner.in_flight.take()
                && let Some(runner) = &inner.runner
            {
                let _ = runner.try_send((Channel::Frames, wire::ack(seq).to_vec()));
            }
            let max_width = if request.max_width == 0 { 1280 } else { request.max_width.clamp(160, 4096) };
            inner.viewer =
                Some(Viewer { port: self.id, device, sender, max_width, shape: (request.screen_width, request.screen_height) });
            control(&inner, &Control::View { on: true, max_width: max_width as u16, quality_hint: 0 });
            after_turn_change(&inner);
        }
        changed(&factory);
        Ok(())
    }

    /// AgentBrowserFrameAck: from the viewer, for the frame in flight.
    pub(crate) fn ack(&self, request: &v1::AgentBrowserFrameAck) -> Result<(), RequestError> {
        let factory = self.factory()?;
        let Some(one) = self.viewed(&factory) else { return Ok(()) };
        let mut inner = one.inner();
        if inner.in_flight.is_some_and(|seq| u64::from(seq) == request.seq) {
            inner.in_flight = None;
            if let Some(runner) = &inner.runner {
                let _ = runner.try_send((Channel::Frames, wire::ack(request.seq as u32).to_vec()));
            }
        }
        Ok(())
    }

    /// AgentBrowserInput: from the viewer, during the person's turn only.
    pub(crate) fn input(&self, request: &v1::AgentBrowserInput) -> Result<(), RequestError> {
        let factory = self.factory()?;
        live_view(&factory)?;
        let one = self.viewed(&factory).ok_or_else(|| refused("Watch Claktory's browser here first."))?;
        let mut inner = one.inner();
        if !persons_turn(inner.state) {
            return Err(refused("It isn't your turn in Claktory's browser: take it over first."));
        }
        let now = now_ms();
        let payloads = view::input(&request.events, inner.screen).map_err(refused)?;
        if now - inner.input_window.0 >= 1000 {
            inner.input_window = (now, 0);
        }
        let room = view::EVENTS_PER_SECOND.saturating_sub(inner.input_window.1) as usize;
        let Some(runner) = inner.runner.clone() else { return Err(refused("Claktory's browser isn't open.")) };
        let taken = payloads.len().min(room);
        inner.input_window.1 += taken as u32;
        for payload in payloads.into_iter().take(taken) {
            if runner.try_send(payload).is_err() {
                break;
            }
        }
        Ok(())
    }

    /// TakeOverAgentBrowser: the agent browsing in it pauses.
    pub(crate) fn take_over(&self) -> Result<(), RequestError> {
        let factory = self.factory()?;
        live_view(&factory)?;
        let one = self.viewed(&factory).ok_or_else(|| refused("Watch Claktory's browser here first."))?;
        {
            let mut inner = one.inner();
            match inner.state {
                State::Driven => set_state(&one, &mut inner, State::TakenOver),
                State::PersonsTurn | State::TakenOver | State::Browsing => return Ok(()),
                _ => return Err(refused("No errand is using Claktory's browser now.")),
            }
            after_turn_change(&inner);
        }
        changed(&factory);
        Ok(())
    }

    /// HandBackAgentBrowser: back to the agent, or done browsing.
    pub(crate) fn hand_back(&self) -> Result<(), RequestError> {
        let factory = self.factory()?;
        live_view(&factory)?;
        let one = self.viewed(&factory).ok_or_else(|| refused("You haven't taken Claktory's browser over."))?;
        {
            let mut inner = one.inner();
            match inner.state {
                State::TakenOver => set_state(&one, &mut inner, State::Driven),
                State::Browsing => warm(&factory, &one, &mut inner),
                _ => return Err(refused("You haven't taken Claktory's browser over.")),
            }
            after_turn_change(&inner);
        }
        changed(&factory);
        Ok(())
    }

    /// StopAgentBrowser: `agent`'s browser (every one when empty) closes,
    /// its browsing ends, and its profile is kept. STOPPING until the
    /// runner has ended (its agent told the person stopped it), then OFF.
    pub(crate) fn stop(&self, agent: &str) -> Result<(), RequestError> {
        let factory = self.factory()?;
        let host = &factory.agent_browser;
        let open: Vec<Arc<One>> =
            host.all().into_iter().filter(|one| (agent.is_empty() || one.agent == agent) && one.inner().runner.is_some()).collect();
        if open.is_empty() {
            return Err(refused("Claktory's browser isn't open."));
        }
        for one in &open {
            stop_now(&factory, one);
        }
        changed(&factory);
        Ok(())
    }

    /// OpenAgentBrowser: the person's own, with no agent in it.
    pub(crate) fn open(&self, request: &v1::OpenAgentBrowser) -> Result<(), RequestError> {
        let factory = self.factory()?;
        live_view(&factory)?;
        let url = view::web_address(&request.url).map_err(refused)?;
        let browser = locale::offer(&factory, &request.device_time_zone, &request.device_languages);
        let one = factory.agent_browser.one("");
        {
            let mut inner = one.inner();
            if !inner.errand.is_empty() {
                return Err(refused("An errand is using Claktory's browser: watch it instead."));
            }
            match inner.state {
                State::Off | State::Unavailable => {
                    start(&factory, &one, &mut inner, Mode::Undriven, url, locale::for_start(&browser))?;
                }
                State::Warm => {
                    ask_mode(&mut inner, Mode::Undriven, url);
                    set_state(&one, &mut inner, State::Browsing);
                }
                State::Browsing => {
                    if url.is_some() {
                        ask_mode(&mut inner, Mode::Undriven, url);
                    }
                }
                _ => return Err(refused("Claktory's browser is busy: try again in a moment.")),
            }
        }
        changed(&factory);
        Ok(())
    }

    /// WatchAgentTab: a live view of an agent's browser, or of a check's tab.
    pub(crate) fn watch_tab(&self, request: &v1::WatchAgentTab) -> Result<(), RequestError> {
        tab_view::watch(&self.factory()?, self.id, request)
    }

    /// AgentTabFrameAck: from the connection watching.
    pub(crate) fn tab_ack(&self, request: &v1::AgentTabFrameAck) -> Result<(), RequestError> {
        let factory = self.factory()?;
        tab_view::ack(&factory, self.id, request);
        Ok(())
    }

    /// AgentTabInput: into a check's tab, from the connection watching it.
    pub(crate) fn tab_input(&self, request: &v1::AgentTabInput) -> Result<(), RequestError> {
        let factory = self.factory()?;
        tab_view::input(&factory, self.id, request)
    }

    /// DeleteAgentBrowserData: everything `agent`'s browser keeps, gone
    /// (every browser's when empty: the person's own and each agent's,
    /// running or not).
    pub(crate) fn delete_data(&self, agent: &str) -> Result<(), RequestError> {
        let factory = self.factory()?;
        let host = &factory.agent_browser;
        let mut agents: Vec<String> = if agent.is_empty() {
            let mut all: Vec<String> = host.browsers().keys().cloned().collect();
            all.extend(profiles::list(factory.home().root()));
            all.push(String::new());
            all
        } else {
            vec![agent.to_owned()]
        };
        agents.sort();
        agents.dedup();
        let mut failed = None;
        for agent in agents {
            if let Err(why) = delete(&factory, &host.one(&agent)) {
                failed.get_or_insert(why);
            }
        }
        changed(&factory);
        failed.map_or(Ok(()), |why| Err(refused(why)))
    }
}

/// Closes `one` now (Stop): its browsing ends, and Chrome quits as the
/// runner's stdin ends.
fn stop_now(factory: &Arc<Factory>, one: &Arc<One>) {
    let mut inner = one.inner();
    if inner.runner.is_none() {
        return;
    }
    inner.errand.clear();
    unwatch(&mut inner);
    set_state(one, &mut inner, State::Stopping);
    let_go(factory, one, &mut inner);
}

/// Deletes everything `one` keeps: its runner quits Chrome, deletes the
/// profile and ends (started for that alone where the profile is the
/// browser user's, on Linux); on a Mac, its volume goes once its VM has.
/// Its sign-outs waiting go with it. Err when it can't be done now (its
/// setup isn't finished), says why.
fn delete(factory: &Arc<Factory>, one: &Arc<One>) -> Result<(), String> {
    {
        let mut inner = one.inner();
        if inner.deleting {
            return Ok(());
        }
        inner.errand.clear();
        unwatch(&mut inner);
        inner.sites.clear();
        inner.providers.clear();
        if inner.runner.is_none() {
            if !cfg!(target_os = "linux") {
                // A Mac's profile is its volume (the stand-in's, a folder):
                // nothing needs to run to delete it.
                inner.deleting = true;
                drop(inner);
                forget(factory, one, true);
                return Ok(());
            }
            if let Some(setup) = setup::problem(factory) {
                return Err(setup.problem);
            }
            spawn(factory, one, &mut inner).map_err(|error| error.to_string())?;
        }
        inner.deleting = true;
        control(&inner, &Control::DeleteData { profile: profile(factory.home().root(), &one.agent) });
        set_state(one, &mut inner, State::Stopping);
    }
    Ok(())
}

/// `one`'s profile is deleted (`confirmed`, on Linux, when its runner said
/// so; on a Mac, it's about to be, with its volume, once its VM has gone): what Claktory keeps of it goes too, and
/// an agent's is taken off the list of profiles once that's done. Its
/// browser stays known (a Handle may hold it), with nothing kept.
fn forget(factory: &Arc<Factory>, one: &Arc<One>, confirmed: bool) {
    let home = factory.home().root().to_path_buf();
    let file = sign_outs_file(factory, one);
    {
        let mut inner = one.inner();
        inner.sign_outs = Some(Vec::new());
    }
    if let Err(error) = signout::save(&file, &[]) {
        tracing::warn!(%error, "a browser's waiting sign-outs weren't cleared after its data was deleted");
    }
    let weak = Arc::downgrade(factory);
    let one = one.clone();
    tokio::spawn(async move {
        let removed = if cfg!(target_os = "linux") {
            // The runner deleted it, as it said, or it's still there.
            if confirmed { Ok(()) } else { Err("the runner ended without saying it deleted the profile".to_owned()) }
        } else if standin_here(&home) {
            standin::forget(&home, &one.agent)
        } else {
            mac::forget(&home, &one.agent).await
        };
        let Some(factory) = weak.upgrade() else { return };
        one.inner().deleting = false;
        match removed {
            Ok(()) if !one.agent.is_empty() => {
                if let Err(error) = profiles::remove(&home, &one.agent) {
                    tracing::warn!(%error, "an agent's deleted browser stayed listed");
                }
            }
            Ok(()) => {}
            // Kept listed: tried again at the next start.
            Err(why) => tracing::warn!(%why, "a browser's profile wasn't deleted"),
        }
        changed(&factory);
    });
}

/// The viewer stops watching: frames off, the clipboard unwatched.
fn unwatch(inner: &mut Inner) {
    if inner.viewer.take().is_some() {
        control(inner, &Control::View { on: false, max_width: 0, quality_hint: 0 });
        if let Some(seq) = inner.in_flight.take()
            && let Some(runner) = &inner.runner
        {
            let _ = runner.try_send((Channel::Frames, wire::ack(seq).to_vec()));
        }
        after_turn_change(inner);
    }
}

impl Drop for Port {
    fn drop(&mut self) {
        let Some(factory) = self.factory.upgrade() else { return };
        factory.agent_browser.ports().remove(&self.id);
        tab_view::stop_port(&factory, self.id);
        let viewed = self.viewed(&factory);
        if let Some(one) = &viewed {
            unwatch(&mut one.inner());
            changed(&factory);
        }
    }
}

/// An errand's way to Claktory's browser (C2): its CDP connection, and the
/// changes an errand makes (P3 calls them).
pub(crate) struct Handle {
    factory: Weak<Factory>,
    /// Whose browser it is.
    one: Arc<One>,
}

// The agent's browsing calls these (`errands`).
impl Handle {
    /// The person's own browser, for tests that drive it as an agent's
    /// would: agents' errands only ever drive their own
    /// (`Browsers::for_agent`).
    fn new(factory: &Arc<Factory>) -> Handle {
        Handle { factory: Arc::downgrade(factory), one: factory.agent_browser.one("") }
    }

    /// Whether its profile has no Safe Browsing lists yet (a new profile
    /// with no seed here): until Chrome has fetched them, nothing it opens
    /// is checked, so sign-ins and payments wait.
    pub(crate) fn lists_missing(&self) -> bool {
        self.one.inner().lists_missing
    }

    /// The sites its profile holds cookies for, as origins, sorted: what
    /// it's signed in to, as last read.
    pub(crate) fn signed_in(&self) -> Vec<String> {
        self.one.inner().sites.clone()
    }

    fn factory(&self) -> Result<Arc<Factory>, String> {
        let factory = self.factory.upgrade().ok_or_else(|| "Claktory is stopping.".to_owned())?;
        if !enabled() {
            return Err("Claktory's browser isn't available yet.".to_owned());
        }
        Ok(factory)
    }

    /// The errand `errand` starts using it, driven, at `url` (or where it
    /// left off).
    pub(crate) fn begin(&self, errand: &str, url: Option<String>) -> Result<(), String> {
        let factory = self.factory()?;
        let locale = locale::for_start(&factory.lock().config.browser);
        {
            let mut inner = self.one.inner();
            if !inner.errand.is_empty() && inner.errand != errand {
                return Err("Another errand is using Claktory's browser.".to_owned());
            }
            match inner.state {
                State::Browsing | State::Stopping => {
                    return Err("The person is using Claktory's browser now: try again later.".to_owned());
                }
                State::Off | State::Unavailable => {
                    inner.errand = errand.to_owned();
                    if let Err(error) = start(&factory, &self.one, &mut inner, Mode::Driven, url, locale) {
                        inner.errand.clear();
                        return Err(error.to_string());
                    }
                }
                State::Warm => {
                    inner.errand = errand.to_owned();
                    set_state(&self.one, &mut inner, State::Starting);
                    ask_mode(&mut inner, Mode::Driven, url);
                }
                // Starting, or this errand's already.
                _ => inner.errand = errand.to_owned(),
            }
        }
        changed(&factory);
        Ok(())
    }

    /// Hands it to the person (a handoff card waits): the agent can't
    /// drive it, and, `undriven`, Chrome starts again with no automation
    /// (a sign-in, or a check such as Cloudflare's, which refuses an
    /// automated browser: Melone 37116, Bruno 37121), at `url` when given
    /// (http or https), else on its restored pages.
    pub(crate) fn handoff(&self, undriven: bool, url: Option<&str>) -> Result<(), String> {
        let url = match url {
            Some(url) => view::web_address(url)?,
            None => None,
        };
        let factory = self.factory()?;
        {
            let mut inner = self.one.inner();
            if inner.errand.is_empty() || inner.runner.is_none() {
                return Err("Claktory's browser isn't open for this errand.".to_owned());
            }
            set_state(&self.one, &mut inner, State::PersonsTurn);
            if undriven && aimed(&inner) != Some(Mode::Undriven) {
                // The page shown comes back by itself (Chrome restores its
                // tabs); opened again, it would be a second tab (Pass H).
                let shown = |url: &String| url::Url::parse(url).is_ok_and(|url| url.origin().ascii_serialization() == inner.origin);
                let url = url.filter(|url| !shown(url));
                ask_mode(&mut inner, Mode::Undriven, url);
            }
            after_turn_change(&inner);
        }
        changed(&factory);
        Ok(())
    }

    /// Back to the errand's agent after the person's turn (Done): driven
    /// again, Chrome started with its pipe if it ran without.
    pub(crate) fn resume(&self) -> Result<(), String> {
        let factory = self.factory()?;
        {
            let mut inner = self.one.inner();
            if inner.errand.is_empty() || inner.runner.is_none() {
                return Err("Claktory's browser isn't open for this errand.".to_owned());
            }
            // Undriven, or on its way there (a turn handed over a moment
            // ago, its report not in yet): driven again from a new start.
            // DRIVEN only once Chrome is, as the runner says.
            if aimed(&inner) == Some(Mode::Undriven) || inner.mode != Some(Mode::Driven) {
                set_state(&self.one, &mut inner, State::Starting);
                ask_mode(&mut inner, Mode::Driven, None);
            } else {
                set_state(&self.one, &mut inner, State::Driven);
            }
            after_turn_change(&inner);
        }
        changed(&factory);
        Ok(())
    }

    /// The errand is over: it stays open, warm, for the next.
    pub(crate) fn end(&self, errand: &str) {
        let Ok(factory) = self.factory() else { return };
        {
            let mut inner = self.one.inner();
            if inner.errand != errand {
                return;
            }
            inner.errand.clear();
            if inner.runner.is_some() && !matches!(inner.state, State::Stopping | State::Off) {
                sites::read_sites(&mut inner);
                warm(&factory, &self.one, &mut inner);
                after_turn_change(&inner);
            }
            inner.cdp = None;
            inner.release.clear();
            end_lease(&mut inner);
        }
        changed(&factory);
        // The sign-outs that waited for this errand to end.
        apply_when_free(&factory, &self.one);
    }

    /// Closes it now if it's warm: the errand it was kept for ended
    /// unused, so its 10 minutes unused are already up.
    pub(crate) fn close_if_warm(&self) {
        let Ok(factory) = self.factory() else { return };
        if self.one.inner().state != State::Warm {
            return;
        }
        stop_gracefully(&factory, &self.one);
        changed(&factory);
    }

    /// Lets the errand's input (Input.*) reach Chrome for at most `until`
    /// (#530, R11.3 #5), until what's returned is dropped: held here and
    /// by the runner, each ending it at its own expiry too. It ends when
    /// the errand stops driving (the person's turn, Stop, a mode change),
    /// when Chrome starts again and when the runner goes. Err when the
    /// errand doesn't drive it now.
    pub(crate) fn input_lease(&self, until: Duration) -> Result<Lease, String> {
        self.factory()?;
        let mut inner = self.one.inner();
        if inner.state != State::Driven || inner.mode != Some(Mode::Driven) || inner.runner.is_none() {
            return Err("Claktory's browser isn't driven now.".to_owned());
        }
        let for_ms = u32::try_from(until.as_millis()).unwrap_or(u32::MAX).min(crate::browser_runner::lease::MOST_MS);
        inner.next_lease += 1;
        let id = inner.next_lease;
        // A lease still held is replaced (one action at a time).
        end_lease(&mut inner);
        if !control(&inner, &Control::LeaseInput { id, for_ms }) {
            return Err("Claktory's browser is busy: try again in a moment.".to_owned());
        }
        inner.lease = Some((id, Instant::now() + Duration::from_millis(u64::from(for_ms))));
        Ok(Lease { one: Arc::downgrade(&self.one), id })
    }

    /// Ready for the errand once its driven start is set up (the sign-outs
    /// waiting applied, before any page): Ok once it's DRIVEN, Err with why
    /// when that start failed or the browser closed first.
    pub(crate) fn bootstrapped(&self) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), String>> + Send>> {
        if let Err(why) = self.factory() {
            return Box::pin(std::future::ready(Err(why)));
        }
        let one = self.one.clone();
        Box::pin(async move {
            let mut states = one.states.subscribe();
            let deadline = tokio::time::Instant::now() + STARTING;
            loop {
                {
                    let inner = one.inner();
                    if inner.state == State::Driven {
                        return Ok(());
                    }
                    if let Some(why) = &inner.bootstrap_failed {
                        return Err(why.clone());
                    }
                    if matches!(inner.state, State::Off | State::Unavailable | State::Stopping) {
                        return Err("Claktory's browser closed before it was ready.".to_owned());
                    }
                }
                match tokio::time::timeout_at(deadline, states.changed()).await {
                    Ok(Ok(())) => {}
                    Ok(Err(_)) => return Err("Claktory is stopping.".to_owned()),
                    Err(_) => return Err("Claktory's browser took too long to start.".to_owned()),
                }
            }
        })
    }

    /// Signs the browser out of identity provider `provider` (by name; see
    /// `signout`): at once while Chrome runs driven with no errand, else
    /// kept and applied before Chrome next runs a page or when the errand
    /// using it ends. The vault forgets the account only once it's applied.
    pub(crate) fn sign_out(&self, provider: &str) -> crate::errands::SigningOut {
        let Some(known) = signout::known(provider) else {
            return Box::pin(std::future::ready(Err("Claktory's browser doesn't sign in to that provider.".to_owned())));
        };
        self.sign_out_of(signout::Waiting { provider: known.name.to_owned(), site: String::new(), asked_ms: now_ms() })
    }

    /// Signs the browser out of `site`, one of its signed-in sites
    /// ("https://x.com"): its cookies, and those of the names under it,
    /// deleted and its storage cleared, when `sign_out` would.
    pub(crate) fn sign_out_site(&self, site: &str) -> crate::errands::SigningOut {
        if signout::site_host(site).is_none() {
            return Box::pin(std::future::ready(Err("That isn't a site Claktory's browser is signed in to.".to_owned())));
        }
        self.sign_out_of(signout::Waiting { provider: String::new(), site: site.to_owned(), asked_ms: now_ms() })
    }

    fn sign_out_of(&self, sign_out: signout::Waiting) -> crate::errands::SigningOut {
        use crate::errands::SignedOut;
        let factory = match self.factory() {
            Ok(factory) => factory,
            Err(why) => return Box::pin(std::future::ready(Err(why))),
        };
        let same = |waiting: &signout::Waiting| waiting.provider == sign_out.provider && waiting.site == sign_out.site;
        let now = {
            let mut inner = self.one.inner();
            let file = sign_outs_file(&factory, &self.one);
            let list = waiting(&factory, &self.one, &mut inner);
            if !list.iter().any(same) {
                list.push(sign_out.clone());
                if let Err(error) = signout::save(&file, list) {
                    list.retain(|waiting| !same(waiting));
                    return Box::pin(std::future::ready(Err(format!("Claktory couldn't keep the sign-out: {error}"))));
                }
            }
            free(&inner)
        };
        if !now {
            return Box::pin(std::future::ready(Ok(SignedOut::Pending)));
        }
        let one = self.one.clone();
        Box::pin(async move {
            let (done, result) = apply(&factory, &one).await;
            if !done.is_empty() {
                sites::read_sites(&mut one.inner());
            }
            changed(&factory);
            match result {
                _ if done.iter().any(|done| done.provider == sign_out.provider && done.site == sign_out.site) => Ok(SignedOut::Applied),
                Err(why) => Err(format!("{why}. It signs out before the browser next opens a page.")),
                Ok(()) => Ok(SignedOut::Pending),
            }
        })
    }

    /// Whether a sign-out of `provider` (by name) waits.
    pub(crate) fn signing_out(&self, provider: &str) -> bool {
        let Some(factory) = self.factory.upgrade() else { return false };
        let mut inner = self.one.inner();
        waiting(&factory, &self.one, &mut inner).iter().any(|waiting| waiting.provider.eq_ignore_ascii_case(provider))
    }

    /// The sessions the errand attached in Chrome: when it stops driving
    /// (a turn, a handoff, a stop), the host detaches each one itself, so
    /// whatever it set on them (Fetch's pauses) lets go of the page. Only
    /// session ids, never messages: nothing the errand gives acts on the
    /// page in the person's turn. At most 16, each up to 64 letters, digits
    /// and hyphens; others are left out.
    pub(crate) fn on_release(&self, errand: &str, sessions: Vec<String>) {
        let mut inner = self.one.inner();
        if inner.errand != errand {
            return;
        }
        inner.release = sessions
            .into_iter()
            .filter(|session| !session.is_empty() && session.len() <= 64 && session.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-'))
            .take(16)
            .collect();
    }

    /// The folder the driven Chrome downloads into, while a runner that
    /// downloads is up (R9).
    pub(crate) fn download_dir(&self) -> Option<String> {
        let inner = self.one.inner();
        inner.runner.as_ref().and(inner.download_dir.clone())
    }

    /// Lands a download Chrome finished for the errand (`downloads`).
    pub(crate) fn land(&self, download: crate::errands::downloads::Download) -> crate::errands::downloads::Landing {
        downloads::land(self.factory.clone(), self.one.clone(), download)
    }

    /// Deletes download `guid` unread.
    pub(crate) fn drop_download(&self, guid: &str) {
        downloads::drop_download(&self.one, guid);
    }

    /// `errand`'s landed download `name`: the store's file and its row.
    pub(crate) fn stored(&self, errand: &str, name: &str) -> Option<(std::path::PathBuf, crate::errands::downloads::StoredRow)> {
        self.factory.upgrade().and_then(|factory| downloads::stored(&factory, errand, name))
    }

    /// Copies a stored download into `folder` (inside Claktory's own) as
    /// `name`, as landing does.
    pub(crate) fn copy_into(&self, file: &std::path::Path, folder: &std::path::Path, name: &str) -> Result<std::path::PathBuf, String> {
        let factory = self.factory.upgrade().ok_or_else(|| "Claktory is stopping.".to_owned())?;
        downloads::copy_into(factory.home.root(), file, folder, name)
    }

    /// Gives the driven browser the file at `path` for a page, when it's a
    /// plain file of at most 100 MB inside one of `roots` (`uploads::open`):
    /// sent to the runner, which stages a copy. The staged copy's path,
    /// the only one `DOM.setFileInputFiles` takes; or why not, in words for
    /// the agent.
    pub(crate) fn upload(&self, path: &Path, roots: &[std::path::PathBuf]) -> Upload {
        let factory = match self.factory() {
            Ok(factory) => factory,
            Err(why) => return Box::pin(std::future::ready(Err(why))),
        };
        let opened = match uploads::open(path, roots) {
            Ok(opened) => opened,
            Err(why) => return Box::pin(std::future::ready(Err(why))),
        };
        let (id, told) = {
            let mut inner = self.one.inner();
            if inner.state != State::Driven || inner.mode != Some(Mode::Driven) || inner.runner.is_none() {
                return Box::pin(std::future::ready(Err("Claktory's browser isn't driven now.".to_owned())));
            }
            inner.next_upload += 1;
            let id = inner.next_upload;
            let (tell, told) = tokio::sync::oneshot::channel();
            if !control(&inner, &Control::Upload { id, name: opened.name.clone(), bytes: opened.bytes }) {
                return Box::pin(std::future::ready(Err("Claktory's browser is busy: try again in a moment.".to_owned())));
            }
            inner.uploads.insert(id, tell);
            (id, told)
        };
        drop(factory);
        let one = self.one.clone();
        Box::pin(async move {
            let sent = send_upload(&one, id, opened).await;
            // The id alone ends it, sent or not, so the runner lets go.
            let ended = to_runner(&one, Channel::Files, id.to_be_bytes().to_vec()).await;
            if let Err(why) = sent.and(ended) {
                one.inner().uploads.remove(&id);
                return Err(why);
            }
            match tokio::time::timeout(UPLOAD_WAIT, told).await {
                Ok(Ok(staged)) => staged,
                Ok(Err(_)) => Err("Claktory's browser closed before it took the file.".to_owned()),
                Err(_) => {
                    one.inner().uploads.remove(&id);
                    Err("Claktory's browser took too long to take the file.".to_owned())
                }
            }
        })
    }

    /// Its states as they change.
    pub(crate) fn states(&self) -> Option<watch::Receiver<State>> {
        Some(self.one.states.subscribe())
    }
}

/// What `Handle::upload` gives: the staged copy's path, or why not.
pub(crate) type Upload = std::pin::Pin<Box<dyn std::future::Future<Output = Result<String, String>> + Send>>;

/// How long the runner may take to say it staged an upload, once sent.
const UPLOAD_WAIT: Duration = Duration::from_secs(60);

/// Sends upload `id`'s bytes on channel 5, a chunk a frame after its id.
async fn send_upload(one: &One, id: u64, opened: uploads::Opened) -> Result<(), String> {
    use tokio::io::AsyncReadExt;
    let mut file = tokio::fs::File::from_std(opened.file);
    let mut sent = 0u64;
    loop {
        let mut payload = id.to_be_bytes().to_vec();
        payload.resize(8 + wire::MAX_FILE_CHUNK, 0);
        let read = file.read(&mut payload[8..]).await.map_err(|error| format!("Couldn't read the file: {error}"))?;
        if read == 0 {
            break;
        }
        payload.truncate(8 + read);
        sent += read as u64;
        if sent > opened.bytes {
            return Err("The file grew while it was being sent.".to_owned());
        }
        to_runner(one, Channel::Files, payload).await?;
    }
    Ok(())
}

/// One frame to `one`'s runner, waiting for room. Its sender is looked up
/// each time and never kept, so a stopped runner isn't held open.
async fn to_runner(one: &One, channel: Channel, payload: Vec<u8>) -> Result<(), String> {
    let runner = one.inner().runner.clone().ok_or("Claktory's browser closed before it took the file.")?;
    runner.send((channel, payload)).await.map_err(|_| "Claktory's browser closed before it took the file.".to_owned())
}

/// An errand's input lease (`Handle::input_lease`): it ends when dropped.
pub(crate) struct Lease {
    one: Weak<One>,
    id: u64,
}

impl Drop for Lease {
    fn drop(&mut self) {
        let Some(one) = self.one.upgrade() else { return };
        let mut inner = one.inner();
        if inner.lease.is_some_and(|(held, _)| held == self.id) {
            end_lease(&mut inner);
        }
    }
}

/// Tests: `agent`'s browser hands the person a check at `seq` on tab
/// `target` of `domain` (as errands do); what's returned holds where the
/// tab went if it leaves.
#[doc(hidden)]
pub fn check_opened_for_test(factory: &Arc<Factory>, agent: &str, seq: u64, target: &str, domain: &str) -> Arc<Mutex<Option<String>>> {
    let left = Arc::new(Mutex::new(None));
    let told = left.clone();
    view::check_opened(factory, agent, seq, target, domain, Box::new(move |site| *told.lock().expect("left") = Some(site)));
    left
}

/// Tests: the check at `seq` ends; `agent`'s open check's tab before.
#[doc(hidden)]
pub fn end_check_for_test(factory: &Arc<Factory>, agent: &str, seq: u64) -> Option<String> {
    let tab = view::check_tab(factory, agent);
    view::end_check(factory, agent, seq);
    tab
}

/// Each agent's browser, for its browsing (`errands`).
#[derive(Clone)]
pub(crate) struct Browsers {
    factory: Weak<Factory>,
}

impl Browsers {
    pub(crate) fn new(factory: &Arc<Factory>) -> Browsers {
        Browsers { factory: Arc::downgrade(factory) }
    }

    /// `agent`'s browser, made the first time: Err, in words for the
    /// agent, when there's no browser here or the agent doesn't have the
    /// Browser extension. Whether this computer has the memory for it is
    /// said when it starts (`Handle::begin`).
    pub(crate) fn for_agent(&self, agent: &str) -> Result<Handle, String> {
        let factory = self.factory.upgrade().ok_or_else(|| "Claktory is stopping.".to_owned())?;
        if !enabled() {
            return Err("Claktory's browser isn't available yet.".to_owned());
        }
        let browses = factory.lock().config.agent(agent).is_some_and(browsing);
        if !browses {
            return Err("You don't have Claktory's browser: only the person, or an admin agent such as Rivet, turns the Browser extension on.".to_owned());
        }
        Ok(Handle { factory: self.factory.clone(), one: factory.agent_browser.one(agent) })
    }

    /// Closes `agent`'s browser, keeping its profile; nothing when it's
    /// closed.
    pub(crate) fn close(&self, agent: &str) {
        let Some(factory) = self.factory.upgrade() else { return };
        if let Some(one) = factory.agent_browser.get(agent) {
            stop_now(&factory, &one);
            changed(&factory);
        }
    }

    /// Deletes `agent`'s browser: closed, and its profile deleted, running
    /// or not; nothing when it has none. Err says why it couldn't now (it's
    /// tried again at the next start).
    pub(crate) fn delete(&self, agent: &str) -> Result<(), String> {
        let Some(factory) = self.factory.upgrade() else { return Ok(()) };
        if agent.is_empty() {
            return Ok(());
        }
        let result = delete(&factory, &factory.agent_browser.one(agent));
        changed(&factory);
        result
    }
}

impl Host {
    /// Installs the browsers for `factory`: from now on a change of
    /// settings deletes the browser of an agent that doesn't browse any
    /// more (`config_changed`), and those left from before go now (`sweep`).
    pub(crate) fn install(&self, factory: &Arc<Factory>) {
        let _ = self.factory.set(Arc::downgrade(factory));
        self.config_changed();
    }

    /// The settings changed: an agent deleted, or whose Browser extension
    /// was turned off, has its browser closed and its profile deleted. Done
    /// a moment later, away from the Factory's lock, which the caller may
    /// hold.
    pub(crate) fn config_changed(&self) {
        let Some(factory) = self.factory.get().and_then(Weak::upgrade) else { return };
        let Ok(runtime) = tokio::runtime::Handle::try_current() else { return };
        runtime.spawn(async move { sweep(&factory) });
    }
}

/// Deletes the browser of every agent listed (`profiles`), or still open,
/// that doesn't browse any more: deleted, or its Browser extension off.
fn sweep(factory: &Arc<Factory>) {
    let home = factory.home().root().to_path_buf();
    let mut agents = profiles::list(&home);
    agents.extend(factory.agent_browser.browsers().keys().filter(|agent| !agent.is_empty()).cloned());
    agents.sort();
    agents.dedup();
    let gone: Vec<String> = {
        let inner = factory.lock();
        agents.into_iter().filter(|agent| !inner.config.agent(agent).is_some_and(browsing)).collect()
    };
    if gone.is_empty() {
        return;
    }
    for agent in gone {
        let one = factory.agent_browser.one(&agent);
        if let Err(why) = delete(factory, &one) {
            tracing::warn!(%why, "a browser of an agent that doesn't browse any more wasn't deleted yet");
        }
    }
    changed(factory);
}

/// Whether Chrome runs driven with no errand driving it (WARM): the host's
/// own calls can sign it out now.
fn free(inner: &Inner) -> bool {
    inner.state == State::Warm && inner.mode == Some(Mode::Driven) && inner.runner.is_some()
}

/// Applies the sign-outs waiting, if any, when nothing drives Chrome.
fn apply_when_free(factory: &Arc<Factory>, one: &Arc<One>) {
    let due = {
        let mut inner = one.inner();
        free(&inner) && !waiting(factory, one, &mut inner).is_empty()
    };
    if !due {
        return;
    }
    let factory = factory.clone();
    let one = one.clone();
    tokio::spawn(async move {
        let (_, result) = apply(&factory, &one).await;
        if let Err(why) = result {
            // Left waiting: the next driven start applies it before a page.
            tracing::warn!(%why, "Claktory's browser didn't sign out after the errand ended");
        }
        changed(&factory);
    });
}

/// The live view and the person's turn in Claktory's browser are for QA
/// and debugging only (#435, Matt's card 222): refused from devices unless
/// config.json's `browser.live_view` is on.
fn live_view(factory: &Factory) -> Result<(), RequestError> {
    if factory.lock().config.browser.live_view {
        return Ok(());
    }
    Err(refused("Claktory's browser has no live view: agents use it on their own, and ask you in a request when they need you."))
}

impl crate::errands::cdp::Transport for Handle {
    fn connect(&self) -> Result<crate::errands::cdp::Pipe, String> {
        let factory = self.factory()?;
        let mut inner = self.one.inner();
        if inner.state != State::Driven || inner.mode != Some(Mode::Driven) {
            return Err("Claktory's browser isn't driven now.".to_owned());
        }
        if inner.cdp.as_ref().is_some_and(|cdp| !cdp.is_closed()) {
            return Err("Claktory's browser is already being driven.".to_owned());
        }
        if inner.runner.is_none() {
            return Err("Claktory's browser isn't open.".to_owned());
        }
        let (send, mut outgoing) = mpsc::channel::<Vec<u8>>(64);
        let (incoming, receive) = mpsc::channel::<Vec<u8>>(256);
        inner.cdp = Some(incoming);
        let generation = inner.generation;
        let weak = Arc::downgrade(&factory);
        let one = self.one.clone();
        // What the errand sends: to Chrome, only while its agent drives.
        tokio::spawn(async move {
            while let Some(message) = outgoing.recv().await {
                if weak.upgrade().is_none() {
                    return;
                }
                // The runner's sender, looked up each time and never kept:
                // a copy held here would keep a stopped runner going
                // (Ghiaccio 38462).
                let runner = {
                    let inner = one.inner();
                    if inner.generation != generation {
                        return;
                    }
                    let Some(runner) = inner.runner.clone() else { return };
                    // Input only under the errand's lease (R11.3 #5), held
                    // here as well as by the runner: refused at once, with
                    // the same CDP error, rather than sent to be refused.
                    let staged = |path: &str| inner.staged.iter().any(|staged| staged == path);
                    match crate::browser_runner::lease::judge_with_uploads(&message, leased(&inner), &staged) {
                        crate::browser_runner::lease::Verdict::Forward => {}
                        crate::browser_runner::lease::Verdict::Refuse(reply) => {
                            if let Some(cdp) = &inner.cdp {
                                let _ = cdp.try_send(reply);
                            }
                            continue;
                        }
                        crate::browser_runner::lease::Verdict::Drop(why) => {
                            tracing::warn!(why, "an errand's DevTools message was dropped");
                            continue;
                        }
                    }
                    (inner.state == State::Driven).then_some(runner)
                };
                // The host's own ids (sites.rs) aren't the errand's: an
                // answer to one would be read as the host's.
                let Some(runner) = runner.filter(|_| message.len() <= wire::MAX_CDP && !sites::host_id(&message)) else { continue };
                if runner.send((Channel::Cdp, message)).await.is_err() {
                    return;
                }
            }
        });
        Ok(crate::errands::cdp::Pipe { send, receive })
    }
}

/// Tests: errand `errand` drives `agent`'s browser, Chrome finished a
/// download of `bytes` the page called `suggested`, and the host lands it,
/// through the stand-in's channel 5: its name, the agent's copy, its flag,
/// and whether the runner's copy is gone after.
#[doc(hidden)]
pub async fn land_for_test(
    factory: &Arc<Factory>,
    errand: &str,
    agent: &str,
    suggested: &str,
    bytes: &[u8],
) -> Result<(String, std::path::PathBuf, String), String> {
    let handle = Browsers::new(factory).for_agent(agent)?;
    handle.begin(errand, None)?;
    let mut dir = None;
    for _ in 0..300 {
        if handle.one.inner().state == State::Driven {
            dir = handle.download_dir();
        }
        if dir.is_some() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let dir = std::path::PathBuf::from(dir.ok_or("it never drove")?);
    let mut random = [0u8; 16];
    getrandom::fill(&mut random).map_err(|error| error.to_string())?;
    let hex = crate::encoding::hex(&random);
    let guid = format!("{}-{}-{}-{}-{}", &hex[..8], &hex[8..12], &hex[12..16], &hex[16..20], &hex[20..]);
    std::fs::write(dir.join(&guid), bytes).map_err(|error| error.to_string())?;
    let download = crate::errands::downloads::Download {
        errand: errand.to_owned(),
        agent: agent.to_owned(),
        guid: guid.clone(),
        suggested: suggested.to_owned(),
        url: "https://files.example.com/x".to_owned(),
        page_origin: "https://example.com".to_owned(),
    };
    let landed = handle.land(download).await?;
    // The runner's copy goes once it's landed.
    for _ in 0..50 {
        if !dir.join(&guid).exists() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    if dir.join(&guid).exists() {
        return Err("the runner's copy stayed".to_owned());
    }
    let (stored, row) = handle.stored(errand, &landed.name).ok_or("not stored")?;
    if std::fs::read(&stored).ok().as_deref() != Some(bytes) || row.sha256 != landed.sha256 || row.bytes != bytes.len() as u64 {
        return Err("the store doesn't hold it".to_owned());
    }
    Ok((landed.name, landed.path, landed.flag))
}

/// Tests: errand `errand` is over (the browser stays warm).
#[doc(hidden)]
pub fn end_for_test(factory: &Arc<Factory>, errand: &str) {
    Handle::new(factory).end(errand);
}

/// Tests: `agent`'s errand `errand` is over.
#[doc(hidden)]
pub fn end_agent_for_test(factory: &Arc<Factory>, agent: &str, errand: &str) -> Result<(), String> {
    Browsers::new(factory).for_agent(agent)?.end(errand);
    Ok(())
}

/// Tests: errand `errand` drives the browser and connects to it, as an
/// errand's agent does; the connection lasts while what's returned is kept.
#[doc(hidden)]
pub async fn connect_for_test(factory: &Arc<Factory>, errand: &str, url: Option<String>) -> Result<Box<dyn std::any::Any + Send>, String> {
    use crate::errands::cdp::Transport;
    let handle = Handle::new(factory);
    handle.begin(errand, url)?;
    for _ in 0..300 {
        if handle.one.inner().state == State::Driven {
            return Ok(Box::new(handle.connect()?));
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    Err("it never drove".to_owned())
}

/// Tests: the sites errand_open tells an agent the browser is signed in to.
#[doc(hidden)]
pub fn errands_signed_in_for_test(factory: &Arc<Factory>) -> Vec<String> {
    Handle::new(factory).signed_in()
}

/// Tests: an errand driving the browser through its own connection, as an
/// errand's agent does (`drive_for_test`).
#[doc(hidden)]
pub struct DriveForTest {
    handle: Handle,
    pipe: crate::errands::cdp::Pipe,
    next: u64,
}

impl DriveForTest {
    /// Sends one CDP command and waits up to 10 s for its answer, the
    /// whole message (`result` or `error`); Null when none came.
    pub async fn call(&mut self, method: &str, params: serde_json::Value) -> serde_json::Value {
        self.call_in("", method, params).await
    }

    /// `call`, in CDP session `session` (none when empty).
    pub async fn call_in(&mut self, session: &str, method: &str, params: serde_json::Value) -> serde_json::Value {
        self.next += 1;
        let id = self.next;
        let mut message = serde_json::json!({"id": id, "method": method, "params": params});
        if !session.is_empty() {
            message["sessionId"] = serde_json::Value::String(session.to_owned());
        }
        if self.pipe.send.send(serde_json::to_vec(&message).unwrap_or_default()).await.is_err() {
            return serde_json::Value::Null;
        }
        let answer = async {
            while let Some(bytes) = self.pipe.receive.recv().await {
                let message: serde_json::Value = serde_json::from_slice(&bytes).unwrap_or_default();
                if message["id"].as_u64() == Some(id) {
                    return message;
                }
            }
            serde_json::Value::Null
        };
        tokio::time::timeout(Duration::from_secs(10), answer).await.unwrap_or_default()
    }

    /// Attaches to the first page target: its session.
    pub async fn page(&mut self) -> String {
        let targets = self.call("Target.getTargets", serde_json::json!({})).await;
        let page = targets["result"]["targetInfos"].as_array().into_iter().flatten().find(|target| target["type"] == "page").cloned();
        let Some(page) = page else { return String::new() };
        let attached = self.call("Target.attachToTarget", serde_json::json!({"targetId": page["targetId"], "flatten": true})).await;
        attached["result"]["sessionId"].as_str().unwrap_or_default().to_owned()
    }

    /// Takes an input lease for `ms`; it ends when what's returned drops.
    pub fn lease(&self, ms: u64) -> Result<Box<dyn std::any::Any + Send>, String> {
        self.handle.input_lease(Duration::from_millis(ms)).map(|lease| Box::new(lease) as Box<dyn std::any::Any + Send>)
    }

    /// `Handle::bootstrapped`.
    pub async fn bootstrapped(&self) -> Result<(), String> {
        self.handle.bootstrapped().await
    }

    /// `Handle::upload`.
    pub async fn upload(&self, path: &Path, roots: &[std::path::PathBuf]) -> Result<String, String> {
        self.handle.upload(path, roots).await
    }

    /// `Handle::sign_out_site`: whether it was applied at once.
    pub async fn sign_out_site(&self, site: &str) -> Result<bool, String> {
        self.handle.sign_out_site(site).await.map(|done| done == crate::errands::SignedOut::Applied)
    }

    /// Ends its errand, as `Handle::end` (the browser stays warm, and
    /// sign-outs waiting for it are applied).
    pub fn end(&self, errand: &str) {
        self.handle.end(errand);
    }
}

/// Tests: errand `errand` drives the browser and connects to it.
#[doc(hidden)]
pub async fn drive_for_test(factory: &Arc<Factory>, errand: &str, url: Option<String>) -> Result<DriveForTest, String> {
    use crate::errands::cdp::Transport;
    let handle = Handle::new(factory);
    handle.begin(errand, url)?;
    for _ in 0..300 {
        if handle.one.inner().state == State::Driven {
            let pipe = handle.connect()?;
            return Ok(DriveForTest { handle, pipe, next: 0 });
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    Err("it never drove".to_owned())
}

/// Tests: `agent`'s own browser drives for errand `errand` and connects to
/// it, as that agent's browsing does.
#[doc(hidden)]
pub async fn drive_agent_for_test(factory: &Arc<Factory>, agent: &str, errand: &str, url: Option<String>) -> Result<DriveForTest, String> {
    use crate::errands::cdp::Transport;
    let handle = Browsers::new(factory).for_agent(agent)?;
    handle.begin(errand, url)?;
    for _ in 0..300 {
        if handle.one.inner().state == State::Driven {
            let pipe = handle.connect()?;
            return Ok(DriveForTest { handle, pipe, next: 0 });
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    Err("it never drove".to_owned())
}

/// Tests: `Browsers::close` and `Browsers::delete` for `agent`.
#[doc(hidden)]
pub fn close_for_test(factory: &Arc<Factory>, agent: &str) {
    Browsers::new(factory).close(agent);
}

#[doc(hidden)]
pub fn delete_for_test(factory: &Arc<Factory>, agent: &str) -> Result<(), String> {
    Browsers::new(factory).delete(agent)
}

/// Tests: `agent`'s browser's state, Off when it has none yet.
#[doc(hidden)]
pub fn agent_state_for_test(factory: &Arc<Factory>, agent: &str) -> i32 {
    factory.agent_browser.get(agent).map_or(State::Off, |one| one.inner().state) as i32
}

/// Tests: where the stand-in keeps `agent`'s profile, and the agents whose
/// browsers have a profile.
#[doc(hidden)]
pub fn standin_profile_for_test(factory: &Arc<Factory>, agent: &str) -> std::path::PathBuf {
    factory.home().root().join("browser-standins").join(agent_key(agent))
}

#[doc(hidden)]
pub fn profiles_for_test(factory: &Arc<Factory>) -> Vec<String> {
    profiles::list(factory.home().root())
}

/// Tests: the person's turn and back at once (Handle::handoff, then
/// resume): with the browser driven throughout, or, `undriven`, Chrome
/// handed over without automation.
#[doc(hidden)]
pub fn turn_for_test(factory: &Arc<Factory>, undriven: bool) -> Result<(), String> {
    let handle = Handle::new(factory);
    handle.handoff(undriven, None)?;
    handle.resume()
}

/// Tests: how Chrome runs, as the runner last said ("driven",
/// "undriven"), and the browser's state.
#[doc(hidden)]
pub fn mode_for_test(factory: &Arc<Factory>) -> (Option<&'static str>, i32) {
    let one = factory.agent_browser.one("");
    let inner = one.inner();
    let mode = inner.mode.map(|mode| match mode {
        Mode::Driven => "driven",
        Mode::Undriven => "undriven",
    });
    (mode, inner.state as i32)
}

/// Tests: signs the browser out of `provider` (Handle::sign_out): "applied"
/// or "pending", or why not.
#[doc(hidden)]
pub async fn sign_out_for_test(factory: &Arc<Factory>, provider: &str) -> Result<&'static str, String> {
    Handle::new(factory).sign_out(provider).await.map(|done| match done {
        crate::errands::SignedOut::Applied => "applied",
        crate::errands::SignedOut::Pending => "pending",
    })
}

/// Tests: whether a sign-out of `provider` waits.
#[doc(hidden)]
pub fn signing_out_for_test(factory: &Arc<Factory>, provider: &str) -> bool {
    Handle::new(factory).signing_out(provider)
}

#[cfg(test)]
mod tests {
    use super::{dev_home, home_id, on_by};
    use std::path::Path;

    /// Each Claktory home has its own browser id, the same each
    /// time; only this user's own ~/.factory-dev is the dev one.
    #[test]
    fn each_claktory_home_has_its_own_browser_id() {
        let a = home_id(Path::new("/Users/someone/.factory"));
        let b = home_id(Path::new("/Users/someone/qa/sandboxes/h1/user/.factory"));
        assert_ne!(a, b);
        assert_eq!(a, home_id(Path::new("/Users/someone/.factory")));
        assert!(a.len() == 16 && a.bytes().all(|byte| byte.is_ascii_hexdigit()), "{a}");
        // A wire profile name, the person's and an agent's.
        assert!(crate::browser_runner::wire::profile_name_ok(&format!("c{a}")));
        let agent = super::profile(Path::new("/Users/someone/.factory"), &"an agent's long id ".repeat(20));
        assert!(crate::browser_runner::wire::profile_name_ok(&agent), "{agent}");
        assert!(!dev_home(Path::new("/Users/someone/qa/sandboxes/h1/user/.factory-dev")));
        if let Some(account) = super::account_home() {
            assert!(dev_home(&account.join(".factory-dev")));
            assert!(!dev_home(&account.join(".factory")));
        }
    }

    /// #435 W12: on in a release build unless turned off, off in a debug
    /// build (the dev backend, tests) unless turned on.
    #[test]
    fn the_browser_is_on_in_releases_and_off_in_debug_builds_unless_said() {
        assert!(on_by(None, false));
        assert!(!on_by(Some("0"), false));
        assert!(on_by(Some("anything"), false));
        assert!(!on_by(None, true));
        assert!(on_by(Some("1"), true));
        assert!(!on_by(Some("true"), true));
    }
}
