//! Errands (#435 P3, agent-browser.md Rev 2, SPLIT.md): an agent's trip
//! through Claktory's browser for the person, within the sites it was
//! opened for. Who opens one (`open`): the person (OpenErrand), or the
//! errand agent's lead with errand_open, never while that lead has read
//! from outside (Fugo 36687). Who does it: a contained agent with the
//! errands extension, which reads and acts on the page through CDP
//! (`cdp`, `reader`), held to the errand's sites (`fence`), handing the
//! person what only they may do (`handoff`). Every step is logged without
//! its payload (`log`).
//!
//! The browser itself is P4's (`browser_host::Handle`): errands reach it
//! through the [`cdp::Transport`] it implements, installed with
//! [`Errands::set_transport`]. Until one is, every errand tool says
//! Claktory's browser isn't available.

// The host (P4) and the protocol's arms come next: parts stay unused until then.
#![allow(dead_code)]

pub(crate) mod cdp;
pub(crate) mod checks;
pub(crate) mod code;
pub(crate) mod downloads;
pub(crate) mod fence;
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
use crate::browser::origin;
use crate::browser::schema::{self, Answer, Op};
use crate::factory::{Factory, host_event};
use crate::transcript::now_ms;

/// Errands (`records`), their log (`log`) and the agents the person
/// contained for them (`contained`).
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
/// started afresh (Fugo 50226): no errand_open for them, as for an agent
/// that read a page in an errand. Not pipes' taint, which would make every
/// such agent's secrets ask (card 283). Cleared at /clear.
pub(crate) const OUTSIDE_READ: &str = "
CREATE TABLE errand_outside_read (agent TEXT PRIMARY KEY, what TEXT NOT NULL, since INTEGER NOT NULL);
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

/// Why an errand open when Claktory stopped ended.
const RESTARTED: &str = "Claktory restarted, which closed the browser.";

pub use checks::check_timing_for_test;

/// The most sites an errand is for.
pub(crate) const MAX_SITES: usize = 10;
/// The most characters of a purpose.
pub(crate) const PURPOSE_MAX: usize = 200;
/// The most bytes of a brief.
pub(crate) const BRIEF_MAX: usize = 16 * 1024;
/// The most errands the Workspace lists: the open one and the latest ended.
const LISTED: usize = 20;

/// Errands, and the browser connection the open one drives.
#[derive(Default)]
pub(crate) struct Errands {
    /// Every errand, newest first (as kept in factory.db's `errands`).
    records: Mutex<Vec<v1::Errand>>,
    /// Where Claktory's browser is: P4's host, or a local Chrome in tests.
    transport: Mutex<Option<Arc<dyn Browser>>>,
    /// The errand the browser was begun for, until it ends.
    begun: Mutex<Option<String>>,
    /// The driven page, while an errand drives it. One errand at a time,
    /// one tool call at a time.
    live: tokio::sync::Mutex<Option<Live>>,
    /// The sites the browser's profile is signed in to, as the host last
    /// read them (origins only).
    signed_in: Mutex<Vec<String>>,
    /// The address the errand last drove (errand, address): after a
    /// relaunch, its tab is the one there (Ghiaccio 38596).
    last: Mutex<Option<(String, String)>>,
    /// The browser's one lane (#530, R11.4): an errand's steps go one at a
    /// time, at its person's pace. Taken before `live`, and held while a
    /// step paces, which `live` isn't: the person's Stop and turns never
    /// wait for a step.
    lane: tokio::sync::Mutex<Option<Pacing>>,
    /// The errand waiting to try a page again for its check that the
    /// visitor is a person (W5), and how long it waits.
    checking: Mutex<Option<checks::Waiting>>,
    check_timing: Mutex<checks::Timing>,
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
    /// Whether the fence lets the top frame's redirects leave the sites,
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

/// `site` as an errand keeps it: an https origin (a bare host means
/// https), never a public suffix alone ("https://co.uk"), or why not.
pub(crate) fn site(site: &str) -> Result<String, String> {
    let site = site.trim();
    let written = if site.contains("://") { site.to_owned() } else { format!("https://{site}") };
    if written.contains('*') {
        return Err(format!("\"{site}\": a site is one origin, never a pattern with *."));
    }
    let origin = origin::canonical(&written)?;
    let host = url::Url::parse(&origin).ok().and_then(|url| url.host_str().map(str::to_owned)).unwrap_or_default();
    if psl::suffix_str(&host).is_none_or(|suffix| suffix == host) {
        return Err(format!("{host} is a whole domain ending, not a site: name the site itself."));
    }
    Ok(origin)
}

/// One line of at most `max` characters, refused (not cut) when it holds
/// control or invisible characters.
fn plain(what: &str, text: &str, max: usize) -> Result<String, String> {
    let text = text.trim();
    if text.is_empty() {
        return Err(format!("{what} is empty."));
    }
    if text.chars().count() > max {
        return Err(format!("{what} is at most {max} characters."));
    }
    if text.chars().any(|c| c.is_control() || factory_agent::text::hidden(c)) {
        return Err(format!("{what} is one line of plain characters."));
    }
    Ok(text.to_owned())
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

    /// Where the browser is: P4's host installs itself (`install`); tests
    /// a local Chrome. From then on errands are part of this Claktory.
    pub(crate) fn set_transport(&self, transport: Option<Arc<dyn Browser>>) {
        if transport.is_some() {
            AVAILABLE.store(true, std::sync::atomic::Ordering::Relaxed);
        }
        *crate::locking::lock(&self.transport) = transport;
    }

    pub(crate) fn browser(&self) -> Option<Arc<dyn Browser>> {
        crate::locking::lock(&self.transport).clone()
    }

    /// The open errand the browser was begun for.
    fn begun(&self) -> Option<v1::Errand> {
        let begun = crate::locking::lock(&self.begun).clone()?;
        self.get(&begun).filter(|errand| State::from_wire(errand.state).open())
    }

    /// The sites the browser's profile is signed in to (the host reads
    /// them; origins only).
    pub(crate) fn set_signed_in(&self, sites: Vec<String>) {
        *crate::locking::lock(&self.signed_in) = sites;
    }

    pub(crate) fn signed_in(&self) -> Vec<String> {
        crate::locking::lock(&self.signed_in).clone()
    }

    /// The address `errand` last drove, if it's the one it was.
    fn last(&self, errand: &str) -> Option<String> {
        crate::locking::lock(&self.last).as_ref().filter(|(of, _)| of == errand).map(|(_, address)| address.clone())
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

    /// The open errand, if any: one at a time, as there's one browser.
    pub(crate) fn open_one(&self) -> Option<v1::Errand> {
        crate::locking::lock(&self.records).iter().find(|errand| State::from_wire(errand.state).open()).cloned()
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

    /// Opens an errand for `agent`, by `opener` (an agent's id, or None for
    /// the person): what the opener is told, or why not.
    pub(crate) fn open_errand(
        self: &Arc<Self>,
        opener: Option<&str>,
        agent: &str,
        purpose: &str,
        sites: &[String],
        brief: &str,
    ) -> Result<String, String> {
        let (name, person, extension_on) = {
            let inner = self.lock();
            let config = &inner.config;
            let Some(doer) = config.agent(agent) else {
                return Err(format!("There's no agent \"{agent}\"."));
            };
            if let Some(opener) = opener {
                let leads = config.lead(doer).is_some_and(|lead| lead.id == opener);
                if !leads {
                    return Err(format!("Only {}'s lead, or the person, can open an errand for it.", doer.name));
                }
            }
            (doer.name.clone(), config.person().to_owned(), crate::extensions::enabled(doer, ERRANDS))
        };
        if let Some(opener) = opener {
            if self.pipes.contained(opener) {
                return Err("A contained agent can't open errands.".to_owned());
            }
            let read = if self.pipes.tainted(opener).is_empty() {
                outside_read(self, opener)
            } else {
                Some(crate::pipes::agents::what_was_read(self, opener))
            };
            if let Some(read) = read {
                return Err(format!(
                    "You've read {read}, which may hold someone else's instructions, so you can't open an errand until your conversation starts afresh: the browser is signed in to {person}'s sites."
                ));
            }
        }
        if !extension_on || !contained(self, agent) {
            return Err(format!(
                "{name} can't do errands: {person} turns on Use for errands… in its settings, then its Errands extension."
            ));
        }
        let purpose = plain("purpose", purpose, PURPOSE_MAX)?;
        if sites.is_empty() || sites.len() > MAX_SITES {
            return Err(format!("sites: one to {MAX_SITES} sites the errand may go to, such as https://play.google.com."));
        }
        let mut kept: Vec<String> = Vec::new();
        for written in sites {
            let site = site(written)?;
            if !kept.contains(&site) {
                kept.push(site);
            }
        }
        let brief = brief.trim();
        if brief.len() > BRIEF_MAX {
            return Err(format!("brief is at most {} KB.", BRIEF_MAX / 1024));
        }
        if brief.chars().any(|c| (c.is_control() && c != '\n' && c != '\t') || factory_agent::text::hidden(c)) {
            return Err("brief holds control or invisible characters: write it with plain characters.".to_owned());
        }
        if let Some(open) = self.errands.open_one() {
            return Err(format!(
                "Errand {} ({}) is still open, and Claktory's browser does one errand at a time. It ends when its agent finishes it or {person} stops it.",
                open.id, open.purpose
            ));
        }
        let id = format!("errand-{}", now_ms());
        // Claktory's browser, driven for it, on its first site.
        if let Some(browser) = self.errands.browser() {
            browser.begin(&id, kept.first().cloned())?;
            *crate::locking::lock(&self.errands.begun) = Some(id.clone());
        }
        let errand = v1::Errand {
            id: id.clone(),
            agent_id: agent.to_owned(),
            purpose: purpose.clone(),
            sites: kept.clone(),
            state: v1::ErrandState::Running as i32,
            opened_by: opener.unwrap_or_default().to_owned(),
            handoff: None,
            created_ms: now_ms(),
            ended_ms: 0,
            end_reason: String::new(),
        };
        self.save_errand(errand, Some(brief));
        log::write(&self.store, &log::Entry { errand: id.clone(), agent: String::new(), action: format!("Opened: {purpose}"), ..Default::default() });
        let from = match opener {
            Some(opener) => format!("your lead, {}", self.lock().config.agent(opener).map_or(opener.to_owned(), |lead| lead.name.clone())),
            None => person.clone(),
        };
        let brief_said = if brief.is_empty() { String::new() } else { format!("\n\nThe brief:\n{brief}") };
        let told = format!(
            "{from} opened errand {id} for you: {purpose}. Its sites: {}. Read the page with errand_read, act on it with errand_act, go to another page of its sites with errand_navigate, hand {person} what only they may do with errand_handoff, and say it's done with errand_done.{brief_said}",
            kept.join(", ")
        );
        self.deliver_event(agent, told, host_event(format!("Errand {id} opened")));
        let signed_in = self.errands.signed_in();
        let theirs: Vec<&String> = kept.iter().filter(|site| signed_in_to(&signed_in, site)).collect();
        let signed = if theirs.is_empty() {
            "The browser isn't signed in to any of them as far as Claktory knows.".to_owned()
        } else {
            format!(
                "The browser is signed in to {} as {person}: {name} acts there as them.",
                theirs.iter().map(|site| site.as_str()).collect::<Vec<_>>().join(", ")
            )
        };
        Ok(format!("Opened errand {id} for {name}, on {}. {signed}", kept.join(", ")))
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
        self.save_errand(record, None);
        let mut live = self.errands.live.lock().await;
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
            let mut begun = crate::locking::lock(&self.errands.begun);
            if begun.as_deref() == Some(errand) { begun.take() } else { None }
        };
        if let (Some(begun), Some(browser)) = (begun, self.errands.browser()) {
            browser.end(&begun);
        }
    }

    /// A recovery-codes or two-step handoff of `errand` at `origin` ended:
    /// what its page showed (the codes, a setup key) leaves Chrome's caches
    /// on disk, whatever the outcome, so a site that sent it cacheable
    /// doesn't leave it in the errands' profile (Fugo 1642).
    pub(crate) async fn forget_cached(&self, errand: &str, origin: &str) {
        let Some(errand) = self.errands.get(errand).filter(|errand| State::from_wire(errand.state).open()) else { return };
        let mut live = self.errands.live.lock().await;
        let page = self.page(&mut live, &errand).await;
        drop(live);
        match page {
            Ok(page) => clear_caches(self, &page, origin).await,
            Err((_, why)) => tracing::warn!(errand = %errand.id, %why, "couldn't clear the browser's caches after a handoff"),
        }
    }

    /// The page `errand` drives, connected when it isn't.
    async fn page(&self, live: &mut Option<Live>, errand: &v1::Errand) -> Result<Arc<Page>, (String, String)> {
        if let Some(live) = live.as_ref().filter(|live| live.errand == errand.id && !live.page.cdp().closed()) {
            live.page.set_two_step(code::two_step_origins(self, &errand.id));
            return Ok(live.page.clone());
        }
        *live = None;
        let Some(transport) = self.errands.browser() else {
            return Err(("interrupted".to_owned(), "Claktory's browser isn't available yet".to_owned()));
        };
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
        let last = self.errands.last(&errand.id);
        let page = Arc::new(Page::shown(cdp.clone(), &errand.sites, last.as_deref()).await.map_err(gone)?);
        let session = page.session().to_owned();
        // Detached when the agent stops driving, so the fence's pauses end
        // with its turn (Giorno 37215).
        transport.on_release(&errand.id, vec![session]);
        let blocked = Arc::new(Mutex::new(None));
        let hops = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let fetching = Arc::new(std::sync::atomic::AtomicBool::new(false));
        fence::guard(page.clone(), errand.sites.clone(), blocked.clone(), hops.clone(), fetching.clone()).await.map_err(gone)?;
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
            let _ = fence::go(&page, &errand.sites, last).await;
        }
        *live = Some(Live { errand: errand.id.clone(), page: page.clone(), blocked, downloads: tracker, hops, fetching });
        Ok(page)
    }

    /// `agent`'s errand while it may drive: running.
    pub(crate) fn driving(&self, agent: &str) -> Result<v1::Errand, (String, String)> {
        let person = self.lock().config.person().to_owned();
        let Some(errand) = self.errands.open_of(agent) else {
            return Err(("wrong_principal".to_owned(), "you have no open errand".to_owned()));
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
        // One step at a time, the lane before the connection.
        let mut lane = self.errands.lane.lock().await;
        if lane.as_ref().is_some_and(|pacing| pacing.errand != errand.id) {
            *lane = None;
        }
        let pacing = lane.get_or_insert_with(|| Pacing::new(&errand.id));
        let mut live = self.errands.live.lock().await;
        let page = self.page(&mut live, &errand).await.map_err(said)?;
        let Some((blocked, tracker, hops, fetching)) = live.as_ref().map(|live| {
            (crate::locking::lock(&live.blocked).take(), live.downloads.clone(), live.hops.clone(), live.fetching.clone())
        }) else {
            return Err(said(("interrupted".to_owned(), "the browser's connection went".to_owned())));
        };
        // Paced outside the connection's lock: the person's Stop or turn
        // ends the connection mid-step, and the step stops at its next
        // event.
        drop(live);
        if pacing.page != Arc::as_ptr(&page) as usize {
            pacing.page = Arc::as_ptr(&page) as usize;
            pacing.pointer = None;
        }
        let browser = self.errands.browser();
        let still = {
            let (factory, id, page, person) = (self.clone(), errand.id.clone(), page.clone(), self.lock().config.person().to_owned());
            let browser = browser.clone();
            move || -> Result<(), (String, String)> {
                match factory.errands.get(&id).map(|errand| State::from_wire(errand.state)) {
                    Some(State::Running) => {}
                    Some(State::PersonsTurn) => return Err(("paused".to_owned(), format!("the browser is {person}'s until they answer your handoff request"))),
                    Some(State::TakenOver) => return Err(("paused".to_owned(), format!("{person} took the browser over: wait until they hand it back"))),
                    _ => return Err(("interrupted".to_owned(), "the errand is over".to_owned())),
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
        // Read, scroll and act only on the errand's sites, or a blank page:
        // never a tab somewhere else the fence didn't see it go (Fugo 38499).
        let elsewhere = match &step {
            Step::Read { .. } | Step::Scroll { .. } | Step::Act(_) | Step::Dismiss { .. } => match page.elsewhere(&errand.sites).await {
                Ok(None) => None,
                Ok(Some(origin)) => Some(("off_site".to_owned(), format!("this tab is on {origin}, not one of the errand's sites: go back to one with errand_navigate"))),
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
            (Step::Act(op), None) => match self.while_signing_out(&page).await {
                Some(refused) => Err(refused),
                None => match lease(until) {
                    Ok(_held) => page.act(&mut hand, op).await.map(Some),
                    Err(refused) => Err(refused),
                },
            },
            (Step::Dismiss { document }, None) => match self.while_signing_out(&page).await {
                Some(refused) => Err(refused),
                None => match lease(until) {
                    Ok(_held) => page.dismiss(&mut hand, document).await.map(Some),
                    Err(refused) => Err(refused),
                },
            },
            (Step::Navigate { address }, None) => fence::go(&page, &errand.sites, address).await.map(Some),
            (Step::Download { address }, None) => fence::fetch(&page, &errand.sites, address).await.map(|()| None),
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
        let frame = page.frame().await.ok();
        if let Some(frame) = &frame {
            *crate::locking::lock(&self.errands.last) = Some((errand.id.clone(), frame.url.clone()));
        }
        let url = frame.as_ref().and_then(|frame| url::Url::parse(&frame.url).ok());
        let origin = url.as_ref().map(|url| url.origin().ascii_serialization()).filter(|origin| origin != "null").unwrap_or_default();
        let path = url.as_ref().map(fence::path_of).unwrap_or_default();
        // Asked for a file and got a page: a check that it's a person, or
        // not a file at all.
        let outcome = match outcome {
            Ok(None) if !began => Err(("not_a_file".to_owned(), "that address opened a page, not a file: read it with errand_read".to_owned())),
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
        // anything is filled (Bruno 37122).
        let moved = !began
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
            crate::pipes::agents::taint(self, agent, &crate::pipes::agents::Kind::Browser(&origin).key());
        }
        let shown = if origin.is_empty() { "about:blank".to_owned() } else { origin };
        let page_said = answer.map(|answer| schema::fenced(&shown, &answer)).unwrap_or_default();
        Ok((format!("{notice}{page_said}{files}").trim_start_matches('\n').to_owned(), json!({})))
    }

    /// Lands the downloads that finished (`finished`) into the errand
    /// agent's folder through the browser's host: a line on each for the
    /// agent (the host's words; the page's only as the name Claktory gave
    /// it), and one for those `still` going. The log keeps name, size,
    /// origin and hash, never the content.
    async fn land_downloads(&self, errand: &v1::Errand, agent: &str, origin: &str, finished: Vec<downloads::Begun>, still: usize) -> Vec<String> {
        let Some(browser) = self.errands.browser() else { return Vec::new() };
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
            lines.push(format!("{them} still going: you're told how it went after your next errand step."));
        }
        lines
    }
}

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

    /// `errand`'s landed download `name`: its stored file, and its row.
    fn stored(&self, _errand: &str, _name: &str) -> Option<(std::path::PathBuf, downloads::StoredRow)> {
        None
    }

    /// Copies a stored file into `folder` as `name`, as landing does: the
    /// copy's path, or why not.
    fn copy_into(&self, _file: &std::path::Path, _folder: &std::path::Path, _name: &str) -> Result<std::path::PathBuf, String> {
        Err("Claktory's browser can't download files here.".to_owned())
    }

    /// Signs the browser out of identity provider `provider` (#435, Fugo
    /// 41236, Giorno 41238): its cookies on the provider's domains
    /// (`fill::owner`) deleted now, or, while an errand drives it, kept
    /// (in the browser's home) and deleted when that errand ends or before
    /// Chrome next runs a page, whichever comes first; Stop doesn't undo it,
    /// Delete browser data makes it moot. Once deleted, the host calls
    /// `Factory::signed_out`, which has the vault forget the account. Err
    /// says why it couldn't. Async: deleting is CDP's round trips.
    fn sign_out(&self, _provider: &str) -> SigningOut {
        Box::pin(std::future::ready(Err("Claktory's browser can't sign out here yet.".to_owned())))
    }

    /// Whether a sign-out of `provider` waits to be applied: nothing is
    /// filled or pressed on its sites meanwhile (`fill::while_signing_out`).
    fn signing_out(&self, _provider: &str) -> bool {
        false
    }

    /// The person's sign-in turn at identity provider `provider` (#435,
    /// Bruno 41834, Giorno 41843): a scoped grant opens the live view and
    /// the person's input only while the top page is on the provider's
    /// sign-in origins, and the agent's input lease is revoked. Resolves
    /// Ok once the host's cookie check sees the provider's session cookies
    /// (it has called `Factory::browser_signed_in` by then), or Err
    /// ("ended: …") on Done without a sign-in, Stop, or after 15 minutes.
    fn sign_in_turn(&self, _provider: &str) -> SignInTurn {
        Box::pin(std::future::ready(Err("Claktory's browser can't open a sign-in here yet.".to_owned())))
    }

    /// The card's Done: ends the sign-in turn early, after the cookie check
    /// has run once more.
    fn sign_in_done(&self) {}

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

/// A sign-in turn on its way (`Browser::sign_in_turn`).
pub(crate) type SignInTurn = std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), String>> + Send>>;

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

impl Browser for cdp::Local {}

/// P4's host, as is.
impl Browser for crate::browser_host::Handle {
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

    fn sign_out(&self, provider: &str) -> SigningOut {
        crate::browser_host::Handle::sign_out(self, provider)
    }

    fn signing_out(&self, provider: &str) -> bool {
        crate::browser_host::Handle::signing_out(self, provider)
    }

    fn input_lease(&self, until: std::time::Duration) -> Result<Option<InputLease>, String> {
        crate::browser_host::Handle::input_lease(self, until).map(|lease| Some(Box::new(lease) as InputLease))
    }
}

/// Installs Claktory's browser for errands, and follows its states: the
/// person taking it over or handing it back, stopping it, or it closing.
pub(crate) fn install(factory: &Arc<Factory>, browser: Arc<dyn Browser>) {
    if let Some(states) = browser.states() {
        tokio::spawn(follow(Arc::downgrade(factory), states));
    }
    factory.errands.set_transport(Some(browser));
}

/// What the browser's state means for the errand it was begun for.
async fn follow(factory: std::sync::Weak<Factory>, mut states: tokio::sync::watch::Receiver<BrowserState>) {
    while states.changed().await.is_ok() {
        let state = *states.borrow_and_update();
        let Some(factory) = factory.upgrade() else { return };
        let Some(errand) = factory.errands.begun() else { continue };
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
                let text = format!("{person} handed the browser back. The page may have changed: read it with errand_read before you go on.");
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
                let text = format!(
                    "{said}, which ended errand {} ({}). Tell your lead with errand_report what was done and what's left.",
                    errand.id, errand.purpose
                );
                factory.deliver_event(&agent, text, crate::factory::host_event(format!("Errand {} ended", errand.id)));
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

/// OpenErrand: the person opens an errand for an agent.
pub(crate) fn open_request(factory: &Arc<Factory>, request: &v1::OpenErrand) -> Result<(), crate::factory::RequestError> {
    if !available() {
        return Err(unavailable());
    }
    factory
        .open_errand(None, &request.agent_id, &request.purpose, &request.sites, &request.brief)
        .map(|_| ())
        .map_err(crate::factory::RequestError::Remote)
}

/// ListErrandLog, to the asker only.
pub(crate) fn log_request(factory: &Factory, request: &v1::ListErrandLog) -> Result<v1::ErrandLog, crate::factory::RequestError> {
    if !available() {
        return Err(unavailable());
    }
    Ok(log::list(&factory.store, &request.errand_id)?)
}

/// SetAgentContained, the person's alone.
pub(crate) async fn contain_request(factory: &Arc<Factory>, request: &v1::SetAgentContained) -> Result<(), crate::factory::RequestError> {
    if !available() {
        return Err(unavailable());
    }
    set_contained(factory, &request.agent_id, request.contained).await
}

/// Tests: errands drive a headless Chrome on this computer, in `profile`;
/// false where there's none.
#[doc(hidden)]
pub fn local_chrome_for_test(factory: &Factory, profile: &std::path::Path) -> bool {
    let Some(chrome) = cdp::find_chrome() else { return false };
    match cdp::Local::start(&chrome, profile) {
        Ok(local) => {
            factory.errands.set_transport(Some(Arc::new(local)));
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
            if let Some(factory) = self.factory.upgrade() {
                factory.signed_out(&provider);
            }
        }
    }

    /// As the host does: at once when no errand drives it, else when the
    /// errand ends.
    fn sign_out(&self, provider: &str) -> SigningOut {
        let driven = matches!(*self.states.borrow(), BrowserState::Driven | BrowserState::PersonsTurn | BrowserState::TakenOver);
        if driven {
            self.call(format!("sign out {provider} later"), None);
            crate::locking::lock(&self.signing_out).push(provider.to_owned());
            return Box::pin(std::future::ready(Ok(SignedOut::Pending)));
        }
        self.call(format!("signed out {provider}"), None);
        if let Some(factory) = self.factory.upgrade() {
            factory.signed_out(provider);
        }
        Box::pin(std::future::ready(Ok(SignedOut::Applied)))
    }

    fn signing_out(&self, provider: &str) -> bool {
        crate::locking::lock(&self.signing_out).iter().any(|waiting| waiting == provider)
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
    });
    crate::locking::lock(&TURNS).push((Arc::as_ptr(factory) as usize, turns.clone()));
    install(factory, turns);
    true
}

/// Tests: the browser just signed in to `provider` with `agent`'s secret
/// `name` for its open errand, as a fill from the vault makes it: the card
/// the person gets, as the fill's result carries it.
#[doc(hidden)]
pub fn first_sign_in_for_test(factory: &Arc<Factory>, agent: &str, provider: &str, name: &str) -> serde_json::Value {
    let fillable = factory.vault.fillable(agent, name).expect("fillable");
    assert!(factory.vault.set_browser_account(provider, &fillable.id).expect("kept"), "a first sign-in");
    let errand = factory.errands.open_of(agent).expect("an open errand");
    code::signed_in(factory, agent, &errand, provider, &fillable).expect("the card")
}

/// Tests: the person approves that card (its action, as the asks table
/// keeps it): what Claktory says, or why it couldn't.
#[doc(hidden)]
pub async fn approve_card_for_test(factory: &Arc<Factory>, ask: &str) -> Result<String, String> {
    let action: String = factory.store.db().query_row("SELECT action FROM asks WHERE id = ?1", [ask], |row| row.get(0)).expect("the card");
    crate::card_actions::check(factory, &action)?;
    crate::card_actions::apply(factory, &action).await
}

/// Tests: the browser's account at `provider`, as the vault keeps it.
#[doc(hidden)]
pub fn browser_account_for_test(factory: &Factory, provider: &str) -> Option<String> {
    factory.vault.browser_account(provider)
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

/// Tests: the browser's driven start fails, saying `why`, or succeeds
/// again (None).
#[doc(hidden)]
pub fn browser_unbooted_for_test(factory: &Arc<Factory>, why: Option<&str>) {
    if let Some(turns) = turns_of(factory) {
        *crate::locking::lock(&turns.unbooted) = why.map(str::to_owned);
    }
}

/// Tests: the open errand's page shows `html` (no network in tests).
#[doc(hidden)]
pub async fn show_for_test(factory: &Factory, html: &str) {
    let live = factory.errands.live.lock().await;
    let page = live.as_ref().expect("an errand's page").page.clone();
    drop(live);
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
    *factory.errands.live.lock().await = None;
}

/// Tests: `agent` read `what` from outside, as web_search marks it (a
/// search needs the internet).
#[doc(hidden)]
pub fn read_outside_for_test(factory: &Factory, agent: &str, what: &str) {
    read_outside(factory, agent, what);
}

/// Tests: `factory`'s code and agreement cards may be asked on pages
/// `show_for_test` showed, which have no https origin (shown as
/// https://shown.test).
#[doc(hidden)]
pub fn code_cards_anywhere_for_test(factory: &Arc<Factory>) {
    crate::locking::lock(&code::ANY_ORIGIN_FOR_TEST).push(Arc::downgrade(factory));
}

/// The extension's id.
pub(crate) const ERRANDS: &str = "errands";

/// Whether `agent` may do errands: the person contained it for them
/// (SetAgentContained), kept in factory.db's `errand_agents`, never in
/// config.json, so no setting or tool lifts it.
pub(crate) fn contained(factory: &Factory, agent: &str) -> bool {
    factory.pipes.errand_agent(agent)
}

/// The extensions a contained errand agent keeps, when they're on for it:
/// its errands and Ask. No Messaging: it reaches its lead only with
/// errand_report, fenced, which marks the lead. No Checklist either: its
/// items would reach the lead unmarked (Fugo 37209), and an errand has its
/// own state, log and handoffs.
pub(crate) const KEPT_EXTENSIONS: &[&str] = &[ERRANDS, crate::extensions::ask::ID];

/// Where a contained errand agent works: a folder of its own, and nothing
/// else (`{home}/errands/<agent>`).
pub(crate) fn places(factory: &Factory, agent: &str) -> crate::runner::Places {
    let folder = factory.home.root().join("errands").join(agent);
    if let Err(error) = std::fs::create_dir_all(&folder) {
        tracing::warn!(%error, %agent, "couldn't make an errand agent's folder");
    }
    crate::runner::Places { folder, extra_dirs: Vec::new(), read_only: Vec::new() }
}

/// The errand agents, at start.
/// At start: reads the errands kept, and tells each agent whose errand
/// Claktory's restart ended, so it reports to its lead rather than wait.
/// Recovery codes whose card no longer waits go (`code::sweep_codes`).
pub(crate) fn recover(factory: &Arc<Factory>) {
    code::sweep_codes(factory);
    for errand in factory.errands.load(&factory.store) {
        let told = format!(
            "{RESTARTED} Your errand {} ({}) ended there. Tell your lead with errand_report what was done and what's left; it can open the errand again.",
            errand.id, errand.purpose
        );
        factory.deliver_event(&errand.agent_id, told, crate::factory::host_event("Your errand ended".to_owned()));
    }
}

pub(crate) fn load_agents(store: &crate::store::Store) -> rusqlite::Result<std::collections::BTreeSet<String>> {
    let db = store.db();
    let mut statement = db.prepare("SELECT agent FROM errand_agents")?;
    statement.query_map([], |row| row.get(0))?.collect()
}

/// SetAgentContained, the person's alone: contains `agent` for errands,
/// or lifts it, which turns its errands extension off and stops its
/// errand.
pub(crate) async fn set_contained(factory: &Arc<Factory>, agent: &str, on: bool) -> Result<(), crate::factory::RequestError> {
    use crate::factory::RequestError;
    let (name, admin) = {
        let inner = factory.lock();
        let Some(found) = inner.config.agent(agent) else {
            return Err(RequestError::Remote(format!("There's no agent \"{agent}\".")));
        };
        (found.name.clone(), crate::proposals::is_admin(found))
    };
    if on == factory.pipes.errand_agent(agent) {
        return Ok(());
    }
    if on {
        if admin {
            return Err(RequestError::Remote(format!("{name} is an admin agent: an errand agent can't be one.")));
        }
        if factory.pipes.contained(agent) {
            return Err(RequestError::Remote(format!("{name} is already limited to a pipe: give errands to another agent.")));
        }
        factory
            .store
            .db()
            .execute("INSERT OR REPLACE INTO errand_agents (agent, since) VALUES (?1, ?2)", params![agent, now_ms()])?;
        factory.pipes.set_errand_agent(agent, true);
    } else {
        factory.store.db().execute("DELETE FROM errand_agents WHERE agent = ?1", [agent])?;
        factory.pipes.set_errand_agent(agent, false);
        if let Some(errand) = factory.errands.open_of(agent) {
            factory.end_errand(&errand.id, State::Stopped, "You let its agent out of errands.").await;
        }
        let off = v1::ExtensionChange { id: ERRANDS.to_owned(), enabled: Some(false), ..Default::default() };
        factory.edit_config(|config| crate::extensions::apply_change(config, agent, &off)).await?;
    }
    factory.broadcast_workspace(&factory.lock());
    Ok(())
}

/// A step that may bring a file (R9): the fence lets the page's redirects
/// hop off the sites for a file, and with browser_download a PDF comes as
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
    /// errand_act's dismiss: clear what covers the page read as `document`.
    Dismiss { document: String },
    Navigate { address: String },
    /// browser_download: the file at `address`, on the errand's sites.
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
/// Whether the browser is signed in to `site` (an errand's origin), by
/// the sites it holds cookies for (origins of cookie domains): the same
/// name, or one its name ends in, since a domain's cookie reaches every name
/// under it.
fn signed_in_to(signed_in: &[String], site: &str) -> bool {
    let host = |origin: &str| url::Url::parse(origin).ok().and_then(|url| url.host_str().map(str::to_owned));
    let Some(name) = host(site) else { return false };
    signed_in.iter().filter_map(|origin| host(origin)).any(|domain| name == domain || name.ends_with(&format!(".{domain}")))
}

pub(crate) fn said((error, why): (String, String)) -> String {
    if why.is_empty() { format!("{error}.") } else { format!("{error}: {why}") }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sites_are_https_origins_never_patterns_or_domain_endings() {
        assert_eq!(site("play.google.com").unwrap(), "https://play.google.com");
        assert_eq!(site("https://Accounts.Google.com/").unwrap(), "https://accounts.google.com");
        for refused in ["*.google.com", "https://*.google.com", "com", "co.uk", "https://co.uk", "http://example.com", "https://10.0.0.1", "github.io"] {
            assert!(site(refused).is_err(), "{refused}");
        }
    }

    #[test]
    fn states_read_back_as_written() {
        for state in State::ALL {
            assert_eq!(State::of(state.word()), state);
            assert_eq!(State::from_wire(state.wire() as i32), state);
        }
        assert!(State::PersonsTurn.open() && !State::Done.open());
    }

    /// A step dropped midway (an errand stopped, the connection gone)
    /// leaves no file asked for: the next step's redirects stay on the
    /// sites and its PDFs show (Fugo 41301).
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

    #[test]
    fn signed_in_by_a_domains_cookies() {
        let signed_in = ["https://scrapingcourse.com".to_owned(), "https://accounts.google.com".to_owned()];
        assert!(signed_in_to(&signed_in, "https://www.scrapingcourse.com"));
        assert!(signed_in_to(&signed_in, "https://scrapingcourse.com"));
        assert!(!signed_in_to(&signed_in, "https://notscrapingcourse.com"));
        assert!(!signed_in_to(&signed_in, "https://google.com"), "a name's cookies don't reach the domain above it");
    }

    #[test]
    fn purposes_are_one_plain_line() {
        assert!(plain("purpose", "Fill the Play form", PURPOSE_MAX).is_ok());
        assert!(plain("purpose", "a\nb", PURPOSE_MAX).is_err());
        assert!(plain("purpose", "pay\u{202e}", PURPOSE_MAX).is_err());
        assert!(plain("purpose", "  ", PURPOSE_MAX).is_err());
    }
}
