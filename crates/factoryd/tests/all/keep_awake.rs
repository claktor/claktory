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

/// A stand-in for the kernel's lid sleep: does as told, and keeps what it
/// was asked.
#[derive(Default)]
struct Clamshell {
    /// The bit, as set.
    bit: std::sync::atomic::AtomicBool,
    asked: std::sync::Mutex<Vec<bool>>,
    refuses: std::sync::atomic::AtomicBool,
}

impl factoryd::lid_closed::Clamshell for Clamshell {
    fn keep_awake(&self, on: bool) -> Result<(), String> {
        self.asked.lock().expect("lock").push(on);
        if self.refuses.load(std::sync::atomic::Ordering::Relaxed) {
            return Err("refused".into());
        }
        self.bit.store(on, std::sync::atomic::Ordering::Relaxed);
        Ok(())
    }
    fn sleep_disabled(&self) -> Option<bool> {
        Some(false)
    }
    fn hot(&self) -> bool {
        false
    }
}

fn lid(on: bool) -> Request {
    Request::SetLidClosed(v1::SetLidClosed { on, ..Default::default() })
}

/// #371: off by default; turned on from this Mac's app, it keeps a closed
/// lid from sleeping the Mac only while agents work, and lets go when they
/// stop or it's turned off; a bit it can't clear says the Mac can't sleep,
/// and Needs you says so.
#[tokio::test]
async fn the_lid_is_kept_from_sleeping_the_mac_only_while_agents_work_and_it_is_on() {
    use std::sync::atomic::Ordering::Relaxed;
    let dir = temp_dir();
    let (addr, factory) = start(&dir).await;
    let (mut client, workspace, _) = Client::connect(addr).await;
    let mut view = View { workspace, ..View::default() };
    let first = keep_awake(&view);
    assert_eq!(first.lid_closed(), v1::LidClosedState::LidClosedOff, "{first:?}");
    assert!(!first.lid_closed_available, "never in a test's home, as never in Claktory Dev");
    let refused = apply_request(&mut client, &mut view, lid(true)).await;
    assert!(matches!(refused, Body::Error(ref error) if error.message.starts_with("Only a laptop")), "{refused:?}");

    let clamshell = std::sync::Arc::new(Clamshell::default());
    factory.set_lid_closed_clamshell(clamshell.clone());
    assert!(matches!(apply_request(&mut client, &mut view, lid(true)).await, Body::Ack(_)));
    view.follow(&mut client, |view| keep_awake(view).lid_closed() == v1::LidClosedState::LidClosedOn).await;
    assert!(!keep_awake(&view).lid_closed_holding && clamshell.asked.lock().expect("lock").is_empty(), "no agent works");
    // A reply that never comes: Ops works until stopped.
    apply_request(&mut client, &mut view, create_agent("", "Ops", "")).await;
    apply_request(&mut client, &mut view, send_message("ops", "hang")).await;
    view.follow(&mut client, |view| keep_awake(view).lid_closed_holding).await;
    assert!(clamshell.bit.load(Relaxed));
    apply_request(&mut client, &mut view, Request::StopAgent(v1::StopAgent { agent_id: "ops".into() })).await;
    view.follow(&mut client, |view| view.agent("ops").status() == v1::AgentStatus::Idle && !keep_awake(view).lid_closed_holding).await;
    assert!(!clamshell.bit.load(Relaxed), "cleared when the work stopped");

    // A clear the kernel refuses: the Mac can't sleep, whatever the setting
    // says, with what fixes it, and Needs you notes it.
    apply_request(&mut client, &mut view, send_message("ops", "hang")).await;
    view.follow(&mut client, |view| keep_awake(view).lid_closed_holding).await;
    clamshell.refuses.store(true, Relaxed);
    apply_request(&mut client, &mut view, lid(false)).await;
    view.follow(&mut client, |view| keep_awake(view).lid_closed() == v1::LidClosedState::LidClosedStuck).await;
    // Named, as Needs you shows it on every device.
    let fix = keep_awake(&view).lid_closed_fix;
    assert!(fix.starts_with("Restart ") && fix.ends_with(" to let it sleep with the lid closed.") && !fix.contains("this Mac"), "{fix}");
    // Cleared at a later look: it can sleep again, and it's off.
    clamshell.refuses.store(false, Relaxed);
    factory.keep_awake_changed();
    view.follow(&mut client, |view| keep_awake(view).lid_closed() == v1::LidClosedState::LidClosedOff).await;
    assert!(!clamshell.bit.load(Relaxed));
    apply_request(&mut client, &mut view, Request::StopAgent(v1::StopAgent { agent_id: "ops".into() })).await;
}

/// #371 (Fugo, 23585): turning it on is taken only from a connection
/// authenticated as this Mac's own app (over its socket; the
/// tests' person token stands for it); a paired device is refused even from
/// this machine's address, and may turn it off; an agent's command, with
/// the tool token, is refused it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn only_this_macs_own_app_turns_the_lid_closed_setting_on_over_either_way_in() {
    let dir = temp_dir();
    let (addr, factory) = start(&dir).await;
    factory.set_lid_closed_clamshell(std::sync::Arc::new(Clamshell::default()));

    // This Mac's app.
    let (mut mac, workspace, _) = Client::connect(addr).await;
    let mut view = View { workspace, ..View::default() };
    assert!(keep_awake(&view).lid_closed_available, "the helper makes it available");

    // A paired device: refused on, even though it connects from this machine.
    let (mut phone, _) = crate::remote::another_device(addr, "Phone").await;
    match phone.request(lid(true)).await.body {
        Body::Error(error) => assert_eq!(error.message, "Turn it on from the Mac."),
        other => panic!("expected the refusal, got {other:?}"),
    }

    // The Mac app: on.
    assert!(matches!(apply_request(&mut mac, &mut view, lid(true)).await, Body::Ack(_)));
    view.follow(&mut mac, |view| keep_awake(view).lid_closed() != v1::LidClosedState::LidClosedOff).await;
    // The paired device may turn it off.
    phone.ok(lid(false)).await;
    view.follow(&mut mac, |view| keep_awake(view).lid_closed() == v1::LidClosedState::LidClosedOff).await;
    // An agent's command, on the loopback port with the tool token: never.
    let mut script = Client::open(addr).await;
    let hello = v1::Hello {
        protocol_version: factory_protocol::PROTOCOL_VERSION,
        client_version: "test".into(),
        token: crate::common::tool_token(addr),
        ..Default::default()
    };
    script.send(v1::ClientMessage { id: 1, body: Some(Request::Hello(hello)) }).await;
    script.welcomed().await;
    match script.request(lid(true)).await.body {
        Body::Error(error) => assert_eq!(error.code(), v1::ErrorCode::PersonOnly),
        other => panic!("expected the refusal, got {other:?}"),
    }
}
