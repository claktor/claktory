//! Grants: which agent of which Claktory may do what in a user-level app.
//! One schema and one library for every app, with the rows in the worker's
//! own database, so an app checks a grant in the same write transaction as
//! the operation it allows ([`check`]): a revoke can't land between the two.
//!
//! A grant binds an agent (its id and incarnation) of a Claktory, under the
//! epoch of that Claktory's registration, to scopes of one app and, as the
//! app declares, to resources. Signing out, being removed or signing in
//! again changes or ends the registration at the root, so a grant made
//! under an old epoch never matches again (and is purged lazily). Anything
//! unknown (an app, a scope, a resource kind, a caller that isn't an agent)
//! is refused.
//!
//! Routes (README.md): `PUT /v1/apps/grants` from a Claktory's backend (its
//! person approved it; factoryd lets only its Rivet propose), `GET` from the
//! backend (its own grants, for its mirror) or a person's device (the
//! account's), `DELETE /v1/apps/grants/<id>` from either.

use rusqlite::{Connection, OptionalExtension, Transaction, params};
use serde::{Deserialize, Serialize};

use crate::ipc::{Actor, Caller};

/// What an app declares about its grants.
pub struct App {
    pub id: &'static str,
    pub scopes: &'static [Scope],
    /// The kinds of resource a grant may name.
    pub resource_kinds: &'static [&'static str],
    /// A grant must name resources: none would grant nothing.
    pub needs_resources: bool,
}

/// A scope, and how often a grant may use it.
pub struct Scope {
    pub name: &'static str,
    pub per_minute: u32,
    pub per_hour: u32,
}

/// The apps there are. DMs grants name the conversations they reach
/// (`conversation` resources); a DM grant without one grants nothing.
pub const APPS: &[App] = &[App {
    id: "dms",
    scopes: &[
        Scope { name: "dm.read", per_minute: 60, per_hour: 600 },
        Scope { name: "dm.draft", per_minute: 5, per_hour: 20 },
        Scope { name: "dm.send", per_minute: 10, per_hour: 60 },
        Scope { name: "dm.subscribe", per_minute: 10, per_hour: 100 },
    ],
    resource_kinds: &["conversation"],
    needs_resources: true,
}];

/// Grants an account may hold, across its Claktories and apps.
const MAX_GRANTS: i64 = 200;
const MAX_SCOPES: usize = 16;
const MAX_RESOURCES: usize = 64;
const MAX_ID: usize = 256;

pub const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS grants (
    id TEXT PRIMARY KEY,
    app TEXT NOT NULL,
    account_id TEXT NOT NULL,
    claktory_id TEXT NOT NULL,
    -- The Claktory's registration it was granted under: only that one's
    -- calls match it.
    epoch INTEGER NOT NULL,
    agent_id TEXT NOT NULL,
    incarnation TEXT NOT NULL,
    -- JSON arrays, sorted: [\"dm.read\"], [{\"kind\":…,\"id\":…}].
    scopes TEXT NOT NULL,
    resources TEXT NOT NULL,
    -- Unix milliseconds; expires 0 for never.
    granted INTEGER NOT NULL,
    expires INTEGER NOT NULL,
    UNIQUE (app, account_id, claktory_id, agent_id, incarnation)
);
CREATE INDEX IF NOT EXISTS grants_by_account ON grants (account_id, claktory_id);
-- Uses of a grant's scope in a window (a minute or an hour, by its start).
CREATE TABLE IF NOT EXISTS grant_uses (
    grant_id TEXT NOT NULL,
    scope TEXT NOT NULL,
    window_start INTEGER NOT NULL,
    span INTEGER NOT NULL,
    count INTEGER NOT NULL,
    PRIMARY KEY (grant_id, scope, span, window_start)
);
";

/// Something a grant names: a resource's kind and the app's id for it.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[serde(deny_unknown_fields)]
pub struct Resource {
    pub kind: String,
    pub id: String,
}

/// A grant as the routes show it.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Grant {
    pub id: String,
    pub app: String,
    pub claktory_id: String,
    pub agent: Actor,
    pub scopes: Vec<String>,
    pub resources: Vec<Resource>,
    pub granted_ms: i64,
    /// 0 for never.
    pub expires_ms: i64,
}

/// `PUT /v1/apps/grants`: the grant an agent of the calling Claktory gets,
/// replacing the one it had in that app.
#[derive(Deserialize, Debug)]
#[serde(deny_unknown_fields)]
pub struct Put {
    pub app: String,
    pub agent: Actor,
    pub scopes: Vec<String>,
    #[serde(default)]
    pub resources: Vec<Resource>,
    /// 0 (or left out) for never.
    #[serde(default)]
    pub expires_ms: i64,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct Grants {
    pub grants: Vec<Grant>,
}

/// Why a grant doesn't allow a call, or a route refuses.
#[derive(Debug, PartialEq, Eq)]
pub enum Refused {
    /// No grant allows it (none, another epoch or incarnation, expired, not
    /// that scope or resource, not an agent).
    NotGranted,
    /// The grant's uses ran out for now: seconds to wait.
    Quota(i64),
    /// What was asked isn't valid: words for the caller.
    Invalid(String),
    /// Not this caller's to see or change, or it doesn't exist.
    NotFound,
    Db(String),
}

impl From<rusqlite::Error> for Refused {
    fn from(error: rusqlite::Error) -> Refused {
        Refused::Db(error.to_string())
    }
}

impl Refused {
    /// The status and JSON body a route answers with.
    pub fn answer(&self) -> (u16, serde_json::Value) {
        let (status, code, message) = match self {
            Refused::NotGranted => (403, "not_granted", "This agent hasn't been given that.".to_owned()),
            Refused::Quota(_) => (429, "rate_limited", "Too many requests with this grant. Try again later.".to_owned()),
            Refused::Invalid(said) => (400, "invalid_request", said.clone()),
            Refused::NotFound => (404, "not_found", "There's no such grant.".to_owned()),
            Refused::Db(error) => {
                tracing::error!("grants: {error}");
                (503, "unavailable", "Something went wrong. Try again later.".to_owned())
            }
        };
        let mut body = serde_json::json!({"error": code, "message": message});
        if let Refused::Quota(wait) = self {
            body["retry_after"] = (*wait).into();
        }
        (status, body)
    }
}

pub fn app(id: &str) -> Option<&'static App> {
    APPS.iter().find(|app| app.id == id)
}

fn valid_name(text: &str, max: usize) -> bool {
    !text.is_empty() && text.len() <= max && text.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b':'))
}

/// Whether an agent's id and incarnation are well formed.
pub fn valid_actor(actor: &Actor) -> bool {
    valid_name(&actor.id, 64) && valid_name(&actor.incarnation, 64)
}

fn new_id() -> String {
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes).expect("the system's random source");
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn read_grant(row: &rusqlite::Row<'_>) -> rusqlite::Result<Grant> {
    let scopes: String = row.get(5)?;
    let resources: String = row.get(6)?;
    Ok(Grant {
        id: row.get(0)?,
        app: row.get(1)?,
        claktory_id: row.get(2)?,
        agent: Actor { id: row.get(3)?, incarnation: row.get(4)? },
        scopes: serde_json::from_str(&scopes).unwrap_or_default(),
        resources: serde_json::from_str(&resources).unwrap_or_default(),
        granted_ms: row.get(7)?,
        expires_ms: row.get(8)?,
    })
}

const COLUMNS: &str = "id, app, claktory_id, agent_id, incarnation, scopes, resources, granted, expires";

/// Makes or replaces the grant `put` asks for, from a Claktory's backend.
pub fn put(db: &mut Connection, caller: &Caller, put: Put, now_ms: i64) -> Result<Grant, Refused> {
    let Caller::Backend { account_id, claktory_id, epoch, .. } = caller else {
        return Err(Refused::Invalid("Only a Claktory gives its agents grants.".to_owned()));
    };
    let app = app(&put.app).ok_or_else(|| Refused::Invalid(format!("There's no app \"{}\".", put.app)))?;
    if !valid_actor(&put.agent) {
        return Err(Refused::Invalid("The agent's id or incarnation isn't valid.".to_owned()));
    }
    let mut scopes = put.scopes;
    scopes.sort();
    scopes.dedup();
    if scopes.is_empty() || scopes.len() > MAX_SCOPES {
        return Err(Refused::Invalid(format!("A grant has 1 to {MAX_SCOPES} scopes.")));
    }
    if let Some(unknown) = scopes.iter().find(|scope| !app.scopes.iter().any(|known| known.name == scope.as_str())) {
        return Err(Refused::Invalid(format!("{} has no scope \"{unknown}\".", app.id)));
    }
    let mut resources = put.resources;
    resources.sort();
    resources.dedup();
    if resources.len() > MAX_RESOURCES {
        return Err(Refused::Invalid(format!("A grant names {MAX_RESOURCES} resources at most.")));
    }
    if app.needs_resources && resources.is_empty() {
        return Err(Refused::Invalid(format!("A grant in {} names what it reaches.", app.id)));
    }
    if let Some(bad) =
        resources.iter().find(|resource| !app.resource_kinds.contains(&resource.kind.as_str()) || !valid_name(&resource.id, MAX_ID))
    {
        return Err(Refused::Invalid(format!("{} has no resources such as \"{}\".", app.id, bad.kind)));
    }
    if put.expires_ms != 0 && put.expires_ms <= now_ms {
        return Err(Refused::Invalid("A grant can't expire in the past.".to_owned()));
    }
    let transaction = db.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    // The agent's grant before (of any epoch) goes, with its uses.
    let replaced: Option<String> = transaction
        .query_row(
            "SELECT id FROM grants WHERE app = ?1 AND account_id = ?2 AND claktory_id = ?3 AND agent_id = ?4 AND incarnation = ?5",
            params![app.id, account_id, claktory_id, put.agent.id, put.agent.incarnation],
            |row| row.get(0),
        )
        .optional()?;
    if let Some(replaced) = replaced {
        forget(&transaction, &replaced)?;
    }
    let held: i64 = transaction.query_row("SELECT COUNT(*) FROM grants WHERE account_id = ?1", [account_id], |row| row.get(0))?;
    if held >= MAX_GRANTS {
        return Err(Refused::Invalid(format!("An account holds {MAX_GRANTS} grants at most. Remove some first.")));
    }
    let grant = Grant {
        id: new_id(),
        app: app.id.to_owned(),
        claktory_id: claktory_id.clone(),
        agent: put.agent,
        scopes,
        resources,
        granted_ms: now_ms,
        expires_ms: put.expires_ms,
    };
    transaction.execute(
        "INSERT INTO grants (id, app, account_id, claktory_id, epoch, agent_id, incarnation, scopes, resources, granted, expires)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
        params![
            grant.id,
            grant.app,
            account_id,
            claktory_id,
            *epoch as i64,
            grant.agent.id,
            grant.agent.incarnation,
            serde_json::to_string(&grant.scopes).unwrap_or_default(),
            serde_json::to_string(&grant.resources).unwrap_or_default(),
            grant.granted_ms,
            grant.expires_ms,
        ],
    )?;
    regroup(&transaction, &grant.app, account_id, now_ms)?;
    transaction.commit()?;
    Ok(grant)
}

/// A DM grant made or ended changes which conversations the account's
/// Claktories reach: their feed says so at once (`groups`), so a Claktory
/// joins, or leaves, without waiting for its next round.
fn regroup(transaction: &Transaction<'_>, app: &str, account_id: &str, now_ms: i64) -> rusqlite::Result<()> {
    if app == "dms" {
        crate::dms::sync::emit(transaction, account_id, crate::dms::sync::Kind::Groups, "", "", 0, now_ms)?;
    }
    Ok(())
}

/// The grants a caller may see, live ones only: a backend its own, at its
/// epoch; a device or the account page its account's, of Claktories signed
/// in now. Dead ones
/// it would have seen (another epoch, expired) are purged on the way.
pub fn list(db: &mut Connection, caller: &Caller, now_ms: i64) -> Result<Vec<Grant>, Refused> {
    let transaction = db.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    let live: Vec<(String, u64)> = match caller {
        Caller::Backend { claktory_id, epoch, .. } => vec![(claktory_id.clone(), *epoch)],
        Caller::Device { registrations, .. } | Caller::Account { registrations, .. } => {
            registrations.iter().map(|registration| (registration.claktory_id.clone(), registration.epoch)).collect()
        }
    };
    let mut grants = Vec::new();
    let mut dead = Vec::new();
    {
        let scope = match caller {
            Caller::Backend { claktory_id, .. } => Some(claktory_id.as_str()),
            Caller::Device { .. } | Caller::Account { .. } => None,
        };
        let mut statement = transaction.prepare(&format!(
            "SELECT {COLUMNS}, epoch FROM grants WHERE account_id = ?1 AND (?2 IS NULL OR claktory_id = ?2) ORDER BY granted, id"
        ))?;
        let rows = statement.query_map(params![caller.account_id(), scope], |row| Ok((read_grant(row)?, row.get::<_, i64>(9)?)))?;
        for row in rows {
            let (grant, epoch) = row?;
            let expired = grant.expires_ms != 0 && grant.expires_ms <= now_ms;
            if !expired && live.iter().any(|(claktory, live)| *claktory == grant.claktory_id && *live as i64 == epoch) {
                grants.push(grant);
            } else {
                dead.push(grant.id);
            }
        }
    }
    for id in dead {
        forget(&transaction, &id)?;
    }
    transaction.commit()?;
    Ok(grants)
}

/// The live grants of `app` that a Claktory's agents hold under its
/// registration `epoch`: what its own leaf may reach (DMs end to end, whose
/// conversations a Claktory's leaf is in). Reads only.
pub fn of_claktory(
    db: &Connection,
    account_id: &str,
    claktory_id: &str,
    epoch: u64,
    app: &str,
    now_ms: i64,
) -> rusqlite::Result<Vec<Grant>> {
    let mut statement = db.prepare(&format!(
        "SELECT {COLUMNS} FROM grants WHERE app = ?1 AND account_id = ?2 AND claktory_id = ?3 AND epoch = ?4
         AND (expires = 0 OR expires > ?5) ORDER BY granted, id"
    ))?;
    statement.query_map(params![app, account_id, claktory_id, epoch as i64, now_ms], read_grant)?.collect()
}

fn forget(transaction: &Transaction<'_>, id: &str) -> rusqlite::Result<()> {
    transaction.execute("DELETE FROM grant_uses WHERE grant_id = ?1", [id])?;
    transaction.execute("DELETE FROM grants WHERE id = ?1", [id])?;
    Ok(())
}

/// Revokes a grant: a device or the account page any of its account's, a
/// backend any of its own Claktory's.
pub fn revoke(db: &mut Connection, caller: &Caller, id: &str, now_ms: i64) -> Result<(), Refused> {
    let transaction = db.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    let owner: Option<(String, String, String)> = transaction
        .query_row("SELECT account_id, claktory_id, app FROM grants WHERE id = ?1", [id], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
        .optional()?;
    let theirs = match (&owner, caller) {
        (Some((account, _, _)), Caller::Device { account_id, .. } | Caller::Account { account_id, .. }) => account == account_id,
        (Some((account, claktory, _)), Caller::Backend { account_id, claktory_id, .. }) => account == account_id && claktory == claktory_id,
        (None, _) => false,
    };
    let Some((account, _, app)) = owner.filter(|_| theirs) else {
        return Err(Refused::NotFound);
    };
    forget(&transaction, id)?;
    regroup(&transaction, &app, &account, now_ms)?;
    transaction.commit()?;
    Ok(())
}

/// Checks, inside the app's own write transaction, that the calling agent
/// holds a live grant of `app` with `scope` (and `resource`, when the call
/// is about one), and counts the use against its quota. The grant's id.
pub fn check(
    transaction: &Transaction<'_>,
    caller: &Caller,
    app: &str,
    scope: &str,
    resource: Option<&Resource>,
    now_ms: i64,
) -> Result<String, Refused> {
    let (id, spec) = holds(transaction, caller, app, scope, resource, now_ms)?;
    count(transaction, &id, spec, now_ms)?;
    Ok(id)
}

/// The same, counting nothing: for a use already counted, such as sending
/// a message the person approved, counted when the agent proposed it.
pub fn live(
    transaction: &Connection,
    caller: &Caller,
    app: &str,
    scope: &str,
    resource: Option<&Resource>,
    now_ms: i64,
) -> Result<String, Refused> {
    Ok(holds(transaction, caller, app, scope, resource, now_ms)?.0)
}

fn holds(
    transaction: &Connection,
    caller: &Caller,
    app: &str,
    scope: &str,
    resource: Option<&Resource>,
    now_ms: i64,
) -> Result<(String, &'static Scope), Refused> {
    let Caller::Backend { account_id, claktory_id, epoch, agent: Some(agent) } = caller else {
        return Err(Refused::NotGranted);
    };
    let Some(spec) = self::app(app).and_then(|app| app.scopes.iter().find(|known| known.name == scope)) else {
        return Err(Refused::NotGranted);
    };
    let found: Option<(Grant, i64)> = transaction
        .query_row(
            &format!(
                "SELECT {COLUMNS}, epoch FROM grants
                 WHERE app = ?1 AND account_id = ?2 AND claktory_id = ?3 AND agent_id = ?4 AND incarnation = ?5"
            ),
            params![app, account_id, claktory_id, agent.id, agent.incarnation],
            |row| Ok((read_grant(row)?, row.get(9)?)),
        )
        .optional()?;
    let Some((grant, granted_epoch)) = found else { return Err(Refused::NotGranted) };
    let live = granted_epoch == *epoch as i64 && (grant.expires_ms == 0 || grant.expires_ms > now_ms);
    let reaches = match resource {
        Some(resource) => grant.resources.contains(resource),
        None => true,
    };
    if !live || !grant.scopes.iter().any(|granted| granted == scope) || !reaches {
        return Err(Refused::NotGranted);
    }
    Ok((grant.id, spec))
}

/// Counts a use of `spec` by the grant `id`, refusing past its quota.
fn count(transaction: &Transaction<'_>, id: &str, spec: &Scope, now_ms: i64) -> Result<(), Refused> {
    let scope = spec.name;
    let now = now_ms / 1000;
    for (span, limit) in [(60, spec.per_minute), (3600, spec.per_hour)] {
        let start = now - now % span;
        let used: i64 = transaction
            .query_row(
                "SELECT count FROM grant_uses WHERE grant_id = ?1 AND scope = ?2 AND span = ?3 AND window_start = ?4",
                params![id, scope, span, start],
                |row| row.get(0),
            )
            .optional()?
            .unwrap_or(0);
        if used >= i64::from(limit) {
            return Err(Refused::Quota(start + span - now));
        }
    }
    for span in [60, 3600] {
        let start = now - now % span;
        transaction.execute(
            "INSERT INTO grant_uses (grant_id, scope, window_start, span, count) VALUES (?1, ?2, ?3, ?4, 1)
             ON CONFLICT (grant_id, scope, span, window_start) DO UPDATE SET count = count + 1",
            params![id, scope, start, span],
        )?;
    }
    transaction.execute("DELETE FROM grant_uses WHERE grant_id = ?1 AND window_start < ?2", params![id, now - 7200])?;
    Ok(())
}

/// Takes `resource` out of every grant of `app` the `accounts` hold (a
/// conversation that ended: a friend removed or blocked), in the caller's
/// transaction. A grant left naming nothing, in an app whose grants need
/// resources, goes. How many grants changed.
pub fn drop_resource(transaction: &Transaction<'_>, app: &str, accounts: &[&str], resource: &Resource) -> rusqlite::Result<usize> {
    let Some(spec) = self::app(app) else { return Ok(0) };
    let mut changed = 0;
    for account in accounts {
        let held: Vec<Grant> = {
            let mut statement = transaction.prepare(&format!("SELECT {COLUMNS} FROM grants WHERE app = ?1 AND account_id = ?2"))?;
            statement.query_map(params![app, account], read_grant)?.collect::<Result<_, _>>()?
        };
        for grant in held.into_iter().filter(|grant| grant.resources.contains(resource)) {
            let left: Vec<&Resource> = grant.resources.iter().filter(|named| *named != resource).collect();
            if left.is_empty() && spec.needs_resources {
                forget(transaction, &grant.id)?;
            } else {
                transaction.execute(
                    "UPDATE grants SET resources = ?2 WHERE id = ?1",
                    params![grant.id, serde_json::to_string(&left).unwrap_or_default()],
                )?;
            }
            changed += 1;
        }
    }
    Ok(changed)
}

/// Forgets an account's grants (the account was deleted).
pub fn purge_account(db: &mut Connection, account_id: &str) -> Result<(), Refused> {
    let transaction = db.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    transaction.execute("DELETE FROM grant_uses WHERE grant_id IN (SELECT id FROM grants WHERE account_id = ?1)", [account_id])?;
    transaction.execute("DELETE FROM grants WHERE account_id = ?1", [account_id])?;
    transaction.commit()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ipc::Registration;

    const NOW: i64 = 1_795_183_320_000;

    fn db() -> Connection {
        let db = Connection::open_in_memory().expect("db");
        db.execute_batch(SCHEMA).expect("schema");
        // The DM outbox, where a DM grant's change is told (`regroup`).
        db.execute_batch(crate::dms::messages::SCHEMA).expect("schema");
        db.execute_batch(crate::dms::sync::SCHEMA).expect("schema");
        db
    }

    fn june() -> Actor {
        Actor { id: "june".into(), incarnation: "inc-1".into() }
    }

    fn backend(epoch: u64, agent: Option<Actor>) -> Caller {
        Caller::Backend { account_id: "acct".into(), claktory_id: "mac".into(), epoch, agent }
    }

    fn conversation(id: &str) -> Resource {
        Resource { kind: "conversation".into(), id: id.into() }
    }

    fn give(db: &mut Connection, scopes: &[&str], expires_ms: i64) -> Grant {
        let ask = Put {
            app: "dms".into(),
            agent: june(),
            scopes: scopes.iter().map(|scope| scope.to_string()).collect(),
            resources: vec![conversation("c1")],
            expires_ms,
        };
        put(db, &backend(3, None), ask, NOW).expect("granted")
    }

    fn allowed(db: &mut Connection, caller: &Caller, scope: &str, resource: Option<&Resource>, now: i64) -> Result<String, Refused> {
        let transaction = db.transaction().expect("transaction");
        let checked = check(&transaction, caller, "dms", scope, resource, now);
        transaction.commit().expect("commit");
        checked
    }

    #[test]
    fn a_grant_allows_only_its_agent_incarnation_epoch_scopes_and_resources() {
        let mut db = db();
        let grant = give(&mut db, &["dm.read", "dm.read", "dm.draft"], 0);
        assert_eq!(grant.scopes, ["dm.draft", "dm.read"], "sorted, once each");
        let as_june = backend(3, Some(june()));
        let c1 = conversation("c1");
        assert_eq!(allowed(&mut db, &as_june, "dm.read", Some(&c1), NOW), Ok(grant.id.clone()));
        assert!(allowed(&mut db, &as_june, "dm.read", None, NOW).is_ok(), "a call about no one resource");
        let refused = |db: &mut Connection, caller: &Caller, scope: &str, resource: Option<&Resource>| {
            assert_eq!(allowed(db, caller, scope, resource, NOW), Err(Refused::NotGranted), "{caller:?} {scope} {resource:?}");
        };
        refused(&mut db, &as_june, "dm.send", Some(&c1));
        refused(&mut db, &as_june, "dm.read", Some(&conversation("c2")));
        refused(&mut db, &as_june, "dm.unknown", Some(&c1));
        refused(&mut db, &backend(4, Some(june())), "dm.read", Some(&c1));
        refused(&mut db, &backend(3, Some(Actor { id: "june".into(), incarnation: "inc-2".into() })), "dm.read", Some(&c1));
        refused(&mut db, &backend(3, Some(Actor { id: "kai".into(), incarnation: "inc-1".into() })), "dm.read", Some(&c1));
        refused(&mut db, &backend(3, None), "dm.read", Some(&c1));
        let device = Caller::Device { account_id: "acct".into(), device_key: "k".into(), registrations: Vec::new(), removed_devices: Vec::new(), signed_in_at: None, bound_devices: None };
        refused(&mut db, &device, "dm.read", Some(&c1));
        let other_account = Caller::Backend { account_id: "other".into(), claktory_id: "mac".into(), epoch: 3, agent: Some(june()) };
        refused(&mut db, &other_account, "dm.read", Some(&c1));
        let transaction = db.transaction().expect("transaction");
        assert_eq!(check(&transaction, &as_june, "no-such-app", "dm.read", Some(&c1), NOW), Err(Refused::NotGranted));
    }

    #[test]
    fn an_ended_conversation_leaves_every_grant_that_named_it() {
        let mut db = db();
        let both = Put {
            app: "dms".into(),
            agent: june(),
            scopes: vec!["dm.read".into()],
            resources: vec![conversation("c1"), conversation("c2")],
            expires_ms: 0,
        };
        put(&mut db, &backend(3, None), both, NOW).expect("granted");
        let kai = Actor { id: "kai".into(), incarnation: "inc-1".into() };
        let only = Put {
            app: "dms".into(),
            agent: kai.clone(),
            scopes: vec!["dm.read".into()],
            resources: vec![conversation("c1")],
            expires_ms: 0,
        };
        put(&mut db, &backend(3, None), only, NOW).expect("granted");
        let transaction = db.transaction().expect("transaction");
        assert_eq!(drop_resource(&transaction, "dms", &["other"], &conversation("c1")), Ok(0), "not theirs");
        assert_eq!(drop_resource(&transaction, "dms", &["acct", "other"], &conversation("c1")), Ok(2));
        transaction.commit().expect("commit");
        let as_june = backend(3, Some(june()));
        assert_eq!(allowed(&mut db, &as_june, "dm.read", Some(&conversation("c1")), NOW), Err(Refused::NotGranted));
        assert!(allowed(&mut db, &as_june, "dm.read", Some(&conversation("c2")), NOW).is_ok(), "the other stays");
        let left: i64 = db.query_row("SELECT COUNT(*) FROM grants", [], |row| row.get(0)).expect("count");
        assert_eq!(left, 1, "kai's named nothing else, so it went");
    }

    #[test]
    fn grants_expire_are_replaced_revoked_and_counted() {
        let mut db = db();
        let grant = give(&mut db, &["dm.send"], NOW + 60_000);
        let as_june = backend(3, Some(june()));
        let c1 = conversation("c1");
        assert!(allowed(&mut db, &as_june, "dm.send", Some(&c1), NOW).is_ok());
        assert_eq!(allowed(&mut db, &as_june, "dm.send", Some(&c1), NOW + 60_000), Err(Refused::NotGranted), "expired");
        // Replaced: one grant per agent and app, the new one's scopes.
        let again = give(&mut db, &["dm.read"], 0);
        assert_ne!(again.id, grant.id);
        assert_eq!(allowed(&mut db, &as_june, "dm.send", Some(&c1), NOW), Err(Refused::NotGranted));
        // A minute's uses of dm.read, then none until the next minute.
        let minute = NOW - NOW % 60_000;
        for _ in 0..60 {
            allowed(&mut db, &as_june, "dm.read", Some(&c1), minute).expect("within the quota");
        }
        assert!(matches!(allowed(&mut db, &as_june, "dm.read", Some(&c1), minute + 1000), Err(Refused::Quota(59))));
        assert!(allowed(&mut db, &as_june, "dm.read", Some(&c1), minute + 60_000).is_ok(), "a new minute");
        // Revoked by a device of the account; another account's can't.
        let stranger = Caller::Device { account_id: "other".into(), device_key: "k".into(), registrations: Vec::new(), removed_devices: Vec::new(), signed_in_at: None, bound_devices: None };
        assert_eq!(revoke(&mut db, &stranger, &again.id, 0), Err(Refused::NotFound));
        let device = Caller::Device { account_id: "acct".into(), device_key: "k".into(), registrations: Vec::new(), removed_devices: Vec::new(), signed_in_at: None, bound_devices: None };
        revoke(&mut db, &device, &again.id, 0).expect("revoked");
        assert_eq!(allowed(&mut db, &as_june, "dm.read", Some(&c1), NOW), Err(Refused::NotGranted));
        let uses: i64 = db.query_row("SELECT COUNT(*) FROM grant_uses", [], |row| row.get(0)).expect("count");
        assert_eq!(uses, 0, "its uses go with it");
    }

    #[test]
    fn what_a_grant_asks_for_is_checked_and_only_live_grants_are_listed() {
        let mut db = db();
        let ask = |scopes: &[&str], resources: Vec<Resource>, expires_ms: i64| Put {
            app: "dms".into(),
            agent: june(),
            scopes: scopes.iter().map(|scope| scope.to_string()).collect(),
            resources,
            expires_ms,
        };
        let invalid = |db: &mut Connection, caller: &Caller, put_: Put| {
            assert!(matches!(put(db, caller, put_, NOW), Err(Refused::Invalid(_))));
        };
        invalid(&mut db, &backend(3, None), ask(&[], vec![conversation("c1")], 0));
        invalid(&mut db, &backend(3, None), ask(&["dm.everything"], vec![conversation("c1")], 0));
        invalid(&mut db, &backend(3, None), ask(&["dm.read"], Vec::new(), 0));
        invalid(&mut db, &backend(3, None), ask(&["dm.read"], vec![Resource { kind: "contact".into(), id: "x".into() }], 0));
        invalid(&mut db, &backend(3, None), ask(&["dm.read"], vec![conversation("a/../b")], 0));
        invalid(&mut db, &backend(3, None), ask(&["dm.read"], vec![conversation("c1")], NOW - 1));
        let mut unknown_app = ask(&["dm.read"], vec![conversation("c1")], 0);
        unknown_app.app = "mail".into();
        invalid(&mut db, &backend(3, None), unknown_app);
        let mut bad_agent = ask(&["dm.read"], vec![conversation("c1")], 0);
        bad_agent.agent.id = "june bob".into();
        invalid(&mut db, &backend(3, None), bad_agent);
        let device = |registrations: Vec<Registration>| Caller::Device { account_id: "acct".into(), device_key: "k".into(), registrations, removed_devices: Vec::new(), signed_in_at: None, bound_devices: None };
        invalid(&mut db, &device(Vec::new()), ask(&["dm.read"], vec![conversation("c1")], 0));

        let grant = give(&mut db, &["dm.read"], 0);
        let live = vec![Registration { claktory_id: "mac".into(), epoch: 3 }];
        assert_eq!(list(&mut db, &device(live.clone()), NOW).expect("list"), std::slice::from_ref(&grant));
        assert_eq!(list(&mut db, &backend(3, None), NOW).expect("list"), std::slice::from_ref(&grant));
        // Signed in again (epoch 4): the old grant is dead, and goes.
        assert!(list(&mut db, &backend(4, None), NOW).expect("list").is_empty());
        assert!(list(&mut db, &device(live), NOW).expect("list").is_empty(), "purged");
        let held: i64 = db.query_row("SELECT COUNT(*) FROM grants", [], |row| row.get(0)).expect("count");
        assert_eq!(held, 0);
    }
}
