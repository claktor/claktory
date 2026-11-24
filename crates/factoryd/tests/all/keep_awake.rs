//! Keeping the Mac awake while agents work (P1, O1): on by default on a
//! desktop, asked on a laptop, the person's answer kept across restarts.

use crate::common::{Client, View, apply_request, create_agent, send_message, start, temp_dir};
use factory_protocol::v1::{self, client_message::Body as Request, server_message::Body};

fn keep_awake(view: &View) -> v1::KeepAwake {
    view.workspace.keep_awake.clone().expect("keep_awake")
}

#[tokio::test]
async fn keeping_the_mac_awake_is_on_for_a_desktop_asked_for_a_laptop_and_the_answer_is_kept() {
    let dir = temp_dir();
    let (addr, factory) = start(&dir).await;
    let (mut client, workspace, _) = Client::connect(addr).await;
    let mut view = View { workspace, ..View::default() };
    let first = keep_awake(&view);
    assert_eq!(first.available, cfg!(target_os = "macos"));
    // A desktop is on by default; a laptop waits for the person's yes.
    assert_eq!(first.on, first.available && !first.laptop, "{first:?}");
    assert!(!first.answered && !first.holding && !first.ask, "{first:?}");

    let yes = apply_request(&mut client, &mut view, Request::SetKeepAwake(v1::SetKeepAwake { on: true })).await;
    if !cfg!(target_os = "macos") {
        assert!(matches!(yes, Body::Error(ref error) if error.message == "Claktory can keep only a Mac awake."), "{yes:?}");
        return;
    }
    assert!(matches!(yes, Body::Ack(_)), "{yes:?}");
    assert!(keep_awake(&view).on && keep_awake(&view).answered && !keep_awake(&view).ask);

    // Rivet answers, then works no more: nothing holds the Mac.
    apply_request(&mut client, &mut view, send_message("rivet", "hello")).await;
    view.follow(&mut client, |view| view.agent("rivet").status() == v1::AgentStatus::Idle && !keep_awake(view).holding).await;

    drop(client);
    drop(factory);
    let (addr, _factory) = start(&dir).await;
    let (_client, workspace, _) = Client::connect(addr).await;
    let kept = workspace.keep_awake.expect("keep_awake");
    assert!(kept.on && kept.answered, "{kept:?}");
}

/// A stand-in for the lid-closed helper: answers as told, and keeps what
/// it was asked.
#[derive(Default)]
struct Helper {
    answering: std::sync::atomic::AtomicBool,
    asked: std::sync::Mutex<Vec<factoryd::lid_closed::Op>>,
}

impl factoryd::lid_closed::Helper for Helper {
    fn call(&self, op: factoryd::lid_closed::Op) -> Option<factoryd::lid_closed::Answer> {
        self.asked.lock().expect("lock").push(op);
        self.answering.load(std::sync::atomic::Ordering::Relaxed).then_some(factoryd::lid_closed::Answer {
            contract: 1,
            version: 1,
            holding: op == factoryd::lid_closed::Op::Hold,
            ..Default::default()
        })
    }
}

fn lid(on: bool, registration: v1::LidClosedRegistration) -> Request {
    Request::SetLidClosed(v1::SetLidClosed { on, registration: registration.into(), error: String::new() })
}

/// #371: off by default; turned on from this Mac's app, it holds the Mac
/// awake through the helper only while agents work, and lets go when they
/// stop or it's turned off.
#[tokio::test]
async fn the_lid_closed_helper_holds_only_while_agents_work_and_it_is_on() {
    let dir = temp_dir();
    let (addr, factory) = start(&dir).await;
    let (mut client, workspace, _) = Client::connect(addr).await;
    let mut view = View { workspace, ..View::default() };
    let first = keep_awake(&view);
    assert_eq!(first.lid_closed(), v1::LidClosedState::LidClosedOff, "{first:?}");
    if !first.lid_closed_available {
        let refused = apply_request(&mut client, &mut view, lid(true, v1::LidClosedRegistration::Enabled)).await;
        assert!(matches!(refused, Body::Error(ref error) if error.message.starts_with("Only a laptop")), "{refused:?}");
    }

    let helper = std::sync::Arc::new(Helper::default());
    factory.set_lid_closed_helper(helper.clone());
    // Waiting for approval: nothing answers yet.
    apply_request(&mut client, &mut view, lid(true, v1::LidClosedRegistration::RequiresApproval)).await;
    view.follow(&mut client, |view| keep_awake(view).lid_closed() == v1::LidClosedState::LidClosedNeedsApproval).await;
    // Approved: it answers, and holds only while Rivet works.
    helper.answering.store(true, std::sync::atomic::Ordering::Relaxed);
    apply_request(&mut client, &mut view, lid(true, v1::LidClosedRegistration::Enabled)).await;
    view.follow(&mut client, |view| keep_awake(view).lid_closed() == v1::LidClosedState::LidClosedOn).await;
    assert!(!keep_awake(&view).lid_closed_holding, "no agent works");
    // A reply that never comes: Ops works until stopped.
    apply_request(&mut client, &mut view, create_agent("", "Ops", "")).await;
    apply_request(&mut client, &mut view, send_message("ops", "hang")).await;
    view.follow(&mut client, |view| keep_awake(view).lid_closed_holding).await;
    apply_request(&mut client, &mut view, Request::StopAgent(v1::StopAgent { agent_id: "ops".into() })).await;
    view.follow(&mut client, |view| view.agent("ops").status() == v1::AgentStatus::Idle && !keep_awake(view).lid_closed_holding).await;
    let asked = helper.asked.lock().expect("lock").clone();
    use factoryd::lid_closed::Op;
    assert!(asked.contains(&Op::Hold) && asked.last() == Some(&Op::Release), "held while Ops worked, then let go: {asked:?}");

    // Off, from anywhere: off it is, and nothing more is held.
    apply_request(&mut client, &mut view, lid(false, v1::LidClosedRegistration::NotRegistered)).await;
    view.follow(&mut client, |view| keep_awake(view).lid_closed() == v1::LidClosedState::LidClosedOff).await;
    let before = helper.asked.lock().expect("lock").len();
    apply_request(&mut client, &mut view, send_message("ops", "again")).await;
    view.follow(&mut client, |view| view.agent("ops").status() == v1::AgentStatus::Idle && view.agent("ops").queued_messages == 0).await;
    assert!(!helper.asked.lock().expect("lock")[before..].contains(&Op::Hold), "off holds nothing");
}

/// #371 (Fugo, 23585): turning it on is taken only from a connection
/// authenticated as this Mac's own app, over the loopback port or through
/// this Mac's own tunnel endpoint as the Mac app connects; a paired device
/// is refused even from this machine's address, and may turn it off.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn only_this_macs_own_app_turns_the_lid_closed_setting_on_over_either_way_in() {
    let dir = temp_dir();
    let (addr, factory) = start(&dir).await;
    factory.set_lid_closed_helper(std::sync::Arc::new(Helper::default()));

    // This Mac's app through the tunnel, with the local pairing code.
    let local = crate::remote::parse(&crate::remote::local_pairing(&dir).await);
    let app = crate::remote::new_device().await;
    let connection = crate::remote::dial(&app, &local, &local.addresses).await.expect("let in");
    let (send, receive) = connection.open_bi().await.expect("stream");
    let mut mac = Client::over(Box::new(tokio::io::join(receive, send)), Some(Box::new((app, connection)))).await;
    let hello = v1::Hello {
        protocol_version: factory_protocol::PROTOCOL_VERSION,
        client_version: "test".into(),
        token: local.token.clone(),
        ..Default::default()
    };
    mac.send(v1::ClientMessage { id: 1, body: Some(Request::Hello(hello)) }).await;
    let (workspace, _) = mac.welcomed().await;
    let mut view = View { workspace, ..View::default() };
    assert!(keep_awake(&view).lid_closed_available, "the helper makes it available");

    // A paired device: refused on, even though it connects from this machine.
    let (mut phone, _) = crate::remote::another_device(addr, "Phone").await;
    match phone.request(lid(true, v1::LidClosedRegistration::Enabled)).await.body {
        Body::Error(error) => assert_eq!(error.message, "Turn it on from the Mac."),
        other => panic!("expected the refusal, got {other:?}"),
    }

    // The Mac app over the tunnel: on.
    assert!(matches!(apply_request(&mut mac, &mut view, lid(true, v1::LidClosedRegistration::Enabled)).await, Body::Ack(_)));
    view.follow(&mut mac, |view| keep_awake(view).lid_closed() != v1::LidClosedState::LidClosedOff).await;
    // The paired device may turn it off.
    phone.ok(lid(false, v1::LidClosedRegistration::Unknown)).await;
    view.follow(&mut mac, |view| keep_awake(view).lid_closed() == v1::LidClosedState::LidClosedOff).await;
    // And the loopback port's client, this Mac's too, turns it on again.
    let (mut script, workspace, _) = Client::connect(addr).await;
    let mut seen = View { workspace, ..View::default() };
    assert!(matches!(apply_request(&mut script, &mut seen, lid(true, v1::LidClosedRegistration::Enabled)).await, Body::Ack(_)));
}
