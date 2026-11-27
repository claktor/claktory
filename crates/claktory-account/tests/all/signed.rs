use claktory_account::protocol::*;

use crate::support::*;

#[tokio::test]
async fn a_share_after_a_restart_is_emailed_with_its_ip_once_a_minute() {
    let instance = start().await;
    let mut claktory = Claktory::new("203.0.113.7");
    instance.sign_in(&mut claktory, "matt@example.com", "Mac mini").await;
    // Not just after signing in, and from a network it hasn't unlocked from:
    // emailed (#412).
    instance.advance(16 * 60);
    assert_eq!(instance.share(&claktory, None).await.ok(), Some((1, instance.expected_share(&claktory, 1))));

    let notice = instance.mail.sent().pop().expect("a notice");
    assert_eq!(notice.from, "Claktory <notices@auth.claktory.com>");
    assert_eq!(notice.subject, "Mac mini unlocked its secrets");
    assert!(notice.text.contains("When: 20 Nov 2026, 14:18 UTC\nFrom: 203.0.113.7\n"), "{}", notice.text);
    assert!(notice.text.contains("at claktory.com/account so it can't unlock again"));

    instance.advance(30);
    let answer = instance.share(&claktory, None).await.expect_err("one a minute");
    assert_eq!((answer.status, answer.error().as_str()), (429, "rate_limited"));
    assert_eq!(answer.value()["retry_after"], 30);
    assert_eq!(answer.headers.get("retry-after").and_then(|value| value.to_str().ok()), Some("30"));
    instance.advance(30);
    assert!(instance.share(&claktory, None).await.is_ok());
    assert_eq!(instance.mail.sent().len(), 2, "the code and one notice: the second came from a network it unlocked from");
}

#[tokio::test]
async fn a_signed_request_needs_the_token_the_key_and_the_right_time_once() {
    let instance = start().await;
    let mut claktory = Claktory::new("198.51.100.20");
    instance.sign_in(&mut claktory, "matt@example.com", "Mac mini").await;
    let check_in = CheckIn { version: "v0.1.9".into(), platform: "macos-arm64".into(), ..Default::default() };

    let answer = instance.signed(&claktory, "POST", "/v1/check-in", &check_in, Some(instance.now() - 301)).await;
    assert_eq!((answer.status, answer.error().as_str()), (401, "clock_skew"));
    assert_eq!(answer.value()["server_time"], instance.now());
    assert_eq!(instance.signed(&claktory, "POST", "/v1/check-in", &check_in, Some(instance.now() + 299)).await.status, 200);

    let thief =
        Claktory { id: claktory.id.clone(), key: new_signing_key(), token: claktory.token.clone(), epoch: 1, ip: claktory.ip.clone() };
    let answer = instance.signed(&thief, "POST", "/v1/check-in", &check_in, None).await;
    assert_eq!((answer.status, answer.error().as_str()), (401, "unauthorized"), "a token without its key");
    let stranger = Claktory { token: "nope".into(), ..Claktory::new("198.51.100.20") };
    assert_eq!(instance.signed(&stranger, "POST", "/v1/check-in", &check_in, None).await.error(), "unauthorized");
    let answer = instance.send("POST", "/v1/check-in", serde_json::to_vec(&check_in).unwrap(), &[]).await;
    assert_eq!(answer.error(), "unauthorized", "no headers");

    // The same request twice.
    let body = serde_json::to_vec(&check_in).unwrap();
    let time = instance.now();
    let headers = [
        ("authorization", format!("Bearer {}", claktory.token)),
        (TIME_HEADER, time.to_string()),
        (SIGNATURE_HEADER, sign_request(&claktory.key, "POST", "/v1/check-in", time, &body)),
    ];
    assert_eq!(instance.send("POST", "/v1/check-in", body.clone(), &headers).await.status, 200);
    let answer = instance.send("POST", "/v1/check-in", body.clone(), &headers).await;
    assert_eq!((answer.status, answer.error().as_str()), (401, "replayed"));
    // Signed for another path, it fails.
    let answer = instance.send("POST", "/v1/share", body, &headers).await;
    assert_eq!(answer.error(), "unauthorized");
}

#[tokio::test]
async fn a_check_in_updates_what_the_service_knows_and_names_the_epoch() {
    let config =
        claktory_account::server::Config { relays: vec!["https://relay-eu.claktory.com".into()], trust_proxy: true, ..Default::default() };
    let instance = start_with(config).await;
    let mut claktory = Claktory::new("198.51.100.21");
    instance.sign_in(&mut claktory, "matt@example.com", "Mac mini").await;
    instance.advance(86_400);
    let check_in = CheckIn { version: "v0.2.0".into(), platform: "linux-x86_64".into(), ..Default::default() };
    let checked: CheckedIn = instance.signed(&claktory, "POST", "/v1/check-in", &check_in, None).await.json();
    assert_eq!(
        checked,
        CheckedIn {
            relays: vec!["https://relay-eu.claktory.com".into()],
            server_time: instance.now(),
            epoch: 1,
            periods_recorded: 0,
            periods_ignored: 0,
            recovery_key: None
        }
    );

    let conn = rusqlite::Connection::open(&instance.db).unwrap();
    let row: (String, String, i64) = conn
        .query_row("SELECT version, platform, last_seen FROM claktories WHERE id = ?1", [&claktory.id], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?))
        })
        .unwrap();
    assert_eq!(row, ("v0.2.0".into(), "linux-x86_64".into(), instance.now()));

    let bad = CheckIn { version: "v0.2.0 <b>".into(), platform: "linux".into(), ..Default::default() };
    assert_eq!(instance.signed(&claktory, "POST", "/v1/check-in", &bad, None).await.error(), "invalid_request");
}

#[tokio::test]
async fn the_share_of_the_epoch_before_can_be_asked_for() {
    let instance = start().await;
    let mut claktory = Claktory::new("198.51.100.22");
    instance.sign_in(&mut claktory, "matt@example.com", "Mac mini").await;
    let answer = instance.share(&claktory, Some(0)).await.expect_err("there's no epoch 0");
    assert_eq!(answer.error(), "invalid_request");

    instance.sign_in(&mut claktory, "matt@example.com", "Mac mini").await;
    assert_eq!(instance.share(&claktory, Some(1)).await.ok(), Some((1, instance.expected_share(&claktory, 1))));
    instance.advance(60);
    assert_eq!(instance.share(&claktory, Some(2)).await.ok(), Some((2, instance.expected_share(&claktory, 2))));
    instance.advance(60);
    assert_eq!(instance.share(&claktory, Some(3)).await.expect_err("not yet").error(), "invalid_request");

    instance.sign_in(&mut claktory, "matt@example.com", "Mac mini").await;
    instance.advance(60);
    assert_eq!(instance.share(&claktory, Some(1)).await.expect_err("two back").error(), "invalid_request");
}

#[tokio::test]
async fn endpoints_make_the_relay_allowlist() {
    let instance = start().await;
    let mut claktory = Claktory::new("198.51.100.23");
    instance.sign_in(&mut claktory, "matt@example.com", "Mac mini").await;
    let (backend, device) = ("a".repeat(64), "B".repeat(64));

    let endpoints = Endpoints { backend: backend.clone(), devices: vec![device.clone(), device.clone()], ..Default::default() };
    let set: EndpointsSet = instance.signed(&claktory, "PUT", "/v1/endpoints", &endpoints, None).await.json();
    assert_eq!(set.count, 2);

    let list: Allowlist = instance.relay_allowlist(None, &instance.relay_bearer).await.json();
    assert_eq!(list.endpoints, vec![backend.clone(), "b".repeat(64)]);
    assert!(!list.unchanged);
    let unchanged: Allowlist = instance.relay_allowlist(Some(list.version), &instance.relay_bearer).await.json();
    assert_eq!(unchanged, Allowlist { version: list.version, endpoints: vec![], unchanged: true });
    // Putting the same again changes nothing (a second later: the same
    // request in the same second is the same signature, refused as replayed).
    instance.advance(1);
    instance.signed(&claktory, "PUT", "/v1/endpoints", &endpoints, None).await;
    let still: Allowlist = instance.relay_allowlist(Some(list.version), &instance.relay_bearer).await.json();
    assert!(still.unchanged);

    let answer = instance.relay_allowlist(None, "wrong").await;
    assert_eq!((answer.status, answer.error().as_str()), (401, "unauthorized"));

    let too_many =
        Endpoints { backend: backend.clone(), devices: (0..20).map(|number| format!("{number:064x}")).collect(), ..Default::default() };
    let answer = instance.signed(&claktory, "PUT", "/v1/endpoints", &too_many, None).await;
    assert_eq!((answer.status, answer.error().as_str()), (400, "too_many_endpoints"));
    let bad = Endpoints { backend: "xyz".into(), devices: vec![], ..Default::default() };
    assert_eq!(instance.signed(&claktory, "PUT", "/v1/endpoints", &bad, None).await.error(), "invalid_request");

    // A removed Claktory's endpoints leave the list.
    let conn = rusqlite::Connection::open(&instance.db).unwrap();
    conn.execute("UPDATE claktories SET revoked_at = 1 WHERE id = ?1", [&claktory.id]).unwrap();
    let list: Allowlist = instance.relay_allowlist(None, &instance.relay_bearer).await.json();
    assert!(list.endpoints.is_empty());
    instance.advance(1);
    let answer = instance.signed(&claktory, "PUT", "/v1/endpoints", &endpoints, None).await;
    assert_eq!((answer.status, answer.error().as_str()), (403, "revoked"));
}

#[tokio::test]
async fn a_moved_claktory_takes_a_new_token_and_key_and_the_old_ones_stop_working() {
    let instance = start().await;
    let mut old = Claktory::new("203.0.113.7");
    instance.sign_in(&mut old, "matt@example.com", "Mac mini").await;
    let check_in = CheckIn { version: "v0.1.9".into(), platform: "macos-arm64".into(), ..Default::default() };

    let moved =
        Claktory { id: old.id.clone(), key: new_signing_key(), token: b64(*random_32()), epoch: old.epoch, ip: "198.51.100.9".into() };
    let request = MoveRequest {
        backend_key: b64(moved.key.verifying_key().as_bytes()),
        machine_token: moved.token.clone(),
        name: "cloud-box".into(),
        version: "v0.1.15".into(),
        platform: "linux-x86_64".into(),
        from: "Mac mini".into(),
    };
    // Signed by the new pair, it's refused: only the old pair can move it.
    assert_eq!(instance.signed(&moved, "POST", "/v1/move", &request, None).await.error(), "unauthorized");
    let answer = instance.signed(&old, "POST", "/v1/move", &request, None).await;
    assert_eq!(answer.status, 200, "{}", answer.text());
    assert_eq!(answer.json::<MoveDone>().epoch, 1, "the epoch, and so the share, stays");

    let notice = instance.mail.sent().pop().expect("a notice");
    assert_eq!(notice.subject, "Mac mini moved to cloud-box");
    assert!(notice.text.contains("From: 203.0.113.7"), "the request came from the old pair's sender: {}", notice.text);

    // The old pair no longer works; the new one does, and opens the same share.
    assert_eq!(instance.signed(&old, "POST", "/v1/check-in", &check_in, None).await.error(), "unauthorized");
    assert_eq!(instance.share(&old, None).await.expect_err("the old copy").error(), "unauthorized");
    assert_eq!(instance.signed(&moved, "POST", "/v1/check-in", &check_in, None).await.status, 200);
    assert_eq!(instance.share(&moved, None).await.ok(), Some((1, instance.expected_share(&old, 1))));

    // Retried with the old pair (its answer was lost): refused, nothing changes.
    assert_eq!(instance.signed(&old, "POST", "/v1/move", &request, None).await.error(), "unauthorized");
    // A token that isn't 32 bytes, or one in use, is refused.
    let short = MoveRequest { machine_token: "abc".into(), ..request.clone() };
    assert_eq!(instance.signed(&moved, "POST", "/v1/move", &short, None).await.error(), "invalid_request");
    let same = MoveRequest { backend_key: b64(new_signing_key().verifying_key().as_bytes()), ..request };
    assert_eq!(instance.signed(&moved, "POST", "/v1/move", &same, None).await.error(), "invalid_request");
}
