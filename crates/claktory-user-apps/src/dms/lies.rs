//! Lies, for QA only: a worker that answers the person's devices falsely
//! on purpose, so their checks can be seen to hold against a worker that
//! lies. Built only with the `qa-lies` feature, which a release build
//! refuses (below), so no deployed worker can carry it. Each lie is off
//! unless its variable is set in the worker's environment:
//!
//! - `QA_LIE_D15_STRANGER=<account>`: `GET conversations` and `GET sync`
//!   list one more conversation, with that account (a stranger) as the
//!   friend. `QA_LIE_D15_MODE=swap` names the stranger in the caller's
//!   newest conversation instead.
//! - `QA_LIE_D16_KEYS_FILE=<file>`: `GET keys/<account>` answers what the
//!   file holds, an earlier `GET keys/<account>` answer saved before the
//!   list changed (its `account_id` says whose).
//! - `QA_LIE_D17=<account>`: `GET keys/<account>` answers a fresh identity
//!   key (one per worker run) and the account's list signed by it, one
//!   version newer, so it's well formed and its signature checks.
//! - `QA_LIE_D18=<group id>` (base64url, or `*` for every group):
//!   `GET groups/<id>/group-info` answers another group's GroupInfo and
//!   epoch: `QA_LIE_D18_SERVE=<group id>`'s, or the newest other group's.
//! - `QA_LIE_ONLY_FOR=<account>`: lie only to that account's callers (the
//!   others, Bo's own devices say, get the truth).

#[cfg(not(debug_assertions))]
compile_error!("The qa-lies feature is for QA builds only: a release build never carries it.");

use std::sync::OnceLock;

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use claktory_dm_crypto::device_list::SignedDeviceList;
use claktory_dm_crypto::envelope::Envelope;
use claktory_dm_crypto::identity::IdentityKey;
use rusqlite::{Connection, OptionalExtension as _};
use serde_json::{Value, json};
use sha2::{Digest as _, Sha256};

use crate::ipc::Caller;

/// The lies that are on.
#[derive(Debug, Default, Clone)]
struct Lies {
    only_for: Option<String>,
    stranger: Option<String>,
    swap: bool,
    older_keys: Option<String>,
    fresh_key: Option<String>,
    other_group: Option<String>,
    serve_group: Option<String>,
}

impl Lies {
    fn from_env() -> Lies {
        let var = |name: &str| std::env::var(name).ok().filter(|value| !value.is_empty());
        Lies {
            only_for: var("QA_LIE_ONLY_FOR"),
            stranger: var("QA_LIE_D15_STRANGER"),
            swap: var("QA_LIE_D15_MODE").as_deref() == Some("swap"),
            older_keys: var("QA_LIE_D16_KEYS_FILE"),
            fresh_key: var("QA_LIE_D17"),
            other_group: var("QA_LIE_D18"),
            serve_group: var("QA_LIE_D18_SERVE"),
        }
    }
}

/// The answer to `method` `route` for `caller`, after the lies that are on.
pub fn lie(db: &Connection, method: &str, route: &str, caller: &Caller, answered: Option<(u16, Value)>) -> Option<(u16, Value)> {
    told(&Lies::from_env(), db, method, route, caller, answered)
}

fn told(lies: &Lies, db: &Connection, method: &str, route: &str, caller: &Caller, answered: Option<(u16, Value)>) -> Option<(u16, Value)> {
    let Some((200, answer)) = answered else { return answered };
    if method != "GET" || lies.only_for.as_ref().is_some_and(|only| only != caller.account_id()) {
        return Some((200, answer));
    }
    let (path, _) = route.split_once('?').unwrap_or((route, ""));
    let parts: Vec<&str> = path.split('/').collect();
    let lied = match parts.as_slice() {
        ["conversations"] | ["sync"] => stranger(lies, answer),
        ["keys", account] => older_list(lies, account).or_else(|| fresh_key(lies, account, &answer)).unwrap_or(answer),
        ["groups", id, "group-info"] => other_group(lies, db, id).unwrap_or(answer),
        _ => answer,
    };
    Some((200, lied))
}

/// D15: a conversation naming a stranger.
fn stranger(lies: &Lies, mut answer: Value) -> Value {
    let Some(cy) = &lies.stranger else { return answer };
    let Some(listed) = answer.get_mut("conversations").and_then(Value::as_array_mut) else { return answer };
    tracing::warn!(stranger = %cy, "QA lie D15: a conversation names a stranger");
    if lies.swap {
        if let Some(first) = listed.first_mut() {
            first["with"]["account_id"] = json!(cy);
        }
        return answer;
    }
    let id: String = Sha256::digest(format!("qa-lie d15 {cy}").as_bytes())[..16].iter().map(|byte| format!("{byte:02x}")).collect();
    let mut made =
        listed.first().cloned().unwrap_or_else(|| json!({"contact_generation": 1, "active": true, "since_ms": 0, "last_seq": 0}));
    made["id"] = json!(id);
    made["contact_id"] = json!(id);
    made["with"] = json!({"account_id": cy, "name": "Cy", "avatar_seed": cy});
    made["view"] = Value::Null;
    listed.push(made);
    answer
}

/// D16: an earlier answer for the account, saved to a file.
fn older_list(lies: &Lies, account: &str) -> Option<Value> {
    let file = lies.older_keys.as_ref()?;
    let saved: Value = match std::fs::read(file).map(|bytes| serde_json::from_slice(&bytes)) {
        Ok(Ok(saved)) => saved,
        _ => {
            tracing::error!(%file, "QA lie D16: the saved keys answer doesn't read");
            return None;
        }
    };
    (saved["account_id"] == account).then(|| {
        tracing::warn!(%account, "QA lie D16: an older device list");
        saved
    })
}

/// D17: a fresh identity key for the account, its list signed by it.
fn fresh_key(lies: &Lies, account: &str, answer: &Value) -> Option<Value> {
    if lies.fresh_key.as_deref() != Some(account) {
        return None;
    }
    static FRESH: OnceLock<IdentityKey> = OnceLock::new();
    let fresh = FRESH.get_or_init(|| IdentityKey::generate().expect("an identity key"));
    let bytes = URL_SAFE_NO_PAD.decode(answer["device_list"].as_str()?).ok()?;
    let signed = SignedDeviceList::from_bytes(&bytes).ok()?;
    let mut list = signed.list;
    list.version += 1;
    let lied = list.sign_with(fresh, signed.home).ok()?;
    tracing::warn!(%account, "QA lie D17: another identity key");
    let mut answer = answer.clone();
    answer["identity_key"] = json!(URL_SAFE_NO_PAD.encode(fresh.public().0));
    answer["device_list"] = json!(URL_SAFE_NO_PAD.encode(lied.to_bytes().ok()?));
    answer["version"] = json!(lied.list.version);
    Some(answer)
}

/// D18: another group's GroupInfo.
fn other_group(lies: &Lies, db: &Connection, id: &str) -> Option<Value> {
    let target = lies.other_group.as_deref()?;
    if target != "*" && target != id {
        return None;
    }
    let asked = URL_SAFE_NO_PAD.decode(id).ok()?;
    let row = |row: &rusqlite::Row<'_>| Ok((row.get::<_, i64>(0)?, row.get::<_, Vec<u8>>(1)?));
    let served = match &lies.serve_group {
        Some(serve) => {
            let serve = URL_SAFE_NO_PAD.decode(serve).ok()?;
            db.query_row("SELECT epoch, group_info FROM dm_groups WHERE group_id = ?1 AND group_id != ?2", [serve, asked], row)
        }
        None => db.query_row("SELECT epoch, group_info FROM dm_groups WHERE group_id != ?1 ORDER BY created DESC LIMIT 1", [asked], row),
    };
    let (epoch, info) = served.optional().ok()??;
    tracing::warn!(group = %id, "QA lie D18: another group's GroupInfo");
    Some(json!({"epoch": epoch, "group_info": Envelope::new(info).ok()?.to_json()}))
}

#[cfg(test)]
mod tests {
    use claktory_dm_crypto::group::Group as Mls;
    use claktory_dm_crypto::validation::GroupKind as Says;

    use super::super::contacts::tests::World;
    use super::super::leaves::b64;
    use super::super::leaves::tests::{Person, call, set_up};
    use super::super::messages::tests::friends;
    use super::*;

    fn ask(lies: &Lies, world: &mut World, caller: &Caller, route: &str) -> Value {
        let truth = call(world, caller, "GET", route, Value::Null);
        assert_eq!(truth.0, 200, "{}", truth.1);
        told(lies, &world.db, "GET", route, caller, Some(truth)).expect("an answer").1
    }

    fn group(world: &mut World, person: &Person, says: Says, id: [u8; 16], mut body: Value) {
        let info = person.with(0, |provider, me| {
            let group = Mls::create(provider, me, says, &id).expect("made");
            group.group_info(provider, me).expect("a GroupInfo")
        });
        body["group_info"] = Envelope::new(info).expect("an envelope").to_json();
        assert_eq!(call(world, &person.caller(0), "POST", "groups", body).0, 200);
    }

    #[test]
    fn d15_a_conversation_names_a_stranger() {
        let answer = json!({"conversations": [{"id": "c1", "with": {"account_id": "bo", "name": "Bo"}, "view": {}}]});
        let mut lies = Lies { stranger: Some("cy".into()), ..Lies::default() };
        let lied = stranger(&lies, answer.clone());
        let listed = lied["conversations"].as_array().expect("listed");
        assert_eq!((listed.len(), &listed[0]["with"]["account_id"], &listed[1]["with"]["account_id"]), (2, &json!("bo"), &json!("cy")));
        assert_ne!(listed[1]["id"], "c1");
        lies.swap = true;
        let swapped = stranger(&lies, answer.clone());
        assert_eq!(
            (swapped["conversations"][0]["id"].as_str(), swapped["conversations"][0]["with"]["account_id"].as_str()),
            (Some("c1"), Some("cy"))
        );
        assert_eq!(stranger(&Lies::default(), answer.clone()), answer, "off unless asked");
    }

    #[test]
    fn d16_and_d17_keys_that_lie_and_only_to_whom_they_should() {
        let mut world = World::new();
        friends(&mut world, "ana", "bo");
        let ana = set_up(&mut world, "ana");
        let bo = set_up(&mut world, "bo");
        let truth = ask(&Lies::default(), &mut world, &ana.caller(0), "keys/bo");

        // D17: another key, the list well formed and signed by it, newer.
        let lies = Lies { fresh_key: Some("bo".into()), ..Lies::default() };
        let lied = ask(&lies, &mut world, &ana.caller(0), "keys/bo");
        assert_ne!(lied["identity_key"], truth["identity_key"]);
        let list =
            SignedDeviceList::from_bytes(&URL_SAFE_NO_PAD.decode(lied["device_list"].as_str().unwrap()).unwrap()).expect("it checks");
        assert_eq!(b64(&list.identity.0), lied["identity_key"].as_str().unwrap());
        assert_eq!((list.list.account_id.as_str(), lied["version"].as_u64()), ("bo", Some(truth["version"].as_u64().unwrap() + 1)));
        assert_eq!(ask(&lies, &mut world, &ana.caller(0), "keys/ana"), ask(&Lies::default(), &mut world, &ana.caller(0), "keys/ana"));
        // Only to Ana's callers when asked: Bo's own device hears the truth.
        let lies = Lies { only_for: Some("ana".into()), ..lies };
        assert_eq!(ask(&lies, &mut world, &bo.caller(0), "keys/bo"), truth);

        // D16: what was saved before.
        let saved = tempfile::NamedTempFile::new().unwrap();
        let mut older = truth.clone();
        older["version"] = json!(0);
        std::fs::write(saved.path(), older.to_string()).unwrap();
        let lies = Lies { older_keys: Some(saved.path().display().to_string()), ..Lies::default() };
        assert_eq!(ask(&lies, &mut world, &ana.caller(0), "keys/bo"), older);
        assert_eq!(ask(&lies, &mut world, &ana.caller(0), "keys/ana")["account_id"], "ana", "only for the account it was saved for");
    }

    #[test]
    fn d18_another_groups_group_info() {
        let mut world = World::new();
        let ana = set_up(&mut world, "ana");
        let bo = set_up(&mut world, "bo");
        let (mine, other) = ([1; 16], [2; 16]);
        group(&mut world, &ana, Says::SelfGroup { account: "ana".into() }, mine, json!({"kind": "self"}));
        group(&mut world, &bo, Says::SelfGroup { account: "bo".into() }, other, json!({"kind": "self"}));
        let route = format!("groups/{}/group-info", b64(&mine));
        let truth = ask(&Lies::default(), &mut world, &ana.caller(0), &route);
        let theirs = ask(&Lies::default(), &mut world, &bo.caller(0), &format!("groups/{}/group-info", b64(&other)));
        assert_ne!(truth, theirs);
        for target in ["*".to_owned(), b64(&mine)] {
            let lies = Lies { other_group: Some(target), ..Lies::default() };
            assert_eq!(ask(&lies, &mut world, &ana.caller(0), &route), theirs);
        }
        let lies = Lies { other_group: Some(b64(&other)), ..Lies::default() };
        assert_eq!(ask(&lies, &mut world, &ana.caller(0), &route), truth, "only the group named");
        let lies = Lies { other_group: Some("*".into()), serve_group: Some(b64(&other)), ..Lies::default() };
        assert_eq!(ask(&lies, &mut world, &ana.caller(0), &route), theirs);
    }
}
