//! Adding a device (dms-e2ee.md §2, Fugo H3 and M3): the new device shows a
//! QR code; one of the person's devices that's set up scans it and sends
//! back the identity key and its pins, sealed so only the device that showed
//! that QR code can open them.
//!
//! The QR code carries the new device's leaf (its signature key and leaf
//! id), its name and platform (what the person is asked about), its iroh
//! endpoint id (the only one history transfer dials), a one-time X25519 key
//! and a one-time secret of 256 bits. The bundle is sealed with HPKE (RFC
//! 9180) in PSK mode to the one-time key, with the secret as the PSK and
//! everything else the code says bound in `info`: the service relays it, but
//! even one that learned the one-time key can't make a bundle the new device
//! opens, so it can't plant an identity of its own.
//!
//! The QR code is the way. Copying the code and pasting it on the other
//! device is the fallback, and only with a comparison: both devices show
//! [`Offer::comparison`] and the person adds the device only once they
//! match, since any app could have swapped the pasted code for its own
//! (Fugo, H3). A typed code can't carry the secret, so there's none.
//!
//! The same HPKE context exports the key history transfer uses
//! ([`crate::transfer`]), so only these two devices have it.

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use hpke_rs::hpke_types::{AeadAlgorithm, KdfAlgorithm, KemAlgorithm};
use hpke_rs::libcrux::HpkeLibcrux;
use hpke_rs::{Hpke, HpkePrivateKey, HpkePublicKey, Mode};
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use zeroize::{Zeroize, Zeroizing};

use chacha20poly1305::aead::{Aead, Payload};
use chacha20poly1305::{ChaCha20Poly1305, KeyInit, Nonce};

use crate::Error;
use crate::certificate::one_line;
use crate::identity::IdentityKey;
use crate::validation::Pins;

/// HPKE in PSK mode: DHKEM(X25519, HKDF-SHA256), HKDF-SHA256,
/// ChaCha20-Poly1305, through libcrux (the provider OpenMLS runs on here).
fn hpke() -> Hpke<HpkeLibcrux> {
    Hpke::new(Mode::Psk, KemAlgorithm::DhKem25519, KdfAlgorithm::HkdfSha256, AeadAlgorithm::ChaCha20Poly1305)
}

/// What a QR code starts with.
const PREFIX: &str = "claktory-dm-add:1:";
const PSK_ID: &[u8] = b"claktory-dm/v1 add device";
/// What the history-transfer key is exported with.
const TRANSFER_LABEL: &[u8] = b"claktory-dm/v1 history transfer";
/// What a pasted code's comparison is hashed with.
const COMPARISON_LABEL: &[u8] = b"claktory-dm/v1 add device comparison";
/// The longest sealed bundle: the key, the account and a few thousand pins,
/// within what the worker takes in one request once it's base64 in JSON.
pub const MAX_SEALED: usize = 512 * 1024;
/// A device's leaf id: its account key, 32 bytes in base64url.
const LEAF_ID_LEN: usize = 43;
/// The longest name and platform, in bytes.
pub const MAX_NAME: usize = 64;
pub const MAX_PLATFORM: usize = 32;

/// What the new device says about itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OfferFor {
    /// Its MLS leaf signature key, to certify.
    pub leaf_signature_key: [u8; 32],
    /// Its leaf id: its account key, base64url (as the worker knows it).
    pub leaf_id: String,
    /// What the person calls it ("Matt's iPad") and what it is ("iPadOS").
    pub name: String,
    pub platform: String,
    /// Its iroh endpoint id: the only one history transfer dials.
    pub endpoint_id: [u8; 32],
}

impl OfferFor {
    fn check(&self) -> Result<(), Error> {
        let id_ok = self.leaf_id.len() == LEAF_ID_LEN
            && self.leaf_id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
            && URL_SAFE_NO_PAD.decode(&self.leaf_id).is_ok_and(|key| key.len() == 32);
        if !id_ok {
            return Err(Error::Format("a device's leaf id is its 32-byte key in base64url".to_owned()));
        }
        if !one_line(&self.name, MAX_NAME) || !one_line(&self.platform, MAX_PLATFORM) {
            return Err(Error::Format(format!(
                "a device's name (up to {MAX_NAME} bytes) and platform (up to {MAX_PLATFORM}) are each one line, with nothing hidden in it"
            )));
        }
        Ok(())
    }
}

/// What the new device's QR code says.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Offer {
    pub device: OfferFor,
    one_time: [u8; 32],
    secret: [u8; 32],
}

/// What the new device keeps while it waits: the offer and the one-time
/// key's secret half. Gone once the bundle is opened.
pub struct Waiting {
    pub offer: Offer,
    /// Zeroized when dropped.
    one_time_secret: HpkePrivateKey,
}

impl Drop for Offer {
    fn drop(&mut self) {
        self.secret.zeroize();
    }
}

fn random<const N: usize>() -> Result<Zeroizing<[u8; N]>, Error> {
    let mut bytes = Zeroizing::new([0u8; N]);
    getrandom::fill(bytes.as_mut()).map_err(|error| Error::Format(format!("no randomness: {error}")))?;
    Ok(bytes)
}

/// `bytes` with its length first, as one byte (every part is checked to be
/// shorter than 256 bytes).
fn prefixed(out: &mut Vec<u8>, bytes: &[u8]) {
    out.push(u8::try_from(bytes.len()).expect("checked: under 256 bytes"));
    out.extend_from_slice(bytes);
}

/// `info` binds the bundle to everything the offer it answers says.
fn info(offer: &Offer) -> Vec<u8> {
    let device = &offer.device;
    let mut info = b"claktory-dm/v1 add device\0".to_vec();
    info.extend_from_slice(&offer.one_time);
    info.extend_from_slice(&device.leaf_signature_key);
    info.extend_from_slice(&device.endpoint_id);
    for part in [&device.leaf_id, &device.name, &device.platform] {
        prefixed(&mut info, part.as_bytes());
    }
    info
}

impl Waiting {
    /// The new device's offer.
    pub fn new(device: OfferFor) -> Result<Waiting, Error> {
        device.check()?;
        let ikm = random::<32>()?;
        let (one_time_secret, public) =
            hpke().derive_key_pair(ikm.as_ref()).map_err(|error| Error::Format(format!("no one-time key: {error:?}")))?.into_keys();
        let one_time = <[u8; 32]>::try_from(public.as_slice()).map_err(|_| Error::Format("an X25519 key is 32 bytes".to_owned()))?;
        Ok(Waiting { offer: Offer { device, one_time, secret: *random::<32>()? }, one_time_secret })
    }
}

impl Offer {
    /// The QR code's text: the leaf key, the one-time key, the secret and the
    /// endpoint id, then the leaf id, name and platform, each after its length.
    pub fn qr(&self) -> String {
        let device = &self.device;
        let mut bytes = Zeroizing::new(Vec::with_capacity(256));
        for part in [&device.leaf_signature_key, &self.one_time, &self.secret, &device.endpoint_id] {
            bytes.extend_from_slice(part);
        }
        for part in [&device.leaf_id, &device.name, &device.platform] {
            prefixed(&mut bytes, part.as_bytes());
        }
        format!("{PREFIX}{}", URL_SAFE_NO_PAD.encode(bytes.as_slice()))
    }

    /// What both devices show before a pasted code is used: four groups of
    /// five digits (66 bits), from SHA-256 over the code as [`Offer::qr`]
    /// writes it. An app that swapped the code would have to find one of
    /// its own with the same digits, about 2^66 tries, in the 30 minutes a
    /// code lives.
    pub fn comparison(&self) -> String {
        let code = Zeroizing::new(self.qr());
        let digest = Sha256::new().chain_update(COMPARISON_LABEL).chain_update(code.as_bytes()).finalize();
        let number = u128::from_be_bytes(digest[..16].try_into().expect("16 bytes")) % 10u128.pow(20);
        let digits = format!("{number:020}");
        [&digits[0..5], &digits[5..10], &digits[10..15], &digits[15..20]].join(" ")
    }

    /// The offer in a scanned QR code.
    pub fn from_qr(text: &str) -> Result<Offer, Error> {
        let not = || Error::Format("not a Claktory code for adding a device".to_owned());
        let encoded = text.trim().strip_prefix(PREFIX).ok_or_else(not)?;
        let bytes = Zeroizing::new(URL_SAFE_NO_PAD.decode(encoded).map_err(|_| not())?);
        if bytes.len() < 128 + 3 {
            return Err(not());
        }
        // Any 32 bytes are an X25519 public key; one that's low order fails
        // when sealing.
        let part = |at: usize| <[u8; 32]>::try_from(&bytes[at..at + 32]).expect("32 bytes");
        let mut rest = &bytes[128..];
        let mut text = || -> Result<String, Error> {
            let (&len, tail) = rest.split_first().ok_or_else(not)?;
            let len = usize::from(len);
            if tail.len() < len {
                return Err(not());
            }
            let (text, tail) = tail.split_at(len);
            rest = tail;
            String::from_utf8(text.to_vec()).map_err(|_| not())
        };
        let (leaf_id, name, platform) = (text()?, text()?, text()?);
        if !rest.is_empty() {
            return Err(not());
        }
        let device = OfferFor { leaf_signature_key: part(0), leaf_id, name, platform, endpoint_id: part(96) };
        device.check()?;
        Ok(Offer { device, one_time: part(32), secret: part(64) })
    }
}

/// What the new device gets.
#[derive(Serialize, Deserialize)]
struct Bundle {
    v: u8,
    account_id: String,
    identity: String,
    pins: Pins,
}

/// The bundle's plaintext.
fn bundle(account_id: &str, identity: &IdentityKey, pins: &Pins) -> Result<Zeroizing<Vec<u8>>, Error> {
    let mut bundle = Bundle {
        v: 1,
        account_id: account_id.to_owned(),
        identity: URL_SAFE_NO_PAD.encode(identity.secret().as_ref()),
        pins: pins.clone(),
    };
    let plaintext = serde_json::to_vec(&bundle).map(Zeroizing::new);
    bundle.identity.zeroize();
    plaintext.map_err(|error| Error::Format(error.to_string()))
}

/// A bundle's account, identity and pins.
fn unbundle(plaintext: &[u8]) -> Result<(String, IdentityKey, Pins), Error> {
    let mut bundle: Bundle = serde_json::from_slice(plaintext).map_err(|_| Error::Format("not a device hand-off".to_owned()))?;
    if bundle.v != 1 {
        return Err(Error::Format(format!("device hand-off version {}", bundle.v)));
    }
    let secret = URL_SAFE_NO_PAD.decode(&bundle.identity).map(Zeroizing::new);
    bundle.identity.zeroize();
    let secret = secret.map_err(|_| Error::Format("no identity key".to_owned()))?;
    let secret: Zeroizing<[u8; 32]> =
        Zeroizing::new(secret.as_slice().try_into().map_err(|_| Error::Format("an identity key is 32 bytes".to_owned()))?);
    let mut pins = bundle.pins;
    pins.reparse()?;
    Ok((bundle.account_id, IdentityKey::from_secret(&secret), pins))
}

/// A new identity for the person's other devices, after a rotation
/// (dms-e2ee §2.1): the bundle sealed under a key of its own. The bytes go
/// to each remaining device through the worker (`me/handoffs`, up to
/// [`MAX_SEALED`]); the key goes in the person's self group, which MLS
/// seals to exactly its members once the lost device is removed, so only
/// they can open it. A key is made for one bundle and never used again.
pub struct RotationSealed {
    pub bytes: Vec<u8>,
    pub key: Zeroizing<[u8; 32]>,
}

/// What a remaining device gets from a rotation.
pub struct Rotated {
    pub identity: IdentityKey,
    pub pins: Pins,
}

const ROTATION_LABEL: &[u8] = b"claktory-dm/v1 identity rotation";

fn rotation_aad(account_id: &str) -> Vec<u8> {
    [ROTATION_LABEL, b"\0", account_id.as_bytes()].concat()
}

/// Seals the new `identity` and `pins` for the person's other devices. The
/// pins must already have the account on the new identity (`key_changed`),
/// the old one retired.
pub fn seal_rotation(account_id: &str, identity: &IdentityKey, pins: &Pins) -> Result<RotationSealed, Error> {
    if pins.get(account_id).map(|pin| pin.identity) != Some(identity.public()) {
        return Err(Error::Refused("the pins don't have this account on the new key".to_owned()));
    }
    let plaintext = bundle(account_id, identity, pins)?;
    let key = random::<32>()?;
    let cipher = ChaCha20Poly1305::new_from_slice(key.as_slice()).expect("a 32-byte key");
    // A fresh key for each bundle: the one nonce is never used twice.
    let bytes = cipher
        .encrypt(&Nonce::default(), Payload { msg: &plaintext, aad: &rotation_aad(account_id) })
        .map_err(|_| Error::Format("couldn't seal the new key".to_owned()))?;
    if bytes.len() > MAX_SEALED {
        return Err(Error::TooBig { kind: "new security key", most: MAX_SEALED });
    }
    Ok(RotationSealed { bytes, key })
}

/// Opens what [`seal_rotation`] made, on a device of `account_id` (its
/// own): the bundle is for that account, and its pins have it on the new
/// identity. Whether the key came from one of the person's own devices in
/// their self group is the caller's to check, as for any self-group
/// message.
pub fn open_rotation(account_id: &str, key: &[u8; 32], bytes: &[u8]) -> Result<Rotated, Error> {
    if bytes.len() > MAX_SEALED {
        return Err(Error::Sealed);
    }
    let cipher = ChaCha20Poly1305::new_from_slice(key).expect("a 32-byte key");
    let plaintext = Zeroizing::new(
        cipher.decrypt(&Nonce::default(), Payload { msg: bytes, aad: &rotation_aad(account_id) }).map_err(|_| Error::Sealed)?,
    );
    let (account, identity, pins) = unbundle(&plaintext)?;
    if account != account_id {
        return Err(Error::Refused("a new key for another account".to_owned()));
    }
    if pins.get(account_id).map(|pin| pin.identity) != Some(identity.public()) {
        return Err(Error::Refused("the pins don't have this account on the new key".to_owned()));
    }
    Ok(Rotated { identity, pins })
}

/// What the scanning device sends, and the history-transfer key it shares
/// with the new device from now on.
pub struct Sealed {
    /// The encapsulated key, then the ciphertext: what goes through the worker.
    pub bytes: Vec<u8>,
    pub transfer_key: Zeroizing<[u8; 32]>,
}

/// What the new device has once it opened the bundle.
pub struct Received {
    pub account_id: String,
    pub identity: IdentityKey,
    pub pins: Pins,
    pub transfer_key: Zeroizing<[u8; 32]>,
}

fn transfer_key(exported: Result<Vec<u8>, hpke_rs::HpkeError>) -> Result<Zeroizing<[u8; 32]>, Error> {
    let exported = Zeroizing::new(exported.map_err(|error| Error::Format(format!("no transfer key: {error:?}")))?);
    let key: [u8; 32] = exported.as_slice().try_into().map_err(|_| Error::Format("a transfer key is 32 bytes".to_owned()))?;
    Ok(Zeroizing::new(key))
}

/// Seals the bundle for `offer`.
pub fn seal(offer: &Offer, account_id: &str, identity: &IdentityKey, pins: &Pins) -> Result<Sealed, Error> {
    offer.device.check()?;
    let plaintext = bundle(account_id, identity, pins)?;
    let recipient = HpkePublicKey::new(offer.one_time.to_vec());
    let (mut sealed, mut context) = hpke()
        .setup_sender(&recipient, &info(offer), Some(&offer.secret), Some(PSK_ID), None)
        .map_err(|error| Error::Format(format!("couldn't seal: {error:?}")))?;
    let ciphertext = context.seal(b"", &plaintext).map_err(|error| Error::Format(format!("couldn't seal: {error:?}")))?;
    let transfer_key = transfer_key(context.export(TRANSFER_LABEL, 32))?;
    sealed.extend_from_slice(&ciphertext);
    if sealed.len() > MAX_SEALED {
        return Err(Error::TooBig { kind: "device hand-off", most: MAX_SEALED });
    }
    Ok(Sealed { bytes: sealed, transfer_key })
}

impl Waiting {
    /// Opens the bundle the scanning device sent; anything that isn't one
    /// sealed for this offer is refused, and nothing in it is used.
    pub fn open(self, sealed: &[u8]) -> Result<Received, Error> {
        if sealed.len() < 32 || sealed.len() > MAX_SEALED {
            return Err(Error::Sealed);
        }
        let mut context = hpke()
            .setup_receiver(&sealed[..32], &self.one_time_secret, &info(&self.offer), Some(&self.offer.secret), Some(PSK_ID), None)
            .map_err(|_| Error::Sealed)?;
        let plaintext = Zeroizing::new(context.open(b"", &sealed[32..]).map_err(|_| Error::Sealed)?);
        let transfer_key = transfer_key(context.export(TRANSFER_LABEL, 32))?;
        let (account_id, identity, pins) = unbundle(&plaintext)?;
        Ok(Received { account_id, identity, pins, transfer_key })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn device() -> OfferFor {
        OfferFor {
            leaf_signature_key: [4; 32],
            leaf_id: URL_SAFE_NO_PAD.encode([5u8; 32]),
            name: "Matt's iPad".to_owned(),
            platform: "iPadOS".to_owned(),
            endpoint_id: [6; 32],
        }
    }

    fn sealed_error<T>(result: Result<T, Error>) -> Option<String> {
        result.err().map(|error| error.to_string())
    }

    /// A pasted code's comparison: the same on both devices, another for
    /// any other code, and the same on every platform (the vector).
    #[test]
    fn a_pasted_code_is_compared_before_it_is_used() {
        let offer = Offer { device: device(), one_time: [7; 32], secret: [8; 32] };
        // Any platform: SHA-256 of the label and this code, the first 16
        // bytes as a big-endian number, mod 10^20, in groups of five.
        let code = "claktory-dm-add:1:BAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgIBgYGBgYGBgYGBgYGBgYGBgYGBgYGBgYGBgYGBgYGBgYrQlFVRkJRVUZCUVVGQlFVRkJRVUZCUVVGQlFVRkJRVUZCUVVGQlFVRkJRVQtNYXR0J3MgaVBhZAZpUGFkT1M";
        assert_eq!(offer.qr(), code);
        assert_eq!(offer.comparison(), "51550 99913 70850 18796");
        let comparison = offer.comparison();
        assert_eq!(comparison.len(), 23);
        assert!(comparison.split(' ').all(|group| group.len() == 5 && group.bytes().all(|b| b.is_ascii_digit())));
        assert_eq!(Offer::from_qr(&format!("  {}\n", offer.qr())).unwrap().comparison(), comparison);
        let swapped = Offer { device: device(), one_time: [9; 32], secret: [8; 32] };
        assert_ne!(swapped.comparison(), comparison);
        let renamed = Offer { device: OfferFor { name: "Matt's iPad 2".to_owned(), ..device() }, one_time: [7; 32], secret: [8; 32] };
        assert_ne!(renamed.comparison(), comparison);
    }

    #[test]
    fn only_the_device_that_showed_the_code_opens_what_it_gets() {
        let identity = IdentityKey::generate().unwrap();
        let mut pins = Pins::default();
        pins.first_contact("ana", IdentityKey::generate().unwrap().public()).unwrap();

        let waiting = Waiting::new(device()).unwrap();
        let scanned = Offer::from_qr(&waiting.offer.qr()).unwrap();
        assert_eq!(scanned, waiting.offer);
        assert_eq!((scanned.device.name.as_str(), scanned.device.platform.as_str()), ("Matt's iPad", "iPadOS"));
        let sealed = seal(&scanned, "matt", &identity, &pins).unwrap();
        let got = waiting.open(&sealed.bytes).unwrap();
        assert_eq!((got.account_id.as_str(), got.identity.public()), ("matt", identity.public()));
        assert_eq!(got.pins, pins, "pins come from the person's own device (M3)");
        assert_eq!(got.transfer_key, sealed.transfer_key, "both have the transfer key");

        // The service knows the one-time key (it relayed nothing else) but
        // not the secret: what it seals isn't opened (H3).
        let waiting = Waiting::new(device()).unwrap();
        let mut guessed = waiting.offer.clone();
        guessed.secret = [0; 32];
        let forged = seal(&guessed, "matt", &IdentityKey::generate().unwrap(), &Pins::default()).unwrap();
        assert_eq!(sealed_error(waiting.open(&forged.bytes)), Some(Error::Sealed.to_string()));

        // A bundle for another offer, a changed one, or one sealed for an
        // offer changed on the way (another endpoint, name or leaf), isn't
        // opened.
        let (first, second) = (Waiting::new(device()).unwrap(), Waiting::new(device()).unwrap());
        let for_first = seal(&first.offer, "matt", &identity, &pins).unwrap();
        assert_eq!(sealed_error(second.open(&for_first.bytes)), Some(Error::Sealed.to_string()));
        let mut changed = seal(&first.offer, "matt", &identity, &pins).unwrap().bytes;
        let last = changed.len() - 1;
        changed[last] ^= 1;
        assert_eq!(sealed_error(first.open(&changed)), Some(Error::Sealed.to_string()));
        let changes: [fn(&mut Offer); 3] = [
            |offer| offer.device.endpoint_id = [7; 32],
            |offer| offer.device.name = "Ana's iPad".to_owned(),
            |offer| offer.device.leaf_id = URL_SAFE_NO_PAD.encode([8u8; 32]),
        ];
        for change in changes {
            let waiting = Waiting::new(device()).unwrap();
            let mut swapped = waiting.offer.clone();
            change(&mut swapped);
            let sealed = seal(&swapped, "matt", &identity, &pins).unwrap();
            assert_eq!(sealed_error(waiting.open(&sealed.bytes)), Some(Error::Sealed.to_string()));
        }
        assert!(Offer::from_qr("claktory-dm-add:1:AAAA").is_err());
    }

    #[test]
    fn an_offer_says_who_the_device_is_in_one_line() {
        for bad in [
            OfferFor { leaf_id: "short".to_owned(), ..device() },
            OfferFor { name: "iPad\nAdd Ana".to_owned(), ..device() },
            OfferFor { name: "\u{202e}daPi".to_owned(), ..device() },
            OfferFor { platform: "x".repeat(MAX_PLATFORM + 1), ..device() },
            OfferFor { name: String::new(), ..device() },
        ] {
            assert!(Waiting::new(bad).is_err());
        }
        // A code with a name that was never checked doesn't read either.
        let waiting = Waiting::new(device()).unwrap();
        let mut forged = waiting.offer.clone();
        forged.device.name = "iPad\u{200b}".to_owned();
        assert!(Offer::from_qr(&forged.qr()).is_err());
    }

    /// After a rotation the new key reaches the person's other devices
    /// sealed under a key of its own (that key goes in the self group):
    /// only for their account, only with pins already on the new key, and
    /// nothing opens under another key.
    #[test]
    fn a_new_identity_reaches_the_persons_other_devices_sealed() {
        let (old, new) = (IdentityKey::generate().unwrap(), IdentityKey::generate().unwrap());
        let mut pins = Pins::default();
        pins.first_contact("matt", old.public()).unwrap();
        assert_eq!(
            sealed_error(seal_rotation("matt", &new, &pins)),
            Some("refused: the pins don't have this account on the new key".to_owned()),
            "the pins move first"
        );
        pins.key_changed("matt", new.public()).unwrap();
        let sealed = seal_rotation("matt", &new, &pins).unwrap();
        let opened = open_rotation("matt", &sealed.key, &sealed.bytes).unwrap();
        assert_eq!(opened.identity.public(), new.public());
        assert!(opened.pins.get("matt").unwrap().retired.contains(&old.public()));
        // Another account's device, another key or changed bytes: nothing.
        assert_eq!(sealed_error(open_rotation("ana", &sealed.key, &sealed.bytes)), Some("a sealed value didn't open".to_owned()));
        assert_eq!(sealed_error(open_rotation("matt", &[9; 32], &sealed.bytes)), Some("a sealed value didn't open".to_owned()));
        let mut changed = sealed.bytes.clone();
        changed[0] ^= 1;
        assert!(open_rotation("matt", &sealed.key, &changed).is_err());
        // Each seal has its own key.
        assert_ne!(seal_rotation("matt", &new, &pins).unwrap().key.as_slice(), sealed.key.as_slice());
    }
}
