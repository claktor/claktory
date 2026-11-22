//! Named parameters in a panel's SQL: `$from`, `$to`, `$step` and the
//! dashboard's variables (`$agent`) are bound as values, never spliced into
//! the statement. The host binds a JSON list to `?1`, `?2`…, so each name
//! becomes the `?N` of its value; text inside quotes and comments is left
//! alone.

use std::collections::BTreeMap;

use serde_json::{Value, json};

/// The names every query can use: the time range in milliseconds and in
/// seconds, now, and a bucket that suits the range.
pub const BUILTINS: [&str; 7] = ["from", "to", "from_s", "to_s", "now", "step", "step_s"];

/// The longest statement a panel or variable may have.
pub const MAX_SQL: usize = 20_000;

/// The buckets `$step` picks from: the smallest that makes at most
/// [`MAX_BUCKETS`] of the range.
const STEPS_S: [i64; 10] = [60, 300, 900, 1800, 3600, 3 * 3600, 6 * 3600, 12 * 3600, 86_400, 7 * 86_400];
const MAX_BUCKETS: i64 = 100;

/// A statement as the host takes it: `?N` in place of each name, and the
/// values, in order.
#[derive(Debug, Clone, PartialEq)]
pub struct Bound {
    pub sql: String,
    pub params: Vec<Value>,
}

/// One pass over `sql`: each `$name` outside quotes and comments, with
/// where it is (the byte range of `$name`).
fn scan(sql: &str) -> Result<Vec<(usize, usize, String)>, String> {
    let bytes = sql.as_bytes();
    let mut found = Vec::new();
    let mut at = 0;
    while at < bytes.len() {
        match bytes[at] {
            quote @ (b'\'' | b'"' | b'`') => {
                // To the closing quote; a doubled one is part of the text.
                at += 1;
                loop {
                    match bytes.get(at) {
                        None => break,
                        Some(&b) if b == quote && bytes.get(at + 1) == Some(&quote) => at += 2,
                        Some(&b) if b == quote => {
                            at += 1;
                            break;
                        }
                        Some(_) => at += 1,
                    }
                }
            }
            b'[' => {
                at = sql[at..].find(']').map_or(bytes.len(), |end| at + end + 1);
            }
            b'-' if bytes.get(at + 1) == Some(&b'-') => {
                at = sql[at..].find('\n').map_or(bytes.len(), |end| at + end + 1);
            }
            b'/' if bytes.get(at + 1) == Some(&b'*') => {
                at = sql[at + 2..].find("*/").map_or(bytes.len(), |end| at + 2 + end + 2);
            }
            b'?' => return Err("use $names for values (such as $from or $agent), not ?".into()),
            b'$' => {
                let start = at;
                at += 1;
                while bytes.get(at).is_some_and(|b| b.is_ascii_alphanumeric() || *b == b'_') {
                    at += 1;
                }
                let name = &sql[start + 1..at];
                if name.is_empty() || name.as_bytes()[0].is_ascii_digit() {
                    return Err(format!("\"{}\" isn't a name: $ is followed by a letter", &sql[start..at.max(start + 1)]));
                }
                found.push((start, at, name.to_owned()));
            }
            _ => at += 1,
        }
    }
    Ok(found)
}

/// The names `sql` uses, each once, in order.
pub fn names(sql: &str) -> Result<Vec<String>, String> {
    let mut names: Vec<String> = Vec::new();
    for (_, _, name) in scan(sql)? {
        if !names.contains(&name) {
            names.push(name);
        }
    }
    Ok(names)
}

/// Checks a statement: there, not too long, and using only `known` names.
pub fn check(sql: &str, known: &[&str]) -> Result<(), String> {
    if sql.trim().is_empty() {
        return Err("the query is empty".into());
    }
    if sql.len() > MAX_SQL {
        return Err(format!("the query is {} bytes; at most {MAX_SQL}", sql.len()));
    }
    for name in names(sql)? {
        if !known.contains(&name.as_str()) {
            return Err(format!("${name} isn't a variable of this dashboard or one of ${}", BUILTINS.join(", $")));
        }
    }
    Ok(())
}

/// `sql` with its names bound to `values`.
pub fn bind(sql: &str, values: &BTreeMap<String, Value>) -> Result<Bound, String> {
    let mut out = String::with_capacity(sql.len());
    let mut order: Vec<String> = Vec::new();
    let mut last = 0;
    for (start, end, name) in scan(sql)? {
        let Some(_) = values.get(&name) else { return Err(format!("${name} has no value")) };
        let index = match order.iter().position(|known| *known == name) {
            Some(index) => index,
            None => {
                order.push(name.clone());
                order.len() - 1
            }
        };
        out.push_str(&sql[last..start]);
        out.push_str(&format!("?{}", index + 1));
        last = end;
    }
    out.push_str(&sql[last..]);
    Ok(Bound { sql: out, params: order.iter().map(|name| values[name].clone()).collect() })
}

/// A time as a dashboard writes it: "now", "now-15m" (m, h, d or w), or
/// milliseconds since 1970.
pub fn time(text: &str, now: i64) -> Option<i64> {
    let text = text.trim();
    if let Ok(ms) = text.parse::<i64>() {
        return (ms >= 0).then_some(ms);
    }
    let rest = text.strip_prefix("now")?;
    if rest.is_empty() {
        return Some(now);
    }
    let ago = rest.strip_prefix('-')?;
    Some(now - duration(ago)?)
}

/// "15m", "6h", "7d" or "2w", in milliseconds.
pub fn duration(text: &str) -> Option<i64> {
    let (count, unit) = text.split_at(text.len().checked_sub(1)?);
    let count: i64 = count.parse().ok().filter(|count| *count > 0 && *count <= 100_000)?;
    let unit = match unit {
        "m" => 60_000,
        "h" => 3_600_000,
        "d" => 86_400_000,
        "w" => 7 * 86_400_000,
        _ => return None,
    };
    Some(count * unit)
}

/// The bucket, in seconds, for a range of `span_ms`.
pub fn step_s(span_ms: i64) -> i64 {
    let span_s = (span_ms / 1000).max(1);
    STEPS_S.into_iter().find(|step| span_s / step <= MAX_BUCKETS).unwrap_or(STEPS_S[STEPS_S.len() - 1])
}

/// The built-in names' values for a range.
pub fn builtins(from: i64, to: i64, now: i64) -> BTreeMap<String, Value> {
    let step = step_s(to - from);
    BTreeMap::from([
        ("from".to_owned(), json!(from)),
        ("to".to_owned(), json!(to)),
        ("from_s".to_owned(), json!(from / 1000)),
        ("to_s".to_owned(), json!(to / 1000)),
        ("now".to_owned(), json!(now)),
        ("step".to_owned(), json!(step * 1000)),
        ("step_s".to_owned(), json!(step)),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_bound_in_order_and_quotes_and_comments_are_left_alone() {
        let values = BTreeMap::from([("from".to_owned(), json!(5)), ("agent".to_owned(), json!("rivet"))]);
        let sql = "SELECT '$agent', \"$x\", [$y] -- $z\n FROM t /* $w */ WHERE a >= $from AND ($agent = '' OR agent = $agent) AND b = 'it''s $no'";
        let bound = bind(sql, &values).unwrap();
        assert_eq!(
            bound.sql,
            "SELECT '$agent', \"$x\", [$y] -- $z\n FROM t /* $w */ WHERE a >= ?1 AND (?2 = '' OR agent = ?2) AND b = 'it''s $no'"
        );
        assert_eq!(bound.params, vec![json!(5), json!("rivet")]);
        assert_eq!(names(sql).unwrap(), vec!["from", "agent"]);
    }

    #[test]
    fn what_cant_be_bound_is_refused_with_why() {
        assert!(bind("SELECT ?1", &BTreeMap::new()).unwrap_err().contains("not ?"));
        assert!(bind("SELECT $gone", &BTreeMap::new()).unwrap_err().contains("$gone has no value"));
        assert!(names("SELECT $1").unwrap_err().contains("isn't a name"));
        assert!(check("  ", &BUILTINS).unwrap_err().contains("empty"));
        assert!(check("SELECT $agent", &BUILTINS).unwrap_err().contains("$agent isn't a variable"));
        assert!(check("SELECT $agent, $from", &["agent", "from"]).is_ok());
        assert!(check(&"x".repeat(MAX_SQL + 1), &BUILTINS).unwrap_err().contains("at most"));
    }

    #[test]
    fn times_and_steps() {
        let now = 1_000_000_000_000;
        assert_eq!(time("now", now), Some(now));
        assert_eq!(time("now-15m", now), Some(now - 900_000));
        assert_eq!(time("now-7d", now), Some(now - 7 * 86_400_000));
        assert_eq!(time("now-2w", now), Some(now - 14 * 86_400_000));
        assert_eq!(time("12345", now), Some(12345));
        for wrong in ["now-", "now-7", "now-7y", "now+1d", "yesterday", "now-0d", "-5"] {
            assert_eq!(time(wrong, now), None, "{wrong}");
        }
        assert_eq!(step_s(3_600_000), 60);
        assert_eq!(step_s(86_400_000), 900);
        assert_eq!(step_s(7 * 86_400_000), 3 * 3600);
        assert_eq!(step_s(30 * 86_400_000), 12 * 3600);
        assert_eq!(step_s(90 * 86_400_000), 86_400);
        assert_eq!(step_s(10 * 365 * 86_400_000), 7 * 86_400);
        let values = builtins(now - 3_600_000, now, now);
        assert_eq!((values["from_s"].clone(), values["step"].clone()), (json!((now - 3_600_000) / 1000), json!(60_000)));
    }
}
