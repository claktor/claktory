//! The review hash (B2b/B3b): SHA-256, in lowercase hex, over the canonical
//! JSON of what the person is asked to allow, bound to the exact command
//! the helper relayed. The extension computes the same with JSON.stringify,
//! so canonical means what JavaScript writes: object keys sorted by UTF-16
//! code units at every level, no whitespace, strings escaped as
//! JSON.stringify escapes them (`\"`, `\\`, `\b \f \n \r \t`, other controls
//! as lowercase `\u00xx`, everything else as it is), integers as digits.
//! A float anywhere has no canonical form here, and is refused.

use std::fmt::Write as _;

use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};

/// `value` as canonical JSON; None when it holds a number that isn't an
/// integer.
pub fn canonical(value: &Value) -> Option<String> {
    let mut out = String::new();
    write(value, &mut out)?;
    Some(out)
}

fn write(value: &Value, out: &mut String) -> Option<()> {
    match value {
        Value::Null => out.push_str("null"),
        Value::Bool(flag) => out.push_str(if *flag { "true" } else { "false" }),
        Value::Number(number) => {
            if let Some(n) = number.as_u64() {
                let _ = write!(out, "{n}");
            } else {
                let _ = write!(out, "{}", number.as_i64()?);
            }
        }
        Value::String(text) => string(text, out),
        Value::Array(items) => {
            out.push('[');
            for (at, item) in items.iter().enumerate() {
                if at > 0 {
                    out.push(',');
                }
                write(item, out)?;
            }
            out.push(']');
        }
        Value::Object(object) => {
            let mut keys: Vec<&String> = object.keys().collect();
            keys.sort_by(|a, b| a.encode_utf16().cmp(b.encode_utf16()));
            out.push('{');
            for (at, key) in keys.into_iter().enumerate() {
                if at > 0 {
                    out.push(',');
                }
                string(key, out);
                out.push(':');
                write(&object[key], out)?;
            }
            out.push('}');
        }
    }
    Some(())
}

fn string(text: &str, out: &mut String) {
    out.push('"');
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

/// The hash a review of `op` (command `id`, `sequence` of `session`) must
/// carry for `action`: None when there's no canonical form.
pub fn review_hash(session: &str, id: u64, sequence: u64, op: &Map<String, Value>, action: &Value) -> Option<String> {
    let bound = json!({
        "action": action,
        "command": id,
        "document": op.get("document").cloned().unwrap_or(Value::Null),
        "op": op,
        "sequence": sequence,
        "session": session,
        "target": op.get("target").cloned().unwrap_or(Value::Null),
    });
    let digest = Sha256::digest(canonical(&bound)?.as_bytes());
    Some(digest.iter().map(|byte| format!("{byte:02x}")).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_json_is_what_javascript_writes_with_sorted_keys() {
        let value = json!({"b": [1, -2, true, null], "a": {"z": "x", "é": "y", "Z": "q\"\\\n\t\u{1}\u{7f}é⟦U+202E⟧\u{2028}"}});
        assert_eq!(
            canonical(&value).unwrap(),
            r#"{"a":{"Z":"q\"\\\n\t\u0001"#.to_owned() + "\u{7f}é⟦U+202E⟧\u{2028}" + r#"","z":"x","é":"y"},"b":[1,-2,true,null]}"#
        );
        // UTF-16 order, as JavaScript sorts: U+FF61 before U+1F600 (a surrogate pair, 0xD83D...).
        let order = json!({"😀": 1, "｡": 2});
        assert_eq!(canonical(&order).unwrap(), "{\"😀\":1,\"｡\":2}");
        assert!(canonical(&json!({"x": 1.5})).is_none(), "no floats");
    }

    /// The vector the extension's test matches (browser-extension/PROTOCOL.md).
    #[test]
    fn the_review_hash_binds_the_action_to_the_command() {
        let op = json!({"op": "fill", "document": "D1", "revision": 3, "target": "r-9f2c", "text": "Hello\nthere"});
        let Value::Object(op) = op else { unreachable!() };
        let action = json!({
            "op": "fill", "origin": "https://example.com", "path": "/compose", "role": "textbox", "name": "Message",
            "text": "Hello⏎there", "warnings": ["may_send_as_typed"]
        });
        let hash = review_hash("s-1", 7, 12, &op, &action).unwrap();
        // Node: the same object, keys sorted, through JSON.stringify and sha256.
        assert_eq!(hash, "d86d019b1243ac926d61234d61c8ce8ebaaffbfbe9286c6a6566c38ab32fc098");
        assert_ne!(hash, review_hash("s-1", 8, 12, &op, &action).unwrap(), "another command");
        assert_ne!(hash, review_hash("s-2", 7, 12, &op, &action).unwrap(), "another session");
        let mut other = op.clone();
        other.insert("text".into(), json!("Goodbye"));
        assert_ne!(hash, review_hash("s-1", 7, 12, &other, &action).unwrap(), "other text, the same words shown");
    }
}
