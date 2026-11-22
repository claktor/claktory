//! A person's own DM identity (D1a): their profile (a name they choose and a
//! generated avatar), their privacy (whether anyone may send them a friend
//! request) and their secret friend ID, which only they see and can reset.
//! Only the person's own signed-in devices reach these: no backend, agent or
//! anyone else, and nothing here looks anyone else up.
//!
//! A friend ID is 20 random bytes (160 bits), shown as `cf1_` and 32
//! base32 letters. It's kept as a keyed digest, to find its owner when a
//! request names it (D1b), and as the owner's own sealed copy, to show it
//! again on any of their devices. A reset makes a new one (a new
//! generation) and retires the old for good: its digest stays, owned by no
//! one who could be found through it again, and closes the requests still
//! waiting through it.

use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::keys::{Field, Keys, Refused, account_purpose};
use crate::ipc::Caller;

pub const SCHEMA: &str = "
-- A person's DM identity, one per account. `profile` is sealed:
-- {\"name\", \"avatar_seed\"}.
CREATE TABLE IF NOT EXISTS dm_people (
    account_id TEXT PRIMARY KEY,
    profile BLOB NOT NULL,
    profile_revision INTEGER NOT NULL,
    requests_on INTEGER NOT NULL,
    privacy_revision INTEGER NOT NULL,
    created INTEGER NOT NULL
);
-- Friend IDs, by their keyed digest. One active (retired NULL) per
-- account; retired ones stay as tombstones, their reveal gone. An account
-- purged leaves its tombstones owned by no one.
CREATE TABLE IF NOT EXISTS dm_friend_codes (
    digest BLOB PRIMARY KEY,
    account_id TEXT NOT NULL,
    generation INTEGER NOT NULL,
    reveal BLOB,
    created INTEGER NOT NULL,
    retired INTEGER
);
CREATE UNIQUE INDEX IF NOT EXISTS dm_friend_codes_active ON dm_friend_codes (account_id) WHERE retired IS NULL;
-- Counts for limits: uses of `key` in the window starting then.
CREATE TABLE IF NOT EXISTS dm_limits (
    key TEXT NOT NULL,
    window_start INTEGER NOT NULL,
    count INTEGER NOT NULL,
    PRIMARY KEY (key, window_start)
);
";

/// A friend ID's format, and its payload's length in base32 letters.
const PREFIX: &str = "cf1_";
const CODE_BYTES: usize = 20;
const CODE_LETTERS: usize = 32;
pub(super) const BASE32: &[u8; 32] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";
/// What friend ID digests are keyed with: a data key of its own.
pub(super) const LOOKUP: &str = "lookup:friend-codes";
/// A name's length, in characters.
const MAX_NAME: usize = 80;

/// A friend ID as shown: `cf1_` and 32 base32 letters.
pub fn format_code(bytes: &[u8; CODE_BYTES]) -> String {
    let mut text = String::with_capacity(PREFIX.len() + CODE_LETTERS);
    text.push_str(PREFIX);
    let (mut held, mut bits) = (0u32, 0u32);
    for byte in bytes {
        held = (held << 8) | u32::from(*byte);
        bits += 8;
        while bits >= 5 {
            bits -= 5;
            text.push(BASE32[((held >> bits) & 31) as usize] as char);
        }
    }
    text
}

/// A friend ID as typed or pasted: the `cf1_` prefix (any case) is
/// optional, and in the 32 letters case, spaces and hyphens don't matter.
/// Anything else, or another length, isn't one.
pub fn parse_code(text: &str) -> Option<[u8; CODE_BYTES]> {
    let text = text.trim();
    let payload = match text.get(..PREFIX.len()) {
        Some(prefix) if prefix.eq_ignore_ascii_case(PREFIX) => &text[PREFIX.len()..],
        _ => text,
    };
    let mut out = [0u8; CODE_BYTES];
    let (mut held, mut bits, mut at, mut letters) = (0u32, 0u32, 0usize, 0usize);
    for c in payload.chars().filter(|c| !matches!(c, ' ' | '-')) {
        let value = BASE32.iter().position(|&b| b as char == c.to_ascii_uppercase())? as u32;
        letters += 1;
        if letters > CODE_LETTERS {
            return None;
        }
        held = (held << 5) | value;
        bits += 5;
        if bits >= 8 {
            bits -= 8;
            out[at] = (held >> bits) as u8;
            at += 1;
        }
    }
    (letters == CODE_LETTERS).then_some(out)
}

/// The keyed digest a friend ID is found by.
pub fn code_digest(keys: &Keys, db: &Connection, code: &[u8; CODE_BYTES], now_ms: i64) -> Result<Vec<u8>, Refused> {
    use hmac::{Hmac, KeyInit, Mac};
    let key = keys.hash_key(db, LOOKUP, now_ms)?;
    let mut mac = <Hmac<sha2::Sha256> as KeyInit>::new_from_slice(key.as_slice()).expect("any key length");
    mac.update(b"claktory-dms friend code v1\0");
    mac.update(code);
    Ok(mac.finalize().into_bytes().to_vec())
}

pub(super) fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

pub(super) fn random<const N: usize>() -> [u8; N] {
    let mut bytes = [0u8; N];
    getrandom::fill(&mut bytes).expect("the system's random source");
    bytes
}

/// A name as kept: trimmed, runs of spaces as one, at most 80 characters,
/// with no control, direction-changing or invisible characters, which could
/// make it read as something else. None if that leaves nothing, or it's
/// longer.
pub fn clean_name(name: &str) -> Option<String> {
    let hidden = |c: char| {
        c.is_control() || matches!(c, '\u{200B}'..='\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2060}'..='\u{2069}' | '\u{061C}' | '\u{FEFF}')
    };
    if name.chars().any(hidden) {
        return None;
    }
    let clean = name.split_whitespace().collect::<Vec<_>>().join(" ");
    (!clean.is_empty() && clean.chars().count() <= MAX_NAME).then_some(clean)
}

/// Whether a name says Claktory, even spaced out or with letters that read
/// alike ("C1aktory Team", "claktory_support", "Сlaktory" in Cyrillic):
/// people would take it for Claktory itself, so only Claktory says it.
pub fn says_claktory(name: &str) -> bool {
    let folded: String = name
        .chars()
        .filter(|c| !c.is_whitespace() && !matches!(c, '-' | '.' | '_' | '·' | '•'))
        .flat_map(char::to_lowercase)
        .map(|c| match c {
            // Fullwidth forms read as their ASCII letters.
            '\u{FF01}'..='\u{FF5E}' => char::from_u32(c as u32 - 0xFEE0).unwrap_or(c).to_ascii_lowercase(),
            _ => c,
        })
        .map(|c| match c {
            'i' | '1' | '|' | '!' | 'ı' | 'ł' | 'ℓ' | 'ι' | 'і' | 'ӏ' => 'l',
            '0' | 'ο' | 'о' | 'ө' | 'ø' => 'o',
            'с' | 'ϲ' | 'ç' => 'c',
            'а' | 'α' | '@' | 'à' | 'á' | 'â' | 'ä' | 'å' | 'ã' => 'a',
            'к' | 'κ' => 'k',
            'т' | 'τ' | '7' | '+' => 't',
            'г' | 'ř' => 'r',
            'у' | 'γ' | 'ý' | 'ÿ' => 'y',
            _ => c,
        })
        .collect();
    folded.contains("claktory")
}

/// The profile, as sealed.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Profile {
    name: String,
    avatar_seed: String,
}

/// Why a route refuses: its status, code and words.
pub struct Refusal(pub u16, pub &'static str, pub String);

impl From<Refused> for Refusal {
    fn from(refused: Refused) -> Refusal {
        tracing::error!("dms: {refused}");
        Refusal(503, "unavailable", "Something went wrong. Try again later.".to_owned())
    }
}

impl From<rusqlite::Error> for Refusal {
    fn from(error: rusqlite::Error) -> Refusal {
        Refusal::from(Refused::from(error))
    }
}

pub(super) fn invalid(message: &str) -> Refusal {
    Refusal(400, "invalid_request", message.to_owned())
}

fn stale() -> Refusal {
    Refusal(409, "stale", "This changed on another device. Look again, then try again.".to_owned())
}

/// When to try again, `seconds` from now, in words, never sooner than it
/// is: "in a minute", "in 5 minutes", "in an hour", "in 20 hours".
pub(super) fn later(seconds: i64) -> String {
    let minutes = ((seconds + 59) / 60).max(1);
    let hours = (minutes + 59) / 60;
    match (minutes, hours) {
        (1, _) => "in a minute".to_owned(),
        (2..=59, _) => format!("in {minutes} minutes"),
        (_, 1) => "in an hour".to_owned(),
        _ => format!("in {hours} hours"),
    }
}

/// Counts one use of `key` in a window of `span` seconds, refusing past
/// `max` with the seconds to wait.
pub(super) fn take(transaction: &Transaction<'_>, key: &str, span: i64, max: i64, now_ms: i64) -> Result<(), Refusal> {
    take_as(transaction, key, span, max, now_ms, "Too many tries.")
}

/// [`take`], refusing with `what` ("Too many messages.") first.
pub(super) fn take_as(transaction: &Transaction<'_>, key: &str, span: i64, max: i64, now_ms: i64, what: &str) -> Result<(), Refusal> {
    let now = now_ms / 1000;
    let start = now - now.rem_euclid(span);
    let used: i64 = transaction
        .query_row("SELECT count FROM dm_limits WHERE key = ?1 AND window_start = ?2", params![key, start], |row| row.get(0))
        .optional()?
        .unwrap_or(0);
    if used >= max {
        return Err(Refusal(429, "rate_limited", format!("{what} Try again {}.", later(start + span - now))));
    }
    transaction.execute(
        "INSERT INTO dm_limits (key, window_start, count) VALUES (?1, ?2, 1)
         ON CONFLICT (key, window_start) DO UPDATE SET count = count + 1",
        params![key, start],
    )?;
    transaction.execute("DELETE FROM dm_limits WHERE key = ?1 AND window_start < ?2", params![key, start])?;
    Ok(())
}

/// Makes a new friend ID for `account`, as generation `generation`: its
/// code.
fn new_code(transaction: &Transaction<'_>, keys: &Keys, account: &str, generation: i64, now_ms: i64) -> Result<String, Refusal> {
    let bytes = random::<CODE_BYTES>();
    let code = format_code(&bytes);
    let digest = code_digest(keys, transaction, &bytes, now_ms)?;
    let field = Field { table: "dm_friend_codes", record: &hex(&digest), column: "reveal" };
    let reveal = keys.seal(transaction, &account_purpose(account), field, code.as_bytes(), now_ms)?;
    transaction.execute(
        "INSERT INTO dm_friend_codes (digest, account_id, generation, reveal, created) VALUES (?1, ?2, ?3, ?4, ?5)",
        params![digest, account, generation, reveal, now_ms],
    )?;
    Ok(code)
}

/// The person's row, made the first time with an empty name, a generated
/// avatar, requests on and a friend ID.
pub(super) fn ensure(transaction: &Transaction<'_>, keys: &Keys, account: &str, now_ms: i64) -> Result<(), Refusal> {
    let exists: bool = transaction.query_row("SELECT COUNT(*) > 0 FROM dm_people WHERE account_id = ?1", [account], |row| row.get(0))?;
    if exists {
        return Ok(());
    }
    let profile = Profile { name: String::new(), avatar_seed: hex(&random::<16>()) };
    let sealed = seal_profile(transaction, keys, account, &profile, now_ms)?;
    transaction.execute(
        "INSERT INTO dm_people (account_id, profile, profile_revision, requests_on, privacy_revision, created) VALUES (?1, ?2, 1, 1, 1, ?3)",
        params![account, sealed, now_ms],
    )?;
    new_code(transaction, keys, account, 1, now_ms)?;
    Ok(())
}

fn seal_profile(db: &Connection, keys: &Keys, account: &str, profile: &Profile, now_ms: i64) -> Result<Vec<u8>, Refused> {
    let plain = zeroize::Zeroizing::new(serde_json::to_vec(profile).unwrap_or_default());
    keys.seal(db, &account_purpose(account), Field { table: "dm_people", record: account, column: "profile" }, &plain, now_ms)
}

/// `account`'s name and avatar, as their requests and contacts show them;
/// None once they're gone.
pub(super) fn shown_profile(db: &Connection, keys: &Keys, account: &str) -> Result<Option<(String, String)>, Refused> {
    let sealed: Option<Vec<u8>> =
        db.query_row("SELECT profile FROM dm_people WHERE account_id = ?1", [account], |row| row.get(0)).optional()?;
    let Some(sealed) = sealed else { return Ok(None) };
    let opened = keys.open(db, Field { table: "dm_people", record: account, column: "profile" }, &sealed)?;
    let profile: Profile = serde_json::from_slice(opened.bytes()).map_err(|_| Refused::DoesntOpen)?;
    Ok(Some((profile.name, profile.avatar_seed)))
}

/// The person's identity as their devices see it (never the code itself).
fn me(transaction: &Transaction<'_>, keys: &Keys, account: &str) -> Result<Value, Refusal> {
    let (sealed, profile_revision, requests_on, privacy_revision): (Vec<u8>, i64, bool, i64) = transaction.query_row(
        "SELECT profile, profile_revision, requests_on, privacy_revision FROM dm_people WHERE account_id = ?1",
        [account],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
    )?;
    let opened = keys.open(transaction, Field { table: "dm_people", record: account, column: "profile" }, &sealed)?;
    let profile: Profile = serde_json::from_slice(opened.bytes()).map_err(|_| Refused::DoesntOpen)?;
    let (generation, created): (i64, i64) = transaction.query_row(
        "SELECT generation, created FROM dm_friend_codes WHERE account_id = ?1 AND retired IS NULL",
        [account],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    Ok(json!({
        "profile": {"name": profile.name, "avatar_seed": profile.avatar_seed, "revision": profile_revision},
        "privacy": {"requests_on": requests_on, "revision": privacy_revision},
        "friend_id": {"generation": generation, "created_ms": created},
    }))
}

/// The active friend ID's code and generation.
fn reveal(transaction: &Transaction<'_>, keys: &Keys, account: &str) -> Result<Value, Refusal> {
    let (digest, generation, sealed): (Vec<u8>, i64, Vec<u8>) = transaction.query_row(
        "SELECT digest, generation, reveal FROM dm_friend_codes WHERE account_id = ?1 AND retired IS NULL",
        [account],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
    )?;
    let opened = keys.open(transaction, Field { table: "dm_friend_codes", record: &hex(&digest), column: "reveal" }, &sealed)?;
    let code = String::from_utf8(opened.bytes().to_vec()).map_err(|_| Refused::DoesntOpen)?;
    Ok(json!({"code": code, "generation": generation}))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Nonce {
    #[serde(default)]
    #[allow(dead_code)]
    request_nonce: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Reset {
    /// The generation the device saw: another device's reset since is 409.
    generation: i64,
    #[serde(default)]
    #[allow(dead_code)]
    request_nonce: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SetProfile {
    /// Left out: unchanged.
    #[serde(default)]
    name: Option<String>,
    /// A new generated avatar.
    #[serde(default)]
    new_avatar: bool,
    revision: i64,
    #[serde(default)]
    #[allow(dead_code)]
    request_nonce: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SetPrivacy {
    requests_on: bool,
    revision: i64,
    #[serde(default)]
    #[allow(dead_code)]
    request_nonce: Option<String>,
}

pub(super) fn body<'a, T: Deserialize<'a>>(body: &'a [u8]) -> Result<T, Refusal> {
    let body = if body.is_empty() { b"{}".as_slice() } else { body };
    serde_json::from_slice(body).map_err(|_| invalid("The request isn't what this endpoint takes."))
}

/// Answers `/v1/apps/dms/<route>`: the person's own identity, their
/// requests, friends and blocks (`contacts`), their conversations
/// (`messages`) and their reports (`reports`), only ever from their own
/// devices. None for a route that isn't one of these.
pub fn route(
    db: &mut Connection,
    keys: &Keys,
    method: &str,
    route: &str,
    caller: &Caller,
    bytes: &[u8],
    now_ms: i64,
) -> Option<(u16, Value)> {
    let (route, query) = route.split_once('?').unwrap_or((route, ""));
    let known = matches!(
        (method, route),
        ("GET", "me") | ("POST", "me/friend-id/reveal") | ("POST", "me/friend-id/reset") | ("PUT", "me/profile") | ("PUT", "me/privacy")
    );
    let contacts = super::contacts::knows(method, route);
    let reports = super::reports::knows(method, route);
    let messages = super::messages::knows(method, route);
    let agents = super::agents::knows(method, route);
    let review = super::review::knows(method, route);
    let leaves = super::leaves::knows(method, route);
    let groups = super::groups::knows(method, route);
    let handoffs = super::handoffs::knows(method, route);
    if !known && !contacts && !reports && !messages && !agents && !review && !leaves && !groups && !handoffs {
        return None;
    }
    let answer = match caller {
        _ if agents => super::agents::answer(db, keys, method, (route, query), caller, bytes, now_ms),
        _ if review => super::review::answer(db, method, (route, query), caller, bytes, now_ms),
        _ if leaves => super::leaves::answer(db, method, route, caller, bytes, now_ms),
        _ if groups => super::groups::answer(db, keys, method, (route, query), caller, bytes, now_ms),
        _ if handoffs => super::handoffs::answer(db, method, route, caller, bytes, now_ms),
        Caller::Device { account_id, .. } if contacts => super::contacts::answer(db, keys, method, route, account_id, bytes, now_ms),
        Caller::Device { account_id, .. } if reports => super::reports::answer(db, keys, method, account_id, bytes, now_ms),
        Caller::Device { .. } if messages => super::messages::answer(db, keys, method, (route, query), caller, bytes, now_ms),
        Caller::Device { account_id, .. } => answer(db, keys, method, route, account_id, bytes, now_ms),
        _ => Err(Refusal(403, "device_only", "Only your own signed-in devices can do this.".to_owned())),
    };
    Some(answer.unwrap_or_else(|Refusal(status, code, message)| (status, json!({"error": code, "message": message}))))
}

fn answer(
    db: &mut Connection,
    keys: &Keys,
    method: &str,
    route: &str,
    account: &str,
    bytes: &[u8],
    now_ms: i64,
) -> Result<(u16, Value), Refusal> {
    let transaction = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
    ensure(&transaction, keys, account, now_ms)?;
    let answer = match (method, route) {
        ("GET", "me") => me(&transaction, keys, account)?,
        ("POST", "me/friend-id/reveal") => {
            body::<Nonce>(bytes)?;
            take(&transaction, &format!("reveal:{account}"), 3600, 30, now_ms)?;
            reveal(&transaction, keys, account)?
        }
        ("POST", "me/friend-id/reset") => {
            let reset: Reset = body(bytes)?;
            let current: i64 = transaction.query_row(
                "SELECT generation FROM dm_friend_codes WHERE account_id = ?1 AND retired IS NULL",
                [account],
                |row| row.get(0),
            )?;
            if reset.generation != current {
                return Err(stale());
            }
            take(&transaction, &format!("reset:{account}"), 86_400, 5, now_ms)?;
            // The old one is retired for good: its digest stays, its reveal
            // goes.
            transaction.execute(
                "UPDATE dm_friend_codes SET retired = ?2, reveal = NULL WHERE account_id = ?1 AND retired IS NULL",
                params![account, now_ms],
            )?;
            // Requests still pending through it can't be accepted any more.
            super::contacts::close_incoming(&transaction, account)?;
            let code = new_code(&transaction, keys, account, current + 1, now_ms)?;
            json!({"code": code, "generation": current + 1})
        }
        ("PUT", "me/profile") => {
            let set: SetProfile = body(bytes)?;
            let (sealed, revision): (Vec<u8>, i64) =
                transaction.query_row("SELECT profile, profile_revision FROM dm_people WHERE account_id = ?1", [account], |row| {
                    Ok((row.get(0)?, row.get(1)?))
                })?;
            if set.revision != revision {
                return Err(stale());
            }
            take(&transaction, &format!("settings:{account}"), 3600, 60, now_ms)?;
            let opened = keys.open(&transaction, Field { table: "dm_people", record: account, column: "profile" }, &sealed)?;
            let mut profile: Profile = serde_json::from_slice(opened.bytes()).map_err(|_| Refused::DoesntOpen)?;
            if let Some(name) = &set.name {
                profile.name =
                    clean_name(name).ok_or_else(|| invalid("A name has 1 to 80 characters, and no hidden or direction-changing ones."))?;
                if says_claktory(&profile.name) {
                    return Err(invalid("A name can't say Claktory: only Claktory itself does."));
                }
            }
            if set.new_avatar {
                profile.avatar_seed = hex(&random::<16>());
            }
            let sealed = seal_profile(&transaction, keys, account, &profile, now_ms)?;
            transaction.execute(
                "UPDATE dm_people SET profile = ?2, profile_revision = profile_revision + 1 WHERE account_id = ?1",
                params![account, sealed],
            )?;
            me(&transaction, keys, account)?
        }
        ("PUT", "me/privacy") => {
            let set: SetPrivacy = body(bytes)?;
            let revision: i64 =
                transaction.query_row("SELECT privacy_revision FROM dm_people WHERE account_id = ?1", [account], |row| row.get(0))?;
            if set.revision != revision {
                return Err(stale());
            }
            take(&transaction, &format!("settings:{account}"), 3600, 60, now_ms)?;
            transaction.execute(
                "UPDATE dm_people SET requests_on = ?2, privacy_revision = privacy_revision + 1 WHERE account_id = ?1",
                params![account, set.requests_on],
            )?;
            if !set.requests_on {
                // Off closes what's waiting too; their own requests out can
                // still be accepted, since they asked.
                super::contacts::close_incoming(&transaction, account)?;
            }
            me(&transaction, keys, account)?
        }
        _ => return Err(Refusal(404, "not_found", "There's nothing here.".to_owned())),
    };
    // The person's other devices hear of it; a reset or requests turned off
    // closes what was waiting too.
    if route != "me/friend-id/reveal" && method != "GET" {
        super::sync::emit(&transaction, account, super::sync::Kind::Me, "", "", 0, now_ms)?;
        if route != "me/profile" {
            super::sync::emit(&transaction, account, super::sync::Kind::Requests, "", "", 0, now_ms)?;
        }
    }
    transaction.commit()?;
    Ok((200, answer))
}

/// Forgets an account's identity (it was deleted): its row goes, and its
/// friend IDs are retired, owned by no one, so none is ever found again.
pub fn purge(db: &mut Connection, keys: &Keys, account: &str, now_ms: i64) -> Result<(), Refused> {
    let transaction = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
    super::contacts::purge(&transaction, keys, account, now_ms)?;
    super::reports::purge(&transaction, account)?;
    super::messages::purge(&transaction, account)?;
    super::review::purge(&transaction, account)?;
    transaction.execute("DELETE FROM dm_people WHERE account_id = ?1", [account])?;
    super::sync::purge(&transaction, account)?;
    super::agents::purge(&transaction, account)?;
    super::leaves::purge(&transaction, account)?;
    super::groups::purge(&transaction, account)?;
    super::handoffs::purge(&transaction, account)?;
    transaction.execute(
        "UPDATE dm_friend_codes SET retired = COALESCE(retired, ?2), reveal = NULL, account_id = '' WHERE account_id = ?1",
        params![account, now_ms],
    )?;
    for limit in ["reveal", "reset", "settings"] {
        transaction.execute("DELETE FROM dm_limits WHERE key = ?1", [format!("{limit}:{account}")])?;
    }
    Ok(transaction.commit()?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ipc::Registration;

    const NOW: i64 = 1_795_183_320_000;

    #[test]
    fn waits_read_in_minutes_then_hours_never_sooner_than_they_are() {
        let said: Vec<String> = [1, 60, 61, 3540, 3541, 3600, 3601, 68_460].into_iter().map(later).collect();
        assert_eq!(
            said,
            ["in a minute", "in a minute", "in 2 minutes", "in 59 minutes", "in an hour", "in an hour", "in 2 hours", "in 20 hours"]
        );
    }

    fn setup() -> (Connection, Keys) {
        let db = Connection::open_in_memory().expect("db");
        let keys = Keys::from_bytes([7; 32]);
        keys.bind(&db).expect("bound");
        db.execute_batch(SCHEMA).expect("schema");
        db.execute_batch(super::super::contacts::SCHEMA).expect("schema");
        db.execute_batch(super::super::reports::SCHEMA).expect("schema");
        db.execute_batch(super::super::messages::SCHEMA).expect("schema");
        db.execute_batch(super::super::sync::SCHEMA).expect("schema");
        db.execute_batch(super::super::review::SCHEMA).expect("schema");
        db.execute_batch(super::super::agents::SCHEMA).expect("schema");
        db.execute_batch(super::super::leaves::SCHEMA).expect("schema");
        db.execute_batch(super::super::groups::SCHEMA).expect("schema");
        db.execute_batch(super::super::handoffs::SCHEMA).expect("schema");
        db.execute_batch(claktory_dm_crypto::storage::SCHEMA).expect("schema");
        (db, keys)
    }

    fn device(account: &str) -> Caller {
        Caller::Device { account_id: account.into(), device_key: "k".into(), registrations: Vec::<Registration>::new() }
    }

    fn call(db: &mut Connection, keys: &Keys, method: &str, route: &str, caller: &Caller, body: Value) -> (u16, Value) {
        let bytes = if body.is_null() { Vec::new() } else { body.to_string().into_bytes() };
        super::route(db, keys, method, route, caller, &bytes, NOW).expect("a route")
    }

    #[test]
    fn friend_ids_read_back_however_they_are_typed_and_nothing_else_does() {
        let bytes = random::<CODE_BYTES>();
        let code = format_code(&bytes);
        assert!(code.starts_with("cf1_") && code.len() == 36, "{code}");
        assert_eq!(parse_code(&code), Some(bytes));
        let payload = &code[4..];
        let spaced = format!(" CF1_{} {}-{} ", payload[..8].to_lowercase(), &payload[8..16], &payload[16..]);
        assert_eq!(parse_code(&spaced), Some(bytes));
        assert_eq!(parse_code(payload), Some(bytes), "without the prefix");
        let last_not_base32 = format!("{}1", &code[..35]);
        for bad in ["", "cf1_", &code[..35], &format!("{code}A"), &last_not_base32, &format!("cf2_{payload}"), &format!("cf1_{payload}_")] {
            assert_eq!(parse_code(bad), None, "{bad}");
        }
    }

    #[test]
    fn no_name_says_claktory_however_it_is_written() {
        for bad in [
            "Claktory",
            "Claktory Support",
            "C1aktory Team",
            "claktory_support",
            "CLAK-TORY",
            "c l a k t o r y",
            "CIaktory",
            "Сlaktоry",
            "Ｃｌａｋｔｏｒｙ",
            "Klaktory's Claktory",
        ] {
            assert!(says_claktory(bad), "{bad}");
        }
        for fine in ["Matt", "Zoë 🌙", "Clak", "Tory", "Factory", "Claire Torres"] {
            assert!(!says_claktory(fine), "{fine}");
        }
    }

    #[test]
    fn names_are_kept_plain() {
        assert_eq!(clean_name("  Matt   Smith "), Some("Matt Smith".into()));
        assert_eq!(clean_name("Zoë 🌙"), Some("Zoë 🌙".into()));
        assert_eq!(clean_name(&"a".repeat(80)), Some("a".repeat(80)));
        for bad in ["", "   ", "Matt\u{202E}htiM", "Ma\u{200B}tt", "Matt\nAdmin", &"a".repeat(81), "\u{2066}x"] {
            assert_eq!(clean_name(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn a_person_sees_and_changes_only_their_own_identity_from_their_devices() {
        let (mut db, keys) = setup();
        let matt = device("matt");
        let (status, me) = call(&mut db, &keys, "GET", "me", &matt, Value::Null);
        assert_eq!(status, 200, "{me}");
        assert_eq!(me["profile"]["name"], "");
        assert_eq!(me["privacy"], json!({"requests_on": true, "revision": 1}));
        assert_eq!(me["friend_id"]["generation"], 1);
        assert!(!me.to_string().contains("cf1_"), "the code shows only when revealed: {me}");

        let (_, shown) = call(&mut db, &keys, "POST", "me/friend-id/reveal", &matt, json!({"request_nonce": "n1"}));
        let code = shown["code"].as_str().expect("a code").to_owned();
        assert!(parse_code(&code).is_some(), "{code}");
        let (_, again) = call(&mut db, &keys, "POST", "me/friend-id/reveal", &matt, Value::Null);
        assert_eq!(again["code"], code.as_str(), "the same until reset");

        // Reset: stale generations refused; the new code differs, the old is
        // a tombstone with no reveal, still found by its digest.
        assert_eq!(call(&mut db, &keys, "POST", "me/friend-id/reset", &matt, json!({"generation": 2})).0, 409);
        let (status, reset) = call(&mut db, &keys, "POST", "me/friend-id/reset", &matt, json!({"generation": 1}));
        assert_eq!(status, 200, "{reset}");
        assert_eq!(reset["generation"], 2);
        assert_ne!(reset["code"], code.as_str());
        let old = code_digest(&keys, &db, &parse_code(&code).expect("parsed"), NOW).expect("digest");
        let (owner, retired, reveal): (String, Option<i64>, Option<Vec<u8>>) = db
            .query_row("SELECT account_id, retired, reveal FROM dm_friend_codes WHERE digest = ?1", [&old], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?))
            })
            .expect("the tombstone");
        assert_eq!((owner.as_str(), retired.is_some(), reveal), ("matt", true, None));

        // Profile and privacy, by revision.
        let (status, me) = call(&mut db, &keys, "PUT", "me/profile", &matt, json!({"name": "  Matt ", "revision": 1}));
        assert_eq!(status, 200, "{me}");
        assert_eq!((me["profile"]["name"].as_str(), me["profile"]["revision"].as_i64()), (Some("Matt"), Some(2)));
        let seed = me["profile"]["avatar_seed"].clone();
        assert_eq!(call(&mut db, &keys, "PUT", "me/profile", &matt, json!({"name": "Old tab", "revision": 1})).0, 409);
        let (_, me) = call(&mut db, &keys, "PUT", "me/profile", &matt, json!({"new_avatar": true, "revision": 2}));
        assert_ne!(me["profile"]["avatar_seed"], seed);
        assert_eq!(me["profile"]["name"], "Matt");
        assert_eq!(call(&mut db, &keys, "PUT", "me/profile", &matt, json!({"name": "Ma\u{202E}tt", "revision": 3})).0, 400);
        let (status, refused) = call(&mut db, &keys, "PUT", "me/profile", &matt, json!({"name": "C1aktory Support", "revision": 3}));
        assert_eq!((status, refused["error"].as_str()), (400, Some("invalid_request")));
        let (_, me) = call(&mut db, &keys, "PUT", "me/privacy", &matt, json!({"requests_on": false, "revision": 1}));
        assert_eq!(me["privacy"], json!({"requests_on": false, "revision": 2}));
        assert_eq!(call(&mut db, &keys, "PUT", "me/privacy", &matt, json!({"requests_on": true, "revision": 1})).0, 409);
        assert_eq!(call(&mut db, &keys, "PUT", "me/privacy", &matt, json!({"requests_on": true, "revision": 2, "x": 1})).0, 400);

        // Nobody else: another account sees its own, a backend or agent nothing.
        let (_, eve) = call(&mut db, &keys, "GET", "me", &device("eve"), Value::Null);
        assert_eq!(eve["profile"]["name"], "");
        let backend = Caller::Backend { account_id: "matt".into(), claktory_id: "mac".into(), epoch: 1, agent: None };
        let (status, refused) = call(&mut db, &keys, "POST", "me/friend-id/reveal", &backend, Value::Null);
        assert_eq!((status, refused["error"].as_str()), (403, Some("device_only")));
        let web = Caller::Account { account_id: "matt".into(), registrations: Vec::new() };
        assert_eq!(call(&mut db, &keys, "GET", "me", &web, Value::Null).0, 403);
        assert!(super::route(&mut db, &keys, "GET", "people", &matt, b"", NOW).is_none(), "not this module's");

        // At rest: no name or code in the database's bytes.
        let dump: Vec<Vec<u8>> = {
            let mut statement = db
                .prepare("SELECT profile FROM dm_people UNION ALL SELECT reveal FROM dm_friend_codes WHERE reveal IS NOT NULL")
                .expect("dump");
            statement.query_map([], |row| row.get(0)).expect("rows").collect::<Result<_, _>>().expect("bytes")
        };
        for bytes in dump {
            assert!(!bytes.windows(4).any(|window| window == b"Matt" || window == b"cf1_"));
        }
    }

    #[test]
    fn resets_and_reveals_are_limited_and_a_purge_leaves_only_ownerless_tombstones() {
        let (mut db, keys) = setup();
        let matt = device("matt");
        for _ in 0..30 {
            assert_eq!(call(&mut db, &keys, "POST", "me/friend-id/reveal", &matt, Value::Null).0, 200);
        }
        assert_eq!(call(&mut db, &keys, "POST", "me/friend-id/reveal", &matt, Value::Null).0, 429);
        for generation in 1..=5 {
            assert_eq!(call(&mut db, &keys, "POST", "me/friend-id/reset", &matt, json!({"generation": generation})).0, 200);
        }
        assert_eq!(call(&mut db, &keys, "POST", "me/friend-id/reset", &matt, json!({"generation": 6})).0, 429);

        purge(&mut db, &keys, "matt", NOW).expect("purged");
        let left: Vec<(String, Option<i64>, Option<Vec<u8>>)> = {
            let mut statement = db.prepare("SELECT account_id, retired, reveal FROM dm_friend_codes").expect("codes");
            statement
                .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
                .expect("rows")
                .collect::<Result<_, _>>()
                .expect("all")
        };
        assert_eq!(left.len(), 6);
        assert!(left.iter().all(|(owner, retired, reveal)| owner.is_empty() && retired.is_some() && reveal.is_none()), "{left:?}");
        let people: i64 = db.query_row("SELECT COUNT(*) FROM dm_people", [], |row| row.get(0)).expect("count");
        assert_eq!(people, 0);
        // Back later: a new identity, never the old codes.
        let (_, me) = call(&mut db, &keys, "GET", "me", &matt, Value::Null);
        assert_eq!(me["friend_id"]["generation"], 1);
    }
}
