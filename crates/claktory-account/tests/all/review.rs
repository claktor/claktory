//! The store reviewers' accounts (`--review-account`, one per store): each
//! email signs in with its fixed code on every route that takes one, is
//! sent nothing, and locks for the hour after 5 wrong codes. It signs in no
//! Claktory but the review box, and its account isn't deleted nor loses its
//! Claktories on the web pages (#319). Off, it's an email like any. A
//! service for the reviewers alone (`--review-only`) signs in no one else.

use claktory_account::protocol::*;
use claktory_account::server::Config;
use claktory_account::server::review::ReviewAccount;
use serde_json::json;

use crate::support::*;
use crate::web::{field, get, post_form};

const FORM: &str = "application/x-www-form-urlencoded";

/// Made up for the tests, as the operator's wrapper makes one.
const EMAIL: &str = "review-0123456789abcdef@example.com";
const CODE: &str = "424242";

/// The second store's, as the wrapper makes them: each its own pair.
const EMAIL_2: &str = "review-fedcba9876543210@example.com";
const CODE_2: &str = "171717";

/// The service with the review account on, and `review_box` the Claktory
/// its email may sign in.
async fn with_review_box(review_box: &Claktory) -> Instance {
    let review = ReviewAccount::new(EMAIL, "saltsaltsaltsalt", CODE).unwrap();
    let key = b64(review_box.key.verifying_key().to_bytes());
    start_with(Config { trust_proxy: true, review_accounts: vec![review], review_claktory_key: Some(key), ..Config::default() }).await
}

/// The service with the review account on and no review box.
async fn with_review() -> Instance {
    with_review_box(&Claktory::new("198.51.100.1")).await
}

/// Signs the review box in with the fixed code, its verify signed by its
/// key as factoryd signs it; its answer.
async fn sign_in_box(instance: &Instance, review_box: &Claktory) -> Answer {
    let answer = instance.start_sign_in(review_box, EMAIL, "Review Mac").await;
    assert_eq!(answer.status, 200, "{}", answer.text());
    let started: SignInStarted = answer.json();
    verify_signed(instance, &review_box.ip, &started.request_id, Some(&review_box.key)).await
}

/// A Claktory's verify with the fixed code, signed by `key` (no token)
/// or not at all.
async fn verify_signed(instance: &Instance, ip: &str, request_id: &str, key: Option<&ed25519_dalek::SigningKey>) -> Answer {
    let verify = SignInVerify { request_id: request_id.to_owned(), code: format!(" {CODE} "), eph_pub: Ephemeral::new().public_b64() };
    let body = serde_json::to_vec(&verify).unwrap();
    let mut headers = vec![("x-forwarded-for", ip.to_owned())];
    if let Some(key) = key {
        let time = instance.now();
        headers.push((TIME_HEADER, time.to_string()));
        headers.push((SIGNATURE_HEADER, sign_request(key, "POST", "/v1/sign-in/verify", time, &body)));
    }
    instance.send("POST", "/v1/sign-in/verify", body, &headers).await
}

/// Signs `email` in on the account page with `code`; the session cookie.
async fn web_sign_in_with(instance: &Instance, email: &str, code: &str) -> String {
    let body = format!("email={}", email.replace('@', "%40")).into_bytes();
    let page = instance
        .send("POST", "/account/sign-in", body, &[("content-type", FORM.into()), ("x-forwarded-for", "198.51.100.62".into())])
        .await;
    let request = field(&page.text(), "request");
    let body = format!("request={request}&email={}&code={code}", email.replace('@', "%40")).into_bytes();
    let answer =
        instance.send("POST", "/account/verify", body, &[("content-type", FORM.into()), ("x-forwarded-for", "198.51.100.62".into())]).await;
    assert_eq!(answer.status, 303, "{}", answer.text());
    answer.headers.get("set-cookie").and_then(|value| value.to_str().ok()).unwrap().split(';').next().unwrap().to_owned()
}

fn mails_to(instance: &Instance, email: &str) -> usize {
    instance.mail.sent().iter().filter(|mail| mail.to == email).count()
}

async fn device_verify(instance: &Instance, device: &Device, request_id: &str, code: &str) -> Answer {
    let verify = json!({"request_id": request_id, "code": code});
    instance.device_signed(device, "POST", "/v1/devices/sign-in/verify", Some(&verify), None).await
}

async fn device_start(instance: &Instance, device: &Device) -> String {
    let answer = instance.start_device_sign_in(device, &EMAIL.to_uppercase(), "Review iPhone").await;
    assert_eq!(answer.status, 200, "{}", answer.text());
    answer.json::<SignInStarted>().request_id
}

#[tokio::test]
async fn the_review_account_signs_in_with_its_code_on_every_route_and_is_sent_nothing() {
    // A Claktory (the review box).
    let mut review_box = Claktory::new("198.51.100.60");
    let instance = with_review_box(&review_box).await;
    let answer = sign_in_box(&instance, &review_box).await;
    assert_eq!(answer.status, 200, "{}", answer.text());
    let signed_in: SignedIn = answer.json();
    assert_eq!(signed_in.email, EMAIL);
    review_box.token = signed_in.machine_token.clone();
    // Its unlock notice isn't sent either.
    assert!(instance.share(&review_box, None).await.is_ok());

    // A device.
    let phone = Device::new("198.51.100.61");
    let request_id = device_start(&instance, &phone).await;
    let answer = device_verify(&instance, &phone, &request_id, CODE).await;
    assert_eq!(answer.json::<DeviceAccount>().account_id, signed_in.account_id);

    // The web pages.
    let cookie = web_sign_in_with(&instance, EMAIL, CODE).await;
    assert!(get(&instance, "/account", &cookie).await.text().contains("Review Mac"));

    assert_eq!(mails_to(&instance, EMAIL), 0, "nothing goes to it");

    // Anyone else gets an emailed code, as always, and the fixed code
    // doesn't open their sign-in.
    let other = Device::new("198.51.100.63");
    let answer = instance.start_device_sign_in(&other, "ana@example.com", "Ana's iPhone").await;
    let request_id = answer.json::<SignInStarted>().request_id;
    assert_eq!(mails_to(&instance, "ana@example.com"), 1);
    if instance.last_code("ana@example.com") != CODE {
        assert_eq!(device_verify(&instance, &other, &request_id, CODE).await.error(), "wrong_code");
    }
    assert_eq!(device_verify(&instance, &other, &request_id, &instance.last_code("ana@example.com")).await.status, 200);
}

#[tokio::test]
async fn five_wrong_codes_lock_the_review_account_for_the_hour() {
    let instance = with_review().await;
    instance.advance(3600 - instance.now().rem_euclid(3600));
    // Wrong codes from two addresses, on two sign-ins: 5 together.
    let (first, second) = (Device::new("198.51.100.70"), Device::new("198.51.100.71"));
    let request_id = device_start(&instance, &first).await;
    for code in ["000000", "111111", "222222"] {
        assert_eq!(device_verify(&instance, &first, &request_id, code).await.error(), "wrong_code");
    }
    let other_request = device_start(&instance, &second).await;
    for code in ["333333", "555555"] {
        assert_eq!(device_verify(&instance, &second, &other_request, code).await.error(), "wrong_code");
    }

    // Locked: the right code answers as a wrong one, from anywhere, and a
    // new sign-in is refused.
    let answer = device_verify(&instance, &first, &request_id, CODE).await;
    assert_eq!((answer.status, answer.error().as_str(), answer.value()["attempts_left"].as_u64()), (400, "wrong_code", Some(1)));
    let answer = instance.start_device_sign_in(&Device::new("198.51.100.72"), EMAIL, "iPhone").await;
    assert_eq!((answer.status, answer.error().as_str()), (429, "rate_limited"));

    // The next hour it works again.
    instance.advance(3600);
    let third = Device::new("198.51.100.73");
    let request_id = device_start(&instance, &third).await;
    assert_eq!(device_verify(&instance, &third, &request_id, CODE).await.status, 200);
}

#[tokio::test]
async fn without_the_option_the_review_email_is_an_email_like_any() {
    let instance = start().await;
    let phone = Device::new("198.51.100.80");
    let request_id = device_start(&instance, &phone).await;
    assert_eq!(mails_to(&instance, EMAIL), 1, "its code is emailed");
    if instance.last_code(EMAIL) != CODE {
        assert_eq!(device_verify(&instance, &phone, &request_id, CODE).await.error(), "wrong_code");
    }
    assert_eq!(device_verify(&instance, &phone, &request_id, &instance.last_code(EMAIL)).await.status, 200);

    // Its per-email limits too: the 6th sign-in in an hour is refused.
    instance.advance(3600 - instance.now().rem_euclid(3600));
    for at in 0..5 {
        device_start(&instance, &Device::new(&format!("198.51.100.{}", 81 + at))).await;
    }
    let answer = instance.start_device_sign_in(&Device::new("198.51.100.86"), EMAIL, "iPhone").await;
    assert_eq!((answer.status, answer.error().as_str()), (429, "rate_limited"));
}

/// A reviewer signs in on every device they try (Abbacchio's R3 was locked
/// out on its 6th): the review email has no per-email limits, as nothing is
/// sent to it. Its code is still kept by the per-IP limit and the lock after
/// 5 wrong codes an hour.
#[tokio::test]
async fn the_review_email_signs_in_on_many_devices_but_wrong_codes_and_one_ip_still_lock_it() {
    let instance = with_review().await;
    instance.advance(3600 - instance.now().rem_euclid(3600));
    // 10 sign-ins within the hour, each from its own address: all start,
    // and the last one too signs in.
    let mut last = None;
    for at in 0..10 {
        let device = Device::new(&format!("198.51.100.{}", 100 + at));
        let request_id = device_start(&instance, &device).await;
        last = Some((device, request_id));
    }
    let (device, request_id) = last.unwrap();
    assert_eq!(device_verify(&instance, &device, &request_id, CODE).await.status, 200);

    // 5 wrong codes still lock it for the hour: the right one answers as
    // wrong, and a new sign-in is refused.
    let (lock, other) = (Device::new("198.51.100.120"), Device::new("198.51.100.122"));
    let (request_id, other_request) = (device_start(&instance, &lock).await, device_start(&instance, &other).await);
    for code in ["000000", "111111", "222222"] {
        assert_eq!(device_verify(&instance, &lock, &request_id, code).await.error(), "wrong_code");
    }
    for code in ["333333", "555555"] {
        assert_eq!(device_verify(&instance, &other, &other_request, code).await.error(), "wrong_code");
    }
    assert_eq!(device_verify(&instance, &lock, &request_id, CODE).await.error(), "wrong_code");
    let answer = instance.start_device_sign_in(&Device::new("198.51.100.121"), EMAIL, "iPhone").await;
    assert_eq!((answer.status, answer.error().as_str()), (429, "rate_limited"));

    // The per-IP limit still holds: 30 sign-ins a day from one address.
    instance.advance(3600);
    let one = Device::new("198.51.100.130");
    for _ in 0..30 {
        let answer = instance.start_device_sign_in(&one, EMAIL, "iPhone").await;
        assert_eq!(answer.status, 200, "{}", answer.text());
    }
    let answer = instance.start_device_sign_in(&one, EMAIL, "iPhone").await;
    assert_eq!((answer.status, answer.error().as_str()), (429, "rate_limited"));
    assert_eq!(mails_to(&instance, EMAIL), 0, "nothing goes to it");
}

/// Fugo 27568: the review account's unlocks from a new network aren't
/// mailed, and are recorded as not, so its account page doesn't say
/// "emailed you"; an ordinary account's still are.
#[tokio::test]
async fn the_review_accounts_unlocks_are_never_mailed_nor_recorded_as_mailed() {
    let mut review_box = Claktory::new("198.51.100.60");
    let instance = with_review_box(&review_box).await;
    review_box.token = sign_in_box(&instance, &review_box).await.json::<SignedIn>().machine_token;
    let mut ordinary = Claktory::new("203.0.113.7");
    instance.sign_in(&mut ordinary, "ana@example.com", "Mac mini").await;

    // Not just after signing in, each from a network it hasn't unlocked from.
    instance.advance(16 * 60);
    for ip in ["198.51.100.60", "192.0.2.9"] {
        review_box.ip = ip.into();
        instance.share(&review_box, None).await.expect("a share");
        instance.advance(3600);
    }
    instance.share(&ordinary, None).await.expect("a share");

    assert_eq!(mails_to(&instance, EMAIL), 0, "nothing goes to it");
    let notices = instance.mail.sent().into_iter().filter(|mail| mail.subject.ends_with("unlocked its secrets")).count();
    assert_eq!(notices, 1, "Ana's unlock is mailed");
    let conn = rusqlite::Connection::open(&instance.db).expect("the database");
    let mailed: Vec<(String, bool)> = conn
        .prepare("SELECT c.name, r.mailed FROM share_releases r JOIN claktories c ON c.id = r.claktory_id ORDER BY r.time")
        .unwrap()
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap();
    let expected = [("Review Mac", false), ("Review Mac", false), ("Mac mini", true)];
    assert_eq!(mailed, expected.map(|(name, mailed)| (name.to_owned(), mailed)));
}

#[test]
fn the_file_is_off_when_missing_and_refused_when_others_can_read_it() {
    use std::os::unix::fs::PermissionsExt as _;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("review-account");
    assert!(ReviewAccount::read(&path).unwrap().is_none(), "off");
    // printf '%s%s' saltsaltsaltsalt 424242 | sha256sum
    let line = "review-0123456789abcdef@example.com saltsaltsaltsalt d42519fc0aac310889bc1c5f066af5951f55ae2d0a83fa905a1a4d7c5779aebf\n";
    std::fs::write(&path, line).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
    assert!(ReviewAccount::read(&path).unwrap_err().contains("readable by others"));
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
    let review = ReviewAccount::read(&path).unwrap().expect("on");
    assert!(review.is(EMAIL));
    assert_eq!(review.digest_of(CODE), review.digest(), "the shell's digest is the service's");
}

#[test]
fn the_service_takes_two_review_accounts_at_most_each_its_own_email() {
    let account = |email: &str| ReviewAccount::new(email, "saltsaltsaltsalt", CODE).unwrap();
    assert!(ReviewAccount::check_all(&[]).is_ok());
    assert!(ReviewAccount::check_all(&[account(EMAIL), account(EMAIL_2)]).is_ok());
    assert!(ReviewAccount::check_all(&[account(EMAIL), account(&EMAIL.to_uppercase())]).unwrap_err().contains("same email"));
    let third = "review-00000000000000ff@example.com";
    assert!(ReviewAccount::check_all(&[account(EMAIL), account(EMAIL_2), account(third)]).unwrap_err().contains("at most 2"));
}

/// App Store's and Play's accounts, each rotated on its own (#319): each
/// signs in with its own code only, and neither is sent anything.
#[tokio::test]
async fn two_review_accounts_each_sign_in_with_their_own_code() {
    let reviews = vec![
        ReviewAccount::new(EMAIL, "saltsaltsaltsalt", CODE).unwrap(),
        ReviewAccount::new(EMAIL_2, "pepperpepperpepper", CODE_2).unwrap(),
    ];
    let instance = start_with(Config { trust_proxy: true, review_accounts: reviews, ..Config::default() }).await;

    let mut accounts = Vec::new();
    for (email, code, other) in [(EMAIL, CODE, CODE_2), (EMAIL_2, CODE_2, CODE)] {
        let phone = Device::new("198.51.100.90");
        let answer = instance.start_device_sign_in(&phone, email, "Review iPhone").await;
        let request_id = answer.json::<SignInStarted>().request_id;
        assert_eq!(device_verify(&instance, &phone, &request_id, other).await.error(), "wrong_code", "the other store's code");
        let answer = device_verify(&instance, &phone, &request_id, code).await;
        assert_eq!(answer.status, 200, "{}", answer.text());
        accounts.push(answer.json::<DeviceAccount>().account_id);
    }
    assert_ne!(accounts[0], accounts[1], "two accounts");
    assert_eq!(mails_to(&instance, EMAIL) + mails_to(&instance, EMAIL_2), 0, "nothing goes to either");
}

/// Fugo 34608: the details the stores hold are no relay credential. A
/// review email signs in only the review box's key, and the box moves to
/// no other; any other email signs in any Claktory, as always.
#[tokio::test]
async fn a_review_email_signs_in_only_the_review_box() {
    let mut review_box = Claktory::new("198.51.100.60");
    let instance = with_review_box(&review_box).await;

    let stranger = Claktory::new("203.0.113.20");
    let answer = instance.start_sign_in(&stranger, &EMAIL.to_uppercase(), "Someone's Mac").await;
    assert_eq!((answer.status, answer.error().as_str()), (403, "review_account"));
    let mut ordinary = Claktory::new("203.0.113.21");
    instance.sign_in(&mut ordinary, "ana@example.com", "Mac mini").await;

    let answer = sign_in_box(&instance, &review_box).await;
    assert_eq!(answer.status, 200, "{}", answer.text());
    let signed_in: SignedIn = answer.json();
    review_box.token = signed_in.machine_token;
    review_box.epoch = signed_in.epoch;

    // Fugo 46276: the box's key is public, so naming it proves nothing. A
    // sign-in naming the box's id and key, verified unsigned or signed by
    // another key, is refused and its code spent: the box keeps its
    // registration, and no other takes a place on the account.
    for key in [None, Some(new_signing_key())] {
        let impostor =
            Claktory { id: review_box.id.clone(), key: review_box.key.clone(), token: String::new(), epoch: 0, ip: "203.0.113.30".into() };
        let started: SignInStarted = instance.start_sign_in(&impostor, EMAIL, "Review Mac").await.json();
        let answer = verify_signed(&instance, &impostor.ip, &started.request_id, key.as_ref()).await;
        assert_eq!((answer.status, answer.error().as_str()), (403, "review_account"), "{}", answer.text());
        let answer = verify_signed(&instance, &impostor.ip, &started.request_id, Some(&review_box.key)).await;
        assert_eq!(answer.error(), "code_expired", "the code is spent");
    }
    assert!(instance.share(&review_box, None).await.is_ok(), "the box is still signed in");
    let fresh = Claktory {
        id: Claktory::new("203.0.113.31").id,
        key: review_box.key.clone(),
        token: String::new(),
        epoch: 0,
        ip: "203.0.113.31".into(),
    };
    let started: SignInStarted = instance.start_sign_in(&fresh, EMAIL, "Review Mac").await.json();
    assert_eq!(verify_signed(&instance, &fresh.ip, &started.request_id, None).await.error(), "review_account");

    // Moved to another key, it would be another Claktory on the account.
    let request = MoveRequest {
        backend_key: b64(new_signing_key().verifying_key().as_bytes()),
        machine_token: b64(*random_32()),
        name: "elsewhere".into(),
        version: "v0.1.15".into(),
        platform: "linux-x86_64".into(),
        from: "Review Mac".into(),
    };
    let answer = instance.signed(&review_box, "POST", "/v1/move", &request, None).await;
    assert_eq!((answer.status, answer.error().as_str()), (403, "review_account"));
    // A minute later, past the share's own limit.
    instance.advance(60);
    if let Err(answer) = instance.share(&review_box, None).await {
        panic!("the box keeps working: {}", answer.text());
    }

    // Without a review box, none signs in with it.
    let instance = start_with(Config {
        trust_proxy: true,
        review_accounts: vec![ReviewAccount::new(EMAIL, "saltsaltsaltsalt", CODE).unwrap()],
        ..Config::default()
    })
    .await;
    let answer = instance.start_sign_in(&review_box, EMAIL, "Review Mac").await;
    assert_eq!((answer.status, answer.error().as_str()), (403, "review_account"));
}

/// Fugo 34608: a reviewer trying Remove doesn't take away the box the
/// next review connects to, so the web pages don't remove a review
/// account's Claktory. Delete account works, as Apple's review needs (Bruno
/// 46782): the account comes back at the next sign-in, and only the review
/// box signs a Claktory in to it again.
#[tokio::test]
async fn a_review_account_keeps_its_claktory_on_the_web_and_comes_back_after_its_deletion() {
    let mut review_box = Claktory::new("198.51.100.60");
    let instance = with_review_box(&review_box).await;
    let first = sign_in_box(&instance, &review_box).await.json::<SignedIn>();
    review_box.token = first.machine_token;
    let cookie = web_sign_in_with(&instance, EMAIL, CODE).await;

    let page = get(&instance, "/account", &cookie).await.text();
    assert!(page.contains("Review Mac"), "{page}");
    assert!(!page.contains("/account/remove\""), "no Remove: {page}");
    assert!(page.contains("/account/delete"), "Delete account is there: {page}");
    let csrf = field(&page, "csrf");
    let answer = post_form(&instance, "/account/remove", &cookie, &format!("csrf={csrf}&claktory={}&confirm=yes", review_box.id)).await;
    assert_eq!(answer.status, 403, "{}", answer.text());
    assert!(answer.text().contains("can't be removed"), "{}", answer.text());
    assert!(instance.share(&review_box, None).await.is_ok(), "the box is still signed in");

    let answer = post_form(&instance, "/account/delete", &cookie, &format!("csrf={csrf}&confirm=yes")).await;
    assert!(answer.text().contains("Your account was deleted."), "{}", answer.text());
    instance.advance(60);
    assert!(instance.share(&review_box, None).await.is_err(), "the box went with the account");

    // It comes back: the box signs in again, a new account; any other
    // Claktory still can't.
    let again = sign_in_box(&instance, &review_box).await;
    assert_eq!(again.status, 200, "{}", again.text());
    assert_ne!(again.json::<SignedIn>().account_id, first.account_id, "a new account");
    let stranger = Claktory::new("203.0.113.50");
    let answer = instance.start_sign_in(&stranger, EMAIL, "Someone's Mac").await;
    assert_eq!(answer.error(), "review_account");
}

async fn relay_token(instance: &Instance, claktory: &Claktory) -> Answer {
    instance.signed(claktory, "POST", "/v1/relay/token", &RelayTokenRequest { endpoint_id: "ab".repeat(32) }, None).await
}

/// Fugo 46276: a review account's Claktory under another key than the
/// review box's (one signed in before the pin) gets no relay token; the
/// box does.
#[tokio::test]
async fn only_the_review_box_gets_a_relay_token_for_a_review_account() {
    let mut review_box = Claktory::new("198.51.100.60");
    let instance = with_review_box(&review_box).await;
    review_box.token = sign_in_box(&instance, &review_box).await.json::<SignedIn>().machine_token;
    assert_eq!(relay_token(&instance, &review_box).await.status, 200);

    // As before the pin: a Claktory on the account with a key of its own.
    let mut earlier = Claktory::new("203.0.113.40");
    instance.sign_in(&mut earlier, "earlier@example.com", "Old review box").await;
    let conn = rusqlite::Connection::open(&instance.db).expect("the database");
    conn.execute("DELETE FROM accounts WHERE email = ?1", [EMAIL]).unwrap();
    conn.execute("UPDATE accounts SET email = ?1 WHERE email = 'earlier@example.com'", [EMAIL]).unwrap();
    let answer = relay_token(&instance, &earlier).await;
    assert_eq!((answer.status, answer.error().as_str()), (403, "review_account"), "{}", answer.text());
}

/// Fugo 46822: the box's key is pinned, so a reviewer signing it out
/// (#524) doesn't keep it out: it signs in to the review email again with
/// the same key, its verify still signed by it, and gets a relay token.
/// Any other retired key stays retired, the box's too with another email.
#[tokio::test]
async fn the_review_box_signs_in_again_with_its_pinned_key_after_signing_out() {
    let mut review_box = Claktory::new("198.51.100.60");
    let instance = with_review_box(&review_box).await;
    let first = sign_in_box(&instance, &review_box).await.json::<SignedIn>();
    review_box.token = first.machine_token;
    review_box.epoch = first.epoch;
    let answer = instance.signed(&review_box, "POST", "/v1/claktories/sign-out", &json!({}), None).await;
    assert_eq!(answer.status, 200, "{}", answer.text());
    assert_eq!(relay_token(&instance, &review_box).await.status, 401, "signed out");

    // With any email but a review one, its key is retired as any is.
    let answer = instance.start_sign_in(&review_box, "ana@example.com", "Review Mac").await;
    assert_eq!((answer.status, answer.error().as_str()), (409, "backend_key_retired"), "{}", answer.text());

    // Unsigned, its verify is refused as before.
    let started: SignInStarted = instance.start_sign_in(&review_box, EMAIL, "Review Mac").await.json();
    let answer = verify_signed(&instance, &review_box.ip, &started.request_id, None).await;
    assert_eq!((answer.status, answer.error().as_str()), (403, "review_account"), "{}", answer.text());

    // Signed: it's back, the same Claktory at the next epoch, on the relay.
    instance.advance(1);
    let answer = sign_in_box(&instance, &review_box).await;
    assert_eq!(answer.status, 200, "{}", answer.text());
    let again: SignedIn = answer.json();
    assert_eq!((again.claktory_id.as_str(), again.epoch), (review_box.id.as_str(), first.epoch + 1));
    review_box.token = again.machine_token;
    review_box.epoch = again.epoch;
    let answer = relay_token(&instance, &review_box).await;
    assert_eq!(answer.status, 200, "{}", answer.text());

    // An ordinary email's retired key: 409, as always.
    let mut ordinary = Claktory::new("203.0.113.60");
    instance.sign_in(&mut ordinary, "ana@example.com", "Mac mini").await;
    instance.advance(1);
    assert_eq!(instance.signed(&ordinary, "POST", "/v1/claktories/sign-out", &json!({}), None).await.status, 200);
    let answer = instance.start_sign_in(&ordinary, "ana@example.com", "Mac mini").await;
    assert_eq!((answer.status, answer.error().as_str()), (409, "backend_key_retired"), "{}", answer.text());

    // A retired key that isn't the box's, with the review email: refused.
    let answer = instance.start_sign_in(&ordinary, EMAIL, "Mac mini").await;
    assert_eq!((answer.status, answer.error().as_str()), (409, "backend_key_retired"), "{}", answer.text());
}

/// A service for the store reviewers alone (`--review-only`): the review
/// accounts sign in as before, and every other email is refused on every
/// route, with nothing sent, so it's no account service for anyone else.
#[tokio::test]
async fn a_review_only_service_signs_in_no_one_but_the_reviewers() {
    let mut review_box = Claktory::new("198.51.100.70");
    let review = ReviewAccount::new(EMAIL, "saltsaltsaltsalt", CODE).unwrap();
    let key = b64(review_box.key.verifying_key().to_bytes());
    let config =
        Config { trust_proxy: true, review_accounts: vec![review], review_claktory_key: Some(key), review_only: true, ..Config::default() };
    let instance = start_with(config).await;

    // The reviewers: the box, a phone and the web pages.
    let answer = sign_in_box(&instance, &review_box).await;
    assert_eq!(answer.status, 200, "{}", answer.text());
    review_box.token = answer.json::<SignedIn>().machine_token;
    let phone = Device::new("198.51.100.71");
    let request_id = device_start(&instance, &phone).await;
    assert_eq!(device_verify(&instance, &phone, &request_id, CODE).await.status, 200);
    web_sign_in_with(&instance, EMAIL, CODE).await;

    // Anyone else, on each route.
    let other = Claktory::new("198.51.100.72");
    let answer = instance.start_sign_in(&other, "ana@example.com", "Ana's Mac").await;
    assert_eq!((answer.status, answer.error().as_str()), (403, "review_only"), "{}", answer.text());
    let answer = instance.start_device_sign_in(&Device::new("198.51.100.73"), "ana@example.com", "Ana's iPhone").await;
    assert_eq!((answer.status, answer.error().as_str()), (403, "review_only"), "{}", answer.text());
    let body = b"email=ana%40example.com".to_vec();
    let page = instance
        .send("POST", "/account/sign-in", body, &[("content-type", FORM.into()), ("x-forwarded-for", "198.51.100.74".into())])
        .await;
    assert_eq!(page.status, 403, "{}", page.text());
    assert_eq!(instance.mail.sent().len(), 0, "nothing is sent to anyone");
    let conn = rusqlite::Connection::open(&instance.db).expect("the database");
    let rows = |table: &str| -> i64 {
        conn.query_row(&format!("SELECT COUNT(*) FROM {table} WHERE email = 'ana@example.com'"), [], |row| row.get(0)).unwrap()
    };
    assert_eq!((rows("accounts"), rows("requests")), (0, 0), "and nothing is made");

    // A device bound through a Claktory (#529): the box may vouch, a
    // Claktory of the account under any other key may not.
    assert_eq!(vouch_for(&instance, &review_box).await.status, 200);
    instance.advance(1);
    // The box's registration under a key of its own, as one signed in
    // before the pin would be.
    let imposter = Claktory {
        id: review_box.id.clone(),
        key: new_signing_key(),
        token: review_box.token.clone(),
        epoch: review_box.epoch,
        ip: review_box.ip.clone(),
    };
    conn.execute(
        "UPDATE claktories SET backend_key = ?1 WHERE id = ?2",
        [b64(imposter.key.verifying_key().to_bytes()), review_box.id.clone()],
    )
    .unwrap();
    let vouches = || -> i64 { conn.query_row("SELECT COUNT(*) FROM requests WHERE kind = 'vouch'", [], |row| row.get(0)).unwrap() };
    let before = vouches();
    let answer = vouch_for(&instance, &imposter).await;
    assert_eq!((answer.status, answer.error().as_str()), (403, "review_account"), "{}", answer.text());
    assert_eq!(vouches(), before, "no code made");
}

/// `claktory` vouches for a new device (#529).
async fn vouch_for(instance: &Instance, claktory: &Claktory) -> Answer {
    let body = DeviceVouch {
        account_key: Device::new("198.51.100.75").account_key(),
        name: "Review Mac".into(),
        platform: "macos-arm64".into(),
        version: "v0.1.26".into(),
    };
    instance.signed(claktory, "POST", "/v1/devices/vouch", &body, None).await
}
