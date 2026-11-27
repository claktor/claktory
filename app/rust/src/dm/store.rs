//! The device's sealed store for direct messages:
//! one SQLite file in the app's support folder holding OpenMLS's state
//! (claktory-dm-crypto's `mls_values`, every value sealed) and the app's own
//! tables (dm.rs's: history, positions, the retry cache, pins). Every table
//! is defined here, behind the store's version, so the file only ever moves
//! forward through [`MIGRATIONS`].
//!
//! - **One transaction per message** ([`Store::with_tx`]): OpenMLS's state
//!   and what the app keeps of the message commit together or not at all, so
//!   a crash never leaves them apart, and a message whose transaction rolled
//!   back decrypts again next time.
//! - **Deleted secrets are gone**: `secure_delete` on and a
//!   rollback journal (TRUNCATE), never WAL; claktory-dm-crypto refuses any
//!   other connection.
//! - **The seal key** comes from the platform key store (dm_keys.dart's
//!   'store'), as bytes, from dm.rs; this never reads it itself. The file
//!   remembers a check of the key it was made with, so a file opened with
//!   another key (keys lost, a backup restored on another device) is told
//!   apart from one that's damaged: [`Opened::OtherKey`].
//! - The file is this user's only (0600).
use std::path::Path;
use std::sync::Mutex;

use claktory_dm_crypto::provider::{Crypto, Provider};
use claktory_dm_crypto::storage::{FLOOR_SCHEMA, SCHEMA, SealKey};
use claktory_dm_crypto::{Error, Result};
use rusqlite::{Connection, OptionalExtension, Transaction, params};
use zeroize::Zeroizing;

/// The store's tables, in order: the store at version N has run the first
/// N. Only ever append; never change one that shipped.
// FLOOR_SCHEMA: each group's epoch floor, kept by the crate as it merges:
// a rejoin never goes back past it.
const MIGRATIONS: &[&str] = &[SCHEMA, APP, SIDE, TRANSFERRED, FLOOR_SCHEMA];

/// The app's own tables (dm.rs). Values are sealed ([`Store::seal`]) except
/// what the service already knows: group ids, their kinds (whose they are),
/// seqs and envelope digests.
/// - `rows`: one-off values by name: this device's certificate and account,
///   its pins.
/// - `groups`: every group this device is in, its kind as the device's own
///   records have it (never the group's say) and the last seq processed.
/// - `history`: what was said, decrypted, by group and seq.
/// - `outgoing`: what this device encrypted and hasn't seen come back: a
///   retry sends the same bytes (MLS never encrypts twice for one
///   message), and when it comes back at its seq it's recognised by its
///   digest, since a member can't decrypt its own messages.
/// - `commits`: this device's commit the worker hasn't taken yet, by its
///   digest, so its coming back in the feed merges it.
const APP: &str = "CREATE TABLE rows (
    name TEXT PRIMARY KEY NOT NULL,
    value BLOB NOT NULL
) WITHOUT ROWID;
CREATE TABLE groups (
    id BLOB PRIMARY KEY NOT NULL,
    kind TEXT NOT NULL,
    seq INTEGER NOT NULL
) WITHOUT ROWID;
CREATE TABLE history (
    grp BLOB NOT NULL REFERENCES groups (id) ON DELETE CASCADE,
    seq INTEGER NOT NULL,
    value BLOB NOT NULL,
    PRIMARY KEY (grp, seq)
) WITHOUT ROWID;
CREATE TABLE outgoing (
    grp BLOB NOT NULL REFERENCES groups (id) ON DELETE CASCADE,
    client_id TEXT NOT NULL,
    digest BLOB NOT NULL,
    envelope BLOB NOT NULL,
    value BLOB NOT NULL,
    created_ms INTEGER NOT NULL,
    PRIMARY KEY (grp, client_id)
) WITHOUT ROWID;
CREATE INDEX outgoing_digest ON outgoing (digest);
CREATE TABLE commits (
    grp BLOB PRIMARY KEY NOT NULL REFERENCES groups (id) ON DELETE CASCADE,
    digest BLOB NOT NULL
) WITHOUT ROWID;";

/// Proposals and agents' drafts, which come by their own routes, not the
/// feed: decrypted once (a member can decrypt each message once) and kept
/// by the envelope's digest.
const SIDE: &str = "CREATE TABLE side (
    grp BLOB NOT NULL REFERENCES groups (id) ON DELETE CASCADE,
    digest BLOB NOT NULL,
    value BLOB NOT NULL,
    PRIMARY KEY (grp, digest)
) WITHOUT ROWID;";

/// History another of the person's devices sent this one (E8,
/// dm/transfer.rs) for a group it hasn't joined yet: sealed as `history`
/// rows are, moved there when it joins (`transfer::adopt`), dropped after a
/// day if it never does. No foreign key: the group isn't here yet.
const TRANSFERRED: &str = "CREATE TABLE transferred (
    grp BLOB NOT NULL,
    seq INTEGER NOT NULL,
    value BLOB NOT NULL,
    created_ms INTEGER NOT NULL,
    PRIMARY KEY (grp, seq)
) WITHOUT ROWID;";

/// What the check of the seal key is made from.
const CHECK: &[u8] = b"claktory-dm store key check v1\0";

/// How [`Store::open`] found the file.
#[derive(Debug)]
#[allow(clippy::large_enum_variant)]
pub enum Opened {
    /// Made now, or made before with this key.
    Ready(Store),
    /// Made with another seal key: what's in it can't be read with this
    /// one. The caller decides (start over with [`Store::remove`]).
    OtherKey,
}

pub struct Store {
    db: Mutex<Connection>,
    crypto: Crypto,
    key: SealKey,
}

impl std::fmt::Debug for Store {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Store")
    }
}

impl Store {
    /// Opens the store at `path`, making it if it isn't there, and brings
    /// its tables up to this version. A store from a newer Claktory is
    /// refused, never written.
    pub fn open(path: &Path, seal_key: [u8; 32]) -> Result<Opened> {
        if let Some(folder) = path.parent() {
            std::fs::create_dir_all(folder).map_err(io)?;
        }
        let fresh = !path.exists();
        let db = Connection::open(path).map_err(sql)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).map_err(io)?;
        }
        db.pragma_update(None, "secure_delete", "ON").map_err(sql)?;
        db.pragma_update(None, "journal_mode", "TRUNCATE").map_err(sql)?;
        db.pragma_update(None, "foreign_keys", "ON").map_err(sql)?;
        db.busy_timeout(std::time::Duration::from_secs(5)).map_err(sql)?;
        let check = check(&seal_key);
        let mut db = db;
        {
            let tx = db.transaction().map_err(sql)?;
            tx.execute_batch(
                "CREATE TABLE IF NOT EXISTS store (
                    one INTEGER PRIMARY KEY CHECK (one = 1),
                    version INTEGER NOT NULL,
                    key_check BLOB NOT NULL
                );",
            )
            .map_err(sql)?;
            let row: Option<(i64, Vec<u8>)> = tx
                .query_row("SELECT version, key_check FROM store WHERE one = 1", [], |row| Ok((row.get(0)?, row.get(1)?)))
                .optional()
                .map_err(sql)?;
            let version = match row {
                None => {
                    if !fresh && has_tables(&tx)? {
                        return Err(Error::Storage("a store without its version".to_owned()));
                    }
                    tx.execute("INSERT INTO store (one, version, key_check) VALUES (1, 0, ?1)", params![check]).map_err(sql)?;
                    0
                }
                Some((_, kept)) if kept != check => return Ok(Opened::OtherKey),
                Some((version, _)) => version,
            };
            let known = MIGRATIONS.len() as i64;
            if version > known {
                return Err(Error::Storage(format!("a store at version {version}, from a newer Claktory (this one knows {known})")));
            }
            for migration in &MIGRATIONS[version as usize..] {
                tx.execute_batch(migration).map_err(sql)?;
            }
            tx.execute("UPDATE store SET version = ?1 WHERE one = 1", params![known]).map_err(sql)?;
            tx.commit().map_err(sql)?;
        }
        Ok(Opened::Ready(Store { db: Mutex::new(db), crypto: Crypto::new()?, key: SealKey::from_bytes(seal_key) }))
    }

    /// Deletes the store at `path` and its journal: starting over, after
    /// [`Opened::OtherKey`] or on signing out. Absent is fine.
    pub fn remove(path: &Path) -> Result<()> {
        for each in [path.to_path_buf(), path.with_extension("db-journal"), journal(path)] {
            match std::fs::remove_file(&each) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(io(error)),
            }
        }
        Ok(())
    }

    /// Runs `work` in one transaction, with OpenMLS's provider over it:
    /// committed if it returns Ok, rolled back if it returns an error (or
    /// panics). One message, one call.
    pub fn with_tx<T, E>(
        &self,
        work: impl FnOnce(&Provider<'_>, &Transaction<'_>) -> std::result::Result<T, E>,
    ) -> std::result::Result<T, E>
    where
        E: From<Error>,
    {
        let mut db = self.db.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let tx = db.transaction().map_err(sql)?;
        let out = {
            let provider = Provider::new(&self.crypto, &tx, &self.key)?;
            work(&provider, &tx)?
        };
        tx.commit().map_err(sql)?;
        Ok(out)
    }
}

impl Store {
    /// `plain` sealed for its exact place (the table, the row's identity,
    /// the column), under the store's key (claktory-dm-crypto's
    /// `seal_row`), so a value pasted over another row doesn't open.
    pub fn seal_row(&self, place: &[&[u8]], plain: &[u8]) -> Result<Vec<u8>> {
        self.key.seal_row(place, plain)
    }

    /// A row's value, opened: [`Error::Sealed`] when it isn't the one
    /// sealed for `place` with this key.
    pub fn open_row(&self, place: &[&[u8]], sealed: &[u8]) -> Result<Zeroizing<Vec<u8>>> {
        self.key.open_row(place, sealed)
    }
}

/// SQLite's journal beside `path` ("store.db" → "store.db-journal").
fn journal(path: &Path) -> std::path::PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push("-journal");
    name.into()
}

fn has_tables(tx: &Transaction<'_>) -> Result<bool> {
    let count: i64 =
        tx.query_row("SELECT count(*) FROM sqlite_master WHERE type = 'table' AND name != 'store'", [], |row| row.get(0)).map_err(sql)?;
    Ok(count > 0)
}

fn check(key: &[u8; 32]) -> Vec<u8> {
    claktory_dm_crypto::envelope::digest(&Zeroizing::new([CHECK, key.as_slice()].concat())).to_vec()
}

fn sql(error: rusqlite::Error) -> Error {
    Error::Storage(error.to_string())
}

fn io(error: std::io::Error) -> Error {
    Error::Storage(error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn folder() -> std::path::PathBuf {
        // Apart for every test: the clock alone can repeat across threads.
        static NEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("claktory-dm-store-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn ready(opened: Opened) -> Store {
        match opened {
            Opened::Ready(store) => store,
            Opened::OtherKey => panic!("opened with another key"),
        }
    }

    #[test]
    fn made_once_then_opened_with_its_key_only() {
        let dir = folder();
        let path = dir.join("dm").join("store.db");
        let store = ready(Store::open(&path, [7; 32]).unwrap());
        drop(store);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
        }
        ready(Store::open(&path, [7; 32]).unwrap());
        assert!(matches!(Store::open(&path, [8; 32]).unwrap(), Opened::OtherKey));
        Store::remove(&path).unwrap();
        assert!(!path.exists());
        ready(Store::open(&path, [8; 32]).unwrap());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn secure_delete_and_a_rollback_journal() {
        let dir = folder();
        let store = ready(Store::open(&dir.join("store.db"), [1; 32]).unwrap());
        let (secure, journal): (i64, String) = store
            .with_tx(|_, tx| -> Result<_> {
                Ok((
                    tx.query_row("PRAGMA secure_delete", [], |row| row.get(0)).map_err(sql)?,
                    tx.query_row("PRAGMA journal_mode", [], |row| row.get(0)).map_err(sql)?,
                ))
            })
            .unwrap();
        assert_eq!(secure, 1);
        assert_eq!(journal, "truncate");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_failed_transaction_leaves_nothing() {
        let dir = folder();
        let store = ready(Store::open(&dir.join("store.db"), [2; 32]).unwrap());
        let count = |store: &Store| -> i64 {
            store
                .with_tx(|_, tx| -> Result<i64> { tx.query_row("SELECT count(*) FROM mls_values", [], |row| row.get(0)).map_err(sql) })
                .unwrap()
        };
        let failed: Result<()> = store.with_tx(|_, tx| {
            tx.execute("INSERT INTO mls_values (key, value) VALUES (x'01', x'02')", []).map_err(sql)?;
            Err(Error::Storage("the message didn't process".to_owned()))
        });
        assert!(failed.is_err());
        assert_eq!(count(&store), 0);
        store
            .with_tx(|_, tx| -> Result<()> {
                tx.execute("INSERT INTO mls_values (key, value) VALUES (x'01', x'02')", []).map_err(sql)?;
                Ok(())
            })
            .unwrap();
        assert_eq!(count(&store), 1);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn rows_open_only_for_their_label_and_key() {
        let dir = folder();
        let store = ready(Store::open(&dir.join("store.db"), [5; 32]).unwrap());
        let sealed = store.seal_row(&[b"rows", b"pins"], b"secret").unwrap();
        assert_eq!(store.open_row(&[b"rows", b"pins"], &sealed).unwrap().as_slice(), b"secret");
        assert!(matches!(store.open_row(&[b"rows", b"me"], &sealed), Err(Error::Sealed)));
        let other = ready(Store::open(&dir.join("other.db"), [6; 32]).unwrap());
        assert!(matches!(other.open_row(&[b"rows", b"pins"], &sealed), Err(Error::Sealed)));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_newer_store_is_refused() {
        let dir = folder();
        let path = dir.join("store.db");
        drop(ready(Store::open(&path, [3; 32]).unwrap()));
        Connection::open(&path).unwrap().execute("UPDATE store SET version = 99", []).unwrap();
        let error = Store::open(&path, [3; 32]).unwrap_err();
        assert!(error.to_string().contains("newer"), "{error}");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn openmls_runs_over_it() {
        use claktory_dm_crypto::certificate::{Leaf, LeafCertificate, LeafKind};
        use claktory_dm_crypto::group::{LeafKey, Me};
        use claktory_dm_crypto::identity::IdentityKey;
        let dir = folder();
        let store = ready(Store::open(&dir.join("store.db"), [4; 32]).unwrap());
        let identity = IdentityKey::generate().unwrap();
        let key = LeafKey::generate().unwrap();
        let leaf = Leaf {
            kind: LeafKind::Device,
            account_id: "matt".to_owned(),
            leaf_id: "phone".to_owned(),
            signature_key: key.public(),
            name: "iPhone".to_owned(),
            created_ms: 1,
        };
        let me = Me::new(key, LeafCertificate::issue(&identity, leaf).unwrap()).unwrap();
        // A key package's private keys go into the table, sealed.
        store.with_tx(|provider, _| me.key_package(provider).map(|_| ())).unwrap();
        let values: i64 = store
            .with_tx(|_, tx| -> Result<i64> { tx.query_row("SELECT count(*) FROM mls_values", [], |row| row.get(0)).map_err(sql) })
            .unwrap();
        assert!(values > 0);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
