//! A device on another machine (the iPhone, over the tunnel) gets the
//! Workspace at most once a second:
//! the first change at once, a burst after it as one patch a second later,
//! and the answer to its own request after what the request changed, at
//! once.
//!
//! On several threads, as remote.rs: the tunnel keeps exchanging packets.

use std::net::SocketAddr;
use std::time::{Duration, Instant};

use crate::common::{Client, View, apply_patch, apply_request, create_agent, start, temp_dir};
use crate::remote::{dial, local_pairing, new_device, parse};
use factory_protocol::PROTOCOL_VERSION;
use factory_protocol::v1::{self, client_message, server_message::Body};

/// A phone of the person's, paired and allowed, applying patches, through
/// the private tunnel.
async fn phone(addr: SocketAddr) -> (Client, v1::Workspace) {
    let home = crate::common::home_of(addr);
    let local = parse(&local_pairing(home.root().parent().expect("the test's folder")).await);
    let (mut setup, _, _) = Client::connect(addr).await;
    let relays = v1::SetRemoteAccess { relays: vec!["http://127.0.0.1:9/".into()], set_relays: true, ..Default::default() };
    setup.ok(client_message::Body::SetRemoteAccess(relays)).await;
    let (token, _) = factoryd::remote::pair(&home, "Phone").expect("paired");
    let device = new_device().await;
    let connection = dial(&device, &local, &local.addresses).await.expect("let in");
    let (send, receive) = connection.open_bi().await.expect("stream");
    let mut client = Client::over(Box::new(tokio::io::join(receive, send)), Some(Box::new((device, connection)))).await;
    let hello = v1::Hello {
        protocol_version: PROTOCOL_VERSION,
        client_version: "test".into(),
        token,
        workspace_patches: true,
        device_kind: v1::DeviceKind::Phone.into(),
        ..Default::default()
    };
    client.send(v1::ClientMessage { id: 1, body: Some(client_message::Body::Hello(hello)) }).await;
    assert!(matches!(client.receive().await.body, Some(Body::PairingPending(_))));
    factoryd::remote::approve(&home, "Phone").expect("allowed");
    let (workspace, _) = client.welcomed().await;
    (client, workspace)
}

/// Applies a Workspace or a patch to what the phone holds; whether it was one.
fn applied(held: &mut v1::Workspace, body: Option<&Body>) -> bool {
    match body {
        Some(Body::Workspace(workspace)) => *held = workspace.clone(),
        Some(Body::WorkspacePatch(patch)) => *held = apply_patch(held, patch),
        _ => return false,
    }
    true
}

fn prompt(workspace: &v1::Workspace, id: &str) -> String {
    workspace.agents.iter().find(|agent| agent.id == id).map(|agent| agent.prompt.clone()).unwrap_or_default()
}

fn set_prompt(prompt: &str) -> client_message::Body {
    client_message::Body::UpdateAgent(v1::UpdateAgent { agent_id: "ops".into(), prompt: Some(prompt.into()), ..Default::default() })
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_device_on_another_machine_gets_a_burst_as_one_patch_and_its_own_answers_at_once() {
    let dir = temp_dir();
    let (addr, _factory) = start(&dir).await;
    let (mut mac, workspace, _) = Client::connect(addr).await;
    let mut view = View { workspace, ..View::default() };
    apply_request(&mut mac, &mut view, create_agent("", "Ops", "")).await;
    let (mut phone, mut held) = phone(addr).await;

    // Six changes from the Mac, as fast as they go: this machine's app gets
    // each at once.
    let started = Instant::now();
    for n in 0..6 {
        apply_request(&mut mac, &mut view, set_prompt(&format!("v{n}"))).await;
        assert_eq!(view.agent("ops").prompt, format!("v{n}"));
    }
    let burst = started.elapsed();
    // The phone: a second apart at least, and the last state in the end.
    let mut arrived = Vec::new();
    while prompt(&held, "ops") != "v5" {
        let message = phone.receive().await;
        if applied(&mut held, message.body.as_ref()) {
            arrived.push(Instant::now());
        }
    }
    let most = 2 + burst.as_secs() as usize;
    assert!(arrived.len() <= most, "{} Workspaces for 6 changes in {burst:?}", arrived.len());
    for pair in arrived.windows(2) {
        assert!(pair[1] - pair[0] >= Duration::from_millis(800), "{:?} apart", pair[1] - pair[0]);
    }

    // Its own request: answered after what it changed, at once, even with a
    // change of the Mac's held.
    apply_request(&mut mac, &mut view, set_prompt("from the Mac")).await;
    apply_request(&mut mac, &mut view, set_prompt("from the Mac again")).await;
    let reply = phone.request(set_prompt("from the phone")).await;
    assert!(matches!(reply.body, Body::Ack(_)), "{:?}", reply.body);
    for event in &reply.events {
        applied(&mut held, event.body.as_ref());
    }
    assert_eq!(prompt(&held, "ops"), "from the phone");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn what_a_model_writes_reaches_a_device_on_another_machine_whole_and_in_order() {
    let dir = temp_dir();
    let (addr, _factory) = start(&dir).await;
    let (mut mac, workspace, _) = Client::connect(addr).await;
    let mut view = View { workspace, ..View::default() };
    apply_request(&mut mac, &mut view, create_agent("", "Ops", "")).await;
    let (mut phone, _) = phone(addr).await;
    phone.ok(client_message::Body::SelectAgent(v1::SelectAgent { agent_id: "ops".into() })).await;
    phone.ok(crate::common::send_message("ops", "hello there")).await;
    // Its pieces, held a while at a time, all come, and before what
    // follows them (here, the notification of the reply).
    let mut written: std::collections::HashMap<u64, String> = std::collections::HashMap::new();
    loop {
        match phone.receive().await.body {
            Some(Body::Delta(delta)) => written.entry(delta.seq).or_default().push_str(&delta.text),
            Some(Body::Notify(notify)) if notify.agent_id == "ops" => {
                assert_eq!(written.get(&notify.seq).map(String::as_str), Some("echo: hello there"), "{written:?}");
                assert_eq!(notify.body, "echo: hello there");
                break;
            }
            _ => {}
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_message_of_the_persons_comes_with_what_it_did_to_its_agent() {
    let dir = temp_dir();
    let (addr, _factory) = start(&dir).await;
    let (mut mac, workspace, _) = Client::connect(addr).await;
    let mut view = View { workspace, ..View::default() };
    apply_request(&mut mac, &mut view, create_agent("", "Ops", "")).await;
    let (mut phone, mut held) = phone(addr).await;
    phone.ok(client_message::Body::SelectAgent(v1::SelectAgent { agent_id: "ops".into() })).await;
    let reply = phone.request(crate::common::send_message("ops", "hello there")).await;
    for event in &reply.events {
        applied(&mut held, event.body.as_ref());
    }
    // The agent took it: when the message shows in its conversation, the
    // phone holds no Workspace that still has it waiting, where the run's
    // start was held a second behind the answer's.
    let started = Instant::now();
    loop {
        let message = phone.receive().await;
        if applied(&mut held, message.body.as_ref()) {
            continue;
        }
        if let Some(Body::Entry(v1::EntryUpdate { entry: Some(entry), .. })) = &message.body
            && matches!(entry.kind, Some(v1::entry::Kind::User(_)))
        {
            break;
        }
    }
    let ops = held.agents.iter().find(|agent| agent.id == "ops").expect("ops");
    assert_eq!(ops.queued_messages, 0, "still waiting {:?} after the answer", started.elapsed());
}
