//! A pipe's liaison uses no other pipe
//! in any role; whoever asks, it's refused with why, and the pipe says
//! who's held. Agents in two ordinary pipes stay allowed, and so does one
//! limited to several. A Claktory from before takes a liaison off its other
//! pipe at start, saying so.

use std::sync::Arc;

use crate::common::{Client, MODEL, Scripted, View, apply_request, create_project, start_with_models, temp_dir};
use crate::pipes::{call, pipe_of, shown};
use factory_protocol::v1::{self, client_message::Body as Request, server_message};
use factoryd::factory::Factory;
use factoryd::pipes;

const HELD: &str = "Freya is Partners' liaison: it reads that pipe, so it can't use another.";

/// Site with Lea and Bo, Rivet (the admin agent), and two open pipes the
/// person named: "Partners" (Lea) and "Launch" (Bo).
async fn setup() -> (Arc<Factory>, Client, View, String, String) {
    let dir = temp_dir();
    std::fs::create_dir_all(dir.join("home")).expect("home");
    let config = serde_json::json!({
        "agents": [
            {"id": "rivet", "name": "Rivet", "model": MODEL, "effort": "off", "extensions": {"agents": {"admin": true}}},
        ],
    });
    std::fs::write(dir.join("home/config.json"), config.to_string()).expect("config");
    let (addr, factory) = start_with_models(&dir, Arc::new(Scripted::signed_in())).await;
    let (mut client, workspace, _) = Client::connect(addr).await;
    let mut view = View { workspace, ..View::default() };
    apply_request(&mut client, &mut view, create_project("Site", &dir.join("site"))).await;
    for name in ["Lea", "Bo"] {
        apply_request(&mut client, &mut view, crate::common::create_agent("site", name, "")).await;
    }
    let settings = v1::UpdateSettings { default_model: Some(MODEL.into()), default_effort: Some("off".into()), ..Default::default() };
    apply_request(&mut client, &mut view, Request::UpdateSettings(settings)).await;
    pipes::set_wake_every(&factory, std::time::Duration::ZERO);
    let partners = pipes::open_for_test(&factory, "site", "p", &["lea"], "lea", false);
    let launch = pipes::open_for_test(&factory, "site", "l", &["bo"], "bo", false);
    for (pipe, name) in [(&partners, "Partners"), (&launch, "Launch")] {
        let rename = v1::UpdatePipe { pipe_id: pipe.clone(), name: Some(name.into()), ..Default::default() };
        apply_request(&mut client, &mut view, Request::UpdatePipe(rename)).await;
    }
    view.follow(&mut client, |view| shown(view, &launch).is_some_and(|pipe| pipe.name == "Launch")).await;
    (factory, client, view, partners, launch)
}

/// Partners gets its liaison, Freya.
async fn with_freya(client: &mut Client, view: &mut View, partners: &str) {
    let add = v1::AddPipeLiaison { pipe_id: partners.to_owned(), name: "Freya".into() };
    apply_request(client, view, Request::AddPipeLiaison(add)).await;
    view.follow(client, |view| shown(view, partners).is_some_and(|pipe| pipe.lead_id == "freya")).await;
}

fn ids(ids: &[&str]) -> Option<v1::PipeMembers> {
    Some(v1::PipeMembers { agent_ids: ids.iter().map(|id| (*id).to_owned()).collect() })
}

async fn refused(client: &mut Client, view: &mut View, request: Request) -> String {
    match apply_request(client, view, request).await {
        server_message::Body::Error(error) => error.message,
        other => panic!("not refused: {other:?}"),
    }
}

fn held(pipe: &v1::Pipe) -> Vec<(String, String)> {
    pipe.held_agents.iter().map(|held| (held.agent_id.clone(), held.reason.clone())).collect()
}

#[tokio::test]
async fn a_pipes_liaison_is_refused_on_another_pipe_whoever_asks() {
    let (factory, mut client, mut view, partners, launch) = setup().await;
    with_freya(&mut client, &mut view, &partners).await;

    // Launch shows Freya held, with why; Partners, its own, doesn't.
    assert_eq!(held(&pipe_of(&view, &launch)), [("freya".to_owned(), HELD.to_owned())]);
    assert!(held(&pipe_of(&view, &partners)).is_empty());

    // As a member, only sending, or the lead: refused, and Launch unchanged.
    let member = v1::UpdatePipe { pipe_id: launch.clone(), members: ids(&["bo", "freya"]), ..Default::default() };
    assert_eq!(refused(&mut client, &mut view, Request::UpdatePipe(member)).await, HELD);
    let sending =
        v1::UpdatePipe { pipe_id: launch.clone(), members: ids(&["bo", "freya"]), send_only: ids(&["freya"]), ..Default::default() };
    assert_eq!(refused(&mut client, &mut view, Request::UpdatePipe(sending)).await, HELD);
    let lead =
        v1::UpdatePipe { pipe_id: launch.clone(), members: ids(&["bo", "freya"]), lead_id: Some("freya".into()), ..Default::default() };
    assert_eq!(refused(&mut client, &mut view, Request::UpdatePipe(lead)).await, HELD);
    assert_eq!(pipe_of(&view, &launch).member_ids, ["bo"]);

    // Rivet letting it only send: refused, saying why.
    let said = call(&mut client, &mut view, "rivet", "pipes_sender", r#"{"pipe": "Launch", "agent": "Freya", "send": true}"#).await;
    assert!(said.contains(HELD), "{said}");
    assert_eq!(pipe_of(&view, &launch).member_ids, ["bo"]);

    // Approving a pipe with it: refused before anything's made.
    let waiting = pipes::approval_for_test(&factory, "site", "Other");
    let approve = v1::ApprovePipe {
        pipe_id: waiting.clone(),
        approve: true,
        member_ids: vec!["freya".into()],
        lead_id: "freya".into(),
        ..Default::default()
    };
    let said = refused(&mut client, &mut view, Request::ApprovePipe(approve)).await;
    assert!(said.starts_with("Freya is "), "{said}");

    // Once it's no longer Partners' liaison, it can join Launch.
    let out = v1::UpdatePipe { pipe_id: partners.clone(), members: ids(&["lea"]), lead_id: Some("lea".into()), ..Default::default() };
    apply_request(&mut client, &mut view, Request::UpdatePipe(out)).await;
    view.follow(&mut client, |view| shown(view, &launch).is_some_and(|pipe| pipe.held_agents.is_empty())).await;
    let join = v1::UpdatePipe { pipe_id: launch.clone(), members: ids(&["bo", "freya"]), ..Default::default() };
    apply_request(&mut client, &mut view, Request::UpdatePipe(join)).await;
    view.follow(&mut client, |view| shown(view, &launch).is_some_and(|pipe| pipe.member_ids == ["bo", "freya"])).await;
}

/// Limiting is the person's containment, built to span pipes:
/// never refused, and kept across a restart.
#[tokio::test]
async fn an_agent_on_another_pipe_can_be_limited_to_this_one_too() {
    let (factory, mut client, mut view, partners, launch) = setup().await;
    assert!(pipe_of(&view, &partners).cant_limit.is_empty() && pipe_of(&view, &launch).held_agents.is_empty());
    // Bo, Launch's, limited to Partners too; then to Launch as well.
    let limited =
        v1::UpdatePipe { pipe_id: partners.clone(), members: ids(&["lea", "bo"]), quarantined: ids(&["bo"]), ..Default::default() };
    apply_request(&mut client, &mut view, Request::UpdatePipe(limited)).await;
    view.follow(&mut client, |view| shown(view, &partners).is_some_and(|pipe| pipe.quarantined_ids == ["bo"])).await;
    let limited = v1::UpdatePipe { pipe_id: launch.clone(), quarantined: ids(&["bo"]), ..Default::default() };
    apply_request(&mut client, &mut view, Request::UpdatePipe(limited)).await;
    view.follow(&mut client, |view| shown(view, &launch).is_some_and(|pipe| pipe.quarantined_ids == ["bo"])).await;
    assert!(pipe_of(&view, &launch).held_agents.is_empty());
    // A restart keeps it on both.
    pipes::start_for_test(&factory);
    assert_eq!(pipe_of(&view, &partners).member_ids, ["lea", "bo"]);
    assert_eq!(pipe_of(&view, &launch).member_ids, ["bo"]);
    assert_eq!(pipe_of(&view, &launch).quarantined_ids, ["bo"]);
}

#[tokio::test]
async fn a_liaison_from_before_on_another_pipe_leaves_it_saying_why() {
    let (factory, mut client, mut view, partners, launch) = setup().await;
    with_freya(&mut client, &mut view, &partners).await;
    // As a Claktory from before could have it: Freya on Launch too, which
    // it leads, with Bo limited to both pipes.
    pipes::join_unchecked_for_test(&factory, &launch, "freya", false);
    let lead = v1::UpdatePipe { pipe_id: launch.clone(), lead_id: Some("freya".into()), ..Default::default() };
    let said = refused(&mut client, &mut view, Request::UpdatePipe(lead)).await;
    assert_eq!(said, HELD, "anything changed on Launch is refused until it leaves");
    pipes::join_unchecked_for_test(&factory, &partners, "bo", true);
    pipes::join_unchecked_for_test(&factory, &launch, "bo", true);
    pipes::start_for_test(&factory);
    view.follow(&mut client, |view| shown(view, &launch).is_some_and(|pipe| pipe.member_ids == ["bo"])).await;
    assert_eq!(pipe_of(&view, &partners).member_ids, ["lea", "freya", "bo"]);
    assert_eq!(pipe_of(&view, &launch).quarantined_ids, ["bo"]);
    let lines = pipes::messages_for_test(&factory, &launch);
    assert!(lines.iter().any(|line| line.text == format!("Freya left this pipe. {HELD}")), "{lines:?}");
}
