//! Android pushes end to end: this Claktory
//! seals a notification for a phone registered with the push relay, the
//! relay's own code (`claktory-push`, with FCM as it runs at
//! push.claktory.com) sends it through Firebase Cloud Messaging's HTTP v1
//! API, here a stand-in for Google's token and send endpoints, and the
//! phone's key opens what Google was given. One Firebase project holds both
//! Android apps, com.claktory.app and com.claktory.app.dev:
//! a token from either goes through that project. A token Google retired is
//! forgotten by the relay and by this Claktory.

use std::sync::Arc;

use aws_lc_rs::encoding::AsDer;
use base64::Engine;
use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use claktory_push::server::{Config, Service};
use claktory_push::{Gateways, fcm::Fcm};
use factory_protocol::v1::{self, client_message, server_message::Body};
use serde_json::{Value, json};

use crate::common::stand_in::{Request, StandIn};
use crate::common::{Client, View, apply_request, start, temp_dir};

/// The Firebase project both Android apps are in.
const PROJECT: &str = "claktory-play";
/// Tokens Google gave each app's install.
const PROD_TOKEN: &str = "prod-app:APA91bProdToken";
const DEV_TOKEN: &str = "dev-app:APA91bDevToken";
/// A token Google refuses to send to, through no fault of its own.
const REFUSED_TOKEN: &str = "refused:APA91bRefused";

/// Google: an access token for the service account's assertion, and
/// messages:send answering for each token as FCM does.
async fn google() -> StandIn {
    StandIn::start(|request: &Request| {
        if request.path() == "/token" {
            return (200, json!({"access_token": "ya29.stand-in", "expires_in": 3600}));
        }
        if request.path() != format!("/v1/projects/{PROJECT}/messages:send") {
            return (404, json!({"error": {"status": "NOT_FOUND"}}));
        }
        match request.json()["message"]["token"].as_str() {
            Some(PROD_TOKEN | DEV_TOKEN) => (200, json!({"name": format!("projects/{PROJECT}/messages/1")})),
            // The relay's service account isn't allowed to send (no FCM role).
            Some(REFUSED_TOKEN) => (403, json!({"error": {"status": "PERMISSION_DENIED"}})),
            // A token Google retired: the app was removed, or got a new one.
            _ => (404, json!({"error": {"status": "NOT_FOUND", "details": [{"errorCode": "UNREGISTERED"}]}})),
        }
    })
    .await
}

/// A service account file for the project, as Google gives it, with a key
/// of the test's own.
fn service_account(google: &StandIn) -> String {
    let rsa = aws_lc_rs::rsa::KeyPair::generate(aws_lc_rs::rsa::KeySize::Rsa2048).expect("key");
    let der = AsDer::<aws_lc_rs::encoding::Pkcs8V1Der>::as_der(&rsa).expect("der");
    let body = STANDARD.encode(der.as_ref());
    let lines: Vec<&str> = body.as_bytes().chunks(64).map(|line| std::str::from_utf8(line).expect("ascii")).collect();
    json!({
        "type": "service_account",
        "project_id": PROJECT,
        "client_email": format!("claktory-push@{PROJECT}.iam.gserviceaccount.com"),
        "private_key": format!("-----BEGIN PRIVATE KEY-----\n{}\n-----END PRIVATE KEY-----\n", lines.join("\n")),
        "token_uri": format!("{}/token", google.base),
    })
    .to_string()
}

/// The relay, with FCM and no Apple key, as push.claktory.com runs with
/// only fcm.json: its address.
async fn relay(dir: &std::path::Path, google: &StandIn) -> String {
    let fcm =
        Fcm::new(&service_account(google)).expect("a service account").with_endpoints(&google.base, &format!("{}/token", google.base));
    let transport = Arc::new(Gateways { apns: None, fcm: Some(fcm) });
    let service = Service::new(&dir.join("push.db"), transport, Config::default(), Arc::new(claktory_push::now)).expect("relay");
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let url = format!("http://{}", listener.local_addr().expect("addr"));
    tokio::spawn(claktory_push::server::serve(listener, service));
    url
}

/// What the relay answers a POST: its status and body.
async fn post(url: &str, body: Value) -> (u16, Value) {
    let response = reqwest::Client::new().post(url).json(&body).send().await.expect("answered");
    (response.status().as_u16(), response.json().await.unwrap_or_default())
}

/// The phone registers its FCM token with the relay, as the Android app
/// does: the handle it's given.
async fn register_token(relay: &str, token: &str) -> String {
    let (status, body) = post(&format!("{relay}/v1/register"), json!({"platform": "fcm", "token": token})).await;
    assert_eq!(status, 200, "{body}");
    body["handle"].as_str().expect("a handle").to_owned()
}

/// The messages Google was asked to send, once there are `count`.
async fn sent(google: &StandIn, count: usize) -> Vec<Request> {
    for _ in 0..500 {
        let sent: Vec<Request> = google.requests().into_iter().filter(|request| request.path().ends_with("/messages:send")).collect();
        if sent.len() >= count {
            return sent;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    panic!("Google wasn't asked to send message {count}");
}

/// What a message to the phone holds, opened with its key.
fn open(key: &[u8], message: &Value) -> Value {
    use aws_lc_rs::aead::{AES_256_GCM, Aad, LessSafeKey, Nonce, UnboundKey};
    let sealed = STANDARD.decode(message["data"]["c"].as_str().expect("ciphertext")).expect("base64");
    let (nonce, sealed) = sealed.split_at(12);
    let cipher = LessSafeKey::new(UnboundKey::new(&AES_256_GCM, key).expect("key"));
    let mut buffer = sealed.to_vec();
    let plain = cipher.open_in_place(Nonce::try_assume_unique_for_key(nonce).expect("nonce"), Aad::empty(), &mut buffer).expect("opens");
    serde_json::from_slice(plain).expect("JSON")
}

fn register(handle: &str, key: &[u8]) -> client_message::Body {
    client_message::Body::RegisterNotifications(v1::RegisterNotifications {
        handle: handle.into(),
        key: key.to_vec(),
        platform: v1::PushPlatform::Fcm.into(),
        permission: v1::NotificationPermission::Allowed.into(),
    })
}

#[tokio::test]
async fn an_android_phone_gets_pushes_through_the_relay_and_fcm() {
    let dir = temp_dir();
    let google = google().await;
    let relay = relay(&dir, &google).await;
    let (addr, factory) = start(&dir).await;
    let (mut client, workspace, _) = Client::connect_as(addr, v1::DeviceKind::Phone).await;
    let mut view = View { workspace, ..View::default() };
    let settings = v1::NotificationSettings {
        needs_you: true,
        alerts: true,
        replies: true,
        relay: relay.clone(),
        preview: v1::NotifyPreview::Full.into(),
        ..Default::default()
    };
    let update = client_message::Body::UpdateSettings(v1::UpdateSettings { notifications: Some(settings), ..Default::default() });
    apply_request(&mut client, &mut view, update).await;

    // Claktory Dev on Android (com.claktory.app.dev).
    let dev_key = [7u8; 32];
    let dev_handle = register_token(&relay, DEV_TOKEN).await;
    assert_eq!(URL_SAFE_NO_PAD.decode(&dev_handle).expect("base64url").len(), 32);
    assert!(matches!(apply_request(&mut client, &mut view, register(&dev_handle, &dev_key)).await, Body::Ack(_)));
    client.ok(client_message::Body::SendTestNotification(v1::SendTestNotification {})).await;
    let message = sent(&google, 1).await.remove(0);
    assert_eq!(message.method, "POST");
    assert_eq!(message.header("authorization"), Some("Bearer ya29.stand-in"));
    let message = message.json()["message"].clone();
    assert_eq!(message["token"], DEV_TOKEN);
    // High priority, kept as long as a test is, and data only: the app
    // decrypts and shows it itself, so Google sees no words.
    assert_eq!(message["android"]["priority"], "HIGH");
    assert_eq!(message["android"]["ttl"], "600s");
    assert!(message.get("notification").is_none(), "{message}");
    assert!(message["data"].as_object().expect("data").keys().all(|key| ["c", "t", "u"].contains(&key.as_str())), "{message}");
    let payload = open(&dev_key, &message);
    assert_eq!((payload["kind"].as_str(), payload["ctr"].as_u64()), (Some("test"), Some(1)));
    let claktory = payload["claktory"].as_str().expect("its name").to_owned();
    assert!(!claktory.is_empty() && !message.to_string().contains(&claktory), "{message}");

    // The same phone with Claktory (com.claktory.app) instead: its token
    // goes through the same project.
    let prod_key = [8u8; 32];
    let prod_handle = register_token(&relay, PROD_TOKEN).await;
    assert!(matches!(apply_request(&mut client, &mut view, register(&prod_handle, &prod_key)).await, Body::Ack(_)));
    client.ok(client_message::Body::SendTestNotification(v1::SendTestNotification {})).await;
    let message = sent(&google, 2).await.remove(1).json()["message"].clone();
    assert_eq!(message["token"], PROD_TOKEN);
    assert_eq!(open(&prod_key, &message)["ctr"], 1, "a new key counts from the start");
    let tokens = google.requests().iter().filter(|request| request.path() == "/token").count();
    assert_eq!(tokens, 1, "one access token for both apps");

    // Google refuses a push for good (here the relay's service account
    // lacks the FCM role): it isn't tried again, and the phone stays.
    let refused = register_token(&relay, REFUSED_TOKEN).await;
    assert!(matches!(apply_request(&mut client, &mut view, register(&refused, &prod_key)).await, Body::Ack(_)));
    client.ok(client_message::Body::SendTestNotification(v1::SendTestNotification {})).await;
    assert_eq!(sent(&google, 3).await[2].json()["message"]["token"], REFUSED_TOKEN);
    assert!(factoryd::notify::registered_for_test(&factory, "local"));
    // The next push goes out alone: the refused one wasn't kept to try again.
    assert!(matches!(apply_request(&mut client, &mut view, register(&prod_handle, &prod_key)).await, Body::Ack(_)));
    client.ok(client_message::Body::SendTestNotification(v1::SendTestNotification {})).await;
    assert_eq!(sent(&google, 4).await[3].json()["message"]["token"], PROD_TOKEN);
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    assert_eq!(sent(&google, 4).await.len(), 4, "nothing tried again");

    // Google retired the token (the app was uninstalled): the relay forgets
    // every handle of it, and this Claktory forgets the device.
    let retired = register_token(&relay, "retired:APA91bGone").await;
    assert!(matches!(apply_request(&mut client, &mut view, register(&retired, &prod_key)).await, Body::Ack(_)));
    assert!(factoryd::notify::registered_for_test(&factory, "local"));
    client.ok(client_message::Body::SendTestNotification(v1::SendTestNotification {})).await;
    assert_eq!(sent(&google, 5).await[4].json()["message"]["token"], "retired:APA91bGone");
    let mut forgotten = false;
    for _ in 0..250 {
        if !factoryd::notify::registered_for_test(&factory, "local") {
            forgotten = true;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    assert!(forgotten, "the device wasn't forgotten");
    let (status, body) = post(&format!("{relay}/v1/push"), json!({"handle": retired, "ciphertext": "AAAA"})).await;
    assert_eq!((status, body["error"].as_str()), (404, Some("unknown_handle")), "the relay forgot the handle");
    // The phone's other handles stay until Google retires their tokens.
    let (status, _) = post(&format!("{relay}/v1/push"), json!({"handle": prod_handle, "ciphertext": "AAAA"})).await;
    assert_eq!(status, 200);
}
