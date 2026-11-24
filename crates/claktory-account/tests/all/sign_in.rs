use claktory_account::protocol::*;

use crate::support::*;

#[tokio::test]
async fn the_service_names_its_key_and_signs_every_answer() {
    let instance = start().await;
    let answer = instance.send("GET", WELL_KNOWN, Vec::new(), &[]).await;
    let known: WellKnown = answer.json();
    assert_eq!((known.service.as_str(), known.api), ("claktory-account", 1));
    assert_eq!(verifying_key(&known.service_key), Some(instance.service_key));
    // Errors are signed too (send checks).
    let answer = instance.send("GET", "/nowhere?x=1", Vec::new(), &[]).await;
    assert_eq!((answer.status, answer.error().as_str()), (404, "not_found"));
}

#[tokio::test]
async fn a_claktory_signs_in_with_an_emailed_code_and_gets_its_share() {
    let instance = start().await;
    let mut claktory = Claktory::new("198.51.100.1");
    let signed_in = instance.sign_in(&mut claktory, " Matt@Example.com ", "Mac mini").await;
    assert_eq!(signed_in.email, "matt@example.com");
    assert_eq!(signed_in.claktory_id, claktory.id);
    assert_eq!(signed_in.epoch, 1);
    assert_eq!(verifying_key(&signed_in.service_key), Some(instance.service_key));

    let mail = instance.mail.sent();
    assert_eq!(mail.len(), 1, "only the code");
    assert_eq!(mail[0].to, "matt@example.com");
    assert_eq!(mail[0].from, "Claktory <code@auth.claktory.com>");
    let code = instance.last_code("matt@example.com");
    assert!(code.len() == 6 && code.bytes().all(|byte| byte.is_ascii_digit()), "{code}");
    assert!(mail[0].text.contains(&format!("\n\n{code}\n\nIt works for 15 minutes.")));

    // A second Claktory joins the same account.
    let mut second = Claktory::new("198.51.100.2");
    let again = instance.sign_in(&mut second, "matt@example.com", "Studio").await;
    assert_eq!(again.account_id, signed_in.account_id);
}

#[tokio::test]
async fn starting_answers_alike_for_any_email_and_refuses_bad_fields() {
    let instance = start().await;
    let claktory = Claktory::new("198.51.100.3");
    let answer = instance.start_sign_in(&claktory, "new@example.com", "Mac mini").await;
    let started: SignInStarted = answer.json();
    assert_eq!((started.expires_in, started.code_from.as_str()), (900, "code@auth.claktory.com"));

    let answer = instance.start_sign_in(&claktory, "not an email", "Mac mini").await;
    assert_eq!((answer.status, answer.error().as_str()), (400, "invalid_email"));
    let mut start = SignInStart {
        email: "matt@example.com".into(),
        claktory_id: "short".into(),
        name: "x".into(),
        backend_key: b64(claktory.key.verifying_key().to_bytes()),
        version: "v1".into(),
        platform: "macos-arm64".into(),
    };
    let answer = instance.post("/v1/sign-in/start", &start, "198.51.100.3").await;
    assert_eq!((answer.status, answer.error().as_str()), (400, "invalid_request"));
    start.claktory_id = claktory.id.clone();
    start.backend_key = "abc".into();
    let answer = instance.post("/v1/sign-in/start", &start, "198.51.100.3").await;
    assert_eq!(answer.error(), "invalid_request");
    let answer = instance.send("POST", "/v1/sign-in/start", b"{".to_vec(), &[]).await;
    assert_eq!(answer.error(), "invalid_request");
}

#[tokio::test]
async fn names_in_emails_are_cleaned() {
    let instance = start().await;
    let mut claktory = Claktory::new("198.51.100.4");
    instance.sign_in(&mut claktory, "matt@example.com", "Visit https://evil.example now\u{7}").await;
    instance.advance(16 * 60);
    instance.share(&claktory, None).await.expect("a share");
    let notice = instance.mail.sent().pop().expect("the notice");
    assert_eq!(notice.subject, "Visit now unlocked its secrets");
}

#[tokio::test]
async fn a_wrong_code_counts_down_and_an_old_one_expires() {
    let instance = start().await;
    let claktory = Claktory::new("198.51.100.5");
    let started: SignInStarted = instance.start_sign_in(&claktory, "matt@example.com", "Mac mini").await.json();
    let code = instance.last_code("matt@example.com");
    let wrong = if code == "000000" { "000001" } else { "000000" };
    let eph_pub = Ephemeral::new().public_b64();
    for left in (0..5).rev() {
        let verify = SignInVerify { request_id: started.request_id.clone(), code: wrong.into(), eph_pub: eph_pub.clone() };
        let answer = instance.post("/v1/sign-in/verify", &verify, &claktory.ip).await;
        assert_eq!((answer.status, answer.error().as_str()), (400, "wrong_code"));
        assert_eq!(answer.value()["attempts_left"], left);
    }
    // Out of tries: even the right code is refused.
    let verify = SignInVerify { request_id: started.request_id, code: code.clone(), eph_pub: eph_pub.clone() };
    let answer = instance.post("/v1/sign-in/verify", &verify, &claktory.ip).await;
    assert_eq!((answer.status, answer.error().as_str()), (410, "code_expired"));

    let started: SignInStarted = instance.start_sign_in(&claktory, "matt@example.com", "Mac mini").await.json();
    instance.advance(CODE_LIFETIME);
    let verify = SignInVerify { request_id: started.request_id, code: instance.last_code("matt@example.com"), eph_pub: eph_pub.clone() };
    let answer = instance.post("/v1/sign-in/verify", &verify, &claktory.ip).await;
    assert_eq!((answer.status, answer.error().as_str()), (410, "code_expired"));

    // A key no one should seal to is refused before the code is tried.
    let started: SignInStarted = instance.start_sign_in(&claktory, "matt@example.com", "Mac mini").await.json();
    let verify = SignInVerify { request_id: started.request_id.clone(), code: "123456".into(), eph_pub: b64([0u8; 32]) };
    assert_eq!(instance.post("/v1/sign-in/verify", &verify, &claktory.ip).await.error(), "invalid_request");
    let verify = SignInVerify { request_id: started.request_id, code: instance.last_code("matt@example.com"), eph_pub };
    assert_eq!(instance.post("/v1/sign-in/verify", &verify, &claktory.ip).await.status, 200, "no try was used");
}

#[tokio::test]
async fn signing_in_again_gives_a_new_token_and_epoch() {
    let instance = start().await;
    let mut claktory = Claktory::new("198.51.100.6");
    instance.sign_in(&mut claktory, "matt@example.com", "Mac mini").await;
    let old_token = claktory.token.clone();
    let old_key = claktory.key.clone();
    claktory.key = new_signing_key();
    let again = instance.sign_in(&mut claktory, "matt@example.com", "Mac mini").await;
    assert_eq!(again.epoch, 2);
    assert_ne!(again.machine_token, old_token);

    let stale = Claktory { id: claktory.id.clone(), key: old_key, token: old_token, epoch: 1, ip: claktory.ip.clone() };
    let answer = instance.share(&stale, None).await.expect_err("the old token is gone");
    assert_eq!((answer.status, answer.error().as_str()), (401, "unauthorized"));
    assert_eq!(instance.share(&claktory, None).await.ok(), Some((2, instance.expected_share(&claktory, 2))));
}

#[tokio::test]
async fn a_claktory_id_belongs_to_one_account_until_it_removes_it() {
    let instance = start().await;
    let mut claktory = Claktory::new("198.51.100.7");
    instance.sign_in(&mut claktory, "matt@example.com", "Mac mini").await;

    let other = Claktory { id: claktory.id.clone(), key: new_signing_key(), token: String::new(), epoch: 0, ip: "198.51.100.8".into() };
    let started: SignInStarted = instance.start_sign_in(&other, "eve@example.com", "Mine").await.json();
    let verify = SignInVerify {
        request_id: started.request_id,
        code: instance.last_code("eve@example.com"),
        eph_pub: Ephemeral::new().public_b64(),
    };
    let answer = instance.post("/v1/sign-in/verify", &verify, &other.ip).await;
    assert_eq!((answer.status, answer.error().as_str()), (409, "claktory_taken"));
    assert!(instance.share(&claktory, None).await.is_ok(), "the owner keeps it");

    // Removed by its account, it can join another, with a new epoch.
    let conn = rusqlite::Connection::open(&instance.db).expect("the database");
    conn.execute("UPDATE claktories SET revoked_at = 1 WHERE id = ?1", [&claktory.id]).expect("removed");
    let mut other = other;
    let signed_in = instance.sign_in(&mut other, "eve@example.com", "Mine").await;
    assert_eq!(signed_in.epoch, 2);
}

#[tokio::test]
async fn an_account_has_ten_claktories_at_most() {
    let instance = start().await;
    for number in 0..10 {
        let mut claktory = Claktory::new(&format!("198.51.101.{number}"));
        instance.sign_in(&mut claktory, "matt@example.com", "Mac").await;
        if number % 5 == 4 {
            instance.advance(3600);
        }
    }
    let eleventh = Claktory::new("198.51.101.10");
    let started: SignInStarted = instance.start_sign_in(&eleventh, "matt@example.com", "Mac").await.json();
    let verify = SignInVerify {
        request_id: started.request_id,
        code: instance.last_code("matt@example.com"),
        eph_pub: Ephemeral::new().public_b64(),
    };
    let answer = instance.post("/v1/sign-in/verify", &verify, &eleventh.ip).await;
    assert_eq!((answer.status, answer.error().as_str()), (403, "too_many_claktories"));
}

/// Signed out of one account, a Claktory signs in to another with one code:
/// its verify is signed with the backend key it has, which proves it's the
/// same Claktory, so it moves. Signed with another key it's still taken,
/// and the code isn't spent on a clock that's off.
#[tokio::test]
async fn a_claktory_proving_its_key_moves_to_another_account_with_one_code() {
    let instance = start().await;
    let mut claktory = Claktory::new("198.51.100.9");
    let first = instance.sign_in(&mut claktory, "matt@example.com", "Mac mini").await;

    let started: SignInStarted = instance.start_sign_in(&claktory, "ana@example.com", "Mac mini").await.json();
    let ephemeral = Ephemeral::new();
    let verify =
        SignInVerify { request_id: started.request_id, code: instance.last_code("ana@example.com"), eph_pub: ephemeral.public_b64() };
    let body = serde_json::to_vec(&verify).unwrap();
    let signed = |key: &ed25519_dalek::SigningKey, time: i64| {
        vec![
            (TIME_HEADER, time.to_string()),
            (SIGNATURE_HEADER, sign_request(key, "POST", "/v1/sign-in/verify", time, &body)),
            ("x-forwarded-for", claktory.ip.clone()),
        ]
    };
    let answer = instance.send("POST", "/v1/sign-in/verify", body.clone(), &signed(&new_signing_key(), instance.now())).await;
    assert_eq!(answer.error(), "claktory_taken", "another key proves nothing");

    let started: SignInStarted = instance.start_sign_in(&claktory, "ana@example.com", "Mac mini").await.json();
    let verify = SignInVerify { request_id: started.request_id, code: instance.last_code("ana@example.com"), ..verify };
    let body = serde_json::to_vec(&verify).unwrap();
    let headers = |time: i64| {
        vec![
            (TIME_HEADER, time.to_string()),
            (SIGNATURE_HEADER, sign_request(&claktory.key, "POST", "/v1/sign-in/verify", time, &body)),
            ("x-forwarded-for", claktory.ip.clone()),
        ]
    };
    let answer = instance.send("POST", "/v1/sign-in/verify", body.clone(), &headers(instance.now() - 1000)).await;
    assert_eq!(answer.error(), "clock_skew", "a clock that's off is said so");
    let answer = instance.send("POST", "/v1/sign-in/verify", body.clone(), &headers(instance.now())).await;
    assert_eq!(answer.status, 200, "the same code, still good: {}", answer.text());
    let moved: SignedIn = answer.json();
    assert_ne!(moved.account_id, first.account_id);
    assert_eq!((moved.email.as_str(), moved.claktory_id.as_str(), moved.epoch), ("ana@example.com", claktory.id.as_str(), 2));
    assert!(ephemeral.open(&moved.sealed_share, &claktory.id, 2).is_some());

    let conn = rusqlite::Connection::open(&instance.db).expect("the database");
    let owner: String = conn.query_row("SELECT account_id FROM claktories WHERE id = ?1", [&claktory.id], |row| row.get(0)).unwrap();
    assert_eq!(owner, moved.account_id, "the old account no longer has it");
}
