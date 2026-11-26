//! Recovery codes a site showed once, held for the person's card (#435
//! W8b, Fugo 42966): sealed with the vault's key in `kept`, as
//! `recovery-codes:<card>`, bound to the vault and the card, the moment
//! Claktory reads them off the page. Never a secret: no agent can resolve,
//! be granted or list them, and nothing else reads them but the card's Show
//! and Keep in Secrets. They outlast restarts; only the person's I saved
//! them or Keep in Secrets removes them (`drop_codes`).

use rusqlite::{OptionalExtension, params};
use zeroize::Zeroizing;

use super::{Value, Vault, VaultError, aad, crypto};

/// The variable a kept card's codes are in. It shows no hint: four
/// characters of a code would reach every device (Abbacchio 45722).
pub(crate) const CODES_VARIABLE: &str = "RECOVERY_CODES";

/// What `kept` names a card's codes.
fn name(card: &str) -> String {
    format!("recovery-codes:{card}")
}

impl Vault {
    /// Seals `codes` for `card` (an ask's id), replacing any it held.
    pub(crate) fn hold_codes(&self, card: &str, codes: &[String]) -> Result<(), VaultError> {
        let name = name(card);
        let plain = Zeroizing::new(codes.join("\n"));
        self.with_unlocked_transaction(|db, vault_id, vmk| {
            let sealed = crypto::seal(vmk, plain.as_bytes(), &aad(&["kept", vault_id, &name]));
            db.execute("INSERT OR REPLACE INTO kept (name, sealed) VALUES (?1, ?2)", params![name, sealed])?;
            Ok(())
        })
    }

    /// `card`'s codes, if it holds them: Ok(None) when it doesn't (or no
    /// longer does), locked or not; Err when it does but is locked.
    pub(crate) fn held_codes(&self, card: &str) -> Result<Option<Zeroizing<Vec<String>>>, VaultError> {
        let name = name(card);
        let inner = self.lock();
        let sealed: Option<Vec<u8>> = Self::db(&inner)?.query_row("SELECT sealed FROM kept WHERE name = ?1", [&name], |row| row.get(0)).optional()?;
        let Some(sealed) = sealed else { return Ok(None) };
        let vmk = Self::open_vmk(&inner)?;
        let opened = crypto::open(vmk, &sealed, &aad(&["kept", &inner.vault_id, &name]))
            .ok_or_else(|| VaultError::Invalid("These recovery codes don't open: they're gone.".to_owned()))?;
        let text = Zeroizing::new(String::from_utf8_lossy(&opened).into_owned());
        Ok(Some(Zeroizing::new(text.lines().map(str::to_owned).collect())))
    }

    /// Forgets `card`'s codes. Whether it held them.
    pub(crate) fn drop_codes(&self, card: &str) -> Result<bool, VaultError> {
        let inner = self.lock();
        Ok(Self::db(&inner)?.execute("DELETE FROM kept WHERE name = ?1", [name(card)])? > 0)
    }

    /// The cards whose codes it holds.
    pub(crate) fn code_cards(&self) -> Result<Vec<String>, VaultError> {
        let inner = self.lock();
        let db = Self::db(&inner)?;
        let mut statement = db.prepare("SELECT name FROM kept WHERE name LIKE 'recovery-codes:%'")?;
        let names = statement.query_map([], |row| row.get::<_, String>(0))?.collect::<Result<Vec<_>, _>>()?;
        Ok(names.into_iter().filter_map(|name| name.strip_prefix("recovery-codes:").map(str::to_owned)).collect())
    }

    /// Keeps `codes` from `site` (a host; "A site" when it's unknown) as a
    /// new secret given to no agent: "<site> recovery codes" (a number after
    /// it when that's taken), its variable RECOVERY_CODES, one code per
    /// line, with `why` in its Recent use (kind "keep"). Its name.
    pub(crate) fn keep_codes(&self, site: &str, codes: &[String], why: &str) -> Result<String, VaultError> {
        let values = [Value {
            variable: CODES_VARIABLE.to_owned(),
            value: Zeroizing::new(codes.join("\n")),
            file: false,
            binary: false,
        }];
        // A secret's name is at most 64 characters: room for " recovery codes 99".
        let site: String = site.chars().filter(|c| c.is_alphanumeric() || matches!(c, '-' | '.' | ' ')).take(46).collect();
        let site = if site.trim().is_empty() { "A site".to_owned() } else { site.trim().to_owned() };
        let description = format!("Recovery codes for {site}, kept from an errand. Given to no agent.");
        let base = format!("{site} recovery codes");
        for n in 1..100 {
            let name = if n == 1 { base.clone() } else { format!("{base} {n}") };
            match self.create_secret(&name, &description, &values, "") {
                Ok(id) => {
                    let inner = self.lock();
                    if let Err(error) = Self::db(&inner).and_then(|db| Self::log(db, &id, &name, "keep", "", "", why)) {
                        tracing::error!(%error, "kept recovery codes weren't noted in their secret's recent use");
                    }
                    return Ok(name);
                }
                Err(VaultError::Invalid(why)) if why.contains("already") => continue,
                Err(error) => return Err(error),
            }
        }
        Err(VaultError::Invalid(format!("There are too many secrets named \"{base}\": rename some and try again.")))
    }
}

#[cfg(test)]
mod tests {
    use super::super::VaultError;
    use super::super::tests::{temp, vault};

    #[test]
    fn codes_are_held_sealed_until_dropped_and_kept_as_a_secret_no_agent_holds() {
        let dir = temp();
        let vault = vault(&dir);
        vault.create(Some("correct horse battery"), false).expect("create");
        let codes = vec!["a8f2-91kd".to_owned(), "c3m9-2wle".to_owned()];
        vault.hold_codes("41", &codes).expect("held");
        assert_eq!(vault.held_codes("41").expect("open").map(|held| held.to_vec()), Some(codes.clone()));
        assert!(vault.held_codes("42").expect("open").is_none());
        // Sealed: the file never has them as they are.
        let bytes = std::fs::read(dir.join("vault/vault.db")).unwrap_or_default();
        assert!(!bytes.windows(9).any(|window| window == b"a8f2-91kd"));
        let first = vault.keep_codes("shop.example", &codes, "kept").expect("kept");
        let second = vault.keep_codes("shop.example", &codes, "kept").expect("kept again");
        let unknown = vault.keep_codes("", &codes, "kept").expect("kept, site unknown");
        assert_eq!(
            (first.as_str(), second.as_str(), unknown.as_str()),
            ("shop.example recovery codes", "shop.example recovery codes 2", "A site recovery codes")
        );
        assert!(vault.held("matt", &first).expect("read").is_none(), "given to no one");
        let state = vault.state(|_, _| false);
        let kept = state.secrets.iter().find(|secret| secret.name == first).expect("listed");
        assert_eq!(kept.variables.iter().map(|variable| (variable.name.as_str(), variable.hint.as_str())).collect::<Vec<_>>(), [("RECOVERY_CODES", "")]);
        assert!(vault.drop_codes("41").expect("dropped"));
        assert!(vault.held_codes("41").expect("open").is_none());
        assert!(!vault.drop_codes("41").expect("dropped"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn locked_it_says_which_codes_it_holds_and_which_are_gone() {
        let dir = temp();
        let first = vault(&dir);
        first.create(Some("correct horse battery"), false).expect("create");
        first.hold_codes("41", &["a8f2-91kd".to_owned()]).expect("held");
        first.set_locked_mode(true).expect("locked mode");
        drop(first);
        let locked = vault(&dir);
        assert!(!locked.is_open());
        assert!(matches!(locked.held_codes("41"), Err(VaultError::Locked)), "held, but locked");
        assert!(matches!(locked.held_codes("42"), Ok(None)), "never held: gone, not locked");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
