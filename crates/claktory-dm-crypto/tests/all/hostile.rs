//! Hostile tests at the crate's level: what a
//! service or a stale copy can make, which no endpoint takes. The person's
//! devices' own are in app/rust/src/dm/hostile.rs; H1 (13) is in
//! group.rs's tests.

use claktory_dm_crypto::certificate::{Leaf, LeafCertificate, LeafKind, claktory_leaf_id};
use claktory_dm_crypto::device_list::{DeviceList, Listed, SignedDeviceList};
use claktory_dm_crypto::group::{Group, Incoming, LeafKey, Me};
use claktory_dm_crypto::identity::IdentityKey;
use claktory_dm_crypto::provider::{Crypto, Provider};
use claktory_dm_crypto::storage::SealKey;
use claktory_dm_crypto::validation::{self, GroupKind, Pins};
use openmls::prelude::tls_codec::{Deserialize as _, Serialize as _};
use openmls::prelude::*;
use rusqlite::Connection;

use crate::support::store;

/// One endpoint: its store and its leaf.
struct End {
    db: Connection,
    key: SealKey,
    crypto: Crypto,
    me: Me,
    seed: u8,
}

impl End {
    fn new(identity: &IdentityKey, kind: LeafKind, account: &str, leaf_id: &str, seed: u8) -> End {
        let key = LeafKey::from_secret(&[seed; 32]);
        let leaf = Leaf {
            kind,
            account_id: account.into(),
            leaf_id: leaf_id.into(),
            signature_key: key.public(),
            name: leaf_id.into(),
            created_ms: 1,
        };
        let me = Me::new(key, LeafCertificate::issue(identity, leaf).unwrap()).unwrap();
        End { db: store(), key: SealKey::from_bytes([seed; 32]), crypto: Crypto::new().unwrap(), me, seed }
    }

    fn listed(&self) -> Listed {
        let leaf = &self.me.certificate.leaf;
        Listed { kind: leaf.kind, leaf_id: leaf.leaf_id.clone(), signature_key: leaf.signature_key }
    }

    /// Runs `step` in a transaction that commits.
    fn step<T>(&mut self, step: impl FnOnce(&Provider, &Me) -> T) -> T {
        let tx = self.db.transaction().unwrap();
        let out = step(&Provider::new(&self.crypto, &tx, &self.key).unwrap(), &self.me);
        tx.commit().unwrap();
        out
    }

    /// A copy of this endpoint as it is now: a restored backup.
    fn copy(&self, path: &std::path::Path) -> End {
        let _ = std::fs::remove_file(path);
        self.db.execute("VACUUM INTO ?1", [path.to_string_lossy()]).unwrap();
        let db = Connection::open(path).unwrap();
        db.pragma_update(None, "secure_delete", "ON").unwrap();
        let me = Me::new(LeafKey::from_secret(&self.me.key.secret()), self.me.certificate.clone()).unwrap();
        End { db, key: SealKey::from_bytes([self.seed; 32]), crypto: Crypto::new().unwrap(), me, seed: self.seed }
    }
}

fn group(end: &mut End, kind: &GroupKind, id: &[u8]) -> Group {
    end.step(|provider, _| Group::load(provider, kind.clone(), id).unwrap().unwrap())
}

fn list(identity: &IdentityKey, account: &str, version: u64, leaves: Vec<Listed>) -> Vec<u8> {
    DeviceList { account_id: account.into(), version, created_ms: version, leaves }.sign(identity).unwrap().to_bytes().unwrap()
}

/// What the service, or anyone, sends as a certificate or a device list:
/// every cut and every byte changed of real ones is refused, as an error,
/// never a panic (a length longer than what follows once panicked in debug
/// builds, in tls_codec's slice reader).
#[test]
fn a_broken_certificate_or_device_list_is_refused_not_a_panic() {
    let identity = IdentityKey::generate().unwrap();
    let end = End::new(&identity, LeafKind::Device, "matt", "phone", 1);
    let certificate = end.me.certificate.to_bytes().unwrap();
    let signed = list(&identity, "matt", 1, vec![end.listed()]);
    for (what, good) in [("certificate", &certificate), ("device list", &signed)] {
        let parse = |bytes: &[u8]| match what {
            "certificate" => LeafCertificate::from_bytes(bytes).map(drop),
            _ => SignedDeviceList::from_bytes(bytes).map(drop),
        };
        assert!(parse(good).is_ok());
        for cut in 0..good.len() {
            assert!(parse(&good[..cut]).is_err(), "{what} cut at {cut}");
        }
        for at in 0..good.len() {
            for value in [0x00, 0x3f, 0x40, 0x7f, 0x80, 0xbf, 0xc0, 0xff] {
                let mut changed = good.clone();
                if changed[at] == value {
                    continue;
                }
                changed[at] = value;
                assert!(parse(&changed).is_err(), "{what} with byte {at} = {value:#x}");
            }
        }
        let mut longer = good.clone();
        longer.push(0);
        assert!(parse(&longer).is_err());
    }
    // In a credential, as an MLS member carries it.
    let bare: Credential = BasicCredential::new(b"matt".to_vec()).into();
    assert!(LeafCertificate::from_credential(&bare).is_err());
}

/// Test 17: A restored or moved copy of a Claktory can't decrypt with its old
/// leaf: moved, the Claktory enrolls a new leaf under its new registration
/// epoch and the old one is removed; a copy of the old one reads nothing
/// after, and can't come back in by itself.
#[test]
fn test_17_a_moved_or_restored_copy_of_a_claktory_reads_nothing_new() {
    let dir = std::env::temp_dir().join(format!("claktory-dm-crypto-hostile-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let matt = IdentityKey::generate().unwrap();
    let mut phone = End::new(&matt, LeafKind::Device, "matt", "phone", 1);
    let mut mini = End::new(&matt, LeafKind::Claktory, "matt", &claktory_leaf_id("mini", 1), 2);
    let mut pins = Pins::default();
    pins.first_contact("matt", matt.public()).unwrap();
    pins.accept_list("matt", &list(&matt, "matt", 1, vec![phone.listed(), mini.listed()])).unwrap();
    let kind = GroupKind::Channel { account: "matt".into(), claktory_id: "mini".into() };
    let id = b"channel-mini";

    let package = mini.step(|provider, me| me.key_package(provider).unwrap());
    let welcome = phone.step(|provider, me| {
        let mut started = Group::create(provider, me, kind.clone(), id).unwrap();
        let out = started.add(provider, me, &pins, &[package]).unwrap();
        started.merge_pending(provider).unwrap();
        out.welcome.unwrap()
    });
    mini.step(|provider, _| Group::join(provider, &pins, kind.clone(), id, &welcome).unwrap());
    let said = phone.step(|provider, me| group_of(provider, &kind, id).send(provider, me, &Pins::default(), b"before the move").unwrap());
    assert!(matches!(
        mini.step(|provider, _| group_of(provider, &kind, id).process(provider, &pins, &said)),
        Ok(Incoming::Application { .. })
    ));

    // A copy of the Claktory's leaf as it is now (a backup, or what a move
    // would carry if dm/ weren't left out).
    let mut copy = mini.copy(&dir.join("copy.db"));

    // Moved: a new registration epoch, a new leaf, listed in place of the
    // old one; the phone removes the old and adds the new.
    let mut moved = End::new(&matt, LeafKind::Claktory, "matt", &claktory_leaf_id("mini", 2), 3);
    pins.accept_list("matt", &list(&matt, "matt", 2, vec![phone.listed(), moved.listed()])).unwrap();
    let old_id = mini.me.certificate.leaf.leaf_id.clone();
    let removed = phone.step(|provider, me| {
        let mut channel = group_of(provider, &kind, id);
        let out = channel.remove(provider, me, &[old_id.as_str()]).unwrap();
        channel.merge_pending(provider).unwrap();
        out
    });
    let package = moved.step(|provider, me| me.key_package(provider).unwrap());
    let added = phone.step(|provider, me| {
        let mut channel = group_of(provider, &kind, id);
        let out = channel.add(provider, me, &pins, &[package]).unwrap();
        channel.merge_pending(provider).unwrap();
        out
    });
    moved.step(|provider, _| Group::join(provider, &pins, kind.clone(), id, added.welcome.as_ref().unwrap()).unwrap());
    let after = phone.step(|provider, me| group_of(provider, &kind, id).send(provider, me, &Pins::default(), b"after the move").unwrap());
    let heard = moved.step(|provider, _| group_of(provider, &kind, id).process(provider, &pins, &after));
    assert!(matches!(heard, Ok(Incoming::Application { .. })), "{heard:?}");

    // The copy, and the old leaf itself: whatever they process, what's said
    // after doesn't open.
    for old in [&mut copy, &mut mini] {
        let mut channel = group(old, &kind, id);
        let _ = old.step(|provider, _| channel.process(provider, &pins, &removed.commit));
        let _ = old.step(|provider, _| channel.process(provider, &pins, &added.commit));
        assert!(old.step(|provider, _| channel.process(provider, &pins, &after)).is_err());
    }
    // Its old certificate isn't a member now.
    let credential = mini.me.certificate.to_credential().unwrap();
    assert!(validation::member(&kind, &pins, &credential, &mini.me.key.public()).is_err());

    // It comes back by an external commit from the latest GroupInfo, past
    // every check of its own: the phone refuses it.
    let info = added.group_info.unwrap();
    let MlsMessageBodyIn::GroupInfo(info) = MlsMessageIn::tls_deserialize_exact(&info).unwrap().extract() else { panic!() };
    let fresh = End::new(&matt, LeafKind::Device, "matt", "scratch", 9);
    let forced = {
        let mut fresh = fresh;
        let key = LeafKey::from_secret(&copy.me.key.secret());
        let with_key = CredentialWithKey { credential, signature_key: key.public().to_vec().into() };
        fresh.step(|provider, _| {
            let config = MlsGroupJoinConfig::builder()
                .use_ratchet_tree_extension(true)
                .wire_format_policy(PURE_PLAINTEXT_WIRE_FORMAT_POLICY)
                .build();
            let (_, bundle) = MlsGroup::external_commit_builder()
                .with_config(config)
                .build_group(provider, info, with_key)
                .unwrap()
                .load_psks(provider.storage())
                .unwrap()
                .build(provider.rand(), provider.crypto(), &key, |_| true)
                .unwrap()
                .finalize(provider)
                .unwrap();
            bundle.into_contents().0.tls_serialize_detached().unwrap()
        })
    };
    let refused = phone.step(|provider, _| group_of(provider, &kind, id).process(provider, &pins, &forced));
    assert!(refused.is_err(), "{refused:?}");
    assert_eq!(phone.step(|provider, _| group_of(provider, &kind, id).epoch()), 3);
    let _ = std::fs::remove_dir_all(dir);
}

fn group_of(provider: &Provider, kind: &GroupKind, id: &[u8]) -> Group {
    Group::load(provider, kind.clone(), id).unwrap().unwrap()
}

/// Ana takes a leaf of hers out of a conversation by its
/// id. Bo's leaf under the same id (no two accounts share a device's key
/// since, but a leaf id is only what a certificate says) stays: a removal
/// never touches another person's leaf. Named with his account, it's his.
#[test]
fn a_removal_by_id_never_takes_out_another_persons_leaf_with_that_id() {
    let ana = IdentityKey::generate().unwrap();
    let bo = IdentityKey::generate().unwrap();
    let mut phone = End::new(&ana, LeafKind::Device, "ana", "ana-phone", 1);
    let mut mac = End::new(&ana, LeafKind::Device, "ana", "shared", 2);
    let mut his = End::new(&bo, LeafKind::Device, "bo", "shared", 3);
    let mut pins = Pins::default();
    pins.first_contact("ana", ana.public()).unwrap();
    pins.first_contact("bo", bo.public()).unwrap();
    pins.accept_list("ana", &list(&ana, "ana", 1, vec![phone.listed(), mac.listed()])).unwrap();
    pins.accept_list("bo", &list(&bo, "bo", 1, vec![his.listed()])).unwrap();
    let kind = GroupKind::Conversation { people: ["ana".into(), "bo".into()] };
    let id = b"ana-and-bo";
    let packages = vec![mac.step(|provider, me| me.key_package(provider).unwrap()), his.step(|provider, me| me.key_package(provider).unwrap())];
    phone.step(|provider, me| {
        let mut started = Group::create(provider, me, kind.clone(), id).unwrap();
        started.add(provider, me, &pins, &packages).unwrap();
        started.merge_pending(provider).unwrap();
    });
    let leaves = |end: &mut End| -> Vec<(String, String)> {
        let mut leaves: Vec<(String, String)> =
            group(end, &kind, id).leaves_unchecked().into_iter().map(|leaf| (leaf.account_id, leaf.leaf_id)).collect();
        leaves.sort();
        leaves
    };
    let pair = |account: &str, leaf: &str| (account.to_owned(), leaf.to_owned());
    assert_eq!(leaves(&mut phone), [pair("ana", "ana-phone"), pair("ana", "shared"), pair("bo", "shared")]);

    phone.step(|provider, me| {
        let mut conversation = group_of(provider, &kind, id);
        conversation.remove(provider, me, &["shared"]).unwrap();
        conversation.merge_pending(provider).unwrap();
    });
    assert_eq!(leaves(&mut phone), [pair("ana", "ana-phone"), pair("bo", "shared")], "hers only");
    let again = phone.step(|provider, me| group_of(provider, &kind, id).remove(provider, me, &["shared"]).map(|_| ()));
    assert!(again.is_err(), "none of hers left under that id");

    phone.step(|provider, me| {
        let mut conversation = group_of(provider, &kind, id);
        conversation.remove_of(provider, me, "bo", &["shared"]).unwrap();
        conversation.merge_pending(provider).unwrap();
    });
    assert_eq!(leaves(&mut phone), [pair("ana", "ana-phone")]);
}
