//! Groups through the crate's own API on the real store, as an endpoint
//! runs them: each step in a transaction, the group loaded again for each.

use claktory_dm_crypto::certificate::{Leaf, LeafCertificate, LeafKind};
use claktory_dm_crypto::device_list::{DeviceList, Listed};
use claktory_dm_crypto::group::{Group, Incoming, LeafKey, Me};
use claktory_dm_crypto::identity::IdentityKey;
use claktory_dm_crypto::provider::{Crypto, Provider};
use claktory_dm_crypto::storage::SealKey;
use claktory_dm_crypto::validation::{GroupKind, Pins};
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
    let said = phone.step(|provider, me| Group::load(provider, kind.clone(), id).unwrap().unwrap().send(provider, me, b"hi Ana").unwrap());
    let heard = ipad.step(|provider, _| Group::load(provider, kind.clone(), id).unwrap().unwrap().process(provider, &pins, &said).unwrap());
    let Incoming::Application { from, content } = heard else { panic!("not a message: {heard:?}") };
    assert_eq!((from.account(), from.leaf_id(), content.as_slice()), ("matt", "phone", b"hi Ana".as_slice()));
    let epochs =
        [&mut phone, &mut ipad].map(|device| device.step(|provider, _| Group::load(provider, kind.clone(), id).unwrap().unwrap().epoch()));
    assert_eq!(epochs, [1, 1]);
}
