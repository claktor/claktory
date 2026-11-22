//! DM keys (D5a): what keeps a copy of the worker's database, or of its
//! backups, from yielding any DM text on its own.
//!
//! - The **wrapping key** is 32 random bytes in a file of its own
//!   (`--dm-key`), kept apart from the database and its backups. It's never
//!   one of the account service's keys, and nothing else is derived from it.
//! - **Data keys** are 32 random bytes, one per purpose (an account's
//!   profile and codes, one conversation's messages), kept in `dm_keys`
//!   sealed with the wrapping key and bound to their id and purpose.
//!   Forgetting one forgets everything sealed with it.
//! - A **field** (a body, a profile, a code's reveal value) is sealed with
//!   its purpose's data key and bound to where it lives (table, record,
//!   column), so a value moved to another record doesn't open: format byte,
//!   cipher byte, the data key's id, a fresh nonce, then XChaCha20-Poly1305.
//!
//! The database records which wrapping key it was made with, so a worker
//! given another key (a restore with the wrong one) refuses to start,
//! rather than sealing new data under a key the old data doesn't open with.
//! Nothing here logs or shows a key or what it opens: `Opened` hides itself
//! from Debug, and every failure is the same `Refused`.

use std::fmt;
use std::path::Path;

use chacha20poly1305::aead::{Aead, Payload};
use chacha20poly1305::{KeyInit, XChaCha20Poly1305, XNonce};
use rusqlite::{Connection, OptionalExtension, params};
use sha2::{Digest, Sha256};
use zeroize::Zeroizing;

/// A field's format, and its cipher.
const FORMAT: u8 = 1;
const XCHACHA20POLY1305: u8 = 1;
const KEY_ID: usize = 16;
const NONCE: usize = 24;
const TAG: usize = 16;

pub const SCHEMA: &str = "
-- Data keys, each sealed with the wrapping key (`wrapped_by` is its id) and
-- bound to its own id and purpose.
CREATE TABLE IF NOT EXISTS dm_keys (
    id BLOB PRIMARY KEY,
    purpose TEXT NOT NULL UNIQUE,
    wrapped BLOB NOT NULL,
    wrapped_by TEXT NOT NULL,
    created INTEGER NOT NULL
);
-- What the store needs to know about itself: 'wrapping_key', the id of the
-- wrapping key its data keys are sealed with.
CREATE TABLE IF NOT EXISTS dm_meta (
    name TEXT PRIMARY KEY,
    value TEXT NOT NULL
);
";

/// Why a key or a field can't be used. It never says what it holds.
#[derive(Debug, PartialEq, Eq)]
pub enum Refused {
    /// The wrapping key file is missing, unreadable or not a key.
    KeyFile(String),
    /// The database's data keys are sealed with another wrapping key.
    WrongKey {
        expected: String,
        given: String,
    },
    /// A field or data key doesn't open: tampered, moved, or under a key
    /// that's gone.
    DoesntOpen,
    Db(String),
}

impl fmt::Display for Refused {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Refused::KeyFile(why) => write!(f, "the DM key: {why}"),
            Refused::WrongKey { expected, given } => write!(
                f,
                "this database's DM keys are sealed with the wrapping key {expected}, not the one given ({given}): give --dm-key the right one"
            ),
            Refused::DoesntOpen => write!(f, "a DM record doesn't open"),
            Refused::Db(error) => write!(f, "the DM store: {error}"),
        }
    }
}

impl std::error::Error for Refused {}

impl From<rusqlite::Error> for Refused {
    fn from(error: rusqlite::Error) -> Refused {
        Refused::Db(error.to_string())
    }
}

/// What a field opened to: wiped when dropped, never shown by Debug.
pub struct Opened(Zeroizing<Vec<u8>>);

impl Opened {
    pub fn bytes(&self) -> &[u8] {
        &self.0
    }
}

impl fmt::Debug for Opened {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Opened([{} bytes hidden])", self.0.len())
    }
}

/// Where a sealed field lives, which it's bound to.
#[derive(Clone, Copy, Debug)]
pub struct Field<'a> {
    pub table: &'a str,
    pub record: &'a str,
    pub column: &'a str,
}

/// The wrapping key, and its id (not secret: a hash of it under a label of
/// its own).
pub struct Keys {
    wrapping: Zeroizing<[u8; 32]>,
    id: String,
}

impl fmt::Debug for Keys {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Keys {{ id: {}, wrapping: [hidden] }}", self.id)
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn random<const N: usize>() -> [u8; N] {
    let mut bytes = [0u8; N];
    getrandom::fill(&mut bytes).expect("the system's random source");
    bytes
}

fn cipher(key: &[u8; 32]) -> XChaCha20Poly1305 {
    XChaCha20Poly1305::new_from_slice(key).expect("a 32-byte key")
}

fn wrap_aad(id: &[u8], purpose: &str, wrapped_by: &str) -> Vec<u8> {
    [b"claktory-dms data key v1\0".as_slice(), id, b"\0", purpose.as_bytes(), b"\0", wrapped_by.as_bytes()].concat()
}

fn field_aad(field: Field<'_>, key_id: &[u8]) -> Vec<u8> {
    [
        b"claktory-dms field v1\0".as_slice(),
        field.table.as_bytes(),
        b"\0",
        field.record.as_bytes(),
        b"\0",
        field.column.as_bytes(),
        b"\0",
        key_id,
    ]
    .concat()
}

impl Keys {
    pub fn from_bytes(wrapping: [u8; 32]) -> Keys {
        let wrapping = Zeroizing::new(wrapping);
        let digest = Sha256::digest([b"claktory-dms wrapping key id v1\0".as_slice(), wrapping.as_slice()].concat());
        Keys { id: hex(&digest[..8]), wrapping }
    }

    /// Reads the wrapping key: 64 hex digits, from a file only its owner
    /// may read.
    pub fn load(path: &Path) -> Result<Keys, Refused> {
        use std::os::unix::fs::PermissionsExt as _;
        let metadata = std::fs::metadata(path).map_err(|error| Refused::KeyFile(format!("can't read {}: {error}", path.display())))?;
        if metadata.permissions().mode() & 0o077 != 0 {
            return Err(Refused::KeyFile(format!("{} can be read by others: make it 0600 (or 0400)", path.display())));
        }
        let text = Zeroizing::new(
            std::fs::read_to_string(path).map_err(|error| Refused::KeyFile(format!("can't read {}: {error}", path.display())))?,
        );
        let text = text.trim();
        let mut key = Zeroizing::new([0u8; 32]);
        if text.len() != 64 || !text.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(Refused::KeyFile(format!("{} isn't a key (64 hex digits)", path.display())));
        }
        for (at, byte) in key.iter_mut().enumerate() {
            *byte = u8::from_str_radix(&text[at * 2..at * 2 + 2], 16).map_err(|_| Refused::KeyFile("not hex".to_owned()))?;
        }
        Ok(Keys::from_bytes(*key))
    }

    /// Writes a new wrapping key to `path`, 0600, never over a file there.
    pub fn create(path: &Path) -> Result<Keys, Refused> {
        use std::io::Write as _;
        use std::os::unix::fs::OpenOptionsExt as _;
        let bytes = random::<32>();
        let keys = Keys::from_bytes(bytes);
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(path)
            .map_err(|error| Refused::KeyFile(format!("can't make {}: {error}", path.display())))?;
        let text = Zeroizing::new(format!("{}\n", hex(keys.wrapping.as_slice())));
        file.write_all(text.as_bytes())
            .and_then(|()| file.sync_all())
            .map_err(|error| Refused::KeyFile(format!("can't write {}: {error}", path.display())))?;
        Ok(keys)
    }

    /// The wrapping key's id, as the database records it.
    pub fn id(&self) -> &str {
        &self.id
    }

    /// Ties `db` to this wrapping key: the first time, records it; after,
    /// refuses another.
    pub fn bind(&self, db: &Connection) -> Result<(), Refused> {
        db.execute_batch(SCHEMA)?;
        let recorded: Option<String> =
            db.query_row("SELECT value FROM dm_meta WHERE name = 'wrapping_key'", [], |row| row.get(0)).optional()?;
        match recorded {
            Some(expected) if expected != self.id => Err(Refused::WrongKey { expected, given: self.id.clone() }),
            Some(_) => Ok(()),
            None => {
                db.execute("INSERT INTO dm_meta (name, value) VALUES ('wrapping_key', ?1)", [&self.id])?;
                Ok(())
            }
        }
    }

    /// The data key for `purpose`, made (and kept, sealed) if there's none:
    /// its id and the key.
    fn data_key(&self, db: &Connection, purpose: &str, now_ms: i64) -> Result<(Vec<u8>, Zeroizing<[u8; 32]>), Refused> {
        let found: Option<(Vec<u8>, Vec<u8>, String)> = db
            .query_row("SELECT id, wrapped, wrapped_by FROM dm_keys WHERE purpose = ?1", [purpose], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?))
            })
            .optional()?;
        if let Some((id, wrapped, wrapped_by)) = found {
            return Ok((id.clone(), self.unwrap_key(&id, purpose, &wrapped, &wrapped_by)?));
        }
        let id = random::<KEY_ID>().to_vec();
        let key = Zeroizing::new(random::<32>());
        let wrapped = self.wrap(&id, purpose, &key)?;
        db.execute(
            "INSERT INTO dm_keys (id, purpose, wrapped, wrapped_by, created) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![id, purpose, wrapped, self.id, now_ms],
        )?;
        Ok((id, key))
    }

    fn unwrap_key(&self, id: &[u8], purpose: &str, wrapped: &[u8], wrapped_by: &str) -> Result<Zeroizing<[u8; 32]>, Refused> {
        if wrapped_by != self.id || wrapped.len() != NONCE + 32 + TAG {
            return Err(Refused::DoesntOpen);
        }
        let (nonce, sealed) = wrapped.split_at(NONCE);
        let nonce: [u8; NONCE] = nonce.try_into().map_err(|_| Refused::DoesntOpen)?;
        let opened = Zeroizing::new(
            cipher(&self.wrapping)
                .decrypt(&XNonce::from(nonce), Payload { msg: sealed, aad: &wrap_aad(id, purpose, wrapped_by) })
                .map_err(|_| Refused::DoesntOpen)?,
        );
        let mut key = Zeroizing::new([0u8; 32]);
        key.copy_from_slice(&opened);
        Ok(key)
    }

    fn wrap(&self, id: &[u8], purpose: &str, key: &[u8; 32]) -> Result<Vec<u8>, Refused> {
        let nonce = random::<NONCE>();
        let sealed = cipher(&self.wrapping)
            .encrypt(&XNonce::from(nonce), Payload { msg: key.as_slice(), aad: &wrap_aad(id, purpose, &self.id) })
            .map_err(|_| Refused::DoesntOpen)?;
        Ok([nonce.as_slice(), &sealed].concat())
    }

    /// `purpose`'s data key itself, made if there's none, for a keyed hash
    /// (a lookup digest): never stored or sent anywhere.
    pub fn hash_key(&self, db: &Connection, purpose: &str, now_ms: i64) -> Result<Zeroizing<[u8; 32]>, Refused> {
        Ok(self.data_key(db, purpose, now_ms)?.1)
    }

    /// Seals `plain` for `field`, with `purpose`'s data key.
    pub fn seal(&self, db: &Connection, purpose: &str, field: Field<'_>, plain: &[u8], now_ms: i64) -> Result<Vec<u8>, Refused> {
        let (key_id, key) = self.data_key(db, purpose, now_ms)?;
        let nonce = random::<NONCE>();
        let sealed = cipher(&key)
            .encrypt(&XNonce::from(nonce), Payload { msg: plain, aad: &field_aad(field, &key_id) })
            .map_err(|_| Refused::DoesntOpen)?;
        Ok([&[FORMAT, XCHACHA20POLY1305][..], &key_id, &nonce, &sealed].concat())
    }

    /// Opens what `seal` made for `field`.
    pub fn open(&self, db: &Connection, field: Field<'_>, sealed: &[u8]) -> Result<Opened, Refused> {
        if sealed.len() < 2 + KEY_ID + NONCE + TAG || sealed[0] != FORMAT || sealed[1] != XCHACHA20POLY1305 {
            return Err(Refused::DoesntOpen);
        }
        let key_id = &sealed[2..2 + KEY_ID];
        let nonce: [u8; NONCE] = sealed[2 + KEY_ID..2 + KEY_ID + NONCE].try_into().map_err(|_| Refused::DoesntOpen)?;
        let found: Option<(String, Vec<u8>, String)> = db
            .query_row("SELECT purpose, wrapped, wrapped_by FROM dm_keys WHERE id = ?1", [key_id], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?))
            })
            .optional()?;
        let (purpose, wrapped, wrapped_by) = found.ok_or(Refused::DoesntOpen)?;
        let key = self.unwrap_key(key_id, &purpose, &wrapped, &wrapped_by)?;
        let opened = cipher(&key)
            .decrypt(&XNonce::from(nonce), Payload { msg: &sealed[2 + KEY_ID + NONCE..], aad: &field_aad(field, key_id) })
            .map_err(|_| Refused::DoesntOpen)?;
        Ok(Opened(Zeroizing::new(opened)))
    }
}

/// Seals every data key in `db` with the wrapping key `to` instead of
/// `from`, in one transaction, and records `to` as the database's: after a
/// wrapping key may have been exposed. Every data key must open with
/// `from`, or nothing changes. Run it with the worker stopped, then start
/// it with `to`. How many data keys it rewrapped.
///
/// The data keys stay the same: this answers a leaked key file, not a leaked
/// key file *and* a copy of the database sealed with it (an older backup),
/// which needs new data keys and every field sealed again. So the old
/// wrapped keys don't linger in this file, it zeroes what it frees
/// (`secure_delete`), then rebuilds the file (`VACUUM`) and empties the WAL.
pub fn rewrap(db: &mut Connection, from: &Keys, to: &Keys) -> Result<usize, Refused> {
    from.bind(db)?;
    db.pragma_update(None, "secure_delete", "ON")?;
    let transaction = db.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    let rows: Vec<(Vec<u8>, String, Vec<u8>, String)> = {
        let mut statement = transaction.prepare("SELECT id, purpose, wrapped, wrapped_by FROM dm_keys")?;
        statement.query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)))?.collect::<Result<_, _>>()?
    };
    for (id, purpose, wrapped, wrapped_by) in &rows {
        let key = from.unwrap_key(id, purpose, wrapped, wrapped_by)?;
        transaction
            .execute("UPDATE dm_keys SET wrapped = ?2, wrapped_by = ?3 WHERE id = ?1", params![id, to.wrap(id, purpose, &key)?, to.id])?;
    }
    transaction.execute("UPDATE dm_meta SET value = ?1 WHERE name = 'wrapping_key'", [&to.id])?;
    transaction.commit()?;
    db.execute_batch("VACUUM")?;
    db.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |_| Ok(()))?;
    Ok(rows.len())
}

/// The purpose of an account's own data key (its profile and codes);
/// `<this>:…` for others of the account's.
pub fn account_purpose(account_id: &str) -> String {
    format!("account:{account_id}")
}

/// Forgets `purpose`'s data key, so nothing sealed with it opens again (a
/// deleted conversation, a purged account), backups included once they
/// expire. Whether there was one.
pub fn forget(db: &Connection, purpose: &str) -> Result<bool, Refused> {
    Ok(db.execute("DELETE FROM dm_keys WHERE purpose = ?1", [purpose])? > 0)
}

/// Forgets every data key whose purpose starts with `prefix` (an account's).
pub fn forget_all(db: &Connection, prefix: &str) -> Result<usize, Refused> {
    let pattern = format!("{}%", prefix.replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_"));
    Ok(db.execute("DELETE FROM dm_keys WHERE purpose LIKE ?1 ESCAPE '\\'", [pattern])?)
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: i64 = 1_795_183_320_000;

    fn db(keys: &Keys) -> Connection {
        let db = Connection::open_in_memory().expect("db");
        keys.bind(&db).expect("bound");
        db
    }

    const BODY: Field<'static> = Field { table: "dm_messages", record: "m1", column: "body" };

    #[test]
    fn a_field_opens_only_where_it_was_sealed_and_with_its_keys() {
        let keys = Keys::from_bytes(random());
        let db = db(&keys);
        let sealed = keys.seal(&db, "conversation:c1", BODY, b"see you at 6", NOW).expect("sealed");
        assert_eq!(keys.open(&db, BODY, &sealed).expect("opens").bytes(), b"see you at 6");
        assert!(!sealed.windows(12).any(|window| window == b"see you at 6"), "no plaintext");
        assert_ne!(keys.seal(&db, "conversation:c1", BODY, b"see you at 6", NOW).expect("sealed"), sealed, "a fresh nonce");
        // Moved to another record or column, tampered, or cut: refused.
        let moved = Field { record: "m2", ..BODY };
        assert_eq!(keys.open(&db, moved, &sealed).map(|_| ()), Err(Refused::DoesntOpen));
        assert_eq!(keys.open(&db, Field { column: "draft", ..BODY }, &sealed).map(|_| ()), Err(Refused::DoesntOpen));
        let mut tampered = sealed.clone();
        *tampered.last_mut().expect("a byte") ^= 1;
        assert_eq!(keys.open(&db, BODY, &tampered).map(|_| ()), Err(Refused::DoesntOpen));
        assert_eq!(keys.open(&db, BODY, &sealed[..20]).map(|_| ()), Err(Refused::DoesntOpen));
        let mut other_format = sealed.clone();
        other_format[0] = 2;
        assert_eq!(keys.open(&db, BODY, &other_format).map(|_| ()), Err(Refused::DoesntOpen));
        // One data key per purpose, and another purpose's doesn't stand in.
        let other = keys.seal(&db, "conversation:c2", BODY, b"x", NOW).expect("sealed");
        assert_ne!(other[2..2 + KEY_ID], sealed[2..2 + KEY_ID]);
        let count: i64 = db.query_row("SELECT COUNT(*) FROM dm_keys", [], |row| row.get(0)).expect("count");
        assert_eq!(count, 2);
        // Forgotten: nothing sealed with it opens again.
        assert!(forget(&db, "conversation:c1").expect("forgot"));
        assert_eq!(keys.open(&db, BODY, &sealed).map(|_| ()), Err(Refused::DoesntOpen));
        assert!(keys.open(&db, BODY, &other).is_ok());
        assert_eq!(forget_all(&db, "conversation:").expect("forgot"), 1);
        assert_eq!(forget_all(&db, "conversation:").expect("none left"), 0);
    }

    #[test]
    fn another_wrapping_key_opens_nothing_and_a_database_refuses_it() {
        let keys = Keys::from_bytes(random());
        let db = db(&keys);
        let sealed = keys.seal(&db, "account:a1", BODY, b"profile", NOW).expect("sealed");
        let other = Keys::from_bytes(random());
        assert_eq!(other.open(&db, BODY, &sealed).map(|_| ()), Err(Refused::DoesntOpen));
        assert!(matches!(other.bind(&db), Err(Refused::WrongKey { .. })));
        keys.bind(&db).expect("its own key, again");
        // A data key re-labelled with the other key's id still doesn't open.
        db.execute("UPDATE dm_keys SET wrapped_by = ?1", [other.id()]).expect("relabelled");
        assert_eq!(other.open(&db, BODY, &sealed).map(|_| ()), Err(Refused::DoesntOpen));
        assert_eq!(keys.open(&db, BODY, &sealed).map(|_| ()), Err(Refused::DoesntOpen));
        // Debug shows no key and nothing opened.
        let shown = format!("{keys:?} {:?}", Opened(Zeroizing::new(b"profile".to_vec())));
        assert!(!shown.contains(&hex(keys.wrapping.as_slice())) && !shown.contains("profile"), "{shown}");
    }

    #[test]
    fn rewrapping_moves_every_data_key_to_the_new_wrapping_key_or_changes_nothing() {
        let old = Keys::from_bytes(random());
        let mut db = db(&old);
        let sealed = old.seal(&db, "conversation:c1", BODY, b"hello", NOW).expect("sealed");
        old.seal(&db, "account:a1", BODY, b"profile", NOW).expect("sealed");
        let new = Keys::from_bytes(random());
        // From a key that isn't the database's: refused, nothing changed.
        assert!(matches!(rewrap(&mut db, &new, &old), Err(Refused::WrongKey { .. })));
        assert_eq!(rewrap(&mut db, &old, &new), Ok(2));
        assert_eq!(new.open(&db, BODY, &sealed).expect("opens with the new key").bytes(), b"hello");
        assert_eq!(old.open(&db, BODY, &sealed).map(|_| ()), Err(Refused::DoesntOpen), "not with the old");
        assert!(matches!(old.bind(&db), Err(Refused::WrongKey { .. })));
        new.bind(&db).expect("the new key is the database's");
        // A data key that doesn't open: the whole rewrap is refused.
        db.execute("UPDATE dm_keys SET wrapped = zeroblob(72) WHERE purpose = 'account:a1'", []).expect("broken");
        let newer = Keys::from_bytes(random());
        assert_eq!(rewrap(&mut db, &new, &newer), Err(Refused::DoesntOpen));
        new.bind(&db).expect("still the database's");
        assert!(new.open(&db, BODY, &sealed).is_ok(), "untouched");
    }

    #[test]
    fn rewrapping_leaves_no_old_wrapped_key_in_the_file_or_its_wal() {
        let dir = tempfile::tempdir().expect("a folder");
        let path = dir.path().join("apps.db");
        let old = Keys::from_bytes(random());
        let mut db = Connection::open(&path).expect("db");
        db.pragma_update(None, "journal_mode", "WAL").expect("wal");
        old.bind(&db).expect("bound");
        for n in 0..50 {
            old.seal(&db, &format!("conversation:c{n}"), BODY, b"hello", NOW).expect("sealed");
        }
        let wrapped: Vec<Vec<u8>> = {
            let mut statement = db.prepare("SELECT wrapped FROM dm_keys").expect("select");
            statement.query_map([], |row| row.get(0)).expect("rows").collect::<Result<_, _>>().expect("read")
        };
        let new = Keys::from_bytes(random());
        assert_eq!(rewrap(&mut db, &old, &new), Ok(50));
        let mut bytes = std::fs::read(&path).expect("the file");
        bytes.extend(std::fs::read(dir.path().join("apps.db-wal")).unwrap_or_default());
        for blob in &wrapped {
            assert!(!bytes.windows(blob.len()).any(|window| window == blob.as_slice()), "an old wrapped key is still in the file");
        }
    }

    #[test]
    fn the_key_file_is_made_once_private_and_read_back() {
        use std::os::unix::fs::PermissionsExt as _;
        let dir = tempfile::tempdir().expect("a folder");
        let path = dir.path().join("dm.key");
        let made = Keys::create(&path).expect("made");
        assert_eq!(std::fs::metadata(&path).expect("the file").permissions().mode() & 0o777, 0o600);
        assert!(matches!(Keys::create(&path), Err(Refused::KeyFile(_))), "never over one");
        assert_eq!(Keys::load(&path).expect("read").id(), made.id());
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o640)).expect("chmod");
        assert!(matches!(Keys::load(&path), Err(Refused::KeyFile(_))), "readable by others");
        let bad = dir.path().join("bad.key");
        std::fs::write(&bad, "not a key").expect("written");
        std::fs::set_permissions(&bad, std::fs::Permissions::from_mode(0o600)).expect("chmod");
        let refused = Keys::load(&bad).expect_err("not a key");
        assert!(!refused.to_string().contains("not a key\n"), "{refused}");
    }
}
