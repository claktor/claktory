//! An agent's browsing in Claktory's browser (browser-simple.md, Bruno
//! 54290): an agent with the Browser extension (`extensions::browser`)
//! opens its browser itself with browser_open, a session kept as an
//! "errand" record (its id is the session's), and drives it until it
//! closes it, the browser closes or Claktory restarts. It goes to any
//! https site on the internet, never this computer or its network
//! (`fence`); it reads and acts on the page through CDP (`cdp`, `reader`)
//! at a person's pace (`pace`), and hands the person what only they may do
//! (`handoff`, `code`). Every step is logged without its payload (`log`).
//!
//! Each agent has a browser of its own, the host's
//! (`browser_host::Browsers::for_agent`): the agent's steps reach it through
//! the [`cdp::Transport`] it implements, kept in the agent's [`Seat`] with
//! what its browsing holds (its page, its pace). The host is installed with
//! [`install`]; until it is, every browser tool says Claktory's browser
//! isn't available.

// The host (P4) and the protocol's arms come next: parts stay unused until then.
#![allow(dead_code)]

pub(crate) mod cdp;
pub(crate) mod checks;
pub(crate) mod code;
pub(crate) mod downloads;
pub(crate) mod fence;
#[doc(hidden)]
pub use fence::{resolve_for_test, serve_for_test};
pub(crate) mod fill;
pub(crate) mod handoff;
pub(crate) mod keys;
pub(crate) mod log;
pub(crate) mod pace;
pub(crate) mod reader;

use std::sync::Arc;
use std::time::Instant;

use factory_protocol::v1;
pub(crate) use factory_protocol::v1::AgentBrowserState as BrowserState;
use std::sync::Mutex;
use rusqlite::params;
use serde_json::json;

use self::cdp::{Cdp, Transport};
use self::reader::Page;
use crate::browser::schema::{self, Answer, Op};
use crate::factory::Factory;
use crate::transcript::now_ms;

/// Browsing sessions, kept as errands (`records`), their log (`log`), and
/// the agents the person once contained for errands (`errand_agents`, read
/// no more since browser-simple: kept one release, for a rollback).
pub(crate) const SCHEMA: &str = "
CREATE TABLE errands (
    id TEXT PRIMARY KEY,
    agent TEXT NOT NULL,
    purpose TEXT NOT NULL,
    -- The origins it may go to, as JSON.
    sites TEXT NOT NULL,
    brief TEXT NOT NULL,
    -- running, persons_turn, taken_over, done, stopped, cancelled, failed
    state TEXT NOT NULL,
    -- The lead's id, or '' for the person.
    opened_by TEXT NOT NULL,
    -- The handoff waiting for the person, as JSON.
    handoff TEXT,
    created INTEGER NOT NULL,
    ended INTEGER NOT NULL,
    end_reason TEXT NOT NULL
);
CREATE TABLE errand_log (
    id INTEGER PRIMARY KEY,
    at INTEGER NOT NULL,
    errand TEXT NOT NULL,
    agent TEXT NOT NULL,
    origin TEXT NOT NULL,
    path TEXT NOT NULL,
    action TEXT NOT NULL,
    outcome TEXT NOT NULL,
    target TEXT NOT NULL
);
CREATE INDEX errand_log_at ON errand_log (at);
CREATE TABLE errand_agents (agent TEXT PRIMARY KEY, since INTEGER NOT NULL);
";

/// Agents whose conversation has read outside content through Claktory's
/// own tools (web_search, web_fetch, an app with web access) since it last
/// started afresh. Nothing refuses on it since errand_open
/// went (browser-simple): to remove in a later release. Not pipes' taint, which would make every
/// such agent's secrets ask (card 283). Cleared at /clear.
pub(crate) const OUTSIDE_READ: &str = "
CREATE TABLE errand_outside_read (agent TEXT PRIMARY KEY, what TEXT NOT NULL, since INTEGER NOT NULL);
";

/// Whether browser control's old "browser" entries were turned off
/// (`retire_browser_control`): a row once they were. Not in `state`, which
/// the app's own saves rewrite whole.
pub(crate) const RETIRED: &str = "
CREATE TABLE browser_control_retired (at INTEGER NOT NULL);
";

/// The origins whose cached pages Chrome still owes a clear (row 1699,
/// Fugo 50272): recorded when a recovery-codes or two-step handoff is
/// asked, removed once its clear succeeds, and cleared at the browser's
/// next driven start, before its first page, when the browser went away
/// before (a restart, a crash, Stop).
pub(crate) const CACHE_OWED: &str = "
CREATE TABLE errand_cache_owed (origin TEXT PRIMARY KEY, since INTEGER NOT NULL);
";

/// A clear of `origin`'s cached pages is owed (`CACHE_OWED`).
pub(crate) fn owe_cache_clear(factory: &Factory, origin: &str) {
    let owed = factory.store.db().execute(
        "INSERT OR IGNORE INTO errand_cache_owed (origin, since) VALUES (?1, ?2)",
        params![origin, now_ms()],
    );
    if let Err(error) = owed {
        tracing::error!(%origin, %error, "could not record a cache clear owed");
    }
}

/// The origins owed a clear.
fn caches_owed(factory: &Factory) -> Vec<String> {
    crate::store::query(&factory.store.db(), "SELECT origin FROM errand_cache_owed ORDER BY since", [], |row| row.get(0)).unwrap_or_default()
}

/// `agent` read outside content through one of Claktory's tools: `what`,
/// in words for its refusal ("a web page"). The first one is kept.
pub(crate) fn read_outside(factory: &Factory, agent: &str, what: &str) {
    let marked = factory.store.db().execute(
        "INSERT OR IGNORE INTO errand_outside_read (agent, what, since) VALUES (?1, ?2, ?3)",
        params![agent, what, now_ms()],
    );
    if let Err(error) = marked {
        tracing::error!(%agent, %error, "could not record that an agent read outside content");
    }
}

/// Claktory's tools whose results carry another person's words, and those
/// words for the refusal: called with `args` before the tool runs, it
/// marks the agent (`read_outside`) through `pipes::agents::gate`, which
/// every agent's tool call passes (Fugo 50817, row 1721). The web's tools
/// and apps mark in their own handlers (row 1695).
pub(crate) fn outside_tool(tool: &str, args: &serde_json::Value) -> Option<&'static str> {
    match tool {
        "pipe_read" => Some("a pipe's messages"),
        "pipe_status" => Some("a pipe's status, with what its other side shares"),
        // text: false gives only a report's facts, without their words,
        // unless it opens a file.
        "handoff_read" if args.get("text").and_then(serde_json::Value::as_bool) != Some(false) || args.get("file").is_some_and(|file| !file.is_null()) => {
            Some("reports from a pipe")
        }
        "dm_read" => Some("a direct message from another Claktory"),
        _ => None,
    }
}

/// The events that bring another person's words, or what an agent that
/// read them says, mark the agent they're given to (Fugo 50817, row 1721),
/// from `Factory::deliver_queued`, which every event passes: a pipe's
/// messages, an app's events, and an agent's message whose sender has read
/// something from outside.
pub(crate) fn heard(factory: &Factory, recipient: &str, event: &factory_agent::Event) {
    match event.source {
        factory_agent::EventSource::Pipe => read_outside(factory, recipient, &format!("a message from the pipe {}", event.from)),
        // An app's events may carry anyone's words (a bot's messages, an
        // email, a webhook): every app's, the simplest (Fugo 51140).
        factory_agent::EventSource::App => read_outside(factory, recipient, &format!("a message from the app {}", event.from)),
        factory_agent::EventSource::Agent => relayed(factory, recipient, &event.id),
        _ => {}
    }
}

/// `recipient` was given `sender`'s words (its message, a quiet copy of
/// one, a monitor it set): when `sender` has read something from outside
/// (the errand mark, or pipes' taint), so has `recipient`, for errands
/// only (Fugo 50817 #3: nothing more asks for its secrets).
pub(crate) fn relayed(factory: &Factory, recipient: &str, sender: &str) {
    if sender.is_empty() || recipient == sender {
        return;
    }
    if factory.pipes.tainted(sender).is_empty() && outside_read(factory, sender).is_none() {
        return;
    }
    let name = factory.lock().config.agent(sender).map_or_else(|| sender.to_owned(), |agent| agent.name.clone());
    read_outside(factory, recipient, &format!("a message from {name}, who read outside content"));
}

/// What `agent` read from outside since its conversation started afresh,
/// if it did. An unreadable record counts as read.
fn outside_read(factory: &Factory, agent: &str) -> Option<String> {
    use rusqlite::OptionalExtension;
    match factory.store.db().query_row("SELECT what FROM errand_outside_read WHERE agent = ?1", [agent], |row| row.get(0)).optional() {
        Ok(what) => what,
        Err(_) => Some("something from outside".to_owned()),
    }
}

/// /clear: a new conversation has read nothing from outside.
pub(crate) fn clear_outside_read(factory: &Factory, agent: &str) {
    if let Err(error) = factory.store.db().execute("DELETE FROM errand_outside_read WHERE agent = ?1", [agent]) {
        tracing::error!(%agent, %error, "could not clear that an agent read outside content");
    }
}

/// Why a session open when Claktory stopped ended.
const RESTARTED: &str = "Claktory restarted, which closed your browser.";

/// Why nothing is done on a page Chrome warned about (Safe Browsing, Fugo
/// 54339): never worded as Claktory blocking phishing.
pub(crate) const FLAGGED: &str = "this tab shows Chrome's error page, not a site (a page that didn't open, one Claktory kept the browser from, or a site Chrome warned is unsafe), so Claktory won't act on it: go to another page with browser_open";

/// FLAGGED's line for what Claktory won't do there: "sign in", say.
pub(crate) fn on_error_page(wont: &str) -> String {
    FLAGGED.replace("act on it", wont)
}

/// Why a session ends when its agent's Browser extension is turned off.
const TURNED_OFF: &str = "Its browser was turned off.";

/// From `Factory::apply_config`, before `after` replaces `before`: the
/// agents whose browser goes off (the extension turned off, or the agent
/// deleted) have their session ended, by `recover`'s task, since the
/// config's lock is held here. Their Chrome and profile are the host's to
/// close and delete.
pub(crate) fn config_changed(errands: &Errands, before: &crate::config::Config, after: &crate::config::Config) {
    let Some(send) = crate::locking::lock(&errands.turned_off).clone() else { return };
    for agent in &before.agents {
        let on = |agent: &crate::config::AgentConfig| crate::extensions::enabled(agent, crate::extensions::browser::ID);
        if on(agent) && !after.agent(&agent.id).is_some_and(on) {
            let _ = send.send(agent.id.clone());
        }
    }
}

/// Why a session ends when its agent closes its browser.
const CLOSED: &str = "Its agent closed its browser.";

pub use checks::check_timing_for_test;

/// The most sessions the Workspace lists: the open ones and the latest ended.
const LISTED: usize = 20;

/// Browsing sessions, and each agent's browser as its browsing holds it.
#[derive(Default)]
pub(crate) struct Errands {
    /// Every session, newest first (as kept in factory.db's `errands`).
    records: Mutex<Vec<v1::Errand>>,
    /// Where each agent's browser comes from: the host, or a local Chrome
    /// in tests.
    source: Mutex<Option<Arc<dyn Source>>>,
    /// Each agent's browser, once it browsed, by agent.
    seats: Mutex<std::collections::HashMap<String, Arc<Seat>>>,
    /// The sessions waiting to try a page again for a check that the
    /// visitor is a person, by session, and how long they wait.
    checking: Mutex<std::collections::HashMap<String, checks::Waiting>>,
    check_timing: Mutex<checks::Timing>,
    /// How long a session may go unused before it ends.
    unused_after: Mutex<UnusedAfter>,
    /// The agents whose browser was turned off (or who were deleted),
    /// for `recover`'s task to end their sessions (`config_changed`).
    turned_off: Mutex<Option<tokio::sync::mpsc::UnboundedSender<String>>>,
}

/// Where agents' browsers come from.
pub(crate) trait Source: Send + Sync {
    /// `agent`'s own browser, or why it has none, in words for the agent.
    fn for_agent(&self, agent: &str) -> Result<Arc<dyn Browser>, String>;
}

/// The host's browsers, one per agent.
impl Source for crate::browser_host::Browsers {
    fn for_agent(&self, agent: &str) -> Result<Arc<dyn Browser>, String> {
        crate::browser_host::Browsers::for_agent(self, agent).map(|handle| Arc::new(handle) as Arc<dyn Browser>)
    }
}

/// Tests: one browser that every agent gets.
struct Shared(Arc<dyn Browser>);

impl Source for Shared {
    fn for_agent(&self, _agent: &str) -> Result<Arc<dyn Browser>, String> {
        Ok(self.0.clone())
    }
}

/// One agent's browser, and what its browsing holds there.
pub(crate) struct Seat {
    browser: Arc<dyn Browser>,
    /// The session the browser was begun for, until it ends.
    begun: Mutex<Option<String>>,
    /// The driven page, while a session drives it: one tool call at a time.
    live: tokio::sync::Mutex<Option<Live>>,
    /// The address the session last drove (session, address): after a
    /// relaunch, its tab is the one there.
    last: Mutex<Option<(String, String)>>,
    /// Its lane: a session's steps go one at a time, at its person's pace.
    /// Taken before `live`, and held while a step paces, which `live`
    /// isn't: the person's Stop and turns never wait for a step.
    lane: tokio::sync::Mutex<Option<Pacing>>,
    /// When its agent last used it: a session unused for
    /// `Errands::unused_after` ends.
    used: Mutex<Instant>,
}

impl Seat {
    fn new(browser: Arc<dyn Browser>) -> Seat {
        Seat {
            browser,
            begun: Mutex::default(),
            live: tokio::sync::Mutex::default(),
            last: Mutex::default(),
            lane: tokio::sync::Mutex::default(),
            used: Mutex::new(Instant::now()),
        }
    }

    /// Its agent uses it now and until what's returned is dropped.
    fn using(&self) -> Using<'_> {
        *crate::locking::lock(&self.used) = Instant::now();
        Using(self)
    }

    /// The address `errand` last drove, if it's the one it was.
    fn last(&self, errand: &str) -> Option<String> {
        crate::locking::lock(&self.last).as_ref().filter(|(of, _)| of == errand).map(|(_, address)| address.clone())
    }
}

/// A seat in use: used again as it's dropped, so a long step counts to its
/// end.
struct Using<'a>(&'a Seat);

impl Drop for Using<'_> {
    fn drop(&mut self) {
        *crate::locking::lock(&self.0.used) = Instant::now();
    }
}

/// Why signing in and payments wait on a profile with no Safe Browsing
/// lists yet.
pub(crate) const LISTS_MISSING: &str =
    "Claktory's browser is still getting Safe Browsing's lists for this new profile, so signing in and payments wait a few minutes. Try again shortly.";

/// Why a session ends when its agent leaves it unused.
const UNUSED: &str = "Its browser closed after 10 minutes unused.";

/// How long a session may go unused: 10 minutes, shorter in tests.
#[derive(Clone, Copy)]
struct UnusedAfter(std::time::Duration);

impl Default for UnusedAfter {
    fn default() -> UnusedAfter {
        UnusedAfter(std::time::Duration::from_secs(10 * 60))
    }
}

/// Tests: sessions end after `after` unused instead.
#[doc(hidden)]
pub fn unused_after_for_test(factory: &Factory, after: std::time::Duration) {
    *crate::locking::lock(&factory.errands.unused_after) = UnusedAfter(after);
}

/// Ends `errand`, `seat`'s session, once its agent has left it unused for
/// `Errands::unused_after`: never while it waits for the person (their
/// turn, a check that they're a person) or a step runs.
async fn close_when_unused(factory: std::sync::Weak<Factory>, seat: std::sync::Weak<Seat>, errand: String) {
    loop {
        let due = {
            let (Some(factory), Some(seat)) = (factory.upgrade(), seat.upgrade()) else { return };
            *crate::locking::lock(&seat.used) + crate::locking::lock(&factory.errands.unused_after).0
        };
        tokio::time::sleep_until(due.into()).await;
        let (Some(factory), Some(seat)) = (factory.upgrade(), seat.upgrade()) else { return };
        let Some(session) = factory.errands.get(&errand).filter(|session| State::from_wire(session.state).open()) else { return };
        if crate::locking::lock(&seat.begun).as_deref() != Some(errand.as_str()) {
            return;
        }
        let unused_after = crate::locking::lock(&factory.errands.unused_after).0;
        if *crate::locking::lock(&seat.used) + unused_after > Instant::now() {
            continue;
        }
        let waits = State::from_wire(session.state) != State::Running || factory.check_waiting(&errand).is_some();
        if waits || seat.lane.try_lock().is_err() {
            drop(seat.using());
            continue;
        }
        factory.end_errand(&errand, State::Done, UNUSED).await;
        seat.browser.close_if_warm();
        log::write(
            &factory.store,
            &log::Entry { errand, agent: session.agent_id, action: "Closed the browser, unused for 10 minutes".to_owned(), ..Default::default() },
        );
        return;
    }
}

/// What paces an errand's steps: the person it drives as, drawn once for
/// the errand, where its mouse is, and when it has read enough to change
/// the page again.
struct Pacing {
    errand: String,
    person: pace::Person,
    rng: pace::Rng,
    pointer: Option<pace::Point>,
    /// The page the pointer is on: a new connection puts it nowhere.
    page: usize,
    /// No change to the page before then: the time to read what the last
    /// read showed.
    ready_at: Instant,
    /// The words of each document counted as read.
    read: std::collections::HashMap<String, usize>,
}

impl Pacing {
    fn new(errand: &str) -> Pacing {
        let mut rng = pace::Rng::fresh();
        let person = pace::Person::drawn(&mut rng);
        Pacing { errand: errand.to_owned(), person, rng, pointer: None, page: 0, ready_at: Instant::now(), read: Default::default() }
    }
}

/// The connection an errand drives.
struct Live {
    errand: String,
    page: Arc<Page>,
    /// A site the fence kept the page from, to tell the agent once.
    blocked: Arc<Mutex<Option<fence::Blocked>>>,
    /// Its downloads (R9).
    downloads: Arc<downloads::Tracker>,
    /// Whether the fence lets the top frame's redirects hop to a file,
    /// for a file only: while the agent asks for a download.
    hops: Arc<std::sync::atomic::AtomicBool>,
    /// Whether the agent asked for a file itself (browser_download): a PDF
    /// then comes as a file, not into Chrome's viewer.
    fetching: Arc<std::sync::atomic::AtomicBool>,
}

/// An errand's state, as kept.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum State {
    Running,
    PersonsTurn,
    TakenOver,
    Done,
    Stopped,
    Cancelled,
    Failed,
}

impl State {
    const ALL: [State; 7] =
        [State::Running, State::PersonsTurn, State::TakenOver, State::Done, State::Stopped, State::Cancelled, State::Failed];

    pub(crate) fn word(self) -> &'static str {
        match self {
            State::Running => "running",
            State::PersonsTurn => "persons_turn",
            State::TakenOver => "taken_over",
            State::Done => "done",
            State::Stopped => "stopped",
            State::Cancelled => "cancelled",
            State::Failed => "failed",
        }
    }

    pub(crate) fn of(word: &str) -> State {
        State::ALL.into_iter().find(|state| state.word() == word).unwrap_or(State::Failed)
    }

    pub(crate) fn wire(self) -> v1::ErrandState {
        match self {
            State::Running => v1::ErrandState::Running,
            State::PersonsTurn => v1::ErrandState::PersonsTurn,
            State::TakenOver => v1::ErrandState::TakenOver,
            State::Done => v1::ErrandState::Done,
            State::Stopped => v1::ErrandState::Stopped,
            State::Cancelled => v1::ErrandState::Cancelled,
            State::Failed => v1::ErrandState::Failed,
        }
    }

    pub(crate) fn from_wire(state: i32) -> State {
        match v1::ErrandState::try_from(state) {
            Ok(v1::ErrandState::Running) => State::Running,
            Ok(v1::ErrandState::PersonsTurn) => State::PersonsTurn,
            Ok(v1::ErrandState::TakenOver) => State::TakenOver,
            Ok(v1::ErrandState::Done) => State::Done,
            Ok(v1::ErrandState::Stopped) => State::Stopped,
            Ok(v1::ErrandState::Cancelled) => State::Cancelled,
            _ => State::Failed,
        }
    }

    /// Still going: not ended.
    pub(crate) fn open(self) -> bool {
        matches!(self, State::Running | State::PersonsTurn | State::TakenOver)
    }
}

/// Reads an errand row.
fn row(row: &rusqlite::Row<'_>) -> rusqlite::Result<v1::Errand> {
    let sites: String = row.get(3)?;
    let handoff: Option<String> = row.get(7)?;
    let state: String = row.get(5)?;
    Ok(v1::Errand {
        id: row.get(0)?,
        agent_id: row.get(1)?,
        purpose: row.get(2)?,
        sites: serde_json::from_str(&sites).unwrap_or_default(),
        state: State::of(&state).wire() as i32,
        opened_by: row.get(6)?,
        handoff: handoff.and_then(|handoff| serde_json::from_str::<HandoffRow>(&handoff).ok()).map(HandoffRow::wire),
        created_ms: row.get(8)?,
        ended_ms: row.get(9)?,
        end_reason: row.get(10)?,
    })
}

/// A handoff as kept with its errand.
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct HandoffRow {
    pub kind: i32,
    pub what: String,
    pub origin: String,
    pub title: String,
    pub seq: u64,
    pub errand: String,
    /// A code card's field takes digits only (W8).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub numeric: bool,
    /// An agreement card's words, the site's (W8b).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub agreement: String,
    /// A payment card's page: its top-level address as Chrome reported it,
    /// https only, unchanged; its registrable domain in Claktory's words;
    /// and when the person opened it on a device.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub url: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub domain: String,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub opened_ms: i64,
}

fn is_zero(ms: &i64) -> bool {
    *ms == 0
}

impl HandoffRow {
    pub(crate) fn wire(self) -> v1::ErrandHandoff {
        v1::ErrandHandoff {
            errand_id: self.errand,
            kind: self.kind,
            what: self.what,
            origin: self.origin,
            title: self.title,
            seq: self.seq,
            numeric: self.numeric,
            agreement: self.agreement,
            url: self.url,
            domain: self.domain,
            opened_ms: self.opened_ms,
        }
    }
}

const COLUMNS: &str = "id, agent, purpose, sites, brief, state, opened_by, handoff, created, ended, end_reason";

impl Errands {
    /// Reads the kept errands. One still open when Claktory stopped has
    /// lost its browser: it failed, and its agent is told at `recover`.
    /// Reads the errands kept, ending those Claktory's restart cut short:
    /// returned, so their agents are told.
    pub(crate) fn load(&self, store: &crate::store::Store) -> Vec<v1::Errand> {
        let db = store.db();
        let read = db
            .prepare(&format!("SELECT {COLUMNS} FROM errands ORDER BY created DESC"))
            .and_then(|mut statement| statement.query_map([], row)?.collect::<Result<Vec<_>, _>>());
        let mut records = match read {
            Ok(records) => records,
            Err(error) => {
                tracing::warn!(%error, "couldn't read errands");
                return Vec::new();
            }
        };
        let mut cut_short = Vec::new();
        for errand in records.iter_mut().filter(|errand| State::from_wire(errand.state).open()) {
            errand.state = v1::ErrandState::Failed as i32;
            errand.ended_ms = now_ms();
            errand.end_reason = RESTARTED.to_owned();
            errand.handoff = None;
            let failed = db.execute(
                "UPDATE errands SET state = 'failed', handoff = NULL, ended = ?2, end_reason = ?3 WHERE id = ?1",
                params![errand.id, errand.ended_ms, RESTARTED],
            );
            if let Err(error) = failed {
                tracing::warn!(%error, errand = %errand.id, "couldn't end an errand Claktory's restart cut short");
            }
            cut_short.push(errand.clone());
        }
        *crate::locking::lock(&self.records) = records;
        cut_short
    }

    /// Where agents' browsers come from: the host (`install`), or a local
    /// Chrome in tests. From then on browsing is part of this Claktory.
    fn set_source(&self, source: Arc<dyn Source>) {
        AVAILABLE.store(true, std::sync::atomic::Ordering::Relaxed);
        *crate::locking::lock(&self.source) = Some(source);
        crate::locking::lock(&self.seats).clear();
    }

    /// `agent`'s browser, if it has browsed since Claktory started.
    pub(crate) fn seat_of(&self, agent: &str) -> Option<Arc<Seat>> {
        crate::locking::lock(&self.seats).get(agent).cloned()
    }

    /// `errand`'s agent's browser, if it has one.
    fn seat_for(&self, errand: &v1::Errand) -> Option<Arc<Seat>> {
        self.seat_of(&errand.agent_id)
    }

    /// The open session `seat`'s browser was begun for.
    fn begun(&self, seat: &Seat) -> Option<v1::Errand> {
        let begun = crate::locking::lock(&seat.begun).clone()?;
        self.get(&begun).filter(|errand| State::from_wire(errand.state).open())
    }

    /// `agent` doesn't browse any more (its Browser turned off, or
    /// deleted): its seat goes, and with it what followed its browser.
    fn unseat(&self, agent: &str) {
        crate::locking::lock(&self.seats).remove(agent);
    }

    /// The Workspace's errands: the latest, newest first, a handoff with
    /// the seq of its card in its agent's conversation.
    pub(crate) fn listed(&self, inner: &crate::factory::Inner) -> Vec<v1::Errand> {
        let mut listed: Vec<v1::Errand> = crate::locking::lock(&self.records).iter().take(LISTED).cloned().collect();
        for errand in &mut listed {
            let Some(handoff) = errand.handoff.as_mut() else { continue };
            let Some(transcript) = inner.agents.get(&errand.agent_id).and_then(|runtime| runtime.transcript.as_ref()) else { continue };
            let card = transcript.waiting_seqs().into_iter().rev().find(|seq| {
                matches!(
                    transcript.entry(*seq).and_then(|entry| entry.kind.as_ref()),
                    Some(v1::entry::Kind::AskPerson(ask)) if ask.handoff.as_ref().is_some_and(|shown| shown.errand_id == errand.id)
                )
            });
            handoff.seq = card.unwrap_or_default();
        }
        listed
    }

    pub(crate) fn get(&self, id: &str) -> Option<v1::Errand> {
        crate::locking::lock(&self.records).iter().find(|errand| errand.id == id).cloned()
    }

    /// `agent`'s open errand, if any.
    pub(crate) fn open_of(&self, agent: &str) -> Option<v1::Errand> {
        crate::locking::lock(&self.records).iter().find(|errand| errand.agent_id == agent && State::from_wire(errand.state).open()).cloned()
    }

    /// `agent`'s latest errand, open or not.
    pub(crate) fn latest_of(&self, agent: &str) -> Option<v1::Errand> {
        crate::locking::lock(&self.records).iter().find(|errand| errand.agent_id == agent).cloned()
    }

}

impl Factory {
    /// Writes `errand` as it is now, in memory and in factory.db, and
    /// tells the apps.
    pub(crate) fn save_errand(&self, errand: v1::Errand, brief: Option<&str>) {
        {
            let mut records = crate::locking::lock(&self.errands.records);
            match records.iter_mut().find(|kept| kept.id == errand.id) {
                Some(kept) => *kept = errand.clone(),
                None => records.insert(0, errand.clone()),
            }
        }
        let handoff = errand.handoff.as_ref().map(|handoff| {
            serde_json::to_string(&HandoffRow {
                kind: handoff.kind,
                what: handoff.what.clone(),
                origin: handoff.origin.clone(),
                title: handoff.title.clone(),
                seq: handoff.seq,
                errand: handoff.errand_id.clone(),
                numeric: handoff.numeric,
                agreement: handoff.agreement.clone(),
                url: handoff.url.clone(),
                domain: handoff.domain.clone(),
                opened_ms: handoff.opened_ms,
            })
            .unwrap_or_default()
        });
        let state = State::from_wire(errand.state).word();
        let sites = serde_json::to_string(&errand.sites).unwrap_or_default();
        let written = self.store.db().execute(
            "INSERT INTO errands (id, agent, purpose, sites, brief, state, opened_by, handoff, created, ended, end_reason)
             VALUES (?1, ?2, ?3, ?4, COALESCE(?5, ''), ?6, ?7, ?8, ?9, ?10, ?11)
             ON CONFLICT (id) DO UPDATE SET state = ?6, handoff = ?8, ended = ?10, end_reason = ?11",
            params![
                errand.id,
                errand.agent_id,
                errand.purpose,
                sites,
                brief,
                state,
                errand.opened_by,
                handoff,
                errand.created_ms,
                errand.ended_ms,
                errand.end_reason
            ],
        );
        if let Err(error) = written {
            tracing::warn!(%error, errand = %errand.id, "couldn't keep an errand");
        }
        self.broadcast_workspace(&self.lock());
    }

    /// `agent`'s browser, got from the host the first time, from then on
    /// followed: the person taking it over or handing it back, stopping
    /// it, or it closing. Err says why it has none, in words for the agent.
    pub(crate) fn seat(self: &Arc<Self>, agent: &str) -> Result<Arc<Seat>, String> {
        if let Some(seat) = self.errands.seat_of(agent) {
            return Ok(seat);
        }
        let source = crate::locking::lock(&self.errands.source).clone().ok_or_else(|| "Claktory's browser isn't available yet.".to_owned())?;
        let browser = source.for_agent(agent)?;
        let mut seats = crate::locking::lock(&self.errands.seats);
        if let Some(seat) = seats.get(agent) {
            return Ok(seat.clone());
        }
        let seat = Arc::new(Seat::new(browser));
        if let Some(states) = seat.browser.states() {
            tokio::spawn(follow(Arc::downgrade(self), Arc::downgrade(&seat), states));
        }
        seats.insert(agent.to_owned(), seat.clone());
        Ok(seat)
    }

    /// `agent`'s browsing session: the open one, or a new one, begun on
    /// its browser. Err says why not, in a line the agent reads.
    pub(crate) fn open_browser(self: &Arc<Self>, agent: &str) -> Result<v1::Errand, String> {
        if let Some(open) = self.errands.open_of(agent) {
            return Ok(open);
        }
        {
            let inner = self.lock();
            let config = &inner.config;
            let Some(me) = config.agent(agent) else {
                return Err("You're no longer in this Claktory.".to_owned());
            };
            if !crate::extensions::enabled(me, crate::extensions::browser::ID) {
                return Err("Your Browser extension is off: only the person, or an admin agent such as Rivet, turns it on.".to_owned());
            }
        }
        if let Some(why) = refused(self, agent) {
            return Err(why.to_owned());
        }
        let seat = self.seat(agent)?;
        let id = format!("browse-{}", now_ms());
        seat.browser.begin(&id, None)?;
        *crate::locking::lock(&seat.begun) = Some(id.clone());
        drop(seat.using());
        tokio::spawn(close_when_unused(Arc::downgrade(self), Arc::downgrade(&seat), id.clone()));
        let session = v1::Errand {
            id: id.clone(),
            agent_id: agent.to_owned(),
            state: v1::ErrandState::Running as i32,
            opened_by: agent.to_owned(),
            created_ms: now_ms(),
            ..Default::default()
        };
        self.save_errand(session.clone(), None);
        log::write(&self.store, &log::Entry { errand: id, agent: agent.to_owned(), action: "Opened the browser".to_owned(), ..Default::default() });
        Ok(session)
    }

    /// browser_close: ends `agent`'s session, if it has one.
    pub(crate) async fn close_browser(&self, agent: &str) -> bool {
        let Some(session) = self.errands.open_of(agent) else { return false };
        self.end_errand(&session.id, State::Done, CLOSED).await;
        log::write(&self.store, &log::Entry { errand: session.id, agent: agent.to_owned(), action: "Closed the browser".to_owned(), ..Default::default() });
        true
    }

    /// Ends `errand` in `state`, with `reason` (Claktory's words, or the
    /// person's for Cancelled), closing its connection.
    pub(crate) async fn end_errand(&self, errand: &str, state: State, reason: &str) {
        let Some(mut record) = self.errands.get(errand) else { return };
        if !State::from_wire(record.state).open() {
            return;
        }
        record.state = state.wire() as i32;
        record.ended_ms = now_ms();
        record.end_reason = reason.to_owned();
        let handoff = record.handoff.take();
        let ended = record.clone();
        self.save_errand(record, None);
        withdraw_handoffs(self, &ended);
        let Some(seat) = self.errands.seat_for(&ended) else { return };
        let mut live = seat.live.lock().await;
        if live.as_ref().is_some_and(|live| live.errand == errand) {
            // A recovery-codes or two-step card still waiting: what its page
            // showed leaves Chrome's caches all the same (Fugo 1642).
            let page = live.as_ref().map(|live| live.page.clone()).filter(|page| !page.cdp().closed());
            if let (Some(page), Some(handoff)) = (page, handoff.filter(|handoff| shows_secrets(handoff.kind))) {
                clear_caches(self, &page, &handoff.origin).await;
            }
            *live = None;
        }
        drop(live);
        let begun = {
            let mut begun = crate::locking::lock(&seat.begun);
            if begun.as_deref() == Some(errand) { begun.take() } else { None }
        };
        if let Some(begun) = begun {
            seat.browser.end(&begun);
        }
    }

    /// A recovery-codes or two-step handoff of `errand` at `origin` ended:
    /// what its page showed (the codes, a setup key) leaves Chrome's caches
    /// on disk, whatever the outcome, so a site that sent it cacheable
    /// doesn't leave it in the errands' profile (Fugo 1642).
    pub(crate) async fn forget_cached(&self, errand: &str, origin: &str) {
        let Some(errand) = self.errands.get(errand).filter(|errand| State::from_wire(errand.state).open()) else { return };
        let Some(seat) = self.errands.seat_for(&errand) else { return };
        let mut live = seat.live.lock().await;
        let page = self.page(&seat, &mut live, &errand).await;
        drop(live);
        match page {
            Ok(page) => clear_caches(self, &page, origin).await,
            Err((_, why)) => tracing::warn!(errand = %errand.id, %why, "couldn't clear the browser's caches after a handoff"),
        }
    }

    /// The page `errand` drives in `seat`'s browser, connected when it
    /// isn't.
    async fn page(&self, seat: &Seat, live: &mut Option<Live>, errand: &v1::Errand) -> Result<Arc<Page>, (String, String)> {
        if let Some(live) = live.as_ref().filter(|live| live.errand == errand.id && !live.page.cdp().closed()) {
            live.page.set_two_step(code::two_step_origins(self, &errand.id));
            return Ok(live.page.clone());
        }
        *live = None;
        let transport = seat.browser.clone();
        // Starting (for the errand, or driven again after the person's
        // turn): connected once it's driven.
        if let Some(mut states) = transport.states() {
            let settled = tokio::time::timeout(STARTING, states.wait_for(|state| *state != BrowserState::Starting)).await;
            let driven = matches!(settled, Ok(Ok(state)) if *state == BrowserState::Driven);
            if !driven {
                return Err(("interrupted".to_owned(), "Claktory's browser isn't driven now: try again in a moment".to_owned()));
            }
        }
        transport
            .bootstrapped()
            .await
            .map_err(|why| ("interrupted".to_owned(), format!("Claktory's browser didn't start hidden ({})", schema::one_line(&why, 200))))?;
        let pipe = transport.connect().map_err(|why| ("interrupted".to_owned(), why))?;
        let cdp = Arc::new(Cdp::new(pipe));
        let gone = |error: cdp::Error| ("interrupted".to_owned(), schema::one_line(&error.to_string(), 200));
        // The tab the person sees, never a page the tab strip doesn't show.
        let last = seat.last(&errand.id);
        let page = Arc::new(Page::shown(cdp.clone(), last.as_deref()).await.map_err(gone)?);
        let session = page.session().to_owned();
        // Detached when the agent stops driving, so the fence's pauses end
        // with its turn (Giorno 37215).
        transport.on_release(&errand.id, vec![session]);
        let blocked = Arc::new(Mutex::new(None));
        let hops = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let fetching = Arc::new(std::sync::atomic::AtomicBool::new(false));
        fence::guard(page.clone(), blocked.clone(), hops.clone(), fetching.clone()).await.map_err(gone)?;
        // Downloads go to the runner's folder, each under its GUID, never a
        // name the page chose (R9); none where it has no folder.
        let behavior = match transport.download_dir() {
            Some(folder) => json!({"behavior": "allowAndName", "downloadPath": folder, "eventsEnabled": true}),
            None => json!({"behavior": "deny"}),
        };
        cdp.call("Browser.setDownloadBehavior", behavior, None).await.map_err(gone)?;
        let tracker = Arc::new(downloads::Tracker::default());
        downloads::watch(cdp, tracker.clone(), transport.clone());
        page.set_two_step(code::two_step_origins(self, &errand.id));
        // Clears owed since the browser went away mid-handoff, before its
        // first page (row 1699).
        for origin in caches_owed(self) {
            clear_caches(self, &page, &origin).await;
        }
        // A driven start opens only a blank page: back to where the errand
        // was, through the fence and with downloads set, as a navigation of
        // its own would.
        if let Some(last) = &last
            && page.frame().await.is_ok_and(|frame| frame.url == "about:blank")
        {
            let _ = fence::go(&page, last).await;
        }
        *live = Some(Live { errand: errand.id.clone(), page: page.clone(), blocked, downloads: tracker, hops, fetching });
        Ok(page)
    }

    /// `agent`'s errand while it may drive: running.
    pub(crate) fn driving(&self, agent: &str) -> Result<v1::Errand, (String, String)> {
        let person = self.lock().config.person().to_owned();
        let Some(errand) = self.errands.open_of(agent) else {
            return Err(("no_browser".to_owned(), "open a page with browser_open first".to_owned()));
        };
        match State::from_wire(errand.state) {
            State::Running => Ok(errand),
            State::PersonsTurn => Err(("paused".to_owned(), format!("the browser is {person}'s until they answer your handoff request"))),
            _ => Err(("paused".to_owned(), format!("{person} took the browser over: wait until they hand it back"))),
        }
    }

    /// Runs `op` for `agent`'s errand: the page's answer, fenced as a
    /// stranger's words, with the agent tainted by the site first, and the
    /// downloads it made; or the failure, as the extension's are.
    ///
    /// A check that the visitor is a person that doesn't clear by itself
    /// makes the errand wait and try again later (W5): its steps meanwhile
    /// are refused.
    pub(crate) async fn errand_step(self: &Arc<Self>, agent: &str, step: Step) -> Result<(String, serde_json::Value), String> {
        let errand = self.driving(agent).map_err(said)?;
        if let Some(waiting) = self.check_waiting(&errand.id) {
            return Err(said(("paused".to_owned(), waiting)));
        }
        let seat = self.seat(agent)?;
        let _using = seat.using();
        // One step at a time, the lane before the connection.
        let mut lane = seat.lane.lock().await;
        if lane.as_ref().is_some_and(|pacing| pacing.errand != errand.id) {
            *lane = None;
        }
        let pacing = lane.get_or_insert_with(|| Pacing::new(&errand.id));
        let mut live = seat.live.lock().await;
        let page = self.page(&seat, &mut live, &errand).await.map_err(said)?;
        let Some((stops, tracker, hops, fetching)) =
            live.as_ref().map(|live| (live.blocked.clone(), live.downloads.clone(), live.hops.clone(), live.fetching.clone()))
        else {
            return Err(said(("interrupted".to_owned(), "the browser's connection went".to_owned())));
        };
        // Paced outside the connection's lock: the person's Stop or turn
        // ends the connection mid-step, and the step stops at its next
        // event.
        drop(live);
        let earlier = crate::locking::lock(&stops).take();
        if pacing.page != Arc::as_ptr(&page) as usize {
            pacing.page = Arc::as_ptr(&page) as usize;
            pacing.pointer = None;
        }
        let browser = Some(seat.browser.clone());
        let still = {
            let (factory, id, page, person) = (self.clone(), errand.id.clone(), page.clone(), self.lock().config.person().to_owned());
            let browser = browser.clone();
            move || -> Result<(), (String, String)> {
                match factory.errands.get(&id).map(|errand| State::from_wire(errand.state)) {
                    Some(State::Running) => {}
                    Some(State::PersonsTurn) => return Err(("paused".to_owned(), format!("the browser is {person}'s until they answer your handoff request"))),
                    Some(State::TakenOver) => return Err(("paused".to_owned(), format!("{person} took the browser over: wait until they hand it back"))),
                    _ => return Err(("interrupted".to_owned(), "your browser closed".to_owned())),
                }
                if page.cdp().closed() {
                    return Err(("interrupted".to_owned(), "the browser closed".to_owned()));
                }
                if browser.as_ref().and_then(|browser| browser.states()).is_some_and(|states| *states.borrow() != BrowserState::Driven) {
                    return Err(("paused".to_owned(), "Claktory's browser isn't driven now".to_owned()));
                }
                Ok(())
            }
        };
        let mut hand = reader::Hand { person: &pacing.person, rng: &mut pacing.rng, pointer: &mut pacing.pointer, still: &still };
        // A change to the page waits until what was read last had a
        // person's time to be read (R11.4).
        let changes = matches!(step, Step::Scroll { .. } | Step::Act(_) | Step::Dismiss { .. } | Step::Navigate { .. } | Step::Download { .. });
        if changes {
            hand.wait_until(pacing.ready_at).await.map_err(said)?;
        }
        // Agent input (Input.*) reaches Chrome only under a lease (R11.3
        // #5), held for this step alone and for the longest it may take: a
        // fill types at the person's pace.
        let lease = |until: std::time::Duration| match &browser {
            Some(browser) => browser.input_lease(until).map_err(|why| ("interrupted".to_owned(), schema::one_line(&why, 200))),
            None => Ok(None),
        };
        let until = match &step {
            Step::Act(Op::Fill { text, .. }) => pace::typing_time(&pacing.person, text) * 2 + ONE_ACTION,
            _ => ONE_ACTION,
        };
        let (action, target) = step.logged();
        // A download is the agent's only when it asked for one: a file, or
        // a click, which may start one (R9).
        let asks_file = matches!(step, Step::Download { .. } | Step::Act(Op::Click { .. }));
        let before = tracker.count();
        // Cleared whenever the step ends, finished or dropped midway (an
        // errand stopped, the connection gone): never left on for the next
        // step (Fugo 41301).
        let asking = asks_file.then(|| FileAsk::new(hops, fetching, tracker.clone(), matches!(step, Step::Download { .. })));
        // Read, scroll and act only on a site on the internet, or a blank
        // page: never a tab somewhere else the fence didn't see it go (Fugo
        // 38499).
        let elsewhere = match &step {
            Step::Act(_) | Step::Dismiss { .. } if page.flagged().await => Some(("error_page".to_owned(), FLAGGED.to_owned())),
            Step::Read { .. } | Step::Scroll { .. } | Step::Act(_) | Step::Dismiss { .. } => match page.elsewhere().await {
                Ok(None) => None,
                Ok(Some(origin)) => Some(("off_site".to_owned(), format!("this tab is on {origin}, where Claktory's browser doesn't go: go to another page with browser_open"))),
                Err(error) => Some(("interrupted".to_owned(), schema::one_line(&error.to_string(), 200))),
            },
            Step::Navigate { .. } | Step::Download { .. } => None,
        };
        let outcome = match (&step, elsewhere) {
            (_, Some(refused)) => Err(refused),
            (Step::Read { limit }, None) => page.read(*limit).await.map(Some),
            (Step::Scroll { direction, pages }, None) => match lease(until) {
                Ok(_held) => page.scroll(&mut hand, *direction, *pages).await.map(Some),
                Err(refused) => Err(refused),
            },
            (Step::Act(op), None) => match self.while_signing_out(seat.browser.as_ref(), &page).await {
                Some(refused) => Err(refused),
                None => match lease(until) {
                    Ok(_held) => page.act(&mut hand, op).await.map(Some),
                    Err(refused) => Err(refused),
                },
            },
            (Step::Dismiss { document }, None) => match self.while_signing_out(seat.browser.as_ref(), &page).await {
                Some(refused) => Err(refused),
                None => match lease(until) {
                    Ok(_held) => page.dismiss(&mut hand, document).await.map(Some),
                    Err(refused) => Err(refused),
                },
            },
            (Step::Navigate { address }, None) => fence::go(&page, address).await.map(Some),
            (Step::Download { address }, None) => fence::fetch(&page, address).await.map(|()| None),
        };
        // What a read showed that's new takes a person's time to read
        // before the next change.
        if let Ok(Some(Answer::Page(read))) = &outcome {
            let words = pace::words(&read.text);
            let counted = pacing.read.entry(read.document.clone()).or_default();
            if words > *counted {
                let reading = pace::reading(&mut pacing.rng, &pacing.person, words - *counted);
                *counted = words;
                pacing.ready_at = pacing.ready_at.max(Instant::now() + reading);
            }
        }
        drop(lane);
        let mut began = false;
        if asks_file {
            let wait = if matches!(step, Step::Download { .. }) { downloads::BEGINS } else { downloads::AFTER_CLICK };
            let until = Instant::now() + wait;
            tracker.expect(until);
            began = outcome.is_ok() && tracker.begun_since(before, until).await;
        }
        drop(asking);
        // What the fence stopped during this step is said in its answer.
        let during = crate::locking::lock(&stops).take();
        let frame = page.frame().await.ok();
        let error_page = frame.as_ref().is_some_and(|frame| frame.url.starts_with("chrome-error:"));
        let during_fence = during.is_some();
        let stopped = during_fence && error_page;
        let blocked = during.or(earlier);
        if let Some(frame) = &frame {
            *crate::locking::lock(&seat.last) = Some((errand.id.clone(), frame.url.clone()));
        }
        let url = frame.as_ref().and_then(|frame| url::Url::parse(&frame.url).ok());
        let origin = url.as_ref().map(|url| url.origin().ascii_serialization()).filter(|origin| origin != "null").unwrap_or_default();
        let path = url.as_ref().map(fence::path_of).unwrap_or_default();
        // Asked for a file and got a page: a check that it's a person, or
        // not a file at all.
        let outcome = match outcome {
            // Blocked, and not by the fence: Chrome's Safe Browsing.
            Err((blocked, why)) if blocked == fence::BLOCKED => match during_fence {
                true => Err(("outcome_unknown".to_owned(), why)),
                false => Err(("unsafe_site".to_owned(), fence::DANGEROUS.to_owned())),
            },
            Ok(None) if !began => Err(("not_a_file".to_owned(), "that address opened a page, not a file: read it with browser_read".to_owned())),
            outcome => outcome,
        };
        // What a dismiss cleared, in Claktory's words: never the page's.
        let dismissed = match &outcome {
            Ok(Some(Answer::Dismissed(dismissed))) => Some(dismissed),
            _ => None,
        };
        let action = match dismissed {
            Some(dismissed) => format!("Cleared a {}{}", dismissed.kind, if dismissed.escape { " with Escape" } else { "" }),
            None => action,
        };
        log::write(
            &self.store,
            &log::Entry {
                errand: errand.id.clone(),
                agent: agent.to_owned(),
                origin: origin.clone(),
                path: path.clone(),
                action,
                outcome: outcome.as_ref().err().map(|(error, why)| format!("{error}: {why}")).unwrap_or_default(),
                target: target.unwrap_or_default(),
            },
        );
        // Cookies taken because the notice offered nothing else are said on
        // their own line (Bruno 40530).
        if dismissed.is_some_and(|dismissed| dismissed.accepted_cookies) {
            log::write(
                &self.store,
                &log::Entry {
                    errand: errand.id.clone(),
                    agent: agent.to_owned(),
                    origin: origin.clone(),
                    path,
                    action: "Accepted the site's cookies: its notice offered no other choice".to_owned(),
                    ..Default::default()
                },
            );
        }
        // A check that the visitor is a person, on arrival: often it clears
        // by itself in a few seconds; otherwise it's the person's, before
        // anything is filled. Chrome's error page, where a stopped page
        // lands, shows none.
        let moved = !began
            && !error_page
            && match &outcome {
                Ok(_) => matches!(step, Step::Read { .. } | Step::Navigate { .. } | Step::Act(Op::Click { .. })),
                Err((error, _)) => error == "not_a_file",
            };
        if moved && page.check().await.is_some() {
            let until = Instant::now() + crate::locking::lock(&self.errands.check_timing).clears;
            while page.check().await.is_some() && Instant::now() < until {
                tokio::time::sleep(std::time::Duration::from_millis(500)).await;
            }
            let site = if origin.is_empty() { "The page" } else { origin.as_str() };
            if page.check().await.is_some() {
                let address = frame.as_ref().map(|frame| frame.url.clone()).unwrap_or_default();
                return Ok((self.check_wait(&errand, agent, &origin, &address), json!({})));
            }
            return Ok((format!("{site} showed a check that you're a person, which cleared by itself: read the page again."), json!({})));
        }
        // The downloads it made, or earlier ones that finished since.
        let until = if began { Instant::now() + downloads::FINISHES } else { Instant::now() };
        let (finished, still) = tracker.settled(until).await;
        let landed = self.land_downloads(&errand, agent, &origin, finished, still).await;
        let notice = blocked.map(|blocked| format!("{}\n", blocked.said())).unwrap_or_default();
        let files = if landed.is_empty() { String::new() } else { format!("\n{}", fenced("downloads", &landed.join("\n"))) };
        let answer = outcome.map_err(|failure| format!("{notice}{}{files}", said(failure)))?;
        let read_page = matches!(answer, Some(Answer::Page(_) | Answer::Filled(_) | Answer::Navigated(_) | Answer::Dismissed(_)));
        if !origin.is_empty() && (read_page || !landed.is_empty()) {
            // As web_fetch: the agent keeps its commands and shell secrets
            // (browser-simple.md, "The browsing agent keeps…").
            read_outside(self, agent, "a web page in Claktory's browser");
        }
        let shown = if origin.is_empty() { "about:blank".to_owned() } else { origin };
        // A step whose page the fence stopped is only that: Chrome's error
        // page in its place says nothing more.
        let page_said = answer.filter(|_| !stopped).map(|answer| schema::fenced(&shown, &answer)).unwrap_or_default();
        Ok((format!("{notice}{page_said}{files}").trim_start_matches('\n').to_owned(), json!({})))
    }

    /// browser_upload: attaches `path` (in `agent`'s own folder or its
    /// uploads folder, by canonical path, so a link can't lead out) to the file
    /// field `target` of `document`, through the browser's host, which
    /// copies it where Chrome can read it. What the agent is told, or why
    /// not.
    pub(crate) async fn browser_upload(self: &Arc<Self>, agent: &str, document: &str, target: &str, path: &str) -> Result<String, String> {
        let (file, roots) = self.uploadable(agent, path)?;
        let errand = self.driving(agent).map_err(said)?;
        if let Some(waiting) = self.check_waiting(&errand.id) {
            return Err(said(("paused".to_owned(), waiting)));
        }
        let seat = self.seat(agent)?;
        let _using = seat.using();
        let _lane = seat.lane.lock().await;
        let mut live = seat.live.lock().await;
        let page = self.page(&seat, &mut live, &errand).await.map_err(said)?;
        drop(live);
        if page.flagged().await {
            return Err(said(("error_page".to_owned(), FLAGGED.to_owned())));
        }
        match page.elsewhere().await {
            Ok(None) => {}
            Ok(Some(origin)) => {
                return Err(said(("off_site".to_owned(), format!("this tab is on {origin}, where Claktory's browser doesn't go: go to another page with browser_open"))));
            }
            Err(error) => return Err(said(("interrupted".to_owned(), schema::one_line(&error.to_string(), 200)))),
        }
        let (named, frame) = page.element(document, target).await.map_err(said)?;
        // A label the read lists for its file input stands for that input.
        let field = "function () { const field = this instanceof HTMLLabelElement ? this.control : this; return field instanceof HTMLInputElement && field.type === 'file' && !field.disabled ? field : null; }";
        let object = page
            .call("Runtime.callFunctionOn", json!({"objectId": named, "functionDeclaration": field}))
            .await
            .ok()
            .and_then(|answer| answer["result"]["objectId"].as_str().map(str::to_owned));
        let Some(object) = object else {
            return Err(said(("unsupported_target".to_owned(), "that element isn't a file field: name the page's file input".to_owned())));
        };
        let name = file.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
        let bytes = std::fs::metadata(&file).map(|meta| meta.len()).unwrap_or_default();
        let origin = url::Url::parse(&frame.origin).map(|url| url.origin().ascii_serialization()).unwrap_or_default();
        let entry = |outcome: String| log::Entry {
            errand: errand.id.clone(),
            agent: agent.to_owned(),
            origin: origin.clone(),
            action: "Attached a file".to_owned(),
            target: format!("{name}, {bytes} bytes"),
            outcome,
            ..Default::default()
        };
        // Chrome gets only the path of the copy the host staged, never the
        // agent's.
        let inside = match seat.browser.upload(&file, &roots).await {
            Ok(inside) => inside,
            Err(why) => {
                log::write(&self.store, &entry(why.clone()));
                return Err(format!("{name} wasn't attached: {why}"));
            }
        };
        let set = page.call("DOM.setFileInputFiles", json!({"files": [inside], "objectId": object})).await;
        if let Err(error) = set {
            let why = schema::one_line(&error.to_string(), 200);
            log::write(&self.store, &entry(why.clone()));
            return Err(said(("interrupted".to_owned(), why)));
        }
        log::write(&self.store, &entry(String::new()));
        Ok(format!("Attached {name} ({}) to the file field. Read the page to see what it did.", downloads::size(bytes)))
    }

    /// The file `path` names, if `agent` may upload it: a file (symlinks
    /// followed) inside its own folder or its uploads folder, at most
    /// `UPLOAD_MAX`.
    fn uploadable(&self, agent: &str, path: &str) -> Result<(std::path::PathBuf, Vec<std::path::PathBuf>), String> {
        let (folder, uploads) = {
            let inner = self.lock();
            let me = inner.config.agent(agent).ok_or("You're no longer in this Claktory.")?;
            (crate::runner::placed(self, &inner.config, me).folder, self.home.agent_dir(agent).join("uploads"))
        };
        let asked = std::path::Path::new(path.trim());
        let full = if asked.is_absolute() { asked.to_path_buf() } else { folder.join(asked) };
        let refused = || format!("{path} isn't a file in your folder or your uploads folder: browser_upload takes only those.");
        let file = std::fs::canonicalize(&full).map_err(|_| refused())?;
        let roots: Vec<std::path::PathBuf> = [folder, uploads].iter().filter_map(|root| std::fs::canonicalize(root).ok()).collect();
        let inside = roots.iter().any(|root| file.starts_with(root));
        let meta = std::fs::metadata(&file).map_err(|_| refused())?;
        if !inside || !meta.is_file() {
            return Err(refused());
        }
        if meta.len() > UPLOAD_MAX {
            return Err(format!("{path} is {}: browser_upload takes at most 100 MB.", downloads::size(meta.len())));
        }
        Ok((file, roots))
    }

    /// browser_sign_in: Claktory types a login from the vault into the
    /// page's `username` and `password` fields (refs of `document`), in a
    /// step of the agent's own (its lane and an input lease held), on a page
    /// Chrome didn't warn about. The vault's part, and the card
    /// a first login asks for, are `fill::sign_in`'s.
    pub(crate) async fn browser_sign_in(
        self: &Arc<Self>,
        agent: &str,
        document: &str,
        username: Option<&str>,
        password: Option<&str>,
        reason: &str,
    ) -> Result<(String, serde_json::Value), String> {
        let errand = self.driving(agent).map_err(said)?;
        if let Some(waiting) = self.check_waiting(&errand.id) {
            return Err(said(("paused".to_owned(), waiting)));
        }
        let seat = self.seat(agent)?;
        let _using = seat.using();
        if seat.browser.lists_missing() {
            return Err(said(("paused".to_owned(), LISTS_MISSING.to_owned())));
        }
        let mut lane = seat.lane.lock().await;
        if lane.as_ref().is_some_and(|pacing| pacing.errand != errand.id) {
            *lane = None;
        }
        let pacing = lane.get_or_insert_with(|| Pacing::new(&errand.id));
        let mut live = seat.live.lock().await;
        let page = self.page(&seat, &mut live, &errand).await.map_err(said)?;
        drop(live);
        if pacing.page != Arc::as_ptr(&page) as usize {
            pacing.page = Arc::as_ptr(&page) as usize;
            pacing.pointer = None;
        }
        if page.flagged().await {
            return Err(said(("error_page".to_owned(), on_error_page("sign in there"))));
        }
        match page.elsewhere().await {
            Ok(None) => {}
            Ok(Some(origin)) => {
                return Err(said(("off_site".to_owned(), format!("this tab is on {origin}, where Claktory's browser doesn't go: go to another page with browser_open"))));
            }
            Err(error) => return Err(said(("interrupted".to_owned(), schema::one_line(&error.to_string(), 200)))),
        }
        let _held = seat.browser.input_lease(SIGNING_IN).map_err(|why| said(("interrupted".to_owned(), schema::one_line(&why, 200))))?;
        let still = {
            let (factory, id, page) = (self.clone(), errand.id.clone(), page.clone());
            move || -> Result<(), (String, String)> {
                if factory.errands.get(&id).map(|errand| State::from_wire(errand.state)) != Some(State::Running) {
                    return Err(("interrupted".to_owned(), "your browser isn't yours to drive now".to_owned()));
                }
                if page.cdp().closed() {
                    return Err(("interrupted".to_owned(), "the browser closed".to_owned()));
                }
                Ok(())
            }
        };
        let mut hand = reader::Hand { person: &pacing.person, rng: &mut pacing.rng, pointer: &mut pacing.pointer, still: &still };
        hand.wait_until(pacing.ready_at).await.map_err(said)?;
        fill::sign_in(self, agent, &page, &mut hand, document, username, password, reason).await
    }

    /// Lands the downloads that finished (`finished`) into the errand
    /// agent's folder through the browser's host: a line on each for the
    /// agent (the host's words; the page's only as the name Claktory gave
    /// it), and one for those `still` going. The log keeps name, size,
    /// origin and hash, never the content.
    async fn land_downloads(&self, errand: &v1::Errand, agent: &str, origin: &str, finished: Vec<downloads::Begun>, still: usize) -> Vec<String> {
        let Some(browser) = self.errands.seat_for(errand).map(|seat| seat.browser.clone()) else { return Vec::new() };
        let mut lines = Vec::new();
        for begun in finished {
            let entry = |action: String, target: String, outcome: String| log::Entry {
                errand: errand.id.clone(),
                agent: agent.to_owned(),
                origin: origin.to_owned(),
                action,
                target,
                outcome,
                ..Default::default()
            };
            if begun.progress == downloads::Progress::Canceled {
                browser.drop_download(&begun.guid);
                lines.push("A download was cancelled, by Chrome (it may have judged the file dangerous) or by Claktory (too big, or too little disk space): nothing was kept.".to_owned());
                log::write(&self.store, &entry("A download was cancelled".to_owned(), String::new(), String::new()));
                continue;
            }
            let download = downloads::Download {
                errand: errand.id.clone(),
                agent: agent.to_owned(),
                guid: begun.guid.clone(),
                suggested: begun.suggested,
                url: begun.url,
                page_origin: origin.to_owned(),
            };
            match browser.land(download).await {
                Ok(landed) => {
                    let flag = if landed.flag.is_empty() {
                        String::new()
                    } else {
                        format!(" It's {}: kept without the right to run, and nothing opens it.", landed.flag)
                    };
                    lines.push(format!(
                        "Saved {} ({}, sha256 {}) in downloads/{}/.{flag}",
                        landed.name,
                        downloads::size(landed.bytes),
                        landed.sha256,
                        errand.id
                    ));
                    let target = format!("{}, {} bytes, sha256 {}", landed.name, landed.bytes, landed.sha256);
                    log::write(&self.store, &entry("Downloaded a file".to_owned(), target, String::new()));
                }
                Err(why) => {
                    browser.drop_download(&begun.guid);
                    lines.push(format!("A download wasn't kept: {why}"));
                    log::write(&self.store, &entry("A download wasn't kept".to_owned(), String::new(), why));
                }
            }
        }
        if still > 0 {
            let them = if still == 1 { "A download is" } else { "Downloads are" };
            lines.push(format!("{them} still going: you're told how it went after your next browser step."));
        }
        lines
    }
}

/// The longest a sign-in step's input lease lasts: a long login typed at a
/// person's pace, with its checks between keys.
const SIGNING_IN: std::time::Duration = std::time::Duration::from_secs(180);

/// The biggest file browser_upload takes (the host's cap too).
const UPLOAD_MAX: u64 = 100 * 1024 * 1024;

/// Why `agent` can't have the browser, in the words Settings shows: an
/// admin agent, or one limited to a pipe or to reading reports. None when it can.
pub(crate) fn refused(factory: &Factory, agent: &str) -> Option<&'static str> {
    if let Some(why) = factory.lock().config.agent(agent).and_then(crate::config::browser_refused) {
        return Some(why);
    }
    if factory.pipes.curator(agent) {
        return Some(REPORTS_REFUSED);
    }
    if factory.pipes.quarantined(agent) {
        return Some(PIPE_REFUSED);
    }
    None
}

/// Whether `update` turns an agent's Browser extension on: the person's
/// alone, or an admin agent's through the settings file;
/// refused from factoryd's own commands here, and from the Windows app by
/// authority.
pub(crate) fn turns_browser_on(update: &v1::UpdateAgent) -> bool {
    update.extension.as_ref().is_some_and(|change| change.id == crate::extensions::browser::ID && change.enabled == Some(true))
}

pub(crate) const PIPE_REFUSED: &str = "Agents limited to a pipe can't use the browser.";
pub(crate) const REPORTS_REFUSED: &str = "Agents that read reports can't use the browser.";

/// How long Claktory's browser may take to start, or to start again driven
/// after the person's turn.
const STARTING: std::time::Duration = std::time::Duration::from_secs(60);

/// Claktory's browser as errands use it: P4's host
/// (`crate::browser_host::Handle`), or a local Chrome in tests, which has
/// no turns, so does nothing but connect.
/// A driven start, finishing.
pub(crate) type Bootstrapped = std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), String>> + Send>>;

pub(crate) trait Browser: Transport {
    /// It starts, or goes on, driven for `errand`, at `url`; Err says why
    /// not (another errand has it, the person is using it).
    fn begin(&self, _errand: &str, _url: Option<String>) -> Result<(), String> {
        Ok(())
    }

    /// The person's turn: `undriven` (a sign-in or a check that they're a
    /// person) starts Chrome again with no automation, at `url`.
    fn handoff(&self, _undriven: bool, _url: Option<&str>) -> Result<(), String> {
        Ok(())
    }

    /// Driven again after the person's Done.
    fn resume(&self) -> Result<(), String> {
        Ok(())
    }

    /// Done once its driven start is (Chrome started hidden, on a blank
    /// page, nothing it restored left open: Giorno 40850); Err says why it
    /// couldn't. At once when there's nothing to wait for.
    fn bootstrapped(&self) -> Bootstrapped {
        Box::pin(async { Ok(()) })
    }

    /// `errand` is over.
    fn end(&self, _errand: &str) {}

    /// Closes it now if it's kept warm, with no session: the last ended
    /// unused.
    fn close_if_warm(&self) {}

    /// The CDP sessions `errand` attached, detached whenever it stops
    /// driving.
    fn on_release(&self, _errand: &str, _sessions: Vec<String>) {}

    /// Its states as they change; None when it has none.
    fn states(&self) -> Option<tokio::sync::watch::Receiver<BrowserState>> {
        None
    }

    /// The folder the driven Chrome saves downloads in (the runner's);
    /// None when it can't download.
    fn download_dir(&self) -> Option<String> {
        None
    }

    /// Lands `download`, which Chrome finished: read, counted, hashed,
    /// stored and copied into its errand agent's folder.
    fn land(&self, _download: downloads::Download) -> downloads::Landing {
        Box::pin(async { Err("Claktory's browser can't download files here.".to_owned()) })
    }

    /// Deletes download `guid` unread.
    fn drop_download(&self, _guid: &str) {}

    /// Makes `file`, a plain file inside one of `roots` (the agent's
    /// folder and its uploads folder), readable by Chrome for
    /// browser_upload: the path of the copy it staged, the only path given
    /// to Chrome, or why not.
    fn upload(&self, _file: &std::path::Path, _roots: &[std::path::PathBuf]) -> Uploading {
        Box::pin(std::future::ready(Err("Claktory's browser can't take files here yet.".to_owned())))
    }

    /// `errand`'s landed download `name`: its stored file, and its row.
    fn stored(&self, _errand: &str, _name: &str) -> Option<(std::path::PathBuf, downloads::StoredRow)> {
        None
    }

    /// Copies a stored file into `folder` as `name`, as landing does: the
    /// copy's path, or why not.
    fn copy_into(&self, _file: &std::path::Path, _folder: &std::path::Path, _name: &str) -> Result<std::path::PathBuf, String> {
        Err("Claktory's browser can't download files here.".to_owned())
    }

    /// Signs the browser out of `site` (an https origin it's signed in
    /// to): its cookies there and under it, and its storage, deleted now,
    /// or while a session drives it, when that session ends or before
    /// Chrome next runs a page. Err says why it couldn't.
    fn sign_out_site(&self, _site: &str) -> SigningOut {
        Box::pin(std::future::ready(Err("Claktory's browser can't sign out here yet.".to_owned())))
    }

    /// Whether a sign-out of `provider` waits to be applied: nothing is
    /// filled or pressed on its sites meanwhile (`fill::while_signing_out`).
    fn signing_out(&self, _provider: &str) -> bool {
        false
    }

    /// Whether its profile has no Safe Browsing lists yet (a new one,
    /// before Chrome fetched them): signing in and payments wait.
    fn lists_missing(&self) -> bool {
        false
    }

    /// Lets the errand's input (Input.*) reach Chrome for one action, at
    /// most `until`, while what's returned is held (R11.3 #5): the host
    /// refuses input outside one. None where nothing gates input (a local
    /// Chrome in tests); Err says why not (it isn't driven now).
    fn input_lease(&self, _until: std::time::Duration) -> Result<Option<InputLease>, String> {
        Ok(None)
    }
}

/// An input lease held (`Browser::input_lease`): it ends when dropped.
pub(crate) type InputLease = Box<dyn std::any::Any + Send>;

/// The longest one step's input lease lasts past its planned time: a
/// click's travel, rest and press, a scroll's bouts, a fill's checks
/// between keys.
const ONE_ACTION: std::time::Duration = std::time::Duration::from_secs(60);

/// A file on its way to the browser (`Browser::upload`).
pub(crate) type Uploading = std::pin::Pin<Box<dyn std::future::Future<Output = Result<String, String>> + Send>>;

/// A sign-out on its way (`Browser::sign_out`).
pub(crate) type SigningOut = std::pin::Pin<Box<dyn std::future::Future<Output = Result<SignedOut, String>> + Send>>;

/// How a sign-out went.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SignedOut {
    /// Its cookies are deleted.
    Applied,
    /// An errand drives the browser: they're deleted when it ends.
    Pending,
}

/// A local Chrome (tests) reads this computer's files itself.
impl Browser for cdp::Local {
    fn upload(&self, file: &std::path::Path, _roots: &[std::path::PathBuf]) -> Uploading {
        Box::pin(std::future::ready(Ok(file.display().to_string())))
    }
}

/// P4's host, as is.
impl Browser for crate::browser_host::Handle {
    fn lists_missing(&self) -> bool {
        crate::browser_host::Handle::lists_missing(self)
    }

    fn close_if_warm(&self) {
        crate::browser_host::Handle::close_if_warm(self);
    }

    fn begin(&self, errand: &str, url: Option<String>) -> Result<(), String> {
        crate::browser_host::Handle::begin(self, errand, url)
    }

    fn bootstrapped(&self) -> Bootstrapped {
        crate::browser_host::Handle::bootstrapped(self)
    }

    fn handoff(&self, undriven: bool, url: Option<&str>) -> Result<(), String> {
        crate::browser_host::Handle::handoff(self, undriven, url)
    }

    fn resume(&self) -> Result<(), String> {
        crate::browser_host::Handle::resume(self)
    }

    fn end(&self, errand: &str) {
        crate::browser_host::Handle::end(self, errand);
    }

    fn on_release(&self, errand: &str, sessions: Vec<String>) {
        crate::browser_host::Handle::on_release(self, errand, sessions);
    }

    fn states(&self) -> Option<tokio::sync::watch::Receiver<BrowserState>> {
        crate::browser_host::Handle::states(self)
    }

    fn download_dir(&self) -> Option<String> {
        crate::browser_host::Handle::download_dir(self)
    }

    fn land(&self, download: downloads::Download) -> downloads::Landing {
        crate::browser_host::Handle::land(self, download)
    }

    fn drop_download(&self, guid: &str) {
        crate::browser_host::Handle::drop_download(self, guid);
    }

    fn stored(&self, errand: &str, name: &str) -> Option<(std::path::PathBuf, downloads::StoredRow)> {
        crate::browser_host::Handle::stored(self, errand, name)
    }

    fn copy_into(&self, file: &std::path::Path, folder: &std::path::Path, name: &str) -> Result<std::path::PathBuf, String> {
        crate::browser_host::Handle::copy_into(self, file, folder, name)
    }

    fn sign_out_site(&self, site: &str) -> SigningOut {
        crate::browser_host::Handle::sign_out_site(self, site)
    }

    fn signing_out(&self, provider: &str) -> bool {
        crate::browser_host::Handle::signing_out(self, provider)
    }

    fn upload(&self, file: &std::path::Path, roots: &[std::path::PathBuf]) -> Uploading {
        crate::browser_host::Handle::upload(self, file, roots)
    }

    fn input_lease(&self, until: std::time::Duration) -> Result<Option<InputLease>, String> {
        crate::browser_host::Handle::input_lease(self, until).map(|lease| Some(Box::new(lease) as InputLease))
    }
}

/// Installs where agents' browsers come from (`Factory::seat`).
pub(crate) fn install(factory: &Arc<Factory>, source: Arc<dyn Source>) {
    factory.errands.set_source(source);
}

/// What `seat`'s browser's state means for the session it was begun for;
/// until the seat goes.
async fn follow(factory: std::sync::Weak<Factory>, seat: std::sync::Weak<Seat>, mut states: tokio::sync::watch::Receiver<BrowserState>) {
    while states.changed().await.is_ok() {
        let state = *states.borrow_and_update();
        let Some(factory) = factory.upgrade() else { return };
        let Some(seat) = seat.upgrade() else { return };
        let Some(errand) = factory.errands.begun(&seat) else { continue };
        let person = factory.lock().config.person_capitalized().to_owned();
        let agent = errand.agent_id.clone();
        let entry = |action: &str| log::Entry { errand: errand.id.clone(), action: action.to_owned(), ..Default::default() };
        match (state, State::from_wire(errand.state)) {
            (BrowserState::TakenOver, State::Running) => {
                let mut record = errand.clone();
                record.state = v1::ErrandState::TakenOver as i32;
                factory.save_errand(record, None);
                log::write(&factory.store, &entry("You took the browser over"));
            }
            (BrowserState::Driven, State::TakenOver) => {
                let mut record = errand.clone();
                record.state = v1::ErrandState::Running as i32;
                factory.save_errand(record, None);
                log::write(&factory.store, &entry("You handed the browser back"));
                let text = format!("{person} handed the browser back. The page may have changed: read it with browser_read before you go on.");
                factory.deliver_event(&agent, text, crate::factory::host_event("The browser is yours again".to_owned()));
            }
            (BrowserState::Stopping | BrowserState::Off | BrowserState::Unavailable, _) => {
                let (state, reason, said) = if state == BrowserState::Stopping {
                    (State::Stopped, "You stopped Claktory's browser.".to_owned(), format!("{person} stopped Claktory's browser"))
                } else {
                    (State::Failed, "Claktory's browser closed.".to_owned(), "Claktory's browser closed".to_owned())
                };
                factory.end_errand(&errand.id, state, &reason).await;
                log::write(&factory.store, &entry(&reason));
                let text = format!("{said}, which closed your browser. Open it again with browser_open if you still need it.");
                factory.deliver_event(&agent, text, crate::factory::host_event("Your browser closed".to_owned()));
            }
            _ => {}
        }
    }
}

/// Whether a handoff of `kind` showed what the person alone may keep: a
/// recovery-codes card's codes, or the setup of two-step sign-in.
pub(crate) fn shows_secrets(kind: i32) -> bool {
    kind == v1::ErrandHandoffKind::RecoveryCodes as i32 || kind == v1::ErrandHandoffKind::TwoStep as i32
}

/// Chrome's HTTP cache emptied, and `origin`'s Cache Storage (Fugo 1642).
/// Recorded for tests when Chrome took both.
async fn clear_caches(factory: &Factory, page: &Page, origin: &str) {
    let cache = page.call("Network.clearBrowserCache", json!({})).await;
    let storage = page.call("Storage.clearDataForOrigin", json!({"origin": origin, "storageTypes": "cache_storage"})).await;
    match (cache, storage) {
        (Ok(_), Ok(_)) => {
            if let Err(error) = factory.store.db().execute("DELETE FROM errand_cache_owed WHERE origin = ?1", [origin]) {
                tracing::error!(%origin, %error, "could not record a cache clear made");
            }
            let mut cleared = crate::locking::lock(&CLEARED);
            if cleared.len() >= 64 {
                cleared.remove(0);
            }
            cleared.push((factory.home.root().to_path_buf(), origin.to_owned()));
        }
        (cache, storage) => {
            tracing::warn!(cache = ?cache.err(), storage = ?storage.err(), "the browser's caches weren't all cleared after a handoff");
        }
    }
}

/// Tests: the origins whose caches were cleared, by Claktory (the last 64).
static CLEARED: Mutex<Vec<(std::path::PathBuf, String)>> = Mutex::new(Vec::new());

/// Tests: the origins `factory` cleared Chrome's caches for, in order.
#[doc(hidden)]
pub fn caches_cleared_for_test(factory: &Factory) -> Vec<String> {
    let root = factory.home.root();
    crate::locking::lock(&CLEARED).iter().filter(|(home, _)| home == root).map(|(_, origin)| origin.clone()).collect()
}

/// `text` between markers made fresh for it, which the text can't guess,
/// so it can't close them and pass itself off as Claktory's words.
pub(crate) fn fenced(what: &str, text: &str) -> String {
    let mut random = [0u8; 9];
    getrandom::fill(&mut random).expect("the system's random source");
    let marker = format!("{what}-{}", base64::Engine::encode(&base64::engine::general_purpose::URL_SAFE_NO_PAD, random));
    format!("<{marker}>\n{text}\n</{marker}>")
}

/// Whether errands are part of this Claktory: its browser exists (P4's
/// host installed a transport, or a test did). Until then the extension is
/// hidden and every errand request is refused.
static AVAILABLE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

pub(crate) fn available() -> bool {
    AVAILABLE.load(std::sync::atomic::Ordering::Relaxed)
}

fn unavailable() -> crate::factory::RequestError {
    crate::factory::RequestError::Command("Claktory's browser isn't available yet.".to_owned())
}

/// ListErrandLog, to the asker only.
pub(crate) fn log_request(factory: &Factory, request: &v1::ListErrandLog) -> Result<v1::ErrandLog, crate::factory::RequestError> {
    if !available() {
        return Err(unavailable());
    }
    Ok(log::list(&factory.store, &request.errand_id, &request.agent_id)?)
}

/// Tests: errands drive a headless Chrome on this computer, in `profile`;
/// false where there's none.
#[doc(hidden)]
pub fn local_chrome_for_test(factory: &Factory, profile: &std::path::Path) -> bool {
    let Some(chrome) = cdp::find_chrome() else { return false };
    match cdp::Local::start(&chrome, profile) {
        Ok(local) => {
            factory.errands.set_source(Arc::new(Shared(Arc::new(local))));
            true
        }
        Err(_) => false,
    }
}

/// Tests: a local Chrome with the host's turns played by the test: it
/// records what errands ask of the browser, and its state is the test's
/// (`browser_state_for_test`).
struct Turns {
    local: cdp::Local,
    states: tokio::sync::watch::Sender<BrowserState>,
    calls: Mutex<Vec<String>>,
    /// Where Chrome downloads, and Claktory's folder, where they land.
    downloads: std::path::PathBuf,
    root: std::path::PathBuf,
    /// What landed, by errand and name: the stored file and its row.
    stored: Mutex<Vec<(String, String, std::path::PathBuf, downloads::StoredRow)>>,
    /// Sign-outs waiting for the errand to end, as the host keeps them.
    signing_out: Mutex<Vec<String>>,
    /// Input leases held now: as the host does, Input.* reaches Chrome
    /// only while one is.
    leases: Arc<std::sync::atomic::AtomicUsize>,
    factory: std::sync::Weak<Factory>,
    /// Why its driven start fails, while the test says so.
    unbooted: Mutex<Option<String>>,
    /// Its profile has no Safe Browsing lists, while the test says so.
    lists_missing: std::sync::atomic::AtomicBool,
}

/// A lease of `Turns`': it ends when dropped.
struct TurnsLease(Arc<std::sync::atomic::AtomicUsize>);

impl Drop for TurnsLease {
    fn drop(&mut self) {
        self.0.fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
    }
}

impl Turns {
    fn call(&self, call: String, state: Option<BrowserState>) {
        crate::locking::lock(&self.calls).push(call);
        if let Some(state) = state {
            self.states.send_replace(state);
        }
    }
}

/// Chrome's pipe with the host's input gate in front (R11.3 #5): Input.*
/// outside a lease is answered with the runner's refusal, never sent.
impl Transport for Turns {
    fn connect(&self) -> Result<cdp::Pipe, String> {
        let chrome = self.local.connect()?;
        let (send, mut outgoing) = tokio::sync::mpsc::channel::<Vec<u8>>(64);
        let (incoming, receive) = tokio::sync::mpsc::channel::<Vec<u8>>(256);
        let leases = self.leases.clone();
        // Weak, so Chrome's end still ends what the errand receives.
        let refusals = incoming.downgrade();
        let to_chrome = chrome.send;
        tokio::spawn(async move {
            while let Some(message) = outgoing.recv().await {
                let leased = leases.load(std::sync::atomic::Ordering::SeqCst) > 0;
                match crate::browser_runner::lease::judge(&message, leased) {
                    crate::browser_runner::lease::Verdict::Forward => {
                        if to_chrome.send(message).await.is_err() {
                            return;
                        }
                    }
                    crate::browser_runner::lease::Verdict::Refuse(reply) => {
                        let Some(refusals) = refusals.upgrade() else { return };
                        let _ = refusals.send(reply).await;
                    }
                    crate::browser_runner::lease::Verdict::Drop(_) => {}
                }
            }
        });
        let mut from_chrome = chrome.receive;
        tokio::spawn(async move {
            while let Some(message) = from_chrome.recv().await {
                if incoming.send(message).await.is_err() {
                    return;
                }
            }
        });
        Ok(cdp::Pipe { send, receive })
    }
}

impl Browser for Turns {
    fn lists_missing(&self) -> bool {
        self.lists_missing.load(std::sync::atomic::Ordering::SeqCst)
    }

    fn begin(&self, errand: &str, url: Option<String>) -> Result<(), String> {
        self.call(format!("begin {errand} {}", url.unwrap_or_default()), Some(BrowserState::Driven));
        Ok(())
    }

    fn handoff(&self, undriven: bool, url: Option<&str>) -> Result<(), String> {
        let how = if undriven { "undriven" } else { "driven" };
        self.call(format!("handoff {how} {}", url.unwrap_or_default()), Some(BrowserState::PersonsTurn));
        Ok(())
    }

    fn resume(&self) -> Result<(), String> {
        self.call("resume".to_owned(), Some(BrowserState::Driven));
        Ok(())
    }

    fn bootstrapped(&self) -> Bootstrapped {
        let unbooted = crate::locking::lock(&self.unbooted).clone();
        Box::pin(async move { unbooted.map_or(Ok(()), Err) })
    }

    fn end(&self, errand: &str) {
        self.call(format!("end {errand}"), Some(BrowserState::Warm));
        let waiting = std::mem::take(&mut *crate::locking::lock(&self.signing_out));
        for provider in waiting {
            self.call(format!("signed out {provider}"), None);
        }
    }

    /// As the host does: at once when no session drives it, else when the
    /// session ends.
    fn sign_out_site(&self, site: &str) -> SigningOut {
        let driven = matches!(*self.states.borrow(), BrowserState::Driven | BrowserState::PersonsTurn | BrowserState::TakenOver);
        if driven {
            self.call(format!("sign out {site} later"), None);
            crate::locking::lock(&self.signing_out).push(site.to_owned());
            return Box::pin(std::future::ready(Ok(SignedOut::Pending)));
        }
        self.call(format!("signed out {site}"), None);
        Box::pin(std::future::ready(Ok(SignedOut::Applied)))
    }

    fn on_release(&self, errand: &str, sessions: Vec<String>) {
        self.call(format!("release {errand} {}", sessions.len()), None);
    }

    fn input_lease(&self, _until: std::time::Duration) -> Result<Option<InputLease>, String> {
        self.call("lease".to_owned(), None);
        self.leases.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Ok(Some(Box::new(TurnsLease(self.leases.clone()))))
    }

    fn states(&self) -> Option<tokio::sync::watch::Receiver<BrowserState>> {
        Some(self.states.subscribe())
    }

    fn download_dir(&self) -> Option<String> {
        Some(self.downloads.display().to_string())
    }

    /// Lands it plainly, as the host would: P4's own rules are its tests'.
    fn land(&self, download: downloads::Download) -> downloads::Landing {
        let landed = (|| {
            let bytes = std::fs::read(self.downloads.join(&download.guid)).map_err(|error| error.to_string())?;
            let name: String = download.suggested.chars().filter(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_')).collect();
            let name = if name.is_empty() || name.starts_with('.') { "download".to_owned() } else { name };
            let folder = self.root.join("errands").join(&download.agent).join("downloads").join(&download.errand);
            std::fs::create_dir_all(&folder).map_err(|error| error.to_string())?;
            let path = folder.join(&name);
            std::fs::write(&path, &bytes).map_err(|error| error.to_string())?;
            let sha256 = crate::encoding::hex(&<sha2::Sha256 as sha2::Digest>::digest(&bytes));
            let host = url::Url::parse(&download.url).ok().and_then(|url| url.host_str().map(str::to_owned)).unwrap_or_default();
            let row = downloads::StoredRow {
                origin: download.page_origin.clone(),
                file_host: host,
                bytes: bytes.len() as u64,
                sha256: sha256.clone(),
                flag: String::new(),
            };
            crate::locking::lock(&self.stored).push((download.errand.clone(), name.clone(), path.clone(), row));
            Ok(downloads::Landed { name, path, bytes: bytes.len() as u64, sha256, flag: String::new() })
        })();
        self.call(format!("land {}", download.suggested), None);
        Box::pin(async move { landed })
    }

    fn stored(&self, errand: &str, name: &str) -> Option<(std::path::PathBuf, downloads::StoredRow)> {
        let stored = crate::locking::lock(&self.stored);
        stored.iter().find(|(of, called, _, _)| of == errand && called == name).map(|(_, _, path, row)| (path.clone(), row.clone()))
    }

    fn upload(&self, file: &std::path::Path, _roots: &[std::path::PathBuf]) -> Uploading {
        self.call(format!("upload {}", file.display()), None);
        Box::pin(std::future::ready(Ok(file.display().to_string())))
    }

    fn copy_into(&self, file: &std::path::Path, folder: &std::path::Path, name: &str) -> Result<std::path::PathBuf, String> {
        std::fs::create_dir_all(folder).map_err(|error| error.to_string())?;
        let copy = folder.join(name);
        std::fs::copy(file, &copy).map_err(|error| error.to_string())?;
        Ok(copy)
    }
}

/// The test browsers, by the Claktory they're in.
static TURNS: Mutex<Vec<(usize, Arc<Turns>)>> = Mutex::new(Vec::new());

fn turns_of(factory: &Arc<Factory>) -> Option<Arc<Turns>> {
    let key = Arc::as_ptr(factory) as usize;
    crate::locking::lock(&TURNS).iter().find(|(of, _)| *of == key).map(|(_, turns)| turns.clone())
}

/// Tests: errands drive a headless Chrome here, in `profile`, through a
/// browser whose turns the test plays; false where there's no Chrome.
#[doc(hidden)]
pub fn local_chrome_with_turns_for_test(factory: &Arc<Factory>, profile: &std::path::Path) -> bool {
    let Some(chrome) = cdp::find_chrome() else { return false };
    let Ok(local) = cdp::Local::start(&chrome, profile) else { return false };
    let downloads = profile.with_extension("downloads");
    if std::fs::create_dir_all(&downloads).is_err() {
        return false;
    }
    let turns = Arc::new(Turns {
        local,
        states: tokio::sync::watch::channel(BrowserState::Off).0,
        calls: Mutex::default(),
        downloads,
        root: factory.home.root().to_path_buf(),
        stored: Mutex::default(),
        signing_out: Mutex::default(),
        leases: Arc::default(),
        factory: Arc::downgrade(factory),
        unbooted: Mutex::default(),
        lists_missing: std::sync::atomic::AtomicBool::new(false),
    });
    crate::locking::lock(&TURNS).push((Arc::as_ptr(factory) as usize, turns.clone()));
    install(factory, Arc::new(Shared(turns)));
    true
}

/// Tests: the person approves that card (its action, as the asks table
/// keeps it): what Claktory says, or why it couldn't.
#[doc(hidden)]
pub async fn approve_card_for_test(factory: &Arc<Factory>, ask: &str) -> Result<String, String> {
    let action: String = factory.store.db().query_row("SELECT action FROM asks WHERE id = ?1", [ask], |row| row.get(0)).expect("the card");
    crate::card_actions::check(factory, &action)?;
    crate::card_actions::apply(factory, &action).await
}

/// Tests: what errands asked of the browser so far (`Turns`), emptied.
#[doc(hidden)]
pub fn browser_calls_for_test(factory: &Arc<Factory>) -> Vec<String> {
    turns_of(factory).map(|turns| std::mem::take(&mut *crate::locking::lock(&turns.calls))).unwrap_or_default()
}

/// Tests: the browser's state is `state` (an AgentBrowserState), as the
/// person's Take over, Hand back or Stop would make it.
#[doc(hidden)]
pub fn browser_state_for_test(factory: &Arc<Factory>, state: i32) {
    if let Some(turns) = turns_of(factory) {
        turns.states.send_replace(BrowserState::try_from(state).unwrap_or(BrowserState::Off));
    }
}

/// Tests: the browser's profile has no Safe Browsing lists (`missing`),
/// or has them.
#[doc(hidden)]
pub fn lists_missing_for_test(factory: &Arc<Factory>, missing: bool) {
    if let Some(turns) = turns_of(factory) {
        turns.lists_missing.store(missing, std::sync::atomic::Ordering::SeqCst);
    }
}

/// Tests: the browser's driven start fails, saying `why`, or succeeds
/// again (None).
#[doc(hidden)]
pub fn browser_unbooted_for_test(factory: &Arc<Factory>, why: Option<&str>) {
    if let Some(turns) = turns_of(factory) {
        *crate::locking::lock(&turns.unbooted) = why.map(str::to_owned);
    }
}

/// Tests: `agent` opens its browser, as browser_open does before it goes
/// to a page (tests have no network): its session, or why not.
#[doc(hidden)]
pub fn open_browser_for_test(factory: &Arc<Factory>, agent: &str) -> Result<String, String> {
    factory.open_browser(agent).map(|session| session.id)
}

/// The page a session drives now, in any agent's browser.
pub(super) async fn any_live_page(factory: &Factory) -> Option<Arc<Page>> {
    let seats: Vec<Arc<Seat>> = crate::locking::lock(&factory.errands.seats).values().cloned().collect();
    for seat in seats {
        if let Some(live) = seat.live.lock().await.as_ref() {
            return Some(live.page.clone());
        }
    }
    None
}

/// Tests: the open session's page shows `html` (no network in tests).
#[doc(hidden)]
pub async fn show_for_test(factory: &Factory, html: &str) {
    let page = any_live_page(factory).await.expect("a session's page");
    let frame = page.frame().await.expect("its frame");
    page.call("Page.setDocumentContent", json!({"frameId": frame.id, "html": html})).await.expect("its content");
}

/// Tests: the sweep of recovery codes whose card no longer waits.
#[doc(hidden)]
pub fn sweep_codes_for_test(factory: &Factory) {
    code::sweep_codes(factory);
}

/// Tests: `errand` ends Failed, as a restart ends an open errand.
#[doc(hidden)]
pub async fn end_errand_for_test(factory: &Factory, errand: &str) {
    factory.end_errand(errand, State::Failed, RESTARTED).await;
}

/// Tests: recovery-codes card `card`'s codes are lost, as a restart while
/// Secrets was locked loses them.
#[doc(hidden)]
pub fn lose_codes_for_test(factory: &Factory, card: &str) {
    code::drop_codes(factory, card.parse().unwrap_or_default());
}

/// Tests: the origins owed a cache clear (`CACHE_OWED`).
#[doc(hidden)]
pub fn caches_owed_for_test(factory: &Factory) -> Vec<String> {
    caches_owed(factory)
}

/// Tests: the browser connection goes without anything said to the page,
/// as when Claktory or the browser restarts (the test then gives it
/// another browser).
#[doc(hidden)]
pub async fn drop_connection_for_test(factory: &Factory) {
    let seats: Vec<Arc<Seat>> = crate::locking::lock(&factory.errands.seats).values().cloned().collect();
    for seat in seats {
        *seat.live.lock().await = None;
    }
}

/// Tests: `agent` read `what` from outside, as web_search marks it (a
/// search needs the internet).
#[doc(hidden)]
pub fn read_outside_for_test(factory: &Factory, agent: &str, what: &str) {
    read_outside(factory, agent, what);
}

/// Tests: a message on a pipe reaches its lead `agent`, as a pipe's
/// messages do (an event from the pipe `from`), or with `app` an app's
/// event (from the app `from`).
#[doc(hidden)]
pub fn pipe_message_for_test(factory: &Arc<Factory>, agent: &str, from: &str, text: &str, app: bool) {
    let event = factory_agent::Event {
        source: if app { factory_agent::EventSource::App } else { factory_agent::EventSource::Pipe },
        from: from.to_owned(),
        id: "pipe:test".to_owned(),
        summary: "A message on the pipe".to_owned(),
        transient: false,
        pipe_seqs: Vec::new(),
        provenance: None,
    };
    factory.deliver_event(agent, text.to_owned(), event);
}

/// Tests: `factory`'s code and agreement cards may be asked on pages
/// `show_for_test` showed, which have no https origin (shown as
/// https://shown.test).
#[doc(hidden)]
pub fn code_cards_anywhere_for_test(factory: &Arc<Factory>) {
    crate::locking::lock(&code::ANY_ORIGIN_FOR_TEST).push(Arc::downgrade(factory));
}

/// The cards an ended errand's handoffs still wait on, withdrawn with why
/// (Fugo 50825, Abbacchio 1719): once the errand is over, approving one
/// does nothing. A recovery-codes card stays: its codes wait for the
/// person's I saved them, whatever became of the errand.
fn withdraw_handoffs(factory: &Factory, errand: &v1::Errand) {
    let pending: Vec<(i64, String)> = crate::store::query(
        &factory.store.db(),
        "SELECT id, action FROM asks WHERE agent = ?1 AND state = 'pending' AND action != ''",
        [&errand.agent_id],
        crate::store::row2,
    )
    .unwrap_or_default();
    let reason = if errand.end_reason.is_empty() { "The browser closed.".to_owned() } else { errand.end_reason.clone() };
    for (id, action) in pending {
        let Ok(crate::card_actions::Action::Handoff { handoff }) = serde_json::from_str(&action) else { continue };
        if handoff.errand == errand.id && handoff.kind != v1::ErrandHandoffKind::RecoveryCodes as i32 {
            crate::extensions::ask::withdraw_for(factory, id, reason.trim_end());
        }
    }
}

/// Until browser-simple, "browser" was Track B's browser control (now
/// "browser_control"), off for everyone outside a preview
/// build and turning itself off after use. An entry of its left on in
/// config.json would make its agent a browsing agent without anyone
/// choosing it, so once, at the first start of a backend with
/// browser-simple, every "browser" entry is turned off: none can be the new
/// extension's yet, by Claktory as History says. Recorded in
/// `browser_control_retired`; tried again at the next start if the config
/// couldn't be changed.
pub(crate) async fn retire_browser_control(factory: &Arc<Factory>) {
    let done = factory.store.db().query_row("SELECT 1 FROM browser_control_retired", [], |_| Ok(())).is_ok();
    if done {
        return;
    }
    let on: Vec<String> = factory
        .lock()
        .config
        .agents
        .iter()
        .filter(|agent| crate::config::browser_on(agent))
        .map(|agent| agent.id.clone())
        .collect();
    if !on.is_empty() {
        let edited = factory
            .edit_config(|config| {
                for id in &on {
                    if let Some(agent) = config.agent_mut(id) {
                        crate::config::browser_off(agent);
                    }
                }
                Ok(())
            })
            .await;
        if let Err(error) = edited {
            tracing::error!(%error, "couldn't turn off browser control left on from before: trying again at the next start");
            return;
        }
        tracing::warn!(agents = ?on, "turned off browser control left on from a preview build: \"browser\" is Claktory's browser now");
        for agent in &on {
            crate::browser_switches::record(&factory.store, crate::browser_switches::By::Claktory, agent, false);
        }
    }
    if let Err(error) = factory.store.db().execute("INSERT INTO browser_control_retired (at) VALUES (?1)", [now_ms()]) {
        tracing::error!(%error, "couldn't record that browser control's old entries were turned off");
    }
}

/// At start: reads the sessions kept, and tells each agent whose browser
/// Claktory's restart closed. Recovery codes whose card no longer waits go
/// (`code::sweep_codes`).
pub(crate) fn recover(factory: &Arc<Factory>) {
    code::sweep_codes(factory);
    let (send, mut turned_off) = tokio::sync::mpsc::unbounded_channel::<String>();
    *crate::locking::lock(&factory.errands.turned_off) = Some(send);
    let weak = Arc::downgrade(factory);
    tokio::spawn(async move {
        while let Some(agent) = turned_off.recv().await {
            let Some(factory) = weak.upgrade() else { return };
            if let Some(session) = factory.errands.open_of(&agent) {
                factory.end_errand(&session.id, State::Stopped, TURNED_OFF).await;
            }
            factory.errands.unseat(&agent);
        }
    });
    for errand in factory.errands.load(&factory.store) {
        withdraw_handoffs(factory, &errand);
        let told = format!("{RESTARTED} Open it again with browser_open if you still need it: you may have to sign in again.");
        factory.deliver_event(&errand.agent_id, told, crate::factory::host_event("Your browser closed".to_owned()));
    }
}

/// The agents contained for errands: none since browser-simple, where any
/// agent with the Browser extension browses on its own. Their rows stay in
/// `errand_agents`, unread, for one release, so a rollback finds them.
pub(crate) fn load_agents(_store: &crate::store::Store) -> rusqlite::Result<std::collections::BTreeSet<String>> {
    Ok(std::collections::BTreeSet::new())
}

/// A step that may bring a file (R9): the fence lets the page's redirects
/// hop to a file, and with browser_download a PDF comes as
/// one, until it's dropped, however the step ends; then the tracker expects
/// no more.
struct FileAsk {
    hops: Arc<std::sync::atomic::AtomicBool>,
    fetching: Arc<std::sync::atomic::AtomicBool>,
    tracker: Arc<downloads::Tracker>,
}

impl FileAsk {
    fn new(
        hops: Arc<std::sync::atomic::AtomicBool>,
        fetching: Arc<std::sync::atomic::AtomicBool>,
        tracker: Arc<downloads::Tracker>,
        download: bool,
    ) -> FileAsk {
        tracker.expect(Instant::now() + downloads::FINISHES);
        hops.store(true, std::sync::atomic::Ordering::SeqCst);
        fetching.store(download, std::sync::atomic::Ordering::SeqCst);
        FileAsk { hops, fetching, tracker }
    }
}

impl Drop for FileAsk {
    fn drop(&mut self) {
        self.tracker.expect_none();
        self.hops.store(false, std::sync::atomic::Ordering::SeqCst);
        self.fetching.store(false, std::sync::atomic::Ordering::SeqCst);
    }
}

/// One errand tool's step on the page.
pub(crate) enum Step {
    Read { limit: usize },
    Scroll { direction: schema::Direction, pages: u8 },
    Act(Op),
    /// browser_act's dismiss: clear what covers the page read as `document`.
    Dismiss { document: String },
    Navigate { address: String },
    /// browser_download: the file at `address`.
    Download { address: String },
}

impl Step {
    /// What the log says it did, never its payload; and its target, once
    /// known.
    fn logged(&self) -> (String, Option<String>) {
        match self {
            Step::Read { .. } => ("Read the page".to_owned(), None),
            Step::Scroll { direction, .. } => {
                (format!("Scrolled {}", if *direction == schema::Direction::Up { "up" } else { "down" }), None)
            }
            Step::Act(Op::Click { .. }) => ("Clicked".to_owned(), None),
            Step::Act(Op::Fill { text, .. }) => {
                let count = text.chars().count();
                (format!("Typed {count} character{}", if count == 1 { "" } else { "s" }), None)
            }
            Step::Act(_) => ("Acted".to_owned(), None),
            Step::Dismiss { .. } => ("Cleared what covered the page".to_owned(), None),
            Step::Navigate { address } => {
                let to = url::Url::parse(address).map(|url| format!("{}{}", url.origin().ascii_serialization(), fence::path_of(&url)));
                ("Went to a page".to_owned(), to.ok())
            }
            Step::Download { address } => {
                let from = url::Url::parse(address).map(|url| format!("{}{}", url.origin().ascii_serialization(), fence::path_of(&url)));
                ("Asked for a file".to_owned(), from.ok())
            }
        }
    }
}

/// A failure as the agent reads it: its word first.
pub(crate) fn said((error, why): (String, String)) -> String {
    if why.is_empty() { format!("{error}.") } else { format!("{error}: {why}") }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn states_read_back_as_written() {
        for state in State::ALL {
            assert_eq!(State::of(state.word()), state);
            assert_eq!(State::from_wire(state.wire() as i32), state);
        }
        assert!(State::PersonsTurn.open() && !State::Done.open());
    }

    /// A step dropped midway (an errand stopped, the connection gone)
    /// leaves no file asked for: the next step's redirects stay on pages
    /// and its PDFs show.
    #[tokio::test]
    async fn a_step_dropped_midway_leaves_no_file_asked_for() {
        let (hops, fetching) = (Arc::new(std::sync::atomic::AtomicBool::new(false)), Arc::new(std::sync::atomic::AtomicBool::new(false)));
        let tracker = Arc::new(downloads::Tracker::default());
        let read = |flag: &Arc<std::sync::atomic::AtomicBool>| flag.load(std::sync::atomic::Ordering::SeqCst);
        let step = {
            let (hops, fetching, tracker) = (hops.clone(), fetching.clone(), tracker.clone());
            async move {
                let _asking = FileAsk::new(hops, fetching, tracker, true);
                std::future::pending::<()>().await;
            }
        };
        let mut step = Box::pin(step);
        assert!(tokio::time::timeout(std::time::Duration::from_millis(50), &mut step).await.is_err(), "still going");
        assert!(read(&hops) && read(&fetching) && tracker.expecting(), "asked while it runs");
        drop(step);
        assert!(!read(&hops) && !read(&fetching) && !tracker.expecting(), "cleared once dropped");
    }
}
