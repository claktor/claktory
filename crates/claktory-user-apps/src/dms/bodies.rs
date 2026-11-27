//! A message's body at rest: the one place that knows what's in it.
//!
//! Everything else (the send transaction, history, sync, retries) handles a
//! body as an opaque [`Stored`]: a format number and its bytes, plus a
//! [`digest`](Submitted::digest) a retry is compared by.
//!
//! - **1, worker-sealed** (D2, gone): text the worker sealed itself. Every
//!   such body was deleted when end to end began ([`forget_format_1`]), and
//!   format 1 is refused from then on.
//! - **2, end to end** (E2): the sender's own MLS message (an `Envelope`),
//!   which the worker stores and hands back as it came, and never opens.
//! - **3, at the home**: the message itself goes through the
//!   person's home Claktory, never here; only the SHA-256 of its MLS bytes
//!   is kept, which the person's device matches it by.

use claktory_dm_crypto::envelope::{Envelope, Kind as MlsKind};
use rusqlite::{Connection, Transaction, params};
use serde_json::{Value, json};

use super::identity::{Refusal, invalid};
use super::keys::Refused;

/// The sender's MLS message.
pub const END_TO_END: i64 = 2;
/// Only the message's digest: the message is at the person's home.
pub const AT_HOME: i64 = 3;
/// Text the worker sealed (D2), deleted when end to end began.
pub const WORKER_SEALED: i64 = 1;

/// What a sender submits as a message's body: an application message.
pub struct Submitted(Envelope);

impl Submitted {
    /// A body from a request's `envelope`: an MLS application message, or
    /// why not.
    pub fn from_json(value: &Value) -> Result<Submitted, Refusal> {
        let envelope = Envelope::from_json(value).map_err(|error| invalid(&format!("envelope isn't one: {error}.")))?;
        if envelope.parsed().kind != MlsKind::Application {
            return Err(invalid("envelope is an application message."));
        }
        Ok(Submitted(envelope))
    }

    pub fn envelope(&self) -> &Envelope {
        &self.0
    }

    /// The group and epoch it's for.
    pub fn group(&self) -> (Vec<u8>, u64) {
        let (group, epoch) = self.0.parsed().group.expect("an application message names its group");
        (group.as_slice().to_vec(), epoch)
    }

    /// What a retry is compared by: SHA-256 of its bytes.
    pub fn digest(&self) -> Vec<u8> {
        self.0.digest().to_vec()
    }

    pub fn store(&self) -> Stored {
        Stored { format: END_TO_END, bytes: self.0.bytes().to_vec() }
    }
}

/// A body as kept: its format and bytes, opaque outside this module.
pub struct Stored {
    pub format: i64,
    pub bytes: Vec<u8>,
}

impl Stored {
    /// The body as a device is shown it: `{"envelope", "epoch"}`, to merge
    /// into what's shown.
    pub fn shown(&self) -> Result<Value, Refused> {
        match self.format {
            END_TO_END => {
                let envelope = Envelope::new(self.bytes.clone()).map_err(|_| Refused::DoesntOpen)?;
                let epoch = envelope.parsed().group.map(|(_, epoch)| epoch);
                Ok(json!({"envelope": envelope.to_json(), "epoch": epoch}))
            }
            // Nothing of it here: the device has it from the home.
            AT_HOME => Ok(json!({})),
            _ => Err(Refused::DoesntOpen),
        }
    }

    /// Roughly how much it takes in an answer, to keep a page bounded.
    pub fn size(&self) -> usize {
        self.bytes.len()
    }
}

/// A digest as a body names it (`proposal_digest`, `draft_digest`): the
/// SHA-256 of the message's MLS bytes, lowercase hex; kept as its bytes.
pub fn digest_of(text: &str, what: &str) -> Result<Vec<u8>, Refusal> {
    let fits = text.len() == 64 && text.bytes().all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte));
    if !fits {
        return Err(invalid(&format!("{what} is a SHA-256 in lowercase hex.")));
    }
    Ok((0..32).map(|at| u8::from_str_radix(&text[at * 2..at * 2 + 2], 16).unwrap_or_default()).collect())
}

/// What's kept of a message that's at the home: its digest only.
pub fn at_home() -> Stored {
    Stored { format: AT_HOME, bytes: Vec::new() }
}

/// The data key a conversation's D2 bodies were sealed with, forgotten
/// with them.
pub fn purpose(conversation: &str) -> String {
    format!("conversation:{conversation}")
}

/// Merges `fields` (an object) into `into`.
pub fn merge(into: &mut Value, fields: Value) {
    if let (Some(into), Value::Object(fields)) = (into.as_object_mut(), fields) {
        into.extend(fields);
    }
}

/// End to end begins: every worker-sealed body goes (messages, drafts,
/// proposals, agents' drafts: all test data), with what pointed at them,
/// and the space they took. Does nothing once there are none. How many
/// went.
pub fn forget_format_1(transaction: &Transaction<'_>) -> rusqlite::Result<usize> {
    let any: bool = transaction.query_row(
        "SELECT EXISTS (SELECT 1 FROM dm_messages WHERE format = ?1) OR EXISTS (SELECT 1 FROM dm_proposals WHERE format = ?1)
         OR EXISTS (SELECT 1 FROM dm_agent_drafts WHERE format = ?1) OR EXISTS (SELECT 1 FROM dm_views WHERE draft_format = ?1)",
        [WORKER_SEALED],
        |row| row.get(0),
    )?;
    if !any {
        return Ok(0);
    }
    let conversations: Vec<String> = {
        let mut statement = transaction.prepare("SELECT DISTINCT conversation_id FROM dm_messages WHERE format = ?1")?;
        statement.query_map([WORKER_SEALED], |row| row.get(0))?.collect::<Result<_, _>>()?
    };
    let mut gone = transaction.execute("DELETE FROM dm_messages WHERE format = ?1", [WORKER_SEALED])?;
    gone += transaction.execute("DELETE FROM dm_proposals WHERE format = ?1", [WORKER_SEALED])?;
    gone += transaction.execute("DELETE FROM dm_agent_drafts WHERE format = ?1", [WORKER_SEALED])?;
    gone += transaction.execute("UPDATE dm_views SET draft_format = NULL, draft = NULL WHERE draft_format = ?1", [WORKER_SEALED])?;
    // Their numbering goes with them: what follows is numbered by the
    // conversation's group.
    for conversation in conversations {
        let left: bool =
            transaction
                .query_row("SELECT EXISTS (SELECT 1 FROM dm_messages WHERE conversation_id = ?1)", [&conversation], |row| row.get(0))?;
        if !left {
            transaction.execute("DELETE FROM dm_conversations WHERE id = ?1", [&conversation])?;
            transaction.execute("UPDATE dm_views SET read_seq = 0, deleted_seq = 0 WHERE conversation_id = ?1", [&conversation])?;
        }
        transaction.execute("DELETE FROM dm_keys WHERE purpose = ?1", [purpose(&conversation)])?;
    }
    // What each account keeps, counted again.
    transaction.execute("DELETE FROM dm_usage", [])?;
    transaction.execute(
        "INSERT INTO dm_usage (account, bytes)
         SELECT account, SUM(size) FROM (
             SELECT c.low AS account, LENGTH(m.body) AS size, m.seq AS seq, m.conversation_id AS conversation_id
                 FROM dm_messages m JOIN dm_contacts c ON c.conversation_id = m.conversation_id
             UNION ALL
             SELECT c.high, LENGTH(m.body), m.seq, m.conversation_id
                 FROM dm_messages m JOIN dm_contacts c ON c.conversation_id = m.conversation_id
         ) kept
         WHERE seq > COALESCE((SELECT deleted_seq FROM dm_views v WHERE v.account = kept.account AND v.conversation_id = kept.conversation_id), 0)
         GROUP BY account",
        params![],
    )?;
    Ok(gone)
}

/// Runs [`forget_format_1`] on the worker's database, in its own
/// transaction.
pub fn forget_format_1_in(db: &mut Connection) -> rusqlite::Result<usize> {
    let transaction = db.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    let gone = forget_format_1(&transaction)?;
    transaction.commit()?;
    Ok(gone)
}

#[cfg(test)]
pub(super) mod tests {
    //! Application messages for tests: the worker never opens one, so a
    //! test's needn't be real MLS, only framed like one, with its text
    //! where the ciphertext goes.

    use super::*;

    fn vector(out: &mut Vec<u8>, bytes: &[u8]) {
        // MLS's variable-length size: one byte below 64, two below 16384.
        match bytes.len() {
            n if n < 64 => out.push(n as u8),
            n if n < 16_384 => out.extend_from_slice(&((n as u16) | 0x4000).to_be_bytes()),
            n => out.extend_from_slice(&((n as u32) | 0x8000_0000).to_be_bytes()),
        }
        out.extend_from_slice(bytes);
    }

    /// An application message for `group` at `epoch`, carrying `text`.
    pub(in crate::dms) fn sealed(group: &[u8], epoch: u64, text: &str) -> Value {
        let mut bytes = vec![0, 1, 0, 2];
        vector(&mut bytes, group);
        bytes.extend_from_slice(&epoch.to_be_bytes());
        bytes.push(1);
        vector(&mut bytes, b"");
        vector(&mut bytes, &[0; 16]);
        vector(&mut bytes, text.as_bytes());
        Envelope::new(bytes).expect("framed as an application message").to_json()
    }

    /// The text [`sealed`] put in.
    pub(in crate::dms) fn opened(envelope: &Value) -> String {
        let envelope = Envelope::from_json(envelope).expect("an envelope");
        let bytes = envelope.bytes();
        // After the version, wire format, group, epoch, content type, data
        // and sender data: the ciphertext, the rest.
        let mut at = 4;
        let skip = |at: &mut usize| {
            let (len, size) = match bytes[*at] >> 6 {
                0 => (bytes[*at] as usize, 1),
                1 => ((u16::from_be_bytes([bytes[*at], bytes[*at + 1]]) & 0x3fff) as usize, 2),
                _ => ((u32::from_be_bytes([bytes[*at], bytes[*at + 1], bytes[*at + 2], bytes[*at + 3]]) & 0x3fff_ffff) as usize, 4),
            };
            *at += size + len;
        };
        skip(&mut at);
        at += 9;
        skip(&mut at);
        skip(&mut at);
        let size = if bytes[at] >> 6 == 0 { 1 } else { 2 };
        String::from_utf8(bytes[at + size..].to_vec()).expect("text")
    }

    #[test]
    fn a_framed_application_message_reads_back() {
        let envelope = sealed(&[1; 16], 3, "Hello");
        let submitted = Submitted::from_json(&envelope).ok().expect("an application message");
        assert_eq!(submitted.group(), (vec![1; 16], 3));
        assert_eq!(opened(&envelope), "Hello");
        assert_eq!(opened(&sealed(&[1; 16], 3, &"x".repeat(100))), "x".repeat(100));
        assert!(Submitted::from_json(&json!({"format": 1, "text": "hi"})).is_err(), "format 1 is refused");
    }
}
