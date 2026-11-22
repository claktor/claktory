//! Ecosia's results page: its organic results, ads left out.

use scraper::Html;

use super::{first_text, selector, text};
use crate::extensions::web::search::Hit;

pub(super) fn hits(page: &Html) -> Vec<Hit> {
    let (item, link, title, snippet) = (
        selector("[data-test-id=organic-result]"),
        selector(".result__title a[data-test-id=result-link]"),
        selector("[data-test-id=result-title]"),
        selector("[data-test-id=web-result-description], [data-test-id=result-description]"),
    );
    page.select(&item)
        .filter_map(|result| {
            let anchor = result.select(&link).next()?;
            let url = anchor.value().attr("href")?.to_owned();
            let title = result.select(&title).next().map(text).unwrap_or_else(|| text(anchor));
            Some(Hit { title, url, snippet: first_text(result, &snippet) })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::super::Engine;
    use super::super::tests::read;

    #[test]
    fn organic_results_are_read() {
        let found = read(Engine::Ecosia, "ecosia-results.html", 8);
        assert_eq!(found[0].url, "https://sqlite.org/c3ref/wal_checkpoint_v2.html");
        assert_eq!(found[0].title, "Checkpoint a database - SQLite");
        assert!(found[0].snippet.starts_with("The sqlite3_wal_checkpoint_v2"), "{:?}", found[0]);
    }
}
