//! Leaves, end to end (E2; README "End to end"): each account's identity
//! key and signed device list, and each leaf's KeyPackages.
//!
//! The worker keeps them as signed data it can't forge. It checks what it
//! can, to keep garbage out: a device list is signed by the identity key
//! the account first uploaded (trust on first use at the service; clients
//! pin for themselves), a KeyPackage is valid, for suite 1, and carries the
//! uploading leaf's own certificate from the account's current identity,
//! named by its current device list. Whether a friend trusts any of it is
//! the friend's client's to decide.

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use claktory_dm_crypto::certificate::{LeafCertificate, LeafKind, claktory_leaf_id};
use claktory_dm_crypto::device_list::SignedDeviceList;
use claktory_dm_crypto::envelope::{self, Envelope, Kind as MlsKind};
use claktory_dm_crypto::identity::IdentityPublic;
use openmls::prelude::{KeyPackage, KeyPackageIn, MlsMessageBodyIn, ProtocolVersion};
use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};
use serde::Deserialize;
use serde_json::{Value, json};

use super::identity::{Refusal, body, invalid, take};
use super::sync::{self, Kind};
use crate::grants;
use crate::ipc::Caller;

pub const SCHEMA: &str = "
-- Each account's identity key, as it first uploaded it or last rotated it,
-- and its signed device list (the version inside, which only goes up).
CREATE TABLE IF NOT EXISTS dm_identities (
    account_id TEXT PRIMARY KEY,
    identity_key BLOB NOT NULL,
    device_list BLOB NOT NULL,
    version INTEGER NOT NULL,
    updated INTEGER NOT NULL
);
-- Each leaf's KeyPackages, by their reference (OpenMLS's hash): one-time
-- ones, and at most one last-resort one.
CREATE TABLE IF NOT EXISTS dm_key_packages (
    ref BLOB PRIMARY KEY,
    account_id TEXT NOT NULL,
    leaf_id TEXT NOT NULL,
    kind TEXT NOT NULL,
    last_resort INTEGER NOT NULL,
    key_package BLOB NOT NULL,
    uploaded INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS dm_key_packages_leaf ON dm_key_packages (account_id, leaf_id, last_resort, uploaded);
-- KeyPackages handed out, so a Welcome to one reaches its leaf.
CREATE TABLE IF NOT EXISTS dm_claims (
    ref BLOB PRIMARY KEY,
    account_id TEXT NOT NULL,
    leaf_id TEXT NOT NULL,
    claimed INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS dm_claims_claimed ON dm_claims (claimed);
";

/// One-time KeyPackages a leaf keeps at most.
pub const KEY_PACKAGES_MOST: i64 = 100;
/// Claims per claiming device an hour, and identity rotations per account a day.
const CLAIMS_AN_HOUR: i64 = 60;
const ROTATIONS_A_DAY: i64 = 3;
/// How long a claim is remembered, for its Welcome.
pub const CLAIM_KEEP_MS: i64 = 30 * 86_400_000;

/// The caller as a leaf: whose, its id and kind. A device is its account
/// key; a Claktory its id and registration epoch, for itself, never for
/// an agent.
pub struct Leaf {
    pub account: String,
    pub leaf_id: String,
    pub kind: LeafKind,
}

pub fn leaf_of(caller: &Caller) -> Result<Leaf, Refusal> {
    match caller {
        Caller::Device { account_id, device_key, .. } => {
            Ok(Leaf { account: account_id.clone(), leaf_id: device_key.clone(), kind: LeafKind::Device })
        }
        Caller::Backend { account_id, claktory_id, epoch, agent: None } => {
            Ok(Leaf { account: account_id.clone(), leaf_id: claktory_leaf_id(claktory_id, *epoch), kind: LeafKind::Claktory })
        }
        Caller::Backend { .. } => Err(Refusal(403, "claktory_only", "Only the Claktory itself, not an agent, can do this.".to_owned())),
        Caller::Account { .. } => Err(device_only()),
    }
}

fn device_only() -> Refusal {
    Refusal(403, "device_only", "Only your own signed-in devices can do this.".to_owned())
}

fn not_found() -> Refusal {
    Refusal(404, "not_found", "There's nothing here.".to_owned())
}

fn not_set_up() -> Refusal {
    Refusal(409, "not_set_up", "Set up messages on one of your devices first.".to_owned())
}

pub(super) fn b64(bytes: &[u8]) -> String {
    URL_SAFE_NO_PAD.encode(bytes)
}

pub(super) fn unb64(text: &str, what: &str) -> Result<Vec<u8>, Refusal> {
    URL_SAFE_NO_PAD.decode(text).map_err(|_| invalid(&format!("{what} is base64url without padding.")))
}

/// What the worker uses OpenMLS's cryptography for: checking, never
/// opening. One for the process (the worker never forks).
pub(super) fn crypto() -> &'static claktory_dm_crypto::provider::Crypto {
    static CRYPTO: std::sync::OnceLock<claktory_dm_crypto::provider::Crypto> = std::sync::OnceLock::new();
    CRYPTO.get_or_init(|| claktory_dm_crypto::provider::Crypto::new().expect("the system's random source"))
}

/// An account's keys as kept.
pub(super) struct Keys {
    pub identity: IdentityPublic,
    pub list: SignedDeviceList,
    raw_list: Vec<u8>,
}

pub(super) fn keys_of(db: &Connection, account: &str) -> Result<Option<Keys>, Refusal> {
    let row: Option<(Vec<u8>, Vec<u8>)> = db
        .query_row("SELECT identity_key, device_list FROM dm_identities WHERE account_id = ?1", [account], |row| {
            Ok((row.get(0)?, row.get(1)?))
        })
        .optional()?;
    let Some((identity, raw_list)) = row else { return Ok(None) };
    let identity = IdentityPublic::from_slice(&identity).map_err(|_| broken())?;
    let list = SignedDeviceList::from_bytes(&raw_list).map_err(|_| broken())?;
    Ok(Some(Keys { identity, list, raw_list }))
}

fn broken() -> Refusal {
    tracing::error!("dms: a stored identity or device list doesn't read");
    Refusal(503, "unavailable", "Something went wrong. Try again later.".to_owned())
}

fn shown(account: &str, keys: &Keys) -> Value {
    json!({
        "account_id": account,
        "identity_key": b64(&keys.identity.0),
        "device_list": b64(&keys.raw_list),
        "version": keys.list.list.version,
    })
}

/// Whether `a` and `b` are or were friends.
fn ever_friends(db: &Connection, a: &str, b: &str) -> rusqlite::Result<bool> {
    let (low, high) = if a < b { (a, b) } else { (b, a) };
    db.query_row("SELECT COUNT(*) > 0 FROM dm_contacts WHERE low = ?1 AND high = ?2", params![low, high], |row| row.get(0))
}

/// Whether `a` and `b` are friends now.
fn friends(db: &Connection, a: &str, b: &str) -> rusqlite::Result<bool> {
    let (low, high) = if a < b { (a, b) } else { (b, a) };
    db.query_row("SELECT COUNT(*) > 0 FROM dm_contacts WHERE low = ?1 AND high = ?2 AND ended IS NULL", params![low, high], |row| {
        row.get(0)
    })
}

/// The conversations a Claktory's agents hold a live DM grant on, under
/// its registration now.
pub(super) fn granted_conversations(db: &Connection, caller: &Caller, now_ms: i64) -> rusqlite::Result<Vec<String>> {
    let Caller::Backend { account_id, claktory_id, epoch, .. } = caller else { return Ok(Vec::new()) };
    let mut conversations: Vec<String> = grants::of_claktory(db, account_id, claktory_id, *epoch, "dms", now_ms)?
        .into_iter()
        .flat_map(|grant| grant.resources)
        .filter(|resource| resource.kind == "conversation")
        .map(|resource| resource.id)
        .collect();
    conversations.sort();
    conversations.dedup();
    Ok(conversations)
}

/// Whether `caller` may read `account`'s keys: its own account's, a
/// friend's (now or before) for a device, and for a Claktory the friend in
/// a conversation one of its agents holds a grant on.
fn may_read_keys(db: &Connection, caller: &Caller, account: &str, now_ms: i64) -> Result<bool, Refusal> {
    if caller.account_id() == account {
        return Ok(matches!(caller, Caller::Device { .. } | Caller::Backend { agent: None, .. }));
    }
    match caller {
        Caller::Device { account_id, .. } => Ok(ever_friends(db, account_id, account)?),
        Caller::Backend { account_id, agent: None, .. } => {
            for conversation in granted_conversations(db, caller, now_ms)? {
                let pair: Option<(String, String)> = db
                    .query_row("SELECT low, high FROM dm_contacts WHERE conversation_id = ?1", [&conversation], |row| {
                        Ok((row.get(0)?, row.get(1)?))
                    })
                    .optional()?;
                if pair.is_some_and(|(low, high)| (low == *account_id && high == account) || (high == *account_id && low == account)) {
                    return Ok(true);
                }
            }
            Ok(false)
        }
        _ => Ok(false),
    }
}

/// Tells the account's own devices, and its friends', that its keys
/// changed.
fn tell(transaction: &Transaction<'_>, account: &str, now_ms: i64) -> rusqlite::Result<()> {
    sync::emit(transaction, account, Kind::Keys, "", account, 0, now_ms)?;
    let friends: Vec<String> = {
        let mut statement = transaction
            .prepare("SELECT CASE WHEN low = ?1 THEN high ELSE low END FROM dm_contacts WHERE (low = ?1 OR high = ?1) AND ended IS NULL")?;
        statement.query_map([account], |row| row.get(0))?.collect::<Result<_, _>>()?
    };
    for friend in friends {
        sync::emit(transaction, &friend, Kind::Keys, "", account, 0, now_ms)?;
    }
    Ok(())
}

/// A device list from a body, signed by the identity inside it, for
/// `account`: or why not.
fn list_for(account: &str, text: &str) -> Result<(SignedDeviceList, Vec<u8>), Refusal> {
    let raw = unb64(text, "device_list")?;
    let list = SignedDeviceList::from_bytes(&raw).map_err(|error| invalid(&format!("That isn't a signed device list: {error}.")))?;
    if list.list.account_id != account {
        return Err(invalid("That device list is another account's."));
    }
    Ok((list, raw))
}

/// Drops the KeyPackages of leaves the account's list no longer names, or
/// that another identity certified.
fn keep_listed(transaction: &Transaction<'_>, account: &str, list: &SignedDeviceList) -> Result<(), Refusal> {
    let stored: Vec<(Vec<u8>, Vec<u8>)> = {
        let mut statement = transaction.prepare("SELECT ref, key_package FROM dm_key_packages WHERE account_id = ?1")?;
        statement.query_map([account], |row| Ok((row.get(0)?, row.get(1)?)))?.collect::<Result<_, _>>()?
    };
    for (reference, bytes) in stored {
        let named = key_package(&bytes).ok().and_then(|(package, _)| certificate(&package).ok()).is_some_and(|cert| list.names(&cert));
        if !named {
            transaction.execute("DELETE FROM dm_key_packages WHERE ref = ?1", [reference])?;
        }
    }
    Ok(())
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PutKeys {
    identity_key: Option<String>,
    device_list: String,
    version: u64,
    previous_version: u64,
    #[serde(default)]
    #[allow(dead_code)]
    request_nonce: Option<String>,
}

/// The account's keys as stored now; or, inner `Err`, the 409 answer for a
/// device whose `previous_version` is stale, with the keys as they are.
fn put_keys(transaction: &Transaction<'_>, account: &str, put: PutKeys, now_ms: i64) -> Result<Result<Value, Value>, Refusal> {
    let (list, raw) = list_for(account, &put.device_list)?;
    if list.list.version != put.version || put.version == 0 {
        return Err(invalid("version is the device list's own, from 1."));
    }
    let current = keys_of(transaction, account)?;
    let identity = match (&current, &put.identity_key) {
        (None, Some(identity)) => {
            if put.previous_version != 0 {
                return Ok(Err(stale(Value::Null)));
            }
            IdentityPublic::from_slice(&unb64(identity, "identity_key")?).map_err(|_| invalid("identity_key isn't an Ed25519 key."))?
        }
        (None, None) => return Err(invalid("The first upload names identity_key.")),
        (Some(current), named) => {
            if put.previous_version != current.list.list.version {
                return Ok(Err(stale(shown(account, current))));
            }
            if named.as_deref().is_some_and(|named| named != b64(&current.identity.0)) {
                return Err(Refusal(
                    409,
                    "identity_changed",
                    "A new security key goes through its own step (me/keys/rotate), not this one.".to_owned(),
                ));
            }
            if list.list.version <= current.list.list.version {
                return Err(invalid("A new device list has a higher version than the one before."));
            }
            current.identity
        }
    };
    if list.identity != identity {
        return Err(invalid("That device list isn't signed by the account's identity key."));
    }
    transaction.execute(
        "INSERT INTO dm_identities (account_id, identity_key, device_list, version, updated) VALUES (?1, ?2, ?3, ?4, ?5)
         ON CONFLICT (account_id) DO UPDATE SET device_list = ?3, version = ?4, updated = ?5",
        params![account, identity.0.to_vec(), raw, list.list.version as i64, now_ms],
    )?;
    keep_listed(transaction, account, &list)?;
    tell(transaction, account, now_ms)?;
    Ok(Ok(shown(account, &Keys { identity, list, raw_list: raw })))
}

fn stale(keys: Value) -> Value {
    json!({"error": "stale", "message": "Your devices changed meanwhile. Look again, then try again.", "keys": keys})
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Rotate {
    identity_key: String,
    device_list: String,
    version: u64,
    #[serde(default)]
    #[allow(dead_code)]
    request_nonce: Option<String>,
}

fn rotate(transaction: &Transaction<'_>, account: &str, rotate: Rotate, now_ms: i64) -> Result<Value, Refusal> {
    let identity = IdentityPublic::from_slice(&unb64(&rotate.identity_key, "identity_key")?)
        .map_err(|_| invalid("identity_key isn't an Ed25519 key."))?;
    let (list, raw) = list_for(account, &rotate.device_list)?;
    if list.list.version != rotate.version || rotate.version != 1 {
        return Err(invalid("A new security key's device list starts at version 1."));
    }
    if list.identity != identity {
        return Err(invalid("That device list isn't signed by the new identity key."));
    }
    let current = keys_of(transaction, account)?.ok_or_else(not_set_up)?;
    if current.identity == identity {
        return Err(invalid("That's the key the account has."));
    }
    take(transaction, &format!("rotate:{account}"), 86_400, ROTATIONS_A_DAY, now_ms)?;
    transaction.execute(
        "UPDATE dm_identities SET identity_key = ?2, device_list = ?3, version = ?4, updated = ?5 WHERE account_id = ?1",
        params![account, identity.0.to_vec(), raw, list.list.version as i64, now_ms],
    )?;
    // Every KeyPackage the old key certified is no use to anyone now.
    keep_listed(transaction, account, &list)?;
    tell(transaction, account, now_ms)?;
    Ok(shown(account, &Keys { identity, list, raw_list: raw }))
}

/// A KeyPackage's bytes, checked: valid now, for suite 1. With its
/// reference.
fn key_package(bytes: &[u8]) -> Result<(KeyPackage, Vec<u8>), Refusal> {
    let parsed = envelope::parse(bytes).map_err(|error| invalid(&format!("That isn't a KeyPackage: {error}.")))?;
    if parsed.kind != MlsKind::KeyPackage {
        return Err(invalid("That isn't a KeyPackage."));
    }
    let MlsMessageBodyIn::KeyPackage(package) = parsed.message.extract() else { return Err(invalid("That isn't a KeyPackage.")) };
    let package: KeyPackageIn = package;
    let crypto = crypto().backend();
    let package =
        package.validate(crypto, ProtocolVersion::Mls10).map_err(|error| invalid(&format!("That KeyPackage isn't valid: {error}.")))?;
    if package.ciphersuite() != claktory_dm_crypto::CIPHERSUITE {
        return Err(invalid("KeyPackages are for MLS_128_DHKEMX25519_AES128GCM_SHA256_Ed25519."));
    }
    let reference = package.hash_ref(crypto).map_err(|_| invalid("That KeyPackage has no reference."))?;
    Ok((package, reference.as_slice().to_vec()))
}

/// The leaf certificate in a KeyPackage, for the leaf's own signature key.
fn certificate(package: &KeyPackage) -> Result<LeafCertificate, Refusal> {
    let certificate = LeafCertificate::from_credential(package.leaf_node().credential())
        .map_err(|error| invalid(&format!("That KeyPackage's credential isn't a leaf certificate: {error}.")))?;
    if certificate.leaf.signature_key.as_slice() != package.leaf_node().signature_key().as_slice() {
        return Err(invalid("That KeyPackage's certificate is for another key."));
    }
    Ok(certificate)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Upload {
    key_packages: Vec<Value>,
    last_resort: Option<Value>,
    #[serde(default)]
    #[allow(dead_code)]
    request_nonce: Option<String>,
}

fn supply(db: &Connection, leaf: &Leaf) -> rusqlite::Result<Value> {
    let (remaining, last_resort): (i64, bool) = db.query_row(
        "SELECT COALESCE(SUM(last_resort = 0), 0), COALESCE(MAX(last_resort), 0) FROM dm_key_packages WHERE account_id = ?1 AND leaf_id = ?2",
        params![leaf.account, leaf.leaf_id],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    Ok(json!({"remaining": remaining, "last_resort": last_resort}))
}

fn upload(transaction: &Transaction<'_>, leaf: &Leaf, upload: Upload, now_ms: i64) -> Result<Value, Refusal> {
    let keys = keys_of(transaction, &leaf.account)?.ok_or_else(not_set_up)?;
    let mut packages = Vec::new();
    let all = upload.key_packages.iter().map(|value| (value, false)).chain(upload.last_resort.iter().map(|value| (value, true)));
    for (value, last_resort) in all {
        let envelope = Envelope::from_json(value).map_err(|error| invalid(&format!("A KeyPackage isn't an envelope: {error}.")))?;
        let (package, reference) = key_package(envelope.bytes())?;
        let certificate = certificate(&package)?;
        let leaf_matches =
            certificate.leaf.account_id == leaf.account && certificate.leaf.leaf_id == leaf.leaf_id && certificate.leaf.kind == leaf.kind;
        if !leaf_matches {
            return Err(invalid("A leaf uploads only its own KeyPackages."));
        }
        if !keys.list.names(&certificate) {
            return Err(invalid("That leaf isn't on the account's device list."));
        }
        packages.push((reference, envelope.into_bytes(), last_resort));
    }
    let held: i64 = transaction.query_row(
        "SELECT COUNT(*) FROM dm_key_packages WHERE account_id = ?1 AND leaf_id = ?2 AND last_resort = 0",
        params![leaf.account, leaf.leaf_id],
        |row| row.get(0),
    )?;
    let one_time = packages.iter().filter(|(_, _, last_resort)| !last_resort).count() as i64;
    if held + one_time > KEY_PACKAGES_MOST {
        return Err(invalid(&format!("A leaf keeps at most {KEY_PACKAGES_MOST} KeyPackages; it has {held}.")));
    }
    let kind = match leaf.kind {
        LeafKind::Device => "device",
        LeafKind::Claktory => "claktory",
    };
    for (reference, bytes, last_resort) in packages {
        if last_resort {
            transaction.execute(
                "DELETE FROM dm_key_packages WHERE account_id = ?1 AND leaf_id = ?2 AND last_resort = 1",
                params![leaf.account, leaf.leaf_id],
            )?;
        }
        transaction.execute(
            "INSERT OR IGNORE INTO dm_key_packages (ref, account_id, leaf_id, kind, last_resort, key_package, uploaded)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![reference, leaf.account, leaf.leaf_id, kind, last_resort, bytes, now_ms],
        )?;
    }
    Ok(supply(transaction, leaf)?)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Claim {
    account_id: String,
    leaf_ids: Option<Vec<String>>,
    #[serde(default)]
    #[allow(dead_code)]
    request_nonce: Option<String>,
}

fn claim(transaction: &Transaction<'_>, caller: &Caller, claim: Claim, now_ms: i64) -> Result<Value, Refusal> {
    let Caller::Device { account_id: claimer, device_key, .. } = caller else { return Err(device_only()) };
    let own = *claimer == claim.account_id;
    if !own && !friends(transaction, claimer, &claim.account_id)? {
        return Err(not_found());
    }
    take(transaction, &format!("claim:{device_key}"), 3600, CLAIMS_AN_HOUR, now_ms)?;
    let keys = keys_of(transaction, &claim.account_id)?.ok_or_else(not_found)?;
    let mut answer = Vec::new();
    for listed in &keys.list.list.leaves {
        // A friend gets only the person's devices, never a Claktory's leaf.
        if !own && listed.kind != LeafKind::Device {
            continue;
        }
        if claim.leaf_ids.as_ref().is_some_and(|wanted| !wanted.contains(&listed.leaf_id)) {
            continue;
        }
        let found: Option<(Vec<u8>, Vec<u8>, bool)> = transaction
            .query_row(
                "SELECT ref, key_package, last_resort FROM dm_key_packages WHERE account_id = ?1 AND leaf_id = ?2
                 ORDER BY last_resort, uploaded LIMIT 1",
                params![claim.account_id, listed.leaf_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .optional()?;
        let Some((reference, bytes, last_resort)) = found else { continue };
        if !last_resort {
            transaction.execute("DELETE FROM dm_key_packages WHERE ref = ?1", [&reference])?;
        }
        transaction.execute(
            "INSERT INTO dm_claims (ref, account_id, leaf_id, claimed) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT (ref) DO UPDATE SET claimed = ?4",
            params![reference, claim.account_id, listed.leaf_id, now_ms],
        )?;
        let envelope = Envelope::new(bytes).map_err(|_| broken())?;
        answer.push(json!({"leaf_id": listed.leaf_id, "key_package": envelope.to_json(), "last_resort": last_resort}));
    }
    Ok(json!({"key_packages": answer}))
}

/// Forgets claims past their time.
pub fn sweep(db: &Connection, now_ms: i64) -> rusqlite::Result<usize> {
    db.execute("DELETE FROM dm_claims WHERE claimed < ?1", [now_ms - CLAIM_KEEP_MS])
}

pub fn knows(method: &str, route: &str) -> bool {
    matches!(
        (method, route),
        ("PUT", "me/keys")
            | ("POST", "me/keys/rotate")
            | ("POST", "key-packages")
            | ("GET", "key-packages")
            | ("POST", "key-packages/claim")
    ) || (method == "GET" && route.strip_prefix("keys/").is_some_and(|account| !account.is_empty() && !account.contains('/')))
}

pub fn answer(db: &mut Connection, method: &str, route: &str, caller: &Caller, bytes: &[u8], now_ms: i64) -> Result<(u16, Value), Refusal> {
    let transaction = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let answered = match (method, route) {
        ("GET", route) if route.starts_with("keys/") => {
            let account = &route["keys/".len()..];
            if !may_read_keys(&transaction, caller, account, now_ms)? {
                return Err(not_found());
            }
            let keys = keys_of(&transaction, account)?.ok_or_else(not_found)?;
            shown(account, &keys)
        }
        ("PUT", "me/keys") => {
            let Caller::Device { account_id, .. } = caller else { return Err(device_only()) };
            match put_keys(&transaction, account_id, body(bytes)?, now_ms)? {
                Ok(value) => value,
                Err(stale) => return Ok((409, stale)),
            }
        }
        ("POST", "me/keys/rotate") => {
            let Caller::Device { account_id, .. } = caller else { return Err(device_only()) };
            rotate(&transaction, account_id, body(bytes)?, now_ms)?
        }
        ("POST", "key-packages") => upload(&transaction, &leaf_of(caller)?, body(bytes)?, now_ms)?,
        ("GET", "key-packages") => supply(&transaction, &leaf_of(caller)?)?,
        ("POST", "key-packages/claim") => claim(&transaction, caller, body(bytes)?, now_ms)?,
        _ => return Err(not_found()),
    };
    transaction.commit()?;
    Ok((200, answered))
}

/// Deleting an account: its keys and KeyPackages go.
pub fn purge(transaction: &Transaction<'_>, account: &str) -> rusqlite::Result<()> {
    transaction.execute("DELETE FROM dm_identities WHERE account_id = ?1", [account])?;
    transaction.execute("DELETE FROM dm_key_packages WHERE account_id = ?1", [account])?;
    transaction.execute("DELETE FROM dm_claims WHERE account_id = ?1", [account])?;
    transaction.execute("DELETE FROM dm_limits WHERE key = ?1", [format!("rotate:{account}")])?;
    Ok(())
}

#[cfg(test)]
pub(super) mod tests {
    use super::super::contacts::tests::World;
    use super::super::messages::tests::friends;
    use super::*;
    use crate::grants::{Put, Resource};
    use crate::ipc::{Actor, Registration};
    use claktory_dm_crypto::certificate::Leaf as Certified;
    use claktory_dm_crypto::device_list::{DeviceList, Listed};
    use claktory_dm_crypto::group::{LeafKey, Me};
    use claktory_dm_crypto::identity::IdentityKey;
    use claktory_dm_crypto::provider::Provider;
    use claktory_dm_crypto::storage::{self as mls_storage, SealKey};

    pub(in crate::dms) fn device(account: &str, key: &str) -> Caller {
        Caller::Device { account_id: account.into(), device_key: key.into(), registrations: Vec::<Registration>::new() }
    }

    pub(in crate::dms) fn claktory(account: &str, id: &str) -> Caller {
        Caller::Backend { account_id: account.into(), claktory_id: id.into(), epoch: 1, agent: None }
    }

    pub(in crate::dms) fn call(world: &mut World, caller: &Caller, method: &str, route: &str, body: Value) -> (u16, Value) {
        let bytes = if body.is_null() { Vec::new() } else { body.to_string().into_bytes() };
        super::super::identity::route(&mut world.db, &world.keys, method, route, caller, &bytes, world.now).expect("a route")
    }

    /// A person as their devices see themselves: their identity, the
    /// version of their list, and their leaves.
    pub(in crate::dms) struct Person {
        pub account: String,
        pub identity: IdentityKey,
        pub version: u64,
        pub leaves: Vec<(Caller, Me)>,
        /// Each leaf's own MLS store (its KeyPackages' secrets, its groups).
        pub stores: Vec<Connection>,
        /// The list as last signed.
        pub signed: Option<SignedDeviceList>,
    }

    impl Person {
        pub(in crate::dms) fn new(account: &str) -> Person {
            Person {
                account: account.into(),
                identity: IdentityKey::generate().expect("a key"),
                version: 0,
                leaves: Vec::new(),
                stores: Vec::new(),
                signed: None,
            }
        }

        pub(in crate::dms) fn certify(&self, caller: &Caller, key: LeafKey) -> Me {
            let leaf = leaf_of(caller).ok().expect("a leaf");
            let certified = Certified {
                kind: leaf.kind,
                account_id: leaf.account,
                leaf_id: leaf.leaf_id,
                signature_key: key.public(),
                name: "iPad".into(),
                created_ms: NOW as u64,
            };
            Me::new(key, LeafCertificate::issue(&self.identity, certified).expect("issued")).expect("me")
        }

        /// A new leaf, not on the list until the next `keys`.
        pub(in crate::dms) fn add(&mut self, caller: Caller) -> usize {
            let me = self.certify(&caller, LeafKey::generate().expect("a key"));
            self.leaves.push((caller, me));
            let store = Connection::open_in_memory().expect("db");
            store.pragma_update(None, "secure_delete", "ON").expect("secure_delete");
            store.execute_batch(mls_storage::SCHEMA).expect("schema");
            self.stores.push(store);
            self.leaves.len() - 1
        }

        /// The list naming every leaf, at the next version.
        pub(in crate::dms) fn list(&mut self) -> String {
            self.version += 1;
            let leaves = self
                .leaves
                .iter()
                .map(|(_, me)| Listed {
                    kind: me.certificate.leaf.kind,
                    leaf_id: me.certificate.leaf.leaf_id.clone(),
                    signature_key: me.certificate.leaf.signature_key,
                })
                .collect();
            let list = DeviceList { account_id: self.account.clone(), version: self.version, created_ms: NOW as u64, leaves };
            let signed = list.sign(&self.identity).expect("signed");
            let bytes = signed.to_bytes().expect("bytes");
            self.signed = Some(signed);
            b64(&bytes)
        }

        /// `PUT me/keys`'s body for the next list.
        pub(in crate::dms) fn put(&mut self, first: bool) -> Value {
            let previous = self.version;
            let list = self.list();
            let mut put = json!({"device_list": list, "version": self.version, "previous_version": previous});
            if first {
                put["identity_key"] = json!(b64(&self.identity.public().0));
            }
            put
        }

        pub(in crate::dms) fn key_packages(&self, leaf: usize, count: usize) -> Vec<Value> {
            self.with(leaf, |provider, me| {
                (0..count).map(|_| Envelope::new(me.key_package(provider).expect("a KeyPackage")).expect("fits").to_json()).collect()
            })
        }

        /// Runs `f` as the leaf: its MLS provider and its self.
        pub(in crate::dms) fn with<R>(&self, leaf: usize, f: impl FnOnce(&Provider<'_>, &Me) -> R) -> R {
            let key = SealKey::from_bytes([1; 32]);
            let provider = Provider::new(crypto(), &self.stores[leaf], &key).expect("a provider");
            f(&provider, &self.leaves[leaf].1)
        }

        pub(in crate::dms) fn caller(&self, leaf: usize) -> Caller {
            self.leaves[leaf].0.clone()
        }
    }

    pub(in crate::dms) const NOW: i64 = super::super::contacts::tests::NOW;

    /// Ana with a device on her list, set up at the worker.
    pub(in crate::dms) fn set_up(world: &mut World, account: &str) -> Person {
        let mut person = Person::new(account);
        person.add(device(account, &format!("{account}-phone")));
        let put = person.put(true);
        let (status, shown) = call(world, &person.caller(0), "PUT", "me/keys", put);
        assert_eq!(status, 200, "{shown}");
        person
    }

    fn keys_events(world: &World, account: &str) -> Vec<Value> {
        let changes = sync::changes(&world.db, account, 0, 100).ok().expect("changes");
        changes["events"].as_array().expect("events").iter().filter(|event| event["kind"] == "keys").cloned().collect()
    }

    /// The device list only goes up, one version over the one the device
    /// saw (told the current one when it's stale), signed by the identity
    /// the account first uploaded; friends, and only friends, read it and
    /// hear when it changes.
    #[test]
    fn a_device_list_goes_up_by_compare_and_swap_under_its_first_identity() {
        let mut world = World::new();
        friends(&mut world, "ana", "bo");
        let mut ana = Person::new("ana");
        ana.add(device("ana", "ana-phone"));
        let phone = ana.caller(0);

        // The first upload names the identity, from nothing.
        let mut first = ana.put(true);
        first["previous_version"] = json!(1);
        let (status, stale) = call(&mut world, &phone, "PUT", "me/keys", first.clone());
        assert_eq!((status, &stale["error"], &stale["keys"]), (409, &json!("stale"), &Value::Null), "{stale}");
        first["identity_key"] = Value::Null;
        first["previous_version"] = json!(0);
        assert_eq!(call(&mut world, &phone, "PUT", "me/keys", first.clone()).0, 400, "the first upload names its identity");
        first["identity_key"] = json!(b64(&IdentityKey::generate().expect("key").public().0));
        assert_eq!(call(&mut world, &phone, "PUT", "me/keys", first.clone()).0, 400, "signed by another identity");
        first["identity_key"] = json!(b64(&ana.identity.public().0));
        let (status, shown) = call(&mut world, &phone, "PUT", "me/keys", first);
        assert_eq!(status, 200, "{shown}");
        assert_eq!(shown["version"], 1);

        // Another device that saw nothing is told what's there.
        let mut late = Person::new("ana");
        late.add(device("ana", "ana-mac"));
        let (status, stale) = call(&mut world, &late.caller(0), "PUT", "me/keys", late.put(true));
        assert_eq!(status, 409, "{stale}");
        assert_eq!(stale["keys"], shown);

        // The next one, over version 1; not over a stale version, not at
        // the same version, not another account's.
        ana.add(device("ana", "ana-mac"));
        let next = ana.put(false);
        let mut behind = next.clone();
        behind["previous_version"] = json!(0);
        assert_eq!(call(&mut world, &phone, "PUT", "me/keys", behind).0, 409);
        let (status, now) = call(&mut world, &phone, "PUT", "me/keys", next.clone());
        assert_eq!((status, &now["version"]), (200, &json!(2)), "{now}");
        assert_eq!(call(&mut world, &phone, "PUT", "me/keys", next).0, 409, "the same again is stale");
        ana.version = 1;
        let mut same = ana.put(false);
        same["previous_version"] = json!(2);
        assert_eq!(call(&mut world, &phone, "PUT", "me/keys", same).0, 400, "version 2 again");
        let mut bo = Person::new("bo");
        bo.add(device("bo", "bo-phone"));
        let mut theirs = bo.put(true);
        theirs["previous_version"] = json!(2);
        theirs["identity_key"] = Value::Null;
        assert_eq!(call(&mut world, &phone, "PUT", "me/keys", theirs).0, 400, "Bo's list isn't Ana's");

        // A new identity takes its own route.
        let mut changed = Person::new("ana");
        changed.add(device("ana", "ana-phone"));
        changed.version = 2;
        let changed = changed.put(true);
        let (status, refused) = call(&mut world, &phone, "PUT", "me/keys", changed);
        assert_eq!((status, &refused["error"]), (409, &json!("identity_changed")), "{refused}");

        // Who reads them: Ana's devices and her friends, not a stranger,
        // not an account page.
        assert_eq!(call(&mut world, &device("bo", "bo-phone"), "GET", "keys/ana", Value::Null), (200, now.clone()));
        assert_eq!(call(&mut world, &phone, "GET", "keys/ana", Value::Null), (200, now));
        assert_eq!(call(&mut world, &device("cy", "cy-phone"), "GET", "keys/ana", Value::Null).0, 404);
        assert_eq!(call(&mut world, &device("bo", "bo-phone"), "GET", "keys/cy", Value::Null).0, 404, "none yet");
        let page = Caller::Account { account_id: "ana".into(), registrations: Vec::new() };
        assert_eq!(call(&mut world, &page, "PUT", "me/keys", Value::Null).0, 403);

        // Each change reached Ana's devices and Bo's.
        assert_eq!(keys_events(&world, "ana"), vec![json!({"kind": "keys", "account_id": "ana"}); 2]);
        assert_eq!(keys_events(&world, "bo").len(), 2);
        assert_eq!(keys_events(&world, "cy").len(), 0);
    }

    pub(in crate::dms) fn upload(world: &mut World, caller: &Caller, one_time: Vec<Value>, last_resort: Option<Value>) -> (u16, Value) {
        call(world, caller, "POST", "key-packages", json!({"key_packages": one_time, "last_resort": last_resort}))
    }

    pub(in crate::dms) fn claim(world: &mut World, caller: &Caller, account: &str) -> Vec<(String, bool)> {
        let (status, claimed) = call(world, caller, "POST", "key-packages/claim", json!({"account_id": account}));
        assert_eq!(status, 200, "{claimed}");
        claimed["key_packages"]
            .as_array()
            .expect("key packages")
            .iter()
            .map(|claimed| {
                let package = Envelope::from_json(&claimed["key_package"]).expect("an envelope");
                let (package, _) = key_package(package.bytes()).ok().expect("a KeyPackage");
                let leaf = claimed["leaf_id"].as_str().expect("a leaf").to_owned();
                assert_eq!(certificate(&package).ok().expect("certified").leaf.leaf_id, leaf);
                (leaf, claimed["last_resort"].as_bool().expect("bool"))
            })
            .collect()
    }

    /// A leaf uploads only its own KeyPackages, certified by the account's
    /// identity and named on its list, at most 100 and one last-resort one;
    /// each one-time one is handed out once, then the last-resort one.
    #[test]
    fn key_packages_are_the_leaf_s_own_and_each_is_claimed_once() {
        let mut world = World::new();
        friends(&mut world, "ana", "bo");
        let mut ana = Person::new("ana");
        ana.add(device("ana", "ana-phone"));
        let phone = ana.caller(0);
        let two = ana.key_packages(0, 3);
        assert_eq!(upload(&mut world, &phone, two[..1].to_vec(), None).1["error"], "not_set_up");
        assert_eq!(call(&mut world, &phone, "PUT", "me/keys", ana.put(true)).0, 200);

        // Not another leaf's, not one off the list, not garbage.
        let mac = ana.add(device("ana", "ana-mac"));
        let unlisted = ana.key_packages(mac, 1);
        assert_eq!(upload(&mut world, &ana.caller(mac), unlisted.clone(), None).0, 400, "not on the list yet");
        assert_eq!(upload(&mut world, &phone, unlisted, None).0, 400, "the Mac's, from the phone");
        let mut stranger = Person::new("ana");
        stranger.add(device("ana", "ana-phone"));
        assert_eq!(upload(&mut world, &phone, stranger.key_packages(0, 1), None).0, 400, "another identity's");
        let not_one = json!({"format": 2, "bytes": b64(b"not a KeyPackage")});
        assert_eq!(upload(&mut world, &phone, vec![not_one], None).0, 400);
        assert_eq!(
            call(&mut world, &device("bo", "bo-phone"), "POST", "key-packages", json!({"key_packages": two})).0,
            409,
            "Bo has no keys"
        );

        // Two one-time ones and a last-resort one.
        let (status, supply) = upload(&mut world, &phone, two[..2].to_vec(), Some(two[2].clone()));
        assert_eq!((status, supply.clone()), (200, json!({"remaining": 2, "last_resort": true})), "{supply}");
        assert_eq!(call(&mut world, &phone, "GET", "key-packages", Value::Null), (200, supply));
        assert_eq!(upload(&mut world, &phone, two[..1].to_vec(), None).1["remaining"], 2, "the same one twice is one");

        // Bo claims both, then the last-resort one, again and again.
        let bo = device("bo", "bo-phone");
        assert_eq!(claim(&mut world, &bo, "ana"), vec![("ana-phone".to_owned(), false)]);
        assert_eq!(claim(&mut world, &bo, "ana"), vec![("ana-phone".to_owned(), false)]);
        assert_eq!(claim(&mut world, &bo, "ana"), vec![("ana-phone".to_owned(), true)]);
        assert_eq!(claim(&mut world, &bo, "ana"), vec![("ana-phone".to_owned(), true)]);
        let supply = call(&mut world, &phone, "GET", "key-packages", Value::Null).1;
        assert_eq!(supply, json!({"remaining": 0, "last_resort": true}));
        let claims: i64 = world.db.query_row("SELECT COUNT(*) FROM dm_claims", [], |row| row.get(0)).expect("claims");
        assert_eq!(claims, 3, "each claimed KeyPackage is remembered once, for its Welcome");

        // A new last-resort one replaces the old.
        let fresh = ana.key_packages(0, 1);
        assert_eq!(upload(&mut world, &phone, Vec::new(), Some(fresh[0].clone())).0, 200);
        let last: i64 =
            world.db.query_row("SELECT COUNT(*) FROM dm_key_packages WHERE last_resort = 1", [], |row| row.get(0)).expect("count");
        assert_eq!(last, 1);

        // No more than 100 one-time ones.
        let many = ana.key_packages(0, KEY_PACKAGES_MOST as usize + 1);
        assert_eq!(upload(&mut world, &phone, many[1..].to_vec(), None).1["remaining"], KEY_PACKAGES_MOST);
        assert_eq!(upload(&mut world, &phone, many[..1].to_vec(), None).0, 400);

        // A stranger and an account page claim nothing; nor does a Claktory.
        assert_eq!(call(&mut world, &device("cy", "cy-phone"), "POST", "key-packages/claim", json!({"account_id": "ana"})).0, 404);
        let page = Caller::Account { account_id: "bo".into(), registrations: Vec::new() };
        assert_eq!(call(&mut world, &page, "POST", "key-packages/claim", json!({"account_id": "ana"})).0, 403);
        assert_eq!(call(&mut world, &claktory("bo", "mac"), "POST", "key-packages/claim", json!({"account_id": "ana"})).0, 403);
    }

    /// A friend gets KeyPackages for the person's devices only, never for
    /// their Claktories; the person's own devices get both. A Claktory
    /// uploads for itself, not for an agent.
    #[test]
    fn a_friend_claims_only_device_leaves_and_claims_are_limited() {
        let mut world = World::new();
        friends(&mut world, "ana", "bo");
        let mut ana = Person::new("ana");
        ana.add(device("ana", "ana-phone"));
        let mac = ana.add(claktory("ana", "mac"));
        assert_eq!(call(&mut world, &ana.caller(0), "PUT", "me/keys", ana.put(true)).0, 200);
        let packages = ana.key_packages(0, 1);
        assert_eq!(upload(&mut world, &ana.caller(0), Vec::new(), Some(packages[0].clone())).0, 200);
        let packages = ana.key_packages(mac, 2);
        let agent = Caller::Backend {
            account_id: "ana".into(),
            claktory_id: "mac".into(),
            epoch: 1,
            agent: Some(Actor { id: "june".into(), incarnation: "1".into() }),
        };
        assert_eq!(upload(&mut world, &agent, packages[..1].to_vec(), None).0, 403);
        let (status, supply) = upload(&mut world, &ana.caller(mac), packages[..1].to_vec(), Some(packages[1].clone()));
        assert_eq!(status, 200, "{supply}");
        assert_eq!(call(&mut world, &ana.caller(mac), "GET", "key-packages", Value::Null).1["remaining"], 1);

        let bo = device("bo", "bo-phone");
        assert_eq!(claim(&mut world, &bo, "ana"), vec![("ana-phone".to_owned(), true)]);
        let (status, claimed) = call(&mut world, &bo, "POST", "key-packages/claim", json!({"account_id": "ana", "leaf_ids": ["mac#1"]}));
        assert_eq!((status, claimed), (200, json!({"key_packages": []})));
        let own = device("ana", "ana-ipad");
        assert_eq!(claim(&mut world, &own, "ana"), vec![("ana-phone".to_owned(), true), ("mac#1".to_owned(), false)]);

        // 60 claims an hour from one device.
        for _ in 2..CLAIMS_AN_HOUR {
            claim(&mut world, &bo, "ana");
        }
        let (status, refused) = call(&mut world, &bo, "POST", "key-packages/claim", json!({"account_id": "ana"}));
        assert_eq!((status, &refused["error"]), (429, &json!("rate_limited")), "{refused}");
        world.now += 3_600_000;
        claim(&mut world, &bo, "ana");
    }

    /// A new identity starts a new list at version 1, signed by itself;
    /// every KeyPackage the old one certified goes, and so does one for a
    /// leaf dropped from the list.
    #[test]
    fn a_new_identity_starts_over_and_what_the_old_one_certified_goes() {
        let mut world = World::new();
        let mut ana = set_up(&mut world, "ana");
        let mac = ana.add(device("ana", "ana-mac"));
        assert_eq!(call(&mut world, &ana.caller(0), "PUT", "me/keys", ana.put(false)).0, 200);
        for leaf in [0, mac] {
            let packages = ana.key_packages(leaf, 2);
            assert_eq!(upload(&mut world, &ana.caller(leaf), packages, None).0, 200);
        }
        let held =
            |world: &World| -> i64 { world.db.query_row("SELECT COUNT(*) FROM dm_key_packages", [], |row| row.get(0)).expect("count") };
        assert_eq!(held(&world), 4);

        // Dropping the Mac drops its KeyPackages.
        let (caller, me) = ana.leaves.remove(mac);
        assert_eq!(call(&mut world, &ana.caller(0), "PUT", "me/keys", ana.put(false)).0, 200);
        assert_eq!(held(&world), 2);
        ana.leaves.push((caller, me));

        // A rotation: version 1, signed by the new key, not the same key.
        let old = ana.identity.public();
        ana.identity = IdentityKey::generate().expect("a key");
        let leaves = std::mem::take(&mut ana.leaves);
        ana.leaves =
            leaves.into_iter().map(|(caller, me)| (caller.clone(), ana.certify(&caller, LeafKey::from_secret(&me.key.secret())))).collect();
        ana.version = 0;
        let list = ana.list();
        let rotate =
            |identity: &IdentityPublic, version: u64| json!({"identity_key": b64(&identity.0), "device_list": list, "version": version});
        let phone = ana.caller(0);
        assert_eq!(call(&mut world, &phone, "POST", "me/keys/rotate", rotate(&old, 1)).0, 400, "signed by another key");
        assert_eq!(call(&mut world, &phone, "POST", "me/keys/rotate", rotate(&ana.identity.public(), 2)).0, 400);
        let (status, shown) = call(&mut world, &phone, "POST", "me/keys/rotate", rotate(&ana.identity.public(), 1));
        assert_eq!((status, &shown["version"]), (200, &json!(1)), "{shown}");
        assert_eq!(shown["identity_key"], b64(&ana.identity.public().0));
        assert_eq!(held(&world), 0, "the old identity's KeyPackages are no use");
        assert_eq!(call(&mut world, &phone, "POST", "me/keys/rotate", rotate(&ana.identity.public(), 1)).0, 400, "the same key");

        // Three a day.
        for n in 1..ROTATIONS_A_DAY {
            ana.identity = IdentityKey::generate().expect("a key");
            ana.version = 0;
            let list = ana.list();
            let body = json!({"identity_key": b64(&ana.identity.public().0), "device_list": list, "version": 1});
            let (status, refused) = call(&mut world, &phone, "POST", "me/keys/rotate", body);
            assert_eq!(status, 200, "rotation {n}: {refused}");
        }
        ana.identity = IdentityKey::generate().expect("a key");
        ana.version = 0;
        let list = ana.list();
        let body = json!({"identity_key": b64(&ana.identity.public().0), "device_list": list, "version": 1});
        assert_eq!(call(&mut world, &phone, "POST", "me/keys/rotate", body).0, 429);
    }

    /// A Claktory reads its own account's keys, and a friend's only while
    /// one of its agents holds a live DM grant on their conversation.
    #[test]
    fn a_claktory_reads_a_friend_s_keys_only_through_a_grant() {
        let mut world = World::new();
        let (conversation, _) = friends(&mut world, "ana", "bo");
        set_up(&mut world, "ana");
        set_up(&mut world, "bo");
        let mac = claktory("ana", "mac");
        assert_eq!(call(&mut world, &mac, "GET", "keys/ana", Value::Null).0, 200);
        assert_eq!(call(&mut world, &mac, "GET", "keys/bo", Value::Null).0, 404);

        let june = Actor { id: "june".into(), incarnation: "1".into() };
        let put = Put {
            app: "dms".into(),
            agent: june.clone(),
            scopes: vec!["dm.read".into()],
            resources: vec![Resource { kind: "conversation".into(), id: conversation.clone() }],
            expires_ms: world.now + 60_000,
        };
        crate::grants::put(&mut world.db, &mac, put, world.now).expect("granted");
        assert_eq!(call(&mut world, &mac, "GET", "keys/bo", Value::Null).0, 200);
        let as_june = Caller::Backend { account_id: "ana".into(), claktory_id: "mac".into(), epoch: 1, agent: Some(june) };
        assert_eq!(call(&mut world, &as_june, "GET", "keys/bo", Value::Null).0, 404, "the Claktory, not its agent");
        let other = Caller::Backend { account_id: "ana".into(), claktory_id: "mac".into(), epoch: 2, agent: None };
        assert_eq!(call(&mut world, &other, "GET", "keys/bo", Value::Null).0, 404, "under another registration");
        world.now += 60_000;
        assert_eq!(call(&mut world, &mac, "GET", "keys/bo", Value::Null).0, 404, "expired");
    }

    #[test]
    fn deleting_an_account_deletes_its_keys() {
        let mut world = World::new();
        let ana = set_up(&mut world, "ana");
        let packages = ana.key_packages(0, 1);
        assert_eq!(upload(&mut world, &ana.caller(0), packages, None).0, 200);
        assert!(super::super::identity::purge(&mut world.db, &world.keys, "ana", world.now).is_ok());
        for table in ["dm_identities", "dm_key_packages"] {
            let left: i64 = world.db.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| row.get(0)).expect("count");
            assert_eq!(left, 0, "{table}");
        }
    }
}
