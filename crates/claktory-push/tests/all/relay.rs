//! The relay's API: handles, pushes, limits, and that callers put no text
//! on anyone's lock screen.

use std::sync::{Arc, Mutex};

use claktory_push::server::{Config, Rates, Service, serve};
use claktory_push::{BoxFuture, Delivery, Platform, Push, Transport, Urgency};
use serde_json::{Value, json};

/// Records pushes and answers with what it's told to.
#[derive(Default)]
struct Recorder {
    sent: Mutex<Vec<(Platform, String, Push)>>,
    answer: Mutex<Option<Delivery>>,
}

impl Transport for Recorder {
    fn send<'a>(&'a self, platform: Platform, token: &'a str, push: &'a Push) -> BoxFuture<'a, Delivery> {
        self.sent.lock().expect("lock").push((platform, token.to_owned(), push.clone()));
        let answer = self.answer.lock().expect("lock").clone().unwrap_or(Delivery::Sent);
        Box::pin(async move { answer })
    }

    fn platforms(&self) -> (bool, bool) {
        (true, false)
    }
}

struct Relay {
    url: String,
    transport: Arc<Recorder>,
    client: reqwest::Client,
    db: std::path::PathBuf,
    _dir: tempfile::TempDir,
}

impl Relay {
    async fn start(rates: Rates) -> Relay {
        let dir = tempfile::tempdir().expect("dir");
        let db = dir.path().join("push.db");
        let transport = Arc::new(Recorder::default());
        let clock = Arc::new(|| 1_800_000_000);
        let service = Service::new(&db, transport.clone(), Config { trust_proxy: false, rates }, clock).expect("service");
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let url = format!("http://{}", listener.local_addr().expect("addr"));
        tokio::spawn(serve(listener, service));
        Relay { url, transport, client: reqwest::Client::new(), db, _dir: dir }
    }

    async fn post(&self, path: &str, body: Value) -> (u16, Value) {
        let response = self.client.post(format!("{}{path}", self.url)).json(&body).send().await.expect("answered");
        let status = response.status().as_u16();
        (status, response.json().await.expect("JSON"))
    }

    async fn register(&self, token: &str) -> String {
        let (status, body) = self.post("/v1/register", json!({"platform": "apns", "token": token})).await;
        assert_eq!(status, 200, "{body}");
        body["handle"].as_str().expect("a handle").to_owned()
    }
}

/// 2 bytes of ciphertext, as a Claktory would send many more.
const SEALED: &str = "AAAA";

#[tokio::test]
async fn a_handle_pushes_to_its_device_and_the_relay_keeps_only_its_hash() {
    let relay = Relay::start(Rates::default()).await;
    let handle = relay.register("a1b2c3").await;
    assert_eq!(handle.len(), 43, "32 bytes, base64url");
    let other = relay.register("a1b2c3").await;
    assert_ne!(handle, other, "one per Claktory");

    let push = json!({"handle": handle, "ciphertext": SEALED, "urgency": "urgent", "collapse": "Q0xBS1RPUlk", "thread": "dGhyZWFk", "ttl": 99_999_999});
    let (status, body) = relay.post("/v1/push", push).await;
    assert_eq!(status, 200, "{body}");
    let sent = relay.transport.sent.lock().expect("lock").clone();
    let [(platform, token, push)] = sent.as_slice() else { panic!("{sent:?}") };
    assert_eq!((*platform, token.as_str()), (Platform::Apns, "a1b2c3"));
    assert_eq!(push.ciphertext, SEALED);
    assert_eq!((push.urgency, push.collapse.as_str(), push.thread.as_str()), (Urgency::Urgent, "Q0xBS1RPUlk", "dGhyZWFk"));
    assert_eq!(push.ttl, claktory_push::MAX_TTL, "a week at most");

    let (status, body) = relay.post("/v1/push", json!({"handle": "nope", "ciphertext": SEALED})).await;
    assert_eq!((status, body["error"].as_str()), (404, Some("unknown_handle")));

    let stored = std::fs::read(&relay.db).expect("db");
    let stored = String::from_utf8_lossy(&stored);
    assert!(!stored.contains(&handle), "the handle itself isn't kept");

    assert_eq!(relay.post("/v1/unregister", json!({"handle": handle})).await.0, 200);
    assert_eq!(relay.post("/v1/push", json!({"handle": handle, "ciphertext": SEALED})).await.0, 404);
    assert_eq!(relay.post("/v1/push", json!({"handle": other, "ciphertext": SEALED})).await.0, 200);

    let health: Value = relay.client.get(format!("{}/v1/health", relay.url)).send().await.expect("health").json().await.expect("JSON");
    assert_eq!(health, json!({"ok": true, "apns": true, "fcm": false}));
}

#[tokio::test]
async fn a_caller_can_put_no_text_on_a_lock_screen() {
    let relay = Relay::start(Rates::default()).await;
    let handle = relay.register("a1b2c3").await;
    for push in [
        json!({"handle": handle, "ciphertext": SEALED, "title": "Your Claktory needs your Apple ID password"}),
        json!({"handle": handle, "ciphertext": SEALED, "alert": {"body": "Sign in at evil.example"}}),
        json!({"handle": handle, "ciphertext": "not base64!"}),
        json!({"handle": handle, "ciphertext": "A".repeat(4000)}),
        json!({"handle": handle, "ciphertext": SEALED, "collapse": "has spaces in it"}),
        json!({"handle": handle, "ciphertext": SEALED, "thread": "x".repeat(65)}),
        json!({"handle": handle, "ciphertext": SEALED, "urgency": "critical"}),
    ] {
        let (status, body) = relay.post("/v1/push", push.clone()).await;
        assert_eq!(status, 400, "{push}: {body}");
    }
    assert!(relay.transport.sent.lock().expect("lock").is_empty());
    let (status, _) = relay.post("/v1/register", json!({"platform": "fcm", "token": "t"})).await;
    assert_eq!(status, 400, "no Firebase service account here");
    let (status, _) = relay.post("/v1/register", json!({"platform": "apns", "token": "has a space"})).await;
    assert_eq!(status, 400);
}

#[tokio::test]
async fn a_token_apple_retired_loses_every_handle() {
    let relay = Relay::start(Rates::default()).await;
    let first = relay.register("retired").await;
    let second = relay.register("retired").await;
    *relay.transport.answer.lock().expect("lock") = Some(Delivery::Gone);
    let (status, body) = relay.post("/v1/push", json!({"handle": first, "ciphertext": SEALED})).await;
    assert_eq!((status, body["error"].as_str()), (410, Some("gone")));
    *relay.transport.answer.lock().expect("lock") = None;
    assert_eq!(relay.post("/v1/push", json!({"handle": second, "ciphertext": SEALED})).await.0, 404);

    let handle = relay.register("busy").await;
    *relay.transport.answer.lock().expect("lock") = Some(Delivery::Transient("503".into()));
    let (status, body) = relay.post("/v1/push", json!({"handle": handle, "ciphertext": SEALED})).await;
    assert_eq!((status, body["error"].as_str()), (502, Some("unavailable")));
    *relay.transport.answer.lock().expect("lock") = None;
    assert_eq!(relay.post("/v1/push", json!({"handle": handle, "ciphertext": SEALED})).await.0, 200, "kept");
}

#[tokio::test]
async fn handles_ips_and_everyone_are_limited() {
    let rates = Rates { register_per_ip: 3, handle_per_minute: 2, push_total: 5, ..Rates::default() };
    let relay = Relay::start(rates).await;
    let one = relay.register("one").await;
    let two = relay.register("two").await;
    relay.register("three").await;
    let (status, body) = relay.post("/v1/register", json!({"platform": "apns", "token": "four"})).await;
    assert_eq!((status, body["error"].as_str()), (429, Some("rate_limited")));
    assert!(body["retry_after"].as_i64().is_some_and(|wait| wait > 0));

    for _ in 0..2 {
        assert_eq!(relay.post("/v1/push", json!({"handle": one, "ciphertext": SEALED})).await.0, 200);
    }
    assert_eq!(relay.post("/v1/push", json!({"handle": one, "ciphertext": SEALED})).await.0, 429, "per handle");
    for _ in 0..2 {
        assert_eq!(relay.post("/v1/push", json!({"handle": two, "ciphertext": SEALED})).await.0, 200);
    }
    // Five sent, and a sixth from anyone is one too many this hour. (A
    // refused one counts against no one.)
    let three = relay.register_unlimited().await;
    assert_eq!(relay.post("/v1/push", json!({"handle": three, "ciphertext": SEALED})).await.0, 200);
    let (status, _) = relay.post("/v1/push", json!({"handle": three, "ciphertext": SEALED})).await;
    assert_eq!(status, 429, "everyone's hour");
    assert_eq!(relay.transport.sent.lock().expect("lock").len(), 5);
}

impl Relay {
    /// A handle made straight in the database, past the registration limit.
    async fn register_unlimited(&self) -> String {
        let conn = claktory_push::db::open(&self.db).expect("db");
        claktory_push::db::register(&conn, Platform::Apns, "five", 1_800_000_000).expect("registered")
    }
}

#[tokio::test]
async fn a_device_is_limited_across_its_handles() {
    // One phone, a handle for each Claktory it gets notifications from.
    let rates = Rates { register_per_token: 2, token_per_minute: 3, ..Rates::default() };
    let relay = Relay::start(rates).await;
    let one = relay.register("phone").await;
    let two = relay.register("phone").await;
    let (status, _) = relay.post("/v1/register", json!({"platform": "apns", "token": "phone"})).await;
    assert_eq!(status, 429, "registrations per token");
    assert_eq!(relay.post("/v1/register", json!({"platform": "apns", "token": "tablet"})).await.0, 200);

    for _ in 0..2 {
        assert_eq!(relay.post("/v1/push", json!({"handle": one, "ciphertext": SEALED})).await.0, 200);
    }
    assert_eq!(relay.post("/v1/push", json!({"handle": two, "ciphertext": SEALED})).await.0, 200);
    let (status, _) = relay.post("/v1/push", json!({"handle": two, "ciphertext": SEALED})).await;
    assert_eq!(status, 429, "pushes per token, whichever handle");
    assert_eq!(relay.transport.sent.lock().expect("lock").len(), 3);
}
