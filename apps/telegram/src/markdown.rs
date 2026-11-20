//! The Markdown agents write, as the HTML Telegram shows: bold, italic,
//! strikethrough, code, code blocks, links, quotes; headings in bold and
//! list bullets as "•". Anything else stays as written, escaped.

/// [markdown] as Telegram's HTML (parse_mode "HTML").
pub fn to_telegram_html(markdown: &str) -> String {
    let mut out = String::new();
    let mut lines = markdown.lines().peekable();
    let mut quote: Vec<String> = Vec::new();
    let flush_quote = |out: &mut String, quote: &mut Vec<String>| {
        if !quote.is_empty() {
            out.push_str("<blockquote>");
            out.push_str(&quote.join("\n"));
            out.push_str("</blockquote>\n");
            quote.clear();
        }
    };
    while let Some(line) = lines.next() {
        let trimmed = line.trim_start();
        if let Some(fence) = trimmed.strip_prefix("```") {
            flush_quote(&mut out, &mut quote);
            let language = fence.trim();
            let mut code = Vec::new();
            for line in lines.by_ref() {
                if line.trim_start().starts_with("```") {
                    break;
                }
                code.push(line);
            }
            let body = escape(&code.join("\n"));
            if language.is_empty() || !language.chars().all(|c| c.is_ascii_alphanumeric() || "+-_#.".contains(c)) {
                out.push_str(&format!("<pre>{body}</pre>\n"));
            } else {
                out.push_str(&format!("<pre><code class=\"language-{language}\">{body}</code></pre>\n"));
            }
            continue;
        }
        if let Some(quoted) = trimmed.strip_prefix('>') {
            quote.push(inline(quoted.strip_prefix(' ').unwrap_or(quoted)));
            continue;
        }
        flush_quote(&mut out, &mut quote);
        let hashes = trimmed.chars().take_while(|c| *c == '#').count();
        if (1..=6).contains(&hashes) && trimmed[hashes..].starts_with(' ') {
            out.push_str(&format!("<b>{}</b>\n", inline(trimmed[hashes..].trim())));
            continue;
        }
        let indent = &line[..line.len() - trimmed.len()];
        if let Some(item) = trimmed.strip_prefix("- ").or_else(|| trimmed.strip_prefix("* ")).or_else(|| trimmed.strip_prefix("+ ")) {
            out.push_str(&format!("{indent}• {}\n", inline(item)));
            continue;
        }
        if trimmed == "---" || trimmed == "***" || trimmed == "___" {
            out.push_str("———\n");
            continue;
        }
        out.push_str(&inline(line));
        out.push('\n');
    }
    flush_quote(&mut out, &mut quote);
    out.trim_end().to_owned()
}

/// One line's inline Markdown.
fn inline(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        let rest = &chars[i..];
        if c == '\\' && i + 1 < chars.len() && chars[i + 1].is_ascii_punctuation() {
            out.push_str(&escape(&chars[i + 1].to_string()));
            i += 2;
            continue;
        }
        if c == '`'
            && let Some(end) = find(&chars, i + 1, "`")
        {
            out.push_str(&format!("<code>{}</code>", escape(&collect(&chars[i + 1..end]))));
            i = end + 1;
            continue;
        }
        let mut paired = false;
        for (marker, tag) in [("**", "b"), ("__", "b"), ("~~", "s")] {
            if starts(rest, marker)
                && let Some(end) = find(&chars, i + 2, marker).filter(|end| *end > i + 2)
            {
                out.push_str(&format!("<{tag}>{}</{tag}>", inline(&collect(&chars[i + 2..end]))));
                i = end + 2;
                paired = true;
                break;
            }
        }
        if paired {
            continue;
        }
        if (c == '*' || c == '_')
            && opens(&chars, i)
            && let Some(end) = closing(&chars, i + 1, c)
        {
            out.push_str(&format!("<i>{}</i>", inline(&collect(&chars[i + 1..end]))));
            i = end + 1;
            continue;
        }
        if c == '['
            && let Some(close) = find(&chars, i + 1, "](")
            && let Some(end) = find(&chars, close + 2, ")")
        {
            let label = collect(&chars[i + 1..close]);
            let url = collect(&chars[close + 2..end]);
            if url.starts_with("http://") || url.starts_with("https://") || url.starts_with("tg://") {
                out.push_str(&format!("<a href=\"{}\">{}</a>", escape(&url).replace('"', "&quot;"), inline(&label)));
                i = end + 1;
                continue;
            }
        }
        out.push_str(&escape(&c.to_string()));
        i += 1;
    }
    out
}

/// Whether a `*` or `_` at [at] can open emphasis: not followed by a space,
/// and for `_`, not inside a word (snake_case stays as it is).
fn opens(chars: &[char], at: usize) -> bool {
    let next = chars.get(at + 1);
    if next.is_none_or(|next| next.is_whitespace()) {
        return false;
    }
    chars[at] != '_' || at == 0 || !chars[at - 1].is_alphanumeric()
}

/// Where emphasis opened with [marker] closes, from [from].
fn closing(chars: &[char], from: usize, marker: char) -> Option<usize> {
    (from + 1..chars.len()).find(|&at| {
        chars[at] == marker
            && !chars[at - 1].is_whitespace()
            && (marker != '_' || chars.get(at + 1).is_none_or(|next| !next.is_alphanumeric()))
            && chars.get(at + 1) != Some(&marker)
    })
}

fn starts(chars: &[char], marker: &str) -> bool {
    let marker: Vec<char> = marker.chars().collect();
    chars.len() >= marker.len() && chars[..marker.len()] == marker[..]
}

fn find(chars: &[char], from: usize, marker: &str) -> Option<usize> {
    (from..chars.len()).find(|&at| starts(&chars[at..], marker))
}

fn collect(chars: &[char]) -> String {
    chars.iter().collect()
}

/// Telegram's HTML escaping: &, < and >.
pub fn escape(text: &str) -> String {
    text.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}
