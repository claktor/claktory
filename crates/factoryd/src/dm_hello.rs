//! One of the person's Claktories that isn't their home introduces itself
//! to the home over `claktory-dm/1` (#451 H2; Fugo 32512), so the home can
//! send it the messages of the groups its leaf is in, and take its own.
//!
//! - **What it proves.** The Claktory's leaf is on the person's current
//!   signed device list (as the home's own leaf verified it), of the
//!   Claktory kind, and its key signed this hello: MLS's SignWithLabel
//!   (label "claktory-dm hello") over the home's endpoint id, the endpoint
//!   it dials from, the time, the account and the leaf. The endpoint the
//!   home binds is the connection's own, never one the hello names, so a
//!   hello replayed from another endpoint fails.
//! - **Fresh.** Within 5 minutes of the home's clock, and newer than the
//!   last hello the home took for that leaf, so an older one can't take a
//!   moved Claktory's place back.
//! - **Before it's known.** A stranger gets one stream, 5 seconds and 4 KiB
//!   for its hello, read strictly; at most 16 strangers at once and 30
//!   hellos a minute in all. A refusal is only a close, whatever failed;
//!   the log says the leaf and why, never the signature. An endpoint
//!   already bound is let in as a route, without waiting on those limits.
//!
//! A friend's new home introduces itself the same way, with its person's
//! signed device list naming it their home (`HomeList`, row 595, Fugo
//! 41473): the home takes it under its leaf's pin for that friend (from the
//! person's own devices) and moves only the groups already routed to that
//! friend there, so a friend who moved while all the person's devices were
//! off is still reached. Within the same limits; its list may be 16 KiB.

use std::collections::VecDeque;
use std::sync::Mutex;
use std::time::Duration;

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD as B64;
use claktory_dm_crypto::certificate::LeafKind;
use claktory_dm_crypto::device_list::SignedDeviceList;
use claktory_dm_crypto::group::LeafKey;
use claktory_dm_crypto::validation::Pins;
use openmls::ciphersuite::signature::SignContent;
use openmls::prelude::tls_codec::Serialize as _;
use serde::{Deserialize, Serialize};

use crate::factory::Factory;
use crate::locking::lock;
use crate::user_apps::dms::{home, leaf};

/// SignWithLabel's label for a hello (RFC 9420 §5.1.2).
const LABEL: &str = "claktory-dm hello";
/// How far a hello's time may be from the home's.
const SKEW_MS: i64 = 5 * 60_000;
/// How long a stranger has to say hello after connecting.
pub(crate) const DEADLINE: Duration = Duration::from_secs(5);
/// The most a hello may be.
pub(crate) const MOST: usize = 4 * 1024;
/// Strangers connected at once, before their hello.
pub(crate) const STRANGERS_AT_ONCE: usize = 16;
/// Hellos taken a minute, from everyone.
const PER_MINUTE: usize = 30;

/// A hello, as it's sent: the first frame of a stranger's first stream.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Hello {
    /// Always "hello".
    pub kind: String,
    pub account_id: String,
    pub leaf_id: String,
    pub at_ms: i64,
    /// The leaf key's SignWithLabel over `content`, base64url.
    pub signature: String,
}

/// The most a friend's new home's `HomeList` may be (a list of 64 leaves,
/// in base64).
pub(crate) const HOME_LIST_MOST: usize = 16 * 1024;

/// A friend's new home, as it introduces itself: its person's latest signed
/// device list (base64url), which names the endpoint it dials from as their
/// home. Also a frame of `claktory-dm/1` (its kind is the request's).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct HomeList {
    /// Always "home_list".
    pub kind: String,
    pub list: String,
}

/// Why a friend's new home wasn't taken: for the log only.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum NotTaken {
    Malformed,
    /// No leaf here yet: the person's device routes it, as before.
    Unready,
    /// No pin for that account here.
    NoPin,
    /// The person hasn't seen that friend's key change yet.
    ChangedUnseen,
    /// Not from that friend's pinned key, or not for its account.
    NotTheirs,
    /// The list names another home than the endpoint dialling.
    NotThatHome,
    /// Older than a list seen for that friend before.
    Older,
    /// The person's own account (their devices route it).
    Own,
    /// No group routed to that friend here.
    NotRouted,
}

/// Checks a friend's new home's `HomeList` from the endpoint `from` (the
/// connection's) and, taken, moves the groups routed to that friend there
/// (`home::friend_moved`): the friend's account, and the endpoints moved
/// off. Taken only under the leaf's pin for that friend, from the person's
/// own devices: not a key change they haven't seen, not a previous or
/// retired key, never older than the pin's list or one taken here before,
/// naming exactly `from` its home.
pub(crate) fn hear_home(factory: &Factory, from: &[u8; 32], proof: &HomeList) -> Result<(String, Vec<String>), NotTaken> {
    if proof.kind != "home_list" {
        return Err(NotTaken::Malformed);
    }
    let bytes = B64.decode(&proof.list).map_err(|_| NotTaken::Malformed)?;
    let list = SignedDeviceList::from_bytes(&bytes).map_err(|_| NotTaken::NotTheirs)?;
    let floor = home::friend_floor(factory, &list.list.account_id).map_err(|_| NotTaken::Unready)?;
    let (account, version) = leaf::step(factory, |step| {
        let ours = step.leaf()?;
        let checked = check_home_list(&ours.pins, &ours.account_id, from, &bytes, floor);
        // Kept, as the leaf keeps any list it takes.
        if checked.is_ok() {
            step.accept_list(&list.list.account_id, &bytes)?;
        }
        Ok(checked)
    })
    .map_err(|_| NotTaken::Unready)??;
    match home::friend_moved(factory, &account, &crate::encoding::hex(from), version) {
        Ok(Ok(old)) => Ok((account, old)),
        Ok(Err(home::NotMoved::Own)) => Err(NotTaken::Own),
        Ok(Err(home::NotMoved::NotRouted)) => Err(NotTaken::NotRouted),
        Ok(Err(home::NotMoved::Older)) => Err(NotTaken::Older),
        Err(_) => Err(NotTaken::Unready),
    }
}

/// Whether `pins` (the leaf's, `own` its account) take `bytes`, a friend's
/// signed device list shown by the endpoint `from`, as naming `from` that
/// friend's home, no older than `floor`: the friend's account and the list's
/// version.
fn check_home_list(pins: &Pins, own: &str, from: &[u8; 32], bytes: &[u8], floor: u64) -> Result<(String, u64), NotTaken> {
    let list = SignedDeviceList::from_bytes(bytes).map_err(|_| NotTaken::NotTheirs)?;
    if list.home != Some(*from) {
        return Err(NotTaken::NotThatHome);
    }
    let account = list.list.account_id.clone();
    if account == own {
        return Err(NotTaken::Own);
    }
    let pin = pins.get(&account).ok_or(NotTaken::NoPin)?;
    if pin.changed_unseen {
        return Err(NotTaken::ChangedUnseen);
    }
    if list.identity != pin.identity || pin.replaced(&list.identity) {
        return Err(NotTaken::NotTheirs);
    }
    if list.list.version < pin.highest_version.max(floor) {
        return Err(NotTaken::Older);
    }
    // As the leaf takes any list: never another list under the same version.
    pins.clone().accept_list(&account, bytes).map_err(|_| NotTaken::NotTheirs)?;
    Ok((account, list.list.version))
}

/// The `HomeList` this Claktory, the person's home, shows a friend's home:
/// the person's signed device list as its leaf took it, if it names this
/// Claktory's endpoint their home.
pub(crate) fn home_list(factory: &Factory, here: &[u8; 32]) -> Option<HomeList> {
    let list = leaf::step(factory, |step| step.own_list()).ok()??;
    if list.home != Some(*here) {
        return None;
    }
    Some(HomeList { kind: "home_list".to_owned(), list: B64.encode(list.to_bytes().ok()?) })
}

/// What the home keeps of hellos while it runs.
pub(crate) struct State {
    /// Strangers connected now, before their hello.
    pub(crate) strangers: std::sync::Arc<tokio::sync::Semaphore>,
    /// When the hellos of the last minute came.
    heard: Mutex<VecDeque<i64>>,
}

impl Default for State {
    fn default() -> State {
        State { strangers: std::sync::Arc::new(tokio::sync::Semaphore::new(STRANGERS_AT_ONCE)), heard: Mutex::new(VecDeque::new()) }
    }
}

/// Whether one more hello may be heard now (and counts it).
pub(crate) fn may_hear(factory: &Factory, now: i64) -> bool {
    let mut heard = lock(&factory.dm_hello.heard);
    while heard.front().is_some_and(|at| *at <= now - 60_000) {
        heard.pop_front();
    }
    if heard.len() >= PER_MINUTE {
        return false;
    }
    heard.push_back(now);
    true
}

/// What the leaf signs: the home it says hello to, the endpoint it says it
/// from, when, and who.
fn content(home: &[u8; 32], from: &[u8; 32], at_ms: i64, account: &str, leaf: &str) -> Vec<u8> {
    let mut out = Vec::with_capacity(80 + account.len() + leaf.len());
    out.extend_from_slice(home);
    out.extend_from_slice(from);
    out.extend_from_slice(&at_ms.to_be_bytes());
    for part in [account, leaf] {
        out.extend_from_slice(&u16::try_from(part.len()).unwrap_or(u16::MAX).to_be_bytes());
        out.extend_from_slice(part.as_bytes());
    }
    out
}

fn labeled(content: Vec<u8>) -> Result<Vec<u8>, String> {
    SignContent::new(LABEL, content.into()).tls_serialize_detached().map_err(|error| error.to_string())
}

fn sign(key: &LeafKey, content: Vec<u8>) -> Result<Vec<u8>, String> {
    use ed25519_dalek::Signer as _;
    let key = ed25519_dalek::SigningKey::from_bytes(&key.secret());
    Ok(key.sign(&labeled(content)?).to_bytes().to_vec())
}

fn verify(public: &[u8; 32], content: Vec<u8>, signature: &[u8]) -> bool {
    let Ok(key) = ed25519_dalek::VerifyingKey::from_bytes(public) else { return false };
    let Ok(signature) = ed25519_dalek::Signature::from_slice(signature) else { return false };
    let Ok(signed) = labeled(content) else { return false };
    key.verify_strict(&signed, &signature).is_ok()
}

/// The hello this Claktory's leaf says to its home at `home` from `from`.
pub(crate) fn make(factory: &Factory, home: &[u8; 32], from: &[u8; 32], at_ms: i64) -> Result<Hello, String> {
    let leaf = leaf::step(factory, |step| step.leaf()).map_err(|error| format!("no leaf to say hello with: {error:?}"))?;
    let leaf_id = leaf.me.certificate.leaf.leaf_id.clone();
    let signature = sign(&leaf.me.key, content(home, from, at_ms, &leaf.account_id, &leaf_id))?;
    Ok(Hello { kind: "hello".to_owned(), account_id: leaf.account_id, leaf_id, at_ms, signature: B64.encode(signature) })
}

/// A hello signed by `key` as `account`'s `leaf`, as it goes on the wire
/// (for tests: a Claktory's own comes from `make`).
pub(crate) fn signed(key: &LeafKey, home: &[u8; 32], from: &[u8; 32], at_ms: i64, account: &str, leaf: &str) -> serde_json::Value {
    let signature = sign(key, content(home, from, at_ms, account, leaf)).map(|signature| B64.encode(signature)).unwrap_or_default();
    serde_json::json!({"kind": "hello", "account_id": account, "leaf_id": leaf, "at_ms": at_ms, "signature": signature})
}

/// Why a hello was refused: for the log only.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Refused {
    Malformed,
    NotOurs,
    Stale,
    NoList,
    UnknownLeaf,
    NotAClaktory,
    Signature,
    Older,
    Unready,
}

/// Checks `hello` from the endpoint `from` (the connection's) against the
/// person's current list and binds that endpoint to its leaf: the leaf.
pub(crate) fn hear(factory: &Factory, here: &[u8; 32], from: &[u8; 32], hello: &Hello, now: i64) -> Result<String, Refused> {
    if hello.kind != "hello" {
        return Err(Refused::Malformed);
    }
    let ours = leaf::step(factory, |step| step.leaf()).map_err(|_| Refused::Unready)?;
    if hello.account_id != ours.account_id {
        return Err(Refused::NotOurs);
    }
    if (hello.at_ms - now).abs() > SKEW_MS {
        return Err(Refused::Stale);
    }
    let list = ours
        .pins
        .get(&ours.account_id)
        .and_then(|pin| pin.device_list.as_deref())
        .and_then(|bytes| SignedDeviceList::from_bytes(bytes).ok())
        .ok_or(Refused::NoList)?;
    let listed = list.list.leaves.iter().find(|listed| listed.leaf_id == hello.leaf_id).ok_or(Refused::UnknownLeaf)?;
    if listed.kind != LeafKind::Claktory || listed.leaf_id == ours.me.certificate.leaf.leaf_id {
        return Err(Refused::NotAClaktory);
    }
    let signature = B64.decode(&hello.signature).map_err(|_| Refused::Malformed)?;
    if !verify(&listed.signature_key, content(here, from, hello.at_ms, &hello.account_id, &hello.leaf_id), &signature) {
        return Err(Refused::Signature);
    }
    match home::bind(factory, &hello.leaf_id, &crate::encoding::hex(from), hello.at_ms) {
        Ok(true) => Ok(hello.leaf_id.clone()),
        Ok(false) => Err(Refused::Older),
        Err(_) => Err(Refused::Unready),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_hello_verifies_only_for_its_own_endpoints_time_and_leaf() {
        let key = LeafKey::generate().expect("a key");
        let (home, from) = ([1; 32], [2; 32]);
        let signed = sign(&key, content(&home, &from, 1000, "ann", "ann-mac")).expect("signed");
        assert!(verify(&key.public(), content(&home, &from, 1000, "ann", "ann-mac"), &signed));
        // Replayed from another endpoint, to another home, at another time,
        // or for another leaf: it doesn't verify.
        assert!(!verify(&key.public(), content(&home, &[3; 32], 1000, "ann", "ann-mac"), &signed));
        assert!(!verify(&key.public(), content(&[3; 32], &from, 1000, "ann", "ann-mac"), &signed));
        assert!(!verify(&key.public(), content(&home, &from, 1001, "ann", "ann-mac"), &signed));
        assert!(!verify(&key.public(), content(&home, &from, 1000, "ann", "ann-pc"), &signed));
        assert!(!verify(&LeafKey::generate().expect("a key").public(), content(&home, &from, 1000, "ann", "ann-mac"), &signed));
        // The label separates it: the same key over the bare content isn't it.
        use ed25519_dalek::Signer as _;
        let bare = ed25519_dalek::SigningKey::from_bytes(&key.secret()).sign(&content(&home, &from, 1000, "ann", "ann-mac")).to_bytes();
        assert!(!verify(&key.public(), content(&home, &from, 1000, "ann", "ann-mac"), &bare));
    }

    /// Row 595 (Fugo 41473): a friend's new home is taken only from that
    /// friend's list under the pin the person's devices gave, current and
    /// seen, never older than what was taken, naming the endpoint dialling.
    #[test]
    fn a_friends_new_home_is_taken_only_from_their_list_under_the_pin() {
        use claktory_dm_crypto::device_list::{DeviceList, Listed};
        use claktory_dm_crypto::identity::IdentityKey;
        let (bo, ann) = (IdentityKey::generate().expect("a key"), IdentityKey::generate().expect("a key"));
        let (new_home, other) = ([7; 32], [8; 32]);
        let list = |identity: &IdentityKey, account: &str, version: u64, home: [u8; 32]| {
            let leaves = vec![Listed { kind: LeafKind::Device, leaf_id: "phone".to_owned(), signature_key: [1; 32] }];
            DeviceList { account_id: account.to_owned(), version, created_ms: 1, leaves }
                .sign_with(identity, Some(home))
                .expect("signed")
                .to_bytes()
                .expect("bytes")
        };
        let mut pins = Pins::default();
        pins.first_contact("ann", ann.public()).expect("pinned");
        pins.first_contact("bo", bo.public()).expect("pinned");
        pins.accept_list("bo", &list(&bo, "bo", 4, other)).expect("taken");

        assert_eq!(check_home_list(&pins, "ann", &new_home, &list(&bo, "bo", 5, new_home), 0), Ok(("bo".to_owned(), 5)));
        // The same version as the pin's, the list it holds: still taken.
        assert_eq!(check_home_list(&pins, "ann", &other, &list(&bo, "bo", 4, other), 0), Ok(("bo".to_owned(), 4)));
        // One version older than the pin's, or than one taken here before.
        assert_eq!(check_home_list(&pins, "ann", &new_home, &list(&bo, "bo", 3, new_home), 0), Err(NotTaken::Older));
        assert_eq!(check_home_list(&pins, "ann", &new_home, &list(&bo, "bo", 5, new_home), 6), Err(NotTaken::Older));
        // Another list under the version the pin holds.
        assert_eq!(check_home_list(&pins, "ann", &new_home, &list(&bo, "bo", 4, new_home), 0), Err(NotTaken::NotTheirs));
        // Dialled from another endpoint than the home it names.
        assert_eq!(check_home_list(&pins, "ann", &other, &list(&bo, "bo", 5, new_home), 0), Err(NotTaken::NotThatHome));
        // Another key's (a stranger's, or the service's), the person's own,
        // or an account with no pin.
        let stranger = IdentityKey::generate().expect("a key");
        assert_eq!(check_home_list(&pins, "ann", &new_home, &list(&stranger, "bo", 5, new_home), 0), Err(NotTaken::NotTheirs));
        assert_eq!(check_home_list(&pins, "ann", &new_home, &list(&ann, "ann", 5, new_home), 0), Err(NotTaken::Own));
        assert_eq!(check_home_list(&pins, "ann", &new_home, &list(&stranger, "cy", 5, new_home), 0), Err(NotTaken::NoPin));

        // A key change the person hasn't seen: not now. Seen, the new key's
        // list is taken; the key it moved off (previous, or retired) never.
        let next = IdentityKey::generate().expect("a key");
        let mut changed = pins.clone();
        changed.key_changed("bo", next.public()).expect("changed");
        assert_eq!(check_home_list(&changed, "ann", &new_home, &list(&next, "bo", 1, new_home), 0), Err(NotTaken::ChangedUnseen));
        changed.seen("bo");
        assert_eq!(check_home_list(&changed, "ann", &new_home, &list(&next, "bo", 1, new_home), 0), Ok(("bo".to_owned(), 1)));
        assert_eq!(check_home_list(&changed, "ann", &new_home, &list(&bo, "bo", 9, new_home), 0), Err(NotTaken::NotTheirs));
        changed.retire_previous("bo");
        assert_eq!(check_home_list(&changed, "ann", &new_home, &list(&bo, "bo", 9, new_home), 0), Err(NotTaken::NotTheirs));
    }

    #[test]
    fn a_hello_is_read_strictly() {
        let hello = r#"{"kind":"hello","account_id":"ann","leaf_id":"x","at_ms":1,"signature":"AA"}"#;
        assert!(serde_json::from_str::<Hello>(hello).is_ok());
        let more = r#"{"kind":"hello","account_id":"ann","leaf_id":"x","at_ms":1,"signature":"AA","endpoint":"ff"}"#;
        assert!(serde_json::from_str::<Hello>(more).is_err(), "an unknown field");
    }
}
