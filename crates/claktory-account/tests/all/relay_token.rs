//! Relay tokens: a signed-in Claktory's token for its endpoint,
//! which Claktory's relays check with the relay-token key's public half.

use claktory_account::protocol::*;
use claktory_account::server::relay_token::{ENDPOINTS_PER_DAY, Keys, TOKENS_PER_DAY};

use crate::support::*;

fn endpoint(n: u8) -> String {
    format!("{n:02x}").repeat(32)
}

async fn ask(instance: &Instance, claktory: &Claktory, endpoint: &str) -> Answer {
    // A second apart, so the same request twice isn't taken for a replay.
    instance.advance(1);
    instance.signed(claktory, "POST", "/v1/relay/token", &RelayTokenRequest { endpoint_id: endpoint.to_owned() }, None).await
}

/// The payload's fields, from a token the relay-token key signed.
fn opened(instance: &Instance, token: &str) -> serde_json::Value {
    let key = ed25519_dalek::VerifyingKey::from_bytes(
        &hex_decode(&Keys::derive(&instance.k_share).public_hex()).try_into().expect("32 bytes"),
    )
    .expect("a key");
    let payload = open_relay_token(&key, token).expect("signed by the relay-token key");
    serde_json::from_slice(&payload).expect("JSON")
}

fn hex_decode(text: &str) -> Vec<u8> {
    (0..text.len()).step_by(2).map(|at| u8::from_str_radix(&text[at..at + 2], 16).expect("hex")).collect()
}

#[tokio::test]
async fn a_signed_in_claktory_gets_a_token_bound_to_its_endpoint() {
    let instance = start().await;
    let mut claktory = Claktory::new("203.0.113.70");
    let signed_in = instance.sign_in(&mut claktory, "matt@example.com", "Mac mini").await;
    let mine = endpoint(0xab).to_ascii_uppercase();

    let answer = ask(&instance, &claktory, &mine).await;
    assert_eq!(answer.status, 200, "{}", answer.text());
    let token: RelayToken = answer.json();
    let now = instance.now();
    assert_eq!(token.expires_at, now + RELAY_TOKEN_LIFETIME);
    let payload = opened(&instance, &token.token);
    let a = Keys::derive(&instance.k_share).account(&signed_in.account_id);
    assert_eq!(
        payload,
        serde_json::json!({"v": 1, "e": endpoint(0xab), "a": a, "iat": now, "exp": now + 7 * 86_400}),
        "bound to the endpoint, lower-cased, and the account only by its opaque id"
    );
    assert!(!token.token.contains(&signed_in.account_id) && !a.contains(&signed_in.account_id));

    // Another Claktory of the same account: the same `a`, its own endpoint.
    let mut studio = Claktory::new("203.0.113.71");
    instance.sign_in(&mut studio, "matt@example.com", "Studio").await;
    let theirs: RelayToken = ask(&instance, &studio, &endpoint(0xcd)).await.json();
    assert_eq!(opened(&instance, &theirs.token)["a"], a.as_str());
    assert_eq!(opened(&instance, &theirs.token)["e"], endpoint(0xcd).as_str());
    // Someone else's account: another.
    let mut other = Claktory::new("203.0.113.72");
    instance.sign_in(&mut other, "ana@example.com", "Ana's Mac").await;
    let others: RelayToken = ask(&instance, &other, &endpoint(0xef)).await.json();
    assert_ne!(opened(&instance, &others.token)["a"], a.as_str());
}

#[tokio::test]
async fn only_a_signed_in_claktory_with_a_real_endpoint_gets_one() {
    let instance = start().await;
    let mut claktory = Claktory::new("203.0.113.73");
    instance.sign_in(&mut claktory, "matt@example.com", "Mac mini").await;
    let unsigned = instance.send("POST", "/v1/relay/token", serde_json::to_vec(&RelayTokenRequest { endpoint_id: endpoint(1) }).unwrap(), &[]).await;
    assert_eq!(unsigned.status, 401);
    let mut stranger = Claktory::new("203.0.113.74");
    stranger.token = b64([9; 32]);
    assert_eq!(ask(&instance, &stranger, &endpoint(1)).await.status, 401, "a token the service never gave");
    for bad in ["", "ab", &"zz".repeat(32), &"ab".repeat(33)] {
        let answer = ask(&instance, &claktory, bad).await;
        assert_eq!((answer.status, answer.error().as_str()), (400, "invalid_request"), "{bad}");
    }

    // Removed from its account: refused.
    rusqlite::Connection::open(&instance.db)
        .unwrap()
        .execute("UPDATE claktories SET revoked_at = 1 WHERE id = ?1", [&claktory.id])
        .unwrap();
    let answer = ask(&instance, &claktory, &endpoint(1)).await;
    assert_eq!((answer.status, answer.error().as_str()), (403, "revoked"));
}

#[tokio::test]
async fn an_account_s_claktories_get_tokens_for_four_new_endpoints_a_day() {
    let instance = start().await;
    let mut mac = Claktory::new("203.0.113.75");
    instance.sign_in(&mut mac, "matt@example.com", "Mac mini").await;
    let mut studio = Claktory::new("203.0.113.76");
    instance.sign_in(&mut studio, "matt@example.com", "Studio").await;
    // Up to the day's most, across the account's Claktories.
    assert_eq!(ENDPOINTS_PER_DAY, 4);
    for n in 1..=3 {
        assert_eq!(ask(&instance, &mac, &endpoint(n)).await.status, 200);
    }
    assert_eq!(ask(&instance, &studio, &endpoint(4)).await.status, 200, "the fourth, exactly at the most");
    let refused = ask(&instance, &studio, &endpoint(5)).await;
    assert_eq!((refused.status, refused.error().as_str()), (429, "rate_limited"));
    // An endpoint that had one today gets another.
    assert_eq!(ask(&instance, &mac, &endpoint(2)).await.status, 200);
    // Another account isn't held up.
    let mut other = Claktory::new("203.0.113.77");
    instance.sign_in(&mut other, "ana@example.com", "Ana's Mac").await;
    assert_eq!(ask(&instance, &other, &endpoint(5)).await.status, 200);
    // The next day, new ones again.
    instance.advance(86_400);
    assert_eq!(ask(&instance, &studio, &endpoint(5)).await.status, 200);
}

#[tokio::test]
async fn a_claktory_gets_a_day_s_tokens_at_most() {
    let instance = start().await;
    let mut mac = Claktory::new("203.0.113.78");
    instance.sign_in(&mut mac, "matt@example.com", "Mac mini").await;
    for _ in 0..TOKENS_PER_DAY {
        assert_eq!(ask(&instance, &mac, &endpoint(1)).await.status, 200);
    }
    let refused = ask(&instance, &mac, &endpoint(1)).await;
    assert_eq!((refused.status, refused.error().as_str()), (429, "rate_limited"));
}

#[tokio::test]
async fn a_deleted_account_s_claktory_gets_none() {
    let instance = start().await;
    let mut mac = Claktory::new("203.0.113.79");
    instance.sign_in(&mut mac, "matt@example.com", "Mac mini").await;
    assert_eq!(ask(&instance, &mac, &endpoint(1)).await.status, 200);
    let cookie = crate::web::web_sign_in(&instance, "matt@example.com", "192.0.2.79").await;
    let page = crate::web::get(&instance, "/account", &cookie).await.text();
    let csrf = crate::web::field(&page, "csrf");
    crate::web::post_form(&instance, "/account/delete", &cookie, &format!("csrf={csrf}&confirm=yes")).await;
    let answer = ask(&instance, &mac, &endpoint(1)).await;
    assert!(matches!(answer.status, 401 | 403), "{} {}", answer.status, answer.text());
}
