//! Mista's (dms-e2ee.md §5.1, §5.2): OpenMLS's storage over the caller's own
//! SQLite connection or transaction, every value sealed.
//!
//! The worker writes a group's `PublicGroup` state in the same transaction
//! that assigns the message's seq, and an endpoint writes its MLS state in
//! the same transaction as the history row, so a crash can never leave group
//! state and the rest of the store apart (the audit's S1-3 class). Hence a
//! [`SqlStorage`] borrows a [`Connection`]: pass a `&Transaction` (it derefs
//! to one) and commit it yourself.
//!
//! One table, [`SCHEMA`]: `mls_values(key, grp, value)`.
//! - `key` is OpenMLS's own: a label, the JSON of its key and the storage
//!   version, as in OpenMLS's memory storage, so values never move between
//!   kinds or versions.
//! - `grp` is the JSON of the group id for every value that belongs to one
//!   group, so [`forget_group`] deletes a group's state without unsealing
//!   anything (revocation and sign-out while the vault is locked). Values
//!   that belong to the leaf rather than a group (its key packages, signature
//!   key pairs, update leaves' key pairs, PSKs) have none.
//! - `value` is the JSON OpenMLS gives, sealed: a format byte (1), a random
//!   24-byte nonce, then XChaCha20-Poly1305 of the JSON with the key as
//!   associated data, so a row moved to another key doesn't open.
//!
//! The format is OpenMLS's serialization, so OpenMLS is pinned exactly and an
//! upgrade is a slice of its own (dms-e2ee.md §3). A value in a format byte
//! this doesn't know is refused as newer, never misread.
//!
//! **Deleted secrets are gone** (forward secrecy; Fugo, E1). OpenMLS forgets
//! an epoch's secrets by deleting or overwriting them, and the seal key lives
//! on the same device, so what SQLite leaves behind matters:
//! - every connection has `PRAGMA secure_delete = ON`, so freed space is
//!   zeroed; [`SqlStorage::new`] refuses a connection without it;
//! - an endpoint's store uses a rollback journal (`journal_mode` DELETE or
//!   TRUNCATE), never WAL, whose old frames would keep overwritten values
//!   until a checkpoint; [`SqlStorage::new`] refuses any other journal;
//! - the worker's store is WAL, so the worker uses [`SqlStorage::public`],
//!   which keeps only a `PublicGroup`'s state (the tree, the group context,
//!   transcript hash, confirmation tag and proposals: nothing secret) and
//!   refuses to write anything else.
//!
//! What flash keeps below SQLite is an accepted residual.

use chacha20poly1305::aead::{Aead, Payload};
use chacha20poly1305::{KeyInit, XChaCha20Poly1305, XNonce};
use openmls_traits::storage::{CURRENT_VERSION, StorageProvider, traits};
use rusqlite::{Connection, OptionalExtension, params};
use serde::Serialize;
use zeroize::Zeroizing;

use crate::{Error, Result};

/// The table, for the caller's own migrations. Never change it in place: a
/// new layout is a new table and a migration that keeps the old one readable.
pub const SCHEMA: &str = "CREATE TABLE IF NOT EXISTS mls_values (
    key BLOB PRIMARY KEY NOT NULL,
    grp BLOB,
    value BLOB NOT NULL
) WITHOUT ROWID;
CREATE INDEX IF NOT EXISTS mls_values_grp ON mls_values (grp) WHERE grp IS NOT NULL;";

/// Each group's epoch floor (Melone's F2, Fugo 40159): the highest epoch an
/// endpoint ever merged in it, recorded as OpenMLS writes the group's
/// context and kept apart from `mls_values`, so neither [`forget_group`] nor
/// a broken state lowers it. A rejoin is refused a GroupInfo below it
/// (`Group::rejoin`'s `floor`, from [`epoch_floor`]), so a sequencer can't
/// send an endpoint back past a Remove it merged. A table of its own, for
/// the caller's migrations after [`SCHEMA`]; the worker's storage
/// ([`SqlStorage::public`]) never writes it.
pub const FLOOR_SCHEMA: &str = "CREATE TABLE IF NOT EXISTS mls_floor (
    grp BLOB PRIMARY KEY NOT NULL,
    epoch INTEGER NOT NULL
) WITHOUT ROWID;";

/// The highest epoch this endpoint merged in `group_id` (0 if none): what
/// `Group::rejoin` takes as its floor.
pub fn epoch_floor(db: &Connection, group_id: &[u8]) -> Result<u64> {
    let grp = json(&openmls::prelude::GroupId::from_slice(group_id))?;
    let epoch: Option<i64> =
        db.query_row("SELECT epoch FROM mls_floor WHERE grp = ?1", params![grp], |row| row.get(0)).optional().map_err(sql)?;
    Ok(epoch.unwrap_or(0).max(0) as u64)
}

/// The sealed value's format byte.
const SEALED_V1: u8 = 1;
const NONCE: usize = 24;
const AAD: &[u8] = b"claktory-dm mls value v1\0";
/// Rows the caller keeps beside OpenMLS's (history, pins, retries): their
/// own prefix, so a row never opens as an MLS value or the other way round.
const ROW_AAD: &[u8] = b"claktory-dm row v1\0";

/// The key every value is sealed with: from the vault (factoryd), the
/// platform key store (devices) or the worker's key file. Never printed.
pub struct SealKey(Zeroizing<[u8; 32]>);

impl SealKey {
    pub fn from_bytes(bytes: [u8; 32]) -> SealKey {
        SealKey(Zeroizing::new(bytes))
    }

    fn cipher(&self) -> XChaCha20Poly1305 {
        XChaCha20Poly1305::new_from_slice(self.0.as_slice()).expect("a 32-byte key")
    }

    /// Seals one of the caller's own rows in this store (decrypted history,
    /// pins, the retry cache), bound to its `place`: the table, the row's
    /// identity and the column, such as `[b"history", conversation, seq,
    /// b"text"]`. The place is the only thing that keeps a sealed value from
    /// being pasted over another row (one friend's pin over another's, one
    /// message's text over another's), so name the exact row. Its parts are
    /// length-prefixed, so no crafted id can collide with another place.
    /// Same format as OpenMLS's values, under another AAD prefix.
    pub fn seal_row(&self, place: &[&[u8]], plain: &[u8]) -> Result<Vec<u8>> {
        self.seal_with(ROW_AAD, &row_place(place), plain)
    }

    /// Opens a row [`SealKey::seal_row`] sealed at the same `place`.
    pub fn open_row(&self, place: &[&[u8]], sealed: &[u8]) -> Result<Zeroizing<Vec<u8>>> {
        self.open_with(ROW_AAD, &row_place(place), sealed)
    }

    fn seal(&self, key: &[u8], plain: &[u8]) -> Result<Vec<u8>> {
        self.seal_with(AAD, key, plain)
    }

    fn open(&self, key: &[u8], sealed: &[u8]) -> Result<Zeroizing<Vec<u8>>> {
        self.open_with(AAD, key, sealed)
    }

    fn seal_with(&self, prefix: &[u8], key: &[u8], plain: &[u8]) -> Result<Vec<u8>> {
        let mut nonce = [0u8; NONCE];
        getrandom::fill(&mut nonce).map_err(|error| Error::Storage(format!("no randomness: {error}")))?;
        let aad = [prefix, key].concat();
        let sealed = self
            .cipher()
            .encrypt(&XNonce::from(nonce), Payload { msg: plain, aad: &aad })
            .map_err(|_| Error::Storage("couldn't seal a value".to_owned()))?;
        Ok([&[SEALED_V1], nonce.as_slice(), &sealed].concat())
    }

    fn open_with(&self, prefix: &[u8], key: &[u8], sealed: &[u8]) -> Result<Zeroizing<Vec<u8>>> {
        let rest = match sealed.split_first() {
            Some((&SEALED_V1, rest)) => rest,
            Some((&format, _)) if format > SEALED_V1 => {
                return Err(Error::Storage(format!("a value in format {format}, from a newer Claktory")));
            }
            _ => return Err(Error::Sealed),
        };
        if rest.len() < NONCE {
            return Err(Error::Sealed);
        }
        let (nonce, body) = rest.split_at(NONCE);
        let nonce: [u8; NONCE] = nonce.try_into().map_err(|_| Error::Sealed)?;
        let aad = [prefix, key].concat();
        self.cipher().decrypt(&XNonce::from(nonce), Payload { msg: body, aad: &aad }).map(Zeroizing::new).map_err(|_| Error::Sealed)
    }
}

/// A row's place, unambiguous: each part's length (u32, big-endian), then
/// the part.
fn row_place(place: &[&[u8]]) -> Vec<u8> {
    place.iter().flat_map(|part| (part.len() as u32).to_be_bytes().into_iter().chain(part.iter().copied())).collect()
}

impl std::fmt::Debug for SealKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("SealKey([hidden])")
    }
}

/// OpenMLS's storage over `db`, which is usually the caller's transaction.
pub struct SqlStorage<'a> {
    db: &'a Connection,
    key: &'a SealKey,
    /// Only a `PublicGroup`'s state: the worker's.
    public: bool,
}

/// Deletes everything stored for one group, without unsealing anything: for
/// revocation, sign-out or a conversation that's gone. Returns how many
/// values went.
pub fn forget_group(db: &Connection, group_id: &openmls::prelude::GroupId) -> Result<usize> {
    let grp = json(group_id)?;
    db.execute("DELETE FROM mls_values WHERE key = ?1", params![rejoin_key(&grp)]).map_err(sql)?;
    db.execute("DELETE FROM mls_values WHERE grp = ?1", params![grp]).map_err(sql)
}

/// Where a rejoin's new state waits for the worker ([`stash_rejoin`]): a row
/// of no group, so [`forget_group`]'s query by group leaves it be.
fn rejoin_key(grp: &[u8]) -> Vec<u8> {
    [b"claktory:rejoin:".as_slice(), grp].concat()
}

/// A rejoin's new state, put aside: the group's rows as they were sealed
/// (each under its own key, so they open only back there), the epoch it
/// reached and the digest (SHA-256) of the external commit that reaches it.
#[derive(Serialize, serde::Deserialize)]
struct Stashed {
    epoch: u64,
    commit: [u8; 32],
    rows: Vec<(Vec<u8>, Vec<u8>)>,
}

/// Puts the state a rejoin just made aside until the worker takes its
/// external `commit` (E11-8), in the same transaction as the rejoin.
/// OpenMLS 0.9's external commit merges itself and stores the group at the
/// new epoch before anyone sequenced it; left so, a commit the worker never
/// takes (refused, lost in a restart) leaves the endpoint on an epoch of its
/// own, its floor raised past the group's. So the group's rows leave
/// `mls_values` (the group has no state here meanwhile) and its floor goes
/// back to `floor_before` ([`epoch_floor`] before the rejoin).
/// [`take_rejoin`] once the worker took it, [`drop_rejoin`] if not.
///
/// The floor's invariant: it rises only with a state this endpoint merged
/// after the worker took the commit that reaches it. Every member commit
/// (`Group::add`, `remove`, `update`, `update_certificate`, `remove_dead`)
/// stays pending, its epoch unwritten, until `merge_pending`; a rejoin's
/// waits here.
pub fn stash_rejoin(db: &Connection, group_id: &[u8], floor_before: u64, commit: &[u8]) -> Result<()> {
    let grp = json(&openmls::prelude::GroupId::from_slice(group_id))?;
    let rows: Vec<(Vec<u8>, Vec<u8>)> = {
        let mut statement = db.prepare("SELECT key, value FROM mls_values WHERE grp = ?1").map_err(sql)?;
        statement.query_map(params![grp], |row| Ok((row.get(0)?, row.get(1)?))).map_err(sql)?.collect::<rusqlite::Result<_>>().map_err(sql)?
    };
    if rows.is_empty() {
        return Err(Error::Storage("no state of the group to put aside".to_owned()));
    }
    let epoch = epoch_floor(db, group_id)?;
    db.execute("DELETE FROM mls_values WHERE grp = ?1", params![grp]).map_err(sql)?;
    if floor_before == 0 {
        db.execute("DELETE FROM mls_floor WHERE grp = ?1", params![grp]).map_err(sql)?;
    } else {
        db.execute("UPDATE mls_floor SET epoch = ?2 WHERE grp = ?1", params![grp, floor_before as i64]).map_err(sql)?;
    }
    let commit = crate::envelope::digest(commit);
    db.execute(
        "INSERT OR REPLACE INTO mls_values (key, grp, value) VALUES (?1, NULL, ?2)",
        params![rejoin_key(&grp), json(&Stashed { epoch, commit, rows })?],
    )
    .map_err(sql)?;
    Ok(())
}

/// The worker took the rejoin's commit, the one whose digest (SHA-256, `envelope::digest`)
/// is `digest`: its state comes back, and the floor rises to its epoch. Whether there was
/// one to take. Matched by the commit's digest, never by its epoch alone: another commit of
/// the same epoch is another's, which the worker took instead.
pub fn take_rejoin(db: &Connection, group_id: &[u8], digest: &[u8]) -> Result<bool> {
    let grp = json(&openmls::prelude::GroupId::from_slice(group_id))?;
    let Some(stashed) = stashed(db, &grp)? else { return Ok(false) };
    if stashed.commit.as_slice() != digest {
        return Err(Error::Refused("another commit than the rejoin's".to_owned()));
    }
    db.execute("DELETE FROM mls_values WHERE grp = ?1", params![grp]).map_err(sql)?;
    for (key, value) in &stashed.rows {
        db.execute("INSERT OR REPLACE INTO mls_values (key, grp, value) VALUES (?1, ?2, ?3)", params![key, grp, value]).map_err(sql)?;
    }
    db.execute(
        "INSERT INTO mls_floor (grp, epoch) VALUES (?1, ?2) ON CONFLICT (grp) DO UPDATE SET epoch = MAX(epoch, excluded.epoch)",
        params![grp, stashed.epoch as i64],
    )
    .map_err(sql)?;
    db.execute("DELETE FROM mls_values WHERE key = ?1", params![rejoin_key(&grp)]).map_err(sql)?;
    Ok(true)
}

/// The worker didn't take the rejoin's commit: its state goes for good
/// (the store deletes securely), and the group stays without one, to
/// rejoin again. Whether there was one.
pub fn drop_rejoin(db: &Connection, group_id: &[u8]) -> Result<bool> {
    let grp = json(&openmls::prelude::GroupId::from_slice(group_id))?;
    Ok(db.execute("DELETE FROM mls_values WHERE key = ?1", params![rejoin_key(&grp)]).map_err(sql)? > 0)
}

/// The epoch a rejoin of the group waiting for the worker reaches
/// ([`stash_rejoin`]), if one waits: its commit is of the epoch before.
pub fn pending_rejoin(db: &Connection, group_id: &[u8]) -> Result<Option<u64>> {
    let grp = json(&openmls::prelude::GroupId::from_slice(group_id))?;
    Ok(stashed(db, &grp)?.map(|stashed| stashed.epoch))
}

fn stashed(db: &Connection, grp: &[u8]) -> Result<Option<Stashed>> {
    let value: Option<Vec<u8>> =
        db.query_row("SELECT value FROM mls_values WHERE key = ?1", params![rejoin_key(grp)], |row| row.get(0)).optional().map_err(sql)?;
    value.map(|value| unjson(&value)).transpose()
}

/// Deletes the key packages this endpoint made whose lifetime ended by
/// `now` (seconds since the epoch), their private init and encryption keys
/// with them (Melone's F4): one never used, or a last resort replaced since,
/// opens no Welcome after. A used one-time package is gone already (OpenMLS
/// deletes it as it joins). Returns how many went; one that doesn't open
/// under `key` is left alone.
pub fn sweep_key_packages(db: &Connection, key: &SealKey, now: u64) -> Result<usize> {
    let rows: Vec<(Vec<u8>, Vec<u8>)> = {
        let mut statement = db.prepare("SELECT key, value FROM mls_values WHERE grp IS NULL AND substr(key, 1, ?1) = ?2").map_err(sql)?;
        statement
            .query_map(params![KEY_PACKAGE.len() as i64, KEY_PACKAGE], |row| Ok((row.get(0)?, row.get(1)?)))
            .map_err(sql)?
            .collect::<std::result::Result<_, _>>()
            .map_err(sql)?
    };
    let mut gone = 0;
    for (at, sealed) in rows {
        let Ok(plain) = key.open(&at, &sealed) else { continue };
        let Ok(bundle) = serde_json::from_slice::<openmls::prelude::KeyPackageBundle>(&plain) else { continue };
        if bundle.key_package().life_time().not_after() <= now {
            gone += db.execute("DELETE FROM mls_values WHERE key = ?1", params![at]).map_err(sql)?;
        }
    }
    Ok(gone)
}

impl SqlStorage<'_> {
    pub(crate) fn sweep_key_packages(&self, now: u64) -> Result<usize> {
        sweep_key_packages(self.db, self.key, now)
    }
}

fn sql(error: rusqlite::Error) -> Error {
    Error::Storage(error.to_string())
}

fn json<T: Serialize + ?Sized>(value: &T) -> Result<Vec<u8>> {
    serde_json::to_vec(value).map_err(|error| Error::Storage(format!("couldn't serialize: {error}")))
}

fn unjson<V: serde::de::DeserializeOwned>(bytes: &[u8]) -> Result<V> {
    serde_json::from_slice(bytes).map_err(|error| Error::Storage(format!("couldn't read a stored value: {error}")))
}

/// OpenMLS's storage key: label, key, version.
fn storage_key(label: &[u8], key: &[u8]) -> Vec<u8> {
    [label, key, &CURRENT_VERSION.to_be_bytes()].concat()
}

const KEY_PACKAGE: &[u8] = b"KeyPackage";
const PSK: &[u8] = b"Psk";
const ENCRYPTION_KEY_PAIR: &[u8] = b"EncryptionKeyPair";
const SIGNATURE_KEY_PAIR: &[u8] = b"SignatureKeyPair";
const EPOCH_KEY_PAIRS: &[u8] = b"EpochKeyPairs";
// The PublicGroup's.
const TREE: &[u8] = b"Tree";
const GROUP_CONTEXT: &[u8] = b"GroupContext";
const INTERIM_TRANSCRIPT_HASH: &[u8] = b"InterimTranscriptHash";
const CONFIRMATION_TAG: &[u8] = b"ConfirmationTag";
// The MlsGroup's.
const JOIN_CONFIG: &[u8] = b"MlsGroupJoinConfig";
const OWN_LEAF_NODES: &[u8] = b"OwnLeafNodes";
const GROUP_STATE: &[u8] = b"GroupState";
const QUEUED_PROPOSAL: &[u8] = b"QueuedProposal";
const PROPOSAL_QUEUE_REFS: &[u8] = b"ProposalQueueRefs";
const OWN_LEAF_NODE_INDEX: &[u8] = b"OwnLeafNodeIndex";
const EPOCH_SECRETS: &[u8] = b"EpochSecrets";
const RESUMPTION_PSK_STORE: &[u8] = b"ResumptionPsk";
const MESSAGE_SECRETS: &[u8] = b"MessageSecrets";

/// What a `PublicGroup` stores: nothing secret.
const PUBLIC: [&[u8]; 6] = [TREE, GROUP_CONTEXT, INTERIM_TRANSCRIPT_HASH, CONFIRMATION_TAG, QUEUED_PROPOSAL, PROPOSAL_QUEUE_REFS];
/// What belongs to the leaf rather than a group, and so has no `grp`.
const LEAF: [&[u8]; 4] = [KEY_PACKAGE, PSK, ENCRYPTION_KEY_PAIR, SIGNATURE_KEY_PAIR];

fn pragma(db: &Connection, name: &str) -> Result<String> {
    db.query_row(&format!("PRAGMA {name}"), [], |row| row.get::<_, rusqlite::types::Value>(0))
        .map(|value| match value {
            rusqlite::types::Value::Integer(n) => n.to_string(),
            rusqlite::types::Value::Text(text) => text.to_ascii_lowercase(),
            _ => String::new(),
        })
        .map_err(sql)
}

fn secure_delete(db: &Connection) -> Result<()> {
    if pragma(db, "secure_delete")? != "1" {
        return Err(Error::Storage("this store needs PRAGMA secure_delete = ON, so deleted secrets are gone".to_owned()));
    }
    Ok(())
}

impl<'a> SqlStorage<'a> {
    /// An endpoint's storage, for everything OpenMLS keeps. Refused unless
    /// the connection has `secure_delete` on and a rollback journal (DELETE
    /// or TRUNCATE).
    pub fn new(db: &'a Connection, key: &'a SealKey) -> Result<SqlStorage<'a>> {
        secure_delete(db)?;
        let journal = pragma(db, "journal_mode")?;
        if !matches!(journal.as_str(), "delete" | "truncate" | "memory") {
            return Err(Error::Storage(format!(
                "this store needs a rollback journal (DELETE or TRUNCATE), not {journal}, so overwritten secrets don't linger"
            )));
        }
        Ok(SqlStorage { db, key, public: false })
    }

    /// The worker's storage, for `PublicGroup`s only: any journal (the
    /// worker's is WAL), `secure_delete` on, and nothing secret is ever
    /// written.
    pub fn public(db: &'a Connection, key: &'a SealKey) -> Result<SqlStorage<'a>> {
        secure_delete(db)?;
        Ok(SqlStorage { db, key, public: true })
    }

    /// The opened value at (label, key), if any.
    fn get(&self, label: &[u8], key: &[u8]) -> Result<Option<Zeroizing<Vec<u8>>>> {
        let at = storage_key(label, key);
        let sealed: Option<Vec<u8>> = self
            .db
            .prepare_cached("SELECT value FROM mls_values WHERE key = ?1")
            .and_then(|mut statement| statement.query_row(params![at], |row| row.get(0)).optional())
            .map_err(sql)?;
        sealed.map(|sealed| self.key.open(&at, &sealed)).transpose()
    }

    fn put(&self, label: &[u8], key: &[u8], grp: Option<&[u8]>, plain: &[u8]) -> Result<()> {
        // Every group's value has its group, so forget_group finds it.
        debug_assert_eq!(grp.is_none(), LEAF.contains(&label), "{}", String::from_utf8_lossy(label));
        if self.public && !PUBLIC.contains(&label) {
            return Err(Error::Storage(format!("the service's storage never holds {}", String::from_utf8_lossy(label))));
        }
        let at = storage_key(label, key);
        let sealed = self.key.seal(&at, plain)?;
        self.db
            .prepare_cached("INSERT OR REPLACE INTO mls_values (key, grp, value) VALUES (?1, ?2, ?3)")
            .and_then(|mut statement| statement.execute(params![at, grp, sealed]))
            .map_err(sql)?;
        Ok(())
    }

    fn delete(&self, label: &[u8], key: &[u8]) -> Result<()> {
        let at = storage_key(label, key);
        self.db
            .prepare_cached("DELETE FROM mls_values WHERE key = ?1")
            .and_then(|mut statement| statement.execute(params![at]))
            .map_err(sql)?;
        Ok(())
    }

    fn read<V: serde::de::DeserializeOwned>(&self, label: &[u8], key: &[u8]) -> Result<Option<V>> {
        self.get(label, key)?.map(|plain| unjson(&plain)).transpose()
    }

    /// A list value: the JSON of a list of each item's JSON, as OpenMLS's
    /// memory storage keeps them.
    fn items(&self, label: &[u8], key: &[u8]) -> Result<Vec<Vec<u8>>> {
        match self.get(label, key)? {
            Some(plain) => unjson(&plain),
            None => Ok(Vec::new()),
        }
    }

    fn read_list<V: serde::de::DeserializeOwned>(&self, label: &[u8], key: &[u8]) -> Result<Vec<V>> {
        self.items(label, key)?.iter().map(|item| unjson(item)).collect()
    }

    fn append(&self, label: &[u8], key: &[u8], grp: &[u8], item: Vec<u8>) -> Result<()> {
        let mut items = self.items(label, key)?;
        items.push(item);
        self.put(label, key, Some(grp), &Zeroizing::new(json(&items)?))
    }

    fn remove_item(&self, label: &[u8], key: &[u8], grp: &[u8], item: &[u8]) -> Result<()> {
        let mut items = self.items(label, key)?;
        if let Some(at) = items.iter().position(|stored| stored == item) {
            items.remove(at);
        }
        self.put(label, key, Some(grp), &Zeroizing::new(json(&items)?))
    }

    /// Writes a value that belongs to `group_id`, keyed by it.
    fn put_group<G: Serialize, V: Serialize>(&self, label: &[u8], group_id: &G, value: &V) -> Result<()> {
        let grp = json(group_id)?;
        self.put(label, &grp, Some(&grp), &Zeroizing::new(json(value)?))
    }

    /// Writes a value that belongs to the leaf, not a group.
    fn put_leaf<K: Serialize, V: Serialize>(&self, label: &[u8], key: &K, value: &V) -> Result<()> {
        self.put(label, &json(key)?, None, &Zeroizing::new(json(value)?))
    }

    fn read_by<K: Serialize, V: serde::de::DeserializeOwned>(&self, label: &[u8], key: &K) -> Result<Option<V>> {
        self.read(label, &json(key)?)
    }

    fn delete_by<K: Serialize>(&self, label: &[u8], key: &K) -> Result<()> {
        self.delete(label, &json(key)?)
    }

    fn epoch_key_pairs_id<G: Serialize, E: Serialize>(group_id: &G, epoch: &E, leaf_index: u32) -> Result<Vec<u8>> {
        Ok([json(group_id)?, json(epoch)?, json(&leaf_index)?].concat())
    }
}

impl StorageProvider<CURRENT_VERSION> for SqlStorage<'_> {
    type Error = Error;

    fn write_mls_join_config<GroupId: traits::GroupId<CURRENT_VERSION>, MlsGroupJoinConfig: traits::MlsGroupJoinConfig<CURRENT_VERSION>>(
        &self,
        group_id: &GroupId,
        config: &MlsGroupJoinConfig,
    ) -> Result<()> {
        self.put_group(JOIN_CONFIG, group_id, config)
    }

    fn append_own_leaf_node<GroupId: traits::GroupId<CURRENT_VERSION>, LeafNode: traits::LeafNode<CURRENT_VERSION>>(
        &self,
        group_id: &GroupId,
        leaf_node: &LeafNode,
    ) -> Result<()> {
        let grp = json(group_id)?;
        self.append(OWN_LEAF_NODES, &grp, &grp, json(leaf_node)?)
    }

    fn queue_proposal<
        GroupId: traits::GroupId<CURRENT_VERSION>,
        ProposalRef: traits::ProposalRef<CURRENT_VERSION>,
        QueuedProposal: traits::QueuedProposal<CURRENT_VERSION>,
    >(
        &self,
        group_id: &GroupId,
        proposal_ref: &ProposalRef,
        proposal: &QueuedProposal,
    ) -> Result<()> {
        let grp = json(group_id)?;
        self.put(QUEUED_PROPOSAL, &json(&(group_id, proposal_ref))?, Some(&grp), &Zeroizing::new(json(proposal)?))?;
        self.append(PROPOSAL_QUEUE_REFS, &grp, &grp, json(proposal_ref)?)
    }

    fn write_tree<GroupId: traits::GroupId<CURRENT_VERSION>, TreeSync: traits::TreeSync<CURRENT_VERSION>>(
        &self,
        group_id: &GroupId,
        tree: &TreeSync,
    ) -> Result<()> {
        self.put_group(TREE, group_id, tree)
    }

    fn write_interim_transcript_hash<
        GroupId: traits::GroupId<CURRENT_VERSION>,
        InterimTranscriptHash: traits::InterimTranscriptHash<CURRENT_VERSION>,
    >(
        &self,
        group_id: &GroupId,
        interim_transcript_hash: &InterimTranscriptHash,
    ) -> Result<()> {
        self.put_group(INTERIM_TRANSCRIPT_HASH, group_id, interim_transcript_hash)
    }

    fn write_context<GroupId: traits::GroupId<CURRENT_VERSION>, GroupContext: traits::GroupContext<CURRENT_VERSION>>(
        &self,
        group_id: &GroupId,
        group_context: &GroupContext,
    ) -> Result<()> {
        self.put_group(GROUP_CONTEXT, group_id, group_context)?;
        if !self.public {
            // The floor only rises (F2).
            let epoch = serde_json::to_value(group_context)
                .ok()
                .and_then(|context| context.get("epoch").and_then(serde_json::Value::as_u64))
                .ok_or_else(|| Error::Storage("a group context without its epoch".to_owned()))?;
            self.db
                .execute(
                    "INSERT INTO mls_floor (grp, epoch) VALUES (?1, ?2) ON CONFLICT (grp) DO UPDATE SET epoch = MAX(epoch, excluded.epoch)",
                    params![json(group_id)?, epoch as i64],
                )
                .map_err(sql)?;
        }
        Ok(())
    }

    fn write_confirmation_tag<GroupId: traits::GroupId<CURRENT_VERSION>, ConfirmationTag: traits::ConfirmationTag<CURRENT_VERSION>>(
        &self,
        group_id: &GroupId,
        confirmation_tag: &ConfirmationTag,
    ) -> Result<()> {
        self.put_group(CONFIRMATION_TAG, group_id, confirmation_tag)
    }

    fn write_group_state<GroupState: traits::GroupState<CURRENT_VERSION>, GroupId: traits::GroupId<CURRENT_VERSION>>(
        &self,
        group_id: &GroupId,
        group_state: &GroupState,
    ) -> Result<()> {
        self.put_group(GROUP_STATE, group_id, group_state)
    }

    fn write_message_secrets<GroupId: traits::GroupId<CURRENT_VERSION>, MessageSecrets: traits::MessageSecrets<CURRENT_VERSION>>(
        &self,
        group_id: &GroupId,
        message_secrets: &MessageSecrets,
    ) -> Result<()> {
        self.put_group(MESSAGE_SECRETS, group_id, message_secrets)
    }

    fn write_resumption_psk_store<
        GroupId: traits::GroupId<CURRENT_VERSION>,
        ResumptionPskStore: traits::ResumptionPskStore<CURRENT_VERSION>,
    >(
        &self,
        group_id: &GroupId,
        resumption_psk_store: &ResumptionPskStore,
    ) -> Result<()> {
        self.put_group(RESUMPTION_PSK_STORE, group_id, resumption_psk_store)
    }

    fn write_own_leaf_index<GroupId: traits::GroupId<CURRENT_VERSION>, LeafNodeIndex: traits::LeafNodeIndex<CURRENT_VERSION>>(
        &self,
        group_id: &GroupId,
        own_leaf_index: &LeafNodeIndex,
    ) -> Result<()> {
        self.put_group(OWN_LEAF_NODE_INDEX, group_id, own_leaf_index)
    }

    fn write_group_epoch_secrets<
        GroupId: traits::GroupId<CURRENT_VERSION>,
        GroupEpochSecrets: traits::GroupEpochSecrets<CURRENT_VERSION>,
    >(
        &self,
        group_id: &GroupId,
        group_epoch_secrets: &GroupEpochSecrets,
    ) -> Result<()> {
        self.put_group(EPOCH_SECRETS, group_id, group_epoch_secrets)
    }

    fn write_signature_key_pair<
        SignaturePublicKey: traits::SignaturePublicKey<CURRENT_VERSION>,
        SignatureKeyPair: traits::SignatureKeyPair<CURRENT_VERSION>,
    >(
        &self,
        public_key: &SignaturePublicKey,
        signature_key_pair: &SignatureKeyPair,
    ) -> Result<()> {
        self.put_leaf(SIGNATURE_KEY_PAIR, public_key, signature_key_pair)
    }

    fn write_encryption_key_pair<
        EncryptionKey: traits::EncryptionKey<CURRENT_VERSION>,
        HpkeKeyPair: traits::HpkeKeyPair<CURRENT_VERSION>,
    >(
        &self,
        public_key: &EncryptionKey,
        key_pair: &HpkeKeyPair,
    ) -> Result<()> {
        self.put_leaf(ENCRYPTION_KEY_PAIR, public_key, key_pair)
    }

    fn write_encryption_epoch_key_pairs<
        GroupId: traits::GroupId<CURRENT_VERSION>,
        EpochKey: traits::EpochKey<CURRENT_VERSION>,
        HpkeKeyPair: traits::HpkeKeyPair<CURRENT_VERSION>,
    >(
        &self,
        group_id: &GroupId,
        epoch: &EpochKey,
        leaf_index: u32,
        key_pairs: &[HpkeKeyPair],
    ) -> Result<()> {
        let key = Self::epoch_key_pairs_id(group_id, epoch, leaf_index)?;
        self.put(EPOCH_KEY_PAIRS, &key, Some(&json(group_id)?), &Zeroizing::new(json(key_pairs)?))
    }

    fn write_key_package<HashReference: traits::HashReference<CURRENT_VERSION>, KeyPackage: traits::KeyPackage<CURRENT_VERSION>>(
        &self,
        hash_ref: &HashReference,
        key_package: &KeyPackage,
    ) -> Result<()> {
        self.put_leaf(KEY_PACKAGE, hash_ref, key_package)
    }

    fn write_psk<PskId: traits::PskId<CURRENT_VERSION>, PskBundle: traits::PskBundle<CURRENT_VERSION>>(
        &self,
        psk_id: &PskId,
        psk: &PskBundle,
    ) -> Result<()> {
        self.put_leaf(PSK, psk_id, psk)
    }

    fn mls_group_join_config<GroupId: traits::GroupId<CURRENT_VERSION>, MlsGroupJoinConfig: traits::MlsGroupJoinConfig<CURRENT_VERSION>>(
        &self,
        group_id: &GroupId,
    ) -> Result<Option<MlsGroupJoinConfig>> {
        self.read_by(JOIN_CONFIG, group_id)
    }

    fn own_leaf_nodes<GroupId: traits::GroupId<CURRENT_VERSION>, LeafNode: traits::LeafNode<CURRENT_VERSION>>(
        &self,
        group_id: &GroupId,
    ) -> Result<Vec<LeafNode>> {
        self.read_list(OWN_LEAF_NODES, &json(group_id)?)
    }

    fn queued_proposal_refs<GroupId: traits::GroupId<CURRENT_VERSION>, ProposalRef: traits::ProposalRef<CURRENT_VERSION>>(
        &self,
        group_id: &GroupId,
    ) -> Result<Vec<ProposalRef>> {
        self.read_list(PROPOSAL_QUEUE_REFS, &json(group_id)?)
    }

    fn queued_proposals<
        GroupId: traits::GroupId<CURRENT_VERSION>,
        ProposalRef: traits::ProposalRef<CURRENT_VERSION>,
        QueuedProposal: traits::QueuedProposal<CURRENT_VERSION>,
    >(
        &self,
        group_id: &GroupId,
    ) -> Result<Vec<(ProposalRef, QueuedProposal)>> {
        let refs: Vec<ProposalRef> = self.read_list(PROPOSAL_QUEUE_REFS, &json(group_id)?)?;
        refs.into_iter()
            .map(|proposal_ref| {
                let proposal = self
                    .read_by(QUEUED_PROPOSAL, &(group_id, &proposal_ref))?
                    .ok_or_else(|| Error::Storage("a queued proposal is missing".to_owned()))?;
                Ok((proposal_ref, proposal))
            })
            .collect()
    }

    fn tree<GroupId: traits::GroupId<CURRENT_VERSION>, TreeSync: traits::TreeSync<CURRENT_VERSION>>(
        &self,
        group_id: &GroupId,
    ) -> Result<Option<TreeSync>> {
        self.read_by(TREE, group_id)
    }

    fn group_context<GroupId: traits::GroupId<CURRENT_VERSION>, GroupContext: traits::GroupContext<CURRENT_VERSION>>(
        &self,
        group_id: &GroupId,
    ) -> Result<Option<GroupContext>> {
        self.read_by(GROUP_CONTEXT, group_id)
    }

    fn interim_transcript_hash<
        GroupId: traits::GroupId<CURRENT_VERSION>,
        InterimTranscriptHash: traits::InterimTranscriptHash<CURRENT_VERSION>,
    >(
        &self,
        group_id: &GroupId,
    ) -> Result<Option<InterimTranscriptHash>> {
        self.read_by(INTERIM_TRANSCRIPT_HASH, group_id)
    }

    fn confirmation_tag<GroupId: traits::GroupId<CURRENT_VERSION>, ConfirmationTag: traits::ConfirmationTag<CURRENT_VERSION>>(
        &self,
        group_id: &GroupId,
    ) -> Result<Option<ConfirmationTag>> {
        self.read_by(CONFIRMATION_TAG, group_id)
    }

    fn group_state<GroupState: traits::GroupState<CURRENT_VERSION>, GroupId: traits::GroupId<CURRENT_VERSION>>(
        &self,
        group_id: &GroupId,
    ) -> Result<Option<GroupState>> {
        self.read_by(GROUP_STATE, group_id)
    }

    fn message_secrets<GroupId: traits::GroupId<CURRENT_VERSION>, MessageSecrets: traits::MessageSecrets<CURRENT_VERSION>>(
        &self,
        group_id: &GroupId,
    ) -> Result<Option<MessageSecrets>> {
        self.read_by(MESSAGE_SECRETS, group_id)
    }

    fn resumption_psk_store<GroupId: traits::GroupId<CURRENT_VERSION>, ResumptionPskStore: traits::ResumptionPskStore<CURRENT_VERSION>>(
        &self,
        group_id: &GroupId,
    ) -> Result<Option<ResumptionPskStore>> {
        self.read_by(RESUMPTION_PSK_STORE, group_id)
    }

    fn own_leaf_index<GroupId: traits::GroupId<CURRENT_VERSION>, LeafNodeIndex: traits::LeafNodeIndex<CURRENT_VERSION>>(
        &self,
        group_id: &GroupId,
    ) -> Result<Option<LeafNodeIndex>> {
        self.read_by(OWN_LEAF_NODE_INDEX, group_id)
    }

    fn group_epoch_secrets<GroupId: traits::GroupId<CURRENT_VERSION>, GroupEpochSecrets: traits::GroupEpochSecrets<CURRENT_VERSION>>(
        &self,
        group_id: &GroupId,
    ) -> Result<Option<GroupEpochSecrets>> {
        self.read_by(EPOCH_SECRETS, group_id)
    }

    fn signature_key_pair<
        SignaturePublicKey: traits::SignaturePublicKey<CURRENT_VERSION>,
        SignatureKeyPair: traits::SignatureKeyPair<CURRENT_VERSION>,
    >(
        &self,
        public_key: &SignaturePublicKey,
    ) -> Result<Option<SignatureKeyPair>> {
        self.read_by(SIGNATURE_KEY_PAIR, public_key)
    }

    fn encryption_key_pair<HpkeKeyPair: traits::HpkeKeyPair<CURRENT_VERSION>, EncryptionKey: traits::EncryptionKey<CURRENT_VERSION>>(
        &self,
        public_key: &EncryptionKey,
    ) -> Result<Option<HpkeKeyPair>> {
        self.read_by(ENCRYPTION_KEY_PAIR, public_key)
    }

    fn encryption_epoch_key_pairs<
        GroupId: traits::GroupId<CURRENT_VERSION>,
        EpochKey: traits::EpochKey<CURRENT_VERSION>,
        HpkeKeyPair: traits::HpkeKeyPair<CURRENT_VERSION>,
    >(
        &self,
        group_id: &GroupId,
        epoch: &EpochKey,
        leaf_index: u32,
    ) -> Result<Vec<HpkeKeyPair>> {
        let key = Self::epoch_key_pairs_id(group_id, epoch, leaf_index)?;
        Ok(self.read(EPOCH_KEY_PAIRS, &key)?.unwrap_or_default())
    }

    fn key_package<KeyPackageRef: traits::HashReference<CURRENT_VERSION>, KeyPackage: traits::KeyPackage<CURRENT_VERSION>>(
        &self,
        hash_ref: &KeyPackageRef,
    ) -> Result<Option<KeyPackage>> {
        self.read_by(KEY_PACKAGE, hash_ref)
    }

    fn psk<PskBundle: traits::PskBundle<CURRENT_VERSION>, PskId: traits::PskId<CURRENT_VERSION>>(
        &self,
        psk_id: &PskId,
    ) -> Result<Option<PskBundle>> {
        self.read_by(PSK, psk_id)
    }

    fn remove_proposal<GroupId: traits::GroupId<CURRENT_VERSION>, ProposalRef: traits::ProposalRef<CURRENT_VERSION>>(
        &self,
        group_id: &GroupId,
        proposal_ref: &ProposalRef,
    ) -> Result<()> {
        let grp = json(group_id)?;
        self.remove_item(PROPOSAL_QUEUE_REFS, &grp, &grp, &json(proposal_ref)?)?;
        self.delete_by(QUEUED_PROPOSAL, &(group_id, proposal_ref))
    }

    fn delete_own_leaf_nodes<GroupId: traits::GroupId<CURRENT_VERSION>>(&self, group_id: &GroupId) -> Result<()> {
        self.delete_by(OWN_LEAF_NODES, group_id)
    }

    fn delete_group_config<GroupId: traits::GroupId<CURRENT_VERSION>>(&self, group_id: &GroupId) -> Result<()> {
        self.delete_by(JOIN_CONFIG, group_id)
    }

    fn delete_tree<GroupId: traits::GroupId<CURRENT_VERSION>>(&self, group_id: &GroupId) -> Result<()> {
        self.delete_by(TREE, group_id)
    }

    fn delete_confirmation_tag<GroupId: traits::GroupId<CURRENT_VERSION>>(&self, group_id: &GroupId) -> Result<()> {
        self.delete_by(CONFIRMATION_TAG, group_id)
    }

    fn delete_group_state<GroupId: traits::GroupId<CURRENT_VERSION>>(&self, group_id: &GroupId) -> Result<()> {
        self.delete_by(GROUP_STATE, group_id)
    }

    fn delete_context<GroupId: traits::GroupId<CURRENT_VERSION>>(&self, group_id: &GroupId) -> Result<()> {
        self.delete_by(GROUP_CONTEXT, group_id)
    }

    fn delete_interim_transcript_hash<GroupId: traits::GroupId<CURRENT_VERSION>>(&self, group_id: &GroupId) -> Result<()> {
        self.delete_by(INTERIM_TRANSCRIPT_HASH, group_id)
    }

    fn delete_message_secrets<GroupId: traits::GroupId<CURRENT_VERSION>>(&self, group_id: &GroupId) -> Result<()> {
        self.delete_by(MESSAGE_SECRETS, group_id)
    }

    fn delete_all_resumption_psk_secrets<GroupId: traits::GroupId<CURRENT_VERSION>>(&self, group_id: &GroupId) -> Result<()> {
        self.delete_by(RESUMPTION_PSK_STORE, group_id)
    }

    fn delete_own_leaf_index<GroupId: traits::GroupId<CURRENT_VERSION>>(&self, group_id: &GroupId) -> Result<()> {
        self.delete_by(OWN_LEAF_NODE_INDEX, group_id)
    }

    fn delete_group_epoch_secrets<GroupId: traits::GroupId<CURRENT_VERSION>>(&self, group_id: &GroupId) -> Result<()> {
        self.delete_by(EPOCH_SECRETS, group_id)
    }

    fn clear_proposal_queue<GroupId: traits::GroupId<CURRENT_VERSION>, ProposalRef: traits::ProposalRef<CURRENT_VERSION>>(
        &self,
        group_id: &GroupId,
    ) -> Result<()> {
        // OpenMLS's memory storage deletes these without their label, so
        // they stay; here they go.
        let refs: Vec<ProposalRef> = self.read_list(PROPOSAL_QUEUE_REFS, &json(group_id)?)?;
        for proposal_ref in refs {
            self.delete_by(QUEUED_PROPOSAL, &(group_id, proposal_ref))?;
        }
        self.delete_by(PROPOSAL_QUEUE_REFS, group_id)
    }

    fn delete_signature_key_pair<SignaturePublicKey: traits::SignaturePublicKey<CURRENT_VERSION>>(
        &self,
        public_key: &SignaturePublicKey,
    ) -> Result<()> {
        self.delete_by(SIGNATURE_KEY_PAIR, public_key)
    }

    fn delete_encryption_key_pair<EncryptionKey: traits::EncryptionKey<CURRENT_VERSION>>(&self, public_key: &EncryptionKey) -> Result<()> {
        self.delete_by(ENCRYPTION_KEY_PAIR, public_key)
    }

    fn delete_encryption_epoch_key_pairs<GroupId: traits::GroupId<CURRENT_VERSION>, EpochKey: traits::EpochKey<CURRENT_VERSION>>(
        &self,
        group_id: &GroupId,
        epoch: &EpochKey,
        leaf_index: u32,
    ) -> Result<()> {
        self.delete(EPOCH_KEY_PAIRS, &Self::epoch_key_pairs_id(group_id, epoch, leaf_index)?)
    }

    fn delete_key_package<KeyPackageRef: traits::HashReference<CURRENT_VERSION>>(&self, hash_ref: &KeyPackageRef) -> Result<()> {
        self.delete_by(KEY_PACKAGE, hash_ref)
    }

    fn delete_psk<PskKey: traits::PskId<CURRENT_VERSION>>(&self, psk_id: &PskKey) -> Result<()> {
        self.delete_by(PSK, psk_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> Connection {
        let db = Connection::open_in_memory().unwrap();
        db.pragma_update(None, "secure_delete", "ON").unwrap();
        db.execute_batch(SCHEMA).unwrap();
        db.execute_batch(FLOOR_SCHEMA).unwrap();
        db
    }

    #[test]
    fn a_store_that_would_keep_deleted_secrets_is_refused() {
        let key = SealKey::from_bytes([3; 32]);
        let bare = Connection::open_in_memory().unwrap();
        assert!(SqlStorage::new(&bare, &key).is_err());
        assert!(SqlStorage::public(&bare, &key).is_err());

        let path = std::env::temp_dir().join(format!("claktory-dm-wal-{}.db", std::process::id()));
        let wal = Connection::open(&path).unwrap();
        wal.pragma_update(None, "journal_mode", "WAL").unwrap();
        wal.pragma_update(None, "secure_delete", "ON").unwrap();
        wal.execute_batch(SCHEMA).unwrap();
        assert!(SqlStorage::new(&wal, &key).is_err());
        let public = SqlStorage::public(&wal, &key).unwrap();
        public.put(TREE, b"g", Some(b"g"), b"{}").unwrap();
        assert!(public.put(MESSAGE_SECRETS, b"g", Some(b"g"), b"{}").is_err());
        assert!(public.put(SIGNATURE_KEY_PAIR, b"k", None, b"{}").is_err());
        drop(wal);
        for suffix in ["", "-wal", "-shm"] {
            let _ = std::fs::remove_file(format!("{}{suffix}", path.display()));
        }
    }

    #[test]
    fn rows_and_mls_values_never_open_as_each_other() {
        let key = SealKey::from_bytes([5; 32]);
        let place: &[&[u8]] = &[b"history", b"c1", b"42", b"text"];
        let row = key.seal_row(place, b"hello").unwrap();
        assert_eq!(key.open_row(place, &row).unwrap().as_slice(), b"hello");
        assert!(matches!(key.open_row(&[b"history", b"c1", b"43", b"text"], &row), Err(Error::Sealed)));
        // Parts can't run into each other.
        assert!(matches!(key.open_row(&[b"history", b"c14", b"2", b"text"], &row), Err(Error::Sealed)));
        assert!(matches!(key.open(&row_place(place), &row), Err(Error::Sealed)));
        let value = key.seal(b"k", b"{}").unwrap();
        assert!(matches!(key.open_row(&[b"k"], &value), Err(Error::Sealed)));
        assert!(matches!(SealKey::from_bytes([6; 32]).open_row(place, &row), Err(Error::Sealed)));
    }

    #[test]
    fn a_newer_format_is_refused_not_misread() {
        let key = SealKey::from_bytes([4; 32]);
        let mut sealed = key.seal(b"k", b"{}").unwrap();
        sealed[0] = 2;
        assert!(matches!(key.open(b"k", &sealed), Err(Error::Storage(_))));
        sealed[0] = 0;
        assert!(matches!(key.open(b"k", &sealed), Err(Error::Sealed)));
    }

    #[test]
    fn a_value_is_sealed_and_bound_to_its_key() {
        let db = store();
        let key = SealKey::from_bytes([7; 32]);
        let storage = SqlStorage::new(&db, &key).unwrap();
        storage.put(TREE, b"\"g\"", Some(b"\"g\""), b"{\"secret\":\"plain words\"}").unwrap();
        storage.put(TREE, b"\"h\"", Some(b"\"h\""), b"{}").unwrap();
        let raw: Vec<u8> =
            db.query_row("SELECT value FROM mls_values WHERE key = ?1", [storage_key(TREE, b"\"g\"")], |row| row.get(0)).unwrap();
        assert_eq!(raw[0], SEALED_V1);
        assert!(!raw.windows(11).any(|window| window == b"plain words"));
        assert_eq!(storage.get(TREE, b"\"g\"").unwrap().unwrap().as_slice(), b"{\"secret\":\"plain words\"}");

        // Moved under another key, it doesn't open.
        db.execute("UPDATE mls_values SET value = ?1 WHERE key = ?2", rusqlite::params![raw, storage_key(TREE, b"\"h\"")]).unwrap();
        assert!(matches!(storage.get(TREE, b"\"h\""), Err(Error::Sealed)));

        // Under another seal key, nothing opens.
        let other = SealKey::from_bytes([8; 32]);
        assert!(matches!(SqlStorage::new(&db, &other).unwrap().get(TREE, b"\"g\""), Err(Error::Sealed)));
    }

    #[test]
    fn lists_append_and_remove() {
        let db = store();
        let key = SealKey::from_bytes([1; 32]);
        let storage = SqlStorage::new(&db, &key).unwrap();
        storage.append(PROPOSAL_QUEUE_REFS, b"g", b"g", b"1".to_vec()).unwrap();
        storage.append(PROPOSAL_QUEUE_REFS, b"g", b"g", b"2".to_vec()).unwrap();
        storage.remove_item(PROPOSAL_QUEUE_REFS, b"g", b"g", b"1").unwrap();
        let left: Vec<u32> = storage.read_list(PROPOSAL_QUEUE_REFS, b"g").unwrap();
        assert_eq!(left, vec![2]);
        assert!(storage.read_list::<u32>(PROPOSAL_QUEUE_REFS, b"none").unwrap().is_empty());
    }

    #[test]
    fn the_key_is_never_printed() {
        assert_eq!(format!("{:?}", SealKey::from_bytes([9; 32])), "SealKey([hidden])");
    }
}
