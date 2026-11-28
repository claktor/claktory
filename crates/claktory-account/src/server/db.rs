//! The service's database: one SQLite file. It holds no shares and no
//! tokens, only their hashes, so a copy of it opens no vault.

use std::path::Path;

use rusqlite::{Connection, OptionalExtension as _, params};
use serde_json::{Value, json};

use super::geo::Location;
use crate::protocol::{Activity, CheckIn, Counts, Health, PublicStats, RecoveryKey, Usage, clean_name, hex, sha256};

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
    first_run INTEGER,
    registered_at INTEGER
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
    epoch INTEGER NOT NULL,
    -- 1 when the account was emailed about it (#412); 0 when not.
    mailed INTEGER
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
-- Activity older than `ACTIVITY_HOURS_KEPT`, rolled up to its UTC days
-- (`roll_up`): minutes and runs summed, the most agents working at once.
CREATE TABLE IF NOT EXISTS activity_days (
    claktory_id TEXT NOT NULL,
    day TEXT NOT NULL,
    agent_minutes INTEGER NOT NULL,
    agents_working INTEGER NOT NULL,
    runs INTEGER NOT NULL,
    PRIMARY KEY (claktory_id, day)
) WITHOUT ROWID;
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
    last_seen INTEGER NOT NULL,
    signed_in INTEGER,
    -- The Claktory that vouched for it (#529), when it signed in through
    -- the Claktory on its own computer rather than by email.
    vouched_by TEXT
);
CREATE INDEX IF NOT EXISTS devices_account ON devices (account_id);
CREATE TABLE IF NOT EXISTS device_keys (
    device_key TEXT NOT NULL,
    account_key TEXT NOT NULL,
    issued INTEGER NOT NULL,
    PRIMARY KEY (device_key, account_key)
);
CREATE INDEX IF NOT EXISTS device_keys_account_key ON device_keys (account_key);
-- Device keys removed from the account (lost, stolen, given away), by
-- another of its devices or on the account page: never signed in to that
-- account again.
CREATE TABLE IF NOT EXISTS removed_devices (
    account_id TEXT NOT NULL,
    account_key TEXT NOT NULL,
    removed INTEGER NOT NULL,
    PRIMARY KEY (account_id, account_key)
);
CREATE INDEX IF NOT EXISTS removed_devices_key ON removed_devices (account_key);
-- The account each device key was last signed in to (row 671), kept after
-- it signs out: a key never signs in to another account, so its direct-
-- message leaf (the key) never spans two. Goes with the account.
CREATE TABLE IF NOT EXISTS key_accounts (
    account_key TEXT PRIMARY KEY,
    account_id TEXT NOT NULL
) WITHOUT ROWID;
CREATE INDEX IF NOT EXISTS key_accounts_account ON key_accounts (account_id);
-- Backend keys of Claktories that signed themselves out (#524): never
-- registered or taken as proof again, whoever holds a copy. Linked to no
-- Claktory or account.
CREATE TABLE IF NOT EXISTS retired_backend_keys (
    backend_key TEXT PRIMARY KEY,
    retired INTEGER NOT NULL
) WITHOUT ROWID;
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
-- Replies people reported (POST /v1/reports/ai), for Claktory's team to
-- read here; kept AI_REPORT_KEEP_DAYS. Nothing of who sent them, and
-- `received` only to the hour.
CREATE TABLE IF NOT EXISTS ai_reports (
    id INTEGER PRIMARY KEY,
    received INTEGER NOT NULL,
    text TEXT NOT NULL,
    model TEXT NOT NULL,
    version TEXT NOT NULL,
    platform TEXT NOT NULL,
    note TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS ai_reports_received ON ai_reports (received);
CREATE TABLE IF NOT EXISTS apps_purges (
    account_id TEXT PRIMARY KEY,
    queued INTEGER NOT NULL,
    tries INTEGER NOT NULL,
    next_try INTEGER NOT NULL
);
-- Devices signed out or removed that the user-apps worker hasn't heard of
-- yet (row 671), asked until it has, like apps_purges.
CREATE TABLE IF NOT EXISTS apps_sign_outs (
    account_id TEXT NOT NULL,
    account_key TEXT NOT NULL,
    signed_out INTEGER NOT NULL,
    tries INTEGER NOT NULL,
    next_try INTEGER NOT NULL,
    PRIMARY KEY (account_id, account_key)
) WITHOUT ROWID;
-- Claktories' registrations ended (removed, signed out, signed in again)
-- that the user-apps worker hasn't heard of yet (E11-4): it drops their
-- grants. Asked until it has, like apps_sign_outs.
CREATE TABLE IF NOT EXISTS apps_claktory_removals (
    account_id TEXT NOT NULL,
    claktory_id TEXT NOT NULL,
    epoch INTEGER NOT NULL,
    tries INTEGER NOT NULL,
    next_try INTEGER NOT NULL,
    PRIMARY KEY (account_id, claktory_id, epoch)
) WITHOUT ROWID;
-- Computers asking to join an account, for the person to approve on one
-- of their devices (#546): no code exists. state: waiting, approved,
-- declined, two_at_once (another asked meanwhile: neither joins), spent;
-- a move (kind 'move', #546 moves) also taken, between approved and spent.
-- A move's receive_relays (JSON), the device's move_approval as it came,
-- the source_claktory_id whose word came with it, and the source_key the
-- source named taking it. declined_by: the device that declined, as
-- approver is the one that approved.
CREATE TABLE IF NOT EXISTS authorizations (
    id TEXT PRIMARY KEY,
    kind TEXT NOT NULL,
    account_id TEXT NOT NULL,
    email TEXT NOT NULL,
    claktory_id TEXT NOT NULL,
    name TEXT NOT NULL,
    backend_key TEXT NOT NULL,
    version TEXT NOT NULL,
    platform TEXT NOT NULL,
    ip TEXT NOT NULL,
    network TEXT NOT NULL,
    place TEXT NOT NULL,
    started INTEGER NOT NULL,
    expires INTEGER NOT NULL,
    state TEXT NOT NULL,
    approver TEXT,
    decided INTEGER,
    receive_relays TEXT,
    move_approval TEXT,
    source_claktory_id TEXT,
    source_key TEXT,
    declined_by TEXT
);
CREATE INDEX IF NOT EXISTS authorizations_by_account ON authorizations (account_id, started);
-- Requests ended by a decline or by two at once, with the address that
-- ended them (#546), kept as share releases are.
CREATE TABLE IF NOT EXISTS authorization_log (
    account_id TEXT NOT NULL,
    request_id TEXT NOT NULL,
    what TEXT NOT NULL,
    ip TEXT NOT NULL,
    time INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS authorization_log_by_account ON authorization_log (account_id, time);
-- Account keys that ever signed in through a Claktory's vouch (#529),
-- kept past their sign-out: a DM identity one of them set up proves
-- nothing about the person (#546).
CREATE TABLE IF NOT EXISTS vouched_keys (
    account_key TEXT PRIMARY KEY,
    account_id TEXT NOT NULL,
    since INTEGER NOT NULL
) WITHOUT ROWID;
-- Each month's usage breakdowns for Charts (#411), published once after
-- it ends (`stats::publish`): counts only, folded and suppressed as
-- Fugo's rules say, never an id. Written once: never changed or deleted.
CREATE TABLE IF NOT EXISTS analytics_months (
    month TEXT PRIMARY KEY,
    -- When it was taken, unix seconds: its counts of what Claktories
    -- last reported are as of then.
    published_at INTEGER NOT NULL,
    -- Its rows, as JSON.
    rows TEXT NOT NULL
);
CREATE TRIGGER IF NOT EXISTS analytics_months_kept BEFORE UPDATE ON analytics_months
BEGIN SELECT RAISE(ABORT, 'a published month is never changed'); END;
CREATE TRIGGER IF NOT EXISTS analytics_months_never_deleted BEFORE DELETE ON analytics_months
BEGIN SELECT RAISE(ABORT, 'a published month is never deleted'); END;
-- Each account's recovery keys (#239), the last 10, the newest its key
-- now: only the public half, with the statement of the Claktory that set
-- it and the device's signature that approved it, that Claktory's id and
-- backend key then (to check it with) and when it was set. Deleted with
-- the account.
CREATE TABLE IF NOT EXISTS recovery_keys (
    id INTEGER PRIMARY KEY,
    account_id TEXT NOT NULL,
    public_key TEXT NOT NULL,
    statement TEXT NOT NULL,
    device_sig TEXT NOT NULL,
    claktory_id TEXT NOT NULL,
    backend_key TEXT NOT NULL,
    set_at INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS recovery_keys_by_account ON recovery_keys (account_id, id);
";

/// How long a period's id is kept, to know it if it's sent again. A
/// Claktory sends a period at most `PERIOD_AGE` after it starts, so no
/// period received earlier than that (and a day, for clocks) can come again.
pub const PERIODS_KEPT: i64 = PERIOD_AGE + 86_400;
/// The oldest a period may be when it's sent.
pub const PERIOD_AGE: i64 = 90 * 86_400;
/// How long each Claktory's activity is kept by the hour; then by the day
/// (`activity_days`), so the account disk doesn't grow by the hour forever.
pub const ACTIVITY_HOURS_KEPT: i64 = 90 * 86_400;

/// How long a share release, with its IP, is kept.
pub const RELEASES_KEPT: i64 = 30 * 86_400;

pub fn open(path: &Path) -> rusqlite::Result<Connection> {
    let mut conn = Connection::open(path)?;
    // Waiting for a lock before anything takes one: a connection still
    // writing (one about to close, a backup) mustn't fail the open.
    conn.busy_timeout(std::time::Duration::from_secs(5))?;
    conn.pragma_update(None, "journal_mode", "WAL")?;
    conn.pragma_update(None, "synchronous", "NORMAL")?;
    conn.pragma_update(None, "foreign_keys", "ON")?;
    // One transaction (SQLite's DDL is transactional): a crash midway
    // leaves the database as it was. It writes (user_version), so it takes
    // the write lock at once, waiting for it: a read turned write could
    // only be refused, not wait, once another connection wrote meanwhile.
    let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
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
            // When it last signed in (#412): NULL for those that signed in
            // before it was kept.
            "registered_at INTEGER",
        ] {
            let name = column.split(' ').next().unwrap_or(column);
            if !have.contains(name) {
                tx.execute_batch(&format!("ALTER TABLE claktories ADD COLUMN {column}"))?;
            }
        }
    }
    // Whether each unlock was emailed (#412): every one was before.
    let mailed: bool =
        tx.query_row("SELECT COUNT(*) > 0 FROM pragma_table_info('share_releases') WHERE name = 'mailed'", [], |row| row.get(0))?;
    if !mailed {
        tx.execute_batch("ALTER TABLE share_releases ADD COLUMN mailed INTEGER; UPDATE share_releases SET mailed = 1;")?;
    }
    // When a device last signed in with a code (row 630): added in place,
    // NULL for devices signed in before, which counts as long ago.
    let signed_in: bool =
        tx.query_row("SELECT COUNT(*) > 0 FROM pragma_table_info('devices') WHERE name = 'signed_in'", [], |row| row.get(0))?;
    if !signed_in {
        tx.execute_batch("ALTER TABLE devices ADD COLUMN signed_in INTEGER")?;
    }
    // The Claktory each vouched device signed in through (#529): NULL for
    // every device before, which all signed in by email.
    let vouched_by: bool =
        tx.query_row("SELECT COUNT(*) > 0 FROM pragma_table_info('devices') WHERE name = 'vouched_by'", [], |row| row.get(0))?;
    if !vouched_by {
        tx.execute_batch("ALTER TABLE devices ADD COLUMN vouched_by TEXT")?;
    }
    // A move's columns, and who declined: added in place, NULL for every
    // request before.
    for column in ["receive_relays TEXT", "move_approval TEXT", "source_claktory_id TEXT", "source_key TEXT", "declined_by TEXT"] {
        let name = column.split(' ').next().unwrap_or(column);
        let have: bool =
            tx.query_row("SELECT COUNT(*) > 0 FROM pragma_table_info('authorizations') WHERE name = ?1", params![name], |row| row.get(0))?;
        if !have {
            tx.execute_batch(&format!("ALTER TABLE authorizations ADD COLUMN {column}"))?;
        }
    }
    // Keys signed in, or removed, before the service kept each key's
    // account (row 671): theirs, so they don't sign in to another either.
    tx.execute_batch(
        "INSERT OR IGNORE INTO key_accounts (account_key, account_id) SELECT account_key, account_id FROM devices;
         INSERT OR IGNORE INTO key_accounts (account_key, account_id) SELECT account_key, account_id FROM removed_devices;",
    )?;
    if version < 5 {
        clean_names(&tx)?;
    }
    tx.pragma_update(None, "user_version", 5)?;
    tx.commit()?;
    Ok(conn)
}

/// Claktories' names kept before `clean_name` left out what they hide
/// (direction and invisible characters), as it would have kept them.
fn clean_names(tx: &rusqlite::Transaction<'_>) -> rusqlite::Result<()> {
    let names: Vec<(String, String, Option<String>)> = tx
        .prepare("SELECT id, name, display_name FROM claktories")?
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?
        .collect::<rusqlite::Result<_>>()?;
    for (id, name, display_name) in names {
        let cleaned = (clean_name(&name), display_name.as_deref().map(clean_name));
        if (cleaned.0.as_str(), cleaned.1.as_deref()) != (name.as_str(), display_name.as_deref()) {
            tx.execute("UPDATE claktories SET name = ?2, display_name = ?3 WHERE id = ?1", params![id, cleaned.0, cleaned.1])?;
        }
    }
    Ok(())
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

/// Keeps a reported reply (`server::reports`), received at the hour it
/// arrived in: no finer, so it can't be lined up with an address the
/// limits held then.
pub fn add_ai_report(conn: &Connection, report: &crate::protocol::AiReport, now: i64) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO ai_reports (received, text, model, version, platform, note) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![now - now.rem_euclid(3600), report.text, report.model, report.version, report.platform, report.note],
    )?;
    Ok(())
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
/// new computer's; the devices it vouched for, apps on the computer it
/// left, are signed out (#529). False when the token is another
/// Claktory's already.
#[allow(clippy::too_many_arguments)]
pub fn moved(
    conn: &Connection,
    id: &str,
    backend_key: &str,
    token: &str,
    name: &str,
    version: &str,
    platform: &str,
    apps: Option<i64>,
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
    vouches_end(conn, id, apps)?;
    Ok(true)
}

/// Reads a pair in column order, returning the first conversion error.
fn row2<A: rusqlite::types::FromSql, B: rusqlite::types::FromSql>(row: &rusqlite::Row<'_>) -> rusqlite::Result<(A, B)> {
    Ok((row.get(0)?, row.get(1)?))
}

/// Reads three columns in order, returning the first conversion error.
fn row3<A: rusqlite::types::FromSql, B: rusqlite::types::FromSql, C: rusqlite::types::FromSql>(
    row: &rusqlite::Row<'_>,
) -> rusqlite::Result<(A, B, C)> {
    Ok((row.get(0)?, row.get(1)?, row.get(2)?))
}

/// Collects an already prepared query, returning the first row error.
fn query<T>(
    mut statement: rusqlite::Statement<'_>,
    parameters: impl rusqlite::Params,
    read: impl FnMut(&rusqlite::Row<'_>) -> rusqlite::Result<T>,
) -> rusqlite::Result<Vec<T>> {
    statement.query_map(parameters, read)?.collect()
}

pub fn claktories(conn: &Connection, account_id: &str) -> rusqlite::Result<Vec<Claktory>> {
    query(conn.prepare(&format!("{CLAKTORY} WHERE account_id = ?1 ORDER BY created"))?, params![account_id], claktory_row)
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
pub fn register(conn: &mut Connection, request: &Request, token: &str, now: i64, proven: bool, apps: bool) -> rusqlite::Result<Registered> {
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
            // Signed in again without signing out first (here, or moving
            // to another account): the registration it had ends (E11-4).
            // A removed one's end was queued when it was removed.
            if apps && existing.revoked_at.is_none() {
                registration_ended(&tx, id, now)?;
            }
            if existing.account_id != account_id {
                // Removed by its old account, or moving from it: what that
                // account knew of it goes.
                tx.execute("DELETE FROM endpoints WHERE claktory_id = ?1", params![id])?;
                tx.execute("DELETE FROM share_releases WHERE claktory_id = ?1", params![id])?;
                anonymize(&tx, id, None)?;
                tx.execute(
                    "UPDATE claktories SET agents = NULL, projects = NULL, devices = NULL, country = NULL, region = NULL, city = NULL,
                     display_name = NULL, color = NULL, relays = NULL WHERE id = ?1",
                    params![id],
                )?;
            }
            let epoch = existing.epoch + 1;
            tx.execute(
                "UPDATE claktories SET account_id = ?2, name = ?3, backend_key = ?4, token_hash = ?5, epoch = ?6,
                 last_seen = ?7, version = ?8, platform = ?9, revoked_at = NULL, registered_at = ?7 WHERE id = ?1",
                params![id, account_id, name, backend_key, hash(token), epoch as i64, now, version, platform],
            )?;
            epoch
        }
        None => {
            tx.execute(
                "INSERT INTO claktories (id, account_id, name, backend_key, token_hash, epoch, created, last_seen, version, platform,
                 registered_at) VALUES (?1, ?2, ?3, ?4, ?5, 1, ?6, ?6, ?7, ?8, ?6)",
                params![id, account_id, name, backend_key, hash(token), now, version, platform],
            )?;
            1
        }
    };
    // Signed in afresh (after signing out, or to another account): the
    // devices it vouched for under its last sign-in are signed out (#529).
    vouches_end(&tx, id, apps.then_some(now))?;
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
/// touch count under a random stand-in of its own, with `account` (a
/// stand-in its deleted account's Claktories share, so the account counts
/// once) as its user, or its own stand-in again when None (it moved to
/// another account: unlinked from the one it left).
fn anonymize(conn: &Connection, id: &str, account: Option<&str>) -> rusqlite::Result<()> {
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
        "INSERT INTO activity_days (claktory_id, day, agent_minutes, agents_working, runs)
         SELECT '', day, agent_minutes, agents_working, runs FROM activity_days WHERE claktory_id = ?1 AND true
         ON CONFLICT (claktory_id, day) DO UPDATE SET
           agent_minutes = agent_minutes + excluded.agent_minutes,
           agents_working = agents_working + excluded.agents_working,
           runs = runs + excluded.runs",
        params![id],
    )?;
    conn.execute("DELETE FROM activity_days WHERE claktory_id = ?1", params![id])?;
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
    let own = stand_in();
    conn.execute(
        "UPDATE seen_days SET claktory_id = ?2, account_id = ?3 WHERE claktory_id = ?1",
        params![id, own, account.unwrap_or(&own)],
    )?;
    conn.execute("DELETE FROM periods WHERE claktory_id = ?1", params![id])?;
    Ok(())
}

/// What stands in for a Claktory or a user no longer there in the days it
/// was in touch: random, never derived from what it stands for.
fn stand_in() -> String {
    format!("{STAND_IN}{}", crate::protocol::b64(&crate::protocol::random_32()[..12]))
}

/// How every stand-in starts. A Claktory could choose an id that starts so
/// too, so a stand-in is one that also has no row in `claktories`.
pub const STAND_IN: &str = "deleted-";

/// A Claktory's usage since a day, per model: (the model's name as the app
/// shows it, or its id; requests; all tokens).
pub fn usage_since(conn: &Connection, id: &str, since: &str) -> rusqlite::Result<Vec<(String, i64, i64)>> {
    query(
        conn.prepare(
            "SELECT coalesce(models.name, usage.model), sum(requests), sum(input_tokens + output_tokens + cache_read_tokens + cache_write_tokens)
         FROM usage LEFT JOIN models ON models.provider = usage.provider AND models.model = usage.model
         WHERE claktory_id = ?1 AND day >= ?2 GROUP BY usage.provider, usage.model ORDER BY 3 DESC",
        )?,
        params![id, since],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
    )
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

/// Records a share release; its row, for `release_not_mailed`.
pub fn record_release(conn: &Connection, id: &str, now: i64, ip: &str, epoch: u64, mailed: bool) -> rusqlite::Result<i64> {
    conn.execute(
        "INSERT INTO share_releases (claktory_id, time, ip, epoch, mailed) VALUES (?1, ?2, ?3, ?4, ?5)",
        params![id, now, ip, epoch as i64, mailed],
    )?;
    Ok(conn.last_insert_rowid())
}

/// The release `row` was to be emailed, and the email couldn't be sent.
pub fn release_not_mailed(conn: &Connection, row: i64) -> rusqlite::Result<()> {
    conn.execute("UPDATE share_releases SET mailed = 0 WHERE rowid = ?1", params![row])?;
    Ok(())
}

/// A Claktory's kept releases, newest first: (time, IP, whether the
/// account was emailed about it).
pub fn releases(conn: &Connection, id: &str, most: u32) -> rusqlite::Result<Vec<(i64, String, bool)>> {
    query(
        conn.prepare("SELECT time, ip, coalesce(mailed, 1) FROM share_releases WHERE claktory_id = ?1 ORDER BY time DESC LIMIT ?2")?,
        params![id, most],
        row3,
    )
}

/// The first unlock after a sign-in this soon after it is the person's,
/// who has just proved their email (#412).
pub const SIGN_IN_UNLOCK: i64 = 15 * 60;

/// Why a share release is mailed to the account (#412, Fugo's rule): when
/// it doesn't look like the Claktory restarting where it always runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Unusual {
    /// From a network none of its unlocks in the last 30 days came from.
    NewNetwork,
    /// Asking for the epoch before the current one after the current one's
    /// share was released: a copy of its disk from before it last signed in.
    OlderCopy,
}

/// Whether releasing `claktory`'s share now, to `ip` at `epoch`, is
/// unusual, from its releases kept (the last 30 days) and when it last
/// signed in; None when it isn't, and nothing is mailed. Before the release
/// is recorded.
pub fn unusual_release(conn: &Connection, claktory: &Claktory, ip: &str, epoch: u64, now: i64) -> rusqlite::Result<Option<Unusual>> {
    let kept: Vec<(i64, String, i64)> = query(
        conn.prepare("SELECT time, ip, epoch FROM share_releases WHERE claktory_id = ?1 AND time > ?2")?,
        params![claktory.id, now - RELEASES_KEPT],
        row3,
    )?;
    if epoch < claktory.epoch && kept.iter().any(|(_, _, released)| *released as u64 == claktory.epoch) {
        return Ok(Some(Unusual::OlderCopy));
    }
    let registered: Option<i64> =
        conn.query_row("SELECT registered_at FROM claktories WHERE id = ?1", params![claktory.id], |row| row.get(0)).optional()?.flatten();
    if let Some(registered) = registered
        && now - registered <= SIGN_IN_UNLOCK
        && !kept.iter().any(|(time, _, _)| *time >= registered)
    {
        return Ok(None);
    }
    let here = network(ip);
    if kept.iter().any(|(_, released, _)| network(released) == here) {
        return Ok(None);
    }
    Ok(Some(Unusual::NewNetwork))
}

/// The network an IP is on, as the rule compares them: its IPv4 /24 or its
/// IPv6 /48 (an IPv4 address mapped into IPv6 as the IPv4 one); the text
/// itself when it isn't an IP.
pub fn network(ip: &str) -> String {
    match ip.parse::<std::net::IpAddr>() {
        Ok(std::net::IpAddr::V4(v4)) => v4_network(v4),
        Ok(std::net::IpAddr::V6(v6)) => match v6.to_ipv4_mapped() {
            Some(v4) => v4_network(v4),
            None => {
                let [a, b, c, ..] = v6.segments();
                format!("{a:x}:{b:x}:{c:x}::/48")
            }
        },
        Err(_) => ip.to_owned(),
    }
}

fn v4_network(ip: std::net::Ipv4Addr) -> String {
    let [a, b, c, _] = ip.octets();
    format!("{a}.{b}.{c}.0/24")
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
    let mut had: Vec<(String, String)> =
        query(tx.prepare("SELECT endpoint, kind FROM endpoints WHERE claktory_id = ?1")?, params![id], row2)?;
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
    query(
        conn.prepare(
            "SELECT endpoint FROM endpoints JOIN claktories ON claktories.id = endpoints.claktory_id
         WHERE claktories.revoked_at IS NULL
         UNION
         SELECT device_key FROM device_keys JOIN devices ON devices.account_key = device_keys.account_key
         ORDER BY 1",
        )?,
        [],
        |row| row.get(0),
    )
}

/// Removes a Claktory from its account, and signs out the devices it
/// vouched for (#529); false when it isn't the account's, or is removed
/// already. With `apps`, the user-apps worker is to hear of those, and of
/// the registration's end, so its agents' grants go (E11-4).
pub fn revoke(conn: &mut Connection, account_id: &str, id: &str, apps: bool, now: i64) -> rusqlite::Result<bool> {
    let tx = conn.transaction()?;
    let changed = tx.execute(
        "UPDATE claktories SET revoked_at = ?3 WHERE id = ?1 AND account_id = ?2 AND revoked_at IS NULL",
        params![id, account_id, now],
    )?;
    if changed > 0 {
        bump_allowlist(&tx)?;
        vouches_end(&tx, id, apps.then_some(now))?;
        if apps {
            registration_ended(&tx, id, now)?;
        }
    }
    tx.commit()?;
    Ok(changed > 0)
}

/// A Claktory signed itself out (#524), with `token`: its registration
/// ends as a removal does (`revoke`), its token stops matching any (so it's
/// `401 unauthorized` from then on, not `403 revoked`), and its backend key
/// is retired for good; the devices it vouched for are signed out (#529).
/// Nothing changes when the token is no longer its (it signed in again
/// meanwhile). With `apps`, the user-apps worker is to hear of those, and
/// of the registration's end, so its agents' grants go (E11-4).
pub fn sign_out_claktory(conn: &mut Connection, claktory: &Claktory, token: &str, apps: bool, now: i64) -> rusqlite::Result<()> {
    let tx = conn.transaction()?;
    // Never a token's hash (64 hex), and unique, as the column must be.
    let changed = tx.execute(
        "UPDATE claktories SET revoked_at = coalesce(revoked_at, ?2), token_hash = ?3 WHERE id = ?1 AND token_hash = ?4",
        params![claktory.id, now, format!("signed-out:{}", claktory.id), hash(token)],
    )?;
    if changed > 0 {
        tx.execute(
            "INSERT OR IGNORE INTO retired_backend_keys (backend_key, retired) VALUES (?1, ?2)",
            params![claktory.backend_key, now],
        )?;
        bump_allowlist(&tx)?;
        vouches_end(&tx, &claktory.id, apps.then_some(now))?;
        if apps {
            registration_ended(&tx, &claktory.id, now)?;
        }
    }
    tx.commit()
}

/// Queues telling the user-apps worker that `claktory_id`'s registration
/// now (its account and epoch, as the row has them) ended (E11-4), in the
/// caller's transaction, before the row changes.
fn registration_ended(tx: &Connection, claktory_id: &str, now: i64) -> rusqlite::Result<()> {
    tx.execute(
        "INSERT INTO apps_claktory_removals (account_id, claktory_id, epoch, tries, next_try)
         SELECT account_id, id, epoch, 0, ?2 FROM claktories WHERE id = ?1
         ON CONFLICT (account_id, claktory_id, epoch) DO UPDATE SET tries = 0, next_try = ?2",
        params![claktory_id, now],
    )?;
    Ok(())
}

/// Registrations ended that are due to be told to the user-apps worker at
/// `now` (at most `limit`): account, Claktory, epoch, and how often it was
/// asked already.
pub fn apps_claktory_removals_due(conn: &Connection, now: i64, limit: u32) -> rusqlite::Result<Vec<(String, String, u64, i64)>> {
    let mut statement = conn.prepare(
        "SELECT account_id, claktory_id, epoch, tries FROM apps_claktory_removals WHERE next_try <= ?1 ORDER BY next_try LIMIT ?2",
    )?;
    statement.query_map(params![now, limit], |row| Ok((row.get(0)?, row.get(1)?, row.get::<_, i64>(2)? as u64, row.get(3)?)))?.collect()
}

/// The worker heard that a registration ended: done.
pub fn apps_claktory_removed(conn: &Connection, account_id: &str, claktory_id: &str, epoch: u64) -> rusqlite::Result<()> {
    conn.execute(
        "DELETE FROM apps_claktory_removals WHERE account_id = ?1 AND claktory_id = ?2 AND epoch = ?3",
        params![account_id, claktory_id, epoch as i64],
    )?;
    Ok(())
}

/// The worker didn't hear of it: asked again at `next_try`.
pub fn apps_claktory_removal_failed(
    conn: &Connection,
    account_id: &str,
    claktory_id: &str,
    epoch: u64,
    next_try: i64,
) -> rusqlite::Result<()> {
    conn.execute(
        "UPDATE apps_claktory_removals SET tries = tries + 1, next_try = ?4 WHERE account_id = ?1 AND claktory_id = ?2 AND epoch = ?3",
        params![account_id, claktory_id, epoch as i64, next_try],
    )?;
    Ok(())
}

/// Whether a backend key was retired by its Claktory's sign-out.
pub fn backend_key_retired(conn: &Connection, backend_key: &str) -> rusqlite::Result<bool> {
    conn.query_row("SELECT EXISTS (SELECT 1 FROM retired_backend_keys WHERE backend_key = ?1)", params![backend_key], |row| row.get(0))
}

/// The codes `claktory_id` vouched with that weren't used yet no longer
/// work (#529).
pub fn forget_vouches(conn: &Connection, claktory_id: &str) -> rusqlite::Result<()> {
    conn.execute("DELETE FROM requests WHERE kind = 'vouch' AND claktory_id = ?1", params![claktory_id])?;
    Ok(())
}

/// What `claktory_id` vouched for ends: the devices signed in through it
/// are signed out, and its codes not used yet no longer work.
fn vouches_end(tx: &Connection, claktory_id: &str, apps: Option<i64>) -> rusqlite::Result<Vec<(String, String)>> {
    forget_vouches(tx, claktory_id)?;
    unbind_vouched(tx, claktory_id, None, apps)
}

/// Signs out the devices `claktory_id` vouched for, other than `except`:
/// (account, key) of each.
fn unbind_vouched(tx: &Connection, claktory_id: &str, except: Option<&str>, apps: Option<i64>) -> rusqlite::Result<Vec<(String, String)>> {
    let keys: Vec<String> =
        query(tx.prepare("SELECT account_key FROM devices WHERE vouched_by = ?1 ORDER BY account_key")?, params![claktory_id], |row| {
            row.get(0)
        })?;
    let mut unbound = Vec::new();
    for key in keys.into_iter().filter(|key| except != Some(key.as_str())) {
        if let Some(account) = unbind(tx, &key, None, apps)? {
            unbound.push((account, key));
        }
    }
    Ok(unbound)
}

/// Deletes an account and everything linked to it. With `apps` (the service
/// runs user-level apps), it's queued for the worker to forget too, in the
/// same transaction, so a deletion is never left half done.
pub fn delete_account(conn: &mut Connection, account: &Account, apps: bool, now: i64) -> rusqlite::Result<()> {
    let tx = conn.transaction()?;
    // One user in the days its Claktories were in touch, however many.
    let user = stand_in();
    for claktory in claktories(&tx, &account.id)? {
        anonymize(&tx, &claktory.id, Some(&user))?;
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
    tx.execute("DELETE FROM removed_devices WHERE account_id = ?1", params![account.id])?;
    tx.execute("DELETE FROM key_accounts WHERE account_id = ?1", params![account.id])?;
    tx.execute("DELETE FROM apps_sign_outs WHERE account_id = ?1", params![account.id])?;
    tx.execute("DELETE FROM apps_claktory_removals WHERE account_id = ?1", params![account.id])?;
    tx.execute("DELETE FROM recovery_keys WHERE account_id = ?1", params![account.id])?;
    tx.execute("DELETE FROM authorizations WHERE account_id = ?1", params![account.id])?;
    tx.execute("DELETE FROM authorization_log WHERE account_id = ?1", params![account.id])?;
    tx.execute("DELETE FROM vouched_keys WHERE account_id = ?1", params![account.id])?;
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
    query(
        conn.prepare("SELECT account_id, tries FROM apps_purges WHERE next_try <= ?1 ORDER BY next_try LIMIT ?2")?,
        params![now, limit],
        row2,
    )
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
        let endpoints: Vec<Value> = query(
            conn.prepare("SELECT endpoint, kind, added FROM endpoints WHERE claktory_id = ?1 ORDER BY added, endpoint")?,
            params![claktory.id],
            |row| Ok(json!({"endpoint": row.get::<_, String>(0)?, "kind": row.get::<_, String>(1)?, "added": row.get::<_, i64>(2)?})),
        )?;
        let unlocks: Vec<Value> =
            releases(conn, &claktory.id, u32::MAX)?
                .into_iter()
                .map(|(time, ip, mailed)| json!({"time": time, "ip": ip, "emailed": mailed}))
                .collect();
        let usage: Vec<Value> = query(
            conn.prepare(
                "SELECT day, provider, model, requests, input_tokens, output_tokens, cache_read_tokens, cache_write_tokens
                 FROM usage WHERE claktory_id = ?1 ORDER BY day, provider, model",
            )?,
            params![claktory.id],
            |row| {
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
            },
        )?;
        let activity: Vec<Value> = query(
            conn.prepare("SELECT hour, agent_minutes, agents_working, runs FROM activity WHERE claktory_id = ?1 ORDER BY hour")?,
            params![claktory.id],
            |row| {
                Ok(json!({
                    "hour": row.get::<_, i64>(0)?,
                    "agent_minutes": row.get::<_, i64>(1)?,
                    "agents_working": row.get::<_, i64>(2)?,
                    "runs": row.get::<_, i64>(3)?,
                }))
            },
        )?;
        let activity_days: Vec<Value> = query(
            conn.prepare("SELECT day, agent_minutes, agents_working, runs FROM activity_days WHERE claktory_id = ?1 ORDER BY day")?,
            params![claktory.id],
            |row| {
                Ok(json!({
                    "day": row.get::<_, String>(0)?,
                    "agent_minutes": row.get::<_, i64>(1)?,
                    "agents_working": row.get::<_, i64>(2)?,
                    "runs": row.get::<_, i64>(3)?,
                }))
            },
        )?;
        let table = |sql: &str, names: &[&str]| -> rusqlite::Result<Vec<Value>> {
            query(conn.prepare(sql)?, params![claktory.id], |row| {
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
            })
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
            conn.query_row("SELECT model_connected, first_run FROM claktories WHERE id = ?1", params![claktory.id], row2)?;
        let days: Vec<String> =
            query(conn.prepare("SELECT day FROM seen_days WHERE claktory_id = ?1 ORDER BY day")?, params![claktory.id], |row| row.get(0))?;
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
            "activity_days": activity_days,
            "setup": {"model_connected": setup.0 == Some(1), "first_run": setup.1 == Some(1)},
            "devices": devices_now,
            "devices_connected": devices_connected,
            "health": health,
            "days_in_touch": days,
        }));
    }
    let mut devices_out = Vec::new();
    for device in devices(conn, &account.id)? {
        let keys: Vec<Value> = query(
            conn.prepare("SELECT device_key, issued FROM device_keys WHERE account_key = ?1 ORDER BY issued")?,
            params![device.account_key],
            |row| Ok(json!({"device_key": row.get::<_, String>(0)?, "issued": row.get::<_, i64>(1)?})),
        )?;
        devices_out.push(json!({
            "account_key": device.account_key,
            "name": device.name,
            "platform": device.platform,
            "version": device.version,
            "signed_in": device.created,
            "last_contact": device.last_seen,
            "vouched_by": device.vouched_by,
            "assertions": keys,
        }));
    }
    let removed_out: Vec<Value> = query(
        conn.prepare("SELECT account_key, removed FROM removed_devices WHERE account_id = ?1 ORDER BY removed, account_key")?,
        params![account.id],
        |row| Ok(json!({"account_key": row.get::<_, String>(0)?, "removed": row.get::<_, i64>(1)?})),
    )?;
    // Every device key that last signed in to this account, signed out
    // since or not: none of them signs in to another (row 671).
    let keys_out: Vec<String> = query(
        conn.prepare("SELECT account_key FROM key_accounts WHERE account_id = ?1 ORDER BY account_key")?,
        params![account.id],
        |row| row.get(0),
    )?;
    Ok(json!({
        "account": {"id": account.id, "email": account.email, "created": account.created, "last_sign_in": account.last_sign_in},
        "claktories": claktories_out,
        "devices": devices_out,
        "removed_devices": removed_out,
        "device_keys": keys_out,
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
    conn.query_row("SELECT email, csrf FROM sessions WHERE token_hash = ?1 AND expires > ?2", params![hash(token), now], row2).optional()
}

pub fn delete_session(conn: &Connection, token: &str) -> rusqlite::Result<()> {
    conn.execute("DELETE FROM sessions WHERE token_hash = ?1", params![hash(token)])?;
    Ok(())
}

/// Forgets what has expired: requests, sessions, old limit windows, share
/// releases (with their IPs) after 30 days, and device keys a day after
/// their last assertion; and rolls old hourly activity up to days.
pub fn purge(conn: &Connection, now: i64) -> rusqlite::Result<()> {
    if conn.execute("DELETE FROM device_keys WHERE issued < ?1", params![now - DEVICE_KEYS_KEPT])? > 0 {
        bump_allowlist(conn)?;
    }
    conn.execute("DELETE FROM requests WHERE expires <= ?1", params![now])?;
    // Kept 10 minutes past their end, so a card can say why one ended.
    conn.execute("DELETE FROM authorizations WHERE expires <= ?1", params![now - crate::protocol::AUTHORIZE_LIFETIME])?;
    conn.execute("DELETE FROM sessions WHERE expires <= ?1", params![now])?;
    conn.execute("DELETE FROM limits WHERE window < ?1", params![now - 2 * 86_400])?;
    conn.execute("DELETE FROM share_releases WHERE time < ?1", params![now - RELEASES_KEPT])?;
    conn.execute("DELETE FROM authorization_log WHERE time < ?1", params![now - RELEASES_KEPT])?;
    conn.execute("DELETE FROM periods WHERE received < ?1", params![now - PERIODS_KEPT])?;
    roll_up(conn, now)?;
    conn.execute("DELETE FROM ai_reports WHERE received < ?1", params![now - crate::protocol::AI_REPORT_KEEP_DAYS * 86_400])?;
    Ok(())
}

/// Rolls each Claktory's hourly activity from before the UTC day
/// `ACTIVITY_HOURS_KEPT` ago up to its days (`activity_days`), in one
/// transaction: minutes and runs summed, `agents_working` the day's most.
/// An hour that comes in late, already past it, joins its day at the next
/// roll-up.
pub fn roll_up(conn: &Connection, now: i64) -> rusqlite::Result<usize> {
    let before = now - ACTIVITY_HOURS_KEPT;
    let before = before - before.rem_euclid(86_400);
    let tx = conn.unchecked_transaction()?;
    tx.execute(
        "INSERT INTO activity_days (claktory_id, day, agent_minutes, agents_working, runs)
         SELECT claktory_id, date(hour, 'unixepoch'), sum(agent_minutes), max(agents_working), sum(runs) FROM activity
         WHERE hour < ?1 GROUP BY claktory_id, date(hour, 'unixepoch')
         ON CONFLICT (claktory_id, day) DO UPDATE SET
           agent_minutes = agent_minutes + excluded.agent_minutes,
           agents_working = max(agents_working, excluded.agents_working),
           runs = runs + excluded.runs",
        params![before],
    )?;
    let rolled = tx.execute("DELETE FROM activity WHERE hour < ?1", params![before])?;
    tx.commit()?;
    Ok(rolled)
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
    /// When it last signed in with a code, the first time or again; None
    /// before the service kept it.
    pub signed_in: Option<i64>,
    /// The Claktory it signed in through (#529); None when it signed in by
    /// email.
    pub vouched_by: Option<String>,
}

const DEVICE: &str = "SELECT account_key, account_id, name, platform, version, created, last_seen, signed_in, vouched_by FROM devices";

fn device_row(row: &rusqlite::Row) -> rusqlite::Result<Device> {
    Ok(Device {
        account_key: row.get(0)?,
        account_id: row.get(1)?,
        name: row.get(2)?,
        platform: row.get(3)?,
        version: row.get(4)?,
        created: row.get(5)?,
        last_seen: row.get(6)?,
        signed_in: row.get(7)?,
        vouched_by: row.get(8)?,
    })
}

pub fn device(conn: &Connection, account_key: &str) -> rusqlite::Result<Option<Device>> {
    conn.query_row(&format!("{DEVICE} WHERE account_key = ?1"), params![account_key], device_row).optional()
}

/// An account's devices, the one in touch last first.
pub fn devices(conn: &Connection, account_id: &str) -> rusqlite::Result<Vec<Device>> {
    query(conn.prepare(&format!("{DEVICE} WHERE account_id = ?1 ORDER BY last_seen DESC, created DESC"))?, params![account_id], device_row)
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
    /// This key was removed from the account.
    Removed,
    /// This key was last signed in to another account (row 671).
    OtherAccount,
    /// The Claktory that vouched for it is no longer signed in to the
    /// account (removed, signed in again or elsewhere since): its code
    /// no longer counts.
    Unvouched,
}

/// Signs a device in from a request whose code was right: makes the
/// account if it's new, then binds the device's account key to it (moving
/// it from another account, whose assertions for it then stop counting).
/// A vouch's (#529) binds it as vouched by its Claktory, only while that
/// Claktory is still the account's, and signs out the key it vouched for
/// before; one by email binds it as not vouched. With `apps`, the
/// user-apps worker is to hear of a key signed out so.
pub fn bind_device(conn: &mut Connection, request: &Request, apps: bool, now: i64) -> rusqlite::Result<Bound> {
    let (Some(account_key), Some(name), Some(version), Some(platform)) =
        (&request.backend_key, &request.name, &request.version, &request.platform)
    else {
        return Err(rusqlite::Error::InvalidQuery);
    };
    let tx = conn.transaction()?;
    if device_removed(&tx, &request.email, account_key)? {
        tx.execute("DELETE FROM requests WHERE id = ?1", params![request.id])?;
        tx.commit()?;
        return Ok(Bound::Removed);
    }
    if key_of_another_account(&tx, &request.email, account_key)? {
        tx.execute("DELETE FROM requests WHERE id = ?1", params![request.id])?;
        tx.commit()?;
        return Ok(Bound::OtherAccount);
    }
    let account_id = account_for(&tx, &request.email, now)?;
    let vouched_by = if request.kind == "vouch" {
        let by = request.claktory_id.clone().unwrap_or_default();
        let current: bool = tx.query_row(
            "SELECT EXISTS (SELECT 1 FROM claktories WHERE id = ?1 AND account_id = ?2 AND revoked_at IS NULL)",
            params![by, account_id],
            |row| row.get(0),
        )?;
        if !current {
            tx.execute("DELETE FROM requests WHERE id = ?1", params![request.id])?;
            tx.commit()?;
            return Ok(Bound::Unvouched);
        }
        Some(by)
    } else {
        None
    };
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
    if let Some(by) = &vouched_by {
        // One live key per Claktory: the one it vouched for before goes.
        unbind_vouched(&tx, by, Some(account_key), apps.then_some(now))?;
        tx.execute(
            "INSERT INTO vouched_keys (account_key, account_id, since) VALUES (?1, ?2, ?3) ON CONFLICT (account_key) DO NOTHING",
            params![account_key, account_id, now],
        )?;
    }
    tx.execute(
        "INSERT INTO devices (account_key, account_id, name, platform, version, created, last_seen, signed_in, vouched_by)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?7, ?8)
         ON CONFLICT (account_key) DO UPDATE SET account_id = ?2, name = ?3, platform = ?4, version = ?5, created = ?6, last_seen = ?7,
         signed_in = ?7, vouched_by = ?8",
        params![account_key, account_id, name, platform, version, created, now, vouched_by],
    )?;
    tx.execute(
        "INSERT INTO key_accounts (account_key, account_id) VALUES (?1, ?2) ON CONFLICT (account_key) DO UPDATE SET account_id = ?2",
        params![account_key, account_id],
    )?;
    tx.execute("UPDATE accounts SET last_sign_in = ?2 WHERE id = ?1", params![account_id, now])?;
    tx.execute("DELETE FROM requests WHERE id = ?1", params![request.id])?;
    tx.commit()?;
    Ok(Bound::Done { account_id })
}

/// Signs a device out: its account key is unbound, and the keys it got
/// assertions for leave the allowlist. The account it was signed in to;
/// none when it wasn't signed in, or (given `account_id`) isn't that
/// account's. With `apps`, the user-apps worker is to hear of it
/// (`apps_sign_outs`).
pub fn unbind_device(conn: &mut Connection, account_key: &str, account_id: Option<&str>, apps: bool, now: i64) -> rusqlite::Result<Option<String>> {
    let tx = conn.transaction()?;
    let unbound = unbind(&tx, account_key, account_id, apps.then_some(now))?;
    tx.commit()?;
    Ok(unbound)
}

/// Removes a device from an account for good (another of its devices, or
/// the account page, asks): the key is unbound (if it's the account's) and
/// remembered, so it never signs in to that account again. Remembered also when it isn't
/// bound now (signed out already, say). False when it wasn't the account's.
pub fn remove_device(conn: &mut Connection, account_key: &str, account_id: &str, apps: bool, now: i64) -> rusqlite::Result<bool> {
    let tx = conn.transaction()?;
    tx.execute(
        "INSERT INTO removed_devices (account_id, account_key, removed) VALUES (?1, ?2, ?3) ON CONFLICT DO NOTHING",
        params![account_id, account_key, now],
    )?;
    let removed = unbind(&tx, account_key, Some(account_id), apps.then_some(now))?.is_some();
    tx.commit()?;
    Ok(removed)
}

/// The device keys removed from an account, for the user-apps worker.
pub fn removed_devices(conn: &Connection, account_id: &str) -> rusqlite::Result<Vec<String>> {
    let mut statement = conn.prepare("SELECT account_key FROM removed_devices WHERE account_id = ?1 ORDER BY account_key")?;
    statement.query_map([account_id], |row| row.get(0))?.collect()
}

/// The device keys removed from an account since `after` (unix seconds;
/// all of them when None), for its Claktories to unpair them (E11-8),
/// oldest first. Those removed in the few minutes before `after` count too,
/// so a removal made in the same second, or under a clock that stepped
/// back, is never missed; unpairing a key twice is harmless.
pub fn removed_device_keys(conn: &Connection, account_id: &str, after: Option<i64>) -> rusqlite::Result<Vec<String>> {
    let mut statement =
        conn.prepare("SELECT account_key FROM removed_devices WHERE account_id = ?1 AND removed >= ?2 ORDER BY removed, account_key")?;
    let from = after.map_or(i64::MIN, |after| after.saturating_sub(REMOVED_OVERLAP));
    statement.query_map(params![account_id, from], |row| row.get(0))?.collect()
}

/// How far before a check-in's cursor removed device keys are sent again.
const REMOVED_OVERLAP: i64 = 300;

/// Whether this key was removed from any account (exactly this key: the
/// column compares bytes).
pub fn key_removed(conn: &Connection, account_key: &str) -> rusqlite::Result<bool> {
    conn.query_row("SELECT EXISTS (SELECT 1 FROM removed_devices WHERE account_key = ?1)", params![account_key], |row| row.get(0))
}

/// Whether this key was last signed in to an account other than the one
/// with this email, or than a new one (row 671): signed in there now or
/// signed out since, it never signs in anywhere else.
pub fn key_of_another_account(conn: &Connection, email: &str, account_key: &str) -> rusqlite::Result<bool> {
    let last: Option<String> =
        conn.query_row("SELECT account_id FROM key_accounts WHERE account_key = ?1", params![account_key], |row| row.get(0)).optional()?;
    let Some(last) = last else { return Ok(false) };
    let this: Option<String> = conn.query_row("SELECT id FROM accounts WHERE email = ?1", params![email], |row| row.get(0)).optional()?;
    Ok(this.as_deref() != Some(last.as_str()))
}

/// Whether this key was removed from the account with this email.
pub fn device_removed(conn: &Connection, email: &str, account_key: &str) -> rusqlite::Result<bool> {
    conn.query_row(
        "SELECT EXISTS (SELECT 1 FROM removed_devices JOIN accounts ON accounts.id = removed_devices.account_id
         WHERE accounts.email = ?1 AND removed_devices.account_key = ?2)",
        params![email, account_key],
        |row| row.get(0),
    )
}

fn unbind(tx: &Connection, account_key: &str, account_id: Option<&str>, apps: Option<i64>) -> rusqlite::Result<Option<String>> {
    let bound = device(tx, account_key)?.map(|device| device.account_id).filter(|bound| account_id.is_none_or(|account_id| account_id == bound));
    let Some(bound) = bound else { return Ok(None) };
    tx.execute("DELETE FROM devices WHERE account_key = ?1", params![account_key])?;
    if tx.execute("DELETE FROM device_keys WHERE account_key = ?1", params![account_key])? > 0 {
        bump_allowlist(tx)?;
    }
    if let Some(now) = apps {
        tx.execute(
            "INSERT INTO apps_sign_outs (account_id, account_key, signed_out, tries, next_try) VALUES (?1, ?2, ?3, 0, ?3)
             ON CONFLICT (account_id, account_key) DO UPDATE SET signed_out = ?3, tries = 0, next_try = ?3",
            params![bound, account_key, now],
        )?;
    }
    Ok(Some(bound))
}

/// Signed-out devices due to be told to the user-apps worker at `now` (at
/// most `limit`): account, key, when, and how often it was asked already.
pub fn apps_sign_outs_due(conn: &Connection, now: i64, limit: u32) -> rusqlite::Result<Vec<(String, String, i64, i64)>> {
    let mut statement = conn.prepare(
        "SELECT account_id, account_key, signed_out, tries FROM apps_sign_outs WHERE next_try <= ?1 ORDER BY next_try LIMIT ?2",
    )?;
    statement.query_map(params![now, limit], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)))?.collect()
}

/// The worker heard of that sign-out: done, unless the device signed out
/// again since (signed back in meanwhile), which it's still to hear of.
pub fn apps_signed_out(conn: &Connection, account_id: &str, account_key: &str, signed_out: i64) -> rusqlite::Result<()> {
    conn.execute(
        "DELETE FROM apps_sign_outs WHERE account_id = ?1 AND account_key = ?2 AND signed_out = ?3",
        params![account_id, account_key, signed_out],
    )?;
    Ok(())
}

/// The worker didn't hear of it: asked again at `next_try`.
pub fn apps_sign_out_failed(conn: &Connection, account_id: &str, account_key: &str, next_try: i64) -> rusqlite::Result<()> {
    conn.execute(
        "UPDATE apps_sign_outs SET tries = tries + 1, next_try = ?3 WHERE account_id = ?1 AND account_key = ?2",
        params![account_id, account_key, next_try],
    )?;
    Ok(())
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

/// How many of an account's recovery keys are kept: its key now and the
/// chain before it.
const RECOVERY_KEYS_KEPT: i64 = 10;

/// The account's recovery key (#239), if it has one, with the ones before it.
pub fn recovery_key(conn: &Connection, account_id: &str) -> rusqlite::Result<Option<RecoveryKey>> {
    let mut keys: Vec<RecoveryKey> = conn
        .prepare(
            // Its Claktory's name as GET /v1/claktories gives it, while
            // it's still the account's.
            "SELECT k.public_key, k.statement, k.device_sig, k.claktory_id, k.backend_key, k.set_at,
                    COALESCE(c.display_name, c.name, '')
             FROM recovery_keys k
             LEFT JOIN claktories c ON c.id = k.claktory_id AND c.account_id = k.account_id AND c.revoked_at IS NULL
             WHERE k.account_id = ?1 ORDER BY k.id DESC LIMIT ?2",
        )?
        .query_map(params![account_id, RECOVERY_KEYS_KEPT], |row| {
            Ok(RecoveryKey {
                public_key: row.get(0)?,
                statement: row.get(1)?,
                device_sig: row.get(2)?,
                claktory_id: row.get(3)?,
                claktory_name: row.get(6)?,
                backend_key: row.get(4)?,
                set_at: row.get(5)?,
                history: Vec::new(),
            })
        })?
        .collect::<rusqlite::Result<_>>()?;
    if keys.is_empty() {
        return Ok(None);
    }
    let mut now = keys.remove(0);
    now.history = keys;
    Ok(Some(now))
}

/// Sets the account's recovery key to `key` if the one it has is
/// `replaces` (`""`: none), or is `key`'s already (a retry): the key it
/// has then. Otherwise nothing changes, and Err gives the one it has.
pub fn set_recovery_key(
    conn: &mut Connection,
    account_id: &str,
    replaces: &str,
    key: &RecoveryKey,
) -> rusqlite::Result<Result<RecoveryKey, Option<RecoveryKey>>> {
    let tx = conn.transaction()?;
    let now = recovery_key(&tx, account_id)?;
    if let Some(now) = &now
        && now.public_key == key.public_key
    {
        return Ok(Ok(now.clone()));
    }
    if now.as_ref().map_or("", |now| now.public_key.as_str()) != replaces {
        return Ok(Err(now));
    }
    tx.execute(
        "INSERT INTO recovery_keys (account_id, public_key, statement, device_sig, claktory_id, backend_key, set_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![account_id, key.public_key, key.statement, key.device_sig, key.claktory_id, key.backend_key, key.set_at],
    )?;
    tx.execute(
        "DELETE FROM recovery_keys WHERE account_id = ?1 AND id NOT IN
         (SELECT id FROM recovery_keys WHERE account_id = ?1 ORDER BY id DESC LIMIT ?2)",
        params![account_id, RECOVERY_KEYS_KEPT],
    )?;
    let set = recovery_key(&tx, account_id)?;
    tx.commit()?;
    Ok(set.ok_or(None))
}

/// A computer's request to join an account, as kept.
#[derive(Clone, Debug, PartialEq)]
pub struct Authorization {
    pub id: String,
    pub kind: String,
    pub account_id: String,
    pub email: String,
    pub claktory_id: String,
    pub name: String,
    pub backend_key: String,
    pub version: String,
    pub platform: String,
    pub ip: String,
    pub network: String,
    pub place: String,
    pub started: i64,
    pub expires: i64,
    pub state: String,
    pub approver: Option<String>,
    pub decided: Option<i64>,
    /// A move's: the relays its destination is reachable through, the
    /// device's approval as it came, the Claktory whose word came with it
    /// (the source), and the endpoint the source named taking it. Empty
    /// for a join.
    pub receive_relays: Vec<String>,
    pub move_approval: Option<String>,
    pub source_claktory_id: Option<String>,
    pub source_key: Option<String>,
    /// The device that declined it (its account key).
    pub declined_by: Option<String>,
}

const AUTHORIZATION: &str = "SELECT id, kind, account_id, email, claktory_id, name, backend_key, version, platform, ip, network, place,
    started, expires, state, approver, decided, receive_relays, move_approval, source_claktory_id, source_key, declined_by
    FROM authorizations";

fn authorization_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Authorization> {
    Ok(Authorization {
        id: row.get(0)?,
        kind: row.get(1)?,
        account_id: row.get(2)?,
        email: row.get(3)?,
        claktory_id: row.get(4)?,
        name: row.get(5)?,
        backend_key: row.get(6)?,
        version: row.get(7)?,
        platform: row.get(8)?,
        ip: row.get(9)?,
        network: row.get(10)?,
        place: row.get(11)?,
        started: row.get(12)?,
        expires: row.get(13)?,
        state: row.get(14)?,
        approver: row.get(15)?,
        decided: row.get(16)?,
        receive_relays: row.get::<_, Option<String>>(17)?.and_then(|relays| serde_json::from_str(&relays).ok()).unwrap_or_default(),
        move_approval: row.get(18)?,
        source_claktory_id: row.get(19)?,
        source_key: row.get(20)?,
        declined_by: row.get(21)?,
    })
}

/// Whether a computer asking to join `account_id` has anyone to approve
/// it: the account has a Claktory signed in (whose word an approver
/// brings) and a device signed in by email itself.
pub fn may_be_approved(conn: &Connection, account_id: &str) -> rusqlite::Result<bool> {
    conn.query_row(
        "SELECT EXISTS (SELECT 1 FROM claktories WHERE account_id = ?1 AND revoked_at IS NULL)
            AND EXISTS (SELECT 1 FROM devices WHERE account_id = ?1 AND vouched_by IS NULL)",
        params![account_id],
        |row| row.get(0),
    )
}

/// Whether this key ever signed in through a Claktory's vouch.
pub fn ever_vouched(conn: &Connection, account_key: &str) -> rusqlite::Result<bool> {
    conn.query_row("SELECT EXISTS (SELECT 1 FROM vouched_keys WHERE account_key = ?1)", params![account_key], |row| row.get(0))
}

/// How a computer's request to join was taken.
#[derive(Debug, PartialEq, Eq)]
pub enum Asked {
    /// It waits for the person's approval.
    Waiting,
    /// Another was waiting for the account: both are cancelled, and both
    /// say why.
    TwoAtOnce,
}

/// Keeps a computer's request to join (its `id`, `started` and `expires`
/// as given), one waiting per account: with another waiting, both end as
/// `two_at_once`.
pub fn ask_to_join(conn: &mut Connection, asked: &Authorization) -> rusqlite::Result<Asked> {
    let tx = conn.transaction()?;
    let cancelled = tx.execute(
        "UPDATE authorizations SET state = 'two_at_once', decided = ?2 WHERE account_id = ?1 AND state = 'waiting' AND expires > ?2",
        params![asked.account_id, asked.started],
    )?;
    let state = if cancelled > 0 { "two_at_once" } else { "waiting" };
    tx.execute(
        "INSERT INTO authorizations (id, kind, account_id, email, claktory_id, name, backend_key, version, platform, ip, network, place,
         started, expires, state, decided, receive_relays) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17)",
        params![
            asked.id,
            asked.kind,
            asked.account_id,
            asked.email,
            asked.claktory_id,
            asked.name,
            asked.backend_key,
            asked.version,
            asked.platform,
            asked.ip,
            asked.network,
            asked.place,
            asked.started,
            asked.expires,
            state,
            (cancelled > 0).then_some(asked.started),
            (!asked.receive_relays.is_empty()).then(|| serde_json::to_string(&asked.receive_relays).unwrap_or_default()),
        ],
    )?;
    tx.commit()?;
    Ok(if cancelled > 0 { Asked::TwoAtOnce } else { Asked::Waiting })
}

pub fn authorization(conn: &Connection, id: &str) -> rusqlite::Result<Option<Authorization>> {
    conn.query_row(&format!("{AUTHORIZATION} WHERE id = ?1"), params![id], authorization_row).optional()
}

/// What an account's devices are shown at `now` of `kind` ("claktory"
/// for joins, "move" for moves, which are only in Settings → Move): its
/// request waiting, and those cancelled as two at once in the last 10
/// minutes, oldest first; for joins, those answered in the last 10 minutes
/// too (approved, spent since, or declined), so a card left open on
/// another device can say how.
pub fn authorizations_shown(conn: &Connection, account_id: &str, kind: &str, now: i64) -> rusqlite::Result<Vec<Authorization>> {
    query(
        conn.prepare(&format!(
            "{AUTHORIZATION} WHERE account_id = ?1 AND kind = ?4
             AND ((state = 'waiting' AND expires > ?2) OR (state = 'two_at_once' AND decided > ?2 - ?3)
                  OR (kind <> 'move' AND state IN ('approved', 'spent', 'declined') AND decided > ?2 - ?3))
             ORDER BY started, id"
        ))?,
        params![account_id, now, crate::protocol::AUTHORIZE_LIFETIME, kind],
        authorization_row,
    )
}

/// The account's request to join waiting at `now`, by id: never a move,
/// which rings no bell.
pub fn authorizations_waiting(conn: &Connection, account_id: &str, now: i64) -> rusqlite::Result<Vec<String>> {
    query(
        conn.prepare(
            "SELECT id FROM authorizations WHERE account_id = ?1 AND kind <> 'move' AND state = 'waiting' AND expires > ?2
             ORDER BY started, id",
        )?,
        params![account_id, now],
        |row| row.get(0),
    )
}

/// The moves approved at `now` with `claktory_id` as their source, not
/// taken yet, oldest first.
pub fn moves_approved(conn: &Connection, claktory_id: &str, now: i64) -> rusqlite::Result<Vec<Authorization>> {
    query(
        conn.prepare(&format!(
            "{AUTHORIZATION} WHERE kind = 'move' AND state = 'approved' AND source_claktory_id = ?1 AND expires > ?2 ORDER BY started, id"
        ))?,
        params![claktory_id, now],
        authorization_row,
    )
}

/// The person approved a move still waiting at `now`, with the device
/// `approver`, its own approval as it came and the Claktory whose word came
/// with it (the source); false when it no longer waits.
pub fn approve_move(
    conn: &Connection,
    id: &str,
    approver: &str,
    approval: &str,
    source_claktory_id: &str,
    now: i64,
) -> rusqlite::Result<bool> {
    let changed = conn.execute(
        "UPDATE authorizations SET state = 'approved', approver = ?2, decided = ?3, move_approval = ?4, source_claktory_id = ?5
         WHERE id = ?1 AND kind = 'move' AND state = 'waiting' AND expires > ?3",
        params![id, approver, now, approval, source_claktory_id],
    )?;
    Ok(changed > 0)
}

/// Its source takes an approved move at `now`, once, naming the endpoint it
/// dials from: false when it isn't approved for that Claktory, was taken
/// already, or ran out.
pub fn take_move(conn: &Connection, id: &str, claktory_id: &str, source_key: &str, now: i64) -> rusqlite::Result<bool> {
    let changed = conn.execute(
        "UPDATE authorizations SET state = 'taken', source_key = ?3
         WHERE id = ?1 AND kind = 'move' AND state = 'approved' AND source_claktory_id = ?2 AND expires > ?4",
        params![id, claktory_id, source_key, now],
    )?;
    Ok(changed > 0)
}

/// Spends a move its source took, once, as its destination hears of it:
/// false when it isn't taken (or was spent already).
pub fn spend_move(conn: &Connection, id: &str) -> rusqlite::Result<bool> {
    Ok(conn.execute("UPDATE authorizations SET state = 'spent' WHERE id = ?1 AND kind = 'move' AND state = 'taken'", params![id])? > 0)
}

/// When a Claktory was last registered (signed in afresh); None for one
/// from before the service kept it.
pub fn registered_at(conn: &Connection, claktory_id: &str) -> rusqlite::Result<Option<i64>> {
    Ok(conn.query_row("SELECT registered_at FROM claktories WHERE id = ?1", params![claktory_id], |row| row.get(0)).optional()?.flatten())
}

/// Records a computer's request ended by the person's decline, or by
/// another asking at the same time, with the address that did it, as
/// share releases are kept.
pub fn log_authorization(conn: &Connection, account_id: &str, request_id: &str, what: &str, ip: &str, now: i64) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO authorization_log (account_id, request_id, what, ip, time) VALUES (?1, ?2, ?3, ?4, ?5)",
        params![account_id, request_id, what, ip, now],
    )?;
    Ok(())
}

/// The person approved or declined a request still waiting at `now`, with
/// the device `by` (its account key); false when it no longer waits.
pub fn decide_authorization(conn: &Connection, id: &str, approve: bool, by: &str, now: i64) -> rusqlite::Result<bool> {
    let (state, approver, declined_by) = if approve { ("approved", Some(by), None) } else { ("declined", None, Some(by)) };
    let changed = conn.execute(
        "UPDATE authorizations SET state = ?2, approver = ?3, declined_by = ?5, decided = ?4
         WHERE id = ?1 AND state = 'waiting' AND expires > ?4",
        params![id, state, approver, now, declined_by],
    )?;
    Ok(changed > 0)
}

/// Spends an approved request, once: false when it isn't approved (or was
/// spent already).
pub fn spend_authorization(conn: &Connection, id: &str) -> rusqlite::Result<bool> {
    Ok(conn.execute("UPDATE authorizations SET state = 'spent' WHERE id = ?1 AND kind <> 'move' AND state = 'approved'", params![id])? > 0)
}
