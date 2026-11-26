//! Rivet's pipe cards (#471, pipes-rivet-stories.md, Fugo's rule C): new
//! words for a pipe shown old beside new, its liaison's spending limit
//! lowered at once or raised on a card, a closed pipe deleted on a card,
//! narrowing after reading from outside labelled so, and what pipes_list
//! counts. Claktory writes each card's words, approving does exactly what it
//! shows, and a card whose target changed first is refused.

use serde_json::json;

use crate::common::{Client, MODEL, View, apply_request};
use crate::pipes::{call, incoming, pipe_of, shown};
use crate::rivet_cards::{answer, cards, setup, told};
use factory_protocol::v1::{self, client_message::Body as Request, server_message::Body};
use factoryd::pipes;

/// The person is Matt, and agents run on the scripted model.
async fn matt(client: &mut Client, view: &mut View) {
    let settings = v1::UpdateSettings {
        user_name: Some("Matt".into()),
        default_model: Some(MODEL.into()),
        default_effort: Some("off".into()),
        ..Default::default()
    };
    apply_request(client, view, Request::UpdateSettings(settings)).await;
}

/// Approving the card at `seq`: the error it's refused with.
async fn refused(client: &mut Client, view: &mut View, seq: u64) -> String {
    apply_request(client, view, Request::SelectAgent(v1::SelectAgent { agent_id: "rivet".into() })).await;
    match apply_request(client, view, answer(seq, false)).await {
        Body::Error(error) => error.message,
        other => panic!("not refused: {other:?}"),
    }
}

fn lines(factory: &factoryd::factory::Factory, pipe: &str) -> Vec<String> {
    pipes::messages_for_test(factory, pipe).into_iter().map(|line| line.text).collect()
}

#[tokio::test]
async fn rivet_changes_a_pipes_words_only_on_a_card_that_shows_them_old_beside_new() {
    let (factory, mut client, mut view, pipe) = setup().await;
    matt(&mut client, &mut view).await;
    let args = json!({"pipe": "Website", "reason": "Matt wants a clearer name.", "name": "Site talk", "purpose": "Talk about the site.", "instructions": "Be brief."});
    let said = call(&mut client, &mut view, "rivet", "pipes_settings", &args.to_string()).await;
    assert!(said.starts_with("Asked "), "{said}");
    let (seq, card) = cards(&view).pop().expect("a card");
    assert_eq!(card.title, "Change the pipe \"Website\"");
    let shown_words = &card.action_description;
    assert!(
        shown_words
            .starts_with("Rivet asks to rename the pipe \"Website\" to \"Site talk\", change what it is for and change its instructions."),
        "{shown_words}"
    );
    assert!(shown_words.contains("Name (you and your agents see it): \"Website\" → \"Site talk\"."), "{shown_words}");
    assert!(
        shown_words
            .contains("What it's for (the other side keeps what it was told when you connected): \"testing\" → \"Talk about the site.\"."),
        "{shown_words}"
    );
    assert!(shown_words.contains("Instructions (only your agents see them): (none) → \"Be brief.\"."), "{shown_words}");
    assert!(!shown_words.contains("clearer"), "Rivet's reason stays apart: {shown_words}");
    assert_eq!(card.action_reason, "Matt wants a clearer name.");
    assert_eq!(pipe_of(&view, &pipe).name, "Website", "nothing until Matt approves");

    // Matt renames it himself meanwhile: the card is void.
    let renamed = v1::UpdatePipe { pipe_id: pipe.clone(), name: Some("Webby".into()), ..Default::default() };
    apply_request(&mut client, &mut view, Request::UpdatePipe(renamed)).await;
    let why = refused(&mut client, &mut view, seq).await;
    assert_eq!(why, "The pipe's name, what it's for or its instructions changed since this card was asked: dismiss it, and ask again.");
    assert_eq!(pipe_of(&view, &pipe).name, "Webby");
    apply_request(&mut client, &mut view, answer(seq, true)).await;

    // Asked again: approved, exactly that, and the pipe says so.
    let args = json!({"pipe": "Webby", "reason": "Matt wants a clearer name.", "name": "Site talk", "instructions": "Be brief."});
    call(&mut client, &mut view, "rivet", "pipes_settings", &args.to_string()).await;
    let (seq, card) = cards(&view).pop().expect("a card");
    assert!(card.action_description.contains("\"Webby\" → \"Site talk\""), "{}", card.action_description);
    assert!(!card.action_description.contains("What it's for"), "{}", card.action_description);
    apply_request(&mut client, &mut view, answer(seq, false)).await;
    told(&mut client, &mut view, "approved \"Change the pipe \"Webby\"\". Renamed the pipe \"Webby\" to \"Site talk\"").await;
    view.follow(&mut client, |view| pipe_of(view, &pipe).name == "Site talk").await;
    let now = pipe_of(&view, &pipe);
    assert_eq!((now.purpose.as_str(), now.prompt.as_str()), ("testing", "Be brief."));
    let said = lines(&factory, &pipe);
    assert!(
        said.iter().any(|line| line == "You renamed this pipe to \"Site talk\" and changed its instructions, on a card you approved."),
        "{said:?}"
    );

    // The same words as now: nothing to ask.
    let same =
        call(&mut client, &mut view, "rivet", "pipes_settings", r#"{"pipe": "Site talk", "reason": "x", "name": "Site talk"}"#).await;
    assert_eq!(same, "Say what to change. Nothing was changed.");

    // A review already as asked: nothing to say it turned on (Abbacchio
    // row 1232).
    let review = r#"{"pipe": "Site talk", "reason": "x", "review_incoming": true}"#;
    let said = call(&mut client, &mut view, "rivet", "pipes_settings", review).await;
    assert!(said.starts_with("Turned Review incoming on"), "{said}");
    let again = call(&mut client, &mut view, "rivet", "pipes_settings", review).await;
    assert_eq!(again, "Say what to change. Nothing was changed.");

    // Having read from outside: refused, and no card.
    pipes::taint_for_test(&factory, "rivet", "browser:https://example.com");
    let before = cards(&view).len();
    let tainted = call(&mut client, &mut view, "rivet", "pipes_settings", r#"{"pipe": "Site talk", "reason": "x", "name": "Other"}"#).await;
    assert!(tainted.starts_with("Not while you've read a page on https://example.com"), "{tainted}");
    assert!(tainted.contains("Narrowing a pipe still works"), "{tainted}");
    assert_eq!(cards(&view).len(), before);
}

#[tokio::test]
async fn rivet_lowers_a_liaisons_spending_limit_at_once_and_raises_it_only_on_a_card() {
    let (factory, mut client, mut view, pipe) = setup().await;
    matt(&mut client, &mut view).await;
    let refused_here =
        call(&mut client, &mut view, "rivet", "pipes_settings", r#"{"pipe": "Website", "reason": "x", "spend_cap_usd": 5}"#).await;
    assert_eq!(refused_here, "This pipe isn't led by a liaison agent, so it has no daily spending limit. Nothing was changed.");
    let add = v1::AddPipeLiaison { pipe_id: pipe.clone(), name: "Liaison".into() };
    apply_request(&mut client, &mut view, Request::AddPipeLiaison(add)).await;
    view.follow(&mut client, |view| shown(view, &pipe).is_some_and(|pipe| pipe.lead_id == "liaison")).await;
    let cap = |view: &View| view.agent("liaison").spend.expect("a liaison's spend").cap_usd;
    assert_eq!(cap(&view), 10.0);

    // Lower: at once, and the pipe says so.
    let said = call(
        &mut client,
        &mut view,
        "rivet",
        "pipes_settings",
        r#"{"pipe": "Website", "reason": "Matt wants it cheaper.", "spend_cap_usd": 5}"#,
    )
    .await;
    assert!(said.starts_with("Set the daily spending limit of the agent answering the pipe \"Website\" from $10 to $5."), "{said}");
    view.follow(&mut client, |view| cap(view) == 5.0).await;
    assert!(cards(&view).is_empty(), "no card for lowering it");
    assert!(
        lines(&factory, &pipe).contains(&"Rivet set the daily spending limit of the agent answering this pipe from $10 to $5.".to_owned())
    );

    // Raise: a card, void if the limit changes first.
    let raise = r#"{"pipe": "Website", "reason": "It needs more.", "spend_cap_usd": 20.5}"#;
    let said = call(&mut client, &mut view, "rivet", "pipes_settings", raise).await;
    assert!(said.starts_with("Asked "), "{said}");
    let (seq, card) = cards(&view).pop().expect("a card");
    assert!(
        card.action_description
            .starts_with("Rivet asks to set the daily spending limit of the agent answering the pipe \"Website\" from $5 to $20.50."),
        "{}",
        card.action_description
    );
    assert_eq!(cap(&view), 5.0);
    let set = v1::SetSpendCap { agent_id: "liaison".into(), cap_usd: 7.0, cap_tokens: 0 };
    apply_request(&mut client, &mut view, Request::SetSpendCap(set)).await;
    let why = refused(&mut client, &mut view, seq).await;
    assert_eq!(why, "The pipe's liaison or its spending limit changed since this card was asked: dismiss it, and ask again.");
    assert_eq!(cap(&view), 7.0);
    apply_request(&mut client, &mut view, answer(seq, true)).await;
    call(&mut client, &mut view, "rivet", "pipes_settings", raise).await;
    let (seq, _) = cards(&view).pop().expect("a card");
    apply_request(&mut client, &mut view, answer(seq, false)).await;
    told(&mut client, &mut view, "approved \"Change the pipe \"Website\"\". Set the daily spending limit").await;
    view.follow(&mut client, |view| cap(view) == 20.5).await;
    let said = lines(&factory, &pipe);
    assert!(
        said.contains(
            &"You set the daily spending limit of the agent answering this pipe from $7 to $20.50, on a card you approved.".to_owned()
        ),
        "{said:?}"
    );

    // Having read from outside, Rivet still narrows, labelled so; never
    // widens.
    pipes::taint_for_test(&factory, "rivet", "browser:https://example.com");
    let said =
        call(&mut client, &mut view, "rivet", "pipes_settings", r#"{"pipe": "Website", "reason": "Cheaper.", "spend_cap_usd": 4}"#).await;
    assert!(said.starts_with("Set the daily spending limit"), "{said}");
    view.follow(&mut client, |view| cap(view) == 4.0).await;
    let said = lines(&factory, &pipe);
    assert!(
        said.contains(
            &"Rivet, after it read from outside, set the daily spending limit of the agent answering this pipe from $20.50 to $4."
                .to_owned()
        ),
        "{said:?}"
    );
    let before = cards(&view).len();
    let tainted = call(&mut client, &mut view, "rivet", "pipes_settings", raise).await;
    assert!(tainted.starts_with("Not while you've read"), "{tainted}");
    assert_eq!((cards(&view).len(), cap(&view)), (before, 4.0));

    // Its conversation cleared, it widens again: letting its liaison run
    // commands, the card says what that means.
    let clear = v1::RunCommand { agent_id: "rivet".into(), name: "clear".into(), args: String::new() };
    apply_request(&mut client, &mut view, Request::RunCommand(clear)).await;
    view.follow(&mut client, |view| view.agent("rivet").tainted_by_pipes.is_empty()).await;
    let release = r#"{"pipe": "Website", "reason": "It needs to build.", "commands": ["Liaison"]}"#;
    call(&mut client, &mut view, "rivet", "pipes_settings", release).await;
    let (seq, card) = cards(&view).pop().expect("a card");
    assert!(
        card.action_description.ends_with(
            "Liaison is then no longer limited to the pipe: it can run commands, reach your files and use secrets on this computer."
        ),
        "{}",
        card.action_description
    );
    assert!(view.agent("liaison").quarantined);
    apply_request(&mut client, &mut view, answer(seq, false)).await;
    view.follow(&mut client, |view| !view.agent("liaison").quarantined).await;
    assert!(pipe_of(&view, &pipe).can_run_commands_ids.contains(&"liaison".to_owned()));
}

#[tokio::test]
async fn rivet_deletes_a_closed_pipe_only_on_a_card() {
    let (_factory, mut client, mut view, pipe) = setup().await;
    matt(&mut client, &mut view).await;
    let open = call(&mut client, &mut view, "rivet", "pipes_settings", r#"{"pipe": "Website", "reason": "x", "delete": true}"#).await;
    assert_eq!(open, "Only a closed pipe can be deleted: close it first (close: true). Nothing was changed.");
    let close = v1::SetPipeState { pipe_id: pipe.clone(), action: v1::PipeAction::Close.into() };
    apply_request(&mut client, &mut view, Request::SetPipeState(close)).await;
    view.follow(&mut client, |view| pipe_of(view, &pipe).state() == v1::PipeState::Closed).await;

    let mixed =
        call(&mut client, &mut view, "rivet", "pipes_settings", r#"{"pipe": "Website", "reason": "x", "delete": true, "name": "Old"}"#)
            .await;
    assert_eq!(mixed, "Delete a pipe on its own, with nothing else. Nothing was changed.");
    let other = call(&mut client, &mut view, "rivet", "pipes_settings", r#"{"pipe": "Website", "reason": "x", "pause": true}"#).await;
    assert_eq!(other, "That pipe is closed: it can only be deleted (delete: true). Nothing was changed.");

    let said = call(
        &mut client,
        &mut view,
        "rivet",
        "pipes_settings",
        r#"{"pipe": "Website", "reason": "Matt is done with it.", "delete": true}"#,
    )
    .await;
    assert!(said.starts_with("Asked "), "{said}");
    let (seq, card) = cards(&view).pop().expect("a card");
    assert!(card.action_description.starts_with("Rivet asks to delete the pipe \"Website\"."), "{}", card.action_description);
    assert!(
        card.action_description
            .ends_with("Deleting it removes it and its messages from Claktory. Its inbox and outbox stay in its folder."),
        "{}",
        card.action_description
    );
    assert!(shown(&view, &pipe).is_some(), "nothing until Matt approves");
    apply_request(&mut client, &mut view, answer(seq, false)).await;
    told(&mut client, &mut view, "approved \"Change the pipe \"Website\"\". Deleted the pipe \"Website\".").await;
    view.follow(&mut client, |view| shown(view, &pipe).is_none()).await;
}

/// Abbacchio's row 1196: a pipe the other side paused, only they resume;
/// Rivet is told so at once, with no card.
#[tokio::test]
async fn a_pipe_the_other_side_paused_is_never_a_card_to_resume() {
    let (factory, mut client, mut view, pipe) = setup().await;
    matt(&mut client, &mut view).await;
    pipes::peer_paused_for_test(&factory, &pipe, true);
    view.follow(&mut client, |view| pipe_of(view, &pipe).state() == v1::PipeState::Paused).await;
    let said = call(&mut client, &mut view, "rivet", "pipes_settings", r#"{"pipe": "Website", "reason": "x", "resume": true}"#).await;
    assert_eq!(said, "The other side paused it: only they can resume it. Nothing was changed.");
    assert!(cards(&view).is_empty());

    // Their loop breaker paused it: the person is told whose Claktory did.
    pipes::peer_paused_for_test(&factory, &pipe, false);
    pipes::peer_looped_for_test(&factory, &pipe, true);
    let resume = v1::SetPipeState { pipe_id: pipe.clone(), action: v1::PipeAction::Resume.into() };
    match apply_request(&mut client, &mut view, Request::SetPipeState(resume)).await {
        Body::Error(error) => {
            assert_eq!(error.message, "Ana's Claktory paused it, because the agents were going back and forth: only they can continue it.")
        }
        other => panic!("not refused: {other:?}"),
    }
}

/// #471 G11: pipes_list counts what waits on each pipe, never what it says.
#[tokio::test]
async fn pipes_list_counts_what_waits_for_the_person() {
    let (factory, mut client, mut view, pipe) = setup().await;
    matt(&mut client, &mut view).await;
    let review = v1::UpdatePipe { pipe_id: pipe.clone(), review_incoming: Some(true), review_outgoing: Some(true), ..Default::default() };
    apply_request(&mut client, &mut view, Request::UpdatePipe(review)).await;
    for number in 1..=2 {
        pipes::receive_for_test(&factory, &pipe, incoming(&format!("m{number}"), &format!("SECRET {number}"))).expect("received");
    }
    let held = call(&mut client, &mut view, "lea", "pipe_send", r#"{"text": "OUTGOING words"}"#).await;
    assert!(!held.is_empty());
    let list = call(&mut client, &mut view, "rivet", "pipes_list", "{}").await;
    assert!(list.contains(": open; 3 messages wait for Matt (1 to send, 2 to let in)"), "{list}");
    assert!(!list.contains("SECRET") && !list.contains("OUTGOING"), "{list}");

    // Paused by the agents' looping: Rivet can ask to continue it.
    for _ in 0..3 {
        pipes::count_for_loop_for_test(&factory, &pipe, "their_agent", "The same message, again and again, for the third time now.");
    }
    view.follow(&mut client, |view| pipe_of(view, &pipe).state() == v1::PipeState::Paused).await;
    let list = call(&mut client, &mut view, "rivet", "pipes_list", "{}").await;
    assert!(
        list.contains(
            "paused: the agents talked on their own; Matt can continue it, or you can ask them to on a card (pipes_settings resume)"
        ),
        "{list}"
    );
}

/// G4: Rivet stops a website the person shares with a pipe at once, after
/// reading from outside too, labelled so; only on that pipe, and only the
/// person's websites shared with it, which pipes_list names.
#[tokio::test]
async fn rivet_stops_sharing_a_website_with_a_pipe_at_once() {
    let (factory, mut client, mut view, pipe) = setup().await;
    matt(&mut client, &mut view).await;
    let other = pipes::open_for_test(&factory, "site", "Partners", &["cy"], "cy", false);
    view.follow(&mut client, |view| shown(view, &other).is_some()).await;
    factoryd::pipe_link::pin_for_test(&factory, &pipe, "peer-endpoint");
    factoryd::pipe_link::pin_for_test(&factory, &other, "other-endpoint");
    let share = |port: u32, name: &str, pipes: Vec<String>| {
        Request::SharePipeWebsite(v1::SharePipeWebsite { port, name: name.into(), pipe_ids: pipes, ..Default::default() })
    };
    apply_request(&mut client, &mut view, share(8123, "Dashboard", vec![pipe.clone()])).await;
    apply_request(&mut client, &mut view, share(8124, "Docs", vec![pipe.clone(), other.clone()])).await;
    view.follow(&mut client, |view| view.workspace.pipe_shares.len() == 2).await;

    let list = call(&mut client, &mut view, "rivet", "pipes_list", "{}").await;
    let line = list.lines().find(|line| line.contains("\"Website\"")).expect("its line");
    assert!(line.contains("; Matt's websites shared with it: ") && line.contains("\"Dashboard\"") && line.contains("\"Docs\""), "{list}");

    // A website not shared with it: refused, nothing changed.
    let args = json!({"pipe": "Partners", "reason": "Matt asked.", "stop_sharing": ["Dashboard"]});
    let refused = call(&mut client, &mut view, "rivet", "pipes_settings", &args.to_string()).await;
    assert_eq!(refused, "No website \"Dashboard\" is shared with that pipe: it gets \"Docs\". Nothing was changed.");

    // After reading from outside: still at once, labelled so, no card.
    pipes::taint_for_test(&factory, "rivet", "browser:https://example.com");
    let args = json!({"pipe": "Website", "reason": "Matt asked.", "stop_sharing": ["dashboard", "Docs"]});
    let said = call(&mut client, &mut view, "rivet", "pipes_settings", &args.to_string()).await;
    assert!(said.starts_with("Stopped sharing ") && said.contains("\"Dashboard\"") && said.contains("\"Docs\""), "{said}");
    assert!(cards(&view).is_empty(), "a narrowing made a card");
    let said = lines(&factory, &pipe);
    for name in ["Dashboard", "Docs"] {
        let line = format!("Rivet, after it read from outside, stopped sharing {name} with this pipe.");
        assert!(said.contains(&line), "{said:?}");
    }
    // Dashboard reached only this pipe: it's over. Docs stays with Partners.
    view.follow(&mut client, |view| view.workspace.pipe_shares.len() == 1).await;
    let docs = &view.workspace.pipe_shares[0];
    assert_eq!((docs.name.as_str(), docs.pipe_ids.clone()), ("Docs", vec![other.clone()]));

    let args = json!({"pipe": "Website", "reason": "Matt asked.", "stop_sharing": ["Docs"]});
    let refused = call(&mut client, &mut view, "rivet", "pipes_settings", &args.to_string()).await;
    assert_eq!(refused, "No website \"Docs\" is shared with that pipe: none of Matt's websites is shared with it. Nothing was changed.");
}
