//! The store reviewers' account (`--review-account`): its email signs in
//! with a fixed code on every route that takes one, is sent nothing, and
//! locks for the hour after 5 wrong codes. Off, it's an email like any.

use claktory_account::protocol::*;
use claktory_account::server::Config;
use claktory_account::server::review::ReviewAccount;
use serde_json::json;

use crate::support::*;
use crate::web::{field, get};

const FORM: &str = "application/x-www-form-urlencoded";

/// Made up for the tests, as the operator's wrapper makes one.
const EMAIL: &str = "review-0123456789abcdef@example.com";
const CODE: &str = "424242";

async fn with_review() -> Instance {
    let review = ReviewAccount::new(EMAIL, "saltsaltsaltsalt", CODE).unwrap();
    start_with(Config { trust_proxy: true, review_account: Some(review), ..Config::default() }).await
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
    let instance = with_review().await;

    // A Claktory (the review box).
    let mut review_box = Claktory::new("198.51.100.60");
    let started: SignInStarted = instance.start_sign_in(&review_box, EMAIL, "Review Mac").await.json();
    let ephemeral = Ephemeral::new();
    let verify = SignInVerify { request_id: started.request_id, code: format!(" {CODE} "), eph_pub: ephemeral.public_b64() };
    let answer = instance.post("/v1/sign-in/verify", &verify, &review_box.ip).await;
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

    // The web pages, the account page and Delete account's among them.
    let body = format!("email={}", EMAIL.replace('@', "%40")).into_bytes();
    let page = instance
        .send("POST", "/account/sign-in", body, &[("content-type", FORM.into()), ("x-forwarded-for", "198.51.100.62".into())])
        .await;
    let request = field(&page.text(), "request");
    let body = format!("request={request}&email={}&code={CODE}", EMAIL.replace('@', "%40")).into_bytes();
    let answer =
        instance.send("POST", "/account/verify", body, &[("content-type", FORM.into()), ("x-forwarded-for", "198.51.100.62".into())]).await;
    assert_eq!(answer.status, 303, "{}", answer.text());
    let cookie = answer.headers.get("set-cookie").and_then(|value| value.to_str().ok()).unwrap().split(';').next().unwrap().to_owned();
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
    let instance = with_review().await;
    let mut review_box = Claktory::new("198.51.100.60");
    let started: SignInStarted = instance.start_sign_in(&review_box, EMAIL, "Review Mac").await.json();
    let verify = SignInVerify { request_id: started.request_id, code: CODE.into(), eph_pub: Ephemeral::new().public_b64() };
    review_box.token = instance.post("/v1/sign-in/verify", &verify, &review_box.ip).await.json::<SignedIn>().machine_token;
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
