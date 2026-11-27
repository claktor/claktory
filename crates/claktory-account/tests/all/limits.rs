use std::sync::atomic::Ordering;

use claktory_account::protocol::*;
use claktory_account::server::{Config, mail};

use crate::support::*;

#[tokio::test]
async fn codes_are_limited_per_email_and_per_ip() {
    let instance = start().await;
    // Per email: 5 an hour.
    for number in 0..5 {
        let claktory = Claktory::new(&format!("198.51.102.{number}"));
        assert_eq!(instance.start_sign_in(&claktory, "matt@example.com", "Mac").await.status, 200);
    }
    let answer = instance.start_sign_in(&Claktory::new("198.51.102.9"), "MATT@example.com", "Mac").await;
    assert_eq!((answer.status, answer.error().as_str()), (429, "rate_limited"));
    assert_eq!(answer.value()["retry_after"], 3600 - START.rem_euclid(3600));
    instance.advance(3600);
    assert_eq!(instance.start_sign_in(&Claktory::new("198.51.102.10"), "matt@example.com", "Mac").await.status, 200);

    // Per IP: 30 a day (a minute apart now and then: 30 requests a minute is
    // a limit of its own).
    let claktory = Claktory::new("198.51.103.1");
    for number in 0..30 {
        if number % 20 == 0 {
            instance.advance(61);
        }
        assert_eq!(instance.start_sign_in(&claktory, &format!("person{number}@example.com"), "Mac").await.status, 200, "{number}");
    }
    instance.advance(61);
    let answer = instance.start_sign_in(&claktory, "someone@example.com", "Mac").await;
    assert_eq!(answer.error(), "rate_limited");
    // Refused, it sent nothing.
    assert!(!instance.mail.sent().iter().any(|mail| mail.to == "someone@example.com"));
}

#[tokio::test]
async fn an_ip_makes_thirty_requests_a_minute() {
    let instance = start().await;
    for _ in 0..30 {
        assert_eq!(
            instance.send("GET", "/.well-known/claktory-account", Vec::new(), &[("x-forwarded-for", "198.51.104.1".into())]).await.status,
            200
        );
    }
    let answer = instance.send("GET", "/.well-known/claktory-account", Vec::new(), &[("x-forwarded-for", "198.51.104.1".into())]).await;
    assert_eq!(answer.error(), "rate_limited");
    let answer =
        instance.send("GET", "/.well-known/claktory-account", Vec::new(), &[("x-forwarded-for", "10.0.0.1, 198.51.104.2".into())]).await;
    assert_eq!(answer.status, 200, "the proxy's last entry is the client");
}

#[tokio::test]
async fn signed_requests_are_limited_by_their_key_not_their_address() {
    let instance = start().await;
    // A person's Claktory, phone and Mac, behind one router.
    let home = "198.51.108.1";
    let mut claktory = Claktory::new(home);
    instance.sign_in(&mut claktory, "matt@example.com", "Mac mini").await;
    let (phone, mac) = (Device::new(home), Device::new(home));
    instance.sign_in_device(&phone, "matt@example.com", "iPhone").await;
    instance.sign_in_device(&mac, "matt@example.com", "MacBook").await;
    instance.advance(60 - instance.now().rem_euclid(60));
    // Each request at a time of its own, a second earlier than the last
    // (within the skew allowed), so none is a replay of another.
    let ago = std::cell::Cell::new(0);
    let at = || {
        ago.set(ago.get() + 1);
        Some(instance.now() - ago.get())
    };

    let check_in = CheckIn { version: "v0.1.9".into(), platform: "macos-arm64".into(), ..Default::default() };
    for number in 0..20 {
        assert_eq!(instance.signed(&claktory, "POST", "/v1/check-in", &check_in, at()).await.status, 200, "{number}");
        assert_eq!(instance.device_signed(&phone, "GET", "/v1/devices/me", None, at()).await.status, 200, "{number}");
        assert_eq!(instance.device_signed(&mac, "GET", "/v1/claktories", None, at()).await.status, 200, "{number}");
    }
    // None of it counted against the address's 30 unsigned requests.
    for number in 0..30 {
        let answer = instance.send("GET", "/.well-known/claktory-account", Vec::new(), &[("x-forwarded-for", home.into())]).await;
        assert_eq!(answer.status, 200, "{number}");
    }
    let answer = instance.send("GET", "/.well-known/claktory-account", Vec::new(), &[("x-forwarded-for", home.into())]).await;
    assert_eq!(answer.error(), "rate_limited");
    assert_eq!(instance.device_signed(&phone, "GET", "/v1/devices/me", None, at()).await.status, 200, "signed, it goes on");

    // A key makes 240 a minute, wherever from.
    for number in 21..240 {
        let ip = format!("198.51.108.{}", 2 + number % 3);
        let phone = Device { key: phone.key.clone(), ip };
        assert_eq!(instance.device_signed(&phone, "GET", "/v1/devices/me", None, at()).await.status, 200, "{number}");
    }
    let answer = instance.device_signed(&phone, "GET", "/v1/devices/me", None, at()).await;
    assert_eq!((answer.status, answer.error().as_str()), (429, "rate_limited"));
    assert_eq!(answer.value()["retry_after"], 60);
    assert_eq!(instance.device_signed(&mac, "GET", "/v1/devices/me", None, at()).await.status, 200, "another key");
    instance.advance(60);
    assert_eq!(instance.device_signed(&phone, "GET", "/v1/devices/me", None, None).await.status, 200, "a new minute");
}

#[tokio::test]
async fn requests_that_dont_verify_are_limited_by_their_address() {
    let instance = start().await;
    let phone = Device::new("198.51.109.1");
    instance.sign_in_device(&phone, "matt@example.com", "iPhone").await;
    instance.advance(60 - instance.now().rem_euclid(60));
    let from = |ip: &str| [("x-forwarded-for", ip.to_owned())];
    let ago = std::cell::Cell::new(0);
    let at = || {
        ago.set(ago.get() + 1);
        Some(instance.now() - ago.get())
    };
    let stranger = Device::new("198.51.109.1");
    for number in 0..10 {
        // Unsigned, signed by a key that isn't signed in, or for an app
        // (none runs here: refused before it's checked).
        assert_eq!(instance.send("GET", "/v1/devices/me", Vec::new(), &from("198.51.109.1")).await.error(), "unauthorized");
        assert_eq!(instance.device_signed(&stranger, "GET", "/v1/claktories", None, at()).await.error(), "unauthorized", "{number}");
        assert_eq!(instance.send("GET", "/v1/apps/dms/sync", Vec::new(), &from("198.51.109.1")).await.status, 503);
    }
    // Spent, the address's signed requests are refused before they're
    // checked, until the next minute: unsigned, or naming a key or token
    // no one has.
    let answer = instance.send("GET", "/v1/devices/me", Vec::new(), &from("198.51.109.1")).await;
    assert_eq!(answer.error(), "rate_limited");
    let answer = instance.device_signed(&stranger, "GET", "/v1/claktories", None, at()).await;
    assert_eq!((answer.status, answer.error().as_str()), (429, "rate_limited"));
    assert_eq!(answer.value()["retry_after"], 60);
    let nobody = Claktory { token: "not-a-token".into(), ..Claktory::new("198.51.109.1") };
    let check_in = CheckIn { version: "v0.1.9".into(), platform: "macos-arm64".into(), ..Default::default() };
    assert_eq!(instance.signed(&nobody, "POST", "/v1/check-in", &check_in, at()).await.error(), "rate_limited");
    // A device or Claktory it knows is still checked, and goes on: someone
    // else on a carrier's NAT can't lock a person's phone out. Naming one
    // with a bad signature is refused as ever.
    assert_eq!(instance.device_signed(&phone, "GET", "/v1/devices/me", None, at()).await.status, 200, "a known device");
    let forger = Device { key: stranger.key.clone(), ip: phone.ip.clone() };
    let signature = sign_request(&forger.key, "GET", "/v1/devices/me", instance.now(), b"");
    let forged = [
        ("authorization", format!("{DEVICE_AUTH} {}", phone.account_key())),
        (TIME_HEADER, instance.now().to_string()),
        (SIGNATURE_HEADER, signature),
        ("x-forwarded-for", phone.ip.clone()),
    ];
    assert_eq!(instance.send("GET", "/v1/devices/me", Vec::new(), &forged).await.error(), "unauthorized");
    let mut mac = Claktory::new("198.51.109.3");
    instance.sign_in(&mut mac, "matt@example.com", "Mac mini").await;
    let mac = Claktory { ip: "198.51.109.1".into(), ..mac };
    assert_eq!(instance.signed(&mac, "POST", "/v1/check-in", &check_in, at()).await.status, 200, "a known Claktory");
    let answer = instance.send("GET", "/.well-known/claktory-account", Vec::new(), &from("198.51.109.1")).await;
    assert_eq!(answer.status, 200, "unsigned routes count on their own");
    instance.advance(60);
    assert_eq!(instance.device_signed(&stranger, "GET", "/v1/claktories", None, None).await.error(), "unauthorized", "a new minute");
}

#[tokio::test]
async fn without_a_trusted_proxy_its_header_is_ignored() {
    let instance = start_with(Config::default()).await;
    let mut claktory = Claktory::new("203.0.113.9");
    instance.sign_in(&mut claktory, "matt@example.com", "Mac mini").await;
    instance.advance(16 * 60);
    instance.share(&claktory, None).await.expect("a share");
    let notice = instance.mail.sent().pop().expect("a notice");
    assert!(notice.text.contains("From: 127.0.0.1\n"), "{}", notice.text);
}

#[tokio::test]
async fn the_days_email_cap_stops_codes_but_not_notices() {
    let config = Config { trust_proxy: true, email_daily_cap: 5, admins: vec!["ops@example.com".into()], ..Config::default() };
    let instance = start_with(config).await;
    let mut claktory = Claktory::new("198.51.105.1");
    instance.sign_in(&mut claktory, "matt@example.com", "Mac mini").await;
    for number in 0..3 {
        let other = Claktory::new(&format!("198.51.105.{}", number + 2));
        assert_eq!(instance.start_sign_in(&other, &format!("p{number}@example.com"), "Mac").await.status, 200);
    }
    // The fourth email of five is 80%: the admin hears of it.
    let alert = instance.mail.sent().into_iter().find(|mail| mail.to == "ops@example.com").expect("an alert");
    assert_eq!(alert.subject, "Claktory account service: 4 of 5 emails sent today");
    let other = Claktory::new("198.51.105.9");
    assert_eq!(instance.start_sign_in(&other, "p9@example.com", "Mac").await.status, 200);
    let answer = instance.start_sign_in(&Claktory::new("198.51.105.10"), "p10@example.com", "Mac").await;
    assert_eq!((answer.status, answer.error().as_str()), (503, "email_unavailable"));

    instance.advance(16 * 60);
    instance.share(&claktory, None).await.expect("a share");
    assert_eq!(instance.mail.sent().pop().expect("a notice").subject, "Mac mini unlocked its secrets");

    instance.advance(86_400);
    assert_eq!(instance.start_sign_in(&Claktory::new("198.51.105.11"), "p11@example.com", "Mac").await.status, 200, "a new day");
}

#[tokio::test]
async fn a_provider_refusing_to_send_is_told_to_whoever_runs_it_once_a_day() {
    let config = Config { trust_proxy: true, admins: vec!["ops@example.com".into()], ..Config::default() };
    let instance = start_with(config).await;
    let refuse =
        || mail::MailError::Refused { kind: "not_entitled", text: "The Cloudflare account isn't entitled to Email Sending.".into() };
    for number in 0..2 {
        *instance.mail.refuse_next.lock().unwrap() = Some(refuse());
        let answer = instance.start_sign_in(&Claktory::new(&format!("198.51.107.{}", number + 1)), "p@example.com", "Mac").await;
        assert_eq!((answer.status, answer.error().as_str()), (503, "email_unavailable"));
    }
    let alerts: Vec<_> = instance.mail.sent().into_iter().filter(|mail| mail.to == "ops@example.com").collect();
    assert_eq!(alerts.len(), 1, "once a day");
    assert_eq!(alerts[0].subject, "Claktory account service: email refused");
    assert!(alerts[0].text.contains("isn't entitled to Email Sending"), "{}", alerts[0].text);
    let conn = rusqlite::Connection::open(&instance.db).unwrap();
    let report = claktory_account::server::stats::report(&conn, START).unwrap();
    assert!(report.contains("Emails the mail provider refused, today and yesterday (UTC):\n  not_entitled: 2"), "{report}");
}

#[tokio::test]
async fn a_mail_outage_refuses_codes_but_still_unlocks() {
    let instance = start().await;
    let mut claktory = Claktory::new("198.51.106.1");
    instance.sign_in(&mut claktory, "matt@example.com", "Mac mini").await;
    instance.mail.fail.store(true, Ordering::SeqCst);
    let answer = instance.start_sign_in(&Claktory::new("198.51.106.2"), "p@example.com", "Mac").await;
    assert_eq!((answer.status, answer.error().as_str()), (503, "email_unavailable"));
    // An unlock that's mailed (later than just after signing in, from a new
    // network) still gets its share while no mail goes.
    instance.advance(16 * 60);
    assert!(instance.share(&claktory, None).await.is_ok());
    // …and is recorded as not emailed, since it wasn't;
    // once mail works again, the next unusual one is.
    instance.mail.fail.store(false, Ordering::SeqCst);
    claktory.ip = "203.0.113.9".into();
    instance.advance(3600);
    assert!(instance.share(&claktory, None).await.is_ok());
    let conn = rusqlite::Connection::open(&instance.db).unwrap();
    let mailed: Vec<bool> =
        conn.prepare("SELECT mailed FROM share_releases ORDER BY time").unwrap().query_map([], |row| row.get(0)).unwrap().map(Result::unwrap).collect();
    assert_eq!(mailed, [false, true]);
}
