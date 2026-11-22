//! DuckDuckGo, through its two script-free pages: HTML and Lite. Both are
//! the results form submitted, a same-origin POST.

use scraper::{ElementRef, Html};

use super::{cut, first_text, query_param, selector, text};
use crate::extensions::web::search::{Hit, Options};

pub(super) fn request(lite: bool, client: &primp::Client, query: String, options: &Options) -> primp::RequestBuilder {
    let origin = if lite { "https://lite.duckduckgo.com" } else { "https://html.duckduckgo.com" };
    let path = if lite { "/lite/" } else { "/html/" };
    let mut form = vec![("q", cut(&query, 499)), ("kl", "wt-wt".to_owned())];
    if !lite {
        form.insert(1, ("b", String::new()));
    }
    if let Some(recency) = options.recency {
        form.push(("df", recency.letter().to_owned()));
    }
    client
        .post(format!("{origin}{path}"))
        .header("Origin", origin)
        .header("Referer", format!("{origin}/"))
        .header("Sec-Fetch-Site", "same-origin")
        .header("Cache-Control", "max-age=0")
        // Sets the form's Content-Type itself; a second one gets blocked.
        .form(&form)
}

/// The real address of a result's link: DuckDuckGo's own links carry it
/// in `uddg`; None for its ads.
fn address(href: &str) -> Option<String> {
    let url = query_param(href, "uddg").unwrap_or_else(|| href.to_owned());
    (!url.starts_with("https://duckduckgo.com/y.js") && url.starts_with("http")).then_some(url)
}

/// Results of the HTML page, ads left out.
pub(super) fn html(page: &Html) -> Vec<Hit> {
    let (item, link, snippet) = (selector("div.result:not(.result--ad)"), selector("a.result__a"), selector(".result__snippet"));
    page.select(&item)
        .filter_map(|result| {
            let anchor = result.select(&link).next()?;
            let url = address(anchor.value().attr("href")?)?;
            Some(Hit { title: text(anchor), url, snippet: first_text(result, &snippet) })
        })
        .collect()
}

/// Results of the Lite page: a table, each result's link in one row and
/// its snippet in the next; sponsored rows left out.
pub(super) fn lite(page: &Html) -> Vec<Hit> {
    let (row, link, snippet) = (selector("tr"), selector("a.result-link"), selector("td.result-snippet"));
    let rows: Vec<ElementRef> = page.select(&row).collect();
    let mut hits = Vec::new();
    for (index, tr) in rows.iter().enumerate() {
        let Some(anchor) = tr.select(&link).next() else { continue };
        if tr.value().attr("class").is_some_and(|class| class.contains("sponsored")) {
            continue;
        }
        let Some(url) = anchor.value().attr("href").and_then(address) else { continue };
        let snippet = rows.get(index + 1).map(|next| first_text(*next, &snippet)).unwrap_or_default();
        hits.push(Hit { title: text(anchor), url, snippet });
    }
    hits
}

pub(super) fn challenged(lower: &str) -> bool {
    lower.contains("challenge-form") || lower.contains("anomaly") || lower.contains("bots use duckduckgo too")
}

pub(super) fn none(page: &Html, lower: &str) -> bool {
    page.select(&selector(".no-results")).next().is_some() || lower.contains("no results.")
}

#[cfg(test)]
mod tests {
    use super::super::tests::{fixture, read};
    use super::super::Engine;
    use crate::extensions::web::search::{Block, Outcome};
    use primp::StatusCode;

    #[test]
    fn html_results_have_their_real_addresses() {
        let found = read(Engine::DuckDuckGo, "ddg-results.html", 10);
        assert!(found.iter().all(|hit| !hit.snippet.is_empty()), "{found:?}");
    }

    #[test]
    fn lite_results_have_their_real_addresses_and_snippets() {
        let found = read(Engine::DuckDuckGoLite, "ddg-lite-results.html", 10);
        assert_eq!(found[0].url, "https://sqlite.org/c3ref/wal_checkpoint_v2.html");
        assert_eq!(found[0].title, "Checkpoint a database - SQLite");
        assert!(found[0].snippet.starts_with("The sqlite3_wal_checkpoint_v2"), "{:?}", found[0]);
    }

    #[test]
    fn a_challenge_is_a_block_on_either_page() {
        let page = fixture("ddg-challenge-202.html");
        for engine in [Engine::DuckDuckGo, Engine::DuckDuckGoLite] {
            let url = "https://html.duckduckgo.com/html/";
            assert_eq!(engine.parse(StatusCode::ACCEPTED, url, &page), Outcome::Blocked(Block::Challenge));
            assert_eq!(engine.parse(StatusCode::OK, url, &page), Outcome::Blocked(Block::Challenge), "even with 200");
        }
    }
}
