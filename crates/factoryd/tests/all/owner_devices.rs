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
        account_key: account_key(device),
        device_key: device.id().to_string(),
        iat: now,
        exp: now + 3600,
    };
    sign_device_assertion(&ed25519_dalek::SigningKey::from_bytes(&instance.secrets.1), &said)
}

/// The account key of the device whose tunnel key is `device`, in these
/// tests: base64url of its key's bytes, as an account key is written.
fn account_key(device: &Endpoint) -> String {
    claktory_account::protocol::b64(device.id().as_bytes())
}

/// A device's app through the tunnel: says Hello with what it has; the
/// answer.
async fn knock(local: &Code, device: &Endpoint, token: &str, assertion: &str) -> (Client, Body) {
    let connection = dial(device, local, &local.addresses).await.expect("let in");
    knock_over(connection, device, token, assertion).await
}

/// `knock`, over a connection the tunnel let in.
async fn knock_over(connection: iroh::endpoint::Connection, device: &Endpoint, token: &str, assertion: &str) -> (Client, Body) {
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

/// Where a refusal says to sign in: (Error.account_service, account_email).
fn where_to(answer: &Body) -> (String, String) {
    match answer {
        Body::Error(error) => (error.account_service.clone(), error.account_email.clone()),
        other => panic!("expected an error, got {other:?}"),
    }
}

fn remote(view: &View) -> v1::RemoteAccess {
    view.workspace.remote.clone().expect("remote access")
}

/// A Claktory signed in to a local account service, its own app connected,
/// with a relay of its own (nothing there) so nothing reaches the internet.
async fn signed_in(dir: &Path, instance: &Instance) -> (std::sync::Arc<factoryd::Factory>, Client, View, Code) {
    let (_, factory, client, view, local) = signed_in_at(dir, instance).await;
    (factory, client, view, local)
}

/// `signed_in`, with the loopback port's address.
async fn signed_in_at(dir: &Path, instance: &Instance) -> (std::net::SocketAddr, std::sync::Arc<factoryd::Factory>, Client, View, Code) {
    let (addr, factory) = open(dir, &instance.url).await;
    factory.set_tunnel_online_wait(Duration::from_millis(100));
    let (mut client, workspace, _) = Client::connect(addr).await;
    let mut view = View { workspace, ..View::default() };
    let relays = v1::SetRemoteAccess { relays: vec!["http://127.0.0.1:9/".into()], set_relays: true, ..Default::default() };
    apply_request(&mut client, &mut view, Request::SetRemoteAccess(relays)).await;
    view.workspace = sign_in(&mut client, instance, "matt@example.com").await;
    let local = parse(&local_pairing(dir).await);
    (addr, factory, client, view, local)
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
    assert_eq!(asking.number, "", "the four digits are only on the device");
    let shown = factoryd::remote::approval_number(&local.endpoint.to_string(), &second.id().to_string(), &hashed);
    apply_request(
        &mut client,
        &mut view,
        Request::ApproveDevice(v1::ApproveDevice { device_id: pending.device_id.clone(), number: shown, ..Default::default() }),
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
    // Turned away to sign in, a device it knows hears where (Sale's row
    // 189, Fugo 47717): this Claktory's service and its email, masked.
    assert_eq!(where_to(&answer), (instance.url.clone(), "m…@example.com".to_owned()));

    // Turned away, it hears it can ask again by connecting again.
    let third = new_device().await;
    let (mut tablet, answer) = knock(&local, &third, "", &assertion(&instance, &account, &third)).await;
    let Body::PairingPending(pending) = answer else { panic!("expected it to wait, got {answer:?}") };
    apply_request(&mut client, &mut view, Request::DenyDevice(v1::DenyDevice { device_id: pending.device_id })).await;
    let answer = tablet.receive().await.body.expect("an answer");
    let (code, reason) = refused(&answer);
    assert!(code == ErrorCode::Unauthorized && reason.ends_with("didn't allow this device."), "{reason}");
    assert_eq!(where_to(&answer), (String::new(), String::new()), "a device turned away hears nothing of the account");

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
    // A device it doesn't know never hears whose Claktory this is: neither
    // with no sign-in nor with one that ran out.
    assert_eq!(where_to(&answer), (String::new(), String::new()));
    let now = jiff::Timestamp::now().as_second();
    let ran_out = DeviceAssertion {
        v: 1,
        account_id: account.clone(),
        account_key: "a-device-account-key".to_owned(),
        device_key: stranger.id().to_string(),
        iat: now - 7200,
        exp: now - 3600,
    };
    let ran_out = sign_device_assertion(&ed25519_dalek::SigningKey::from_bytes(&instance.secrets.1), &ran_out);
    let (_, answer) = knock(&local, &stranger, "", &ran_out).await;
    assert_eq!(refused(&answer).0, ErrorCode::SignInRequired);
    assert_eq!(where_to(&answer), (String::new(), String::new()));
    drop(phone);
}

/// A device the person removed stays removed: back through the account,
/// with or without its old code, it's refused as removed and nothing asks
/// the person to allow it. Only a code lets its key in again: allowed, or
/// with the person's account.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_removed_device_back_through_the_account_asks_nothing_until_a_code_lets_it_in() {
    let dir = temp_dir();
    let instance = Instance::start(&dir).await;
    let (_factory, mut client, mut view, local) = signed_in(&dir, &instance).await;
    let account = account_id(&dir);
    let phone = new_device().await;
    let removed = || factoryd::remote::removed_devices(&home(&dir)).into_iter().map(|removed| removed.endpoint_id).collect::<Vec<_>>();
    let only_this_mac = || factoryd::remote::devices(&home(&dir)).expect("devices").iter().all(|device| device.local);
    let refused_as_removed = |answer: &Body| {
        let (code, reason) = refused(answer);
        assert!(code == ErrorCode::Unauthorized && reason.contains("no longer allowed"), "{reason}");
    };

    // In through the account, then removed by the person.
    let (mut app, answer) = knock(&local, &phone, "", &assertion(&instance, &account, &phone)).await;
    let id = welcomed(&mut app, answer).await.device_id;
    apply_request(&mut client, &mut view, Request::UnpairDevice(v1::UnpairDevice { device_id: id })).await;
    closed(&mut app).await;
    assert_eq!(removed(), [phone.id().to_string()]);
    // Back, again and again: refused, and nothing waits to be allowed.
    for _ in 0..2 {
        let (_, answer) = knock(&local, &phone, "", &assertion(&instance, &account, &phone)).await;
        refused_as_removed(&answer);
        assert!(only_this_mac(), "{:?}", factoryd::remote::devices(&home(&dir)));
    }

    // Added again with a code and allowed: in, and no longer removed.
    let (token, made) = factoryd::remote::pair(&home(&dir), "Phone").expect("paired");
    let (mut app, answer) = knock(&local, &phone, &token, "").await;
    assert!(matches!(answer, Body::PairingPending(_)), "{answer:?}");
    factoryd::remote::approve(&home(&dir), &made.id).expect("allowed");
    let answer = app.receive().await.body.expect("an answer");
    welcomed(&mut app, answer).await;
    assert!(removed().is_empty(), "{:?}", removed());

    // Removed again, its old code with the account lets nothing in either.
    apply_request(&mut client, &mut view, Request::UnpairDevice(v1::UnpairDevice { device_id: made.id })).await;
    closed(&mut app).await;
    let (_, answer) = knock(&local, &phone, &token, &assertion(&instance, &account, &phone)).await;
    refused_as_removed(&answer);
    assert!(only_this_mac(), "{:?}", factoryd::remote::devices(&home(&dir)));
    // A new code with the person's account: in at once, no longer removed.
    let (token, made) = factoryd::remote::pair(&home(&dir), "Phone").expect("paired");
    let (mut app, answer) = knock(&local, &phone, &token, &assertion(&instance, &account, &phone)).await;
    assert_eq!(welcomed(&mut app, answer).await.device_id, made.id);
    assert!(removed().is_empty(), "{:?}", removed());
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
    assert_eq!(pending_number(&view, &pending.device_id).as_deref(), Some(""), "the digits are only on the device");
    let number = digits(&local, &token, &mac);
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

/// What a connection is told as it's turned away.
async fn turned_away(client: &mut Client) -> v1::Error {
    client.wait_for(|body| if let Body::Error(error) = body { Some(error.clone()) } else { None }).await
}

/// Still connected: it's answered.
async fn answers(client: &mut Client) {
    let reply = client.request(Request::ShowRecoveryKey(v1::ShowRecoveryKey {})).await;
    assert!(!matches!(reply.body, Body::Error(ref error) if error.code() == ErrorCode::SignInRequired), "{:?}", reply.body);
}

/// The first device found through the account, connected.
async fn through_the_account(instance: &Instance, account: &str, local: &Code) -> Client {
    let key = new_device().await;
    let (mut phone, answer) = knock(local, &key, "", &assertion(instance, account, &key)).await;
    welcomed(&mut phone, answer).await;
    phone
}

/// Row 713 (Fugo 49686): signing out closes the connections of devices the
/// account let in, as a new connection would be turned away, within a
/// second and after the sign-out's own answer; a device paired with a code
/// and this computer's app stay connected.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn signing_out_closes_the_connections_the_account_let_in_and_only_those() {
    let dir = temp_dir();
    let instance = Instance::start(&dir).await;
    let (_factory, mut client, _view, local) = signed_in(&dir, &instance).await;
    let account = account_id(&dir);
    let mut phone = through_the_account(&instance, &account, &local).await;
    // Paired with a code (and signed in to the account, so in at once):
    // not found through the account, so the sign-in doesn't hold it open.
    let (token, _) = factoryd::remote::pair(&home(&dir), "iPad").expect("paired");
    let key = new_device().await;
    let (mut ipad, answer) = knock(&local, &key, &token, &assertion(&instance, &account, &key)).await;
    welcomed(&mut ipad, answer).await;

    // The phone signs this Claktory out itself: it hears the Ack first.
    phone.ok(Request::AccountSignOut(v1::AccountSignOut {})).await;
    let started = std::time::Instant::now();
    let why = turned_away(&mut phone).await;
    assert_eq!(why.code(), ErrorCode::SignInRequired, "{why:?}");
    assert!(why.message.contains("signed out of your Claktory account"), "{why:?}");
    assert!(started.elapsed() < Duration::from_secs(1), "{:?}", started.elapsed());
    answers(&mut ipad).await;
    answers(&mut client).await;
}

/// Row 713: switched to another account, a device let in only through the
/// one before is turned away, as the gate would turn it away now.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn switching_accounts_closes_the_connections_the_account_before_let_in() {
    let dir = temp_dir();
    let instance = Instance::start(&dir).await;
    let (_factory, mut client, _view, local) = signed_in(&dir, &instance).await;
    let account = account_id(&dir);
    let key = new_device().await;
    let (mut phone, answer) = knock(&local, &key, "", &assertion(&instance, &account, &key)).await;
    welcomed(&mut phone, answer).await;
    crate::claktory_account::sign_in(&mut client, &instance, "ana@example.com").await;
    assert_ne!(account_id(&dir), account, "switched");
    // It's this Claktory that changed accounts, and the words say so
    // (Zucchero's row 718, Trish 50372), with its new email masked.
    let switched = |why: &v1::Error| {
        why.code() == ErrorCode::OtherAccount
            && why.other_account_before
            && why.message.contains("is now signed in as a…\u{2060}@example.com, a different account, so it no longer lets this device in")
    };
    let why = turned_away(&mut phone).await;
    assert!(switched(&why), "{why:?}");
    // Connecting again, it hears the same.
    let (_, answer) = knock(&local, &key, "", &assertion(&instance, &account, &key)).await;
    assert!(matches!(&answer, Body::Error(why) if switched(why)), "{answer:?}");
    answers(&mut client).await;
}

/// Row 713: removed at claktory.com and seen at a check-in, it turns away
/// the devices the account let in.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_claktory_removed_at_the_service_closes_the_connections_the_account_let_in() {
    let dir = temp_dir();
    let instance = Instance::start(&dir).await;
    let (factory, mut client, _view, local) = signed_in(&dir, &instance).await;
    let account = account_id(&dir);
    let mut phone = through_the_account(&instance, &account, &local).await;
    instance.sql("UPDATE claktories SET revoked_at = 1");
    factory.check_in_now();
    let why = turned_away(&mut phone).await;
    assert_eq!(why.code(), ErrorCode::SignInRequired, "{why:?}");
    answers(&mut client).await;
}

/// The paired device with this id, as the Workspace shows it.
fn shown(view: &View, id: &str) -> Option<v1::PairedDevice> {
    remote(view).devices.into_iter().find(|device| device.id == id)
}

/// What the Workspace says of a waiting device's four digits: never
/// them, so empty once it waits.
fn pending_number(view: &View, id: &str) -> Option<String> {
    remote(view).pending_devices.into_iter().find(|device| device.device_id == id).map(|device| device.number)
}

/// The four digits a device paired with `token` shows, worked out as it
/// does.
fn digits(local: &Code, token: &str, device: &Endpoint) -> String {
    let hashed: String = sha2::Sha256::digest(token.as_bytes()).iter().map(|byte| format!("{byte:02x}")).collect();
    factoryd::remote::approval_number(&local.endpoint.to_string(), &device.id().to_string(), &hashed)
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
    // Told to the person's devices as an alert, no longer as a notice.
    view.follow(&mut client, |view| view.workspace.alerts.iter().any(|alert| alert.device_id == made_tablet.id)).await;
    assert!(remote(&view).allow_notices.is_empty());
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

/// Every way a device gets in is told to the person's other devices at
/// once: an urgent alert, pushed even to a device in use, said once however
/// often the device connects after. The device let in can't mark it read;
/// another of the person's can.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_device_let_in_is_told_to_the_persons_other_devices_once_and_it_cant_hide_that() {
    use base64::Engine as _;
    let dir = temp_dir();
    let instance = Instance::start(&dir).await;
    let (_factory, mut client, mut view, local) = signed_in(&dir, &instance).await;
    let account = account_id(&dir);
    let relay = crate::notify::relay().await;
    apply_request(&mut client, &mut view, crate::notify::settings(|settings| settings.relay = relay.url.clone())).await;
    let key = [42u8; 32];
    let register = v1::RegisterNotifications {
        handle: base64::engine::general_purpose::URL_SAFE_NO_PAD.encode([9u8; 32]),
        key: key.to_vec(),
        platform: v1::PushPlatform::Apns.into(),
        ..Default::default()
    };
    assert!(matches!(apply_request(&mut client, &mut view, Request::RegisterNotifications(register)).await, Body::Ack(_)));
    client.ok(crate::notify::set_active(true)).await;
    let about = |view: &View, id: &str| view.workspace.alerts.iter().filter(|alert| alert.device_id == id).cloned().collect::<Vec<_>>();
    let ending = " If that wasn't you, remove it in Settings → Devices.";

    // Allowed at a terminal, by another process: told once it comes in.
    let (token, tablet_made) = factoryd::remote::pair(&home(&dir), "Tablet").expect("paired");
    let tablet = new_device().await;
    let (mut tablet_app, answer) = knock(&local, &tablet, &token, "").await;
    assert!(matches!(answer, Body::PairingPending(_)), "{answer:?}");
    factoryd::remote::approve(&home(&dir), &tablet_made.id).expect("allowed");
    let answer = tablet_app.receive().await.body.expect("an answer");
    welcomed(&mut tablet_app, answer).await;
    view.follow(&mut client, |view| !about(view, &tablet_made.id).is_empty()).await;
    let alert = about(&view, &tablet_made.id).remove(0);
    assert!(alert.title.starts_with("Tablet can now use ") && alert.urgent, "{alert:?}");
    assert!(alert.text.starts_with("Allowed at a terminal on ") && alert.text.ends_with(ending), "{alert:?}");
    let (_, pushed) = crate::notify::open(&key, &relay.next(1).await);
    assert_eq!((pushed["kind"].as_str(), pushed["urgent"].as_bool()), (Some("alert"), Some(true)), "pushed though the Mac is in use");
    assert_eq!(pushed["title"], alert.title.as_str());

    // Connecting again tells nothing more.
    drop(tablet_app);
    let (mut tablet_app, answer) = knock(&local, &tablet, &token, "").await;
    welcomed(&mut tablet_app, answer).await;

    // The tablet can't mark the alert about itself read.
    let read = v1::ReadAlerts { ids: vec![alert.id.clone()] };
    let answer = tablet_app.request(Request::ReadAlerts(read.clone())).await;
    assert_eq!(refused(&answer.body).0, ErrorCode::InvalidRequest);
    // An agent's alert in the same words within the half hour folds
    // nothing into it, nor it into that: it still names the tablet, which
    // still can't mark it read.
    apply_request(&mut client, &mut view, crate::common::create_project("Site", &dir.join("site"))).await;
    apply_request(&mut client, &mut view, crate::common::create_agent("site", "Builder", "")).await;
    let mut notes = Vec::new();
    let same = serde_json::json!({"title": alert.title, "text": "Same words."});
    crate::notify::call(&mut client, &mut view, &mut notes, "builder", "notify_person", same).await;
    let agents = view.workspace.alerts.iter().filter(|other| other.agent_id == "builder").collect::<Vec<_>>();
    assert_eq!(agents.len(), 1, "kept as its own: {:?}", view.workspace.alerts);
    assert_eq!(about(&view, &tablet_made.id).len(), 1);
    assert!(about(&view, &tablet_made.id)[0].also_from.is_empty());
    let answer = tablet_app.request(Request::ReadAlerts(read)).await;
    assert_eq!(refused(&answer.body).0, ErrorCode::InvalidRequest);

    // Allowed with the digits on this computer's own app: told at once,
    // from it, never with the digits.
    let (token, phone_made) = factoryd::remote::pair(&home(&dir), "Phone").expect("paired");
    let phone = new_device().await;
    let (mut phone_app, answer) = knock(&local, &phone, &token, "").await;
    assert!(matches!(answer, Body::PairingPending(_)), "{answer:?}");
    view.follow(&mut client, |view| pending_number(view, &phone_made.id).is_some()).await;
    let number = digits(&local, &token, &phone);
    let allow = v1::ApproveDevice { device_id: phone_made.id.clone(), number: number.clone(), ..Default::default() };
    apply_request(&mut client, &mut view, Request::ApproveDevice(allow)).await;
    view.follow(&mut client, |view| !about(view, &phone_made.id).is_empty()).await;
    let alert = about(&view, &phone_made.id).remove(0);
    assert!(alert.text.starts_with("Allowed from ") && alert.text.ends_with(ending), "{alert:?}");
    assert!(!format!("{alert:?}").contains(&number), "never the digits");
    let answer = phone_app.receive().await.body.expect("an answer");
    welcomed(&mut phone_app, answer).await;

    // A code and the account: in at once, and told too.
    let (token, laptop_made) = factoryd::remote::pair(&home(&dir), "Laptop").expect("paired");
    let laptop = new_device().await;
    let (mut laptop_app, answer) = knock(&local, &laptop, &token, &assertion(&instance, &account, &laptop)).await;
    welcomed(&mut laptop_app, answer).await;
    view.follow(&mut client, |view| !about(view, &laptop_made.id).is_empty()).await;
    let alert = about(&view, &laptop_made.id).remove(0);
    assert_eq!(alert.text, format!("Allowed with its code and your Claktory account.{ending}"));

    // Each was told once, the tablet's reconnect included; another device
    // of the person's marks them read.
    assert_eq!(about(&view, &tablet_made.id).len(), 1);
    assert_eq!(about(&view, &phone_made.id).len(), 1);
    let ids = [&tablet_made.id, &phone_made.id, &laptop_made.id]
        .iter()
        .flat_map(|id| about(&view, id))
        .map(|alert| alert.id)
        .collect::<Vec<_>>();
    phone_app.ok(Request::ReadAlerts(v1::ReadAlerts { ids: vec![about(&view, &tablet_made.id)[0].id.clone()] })).await;
    apply_request(&mut client, &mut view, Request::ReadAlerts(v1::ReadAlerts { ids })).await;
    view.follow(&mut client, |view| view.workspace.alerts.iter().all(|alert| alert.device_id.is_empty())).await;
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
    let number = digits(&local, &token, &phone);
    let allow = v1::ApproveDevice { device_id: by_app.id.clone(), number: number.clone(), ..Default::default() };
    apply_request(&mut client, &mut view, Request::ApproveDevice(allow)).await;
    let answer = phone_app.receive().await.body.expect("an answer");
    welcomed(&mut phone_app, answer).await;
    view.follow(&mut client, |view| shown(view, &by_app.id).is_some_and(|device| device.confirmed)).await;
    let listed = shown(&view, &by_app.id).expect("listed");
    assert_eq!((listed.allowed_by.as_str(), listed.allowed_by_device_id.as_str()), ("device", "local"));
    assert!(!listed.allowed_by_name.is_empty());
    view.follow(&mut client, |view| view.workspace.alerts.iter().any(|alert| alert.device_id == by_app.id)).await;
    let alert = view.workspace.alerts.iter().find(|alert| alert.device_id == by_app.id).expect("alerted");
    assert!(!format!("{alert:?}").contains(&number), "never the digits");

    // The unconfirmed device allows one: let in, not confirmed.
    let (token, by_device) = factoryd::remote::pair(&home(&dir), "Laptop").expect("paired");
    let laptop = new_device().await;
    let (mut laptop_app, answer) = knock(&local, &laptop, &token, "").await;
    assert!(matches!(answer, Body::PairingPending(_)), "{answer:?}");
    view.follow(&mut client, |view| pending_number(view, &by_device.id).is_some()).await;
    let number = digits(&local, &token, &laptop);
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

/// What the service gives a device signed in to `account` through the
/// device whose account key is `account_key` (b64), with tunnel key `device`.
fn assertion_with_key(instance: &Instance, account: &str, account_key: &str, device: &Endpoint) -> String {
    let now = jiff::Timestamp::now().as_second();
    let said = DeviceAssertion {
        v: 1,
        account_id: account.to_owned(),
        account_key: account_key.to_owned(),
        device_key: device.id().to_string(),
        iat: now,
        exp: now + 3600,
    };
    sign_device_assertion(&ed25519_dalek::SigningKey::from_bytes(&instance.secrets.1), &said)
}

/// #619 (Fugo 53968): this computer's own app, signed in through this
/// Claktory, reaching it through the account (an entry of its own for it,
/// beside This Mac) is only turned away: no one waits to be allowed, and it
/// isn't let in. Any other key is a newcomer as ever; after a sign-out the
/// key vouched for before counts for nothing; and the app's own connection
/// works as before.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn this_computers_own_app_through_the_account_is_turned_away_not_asked_about() {
    let dir = temp_dir();
    let instance = Instance::start(&dir).await;
    let (_factory, mut client, mut view, local) = signed_in(&dir, &instance).await;
    let account = account_id(&dir);
    // The first device through the account is in at once: one, so the
    // others are newcomers.
    let first = new_device().await;
    let (mut phone, answer) = knock(&local, &first, "", &assertion(&instance, &account, &first)).await;
    welcomed(&mut phone, answer).await;
    let key = ed25519_dalek::SigningKey::from_bytes(&[7; 32]);
    let app_key = claktory_account::protocol::b64(key.verifying_key().as_bytes());
    let Body::DeviceVouched(vouched) = client.request(vouch(&key)).await.body else { panic!("expected DeviceVouched") };
    assert!(vouched.refused.is_empty() && !vouched.code.is_empty(), "{vouched:?}");
    let relays = v1::SetRemoteAccess { relays: vec!["http://127.0.0.1:9/".into()], set_relays: true, ..Default::default() };
    apply_request(&mut client, &mut view, Request::SetRemoteAccess(relays)).await;
    let devices = remote(&view).devices.len();
    assert_eq!(devices, 1, "the phone");

    // Its key through the tunnel: turned away, and nothing waits.
    let own = new_device().await;
    let (_, answer) = knock(&local, &own, "", &assertion_with_key(&instance, &account, &app_key, &own)).await;
    let (code, reason) = refused(&answer);
    assert!(code == ErrorCode::InvalidRequest && reason.contains("this computer's own Claktory"), "{code:?} {reason}");
    // Its own connection works as before, and shows no one waiting.
    let relays = v1::SetRemoteAccess { relays: vec!["http://127.0.0.1:9/".into()], set_relays: true, ..Default::default() };
    assert!(!matches!(apply_request(&mut client, &mut view, Request::SetRemoteAccess(relays)).await, Body::Error(_)));
    assert!(remote(&view).pending_devices.is_empty(), "{:?}", remote(&view).pending_devices);
    assert_eq!(remote(&view).devices.len(), devices);

    // Another device's key: a newcomer, as ever.
    let other = new_device().await;
    let (_, answer) = knock(&local, &other, "", &assertion_with_key(&instance, &account, "another-device-key", &other)).await;
    assert!(matches!(answer, Body::PairingPending(_)), "{answer:?}");

    // Signed out and in again: the key vouched for before is any key.
    client.ok(Request::AccountSignOut(v1::AccountSignOut {})).await;
    view.workspace = sign_in(&mut client, &instance, "matt@example.com").await;
    let again = new_device().await;
    let (_, answer) = knock(&local, &again, "", &assertion_with_key(&instance, &account, &app_key, &again)).await;
    assert!(matches!(answer, Body::PairingPending(_)), "{answer:?}");
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

/// The device's connection has closed: nothing more comes.
async fn closed(client: &mut Client) {
    assert!(tokio::time::timeout(Duration::from_secs(10), client.closed()).await.expect("closed in time"), "still open");
}

/// E11-8 point 6: Lock out on one of the person's devices unpairs the
/// locked-out device here at once: its connection closes, and no assertion
/// for its account key lets it in again. Only the person can ask, with a
/// real account key; asked again, nothing changes.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn lock_out_unpairs_the_device_and_its_account_key_lets_nothing_in_again() {
    let dir = temp_dir();
    let instance = Instance::start(&dir).await;
    let (addr, _factory, mut client, mut view, local) = signed_in_at(&dir, &instance).await;
    let account = account_id(&dir);
    let stolen = new_device().await;
    let (mut ipad, answer) = knock(&local, &stolen, "", &assertion(&instance, &account, &stolen)).await;
    let id = welcomed(&mut ipad, answer).await.device_id;
    let key = account_key(&stolen);
    let recorded = factoryd::remote::devices(&home(&dir)).expect("devices").into_iter().find(|device| device.id == id).expect("paired");
    assert_eq!(recorded.account_key, key, "kept from its assertion");

    // An agent's command can't, and a key that isn't one is refused.
    let mut tool = Client::open(addr).await;
    let hello = v1::Hello {
        protocol_version: PROTOCOL_VERSION,
        client_version: "agent".into(),
        token: crate::common::tool_token(addr),
        ..Default::default()
    };
    tool.send(v1::ClientMessage { id: 1, body: Some(Request::Hello(hello)) }).await;
    tool.welcomed().await;
    let lock_out = |key: &str| Request::UnpairAccountKey(v1::UnpairAccountKey { account_key: key.to_owned() });
    assert_eq!(refused(&tool.request(lock_out(&key)).await.body).0, ErrorCode::PersonOnly);
    assert_eq!(refused(&client.request(lock_out("not-a-key")).await.body).0, ErrorCode::InvalidRequest);
    assert!(factoryd::remote::devices(&home(&dir)).expect("devices").iter().any(|device| device.id == id), "still paired");

    // The person locks it out: unpaired, its connection closed at once.
    assert!(matches!(apply_request(&mut client, &mut view, lock_out(&key)).await, Body::Ack(_)));
    closed(&mut ipad).await;
    view.follow(&mut client, |view| remote(view).devices.iter().all(|device| device.id != id)).await;
    let kept = factoryd::remote::removed_keys(&home(&dir));
    assert_eq!((kept.len(), kept[0].account_key.as_str(), kept[0].why.as_str()), (1, key.as_str(), "lock_out"));
    // Back with a fresh assertion (one made before its removal, say): out,
    // as removed.
    let (_, answer) = knock(&local, &stolen, "", &assertion(&instance, &account, &stolen)).await;
    let (code, reason) = refused(&answer);
    assert!(code == ErrorCode::Unauthorized && reason.contains("removed from your account"), "{reason}");
    // Again: nothing changes.
    assert!(matches!(apply_request(&mut client, &mut view, lock_out(&key)).await, Body::Ack(_)));
    assert_eq!(factoryd::remote::removed_keys(&home(&dir)).len(), 1);
}

/// E11-8 point 6: a Claktory that was off while the person locked a device
/// out hears of it at its first check-in, before it lets that device in.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_claktory_off_during_lock_out_turns_the_device_away_when_it_comes_back() {
    let dir = temp_dir();
    let instance = Instance::start(&dir).await;
    let stolen = new_device().await;
    let (account, before) = {
        let (_factory, _client, _view, local) = signed_in(&dir, &instance).await;
        let account = account_id(&dir);
        let (mut ipad, answer) = knock(&local, &stolen, "", &assertion(&instance, &account, &stolen)).await;
        welcomed(&mut ipad, answer).await;
        (account, local_pairing(&dir).await)
    };
    // Locked out on another device meanwhile (after the last check-in's
    // cursor): the service has it.
    instance.sql(&format!(
        "INSERT INTO removed_devices (account_id, account_key, removed) VALUES ('{account}', '{}', CAST(strftime('%s', 'now') AS INTEGER))",
        account_key(&stolen)
    ));
    let (_addr, factory) = open(&dir, &instance.url).await;
    factory.set_tunnel_online_wait(Duration::from_millis(100));
    // This start's end of the tunnel.
    let mut code = local_pairing(&dir).await;
    for _ in 0..200 {
        if code != before {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
        code = local_pairing(&dir).await;
    }
    let local = parse(&code);
    // Turned away at its Hello, or already at the tunnel when the check-in
    // at start unpaired it first.
    match dial(&stolen, &local, &local.addresses).await {
        Err(error) => assert!(error.to_string().contains("unpaired"), "{error}"),
        Ok(connection) => {
            let (_, answer) = knock_over(connection, &stolen, "", &assertion(&instance, &account, &stolen)).await;
            let (code, reason) = refused(&answer);
            assert!(code == ErrorCode::Unauthorized && reason.contains("removed from your account"), "{reason}");
        }
    }
    // Unpaired, as the check-in that heard of it saves (the first start of
    // this home still runs in this test, beside the second).
    let unpaired = || factoryd::remote::devices(&home(&dir)).expect("devices").iter().all(|device| device.account_key != account_key(&stolen));
    for _ in 0..100 {
        if unpaired() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(unpaired(), "unpaired: {:?}", factoryd::remote::devices(&home(&dir)));
}

/// A request to the account service signed as a device by `key`, with a
/// body (POST) or none (GET).
async fn device_call(
    instance: &Instance,
    key: &ed25519_dalek::SigningKey,
    method: &str,
    path: &str,
    body: Option<serde_json::Value>,
) -> (u16, serde_json::Value) {
    use claktory_account::protocol as api;
    // Each call a second later than the last, at least: the same request
    // signed twice at one time is refused as replayed.
    static LAST: std::sync::atomic::AtomicI64 = std::sync::atomic::AtomicI64::new(0);
    let body = body.map(|body| serde_json::to_vec(&body).expect("json")).unwrap_or_default();
    let now = jiff::Timestamp::now().as_second();
    let time = LAST.fetch_max(now, std::sync::atomic::Ordering::SeqCst).max(now - 1) + 1;
    LAST.fetch_max(time, std::sync::atomic::Ordering::SeqCst);
    let mut request = reqwest::Client::new()
        .request(method.parse().expect("a method"), format!("{}{path}", instance.url))
        .header("authorization", format!("Device {}", api::b64(key.verifying_key().as_bytes())))
        .header(api::TIME_HEADER, time.to_string())
        .header(api::SIGNATURE_HEADER, api::sign_request(key, method, path, time, &body));
    if !body.is_empty() {
        request = request.header("content-type", "application/json").body(body);
    }
    let answer = request.send().await.expect("the account service");
    let status = answer.status().as_u16();
    (status, answer.json().await.unwrap_or_default())
}

/// A phone that signs in to the account itself, with the emailed code.
async fn device_signs_in(instance: &Instance, key: &ed25519_dalek::SigningKey) {
    use claktory_account::protocol as api;
    let start = serde_json::json!({
        "email": "matt@example.com",
        "account_key": api::b64(key.verifying_key().as_bytes()),
        "name": "Matt's iPhone",
        "platform": "ios",
        "version": "v0.1.23",
    });
    let (status, started) = device_call(instance, key, "POST", "/v1/devices/sign-in/start", Some(start)).await;
    assert_eq!(status, 200, "{started}");
    let verify = serde_json::json!({"request_id": started["request_id"], "code": instance.last_code("matt@example.com")});
    let (status, signed) = device_call(instance, key, "POST", "/v1/devices/sign-in/verify", Some(verify)).await;
    assert_eq!(status, 200, "{signed}");
}

/// What the service gives the device with account key `key` for tunnel key
/// `device`.
fn assertion_for(instance: &Instance, account: &str, key: &ed25519_dalek::SigningKey, device: &Endpoint) -> String {
    let now = jiff::Timestamp::now().as_second();
    let said = DeviceAssertion {
        v: 1,
        account_id: account.to_owned(),
        account_key: claktory_account::protocol::b64(key.verifying_key().as_bytes()),
        device_key: device.id().to_string(),
        iat: now,
        exp: now + 3600,
    };
    sign_device_assertion(&ed25519_dalek::SigningKey::from_bytes(&instance.secrets.1), &said)
}

/// The no-code setup (server-authorize.md): an agent's command on a new
/// server asks; the person's devices hear the bell from their Claktory; the
/// phone gets that Claktory's word that it's allowed there and approves;
/// the server is signed in, and lets in at once only the phone that
/// approved it. An agent's command can't ask on a Claktory that's in use.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_server_an_agent_sets_up_joins_the_account_once_a_phone_approves_it_and_lets_in_only_that_phone() {
    use claktory_account::protocol as api;
    let dir = temp_dir();
    let instance = Instance::start(&dir).await;
    let (studio_addr, studio, mut mac, mut mac_view, _) = signed_in_at(&dir, &instance).await;
    let account = account_id(&dir);
    let phone_key = ed25519_dalek::SigningKey::from_bytes(&[11; 32]);
    device_signs_in(&instance, &phone_key).await;

    // An agent's command on the studio, which is in use: refused.
    let flags = |line: &str| factoryd::cli::Flags::parse(&line.split(' ').map(str::to_owned).collect::<Vec<_>>()).expect("a command");
    let studio_home = factoryd::home::Instance { home: home(&dir), addr: studio_addr };
    let asked = factoryd::cli::run(&studio_home, &flags("account authorize --email matt@example.com --json"), String::new).await;
    assert_eq!(asked.exit, factoryd::cli::Exit::Ok, "already signed in: {}", asked.text);

    // A new server; an agent's command there asks the person's devices.
    let server_dir = temp_dir();
    let (server_addr, server) = open(&server_dir, &instance.url).await;
    server.set_tunnel_online_wait(Duration::from_millis(100));
    let server_home = factoryd::home::Instance { home: home(&server_dir), addr: server_addr };
    let authorizing = tokio::spawn(async move {
        let flags = factoryd::cli::Flags::parse(&["account", "authorize", "--email", "matt@example.com", "--json"].map(str::to_owned))
            .expect("a command");
        factoryd::cli::run(&server_home, &flags, String::new).await
    });

    // The phone sees the request, from the service.
    let mut waiting = serde_json::Value::Null;
    for _ in 0..200 {
        let (status, shown) = device_call(&instance, &phone_key, "GET", "/v1/devices/authorizations", None).await;
        assert_eq!(status, 200, "{shown}");
        if let Some(first) = shown["authorizations"].as_array().and_then(|all| all.first()).filter(|first| first["state"] == "waiting") {
            waiting = first.clone();
            break;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    let id = waiting["id"].as_str().expect("a request waiting").to_owned();
    assert_eq!(waiting["may_approve"], true, "{waiting}");

    // The studio rings the bell on the person's devices: the request's id,
    // nothing more.
    factoryd::ring_account_bell_for_tests(&studio);
    mac_view
        .follow(&mut mac, |view| view.workspace.account.as_ref().is_some_and(|account| account.authorizations_waiting == [id.clone()]))
        .await;
    // Never to an agent's command.
    let (_tool, seen) = tool_client(studio_addr).await;
    assert!(seen.account.expect("the account").authorizations_waiting.is_empty());

    // The phone, connected to the studio, asks for its word that it's
    // allowed there.
    let (mut phone_app, _) = {
        let mut client = Client::open(studio_addr).await;
        let tunnel = new_device().await;
        let hello = v1::Hello {
            protocol_version: PROTOCOL_VERSION,
            client_version: "test".into(),
            token: factoryd::authority::person_token_for_test(home(&dir).root()),
            assertion: assertion_for(&instance, &account, &phone_key, &tunnel),
            ..Default::default()
        };
        client.send(v1::ClientMessage { id: 1, body: Some(Request::Hello(hello)) }).await;
        let welcomed = client.welcomed().await;
        (client, welcomed)
    };
    let answer = phone_app.request(Request::ProveAllowedDevice(v1::ProveAllowedDevice { request_id: id.clone() })).await;
    let Body::AllowedDeviceWord(word) = answer.body else { panic!("expected the word, got {:?}", answer.body) };
    assert_eq!(word.request_id, id);
    let backend_key = factoryd::account_backend_key_for_tests(&studio);
    let said = api::verify_allowed_device(&backend_key, &word.allowed, jiff::Timestamp::now().as_second()).expect("the studio's word");
    assert_eq!(said.account_key, api::b64(phone_key.verifying_key().as_bytes()), "for the key the phone's connection proved");
    // An agent's command can't ask for one.
    let mut tool = Client::open(studio_addr).await;
    let hello = v1::Hello {
        protocol_version: PROTOCOL_VERSION,
        client_version: "factoryd-cli test".into(),
        token: crate::common::tool_token(studio_addr),
        ..Default::default()
    };
    tool.send(v1::ClientMessage { id: 1, body: Some(Request::Hello(hello)) }).await;
    tool.welcomed().await;
    let refused_word = tool.request(Request::ProveAllowedDevice(v1::ProveAllowedDevice { request_id: id.clone() })).await;
    assert_eq!(refused(&refused_word.body).0, ErrorCode::PersonOnly);

    // The phone approves with it; the server is signed in.
    let decision = serde_json::json!({"approve": true, "fingerprint": waiting["fingerprint"], "allowed": word.allowed});
    let (status, decided) = device_call(&instance, &phone_key, "POST", &format!("/v1/devices/authorizations/{id}"), Some(decision)).await;
    assert_eq!(status, 200, "{decided}");
    let done = authorizing.await.expect("the command");
    assert_eq!(done.exit, factoryd::cli::Exit::Ok, "{}", done.text);
    assert_eq!(done.json["state"], "signed_in", "{}", done.json);

    // Only the phone that approved is let in at once; another device of the
    // person's waits to be allowed.
    let server_code = parse(&local_pairing(&server_dir).await);
    let phone_tunnel = new_device().await;
    let (mut phone_on_server, answer) =
        knock(&server_code, &phone_tunnel, "", &assertion_for(&instance, &account, &phone_key, &phone_tunnel)).await;
    welcomed(&mut phone_on_server, answer).await;
    let laptop_key = ed25519_dalek::SigningKey::from_bytes(&[12; 32]);
    let laptop = new_device().await;
    let (_laptop, answer) = knock(&server_code, &laptop, "", &assertion_for(&instance, &account, &laptop_key, &laptop)).await;
    assert!(matches!(answer, Body::PairingPending(_)), "only the phone that approved: {answer:?}");

    // Now in use, the server takes no more requests from agents' commands.
    let mut tool = Client::open(server_addr).await;
    let hello = v1::Hello {
        protocol_version: PROTOCOL_VERSION,
        client_version: "factoryd-cli test".into(),
        token: crate::common::tool_token(server_addr),
        ..Default::default()
    };
    tool.send(v1::ClientMessage { id: 1, body: Some(Request::Hello(hello)) }).await;
    tool.welcomed().await;
    let again = tool.request(Request::AccountAuthorize(v1::AccountAuthorize { email: "someone@example.com".into() })).await;
    assert_eq!(refused(&again.body).0, ErrorCode::PersonOnly);
}

/// The bell is asked for while an app is connected. An app connecting
/// after the account's loop last looked, with none connected then, hears
/// a computer that asks to join within the bell's minute, not at the next
/// hourly check-in.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_app_connecting_hears_a_computer_waiting_to_join_without_waiting_for_a_check_in() {
    let dir = temp_dir();
    let instance = Instance::start(&dir).await;
    let (studio_addr, studio, mac, mac_view, _) = signed_in_at(&dir, &instance).await;
    let phone_key = ed25519_dalek::SigningKey::from_bytes(&[11; 32]);
    device_signs_in(&instance, &phone_key).await;

    // No app connected; the loop looks once, with nobody to ring.
    drop((mac, mac_view));
    for _ in 0..100 {
        if factoryd::apps_connected_for_tests(&studio) == 0 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert_eq!(factoryd::apps_connected_for_tests(&studio), 0);
    factoryd::ring_account_bell_for_tests(&studio);

    let server_dir = temp_dir();
    let (server_addr, server) = open(&server_dir, &instance.url).await;
    server.set_tunnel_online_wait(Duration::from_millis(100));
    let server_home = factoryd::home::Instance { home: home(&server_dir), addr: server_addr };
    let _authorizing = tokio::spawn(async move {
        let flags = factoryd::cli::Flags::parse(&["account", "authorize", "--email", "matt@example.com", "--json"].map(str::to_owned))
            .expect("a command");
        factoryd::cli::run(&server_home, &flags, String::new).await
    });
    let mut id = String::new();
    for _ in 0..200 {
        let (status, shown) = device_call(&instance, &phone_key, "GET", "/v1/devices/authorizations", None).await;
        assert_eq!(status, 200, "{shown}");
        if let Some(waiting) = shown["authorizations"].as_array().and_then(|all| all.first()).filter(|first| first["state"] == "waiting") {
            id = waiting["id"].as_str().expect("its id").to_owned();
            break;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    assert!(!id.is_empty(), "a computer waits to join");

    let (mut mac, workspace, _) = Client::connect(studio_addr).await;
    let mut view = View { workspace, ..View::default() };
    let heard =
        view.follow(&mut mac, |view| view.workspace.account.as_ref().is_some_and(|account| account.authorizations_waiting == [id.clone()]));
    tokio::time::timeout(Duration::from_secs(20), heard).await.expect("the bell rings once an app connects");
}

/// A move with the account, end to end through the account service: the other computer
/// runs receive with the account's email and shows no code; the phone,
/// connected to the studio, finds the move only in Settings → Move (never
/// with the bell), gets the studio's word and approves with its own
/// signature; the studio checks it against its own devices, takes the move
/// and starts it with no digits, naming the phone; the other computer then
/// waits only for the studio's endpoint, and the service spent the move.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_move_through_the_account_starts_once_a_phone_approves_it_and_the_other_computer_waits_only_for_its_source() {
    use claktory_account::protocol as api;
    factoryd::move_run::hold_at_freeze();
    let dir = temp_dir();
    let instance = Instance::start(&dir).await;
    let (studio_addr, _studio, mut mac, _, _) = signed_in_at(&dir, &instance).await;
    let account = account_id(&dir);
    let studio_id = {
        let text = std::fs::read_to_string(dir.join("home/account/account.json")).expect("account.json");
        serde_json::from_str::<serde_json::Value>(&text).expect("json")["claktory_id"].as_str().expect("claktory_id").to_owned()
    };
    let phone_key = ed25519_dalek::SigningKey::from_bytes(&[13; 32]);
    device_signs_in(&instance, &phone_key).await;
    // The phone, as this computer's app, proving its account key.
    let mut phone_app = Client::open(studio_addr).await;
    let tunnel = new_device().await;
    let hello = v1::Hello {
        protocol_version: PROTOCOL_VERSION,
        client_version: "test".into(),
        token: factoryd::authority::person_token_for_test(home(&dir).root()),
        assertion: assertion_for(&instance, &account, &phone_key, &tunnel),
        ..Default::default()
    };
    phone_app.send(v1::ClientMessage { id: 1, body: Some(Request::Hello(hello)) }).await;
    phone_app.welcomed().await;

    // The other computer asks the account to receive a Claktory, a second
    // after the studio signed in (the service takes the word only of a
    // Claktory signed in before the request).
    tokio::time::sleep(Duration::from_millis(1100)).await;
    let target = temp_dir();
    let there = factoryd::home::Home::new(target.join("home"));
    let said = std::sync::Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    let codes = std::sync::Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    let ui = factoryd::move_import::receive::Ui {
        code: {
            let codes = codes.clone();
            std::sync::Arc::new(move |code| codes.lock().unwrap().push(code.secret.clone()))
        },
        say: {
            let said = said.clone();
            std::sync::Arc::new(move |text| said.lock().unwrap().push(text.to_owned()))
        },
        confirm: std::sync::Arc::new(|_| Box::pin(async { panic!("no digits to confirm") })),
    };
    let options = factoryd::move_import::receive::Options {
        local_only: true,
        email: Some("matt@example.com".into()),
        service: Some(instance.url.clone()),
        ..Default::default()
    };
    let receiving = tokio::spawn(async move {
        let nowhere = std::net::SocketAddr::from(([127, 0, 0, 1], 1));
        factoryd::move_import::receive::receive(&there, nowhere, &options, &ui).await
    });

    // Only in Settings → Move: the phone finds it there, not with the bell.
    let mut waiting = serde_json::Value::Null;
    for _ in 0..200 {
        let (status, moves) = device_call(&instance, &phone_key, "GET", "/v1/devices/moves", None).await;
        assert_eq!(status, 200, "{moves}");
        if let Some(first) = moves["moves"].as_array().and_then(|all| all.first()).filter(|first| first["state"] == "waiting") {
            waiting = first.clone();
            break;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    let id = waiting["id"].as_str().expect("a move waiting").to_owned();
    assert_eq!(waiting["kind"], "move");
    let (_, joins) = device_call(&instance, &phone_key, "GET", "/v1/devices/authorizations", None).await;
    assert_eq!(joins["authorizations"], serde_json::json!([]), "never with the bell");
    assert!(codes.lock().unwrap().iter().all(String::is_empty), "no code shown");
    assert!(said.lock().unwrap().iter().any(|line| line.contains("Settings → Move")), "{:?}", said.lock().unwrap());
    // This computer's code, for the person to compare with the card's: the
    // fingerprint of the endpoint the card names.
    let shown = waiting["fingerprint"].as_str().expect("its fingerprint").to_uppercase();
    let code = shown.as_bytes().chunks(4).map(|group| std::str::from_utf8(group).expect("hex")).collect::<Vec<_>>().join(" ");
    assert!(said.lock().unwrap().iter().any(|line| line.contains(&format!("this computer's code: {code}."))), "{:?}", said.lock().unwrap());

    // The studio's word, and the phone's own approval.
    let answer = phone_app.request(Request::ProveAllowedDevice(v1::ProveAllowedDevice { request_id: id.clone() })).await;
    let Body::AllowedDeviceWord(word) = answer.body else { panic!("expected the word, got {:?}", answer.body) };
    let now = jiff::Timestamp::now().as_second();
    let approval = api::MoveApproval {
        v: 1,
        kind: api::KIND_MOVE.into(),
        request_id: id.clone(),
        account_id: account.clone(),
        source_claktory_id: studio_id.clone(),
        endpoint: waiting["endpoint"].as_str().expect("its endpoint").to_owned(),
        fingerprint: waiting["fingerprint"].as_str().expect("its fingerprint").to_owned(),
        device_key: api::b64(phone_key.verifying_key().as_bytes()),
        iat: now,
        exp: now + 300,
    };
    let decision = serde_json::json!({
        "approve": true,
        "fingerprint": waiting["fingerprint"],
        "allowed": word.allowed,
        "move_approval": api::sign_move_approval(&phone_key, &approval),
    });
    let (status, decided) = device_call(&instance, &phone_key, "POST", &format!("/v1/devices/authorizations/{id}"), Some(decision)).await;
    assert_eq!(status, 200, "{decided}");

    // The studio takes it and starts the move, no digits, naming the phone.
    let started = tokio::time::timeout(
        Duration::from_secs(60),
        mac.wait_for(|body| match body {
            Body::Workspace(workspace) => workspace.r#move.clone().filter(|seen| !seen.approval_key.is_empty()),
            _ => None,
        }),
    )
    .await
    .expect("the studio takes the move");
    assert_eq!(started.approval_key, api::b64(phone_key.verifying_key().as_bytes()));
    assert!(started.digits.is_empty(), "{started:?}");
    assert_ne!(started.phase(), v1::MovePhase::Confirm);

    // The other computer heard it: only the studio's endpoint, from now on.
    for _ in 0..200 {
        if said.lock().unwrap().iter().any(|line| line.starts_with("Approved.")) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(said.lock().unwrap().iter().any(|line| line.starts_with("Approved.")), "{:?}", said.lock().unwrap());
    let kept = std::fs::read_to_string(target.join("home/import/receive/approved.json")).expect("approved.json");
    let kept: serde_json::Value = serde_json::from_str(&kept).expect("json");
    assert_eq!(kept["source_claktory_id"], studio_id.as_str());
    assert_eq!(kept["source_key"].as_str().map(str::len), Some(64));
    // Spent at the service: not offered to the studio again.
    let (status, moves) = device_call(&instance, &phone_key, "GET", "/v1/devices/moves", None).await;
    assert_eq!((status, moves["moves"].clone()), (200, serde_json::json!([])));

    mac.ok(Request::CancelMove(v1::CancelMove {})).await;
    receiving.abort();
}

/// A tool connection on `addr`, as an agent's command makes one.
async fn tool_client(addr: std::net::SocketAddr) -> (Client, v1::Workspace) {
    let mut tool = Client::open(addr).await;
    let hello = v1::Hello {
        protocol_version: PROTOCOL_VERSION,
        client_version: "factoryd-cli test".into(),
        token: crate::common::tool_token(addr),
        ..Default::default()
    };
    tool.send(v1::ClientMessage { id: 1, body: Some(Request::Hello(hello)) }).await;
    let (workspace, _) = tool.welcomed().await;
    (tool, workspace)
}

/// An agent's command never starts the no-code setup on a
/// Claktory this computer's app has used (a Mac whose person hasn't signed
/// in yet, say), or an outsider's approval could let their device in. The
/// app's first Hello leaves a mark that outlasts its device's entry.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_agent_cant_start_the_no_code_setup_on_a_claktory_its_app_has_used() {
    let dir = temp_dir();
    let instance = Instance::start(&dir).await;
    let (addr, _factory) = open(&dir, &instance.url).await;
    let ask = || Request::AccountAuthorize(v1::AccountAuthorize { email: "outsider@example.com".into() });

    // The person's app has connected (and not signed in): refused.
    drop(Client::connect(addr).await);
    let (mut tool, _) = tool_client(addr).await;
    let answer = tool.request(ask()).await;
    assert_eq!(refused(&answer.body).0, ErrorCode::PersonOnly);

    // Its device's last connection forgotten, the mark still holds.
    let devices = home(&dir).root().join("remote/devices.json");
    let mut kept: Vec<serde_json::Value> = serde_json::from_str(&std::fs::read_to_string(&devices).expect("devices.json")).expect("json");
    let local = kept.iter_mut().find(|device| device["local"] == true).expect("the app's device");
    assert!(local["last_seen"].is_i64(), "seen: {local}");
    local.as_object_mut().unwrap().remove("last_seen");
    std::fs::write(&devices, serde_json::to_string(&kept).unwrap()).expect("devices.json");
    let answer = tool.request(ask()).await;
    assert_eq!(refused(&answer.body).0, ErrorCode::PersonOnly);
    assert!(home(&dir).root().join("remote/app-seen").exists());
}

/// Path 4b of the no-code setup: once the account has a device that may
/// approve, the emailed code alone no longer signs a further Claktory in:
/// it waits for the person's approval, and a device that declines ends it,
/// saying so.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_emailed_code_signs_in_a_further_claktory_only_once_the_person_approves_it() {
    let dir = temp_dir();
    let instance = Instance::start(&dir).await;
    let (_studio, _mac, _mac_view, _) = signed_in(&dir, &instance).await;
    let phone_key = ed25519_dalek::SigningKey::from_bytes(&[13; 32]);
    device_signs_in(&instance, &phone_key).await;

    // A laptop's Claktory, signed in by the person with the emailed code.
    let laptop_dir = temp_dir();
    let (laptop_addr, _laptop) = open(&laptop_dir, &instance.url).await;
    let (mut laptop, workspace, _) = Client::connect(laptop_addr).await;
    let mut view = View { workspace, ..View::default() };
    apply_request(&mut laptop, &mut view, Request::AccountSignIn(v1::AccountSignIn { email: "matt@example.com".into() })).await;
    let code = instance.last_code("matt@example.com");
    apply_request(&mut laptop, &mut view, Request::AccountVerify(v1::AccountVerify { code })).await;
    let state = |view: &View| view.workspace.account.clone().unwrap_or_default();
    view.follow(&mut laptop, |view| state(view).state() == v1::ClaktoryAccountState::AwaitingApproval).await;
    assert!(state(&view).approval_expires_ms > 0 && state(&view).code_expires_ms == 0);

    // The phone sees it and declines.
    let mut waiting = serde_json::Value::Null;
    for _ in 0..50 {
        let (_, shown) = device_call(&instance, &phone_key, "GET", "/v1/devices/authorizations", None).await;
        if let Some(first) = shown["authorizations"].as_array().and_then(|all| all.first()).filter(|first| first["state"] == "waiting") {
            waiting = first.clone();
            break;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    let id = waiting["id"].as_str().expect("a request waiting").to_owned();
    let decision = serde_json::json!({"approve": false, "fingerprint": waiting["fingerprint"]});
    let (status, decided) = device_call(&instance, &phone_key, "POST", &format!("/v1/devices/authorizations/{id}"), Some(decision)).await;
    assert_eq!(status, 200, "{decided}");
    view.follow(&mut laptop, |view| state(view).state() == v1::ClaktoryAccountState::SignedOut).await;
    assert!(state(&view).problem.contains("declined"), "{}", state(&view).problem);
}

/// The Welcome of a client on the loopback port whose Hello has `token`.
async fn welcome_on_loopback(addr: std::net::SocketAddr, token: String) -> (Client, v1::Welcome) {
    let mut client = Client::open(addr).await;
    let hello = v1::Hello { protocol_version: PROTOCOL_VERSION, client_version: "test".into(), token, ..Default::default() };
    client.send(v1::ClientMessage { id: 1, body: Some(Request::Hello(hello)) }).await;
    let Some(Body::Welcome(welcome)) = client.receive().await.body else { panic!("expected welcome") };
    (client, welcome)
}

/// A device paired with a code before it showed the
/// account can't approve a move or a join; its Welcome says why
/// (`cannot_approve`) and so does ProveAllowedDevice's refusal. Connected
/// again with its assertion, it can.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_device_paired_with_a_code_approves_once_it_connects_with_the_account() {
    let dir = temp_dir();
    let instance = Instance::start(&dir).await;
    let (addr, studio, _mac, _view, local) = signed_in_at(&dir, &instance).await;
    let account = account_id(&dir);
    let prove = || Request::ProveAllowedDevice(v1::ProveAllowedDevice { request_id: "r1".into() });

    let (token, made) = factoryd::remote::pair(&home(&dir), "Phone").expect("paired");
    let phone = new_device().await;
    let (mut app, answer) = knock(&local, &phone, &token, "").await;
    assert!(matches!(answer, Body::PairingPending(_)), "{answer:?}");
    factoryd::remote::approve(&home(&dir), &made.id).expect("allowed");
    let answer = app.receive().await.body.expect("an answer");
    let welcome = welcomed(&mut app, answer).await;
    let name = &welcome.name;
    assert_eq!(
        welcome.cannot_approve,
        format!(
            "This device connected to {name} with a code, not with your Claktory account. To approve here, sign in to your account on this device and connect again."
        )
    );
    let (code, reason) = refused(&app.request(prove()).await.body);
    assert_eq!((code, reason.as_str()), (ErrorCode::SignInRequired, welcome.cannot_approve.as_str()));
    drop(app);

    // Again, with its assertion: it can, for the key it showed.
    let (mut app, answer) = knock(&local, &phone, &token, &assertion(&instance, &account, &phone)).await;
    assert_eq!(welcomed(&mut app, answer).await.cannot_approve, "");
    let Body::AllowedDeviceWord(word) = app.request(prove()).await.body else { panic!("expected the word") };
    let backend_key = factoryd::account_backend_key_for_tests(&studio);
    let said = claktory_account::protocol::verify_allowed_device(&backend_key, &word.allowed, jiff::Timestamp::now().as_second())
        .expect("the studio's word");
    assert_eq!(said.account_key, account_key(&phone));
    drop(app);
    // Kept: the next connection without one can still approve.
    let (mut app, answer) = knock(&local, &phone, &token, "").await;
    assert_eq!(welcomed(&mut app, answer).await.cannot_approve, "");

    // This computer's app without an assertion is told to sign in; an
    // agent's command, that it's the person's to do.
    let (_, welcome) = welcome_on_loopback(addr, factoryd::authority::person_token_for_test(home(&dir).root())).await;
    assert_eq!(welcome.cannot_approve, "Sign in to your Claktory account on this device first.");
    let (_, welcome) = welcome_on_loopback(addr, crate::common::tool_token(addr)).await;
    assert!(welcome.cannot_approve.starts_with("Do this in the Claktory app"), "{}", welcome.cannot_approve);
}

/// Apps before 0.2.0 show their account only when they connect: a device
/// paired with a code that signs in afterwards asks for its direct messages
/// under its account key on the same connection. That connection closes,
/// once, so it connects again showing its account, and the next one has
/// them. One that comes back without its assertion keeps its connection.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_device_that_signed_in_since_it_connected_is_asked_once_to_connect_again() {
    let dir = temp_dir();
    let instance = Instance::start(&dir).await;
    let (_studio, _mac, _view, local) = signed_in(&dir, &instance).await;
    let account = account_id(&dir);
    let (token, made) = factoryd::remote::pair(&home(&dir), "Phone").expect("paired");
    let phone = new_device().await;
    let (mut app, answer) = knock(&local, &phone, &token, "").await;
    assert!(matches!(answer, Body::PairingPending(_)), "{answer:?}");
    factoryd::remote::approve(&home(&dir), &made.id).expect("allowed");
    let answer = app.receive().await.body.expect("an answer");
    welcomed(&mut app, answer).await;
    let follow = || Request::DmFollow(v1::DmFollow { leaf_id: account_key(&phone), cursors: Vec::new() });
    let not_signed_in = (ErrorCode::InvalidRequest, "This device isn't signed in to your account.".to_owned());

    // Signed in on the phone since: its follow is answered, then its
    // connection closes.
    assert_eq!(refused(&app.request(follow()).await.body), not_signed_in);
    closed(&mut app).await;

    // Back without its assertion (it couldn't get one in time): refused as
    // before, and left open.
    let (mut app, answer) = knock(&local, &phone, &token, "").await;
    welcomed(&mut app, answer).await;
    assert_eq!(refused(&app.request(follow()).await.body), not_signed_in);
    assert_eq!(refused(&app.request(follow()).await.body), not_signed_in);
    drop(app);

    // Back with it: its follow goes under its own leaf.
    let (mut app, answer) = knock(&local, &phone, &token, &assertion(&instance, &account, &phone)).await;
    welcomed(&mut app, answer).await;
    let answer = app.request(follow()).await.body;
    assert!(!matches!(&answer, Body::Error(error) if error.message == not_signed_in.1), "{answer:?}");
}

/// The Windows app's token signs this Claktory in only while it
/// has never been signed in, by what it keeps: not after a sign-in, nor
/// after the sign-out that follows.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_windows_app_signs_in_only_a_claktory_never_signed_in() {
    let dir = temp_dir();
    let instance = Instance::start(&dir).await;
    let (addr, studio) = open(&dir, &instance.url).await;
    let sign_in_asked = Request::AccountSignIn(v1::AccountSignIn { email: "matt@example.com".into() });
    let send = Request::SendMessage(v1::SendMessage::default());
    let pairing = Request::CreatePairing(v1::CreatePairing::default());
    assert!(factoryd::authority::local_may_for_tests(&studio, &sign_in_asked));
    assert!(factoryd::authority::local_may_for_tests(&studio, &send));
    assert!(!factoryd::authority::local_may_for_tests(&studio, &pairing));
    let (mut mac, _, _) = Client::connect(addr).await;
    sign_in(&mut mac, &instance, "matt@example.com").await;
    assert!(!factoryd::authority::local_may_for_tests(&studio, &sign_in_asked));
    mac.ok(Request::AccountSignOut(v1::AccountSignOut {})).await;
    assert!(!factoryd::authority::local_may_for_tests(&studio, &sign_in_asked), "signed out, but signed in before");
    assert!(factoryd::authority::local_may_for_tests(&studio, &send));
}

/// A secret, so this Claktory has a vault to seal.
fn a_secret() -> Request {
    Request::CreateSecret(v1::CreateSecret {
        name: "AWS".into(),
        entries: vec![v1::SecretEntry { variable: "AWS_SECRET_ACCESS_KEY".into(), value: "wJalrXUtnFEMI-key-test".into(), file: false }],
        ..Default::default()
    })
}

fn covered(view: &View) -> bool {
    view.workspace.vault.as_ref().is_some_and(|vault| vault.recovery_key_covered && !vault.recovery_key_change_waiting)
}

/// The account's recovery key made on a phone paired here.
/// The phone asks with its own account key, signs the statement, and this
/// Claktory sets it at claktory.com and is covered by it. Another Claktory
/// of the account where the phone isn't paired waits for the person; once
/// the phone is paired there too, it seals by itself at its next check-in,
/// with a record in History and no alert. Only the device that asked can
/// sign, and only with its own key; agents' commands can do neither.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_recovery_key_made_on_a_phone_covers_the_claktories_where_its_paired() {
    use claktory_account::protocol as api;
    let dir = temp_dir();
    let instance = Instance::start(&dir).await;
    let (addr, _studio, mut mac, mut mac_view, local) = signed_in_at(&dir, &instance).await;
    apply_request(&mut mac, &mut mac_view, a_secret()).await;
    let account = account_id(&dir);
    let phone_key = ed25519_dalek::SigningKey::from_bytes(&[12; 32]);
    let device_key = api::b64(phone_key.verifying_key().as_bytes());
    let (token, _) = factoryd::remote::pair(&home(&dir), "Phone").expect("paired");
    let tunnel = new_device().await;
    let (mut phone, answer) = knock(&local, &tunnel, &token, &assertion_for(&instance, &account, &phone_key, &tunnel)).await;
    welcomed(&mut phone, answer).await;
    // This Claktory's own key, made at sign-in, until the account's covers it.
    let Body::RecoveryKey(_) = mac.request(Request::ShowRecoveryKey(v1::ShowRecoveryKey::default())).await.body else {
        panic!("its own key")
    };
    let own_keys = |dir: &std::path::Path| -> i64 {
        rusqlite::Connection::open(dir.join("home/vault/vault.db"))
            .expect("vault.db")
            .query_row("SELECT COUNT(*) FROM protectors WHERE kind = 'recovery'", [], |row| row.get(0))
            .expect("count")
    };
    assert_eq!(own_keys(&dir), 1);
    let public = crate::claktory_account::recovery_public(&[21; 20]);
    let new_key = |device_key: &str| {
        Request::NewRecoveryKey(v1::NewRecoveryKey { public_key: public.clone(), device_key: device_key.into(), ..Default::default() })
    };

    // Agents' commands can't; nor another key than the connection's.
    let (mut tool, _) = tool_client(addr).await;
    assert_eq!(refused(&tool.request(new_key(&device_key)).await.body).0, ErrorCode::PersonOnly);
    let publish = |device_sig: String| Request::PublishRecoveryKey(v1::PublishRecoveryKey { device_key: device_key.clone(), device_sig });
    assert_eq!(refused(&tool.request(publish("x".into())).await.body).0, ErrorCode::PersonOnly);
    let other = api::b64(ed25519_dalek::SigningKey::from_bytes(&[13; 32]).verifying_key().as_bytes());
    assert_eq!(refused(&phone.request(new_key(&other)).await.body).0, ErrorCode::InvalidRequest);
    // Nothing asked yet: nothing to sign.
    assert_eq!(refused(&phone.request(publish("x".into())).await.body).0, ErrorCode::InvalidRequest);

    // The statement to sign: this account, this Claktory, the phone's key,
    // the account's first.
    let Body::RecoveryKeyToSign(to_sign) = phone.request(new_key(&device_key)).await.body else { panic!("expected the statement") };
    let said: api::RecoveryKeyStatement = serde_json::from_slice(&api::unb64(&to_sign.payload).expect("base64")).expect("json");
    assert_eq!((said.account_id.as_str(), said.public_key.as_str(), said.replaces.as_str()), (account.as_str(), public.as_str(), ""));
    assert_eq!(said.device_key, device_key);
    // Signed by another key: refused, and it can still be signed.
    let forged = api::sign_recovery_key_device(&ed25519_dalek::SigningKey::from_bytes(&[13; 32]), &format!("{}.x", to_sign.payload));
    assert_eq!(refused(&phone.request(publish(forged)).await.body).0, ErrorCode::InvalidRequest);
    let signed = api::sign_recovery_key_device(&phone_key, &format!("{}.x", to_sign.payload));
    let answered = phone.request(publish(signed)).await;
    assert!(matches!(answered.body, Body::Ack(_)), "{:?}", answered.body);
    mac_view.follow(&mut mac, covered).await;
    let vault = mac_view.workspace.vault.clone().expect("vault");
    assert!(vault.recovery_key_made_ms > 0 && !vault.recovery_key_made_on.is_empty(), "made here: {vault:?}");
    // The key saved before no longer opens it.
    assert_eq!(own_keys(&dir), 0, "its own key is gone");
    assert!(!matches!(mac.request(Request::ShowRecoveryKey(v1::ShowRecoveryKey::default())).await.body, Body::RecoveryKey(_)));

    // Another Claktory of the account, where the phone isn't paired: it waits.
    let other_dir = temp_dir();
    let (_, other, mut other_mac, mut other_view, other_local) = signed_in_at(&other_dir, &instance).await;
    apply_request(&mut other_mac, &mut other_view, a_secret()).await;
    other.check_in_now();
    other_view
        .follow(&mut other_mac, |view| view.workspace.vault.as_ref().is_some_and(|vault| vault.recovery_key_change_waiting))
        .await;
    assert!(other_view.workspace.alerts.iter().any(|alert| alert.title.starts_with("Your account has a new recovery key")));

    // Paired there too: sealed by itself at the next check-in.
    let (token, _) = factoryd::remote::pair(&home(&other_dir), "Phone").expect("paired");
    let (mut there, answer) =
        knock(&other_local, &tunnel, &token, &assertion_for(&instance, &account, &phone_key, &tunnel)).await;
    welcomed(&mut there, answer).await;
    other.check_in_now();
    other_view.follow(&mut other_mac, covered).await;
    assert_eq!(own_keys(&other_dir), 0, "its own key is gone there too");
    let changes: i64 = rusqlite::Connection::open(other_dir.join("home/factory.db"))
        .expect("factory.db")
        .query_row("SELECT COUNT(*) FROM recovery_key_changes", [], |row| row.get(0))
        .expect("count");
    assert_eq!(changes, 1, "a record in History");
}
