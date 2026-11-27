//! The browser gate's key: an ed25519 key whose public half setup puts in
//! a file of root's, so the gate runs the browser's runner only for
//! factoryd, which signs its challenge (`browser_runner::gate`). Its seed
//! is kept sealed in `kept`, never a secret: it can't be listed, granted,
//! revealed or put in a command.

use rusqlite::OptionalExtension;
use zeroize::Zeroizing;

use super::{Vault, VaultError, aad, crypto};

/// What `kept` names it.
const NAME: &str = "browser-runner-key";

impl Vault {
    /// The runner's signing key, made the first time it's asked for. Err
    /// while the vault is locked: the browser waits for it, as fills do.
    pub(crate) fn browser_runner_key(&self) -> Result<ed25519_dalek::SigningKey, VaultError> {
        let seed = self.with_unlocked_transaction(|db, vault_id, vmk| {
            let sealed: Option<Vec<u8>> = db.query_row("SELECT sealed FROM kept WHERE name = ?1", [NAME], |row| row.get(0)).optional()?;
            let kept = sealed
                .and_then(|sealed| crypto::open(vmk, &sealed, &aad(&["kept", vault_id, NAME])))
                .and_then(|seed| <[u8; 32]>::try_from(seed.as_slice()).ok());
            if let Some(seed) = kept {
                return Ok(Zeroizing::new(seed));
            }
            let seed = Zeroizing::new(crypto::random::<32>());
            Self::keep(db, vault_id, vmk, NAME, seed.as_slice())?;
            Ok(seed)
        })?;
        Ok(ed25519_dalek::SigningKey::from_bytes(&seed))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_runners_key_is_made_once_and_kept_sealed() {
        let dir = std::env::temp_dir().join(format!("vault-runner-key-{}", crypto::hex(&crypto::random::<8>())));
        let vault = Vault::open_or_make(dir.join("vault"));
        let first = vault.browser_runner_key().expect("made");
        assert_eq!(vault.browser_runner_key().expect("kept").to_bytes(), first.to_bytes());
        let stored = std::fs::read(dir.join("vault/vault.db")).expect("db");
        assert!(!stored.windows(32).any(|window| window == first.to_bytes()), "never stored in the clear");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
