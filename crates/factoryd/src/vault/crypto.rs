//! The vault's cryptography. Every value is sealed with XChaCha20-Poly1305
//! (a random 24-byte nonce each time, so nonces need no bookkeeping), with
//! associated data naming what it is, so a sealed value can't be moved to
//! another secret, variable or vault.
//!
//! Keys: each secret's own key (its DEK) seals its values; the vault's key
//! (the VMK) seals the DEKs; and each way to open the vault (a protector:
//! the passphrase, the recovery key, the key kept on disk) seals the VMK.

use chacha20poly1305::aead::{Aead, Payload};
use chacha20poly1305::{KeyInit, XChaCha20Poly1305, XNonce};
use hmac::{Hmac, Mac};
use sha2::Sha256;
use zeroize::Zeroizing;

/// A 256-bit key, wiped from memory when dropped.
pub(crate) type Key = Zeroizing<[u8; 32]>;

const NONCE: usize = 24;

/// Argon2id's cost for passphrases: 64 MiB, 3 passes, one lane.
pub(crate) const PASSPHRASE_PARAMS: &str = "argon2id m=65536 t=3 p=1";

pub(crate) fn random<const N: usize>() -> [u8; N] {
    let mut bytes = [0; N];
    // The system's random source failing is not something to carry on from.
    getrandom::fill(&mut bytes).expect("the system's random source");
    bytes
}

pub(crate) fn new_key() -> Key {
    Zeroizing::new(random())
}

/// `plaintext` sealed with `key`: the nonce, then the ciphertext and its tag.
pub(crate) fn seal(key: &[u8; 32], plaintext: &[u8], aad: &[u8]) -> Vec<u8> {
    let cipher = XChaCha20Poly1305::new_from_slice(key).expect("a 32-byte key");
    let nonce: [u8; NONCE] = random();
    let sealed = cipher.encrypt(&XNonce::from(nonce), Payload { msg: plaintext, aad }).expect("sealing in memory");
    let mut out = Vec::with_capacity(NONCE + sealed.len());
    out.extend_from_slice(&nonce);
    out.extend_from_slice(&sealed);
    out
}

/// What `seal` sealed, if `key` and `aad` are the ones it was sealed with
/// and nothing changed it since.
pub(crate) fn open(key: &[u8; 32], sealed: &[u8], aad: &[u8]) -> Option<Zeroizing<Vec<u8>>> {
    if sealed.len() < NONCE {
        return None;
    }
    let (nonce, rest) = sealed.split_at(NONCE);
    let nonce: [u8; NONCE] = nonce.try_into().ok()?;
    let cipher = XChaCha20Poly1305::new_from_slice(key).expect("a 32-byte key");
    cipher.decrypt(&XNonce::from(nonce), Payload { msg: rest, aad }).ok().map(Zeroizing::new)
}

/// A key sealed with another (the VMK by a protector, a DEK by the VMK).
pub(crate) fn open_key(key: &[u8; 32], sealed: &[u8], aad: &[u8]) -> Option<Key> {
    let bytes = open(key, sealed, aad)?;
    let key: [u8; 32] = bytes.as_slice().try_into().ok()?;
    Some(Zeroizing::new(key))
}

/// The key a passphrase makes, with `params` as stored beside it.
pub(crate) fn passphrase_key(passphrase: &str, salt: &[u8], params: &str) -> Option<Key> {
    let mut costs = [0u32; 3];
    let mut parts = params.split(' ');
    if parts.next() != Some("argon2id") {
        return None;
    }
    for (part, (name, cost)) in parts.zip([("m", 0), ("t", 1), ("p", 2)]) {
        costs[cost] = part.strip_prefix(name)?.strip_prefix('=')?.parse().ok()?;
    }
    let params = argon2::Params::new(costs[0], costs[1], costs[2], Some(32)).ok()?;
    let argon = argon2::Argon2::new(argon2::Algorithm::Argon2id, argon2::Version::V0x13, params);
    let mut key = Zeroizing::new([0u8; 32]);
    argon.hash_password_into(passphrase.as_bytes(), salt, key.as_mut()).ok()?;
    Some(key)
}

/// A key derived from another for one purpose (HKDF-SHA256).
pub(crate) fn derive(ikm: &[u8], salt: &[u8], info: &str) -> Key {
    let mut key = Zeroizing::new([0u8; 32]);
    hkdf::Hkdf::<Sha256>::new(Some(salt), ikm).expand(info.as_bytes(), key.as_mut()).expect("32 bytes is a valid length");
    key
}

/// HMAC-SHA256 of `message`.
pub(crate) fn mac(key: &[u8; 32], message: &[u8]) -> Vec<u8> {
    let mut mac = <Hmac<Sha256> as hmac::KeyInit>::new_from_slice(key).expect("any key length");
    mac.update(message);
    mac.finalize().into_bytes().to_vec()
}

/// Whether `tag` is `message`'s HMAC, compared in constant time.
pub(crate) fn mac_matches(key: &[u8; 32], message: &[u8], tag: &[u8]) -> bool {
    let mut mac = <Hmac<Sha256> as hmac::KeyInit>::new_from_slice(key).expect("any key length");
    mac.update(message);
    mac.verify_slice(tag).is_ok()
}

const BASE32: &[u8; 32] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";

/// A recovery key: 160 random bits as 32 letters and digits in groups of 4
/// ("K7QM-2XPA-…"), easy to write down and type.
pub(crate) fn new_recovery() -> (String, Zeroizing<[u8; 20]>) {
    let bytes = Zeroizing::new(random::<20>());
    let mut text = String::with_capacity(39);
    let mut bits = 0u32;
    let mut held = 0u32;
    for byte in bytes.iter() {
        held = (held << 8) | u32::from(*byte);
        bits += 8;
        while bits >= 5 {
            bits -= 5;
            if !text.is_empty() && text.len() % 5 == 4 {
                text.push('-');
            }
            text.push(BASE32[((held >> bits) & 31) as usize] as char);
        }
    }
    (text, bytes)
}

/// A recovery key as typed back: dashes, spaces and case don't matter, and
/// 0, 1 and 8 read as the letters O, I and B they look like. None unless
/// it's 32 such characters.
pub(crate) fn parse_recovery(text: &str) -> Option<Zeroizing<[u8; 20]>> {
    let mut out = Zeroizing::new([0u8; 20]);
    let mut count = 0;
    let (mut bits, mut held, mut at) = (0u32, 0u32, 0usize);
    for c in text.chars().filter(|c| !matches!(c, '-' | ' ')) {
        let c = match c.to_ascii_uppercase() {
            '0' => 'O',
            '1' => 'I',
            '8' => 'B',
            c => c,
        };
        let value = BASE32.iter().position(|&b| b as char == c)? as u32;
        count += 1;
        held = (held << 5) | value;
        bits += 5;
        if bits >= 8 {
            bits -= 8;
            if at < 20 {
                out[at] = (held >> bits) as u8;
            }
            at += 1;
        }
    }
    (count == 32).then_some(out)
}

pub(crate) fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

pub(crate) fn unhex(text: &str) -> Option<Vec<u8>> {
    let text = text.trim();
    if !text.len().is_multiple_of(2) {
        return None;
    }
    (0..text.len()).step_by(2).map(|at| u8::from_str_radix(text.get(at..at + 2)?, 16).ok()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sealed_values_open_only_with_their_key_and_associated_data() {
        let key = new_key();
        let sealed = seal(&key, b"hunter2-but-longer", b"secret a");
        assert_eq!(open(&key, &sealed, b"secret a").as_deref().map(Vec::as_slice), Some(&b"hunter2-but-longer"[..]));
        assert!(open(&key, &sealed, b"secret b").is_none(), "moved to another secret");
        assert!(open(&new_key(), &sealed, b"secret a").is_none(), "another key");
        let mut tampered = sealed.clone();
        *tampered.last_mut().unwrap() ^= 1;
        assert!(open(&key, &tampered, b"secret a").is_none(), "changed");
        assert_ne!(seal(&key, b"same", b""), seal(&key, b"same", b""), "a fresh nonce each time");
    }

    #[test]
    fn recovery_keys_read_back_however_they_are_typed() {
        let (text, bytes) = new_recovery();
        assert_eq!(text.len(), 39, "{text}");
        assert_eq!(text.matches('-').count(), 7, "{text}");
        assert_eq!(parse_recovery(&text).as_deref(), Some(&*bytes));
        let typed = text.to_lowercase().replace('-', " ").replace('o', "0");
        assert_eq!(parse_recovery(&typed).as_deref(), Some(&*bytes));
        assert!(parse_recovery("not a key").is_none());
        assert!(parse_recovery(&text[..30]).is_none());
    }

    #[test]
    fn passphrases_make_the_same_key_with_the_same_salt() {
        let salt = random::<16>();
        let params = "argon2id m=1024 t=1 p=1";
        let key = passphrase_key("correct horse", &salt, params).expect("key");
        assert_eq!(passphrase_key("correct horse", &salt, params).as_deref(), Some(&*key));
        assert_ne!(passphrase_key("correct horsf", &salt, params).as_deref(), Some(&*key));
        assert!(passphrase_key("x", &salt, "scrypt n=1").is_none());
        // The real cost: 64 MiB, well under a second optimized.
        assert!(passphrase_key("correct horse", &salt, PASSPHRASE_PARAMS).is_some());
        assert_eq!(unhex(&hex(&[0, 15, 255])), Some(vec![0, 15, 255]));
    }
}
