//! Formats: what an application message carries,
//! once decrypted, and its padding.
//!
//! A content is a JSON object: `{v, kind, client_message_id, text,
//! by_agent?, …, padding}`. `v` is 1. Kinds other than a message's (a draft,
//! a read position, a deletion, a proposal) carry fields of their own, kept
//! in [`Content::extra`] so a reader passes on what it doesn't know.
//!
//! Padding brings the whole JSON up to a bucket (256 B, 1 KiB, 4 KiB, 16 KiB
//! or [`MOST`]), so a message's size tells the service only its bucket. The
//! padding is spaces in a `padding` field, so the size comes out exact.
//!
//! A message may say when its sender sent it: `sent_ms`, milliseconds since
//! 1970 by the sender's clock ([`Content::sent_ms`]). It's the sender's claim,
//! authenticated as theirs but not checked against anything: shown, never
//! used to order (the home's time it took a message does that). Kept in
//! [`Content::extra`], so a reader from before it passes it on as it came.
//!
//! Text is at most 16 KiB of UTF-8, checked by the sender and every
//! receiver. Escaped, 16 KiB of text can be more than [`MOST`] (JSON writes a
//! newline or a quote in two bytes); such a text is refused as too long.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::{Error, Result};

/// The content's version.
pub const VERSION: u32 = 1;
/// The most text, in bytes of UTF-8.
pub const MAX_TEXT: usize = 16 * 1024;
/// The most a content may be, padding included: what fits the worker's
/// 20 KiB cap on application messages ([`crate::envelope::APPLICATION_MOST`])
/// with room for MLS's framing, sender data, signature and AEAD tag.
pub const MOST: usize = crate::envelope::APPLICATION_MOST - 512;
/// The latest `sent_ms` taken: the end of the year 9999.
pub const LATEST_SENT_MS: i64 = 253_402_300_799_999;
/// The sizes a content is padded to.
pub const BUCKETS: [usize; 5] = [256, 1024, 4096, 16 * 1024, MOST];

/// The kinds of content.
pub mod kind {
    /// A message in a conversation.
    pub const TEXT: &str = "text";
    /// A draft, in the self group.
    pub const DRAFT: &str = "draft";
    /// A read position, in the self group.
    pub const READ: &str = "read";
    /// A deletion, in the self group.
    pub const DELETE: &str = "delete";
    /// An agent's proposed message, in a Claktory channel.
    pub const PROPOSAL: &str = "proposal";
    /// An agent's suggested draft for a conversation, in a Claktory channel
    /// (`draft_id` and the conversation in `extra`).
    pub const AGENT_DRAFT: &str = "agent_draft";
    /// The person's pins for their friends, from their devices to a
    /// Claktory, in its channel (a Claktory pins only what they give it).
    pub const PINS: &str = "pins";
    /// A new device asking the person's device that added it to send its
    /// history again (E8, "Try again"), in the self group: the asking
    /// device is the sender's leaf, and nothing else is in it.
    pub const HISTORY_AGAIN: &str = "history_again";
}

/// Which agent wrote a message the person approved: signed by the
/// person's device, inside the encryption.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ByAgent {
    pub claktory_id: String,
    pub agent_id: String,
    pub incarnation: String,
    pub name: String,
}

/// An application message's content.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Content {
    pub v: u32,
    pub kind: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub client_message_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub by_agent: Option<ByAgent>,
    /// The kind's other fields, kept as they came.
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

#[derive(Serialize)]
struct Padded<'a> {
    #[serde(flatten)]
    content: &'a Content,
    padding: &'a str,
}

impl Content {
    /// A message's content: its retry id and text.
    pub fn text(client_message_id: &str, text: &str) -> Content {
        Content {
            v: VERSION,
            kind: kind::TEXT.to_owned(),
            client_message_id: client_message_id.to_owned(),
            text: Some(text.to_owned()),
            by_agent: None,
            extra: Map::new(),
        }
    }

    /// When the sender says it sent this, in milliseconds since 1970; None
    /// when it doesn't say.
    pub fn sent_ms(&self) -> Option<i64> {
        self.extra.get(SENT_MS).and_then(sent_ms)
    }

    /// Says it was sent at `ms` (milliseconds since 1970, the sender's
    /// clock).
    pub fn set_sent_ms(&mut self, ms: i64) {
        self.extra.insert(SENT_MS.to_owned(), Value::from(ms));
    }

    fn check(&self) -> Result<()> {
        if self.v != VERSION {
            return Err(Error::Format(format!("content version {} isn't one this reads (it reads {VERSION})", self.v)));
        }
        if self.kind.is_empty() {
            return Err(Error::Format("a content has a kind".to_owned()));
        }
        if self.extra.contains_key("padding") {
            return Err(Error::Format("padding is the padding's own field".to_owned()));
        }
        if self.extra.get(SENT_MS).is_some_and(|value| sent_ms(value).is_none()) {
            return Err(Error::Format(format!("sent_ms is a time from 1 to {LATEST_SENT_MS} ms")));
        }
        if let Some(text) = &self.text
            && text.len() > MAX_TEXT
        {
            return Err(Error::TooBig { kind: "a message's text", most: MAX_TEXT });
        }
        Ok(())
    }

    /// The content as sent: checked, then padded to its bucket.
    pub fn encode(&self) -> Result<Vec<u8>> {
        self.check()?;
        let bare = serde_json::to_vec(&Padded { content: self, padding: "" }).map_err(|error| Error::Format(error.to_string()))?;
        let bucket = BUCKETS
            .into_iter()
            .find(|&bucket| bucket >= bare.len())
            .ok_or(Error::TooBig { kind: "a message, once encoded", most: MOST })?;
        let padding = " ".repeat(bucket - bare.len());
        let padded = serde_json::to_vec(&Padded { content: self, padding: &padding }).map_err(|error| Error::Format(error.to_string()))?;
        debug_assert_eq!(padded.len(), bucket);
        Ok(padded)
    }

    /// A content as received, checked as the sender had to: at most
    /// [`MOST`] bytes, a version this reads, text within 16 KiB. The padding
    /// is dropped.
    pub fn decode(bytes: &[u8]) -> Result<Content> {
        if bytes.len() > MOST {
            return Err(Error::TooBig { kind: "a content", most: MOST });
        }
        let mut object: Map<String, Value> =
            serde_json::from_slice(bytes).map_err(|_| Error::Format("a content is a JSON object".to_owned()))?;
        match object.remove("padding") {
            None | Some(Value::String(_)) => {}
            Some(_) => return Err(Error::Format("padding is a string".to_owned())),
        }
        // A time that isn't one is the sender's mistake, not a reason to lose
        // the message: it reads as saying no time.
        if object.get(SENT_MS).is_some_and(|value| sent_ms(value).is_none()) {
            object.remove(SENT_MS);
        }
        let content: Content =
            serde_json::from_value(Value::Object(object)).map_err(|error| Error::Format(format!("not a content: {error}")))?;
        content.check()?;
        Ok(content)
    }
}

/// The field a content's sent time is in.
const SENT_MS: &str = "sent_ms";

/// A sent time as JSON gives it, if it's one.
fn sent_ms(value: &Value) -> Option<i64> {
    value.as_i64().filter(|ms| (1..=LATEST_SENT_MS).contains(ms))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn contents_pad_to_their_bucket() {
        for (size, bucket) in [(0, 256), (150, 256), (300, 1024), (2000, 4096), (10_000, 16 * 1024), (MAX_TEXT, MOST)] {
            let content = Content::text("abcdefghijklmnopqrstuv", &"a".repeat(size));
            let bytes = content.encode().unwrap();
            assert_eq!(bytes.len(), bucket, "{size} bytes of text");
            assert_eq!(Content::decode(&bytes).unwrap(), content);
        }
    }

    #[test]
    fn text_is_held_to_16_kib_both_ways() {
        let long = Content::text("abcdefghijklmnopqrstuv", &"a".repeat(MAX_TEXT + 1));
        assert!(matches!(long.encode(), Err(Error::TooBig { .. })));
        // A sender that skipped the check is refused by the receiver.
        let bytes = serde_json::to_vec(&long).unwrap();
        assert!(matches!(Content::decode(&bytes), Err(Error::TooBig { .. })));
        // So is text that escapes past the most a content may be.
        let quotes = Content::text("abcdefghijklmnopqrstuv", &"\"".repeat(MAX_TEXT));
        assert!(matches!(quotes.encode(), Err(Error::TooBig { .. })));
    }

    #[test]
    fn unknown_fields_pass_through_and_versions_are_checked() {
        let bytes = br#"{"v":1,"kind":"read","seq":42,"padding":"   "}"#;
        let content = Content::decode(bytes).unwrap();
        assert_eq!(content.kind, kind::READ);
        assert_eq!(content.extra.get("seq"), Some(&Value::from(42)));
        let again = Content::decode(&content.encode().unwrap()).unwrap();
        assert_eq!(again, content);

        assert!(Content::decode(br#"{"v":2,"kind":"text"}"#).is_err());
        assert!(Content::decode(br#"{"v":1,"kind":"text","padding":7}"#).is_err());
        assert!(Content::decode(b"[1]").is_err());
    }

    #[test]
    fn a_sent_time_rides_inside_and_older_contents_say_none() {
        let mut content = Content::text("abcdefghijklmnopqrstuv", "hello");
        assert_eq!(content.sent_ms(), None);
        assert_eq!(Content::decode(&content.encode().unwrap()).unwrap().sent_ms(), None);
        content.set_sent_ms(1_795_672_200_000);
        let decoded = Content::decode(&content.encode().unwrap()).unwrap();
        assert_eq!(decoded.sent_ms(), Some(1_795_672_200_000));
        assert_eq!(decoded, content);

        // From a reader before it: kept as it came, and passed on.
        let old = br#"{"v":1,"kind":"text","client_message_id":"abcdefghijklmnopqrstuv","text":"hi","sent_ms":5,"padding":""}"#;
        assert_eq!(Content::decode(old).unwrap().sent_ms(), Some(5));

        // Not a time: the message still reads, as saying none.
        for bad in [r#""5""#, "0", "-1", "1.5", "253402300800000", "null", "[]"] {
            let bytes = format!(r#"{{"v":1,"kind":"text","client_message_id":"abcdefghijklmnopqrstuv","text":"hi","sent_ms":{bad}}}"#);
            let decoded = Content::decode(bytes.as_bytes()).unwrap_or_else(|error| panic!("{bad}: {error}"));
            assert_eq!((decoded.sent_ms(), decoded.extra.contains_key("sent_ms")), (None, false), "{bad}");
        }
        // And a sender never writes one.
        content.extra.insert("sent_ms".into(), Value::from("soon"));
        assert!(content.encode().is_err());
    }
}
