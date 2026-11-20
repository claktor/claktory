//! Reading a pasted .env file: `KEY=value` lines, with `export` in front or
//! not, values bare, in single quotes (as they are) or double quotes (with
//! \n, \t, \" and \\), quoted values over several lines, and # comments.
//! Warnings never quote a value, or a line that might hold one.

use zeroize::Zeroizing;

use super::{SHORT, Value, valid_variable};

pub(crate) struct Parsed {
    /// In the order they appear; a variable set twice keeps its last value.
    pub values: Vec<Value>,
    pub warnings: Vec<String>,
}

pub(crate) fn parse(text: &str) -> Parsed {
    let mut values: Vec<Value> = Vec::new();
    let mut warnings = Vec::new();
    let lines: Vec<&str> = text.lines().collect();
    let mut at = 0;
    while at < lines.len() {
        let number = at + 1;
        let line = lines[at].trim();
        at += 1;
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let line = line.strip_prefix("export ").map_or(line, str::trim_start);
        let Some((key, rest)) = line.split_once('=') else {
            warnings.push(format!("Line {number} isn't KEY=value, so it was left out."));
            continue;
        };
        let key = key.trim();
        if !valid_variable(key) {
            warnings.push(format!("Line {number}'s name isn't a variable's name, so it was left out."));
            continue;
        }
        let rest = rest.trim_start();
        let value = match rest.chars().next() {
            Some(quote @ ('"' | '\'')) => {
                // The value runs to the closing quote, maybe on a later line.
                let mut raw = String::from(&rest[1..]);
                let mut closed = closing(&raw, quote);
                while closed.is_none() && at < lines.len() {
                    raw.push('\n');
                    raw.push_str(lines[at]);
                    at += 1;
                    closed = closing(&raw, quote);
                }
                let Some(end) = closed else {
                    warnings.push(format!("Line {number}'s value has no closing quote, so it was left out."));
                    continue;
                };
                raw.truncate(end);
                if quote == '"' { unescape(&raw) } else { raw }
            }
            _ => {
                // A bare value ends at a comment.
                let end = rest.find(" #").unwrap_or(rest.len());
                rest[..end].trim_end().to_owned()
            }
        };
        let value = Zeroizing::new(value);
        if value.is_empty() {
            warnings.push(format!("{key} is empty."));
        } else if value.len() < SHORT {
            warnings.push(format!("{key} is too short to hide in agents' output."));
        }
        if let Some(earlier) = values.iter_mut().find(|earlier| earlier.variable == key) {
            warnings.push(format!("{key} is set twice; the last one is kept."));
            earlier.value = value;
        } else {
            values.push(Value { variable: key.to_owned(), value, file: false });
        }
    }
    Parsed { values, warnings }
}

/// Where the closing `quote` is, skipping escaped ones in double quotes.
fn closing(raw: &str, quote: char) -> Option<usize> {
    let mut escaped = false;
    for (index, c) in raw.char_indices() {
        if escaped {
            escaped = false;
        } else if c == '\\' && quote == '"' {
            escaped = true;
        } else if c == quote {
            return Some(index);
        }
    }
    None
}

fn unescape(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut chars = raw.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('n') => out.push('\n'),
            Some('t') => out.push('\t'),
            Some('r') => out.push('\r'),
            Some(other) => out.push(other),
            None => out.push('\\'),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_dotenv_file_reads_as_its_variables() {
        let text = "# Cloudflare\nexport CLOUDFLARE_API_TOKEN=abcdefghijklmnop # the token\nZONE='claktory.com'\nKEY=\"-----BEGIN KEY-----\nline two\n-----END KEY-----\"\nESCAPED=\"a\\nb \\\"quoted\\\"\"\nPIN=1234\nEMPTY=\nnot a line\n1BAD=x\nZONE=claktory.dev\nOPEN=\"never closed\n";
        let parsed = parse(text);
        let values: Vec<(&str, &str)> = parsed.values.iter().map(|value| (value.variable.as_str(), value.value.as_str())).collect();
        assert_eq!(
            values,
            [
                ("CLOUDFLARE_API_TOKEN", "abcdefghijklmnop"),
                ("ZONE", "claktory.dev"),
                ("KEY", "-----BEGIN KEY-----\nline two\n-----END KEY-----"),
                ("ESCAPED", "a\nb \"quoted\""),
                ("PIN", "1234"),
                ("EMPTY", ""),
            ]
        );
        assert_eq!(
            parsed.warnings,
            [
                "PIN is too short to hide in agents' output.",
                "EMPTY is empty.",
                "Line 10 isn't KEY=value, so it was left out.",
                "Line 11's name isn't a variable's name, so it was left out.",
                "ZONE is set twice; the last one is kept.",
                "Line 13's value has no closing quote, so it was left out.",
            ]
        );
    }
}
