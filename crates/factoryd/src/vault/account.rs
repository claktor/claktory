//! The vault's account protector (#116): once this Claktory signs in at
//! claktory.com, its vault's key is sealed under a key made from a share
//! kept on disk (L) and one the service releases only to this Claktory (S):
//! `KEK = HKDF(L ‖ S, salt = vault_id, info = "claktory vault account v1")`.
//! The key on disk (`auto.key`) goes, and a recovery key is made and kept,
//! sealed with the vault's key, to show the person, until the account's
//! own recovery key covers the vault (`account_recovery`, #239). After a
//! restart the account module fetches S and opens the vault with
//! `open_with_account`.

use rusqlite::{Connection, OptionalExtension, params};
use zeroize::Zeroizing;

use super::{Inner, Protector, Vault, VaultError, aad, crypto};

/// The protector kinds that are accounts start with this.
const PREFIX: &str = "account:";

/// The account protector's kind and params, if the vault has one.
pub(super) fn protector(inner: &Inner) -> Option<(String, String)> {
    inner
        .db
        .as_ref()?
        .query_row("SELECT kind, params FROM protectors WHERE kind LIKE 'account:%' LIMIT 1", [], crate::store::row2)
        .optional()
        .ok()?
}

fn kek(vault_id: &str, shares: &[u8]) -> super::Key {
    crypto::derive(shares, vault_id.as_bytes(), "claktory vault account v1")
}

impl Vault {
    /// Seals `value` with the vault's key, to show the person later.
    pub(super) fn keep(db: &Connection, vault_id: &str, vmk: &[u8; 32], name: &str, value: &[u8]) -> Result<(), VaultError> {
        let sealed = crypto::seal(vmk, value, &aad(&["kept", vault_id, name]));
        db.execute("INSERT OR REPLACE INTO kept (name, sealed) VALUES (?1, ?2)", params![name, sealed])?;
        Ok(())
    }

    /// Protects the open vault with the account `account_id`: `shares` is
    /// L ‖ S, `params` what the account module needs to fetch S again (kept
    /// as is). Replaces any account protector before it, makes a recovery
    /// key if none is kept, and removes the key on disk.
    pub(crate) fn protect_with_account(&self, account_id: &str, params: &str, shares: &[u8]) -> Result<(), VaultError> {
        let mut inner = self.lock();
        let vmk = Zeroizing::new(*Self::open_vmk(&inner)?);
        let vault_id = inner.vault_id.clone();
        let kind = format!("{PREFIX}{account_id}");
        let sealed_vmk = crypto::seal(&kek(&vault_id, shares), vmk.as_slice(), &aad(&["vmk", &vault_id, &kind]));
        let protector = Protector { kind: kind.clone(), params: params.to_owned(), salt: Vec::new(), sealed_vmk };
        let Some(db) = inner.db.as_mut() else { return Err(VaultError::NoVault) };
        let transaction = db.transaction()?;
        transaction.execute("DELETE FROM protectors WHERE kind LIKE 'account:%' AND kind != ?1", [&kind])?;
        Self::save_protector(&transaction, &protector)?;
        let kept: bool = transaction.query_row("SELECT COUNT(*) > 0 FROM kept WHERE name = 'recovery'", [], |row| row.get(0))?;
        if !kept {
            let (text, recovery) = Self::recovery_protector(&vault_id, &vmk);
            Self::save_protector(&transaction, &recovery)?;
            Self::keep(&transaction, &vault_id, &vmk, "recovery", text.as_bytes())?;
        }
        transaction.execute("DELETE FROM protectors WHERE kind = 'auto'", [])?;
        transaction.execute("UPDATE vault SET locked_mode = 0", [])?;
        transaction.commit()?;
        match std::fs::remove_file(self.auto_path()) {
            Err(error) if error.kind() != std::io::ErrorKind::NotFound => return Err(error.into()),
            _ => {}
        }
        inner.locked_mode = false;
        inner.state = None;
        Ok(())
    }

    /// After signing out: an open vault opens by itself again, with a key on
    /// disk, instead of with the account. A locked one keeps its account
    /// protector (its recovery key opens it). Whether it changed.
    pub(crate) fn drop_account(&self) -> Result<bool, VaultError> {
        let mut inner = self.lock();
        if inner.vmk.is_none() || protector(&inner).is_none() {
            return Ok(false);
        }
        let vmk = Zeroizing::new(*Self::open_vmk(&inner)?);
        let vault_id = inner.vault_id.clone();
        let auto = self.auto_protector(&vault_id, &vmk)?;
        let Some(db) = inner.db.as_mut() else { return Err(VaultError::NoVault) };
        let transaction = db.transaction()?;
        Self::save_protector(&transaction, &auto)?;
        transaction.execute("DELETE FROM protectors WHERE kind LIKE 'account:%'", [])?;
        transaction.commit()?;
        inner.state = None;
        Ok(true)
    }

    /// The account protector's params (as given to `protect_with_account`),
    /// if the vault has one.
    pub(crate) fn account_params(&self) -> Option<String> {
        protector(&self.lock()).map(|(_, params)| params)
    }

    /// Opens a locked vault with the account's shares, L ‖ S. Whether it's
    /// open now; false when they don't open it (another epoch's S, say).
    pub(crate) fn open_with_account(&self, shares: &[u8]) -> Result<bool, VaultError> {
        let mut inner = self.lock();
        if inner.vmk.is_some() {
            return Ok(true);
        }
        let db = Self::db(&inner)?;
        let found: Option<(String, Vec<u8>)> = db
            .query_row("SELECT kind, sealed_vmk FROM protectors WHERE kind LIKE 'account:%' LIMIT 1", [], crate::store::row2)
            .optional()?;
        let Some((kind, sealed)) = found else { return Ok(false) };
        let Some(vmk) = crypto::open_key(&kek(&inner.vault_id, shares), &sealed, &aad(&["vmk", &inner.vault_id, &kind])) else {
            return Ok(false);
        };
        Self::set_open(&mut inner, &vmk);
        Ok(inner.vmk.is_some())
    }

    /// The recovery key, to show the person (logged): this Claktory's own
    /// while it's shown (`own_showable`), made and kept if there's none and
    /// the account's key doesn't cover the vault (#239). Covered by it,
    /// there's nothing to show: no Claktory keeps the account's key.
    pub(crate) fn recovery_key(&self, device: &str) -> Result<String, VaultError> {
        let kept = {
            let inner = self.lock();
            Self::open_vmk(&inner)?;
            let text = if Self::own_showable(&inner)? {
                Self::kept_recovery(&inner)?
            } else if Self::account_covered(Self::db(&inner)?)? {
                return Err(VaultError::Invalid("This Claktory uses your account's recovery key. Claktory doesn't keep a copy, so it can't show it here.".to_owned()));
            } else {
                Self::kept_recovery(&inner)?
            };
            if text.is_some() {
                Self::log(Self::db(&inner)?, "", "", "recovery", "", "", device)?;
            }
            text.map(|text| text.to_string())
        };
        match kept {
            Some(text) => Ok(text),
            None => {
                let text = self.new_recovery_key()?;
                let inner = self.lock();
                Self::log(Self::db(&inner)?, "", "", "recovery", "", "", device)?;
                Ok(text)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_account_protects_the_vault_instead_of_the_key_on_disk() {
        let dir = std::env::temp_dir().join(format!("vault-account-{}", crypto::hex(&crypto::random::<8>())));
        let vault = Vault::open_or_make(dir.join("vault"));
        assert!(dir.join("vault/auto.key").exists());
        let id = vault
            .create_secret(
                "AWS",
                "",
                &[super::super::Value::from_entry(&factory_protocol::v1::SecretEntry {
                    variable: "KEY".into(),
                    value: "wJalrXUtnFEMI-account".into(),
                    file: false,
                })],
                "",
            )
            .expect("secret");
        let shares = [7u8; 64];
        vault.protect_with_account("acct1", r#"{"epoch":1}"#, &shares).expect("protect");
        assert!(!dir.join("vault/auto.key").exists(), "the key on disk goes");
        assert_eq!(vault.account_params().as_deref(), Some(r#"{"epoch":1}"#));
        let recovery = vault.recovery_key("Mac").expect("recovery");
        assert_eq!(vault.recovery_key("Mac").expect("again"), recovery, "the same one, kept");
        drop(vault);

        // After a restart it's locked until the account's shares open it.
        let vault = Vault::open_or_make(dir.join("vault"));
        assert!(vault.exists() && !vault.is_open());
        assert!(!vault.open_with_account(&[8u8; 64]).expect("wrong shares"));
        assert!(vault.open_with_account(&shares).expect("shares"));
        assert!(vault.reveal(&id, "Mac").is_ok());
        drop(vault);

        // The recovery key opens it too.
        let vault = Vault::open_or_make(dir.join("vault"));
        let (vault_id, protectors) = vault.protectors().expect("protectors");
        let vmk = Vault::check(&vault_id, &protectors, &recovery).expect("the recovery key opens it");
        vault.unlock(vmk);
        assert!(vault.is_open());
        let kinds: Vec<String> = vault.uses("", 0).expect("uses").into_iter().map(|used| used.kind).collect();
        assert_eq!(kinds, ["reveal", "recovery", "recovery"]);
    }
}
