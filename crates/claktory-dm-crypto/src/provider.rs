//! Mista's (dms-e2ee.md §3): the OpenMLS provider, libcrux's cryptography
//! and randomness with [`SqlStorage`] over the caller's transaction.
//!
//! Make one [`Crypto`] per process and keep it; make a [`Provider`] per
//! transaction:
//!
//! ```ignore
//! let tx = db.transaction()?;
//! let provider = Provider::new(&crypto, &tx, &seal_key)?;
//! let mut group = MlsGroup::load(provider.storage(), &group_id)?;
//! // … process a message, write its history row with `tx` …
//! tx.commit()?;
//! ```

use openmls_traits::OpenMlsProvider;
use rusqlite::Connection;

use crate::storage::{SealKey, SqlStorage};
use crate::{Error, Result};

/// libcrux's cryptography and randomness (an HMAC-DRBG seeded from the
/// operating system), one per process.
///
/// Never fork without exec while one is alive: the child would copy the
/// DRBG's state and repeat its randomness. factoryd's commands fork and exec
/// at once (Fugo, E1).
pub struct Crypto(openmls_libcrux_crypto::CryptoProvider);

impl Crypto {
    pub fn new() -> Result<Crypto> {
        openmls_libcrux_crypto::CryptoProvider::new().map(Crypto).map_err(|error| Error::Mls(format!("no cryptography: {error:?}")))
    }

    /// OpenMLS's cryptography itself, for checks that need no storage
    /// (validating a KeyPackage, say).
    pub fn backend(&self) -> &openmls_libcrux_crypto::CryptoProvider {
        &self.0
    }
}

/// OpenMLS's provider for one transaction.
pub struct Provider<'a> {
    crypto: &'a openmls_libcrux_crypto::CryptoProvider,
    storage: SqlStorage<'a>,
}

impl<'a> Provider<'a> {
    /// An endpoint's: see [`SqlStorage::new`] for what `db` needs.
    pub fn new(crypto: &'a Crypto, db: &'a Connection, key: &'a SealKey) -> Result<Provider<'a>> {
        Ok(Provider { crypto: &crypto.0, storage: SqlStorage::new(db, key)? })
    }

    /// The worker's, for `PublicGroup`s only: see [`SqlStorage::public`].
    pub fn public(crypto: &'a Crypto, db: &'a Connection, key: &'a SealKey) -> Result<Provider<'a>> {
        Ok(Provider { crypto: &crypto.0, storage: SqlStorage::public(db, key)? })
    }

    /// Deletes this endpoint's key packages whose lifetime ended by `now`
    /// (seconds since the epoch): see [`crate::storage::sweep_key_packages`].
    pub fn sweep_key_packages(&self, now: u64) -> Result<usize> {
        self.storage.sweep_key_packages(now)
    }
}

impl<'a> OpenMlsProvider for Provider<'a> {
    type CryptoProvider = openmls_libcrux_crypto::CryptoProvider;
    type RandProvider = openmls_libcrux_crypto::CryptoProvider;
    type StorageProvider = SqlStorage<'a>;

    fn storage(&self) -> &Self::StorageProvider {
        &self.storage
    }

    fn crypto(&self) -> &Self::CryptoProvider {
        self.crypto
    }

    fn rand(&self) -> &Self::RandProvider {
        self.crypto
    }
}
