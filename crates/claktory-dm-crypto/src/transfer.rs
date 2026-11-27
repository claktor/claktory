//! History transfer's sealing (E8): an existing
//! device sends a new one its decrypted history in chunks, sealed with the
//! key both got from the add-device hand-off's HPKE context
//! (`handoff::Sealed::transfer_key`, `handoff::Received::transfer_key`), then
//! an end record saying how much there was, so a transfer cut short or
//! reordered is noticed.
//!
//! - One key per session: `HKDF-SHA256(transfer_key, salt = session)`, so a
//!   chunk's index can be its nonce without ever repeating under one key.
//!   **A (session, index) pair is never sealed twice** (a repeated nonce
//!   would give both plaintexts away and let anyone forge): a [`Sealer`]
//!   makes its own random session and counts its own indexes, and every
//!   attempt, a resume too, is a new `Sealer`. The receiver resumes by what
//!   it kept per conversation, never by index.
//! - An [`Opener`] takes chunks only in order from 0, each once, then the
//!   end record last, and holds the transfer to the totals it says: a
//!   transfer that doesn't add up is incomplete, and is shown so.
//! - Each chunk's associated data binds the label, the session, its index
//!   and its conversation: a chunk can't be moved to another place, session
//!   or conversation.
//! - The end record is sealed at index `u64::MAX` under a label of its own,
//!   so it can't pass for a chunk nor a chunk for it.
//!
//! ChaCha20-Poly1305 (RFC 8439).

use std::collections::BTreeMap;

use chacha20poly1305::aead::{Aead, Payload};
use chacha20poly1305::{ChaCha20Poly1305, KeyInit, Nonce};
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use zeroize::Zeroizing;

use crate::Error;

/// The most a chunk holds, in bytes.
pub const MAX_CHUNK: usize = 256 * 1024;
const CHUNK: &[u8] = b"claktory-dm/v1 transfer chunk";
const END: &[u8] = b"claktory-dm/v1 transfer end";
const END_INDEX: u64 = u64::MAX;

/// What a transfer had, sealed last.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct End {
    /// How many chunks came before it (indexes 0 to chunks - 1).
    pub chunks: u64,
    /// Messages per conversation.
    pub messages: BTreeMap<String, u64>,
}

/// The session's own key.
fn cipher(key: &[u8; 32], session: &[u8; 16]) -> ChaCha20Poly1305 {
    let mut derived = Zeroizing::new([0u8; 32]);
    hkdf::Hkdf::<Sha256>::new(Some(session), key)
        .expand(b"claktory-dm/v1 transfer session", derived.as_mut())
        .expect("32 bytes is a valid length");
    ChaCha20Poly1305::new_from_slice(derived.as_ref()).expect("a 32-byte key")
}

fn nonce(index: u64) -> Nonce {
    let mut nonce = [0u8; 12];
    nonce[4..].copy_from_slice(&index.to_be_bytes());
    Nonce::from(nonce)
}

/// The associated data: the label, the session, the index and the
/// conversation, each part after its length.
fn aad(label: &[u8], session: &[u8; 16], index: u64, conversation: &str) -> Vec<u8> {
    let mut aad = Vec::with_capacity(label.len() + 16 + 8 + conversation.len() + 8);
    for part in [label, session.as_slice(), &index.to_be_bytes(), conversation.as_bytes()] {
        aad.extend_from_slice(&(part.len() as u32).to_be_bytes());
        aad.extend_from_slice(part);
    }
    aad
}

/// Seals chunk `index` of a session, from one conversation.
fn seal_chunk(key: &[u8; 32], session: &[u8; 16], index: u64, conversation: &str, plain: &[u8]) -> Result<Vec<u8>, Error> {
    if index == END_INDEX {
        return Err(Error::Format("that index is the end record's".to_owned()));
    }
    if plain.len() > MAX_CHUNK {
        return Err(Error::TooBig { kind: "a transfer chunk", most: MAX_CHUNK });
    }
    cipher(key, session)
        .encrypt(&nonce(index), Payload { msg: plain, aad: &aad(CHUNK, session, index, conversation) })
        .map_err(|_| Error::Format("couldn't seal a chunk".to_owned()))
}

/// Opens chunk `index` of a session, from `conversation`; anything else is
/// refused.
fn open_chunk(key: &[u8; 32], session: &[u8; 16], index: u64, conversation: &str, sealed: &[u8]) -> Result<Zeroizing<Vec<u8>>, Error> {
    if index == END_INDEX || sealed.len() > MAX_CHUNK + 16 {
        return Err(Error::Sealed);
    }
    cipher(key, session)
        .decrypt(&nonce(index), Payload { msg: sealed, aad: &aad(CHUNK, session, index, conversation) })
        .map(Zeroizing::new)
        .map_err(|_| Error::Sealed)
}

/// Seals the session's end record.
fn seal_end(key: &[u8; 32], session: &[u8; 16], end: &End) -> Result<Vec<u8>, Error> {
    let plain = serde_json::to_vec(end).map_err(|error| Error::Format(error.to_string()))?;
    if plain.len() > MAX_CHUNK {
        return Err(Error::TooBig { kind: "a transfer's end record", most: MAX_CHUNK });
    }
    cipher(key, session)
        .encrypt(&nonce(END_INDEX), Payload { msg: &plain, aad: &aad(END, session, END_INDEX, "") })
        .map_err(|_| Error::Format("couldn't seal the end record".to_owned()))
}

/// Opens the session's end record.
fn open_end(key: &[u8; 32], session: &[u8; 16], sealed: &[u8]) -> Result<End, Error> {
    if sealed.len() > MAX_CHUNK + 16 {
        return Err(Error::Sealed);
    }
    let plain = cipher(key, session)
        .decrypt(&nonce(END_INDEX), Payload { msg: sealed, aad: &aad(END, session, END_INDEX, "") })
        .map_err(|_| Error::Sealed)?;
    serde_json::from_slice(&plain).map_err(|_| Error::Format("not a transfer's end record".to_owned()))
}

/// The sending device's side of one transfer attempt: a fresh random
/// session, its indexes counted here.
pub struct Sealer {
    key: Zeroizing<[u8; 32]>,
    session: [u8; 16],
    next: u64,
    messages: BTreeMap<String, u64>,
    ended: bool,
}

impl Sealer {
    pub fn new(transfer_key: &[u8; 32]) -> Result<Sealer, Error> {
        let mut session = [0u8; 16];
        getrandom::fill(&mut session).map_err(|error| Error::Format(format!("no randomness: {error}")))?;
        Ok(Sealer { key: Zeroizing::new(*transfer_key), session, next: 0, messages: BTreeMap::new(), ended: false })
    }

    /// The session, to send with every chunk.
    pub fn session(&self) -> [u8; 16] {
        self.session
    }

    /// Seals the next chunk, of `messages` messages from `conversation`: its
    /// index and its bytes.
    pub fn seal_chunk(&mut self, conversation: &str, messages: u64, plain: &[u8]) -> Result<(u64, Vec<u8>), Error> {
        if self.ended {
            return Err(Error::Format("the transfer has ended".to_owned()));
        }
        let index = self.next;
        let sealed = seal_chunk(&self.key, &self.session, index, conversation, plain)?;
        self.next += 1;
        *self.messages.entry(conversation.to_owned()).or_default() += messages;
        Ok((index, sealed))
    }

    /// Seals the end record: how many chunks, and messages per conversation.
    pub fn seal_end(&mut self) -> Result<Vec<u8>, Error> {
        self.ended = true;
        seal_end(&self.key, &self.session, &End { chunks: self.next, messages: self.messages.clone() })
    }
}

/// The receiving device's side of one transfer: chunks in order from 0,
/// each once, then the end record.
pub struct Opener {
    key: Zeroizing<[u8; 32]>,
    session: [u8; 16],
    next: u64,
}

impl Opener {
    pub fn new(transfer_key: &[u8; 32], session: [u8; 16]) -> Opener {
        Opener { key: Zeroizing::new(*transfer_key), session, next: 0 }
    }

    /// Opens chunk `index`, which must be the next one.
    pub fn open_chunk(&mut self, index: u64, conversation: &str, sealed: &[u8]) -> Result<Zeroizing<Vec<u8>>, Error> {
        if index != self.next {
            return Err(Error::Format(format!("chunk {index} came where chunk {} should have", self.next)));
        }
        let plain = open_chunk(&self.key, &self.session, index, conversation, sealed)?;
        self.next += 1;
        Ok(plain)
    }

    /// Opens the end record, last, and checks the transfer against it:
    /// every chunk came, and `kept` (messages kept per conversation from
    /// this transfer) is what it says. Err: the transfer is incomplete.
    pub fn finish(self, sealed: &[u8], kept: &BTreeMap<String, u64>) -> Result<End, Error> {
        let end = open_end(&self.key, &self.session, sealed)?;
        if end.chunks != self.next {
            return Err(Error::Format(format!("{} of {} chunks came", self.next, end.chunks)));
        }
        let said: BTreeMap<&String, u64> = end.messages.iter().filter(|(_, count)| **count > 0).map(|(id, count)| (id, *count)).collect();
        let got: BTreeMap<&String, u64> = kept.iter().filter(|(_, count)| **count > 0).map(|(id, count)| (id, *count)).collect();
        if said != got {
            return Err(Error::Format("the messages kept aren't what the transfer says it sent".to_owned()));
        }
        Ok(end)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_chunk_opens_only_where_it_was_sealed_and_the_end_is_apart() {
        let (key, session) = ([1u8; 32], [2u8; 16]);
        let sealed = seal_chunk(&key, &session, 0, "conversation-1", b"hello").unwrap();
        assert_eq!(open_chunk(&key, &session, 0, "conversation-1", &sealed).unwrap().as_slice(), b"hello");
        for (key, session, index, conversation) in [
            ([9u8; 32], session, 0, "conversation-1"),
            (key, [9u8; 16], 0, "conversation-1"),
            (key, session, 1, "conversation-1"),
            (key, session, 0, "conversation-2"),
        ] {
            assert!(open_chunk(&key, &session, index, conversation, &sealed).is_err());
        }
        assert!(open_end(&key, &session, &sealed).is_err(), "a chunk isn't an end record");

        let end = End { chunks: 1, messages: [("conversation-1".to_owned(), 1)].into_iter().collect() };
        let sealed_end = seal_end(&key, &session, &end).unwrap();
        assert_eq!(open_end(&key, &session, &sealed_end).unwrap(), end);
        assert!(open_chunk(&key, &session, END_INDEX, "", &sealed_end).is_err());
        assert!(seal_chunk(&key, &session, END_INDEX, "", b"x").is_err());
        assert!(seal_chunk(&key, &session, 0, "c", &vec![0; MAX_CHUNK + 1]).is_err());
    }

    #[test]
    fn a_transfer_opens_in_order_and_only_adds_up_when_whole() {
        let key = [3u8; 32];
        let mut sealer = Sealer::new(&key).unwrap();
        assert_ne!(sealer.session(), Sealer::new(&key).unwrap().session(), "a new session each attempt");
        let chunks: Vec<(u64, Vec<u8>)> = [("c1", 2, b"one".as_slice()), ("c2", 1, b"two"), ("c1", 1, b"three")]
            .map(|(c, n, p)| sealer.seal_chunk(c, n, p).unwrap())
            .to_vec();
        let end = sealer.seal_end().unwrap();
        assert!(sealer.seal_chunk("c1", 1, b"late").is_err());
        let names = ["c1", "c2", "c1"];
        let kept: BTreeMap<String, u64> = [("c1".to_owned(), 3), ("c2".to_owned(), 1)].into_iter().collect();

        let mut opener = Opener::new(&key, sealer.session());
        for ((index, sealed), conversation) in chunks.iter().zip(names) {
            opener.open_chunk(*index, conversation, sealed).unwrap();
        }
        assert_eq!(opener.finish(&end, &kept).unwrap().chunks, 3);

        // Out of order, a chunk twice, one missing, or other counts: refused.
        let mut opener = Opener::new(&key, sealer.session());
        assert!(opener.open_chunk(1, "c2", &chunks[1].1).is_err());
        opener.open_chunk(0, "c1", &chunks[0].1).unwrap();
        assert!(opener.open_chunk(0, "c1", &chunks[0].1).is_err());
        assert!(opener.finish(&end, &kept).is_err(), "two chunks never came");
        let mut opener = Opener::new(&key, sealer.session());
        for ((index, sealed), conversation) in chunks.iter().zip(names) {
            opener.open_chunk(*index, conversation, sealed).unwrap();
        }
        let short: BTreeMap<String, u64> = [("c1".to_owned(), 2), ("c2".to_owned(), 1)].into_iter().collect();
        assert!(opener.finish(&end, &short).is_err());
    }
}
