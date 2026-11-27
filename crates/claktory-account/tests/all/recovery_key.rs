//! The account's recovery key: a signed-in Claktory sets its public half, every key needs a
//! device's approval, the first too, and replaces only the key it names, and every Claktory of the
//! account gets it, with the chain before it, at sign-in and check-in.

use claktory_account::protocol::*;
use serde_json::json;

use crate::support::*;
use crate::web::{field, get, post_form, web_sign_in};

const EMAIL: &str = "matt@example.com";
const PATH: &str = "/v1/account/recovery-key";

/// A recovery key's public half, made up.
fn public_key() -> String {
    b64(*random_32())
}

/// `claktory`'s statement that `public_key` replaces `replaces`, approved
/// by `device` when given.
fn statement(
    instance: &Instance,
    claktory: &Claktory,
    account_id: &str,
    public_key: &str,
    replaces: &str,
    device: Option<&Device>,
) -> SetRecoveryKey {
    let said = RecoveryKeyStatement {
        v: 1,
        account_id: account_id.to_owned(),
        claktory_id: claktory.id.clone(),
        public_key: public_key.to_owned(),
        replaces: replaces.to_owned(),
        device_key: device.map(Device::account_key).unwrap_or_default(),
        iat: instance.now(),
    };
    let statement = sign_recovery_key(&claktory.key, &said);
    let device_sig = device.map(|device| sign_recovery_key_device(&device.key, &statement)).unwrap_or_default();
    SetRecoveryKey { statement, device_sig }
}

async fn put(instance: &Instance, claktory: &Claktory, set: &SetRecoveryKey) -> Answer {
    instance.advance(1);
    instance.signed(claktory, "PUT", PATH, set, None).await
}

async fn set(instance: &Instance, claktory: &Claktory, set: &SetRecoveryKey) -> RecoveryKey {
    let answer = put(instance, claktory, set).await;
    assert_eq!(answer.status, 200, "{}", answer.text());
    answer.json::<RecoveryKeySet>().recovery_key.expect("set")
}

async fn checked_in(instance: &Instance, claktory: &Claktory) -> Option<RecoveryKey> {
    instance.advance(1);
    let check_in = CheckIn { version: "v0.1.25".into(), platform: "macos-arm64".into(), ..Default::default() };
    let answer = instance.signed(claktory, "POST", "/v1/check-in", &check_in, None).await;
    assert_eq!(answer.status, 200, "{}", answer.text());
    answer.json::<CheckedIn>().recovery_key
}

#[tokio::test]
async fn the_first_claktory_sets_it_and_every_claktory_of_the_account_gets_it() {
    let instance = start().await;
    let mut studio = Claktory::new("198.51.100.30");
    let account = instance.sign_in(&mut studio, EMAIL, "Mac mini").await.account_id;
    assert_eq!(instance.sign_in(&mut Claktory::new("198.51.100.31"), "eve@example.com", "Eve's").await.recovery_key, None);
    assert_eq!(checked_in(&instance, &studio).await, None, "none yet");

    // The first key needs a device's approval too, so one
    // Claktory's backend key alone can't plant it.
    let first = public_key();
    let alone = statement(&instance, &studio, &account, &first, "", None);
    assert_eq!(put(&instance, &studio, &alone).await.error(), "invalid_request");
    assert_eq!(checked_in(&instance, &studio).await, None, "still none");
    let phone = Device::new("192.0.2.80");
    let made = statement(&instance, &studio, &account, &first, "", Some(&phone));
    let set = set(&instance, &studio, &made).await;
    assert_eq!(
        (set.public_key.as_str(), set.claktory_id.as_str(), set.claktory_name.as_str()),
        (first.as_str(), studio.id.as_str(), "Mac mini")
    );
    assert!(verify_recovery_key_device(&set.statement, &set.device_sig));
    assert_eq!(set.backend_key, b64(studio.key.verifying_key().to_bytes()));
    assert!(set.history.is_empty());
    // Its statement checks with the backend key the answer gives.
    let key = ed25519_dalek::VerifyingKey::from_bytes(&unb64_32(&set.backend_key).unwrap()).unwrap();
    let said = verify_recovery_key(&key, &set.statement).expect("checks");
    assert_eq!((said.public_key.as_str(), said.replaces.as_str()), (first.as_str(), ""));
    // Sent again (its answer lost): the same.
    assert_eq!(put(&instance, &studio, &made).await.json::<RecoveryKeySet>().recovery_key, Some(set.clone()));

    // Every Claktory of the account gets it: at check-in, and at sign-in.
    assert_eq!(checked_in(&instance, &studio).await, Some(set.clone()));
    let mut server = Claktory::new("203.0.113.9");
    assert_eq!(instance.sign_in(&mut server, EMAIL, "hetzner").await.recovery_key, Some(set.clone()));
    assert_eq!(checked_in(&instance, &server).await, Some(set));
    // Another account's Claktory doesn't.
    let mut eve = Claktory::new("198.51.100.32");
    instance.sign_in(&mut eve, "eve@example.com", "Eve's").await;
    assert_eq!(checked_in(&instance, &eve).await, None);
}

#[tokio::test]
async fn a_new_key_needs_a_devices_approval_and_replaces_only_the_key_it_names() {
    let instance = start().await;
    let mut studio = Claktory::new("198.51.100.30");
    let account = instance.sign_in(&mut studio, EMAIL, "Mac mini").await.account_id;
    let mut server = Claktory::new("203.0.113.9");
    instance.sign_in(&mut server, EMAIL, "hetzner").await;
    let phone = Device::new("192.0.2.80");
    let (first, second) = (public_key(), public_key());
    set(&instance, &studio, &statement(&instance, &studio, &account, &first, "", Some(&phone))).await;

    // Whoever holds one Claktory's backend key can't change it
    // alone; a device's signature that doesn't check is refused too.
    let alone = statement(&instance, &server, &account, &second, &first, None);
    assert_eq!(put(&instance, &server, &alone).await.error(), "invalid_request");
    let mut forged = statement(&instance, &server, &account, &second, &first, Some(&phone));
    forged.device_sig = sign_recovery_key_device(&Device::new("192.0.2.81").key, &forged.statement);
    assert_eq!(put(&instance, &server, &forged).await.error(), "invalid_request");
    assert_eq!(checked_in(&instance, &studio).await.map(|key| key.public_key), Some(first.clone()));

    // Approved on the phone: the new key, the first behind it.
    let changed = set(&instance, &server, &statement(&instance, &server, &account, &second, &first, Some(&phone))).await;
    assert_eq!((changed.public_key.as_str(), changed.claktory_id.as_str()), (second.as_str(), server.id.as_str()));
    assert_eq!(changed.claktory_name, "hetzner", "named as the account lists it");
    assert!(verify_recovery_key_device(&changed.statement, &changed.device_sig));
    assert_eq!(changed.history.iter().map(|key| key.public_key.as_str()).collect::<Vec<_>>(), [first.as_str()]);
    assert_eq!(checked_in(&instance, &studio).await, Some(changed.clone()));

    // Made on what the Claktory last saw: replacing the first again isn't
    // undoing the second.
    let stale = statement(&instance, &studio, &account, &public_key(), &first, Some(&phone));
    let answer = put(&instance, &studio, &stale).await;
    assert_eq!((answer.status, answer.error().as_str()), (409, "recovery_key_changed"), "{}", answer.text());
    assert_eq!(answer.json::<ErrorBody>().recovery_key.map(|key| key.public_key), Some(second.clone()));
    let again = statement(&instance, &studio, &account, &public_key(), "", Some(&phone));
    assert_eq!(put(&instance, &studio, &again).await.error(), "recovery_key_changed", "the first only while it has none");

    // What doesn't check.
    let other_account = statement(&instance, &studio, "someone-else", &public_key(), &second, Some(&phone));
    assert_eq!(put(&instance, &studio, &other_account).await.error(), "invalid_request");
    let not_its_own = statement(&instance, &server, &account, &public_key(), &second, Some(&phone));
    assert_eq!(put(&instance, &studio, &not_its_own).await.error(), "invalid_request", "signed by another Claktory");
    let zero = statement(&instance, &studio, &account, &b64([0; 32]), &second, Some(&phone));
    assert_eq!(put(&instance, &studio, &zero).await.error(), "invalid_request");
    let short = statement(&instance, &studio, &account, &b64([7; 16]), &second, Some(&phone));
    assert_eq!(put(&instance, &studio, &short).await.error(), "invalid_request");
    let old = statement(&instance, &studio, &account, &public_key(), &second, Some(&phone));
    instance.advance(301);
    assert_eq!(put(&instance, &studio, &old).await.error(), "invalid_request", "out of time");

    // Signed out, it sets nothing.
    instance.advance(1);
    assert_eq!(instance.signed(&server, "POST", "/v1/claktories/sign-out", &json!({}), None).await.status, 200);
    let gone = statement(&instance, &server, &account, &public_key(), &second, Some(&phone));
    assert_eq!(put(&instance, &server, &gone).await.status, 401);
    let kept = checked_in(&instance, &studio).await.expect("it stays with the account");
    assert_eq!(kept.public_key, second);
    assert_eq!(kept.claktory_name, "", "its Claktory is no longer the account's");
    assert_eq!(kept.history[0].claktory_name, "Mac mini");
}

#[tokio::test]
async fn the_chain_keeps_the_last_ten_and_ten_changes_a_day_at_most() {
    let instance = start().await;
    let mut studio = Claktory::new("198.51.100.30");
    let account = instance.sign_in(&mut studio, EMAIL, "Mac mini").await.account_id;
    let phone = Device::new("192.0.2.80");
    let mut keys = vec![public_key()];
    set(&instance, &studio, &statement(&instance, &studio, &account, &keys[0], "", Some(&phone))).await;
    for _ in 0..9 {
        let next = public_key();
        set(&instance, &studio, &statement(&instance, &studio, &account, &next, keys.last().unwrap(), Some(&phone))).await;
        keys.push(next);
    }
    let eleventh = statement(&instance, &studio, &account, &public_key(), keys.last().unwrap(), Some(&phone));
    assert_eq!(put(&instance, &studio, &eleventh).await.status, 429);
    instance.advance(86_400);
    for _ in 0..2 {
        let next = public_key();
        set(&instance, &studio, &statement(&instance, &studio, &account, &next, keys.last().unwrap(), Some(&phone))).await;
        keys.push(next);
    }
    let now = checked_in(&instance, &studio).await.expect("its key");
    assert_eq!(now.public_key, keys[11]);
    let chain: Vec<&str> = now.history.iter().map(|key| key.public_key.as_str()).collect();
    let expected: Vec<&str> = keys[2..11].iter().rev().map(String::as_str).collect();
    assert_eq!(chain, expected, "the 9 before it, newest first");
    assert!(now.history.iter().all(|key| key.history.is_empty()));
}

#[tokio::test]
async fn it_goes_with_the_account() {
    let instance = start().await;
    let mut studio = Claktory::new("198.51.100.30");
    let account = instance.sign_in(&mut studio, EMAIL, "Mac mini").await.account_id;
    let phone = Device::new("192.0.2.80");
    set(&instance, &studio, &statement(&instance, &studio, &account, &public_key(), "", Some(&phone))).await;
    let cookie = web_sign_in(&instance, EMAIL, "198.51.100.62").await;
    let csrf = field(&get(&instance, "/account", &cookie).await.text(), "csrf");
    let answer = post_form(&instance, "/account/delete", &cookie, &format!("csrf={csrf}&confirm=yes")).await;
    assert!(answer.text().contains("Your account was deleted."), "{}", answer.text());
    let conn = rusqlite::Connection::open(&instance.db).expect("the database");
    let kept: i64 = conn.query_row("SELECT count(*) FROM recovery_keys", [], |row| row.get(0)).unwrap();
    assert_eq!(kept, 0);
    // Signed in again: a new account, with none.
    instance.advance(60);
    let mut again = Claktory::new("198.51.100.30");
    assert_eq!(instance.sign_in(&mut again, EMAIL, "Mac mini").await.recovery_key, None);
}
