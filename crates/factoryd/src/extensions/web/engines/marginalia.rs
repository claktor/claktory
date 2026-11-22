//! Marginalia, an independent index of the small web: each result is a
//! heading's link, with its description in a paragraph below. When it's
//! busy it first asks every visitor to wait a second or two, and gives
//! browsers without JavaScript a link to go on by hand once they have.

use scraper::{ElementRef, Html};

use super::{selector, text};
use crate::extensions::web::search::Hit;

pub(super) fn hits(page: &Html) -> Vec<Hit> {
    // Results' links say which way their text runs; the page's own don't.
    let (link, paragraph) = (selector("h2 > a[href^=http][dir=auto]"), selector("p"));
    page.select(&link)
        .filter_map(|anchor| {
            let url = anchor.value().attr("href")?.to_owned();
            // The nearest card around the heading that also holds a paragraph.
            let snippet = anchor
                .ancestors()
                .filter_map(ElementRef::wrap)
                .take(8)
                .find_map(|card| card.select(&paragraph).next().map(text))
                .unwrap_or_default();
            // Its titles break long words with <wbr>: no space between those parts.
            let title = anchor.text().collect::<String>().split_whitespace().collect::<Vec<_>>().join(" ");
            Some(Hit { title, url, snippet })
        })
        .collect()
}

/// Its wait page: how long it asks to wait, in seconds, and the link to go
/// on by hand afterwards. None for any other page.
pub(super) fn wait(page: &Html) -> Option<(u64, String)> {
    let countdown = page.select(&selector("#countdown")).next()?;
    let seconds = countdown.value().attr("data-tr").and_then(|seconds| seconds.parse().ok()).unwrap_or(1);
    // The link is inside <noscript>, which the parser keeps as text.
    let noscript = page.select(&selector("noscript")).map(|element| element.inner_html()).find(|html| html.contains("/search?"))?;
    let start = noscript.find("href=\"")? + 6;
    // Escaped once by the page and again as <noscript>'s text.
    let mut href = noscript[start..].split('"').next()?.to_owned();
    while href.contains("&amp;") {
        href = href.replace("&amp;", "&");
    }
    href.starts_with("/search?").then(|| (seconds, format!("https://marginalia-search.com{href}")))
}

#[cfg(test)]
mod tests {
    use super::super::Engine;
    use super::super::tests::read;

    #[test]
    fn results_are_read_with_their_descriptions() {
        let found = read(Engine::Marginalia, "marginalia-results.html", 10);
        let wal = found.iter().find(|hit| hit.url == "https://www.sqlite.org/wal.html").expect("the WAL page");
        assert_eq!(wal.title, "Write-Ahead Logging");
        assert!(wal.snippet.starts_with("By default, SQLite does a checkpoint"), "{wal:?}");
    }

    #[test]
    fn its_wait_page_is_waited_out_then_followed() {
        let page = scraper::Html::parse_document(&super::super::tests::fixture("marginalia-wait.html"));
        let (seconds, url) = super::wait(&page).expect("a wait page");
        assert_eq!(seconds, 1);
        assert_eq!(url, "https://marginalia-search.com/search?query=sqlite+wal+checkpoint&sst=SE-2b2f453767c6f98f");
        // Read by itself, it's a block, not results.
        let outcome = Engine::Marginalia.parse(primp::StatusCode::OK, &url, &super::super::tests::fixture("marginalia-wait.html"));
        assert_eq!(outcome, crate::extensions::web::search::Outcome::Blocked(crate::extensions::web::search::Block::Challenge));
        let results = scraper::Html::parse_document(&super::super::tests::fixture("marginalia-results.html"));
        assert_eq!(super::wait(&results), None);
    }
}
