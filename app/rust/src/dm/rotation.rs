//! Lost or stolen, then a new security key (dms-e2ee.md §2.1, R4), on the
//! person's devices through what Dart calls ([`DmDevice`]): the rotating
//! device's checks, the self group's hand-off and Fugo's acceptance rules
//! on a remaining device, each leaf's move, and a friend holding the moves
//! until the new key is in.

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;

use crate::api::dm::{
    DmContent, DmDevice, DmGroupKind, DmGroupType, DmLeafKind, DmListed, DmOutcome, DmRemoval, ROTATION_KIND, dm_new_secret, dm_public_key,
};

/// New devices' leaf ids: their 32-byte account keys, base64url.
const TABLET: &str = "dGFibGV0LXRhYmxldC10YWJsZXQtdGFibGV0LXRhYmw";
const MAC: &str = "bWFjYm9vay1tYWNib29rLW1hY2Jvb2stbWFjYm9vay0";

fn folder() -> std::path::PathBuf {
    static NEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("claktory-dm-rotation-{}-{n}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn device(dir: &std::path::Path, name: &str) -> DmDevice {
    let path = dir.join(format!("{name}.db")).to_string_lossy().into_owned();
    DmDevice::open(path, dm_new_secret().unwrap(), dm_new_secret().unwrap(), None).unwrap().unwrap()
}

fn listed(device: &DmDevice, leaf_id: &str) -> DmListed {
    DmListed { kind: DmLeafKind::Device, leaf_id: leaf_id.into(), signature_key: device.status().leaf_public }
}

fn b64(bytes: &[u8]) -> String {
    URL_SAFE_NO_PAD.encode(bytes)
}

fn rotation(id: &str, key: &[u8], identity: &[u8], removed: &[&str]) -> DmContent {
    DmContent {
        kind: ROTATION_KIND.into(),
        client_message_id: id.into(),
        text: None,
        by_agent: None,
        extra_json: serde_json::json!({"key": b64(key), "identity": b64(identity), "removed": removed}).to_string(),
    }
}

/// One group's feed, as the worker numbers it.
struct Feed {
    id: Vec<u8>,
    seq: i64,
}

impl Feed {
    /// [envelope] at the next seq, to [devices] (the sender among them, to
    /// merge its own).
    fn deliver(&mut self, envelope: &str, devices: &[&DmDevice]) -> Vec<crate::api::dm::DmProcessed> {
        self.seq += 1;
        devices.iter().map(|device| device.process(self.id.clone(), self.seq, envelope.to_owned(), self.seq).unwrap()).collect()
    }
}

#[test]
fn a_new_key_reaches_the_kept_devices_and_friends_hold_its_moves_until_they_have_it() {
    let dir = folder();
    let (phone, tablet, mac, ana) = (device(&dir, "phone"), device(&dir, TABLET), device(&dir, MAC), device(&dir, "ana"));
    let matt = phone.set_up_first(dm_new_secret().unwrap(), "matt".into(), "phone".into(), "iPhone".into(), 1).unwrap();
    let her = ana.set_up_first(dm_new_secret().unwrap(), "ana".into(), "ipad".into(), "iPad".into(), 1).unwrap();
    phone.first_contact("ana".into(), her.identity_public.clone()).unwrap();
    phone.accept_list("ana".into(), her.device_list).unwrap();
    ana.first_contact("matt".into(), matt.identity_public.clone()).unwrap();
    for (other, leaf, name) in [(&tablet, TABLET, "iPad"), (&mac, MAC, "MacBook")] {
        let offer = other.start_handoff(leaf.into(), name.into(), "x".into(), vec![1; 32]).unwrap();
        let sealed = phone.seal_handoff(offer.qr()).unwrap();
        other.finish_handoff(&offer, sealed.sealed, 2).unwrap();
    }
    let list = phone.sign_device_list(2, 3, vec![listed(&phone, "phone"), listed(&tablet, TABLET), listed(&mac, MAC)]).unwrap();
    for other in [&tablet, &mac, &ana] {
        other.accept_list("matt".into(), list.clone()).unwrap();
    }
    let mine = DmGroupKind { kind: DmGroupType::SelfGroup, accounts: vec!["matt".into()], claktory_id: String::new() };
    let chat = DmGroupKind { kind: DmGroupType::Conversation, accounts: vec!["ana".into(), "matt".into()], claktory_id: String::new() };
    let mut own = Feed { id: b"self".to_vec(), seq: 0 };
    let mut talk = Feed { id: b"chat".to_vec(), seq: 0 };
    phone.create_group(own.id.clone(), mine.clone()).unwrap();
    let added =
        phone.add_members(own.id.clone(), vec![tablet.key_packages(1).unwrap().remove(0), mac.key_packages(1).unwrap().remove(0)]).unwrap();
    own.deliver(&added.commit, &[&phone]);
    for other in [&tablet, &mac] {
        other.join_group(own.id.clone(), mine.clone(), added.welcome.clone().unwrap(), own.seq).unwrap();
    }
    phone.create_group(talk.id.clone(), chat.clone()).unwrap();
    let added = phone
        .add_members(
            talk.id.clone(),
            vec![ana.key_packages(1).unwrap().remove(0), tablet.key_packages(1).unwrap().remove(0), mac.key_packages(1).unwrap().remove(0)],
        )
        .unwrap();
    talk.deliver(&added.commit, &[&phone]);
    for other in [&ana, &tablet, &mac] {
        other.join_group(talk.id.clone(), chat.clone(), added.welcome.clone().unwrap(), talk.seq).unwrap();
    }

    // (a) A rotation message while the Mac is still in the self group: the
    // tablet ignores it.
    let early = phone.send(own.id.clone(), rotation("r0", &[9; 32], &dm_public_key(dm_new_secret().unwrap()).unwrap(), &[MAC]), 4).unwrap();
    own.deliver(&early, &[&phone, &tablet, &mac]);
    assert!(tablet.pending_rotation().unwrap().is_none());

    // The Mac is lost: Lock out is kept from its start (R5b), then the
    // removal (a list without it, out of each group).
    let lock_out = DmRemoval { remove: vec![MAC.into()], names: vec!["MacBook".into()], keep: vec![TABLET.into()], new_key: true };
    phone.begin_removal(lock_out).unwrap();
    let kept = phone.removal().unwrap().unwrap();
    assert_eq!(
        (kept.remove.clone(), kept.names.clone(), kept.keep.clone(), kept.new_key),
        (vec![MAC.to_owned()], vec!["MacBook".to_owned()], vec![TABLET.to_owned()], true)
    );
    let without = phone.sign_device_list(3, 5, vec![listed(&phone, "phone"), listed(&tablet, TABLET)]).unwrap();
    for other in [&tablet, &ana] {
        other.accept_list("matt".into(), without.clone()).unwrap();
    }
    let out = phone.remove_members(own.id.clone(), vec![MAC.into()]).unwrap();
    own.deliver(&out.commit, &[&phone, &tablet]);
    let out = phone.remove_members(talk.id.clone(), vec![MAC.into()]).unwrap();
    talk.deliver(&out.commit, &[&phone, &tablet, &ana]);

    // The rotating phone's checks: the self group must hold exactly what's
    // kept.
    let next = dm_new_secret().unwrap();
    assert!(phone.prepare_rotation(next.clone(), vec![], vec![MAC.into()], 6).is_err());
    assert!(phone.prepare_rotation(next.clone(), vec![TABLET.into(), MAC.into()], vec![], 6).is_err());
    let rotated = phone.prepare_rotation(next.clone(), vec![TABLET.into()], vec![MAC.into()], 6).unwrap();
    assert!(phone.removal().unwrap().is_some());
    assert_eq!(rotated.identity_public, dm_public_key(next.clone()).unwrap());
    // Crash and again: the same rotation; never a second new key.
    let again = phone.prepare_rotation(next.clone(), vec![TABLET.into()], vec![MAC.into()], 7).unwrap();
    assert_eq!((again.key.clone(), again.sealed.clone()), (rotated.key.clone(), rotated.sealed.clone()));
    assert!(phone.prepare_rotation(dm_new_secret().unwrap(), vec![TABLET.into()], vec![MAC.into()], 7).is_err());

    // The key goes in the self group; the tablet keeps it to act on.
    let said = phone.send(own.id.clone(), rotation("r1", &rotated.key, &rotated.identity_public, &[MAC]), 8).unwrap();
    own.deliver(&said, &[&phone, &tablet]);
    phone.rotation_said().unwrap();
    assert!(phone.rotation().unwrap().unwrap().said);
    let pending = tablet.pending_rotation().unwrap().unwrap();
    assert_eq!((pending.from_leaf.as_str(), pending.from_name.as_str()), ("phone", "iPhone"));
    assert_eq!(pending.identity_public, rotated.identity_public);
    // One naming the key the person has now is no rotation: ignored.
    let current = phone.send(own.id.clone(), rotation("r2", &[3; 32], &matt.identity_public, &[MAC]), 9).unwrap();
    own.deliver(&current, &[&phone, &tablet]);
    assert_eq!(tablet.pending_rotation().unwrap().unwrap().identity_public, rotated.identity_public);

    // The worker took the new key: the phone takes it, and moves.
    phone.finish_rotation(next.clone()).unwrap();
    assert!(phone.rotation().unwrap().is_none());
    // The Lock out ends with it, and not before.
    assert!(phone.removal().unwrap().is_none());
    assert_eq!(phone.status().identity_public, Some(rotated.identity_public.clone()));
    assert!(phone.groups().unwrap().iter().all(|group| !group.moved));
    let moved = phone.move_group(own.id.clone()).unwrap();
    let done = own.deliver(&moved.commit, &[&phone, &tablet]);
    assert_eq!(done[0].outcome, DmOutcome::Committed);
    // The tablet doesn't have the new key yet: the move is held, unread.
    assert_eq!((done[1].outcome, done[1].why.as_str()), (DmOutcome::Held, "matt"));

    // Its bundle opens with the key from the self group; then the held
    // move goes through.
    let secret = tablet.open_rotation(rotated.sealed.clone()).unwrap();
    assert_eq!(secret, next);
    assert!(tablet.take_rotation(dm_new_secret().unwrap(), rotated.sealed.clone()).is_err(), "another key than the message named");
    tablet.take_rotation(secret, rotated.sealed.clone()).unwrap();
    assert!(tablet.pending_rotation().unwrap().is_none());
    assert_eq!(tablet.status().identity_public, Some(rotated.identity_public.clone()));
    assert_eq!(tablet.process(own.id.clone(), own.seq, moved.commit.clone(), own.seq).unwrap().outcome, DmOutcome::Committed);
    // (d) The old message again, now: nothing pending.
    assert!(tablet.pending_rotation().unwrap().is_none());
    let tablet_moves = tablet.move_group(own.id.clone()).unwrap();
    let done = own.deliver(&tablet_moves.commit, &[&tablet, &phone]);
    assert!(done.iter().all(|done| done.outcome == DmOutcome::Committed), "{done:?}");

    // In the conversation: Ana holds the phone's move until she has Matt's
    // new key, then takes it: a line that Matt's key changed, here before
    // anything said under it, and none saying anyone came or went.
    let moved = phone.move_group(talk.id.clone()).unwrap();
    let done = talk.deliver(&moved.commit, &[&phone, &tablet, &ana]);
    assert_eq!((done[0].outcome, done[1].outcome), (DmOutcome::Committed, DmOutcome::Committed));
    assert_eq!((done[2].outcome, done[2].why.as_str()), (DmOutcome::Held, "matt"));
    ana.key_changed("matt".into(), rotated.identity_public.clone()).unwrap();
    ana.accept_list("matt".into(), rotated.device_list.clone()).unwrap();
    let taken = ana.process(talk.id.clone(), talk.seq, moved.commit.clone(), talk.seq).unwrap();
    assert_eq!(taken.outcome, DmOutcome::Committed);
    let members = taken.message.and_then(|message| message.members).expect("a line");
    assert!(members.added.is_empty() && members.removed.is_empty(), "{members:?}");
    assert_eq!(members.key_changed, ["matt"]);
    // Matt's own devices say nothing of their own key.
    assert!(done[1].message.is_none(), "{:?}", done[1].message);
    // The tablet moves after it: said once, not again.
    let tablet_moved = tablet.move_group(talk.id.clone()).unwrap();
    let done = talk.deliver(&tablet_moved.commit, &[&tablet, &phone, &ana]);
    assert!(done.iter().all(|done| done.outcome == DmOutcome::Committed), "{done:?}");
    assert!(done[2].message.is_none(), "{:?}", done[2].message);
    let pin = ana.pin("matt".into()).unwrap();
    assert!(pin.changed_unseen && !pin.can_send);
    // (b) What's said in a conversation is never a rotation for the tablet.
    ana.seen("matt".into()).unwrap();
    let sly = ana.send(talk.id.clone(), rotation("x", &[1; 32], &dm_public_key(dm_new_secret().unwrap()).unwrap(), &[]), 10).unwrap();
    talk.deliver(&sly, &[&ana, &tablet]);
    assert!(tablet.pending_rotation().unwrap().is_none());
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn a_removed_device_is_committed_out_of_every_group_and_reads_nothing_after() {
    let dir = folder();
    let (phone, tablet, mac, ana) = (device(&dir, "phone"), device(&dir, TABLET), device(&dir, MAC), device(&dir, "ana"));
    let matt = phone.set_up_first(dm_new_secret().unwrap(), "matt".into(), "phone".into(), "iPhone".into(), 1).unwrap();
    let her = ana.set_up_first(dm_new_secret().unwrap(), "ana".into(), "ipad".into(), "iPad".into(), 1).unwrap();
    phone.first_contact("ana".into(), her.identity_public.clone()).unwrap();
    phone.accept_list("ana".into(), her.device_list).unwrap();
    ana.first_contact("matt".into(), matt.identity_public.clone()).unwrap();
    for (other, leaf, name) in [(&tablet, TABLET, "iPad"), (&mac, MAC, "MacBook")] {
        let offer = other.start_handoff(leaf.into(), name.into(), "x".into(), vec![1; 32]).unwrap();
        let sealed = phone.seal_handoff(offer.qr()).unwrap();
        other.finish_handoff(&offer, sealed.sealed, 2).unwrap();
    }
    let list = phone.sign_device_list(2, 3, vec![listed(&phone, "phone"), listed(&tablet, TABLET), listed(&mac, MAC)]).unwrap();
    for other in [&tablet, &mac, &ana] {
        other.accept_list("matt".into(), list.clone()).unwrap();
    }
    let mine = DmGroupKind { kind: DmGroupType::SelfGroup, accounts: vec!["matt".into()], claktory_id: String::new() };
    let chat = DmGroupKind { kind: DmGroupType::Conversation, accounts: vec!["ana".into(), "matt".into()], claktory_id: String::new() };
    let mut own = Feed { id: b"self".to_vec(), seq: 0 };
    let mut talk = Feed { id: b"chat".to_vec(), seq: 0 };
    phone.create_group(own.id.clone(), mine.clone()).unwrap();
    let added =
        phone.add_members(own.id.clone(), vec![tablet.key_packages(1).unwrap().remove(0), mac.key_packages(1).unwrap().remove(0)]).unwrap();
    own.deliver(&added.commit, &[&phone]);
    for other in [&tablet, &mac] {
        other.join_group(own.id.clone(), mine.clone(), added.welcome.clone().unwrap(), own.seq).unwrap();
    }
    phone.create_group(talk.id.clone(), chat.clone()).unwrap();
    let added = phone
        .add_members(
            talk.id.clone(),
            vec![ana.key_packages(1).unwrap().remove(0), tablet.key_packages(1).unwrap().remove(0), mac.key_packages(1).unwrap().remove(0)],
        )
        .unwrap();
    talk.deliver(&added.commit, &[&phone]);
    for other in [&ana, &tablet, &mac] {
        other.join_group(talk.id.clone(), chat.clone(), added.welcome.clone().unwrap(), talk.seq).unwrap();
    }

    // The Mac is taken off the list first: it fails the checks, so it's no
    // member any more, but every leaf is still in the tree until a commit.
    let without = phone.sign_device_list(3, 5, vec![listed(&phone, "phone"), listed(&tablet, TABLET)]).unwrap();
    for other in [&tablet, &ana] {
        other.accept_list("matt".into(), without.clone()).unwrap();
    }
    for group in [&own.id, &talk.id] {
        assert!(!phone.members(group.clone()).unwrap().iter().any(|member| member.leaf_id == MAC));
        assert!(phone.leaf_ids(group.clone()).unwrap().iter().any(|leaf| leaf == MAC));
    }
    // A new key can't be made while it's still there: said plainly.
    let refused = phone.prepare_rotation(dm_new_secret().unwrap(), vec![TABLET.into()], vec![MAC.into()], 6).unwrap_err();
    assert_eq!(refused.to_string(), "a device being removed is still in your self group");
    let refused = phone.prepare_rotation(dm_new_secret().unwrap(), vec![TABLET.into()], vec![], 6).unwrap_err();
    assert!(refused.to_string().contains("isn't on your device list any more"), "{refused}");

    // Removed by its leaf id, though it doesn't pass: out of both groups,
    // each a new epoch.
    let before = phone.groups().unwrap();
    let out = phone.remove_members(own.id.clone(), vec![MAC.into()]).unwrap();
    let done = own.deliver(&out.commit, &[&phone, &tablet, &mac]);
    assert_eq!((done[0].outcome, done[1].outcome), (DmOutcome::Committed, DmOutcome::Committed));
    let out = phone.remove_members(talk.id.clone(), vec![MAC.into()]).unwrap();
    let done = talk.deliver(&out.commit, &[&phone, &tablet, &ana, &mac]);
    assert!(done[..3].iter().all(|done| done.outcome == DmOutcome::Committed), "{done:?}");
    for group in [&own.id, &talk.id] {
        assert!(!phone.leaf_ids(group.clone()).unwrap().iter().any(|leaf| leaf == MAC));
    }
    let after = phone.groups().unwrap();
    for group in &after {
        let was = before.iter().find(|each| each.group_id == group.group_id).unwrap();
        assert!(group.epoch > was.epoch, "{group:?} {was:?}");
    }
    // Ana, who took the list without it first, still sees it go; so does
    // the phone that removed it.
    let line = done[2].message.clone().and_then(|message| message.members).expect("a line");
    assert_eq!(line.removed.iter().map(|member| member.leaf_id.as_str()).collect::<Vec<_>>(), [MAC]);
    let line = done[0].message.clone().and_then(|message| message.members).expect("the phone's line");
    assert_eq!(line.removed.iter().map(|member| (member.leaf_id.as_str(), member.name.as_str())).collect::<Vec<_>>(), [(MAC, "MacBook")]);

    // The Mac, its store intact, reads nothing said after, in either group.
    let text = |id: &str| DmContent {
        kind: "text".into(),
        client_message_id: id.into(),
        text: Some("after".into()),
        by_agent: None,
        extra_json: String::new(),
    };
    let said = phone.send(own.id.clone(), text("s1"), 7).unwrap();
    let heard = own.deliver(&said, &[&tablet, &mac]);
    assert_eq!(heard[0].outcome, DmOutcome::Message);
    assert_ne!(heard[1].outcome, DmOutcome::Message, "{:?}", heard[1]);
    let said = ana.send(talk.id.clone(), text("t1"), 8).unwrap();
    let heard = talk.deliver(&said, &[&phone, &mac]);
    assert_eq!(heard[0].outcome, DmOutcome::Message);
    assert_ne!(heard[1].outcome, DmOutcome::Message, "{:?}", heard[1]);
    assert!(mac.history(talk.id.clone(), None, 50).unwrap().iter().all(|message| message.content.text.as_deref() != Some("after")));
}
