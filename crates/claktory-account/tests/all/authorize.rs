//! A computer joins the account with the person's approval and no code: it
//! asks, the account's devices see what the service saw, one of them
//! approves with a Claktory's word that it's allowed there, and the
//! computer polls.

use claktory_account::protocol::*;
use serde_json::{Value, json};

use crate::support::*;

pub(crate) const EMAIL: &str = "matt@example.com";

/// A request signed by `computer`'s backend key, without a token.
pub(crate) async fn key_signed(instance: &Instance, computer: &Claktory, path: &str, body: &impl serde::Serialize) -> Answer {
    let body = serde_json::to_vec(body).expect("json");
    instance.advance(1);
    let time = instance.now();
    let headers = [
        (TIME_HEADER, time.to_string()),
        (SIGNATURE_HEADER, sign_request(&computer.key, "POST", path, time, &body)),
        ("x-forwarded-for", computer.ip.clone()),
    ];
    instance.send("POST", path, body, &headers).await
}

pub(crate) async fn ask(instance: &Instance, computer: &Claktory, email: &str, name: &str) -> Answer {
    let body = AuthorizeStart {
        email: email.to_owned(),
        claktory_id: computer.id.clone(),
        backend_key: b64(computer.key.verifying_key().to_bytes()),
        name: name.to_owned(),
        version: "v0.1.21".to_owned(),
        platform: "linux-x86_64".to_owned(),
        kind: KIND_CLAKTORY.to_owned(),
        receive: None,
    };
    key_signed(instance, computer, "/v1/claktories/authorize/start", &body).await
}

pub(crate) async fn poll(instance: &Instance, computer: &Claktory, request_id: &str) -> (Answer, Ephemeral) {
    let ephemeral = Ephemeral::new();
    let body = AuthorizePoll { request_id: request_id.to_owned(), eph_pub: ephemeral.public_b64() };
    (key_signed(instance, computer, "/v1/claktories/authorize/poll", &body).await, ephemeral)
}

/// `claktory`'s word that `device` is allowed on it, for `request_id`, at
/// `iat` (now if None).
pub(crate) fn word(instance: &Instance, claktory: &Claktory, account_id: &str, device: &Device, request_id: &str, iat: Option<i64>) -> String {
    let allowed = AllowedDevice {
        v: 1,
        account_id: account_id.to_owned(),
        claktory_id: claktory.id.clone(),
        account_key: device.account_key(),
        request_id: request_id.to_owned(),
        iat: iat.unwrap_or_else(|| instance.now()),
    };
    sign_allowed_device(&claktory.key, &allowed)
}

pub(crate) async fn shown(instance: &Instance, device: &Device) -> Vec<DeviceAuthorization> {
    instance.advance(1);
    let answer = instance.device_signed(device, "GET", "/v1/devices/authorizations", None, None).await;
    assert_eq!(answer.status, 200, "{}", answer.text());
    answer.json::<DeviceAuthorizations>().authorizations
}

pub(crate) async fn decide(instance: &Instance, device: &Device, id: &str, decision: Value) -> Answer {
    instance.advance(1);
    instance.device_signed(device, "POST", &format!("/v1/devices/authorizations/{id}"), Some(&decision), None).await
}

/// The person's setup: a Claktory signed in by email, and a phone.
pub(crate) async fn matts() -> (Instance, Claktory, Device, String) {
    let instance = start().await;
    let mut studio = Claktory::new("198.51.100.30");
    let account = instance.sign_in(&mut studio, EMAIL, "Mac mini").await.account_id;
    let phone = Device::new("192.0.2.80");
    instance.sign_in_device(&phone, EMAIL, "Matt's iPhone").await;
    (instance, studio, phone, account)
}

#[tokio::test]
async fn a_server_joins_once_the_person_approves_it_on_a_device_with_no_code_anywhere() {
    let (instance, studio, phone, account) = matts().await;
    let mut server = Claktory::new("203.0.113.9");
    let mails = instance.mail.sent().len();
    let answer = ask(&instance, &server, EMAIL, "hetzner\u{202E}box https://evil.example").await;
    assert_eq!(answer.status, 200, "{}", answer.text());
    let started: AuthorizeStarted = answer.json();
    assert_eq!((started.expires_in, started.interval), (600, 5));
    // The heads-up: no code, no link that approves anything.
    let sent = instance.mail.sent();
    assert_eq!(sent.len(), mails + 1);
    let heads_up = sent.last().expect("a heads-up");
    assert_eq!(heads_up.subject, "A computer asked to join your Claktory account");
    assert!(heads_up.text.contains("203.0.113.9") && !heads_up.text.contains("http"), "{}", heads_up.text);
    assert!(!heads_up.html.contains("<a ") && !heads_up.html.contains("href"), "no link");

    // The phone's card: what the service saw first, the name cleaned.
    let cards = shown(&instance, &phone).await;
    assert_eq!(cards.len(), 1);
    let card = &cards[0];
    assert_eq!((card.id.as_str(), card.kind.as_str(), card.state.as_str()), (started.request_id.as_str(), "claktory", "waiting"));
    assert_eq!(card.seen.ip, "203.0.113.9");
    assert_eq!(card.says.name, "hetznerbox", "no bidi override, no address");
    assert_eq!(card.says.platform, "linux-x86_64");
    assert!(card.may_approve && card.expires_in > 590);
    assert_eq!(card.fingerprint, key_fingerprint(&b64(server.key.verifying_key().to_bytes())));
    let card_fingerprint = card.fingerprint.clone();
    // The bell: a Claktory of the account sees its id, only.
    let waiting = instance.signed(&studio, "GET", "/v1/claktories/authorizations", &json!({}), None).await;
    assert_eq!(waiting.json::<ClaktoryAuthorizations>().waiting, std::slice::from_ref(&started.request_id));

    let (pending, _) = poll(&instance, &server, &started.request_id).await;
    assert_eq!((pending.status, pending.error()), (409, "authorization_pending".to_owned()));
    assert_eq!(pending.value()["interval"], 5);
    // Another key can't poll it.
    let (other, _) = poll(&instance, &Claktory::new("203.0.113.9"), &started.request_id).await;
    assert_eq!(other.status, 401);

    let allowed = word(&instance, &studio, &account, &phone, &started.request_id, None);
    let approved = decide(&instance, &phone, &card.id, json!({"approve": true, "fingerprint": card.fingerprint, "allowed": allowed})).await;
    assert_eq!(approved.status, 200, "{}", approved.text());
    let decided = instance.now();

    let (answer, ephemeral) = poll(&instance, &server, &started.request_id).await;
    assert_eq!(answer.status, 200, "{}", answer.text());
    let joined: Authorized = answer.json();
    assert_eq!(joined.first_device, phone.account_key(), "only the approver gets in at once");
    assert_eq!((joined.signed_in.account_id.as_str(), joined.signed_in.claktory_id.as_str()), (account.as_str(), server.id.as_str()));
    let share = ephemeral.open(&joined.signed_in.sealed_share, &server.id, joined.signed_in.epoch).expect("the share opens");
    assert_eq!(*share, instance.expected_share(&server, joined.signed_in.epoch));
    server.token = joined.signed_in.machine_token;
    let check_in = CheckIn { version: "v0.1.21".into(), platform: "linux-x86_64".into(), ..Default::default() };
    assert_eq!(instance.signed(&server, "POST", "/v1/check-in", &check_in, None).await.status, 200);
    // Spent: once only.
    let (again, _) = poll(&instance, &server, &started.request_id).await;
    assert_eq!((again.status, again.error()), (410, "expired_token".to_owned()));
    // A card left open on another device says how it was answered, and
    // where, for 10 minutes, with nothing to act on.
    let laptop = Device::new("192.0.2.81");
    instance.sign_in_device(&laptop, EMAIL, "Matt's MacBook").await;
    let cards = shown(&instance, &laptop).await;
    let [card] = cards.as_slice() else { panic!("{cards:?}") };
    assert_eq!((card.id.as_str(), card.state.as_str()), (started.request_id.as_str(), "approved"));
    assert_eq!((card.answered_by.as_str(), card.answered_at), ("Matt's iPhone", decided));
    assert_eq!((card.fingerprint.as_str(), card.may_approve, card.expires_in, card.endpoint.as_str()), ("", false, 0, ""));
    assert_eq!(card.seen.ip, "203.0.113.9", "still the computer it was");
    let late = decide(&instance, &laptop, &card.id, json!({"approve": false, "fingerprint": card.fingerprint})).await;
    assert_eq!(late.status, 410, "nothing to decide");
    instance.advance(600);
    assert!(shown(&instance, &phone).await.is_empty(), "gone after 10 minutes");
    // The Claktory that joined lists with the fingerprint its card showed,
    // so the device connects to that one only.
    let list: ClaktoryList = instance.device_signed(&phone, "GET", "/v1/claktories", None, None).await.json();
    let joined = list.claktories.iter().find(|listed| listed.claktory_id == server.id).expect("listed");
    assert_eq!(joined.fingerprint, card_fingerprint);
}

#[tokio::test]
async fn only_a_device_with_a_claktory_s_fresh_word_for_this_request_approves() {
    let (instance, studio, phone, account) = matts().await;
    let server = Claktory::new("203.0.113.9");
    let id = ask(&instance, &server, EMAIL, "box").await.json::<AuthorizeStarted>().request_id;
    let fingerprint = shown(&instance, &phone).await[0].fingerprint.clone();
    let approve = |allowed: Option<String>| json!({"approve": true, "fingerprint": fingerprint, "allowed": allowed});
    let refused = |answer: Answer| {
        assert_eq!((answer.status, answer.error()), (403, "not_allowed_to_approve".to_owned()), "{}", answer.text());
    };

    // No word, a word for another request, a stale one, one the server
    // signed itself, one from another account's Claktory.
    refused(decide(&instance, &phone, &id, approve(None)).await);
    refused(decide(&instance, &phone, &id, approve(Some(word(&instance, &studio, &account, &phone, "another", None)))).await);
    let stale = Some(word(&instance, &studio, &account, &phone, &id, Some(instance.now() - 200)));
    refused(decide(&instance, &phone, &id, approve(stale)).await);
    let laptop = Device::new("192.0.2.81");
    refused(decide(&instance, &phone, &id, approve(Some(word(&instance, &studio, &account, &laptop, &id, None)))).await);
    let mut forged = Claktory::new("203.0.113.9");
    forged.id = studio.id.clone();
    refused(decide(&instance, &phone, &id, approve(Some(word(&instance, &forged, &account, &phone, &id, None)))).await);
    let mut eves = Claktory::new("198.51.100.50");
    let eve_account = instance.sign_in(&mut eves, "eve@example.com", "Eve's Mac").await.account_id;
    refused(decide(&instance, &phone, &id, approve(Some(word(&instance, &eves, &eve_account, &phone, &id, None)))).await);
    // Another account's device can't see or decide it.
    let eve = Device::new("192.0.2.90");
    instance.sign_in_device(&eve, "eve@example.com", "Eve's phone").await;
    assert!(shown(&instance, &eve).await.is_empty());
    let theirs = decide(&instance, &eve, &id, json!({"approve": false, "fingerprint": fingerprint})).await;
    assert_eq!(theirs.status, 404);
    // Another computer than the card showed.
    let changed = decide(
        &instance,
        &phone,
        &id,
        json!({"approve": true, "fingerprint": "0000000000000000", "allowed": word(&instance, &studio, &account, &phone, &id, None)}),
    )
    .await;
    assert_eq!((changed.status, changed.error()), (409, "fingerprint_changed".to_owned()));
    // Nothing of that decided it.
    let (pending, _) = poll(&instance, &server, &id).await;
    assert_eq!(pending.error(), "authorization_pending");

    // Declined: by any device of the account, recorded with its address.
    // A Claktory signed in afresh since the request started doesn't vouch
    // for it: here the studio, signing in again with its key, proven.
    let mut studio = studio;
    instance.sign_in(&mut studio, EMAIL, "Mac mini").await;
    refused(decide(&instance, &phone, &id, approve(Some(word(&instance, &studio, &account, &phone, &id, None)))).await);

    // Here a tablet whose name would reorder a card's words.
    let tablet = Device::new("192.0.2.82");
    instance.sign_in_device(&tablet, EMAIL, "Matt's \u{202E}iPad https://evil.example").await;
    let declined = decide(&instance, &tablet, &id, json!({"approve": false, "fingerprint": fingerprint})).await;
    assert_eq!(declined.status, 200, "{}", declined.text());
    let (answer, _) = poll(&instance, &server, &id).await;
    assert_eq!((answer.status, answer.error()), (403, "access_denied".to_owned()));
    let logged: (String, String) = rusqlite::Connection::open(&instance.db)
        .expect("the database")
        .query_row("SELECT what, ip FROM authorization_log WHERE request_id = ?1", [&id], |row| Ok((row.get(0)?, row.get(1)?)))
        .expect("logged");
    assert_eq!(logged, ("declined".to_owned(), "192.0.2.82".to_owned()));
    let cards = shown(&instance, &phone).await;
    assert_eq!(
        cards.iter().map(|card| (card.state.as_str(), card.answered_by.as_str(), card.fingerprint.as_str())).collect::<Vec<_>>(),
        [("declined", "Matt's iPad", "")],
        "cleaned: no bidi override, no address"
    );
    let late = decide(&instance, &phone, &id, approve(Some(word(&instance, &studio, &account, &phone, &id, None)))).await;
    assert_eq!(late.status, 410);
}

#[tokio::test]
async fn a_device_a_claktory_vouched_for_may_decline_but_never_approve() {
    let (instance, studio, phone, account) = matts().await;
    // The app on the studio's own computer, vouched for: no email.
    let mac = Device::new("198.51.100.30");
    let vouch = json!({"account_key": mac.account_key(), "name": "Matt's Mac", "platform": "macos-arm64", "version": "v0.1.21"});
    let vouched: DeviceVouched = instance.signed(&studio, "POST", "/v1/devices/vouch", &vouch, None).await.json();
    let verify = json!({"request_id": vouched.request_id, "code": vouched.code});
    assert_eq!(instance.device_signed(&mac, "POST", "/v1/devices/sign-in/verify", Some(&verify), None).await.status, 200);

    let server = Claktory::new("203.0.113.9");
    let id = ask(&instance, &server, EMAIL, "box").await.json::<AuthorizeStarted>().request_id;
    let cards = shown(&instance, &mac).await;
    assert!(!cards[0].may_approve, "vouched for");
    assert!(shown(&instance, &phone).await[0].may_approve);
    let approve =
        json!({"approve": true, "fingerprint": cards[0].fingerprint, "allowed": word(&instance, &studio, &account, &mac, &id, None)});
    let refused = decide(&instance, &mac, &id, approve).await;
    assert_eq!((refused.status, refused.error()), (403, "not_allowed_to_approve".to_owned()));
}

#[tokio::test]
async fn two_computers_asking_at_once_both_end_and_say_why() {
    let (instance, _studio, phone, _) = matts().await;
    let first = Claktory::new("203.0.113.9");
    let second = Claktory::new("203.0.113.66");
    let id = ask(&instance, &first, EMAIL, "mine").await.json::<AuthorizeStarted>().request_id;
    let answer = ask(&instance, &second, EMAIL, "theirs").await;
    assert_eq!((answer.status, answer.error()), (409, "two_at_once".to_owned()));
    let (polled, _) = poll(&instance, &first, &id).await;
    assert_eq!((polled.status, polled.error()), (409, "two_at_once".to_owned()));
    let cards = shown(&instance, &phone).await;
    assert_eq!(
        cards.iter().map(|card| (card.says.name.as_str(), card.state.as_str())).collect::<Vec<_>>(),
        [("mine", "two_at_once"), ("theirs", "two_at_once")]
    );
    assert!(cards.iter().all(|card| card.expires_in == 0));
    let logged: String = rusqlite::Connection::open(&instance.db)
        .expect("the database")
        .query_row("SELECT ip FROM authorization_log WHERE what = 'two_at_once'", [], |row| row.get(0))
        .expect("logged");
    assert_eq!(logged, "203.0.113.66", "the address that asked second");
    // Shown for 10 minutes, then gone; and a new start then waits again.
    instance.advance(601);
    assert!(shown(&instance, &phone).await.is_empty());
    assert_eq!(ask(&instance, &first, EMAIL, "mine").await.status, 200);
}

#[tokio::test]
async fn nobody_to_approve_is_said_and_starts_are_limited() {
    let instance = start().await;
    let server = Claktory::new("203.0.113.9");
    // No account, then an account with no Claktory signed in, then one with
    // a Claktory and no device signed in by email.
    assert_eq!(ask(&instance, &server, "new@example.com", "box").await.error(), "no_device_to_approve");
    let phone = Device::new("192.0.2.80");
    instance.sign_in_device(&phone, EMAIL, "Matt's iPhone").await;
    assert_eq!(ask(&instance, &server, EMAIL, "box").await.error(), "no_device_to_approve");
    let mut studio = Claktory::new("198.51.100.30");
    instance.sign_in(&mut studio, "solo@example.com", "Mac mini").await;
    assert_eq!(ask(&instance, &server, "solo@example.com", "box").await.error(), "no_device_to_approve");
    // Unsigned, or signed by another key: refused before anything's kept.
    let body = AuthorizeStart {
        email: EMAIL.to_owned(),
        claktory_id: server.id.clone(),
        backend_key: b64(server.key.verifying_key().to_bytes()),
        name: "box".to_owned(),
        version: "v0.1.21".to_owned(),
        platform: "linux-x86_64".to_owned(),
        kind: KIND_CLAKTORY.to_owned(),
        receive: None,
    };
    assert_eq!(instance.post("/v1/claktories/authorize/start", &body, &server.ip).await.status, 401);
    let mut impostor = Claktory::new("203.0.113.9");
    impostor.id = server.id.clone();
    assert_eq!(key_signed(&instance, &impostor, "/v1/claktories/authorize/start", &body).await.status, 401);
    // 5 an hour per email.
    let instance = start().await;
    for _ in 0..5 {
        ask(&instance, &Claktory::new("203.0.113.10"), "limit@example.com", "box").await;
    }
    assert_eq!(ask(&instance, &Claktory::new("203.0.113.11"), "limit@example.com", "box").await.status, 429);
}

/// Path 4b: once the account has a device that may approve, an emailed
/// code alone no longer signs a further Claktory in; it waits for the
/// same approval. The first Claktory, and one signing in again with the
/// key it has there now (proven by signing its verify), go by the code.
#[tokio::test]
async fn an_emailed_code_signs_in_a_further_claktory_only_with_the_person_s_approval() {
    let (instance, mut studio, phone, account) = matts().await;
    instance.advance(60);
    let mut agents = Claktory::new("203.0.113.9");
    let started: SignInStarted = instance.start_sign_in(&agents, EMAIL, "agent-box").await.json();
    let verify = SignInVerify { request_id: started.request_id, code: instance.last_code(EMAIL), eph_pub: Ephemeral::new().public_b64() };
    let answer = instance.post("/v1/sign-in/verify", &verify, &agents.ip).await;
    assert_eq!((answer.status, answer.error()), (409, "authorization_pending".to_owned()), "{}", answer.text());
    let pending = answer.value();
    let id = pending["request_id"].as_str().expect("a request").to_owned();
    assert_eq!((pending["expires_in"].as_i64(), pending["interval"].as_i64()), (Some(600), Some(5)));
    // The code is spent.
    assert_eq!(instance.post("/v1/sign-in/verify", &verify, &agents.ip).await.error(), "code_expired");
    let card = shown(&instance, &phone).await.remove(0);
    assert_eq!((card.id.as_str(), card.says.name.as_str()), (id.as_str(), "agent-box"));
    let approve =
        json!({"approve": true, "fingerprint": card.fingerprint, "allowed": word(&instance, &studio, &account, &phone, &id, None)});
    let decided = decide(&instance, &phone, &id, approve).await;
    assert_eq!(decided.status, 200, "{}", decided.text());
    let (answer, _) = poll(&instance, &agents, &id).await;
    assert_eq!(answer.status, 200, "{}", answer.text());
    agents.token = answer.json::<Authorized>().signed_in.machine_token;
    assert_eq!(instance.signed(&agents, "GET", "/v1/claktories/authorizations", &json!({}), None).await.status, 200);

    // The studio signing in again with the key it has, proving it: by the
    // code alone.
    instance.sign_in(&mut studio, EMAIL, "Mac mini").await;
    // Unproven (a new key for the same Claktory): it waits.
    studio.key = new_signing_key();
    let started: SignInStarted = instance.start_sign_in(&studio, EMAIL, "Mac mini").await.json();
    let verify = SignInVerify { request_id: started.request_id, code: instance.last_code(EMAIL), eph_pub: Ephemeral::new().public_b64() };
    assert_eq!(instance.post("/v1/sign-in/verify", &verify, &studio.ip).await.error(), "authorization_pending");
}

/// A Claktory's word comes from one bound to the account before the
/// request started, and never from the computer asking: in path 4b, the
/// Claktory signing in again with a new key can't vouch for itself with
/// the key it has there now.
#[tokio::test]
async fn the_computer_asking_never_vouches_for_its_own_approval() {
    let (instance, mut studio, phone, account) = matts().await;
    let old = Claktory {
        id: studio.id.clone(),
        key: studio.key.clone(),
        token: studio.token.clone(),
        epoch: studio.epoch,
        ip: studio.ip.clone(),
    };
    studio.key = new_signing_key();
    let started: SignInStarted = instance.start_sign_in(&studio, EMAIL, "Mac mini").await.json();
    let verify = SignInVerify { request_id: started.request_id, code: instance.last_code(EMAIL), eph_pub: Ephemeral::new().public_b64() };
    let id = instance.post("/v1/sign-in/verify", &verify, &studio.ip).await.value()["request_id"].as_str().expect("waits").to_owned();
    let fingerprint = shown(&instance, &phone).await[0].fingerprint.clone();
    let own = json!({"approve": true, "fingerprint": fingerprint, "allowed": word(&instance, &old, &account, &phone, &id, None)});
    assert_eq!(decide(&instance, &phone, &id, own).await.error(), "not_allowed_to_approve");
}

/// Once the account's devices set up its DM identity before the request,
/// only a device on its signed device list approves: a device signed in by email but never listed can't. An
/// identity set up after the request started doesn't count.
#[tokio::test]
async fn with_a_dm_identity_only_a_device_on_its_list_approves() {
    use claktory_dm_crypto::certificate::LeafKind;
    use claktory_dm_crypto::device_list::{DeviceList, Listed};
    use claktory_dm_crypto::identity::IdentityKey;

    use crate::user_apps::{uid, with_apps, worker};

    let dir = tempfile::tempdir().expect("a folder");
    let instance = with_apps(Some(worker(dir.path(), uid(dir.path()))), uid(dir.path())).await;
    // The account's DM identity, set up by `phone` and listing only it.
    let identity_of = |account: &str, phone: &Device| {
        let identity = IdentityKey::generate().expect("a key");
        let leaves = vec![Listed { kind: LeafKind::Device, leaf_id: phone.account_key(), signature_key: [1; 32] }];
        let signed = DeviceList { account_id: account.to_owned(), version: 1, created_ms: 1, leaves }.sign(&identity).expect("signed");
        json!({"identity_key": b64(identity.public().0), "device_list": b64(signed.to_bytes().expect("bytes")),
               "version": 1, "previous_version": 0})
    };
    let set_up = async |email: &str, n: u8| {
        let mut studio = Claktory::new(&format!("198.51.100.{n}"));
        let account = instance.sign_in(&mut studio, email, "Mac mini").await.account_id;
        let phone = Device::new(&format!("192.0.2.{n}"));
        instance.sign_in_device(&phone, email, "iPhone").await;
        let tablet = Device::new(&format!("192.0.2.{}", n + 1));
        instance.sign_in_device(&tablet, email, "iPad").await;
        (studio, account, phone, tablet)
    };
    let approve = |studio: &Claktory, account: &str, device: &Device, id: &str, fingerprint: &str| json!({"approve": true, "fingerprint": fingerprint, "allowed": word(&instance, studio, account, device, id, None)});

    // Set up before the request: the tablet, not on the list, can't.
    let (studio, account, phone, tablet) = set_up(EMAIL, 40).await;
    instance.advance(1);
    let put = instance.device_signed(&phone, "PUT", "/v1/apps/dms/me/keys", Some(&identity_of(&account, &phone)), None).await;
    assert_eq!(put.status, 200, "{}", put.text());
    instance.advance(60);
    let id = ask(&instance, &Claktory::new("203.0.113.9"), EMAIL, "box").await.json::<AuthorizeStarted>().request_id;
    let fingerprint = shown(&instance, &tablet).await[0].fingerprint.clone();
    let refused = decide(&instance, &tablet, &id, approve(&studio, &account, &tablet, &id, &fingerprint)).await;
    assert_eq!((refused.status, refused.error()), (403, "not_allowed_to_approve".to_owned()), "{}", refused.text());
    let approved = decide(&instance, &phone, &id, approve(&studio, &account, &phone, &id, &fingerprint)).await;
    assert_eq!(approved.status, 200, "{}", approved.text());

    // Set up after the request started: the tablet approves.
    let (studio, account, phone, tablet) = set_up("jo@example.com", 50).await;
    instance.advance(60);
    let id = ask(&instance, &Claktory::new("203.0.113.19"), "jo@example.com", "box").await.json::<AuthorizeStarted>().request_id;
    instance.advance(1);
    let put = instance.device_signed(&phone, "PUT", "/v1/apps/dms/me/keys", Some(&identity_of(&account, &phone)), None).await;
    assert_eq!(put.status, 200, "{}", put.text());
    let fingerprint = shown(&instance, &tablet).await[0].fingerprint.clone();
    let approved = decide(&instance, &tablet, &id, approve(&studio, &account, &tablet, &id, &fingerprint)).await;
    assert_eq!(approved.status, 200, "{}", approved.text());
}
