//! A stand-in for one of the person's devices in end-to-end direct messages
//! (dms-e2ee.md), for factoryd's tests: its identity key, its own leaf, its
//! MLS store, and its calls to the user-apps worker as the root forwards a
//! device's. It enrolls the Claktory's leaf over the Claktory's own
//! connection, as a real device does (H2).
//!
//! Shared by the DM test binaries: `#[path = "common/dm_device.rs"] mod
//! dm_device;` beside `common`. Ghiaccio's: identity, enrollment, keys,
//! the channel. Cioccolata's: proposals and approvals.

#![allow(dead_code)]

use std::os::unix::fs::MetadataExt as _;
use std::path::{Path, PathBuf};

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use claktory_dm_crypto::certificate::{Leaf, LeafCertificate, LeafKind, claktory_of};
use claktory_dm_crypto::device_list::{DeviceList, Listed};
use claktory_dm_crypto::content::{self, ByAgent, Content};
use claktory_dm_crypto::envelope::Envelope;
use claktory_dm_crypto::group::{Group, Incoming, LeafKey, Me, Outgoing};
use claktory_dm_crypto::identity::{IdentityKey, IdentityPublic};
use claktory_dm_crypto::provider::{Crypto, Provider};
use claktory_dm_crypto::storage::{SCHEMA, SealKey};
use claktory_dm_crypto::validation::{GroupKind, Pins};
use claktory_user_apps::ipc::{self, Caller, Request as AppRequest};
use factory_protocol::v1::{self, client_message::Body as Request, server_message::Body};
use rusqlite::Connection;
use serde_json::{Value, json};

use super::common::Client;

pub fn b64(bytes: &[u8]) -> String {
    URL_SAFE_NO_PAD.encode(bytes)
}

/// One of the person's devices.
pub struct Device {
    /// Where the worker's socket is.
    dir: PathBuf,
    pub account: String,
    /// Its account key as the worker knows it: its leaf id.
    pub device_key: String,
    pub identity: IdentityKey,
    pub me: Me,
    pub pins: Pins,
    /// The leaves on the person's list, this device's first.
    pub listed: Vec<Listed>,
    pub version: u64,
    /// The list as last signed, for friends to pin.
    pub signed_list: Vec<u8>,
    /// The groups it's in, by id (base64url), and what each is.
    pub groups: std::collections::HashMap<String, GroupKind>,
    /// How far it read each group.
    pub positions: std::collections::HashMap<String, i64>,
    db: Connection,
    key: SealKey,
    crypto: Crypto,
}

impl Device {
    /// The person's first device, with a new identity.
    pub fn new(dir: &Path, account: &str, seed: u8) -> Device {
        let identity = IdentityKey::generate().expect("an identity");
        let device_key = b64(&[seed; 32]);
        let key = LeafKey::generate().expect("a leaf key");
        let leaf = Leaf {
            kind: LeafKind::Device,
            account_id: account.to_owned(),
            leaf_id: device_key.clone(),
            signature_key: key.public(),
            name: "Phone".to_owned(),
            created_ms: 1,
        };
        let listed = vec![Listed { kind: LeafKind::Device, leaf_id: device_key.clone(), signature_key: key.public() }];
        let me = Me::new(key, LeafCertificate::issue(&identity, leaf).expect("a certificate")).expect("me");
        let mut pins = Pins::default();
        pins.first_contact(account, identity.public()).expect("its own pin");
        let db = Connection::open_in_memory().expect("a store");
        db.pragma_update(None, "secure_delete", "ON").expect("secure_delete");
        db.execute_batch(SCHEMA).expect("the schema");
        Device {
            dir: dir.to_path_buf(),
            account: account.to_owned(),
            device_key,
            identity,
            me,
            pins,
            listed,
            version: 0,
            signed_list: Vec::new(),
            groups: std::collections::HashMap::new(),
            positions: std::collections::HashMap::new(),
            db,
            key: SealKey::from_bytes([seed; 32]),
            crypto: Crypto::new().expect("crypto"),
        }
    }

    /// Runs `f` with this device's MLS provider.
    pub fn with<R>(&self, f: impl FnOnce(&Provider<'_>, &Me) -> R) -> R {
        let provider = Provider::new(&self.crypto, &self.db, &self.key).expect("a provider");
        f(&provider, &self.me)
    }

    /// A call to the worker as this device.
    pub async fn call(&self, method: &str, route: &str, body: Value) -> (u16, Value) {
        self.call_apps(method, &format!("dms/{route}"), body).await
    }

    /// The person ends a grant they gave one of their Claktory's agents, as
    /// from the app's Agents' access.
    pub async fn revoke_grant(&self, id: &str) {
        let (status, answer) = self.call_apps("DELETE", &format!("grants/{id}"), Value::Null).await;
        assert_eq!(status, 204, "{answer}");
    }

    /// A call to the worker's apps as this device (`dms/…`, `grants/…`).
    async fn call_apps(&self, method: &str, route: &str, body: Value) -> (u16, Value) {
        let uid = std::fs::metadata(&self.dir).expect("the folder").uid();
        let (path, query) = route.split_once('?').unwrap_or((route, ""));
        // One reading of the clock: signed in "now" must never be a second
        // after the request's own time.
        let now = factoryd::transcript::now_ms() / 1000;
        let request = AppRequest::Call {
            method: method.into(),
            path: format!("/v1/apps/{path}"),
            query: query.into(),
            time: now,
            caller: Caller::Device {
                account_id: self.account.clone(),
                device_key: self.device_key.clone(),
                registrations: self.registrations(),
                removed_devices: Vec::new(),
                signed_in_at: Some(now),
                bound_devices: None,
            },
        };
        let bytes = if body.is_null() { Vec::new() } else { body.to_string().into_bytes() };
        let (status, answer) = ipc::ask(&self.dir.join("apps.sock"), uid, &request, &bytes).await.expect("the worker answers");
        (status, if answer.is_empty() { Value::Null } else { serde_json::from_slice(&answer).expect("JSON") })
    }

    /// The account's Claktories, as the root passes them on with a
    /// device's call.
    fn registrations(&self) -> Vec<ipc::Registration> {
        let Ok(db) = Connection::open(self.dir.join("account.db")) else { return Vec::new() };
        let Ok(mut statement) = db.prepare("SELECT id, epoch FROM claktories WHERE account_id = ?1") else { return Vec::new() };
        statement
            .query_map([&self.account], |row| Ok(ipc::Registration { claktory_id: row.get(0)?, epoch: row.get::<_, i64>(1)? as u64 }))
            .map(|rows| rows.filter_map(Result::ok).collect())
            .unwrap_or_default()
    }

    /// The list of the person's leaves at the next version, sent to the
    /// worker; the device takes it into its own pins.
    pub async fn publish_keys(&mut self) {
        let previous = self.version;
        self.version += 1;
        let list = DeviceList { account_id: self.account.clone(), version: self.version, created_ms: 1, leaves: self.listed.clone() };
        let bytes = list.sign(&self.identity).expect("signed").to_bytes().expect("bytes");
        self.pins.accept_list(&self.account, &bytes).expect("its own list");
        self.signed_list = bytes.clone();
        let mut put = json!({"device_list": b64(&bytes), "version": self.version, "previous_version": previous});
        if previous == 0 {
            put["identity_key"] = json!(b64(&self.identity.public().0));
        }
        let (status, answer) = self.call("PUT", "me/keys", put).await;
        assert_eq!(status, 200, "{answer}");
    }

    /// Enrolls the Claktory `client` is connected to: asks for its leaf,
    /// certifies it, lists it and gives it the pins.
    pub async fn enroll(&mut self, client: &mut Client) -> v1::DmLeafOffer {
        let offer = match client.request(Request::DmLeafOffer(v1::DmLeafOfferRequest {})).await.body {
            Body::DmLeafOffer(offer) => offer,
            other => panic!("expected an offer, got {other:?}"),
        };
        let signature_key: [u8; 32] = offer.leaf_signature_key.clone().try_into().expect("32 bytes");
        let leaf = Leaf {
            kind: LeafKind::Claktory,
            account_id: self.account.clone(),
            leaf_id: offer.leaf_id.clone(),
            signature_key,
            name: offer.name.clone(),
            created_ms: 1,
        };
        let certificate = LeafCertificate::issue(&self.identity, leaf).expect("a certificate");
        self.listed.retain(|listed| listed.leaf_id != offer.leaf_id);
        self.listed.push(Listed { kind: LeafKind::Claktory, leaf_id: offer.leaf_id.clone(), signature_key });
        self.publish_keys().await;
        let request =
            v1::DmEnrollLeaf { certificate: certificate.to_bytes().expect("bytes"), pins: serde_json::to_vec(&self.pins).expect("pins") };
        match client.request(Request::DmEnrollLeaf(request)).await.body {
            Body::Ack(_) => {}
            other => panic!("the enrollment wasn't taken: {other:?}"),
        }
        offer
    }
    /// Puts up `n` one-time KeyPackages and a last-resort one.
    pub async fn stock(&self, n: usize) {
        let (packages, last_resort) = self.with(|provider, me| {
            let packages: Vec<Value> = (0..n).map(|_| envelope(me.key_package(provider).expect("a key package"))).collect();
            (packages, envelope(me.last_resort_key_package(provider).expect("a last-resort one")))
        });
        let (status, supply) = self.call("POST", "key-packages", json!({"key_packages": packages, "last_resort": last_resort})).await;
        assert_eq!(status, 200, "{supply}");
    }

    /// Pins `friend` as at first contact, with its current list.
    pub fn pin(&mut self, friend: &Device) {
        self.pins.first_contact(&friend.account, friend.identity.public()).expect("pinned");
        self.pins.accept_list(&friend.account, &friend.signed_list).expect("its list");
    }

    /// Takes in `friend`'s current list, as fetched with its keys.
    pub fn refresh(&mut self, friend: &Device) {
        self.pins.accept_list(&friend.account, &friend.signed_list).expect("its list");
    }

    /// The person makes a new security key on this device (dms-e2ee.md
    /// §2.1): its own leaf certified again under it, the old one retired in
    /// its pins, and the list at version 1 under it sent with
    /// `me/keys/rotate`. It moves in no group here, and tells no one.
    pub async fn rotate_identity(&mut self) {
        let identity = IdentityKey::generate().expect("a new identity");
        let certificate = LeafCertificate::issue(&identity, self.me.certificate.leaf.clone()).expect("a certificate");
        self.me = self.me.recertified(certificate).expect("the same leaf");
        self.identity = identity;
        self.pins.key_changed(&self.account, self.identity.public()).expect("its own new key");
        self.version = 1;
        let list = DeviceList { account_id: self.account.clone(), version: 1, created_ms: 1, leaves: self.listed.clone() };
        let bytes = list.sign(&self.identity).expect("signed").to_bytes().expect("bytes");
        self.pins.accept_list(&self.account, &bytes).expect("its own list");
        self.signed_list = bytes.clone();
        let body = json!({"identity_key": b64(&self.identity.public().0), "device_list": b64(&bytes), "version": 1});
        let (status, answer) = self.call("POST", "me/keys/rotate", body).await;
        assert_eq!(status, 200, "{answer}");
    }

    /// The person's device `seed` lost every key and starts over on it (row
    /// 630): the same account key (leaf id), a new leaf key and identity, the
    /// account's keys before (`retired`, oldest first: the one it lost and
    /// any it replaced, as the worker showed them) retired in its pins, and
    /// a version-1 list naming only itself sent with `me/keys/rotate`. It's
    /// in no group yet.
    pub async fn start_over(dir: &Path, account: &str, seed: u8, retired: &[IdentityPublic]) -> Device {
        let mut fresh = Device::new(dir, account, seed);
        let mut pins = Pins::default();
        let (first, rest) = retired.split_first().expect("a key before");
        pins.first_contact(account, *first).expect("the first key");
        for key in rest {
            pins.key_changed(account, *key).expect("a key since");
        }
        pins.key_changed(account, fresh.identity.public()).expect("its new key");
        pins.seen(account);
        fresh.pins = pins;
        fresh.version = 1;
        let list = DeviceList { account_id: account.to_owned(), version: 1, created_ms: 1, leaves: fresh.listed.clone() };
        let bytes = list.sign(&fresh.identity).expect("signed").to_bytes().expect("bytes");
        fresh.pins.accept_list(account, &bytes).expect("its own list");
        fresh.signed_list = bytes.clone();
        let body = json!({"identity_key": b64(&fresh.identity.public().0), "device_list": b64(&bytes), "version": 1});
        let (status, answer) = fresh.call("POST", "me/keys/rotate", body).await;
        assert_eq!(status, 200, "{answer}");
        fresh
    }

    /// Rejoins `group_id` by external commit from the worker's GroupInfo,
    /// then takes out its account's dead leaves in its next commit, as a
    /// device that started over does: the seqs of the two commits.
    pub async fn rejoin(&mut self, group_id: &str, kind: GroupKind) -> (i64, Option<i64>) {
        let (status, info) = self.call("GET", &format!("groups/{group_id}/group-info"), Value::Null).await;
        assert_eq!(status, 200, "{info}");
        let bytes = unenvelope(&info["group_info"]);
        let id = URL_SAFE_NO_PAD.decode(group_id).expect("an id");
        let out = self.with(|provider, me| Group::rejoin(provider, me, &self.pins, kind.clone(), &id, &bytes).expect("rejoined").1);
        self.groups.insert(group_id.to_owned(), kind);
        let body = json!({"commit": envelope(out.commit), "group_info": envelope(out.group_info.expect("a GroupInfo"))});
        let (status, taken) = self.call("POST", &format!("groups/{group_id}/commits"), body).await;
        assert_eq!(status, 200, "{taken}");
        let joined = taken["seq"].as_i64().expect("a seq");
        self.positions.insert(group_id.to_owned(), joined);
        self.with(|provider, _| self.load(provider, group_id).merge_pending(provider).expect("merged"));
        let dead = self.with(|provider, me| self.load(provider, group_id).remove_dead(provider, me, &self.pins).expect("made"));
        let Some(out) = dead else { return (joined, None) };
        let body = json!({"commit": envelope(out.commit), "group_info": envelope(out.group_info.expect("a GroupInfo"))});
        let (status, taken) = self.call("POST", &format!("groups/{group_id}/commits"), body).await;
        assert_eq!(status, 200, "{taken}");
        let removed = taken["seq"].as_i64().expect("a seq");
        self.positions.insert(group_id.to_owned(), removed);
        self.with(|provider, _| self.load(provider, group_id).merge_pending(provider).expect("merged"));
        (joined, Some(removed))
    }

    /// Takes in `friend`'s new security key, as a friend's device does once
    /// it fetched their keys: the old one retired for good.
    pub fn key_changed(&mut self, friend: &Device) {
        self.pins.key_changed(&friend.account, friend.identity.public()).expect("the new key");
        self.refresh(friend);
    }

    /// The group's members as this device checks them now: each one's
    /// certificate, or why it isn't a member it trusts.
    pub fn members(&self, group_id: &str) -> Vec<Result<LeafCertificate, String>> {
        self.with(|provider, _| {
            let members = self.load(provider, group_id).members(&self.pins);
            members.into_iter().map(|member| member.map(|member| member.certificate).map_err(|error| error.to_string())).collect()
        })
    }

    /// Starts a group of `kind` with only this device in it, registered
    /// with `body` (its kind and subject): its id.
    pub async fn create(&mut self, kind: GroupKind, mut body: Value) -> String {
        let mut id = [0u8; 16];
        getrandom::fill(&mut id).expect("random");
        let info = self.with(|provider, me| {
            let group = Group::create(provider, me, kind.clone(), &id).expect("made");
            group.group_info(provider, me).expect("a GroupInfo")
        });
        body["group_info"] = envelope(info);
        let (status, made) = self.call("POST", "groups", body).await;
        assert_eq!(status, 200, "{made}");
        let group_id = b64(&id);
        self.groups.insert(group_id.clone(), kind);
        group_id
    }

    /// Claims a KeyPackage of each of `leaf_ids` of `account`.
    pub async fn claim(&self, account: &str, leaf_ids: &[&str]) -> Vec<Vec<u8>> {
        let (status, claimed) = self.call("POST", "key-packages/claim", json!({"account_id": account, "leaf_ids": leaf_ids})).await;
        assert_eq!(status, 200, "{claimed}");
        let packages = claimed["key_packages"].as_array().expect("key packages");
        assert_eq!(packages.len(), leaf_ids.len(), "{claimed}");
        packages.iter().map(|package| unenvelope(&package["key_package"])).collect()
    }

    fn load(&self, provider: &Provider<'_>, group_id: &str) -> Group {
        let kind = self.groups.get(group_id).expect("a group of this device's").clone();
        let id = URL_SAFE_NO_PAD.decode(group_id).expect("an id");
        Group::load(provider, kind, &id).expect("loads").expect("kept")
    }

    /// Commits `change` to the group and merges it once the worker took it.
    pub async fn commit(&mut self, group_id: &str, change: impl FnOnce(&mut Group, &Provider<'_>, &Me, &Pins) -> Outgoing) -> Value {
        let out = self.with(|provider, me| change(&mut self.load(provider, group_id), provider, me, &self.pins));
        let mut body = json!({"commit": envelope(out.commit), "group_info": envelope(out.group_info.expect("a GroupInfo"))});
        if let Some(welcome) = out.welcome {
            body["welcome"] = envelope(welcome);
        }
        let (status, taken) = self.call("POST", &format!("groups/{group_id}/commits"), body).await;
        assert_eq!(status, 200, "{taken}");
        self.positions.insert(group_id.to_owned(), taken["seq"].as_i64().expect("a seq"));
        self.with(|provider, _| self.load(provider, group_id).merge_pending(provider).expect("merged"));
        taken
    }

    /// Adds these KeyPackages' leaves to the group.
    pub async fn add(&mut self, group_id: &str, key_packages: Vec<Vec<u8>>) -> Value {
        self.commit(group_id, |group, provider, me, pins| group.add(provider, me, pins, &key_packages).expect("added")).await
    }

    /// Makes the person's channel with the Claktory `offer` is for, and adds
    /// its leaf: the channel's id.
    pub async fn channel(&mut self, offer: &v1::DmLeafOffer) -> String {
        let kind = GroupKind::Channel { account: self.account.clone(), claktory_id: offer.claktory_id.clone() };
        let channel = self.create(kind, json!({"kind": "channel", "claktory_id": offer.claktory_id})).await;
        // The Claktory puts its KeyPackages up once it's on the list.
        let claim = json!({"account_id": self.account, "leaf_ids": [offer.leaf_id]});
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
        let package = loop {
            let (status, claimed) = self.call("POST", "key-packages/claim", claim.clone()).await;
            assert_eq!(status, 200, "{claimed}");
            if let Some(package) = claimed["key_packages"].get(0) {
                break unenvelope(&package["key_package"]);
            }
            assert!(std::time::Instant::now() < deadline, "the Claktory never put up KeyPackages");
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        };
        self.add(&channel, vec![package]).await;
        channel
    }

    /// Says `content` in a self group or channel: its seq.
    pub async fn say(&self, group_id: &str, content: &Content) -> i64 {
        let bytes = self.with(|provider, me| self.load(provider, group_id).send(provider, me, &self.pins, &content.encode().expect("encoded")).expect("sent"));
        let body = json!({"client_message_id": content.client_message_id, "envelope": envelope(bytes)});
        let (status, said) = self.call("POST", &format!("groups/{group_id}/messages"), body).await;
        assert_eq!(status, 200, "{said}");
        said["seq"].as_i64().expect("a seq")
    }

    /// Sends `text` in a conversation's group, as the person's device does:
    /// its seq.
    pub async fn post(&self, group_id: &str, conversation: &str, generation: i64, n: u32, text: &str) -> i64 {
        let id = format!("message-{}-{n:014}", &self.device_key[..6]);
        let content = Content::text(&id, text);
        let bytes = self.with(|provider, me| self.load(provider, group_id).send(provider, me, &self.pins, &content.encode().expect("encoded")).expect("sent"));
        let body = json!({"client_message_id": id, "contact_generation": generation, "envelope": envelope(bytes)});
        let (status, sent) = self.call("POST", &format!("conversations/{conversation}/messages"), body).await;
        assert_eq!(status, 200, "{sent}");
        sent["seq"].as_i64().expect("a seq")
    }

    /// Tells the channel's Claktory what this device pinned.
    pub async fn share_pins(&self, channel: &str, n: u32) -> i64 {
        let mut pins = Content::text(&format!("pins-{n:020}"), "");
        pins.kind = content::kind::PINS.to_owned();
        pins.text = None;
        pins.extra.insert("pins".to_owned(), serde_json::to_value(&self.pins).expect("pins"));
        self.say(channel, &pins).await
    }

    /// Joins the groups waiting for this device: their ids.
    pub async fn join_welcomes(&mut self, kind_of: impl Fn(&Value) -> GroupKind) -> Vec<String> {
        let (status, listed) = self.call("GET", "welcomes", Value::Null).await;
        assert_eq!(status, 200, "{listed}");
        let mut joined = Vec::new();
        for welcome in listed["welcomes"].as_array().expect("welcomes") {
            let group_id = welcome["group_id"].as_str().expect("a group").to_owned();
            let kind = kind_of(welcome);
            let id = URL_SAFE_NO_PAD.decode(&group_id).expect("an id");
            let bytes = unenvelope(&welcome["welcome"]);
            self.with(|provider, _| Group::join(provider, &self.pins, kind.clone(), &id, &bytes).expect("joined"));
            self.groups.insert(group_id.clone(), kind);
            self.positions.insert(group_id.clone(), welcome["seq"].as_i64().expect("a seq"));
            let (status, gone) = self.call("DELETE", &format!("welcomes/{}", welcome["id"].as_str().expect("an id")), Value::Null).await;
            assert_eq!(status, 204, "{gone}");
            joined.push(group_id);
        }
        joined
    }

    /// Reads the group on from where this device last read it (or from
    /// where it joined): what each message was.
    pub async fn catch_up(&mut self, group_id: &str) -> Vec<(i64, Option<Content>)> {
        let after = self.positions.get(group_id).copied().unwrap_or(0);
        let read = self.read(group_id, after).await;
        if let Some((seq, _)) = read.last() {
            self.positions.insert(group_id.to_owned(), *seq);
        }
        read
    }

    /// Reads the group's messages after `after`, in order: what each was.
    pub async fn read(&self, group_id: &str, after: i64) -> Vec<(i64, Option<Content>)> {
        let (status, page) = self.call("GET", &format!("groups/{group_id}/messages?after={after}"), Value::Null).await;
        assert_eq!(status, 200, "{page}");
        let mut read = Vec::new();
        for message in page["messages"].as_array().expect("messages") {
            let bytes = unenvelope(&message["envelope"]);
            let seq = message["seq"].as_i64().expect("a seq");
            let incoming = self.with(|provider, _| self.load(provider, group_id).process(provider, &self.pins, &bytes));
            // None for what isn't said (a commit), and for its own messages,
            // which MLS doesn't decrypt for their sender.
            match incoming {
                Ok(Incoming::Application { content, .. }) => read.push((seq, Some(Content::decode(&content).expect("content")))),
                _ => read.push((seq, None)),
            }
        }
        read
    }
}

pub fn envelope(bytes: Vec<u8>) -> Value {
    Envelope::new(bytes).expect("an envelope").to_json()
}

pub fn unenvelope(value: &Value) -> Vec<u8> {
    Envelope::from_json(value).expect("an envelope").into_bytes()
}

// Cioccolata's: an agent's proposals and the person's approval (dms-e2ee.md §6).

/// A proposal as the person's device opened it: the Claktory that sent it
/// (its certified leaf, never anything it said) and what it says.
pub struct Opened {
    pub from: LeafCertificate,
    pub content: Content,
    /// The proposal as the worker showed it.
    pub shown: Value,
}

impl Opened {
    pub fn extra(&self, field: &str) -> &Value {
        self.content.extra.get(field).unwrap_or(&Value::Null)
    }
}

impl Device {
    /// Another device of the same person (its own leaf, the same identity),
    /// listed and published by this one, with KeyPackages up.
    pub async fn another(&mut self, seed: u8) -> Device {
        let identity = IdentityKey::from_secret(&self.identity.secret());
        let device_key = b64(&[seed; 32]);
        let key = LeafKey::generate().expect("a leaf key");
        let leaf = Leaf {
            kind: LeafKind::Device,
            account_id: self.account.clone(),
            leaf_id: device_key.clone(),
            signature_key: key.public(),
            name: "Tablet".to_owned(),
            created_ms: 1,
        };
        self.listed.push(Listed { kind: LeafKind::Device, leaf_id: device_key.clone(), signature_key: key.public() });
        let me = Me::new(key, LeafCertificate::issue(&self.identity, leaf).expect("a certificate")).expect("me");
        self.publish_keys().await;
        let db = Connection::open_in_memory().expect("a store");
        db.pragma_update(None, "secure_delete", "ON").expect("secure_delete");
        db.execute_batch(SCHEMA).expect("the schema");
        let other = Device {
            dir: self.dir.clone(),
            account: self.account.clone(),
            device_key,
            identity,
            me,
            pins: self.pins.clone(),
            listed: self.listed.clone(),
            version: self.version,
            signed_list: self.signed_list.clone(),
            groups: Default::default(),
            positions: Default::default(),
            db,
            key: SealKey::from_bytes([seed; 32]),
            crypto: Crypto::new().expect("crypto"),
        };
        other.stock(2).await;
        other
    }

    /// The agents' proposals waiting for the person, as the worker shows them.
    pub async fn proposals(&self) -> Vec<Value> {
        let (status, listed) = self.call("GET", "proposals", Value::Null).await;
        assert_eq!(status, 200, "{listed}");
        listed["proposals"].as_array().expect("proposals").clone()
    }

    /// Opens a proposal shown by the worker, in `channel`: only once, since
    /// a message's key goes once it's used.
    pub fn open(&self, channel: &str, shown: &Value) -> Opened {
        let bytes = unenvelope(&shown["envelope"]);
        let incoming = self.with(|provider, _| self.load(provider, channel).process(provider, &self.pins, &bytes)).expect("opened");
        let Incoming::Application { from, content } = incoming else { panic!("not a message: {incoming:?}") };
        Opened { from: from.certificate.clone(), content: Content::decode(&content).expect("content"), shown: shown.clone() }
    }

    /// Approves what it opened: its exact text, by its agent, encrypted into
    /// the conversation's group and sent with the approval of that revision
    /// and the digest of what it fetched. The worker's answer.
    pub async fn approve(&self, conversation_group: &str, opened: &Opened) -> (u16, Value) {
        let text = |field: &str| opened.extra(field).as_str().expect(field).to_owned();
        let claktory_id = claktory_of(&opened.from.leaf.leaf_id).expect("a Claktory's leaf").to_owned();
        let mut message = Content::text(&opened.content.client_message_id, opened.content.text.as_deref().expect("text"));
        message.by_agent =
            Some(ByAgent { claktory_id, agent_id: text("agent_id"), incarnation: text("incarnation"), name: text("agent_name") });
        let bytes = self.with(|provider, me| {
            self.load(provider, conversation_group).send(provider, me, &self.pins, &message.encode().expect("encoded")).expect("sealed")
        });
        let digest: String =
            Envelope::from_json(&opened.shown["envelope"]).expect("an envelope").digest().iter().map(|byte| format!("{byte:02x}")).collect();
        let id = opened.shown["id"].as_str().expect("an id");
        let body = json!({"revision": opened.shown["revision"], "proposal_digest": digest, "envelope": envelope(bytes)});
        self.call("POST", &format!("proposals/{id}/approve"), body).await
    }
}
