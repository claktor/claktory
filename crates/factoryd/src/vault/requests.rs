//! The app's vault requests (Settings → Secrets).

use std::sync::Arc;

use factory_protocol::v1;

use super::{Key, Value, Vault, VaultError, env};
use crate::decisions::note;
use crate::extensions::checklist::Card;
use crate::factory::{Factory, RequestError, host_event};

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
        crate::account::opened_by_hand(factory);
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
        crate::account::opened_by_hand(factory);
    }
    Ok(())
}

fn values(entries: &[v1::SecretEntry]) -> Vec<Value> {
    entries.iter().map(Value::from_entry).collect()
}

pub(crate) fn create_secret(factory: &Factory, request: &v1::CreateSecret) -> Result<(), RequestError> {
    let id = factory.vault.create_secret(&request.name, &request.description, &values(&request.entries), "")?;
    if request.ask_each_time {
        factory.vault.set_ask_each_time(&id, true)?;
    }
    factory.vault_changed();
    Ok(())
}

/// SetSecretAskEachTime: on at once; off only with the vault's proof, since
/// it lets agents' commands use the secret without asking.
pub(crate) async fn set_ask_each_time(
    factory: &Arc<Factory>,
    session: &mut Session,
    request: &v1::SetSecretAskEachTime,
) -> Result<v1::VaultSession, RequestError> {
    if !request.ask_each_time {
        open(factory, request.proof.as_ref()).await?;
    }
    factory.vault.set_ask_each_time(&request.id, request.ask_each_time)?;
    factory.vault_changed();
    Ok(session.answer())
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
    factory.known_agent(agent)?;
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
    /// Which device it is for checks bound to one (CheckRecoveryKey): the
    /// paired device's id, "local" for this machine's own app.
    binding: String,
}

impl Session {
    pub(crate) fn new(device: String, binding: String) -> Session {
        Session { device, binding }
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
    answer_waiting(factory, &request.id, &request.agent_id)?;
    Ok(session.answer())
}

/// The person gave `agent` a secret from Settings → Secrets (#358, K1):
/// a request of its that waits for that secret (by its id or name, or for
/// variables it sets) is answered as if they had given it on the card,
/// which closes the card and wakes the agent. With others waiting and none
/// of them for it, the agent is woken and told, so it can withdraw any it
/// no longer needs; nothing is closed for it.
fn answer_waiting(factory: &Arc<Factory>, secret: &str, agent: &str) -> Result<(), RequestError> {
    let state = factory.vault.state(|_, _| false);
    let Some(given) = state.secrets.iter().find(|found| found.id == secret || found.name == secret) else { return Ok(()) };
    let waiting: Vec<v1::SecretRequest> = factory
        .vault
        .requests_of(agent)?
        .into_iter()
        .filter(|asked| matches!(asked.state(), v1::SecretRequestState::Pending | v1::SecretRequestState::Approved))
        .filter(|asked| asked.imports.is_empty() && asked.generate.is_empty())
        .collect();
    let sets: Vec<&str> = given.variables.iter().map(|variable| variable.name.as_str()).collect();
    let (answered, others): (Vec<_>, Vec<_>) = waiting.into_iter().partition(|asked| {
        let wanted: Vec<&str> =
            asked.new_secret.iter().flat_map(|wanted| wanted.variables.iter().map(|variable| variable.name.as_str())).collect();
        asked.secret_id == given.id
            || asked.secret_name.trim().eq_ignore_ascii_case(given.name.trim())
            || (!wanted.is_empty() && wanted.iter().all(|name| sets.contains(name)))
    });
    for asked in &answered {
        settle_request(factory, asked, Some((given.id.clone(), given.name.clone())), "", false)?;
    }
    if answered.is_empty() && !others.is_empty() {
        let person = factory.person_capitalized();
        let label = factory.vault.granted(agent).remove(&given.id).unwrap_or_else(|| given.name.clone());
        let names: Vec<String> = others.iter().map(|asked| format!("\"{}\"", asked.secret_name)).collect();
        let summary = format!("{person} gave you {}: pass it in bash's `secrets`", given.name);
        let text = format!(
            "{summary}.\n\nIt's {label}. They gave it in Settings, not on your request for {}, which still waits: if this covers it, withdraw it with secrets_request_cancel.",
            names.join(", ")
        );
        factory.told_secret(agent, &given.id);
        factory.deliver_event(agent, text, host_event(summary));
    }
    Ok(())
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

/// Starts checking that the person saved the recovery key, on this device.
pub(crate) fn check_recovery_key(factory: &Factory, session: &Session) -> Result<v1::RecoveryKeyCheck, RequestError> {
    Ok(factory.vault.check_recovery_key(&session.binding)?)
}

/// The group the check asked for, as typed; right, every device stops
/// asking (VaultState.recovery_key_unconfirmed).
pub(crate) fn confirm_recovery_key(factory: &Factory, session: &Session, request: &v1::ConfirmRecoveryKey) -> Result<(), RequestError> {
    factory.vault.confirm_recovery_key(&session.binding, &request.check_id, &request.typed)?;
    factory.vault_changed();
    Ok(())
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

/// The admin agent that takes approved requests: Rivet when it's an admin
/// with Secrets on, else the first such admin in the settings order; never
/// the asker itself (no agent may end up granting to itself), and None
/// when there's no other.
pub(crate) fn secrets_admin(config: &crate::config::Config, asker: &str) -> Option<String> {
    let qualifies = |agent: &&crate::config::AgentConfig| {
        agent.id != asker && crate::proposals::is_admin(agent) && crate::extensions::enabled(agent, crate::extensions::secrets::ID)
    };
    let agents = &config.agents;
    agents
        .iter()
        .find(|agent| agent.id == "rivet" && qualifies(agent))
        .or_else(|| agents.iter().find(qualifies))
        .map(|agent| agent.id.clone())
}

/// The person approved the need: the request passes to the admin agent,
/// which finds a matching secret or collects a new one. The admin is
/// re-picked here in case the one chosen at request time is gone.
fn approve_to_admin(factory: &Arc<Factory>, asked: &v1::SecretRequest, comment: &str) -> Result<(), RequestError> {
    let admin = {
        let inner = factory.lock();
        let stored = inner
            .config
            .agent(&asked.admin_agent_id)
            .filter(|agent| {
                agent.id != asked.agent_id
                    && crate::proposals::is_admin(agent)
                    && crate::extensions::enabled(agent, crate::extensions::secrets::ID)
            })
            .map(|agent| agent.id.clone());
        stored.or_else(|| secrets_admin(&inner.config, &asked.agent_id))
    };
    let Some(admin) = admin else {
        return Err(VaultError::Invalid("No admin agent can take this request: give a secret or add one directly.".to_owned()).into());
    };
    let approved = factory.vault.approve_request(&asked.agent_id, &asked.id, &admin, comment)?;
    factory.vault_changed();
    show_everywhere(factory, approved.clone());
    let (person, asker_name) = {
        let inner = factory.lock();
        let asker_name = inner.config.agent(&asked.agent_id).map_or_else(|| asked.agent_id.clone(), |agent| agent.name.clone());
        (inner.config.person_capitalized().to_owned(), asker_name)
    };
    let wanted = approved.new_secret.clone().unwrap_or_default();
    let variables: Vec<&str> = wanted.variables.iter().map(|variable| variable.name.as_str()).collect();
    let variables = if variables.is_empty() { "none named".to_owned() } else { variables.join(", ") };
    let summary = format!("{person} approved {asker_name}'s request for {}", approved.secret_name);
    let mut text = format!(
        "{summary}: find it one.\n\nIt needs: {}\nVariables it expects: {variables}\nWhy it asked: {}",
        if wanted.description.is_empty() { approved.secret_name.as_str() } else { wanted.description.as_str() },
        approved.reason
    );
    text.push_str(&note(&person, comment));
    text.push_str(&format!(
        "\n\nGive it a matching secret from the vault: secrets_list to look, then secrets_grant with request: \"{id}\" (and aliases if its variable names differ). When none fits, secrets_collect with request: \"{id}\" asks {person} to add one: the card shows in your conversation, prefilled, and they type the values straight into the vault; you never see them. You'll be told how it ends either way.",
        id = approved.id
    ));
    let event = factory_agent::Event {
        source: factory_agent::EventSource::Host,
        from: "Claktory".to_owned(),
        id: String::new(),
        summary: format!("{summary}."),
        transient: false,
        pipe_seqs: Vec::new(),
        provenance: None,
    };
    factory.deliver_event(&admin, text, event);
    Ok(())
}

/// The person gives an agent what it asked for (a new secret made from
/// `entries`, or an existing one), or dismisses the request; its entry
/// shows the decision, and the agent is told. With `approve_only`, they
/// approve the need instead, and the admin agent takes it from there.
pub(crate) fn decide_request(factory: &Arc<Factory>, request: &v1::DecideSecretRequest) -> Result<(), RequestError> {
    let agent = request.agent_id.as_str();
    factory.known_agent(agent)?;
    crate::decisions::comment(&request.comment)?;
    let asked = factory.vault.pending_request(agent, &request.request_id).inspect_err(|error| {
        // It closed meanwhile: the card that asked shows how.
        if matches!(error, VaultError::Invalid(_)) {
            reshow(factory, &request.request_id);
        }
    })?;
    // A reply (#370 Submit): nothing is given; the agent gets the person's
    // words, and asks again with them in mind if it still needs it.
    if request.reply {
        if request.comment.trim().is_empty() {
            return Err(VaultError::Invalid("Write a reply, or give a secret or dismiss the request.".to_owned()).into());
        }
        return settle_request(factory, &asked, None, &request.comment, true);
    }
    if !asked.imports.is_empty() && request.approve {
        if request.approve_only {
            return Err(
                VaultError::Invalid("Importing files is yours to approve: there's nothing for an admin agent to find.".to_owned()).into()
            );
        }
        let imported = import(factory, &asked, request);
        if imported.is_err() {
            // The card shows what's wrong now.
            refresh_imports(factory);
        }
        return imported;
    }
    if !asked.generate.is_empty() && request.approve {
        if request.approve_only {
            return Err(VaultError::Invalid(
                "What the vault generates is yours to approve: there's nothing for an admin agent to find.".to_owned(),
            )
            .into());
        }
        return generate(factory, &asked, request);
    }
    if request.approve && request.approve_only {
        if asked.state() == v1::SecretRequestState::Approved {
            return Err(VaultError::Invalid("It's approved already; the admin agent is on it.".to_owned()).into());
        }
        return approve_to_admin(factory, &asked, &request.comment);
    }
    let mut added = false;
    let given = if request.approve {
        let mut aliases = std::collections::BTreeMap::new();
        let secret = if !request.secret_id.is_empty() {
            aliases = aliases_for(factory, &asked, request)?;
            request.secret_id.clone()
        } else if !request.entries.is_empty() {
            let wanted = asked.new_secret.clone().unwrap_or_default();
            let pick = |chosen: &str, asked: &str| if chosen.trim().is_empty() { asked.to_owned() } else { chosen.to_owned() };
            let name = pick(&request.name, if wanted.name.is_empty() { &asked.secret_name } else { &wanted.name });
            let description = pick(&request.description, &wanted.description);
            added = true;
            factory.vault.create_secret(&name, &description, &values(&request.entries), "")?
        } else if !asked.secret_id.is_empty() {
            asked.secret_id.clone()
        } else {
            return Err(VaultError::Invalid("Choose a secret to give, or add a new one with its values.".to_owned()).into());
        };
        factory.known_agent(agent)?;
        let name = factory.vault.grant_as(&secret, agent, "", &aliases)?;
        Some((secret, name))
    } else {
        None
    };
    if added {
        factory.vault.mark_added(&asked.id)?;
    }
    settle_request(factory, &asked, given, &request.comment, false)
}

/// The person gave `asked`'s agent `given` (the secret's id and name) for
/// it, or dismissed it, or replied to it (`replied`, #370: nothing given):
/// its entry shows the decision, an admin agent that was working it hears,
/// and the agent is told: quietly when dismissed with nothing more.
fn settle_request(
    factory: &Arc<Factory>,
    asked: &v1::SecretRequest,
    given: Option<(String, String)>,
    comment: &str,
    replied: bool,
) -> Result<(), RequestError> {
    let agent = asked.agent_id.as_str();
    let replied = replied && given.is_none();
    let decided = factory.vault.decide_request(
        agent,
        &asked.id,
        given.as_ref().map(|(id, name)| (id.as_str(), name.as_str())),
        comment,
        replied,
    )?;
    factory.vault_changed();
    // Its entry shows the decision, the admin agent's card too.
    show_everywhere(factory, decided);
    // An admin agent that was working it hears the person handled it.
    if asked.state() == v1::SecretRequestState::Approved && !asked.admin_agent_id.is_empty() {
        let (person, asker_name) = {
            let inner = factory.lock();
            let asker_name = inner.config.agent(agent).map_or_else(|| agent.to_owned(), |found| found.name.clone());
            (inner.config.person_capitalized().to_owned(), asker_name)
        };
        let summary = match &given {
            Some((_, name)) => format!("{person} gave {asker_name} {name} for its request themselves; nothing more to do"),
            None if replied => {
                format!("{person} replied to {asker_name}'s request for {} themselves; nothing more to do", asked.secret_name)
            }
            None => format!("{person} dismissed {asker_name}'s request for {} themselves; nothing more to do", asked.secret_name),
        };
        let event = factory_agent::Event {
            source: factory_agent::EventSource::Host,
            from: "Claktory".to_owned(),
            id: String::new(),
            summary: format!("{summary}."),
            transient: true,
            pipe_seqs: Vec::new(),
            provenance: None,
        };
        factory.deliver_event(&asked.admin_agent_id, format!("{summary}."), event);
    }
    let person = factory.person_capitalized();
    // What it got, and nothing else of the vault.
    let (mut summary, what) = match &given {
        Some((id, name)) => {
            let label = factory.vault.granted(agent).remove(id).unwrap_or_else(|| name.clone());
            // This answer says it; the note of what changed needn't too.
            factory.told_secret(agent, id);
            (format!("{person} gave you {name}: pass it in bash's `secrets`"), format!("\n\nIt's {label}."))
        }
        None if replied => (format!("{person} replied to your request for {}", asked.secret_name), String::new()),
        None => (format!("{person} dismissed your request for {}", asked.secret_name), String::new()),
    };
    summary.push('.');
    // A reply counts as answered: the item isn't asked about again.
    let about = crate::extensions::checklist::card_decided(factory, &Card::Secret(asked.id.clone()), given.is_none() && !replied);
    // Each note says whose it is (#253).
    let text = if replied {
        format!(
            "{person} replied to your request for {}; nothing was given. They say:\n\n{}\n\nIf you still need it, ask again with that in mind.\n\nYou asked because: {}{about}",
            asked.secret_name,
            comment.trim(),
            asked.reason
        )
    } else {
        format!("{summary}{what}{}\n\nYou asked because: {}{about}", note(&person, comment), asked.reason)
    };
    // Dismissed with nothing more, it's told quietly, at the agent's next
    // wake (friend bug #19): often it was settled elsewhere.
    let bare = given.is_none() && !replied && comment.trim().is_empty();
    let event = if bare { crate::needs_you::dismissed_event(summary) } else { host_event(summary) };
    factory.deliver_event(agent, text, event);
    Ok(())
}

/// The secret `secret_ref` names (id or name) and the other names it's
/// given under for `asked`: `given`'s (trimmed), none (`as_is`, or the
/// secret sets every name asked for), or the one missing name paired with
/// the one unused variable. Any other mismatch is the caller's to settle.
fn alias_plan(
    factory: &Factory,
    asked: &v1::SecretRequest,
    secret_ref: &str,
    given: &std::collections::BTreeMap<String, String>,
    as_is: bool,
) -> Result<(String, std::collections::BTreeMap<String, String>), RequestError> {
    let state = factory.vault.state(|_, _| false);
    let Some(secret) = state.secrets.iter().find(|secret| secret.id == secret_ref || secret.name == secret_ref) else {
        return Err(VaultError::NoSecret(secret_ref.to_owned()).into());
    };
    if !given.is_empty() {
        return Ok((
            secret.id.clone(),
            given.iter().map(|(alias, variable)| (alias.trim().to_owned(), variable.trim().to_owned())).collect(),
        ));
    }
    if as_is {
        return Ok((secret.id.clone(), std::collections::BTreeMap::new()));
    }
    let has: Vec<&str> = secret.variables.iter().map(|variable| variable.name.as_str()).collect();
    let wanted: Vec<&str> =
        asked.new_secret.iter().flat_map(|wanted| wanted.variables.iter().map(|variable| variable.name.as_str())).collect();
    let missing: Vec<&str> = wanted.iter().copied().filter(|name| !has.contains(name)).collect();
    let unused: Vec<&str> = has.iter().copied().filter(|name| !wanted.contains(name)).collect();
    match (missing.as_slice(), unused.as_slice()) {
        ([], _) => Ok((secret.id.clone(), std::collections::BTreeMap::new())),
        ([alias], [variable]) => Ok((secret.id.clone(), std::collections::BTreeMap::from([((*alias).to_owned(), (*variable).to_owned())]))),
        _ => Err(VaultError::Invalid(format!(
            "{} sets {}, not {}: choose which of its variables each name is, or give it as it is.",
            secret.name,
            has.join(", "),
            missing.join(", ")
        ))
        .into()),
    }
}

/// The other names an existing secret given for `asked` is given under; the
/// person's pick resolved as `alias_plan` says.
fn aliases_for(
    factory: &Factory,
    asked: &v1::SecretRequest,
    request: &v1::DecideSecretRequest,
) -> Result<std::collections::BTreeMap<String, String>, RequestError> {
    alias_plan(factory, asked, &request.secret_id, &request.aliases, request.as_is).map(|(_, aliases)| aliases)
}

/// An admin agent gives an approved request an existing secret: the grant
/// goes to the asker, with the aliases the request needs, the request is
/// granted, and the asker is told. Returns the secret's name and the
/// asker's id, for the admin's answer.
pub(crate) fn fulfil_request(
    factory: &Arc<Factory>,
    admin: &str,
    request_id: &str,
    secret_ref: &str,
    aliases: &std::collections::BTreeMap<String, String>,
    as_is: bool,
    admin_note: &str,
) -> Result<(String, String), RequestError> {
    let asked = factory.vault.approved_request(request_id)?;
    let agent = asked.agent_id.clone();
    if agent == admin {
        return Err(VaultError::Invalid("You can't give a secret to yourself; ask the person.".to_owned()).into());
    }
    factory.known_agent(&agent)?;
    let (secret_id, aliases) = alias_plan(factory, &asked, secret_ref, aliases, as_is)?;
    let name = factory.vault.grant_as(&secret_id, &agent, admin, &aliases)?;
    // The person's approval comment stays with the decided request.
    let decided = factory.vault.decide_request(&agent, &asked.id, Some((secret_id.as_str(), name.as_str())), &asked.comment, false)?;
    factory.vault_changed();
    show_everywhere(factory, decided);
    let (person, admin_name) = {
        let inner = factory.lock();
        let admin_name = inner.config.agent(admin).map_or_else(|| admin.to_owned(), |found| found.name.clone());
        (inner.config.person_capitalized().to_owned(), admin_name)
    };
    let label = factory.vault.granted(&agent).remove(&secret_id).unwrap_or_else(|| name.clone());
    let summary = format!("{person} approved your request and {admin_name} gave you {name}: pass it in bash's `secrets`");
    // The person's approval comment and the admin agent's note, each as its writer's.
    let notes = format!("{}{}", note(&person, &asked.comment), note(&admin_name, admin_note));
    let about = crate::extensions::checklist::card_decided(factory, &Card::Secret(asked.id.clone()), false);
    let text = format!("{summary}.\n\nIt's {label}.{notes}\n\nYou asked because: {}{about}", asked.reason);
    let event = factory_agent::Event {
        source: factory_agent::EventSource::Host,
        from: "Claktory".to_owned(),
        id: String::new(),
        summary: format!("{summary}."),
        transient: false,
        pipe_seqs: Vec::new(),
        provenance: None,
    };
    factory.told_secret(&agent, &secret_id);
    factory.deliver_event(&agent, text, event);
    Ok((name, agent))
}

/// The person approves a generate request (#252): the vault makes the key
/// or password, seals it as a new secret (asking each time only if they
/// turned that on; an older app that doesn't say gets the same default),
/// hides it from every output at once, and gives it to the asker, or to no
/// one as it asked. The asker is told a key's public half, or what the
/// password is made of; never the secret itself.
fn generate(factory: &Arc<Factory>, asked: &v1::SecretRequest, request: &v1::DecideSecretRequest) -> Result<(), RequestError> {
    let agent = asked.agent_id.as_str();
    let wanted = asked.new_secret.clone().unwrap_or_default();
    let name = match request.name.trim() {
        "" => asked.secret_name.clone(),
        name => name.to_owned(),
    };
    let password = asked.generate == super::generate::PASSWORD;
    let (value, public_key, spec, variable) = if password {
        let Some(asked_spec) = &asked.password else {
            return Err(
                VaultError::Invalid("This password's request no longer says what to make; ask the agent to ask again.".to_owned()).into()
            );
        };
        // The length the person chose, checked against the bounds again.
        let spec = super::generate::password_spec(&super::generate::password_ask(asked_spec), request.password_length)?;
        let variable = wanted
            .variables
            .first()
            .cloned()
            .unwrap_or_else(|| v1::SecretVariable { name: super::generate::PASSWORD_VARIABLE.to_owned(), ..Default::default() });
        (super::generate::make_password(&spec)?, String::new(), Some(spec), variable)
    } else {
        let variable = wanted.variables.first().map_or_else(|| super::generate::VARIABLE.to_owned(), |variable| variable.name.clone());
        let made = super::generate::make(&asked.generate, name.trim())?;
        (made.private, made.public, None, v1::SecretVariable { name: variable, file: true, ..Default::default() })
    };
    let kind = super::generate::called(&asked.generate, spec.as_ref());
    let made_of = match &spec {
        Some(_) => format!("Generated by the vault: {kind}."),
        None => format!("Public key: {public_key}"),
    };
    let description = match wanted.description.trim() {
        "" => made_of,
        description => format!("{description}\n\n{made_of}"),
    };
    let values = [Value { variable: variable.name.clone(), value, file: variable.file, binary: false }];
    let id = factory.vault.create_secret(&name, &description, &values, "")?;
    drop(values);
    let asks = request.ask_each_time.unwrap_or(false);
    if asks {
        factory.vault.set_ask_each_time(&id, true)?;
    }
    // Hidden from every output from now on, before anyone can use it.
    factory.vault_changed();
    let name = factory.vault.state(|_, _| false).secrets.iter().find(|secret| secret.id == id).map_or(name, |secret| secret.name.clone());
    let given = if asked.grant_to_nobody { name.clone() } else { factory.vault.grant_as(&id, agent, "", &Default::default())? };
    let decided =
        factory.vault.decide_generated(asked, (&id, &given), (&public_key, spec.as_ref()), asked.grant_to_nobody, &request.comment)?;
    factory.vault_changed();
    show_everywhere(factory, decided);
    let person = factory.person_capitalized();
    let variable = variable.name;
    let (summary, how) = if asked.grant_to_nobody {
        (format!("{person} approved your request: the vault generated {name}, {kind}, and gave it to no one, as you asked"), String::new())
    } else {
        // Mid-sentence: "the person" when they have no name, never "The person".
        let them = factory.lock().config.person().to_owned();
        let asking = if asks {
            format!(
                " It asks {them} each time it's used: a command with it isn't run at first, and they're asked to allow that exact command once; when you're told they allowed it, run it again exactly as it was."
            )
        } else {
            String::new()
        };
        let holds = if password { "holds the password" } else { "is its private key's file" };
        let never = if password { "the password" } else { "the key" };
        (
            format!("{person} approved your request: the vault generated {given}, {kind}, and gave it to you: pass it in bash's `secrets`"),
            format!("\n\n{variable} {holds}; you never see {never} itself.{asking}"),
        )
    };
    let public = if password { String::new() } else { format!("\n\nIts public key:\n{public_key}") };
    let about = crate::extensions::checklist::card_decided(factory, &Card::Secret(asked.id.clone()), false);
    let text = format!("{summary}.{how}{public}{}\n\nYou asked because: {}{about}", note(&person, &request.comment), asked.reason);
    // This answer says it; the note of what changed needn't too.
    if !asked.grant_to_nobody {
        factory.told_secret(agent, &id);
    }
    factory.deliver_event(agent, text, host_event(format!("{summary}.")));
    Ok(())
}

/// The person approves an import: each file is read again, held to being
/// the same file, unchanged, and sealed straight into a new secret (a file
/// variable each; binary as it is), its kind "import" so its values can't
/// be edited, given to the asker only if the person chose that, then each
/// original deleted if they asked and it's still the same file. The asker
/// is told what was stored, never the contents; the secret's log keeps
/// what was imported.
fn import(factory: &Arc<Factory>, asked: &v1::SecretRequest, request: &v1::DecideSecretRequest) -> Result<(), RequestError> {
    let agent = asked.agent_id.as_str();
    // The agent asked for it to go to no one: the card offers Import only.
    if asked.grant_to_nobody && request.grant_to_asker {
        return Err(VaultError::Invalid(format!("{} asked for this to go to no one; import it only.", asked.secret_name)).into());
    }
    let kept = factory.vault.wanted(&asked.id)?;
    let homes = super::import::homes(factory.home.root());
    let opened = factory.vault.approved_imports(&kept.imports, &homes)?;
    let name = match request.name.trim() {
        "" => asked.secret_name.clone(),
        name => name.to_owned(),
    };
    let values: Vec<Value> = kept
        .imports
        .iter()
        .zip(&opened)
        .map(|(file, opened)| {
            let (value, binary) = super::import::value(&opened.bytes);
            Value { variable: file.variable.clone(), value, file: true, binary }
        })
        .collect();
    drop(opened);
    let lines: Vec<String> = kept
        .imports
        .iter()
        .zip(&values)
        .map(|(file, value)| {
            let kind = if value.binary { "binary file" } else { "text file" };
            format!("{}: {} ({kind}, {} bytes, fingerprint {})", file.variable, file.path, file.size, super::import::shown(&file.keyed))
        })
        .collect();
    let wanted = asked.new_secret.clone().unwrap_or_default();
    let made_of = format!("Imported from this computer:\n{}", lines.join("\n"));
    let description = match wanted.description.trim() {
        "" => made_of,
        description => format!("{description}\n\n{made_of}"),
    };
    let id = factory.vault.create_secret(&name, &description, &values, "")?;
    drop(values);
    factory.vault.mark_imported(&id)?;
    let asks = request.ask_each_time.unwrap_or(false);
    if asks {
        factory.vault.set_ask_each_time(&id, true)?;
    }
    // Hidden from every output from now on, before anyone can use it.
    factory.vault_changed();
    let name = factory.vault.state(|_, _| false).secrets.iter().find(|secret| secret.id == id).map_or(name, |secret| secret.name.clone());
    let nobody = !request.grant_to_asker;
    let given = if nobody { name.clone() } else { factory.vault.grant_as(&id, agent, "", &Default::default())? };
    let decided = factory.vault.decide_generated(asked, (&id, &given), ("", None), nobody, &request.comment)?;
    factory.vault_changed();
    show_everywhere(factory, decided);
    // Only once it's in the vault, and only the same file.
    let mut deleted = Vec::new();
    if request.delete_originals {
        for file in &kept.imports {
            if super::import::delete_original(file) {
                deleted.push(file.path.clone());
            } else {
                tracing::warn!(path = %file.path, "an imported file's original wasn't deleted: it isn't the same file any more");
            }
        }
    }
    let gave = if nobody { "no one".to_owned() } else { agent.to_owned() };
    let originals = if !request.delete_originals {
        "originals kept".to_owned()
    } else if deleted.len() == kept.imports.len() {
        "originals deleted".to_owned()
    } else {
        format!("originals deleted: {}", if deleted.is_empty() { "none".to_owned() } else { deleted.join(", ") })
    };
    factory.vault.log_import(&id, agent, &format!("{}; given to {gave}; {originals}", lines.join("; ")))?;
    let person = factory.person_capitalized();
    let summary = if nobody {
        format!("{person} approved your request: the vault imported {name} and gave it to no one")
    } else {
        format!("{person} approved your request: the vault imported {given} and gave it to you: pass it in bash's `secrets`")
    };
    let how = if nobody {
        ""
    } else {
        " Each variable holds the path of a private file with the file's contents, exactly as they were; you never see them."
    };
    let gone = if deleted.is_empty() { String::new() } else { format!("\n\nThe originals were deleted: {}.", deleted.join(", ")) };
    let text = format!(
        "{summary}.{how}\n\nWhat it holds:\n{}{gone}{}\n\nYou asked because: {}{}",
        lines.join("\n"),
        note(&person, &request.comment),
        asked.reason,
        crate::extensions::checklist::card_decided(factory, &Card::Secret(asked.id.clone()), false)
    );
    if !nobody {
        factory.told_secret(agent, &id);
    }
    factory.deliver_event(agent, text, host_event(format!("{summary}.")));
    Ok(())
}

/// Shows a request as it is now in its agent's conversation.
pub(crate) fn show(factory: &Factory, request: v1::SecretRequest) {
    let request = fitted(factory, request);
    let agent = request.agent_id.clone();
    let mut inner = factory.lock();
    let entry = inner
        .agents
        .get_mut(&agent)
        .and_then(|runtime| runtime.transcript.as_mut())
        .and_then(|transcript| transcript.set_secret_request(request));
    if let Some(entry) = entry {
        factory.send_change(&inner, &agent, crate::transcript::Change::Entry(entry));
        // Whether it waits for the person shows with the agent.
        factory.broadcast_workspace(&inner);
    }
}

/// The admin agent `id` is being deleted: the open requests it held go
/// back to the person (their admin cleared, collecting ended), so the
/// asker's card is theirs to answer directly again, and counts in Needs
/// you again. New requests pick the next admin by themselves.
pub(crate) fn forget_admin(factory: &Factory, id: &str) {
    let orphaned = match factory.vault.forget_admin(id) {
        Ok(orphaned) => orphaned,
        Err(error) => {
            tracing::error!(agent = %id, %error, "could not hand back the secret requests it held");
            return;
        }
    };
    for request in orphaned {
        show_everywhere(factory, request);
    }
}

/// Why a card closes when its asker is deleted.
const ASKER_DELETED: &str = "The agent was deleted.";

/// Closes the open request cards in `transcript` that `closes` picks, as
/// withdrawn: their requests are gone with the agent that asked (deleting
/// an agent deletes its requests). The entries that changed.
fn close_shown(transcript: &mut crate::transcript::Transcript, closes: impl Fn(&v1::SecretRequest) -> bool) -> Vec<v1::Entry> {
    let open: Vec<v1::SecretRequest> = transcript
        .in_memory()
        .filter_map(|entry| match &entry.kind {
            Some(v1::entry::Kind::SecretRequest(request))
                if matches!(request.state(), v1::SecretRequestState::Pending | v1::SecretRequestState::Approved) && closes(request) =>
            {
                Some(request.clone())
            }
            _ => None,
        })
        .collect();
    let now = crate::transcript::now_ms();
    open.into_iter()
        .filter_map(|mut request| {
            request.set_state(v1::SecretRequestState::Withdrawn);
            request.collecting = false;
            request.decided_ms = now;
            request.comment = ASKER_DELETED.to_owned();
            transcript.set_secret_request(request)
        })
        .collect()
}

/// The agent `asker` was deleted, and its requests with it: the cards for
/// them in other agents' conversations (an admin agent's add-secret card)
/// close, rather than waiting in Needs you for good.
pub(crate) fn close_cards_in(factory: &Factory, inner: &mut crate::factory::Inner, asker: &str) {
    let mut changed = Vec::new();
    for (owner, runtime) in &mut inner.agents {
        if let Some(transcript) = runtime.transcript.as_mut() {
            changed.extend(close_shown(transcript, |request| request.agent_id == asker).into_iter().map(|entry| (owner.clone(), entry)));
        }
    }
    if changed.is_empty() {
        return;
    }
    for (owner, entry) in changed {
        factory.send_change(inner, &owner, crate::transcript::Change::Entry(entry));
    }
    factory.broadcast_workspace(inner);
}

/// Closes the cards a just-loaded conversation shows open for requests the
/// vault no longer has (their agent was deleted); nothing while the vault
/// can't be read.
pub(crate) fn close_missing(factory: &Factory, transcript: &mut crate::transcript::Transcript) {
    let gone = |request: &v1::SecretRequest| matches!(factory.vault.find_request(&request.id), Ok(None));
    close_shown(transcript, gone);
}

/// A card asked for something of request `id` that it can't do any more
/// (it closed meanwhile): shows the request as it is now wherever it's
/// shown, so a card that missed the change catches up and leaves Needs you.
pub(crate) fn reshow(factory: &Factory, id: &str) {
    match factory.vault.find_request(id) {
        Ok(Some(request)) => show_everywhere(factory, request),
        Ok(None) => {}
        Err(error) => tracing::warn!(request = %id, %error, "could not read a secret request to show it again"),
    }
}

/// `request` with the vault's secret that fits it (SecretRequest's
/// fitting_secret_id, #360 K4), while it waits; decided, none.
pub(crate) fn fitted(factory: &Factory, mut request: v1::SecretRequest) -> v1::SecretRequest {
    let open = matches!(request.state(), v1::SecretRequestState::Pending | v1::SecretRequestState::Approved);
    let (id, name) = if open && request.imports.is_empty() && request.generate.is_empty() {
        fitting(&factory.vault.state(|_, _| false).secrets, &request).unwrap_or_default()
    } else {
        Default::default()
    };
    request.fitting_secret_id = id;
    request.fitting_secret_name = name;
    request
}

/// The secret in `secrets` that fits `asked`: the one it named, else one
/// named what it asks for (ignoring case), else one that sets every
/// variable it asks for; else a connected service whose connector or name
/// it names, which an admin agent makes one from. Its id and name.
fn fitting(secrets: &[v1::SecretInfo], asked: &v1::SecretRequest) -> Option<(String, String)> {
    let wanted: Vec<&str> =
        asked.new_secret.iter().flat_map(|wanted| wanted.variables.iter().map(|variable| variable.name.as_str())).collect();
    let plain = || secrets.iter().filter(|secret| secret.connection.is_none());
    let found = plain()
        .find(|secret| !asked.secret_id.is_empty() && secret.id == asked.secret_id)
        .or_else(|| plain().find(|secret| secret.name.trim().eq_ignore_ascii_case(asked.secret_name.trim())))
        .or_else(|| {
            (!wanted.is_empty())
                .then(|| plain().find(|secret| wanted.iter().all(|name| secret.variables.iter().any(|variable| variable.name == *name))))
                .flatten()
        })
        .or_else(|| {
            let words = format!("{} {}", asked.secret_name, asked.new_secret.as_ref().map_or("", |wanted| wanted.description.as_str()))
                .to_lowercase();
            secrets.iter().find(|secret| {
                secret.connection.as_ref().is_some_and(|connection| {
                    let connector = connection.connector_id.to_lowercase();
                    (!connector.is_empty() && words.contains(&connector)) || words.contains(&secret.name.to_lowercase())
                })
            })
        })?;
    Some((found.id.clone(), found.name.clone()))
}

/// The vault changed: each request waiting shows the secret that fits it
/// now, where that changed.
pub(crate) fn refresh_fitting(factory: &Factory) {
    let Ok(waiting) = factory.vault.waiting_requests() else { return };
    for (asked, _) in waiting {
        let now = fitted(factory, asked);
        let shown = {
            let inner = factory.lock();
            inner.agents.get(&now.agent_id).and_then(|runtime| runtime.transcript.as_ref()).and_then(|transcript| {
                transcript.in_memory().rev().find_map(|entry| match &entry.kind {
                    Some(v1::entry::Kind::SecretRequest(shown)) if shown.id == now.id => Some(shown.fitting_secret_id.clone()),
                    _ => None,
                })
            })
        };
        if shown.is_some_and(|shown| shown != now.fitting_secret_id) {
            show(factory, now);
        }
    }
}

/// SecretRequestDone: the person put a generated key where it goes (K6);
/// the agent is told, once.
pub(crate) fn key_in_place(factory: &Arc<Factory>, request: &v1::SecretRequestDone) -> Result<(), RequestError> {
    let agent = request.agent_id.as_str();
    factory.known_agent(agent)?;
    let Some(done) = factory.vault.mark_done(agent, &request.request_id)? else { return Ok(()) };
    let place = if done.place.is_empty() { "where you asked".to_owned() } else { format!("to {}", done.place) };
    show_everywhere(factory, done.clone());
    let person = factory.person_capitalized();
    let name = if done.given_secret_name.is_empty() { done.secret_name.clone() } else { done.given_secret_name.clone() };
    let summary = format!("{person} added the public key of {name} {place}: it's in place");
    factory.deliver_event(agent, format!("{summary}."), host_event(summary));
    Ok(())
}

/// Shows a request in its asker's conversation and, when an admin agent's
/// conversation shows its card (secrets_collect), there too.
pub(crate) fn show_everywhere(factory: &Factory, request: v1::SecretRequest) {
    let admin = request.admin_agent_id.clone();
    let asker = request.agent_id.clone();
    show(factory, request.clone());
    if admin.is_empty() || admin == asker {
        return;
    }
    let mut inner = factory.lock();
    let entry = inner
        .agents
        .get_mut(&admin)
        .and_then(|runtime| runtime.transcript.as_mut())
        .and_then(|transcript| transcript.set_secret_request(request));
    if let Some(entry) = entry {
        factory.send_change(&inner, &admin, crate::transcript::Change::Entry(entry));
        factory.broadcast_workspace(&inner);
    }
}

/// A request left waiting this long since it was made or last changed
/// expires, however much its agent works meanwhile: the person may not open
/// its conversation for hours.
const EXPIRE_AFTER_MS: i64 = 12 * 60 * 60 * 1000;
/// How often waiting requests are checked.
const CHECK_EVERY: std::time::Duration = std::time::Duration::from_secs(60);

/// Whether a request made or last changed at `since` has expired at `now`.
fn expired(since: i64, now: i64) -> bool {
    now - since >= EXPIRE_AFTER_MS
}

/// How often the files waiting imports name are looked at again.
const IMPORTS_EVERY: std::time::Duration = std::time::Duration::from_secs(5);

/// Checks waiting requests every minute, and the files of waiting imports
/// every few seconds.
pub(crate) async fn monitor(factory: std::sync::Weak<Factory>) {
    let mut since_expiry = std::time::Duration::ZERO;
    // Files can be on a mount that hangs: looked at off this task, one look
    // at a time, so expiry never waits on them.
    let looking = Arc::new(std::sync::atomic::AtomicBool::new(false));
    loop {
        tokio::time::sleep(IMPORTS_EVERY).await;
        let Some(factory) = factory.upgrade() else { return };
        if !looking.swap(true, std::sync::atomic::Ordering::AcqRel) {
            let (factory, looking) = (factory.clone(), looking.clone());
            tokio::task::spawn_blocking(move || {
                refresh_imports(&factory);
                looking.store(false, std::sync::atomic::Ordering::Release);
            });
        }
        since_expiry += IMPORTS_EVERY;
        if since_expiry >= CHECK_EVERY {
            since_expiry = std::time::Duration::ZERO;
            expire_requests(&factory, crate::transcript::now_ms());
        }
    }
}

/// Shows each waiting import's card again when what's wrong with its files
/// changed (one deleted, replaced or edited since it was asked for), so the
/// card says so, and Import is off, before the person tries it.
pub(crate) fn refresh_imports(factory: &Arc<Factory>) {
    let Ok(waiting) = factory.vault.waiting_requests() else { return };
    for (asked, _) in waiting {
        if asked.imports.is_empty() || asked.state() != v1::SecretRequestState::Pending {
            continue;
        }
        let problems = |request: &v1::SecretRequest| request.imports.iter().map(|file| file.problem.clone()).collect::<Vec<_>>();
        let shown = {
            let inner = factory.lock();
            inner.agents.get(&asked.agent_id).and_then(|runtime| runtime.transcript.as_ref()).and_then(|transcript| {
                transcript.in_memory().rev().find_map(|entry| match &entry.kind {
                    Some(v1::entry::Kind::SecretRequest(shown)) if shown.id == asked.id => Some(problems(shown)),
                    _ => None,
                })
            })
        };
        if shown.is_some_and(|shown| shown != problems(&asked)) {
            show(factory, asked);
        }
    }
}

/// Expires the requests that have waited too long by `now`: each shows
/// Expired in its conversation, and its agent is told, so it can ask again.
pub(crate) fn expire_requests(factory: &Arc<Factory>, now: i64) {
    let waiting = match factory.vault.waiting_requests() {
        Ok(waiting) => waiting,
        Err(error) => {
            tracing::debug!(%error, "could not read the secret requests waiting");
            return;
        }
    };
    for (asked, since) in waiting {
        if !expired(since, now) {
            continue;
        }
        let expired = match factory.vault.expire_request(&asked.agent_id, &asked.id) {
            Ok(Some(expired)) => expired,
            Ok(None) => continue,
            Err(error) => {
                tracing::warn!(%error, request = %asked.id, "could not expire a secret request");
                continue;
            }
        };
        tracing::info!(request = %asked.id, agent = %asked.agent_id, "a secret request expired");
        factory.vault_changed();
        let agent = asked.agent_id.as_str();
        show_everywhere(factory, expired);
        if factory.lock().config.agent(agent).is_none() {
            continue;
        }
        let summary = format!("Your request for {} expired", asked.secret_name);
        let text = format!(
            "{summary}: it waited 12 hours without an answer, so it can't be answered any more. If you still need it, ask again with secrets_request.\n\nYou asked because: {}",
            asked.reason
        );
        let event = host_event(format!("{summary}."));
        factory.deliver_event(agent, text, event);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requests_expire_after_12_hours() {
        let hour = 3_600_000;
        assert!(!expired(0, 11 * hour));
        assert!(expired(0, 12 * hour));
    }
}
