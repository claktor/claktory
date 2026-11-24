//! Devices of the person's account (onboarding slice 6): a Claktory signed
//! in to claktory.com lets a device signed in to the same account find it
//! and connect with its device assertion. The first, just after this
//! Claktory signs in, is in at once; any other waits for Allow; a device
//! paired with a code before is asked to confirm it's the owner's, once.

use std::path::Path;
use std::time::Duration;

use claktory_account::protocol::{DeviceAssertion, sign_device_assertion};
use factory_protocol::PROTOCOL_VERSION;
use factory_protocol::v1::{self, ErrorCode, client_message::Body as Request, server_message::Body};
use iroh::Endpoint;
use sha2::Digest;

use crate::claktory_account::{Instance, open, sign_in};
use crate::common::{Client, View, apply_request, home, temp_dir};
use crate::remote::{Code, dial, local_pairing, new_device, parse};

/// The account this Claktory signed in to.
fn account_id(dir: &Path) -> String {
    let text = std::fs::read_to_string(dir.join("home/account/account.json")).expect("account.json");
    serde_json::from_str::<serde_json::Value>(&text).expect("json")["account_id"].as_str().expect("account_id").to_owned()
}

/// What the service gives a device signed in to `account` with tunnel key
/// `device`.
fn assertion(instance: &Instance, account: &str, device: &Endpoint) -> String {
    let now = jiff::Timestamp::now().as_second();
    let said = DeviceAssertion {
        v: 1,
        account_id: account.to_owned(),
        account_key: "a-device-account-key".to_owned(),
        device_key: device.id().to_string(),
        iat: now,
        exp: now + 3600,
    };
    sign_device_assertion(&ed25519_dalek::SigningKey::from_bytes(&instance.secrets.1), &said)
}

/// A device's app through the tunnel: says Hello with what it has; the
/// answer.
async fn knock(local: &Code, device: &Endpoint, token: &str, assertion: &str) -> (Client, Body) {
    let connection = dial(device, local, &local.addresses).await.expect("let in");
    let (send, receive) = connection.open_bi().await.expect("stream");
    let mut client = Client::over(Box::new(tokio::io::join(receive, send)), Some(Box::new((device.clone(), connection)))).await;
    let hello = v1::Hello {
        protocol_version: PROTOCOL_VERSION,
        client_version: "test".into(),
        token: token.into(),
        assertion: assertion.into(),
        can_confirm: true,
        device_kind: v1::DeviceKind::Phone.into(),
        device_name: format!("Phone {}", &device.id().to_string()[..4]),
        ..Default::default()
    };
    client.send(v1::ClientMessage { id: 1, body: Some(v1::client_message::Body::Hello(hello)) }).await;
    let answer = client.receive().await.body.expect("an answer");
    (client, answer)
}

/// The rest of a Welcome: the state that follows it.
async fn welcomed(client: &mut Client, answer: Body) -> v1::Welcome {
    let Body::Welcome(welcome) = answer else { panic!("expected welcome, got {answer:?}") };
    let Some(Body::Workspace(_)) = client.receive().await.body else { panic!("expected the workspace") };
    welcome
}

fn refused(answer: &Body) -> (ErrorCode, String) {
    match answer {
        Body::Error(error) => (error.code(), error.message.clone()),
        other => panic!("expected an error, got {other:?}"),
    }
}

fn remote(view: &View) -> v1::RemoteAccess {
    view.workspace.remote.clone().expect("remote access")
}

/// A Claktory signed in to a local account service, its own app connected,
/// with a relay of its own (nothing there) so nothing reaches the internet.
async fn signed_in(dir: &Path, instance: &Instance) -> (std::sync::Arc<factoryd::Factory>, Client, View, Code) {
    let (addr, factory) = open(dir, &instance.url).await;
    factory.set_tunnel_online_wait(Duration::from_millis(100));
    let (mut client, workspace, _) = Client::connect(addr).await;
    let mut view = View { workspace, ..View::default() };
    let relays = v1::SetRemoteAccess { relays: vec!["http://127.0.0.1:9/".into()], set_relays: true, ..Default::default() };
    apply_request(&mut client, &mut view, Request::SetRemoteAccess(relays)).await;
    view.workspace = sign_in(&mut client, instance, "matt@example.com").await;
    let local = parse(&local_pairing(dir).await);
    (factory, client, view, local)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_first_device_through_the_account_is_in_at_once_and_the_next_waits_for_allow() {
    let dir = temp_dir();
    let instance = Instance::start(&dir).await;
    let (_factory, mut client, mut view, local) = signed_in(&dir, &instance).await;
    let account = account_id(&dir);

    // Just signed in, with no device yet: the first is in by its account.
    let first = new_device().await;
    let (mut phone, answer) = knock(&local, &first, "", &assertion(&instance, &account, &first)).await;
    let welcome = welcomed(&mut phone, answer).await;
    assert!(!welcome.device_id.is_empty());
    assert_eq!(welcome.confirm_by_ms, 0, "it's the owner's");
    view.follow(&mut client, |view| remote(view).devices.iter().any(|device| device.id == welcome.device_id && device.confirmed)).await;

    // The next waits for Allow, with the four digits worked out from its
    // assertion.
    let second = new_device().await;
    let said = assertion(&instance, &account, &second);
    let (mut laptop, answer) = knock(&local, &second, "", &said).await;
    let Body::PairingPending(pending) = answer else { panic!("expected it to wait, got {answer:?}") };
    assert!(pending.discovered);
    view.follow(&mut client, |view| remote(view).pending_devices.iter().any(|device| device.device_id == pending.device_id)).await;
    let asking = remote(&view).pending_devices.into_iter().find(|device| device.device_id == pending.device_id).expect("waiting");
    assert!(asking.signed_in && asking.discovered);
    let hashed: String = sha2::Sha256::digest(said.as_bytes()).iter().map(|byte| format!("{byte:02x}")).collect();
    assert_eq!(asking.number, factoryd::remote::approval_number(&local.endpoint.to_string(), &second.id().to_string(), &hashed));
    apply_request(
        &mut client,
        &mut view,
        Request::ApproveDevice(v1::ApproveDevice { device_id: pending.device_id.clone(), number: asking.number.clone() }),
    )
    .await;
    let answer = laptop.receive().await.body.expect("an answer");
    assert_eq!(welcomed(&mut laptop, answer).await.device_id, pending.device_id);
    drop(laptop);
    // Allowed, it comes back with its key and a fresh assertion, the
    // owner's…
    let (mut laptop, answer) = knock(&local, &second, "", &assertion(&instance, &account, &second)).await;
    let welcome = welcomed(&mut laptop, answer).await;
    assert_eq!((welcome.device_id.as_str(), welcome.confirm_by_ms), (pending.device_id.as_str(), 0));
    drop(laptop);
    // …never by its key alone: anything that borrows the device's tunnel
    // shares its key, so every Hello proves the account afresh.
    let (_, answer) = knock(&local, &second, "", "").await;
    assert_eq!(refused(&answer).0, ErrorCode::SignInRequired);

    // Turned away, it hears it can ask again by connecting again.
    let third = new_device().await;
    let (mut tablet, answer) = knock(&local, &third, "", &assertion(&instance, &account, &third)).await;
    let Body::PairingPending(pending) = answer else { panic!("expected it to wait, got {answer:?}") };
    apply_request(&mut client, &mut view, Request::DenyDevice(v1::DenyDevice { device_id: pending.device_id })).await;
    let answer = tablet.receive().await.body.expect("an answer");
    let (code, reason) = refused(&answer);
    assert!(code == ErrorCode::Unauthorized && reason.ends_with("didn't allow this device."), "{reason}");

    // Another key with that assertion, another account's, or no sign-in at
    // all: turned away, saying what to do.
    let stranger = new_device().await;
    let (_, answer) = knock(&local, &stranger, "", &said).await;
    assert_eq!(refused(&answer).0, ErrorCode::Unauthorized);
    let (_, answer) = knock(&local, &stranger, "", &assertion(&instance, "someone-else", &stranger)).await;
    assert_eq!(refused(&answer), (ErrorCode::Unauthorized, "This Claktory belongs to another account.".to_owned()));
    let (_, answer) = knock(&local, &stranger, "", "").await;
    assert_eq!(refused(&answer).0, ErrorCode::SignInRequired);
    drop(phone);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_device_paired_with_a_code_confirms_it_is_the_owners_once() {
    let dir = temp_dir();
    let instance = Instance::start(&dir).await;
    let (_factory, mut client, mut view, local) = signed_in(&dir, &instance).await;
    let account = account_id(&dir);

    // Paired with a code: a code and the account let it in without Allow.
    let (token, made) = factoryd::remote::pair(&home(&dir), "Phone").expect("paired");
    let phone = new_device().await;
    let (mut app, answer) = knock(&local, &phone, &token, &assertion(&instance, &account, &phone)).await;
    assert_eq!(welcomed(&mut app, answer).await.confirm_by_ms, 0);
    drop(app);
    assert_eq!(
        factoryd::remote::devices(&home(&dir)).expect("devices").iter().find(|device| device.id == made.id).map(|d| d.endpoint_id.clone()),
        Some(phone.id().to_string())
    );

    // Paired with a code alone (Allow): asked to confirm, with a week to.
    let (token, made) = factoryd::remote::pair(&home(&dir), "Tablet").expect("paired");
    let tablet = new_device().await;
    let (mut app, answer) = knock(&local, &tablet, &token, "").await;
    assert!(matches!(answer, Body::PairingPending(_)), "{answer:?}");
    factoryd::remote::approve(&home(&dir), &made.id).expect("allowed");
    let answer = app.receive().await.body.expect("an answer");
    let by = welcomed(&mut app, answer).await.confirm_by_ms;
    let week = 7 * 24 * 3600 * 1000;
    assert!((by - factoryd::transcript::now_ms() - week).abs() < 60_000, "{by}");
    view.follow(&mut client, |view| {
        remote(view).devices.iter().any(|device| device.id == made.id && !device.confirmed && device.confirm_by_ms == by)
    })
    .await;
    // Again: the same week, not a new one.
    drop(app);
    let (mut app, answer) = knock(&local, &tablet, &token, "").await;
    assert_eq!(welcomed(&mut app, answer).await.confirm_by_ms, by);
    // Signed in on it, it confirms itself.
    app.ok(Request::ConfirmDevice(v1::ConfirmDevice { assertion: assertion(&instance, &account, &tablet), device_id: String::new() }))
        .await;
    view.follow(&mut client, |view| remote(view).devices.iter().any(|device| device.id == made.id && device.confirmed)).await;
    drop(app);
    let (mut app, answer) = knock(&local, &tablet, &token, "").await;
    assert_eq!(welcomed(&mut app, answer).await.confirm_by_ms, 0);

    // Or the person confirms one from a device that's the owner's.
    let (token, made) = factoryd::remote::pair(&home(&dir), "Old laptop").expect("paired");
    let laptop = new_device().await;
    let (mut old, answer) = knock(&local, &laptop, &token, "").await;
    assert!(matches!(answer, Body::PairingPending(_)), "{answer:?}");
    factoryd::remote::approve(&home(&dir), &made.id).expect("allowed");
    let answer = old.receive().await.body.expect("an answer");
    assert!(welcomed(&mut old, answer).await.confirm_by_ms > 0);
    // Not from itself, unconfirmed.
    let mine = Request::ConfirmDevice(v1::ConfirmDevice { assertion: String::new(), device_id: made.id.clone() });
    let reply = old.request(mine.clone()).await;
    assert_eq!(refused(&reply.body).0, ErrorCode::Unauthorized);
    app.ok(mine).await;
    view.follow(&mut client, |view| remote(view).devices.iter().any(|device| device.id == made.id && device.confirmed)).await;
}

/// F-OK1's kill switch: once a release's floor is after the pinned service
/// key's statement (or the key was pinned without one, as before anchoring),
/// a leaked key's assertions let no device in.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_retired_service_key_lets_no_device_in_by_its_assertion() {
    let dir = temp_dir();
    let instance = Instance::start(&dir).await;
    let (_factory, _client, _view, local) = signed_in(&dir, &instance).await;
    let account = account_id(&dir);
    factoryd::anchor_account_for_tests(&instance.url, Vec::new(), 1);
    let device = new_device().await;
    let (_, answer) = knock(&local, &device, "", &assertion(&instance, &account, &device)).await;
    assert!(matches!(answer, Body::Error(_)), "turned away: {answer:?}");
}

/// Who a paired device is confirmed for, as devices.json keeps it.
fn confirmed_as(dir: &Path, id: &str) -> String {
    factoryd::remote::devices(&home(dir)).expect("devices").into_iter().find(|device| device.id == id).expect("the device").confirmed_as
}

/// A paired device's app through the tunnel, welcomed: paired with a code
/// and allowed.
async fn paired(dir: &Path, local: &Code, name: &str) -> (Client, String) {
    let (token, made) = factoryd::remote::pair(&home(dir), name).expect("paired");
    let device = new_device().await;
    let (mut app, answer) = knock(local, &device, &token, "").await;
    assert!(matches!(answer, Body::PairingPending(_)), "{answer:?}");
    factoryd::remote::approve(&home(dir), &made.id).expect("allowed");
    let answer = app.receive().await.body.expect("an answer");
    welcomed(&mut app, answer).await;
    (app, made.id)
}

/// #365 R5: signing this Claktory in on a paired device confirms that
/// device, and only it, for the account it just signed in to.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn signing_in_on_a_paired_device_confirms_that_device_alone() {
    let dir = temp_dir();
    let instance = Instance::start(&dir).await;
    let (addr, _factory) = open(&dir, &instance.url).await;
    let (mut client, workspace, _) = Client::connect(addr).await;
    let mut view = View { workspace, ..View::default() };
    let relays = v1::SetRemoteAccess { relays: vec!["http://127.0.0.1:9/".into()], set_relays: true, ..Default::default() };
    apply_request(&mut client, &mut view, Request::SetRemoteAccess(relays)).await;
    let local = parse(&local_pairing(&dir).await);
    let (mut phone, phone_id) = paired(&dir, &local, "Phone").await;
    let (mut tablet, tablet_id) = paired(&dir, &local, "Tablet").await;
    // One waiting to be allowed.
    let (token, waiting) = factoryd::remote::pair(&home(&dir), "Laptop").expect("paired");
    let laptop = new_device().await;
    let (_laptop, answer) = knock(&local, &laptop, &token, "").await;
    assert!(matches!(answer, Body::PairingPending(_)), "{answer:?}");

    // A wrong code signs in no one, and confirms no one.
    phone.ok(Request::AccountSignIn(v1::AccountSignIn { email: "matt@example.com".into() })).await;
    let wrong = phone.request(Request::AccountVerify(v1::AccountVerify { code: "000000".into() })).await;
    assert!(matches!(wrong.body, Body::Error(_)), "{:?}", wrong.body);
    assert_eq!(confirmed_as(&dir, &phone_id), "");

    // The right one: the phone that typed it is confirmed, as that account;
    // the tablet, and the laptop still waiting, aren't.
    let code = instance.last_code("matt@example.com");
    phone.ok(Request::AccountVerify(v1::AccountVerify { code })).await;
    let matt = account_id(&dir);
    assert_eq!(confirmed_as(&dir, &phone_id), matt);
    assert_eq!(confirmed_as(&dir, &tablet_id), "");
    assert_eq!(confirmed_as(&dir, &waiting.id), "");
    view.follow(&mut client, |view| remote(view).devices.iter().any(|device| device.id == phone_id && device.confirmed)).await;
    assert!(remote(&view).devices.iter().any(|device| device.id == tablet_id && !device.confirmed));

    // Signed in again into another account, on the tablet: only the tablet
    // is confirmed, as the new account; the phone stays the old one's.
    client.ok(Request::AccountSignOut(v1::AccountSignOut {})).await;
    tablet.ok(Request::AccountSignIn(v1::AccountSignIn { email: "fred@example.com".into() })).await;
    let code = instance.last_code("fred@example.com");
    tablet.ok(Request::AccountVerify(v1::AccountVerify { code })).await;
    let fred = account_id(&dir);
    assert_ne!(fred, matt);
    assert_eq!((confirmed_as(&dir, &tablet_id), confirmed_as(&dir, &phone_id)), (fred, matt));

    // This computer's own app signing in confirms no paired device.
    client.ok(Request::AccountSignOut(v1::AccountSignOut {})).await;
    let workspace = sign_in(&mut client, &instance, "matt@example.com").await;
    assert!(workspace.remote.expect("remote").devices.iter().all(|device| device.id != tablet_id || !device.confirmed));
    assert_eq!(confirmed_as(&dir, &waiting.id), "");
}
