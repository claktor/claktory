//! The storage provider under real groups: state survives a reload, lives
//! and dies with the caller's transaction, and goes with `forget_group`.

use claktory_dm_crypto::provider::Provider;
use claktory_dm_crypto::storage::forget_group;
use openmls::prelude::*;

use crate::support::{Member, wire};

/// Alice makes a group and adds Bob; both have merged.
fn pair() -> (Member, MlsGroup, Member, MlsGroup) {
    let alice = Member::new("alice", 1);
    let bob = Member::new("bob", 2);
    let mut alice_group = alice.create_group();
    let (_commit, welcome, _info) = alice_group.add_members(&alice.provider(), &alice.signer, &[bob.key_package()]).unwrap();
    alice_group.merge_pending_commit(&alice.provider()).unwrap();
    let bob_group = bob.join(welcome);
    (alice, alice_group, bob, bob_group)
}

fn text(processed: ProcessedMessage) -> Vec<u8> {
    match processed.into_content() {
        ProcessedMessageContent::ApplicationMessage(message) => message.into_bytes(),
        other => panic!("not an application message: {other:?}"),
    }
}

#[test]
fn a_group_reloads_from_the_store() {
    let (alice, mut alice_group, bob, bob_group) = pair();
    let group_id = bob_group.group_id().clone();
    drop(bob_group);

    let mut bob_group = MlsGroup::load(bob.provider().storage(), &group_id).unwrap().expect("bob's group is stored");
    assert_eq!(bob_group.epoch(), alice_group.epoch());
    let sent = alice_group.create_message(&alice.provider(), &alice.signer, b"hello bob").unwrap();
    let processed = bob_group.process_message(&bob.provider(), wire(&sent)).unwrap();
    assert_eq!(text(processed), b"hello bob");
}

#[test]
fn state_moves_only_when_the_transaction_commits() {
    let (alice, mut alice_group, mut bob, _) = pair();
    let group_id = alice_group.group_id().clone();
    let first = alice_group.create_message(&alice.provider(), &alice.signer, b"one").unwrap();

    // Processed in a transaction that rolls back: as if it never happened,
    // so the same message decrypts again afterwards.
    {
        let tx = bob.db.transaction().unwrap();
        let provider = Provider::new(&bob.crypto, &tx, &bob.key).unwrap();
        let mut group = MlsGroup::load(provider.storage(), &group_id).unwrap().unwrap();
        assert_eq!(text(group.process_message(&provider, wire(&first)).unwrap()), b"one");
        tx.rollback().unwrap();
    }
    {
        let tx = bob.db.transaction().unwrap();
        let provider = Provider::new(&bob.crypto, &tx, &bob.key).unwrap();
        let mut group = MlsGroup::load(provider.storage(), &group_id).unwrap().unwrap();
        assert_eq!(text(group.process_message(&provider, wire(&first)).unwrap()), b"one");
        tx.commit().unwrap();
    }

    // Committed: the ratchet moved on, so a replay is refused.
    let mut group = MlsGroup::load(bob.provider().storage(), &group_id).unwrap().unwrap();
    assert!(group.process_message(&bob.provider(), wire(&first)).is_err());
}

#[test]
fn a_commit_rolls_back_with_its_transaction() {
    let (alice, mut alice_group, mut bob, _) = pair();
    let group_id = alice_group.group_id().clone();
    let (commit, _, _) = alice_group.self_update(&alice.provider(), &alice.signer, LeafNodeParameters::default()).unwrap().into_contents();
    alice_group.merge_pending_commit(&alice.provider()).unwrap();

    let before = MlsGroup::load(bob.provider().storage(), &group_id).unwrap().unwrap().epoch();
    {
        let tx = bob.db.transaction().unwrap();
        let provider = Provider::new(&bob.crypto, &tx, &bob.key).unwrap();
        let mut group = MlsGroup::load(provider.storage(), &group_id).unwrap().unwrap();
        let ProcessedMessageContent::StagedCommitMessage(staged) = group.process_message(&provider, wire(&commit)).unwrap().into_content()
        else {
            panic!("not a commit");
        };
        group.merge_staged_commit(&provider, *staged).unwrap();
        assert_eq!(group.epoch(), alice_group.epoch());
        tx.rollback().unwrap();
    }
    assert_eq!(MlsGroup::load(bob.provider().storage(), &group_id).unwrap().unwrap().epoch(), before);
}

#[test]
fn the_worker_keeps_a_public_group_in_its_own_transaction() {
    let (alice, mut alice_group, _bob, _) = pair();
    let worker = Member::new("worker", 9);
    let info = alice_group.export_group_info(alice.provider().crypto(), &alice.signer, true).unwrap();
    let MlsMessageBodyIn::GroupInfo(info) = MlsMessageIn::from(info).extract() else { panic!("not a GroupInfo") };
    let tree = alice_group.export_ratchet_tree();
    let group_id = alice_group.group_id().clone();

    let mut db = worker.db;
    {
        let tx = db.transaction().unwrap();
        let provider = Provider::public(&worker.crypto, &tx, &worker.key).unwrap();
        PublicGroup::from_external(provider.crypto(), provider.storage(), tree.into(), info, ProposalStore::default()).unwrap();
        tx.commit().unwrap();
    }

    let (commit, _, _) = alice_group.self_update(&alice.provider(), &alice.signer, LeafNodeParameters::default()).unwrap().into_contents();
    alice_group.merge_pending_commit(&alice.provider()).unwrap();
    {
        let tx = db.transaction().unwrap();
        let provider = Provider::public(&worker.crypto, &tx, &worker.key).unwrap();
        let mut public = PublicGroup::load(provider.storage(), &group_id).unwrap().expect("stored");
        let processed = public.process_message(provider.crypto(), wire(&commit)).unwrap();
        let ProcessedMessageContent::StagedCommitMessage(staged) = processed.into_content() else { panic!("not a commit") };
        public.merge_commit(provider.storage(), *staged).unwrap();
        tx.commit().unwrap();
    }
    let provider = Provider::public(&worker.crypto, &db, &worker.key).unwrap();
    let public = PublicGroup::load(provider.storage(), &group_id).unwrap().unwrap();
    assert_eq!(public.group_context().epoch(), alice_group.epoch());
}

#[test]
fn forgetting_a_group_leaves_none_of_it() {
    let (alice, alice_group, _bob, _) = pair();
    let other = alice.create_group();
    let before = alice.values();
    let gone = forget_group(&alice.db, alice_group.group_id()).unwrap();
    assert!(gone > 0);
    assert_eq!(alice.values(), before - gone as i64);
    assert!(MlsGroup::load(alice.provider().storage(), alice_group.group_id()).unwrap().is_none());
    assert!(MlsGroup::load(alice.provider().storage(), other.group_id()).unwrap().is_some());
}

/// The sealed MessageSecrets of the member's one group, as stored.
fn sealed_message_secrets(member: &Member) -> Vec<u8> {
    member
        .db
        .query_row("SELECT value FROM mls_values WHERE CAST(substr(key, 1, 14) AS TEXT) = 'MessageSecrets'", [], |row| row.get(0))
        .unwrap()
}

#[test]
fn overwritten_secrets_leave_nothing_in_the_files() {
    let dir = std::env::temp_dir().join(format!("claktory-dm-fs-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("bob.db");
    let alice = Member::new("alice", 1);
    let mut bob = Member::on_disk("bob", 2, &path);
    let mut alice_group = alice.create_group();
    let (_commit, welcome, _info) = alice_group.add_members(&alice.provider(), &alice.signer, &[bob.key_package()]).unwrap();
    alice_group.merge_pending_commit(&alice.provider()).unwrap();
    let group_id = bob.join(welcome).group_id().clone();

    let mut receive = |bob: &mut Member, text: &[u8]| {
        let sent = alice_group.create_message(&alice.provider(), &alice.signer, text).unwrap();
        let tx = bob.db.transaction().unwrap();
        let provider = Provider::new(&bob.crypto, &tx, &bob.key).unwrap();
        let mut group = MlsGroup::load(provider.storage(), &group_id).unwrap().unwrap();
        group.process_message(&provider, wire(&sent)).unwrap();
        tx.commit().unwrap();
    };
    receive(&mut bob, b"one");
    let before = sealed_message_secrets(&bob);
    receive(&mut bob, b"two");
    let after = sealed_message_secrets(&bob);
    assert_ne!(before, after);

    let scan = || {
        let mut files = Vec::new();
        for entry in std::fs::read_dir(&dir).unwrap() {
            files.extend(std::fs::read(entry.unwrap().path()).unwrap());
        }
        files
    };
    // Each sealed value ends in its own random 16-byte tag.
    let found = |files: &[u8], value: &[u8]| files.windows(16).any(|window| window == &value[value.len() - 16..]);
    let files = scan();
    assert!(found(&files, &after), "the scan finds what's there");
    assert!(!found(&files, &before), "the earlier secrets are gone from the files");

    // Forgotten, the group's secrets are gone too.
    let all: Vec<Vec<u8>> = {
        let mut statement = bob.db.prepare("SELECT value FROM mls_values WHERE grp IS NOT NULL").unwrap();
        statement.query_map([], |row| row.get(0)).unwrap().map(Result::unwrap).collect()
    };
    assert!(all.len() > 5);
    forget_group(&bob.db, &group_id).unwrap();
    let files = scan();
    for value in &all {
        assert!(!found(&files, value), "a forgotten value is gone from the files");
    }
    drop(bob);
    std::fs::remove_dir_all(&dir).unwrap();
}
