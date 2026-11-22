//! Our own formats, pinned (tests/vectors/golden.json): what one version
//! wrote, every later one reads the same way. A change here is a format
//! change: a new version beside the old, never an edit.

use claktory_dm_crypto::content::{ByAgent, Content};
use claktory_dm_crypto::envelope::{Envelope, Kind};
use claktory_dm_crypto::storage::{SCHEMA, SealKey, SqlStorage};
use openmls::prelude::{GroupId, LeafNodeIndex};
use openmls_traits::storage::StorageProvider as _;
use rusqlite::Connection;
use serde_json::Value;

use crate::interop::hex;

fn golden() -> Value {
    let path = format!("{}/tests/vectors/golden.json", env!("CARGO_MANIFEST_DIR"));
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

fn h(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[test]
fn a_stored_value_reads_as_it_was_written() {
    let golden = &golden()["storage"];
    let db = Connection::open_in_memory().unwrap();
    db.pragma_update(None, "secure_delete", "ON").unwrap();
    db.execute_batch(SCHEMA).unwrap();
    db.execute(
        "INSERT INTO mls_values (key, grp, value) VALUES (?1, ?2, ?3)",
        rusqlite::params![
            hex(golden["key"].as_str().unwrap()),
            hex(golden["grp"].as_str().unwrap()),
            hex(golden["sealed"].as_str().unwrap())
        ],
    )
    .unwrap();
    let mut seal = [0u8; 32];
    seal.copy_from_slice(golden["seal_key"].as_str().unwrap().as_bytes());
    let seal = SealKey::from_bytes(seal);
    let storage = SqlStorage::new(&db, &seal).unwrap();
    let group = GroupId::from_slice(golden["group_id"].as_str().unwrap().as_bytes());
    let index: Option<LeafNodeIndex> = storage.own_leaf_index(&group).unwrap();
    assert_eq!(index.map(|index| index.u32()), Some(golden["own_leaf_index"].as_u64().unwrap() as u32));
}

#[test]
fn contents_encode_as_they_did() {
    for case in golden()["content"].as_array().unwrap() {
        let mut content = Content::text(case["client_message_id"].as_str().unwrap(), case["text"].as_str().unwrap());
        if let Some(agent) = case.get("by_agent") {
            content.by_agent = Some(serde_json::from_value::<ByAgent>(agent.clone()).unwrap());
        }
        let encoded = content.encode().unwrap();
        let expected = case["encoded"].as_str().unwrap();
        assert_eq!(String::from_utf8(encoded.clone()).unwrap(), expected);
        assert_eq!(Content::decode(expected.as_bytes()).unwrap(), content);
    }
}

#[test]
fn envelopes_read_as_they_did() {
    let golden = &golden()["envelope"];
    let envelope = Envelope::from_json(&golden["json"]).unwrap();
    assert_eq!(envelope.parsed().kind, Kind::KeyPackage);
    assert_eq!(h(&envelope.digest()), golden["digest"].as_str().unwrap());
    assert_eq!(envelope.to_json(), golden["json"]);
}
