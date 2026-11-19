//! The search engines Factory queries the way a browser does: how each one's
//! request looks, which browser it's made as, and how its page is read,
//! blocks included.

use primp::{Impersonate, ImpersonateOS, StatusCode};
use scraper::{ElementRef, Html, Selector};

use super::search::{Block, Hit, Options, Outcome, Recency};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum Engine {
    DuckDuckGo,
    Brave,
}

/// The order queries try them in.
pub(crate) const ORDER: [Engine; 2] = [Engine::DuckDuckGo, Engine::Brave];

/// How often an engine may be asked, and how long it's left alone after a
/// block. Every agent shares one IP, so this is for the whole Factory.
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
            Engine::Brave => "Brave",
        }
    }

    /// DuckDuckGo challenges Chrome's handshake, so it gets Firefox; Brave
    /// is Chromium, so it gets Chrome.
    pub(crate) fn profile(self) -> Profile {
        let browser = match self {
            Engine::DuckDuckGo => Impersonate::FirefoxV151,
            Engine::Brave => Impersonate::ChromeV153,
        };
        Profile { browser, os: host_os() }
    }

    pub(crate) fn policy(self) -> Policy {
        match self {
            Engine::DuckDuckGo => {
                Policy { gap_ms: 3_000, jitter_ms: 2_000, per_hour: 60, challenge_max_ms: 24 * HOUR, limited_max_ms: HOUR }
            }
            Engine::Brave => {
                Policy { gap_ms: 20_000, jitter_ms: 5_000, per_hour: 15, challenge_max_ms: 24 * HOUR, limited_max_ms: 6 * HOUR }
            }
        }
    }

    /// Cookies a browser that had used the site would carry, by URL.
    pub(crate) fn cookies(self) -> (&'static str, &'static [&'static str]) {
        match self {
            Engine::DuckDuckGo => ("https://html.duckduckgo.com/", &["kl=wt-wt"]),
            Engine::Brave => {
                ("https://search.brave.com/", &["country=all", "useLocation=0", "summarizer=0", "safesearch=moderate", "ui_lang=en-us"])
            }
        }
    }

    /// The request for `query`, as the browser would send it.
    pub(crate) fn request(self, client: &primp::Client, query: &str, options: &Options) -> primp::RequestBuilder {
        let query = with_domains(query, &options.domains);
        match self {
            // The results form, submitted: a same-origin POST.
            Engine::DuckDuckGo => {
                let mut form = vec![("q", cut(&query, 499)), ("b", String::new()), ("kl", "wt-wt".to_owned())];
                if let Some(recency) = options.recency {
                    form.push(("df", recency.letter().to_owned()));
                }
                client
                    .post("https://html.duckduckgo.com/html/")
                    .header("Origin", "https://html.duckduckgo.com")
                    .header("Referer", "https://html.duckduckgo.com/")
                    .header("Sec-Fetch-Site", "same-origin")
                    .header("Cache-Control", "max-age=0")
                    // Sets the form's Content-Type itself; a second one gets blocked.
                    .form(&form)
            }
            // Typed into the address bar: a navigation, with the profile's headers as they are.
            Engine::Brave => {
                let mut params = vec![("q", query), ("source", "web".to_owned())];
                if let Some(recency) = options.recency {
                    params.push(("tf", format!("p{}", recency.letter())));
                }
                client.get("https://search.brave.com/search").query(&params)
            }
        }
    }

    /// What the engine's answer says.
    pub(crate) fn parse(self, status: StatusCode, final_url: &str, body: &str) -> Outcome {
        if final_url.contains("challenges.cloudflare.com") || final_url.contains("/sorry/") {
            return Outcome::Blocked(Block::Challenge);
        }
        match status.as_u16() {
            429 => return Outcome::Blocked(Block::RateLimited),
            402 | 403 => return Outcome::Blocked(Block::Denied),
            // DuckDuckGo answers a suspected bot with 202 and a challenge.
            202 if self == Engine::DuckDuckGo => return Outcome::Blocked(Block::Challenge),
            _ => {}
        }
        if !status.is_success() {
            return Outcome::Broken(format!("HTTP {status}"));
        }
        let page = Html::parse_document(body);
        let hits = match self {
            Engine::DuckDuckGo => duckduckgo(&page),
            Engine::Brave => brave(&page),
        };
        if !hits.is_empty() {
            return Outcome::Results(hits);
        }
        let lower = body.to_lowercase();
        let challenged = match self {
            Engine::DuckDuckGo => {
                lower.contains("challenge-form") || lower.contains("anomaly") || lower.contains("bots use duckduckgo too")
            }
            Engine::Brave => lower.contains("captcha") || title(&page).trim() == "Brave Search",
        };
        if challenged {
            return Outcome::Blocked(Block::Challenge);
        }
        let none = match self {
            Engine::DuckDuckGo => page.select(&selector(".no-results")).next().is_some(),
            Engine::Brave => lower.contains("no results found") || lower.contains("not many great matches"),
        };
        if none { Outcome::NoResults } else { Outcome::Broken(body.chars().take(300).collect()) }
    }
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

/// Results of DuckDuckGo's HTML page, ads left out. Links go through
/// DuckDuckGo; the real address is their `uddg` parameter.
fn duckduckgo(page: &Html) -> Vec<Hit> {
    let (item, link, snippet) = (selector("div.result:not(.result--ad)"), selector("a.result__a"), selector(".result__snippet"));
    page.select(&item)
        .filter_map(|result| {
            let anchor = result.select(&link).next()?;
            let href = anchor.value().attr("href")?;
            let url = query_param(href, "uddg").unwrap_or_else(|| href.to_owned());
            if url.starts_with("https://duckduckgo.com/y.js") || !url.starts_with("http") {
                return None;
            }
            let snippet = result.select(&snippet).next().map(text).unwrap_or_default();
            Some(Hit { title: text(anchor), url, snippet })
        })
        .collect()
}

/// Results of Brave's page: its web snippets only.
fn brave(page: &Html) -> Vec<Hit> {
    let (item, link, title, snippet) = (
        selector("div.snippet[data-type=web]"),
        selector("a[href^=http]"),
        selector(".title"),
        selector(".generic-snippet .content, .snippet-description"),
    );
    page.select(&item)
        .filter_map(|result| {
            let url = result.select(&link).next()?.value().attr("href")?.to_owned();
            let title = result.select(&title).next().map(text).unwrap_or_default();
            let snippet = result.select(&snippet).next().map(text).unwrap_or_default();
            Some(Hit { title, url, snippet })
        })
        .collect()
}

fn query_param(href: &str, key: &str) -> Option<String> {
    let full = if href.starts_with("//") { format!("https:{href}") } else { href.to_owned() };
    primp::Url::parse(&full).ok()?.query_pairs().find(|(name, _)| name == key).map(|(_, value)| value.into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(name: &str) -> String {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/web").join(name);
        std::fs::read_to_string(&path).unwrap_or_else(|error| panic!("{}: {error}", path.display()))
    }

    fn hits(outcome: Outcome) -> Vec<Hit> {
        match outcome {
            Outcome::Results(hits) => hits,
            other => panic!("not results: {other:?}"),
        }
    }

    #[test]
    fn duckduckgo_results_have_their_real_addresses() {
        let found = hits(Engine::DuckDuckGo.parse(StatusCode::OK, "https://html.duckduckgo.com/html/", &fixture("ddg-results.html")));
        assert_eq!(found.len(), 10);
        assert!(found.iter().all(|hit| hit.url.starts_with("http") && !hit.url.contains("duckduckgo.com/l/")), "{found:?}");
        assert!(found.iter().all(|hit| !hit.title.is_empty() && !hit.snippet.is_empty()), "{found:?}");
    }

    #[test]
    fn a_duckduckgo_challenge_is_a_block() {
        let page = fixture("ddg-challenge-202.html");
        let url = "https://html.duckduckgo.com/html/";
        assert_eq!(Engine::DuckDuckGo.parse(StatusCode::ACCEPTED, url, &page), Outcome::Blocked(Block::Challenge));
        assert_eq!(Engine::DuckDuckGo.parse(StatusCode::OK, url, &page), Outcome::Blocked(Block::Challenge), "even with 200");
    }

    #[test]
    fn brave_results_and_its_rate_limit() {
        let found = hits(Engine::Brave.parse(StatusCode::OK, "https://search.brave.com/search", &fixture("brave-results.html")));
        assert!(found.len() >= 15, "{}", found.len());
        assert!(found.iter().all(|hit| hit.url.starts_with("http") && !hit.title.is_empty()), "{found:?}");
        let page = fixture("brave-429-captcha.html");
        let url = "https://search.brave.com/search";
        assert_eq!(Engine::Brave.parse(StatusCode::TOO_MANY_REQUESTS, url, &page), Outcome::Blocked(Block::RateLimited));
        assert_eq!(Engine::Brave.parse(StatusCode::OK, url, &page), Outcome::Blocked(Block::Challenge), "the CAPTCHA page itself");
    }

    #[test]
    fn an_empty_page_is_broken_and_a_challenge_host_is_a_block() {
        assert!(matches!(Engine::Brave.parse(StatusCode::OK, "https://search.brave.com/search", ""), Outcome::Broken(_)));
        let outcome = Engine::DuckDuckGo.parse(StatusCode::OK, "https://challenges.cloudflare.com/x", "<html></html>");
        assert_eq!(outcome, Outcome::Blocked(Block::Challenge));
        assert_eq!(Engine::Brave.parse(StatusCode::FORBIDDEN, "https://search.brave.com/search", ""), Outcome::Blocked(Block::Denied));
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
            assert!(!matches!(profile.browser, Impersonate::Random | Impersonate::Chrome | Impersonate::Firefox));
        }
        assert_eq!(Engine::DuckDuckGo.profile().browser, Impersonate::FirefoxV151, "DuckDuckGo challenges Chrome");
    }
}
