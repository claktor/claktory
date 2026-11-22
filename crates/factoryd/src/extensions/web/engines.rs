//! The search engines Claktory queries the way a browser does: how each
//! one's request looks, which browser it's made as, and how its page is
//! read, blocks included. Each engine's page reader is in `engines/`.
//!
//! Only free sources that answer a plain page request, with no key and no
//! JavaScript, are here. Tried from a Mac in November 2026 and left out:
//! Google (its results need JavaScript), Startpage (a JavaScript proof of
//! work), Qwant (its API refuses outside requests), Yandex (a CAPTCHA at
//! once), Mojeek (a JavaScript check before any results), Dogpile (results
//! load by script), AOL and Ask (no results page).

mod bing;
mod brave;
mod duckduckgo;
mod ecosia;
mod marginalia;
mod yahoo;

use primp::{Impersonate, ImpersonateOS, StatusCode};
use scraper::{ElementRef, Html, Selector};

use super::search::{Block, Hit, Options, Outcome, Recency};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum Engine {
    DuckDuckGo,
    DuckDuckGoLite,
    Bing,
    Yahoo,
    Ecosia,
    Brave,
    Marginalia,
}

/// Every engine, in the order they're preferred when all are equally
/// healthy: the largest indexes first, Marginalia's small one last.
pub(crate) const ORDER: [Engine; 7] = [
    Engine::DuckDuckGo,
    Engine::Bing,
    Engine::Yahoo,
    Engine::DuckDuckGoLite,
    Engine::Ecosia,
    Engine::Brave,
    Engine::Marginalia,
];

/// How often an engine may be asked, and how long it's left alone after a
/// block. Every agent shares one IP, so this is for the whole Claktory.
pub(crate) struct Policy {
    /// The least time between two requests, in ms…
    pub(crate) gap_ms: i64,
    /// …plus up to this much at random.
    pub(crate) jitter_ms: i64,
    /// Requests in any hour.
    pub(crate) per_hour: usize,
    /// The longest a challenge's cooldown grows to, doubling from an hour.
    pub(crate) challenge_max_ms: i64,
    /// The longest a rate limit's cooldown grows to, doubling from an hour.
    pub(crate) limited_max_ms: i64,
}

const HOUR: i64 = 3_600_000;

/// The browser a client is made as: every layer (TLS, HTTP/2, headers, the
/// UA and its client hints) from the same one, pinned, never random.
pub(crate) struct Profile {
    pub(crate) browser: Impersonate,
    pub(crate) os: ImpersonateOS,
}

/// The OS the browser claims to run on: this computer's. A server's
/// network fingerprint would contradict any other.
pub(crate) fn host_os() -> ImpersonateOS {
    if cfg!(target_os = "macos") { ImpersonateOS::MacOS } else { ImpersonateOS::Linux }
}

/// Pages are fetched as Chrome, the most common browser.
pub(crate) fn fetch_profile() -> Profile {
    Profile { browser: Impersonate::ChromeV153, os: host_os() }
}

impl Engine {
    pub(crate) fn name(self) -> &'static str {
        match self {
            Engine::DuckDuckGo => "DuckDuckGo",
            Engine::DuckDuckGoLite => "DuckDuckGo Lite",
            Engine::Bing => "Bing",
            Engine::Yahoo => "Yahoo",
            Engine::Ecosia => "Ecosia",
            Engine::Brave => "Brave",
            Engine::Marginalia => "Marginalia",
        }
    }

    /// Each engine's own browser, pinned: DuckDuckGo challenges Chrome's
    /// handshake, so it and the small independents get Firefox; Bing gets
    /// Edge, Microsoft's own; Brave, Yahoo and Ecosia get Chrome.
    pub(crate) fn profile(self) -> Profile {
        let browser = match self {
            Engine::DuckDuckGo | Engine::DuckDuckGoLite | Engine::Marginalia => Impersonate::FirefoxV151,
            Engine::Bing => Impersonate::EdgeV153,
            Engine::Yahoo | Engine::Ecosia | Engine::Brave => Impersonate::ChromeV153,
        };
        Profile { browser, os: host_os() }
    }

    pub(crate) fn policy(self) -> Policy {
        let (gap_ms, jitter_ms, per_hour, limited_max_ms) = match self {
            Engine::DuckDuckGo | Engine::DuckDuckGoLite => (3_000, 2_000, 60, HOUR),
            Engine::Bing | Engine::Yahoo => (4_000, 3_000, 60, 6 * HOUR),
            Engine::Ecosia => (5_000, 3_000, 40, 6 * HOUR),
            Engine::Brave => (20_000, 5_000, 15, 6 * HOUR),
            // A one-person project: asked least.
            Engine::Marginalia => (10_000, 5_000, 20, 6 * HOUR),
        };
        Policy { gap_ms, jitter_ms, per_hour, challenge_max_ms: 24 * HOUR, limited_max_ms }
    }

    /// Whether it can answer `options` as asked: an engine with no date
    /// filter isn't asked for recent results.
    pub(crate) fn supports(self, options: &Options) -> bool {
        options.recency.is_none() || !matches!(self, Engine::Ecosia | Engine::Marginalia)
    }

    /// Cookies a browser that had used the site would carry, by URL. Each
    /// engine's client has its own jar: nothing is shared between engines.
    pub(crate) fn cookies(self) -> (&'static str, &'static [&'static str]) {
        match self {
            Engine::DuckDuckGo => ("https://html.duckduckgo.com/", &["kl=wt-wt"]),
            Engine::DuckDuckGoLite => ("https://lite.duckduckgo.com/", &["kl=wt-wt"]),
            Engine::Brave => {
                ("https://search.brave.com/", &["country=all", "useLocation=0", "summarizer=0", "safesearch=moderate", "ui_lang=en-us"])
            }
            _ => ("", &[]),
        }
    }

    /// The request for `query`, as the browser would send it.
    pub(crate) fn request(self, client: &primp::Client, query: &str, options: &Options) -> primp::RequestBuilder {
        let query = with_domains(query, &options.domains);
        match self {
            Engine::DuckDuckGo | Engine::DuckDuckGoLite => duckduckgo::request(self == Engine::DuckDuckGoLite, client, query, options),
            Engine::Bing => bing::request(client, query, options),
            Engine::Yahoo => yahoo::request(client, query, options),
            Engine::Ecosia => client.get("https://www.ecosia.org/search").query(&[("method", "index"), ("q", query.as_str())]),
            Engine::Brave => brave::request(client, query, options),
            Engine::Marginalia => client.get("https://marginalia-search.com/search").query(&[("query", query.as_str())]),
        }
    }

    /// What the engine's answer says.
    pub(crate) fn parse(self, status: StatusCode, final_url: &str, body: &str) -> Outcome {
        if final_url.contains("challenges.cloudflare.com") || final_url.contains("/sorry/") || final_url.contains("showcaptcha") {
            return Outcome::Blocked(Block::Challenge);
        }
        match status.as_u16() {
            // Yahoo's "999 Unable to process request": too many from here.
            429 | 999 => return Outcome::Blocked(Block::RateLimited),
            402 | 403 => {
                return Outcome::Blocked(if challenge_page(&body.to_lowercase()) { Block::Challenge } else { Block::Denied });
            }
            // DuckDuckGo answers a suspected bot with 202 and a challenge.
            202 if matches!(self, Engine::DuckDuckGo | Engine::DuckDuckGoLite) => return Outcome::Blocked(Block::Challenge),
            _ => {}
        }
        if !status.is_success() {
            return Outcome::Broken(format!("HTTP {status}"));
        }
        let page = Html::parse_document(body);
        if self == Engine::Marginalia && marginalia::wait(&page).is_some() {
            return Outcome::Blocked(Block::Challenge);
        }
        let hits: Vec<Hit> = match self {
            Engine::DuckDuckGo => duckduckgo::html(&page),
            Engine::DuckDuckGoLite => duckduckgo::lite(&page),
            Engine::Bing => bing::hits(&page),
            Engine::Yahoo => yahoo::hits(&page),
            Engine::Ecosia => ecosia::hits(&page),
            Engine::Brave => brave::hits(&page),
            Engine::Marginalia => marginalia::hits(&page),
        }
        .into_iter()
        .filter(|hit| hit.url.starts_with("http") && !hit.title.is_empty())
        .collect();
        if !hits.is_empty() {
            return Outcome::Results(hits);
        }
        let lower = body.to_lowercase();
        let challenged = challenge_page(&lower)
            || match self {
                Engine::DuckDuckGo | Engine::DuckDuckGoLite => duckduckgo::challenged(&lower),
                Engine::Brave => lower.contains("captcha") || title(&page).trim() == "Brave Search",
                _ => false,
            };
        if challenged {
            return Outcome::Blocked(Block::Challenge);
        }
        let none = match self {
            Engine::DuckDuckGo | Engine::DuckDuckGoLite => duckduckgo::none(&page, &lower),
            Engine::Bing => lower.contains("class=\"b_no\"") || lower.contains("there are no results for"),
            Engine::Yahoo => lower.contains("we did not find results for"),
            Engine::Ecosia => lower.contains("no results found"),
            Engine::Brave => lower.contains("no results found") || lower.contains("not many great matches"),
            Engine::Marginalia => lower.contains("no search results found") || lower.contains("no results"),
        };
        if none { Outcome::NoResults } else { Outcome::Broken(body.chars().take(300).collect()) }
    }
}

/// For an engine that asks a browser to wait before it goes on: how long,
/// and where to go then. Only Marginalia does.
pub(crate) fn follow(engine: Engine, body: &str) -> Option<(std::time::Duration, String)> {
    if engine != Engine::Marginalia || !body.contains("countdown") {
        return None;
    }
    let (seconds, url) = marginalia::wait(&Html::parse_document(body))?;
    // Never longer than a few seconds: past that, it's a block.
    (seconds <= 5).then(|| (std::time::Duration::from_millis(seconds * 1000 + 300), url))
}

/// A bot check any site may put up: Cloudflare's, Anubis's proof of work,
/// a CAPTCHA, or a page that needs JavaScript to go on.
fn challenge_page(lower: &str) -> bool {
    [
        "cf-chl-",
        "challenge-platform",
        "<title>just a moment",
        "anubis",
        "verifying your request",
        "captcha",
        "unusual traffic",
        "javascript is required to complete this challenge",
        "please enable javascript",
        "are you a robot",
    ]
    .iter()
    .any(|sign| lower.contains(sign))
}

impl Recency {
    fn letter(self) -> &'static str {
        match self {
            Recency::Day => "d",
            Recency::Week => "w",
            Recency::Month => "m",
            Recency::Year => "y",
        }
    }
}

/// The query with its domain filters as operators: one domain to keep as
/// `site:`, and each to leave out as `-site:`. Several to keep are filtered
/// afterwards (see `search::keep`).
fn with_domains(query: &str, domains: &[String]) -> String {
    let mut query = query.trim().to_owned();
    let keep: Vec<&String> = domains.iter().filter(|domain| !domain.starts_with('-')).collect();
    if let [only] = keep.as_slice() {
        query.push_str(&format!(" site:{only}"));
    }
    for domain in domains.iter().filter_map(|domain| domain.strip_prefix('-')) {
        query.push_str(&format!(" -site:{domain}"));
    }
    query
}

/// At most `max` characters, cut at a word.
fn cut(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_owned();
    }
    let head: String = text.chars().take(max).collect();
    match head.rfind(' ') {
        Some(space) if space > 0 => head[..space].to_owned(),
        _ => head,
    }
}

fn selector(css: &str) -> Selector {
    Selector::parse(css).unwrap_or_else(|error| panic!("bad selector {css}: {error}"))
}

/// An element's text, whitespace collapsed.
fn text(element: ElementRef) -> String {
    element.text().collect::<Vec<_>>().join(" ").split_whitespace().collect::<Vec<_>>().join(" ")
}

fn title(page: &Html) -> String {
    page.select(&selector("title")).next().map(text).unwrap_or_default()
}

/// The text of the first `css` inside `element`; empty when there's none.
fn first_text(element: ElementRef, css: &Selector) -> String {
    element.select(css).next().map(text).unwrap_or_default()
}

fn query_param(href: &str, key: &str) -> Option<String> {
    let full = if href.starts_with("//") { format!("https:{href}") } else { href.to_owned() };
    primp::Url::parse(&full).ok()?.query_pairs().find(|(name, _)| name == key).map(|(_, value)| value.into_owned())
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub(crate) fn fixture(name: &str) -> String {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/web").join(name);
        std::fs::read_to_string(&path).unwrap_or_else(|error| panic!("{}: {error}", path.display()))
    }

    pub(crate) fn hits(outcome: Outcome) -> Vec<Hit> {
        match outcome {
            Outcome::Results(hits) => hits,
            other => panic!("not results: {other:?}"),
        }
    }

    /// Results read from `page` as `engine` serves it: at least `least`,
    /// each with a real address and a title, none of them the engine's own.
    pub(crate) fn read(engine: Engine, page: &str, least: usize) -> Vec<Hit> {
        let found = hits(engine.parse(StatusCode::OK, "https://example.com/search", &fixture(page)));
        assert!(found.len() >= least, "{}: {} results: {found:#?}", engine.name(), found.len());
        for hit in &found {
            assert!(hit.url.starts_with("https://") || hit.url.starts_with("http://"), "{hit:?}");
            assert!(!hit.title.is_empty(), "{hit:?}");
            for own in ["duckduckgo.com/l/", "bing.com/ck/", "search.yahoo.com/", "ecosia.org/", "search.brave.com"] {
                assert!(!hit.url.contains(own), "{}: a link through the engine: {hit:?}", engine.name());
            }
        }
        found
    }

    #[test]
    fn an_empty_page_is_broken_and_a_challenge_host_is_a_block() {
        for engine in ORDER {
            assert!(matches!(engine.parse(StatusCode::OK, "https://example.com/search", ""), Outcome::Broken(_)), "{}", engine.name());
            let outcome = engine.parse(StatusCode::OK, "https://challenges.cloudflare.com/x", "<html></html>");
            assert_eq!(outcome, Outcome::Blocked(Block::Challenge), "{}", engine.name());
            assert_eq!(engine.parse(StatusCode::FORBIDDEN, "https://example.com/", ""), Outcome::Blocked(Block::Denied));
            assert_eq!(engine.parse(StatusCode::TOO_MANY_REQUESTS, "https://example.com/", ""), Outcome::Blocked(Block::RateLimited));
            assert!(matches!(engine.parse(StatusCode::INTERNAL_SERVER_ERROR, "https://example.com/", ""), Outcome::Broken(_)));
        }
    }

    #[test]
    fn bot_checks_of_any_kind_are_blocks() {
        for (page, status) in [
            ("startpage-anubis.html", StatusCode::OK),
            ("mojeek-captcha.html", StatusCode::OK),
            ("google-javascript.html", StatusCode::OK),
        ] {
            for engine in ORDER {
                let outcome = engine.parse(status, "https://example.com/search", &fixture(page));
                assert!(
                    matches!(outcome, Outcome::Blocked(Block::Challenge)) || (page == "google-javascript.html" && matches!(outcome, Outcome::Broken(_))),
                    "{} on {page}: {outcome:?}",
                    engine.name()
                );
            }
        }
        // A 403 behind Cloudflare is its challenge, not a plain refusal.
        let cloudflare = "<html><title>Just a moment...</title><script src=\"/cdn-cgi/challenge-platform/x\"></script></html>";
        assert_eq!(Engine::Ecosia.parse(StatusCode::FORBIDDEN, "https://www.ecosia.org/search", cloudflare), Outcome::Blocked(Block::Challenge));
    }

    #[test]
    fn domains_become_operators_and_long_queries_are_cut_at_a_word() {
        let domains = vec!["sqlite.org".to_owned(), "-w3schools.com".to_owned()];
        assert_eq!(with_domains("wal mode", &domains), "wal mode site:sqlite.org -site:w3schools.com");
        let both = vec!["a.org".to_owned(), "b.org".to_owned()];
        assert_eq!(with_domains("x", &both), "x", "several are filtered afterwards");
        let long = "word ".repeat(200);
        let short = cut(&long, 499);
        assert!(short.len() <= 499 && short.ends_with("word"), "{}", short.len());
    }

    #[test]
    fn every_client_claims_this_computer_and_a_pinned_browser() {
        let os = if cfg!(target_os = "macos") { ImpersonateOS::MacOS } else { ImpersonateOS::Linux };
        for profile in ORDER.map(Engine::profile).into_iter().chain([fetch_profile()]) {
            assert_eq!(profile.os, os, "never Random: primp picks a random OS when none is set");
            assert!(!matches!(
                profile.browser,
                Impersonate::Random | Impersonate::Chrome | Impersonate::Firefox | Impersonate::Edge | Impersonate::Safari
            ));
        }
        assert_eq!(Engine::DuckDuckGo.profile().browser, Impersonate::FirefoxV151, "DuckDuckGo challenges Chrome");
    }

    /// The pages here are real ones, served to this computer, and go public
    /// with the source: none may say where it is or who it is. Pages saved
    /// later get the same scrub: this Mac's address as 203.0.113.1, places
    /// as US placeholders, session ids as zeros of the same length.
    #[test]
    fn saved_pages_say_nothing_of_this_computer() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/web");
        let quad = regex::Regex::new(r"(^|[^\w./-])(\d{1,3}(?:\.\d{1,3}){3})($|[^\w.])").expect("regex");
        let allowed = |ip: std::net::Ipv4Addr| {
            let [first, second, third, _] = ip.octets();
            first == 127 || first == 0 || matches!((first, second, third), (192, 0, 2) | (198, 51, 100) | (203, 0, 113))
        };
        let country = regex::Regex::new(r#"(RevIpCC:"|"yIpc":")([a-z]*)""#).expect("regex");
        // Any 32-hex token in a query string (its & escaped in JSON too):
        // a session, nonce or challenge id.
        let token = regex::Regex::new(r"(?:[?&;]|\\u0026)[A-Za-z_]+=([0-9A-Fa-f]{32})\b").expect("regex");
        let session = regex::Regex::new(r#"(?:^|[^A-Za-z])(?:IG|CID|sid|fclid)[=:]"?([A-Za-z0-9-]{8,})|clientId":"([A-Za-z0-9]+)""#).expect("regex");
        for entry in std::fs::read_dir(&dir).expect("fixtures") {
            let path = entry.expect("entry").path();
            let name = path.file_name().unwrap_or_default().to_string_lossy().into_owned();
            let page = String::from_utf8_lossy(&std::fs::read(&path).expect("read")).into_owned();
            for found in quad.captures_iter(&page) {
                let text = &found[2];
                // Versions such as Chrome/131.0.0.0 aren't addresses.
                if text.ends_with(".0.0") {
                    continue;
                }
                if let Ok(ip) = text.parse::<std::net::Ipv4Addr>() {
                    assert!(allowed(ip), "{name}: an IP address, {ip}");
                }
            }
            assert!(!page.contains("\"ll\":\"") || page.contains("\"ll\":\"0,0\""), "{name}: a position (\"ll\")");
            assert!(page.match_indices("geoid\":").all(|(at, _)| page[at + 7..].starts_with('0')), "{name}: a geoid");
            for found in country.captures_iter(&page) {
                assert_eq!(&found[2], "us", "{name}: a country, {}", &found[0]);
            }
            for found in token.captures_iter(&page) {
                assert!(found[1].chars().all(|c| c == '0'), "{name}: a token, {}", &found[0]);
            }
            for found in session.captures_iter(&page) {
                let id = found.get(1).or_else(|| found.get(2)).map_or("", |id| id.as_str());
                assert!(id.chars().all(|c| c == '0' || c == '-'), "{name}: a session id, {}", &found[0]);
            }
        }
    }

    #[test]
    fn engines_without_a_date_filter_arent_asked_for_recent_results() {
        let recent = Options { recency: Some(Recency::Week), ..Options::default() };
        let supported: Vec<&str> = ORDER.iter().filter(|engine| engine.supports(&recent)).map(|engine| engine.name()).collect();
        assert_eq!(supported, ["DuckDuckGo", "Bing", "Yahoo", "DuckDuckGo Lite", "Brave"]);
        assert!(ORDER.iter().all(|engine| engine.supports(&Options::default())));
    }
}
