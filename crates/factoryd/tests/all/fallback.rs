//! The refusal fallback (#225): a step the agent's model declines is
//! retried once on the fallback model named in the agent's settings (or
//! the Claktory-wide default), the refusal shown as itself, and the agent
//! told in a note.

use crate::common::{
    Client, MODEL, OTHER_MODEL, Scripted, View, apply_request, create_agent, describe, send_message, start_with, temp_dir,
};
use factory_protocol::v1::{self, client_message::Body as Request, entry::Kind};

fn set_agent_fallback(agent: &str, model: &str) -> Request {
    Request::UpdateAgent(v1::UpdateAgent { agent_id: agent.into(), fallback_model: Some(model.into()), ..Default::default() })
}

fn set_default_fallback(model: &str) -> Request {
    Request::UpdateSettings(v1::UpdateSettings { default_fallback_model: Some(model.into()), ..Default::default() })
}

fn idle(view: &View, id: &str) -> bool {
    view.agent(id).status() == v1::AgentStatus::Idle
}

/// The refusal notices among `entries`: (text, category).
fn refusals(entries: &[v1::Entry]) -> Vec<(String, String)> {
    entries
        .iter()
        .filter_map(|entry| match &entry.kind {
            Some(Kind::Notice(notice)) if !notice.refusal_category.is_empty() => {
                Some((notice.text.clone(), notice.refusal_category.clone()))
            }
            _ => None,
        })
        .collect()
}

#[tokio::test]
async fn a_refused_step_is_retried_on_the_agents_fallback_and_shown_as_itself() {
    let dir = temp_dir();
    let scripted = Scripted::signed_in();
    let requests = scripted.requests.clone();
    let (addr, _factory) = start_with(&dir, scripted).await;
    let (mut client, workspace, _) = Client::connect(addr).await;
    let mut view = View { workspace, ..View::default() };
    apply_request(&mut client, &mut view, create_agent("", "Builder", "")).await;
    apply_request(&mut client, &mut view, Request::SelectAgent(v1::SelectAgent { agent_id: "builder".into() })).await;
    apply_request(&mut client, &mut view, set_agent_fallback("builder", OTHER_MODEL)).await;
    assert_eq!(view.agent("builder").fallback_model, OTHER_MODEL);

    apply_request(&mut client, &mut view, send_message("builder", "please decline")).await;
    view.follow(&mut client, |view| view.entries.len() >= 4 && idle(view, "builder")).await;
    // Echo declined; Parrot answered the same context; the agent was told.
    let models: Vec<String> = requests.lock().expect("lock").iter().map(|request| request.model.clone()).collect();
    assert_eq!(models, ["echo", "parrot"]);
    let entries = describe(&view.entries);
    assert_eq!(entries[0], "user: please decline");
    assert_eq!(entries[1], "notice: I can't help with this.");
    assert_eq!(entries[2], "assistant: echo: please decline");
    assert_eq!(entries[3], "event from Claktory: Another model answered a step this agent's model declined.");
    // The notice is a refusal, with the policy area the model named.
    assert_eq!(refusals(&view.entries), [("I can't help with this.".to_owned(), "cybersecurity".to_owned())]);

    // The settings survive in config.json and clear with an empty value.
    let config = std::fs::read_to_string(dir.join("home/config.json")).expect("config");
    assert!(config.contains("\"fallback_model\""), "{config}");
    apply_request(&mut client, &mut view, set_agent_fallback("builder", "")).await;
    assert_eq!(view.agent("builder").fallback_model, "");
    let config = std::fs::read_to_string(dir.join("home/config.json")).expect("config");
    assert!(!config.contains("\"fallback_model\""), "{config}");
}

#[tokio::test]
async fn the_claktory_wide_default_applies_when_the_agent_names_no_fallback() {
    let dir = temp_dir();
    let scripted = Scripted::signed_in();
    let requests = scripted.requests.clone();
    let (addr, _factory) = start_with(&dir, scripted).await;
    let (mut client, workspace, _) = Client::connect(addr).await;
    let mut view = View { workspace, ..View::default() };
    apply_request(&mut client, &mut view, create_agent("", "Builder", "")).await;
    apply_request(&mut client, &mut view, Request::SelectAgent(v1::SelectAgent { agent_id: "builder".into() })).await;
    apply_request(&mut client, &mut view, set_default_fallback(OTHER_MODEL)).await;
    assert_eq!(view.workspace.settings.as_ref().map(|settings| settings.default_fallback_model.clone()), Some(OTHER_MODEL.to_owned()));
    let config = std::fs::read_to_string(dir.join("home/config.json")).expect("config");
    assert!(config.contains("\"default_fallback_model\""), "{config}");

    apply_request(&mut client, &mut view, send_message("builder", "please decline")).await;
    view.follow(&mut client, |view| view.entries.len() >= 4 && idle(view, "builder")).await;
    let models: Vec<String> = requests.lock().expect("lock").iter().map(|request| request.model.clone()).collect();
    assert_eq!(models, ["echo", "parrot"]);
    assert_eq!(describe(&view.entries)[2], "assistant: echo: please decline");
}

#[tokio::test]
async fn without_a_usable_fallback_the_refusal_stands() {
    // No fallback at all, the agent's own model, and a model that isn't
    // available: each leaves the refusal as the turn's end.
    for fallback in ["", MODEL, "scripted/unknown"] {
        let dir = temp_dir();
        let scripted = Scripted::signed_in();
        let requests = scripted.requests.clone();
        let (addr, _factory) = start_with(&dir, scripted).await;
        let (mut client, workspace, _) = Client::connect(addr).await;
        let mut view = View { workspace, ..View::default() };
        apply_request(&mut client, &mut view, create_agent("", "Builder", "")).await;
        apply_request(&mut client, &mut view, Request::SelectAgent(v1::SelectAgent { agent_id: "builder".into() })).await;
        if !fallback.is_empty() {
            apply_request(&mut client, &mut view, set_agent_fallback("builder", fallback)).await;
        }
        apply_request(&mut client, &mut view, send_message("builder", "please decline")).await;
        view.follow(&mut client, |view| view.entries.len() >= 2 && idle(view, "builder")).await;
        let models: Vec<String> = requests.lock().expect("lock").iter().map(|request| request.model.clone()).collect();
        assert_eq!(models, ["echo"], "fallback: {fallback:?}");
        assert_eq!(refusals(&view.entries), [("I can't help with this.".to_owned(), "cybersecurity".to_owned())], "fallback: {fallback:?}");
        assert_eq!(view.entries.len(), 2, "no note about an answer that never came; fallback: {fallback:?}");
    }
}
