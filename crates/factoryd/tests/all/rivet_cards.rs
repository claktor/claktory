//! What Rivet changes only through a card the person approves (#423, Bruno
//! 28259): Continue automatically, uninstalling an app, and widening a pipe;
//! narrowing a pipe it does at once, and the pipe says so. Dismissing or
//! replying changes nothing; approving makes exactly the change asked for,
//! checked again then, and Rivet is told how it went.

use serde_json::json;

use crate::common::{Client, MODEL, Scripted, View, apply_request, create_project, start_with_models, temp_dir};
use crate::pipes::{call, pipe_of, shown};
use factory_protocol::v1::{self, AgentStatus, client_message::Body as Request, entry::Kind};
use factoryd::pipes;

/// Rivet's cards (questions) in its conversation, with their seqs.
fn cards(view: &View) -> Vec<(u64, v1::AskPerson)> {
    view.entries
        .iter()
        .filter_map(|entry| match &entry.kind {
            Some(Kind::AskPerson(ask)) => Some((entry.seq, ask.clone())),
            _ => None,
        })
        .collect()
}

fn answer(seq: u64, dismiss: bool) -> Request {
    Request::AnswerAsk(v1::AnswerAsk { agent_id: "rivet".into(), seq, dismiss, ..Default::default() })
}

fn reply(seq: u64, text: &str) -> Request {
    Request::AnswerAsk(v1::AnswerAsk { agent_id: "rivet".into(), seq, reply: true, text: text.into(), ..Default::default() })
}

/// The event from Claktory in Rivet's conversation that holds `containing`,
/// once Rivet has run on it.
async fn told(client: &mut Client, view: &mut View, containing: &str) -> String {
    let found = |view: &View| {
        view.entries.iter().find_map(|entry| match &entry.kind {
            Some(Kind::Event(event)) if event.source() == v1::EventSource::Factory && event.text.contains(containing) => {
                Some(event.text.clone())
            }
            _ => None,
        })
    };
    view.follow(client, |view| {
        let rivet = view.agent("rivet");
        found(view).is_some() && rivet.status() == AgentStatus::Idle && rivet.queued_messages == 0
    })
    .await;
    found(view).expect("the event")
}

/// Rivet (the admin agent) on the scripted model, Site with Lea, Bo and Cy,
/// and an open pipe "Website" whose agents are Lea (its lead) and Bo.
async fn setup() -> (std::sync::Arc<factoryd::factory::Factory>, Client, View, String) {
    let dir = temp_dir();
    std::fs::create_dir_all(dir.join("home")).expect("home");
    let config = json!({
        "agents": [
            {"id": "rivet", "name": "Rivet", "model": MODEL, "effort": "off", "extensions": {"agents": {"admin": true}}},
        ],
    });
    std::fs::write(dir.join("home/config.json"), config.to_string()).expect("config");
    let (addr, factory) = start_with_models(&dir, std::sync::Arc::new(Scripted::signed_in())).await;
    let (mut client, workspace, _) = Client::connect(addr).await;
    let mut view = View { workspace, ..View::default() };
    apply_request(&mut client, &mut view, create_project("Site", &dir.join("site"))).await;
    for name in ["Lea", "Bo", "Cy"] {
        apply_request(&mut client, &mut view, crate::common::create_agent("site", name, "")).await;
    }
    pipes::set_wake_every(&factory, std::time::Duration::ZERO);
    let pipe = pipes::open_for_test(&factory, "site", "Website", &["lea", "bo"], "lea", false);
    view.follow(&mut client, |view| shown(view, &pipe).is_some()).await;
    (factory, client, view, pipe)
}

#[tokio::test]
async fn continue_automatically_changes_only_with_the_persons_approval() {
    let (_factory, mut client, mut view, _pipe) = setup().await;
    assert!(!view.workspace.usage_auto_continue);

    let said = call(&mut client, &mut view, "rivet", "models_auto_continue", r#"{"on": true, "reason": "Matt asked for it."}"#).await;
    assert!(said.starts_with("Asked "), "{said}");
    let (seq, card) = cards(&view).pop().expect("a card");
    assert_eq!(card.title, "Turn on Continue automatically");
    assert!(card.body.starts_with("Rivet asks to change it in Settings → Providers:"), "{}", card.body);
    assert!(card.body.ends_with("Matt asked for it."), "{}", card.body);
    // Its two parts apart (Fugo): Claktory's words, and Rivet's reason.
    assert!(card.action_description.starts_with("Rivet asks to change it in Settings → Providers:"), "{}", card.action_description);
    assert!(card.action_description.ends_with('.') && !card.action_description.contains("Matt asked"), "{}", card.action_description);
    assert_eq!(card.action_reason, "Matt asked for it.");
    assert!(!view.workspace.usage_auto_continue, "nothing until they approve");
    let again = call(&mut client, &mut view, "rivet", "models_auto_continue", r#"{"on": true, "reason": "Matt asked for it."}"#).await;
    assert!(again.contains("You already asked"), "{again}");

    // Dismissed: nothing changes, and Rivet is told quietly.
    apply_request(&mut client, &mut view, Request::SelectAgent(v1::SelectAgent { agent_id: "rivet".into() })).await;
    apply_request(&mut client, &mut view, answer(seq, true)).await;
    assert!(!view.workspace.usage_auto_continue);

    // Approved: it's on.
    call(&mut client, &mut view, "rivet", "models_auto_continue", r#"{"on": true, "reason": "Matt asked again."}"#).await;
    let (seq, _) = cards(&view).pop().expect("a card");
    apply_request(&mut client, &mut view, answer(seq, false)).await;
    told(&mut client, &mut view, "approved \"Turn on Continue automatically\". Continue automatically is on").await;
    let (_, decided) = cards(&view).pop().expect("the card");
    assert_eq!((decided.state(), decided.action_reason.as_str()), (v1::AskPersonState::Answered, "Matt asked again."), "kept as asked");
    // The dismissal, with nothing of the person's, came quietly with it.
    told(&mut client, &mut view, "dismissed \"Turn on Continue automatically\", so nothing was changed").await;
    view.follow(&mut client, |view| view.workspace.usage_auto_continue).await;

    // On already: nothing to ask.
    let said = call(&mut client, &mut view, "rivet", "models_auto_continue", r#"{"on": true, "reason": "x"}"#).await;
    assert_eq!(said, "Continue automatically is on already. Nothing was asked.");
}

#[tokio::test]
async fn rivet_narrows_a_pipe_at_once_and_widens_it_only_with_the_persons_approval() {
    let (factory, mut client, mut view, pipe) = setup().await;
    let before = pipe_of(&view, &pipe);
    assert_eq!(before.can_run_commands_ids, ["lea", "bo"]);
    let limits = before.limits.expect("limits");

    // Narrowing: lower limits, Bo off commands, Review incoming on, at once.
    let lower = limits.messages_per_hour - 1;
    let args = json!({"pipe": "Website", "reason": "Matt wants it quieter.", "messages_per_hour": lower, "quarantine": ["Bo"], "review_incoming": true});
    let said = call(&mut client, &mut view, "rivet", "pipes_settings", &args.to_string()).await;
    assert!(said.starts_with("The pipe \"Website\" is changed:"), "{said}");
    view.follow(&mut client, |view| pipe_of(view, &pipe).can_run_commands_ids == ["lea"]).await;
    let now = pipe_of(&view, &pipe);
    assert_eq!(now.limits.expect("limits").messages_per_hour, lower);
    assert!(now.review_incoming);
    assert!(cards(&view).is_empty(), "no card for narrowing");
    let lines = pipes::messages_for_test(&factory, &pipe);
    assert!(lines.iter().any(|line| line.text.starts_with("Rivet changed its settings: stop Bo running commands")), "{lines:?}");

    // Widening: Cy added, raised limits and Bo back on commands, on a card.
    let args = json!({"pipe": "Website", "reason": "Cy takes over.", "add": ["Cy"], "commands": ["Bo"], "messages_per_hour": lower + 5});
    let said = call(&mut client, &mut view, "rivet", "pipes_settings", &args.to_string()).await;
    assert!(said.starts_with("Asked "), "{said}");
    let (seq, card) = cards(&view).pop().expect("a card");
    assert_eq!(card.title, "Change the pipe \"Website\"");
    assert_eq!(card.action_reason, "Cy takes over.");
    assert!(card.action_description.ends_with("on the pipe \"Website\"."), "{}", card.action_description);
    assert!(card.body.starts_with("Rivet asks to add Cy, let Bo run commands on this computer and set its limits to"), "{}", card.body);
    // Its words can't be changed while it waits (Fugo): what the person
    // approves is what happens.
    let id = card.id.clone();
    let update = json!({"id": id, "title": "Pause the pipe", "body": "Rivet asks to pause it.", "reason": "Shorter."});
    let refused = call(&mut client, &mut view, "rivet", "ask_update", &update.to_string()).await;
    assert!(
        refused.contains("makes a change when the person approves it, so it can't be changed: take it back with ask_withdraw"),
        "{refused}"
    );
    let (_, kept) = cards(&view).pop().expect("the card");
    assert_eq!((kept.title.as_str(), kept.body.as_str(), kept.revision), (card.title.as_str(), card.body.as_str(), card.revision));
    let still = pipe_of(&view, &pipe);
    assert_eq!(
        (still.member_ids.clone(), still.can_run_commands_ids.clone()),
        (vec!["lea".to_owned(), "bo".to_owned()], vec!["lea".to_owned()])
    );

    // A reply changes nothing.
    apply_request(&mut client, &mut view, reply(seq, "Not Cy, Bo.")).await;
    told(&mut client, &mut view, "instead of approving it, so nothing was changed:\n\nNot Cy, Bo.").await;
    assert_eq!(pipe_of(&view, &pipe).member_ids, ["lea", "bo"]);

    // Approved: exactly that, Cy unable to run commands.
    call(&mut client, &mut view, "rivet", "pipes_settings", &args.to_string()).await;
    let (seq, _) = cards(&view).pop().expect("a card");
    apply_request(&mut client, &mut view, answer(seq, false)).await;
    told(&mut client, &mut view, "is changed: add Cy, let Bo run commands").await;
    view.follow(&mut client, |view| pipe_of(view, &pipe).member_ids.len() == 3).await;
    let after = pipe_of(&view, &pipe);
    assert_eq!(after.member_ids, ["lea", "bo", "cy"]);
    assert_eq!(after.can_run_commands_ids, ["lea", "bo"]);
    assert_eq!(after.limits.expect("limits").messages_per_hour, lower + 5);

    // A change that's no longer one to make is refused, and the card stays.
    let args = json!({"pipe": "Website", "reason": "Lead.", "lead": "Cy"});
    call(&mut client, &mut view, "rivet", "pipes_settings", &args.to_string()).await;
    let (seq, _) = cards(&view).pop().expect("a card");
    apply_request(&mut client, &mut view, Request::DeleteAgent(v1::DeleteAgent { agent_id: "cy".into() })).await;
    apply_request(&mut client, &mut view, Request::SelectAgent(v1::SelectAgent { agent_id: "rivet".into() })).await;
    match apply_request(&mut client, &mut view, answer(seq, false)).await {
        v1::server_message::Body::Error(error) => assert!(error.message.contains("dismiss this card"), "{}", error.message),
        other => panic!("not refused: {other:?}"),
    }
    assert_eq!(cards(&view).pop().expect("the card").1.state(), v1::AskPersonState::Pending);

    // What it can't do.
    let refused = call(&mut client, &mut view, "rivet", "pipes_settings", r#"{"pipe": "Website", "reason": "x"}"#).await;
    assert_eq!(refused, "Say what to change. Nothing was changed.");
    let refused =
        call(&mut client, &mut view, "rivet", "pipes_settings", r#"{"pipe": "Website", "reason": "x", "pause": true, "resume": true}"#)
            .await;
    assert_eq!(refused, "Pause or resume it, not both. Nothing was changed.");
}

#[tokio::test(flavor = "multi_thread")]
async fn rivet_uninstalls_an_app_only_with_the_persons_approval() {
    let dir = temp_dir();
    let (mut client, mut view, _factory, _stand, _tools) = crate::app_access::charts(&dir).await;
    let said = crate::app_access::call(
        &mut client,
        &mut view,
        "apps_uninstall",
        &json!({"app": "charts", "keep_data": true, "reason": "Matt is done with it."}),
    )
    .await;
    assert!(said.starts_with("Asked "), "{said}");
    let (seq, card) = cards(&view).pop().expect("a card");
    assert_eq!(card.title, "Uninstall Charts");
    assert!(card.body.contains("what it stored is kept for a later install"), "{}", card.body);
    assert!(view.workspace.apps.iter().any(|app| app.id == "charts"), "still installed");

    apply_request(&mut client, &mut view, answer(seq, false)).await;
    told(&mut client, &mut view, "approved \"Uninstall Charts\". Charts is uninstalled; what it stored is kept").await;
    view.follow(&mut client, |view| !view.workspace.apps.iter().any(|app| app.id == "charts")).await;

    let refused = crate::app_access::call(&mut client, &mut view, "apps_uninstall", &json!({"app": "charts", "reason": "x"})).await;
    assert!(refused.starts_with("There's no app \"charts\" installed"), "{refused}");
}
