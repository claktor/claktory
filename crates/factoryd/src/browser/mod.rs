//! Browser control's backend (Track B, B3a; browser-control.md §§5–7): read
//! sessions an agent asks for and the person grants on one of their Macs.
//!
//! An agent with the browser extension asks to read a site: a pending
//! session, owned by that agent as it is now. The person opens the site in
//! Chrome on a Mac and confirms there; that Mac's app grants it
//! (BrowserGrant), and from then on commands go to that one connection's own
//! queue (never `Factory::send` or any broadcast), one at a time, each with
//! a deadline, and only that connection's results for the command it was
//! sent are taken. The Mac heartbeats while it's active.
//!
//! Nothing is restored: sessions live in memory, so a restart ends them all,
//! as does the connection closing, a heartbeat lapsing, the limits, or the
//! agent changing (its model, instructions, project or folder, /clear, the
//! extension off, deleted). Every check is made again when a command is
//! queued and when its result comes back.
//!
//! An "assist" session (B3b) may also click, fill and go to another page of
//! the site. The person reviews each such action on the Mac, bound to its
//! exact JSON, session and sequence (the helper checks the hash); the
//! decision stays there, and the backend neither sees nor sends one. An
//! action the backend stops waiting for may have happened anyway: it
//! answers outcome_unknown and is never sent again.
//!
//! What happened is logged for 7 days without what was typed or read
//! (`log`), and browser control turned on for an agent turns itself off
//! again once it's been used, or not, in a while (`activation`).
//!
//! Off for everyone, Rivet included, until B6a's security proof passes: the
//! extension doesn't exist outside a preview (`set_preview`).

pub(crate) mod activation;
pub(crate) mod log;
pub(crate) mod origin;
pub(crate) mod schema;

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, Weak};
use std::time::{Duration, Instant};

use factory_protocol::v1::{self, server_message::Body};
use tokio::sync::{mpsc, oneshot};

use crate::factory::{Factory, RequestError};
use crate::transcript::now_ms;

/// Browser control's tables (B3b): its log, and the activations that turn
/// it off again.
pub(crate) const SCHEMA: &str = "
-- Browser control's log, 7 days: what happened in each session, never what
-- was typed, which element, a path or page data.
CREATE TABLE browser_log (
    id INTEGER PRIMARY KEY,
    at INTEGER NOT NULL,
    session TEXT NOT NULL,
    agent TEXT NOT NULL,
    -- The paired device it was bound to; '' before.
    device TEXT NOT NULL,
    origin TEXT NOT NULL,
    -- request, grant, end, read, scroll, click, fill or navigate
    op TEXT NOT NULL,
    -- A command's sequence in its session; 0 for the others.
    sequence INTEGER NOT NULL,
    -- The mode asked for, granted, why it ended, done or an error word
    outcome TEXT NOT NULL
);
CREATE INDEX browser_log_by_time ON browser_log (at);
-- Browser control turned on for an agent, until it turns itself off
-- (browser::activation). A new one is a new generation.
CREATE TABLE browser_activations (
    generation INTEGER PRIMARY KEY AUTOINCREMENT,
    agent TEXT NOT NULL UNIQUE,
    -- Its extensions.browser settings when it was turned on, as JSON: the
    -- only ones it turns off from.
    settings TEXT NOT NULL,
    activated INTEGER NOT NULL,
    -- 1 once it asked for a session.
    used INTEGER NOT NULL
);
";

static PREVIEW: AtomicBool = AtomicBool::new(false);

/// Turns browser control on for this process: in the dev Claktory
/// (FACTORY_BROWSER_PREVIEW=1) and tests, until B6a passes.
#[doc(hidden)]
pub fn set_preview(on: bool) {
    PREVIEW.store(on, Ordering::Relaxed);
}

pub(crate) fn preview() -> bool {
    PREVIEW.load(Ordering::Relaxed)
}

/// How long things may take. Tests shorten them.
#[derive(Clone, Copy, Debug)]
#[doc(hidden)]
pub struct Limits {
    /// A pending session waits this long for the person.
    pub pending: Duration,
    /// An active session ends this long after it started.
    pub hard: Duration,
    /// An active session ends after this long without a command.
    pub idle: Duration,
    /// An active session ends after this long without a heartbeat.
    pub heartbeat: Duration,
    pub commands: u32,
    pub bytes: u64,
    /// A read's deadline, and a scroll's.
    pub read: Duration,
    pub scroll: Duration,
    /// A click's, fill's or navigation's: the person's review on the Mac
    /// (2 minutes), going back to the tab (10 s), doing it, and a margin.
    pub action: Duration,
    /// Browser control turned on for an agent that hasn't asked for a
    /// session turns off after this long (`activation`).
    pub activation: Duration,
    /// Whether activations turn browser control off at all: tests that ask
    /// for one session after another leave it on.
    pub auto_off: bool,
    /// How long an ended session stays listed.
    pub keep_ended: Duration,
}

impl Default for Limits {
    fn default() -> Self {
        Limits {
            pending: Duration::from_secs(5 * 60),
            hard: Duration::from_secs(15 * 60),
            idle: Duration::from_secs(5 * 60),
            heartbeat: Duration::from_secs(15),
            commands: 100,
            bytes: 2 * 1024 * 1024,
            read: Duration::from_secs(10),
            scroll: Duration::from_secs(5),
            action: Duration::from_secs(150),
            activation: Duration::from_secs(5 * 60),
            auto_off: true,
            keep_ended: Duration::from_secs(60),
        }
    }
}

/// A connection's queue holds this many commands and endings.
const QUEUE: usize = 8;
/// What a Mac may say a session ended for; anything else reads "stopped".
const STOP_REASONS: &[&str] = &["stopped", "declined", "tab_closed", "origin_changed", "chrome_closed", "helper_gone", "app_quit", "time_limit", "interrupted"];
const PURPOSE_MAX: usize = 500;
/// What a Mac's broker may say it can do.
const OPERATIONS: &[&str] = &["read", "scroll", "click", "fill", "navigate"];

/// What an agent may do in a session: read (and scroll), or assist: also
/// click, fill and navigate, each once the person allows it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Mode {
    Read,
    Assist,
}

impl Mode {
    pub(crate) fn parse(mode: &str) -> Result<Mode, String> {
        match mode {
            "" | "read" => Ok(Mode::Read),
            "assist" => Ok(Mode::Assist),
            other => Err(format!("No mode \"{other}\": read or assist.")),
        }
    }

    pub(crate) fn word(self) -> &'static str {
        match self {
            Mode::Read => "read",
            Mode::Assist => "assist",
        }
    }
}

#[derive(Default)]
pub(crate) struct Browser {
    state: Mutex<State>,
    limits: Mutex<Limits>,
    next_port: AtomicU64,
    /// Wakes the watch over deadlines.
    wake: Arc<tokio::sync::Notify>,
}

#[derive(Default)]
struct State {
    ports: HashMap<u64, Slot>,
    sessions: Vec<Session>,
    next_command: u64,
    /// Log entries not written yet (`log::flush`).
    log: Vec<v1::BrowserLogEntry>,
}

/// One connection, as browser control knows it.
struct Slot {
    /// The paired device it's authenticated as.
    device: String,
    capability: Option<Capability>,
    sender: mpsc::Sender<Body>,
}

struct Capability {
    instance: String,
    helper_ready: bool,
    operations: Vec<String>,
}

/// Who a session is for: an agent as it was when it asked. Any change to
/// these ends it.
#[derive(Clone, PartialEq, Eq, Debug)]
pub(crate) struct Owner {
    agent: String,
    /// Its conversation: /clear starts a new one (`Runtime::epoch`).
    epoch: u64,
    /// Its model, instructions, project and folder.
    settings: String,
}

enum Phase {
    Pending,
    Active { port: u64, device: String, lease: String, started: Instant, last_command: Instant, last_heartbeat: Instant },
    Ended { at: Instant, at_ms: i64, reason: String, device: String },
}

struct Session {
    id: String,
    owner: Owner,
    purpose: String,
    origin: String,
    mode: Mode,
    created: Instant,
    created_ms: i64,
    phase: Phase,
    commands: u32,
    bytes: u64,
    sequence: u64,
    in_flight: Option<InFlight>,
}

struct InFlight {
    command_id: u64,
    /// "click", "fill" or "navigate" for an action; "" otherwise.
    action: &'static str,
    waiter: oneshot::Sender<Result<String, String>>,
}

fn guard<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

fn refused(why: impl Into<String>) -> RequestError {
    RequestError::Remote(why.into())
}

/// Random bytes as base64url, for ids.
fn random(bytes: usize) -> String {
    let mut random = vec![0u8; bytes];
    getrandom::fill(&mut random).expect("the system's random source");
    base64::Engine::encode(&base64::engine::general_purpose::URL_SAFE_NO_PAD, random)
}

fn ms(duration: Duration) -> i64 {
    i64::try_from(duration.as_millis()).unwrap_or(i64::MAX)
}

impl Browser {
    fn state(&self) -> MutexGuard<'_, State> {
        guard(&self.state)
    }

    pub(crate) fn limits(&self) -> Limits {
        *guard(&self.limits)
    }

    /// The sessions for Workspace: a status, never page data.
    pub(crate) fn view(&self) -> Vec<v1::BrowserSession> {
        let limits = self.limits();
        let now = Instant::now();
        let at = |instant: Instant| now_ms() - ms(now.saturating_duration_since(instant)) + ms(instant.saturating_duration_since(now));
        self.state()
            .sessions
            .iter()
            .map(|session| {
                let mut view = v1::BrowserSession {
                    id: session.id.clone(),
                    agent_id: session.owner.agent.clone(),
                    purpose: session.purpose.clone(),
                    origin: session.origin.clone(),
                    mode: session.mode.word().to_owned(),
                    action: session.in_flight.as_ref().map(|in_flight| in_flight.action.to_owned()).unwrap_or_default(),
                    created_ms: session.created_ms,
                    commands_used: session.commands,
                    commands_limit: limits.commands,
                    bytes_returned: session.bytes,
                    bytes_limit: limits.bytes,
                    ..Default::default()
                };
                match &session.phase {
                    Phase::Pending => {
                        view.state = "pending".to_owned();
                        view.deadline_ms = at(session.created + limits.pending);
                    }
                    Phase::Active { device, started, last_command, .. } => {
                        view.state = "active".to_owned();
                        view.device_id = device.clone();
                        view.deadline_ms = at(*started + limits.hard);
                        view.idle_deadline_ms = at(*last_command + limits.idle);
                    }
                    Phase::Ended { at_ms, reason, device, .. } => {
                        view.state = "ended".to_owned();
                        view.reason = reason.clone();
                        view.device_id = device.clone();
                        view.ended_ms = *at_ms;
                    }
                }
                view
            })
            .collect()
    }
}

/// Ends `session` for `reason`: its command fails, and its Mac is told.
/// Whether it was still going.
fn end(state: &mut State, index: usize, reason: &str) -> bool {
    let session = &mut state.sessions[index];
    if matches!(session.phase, Phase::Ended { .. }) {
        return false;
    }
    if let Some(in_flight) = session.in_flight.take() {
        let _ = in_flight.waiter.send(Err("interrupted".to_owned()));
    }
    let device = match &session.phase {
        Phase::Active { port, device, .. } => {
            if let Some(slot) = state.ports.get(port) {
                let ended = v1::BrowserEnded { session_id: session.id.clone(), reason: reason.to_owned() };
                let _ = slot.sender.try_send(Body::BrowserEnded(ended));
            }
            device.clone()
        }
        _ => String::new(),
    };
    tracing::info!(session = %session.id, %reason, "browser session ended");
    let entry = log::entry(session, &device, "end", 0, reason);
    session.phase = Phase::Ended { at: Instant::now(), at_ms: now_ms(), reason: reason.to_owned(), device };
    state.log.push(entry);
    true
}

/// Tells every client the sessions changed, and logs what happened, from
/// outside any lock.
fn changed(factory: &Factory) {
    log::flush(factory);
    factory.broadcast_workspace(&factory.lock());
}

/// One connection's part in browser control. Made for every connection;
/// dropped with it, ending what it was bound to.
pub(crate) struct Port {
    id: u64,
    factory: Weak<Factory>,
    receiver: tokio::sync::Mutex<mpsc::Receiver<Body>>,
}

impl Port {
    pub(crate) fn new(factory: &Arc<Factory>, device: &str) -> Port {
        let id = factory.browser.next_port.fetch_add(1, Ordering::Relaxed);
        let (sender, receiver) = mpsc::channel(QUEUE);
        factory.browser.state().ports.insert(id, Slot { device: device.to_owned(), capability: None, sender });
        Port { id, factory: Arc::downgrade(factory), receiver: tokio::sync::Mutex::new(receiver) }
    }

    fn factory(&self) -> Result<Arc<Factory>, RequestError> {
        let factory = self.factory.upgrade().ok_or_else(|| refused("Claktory is stopping."))?;
        if !preview() {
            return Err(refused("Browser control isn't available yet."));
        }
        Ok(factory)
    }

    /// The next command or ending for this connection's app.
    pub(crate) async fn next(&self) -> Body {
        match self.receiver.lock().await.recv().await {
            Some(body) => body,
            None => std::future::pending().await,
        }
    }

    /// BrowserCapability: this connection's app brokers for its Chrome.
    pub(crate) fn capability(&self, capability: &v1::BrowserCapability) -> Result<(), RequestError> {
        let factory = self.factory()?;
        let instance = capability.broker_instance.as_str();
        if instance.is_empty() || instance.len() > 64 || !instance.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_') {
            return Err(refused("A broker instance is 1 to 64 letters, digits, - or _."));
        }
        if capability.version != 1 {
            return Err(refused(format!("This Claktory speaks browser version 1, not {}.", capability.version)));
        }
        let operations: Vec<String> =
            capability.operations.iter().filter(|op| OPERATIONS.contains(&op.as_str())).take(8).cloned().collect();
        let mut state = factory.browser.state();
        let slot = state.ports.get_mut(&self.id).ok_or_else(|| refused("This connection is closing."))?;
        // A connection brokers as one instance for as long as it lasts.
        if slot.capability.as_ref().is_some_and(|known| known.instance != instance) {
            return Err(refused("This connection already brokers as another instance."));
        }
        slot.capability = Some(Capability { instance: instance.to_owned(), helper_ready: capability.helper_ready, operations });
        Ok(())
    }

    /// BrowserGrant: binds a pending session to this connection, once.
    pub(crate) fn grant(&self, grant: &v1::BrowserGrant) -> Result<(), RequestError> {
        let factory = self.factory()?;
        let origin = origin::canonical(&grant.origin).map_err(|_| refused("That isn't the session's site."))?;
        let lease = grant.lease.as_str();
        let decoded = base64::Engine::decode(&base64::engine::general_purpose::URL_SAFE_NO_PAD, lease).unwrap_or_default();
        if decoded.len() != 16 {
            return Err(refused("A lease is 16 random bytes, in base64url."));
        }
        // Who owns it now, read before the browser's lock (Factory's comes first).
        let agent = factory.browser.state().sessions.iter().find(|session| session.id == grant.session_id).map(|s| s.owner.agent.clone());
        let current = agent.as_deref().and_then(|agent| factory.browser_owner(agent));
        let mut state = factory.browser.state();
        let slot = state.ports.get(&self.id).ok_or_else(|| refused("This connection is closing."))?;
        let device = slot.device.clone();
        let Some(capability) = slot.capability.as_ref() else { return Err(refused("Say BrowserCapability first.")) };
        if capability.instance != grant.broker_instance {
            return Err(refused("This connection doesn't broker as that instance."));
        }
        if !capability.helper_ready {
            return Err(refused("Claktory's helper isn't connected to this app."));
        }
        let Some(index) = state.sessions.iter().position(|session| session.id == grant.session_id) else {
            return Err(refused("No such browser session."));
        };
        let busy = state.sessions.iter().any(|session| matches!(&session.phase, Phase::Active { device: bound, .. } if *bound == device));
        let session = &state.sessions[index];
        if !matches!(session.phase, Phase::Pending) {
            return Err(refused("That browser session isn't waiting for a grant."));
        }
        if current.as_ref() != Some(&session.owner) {
            end(&mut state, index, "agent_changed");
            drop(state);
            changed(&factory);
            return Err(refused("The agent that asked has changed since: the session ended."));
        }
        if session.origin != origin {
            return Err(refused("That isn't the session's site."));
        }
        if busy {
            return Err(refused("This Mac already has a browser session going: one at a time."));
        }
        let now = Instant::now();
        let entry = log::entry(&state.sessions[index], &device, "grant", 0, "granted");
        state.log.push(entry);
        state.sessions[index].phase =
            Phase::Active { port: self.id, device, lease: lease.to_owned(), started: now, last_command: now, last_heartbeat: now };
        tracing::info!(session = %grant.session_id, "browser session granted");
        drop(state);
        factory.browser.wake.notify_one();
        changed(&factory);
        Ok(())
    }

    /// BrowserResult: completes the command this connection was sent.
    pub(crate) fn result(&self, result: &v1::BrowserResult) -> Result<(), RequestError> {
        let factory = self.factory()?;
        let mut state = factory.browser.state();
        let found = state.sessions.iter_mut().find(|session| {
            session.id == result.session_id
                && matches!(session.phase, Phase::Active { port, .. } if port == self.id)
                && session.in_flight.as_ref().is_some_and(|in_flight| in_flight.command_id == result.command_id)
        });
        let Some(session) = found else {
            tracing::warn!(session = %result.session_id, command = result.command_id, length = result.result_json.len(), "refused a browser result nothing waits for here");
            return Err(refused("No command of that session waits on this connection."));
        };
        let in_flight = session.in_flight.take().expect("checked");
        let answer = if !result.failure.is_empty() {
            Err(if schema::ERRORS.contains(&result.failure.as_str()) { result.failure.clone() } else { "interrupted".to_owned() })
        } else if result.result_json.len() > schema::RESULT_MAX {
            tracing::warn!(session = %result.session_id, command = result.command_id, length = result.result_json.len(), "refused a browser result too big");
            Err("too_big".to_owned())
        } else {
            Ok(result.result_json.clone())
        };
        let _ = in_flight.waiter.send(answer);
        Ok(())
    }

    /// BrowserHeartbeat: the bound Mac is still there.
    pub(crate) fn heartbeat(&self, heartbeat: &v1::BrowserHeartbeat) -> Result<(), RequestError> {
        let factory = self.factory()?;
        let mut state = factory.browser.state();
        for session in &mut state.sessions {
            if session.id != heartbeat.session_id {
                continue;
            }
            if let Phase::Active { port, lease, last_heartbeat, .. } = &mut session.phase
                && *port == self.id
                && *lease == heartbeat.lease
            {
                *last_heartbeat = Instant::now();
                return Ok(());
            }
        }
        Err(refused("No browser session of this connection has that lease."))
    }

    /// BrowserStop: from the bound Mac, the session ended there; from any
    /// other of the person's devices, they ask to stop it. Idempotent.
    pub(crate) fn stop(&self, stop: &v1::BrowserStop) -> Result<(), RequestError> {
        let factory = self.factory()?;
        let mut state = factory.browser.state();
        let Some(index) = state.sessions.iter().position(|session| session.id == stop.session_id) else {
            return Err(refused("No such browser session."));
        };
        let bound = matches!(state.sessions[index].phase, Phase::Active { port, .. } if port == self.id);
        let pending = matches!(state.sessions[index].phase, Phase::Pending);
        let reason = match stop.reason.as_str() {
            reason if bound && STOP_REASONS.contains(&reason) => reason,
            "declined" if pending => "declined",
            _ => "stopped",
        };
        let ended = end(&mut state, index, reason);
        drop(state);
        if ended {
            changed(&factory);
        }
        Ok(())
    }
}

impl Drop for Port {
    fn drop(&mut self) {
        let Some(factory) = self.factory.upgrade() else { return };
        let mut state = factory.browser.state();
        let bound: Vec<usize> = (0..state.sessions.len())
            .filter(|index| matches!(state.sessions[*index].phase, Phase::Active { port, .. } if port == self.id))
            .collect();
        let mut ended = false;
        for index in bound {
            ended |= end(&mut state, index, "interrupted");
        }
        state.ports.remove(&self.id);
        drop(state);
        if ended && let Ok(runtime) = tokio::runtime::Handle::try_current() {
            runtime.spawn(async move { changed(&factory) });
        }
    }
}

/// A command's outcome for the agent: the extension's answer, or an error
/// word and why.
pub(crate) type Outcome = Result<schema::Answer, (String, String)>;

fn failed(word: &str, why: impl Into<String>) -> (String, String) {
    (word.to_owned(), why.into())
}

impl Factory {
    /// Who `agent` is now, for a session's owner; None if it's gone or the
    /// extension is off for it.
    pub(crate) fn browser_owner(&self, agent: &str) -> Option<Owner> {
        let inner = self.lock();
        let config = inner.config.agent(agent)?;
        if !crate::extensions::enabled(config, crate::extensions::browser::ID) {
            return None;
        }
        let epoch = inner.agents.get(agent).map_or(0, |runtime| runtime.epoch);
        let settings = format!("{}\n{}\n{}\n{}", config.model, config.prompt, config.project.clone().unwrap_or_default(), config.folder);
        Some(Owner { agent: agent.to_owned(), epoch, settings })
    }

    /// Shortens browser control's limits, for tests.
    #[doc(hidden)]
    pub fn set_browser_limits(&self, limits: Limits) {
        *guard(&self.browser.limits) = limits;
        self.browser.wake.notify_one();
    }

    /// An agent asks to read `origin`, or assist on it: a pending session,
    /// its id.
    pub(crate) fn browser_request(self: &Arc<Self>, agent: &str, purpose: &str, origin: &str, mode: Mode) -> Result<String, String> {
        let owner = self.browser_owner(agent).ok_or("You don't have browser control.")?;
        let origin = origin::canonical(origin)?;
        let purpose = clean_purpose(purpose);
        if purpose.is_empty() {
            return Err("Say what the session is for, in purpose: the person sees it before they agree.".to_owned());
        }
        let mut state = self.browser.state();
        if state.sessions.iter().any(|session| session.owner.agent == agent && !matches!(session.phase, Phase::Ended { .. })) {
            return Err("You already have a browser session: end it (browser_session with action \"end\") before asking for another.".to_owned());
        }
        let id = format!("bs-{}", random(12));
        state.sessions.push(Session {
            id: id.clone(),
            owner,
            purpose,
            origin,
            mode,
            created: Instant::now(),
            created_ms: now_ms(),
            phase: Phase::Pending,
            commands: 0,
            bytes: 0,
            sequence: 0,
            in_flight: None,
        });
        let entry = log::entry(state.sessions.last().expect("just pushed"), "", "request", 0, mode.word());
        state.log.push(entry);
        drop(state);
        activation::used(self, agent);
        tracing::info!(session = %id, %agent, "browser session requested");
        self.browser.wake.notify_one();
        changed(self);
        Ok(id)
    }

    /// `agent`'s session `id` as it stands, for the agent.
    pub(crate) fn browser_status(&self, agent: &str, id: &str) -> Result<v1::BrowserSession, String> {
        self.browser.view().into_iter().find(|session| session.id == id && session.agent_id == agent).ok_or_else(|| no_session(id))
    }

    /// The agent ends its session. Idempotent.
    pub(crate) fn browser_end(self: &Arc<Self>, agent: &str, id: &str) -> Result<(), String> {
        let mut state = self.browser.state();
        let index = state.sessions.iter().position(|session| session.id == id && session.owner.agent == agent).ok_or_else(|| no_session(id))?;
        let ended = end(&mut state, index, "ended");
        drop(state);
        if ended {
            changed(self);
        }
        Ok(())
    }

    /// Ends every session of `agent` now (/clear, deleted).
    pub(crate) fn browser_forget(&self, agent: &str, reason: &str) {
        let mut state = self.browser.state();
        let mut ended = false;
        for index in 0..state.sessions.len() {
            if state.sessions[index].owner.agent == agent {
                ended |= end(&mut state, index, reason);
            }
        }
        drop(state);
        if ended {
            changed(self);
        }
    }

    /// Runs `op` in `agent`'s session `id`, and logs it once it was sent,
    /// with its outcome.
    pub(crate) async fn browser_command(self: &Arc<Self>, agent: &str, id: &str, op: schema::Op) -> Outcome {
        let mut sent = None;
        let outcome = self.browser_send(agent, id, &op, &mut sent).await;
        if let Some(mut entry) = sent {
            entry.outcome = match &outcome {
                Ok(schema::Answer::Failed(word, _)) | Err((word, _)) => word.clone(),
                Ok(_) => "done".to_owned(),
            };
            self.browser.state().log.push(entry);
            if op.is_action() {
                // Its review is over: every device stops showing it.
                changed(self);
            } else {
                log::flush(self);
            }
        }
        outcome
    }

    /// Queues `op` to the session's Mac's connection alone, answered by it
    /// alone, by the op's deadline. `sent` gets its log entry once it's
    /// queued.
    async fn browser_send(self: &Arc<Self>, agent: &str, id: &str, op: &schema::Op, sent: &mut Option<v1::BrowserLogEntry>) -> Outcome {
        let limits = self.browser.limits();
        let owner = self.browser_owner(agent);
        let json = serde_json::to_string(op).expect("an op is JSON");
        let action = op.is_action();
        let deadline = match op {
            schema::Op::Read { .. } => limits.read,
            schema::Op::Scroll { .. } => limits.scroll,
            _ => limits.action,
        };
        let (waiter, answer) = oneshot::channel();
        let command_id;
        {
            let mut state = self.browser.state();
            let index = state.sessions.iter().position(|session| session.id == id && session.owner.agent == agent);
            let Some(index) = index else { return Err(failed("wrong_principal", no_session(id))) };
            if owner.as_ref() != Some(&state.sessions[index].owner) {
                end(&mut state, index, "agent_changed");
                drop(state);
                changed(self);
                return Err(failed("wrong_principal", "You've changed since you asked for this session (or lost browser control), so it ended."));
            }
            if action && state.sessions[index].mode != Mode::Assist {
                return Err(failed(
                    "denied",
                    "This is a read session: to click, fill or go to another page, end it and ask for one with mode \"assist\".",
                ));
            }
            state.next_command += 1;
            command_id = state.next_command;
            let session = &mut state.sessions[index];
            let (port, lease, device) = match &mut session.phase {
                Phase::Pending => return Err(failed("approval_pending", "The person hasn't opened the site and confirmed on their Mac yet.")),
                Phase::Ended { reason, .. } => return Err(failed("expired", format!("This session ended ({reason}). Ask for a new one."))),
                Phase::Active { port, lease, device, last_command, .. } => {
                    *last_command = Instant::now();
                    (*port, lease.clone(), device.clone())
                }
            };
            if session.in_flight.is_some() {
                return Err(failed("limit_exceeded", "One command at a time: wait for the last one's answer."));
            }
            if session.commands >= limits.commands {
                end(&mut state, index, "command_limit");
                drop(state);
                changed(self);
                return Err(failed("limit_exceeded", format!("A session takes at most {} commands; it ended.", limits.commands)));
            }
            if session.bytes >= limits.bytes {
                end(&mut state, index, "data_limit");
                drop(state);
                changed(self);
                return Err(failed("limit_exceeded", "This session has returned all the page data it may; it ended."));
            }
            let session = &mut state.sessions[index];
            session.commands += 1;
            session.sequence += 1;
            let command = v1::BrowserCommand {
                session_id: id.to_owned(),
                lease,
                command_id,
                sequence: session.sequence,
                deadline_ms: u32::try_from(deadline.as_millis()).unwrap_or(u32::MAX),
                op_json: json,
            };
            let entry = log::entry(session, &device, op.name(), session.sequence, "");
            session.in_flight = Some(InFlight { command_id, action: if action { op.name() } else { "" }, waiter });
            let queued = state.ports.get(&port).is_some_and(|slot| {
                let ready = slot.capability.as_ref().is_some_and(|capability| capability.operations.iter().any(|known| known == op.name()));
                ready && slot.sender.try_send(Body::BrowserCommand(command)).is_ok()
            });
            if !queued {
                state.sessions[index].in_flight = None;
                return Err(failed("no_mac", "The Mac couldn't take the command now."));
            }
            *sent = Some(entry);
        }
        if action {
            // Every device shows that an action waits for the person's review.
            changed(self);
        }
        // An action that may have happened, for all the backend knows: never
        // sent again, and the agent reads the page before anything else.
        let unknown = || {
            failed(
                "outcome_unknown",
                "Claktory doesn't know whether it was done: it may have happened. Read the page to see before anything else; it's never sent again.",
            )
        };
        let outcome = tokio::time::timeout(deadline, answer).await;
        // Checked again with the answer: a session that ended meanwhile, or
        // an agent that changed, gets nothing of it.
        let owner_now = self.browser_owner(agent);
        let mut state = self.browser.state();
        let index = state.sessions.iter().position(|session| session.id == id);
        if let Some(index) = index
            && state.sessions[index].in_flight.as_ref().is_some_and(|in_flight| in_flight.command_id == command_id)
        {
            state.sessions[index].in_flight = None;
        }
        let json = match outcome {
            Ok(Ok(Ok(json))) => json,
            Ok(Ok(Err(word))) if action && (word == "too_big" || word == "interrupted") => return Err(unknown()),
            Ok(Ok(Err(word))) if word == "too_big" => return Err(failed("limit_exceeded", "The page's answer was too big.")),
            Ok(Ok(Err(word))) => return Err(failed(&word, "")),
            Ok(Err(_)) if action => return Err(unknown()),
            Ok(Err(_)) => return Err(failed("interrupted", "The Mac's connection ended.")),
            Err(_) if action => {
                // The Mac outlived the review's own deadline without a word:
                // the session can't be trusted to know what it did.
                if let Some(index) = index {
                    end(&mut state, index, "interrupted");
                }
                drop(state);
                changed(self);
                return Err(unknown());
            }
            Err(_) => return Err(failed("expired", "The Mac didn't answer in time.")),
        };
        let Some(index) = index.filter(|index| matches!(state.sessions[*index].phase, Phase::Active { .. })) else {
            return Err(if action { unknown() } else { failed("interrupted", "The session ended before the answer came.") });
        };
        if owner_now.as_ref() != Some(&state.sessions[index].owner) {
            end(&mut state, index, "agent_changed");
            drop(state);
            changed(self);
            return Err(failed("wrong_principal", "You've changed since you asked for this session, so it ended."));
        }
        let answer = match schema::answer(op, &json) {
            Ok(answer) => answer,
            Err(why) => {
                tracing::warn!(session = %id, command = command_id, length = json.len(), %why, "refused a browser result");
                return Err(if action { unknown() } else { failed("interrupted", "The page's answer wasn't one Claktory accepts.") });
            }
        };
        let session = &mut state.sessions[index];
        let length = json.len() as u64;
        // An action's done whatever it brought back: it counts, and the
        // next command finds the limit.
        if !action && session.bytes + length > limits.bytes {
            end(&mut state, index, "data_limit");
            drop(state);
            changed(self);
            return Err(failed("limit_exceeded", "This session has returned all the page data it may; it ended."));
        }
        session.bytes += length;
        drop(state);
        changed(self);
        Ok(answer)
    }

    /// A session's origin, for fencing its answers.
    pub(crate) fn browser_origin(&self, id: &str) -> String {
        self.browser.state().sessions.iter().find(|session| session.id == id).map(|session| session.origin.clone()).unwrap_or_default()
    }
}

fn no_session(id: &str) -> String {
    format!("You have no browser session \"{id}\".")
}

/// The agent's words for what a session is for, as every device shows them:
/// one line, no control, bidi or zero-width characters, bounded.
fn clean_purpose(purpose: &str) -> String {
    let hidden = |c: char| {
        c.is_control()
            || matches!(c, '\u{061C}' | '\u{200B}'..='\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2060}'..='\u{2069}' | '\u{FEFF}')
    };
    let words: String = purpose.chars().map(|c| if hidden(c) { ' ' } else { c }).collect();
    words.split_whitespace().collect::<Vec<_>>().join(" ").chars().take(PURPOSE_MAX).collect()
}

/// Ends sessions at their deadlines, and when their agent changed, and
/// turns browser control off again when its activation is over.
pub(crate) async fn monitor(factory: Weak<Factory>) {
    loop {
        let (tick, wake) = {
            let Some(factory) = factory.upgrade() else { return };
            let tick = if !factory.browser.state().sessions.is_empty() {
                Some(Duration::from_millis(250))
            } else if activation::any(&factory) {
                Some(Duration::from_secs(1))
            } else {
                None
            };
            (tick, factory.browser.wake.clone())
        };
        match tick {
            Some(tick) => tokio::select! {
                () = tokio::time::sleep(tick) => {}
                () = wake.notified() => {}
            },
            None => wake.notified().await,
        }
        let Some(factory) = factory.upgrade() else { return };
        check(&factory);
        activation::sweep(&factory).await;
    }
}

fn check(factory: &Arc<Factory>) {
    let limits = factory.browser.limits();
    let agents: Vec<String> = factory.browser.state().sessions.iter().map(|session| session.owner.agent.clone()).collect();
    let owners: HashMap<String, Option<Owner>> = agents.into_iter().map(|agent| (agent.clone(), factory.browser_owner(&agent))).collect();
    let now = Instant::now();
    let mut state = factory.browser.state();
    let mut ended = false;
    for index in 0..state.sessions.len() {
        let session = &state.sessions[index];
        let changed_owner = owners.get(&session.owner.agent).is_none_or(|now| now.as_ref() != Some(&session.owner));
        let reason = match &session.phase {
            Phase::Ended { .. } => None,
            _ if changed_owner => Some("agent_changed"),
            Phase::Pending => (now >= session.created + limits.pending).then_some("expired"),
            Phase::Active { started, last_command, last_heartbeat, .. } => {
                if now >= *started + limits.hard {
                    Some("time_limit")
                } else if now >= *last_command + limits.idle && session.in_flight.is_none() {
                    Some("idle")
                } else if now >= *last_heartbeat + limits.heartbeat {
                    Some("interrupted")
                } else {
                    None
                }
            }
        };
        if let Some(reason) = reason {
            ended |= end(&mut state, index, reason);
        }
    }
    let before = state.sessions.len();
    state.sessions.retain(|session| !matches!(session.phase, Phase::Ended { at, .. } if now >= at + limits.keep_ended));
    let gone = state.sessions.len() != before;
    drop(state);
    if ended || gone {
        changed(factory);
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn purposes_are_one_plain_line() {
        assert_eq!(super::clean_purpose("  Read\nthe\u{202E}docs\u{200B} page "), "Read the docs page");
        assert_eq!(super::clean_purpose(&"a".repeat(600)).len(), 500);
    }
}
