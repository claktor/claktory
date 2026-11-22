//! Verified pipes (Fugo's list): whose Claktory the other side is, as the
//! account service vouches, pinned with the pipe. Statements here are a
//! test's ("account|email|iat|exp|endpoint"), checked the way the real
//! verifier answers: what doesn't check is None.

use crate::common::apply_request;
use crate::pipes::{call, incoming, setup, shown};
use factory_protocol::v1::{self, PipeIdentityChange as Change, client_message};
use factoryd::pipes::{self, identity};

const PEER: &str = "peer-endpoint";

fn statement(account: &str, email: &str, iat: i64, exp: i64) -> String {
    format!("{account}|{email}|{iat}|{exp}|{PEER}")
}

/// Checks a test statement: for this endpoint, unexpired, and not "forged".
fn vouch(factory: &factoryd::factory::Factory, can_check: bool) {
    identity::vouch_for_test(factory, can_check, Some(String::new()), |statement: &str, endpoint: &str, now: i64| {
        let parts: Vec<&str> = statement.split('|').collect();
        let [account, email, iat, exp, key] = parts[..] else { return None };
        let (iat, exp): (i64, i64) = (iat.parse().ok()?, exp.parse().ok()?);
        (key == endpoint && account != "forged" && now <= exp).then(|| (account.to_owned(), email.to_owned(), iat, exp))
    });
}

fn now() -> i64 {
    factoryd::transcript::now_ms() / 1000
}

fn peer_of(view: &crate::common::View, pipe: &str) -> (bool, String, Change, String) {
    let peer = shown(view, pipe).and_then(|pipe| pipe.peer).expect("its peer");
    (peer.verified, peer.verified_email.clone(), peer.identity_change(), peer.previous_email)
}

fn state_of(factory: &factoryd::factory::Factory, pipe: &str, text: &str) -> v1::PipeMessageState {
    pipes::messages_for_test(factory, pipe).into_iter().find(|message| message.text == text).expect("the message").state()
}

#[tokio::test]
async fn a_pipe_is_verified_by_what_this_side_checks_and_a_change_of_account_holds_it() {
    let (_dir, factory, mut client, mut view, pipe) = setup(false).await;
    vouch(&factory, true);
    let (t, week) = (now(), 7 * 24 * 3600);
    let ana = |iat: i64| statement("a1", "ana@example.com", iat, iat + week);
    let hear = |identity: &str| identity::heard_for_test(&factory, &pipe, PEER, identity);
    view.follow(&mut client, |view| shown(view, &pipe).is_some_and(|pipe| pipe.peer.is_some())).await;

    // Nothing yet: not verified. What doesn't check changes nothing: for
    // another endpoint (relayed from a third Claktory), forged, expired, too
    // big.
    assert_eq!(peer_of(&view, &pipe), (false, String::new(), Change::None, String::new()));
    hear(&format!("a1|ana@example.com|{t}|{}|another-endpoint", t + week));
    hear(&statement("forged", "ana@example.com", t, t + week));
    hear(&statement("a1", "ana@example.com", t - 2 * week, t - week));
    hear(&format!("{}{}", ana(t), " ".repeat(5000)));
    apply_request(&mut client, &mut view, crate::pipes::select("lea")).await;
    assert!(!peer_of(&view, &pipe).0);

    // The first that checks is pinned as it is.
    hear(&ana(t));
    view.follow(&mut client, |view| peer_of(view, &pipe).0).await;
    assert_eq!(peer_of(&view, &pipe), (true, "ana@example.com".into(), Change::None, String::new()));

    // The same account with another email: a notice, still verified.
    hear(&statement("a1", "ana@new.example", t + 1, t + 1 + week));
    view.follow(&mut client, |view| peer_of(view, &pipe).2 == Change::EmailChanged).await;
    assert_eq!(peer_of(&view, &pipe), (true, "ana@new.example".into(), Change::EmailChanged, "ana@example.com".into()));
    apply_request(&mut client, &mut view, accept(&pipe)).await;
    view.follow(&mut client, |view| peer_of(view, &pipe).2 == Change::None).await;
    assert_eq!(peer_of(&view, &pipe).1, "ana@new.example");

    // Signed out there: no longer verified, and the pin stays.
    hear("");
    view.follow(&mut client, |view| peer_of(view, &pipe).2 == Change::Lapsed).await;
    assert_eq!(peer_of(&view, &pipe), (false, String::new(), Change::Lapsed, "ana@new.example".into()));
    // Dismissing a lapse keeps the pin: another account after it is a change.
    apply_request(&mut client, &mut view, accept(&pipe)).await;
    view.follow(&mut client, |view| peer_of(view, &pipe).2 == Change::None).await;
    assert_eq!(peer_of(&view, &pipe), (false, String::new(), Change::None, String::new()));
    hear(&statement("b1", "bo@example.com", t + 2, t + 2 + week));
    view.follow(&mut client, |view| peer_of(view, &pipe).2 == Change::AccountChanged).await;
    assert_eq!(peer_of(&view, &pipe), (false, "bo@example.com".into(), Change::AccountChanged, "ana@new.example".into()));

    // A statement older than the one held is ignored: no flipping back.
    hear(&statement("a1", "ana@new.example", t + 1, t + 1 + week));
    assert_eq!(peer_of(&view, &pipe).2, Change::AccountChanged);

    // While it's another account: what comes waits for the person, agents
    // can't send, and the person still can.
    pipes::receive_for_test(&factory, &pipe, incoming("m1", "FROM-BO")).expect("received");
    assert_eq!(state_of(&factory, &pipe, "FROM-BO"), v1::PipeMessageState::HeldIncoming);
    let held = pipes::messages_for_test(&factory, &pipe).into_iter().find(|message| message.text == "FROM-BO").expect("held");
    assert!(held.note.starts_with("Waiting for you: this pipe's Claktory is no longer confirmed as ana@new.example's. Check with "), "{}", held.note);
    assert!(held.note.ends_with(", then choose Trust this Claktory… to let it through."), "{}", held.note);
    let refused = call(&mut client, &mut view, "lea", "pipe_send", r#"{"text": "hello"}"#).await;
    assert!(refused.contains("now belongs to a different account; the person has to accept it first"), "{refused}");
    let mine = v1::SendPipeMessage { pipe_id: pipe.clone(), text: "Is this you, Ana?".into() };
    apply_request(&mut client, &mut view, client_message::Body::SendPipeMessage(mine)).await;
    // (No transport here: it can't go, but not for the account.)
    let mine = pipes::messages_for_test(&factory, &pipe).into_iter().find(|message| message.text == "Is this you, Ana?").expect("kept");
    assert!(!mine.note.contains("different account"), "{}", mine.note);

    // Accepted: Bo is pinned, what waited goes in, agents can send again.
    apply_request(&mut client, &mut view, accept(&pipe)).await;
    view.follow(&mut client, |view| peer_of(view, &pipe) == (true, "bo@example.com".into(), Change::None, String::new())).await;
    assert_eq!(state_of(&factory, &pipe, "FROM-BO"), v1::PipeMessageState::Received);
    let sent = call(&mut client, &mut view, "lea", "pipe_send", r#"{"text": "hello Bo"}"#).await;
    assert!(!sent.contains("different account"), "{sent}");
}

#[tokio::test]
async fn while_this_side_cant_check_nothing_is_verified_and_nothing_changes() {
    let (_dir, factory, mut client, mut view, pipe) = setup(false).await;
    vouch(&factory, true);
    let (t, week) = (now(), 7 * 24 * 3600);
    view.follow(&mut client, |view| shown(view, &pipe).is_some_and(|pipe| pipe.peer.is_some())).await;
    identity::heard_for_test(&factory, &pipe, PEER, &statement("a1", "ana@example.com", t, t + week));
    view.follow(&mut client, |view| peer_of(view, &pipe).0).await;

    // Signed out here: not verified, no change, nothing held; the pin kept.
    identity::vouch_for_test(&factory, false, Some(String::new()), |_: &str, _: &str, _: i64| None);
    identity::heard_for_test(&factory, &pipe, PEER, &statement("b1", "bo@example.com", t + 1, t + 1 + week));
    apply_request(&mut client, &mut view, crate::pipes::select("bo")).await;
    assert_eq!(peer_of(&view, &pipe), (false, String::new(), Change::None, String::new()));
    pipes::receive_for_test(&factory, &pipe, incoming("m1", "HELLO")).expect("received");
    assert_eq!(state_of(&factory, &pipe, "HELLO"), v1::PipeMessageState::Received);

    // Signed in again, it's Ana as before.
    vouch(&factory, true);
    apply_request(&mut client, &mut view, crate::pipes::select("lea")).await;
    assert_eq!(peer_of(&view, &pipe), (true, "ana@example.com".into(), Change::None, String::new()));
}

fn accept(pipe: &str) -> client_message::Body {
    client_message::Body::AcceptPipeIdentity(v1::AcceptPipeIdentity { pipe_id: pipe.into() })
}

/// The real thing, end to end with an account service: this Claktory proves
/// its tunnel key (iroh's form of it, as the pipes pin it) and gets a
/// statement that checks here for that key, and for no other; signed out,
/// it has none.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_account_service_vouches_for_the_key_the_pipes_pin() {
    let dir = crate::common::temp_dir();
    let service = crate::claktory_account::Instance::start(&dir).await;
    let (addr, factory) = crate::claktory_account::open(&dir, &service.url).await;
    let (mut client, _, _) = crate::common::Client::connect(addr).await;
    assert_eq!(identity::own_statement_for_test(&factory).await, Ok(None), "not signed in: none");
    crate::claktory_account::sign_in(&mut client, &service, "ana@example.com").await;
    let statement = identity::own_statement_for_test(&factory).await.expect("answered").expect("a statement");
    let endpoint = identity::endpoint_for_test(&factory);
    let (account, email) = identity::check_for_test(&factory, &statement, &endpoint).expect("it checks for this Claktory's key");
    assert!(!account.is_empty());
    assert_eq!(email, "ana@example.com");
    assert!(identity::check_for_test(&factory, &statement, &endpoint.to_uppercase()).is_some(), "hex in either case");
    // For any other key, or changed, it's nothing.
    let other = format!("{}{}", &endpoint[..63], if endpoint.ends_with('0') { '1' } else { '0' });
    assert!(identity::check_for_test(&factory, &statement, &other).is_none());
    let mut tampered = statement.clone();
    tampered.insert(5, 'x');
    assert!(identity::check_for_test(&factory, &tampered, &endpoint).is_none());
}
