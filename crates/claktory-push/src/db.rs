//! The handles: each one's SHA-256 (never the handle itself, so the
//! database hands out no way to push), the platform and the device's token.

use std::path::Path;

use rusqlite::{Connection, OptionalExtension, params};
use sha2::{Digest, Sha256};

use crate::Platform;

/// A token keeps at most this many handles (one per Claktory it connects
/// to); registering more drops the oldest.
pub const HANDLES_PER_TOKEN: i64 = 20;
/// A handle no push has used for this long is forgotten, in seconds.
pub const UNUSED_FOR: i64 = 180 * 24 * 3600;

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS handles (
    -- SHA-256 of the handle, hex.
    hash TEXT PRIMARY KEY,
    platform TEXT NOT NULL,
    token TEXT NOT NULL,
    -- Unix seconds.
    created INTEGER NOT NULL,
    last_used INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS handles_by_token ON handles (token, created);
";

pub fn open(path: &Path) -> rusqlite::Result<Connection> {
    let conn = Connection::open(path)?;
    conn.pragma_update(None, "journal_mode", "WAL")?;
    conn.pragma_update(None, "busy_timeout", 5000)?;
    conn.execute_batch(SCHEMA)?;
    Ok(conn)
}

pub fn hash(handle: &str) -> String {
    Sha256::digest(handle.as_bytes()).iter().map(|byte| format!("{byte:02x}")).collect()
}

/// A new handle for `token`.
pub fn register(conn: &Connection, platform: Platform, token: &str, now: i64) -> rusqlite::Result<String> {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes).map_err(|error| rusqlite::Error::ToSqlConversionFailure(Box::new(error)))?;
    let handle = crate::b64url(bytes);
    conn.execute(
        "INSERT INTO handles (hash, platform, token, created, last_used) VALUES (?1, ?2, ?3, ?4, ?4)",
        params![hash(&handle), platform.as_str(), token, now],
    )?;
    conn.execute(
        "DELETE FROM handles WHERE token = ?1 AND hash NOT IN
            (SELECT hash FROM handles WHERE token = ?1 ORDER BY created DESC, rowid DESC LIMIT ?2)",
        params![token, HANDLES_PER_TOKEN],
    )?;
    Ok(handle)
}

/// The platform and token `handle` pushes to, if it's known; noted as used.
pub fn lookup(conn: &Connection, handle: &str, now: i64) -> rusqlite::Result<Option<(Platform, String)>> {
    let hash = hash(handle);
    let found: Option<(String, String)> =
        conn.query_row("SELECT platform, token FROM handles WHERE hash = ?1", [&hash], |row| Ok((row.get(0)?, row.get(1)?))).optional()?;
    let Some((platform, token)) = found else { return Ok(None) };
    conn.execute("UPDATE handles SET last_used = ?2 WHERE hash = ?1", params![hash, now])?;
    Ok(Platform::parse(&platform).map(|platform| (platform, token)))
}

pub fn unregister(conn: &Connection, handle: &str) -> rusqlite::Result<bool> {
    Ok(conn.execute("DELETE FROM handles WHERE hash = ?1", [hash(handle)])? > 0)
}

/// Apple or Google retired the token: every handle of it goes.
pub fn forget_token(conn: &Connection, token: &str) -> rusqlite::Result<usize> {
    conn.execute("DELETE FROM handles WHERE token = ?1", [token])
}

/// Forgets handles unused for `UNUSED_FOR`.
pub fn purge(conn: &Connection, now: i64) -> rusqlite::Result<usize> {
    conn.execute("DELETE FROM handles WHERE last_used < ?1", [now - UNUSED_FOR])
}

pub fn count(conn: &Connection) -> rusqlite::Result<i64> {
    conn.query_row("SELECT COUNT(*) FROM handles", [], |row| row.get(0))
}
