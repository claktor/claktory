//! The Devices extension: an admin agent adds the person's devices
//! (devices_add), lists them and removes them. The pairing shows as a card
//! in its conversation, whose code only the person's app gets
//! (ShowPairingCode); the device still waits for the person to allow it.

use std::time::Duration;

use crate::common::{Client, MODEL, View, apply_request, home, send_message, start, temp_dir};
use crate::remote::{dial, hello, new_device, next, open, parse};
use factory_protocol::v1::{self, AgentStatus, DevicePairingState, ErrorCode, client_message, entry::Kind, server_message::Body};

fn idle(view: &View, agent: &str) -> bool {
    let agent = view.agent(agent);
    agent.status() == AgentStatus::Idle && agent.queued_messages == 0
}

/// A Factory whose Rivet runs on the scripted model, selected, with a relay
/// that leads nowhere (no internet in tests).
async fn setup(dir: &std::path::Path) -> (std::sync::Arc<factoryd::Factory>, Client, View) {
    let (addr, factory) = start(dir).await;
    factory.set_tunnel_online_wait(Duration::from_millis(100));
    let (mut client, workspace, _) = Client::connect(addr).await;
    let mut view = View { workspace, ..View::default() };
    let relays = v1::SetRemoteAccess { relays: vec!["http://127.0.0.1:9/".into()], set_relays: true, ..Default::default() };
    apply_request(&mut client, &mut view, client_message::Body::SetRemoteAccess(relays)).await;
    let rivet = v1::UpdateAgent { agent_id: "rivet".into(), model: Some(MODEL.into()), effort: Some("off".into()), ..Default::default() };
    apply_request(&mut client, &mut view, client_message::Body::UpdateAgent(rivet)).await;
    apply_request(&mut client, &mut view, client_message::Body::SelectAgent(v1::SelectAgent { agent_id: "rivet".into() })).await;
    (factory, client, view)
}

/// Rivet calls `tool` with `args`; returns what it said.
async fn call(client: &mut Client, view: &mut View, tool: &str, args: serde_json::Value) -> String {
    let before = view.entries.len();
    apply_request(client, view, send_message("rivet", &format!("call {tool} {args}"))).await;
    view.follow(client, |view| view.entries.len() >= before + 3 && idle(view, "rivet")).await;
    view.entries[before..]
        .iter()
        .find_map(|entry| match &entry.kind {
            Some(Kind::ToolCall(call)) if call.name == tool => Some(call.output.clone()),
            _ => None,
        })
        .expect("the call")
}

/// The card for the device called `name`, as the conversation shows it now.
fn card(view: &View, name: &str) -> Option<(u64, v1::DevicePairing)> {
    view.entries.iter().rev().find_map(|entry| match &entry.kind {
        Some(Kind::DevicePairing(pairing)) if pairing.name == name => Some((entry.seq, pairing.clone())),
        _ => None,
    })
}

async fn follow_card(client: &mut Client, view: &mut View, name: &str, state: DevicePairingState) -> v1::DevicePairing {
    view.follow(client, |view| card(view, name).is_some_and(|(_, pairing)| pairing.state() == state)).await;
    card(view, name).expect("the card").1
}

fn show(pairing: &v1::DevicePairing) -> client_message::Body {
    client_message::Body::ShowPairingCode(v1::ShowPairingCode { pairing_id: pairing.id.clone() })
}

/// Everything under the Factory's home, as text where it is text.
fn everything_written(dir: &std::path::Path) -> String {
    fn walk(path: &std::path::Path, out: &mut String) {
        if path.is_dir() {
            for entry in std::fs::read_dir(path).into_iter().flatten().flatten() {
                walk(&entry.path(), out);
            }
        } else if let Ok(bytes) = std::fs::read(path) {
            out.push_str(&String::from_utf8_lossy(&bytes));
        }
    }
    let mut out = String::new();
    walk(&dir.join("home"), &mut out);
    out
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_admin_agent_adds_a_device_the_person_allows_and_the_code_never_reaches_it() {
    let dir = temp_dir();
    let (_factory, mut client, mut view) = setup(&dir).await;

    let said = call(&mut client, &mut view, "devices_add", serde_json::json!({"name": "Laptop"})).await;
    assert!(said.starts_with("Started adding Laptop. The person sees its code in a request"), "{said}");
    let (seq, pairing) = card(&view, "Laptop").expect("the request follows the call");
    assert_eq!((pairing.state(), pairing.agent_id.as_str(), pairing.device_id.as_str()), (DevicePairingState::Waiting, "rivet", "laptop"));
    assert!(pairing.expires_ms > 0 && pairing.ended_ms == 0);
    assert!(view.agent("rivet").waiting_seqs.contains(&seq), "it waits for the person");

    // The person's app gets the code, the same each time.
    let Body::PairingCode(made) = apply_request(&mut client, &mut view, show(&pairing)).await else { panic!("a code") };
    assert_eq!(made.device_id, "laptop");
    let Body::PairingCode(again) = apply_request(&mut client, &mut view, show(&pairing)).await else { panic!("a code") };
    assert_eq!((again.code.as_str(), again.link.as_str()), (made.code.as_str(), made.link.as_str()));
    assert_eq!(made.link, factoryd::remote::pairing_link(&made.code), "the same code, as a link for its QR code");
    let code = parse(&made.code);
    // Neither the agent nor anything on disk ever has it.
    let shown: Vec<String> = crate::common::describe(&view.entries);
    assert!(!shown.iter().any(|entry| entry.contains(&code.token)), "{shown:?}");
    assert!(!everything_written(&dir).contains(&code.token), "the token is only in memory");

    // A device uses it: the card says it waits for the person to allow it.
    let device = new_device().await;
    let connection = dial(&device, &code, &code.addresses).await.expect("let in to wait");
    let mut socket = open(&connection).await;
    assert!(matches!(hello(&mut socket, &code.token).await, Body::PairingPending(_)));
    follow_card(&mut client, &mut view, "Laptop", DevicePairingState::Asking).await;
    let Body::Error(error) = apply_request(&mut client, &mut view, show(&pairing)).await else { panic!("no code once used") };
    assert_eq!(error.code(), ErrorCode::NotFound);

    // Allowed, exactly as before: it's added, and Rivet is told.
    let number = crate::remote::number(&code, &device.id().to_string());
    let allow = client_message::Body::ApproveDevice(v1::ApproveDevice { device_id: "laptop".into(), number, ..Default::default() });
    assert!(matches!(apply_request(&mut client, &mut view, allow).await, Body::Ack(_)));
    assert!(matches!(next(&mut socket).await, Some(Body::Welcome(_))));
    let added = follow_card(&mut client, &mut view, "Laptop", DevicePairingState::Added).await;
    assert!(added.ended_ms > 0);
    view.follow(&mut client, |view| {
        view.entries.iter().any(|entry| matches!(&entry.kind, Some(Kind::Event(event)) if event.summary.starts_with("Laptop was added")))
            && idle(view, "rivet")
    })
    .await;
    assert!(!view.agent("rivet").waiting_seqs.contains(&seq), "nothing waits any more");

    let listed = call(&mut client, &mut view, "devices_list", serde_json::json!({})).await;
    assert!(listed.contains("- Laptop (id laptop): connected now; added"), "{listed}");
    let removed = call(&mut client, &mut view, "devices_remove", serde_json::json!({"device": "Laptop"})).await;
    assert_eq!(removed, "Removed Laptop: it can't connect to Claktory any more.");
    assert!(factoryd::remote::devices(&home(&dir)).expect("devices").iter().all(|device| device.id != "laptop"));
    assert_eq!(card(&view, "Laptop").expect("the card").1.state(), DevicePairingState::Added, "the request keeps how it went");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_card_shows_how_adding_a_device_ended_and_the_agent_is_told() {
    let dir = temp_dir();
    let (_factory, mut client, mut view) = setup(&dir).await;

    // Turned away by the person.
    call(&mut client, &mut view, "devices_add", serde_json::json!({"name": "Tablet"})).await;
    let tablet = card(&view, "Tablet").expect("card").1;
    let Body::PairingCode(made) = apply_request(&mut client, &mut view, show(&tablet)).await else { panic!("a code") };
    let code = parse(&made.code);
    let device = new_device().await;
    let connection = dial(&device, &code, &code.addresses).await.expect("let in to wait");
    let mut socket = open(&connection).await;
    assert!(matches!(hello(&mut socket, &code.token).await, Body::PairingPending(_)));
    follow_card(&mut client, &mut view, "Tablet", DevicePairingState::Asking).await;
    let deny = client_message::Body::DenyDevice(v1::DenyDevice { device_id: tablet.device_id.clone() });
    assert!(matches!(apply_request(&mut client, &mut view, deny).await, Body::Ack(_)));
    follow_card(&mut client, &mut view, "Tablet", DevicePairingState::Denied).await;

    // Cancelled from the card. Said to be a phone, the card knows
    // and keeps it through every later update, which factory.db can't say.
    let said = call(&mut client, &mut view, "devices_add", serde_json::json!({"name": "Phone", "kind": "watch"})).await;
    assert!(said.starts_with("kind is \"phone\", \"tablet\", \"laptop\" or \"desktop\", or left out"), "{said}");
    call(&mut client, &mut view, "devices_add", serde_json::json!({"name": "Phone", "kind": "phone"})).await;
    let phone = card(&view, "Phone").expect("card").1;
    assert_eq!(phone.kind(), v1::DeviceKind::Phone);
    let cancel = client_message::Body::CancelPairing(v1::CancelPairing { device_id: phone.device_id.clone() });
    assert!(matches!(apply_request(&mut client, &mut view, cancel).await, Body::Ack(_)));
    let ended = follow_card(&mut client, &mut view, "Phone", DevicePairingState::Cancelled).await;
    assert_eq!(ended.kind(), v1::DeviceKind::Phone, "the request keeps what the device is");

    // Its 10 minutes run out.
    call(&mut client, &mut view, "devices_add", serde_json::json!({"name": "Watch"})).await;
    let watch = card(&view, "Watch").expect("card").1;
    let mut devices = factoryd::remote::devices(&home(&dir)).expect("devices");
    devices.iter_mut().find(|device| device.id == watch.device_id).expect("waiting").code_expires = Some(1);
    std::fs::write(dir.join("home/remote/devices.json"), serde_json::to_string(&devices).expect("json")).expect("write");
    follow_card(&mut client, &mut view, "Watch", DevicePairingState::Expired).await;

    // The agent cancels one it started.
    call(&mut client, &mut view, "devices_add", serde_json::json!({"name": "Desk"})).await;
    let listed = call(&mut client, &mut view, "devices_list", serde_json::json!({})).await;
    assert!(listed.contains("Codes not used yet:\n- Desk (id desk): ends in 10m"), "{listed}");
    let said = call(&mut client, &mut view, "devices_remove", serde_json::json!({"device": "desk"})).await;
    assert_eq!(said, "Cancelled adding Desk: its code no longer works.");
    follow_card(&mut client, &mut view, "Desk", DevicePairingState::Cancelled).await;
    view.follow(&mut client, |view| {
        let told = |start: &str| {
            view.entries.iter().any(|entry| matches!(&entry.kind, Some(Kind::Event(event)) if event.summary.starts_with(start)))
        };
        told("The person didn't allow the device that used the code for Tablet")
            && told("Adding Phone was cancelled")
            && told("The code for Watch ran out")
            && idle(view, "rivet")
    })
    .await;
    assert!(view.agent("rivet").waiting_seqs.is_empty());

    // Each card shows how it ended, and what the device is only when the
    // call said (Tablet's call didn't, so its card offers both ways).
    let states: Vec<(String, DevicePairingState, v1::DeviceKind)> = view
        .entries
        .iter()
        .filter_map(|entry| match &entry.kind {
            Some(Kind::DevicePairing(pairing)) => Some((pairing.name.clone(), pairing.state(), pairing.kind())),
            _ => None,
        })
        .collect();
    assert_eq!(
        states,
        [
            ("Tablet".to_owned(), DevicePairingState::Denied, v1::DeviceKind::Unspecified),
            ("Phone".to_owned(), DevicePairingState::Cancelled, v1::DeviceKind::Phone),
            ("Watch".to_owned(), DevicePairingState::Expired, v1::DeviceKind::Unspecified),
            ("Desk".to_owned(), DevicePairingState::Cancelled, v1::DeviceKind::Unspecified),
        ]
    );

    // A new backend on the same home shows them the same, and gives a code
    // still waiting a new token, since tokens are kept only in memory.
    call(&mut client, &mut view, "devices_add", serde_json::json!({"name": "Spare"})).await;
    let spare = card(&view, "Spare").expect("card").1;
    let Body::PairingCode(before) = apply_request(&mut client, &mut view, show(&spare)).await else { panic!("a code") };
    let (restarted, factory) = start(&dir).await;
    factory.set_tunnel_online_wait(Duration::from_millis(100));
    let (mut client, workspace, transcript) = Client::connect(restarted).await;
    let mut view = View { workspace, entries: transcript.expect("Rivet's transcript").entries, ..View::default() };
    let states: Vec<(String, DevicePairingState, v1::DeviceKind)> = view
        .entries
        .iter()
        .filter_map(|entry| match &entry.kind {
            Some(Kind::DevicePairing(pairing)) => Some((pairing.name.clone(), pairing.state(), pairing.kind())),
            _ => None,
        })
        .collect();
    assert_eq!(
        states.iter().map(|(_, state, _)| *state).collect::<Vec<_>>(),
        [
            DevicePairingState::Denied,
            DevicePairingState::Cancelled,
            DevicePairingState::Expired,
            DevicePairingState::Cancelled,
            DevicePairingState::Waiting,
        ]
    );
    assert_eq!(states[1].2, v1::DeviceKind::Phone, "a reload keeps what the device is");
    let Body::PairingCode(after) = apply_request(&mut client, &mut view, show(&spare)).await else { panic!("a code") };
    assert_ne!(parse(&after.code).token, parse(&before.code).token);
    let devices = factoryd::remote::devices(&home(&dir)).expect("devices");
    let saved = devices.iter().find(|device| device.id == spare.device_id).expect("waiting");
    use sha2::Digest as _;
    let hash: String = sha2::Sha256::digest(parse(&after.code).token.as_bytes()).iter().map(|byte| format!("{byte:02x}")).collect();
    assert_eq!(saved.token_sha256, hash, "the new code is the one that works");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn only_admin_agents_have_devices_unless_given_it() {
    let dir = temp_dir();
    let (_factory, _client, view) = setup(&dir).await;
    let extension = view.workspace.extensions.iter().find(|extension| extension.id == "devices").expect("the Devices extension");
    assert!(extension.admin_default && !extension.default_enabled, "{extension:?}");
}
