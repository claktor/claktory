//! `POST /v1/pipe-identity`: verified pipes, the service's part. It vouches
//! only for the endpoint a Claktory registered as its backend and proves it
//! holds, for this Claktory and now, and only for a plain email.

use claktory_account::protocol::*;
use ed25519_dalek::{Signer as _, SigningKey};

use claktory_account::server::Config;

use crate::support::*;

/// A Claktory's tunnel key, as pipes pin it: hex.
fn endpoint_id(key: &SigningKey) -> String {
    hex(key.verifying_key().to_bytes())
}

fn request(claktory: &Claktory, key: &SigningKey, time: i64) -> PipeIdentityRequest {
    let endpoint_id = endpoint_id(key);
    let proof = b64(key.sign(&pipe_identity_proof(&claktory.id, &endpoint_id, time)).to_bytes());
    PipeIdentityRequest { endpoint_id, time, proof }
}

async fn register(instance: &Instance, claktory: &Claktory, key: &SigningKey) {
    let endpoints = Endpoints { backend: endpoint_id(key), ..Default::default() };
    assert_eq!(instance.signed(claktory, "PUT", "/v1/endpoints", &endpoints, None).await.status, 200);
}

#[tokio::test]
async fn a_claktory_gets_its_pipe_key_vouched_for_a_week() {
    let instance = start().await;
    let mut claktory = Claktory::new("198.51.100.23");
    let signed_in = instance.sign_in(&mut claktory, "Ana@Example.com", "Mac mini").await;
    let tunnel = new_signing_key();
    register(&instance, &claktory, &tunnel).await;

    let issued: PipeIdentityIssued =
        instance.signed(&claktory, "POST", "/v1/pipe-identity", &request(&claktory, &tunnel, instance.now()), None).await.json();
    assert_eq!(issued.expires_in, PIPE_PEER_LIFETIME);
    let now = instance.now();
    let said = verify_pipe_peer(&instance.service_key, &issued.statement, &endpoint_id(&tunnel), now).expect("it verifies");
    assert_eq!((said.account_id.as_str(), said.email.as_str()), (signed_in.account_id.as_str(), "ana@example.com"));
    assert_eq!((said.iat, said.exp), (now, now + PIPE_PEER_LIFETIME));
    assert_eq!(said.service_key, b64(instance.service_key.to_bytes()));
    // Shown by another Claktory, it's about a key that isn't that one's.
    assert_eq!(verify_pipe_peer(&instance.service_key, &issued.statement, &endpoint_id(&new_signing_key()), now), None);
    assert_eq!(verify_pipe_peer(&instance.service_key, &issued.statement, &endpoint_id(&tunnel), said.exp + ASSERTION_SKEW + 1), None);
}

#[tokio::test]
async fn only_a_key_the_claktory_proves_it_holds_for_itself_and_now() {
    let instance = start().await;
    let mut claktory = Claktory::new("198.51.100.23");
    instance.sign_in(&mut claktory, "ana@example.com", "Mac mini").await;
    let tunnel = new_signing_key();
    register(&instance, &claktory, &tunnel).await;
    let now = instance.now();
    let refused = |answer: Answer| (answer.status, answer.error());

    // A proof made for another Claktory: replayed, it vouches nothing.
    let mut other = request(&claktory, &tunnel, now);
    other.proof = b64(tunnel.sign(&pipe_identity_proof(&Claktory::new("x").id, &other.endpoint_id, now)).to_bytes());
    let answer = instance.signed(&claktory, "POST", "/v1/pipe-identity", &other, None).await;
    assert_eq!(refused(answer), (400, "invalid_request".to_owned()), "another Claktory's proof");
    // Signed by a key that isn't the endpoint's.
    let mut theirs = request(&claktory, &new_signing_key(), now);
    theirs.endpoint_id = endpoint_id(&tunnel);
    let answer = instance.signed(&claktory, "POST", "/v1/pipe-identity", &theirs, None).await;
    assert_eq!(refused(answer), (400, "invalid_request".to_owned()), "not the endpoint's signature");
    // Outside five minutes, either way.
    for off in [-MAX_SKEW - 1, MAX_SKEW + 1] {
        instance.advance(1);
        let answer =
            instance.signed(&claktory, "POST", "/v1/pipe-identity", &request(&claktory, &tunnel, instance.now() + off), None).await;
        assert_eq!(refused(answer), (400, "invalid_request".to_owned()), "a proof from {off} s off");
    }
    let mut garbled = request(&claktory, &tunnel, instance.now());
    garbled.endpoint_id = "zz".repeat(32);
    instance.advance(1);
    assert_eq!(instance.signed(&claktory, "POST", "/v1/pipe-identity", &garbled, None).await.error(), "invalid_request");

    // A key it holds but didn't register: not its pipes' key.
    let unregistered = new_signing_key();
    instance.advance(1);
    let answer = instance.signed(&claktory, "POST", "/v1/pipe-identity", &request(&claktory, &unregistered, instance.now()), None).await;
    assert_eq!(refused(answer), (403, "wrong_endpoint".to_owned()));
    // Another Claktory's registered endpoint, even with its key, isn't this one's.
    let mut ben = Claktory::new("198.51.100.24");
    instance.sign_in(&mut ben, "ben@example.com", "Studio").await;
    instance.advance(1);
    let answer = instance.signed(&ben, "POST", "/v1/pipe-identity", &request(&ben, &tunnel, instance.now()), None).await;
    assert_eq!(refused(answer), (403, "wrong_endpoint".to_owned()), "Ana's endpoint for Ben");
    // Nothing registered yet.
    let mut fresh = Claktory::new("198.51.100.25");
    instance.sign_in(&mut fresh, "cy@example.com", "Laptop").await;
    let answer = instance.signed(&fresh, "POST", "/v1/pipe-identity", &request(&fresh, &new_signing_key(), instance.now()), None).await;
    assert_eq!(refused(answer), (403, "wrong_endpoint".to_owned()));

    // Not signed in, or removed from its account: nothing.
    let stranger = Claktory::new("198.51.100.26");
    let answer = instance.signed(&stranger, "POST", "/v1/pipe-identity", &request(&stranger, &tunnel, instance.now()), None).await;
    assert_eq!(answer.status, 401);
    let conn = rusqlite::Connection::open(&instance.db).expect("the database");
    conn.execute("UPDATE claktories SET revoked_at = 1 WHERE id = ?1", [&claktory.id]).expect("removed");
    instance.advance(1);
    let answer = instance.signed(&claktory, "POST", "/v1/pipe-identity", &request(&claktory, &tunnel, instance.now()), None).await;
    assert_eq!(refused(answer), (403, "revoked".to_owned()));
}

#[tokio::test]
async fn an_email_beyond_plain_ascii_is_never_vouched() {
    let instance = start().await;
    let mut claktory = Claktory::new("198.51.100.23");
    instance.sign_in(&mut claktory, "mätt@example.com", "Mac mini").await;
    let tunnel = new_signing_key();
    register(&instance, &claktory, &tunnel).await;
    let answer = instance.signed(&claktory, "POST", "/v1/pipe-identity", &request(&claktory, &tunnel, instance.now()), None).await;
    assert_eq!((answer.status, answer.error().as_str()), (403, "unsupported_email"));
    // And no account can sign in with one that hides or turns around.
    let answer = instance.start_sign_in(&Claktory::new("198.51.100.24"), "elpmaxe\u{202E}@example.com", "Mac").await;
    assert_eq!((answer.status, answer.error().as_str()), (400, "invalid_email"));
}

#[tokio::test]
async fn a_claktory_gets_48_statements_a_day() {
    let instance = start().await;
    let mut claktory = Claktory::new("198.51.100.23");
    instance.sign_in(&mut claktory, "ana@example.com", "Mac mini").await;
    let tunnel = new_signing_key();
    register(&instance, &claktory, &tunnel).await;
    for _ in 0..48 {
        instance.advance(1);
        let answer = instance.signed(&claktory, "POST", "/v1/pipe-identity", &request(&claktory, &tunnel, instance.now()), None).await;
        assert_eq!(answer.status, 200, "{}", answer.text());
    }
    instance.advance(1);
    let answer = instance.signed(&claktory, "POST", "/v1/pipe-identity", &request(&claktory, &tunnel, instance.now()), None).await;
    assert_eq!(answer.status, 429);
}

/// A claktory.com address (QA's shared inbox, say) would read as Claktory's
/// staff: vouched for only when the operator allows that one address.
#[tokio::test]
async fn a_claktory_com_address_is_vouched_only_when_the_operator_allows_it() {
    let config = Config { trust_proxy: true, vouched_claktory_emails: vec!["Team@Claktory.com".into()], ..Config::default() };
    let instance = start_with(config).await;
    let ask = |email: &'static str, ip: &'static str| {
        let instance = &instance;
        async move {
            let mut claktory = Claktory::new(ip);
            instance.sign_in(&mut claktory, email, "Mac mini").await;
            let tunnel = new_signing_key();
            register(instance, &claktory, &tunnel).await;
            let answer = instance.signed(&claktory, "POST", "/v1/pipe-identity", &request(&claktory, &tunnel, instance.now()), None).await;
            (answer.status, if answer.status == 200 { String::new() } else { answer.error() })
        }
    };
    assert_eq!(ask("qa@claktory.com", "198.51.100.23").await, (403, "unsupported_email".to_owned()));
    assert_eq!(ask("x@mail.claktory.com", "198.51.100.24").await, (403, "unsupported_email".to_owned()), "a subdomain");
    assert_eq!(ask("team@claktory.com", "198.51.100.25").await, (200, String::new()), "allowed by the operator");
    assert_eq!(ask("ana@notclaktory.com", "198.51.100.26").await, (200, String::new()), "another domain ending alike");
    // None is allowed by default.
    let instance = start().await;
    let mut claktory = Claktory::new("198.51.100.27");
    instance.sign_in(&mut claktory, "team@claktory.com", "Mac mini").await;
    let tunnel = new_signing_key();
    register(&instance, &claktory, &tunnel).await;
    let answer = instance.signed(&claktory, "POST", "/v1/pipe-identity", &request(&claktory, &tunnel, instance.now()), None).await;
    assert_eq!((answer.status, answer.error().as_str()), (403, "unsupported_email"));
}
