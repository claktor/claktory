//! Golden vectors for Ghiaccio's formats: a leaf certificate, a signed
//! device list and a safety number, from fixed keys. Every endpoint (the
//! app, factoryd, the worker) reads these bytes, so a change to any of them
//! is a format change and must be deliberate: this fails until the vectors
//! are written again with `CLAKTORY_DM_WRITE_VECTORS=1`, on purpose.

use claktory_dm_crypto::certificate::{Leaf, LeafCertificate, LeafKind, claktory_leaf_id};
use claktory_dm_crypto::device_list::{DeviceList, Listed, SignedDeviceList};
use claktory_dm_crypto::identity::IdentityKey;
use claktory_dm_crypto::safety::SafetyNumber;

const FILE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/vectors/formats.json");

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[test]
fn certificates_device_lists_and_safety_numbers_keep_their_bytes() {
    let matt = IdentityKey::from_secret(&[1; 32]);
    let ana = IdentityKey::from_secret(&[2; 32]);
    let phone = Leaf {
        kind: LeafKind::Device,
        account_id: "acct-matt".to_owned(),
        leaf_id: "dev-phone".to_owned(),
        signature_key: [3; 32],
        name: "Phone".to_owned(),
        created_ms: 1_763_700_000_000,
    };
    let mini = Leaf {
        kind: LeafKind::Claktory,
        account_id: "acct-matt".to_owned(),
        leaf_id: claktory_leaf_id("mac-mini", 2),
        signature_key: [4; 32],
        name: "mac-mini".to_owned(),
        created_ms: 1_763_700_000_001,
    };
    let certificate = LeafCertificate::issue(&matt, phone.clone()).unwrap().to_bytes().unwrap();
    let claktory = LeafCertificate::issue(&matt, mini.clone()).unwrap().to_bytes().unwrap();
    let list = DeviceList {
        account_id: "acct-matt".to_owned(),
        version: 7,
        created_ms: 1_763_700_000_002,
        leaves: [&phone, &mini]
            .iter()
            .map(|leaf| Listed { kind: leaf.kind, leaf_id: leaf.leaf_id.clone(), signature_key: leaf.signature_key })
            .collect(),
    }
    .sign(&matt)
    .unwrap()
    .to_bytes()
    .unwrap();
    let safety = SafetyNumber::new((&matt.public(), "acct-matt"), (&ana.public(), "acct-ana"));
    let now = serde_json::json!({
        "identity_public": hex(&matt.public().0),
        "leaf_certificate": hex(&certificate),
        "claktory_certificate": hex(&claktory),
        "device_list": hex(&list),
        "safety_digits": safety.digits(),
        "safety_qr": hex(&safety.qr()),
    });
    if std::env::var_os("CLAKTORY_DM_WRITE_VECTORS").is_some() {
        std::fs::create_dir_all(std::path::Path::new(FILE).parent().unwrap()).unwrap();
        std::fs::write(FILE, serde_json::to_string_pretty(&now).unwrap() + "\n").unwrap();
    }
    let kept: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(FILE).expect("the vectors")).unwrap();
    assert_eq!(now, kept, "a format changed; if on purpose, write the vectors again");

    // And they read back.
    let unhex = |text: &str| (0..text.len()).step_by(2).map(|at| u8::from_str_radix(&text[at..at + 2], 16).unwrap()).collect::<Vec<u8>>();
    assert_eq!(LeafCertificate::from_bytes(&unhex(kept["leaf_certificate"].as_str().unwrap())).unwrap().leaf, phone);
    assert_eq!(SignedDeviceList::from_bytes(&unhex(kept["device_list"].as_str().unwrap())).unwrap().list.version, 7);
}
