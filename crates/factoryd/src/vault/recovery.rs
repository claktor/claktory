//! Making sure the person saved the recovery key (R1, damage reduction §7):
//! they type back one group of it, picked at random, and the vault keeps
//! that the key was confirmed, bound to that key, so a new one asks again.
//!
//! What's kept is not secret: which recovery key was confirmed (a hash of
//! its protector, which changes with every new key), sealed in `kept` as
//! `recovery-confirmed`. A check lives only in memory, belongs to the
//! device that started it and to the key it asks about, and runs out; wrong
//! groups are counted across devices. Nothing typed is logged.

use factory_protocol::v1;
use rusqlite::{Connection, OptionalExtension};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use zeroize::Zeroizing;

use super::{Inner, Vault, VaultError, aad, account_recovery, crypto};
use crate::encoding::same;
use crate::transcript::now_ms;

/// How long a check lasts.
const CHECK_MS: i64 = 10 * 60_000;
/// Wrong groups allowed in an hour, from any device, before everything is
/// refused until the oldest is an hour old.
const WRONG_PER_HOUR: usize = 5;
const HOUR_MS: i64 = 3600 * 1000;
/// Groups in a recovery key ("K7QM-2XPA-…").
const GROUPS: u32 = 8;
/// What `kept` names the confirmation.
const CONFIRMED: &str = "recovery-confirmed";
/// What `kept` names the recovery key last typed to open the vault: its
/// protector's hash.
const USED: &str = "recovery-used";

/// The checks running now, by device, and when wrong groups were typed.
#[derive(Default)]
pub(super) struct Checks {
    by_device: HashMap<String, Check>,
    wrong: Vec<i64>,
}

struct Check {
    id: String,
    /// 0-based.
    group: usize,
    generation: String,
    until: i64,
}

/// Which recovery key of its own the vault holds now: a hash of its
/// protector, which a new key replaces. None if it has none.
fn generation(db: &Connection) -> Result<Option<String>, VaultError> {
    let sealed: Option<Vec<u8>> =
        db.query_row("SELECT sealed_vmk FROM protectors WHERE kind = 'recovery'", [], |row| row.get(0)).optional()?;
    Ok(sealed.map(|sealed| crypto::hex(&Sha256::digest(&sealed)[..16])))
}

/// A group as typed or kept, read the way a typed key is: no dashes or
/// spaces, upper case, 0, 1 and 8 as O, I and B.
fn normal(text: &str) -> Zeroizing<String> {
    Zeroizing::new(
        text.chars()
            .filter(|c| !matches!(c, '-' | ' '))
            .map(|c| match c.to_ascii_uppercase() {
                '0' => 'O',
                '1' => 'I',
                '8' => 'B',
                c => c,
            })
            .collect(),
    )
}

impl Vault {
    /// The recovery key kept to show again, if there is one.
    pub(super) fn kept_recovery(inner: &Inner) -> Result<Option<Zeroizing<String>>, VaultError> {
        let vmk = Self::open_vmk(inner)?;
        let db = Self::db(inner)?;
        let sealed: Option<Vec<u8>> = db.query_row("SELECT sealed FROM kept WHERE name = 'recovery'", [], |row| row.get(0)).optional()?;
        let Some(sealed) = sealed else { return Ok(None) };
        let opened = crypto::open(vmk, &sealed, &aad(&["kept", &inner.vault_id, "recovery"]))
            .ok_or_else(|| VaultError::Invalid("The kept recovery key doesn't open; make a new one.".to_owned()))?;
        Ok(Some(Zeroizing::new(String::from_utf8_lossy(&opened).into_owned())))
    }

    /// The vault is sealed to the account's recovery key: covered by
    /// it, rather than by a key of its own.
    pub(super) fn account_covered(db: &Connection) -> Result<bool, VaultError> {
        Ok(account_recovery::generation(db)?.is_some())
    }

    /// Whether this Claktory's own recovery key was confirmed (the one it
    /// holds now).
    fn own_confirmed(inner: &Inner, vmk: &[u8; 32]) -> Result<bool, VaultError> {
        let db = Self::db(inner)?;
        let Some(generation) = generation(db)? else { return Ok(false) };
        let sealed: Option<Vec<u8>> = db.query_row("SELECT sealed FROM kept WHERE name = ?1", [CONFIRMED], |row| row.get(0)).optional()?;
        let confirmed = sealed.and_then(|sealed| crypto::open(vmk, &sealed, &aad(&["kept", &inner.vault_id, CONFIRMED])));
        Ok(confirmed.is_some_and(|confirmed| confirmed.as_slice() == generation.as_bytes()))
    }

    /// Whether this Claktory's own recovery key, kept, is shown: always
    /// without the account's key; with it, only one the person confirmed
    /// before (one they saved).
    pub(super) fn own_showable(inner: &Inner) -> Result<bool, VaultError> {
        let vmk = Self::open_vmk(inner)?;
        let db = Self::db(inner)?;
        if !account_recovery::kept(db, "recovery")? {
            return Ok(false);
        }
        Ok(!Self::account_covered(db)? || Self::own_confirmed(inner, vmk)?)
    }

    /// The recovery key the person is asked to save and confirm, while it's
    /// shown: its generation and text.
    fn current(inner: &Inner) -> Result<Option<(String, Zeroizing<String>)>, VaultError> {
        if !Self::own_showable(inner)? {
            return Ok(None);
        }
        let (Some(text), Some(generation)) = (Self::kept_recovery(inner)?, generation(Self::db(inner)?)?) else { return Ok(None) };
        Ok(Some((generation, text)))
    }

    /// Whether the vault, open, holds a recovery key of its own the person
    /// hasn't confirmed (VaultState.recovery_key_unconfirmed); covered by the
    /// account's, nothing is asked. False while locked.
    pub(super) fn recovery_unconfirmed(inner: &Inner) -> Result<bool, VaultError> {
        let Some(vmk) = inner.vmk.as_deref() else { return Ok(false) };
        let db = Self::db(inner)?;
        if Self::account_covered(db)? {
            return Ok(false);
        }
        let kept: bool = db.query_row("SELECT COUNT(*) > 0 FROM kept WHERE name = 'recovery'", [], |row| row.get(0))?;
        if !kept || generation(db)?.is_none() {
            return Ok(false);
        }
        Ok(!Self::own_confirmed(inner, vmk)?)
    }

    /// Whether ShowRecoveryKey shows a key (VaultState.recovery_key_showable):
    /// anything but a vault covered by the account's key with nothing of its
    /// own to show. False while locked.
    pub(super) fn recovery_showable(inner: &Inner) -> Result<bool, VaultError> {
        if inner.vmk.is_none() {
            return Ok(false);
        }
        Ok(Self::own_showable(inner)? || !Self::account_covered(Self::db(inner)?)?)
    }

    /// The recovery key that opened the vault was typed here
    /// (VaultState.recovery_key_used): `generation` is its protector's hash,
    /// from `check_which`.
    pub(crate) fn note_recovery_used(&self, generation: &str) -> Result<(), VaultError> {
        let mut inner = self.lock();
        let vmk = Self::open_vmk(&inner)?;
        Self::keep(Self::db(&inner)?, &inner.vault_id, vmk, USED, generation.as_bytes())?;
        inner.state = None;
        Ok(())
    }

    /// Whether the recovery key that covers the vault now (the account's, or
    /// else its own) was typed here to open it since it was made.
    pub(super) fn recovery_used(inner: &Inner) -> Result<bool, VaultError> {
        let Some(vmk) = inner.vmk.as_deref() else { return Ok(false) };
        let db = Self::db(inner)?;
        let now = match account_recovery::generation(db)? {
            Some(account) => Some(account),
            None => generation(db)?,
        };
        let Some(now) = now else { return Ok(false) };
        let sealed: Option<Vec<u8>> = db.query_row("SELECT sealed FROM kept WHERE name = ?1", [USED], |row| row.get(0)).optional()?;
        let used = sealed.and_then(|sealed| crypto::open(vmk, &sealed, &aad(&["kept", &inner.vault_id, USED])));
        Ok(used.is_some_and(|used| used.as_slice() == now.as_bytes()))
    }

    /// Refused while too many wrong groups were typed in the last hour.
    fn tries_left(checks: &mut Checks, now: i64) -> Result<(), VaultError> {
        checks.wrong.retain(|at| now - at < HOUR_MS);
        if checks.wrong.len() < WRONG_PER_HOUR {
            return Ok(());
        }
        let wait = checks.wrong.first().map_or(HOUR_MS, |oldest| oldest + HOUR_MS - now);
        let minutes = (wait + 59_999) / 60_000;
        Err(VaultError::TooManyTries(format!(
            "Too many tries. Wait {minutes} minute{} and try again.",
            if minutes == 1 { "" } else { "s" }
        )))
    }

    /// Starts checking the recovery key on `device`: a group picked at
    /// random, for this device and this key only. Replaces the device's last
    /// check.
    pub(crate) fn check_recovery_key(&self, device: &str) -> Result<v1::RecoveryKeyCheck, VaultError> {
        let mut inner = self.lock();
        let now = now_ms();
        Self::tries_left(&mut inner.checks, now)?;
        let Some(current) = Self::current(&inner)? else {
            return Err(VaultError::Invalid("There's no recovery key yet: show it first.".to_owned()));
        };
        // 8 groups: a byte's remainder is as likely for each.
        let group = usize::from(crypto::random::<1>()[0]) % GROUPS as usize;
        let id = crypto::hex(&crypto::random::<16>());
        inner.checks.by_device.insert(device.to_owned(), Check { id: id.clone(), group, generation: current.0, until: now + CHECK_MS });
        Ok(v1::RecoveryKeyCheck { check_id: id, group: group as u32 + 1, groups: GROUPS })
    }

    /// The group a check asked for, as typed on `device`: right, the current
    /// recovery key is kept as confirmed and every device's check ends.
    pub(crate) fn confirm_recovery_key(&self, device: &str, check_id: &str, typed: &str) -> Result<(), VaultError> {
        let mut inner = self.lock();
        let now = now_ms();
        Self::tries_left(&mut inner.checks, now)?;
        let ran_out = || VaultError::Invalid("This check timed out. Start again.".to_owned());
        let current = Self::current(&inner)?;
        let (group, current) = match (inner.checks.by_device.get(device), current) {
            (Some(check), Some(current)) if check.id == check_id && check.until > now && current.0 == check.generation => {
                (check.group, current)
            }
            (Some(check), _) if check.id == check_id => {
                inner.checks.by_device.remove(device);
                return Err(ran_out());
            }
            _ => return Err(ran_out()),
        };
        let expected = current.1.split('-').nth(group).map(normal).ok_or_else(ran_out)?;
        if !same(&normal(typed), &expected) {
            inner.checks.wrong.push(now);
            return Err(VaultError::WrongGroup(group as u32 + 1));
        }
        let vmk = Self::open_vmk(&inner)?;
        Self::keep(Self::db(&inner)?, &inner.vault_id, vmk, CONFIRMED, current.0.as_bytes())?;
        inner.checks.by_device.clear();
        inner.state = None;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vault() -> Vault {
        let dir = std::env::temp_dir().join(format!("vault-recovery-{}", crypto::hex(&crypto::random::<8>())));
        Vault::open_or_make(dir.join("vault"))
    }

    fn unconfirmed(vault: &Vault) -> bool {
        Vault::recovery_unconfirmed(&vault.lock()).expect("read")
    }

    fn group(key: &str, check: &v1::RecoveryKeyCheck) -> String {
        key.split('-').nth(check.group as usize - 1).expect("group").to_owned()
    }

    #[test]
    fn a_recovery_key_asks_until_a_group_of_it_is_typed_back_and_again_when_replaced() {
        let vault = vault();
        assert!(!unconfirmed(&vault), "no recovery key, nothing to confirm");
        assert!(vault.check_recovery_key("mac").is_err(), "nothing to check");
        let key = vault.recovery_key("mac").expect("made");
        assert!(unconfirmed(&vault));
        let check = vault.check_recovery_key("mac").expect("check");
        assert!((1..=8).contains(&check.group) && check.groups == 8, "{check:?}");
        // Typed however: lower case, spaced, 0 for O.
        let typed = format!(" {} ", group(&key, &check).to_lowercase().replace('o', "0"));
        vault.confirm_recovery_key("mac", &check.check_id, &typed).expect("confirmed");
        assert!(!unconfirmed(&vault));
        assert!(vault.confirm_recovery_key("mac", &check.check_id, &typed).is_err(), "a check is used once");
        // A new key asks again, and an answer about the old one fails.
        let check = vault.check_recovery_key("mac").expect("check");
        let new = vault.new_recovery_key().expect("new key");
        assert!(unconfirmed(&vault));
        let stale = vault.confirm_recovery_key("mac", &check.check_id, &group(&key, &check));
        assert!(matches!(stale, Err(VaultError::Invalid(_))), "{stale:?}");
        let stale = vault.confirm_recovery_key("mac", &check.check_id, &group(&new, &check));
        assert!(matches!(stale, Err(VaultError::Invalid(_))), "the check went with the old key: {stale:?}");
        let check = vault.check_recovery_key("mac").expect("check");
        vault.confirm_recovery_key("mac", &check.check_id, &group(&new, &check)).expect("confirmed");
        assert!(!unconfirmed(&vault));
    }

    #[test]
    fn a_check_belongs_to_its_device_runs_out_and_wrong_groups_are_limited() {
        let vault = vault();
        let key = vault.recovery_key("mac").expect("made");
        let check = vault.check_recovery_key("mac").expect("check");
        let right = group(&key, &check);
        let other = vault.confirm_recovery_key("phone", &check.check_id, &right);
        assert!(matches!(other, Err(VaultError::Invalid(_))), "another device's check: {other:?}");
        // A newer check on the device replaces it.
        let newer = vault.check_recovery_key("mac").expect("check");
        assert!(vault.confirm_recovery_key("mac", &check.check_id, &right).is_err(), "replaced");
        // Ran out.
        vault.lock().checks.by_device.get_mut("mac").expect("check").until = now_ms() - 1;
        let late = vault.confirm_recovery_key("mac", &newer.check_id, &group(&key, &newer));
        assert!(matches!(late, Err(VaultError::Invalid(_))), "{late:?}");
        assert!(unconfirmed(&vault));
        // Wrong groups: the check stays to try again, five in an hour from
        // any device, then everything waits, the right group too.
        let check = vault.check_recovery_key("mac").expect("check");
        let wrong = if group(&key, &check) == "AAAA" { "BBBB" } else { "AAAA" };
        for _ in 0..3 {
            let refused = vault.confirm_recovery_key("mac", &check.check_id, wrong);
            assert!(matches!(refused, Err(VaultError::WrongGroup(n)) if n == check.group), "{refused:?}");
        }
        let phone = vault.check_recovery_key("phone").expect("check");
        for _ in 0..2 {
            assert!(vault.confirm_recovery_key("phone", &phone.check_id, "").is_err());
        }
        let limited = vault.confirm_recovery_key("mac", &check.check_id, &group(&key, &check));
        assert!(matches!(&limited, Err(VaultError::TooManyTries(said)) if said.contains("60 minutes")), "{limited:?}");
        assert!(matches!(vault.check_recovery_key("mac"), Err(VaultError::TooManyTries(_))));
        assert!(unconfirmed(&vault));
        // An hour on, it works again.
        for at in &mut vault.lock().checks.wrong {
            *at -= HOUR_MS;
        }
        vault.confirm_recovery_key("mac", &check.check_id, &group(&key, &check)).expect("confirmed");
        assert!(!unconfirmed(&vault));
    }

    #[test]
    fn what_was_typed_shows_nowhere_in_what_is_said() {
        let vault = vault();
        vault.recovery_key("mac").expect("made");
        let check = vault.check_recovery_key("mac").expect("check");
        let refused = vault.confirm_recovery_key("mac", &check.check_id, "ZQZQ").expect_err("wrong");
        let said = refused.to_string();
        assert!(!said.contains("ZQZQ") && !format!("{refused:?}").contains("ZQZQ"), "{said}");
        assert!(said.contains(&format!("block {}", check.group)), "{said}");
        // A locked vault asks nothing.
        vault.lock().vmk = None;
        assert!(!unconfirmed(&vault));
        assert!(matches!(vault.check_recovery_key("mac"), Err(VaultError::Locked)));
    }
}
