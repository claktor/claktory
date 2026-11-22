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
