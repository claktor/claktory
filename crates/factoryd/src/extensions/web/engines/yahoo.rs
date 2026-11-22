//! Yahoo's results page. Its links go through r.search.yahoo.com (or
//! rd.search.yahoo.com, at times), the address in the path's `RU=` part.

use scraper::Html;

use super::{first_text, selector, text};
use crate::extensions::web::search::{Hit, Options, Recency};

pub(super) fn request(client: &primp::Client, query: String, options: &Options) -> primp::RequestBuilder {
    let mut params = vec![("p", query), ("ei", "UTF-8".to_owned())];
    // Yahoo filters by a day, week or month; a year is asked for unfiltered.
    let age = match options.recency {
        Some(Recency::Day) => Some(("1d", "d")),
        Some(Recency::Week) => Some(("1w", "w")),
        Some(Recency::Month) => Some(("1m", "m")),
        Some(Recency::Year) | None => None,
    };
    if let Some((age, btf)) = age {
        params.push(("age", age.to_owned()));
        params.push(("btf", btf.to_owned()));
    }
    client.get("https://search.yahoo.com/search").query(&params)
}

/// The address a Yahoo link stands for.
fn address(href: &str) -> Option<String> {
    let host = primp::Url::parse(href).ok().and_then(|url| url.host_str().map(str::to_owned)).unwrap_or_default();
    if !host.ends_with(".search.yahoo.com") || !href.contains("/RU=") {
        return href.starts_with("http").then(|| href.to_owned());
    }
    let encoded = href.split("/RU=").nth(1)?.split('/').next()?;
    let url = percent_decode(encoded);
    url.starts_with("http").then_some(url)
}

fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%'
            && let Some(byte) = text.get(index + 1..index + 3).and_then(|hex| u8::from_str_radix(hex, 16).ok())
        {
            out.push(byte);
            index += 3;
        } else {
            out.push(bytes[index]);
            index += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

pub(super) fn hits(page: &Html) -> Vec<Hit> {
    let (item, link, title, snippet) = (selector("div.algo"), selector(".compTitle a"), selector("h3"), selector(".compText"));
    page.select(&item)
        .filter_map(|result| {
            let anchor = result.select(&link).next()?;
            let url = address(anchor.value().attr("href")?)?;
            let title = anchor.select(&title).next().map(text).unwrap_or_else(|| text(anchor));
            Some(Hit { title, url, snippet: first_text(result, &snippet) })
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
        let found = read(Engine::Yahoo, "yahoo-results.html", 6);
        assert_eq!(found[0].url, "https://sqlite.org/wal.html");
        assert_eq!(found[0].title, "Write-Ahead Logging - SQLite");
        assert!(found[0].snippet.contains("checkpoint"), "{:?}", found[0]);
    }

    #[test]
    fn a_link_through_yahoo_is_decoded() {
        let href = "https://r.search.yahoo.com/_ylt=A;_ylu=Y/RV=2/RE=1/RO=10/RU=https%3a%2f%2fsqlite.org%2fwal.html/RK=2/RS=x-";
        assert_eq!(address(href).as_deref(), Some("https://sqlite.org/wal.html"));
        // Its other redirect host, seen now and then.
        let rd = "https://rd.search.yahoo.com/_ylt=A;_ylu=Y/RV=2/RE=1/RO=10/RU=https%3a%2f%2fdocs.rs%2ftokio%2flatest%2ftokio%2fmacro.select.html/RK=0/RS=x-";
        assert_eq!(address(rd).as_deref(), Some("https://docs.rs/tokio/latest/tokio/macro.select.html"));
        // A link that isn't a redirect stays as it is.
        assert_eq!(address("https://www.yahoo.com/news/x.html").as_deref(), Some("https://www.yahoo.com/news/x.html"));
    }
}
