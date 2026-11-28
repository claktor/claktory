//! Groups through the crate's own API on the real store, as an endpoint
//! runs them: each step in a transaction, the group loaded again for each.

use claktory_dm_crypto::certificate::{Leaf, LeafCertificate, LeafKind};
use claktory_dm_crypto::device_list::{DeviceList, Listed};
use claktory_dm_crypto::group::{Group, Incoming, LeafKey, Me};
use claktory_dm_crypto::identity::IdentityKey;
use claktory_dm_crypto::provider::{Crypto, Provider};
use claktory_dm_crypto::storage::{
    SealKey, drop_rejoin, epoch_floor, forget_group, pending_rejoin, stash_rejoin, sweep_key_packages, take_rejoin,
};
use claktory_dm_crypto::validation::{GroupKind, Held, Pins, Trust};
use rusqlite::Connection;

use crate::support::store;

/// One device: its store and its leaf.
struct Device {
    db: Connection,
    key: SealKey,
    crypto: Crypto,
    me: Me,
    listed: Listed,
}

impl Device {
    fn new(identity: &IdentityKey, account: &str, leaf_id: &str, seed: u8) -> Device {
        let key = LeafKey::from_secret(&[seed; 32]);
        let leaf = Leaf {
            kind: LeafKind::Device,
            account_id: account.to_owned(),
            leaf_id: leaf_id.to_owned(),
            signature_key: key.public(),
            name: leaf_id.to_owned(),
            created_ms: 1,
        };
        let listed = Listed { kind: LeafKind::Device, leaf_id: leaf_id.to_owned(), signature_key: key.public() };
        let me = Me::new(key, LeafCertificate::issue(identity, leaf).unwrap()).unwrap();
        Device { db: store(), key: SealKey::from_bytes([seed; 32]), crypto: Crypto::new().unwrap(), me, listed }
    }

    /// Runs `step` in a transaction that commits.
    fn step<T>(&mut self, step: impl FnOnce(&Provider, &Me) -> T) -> T {
        let tx = self.db.transaction().unwrap();
        let out = step(&Provider::new(&self.crypto, &tx, &self.key).unwrap(), &self.me);
        tx.commit().unwrap();
        out
    }
}

#[test]
fn a_conversation_runs_on_the_store_and_survives_a_reload() {
    let (matt, ana) = (IdentityKey::from_secret(&[1; 32]), IdentityKey::from_secret(&[2; 32]));
    let mut phone = Device::new(&matt, "matt", "phone", 3);
    let mut ipad = Device::new(&ana, "ana", "ipad", 4);
    let mut pins = Pins::default();
    for (identity, account, device) in [(&matt, "matt", &phone), (&ana, "ana", &ipad)] {
        pins.first_contact(account, identity.public()).unwrap();
        let list = DeviceList { account_id: account.to_owned(), version: 1, created_ms: 1, leaves: vec![device.listed.clone()] };
        pins.accept_list(account, &list.sign(identity).unwrap().to_bytes().unwrap()).unwrap();
    }
    let kind = GroupKind::Conversation { people: ["ana".to_owned(), "matt".to_owned()] };
    let id = b"conversation-1";

    let package = ipad.step(|provider, me| me.key_package(provider).unwrap());
    let welcome = phone.step(|provider, me| {
        let mut group = Group::create(provider, me, kind.clone(), id).unwrap();
        let out = group.add(provider, me, &pins, &[package]).unwrap();
        group.merge_pending(provider).unwrap();
        out.welcome.unwrap()
    });
    ipad.step(|provider, _| Group::join(provider, &pins, kind.clone(), id, &welcome).unwrap());

    // Each side loads its group again from its own store.
    let said = phone.step(|provider, me| {
        Group::load(provider, kind.clone(), id).unwrap().unwrap().send(provider, me, &Pins::default(), b"hi Ana").unwrap()
    });
    let heard = ipad.step(|provider, _| Group::load(provider, kind.clone(), id).unwrap().unwrap().process(provider, &pins, &said).unwrap());
    let Incoming::Application { from, content, .. } = heard else { panic!("not a message: {heard:?}") };
    assert_eq!((from.account(), from.leaf_id(), content.as_slice()), ("matt", "phone", b"hi Ana".as_slice()));
    let epochs =
        [&mut phone, &mut ipad].map(|device| device.step(|provider, _| Group::load(provider, kind.clone(), id).unwrap().unwrap().epoch()));
    assert_eq!(epochs, [1, 1]);
}

/// Melone's F2: the store keeps each group's highest merged epoch beside
/// the MLS state, where forgetting the group (a rejoin's first step) leaves
/// it, so a rejoin is refused a GroupInfo from before it.
#[test]
fn the_store_keeps_each_groups_floor_past_forgetting_it() {
    let (matt, ana) = (IdentityKey::from_secret(&[1; 32]), IdentityKey::from_secret(&[2; 32]));
    let mut phone = Device::new(&matt, "matt", "phone", 3);
    let mut ipad = Device::new(&ana, "ana", "ipad", 4);
    let mut pins = Pins::default();
    for (identity, account, device) in [(&matt, "matt", &phone), (&ana, "ana", &ipad)] {
        pins.first_contact(account, identity.public()).unwrap();
        let list = DeviceList { account_id: account.to_owned(), version: 1, created_ms: 1, leaves: vec![device.listed.clone()] };
        pins.accept_list(account, &list.sign(identity).unwrap().to_bytes().unwrap()).unwrap();
    }
    let kind = GroupKind::Conversation { people: ["ana".to_owned(), "matt".to_owned()] };
    let id = b"conversation-1";
    let package = ipad.step(|provider, me| me.key_package(provider).unwrap());
    let (welcome, first) = phone.step(|provider, me| {
        let mut group = Group::create(provider, me, kind.clone(), id).unwrap();
        let out = group.add(provider, me, &pins, &[package]).unwrap();
        group.merge_pending(provider).unwrap();
        (out.welcome.unwrap(), out.group_info.unwrap())
    });
    ipad.step(|provider, _| Group::join(provider, &pins, kind.clone(), id, &welcome).unwrap());
    let update = phone.step(|provider, me| {
        let mut group = Group::load(provider, kind.clone(), id).unwrap().unwrap();
        let out = group.update(provider, me).unwrap();
        group.merge_pending(provider).unwrap();
        out
    });
    ipad.step(|provider, _| Group::load(provider, kind.clone(), id).unwrap().unwrap().process(provider, &pins, &update.commit).unwrap());
    assert_eq!([epoch_floor(&phone.db, id).unwrap(), epoch_floor(&ipad.db, id).unwrap()], [2, 2]);
    assert_eq!(epoch_floor(&ipad.db, b"another").unwrap(), 0);

    // The iPad's state breaks; it forgets the group and rejoins: never from
    // the GroupInfo of epoch 1.
    forget_group(&ipad.db, &openmls::prelude::GroupId::from_slice(id)).unwrap();
    assert_eq!(epoch_floor(&ipad.db, id).unwrap(), 2, "kept past forgetting the group");
    let floor = epoch_floor(&ipad.db, id).unwrap();
    let refused = ipad.step(|provider, me| Group::rejoin(provider, me, &pins, kind.clone(), id, &first, floor).err());
    assert!(refused.is_some_and(|error| error.to_string().contains("before epoch 2")));
    ipad.step(|provider, me| Group::rejoin(provider, me, &pins, kind.clone(), id, &update.group_info.clone().unwrap(), floor).unwrap());
}

/// Melone's F4: the store lets go of key packages whose 30 days are up, their
/// private keys with them, and keeps the others.
#[test]
fn expired_key_packages_leave_the_store() {
    let matt = IdentityKey::from_secret(&[1; 32]);
    let mut phone = Device::new(&matt, "matt", "phone", 3);
    phone.step(|provider, me| {
        me.key_package(provider).unwrap();
        me.last_resort_key_package(provider).unwrap();
    });
    let count = |db: &Connection| -> i64 {
        db.query_row("SELECT COUNT(*) FROM mls_values WHERE grp IS NULL AND substr(key, 1, 10) = ?1", [b"KeyPackage".as_slice()], |row| {
            row.get(0)
        })
        .unwrap()
    };
    assert_eq!(count(&phone.db), 2);
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs();
    assert_eq!(sweep_key_packages(&phone.db, &phone.key, now).unwrap(), 0, "within their 30 days");
    assert_eq!(count(&phone.db), 2);
    assert_eq!(sweep_key_packages(&phone.db, &phone.key, now + 31 * 24 * 60 * 60).unwrap(), 2);
    assert_eq!(count(&phone.db), 0);
}

impl Device {
    /// Runs `step` in a transaction that commits, with the transaction too,
    /// for the storage calls an endpoint makes beside OpenMLS's.
    fn step_db<T>(&mut self, step: impl FnOnce(&Provider, &Me, &Connection) -> T) -> T {
        let tx = self.db.transaction().unwrap();
        let out = step(&Provider::new(&self.crypto, &tx, &self.key).unwrap(), &self.me, &tx);
        tx.commit().unwrap();
        out
    }

    /// The group's epoch as the store has it now, and its floor.
    fn at(&mut self, kind: &GroupKind, id: &[u8]) -> (Option<u64>, u64) {
        let epoch = self.step(|provider, _| Group::load(provider, kind.clone(), id).unwrap().map(|group| group.epoch()));
        (epoch, epoch_floor(&self.db, id).unwrap())
    }

    /// Every row of the store, as it lies.
    fn rows(&self) -> Vec<Row> {
        let mut statement = self.db.prepare("SELECT key, grp, value FROM mls_values ORDER BY key").unwrap();
        statement.query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?))).unwrap().collect::<rusqlite::Result<_>>().unwrap()
    }

    fn floors(&self) -> Vec<(Vec<u8>, i64)> {
        let mut statement = self.db.prepare("SELECT grp, epoch FROM mls_floor ORDER BY grp").unwrap();
        statement.query_map([], |row| Ok((row.get(0)?, row.get(1)?))).unwrap().collect::<rusqlite::Result<_>>().unwrap()
    }
}

/// A row of `mls_values`: key, group, sealed value.
type Row = (Vec<u8>, Option<Vec<u8>>, Vec<u8>);

/// Matt's phone and Ana's iPad in a conversation at epoch 1.
fn conversation() -> (Device, Device, Pins, GroupKind, Vec<u8>) {
    let (matt, ana) = (IdentityKey::from_secret(&[1; 32]), IdentityKey::from_secret(&[2; 32]));
    let mut phone = Device::new(&matt, "matt", "phone", 3);
    let mut ipad = Device::new(&ana, "ana", "ipad", 4);
    let mut pins = Pins::default();
    for (identity, account, device) in [(&matt, "matt", &phone), (&ana, "ana", &ipad)] {
        pins.first_contact(account, identity.public()).unwrap();
        let list = DeviceList { account_id: account.to_owned(), version: 1, created_ms: 1, leaves: vec![device.listed.clone()] };
        pins.accept_list(account, &list.sign(identity).unwrap().to_bytes().unwrap()).unwrap();
    }
    let kind = GroupKind::Conversation { people: ["ana".to_owned(), "matt".to_owned()] };
    let id = b"conversation-1".to_vec();
    let package = ipad.step(|provider, me| me.key_package(provider).unwrap());
    let welcome = phone.step(|provider, me| {
        let mut group = Group::create(provider, me, kind.clone(), &id).unwrap();
        let out = group.add(provider, me, &pins, &[package]).unwrap();
        group.merge_pending(provider).unwrap();
        out.welcome.unwrap()
    });
    ipad.step(|provider, _| Group::join(provider, &pins, kind.clone(), &id, &welcome).unwrap());
    (phone, ipad, pins, kind, id)
}

/// The iPad rejoins from `info` as an endpoint does (the group forgotten
/// first, its state put aside at once): its commit.
fn rejoin(ipad: &mut Device, pins: &Pins, kind: &GroupKind, id: &[u8], info: &[u8]) -> Vec<u8> {
    ipad.step_db(|provider, me, tx| {
        forget_group(tx, &openmls::prelude::GroupId::from_slice(id)).unwrap();
        let floor = epoch_floor(tx, id).unwrap();
        let (_, out) = Group::rejoin(provider, me, pins, kind.clone(), id, info, floor).unwrap();
        stash_rejoin(tx, id, floor, &out.commit).unwrap();
        out.commit
    })
}

fn tag(device: &mut Device, kind: &GroupKind, id: &[u8]) -> Vec<u8> {
    device.step(|provider, _| Group::load(provider, kind.clone(), id).unwrap().unwrap().confirmation_tag().unwrap())
}

/// A rejoin's external commit the worker never
/// took (lost in its restart) leaves the device no state of its own epoch
/// and its floor where it was, and nothing of that state in the store; it
/// rejoins again, and only a commit the worker took raises the floor,
/// matched by its digest.
#[test]
fn a_rejoin_the_worker_never_took_leaves_no_epoch_of_its_own() {
    let (mut phone, mut ipad, pins, kind, id) = conversation();
    let update = phone.step(|provider, me| {
        let mut group = Group::load(provider, kind.clone(), &id).unwrap().unwrap();
        let out = group.update(provider, me).unwrap();
        group.merge_pending(provider).unwrap();
        out
    });
    ipad.step(|provider, _| Group::load(provider, kind.clone(), &id).unwrap().unwrap().process(provider, &pins, &update.commit).unwrap());
    assert_eq!(ipad.at(&kind, &id), (Some(2), 2));

    // Its state breaks: forgotten, then the rejoin from the phone's GroupInfo
    // of epoch 2. Until the worker takes the commit, no state and the floor
    // at 2, not 3.
    forget_group(&ipad.db, &openmls::prelude::GroupId::from_slice(&id)).unwrap();
    let (rows, floors) = (ipad.rows(), ipad.floors());
    let info = phone.step(|provider, me| Group::load(provider, kind.clone(), &id).unwrap().unwrap().group_info(provider, me).unwrap());
    let lost = rejoin(&mut ipad, &pins, &kind, &id, &info);
    assert_eq!(ipad.at(&kind, &id), (None, 2), "the epoch-3 state waits aside");
    assert_eq!(pending_rejoin(&ipad.db, &id).unwrap(), Some(3));

    // The phone commits first: the worker refuses the iPad's. Dropped,
    // nothing of epoch 3 is left: the store is as it was before the rejoin.
    let moved = phone.step(|provider, me| {
        let mut group = Group::load(provider, kind.clone(), &id).unwrap().unwrap();
        let out = group.update(provider, me).unwrap();
        group.merge_pending(provider).unwrap();
        out
    });
    let digest = claktory_dm_crypto::envelope::digest(&moved.commit);
    assert!(take_rejoin(&ipad.db, &id, &digest).is_err(), "never taken back on another commit of the same epoch");
    assert!(drop_rejoin(&ipad.db, &id).unwrap());
    assert_eq!((ipad.rows(), ipad.floors()), (rows, floors), "nothing of the dropped epoch is left");
    assert_eq!(pending_rejoin(&ipad.db, &id).unwrap(), None);
    assert!(take_rejoin(&ipad.db, &id, &claktory_dm_crypto::envelope::digest(&lost)).is_ok_and(|taken| !taken));

    // It rejoins again from epoch 3; the worker takes this one, and only
    // then does the state come back and the floor rise, in step with the
    // phone.
    let info = phone.step(|provider, me| Group::load(provider, kind.clone(), &id).unwrap().unwrap().group_info(provider, me).unwrap());
    let commit = rejoin(&mut ipad, &pins, &kind, &id, &info);
    assert_eq!(ipad.at(&kind, &id), (None, 2));
    phone.step(|provider, _| Group::load(provider, kind.clone(), &id).unwrap().unwrap().process(provider, &pins, &commit).unwrap());
    assert!(take_rejoin(&ipad.db, &id, &claktory_dm_crypto::envelope::digest(&commit)).unwrap());
    assert_eq!(ipad.at(&kind, &id), (Some(4), 4));
    assert_eq!(tag(&mut ipad, &kind, &id), tag(&mut phone, &kind, &id));
    assert!(!take_rejoin(&ipad.db, &id, &[0; 32]).unwrap(), "nothing more to take");
}

/// The worker took the rejoin's commit but its
/// answer was lost, so the device dropped the state; it rejoins again,
/// and the second external commit takes the first's leaf out: one leaf
/// of the iPad's in the tree.
#[test]
fn a_rejoin_taken_but_unheard_is_replaced_by_the_next() {
    let (mut phone, mut ipad, pins, kind, id) = conversation();
    let info = phone.step(|provider, me| Group::load(provider, kind.clone(), &id).unwrap().unwrap().group_info(provider, me).unwrap());
    let first = rejoin(&mut ipad, &pins, &kind, &id, &info);
    phone.step(|provider, _| Group::load(provider, kind.clone(), &id).unwrap().unwrap().process(provider, &pins, &first).unwrap());
    assert!(drop_rejoin(&ipad.db, &id).unwrap(), "it never heard the worker took it");

    let info = phone.step(|provider, me| Group::load(provider, kind.clone(), &id).unwrap().unwrap().group_info(provider, me).unwrap());
    let second = rejoin(&mut ipad, &pins, &kind, &id, &info);
    phone.step(|provider, _| Group::load(provider, kind.clone(), &id).unwrap().unwrap().process(provider, &pins, &second).unwrap());
    assert!(take_rejoin(&ipad.db, &id, &claktory_dm_crypto::envelope::digest(&second)).unwrap());
    let leaves = phone.step(|provider, _| Group::load(provider, kind.clone(), &id).unwrap().unwrap().leaf_ids());
    assert_eq!(leaves.iter().filter(|leaf| *leaf == "ipad").count(), 1, "{leaves:?}");
    assert_eq!(leaves.len(), 2, "{leaves:?}");
    assert_eq!(ipad.at(&kind, &id), (Some(3), 3));
    assert_eq!(tag(&mut ipad, &kind, &id), tag(&mut phone, &kind, &id));
}

/// Every member commit leaves the device on epoch N,
/// in the store and in its floor, until the worker takes it
/// (`merge_pending`); refused (`clear_pending`), it stays there. The pinned
/// OpenMLS's semantics, checked path by path: add (and so the Welcome,
/// which goes out only once the add is taken), Update, the certificate's
/// Update, the Removes of a stale leaf and of a leaf, and a leave proposal,
/// which commits nothing.
#[test]
fn every_member_commit_stays_at_its_epoch_until_taken() {
    let (matt, ana) = (IdentityKey::from_secret(&[1; 32]), IdentityKey::from_secret(&[2; 32]));
    let mut phone = Device::new(&matt, "matt", "phone", 3);
    let mut laptop = Device::new(&matt, "matt", "laptop", 5);
    let mut desk = Device::new(&matt, "matt", "desk", 6);
    let mut ipad = Device::new(&ana, "ana", "ipad", 4);
    let mut pins = Pins::default();
    pins.first_contact("matt", matt.public()).unwrap();
    pins.first_contact("ana", ana.public()).unwrap();
    let matt_v1 = DeviceList {
        account_id: "matt".to_owned(),
        version: 1,
        created_ms: 1,
        leaves: vec![phone.listed.clone(), laptop.listed.clone(), desk.listed.clone()],
    };
    pins.accept_list("matt", &matt_v1.sign(&matt).unwrap().to_bytes().unwrap()).unwrap();
    let ana_v1 = DeviceList { account_id: "ana".to_owned(), version: 1, created_ms: 1, leaves: vec![ipad.listed.clone()] };
    pins.accept_list("ana", &ana_v1.sign(&ana).unwrap().to_bytes().unwrap()).unwrap();
    let kind = GroupKind::Conversation { people: ["ana".to_owned(), "matt".to_owned()] };
    let id = b"conversation-1".to_vec();
    let packages: Vec<Vec<u8>> =
        [&mut laptop, &mut desk, &mut ipad].into_iter().map(|device| device.step(|provider, me| me.key_package(provider).unwrap())).collect();

    // Add: epoch 0 until taken; refused, still 0; made again and taken, 1.
    phone.step(|provider, me| {
        Group::create(provider, me, kind.clone(), &id).unwrap();
    });
    let add = |phone: &mut Device| {
        phone.step(|provider, me| Group::load(provider, kind.clone(), &id).unwrap().unwrap().add(provider, me, &pins, &packages).unwrap())
    };
    add(&mut phone);
    assert_eq!(phone.at(&kind, &id), (Some(0), 0), "add");
    phone.step(|provider, _| Group::load(provider, kind.clone(), &id).unwrap().unwrap().clear_pending(provider).unwrap());
    assert_eq!(phone.at(&kind, &id), (Some(0), 0), "add refused");
    let added = add(&mut phone);
    phone.step(|provider, _| Group::load(provider, kind.clone(), &id).unwrap().unwrap().merge_pending(provider).unwrap());
    assert_eq!(phone.at(&kind, &id), (Some(1), 1), "add taken");
    let welcome = added.welcome.unwrap();
    for device in [&mut laptop, &mut desk, &mut ipad] {
        device.step(|provider, _| Group::join(provider, &pins, kind.clone(), &id, &welcome).unwrap());
        assert_eq!(device.at(&kind, &id), (Some(1), 1), "joined from the Welcome of the taken add");
    }

    // Each path: made and kept, the store still at `epoch`; taken, one on.
    type Commit<'a> = Box<dyn Fn(&mut Group, &Provider, &Me) -> claktory_dm_crypto::group::Outgoing + 'a>;
    let mut epoch = 1;
    let mut check = |pins: &Pins, device: &mut Device, path: &str, commit: Commit, others: &mut [&mut Device]| {
        let out = device.step(|provider, me| commit(&mut Group::load(provider, kind.clone(), &id).unwrap().unwrap(), provider, me));
        assert_eq!(device.at(&kind, &id), (Some(epoch), epoch), "{path}: still at epoch {epoch} until taken");
        device.step(|provider, _| Group::load(provider, kind.clone(), &id).unwrap().unwrap().merge_pending(provider).unwrap());
        epoch += 1;
        assert_eq!(device.at(&kind, &id), (Some(epoch), epoch), "{path}: taken");
        for other in others.iter_mut() {
            other.step(|provider, _| Group::load(provider, kind.clone(), &id).unwrap().unwrap().process(provider, pins, &out.commit).unwrap());
            assert_eq!(other.at(&kind, &id), (Some(epoch), epoch), "{path}: another member");
        }
    };
    check(&pins, &mut phone, "update", Box::new(|group, provider, me| group.update(provider, me).unwrap()), &mut [&mut laptop, &mut desk, &mut ipad]);
    check(
        &pins,
        &mut ipad,
        "the certificate's update",
        Box::new(|group, provider, me| group.update_certificate(provider, me).unwrap()),
        &mut [&mut phone, &mut laptop, &mut desk],
    );

    // The laptop reinstalled: its list names it under a new key, so the old
    // leaf is stale, and the phone takes it out.
    let reinstalled = Device::new(&matt, "matt", "laptop", 7);
    let matt_v2 = DeviceList {
        account_id: "matt".to_owned(),
        version: 2,
        created_ms: 2,
        leaves: vec![phone.listed.clone(), reinstalled.listed.clone(), desk.listed.clone()],
    };
    pins.accept_list("matt", &matt_v2.sign(&matt).unwrap().to_bytes().unwrap()).unwrap();
    let trust = pins.clone();
    check(
        &pins,
        &mut phone,
        "the stale leaf's remove",
        Box::new(move |group, provider, me| group.remove_dead(provider, me, &trust).unwrap().unwrap()),
        &mut [&mut desk, &mut ipad],
    );
    check(&pins, &mut phone, "remove", Box::new(|group, provider, me| group.remove(provider, me, &["desk"]).unwrap()), &mut [&mut ipad]);

    // A leave proposal commits nothing: the iPad stays where it is.
    let at = ipad.at(&kind, &id);
    ipad.step(|provider, me| Group::load(provider, kind.clone(), &id).unwrap().unwrap().leave(provider, me).unwrap());
    assert_eq!(ipad.at(&kind, &id), at, "a leave proposal");
}

/// Ana takes her device C off her list (Lock
/// out); while a group still holds C, nobody sends there, Bo's devices
/// included, once they took her new list, whether or not her removal
/// reached the group. Her own device takes C out (`remove_dead`), and
/// sends go on.
#[test]
fn nobody_sends_to_a_group_holding_a_device_its_person_took_off() {
    let (bo, ana) = (IdentityKey::from_secret(&[1; 32]), IdentityKey::from_secret(&[2; 32]));
    let mut phone = Device::new(&bo, "bo", "phone", 3);
    let mut ipad = Device::new(&ana, "ana", "ipad", 4);
    let mut c = Device::new(&ana, "ana", "c", 5);
    let mut pins = Pins::default();
    pins.first_contact("bo", bo.public()).unwrap();
    pins.first_contact("ana", ana.public()).unwrap();
    let list = |version, leaves: Vec<Listed>| DeviceList { account_id: "ana".to_owned(), version, created_ms: version, leaves };
    let bo_v1 = DeviceList { account_id: "bo".to_owned(), version: 1, created_ms: 1, leaves: vec![phone.listed.clone()] };
    pins.accept_list("bo", &bo_v1.sign(&bo).unwrap().to_bytes().unwrap()).unwrap();
    pins.accept_list("ana", &list(7, vec![ipad.listed.clone(), c.listed.clone()]).sign(&ana).unwrap().to_bytes().unwrap()).unwrap();
    let kind = GroupKind::Conversation { people: ["ana".to_owned(), "bo".to_owned()] };
    let id = b"conversation-1".to_vec();
    let packages: Vec<Vec<u8>> = [&mut ipad, &mut c].into_iter().map(|device| device.step(|provider, me| me.key_package(provider).unwrap())).collect();
    let welcome = phone.step(|provider, me| {
        let mut group = Group::create(provider, me, kind.clone(), &id).unwrap();
        let out = group.add(provider, me, &pins, &packages).unwrap();
        group.merge_pending(provider).unwrap();
        out.welcome.unwrap()
    });
    for device in [&mut ipad, &mut c] {
        device.step(|provider, _| Group::join(provider, &pins, kind.clone(), &id, &welcome).unwrap());
    }
    let send = |device: &mut Device, pins: &Pins| {
        device.step(|provider, me| Group::load(provider, kind.clone(), &id).unwrap().unwrap().send(provider, me, pins, b"hi"))
    };
    assert!(send(&mut phone, &pins).is_ok());

    // v8 has no C; Bo's phone took it, Ana's removal hasn't come.
    pins.accept_list("ana", &list(8, vec![ipad.listed.clone()]).sign(&ana).unwrap().to_bytes().unwrap()).unwrap();
    let held = send(&mut phone, &pins);
    assert!(
        matches!(held, Err(claktory_dm_crypto::Error::Waiting { ref account, reason: Held::Removed }) if account == "ana"),
        "{held:?}"
    );
    let removed = phone.step(|provider, _| Group::load(provider, kind.clone(), &id).unwrap().unwrap().taken_off(&pins));
    assert_eq!(removed.iter().map(|each| each.leaf.leaf_id.as_str()).collect::<Vec<_>>(), ["c"]);
    assert!(send(&mut ipad, &pins).is_err(), "nor Ana's own devices");

    // Bo's phone never takes a friend's device out; Ana's iPad does.
    let none = phone.step(|provider, me| Group::load(provider, kind.clone(), &id).unwrap().unwrap().remove_dead(provider, me, &pins).unwrap());
    assert!(none.is_none());
    let out = ipad.step(|provider, me| {
        let mut group = Group::load(provider, kind.clone(), &id).unwrap().unwrap();
        let out = group.remove_dead(provider, me, &pins).unwrap().unwrap();
        group.merge_pending(provider).unwrap();
        out
    });
    phone.step(|provider, _| Group::load(provider, kind.clone(), &id).unwrap().unwrap().process(provider, &pins, &out.commit).unwrap());
    assert!(send(&mut phone, &pins).is_ok());
    assert!(send(&mut ipad, &pins).is_ok());
}

/// E11-8 (Fugo's liveness question on point 6): Ana locks out C, and her
/// iPad's state in a group still holding C breaks before anyone took C
/// out. It isn't stuck waiting for a Remove that may never come (C was
/// her only other device, and Bo's phone never takes a friend's device
/// out): its own lists show C going (v7 named it, v8 doesn't), so it
/// rejoins past C, nothing goes out meanwhile, and it takes C out itself.
/// A device that never took a list naming C (the newest withheld from it,
/// R5) can't tell C from a device added since, and stays refused.
#[test]
fn a_rejoin_after_a_lock_out_is_not_stuck_waiting_for_the_removal() {
    let (bo, ana) = (IdentityKey::from_secret(&[1; 32]), IdentityKey::from_secret(&[2; 32]));
    let mut phone = Device::new(&bo, "bo", "phone", 3);
    let mut ipad = Device::new(&ana, "ana", "ipad", 4);
    let mut c = Device::new(&ana, "ana", "c", 5);
    let list = |version, leaves: Vec<Listed>| {
        DeviceList { account_id: "ana".to_owned(), version, created_ms: version, leaves }.sign(&ana).unwrap().to_bytes().unwrap()
    };
    let bo_v1 = DeviceList { account_id: "bo".to_owned(), version: 1, created_ms: 1, leaves: vec![phone.listed.clone()] };
    let pinned = |lists: &[&[u8]]| {
        let mut pins = Pins::default();
        pins.first_contact("bo", bo.public()).unwrap();
        pins.first_contact("ana", ana.public()).unwrap();
        pins.accept_list("bo", &bo_v1.clone().sign(&bo).unwrap().to_bytes().unwrap()).unwrap();
        for each in lists {
            pins.accept_list("ana", each).unwrap();
        }
        pins
    };
    let (v7, v8) = (list(7, vec![ipad.listed.clone(), c.listed.clone()]), list(8, vec![ipad.listed.clone()]));
    let mut pins = pinned(&[&v7]);
    let kind = GroupKind::Conversation { people: ["ana".to_owned(), "bo".to_owned()] };
    let id = b"conversation-1".to_vec();
    let packages: Vec<Vec<u8>> = [&mut ipad, &mut c].into_iter().map(|device| device.step(|provider, me| me.key_package(provider).unwrap())).collect();
    let welcome = phone.step(|provider, me| {
        let mut group = Group::create(provider, me, kind.clone(), &id).unwrap();
        let out = group.add(provider, me, &pins, &packages).unwrap();
        group.merge_pending(provider).unwrap();
        out.welcome.unwrap()
    });
    for device in [&mut ipad, &mut c] {
        device.step(|provider, _| Group::join(provider, &pins, kind.clone(), &id, &welcome).unwrap());
    }

    // The Lock out: v8 has no C. Everyone takes it; the iPad's state breaks
    // before its Remove went out.
    pins.accept_list("ana", &v8).unwrap();
    assert!(pins.taken_off("ana", &c.listed.signature_key), "v7 named C, v8 doesn't");
    assert!(!pins.taken_off("ana", &ipad.listed.signature_key));
    forget_group(&ipad.db, &openmls::prelude::GroupId::from_slice(&id)).unwrap();
    let info = phone.step(|provider, me| Group::load(provider, kind.clone(), &id).unwrap().unwrap().group_info(provider, me).unwrap());

    // Lists withheld: v8 only, never v7. C is off a list and on none it
    // took, as a device added on a list withheld from it would be: refused.
    let withheld = pinned(&[&v8]);
    let floor = epoch_floor(&ipad.db, &id).unwrap();
    let refused = {
        let tx = ipad.db.transaction().unwrap();
        let provider = Provider::new(&ipad.crypto, &tx, &ipad.key).unwrap();
        Group::rejoin(&provider, &ipad.me, &withheld, kind.clone(), &id, &info, floor).err()
    };
    assert!(refused.is_some(), "a device on no list it took");

    // With its own lists it rejoins past C, and the worker takes it.
    let commit = rejoin(&mut ipad, &pins, &kind, &id, &info);
    phone.step(|provider, _| Group::load(provider, kind.clone(), &id).unwrap().unwrap().process(provider, &pins, &commit).unwrap());
    assert!(take_rejoin(&ipad.db, &id, &claktory_dm_crypto::envelope::digest(&commit)).unwrap());
    let send = |device: &mut Device, pins: &Pins| {
        device.step(|provider, me| Group::load(provider, kind.clone(), &id).unwrap().unwrap().send(provider, me, pins, b"hi"))
    };
    for (who, device) in [("the iPad", &mut ipad), ("Bo's phone", &mut phone)] {
        let held = send(device, &pins);
        assert!(
            matches!(held, Err(claktory_dm_crypto::Error::Waiting { ref account, reason: Held::Removed }) if account == "ana"),
            "{who}: nothing goes out while C is in: {held:?}"
        );
    }

    // It takes C out itself; sends go on, and C reads nothing more.
    let out = ipad.step(|provider, me| {
        let mut group = Group::load(provider, kind.clone(), &id).unwrap().unwrap();
        let out = group.remove_dead(provider, me, &pins).unwrap().expect("C taken out");
        group.merge_pending(provider).unwrap();
        out
    });
    phone.step(|provider, _| Group::load(provider, kind.clone(), &id).unwrap().unwrap().process(provider, &pins, &out.commit).unwrap());
    let leaves = phone.step(|provider, _| Group::load(provider, kind.clone(), &id).unwrap().unwrap().leaf_ids());
    assert_eq!(leaves, ["phone", "ipad"], "{leaves:?}");
    assert!(send(&mut phone, &pins).is_ok());
    assert!(send(&mut ipad, &pins).is_ok());
    assert_eq!(tag(&mut ipad, &kind, &id), tag(&mut phone, &kind, &id));
}
