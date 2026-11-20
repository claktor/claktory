//! Connected services in the vault (docs/CONNECTORS.md). A connection is a
//! secret of kind `connection`: its tokens are the connector's own JSON (a
//! control), sealed with the secret's key apart from any variable, so
//! nothing that resolves, reveals or lists variables ever reads them. It has
//! no variables and can't be granted. Secrets made from it (kind `export`)
//! are ordinary secrets with variables, plus their connection (`parent`)
//! and what they reach (`meta`). Agents' proposals to connect a service
//! are kept here too, like their requests.

use factory_protocol::v1::{self, ConnectProposalState, ConnectionStatus};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

use super::{Value, Vault, VaultError, aad, crypto};
use crate::connectors::{Control, ExportMeta};
use crate::transcript::now_ms;

pub(super) const KIND_CONNECTION: &str = "connection";
pub(super) const KIND_EXPORT: &str = "export";

/// What a connection reaches, as its connector read it: what the app and
/// admin agents see, never a token.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ConnectionMeta {
    /// The account's stable id at the service, to tell a reconnect to
    /// another account.
    #[serde(default)]
    pub subject: String,
    #[serde(default)]
    pub account: String,
    #[serde(default)]
    pub access: Vec<String>,
    #[serde(default)]
    pub scopes: Vec<String>,
    #[serde(default)]
    pub resources: Vec<String>,
    #[serde(default)]
    pub expires_ms: i64,
    #[serde(default)]
    pub checked_ms: i64,
    /// Why it needs connecting again; empty while it works.
    #[serde(default)]
    pub problem: String,
    #[serde(default)]
    pub proposal_id: String,
    /// The access asked for (a ConnectorAccess id).
    #[serde(default)]
    pub access_id: String,
}

impl ConnectionMeta {
    pub(crate) fn to_proto(&self, connector: &str) -> v1::SecretConnection {
        let status = if self.problem.is_empty() { ConnectionStatus::Ready } else { ConnectionStatus::NeedsReconnect };
        v1::SecretConnection {
            connector_id: connector.to_owned(),
            account: self.account.clone(),
            access: self.access.clone(),
            scopes: self.scopes.clone(),
            resources: self.resources.clone(),
            status: status.into(),
            problem: self.problem.clone(),
            expires_ms: self.expires_ms,
            checked_ms: self.checked_ms,
            proposal_id: self.proposal_id.clone(),
        }
    }
}

/// A connection as the broker reads it.
pub(crate) struct Opened {
    pub id: String,
    pub name: String,
    pub connector: String,
    pub meta: ConnectionMeta,
    pub control: Control,
}

/// A secret made from a connection.
#[derive(Clone, Debug)]
pub(crate) struct Export {
    pub id: String,
    pub name: String,
    pub connection: String,
    pub meta: ExportMeta,
}

fn control_aad(vault_id: &str, id: &str, connector: &str, generation: i64) -> Vec<u8> {
    aad(&["control", vault_id, id, connector, &generation.to_string()])
}

/// The secret's own key.
fn dek(db: &Connection, vault_id: &str, vmk: &[u8; 32], id: &str) -> Result<crypto::Key, VaultError> {
    let (version, sealed): (u32, Vec<u8>) =
        db.query_row("SELECT version, sealed_dek FROM secrets WHERE id = ?1", [id], |row| Ok((row.get(0)?, row.get(1)?)))?;
    crypto::open_key(vmk, &sealed, &aad(&["dek", vault_id, id, &version.to_string()]))
        .ok_or_else(|| VaultError::Invalid(format!("The secret {id} can't be opened: the vault was changed outside Claktory.")))
}

fn write_control(db: &Connection, vault_id: &str, vmk: &[u8; 32], id: &str, connector: &str, control: &Control) -> Result<(), VaultError> {
    let generation: i64 = db.query_row("SELECT COALESCE(MAX(version), 0) + 1 FROM controls WHERE secret = ?1", [id], |row| row.get(0))?;
    let key = dek(db, vault_id, vmk, id)?;
    let sealed = crypto::seal(&key, control.as_str().as_bytes(), &control_aad(vault_id, id, connector, generation));
    db.execute("INSERT OR REPLACE INTO controls (secret, version, sealed) VALUES (?1, ?2, ?3)", params![id, generation, sealed])?;
    Ok(())
}

fn json<T: Serialize>(value: &T) -> Result<String, VaultError> {
    serde_json::to_string(value).map_err(|error| VaultError::Invalid(error.to_string()))
}

/// A name not taken yet: `base`, else "base 2", "base 3"…
fn free_name(db: &Connection, base: &str) -> Result<String, VaultError> {
    let base = super::check_name(base).or_else(|_| super::check_name(&clean_name(base)))?;
    for n in 1.. {
        let name = if n == 1 { base.clone() } else { format!("{base} {n}") };
        let taken: bool = db.query_row("SELECT COUNT(*) > 0 FROM secrets WHERE name = ?1", [&name], |row| row.get(0))?;
        if !taken {
            return Ok(name);
        }
    }
    unreachable!()
}

/// A secret's name made of anything: what isn't allowed becomes a space.
pub(crate) fn clean_name(text: &str) -> String {
    let cleaned: String = text
        .chars()
        .map(|c| if c.is_alphanumeric() || matches!(c, ' ' | '-' | '_' | '.' | '@' | '(' | ')') { c } else { ' ' })
        .take(64)
        .collect();
    let words: Vec<&str> = cleaned.split_whitespace().collect();
    let name = words.join(" ");
    if name.is_empty() { "Connection".to_owned() } else { name }
}

impl Vault {
    /// Keeps a newly connected service; returns its id and name, made from
    /// `name` (" 2" if it's taken).
    pub(crate) fn add_connection(
        &self,
        name: &str,
        connector: &str,
        meta: &ConnectionMeta,
        control: &Control,
    ) -> Result<(String, String), VaultError> {
        let mut inner = self.lock();
        let vmk = Zeroizing::new(*Self::open_vmk(&inner)?);
        let vault_id = inner.vault_id.clone();
        let Some(db) = inner.db.as_mut() else { return Err(VaultError::NoVault) };
        let transaction = db.transaction()?;
        let name = free_name(&transaction, name)?;
        let id = format!("s{}", crypto::hex(&crypto::random::<6>()));
        let sealed_dek = Self::write_values(&transaction, &vault_id, &vmk, &id, 1, &[])?;
        let now = now_ms();
        transaction.execute(
            "INSERT INTO secrets (id, name, description, version, sealed_dek, owner, created, updated, kind, connector, meta)
             VALUES (?1, ?2, '', 1, ?3, '', ?4, ?4, ?5, ?6, ?7)",
            params![id, name, sealed_dek, now, KIND_CONNECTION, connector, json(meta)?],
        )?;
        write_control(&transaction, &vault_id, &vmk, &id, connector, control)?;
        transaction.commit()?;
        inner.state = None;
        Ok((id, name))
    }

    /// A connection (by id or name) with its control, for the broker only.
    pub(crate) fn connection(&self, id: &str) -> Result<Opened, VaultError> {
        let inner = self.lock();
        let vmk = Self::open_vmk(&inner)?;
        let db = Self::db(&inner)?;
        let (id, name) = Self::secret_id(db, id)?;
        let (kind, connector, meta): (String, String, String) =
            db.query_row("SELECT kind, connector, meta FROM secrets WHERE id = ?1", [&id], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?))
            })?;
        if kind != KIND_CONNECTION {
            return Err(VaultError::Invalid(format!("{name} isn't a connected service.")));
        }
        let (generation, sealed): (i64, Vec<u8>) = db
            .query_row("SELECT version, sealed FROM controls WHERE secret = ?1", [&id], |row| Ok((row.get(0)?, row.get(1)?)))
            .optional()?
            .ok_or_else(|| VaultError::Invalid(format!("{name} has lost its sign-in: connect it again.")))?;
        let key = dek(db, &inner.vault_id, vmk, &id)?;
        let opened = crypto::open(&key, &sealed, &control_aad(&inner.vault_id, &id, &connector, generation))
            .ok_or_else(|| VaultError::Invalid(format!("{name} can't be opened: the vault was changed outside Claktory.")))?;
        let text = String::from_utf8(opened.to_vec()).map_err(|_| VaultError::Invalid(format!("{name} can't be opened.")))?;
        let meta = serde_json::from_str(&meta).unwrap_or_default();
        Ok(Opened { id, name, connector, meta, control: Control::new(text) })
    }

    /// Replaces a connection's tokens (refreshed, or connected again).
    pub(crate) fn set_control(&self, id: &str, control: &Control) -> Result<(), VaultError> {
        let inner = self.lock();
        let vmk = Self::open_vmk(&inner)?;
        let db = Self::db(&inner)?;
        let connector: String =
            db.query_row("SELECT connector FROM secrets WHERE id = ?1 AND kind = ?2", [id, KIND_CONNECTION], |row| row.get(0))?;
        write_control(db, &inner.vault_id, vmk, id, &connector, control)
    }

    /// Replaces what a connection reaches (and with `control`, its tokens).
    pub(crate) fn set_connection(&self, id: &str, meta: &ConnectionMeta, control: Option<&Control>) -> Result<(), VaultError> {
        let mut inner = self.lock();
        let vmk = Zeroizing::new(*Self::open_vmk(&inner)?);
        let vault_id = inner.vault_id.clone();
        let Some(db) = inner.db.as_mut() else { return Err(VaultError::NoVault) };
        let transaction = db.transaction()?;
        let connector: String = transaction
            .query_row("SELECT connector FROM secrets WHERE id = ?1 AND kind = ?2", [id, KIND_CONNECTION], |row| row.get(0))
            .optional()?
            .ok_or_else(|| VaultError::NoSecret(id.to_owned()))?;
        transaction.execute("UPDATE secrets SET meta = ?2, updated = ?3 WHERE id = ?1", params![id, json(meta)?, now_ms()])?;
        if let Some(control) = control {
            write_control(&transaction, &vault_id, &vmk, id, &connector, control)?;
        }
        transaction.commit()?;
        inner.state = None;
        Ok(())
    }

    /// Adds a secret made from `connection`, given to no one; returns its id
    /// and name (" 2" if `name` is taken).
    pub(crate) fn add_export(
        &self,
        connection: &str,
        name: &str,
        description: &str,
        values: &[Value],
        meta: &ExportMeta,
    ) -> Result<(String, String), VaultError> {
        super::check_values(values)?;
        let mut inner = self.lock();
        let vmk = Zeroizing::new(*Self::open_vmk(&inner)?);
        let vault_id = inner.vault_id.clone();
        let Some(db) = inner.db.as_mut() else { return Err(VaultError::NoVault) };
        let transaction = db.transaction()?;
        let connector: String = transaction
            .query_row("SELECT connector FROM secrets WHERE id = ?1 AND kind = ?2", [connection, KIND_CONNECTION], |row| row.get(0))
            .optional()?
            .ok_or_else(|| VaultError::NoSecret(connection.to_owned()))?;
        let name = free_name(&transaction, name)?;
        let id = format!("s{}", crypto::hex(&crypto::random::<6>()));
        let sealed_dek = Self::write_values(&transaction, &vault_id, &vmk, &id, 1, values)?;
        let now = now_ms();
        transaction.execute(
            "INSERT INTO secrets (id, name, description, version, sealed_dek, owner, created, updated, kind, connector, parent, meta)
             VALUES (?1, ?2, ?3, 1, ?4, '', ?5, ?5, ?6, ?7, ?8, ?9)",
            params![id, name, description.trim(), sealed_dek, now, KIND_EXPORT, connector, connection, json(meta)?],
        )?;
        transaction.commit()?;
        inner.state = None;
        Ok((id, name))
    }

    /// Renews a secret made from a connection: new values (a new version)
    /// or the same ones, and what it reaches now.
    pub(crate) fn set_export(&self, id: &str, values: Option<&[Value]>, meta: &ExportMeta) -> Result<(), VaultError> {
        if let Some(values) = values {
            super::check_values(values)?;
        }
        let mut inner = self.lock();
        let vmk = Zeroizing::new(*Self::open_vmk(&inner)?);
        let vault_id = inner.vault_id.clone();
        let Some(db) = inner.db.as_mut() else { return Err(VaultError::NoVault) };
        let transaction = db.transaction()?;
        let version: u32 = transaction
            .query_row("SELECT version FROM secrets WHERE id = ?1 AND kind = ?2", [id, KIND_EXPORT], |row| row.get(0))
            .optional()?
            .ok_or_else(|| VaultError::NoSecret(id.to_owned()))?;
        let now = now_ms();
        transaction.execute("UPDATE secrets SET meta = ?2, updated = ?3 WHERE id = ?1", params![id, json(meta)?, now])?;
        if let Some(values) = values {
            let sealed_dek = Self::write_values(&transaction, &vault_id, &vmk, id, version + 1, values)?;
            transaction.execute("UPDATE secrets SET version = ?2, sealed_dek = ?3 WHERE id = ?1", params![id, version + 1, sealed_dek])?;
        }
        transaction.commit()?;
        inner.state = None;
        Ok(())
    }

    fn exports_where(db: &Connection, clause: &str, arg: &str) -> Result<Vec<Export>, VaultError> {
        let mut statement =
            db.prepare(&format!("SELECT id, name, parent, meta FROM secrets WHERE kind = '{KIND_EXPORT}' AND {clause} ORDER BY name"))?;
        let rows = statement
            .query_map([arg], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?, row.get::<_, String>(2)?, row.get::<_, String>(3)?))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows
            .into_iter()
            .map(|(id, name, connection, meta)| Export { id, name, connection, meta: serde_json::from_str(&meta).unwrap_or_default() })
            .collect())
    }

    /// A secret's values, unlogged: to keep hiding them once renewed.
    pub(crate) fn values(&self, id: &str) -> Result<Vec<Value>, VaultError> {
        let inner = self.lock();
        let vmk = Self::open_vmk(&inner)?;
        Self::values_of(Self::db(&inner)?, &inner.vault_id, vmk, id)
    }

    /// The secrets made from a connection.
    pub(crate) fn exports_of(&self, connection: &str) -> Result<Vec<Export>, VaultError> {
        let inner = self.lock();
        Self::exports_where(Self::db(&inner)?, "parent = ?1", connection)
    }

    /// A secret made from a connection, by id or name; None for any other.
    pub(crate) fn export(&self, id: &str) -> Result<Option<Export>, VaultError> {
        let inner = self.lock();
        let db = Self::db(&inner)?;
        let Ok((id, _)) = Self::secret_id(db, id) else { return Ok(None) };
        Ok(Self::exports_where(db, "id = ?1", &id)?.into_iter().next())
    }

    /// Of the secrets `names`, those made from a connection and granted to
    /// `agent`: the ones a command of its may need renewed first.
    pub(crate) fn granted_exports(&self, agent: &str, names: &[String]) -> Vec<Export> {
        let inner = self.lock();
        let (Ok(key), Ok(db)) = (Self::grant_key(&inner), Self::db(&inner)) else { return Vec::new() };
        let mut found = Vec::new();
        for name in names {
            let Ok(exports) = Self::exports_where(db, "name = ?1", name.trim()) else { continue };
            for export in exports {
                if Self::is_granted(&inner, &key, &export.id, agent).unwrap_or(false) {
                    found.push(export);
                }
            }
        }
        found
    }

    /// The kind of a secret: '' (the person's), `connection` or `export`.
    pub(super) fn kind_of(db: &Connection, id: &str) -> Result<String, VaultError> {
        Ok(db.query_row("SELECT kind FROM secrets WHERE id = ?1", [id], |row| row.get(0))?)
    }

    /// Fills a secret's connection or export view from its row.
    pub(super) fn connection_view(db: &Connection, secret: &mut v1::SecretInfo) -> Result<(), VaultError> {
        let (kind, connector, parent, meta): (String, String, String, String) =
            db.query_row("SELECT kind, connector, parent, meta FROM secrets WHERE id = ?1", [&secret.id], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
            })?;
        match kind.as_str() {
            KIND_CONNECTION => {
                let meta: ConnectionMeta = serde_json::from_str(&meta).unwrap_or_default();
                secret.connection = Some(meta.to_proto(&connector));
            }
            KIND_EXPORT => {
                let meta: ExportMeta = serde_json::from_str(&meta).unwrap_or_default();
                secret.export = Some(v1::SecretExport {
                    connection_id: parent,
                    access: meta.access,
                    permissions: meta.permissions,
                    resources: meta.resources,
                    expires_ms: meta.expires_ms,
                    per_use: meta.per_use,
                });
            }
            _ => {}
        }
        Ok(())
    }

    /// Removes a connection and the secrets made from it, with their
    /// grants. Returns the names removed (the connection's first) and the
    /// agents that had any.
    pub(crate) fn remove_connection(&self, id: &str) -> Result<(Vec<String>, Vec<String>), VaultError> {
        let mut inner = self.lock();
        let Some(db) = inner.db.as_mut() else { return Err(VaultError::NoVault) };
        let transaction = db.transaction()?;
        let (id, name) = Self::secret_id(&transaction, id)?;
        if Self::kind_of(&transaction, &id)? != KIND_CONNECTION {
            return Err(VaultError::Invalid(format!("{name} isn't a connected service.")));
        }
        let exports: Vec<(String, String)> = transaction
            .prepare("SELECT id, name FROM secrets WHERE parent = ?1 AND kind = ?2")?
            .query_map([&id, KIND_EXPORT], |row| Ok((row.get(0)?, row.get(1)?)))?
            .collect::<Result<_, _>>()?;
        let mut names = vec![name];
        let mut agents = Vec::new();
        for secret in std::iter::once(id.as_str()).chain(exports.iter().map(|(id, _)| id.as_str())) {
            let had: Vec<String> = transaction
                .prepare("SELECT agent FROM grants WHERE secret = ?1")?
                .query_map([secret], |row| row.get(0))?
                .collect::<Result<_, _>>()?;
            for agent in had {
                if !agents.contains(&agent) {
                    agents.push(agent);
                }
            }
            for table in ["variables", "grants", "controls"] {
                transaction.execute(&format!("DELETE FROM {table} WHERE secret = ?1"), [secret])?;
            }
            transaction.execute("DELETE FROM secrets WHERE id = ?1", [secret])?;
        }
        names.extend(exports.into_iter().map(|(_, name)| name));
        transaction.commit()?;
        inner.state = None;
        Ok((names, agents))
    }

    /// Each connection's name, connector and tokens.
    pub(crate) fn controls(&self) -> Vec<(String, String, Control)> {
        let ids: Vec<String> = {
            let inner = self.lock();
            let Ok(db) = Self::db(&inner) else { return Vec::new() };
            let Ok(mut statement) = db.prepare("SELECT id FROM secrets WHERE kind = ?1") else { return Vec::new() };
            statement.query_map([KIND_CONNECTION], |row| row.get(0)).map(|rows| rows.flatten().collect()).unwrap_or_default()
        };
        ids.into_iter().filter_map(|id| self.connection(&id).ok().map(|opened| (opened.name, opened.connector, opened.control))).collect()
    }
}

// ---- Proposals ----

const PROPOSAL_COLUMNS: &str =
    "id, agent, connector, access_id, purpose, access, reason, state, created, decided, comment, secret, secret_name";

fn proposal_state(state: &str) -> ConnectProposalState {
    match state {
        "connected" => ConnectProposalState::Connected,
        "dismissed" => ConnectProposalState::Dismissed,
        _ => ConnectProposalState::Pending,
    }
}

fn proposal_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<v1::ConnectProposal> {
    let state: String = row.get(7)?;
    Ok(v1::ConnectProposal {
        id: row.get(0)?,
        agent_id: row.get(1)?,
        connector_id: row.get(2)?,
        access_id: row.get(3)?,
        purpose: row.get(4)?,
        access: row.get(5)?,
        reason: row.get(6)?,
        state: proposal_state(&state).into(),
        created_ms: row.get(8)?,
        decided_ms: row.get(9)?,
        comment: row.get(10)?,
        secret_id: row.get(11)?,
        secret_name: row.get(12)?,
    })
}

/// The proposals waiting for the person, oldest first.
pub(super) fn pending_proposals(db: &Connection) -> Result<Vec<v1::ConnectProposal>, VaultError> {
    let mut statement =
        db.prepare(&format!("SELECT {PROPOSAL_COLUMNS} FROM connect_proposals WHERE state = 'pending' ORDER BY created, id"))?;
    let proposals = statement.query_map([], proposal_row)?.collect::<Result<Vec<_>, _>>()?;
    Ok(proposals)
}

impl Vault {
    /// Records an agent's proposal, checked by the caller. One per agent and
    /// service at a time.
    pub(crate) fn propose(&self, proposal: &v1::ConnectProposal) -> Result<v1::ConnectProposal, VaultError> {
        let mut inner = self.lock();
        let db = Self::db(&inner)?;
        let waiting: bool = db.query_row(
            "SELECT COUNT(*) > 0 FROM connect_proposals WHERE agent = ?1 AND connector = ?2 AND state = 'pending'",
            [&proposal.agent_id, &proposal.connector_id],
            |row| row.get(0),
        )?;
        if waiting {
            return Err(VaultError::Invalid("You proposed connecting this service already; it's waiting for the person.".to_owned()));
        }
        let id = format!("c{}", crypto::hex(&crypto::random::<4>()));
        db.execute(
            "INSERT INTO connect_proposals (id, agent, connector, access_id, purpose, access, reason, state, created)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 'pending', ?8)",
            params![
                id,
                proposal.agent_id,
                proposal.connector_id,
                proposal.access_id,
                proposal.purpose,
                proposal.access,
                proposal.reason,
                now_ms()
            ],
        )?;
        let proposal = db.query_row(&format!("SELECT {PROPOSAL_COLUMNS} FROM connect_proposals WHERE id = ?1"), [&id], proposal_row)?;
        inner.state = None;
        Ok(proposal)
    }

    pub(crate) fn connect_proposal(&self, id: &str) -> Result<v1::ConnectProposal, VaultError> {
        let inner = self.lock();
        Self::db(&inner)?
            .query_row(&format!("SELECT {PROPOSAL_COLUMNS} FROM connect_proposals WHERE id = ?1"), [id], proposal_row)
            .optional()?
            .ok_or_else(|| VaultError::Invalid("This suggestion no longer exists.".to_owned()))
    }

    /// Marks a pending proposal connected (the connection: its id and name)
    /// or dismissed (None); returns it as it is now.
    pub(crate) fn decide_proposal(
        &self,
        id: &str,
        connected: Option<(&str, &str)>,
        comment: &str,
    ) -> Result<v1::ConnectProposal, VaultError> {
        let mut inner = self.lock();
        let db = Self::db(&inner)?;
        let state = if connected.is_some() { "connected" } else { "dismissed" };
        let (secret, secret_name) = connected.unwrap_or_default();
        let changed = db.execute(
            "UPDATE connect_proposals SET state = ?2, decided = ?3, comment = ?4, secret = ?5, secret_name = ?6
             WHERE id = ?1 AND state = 'pending'",
            params![id, state, now_ms(), comment.trim(), secret, secret_name],
        )?;
        if changed == 0 {
            return Err(VaultError::Invalid("This suggestion was already answered.".to_owned()));
        }
        let proposal = db.query_row(&format!("SELECT {PROPOSAL_COLUMNS} FROM connect_proposals WHERE id = ?1"), [id], proposal_row)?;
        inner.state = None;
        Ok(proposal)
    }

    /// Every proposal of `agent`'s, to show its conversation as it is.
    pub(crate) fn proposals_of(&self, agent: &str) -> Result<Vec<v1::ConnectProposal>, VaultError> {
        let inner = self.lock();
        let Some(db) = &inner.db else { return Ok(Vec::new()) };
        let mut statement = db.prepare(&format!("SELECT {PROPOSAL_COLUMNS} FROM connect_proposals WHERE agent = ?1 ORDER BY created"))?;
        let proposals = statement.query_map([agent], proposal_row)?.collect::<Result<Vec<_>, _>>()?;
        Ok(proposals)
    }
}
