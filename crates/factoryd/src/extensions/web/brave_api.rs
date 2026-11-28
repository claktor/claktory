//! The Brave Search API: search through Brave's official API with the
//! person's own key, before the public engines. They add it as a secret in
//! the vault that sets `BRAVE_SEARCH_API_KEY` (or `BRAVE_API_KEY`, as
//! Brave's own tools name it); with none, search is as it was. Its free
//! credits cover about 1,000 searches a month, so it's asked at most that
//! many times in any 31 days (`Engine::policy`), and when Brave says the
//! month's are used, or refuses or fails, the query goes to the public
//! engines instead.
//!
//! The key goes only in the `X-Subscription-Token` header, only to Brave's
//! fixed address, and redirects aren't followed. No location is sent, nor
//! anything but the query and its filters.

use std::sync::OnceLock;
use std::time::Duration;

use serde::Deserialize;

use super::search::{Block, Hit, Options, Outcome};

/// Where every request goes.
const ENDPOINT: &str = "https://api.search.brave.com/res/v1/web/search";
/// Variables a vault secret may set the key in, the first found used.
pub(crate) const VARIABLES: [&str; 2] = ["BRAVE_SEARCH_API_KEY", "BRAVE_API_KEY"];
/// The most of an answer read.
const MAX_BODY: usize = 1 << 20;
/// What Brave takes in a query.
const MAX_CHARS: usize = 600;
const MAX_WORDS: usize = 75;
/// A quota window this long or longer is the month's (`X-RateLimit-Policy`).
const DAY_S: i64 = 86_400;
/// The longest Claktory waits for a quota to come back, in ms.
const MAX_WAIT_MS: i64 = 31 * DAY_S * 1000;

/// Whether the query, with its domain filters, is one Brave takes; a longer
/// one goes to the public engines rather than being cut.
pub(crate) fn fits(query: &str, options: &Options) -> bool {
    let query = super::engines::with_domains(query, &options.domains);
    query.chars().count() <= MAX_CHARS && query.split_whitespace().count() <= MAX_WORDS
}

fn client() -> Result<&'static reqwest::Client, String> {
    static CLIENT: OnceLock<Result<reqwest::Client, String>> = OnceLock::new();
    CLIENT
        .get_or_init(|| {
            reqwest::Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .https_only(true)
                .timeout(Duration::from_secs(10))
                .build()
                .map_err(|error| format!("can't set up the Brave Search API: {error}"))
        })
        .as_ref()
        .map_err(Clone::clone)
}

/// Asks Brave for `query` with `key`: what its answer says, or why it
/// couldn't be had (Brave out of reach, or failing).
pub(crate) async fn ask(key: &str, query: &str, options: &Options) -> Result<Outcome, String> {
    let mut params = vec![
        ("q", super::engines::with_domains(query, &options.domains)),
        ("count", "20".to_owned()),
        ("country", "ALL".to_owned()),
        ("safesearch", "moderate".to_owned()),
        ("text_decorations", "false".to_owned()),
        ("result_filter", "web".to_owned()),
    ];
    if let Some(recency) = options.recency {
        params.push(("freshness", format!("p{}", recency.letter())));
    }
    let mut response = client()?
        .get(ENDPOINT)
        .query(&params)
        .header("Accept", "application/json")
        .header("X-Subscription-Token", key)
        .send()
        .await
        // Without the URL: it holds the query, and isn't needed to say why.
        .map_err(|error| error.without_url().to_string())?;
    let status = response.status().as_u16();
    let headers = response.headers().clone();
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|error| error.without_url().to_string())? {
        if body.len() + chunk.len() > MAX_BODY {
            return Ok(Outcome::Broken(format!("an answer over {MAX_BODY} bytes")));
        }
        body.extend_from_slice(&chunk);
    }
    if status >= 500 {
        return Err(format!("Brave answered {status}"));
    }
    Ok(parse(
        status,
        &header(&headers, "x-ratelimit-policy"),
        &header(&headers, "x-ratelimit-remaining"),
        &header(&headers, "x-ratelimit-reset"),
        &body,
    ))
}

fn header(headers: &reqwest::header::HeaderMap, name: &str) -> String {
    headers.get(name).and_then(|value| value.to_str().ok()).unwrap_or_default().to_owned()
}

#[derive(Deserialize)]
struct Answer {
    #[serde(default)]
    web: Option<Web>,
}

#[derive(Deserialize)]
struct Web {
    #[serde(default)]
    results: Vec<Result_>,
}

#[derive(Deserialize)]
struct Result_ {
    #[serde(default)]
    title: String,
    #[serde(default)]
    url: String,
    #[serde(default)]
    description: String,
}

/// What Brave's answer says, from its status, its rate limit headers
/// (`policy`, `remaining` and `reset`, each a comma-separated list, one per
/// window) and its body.
pub(crate) fn parse(status: u16, policy: &str, remaining: &str, reset: &str, body: &[u8]) -> Outcome {
    match status {
        200 => {}
        401 | 403 => return Outcome::Blocked(Block::KeyRefused),
        // No credits left.
        402 => return Outcome::Exhausted(DAY_S * 1000),
        429 => {
            return match month_used(policy, remaining, reset) {
                Some(wait_ms) => Outcome::Exhausted(wait_ms),
                None => Outcome::Blocked(Block::RateLimited),
            };
        }
        // Brave answers a key it doesn't know with 422, as it does a query
        // it won't take; its error says which.
        400..=499 if key_refused(body) => return Outcome::Blocked(Block::KeyRefused),
        // The query itself (a filter Brave won't take): another engine may.
        400..=499 => return Outcome::Refused(format!("Brave refused this query ({status})")),
        _ => return Outcome::Broken(format!("HTTP {status}")),
    }
    let Ok(answer) = serde_json::from_slice::<Answer>(body) else {
        return Outcome::Broken(String::from_utf8_lossy(&body[..body.len().min(300)]).into_owned());
    };
    let hits: Vec<Hit> = answer
        .web
        .map(|web| web.results)
        .unwrap_or_default()
        .into_iter()
        .filter(|result| result.url.starts_with("https://") || result.url.starts_with("http://"))
        .take(20)
        .map(|result| Hit { title: plain(&result.title), url: result.url, snippet: plain(&result.description) })
        .collect();
    if hits.is_empty() { Outcome::NoResults } else { Outcome::Results(hits) }
}

/// Whether an error answer is about the key rather than the query: its
/// code, or the part of Brave that refused it.
fn key_refused(body: &[u8]) -> bool {
    let Ok(answer) = serde_json::from_slice::<serde_json::Value>(body) else { return false };
    let error = answer.get("error").unwrap_or(&answer);
    error["code"].as_str().is_some_and(|code| code.starts_with("SUBSCRIPTION_TOKEN"))
        || error["meta"]["component"].as_str() == Some("authentication")
}

/// When a 429 is the month's quota used up: how long until it comes back
/// (ms, at most 31 days). None when it's only the per-second limit.
fn month_used(policy: &str, remaining: &str, reset: &str) -> Option<i64> {
    let list = |text: &str| -> Vec<i64> {
        text.split(',').map(|part| part.split(';').next().unwrap_or_default().trim().parse().unwrap_or(-1)).collect()
    };
    let windows: Vec<i64> = policy
        .split(',')
        .map(|part| part.split(';').find_map(|field| field.trim().strip_prefix("w=")).and_then(|w| w.trim().parse().ok()).unwrap_or(0))
        .collect();
    let (remaining, reset) = (list(remaining), list(reset));
    windows
        .iter()
        .enumerate()
        .filter(|(at, window)| **window >= DAY_S && remaining.get(*at) == Some(&0))
        .map(|(at, _)| reset.get(at).copied().filter(|seconds| *seconds > 0).unwrap_or(DAY_S).saturating_mul(1000).min(MAX_WAIT_MS))
        .max()
}

/// Text as shown: any markup taken out, entities read, spaces collapsed.
fn plain(text: &str) -> String {
    let fragment = scraper::Html::parse_fragment(text);
    fragment.root_element().text().collect::<Vec<_>>().join("").split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    const RESULTS: &str = r#"{"type":"search","query":{"original":"sqlite wal"},"web":{"type":"search","results":[
        {"title":"Write-Ahead Logging","url":"https://sqlite.org/wal.html","description":"The <strong>WAL</strong> journal mode &amp; checkpoints.","extra":1},
        {"title":"Not a page","url":"javascript:alert(1)","description":"x"},
        {"title":"WAL mode in practice","url":"https://example.com/wal","description":""}]}}"#;

    #[test]
    fn results_are_read_plain_and_only_web_addresses_kept() {
        let Outcome::Results(hits) = parse(200, "", "", "", RESULTS.as_bytes()) else { panic!("results") };
        assert_eq!(hits.len(), 2);
        assert_eq!(
            hits[0],
            Hit {
                title: "Write-Ahead Logging".into(),
                url: "https://sqlite.org/wal.html".into(),
                snippet: "The WAL journal mode & checkpoints.".into()
            }
        );
        assert_eq!(parse(200, "", "", "", br#"{"type":"search","query":{"original":"zxqv"}}"#), Outcome::NoResults);
        assert!(matches!(parse(200, "", "", "", b"<html>"), Outcome::Broken(_)));
    }

    #[test]
    fn a_used_month_waits_for_its_reset_and_a_busy_second_doesnt() {
        let policy = "1;w=1, 1000;w=2592000";
        assert_eq!(parse(429, policy, "0, 0", "1, 86400", b""), Outcome::Exhausted(86_400_000));
        assert_eq!(parse(429, policy, "0, 412", "1, 86400", b""), Outcome::Blocked(Block::RateLimited));
        assert_eq!(parse(429, policy, "0, 0", "1, 999999999", b""), Outcome::Exhausted(MAX_WAIT_MS), "at most 31 days");
        assert_eq!(parse(429, "", "", "", b""), Outcome::Blocked(Block::RateLimited), "no headers");
        assert_eq!(parse(402, "", "", "", b""), Outcome::Exhausted(86_400_000));
        assert_eq!(parse(401, "", "", "", b"{\"error\":\"bad token\"}"), Outcome::Blocked(Block::KeyRefused));
        assert!(matches!(parse(422, "", "", "", b""), Outcome::Refused(_)));
        let invalid = br#"{"error":{"code":"SUBSCRIPTION_TOKEN_INVALID","detail":"The provided subscription token is invalid.","meta":{"component":"authentication"},"status":422},"type":"ErrorResponse"}"#;
        assert_eq!(parse(422, "", "", "", invalid), Outcome::Blocked(Block::KeyRefused), "Brave's answer to a key it doesn't know");
        let filter = br#"{"error":{"code":"VALIDATION","detail":"Unable to validate request parameter(s)","meta":{"component":"validation"},"status":422},"type":"ErrorResponse"}"#;
        assert!(matches!(parse(422, "", "", "", filter), Outcome::Refused(_)), "a query it won't take");
    }

    /// One real search with a key, by hand:
    /// `BRAVE_SEARCH_API_KEY=… cargo test -p factoryd -- --ignored web_live_brave_api`.
    /// It uses one of the key's searches.
    #[tokio::test]
    #[ignore]
    async fn web_live_brave_api() {
        let key = std::env::var("BRAVE_SEARCH_API_KEY").expect("BRAVE_SEARCH_API_KEY");
        let outcome = ask(&key, "sqlite wal checkpoint", &Options { limit: 8, ..Options::default() }).await.expect("answered");
        eprintln!("{outcome:?}");
        assert!(matches!(outcome, Outcome::Results(ref hits) if hits.len() >= 5), "{outcome:?}");
    }

    #[test]
    fn only_queries_brave_takes_go_to_it() {
        let options = Options { domains: vec!["sqlite.org".into()], ..Options::default() };
        assert!(fits("sqlite wal checkpoint", &options));
        assert!(!fits(&"word ".repeat(80), &Options::default()), "over 75 words");
        assert!(!fits(&"x".repeat(601), &Options::default()), "over 600 characters");
    }
}
