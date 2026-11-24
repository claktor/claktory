//! web_search: each query goes to the healthiest engine free to answer, and
//! falls through to the next while engines block, break or find nothing,
//! paced and budgeted for the whole Claktory, since every agent shares one
//! IP. Engines that have been answering lately go first, and among those
//! the least asked this hour, so the load is spread. A block is never
//! retried: the engine cools down, and when its cooldown ends it's asked
//! once (a probe, after the healthy ones) before it's back in rotation. Whether an engine may be asked is
//! settled in its turn, after the wait, so requests queued behind a block
//! or the hour's last one don't go out; the same search asked by several
//! callers at once goes out once (a flight), and is dropped when no one
//! waits for it any more.

use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use futures_util::future::BoxFuture;
use serde::{Deserialize, Serialize};
use tokio::sync::watch;

use super::engines::{self, Engine, Policy};
use crate::transcript::now_ms;

const MINUTE: i64 = 60_000;
const HOUR: i64 = 60 * MINUTE;
/// Searches are kept this long…
const CACHE_MS: i64 = HOUR;
/// …a search that found nothing this long…
const EMPTY_MS: i64 = MINUTE;
/// …this many of them.
const CACHE_SIZE: usize = 500;
/// Results kept of a search, the most a query can ask for.
const MAX_HITS: usize = 10;
/// Different searches on their way at once…
const MAX_FLIGHTS: usize = 64;
/// …callers waiting on them…
const MAX_WAITERS: usize = 128;
/// …and of those, one agent's.
const PER_AGENT: usize = 4;
/// A snippet is cut to about this many characters.
const SNIPPET: usize = 200;

#[derive(Clone, Debug, PartialEq, Serialize)]
pub(crate) struct Hit {
    pub(crate) title: String,
    pub(crate) url: String,
    pub(crate) snippet: String,
}

/// What an engine's answer says.
#[derive(Debug, PartialEq)]
pub(crate) enum Outcome {
    Results(Vec<Hit>),
    NoResults,
    Blocked(Block),
    /// A page with no results and no sign of a block: its layout changed.
    /// What it began with, for the log.
    Broken(String),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Block {
    /// A CAPTCHA or bot check.
    Challenge,
    /// 429: too many requests.
    RateLimited,
    /// 403: access denied.
    Denied,
    /// Results that don't match the query: a soft block.
    Degraded,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum Recency {
    Day,
    Week,
    Month,
    Year,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub(crate) struct Options {
    /// "example.com" keeps only that domain; "-example.com" leaves it out.
    pub(crate) domains: Vec<String>,
    pub(crate) recency: Option<Recency>,
    pub(crate) limit: usize,
}

/// One query's answer.
#[derive(Clone, Debug, Serialize)]
pub(crate) struct Answer {
    pub(crate) q: String,
    /// The engine that answered; empty when none could.
    pub(crate) engine: String,
    pub(crate) results: Vec<Hit>,
    /// Why there are no results, when no engine could answer.
    #[serde(skip)]
    pub(crate) unavailable: Option<String>,
}

/// How an engine is doing. Kept across restarts (`Searcher::new`), so an
/// update during a cooldown doesn't knock on the engine again.
#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct Health {
    /// Left alone until then (ms), and why.
    pub(crate) cooling_until: i64,
    pub(crate) why: String,
    /// Blocks in a row: each doubles the next cooldown; a success resets it.
    pub(crate) strikes: u32,
    /// When it was asked, over the last hour.
    pub(crate) asked: VecDeque<i64>,
    /// How often it failed lately, from 0 (never) to 1 (every time): each
    /// answer moves it 30% of the way to 0 or 1.
    pub(crate) failing: f64,
    /// Answers about something else in a row (soft blocks): two cool it down.
    pub(crate) degraded: u32,
}

impl Health {
    /// Why it can't be asked now (with when it can), or None.
    pub(crate) fn unavailable(&mut self, engine: Engine, policy: &Policy, now: i64) -> Option<String> {
        if now < self.cooling_until {
            return Some(format!("{} {} (trying again after {})", engine.name(), self.why, clock(self.cooling_until)));
        }
        while self.asked.front().is_some_and(|time| *time <= now - HOUR) {
            self.asked.pop_front();
        }
        if self.asked.len() >= policy.per_hour {
            let free = self.asked.front().map_or(now, |time| time + HOUR);
            return Some(format!("{} has had its {} searches this hour (after {})", engine.name(), self.asked.len(), clock(free)));
        }
        None
    }

    /// Whether its cooldown is over but it hasn't answered since: its next
    /// request is a probe.
    pub(crate) fn probing(&self, now: i64) -> bool {
        self.cooling_until != 0 && now >= self.cooling_until
    }

    /// When it can be asked again: now, or the end of its cooldown or of
    /// its hour's budget.
    fn free_at(&self, policy: &Policy, now: i64) -> i64 {
        let budget = if self.asked.len() >= policy.per_hour { self.asked.front().map_or(now, |time| time + HOUR) } else { now };
        budget.max(self.cooling_until).max(now)
    }

    /// Records what an answer said, and how long to leave the engine alone.
    pub(crate) fn record(&mut self, policy: &Policy, outcome: &Result<Outcome, String>, now: i64) {
        let answered = matches!(outcome, Ok(Outcome::Results(_) | Outcome::NoResults));
        self.failing = self.failing * 0.7 + if answered { 0.0 } else { 0.3 };
        if !matches!(outcome, Ok(Outcome::Blocked(Block::Degraded))) {
            self.degraded = 0;
        }
        let doubled = |max: i64| (HOUR << self.strikes.min(16)).min(max);
        let (cooldown, why) = match outcome {
            Ok(Outcome::Results(_) | Outcome::NoResults) => {
                self.strikes = 0;
                self.cooling_until = 0;
                self.why.clear();
                return;
            }
            Ok(Outcome::Blocked(Block::Challenge)) => (doubled(policy.challenge_max_ms), "asked for a CAPTCHA"),
            Ok(Outcome::Blocked(Block::RateLimited)) => (doubled(policy.limited_max_ms), "rate-limited this computer"),
            Ok(Outcome::Blocked(Block::Denied)) => (HOUR, "denied access"),
            // Some engines give a scraper's long queries unrelated results:
            // once may be the query, twice in a row is the engine.
            Ok(Outcome::Blocked(Block::Degraded)) => {
                self.degraded += 1;
                if self.degraded < 2 {
                    return;
                }
                (10 * MINUTE, "returned results that don't match (a soft block)")
            }
            Ok(Outcome::Broken(_)) => (HOUR, "returned a page Claktory can't read"),
            Err(_) => (MINUTE, "couldn't be reached"),
        };
        if matches!(outcome, Ok(Outcome::Blocked(Block::Challenge | Block::RateLimited))) {
            self.strikes += 1;
        }
        self.cooling_until = now + cooldown;
        self.why = why.to_owned();
    }
}

/// Asks an engine for a query: what its answer says, or why it couldn't
/// be had. Real engines in use (`browser`); scripted ones in tests.
pub(crate) type Wire = Arc<dyn Fn(Engine, String, Options) -> BoxFuture<'static, Result<Outcome, String>> + Send + Sync>;

/// What a search is the same as: the query as written (case and quotes
/// count, runs of spaces don't), the domains as a set, and the recency.
/// The limit only cuts what's shown, so it isn't part of it.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct Key {
    query: String,
    domains: Vec<String>,
    recency: Option<Recency>,
}

/// What a search found, for everyone who asked it.
#[derive(Clone, Debug)]
struct Found {
    /// The engine that answered; empty when none could.
    engine: String,
    /// Within the domains asked for, up to `MAX_HITS`.
    hits: Vec<Hit>,
    /// Why there are no results, when no engine could answer.
    unavailable: Option<String>,
    /// The engines that challenged this computer, for the person.
    challenged: Vec<&'static str>,
}

/// A search on its way: everyone asking the same waits for the one request.
struct Flight {
    id: u64,
    done: watch::Receiver<Option<Found>>,
    waiters: usize,
    task: tokio::task::AbortHandle,
}

/// Search for the whole Factory.
pub(crate) struct Searcher {
    shared: Arc<Shared>,
}

struct Shared {
    wire: Wire,
    pace: fn(Engine) -> Policy,
    /// Where engines' health is kept across restarts; None in tests.
    saved: Option<PathBuf>,
    /// One turn per engine: held while waiting for its pace and through
    /// its answer, so an engine is asked one request at a time, and each is
    /// admitted only in its turn.
    turns: HashMap<Engine, tokio::sync::Mutex<i64>>,
    state: Mutex<SearchState>,
    /// Writes of `saved`, one at a time.
    saving: Mutex<()>,
    /// Requests that passed the first look and are waiting for their turn.
    #[cfg(test)]
    queued: std::sync::atomic::AtomicUsize,
}

#[derive(Default)]
struct SearchState {
    health: HashMap<Engine, Health>,
    cache: HashMap<Key, (i64, Found)>,
    flights: HashMap<Key, Flight>,
    next_flight: u64,
    /// Searches each agent is waiting on.
    waiting: HashMap<String, usize>,
}

impl Shared {
    fn state(&self) -> std::sync::MutexGuard<'_, SearchState> {
        self.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Keeps engines' health for the next start.
    fn save(&self) {
        let Some(path) = &self.saved else { return };
        let text = {
            let state = self.state();
            let by_name: HashMap<&str, &Health> = state.health.iter().map(|(engine, health)| (engine.name(), health)).collect();
            serde_json::to_string(&by_name).unwrap_or_default()
        };
        let _saving = self.saving.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let temporary = path.with_extension("json.tmp");
        if let Err(error) = std::fs::write(&temporary, text).and_then(|()| std::fs::rename(&temporary, path)) {
            tracing::warn!(%error, "couldn't keep search engines' health");
        }
    }
}

impl Searcher {
    /// Searches the real engines, keeping their health in `saved`.
    pub(crate) fn new(saved: Option<PathBuf>) -> Self {
        Searcher::with(browser(), Engine::policy, saved)
    }

    fn with(wire: Wire, pace: fn(Engine) -> Policy, saved: Option<PathBuf>) -> Self {
        let mut health = HashMap::new();
        if let Some(text) = saved.as_ref().and_then(|path| std::fs::read_to_string(path).ok()) {
            let by_name: HashMap<String, Health> = serde_json::from_str(&text).unwrap_or_default();
            for (name, kept) in by_name {
                if let Some(engine) = engines::ORDER.iter().find(|engine| engine.name() == name) {
                    health.insert(*engine, kept);
                }
            }
        }
        let shared = Shared {
            wire,
            pace,
            saved,
            turns: engines::ORDER.iter().map(|engine| (*engine, tokio::sync::Mutex::new(0))).collect(),
            state: Mutex::new(SearchState { health, ..SearchState::default() }),
            saving: Mutex::new(()),
            #[cfg(test)]
            queued: std::sync::atomic::AtomicUsize::new(0),
        };
        Searcher { shared: Arc::new(shared) }
    }

    /// Answers each query in turn, for `agent`. Also says what blocked,
    /// once, for the person: a block hits their own browser's searches too.
    pub(crate) async fn search(&self, agent: &str, queries: &[String], options: &Options) -> (Vec<Answer>, Vec<String>) {
        let mut answers = Vec::new();
        let mut blocked = Vec::new();
        for query in queries {
            let (answer, challenged) = self.one(agent, query, options).await;
            answers.push(answer);
            for engine in challenged {
                if !blocked.contains(&engine) {
                    blocked.push(engine);
                }
            }
        }
        let notice = match blocked.as_slice() {
            [] => None,
            [one] => Some(format!("{one} is challenging this computer's IP")),
            [rest @ .., last] => Some(format!("{} and {last} are challenging this computer's IP", rest.join(", "))),
        };
        let notices = notice
            .map(|notice| format!("{notice}; the person's own searches there may show a CAPTCHA for about an hour."))
            .into_iter()
            .collect();
        (answers, notices)
    }

    async fn one(&self, agent: &str, query: &str, options: &Options) -> (Answer, Vec<&'static str>) {
        let key = Key { query: normalized(query), domains: options.domains.clone(), recency: options.recency };
        let answer = |found: &Found| Answer {
            q: query.to_owned(),
            engine: found.engine.clone(),
            results: found.hits.iter().take(options.limit.max(1)).cloned().collect(),
            unavailable: found.unavailable.clone(),
        };
        let busy = |why: String| Answer { q: query.to_owned(), engine: String::new(), results: Vec::new(), unavailable: Some(why) };
        let (id, mut done) = {
            let mut state = self.shared.state();
            let now = now_ms();
            if let Some((time, found)) = state.cache.get(&key)
                && now - time < fresh_for(found)
            {
                return (answer(found), Vec::new());
            }
            if state.waiting.get(agent).copied().unwrap_or(0) >= PER_AGENT {
                return (busy(format!("you already have {PER_AGENT} searches waiting; wait for them")), Vec::new());
            }
            let waiters: usize = state.flights.values().map(|flight| flight.waiters).sum();
            if waiters >= MAX_WAITERS || (!state.flights.contains_key(&key) && state.flights.len() >= MAX_FLIGHTS) {
                return (busy("too many searches are waiting; try again in a minute".to_owned()), Vec::new());
            }
            *state.waiting.entry(agent.to_owned()).or_default() += 1;
            match state.flights.get_mut(&key) {
                Some(flight) => {
                    flight.waiters += 1;
                    (flight.id, flight.done.clone())
                }
                None => {
                    let id = state.next_flight;
                    state.next_flight += 1;
                    let (sender, receiver) = watch::channel(None);
                    let task = tokio::spawn(fly(self.shared.clone(), key.clone(), id, query.to_owned(), options.clone(), sender));
                    state.flights.insert(key.clone(), Flight { id, done: receiver.clone(), waiters: 1, task: task.abort_handle() });
                    (id, receiver)
                }
            }
        };
        // Leaving (answered, or the caller gone) frees its place, and the
        // search once no one waits on it any more.
        let _waiting = Waiting { shared: &self.shared, key, id, agent };
        let found = match done.wait_for(Option::is_some).await {
            Ok(found) => found.clone(),
            Err(_) => None,
        };
        match found {
            Some(found) => (answer(&found), found.challenged.clone()),
            None => (busy("the search stopped before it finished".to_owned()), Vec::new()),
        }
    }
}

/// A caller's place on a flight.
struct Waiting<'a> {
    shared: &'a Shared,
    key: Key,
    id: u64,
    agent: &'a str,
}

impl Drop for Waiting<'_> {
    fn drop(&mut self) {
        let mut state = self.shared.state();
        if let Some(count) = state.waiting.get_mut(self.agent) {
            *count = count.saturating_sub(1);
            if *count == 0 {
                state.waiting.remove(self.agent);
            }
        }
        if let Some(flight) = state.flights.get_mut(&self.key)
            && flight.id == self.id
        {
            flight.waiters = flight.waiters.saturating_sub(1);
            if flight.waiters == 0
                && let Some(flight) = state.flights.remove(&self.key)
            {
                flight.task.abort();
            }
        }
    }
}

/// How long a search's answer is reused: results for an hour, nothing
/// found for a minute.
fn fresh_for(found: &Found) -> i64 {
    if found.hits.is_empty() { EMPTY_MS } else { CACHE_MS }
}

/// One search, for everyone waiting on it: tries each engine in turn, then
/// keeps what it found and hands it over.
async fn fly(shared: Arc<Shared>, key: Key, id: u64, query: String, options: Options, done: watch::Sender<Option<Found>>) {
    let found = ask_engines(&shared, &query, &options).await;
    {
        let mut state = shared.state();
        if found.unavailable.is_none() {
            let now = now_ms();
            if state.cache.len() >= CACHE_SIZE {
                state.cache.retain(|_, (time, found)| now - *time < fresh_for(found));
            }
            if state.cache.len() >= CACHE_SIZE
                && let Some(oldest) = state.cache.iter().min_by_key(|(_, (time, _))| *time).map(|(key, _)| key.clone())
            {
                state.cache.remove(&oldest);
            }
            state.cache.insert(key.clone(), (now, found.clone()));
        }
        if state.flights.get(&key).is_some_and(|flight| flight.id == id) {
            state.flights.remove(&key);
        }
    }
    let _ = done.send(Some(found));
}

/// The engines to try for `options`, best first: those answering lately
/// (by how often they failed, in steps of a tenth), then the least asked
/// this hour for its budget, then `engines::ORDER`; engines whose cooldown
/// ended and haven't answered since come after all of those. Engines that
/// can't be asked now are left out.
fn order(shared: &Shared, options: &Options, now: i64) -> Vec<Engine> {
    let mut state = shared.state();
    let mut ranked: Vec<(bool, i64, usize, usize, Engine)> = Vec::new();
    for (index, engine) in engines::ORDER.into_iter().enumerate() {
        if !engine.supports(options) {
            continue;
        }
        let policy = (shared.pace)(engine);
        let health = state.health.entry(engine).or_default();
        if health.unavailable(engine, &policy, now).is_some() {
            continue;
        }
        let load = health.asked.len() * 1000 / policy.per_hour.max(1);
        ranked.push((health.probing(now), (health.failing * 10.0).round() as i64, load, index, engine));
    }
    ranked.sort_by_key(|(probing, failing, load, index, _)| (*probing, *failing, *load, *index));
    ranked.into_iter().map(|(.., engine)| engine).collect()
}

/// Tries engines in turn (`order`) until one gives results that match the
/// query. A block, a page it can't read or results about something else
/// move on to the next; so does one engine finding nothing, but two
/// finding nothing is the answer.
async fn ask_engines(shared: &Shared, query: &str, options: &Options) -> Found {
    let mut reasons: Vec<(Engine, String)> = Vec::new();
    let mut challenged = Vec::new();
    let mut nothing: Option<String> = None;
    for engine in order(shared, options, now_ms()) {
        let outcome = match dispatch(shared, engine, query, options).await {
            Ok(outcome) => outcome,
            Err(why) => {
                reasons.push((engine, why));
                continue;
            }
        };
        let name = engine.name().to_owned();
        match outcome {
            Ok(Outcome::Results(hits)) => {
                return Found { engine: name, hits: keep(hits, &options.domains), unavailable: None, challenged };
            }
            Ok(Outcome::NoResults) => {
                if nothing.is_some() {
                    return Found { engine: name, hits: Vec::new(), unavailable: None, challenged };
                }
                nothing = Some(name);
            }
            Ok(Outcome::Blocked(block)) => {
                tracing::warn!(engine = engine.name(), ?block, "a search engine blocked this computer");
                if block == Block::Challenge {
                    challenged.push(engine.name());
                }
                let state = shared.state();
                let health = &state.health[&engine];
                let why = if health.why.is_empty() { "returned results that don't match" } else { health.why.as_str() };
                reasons.push((engine, format!("{} {why}", engine.name())));
            }
            Ok(Outcome::Broken(start)) => {
                tracing::warn!(engine = engine.name(), start, "a search engine's page couldn't be read; its layout may have changed");
                reasons.push((engine, format!("{} returned a page Claktory can't read", engine.name())));
            }
            Err(error) => {
                tracing::warn!(engine = engine.name(), %error, "a search engine couldn't be reached");
                reasons.push((engine, format!("{} couldn't be reached ({error})", engine.name())));
            }
        }
    }
    if let Some(engine) = nothing {
        return Found { engine, hits: Vec::new(), unavailable: None, challenged };
    }
    Found { engine: String::new(), hits: Vec::new(), unavailable: Some(unavailable(shared, options, &reasons)), challenged }
}

/// Why no engine could answer, for the agent: when to try again, and what
/// each engine did.
fn unavailable(shared: &Shared, options: &Options, reasons: &[(Engine, String)]) -> String {
    let now = now_ms();
    let mut state = shared.state();
    let free_at = engines::ORDER
        .into_iter()
        .filter(|engine| engine.supports(options))
        .map(|engine| state.health.entry(engine).or_default().free_at(&(shared.pace)(engine), now))
        .min()
        .unwrap_or(now);
    let offline = !reasons.is_empty() && reasons.iter().all(|(_, reason)| reason.contains("couldn't be reached"));
    // Engines that weren't asked, and why.
    let mut details: Vec<String> = reasons.iter().map(|(_, reason)| reason.clone()).collect();
    for engine in engines::ORDER.into_iter().filter(|engine| engine.supports(options)) {
        if reasons.iter().any(|(asked, _)| *asked == engine) {
            continue;
        }
        if let Some(why) = state.health.entry(engine).or_default().unavailable(engine, &(shared.pace)(engine), now) {
            details.push(why);
        }
    }
    let details = details.join("; ");
    if offline {
        return format!("no search engine could be reached from this computer; check its internet connection ({details})");
    }
    let wait = if free_at <= now + MINUTE {
        "in a minute".to_owned()
    } else {
        let minutes = (free_at - now + MINUTE - 1) / MINUTE;
        format!("in {minutes} minute{} (after {})", if minutes == 1 { "" } else { "s" }, clock(free_at))
    };
    format!("every search engine is busy or blocking searches from this computer; try again {wait} ({details})")
}

/// Asks one engine, in its turn: Err says why it wasn't asked. Whether it
/// may be asked is settled only once its turn comes, so a block or the
/// hour's budget reached by the requests before holds for the ones that
/// waited behind them; and the answer is recorded before the turn passes.
async fn dispatch(shared: &Shared, engine: Engine, query: &str, options: &Options) -> Result<Result<Outcome, String>, String> {
    let policy = (shared.pace)(engine);
    // A first look, so a query doesn't wait for an engine that's out.
    if let Some(why) = shared.state().health.entry(engine).or_default().unavailable(engine, &policy, now_ms()) {
        return Err(why);
    }
    #[cfg(test)]
    shared.queued.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let mut last = shared.turns[&engine].lock().await;
    let wait = *last + policy.gap_ms + jitter(policy.jitter_ms) - now_ms();
    if wait > 0 {
        tokio::time::sleep(Duration::from_millis(wait as u64)).await;
    }
    {
        let mut state = shared.state();
        let health = state.health.entry(engine).or_default();
        let now = now_ms();
        if let Some(why) = health.unavailable(engine, &policy, now) {
            return Err(why);
        }
        health.asked.push_back(now);
        *last = now;
    }
    shared.save();
    let outcome = (shared.wire)(engine, query.to_owned(), options.clone()).await;
    *last = now_ms();
    let outcome = match outcome {
        Ok(Outcome::Results(hits)) if !relevant(query, &hits) => Ok(Outcome::Blocked(Block::Degraded)),
        other => other,
    };
    shared.state().health.entry(engine).or_default().record(&policy, &outcome, now_ms());
    shared.save();
    Ok(outcome)
}

/// The real engines, each asked through its own browser-like client.
fn browser() -> Wire {
    let clients: Arc<HashMap<Engine, OnceLock<Result<primp::Client, String>>>> =
        Arc::new(engines::ORDER.iter().map(|engine| (*engine, OnceLock::new())).collect());
    Arc::new(move |engine, query, options| {
        let clients = clients.clone();
        Box::pin(async move {
            let client = clients[&engine].get_or_init(|| client(engine)).clone()?;
            let response = engine.request(&client, &query, &options).send().await.map_err(|error| error.to_string())?;
            let (mut status, mut url) = (response.status(), response.url().to_string());
            let mut body = response.text().await.map_err(|error| error.to_string())?;
            // A wait page: waited out, then its link followed, once.
            if let Some((wait, next)) = engines::follow(engine, &body) {
                tokio::time::sleep(wait).await;
                let response = client.get(&next).header("Referer", &url).send().await.map_err(|error| error.to_string())?;
                (status, url) = (response.status(), response.url().to_string());
                body = response.text().await.map_err(|error| error.to_string())?;
            }
            Ok(engine.parse(status, &url, &body))
        })
    })
}

/// The engine's client: its browser, cookies and connections, kept for the
/// whole Factory as a browser keeps a tab.
fn client(engine: Engine) -> Result<primp::Client, String> {
    let profile = engine.profile();
    let jar = Arc::new(primp::cookie::Jar::default());
    let (site, cookies) = engine.cookies();
    if let Ok(url) = primp::Url::parse(site) {
        for cookie in cookies {
            jar.add_cookie_str(cookie, &url);
        }
    }
    primp::Client::builder()
        .impersonate(profile.browser)
        .impersonate_os(profile.os)
        .cookie_provider(jar)
        .timeout(Duration::from_secs(10))
        .pool_idle_timeout(Duration::from_secs(90))
        .build()
        .map_err(|error| format!("can't set up {}: {error}", engine.name()))
}

/// Up to `max_ms` at random, so requests aren't evenly spaced.
fn jitter(max_ms: i64) -> i64 {
    if max_ms <= 0 {
        return 0;
    }
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |time| time.subsec_nanos());
    i64::from(nanos) % max_ms
}

/// A time today, as "22:41", in this computer's time zone.
pub(crate) fn clock(ms: i64) -> String {
    let time = jiff::Timestamp::from_millisecond(ms).unwrap_or_default();
    time.to_zoned_datetime(jiff::tz::TimeZone::system()).strftime("%H:%M").to_string()
}

/// The query as the engines read it: runs of spaces are one, but case and
/// quotes are kept, since they can change what's found.
fn normalized(query: &str) -> String {
    query.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Domains as a set: trimmed, lower case, sorted, each once.
pub(crate) fn canonical_domains(domains: Vec<String>) -> Vec<String> {
    let mut domains: Vec<String> = domains
        .into_iter()
        .map(|domain| domain.trim().trim_end_matches('.').to_lowercase())
        .filter(|domain| !domain.is_empty() && domain != "-")
        .collect();
    domains.sort();
    domains.dedup();
    domains
}

const STOP_WORDS: [&str; 24] = [
    "the", "and", "for", "with", "how", "what", "why", "when", "where", "who", "which", "does", "are", "was", "can", "from", "into",
    "that", "this", "use", "using", "not", "you", "your",
];

/// Whether results match their query: a page of answers about something
/// else is a soft block (an engine that degrades results for scrapers).
/// With at least 3 terms, at least half of the top 5 must hold 2 of them.
pub(crate) fn relevant(query: &str, hits: &[Hit]) -> bool {
    let terms: Vec<String> = query
        .split_whitespace()
        .filter(|word| !word.contains(':') && !word.starts_with('-'))
        .flat_map(|word| word.split(|c: char| !c.is_alphanumeric()))
        .map(str::to_lowercase)
        .filter(|term| term.chars().count() >= 3 && !STOP_WORDS.contains(&term.as_str()))
        .collect();
    if terms.len() < 3 {
        return true;
    }
    let top: Vec<&Hit> = hits.iter().take(5).collect();
    let matching = top
        .iter()
        .filter(|hit| {
            let text = format!("{} {} {}", hit.title, hit.url, hit.snippet).to_lowercase();
            terms.iter().filter(|term| text.contains(term.as_str())).count() >= 2
        })
        .count();
    matching * 2 >= top.len()
}

/// The results within the domains asked for, up to `MAX_HITS`.
fn keep(hits: Vec<Hit>, domains: &[String]) -> Vec<Hit> {
    let host = |url: &str| primp::Url::parse(url).ok().and_then(|url| url.host_str().map(str::to_lowercase)).unwrap_or_default();
    let within = |host: &str, domain: &str| {
        let domain = domain.trim().to_lowercase();
        host == domain || host.ends_with(&format!(".{domain}"))
    };
    let keep: Vec<&String> = domains.iter().filter(|domain| !domain.starts_with('-')).collect();
    let drop: Vec<&str> = domains.iter().filter_map(|domain| domain.strip_prefix('-')).collect();
    hits.into_iter()
        .filter(|hit| {
            let host = host(&hit.url);
            (keep.is_empty() || keep.iter().any(|domain| within(&host, domain))) && !drop.iter().any(|domain| within(&host, domain))
        })
        .take(MAX_HITS)
        .collect()
}

/// The answers as the agent reads them: each query, the engine that
/// answered, and its results, each with its site (the address's origin) in
/// brackets before its title and its address on its own line.
pub(crate) fn render(answers: &[Answer]) -> String {
    let mut out = String::new();
    for (index, answer) in answers.iter().enumerate() {
        if index > 0 {
            out.push('\n');
        }
        let number = index + 1;
        match &answer.unavailable {
            Some(why) => out.push_str(&format!("Q{number} \"{}\": no results: {why}.\n", answer.q)),
            None if answer.results.is_empty() => out.push_str(&format!("Q{number} \"{}\" ({}): no results.\n", answer.q, answer.engine)),
            None => {
                out.push_str(&format!("Q{number} \"{}\" ({}):\n", answer.q, answer.engine));
                for (rank, hit) in answer.results.iter().enumerate() {
                    // Its site first, in ASCII, from the address, so a title can't pass for another.
                    let url = primp::Url::parse(hit.url.trim()).map_or_else(|_| hit.url.clone(), |url| url.to_string());
                    let site = super::fence::origin(&url).unwrap_or_else(|| "unknown site".to_owned());
                    out.push_str(&format!("{}. [{site}] {}\n   {url}\n", rank + 1, hit.title));
                    if !hit.snippet.is_empty() {
                        out.push_str(&format!("   {}\n", trimmed(&hit.snippet, SNIPPET)));
                    }
                }
            }
        }
    }
    out
}

fn trimmed(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_owned();
    }
    let head: String = text.chars().take(max).collect();
    format!("{}…", head.trim_end())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hit(title: &str, url: &str) -> Hit {
        Hit { title: title.into(), url: url.into(), snippet: String::new() }
    }

    #[test]
    fn a_block_cools_an_engine_down_doubling_and_a_success_resets_it() {
        let mut health = Health::default();
        let engine = Engine::DuckDuckGo;
        let policy = engine.policy();
        let now = 1_000_000_000_000;
        assert_eq!(health.unavailable(engine, &policy, now), None);
        health.record(&policy, &Ok(Outcome::Blocked(Block::Challenge)), now);
        assert_eq!(health.cooling_until, now + HOUR);
        let why = health.unavailable(engine, &policy, now + 1).expect("cooling");
        assert!(why.starts_with("DuckDuckGo asked for a CAPTCHA (trying again after "), "{why}");
        health.record(&policy, &Ok(Outcome::Blocked(Block::Challenge)), now + HOUR);
        assert_eq!(health.cooling_until, now + HOUR + 2 * HOUR, "doubled");
        health.record(&policy, &Ok(Outcome::Results(vec![])), now + 3 * HOUR);
        assert_eq!(health.strikes, 0);
        health.record(&policy, &Ok(Outcome::Blocked(Block::Challenge)), now + 3 * HOUR);
        assert_eq!(health.cooling_until, now + 4 * HOUR, "back to an hour");
        // Doubling stops at its cap; a network error is a minute.
        let mut brave = Health { strikes: 10, ..Health::default() };
        brave.record(&Engine::Brave.policy(), &Ok(Outcome::Blocked(Block::RateLimited)), now);
        assert_eq!(brave.cooling_until, now + 6 * HOUR);
        brave.record(&Engine::Brave.policy(), &Err("timed out".into()), now);
        assert_eq!(brave.cooling_until, now + MINUTE);
    }

    #[test]
    fn an_engine_past_its_hourly_budget_waits() {
        let mut health = Health::default();
        let now = 1_000_000_000_000;
        for minute in 0..15 {
            health.asked.push_back(now + minute * MINUTE);
        }
        let later = now + 30 * MINUTE;
        let (brave, duckduckgo) = (Engine::Brave.policy(), Engine::DuckDuckGo.policy());
        assert!(health.unavailable(Engine::Brave, &brave, later).expect("over").contains("has had its 15 searches this hour"));
        assert_eq!(health.unavailable(Engine::Brave, &brave, now + HOUR + 1), None, "the oldest left the hour");
        assert_eq!(health.unavailable(Engine::DuckDuckGo, &duckduckgo, later), None, "DuckDuckGo allows 60");
    }

    #[test]
    fn results_about_something_else_are_a_soft_block() {
        let query = "tokio select cancellation safety";
        let tokyo = [
            hit("Tokyo travel guide", "https://www.japan-guide.com/e/e2164.html"),
            hit("Tokyo hotels", "https://www.booking.com/city/jp/tokyo.html"),
            hit("Tokyo weather", "https://weather.com/tokyo"),
            hit("Things to do in Tokyo", "https://www.tripadvisor.com/tokyo"),
            hit("tokio - Rust", "https://docs.rs/tokio"),
        ];
        assert!(!relevant(query, &tokyo));
        let rust = [
            hit("Cancellation safety in tokio::select!", "https://docs.rs/tokio/latest/tokio/macro.select.html"),
            hit("Tokio select and cancellation", "https://tokio.rs/tokio/tutorial/select"),
            hit("Unrelated", "https://example.com"),
        ];
        assert!(relevant(query, &rust));
        assert!(relevant("rust", &tokyo), "too few terms to judge");
    }

    #[test]
    fn domains_are_kept_or_dropped_and_read_as_a_set() {
        let hits = vec![hit("a", "https://www.sqlite.org/wal.html"), hit("b", "https://w3schools.com/x"), hit("c", "https://sqlite.org/c")];
        assert_eq!(keep(hits.clone(), &["sqlite.org".into()]).len(), 2);
        assert_eq!(keep(hits, &["-w3schools.com".into()]).len(), 2);
        let many: Vec<Hit> = (0..20).map(|n| hit("x", &format!("https://example.com/{n}"))).collect();
        assert_eq!(keep(many, &[]).len(), MAX_HITS);
        let domains = canonical_domains(vec![" SQLite.org. ".into(), "-b.com".into(), "sqlite.org".into(), "".into(), "-".into()]);
        assert_eq!(domains, ["-b.com", "sqlite.org"]);
    }

    #[test]
    fn answers_read_one_address_per_line() {
        let answers = vec![
            Answer {
                q: "sqlite wal".into(),
                engine: "DuckDuckGo".into(),
                results: vec![Hit { title: "WAL".into(), url: "https://sqlite.org/wal.html".into(), snippet: "x".repeat(300) }],
                unavailable: None,
            },
            Answer { q: "other".into(), engine: String::new(), results: vec![], unavailable: Some("Brave found nothing".into()) },
        ];
        let text = render(&answers);
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines[0], "Q1 \"sqlite wal\" (DuckDuckGo):");
        assert_eq!(lines[1..3], ["1. [https://sqlite.org] WAL", "   https://sqlite.org/wal.html"]);
        assert_eq!(lines[3].chars().count(), 3 + 200 + 1, "trimmed with …");
        assert_eq!(lines[5], "Q2 \"other\": no results: Brave found nothing.");
        let empty = vec![Answer { q: "x".into(), engine: "DuckDuckGo".into(), results: vec![], unavailable: None }];
        assert_eq!(render(&empty), "Q1 \"x\" (DuckDuckGo): no results.\n");
    }

    /// A scripted engine: keeps what it was asked, holds every request
    /// until let go, then answers as `answer` says.
    struct Fake {
        asked: Mutex<Vec<(Engine, String)>>,
        open: watch::Sender<bool>,
        answer: fn(Engine, &str) -> Result<Outcome, String>,
        /// Requests dropped before they were answered.
        dropped: std::sync::atomic::AtomicUsize,
    }

    /// A request on its way; counted as dropped unless answered.
    struct Pending(Arc<Fake>, bool);

    impl Drop for Pending {
        fn drop(&mut self) {
            if !self.1 {
                self.0.dropped.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            }
        }
    }

    impl Fake {
        fn new(answer: fn(Engine, &str) -> Result<Outcome, String>) -> Arc<Fake> {
            Arc::new(Fake { asked: Mutex::default(), open: watch::Sender::new(false), answer, dropped: Default::default() })
        }

        fn wire(self: &Arc<Self>) -> Wire {
            let fake = self.clone();
            Arc::new(move |engine, query, _options| {
                let fake = fake.clone();
                Box::pin(async move {
                    fake.asked.lock().expect("lock").push((engine, query.clone()));
                    let mut pending = Pending(fake.clone(), false);
                    let mut open = fake.open.subscribe();
                    let _ = open.wait_for(|open| *open).await;
                    pending.1 = true;
                    (fake.answer)(engine, &query)
                })
            })
        }

        fn let_go(&self) {
            self.open.send_replace(true);
        }

        fn asked(&self, engine: Engine) -> usize {
            self.asked.lock().expect("lock").iter().filter(|(asked, _)| *asked == engine).count()
        }

        fn total(&self) -> usize {
            self.asked.lock().expect("lock").len()
        }

        fn dropped(&self) -> usize {
            self.dropped.load(std::sync::atomic::Ordering::SeqCst)
        }
    }

    /// No waiting between requests, and 3 an hour.
    fn quick(_engine: Engine) -> Policy {
        Policy { gap_ms: 0, jitter_ms: 0, per_hour: 3, challenge_max_ms: 24 * HOUR, limited_max_ms: 6 * HOUR }
    }

    fn results(query: &str) -> Vec<Hit> {
        (0..12).map(|n| hit(&format!("{query} {n}"), &format!("https://example.com/{n}"))).collect()
    }

    async fn until(what: &str, holds: impl Fn() -> bool) {
        for _ in 0..1000 {
            if holds() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        panic!("never: {what}");
    }

    fn waiters(searcher: &Searcher) -> usize {
        searcher.shared.state().flights.values().map(|flight| flight.waiters).sum()
    }

    fn queued(searcher: &Searcher) -> usize {
        searcher.shared.queued.load(std::sync::atomic::Ordering::SeqCst)
    }

    fn search(searcher: &Arc<Searcher>, agent: &str, query: &str) -> tokio::task::JoinHandle<(Vec<Answer>, Vec<String>)> {
        let (searcher, agent, query) = (searcher.clone(), agent.to_owned(), query.to_owned());
        tokio::spawn(async move { searcher.search(&agent, &[query], &Options { limit: 8, ..Options::default() }).await })
    }

    #[tokio::test]
    async fn the_same_search_asked_twenty_times_at_once_goes_out_once() {
        let fake = Fake::new(|_, query| Ok(Outcome::Results(results(query))));
        let searcher = Arc::new(Searcher::with(fake.wire(), quick, None));
        let tasks: Vec<_> = (0..20).map(|n| search(&searcher, &format!("agent{}", n % 5), "tokio select")).collect();
        until("twenty wait on one search", || waiters(&searcher) == 20).await;
        // Each agent has 4 waiting: a fifth is turned away at once.
        let (busy, _) = searcher.search("agent0", &["other".to_owned()], &Options::default()).await;
        assert!(busy[0].unavailable.as_deref().is_some_and(|why| why.contains("4 searches waiting")), "{busy:?}");

        fake.let_go();
        for task in tasks {
            let (answers, _) = task.await.expect("search");
            assert_eq!((answers[0].engine.as_str(), answers[0].results.len()), ("DuckDuckGo", 8));
        }
        assert_eq!(fake.total(), 1);
        assert!(searcher.shared.state().waiting.is_empty() && searcher.shared.state().flights.is_empty());

        // A different limit is a slice of the same answer; spacing doesn't
        // count, but case and quotes do; each caller gets its own words back.
        let options = Options { limit: 3, ..Options::default() };
        let (answers, _) = searcher.search("a", &["  tokio   select ".to_owned()], &options).await;
        assert_eq!((answers[0].q.as_str(), answers[0].results.len()), ("  tokio   select ", 3));
        assert_eq!(fake.total(), 1);
        searcher.search("a", &["Tokio select".to_owned()], &options).await;
        searcher.search("a", &["\"tokio select\"".to_owned()], &options).await;
        assert_eq!(fake.total(), 3);
    }

    #[tokio::test]
    async fn a_search_no_one_waits_for_any_more_is_dropped() {
        let fake = Fake::new(|_, query| Ok(Outcome::Results(results(query))));
        let searcher = Arc::new(Searcher::with(fake.wire(), quick, None));

        // One of two callers leaves: the search goes on for the other.
        let (stays, leaves) = (search(&searcher, "a", "sqlite wal"), search(&searcher, "b", "sqlite wal"));
        until("both wait", || waiters(&searcher) == 2 && fake.asked(Engine::DuckDuckGo) == 1).await;
        leaves.abort();
        let _ = leaves.await;
        until("one waits", || waiters(&searcher) == 1).await;
        assert_eq!(fake.dropped(), 0);

        // The last leaves too: the request is dropped, and nothing is left behind.
        stays.abort();
        let _ = stays.await;
        until("the request dropped", || fake.dropped() == 1).await;
        assert!(searcher.shared.state().flights.is_empty() && searcher.shared.state().waiting.is_empty());

        // Asked again, it goes out again, and the engine's turn is free.
        fake.let_go();
        let (answers, _) = searcher.search("a", &["sqlite wal".to_owned()], &Options::default()).await;
        assert!(!answers[0].engine.is_empty() && answers[0].unavailable.is_none(), "{answers:?}");
        assert_eq!(fake.total(), 2);
    }

    /// Only DuckDuckGo, 3 an hour; every other engine is out.
    fn only_duckduckgo(engine: Engine) -> Policy {
        Policy { per_hour: if engine == Engine::DuckDuckGo { 3 } else { 0 }, ..quick(engine) }
    }

    #[tokio::test]
    async fn requests_waiting_behind_a_block_or_the_hours_last_dont_go_out() {
        // A CAPTCHA while two more wait their turn: they don't go out.
        let fake = Fake::new(|engine, query| match (engine, query) {
            (Engine::DuckDuckGo, "first") => Ok(Outcome::Blocked(Block::Challenge)),
            _ => Ok(Outcome::Results(results(query))),
        });
        let searcher = Arc::new(Searcher::with(fake.wire(), only_duckduckgo, None));
        let first = search(&searcher, "a", "first");
        until("the first is asked", || fake.asked(Engine::DuckDuckGo) == 1).await;
        let others = [search(&searcher, "b", "second"), search(&searcher, "c", "third")];
        until("two wait their turn", || queued(&searcher) == 3).await;
        fake.let_go();
        let (answers, notices) = first.await.expect("search");
        assert!(answers[0].unavailable.as_deref().is_some_and(|why| why.starts_with("every search engine is busy or blocking")), "{answers:?}");
        assert!(notices[0].starts_with("DuckDuckGo is challenging this computer's IP"), "{notices:?}");
        for other in others {
            let (answers, _) = other.await.expect("search");
            assert!(answers[0].unavailable.as_deref().is_some_and(|why| why.contains("DuckDuckGo asked for a CAPTCHA")), "{answers:?}");
        }
        assert_eq!(fake.total(), 1);

        // Five wait while the hour has room for 3: the rest don't go out.
        let fake = Fake::new(|_, query| Ok(Outcome::Results(results(query))));
        let searcher = Arc::new(Searcher::with(fake.wire(), only_duckduckgo, None));
        let tasks: Vec<_> = (0..5).map(|n| search(&searcher, &format!("agent{n}"), &format!("query {n}"))).collect();
        until("all five wait", || queued(&searcher) == 5).await;
        fake.let_go();
        let mut answered = 0;
        for task in tasks {
            let (answers, _) = task.await.expect("search");
            match &answers[0].unavailable {
                None => answered += 1,
                Some(why) => assert!(why.contains("DuckDuckGo has had its 3 searches this hour"), "{why}"),
            }
        }
        assert_eq!((answered, fake.total()), (3, 3));
    }

    #[tokio::test]
    async fn a_blocked_search_moves_on_to_the_next_engine_and_the_load_is_spread() {
        let fake = Fake::new(|engine, query| match engine {
            Engine::DuckDuckGo => Ok(Outcome::Blocked(Block::Challenge)),
            Engine::Bing => Ok(Outcome::Results(vec![hit("Tokyo travel guide", "https://example.com/tokyo"); 5])),
            _ => Ok(Outcome::Results(results(query))),
        });
        fake.let_go();
        let searcher = Searcher::with(fake.wire(), quick, None);
        let options = Options { limit: 8, ..Options::default() };
        // DuckDuckGo's CAPTCHA, then Bing's results about something else:
        // Yahoo answers, in the same call.
        let (answers, notices) = searcher.search("a", &["tokio select cancellation safety".to_owned()], &options).await;
        assert_eq!((answers[0].engine.as_str(), answers[0].results.len()), ("Yahoo", 8), "{answers:?}");
        assert_eq!(notices.len(), 1);
        // The next search starts with an engine not asked yet this hour.
        let (answers, _) = searcher.search("a", &["sqlite wal checkpoint".to_owned()], &options).await;
        assert_eq!(answers[0].engine, "DuckDuckGo Lite");
        // One soft block doesn't cool Bing down; it's asked again.
        let health = |engine: Engine| searcher.shared.state().health.get(&engine).map(|health| (health.cooling_until > 0, health.failing > 0.0));
        assert_eq!(health(Engine::Bing), Some((false, true)));
        assert_eq!(health(Engine::DuckDuckGo), Some((true, true)));
    }

    #[tokio::test]
    async fn engines_answering_lately_go_first_and_a_cooled_one_is_probed_last() {
        let fake = Fake::new(|_, query| Ok(Outcome::Results(results(query))));
        fake.let_go();
        let searcher = Searcher::with(fake.wire(), quick, None);
        let now = now_ms();
        {
            let mut state = searcher.shared.state();
            // DuckDuckGo failed lately; Bing's cooldown just ended.
            state.health.insert(Engine::DuckDuckGo, Health { failing: 0.5, ..Health::default() });
            state.health.insert(Engine::Bing, Health { cooling_until: now - 1, why: "asked for a CAPTCHA".into(), ..Health::default() });
        }
        let ranked = order(&searcher.shared, &Options::default(), now);
        assert_eq!(ranked.first(), Some(&Engine::Yahoo));
        let expected = [Engine::Yahoo, Engine::DuckDuckGoLite, Engine::Ecosia, Engine::Brave, Engine::Marginalia, Engine::DuckDuckGo, Engine::Bing];
        assert_eq!(ranked, expected, "failing lately goes after the healthy ones");
        assert_eq!(ranked.last(), Some(&Engine::Bing), "probed after every healthy engine");
        // Its probe answers: it's back, its cooldown gone.
        let mut health = Health { cooling_until: now - 1, ..Health::default() };
        health.record(&quick(Engine::Bing), &Ok(Outcome::Results(vec![])), now);
        assert!(!health.probing(now) && health.cooling_until == 0);
        // Asking for this week leaves out the engines that can't.
        let recent = Options { recency: Some(Recency::Week), ..Options::default() };
        assert!(!order(&searcher.shared, &recent, now).contains(&Engine::Ecosia));
    }

    #[tokio::test]
    async fn when_no_engine_can_answer_it_says_so_and_when_to_try_again() {
        let fake = Fake::new(|engine, _| match engine {
            Engine::Bing | Engine::Yahoo => Ok(Outcome::Blocked(Block::RateLimited)),
            _ => Ok(Outcome::Blocked(Block::Challenge)),
        });
        fake.let_go();
        let searcher = Searcher::with(fake.wire(), quick, None);
        let (answers, notices) = searcher.search("a", &["sqlite wal".to_owned()], &Options::default()).await;
        let why = answers[0].unavailable.clone().expect("unavailable");
        assert!(why.starts_with("every search engine is busy or blocking searches from this computer; try again in 60 minutes (after "), "{why}");
        assert!(why.contains("DuckDuckGo asked for a CAPTCHA") && why.contains("Bing rate-limited this computer"), "{why}");
        assert_eq!(fake.total(), engines::ORDER.len(), "each asked once");
        assert_eq!(
            notices,
            ["DuckDuckGo, DuckDuckGo Lite, Ecosia, Brave and Marginalia are challenging this computer's IP; the person's own searches there may show a CAPTCHA for about an hour."],
        );
        // Asked again: nothing goes out.
        searcher.search("a", &["other".to_owned()], &Options::default()).await;
        assert_eq!(fake.total(), engines::ORDER.len());

        // Offline: it says so.
        let fake = Fake::new(|_, _| Err("connection refused".to_owned()));
        fake.let_go();
        let searcher = Searcher::with(fake.wire(), quick, None);
        let (answers, _) = searcher.search("a", &["sqlite wal".to_owned()], &Options::default()).await;
        let why = answers[0].unavailable.clone().expect("unavailable");
        assert!(why.starts_with("no search engine could be reached from this computer; check its internet connection"), "{why}");
    }

    #[tokio::test]
    async fn a_cooldown_outlasts_a_restart() {
        let path = std::env::temp_dir().join(format!("factoryd-search-{}-{}.json", std::process::id(), now_ms()));
        let fake = Fake::new(|engine, query| match engine {
            Engine::DuckDuckGo => Ok(Outcome::Blocked(Block::Challenge)),
            _ => Ok(Outcome::Results(results(query))),
        });
        fake.let_go();
        let searcher = Searcher::with(fake.wire(), quick, Some(path.clone()));
        assert_eq!(searcher.search("a", &["first".to_owned()], &Options::default()).await.0[0].engine, "Bing");
        assert_eq!(fake.asked(Engine::DuckDuckGo), 1);
        drop(searcher);

        let fake = Fake::new(|_, query| Ok(Outcome::Results(results(query))));
        fake.let_go();
        let again = Searcher::with(fake.wire(), quick, Some(path.clone()));
        assert_ne!(again.search("a", &["second".to_owned()], &Options::default()).await.0[0].engine, "DuckDuckGo");
        assert_eq!(fake.asked(Engine::DuckDuckGo), 0, "still cooling down");
        let _ = std::fs::remove_file(path);
    }

    #[tokio::test]
    async fn nothing_found_is_an_answer_not_an_outage() {
        let fake = Fake::new(|_, _| Ok(Outcome::NoResults));
        fake.let_go();
        let searcher = Searcher::with(fake.wire(), quick, None);
        let (answers, _) = searcher.search("a", &["zxqv nothing".to_owned()], &Options::default()).await;
        assert_eq!((answers[0].engine.as_str(), answers[0].results.len(), answers[0].unavailable.is_none()), ("Bing", 0, true));
        assert_eq!((fake.asked(Engine::DuckDuckGo), fake.asked(Engine::Bing), fake.total()), (1, 1, 2), "a second opinion, no more");
        searcher.search("b", &["zxqv nothing".to_owned()], &Options::default()).await;
        assert_eq!(fake.total(), 2, "kept for a minute");
    }


    /// One real search on each engine, by hand when results look wrong:
    /// `cargo test -p factoryd -- --ignored web_live`. Never loop it: a
    /// burst is what gets this computer's IP flagged.
    #[tokio::test]
    #[ignore]
    async fn web_live_search() {
        let mut failed = Vec::new();
        for engine in engines::ORDER {
            let client = client(engine).expect("client");
            let options = Options { limit: 8, ..Options::default() };
            let response = engine.request(&client, "sqlite wal checkpoint", &options).send().await.expect("sent");
            let (status, url) = (response.status(), response.url().to_string());
            let outcome = engine.parse(status, &url, &response.text().await.expect("body"));
            match &outcome {
                Outcome::Results(hits) => eprintln!("{}: {status}, {} results, first {:?}", engine.name(), hits.len(), hits.first()),
                other => eprintln!("{}: {status} {other:?}", engine.name()),
            }
            // A block is read right but means this IP is flagged (Brave holds
            // a flag for most of an hour): try again much later, not now.
            if !matches!(outcome, Outcome::Results(ref hits) if hits.len() >= 5) {
                failed.push(engine.name());
            }
            tokio::time::sleep(Duration::from_secs(5)).await;
        }
        assert!(failed.is_empty(), "{failed:?}; if it's a block, wait an hour before trying again");
    }

    /// The acceptance run, against the real engines from this computer, at
    /// an agent's pace: batches of 1 to 4 searches, 15 to 40 s apart, for
    /// `WEB_RELIABILITY_MINUTES` (80) or until the queries in
    /// tests/fixtures/web/queries.txt run out. Writes each search to
    /// `WEB_RELIABILITY_OUT` (/tmp/web-reliability.csv) and prints the
    /// success rate, which engine answered and how long it took. Starts
    /// with no health kept, as a new Claktory does. `WEB_RELIABILITY_PER_HOUR`
    /// paces it to about that many searches an hour; unset, it asks as fast as
    /// a busy agent would (a few every 15 to 40 seconds).
    #[tokio::test]
    #[ignore]
    async fn web_live_reliability() {
        use std::io::Write;
        let minutes: u64 = std::env::var("WEB_RELIABILITY_MINUTES").ok().and_then(|text| text.parse().ok()).unwrap_or(80);
        let per_hour: Option<u64> = std::env::var("WEB_RELIABILITY_PER_HOUR").ok().and_then(|text| text.parse().ok()).filter(|per_hour| *per_hour > 0);
        let out = std::env::var("WEB_RELIABILITY_OUT").unwrap_or_else(|_| "/tmp/web-reliability.csv".to_owned());
        let health = std::env::temp_dir().join(format!("web-reliability-health-{}.json", now_ms()));
        let searcher = Searcher::new(Some(health.clone()));
        let mut queries: Vec<(String, Options)> = engines::tests::fixture("queries.txt")
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty() && !line.starts_with('#'))
            .map(|line| {
                let mut parts = line.split('|').map(str::trim);
                let query = parts.next().unwrap_or_default().to_owned();
                let domains = parts.next().filter(|domains| !domains.is_empty()).map(|domain| vec![domain.to_owned()]).unwrap_or_default();
                let recency = match parts.next() {
                    Some("day") => Some(Recency::Day),
                    Some("week") => Some(Recency::Week),
                    Some("month") => Some(Recency::Month),
                    Some("year") => Some(Recency::Year),
                    _ => None,
                };
                (query, Options { domains, recency, limit: 8 })
            })
            .collect();
        // Shuffled, so each run asks in another order.
        let mut seed = now_ms() as u64 | 1;
        let mut random = move |below: u64| {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed % below.max(1)
        };
        for index in (1..queries.len()).rev() {
            queries.swap(index, random(index as u64 + 1) as usize);
        }
        let mut csv = std::fs::File::create(&out).expect("output");
        writeln!(csv, "time,query,engine,results,ok,latency_ms,why").expect("write");
        let started = std::time::Instant::now();
        let mut rows: Vec<(String, bool, u128, String, String)> = Vec::new();
        let mut next = queries.into_iter();
        'run: while started.elapsed() < Duration::from_secs(minutes * 60) {
            let burst = 1 + random(4);
            for _ in 0..burst {
                let Some((query, options)) = next.next() else { break 'run };
                let began = std::time::Instant::now();
                let (answers, notices) = searcher.search("measure", std::slice::from_ref(&query), &options).await;
                let latency = began.elapsed().as_millis();
                let answer = &answers[0];
                let ok = answer.unavailable.is_none() && !answer.results.is_empty();
                let why = answer.unavailable.clone().unwrap_or_default();
                let clean = |text: &str| text.replace(['"', '\n'], " ");
                writeln!(csv, "{},\"{}\",{},{},{ok},{latency},\"{}\"", clock(now_ms()), clean(&query), answer.engine, answer.results.len(), clean(&why))
                    .expect("write");
                csv.flush().expect("flush");
                eprintln!(
                    "{} {:>5}ms {:<16} {:>2} {query}{}{}",
                    if ok { "ok  " } else { "FAIL" },
                    latency,
                    answer.engine,
                    answer.results.len(),
                    if why.is_empty() { String::new() } else { format!(" — {why}") },
                    notices.first().map(|notice| format!(" [{notice}]")).unwrap_or_default()
                );
                rows.push((query, ok, latency, answer.engine.clone(), why));
            }
            let pause = match per_hour {
                // Half to one and a half times the even spacing, so the pace
                // averages out but never ticks like a clock.
                Some(per_hour) => {
                    let even = burst * 3600 / per_hour;
                    even / 2 + random(even + 1)
                }
                None => 15 + random(26),
            };
            tokio::time::sleep(Duration::from_secs(pause)).await;
        }
        let total = rows.len();
        let ok = rows.iter().filter(|row| row.1).count();
        let mut latencies: Vec<u128> = rows.iter().filter(|row| row.1).map(|row| row.2).collect();
        latencies.sort_unstable();
        let at = |fraction: f64| latencies.get(((latencies.len() as f64 - 1.0) * fraction).round() as usize).copied().unwrap_or(0);
        let mut by_engine: HashMap<String, usize> = HashMap::new();
        for row in rows.iter().filter(|row| row.1) {
            *by_engine.entry(row.3.clone()).or_default() += 1;
        }
        let mut by_engine: Vec<_> = by_engine.into_iter().collect();
        by_engine.sort_by_key(|(_, count)| std::cmp::Reverse(*count));
        let summary = format!(
            "{ok} of {total} searches answered ({:.1}%) in {:.0} minutes; latency p50 {} ms, p90 {} ms, max {} ms; answered by {by_engine:?}; failed: {:?}; health at the end: {}",
            100.0 * ok as f64 / total.max(1) as f64,
            started.elapsed().as_secs_f64() / 60.0,
            at(0.5),
            at(0.9),
            latencies.last().copied().unwrap_or(0),
            rows.iter().filter(|row| !row.1).map(|row| (&row.0, &row.4)).collect::<Vec<_>>(),
            std::fs::read_to_string(&health).unwrap_or_default(),
        );
        eprintln!("{summary}");
        std::fs::write(format!("{out}.summary.txt"), &summary).expect("summary");
        let _ = std::fs::remove_file(health);
    }

    /// What a fingerprinting service sees of each client, after every primp
    /// update: `cargo test -p factoryd -- --ignored web_live`.
    #[tokio::test]
    #[ignore]
    async fn web_live_fingerprints() {
        let os = if cfg!(target_os = "macos") { "Macintosh" } else { "Linux" };
        for engine in engines::ORDER {
            let firefox = engine.profile().browser == primp::Impersonate::FirefoxV151;
            let (ja4, akamai) = if firefox {
                ("t13d1717h2_5b57614c22b0", "1:65536;2:0;4:131072;5:16384|12517377|0|m,p,a,s")
            } else {
                ("_8daaf6152771_", "1:65536;2:0;4:6291456;6:262144|15663105|0|m,a,s,p")
            };
            let client = client(engine).expect("client");
            let request = match engine {
                Engine::DuckDuckGo => client
                    .post("https://tls.peet.ws/api/all")
                    .header("Origin", "https://html.duckduckgo.com")
                    .header("Referer", "https://html.duckduckgo.com/")
                    .header("Sec-Fetch-Site", "same-origin")
                    .form(&[("q", "x")]),
                _ => client.get("https://tls.peet.ws/api/all"),
            };
            let seen: serde_json::Value = request.send().await.expect("sent").json().await.expect("json");
            let got = seen["tls"]["ja4"].as_str().unwrap_or_default();
            assert!(got.contains(ja4) || got.starts_with(ja4), "{}: {got}", engine.name());
            assert_eq!(seen["http2"]["akamai_fingerprint"].as_str(), Some(akamai), "{}", engine.name());
            assert!(seen["user_agent"].as_str().unwrap_or_default().contains(os), "{}: {}", engine.name(), seen["user_agent"]);
            // No cookie or referrer of one engine's goes to another.
            let headers = seen["http2"]["sent_frames"].to_string().to_lowercase();
            if engine == Engine::DuckDuckGo {
                for header in
                    ["sec-fetch-site: same-origin", "origin: https://html.duckduckgo.com", "referer: https://html.duckduckgo.com/"]
                {
                    assert!(headers.contains(header), "{header}");
                }
            } else {
                assert!(!headers.contains("cookie:") && !headers.contains("referer:"), "{}: {headers}", engine.name());
            }
        }
    }
}
