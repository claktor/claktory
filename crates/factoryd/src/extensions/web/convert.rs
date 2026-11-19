//! Pages as Markdown, and the slices and matches of them web_fetch shows.

use std::sync::LazyLock;

use regex::Regex;

/// About this many characters of a page per call.
const SLICE: usize = 20_000;
/// At most this many matches of `find`…
const MATCHES: usize = 40;
/// …each with this many lines around it.
const CONTEXT: usize = 2;

/// A page read as Markdown.
#[derive(Clone, Debug)]
pub(crate) struct Page {
    pub(crate) url: String,
    pub(crate) title: String,
    pub(crate) lines: Vec<String>,
}

impl Page {
    pub(crate) fn new(url: &str, title: String, markdown: &str) -> Page {
        // Runs of blank lines are one.
        let mut lines: Vec<String> = Vec::new();
        for line in markdown.lines().map(str::trim_end) {
            if line.is_empty() && lines.last().is_none_or(String::is_empty) {
                continue;
            }
            lines.push(line.to_owned());
        }
        while lines.last().is_some_and(String::is_empty) {
            lines.pop();
        }
        Page { url: url.to_owned(), title, lines }
    }

    fn heading(&self) -> String {
        if self.title.is_empty() { self.url.clone() } else { format!("{} — \"{}\"", self.url, self.title) }
    }

    /// About `SLICE` characters of numbered lines from `offset` (1-based).
    pub(crate) fn slice(&self, offset: usize) -> String {
        let total = self.lines.len();
        if total == 0 {
            return format!("{} — the page has no text.", self.heading());
        }
        let start = offset.clamp(1, total);
        let mut shown = Vec::new();
        let mut size = 0;
        for (index, line) in self.lines.iter().enumerate().skip(start - 1) {
            let numbered = format!("{}: {line}", index + 1);
            if size > 0 && size + numbered.len() > SLICE {
                break;
            }
            size += numbered.len() + 1;
            shown.push(numbered);
        }
        let end = start + shown.len() - 1;
        let more = if end < total { format!(" Continue with offset={}, or use find.", end + 1) } else { String::new() };
        format!("{} — lines {start}–{end} of {total}.{more}\n\n{}", self.heading(), shown.join("\n"))
    }

    /// Every line holding one of `find`'s parts (`|` between them, any
    /// case), with the lines around it.
    pub(crate) fn find(&self, find: &str) -> String {
        let parts: Vec<String> = find.split('|').map(|part| part.trim().to_lowercase()).filter(|part| !part.is_empty()).collect();
        let hits: Vec<usize> = (0..self.lines.len())
            .filter(|index| {
                let line = self.lines[*index].to_lowercase();
                parts.iter().any(|part| line.contains(part.as_str()))
            })
            .collect();
        if hits.is_empty() {
            return format!("{} — nothing matches \"{find}\" in its {} lines.", self.heading(), self.lines.len());
        }
        let shown = &hits[..hits.len().min(MATCHES)];
        let mut out = format!(
            "{} — {} line{} match \"{find}\"{}:\n",
            self.heading(),
            hits.len(),
            if hits.len() == 1 { "" } else { "s" },
            if hits.len() > shown.len() { format!(", the first {} shown", shown.len()) } else { String::new() }
        );
        let mut last = None;
        for &hit in shown {
            let from = hit.saturating_sub(CONTEXT).max(last.map_or(0, |last: usize| last + 1));
            let to = (hit + CONTEXT).min(self.lines.len() - 1);
            if last.is_none_or(|last| from > last + 1) {
                out.push('\n');
            }
            for index in from..=to {
                out.push_str(&format!("{}: {}\n", index + 1, self.lines[index]));
            }
            last = Some(to);
        }
        out
    }
}

/// An HTML page's main content as Markdown, with its title. Links are made
/// absolute against `url`.
pub(crate) fn html(url: &str, html: &str) -> (String, String) {
    let (title, content) = match dom_smoothie::Readability::new(html, Some(url), None).and_then(|mut page| page.parse()) {
        Ok(article) if !article.text_content.trim().is_empty() => (article.title, article.content.to_string()),
        // No article found: the whole page, without its chrome.
        _ => (String::new(), html.to_owned()),
    };
    let converter =
        htmd::HtmlToMarkdown::builder().skip_tags(vec!["script", "style", "noscript", "nav", "header", "footer", "aside", "head"]).build();
    let markdown = converter.convert(&content).unwrap_or_default();
    let title = if title.trim().is_empty() { page_title(html) } else { title.trim().to_owned() };
    (title, absolute_links(url, &markdown))
}

fn page_title(html: &str) -> String {
    let page = scraper::Html::parse_document(html);
    let Ok(selector) = scraper::Selector::parse("title") else { return String::new() };
    page.select(&selector).next().map(|title| title.text().collect::<String>().trim().to_owned()).unwrap_or_default()
}

static LINK: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\]\(([^)\s]+)").unwrap_or_else(|error| panic!("{error}")));

/// `[text](/docs)` as `[text](https://site/docs)`.
fn absolute_links(url: &str, markdown: &str) -> String {
    let Ok(base) = primp::Url::parse(url) else { return markdown.to_owned() };
    LINK.replace_all(markdown, |captures: &regex::Captures| {
        let target = &captures[1];
        match base.join(target) {
            Ok(joined) if !target.starts_with('#') => format!("]({joined}"),
            _ => format!("]({target}"),
        }
    })
    .into_owned()
}

/// A PDF's text.
pub(crate) fn pdf(bytes: &[u8]) -> Result<String, String> {
    pdf_extract::extract_text_from_mem(bytes).map_err(|error| format!("can't read the PDF: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    const ARTICLE: &str = r#"<html><head><title>Write-Ahead Logging</title><script>var x = 1;</script></head><body>
<nav><a href="/">Home</a> <a href="/docs">Docs</a></nav>
<article><h1>Write-Ahead Logging</h1>
<p>The default method by which SQLite implements atomic commit and rollback is a rollback journal.
Beginning with version 3.7.0, a new "Write-Ahead Log" option (hereafter referred to as "WAL") is available.</p>
<p>There are advantages and disadvantages to using WAL instead of a rollback journal. See <a href="/lockingv3.html">locking</a>
and <a href="https://example.com/x">elsewhere</a>.</p>
<h2>Checkpointing</h2>
<p>Because the WAL can grow without bound in some circumstances, a checkpoint moves its content back into the database file.
Checkpointing happens automatically when the WAL reaches a threshold of 1000 pages.</p>
</article><footer>Copyright</footer></body></html>"#;

    #[test]
    fn a_page_is_its_content_as_markdown_with_absolute_links() {
        let (title, markdown) = html("https://sqlite.org/wal.html", ARTICLE);
        assert_eq!(title, "Write-Ahead Logging");
        assert!(markdown.contains("[locking](https://sqlite.org/lockingv3.html)"), "{markdown}");
        assert!(markdown.contains("[elsewhere](https://example.com/x)"), "{markdown}");
        assert!(markdown.contains("Checkpointing"), "{markdown}");
        assert!(!markdown.contains("var x") && !markdown.contains("Copyright"), "{markdown}");
    }

    #[test]
    fn slices_say_which_lines_and_where_to_go_on() {
        let text: String = (1..=3000).map(|n| format!("line number {n} of a long page\n")).collect();
        let page = Page::new("https://x.org/long", "Long".into(), &text);
        let first = page.slice(1);
        let heading = first.lines().next().expect("heading");
        assert!(heading.starts_with("https://x.org/long — \"Long\" — lines 1–"), "{heading}");
        assert!(heading.ends_with("of 3000. Continue with offset=") || heading.contains("Continue with offset="), "{heading}");
        assert!(first.len() < 22_000 && first.contains("\n1: line number 1 of"), "{}", first.len());
        let last = page.slice(2990);
        assert!(last.starts_with("https://x.org/long — \"Long\" — lines 2990–3000 of 3000.\n"), "{last}");
        assert!(page.slice(99_999).contains("lines 3000–3000 of 3000."));
    }

    #[test]
    fn find_shows_matches_with_their_context() {
        let text = "alpha\nbeta\ngamma\ndelta\nepsilon\nzeta\neta\ntheta\niota\nkappa";
        let page = Page::new("https://x.org", String::new(), text);
        let found = page.find("Gamma|iota");
        assert_eq!(
            found,
            "https://x.org — 2 lines match \"Gamma|iota\":\n\n1: alpha\n2: beta\n3: gamma\n4: delta\n5: epsilon\n\n7: eta\n8: theta\n9: iota\n10: kappa\n"
        );
        assert_eq!(page.find("omega"), "https://x.org — nothing matches \"omega\" in its 10 lines.");
    }
}
