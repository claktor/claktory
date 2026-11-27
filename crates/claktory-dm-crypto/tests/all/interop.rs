//! RFC 9420's interoperability vectors (github.com/mlswg/mls-implementations,
//! test-vectors/, at 2034fe72), suite 0x0001 only, run against our backend
//! (libcrux) and our storage: that it agrees with every other implementation
//! on every derivation.
//!
//! The passive-client vectors join by a Welcome and process every proposal
//! and commit after it, checking each epoch's authenticator: key schedule,
//! TreeKEM, HPKE, signatures, transcript hashes and the secret tree, all
//! through the public API.

use claktory_dm_crypto::CIPHERSUITE;
use openmls::prelude::*;
use openmls::schedule::PreSharedKeyId;
use openmls_traits::storage::{CURRENT_VERSION, Entity, StorageProvider as _, traits};
use openmls_traits::types::HpkePrivateKey;
use serde::{Deserialize, Serialize};
use serde_json::json;
use tls_codec::Deserialize as _;

use crate::support::Member;

pub fn hex(text: &str) -> Vec<u8> {
    (0..text.len()).step_by(2).map(|at| u8::from_str_radix(&text[at..at + 2], 16).unwrap()).collect()
}

pub fn vectors(name: &str) -> Vec<serde_json::Value> {
    let path = format!("{}/tests/vectors/rfc9420/{name}.json", env!("CARGO_MANIFEST_DIR"));
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

/// OpenMLS's own leaf key pair, as it stores one.
#[derive(Serialize, Deserialize)]
struct EncryptionKeyPair {
    public_key: openmls::treesync::EncryptionKey,
    private_key: serde_json::Value,
}
impl Entity<CURRENT_VERSION> for EncryptionKeyPair {}
impl traits::HpkeKeyPair<CURRENT_VERSION> for EncryptionKeyPair {}

fn private(bytes: Vec<u8>) -> serde_json::Value {
    serde_json::to_value(HpkePrivateKey::from(bytes)).unwrap()
}

/// What a passive client does with each vector: take its KeyPackage's
/// private keys, join, then follow every epoch.
fn passive(test: &serde_json::Value) -> Result<(), String> {
    assert_eq!(test["cipher_suite"], 1);
    let client = Member::new("passive", 7);
    let provider = client.provider();

    for psk in test["external_psks"].as_array().unwrap() {
        PreSharedKeyId::external(hex(psk["psk_id"].as_str().unwrap()), vec![])
            .store(&provider, &hex(psk["psk"].as_str().unwrap()))
            .map_err(|error| format!("psk: {error:?}"))?;
    }

    let MlsMessageBodyIn::KeyPackage(key_package) =
        MlsMessageIn::tls_deserialize_exact(hex(test["key_package"].as_str().unwrap())).unwrap().extract()
    else {
        panic!("not a KeyPackage")
    };
    // Its lifetime ended long ago, so it's taken as it is rather than
    // validated now; the Welcome is what's checked.
    let key_package: KeyPackage = match key_package.clone().validate(provider.crypto(), ProtocolVersion::Mls10) {
        Ok(key_package) => key_package,
        Err(KeyPackageVerifyError::LifetimeError(_)) => {
            serde_json::from_value(serde_json::to_value(&key_package).unwrap()).map_err(|error| format!("key package: {error}"))?
        }
        Err(error) => return Err(format!("key package: {error:?}")),
    };
    assert_eq!(key_package.ciphersuite(), CIPHERSUITE);
    let encryption_priv = hex(test["encryption_priv"].as_str().unwrap());
    let bundle: KeyPackageBundle = serde_json::from_value(json!({
        "key_package": key_package,
        "private_init_key": private(hex(test["init_priv"].as_str().unwrap())),
        "private_encryption_key": {"key": private(encryption_priv.clone())},
    }))
    .unwrap();
    let hash_ref = key_package.hash_ref(provider.crypto()).unwrap();
    provider.storage().write_key_package(&hash_ref, &bundle).unwrap();
    let public_key = key_package.leaf_node().encryption_key().clone();
    let pair = EncryptionKeyPair { public_key: public_key.clone(), private_key: json!({"key": private(encryption_priv)}) };
    provider.storage().write_encryption_key_pair(&public_key, &pair).unwrap();

    let config = MlsGroupJoinConfig::builder()
        .use_ratchet_tree_extension(true)
        .wire_format_policy(MIXED_PLAINTEXT_WIRE_FORMAT_POLICY)
        .number_of_resumption_psks(16)
        .build();
    let MlsMessageBodyIn::Welcome(welcome) = MlsMessageIn::tls_deserialize_exact(hex(test["welcome"].as_str().unwrap())).unwrap().extract()
    else {
        panic!("not a Welcome")
    };
    let mut join = StagedWelcome::build_from_welcome(&provider, &config, welcome)
        .map_err(|error| format!("welcome: {error:?}"))?
        // The vectors were made years ago; their leaves' lifetimes are over.
        .skip_lifetime_validation();
    if let Some(tree) = test["ratchet_tree"].as_str() {
        join = join.with_ratchet_tree(RatchetTreeIn::tls_deserialize_exact(hex(tree)).unwrap());
    }
    let mut group = join.build().and_then(|staged| staged.into_group(&provider)).map_err(|error| format!("join: {error:?}"))?;
    if group.epoch_authenticator().as_slice() != hex(test["initial_epoch_authenticator"].as_str().unwrap()) {
        return Err("the initial epoch authenticator differs".to_owned());
    }

    for (at, epoch) in test["epochs"].as_array().unwrap().iter().enumerate() {
        let messages = epoch["proposals"].as_array().unwrap().iter().chain([&epoch["commit"]]);
        for message in messages {
            let message = MlsMessageIn::tls_deserialize_exact(hex(message.as_str().unwrap())).unwrap();
            let processed = group
                .process_message(&provider, message.try_into_protocol_message().unwrap())
                .map_err(|error| format!("epoch {at}: {error:?}"))?;
            match processed.into_content() {
                ProcessedMessageContent::ProposalMessage(proposal) => group.store_pending_proposal(provider.storage(), *proposal).unwrap(),
                ProcessedMessageContent::StagedCommitMessage(commit) => {
                    group.merge_staged_commit(&provider, *commit).map_err(|error| format!("epoch {at}: {error:?}"))?
                }
                other => return Err(format!("epoch {at}: unexpected {other:?}")),
            }
        }
        if group.epoch_authenticator().as_slice() != hex(epoch["epoch_authenticator"].as_str().unwrap()) {
            return Err(format!("epoch {at}'s authenticator differs"));
        }
    }
    Ok(())
}

fn run(name: &str) {
    let tests = vectors(name);
    assert!(!tests.is_empty());
    let failed: Vec<String> =
        tests.iter().enumerate().filter_map(|(at, test)| passive(test).err().map(|error| format!("#{at}: {error}"))).collect();
    assert!(failed.is_empty(), "{name}: {} of {} failed:\n{}", failed.len(), tests.len(), failed.join("\n"));
}

#[test]
fn passive_client_welcome() {
    run("passive-client-welcome");
}

#[test]
fn passive_client_handling_commit() {
    run("passive-client-handling-commit");
}

#[test]
fn passive_client_random() {
    run("passive-client-random");
}
