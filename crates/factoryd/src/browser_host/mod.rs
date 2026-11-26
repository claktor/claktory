//! Claktory's browser, factoryd's side (#435 P4, agent-browser.md Rev 2,
//! SPLIT.md): the one browser agents' errands use, run by the browser
//! runner (`crate::browser_runner`, C1) as its own user on a Linux server,
//! or, on a Mac dev backend, by a stand-in over a local headless Chrome
//! (`standin`), so the app's live view can be built before Linux runs.
//!
//! The person sees it only when they ask (Show me, Watch, Take over,
//! Open): its frames, and what's copied in it, go only to the one
//! connection watching (`Port`), never in the Workspace, to agents or
//! pipes; input counts only from that connection, only during the person's
//! turn (PERSONS_TURN, TAKEN_OVER, BROWSING). Whatever the runner says is a
//! hostile page's after a Chrome escape: capped and checked before it
//! reaches anything (Fugo 36537, 36598).
//!
//! Off unless FACTORY_AGENT_BROWSER=1 (until slice 1 ships): the
//! Workspace leaves it unset and every request is refused.

pub(crate) mod downloads;
pub(crate) mod linux;
pub(crate) mod locale;
pub(crate) mod mac;
pub(crate) mod setup;
pub(crate) mod sites;
pub(crate) mod standin;
pub(crate) mod view;

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, Weak};
use std::time::Duration;

use factory_protocol::v1::{self, AgentBrowserState as State, server_message::Body};
use tokio::sync::{mpsc, watch};
use tokio_util::sync::{CancellationToken, DropGuard};

use crate::browser_runner::wire::{self, Channel, Control, Mode, Report};
use crate::factory::{Factory, RequestError};
use crate::transcript::now_ms;

/// How long it stays open with no one using it (R8.2).
const WARM: Duration = Duration::from_secs(30 * 60);
/// How long a graceful stop may take before the runner is closed.
const STOP_WAIT: Duration = Duration::from_secs(10);
/// How long a runner let go of may take to end (its stdin closed, SIGTERM
/// 2 s later, SIGKILL 2 s after that) before another may start anyway.
const GONE_WAIT: Duration = Duration::from_secs(10);
/// The desktop's screen (Chrome's window maximised on it, Melone 37116).
const DESKTOP: (u16, u16) = (1920, 1080);
/// A connection's queue: frames go one at a time, so a few suffice.
const QUEUE: usize = 8;

/// Turned on by a test, whatever the environment says.
static TURNED_ON: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Whether Claktory's browser is on here (off until slice 1 ships).
pub(crate) fn enabled() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    TURNED_ON.load(Ordering::Relaxed) || *ON.get_or_init(|| std::env::var("FACTORY_AGENT_BROWSER").is_ok_and(|value| value == "1"))
}

/// Turned on by a test: the stand-in rather than the Mac's VM.
static STANDIN_ON: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Whether a Mac runs the dev stand-in over its own Chrome
/// (FACTORY_AGENT_BROWSER_STANDIN=1, the dev Claktory or a debug build
/// only: `standin` refuses otherwise) rather than the browser's VM (`mac`).
pub(crate) fn standin_asked() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    STANDIN_ON.load(Ordering::Relaxed) || *ON.get_or_init(|| std::env::var("FACTORY_AGENT_BROWSER_STANDIN").is_ok_and(|value| value == "1"))
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

/// Claktory's browser: one per Claktory.
pub(crate) struct Host {
    inner: Mutex<Inner>,
    next_port: AtomicU64,
    /// Its state, for errands (C2) to follow.
    states: watch::Sender<State>,
}

impl Default for Host {
    fn default() -> Host {
        Host { inner: Mutex::new(Inner::default()), next_port: AtomicU64::new(1), states: watch::channel(State::Off).0 }
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
    /// Every connection's slot, by port id.
    ports: HashMap<u64, Slot>,
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
    chrome: String,
    warm_until: i64,
    /// Why the runner itself refused to start (a setup problem it found:
    /// no firewall, no /proc of its own…), cleaned; shown on the setup
    /// card until the next start.
    refused: Option<String>,
    /// Input this second: when it began and how many events.
    input_window: (i64, u32),
    /// How Chrome runs now, once the runner said.
    mode: Option<Mode>,
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
    /// The downloads being read over channel 5, by GUID.
    reads: downloads::Reads,
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
            ports: HashMap::new(),
            viewer: None,
            screen: DESKTOP,
            in_flight: None,
            origin: String::new(),
            title: String::new(),
            text_field_focused: false,
            sites: Vec::new(),
            chrome: String::new(),
            warm_until: 0,
            refused: None,
            input_window: (0, 0),
            mode: None,
            cdp: None,
            next_call: sites::HOST_IDS,
            calls: HashMap::new(),
            page_read: 0,
            release: Vec::new(),
            download_dir: None,
            reads: downloads::Reads::new(),
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

impl Host {
    fn inner(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// What the Workspace shows; None while it's off here.
    pub(crate) fn view(&self, factory: &Factory, browser: &crate::config::BrowserConfig) -> Option<v1::AgentBrowser> {
        if !enabled() {
            return None;
        }
        let (time_zone, languages) = locale::shown(browser);
        let inner = self.inner();
        let setup = setup::cached(factory)
            .or_else(|| inner.refused.as_ref().map(|problem| v1::AgentBrowserSetup { problem: problem.clone(), ..Default::default() }));
        let state = if setup.is_some() && inner.runner.is_none() { State::Unavailable } else { inner.state };
        Some(v1::AgentBrowser {
            state: state.into(),
            errand_id: inner.errand.clone(),
            setup,
            signed_in_sites: inner.sites.clone(),
            viewer_device: inner.viewer.as_ref().map(|viewer| viewer.device.clone()).unwrap_or_default(),
            warm_until_ms: if inner.state == State::Warm { inner.warm_until } else { 0 },
            chrome_version: inner.chrome.clone(),
            time_zone,
            languages,
        })
    }
}

/// Sets the state, tells errands, and leaves the Workspace to the caller.
fn set_state(host: &Host, inner: &mut Inner, state: State) {
    if inner.state != state {
        // Out of the agent's hands: its sessions in Chrome are let go (so
        // a fence it set on one no longer holds the person's page), and
        // its connection ends; it connects again once it drives again.
        if inner.state == State::Driven {
            sites::release(inner);
            inner.cdp = None;
        }
        inner.state = state;
        host.states.send_replace(state);
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

/// The kept profile's name: one for the real Claktory, one for the dev
/// one, so they never share sign-ins.
fn profile(factory: &Factory) -> &'static str {
    if factory.home().root().file_name().is_some_and(|name| name == ".factory-dev") { "dev" } else { "claktory" }
}

/// Starts the runner (state STARTING), Chrome in `mode` at `url`.
/// Starts it in `mode`, its pages in `locale`'s time zone and languages
/// (`locale::for_start`).
fn start(
    factory: &Arc<Factory>,
    inner: &mut Inner,
    mode: Mode,
    url: Option<String>,
    locale: (Option<String>, Vec<String>),
) -> Result<(), RequestError> {
    if let Some(setup) = setup::problem(factory) {
        return Err(refused(setup.problem));
    }
    spawn(factory, inner)?;
    set_state(&factory.agent_browser, inner, State::Starting);
    let (time_zone, languages) = locale;
    let start =
        Control::Start { profile: profile(factory).to_owned(), mode, url, width: DESKTOP.0, height: DESKTOP.1, time_zone, languages };
    control(inner, &start);
    Ok(())
}

/// Starts a runner, with nothing up in it yet.
fn spawn(factory: &Arc<Factory>, inner: &mut Inner) -> Result<(), RequestError> {
    if inner.exiting {
        return Err(refused("Claktory's browser is still closing: try again in a moment."));
    }
    let io = if cfg!(target_os = "linux") {
        linux::spawn(factory)
    } else if standin_asked() {
        standin::spawn(factory)
    } else {
        mac::spawn(factory)
    }
    .map_err(refused)?;
    inner.refused = None;
    inner.generation += 1;
    inner.runner = Some(io.to);
    inner.quit = Some(io.quit.drop_guard());
    inner.screen = DESKTOP;
    tokio::spawn(read(Arc::downgrade(factory), inner.generation, io.from));
    Ok(())
}

/// What the runner says, until it ends.
async fn read(factory: Weak<Factory>, generation: u64, mut from: mpsc::Receiver<(Channel, Vec<u8>)>) {
    while let Some((channel, payload)) = from.recv().await {
        let Some(factory) = factory.upgrade() else { return };
        let current = {
            let inner = factory.agent_browser.inner();
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
            downloads::chunk(&factory, payload).await;
            continue;
        }
        output(&factory, channel, payload);
    }
    if let Some(factory) = factory.upgrade() {
        gone(&factory, generation);
    }
}

/// The runner of `generation` has ended (its output closed): another may
/// start, and the host is off if it wasn't yet.
fn gone(factory: &Factory, generation: u64) {
    {
        let mut inner = factory.agent_browser.inner();
        if inner.generation == generation {
            inner.exiting = false;
        }
    }
    ended(factory, generation, "Chrome closed unexpectedly.");
}

/// The runner is gone: off, its errand told through the state.
fn ended(factory: &Factory, generation: u64, why: &str) {
    let host = &factory.agent_browser;
    {
        let mut inner = host.inner();
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
        inner.cdp = None;
        // Reads under way end, unread.
        inner.reads.clear();
        inner.download_dir = None;
        set_state(host, &mut inner, State::Off);
    }
    changed(factory);
}

/// One payload from the runner: hostile until checked.
fn output(factory: &Arc<Factory>, channel: Channel, payload: Vec<u8>) {
    match channel {
        Channel::Control => match wire::decode::<Report>(&payload) {
            Some(report) => reported(factory, report),
            None => tracing::warn!(bytes = payload.len(), "Claktory's browser sent a report it couldn't read"),
        },
        Channel::Frames => view::frame(factory, &payload),
        Channel::Clipboard => view::clipboard(factory, payload),
        // To the errand's connection (C2), only while its agent drives.
        Channel::Cdp => {
            // The host's own answers (sites.rs) never reach the errand.
            if sites::answer(factory, &payload) {
                return;
            }
            let inner = factory.agent_browser.inner();
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

fn reported(factory: &Arc<Factory>, report: Report) {
    let host = &factory.agent_browser;
    let mut stop = false;
    {
        let mut inner = host.inner();
        match report {
            Report::Ready { wire: version, chrome, mode, download_dir, .. } => {
                // Only a plain absolute folder (the errand hands it to
                // Chrome, inside the runner's fence).
                inner.download_dir =
                    download_dir.filter(|dir| dir.starts_with('/') && dir.len() <= 1024 && view::cleaned(dir, 1024) == *dir);
                if version != wire::VERSION {
                    tracing::warn!(version, "Claktory's browser runner speaks another wire: stopped");
                    stop = true;
                } else {
                    inner.chrome = view::cleaned(&chrome, 40);
                    set_mode(&mut inner, mode);
                    let state = settled(&inner, mode);
                    set_state(host, &mut inner, state);
                    after_turn_change(&inner);
                    sites::read_sites(&mut inner);
                }
            }
            Report::ModeChanged { mode } => {
                set_mode(&mut inner, mode);
                let state = settled(&inner, mode);
                set_state(host, &mut inner, state);
                after_turn_change(&inner);
                // Back from the person's turn: what they signed in to.
                sites::read_sites(&mut inner);
            }
            Report::Resized { width, height } => inner.screen = (width.clamp(1, 4096), height.clamp(1, 4096)),
            Report::ChromeExited { .. } => stop = true,
            Report::DataDeleted => {
                inner.sites.clear();
                factory.errands.set_signed_in(Vec::new());
            }
            Report::Stopped { .. } => {}
            // Downloads (R9): not asked for yet; the runner deletes its own.
            // A hint only: factoryd counts and hashes what it reads itself.
            Report::DownloadDone { .. } => {}
            Report::DownloadCancelled { guid, why } => downloads::cancelled(&mut inner, &guid, &why),
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
        close(factory, "it stopped after an error");
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
fn let_go(factory: &Arc<Factory>, inner: &mut Inner) {
    inner.quit = None;
    if inner.runner.take().is_none() {
        return;
    }
    inner.exiting = true;
    let generation = inner.generation;
    let weak = Arc::downgrade(factory);
    tokio::spawn(async move {
        tokio::time::sleep(GONE_WAIT).await;
        if let Some(factory) = weak.upgrade()
            && let inner = factory.agent_browser.inner()
            && inner.exiting
            && inner.generation == generation
        {
            drop(inner);
            tracing::warn!("Claktory's browser runner didn't end after it was let go of");
            gone(&factory, generation);
        }
    });
}

/// Closes the runner at once (a fault, or a stop that took too long):
/// off now, whatever used it told it closed.
fn close(factory: &Arc<Factory>, why: &str) {
    let generation = {
        let mut inner = factory.agent_browser.inner();
        let_go(factory, &mut inner);
        inner.generation
    };
    ended(factory, generation, why);
}

/// Quits Chrome gracefully (its cookies written), then closes the runner.
fn stop_gracefully(factory: &Arc<Factory>) {
    let generation = {
        let mut inner = factory.agent_browser.inner();
        if inner.runner.is_none() {
            return;
        }
        control(&inner, &Control::Stop);
        set_state(&factory.agent_browser, &mut inner, State::Stopping);
        inner.generation
    };
    let weak = Arc::downgrade(factory);
    tokio::spawn(async move {
        tokio::time::sleep(STOP_WAIT).await;
        if let Some(factory) = weak.upgrade()
            && factory.agent_browser.inner().generation == generation
        {
            close(&factory, "it didn't stop in time");
        }
    });
}

/// WARM from now: it closes by itself 30 minutes after it was last used.
fn warm(factory: &Arc<Factory>, inner: &mut Inner) {
    set_state(&factory.agent_browser, inner, State::Warm);
    let until = now_ms() + WARM.as_millis() as i64;
    inner.warm_until = until;
    let weak = Arc::downgrade(factory);
    tokio::spawn(async move {
        tokio::time::sleep(WARM).await;
        let Some(factory) = weak.upgrade() else { return };
        let due = {
            let inner = factory.agent_browser.inner();
            inner.state == State::Warm && inner.warm_until == until
        };
        if due {
            stop_gracefully(&factory);
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

impl Port {
    pub(crate) fn new(factory: &Arc<Factory>, device: &str) -> Port {
        let host = &factory.agent_browser;
        let id = host.next_port.fetch_add(1, Ordering::Relaxed);
        let (sender, receiver) = mpsc::channel(QUEUE);
        host.inner().ports.insert(id, Slot { device: device.to_owned(), sender });
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

    /// WatchAgentBrowser.
    pub(crate) fn watch(&self, request: &v1::WatchAgentBrowser) -> Result<(), RequestError> {
        let factory = self.factory()?;
        if request.on {
            locale::offer(&factory, &request.device_time_zone, &request.device_languages);
        }
        let host = &factory.agent_browser;
        {
            let mut inner = host.inner();
            if !request.on {
                if inner.viewer.as_ref().is_some_and(|viewer| viewer.port == self.id) {
                    unwatch(&mut inner);
                }
                drop(inner);
                changed(&factory);
                return Ok(());
            }
            if inner.runner.is_none() || matches!(inner.state, State::Off | State::Stopping | State::Unavailable) {
                return Err(refused("Claktory's browser isn't open. Open it first."));
            }
            let device = inner.ports.get(&self.id).map(|slot| slot.device.clone()).ok_or_else(|| refused("This connection is closing."))?;
            // The view moves: the connection that had it hears so first,
            // before this one's first frame (Formaggio 36591).
            if let Some(before) = inner.viewer.take().filter(|viewer| viewer.port != self.id) {
                if let Some(slot) = inner.ports.get(&before.port) {
                    let _ = slot.sender.try_send(Body::AgentBrowserViewMoved(v1::AgentBrowserViewMoved { device: device.clone() }));
                }
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
            inner.viewer = Some(Viewer { port: self.id, device, max_width, shape: (request.screen_width, request.screen_height) });
            control(&inner, &Control::View { on: true, max_width: max_width as u16, quality_hint: 0 });
            after_turn_change(&inner);
        }
        changed(&factory);
        Ok(())
    }

    /// AgentBrowserFrameAck: from the viewer, for the frame in flight.
    pub(crate) fn ack(&self, request: &v1::AgentBrowserFrameAck) -> Result<(), RequestError> {
        let factory = self.factory()?;
        let mut inner = factory.agent_browser.inner();
        if inner.viewer.as_ref().is_none_or(|viewer| viewer.port != self.id) {
            return Ok(());
        }
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
        let mut inner = factory.agent_browser.inner();
        if inner.viewer.as_ref().is_none_or(|viewer| viewer.port != self.id) {
            return Err(refused("Watch Claktory's browser here first."));
        }
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

    /// TakeOverAgentBrowser: the errand's agent pauses.
    pub(crate) fn take_over(&self) -> Result<(), RequestError> {
        let factory = self.factory()?;
        {
            let mut inner = factory.agent_browser.inner();
            if inner.viewer.as_ref().is_none_or(|viewer| viewer.port != self.id) {
                return Err(refused("Watch Claktory's browser here first."));
            }
            match inner.state {
                State::Driven => set_state(&factory.agent_browser, &mut inner, State::TakenOver),
                State::PersonsTurn | State::TakenOver | State::Browsing => return Ok(()),
                _ => return Err(refused("No errand is using Claktory's browser now.")),
            }
            after_turn_change(&inner);
        }
        changed(&factory);
        Ok(())
    }

    /// HandBackAgentBrowser: back to the errand, or done browsing.
    pub(crate) fn hand_back(&self) -> Result<(), RequestError> {
        let factory = self.factory()?;
        {
            let mut inner = factory.agent_browser.inner();
            match inner.state {
                State::TakenOver => set_state(&factory.agent_browser, &mut inner, State::Driven),
                State::Browsing => warm(&factory, &mut inner),
                _ => return Err(refused("You haven't taken Claktory's browser over.")),
            }
            after_turn_change(&inner);
        }
        changed(&factory);
        Ok(())
    }

    /// StopAgentBrowser: the errand ends and Chrome closes; the profile is
    /// kept. STOPPING until the runner has ended (its errand told the
    /// person stopped it), then OFF.
    pub(crate) fn stop(&self) -> Result<(), RequestError> {
        let factory = self.factory()?;
        {
            let mut inner = factory.agent_browser.inner();
            if inner.runner.is_none() {
                return Err(refused("Claktory's browser isn't open."));
            }
            inner.errand.clear();
            unwatch(&mut inner);
            set_state(&factory.agent_browser, &mut inner, State::Stopping);
            let_go(&factory, &mut inner);
        }
        changed(&factory);
        Ok(())
    }

    /// OpenAgentBrowser: the person's, with no errand.
    pub(crate) fn open(&self, request: &v1::OpenAgentBrowser) -> Result<(), RequestError> {
        let factory = self.factory()?;
        let url = view::web_address(&request.url).map_err(refused)?;
        let browser = locale::offer(&factory, &request.device_time_zone, &request.device_languages);
        {
            let mut inner = factory.agent_browser.inner();
            if !inner.errand.is_empty() {
                return Err(refused("An errand is using Claktory's browser: watch it instead."));
            }
            match inner.state {
                State::Off | State::Unavailable => start(&factory, &mut inner, Mode::Undriven, url, locale::for_start(&browser))?,
                State::Warm => {
                    control(&inner, &Control::Mode { mode: Mode::Undriven, url });
                    set_state(&factory.agent_browser, &mut inner, State::Browsing);
                }
                State::Browsing => {
                    if url.is_some() {
                        control(&inner, &Control::Mode { mode: Mode::Undriven, url });
                    }
                }
                _ => return Err(refused("Claktory's browser is busy: try again in a moment.")),
            }
        }
        changed(&factory);
        Ok(())
    }

    /// DeleteAgentBrowserData: everything it keeps, gone. The runner quits
    /// Chrome, deletes the profile and ends (started for it alone when it
    /// isn't running).
    pub(crate) fn delete_data(&self) -> Result<(), RequestError> {
        let factory = self.factory()?;
        {
            let mut inner = factory.agent_browser.inner();
            inner.errand.clear();
            unwatch(&mut inner);
            if inner.runner.is_none() {
                if let Some(setup) = setup::problem(&factory) {
                    return Err(refused(setup.problem));
                }
                spawn(&factory, &mut inner)?;
            }
            control(&inner, &Control::DeleteData { profile: profile(&factory).to_owned() });
            set_state(&factory.agent_browser, &mut inner, State::Stopping);
            inner.sites.clear();
        }
        factory.errands.set_signed_in(Vec::new());
        changed(&factory);
        Ok(())
    }
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
        let was_viewer = {
            let mut inner = factory.agent_browser.inner();
            inner.ports.remove(&self.id);
            let was = inner.viewer.as_ref().is_some_and(|viewer| viewer.port == self.id);
            if was {
                unwatch(&mut inner);
            }
            was
        };
        if was_viewer {
            changed(&factory);
        }
    }
}

/// An errand's way to Claktory's browser (C2): its CDP connection, and the
/// changes an errand makes (P3 calls them).
pub(crate) struct Handle {
    factory: Weak<Factory>,
}

// P3's errands call these (Secco).
impl Handle {
    pub(crate) fn new(factory: &Arc<Factory>) -> Handle {
        Handle { factory: Arc::downgrade(factory) }
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
            let mut inner = factory.agent_browser.inner();
            if !inner.errand.is_empty() && inner.errand != errand {
                return Err("Another errand is using Claktory's browser.".to_owned());
            }
            match inner.state {
                State::Browsing | State::Stopping => {
                    return Err("The person is using Claktory's browser now: try again later.".to_owned());
                }
                State::Off | State::Unavailable => {
                    inner.errand = errand.to_owned();
                    if let Err(error) = start(&factory, &mut inner, Mode::Driven, url, locale) {
                        inner.errand.clear();
                        return Err(error.to_string());
                    }
                }
                State::Warm => {
                    inner.errand = errand.to_owned();
                    set_state(&factory.agent_browser, &mut inner, State::Starting);
                    control(&inner, &Control::Mode { mode: Mode::Driven, url });
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
            let mut inner = factory.agent_browser.inner();
            if inner.errand.is_empty() || inner.runner.is_none() {
                return Err("Claktory's browser isn't open for this errand.".to_owned());
            }
            set_state(&factory.agent_browser, &mut inner, State::PersonsTurn);
            if undriven && inner.mode != Some(Mode::Undriven) {
                // The page shown comes back by itself (Chrome restores its
                // tabs); opened again, it would be a second tab (Pass H).
                let shown = |url: &String| url::Url::parse(url).is_ok_and(|url| url.origin().ascii_serialization() == inner.origin);
                let url = url.filter(|url| !shown(url));
                control(&inner, &Control::Mode { mode: Mode::Undriven, url });
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
            let mut inner = factory.agent_browser.inner();
            if inner.errand.is_empty() || inner.runner.is_none() {
                return Err("Claktory's browser isn't open for this errand.".to_owned());
            }
            if inner.mode == Some(Mode::Undriven) {
                set_state(&factory.agent_browser, &mut inner, State::Starting);
                control(&inner, &Control::Mode { mode: Mode::Driven, url: None });
            } else {
                set_state(&factory.agent_browser, &mut inner, State::Driven);
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
            let mut inner = factory.agent_browser.inner();
            if inner.errand != errand {
                return;
            }
            inner.errand.clear();
            if inner.runner.is_some() && !matches!(inner.state, State::Stopping | State::Off) {
                sites::read_sites(&mut inner);
                warm(&factory, &mut inner);
                after_turn_change(&inner);
            }
            inner.cdp = None;
            inner.release.clear();
        }
        changed(&factory);
    }

    /// The sessions the errand attached in Chrome: when it stops driving
    /// (a turn, a handoff, a stop), the host detaches each one itself, so
    /// whatever it set on them (Fetch's pauses) lets go of the page. Only
    /// session ids, never messages: nothing the errand gives acts on the
    /// page in the person's turn. At most 16, each up to 64 letters, digits
    /// and hyphens; others are left out.
    pub(crate) fn on_release(&self, errand: &str, sessions: Vec<String>) {
        let Some(factory) = self.factory.upgrade() else { return };
        let mut inner = factory.agent_browser.inner();
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
        let factory = self.factory.upgrade()?;
        let inner = factory.agent_browser.inner();
        inner.runner.as_ref().and(inner.download_dir.clone())
    }

    /// Lands a download Chrome finished for the errand (`downloads`).
    pub(crate) fn land(&self, download: crate::errands::downloads::Download) -> crate::errands::downloads::Landing {
        downloads::land(self.factory.clone(), download)
    }

    /// Deletes download `guid` unread.
    pub(crate) fn drop_download(&self, guid: &str) {
        if let Some(factory) = self.factory.upgrade() {
            downloads::drop_download(&factory, guid);
        }
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

    /// Its states as they change.
    pub(crate) fn states(&self) -> Option<watch::Receiver<State>> {
        self.factory.upgrade().map(|factory| factory.agent_browser.states.subscribe())
    }
}

impl crate::errands::cdp::Transport for Handle {
    fn connect(&self) -> Result<crate::errands::cdp::Pipe, String> {
        let factory = self.factory()?;
        let mut inner = factory.agent_browser.inner();
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
        // What the errand sends: to Chrome, only while its agent drives.
        tokio::spawn(async move {
            while let Some(message) = outgoing.recv().await {
                let Some(factory) = weak.upgrade() else { return };
                // The runner's sender, looked up each time and never kept:
                // a copy held here would keep a stopped runner going
                // (Ghiaccio 38462).
                let runner = {
                    let inner = factory.agent_browser.inner();
                    if inner.generation != generation {
                        return;
                    }
                    let Some(runner) = inner.runner.clone() else { return };
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

/// Tests: errand `errand` (of `agent`) drives the browser, Chrome finished
/// a download of `bytes` the page called `suggested`, and the host lands
/// it, through the stand-in's channel 5: its name, the agent's copy, its
/// flag, and whether the runner's copy is gone after.
#[doc(hidden)]
pub async fn land_for_test(
    factory: &Arc<Factory>,
    errand: &str,
    agent: &str,
    suggested: &str,
    bytes: &[u8],
) -> Result<(String, std::path::PathBuf, String), String> {
    let handle = Handle::new(factory);
    handle.begin(errand, None)?;
    let mut dir = None;
    for _ in 0..300 {
        if handle.factory.upgrade().is_some_and(|factory| factory.agent_browser.inner().state == State::Driven) {
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

/// Tests: errand `errand` drives the browser and connects to it, as an
/// errand's agent does; the connection lasts while what's returned is kept.
#[doc(hidden)]
pub async fn connect_for_test(factory: &Arc<Factory>, errand: &str, url: Option<String>) -> Result<Box<dyn std::any::Any + Send>, String> {
    use crate::errands::cdp::Transport;
    let handle = Handle::new(factory);
    handle.begin(errand, url)?;
    for _ in 0..300 {
        if factory.agent_browser.inner().state == State::Driven {
            return Ok(Box::new(handle.connect()?));
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    Err("it never drove".to_owned())
}

/// Tests: the sites errand_open tells an agent the browser is signed in to.
#[doc(hidden)]
pub fn errands_signed_in_for_test(factory: &Arc<Factory>) -> Vec<String> {
    factory.errands.signed_in()
}
