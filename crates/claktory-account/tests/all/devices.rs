//! A person's devices: signing in with an account key, finding the
//! account's Claktories, and assertions for the Claktories they connect to.

use claktory_account::protocol::*;
use claktory_account::server::db;
use serde_json::json;

use crate::support::*;
use crate::web::{field, get, post_form, web_sign_in};

fn relays(list: &[&str]) -> Option<Vec<String>> {
    Some(list.iter().map(|relay| (*relay).to_owned()).collect())
}

#[tokio::test]
async fn a_device_signs_in_with_its_key_and_finds_the_accounts_claktories() {
    let instance = start().await;
    let mut studio = Claktory::new("198.51.100.30");
    let signed_in = instance.sign_in(&mut studio, "matt@example.com", "Mac mini").await;
    let endpoints = Endpoints {
        backend: "A".repeat(64),
        devices: vec![],
        name: Some("Studio".into()),
        color: Some("blue".into()),
        relays: relays(&["https://relay-eu.claktory.com", "javascript:alert(1)", "https://relay-eu.claktory.com", "https://x.dev/\"a"]),
    };
    assert_eq!(instance.signed(&studio, "PUT", "/v1/endpoints", &endpoints, None).await.status, 200);
    instance.advance(60);
    let mut laptop = Claktory::new("198.51.100.31");
    instance.sign_in(&mut laptop, "matt@example.com", "MacBook").await;
    let mut removed = Claktory::new("198.51.100.32");
    instance.sign_in(&mut removed, "matt@example.com", "Old Mac").await;
    rusqlite::Connection::open(&instance.db).unwrap().execute("UPDATE claktories SET revoked_at = 1 WHERE id = ?1", [&removed.id]).unwrap();

    let phone = Device::new("192.0.2.80");
    let answer = instance.start_device_sign_in(&phone, "Matt@Example.com", "Matt's iPhone").await;
    assert_eq!(answer.status, 200, "{}", answer.text());
    let started: SignInStarted = answer.json();
    assert_eq!((started.expires_in, started.code_from.as_str()), (900, "code@auth.claktory.com"));
    let mail = instance.mail.sent().pop().expect("a code");
    assert_eq!(mail.to, "matt@example.com");
    assert!(mail.text.starts_with("Your code to sign in to Claktory on Matt's iPhone:"), "{}", mail.text);
    assert!(mail.html.contains("on Matt&#39;s iPhone"), "{}", mail.html);
    let verify = json!({"request_id": started.request_id, "code": instance.last_code("matt@example.com")});

    // Only the key that asked may use the code, and trying spends no tries.
    let thief = Device::new("192.0.2.81");
    let answer = instance.device_signed(&thief, "POST", "/v1/devices/sign-in/verify", Some(&verify), None).await;
    assert_eq!((answer.status, answer.error().as_str()), (401, "unauthorized"));
    assert_eq!(answer.value()["message"], "This device isn't signed in. Sign in again.");
    let answer = instance.device_signed(&phone, "GET", "/v1/devices/me", None, None).await;
    assert_eq!((answer.status, answer.error().as_str()), (401, "unauthorized"), "not signed in yet");
    let account: DeviceAccount = instance.device_signed(&phone, "POST", "/v1/devices/sign-in/verify", Some(&verify), None).await.json();
    let expected =
        DeviceAccount { account_id: signed_in.account_id.clone(), email: "matt@example.com".into(), name: "Matt's iPhone".into() };
    assert_eq!(account, expected);
    instance.advance(1);
    assert_eq!(instance.device_signed(&phone, "GET", "/v1/devices/me", None, None).await.json::<DeviceAccount>(), expected);
    let answer = instance.device_signed(&phone, "POST", "/v1/devices/sign-in/verify", Some(&verify), None).await;
    assert_eq!(answer.error(), "code_expired", "a code works once");

    // Newest contact first; the removed one isn't there.
    let list: ClaktoryList = instance.device_signed(&phone, "GET", "/v1/claktories", None, None).await.json();
    let laptop_listed = ListedClaktory {
        claktory_id: laptop.id.clone(),
        name: "MacBook".into(),
        color: String::new(),
        machine: "MacBook".into(),
        platform: "macos-arm64".into(),
        version: "v0.1.9".into(),
        endpoint_id: String::new(),
        relays: vec![],
        last_contact_ms: (START + 60) * 1000,
    };
    let studio_listed = ListedClaktory {
        claktory_id: studio.id.clone(),
        name: "Studio".into(),
        color: "blue".into(),
        machine: "Mac mini".into(),
        platform: "macos-arm64".into(),
        version: "v0.1.9".into(),
        endpoint_id: "a".repeat(64),
        relays: vec!["https://relay-eu.claktory.com".into()],
        last_contact_ms: START * 1000,
    };
    assert_eq!(list.claktories, vec![laptop_listed, studio_listed.clone()]);

    // A Claktory from before it sent its name keeps the one it has.
    instance.advance(1);
    let older = Endpoints { backend: "a".repeat(64), ..Default::default() };
    instance.signed(&studio, "PUT", "/v1/endpoints", &older, None).await;
    let list: ClaktoryList = instance.device_signed(&phone, "GET", "/v1/claktories", None, None).await.json();
    assert_eq!(list.claktories[0], ListedClaktory { last_contact_ms: instance.now() * 1000, ..studio_listed }, "in touch last");

    // Signed out, it's a stranger; signing out again is fine.
    instance.advance(1);
    assert_eq!(instance.device_signed(&phone, "POST", "/v1/devices/sign-out", Some(&json!({})), None).await.status, 200);
    assert_eq!(instance.device_signed(&phone, "GET", "/v1/devices/me", None, None).await.error(), "unauthorized");
    assert_eq!(instance.device_signed(&phone, "GET", "/v1/claktories", None, None).await.error(), "unauthorized");
    instance.advance(1);
    assert_eq!(instance.device_signed(&phone, "POST", "/v1/devices/sign-out", Some(&json!({})), None).await.status, 200);
}

#[tokio::test]
async fn a_device_signed_request_needs_its_key_the_right_time_and_once() {
    let instance = start().await;
    let phone = Device::new("192.0.2.82");
    instance.sign_in_device(&phone, "matt@example.com", "iPhone").await;

    let answer = instance.device_signed(&phone, "GET", "/v1/devices/me", None, Some(instance.now() - 301)).await;
    assert_eq!((answer.status, answer.error().as_str()), (401, "clock_skew"));
    assert_eq!(answer.value()["server_time"], instance.now());
    let time = instance.now();
    let signature = sign_request(&phone.key, "GET", "/v1/devices/me", time, b"");
    let headers = [
        ("authorization", format!("Device {}", phone.account_key())),
        (TIME_HEADER, time.to_string()),
        (SIGNATURE_HEADER, signature.clone()),
    ];
    assert_eq!(instance.send("GET", "/v1/devices/me", Vec::new(), &headers).await.status, 200);
    assert_eq!(instance.send("GET", "/v1/devices/me", Vec::new(), &headers).await.error(), "replayed");
    let answer = instance.send("GET", "/v1/claktories", Vec::new(), &headers).await;
    assert_eq!(answer.error(), "unauthorized", "signed for another path");
    let impostor = [
        ("authorization", format!("Device {}", Device::new("192.0.2.83").account_key())),
        (TIME_HEADER, time.to_string()),
        (SIGNATURE_HEADER, signature),
    ];
    assert_eq!(instance.send("GET", "/v1/devices/me", Vec::new(), &impostor).await.error(), "unauthorized", "another key");
    assert_eq!(instance.send("GET", "/v1/devices/me", Vec::new(), &[]).await.error(), "unauthorized", "no headers");

    let bad = DeviceSignInStart {
        email: "matt@example.com".into(),
        account_key: "nope".into(),
        name: "iPhone".into(),
        platform: "ios".into(),
        version: "v0.1.13".into(),
    };
    assert_eq!(instance.post("/v1/devices/sign-in/start", &bad, "192.0.2.82").await.error(), "invalid_request");
}

#[tokio::test]
async fn an_assertion_vouches_for_a_tunnel_key_and_lets_it_through_relays_for_a_day() {
    let instance = start().await;
    let phone = Device::new("192.0.2.84");
    let account = instance.sign_in_device(&phone, "matt@example.com", "iPhone").await;
    let before: Allowlist = instance.relay_allowlist(None, &instance.relay_bearer).await.json();

    let answer = instance.device_signed(&phone, "POST", "/v1/devices/assertion", Some(&json!({"device_key": "C".repeat(64)})), None).await;
    assert_eq!(answer.status, 200, "{}", answer.text());
    let issued: AssertionIssued = answer.json();
    assert_eq!(issued.expires_in, 3600);
    let said = verify_device_assertion(&instance.service_key, &issued.assertion, instance.now()).expect("the service's word");
    assert_eq!(
        said,
        DeviceAssertion {
            v: 1,
            account_id: account.account_id.clone(),
            account_key: phone.account_key(),
            device_key: "c".repeat(64),
            iat: instance.now(),
            exp: instance.now() + 3600,
        }
    );
    let list: Allowlist = instance.relay_allowlist(Some(before.version), &instance.relay_bearer).await.json();
    assert_eq!(list.endpoints, vec!["c".repeat(64)]);
    let answer = instance.device_signed(&phone, "POST", "/v1/devices/assertion", Some(&json!({"device_key": "xyz"})), None).await;
    assert_eq!(answer.error(), "invalid_request");

    // A day after its last assertion, the key leaves the list.
    instance.advance(86_400 - 10);
    db::purge(&rusqlite::Connection::open(&instance.db).unwrap(), instance.now()).unwrap();
    assert!(instance.relay_allowlist(Some(list.version), &instance.relay_bearer).await.json::<Allowlist>().unchanged);
    instance.advance(11);
    db::purge(&rusqlite::Connection::open(&instance.db).unwrap(), instance.now()).unwrap();
    let list: Allowlist = instance.relay_allowlist(Some(list.version), &instance.relay_bearer).await.json();
    assert!(list.endpoints.is_empty() && !list.unchanged, "{list:?}");

    // 60 an hour.
    instance.advance(3600 - instance.now().rem_euclid(3600));
    for number in 0..60 {
        if number % 20 == 0 {
            // An IP may make 30 requests a minute.
            instance.advance(60);
        }
        let body = json!({"device_key": format!("{number:064x}")});
        assert_eq!(instance.device_signed(&phone, "POST", "/v1/devices/assertion", Some(&body), None).await.status, 200, "{number}");
    }
    let answer = instance.device_signed(&phone, "POST", "/v1/devices/assertion", Some(&json!({"device_key": "d".repeat(64)})), None).await;
    assert_eq!((answer.status, answer.error().as_str()), (429, "rate_limited"));
    let list: Allowlist = instance.relay_allowlist(None, &instance.relay_bearer).await.json();
    assert_eq!(list.endpoints.len(), 60);

    // Signing out takes its keys off at once.
    instance.device_signed(&phone, "POST", "/v1/devices/sign-out", Some(&json!({})), None).await;
    let list: Allowlist = instance.relay_allowlist(None, &instance.relay_bearer).await.json();
    assert!(list.endpoints.is_empty(), "{list:?}");
}

#[tokio::test]
async fn a_key_signed_in_to_another_account_moves_and_an_account_has_30_devices() {
    let instance = start().await;
    let phone = Device::new("192.0.2.85");
    instance.sign_in_device(&phone, "matt@example.com", "iPhone").await;
    instance.device_signed(&phone, "POST", "/v1/devices/assertion", Some(&json!({"device_key": "e".repeat(64)})), None).await;
    let moved = instance.sign_in_device(&phone, "ana@example.com", "Ana's iPhone").await;
    assert_eq!(moved.email, "ana@example.com");
    let me: DeviceAccount = instance.device_signed(&phone, "GET", "/v1/devices/me", None, None).await.json();
    assert_eq!(me, moved);
    let list: Allowlist = instance.relay_allowlist(None, &instance.relay_bearer).await.json();
    assert!(list.endpoints.is_empty(), "the old account's assertions stop counting: {list:?}");

    {
        let conn = rusqlite::Connection::open(&instance.db).unwrap();
        for number in 0..29 {
            conn.execute(
                "INSERT INTO devices (account_key, account_id, name, platform, version, created, last_seen) VALUES (?1, ?2, 'Phone', 'ios', 'v1', 0, 0)",
                [format!("key-{number}"), moved.account_id.clone()],
            )
            .unwrap();
        }
    }
    let tablet = Device::new("192.0.2.86");
    let answer = instance.start_device_sign_in(&tablet, "ana@example.com", "iPad").await;
    let started: SignInStarted = answer.json();
    let verify = json!({"request_id": started.request_id, "code": instance.last_code("ana@example.com")});
    let answer = instance.device_signed(&tablet, "POST", "/v1/devices/sign-in/verify", Some(&verify), None).await;
    assert_eq!((answer.status, answer.error().as_str()), (403, "too_many_devices"));
    instance.advance(1);
    instance.sign_in_device(&phone, "ana@example.com", "Ana's iPhone").await;
}

#[tokio::test]
async fn the_account_page_lists_devices_and_removes_one() {
    let instance = start().await;
    let phone = Device::new("192.0.2.87");
    instance.sign_in_device(&phone, "matt@example.com", "Matt's <iPhone>").await;
    instance.device_signed(&phone, "POST", "/v1/devices/assertion", Some(&json!({"device_key": "f".repeat(64)})), None).await;
    let cookie = web_sign_in(&instance, "matt@example.com", "192.0.2.88").await;
    let page = get(&instance, "/account", &cookie).await.text();
    assert!(page.contains("<h2>Devices signed in</h2>"), "{page}");
    assert!(page.contains("Matt&#39;s iPhone <span class=\"muted\">· iOS · last contact 20 Nov 2026, 14:02 UTC</span>"), "{page}");
    let csrf = field(&page, "csrf");
    let device = field(&page, "device");
    assert_eq!(device, phone.account_key());

    let data = get(&instance, "/account/data", &cookie).await.value();
    assert_eq!(data["devices"][0]["name"], "Matt's iPhone");
    assert_eq!(data["devices"][0]["assertions"][0]["device_key"], "f".repeat(64));

    post_form(&instance, "/account/remove-device", &cookie, &format!("device={device}&confirm=yes")).await;
    assert_eq!(instance.device_signed(&phone, "GET", "/v1/devices/me", None, None).await.status, 200, "not without its CSRF token");
    let confirm = post_form(&instance, "/account/remove-device", &cookie, &format!("csrf={csrf}&device={device}")).await.text();
    assert!(confirm.contains("Remove Matt&#39;s iPhone?"), "{confirm}");
    let done = post_form(&instance, "/account/remove-device", &cookie, &format!("csrf={csrf}&device={device}&confirm=yes")).await;
    assert_eq!(done.status, 303);
    instance.advance(1);
    assert_eq!(instance.device_signed(&phone, "GET", "/v1/devices/me", None, None).await.error(), "unauthorized");
    assert!(instance.relay_allowlist(None, &instance.relay_bearer).await.json::<Allowlist>().endpoints.is_empty());
    assert!(!get(&instance, "/account", &cookie).await.text().contains("Devices signed in"));

    // Deleting the account takes its devices with it.
    instance.sign_in_device(&phone, "matt@example.com", "iPhone").await;
    let mut claktory = Claktory::new("192.0.2.89");
    instance.sign_in(&mut claktory, "matt@example.com", "Mac mini").await;
    let page = get(&instance, "/account", &cookie).await.text();
    let csrf = field(&page, "csrf");
    post_form(&instance, "/account/delete", &cookie, &format!("csrf={csrf}&confirm=yes")).await;
    let conn = rusqlite::Connection::open(&instance.db).unwrap();
    for table in ["devices", "device_keys", "accounts"] {
        let count: i64 = conn.query_row(&format!("SELECT count(*) FROM {table}"), [], |row| row.get(0)).unwrap();
        assert_eq!(count, 0, "{table}");
    }
}

#[test]
fn a_database_from_before_devices_gains_their_tables_and_columns() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("account.db");
    {
        let conn = rusqlite::Connection::open(&path).unwrap();
        conn.execute_batch(
            "CREATE TABLE claktories (id TEXT PRIMARY KEY, account_id TEXT NOT NULL, name TEXT NOT NULL, backend_key TEXT NOT NULL,
             token_hash TEXT NOT NULL UNIQUE, epoch INTEGER NOT NULL, created INTEGER NOT NULL, last_seen INTEGER NOT NULL,
             version TEXT NOT NULL, platform TEXT NOT NULL, revoked_at INTEGER, agents INTEGER, projects INTEGER, devices INTEGER,
             country TEXT, region TEXT, city TEXT);
             INSERT INTO claktories VALUES ('claktory-0123456789', 'a', 'Mac', 'k', 'h', 1, 0, 0, 'v0.1.0', 'macos-arm64', NULL,
             2, 1, 1, NULL, NULL, NULL);
             PRAGMA user_version = 2;",
        )
        .unwrap();
    }
    let conn = db::open(&path).unwrap();
    let claktory = db::claktory(&conn, "claktory-0123456789").unwrap().expect("kept");
    assert_eq!((claktory.display_name, claktory.color, claktory.relays), (None, None, vec![]));
    db::set_identity(&conn, "claktory-0123456789", Some("Studio"), Some("blue"), Some(&["https://relay.example".to_owned()])).unwrap();
    let claktory = db::claktory(&conn, "claktory-0123456789").unwrap().expect("kept");
    assert_eq!(claktory.display_name.as_deref(), Some("Studio"));
    assert_eq!(claktory.relays, vec!["https://relay.example"]);
    assert!(db::devices(&conn, "a").unwrap().is_empty());
    drop(conn);
    db::open(&path).expect("opens again");
    // An older service went back to it (setting its own user_version) and
    // this one came again: the columns it has already aren't added twice.
    rusqlite::Connection::open(&path).unwrap().pragma_update(None, "user_version", 3).unwrap();
    let conn = db::open(&path).expect("opens after a rollback");
    assert_eq!(conn.pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0)).unwrap(), 4);
    assert_eq!(db::claktory(&conn, "claktory-0123456789").unwrap().expect("kept").display_name.as_deref(), Some("Studio"));
}
