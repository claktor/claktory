//! The account's recovery key (#239; coordination/security/
//! recovery-and-sign-in.md, part 1): one key covers every Claktory of the
//! account. claktory.com keeps only its public half, made and approved on
//! one of the person's devices, and gives it back at sign-in and at every
//! check-in; each Claktory seals its own vault to it (`vault::
//! account_recovery`).
//!
//! At sign-in and each check-in this Claktory compares claktory.com's key
//! with the one its vault is sealed to. The same: nothing to do. Another,
//! the account's first included: this version knows no device's account
//! key, so it never seals by itself. A key whose statements check (each
//! signed by its Claktory's backend key and approved by a device, back to
//! the key the vault is sealed to, or to the account's first) waits for the
//! person to choose Use the key here (UseRecoveryKey) on this computer's
//! app or a device paired here and confirmed as theirs; Claktory's own
//! alert says so. One that doesn't check gets an alert saying it couldn't
//! be checked, and nothing to choose. Each seal is a record in History.

use std::sync::Arc;

use claktory_account::protocol::{self as api, RecoveryKey, RecoveryKeyStatement};
use factory_protocol::v1::{self, ErrorCode};
use serde::{Deserialize, Serialize};

use super::{changed, refused};
use crate::config::Config;
use crate::factory::{Factory, RequestError};
use crate::transcript::now_ms;
use crate::vault::account_recovery;

/// The table of recovery key changes this Claktory sealed its vault to, for
/// History (NEEDS_YOU_RECORD_KIND_RECOVERY_KEY_CHANGED).
pub(crate) const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS recovery_key_changes (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    -- Claktory's sentence, written when it happened.
    title TEXT NOT NULL,
    -- When the check-in saw the new key, and when the vault was sealed to it.
    seen INTEGER NOT NULL,
    at INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS recovery_key_changes_at ON recovery_key_changes (at);
";

/// A key claktory.com gave for the account that isn't the one the vault is
/// sealed to and wasn't verified: it waits for the person's choice.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub(super) struct Waiting {
    pub account_id: String,
    pub public_key: String,
    /// The Claktory that set it, by its id at the service and its name as
    /// the account lists it (`made_name`, untrusted: `name`), and when the
    /// service took it (ms since 1970), by claktory.com's word.
    pub made_by: String,
    pub made_name: String,
    pub made_ms: i64,
    /// When this Claktory first saw it (ms since 1970).
    pub seen_ms: i64,
    /// Its statements check (`checked`): the person may choose to use it.
    pub checked: bool,
}

/// What VaultState says of the account's key.
#[derive(Default)]
pub(crate) struct View {
    pub covered: bool,
    pub made_on: String,
    pub made_ms: i64,
    pub waiting: bool,
    pub waiting_made_on: String,
    pub waiting_made_ms: i64,
}

/// A Claktory's name as claktory.com gives it (not signed, set by the
/// person or a Claktory): untrusted text, shown on one line with no control
/// or invisible characters, cut short (Fugo 49190); empty when that
/// Claktory is no longer the account's.
fn name(given: &str) -> String {
    crate::remote::shown_name(given)
}

/// The Claktory that made a key, to show: this one's own name when it's
/// this one, else its name as claktory.com gave it.
fn name_of(config: &Config, ours: &str, made_by: &str, given: &str) -> String {
    if !made_by.is_empty() && made_by == ours { config.claktory_name() } else { name(given) }
}

/// What the alert calls a Claktory with no name.
const NO_LONGER: &str = "a Claktory no longer on your account";

/// VaultState's account fields (#239). Takes the account's lock, so the
/// caller may hold Factory's.
fn view(factory: &Factory, config: &Config) -> View {
    if !config.account.enabled {
        return View::default();
    }
    let saved = factory.account.lock().saved.clone();
    if !saved.signed_in() {
        return View::default();
    }
    let sealed = factory.vault.account_key().filter(|sealed| sealed.account_id == saved.account_id);
    let mut view = View::default();
    let waiting = saved.recovery_waiting.as_ref().filter(|waiting| waiting.account_id == saved.account_id && waiting.checked);
    if let Some(waiting) = waiting {
        view.waiting = true;
        view.waiting_made_on = name_of(config, &saved.claktory_id, &waiting.made_by, &waiting.made_name);
        view.waiting_made_ms = waiting.made_ms;
    } else if sealed.is_some() && !saved.recovery_made_by.is_empty() && factory.vault.account_covered_now() {
        view.covered = true;
        view.made_on = name_of(config, &saved.claktory_id, &saved.recovery_made_by, &saved.recovery_made_name);
        view.made_ms = saved.recovery_made_ms;
    }
    view
}

/// A key the service gave, if it's one this can seal to.
fn usable(key: Option<RecoveryKey>) -> Result<Option<RecoveryKey>, ()> {
    match key {
        Some(key) if account_recovery::public_of(&key.public_key).is_none() => Err(()),
        key => Ok(key),
    }
}

/// One key's statement, if it checks: signed by the backend key given for
/// its Claktory, for this account, saying this key, and approved by the
/// device it names.
fn statement_of(account_id: &str, key: &RecoveryKey) -> Option<RecoveryKeyStatement> {
    let backend_key = api::verifying_key(&key.backend_key)?;
    let statement = api::verify_recovery_key(&backend_key, &key.statement)?;
    let said = statement.account_id == account_id && statement.public_key == key.public_key && statement.claktory_id == key.claktory_id;
    (said && !statement.device_key.is_empty() && api::verify_recovery_key_device(&key.statement, &key.device_sig)).then_some(statement)
}

/// Whether `key` checks back to `sealed`, the public half the vault is
/// sealed to ("" for none: back to the account's first key): its statement
/// and each one before it in `history`, unbroken, each naming the key it
/// replaces. The service's word on which backend key a Claktory has is only
/// its own, so this proves no more than that a device approved each step:
/// on its own it's never a reason to seal, only to let the person choose.
fn checked(account_id: &str, key: &RecoveryKey, sealed: &str) -> bool {
    let mut link = key;
    for _ in 0..=key.history.len() {
        let Some(statement) = statement_of(account_id, link) else { return false };
        if statement.replaces == sealed {
            return true;
        }
        let Some(before) = key.history.iter().find(|before| !statement.replaces.is_empty() && before.public_key == statement.replaces) else {
            return false;
        };
        link = before;
    }
    false
}

/// Compares the account's key, as claktory.com gave it at sign-in or a
/// check-in (`given`), with the one the vault is sealed to, and does what
/// the module's comment says.
pub(super) async fn compare(factory: &Arc<Factory>, given: Option<RecoveryKey>) {
    let account = &factory.account;
    let account_id = {
        let inner = account.lock();
        if !inner.saved.signed_in() || inner.saved.revoked {
            return;
        }
        inner.saved.account_id.clone()
    };
    if !factory.vault.exists() {
        return;
    }
    let Ok(given) = usable(given) else {
        tracing::warn!("claktory.com gave a recovery key for the account that can't be used; ignored");
        return;
    };
    let sealed = factory.vault.account_key().filter(|sealed| sealed.account_id == account_id).map(|sealed| sealed.public_key);
    {
        let mut inner = account.lock();
        match (&given, &sealed) {
            (Some(key), Some(sealed)) if key.public_key == *sealed => {
                inner.saved.recovery_made_by = key.claktory_id.clone();
                inner.saved.recovery_made_name = name(&key.claktory_name);
                inner.saved.recovery_made_ms = key.set_at.saturating_mul(1000);
                inner.saved.recovery_waiting = None;
            }
            // claktory.com has none, or one this can't seal to by itself.
            (None, _) => {
                inner.saved.recovery_made_by.clear();
                inner.saved.recovery_waiting = None;
            }
            (Some(_), _) => inner.saved.recovery_made_by.clear(),
        }
    }
    if let Some(key) = given.filter(|key| sealed.as_deref() != Some(key.public_key.as_str())) {
        let checked = checked(&account_id, &key, sealed.as_deref().unwrap_or_default());
        wait(factory, &account_id, &key, checked);
    }
    // The Workspace goes out after: the check-in's, or the sign-in's.
    account.save();
}

/// claktory.com's key isn't the one the vault is sealed to: the vault stays
/// as it is, and the person hears of it once per key.
fn wait(factory: &Factory, account_id: &str, key: &RecoveryKey, checked: bool) {
    let new = {
        let mut inner = factory.account.lock();
        let new = inner.saved.recovery_waiting.as_ref().is_none_or(|waiting| waiting.public_key != key.public_key || waiting.account_id != account_id);
        if new {
            inner.saved.recovery_waiting = Some(Waiting {
                account_id: account_id.to_owned(),
                public_key: key.public_key.clone(),
                made_by: key.claktory_id.clone(),
                made_name: name(&key.claktory_name),
                made_ms: key.set_at.saturating_mul(1000),
                seen_ms: now_ms(),
                checked,
            });
        }
        new
    };
    if new {
        tracing::warn!(checked, "claktory.com has another recovery key for the account than the vault is sealed to; waiting for the person");
        let config = factory.lock().config.clone();
        let ours = factory.account.lock().saved.claktory_id.clone();
        let made_on = name_of(&config, &ours, &key.claktory_id, &key.claktory_name);
        let made_on = if made_on.is_empty() { NO_LONGER.to_owned() } else { made_on };
        crate::notify::host_alert(factory, &config, &alert(checked, &made_on, key.set_at.saturating_mul(1000)));
    }
}

/// Claktory's own alert about a key the vault isn't sealed to: `checked`,
/// the person may choose to use it; `made_on`, the Claktory that made it.
fn alert(checked: bool, made_on: &str, made_ms: i64) -> String {
    let when = crate::transcript::local_time(made_ms, "%-d %B %Y at %H:%M");
    if checked {
        format!(
            "Your account has a new recovery key. It was made on {made_on} on {when}. Use it here only if you made it: open Settings → Secrets \
             and choose Use the key here. Until you do, the key you saved before still opens your secrets here."
        )
    } else {
        format!(
            "Claktory couldn't check a new recovery key for your account. claktory.com says it was made on {made_on} on {when}, but nothing \
             shows you made it, so Claktory won't use it here. The key you saved before still opens your secrets. If you didn't make a new \
             key, check your account at claktory.com."
        )
    }
}

/// UseRecoveryKey: the person chose to seal the vault to claktory.com's
/// current key, from a device paired here (or this computer's own app).
pub(crate) async fn use_new(factory: &Arc<Factory>, device: &str, tool: bool) -> Result<(), RequestError> {
    super::enabled(factory)?;
    let account_config = factory.lock().config.account.clone();
    let owner = super::owner(factory, &account_config).map(|owner| owner.account_id);
    let trusted = !tool
        && (device == crate::remote::LOCAL_DEVICE
            || crate::remote::devices(&factory.home)
                .map_err(|error| RequestError::Remote(error.to_string()))?
                .iter()
                .any(|known| known.id == device && known.confirmed_for(owner.as_deref())));
    if !trusted {
        return Err(refused(
            ErrorCode::Unauthorized,
            "To use the new key, sign in to your Claktory account on this device first.",
        ));
    }
    let account = &factory.account;
    let _busy = account.busy.lock().await;
    let (account_id, waiting) = {
        let inner = account.lock();
        (inner.saved.account_id.clone(), inner.saved.recovery_waiting.clone())
    };
    let Some(waiting) = waiting.filter(|waiting| waiting.account_id == account_id && waiting.checked) else {
        return Err(refused(ErrorCode::InvalidRequest, "There's no new recovery key to use."));
    };
    let Some(public) = account_recovery::public_of(&waiting.public_key) else {
        return Err(refused(ErrorCode::InvalidRequest, "There's no new recovery key to use."));
    };
    factory.vault.seal_to_account_key(&account_id, &public)?;
    {
        let mut inner = account.lock();
        inner.saved.recovery_made_by = waiting.made_by.clone();
        inner.saved.recovery_made_name = waiting.made_name.clone();
        inner.saved.recovery_made_ms = waiting.made_ms;
        inner.saved.recovery_waiting = None;
    }
    account.save();
    record(factory, "You chose to use your account's new recovery key here", waiting.seen_ms);
    factory.vault_changed();
    changed(factory);
    Ok(())
}

/// Keeps a change of the key the vault is sealed to, for History.
fn record(factory: &Factory, title: &str, seen: i64) {
    let kept = factory.store.db().execute(
        "INSERT INTO recovery_key_changes (title, seen, at) VALUES (?1, ?2, ?3)",
        rusqlite::params![title, seen, now_ms()],
    );
    if let Err(error) = kept {
        tracing::error!(%error, "couldn't keep a recovery key change for the history");
    }
}

/// A change as kept.
#[derive(Clone, Debug)]
pub(crate) struct Change {
    pub id: i64,
    pub title: String,
    pub seen: i64,
    pub at: i64,
}

/// Changes from `since` to before `before`, newest first, at most `limit`.
pub(crate) fn between(db: &rusqlite::Connection, since: i64, before: i64, limit: i64) -> rusqlite::Result<Vec<Change>> {
    let mut statement =
        db.prepare("SELECT id, title, seen, at FROM recovery_key_changes WHERE at >= ?1 AND at < ?2 ORDER BY at DESC, id DESC LIMIT ?3")?;
    let rows = statement.query_map(rusqlite::params![since, before, limit], |row| {
        Ok(Change { id: row.get(0)?, title: row.get(1)?, seen: row.get(2)?, at: row.get(3)? })
    })?;
    rows.collect()
}

/// The account's fields (#239) of a VaultState.
pub(crate) fn fill(factory: &Factory, config: &Config, state: &mut v1::VaultState) {
    if !state.exists {
        return;
    }
    let view = view(factory, config);
    state.recovery_key_covered = view.covered;
    state.recovery_key_made_on = view.made_on;
    state.recovery_key_made_ms = view.made_ms;
    state.recovery_key_change_waiting = view.waiting;
    state.recovery_key_change_made_on = view.waiting_made_on;
    state.recovery_key_change_made_ms = view.waiting_made_ms;
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::SigningKey;

    /// A key `claktory` set for `account`, replacing `replaces`, approved
    /// by `device` (None: by no device).
    fn key(account: &str, claktory: &SigningKey, public: u8, replaces: &str, device: Option<&SigningKey>) -> RecoveryKey {
        let public_key = api::b64(account_recovery::pair(&[public; 20]).1);
        let statement = RecoveryKeyStatement {
            v: 1,
            account_id: account.to_owned(),
            claktory_id: "c1".to_owned(),
            public_key: public_key.clone(),
            replaces: replaces.to_owned(),
            device_key: device.map(|device| api::b64(device.verifying_key().as_bytes())).unwrap_or_default(),
            iat: 1_763_647_200,
        };
        let statement = api::sign_recovery_key(claktory, &statement);
        let device_sig = device.map(|device| api::sign_recovery_key_device(device, &statement)).unwrap_or_default();
        RecoveryKey {
            public_key,
            statement,
            device_sig,
            claktory_id: "c1".to_owned(),
            backend_key: api::b64(claktory.verifying_key().as_bytes()),
            set_at: 1_763_647_200,
            claktory_name: "Mac\u{202e}mini\nOne".to_owned(),
            history: Vec::new(),
        }
    }

    #[test]
    fn a_key_checks_only_with_every_statement_back_to_the_sealed_one_approved_by_a_device() {
        let (claktory, device) = (api::new_signing_key(), api::new_signing_key());
        let first = key("a1", &claktory, 1, "", Some(&device));
        assert!(checked("a1", &first, ""), "the account's first, approved");
        assert!(!checked("a2", &first, ""), "another account's");
        assert!(!checked("a1", &first, &api::b64([5u8; 32])), "it doesn't replace the sealed one");
        assert!(!checked("a1", &key("a1", &claktory, 1, "", None), ""), "approved by no device");
        let mut forged = first.clone();
        forged.backend_key = api::b64(api::new_signing_key().verifying_key().as_bytes());
        assert!(!checked("a1", &forged, ""), "not signed by the backend key given");
        let mut swapped = first.clone();
        swapped.public_key = api::b64(account_recovery::pair(&[9; 20]).1);
        assert!(!checked("a1", &swapped, ""), "a statement about another key");
        let mut unapproved = first.clone();
        unapproved.device_sig = api::sign_recovery_key_device(&api::new_signing_key(), &first.statement);
        assert!(!checked("a1", &unapproved, ""), "signed by another device than it names");

        // Two changes on, back to the sealed one through the history.
        let second = key("a1", &claktory, 2, &first.public_key, Some(&device));
        let mut third = key("a1", &claktory, 3, &second.public_key, Some(&device));
        third.history = vec![second.clone(), first.clone()];
        assert!(checked("a1", &third, &first.public_key));
        assert!(checked("a1", &third, ""), "or back to the account's first");
        assert!(checked("a1", &third, &second.public_key));
        third.history = vec![first.clone()];
        assert!(!checked("a1", &third, &first.public_key), "a link missing");
        let mut broken = key("a1", &claktory, 2, &first.public_key, None);
        broken.history.clear();
        third.history = vec![broken, first.clone()];
        assert!(!checked("a1", &third, &first.public_key), "a link no device approved");
        // A history that goes round in circles ends.
        let mut looping = key("a1", &claktory, 4, &api::b64(account_recovery::pair(&[4; 20]).1), Some(&device));
        looping.history = vec![looping.clone()];
        assert!(!checked("a1", &looping, &first.public_key));
    }

    #[test]
    fn a_name_from_claktory_com_shows_on_one_line_with_nothing_hidden() {
        assert_eq!(name("Mac\u{202e}mini\nOne"), "MacminiOne");
        assert_eq!(name(&"x".repeat(100)).chars().count(), 40);
        assert_eq!(name(""), "");
    }

    #[test]
    fn the_alert_says_whether_the_person_may_use_the_key() {
        let checked = alert(true, "another Claktory", 1_763_647_200_000);
        assert!(checked.starts_with("Your account has a new recovery key. ") && checked.contains("Use it here only if you made it"), "{checked}");
        let not = alert(false, "another Claktory", 1_763_647_200_000);
        assert!(not.contains("couldn't check") && !not.contains("Use the key here"), "{not}");
    }
}
