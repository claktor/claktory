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
        // As a request to join shows it.
        fingerprint: key_fingerprint(&b64(laptop.key.verifying_key().to_bytes())),
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
        fingerprint: key_fingerprint(&b64(studio.key.verifying_key().to_bytes())),
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

/// Row 671: a key signed in to one account never signs in to another
/// while it's signed in there (its direct-message leaf would span both);
/// it stays where it is. An account has at most 30 devices.
#[tokio::test]
async fn a_key_signed_in_to_one_account_stays_there_and_an_account_has_30_devices() {
    let instance = start().await;
    let phone = Device::new("192.0.2.85");
    let matt = instance.sign_in_device(&phone, "matt@example.com", "iPhone").await;
    instance.device_signed(&phone, "POST", "/v1/devices/assertion", Some(&json!({"device_key": "e".repeat(64)})), None).await;
    let started: SignInStarted = instance.start_device_sign_in(&phone, "ana@example.com", "Ana's iPhone").await.json();
    let verify = json!({"request_id": started.request_id, "code": instance.last_code("ana@example.com")});
    let answer = instance.device_signed(&phone, "POST", "/v1/devices/sign-in/verify", Some(&verify), None).await;
    assert_eq!((answer.status, answer.error().as_str()), (403, "key_of_another_account"), "{}", answer.text());
    let me: DeviceAccount = instance.device_signed(&phone, "GET", "/v1/devices/me", None, None).await.json();
    assert_eq!(me, matt, "still Matt's");
    let list: Allowlist = instance.relay_allowlist(None, &instance.relay_bearer).await.json();
    assert_eq!(list.endpoints.len(), 1, "its assertion still counts: {list:?}");
    let moved = instance.sign_in_device(&Device::new("192.0.2.87"), "ana@example.com", "Ana's iPhone").await;

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
    assert!(confirm.contains("If you sign in on it again, it starts over as a new device."), "{confirm}");
    // A code it asked for before the removal.
    let started: SignInStarted = instance.start_device_sign_in(&phone, "matt@example.com", "iPhone").await.json();
    let verify = json!({"request_id": started.request_id, "code": instance.last_code("matt@example.com")});
    let done = post_form(&instance, "/account/remove-device", &cookie, &format!("csrf={csrf}&device={device}&confirm=yes")).await;
    assert_eq!(done.status, 303);
    instance.advance(1);
    assert_eq!(instance.device_signed(&phone, "GET", "/v1/devices/me", None, None).await.error(), "device_removed");
    assert!(instance.relay_allowlist(None, &instance.relay_bearer).await.json::<Allowlist>().endpoints.is_empty());
    assert!(!get(&instance, "/account", &cookie).await.text().contains("Devices signed in"));

    // Removed there, it never signs in to the account again either: not
    // with the code it had, nor a new one.
    let answer = instance.device_signed(&phone, "POST", "/v1/devices/sign-in/verify", Some(&verify), None).await;
    assert_eq!((answer.status, answer.error().as_str()), (403, "device_removed"), "{}", answer.text());
    let answer = instance.start_device_sign_in(&phone, "matt@example.com", "iPhone").await;
    assert_eq!((answer.status, answer.error().as_str()), (403, "device_removed"));

    // Deleting the account takes its devices with it. (An hour on: an
    // email gets 5 codes an hour.)
    instance.advance(3600);
    let cookie = web_sign_in(&instance, "matt@example.com", "192.0.2.88").await;
    let phone = Device::new("192.0.2.87");
    instance.sign_in_device(&phone, "matt@example.com", "iPhone").await;
    let mut claktory = Claktory::new("192.0.2.89");
    instance.sign_in(&mut claktory, "matt@example.com", "Mac mini").await;
    let page = get(&instance, "/account", &cookie).await.text();
    let csrf = field(&page, "csrf");
    post_form(&instance, "/account/delete", &cookie, &format!("csrf={csrf}&confirm=yes")).await;
    let conn = rusqlite::Connection::open(&instance.db).unwrap();
    for table in ["devices", "device_keys", "removed_devices", "accounts"] {
        let count: i64 = conn.query_row(&format!("SELECT count(*) FROM {table}"), [], |row| row.get(0)).unwrap();
        assert_eq!(count, 0, "{table}");
    }
}

/// A device signs another of the account's out (lost or stolen), as the
/// account page does: that one is answered 401 from then on, and its
/// assertions' keys leave the allowlist. Never another account's device,
/// nor itself.
#[tokio::test]
async fn a_device_removes_another_of_the_account_s_devices() {
    let instance = start().await;
    let phone = Device::new("192.0.2.90");
    instance.sign_in_device(&phone, "matt@example.com", "iPhone").await;
    let stolen = Device::new("192.0.2.91");
    instance.sign_in_device(&stolen, "matt@example.com", "iPad").await;
    let theirs = Device::new("192.0.2.92");
    instance.sign_in_device(&theirs, "ana@example.com", "Ana's iPhone").await;
    instance.device_signed(&stolen, "POST", "/v1/devices/assertion", Some(&json!({"device_key": "a".repeat(64)})), None).await;
    let list: Allowlist = instance.relay_allowlist(None, &instance.relay_bearer).await.json();
    assert_eq!(list.endpoints, vec!["a".repeat(64)]);
    let remove = |device: &Device| json!({"account_key": device.account_key()});

    // Not another account's device, not itself, not a key that isn't one,
    // not unsigned.
    let answer = instance.device_signed(&phone, "POST", "/v1/devices/remove", Some(&remove(&theirs)), None).await;
    assert_eq!((answer.status, answer.json::<DeviceRemoved>()), (200, DeviceRemoved { removed: false }));
    instance.advance(1);
    assert_eq!(instance.device_signed(&theirs, "GET", "/v1/devices/me", None, None).await.status, 200, "still signed in");
    let answer = instance.device_signed(&phone, "POST", "/v1/devices/remove", Some(&remove(&phone)), None).await;
    assert_eq!((answer.status, answer.error().as_str()), (400, "invalid_request"));
    let answer = instance.device_signed(&phone, "POST", "/v1/devices/remove", Some(&json!({"account_key": "nope"})), None).await;
    assert_eq!((answer.status, answer.error().as_str()), (400, "invalid_request"));
    let outsider = Device::new("192.0.2.93");
    let answer = instance.device_signed(&outsider, "POST", "/v1/devices/remove", Some(&remove(&stolen)), None).await;
    assert_eq!((answer.status, answer.error().as_str()), (401, "unauthorized"));

    // The phone removes the stolen iPad.
    let answer = instance.device_signed(&phone, "POST", "/v1/devices/remove", Some(&remove(&stolen)), None).await;
    assert_eq!((answer.status, answer.json::<DeviceRemoved>()), (200, DeviceRemoved { removed: true }), "{}", answer.text());
    instance.advance(1);
    for (method, path, body) in [
        ("GET", "/v1/devices/me", None),
        ("GET", "/v1/claktories", None),
        ("POST", "/v1/devices/assertion", Some(json!({"device_key": "b".repeat(64)}))),
        ("POST", "/v1/devices/remove", Some(remove(&phone))),
    ] {
        let answer = instance.device_signed(&stolen, method, path, body.as_ref(), None).await;
        assert_eq!((answer.status, answer.error().as_str()), (403, "device_removed"), "{path}");
    }
    assert!(instance.relay_allowlist(None, &instance.relay_bearer).await.json::<Allowlist>().endpoints.is_empty());
    assert_eq!(instance.device_signed(&phone, "GET", "/v1/devices/me", None, None).await.status, 200, "the phone stays");
    // Again (a lost answer): done already, no error.
    let answer = instance.device_signed(&phone, "POST", "/v1/devices/remove", Some(&remove(&stolen)), None).await;
    assert_eq!((answer.status, answer.json::<DeviceRemoved>()), (200, DeviceRemoved { removed: false }));

    // 10 an hour.
    instance.advance(3600 - instance.now().rem_euclid(3600));
    for number in 0..10 {
        let other = Device::new(&format!("192.0.2.{}", 100 + number));
        let answer = instance.device_signed(&phone, "POST", "/v1/devices/remove", Some(&remove(&other)), None).await;
        assert_eq!(answer.status, 200, "{number}");
    }
    let answer = instance.device_signed(&phone, "POST", "/v1/devices/remove", Some(&remove(&theirs)), None).await;
    assert_eq!((answer.status, answer.error().as_str()), (429, "rate_limited"));
}

/// A removed key's signed call is told it was removed (`403
/// device_removed`, and nothing more), so its app can start over; but only
/// once its signature, time and limits passed, exactly as a signed-in
/// device's would: a bad, stale or replayed signature under it is a plain
/// `401`, as is a key that was never signed in or signed itself out.
#[tokio::test]
async fn only_a_removed_keys_own_signed_call_learns_it_was_removed() {
    let instance = start().await;
    let phone = Device::new("192.0.2.130");
    instance.sign_in_device(&phone, "matt@example.com", "iPhone").await;
    let stolen = Device::new("192.0.2.131");
    instance.sign_in_device(&stolen, "matt@example.com", "Mac").await;
    let quitter = Device::new("192.0.2.132");
    instance.sign_in_device(&quitter, "matt@example.com", "iPad").await;
    let answer =
        instance.device_signed(&phone, "POST", "/v1/devices/remove", Some(&json!({"account_key": stolen.account_key()})), None).await;
    assert_eq!(answer.json::<DeviceRemoved>(), DeviceRemoved { removed: true });
    instance.device_signed(&quitter, "POST", "/v1/devices/sign-out", Some(&json!({})), None).await;
    instance.advance(60 - instance.now().rem_euclid(60));

    // Its own signed call: removed, with the usual message and nothing else.
    let answer = instance.device_signed(&stolen, "GET", "/v1/devices/me", None, None).await;
    assert_eq!((answer.status, answer.error().as_str()), (403, "device_removed"), "{}", answer.text());
    let fields: Vec<String> = answer.value().as_object().expect("an object").keys().cloned().collect();
    assert_eq!(fields, ["error", "message"], "no account, name or time");
    assert_eq!(answer.value()["message"], "This device was removed from your account. Sign in again to start over as a new device.");

    // Never signed in, or signed itself out: 401.
    let stranger = Device::new("192.0.2.133");
    assert_eq!(instance.device_signed(&stranger, "GET", "/v1/devices/me", None, None).await.error(), "unauthorized");
    assert_eq!(instance.device_signed(&quitter, "GET", "/v1/devices/me", None, None).await.error(), "unauthorized");

    // Under the removed key, a signature by another key, one too old, and
    // one already seen are what they always were.
    let time = instance.now() - 1;
    let headers = |signer: &Device| {
        [
            ("authorization", format!("Device {}", stolen.account_key())),
            (TIME_HEADER, time.to_string()),
            (SIGNATURE_HEADER, sign_request(&signer.key, "GET", "/v1/devices/me", time, b"")),
        ]
    };
    let forged = instance.send("GET", "/v1/devices/me", Vec::new(), &headers(&stranger)).await;
    assert_eq!((forged.status, forged.error().as_str()), (401, "unauthorized"), "{}", forged.text());
    let stale = instance.device_signed(&stolen, "GET", "/v1/devices/me", None, Some(instance.now() - 301)).await;
    assert_eq!((stale.status, stale.error().as_str()), (401, "clock_skew"));
    let first = instance.send("GET", "/v1/devices/me", Vec::new(), &headers(&stolen)).await;
    assert_eq!(first.error(), "device_removed");
    let replayed = instance.send("GET", "/v1/devices/me", Vec::new(), &headers(&stolen)).await;
    assert_eq!((replayed.status, replayed.error().as_str()), (401, "replayed"));

    // And it counts against the key's 240 a minute, as a signed-in one's would (2 so far).
    let ago = std::cell::Cell::new(1);
    let at = || {
        ago.set(ago.get() + 1);
        Some(instance.now() - ago.get())
    };
    for number in 2..240 {
        let stolen = Device { key: stolen.key.clone(), ip: format!("198.51.110.{}", 2 + number % 3) };
        assert_eq!(instance.device_signed(&stolen, "GET", "/v1/devices/me", None, at()).await.error(), "device_removed", "{number}");
    }
    let answer = instance.device_signed(&stolen, "GET", "/v1/devices/me", None, at()).await;
    assert_eq!((answer.status, answer.error().as_str()), (429, "rate_limited"));
}

/// A key another device removed never signs in to that account again, not
/// even with the right code (a thief with the device and the mail): the app
/// starts over with a new key, a new device. Signing itself out leaves it
/// free to sign in again.
#[tokio::test]
async fn a_removed_device_never_signs_in_to_the_account_again() {
    let instance = start().await;
    let phone = Device::new("192.0.2.120");
    instance.sign_in_device(&phone, "matt@example.com", "iPhone").await;
    let stolen = Device::new("192.0.2.121");
    instance.sign_in_device(&stolen, "matt@example.com", "Mac").await;
    let quitter = Device::new("192.0.2.122");
    instance.sign_in_device(&quitter, "matt@example.com", "iPad").await;
    let remove = |device: &Device| json!({"account_key": device.account_key()});

    // A code it asked for before it was removed no longer works, and is spent.
    let started: SignInStarted = instance.start_device_sign_in(&stolen, "matt@example.com", "Mac").await.json();
    let verify = json!({"request_id": started.request_id, "code": instance.last_code("matt@example.com")});
    let answer = instance.device_signed(&phone, "POST", "/v1/devices/remove", Some(&remove(&stolen)), None).await;
    assert_eq!(answer.json::<DeviceRemoved>(), DeviceRemoved { removed: true });
    let answer = instance.device_signed(&stolen, "POST", "/v1/devices/sign-in/verify", Some(&verify), None).await;
    assert_eq!((answer.status, answer.error().as_str()), (403, "device_removed"), "{}", answer.text());
    assert_eq!(answer.value()["message"], "This device was removed from your account. Sign in again to start over as a new device.");
    instance.advance(1);
    let answer = instance.device_signed(&stolen, "POST", "/v1/devices/sign-in/verify", Some(&verify), None).await;
    assert_eq!(answer.error(), "code_expired");
    assert_eq!(instance.device_signed(&stolen, "GET", "/v1/devices/me", None, None).await.error(), "device_removed");

    // Asking for a new code is refused, and no mail goes out.
    let mails = instance.mail.sent().len();
    let answer = instance.start_device_sign_in(&stolen, "Matt@Example.com", "Mac").await;
    assert_eq!((answer.status, answer.error().as_str()), (403, "device_removed"));
    assert_eq!(instance.mail.sent().len(), mails);

    // Nor another account (row 671): the key is still Matt's.
    let started: SignInStarted = instance.start_device_sign_in(&stolen, "ana@example.com", "Ana's Mac").await.json();
    let verify = json!({"request_id": started.request_id, "code": instance.last_code("ana@example.com")});
    let answer = instance.device_signed(&stolen, "POST", "/v1/devices/sign-in/verify", Some(&verify), None).await;
    assert_eq!((answer.status, answer.error().as_str()), (403, "key_of_another_account"));

    // Removed after signing itself out: still never again. (An hour on:
    // refused starts count towards an email's 5 an hour.)
    instance.advance(3600);
    instance.device_signed(&quitter, "POST", "/v1/devices/sign-out", Some(&json!({})), None).await;
    let answer = instance.device_signed(&phone, "POST", "/v1/devices/remove", Some(&remove(&quitter)), None).await;
    assert_eq!(answer.json::<DeviceRemoved>(), DeviceRemoved { removed: false });
    let answer = instance.start_device_sign_in(&quitter, "matt@example.com", "iPad").await;
    assert_eq!((answer.status, answer.error().as_str()), (403, "device_removed"));

    // Download My Data lists them; deleting the account forgets them.
    let cookie = web_sign_in(&instance, "matt@example.com", "192.0.2.123").await;
    let data = get(&instance, "/account/data", &cookie).await.value();
    let removed: Vec<&str> = data["removed_devices"].as_array().unwrap().iter().map(|key| key["account_key"].as_str().unwrap()).collect();
    assert_eq!(removed, [stolen.account_key().as_str(), quitter.account_key().as_str()]);
    let mut kept: Vec<String> = data["device_keys"].as_array().unwrap().iter().map(|key| key.as_str().unwrap().to_owned()).collect();
    kept.sort();
    let mut keys = vec![phone.account_key(), stolen.account_key(), quitter.account_key()];
    keys.sort();
    assert_eq!(kept, keys, "every key that last signed in here");
    let csrf = field(&get(&instance, "/account", &cookie).await.text(), "csrf");
    post_form(&instance, "/account/delete", &cookie, &format!("csrf={csrf}&confirm=yes")).await;
    let conn = rusqlite::Connection::open(&instance.db).unwrap();
    let count: i64 = conn.query_row("SELECT count(*) FROM removed_devices", [], |row| row.get(0)).unwrap();
    assert_eq!(count, 0);
    let count: i64 = conn.query_row("SELECT count(*) FROM key_accounts", [], |row| row.get(0)).unwrap();
    assert_eq!(count, 0, "the keys go with the account");
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
    assert_eq!(conn.pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0)).unwrap(), 5);
    assert_eq!(db::claktory(&conn, "claktory-0123456789").unwrap().expect("kept").display_name.as_deref(), Some("Studio"));
}

/// Names kept before the service left out what they hide are cleaned once
/// the database opens: a direction override or a zero width space can't
/// reorder or hide what's around a Claktory's name on any device, an emoji
/// joined by a joiner stays one. Again after an older service went back to
/// it.
#[test]
fn names_kept_with_what_they_hide_are_cleaned() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("account.db");
    drop(db::open(&path).unwrap());
    let keep = |name: &str, display_name: &str| {
        let conn = rusqlite::Connection::open(&path).unwrap();
        conn.execute(
            "INSERT OR REPLACE INTO claktories (id, account_id, name, backend_key, token_hash, epoch, created, last_seen, version, platform, display_name)
             VALUES ('claktory-0123456789', 'a', ?1, 'k', 'h', 1, 0, 0, 'v0.1.0', 'macos-arm64', ?2)",
            [name, display_name],
        )
        .unwrap();
        conn.pragma_update(None, "user_version", 4).unwrap();
    };
    let names = || {
        let claktory = db::claktory(&db::open(&path).unwrap(), "claktory-0123456789").unwrap().expect("kept");
        (claktory.name, claktory.display_name.unwrap_or_default())
    };
    keep("Mac\u{202E}mini", "\u{200B}\u{1F469}\u{200D}\u{1F4BB} Studio\u{2066}");
    assert_eq!(names(), ("Macmini".to_owned(), "\u{1F469}\u{200D}\u{1F4BB} Studio".to_owned()));
    keep("\u{2067}Lab", "Lab\u{200D}x");
    assert_eq!(names(), ("Lab".to_owned(), "Labx".to_owned()), "again after a rollback");
}

/// Row 630: devices signed in before the service kept when (no
/// `signed_in` column) open as never signed in recently, and opening again
/// doesn't add it twice.
#[test]
fn devices_from_before_sign_in_times_open_as_signed_in_long_ago() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("account.db");
    {
        let conn = rusqlite::Connection::open(&path).unwrap();
        conn.execute_batch(
            "CREATE TABLE devices (account_key TEXT PRIMARY KEY, account_id TEXT NOT NULL, name TEXT NOT NULL, platform TEXT NOT NULL,
             version TEXT NOT NULL, created INTEGER NOT NULL, last_seen INTEGER NOT NULL);
             INSERT INTO devices VALUES ('key-1', 'a', 'Phone', 'ios', 'v0.1.40', 5, 6);
             PRAGMA user_version = 4;",
        )
        .unwrap();
    }
    let conn = db::open(&path).unwrap();
    let device = db::device(&conn, "key-1").unwrap().expect("kept");
    assert_eq!((device.created, device.last_seen, device.signed_in), (5, 6, None));
    drop(conn);
    let conn = db::open(&path).expect("opens again");
    assert_eq!(db::device(&conn, "key-1").unwrap().expect("kept").signed_in, None);
}

/// Row 671 (Fugo's 1A): a key last signed in to Matt's account, signed out
/// since, never signs in to another: Ana's, or a new one's. It signs back
/// in to Matt's as before; a new key signs in to Ana's. A start signed by
/// the key it names is told before a code goes out; an unsigned one (an
/// older app) only at verify, so a public key alone tells nobody which
/// account it's in. Deleting Matt's account frees it.
#[tokio::test]
async fn a_device_key_never_spans_two_accounts() {
    let instance = start().await;
    let phone = Device::new("192.0.2.140");
    let matt = instance.sign_in_device(&phone, "matt@example.com", "iPhone").await;
    instance.device_signed(&phone, "POST", "/v1/devices/sign-out", Some(&json!({})), None).await;
    let start_for = |email: &str| {
        json!({"email": email, "account_key": phone.account_key(), "name": "iPhone", "platform": "ios", "version": "v0.1.13"})
    };

    // Signed: refused at start, and no code is sent.
    let mails = instance.mail.sent().len();
    for email in ["ana@example.com", "new@example.com"] {
        let answer = instance.device_signed(&phone, "POST", "/v1/devices/sign-in/start", Some(&start_for(email)), None).await;
        assert_eq!((answer.status, answer.error().as_str()), (403, "key_of_another_account"), "{email}: {}", answer.text());
        assert_eq!(
            answer.value()["message"],
            "This device was used with another account. Sign in again to start over as a new device."
        );
    }
    assert_eq!(instance.mail.sent().len(), mails);
    // Signed by another key than the one it names: not its to ask.
    let other = Device::new("192.0.2.141");
    let answer = instance.device_signed(&other, "POST", "/v1/devices/sign-in/start", Some(&start_for("ana@example.com")), None).await;
    assert_eq!(answer.status, 401);

    // Unsigned: a code goes out as for any key, and verify refuses it.
    let answer = instance.start_device_sign_in(&phone, "ana@example.com", "iPhone").await;
    assert_eq!(answer.status, 200, "{}", answer.text());
    let started: SignInStarted = answer.json();
    let verify = json!({"request_id": started.request_id, "code": instance.last_code("ana@example.com")});
    let answer = instance.device_signed(&phone, "POST", "/v1/devices/sign-in/verify", Some(&verify), None).await;
    assert_eq!((answer.status, answer.error().as_str()), (403, "key_of_another_account"));
    instance.advance(1);
    let answer = instance.device_signed(&phone, "POST", "/v1/devices/sign-in/verify", Some(&verify), None).await;
    assert_eq!(answer.error(), "code_expired", "the code is spent");

    // Back to Matt's: as before, signed or not.
    let answer = instance.device_signed(&phone, "POST", "/v1/devices/sign-in/start", Some(&start_for("Matt@Example.com")), None).await;
    assert_eq!(answer.status, 200, "{}", answer.text());
    assert_eq!(instance.sign_in_device(&phone, "matt@example.com", "iPhone").await, matt);

    // A new key signs in to Ana's.
    let fresh = Device::new("192.0.2.142");
    assert_eq!(instance.sign_in_device(&fresh, "ana@example.com", "iPhone").await.email, "ana@example.com");

    // Matt's account deleted, the key is no one's.
    let cookie = web_sign_in(&instance, "matt@example.com", "192.0.2.143").await;
    let csrf = field(&get(&instance, "/account", &cookie).await.text(), "csrf");
    post_form(&instance, "/account/delete", &cookie, &format!("csrf={csrf}&confirm=yes")).await;
    instance.advance(3600);
    assert_eq!(instance.sign_in_device(&phone, "ana@example.com", "iPhone").await.email, "ana@example.com");
}

/// Row 671: keys signed in, or removed, before the service kept each key's
/// account are kept as their account's, so they don't sign in to another.
#[test]
fn keys_from_before_are_kept_as_their_accounts() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("account.db");
    {
        let conn = db::open(&path).unwrap();
        conn.execute_batch(
            "INSERT INTO accounts (id, email, created, last_sign_in) VALUES ('a', 'a@example.com', 1, 1), ('b', 'b@example.com', 1, 1);
             INSERT INTO devices (account_key, account_id, name, platform, version, created, last_seen) VALUES ('key-1', 'a', 'Phone', 'ios', 'v1', 5, 6);
             INSERT INTO removed_devices (account_id, account_key, removed) VALUES ('a', 'key-2', 7);
             DROP TABLE key_accounts;",
        )
        .unwrap();
    }
    let conn = db::open(&path).unwrap();
    for key in ["key-1", "key-2"] {
        assert!(db::key_of_another_account(&conn, "b@example.com", key).unwrap(), "{key}");
        assert!(db::key_of_another_account(&conn, "new@example.com", key).unwrap(), "{key}");
        assert!(!db::key_of_another_account(&conn, "a@example.com", key).unwrap(), "{key}");
    }
    assert!(!db::key_of_another_account(&conn, "b@example.com", "key-3").unwrap(), "a key never seen");
}

/// E11-8: a check-in hears the account's devices removed since its cursor,
/// so a Claktory that was off when the person locked one out unpairs it
/// when it's back; all of them with no cursor, one it can't read, or one
/// from another account. Claktories that don't ask hear nothing.
#[tokio::test]
async fn a_check_in_hears_the_device_keys_removed_since_it_last_asked() {
    let instance = start().await;
    let mut studio = Claktory::new("198.51.100.30");
    let account = instance.sign_in(&mut studio, "matt@example.com", "Mac mini").await.account_id;
    let phone = Device::new("192.0.2.90");
    instance.sign_in_device(&phone, "matt@example.com", "iPhone").await;
    let [stolen, lost] = [Device::new("192.0.2.91"), Device::new("192.0.2.92")];
    instance.sign_in_device(&stolen, "matt@example.com", "iPad").await;
    instance.sign_in_device(&lost, "matt@example.com", "MacBook").await;
    let theirs = Device::new("192.0.2.93");
    instance.sign_in_device(&theirs, "ana@example.com", "Ana's iPhone").await;
    let remove = async |device: &Device| {
        instance.advance(1);
        let answer =
            instance.device_signed(&phone, "POST", "/v1/devices/remove", Some(&json!({"account_key": device.account_key()})), None).await;
        assert_eq!(answer.json::<DeviceRemoved>(), DeviceRemoved { removed: true });
    };
    let check_in = async |since: Option<&str>| -> CheckedIn {
        instance.advance(1);
        let body = CheckIn {
            version: "v0.1.30".into(),
            platform: "macos-arm64".into(),
            removed_since: since.map(str::to_owned),
            ..CheckIn::default()
        };
        let answer = instance.signed(&studio, "POST", "/v1/check-in", &body, None).await;
        assert_eq!(answer.status, 200, "{}", answer.text());
        answer.json()
    };

    let none = check_in(Some("")).await;
    assert!(none.removed_device_keys.is_empty());
    let cursor = none.removed_cursor.expect("a cursor");
    remove(&stolen).await;
    instance.advance(600);
    let heard = check_in(Some(&cursor)).await;
    assert_eq!(heard.removed_device_keys, [stolen.account_key()]);
    let cursor = heard.removed_cursor.expect("a cursor");

    // The next one: only it.
    remove(&lost).await;
    let heard = check_in(Some(&cursor)).await;
    assert_eq!(heard.removed_device_keys, [lost.account_key()]);
    let cursor = heard.removed_cursor.expect("a cursor");
    // Again within minutes: it may come again (unpairing twice is
    // harmless); later, nothing new.
    assert_eq!(check_in(Some(&cursor)).await.removed_device_keys, [lost.account_key()]);
    instance.advance(600);
    let cursor = check_in(Some(&cursor)).await.removed_cursor.expect("a cursor");
    instance.advance(3600);
    assert!(check_in(Some(&cursor)).await.removed_device_keys.is_empty(), "nothing new");

    // No cursor, an unreadable one, or another account's: all of them,
    // never another account's keys.
    let mut both = [stolen.account_key(), lost.account_key()];
    both.sort();
    for since in ["", "garbage", &format!("someone-else.{}", instance.now()), &format!("{account}.-5")] {
        let mut keys = check_in(Some(since)).await.removed_device_keys;
        keys.sort();
        assert_eq!(keys, both, "{since}");
    }
    assert!(!check_in(Some("")).await.removed_device_keys.contains(&theirs.account_key()));

    // A Claktory that doesn't ask: nothing, and no cursor.
    let old = check_in(None).await;
    assert_eq!((old.removed_device_keys.len(), old.removed_cursor), (0, None));
}
