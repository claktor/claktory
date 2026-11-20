//! The service's database: one SQLite file. It holds no shares and no
//! tokens, only their hashes, so a copy of it opens no vault.

use std::path::Path;

use rusqlite::{Connection, OptionalExtension as _, params};
use serde_json::{Value, json};

use super::geo::Location;
use crate::protocol::{Activity, CheckIn, Counts, PublicStats, Usage, hex, sha256};

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
    city TEXT
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
CREATE TABLE IF NOT EXISTS meta (
    key TEXT PRIMARY KEY,
    value INTEGER NOT NULL
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
    let version: i64 = conn.pragma_query_value(None, "user_version", |row| row.get(0))?;
    conn.execute_batch(SCHEMA)?;
    if version == 1 {
        // The first version's claktories had no counts or location.
        for column in ["agents INTEGER", "projects INTEGER", "devices INTEGER", "country TEXT", "region TEXT", "city TEXT"] {
            conn.execute_batch(&format!("ALTER TABLE claktories ADD COLUMN {column}"))?;
        }
    }
    conn.pragma_update(None, "user_version", 2)?;
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
    let mut wait = 0;
    for (key, period, most) in limits {
        let window = now - now.rem_euclid(*period);
        if count_in(conn, key, window)? >= *most {
            wait = wait.max(window + period - now);
        }
    }
    if wait > 0 {
        return Ok(Err(wait));
    }
    for (key, period, _) in limits {
        bump(conn, key, now - now.rem_euclid(*period))?;
    }
    Ok(Ok(()))
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
}

const CLAKTORY: &str = "SELECT id, account_id, name, backend_key, epoch, created, last_seen, version, platform, revoked_at,
    agents, projects, devices, country, region, city FROM claktories";

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
            (Some(agents), Some(projects), Some(devices)) => Some(Counts { agents, projects, devices }),
            _ => None,
        },
        location: Location { country: row.get(13)?, region: row.get(14)?, city: row.get(15)? },
    })
}

pub fn claktory(conn: &Connection, id: &str) -> rusqlite::Result<Option<Claktory>> {
    conn.query_row(&format!("{CLAKTORY} WHERE id = ?1"), params![id], claktory_row).optional()
}

pub fn claktory_by_token(conn: &Connection, token: &str) -> rusqlite::Result<Option<Claktory>> {
    conn.query_row(&format!("{CLAKTORY} WHERE token_hash = ?1"), params![hash(token)], claktory_row).optional()
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
/// again a new token, backend key and epoch, undoing a removal.
pub fn register(conn: &mut Connection, request: &Request, token: &str, now: i64) -> rusqlite::Result<Registered> {
    let (Some(id), Some(name), Some(backend_key), Some(version), Some(platform)) =
        (&request.claktory_id, &request.name, &request.backend_key, &request.version, &request.platform)
    else {
        return Err(rusqlite::Error::InvalidQuery);
    };
    let tx = conn.transaction()?;
    let account_id = match account_by_email(&tx, &request.email)? {
        Some(account) => account.id,
        None => {
            let id = crate::protocol::b64(*crate::protocol::random_32());
            tx.execute("INSERT INTO accounts (id, email, created, last_sign_in) VALUES (?1, ?2, ?3, ?3)", params![id, request.email, now])?;
            id
        }
    };
    let existing = claktory(&tx, id)?;
    if let Some(existing) = &existing
        && existing.account_id != account_id
        && existing.revoked_at.is_none()
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
                // Removed by its old account: what that account knew of it goes.
                tx.execute("DELETE FROM endpoints WHERE claktory_id = ?1", params![id])?;
                tx.execute("DELETE FROM share_releases WHERE claktory_id = ?1", params![id])?;
                anonymize(&tx, id)?;
                tx.execute(
                    "UPDATE claktories SET agents = NULL, projects = NULL, devices = NULL, country = NULL, region = NULL, city = NULL WHERE id = ?1",
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
            "UPDATE claktories SET agents = ?2, projects = ?3, devices = ?4 WHERE id = ?1",
            params![claktory.id, counts.agents, counts.projects, counts.devices],
        )?;
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

pub fn allowlist_version(conn: &Connection) -> rusqlite::Result<i64> {
    Ok(conn.query_row("SELECT value FROM meta WHERE key = 'allowlist_version'", [], |row| row.get(0)).optional()?.unwrap_or(0))
}

fn bump_allowlist(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute("INSERT INTO meta (key, value) VALUES ('allowlist_version', 1) ON CONFLICT (key) DO UPDATE SET value = value + 1", [])?;
    Ok(())
}

/// The endpoints relays let through: those of Claktories not removed.
pub fn allowlist(conn: &Connection) -> rusqlite::Result<Vec<String>> {
    let mut statement = conn.prepare(
        "SELECT DISTINCT endpoint FROM endpoints JOIN claktories ON claktories.id = endpoints.claktory_id
         WHERE claktories.revoked_at IS NULL ORDER BY endpoint",
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

/// Deletes an account and everything about it.
pub fn delete_account(conn: &mut Connection, account: &Account) -> rusqlite::Result<()> {
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
    tx.execute("DELETE FROM requests WHERE email = ?1", params![account.email])?;
    tx.execute("DELETE FROM sessions WHERE email = ?1", params![account.email])?;
    tx.execute("DELETE FROM accounts WHERE id = ?1", params![account.id])?;
    bump_allowlist(&tx)?;
    tx.commit()
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
            "counts": claktory.counts,
            "location": claktory.location,
            "endpoints": endpoints,
            "unlocks": unlocks,
            "usage": usage,
            "activity": activity,
            "days_in_touch": days,
        }));
    }
    Ok(json!({
        "account": {"id": account.id, "email": account.email, "created": account.created, "last_sign_in": account.last_sign_in},
        "claktories": claktories_out,
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

/// Forgets what has expired: requests, sessions, old limit windows, and
/// share releases (with their IPs) after 30 days.
pub fn purge(conn: &Connection, now: i64) -> rusqlite::Result<()> {
    conn.execute("DELETE FROM requests WHERE expires <= ?1", params![now])?;
    conn.execute("DELETE FROM sessions WHERE expires <= ?1", params![now])?;
    conn.execute("DELETE FROM limits WHERE window < ?1", params![now - 2 * 86_400])?;
    conn.execute("DELETE FROM share_releases WHERE time < ?1", params![now - RELEASES_KEPT])?;
    conn.execute("DELETE FROM periods WHERE received < ?1", params![now - PERIODS_KEPT])?;
    Ok(())
}
