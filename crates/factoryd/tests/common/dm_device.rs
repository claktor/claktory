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
use claktory_dm_crypto::content::{self, ByAgent, Content};
use claktory_dm_crypto::device_list::{DeviceList, Listed};
use claktory_dm_crypto::envelope::Envelope;
use claktory_dm_crypto::group::{Group, Incoming, LeafKey, Me, Outgoing};
use claktory_dm_crypto::identity::{IdentityKey, IdentityPublic};
use claktory_dm_crypto::provider::{Crypto, Provider};
use claktory_dm_crypto::storage::{FLOOR_SCHEMA, SCHEMA, SealKey, epoch_floor};
use claktory_dm_crypto::validation::{GroupKind, Pins};
use claktory_user_apps::ipc::{self, Caller, Request as AppRequest};
use factory_protocol::v1::{self, client_message::Body as Request, server_message::Body};
use rusqlite::Connection;
use serde_json::{Value, json};

use super::common::Client;

pub fn b64(bytes: &[u8]) -> String {
    URL_SAFE_NO_PAD.encode(bytes)
}

/// The test account service's signing key: every DM test's service signs
/// with it, so a device can show the assertion its app shows (E11-8 point
/// 6: its DM requests go only under the account key it was let in with).
pub const SERVICE_KEY: [u8; 32] = [0x5e; 32];

/// What the service gives an app signed in to `account` with
/// `account_key`, for tunnel key `device_key`.
pub fn assertion(account: &str, account_key: &str, device_key: &str) -> String {
    let now = jiff::Timestamp::now().as_second();
    let said = claktory_account::protocol::DeviceAssertion {
        v: 1,
        account_id: account.to_owned(),
        account_key: account_key.to_owned(),
        device_key: device_key.to_owned(),
        iat: now,
        exp: now + 3600,
    };
    claktory_account::protocol::sign_device_assertion(&ed25519_dalek::SigningKey::from_bytes(&SERVICE_KEY), &said)
}

/// An app connected to the Claktory at `addr` as the person, signed in to
/// `account` with `account_key`: its DM requests go under that leaf.
pub async fn link_as(addr: std::net::SocketAddr, account: &str, account_key: &str) -> Client {
    let mut client = Client::open(addr).await;
    let hello = v1::Hello {
        protocol_version: factory_protocol::PROTOCOL_VERSION,
        client_version: "test".into(),
        token: super::common::local_token(addr),
        assertion: assertion(account, account_key, ""),
        ..Default::default()
    };
    client.send(v1::ClientMessage { id: 1, body: Some(Request::Hello(hello)) }).await;
    client.welcomed().await;
    client
}

/// Secrets for a test account service signing with [SERVICE_KEY].
pub fn service_secrets() -> claktory_account::server::Secrets {
    claktory_account::server::Secrets {
        signing_key: ed25519_dalek::SigningKey::from_bytes(&SERVICE_KEY),
        ..claktory_account::server::Secrets::generate()
    }
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
    /// The person's home Claktory the list names (#451): its endpoint id.
    pub home: Option<[u8; 32]>,
    /// The groups it's in, by id (base64url), and what each is.
    pub groups: std::collections::HashMap<String, GroupKind>,
    /// How far it read each group: at its home (the home's seqs), or for
    /// a friend's device the worker's commits (the worker's seqs).
    pub positions: std::collections::HashMap<String, i64>,
    /// The person's home's address, for a device of the person whose
    /// Claktory is in the test (#451): it sends and reads there. Shared by
    /// the person's devices, so a restart's new address reaches them all.
    home_at: std::sync::Arc<std::sync::Mutex<Option<std::net::SocketAddr>>>,
    /// A friend's home (#451), for a friend's device with no Claktory of
    /// its own in the test: one that only keeps what it's given.
    pub friend_home: Option<std::sync::Arc<FriendHome>>,
    /// How far a device of the person's read the worker's mailbox, for the
    /// MLS proposals (a leaf's own Remove) that stay there (#451).
    pub worker_positions: std::collections::HashMap<String, i64>,
    /// How far a friend's device read what its home kept, by group.
    pub friend_positions: std::collections::HashMap<String, usize>,
    /// The routes this device gave its home: friends' homes.
    pub routes: Vec<v1::DmRoute>,
    /// Application messages it read, by their digest (lowercase hex): who
    /// sent each, and what it said.
    pub opened: std::collections::HashMap<String, (LeafCertificate, Content)>,
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
        db.execute_batch(FLOOR_SCHEMA).expect("the schema");
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
            home: None,
            groups: std::collections::HashMap::new(),
            positions: std::collections::HashMap::new(),
            home_at: Default::default(),
            friend_home: None,
            friend_positions: std::collections::HashMap::new(),
            worker_positions: std::collections::HashMap::new(),
            routes: Vec::new(),
            opened: std::collections::HashMap::new(),
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
        let bytes = list.sign_with(&self.identity, self.home).expect("signed").to_bytes().expect("bytes");
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
        let bytes = list.sign_with(&self.identity, self.home).expect("signed").to_bytes().expect("bytes");
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
    /// It names the same home as `old`, the device it was.
    pub async fn start_over(old: &Device, seed: u8, retired: &[IdentityPublic]) -> Device {
        let account = old.account.as_str();
        let mut fresh = Device::new(&old.dir, account, seed);
        fresh.home = old.home;
        fresh.home_at = old.home_at.clone();
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
        let bytes = list.sign_with(&fresh.identity, fresh.home).expect("signed").to_bytes().expect("bytes");
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
        let floor = epoch_floor(&self.db, &id).expect("its floor");
        let out = self.with(|provider, me| Group::rejoin(provider, me, &self.pins, kind.clone(), &id, &bytes, floor).expect("rejoined").1);
        self.groups.insert(group_id.to_owned(), kind);
        let commit = out.commit.clone();
        let body = json!({"commit": envelope(out.commit), "group_info": envelope(out.group_info.expect("a GroupInfo"))});
        let (status, taken) = self.call("POST", &format!("groups/{group_id}/commits"), body).await;
        assert_eq!(status, 200, "{taken}");
        let joined = taken["seq"].as_i64().expect("a seq");
        self.took(group_id, &taken, commit).await;
        self.with(|provider, _| self.load(provider, group_id).merge_pending(provider).expect("merged"));
        let dead = self.with(|provider, me| self.load(provider, group_id).remove_dead(provider, me, &self.pins).expect("made"));
        let Some(out) = dead else { return (joined, None) };
        let commit = out.commit.clone();
        let body = json!({"commit": envelope(out.commit), "group_info": envelope(out.group_info.expect("a GroupInfo"))});
        let (status, taken) = self.call("POST", &format!("groups/{group_id}/commits"), body).await;
        assert_eq!(status, 200, "{taken}");
        let removed = taken["seq"].as_i64().expect("a seq");
        self.took(group_id, &taken, commit).await;
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
        let commit_bytes = out.commit.clone();
        let mut body = json!({"commit": envelope(out.commit), "group_info": envelope(out.group_info.expect("a GroupInfo"))});
        if let Some(welcome) = out.welcome {
            body["welcome"] = envelope(welcome);
        }
        let (status, taken) = self.call("POST", &format!("groups/{group_id}/commits"), body).await;
        assert_eq!(status, 200, "{taken}");
        self.with(|provider, _| self.load(provider, group_id).merge_pending(provider).expect("merged"));
        self.took(group_id, &taken, commit_bytes).await;
        taken
    }

    /// Commits `change` to the group at the worker only, as a device away
    /// from its home does (Zucchero's row 568), and merges it.
    pub async fn commit_away(&mut self, group_id: &str, change: impl FnOnce(&mut Group, &Provider<'_>, &Me, &Pins) -> Outgoing) {
        let out = self.with(|provider, me| change(&mut self.load(provider, group_id), provider, me, &self.pins));
        let mut body = json!({"commit": envelope(out.commit), "group_info": envelope(out.group_info.expect("a GroupInfo"))});
        if let Some(welcome) = out.welcome {
            body["welcome"] = envelope(welcome);
        }
        let (status, taken) = self.call("POST", &format!("groups/{group_id}/commits"), body).await;
        assert_eq!(status, 200, "{taken}");
        self.with(|provider, _| self.load(provider, group_id).merge_pending(provider).expect("merged"));
    }

    /// The worker took this device's commit (`taken`): then its home, as
    /// the app does (#451): the person's takes it, a friend's tells the
    /// person's home there's one to fetch. Read past it at the worker.
    async fn took(&mut self, group_id: &str, taken: &Value, commit: Vec<u8>) {
        let seq = taken["seq"].as_i64().expect("a seq");
        let epoch = taken["epoch"].as_u64().unwrap_or_default();
        if self.home_addr().is_some() {
            self.worker_positions.insert(group_id.to_owned(), seq);
            self.to_home(group_id, commit, "", false).await;
            return;
        }
        if let Some(home) = &self.friend_home {
            home.hint(group_id, epoch.saturating_sub(1)).await;
        }
        self.positions.insert(group_id.to_owned(), seq);
    }

    /// Adds these KeyPackages' leaves to the group.
    pub async fn add(&mut self, group_id: &str, key_packages: Vec<Vec<u8>>) -> Value {
        self.commit(group_id, |group, provider, me, pins| group.add(provider, me, pins, &key_packages).expect("added")).await
    }

    /// Why adding these KeyPackages' leaves to the group is refused, without
    /// sending anything; panics if it was made.
    pub fn add_refused(&self, group_id: &str, key_packages: &[Vec<u8>]) -> String {
        let made = self.with(|provider, me| self.load(provider, group_id).add(provider, me, &self.pins, key_packages).map(|_| ()));
        made.expect_err("the add was made").to_string()
    }

    /// The group's epoch on this device.
    pub fn epoch(&self, group_id: &str) -> u64 {
        self.with(|provider, _| self.load(provider, group_id).epoch())
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

    /// Says `content` in a self group or channel, through its home: its
    /// seq there.
    pub async fn say(&self, group_id: &str, content: &Content) -> i64 {
        let bytes = self.with(|provider, me| {
            self.load(provider, group_id).send(provider, me, &self.pins, &content.encode().expect("encoded")).expect("sent")
        });
        self.send(group_id, bytes, &content.client_message_id).await
    }

    /// `text` sealed for the group at this device's epoch, under the id
    /// `post` gives message `n`: its bytes and id, not sent.
    pub fn seal(&self, group_id: &str, n: u32, text: &str) -> (Vec<u8>, String) {
        let id = format!("message-{}-{n:014}", &self.device_key[..6]);
        let content = Content::text(&id, text);
        let bytes = self.with(|provider, me| {
            self.load(provider, group_id).send(provider, me, &self.pins, &content.encode().expect("encoded")).expect("sent")
        });
        (bytes, id)
    }

    /// What the service gives this device's app, signed in to its account
    /// with its account key (its leaf id), for tunnel key `device_key`.
    pub fn assertion(&self, device_key: &str) -> String {
        assertion(&self.account, &self.device_key, device_key)
    }

    /// This device's app connected to the Claktory at `addr`, as the
    /// person, with its assertion: its DM requests go under its own leaf.
    pub async fn link(&self, addr: std::net::SocketAddr) -> Client {
        link_as(addr, &self.account, &self.device_key).await
    }

    /// One DmSend to the person's home, whatever it answers: its receipt.
    pub async fn send_once(&self, group_id: &str, bytes: Vec<u8>, client_message_id: &str) -> v1::DmReceipt {
        let addr = self.home_addr().expect("a home to send through");
        let mut client = self.link(addr).await;
        let request = v1::DmSend {
            group_id: URL_SAFE_NO_PAD.decode(group_id).expect("an id"),
            envelope: bytes,
            client_message_id: client_message_id.to_owned(),
            leaf_id: self.device_key.clone(),
        };
        match client.request(Request::DmSend(request)).await.body {
            Body::DmFeed(v1::DmFeed { sent: Some(receipt), .. }) => receipt,
            other => panic!("expected a receipt, got {other:?}"),
        }
    }

    /// Sends `text` in a conversation's group, as the person's device does,
    /// through its home: its seq there (for a friend's device, how many its
    /// home has sent on).
    pub async fn post(&self, group_id: &str, _conversation: &str, _generation: i64, n: u32, text: &str) -> i64 {
        let id = format!("message-{}-{n:014}", &self.device_key[..6]);
        let content = Content::text(&id, text);
        let bytes = self.with(|provider, me| {
            self.load(provider, group_id).send(provider, me, &self.pins, &content.encode().expect("encoded")).expect("sent")
        });
        self.send(group_id, bytes, &id).await
    }

    /// An MLS message through this device's home: the person's (DmSend,
    /// waiting while it isn't the home yet or holds the message), or a
    /// friend's, which delivers it to the person's home.
    pub async fn send(&self, group_id: &str, bytes: Vec<u8>, client_message_id: &str) -> i64 {
        if let Some(home) = &self.friend_home {
            return home.deliver(group_id, bytes).await;
        }
        self.to_home(group_id, bytes, client_message_id, true).await
    }

    /// DmSend to the person's home: its seq there. Waits while the home
    /// isn't it yet, holds it, or limits it, or holds a group whose device
    /// is off the list the home has (one just added, before the home reads
    /// the newest list; a device with something to take out takes it out
    /// first), or doesn't list this device yet (just added); `wait: false`
    /// gives up then.
    async fn to_home(&self, group_id: &str, bytes: Vec<u8>, client_message_id: &str, wait: bool) -> i64 {
        let addr = self.home_addr().expect("a home to send through");
        let group = URL_SAFE_NO_PAD.decode(group_id).expect("an id");
        let deadline = std::time::Instant::now() + PATIENCE;
        let mut client = self.link(addr).await;
        loop {
            let request = v1::DmSend {
                group_id: group.clone(),
                envelope: bytes.clone(),
                client_message_id: client_message_id.to_owned(),
                leaf_id: self.device_key.clone(),
            };
            let receipt = match client.request(Request::DmSend(request)).await.body {
                Body::DmFeed(v1::DmFeed { sent: Some(receipt), .. }) => receipt,
                other => panic!("expected a receipt, got {other:?}"),
            };
            let result = v1::DmSendResult::try_from(receipt.result).expect("a result");
            match result {
                v1::DmSendResult::Stored | v1::DmSendResult::Duplicate => return receipt.seq as i64,
                v1::DmSendResult::NotHome
                | v1::DmSendResult::Held
                | v1::DmSendResult::WaitingForFriend
                | v1::DmSendResult::RateLimited
                | v1::DmSendResult::RemovedDeviceInGroup
                | v1::DmSendResult::NotListed
                    if wait =>
                {
                    assert!(std::time::Instant::now() < deadline, "the home never took it: {result:?}");
                    let pause = receipt.retry_after_ms.clamp(100, 2000) as u64;
                    tokio::time::sleep(std::time::Duration::from_millis(pause)).await;
                }
                _ if !wait => return 0,
                other => panic!("the home refused it: {other:?} {receipt:?}"),
            }
        }
    }

    /// The group's epoch at the person's home, as its DmFollow answer says
    /// (None while it doesn't hold the group).
    pub async fn home_epoch(&self, group_id: &str) -> Option<u64> {
        let addr = self.home_addr()?;
        let group = URL_SAFE_NO_PAD.decode(group_id).expect("an id");
        let mut client = self.link(addr).await;
        let after = self.positions.get(group_id).copied().unwrap_or(0).max(0) as u64;
        let request =
            v1::DmFollow { leaf_id: self.device_key.clone(), cursors: vec![v1::DmCursor { group_id: group.clone(), seq: after }] };
        match client.request(Request::DmFollow(request)).await.body {
            Body::DmFeed(feed) => feed.groups.iter().find(|state| state.group_id == group).map(|state| state.epoch),
            _ => None,
        }
    }

    /// Tells the person's home this device kept the group through where it
    /// read it (DmAck), as the app does.
    pub async fn ack(&self, group_id: &str) {
        let addr = self.home_addr().expect("a home");
        let group = URL_SAFE_NO_PAD.decode(group_id).expect("an id");
        let through = self.positions.get(group_id).copied().unwrap_or(0).max(0) as u64;
        let mut client = self.link(addr).await;
        let request = v1::DmAck { leaf_id: self.device_key.clone(), through: vec![v1::DmCursor { group_id: group, seq: through }] };
        client.ok(Request::DmAck(request)).await;
    }

    /// Where the person's home answers now, for this device and the
    /// person's others made from it.
    pub fn set_home_addr(&self, addr: std::net::SocketAddr) {
        *self.home_at.lock().expect("the home's address") = Some(addr);
    }

    pub fn home_addr(&self) -> Option<std::net::SocketAddr> {
        *self.home_at.lock().expect("the home's address")
    }

    /// Gives the person's home the route to `friend`'s home for these
    /// groups (DmRoutes, as the app does): the friend's own Claktory, as
    /// their list names it, or a test's FriendHome, given the way to this
    /// person's home.
    pub async fn route(&mut self, friend: &Device, groups: &[&str]) {
        let endpoint = match &friend.friend_home {
            Some(home) => {
                home.toward(self.home.expect("this person's home on their list"));
                home.id()
            }
            None => friend.home.expect("the friend's home on their list"),
        };
        let group_ids: Vec<Vec<u8>> = groups.iter().map(|group| URL_SAFE_NO_PAD.decode(group).expect("an id")).collect();
        self.routes.retain(|route| route.account_id != friend.account);
        self.routes.push(v1::DmRoute { account_id: friend.account.clone(), home_endpoint: endpoint.to_vec(), group_ids });
        let addr = self.home_addr().expect("a home");
        let mut client = self.link(addr).await;
        match client.request(Request::DmRoutes(v1::DmRoutes { routes: self.routes.clone() })).await.body {
            Body::DmHome(_) => {}
            other => panic!("the routes weren't taken: {other:?}"),
        }
    }

    /// Tells the channel's Claktory what this device pinned.
    pub async fn share_pins(&self, channel: &str, n: u32) -> i64 {
        self.say(channel, &self.pins_content(n)).await
    }

    /// What `share_pins` says, sealed and not sent: its bytes and id.
    pub fn seal_pins(&self, channel: &str, n: u32) -> (Vec<u8>, String) {
        let pins = self.pins_content(n);
        let bytes = self.with(|provider, me| {
            self.load(provider, channel).send(provider, me, &self.pins, &pins.encode().expect("encoded")).expect("sent")
        });
        (bytes, pins.client_message_id)
    }

    fn pins_content(&self, n: u32) -> Content {
        let mut pins = Content::text(&format!("pins-{n:020}"), "");
        pins.kind = content::kind::PINS.to_owned();
        pins.text = None;
        pins.extra.insert("pins".to_owned(), serde_json::to_value(&self.pins).expect("pins"));
        pins
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
            let seq = welcome["seq"].as_i64().expect("a seq");
            if self.home_addr().is_some() {
                self.worker_positions.insert(group_id.clone(), seq);
            } else {
                self.positions.insert(group_id.clone(), seq);
            }
            let (status, gone) = self.call("DELETE", &format!("welcomes/{}", welcome["id"].as_str().expect("an id")), Value::Null).await;
            assert_eq!(status, 204, "{gone}");
            joined.push(group_id);
        }
        joined
    }

    /// Reads the group on from where this device last read it (or from
    /// where it joined): what each message was. From its home (#451); a
    /// friend's device reads the worker's commits, then what its home kept.
    pub async fn catch_up(&mut self, group_id: &str) -> Vec<(i64, Option<Content>)> {
        let after = self.positions.get(group_id).copied().unwrap_or(0);
        if let Some(home) = self.friend_home.clone() {
            let mut read = self.read(group_id, after).await;
            if let Some((seq, _)) = read.last() {
                self.positions.insert(group_id.to_owned(), *seq);
            }
            let from = self.friend_positions.get(group_id).copied().unwrap_or(0);
            let kept = home.kept(group_id);
            for (n, bytes) in kept.iter().enumerate().skip(from) {
                read.push(((n + 1) as i64, self.process(group_id, bytes)));
            }
            self.friend_positions.insert(group_id.to_owned(), kept.len());
            return read;
        }
        let read = self.read(group_id, after).await;
        if let Some((seq, _)) = read.last() {
            self.positions.insert(group_id.to_owned(), *seq);
        }
        read
    }

    /// Reads the group's messages after `after`, in order: what each was.
    /// At its home, or (a friend's device) the worker's commits.
    pub async fn read(&mut self, group_id: &str, after: i64) -> Vec<(i64, Option<Content>)> {
        let mut read = Vec::new();
        if let Some(addr) = self.home_addr() {
            // The MLS proposals stay at the worker (a leaf's own Remove):
            // each goes in before the commit of its epoch that may take it.
            let mut proposals = self.proposals_at_worker(group_id).await.into_iter().peekable();
            for entry in self.follow(addr, group_id, after).await {
                if entry.kind() == v1::DmEntryKind::Commit {
                    while let Some((_, bytes)) = proposals.next_if(|(epoch, _)| *epoch <= entry.epoch) {
                        self.process(group_id, &bytes);
                    }
                }
                // `from_me` is the person's account, its other leaves too;
                // this device's own messages just don't open for it.
                let content = self.process(group_id, &entry.envelope);
                read.push((entry.seq as i64, content));
            }
            for (_, bytes) in proposals {
                self.process(group_id, &bytes);
            }
            return read;
        }
        let (status, page) = self.call("GET", &format!("groups/{group_id}/messages?after={after}"), Value::Null).await;
        assert_eq!(status, 200, "{page}");
        for message in page["messages"].as_array().expect("messages") {
            let bytes = unenvelope(&message["envelope"]);
            let seq = message["seq"].as_i64().expect("a seq");
            read.push((seq, self.process(group_id, &bytes)));
        }
        read
    }

    /// The group's MLS proposals at the worker past where this device read
    /// it there, with their epochs, in order.
    async fn proposals_at_worker(&mut self, group_id: &str) -> Vec<(u64, Vec<u8>)> {
        let mut proposals = Vec::new();
        loop {
            let after = self.worker_positions.get(group_id).copied().unwrap_or(0);
            let (status, page) = self.call("GET", &format!("groups/{group_id}/messages?after={after}&limit=100"), Value::Null).await;
            if status != 200 {
                return proposals;
            }
            let messages = page["messages"].as_array().cloned().unwrap_or_default();
            for message in &messages {
                self.worker_positions.insert(group_id.to_owned(), message["seq"].as_i64().expect("a seq"));
                if message["kind"] == "proposal" {
                    proposals.push((message["epoch"].as_u64().unwrap_or_default(), unenvelope(&message["envelope"])));
                }
            }
            if messages.is_empty() || page["has_more"] != true {
                return proposals;
            }
        }
    }

    /// Processes one message of the group: what it said (None for a commit,
    /// or what it can't open), kept by its digest with who said it.
    fn process(&mut self, group_id: &str, bytes: &[u8]) -> Option<Content> {
        let incoming = self.with(|provider, _| self.load(provider, group_id).process(provider, &self.pins, bytes));
        let Ok(Incoming::Application { from, content, .. }) = incoming else { return None };
        let content = Content::decode(&content).expect("content");
        let digest = Envelope::new(bytes.to_vec()).expect("an envelope").digest().iter().map(|byte| format!("{byte:02x}")).collect();
        self.opened.insert(digest, (from.certificate.clone(), content.clone()));
        Some(content)
    }

    /// The group's entries at the person's home after `after` (DmFollow):
    /// the first answer, then the pages it pushes until none are left.
    async fn follow(&self, addr: std::net::SocketAddr, group_id: &str, after: i64) -> Vec<v1::DmEntry> {
        let group = URL_SAFE_NO_PAD.decode(group_id).expect("an id");
        let mut client = self.link(addr).await;
        let cursors = vec![v1::DmCursor { group_id: group.clone(), seq: after.max(0) as u64 }];
        let request = v1::DmFollow { leaf_id: self.device_key.clone(), cursors };
        let mut feed = match client.request(Request::DmFollow(request)).await.body {
            Body::DmFeed(feed) => feed,
            Body::Error(error) => panic!("the home didn't let it follow: {}", error.message),
            other => panic!("expected a feed, got {other:?}"),
        };
        let mut entries = std::mem::take(&mut feed.entries);
        let mut more = feed.has_more;
        while more {
            let page = tokio::time::timeout(
                PATIENCE,
                client.wait_for(|body| if let Body::DmFeed(feed) = body { feed.sent.is_none().then(|| feed.clone()) } else { None }),
            )
            .await
            .expect("the home's pages in time");
            entries.extend(page.entries);
            more = page.has_more;
        }
        entries.retain(|entry| entry.group_id == group);
        entries
    }
}

/// How long the helpers wait on a home.
const PATIENCE: std::time::Duration = std::time::Duration::from_secs(90);

/// A friend's home in a test with no Claktory of the friend's (#451): an
/// endpoint on `claktory-dm/1` that keeps what the person's home delivers,
/// in order, and delivers what the friend's device sends to it.
pub struct FriendHome {
    endpoint: iroh::Endpoint,
    /// The person's home, once a route names this one.
    toward: std::sync::Mutex<Option<iroh::EndpointId>>,
    /// What it was given, by group (base64url), in order.
    kept: std::sync::Arc<std::sync::Mutex<std::collections::HashMap<String, Vec<Vec<u8>>>>>,
}

/// A relay and an address lookup on this machine, for homes to find each
/// other by endpoint id as on the internet.
pub struct Network {
    pub relay: iroh::RelayUrl,
    pub lookup: std::sync::Arc<iroh::test_utils::DnsPkarrServer>,
    _relay: Box<dyn std::any::Any + Send + Sync>,
}

impl Network {
    pub async fn start() -> Network {
        let (_, relay, server) = iroh::test_utils::run_relay_server().await.expect("a relay");
        let lookup = std::sync::Arc::new(iroh::test_utils::DnsPkarrServer::run().await.expect("an address lookup"));
        Network { relay, lookup, _relay: Box::new(server) }
    }

    /// Puts `factory`'s tunnel on this network: before it starts (signing
    /// in starts it).
    pub fn serve(&self, factory: &factoryd::factory::Factory) {
        let lookup = self.lookup.clone();
        factory.set_tunnel_network(vec![self.relay.to_string()], move |builder| {
            builder.preset(lookup.preset()).ca_tls_config(iroh::tls::CaTlsConfig::insecure_skip_verify())
        });
    }
}

const ALPN_DM: &[u8] = b"claktory-dm/1";

impl FriendHome {
    pub async fn start(network: &Network) -> std::sync::Arc<FriendHome> {
        let endpoint = iroh::Endpoint::builder(iroh::endpoint::presets::Minimal)
            .secret_key(iroh::SecretKey::generate())
            .ca_tls_config(iroh::tls::CaTlsConfig::insecure_skip_verify())
            .relay_mode(iroh::RelayMode::Custom(network.relay.clone().into()))
            .preset(network.lookup.preset())
            .alpns(vec![ALPN_DM.to_vec()])
            .bind()
            .await
            .expect("an endpoint");
        network.lookup.on_endpoint(&endpoint.id(), PATIENCE).await.expect("published");
        let home = std::sync::Arc::new(FriendHome { endpoint: endpoint.clone(), toward: Default::default(), kept: Default::default() });
        let kept = home.kept.clone();
        tokio::spawn(async move {
            while let Some(incoming) = endpoint.accept().await {
                let kept = kept.clone();
                tokio::spawn(async move {
                    let Ok(connection) = incoming.await else { return };
                    while let Ok((mut send, mut receive)) = connection.accept_bi().await {
                        let Some(request) = read_frame(&mut receive).await else { continue };
                        let code = match request["kind"].as_str() {
                            Some("deliver") => {
                                let group = request["group_id"].as_str().unwrap_or_default().to_owned();
                                let bytes = URL_SAFE_NO_PAD.decode(request["envelope"].as_str().unwrap_or_default()).unwrap_or_default();
                                let mut kept = kept.lock().expect("kept");
                                let group = kept.entry(group).or_default();
                                if !group.contains(&bytes) {
                                    group.push(bytes);
                                }
                                "took"
                            }
                            _ => "ok",
                        };
                        let _ = write_frame(&mut send, &json!({"code": code})).await;
                    }
                });
            }
        });
        home
    }

    /// Its endpoint id.
    pub fn id(&self) -> [u8; 32] {
        *self.endpoint.id().as_bytes()
    }

    fn toward(&self, home: [u8; 32]) {
        *self.toward.lock().expect("toward") = Some(iroh::EndpointId::from_bytes(&home).expect("an endpoint id"));
    }

    /// What the person's home delivered in the group, in order.
    pub fn kept(&self, group_id: &str) -> Vec<Vec<u8>> {
        self.kept.lock().expect("kept").get(group_id).cloned().unwrap_or_default()
    }

    /// One request to the person's home, again until it takes it.
    async fn ask(&self, request: Value, done: &[&str]) {
        let home = self.toward.lock().expect("toward").expect("a route from the person's home first");
        let deadline = std::time::Instant::now() + PATIENCE;
        loop {
            let answer = async {
                let connection = self.endpoint.connect(iroh::EndpointAddr::new(home), ALPN_DM).await.ok()?;
                let (mut send, mut receive) = connection.open_bi().await.ok()?;
                write_frame(&mut send, &request).await.ok()?;
                let reply = read_frame(&mut receive).await?;
                reply["code"].as_str().map(str::to_owned)
            };
            let code = tokio::time::timeout(std::time::Duration::from_secs(10), answer).await.ok().flatten();
            if code.as_deref().is_some_and(|code| done.contains(&code)) {
                return;
            }
            assert!(std::time::Instant::now() < deadline, "the person's home never took it: {code:?}");
            tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        }
    }

    /// One delivery to the person's home, whatever it answers: its code
    /// (None when it answered nothing).
    pub async fn deliver_once(&self, group_id: &str, bytes: &[u8]) -> Option<String> {
        use sha2::Digest as _;
        let home = self.toward.lock().expect("toward").expect("a route from the person's home first");
        let id: String = sha2::Sha256::digest(bytes).iter().map(|byte| format!("{byte:02x}")).collect();
        let request = json!({"kind": "deliver", "id": id, "group_id": group_id, "envelope": URL_SAFE_NO_PAD.encode(bytes)});
        let answer = async {
            let connection = self.endpoint.connect(iroh::EndpointAddr::new(home), ALPN_DM).await.ok()?;
            let (mut send, mut receive) = connection.open_bi().await.ok()?;
            write_frame(&mut send, &request).await.ok()?;
            let reply = read_frame(&mut receive).await?;
            reply["code"].as_str().map(str::to_owned)
        };
        tokio::time::timeout(std::time::Duration::from_secs(10), answer).await.ok().flatten()
    }

    /// Delivers a message of the friend's to the person's home: how many
    /// it delivered so far.
    async fn deliver(&self, group_id: &str, bytes: Vec<u8>) -> i64 {
        use sha2::Digest as _;
        let id: String = sha2::Sha256::digest(&bytes).iter().map(|byte| format!("{byte:02x}")).collect();
        let request = json!({"kind": "deliver", "id": id, "group_id": group_id, "envelope": URL_SAFE_NO_PAD.encode(&bytes)});
        self.ask(request, &["took", "held"]).await;
        1
    }

    /// Tells the person's home the group has a commit at `epoch`.
    async fn hint(&self, group_id: &str, epoch: u64) {
        if self.toward.lock().expect("toward").is_none() {
            return;
        }
        self.ask(json!({"kind": "hint", "group_id": group_id, "epoch": epoch}), &["ok"]).await;
    }
}

async fn read_frame(receive: &mut iroh::endpoint::RecvStream) -> Option<Value> {
    use tokio::io::AsyncReadExt as _;
    let length = receive.read_u32().await.ok()? as usize;
    let mut body = vec![0; length.min(1 << 20)];
    receive.read_exact(&mut body).await.ok()?;
    serde_json::from_slice(&body).ok()
}

async fn write_frame(send: &mut iroh::endpoint::SendStream, value: &Value) -> std::io::Result<()> {
    use tokio::io::AsyncWriteExt as _;
    let body = value.to_string().into_bytes();
    send.write_u32(body.len() as u32).await?;
    send.write_all(&body).await?;
    send.finish().map_err(std::io::Error::other)?;
    Ok(())
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
        db.execute_batch(FLOOR_SCHEMA).expect("the schema");
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
            home: self.home,
            groups: Default::default(),
            positions: Default::default(),
            home_at: self.home_at.clone(),
            friend_home: self.friend_home.clone(),
            friend_positions: Default::default(),
            worker_positions: Default::default(),
            routes: Vec::new(),
            opened: Default::default(),
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

    /// Opens a proposal or an agent's draft the worker shows (by its
    /// digest, #451) from `channel` at the person's home: what the Claktory
    /// said there, read on until it's come.
    pub async fn open(&mut self, channel: &str, shown: &Value) -> Opened {
        let digest = shown["proposal_digest"].as_str().or_else(|| shown["draft_digest"].as_str()).expect("a digest").to_owned();
        let deadline = std::time::Instant::now() + PATIENCE;
        loop {
            self.catch_up(channel).await;
            if let Some((from, content)) = self.opened.get(&digest) {
                return Opened { from: from.clone(), content: content.clone(), shown: shown.clone() };
            }
            assert!(std::time::Instant::now() < deadline, "the channel never had {digest}");
            tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        }
    }

    /// Approves what it opened: that revision, by the digest the worker
    /// showed; then sends its exact text, by its agent, encrypted into the
    /// conversation's group, through the person's home. The worker's
    /// answer to the approval.
    pub async fn approve(&self, conversation_group: &str, opened: &Opened) -> (u16, Value) {
        let text = |field: &str| opened.extra(field).as_str().expect(field).to_owned();
        let claktory_id = claktory_of(&opened.from.leaf.leaf_id).expect("a Claktory's leaf").to_owned();
        let mut message = Content::text(&opened.content.client_message_id, opened.content.text.as_deref().expect("text"));
        message.by_agent =
            Some(ByAgent { claktory_id, agent_id: text("agent_id"), incarnation: text("incarnation"), name: text("agent_name") });
        let bytes = self.with(|provider, me| {
            self.load(provider, conversation_group).send(provider, me, &self.pins, &message.encode().expect("encoded")).expect("sealed")
        });
        let id = opened.shown["id"].as_str().expect("an id");
        let body = json!({"revision": opened.shown["revision"], "proposal_digest": opened.shown["proposal_digest"]});
        let (status, answer) = self.call("POST", &format!("proposals/{id}/approve"), body).await;
        // Approved: the device sends it, under the proposal's id (#451).
        if status == 200 {
            self.send(conversation_group, bytes, &opened.content.client_message_id).await;
        }
        (status, answer)
    }
}
