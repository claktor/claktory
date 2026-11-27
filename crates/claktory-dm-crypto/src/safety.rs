//! The safety number (dms-e2ee.md §5.3 Verify): what two people
//! compare, read aloud or by scanning each other's QR code, to know their
//! clients pinned each other's real identity keys and nothing between
//! them swapped one. It's made from both identity keys and both account
//! ids, so it's the same on both sides and changes when either key does.
//!
//! As Signal's: each side's half is SHA-512 iterated 5,200 times over its
//! key and account id, so finding another key that gives the same digits
//! costs that many hashes per try; 30 digits a side, 60 in all, in groups
//! of five, the lower half first.

use sha2::{Digest, Sha512};

use crate::Error;
use crate::identity::IdentityPublic;

const VERSION: u8 = 1;
const ITERATIONS: usize = 5200;
/// The QR code's bytes: the version, then two 30-byte halves.
const QR_LEN: usize = 1 + 30 + 30;

/// One side's half: 30 bytes.
fn half(identity: &IdentityPublic, account_id: &str) -> [u8; 30] {
    let mut hash = Sha512::new();
    hash.update([0, VERSION]);
    hash.update(b"claktory-dm/v1 safety number\0");
    hash.update(identity.0);
    hash.update(account_id.as_bytes());
    let mut digest = hash.finalize();
    for _ in 1..ITERATIONS {
        let mut next = Sha512::new();
        next.update(digest);
        next.update(identity.0);
        digest = next.finalize();
    }
    digest[..30].try_into().expect("30 of 64 bytes")
}

/// 30 bytes as 30 digits: six five-byte chunks, each modulo 100,000.
fn digits(half: &[u8; 30]) -> String {
    half.chunks(5)
        .map(|chunk| {
            let value = chunk.iter().fold(0u64, |value, byte| (value << 8) | u64::from(*byte));
            format!("{:05}", value % 100_000)
        })
        .collect()
}

/// Two people's safety number, as both see it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SafetyNumber {
    ours: [u8; 30],
    theirs: [u8; 30],
}

impl SafetyNumber {
    pub fn new(ours: (&IdentityPublic, &str), theirs: (&IdentityPublic, &str)) -> SafetyNumber {
        SafetyNumber { ours: half(ours.0, ours.1), theirs: half(theirs.0, theirs.1) }
    }

    /// 60 digits in twelve groups of five, the same on both sides.
    pub fn digits(&self) -> String {
        let (ours, theirs) = (digits(&self.ours), digits(&self.theirs));
        let (first, second) = if ours <= theirs { (ours, theirs) } else { (theirs, ours) };
        let all = format!("{first}{second}");
        all.as_bytes().chunks(5).map(|group| std::str::from_utf8(group).expect("digits")).collect::<Vec<_>>().join(" ")
    }

    /// What this side's QR code holds: its half first.
    pub fn qr(&self) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(QR_LEN);
        bytes.push(VERSION);
        bytes.extend_from_slice(&self.ours);
        bytes.extend_from_slice(&self.theirs);
        bytes
    }

    /// Whether the other side's scanned QR code shows the same two keys:
    /// its first half is their key as they hold it, and its second is ours
    /// as they pinned it.
    pub fn matches_scanned(&self, scanned: &[u8]) -> Result<bool, Error> {
        if scanned.len() != QR_LEN || scanned[0] != VERSION {
            return Err(Error::Format("not a Claktory safety code".to_owned()));
        }
        // Comparing digests of public keys: no secret here to time.
        Ok(scanned[1..31] == self.theirs && scanned[31..] == self.ours)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity::IdentityKey;

    #[test]
    fn both_sides_see_the_same_number_and_a_changed_key_changes_it() {
        let (matt, ana) = (IdentityKey::generate().unwrap().public(), IdentityKey::generate().unwrap().public());
        let at_matt = SafetyNumber::new((&matt, "matt"), (&ana, "ana"));
        let at_ana = SafetyNumber::new((&ana, "ana"), (&matt, "matt"));
        let shown = at_matt.digits();
        assert_eq!(shown, at_ana.digits());
        assert_eq!(shown.len(), 60 + 11);
        assert!(shown.split(' ').all(|group| group.len() == 5 && group.bytes().all(|b| b.is_ascii_digit())));
        assert!(at_matt.matches_scanned(&at_ana.qr()).unwrap());
        assert!(!at_matt.matches_scanned(&at_matt.qr()).unwrap(), "my own code isn't theirs");

        // Someone between them, with a key of their own for Ana.
        let mallory = IdentityKey::generate().unwrap().public();
        let fooled = SafetyNumber::new((&matt, "matt"), (&mallory, "ana"));
        assert_ne!(fooled.digits(), shown);
        assert!(!fooled.matches_scanned(&at_ana.qr()).unwrap());
        assert!(at_matt.matches_scanned(b"nope").is_err());
    }
}
