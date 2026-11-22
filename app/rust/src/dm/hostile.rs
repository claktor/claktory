//! Hostile tests (dms-e2ee.md §9, E9) on the person's devices: a service
//! that substitutes, adds, replays and reorders, and a stale or removed
//! device, driven through what Dart calls ([`DmDevice`]). Each test is
//! named by its number in the spec's list. History transfer's (20–23) are
//! in transfer.rs's tests, beside the wire they attack.

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use claktory_dm_crypto::certificate::{Leaf, LeafCertificate, LeafKind, claktory_leaf_id};
use claktory_dm_crypto::device_list::{DeviceList, Listed};
use claktory_dm_crypto::envelope::Envelope;
use claktory_dm_crypto::group::{LeafKey, Me};
use claktory_dm_crypto::handoff::{self, Offer};
use claktory_dm_crypto::identity::IdentityKey;
use claktory_dm_crypto::validation::Pins;
use openmls::prelude::tls_codec::{Deserialize as _, Serialize as _};
use openmls::prelude::*;

use crate::api::dm::{DmContent, DmDevice, DmGroupKind, DmGroupType, DmLeafKind, DmListed, DmOutcome, DmProcessed, dm_new_secret};

fn folder() -> std::path::PathBuf {
    static NEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("claktory-dm-hostile-{}-{n}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn device(dir: &std::path::Path, name: &str) -> DmDevice {
    let path = dir.join(format!("{name}.db")).to_string_lossy().into_owned();
    DmDevice::open(path, dm_new_secret().unwrap(), dm_new_secret().unwrap(), None).unwrap().unwrap()
}

/// A new device's leaf id: its 32-byte account key, base64url.
const TABLET: &str = "dGFibGV0LXRhYmxldC10YWJsZXQtdGFibGV0LXRhYmw";

fn text(id: &str, text: &str) -> DmContent {
    DmContent { kind: "text".into(), client_message_id: id.into(), text: Some(text.into()), by_agent: None, extra_json: String::new() }
}

fn chat() -> DmGroupKind {
    DmGroupKind { kind: DmGroupType::Conversation, accounts: vec!["ana".into(), "matt".into()], claktory_id: String::new() }
}

fn listed(device: &DmDevice, leaf_id: &str) -> Listed {
    Listed { kind: LeafKind::Device, leaf_id: leaf_id.into(), signature_key: device.status().leaf_public.try_into().unwrap() }
}

/// A device list signed with [identity]: what the service could only
/// replay, unless it had the key.
fn list(identity: &IdentityKey, account: &str, version: u64, leaves: Vec<Listed>) -> Vec<u8> {
    DeviceList { account_id: account.into(), version, created_ms: version, leaves }.sign(identity).unwrap().to_bytes().unwrap()
}

fn envelope(json: &str) -> Envelope {
    Envelope::from_json(&serde_json::from_str(json).unwrap()).unwrap()
}

fn wrap(bytes: Vec<u8>) -> String {
    Envelope::new(bytes).unwrap().to_json().to_string()
}

/// Matt's phone and Ana's iPad, set up and pinning each other, in one
/// conversation the phone started (the iPad joined at seq 1). The feed's
/// position is [World::seq]; the service's latest GroupInfo [World::info].
struct World {
    dir: std::path::PathBuf,
    phone: DmDevice,
    ipad: DmDevice,
    matt: IdentityKey,
    ana: IdentityKey,
    id: Vec<u8>,
    seq: std::cell::Cell<i64>,
    info: std::cell::RefCell<String>,
}

fn world() -> World {
    let dir = folder();
    let (phone, ipad) = (device(&dir, "phone"), device(&dir, "ipad"));
    let (matt, ana) = (dm_new_secret().unwrap(), dm_new_secret().unwrap());
    let at_phone = phone.set_up_first(matt.clone(), "matt".into(), "phone".into(), "iPhone".into(), 1).unwrap();
    let at_ipad = ipad.set_up_first(ana.clone(), "ana".into(), "ipad".into(), "iPad".into(), 1).unwrap();
    phone.first_contact("ana".into(), at_ipad.identity_public).unwrap();
    phone.accept_list("ana".into(), at_ipad.device_list).unwrap();
    ipad.first_contact("matt".into(), at_phone.identity_public).unwrap();
    ipad.accept_list("matt".into(), at_phone.device_list).unwrap();
    let id = b"conversation-1".to_vec();
    phone.create_group(id.clone(), chat()).unwrap();
    let added = phone.add_members(id.clone(), ipad.key_packages(1).unwrap()).unwrap();
    assert_eq!(phone.process(id.clone(), 1, added.commit, 1).unwrap().outcome, DmOutcome::Committed);
    ipad.join_group(id.clone(), chat(), added.welcome.unwrap(), 1).unwrap();
    let key = |secret: Vec<u8>| IdentityKey::from_secret(&secret.try_into().unwrap());
    World { dir, phone, ipad, matt: key(matt), ana: key(ana), id, seq: 1.into(), info: added.group_info.unwrap().into() }
}

impl World {
    /// The service puts [envelope] next in the feed and [to] each process
    /// it, as their sync would.
    fn feed(&self, envelope: &str, to: &[&DmDevice]) -> Vec<DmProcessed> {
        self.seq.set(self.seq.get() + 1);
        self.at(self.seq.get(), envelope, to)
    }

    /// The same at [seq], whatever the feed's position.
    fn at(&self, seq: i64, envelope: &str, to: &[&DmDevice]) -> Vec<DmProcessed> {
        to.iter().map(|device| device.process(self.id.clone(), seq, envelope.into(), seq).unwrap()).collect()
    }

    /// A commit the worker took: fed to [to], and its GroupInfo kept.
    fn commit(&self, commit: crate::api::dm::DmCommit, to: &[&DmDevice]) -> Vec<DmProcessed> {
        if let Some(info) = commit.group_info {
            *self.info.borrow_mut() = info;
        }
        self.feed(&commit.commit, to)
    }

    fn epoch(&self, device: &DmDevice) -> u64 {
        device.groups().unwrap().into_iter().find(|group| group.group_id == self.id).unwrap().epoch
    }

    fn texts(&self, device: &DmDevice) -> Vec<String> {
        device.history(self.id.clone(), None, 100).unwrap().into_iter().filter_map(|message| message.content.text).collect()
    }

    /// Matt's tablet, added by the QR hand-off and listed (version 2) on
    /// both people's devices, and in the conversation.
    fn tablet(&self) -> DmDevice {
        let tablet = device(&self.dir, "tablet");
        let handoff = tablet.start_handoff(TABLET.into(), "iPad".into(), "iPadOS".into(), vec![7; 32]).unwrap();
        let sealed = self.phone.seal_handoff(handoff.qr()).unwrap();
        tablet.finish_handoff(&handoff, sealed.sealed, 2).unwrap();
        let both = vec![
            DmListed { kind: DmLeafKind::Device, leaf_id: "phone".into(), signature_key: self.phone.status().leaf_public },
            DmListed { kind: DmLeafKind::Device, leaf_id: TABLET.into(), signature_key: tablet.status().leaf_public },
        ];
        let v2 = self.phone.sign_device_list(2, 2, both).unwrap();
        tablet.accept_list("matt".into(), v2.clone()).unwrap();
        self.ipad.accept_list("matt".into(), v2).unwrap();
        let added = self.phone.add_members(self.id.clone(), tablet.key_packages(1).unwrap()).unwrap();
        let welcome = added.welcome.clone().unwrap();
        for done in self.commit(added, &[&self.phone, &self.ipad]) {
            assert_eq!(done.outcome, DmOutcome::Committed);
        }
        tablet.join_group(self.id.clone(), chat(), welcome, self.seq.get()).unwrap();
        tablet
    }
}

impl Drop for World {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// A leaf the service made: its own key, certified by [identity] (one it
/// made up, or none it has: the test passes the real one only to show the
/// certificate isn't the point).
fn ghost(identity: &IdentityKey, kind: LeafKind, account: &str, leaf_id: &str) -> Me {
    let key = LeafKey::generate().unwrap();
    let leaf = Leaf {
        kind,
        account_id: account.into(),
        leaf_id: leaf_id.into(),
        signature_key: key.public(),
        name: "ghost".into(),
        created_ms: 1,
    };
    Me::new(key, LeafCertificate::issue(identity, leaf).unwrap()).unwrap()
}

/// An external commit into the group whose GroupInfo is [info], made with
/// OpenMLS directly (no check of ours in the way) by a leaf with [key]
/// and [credential], on a store of its own: what a service holding the
/// GroupInfo can make.
fn forced_join(dir: &std::path::Path, info: &str, key: &LeafKey, credential: Credential) -> String {
    static NEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let scratch = device(dir, &format!("service-{}", NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)));
    let MlsMessageBodyIn::GroupInfo(info) = MlsMessageIn::tls_deserialize_exact(envelope(info).bytes()).unwrap().extract() else {
        panic!("not a group info")
    };
    let config =
        MlsGroupJoinConfig::builder().use_ratchet_tree_extension(true).wire_format_policy(PURE_PLAINTEXT_WIRE_FORMAT_POLICY).build();
    let with_key = CredentialWithKey { credential, signature_key: key.public().to_vec().into() };
    let commit = scratch
        .store()
        .with_tx(|provider, _| -> Result<Vec<u8>, claktory_dm_crypto::Error> {
            let (_, bundle) = MlsGroup::external_commit_builder()
                .with_config(config)
                .build_group(provider, info, with_key)
                .unwrap()
                .load_psks(provider.storage())
                .unwrap()
                .build(provider.rand(), provider.crypto(), key, |_| true)
                .unwrap()
                .finalize(provider)
                .unwrap();
            Ok(bundle.into_contents().0.tls_serialize_detached().unwrap())
        })
        .unwrap();
    wrap(commit)
}

/// A key package for [credential] signed by [key], whatever the
/// credential says.
fn key_package(scratch: &DmDevice, key: &LeafKey, credential: Credential) -> String {
    let with_key = CredentialWithKey { credential, signature_key: key.public().to_vec().into() };
    let bytes = scratch
        .store()
        .with_tx(|provider, _| -> Result<Vec<u8>, claktory_dm_crypto::Error> {
            let bundle = KeyPackage::builder().build(claktory_dm_crypto::CIPHERSUITE, provider, key, with_key).unwrap();
            Ok(MlsMessageOut::from(bundle.key_package().clone()).tls_serialize_detached().unwrap())
        })
        .unwrap();
    wrap(bytes)
}

fn not_taken(processed: &[DmProcessed]) -> bool {
    processed.iter().all(|done| matches!(done.outcome, DmOutcome::Dropped | DmOutcome::Held) && done.message.is_none())
}

fn dropped(processed: &[DmProcessed]) -> bool {
    processed.iter().all(|done| done.outcome == DmOutcome::Dropped && done.message.is_none())
}

/// Test 1: The service substitutes a contact's key package or identity:
/// refused, and a key change never happens silently.
#[test]
fn test_1_a_substituted_key_package_or_identity_is_refused_and_never_silent() {
    let w = world();
    // A device the service set up as "ana", with a key of its own.
    let fake = device(&w.dir, "fake-ana");
    let made = fake.set_up_first(dm_new_secret().unwrap(), "ana".into(), "ipad-2".into(), "iPad".into(), 1).unwrap();
    let error = w.phone.add_members(w.id.clone(), fake.key_packages(1).unwrap()).unwrap_err();
    assert!(error.to_string().contains("isn't from their security key"), "{error}");
    // Its last-resort package, and Ana's real one among them: none goes.
    let both = vec![w.ipad.key_packages(1).unwrap().remove(0), fake.last_resort_key_package().unwrap()];
    assert!(w.phone.add_members(w.id.clone(), both).is_err());
    assert_eq!(w.epoch(&w.phone), 1, "nothing committed");

    // Its identity in place of Ana's, as at first contact or as a list.
    let pinned = w.phone.pin("ana".into()).unwrap();
    assert!(w.phone.first_contact("ana".into(), made.identity_public.clone()).is_err());
    assert!(w.phone.accept_list("ana".into(), made.device_list).is_err());
    let after = w.phone.pin("ana".into()).unwrap();
    assert_eq!((after.identity_public, after.changed_unseen, after.can_send), (pinned.identity_public, false, true));

    // The one way a key changes shows: sending waits until it's seen.
    w.phone.key_changed("ana".into(), made.identity_public).unwrap();
    let pin = w.phone.pin("ana".into()).unwrap();
    assert!(pin.changed_unseen && !pin.can_send && !pin.verified);
    assert!(w.phone.send(w.id.clone(), text("m1", "hi"), 2).is_err());
}

/// Test 2: The service adds a device or Claktory leaf: every client refuses it.
#[test]
fn test_2_a_leaf_the_service_adds_is_refused_by_every_member() {
    let w = world();
    let made_up = IdentityKey::generate().unwrap();
    let members = |device: &DmDevice| device.members(w.id.clone()).unwrap().len();

    // External commits from the GroupInfo the service keeps: a device of
    // Matt's under a key it made, one of its Claktories, a credential
    // that's no certificate at all, and Ana's real leaf id under its key.
    let device_ghost = ghost(&made_up, LeafKind::Device, "matt", "phone-2");
    let claktory_ghost = ghost(&made_up, LeafKind::Claktory, "matt", &claktory_leaf_id("mini", 1));
    let ana_ghost = ghost(&made_up, LeafKind::Device, "ana", "ipad");
    let bare = LeafKey::generate().unwrap();
    let forced = [
        forced_join(&w.dir, &w.info.borrow(), &device_ghost.key, device_ghost.certificate.to_credential().unwrap()),
        forced_join(&w.dir, &w.info.borrow(), &claktory_ghost.key, claktory_ghost.certificate.to_credential().unwrap()),
        forced_join(&w.dir, &w.info.borrow(), &ana_ghost.key, ana_ghost.certificate.to_credential().unwrap()),
        forced_join(&w.dir, &w.info.borrow(), &bare, BasicCredential::new(b"matt".to_vec()).into()),
    ];
    for commit in &forced {
        let processed = w.feed(commit, &[&w.phone, &w.ipad]);
        // Never taken: dropped, or, under a key nobody has pinned, held
        // unread (R4: a rotation's commits wait for the account's keys).
        assert!(not_taken(&processed), "{processed:?}");
        assert!(processed.iter().all(|done| done.handshake));
    }
    assert_eq!((w.epoch(&w.phone), w.epoch(&w.ipad)), (1, 1));
    assert_eq!((members(&w.phone), members(&w.ipad)), (2, 2));
    // Nothing forked: they still talk, and no member line was kept.
    let said = w.phone.send(w.id.clone(), text("m1", "still us"), 3).unwrap();
    assert_eq!(w.feed(&said, &[&w.ipad])[0].outcome, DmOutcome::Message);
    assert!(w.ipad.history(w.id.clone(), None, 100).unwrap().iter().all(|message| message.members.is_none()));

    // A group the service started itself, under Matt's name, to bring
    // Ana into with a Welcome: she won't join.
    let fake_matt = device(&w.dir, "fake-matt");
    fake_matt.set_up_first(made_up.secret().to_vec(), "matt".into(), "phone".into(), "iPhone".into(), 1).unwrap();
    fake_matt.first_contact("ana".into(), w.ana.public().0.to_vec()).unwrap();
    fake_matt.accept_list("ana".into(), list(&w.ana, "ana", 1, vec![listed(&w.ipad, "ipad")])).unwrap();
    let other = b"conversation-2".to_vec();
    fake_matt.create_group(other.clone(), chat()).unwrap();
    let added = fake_matt.add_members(other.clone(), w.ipad.key_packages(1).unwrap()).unwrap();
    let joined = w.ipad.join_group(other.clone(), chat(), added.welcome.unwrap(), 1);
    assert!(joined.unwrap_err().to_string().contains("isn't from their security key"));
    assert!(w.ipad.groups().unwrap().iter().all(|group| group.group_id != other));
}

/// Test 2, held (R4, Fugo): a leaf the service makes under an identity it
/// made up is held, unread, while its account's keys are fetched. Then the
/// service serves that made-up key as the account's, with a list naming
/// the leaf. On Matt's own phone, the commit is never taken: his own key
/// moves only by a rotation message from another of his devices, so the
/// service's word changes nothing. On Ana's iPad, Matt is a friend: the
/// commit may go through only after a key change Ana sees (the accepted
/// trust-on-first-use residual), never as a silent add.
#[test]
fn test_2_held_a_leaf_under_a_made_up_key_is_never_taken_silently() {
    // A leaf of a new id, and one in the place of Matt's own phone.
    for leaf_id in ["phone-2", "phone"] {
        held_then_served(leaf_id);
    }
}

fn held_then_served(leaf_id: &str) {
    let w = world();
    let made_up = IdentityKey::generate().unwrap();
    let ghost = ghost(&made_up, LeafKind::Device, "matt", leaf_id);
    let forced = forced_join(&w.dir, &w.info.borrow(), &ghost.key, ghost.certificate.to_credential().unwrap());
    let processed = w.feed(&forced, &[&w.phone, &w.ipad]);
    assert!(processed.iter().all(|done| done.outcome == DmOutcome::Held && done.why == "matt"), "{leaf_id}: {processed:?}");
    let held = w.seq.get();
    // What the service serves for Matt now: the made-up key, and a list
    // under it naming its leaf alone.
    let served = list(
        &made_up,
        "matt",
        1,
        vec![Listed { kind: LeafKind::Device, leaf_id: leaf_id.into(), signature_key: ghost.certificate.leaf.signature_key }],
    );

    // 1. Matt's own phone: his own key moves only by a rotation message
    // from another of his devices, so nothing the service serves changes
    // it; the commit stays held.
    let own = w.phone.pin("matt".into()).unwrap();
    assert!(w.phone.key_changed("matt".into(), made_up.public().0.to_vec()).is_err());
    assert!(w.phone.accept_list("matt".into(), served.clone()).is_err());
    assert!(w.phone.first_contact("matt".into(), made_up.public().0.to_vec()).is_err());
    let again = w.at(held, &forced, &[&w.phone]);
    assert!(not_taken(&again), "{leaf_id}: {again:?}");
    let pin = w.phone.pin("matt".into()).unwrap();
    assert_eq!((pin.identity_public, pin.changed_unseen, pin.can_send), (own.identity_public, false, true));
    assert_eq!(w.epoch(&w.phone), 1);
    assert_eq!(w.phone.members(w.id.clone()).unwrap().len(), 2);

    // 2. Ana's iPad: the friend's key changes, and shows it, unseen,
    // Verified reset, sending held, before the commit counts; the line at
    // the commit says Matt's key changed.
    w.ipad.verify("matt".into()).unwrap();
    assert!(w.ipad.pin("matt".into()).unwrap().verified);
    w.ipad.key_changed("matt".into(), made_up.public().0.to_vec()).unwrap();
    w.ipad.accept_list("matt".into(), served).unwrap();
    let pin = w.ipad.pin("matt".into()).unwrap();
    assert!(pin.changed_unseen && !pin.verified && !pin.can_send, "{leaf_id}: {pin:?}");
    assert!(w.ipad.send(w.id.clone(), text("m1", "hi"), 2).is_err(), "sending waits until Ana has seen it");
    let taken = w.at(held, &forced, &[&w.ipad]);
    assert_eq!(taken[0].outcome, DmOutcome::Committed, "{leaf_id}: {taken:?}");
    let line = taken[0].message.as_ref().and_then(|message| message.members.as_ref()).expect("a member line");
    assert_eq!(line.key_changed, ["matt"], "{leaf_id}: {line:?}");
    assert!(w.ipad.pin("matt".into()).unwrap().changed_unseen);
}

/// Test 3: The service replays an old message or commit: refused by epoch and
/// generation, and nothing shows twice.
#[test]
fn test_3_a_replayed_message_or_commit_is_refused() {
    let w = world();
    let said = w.phone.send(w.id.clone(), text("m1", "once"), 2).unwrap();
    assert_eq!(w.feed(&said, &[&w.phone, &w.ipad]).iter().map(|done| done.outcome).collect::<Vec<_>>(), [DmOutcome::Message; 2]);
    // Again at a later seq: the generation was used.
    assert!(dropped(&w.feed(&said, &[&w.phone, &w.ipad])));
    // Again at a seq already processed: not even looked at.
    assert_eq!(w.at(2, &said, &[&w.ipad])[0].outcome, DmOutcome::Already);

    // A commit, then the same commit again: the epoch moved.
    let update = w.phone.update_group(w.id.clone()).unwrap();
    let commit = update.commit.clone();
    assert!(w.commit(update, &[&w.phone, &w.ipad]).iter().all(|done| done.outcome == DmOutcome::Committed));
    assert!(dropped(&w.feed(&commit, &[&w.phone, &w.ipad])));
    // The old message after the epoch moved: still refused.
    assert!(dropped(&w.feed(&said, &[&w.ipad])));
    assert_eq!((w.epoch(&w.phone), w.epoch(&w.ipad)), (2, 2));
    assert_eq!(w.texts(&w.ipad), ["once"]);

    // The message put in another of Ana's groups: not that group's.
    let other = b"conversation-2".to_vec();
    w.phone.create_group(other.clone(), chat()).unwrap();
    let added = w.phone.add_members(other.clone(), w.ipad.key_packages(1).unwrap()).unwrap();
    w.phone.commit_taken(other.clone()).unwrap();
    w.ipad.join_group(other.clone(), chat(), added.welcome.unwrap(), 1).unwrap();
    let moved = w.ipad.process(other.clone(), 2, said, 9).unwrap();
    assert_eq!(moved.outcome, DmOutcome::Dropped);
    assert!(w.ipad.history(other, None, 10).unwrap().is_empty());

    // Still in step: a new message reads.
    let next = w.ipad.send(w.id.clone(), text("a1", "after"), 10).unwrap();
    assert_eq!(w.feed(&next, &[&w.phone])[0].outcome, DmOutcome::Message);
}

/// Test 4: A format-1 body arrives in any conversation: never shown.
#[test]
fn test_4_a_format_1_body_is_never_shown() {
    let w = world();
    let own = b"self-group".to_vec();
    let mine = DmGroupKind { kind: DmGroupType::SelfGroup, accounts: vec!["matt".into()], claktory_id: String::new() };
    w.phone.create_group(own.clone(), mine).unwrap();
    let said = w.phone.send(w.id.clone(), text("m1", "in format 2"), 2).unwrap();
    let bytes = serde_json::from_str::<serde_json::Value>(&said).unwrap()["bytes"].as_str().unwrap().to_owned();
    // The same MLS bytes, and a plain body, each called format 1.
    let plain = URL_SAFE_NO_PAD.encode(br#"{"text":"hello in the clear"}"#);
    for format_1 in [
        serde_json::json!({"format": 1, "bytes": bytes}).to_string(),
        serde_json::json!({"format": 1, "bytes": plain}).to_string(),
        serde_json::json!({"format": 1, "text": "hello in the clear"}).to_string(),
    ] {
        assert!(dropped(&w.feed(&format_1, &[&w.ipad])));
        let in_self = w.phone.process(own.clone(), w.seq.get(), format_1.clone(), 1).unwrap();
        assert_eq!(in_self.outcome, DmOutcome::Dropped);
    }
    assert!(w.texts(&w.ipad).is_empty());
    assert!(w.phone.history(own, None, 10).unwrap().is_empty());
}

/// Test 5: A removed device gets new ciphertext and can't open it, and its
/// external-commit rejoin is refused.
#[test]
fn test_5_a_removed_device_reads_nothing_new_and_cant_rejoin() {
    let w = world();
    let tablet = w.tablet();
    let before = w.ipad.send(w.id.clone(), text("a1", "before"), 3).unwrap();
    assert!(w.feed(&before, &[&w.phone, &tablet]).iter().all(|done| done.outcome == DmOutcome::Message));

    // Lost: the phone lists Matt's devices without it and removes it.
    let only_phone = vec![DmListed { kind: DmLeafKind::Device, leaf_id: "phone".into(), signature_key: w.phone.status().leaf_public }];
    let v3 = w.phone.sign_device_list(3, 3, only_phone).unwrap();
    w.ipad.accept_list("matt".into(), v3).unwrap();
    // Off the list, its leaf fails the checks, so it isn't a member; but
    // unchecked it's there as it claims, to decide its removal (Fugo, #78).
    assert!(!w.ipad.members(w.id.clone()).unwrap().iter().any(|member| member.leaf_id == TABLET));
    let leaves = w.ipad.leaves(w.id.clone()).unwrap();
    let claimed = leaves.iter().find(|leaf| leaf.leaf_id == TABLET).expect("unchecked, it's there");
    assert_eq!((claimed.account.as_str(), claimed.kind), ("matt", DmLeafKind::Device));
    let removed = w.phone.remove_members(w.id.clone(), vec![TABLET.into()]).unwrap();
    w.commit(removed, &[&w.phone, &w.ipad, &tablet]);
    assert_eq!(w.phone.members(w.id.clone()).unwrap().len(), 2);
    assert_eq!(w.phone.leaves(w.id.clone()).unwrap(), w.phone.members(w.id.clone()).unwrap(), "gone, checked or not");

    // What's said after: the tablet can't open it.
    let after = w.ipad.send(w.id.clone(), text("a2", "after"), 4).unwrap();
    let processed = w.feed(&after, &[&w.phone, &tablet]);
    assert_eq!(processed[0].outcome, DmOutcome::Message);
    assert!(dropped(&processed[1..]));
    assert_eq!(w.texts(&tablet), ["before"]);

    // It rejoins from the service's GroupInfo: every member refuses it.
    let rejoin = tablet.rejoin_group(w.id.clone(), chat(), w.info.borrow().clone(), w.seq.get()).unwrap();
    let processed = w.feed(&rejoin.commit, &[&w.phone, &w.ipad]);
    assert!(dropped(&processed), "{processed:?}");
    assert_eq!((w.epoch(&w.phone), w.epoch(&w.ipad)), (3, 3));
    assert_eq!(w.ipad.members(w.id.clone()).unwrap().len(), 2);
    // Forced with OpenMLS alone, by a leaf Matt's own key certified but
    // no list names now: the same.
    let key = LeafKey::generate().unwrap();
    let leaf = Leaf {
        kind: LeafKind::Device,
        account_id: "matt".into(),
        leaf_id: TABLET.into(),
        signature_key: key.public(),
        name: "iPad".into(),
        created_ms: 9,
    };
    let certificate = LeafCertificate::issue(&w.matt, leaf).unwrap();
    let forced = forced_join(&w.dir, &w.info.borrow(), &key, certificate.to_credential().unwrap());
    assert!(dropped(&w.feed(&forced, &[&w.phone, &w.ipad])));
}

/// Test 7: A stale signed device list (a lower version) is refused, and so is
/// another list under a version already seen.
#[test]
fn test_7_a_stale_or_forked_device_list_is_refused() {
    let w = world();
    let tablet = w.tablet();
    assert_eq!(w.ipad.pin("matt".into()).unwrap().device_list_version, 2);
    let v1 = list(&w.matt, "matt", 1, vec![listed(&w.phone, "phone")]);
    let error = w.ipad.accept_list("matt".into(), v1).unwrap_err();
    assert!(error.to_string().contains("older device list"), "{error}");
    // The same version, other leaves: a fork the service shows some.
    let forked = list(&w.matt, "matt", 2, vec![listed(&w.phone, "phone")]);
    let error = w.ipad.accept_list("matt".into(), forked).unwrap_err();
    assert!(error.to_string().contains("same version"), "{error}");
    // Another account's list, signed by its own key, for Matt.
    assert!(w.ipad.accept_list("matt".into(), list(&w.ana, "ana", 9, vec![listed(&w.ipad, "ipad")])).is_err());
    let kept = w.ipad.device_list("matt".into()).unwrap().unwrap();
    assert_eq!((kept.version, kept.leaves.len()), (2, 2));
    // The tablet is still a member to Ana: nothing went stale.
    let said = tablet.send(w.id.clone(), text("t1", "from the tablet"), 5).unwrap();
    assert_eq!(w.feed(&said, &[&w.ipad])[0].outcome, DmOutcome::Message);
}

/// Test 8: Two devices commit at once: one gets `409 epoch`, and both converge.
#[test]
fn test_8_two_commits_at_once_converge() {
    let w = world();
    let (first, second) = (w.phone.update_group(w.id.clone()).unwrap(), w.ipad.update_group(w.id.clone()).unwrap());
    let late = second.commit.clone();
    // The worker took the phone's; the iPad's is refused with 409 epoch.
    w.ipad.commit_refused(w.id.clone()).unwrap();
    let processed = w.commit(first, &[&w.phone, &w.ipad]);
    assert!(processed.iter().all(|done| (done.outcome, done.epoch) == (DmOutcome::Committed, 2)), "{processed:?}");
    // The refused one, served anyway, is refused by both.
    assert!(dropped(&w.feed(&late, &[&w.phone, &w.ipad])));
    // Converged: the iPad commits again on the new epoch, and they talk.
    let again = w.ipad.update_group(w.id.clone()).unwrap();
    assert!(w.commit(again, &[&w.phone, &w.ipad]).iter().all(|done| (done.outcome, done.epoch) == (DmOutcome::Committed, 3)));
    for (from, to, id) in [(&w.phone, &w.ipad, "m1"), (&w.ipad, &w.phone, "a1")] {
        let said = from.send(w.id.clone(), text(id, id), 6).unwrap();
        assert_eq!(w.feed(&said, &[from, to]).iter().map(|done| done.outcome).collect::<Vec<_>>(), [DmOutcome::Message; 2]);
    }
}

/// Test 14: The service substitutes the Claktory's leaf at enrollment: the
/// person's device refuses it (H2). The device certifies only the key that
/// came over the tunnel; a key package under that certificate with another
/// key, or under a certificate the service made, isn't added.
#[test]
fn test_14_a_claktory_leaf_the_service_substituted_is_refused() {
    let w = world();
    let scratch = device(&w.dir, "service");
    let channel = b"channel-mini".to_vec();
    let kind = DmGroupKind { kind: DmGroupType::Channel, accounts: vec!["matt".into()], claktory_id: "mini".into() };
    w.phone.create_group(channel.clone(), kind).unwrap();

    // The Claktory's real key, from the tunnel: certified and listed.
    let real = LeafKey::generate().unwrap();
    let leaf_id = claktory_leaf_id("mini", 1);
    let certificate = w.phone.issue_certificate(DmLeafKind::Claktory, leaf_id.clone(), real.public().to_vec(), "mini".into(), 1).unwrap();
    let certificate = LeafCertificate::from_bytes(&certificate).unwrap();
    let claktory = DmListed { kind: DmLeafKind::Claktory, leaf_id: leaf_id.clone(), signature_key: real.public().to_vec() };
    let phone = DmListed { kind: DmLeafKind::Device, leaf_id: "phone".into(), signature_key: w.phone.status().leaf_public };
    w.phone.sign_device_list(2, 2, vec![phone, claktory]).unwrap();

    // The service's key package: the real certificate, its own key.
    let swapped = LeafKey::generate().unwrap();
    let package = key_package(&scratch, &swapped, certificate.to_credential().unwrap());
    assert!(w.phone.add_members(channel.clone(), vec![package]).is_err());
    // A certificate the service made for that leaf, under its own identity.
    let made = ghost(&IdentityKey::generate().unwrap(), LeafKind::Claktory, "matt", &leaf_id);
    let package = key_package(&scratch, &made.key, made.certificate.to_credential().unwrap());
    assert!(w.phone.add_members(channel.clone(), vec![package]).is_err());
    // A certificate the device signed, for a key the list doesn't name.
    let other = LeafKey::generate().unwrap();
    let signed = w.phone.issue_certificate(DmLeafKind::Claktory, leaf_id, other.public().to_vec(), "mini".into(), 1).unwrap();
    let signed = LeafCertificate::from_bytes(&signed).unwrap();
    let package = key_package(&scratch, &other, signed.to_credential().unwrap());
    assert!(w.phone.add_members(channel.clone(), vec![package]).is_err());

    // The real Claktory's own package goes in.
    let package = key_package(&scratch, &real, certificate.to_credential().unwrap());
    let added = w.phone.add_members(channel.clone(), vec![package]).unwrap();
    w.phone.commit_taken(channel.clone()).unwrap();
    assert!(added.welcome.is_some());
    assert_eq!(w.phone.members(channel).unwrap().len(), 2);
}

/// Test 15: A forged identity hand-off to a new device is refused (H3).
#[test]
fn test_15_a_forged_hand_off_is_refused() {
    let w = world();
    let attacker = IdentityKey::generate().unwrap();
    let tablet = device(&w.dir, "tablet");
    let start = || tablet.start_handoff(TABLET.into(), "iPad".into(), "iPadOS".into(), vec![7; 32]).unwrap();

    // The service knows all the code says but its secret (the one-time
    // key and the device, say, from a pasted code it saw): a bundle of its
    // own for that, under its own identity.
    let handoff = start();
    let encoded = handoff.qr().strip_prefix("claktory-dm-add:1:").unwrap().to_owned();
    let mut bytes = URL_SAFE_NO_PAD.decode(encoded).unwrap();
    bytes[64..96].fill(0);
    let guessed = Offer::from_qr(&format!("claktory-dm-add:1:{}", URL_SAFE_NO_PAD.encode(&bytes))).unwrap();
    let forged = handoff::seal(&guessed, "matt", &attacker, &Pins::default()).unwrap();
    assert!(tablet.finish_handoff(&handoff, forged.bytes, 5).is_err());
    assert!(!tablet.status().set_up);

    // A real bundle the phone sealed for another device's code.
    let other = device(&w.dir, "other");
    let elsewhere = other.start_handoff(URL_SAFE_NO_PAD.encode([9; 32]), "iPad".into(), "iPadOS".into(), vec![8; 32]).unwrap();
    let sealed = w.phone.seal_handoff(elsewhere.qr()).unwrap();
    let handoff = start();
    assert!(tablet.finish_handoff(&handoff, sealed.sealed.clone(), 5).is_err());
    // The real bundle for this code, with one byte changed.
    let handoff = start();
    let mut changed = w.phone.seal_handoff(handoff.qr()).unwrap().sealed;
    let last = changed.len() - 1;
    changed[last] ^= 1;
    assert!(tablet.finish_handoff(&handoff, changed, 5).is_err());
    // Nothing at all, or too much.
    let handoff = start();
    assert!(tablet.finish_handoff(&handoff, vec![0; 31], 5).is_err());
    let handoff = start();
    assert!(tablet.finish_handoff(&handoff, vec![0; handoff::MAX_SEALED + 1], 5).is_err());
    assert!(!tablet.status().set_up);
    assert!(tablet.transfers.receiving().is_none(), "no history key from a forged bundle");

    // A pasted code swapped for the service's own: its digits differ, so
    // the person sees it before adding anything.
    let handoff = start();
    let swapped = device(&w.dir, "swapped").start_handoff(TABLET.into(), "iPad".into(), "iPadOS".into(), vec![7; 32]).unwrap();
    let sealed = w.phone.seal_handoff(swapped.qr()).unwrap();
    assert_ne!(sealed.comparison, handoff.comparison());

    // The real one still works.
    let sealed = w.phone.seal_handoff(handoff.qr()).unwrap();
    assert_eq!(tablet.finish_handoff(&handoff, sealed.sealed, 5).unwrap().account_id, "matt");
    let _ = other;
}

/// Test 19: After a key rotation, a device list or certificate signed by the
/// old key is refused (L5).
#[test]
fn test_19_after_a_rotation_the_old_key_signs_nothing() {
    let w = world();
    let new_ana = IdentityKey::generate().unwrap();
    w.phone.key_changed("ana".into(), new_ana.public().0.to_vec()).unwrap();
    w.phone.seen("ana".into()).unwrap();

    // Lists from the old key, at any version: refused.
    for version in [1, 2, 50] {
        let old = list(&w.ana, "ana", version, vec![listed(&w.ipad, "ipad")]);
        let error = w.phone.accept_list("ana".into(), old).unwrap_err();
        assert!(error.to_string().contains("replaced"), "{error}");
    }
    // The old key back as hers, by any route: refused.
    assert!(w.phone.key_changed("ana".into(), w.ana.public().0.to_vec()).is_err());
    assert!(w.phone.first_contact("ana".into(), w.ana.public().0.to_vec()).is_err());
    // The new key's list names her iPad, but its certificate is the old
    // key's: its key package, and its messages, are refused.
    w.phone.accept_list("ana".into(), list(&new_ana, "ana", 1, vec![listed(&w.ipad, "ipad")])).unwrap();
    assert!(w.phone.add_members(w.id.clone(), w.ipad.key_packages(1).unwrap()).is_err());
    let said = w.ipad.send(w.id.clone(), text("a1", "under the old key"), 3).unwrap();
    assert!(dropped(&w.feed(&said, &[&w.phone])));
    assert!(w.texts(&w.phone).is_empty());

    // Pins from another of Matt's devices that still trusts the old key
    // don't bring it back either.
    let stale = device(&w.dir, "stale");
    let secret = w.matt.secret().to_vec();
    stale.set_up_first(secret, "matt".into(), "laptop".into(), "Mac".into(), 1).unwrap();
    stale.first_contact("ana".into(), w.ana.public().0.to_vec()).unwrap();
    let exported = stale.export_pins().unwrap();
    w.phone.import_pins(exported).unwrap();
    assert_eq!(w.phone.pin("ana".into()).unwrap().identity_public, new_ana.public().0.to_vec());
}
