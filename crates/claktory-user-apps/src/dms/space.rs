//! How much of the account box's disk DMs may take (#448; Fugo's sizing,
//! narrowed for #451, when the worker stopped keeping messages: what's left
//! is the control plane, commits, a leaf's own Remove proposal and the
//! Welcomes commits make). The box's 20 GB disk is shared with the account
//! service, whose writes (sign-in, the vault's service share, check-ins)
//! fail for everyone if it fills, so:
//!
//! - **The disk guard.** Before anything new is stored (a mailbox entry, a
//!   Welcome, a KeyPackage, an agent's proposal digest, a hand-off), the free
//!   space on the worker database's filesystem is checked (read at most
//!   every `FREE_EVERY`). Below `FREE_FLOOR`, every new store is refused
//!   with `507 insufficient_storage`, except a removal (`Entry::Exempt`),
//!   which goes on down to `COMMIT_FLOOR`: a lost device mustn't stay in a
//!   group for want of room. Reads, pickups and deletes go on. The account
//!   service's own writes never come here: they're another process, with a
//!   database of its own.
//! - **Each account's room.** What the mailboxes keep (entries for 30 days,
//!   a self group's for 7, Welcomes until picked up or 30 days), at its
//!   stored size (`ENTRY_OVERHEAD` more than the MLS message), counts for
//!   whoever sent it, up to `SENT_MOST`, and for the friend it goes to, up to
//!   `RECEIVED_MOST`, of which one sender may fill `SHARE_MOST`, so neither
//!   several accounts nor one or two friends can fill one person's room and
//!   hold up the rest of their friends. Past any of them the sender is
//!   refused with `429 rate_limited`, without being told it's the friend's.
//!   A removal counts but isn't refused for it, when the worker read it as
//!   one (`groups`) and it's at most `EXEMPT_MOST` as received (anything
//!   bigger is capped like the rest), until the sender's own sum is
//!   `EXEMPT_ROOM` past its most: a bound on what any account writes, which
//!   a person's own removals never reach. A leaf keeps one own Remove
//!   waiting an epoch. When a friendship ends, what each of
//!   the two sent the other stops counting for the other at once
//!   (`release`). The counts are running sums in `dm_mailbox_space` and
//!   `dm_mailbox_shares`, kept in the same transaction as each store and each
//!   deletion, and counted again from the mailboxes at every start.
//!
//! The constants are here, in one place, for the cost document to cite.

use std::cell::RefCell;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use rusqlite::{Connection, ToSql, Transaction, params};

use super::identity::Refusal;

/// The free space under which no new DM is stored.
pub const FREE_FLOOR: u64 = 3 * 1024 * 1024 * 1024;
/// The free space under which not even a commit is stored.
pub const COMMIT_FLOOR: u64 = 1024 * 1024 * 1024;
/// How long a reading of the free space holds.
pub const FREE_EVERY: Duration = Duration::from_secs(10);
/// How often a refusal for want of space is logged at most.
const LOG_EVERY: Duration = Duration::from_secs(60);
/// The most an account's sent entries still kept may take: about 2 MB a day
/// over the 30 days they're kept, where a person's commits take about 1 MB
/// a month.
pub const SENT_MOST: i64 = 64 * 1024 * 1024;
/// The most the entries sent to an account by others may take.
pub const RECEIVED_MOST: i64 = 128 * 1024 * 1024;
/// The most of that one sender may take.
pub const SHARE_MOST: i64 = RECEIVED_MOST / 4;
/// The largest removal that's exempt from the caps, as received (its
/// envelope's base64): a real one is a few KB.
pub const EXEMPT_MOST: usize = 64 * 1024;
/// How far past `SENT_MOST` an account's removals may still take it: never
/// reached by a person's own removals, it bounds what any account can
/// write at all (a removal a new epoch lets each leaf ask for again).
pub const EXEMPT_ROOM: i64 = 16 * 1024 * 1024;
/// What an entry's row takes besides its MLS message: its ids, digest and
/// times.
pub const ENTRY_OVERHEAD: i64 = 200;

pub const SCHEMA: &str = "
-- What each account's mailbox entries and Welcomes take, as stored: those
-- it sent, and those others sent to it (`dms::space`).
CREATE TABLE IF NOT EXISTS dm_mailbox_space (
    account TEXT PRIMARY KEY,
    sent INTEGER NOT NULL DEFAULT 0,
    received INTEGER NOT NULL DEFAULT 0
) WITHOUT ROWID;
-- Of what an account received, what each sender's take.
CREATE TABLE IF NOT EXISTS dm_mailbox_shares (
    receiver TEXT NOT NULL,
    sender TEXT NOT NULL,
    bytes INTEGER NOT NULL,
    PRIMARY KEY (receiver, sender)
) WITHOUT ROWID;
";

/// The free space on the disk the worker's database is on.
pub struct Disk {
    /// The database's folder; None for a fixed reading (tests).
    path: Option<PathBuf>,
    fixed: Option<u64>,
    read: Mutex<Option<(Instant, Option<u64>)>>,
    /// When a refusal, or a failure to read the free space, was last logged.
    logged: Mutex<Option<Instant>>,
}

impl Disk {
    /// The disk the database `db` is on.
    pub fn of(db: &std::path::Path) -> Disk {
        let folder = db.parent().filter(|parent| !parent.as_os_str().is_empty()).map_or_else(|| PathBuf::from("."), |parent| parent.to_owned());
        Disk { path: Some(folder), fixed: None, read: Mutex::new(None), logged: Mutex::new(None) }
    }

    /// A disk that always has `free` bytes free: for tests.
    #[doc(hidden)]
    pub fn fixed(free: u64) -> Disk {
        Disk { path: None, fixed: Some(free), read: Mutex::new(None), logged: Mutex::new(None) }
    }

    /// The free space, read again once `FREE_EVERY` has passed; None when
    /// it can't be read.
    fn free(&self) -> Option<u64> {
        if let Some(free) = self.fixed {
            return Some(free);
        }
        let mut read = self.read.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some((at, free)) = *read
            && at.elapsed() < FREE_EVERY
        {
            return free;
        }
        let free = self.path.as_deref().and_then(available);
        *read = Some((Instant::now(), free));
        free
    }

    /// Whether to log now: at most once every `LOG_EVERY`.
    fn log_now(&self) -> bool {
        let mut logged = self.logged.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let now = logged.is_none_or(|at| at.elapsed() >= LOG_EVERY);
        if now {
            *logged = Some(Instant::now());
        }
        now
    }

    /// Refuses a new store while the disk has less than `floor` free. A
    /// disk whose free space can't be read isn't held up, but it's logged.
    fn check(&self, floor: u64) -> Result<(), Refusal> {
        let Some(free) = self.free() else {
            if self.log_now() {
                tracing::error!(path = ?self.path, "can't read the disk's free space: new DMs aren't checked against it");
            }
            return Ok(());
        };
        if free >= floor {
            return Ok(());
        }
        if self.log_now() {
            tracing::warn!(free, floor, "the disk is nearly full: new DMs are refused until there's room");
        }
        Err(Refusal(507, "insufficient_storage", "Messages can't be sent right now. Try again later.".to_owned()))
    }
}

/// The bytes free for an unprivileged user on the filesystem of `path`.
fn available(path: &std::path::Path) -> Option<u64> {
    use std::os::unix::ffi::OsStrExt;
    let c_path = std::ffi::CString::new(path.as_os_str().as_bytes()).ok()?;
    // SAFETY: statvfs fills the zeroed struct it's given, for a valid C string.
    let mut stats: libc::statvfs = unsafe { std::mem::zeroed() };
    if unsafe { libc::statvfs(c_path.as_ptr(), &raw mut stats) } != 0 {
        return None;
    }
    #[allow(clippy::useless_conversion)] // u32 or u64 depending on the platform
    let (available, block) = (u64::from(stats.f_bavail), u64::from(stats.f_frsize));
    Some(available.saturating_mul(block))
}

thread_local! {
    /// The disk of the call being answered on this thread (`with`).
    static DISK: RefCell<Option<Arc<Disk>>> = const { RefCell::new(None) };
}

/// Answers a call with `disk` as the one new DMs are checked against.
/// Without one (the modules' own tests), nothing is held up for space.
pub fn with<T>(disk: &Arc<Disk>, call: impl FnOnce() -> T) -> T {
    let before = DISK.with(|current| current.replace(Some(disk.clone())));
    let answer = call();
    DISK.with(|current| *current.borrow_mut() = before);
    answer
}

/// Refuses a new DM store while the disk is nearly full.
pub fn guard() -> Result<(), Refusal> {
    guard_at(FREE_FLOOR)
}

fn guard_at(floor: u64) -> Result<(), Refusal> {
    let disk = DISK.with(|current| current.borrow().clone());
    // Every call that stores comes through `with`; only the modules' own
    // tests call them without a disk.
    debug_assert!(cfg!(test) || disk.is_some(), "a DM store outside space::with");
    match disk {
        Some(disk) => disk.check(floor),
        None => Ok(()),
    }
}

/// What `account` has kept: (sent, received).
pub fn of(db: &Connection, account: &str) -> rusqlite::Result<(i64, i64)> {
    use rusqlite::OptionalExtension as _;
    Ok(db
        .query_row("SELECT sent, received FROM dm_mailbox_space WHERE account = ?1", [account], |row| Ok((row.get(0)?, row.get(1)?)))
        .optional()?
        .unwrap_or((0, 0)))
}

/// What a mailbox entry is, for the room it may take.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Entry {
    /// A removal, as the worker read it (`groups`: a leaf's own Remove, a
    /// commit of only Removes the rules allow, a leaf's move to a new
    /// security key), at most `EXEMPT_MOST` as received: counted, but never
    /// refused for room, and stored down to `COMMIT_FLOOR`.
    Exempt,
    /// Anything else.
    Capped,
}

impl Entry {
    /// What an entry is, `received` bytes as it came (its envelope's
    /// base64): exempt only when the worker read it as a removal and it's
    /// no bigger than one is.
    pub fn of(removal: bool, received: usize) -> Entry {
        if removal && received <= EXEMPT_MOST { Entry::Exempt } else { Entry::Capped }
    }
}

/// Takes room for a new entry of `bytes` (as stored) from `sender` to
/// `receiver` (another account, if any), in the transaction that stores it:
/// refused while the disk is nearly full, or (except an exempt one) past the
/// sender's most, the receiver's, or the sender's share of the receiver's;
/// an exempt one only `EXEMPT_ROOM` past the sender's most.
pub fn take(transaction: &Transaction<'_>, sender: &str, receiver: Option<&str>, bytes: i64, entry: Entry) -> Result<(), Refusal> {
    guard_at(if entry == Entry::Exempt { COMMIT_FLOOR } else { FREE_FLOOR })?;
    let receiver = receiver.filter(|receiver| *receiver != sender);
    let most = if entry == Entry::Capped { SENT_MOST } else { SENT_MOST + EXEMPT_ROOM };
    if of(transaction, sender)?.0 + bytes > most {
        return Err(Refusal(
            429,
            "rate_limited",
            "You've sent a lot of messages recently. Older ones need to expire before you can send more.".to_owned(),
        ));
    }
    if entry == Entry::Capped
        && let Some(receiver) = receiver
        && (of(transaction, receiver)?.1 + bytes > RECEIVED_MOST || share(transaction, receiver, sender)? + bytes > SHARE_MOST)
    {
        return Err(Refusal(429, "rate_limited", "This message can't be delivered right now. Try again later.".to_owned()));
    }
    transaction.execute(
        "INSERT INTO dm_mailbox_space (account, sent) VALUES (?1, ?2) ON CONFLICT (account) DO UPDATE SET sent = sent + ?2",
        params![sender, bytes],
    )?;
    if let Some(receiver) = receiver {
        transaction.execute(
            "INSERT INTO dm_mailbox_space (account, received) VALUES (?1, ?2) ON CONFLICT (account) DO UPDATE SET received = received + ?2",
            params![receiver, bytes],
        )?;
        transaction.execute(
            "INSERT INTO dm_mailbox_shares (receiver, sender, bytes) VALUES (?1, ?2, ?3)
             ON CONFLICT (receiver, sender) DO UPDATE SET bytes = bytes + ?3",
            params![receiver, sender, bytes],
        )?;
    }
    Ok(())
}

/// What of `receiver`'s room `sender`'s entries take.
pub fn share(db: &Connection, receiver: &str, sender: &str) -> rusqlite::Result<i64> {
    use rusqlite::OptionalExtension as _;
    Ok(db
        .query_row("SELECT bytes FROM dm_mailbox_shares WHERE receiver = ?1 AND sender = ?2", [receiver, sender], |row| row.get(0))
        .optional()?
        .unwrap_or(0))
}

/// Gives back the room of the mailbox entries `which` picks (a condition on
/// `dm_mailbox`), before they're deleted in the same transaction.
pub fn free_entries(transaction: &Transaction<'_>, which: &str, values: &[&dyn ToSql]) -> rusqlite::Result<()> {
    free(transaction, ENTRIES, which, values, true)
}

/// Gives back the room of the Welcomes `which` picks (a condition on
/// `dm_welcomes`), before they're deleted in the same transaction.
pub fn free_welcomes(transaction: &Transaction<'_>, which: &str, values: &[&dyn ToSql]) -> rusqlite::Result<()> {
    free(transaction, WELCOMES, which, values, true)
}

/// Where a table keeps what's counted: its table, sender, receiver and
/// bytes, as SQL.
struct Kept {
    table: &'static str,
    sender: &'static str,
    receiver: &'static str,
    data: &'static str,
}

const ENTRIES: Kept = Kept { table: "dm_mailbox", sender: "account_id", receiver: "receiver", data: "envelope" };
const WELCOMES: Kept = Kept { table: "dm_welcomes", sender: "sender", receiver: "NULLIF(account_id, sender)", data: "welcome" };

/// Takes what `which` picks of `kept` off the receivers' sums and the
/// senders' shares of them, and (`sent`) off the senders' sums. Rows left
/// at nothing go, so no account stays named for nothing.
fn free(transaction: &Transaction<'_>, kept: Kept, which: &str, values: &[&dyn ToSql], sent: bool) -> rusqlite::Result<()> {
    let Kept { table, sender, receiver, data } = kept;
    // `who`: the accounts (one or two columns) the bytes are summed by.
    let freed = |who: &str, columns: &str, also: &str| {
        format!(
            "(SELECT {who}, SUM(length({data}) + {ENTRY_OVERHEAD}) AS bytes FROM {table}
              WHERE ({which}) AND {sender} IS NOT NULL{also} GROUP BY {columns}) AS freed"
        )
    };
    let received = format!(" AND {receiver} IS NOT NULL");
    if sent {
        transaction.execute(
            &format!(
                "UPDATE dm_mailbox_space SET sent = MAX(sent - freed.bytes, 0) FROM {}
                 WHERE dm_mailbox_space.account = freed.account",
                freed(&format!("{sender} AS account"), "1", "")
            ),
            values,
        )?;
    }
    transaction.execute(
        &format!(
            "UPDATE dm_mailbox_space SET received = MAX(received - freed.bytes, 0) FROM {}
             WHERE dm_mailbox_space.account = freed.account",
            freed(&format!("{receiver} AS account"), "1", &received)
        ),
        values,
    )?;
    transaction.execute(
        &format!(
            "UPDATE dm_mailbox_shares SET bytes = MAX(dm_mailbox_shares.bytes - freed.bytes, 0) FROM {}
             WHERE dm_mailbox_shares.receiver = freed.receiver AND dm_mailbox_shares.sender = freed.sender",
            freed(&format!("{receiver} AS receiver, {sender} AS sender"), "1, 2", &received)
        ),
        values,
    )?;
    transaction.execute_batch(
        "DELETE FROM dm_mailbox_shares WHERE bytes <= 0;
         DELETE FROM dm_mailbox_space WHERE sent <= 0 AND received <= 0;",
    )
}

/// A friendship's end (removed, blocked, an account deleted): what each of
/// the two sent the other in their conversation stops counting for the
/// other at once (Fugo), while the entries stay for the drain until their
/// time. Welcomes between them in it, which no one can use now, go.
pub fn release(transaction: &Transaction<'_>, conversation: &str) -> rusqlite::Result<()> {
    let groups = "group_id IN (SELECT group_id FROM dm_groups WHERE conversation_id = ?1 AND kind = 'conversation')";
    free(transaction, ENTRIES, &format!("{groups} AND receiver IS NOT NULL"), &[&conversation], false)?;
    transaction.execute(&format!("UPDATE dm_mailbox SET receiver = NULL WHERE {groups} AND receiver IS NOT NULL"), [conversation])?;
    let welcomes = format!("{groups} AND sender IS NOT NULL AND sender != account_id");
    free(transaction, WELCOMES, &welcomes, &[&conversation], true)?;
    transaction.execute(&format!("DELETE FROM dm_welcomes WHERE {welcomes}"), [conversation])?;
    Ok(())
}

/// Deleting an account: its sums and shares go, whatever's left of them.
pub fn purge(transaction: &Transaction<'_>, account: &str) -> rusqlite::Result<()> {
    transaction.execute("DELETE FROM dm_mailbox_space WHERE account = ?1", [account])?;
    transaction.execute("DELETE FROM dm_mailbox_shares WHERE receiver = ?1 OR sender = ?1", [account])?;
    Ok(())
}

/// Counts what every account's mailbox entries and Welcomes take again,
/// from the mailboxes: at every start, so nothing a crash or an older
/// worker left uncounted lasts. Only accounts still here count: a deleted
/// one's commits stay in its friends' conversations for their drain, but
/// no sum names it again.
pub fn recount(db: &Connection) -> rusqlite::Result<()> {
    db.execute_batch(&format!(
        "BEGIN IMMEDIATE;
         DELETE FROM dm_mailbox_space;
         INSERT INTO dm_mailbox_space (account, sent, received)
         SELECT account, SUM(sent), SUM(received) FROM (
             SELECT account_id AS account, length(envelope) + {ENTRY_OVERHEAD} AS sent, 0 AS received FROM dm_mailbox
             UNION ALL SELECT receiver, 0, length(envelope) + {ENTRY_OVERHEAD} FROM dm_mailbox WHERE receiver IS NOT NULL
             UNION ALL SELECT sender, length(welcome) + {ENTRY_OVERHEAD}, 0 FROM dm_welcomes WHERE sender IS NOT NULL
             UNION ALL SELECT account_id, 0, length(welcome) + {ENTRY_OVERHEAD} FROM dm_welcomes
                 WHERE sender IS NOT NULL AND sender != account_id
         ) WHERE account IN (SELECT account_id FROM dm_identities) GROUP BY account;
         DELETE FROM dm_mailbox_shares;
         INSERT INTO dm_mailbox_shares (receiver, sender, bytes)
         SELECT receiver, sender, SUM(bytes) FROM (
             SELECT receiver, account_id AS sender, length(envelope) + {ENTRY_OVERHEAD} AS bytes FROM dm_mailbox
                 WHERE receiver IS NOT NULL AND receiver != account_id
             UNION ALL SELECT account_id, sender, length(welcome) + {ENTRY_OVERHEAD} FROM dm_welcomes
                 WHERE sender IS NOT NULL AND sender != account_id
         ) WHERE receiver IN (SELECT account_id FROM dm_identities) AND sender IN (SELECT account_id FROM dm_identities)
         GROUP BY receiver, sender;
         COMMIT;"
    ))
}

/// Gives a database from before this was kept the columns it needs: who
/// a mailbox entry went to, and whose commit made a Welcome, filled in
/// from what's there.
pub fn upgrade(db: &Connection) -> rusqlite::Result<()> {
    let has = |table: &str, column: &str| -> rusqlite::Result<bool> {
        db.query_row(&format!("SELECT COUNT(*) > 0 FROM pragma_table_info('{table}') WHERE name = ?1"), [column], |row| row.get(0))
    };
    if !has("dm_mailbox", "receiver")? {
        db.execute_batch(
            "ALTER TABLE dm_mailbox ADD COLUMN receiver TEXT;
             UPDATE dm_mailbox SET receiver = (
                 SELECT CASE WHEN c.low = dm_mailbox.account_id THEN c.high WHEN c.high = dm_mailbox.account_id THEN c.low END
                 FROM dm_groups g JOIN dm_contacts c ON c.conversation_id = g.conversation_id
                 WHERE g.group_id = dm_mailbox.group_id AND g.kind = 'conversation');",
        )?;
    }
    if !has("dm_welcomes", "sender")? {
        db.execute_batch(
            "ALTER TABLE dm_welcomes ADD COLUMN sender TEXT;
             UPDATE dm_welcomes SET sender = (
                 SELECT m.account_id FROM dm_mailbox m WHERE m.group_id = dm_welcomes.group_id AND m.seq = dm_welcomes.seq);",
        )?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A database from before this was kept: its mailbox entries and
    /// Welcomes get who they went to and came from, and are counted.
    #[test]
    fn an_older_database_is_counted_from_its_mailboxes() {
        let db = Connection::open_in_memory().expect("db");
        let older = super::super::groups::SCHEMA.replace("    receiver TEXT,\n", "").replace(",\n    sender TEXT", "");
        assert!(!older.contains("receiver TEXT") && !older.contains("sender TEXT"), "the schema before");
        db.execute_batch(super::super::contacts::SCHEMA).expect("contacts");
        db.execute_batch(&older).expect("the older tables");
        db.execute_batch(SCHEMA).expect("space");
        db.execute_batch(super::super::leaves::SCHEMA).expect("leaves");
        // Ana and Bo are here; Cy's account was deleted, its commit left
        // for their drain.
        db.execute_batch(
            "INSERT INTO dm_identities (account_id, identity_key, device_list, version, updated)
                 VALUES ('ana', x'00', x'00', 1, 0), ('bo', x'00', x'00', 1, 0);",
        )
        .expect("identities");
        db.execute_batch(
            "INSERT INTO dm_contacts (id, low, high, conversation_id, generation, request_id, created) VALUES ('c', 'ana', 'bo', 'talk', 1, 'r', 0);
             INSERT INTO dm_groups (group_id, kind, subject, account_id, conversation_id, contact_generation, epoch, last_seq, group_info, created)
                 VALUES (x'01', 'conversation', 's', 'ana', 'talk', 1, 1, 2, x'00', 0);
             INSERT INTO dm_groups (group_id, kind, subject, account_id, epoch, last_seq, group_info, created)
                 VALUES (x'02', 'self', 't', 'ana', 1, 1, x'00', 0);
             INSERT INTO dm_mailbox (group_id, seq, kind, epoch, envelope, digest, account_id, leaf_id, accepted)
                 VALUES (x'01', 1, 'commit', 0, zeroblob(100), x'00', 'ana', 'ana-phone', 0),
                        (x'01', 2, 'application', 1, zeroblob(10), x'00', 'bo', 'bo-phone', 0),
                        (x'02', 1, 'application', 1, zeroblob(1000), x'00', 'ana', 'ana-phone', 0),
                        (x'01', 3, 'commit', 2, zeroblob(70), x'00', 'cy', 'cy-phone', 0);
             INSERT INTO dm_welcomes (id, group_id, account_id, leaf_id, welcome, seq, accepted)
                 VALUES ('w', x'01', 'bo', 'bo-phone', zeroblob(50), 1, 0);",
        )
        .expect("rows");
        upgrade(&db).expect("upgraded");
        upgrade(&db).expect("again, nothing to do");
        recount(&db).expect("counted");
        let ana = (100 + ENTRY_OVERHEAD) + (1000 + ENTRY_OVERHEAD) + (50 + ENTRY_OVERHEAD);
        assert_eq!(of(&db, "ana").expect("ana"), (ana, 10 + ENTRY_OVERHEAD));
        assert_eq!(of(&db, "bo").expect("bo"), (10 + ENTRY_OVERHEAD, (100 + ENTRY_OVERHEAD) + (50 + ENTRY_OVERHEAD)));
        assert_eq!((share(&db, "bo", "ana").expect("share"), share(&db, "ana", "bo").expect("share")), (150 + 2 * ENTRY_OVERHEAD, 10 + ENTRY_OVERHEAD));
        let named: i64 = db
            .query_row(
                "SELECT (SELECT COUNT(*) FROM dm_mailbox_space WHERE account = 'cy')
                      + (SELECT COUNT(*) FROM dm_mailbox_shares WHERE 'cy' IN (receiver, sender))",
                [],
                |row| row.get(0),
            )
            .expect("count");
        assert_eq!(named, 0, "a deleted account's sums aren't counted back");
    }
}
