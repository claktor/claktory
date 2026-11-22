//! Helpers shared by the tests: a store, a signer, members.

use claktory_dm_crypto::CIPHERSUITE;
use claktory_dm_crypto::provider::{Crypto, Provider};
use claktory_dm_crypto::storage::{SCHEMA, SealKey};
use ed25519_dalek::Signer as _;
use openmls::prelude::*;
use openmls_traits::signatures::{Signer, SignerError};
use openmls_traits::types::SignatureScheme;
use rusqlite::Connection;

/// An Ed25519 signer for tests; the real one is `identity`'s.
pub struct TestSigner(pub ed25519_dalek::SigningKey);

impl Signer for TestSigner {
    fn sign(&self, payload: &[u8]) -> Result<Vec<u8>, SignerError> {
        Ok(self.0.sign(payload).to_bytes().to_vec())
    }

    fn signature_scheme(&self) -> SignatureScheme {
        SignatureScheme::ED25519
    }
}

/// One endpoint: its store, seal key, cryptography and leaf.
pub struct Member {
    pub db: Connection,
    pub key: SealKey,
    pub crypto: Crypto,
    pub signer: TestSigner,
    pub credential: CredentialWithKey,
}

pub fn store() -> Connection {
    let db = Connection::open_in_memory().unwrap();
    db.pragma_update(None, "secure_delete", "ON").unwrap();
    db.execute_batch(SCHEMA).unwrap();
    db
}

impl Member {
    pub fn new(name: &str, seed: u8) -> Member {
        let signer = TestSigner(ed25519_dalek::SigningKey::from_bytes(&[seed; 32]));
        let credential = CredentialWithKey {
            credential: BasicCredential::new(name.as_bytes().to_vec()).into(),
            signature_key: signer.0.verifying_key().to_bytes().to_vec().into(),
        };
        Member { db: store(), key: SealKey::from_bytes([seed; 32]), crypto: Crypto::new().unwrap(), signer, credential }
    }

    /// One whose store is a file, as an endpoint's is: a rollback journal
    /// and secure_delete.
    pub fn on_disk(name: &str, seed: u8, path: &std::path::Path) -> Member {
        let db = Connection::open(path).unwrap();
        db.pragma_update(None, "journal_mode", "DELETE").unwrap();
        db.pragma_update(None, "secure_delete", "ON").unwrap();
        db.execute_batch(SCHEMA).unwrap();
        Member { db, ..Member::new(name, seed) }
    }

    pub fn provider(&self) -> Provider<'_> {
        Provider::new(&self.crypto, &self.db, &self.key).unwrap()
    }

    pub fn key_package(&self) -> KeyPackage {
        KeyPackage::builder().build(CIPHERSUITE, &self.provider(), &self.signer, self.credential.clone()).unwrap().key_package().clone()
    }

    pub fn create_group(&self) -> MlsGroup {
        MlsGroup::builder()
            .ciphersuite(CIPHERSUITE)
            // Commits are PublicMessages, for the worker's PublicGroup (§4).
            .with_wire_format_policy(PURE_PLAINTEXT_WIRE_FORMAT_POLICY)
            .use_ratchet_tree_extension(true)
            .build(&self.provider(), &self.signer, self.credential.clone())
            .unwrap()
    }

    pub fn join(&self, welcome: MlsMessageOut) -> MlsGroup {
        let welcome = match MlsMessageIn::from(welcome).extract() {
            MlsMessageBodyIn::Welcome(welcome) => welcome,
            _ => panic!("not a welcome"),
        };
        let config =
            MlsGroupJoinConfig::builder().wire_format_policy(PURE_PLAINTEXT_WIRE_FORMAT_POLICY).use_ratchet_tree_extension(true).build();
        StagedWelcome::new_from_welcome(&self.provider(), &config, welcome, None).unwrap().into_group(&self.provider()).unwrap()
    }

    pub fn values(&self) -> i64 {
        self.db.query_row("SELECT count(*) FROM mls_values", [], |row| row.get(0)).unwrap()
    }
}

/// What travels: serialized and parsed again, as the worker relays it.
pub fn wire(message: &MlsMessageOut) -> ProtocolMessage {
    use tls_codec::{Deserialize as _, Serialize as _};
    let bytes = message.tls_serialize_detached().unwrap();
    MlsMessageIn::tls_deserialize_exact(bytes).unwrap().try_into_protocol_message().unwrap()
}
