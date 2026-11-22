//! The service's database: one SQLite file. It holds no shares and no
//! tokens, only their hashes, so a copy of it opens no vault.

use std::path::Path;

use rusqlite::{Connection, OptionalExtension as _, params};
use serde_json::{Value, json};

use super::geo::Location;
use crate::protocol::{Activity, CheckIn, Counts, Health, PublicStats, Usage, hex, sha256};

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS accounts (
    id TEXT PRIMARY KEY,
    email TEXT NOT NULL UNIQUE,
    created INTEGER NOT NULL,
    last_sign_in INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS claktories (
    id TEXT PRIMARY KEY,
    account_id TEXT NOT NULL,
    name TEXT NOT NULL,
    backend_key TEXT NOT NULL,
    token_hash TEXT NOT NULL UNIQUE,
    epoch INTEGER NOT NULL,
    created INTEGER NOT NULL,
    last_seen INTEGER NOT NULL,
    version TEXT NOT NULL,
    platform TEXT NOT NULL,
    revoked_at INTEGER,
    agents INTEGER,
    projects INTEGER,
    devices INTEGER,
    country TEXT,
    region TEXT,
    city TEXT,
    display_name TEXT,
    color TEXT,
    relays TEXT,
    pipes INTEGER,
    secrets INTEGER,
    monitors INTEGER,
    model_connected INTEGER,
    first_run INTEGER
);
CREATE INDEX IF NOT EXISTS claktories_account ON claktories (account_id);
CREATE TABLE IF NOT EXISTS endpoints (
    claktory_id TEXT NOT NULL,
    endpoint TEXT NOT NULL,
    kind TEXT NOT NULL,
    added INTEGER NOT NULL,
    PRIMARY KEY (claktory_id, endpoint)
);
CREATE TABLE IF NOT EXISTS share_releases (
    claktory_id TEXT NOT NULL,
    time INTEGER NOT NULL,
    ip TEXT NOT NULL,
    epoch INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS share_releases_claktory ON share_releases (claktory_id, time);
CREATE TABLE IF NOT EXISTS requests (
    id TEXT PRIMARY KEY,
    kind TEXT NOT NULL,
    email TEXT NOT NULL,
    claktory_id TEXT,
    name TEXT,
    backend_key TEXT,
    version TEXT,
    platform TEXT,
    code_hash TEXT NOT NULL,
    attempts INTEGER NOT NULL DEFAULT 0,
    expires INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS limits (
    key TEXT NOT NULL,
    window INTEGER NOT NULL,
    count INTEGER NOT NULL,
    PRIMARY KEY (key, window)
);
CREATE TABLE IF NOT EXISTS sessions (
    token_hash TEXT PRIMARY KEY,
    email TEXT NOT NULL,
    csrf TEXT NOT NULL,
    expires INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS sign_ins (
    day TEXT PRIMARY KEY,
    count INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS periods (
    claktory_id TEXT NOT NULL,
    id TEXT NOT NULL,
    from_time INTEGER NOT NULL,
    to_time INTEGER NOT NULL,
    received INTEGER NOT NULL,
    PRIMARY KEY (claktory_id, id)
);
CREATE TABLE IF NOT EXISTS usage (
    claktory_id TEXT NOT NULL,
    day TEXT NOT NULL,
    provider TEXT NOT NULL,
    model TEXT NOT NULL,
    requests INTEGER NOT NULL,
    input_tokens INTEGER NOT NULL,
    output_tokens INTEGER NOT NULL,
    cache_read_tokens INTEGER NOT NULL,
    cache_write_tokens INTEGER NOT NULL,
    PRIMARY KEY (claktory_id, day, provider, model)
);
CREATE TABLE IF NOT EXISTS models (
    provider TEXT NOT NULL,
    model TEXT NOT NULL,
    name TEXT NOT NULL,
    PRIMARY KEY (provider, model)
);
CREATE TABLE IF NOT EXISTS activity (
    claktory_id TEXT NOT NULL,
    hour INTEGER NOT NULL,
    agent_minutes INTEGER NOT NULL,
    agents_working INTEGER NOT NULL,
    runs INTEGER NOT NULL,
    PRIMARY KEY (claktory_id, hour)
);
CREATE TABLE IF NOT EXISTS seen_days (
    claktory_id TEXT NOT NULL,
    account_id TEXT NOT NULL,
    day TEXT NOT NULL,
    PRIMARY KEY (claktory_id, day)
);
CREATE TABLE IF NOT EXISTS devices (
    account_key TEXT PRIMARY KEY,
    account_id TEXT NOT NULL,
    name TEXT NOT NULL,
    platform TEXT NOT NULL,
    version TEXT NOT NULL,
    created INTEGER NOT NULL,
    last_seen INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS devices_account ON devices (account_id);
CREATE TABLE IF NOT EXISTS device_keys (
    device_key TEXT NOT NULL,
    account_key TEXT NOT NULL,
    issued INTEGER NOT NULL,
    PRIMARY KEY (device_key, account_key)
);
CREATE INDEX IF NOT EXISTS device_keys_account_key ON device_keys (account_key);
CREATE TABLE IF NOT EXISTS meta (
    key TEXT PRIMARY KEY,
    value INTEGER NOT NULL
);
-- A Claktory's paired devices at its last check-in, counted by system,
-- kind and app version (never named).
CREATE TABLE IF NOT EXISTS claktory_devices (
    claktory_id TEXT NOT NULL,
    platform TEXT NOT NULL,
    kind TEXT NOT NULL,
    app_version TEXT NOT NULL,
    count INTEGER NOT NULL,
    PRIMARY KEY (claktory_id, platform, kind, app_version)
);
-- The most of a Claktory's devices that connected in a period, per day.
CREATE TABLE IF NOT EXISTS devices_connected (
    claktory_id TEXT NOT NULL,
    day TEXT NOT NULL,
    count INTEGER NOT NULL,
    PRIMARY KEY (claktory_id, day)
);
-- Backend versions rolled back after an update, or stopped without
-- stopping cleanly, per day (the period's start).
CREATE TABLE IF NOT EXISTS health (
    claktory_id TEXT NOT NULL,
    day TEXT NOT NULL,
    version TEXT NOT NULL,
    rolled_back INTEGER NOT NULL,
    crashes INTEGER NOT NULL,
    PRIMARY KEY (claktory_id, day, version)
);
-- Deleted accounts the user-apps worker hasn't yet said it forgot: asked
-- again, with a growing wait, until it does.
CREATE TABLE IF NOT EXISTS apps_purges (
    account_id TEXT PRIMARY KEY,
    queued INTEGER NOT NULL,
    tries INTEGER NOT NULL,
    next_try INTEGER NOT NULL
);
";

/// How long a period's id is kept, to know it if it's sent again. A
/// Claktory sends a period at most `PERIOD_AGE` after it starts.
pub const PERIODS_KEPT: i64 = 120 * 86_400;
/// The oldest a period may be when it's sent.
pub const PERIOD_AGE: i64 = 90 * 86_400;

/// How long a share release, with its IP, is kept.
pub const RELEASES_KEPT: i64 = 30 * 86_400;

pub fn open(path: &Path) -> rusqlite::Result<Connection> {
    let conn = Connection::open(path)?;
    conn.pragma_update(None, "journal_mode", "WAL")?;
    conn.pragma_update(None, "synchronous", "NORMAL")?;
    conn.pragma_update(None, "foreign_keys", "ON")?;
    conn.busy_timeout(std::time::Duration::from_secs(5))?;
    // One transaction (SQLite's DDL is transactional): a crash midway
    // leaves the database as it was.
    let tx = conn.unchecked_transaction()?;
    let version: i64 = tx.pragma_query_value(None, "user_version", |row| row.get(0))?;
    tx.execute_batch(SCHEMA)?;
    if version >= 1 {
        // Columns added since the first version: the first had no counts or
        // location; before devices signed in, a Claktory sent no name, color
        // or relays; before #119, check-ins counted no pipes, secrets or
        // monitors, nor how far setup got. Each is added only when missing,
        // so a database an older service went back to (which sets its own,
        // lower user_version) opens again in this one.
        let mut have = std::collections::HashSet::new();
        {
            let mut columns = tx.prepare("SELECT name FROM pragma_table_info('claktories')")?;
            for name in columns.query_map([], |row| row.get::<_, String>(0))? {
                have.insert(name?);
            }
        }
        for column in [
            "agents INTEGER",
            "projects INTEGER",
            "devices INTEGER",
            "country TEXT",
            "region TEXT",
            "city TEXT",
            "display_name TEXT",
            "color TEXT",
            "relays TEXT",
            "pipes INTEGER",
            "secrets INTEGER",
            "monitors INTEGER",
            "model_connected INTEGER",
            "first_run INTEGER",
        ] {
            let name = column.split(' ').next().unwrap_or(column);
            if !have.contains(name) {
                tx.execute_batch(&format!("ALTER TABLE claktories ADD COLUMN {column}"))?;
            }
        }
    }
    tx.pragma_update(None, "user_version", 4)?;
    tx.commit()?;
    Ok(conn)
}

/// Writes a consistent copy of the database at `db` to `dir` (made if
/// missing), while the service uses it, as `account-<UTC time>.db`, and
/// keeps only the newest `keep` copies. The copy's path.
pub fn backup(db: &Path, dir: &Path, keep: usize, now: i64) -> Result<std::path::PathBuf, String> {
    std::fs::create_dir_all(dir).map_err(|error| format!("can't make {}: {error}", dir.display()))?;
    let stamp = jiff::Timestamp::from_second(now).map_err(|error| error.to_string())?.strftime("%Y%m%d-%H%M%S").to_string();
    let path = dir.join(format!("account-{stamp}.db"));
    let conn = Connection::open_with_flags(db, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(|error| format!("can't open {}: {error}", db.display()))?;
    conn.busy_timeout(std::time::Duration::from_secs(30)).map_err(|error| error.to_string())?;
    let target = path.to_str().ok_or("the backup folder's path isn't UTF-8")?;
    conn.execute("VACUUM INTO ?1", params![target]).map_err(|error| format!("can't write {}: {error}", path.display()))?;
    let mut copies: Vec<std::path::PathBuf> = std::fs::read_dir(dir)
        .map_err(|error| error.to_string())?
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| {
            path.file_name().and_then(|name| name.to_str()).is_some_and(|name| name.starts_with("account-") && name.ends_with(".db"))
        })
        .collect();
    copies.sort();
    for old in copies.iter().rev().skip(keep) {
        let _ = std::fs::remove_file(old);
    }
    Ok(path)
}

/// The hex SHA-256 a token or code is kept as.
pub fn hash(text: &str) -> String {
    hex(sha256(text.as_bytes()))
}

/// A code's hash, salted with its request's id.
pub fn code_hash(code: &str, request_id: &str) -> String {
    hash(&format!("{code}\0{request_id}"))
}

/// A limit's key for an email address, without the address in it.
pub fn email_key(kind: &str, email: &str) -> String {
    format!("{kind}:{}", &hash(email)[..24])
}

/// Counts one use of each `(key, period, most)` in fixed windows of
/// `period` seconds, when all have room; otherwise the seconds until the
/// fullest has room again, counting nothing.
pub fn take(conn: &Connection, limits: &[(&str, i64, u32)], now: i64) -> rusqlite::Result<Result<(), i64>> {
    if let Some(wait) = spent(conn, limits, now)? {
        return Ok(Err(wait));
    }
    for (key, period, _) in limits {
        bump(conn, key, now - now.rem_euclid(*period))?;
    }
    Ok(Ok(()))
}

/// The seconds until the fullest of `limits` has room again, if one is
/// full; counting nothing.
pub fn spent(conn: &Connection, limits: &[(&str, i64, u32)], now: i64) -> rusqlite::Result<Option<i64>> {
    let mut wait = 0;
    for (key, period, most) in limits {
        let window = now - now.rem_euclid(*period);
        if count_in(conn, key, window)? >= *most {
            wait = wait.max(window + period - now);
        }
    }
    Ok((wait > 0).then_some(wait))
}

/// Counts one use of `key` in today's window (UTC), and returns the count.
pub fn count_today(conn: &Connection, key: &str, now: i64) -> rusqlite::Result<u32> {
    let window = now - now.rem_euclid(86_400);
    bump(conn, key, window)?;
    count_in(conn, key, window)
}

/// Today's count of `key`, without counting.
pub fn today(conn: &Connection, key: &str, now: i64) -> rusqlite::Result<u32> {
    count_in(conn, key, now - now.rem_euclid(86_400))
}

fn count_in(conn: &Connection, key: &str, window: i64) -> rusqlite::Result<u32> {
    Ok(conn
        .query_row("SELECT count FROM limits WHERE key = ?1 AND window = ?2", params![key, window], |row| row.get(0))
        .optional()?
        .unwrap_or(0))
}

fn bump(conn: &Connection, key: &str, window: i64) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO limits (key, window, count) VALUES (?1, ?2, 1) ON CONFLICT (key, window) DO UPDATE SET count = count + 1",
        params![key, window],
    )?;
    Ok(())
}

/// A sign-in waiting for its code.
#[derive(Clone, Debug)]
pub struct Request {
    pub id: String,
    /// "claktory" from a Claktory, "web" from claktory.com/account.
    pub kind: String,
    pub email: String,
    pub claktory_id: Option<String>,
    pub name: Option<String>,
    pub backend_key: Option<String>,
    pub version: Option<String>,
    pub platform: Option<String>,
    pub code_hash: String,
    pub attempts: u32,
    pub expires: i64,
}

pub fn insert_request(conn: &Connection, request: &Request) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO requests (id, kind, email, claktory_id, name, backend_key, version, platform, code_hash, attempts, expires)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
        params![
            request.id,
            request.kind,
            request.email,
            request.claktory_id,
            request.name,
            request.backend_key,
            request.version,
            request.platform,
            request.code_hash,
            request.attempts,
            request.expires
        ],
    )?;
    Ok(())
}

pub fn request(conn: &Connection, id: &str) -> rusqlite::Result<Option<Request>> {
    conn.query_row(
        "SELECT id, kind, email, claktory_id, name, backend_key, version, platform, code_hash, attempts, expires FROM requests WHERE id = ?1",
        params![id],
        |row| {
            Ok(Request {
                id: row.get(0)?,
                kind: row.get(1)?,
                email: row.get(2)?,
                claktory_id: row.get(3)?,
                name: row.get(4)?,
                backend_key: row.get(5)?,
                version: row.get(6)?,
                platform: row.get(7)?,
                code_hash: row.get(8)?,
                attempts: row.get(9)?,
                expires: row.get(10)?,
            })
        },
    )
    .optional()
}

pub fn delete_request(conn: &Connection, id: &str) -> rusqlite::Result<()> {
    conn.execute("DELETE FROM requests WHERE id = ?1", params![id])?;
    Ok(())
}

pub fn count_attempt(conn: &Connection, id: &str) -> rusqlite::Result<()> {
    conn.execute("UPDATE requests SET attempts = attempts + 1 WHERE id = ?1", params![id])?;
    Ok(())
}

#[derive(Clone, Debug)]
pub struct Account {
    pub id: String,
    pub email: String,
    pub created: i64,
    pub last_sign_in: i64,
}

pub fn account_by_email(conn: &Connection, email: &str) -> rusqlite::Result<Option<Account>> {
    conn.query_row("SELECT id, email, created, last_sign_in FROM accounts WHERE email = ?1", params![email], |row| {
        Ok(Account { id: row.get(0)?, email: row.get(1)?, created: row.get(2)?, last_sign_in: row.get(3)? })
    })
    .optional()
}

pub fn account_email(conn: &Connection, account_id: &str) -> rusqlite::Result<Option<String>> {
    conn.query_row("SELECT email FROM accounts WHERE id = ?1", params![account_id], |row| row.get(0)).optional()
}

#[derive(Clone, Debug)]
pub struct Claktory {
    pub id: String,
    pub account_id: String,
    pub name: String,
    pub backend_key: String,
    pub epoch: u64,
    pub created: i64,
    pub last_seen: i64,
    pub version: String,
    pub platform: String,
    pub revoked_at: Option<i64>,
    /// From its last check-in that had them.
    pub counts: Option<Counts>,
    /// From the IP of its last request, when the service has a GeoIP file.
    pub location: Location,
    /// Its own name and color, and the relays its pairing codes give, from
    /// its last `PUT /v1/endpoints` that had them.
    pub display_name: Option<String>,
    pub color: Option<String>,
    pub relays: Vec<String>,
}

const CLAKTORY: &str = "SELECT id, account_id, name, backend_key, epoch, created, last_seen, version, platform, revoked_at,
    agents, projects, devices, country, region, city, display_name, color, relays, pipes, secrets, monitors FROM claktories";

fn claktory_row(row: &rusqlite::Row) -> rusqlite::Result<Claktory> {
    Ok(Claktory {
        id: row.get(0)?,
        account_id: row.get(1)?,
        name: row.get(2)?,
        backend_key: row.get(3)?,
        epoch: row.get::<_, i64>(4)? as u64,
        created: row.get(5)?,
        last_seen: row.get(6)?,
        version: row.get(7)?,
        platform: row.get(8)?,
        revoked_at: row.get(9)?,
        counts: match (row.get::<_, Option<u32>>(10)?, row.get::<_, Option<u32>>(11)?, row.get::<_, Option<u32>>(12)?) {
            (Some(agents), Some(projects), Some(devices)) => Some(Counts {
                agents,
                projects,
                devices,
                pipes: row.get::<_, Option<u32>>(19)?.unwrap_or(0),
                secrets: row.get::<_, Option<u32>>(20)?.unwrap_or(0),
                monitors: row.get::<_, Option<u32>>(21)?.unwrap_or(0),
            }),
            _ => None,
        },
        location: Location { country: row.get(13)?, region: row.get(14)?, city: row.get(15)? },
        display_name: row.get(16)?,
        color: row.get(17)?,
        relays: row.get::<_, Option<String>>(18)?.and_then(|relays| serde_json::from_str(&relays).ok()).unwrap_or_default(),
    })
}

pub fn claktory(conn: &Connection, id: &str) -> rusqlite::Result<Option<Claktory>> {
    conn.query_row(&format!("{CLAKTORY} WHERE id = ?1"), params![id], claktory_row).optional()
}

pub fn claktory_by_token(conn: &Connection, token: &str) -> rusqlite::Result<Option<Claktory>> {
    conn.query_row(&format!("{CLAKTORY} WHERE token_hash = ?1"), params![hash(token)], claktory_row).optional()
}

/// A Claktory moved: its new backend key and token replace the old (which
/// stop working), and its computer's name, version and platform are the
/// new computer's. False when the token is another Claktory's already.
pub fn moved(
    conn: &Connection,
    id: &str,
    backend_key: &str,
    token: &str,
    name: &str,
    version: &str,
    platform: &str,
) -> rusqlite::Result<bool> {
    let taken: bool =
        conn.query_row("SELECT EXISTS (SELECT 1 FROM claktories WHERE token_hash = ?1)", params![hash(token)], |row| row.get(0))?;
    if taken {
        return Ok(false);
    }
    conn.execute(
        "UPDATE claktories SET backend_key = ?2, token_hash = ?3, name = ?4, version = ?5, platform = ?6 WHERE id = ?1",
        params![id, backend_key, hash(token), name, version, platform],
    )?;
    Ok(true)
}

pub fn claktories(conn: &Connection, account_id: &str) -> rusqlite::Result<Vec<Claktory>> {
    let mut statement = conn.prepare(&format!("{CLAKTORY} WHERE account_id = ?1 ORDER BY created"))?;
    statement.query_map(params![account_id], claktory_row)?.collect()
}

/// What a code that was right signs in to.
pub enum Registered {
    Done {
        account_id: String,
        epoch: u64,
    },
    /// Another account has this Claktory id, and hasn't removed it.
    Taken,
    /// The account has as many Claktories as it may.
    TooMany,
}

/// The most Claktories an account may have (removed ones don't count).
pub const CLAKTORIES_PER_ACCOUNT: u32 = 10;

/// Signs a Claktory in from a request whose code was right: makes the
/// account if it's new, then registers the Claktory, or gives one signing in
/// again a new token, backend key and epoch, undoing a removal. `proven`:
/// the Claktory proved it holds the backend key it has now, so it may move
/// from another account (signed out there, signing in to this one).
pub fn register(conn: &mut Connection, request: &Request, token: &str, now: i64, proven: bool) -> rusqlite::Result<Registered> {
    let (Some(id), Some(name), Some(backend_key), Some(version), Some(platform)) =
        (&request.claktory_id, &request.name, &request.backend_key, &request.version, &request.platform)
    else {
        return Err(rusqlite::Error::InvalidQuery);
    };
    let tx = conn.transaction()?;
    let account_id = account_for(&tx, &request.email, now)?;
    let existing = claktory(&tx, id)?;
    if let Some(existing) = &existing
        && existing.account_id != account_id
        && existing.revoked_at.is_none()
        && !proven
    {
        return Ok(Registered::Taken);
    }
    let others: u32 = tx.query_row(
        "SELECT count(*) FROM claktories WHERE account_id = ?1 AND revoked_at IS NULL AND id != ?2",
        params![account_id, id],
        |row| row.get(0),
    )?;
    if others >= CLAKTORIES_PER_ACCOUNT {
        return Ok(Registered::TooMany);
    }
    let epoch = match &existing {
        Some(existing) => {
            if existing.account_id != account_id {
                // Removed by its old account, or moving from it: what that
                // account knew of it goes.
                tx.execute("DELETE FROM endpoints WHERE claktory_id = ?1", params![id])?;
                tx.execute("DELETE FROM share_releases WHERE claktory_id = ?1", params![id])?;
                anonymize(&tx, id)?;
                tx.execute(
                    "UPDATE claktories SET agents = NULL, projects = NULL, devices = NULL, country = NULL, region = NULL, city = NULL,
                     display_name = NULL, color = NULL, relays = NULL WHERE id = ?1",
                    params![id],
                )?;
            }
            let epoch = existing.epoch + 1;
            tx.execute(
                "UPDATE claktories SET account_id = ?2, name = ?3, backend_key = ?4, token_hash = ?5, epoch = ?6,
                 last_seen = ?7, version = ?8, platform = ?9, revoked_at = NULL WHERE id = ?1",
                params![id, account_id, name, backend_key, hash(token), epoch as i64, now, version, platform],
            )?;
            epoch
        }
        None => {
            tx.execute(
                "INSERT INTO claktories (id, account_id, name, backend_key, token_hash, epoch, created, last_seen, version, platform)
                 VALUES (?1, ?2, ?3, ?4, ?5, 1, ?6, ?6, ?7, ?8)",
                params![id, account_id, name, backend_key, hash(token), now, version, platform],
            )?;
            1
        }
    };
    tx.execute("UPDATE accounts SET last_sign_in = ?2 WHERE id = ?1", params![account_id, now])?;
    tx.execute("INSERT OR IGNORE INTO meta (key, value) VALUES ('since', ?1)", params![now])?;
    seen(&tx, id, &account_id, now)?;
    tx.execute("DELETE FROM requests WHERE id = ?1", params![request.id])?;
    count_sign_in(&tx, now)?;
    bump_allowlist(&tx)?;
    tx.commit()?;
    Ok(Registered::Done { account_id, epoch })
}

/// The id of the account with this email, made now if there's none.
fn account_for(conn: &Connection, email: &str, now: i64) -> rusqlite::Result<String> {
    if let Some(account) = account_by_email(conn, email)? {
        return Ok(account.id);
    }
    let id = crate::protocol::b64(*crate::protocol::random_32());
    conn.execute("INSERT INTO accounts (id, email, created, last_sign_in) VALUES (?1, ?2, ?3, ?3)", params![id, email, now])?;
    Ok(id)
}

/// Counts a sign-in for its day, for `stats`.
pub fn count_sign_in(conn: &Connection, now: i64) -> rusqlite::Result<()> {
    let day = jiff::Timestamp::from_second(now).map(|time| time.strftime("%Y-%m-%d").to_string()).unwrap_or_default();
    conn.execute("INSERT INTO sign_ins (day, count) VALUES (?1, 1) ON CONFLICT (day) DO UPDATE SET count = count + 1", params![day])?;
    Ok(())
}

/// "2026-11-20", the UTC day of a time.
pub fn day(unix: i64) -> String {
    jiff::Timestamp::from_second(unix).map(|time| time.strftime("%Y-%m-%d").to_string()).unwrap_or_default()
}

/// Records that a Claktory was in touch today.
fn seen(conn: &Connection, id: &str, account_id: &str, now: i64) -> rusqlite::Result<()> {
    conn.execute("INSERT OR IGNORE INTO seen_days (claktory_id, account_id, day) VALUES (?1, ?2, ?3)", params![id, account_id, day(now)])?;
    Ok(())
}

/// A Claktory was in touch: its last contact, the day, and where from.
pub fn touch(conn: &Connection, claktory: &Claktory, location: Option<&Location>, now: i64) -> rusqlite::Result<()> {
    conn.execute("UPDATE claktories SET last_seen = ?2 WHERE id = ?1", params![claktory.id, now])?;
    seen(conn, &claktory.id, &claktory.account_id, now)?;
    if let Some(location) = location {
        conn.execute(
            "UPDATE claktories SET country = ?2, region = ?3, city = ?4 WHERE id = ?1",
            params![claktory.id, location.country, location.region, location.city],
        )?;
    }
    Ok(())
}

/// Records a check-in whose periods are checked already; how many periods
/// were new.
pub fn check_in(
    conn: &mut Connection,
    claktory: &Claktory,
    body: &CheckIn,
    location: Option<&Location>,
    now: i64,
) -> rusqlite::Result<u32> {
    let tx = conn.transaction()?;
    tx.execute("UPDATE claktories SET version = ?2, platform = ?3 WHERE id = ?1", params![claktory.id, body.version, body.platform])?;
    if let Some(counts) = body.counts {
        tx.execute(
            "UPDATE claktories SET agents = ?2, projects = ?3, devices = ?4, pipes = ?5, secrets = ?6, monitors = ?7 WHERE id = ?1",
            params![claktory.id, counts.agents, counts.projects, counts.devices, counts.pipes, counts.secrets, counts.monitors],
        )?;
    }
    // Reached once, kept: a later check-in can't undo a step.
    if let Some(setup) = body.setup {
        tx.execute(
            "UPDATE claktories SET model_connected = max(coalesce(model_connected, 0), ?2), first_run = max(coalesce(first_run, 0), ?3)
             WHERE id = ?1",
            params![claktory.id, setup.model_connected, setup.first_run],
        )?;
    }
    if let Some(devices) = &body.devices {
        tx.execute("DELETE FROM claktory_devices WHERE claktory_id = ?1", params![claktory.id])?;
        for device in devices {
            tx.execute(
                "INSERT INTO claktory_devices (claktory_id, platform, kind, app_version, count) VALUES (?1, ?2, ?3, ?4, ?5)
                 ON CONFLICT (claktory_id, platform, kind, app_version) DO UPDATE SET count = count + excluded.count",
                params![claktory.id, device.platform, device.kind, device.app_version, device.count],
            )?;
        }
    }
    touch(&tx, claktory, location, now)?;
    let mut recorded = 0;
    for period in &body.periods {
        let new = tx.execute(
            "INSERT OR IGNORE INTO periods (claktory_id, id, from_time, to_time, received) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![claktory.id, period.id, period.from, period.to, now],
        )?;
        if new == 0 {
            continue;
        }
        recorded += 1;
        for usage in &period.usage {
            add_usage(&tx, &claktory.id, &day(period.from), usage)?;
        }
        for activity in &period.activity {
            add_activity(&tx, &claktory.id, activity)?;
        }
        if let Some(connected) = period.devices_connected {
            tx.execute(
                "INSERT INTO devices_connected (claktory_id, day, count) VALUES (?1, ?2, ?3)
                 ON CONFLICT (claktory_id, day) DO UPDATE SET count = max(count, excluded.count)",
                params![claktory.id, day(period.from), connected],
            )?;
        }
        for health in &period.health {
            add_health(&tx, &claktory.id, &day(period.from), health)?;
        }
    }
    // Names are kept from any check-in, even a resent period's.
    for usage in body.periods.iter().flat_map(|period| &period.usage) {
        if let Some(name) = &usage.model_name {
            tx.execute(
                "INSERT INTO models (provider, model, name) VALUES (?1, ?2, ?3) ON CONFLICT (provider, model) DO UPDATE SET name = excluded.name",
                params![usage.provider, usage.model, name],
            )?;
        }
    }
    tx.commit()?;
    Ok(recorded)
}

const ADD_USAGE: &str = "ON CONFLICT (claktory_id, day, provider, model) DO UPDATE SET
    requests = requests + excluded.requests,
    input_tokens = input_tokens + excluded.input_tokens,
    output_tokens = output_tokens + excluded.output_tokens,
    cache_read_tokens = cache_read_tokens + excluded.cache_read_tokens,
    cache_write_tokens = cache_write_tokens + excluded.cache_write_tokens";

const ADD_ACTIVITY: &str = "ON CONFLICT (claktory_id, hour) DO UPDATE SET
    agent_minutes = agent_minutes + excluded.agent_minutes,
    agents_working = max(agents_working, excluded.agents_working),
    runs = runs + excluded.runs";

fn add_usage(conn: &Connection, id: &str, day: &str, usage: &Usage) -> rusqlite::Result<()> {
    conn.execute(
        &format!(
            "INSERT INTO usage (claktory_id, day, provider, model, requests, input_tokens, output_tokens, cache_read_tokens, cache_write_tokens)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9) {ADD_USAGE}"
        ),
        params![
            id,
            day,
            usage.provider,
            usage.model,
            usage.requests as i64,
            usage.input_tokens as i64,
            usage.output_tokens as i64,
            usage.cache_read_tokens as i64,
            usage.cache_write_tokens as i64
        ],
    )?;
    Ok(())
}

const ADD_HEALTH: &str = "ON CONFLICT (claktory_id, day, version) DO UPDATE SET
    rolled_back = rolled_back + excluded.rolled_back,
    crashes = crashes + excluded.crashes";

fn add_health(conn: &Connection, id: &str, day: &str, health: &Health) -> rusqlite::Result<()> {
    conn.execute(
        &format!("INSERT INTO health (claktory_id, day, version, rolled_back, crashes) VALUES (?1, ?2, ?3, ?4, ?5) {ADD_HEALTH}"),
        params![id, day, health.version, health.rolled_back, health.crashes],
    )?;
    Ok(())
}

fn add_activity(conn: &Connection, id: &str, activity: &Activity) -> rusqlite::Result<()> {
    conn.execute(
        &format!(
            "INSERT INTO activity (claktory_id, hour, agent_minutes, agents_working, runs) VALUES (?1, ?2, ?3, ?4, ?5) {ADD_ACTIVITY}"
        ),
        params![id, activity.hour, activity.agent_minutes, activity.agents_working, activity.runs],
    )?;
    Ok(())
}

/// Unlinks a Claktory's usage and activity from it, keeping the totals:
/// they join the anonymous ones (Claktory id ""), and the days it was in
/// touch count under a random stand-in.
fn anonymize(conn: &Connection, id: &str) -> rusqlite::Result<()> {
    // `WHERE true` lets SQLite parse the upsert after a SELECT.
    conn.execute(
        &format!(
            "INSERT INTO usage (claktory_id, day, provider, model, requests, input_tokens, output_tokens, cache_read_tokens, cache_write_tokens)
             SELECT '', day, provider, model, requests, input_tokens, output_tokens, cache_read_tokens, cache_write_tokens
             FROM usage WHERE claktory_id = ?1 AND true {ADD_USAGE}"
        ),
        params![id],
    )?;
    conn.execute("DELETE FROM usage WHERE claktory_id = ?1", params![id])?;
    // Across Claktories, the most working at once adds up.
    conn.execute(
        "INSERT INTO activity (claktory_id, hour, agent_minutes, agents_working, runs)
         SELECT '', hour, agent_minutes, agents_working, runs FROM activity WHERE claktory_id = ?1 AND true
         ON CONFLICT (claktory_id, hour) DO UPDATE SET
           agent_minutes = agent_minutes + excluded.agent_minutes,
           agents_working = agents_working + excluded.agents_working,
           runs = runs + excluded.runs",
        params![id],
    )?;
    conn.execute("DELETE FROM activity WHERE claktory_id = ?1", params![id])?;
    conn.execute(
        &format!(
            "INSERT INTO health (claktory_id, day, version, rolled_back, crashes)
             SELECT '', day, version, rolled_back, crashes FROM health WHERE claktory_id = ?1 AND true {ADD_HEALTH}"
        ),
        params![id],
    )?;
    conn.execute("DELETE FROM health WHERE claktory_id = ?1", params![id])?;
    conn.execute("DELETE FROM claktory_devices WHERE claktory_id = ?1", params![id])?;
    conn.execute("DELETE FROM devices_connected WHERE claktory_id = ?1", params![id])?;
    let stand_in = format!("deleted-{}", crate::protocol::b64(&crate::protocol::random_32()[..12]));
    conn.execute("UPDATE seen_days SET claktory_id = ?2, account_id = ?2 WHERE claktory_id = ?1", params![id, stand_in])?;
    conn.execute("DELETE FROM periods WHERE claktory_id = ?1", params![id])?;
    Ok(())
}

/// A Claktory's usage since a day, per model: (the model's name as the app
/// shows it, or its id; requests; all tokens).
pub fn usage_since(conn: &Connection, id: &str, since: &str) -> rusqlite::Result<Vec<(String, i64, i64)>> {
    let mut statement = conn.prepare(
        "SELECT coalesce(models.name, usage.model), sum(requests), sum(input_tokens + output_tokens + cache_read_tokens + cache_write_tokens)
         FROM usage LEFT JOIN models ON models.provider = usage.provider AND models.model = usage.model
         WHERE claktory_id = ?1 AND day >= ?2 GROUP BY usage.provider, usage.model ORDER BY 3 DESC",
    )?;
    statement.query_map(params![id, since], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?.collect()
}

/// Minutes a Claktory's agents worked since an hour.
pub fn agent_minutes_since(conn: &Connection, id: &str, since: i64) -> rusqlite::Result<i64> {
    conn.query_row(
        "SELECT coalesce(sum(agent_minutes), 0) FROM activity WHERE claktory_id = ?1 AND hour >= ?2",
        params![id, since],
        |row| row.get(0),
    )
}

/// What claktory.com's counter shows.
pub fn public_stats(conn: &Connection) -> rusqlite::Result<PublicStats> {
    let claktories: i64 = conn.query_row("SELECT count(*) FROM claktories WHERE revoked_at IS NULL", [], |row| row.get(0))?;
    let (input, output, read, write): (i64, i64, i64, i64) = conn.query_row(
        "SELECT coalesce(sum(input_tokens), 0), coalesce(sum(output_tokens), 0), coalesce(sum(cache_read_tokens), 0), coalesce(sum(cache_write_tokens), 0)
         FROM usage",
        [],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
    )?;
    let since: Option<i64> = conn.query_row("SELECT value FROM meta WHERE key = 'since'", [], |row| row.get(0)).optional()?;
    let count = |number: i64| number.max(0) as u64;
    Ok(PublicStats {
        claktories: count(claktories),
        tokens_total: count(input) + count(output) + count(read) + count(write),
        input_tokens: count(input),
        output_tokens: count(output),
        cache_read_tokens: count(read),
        cache_write_tokens: count(write),
        since: since.unwrap_or(0),
    })
}

/// The last share release of a Claktory, if any is kept.
pub fn last_release(conn: &Connection, id: &str) -> rusqlite::Result<Option<i64>> {
    conn.query_row("SELECT max(time) FROM share_releases WHERE claktory_id = ?1", params![id], |row| row.get(0))
}

pub fn record_release(conn: &Connection, id: &str, now: i64, ip: &str, epoch: u64) -> rusqlite::Result<()> {
    conn.execute("INSERT INTO share_releases (claktory_id, time, ip, epoch) VALUES (?1, ?2, ?3, ?4)", params![id, now, ip, epoch as i64])?;
    Ok(())
}

/// A Claktory's kept releases, newest first: (time, IP).
pub fn releases(conn: &Connection, id: &str, most: u32) -> rusqlite::Result<Vec<(i64, String)>> {
    let mut statement = conn.prepare("SELECT time, ip FROM share_releases WHERE claktory_id = ?1 ORDER BY time DESC LIMIT ?2")?;
    statement.query_map(params![id, most], |row| Ok((row.get(0)?, row.get(1)?)))?.collect()
}

/// Replaces a Claktory's endpoints; whether they changed.
pub fn set_endpoints(conn: &mut Connection, id: &str, backend: &str, devices: &[String], now: i64) -> rusqlite::Result<bool> {
    let tx = conn.transaction()?;
    let mut wanted: Vec<(String, &str)> = vec![(backend.to_owned(), "backend")];
    for device in devices {
        if !wanted.iter().any(|(endpoint, _)| endpoint == device) {
            wanted.push((device.clone(), "device"));
        }
    }
    let mut had: Vec<(String, String)> = {
        let mut statement = tx.prepare("SELECT endpoint, kind FROM endpoints WHERE claktory_id = ?1")?;
        statement.query_map(params![id], |row| Ok((row.get(0)?, row.get(1)?)))?.collect::<rusqlite::Result<_>>()?
    };
    let mut new: Vec<(String, String)> = wanted.iter().map(|(endpoint, kind)| (endpoint.clone(), (*kind).to_owned())).collect();
    had.sort();
    new.sort();
    if had == new {
        return Ok(false);
    }
    tx.execute("DELETE FROM endpoints WHERE claktory_id = ?1", params![id])?;
    for (endpoint, kind) in &wanted {
        tx.execute("INSERT INTO endpoints (claktory_id, endpoint, kind, added) VALUES (?1, ?2, ?3, ?4)", params![id, endpoint, kind, now])?;
    }
    bump_allowlist(&tx)?;
    tx.commit()?;
    Ok(true)
}

/// Keeps what a Claktory sent of its own name, color and relays (each only
/// when it sent it).
pub fn set_identity(
    conn: &Connection,
    id: &str,
    name: Option<&str>,
    color: Option<&str>,
    relays: Option<&[String]>,
) -> rusqlite::Result<()> {
    if let Some(name) = name {
        conn.execute("UPDATE claktories SET display_name = ?2 WHERE id = ?1", params![id, name])?;
    }
    if let Some(color) = color {
        conn.execute("UPDATE claktories SET color = ?2 WHERE id = ?1", params![id, color])?;
    }
    if let Some(relays) = relays {
        let relays = serde_json::to_string(relays).unwrap_or_else(|_| "[]".to_owned());
        conn.execute("UPDATE claktories SET relays = ?2 WHERE id = ?1", params![id, relays])?;
    }
    Ok(())
}

/// A Claktory's own endpoint id, if it registered one.
pub fn backend_endpoint(conn: &Connection, id: &str) -> rusqlite::Result<Option<String>> {
    conn.query_row("SELECT endpoint FROM endpoints WHERE claktory_id = ?1 AND kind = 'backend'", params![id], |row| row.get(0)).optional()
}

pub fn allowlist_version(conn: &Connection) -> rusqlite::Result<i64> {
    Ok(conn.query_row("SELECT value FROM meta WHERE key = 'allowlist_version'", [], |row| row.get(0)).optional()?.unwrap_or(0))
}

fn bump_allowlist(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute("INSERT INTO meta (key, value) VALUES ('allowlist_version', 1) ON CONFLICT (key) DO UPDATE SET value = value + 1", [])?;
    Ok(())
}

/// The endpoints relays let through: those of Claktories not removed, and
/// the device keys assertions were made for in the last day (`purge` drops
/// older ones), of devices still signed in.
pub fn allowlist(conn: &Connection) -> rusqlite::Result<Vec<String>> {
    let mut statement = conn.prepare(
        "SELECT endpoint FROM endpoints JOIN claktories ON claktories.id = endpoints.claktory_id
         WHERE claktories.revoked_at IS NULL
         UNION
         SELECT device_key FROM device_keys JOIN devices ON devices.account_key = device_keys.account_key
         ORDER BY 1",
    )?;
    statement.query_map([], |row| row.get(0))?.collect()
}

/// Removes a Claktory from its account; false when it isn't the account's,
/// or is removed already.
pub fn revoke(conn: &Connection, account_id: &str, id: &str, now: i64) -> rusqlite::Result<bool> {
    let changed = conn.execute(
        "UPDATE claktories SET revoked_at = ?3 WHERE id = ?1 AND account_id = ?2 AND revoked_at IS NULL",
        params![id, account_id, now],
    )?;
    if changed > 0 {
        bump_allowlist(conn)?;
    }
    Ok(changed > 0)
}

/// Deletes an account and everything linked to it. With `apps` (the service
/// runs user-level apps), it's queued for the worker to forget too, in the
/// same transaction, so a deletion is never left half done.
pub fn delete_account(conn: &mut Connection, account: &Account, apps: bool, now: i64) -> rusqlite::Result<()> {
    let tx = conn.transaction()?;
    for claktory in claktories(&tx, &account.id)? {
        anonymize(&tx, &claktory.id)?;
    }
    for table in ["endpoints", "share_releases"] {
        tx.execute(
            &format!("DELETE FROM {table} WHERE claktory_id IN (SELECT id FROM claktories WHERE account_id = ?1)"),
            params![account.id],
        )?;
    }
    tx.execute("DELETE FROM claktories WHERE account_id = ?1", params![account.id])?;
    tx.execute(
        "DELETE FROM device_keys WHERE account_key IN (SELECT account_key FROM devices WHERE account_id = ?1)",
        params![account.id],
    )?;
    tx.execute("DELETE FROM devices WHERE account_id = ?1", params![account.id])?;
    tx.execute("DELETE FROM requests WHERE email = ?1", params![account.email])?;
    tx.execute("DELETE FROM sessions WHERE email = ?1", params![account.email])?;
    tx.execute("DELETE FROM accounts WHERE id = ?1", params![account.id])?;
    if apps {
        tx.execute(
            "INSERT INTO apps_purges (account_id, queued, tries, next_try) VALUES (?1, ?2, 0, ?2) ON CONFLICT (account_id) DO NOTHING",
            params![account.id, now],
        )?;
    }
    bump_allowlist(&tx)?;
    tx.commit()
}

/// Deleted accounts due to be purged from the user-apps worker at `now`
/// (at most `limit`), with how often it was asked already.
pub fn apps_purges_due(conn: &Connection, now: i64, limit: u32) -> rusqlite::Result<Vec<(String, i64)>> {
    let mut statement = conn.prepare("SELECT account_id, tries FROM apps_purges WHERE next_try <= ?1 ORDER BY next_try LIMIT ?2")?;
    statement.query_map(params![now, limit], |row| Ok((row.get(0)?, row.get(1)?)))?.collect()
}

/// The worker purged it: done.
pub fn apps_purged(conn: &Connection, account_id: &str) -> rusqlite::Result<()> {
    conn.execute("DELETE FROM apps_purges WHERE account_id = ?1", params![account_id])?;
    Ok(())
}

/// The worker didn't purge it: asked again at `next_try`.
pub fn apps_purge_failed(conn: &Connection, account_id: &str, next_try: i64) -> rusqlite::Result<()> {
    conn.execute("UPDATE apps_purges SET tries = tries + 1, next_try = ?2 WHERE account_id = ?1", params![account_id, next_try])?;
    Ok(())
}

/// Everything kept about an account, for Download My Data.
pub fn account_data(conn: &Connection, account: &Account) -> rusqlite::Result<Value> {
    let mut claktories_out = Vec::new();
    for claktory in claktories(conn, &account.id)? {
        let endpoints: Vec<Value> = {
            let mut statement =
                conn.prepare("SELECT endpoint, kind, added FROM endpoints WHERE claktory_id = ?1 ORDER BY added, endpoint")?;
            statement
                .query_map(params![claktory.id], |row| {
                    Ok(json!({"endpoint": row.get::<_, String>(0)?, "kind": row.get::<_, String>(1)?, "added": row.get::<_, i64>(2)?}))
                })?
                .collect::<rusqlite::Result<_>>()?
        };
        let unlocks: Vec<Value> =
            releases(conn, &claktory.id, u32::MAX)?.into_iter().map(|(time, ip)| json!({"time": time, "ip": ip})).collect();
        let usage: Vec<Value> = {
            let mut statement = conn.prepare(
                "SELECT day, provider, model, requests, input_tokens, output_tokens, cache_read_tokens, cache_write_tokens
                 FROM usage WHERE claktory_id = ?1 ORDER BY day, provider, model",
            )?;
            statement
                .query_map(params![claktory.id], |row| {
                    Ok(json!({
                        "day": row.get::<_, String>(0)?,
                        "provider": row.get::<_, String>(1)?,
                        "model": row.get::<_, String>(2)?,
                        "requests": row.get::<_, i64>(3)?,
                        "input_tokens": row.get::<_, i64>(4)?,
                        "output_tokens": row.get::<_, i64>(5)?,
                        "cache_read_tokens": row.get::<_, i64>(6)?,
                        "cache_write_tokens": row.get::<_, i64>(7)?,
                    }))
                })?
                .collect::<rusqlite::Result<_>>()?
        };
        let activity: Vec<Value> = {
            let mut statement =
                conn.prepare("SELECT hour, agent_minutes, agents_working, runs FROM activity WHERE claktory_id = ?1 ORDER BY hour")?;
            statement
                .query_map(params![claktory.id], |row| {
                    Ok(json!({
                        "hour": row.get::<_, i64>(0)?,
                        "agent_minutes": row.get::<_, i64>(1)?,
                        "agents_working": row.get::<_, i64>(2)?,
                        "runs": row.get::<_, i64>(3)?,
                    }))
                })?
                .collect::<rusqlite::Result<_>>()?
        };
        let table = |sql: &str, names: &[&str]| -> rusqlite::Result<Vec<Value>> {
            let mut statement = conn.prepare(sql)?;
            statement
                .query_map(params![claktory.id], |row| {
                    let mut object = serde_json::Map::new();
                    for (at, name) in names.iter().enumerate() {
                        let value = match row.get::<_, rusqlite::types::Value>(at)? {
                            rusqlite::types::Value::Integer(number) => json!(number),
                            rusqlite::types::Value::Text(text) => json!(text),
                            _ => Value::Null,
                        };
                        object.insert((*name).to_owned(), value);
                    }
                    Ok(Value::Object(object))
                })?
                .collect()
        };
        let devices_now = table(
            "SELECT platform, kind, app_version, count FROM claktory_devices WHERE claktory_id = ?1 ORDER BY platform, kind, app_version",
            &["platform", "kind", "app_version", "count"],
        )?;
        let devices_connected = table("SELECT day, count FROM devices_connected WHERE claktory_id = ?1 ORDER BY day", &["day", "count"])?;
        let health = table(
            "SELECT day, version, rolled_back, crashes FROM health WHERE claktory_id = ?1 ORDER BY day, version",
            &["day", "version", "rolled_back", "crashes"],
        )?;
        let setup: (Option<i64>, Option<i64>) =
            conn.query_row("SELECT model_connected, first_run FROM claktories WHERE id = ?1", params![claktory.id], |row| {
                Ok((row.get(0)?, row.get(1)?))
            })?;
        let days: Vec<String> = {
            let mut statement = conn.prepare("SELECT day FROM seen_days WHERE claktory_id = ?1 ORDER BY day")?;
            statement.query_map(params![claktory.id], |row| row.get(0))?.collect::<rusqlite::Result<_>>()?
        };
        claktories_out.push(json!({
            "id": claktory.id,
            "name": claktory.name,
            "backend_key": claktory.backend_key,
            "created": claktory.created,
            "last_contact": claktory.last_seen,
            "version": claktory.version,
            "platform": claktory.platform,
            "removed": claktory.revoked_at,
            "own_name": claktory.display_name,
            "color": claktory.color,
            "relays": claktory.relays,
            "counts": claktory.counts,
            "location": claktory.location,
            "endpoints": endpoints,
            "unlocks": unlocks,
            "usage": usage,
            "activity": activity,
            "setup": {"model_connected": setup.0 == Some(1), "first_run": setup.1 == Some(1)},
            "devices": devices_now,
            "devices_connected": devices_connected,
            "health": health,
            "days_in_touch": days,
        }));
    }
    let mut devices_out = Vec::new();
    for device in devices(conn, &account.id)? {
        let keys: Vec<Value> = {
            let mut statement = conn.prepare("SELECT device_key, issued FROM device_keys WHERE account_key = ?1 ORDER BY issued")?;
            statement
                .query_map(params![device.account_key], |row| {
                    Ok(json!({"device_key": row.get::<_, String>(0)?, "issued": row.get::<_, i64>(1)?}))
                })?
                .collect::<rusqlite::Result<_>>()?
        };
        devices_out.push(json!({
            "account_key": device.account_key,
            "name": device.name,
            "platform": device.platform,
            "version": device.version,
            "signed_in": device.created,
            "last_contact": device.last_seen,
            "assertions": keys,
        }));
    }
    Ok(json!({
        "account": {"id": account.id, "email": account.email, "created": account.created, "last_sign_in": account.last_sign_in},
        "claktories": claktories_out,
        "devices": devices_out,
    }))
}

pub fn insert_session(conn: &Connection, token: &str, email: &str, csrf: &str, expires: i64) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO sessions (token_hash, email, csrf, expires) VALUES (?1, ?2, ?3, ?4)",
        params![hash(token), email, csrf, expires],
    )?;
    Ok(())
}

/// A web session's (email, CSRF token), while it lasts.
pub fn session(conn: &Connection, token: &str, now: i64) -> rusqlite::Result<Option<(String, String)>> {
    conn.query_row("SELECT email, csrf FROM sessions WHERE token_hash = ?1 AND expires > ?2", params![hash(token), now], |row| {
        Ok((row.get(0)?, row.get(1)?))
    })
    .optional()
}

pub fn delete_session(conn: &Connection, token: &str) -> rusqlite::Result<()> {
    conn.execute("DELETE FROM sessions WHERE token_hash = ?1", params![hash(token)])?;
    Ok(())
}

/// Forgets what has expired: requests, sessions, old limit windows, share
/// releases (with their IPs) after 30 days, and device keys a day after
/// their last assertion.
pub fn purge(conn: &Connection, now: i64) -> rusqlite::Result<()> {
    if conn.execute("DELETE FROM device_keys WHERE issued < ?1", params![now - DEVICE_KEYS_KEPT])? > 0 {
        bump_allowlist(conn)?;
    }
    conn.execute("DELETE FROM requests WHERE expires <= ?1", params![now])?;
    conn.execute("DELETE FROM sessions WHERE expires <= ?1", params![now])?;
    conn.execute("DELETE FROM limits WHERE window < ?1", params![now - 2 * 86_400])?;
    conn.execute("DELETE FROM share_releases WHERE time < ?1", params![now - RELEASES_KEPT])?;
    conn.execute("DELETE FROM periods WHERE received < ?1", params![now - PERIODS_KEPT])?;
    Ok(())
}

/// How long a device key stays on the relay allowlist after its last
/// assertion: a day, many times an assertion's hour, so a device connected
/// through a relay isn't cut off while it renews.
pub const DEVICE_KEYS_KEPT: i64 = 86_400;

/// The most devices an account may have signed in.
pub const DEVICES_PER_ACCOUNT: u32 = 30;

/// A device signed in to an account, by its account key.
#[derive(Clone, Debug)]
pub struct Device {
    pub account_key: String,
    pub account_id: String,
    pub name: String,
    pub platform: String,
    pub version: String,
    /// When it signed in.
    pub created: i64,
    pub last_seen: i64,
}

const DEVICE: &str = "SELECT account_key, account_id, name, platform, version, created, last_seen FROM devices";

fn device_row(row: &rusqlite::Row) -> rusqlite::Result<Device> {
    Ok(Device {
        account_key: row.get(0)?,
        account_id: row.get(1)?,
        name: row.get(2)?,
        platform: row.get(3)?,
        version: row.get(4)?,
        created: row.get(5)?,
        last_seen: row.get(6)?,
    })
}

pub fn device(conn: &Connection, account_key: &str) -> rusqlite::Result<Option<Device>> {
    conn.query_row(&format!("{DEVICE} WHERE account_key = ?1"), params![account_key], device_row).optional()
}

/// An account's devices, the one in touch last first.
pub fn devices(conn: &Connection, account_id: &str) -> rusqlite::Result<Vec<Device>> {
    let mut statement = conn.prepare(&format!("{DEVICE} WHERE account_id = ?1 ORDER BY last_seen DESC, created DESC"))?;
    statement.query_map(params![account_id], device_row)?.collect()
}

/// A device was in touch.
pub fn touch_device(conn: &Connection, account_key: &str, now: i64) -> rusqlite::Result<()> {
    conn.execute("UPDATE devices SET last_seen = ?2 WHERE account_key = ?1", params![account_key, now])?;
    Ok(())
}

/// What a device's right code signs in to.
pub enum Bound {
    Done {
        account_id: String,
    },
    /// The account has as many devices as it may.
    TooMany,
}

/// Signs a device in from a request whose code was right: makes the
/// account if it's new, then binds the device's account key to it (moving
/// it from another account, whose assertions for it then stop counting).
pub fn bind_device(conn: &mut Connection, request: &Request, now: i64) -> rusqlite::Result<Bound> {
    let (Some(account_key), Some(name), Some(version), Some(platform)) =
        (&request.backend_key, &request.name, &request.version, &request.platform)
    else {
        return Err(rusqlite::Error::InvalidQuery);
    };
    let tx = conn.transaction()?;
    let account_id = account_for(&tx, &request.email, now)?;
    let others: u32 = tx.query_row(
        "SELECT count(*) FROM devices WHERE account_id = ?1 AND account_key != ?2",
        params![account_id, account_key],
        |row| row.get(0),
    )?;
    if others >= DEVICES_PER_ACCOUNT {
        tx.execute("DELETE FROM requests WHERE id = ?1", params![request.id])?;
        tx.commit()?;
        return Ok(Bound::TooMany);
    }
    let existing = device(&tx, account_key)?;
    let created = match &existing {
        Some(existing) if existing.account_id == account_id => existing.created,
        _ => now,
    };
    if existing.as_ref().is_some_and(|existing| existing.account_id != account_id)
        && tx.execute("DELETE FROM device_keys WHERE account_key = ?1", params![account_key])? > 0
    {
        bump_allowlist(&tx)?;
    }
    tx.execute(
        "INSERT INTO devices (account_key, account_id, name, platform, version, created, last_seen) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
         ON CONFLICT (account_key) DO UPDATE SET account_id = ?2, name = ?3, platform = ?4, version = ?5, created = ?6, last_seen = ?7",
        params![account_key, account_id, name, platform, version, created, now],
    )?;
    tx.execute("UPDATE accounts SET last_sign_in = ?2 WHERE id = ?1", params![account_id, now])?;
    tx.execute("DELETE FROM requests WHERE id = ?1", params![request.id])?;
    tx.commit()?;
    Ok(Bound::Done { account_id })
}

/// Signs a device out: its account key is unbound, and the keys it got
/// assertions for leave the allowlist. False when it wasn't signed in, or
/// (given `account_id`) isn't that account's.
pub fn unbind_device(conn: &mut Connection, account_key: &str, account_id: Option<&str>) -> rusqlite::Result<bool> {
    let tx = conn.transaction()?;
    let removed = match account_id {
        Some(account_id) => {
            tx.execute("DELETE FROM devices WHERE account_key = ?1 AND account_id = ?2", params![account_key, account_id])?
        }
        None => tx.execute("DELETE FROM devices WHERE account_key = ?1", params![account_key])?,
    };
    if removed > 0 && tx.execute("DELETE FROM device_keys WHERE account_key = ?1", params![account_key])? > 0 {
        bump_allowlist(&tx)?;
    }
    tx.commit()?;
    Ok(removed > 0)
}

/// Records that a device got an assertion for `device_key`, which puts the
/// key on the allowlist for a day.
pub fn issued_assertion(conn: &Connection, account_key: &str, device_key: &str, now: i64) -> rusqlite::Result<()> {
    let new = conn.execute(
        "INSERT OR IGNORE INTO device_keys (device_key, account_key, issued) VALUES (?1, ?2, ?3)",
        params![device_key, account_key, now],
    )?;
    if new > 0 {
        bump_allowlist(conn)?;
    } else {
        conn.execute(
            "UPDATE device_keys SET issued = ?3 WHERE device_key = ?1 AND account_key = ?2",
            params![device_key, account_key, now],
        )?;
    }
    Ok(())
}
