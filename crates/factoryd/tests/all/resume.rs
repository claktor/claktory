//! Resuming the Workspace on reconnect (#378): an app that comes back with
//! the revision it holds gets only what changed while it was away, and ends
//! up with exactly what a fresh app gets. A restart, or a revision the
//! backend doesn't have, gets a whole Workspace, as before.

use std::sync::Arc;

use crate::common::{Client, Scripted, apply_patch, apply_request, create_agent, create_project, local_token, start, temp_dir};
use factory_protocol::v1::{self, ClientMessage, ServerMessage, client_message, server_message::Body};
use factory_protocol::PROTOCOL_VERSION;
use factoryd::factory::Factory;

/// Connects as an app that applies patches, holding `holds`: the first
/// message after Welcome.
async fn hello(addr: std::net::SocketAddr, holds: &str) -> (Client, ServerMessage) {
    let mut client = Client::open(addr).await;
    let hello = v1::Hello {
        protocol_version: PROTOCOL_VERSION,
        client_version: "test".into(),
        token: local_token(addr),
        device_kind: v1::DeviceKind::Phone.into(),
        workspace_patches: true,
        workspace_revision: holds.into(),
        ..Default::default()
    };
    client.send(ClientMessage { id: 1, body: Some(client_message::Body::Hello(hello)) }).await;
    assert!(matches!(client.receive().await.body, Some(Body::Welcome(_))));
    let first = client.receive().await;
    (client, first)
}

/// The Workspace a first message brings an app holding `held` to.
fn applied(held: &v1::Workspace, first: &ServerMessage) -> v1::Workspace {
    match first.body.as_ref().expect("a body") {
        Body::Workspace(whole) => whole.clone(),
        Body::WorkspacePatch(patch) => apply_patch(held, patch),
        other => panic!("expected the Workspace first, got {other:?}"),
    }
}

/// What a fresh app gets now.
async fn fresh(addr: std::net::SocketAddr) -> v1::Workspace {
    let (_client, first) = hello(addr, "").await;
    match first.body {
        Some(Body::Workspace(whole)) => whole,
        other => panic!("a fresh app gets a whole Workspace, got {other:?}"),
    }
}

#[tokio::test]
async fn an_app_back_from_away_gets_only_what_changed_and_sees_what_a_fresh_one_does() {
    let dir = temp_dir();
    let (addr, _factory) = start(&dir).await;
    let (app, first) = hello(addr, "").await;
    let Some(Body::Workspace(mut held)) = first.body.clone() else { panic!("whole first: {first:?}") };
    let mut revision = first.workspace_revision.clone();
    assert!(!revision.is_empty() && first.workspace_base_revision.is_empty());
    drop(app);

    // While it's away, another app (not resuming) changes things, a few
    // times over: each comeback is a patch, equal to a fresh app's view.
    let (mut other, workspace, _) = Client::connect_with(addr, v1::DeviceKind::Desktop, false).await;
    let mut view = crate::common::View { workspace, ..Default::default() };
    apply_request(&mut other, &mut view, create_project("Site", &dir.join("site"))).await;
    for (round, name) in ["Builder", "Helper", "Tester"].into_iter().enumerate() {
        apply_request(&mut other, &mut view, create_agent("site", name, "")).await;
        let rename = v1::UpdateSettings { name: Some(format!("Claktory {round}")), ..Default::default() };
        apply_request(&mut other, &mut view, client_message::Body::UpdateSettings(rename)).await;

        let (back, first) = hello(addr, &revision).await;
        assert!(matches!(first.body, Some(Body::WorkspacePatch(_))), "round {round}: {first:?}");
        assert_eq!(first.workspace_base_revision, revision);
        let before = held.clone();
        held = match first.body.as_ref() {
            Some(Body::WorkspacePatch(patch)) => apply_patch(&held, patch),
            _ => unreachable!(),
        };
        let now = fresh(addr).await;
        assert_eq!(held, now, "round {round}");
        // Real resume patches, for the app's tests (Formaggio).
        if let Some(out) = std::env::var_os("CLAKTORY_RESUME_FIXTURES") {
            use prost::Message as _;
            let out = std::path::Path::new(&out).join(format!("round{round}"));
            std::fs::create_dir_all(&out).expect("fixtures");
            std::fs::write(out.join("held_workspace.pb"), before.encode_to_vec()).expect("held");
            std::fs::write(out.join("resume_server_message.pb"), first.encode_to_vec()).expect("resume");
            std::fs::write(out.join("fresh_workspace.pb"), now.encode_to_vec()).expect("fresh");
        }
        revision = first.workspace_revision.clone();
        drop(back);
    }

    // Nothing changed: still a patch, all of it kept.
    let (_back, first) = hello(addr, &revision).await;
    let Some(Body::WorkspacePatch(patch)) = &first.body else { panic!("a patch: {first:?}") };
    assert!(patch.workspace.as_ref().is_some_and(|workspace| workspace.agents.iter().all(|agent| patch.kept_agents.contains(&agent.id))));
    assert_eq!(apply_patch(&held, patch), fresh(addr).await);

    // A revision it doesn't have: whole.
    let (_stale, first) = hello(addr, "0123456789abcdef-1").await;
    assert!(matches!(first.body, Some(Body::Workspace(_))), "{first:?}");
    assert_eq!(applied(&held, &first), fresh(addr).await);
}

#[tokio::test]
async fn after_a_restart_an_app_gets_a_whole_workspace() {
    let dir = temp_dir();
    let (addr, _factory) = start(&dir).await;
    let (app, first) = hello(addr, "").await;
    let revision = first.workspace_revision.clone();
    drop(app);

    // The same home, another process's backend (the first runs on here).
    let again = Factory::open(crate::common::home(&dir), Arc::new(Scripted::signed_in())).await.expect("open again");
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("addr");
    crate::common::serves(addr, crate::common::home(&dir));
    tokio::spawn(factoryd::serve(listener, again.clone()));
    let (_back, first) = hello(addr, &revision).await;
    assert!(matches!(first.body, Some(Body::Workspace(_))), "{first:?}");
    assert!(first.workspace_base_revision.is_empty() && !first.workspace_revision.is_empty());
    assert_ne!(first.workspace_revision, revision);
}
