//! The app's vault requests (Settings → Secrets).

use std::sync::Arc;

use factory_protocol::v1;

use super::{Key, Value, Vault, VaultError, env};
use crate::factory::{Factory, RequestError};

/// How long a wrong passphrase takes to be refused, so guessing is slow.
const WRONG_DELAY: std::time::Duration = std::time::Duration::from_secs(1);

/// Runs slow vault work (Argon2) off the async threads.
async fn blocking<T: Send + 'static>(
    factory: &Arc<Factory>,
    work: impl FnOnce(&Vault) -> Result<T, VaultError> + Send + 'static,
) -> Result<T, RequestError> {
    let factory = factory.clone();
    match tokio::task::spawn_blocking(move || work(&factory.vault)).await {
        Ok(result) => result.map_err(RequestError::Vault),
        Err(error) => Err(RequestError::Io(std::io::Error::other(error))),
    }
}

/// The vault's key, if `given` is its passphrase or recovery key; a wrong
/// one is refused after a second.
pub(crate) async fn prove(factory: &Arc<Factory>, given: &str) -> Result<Key, RequestError> {
    let given = zeroize::Zeroizing::new(given.to_owned());
    let (found, passphrase) = blocking(factory, move |vault| {
        let (vault_id, protectors) = vault.protectors()?;
        let passphrase = protectors.iter().any(|protector| protector.kind == "passphrase");
        Ok((Vault::check(&vault_id, &protectors, &given), passphrase))
    })
    .await?;
    match found {
        Some(vmk) => Ok(vmk),
        None => {
            tokio::time::sleep(WRONG_DELAY).await;
            // Only vaults from before Claktory made them by itself have a passphrase.
            let said = if passphrase { "That passphrase or recovery key isn't right." } else { "That recovery key isn't right." };
            Err(RequestError::Vault(VaultError::WrongProof(said)))
        }
    }
}

/// Opens a locked vault; in any mode, checks the passphrase.
pub(crate) async fn unlock(factory: &Arc<Factory>, passphrase: &str) -> Result<(), RequestError> {
    let vmk = prove(factory, passphrase).await?;
    if !factory.vault.is_open() {
        factory.vault.unlock(vmk);
        factory.vault_changed();
    }
    Ok(())
}

pub(crate) async fn change_passphrase(factory: &Arc<Factory>, old: &str, new: &str) -> Result<(), RequestError> {
    let vmk = prove(factory, old).await?;
    let new = zeroize::Zeroizing::new(new.to_owned());
    let reopened = vmk.clone();
    blocking(factory, move |vault| vault.set_passphrase(&vmk, &new)).await?;
    // The old passphrase (or recovery key) opens a locked vault as well.
    if !factory.vault.is_open() {
        factory.vault.unlock(reopened);
        factory.vault_changed();
    }
    Ok(())
}

fn values(entries: &[v1::SecretEntry]) -> Vec<Value> {
    entries.iter().map(Value::from_entry).collect()
}

pub(crate) fn create_secret(factory: &Factory, request: &v1::CreateSecret) -> Result<(), RequestError> {
    factory.vault.create_secret(&request.name, &request.description, &values(&request.entries), "")?;
    factory.vault_changed();
    Ok(())
}

pub(crate) fn update_secret(factory: &Factory, request: &v1::UpdateSecret) -> Result<(), RequestError> {
    let values = values(&request.entries);
    let values = (!values.is_empty()).then_some(values.as_slice());
    factory.vault.update_secret(&request.id, &request.name, &request.description, values)?;
    factory.vault_changed();
    Ok(())
}

pub(crate) fn revoke_secret(factory: &Factory, request: &v1::RevokeSecret) -> Result<(), RequestError> {
    take_back(factory, &request.id, &request.agent_id, "", request.stop_running_tasks)?;
    Ok(())
}

/// Lets `agent` use the secret (by id or name), `by` an admin agent ('' for
/// the person). Returns its name.
pub(crate) fn give(factory: &Factory, secret: &str, agent: &str, by: &str) -> Result<String, RequestError> {
    if factory.lock().config.agent(agent).is_none() {
        return Err(RequestError::UnknownAgent(agent.to_owned()));
    }
    let name = factory.vault.grant(secret, agent, by)?;
    factory.vault_changed();
    Ok(name)
}

/// Takes the secret (by id or name) back from `agent`, `by` an admin agent
/// ('' for the person), and with `stop_tasks` stops the agent's running
/// tasks that got it. Returns its name and the tasks stopped.
pub(crate) fn take_back(
    factory: &Factory,
    secret: &str,
    agent: &str,
    by: &str,
    stop_tasks: bool,
) -> Result<(String, Vec<String>), RequestError> {
    let tasks = if stop_tasks { factory.vault.recent_tasks(secret, agent) } else { Vec::new() };
    let name = factory.vault.revoke(secret, agent, by)?;
    factory.vault_changed();
    // Only the ones still running stop; the rest have ended already.
    let stopper = {
        let inner = factory.lock();
        match inner.config.agent(by) {
            Some(admin) => admin.name.clone(),
            None => inner.config.person().to_owned(),
        }
    };
    let mut stopped = Vec::new();
    for task in tasks {
        if !factory.store.task_running(agent, &task) {
            continue;
        }
        match crate::extensions::tasks::stop(factory, agent, &task, &stopper) {
            Ok(()) => stopped.push(task),
            Err(error) => tracing::warn!(agent, task, error, "could not stop a task that had a revoked secret"),
        }
    }
    Ok((name, stopped))
}

pub(crate) fn preview_env(text: &str) -> v1::EnvPreview {
    let parsed = env::parse(text);
    let variables = parsed
        .values
        .iter()
        .map(|value| v1::SecretVariable { name: value.variable.clone(), hint: super::hint(&value.value), file: false })
        .collect();
    v1::EnvPreview { variables, warnings: parsed.warnings }
}

pub(crate) fn import_env(factory: &Factory, request: &v1::ImportEnv) -> Result<(), RequestError> {
    let parsed = env::parse(&request.text);
    if parsed.values.is_empty() {
        return Err(VaultError::Invalid("There are no KEY=value lines in it.".to_owned()).into());
    }
    if request.one_per_variable {
        for value in parsed.values {
            factory.vault.create_secret(&value.variable.clone(), &request.description, &[value], "")?;
        }
    } else {
        factory.vault.create_secret(&request.name, &request.description, &parsed.values, "")?;
    }
    factory.vault_changed();
    Ok(())
}

pub(crate) fn list_uses(factory: &Factory, request: &v1::ListSecretUses) -> Result<v1::SecretUses, RequestError> {
    Ok(v1::SecretUses { uses: factory.vault.uses(&request.id, request.limit)? })
}

/// A connection's side of the vault: which device it is, for the log of
/// reveals. The person's requests need no proof: a connection is the
/// person's app, which proved itself with its device's token, and agents
/// can't make them.
pub(crate) struct Session {
    device: String,
}

impl Session {
    pub(crate) fn new(device: String) -> Session {
        Session { device }
    }

    pub(crate) fn device(&self) -> &str {
        &self.device
    }

    /// Unused since requests need no proof: always 0.
    pub(crate) fn answer(&self) -> v1::VaultSession {
        v1::VaultSession { unlocked_until_ms: 0 }
    }
}

/// The vault open, for what needs its key. A vault made with a passphrase
/// in locked mode (before Claktory made the vault by itself) opens with the
/// passphrase or recovery key given as proof, or with UnlockVault.
async fn open(factory: &Arc<Factory>, proof: Option<&v1::VaultProof>) -> Result<(), RequestError> {
    if !factory.vault.exists() {
        return Err(VaultError::NoVault.into());
    }
    if factory.vault.is_open() {
        return Ok(());
    }
    match proof.map(|proof| proof.passphrase.as_str()).filter(|passphrase| !passphrase.is_empty()) {
        Some(passphrase) => unlock(factory, passphrase).await,
        None => Err(VaultError::Locked.into()),
    }
}

/// Claktory makes the vault by itself when it starts.
pub(crate) fn create_vault_in(
    _factory: &Arc<Factory>,
    _session: &mut Session,
    _request: &v1::CreateVault,
) -> Result<v1::RecoveryKey, RequestError> {
    Err(VaultError::Exists.into())
}

pub(crate) async fn unlock_in(factory: &Arc<Factory>, session: &mut Session, passphrase: &str) -> Result<v1::VaultSession, RequestError> {
    unlock(factory, passphrase).await?;
    Ok(session.answer())
}

pub(crate) async fn change_passphrase_in(
    factory: &Arc<Factory>,
    session: &mut Session,
    request: &v1::ChangePassphrase,
) -> Result<v1::VaultSession, RequestError> {
    change_passphrase(factory, &request.old_passphrase, &request.new_passphrase).await?;
    Ok(session.answer())
}

pub(crate) async fn delete_secret(
    factory: &Arc<Factory>,
    session: &mut Session,
    request: &v1::DeleteSecret,
) -> Result<v1::VaultSession, RequestError> {
    open(factory, request.proof.as_ref()).await?;
    // A connection goes with the secrets made from it, and a secret made
    // from one with its token at the service.
    if factory.vault.connection(&request.id).is_ok() {
        factory.disconnect_service(&request.id, false).await?;
        return Ok(session.answer());
    }
    if let Some(export) = factory.vault.export(&request.id)? {
        factory.remove_export(&export).await?;
        return Ok(session.answer());
    }
    factory.vault.delete_secret(&request.id)?;
    factory.vault_changed();
    Ok(session.answer())
}

pub(crate) async fn grant_secret(
    factory: &Arc<Factory>,
    session: &mut Session,
    request: &v1::GrantSecret,
) -> Result<v1::VaultSession, RequestError> {
    open(factory, request.proof.as_ref()).await?;
    give(factory, &request.id, &request.agent_id, "")?;
    Ok(session.answer())
}

pub(crate) async fn reveal_secret(
    factory: &Arc<Factory>,
    session: &mut Session,
    request: &v1::RevealSecret,
) -> Result<v1::SecretValue, RequestError> {
    open(factory, request.proof.as_ref()).await?;
    let (id, values) = factory.vault.reveal(&request.id, &session.device)?;
    let entries = values
        .iter()
        .map(|value| v1::SecretEntry { variable: value.variable.clone(), value: value.value.to_string(), file: value.file })
        .collect();
    Ok(v1::SecretValue { id, entries, unlocked_until_ms: 0 })
}

/// A recovery key, a key the person keeps that opens the vault too; a new
/// one replaces the last.
pub(crate) async fn new_recovery_key(
    factory: &Arc<Factory>,
    _session: &mut Session,
    request: &v1::NewRecoveryKey,
) -> Result<v1::RecoveryKey, RequestError> {
    open(factory, request.proof.as_ref()).await?;
    let key = factory.vault.new_recovery_key()?;
    Ok(v1::RecoveryKey { key, unlocked_until_ms: 0 })
}

/// The recovery key to note down (one is made if there's none), logged as
/// shown on the session's device.
pub(crate) fn show_recovery_key(factory: &Factory, session: &Session) -> Result<v1::RecoveryKey, RequestError> {
    let key = factory.vault.recovery_key(session.device())?;
    factory.vault_changed();
    Ok(v1::RecoveryKey { key, unlocked_until_ms: 0 })
}

pub(crate) async fn set_locked_mode(
    factory: &Arc<Factory>,
    session: &mut Session,
    request: &v1::SetVaultLockedMode,
) -> Result<v1::VaultSession, RequestError> {
    open(factory, request.proof.as_ref()).await?;
    factory.vault.set_locked_mode(request.locked_mode)?;
    factory.vault_changed();
    Ok(session.answer())
}

/// The person gives an agent what it asked for (a new secret made from
/// `entries`, or an existing one), or dismisses the request; its entry
/// shows the decision, and the agent is told.
pub(crate) fn decide_request(factory: &Arc<Factory>, request: &v1::DecideSecretRequest) -> Result<(), RequestError> {
    let agent = request.agent_id.as_str();
    if factory.lock().config.agent(agent).is_none() {
        return Err(RequestError::UnknownAgent(agent.to_owned()));
    }
    let asked = factory.vault.pending_request(agent, &request.request_id)?;
    let given = if request.approve {
        let secret = if !request.secret_id.is_empty() {
            request.secret_id.clone()
        } else if !request.entries.is_empty() {
            let wanted = asked.new_secret.clone().unwrap_or_default();
            let pick = |chosen: &str, asked: &str| if chosen.trim().is_empty() { asked.to_owned() } else { chosen.to_owned() };
            let name = pick(&request.name, if wanted.name.is_empty() { &asked.secret_name } else { &wanted.name });
            let description = pick(&request.description, &wanted.description);
            factory.vault.create_secret(&name, &description, &values(&request.entries), "")?
        } else if !asked.secret_id.is_empty() {
            asked.secret_id.clone()
        } else {
            return Err(VaultError::Invalid("Choose a secret to give, or add a new one with its values.".to_owned()).into());
        };
        let name = give(factory, &secret, agent, "")?;
        Some((secret, name))
    } else {
        None
    };
    let decided =
        factory.vault.decide_request(agent, &asked.id, given.as_ref().map(|(id, name)| (id.as_str(), name.as_str())), &request.comment)?;
    factory.vault_changed();
    // Its entry shows the decision.
    {
        let mut inner = factory.lock();
        let entry = inner
            .agents
            .get_mut(agent)
            .and_then(|runtime| runtime.transcript.as_mut())
            .and_then(|transcript| transcript.set_secret_request(decided));
        if let Some(entry) = entry {
            factory.send_change(&inner, agent, crate::transcript::Change::Entry(entry));
        }
    }
    let person = factory.lock().config.person_capitalized().to_owned();
    let comment = request.comment.trim();
    // What it got, and nothing else of the vault.
    let (mut summary, what) = match &given {
        Some((id, name)) => {
            let label = factory.vault.granted(agent).remove(id).unwrap_or_else(|| name.clone());
            (format!("{person} gave you {name}: pass it in bash's `secrets`"), format!("\n\nIt's {label}."))
        }
        None => (format!("{person} dismissed your request for {}", asked.secret_name), String::new()),
    };
    if !comment.is_empty() {
        summary.push_str(&format!(" (comment: {comment})"));
    }
    summary.push('.');
    let text = format!("{summary}{what}\n\nYou asked because: {}", asked.reason);
    let event = factory_agent::Event {
        source: factory_agent::EventSource::Host,
        from: "Claktory".to_owned(),
        id: String::new(),
        summary,
        transient: false,
    };
    factory.deliver_event(agent, text, event);
    Ok(())
}
