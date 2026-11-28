//! The account's recovery key (#239; coordination/security/
//! recovery-and-sign-in.md, part 1): one key covers every Claktory of the
//! account. claktory.com keeps only its public half, made and approved on
//! one of the person's devices, and gives it back at sign-in and at every
//! check-in; each Claktory seals its own vault to it (`vault::
//! account_recovery`).
//!
//! A key is made on one of the person's devices (`make`, `publish`): the
//! device makes the pair and sends its public half with its own account
//! key, the one its connection was let in with; this Claktory gives it the
//! statement to sign, then signs it too, sets it at claktory.com and seals
//! its vault to it.
//!
//! At sign-in and each check-in this Claktory compares claktory.com's key
//! with the one its vault is sealed to. The same: nothing to do. Another,
//! the account's first included: a key whose statements check (each signed
//! by its Claktory's backend key and approved by a device, back to the key
//! the vault is sealed to, or to the account's first) is sealed to by
//! itself when every device that approved a step is one paired here and
//! not removed (recovery-and-sign-in.md part 1, point 3); otherwise it
//! waits for the person to choose Use the key here (UseRecoveryKey) on
//! this computer's app or a device paired here and confirmed as theirs,
//! and Claktory's own alert says so. One that doesn't check gets an alert
//! saying it couldn't be checked, and nothing to choose. Each seal is a
//! record in History.

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
    /// Claktory's alert about it in Needs you, read once the person uses
    /// it (Abbacchio's row 1729b); empty when none was kept.
    pub alert_id: String,
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
    }
    // Still covered while a newer key waits, or one that doesn't check
    // came: the vault stays sealed to the key that covered it, which still
    // opens it (Abbacchio's row 1729a).
    if sealed.is_some() && !saved.recovery_made_by.is_empty() && factory.vault.account_covered_now() {
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
#[cfg(test)]
fn checked(account_id: &str, key: &RecoveryKey, sealed: &str) -> bool {
    chain(account_id, key, sealed).is_some()
}

/// `checked`'s statements, newest first, when they check.
fn chain(account_id: &str, key: &RecoveryKey, sealed: &str) -> Option<Vec<RecoveryKeyStatement>> {
    let mut link = key;
    let mut links = Vec::new();
    for _ in 0..=key.history.len() {
        let statement = statement_of(account_id, link)?;
        let replaces = statement.replaces.clone();
        links.push(statement);
        if replaces == sealed {
            return Some(links);
        }
        link = key.history.iter().find(|before| !replaces.is_empty() && before.public_key == replaces)?;
    }
    None
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
        inner.saved.recovery_account_key = given.as_ref().map(|key| key.public_key.clone()).unwrap_or_default();
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
            // Another key: the vault stays sealed to the one it was, and
            // who made that one stays known (row 1729a).
            (Some(_), _) => {}
        }
    }
    if let Some(key) = given.filter(|key| sealed.as_deref() != Some(key.public_key.as_str())) {
        let chain = chain(&account_id, &key, sealed.as_deref().unwrap_or_default());
        // Every step approved on a device paired here: sealed to by itself.
        let ours = chain.as_ref().is_some_and(|links| {
            links.iter().all(|link| crate::remote::approver(&factory.home, &link.device_key, &account_id).is_some())
        });
        if !(ours && seal_by_itself(factory, &account_id, &key)) {
            wait(factory, &account_id, &key, chain.is_some());
        }
    }
    // The Workspace goes out after: the check-in's, or the sign-in's.
    account.save();
}

/// Seals the vault to the account's new key `key`, approved at every step
/// on a device paired here: no alert, a record in History. False when it
/// couldn't (the vault locked, say): it waits for the person instead.
fn seal_by_itself(factory: &Factory, account_id: &str, key: &RecoveryKey) -> bool {
    let Some(public) = account_recovery::public_of(&key.public_key) else { return false };
    if let Err(error) = factory.vault.seal_to_account_key(account_id, &public) {
        tracing::warn!(%error, "couldn't seal the vault to the account's new recovery key");
        return false;
    }
    {
        let mut inner = factory.account.lock();
        inner.saved.recovery_made_by = key.claktory_id.clone();
        inner.saved.recovery_made_name = name(&key.claktory_name);
        inner.saved.recovery_made_ms = key.set_at.saturating_mul(1000);
        inner.saved.recovery_waiting = None;
    }
    let config = factory.lock().config.clone();
    let ours = factory.account.lock().saved.claktory_id.clone();
    let made_on = name_of(&config, &ours, &key.claktory_id, &key.claktory_name);
    let made_on = if made_on.is_empty() { NO_LONGER.to_owned() } else { made_on };
    tracing::info!("sealed the vault to the account's new recovery key, approved on a device paired here");
    record(factory, &format!("Your account's new recovery key, made on {made_on}, now opens your secrets here too"), now_ms());
    factory.vault_changed();
    true
}

/// A recovery key a device is making for the account (`make`): the
/// statement it was given to sign, and when.
#[derive(Clone, Debug)]
pub(super) struct Making {
    statement: RecoveryKeyStatement,
    asked: std::time::Instant,
}

/// How long a device has to sign a new key's statement.
const SIGN_WITHIN: std::time::Duration = std::time::Duration::from_secs(300);

/// NewRecoveryKey with a public half: a device of the person's made a key
/// pair for the account and asks for the statement to sign. Its
/// `device_key` must be the account key its connection was let in with
/// (`account_key`; `remote`, a device on another machine), of a device
/// paired here, confirmed for the account and not removed (`remote::
/// approver`). The statement replaces the account's key as claktory.com
/// last gave it.
pub(crate) fn make(factory: &Factory, account_key: Option<&str>, remote: bool, request: &v1::NewRecoveryKey) -> Result<v1::RecoveryKeyToSign, RequestError> {
    super::enabled(factory)?;
    let (account_id, claktory_id, replaces, offset) = {
        let inner = factory.account.lock();
        if !inner.saved.signed_in() || inner.saved.revoked {
            return Err(refused(ErrorCode::SignInRequired, "This Claktory isn't signed in to your account."));
        }
        let saved = &inner.saved;
        (saved.account_id.clone(), saved.claktory_id.clone(), saved.recovery_account_key.clone(), inner.clock_offset)
    };
    let Some(account_key) = account_key.filter(|key| !key.is_empty()) else {
        return Err(refused(ErrorCode::SignInRequired, super::cannot_approve(factory, None, remote)));
    };
    if request.device_key != account_key {
        return Err(refused(ErrorCode::InvalidRequest, "That isn't the account key this device connected with."));
    }
    if crate::remote::approver(&factory.home, account_key, &account_id).is_none() {
        return Err(refused(
            ErrorCode::Unauthorized,
            "To make a recovery key for your account, sign in to your Claktory account on this device first.",
        ));
    }
    if account_recovery::public_of(&request.public_key).is_none() {
        return Err(refused(ErrorCode::InvalidRequest, "That isn't a recovery key Claktory can use."));
    }
    if !factory.vault.exists() {
        return Err(refused(ErrorCode::InvalidRequest, "This Claktory has no secrets to protect yet."));
    }
    let statement = RecoveryKeyStatement {
        v: 1,
        account_id,
        claktory_id,
        public_key: request.public_key.clone(),
        replaces,
        device_key: account_key.to_owned(),
        iat: super::now_s() + offset,
    };
    let payload = serde_json::to_vec(&statement).map_err(|error| refused(ErrorCode::Internal, error.to_string()))?;
    factory.account.lock().recovery_making = Some(Making { statement, asked: std::time::Instant::now() });
    Ok(v1::RecoveryKeyToSign { payload: api::b64(payload) })
}

/// PublishRecoveryKey: the device's signature on `make`'s statement.
/// Checked with its account key, the statement is signed with this
/// Claktory's backend key, set at claktory.com, and the vault sealed to it
/// in place of the account's last key.
pub(crate) async fn publish(factory: &Arc<Factory>, account_key: Option<&str>, request: &v1::PublishRecoveryKey) -> Result<(), RequestError> {
    super::enabled(factory)?;
    let account = &factory.account;
    let _busy = account.busy.lock().await;
    let making = account.lock().recovery_making.clone();
    let Some(making) = making.filter(|making| {
        account_key == Some(making.statement.device_key.as_str()) && making.statement.device_key == request.device_key
    }) else {
        return Err(refused(ErrorCode::InvalidRequest, "No recovery key is being made on this device. Make it again."));
    };
    if making.asked.elapsed() > SIGN_WITHIN {
        account.lock().recovery_making = None;
        return Err(refused(ErrorCode::InvalidRequest, "Making the key took too long. Make it again."));
    }
    let still = {
        let inner = account.lock();
        inner.saved.signed_in() && !inner.saved.revoked && inner.saved.account_id == making.statement.account_id
    };
    if !still {
        return Err(refused(ErrorCode::SignInRequired, "This Claktory isn't signed in to that account any more."));
    }
    let (_, backend_key) = account.identity().map_err(|error| refused(ErrorCode::Internal, error.to_string()))?;
    let statement = api::sign_recovery_key(&backend_key, &making.statement);
    if !api::verify_recovery_key_device(&statement, &request.device_sig) {
        return Err(refused(ErrorCode::InvalidRequest, "This device's approval of the key doesn't check. Make it again."));
    }
    let Some(public) = account_recovery::public_of(&making.statement.public_key) else {
        return Err(refused(ErrorCode::InvalidRequest, "That isn't a recovery key Claktory can use."));
    };
    let set = api::SetRecoveryKey { statement, device_sig: request.device_sig.clone() };
    match account.signed_with::<api::RecoveryKeySet>("PUT", "/v1/account/recovery-key", &set).await {
        Ok(_) => {}
        Err(super::Failure::Refused(409, _)) => {
            account.lock().recovery_making = None;
            account.wake.notify_one();
            return Err(refused(
                ErrorCode::InvalidRequest,
                "Your account's recovery key changed meanwhile. Look at it again, then make a new one if you still want to.",
            ));
        }
        Err(super::Failure::Refused(status, body)) => {
            tracing::warn!(status, error = body.error, "claktory.com refused the new recovery key");
            return Err(refused(ErrorCode::InvalidRequest, format!("claktory.com didn't take the key: {}", body.message)));
        }
        Err(failure) => {
            tracing::warn!(%failure, "couldn't set the new recovery key at claktory.com");
            return Err(refused(ErrorCode::Unreachable, "Can't reach claktory.com. Check your internet connection and try again."));
        }
    }
    let account_id = making.statement.account_id.clone();
    {
        let mut inner = account.lock();
        inner.recovery_making = None;
        inner.saved.recovery_account_key = making.statement.public_key.clone();
    }
    account.save();
    // Set at claktory.com: if the seal fails here, the next check-in seals
    // by itself (approved on a device paired here).
    factory.vault.seal_to_account_key(&account_id, &public)?;
    {
        let mut inner = account.lock();
        inner.saved.recovery_made_by = inner.saved.claktory_id.clone();
        inner.saved.recovery_made_name.clear();
        inner.saved.recovery_made_ms = now_ms();
        inner.saved.recovery_waiting = None;
    }
    account.save();
    record(factory, "You made a new recovery key for your account", now_ms());
    factory.vault_changed();
    changed(factory);
    Ok(())
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
                alert_id: String::new(),
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
        let alert_id = crate::notify::host_alert_kept(factory, &config, &alert(checked, &made_on, key.set_at.saturating_mul(1000)));
        let mut inner = factory.account.lock();
        if let Some(waiting) = inner.saved.recovery_waiting.as_mut().filter(|waiting| waiting.public_key == key.public_key) {
            waiting.alert_id = alert_id;
        }
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
pub(crate) async fn use_new(factory: &Arc<Factory>, device: &str, device_name: &str, tool: bool) -> Result<(), RequestError> {
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
    // Its alert is done with: read, on the device that chose.
    if !waiting.alert_id.is_empty() {
        crate::notify::alerts::read(factory, std::slice::from_ref(&waiting.alert_id), device, device_name)?;
    }
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
        assert_eq!(name("Mac\u{202e}mini\nOne"), "Macmini One");
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
