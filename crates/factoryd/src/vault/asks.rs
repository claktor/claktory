//! Agents' requests for secrets (`secrets_request`). An agent doesn't know
//! which secrets the vault holds: it describes what it needs (a name, what
//! it's for, the variables it expects, never values) and why, and is
//! answered the same whether or not the vault has such a secret. Each
//! request shows in the agent's conversation after the call, where the
//! person gives one of the vault's secrets, adds a new one from the
//! description, or dismisses it (`requests::decide_request`), and the agent
//! is told only what it got. One left waiting expires
//! (`requests::expire_requests`). Requests from before named an existing
//! secret (`secret`).

use factory_protocol::v1::{self, SecretRequestState};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};

use super::{Vault, VaultError, check_name, crypto, valid_variable};
use crate::transcript::now_ms;

/// The longest reason an agent may give.
const MAX_REASON: usize = 2000;
/// The most variables a new secret asked for may have.
const MAX_VARIABLES: usize = 20;

/// A new secret an agent asks for: what the person fills in.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub(crate) struct Wanted {
    pub name: String,
    #[serde(default)]
    pub description: String,
    pub variables: Vec<WantedVariable>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub(crate) struct WantedVariable {
    pub name: String,
    /// Given to commands as a file's path.
    #[serde(default)]
    pub file: bool,
}

fn state_of(state: &str) -> SecretRequestState {
    match state {
        "granted" => SecretRequestState::Granted,
        "dismissed" => SecretRequestState::Dismissed,
        "expired" => SecretRequestState::Expired,
        "withdrawn" => SecretRequestState::Withdrawn,
        _ => SecretRequestState::Pending,
    }
}

const COLUMNS: &str = "id, agent, secret, secret_name, new_secret, reason, state, decided, comment, given, given_name, updated";

fn row(row: &rusqlite::Row<'_>) -> rusqlite::Result<v1::SecretRequest> {
    let new_secret: String = row.get(4)?;
    let new_secret = serde_json::from_str::<Wanted>(&new_secret).ok().map(|wanted| v1::NewSecret {
        name: wanted.name,
        description: wanted.description,
        variables: wanted
            .variables
            .into_iter()
            .map(|variable| v1::SecretVariable { name: variable.name, hint: String::new(), file: variable.file })
            .collect(),
    });
    let state: String = row.get(6)?;
    Ok(v1::SecretRequest {
        id: row.get(0)?,
        agent_id: row.get(1)?,
        secret_id: row.get(2)?,
        secret_name: row.get(3)?,
        new_secret,
        reason: row.get(5)?,
        state: state_of(&state).into(),
        decided_ms: row.get(7)?,
        comment: row.get(8)?,
        given_secret_id: row.get(9)?,
        given_secret_name: row.get(10)?,
        updated_ms: row.get(11)?,
    })
}

/// The tool agents ask with.
pub(crate) const TOOL: &str = "secrets_request";

/// What a secrets_request result's details hold: the request as made.
pub(crate) fn details(request: &v1::SecretRequest) -> serde_json::Value {
    let new_secret = request.new_secret.as_ref().map(|wanted| {
        let variables: Vec<serde_json::Value> =
            wanted.variables.iter().map(|variable| serde_json::json!({"name": variable.name, "file": variable.file})).collect();
        serde_json::json!({"name": wanted.name, "description": wanted.description, "variables": variables})
    });
    serde_json::json!({
        "request": request.id,
        "agent": request.agent_id,
        "secret_id": request.secret_id,
        "secret_name": request.secret_name,
        "new_secret": new_secret,
        "reason": request.reason,
    })
}

/// The entry a secrets_request result shows (called directly, or through
/// extension_call), pending; None for any other result, or one that asked
/// for nothing.
pub(crate) fn requested(tool: &str, is_error: bool, details: &serde_json::Value) -> Option<v1::SecretRequest> {
    if !matches!(tool, TOOL | "extension_call") || is_error {
        return None;
    }
    let text = |key: &str| details.get(key).and_then(serde_json::Value::as_str).unwrap_or_default().to_owned();
    let id = details.get("request")?.as_str()?.to_owned();
    let new_secret =
        details.get("new_secret").and_then(|wanted| serde_json::from_value::<Wanted>(wanted.clone()).ok()).map(|wanted| v1::NewSecret {
            name: wanted.name,
            description: wanted.description,
            variables: wanted
                .variables
                .into_iter()
                .map(|variable| v1::SecretVariable { name: variable.name, hint: String::new(), file: variable.file })
                .collect(),
        });
    Some(v1::SecretRequest {
        id,
        agent_id: text("agent"),
        secret_id: text("secret_id"),
        secret_name: text("secret_name"),
        new_secret,
        reason: text("reason"),
        state: SecretRequestState::Pending.into(),
        decided_ms: 0,
        comment: String::new(),
        given_secret_id: String::new(),
        given_secret_name: String::new(),
        updated_ms: 0,
    })
}

/// The requests waiting for the person, oldest first, each with when it
/// was made or the agent last changed it (ms since 1970).
fn pending(db: &Connection) -> Result<Vec<(v1::SecretRequest, i64)>, VaultError> {
    let mut statement =
        db.prepare(&format!("SELECT {COLUMNS}, MAX(created, updated) FROM requests WHERE state = 'pending' ORDER BY created, id"))?;
    let requests = statement.query_map([], |found| Ok((row(found)?, found.get(12)?)))?.collect::<Result<Vec<_>, _>>()?;
    Ok(requests)
}

/// A reason as an agent gives it, checked.
fn check_reason(reason: &str) -> Result<String, VaultError> {
    let reason = reason.trim();
    if reason.is_empty() {
        return Err(VaultError::Invalid("Say why you need it, in reason: the person reads it.".to_owned()));
    }
    if reason.chars().count() > MAX_REASON {
        return Err(VaultError::Invalid(format!("Keep reason under {MAX_REASON} characters.")));
    }
    Ok(reason.to_owned())
}

fn check_description(description: &str) -> Result<String, VaultError> {
    let description = description.trim();
    if description.chars().count() > MAX_REASON {
        return Err(VaultError::Invalid(format!("Keep description under {MAX_REASON} characters.")));
    }
    Ok(description.to_owned())
}

fn check_variables(given: &[WantedVariable]) -> Result<Vec<WantedVariable>, VaultError> {
    if given.len() > MAX_VARIABLES {
        return Err(VaultError::Invalid(format!("Name {MAX_VARIABLES} variables at most.")));
    }
    let mut variables = Vec::new();
    for variable in given {
        let variable_name = variable.name.trim();
        if !valid_variable(variable_name) {
            return Err(VaultError::Invalid(format!(
                "\"{variable_name}\" isn't a variable name: letters, digits and _, not starting with a digit."
            )));
        }
        if variables.iter().any(|seen: &WantedVariable| seen.name == variable_name) {
            return Err(VaultError::Invalid(format!("{variable_name} is named twice.")));
        }
        variables.push(WantedVariable { name: variable_name.to_owned(), file: variable.file });
    }
    Ok(variables)
}

/// What an agent changes in a request of its own; None keeps it as it was.
#[derive(Default)]
pub(crate) struct Change {
    pub description: Option<String>,
    pub reason: Option<String>,
    pub variables: Option<Vec<WantedVariable>>,
}

impl Vault {
    /// Records `agent`'s request for what `wanted` describes. Whether or
    /// not the vault has such a secret, or has given it one, makes no
    /// difference to the answer: nothing of the vault shows through. Errors
    /// say what to fix.
    pub(crate) fn ask(&self, agent: &str, wanted: Wanted, reason: &str) -> Result<v1::SecretRequest, VaultError> {
        let reason = check_reason(reason)?;
        let name = check_name(&wanted.name).map_err(|_| {
            VaultError::Invalid(
                "Give what you need a short name, such as \"Cloudflare token\": letters, digits, spaces and - _ . @ ( ), up to 64."
                    .to_owned(),
            )
        })?;
        let description = check_description(&wanted.description)?;
        let variables = check_variables(&wanted.variables)?;
        let wanted = Wanted { name: name.clone(), description, variables };
        let json = serde_json::to_string(&wanted).map_err(|error| VaultError::Invalid(error.to_string()))?;
        let mut inner = self.lock();
        let db = Self::db(&inner)?;
        let waiting: Option<String> = db
            .query_row(
                "SELECT id FROM requests WHERE agent = ?1 AND secret_name = ?2 COLLATE NOCASE AND state = 'pending'",
                [agent, &name],
                |row| row.get(0),
            )
            .optional()?;
        if let Some(waiting) = waiting {
            return Err(VaultError::Invalid(format!(
                "You asked for {name} already ({waiting}); it's waiting for the person. Change it with secrets_request_update, or withdraw it with secrets_request_cancel."
            )));
        }
        let id = format!("r{}", crypto::hex(&crypto::random::<4>()));
        db.execute(
            "INSERT INTO requests (id, agent, secret, secret_name, new_secret, reason, state, created) VALUES (?1, ?2, '', ?3, ?4, ?5, 'pending', ?6)",
            params![id, agent, name, json, reason, now_ms()],
        )?;
        let request = db.query_row(&format!("SELECT {COLUMNS} FROM requests WHERE id = ?1"), [&id], row)?;
        inner.state = None;
        Ok(request)
    }

    /// `agent`'s request `id`, if it's still waiting.
    pub(crate) fn pending_request(&self, agent: &str, id: &str) -> Result<v1::SecretRequest, VaultError> {
        let inner = self.lock();
        let found = Self::db(&inner)?
            .query_row(&format!("SELECT {COLUMNS} FROM requests WHERE id = ?1 AND agent = ?2"), [id, agent], row)
            .optional()?;
        match found {
            None => Err(VaultError::NoRequest(id.to_owned())),
            Some(request) if request.state() == SecretRequestState::Expired => Err(VaultError::Invalid("This request expired.".to_owned())),
            Some(request) if request.state() == SecretRequestState::Withdrawn => {
                Err(VaultError::Invalid("The agent withdrew this request.".to_owned()))
            }
            Some(request) if request.state() != SecretRequestState::Pending => {
                Err(VaultError::Invalid("This request was decided already.".to_owned()))
            }
            Some(request) => Ok(request),
        }
    }

    /// Marks the request given (the secret `given`: its id and name) or
    /// dismissed (None); returns it as it is now.
    pub(crate) fn decide_request(
        &self,
        agent: &str,
        id: &str,
        given: Option<(&str, &str)>,
        comment: &str,
    ) -> Result<v1::SecretRequest, VaultError> {
        let mut inner = self.lock();
        let db = Self::db(&inner)?;
        let state = if given.is_some() { "granted" } else { "dismissed" };
        let (given, given_name) = given.unwrap_or_default();
        let changed = db.execute(
            "UPDATE requests SET state = ?3, decided = ?4, comment = ?5, given = ?6, given_name = ?7
             WHERE id = ?1 AND agent = ?2 AND state = 'pending'",
            params![id, agent, state, now_ms(), comment.trim(), given, given_name],
        )?;
        if changed == 0 {
            return Err(VaultError::Invalid("This request was decided already.".to_owned()));
        }
        let request = db.query_row(&format!("SELECT {COLUMNS} FROM requests WHERE id = ?1"), [id], row)?;
        inner.state = None;
        Ok(request)
    }

    /// The requests waiting for the person, oldest first, each with when it
    /// was made or last changed; none while there's no vault.
    pub(crate) fn waiting_requests(&self) -> Result<Vec<(v1::SecretRequest, i64)>, VaultError> {
        let inner = self.lock();
        let Some(db) = &inner.db else { return Ok(Vec::new()) };
        pending(db)
    }

    /// Marks `agent`'s request `id` expired, if it's still waiting; returns
    /// it as it is now, or None if it was decided meanwhile.
    pub(crate) fn expire_request(&self, agent: &str, id: &str) -> Result<Option<v1::SecretRequest>, VaultError> {
        let mut inner = self.lock();
        let db = Self::db(&inner)?;
        let changed = db.execute(
            "UPDATE requests SET state = 'expired', decided = ?3 WHERE id = ?1 AND agent = ?2 AND state = 'pending'",
            params![id, agent, now_ms()],
        )?;
        if changed == 0 {
            return Ok(None);
        }
        let request = db.query_row(&format!("SELECT {COLUMNS} FROM requests WHERE id = ?1"), [id], row)?;
        inner.state = None;
        Ok(Some(request))
    }

    /// Changes `agent`'s waiting request `id`; returns it as it is now.
    pub(crate) fn update_request(&self, agent: &str, id: &str, change: Change) -> Result<v1::SecretRequest, VaultError> {
        let current = self.own_waiting(agent, id)?;
        let mut wanted: Wanted = Wanted {
            name: current.secret_name.clone(),
            ..current.new_secret.as_ref().map_or_else(Wanted::default, |wanted| Wanted {
                name: wanted.name.clone(),
                description: wanted.description.clone(),
                variables: wanted
                    .variables
                    .iter()
                    .map(|variable| WantedVariable { name: variable.name.clone(), file: variable.file })
                    .collect(),
            })
        };
        if let Some(description) = &change.description {
            wanted.description = check_description(description)?;
        }
        if let Some(variables) = &change.variables {
            wanted.variables = check_variables(variables)?;
        }
        let reason = match &change.reason {
            Some(reason) => check_reason(reason)?,
            None => current.reason.clone(),
        };
        if change.description.is_none() && change.variables.is_none() && change.reason.is_none() {
            return Err(VaultError::Invalid("Say what to change: description, reason or variables.".to_owned()));
        }
        let json = serde_json::to_string(&wanted).map_err(|error| VaultError::Invalid(error.to_string()))?;
        let mut inner = self.lock();
        let db = Self::db(&inner)?;
        let changed = db.execute(
            "UPDATE requests SET new_secret = ?3, reason = ?4, updated = ?5 WHERE id = ?1 AND agent = ?2 AND state = 'pending'",
            params![id, agent, json, reason, now_ms()],
        )?;
        if changed == 0 {
            return Err(VaultError::Invalid(format!("Your request {id} isn't waiting any more.")));
        }
        let request = db.query_row(&format!("SELECT {COLUMNS} FROM requests WHERE id = ?1"), [id], row)?;
        inner.state = None;
        Ok(request)
    }

    /// Withdraws `agent`'s waiting request `id`, with its reason (may be
    /// empty); returns it as it is now.
    pub(crate) fn withdraw_request(&self, agent: &str, id: &str, reason: &str) -> Result<v1::SecretRequest, VaultError> {
        self.own_waiting(agent, id)?;
        let reason = check_description(reason)?;
        let mut inner = self.lock();
        let db = Self::db(&inner)?;
        let changed = db.execute(
            "UPDATE requests SET state = 'withdrawn', decided = ?3, comment = ?4 WHERE id = ?1 AND agent = ?2 AND state = 'pending'",
            params![id, agent, now_ms(), reason],
        )?;
        if changed == 0 {
            return Err(VaultError::Invalid(format!("Your request {id} isn't waiting any more.")));
        }
        let request = db.query_row(&format!("SELECT {COLUMNS} FROM requests WHERE id = ?1"), [id], row)?;
        inner.state = None;
        Ok(request)
    }

    /// `agent`'s own request `id`, if it's waiting; or why not, for the agent.
    fn own_waiting(&self, agent: &str, id: &str) -> Result<v1::SecretRequest, VaultError> {
        let inner = self.lock();
        let found = Self::db(&inner)?
            .query_row(&format!("SELECT {COLUMNS} FROM requests WHERE id = ?1 AND agent = ?2"), [id.trim(), agent], row)
            .optional()?;
        match found {
            None => Err(VaultError::Invalid(format!("You have no request {}. secrets_requests lists yours.", id.trim()))),
            Some(request) if request.state() != SecretRequestState::Pending => {
                Err(VaultError::Invalid(format!("Your request {} isn't waiting any more.", request.id)))
            }
            Some(request) => Ok(request),
        }
    }

    /// `agent`'s waiting requests and its latest `decided` others, newest
    /// first.
    pub(crate) fn recent_requests(&self, agent: &str, decided: u32) -> Result<Vec<v1::SecretRequest>, VaultError> {
        let inner = self.lock();
        let Some(db) = &inner.db else { return Ok(Vec::new()) };
        let mut statement = db.prepare(&format!(
            "SELECT {COLUMNS} FROM requests WHERE agent = ?1 AND state = 'pending'
             UNION ALL SELECT * FROM (SELECT {COLUMNS} FROM requests WHERE agent = ?1 AND state != 'pending' ORDER BY decided DESC LIMIT ?2)"
        ))?;
        let requests = statement.query_map(params![agent, decided], row)?.collect::<Result<Vec<_>, _>>()?;
        Ok(requests)
    }

    /// Every request of `agent`'s, to show its conversation as it is.
    pub(crate) fn requests_of(&self, agent: &str) -> Result<Vec<v1::SecretRequest>, VaultError> {
        let inner = self.lock();
        let Some(db) = &inner.db else { return Ok(Vec::new()) };
        let mut statement = db.prepare(&format!("SELECT {COLUMNS} FROM requests WHERE agent = ?1 ORDER BY created"))?;
        let requests = statement.query_map([agent], row)?.collect::<Result<Vec<_>, _>>()?;
        Ok(requests)
    }
}
