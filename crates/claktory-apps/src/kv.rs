//! An app's own key-value store: `kv.sqlite` in its data folder, kept across
//! restarts and updates, within its manifest's storage allowance.

use std::path::Path;

use rusqlite::{Connection, OptionalExtension, params};

/// The longest key, in bytes.
pub const MAX_KEY: usize = 1024;

pub struct Kv {
    db: Connection,
    /// The bytes its keys and values take now.
    used: u64,
    /// The most they may take.
    limit: u64,
}

impl Kv {
    /// Opens (or creates) the store in `dir`, allowed `limit` bytes.
    pub fn open(dir: &Path, limit: u64) -> rusqlite::Result<Kv> {
        std::fs::create_dir_all(dir).map_err(|error| rusqlite::Error::ToSqlConversionFailure(Box::new(error)))?;
        let db = Connection::open(dir.join("kv.sqlite"))?;
        db.pragma_update(None, "journal_mode", "WAL")?;
        db.pragma_update(None, "synchronous", "NORMAL")?;
        db.execute("CREATE TABLE IF NOT EXISTS kv (key TEXT PRIMARY KEY, value BLOB NOT NULL)", [])?;
        let used: i64 = db.query_row("SELECT COALESCE(SUM(LENGTH(CAST(key AS BLOB)) + LENGTH(value)), 0) FROM kv", [], |row| row.get(0))?;
        Ok(Kv { db, used: used.max(0) as u64, limit })
    }

    pub fn get(&self, key: &str) -> Option<Vec<u8>> {
        let found = self.db.query_row("SELECT value FROM kv WHERE key = ?1", [key], |row| row.get(0)).optional();
        found.unwrap_or_else(|error| {
            tracing::error!(%error, "could not read the app's store");
            None
        })
    }

    /// Keeps `value` under `key`, or says why not.
    pub fn set(&mut self, key: &str, value: &[u8]) -> Result<(), String> {
        if key.len() > MAX_KEY {
            return Err(format!("Keys are up to {MAX_KEY} bytes."));
        }
        let old = self.size(key);
        let new = (key.len() + value.len()) as u64;
        let used = self.used - old + new;
        if used > self.limit {
            return Err(format!("The store would grow past its {} MB.", self.limit.div_ceil(1 << 20)));
        }
        self.db
            .execute(
                "INSERT INTO kv (key, value) VALUES (?1, ?2) ON CONFLICT (key) DO UPDATE SET value = excluded.value",
                params![key, value],
            )
            .map_err(|error| format!("The store couldn't keep it: {error}."))?;
        self.used = used;
        Ok(())
    }

    pub fn delete(&mut self, key: &str) {
        let old = self.size(key);
        match self.db.execute("DELETE FROM kv WHERE key = ?1", [key]) {
            Ok(_) => self.used -= old,
            Err(error) => tracing::error!(%error, "could not delete from the app's store"),
        }
    }

    /// The bytes `key` and its value take, 0 when it has none.
    fn size(&self, key: &str) -> u64 {
        let size: Option<i64> = self
            .db
            .query_row("SELECT LENGTH(CAST(key AS BLOB)) + LENGTH(value) FROM kv WHERE key = ?1", [key], |row| row.get(0))
            .optional()
            .ok()
            .flatten();
        size.unwrap_or(0).max(0) as u64
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("claktory-apps-kv-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn values_last_and_the_allowance_holds_across_opens() {
        let dir = dir("allowance");
        let mut kv = Kv::open(&dir, 100).expect("open");
        kv.set("offset", b"42").expect("set");
        kv.set("offset", b"43").expect("replace");
        assert_eq!(kv.get("offset").as_deref(), Some(&b"43"[..]));
        assert_eq!(kv.set("big", &[0; 95]), Err("The store would grow past its 1 MB.".to_owned()));
        kv.set("big", &[0; 80]).expect("fits");
        drop(kv);

        let mut kv = Kv::open(&dir, 100).expect("open again");
        assert_eq!(kv.used, 6 + 2 + 3 + 80);
        assert!(kv.set("more", &[0; 20]).is_err(), "what it kept still counts");
        kv.delete("big");
        assert_eq!(kv.get("big"), None);
        kv.set("more", &[0; 20]).expect("room again");
        assert!(kv.set(&"k".repeat(MAX_KEY + 1), b"").is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
