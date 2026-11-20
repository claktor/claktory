//! A new Claktory's first run: Connect a model, held in the backend, and
//! Rivet greeting the person once a model is available.

use crate::common::{Client, MODEL, Scripted, View, apply_request, start_with, temp_dir};
use factory_protocol::v1::{self, AgentStatus, LoginPromptKind, OnboardingStep, client_message::Body as Request, entry::Kind};

fn step(view: &View) -> OnboardingStep {
    view.workspace.settings.as_ref().and_then(|settings| settings.onboarding.as_ref()).expect("onboarding").model()
}

async fn sign_in(client: &mut Client, view: &mut View) {
    let start = v1::StartLogin { provider_id: "scripted".into(), method_id: "api_key".into(), ..Default::default() };
    apply_request(client, view, Request::StartLogin(start)).await;
    view.follow(client, |view| {
        view.workspace.login.as_ref().and_then(|login| login.prompt.as_ref()).is_some_and(|prompt| prompt.kind() == LoginPromptKind::Secret)
    })
    .await;
    apply_request(client, view, Request::AnswerLogin(v1::AnswerLogin { text: "sk-good".into() })).await;
    view.follow(client, |view| view.workspace.login.is_none() && !view.workspace.models.is_empty()).await;
}

fn idle(view: &View, agent: &str) -> bool {
    let agent = view.agent(agent);
    agent.status() == AgentStatus::Idle && agent.queued_messages == 0
}

#[tokio::test]
async fn a_new_claktory_asks_for_a_model_and_rivet_greets_once_one_is_connected() {
    let dir = temp_dir();
    let (addr, _factory) = start_with(&dir, Scripted::signed_out()).await;
    let (mut client, workspace, _) = Client::connect(addr).await;
    let mut view = View { workspace, ..View::default() };
    assert_eq!(step(&view), OnboardingStep::Needed);

    // Skipped, it stays skipped on every device, until a model is connected.
    let skip = Request::SetOnboarding(v1::SetOnboarding { skip_model: true });
    apply_request(&mut client, &mut view, skip.clone()).await;
    assert_eq!(step(&view), OnboardingStep::Skipped);
    let (_other, workspace, _) = Client::connect(addr).await;
    assert_eq!(step(&View { workspace, ..View::default() }), OnboardingStep::Skipped);

    // Connected: done, and Rivet, moved to a model that runs, greets.
    sign_in(&mut client, &mut view).await;
    view.follow(&mut client, |view| step(view) == OnboardingStep::Done).await;
    view.follow(&mut client, |view| view.agent("rivet").model == MODEL).await;
    apply_request(&mut client, &mut view, Request::SelectAgent(v1::SelectAgent { agent_id: "rivet".into() })).await;
    view.follow(&mut client, |view| view.entries.len() >= 2 && idle(view, "rivet")).await;
    let Some(Kind::Event(event)) = &view.entries[0].kind else { panic!("Claktory's event first: {:?}", view.entries) };
    assert_eq!(event.source(), v1::EventSource::Factory);
    assert_eq!(event.summary, "The person connected a model");
    assert!(
        matches!(&view.entries[1].kind, Some(Kind::Assistant(text)) if text.text.contains("The person just set up Claktory and connected Scripted key. Greet them in two sentences")
            && text.text.contains("You run on Echo now")),
        "{:?}",
        view.entries[1]
    );

    // Only once: signing out and in again greets no more.
    let out = v1::SignOut { provider_id: "scripted".into(), ..Default::default() };
    apply_request(&mut client, &mut view, Request::SignOut(out)).await;
    sign_in(&mut client, &mut view).await;
    apply_request(&mut client, &mut view, skip).await;
    assert_eq!(step(&view), OnboardingStep::Done, "skipping a step that's done changes nothing");
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    view.follow(&mut client, |view| idle(view, "rivet")).await;
    assert_eq!(view.entries.len(), 2);
}

#[tokio::test]
async fn a_claktory_with_a_model_from_the_start_or_from_before_needs_no_step() {
    // A model from the start: nothing to connect, and Rivet waits for the person.
    let dir = temp_dir();
    let (addr, _factory) = start_with(&dir, Scripted::signed_in()).await;
    let (_client, workspace, _) = Client::connect(addr).await;
    let view = View { workspace, ..View::default() };
    assert_eq!(step(&view), OnboardingStep::Done);
    assert_eq!(view.agent("rivet").queued_messages, 0);

    // A Claktory from before it (its settings already there), even with no model.
    let dir = temp_dir();
    std::fs::create_dir_all(dir.join("home")).expect("home");
    std::fs::write(dir.join("home/config.json"), "{}").expect("config");
    let (addr, _factory) = start_with(&dir, Scripted::signed_out()).await;
    let (_client, workspace, _) = Client::connect(addr).await;
    assert_eq!(step(&View { workspace, ..View::default() }), OnboardingStep::Done);
}
