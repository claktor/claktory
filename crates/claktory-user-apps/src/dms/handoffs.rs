//! Device hand-offs (E2; dms-e2ee.md §2): a device of the person's that
//! holds the identity key seals it for a new one (`handoff`, from the QR
//! the new device shows) and passes it through the worker, which keeps it
//! sealed, for that one device, for 30 minutes at most.

use rusqlite::{Connection, Transaction, TransactionBehavior, params};
use serde::Deserialize;
use serde_json::{Value, json};

use super::identity::{Refusal, body, invalid, take};
use super::leaves::{b64, keys_of, unb64};
use super::sync::{self, Kind};
use crate::ipc::Caller;
use claktory_dm_crypto::certificate::LeafKind;

pub const SCHEMA: &str = "
-- Hand-offs waiting for their device: sealed by the sender for it alone.
CREATE TABLE IF NOT EXISTS dm_handoffs (
    id TEXT PRIMARY KEY,
    account_id TEXT NOT NULL,
    from_key TEXT NOT NULL,
    to_key TEXT NOT NULL,
    sealed BLOB NOT NULL,
    accepted INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS dm_handoffs_to ON dm_handoffs (account_id, to_key);
CREATE INDEX IF NOT EXISTS dm_handoffs_accepted ON dm_handoffs (accepted);
";

/// How long a hand-off waits: as long as the QR's one-time key lives.
pub const KEEP_MS: i64 = 30 * 60_000;
/// The most a sealed hand-off may be, so it fits a request in base64.
pub const SEALED_MOST: usize = 512 * 1024;
/// Hand-offs waiting for one device at most, and sent an hour per device.
const WAITING_MOST: i64 = 3;
const SENT_AN_HOUR: i64 = 10;

fn not_found() -> Refusal {
    Refusal(404, "not_found", "There's nothing here.".to_owned())
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Send {
    to: String,
    sealed: String,
    #[serde(default)]
    #[allow(dead_code)]
    request_nonce: Option<String>,
}

fn send(transaction: &Transaction<'_>, account: &str, from: &str, removed: &[String], send: Send, now_ms: i64) -> Result<Value, Refusal> {
    let sealed = unb64(&send.sealed, "sealed")?;
    if sealed.is_empty() || sealed.len() > SEALED_MOST {
        return Err(invalid(&format!("A hand-off is at most {} KiB sealed.", SEALED_MOST / 1024)));
    }
    if send.to == from {
        return Err(invalid("A device hands off to another device."));
    }
    // Never to a device removed from the account, even one a list still
    // names (row 612): it may be in a thief's hands.
    if removed.contains(&send.to) {
        return Err(Refusal(409, "removed_device", "That device was removed from your account, so nothing is handed off to it.".to_owned()));
    }
    let Some(keys) = keys_of(transaction, account)? else {
        return Err(Refusal(409, "not_set_up", "Set up messages on one of your devices first.".to_owned()));
    };
    if !keys.list.list.leaves.iter().any(|leaf| leaf.kind == LeafKind::Device && leaf.leaf_id == send.to) {
        return Err(not_found());
    }
    take(transaction, &format!("handoff:{from}"), 3600, SENT_AN_HOUR, now_ms)?;
    let waiting: i64 = transaction.query_row(
        "SELECT COUNT(*) FROM dm_handoffs WHERE account_id = ?1 AND to_key = ?2 AND accepted > ?3",
        params![account, send.to, now_ms - KEEP_MS],
        |row| row.get(0),
    )?;
    if waiting >= WAITING_MOST {
        return Err(Refusal(429, "rate_limited", "That device has hand-offs waiting already. Try again in a few minutes.".to_owned()));
    }
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes).expect("the system's random source");
    let id = b64(&bytes);
    transaction.execute(
        "INSERT INTO dm_handoffs (id, account_id, from_key, to_key, sealed, accepted) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![id, account, from, send.to, sealed, now_ms],
    )?;
    sync::emit(transaction, account, Kind::Handoff, "", &id, 0, now_ms)?;
    Ok(json!({"id": id}))
}

fn waiting(db: &Connection, account: &str, to: &str, now_ms: i64) -> Result<Value, Refusal> {
    let mut statement = db.prepare(
        "SELECT id, from_key, sealed, accepted FROM dm_handoffs WHERE account_id = ?1 AND to_key = ?2 AND accepted > ?3
         ORDER BY accepted, id",
    )?;
    let handoffs: Vec<Value> = statement
        .query_map(params![account, to, now_ms - KEEP_MS], |row| {
            let sealed: Vec<u8> = row.get(2)?;
            Ok(json!({"id": row.get::<_, String>(0)?, "from": row.get::<_, String>(1)?, "sealed": b64(&sealed), "accepted_ms": row.get::<_, i64>(3)?}))
        })?
        .collect::<Result<_, _>>()?;
    Ok(json!({"handoffs": handoffs}))
}

/// Deletes hand-offs past their time.
pub fn sweep(db: &Connection, now_ms: i64) -> rusqlite::Result<usize> {
    db.execute("DELETE FROM dm_handoffs WHERE accepted <= ?1", [now_ms - KEEP_MS])
}

pub fn purge(transaction: &Transaction<'_>, account: &str) -> rusqlite::Result<()> {
    transaction.execute("DELETE FROM dm_handoffs WHERE account_id = ?1", [account])?;
    Ok(())
}

pub fn knows(method: &str, route: &str) -> bool {
    matches!((method, route), ("POST" | "GET", "me/handoffs"))
        || (method == "DELETE" && route.strip_prefix("me/handoffs/").is_some_and(|id| !id.is_empty() && !id.contains('/')))
}

pub fn answer(db: &mut Connection, method: &str, route: &str, caller: &Caller, bytes: &[u8], now_ms: i64) -> Result<(u16, Value), Refusal> {
    let Caller::Device { account_id, device_key, removed_devices, .. } = caller else {
        return Err(Refusal(403, "device_only", "Only your own signed-in devices can do this.".to_owned()));
    };
    let transaction = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let answer = match (method, route) {
        ("POST", "me/handoffs") => (200, send(&transaction, account_id, device_key, removed_devices, body(bytes)?, now_ms)?),
        ("GET", "me/handoffs") => (200, waiting(&transaction, account_id, device_key, now_ms)?),
        ("DELETE", route) => {
            let id = &route["me/handoffs/".len()..];
            let gone = transaction.execute(
                "DELETE FROM dm_handoffs WHERE id = ?1 AND account_id = ?2 AND to_key = ?3",
                params![id, account_id, device_key],
            )?;
            if gone == 0 {
                return Err(not_found());
            }
            (204, Value::Null)
        }
        _ => return Err(not_found()),
    };
    transaction.commit()?;
    Ok(answer)
}

#[cfg(test)]
mod tests {
    use super::super::contacts::tests::World;
    use super::super::leaves::tests::{Person, call, claktory, device, set_up};
    use super::*;

    /// Row 612: nothing is handed off to a device removed from the account,
    /// even while the list still names it.
    #[test]
    fn nothing_is_handed_off_to_a_removed_device() {
        let mut world = World::new();
        let mut ana = set_up(&mut world, "ana");
        ana.add(device("ana", "ana-mac"));
        assert_eq!(call(&mut world, &ana.caller(0), "PUT", "me/keys", ana.put(false)).0, 200);
        let to_mac = json!({"to": "ana-mac", "sealed": b64(&[7; 64])});
        let Caller::Device { removed_devices, .. } = &mut ana.leaves[0].0 else { unreachable!() };
        removed_devices.push("ana-mac".into());
        let (status, refused) = call(&mut world, &ana.caller(0), "POST", "me/handoffs", to_mac);
        assert_eq!((status, &refused["error"]), (409, &json!("removed_device")), "{refused}");
        let waiting: i64 = world.db.query_row("SELECT COUNT(*) FROM dm_handoffs", [], |row| row.get(0)).expect("counted");
        assert_eq!(waiting, 0);
    }

    /// A device of the account hands off to a device on its list, which
    /// alone reads it, for 30 minutes; a few at a time.
    #[test]
    fn a_hand_off_reaches_only_its_listed_device_for_half_an_hour() {
        let mut world = World::new();
        let mut ana = set_up(&mut world, "ana");
        let phone = ana.caller(0);
        let sealed = b64(&[7; 64]);
        let to_mac = json!({"to": "ana-mac", "sealed": sealed});

        // Not before the Mac is on the list; not to itself; not too big;
        // not from a Claktory or another account.
        assert_eq!(call(&mut world, &phone, "POST", "me/handoffs", to_mac.clone()).0, 404);
        ana.add(device("ana", "ana-mac"));
        assert_eq!(call(&mut world, &phone, "PUT", "me/keys", ana.put(false)).0, 200);
        assert_eq!(call(&mut world, &phone, "POST", "me/handoffs", json!({"to": "ana-phone", "sealed": sealed})).0, 400);
        let big = json!({"to": "ana-mac", "sealed": b64(&vec![0; SEALED_MOST + 1])});
        assert_eq!(call(&mut world, &phone, "POST", "me/handoffs", big).0, 400);
        assert_eq!(call(&mut world, &claktory("ana", "mac"), "POST", "me/handoffs", to_mac.clone()).0, 403);
        assert_eq!(call(&mut world, &device("bo", "bo-phone"), "POST", "me/handoffs", to_mac.clone()).0, 409, "Bo has no keys");
        let mut cy = Person::new("cy");
        cy.add(device("cy", "cy-phone"));
        assert_eq!(call(&mut world, &cy.caller(0), "POST", "me/handoffs", to_mac.clone()).0, 409);

        let (status, sent) = call(&mut world, &phone, "POST", "me/handoffs", to_mac.clone());
        assert_eq!(status, 200, "{sent}");
        let id = sent["id"].as_str().expect("an id").to_owned();
        let changes = sync::changes(&world.db, "ana", 0, 100).ok().expect("changes");
        assert!(changes["events"].as_array().expect("events").contains(&json!({"kind": "handoff", "id": id})));

        // The Mac reads it; the phone doesn't, nor Bo's device of the same key.
        let mac = device("ana", "ana-mac");
        let (_, waiting) = call(&mut world, &mac, "GET", "me/handoffs", Value::Null);
        assert_eq!(waiting, json!({"handoffs": [{"id": id, "from": "ana-phone", "sealed": sealed, "accepted_ms": world.now}]}));
        assert_eq!(call(&mut world, &phone, "GET", "me/handoffs", Value::Null).1, json!({"handoffs": []}));
        assert_eq!(call(&mut world, &device("bo", "ana-mac"), "GET", "me/handoffs", Value::Null).1, json!({"handoffs": []}));
        assert_eq!(call(&mut world, &phone, "DELETE", &format!("me/handoffs/{id}"), Value::Null).0, 404);
        assert_eq!(call(&mut world, &mac, "DELETE", &format!("me/handoffs/{id}"), Value::Null).0, 204);

        // Three waiting at most; gone after 30 minutes.
        for _ in 0..WAITING_MOST {
            assert_eq!(call(&mut world, &phone, "POST", "me/handoffs", to_mac.clone()).0, 200);
        }
        assert_eq!(call(&mut world, &phone, "POST", "me/handoffs", to_mac.clone()).0, 429);
        world.now += KEEP_MS;
        assert_eq!(call(&mut world, &mac, "GET", "me/handoffs", Value::Null).1, json!({"handoffs": []}));
        assert_eq!(sweep(&world.db, world.now).expect("swept"), 3);

        // Ten an hour from one device.
        for _ in 4..SENT_AN_HOUR {
            assert_eq!(call(&mut world, &phone, "POST", "me/handoffs", to_mac.clone()).0, 200);
            world.db.execute("DELETE FROM dm_handoffs", []).expect("cleared");
        }
        assert_eq!(call(&mut world, &phone, "POST", "me/handoffs", to_mac).1["error"], "rate_limited");
    }
}
