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
        Request::ApproveDevice(v1::ApproveDevice {
            device_id: pending.device_id.clone(),
            number: asking.number.clone(),
            ..Default::default()
        }),
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
    // Another account's (#527 S4): a pipe is for that.
    let (code, reason) = refused(&answer);
    assert!(code == ErrorCode::OtherAccount && reason.contains("open a pipe"), "{reason}");
    assert!(matches!(&answer, Body::Error(error) if !error.other_account_before), "never let in here: {answer:?}");
    let (_, answer) = knock(&local, &stranger, "", "").await;
    assert_eq!(refused(&answer).0, ErrorCode::SignInRequired);
    drop(phone);
}

/// Friend bug #38: a Claktory signed out of its account (its sign-in lost to
/// a failing disk, say) tells a device that came through the account to
/// sign it in again, which lets the device back in as before; and pairing
/// that device with a code meanwhile, with its key, lists it once.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_signed_out_claktory_asks_to_be_signed_in_again_and_a_code_never_pairs_a_device_twice() {
    let dir = temp_dir();
    let instance = Instance::start(&dir).await;
    let (_factory, mut client, mut view, local) = signed_in(&dir, &instance).await;
    let account = account_id(&dir);
    let mac = new_device().await;
    let (mut laptop, answer) = knock(&local, &mac, "", &assertion(&instance, &account, &mac)).await;
    let first = welcomed(&mut laptop, answer).await.device_id;
    drop(laptop);

    apply_request(&mut client, &mut view, Request::AccountSignOut(v1::AccountSignOut {})).await;
    let (_, answer) = knock(&local, &mac, "", &assertion(&instance, &account, &mac)).await;
    let (code, reason) = refused(&answer);
    // Not Unauthorized, which says it was removed: the app would wipe what
    // it hadn't sent (#446).
    assert_eq!(code, ErrorCode::SignInRequired);
    assert!(reason.contains("signed out of your Claktory account") && reason.contains("Sign it in again"), "{reason}");
    // One it doesn't know still hears how to be added.
    let stranger = new_device().await;
    let (_, answer) = knock(&local, &stranger, "", &assertion(&instance, &account, &stranger)).await;
    assert_eq!(refused(&answer).0, ErrorCode::Unauthorized, "not paired: as before");
    assert!(refused(&answer).1.contains("with a code"), "{:?}", refused(&answer));

    // Signed in again: in as before, the same device.
    view.workspace = crate::claktory_account::sign_in(&mut client, &instance, "matt@example.com").await;
    let (mut laptop, answer) = knock(&local, &mac, "", &assertion(&instance, &account, &mac)).await;
    assert_eq!(welcomed(&mut laptop, answer).await.device_id, first);
    drop(laptop);

    // Paired with a code as well (the app keeps its key for this Claktory):
    // once allowed, it's one device, the new pairing.
    let made =
        apply_request(&mut client, &mut view, Request::CreatePairing(v1::CreatePairing { name: "MacBook".into(), host: String::new() }))
            .await;
    let Body::PairingCode(made) = made else { panic!("a code, got {made:?}") };
    let token = crate::remote::parse(&made.code).token;
    let (mut laptop, answer) = knock(&local, &mac, &token, "").await;
    let Body::PairingPending(pending) = answer else { panic!("expected it to wait, got {answer:?}") };
    view.follow(&mut client, |view| remote(view).pending_devices.iter().any(|device| device.device_id == pending.device_id)).await;
    let number = remote(&view).pending_devices.into_iter().find(|device| device.device_id == pending.device_id).expect("waiting").number;
    apply_request(
        &mut client,
        &mut view,
        Request::ApproveDevice(v1::ApproveDevice { device_id: pending.device_id.clone(), number, ..Default::default() }),
    )
    .await;
    let answer = laptop.receive().await.body.expect("an answer");
    assert_eq!(welcomed(&mut laptop, answer).await.device_id, pending.device_id);
    view.follow(&mut client, |view| remote(view).devices.iter().all(|device| device.id != first)).await;
    let listed: Vec<String> = remote(&view).devices.into_iter().map(|device| device.id).collect();
    assert_eq!(listed, [pending.device_id], "listed once");
}

/// The paired device with this id, as the Workspace shows it.
fn shown(view: &View, id: &str) -> Option<v1::PairedDevice> {
    remote(view).devices.into_iter().find(|device| device.id == id)
}

/// The four digits a waiting device shows, as the Workspace has them.
fn pending_number(view: &View, id: &str) -> Option<String> {
    remote(view).pending_devices.into_iter().find(|device| device.device_id == id).map(|device| device.number)
}

/// #527 S3: no deadlines. A code and the owner's account need no Allow; an
/// Allow a person made with the digits (here at a terminal) confirms the
/// device; one let in by a program shows the owner's assertion at once, or
/// is confirmed from a confirmed device. Each Allow is recorded and
/// noticed until a device dismisses it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_allow_a_person_made_confirms_a_device_and_one_by_a_program_needs_the_account() {
    let dir = temp_dir();
    let instance = Instance::start(&dir).await;
    let (_factory, mut client, mut view, local) = signed_in(&dir, &instance).await;
    let account = account_id(&dir);

    // A code and the account: in without Allow.
    let (token, _) = factoryd::remote::pair(&home(&dir), "Phone").expect("paired");
    let phone = new_device().await;
    let (mut app, answer) = knock(&local, &phone, &token, &assertion(&instance, &account, &phone)).await;
    assert_eq!(welcomed(&mut app, answer).await.confirm_by_ms, 0);
    drop(app);

    // Allowed at a terminal: confirmed at once, no deadline, and noticed.
    let (token, made_tablet) = factoryd::remote::pair(&home(&dir), "Tablet").expect("paired");
    let tablet = new_device().await;
    let (mut app, answer) = knock(&local, &tablet, &token, "").await;
    assert!(matches!(answer, Body::PairingPending(_)), "{answer:?}");
    factoryd::remote::approve(&home(&dir), &made_tablet.id).expect("allowed");
    let answer = app.receive().await.body.expect("an answer");
    assert_eq!(welcomed(&mut app, answer).await.confirm_by_ms, 0);
    view.follow(&mut client, |view| shown(view, &made_tablet.id).is_some_and(|device| device.confirmed)).await;
    let listed = shown(&view, &made_tablet.id).expect("listed");
    assert_eq!((listed.allowed_by.as_str(), listed.confirm_by_ms), ("terminal", 0));
    assert!(listed.allowed_ms > 0);
    let notices = remote(&view).allow_notices;
    assert_eq!(notices.len(), 1, "{notices:?}");
    assert_eq!((notices[0].device_id.as_str(), notices[0].allowed_by.as_str()), (made_tablet.id.as_str(), "terminal"));
    // Dismissed on one device, gone on all.
    apply_request(&mut client, &mut view, Request::DismissAllowNotice(v1::DismissAllowNotice { device_id: made_tablet.id.clone() })).await;
    view.follow(&mut client, |view| remote(view).allow_notices.is_empty()).await;
    drop(app);
    // Signed in to another account on it (a stolen tablet, say): turned
    // away, and the person's devices see it (#527 S4) until it's back.
    let (_, answer) = knock(&local, &tablet, &token, &assertion(&instance, "someone-else", &tablet)).await;
    let (code, reason) = refused(&answer);
    assert!(code == ErrorCode::OtherAccount && reason.contains("now signed in to a different"), "{reason}");
    assert!(matches!(&answer, Body::Error(error) if error.other_account_before), "let in here before: {answer:?}");
    view.follow(&mut client, |view| {
        shown(view, &made_tablet.id).is_some_and(|device| device.turned_why == "other_account" && device.turned_ms > 0)
    })
    .await;
    let (mut app, answer) = knock(&local, &tablet, &token, "").await;
    welcomed(&mut app, answer).await;
    view.follow(&mut client, |view| shown(view, &made_tablet.id).is_some_and(|device| device.turned_why.is_empty())).await;

    // Allowed by a program (`factoryd allow` with no terminal): turned away
    // at once until it shows the owner's assertion.
    let (token, made) = factoryd::remote::pair(&home(&dir), "Old laptop").expect("paired");
    let laptop = new_device().await;
    let (mut old, answer) = knock(&local, &laptop, &token, "").await;
    assert!(matches!(answer, Body::PairingPending(_)), "{answer:?}");
    factoryd::remote::approve_by(&home(&dir), &made.id, &factoryd::remote::Allower::Program).expect("allowed");
    let answer = old.receive().await.body.expect("an answer");
    assert_eq!(refused(&answer).0, ErrorCode::SignInRequired, "{answer:?}");
    view.follow(&mut client, |view| shown(view, &made.id).is_some_and(|device| !device.confirmed && device.allowed_by == "program")).await;
    let (_, answer) = knock(&local, &laptop, &token, "").await;
    assert_eq!(refused(&answer).0, ErrorCode::SignInRequired);
    // The person confirms it from a device that's the owner's.
    app.ok(Request::ConfirmDevice(v1::ConfirmDevice { assertion: String::new(), device_id: made.id.clone() })).await;
    let (mut old, answer) = knock(&local, &laptop, &token, "").await;
    welcomed(&mut old, answer).await;
    drop(old);

    // Or its own assertion confirms it.
    let (token, made) = factoryd::remote::pair(&home(&dir), "Spare").expect("paired");
    let spare = new_device().await;
    let (mut waiting, answer) = knock(&local, &spare, &token, "").await;
    assert!(matches!(answer, Body::PairingPending(_)), "{answer:?}");
    factoryd::remote::approve_by(&home(&dir), &made.id, &factoryd::remote::Allower::Program).expect("allowed");
    assert_eq!(refused(&waiting.receive().await.body.expect("an answer")).0, ErrorCode::SignInRequired);
    let (mut spare_app, answer) = knock(&local, &spare, &token, &assertion(&instance, &account, &spare)).await;
    welcomed(&mut spare_app, answer).await;
    view.follow(&mut client, |view| shown(view, &made.id).is_some_and(|device| device.confirmed)).await;
}

/// #527 S3: this computer's own app allows with the digits, and its Allow
/// confirms; a device not confirmed itself lets another in, unconfirmed.
/// The record says who, by name as it was then, and never the digits.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn who_allowed_a_device_is_recorded_and_only_a_confirmed_one_confirms() {
    let dir = temp_dir();
    let instance = Instance::start(&dir).await;
    let (_factory, mut client, mut view, local) = signed_in(&dir, &instance).await;
    // Signed out for now: a device let in by a program connects meanwhile.
    apply_request(&mut client, &mut view, Request::AccountSignOut(v1::AccountSignOut {})).await;
    let (mut unconfirmed, unconfirmed_id) = paired(&dir, &local, "Borrowed tablet").await;

    // This computer's own app allows one: confirmed, by "local".
    let (token, by_app) = factoryd::remote::pair(&home(&dir), "Phone").expect("paired");
    let phone = new_device().await;
    let (mut phone_app, answer) = knock(&local, &phone, &token, "").await;
    assert!(matches!(answer, Body::PairingPending(_)), "{answer:?}");
    view.follow(&mut client, |view| pending_number(view, &by_app.id).is_some()).await;
    let number = pending_number(&view, &by_app.id).expect("waiting");
    let allow = v1::ApproveDevice { device_id: by_app.id.clone(), number: number.clone(), ..Default::default() };
    apply_request(&mut client, &mut view, Request::ApproveDevice(allow)).await;
    let answer = phone_app.receive().await.body.expect("an answer");
    welcomed(&mut phone_app, answer).await;
    view.follow(&mut client, |view| shown(view, &by_app.id).is_some_and(|device| device.confirmed)).await;
    let listed = shown(&view, &by_app.id).expect("listed");
    assert_eq!((listed.allowed_by.as_str(), listed.allowed_by_device_id.as_str()), ("device", "local"));
    assert!(!listed.allowed_by_name.is_empty());
    let notice = remote(&view).allow_notices.into_iter().find(|notice| notice.device_id == by_app.id).expect("noticed");
    assert!(!format!("{notice:?}").contains(&number), "never the digits");

    // The unconfirmed device allows one: let in, not confirmed.
    let (token, by_device) = factoryd::remote::pair(&home(&dir), "Laptop").expect("paired");
    let laptop = new_device().await;
    let (mut laptop_app, answer) = knock(&local, &laptop, &token, "").await;
    assert!(matches!(answer, Body::PairingPending(_)), "{answer:?}");
    view.follow(&mut client, |view| pending_number(view, &by_device.id).is_some()).await;
    let number = pending_number(&view, &by_device.id).expect("waiting");
    unconfirmed.ok(Request::ApproveDevice(v1::ApproveDevice { device_id: by_device.id.clone(), number, ..Default::default() })).await;
    let answer = laptop_app.receive().await.body.expect("an answer");
    welcomed(&mut laptop_app, answer).await;
    view.follow(&mut client, |view| shown(view, &by_device.id).is_some_and(|device| device.allowed_ms > 0)).await;
    let listed = shown(&view, &by_device.id).expect("listed");
    assert!(!listed.confirmed);
    assert_eq!(
        (listed.allowed_by.as_str(), listed.allowed_by_device_id.as_str(), listed.allowed_by_name.as_str()),
        ("device", unconfirmed_id.as_str(), "Borrowed tablet")
    );
    drop(laptop_app);

    // Signed in again: the one it let in shows the owner's assertion first.
    view.workspace = crate::claktory_account::sign_in(&mut client, &instance, "matt@example.com").await;
    let (_, answer) = knock(&local, &laptop, &token, "").await;
    assert_eq!(refused(&answer).0, ErrorCode::SignInRequired);
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
/// and allowed by a program, so not confirmed by its Allow (#527 S3).
async fn paired(dir: &Path, local: &Code, name: &str) -> (Client, String) {
    let (token, made) = factoryd::remote::pair(&home(dir), name).expect("paired");
    let device = new_device().await;
    let (mut app, answer) = knock(local, &device, &token, "").await;
    assert!(matches!(answer, Body::PairingPending(_)), "{answer:?}");
    factoryd::remote::approve_by(&home(dir), &made.id, &factoryd::remote::Allower::Program).expect("allowed");
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

/// What the Mac app sends to sign in to the account through its own
/// Claktory (#529), with the public half of `key`.
fn vouch(key: &ed25519_dalek::SigningKey) -> Request {
    Request::VouchDevice(v1::VouchDevice {
        account_key: claktory_account::protocol::b64(key.verifying_key().as_bytes()),
        name: "Matt's MacBook".into(),
        platform: "macos".into(),
        version: "test".into(),
    })
}

/// #529 (Fugo's condition 3): only this computer's own app signs in to the
/// account through this Claktory. A device through the tunnel is refused
/// whoever it is, the loopback port lets no one ask without the app's
/// token, and the app is refused while the Claktory isn't signed in.
/// (Rivet's reach: `rivet_reach`, which leaves VouchDevice to the person;
/// pipes send no requests.)
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn only_this_computers_own_app_signs_in_through_this_claktory() {
    let dir = temp_dir();
    let instance = Instance::start(&dir).await;
    let (addr, factory) = open(&dir, &instance.url).await;
    factory.set_tunnel_online_wait(Duration::from_millis(100));
    let (mut client, workspace, _) = Client::connect(addr).await;
    let mut view = View { workspace, ..View::default() };
    let key = ed25519_dalek::SigningKey::from_bytes(&[7; 32]);

    // Not signed in: nothing to sign in through.
    let (code, reason) = refused(&client.request(vouch(&key)).await.body);
    assert_eq!(code, ErrorCode::SignInRequired, "{reason}");

    let relays = v1::SetRemoteAccess { relays: vec!["http://127.0.0.1:9/".into()], set_relays: true, ..Default::default() };
    apply_request(&mut client, &mut view, Request::SetRemoteAccess(relays)).await;
    view.workspace = sign_in(&mut client, &instance, "matt@example.com").await;
    let local = parse(&local_pairing(&dir).await);
    let account = account_id(&dir);

    // A device of the owner's, through the tunnel: never.
    let device = new_device().await;
    let (mut phone, answer) = knock(&local, &device, "", &assertion(&instance, &account, &device)).await;
    welcomed(&mut phone, answer).await;
    let (code, reason) = refused(&phone.request(vouch(&key)).await.body);
    assert_eq!(code, ErrorCode::Unauthorized, "{reason}");

    // The loopback port without the app's token: not even let in to ask.
    let mut stranger = Client::open(addr).await;
    let hello =
        v1::Hello { protocol_version: PROTOCOL_VERSION, client_version: "test".into(), token: "not-the-apps".into(), ..Default::default() };
    stranger.send(v1::ClientMessage { id: 1, body: Some(Request::Hello(hello)) }).await;
    assert!(matches!(stranger.receive().await.body, Some(Body::Error(_))), "a Hello without the app's token is refused");

    // This computer's own app: the service is asked, and gives a code
    // (or, from before the vouch, says it can't).
    let Body::DeviceVouched(vouched) = client.request(vouch(&key)).await.body else { panic!("expected DeviceVouched") };
    assert!(vouched.refused == "unavailable" || (vouched.refused.is_empty() && !vouched.code.is_empty()), "{vouched:?}");
}

/// A call to the account service as a device, signed by its account key
/// `key`: the answer's status and body.
async fn as_device(instance: &Instance, key: &ed25519_dalek::SigningKey, path: &str, body: serde_json::Value) -> (u16, serde_json::Value) {
    use claktory_account::protocol as api;
    let body = serde_json::to_vec(&body).expect("json");
    let time = jiff::Timestamp::now().as_second();
    let answer = reqwest::Client::new()
        .post(format!("{}{path}", instance.url))
        .header("authorization", format!("Device {}", api::b64(key.verifying_key().as_bytes())))
        .header(api::TIME_HEADER, time.to_string())
        .header(api::SIGNATURE_HEADER, api::sign_request(key, "POST", path, time, &body))
        .header("content-type", "application/json")
        .body(body)
        .send()
        .await
        .expect("the account service");
    let status = answer.status().as_u16();
    (status, answer.json().await.unwrap_or_default())
}

/// #529 end to end: the Mac app signs in to the account through its own
/// Claktory, with no emailed code, and another Claktory of the account
/// that it was paired with by a code alone then takes it as the owner's.
/// A code given for one key signs in no other, and signing the Claktory
/// out signs its app out too.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_mac_app_signs_in_through_its_own_claktory_and_another_claktory_confirms_it() {
    let dir = temp_dir();
    let instance = Instance::start(&dir).await;
    let (_mac_factory, mut mac, mut mac_view, _) = signed_in(&dir, &instance).await;
    let server_dir = temp_dir();
    let (_server_factory, mut server, mut server_view, server_code) = signed_in(&server_dir, &instance).await;
    let server_view = &mut server_view;

    // Paired with the server by a code and let in by a program: it shows
    // the owner's assertion before it connects (#527 S3).
    let (token, made) = factoryd::remote::pair(&home(&server_dir), "Mac").expect("paired");
    let tunnel = new_device().await;
    let (mut app, answer) = knock(&server_code, &tunnel, &token, "").await;
    assert!(matches!(answer, Body::PairingPending(_)), "{answer:?}");
    factoryd::remote::approve_by(&home(&server_dir), &made.id, &factoryd::remote::Allower::Program).expect("allowed");
    let answer = app.receive().await.body.expect("an answer");
    assert_eq!(refused(&answer).0, ErrorCode::SignInRequired, "not the owner's yet");
    drop(app);

    // The Mac app asks its own Claktory for a code for its account key.
    let key = ed25519_dalek::SigningKey::from_bytes(&[7; 32]);
    let Body::DeviceVouched(vouched) = mac.request(vouch(&key)).await.body else { panic!("expected DeviceVouched") };
    assert!(vouched.refused.is_empty() && !vouched.code.is_empty(), "{vouched:?}");
    assert_eq!((vouched.service.as_str(), vouched.email.as_str()), (instance.url.as_str(), "matt@example.com"));
    let verify = serde_json::json!({"request_id": vouched.request_id, "code": vouched.code});

    // Another key can't use it.
    let other = ed25519_dalek::SigningKey::from_bytes(&[8; 32]);
    let (status, _) = as_device(&instance, &other, "/v1/devices/sign-in/verify", verify.clone()).await;
    assert_eq!(status, 401, "a code for one key signs in no other");

    // The key it was given for signs in, and gets an assertion for its
    // tunnel key to the server, as a phone that signed in itself does.
    let (status, signed) = as_device(&instance, &key, "/v1/devices/sign-in/verify", verify.clone()).await;
    assert_eq!(status, 200, "{signed}");
    assert_eq!(signed["email"], "matt@example.com");
    let (status, _) = as_device(&instance, &key, "/v1/devices/sign-in/verify", verify).await;
    assert_ne!(status, 200, "a code works once");
    let (status, issued) =
        as_device(&instance, &key, "/v1/devices/assertion", serde_json::json!({"device_key": tunnel.id().to_string()})).await;
    assert_eq!(status, 200, "{issued}");
    let said = issued["assertion"].as_str().expect("an assertion").to_owned();

    // The server takes it as the owner's from its next Hello, with no
    // code.
    let (mut app, answer) = knock(&server_code, &tunnel, &token, &said).await;
    assert_eq!(welcomed(&mut app, answer).await.confirm_by_ms, 0);
    server_view.follow(&mut server, |view| remote(view).devices.iter().any(|device| device.id == made.id && device.confirmed)).await;
    drop(app);

    // Signing the Mac's Claktory out signs its app out of the account too.
    apply_request(&mut mac, &mut mac_view, Request::AccountSignOut(v1::AccountSignOut {})).await;
    let (status, _) =
        as_device(&instance, &key, "/v1/devices/assertion", serde_json::json!({"device_key": new_device().await.id().to_string()})).await;
    assert_eq!(status, 401, "signed out with the Claktory that signed it in");
}
