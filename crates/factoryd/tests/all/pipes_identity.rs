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
    client_message::Body::AcceptPipeIdentity(v1::AcceptPipeIdentity { pipe_id: pipe.into(), ..Default::default() })
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

/// What this side presents follows its sign-in at once (Abbacchio's row
/// 1252): signed out and in again to another account, between two pings,
/// it never presents the first account's statement, and presents the new
/// one as soon as the service gives it, not a day later.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_new_sign_in_presents_its_own_statement_at_once() {
    let dir = crate::common::temp_dir();
    let service = crate::claktory_account::Instance::start(&dir).await;
    let (addr, factory) = crate::claktory_account::open(&dir, &service.url).await;
    let (mut client, _, _) = crate::common::Client::connect(addr).await;
    let endpoint = identity::endpoint_for_test(&factory);
    let presented = |factory: &std::sync::Arc<factoryd::factory::Factory>| {
        let statement = identity::own_for_test(factory).expect("a real statement");
        identity::check_for_test(factory, &statement, &endpoint).map(|(_, email)| email)
    };
    let until = async |factory: &std::sync::Arc<factoryd::factory::Factory>, email: &str| {
        for _ in 0..200 {
            if presented(factory).as_deref() == Some(email) {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        panic!("never presented {email}'s statement; presents {:?}", presented(factory));
    };
    crate::claktory_account::sign_in(&mut client, &service, "ana@example.com").await;
    until(&factory, "ana@example.com").await;

    // Out and in again as another account, with no ping in between.
    client.ok(client_message::Body::AccountSignOut(v1::AccountSignOut {})).await;
    crate::claktory_account::sign_in(&mut client, &service, "bo@example.com").await;
    assert_ne!(presented(&factory).as_deref(), Some("ana@example.com"), "never the other account's");
    until(&factory, "bo@example.com").await;

    // Signed in again, even to the same account: nothing until the new
    // sign-in's statement comes (Fugo), then that.
    client.ok(client_message::Body::AccountSignOut(v1::AccountSignOut {})).await;
    crate::claktory_account::sign_in(&mut client, &service, "bo@example.com").await;
    assert_eq!(identity::own_for_test(&factory).as_deref(), Some(""), "the last sign-in's dropped at once");
    until(&factory, "bo@example.com").await;
}

/// A statement vouches for the other side only for a while after the
/// account service made it (Fugo, row 1252: 25 hours for now, an hour
/// later), however long it says it
/// lasts: one kept on past a sign-out or another sign-in stops showing the
/// pipe as verified, and one heard already stale changes nothing.
#[tokio::test]
async fn a_statement_vouches_only_while_its_fresh() {
    let (_dir, factory, mut client, mut view, pipe) = setup(false).await;
    vouch(&factory, true);
    let (t, week) = (now(), 7 * 24 * 3600);
    let hear = |identity: &str| identity::heard_for_test(&factory, &pipe, PEER, identity);
    view.follow(&mut client, |view| shown(view, &pipe).is_some_and(|pipe| pipe.peer.is_some())).await;

    let fresh = 25 * 3600;
    // Made before that, though it says it lasts a week: nothing.
    hear(&statement("a1", "ana@example.com", t - fresh - 1, t + week));
    apply_request(&mut client, &mut view, crate::pipes::select("lea")).await;
    assert_eq!(peer_of(&view, &pipe), (false, String::new(), Change::None, String::new()));

    // Made just within it: verified, until it's up.
    hear(&statement("a1", "ana@example.com", t - fresh + 2, t + week));
    view.follow(&mut client, |view| peer_of(view, &pipe).0).await;
    tokio::time::sleep(std::time::Duration::from_secs(3)).await;
    apply_request(&mut client, &mut view, crate::pipes::select("lea")).await;
    assert_eq!(peer_of(&view, &pipe), (false, String::new(), Change::Lapsed, "ana@example.com".into()));
}

/// Abbacchio's rows 1327 and 1328, Fugo: back to the account the person
/// accepted, with a statement made after the other account's, the change
/// clears by itself; one kept from before can't clear it. What came
/// meanwhile isn't let in but waits for the person. Another account with
/// nothing presented can't be accepted.
#[tokio::test]
async fn back_to_the_accepted_account_clears_the_change_only_with_a_newer_statement() {
    let (_dir, factory, mut client, mut view, pipe) = setup(false).await;
    vouch(&factory, true);
    let (t, week) = (now(), 7 * 24 * 3600);
    let ana = |iat: i64| statement("a1", "ana@example.com", iat, iat + week);
    let hear = |identity: &str| identity::heard_for_test(&factory, &pipe, PEER, identity);
    view.follow(&mut client, |view| shown(view, &pipe).is_some_and(|pipe| pipe.peer.is_some())).await;
    hear(&ana(t));
    view.follow(&mut client, |view| peer_of(view, &pipe).0).await;

    // Cyd: held; what comes waits.
    hear(&statement("c1", "cyd@example.com", t + 10, t + 10 + week));
    view.follow(&mut client, |view| peer_of(view, &pipe).2 == Change::AccountChanged).await;
    pipes::receive_for_test(&factory, &pipe, incoming("m1", "FROM-CYD")).expect("received");
    assert_eq!(state_of(&factory, &pipe, "FROM-CYD"), v1::PipeMessageState::HeldIncoming);
    let early = pipes::messages_for_test(&factory, &pipe).into_iter().find(|message| message.text == "FROM-CYD").expect("held").seq;
    // Then another, after more than a page of the pipe (Fugo 37600): every
    // one held is relabelled, however far back.
    for n in 0..1001 {
        pipes::sent_for_test(&factory, &pipe, &format!("filler {n}"), true);
    }
    pipes::receive_for_test(&factory, &pipe, incoming("m3", "FROM-CYD")).expect("received");

    // Ana's statements from before Cyd's, or of the same second, replayed:
    // still held.
    for iat in [t, t + 9, t + 10] {
        hear(&ana(iat));
        apply_request(&mut client, &mut view, crate::pipes::select("lea")).await;
        assert_eq!(peer_of(&view, &pipe), (false, "cyd@example.com".into(), Change::AccountChanged, "ana@example.com".into()));
    }

    // Signed out there: another account with nothing presented can't be
    // accepted (row 1328), and nothing is let in.
    hear("");
    view.follow(&mut client, |view| peer_of(view, &pipe).1.is_empty()).await;
    match apply_request(&mut client, &mut view, accept(&pipe)).await {
        v1::server_message::Body::Error(error) => {
            assert_eq!(error.message, "Wait until the other side's Claktory says which account it's signed in to; it doesn't say now.")
        }
        other => panic!("not refused: {other:?}"),
    }
    assert_eq!(peer_of(&view, &pipe).2, Change::AccountChanged);
    assert_eq!(state_of(&factory, &pipe, "FROM-CYD"), v1::PipeMessageState::HeldIncoming);

    // Ana again, newly vouched: verified as before, no card or accept
    // needed; what came from Cyd waits for the person, never let in.
    hear(&ana(t + 11));
    view.follow(&mut client, |view| peer_of(view, &pipe).2 == Change::None).await;
    assert_eq!(peer_of(&view, &pipe), (true, "ana@example.com".into(), Change::None, String::new()));
    let messages = pipes::messages_for_test(&factory, &pipe);
    let held = messages.iter().find(|message| message.text == "FROM-CYD").expect("kept");
    assert_eq!(held.state(), v1::PipeMessageState::HeldIncoming);
    let meanwhile = "Waiting for you: this came while their Claktory was signed in to a different account. Let it in, or discard it.";
    assert_eq!(held.note, meanwhile);
    assert_eq!(identity::kept_for_test(&factory, &pipe, early), Some(("held_incoming".into(), meanwhile.into())));
    assert!(
        messages.iter().any(|message| message.text
            == "The other side's Claktory signs in with the account you accepted again, ana@example.com. What came from it while it was another account waits for you."),
        "{messages:?}"
    );
    // Agents can send again; what comes now goes in.
    let sent = call(&mut client, &mut view, "lea", "pipe_send", r#"{"text": "hello Ana"}"#).await;
    assert!(!sent.contains("different account"), "{sent}");
    pipes::receive_for_test(&factory, &pipe, incoming("m2", "FROM-ANA")).expect("received");
    assert_eq!(state_of(&factory, &pipe, "FROM-ANA"), v1::PipeMessageState::Received);
    // The person lets in what came meanwhile.
    let seq = held.seq;
    let let_in = v1::DecidePipeMessage { pipe_id: pipe.clone(), seq, allow: true, ..Default::default() };
    apply_request(&mut client, &mut view, client_message::Body::DecidePipeMessage(let_in)).await;
    assert_eq!(state_of(&factory, &pipe, "FROM-CYD"), v1::PipeMessageState::Received);
}
