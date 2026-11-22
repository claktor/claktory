//! Format-2 envelopes around real MLS messages: what the worker reads from
//! the framing, and what it refuses.

use claktory_dm_crypto::CIPHERSUITE;
use claktory_dm_crypto::Error;
use claktory_dm_crypto::content::{Content, MAX_TEXT};
use claktory_dm_crypto::envelope::{self, APPLICATION_MOST, Envelope, Kind};
use openmls::prelude::*;
use serde_json::json;
use sha2::{Digest as _, Sha256};
use tls_codec::Serialize as _;

use crate::support::Member;

fn bytes(message: &MlsMessageOut) -> Vec<u8> {
    message.tls_serialize_detached().unwrap()
}

#[test]
fn each_kind_reads_from_its_framing() {
    let alice = Member::new("alice", 1);
    let bob = Member::new("bob", 2);
    let mut group = alice.create_group();
    let (commit, welcome, _) = group.add_members(&alice.provider(), &alice.signer, &[bob.key_package()]).unwrap();
    group.merge_pending_commit(&alice.provider()).unwrap();
    let info = group.export_group_info(alice.provider().crypto(), &alice.signer, true).unwrap();
    let message = group.create_message(&alice.provider(), &alice.signer, b"hi").unwrap();
    let key_package = MlsMessageOut::from(bob.key_package());

    let id = group.group_id().clone();
    for (out, kind, group) in [
        (&commit, Kind::Commit, Some((id.clone(), 0))),
        (&message, Kind::Application, Some((id.clone(), 1))),
        (&welcome, Kind::Welcome, None),
        (&info, Kind::GroupInfo, None),
        (&key_package, Kind::KeyPackage, None),
    ] {
        let envelope = Envelope::new(bytes(out)).unwrap();
        let parsed = envelope.parsed();
        assert_eq!(parsed.kind, kind);
        assert_eq!(parsed.group, group);
        assert_eq!(envelope.digest().as_slice(), Sha256::digest(bytes(out)).as_slice());
        let again = Envelope::from_json(&envelope.to_json()).unwrap();
        assert_eq!(again, envelope);
    }
    // Application messages and commits go on to OpenMLS.
    assert!(Envelope::new(bytes(&message)).unwrap().parsed().into_protocol_message().is_ok());
}

#[test]
fn a_commit_the_worker_cant_read_is_refused() {
    let alice = Member::new("alice", 1);
    let bob = Member::new("bob", 2);
    // OpenMLS's default encrypts commits; ours are PublicMessages.
    let mut group = MlsGroup::builder().ciphersuite(CIPHERSUITE).build(&alice.provider(), &alice.signer, alice.credential.clone()).unwrap();
    let (commit, _, _) = group.add_members(&alice.provider(), &alice.signer, &[bob.key_package()]).unwrap();
    group.merge_pending_commit(&alice.provider()).unwrap();
    let (update, _, _) = group.self_update(&alice.provider(), &alice.signer, LeafNodeParameters::default()).unwrap().into_contents();
    assert!(matches!(Envelope::new(bytes(&commit)), Err(Error::NotMls(_))));
    assert!(matches!(Envelope::new(bytes(&update)), Err(Error::NotMls(_))));
}

#[test]
fn malformed_and_oversized_are_refused() {
    let alice = Member::new("alice", 1);
    let mut group = alice.create_group();
    let message = bytes(&group.create_message(&alice.provider(), &alice.signer, b"hi").unwrap());

    let mut trailing = message.clone();
    trailing.push(0);
    assert!(Envelope::new(trailing).is_err());
    assert!(Envelope::new(message[..message.len() - 1].to_vec()).is_err());
    assert!(Envelope::new(b"not mls".to_vec()).is_err());
    assert!(matches!(envelope::parse(&vec![0; envelope::MOST + 1]), Err(Error::TooBig { .. })));

    let b64 = |bytes: &[u8]| base64::Engine::encode(&base64::engine::general_purpose::URL_SAFE_NO_PAD, bytes);
    assert!(Envelope::from_json(&json!({"format": 2, "bytes": b64(&message)})).is_ok());
    assert!(Envelope::from_json(&json!({"format": 1, "bytes": b64(&message)})).is_err());
    assert!(Envelope::from_json(&json!({"format": 2, "bytes": b64(&message), "group": "x"})).is_err());
    let padded = base64::Engine::encode(&base64::engine::general_purpose::STANDARD, &message);
    if padded.ends_with('=') || padded.contains('+') || padded.contains('/') {
        assert!(Envelope::from_json(&json!({"format": 2, "bytes": padded})).is_err());
    }
    assert!(Envelope::from_json(&json!({"format": 2, "bytes": "A".repeat(envelope::MOST * 2)})).is_err());
}

#[test]
fn the_longest_message_fits_the_application_cap() {
    let alice = Member::new("alice", 1);
    let mut group = alice.create_group();
    let content = Content::text("abcdefghijklmnopqrstuvwxyz", &"é".repeat(MAX_TEXT / 2)).encode().unwrap();
    let message = bytes(&group.create_message(&alice.provider(), &alice.signer, &content).unwrap());
    assert!(message.len() <= APPLICATION_MOST, "{} bytes", message.len());
    let envelope = Envelope::new(message).unwrap();
    assert_eq!(envelope.parsed().kind, Kind::Application);
}
