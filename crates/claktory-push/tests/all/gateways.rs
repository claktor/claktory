//! Apple's and Google's gateways, faked: what the relay sends them, and
//! what it makes of their answers.

use aws_lc_rs::encoding::AsDer;
use aws_lc_rs::rand::SystemRandom;
use aws_lc_rs::signature::{
    ECDSA_P256_SHA256_FIXED, ECDSA_P256_SHA256_FIXED_SIGNING, EcdsaKeyPair, KeyPair, RSA_PKCS1_2048_8192_SHA256, UnparsedPublicKey,
};
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use claktory_push::{ALERT_BODY, ALERT_TITLE, Delivery, Push, Urgency, apns::Apns, fcm::Fcm};
use serde_json::{Value, json};

use crate::support::{Seen, fake, pem};

fn push(urgency: Urgency) -> Push {
    Push { ciphertext: "c2VhbGVk".into(), urgency, collapse: "Y29sbGFwc2U".into(), thread: "dGhyZWFk".into(), ttl: 3600 }
}

/// A JWT's header and claims, once its signature checks out under `public`.
fn jwt(token: &str, algorithm: &'static dyn aws_lc_rs::signature::VerificationAlgorithm, public: &[u8]) -> (Value, Value) {
    let parts: Vec<&str> = token.split('.').collect();
    let [header, claims, signature] = parts.as_slice() else { panic!("{token}") };
    let signature = URL_SAFE_NO_PAD.decode(signature).expect("base64url");
    UnparsedPublicKey::new(algorithm, public).verify(format!("{header}.{claims}").as_bytes(), &signature).expect("signed");
    let decode = |part: &str| serde_json::from_slice(&URL_SAFE_NO_PAD.decode(part).expect("base64url")).expect("JSON");
    (decode(header), decode(claims))
}

#[tokio::test]
async fn apns_gets_the_fixed_alert_the_ciphertext_and_a_signed_token() {
    let p8 = EcdsaKeyPair::generate_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, &SystemRandom::new()).expect("key");
    let public = EcdsaKeyPair::from_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, p8.as_ref()).expect("key").public_key().as_ref().to_vec();
    let production = fake(|seen: &Seen| match seen.head.split_whitespace().nth(1) {
        Some("/3/device/0a0b") => (200, String::new()),
        Some("/3/device/dead") => (410, json!({"reason": "Unregistered"}).to_string()),
        Some("/3/device/bad0") => (400, json!({"reason": "BadDeviceToken"}).to_string()),
        Some("/3/device/b0b0") => (400, json!({"reason": "PayloadTooLarge"}).to_string()),
        _ => (503, json!({"reason": "ServiceUnavailable"}).to_string()),
    })
    .await;
    let sandbox = fake(|_: &Seen| (200, String::new())).await;
    let apns = Apns::new(&pem("PRIVATE KEY", p8.as_ref()), "KEY123", "TEAM456", "com.claktory.app")
        .expect("a key")
        .with_gateways(&production.url, &sandbox.url);

    assert_eq!(apns.send(false, "0a0b", &push(Urgency::Normal)).await, Delivery::Sent);
    let seen = production.seen.lock().expect("lock")[0].clone();
    assert!(seen.head.starts_with("POST /3/device/0a0b "), "{}", seen.head);
    assert_eq!(seen.header("apns-push-type").as_deref(), Some("alert"));
    assert_eq!(seen.header("apns-topic").as_deref(), Some("com.claktory.app"));
    assert_eq!(seen.header("apns-priority").as_deref(), Some("10"));
    assert_eq!(seen.header("apns-collapse-id").as_deref(), Some("Y29sbGFwc2U"));
    let expires: i64 = seen.header("apns-expiration").expect("expiration").parse().expect("a time");
    assert!((expires - claktory_push::now() - 3600).abs() < 5);
    let token = seen.header("authorization").expect("authorization");
    let (header, claims) = jwt(token.trim_start_matches("bearer ").trim_start_matches("Bearer "), &ECDSA_P256_SHA256_FIXED, &public);
    assert_eq!((header["alg"].as_str(), header["kid"].as_str()), (Some("ES256"), Some("KEY123")));
    assert_eq!(claims["iss"], "TEAM456");
    assert_eq!(
        seen.json(),
        json!({
            "aps": {
                "alert": {"title": ALERT_TITLE, "body": ALERT_BODY},
                "mutable-content": 1,
                "sound": "default",
                "interruption-level": "active",
                "thread-id": "dGhyZWFk",
            },
            "c": "c2VhbGVk",
        })
    );

    // Urgent: time-sensitive; low: priority 5; development builds: the sandbox.
    assert_eq!(apns.send(true, "0a0b", &push(Urgency::Urgent)).await, Delivery::Sent);
    let seen = sandbox.seen.lock().expect("lock")[0].clone();
    assert_eq!(seen.json()["aps"]["interruption-level"], "time-sensitive");
    assert_eq!(seen.header("authorization"), production.seen.lock().expect("lock")[0].header("authorization"), "the token is reused");
    apns.send(false, "0a0b", &push(Urgency::Low)).await;
    assert_eq!(production.seen.lock().expect("lock")[1].header("apns-priority").as_deref(), Some("5"));

    assert_eq!(apns.send(false, "dead", &push(Urgency::Normal)).await, Delivery::Gone);
    assert_eq!(apns.send(false, "bad0", &push(Urgency::Normal)).await, Delivery::Gone);
    assert!(matches!(apns.send(false, "b0b0", &push(Urgency::Normal)).await, Delivery::Refused(_)));
    assert!(matches!(apns.send(false, "ffff", &push(Urgency::Normal)).await, Delivery::Transient(_)));
    // Not a token at all: nothing sent.
    let before = production.seen.lock().expect("lock").len();
    assert_eq!(apns.send(false, "../../x", &push(Urgency::Normal)).await, Delivery::Gone);
    assert_eq!(production.seen.lock().expect("lock").len(), before);

    assert!(Apns::new("not a key", "K", "T", "com.claktory.app").is_err());
}

#[tokio::test]
async fn fcm_gets_a_data_message_with_an_access_token_from_the_service_account() {
    let rsa = aws_lc_rs::rsa::KeyPair::generate(aws_lc_rs::rsa::KeySize::Rsa2048).expect("key");
    let public = rsa.public_key().as_ref().to_vec();
    let der = AsDer::<aws_lc_rs::encoding::Pkcs8V1Der>::as_der(&rsa).expect("der");
    let google = fake(|seen: &Seen| {
        let path = seen.head.split_whitespace().nth(1).unwrap_or_default().to_owned();
        if path == "/token" {
            return (200, json!({"access_token": "ya29.token", "expires_in": 3600}).to_string());
        }
        match seen.json()["message"]["token"].as_str() {
            Some("live") => (200, json!({"name": "projects/p/messages/1"}).to_string()),
            Some("gone") => (404, json!({"error": {"status": "NOT_FOUND"}}).to_string()),
            Some("unregistered") => {
                (400, json!({"error": {"status": "INVALID_ARGUMENT", "details": [{"errorCode": "UNREGISTERED"}]}}).to_string())
            }
            _ => (503, json!({"error": {"status": "UNAVAILABLE"}}).to_string()),
        }
    })
    .await;
    let account = json!({
        "type": "service_account",
        "project_id": "claktory-app",
        "client_email": "push@claktory-app.iam.gserviceaccount.com",
        "private_key": pem("PRIVATE KEY", der.as_ref()),
        "token_uri": format!("{}/token", google.url),
    });
    let fcm = Fcm::new(&account.to_string()).expect("an account").with_endpoints(&google.url, &format!("{}/token", google.url));

    assert_eq!(fcm.send("live", &push(Urgency::Urgent)).await, Delivery::Sent);
    let seen = google.seen.lock().expect("lock").clone();
    let [token, message] = seen.as_slice() else { panic!("{seen:?}") };
    assert!(token.body.starts_with("grant_type=urn%3Aietf%3Aparams%3Aoauth%3Agrant-type%3Ajwt-bearer&assertion="));
    let assertion = token.body.split("assertion=").nth(1).expect("assertion");
    let (header, claims) = jwt(assertion, &RSA_PKCS1_2048_8192_SHA256, &public);
    assert_eq!(header["alg"], "RS256");
    assert_eq!(claims["iss"], "push@claktory-app.iam.gserviceaccount.com");
    assert_eq!(claims["scope"], "https://www.googleapis.com/auth/firebase.messaging");
    assert!(message.head.starts_with("POST /v1/projects/claktory-app/messages:send "), "{}", message.head);
    assert_eq!(message.header("authorization").as_deref(), Some("Bearer ya29.token"));
    assert_eq!(
        message.json(),
        json!({"message": {
            "token": "live",
            "android": {"priority": "HIGH", "ttl": "3600s", "collapse_key": "Y29sbGFwc2U"},
            "data": {"c": "c2VhbGVk", "t": "dGhyZWFk", "u": "1"},
        }})
    );

    assert_eq!(fcm.send("gone", &push(Urgency::Normal)).await, Delivery::Gone);
    assert_eq!(fcm.send("unregistered", &push(Urgency::Normal)).await, Delivery::Gone);
    assert!(matches!(fcm.send("busy", &push(Urgency::Normal)).await, Delivery::Transient(_)));
    let tokens = google.seen.lock().expect("lock").iter().filter(|seen| seen.head.contains("/token")).count();
    assert_eq!(tokens, 1, "the access token is reused");
}
