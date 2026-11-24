//! Agents and the person's user-level apps (U2): Rivet proposes giving an
//! agent access to an app at the account (app_grants_propose), the person
//! decides on its card (DecideAppGrant), and only then does the account
//! service make the grant, for the agent's incarnation, which renaming
//! keeps and deleting ends. Here against a local account service with its
//! user-apps worker, as the deploy runs them.

use std::os::unix::fs::MetadataExt as _;
use std::path::Path;
use std::time::Duration;

use claktory_account::server::UserApps;
use claktory_user_apps::worker::{self, Worker};
use factory_protocol::v1::{self, AgentStatus, AppGrantState as State, client_message::Body as Request, entry::Kind};

use crate::claktory_account::{Instance, open, sign_in};
use crate::common::{Client, MODEL, View, apply_request, create_agent, create_project, send_message, start, temp_dir};

const WAIT: Duration = Duration::from_secs(60);

/// A user-apps worker at `dir/apps.sock`, answering this process's user,
/// and an account service that forwards to it.
async fn service(dir: &Path) -> Instance {
    let uid = std::fs::metadata(dir).expect("the folder").uid();
    let socket = dir.join("apps.sock");
    let listener = tokio::net::UnixListener::bind(&socket).expect("a socket");
    let keys = claktory_user_apps::dms::keys::Keys::from_bytes([7; 32]);
    let worker = Worker::open(&dir.join("apps.db"), keys).expect("the worker");
    tokio::spawn(worker::serve(listener, worker, uid));
    Instance::start_with_apps(dir, Some(UserApps { socket, uid })).await
}

/// The grants the worker holds: (agent, incarnation, scopes).
fn held(dir: &Path) -> Vec<(String, String, String)> {
    let db = rusqlite::Connection::open(dir.join("apps.db")).expect("the worker's database");
    let mut statement = db.prepare("SELECT agent_id, incarnation, scopes FROM grants ORDER BY agent_id").expect("sql");
    statement.query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?))).expect("sql").map(|row| row.expect("a row")).collect()
}

fn idle(view: &View, agent: &str) -> bool {
    let agent = view.agent(agent);
    agent.status() == AgentStatus::Idle && agent.queued_messages == 0
}

/// Rivet calls `tool` with `args`; what it said.
async fn call(client: &mut Client, view: &mut View, args: &serde_json::Value) -> String {
    let before = view.entries.len();
    apply_request(client, view, send_message("rivet", &format!("call app_grants_propose {args}"))).await;
    view.follow(client, |view| view.entries.len() >= before + 3 && idle(view, "rivet")).await;
    view.entries[before..]
        .iter()
        .find_map(|entry| match &entry.kind {
            Some(Kind::ToolCall(call)) => Some(call.output.clone()),
            _ => None,
        })
        .expect("a tool call")
}

/// Rivet's latest proposal card, and its seq.
fn card(view: &View) -> (u64, v1::AppGrantProposal) {
    view.entries
        .iter()
        .rev()
        .find_map(|entry| match &entry.kind {
            Some(Kind::AppGrant(proposal)) => Some((entry.seq, proposal.clone())),
            _ => None,
        })
        .expect("a card")
}

fn card_at(view: &View, seq: u64) -> v1::AppGrantProposal {
    match &view.entries.iter().find(|entry| entry.seq == seq).expect("the card").kind {
        Some(Kind::AppGrant(proposal)) => proposal.clone(),
        other => panic!("expected a card, got {other:?}"),
    }
}

/// Whether Rivet was told something containing `text`.
fn told(view: &View, text: &str) -> bool {
    view.entries.iter().any(|entry| matches!(&entry.kind, Some(Kind::Event(event)) if event.text.contains(text)))
}

fn incarnation(dir: &Path, agent: &str) -> Option<String> {
    std::fs::read_to_string(dir.join("home/agents").join(agent).join("incarnation")).ok().map(|text| text.trim().to_owned())
}

async fn decide(client: &mut Client, view: &mut View, seq: u64, approve: bool, comment: &str) -> v1::server_message::Body {
    let decision = v1::DecideAppGrant { agent_id: "rivet".into(), seq, approve, comment: comment.into(), ..Default::default() };
    apply_request(client, view, Request::DecideAppGrant(decision)).await
}

fn proposal(agent: &str, scopes: &[&str], conversation: &str) -> serde_json::Value {
    serde_json::json!({
        "agent": agent,
        "app": "dms",
        "scopes": scopes,
        "resources": [{"kind": "conversation", "id": conversation}],
        "reason": "Matt wants Mike to keep up with the launch thread.",
    })
}

#[tokio::test(flavor = "multi_thread")]
async fn rivet_proposes_app_access_and_only_the_persons_approval_gives_it() {
    let dir = temp_dir();
    let instance = service(&dir).await;
    // Signing in on, and Rivet, the admin agent.
    std::fs::create_dir_all(dir.join("home")).expect("home");
    let config = serde_json::json!({
        "account": {"enabled": true, "service": instance.url},
        "agents": [{"id": "rivet", "name": "Rivet", "model": MODEL, "effort": "off", "extensions": {"agents": {"admin": true}}}],
    });
    std::fs::write(dir.join("home/config.json"), config.to_string()).expect("config");
    let (addr, factory) = open(&dir, &instance.url).await;
    let (mut client, _, _) = Client::connect(addr).await;
    let workspace = sign_in(&mut client, &instance, "matt@example.com").await;
    let mut view = View { workspace, ..View::default() };
    apply_request(&mut client, &mut view, create_agent("", "Mike", "")).await;
    apply_request(&mut client, &mut view, Request::SelectAgent(v1::SelectAgent { agent_id: "rivet".into() })).await;

    // What the account's apps don't have is refused before any card.
    let refused = call(&mut client, &mut view, &proposal("Mike", &["dm.everything"], "c-1")).await;
    assert!(refused.contains("has no scope \"dm.everything\""), "{refused}");
    let mut nothing = proposal("Mike", &["dm.read"], "c-1");
    nothing["resources"] = serde_json::json!([]);
    let refused = call(&mut client, &mut view, &nothing).await;
    assert!(refused.starts_with("Name what it would reach in Direct messages"), "{refused}");
    let refused = call(&mut client, &mut view, &proposal("Nobody", &["dm.read"], "c-1")).await;
    assert_eq!(refused, "There's no agent \"Nobody\" in this Claktory.");
    assert!(view.entries.iter().all(|entry| !matches!(entry.kind, Some(Kind::AppGrant(_)))), "no card");

    // A liaison, limited to a pipe, is never given access (Fugo's review).
    apply_request(&mut client, &mut view, create_project("Site", &dir.join("site"))).await;
    let pipe = factoryd::pipes::open_for_test(&factory, "site", "Website ⇄ Marketing", &[], "", false);
    apply_request(&mut client, &mut view, Request::AddPipeLiaison(v1::AddPipeLiaison { pipe_id: pipe, name: "Liaison".into() })).await;
    apply_request(&mut client, &mut view, Request::SelectAgent(v1::SelectAgent { agent_id: "rivet".into() })).await;
    let refused = call(&mut client, &mut view, &proposal("Liaison", &["dm.read"], "c-1")).await;
    assert!(refused.starts_with("Liaison is limited to a pipe"), "{refused}");
    assert!(view.entries.iter().all(|entry| !matches!(entry.kind, Some(Kind::AppGrant(_)))), "no card");

    // A card, waiting for the person; nothing is given yet.
    let said = call(&mut client, &mut view, &proposal("Mike", &["dm.read", "dm.draft"], "c-1")).await;
    assert!(said.starts_with("Opened a card in your conversation for the person to approve or decline giving Mike access"), "{said}");
    let (seq, shown) = card(&view);
    assert_eq!((shown.state(), shown.app_id.as_str(), shown.app_name.as_str()), (State::Pending, "dms", "Direct messages"));
    assert_eq!(shown.agent_id, "mike");
    let scopes: Vec<(&str, &str)> = shown.scopes.iter().map(|scope| (scope.id.as_str(), scope.label.as_str())).collect();
    assert_eq!(scopes, [("dm.draft", "Write drafts in them for you to send"), ("dm.read", "Read these conversations")]);
    assert_eq!((shown.resources[0].kind.as_str(), shown.resources[0].id.as_str()), ("conversation", "c-1"));
    assert_eq!((shown.expires_ms, shown.reason.as_str()), (0, "Matt wants Mike to keep up with the launch thread."));
    assert!(!shown.grantee_read_pipes);
    assert!(view.agent("rivet").waiting_seqs.contains(&seq), "it waits in Needs you");
    assert!(held(&dir).is_empty());
    let first = incarnation(&dir, "mike").expect("Mike's incarnation");
    assert!(first.len() == 32 && first.bytes().all(|b| b.is_ascii_hexdigit()), "{first}");

    // Declined, with a comment: Rivet hears it, and nothing is given. Mike
    // has read a web page by then, which the card says.
    factoryd::pipes::taint_for_test(&factory, "mike", "browser:https://example.com");
    call(&mut client, &mut view, &proposal("mike", &["dm.send"], "c-2")).await;
    let (declined, shown) = card(&view);
    assert!(shown.grantee_read_pipes);
    decide(&mut client, &mut view, declined, false, "Not sending for me yet.").await;
    view.follow(&mut client, |view| told(view, "Not sending for me yet.") && idle(view, "rivet")).await;
    let shown = card_at(&view, declined);
    assert_eq!((shown.state(), shown.comment.as_str()), (State::Declined, "Not sending for me yet."));
    assert!(told(
        &view,
        "The person declined giving Mike access to Direct messages. Nothing was given.\n\nThe person's note: Not sending for me yet."
    ));
    assert!(held(&dir).is_empty());

    // Submit (#370): a comment instead of Approve gives nothing; Rivet gets
    // the comment, to propose again with it in mind.
    call(&mut client, &mut view, &proposal("mike", &["dm.read", "dm.send"], "c-4")).await;
    let (answered, _) = card(&view);
    let reply = |comment: &str| v1::DecideAppGrant {
        agent_id: "rivet".into(),
        seq: answered,
        approve: true,
        comment: comment.into(),
        reply: true,
    };
    let empty = apply_request(&mut client, &mut view, Request::DecideAppGrant(reply(" "))).await;
    let v1::server_message::Body::Error(error) = empty else { panic!("expected a refusal, got {empty:?}") };
    assert_eq!(error.message, "Write a reply, or approve or decline it.");
    let replied = apply_request(&mut client, &mut view, Request::DecideAppGrant(reply("Read only, please."))).await;
    assert!(matches!(replied, v1::server_message::Body::Ack(_)), "{replied:?}");
    view.follow(&mut client, |view| told(view, "Read only, please.") && idle(view, "rivet")).await;
    let shown = card_at(&view, answered);
    assert_eq!((shown.state(), shown.replied, shown.comment.as_str()), (State::Declined, true, "Read only, please."));
    assert!(told(
        &view,
        "The person replied about giving Mike access to Direct messages; nothing was given yet. They say:\n\nRead only, please.\n\nIf it still fits, propose it again with that in mind."
    ));
    assert!(held(&dir).is_empty(), "a reply gives nothing");

    // Approved: the service holds the grant, for Mike's incarnation. Mike
    // read a web page after this card was shown, so the first approval is
    // refused and the card warns; the next is informed.
    assert!(!card_at(&view, seq).grantee_read_pipes);
    let refused = decide(&mut client, &mut view, seq, true, "").await;
    let v1::server_message::Body::Error(error) = refused else { panic!("expected a refusal, got {refused:?}") };
    assert_eq!(error.message, "Mike has read a page on https://example.com since Rivet suggested this. Look at it again.");
    view.follow(&mut client, |view| card_at(view, seq).grantee_read_pipes).await;
    assert_eq!((card_at(&view, seq).state(), held(&dir).len()), (State::Pending, 0));
    let reply = decide(&mut client, &mut view, seq, true, "").await;
    assert!(matches!(reply, v1::server_message::Body::Ack(_)), "{reply:?}");
    view.follow(&mut client, |view| card_at(view, seq).state() == State::Granted && !view.agent("rivet").waiting_seqs.contains(&seq)).await;
    assert!(!card_at(&view, seq).grant_id.is_empty());
    assert_eq!(held(&dir), [("mike".to_owned(), first.clone(), r#"["dm.draft","dm.read"]"#.to_owned())]);
    view.follow(&mut client, |view| told(view, "The person gave Mike access to Direct messages (dm.draft, dm.read)")).await;
    let again = decide(&mut client, &mut view, seq, true, "").await;
    assert!(matches!(again, v1::server_message::Body::Ack(_)), "deciding twice changes nothing: {again:?}");
    assert_eq!(held(&dir).len(), 1);

    // Mike hears it too, in Claktory's words.
    apply_request(&mut client, &mut view, Request::SelectAgent(v1::SelectAgent { agent_id: "mike".into() })).await;
    view.follow(&mut client, |view| told(view, "The person gave you access to Direct messages, until they take it back: Write drafts in them for you to send; Read these conversations (conversation c-1).")).await;
    apply_request(&mut client, &mut view, Request::SelectAgent(v1::SelectAgent { agent_id: "rivet".into() })).await;

    // Renamed, Mike is the same agent; proposing again says what it replaces.
    let renamed = v1::UpdateAgent { agent_id: "mike".into(), name: Some("Michael".into()), ..Default::default() };
    apply_request(&mut client, &mut view, Request::UpdateAgent(renamed)).await;
    assert_eq!(incarnation(&dir, "mike").as_deref(), Some(first.as_str()));
    let said = call(&mut client, &mut view, &proposal("Michael", &["dm.read"], "c-3")).await;
    assert!(said.ends_with("Michael already has dm.draft, dm.read on c-1 there; approved, this replaces it."), "{said}");
    let (stale, _) = card(&view);

    // Deleted, its grant is revoked at the service; made again, it's
    // another agent, which the card waiting for the first can't give to.
    apply_request(&mut client, &mut view, Request::DeleteAgent(v1::DeleteAgent { agent_id: "mike".into() })).await;
    let revoked = async {
        while !held(&dir).is_empty() {
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    };
    tokio::time::timeout(WAIT, revoked).await.expect("the deleted agent's grant revoked");
    apply_request(&mut client, &mut view, create_agent("", "Mike", "")).await;
    assert!(view.workspace.agents.iter().any(|agent| agent.id == "mike"), "the same id again");
    apply_request(&mut client, &mut view, Request::SelectAgent(v1::SelectAgent { agent_id: "rivet".into() })).await;
    decide(&mut client, &mut view, stale, true, "").await;
    view.follow(&mut client, |view| card_at(view, stale).state() == State::Failed).await;
    let shown = card_at(&view, stale);
    assert_eq!(shown.error, "Mike was deleted since Rivet suggested this, so nothing was given.");
    assert!(held(&dir).is_empty());
    call(&mut client, &mut view, &proposal("Mike", &["dm.read"], "c-1")).await;
    let second = incarnation(&dir, "mike").expect("the new Mike's incarnation");
    assert_ne!(second, first, "a new agent, a new incarnation");

    // Needs you's history keeps each decision, opening Rivet's conversation
    // at its card.
    let history = client.request(Request::ListNeedsYouHistory(v1::ListNeedsYouHistory::default())).await.body;
    let v1::server_message::Body::NeedsYouHistory(history) = history else { panic!("expected the history, got {history:?}") };
    let decided: Vec<(u64, v1::NeedsYouOutcome)> = history.records.iter().map(|record| (record.seq, record.outcome())).collect();
    use v1::NeedsYouOutcome as Outcome;
    assert_eq!(
        decided,
        [(stale, Outcome::Failed), (seq, Outcome::Approved), (answered, Outcome::Replied), (declined, Outcome::Denied)]
    );

    // /clear closes what waits, as cleared, not as the person's decline.
    assert!(!view.agent("rivet").waiting_seqs.is_empty());
    let clear = v1::RunCommand { agent_id: "rivet".into(), name: "clear".into(), args: String::new() };
    apply_request(&mut client, &mut view, Request::RunCommand(clear)).await;
    view.follow(&mut client, |view| view.agent("rivet").waiting_seqs.is_empty()).await;
    let states: Vec<String> = rusqlite::Connection::open(dir.join("home/factory.db"))
        .expect("the records")
        .prepare("SELECT state FROM app_grant_proposals ORDER BY created")
        .expect("sql")
        .query_map([], |row| row.get(0))
        .expect("sql")
        .map(|row| row.expect("a row"))
        .collect();
    assert_eq!(states, ["granted", "declined", "declined", "failed", "cleared"]);
}

#[tokio::test(flavor = "multi_thread")]
async fn only_an_untainted_rivet_of_a_signed_in_claktory_proposes() {
    let dir = temp_dir();
    let (addr, factory) = start(&dir).await;
    let (mut client, workspace, _) = Client::connect(addr).await;
    let mut view = View { workspace, ..View::default() };
    let rivet = v1::UpdateAgent { agent_id: "rivet".into(), model: Some(MODEL.into()), effort: Some("off".into()), ..Default::default() };
    apply_request(&mut client, &mut view, Request::UpdateAgent(rivet)).await;
    apply_request(&mut client, &mut view, create_agent("", "Mike", "")).await;
    apply_request(&mut client, &mut view, Request::SelectAgent(v1::SelectAgent { agent_id: "rivet".into() })).await;

    let refused = call(&mut client, &mut view, &proposal("Mike", &["dm.read"], "c-1")).await;
    assert!(refused.starts_with("This Claktory isn't signed in to the person's account"), "{refused}");
    factoryd::pipes::taint_for_test(&factory, "rivet", "browser:https://example.com");
    let refused = call(&mut client, &mut view, &proposal("Mike", &["dm.read"], "c-1")).await;
    assert!(refused.starts_with("You've read a page on https://example.com, so you can't propose access"), "{refused}");
    assert!(view.entries.iter().all(|entry| !matches!(entry.kind, Some(Kind::AppGrant(_)))), "no card");
    assert_eq!(incarnation(&dir, "mike"), None, "none made for nothing");
}
