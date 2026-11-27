//! A Claktory vouching for the app on its own computer: the app
//! signs in as a device with the code its Claktory gets, no email sent,
//! and is signed out whenever that Claktory's sign-in ends.

use claktory_account::protocol::*;
use serde_json::json;

use crate::support::*;
use crate::web::{field, get, post_form, web_sign_in};

/// `claktory` vouches for `device` (a second later: never a replay).
async fn vouch(instance: &Instance, claktory: &Claktory, device: &Device) -> Answer {
    instance.advance(1);
    let body = DeviceVouch {
        account_key: device.account_key(),
        name: "Matt's MacBook Pro".into(),
        platform: "macos-arm64".into(),
        version: "v0.1.20".into(),
    };
    instance.signed(claktory, "POST", "/v1/devices/vouch", &body, None).await
}

/// `device` signs in with what its Claktory was given.
async fn verify(instance: &Instance, device: &Device, vouched: &DeviceVouched) -> Answer {
    instance.advance(1);
    let body = json!({"request_id": vouched.request_id, "code": vouched.code});
    instance.device_signed(device, "POST", "/v1/devices/sign-in/verify", Some(&body), None).await
}

/// Vouched for and signed in.
async fn vouched_in(instance: &Instance, claktory: &Claktory, device: &Device) -> DeviceAccount {
    let answer = vouch(instance, claktory, device).await;
    assert_eq!(answer.status, 200, "{}", answer.text());
    let answer = verify(instance, device, &answer.json()).await;
    assert_eq!(answer.status, 200, "{}", answer.text());
    answer.json()
}

/// What `GET /v1/devices/me` says to `device`: 200, or its error.
async fn me(instance: &Instance, device: &Device) -> String {
    instance.advance(1);
    let answer = instance.device_signed(device, "GET", "/v1/devices/me", None, None).await;
    if answer.status == 200 { "signed in".to_owned() } else { answer.error() }
}

#[tokio::test]
async fn a_signed_in_claktory_vouches_for_the_app_on_its_computer_once() {
    let instance = start().await;
    let mut claktory = Claktory::new("198.51.100.40");
    let signed_in = instance.sign_in(&mut claktory, "matt@example.com", "Mac mini").await;
    let mails = instance.mail.sent().len();
    let mac = Device::new("198.51.100.40");

    let answer = vouch(&instance, &claktory, &mac).await;
    assert_eq!(answer.status, 200, "{}", answer.text());
    let vouched: DeviceVouched = answer.json();
    assert_eq!(vouched.expires_in, 300);
    assert!(vouched.code.len() >= 20, "never a code to guess: {}", vouched.code);
    assert_eq!(instance.mail.sent().len(), mails, "no email");

    // Only the key vouched for may use it, and trying spends nothing.
    let other = Device::new("198.51.100.41");
    assert_eq!(verify(&instance, &other, &vouched).await.error(), "unauthorized");
    let answer = verify(&instance, &mac, &vouched).await;
    assert_eq!(answer.status, 200, "{}", answer.text());
    let account: DeviceAccount = answer.json();
    let expected = DeviceAccount { account_id: signed_in.account_id, email: "matt@example.com".into(), name: "Matt's MacBook Pro".into() };
    assert_eq!(account, expected);
    assert_eq!(me(&instance, &mac).await, "signed in");
    assert_eq!(verify(&instance, &mac, &vouched).await.error(), "code_expired", "once");
    let conn = rusqlite::Connection::open(&instance.db).unwrap();
    let kept: i64 = conn.query_row("SELECT count(*) FROM requests WHERE kind = 'vouch'", [], |row| row.get(0)).unwrap();
    assert_eq!(kept, 0);

    // An assertion as any device gets.
    let assertion = json!({"device_key": "b".repeat(64)});
    assert_eq!(instance.device_signed(&mac, "POST", "/v1/devices/assertion", Some(&assertion), None).await.status, 200);

    // The account page says how it signed in; Download My Data too.
    let cookie = web_sign_in(&instance, "matt@example.com", "192.0.2.50").await;
    let page = get(&instance, "/account", &cookie).await.text();
    assert!(
        page.contains("Matt&#39;s MacBook Pro <span class=\"muted\">· macOS (Apple silicon) · signed in through Mac mini · last contact"),
        "{page}"
    );
    let data = get(&instance, "/account/data", &cookie).await.value();
    assert_eq!(data["devices"][0]["vouched_by"], json!(claktory.id), "{data}");

    // 5 minutes, then it no longer works.
    let mac2 = Device::new("198.51.100.40");
    let late: DeviceVouched = vouch(&instance, &claktory, &mac2).await.json();
    instance.advance(301);
    assert_eq!(verify(&instance, &mac2, &late).await.error(), "code_expired");

    // One live key per Claktory: the next one it vouches for signs the
    // last out. A code it gave before the next is spent.
    let first: DeviceVouched = vouch(&instance, &claktory, &mac2).await.json();
    let second: DeviceVouched = vouch(&instance, &claktory, &mac2).await.json();
    assert_eq!(verify(&instance, &mac2, &first).await.error(), "code_expired");
    assert_eq!(verify(&instance, &mac2, &second).await.status, 200);
    assert_eq!(me(&instance, &mac).await, "unauthorized", "the key it vouched for before");
    assert_eq!(me(&instance, &mac2).await, "signed in");
}

#[tokio::test]
async fn only_a_signed_in_claktory_vouches_and_never_for_a_removed_or_foreign_key() {
    let instance = start().await;
    let mut claktory = Claktory::new("198.51.100.42");
    instance.sign_in(&mut claktory, "matt@example.com", "Mac mini").await;
    let mac = Device::new("198.51.100.42");

    // Not signed: refused, as is a key that isn't one.
    let body =
        DeviceVouch { account_key: mac.account_key(), name: "Mac".into(), platform: "macos-arm64".into(), version: "v0.1.20".into() };
    assert_eq!(instance.post("/v1/devices/vouch", &body, "198.51.100.42").await.status, 401);
    let stranger = Claktory { token: b64(*random_32()), ..Claktory::new("198.51.100.43") };
    assert_eq!(vouch(&instance, &stranger, &mac).await.status, 401);
    let bad = DeviceVouch { account_key: "not a key".into(), ..body.clone() };
    assert_eq!(instance.signed(&claktory, "POST", "/v1/devices/vouch", &bad, None).await.error(), "invalid_request");

    // Removed from the account by another device: the key is never vouched
    // for again, and a code from before is spent.
    let phone = Device::new("192.0.2.90");
    instance.sign_in_device(&phone, "matt@example.com", "Matt's iPhone").await;
    vouched_in(&instance, &claktory, &mac).await;
    let pending: DeviceVouched = vouch(&instance, &claktory, &mac).await.json();
    let remove = json!({"account_key": mac.account_key()});
    assert_eq!(instance.device_signed(&phone, "POST", "/v1/devices/remove", Some(&remove), None).await.status, 200);
    assert_eq!(me(&instance, &mac).await, "device_removed");
    let answer = vouch(&instance, &claktory, &mac).await;
    assert_eq!((answer.status, answer.error().as_str()), (403, "device_removed"));
    assert_eq!(verify(&instance, &mac, &pending).await.error(), "device_removed");

    // A key signed in to another account isn't this one's to vouch for.
    let anas = Device::new("192.0.2.91");
    instance.sign_in_device(&anas, "ana@example.com", "Ana's Mac").await;
    let answer = vouch(&instance, &claktory, &anas).await;
    assert_eq!((answer.status, answer.error().as_str()), (403, "key_of_another_account"));

    // 10 an hour.
    instance.advance(3600);
    let mut answers = Vec::new();
    for _ in 0..10 {
        answers.push(vouch(&instance, &claktory, &Device::new("198.51.100.42")).await.status);
    }
    assert_eq!(answers, [200; 10].to_vec());
    assert_eq!(vouch(&instance, &claktory, &Device::new("198.51.100.42")).await.error(), "rate_limited");

    // Removed at claktory.com/account: it vouches no more.
    let cookie = web_sign_in(&instance, "matt@example.com", "192.0.2.50").await;
    let csrf = field(&get(&instance, "/account", &cookie).await.text(), "csrf");
    post_form(&instance, "/account/remove", &cookie, &format!("csrf={csrf}&claktory={}&confirm=yes", claktory.id)).await;
    instance.advance(3600);
    let answer = vouch(&instance, &claktory, &Device::new("198.51.100.42")).await;
    assert_eq!((answer.status, answer.error().as_str()), (403, "revoked"));
}

/// The keys a Claktory vouched for are signed out when
/// it signs out, signs in again or to another account, moves or is
/// removed; one that signed in by email itself since is no longer its.
#[tokio::test]
async fn the_keys_a_claktory_vouched_for_end_with_its_sign_in() {
    let instance = start().await;
    let mut claktory = Claktory::new("198.51.100.44");
    instance.sign_in(&mut claktory, "matt@example.com", "Mac mini").await;
    let mac = Device::new("198.51.100.44");

    // It signs out: so does the app, and a code it gave before is spent.
    vouched_in(&instance, &claktory, &mac).await;
    let spare = Device::new("198.51.100.44");
    let pending: DeviceVouched = vouch(&instance, &claktory, &spare).await.json();
    let answer = instance.signed(&claktory, "POST", "/v1/claktories/sign-out", &json!({}), None).await;
    assert_eq!(answer.status, 200, "{}", answer.text());
    assert_eq!(me(&instance, &mac).await, "unauthorized");
    assert_eq!(verify(&instance, &spare, &pending).await.error(), "code_expired");
    let conn = rusqlite::Connection::open(&instance.db).unwrap();
    let kept: i64 = conn.query_row("SELECT count(*) FROM requests WHERE kind = 'vouch'", [], |row| row.get(0)).unwrap();
    assert_eq!(kept, 0, "its codes went with it");

    // Signed in again (with a new backend key: its old one is retired),
    // the key isn't removed: vouched for again, it's back.
    claktory.key = new_signing_key();
    instance.sign_in(&mut claktory, "matt@example.com", "Mac mini").await;
    vouched_in(&instance, &claktory, &mac).await;
    // It signs in again without signing out (it was offline, say): the
    // app's out.
    instance.sign_in(&mut claktory, "matt@example.com", "Mac mini").await;
    assert_eq!(me(&instance, &mac).await, "unauthorized");

    // It moves to another computer: the app on the one it left is out.
    vouched_in(&instance, &claktory, &mac).await;
    let moved = Claktory {
        id: claktory.id.clone(),
        key: new_signing_key(),
        token: b64(*random_32()),
        epoch: claktory.epoch,
        ip: "198.51.100.45".into(),
    };
    let request = MoveRequest {
        backend_key: b64(moved.key.verifying_key().as_bytes()),
        machine_token: moved.token.clone(),
        name: "cloud-box".into(),
        version: "v0.1.20".into(),
        platform: "linux-x86_64".into(),
        from: "Mac mini".into(),
    };
    assert_eq!(instance.signed(&claktory, "POST", "/v1/move", &request, None).await.status, 200);
    assert_eq!(me(&instance, &mac).await, "unauthorized");
    let mut claktory = moved;

    // It moves to another account, proving its key: out.
    vouched_in(&instance, &claktory, &mac).await;
    let started: SignInStarted = instance.start_sign_in(&claktory, "ana@example.com", "cloud-box").await.json();
    let verify_body = SignInVerify {
        request_id: started.request_id,
        code: instance.last_code("ana@example.com"),
        eph_pub: Ephemeral::new().public_b64(),
    };
    let body = serde_json::to_vec(&verify_body).unwrap();
    let time = instance.now();
    let headers = [
        (TIME_HEADER, time.to_string()),
        (SIGNATURE_HEADER, sign_request(&claktory.key, "POST", "/v1/sign-in/verify", time, &body)),
        ("x-forwarded-for", claktory.ip.clone()),
    ];
    let answer = instance.send("POST", "/v1/sign-in/verify", body, &headers).await;
    assert_eq!(answer.status, 200, "{}", answer.text());
    claktory.token = answer.json::<SignedIn>().machine_token;
    assert_eq!(me(&instance, &mac).await, "unauthorized");

    // Removed from its account at claktory.com/account: out.
    let mac = Device::new("198.51.100.45");
    vouched_in(&instance, &claktory, &mac).await;
    let cookie = web_sign_in(&instance, "ana@example.com", "192.0.2.50").await;
    let csrf = field(&get(&instance, "/account", &cookie).await.text(), "csrf");
    let done = post_form(&instance, "/account/remove", &cookie, &format!("csrf={csrf}&claktory={}&confirm=yes", claktory.id)).await;
    assert_eq!(done.status, 303);
    assert_eq!(me(&instance, &mac).await, "unauthorized");

    // A key that signed in by email since is its own: the Claktory's
    // sign-out leaves it be.
    let mut other = Claktory::new("198.51.100.46");
    instance.sign_in(&mut other, "ana@example.com", "Studio").await;
    let laptop = Device::new("198.51.100.46");
    vouched_in(&instance, &other, &laptop).await;
    instance.sign_in_device(&laptop, "ana@example.com", "Ana's laptop").await;
    assert_eq!(instance.signed(&other, "POST", "/v1/claktories/sign-out", &json!({}), None).await.status, 200);
    assert_eq!(me(&instance, &laptop).await, "signed in");
}
