//! Agents' requests for secrets (`secrets_request`): an existing secret by
//! name, or a new one described (name, description, variables, never
//! values). Each shows in the agent's conversation after the call, and in
//! the vault's state while it waits; the person gives it or dismisses it
//! (`requests::decide`), and the agent is told.

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
        _ => SecretRequestState::Pending,
    }
}

const COLUMNS: &str = "id, agent, secret, secret_name, new_secret, reason, state, decided, comment";

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
    })
}

/// The requests waiting for the person, oldest first.
pub(super) fn pending(db: &Connection) -> Result<Vec<v1::SecretRequest>, VaultError> {
    let mut statement = db.prepare(&format!("SELECT {COLUMNS} FROM requests WHERE state = 'pending' ORDER BY created, id"))?;
    let requests = statement.query_map([], row)?.collect::<Result<Vec<_>, _>>()?;
    Ok(requests)
}

impl Vault {
    /// Records `agent`'s request for `secret` (an existing one, by name) or
    /// for a new one, `wanted`; exactly one of them. Errors say what to do.
    pub(crate) fn ask(
        &self,
        agent: &str,
        secret: Option<&str>,
        wanted: Option<Wanted>,
        reason: &str,
    ) -> Result<v1::SecretRequest, VaultError> {
        let reason = reason.trim();
        if reason.is_empty() {
            return Err(VaultError::Invalid("Say why you need it, in reason: the person reads it.".to_owned()));
        }
        if reason.chars().count() > MAX_REASON {
            return Err(VaultError::Invalid(format!("Keep reason under {MAX_REASON} characters.")));
        }
        let mut inner = self.lock();
        let db = Self::db(&inner)?;
        let (secret_id, secret_name, new_secret) = match (secret.map(str::trim).filter(|name| !name.is_empty()), wanted) {
            (Some(name), None) => {
                let found: Option<(String, String)> = db
                    .query_row("SELECT id, name FROM secrets WHERE name = ?1", [name], |row| Ok((row.get(0)?, row.get(1)?)))
                    .optional()?;
                let Some((id, name)) = found else {
                    return Err(VaultError::Invalid(format!(
                        "There's no secret named \"{name}\". To ask for a new one, describe it in `new` (name, description, variables) instead."
                    )));
                };
                let granted: bool =
                    db.query_row("SELECT COUNT(*) > 0 FROM grants WHERE secret = ?1 AND agent = ?2", [&id, agent], |row| row.get(0))?;
                if granted {
                    return Err(VaultError::Invalid(format!("You have {name} already: pass it in bash's `secrets`.")));
                }
                (id, name, String::new())
            }
            (None, Some(wanted)) => {
                let name = check_name(&wanted.name)?;
                let taken: Option<String> =
                    db.query_row("SELECT name FROM secrets WHERE name = ?1", [&name], |row| row.get(0)).optional()?;
                if let Some(taken) = taken {
                    return Err(VaultError::Invalid(format!("There's a secret named {taken} already: ask for it by name, in `secret`.")));
                }
                if wanted.variables.is_empty() || wanted.variables.len() > MAX_VARIABLES {
                    return Err(VaultError::Invalid(format!(
                        "Name the environment variables it sets, 1 to {MAX_VARIABLES}, such as AWS_SECRET_ACCESS_KEY."
                    )));
                }
                let mut variables = Vec::new();
                for variable in &wanted.variables {
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
                let wanted = Wanted { name: name.clone(), description: wanted.description.trim().to_owned(), variables };
                let json = serde_json::to_string(&wanted).map_err(|error| VaultError::Invalid(error.to_string()))?;
                (String::new(), name, json)
            }
            _ => {
                return Err(VaultError::Invalid(
                    "Ask for one secret: an existing one by name in `secret`, or a new one described in `new`.".to_owned(),
                ));
            }
        };
        let waiting: bool = db.query_row(
            "SELECT COUNT(*) > 0 FROM requests WHERE agent = ?1 AND secret_name = ?2 COLLATE NOCASE AND state = 'pending'",
            [agent, &secret_name],
            |row| row.get(0),
        )?;
        if waiting {
            return Err(VaultError::Invalid(format!("You asked for {secret_name} already; it's waiting for the person.")));
        }
        let id = format!("r{}", crypto::hex(&crypto::random::<4>()));
        db.execute(
            "INSERT INTO requests (id, agent, secret, secret_name, new_secret, reason, state, created) VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'pending', ?7)",
            params![id, agent, secret_id, secret_name, new_secret, reason, now_ms()],
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
            Some(request) if request.state() != SecretRequestState::Pending => {
                Err(VaultError::Invalid("This request was decided already.".to_owned()))
            }
            Some(request) => Ok(request),
        }
    }

    /// Marks the request given or dismissed; returns it as it is now.
    pub(crate) fn decide_request(&self, agent: &str, id: &str, granted: bool, comment: &str) -> Result<v1::SecretRequest, VaultError> {
        let mut inner = self.lock();
        let db = Self::db(&inner)?;
        let state = if granted { "granted" } else { "dismissed" };
        let changed = db.execute(
            "UPDATE requests SET state = ?3, decided = ?4, comment = ?5 WHERE id = ?1 AND agent = ?2 AND state = 'pending'",
            params![id, agent, state, now_ms(), comment.trim()],
        )?;
        if changed == 0 {
            return Err(VaultError::Invalid("This request was decided already.".to_owned()));
        }
        let request = db.query_row(&format!("SELECT {COLUMNS} FROM requests WHERE id = ?1"), [id], row)?;
        inner.state = None;
        Ok(request)
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
