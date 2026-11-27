//! The Claktory leaf's sealed store (dms-e2ee.md §5.2): `<home>/dm/leaf.db`,
//! one SQLite file holding the leaf's MLS state (`claktory_dm_crypto`'s
//! storage), the decrypted history of the conversations granted here, and
//! what the leaf keeps beside it. Every value is sealed with one key derived
//! from the vault's (`Vault::dm_store_key`), so the file opens nothing
//! without the vault, and only while it's open.
//!
//! - **One transaction a step.** A message is processed, its history row
//!   written and the group's position moved in one transaction: with
//!   forward secrecy a message can be decrypted only once, so a crash
//!   between the ratchet and the row would lose it for good.
//! - **Rows apart by group,** keyed by plain ids, values sealed: forgetting
//!   a group (a grant revoked, signed out, a new registration) deletes its
//!   rows without the key, so it works while the vault is locked.
//! - **Deleted means gone:** `secure_delete` on and a rollback journal,
//!   never WAL (the crypto crate refuses otherwise).
//! - **Versioned** (`FORMAT`): a store written by a newer Claktory is left
//!   alone, never read or changed; a newer one changes it only by adding.
//!   An older Claktory, after a rollback, doesn't know `dm/` at all.
//! - **Never moved or restored:** `dm/` stays out of a move's bundle and
//!   the vault's backups, so no two copies of a leaf ever run. A store the
//!   vault's key doesn't open (a vault made again) is started over: the leaf
//!   enrolls again.

use std::path::{Path, PathBuf};

use claktory_dm_crypto::provider::Crypto;
use claktory_dm_crypto::storage::{FLOOR_SCHEMA, SCHEMA as MLS_SCHEMA, SealKey};
use rusqlite::{Connection, OptionalExtension, params};

use crate::home::Home;
use crate::vault::{Vault, VaultError};

/// The store's format, in `meta`; one a Claktory doesn't know is newer.
pub(crate) const FORMAT: i64 = 1;
const FILE: &str = "leaf.db";
/// What `check` holds, sealed: it opens only with the key the store was made with.
const CHECK: &[u8] = b"claktory dm leaf store";

const SCHEMA: &str = "
-- format (the store's), store_id (what its key is derived with), check.
CREATE TABLE meta (
    name TEXT PRIMARY KEY,
    value BLOB NOT NULL
) WITHOUT ROWID;
-- This Claktory's leaf: one, for its registration as it was made.
CREATE TABLE leaf (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    account_id TEXT NOT NULL,
    claktory_id TEXT NOT NULL,
    registration_epoch INTEGER NOT NULL,
    -- The leaf's signature key's secret, sealed.
    sealed_key BLOB NOT NULL,
    -- The certificate the person's device signed (not secret: every member
    -- and the service see it); empty until it's enrolled.
    certificate BLOB NOT NULL DEFAULT x'',
    -- The person's pins as their device gave them, sealed; empty until then.
    sealed_pins BLOB NOT NULL DEFAULT x'',
    created INTEGER NOT NULL,
    enrolled INTEGER NOT NULL DEFAULT 0
);
-- The groups the leaf is in, and how far each is processed.
CREATE TABLE groups (
    group_id BLOB PRIMARY KEY,
    -- 'channel' or 'conversation'.
    kind TEXT NOT NULL,
    -- A conversation's id, its generation and the friend's account.
    conversation TEXT NOT NULL DEFAULT '',
    generation INTEGER NOT NULL DEFAULT 0,
    peer TEXT NOT NULL DEFAULT '',
    -- The last seq processed.
    position INTEGER NOT NULL DEFAULT 0,
    joined INTEGER NOT NULL
) WITHOUT ROWID;
CREATE INDEX groups_by_conversation ON groups (conversation) WHERE conversation != '';
-- What was said in a group, decrypted here: the message's seq and when the
-- service took it in the clear (what reads are windowed by), the rest sealed.
CREATE TABLE history (
    group_id BLOB NOT NULL,
    seq INTEGER NOT NULL,
    accepted INTEGER NOT NULL,
    sealed BLOB NOT NULL,
    PRIMARY KEY (group_id, seq)
) WITHOUT ROWID;
-- What agents here proposed and wait on (`sends`): its text sealed, kept
-- until it's decided so it can be posted again for a device added since.
CREATE TABLE proposals (
    client_message_id TEXT PRIMARY KEY,
    proposal_id TEXT NOT NULL DEFAULT '',
    agent TEXT NOT NULL,
    conversation TEXT NOT NULL,
    generation INTEGER NOT NULL,
    sealed_text BLOB NOT NULL
) WITHOUT ROWID;
CREATE INDEX proposals_by_agent ON proposals (agent);
";

/// Where the store lives.
pub(crate) fn dir(home: &Home) -> PathBuf {
    home.root().join("dm")
}

fn path(home: &Home) -> PathBuf {
    dir(home).join(FILE)
}

/// Why the store can't be used now.
#[derive(Debug)]
pub(crate) enum OpenError {
    /// This Claktory has no vault.
    NoVault,
    /// The vault is locked.
    Locked,
    /// A newer Claktory wrote it.
    Newer(i64),
    Failed(String),
}

impl From<rusqlite::Error> for OpenError {
    fn from(error: rusqlite::Error) -> OpenError {
        OpenError::Failed(error.to_string())
    }
}

/// The store, open and its key at hand.
pub(crate) struct Store {
    pub(crate) db: Connection,
    pub(crate) key: SealKey,
    pub(crate) crypto: Crypto,
}

impl std::fmt::Debug for Store {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Store")
    }
}

/// The file opened as the store must be, with no key: for forgetting.
fn connect(path: &Path) -> rusqlite::Result<Connection> {
    let db = Connection::open(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
    }
    #[cfg(windows)]
    let _ = factory_agent::windows::private::protect(path);
    // Private anyway, by the home's DACL: a test notices if not.
    #[cfg(windows)]
    debug_assert!(factory_agent::windows::system::is_private_path(path).unwrap_or(false), "{} isn't private", path.display());
    db.busy_timeout(std::time::Duration::from_secs(5))?;
    db.pragma_update(None, "secure_delete", "ON")?;
    // Answers the mode it's in.
    let _: String = db.query_row("PRAGMA journal_mode = DELETE", [], |row| row.get(0))?;
    Ok(db)
}

/// The store's format, or None when it's new.
fn format(db: &Connection) -> rusqlite::Result<Option<i64>> {
    let made: bool = db.query_row("SELECT EXISTS (SELECT 1 FROM sqlite_master WHERE name = 'meta')", [], |row| row.get(0))?;
    if !made {
        return Ok(None);
    }
    db.query_row("SELECT value FROM meta WHERE name = 'format'", [], |row| row.get(0)).optional()
}

fn meta(db: &Connection, name: &str) -> rusqlite::Result<Option<Vec<u8>>> {
    db.query_row("SELECT value FROM meta WHERE name = ?1", [name], |row| row.get(0)).optional()
}

impl Store {
    /// Opens the store, making it on first use. A store the vault's key
    /// doesn't open is started over.
    pub(crate) fn open(home: &Home, vault: &Vault) -> Result<Store, OpenError> {
        match Store::open_once(home, vault)? {
            Some(store) => Ok(store),
            None => {
                tracing::warn!("the direct-message store doesn't open with this vault's key: starting it over");
                forget_all(home);
                Store::open_once(home, vault)?.ok_or_else(|| OpenError::Failed("a new store didn't open".to_owned()))
            }
        }
    }

    /// None when the store is there but its key isn't this vault's.
    fn open_once(home: &Home, vault: &Vault) -> Result<Option<Store>, OpenError> {
        let dir = dir(home);
        crate::remote::private_dir(&dir).map_err(|error| OpenError::Failed(format!("{}: {error}", dir.display())))?;
        let mut db = connect(&path(home))?;
        match format(&db)? {
            Some(format) if format > FORMAT => return Err(OpenError::Newer(format)),
            Some(_) => {}
            None => {
                let tx = db.transaction()?;
                tx.execute_batch(SCHEMA)?;
                tx.execute_batch(MLS_SCHEMA)?;
                tx.execute("INSERT INTO meta (name, value) VALUES ('format', ?1)", [FORMAT])?;
                let store_id = crate::vault::crypto::random::<32>();
                tx.execute("INSERT INTO meta (name, value) VALUES ('store_id', ?1)", [store_id.as_slice()])?;
                tx.commit()?;
            }
        }
        // Added since a store's first version, at every open (Melone's F2):
        // each group's epoch floor, which a rejoin never goes below.
        db.execute_batch(FLOOR_SCHEMA)?;
        let store_id = meta(&db, "store_id")?.ok_or_else(|| OpenError::Failed("the store has no id".to_owned()))?;
        let key = match vault.dm_store_key(&store_id, "seal") {
            Ok(key) => key,
            Err(VaultError::Locked) => return Err(OpenError::Locked),
            Err(VaultError::NoVault) => return Err(OpenError::NoVault),
            Err(error) => return Err(OpenError::Failed(error.to_string())),
        };
        let key = SealKey::from_bytes(*key);
        match meta(&db, "check")? {
            Some(sealed) => {
                if key.open_row(&[b"meta", b"check"], &sealed).ok().is_none_or(|plain| plain.as_slice() != CHECK) {
                    return Ok(None);
                }
            }
            None => {
                let sealed = key.seal_row(&[b"meta", b"check"], CHECK).map_err(|error| OpenError::Failed(error.to_string()))?;
                db.execute("INSERT INTO meta (name, value) VALUES ('check', ?1)", [sealed])?;
            }
        }
        let crypto = Crypto::new().map_err(|error| OpenError::Failed(error.to_string()))?;
        Ok(Some(Store { db, key, crypto }))
    }
}

/// Runs `forget` on the store's file with no key, if there's a store of a
/// format this knows.
fn without_key(home: &Home, forget: impl FnOnce(&Connection) -> rusqlite::Result<()>) {
    let path = path(home);
    if !path.exists() {
        return;
    }
    let forgot = connect(&path).and_then(|db| match format(&db)? {
        Some(format) if format <= FORMAT => forget(&db),
        _ => Ok(()),
    });
    if let Err(error) = forgot {
        tracing::warn!(%error, "couldn't forget direct messages in this Claktory's store");
    }
}

/// Forgets one group: its MLS state, its history and its row.
pub(crate) fn forget_group(db: &Connection, group_id: &[u8]) -> rusqlite::Result<()> {
    claktory_dm_crypto::storage::forget_group(db, &openmls::prelude::GroupId::from_slice(group_id))
        .map_err(|error| rusqlite::Error::ToSqlConversionFailure(error.to_string().into()))?;
    db.execute("DELETE FROM history WHERE group_id = ?1", [group_id])?;
    db.execute("DELETE FROM groups WHERE group_id = ?1", [group_id])?;
    Ok(())
}

/// Forgets a proposal's text, with no key.
pub(crate) fn forget_proposal(home: &Home, client_message_id: &str) {
    without_key(home, |db| db.execute("DELETE FROM proposals WHERE client_message_id = ?1", [client_message_id]).map(drop));
}

/// Forgets an agent's proposals' texts, with no key.
pub(crate) fn forget_agent_proposals(home: &Home, agent: &str) {
    without_key(home, |db| db.execute("DELETE FROM proposals WHERE agent = ?1", [agent]).map(drop));
}

/// Forgets the whole store: signed out, a new registration, or a store
/// that doesn't open. Its file is deleted (secure_delete zeroed what it
/// freed while it was used; the file goes whole).
pub(crate) fn forget_all(home: &Home) {
    let path = path(home);
    for file in [path.clone(), path.with_extension("db-journal")] {
        if let Err(error) = std::fs::remove_file(&file)
            && error.kind() != std::io::ErrorKind::NotFound
        {
            tracing::warn!(%error, file = %file.display(), "couldn't delete the direct-message store");
        }
    }
}

/// How far `group` is processed; None when the leaf isn't in it.
pub(crate) fn position(db: &Connection, group_id: &[u8]) -> rusqlite::Result<Option<i64>> {
    db.query_row("SELECT position FROM groups WHERE group_id = ?1", [group_id], |row| row.get(0)).optional()
}

pub(crate) fn set_position(db: &Connection, group_id: &[u8], seq: i64) -> rusqlite::Result<()> {
    db.execute("UPDATE groups SET position = MAX(position, ?2) WHERE group_id = ?1", params![group_id, seq]).map(drop)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A home of its own, deleted when dropped.
    struct TempHome(Home);

    impl TempHome {
        fn new() -> TempHome {
            let root = std::env::temp_dir().join(format!("dm-store-{}", crate::vault::crypto::hex(&crate::vault::crypto::random::<8>())));
            std::fs::create_dir_all(&root).unwrap();
            TempHome(Home::new(root))
        }
    }

    impl Drop for TempHome {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(self.0.root());
        }
    }

    fn vault(home: &Home) -> Vault {
        let vault = Vault::open_or_make(home.root().join("vault"));
        if !vault.exists() {
            vault.create(None, false).unwrap();
        }
        vault
    }

    #[test]
    fn the_store_opens_with_its_vault_and_starts_over_with_another() {
        let temp = TempHome::new();
        let home = &temp.0;
        let first = vault(home);
        let store = Store::open(home, &first).unwrap();
        let journal: String = store.db.query_row("PRAGMA journal_mode", [], |row| row.get(0)).unwrap();
        let secure: i64 = store.db.query_row("PRAGMA secure_delete", [], |row| row.get(0)).unwrap();
        assert_eq!((journal.as_str(), secure), ("delete", 1));
        let sealed = store.key.seal_row(&[b"history", b"g", &1i64.to_be_bytes()], b"hello").unwrap();
        store
            .db
            .execute("INSERT INTO history (group_id, seq, accepted, sealed) VALUES (x'67', 1, 1, ?1)", [&sealed])
            .unwrap();
        drop(store);
        let again = Store::open(home, &first).unwrap();
        let kept: Vec<u8> = again.db.query_row("SELECT sealed FROM history", [], |row| row.get(0)).unwrap();
        assert_eq!(again.key.open_row(&[b"history", b"g", &1i64.to_be_bytes()], &kept).unwrap().as_slice(), b"hello");
        assert!(again.key.open_row(&[b"history", b"g", &2i64.to_be_bytes()], &kept).is_err(), "bound to its row");
        drop(again);

        // Another vault: the store is started over, nothing of it read.
        // Windows deletes no file that's open: the first vault's closes first.
        #[cfg(windows)]
        drop(first);
        std::fs::remove_dir_all(home.root().join("vault")).unwrap();
        let second = vault(home);
        let fresh = Store::open(home, &second).unwrap();
        let rows: i64 = fresh.db.query_row("SELECT COUNT(*) FROM history", [], |row| row.get(0)).unwrap();
        assert_eq!(rows, 0);
    }

    #[test]
    fn a_newer_store_is_left_alone_and_a_locked_vault_says_so() {
        let temp = TempHome::new();
        let home = &temp.0;
        let vault = vault(home);
        drop(Store::open(home, &vault).unwrap());
        let db = connect(&path(home)).unwrap();
        db.execute("UPDATE meta SET value = ?1 WHERE name = 'format'", [FORMAT + 1]).unwrap();
        drop(db);
        assert!(matches!(Store::open(home, &vault), Err(OpenError::Newer(_))));
        forget_proposal(home, "c1");
        let db = connect(&path(home)).unwrap();
        let format: i64 = db.query_row("SELECT value FROM meta WHERE name = 'format'", [], |row| row.get(0)).unwrap();
        assert_eq!(format, FORMAT + 1, "untouched");

        let locked = Vault::open(home.root().join("vault-locked"));
        assert!(matches!(Store::open(&Home::new(home.root().join("other")), &locked), Err(OpenError::NoVault)));
    }
}
