//! Brave Search's results page, typed into the address bar: a navigation,
//! with the profile's headers as they are.

use scraper::Html;

use super::{first_text, selector};
use crate::extensions::web::search::{Hit, Options};

pub(super) fn request(client: &primp::Client, query: String, options: &Options) -> primp::RequestBuilder {
    let mut params = vec![("q", query), ("source", "web".to_owned())];
    if let Some(recency) = options.recency {
        params.push(("tf", format!("p{}", recency.letter())));
    }
    client.get("https://search.brave.com/search").query(&params)
}

/// Its web snippets only.
pub(super) fn hits(page: &Html) -> Vec<Hit> {
    let (item, link, title, snippet) = (
        selector("div.snippet[data-type=web]"),
        selector("a[href^=http]"),
        selector(".title"),
        selector(".generic-snippet .content, .snippet-description"),
    );
    page.select(&item)
        .filter_map(|result| {
            let url = result.select(&link).next()?.value().attr("href")?.to_owned();
            Some(Hit { title: first_text(result, &title), url, snippet: first_text(result, &snippet) })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::super::Engine;
    use super::super::tests::{fixture, read};
    use crate::extensions::web::search::{Block, Outcome};
    use primp::StatusCode;

    #[test]
    fn results_and_its_rate_limit() {
        read(Engine::Brave, "brave-results.html", 15);
        let page = fixture("brave-429-captcha.html");
        let url = "https://search.brave.com/search";
        assert_eq!(Engine::Brave.parse(StatusCode::TOO_MANY_REQUESTS, url, &page), Outcome::Blocked(Block::RateLimited));
        assert_eq!(Engine::Brave.parse(StatusCode::OK, url, &page), Outcome::Blocked(Block::Challenge), "the CAPTCHA page itself");
    }
}
