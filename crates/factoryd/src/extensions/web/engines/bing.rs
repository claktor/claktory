//! Bing's results page. Its links go through bing.com/ck/a, the address in
//! `u` as "a1" and unpadded URL-safe base64.

use base64::Engine as _;
use scraper::Html;

use super::{first_text, query_param, selector, text};
use crate::extensions::web::search::{Hit, Options, Recency};

const DAY_MS: i64 = 86_400_000;

pub(super) fn request(client: &primp::Client, query: String, options: &Options) -> primp::RequestBuilder {
    // `form=QBLH` is what the search box sends; without it Bing answers
    // a plain request with results for only the query's first word.
    let mut params = vec![("q", query), ("form", "QBLH".to_owned())];
    if let Some(recency) = options.recency {
        let filter = match recency {
            Recency::Day => "ex1:\"ez1\"".to_owned(),
            Recency::Week => "ex1:\"ez2\"".to_owned(),
            Recency::Month => "ex1:\"ez3\"".to_owned(),
            // A range of days since 1970: the last year.
            Recency::Year => {
                let today = crate::transcript::now_ms() / DAY_MS;
                format!("ex1:\"ez5_{}_{today}\"", today - 365)
            }
        };
        params.push(("filters", filter));
    }
    client.get("https://www.bing.com/search").query(&params)
}

/// The address a Bing link stands for.
fn address(href: &str) -> Option<String> {
    if !href.contains("bing.com/ck/") {
        return href.starts_with("http").then(|| href.to_owned());
    }
    let encoded = query_param(href, "u")?;
    let encoded = encoded.strip_prefix("a1")?;
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(encoded.trim_end_matches('=')).ok()?;
    String::from_utf8(bytes).ok().filter(|url| url.starts_with("http"))
}

pub(super) fn hits(page: &Html) -> Vec<Hit> {
    let (item, link, snippet) = (selector("li.b_algo"), selector("h2 a"), selector(".b_caption p, p.b_lineclamp2, p.b_lineclamp3, p.b_lineclamp4"));
    page.select(&item)
        .filter_map(|result| {
            let anchor = result.select(&link).next()?;
            let url = address(anchor.value().attr("href")?)?;
            Some(Hit { title: text(anchor), url, snippet: first_text(result, &snippet) })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::super::Engine;
    use super::super::tests::read;
    use super::address;

    #[test]
    fn results_have_their_real_addresses() {
        let found = read(Engine::Bing, "bing-results.html", 8);
        assert_eq!(found[0].url, "https://sqlite.org/c3ref/wal_checkpoint_v2.html");
        assert_eq!(found[0].title, "Checkpoint a database - SQLite");
        assert!(found.iter().filter(|hit| !hit.snippet.is_empty()).count() >= 8, "{found:#?}");
    }

    #[test]
    fn its_challenge_is_a_block() {
        let page = super::super::tests::fixture("bing-challenge.html");
        let outcome = Engine::Bing.parse(primp::StatusCode::OK, "https://www.bing.com/search", &page);
        assert_eq!(outcome, crate::extensions::web::search::Outcome::Blocked(crate::extensions::web::search::Block::Challenge));
    }

    #[test]
    fn a_link_through_bing_is_decoded() {
        let href = "https://www.bing.com/ck/a?!&&p=x&ptn=3&u=a1aHR0cHM6Ly9zcWxpdGUub3JnL2luZGV4Lmh0bWw&ntb=1";
        assert_eq!(address(href).as_deref(), Some("https://sqlite.org/index.html"));
        assert_eq!(address("https://www.bing.com/ck/a?u=zz").as_deref(), None);
        assert_eq!(address("https://example.com/x").as_deref(), Some("https://example.com/x"));
    }
}
