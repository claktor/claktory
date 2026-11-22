//! The app's vault requests (Settings → Secrets).

use std::sync::Arc;

use factory_protocol::v1;

use super::{Key, Value, Vault, VaultError, env};
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
    if !comment.trim().is_empty() {
        text.push_str(&format!("\n{person} added: {}", comment.trim()));
    }
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
    let asked = factory.vault.pending_request(agent, &request.request_id)?;
    if !asked.generate.is_empty() && request.approve {
        if request.approve_only {
            return Err(VaultError::Invalid(
                "A key to generate is yours to approve: there's nothing for an admin agent to find.".to_owned(),
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
    let decided =
        factory.vault.decide_request(agent, &asked.id, given.as_ref().map(|(id, name)| (id.as_str(), name.as_str())), &request.comment)?;
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
            None => format!("{person} dismissed {asker_name}'s request for {} themselves; nothing more to do", asked.secret_name),
        };
        let event = factory_agent::Event {
            source: factory_agent::EventSource::Host,
            from: "Claktory".to_owned(),
            id: String::new(),
            summary: format!("{summary}."),
            transient: true,
            pipe_seqs: Vec::new(),
        };
        factory.deliver_event(&asked.admin_agent_id, format!("{summary}."), event);
    }
    let person = factory.lock().config.person_capitalized().to_owned();
    // What it got, and nothing else of the vault.
    let (mut summary, what) = match &given {
        Some((id, name)) => {
            let label = factory.vault.granted(agent).remove(id).unwrap_or_else(|| name.clone());
            (format!("{person} gave you {name}: pass it in bash's `secrets`"), format!("\n\nIt's {label}."))
        }
        None => (format!("{person} dismissed your request for {}", asked.secret_name), String::new()),
    };
    summary.push('.');
    // Each note says whose it is (#253).
    let text = format!("{summary}{what}{}\n\nYou asked because: {}", note(&person, &request.comment), asked.reason);
    let event = host_event(summary);
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
    let decided = factory.vault.decide_request(&agent, &asked.id, Some((secret_id.as_str(), name.as_str())), &asked.comment)?;
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
    let text = format!("{summary}.\n\nIt's {label}.{notes}\n\nYou asked because: {}", asked.reason);
    let event = factory_agent::Event {
        source: factory_agent::EventSource::Host,
        from: "Claktory".to_owned(),
        id: String::new(),
        summary: format!("{summary}."),
        transient: false,
        pipe_seqs: Vec::new(),
    };
    factory.deliver_event(&agent, text, event);
    Ok((name, agent))
}

/// A note for the asker, as its writer's ("Matt's note: …", "Rivet's
/// note: …"), on a line of its own; empty without one.
fn note(writer: &str, note: &str) -> String {
    let note = note.trim();
    if note.is_empty() { String::new() } else { format!("\n\n{writer}'s note: {note}") }
}

/// The person approves a generate request (#252): the vault makes the key,
/// seals its private half as a new secret (asking each time unless they
/// said not to), hides it from every output at once, and gives it to the
/// asker, or to no one as it asked. The asker is told its public half only.
fn generate(factory: &Arc<Factory>, asked: &v1::SecretRequest, request: &v1::DecideSecretRequest) -> Result<(), RequestError> {
    let agent = asked.agent_id.as_str();
    let wanted = asked.new_secret.clone().unwrap_or_default();
    let name = match request.name.trim() {
        "" => asked.secret_name.clone(),
        name => name.to_owned(),
    };
    let variable = wanted.variables.first().map_or_else(|| super::generate::VARIABLE.to_owned(), |variable| variable.name.clone());
    let made = super::generate::make(&asked.generate, name.trim())?;
    let public_key = made.public.clone();
    let description = match wanted.description.trim() {
        "" => format!("Public key: {public_key}"),
        description => format!("{description}\n\nPublic key: {public_key}"),
    };
    let values = [Value { variable: variable.clone(), value: made.private, file: true }];
    let id = factory.vault.create_secret(&name, &description, &values, "")?;
    drop(values);
    let asks = request.ask_each_time.unwrap_or(true);
    if asks {
        factory.vault.set_ask_each_time(&id, true)?;
    }
    // Hidden from every output from now on, before anyone can use it.
    factory.vault_changed();
    let name = factory.vault.state(|_, _| false).secrets.iter().find(|secret| secret.id == id).map_or(name, |secret| secret.name.clone());
    let given = if asked.grant_to_nobody { name.clone() } else { factory.vault.grant_as(&id, agent, "", &Default::default())? };
    let decided = factory.vault.decide_generated(asked, (&id, &given), &public_key, &request.comment)?;
    factory.vault_changed();
    show_everywhere(factory, decided);
    let person = factory.lock().config.person_capitalized().to_owned();
    let kind = super::generate::called(&asked.generate);
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
        (
            format!("{person} approved your request: the vault generated {given}, {kind}, and gave it to you: pass it in bash's `secrets`"),
            format!("\n\n{variable} is its private key's file; you never see the key itself.{asking}"),
        )
    };
    let text = format!(
        "{summary}.{how}\n\nIts public key:\n{public_key}{}\n\nYou asked because: {}",
        note(&person, &request.comment),
        asked.reason
    );
    factory.deliver_event(agent, text, host_event(format!("{summary}.")));
    Ok(())
}

/// Shows a request as it is now in its agent's conversation.
pub(crate) fn show(factory: &Factory, request: v1::SecretRequest) {
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

/// Checks waiting requests every minute.
pub(crate) async fn monitor(factory: std::sync::Weak<Factory>) {
    loop {
        tokio::time::sleep(CHECK_EVERY).await;
        let Some(factory) = factory.upgrade() else { return };
        expire_requests(&factory, crate::transcript::now_ms());
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
        show(factory, expired);
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
