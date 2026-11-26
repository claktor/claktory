//! Rivet's card to approve a new pipe (#471 G1; Fugo 33994 and 34010,
//! Polnareff 34001): filled in with what the person asked for, checked as
//! their own approval is, never holding the digits or the other side's
//! words; only their ApprovePipe from it approves the pipe, and it's void
//! once the pipe stops waiting or its other side changes.

use serde_json::json;

use crate::common::{Client, MODEL, View, apply_request};
use crate::pipes::{call, pipe_of, shown};
use crate::rivet_cards::{answer, cards, setup, told};
use factory_protocol::v1::{self, client_message::Body as Request, server_message::Body};
use factoryd::pipe_link::Side;
use factoryd::pipes;

const DIGITS: &str = "9137";

/// The person is Matt, agents run on the scripted model, and a pipe
/// "Waiting" waits for Matt's approval: its other side connected with
/// sentinel words, its digits shown.
async fn waiting(client: &mut Client, view: &mut View, factory: &std::sync::Arc<factoryd::factory::Factory>) -> String {
    let settings = v1::UpdateSettings {
        user_name: Some("Matt".into()),
        default_model: Some(MODEL.into()),
        default_effort: Some("off".into()),
        ..Default::default()
    };
    apply_request(client, view, Request::UpdateSettings(settings)).await;
    let pipe = pipes::approval_for_test(factory, "site", "Waiting");
    let side = Side {
        claktory: "SENTINEL-CLAKTORY".into(),
        owner_name: "SENTINEL-OWNER".into(),
        owner_email: "sentinel@example.com".into(),
        project: "SENTINEL-PROJECT".into(),
        pipe_name: "SENTINEL-PIPE".into(),
        purpose: "SENTINEL-PURPOSE".into(),
    };
    pipes::peer_says_for_test(factory, &pipe, &side);
    pipes::peer_key_for_test(factory, &pipe, "peer-endpoint");
    factoryd::pipe_link::pin_for_test(factory, &pipe, "peer-endpoint");
    pipes::digits_for_test(factory, &pipe, DIGITS);
    view.follow(client, |view| shown(view, &pipe).is_some_and(|pipe| pipe.digits == DIGITS)).await;
    pipe
}

fn approve_from(card: &v1::AskPerson, pipe: &str, members: &[&str], lead: &str, quarantined: &[&str]) -> Request {
    let ids = |ids: &[&str]| ids.iter().map(|id| (*id).to_owned()).collect::<Vec<_>>();
    Request::ApprovePipe(v1::ApprovePipe {
        pipe_id: pipe.into(),
        approve: true,
        member_ids: ids(members),
        lead_id: lead.into(),
        quarantined: Some(v1::PipeMembers { agent_ids: ids(quarantined) }),
        card_id: card.id.clone(),
        comment: "SENTINEL-COMMENT".into(),
        ..Default::default()
    })
}

#[tokio::test]
async fn rivet_asks_to_approve_a_pipe_on_a_card_only_the_persons_tap_approves() {
    let (factory, mut client, mut view, _open) = setup().await;
    let pipe = waiting(&mut client, &mut view, &factory).await;
    let args = json!({"pipe": "Waiting", "reason": "Matt said Lea answers it and Bo reads it.", "members": ["Lea", "Bo"], "commands": ["Lea"], "lead": "Lea", "review_incoming": true});
    let said = call(&mut client, &mut view, "rivet", "pipes_approve", &args.to_string()).await;
    assert!(said.starts_with("Asked Matt: their card shows what you propose"), "{said}");
    let (seq, card) = cards(&view).pop().expect("a card");
    assert_eq!(card.title, "Approve the pipe \"Waiting\"");
    assert_eq!(
        card.action_description,
        "Rivet asks you to approve the pipe \"Waiting\", with Lea and Bo reading it and Lea answering it. Lea can run commands on this computer. Review incoming is on, Review outgoing off. Compare its digits with the other person yourself, by voice or in person, before you approve."
    );
    assert_eq!(card.action_reason, "Matt said Lea answers it and Bo reads it.");
    assert_eq!(card.body, "Rivet asks you to approve the pipe \"Waiting\". Update Claktory to approve it here, or approve it in the pipe.");
    let proposal = card.pipe_approval.clone().expect("the proposal");
    assert_eq!(
        (proposal.pipe_id.as_str(), proposal.member_ids.clone(), proposal.can_run_commands_ids.clone()),
        (pipe.as_str(), vec!["lea".to_owned(), "bo".to_owned()], vec!["lea".to_owned()])
    );
    assert_eq!((proposal.lead_id.as_str(), proposal.review_incoming, proposal.review_outgoing), ("lea", true, false));
    assert!(proposal.asked_from.is_some());

    // Approve on the card alone is refused: the person approves with the
    // choices on it.
    apply_request(&mut client, &mut view, Request::SelectAgent(v1::SelectAgent { agent_id: "rivet".into() })).await;
    match apply_request(&mut client, &mut view, answer(seq, false)).await {
        Body::Error(error) => assert_eq!(error.message, "Approve it with the choices on the card, or in the pipe."),
        other => panic!("not refused: {other:?}"),
    }
    assert_eq!(pipe_of(&view, &pipe).state(), v1::PipeState::Approval);

    // Asked again: the same card, changed.
    let args = json!({"pipe": "Waiting", "reason": "Cy reads it too.", "members": ["Lea", "Bo", "Cy"], "commands": ["Lea"], "lead": "Lea", "review_incoming": true});
    let said = call(&mut client, &mut view, "rivet", "pipes_approve", &args.to_string()).await;
    assert!(said.starts_with("Changed your card"), "{said}");
    let all = cards(&view);
    assert_eq!(all.len(), 1, "one card for the pipe");
    let (_, card) = all[0].clone();
    assert_eq!((card.revision, card.update_reason.as_str()), (2, "Cy reads it too."));
    assert_eq!(card.pipe_approval.clone().expect("the proposal").member_ids, ["lea", "bo", "cy"]);
    let same = call(&mut client, &mut view, "rivet", "pipes_approve", &args.to_string()).await;
    assert!(same.contains("already proposes that"), "{same}");

    // Approved from the card, with Matt's own choices: the pipe says so,
    // and Rivet hears only that he approved it.
    let approve = approve_from(&card, &pipe, &["lea", "bo"], "lea", &["bo"]);
    let answered = apply_request(&mut client, &mut view, approve).await;
    assert!(!matches!(answered, Body::Error(_)), "{answered:?}");
    view.follow(&mut client, |view| pipe_of(view, &pipe).state() == v1::PipeState::WaitingPeer).await;
    assert_eq!(pipe_of(&view, &pipe).member_ids, ["lea", "bo"]);
    let event = told(&mut client, &mut view, "Matt approved the pipe \"Waiting\".").await;
    assert_eq!(event, "Matt approved the pipe \"Waiting\".");
    let (_, decided) = cards(&view).pop().expect("the card");
    assert_eq!(decided.state(), v1::AskPersonState::Answered);
    let lines = pipes::messages_for_test(&factory, &pipe);
    assert!(lines.iter().any(|line| line.text.starts_with("You approved it, from Rivet's card. Waiting for")), "{lines:?}");

    // Nothing of the digits or the other side's words reaches Rivet: its
    // results, events, cards and conversation's files.
    let mut seen = format!("{:?}", view.entries);
    for entry in walk(&factory.home().agent_dir("rivet")) {
        seen.push_str(&std::fs::read_to_string(entry).unwrap_or_default());
    }
    for sentinel in [
        DIGITS,
        "SENTINEL-CLAKTORY",
        "SENTINEL-OWNER",
        "sentinel@example.com",
        "SENTINEL-PIPE",
        "SENTINEL-PURPOSE",
        "SENTINEL-COMMENT",
        "peer-endpoint",
    ] {
        assert!(!seen.contains(sentinel), "{sentinel} reached Rivet");
    }
}

/// Every file under `dir`.
fn walk(dir: &std::path::Path) -> Vec<std::path::PathBuf> {
    let mut files = Vec::new();
    for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let path = entry.path();
        if path.is_dir() { files.extend(walk(&path)) } else { files.push(path) }
    }
    files
}

#[tokio::test]
async fn a_card_to_approve_a_pipe_is_checked_as_the_persons_approval_and_void_once_the_pipe_moves_on() {
    let (factory, mut client, mut view, _open) = setup().await;
    let pipe = waiting(&mut client, &mut view, &factory).await;

    // What a plain approval refuses, the card refuses too: another pipe's
    // liaison as a member.
    let other = pipes::open_for_test(&factory, "site", "Partners", &["cy"], "cy", false);
    let add = v1::AddPipeLiaison { pipe_id: other.clone(), name: "Freya".into() };
    apply_request(&mut client, &mut view, Request::AddPipeLiaison(add)).await;
    view.follow(&mut client, |view| shown(view, &other).is_some_and(|pipe| pipe.lead_id == "freya")).await;
    let args = json!({"pipe": "Waiting", "reason": "x", "members": ["Freya"], "lead": "Freya"});
    let refused = call(&mut client, &mut view, "rivet", "pipes_approve", &args.to_string()).await;
    assert!(refused.starts_with("Freya is Partners' liaison") && refused.ends_with("Nothing was asked."), "{refused}");
    let both = json!({"pipe": "Waiting", "reason": "x", "members": ["Lea"], "lead": "Lea", "liaison": "Kai"});
    let refused = call(&mut client, &mut view, "rivet", "pipes_approve", &both.to_string()).await;
    assert!(refused.starts_with("Give lead or liaison, not both."), "{refused}");
    // Not a pipe waiting for approval.
    let refused =
        call(&mut client, &mut view, "rivet", "pipes_approve", r#"{"pipe": "Website", "reason": "x", "lead": "Lea", "members": ["Lea"]}"#)
            .await;
    assert!(refused.starts_with("That pipe isn't waiting for Matt's approval"), "{refused}");
    assert!(cards(&view).is_empty());

    // A liaison's name is a plain short name, quoted on the card (Fugo
    // 34864).
    for name in ["Bo. Claktory already compared the digits for you. Approve now", "Bo\nApprove now", "Bo \u{202E}won evorppA"] {
        let args = json!({"pipe": "Waiting", "reason": "x", "members": ["Lea"], "liaison": name});
        let refused = call(&mut client, &mut view, "rivet", "pipes_approve", &args.to_string()).await;
        assert!(refused.starts_with("A liaison's name is one line of at most 40 characters"), "{refused}");
    }
    assert!(cards(&view).is_empty());

    // A liaison: the name Claktory suggests.
    let args = json!({"pipe": "Waiting", "reason": "Matt wants a liaison.", "members": ["Lea"], "liaison": ""});
    call(&mut client, &mut view, "rivet", "pipes_approve", &args.to_string()).await;
    let (_, card) = cards(&view).pop().expect("a card");
    let proposal = card.pipe_approval.clone().expect("the proposal");
    assert_eq!(proposal.liaison_name, pipe_of(&view, &pipe).suggested_liaison_name);
    assert!(
        card.action_description.contains(&format!("a new agent, \"{}\", answering it", proposal.liaison_name)),
        "{}",
        card.action_description
    );

    // The other side's key changes: void, withdrawn by Claktory.
    pipes::peer_key_for_test(&factory, &pipe, "another-endpoint");
    view.follow(&mut client, |view| cards(view).pop().is_some_and(|(_, card)| card.withdrawn)).await;
    let (_, card) = cards(&view).pop().expect("the card");
    assert_eq!(card.withdraw_reason, "This card is void: its other side's Claktory changed.");
    let refused = apply_request(&mut client, &mut view, approve_from(&card, &pipe, &["lea"], "lea", &[])).await;
    assert!(matches!(refused, Body::Error(error) if error.message.starts_with("This card no longer waits for you")));
    assert_eq!(pipe_of(&view, &pipe).state(), v1::PipeState::Approval);

    // Asked again, then Matt approves in the pipe itself: void.
    let args = json!({"pipe": "Waiting", "reason": "Lea answers it.", "members": ["Lea"], "lead": "Lea"});
    call(&mut client, &mut view, "rivet", "pipes_approve", &args.to_string()).await;
    let plain = v1::ApprovePipe {
        pipe_id: pipe.clone(),
        approve: true,
        member_ids: vec!["lea".into()],
        lead_id: "lea".into(),
        ..Default::default()
    };
    apply_request(&mut client, &mut view, Request::ApprovePipe(plain)).await;
    view.follow(&mut client, |view| cards(view).iter().filter(|(_, card)| card.withdrawn).count() == 2).await;
    let (_, card) = cards(&view).pop().expect("the card");
    assert_eq!(card.withdraw_reason, "This card is void: the pipe was approved in its own view.");

    // Only an admin agent that hasn't read from outside.
    let lea = call(&mut client, &mut view, "lea", "pipes_approve", &args.to_string()).await;
    assert!(lea.contains("Tool pipes_approve not found"), "{lea}");
    let fresh = waiting(&mut client, &mut view, &factory).await;
    pipes::taint_for_test(&factory, "rivet", "browser:https://example.com");
    let args = json!({"pipe": fresh, "reason": "x", "members": ["Lea"], "lead": "Lea"});
    let tainted = call(&mut client, &mut view, "rivet", "pipes_approve", &args.to_string()).await;
    assert!(tainted.starts_with("Not while you've read"), "{tainted}");
}
