//! The key that seals this Claktory's direct-message leaf store
//! (`user_apps::dms::store`, dms-e2ee.md §5.2): derived from the vault's key
//! and the store's own random id, so it needs nothing kept in the vault, a
//! vault backup carries nothing of it, and it's there only while the vault
//! is open. A vault made again (a new key) can't open an old store, which is
//! then started over: the leaf enrolls again, as after a move.

use super::{Vault, VaultError, crypto};

impl Vault {
    /// The store's key for `purpose`, for the store whose id is `store_id`.
    pub(crate) fn dm_store_key(&self, store_id: &[u8], purpose: &str) -> Result<crypto::Key, VaultError> {
        let inner = self.lock();
        let vmk = Self::open_vmk(&inner)?;
        Ok(crypto::derive(vmk, store_id, &format!("claktory dm store v1 {purpose}")))
    }
}
