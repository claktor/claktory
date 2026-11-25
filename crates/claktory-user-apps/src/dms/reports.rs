//! Reports (D-S5): a person reports someone who sent them a request, a
//! friend (or one they were friends with) or someone they blocked, to
//! Claktory's operators. Nobody else is told, the person reported least of
//! all.
//!
//! A report keeps only what the person chose to send: always why (a reason)
//! and what it's about (a request, a friendship or a block, with its times);
//! their own words, the name and avatar the person reported showed them,
//! and, about a contact, messages of their conversation, only if they add
//! them. Never anything else of theirs. Messages are end-to-end encrypted,
//! so those are the text the reporter's device shows, which nobody here can
//! check (dms-e2ee.md §7: no franking in v1); operators see them labelled
//! so. What they chose is sealed with a data key of the report's own,
//! forgotten when the report goes, 30 days after it was made.
//!
//! Operators read open reports with `claktory-user-apps reports`, which
//! records who looked at each and when, and close them there.

use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::contacts::contact_tag;
use super::identity::{self, Refusal, body, hex, invalid, random, shown_profile, take};
use super::keys::{self, Field, Keys, Refused};

pub const SCHEMA: &str = "
-- Reports to Claktory's operators. `reporter` is '' once their account is
-- deleted (the report stays, as they chose to send it). `evidence` is
-- sealed with the report's own data key (`report:<id>`): only what the
-- person chose. `about` (sealed the same way) is the name and avatar the
-- person reported showed when it was made, for the reporter's list. state:
-- open, closed.
CREATE TABLE IF NOT EXISTS dm_reports (
    id TEXT PRIMARY KEY,
    reporter TEXT NOT NULL,
    action_id TEXT NOT NULL,
    reported TEXT NOT NULL,
    subject TEXT NOT NULL,
    reason TEXT NOT NULL,
    evidence BLOB NOT NULL,
    about BLOB NOT NULL,
    state TEXT NOT NULL,
    created INTEGER NOT NULL,
    expires INTEGER NOT NULL,
    UNIQUE (reporter, action_id)
);
CREATE INDEX IF NOT EXISTS dm_reports_state ON dm_reports (state, created);
CREATE INDEX IF NOT EXISTS dm_reports_expiry ON dm_reports (expires);
-- Who read or closed which report, and when.
CREATE TABLE IF NOT EXISTS dm_report_views (
    report TEXT NOT NULL,
    operator TEXT NOT NULL,
    action TEXT NOT NULL,
    at INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS dm_report_views_report ON dm_report_views (report);
";

const DAY_MS: i64 = 86_400_000;
/// How long a report is kept (the proposed retention, D3).
const KEEP_MS: i64 = 30 * DAY_MS;
/// Reports a person may make a day.
const REPORTS_PER_DAY: i64 = 20;
/// The person's own words, in characters.
const MAX_NOTE: usize = 1000;
/// Why someone reports.
const REASONS: [&str; 4] = ["spam", "harassment", "impersonation", "other"];
/// Messages a report carries at most, the most text in one (in bytes, the
/// content's own cap), and in all.
const MAX_MESSAGES: usize = 50;
const MAX_MESSAGE_TEXT: usize = 16 * 1024;
const MAX_MESSAGES_TEXT: usize = 64 * 1024;
/// What operators see beside a report's messages.
const MESSAGES_LABEL: &str = "As the reporter's device shows them; not verifiable.";
/// What every report is answered with.
const RECEIVED: &str = "Report sent. Claktory's team will look at it. They won't know you reported them.";

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Report {
    /// The device's id for this report, the same on every retry.
    action_id: String,
    /// One of: a request that reached the person, one of their contacts
    /// (now or before), one of their blocks.
    #[serde(default)]
    request_id: Option<String>,
    #[serde(default)]
    contact_id: Option<String>,
    #[serde(default)]
    block_id: Option<String>,
    reason: String,
    /// Their own words, if they add any.
    #[serde(default)]
    note: Option<String>,
    /// Whether to send the name and avatar the person reported shows them.
    #[serde(default)]
    include_profile: bool,
    /// Messages of the conversation with a contact they chose, oldest
    /// first: as their device shows them.
    #[serde(default)]
    messages: Vec<Quoted>,
    #[serde(default)]
    #[allow(dead_code)]
    request_nonce: Option<String>,
}

/// One message the reporter chose: its place in the conversation, its text
/// as their device shows it, who sent it ("you", the reporter, or "them")
/// and when.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Quoted {
    seq: i64,
    text: String,
    from: String,
    sent_ms: i64,
}

/// What a report keeps, sealed: only what the person chose.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Evidence {
    /// What it's about, with its times.
    about: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    note: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    profile: Option<Value>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    messages: Vec<Quoted>,
}

fn not_found() -> Refusal {
    Refusal(404, "not_found", "There's nothing here.".to_owned())
}

fn field<'a>(id: &'a str, column: &'a str) -> Field<'a> {
    Field { table: "dm_reports", record: id, column }
}

fn purpose(id: &str) -> String {
    format!("report:{id}")
}

/// Whether `route` is this module's.
pub fn knows(method: &str, route: &str) -> bool {
    matches!((method, route), ("POST" | "GET", "reports"))
}

/// Answers one of this module's routes for `account`, from one of their
/// devices.
pub fn answer(db: &mut Connection, keys: &Keys, method: &str, account: &str, bytes: &[u8], now_ms: i64) -> Result<(u16, Value), Refusal> {
    let transaction = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
    identity::ensure(&transaction, keys, account, now_ms)?;
    let answer = match method {
        "POST" => (200, report(&transaction, keys, account, body(bytes)?, now_ms)?),
        _ => {
            expire(&transaction, now_ms)?;
            (200, reports(&transaction, keys, account, now_ms)?)
        }
    };
    transaction.commit()?;
    Ok(answer)
}

/// The characters of `text` that are controls but line breaks, or change
/// direction or hide (the set DM names are checked against), a variation
/// selector anywhere but after a visible character included.
fn hidden(text: &str) -> impl Iterator<Item = char> + '_ {
    claktory_dm_crypto::certificate::hidden_in(text).filter(|c| *c != '\n')
}

/// The clean note: trimmed, at most 1000 characters, no control characters
/// but line breaks, nothing that changes direction or hides.
fn clean_note(note: &str) -> Result<Option<String>, Refusal> {
    let note = note.trim();
    if hidden(note).next().is_some() || note.chars().count() > MAX_NOTE {
        return Err(invalid("A note has at most 1000 characters, and no hidden or direction-changing ones."));
    }
    Ok((!note.is_empty()).then(|| note.to_owned()))
}

/// The messages a report carries: only about a contact, at most 50, oldest
/// first, each with its text. What the text says is kept as it is, hidden
/// characters too, since they may be what's reported; operators see them
/// as codes.
fn check_messages(report: &Report) -> Result<(), Refusal> {
    let messages = &report.messages;
    if messages.is_empty() {
        return Ok(());
    }
    if report.contact_id.is_none() {
        return Err(invalid("Messages go only with a contact_id."));
    }
    if messages.len() > MAX_MESSAGES {
        return Err(invalid("A report has at most 50 messages."));
    }
    let rising = messages.windows(2).all(|pair| pair[0].seq < pair[1].seq);
    let each = messages.iter().all(|message| {
        message.seq >= 1
            && message.sent_ms >= 0
            && matches!(message.from.as_str(), "you" | "them")
            && (1..=MAX_MESSAGE_TEXT).contains(&message.text.len())
    });
    if !rising || !each {
        return Err(invalid("Each message has its seq (oldest first), its text (up to 16 KiB), from \"you\" or \"them\", and sent_ms."));
    }
    if messages.iter().map(|message| message.text.len()).sum::<usize>() > MAX_MESSAGES_TEXT {
        return Err(invalid("A report's messages have at most 64 KiB of text in all."));
    }
    Ok(())
}

fn report(transaction: &Transaction<'_>, keys: &Keys, account: &str, report: Report, now_ms: i64) -> Result<Value, Refusal> {
    if !(22..=64).contains(&report.action_id.len())
        || !report.action_id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    {
        return Err(invalid("action_id has to be 22 to 64 letters, digits, - and _."));
    }
    if !REASONS.contains(&report.reason.as_str()) {
        return Err(invalid("reason is spam, harassment, impersonation or other."));
    }
    let note = clean_note(report.note.as_deref().unwrap_or(""))?;
    check_messages(&report)?;
    // A retry: the same report.
    let earlier: Option<String> = transaction
        .query_row("SELECT id FROM dm_reports WHERE reporter = ?1 AND action_id = ?2", params![account, report.action_id], |row| row.get(0))
        .optional()?;
    if let Some(id) = earlier {
        return report_json(transaction, keys, account, &id, now_ms);
    }
    // Whom it's about, from something of the person's own.
    let (subject, reported, about) = match (&report.request_id, &report.contact_id, &report.block_id) {
        (Some(request), None, None) => {
            let found: Option<(String, i64)> = transaction
                .query_row(
                    "SELECT sender, created FROM dm_requests WHERE id = ?1 AND target = ?2 AND delivered = 1",
                    params![request, account],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .optional()?;
            let (sender, sent) = found.ok_or_else(not_found)?;
            ("request", sender, json!({"kind": "request", "sent_ms": sent}))
        }
        (None, Some(contact), None) => {
            let found: Option<(String, String, i64, Option<i64>)> = transaction
                .query_row(
                    "SELECT low, high, created, ended FROM dm_contacts WHERE id = ?1 AND (low = ?2 OR high = ?2)",
                    params![contact, account],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
                )
                .optional()?;
            let (low, high, since, ended) = found.ok_or_else(not_found)?;
            let other = if low == account { high } else { low };
            ("contact", other, json!({"kind": "contact", "since_ms": since, "ended_ms": ended}))
        }
        (None, None, Some(block)) => {
            let found: Option<(String, i64)> = transaction
                .query_row("SELECT blocked, created FROM dm_blocks WHERE id = ?1 AND blocker = ?2", params![block, account], |row| {
                    Ok((row.get(0)?, row.get(1)?))
                })
                .optional()?;
            let (blocked, since) = found.ok_or_else(not_found)?;
            ("block", blocked, json!({"kind": "block", "blocked_ms": since}))
        }
        _ => return Err(invalid("Name one request_id, contact_id or block_id.")),
    };
    take(transaction, &format!("reports:{account}"), 86_400, REPORTS_PER_DAY, now_ms)?;
    expire(transaction, now_ms)?;
    // How they showed then: the reporter's list keeps to it, so it follows
    // none of their later changes.
    let (name, avatar_seed) = shown_profile(transaction, keys, &reported)?.unwrap_or_default();
    let shown = json!({"name": name, "avatar_seed": avatar_seed});
    let profile = report.include_profile.then(|| shown.clone());
    let id = hex(&random::<16>());
    let evidence =
        zeroize::Zeroizing::new(serde_json::to_vec(&Evidence { about, note, profile, messages: report.messages }).unwrap_or_default());
    let sealed = keys.seal(transaction, &purpose(&id), field(&id, "evidence"), &evidence, now_ms)?;
    let shown = zeroize::Zeroizing::new(serde_json::to_vec(&shown).unwrap_or_default());
    let shown = keys.seal(transaction, &purpose(&id), field(&id, "about"), &shown, now_ms)?;
    transaction.execute(
        "INSERT INTO dm_reports (id, reporter, action_id, reported, subject, reason, evidence, about, state, created, expires)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 'open', ?9, ?10)",
        params![id, account, report.action_id, reported, subject, report.reason, sealed, shown, now_ms, now_ms + KEEP_MS],
    )?;
    report_json(transaction, keys, account, &id, now_ms)
}

/// A report as the person who made it sees it: whom it's about as they
/// showed when it was made.
fn report_json(db: &Connection, keys: &Keys, account: &str, id: &str, now_ms: i64) -> Result<Value, Refusal> {
    let (reported, subject, reason, sealed, state, created): (String, String, String, Vec<u8>, String, i64) =
        db.query_row("SELECT reported, subject, reason, about, state, created FROM dm_reports WHERE id = ?1", [id], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?, row.get(5)?))
        })?;
    let opened = keys.open(db, field(id, "about"), &sealed)?;
    let mut about: Value = serde_json::from_slice(opened.bytes()).map_err(|_| Refused::DoesntOpen)?;
    about["tag"] = json!(contact_tag(db, keys, account, &reported, now_ms)?);
    Ok(json!({
        "id": id,
        "about": about,
        "subject": subject,
        "reason": reason,
        "state": if state == "open" { "received" } else { "reviewed" },
        "created_ms": created,
        "message": RECEIVED,
    }))
}

/// The person's own reports, newest first.
fn reports(transaction: &Transaction<'_>, keys: &Keys, account: &str, now_ms: i64) -> Result<Value, Refusal> {
    let ids: Vec<String> = {
        let mut statement = transaction.prepare("SELECT id FROM dm_reports WHERE reporter = ?1 AND expires > ?2 ORDER BY created DESC")?;
        statement.query_map(params![account, now_ms], |row| row.get(0))?.collect::<Result<_, _>>()?
    };
    let mut reports = Vec::new();
    for id in ids {
        let mut shown = report_json(transaction, keys, account, &id, now_ms)?;
        if let Some(shown) = shown.as_object_mut() {
            shown.remove("message");
        }
        reports.push(shown);
    }
    Ok(json!({"reports": reports}))
}

/// Deletes reports past their 30 days, and their keys, so what they held
/// opens to nothing, in backups too.
fn expire(transaction: &Transaction<'_>, now_ms: i64) -> Result<usize, Refused> {
    let ids: Vec<String> = {
        let mut statement = transaction.prepare("SELECT id FROM dm_reports WHERE expires <= ?1 LIMIT 100")?;
        statement.query_map([now_ms], |row| row.get(0))?.collect::<Result<_, _>>()?
    };
    for id in &ids {
        transaction.execute("DELETE FROM dm_reports WHERE id = ?1", [id])?;
        transaction.execute("DELETE FROM dm_report_views WHERE report = ?1", [id])?;
        keys::forget(transaction, &purpose(id))?;
    }
    Ok(ids.len())
}

/// Deletes every report past its 30 days, with its key: the worker runs it
/// when it starts and every hour, besides on the way.
pub fn sweep(db: &mut Connection, now_ms: i64) -> Result<usize, Refused> {
    let mut total = 0;
    loop {
        let transaction = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let deleted = expire(&transaction, now_ms)?;
        transaction.commit()?;
        total += deleted;
        if deleted < 100 {
            return Ok(total);
        }
    }
}

/// An account is deleted: reports it made stay, as it chose to send them,
/// with no one as their reporter. Reports about it stay until they expire.
pub fn purge(transaction: &Transaction<'_>, account: &str) -> rusqlite::Result<()> {
    transaction.execute("UPDATE dm_reports SET reporter = '', action_id = id WHERE reporter = ?1", [account])?;
    transaction.execute("DELETE FROM dm_limits WHERE key = ?1", [format!("reports:{account}")])?;
    Ok(())
}

/// An open report's row: id, reporter, reported, subject, reason, sealed
/// evidence, made.
type Open = (String, String, String, String, String, Vec<u8>, i64);

/// For operators: the open reports, oldest first, each with what its
/// reporter chose to send, and who it's from and about (accounts). Each one
/// shown is recorded as read by `operator`.
pub fn review(db: &mut Connection, keys: &Keys, operator: &str, now_ms: i64) -> Result<Vec<Value>, Refused> {
    let transaction = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
    expire(&transaction, now_ms)?;
    let rows: Vec<Open> = {
        let mut statement = transaction.prepare(
            "SELECT id, reporter, reported, subject, reason, evidence, created FROM dm_reports WHERE state = 'open' ORDER BY created",
        )?;
        statement
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?, row.get(5)?, row.get(6)?)))?
            .collect::<Result<_, _>>()?
    };
    let mut shown = Vec::new();
    for (id, reporter, reported, subject, reason, sealed, created) in rows {
        let opened = keys.open(&transaction, field(&id, "evidence"), &sealed)?;
        let mut evidence: Value = serde_json::from_slice(opened.bytes()).map_err(|_| Refused::DoesntOpen)?;
        labelled(&mut evidence);
        transaction.execute(
            "INSERT INTO dm_report_views (report, operator, action, at) VALUES (?1, ?2, 'read', ?3)",
            params![id, operator, now_ms],
        )?;
        shown.push(json!({
            "id": id,
            "reporter": reporter,
            "reported": reported,
            "subject": subject,
            "reason": reason,
            "evidence": evidence,
            "created_ms": created,
        }));
    }
    transaction.commit()?;
    Ok(shown)
}

/// A report's messages, for operators: labelled as the reporter's device
/// shows them, and each one's hidden characters named by their codes.
fn labelled(evidence: &mut Value) {
    let Some(messages) = evidence.get_mut("messages").and_then(Value::as_array_mut) else { return };
    for message in messages.iter_mut() {
        let codes: Vec<String> = hidden(message["text"].as_str().unwrap_or_default()).map(|c| format!("U+{:04X}", u32::from(c))).collect();
        if !codes.is_empty() {
            message["hidden"] = json!(codes);
        }
    }
    evidence["messages_label"] = json!(MESSAGES_LABEL);
}

/// For operators: closes an open report (the person who made it sees it
/// reviewed), recorded as `operator`'s. Whether there was one.
pub fn close(db: &mut Connection, id: &str, operator: &str, now_ms: i64) -> Result<bool, Refused> {
    let transaction = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let closed = transaction.execute("UPDATE dm_reports SET state = 'closed' WHERE id = ?1 AND state = 'open'", [id])? > 0;
    if closed {
        transaction.execute(
            "INSERT INTO dm_report_views (report, operator, action, at) VALUES (?1, ?2, 'closed', ?3)",
            params![id, operator, now_ms],
        )?;
    }
    transaction.commit()?;
    Ok(closed)
}

#[cfg(test)]
mod tests {
    use super::super::contacts::tests::World;
    use super::*;
    use crate::ipc::Caller;

    fn action(n: u32) -> String {
        format!("report-{n:018}")
    }

    /// Ana sends Bo a request; Bo's view of it.
    fn requested(world: &mut World) -> String {
        world.name("ana", "Ana");
        let bo = world.code("bo");
        world.send("ana", &bo)
    }

    fn mine(world: &mut World, account: &str) -> Vec<Value> {
        world.call(account, "GET", "reports", Value::Null).1["reports"].as_array().cloned().unwrap_or_default()
    }

    #[test]
    fn a_report_keeps_only_what_the_person_chose_sealed_and_tells_no_one() {
        let mut world = World::new();
        let request = requested(&mut world);
        let ask = json!({"action_id": action(1), "request_id": request, "reason": "spam",
                         "note": "  Sends these to everyone.  ", "include_profile": true});
        let (status, report) = world.call("bo", "POST", "reports", ask.clone());
        assert_eq!(status, 200, "{report}");
        assert_eq!(report["message"], RECEIVED);
        assert_eq!((report["about"]["name"].as_str(), report["subject"].as_str()), (Some("Ana"), Some("request")));
        assert_eq!((report["reason"].as_str(), report["state"].as_str()), (Some("spam"), Some("received")));
        let id = report["id"].as_str().expect("id").to_owned();
        assert_eq!(world.call("bo", "POST", "reports", ask).1["id"], id.as_str(), "a retry is the same report");

        // Only Bo sees it; Ana is told nothing, and the request still waits.
        assert_eq!(mine(&mut world, "bo").len(), 1);
        assert_eq!(mine(&mut world, "bo")[0].get("message"), None);
        assert_eq!(mine(&mut world, "ana"), Vec::<Value>::new());
        assert_eq!(world.outgoing("ana")[0]["state"], "pending");
        assert_eq!(world.incoming("bo").len(), 1, "reporting isn't declining or blocking");
        // Bo's list shows Ana as she was when he reported her, whatever she
        // changes after.
        world.name("ana", "Not Ana");
        let listed = &mine(&mut world, "bo")[0];
        assert_eq!(listed["about"]["name"], "Ana");
        assert_eq!(listed["about"]["tag"].as_str().map(str::len), Some(6));

        // Sealed at rest: the note and name aren't in the database as text.
        let sealed: Vec<u8> = world.db.query_row("SELECT evidence FROM dm_reports WHERE id = ?1", [&id], |row| row.get(0)).expect("row");
        let text = String::from_utf8_lossy(&sealed);
        assert!(!text.contains("everyone") && !text.contains("Ana"), "{text}");

        // An operator reads what Bo chose, and that they read it is kept.
        let shown = review(&mut world.db, &world.keys, "lena", world.now).expect("reviewed");
        assert_eq!(shown.len(), 1);
        assert_eq!((shown[0]["reporter"].as_str(), shown[0]["reported"].as_str()), (Some("bo"), Some("ana")));
        assert_eq!(
            shown[0]["evidence"],
            json!({"about": {"kind": "request", "sent_ms": world.now}, "note": "Sends these to everyone.",
                   "profile": {"name": "Ana", "avatar_seed": shown[0]["evidence"]["profile"]["avatar_seed"]}})
        );
        let read: i64 = world
            .db
            .query_row("SELECT COUNT(*) FROM dm_report_views WHERE report = ?1 AND operator = 'lena' AND action = 'read'", [&id], |row| {
                row.get(0)
            })
            .expect("count");
        assert_eq!(read, 1);
        assert!(close(&mut world.db, &id, "lena", world.now).expect("closed"));
        assert!(!close(&mut world.db, &id, "lena", world.now).expect("once"));
        assert_eq!(mine(&mut world, "bo")[0]["state"], "reviewed");
        assert_eq!(review(&mut world.db, &world.keys, "lena", world.now).expect("reviewed"), Vec::<Value>::new());
    }

    #[test]
    fn a_report_is_about_something_of_the_person_s_own() {
        let mut world = World::new();
        let request = requested(&mut world);
        let ask = |n: u32, subject: Value| {
            let mut ask = json!({"action_id": action(n), "reason": "harassment"});
            for (key, value) in subject.as_object().expect("object") {
                ask[key] = value.clone();
            }
            ask
        };
        // Someone else's request, contact or block: nothing there.
        assert_eq!(world.call("carol", "POST", "reports", ask(1, json!({"request_id": request}))).0, 404);
        assert_eq!(world.call("ana", "POST", "reports", ask(2, json!({"request_id": request}))).0, 404, "not the sender's own");
        // Not what a report takes.
        assert_eq!(world.call("bo", "POST", "reports", ask(3, json!({}))).0, 400);
        assert_eq!(world.call("bo", "POST", "reports", ask(4, json!({"request_id": request, "block_id": "x"}))).0, 400);
        let mut wrong = ask(5, json!({"request_id": request}));
        wrong["reason"] = json!("annoying");
        assert_eq!(world.call("bo", "POST", "reports", wrong).0, 400);
        let mut hidden = ask(6, json!({"request_id": request}));
        hidden["note"] = json!("a\u{202E}b");
        assert_eq!(world.call("bo", "POST", "reports", hidden).0, 400);
        let mut long = ask(7, json!({"request_id": request}));
        long["note"] = json!("x".repeat(MAX_NOTE + 1));
        assert_eq!(world.call("bo", "POST", "reports", long).0, 400);
        // Only from the person's devices.
        let backend = Caller::Backend { account_id: "bo".into(), claktory_id: "mac".into(), epoch: 1, agent: None };
        let bytes = ask(8, json!({"request_id": request})).to_string().into_bytes();
        let (status, _) = identity::route(&mut world.db, &world.keys, "POST", "reports", &backend, &bytes, world.now).expect("route");
        assert_eq!(status, 403);

        // A friend, one no longer, and someone blocked can all be reported;
        // with nothing chosen, only why and what it's about is kept.
        let (_, contact) = world.accept("bo", &request);
        let contact = contact["id"].as_str().expect("id").to_owned();
        assert_eq!(world.call("ana", "POST", "reports", ask(9, json!({"contact_id": contact}))).0, 200);
        world.call("bo", "DELETE", &format!("contacts/{contact}"), Value::Null);
        assert_eq!(world.call("bo", "POST", "reports", ask(10, json!({"contact_id": contact}))).0, 200, "ended, still theirs");
        let (_, block) = world.call("bo", "POST", "blocks", json!({"request_id": request}));
        let (status, report) = world.call("bo", "POST", "reports", ask(11, json!({"block_id": block["id"]})));
        assert_eq!((status, report["subject"].as_str()), (200, Some("block")));
        let shown = review(&mut world.db, &world.keys, "lena", world.now).expect("reviewed");
        let blocked = shown.iter().find(|report| report["subject"] == "block").expect("the block's");
        assert_eq!(blocked["evidence"], json!({"about": {"kind": "block", "blocked_ms": world.now}}));
        assert_eq!(world.call("carol", "POST", "reports", ask(12, json!({"block_id": block["id"]}))).0, 404);
    }

    #[test]
    fn a_note_with_anything_hidden_is_refused_but_an_emoji_is_kept() {
        for note in ["a\u{202E}b", "a\u{E0065}b", "a\u{61C}b", "Ana\u{3164}", "a\u{FE0F}\u{FE0F}", "\u{FE0F}a", "a\tb"] {
            assert!(clean_note(note).is_err(), "{note:?}");
        }
        assert_eq!(
            clean_note(" two\nlines, thanks \u{2764}\u{FE0F} ").ok().flatten().as_deref(),
            Some("two\nlines, thanks \u{2764}\u{FE0F}")
        );
    }

    #[test]
    fn reports_go_after_30_days_with_their_keys_and_outlast_the_reporter() {
        let mut world = World::new();
        let request = requested(&mut world);
        let (_, first) = world.call("bo", "POST", "reports", json!({"action_id": action(1), "request_id": request, "reason": "other"}));
        let first = first["id"].as_str().expect("id").to_owned();

        // Bo's account is deleted: the report stays, from no one.
        identity::purge(&mut world.db, &world.keys, "bo", world.now).expect("purged");
        let shown = review(&mut world.db, &world.keys, "lena", world.now).expect("reviewed");
        assert_eq!((shown.len(), shown[0]["reporter"].as_str()), (1, Some("")));

        // 30 days on, the worker's sweep deletes it and its key, with no one
        // asking.
        world.now += KEEP_MS;
        assert_eq!(sweep(&mut world.db, world.now).expect("swept"), 1);
        assert_eq!(review(&mut world.db, &world.keys, "lena", world.now).expect("reviewed"), Vec::<Value>::new());
        let left: i64 =
            world.db.query_row("SELECT COUNT(*) FROM dm_keys WHERE purpose = ?1", [purpose(&first)], |row| row.get(0)).expect("count");
        assert_eq!(left, 0);
        let views: i64 = world.db.query_row("SELECT COUNT(*) FROM dm_report_views", [], |row| row.get(0)).expect("count");
        assert_eq!(views, 0);
    }

    #[test]
    fn messages_the_person_chose_go_sealed_and_labelled_as_not_verifiable() {
        let mut world = World::new();
        let request = requested(&mut world);
        let (_, contact) = world.accept("bo", &request);
        let contact = contact["id"].as_str().expect("id").to_owned();
        let said = |seq: i64, text: &str, from: &str| json!({"seq": seq, "text": text, "from": from, "sent_ms": 1_000 + seq});
        let ask =
            |n: u32, messages: Value| json!({"action_id": action(n), "contact_id": contact, "reason": "harassment", "messages": messages});
        // Kept as the device shows them, a direction change too.
        let chosen = json!([
            said(3, "you again", "them"),
            said(4, "stop", "you"),
            said(7, "no\u{202E}won", "them"),
            said(8, "hi\u{3164}\u{E0065}\u{61C} \u{2764}\u{FE0F}", "them")
        ]);
        let (status, report) = world.call("bo", "POST", "reports", ask(1, chosen.clone()));
        assert_eq!(status, 200, "{report}");
        let id = report["id"].as_str().expect("id").to_owned();
        assert_eq!(world.call("bo", "POST", "reports", ask(1, chosen.clone())).1["id"], id.as_str(), "a retry is the same report");
        let sealed: Vec<u8> = world.db.query_row("SELECT evidence FROM dm_reports WHERE id = ?1", [&id], |row| row.get(0)).expect("row");
        assert!(!String::from_utf8_lossy(&sealed).contains("you again"));

        // Operators see them labelled, with the hidden character's code.
        let shown = review(&mut world.db, &world.keys, "lena", world.now).expect("reviewed");
        let evidence = &shown[0]["evidence"];
        assert_eq!(evidence["messages_label"], MESSAGES_LABEL);
        assert_eq!(evidence["messages"][0], said(3, "you again", "them"));
        assert_eq!(evidence["messages"][1]["from"], "you");
        assert_eq!(
            (evidence["messages"][2]["text"].as_str(), &evidence["messages"][2]["hidden"]),
            (Some("no\u{202E}won"), &json!(["U+202E"]))
        );
        // Every hidden character named, an emoji's own selector not.
        assert_eq!(evidence["messages"][3]["hidden"], json!(["U+3164", "U+E0065", "U+061C"]));
        // Without messages, no label.
        let plain = world.call("bo", "POST", "reports", ask(2, json!([]))).1["id"].clone();
        let shown = review(&mut world.db, &world.keys, "lena", world.now).expect("reviewed");
        let plain = shown.iter().find(|report| report["id"] == plain).expect("shown");
        assert_eq!(plain["evidence"].get("messages_label"), None);

        // Not with a request or a block; at most 50, oldest first, each
        // with its text up to 16 KiB, 64 KiB in all, from you or them.
        let on_request = json!({"action_id": action(3), "request_id": request, "reason": "spam", "messages": [said(1, "hi", "them")]});
        assert_eq!(world.call("bo", "POST", "reports", on_request).0, 400);
        let many: Vec<Value> = (1..=51).map(|seq| said(seq, "x", "them")).collect();
        let big = "x".repeat(MAX_MESSAGE_TEXT);
        let too_much: Vec<Value> = (1..=5).map(|seq| said(seq, &big, "them")).collect();
        let refused = [
            json!(many),
            json!([said(4, "a", "them"), said(3, "b", "them")]),
            json!([said(3, "a", "them"), said(3, "b", "them")]),
            json!([said(0, "a", "them")]),
            json!([said(1, "", "them")]),
            json!([said(1, &"x".repeat(MAX_MESSAGE_TEXT + 1), "them")]),
            json!(too_much),
            json!([said(1, "a", "ana")]),
            json!([{"seq": 1, "text": "a", "from": "them", "sent_ms": -1}]),
            json!([{"seq": 1, "text": "a", "from": "them", "sent_ms": 1, "verified": true}]),
        ];
        for (n, messages) in refused.into_iter().enumerate() {
            assert_eq!(world.call("bo", "POST", "reports", ask(10 + n as u32, messages.clone())).0, 400, "{messages}");
        }
        let fifty: Vec<Value> = (1..=50).map(|seq| said(seq, "x", "them")).collect();
        assert_eq!(world.call("bo", "POST", "reports", ask(30, json!(fifty))).0, 200);
    }

    #[test]
    fn a_person_makes_20_reports_a_day() {
        let mut world = World::new();
        let request = requested(&mut world);
        for n in 0..REPORTS_PER_DAY as u32 {
            let ask = json!({"action_id": action(n), "request_id": request, "reason": "spam"});
            assert_eq!(world.call("bo", "POST", "reports", ask).0, 200);
        }
        let ask = json!({"action_id": action(99), "request_id": request, "reason": "spam"});
        assert_eq!(world.call("bo", "POST", "reports", ask).0, 429);
    }
}
