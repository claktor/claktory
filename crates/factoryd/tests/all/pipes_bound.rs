//! A pipe's bound liaison (#250): made for the pipe, it's shown in it
//! (Agent.pipe_id), its conversation streams with the pipe's, the pipe's
//! wake names the messages it brings, and it goes with the pipe, its
//! conversation kept in the pipe's folder.

use std::path::PathBuf;
use std::sync::Arc;

use crate::common::{Client, MODEL, View, apply_request, create_agent, create_project, start, temp_dir};
use crate::pipes::{events, incoming, pipe_of, shown};
use factory_protocol::v1::{self, client_message};
use factoryd::factory::Factory;
use factoryd::pipes;

/// A project "Site" with Lea and Bo and an open pipe, Lea its lead; a new
/// liaison is made on the scripted model.
async fn site() -> (PathBuf, Arc<Factory>, Client, View, String) {
    let (dir, _, factory, client, view, pipe) = site_at().await;
    (dir, factory, client, view, pipe)
}

/// `site`, and the address clients connect at.
async fn site_at() -> (PathBuf, std::net::SocketAddr, Arc<Factory>, Client, View, String) {
    let dir = temp_dir();
    let (addr, factory) = start(&dir).await;
    let (mut client, workspace, _) = Client::connect(addr).await;
    let mut view = View { workspace, ..View::default() };
    apply_request(&mut client, &mut view, create_project("Site", &dir.join("site"))).await;
    apply_request(&mut client, &mut view, create_agent("site", "Lea", "")).await;
    apply_request(&mut client, &mut view, create_agent("site", "Bo", "")).await;
    let settings = v1::UpdateSettings { default_model: Some(MODEL.into()), default_effort: Some("off".into()), ..Default::default() };
    apply_request(&mut client, &mut view, client_message::Body::UpdateSettings(settings)).await;
    pipes::set_wake_every(&factory, std::time::Duration::ZERO);
    let pipe = pipes::open_for_test(&factory, "site", "Website ⇄ Marketing", &["lea", "bo"], "lea", false);
    view.follow(&mut client, |view| shown(view, &pipe).is_some()).await;
    (dir, addr, factory, client, view, pipe)
}

fn agent_named<'a>(view: &'a View, name: &str) -> Option<&'a v1::Agent> {
    view.workspace.agents.iter().find(|agent| agent.name == name)
}

#[tokio::test]
async fn a_liaison_is_bound_to_its_pipe_and_shown_with_it() {
    let (_dir, factory, mut client, mut view, pipe) = site().await;
    let suggested = pipe_of(&view, &pipe).suggested_liaison_name;
    assert!(!suggested.is_empty() && suggested != "Liaison", "{suggested}");

    // No name: it gets the one suggested, and is the pipe's.
    let add = v1::AddPipeLiaison { pipe_id: pipe.clone(), name: String::new() };
    apply_request(&mut client, &mut view, client_message::Body::AddPipeLiaison(add)).await;
    view.follow(&mut client, |view| agent_named(view, &suggested).is_some_and(|agent| agent.pipe_id == pipe)).await;
    let liaison = agent_named(&view, &suggested).expect("it").id.clone();
    assert_eq!(pipe_of(&view, &pipe).lead_id, liaison);
    assert!(view.agent("lea").pipe_id.is_empty());
    assert_ne!(pipe_of(&view, &pipe).suggested_liaison_name, suggested, "a taken name isn't suggested");

    // One liaison a pipe.
    let again = v1::AddPipeLiaison { pipe_id: pipe.clone(), name: "Other".into() };
    let refused = apply_request(&mut client, &mut view, client_message::Body::AddPipeLiaison(again)).await;
    let v1::server_message::Body::Error(error) = refused else { panic!("not refused") };
    assert!(error.message.contains(&format!("This pipe already has its liaison, {suggested}.")), "{}", error.message);

    // Shown, the pipe brings its liaison's conversation as the selected agent's.
    apply_request(&mut client, &mut view, crate::pipes::select("lea")).await;
    let select = v1::SelectPipe { pipe_id: pipe.clone() };
    apply_request(&mut client, &mut view, client_message::Body::SelectPipe(select)).await;
    view.follow(&mut client, |view| view.workspace.selected_pipe_id == pipe && view.workspace.selected_agent_id == liaison).await;

    // What arrives wakes it, the event naming the messages it brings.
    pipes::receive_for_test(&factory, &pipe, incoming("m1", "Hello")).expect("received");
    view.follow(&mut client, |view| !events(view).is_empty()).await;
    let seq = pipes::messages_for_test(&factory, &pipe).into_iter().find(|message| message.text == "Hello").expect("it").seq;
    assert_eq!(events(&view)[0].pipe_seqs, [seq]);

    // Out of the pipe's members, it's an agent of its own again.
    let members = v1::PipeMembers { agent_ids: vec!["lea".into(), "bo".into()] };
    let update = v1::UpdatePipe { pipe_id: pipe.clone(), members: Some(members), lead_id: Some("lea".into()), ..Default::default() };
    apply_request(&mut client, &mut view, client_message::Body::UpdatePipe(update)).await;
    view.follow(&mut client, |view| view.agent(&liaison).pipe_id.is_empty()).await;
    let lines: Vec<String> = pipes::messages_for_test(&factory, &pipe).into_iter().map(|message| message.text).collect();
    assert!(lines.iter().any(|line| line.starts_with(&format!("{suggested} isn't this pipe's liaison any more"))), "{lines:?}");
    // It's told, with what it has back, since it was limited to the pipe.
    apply_request(&mut client, &mut view, crate::pipes::select(&liaison)).await;
    view.follow(&mut client, |view| {
        view.entries.iter().any(|entry| {
            matches!(&entry.kind, Some(v1::entry::Kind::Event(event))
            if event.text.starts_with("The person took you out of the pipe \"Website ⇄ Marketing\": you're no longer its liaison")
                && event.text.contains("Your usual tools and folders are back"))
        })
    })
    .await;
}

// A liaison from before binding is bound once, named if it's still
// "Liaison", and told; an agent merely called that isn't.
#[tokio::test]
async fn a_liaison_from_before_is_bound_and_named_once() {
    let (_dir, factory, mut client, mut view, _) = site().await;
    let prompt = "You are the liaison for the pipe \"Launch\": you read what comes through it.";
    apply_request(&mut client, &mut view, create_agent("site", "Liaison", prompt)).await;
    apply_request(&mut client, &mut view, create_agent("site", "Ana", "Answer pipes. You are the liaison for the pipe.")).await;
    let pipe = pipes::open_for_test(&factory, "site", "Launch", &["ana", "liaison"], "ana", false);
    pipes::unbound_for_test(&factory, &pipe);
    view.follow(&mut client, |view| view.agent("liaison").pipe_id == pipe && view.agent("liaison").name != "Liaison").await;
    let name = view.agent("liaison").name.clone();
    assert!(view.agent("ana").pipe_id.is_empty());
    apply_request(&mut client, &mut view, crate::pipes::select("liaison")).await;
    view.follow(&mut client, |view| {
        view.entries
            .iter()
            .any(|entry| matches!(&entry.kind, Some(v1::entry::Kind::Event(event)) if event.text == format!("Your name is now {name}.")))
    })
    .await;
    // Once: the next start leaves it as it is.
    pipes::start_for_test(&factory);
    assert_eq!(view.agent("liaison").pipe_id, pipe);
}

// Deleting the pipe deletes its liaison; its conversation stays in the
// pipe's folder.
#[tokio::test]
async fn a_deleted_pipe_takes_its_liaison_and_keeps_its_conversation() {
    let (dir, _factory, mut client, mut view, pipe) = site().await;
    let add = v1::AddPipeLiaison { pipe_id: pipe.clone(), name: "Kai".into() };
    apply_request(&mut client, &mut view, client_message::Body::AddPipeLiaison(add)).await;
    view.follow(&mut client, |view| agent_named(view, "Kai").is_some_and(|agent| agent.pipe_id == pipe)).await;
    let kai = agent_named(&view, "Kai").expect("Kai").id.clone();
    apply_request(&mut client, &mut view, crate::pipes::select(&kai)).await;
    apply_request(&mut client, &mut view, crate::common::send_message(&kai, "Hello")).await;
    view.follow(&mut client, |view| crate::pipes::idle(view, &kai) && view.entries.len() >= 2).await;

    let named = pipe_of(&view, &pipe).folder;
    for action in [v1::PipeAction::Close, v1::PipeAction::Delete] {
        let set = v1::SetPipeState { pipe_id: pipe.clone(), action: action.into() };
        apply_request(&mut client, &mut view, client_message::Body::SetPipeState(set)).await;
    }
    view.follow(&mut client, |view| shown(view, &pipe).is_none() && agent_named(view, "Kai").is_none()).await;
    let folder = std::fs::read_dir(dir.join("site/pipes")).expect("pipes").next().expect("a folder").expect("entry").path();
    assert_eq!(folder.file_name().and_then(|name| name.to_str()), Some(named.as_str()));
    let kept: Vec<String> = std::fs::read_dir(folder.join("conversation"))
        .expect("its conversation")
        .map(|entry| entry.expect("entry").file_name().to_string_lossy().into_owned())
        .collect();
    assert!(kept.iter().any(|name| name.starts_with("segment-")), "{kept:?}");
}

/// The PipeTranscript a client connecting now gets right after the
/// Workspace (and the selected agent's Transcript).
async fn connected_with_pipe(addr: std::net::SocketAddr, pipe: &str) -> (v1::Workspace, Option<v1::Transcript>, v1::PipeTranscript) {
    let (mut client, workspace, transcript) = Client::connect(addr).await;
    assert_eq!(workspace.selected_pipe_id, pipe);
    match client.receive().await.body {
        Some(v1::server_message::Body::PipeTranscript(messages)) => (workspace, transcript, messages),
        other => panic!("expected the shown pipe's messages, got {other:?}"),
    }
}

#[tokio::test]
async fn a_client_connecting_while_a_pipe_is_shown_gets_its_messages() {
    let (_dir, addr, factory, mut client, mut view, pipe) = site_at().await;
    pipes::receive_for_test(&factory, &pipe, incoming("m1", "Hello")).expect("received");
    // A pipe of its own, shown: the app relaunched, a Claktory just added,
    // or a reconnect after a drop all start from what's shown.
    let select = v1::SelectPipe { pipe_id: pipe.clone() };
    apply_request(&mut client, &mut view, client_message::Body::SelectPipe(select.clone())).await;
    view.follow(&mut client, |view| view.workspace.selected_pipe_id == pipe).await;
    let (_, _, messages) = connected_with_pipe(addr, &pipe).await;
    assert_eq!(messages.pipe_id, pipe);
    assert!(messages.messages.iter().any(|message| message.text == "Hello"), "{messages:?}");

    // With its liaison, shown with the liaison's conversation: both come.
    let add = v1::AddPipeLiaison { pipe_id: pipe.clone(), name: "Max".into() };
    apply_request(&mut client, &mut view, client_message::Body::AddPipeLiaison(add)).await;
    view.follow(&mut client, |view| agent_named(view, "Max").is_some_and(|agent| agent.pipe_id == pipe)).await;
    let liaison = agent_named(&view, "Max").expect("it").id.clone();
    apply_request(&mut client, &mut view, client_message::Body::SelectPipe(select)).await;
    view.follow(&mut client, |view| view.workspace.selected_pipe_id == pipe && view.workspace.selected_agent_id == liaison).await;
    let (workspace, transcript, messages) = connected_with_pipe(addr, &pipe).await;
    assert_eq!(workspace.selected_agent_id, liaison);
    assert_eq!(transcript.expect("the liaison's conversation").agent_id, liaison);
    assert!(messages.messages.iter().any(|message| message.text == "Hello"), "{messages:?}");
}

#[tokio::test]
async fn the_pipe_shown_is_still_shown_after_a_restart() {
    let (dir, _, factory, mut client, mut view, pipe) = site_at().await;
    let select = v1::SelectPipe { pipe_id: pipe.clone() };
    apply_request(&mut client, &mut view, client_message::Body::SelectPipe(select)).await;
    view.follow(&mut client, |view| view.workspace.selected_pipe_id == pipe).await;

    // A restart, as an update makes: the same pipe is shown, its messages
    // with it.
    drop((client, factory));
    let (addr, factory) = start(&dir).await;
    let (mut client, workspace, _) = Client::connect(addr).await;
    assert_eq!(workspace.selected_pipe_id, pipe, "the pipe shown before the restart");
    assert!(matches!(client.receive().await.body, Some(v1::server_message::Body::PipeTranscript(shown)) if shown.pipe_id == pipe));

    // An agent shown instead: after a restart no pipe is.
    let mut view = View { workspace, ..View::default() };
    apply_request(&mut client, &mut view, crate::pipes::select("bo")).await;
    view.follow(&mut client, |view| view.workspace.selected_pipe_id.is_empty()).await;
    drop((client, factory));
    let (addr, _factory) = start(&dir).await;
    let (_client, workspace, _) = Client::connect(addr).await;
    assert_eq!((workspace.selected_pipe_id.as_str(), workspace.selected_agent_id.as_str()), ("", "bo"));
}
