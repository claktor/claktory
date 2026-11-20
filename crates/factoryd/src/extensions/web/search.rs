//! web_search: each query goes to the first engine that's healthy and
//! answers, paced and budgeted for the whole Factory, since every agent
//! shares one IP. A block is never retried: the engine cools down and the
//! query falls through to the next one.

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use serde::Serialize;

use super::engines::{self, Engine};
use crate::transcript::now_ms;

const MINUTE: i64 = 60_000;
const HOUR: i64 = 60 * MINUTE;
/// Searches are kept this long…
const CACHE_MS: i64 = HOUR;
/// …this many of them.
const CACHE_SIZE: usize = 500;
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

/// How an engine is doing.
#[derive(Debug, Default)]
pub(crate) struct Health {
    /// Left alone until then (ms), and why.
    pub(crate) cooling_until: i64,
    pub(crate) why: String,
    /// Blocks in a row: each doubles the next cooldown; a success resets it.
    pub(crate) strikes: u32,
    /// When it was asked, over the last hour.
    pub(crate) asked: VecDeque<i64>,
}

impl Health {
    /// Why it can't be asked now (with when it can), or None.
    pub(crate) fn unavailable(&mut self, engine: Engine, now: i64) -> Option<String> {
        if now < self.cooling_until {
            return Some(format!("{} {} (trying again after {})", engine.name(), self.why, clock(self.cooling_until)));
        }
        while self.asked.front().is_some_and(|time| *time <= now - HOUR) {
            self.asked.pop_front();
        }
        if self.asked.len() >= engine.policy().per_hour {
            let free = self.asked.front().map_or(now, |time| time + HOUR);
            return Some(format!("{} has had its {} searches this hour (after {})", engine.name(), self.asked.len(), clock(free)));
        }
        None
    }

    /// Records what an answer said, and how long to leave the engine alone.
    pub(crate) fn record(&mut self, engine: Engine, outcome: &Result<Outcome, String>, now: i64) {
        let policy = engine.policy();
        let doubled = |max: i64| (HOUR << self.strikes.min(16)).min(max);
        let (cooldown, why) = match outcome {
            Ok(Outcome::Results(_) | Outcome::NoResults) => {
                self.strikes = 0;
                return;
            }
            Ok(Outcome::Blocked(Block::Challenge)) => (doubled(policy.challenge_max_ms), "asked for a CAPTCHA"),
            Ok(Outcome::Blocked(Block::RateLimited)) => (doubled(policy.limited_max_ms), "rate-limited this computer"),
            Ok(Outcome::Blocked(Block::Denied)) => (HOUR, "denied access"),
            Ok(Outcome::Blocked(Block::Degraded)) => (HOUR, "returned results that don't match (a soft block)"),
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

/// An engine, its client and its pace.
struct Slot {
    engine: Engine,
    client: OnceLock<Result<primp::Client, String>>,
    /// When it was last asked; held while waiting for its turn, so requests
    /// go one at a time.
    last: tokio::sync::Mutex<i64>,
}

/// Search for the whole Factory.
pub(crate) struct Searcher {
    slots: Vec<Arc<Slot>>,
    state: Mutex<SearchState>,
}

#[derive(Default)]
struct SearchState {
    health: HashMap<Engine, Health>,
    cache: HashMap<(String, Options), (i64, Answer)>,
}

impl Default for Searcher {
    fn default() -> Self {
        let slots = engines::ORDER
            .iter()
            .map(|engine| Arc::new(Slot { engine: *engine, client: OnceLock::new(), last: tokio::sync::Mutex::new(0) }))
            .collect();
        Searcher { slots, state: Mutex::default() }
    }
}

impl Searcher {
    fn state(&self) -> std::sync::MutexGuard<'_, SearchState> {
        self.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Answers each query in turn. Also says what blocked, once, for the
    /// person: a block hits their own browser's searches too.
    pub(crate) async fn search(&self, queries: &[String], options: &Options) -> (Vec<Answer>, Vec<String>) {
        let mut answers = Vec::new();
        let mut blocked = Vec::new();
        for query in queries {
            answers.push(self.one(query, options, &mut blocked).await);
        }
        (answers, blocked)
    }

    async fn one(&self, query: &str, options: &Options, blocked: &mut Vec<String>) -> Answer {
        let key = (normalized(query), options.clone());
        let now = now_ms();
        if let Some((time, answer)) = self.state().cache.get(&key)
            && now - time < CACHE_MS
        {
            return answer.clone();
        }
        let mut reasons = Vec::new();
        for slot in &self.slots {
            let engine = slot.engine;
            if let Some(why) = self.state().health.entry(engine).or_default().unavailable(engine, now_ms()) {
                reasons.push(why);
                continue;
            }
            let outcome = self.ask(slot, query, options).await;
            let outcome = match outcome {
                Ok(Outcome::Results(hits)) => match relevant(query, &hits) {
                    true => Ok(Outcome::Results(keep(hits, options))),
                    false => Ok(Outcome::Blocked(Block::Degraded)),
                },
                other => other,
            };
            self.state().health.entry(engine).or_default().record(engine, &outcome, now_ms());
            match outcome {
                Ok(Outcome::Results(hits)) if !hits.is_empty() => {
                    let answer = Answer { q: query.to_owned(), engine: engine.name().to_owned(), results: hits, unavailable: None };
                    let mut state = self.state();
                    if state.cache.len() >= CACHE_SIZE
                        && let Some(oldest) = state.cache.iter().min_by_key(|(_, (time, _))| *time).map(|(key, _)| key.clone())
                    {
                        state.cache.remove(&oldest);
                    }
                    state.cache.insert(key, (now_ms(), answer.clone()));
                    return answer;
                }
                Ok(Outcome::Results(_) | Outcome::NoResults) => reasons.push(format!("{} found nothing", engine.name())),
                Ok(Outcome::Blocked(block)) => {
                    tracing::warn!(engine = engine.name(), ?block, "a search engine blocked this computer");
                    if block == Block::Challenge {
                        blocked.push(format!(
                            "{} is challenging this computer's IP; the person's own searches there may show a CAPTCHA for about an hour.",
                            engine.name()
                        ));
                    }
                    let health = &self.state().health[&engine];
                    reasons.push(format!("{} {} (trying again after {})", engine.name(), health.why, clock(health.cooling_until)));
                }
                Ok(Outcome::Broken(start)) => {
                    tracing::warn!(engine = engine.name(), start, "a search engine's page couldn't be read; its layout may have changed");
                    reasons.push(format!("{} returned a page Claktory can't read", engine.name()));
                }
                Err(error) => {
                    tracing::warn!(engine = engine.name(), %error, "a search engine couldn't be reached");
                    reasons.push(format!("{} couldn't be reached ({error})", engine.name()));
                }
            }
        }
        Answer { q: query.to_owned(), engine: String::new(), results: Vec::new(), unavailable: Some(reasons.join(", ")) }
    }

    /// Asks one engine, in its turn.
    async fn ask(&self, slot: &Slot, query: &str, options: &Options) -> Result<Outcome, String> {
        let engine = slot.engine;
        let client = slot.client.get_or_init(|| client(engine)).clone()?;
        let mut last = slot.last.lock().await;
        let policy = engine.policy();
        let wait = *last + policy.gap_ms + jitter(policy.jitter_ms) - now_ms();
        if wait > 0 {
            tokio::time::sleep(Duration::from_millis(wait as u64)).await;
        }
        self.state().health.entry(engine).or_default().asked.push_back(now_ms());
        let sent = engine.request(&client, query, options).send().await;
        *last = now_ms();
        let response = sent.map_err(|error| error.to_string())?;
        let (status, url) = (response.status(), response.url().to_string());
        let body = response.text().await.map_err(|error| error.to_string())?;
        Ok(engine.parse(status, &url, &body))
    }
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

fn normalized(query: &str) -> String {
    query.split_whitespace().collect::<Vec<_>>().join(" ").to_lowercase()
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

/// The results within the domains asked for, up to the limit.
fn keep(hits: Vec<Hit>, options: &Options) -> Vec<Hit> {
    let host = |url: &str| primp::Url::parse(url).ok().and_then(|url| url.host_str().map(str::to_lowercase)).unwrap_or_default();
    let within = |host: &str, domain: &str| {
        let domain = domain.trim().to_lowercase();
        host == domain || host.ends_with(&format!(".{domain}"))
    };
    let keep: Vec<&String> = options.domains.iter().filter(|domain| !domain.starts_with('-')).collect();
    let drop: Vec<&str> = options.domains.iter().filter_map(|domain| domain.strip_prefix('-')).collect();
    hits.into_iter()
        .filter(|hit| {
            let host = host(&hit.url);
            (keep.is_empty() || keep.iter().any(|domain| within(&host, domain))) && !drop.iter().any(|domain| within(&host, domain))
        })
        .take(options.limit.max(1))
        .collect()
}

/// The answers as the agent reads them: each query, the engine that
/// answered, and its results, each address on its own line.
pub(crate) fn render(answers: &[Answer]) -> String {
    let mut out = String::new();
    for (index, answer) in answers.iter().enumerate() {
        if index > 0 {
            out.push('\n');
        }
        let number = index + 1;
        match &answer.unavailable {
            Some(why) => out.push_str(&format!("Q{number} \"{}\": no results: {why}.\n", answer.q)),
            None => {
                out.push_str(&format!("Q{number} \"{}\" ({}):\n", answer.q, answer.engine));
                for (rank, hit) in answer.results.iter().enumerate() {
                    out.push_str(&format!("{}. {}\n   {}\n", rank + 1, hit.title, hit.url));
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
        let now = 1_000_000_000_000;
        assert_eq!(health.unavailable(engine, now), None);
        health.record(engine, &Ok(Outcome::Blocked(Block::Challenge)), now);
        assert_eq!(health.cooling_until, now + HOUR);
        let why = health.unavailable(engine, now + 1).expect("cooling");
        assert!(why.starts_with("DuckDuckGo asked for a CAPTCHA (trying again after "), "{why}");
        health.record(engine, &Ok(Outcome::Blocked(Block::Challenge)), now + HOUR);
        assert_eq!(health.cooling_until, now + HOUR + 2 * HOUR, "doubled");
        health.record(engine, &Ok(Outcome::Results(vec![])), now + 3 * HOUR);
        assert_eq!(health.strikes, 0);
        health.record(engine, &Ok(Outcome::Blocked(Block::Challenge)), now + 3 * HOUR);
        assert_eq!(health.cooling_until, now + 4 * HOUR, "back to an hour");
        // Doubling stops at its cap; a network error is a minute.
        let mut brave = Health { strikes: 10, ..Health::default() };
        brave.record(Engine::Brave, &Ok(Outcome::Blocked(Block::RateLimited)), now);
        assert_eq!(brave.cooling_until, now + 6 * HOUR);
        brave.record(Engine::Brave, &Err("timed out".into()), now);
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
        assert!(health.unavailable(Engine::Brave, later).expect("over").contains("has had its 15 searches this hour"));
        assert_eq!(health.unavailable(Engine::Brave, now + HOUR + 1), None, "the oldest left the hour");
        assert_eq!(health.unavailable(Engine::DuckDuckGo, later), None, "DuckDuckGo allows 60");
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
    fn domains_are_kept_or_dropped_and_the_limit_holds() {
        let hits = vec![hit("a", "https://www.sqlite.org/wal.html"), hit("b", "https://w3schools.com/x"), hit("c", "https://sqlite.org/c")];
        let options = Options { domains: vec!["sqlite.org".into()], limit: 8, recency: None };
        assert_eq!(keep(hits.clone(), &options).len(), 2);
        let options = Options { domains: vec!["-w3schools.com".into()], limit: 1, recency: None };
        assert_eq!(keep(hits, &options), vec![hit("a", "https://www.sqlite.org/wal.html")]);
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
        assert_eq!(lines[1..3], ["1. WAL", "   https://sqlite.org/wal.html"]);
        assert_eq!(lines[3].chars().count(), 3 + 200 + 1, "trimmed with …");
        assert_eq!(lines[5], "Q2 \"other\": no results: Brave found nothing.");
    }

    /// One real search on each engine, by hand when results look wrong:
    /// `cargo test -p factoryd -- --ignored web_live`. Never loop it: a
    /// burst is what gets this computer's IP flagged.
    #[tokio::test]
    #[ignore]
    async fn web_live_search() {
        for engine in engines::ORDER {
            let client = client(engine).expect("client");
            let options = Options { limit: 8, ..Options::default() };
            let response = engine.request(&client, "sqlite wal checkpoint", &options).send().await.expect("sent");
            let (status, url) = (response.status(), response.url().to_string());
            let outcome = engine.parse(status, &url, &response.text().await.expect("body"));
            eprintln!("{}: {status} {outcome:?}", engine.name());
            // A block is read right but means this IP is flagged (Brave holds
            // a flag for most of an hour): try again much later, not now.
            assert!(
                matches!(outcome, Outcome::Results(ref hits) if hits.len() >= 5),
                "{}: {outcome:?}; if it's a block, wait an hour before trying again",
                engine.name()
            );
            tokio::time::sleep(Duration::from_secs(5)).await;
        }
    }

    /// What a fingerprinting service sees of each client, after every primp
    /// update: `cargo test -p factoryd -- --ignored web_live`.
    #[tokio::test]
    #[ignore]
    async fn web_live_fingerprints() {
        let expected = |engine: Engine| match engine {
            Engine::DuckDuckGo => ("t13d1717h2_5b57614c22b0", "1:65536;2:0;4:131072;5:16384|12517377|0|m,p,a,s"),
            Engine::Brave => ("_8daaf6152771_", "1:65536;2:0;4:6291456;6:262144|15663105|0|m,a,s,p"),
        };
        let os = if cfg!(target_os = "macos") { "Macintosh" } else { "Linux" };
        for engine in engines::ORDER {
            let client = client(engine).expect("client");
            let request = match engine {
                Engine::DuckDuckGo => client
                    .post("https://tls.peet.ws/api/all")
                    .header("Origin", "https://html.duckduckgo.com")
                    .header("Referer", "https://html.duckduckgo.com/")
                    .header("Sec-Fetch-Site", "same-origin")
                    .form(&[("q", "x")]),
                Engine::Brave => client.get("https://tls.peet.ws/api/all"),
            };
            let seen: serde_json::Value = request.send().await.expect("sent").json().await.expect("json");
            let (ja4, akamai) = expected(engine);
            let got = seen["tls"]["ja4"].as_str().unwrap_or_default();
            assert!(got.contains(ja4) || got.starts_with(ja4), "{}: {got}", engine.name());
            assert_eq!(seen["http2"]["akamai_fingerprint"].as_str(), Some(akamai), "{}", engine.name());
            assert!(seen["user_agent"].as_str().unwrap_or_default().contains(os), "{}: {}", engine.name(), seen["user_agent"]);
            if engine == Engine::DuckDuckGo {
                let headers = seen["http2"]["sent_frames"].to_string().to_lowercase();
                for header in
                    ["sec-fetch-site: same-origin", "origin: https://html.duckduckgo.com", "referer: https://html.duckduckgo.com/"]
                {
                    assert!(headers.contains(header), "{header}");
                }
            }
        }
    }
}
