//! The Windows app's token (windows-constraints.md §9 and §14, position B):
//! a Hello with it is the Windows app, `Authority::LocalUncontained`. It
//! uses Claktory but never the boundary set, which is refused with
//! PERSON_ONLY in Windows words, whatever its Hello says. Test builds hold
//! a token on any system (`set_app_token_for_test`).

use factory_protocol::PROTOCOL_VERSION;
use factory_protocol::v1::{self, ErrorCode, client_message::Body as Request, server_message::Body};

use crate::claktory_account::{Instance, open, sign_in};
use crate::common::{Client, temp_dir};

const TOKEN: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
const NOT_ON_WINDOWS: &str = "Claktory on Windows can't do this. Do it on your phone, tablet or Mac.";

/// Says Hello on the loopback port with `hello`; its answer (Welcome, or
/// the refusal), and the client past its Workspace once welcomed.
async fn hello(addr: std::net::SocketAddr, hello: v1::Hello) -> (Client, Body) {
    let mut client = Client::open(addr).await;
    let hello = v1::Hello { protocol_version: PROTOCOL_VERSION, client_version: "windows test".into(), ..hello };
    client.send(v1::ClientMessage { id: 1, body: Some(Request::Hello(hello)) }).await;
    let answer = client.receive().await.body.expect("an answer");
    if matches!(answer, Body::Welcome(_)) {
        let Some(Body::Workspace(_)) = client.receive().await.body else { panic!("expected the workspace") };
    }
    (client, answer)
}

fn refused(answer: &Body) -> (ErrorCode, String) {
    match answer {
        Body::Error(error) => (error.code(), error.message.clone()),
        other => panic!("expected an error, got {other:?}"),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_windows_apps_token_uses_claktory_but_never_the_boundary_set() {
    let dir = temp_dir();
    let instance = Instance::start(&dir).await;
    let (addr, factory) = open(&dir, &instance.url).await;

    // No token held yet: nothing is the Windows app.
    let (_, answer) = hello(addr, v1::Hello { token: TOKEN.into(), ..Default::default() }).await;
    assert_eq!(refused(&answer).0, ErrorCode::Unauthorized);

    factory.set_app_token_for_test(TOKEN);
    // Another token of the same length, or a piece of it, is still turned
    // away.
    let wrong = TOKEN.replace('0', "f");
    for token in [wrong.as_str(), &TOKEN[..63]] {
        let (_, answer) = hello(addr, v1::Hello { token: token.into(), ..Default::default() }).await;
        assert_eq!(refused(&answer).0, ErrorCode::Unauthorized);
    }

    // Its Hello, even one naming a Mac and carrying an assertion, is this
    // computer's app held to the Windows class.
    let windows = v1::Hello {
        token: TOKEN.into(),
        assertion: "not.an.assertion".into(),
        device_kind: v1::DeviceKind::Laptop.into(),
        platform: "macOS 26.1".into(),
        can_confirm: true,
        ..Default::default()
    };
    let (mut app, answer) = hello(addr, windows.clone()).await;
    let Body::Welcome(welcome) = answer else { panic!("expected welcome, got {answer:?}") };
    assert_eq!((welcome.device_id.as_str(), welcome.cannot_approve.as_str()), (factoryd::remote::LOCAL_DEVICE, NOT_ON_WINDOWS));

    // Ordinary work goes through.
    let project = dir.join("work");
    std::fs::create_dir_all(&project).expect("a folder");
    app.ok(crate::common::create_project("Work", &project)).await;
    // So does signing this Claktory in, while it has never been.
    app.ok(Request::AccountSignIn(v1::AccountSignIn { email: "matt@example.com".into() })).await;
    app.ok(Request::AccountSignInCancel(v1::AccountSignInCancel {})).await;

    // The boundary set doesn't, in Windows words.
    let boundary = [
        Request::CreatePairing(v1::CreatePairing::default()),
        Request::ApproveDevice(v1::ApproveDevice::default()),
        Request::ShowRecoveryKey(v1::ShowRecoveryKey::default()),
        Request::VouchDevice(v1::VouchDevice::default()),
        Request::ProveAllowedDevice(v1::ProveAllowedDevice { request_id: "r1".into() }),
        Request::StartMove(v1::StartMove::default()),
        Request::SetAgentContained(v1::SetAgentContained::default()),
    ];
    for body in boundary {
        let answer = app.request(body.clone()).await;
        assert_eq!(refused(&answer.body), (ErrorCode::PersonOnly, NOT_ON_WINDOWS.to_owned()), "{body:?}");
    }
    // Nor the no-code setup an agent could start with its token: this
    // computer's app has connected.
    let answer = app.request(Request::AccountAuthorize(v1::AccountAuthorize { email: "outsider@example.com".into() })).await;
    assert_eq!(refused(&answer.body).0, ErrorCode::PersonOnly);
    // Nothing it showed was kept as this computer's account key.
    let devices = std::fs::read_to_string(dir.join("home/remote/devices.json")).expect("devices.json");
    let devices: Vec<serde_json::Value> = serde_json::from_str(&devices).expect("json");
    let local = devices.iter().find(|device| device["local"] == true).expect("this computer's app");
    assert!(local.get("account_key").is_none_or(|key| key == ""), "{local}");

    // Once the person signed in elsewhere, signing in is theirs.
    let (mut mac, _, _) = Client::connect(addr).await;
    sign_in(&mut mac, &instance, "matt@example.com").await;
    let (mut app, _) = hello(addr, windows).await;
    let answer = app.request(Request::AccountSignIn(v1::AccountSignIn { email: "someone@example.com".into() })).await;
    assert_eq!(refused(&answer.body), (ErrorCode::PersonOnly, NOT_ON_WINDOWS.to_owned()));
    let answer = app.request(Request::AccountSignOut(v1::AccountSignOut {})).await;
    assert_eq!(refused(&answer.body).0, ErrorCode::PersonOnly);
    // The person's own app still may.
    let answer = mac.request(Request::ShowRecoveryKey(v1::ShowRecoveryKey::default())).await;
    assert!(!matches!(&answer.body, Body::Error(error) if error.code() == ErrorCode::PersonOnly), "{:?}", answer.body);
}
