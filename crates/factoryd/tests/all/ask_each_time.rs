//! Ask each time (threat model K1): a secret that asks the person for every
//! command that uses it, the approval bound to that exact command, once.

use crate::common::{Client, View, apply_request, create_agent, create_project, start, temp_dir};
use crate::pipes::{call, idle};
use factory_protocol::v1::{self, client_message, entry::Kind};

/// A project "Site" with Bo, given "AWS", which asks each time.
async fn setup() -> (std::path::PathBuf, Client, View, String) {
    let dir = temp_dir();
    let (addr, _factory) = start(&dir).await;
    let (mut client, workspace, _) = Client::connect(addr).await;
    let mut view = View { workspace, ..View::default() };
    apply_request(&mut client, &mut view, create_project("Site", &dir.join("site"))).await;
    apply_request(&mut client, &mut view, create_agent("site", "Bo", "")).await;
    let secret = v1::CreateSecret {
        name: "AWS".into(),
        description: "Deploys.".into(),
        entries: vec![v1::SecretEntry { variable: "AWS_SECRET".into(), value: "wJalrXUtnFEMI-K7MDENG".into(), file: false }],
        ask_each_time: true,
    };
    apply_request(&mut client, &mut view, client_message::Body::CreateSecret(secret)).await;
    let id = view.workspace.vault.clone().expect("vault").secrets[0].id.clone();
    let grant = v1::GrantSecret { id: id.clone(), agent_id: "bo".into(), proof: None };
    apply_request(&mut client, &mut view, client_message::Body::GrantSecret(grant)).await;
    (dir, client, view, id)
}

fn approvals(view: &View) -> Vec<(u64, v1::SecretApproval)> {
    view.entries
        .iter()
        .filter_map(|entry| match &entry.kind {
            Some(Kind::SecretApproval(approval)) => Some((entry.seq, approval.clone())),
            _ => None,
        })
        .collect()
}

async fn allow(client: &mut Client, view: &mut View, seq: u64) {
    let allow = v1::DecideSecretApproval { agent_id: "bo".into(), seq, allow: true, ..Default::default() };
    apply_request(client, view, client_message::Body::DecideSecretApproval(allow)).await;
    view.follow(client, |view| {
        view.entries.iter().any(|entry| {
            entry.seq == seq
                && matches!(&entry.kind, Some(Kind::SecretApproval(approval)) if approval.state() == v1::SecretApprovalState::Allowed)
        })
    })
    .await;
    view.follow(client, |view| idle(view, "bo")).await;
}

#[tokio::test]
async fn each_command_with_the_secret_waits_for_the_person_once() {
    let (_dir, mut client, mut view, _id) = setup().await;
    assert!(view.workspace.vault.clone().expect("vault").secrets[0].ask_each_time);
    let command = r#"{"command": "echo deployed", "secrets": ["AWS"]}"#;
    let refused = call(&mut client, &mut view, "bo", "bash", command).await;
    assert!(refused.starts_with("Not run: AWS asks the person each time it's used"), "{refused}");
    let (seq, card) = approvals(&view).pop().expect("a card");
    assert_eq!((card.secret_names.clone(), card.ask_each_time_names.clone()), (vec!["AWS".to_owned()], vec!["AWS".to_owned()]));
    assert!(card.pipe_names.is_empty() && !card.keeps_running);
    assert_eq!(card.command, "echo deployed");
    allow(&mut client, &mut view, seq).await;

    // Another command doesn't get it; that one does, once.
    let other = call(&mut client, &mut view, "bo", "bash", r#"{"command": "echo other", "secrets": ["AWS"]}"#).await;
    assert!(other.starts_with("Not run"), "{other}");
    assert!(call(&mut client, &mut view, "bo", "bash", command).await.starts_with("deployed"));
    assert!(call(&mut client, &mut view, "bo", "bash", command).await.starts_with("Not run"));

    // In the background it's another approval, which says it keeps running.
    let background = r#"{"command": "echo deployed", "secrets": ["AWS"], "run_in_background": true}"#;
    assert!(call(&mut client, &mut view, "bo", "bash", background).await.starts_with("Not run"));
    let (_, card) = approvals(&view).pop().expect("a card");
    assert!(card.keeps_running);
    let detached = r#"{"command": "nohup ./deploy.sh > log 2>&1 &", "secrets": ["AWS"]}"#;
    call(&mut client, &mut view, "bo", "bash", detached).await;
    assert!(approvals(&view).pop().expect("a card").1.keeps_running);
    let chained = r#"{"command": "make && ./deploy.sh 2>&1", "secrets": ["AWS"]}"#;
    call(&mut client, &mut view, "bo", "bash", chained).await;
    assert!(!approvals(&view).pop().expect("a card").1.keeps_running);
}

#[tokio::test]
async fn the_person_turns_it_off_and_on() {
    let (_dir, mut client, mut view, id) = setup().await;
    let off = v1::SetSecretAskEachTime { id: id.clone(), ask_each_time: false, proof: None };
    apply_request(&mut client, &mut view, client_message::Body::SetSecretAskEachTime(off)).await;
    view.follow(&mut client, |view| view.workspace.vault.as_ref().is_some_and(|vault| !vault.secrets[0].ask_each_time)).await;
    let command = r#"{"command": "echo deployed", "secrets": ["AWS"]}"#;
    assert!(call(&mut client, &mut view, "bo", "bash", command).await.starts_with("deployed"));

    let on = v1::SetSecretAskEachTime { id: id.clone(), ask_each_time: true, proof: None };
    apply_request(&mut client, &mut view, client_message::Body::SetSecretAskEachTime(on)).await;
    view.follow(&mut client, |view| view.workspace.vault.as_ref().is_some_and(|vault| vault.secrets[0].ask_each_time)).await;
    assert!(call(&mut client, &mut view, "bo", "bash", command).await.starts_with("Not run"));
}

#[tokio::test]
async fn the_persons_comment_reaches_the_agent_with_the_decision() {
    let (_dir, mut client, mut view, _id) = setup().await;
    call(&mut client, &mut view, "bo", "bash", r#"{"command": "echo deployed", "secrets": ["AWS"]}"#).await;
    let (seq, _) = approvals(&view).pop().expect("a card");
    let deny = v1::DecideSecretApproval { agent_id: "bo".into(), seq, allow: false, comment: " Use staging first. ".into(), ..Default::default() };
    apply_request(&mut client, &mut view, client_message::Body::DecideSecretApproval(deny)).await;
    let told = "The person didn't allow you to use AWS.\n\nThe person's note: Use staging first.";
    view.follow(&mut client, |view| view.entries.iter().any(|entry| matches!(&entry.kind, Some(Kind::Event(event)) if event.text == told)))
        .await;
}

/// Submit (#370): a comment instead of Allow is a reply. Nothing runs, and
/// the card shows the reply, kept apart from the pipe tables.
#[tokio::test]
async fn a_reply_runs_nothing_and_the_card_keeps_it() {
    let (dir, mut client, mut view, _id) = setup().await;
    call(&mut client, &mut view, "bo", "bash", r#"{"command": "echo deployed", "secrets": ["AWS"]}"#).await;
    let (seq, _) = approvals(&view).pop().expect("a card");
    let reply = |comment: &str| {
        client_message::Body::DecideSecretApproval(v1::DecideSecretApproval {
            agent_id: "bo".into(),
            seq,
            allow: true,
            comment: comment.into(),
            reply: true,
        })
    };
    let empty = apply_request(&mut client, &mut view, reply("")).await;
    assert!(matches!(empty, v1::server_message::Body::Error(_)), "{empty:?}");
    apply_request(&mut client, &mut view, reply("Deploy to staging first.")).await;
    let told = "The person didn't allow you to use AWS this time, and replied; nothing ran. They say:\n\nDeploy to staging first.";
    view.follow(&mut client, |view| view.entries.iter().any(|entry| matches!(&entry.kind, Some(Kind::Event(event)) if event.text == told)))
        .await;
    let card = approvals(&view).into_iter().find(|(at, _)| *at == seq).expect("the card").1;
    assert_eq!(
        (card.state(), card.replied, card.comment.as_str()),
        (v1::SecretApprovalState::Denied, true, "Deploy to staging first.")
    );
    // The same command still asks: nothing was allowed.
    let again = call(&mut client, &mut view, "bo", "bash", r#"{"command": "echo deployed", "secrets": ["AWS"]}"#).await;
    assert!(again.starts_with("Not run"), "{again}");
    // Kept apart from the pipe tables (Fugo): only decision_notes holds it.
    let db = rusqlite::Connection::open(crate::common::home(&dir).db_path()).expect("factory.db");
    let kept: String = db.query_row("SELECT comment FROM decision_notes WHERE kind = 'secret_use'", [], |row| row.get(0)).expect("kept");
    assert_eq!(kept, "Deploy to staging first.");
}
