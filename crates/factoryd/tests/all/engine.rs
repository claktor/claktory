//! Stopping Claktory from its Mac app, and what happens when the app quits
//! (friend bug #27): the setting is kept, and StopEngine is taken only from
//! this Mac's own app, by its connection's token, never from a paired
//! device, even one connecting from this machine.

use crate::common::{Client, View, apply_request, start, temp_dir};
use factory_protocol::v1::{self, client_message::Body as Request, server_message::Body};

fn settings(view: &View) -> v1::Settings {
    view.workspace.settings.clone().expect("settings")
}

fn stop(quitting: bool) -> Request {
    Request::StopEngine(v1::StopEngine { quitting })
}

fn when_app_quits(mode: v1::QuitMode) -> Request {
    Request::UpdateSettings(v1::UpdateSettings { when_app_quits: Some(mode.into()), ..Default::default() })
}

fn refusal(body: Body) -> String {
    match body {
        Body::Error(error) => error.message,
        other => panic!("expected a refusal, got {other:?}"),
    }
}

#[tokio::test]
async fn what_happens_when_the_app_quits_keeps_running_until_the_person_chooses_and_is_kept() {
    let dir = temp_dir();
    let (addr, factory) = start(&dir).await;
    let (mut client, workspace, _) = Client::connect(addr).await;
    let mut view = View { workspace, ..View::default() };
    // Every Claktory keeps running until the person says otherwise, and
    // one its Mac's background service doesn't run isn't asked.
    assert_eq!(settings(&view).when_app_quits(), v1::QuitMode::KeepRunning);
    assert_eq!(settings(&view).onboarding.expect("onboarding").when_app_quits(), v1::OnboardingStep::Done);
    assert!(!view.workspace.backend_service.clone().unwrap_or_default().can_stop, "not run by this Mac's service");

    assert!(matches!(apply_request(&mut client, &mut view, when_app_quits(v1::QuitMode::Stop)).await, Body::Ack(_)));
    view.follow(&mut client, |view| settings(view).when_app_quits() == v1::QuitMode::Stop).await;
    let unspecified = apply_request(&mut client, &mut view, when_app_quits(v1::QuitMode::Unspecified)).await;
    assert_eq!(refusal(unspecified), "when_app_quits must be KEEP_RUNNING or STOP");

    // Kept in factory.db across a restart.
    drop(client);
    drop(factory);
    let (addr, _factory) = start(&dir).await;
    let (_client, workspace, _) = Client::connect(addr).await;
    assert_eq!(workspace.settings.expect("settings").when_app_quits(), v1::QuitMode::Stop);
}

/// Fugo's requirement: "local" is the connection's token, never its
/// address. A paired device is refused even from this machine; this Mac's
/// app quitting in keep mode changes nothing; and a Claktory its Mac's
/// background service doesn't run can't be stopped at all.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn only_this_macs_own_app_can_stop_claktory() {
    let dir = temp_dir();
    let (addr, _factory) = start(&dir).await;
    let (mut mac, workspace, _) = Client::connect(addr).await;
    let mut view = View { workspace, ..View::default() };

    let (mut phone, _) = crate::remote::another_device(addr, "Phone").await;
    for quitting in [false, true] {
        assert_eq!(refusal(phone.request(stop(quitting)).await.body), "Stop Claktory from the Mac it runs on.");
    }

    // Keep mode: the app quitting leaves Claktory running.
    assert!(matches!(apply_request(&mut mac, &mut view, stop(true)).await, Body::Ack(_)));
    // The person's switch, here where no background service runs it.
    assert_eq!(
        refusal(apply_request(&mut mac, &mut view, stop(false)).await),
        "Only Claktory on a Mac, run by its background service, can be stopped from its app."
    );
    // Stop mode, likewise.
    apply_request(&mut mac, &mut view, when_app_quits(v1::QuitMode::Stop)).await;
    assert!(refusal(apply_request(&mut mac, &mut view, stop(true)).await).starts_with("Only Claktory on a Mac"));
    assert!(!view.workspace.backend_service.clone().unwrap_or_default().stopping);
    // Still running.
    mac.ok(when_app_quits(v1::QuitMode::KeepRunning)).await;
}

/// A client on the loopback port without this Mac's token is turned away
/// at Hello, and the connection ends, so StopEngine never gets in.
#[tokio::test]
async fn a_client_without_the_token_cannot_ask_to_stop() {
    let dir = temp_dir();
    let (addr, _factory) = start(&dir).await;
    for token in ["", "not the token"] {
        let mut client = Client::open(addr).await;
        let hello = v1::Hello {
            protocol_version: factory_protocol::PROTOCOL_VERSION,
            client_version: "test".into(),
            token: token.into(),
            ..Default::default()
        };
        client.send(v1::ClientMessage { id: 1, body: Some(Request::Hello(hello)) }).await;
        match client.receive().await.body {
            Some(Body::Error(error)) => assert_eq!(error.code(), v1::ErrorCode::Unauthorized),
            other => panic!("expected the refusal, got {other:?}"),
        }
        assert!(client.closed().await, "the connection ends: nothing more gets in");
    }
}
