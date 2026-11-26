//! Rivet's card to accept a change in who's at a pipe's other end (#471
//! G7; Fugo 34521): it never holds who that is, only the person's
//! AcceptPipeIdentity from it accepts the change, its comment goes to the
//! pipe's lead, and it's void once the change is accepted in the pipe or
//! becomes another.

use serde_json::json;

use crate::common::{Client, MODEL, View, apply_request};
use crate::pipes::{call, pipe_of};
use crate::rivet_cards::{answer, cards, setup, told};
use factory_protocol::v1::{self, client_message::Body as Request, entry::Kind, server_message::Body};
use factoryd::pipes;

/// The person is Matt; the account service vouches for statements
/// "account|email|iat|key".
async fn vouching(client: &mut Client, view: &mut View, factory: &std::sync::Arc<factoryd::factory::Factory>) {
    let settings = v1::UpdateSettings {
        user_name: Some("Matt".into()),
        default_model: Some(MODEL.into()),
        default_effort: Some("off".into()),
        ..Default::default()
    };
    apply_request(client, view, Request::UpdateSettings(settings)).await;
    pipes::identity::vouch_for_test(factory, true, Some(String::new()), |statement: &str, at: &str, _now: i64| {
        let [account, email, iat, key] = statement.split('|').collect::<Vec<_>>()[..] else { return None };
        let iat: i64 = iat.parse().ok()?;
        (key == at).then(|| (account.to_owned(), email.to_owned(), iat, iat + 7 * 24 * 3600))
    });
}

/// The other side presents `account` with `email`, `later` seconds on.
fn presents(factory: &std::sync::Arc<factoryd::factory::Factory>, pipe: &str, account: &str, email: &str, later: i64) {
    let now = factoryd::transcript::now_ms() / 1000 + later;
    pipes::identity::heard_for_test(factory, pipe, "peer-endpoint", &format!("{account}|{email}|{now}|peer-endpoint"));
}

fn change(view: &View, pipe: &str) -> v1::PipeIdentityChange {
    pipe_of(view, pipe).peer.map(|peer| peer.identity_change()).unwrap_or_default()
}

fn accept(pipe: &str, card: &v1::AskPerson) -> Request {
    Request::AcceptPipeIdentity(v1::AcceptPipeIdentity { pipe_id: pipe.into(), card_id: card.id.clone() })
}

#[tokio::test]
async fn rivet_asks_to_trust_another_account_only_on_a_card_that_never_says_who() {
    let (factory, mut client, mut view, pipe) = setup().await;
    vouching(&mut client, &mut view, &factory).await;
    presents(&factory, &pipe, "a1", "ana@example.com", 0);
    let args = json!({"pipe": "Website", "reason": "The pipe says Ana's account changed."}).to_string();
    let none = call(&mut client, &mut view, "rivet", "pipes_trust", &args).await;
    assert!(none.starts_with("Who's at the other end of that pipe hasn't changed since Matt last accepted it"), "{none}");

    presents(&factory, &pipe, "b1", "SENTINEL-NEW@example.com", 1);
    view.follow(&mut client, |view| change(view, &pipe) == v1::PipeIdentityChange::AccountChanged).await;
    let said = call(&mut client, &mut view, "rivet", "pipes_trust", &args).await;
    assert!(said.starts_with("Asked Matt: their card shows who the other side was and is now, which you never see"), "{said}");
    let (seq, card) = cards(&view).pop().expect("a card");
    assert_eq!(card.title, "Trust the Claktory at the other end of the pipe \"Website\"");
    assert!(
        card.action_description.contains("Check with the other person yourself that this change is theirs"),
        "{}",
        card.action_description
    );
    assert_eq!(card.action_reason, "The pipe says Ana's account changed.");
    let proposal = card.pipe_trust.clone().expect("the card's pipe");
    assert_eq!((proposal.pipe_id.as_str(), proposal.identity_change()), (pipe.as_str(), v1::PipeIdentityChange::AccountChanged));
    let again = call(&mut client, &mut view, "rivet", "pipes_trust", &args).await;
    assert!(again.contains("is still waiting for Matt"), "{again}");

    // Approving the card alone is refused.
    apply_request(&mut client, &mut view, Request::SelectAgent(v1::SelectAgent { agent_id: "rivet".into() })).await;
    match apply_request(&mut client, &mut view, answer(seq, false)).await {
        Body::Error(error) => assert_eq!(error.message, "Accept it on the card, or in the pipe."),
        other => panic!("not refused: {other:?}"),
    }
    // Accepted from the card: Rivet hears only that.
    let accepted = apply_request(&mut client, &mut view, accept(&pipe, &card)).await;
    assert!(!matches!(accepted, Body::Error(_)), "{accepted:?}");
    view.follow(&mut client, |view| change(view, &pipe) == v1::PipeIdentityChange::None).await;
    let event = told(&mut client, &mut view, "Matt accepted the change").await;
    assert_eq!(event, "Matt accepted the change on the pipe \"Website\".");
    assert_eq!(cards(&view).pop().expect("the card").1.state(), v1::AskPersonState::Answered);

    // Who it is never reaches Rivet, nor the card's row.
    let mut seen = format!("{:?}", view.entries);
    let db = rusqlite::Connection::open(factory.home().db_path()).expect("factory.db");
    let rows: Vec<String> = db
        .prepare("SELECT title || body || action || action_description || action_reason FROM asks")
        .expect("asks")
        .query_map([], |row| row.get(0))
        .expect("rows")
        .collect::<Result<_, _>>()
        .expect("read");
    seen.push_str(&rows.join("\n"));
    for sentinel in ["SENTINEL-NEW", "ana@example.com", "b1|", "peer-endpoint"] {
        assert!(!seen.contains(sentinel), "{sentinel} reached Rivet or the card");
    }
}

#[tokio::test]
async fn a_trust_card_is_void_once_the_change_moves_on_and_its_comment_goes_to_the_lead() {
    let (factory, mut client, mut view, pipe) = setup().await;
    vouching(&mut client, &mut view, &factory).await;
    presents(&factory, &pipe, "a1", "ana@example.com", 0);
    presents(&factory, &pipe, "b1", "bo@example.com", 1);
    view.follow(&mut client, |view| change(view, &pipe) == v1::PipeIdentityChange::AccountChanged).await;
    let args = json!({"pipe": "Website", "reason": "Trust it."}).to_string();
    call(&mut client, &mut view, "rivet", "pipes_trust", &args).await;
    let (_, card) = cards(&view).pop().expect("a card");

    // Another account again: the same kind of change, but not the one it
    // was asked for. Void, and Approve is refused.
    presents(&factory, &pipe, "c1", "cy@example.com", 2);
    view.follow(&mut client, |view| cards(view).pop().is_some_and(|(_, card)| card.withdrawn)).await;
    let (_, withdrawn) = cards(&view).pop().expect("the card");
    assert_eq!(withdrawn.withdraw_reason, "This card is void: who's at the other end of the pipe changed again since it was asked.");
    match apply_request(&mut client, &mut view, accept(&pipe, &card)).await {
        Body::Error(error) => assert!(error.message.starts_with("This card no longer waits for you"), "{}", error.message),
        other => panic!("not refused: {other:?}"),
    }
    assert_eq!(change(&view, &pipe), v1::PipeIdentityChange::AccountChanged);

    // Asked again, then accepted in the pipe itself: void.
    call(&mut client, &mut view, "rivet", "pipes_trust", &args).await;
    let plain = v1::AcceptPipeIdentity { pipe_id: pipe.clone(), ..Default::default() };
    apply_request(&mut client, &mut view, Request::AcceptPipeIdentity(plain)).await;
    view.follow(&mut client, |view| cards(view).iter().filter(|(_, card)| card.withdrawn).count() == 2).await;
    assert_eq!(cards(&view).pop().expect("the card").1.withdraw_reason, "This card is void: the change was accepted in the pipe.");

    // Dismissed with a comment: the pipe's lead gets it, Rivet only that
    // it was dismissed.
    presents(&factory, &pipe, "d1", "di@example.com", 3);
    view.follow(&mut client, |view| change(view, &pipe) == v1::PipeIdentityChange::AccountChanged).await;
    call(&mut client, &mut view, "rivet", "pipes_trust", &args).await;
    let (seq, _) = cards(&view).pop().expect("a card");
    let dismiss = Request::AnswerAsk(v1::AnswerAsk {
        agent_id: "rivet".into(),
        seq,
        dismiss: true,
        text: "SENTINEL-COMMENT".into(),
        ..Default::default()
    });
    apply_request(&mut client, &mut view, Request::SelectAgent(v1::SelectAgent { agent_id: "rivet".into() })).await;
    apply_request(&mut client, &mut view, dismiss).await;
    let event = told(&mut client, &mut view, "dismissed").await;
    assert!(!event.contains("SENTINEL-COMMENT"), "{event}");
    apply_request(&mut client, &mut view, Request::SelectAgent(v1::SelectAgent { agent_id: "lea".into() })).await;
    view.follow(&mut client, |view| {
        view.entries.iter().any(|entry| matches!(&entry.kind, Some(Kind::Event(event)) if event.text.contains("SENTINEL-COMMENT")))
    })
    .await;

    // Signed out there (or signing in again): void, saying so; then back
    // to the account accepted (Cy's) with a card waiting: void, saying that
    // (rows 1355 and 1356).
    call(&mut client, &mut view, "rivet", "pipes_trust", &args).await;
    pipes::identity::heard_for_test(&factory, &pipe, "peer-endpoint", "");
    view.follow(&mut client, |view| cards(view).iter().filter(|(_, card)| card.withdrawn).count() == 3).await;
    assert_eq!(
        cards(&view).pop().expect("the card").1.withdraw_reason,
        "This card is void: the other side's Claktory no longer says which account it's signed in to."
    );
    call(&mut client, &mut view, "rivet", "pipes_trust", &args).await;
    presents(&factory, &pipe, "c1", "cy@example.com", 5);
    view.follow(&mut client, |view| cards(view).iter().filter(|(_, card)| card.withdrawn).count() == 4).await;
    assert_eq!(
        cards(&view).pop().expect("the card").1.withdraw_reason,
        "This card is void: the other side signs in with the account you accepted again."
    );
    assert_eq!(change(&view, &pipe), v1::PipeIdentityChange::None);
    presents(&factory, &pipe, "e1", "eve@example.com", 6);
    view.follow(&mut client, |view| change(view, &pipe) == v1::PipeIdentityChange::AccountChanged).await;

    // Only an admin agent that hasn't read from outside.
    pipes::taint_for_test(&factory, "rivet", "browser:https://example.com");
    let tainted = call(&mut client, &mut view, "rivet", "pipes_trust", &args).await;
    assert!(tainted.starts_with("Not while you've read"), "{tainted}");
}
