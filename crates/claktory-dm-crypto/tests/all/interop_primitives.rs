//! RFC 9420's crypto-basics and secret-tree vectors (suite 0x0001), through
//! libcrux as OpenMLS calls it: the derivations application messages use,
//! which the passive-client vectors don't reach.

use claktory_dm_crypto::CIPHERSUITE;
use openmls_traits::crypto::OpenMlsCrypto;
use openmls_traits::types::{HpkeCiphertext, HpkeConfig};

use crate::interop::{hex, vectors};

fn config() -> HpkeConfig {
    CIPHERSUITE.hpke_config()
}

fn crypto() -> openmls_libcrux_crypto::CryptoProvider {
    openmls_libcrux_crypto::CryptoProvider::new().unwrap()
}

/// An MLS vector: a QUIC varint length, then the bytes.
fn vl(bytes: &[u8]) -> Vec<u8> {
    let n = bytes.len();
    let mut out = match n {
        0..64 => vec![n as u8],
        64..16384 => vec![0x40 | (n >> 8) as u8, n as u8],
        _ => vec![0x80 | (n >> 24) as u8, (n >> 16) as u8, (n >> 8) as u8, n as u8],
    };
    out.extend_from_slice(bytes);
    out
}

fn mls(label: &str) -> Vec<u8> {
    [b"MLS 1.0 ".as_slice(), label.as_bytes()].concat()
}

fn expand_with_label(crypto: &impl OpenMlsCrypto, secret: &[u8], label: &str, context: &[u8], length: usize) -> Vec<u8> {
    let info = [(length as u16).to_be_bytes().as_slice(), &vl(&mls(label)), &vl(context)].concat();
    crypto.hkdf_expand(CIPHERSUITE.hash_algorithm(), secret, &info, length).unwrap().as_slice().to_vec()
}

fn derive_tree_secret(crypto: &impl OpenMlsCrypto, secret: &[u8], label: &str, generation: u32, length: usize) -> Vec<u8> {
    expand_with_label(crypto, secret, label, &generation.to_be_bytes(), length)
}

fn field(test: &serde_json::Value, outer: &str, inner: &str) -> Vec<u8> {
    hex(test[outer][inner].as_str().unwrap())
}

fn text<'a>(test: &'a serde_json::Value, outer: &str, inner: &str) -> &'a str {
    test[outer][inner].as_str().unwrap()
}

#[test]
fn crypto_basics() {
    let crypto = crypto();
    let hash = CIPHERSUITE.hash_algorithm();
    let tests = vectors("crypto-basics");
    assert_eq!(tests.len(), 1);
    let test = &tests[0];

    let input = [vl(text(test, "ref_hash", "label").as_bytes()), vl(&field(test, "ref_hash", "value"))].concat();
    assert_eq!(crypto.hash(hash, &input).unwrap(), field(test, "ref_hash", "out"), "RefHash");

    let out = expand_with_label(
        &crypto,
        &field(test, "expand_with_label", "secret"),
        text(test, "expand_with_label", "label"),
        &field(test, "expand_with_label", "context"),
        test["expand_with_label"]["length"].as_u64().unwrap() as usize,
    );
    assert_eq!(out, field(test, "expand_with_label", "out"), "ExpandWithLabel");

    let out = expand_with_label(&crypto, &field(test, "derive_secret", "secret"), text(test, "derive_secret", "label"), b"", hash.size());
    assert_eq!(out, field(test, "derive_secret", "out"), "DeriveSecret");

    let tree = &test["derive_tree_secret"];
    let out = derive_tree_secret(
        &crypto,
        &field(test, "derive_tree_secret", "secret"),
        text(test, "derive_tree_secret", "label"),
        tree["generation"].as_u64().unwrap() as u32,
        tree["length"].as_u64().unwrap() as usize,
    );
    assert_eq!(out, field(test, "derive_tree_secret", "out"), "DeriveTreeSecret");

    let scheme = CIPHERSUITE.signature_algorithm();
    let content = [vl(&mls(text(test, "sign_with_label", "label"))), vl(&field(test, "sign_with_label", "content"))].concat();
    let signature = field(test, "sign_with_label", "signature");
    crypto.verify_signature(scheme, &content, &field(test, "sign_with_label", "pub"), &signature).expect("VerifyWithLabel");
    let signed = crypto.sign(scheme, &content, &field(test, "sign_with_label", "priv")).unwrap();
    assert_eq!(signed, signature, "SignWithLabel (Ed25519 is deterministic)");

    let info = [vl(&mls(text(test, "encrypt_with_label", "label"))), vl(&field(test, "encrypt_with_label", "context"))].concat();
    let sealed = HpkeCiphertext {
        kem_output: field(test, "encrypt_with_label", "kem_output").into(),
        ciphertext: field(test, "encrypt_with_label", "ciphertext").into(),
    };
    let private = field(test, "encrypt_with_label", "priv");
    let opened = crypto.hpke_open(config(), &sealed, &private, &info, b"").unwrap();
    assert_eq!(opened, field(test, "encrypt_with_label", "plaintext"), "DecryptWithLabel");
    // What it seals now opens with the same key.
    let fresh = crypto.hpke_seal(config(), &field(test, "encrypt_with_label", "pub"), &info, b"", b"again").unwrap();
    assert_eq!(crypto.hpke_open(config(), &fresh, &private, &info, b"").unwrap(), b"again");
}

#[test]
fn secret_tree() {
    let crypto = crypto();
    let hash = CIPHERSUITE.hash_algorithm().size();
    let (key_size, nonce_size) = (CIPHERSUITE.aead_key_length(), CIPHERSUITE.aead_nonce_length());
    let tests = vectors("secret-tree");
    assert!(!tests.is_empty());
    for test in &tests {
        let secret = field(test, "sender_data", "sender_data_secret");
        let ciphertext = field(test, "sender_data", "ciphertext");
        let sample = &ciphertext[..hash.min(ciphertext.len())];
        assert_eq!(expand_with_label(&crypto, &secret, "key", sample, key_size), field(test, "sender_data", "key"));
        assert_eq!(expand_with_label(&crypto, &secret, "nonce", sample, nonce_size), field(test, "sender_data", "nonce"));

        let leaves = test["leaves"].as_array().unwrap();
        let root = leaves.len().next_power_of_two() - 1;
        let encryption_secret = hex(test["encryption_secret"].as_str().unwrap());
        for (leaf, generations) in leaves.iter().enumerate() {
            // Down from the root to the leaf's node, 2 × leaf.
            let (mut node, mut secret) = (root, encryption_secret.clone());
            while node != 2 * leaf {
                let level = (!node).trailing_zeros();
                let (left, right) = (node ^ (1 << (level - 1)), node ^ (3 << (level - 1)));
                let (next, side) = if 2 * leaf < node { (left, "left") } else { (right, "right") };
                secret = expand_with_label(&crypto, &secret, "tree", side.as_bytes(), hash);
                node = next;
            }
            for kind in ["application", "handshake"] {
                let mut ratchet = expand_with_label(&crypto, &secret, kind, b"", hash);
                let mut at = 0u32;
                for expected in generations.as_array().unwrap() {
                    let generation = expected["generation"].as_u64().unwrap() as u32;
                    while at < generation {
                        ratchet = derive_tree_secret(&crypto, &ratchet, "secret", at, hash);
                        at += 1;
                    }
                    let key = derive_tree_secret(&crypto, &ratchet, "key", at, key_size);
                    let nonce = derive_tree_secret(&crypto, &ratchet, "nonce", at, nonce_size);
                    assert_eq!(key, hex(expected[format!("{kind}_key")].as_str().unwrap()), "leaf {leaf} {kind} key at {generation}");
                    assert_eq!(nonce, hex(expected[format!("{kind}_nonce")].as_str().unwrap()), "leaf {leaf} {kind} nonce at {generation}");
                }
            }
        }
    }
}
