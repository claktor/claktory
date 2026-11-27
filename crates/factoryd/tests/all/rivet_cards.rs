//! What Rivet changes only through a card the person approves: Continue
//! automatically, uninstalling an app, and widening a pipe; narrowing a pipe
//! it does at once, and the pipe says so. Dismissing or replying changes
//! nothing; approving makes exactly the change asked for, checked again
//! then, and Rivet is told how it went.

use serde_json::json;

use crate::common::{Client, MODEL, Scripted, View, apply_request, create_project, start_with_models, temp_dir};
use crate::pipes::{call, pipe_of, shown};
use factory_protocol::v1::{self, AgentStatus, client_message::Body as Request, entry::Kind};
use factoryd::pipes;

/// Rivet's cards (questions) in its conversation, with their seqs.
pub(crate) fn cards(view: &View) -> Vec<(u64, v1::AskPerson)> {
    view.entries
        .iter()
        .filter_map(|entry| match &entry.kind {
            Some(Kind::AskPerson(ask)) => Some((entry.seq, ask.clone())),
            _ => None,
        })
        .collect()
}

pub(crate) fn answer(seq: u64, dismiss: bool) -> Request {
    Request::AnswerAsk(v1::AnswerAsk { agent_id: "rivet".into(), seq, dismiss, ..Default::default() })
}

fn reply(seq: u64, text: &str) -> Request {
    Request::AnswerAsk(v1::AnswerAsk { agent_id: "rivet".into(), seq, reply: true, text: text.into(), ..Default::default() })
}

/// The event from Claktory in Rivet's conversation that holds `containing`,
/// once Rivet has run on it.
pub(crate) async fn told(client: &mut Client, view: &mut View, containing: &str) -> String {
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
pub(crate) async fn setup() -> (std::sync::Arc<factoryd::factory::Factory>, Client, View, String) {
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
    // Its two parts apart: Claktory's words, and Rivet's reason.
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

    // Its words can't be talked over: a reply to the person's
    // comment, with new words or none, is refused, and the card stays as it was.
    call(&mut client, &mut view, "rivet", "models_auto_continue", r#"{"on": false, "reason": "Matt asked."}"#).await;
    let (seq, _) = cards(&view).pop().expect("a card");
    apply_request(&mut client, &mut view, reply(seq, "Only at night, please.")).await;
    told(&mut client, &mut view, "Only at night, please.").await;
    let before = cards(&view).pop().expect("the card").1;
    for args in [json!({"id": before.id, "text": "Nights only, then."}), json!({"id": before.id, "text": "Nights only.", "body": "Turn it off at night."}), json!({"id": before.id, "text": "Settled.", "resolve": true})] {
        let said = call(&mut client, &mut view, "rivet", "ask_reply", &args.to_string()).await;
        assert!(said.contains("does exactly what it shows when the person approves it, so it can't be answered on, opened again or changed in a reply."), "{said}");
    }
    let after = cards(&view).pop().expect("the card").1;
    assert_eq!((after.revision, after.body.as_str(), after.thread.len(), after.approve_follows_talk), (before.revision, before.body.as_str(), 0, false));
    assert!(view.workspace.usage_auto_continue, "nothing changed");
}

#[tokio::test]
async fn rivet_narrows_a_pipe_at_once_and_widens_it_only_with_the_persons_approval() {
    let (factory, mut client, mut view, pipe) = setup().await;
    let before = pipe_of(&view, &pipe);
    assert_eq!(before.can_run_commands_ids, ["lea", "bo"]);
    let limits = before.limits.expect("limits");

    let matt = v1::UpdateSettings { user_name: Some("Matt".into()), ..Default::default() };
    apply_request(&mut client, &mut view, Request::UpdateSettings(matt)).await;
    // Narrowing: lower limits, Bo off commands, Review incoming on, at once.
    let lower = limits.messages_per_hour - 1;
    let args = json!({"pipe": "Website", "reason": "Matt wants it quieter.", "messages_per_hour": lower, "quarantine": ["Bo"], "review_incoming": true});
    let said = call(&mut client, &mut view, "rivet", "pipes_settings", &args.to_string()).await;
    assert!(said.starts_with("Stopped Bo running commands on this computer in the pipe \"Website\", set its limits to"), "{said}");
    assert!(said.contains(" and turned Review incoming on for it. "), "{said}");
    assert!(said.ends_with(" Matt sees it in the pipe."), "{said}");
    view.follow(&mut client, |view| pipe_of(view, &pipe).can_run_commands_ids == ["lea"]).await;
    let now = pipe_of(&view, &pipe);
    assert_eq!(now.limits.expect("limits").messages_per_hour, lower);
    assert!(now.review_incoming);
    assert!(cards(&view).is_empty(), "no request for narrowing");
    let lines = pipes::messages_for_test(&factory, &pipe);
    assert!(
        lines
            .iter()
            .any(|line| line.text.starts_with("Rivet stopped Bo running commands on this computer in this pipe, set its limits to")),
        "{lines:?}"
    );

    // Widening: Cy added, raised limits and Bo back on commands, on a card.
    let args = json!({"pipe": "Website", "reason": "Cy takes over.", "add": ["Cy"], "commands": ["Bo"], "messages_per_hour": lower + 5});
    let said = call(&mut client, &mut view, "rivet", "pipes_settings", &args.to_string()).await;
    assert!(said.starts_with("Asked "), "{said}");
    let (seq, card) = cards(&view).pop().expect("a card");
    assert_eq!(card.title, "Change the pipe \"Website\"");
    assert_eq!(card.action_reason, "Cy takes over.");
    assert!(
        card.action_description
            .starts_with("Rivet asks to add Cy to the pipe \"Website\", let Bo run commands on this computer in it and set its limits to"),
        "{}",
        card.action_description
    );
    assert!(card.body.starts_with(&card.action_description), "{}", card.body);
    // Its words can't be changed while it waits: what the person
    // approves is what happens.
    let id = card.id.clone();
    let update = json!({"id": id, "title": "Pause the pipe", "body": "Rivet asks to pause it.", "reason": "Shorter."});
    let refused = call(&mut client, &mut view, "rivet", "ask_update", &update.to_string()).await;
    assert!(refused.contains("makes a change when Matt approves it, so it can't be changed: take it back with ask_withdraw"), "{refused}");
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
    told(&mut client, &mut view, "approved \"Change the pipe \"Website\"\". Added Cy to the pipe \"Website\", let Bo run commands").await;
    view.follow(&mut client, |view| pipe_of(view, &pipe).member_ids.len() == 3).await;
    let after = pipe_of(&view, &pipe);
    assert_eq!(after.member_ids, ["lea", "bo", "cy"]);
    assert_eq!(after.can_run_commands_ids, ["lea", "bo"]);
    assert_eq!(after.limits.expect("limits").messages_per_hour, lower + 5);
    // Who joined shows in the pipe.
    let lines = pipes::messages_for_test(&factory, &pipe);
    assert!(lines.iter().any(|line| line.text == "Cy joined the pipe."), "{lines:?}");

    // A change that's no longer one to make is refused, and the card stays.
    let args = json!({"pipe": "Website", "reason": "Lead.", "lead": "Cy"});
    call(&mut client, &mut view, "rivet", "pipes_settings", &args.to_string()).await;
    let (seq, _) = cards(&view).pop().expect("a card");
    apply_request(&mut client, &mut view, Request::DeleteAgent(v1::DeleteAgent { agent_id: "cy".into() })).await;
    apply_request(&mut client, &mut view, Request::SelectAgent(v1::SelectAgent { agent_id: "rivet".into() })).await;
    match apply_request(&mut client, &mut view, answer(seq, false)).await {
        // Named as the person knew it, not by its id.
        v1::server_message::Body::Error(error) => {
            assert!(error.message.starts_with("Cy is no longer in this Claktory"), "{}", error.message)
        }
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

/// Moving a pipe into a group that shares websites gives the pipe's other
/// side those websites too: when Rivet does it, it's a card, and nothing
/// moves until the person approves. A group with none moves it at once.
#[tokio::test]
async fn moving_a_pipe_into_a_group_that_shares_websites_asks_the_person() {
    let (_factory, mut client, mut view, pipe) = setup().await;
    for name in ["Partners", "Plain"] {
        let group = v1::CreatePipeGroup { project_id: "site".into(), name: name.into(), pipe_id: String::new() };
        apply_request(&mut client, &mut view, Request::CreatePipeGroup(group)).await;
    }
    view.follow(&mut client, |view| view.workspace.pipe_groups.len() == 2).await;
    let group = |view: &View, name: &str| view.workspace.pipe_groups.iter().find(|group| group.name == name).expect("group").id.clone();
    let partners = group(&view, "Partners");
    let share = v1::SharePipeWebsite { port: 8123, name: "Dashboard".into(), group_id: partners.clone(), ..Default::default() };
    apply_request(&mut client, &mut view, Request::SharePipeWebsite(share)).await;
    view.follow(&mut client, |view| !view.workspace.pipe_shares.is_empty()).await;

    let args = json!({"action": "move", "pipe": "Website", "group": "Partners", "reason": "Matt wants the partners together."}).to_string();
    let said = call(&mut client, &mut view, "rivet", "pipes_group", &args).await;
    assert!(said.contains("Asked the person"), "{said}");
    let (seq, card) = cards(&view).pop().expect("a card");
    assert!(card.action_description.contains("This pipe will also get: Dashboard."), "{}", card.action_description);
    assert_eq!(card.action_reason, "Matt wants the partners together.");
    assert!(pipe_of(&view, &pipe).group_id.is_empty(), "nothing moved yet");

    // The group shares more meanwhile: Approve is refused, the card stays;
    // asked again, the card names both.
    let more = v1::SharePipeWebsite { port: 8124, name: "Metrics".into(), group_id: partners.clone(), ..Default::default() };
    apply_request(&mut client, &mut view, Request::SharePipeWebsite(more)).await;
    apply_request(&mut client, &mut view, Request::SelectAgent(v1::SelectAgent { agent_id: "rivet".into() })).await;
    match apply_request(&mut client, &mut view, answer(seq, false)).await {
        v1::server_message::Body::Error(error) => {
            assert_eq!(error.message, "The group shares more websites now: dismiss this request and ask again.")
        }
        other => panic!("not refused: {other:?}"),
    }
    assert!(pipe_of(&view, &pipe).group_id.is_empty(), "nothing moved");
    apply_request(&mut client, &mut view, answer(seq, true)).await;
    let said = call(&mut client, &mut view, "rivet", "pipes_group", &args).await;
    assert!(said.contains("Asked the person"), "{said}");
    let (seq, card) = cards(&view).pop().expect("a card");
    assert!(card.action_description.contains("This pipe will also get: Dashboard and Metrics."), "{}", card.action_description);

    apply_request(&mut client, &mut view, answer(seq, false)).await;
    view.follow(&mut client, |view| pipe_of(view, &pipe).group_id == partners).await;
    told(&mut client, &mut view, "Moved").await;

    // Out of it, and into a group with no websites: at once.
    let args = json!({"action": "move", "pipe": "Website", "group": "Plain"}).to_string();
    let said = call(&mut client, &mut view, "rivet", "pipes_group", &args).await;
    assert!(said.starts_with("Moved"), "{said}");
    let plain = group(&view, "Plain");
    view.follow(&mut client, |view| pipe_of(view, &pipe).group_id == plain).await;
}

/// Another Claktory can share at most 32 websites over one pipe: past
/// that, more are dropped, with no line for each.
#[tokio::test]
async fn the_other_side_shares_at_most_so_many_websites_over_a_pipe() {
    let (factory, mut client, mut view, pipe) = setup().await;
    for n in 0..40 {
        pipes::shares::offered_for_test(&factory, &pipe, "peer-key", &format!("{n:032x}"), &format!("Site {n}"));
    }
    view.follow(&mut client, |view| view.workspace.pipe_shares.len() == 32).await;
    assert!(view.workspace.pipe_shares.iter().all(|share| !share.mine && share.pipe_ids == [pipe.clone()]));
    let lines = pipes::messages_for_test(&factory, &pipe).into_iter().filter(|line| line.text.contains("shares Site")).count();
    assert_eq!(lines, 32);
    // One it has already is updated, not refused.
    pipes::shares::offered_for_test(&factory, &pipe, "peer-key", &format!("{:032x}", 0), "Renamed");
    view.follow(&mut client, |view| view.workspace.pipe_shares.iter().any(|share| share.name == "Renamed")).await;
}

/// Approved, but the change couldn't be made: the card says why on every device, not only to Rivet.
#[tokio::test]
async fn an_approved_change_that_cant_be_made_says_why_on_the_card() {
    let (factory, mut client, mut view, pipe) = setup().await;
    let other = pipes::open_for_test(&factory, "site", "Partners", &["cy"], "cy", false);
    let add = v1::AddPipeLiaison { pipe_id: other.clone(), name: "Freya".into() };
    apply_request(&mut client, &mut view, Request::AddPipeLiaison(add)).await;
    view.follow(&mut client, |view| shown(view, &other).is_some_and(|pipe| pipe.lead_id == "freya")).await;

    let args = json!({"pipe": "Website", "reason": "Freya helps.", "add": ["Freya"]});
    let said = call(&mut client, &mut view, "rivet", "pipes_settings", &args.to_string()).await;
    assert!(said.starts_with("Asked "), "{said}");
    let (seq, card) = cards(&view).pop().expect("a card");
    assert_eq!(card.action_error, "", "nothing to say while it waits");
    apply_request(&mut client, &mut view, answer(seq, false)).await;
    let told = told(&mut client, &mut view, "but it couldn't be done: ").await;
    let why = told.split("but it couldn't be done: ").nth(1).expect("why").to_owned();
    assert!(why.starts_with("Freya is Partners' liaison"), "{why}");

    let (_, decided) = cards(&view).pop().expect("the card");
    assert_eq!(decided.state(), v1::AskPersonState::Answered);
    assert!(why.starts_with(&decided.action_error) && !decided.action_error.is_empty(), "{:?} / {why}", decided.action_error);
    assert_eq!(pipe_of(&view, &pipe).member_ids, ["lea", "bo"], "nothing changed");

    // Kept: the conversation read again still says it.
    apply_request(&mut client, &mut view, Request::SelectAgent(v1::SelectAgent { agent_id: "lea".into() })).await;
    apply_request(&mut client, &mut view, Request::SelectAgent(v1::SelectAgent { agent_id: "rivet".into() })).await;
    let (_, again) = cards(&view).pop().expect("the card");
    assert_eq!(again.action_error, decided.action_error);
}

/// Rivet asks to share a website with a pipe
/// only on a card, which shows exactly what it exposes; never for another
/// agent, nor while it has read from outside, nor with a pipe it can't
/// trust, nor for a port that can't be shared; and nothing is shared until
/// the person approves, the same program still listening.
#[tokio::test]
async fn rivet_asks_to_share_a_website_only_on_a_card_that_shows_what_it_exposes() {
    let dir = temp_dir();
    std::fs::create_dir_all(dir.join("home")).expect("home");
    let config = json!({
        "agents": [
            {"id": "rivet", "name": "Rivet", "model": MODEL, "effort": "off", "extensions": {"agents": {"admin": true}}},
            {"id": "dev", "name": "Dev", "model": MODEL, "effort": "off", "extensions": {"pipes": {"enabled": true}}},
        ],
    });
    std::fs::write(dir.join("home/config.json"), config.to_string()).expect("config");
    let (addr, factory) = start_with_models(&dir, std::sync::Arc::new(Scripted::signed_in())).await;
    let (mut client, workspace, _) = Client::connect(addr).await;
    let mut view = View { workspace, ..View::default() };
    apply_request(&mut client, &mut view, create_project("Site", &dir.join("site"))).await;
    let pipe = pipes::open_for_test(&factory, "site", "Website", &[], "", false);
    view.follow(&mut client, |view| shown(view, &pipe).is_some()).await;
    let site = std::net::TcpListener::bind("127.0.0.1:0").expect("a website");
    let port = site.local_addr().expect("its port").port();
    let ask = |port: u16| json!({"pipe": "Website", "port": port, "name": "Dashboard", "reason": "Matt wants Ana to see it."}).to_string();

    // Not an admin agent.
    let refused = call(&mut client, &mut view, "dev", "pipes_share", &ask(port)).await;
    assert!(refused.starts_with("Only an admin agent can ask to share a website"), "{refused}");
    // A pipe whose other side isn't pinned yet.
    let refused = call(&mut client, &mut view, "rivet", "pipes_share", &ask(port)).await;
    assert_eq!(refused, "The pipe's other side isn't pinned to a key yet, so it can't get a website. Nothing was asked.");
    factoryd::pipe_link::pin_for_test(&factory, &pipe, "peer-endpoint");
    // Ports that can't be shared, or where nothing answers.
    let refused = call(&mut client, &mut view, "rivet", "pipes_share", &ask(7421)).await;
    assert_eq!(refused, "Port 7421 is Claktory's own. Nothing was asked.");
    let nothing = std::net::TcpListener::bind("127.0.0.1:0").expect("a port").local_addr().expect("its port").port();
    let refused = call(&mut client, &mut view, "rivet", "pipes_share", &ask(nothing)).await;
    assert!(refused.starts_with(&format!("Nothing answers on port {nothing} on this computer")), "{refused}");
    // How long: 1 to 168 hours, never until it's stopped, which is the
    // person's.
    for hours in [json!(0), json!(169), json!("never"), json!(-24)] {
        let args = json!({"pipe": "Website", "port": port, "name": "Dashboard", "hours": hours, "reason": "Matt wants Ana to see it."});
        let refused = call(&mut client, &mut view, "rivet", "pipes_share", &args.to_string()).await;
        assert!(!refused.starts_with("Asked "), "{hours}: {refused}");
    }
    assert!(cards(&view).is_empty(), "a refusal made a request");

    // The card: what it exposes, in Claktory's words; nothing shared yet.
    let said = call(&mut client, &mut view, "rivet", "pipes_share", &ask(port)).await;
    assert!(said.starts_with("Asked "), "{said}");
    let (seq, card) = cards(&view).pop().expect("a card");
    assert_eq!(card.title, "Share a website with the pipe \"Website\"");
    // On this machine, by its name (W3).
    let expected = format!("Rivet asks to share the website at http://localhost:{port} on ");
    assert!(card.action_description.starts_with(&expected), "{}", card.action_description);
    let expected = format!(" with the pipe \"Website\" (Not verified), called \"Dashboard\" there. What listens on port {port} now: ");
    assert!(card.action_description.contains(&expected), "{}", card.action_description);
    assert!(!card.action_description.starts_with(&format!("Rivet asks to share the website at http://localhost:{port} on this computer")));
    assert!(card.action_description.contains(&format!("(process {}).", std::process::id())), "{}", card.action_description);
    assert!(
        card.action_description.ends_with(
            "While it's shared, the other side can use every page and action of that site, as a browser on this computer could. It stays shared for 24 hours, then stops by itself. You can change that or stop it sooner under Shared websites in the pipe's settings."
        ),
        "{}",
        card.action_description
    );
    assert_eq!(card.action_reason, "Matt wants Ana to see it.");
    assert!(view.workspace.pipe_shares.is_empty(), "nothing until they approve");

    // What listened there is gone: the card is void, and nothing is shared.
    drop(site);
    apply_request(&mut client, &mut view, Request::SelectAgent(v1::SelectAgent { agent_id: "rivet".into() })).await;
    match apply_request(&mut client, &mut view, answer(seq, false)).await {
        v1::server_message::Body::Error(error) => {
            assert_eq!(error.message, format!("Nothing answers on port {port} now, so there's nothing to share: dismiss this request."))
        }
        other => panic!("not refused: {other:?}"),
    }
    assert!(view.workspace.pipe_shares.is_empty());

    // A program Claktory can't name there (another user's, say): never
    // from a card: refused when asked, and void at Approve.
    let theirs = std::net::TcpListener::bind("127.0.0.1:0").expect("someone else's website");
    let unnamed = theirs.local_addr().expect("its port").port();
    pipes::shares::unnamed_listener_for_test(unnamed);
    let refused = call(&mut client, &mut view, "rivet", "pipes_share", &ask(unnamed)).await;
    assert_eq!(
        refused,
        format!(
            "Claktory can't tell which of the person's programs listens on port {unnamed}, so you can't ask to share it. They can share it themselves from the pipe's settings if it's theirs. Nothing was asked."
        )
    );
    let website = std::net::TcpListener::bind("127.0.0.1:0").expect("a website");
    let port = website.local_addr().expect("its port").port();
    call(&mut client, &mut view, "rivet", "pipes_share", &ask(port)).await;
    let (seq, _) = cards(&view).pop().expect("a card");
    pipes::shares::unnamed_listener_for_test(port);
    match apply_request(&mut client, &mut view, answer(seq, false)).await {
        v1::server_message::Body::Error(error) => {
            assert_eq!(error.message, format!("A different program listens on port {port} now: dismiss this request and ask again."))
        }
        other => panic!("not refused: {other:?}"),
    }
    assert!(view.workspace.pipe_shares.is_empty());

    // Another account on the other side now: refused.
    let website = std::net::TcpListener::bind("127.0.0.1:0").expect("a website");
    let port = website.local_addr().expect("its port").port();
    pipes::identity::vouch_for_test(&factory, true, Some(String::new()), |statement: &str, at: &str, _now: i64| {
        let [account, email, iat, key] = statement.split('|').collect::<Vec<_>>()[..] else { return None };
        let iat: i64 = iat.parse().ok()?;
        (key == at).then(|| (account.to_owned(), email.to_owned(), iat, iat + 7 * 24 * 3600))
    });
    let now = factoryd::transcript::now_ms() / 1000;
    pipes::identity::heard_for_test(&factory, &pipe, "peer-endpoint", &format!("a1|ana@example.com|{now}|peer-endpoint"));
    view.follow(&mut client, |view| shown(view, &pipe).and_then(|pipe| pipe.peer).is_some_and(|peer| peer.verified)).await;
    let said = call(&mut client, &mut view, "rivet", "pipes_share", &ask(port)).await;
    assert!(said.starts_with("Asked "), "{said}");
    let (_, card) = cards(&view).pop().expect("a card");
    assert!(card.action_description.contains("(Verified: ana@example.com)"), "{}", card.action_description);
    pipes::identity::heard_for_test(&factory, &pipe, "peer-endpoint", &format!("b1|bo@example.com|{}|peer-endpoint", now + 1));
    let refused = call(&mut client, &mut view, "rivet", "pipes_share", &ask(port)).await;
    assert!(refused.starts_with("The pipe's Claktory now belongs to a different account"), "{refused}");

    // Having read from outside.
    pipes::taint_for_test(&factory, "rivet", "browser:https://example.com");
    let refused = call(&mut client, &mut view, "rivet", "pipes_share", &ask(port)).await;
    assert!(refused.starts_with("Not while you've read"), "{refused}");
    assert!(view.workspace.pipe_shares.is_empty());
}
