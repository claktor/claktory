//! `POST /v1/claktories/sign-out` (#524): a Claktory signing itself out
//! ends its registration at the service, so a copy of its token and key
//! is worth nothing afterwards.

use claktory_account::protocol::*;
use ed25519_dalek::Signer as _;
use serde_json::json;

use crate::support::*;
use crate::user_apps::{get, uid, with_apps, worker};

#[tokio::test]
async fn a_claktory_signed_out_is_refused_everywhere_and_its_grants_and_endpoints_go() {
    let dir = tempfile::tempdir().expect("a folder");
    let socket = worker(dir.path(), uid(dir.path()));
    let instance = with_apps(Some(socket), uid(dir.path())).await;
    let mut studio = Claktory::new("198.51.100.30");
    instance.sign_in(&mut studio, "matt@example.com", "Mac mini").await;
    let phone = Device::new("192.0.2.80");
    instance.sign_in_device(&phone, "matt@example.com", "Matt's iPhone").await;
    let tunnel = new_signing_key();
    let backend = hex(tunnel.verifying_key().to_bytes());
    let endpoints = Endpoints { backend: backend.clone(), ..Default::default() };
    assert_eq!(instance.signed(&studio, "PUT", "/v1/endpoints", &endpoints, None).await.status, 200);
    let grant = json!({"app": "dms", "agent": {"id": "june", "incarnation": "inc-1"}, "scopes": ["dm.read"],
                       "resources": [{"kind": "conversation", "id": "c1"}]});
    assert_eq!(instance.signed(&studio, "PUT", "/v1/apps/grants", &grant, None).await.status, 200);
    let listed = instance.device_signed(&phone, "GET", "/v1/apps/grants", None, None).await;
    assert_eq!(listed.value()["grants"].as_array().map(Vec::len), Some(1));
    let allowed = instance.relay_allowlist(None, &instance.relay_bearer).await.json::<Allowlist>();
    assert!(allowed.endpoints.contains(&backend));

    instance.advance(1);
    let answer = instance.signed(&studio, "POST", "/v1/claktories/sign-out", &json!({}), None).await;
    assert_eq!((answer.status, answer.value()), (200, json!({})), "{}", answer.text());
    // E11-4: its grants are gone at the worker, which heard of the sign-out
    // at once (before anyone lists them, which purges dead ones too).
    let apps = rusqlite::Connection::open(dir.path().join("apps.db")).expect("the worker's records");
    let held: i64 = apps.query_row("SELECT COUNT(*) FROM grants", [], |row| row.get(0)).expect("grants");
    assert_eq!(held, 0);

    // Its token and key, as a copy holds them: 401, never 403 revoked.
    let pipe = {
        let time = instance.now();
        let proof = b64(tunnel.sign(&pipe_identity_proof(&studio.id, &backend, time)).to_bytes());
        PipeIdentityRequest { endpoint_id: backend.clone(), time, proof }
    };
    let refused = [
        instance.signed(&studio, "POST", "/v1/pipe-identity", &pipe, None).await,
        instance.signed(&studio, "POST", "/v1/check-in", &json!({"version": "v0.1.9", "platform": "macos-arm64"}), None).await,
        instance.signed(&studio, "POST", "/v1/relay/token", &RelayTokenRequest { endpoint_id: backend.clone() }, None).await,
        instance.signed(&studio, "POST", "/v1/share", &json!({"eph_pub": Ephemeral::new().public_b64()}), None).await,
        instance.signed(&studio, "PUT", "/v1/endpoints", &endpoints, None).await,
        get(&instance, &studio, "/v1/apps/grants").await,
        get(&instance, &studio, "/v1/apps/whoami").await,
        instance.signed(&studio, "POST", "/v1/claktories/sign-out", &json!({}), Some(instance.now() + 2)).await,
    ];
    for answer in refused {
        assert_eq!((answer.status, answer.error()), (401, "unauthorized".to_owned()), "{answer:?}");
    }
    // The account's devices no longer see it, nor its grants; relays drop it.
    let whoami = instance.device_signed(&phone, "GET", "/v1/apps/whoami", None, None).await.value();
    assert_eq!(whoami["registrations"], json!([]), "{whoami}");
    let listed = instance.device_signed(&phone, "GET", "/v1/apps/grants", None, None).await;
    assert_eq!(listed.value()["grants"], json!([]));
    let listed = instance.device_signed(&phone, "GET", "/v1/claktories", None, None).await;
    assert_eq!(listed.value()["claktories"], json!([]));
    let allowed = instance.relay_allowlist(Some(allowed.version), &instance.relay_bearer).await.json::<Allowlist>();
    assert!(!allowed.unchanged && !allowed.endpoints.contains(&backend), "{:?}", allowed.endpoints);
}

#[tokio::test]
async fn a_key_signed_out_with_never_signs_in_again_but_the_same_claktory_does_with_a_new_one() {
    let instance = start().await;
    let mut studio = Claktory::new("198.51.100.30");
    instance.sign_in(&mut studio, "matt@example.com", "Mac mini").await;
    // A sign-in started before the sign-out: its code no longer works.
    let started = instance.start_sign_in(&studio, "eve@example.com", "Mac mini").await.json::<SignInStarted>();
    instance.advance(1);
    assert_eq!(instance.signed(&studio, "POST", "/v1/claktories/sign-out", &json!({}), None).await.status, 200);

    let answer = instance.start_sign_in(&studio, "matt@example.com", "Mac mini").await;
    assert_eq!((answer.status, answer.error()), (409, "backend_key_retired".to_owned()), "{answer:?}");
    let verify = SignInVerify {
        request_id: started.request_id,
        code: instance.last_code("eve@example.com"),
        eph_pub: Ephemeral::new().public_b64(),
    };
    let answer = instance.post("/v1/sign-in/verify", &verify, &studio.ip).await;
    assert_eq!((answer.status, answer.error()), (409, "backend_key_retired".to_owned()), "{answer:?}");

    // A new key: the same Claktory, at the next epoch.
    let epoch = studio.epoch;
    studio.key = new_signing_key();
    let signed_in = instance.sign_in(&mut studio, "matt@example.com", "Mac mini").await;
    assert_eq!((signed_in.claktory_id.as_str(), signed_in.epoch), (studio.id.as_str(), epoch + 1));
    assert_eq!(
        instance.signed(&studio, "POST", "/v1/check-in", &json!({"version": "v0.1.9", "platform": "macos-arm64"}), None).await.status,
        200
    );
}

#[tokio::test]
async fn a_claktory_removed_at_its_account_page_can_still_sign_itself_out() {
    let instance = start().await;
    let mut studio = Claktory::new("198.51.100.30");
    instance.sign_in(&mut studio, "matt@example.com", "Mac mini").await;
    rusqlite::Connection::open(&instance.db)
        .expect("the database")
        .execute("UPDATE claktories SET revoked_at = 1 WHERE id = ?1", [&studio.id])
        .expect("removed");
    assert_eq!(
        instance.signed(&studio, "POST", "/v1/check-in", &json!({"version": "v0.1.9", "platform": "macos-arm64"}), None).await.error(),
        "revoked"
    );
    instance.advance(1);
    assert_eq!(instance.signed(&studio, "POST", "/v1/claktories/sign-out", &json!({}), None).await.status, 200);
    instance.advance(1);
    let answer = instance.signed(&studio, "POST", "/v1/check-in", &json!({"version": "v0.1.9", "platform": "macos-arm64"}), None).await;
    assert_eq!((answer.status, answer.error()), (401, "unauthorized".to_owned()));
}
