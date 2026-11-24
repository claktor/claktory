//! Agents proposing direct messages (D4b): Mike, given access to send in
//! Bo's conversation, proposes a message that's sent only once the person
//! approves exactly it on a device. Here against a local account service
//! with its user-apps worker, as the deploy runs them, and real MLS groups
//! on the person's and Bo's devices. Reading, drafting and following (D4a)
//! are in dms_leaf.rs, with the leaf they read through.
//!
//! A file of its own: agents in direct messages are a process-wide preview
//! until People ships, and the shared test binary counts the extensions it
//! lists.

#[path = "common/mod.rs"]
mod common;
#[path = "common/dm_device.rs"]
mod dm_device;

use std::os::unix::fs::MetadataExt as _;
use std::path::Path;
use std::sync::Arc;

use claktory_account::server::{self, Config, Secrets, Service, UserApps, mail};
use claktory_user_apps::worker::{self, Worker};
use common::{Client, MODEL, View, apply_request, create_agent, last_workspace, send_message, start, temp_dir};
use factory_protocol::v1::{self, AgentStatus, client_message::Body as Request, entry::Kind};
use serde_json::{Value, json};

/// A user-apps worker at `dir/apps.sock` and an account service that
/// forwards to it; the service's sent mail.
async fn service(dir: &Path) -> (String, Arc<mail::Memory>) {
    let uid = std::fs::metadata(dir).expect("the folder").uid();
    let socket = dir.join("apps.sock");
    let listener = tokio::net::UnixListener::bind(&socket).expect("a socket");
    let keys = claktory_user_apps::dms::keys::Keys::from_bytes([7; 32]);
    let worker = Worker::open(&dir.join("apps.db"), keys).expect("the worker");
    tokio::spawn(worker::serve(listener, worker, uid));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("a port");
    let url = format!("http://{}", listener.local_addr().expect("an address"));
    let mail = Arc::new(mail::Memory::default());
    let config = Config { user_apps: Some(UserApps { socket, uid }), ..Config::default() };
    let clock: server::Clock = Arc::new(|| jiff::Timestamp::now().as_second());
    let service = Service::new(&dir.join("account.db"), Secrets::generate(), config, mail.clone(), clock).expect("the service");
    tokio::spawn(server::serve(listener, service));
    (url, mail)
}

/// Signs in to the account service as `email`.
async fn sign_in(client: &mut Client, mail: &mail::Memory, email: &str) -> v1::Workspace {
    client.ok(Request::AccountSignIn(v1::AccountSignIn { email: email.into() })).await;
    let sent = mail.sent().into_iter().rev().find(|sent| sent.to == email).expect("an email");
    let code = sent.subject.rsplit(' ').next().expect("a code").to_owned();
    let events = client.ok(Request::AccountVerify(v1::AccountVerify { code })).await;
    last_workspace(&events).expect("workspace").clone()
}

fn idle(view: &View, agent: &str) -> bool {
    let agent = view.agent(agent);
    agent.status() == AgentStatus::Idle && agent.queued_messages == 0
}

/// `agent` calls `tool` with `args`: what it said.
async fn call(client: &mut Client, view: &mut View, agent: &str, tool: &str, args: &Value) -> String {
    let before = view.entries.len();
    apply_request(client, view, send_message(agent, &format!("call {tool} {args}"))).await;
    view.follow(client, |view| view.entries.len() >= before + 3 && idle(view, agent)).await;
    view.entries[before..]
        .iter()
        .find_map(|entry| match &entry.kind {
            Some(Kind::ToolCall(call)) => Some(call.output.clone()),
            _ => None,
        })
        .expect("a tool call")
}

async fn select(client: &mut Client, view: &mut View, agent: &str) {
    apply_request(client, view, Request::SelectAgent(v1::SelectAgent { agent_id: agent.into() })).await;
}

/// The proposal id a dm_send answer names.
fn proposal_in(said: &str) -> String {
    let start = said.find("(proposal ").expect("a proposal") + "(proposal ".len();
    said[start..].split(')').next().expect("its id").to_owned()
}

fn told(view: &View, start: &str) -> Option<String> {
    view.entries.iter().rev().find_map(|entry| match &entry.kind {
        Some(Kind::Event(event)) if event.text.starts_with(start) => Some(event.text.clone()),
        _ => None,
    })
}

/// Waits for `ok`, for up to 20 s.
async fn until(what: &str, ok: impl Fn() -> bool) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    while !ok() {
        assert!(std::time::Instant::now() < deadline, "{what}");
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
}

/// D4b, end to end (dms-e2ee.md §6): Mike, given access to send in Bo's
/// conversation, proposes a message. It's encrypted for Matt's own devices
/// only, in this Claktory's channel with them: the service never holds its
/// text, and it waits in Needs you (who and to whom, never the text) until
/// Matt's device opens it, shows it and, approved, encrypts exactly it to
/// Bo itself. He can change it while it waits, or take it back; Matt can
/// decline it. A device that joins later gets what still waits sent again.
#[tokio::test(flavor = "multi_thread")]
async fn an_agents_message_is_sent_only_once_the_person_approves_exactly_it() {
    use claktory_dm_crypto::certificate::{LeafKind, claktory_of};
    use claktory_dm_crypto::validation::GroupKind;

    factoryd::user_apps::dms::set_preview(true);
    let dir = temp_dir();
    let (url, mail) = service(&dir).await;
    std::fs::create_dir_all(dir.join("home")).expect("home");
    let config = json!({
        "account": {"enabled": true, "service": url},
        "agents": [{"id": "rivet", "name": "Rivet", "model": MODEL, "effort": "off", "extensions": {"agents": {"admin": true}}}],
    });
    std::fs::write(dir.join("home/config.json"), config.to_string()).expect("config");
    let (addr, factory) = start(&dir).await;
    let (mut client, _, _) = Client::connect(addr).await;
    let workspace = sign_in(&mut client, &mail, "matt@example.com").await;
    let mut view = View { workspace, ..View::default() };
    apply_request(&mut client, &mut view, create_agent("", "Mike", "")).await;
    select(&mut client, &mut view, "rivet").await;
    let matt: String = rusqlite::Connection::open(dir.join("account.db"))
        .expect("the account service's records")
        .query_row("SELECT id FROM accounts WHERE email = 'matt@example.com'", [], |row| row.get(0))
        .expect("Matt's account");

    // Matt's phone and Bo, with a device of his own, become friends.
    let mut bo = dm_device::Device::new(&dir, "bo", 9);
    bo.publish_keys().await;
    bo.stock(3).await;
    let mut phone = dm_device::Device::new(&dir, &matt, 3);
    phone.publish_keys().await;
    let (_, me) = bo.call("GET", "me", Value::Null).await;
    let (status, shown) = bo.call("PUT", "me/profile", json!({"name": "Bo", "revision": me["profile"]["revision"]})).await;
    assert_eq!(status, 200, "{shown}");
    let (_, code) = bo.call("POST", "me/friend-id/reveal", Value::Null).await;
    let (status, sent) = phone.call("POST", "requests", json!({"code": code["code"], "action_id": "action-000000000000000001"})).await;
    assert_eq!(status, 202, "{sent}");
    let request = sent["receipt"]["id"].as_str().expect("a receipt");
    let (status, contact) = bo.call("POST", &format!("requests/{request}/accept"), Value::Null).await;
    assert_eq!(status, 200, "{contact}");
    let conversation = contact["conversation_id"].as_str().expect("a conversation").to_owned();
    let generation = contact["generation"].as_i64().expect("a generation");

    // Mike may send there.
    let proposal = json!({
        "agent": "Mike",
        "app": "dms",
        "scopes": ["dm.read", "dm.send"],
        "resources": [{"kind": "conversation", "id": conversation}],
        "reason": "Answer Bo for me.",
    });
    call(&mut client, &mut view, "rivet", "app_grants_propose", &proposal).await;
    let seq =
        view.entries.iter().rev().find_map(|entry| matches!(entry.kind, Some(Kind::AppGrant(_))).then_some(entry.seq)).expect("a card");
    let decision = v1::DecideAppGrant { agent_id: "rivet".into(), seq, approve: true, comment: String::new(), ..Default::default() };
    apply_request(&mut client, &mut view, Request::DecideAppGrant(decision)).await;
    view.follow(&mut client, |view| view.agent("mike").extensions.iter().any(|extension| extension.id == "dm" && extension.enabled)).await;
    select(&mut client, &mut view, "mike").await;
    view.follow(&mut client, |view| idle(view, "mike")).await;

    // No device has given this Claktory its channel yet: nothing is proposed.
    let args = json!({"conversation": conversation, "text": "Too soon"});
    let said = call(&mut client, &mut view, "mike", "dm_send", &args).await;
    assert_eq!(said, "The person hasn't finished giving this Claktory the chat yet.");
    assert!(view.workspace.dm_sends.is_empty());

    // Matt's phone enrolls this Claktory and makes its channel, and the
    // conversation's group with Bo's device.
    let offer = phone.enroll(&mut client).await;
    let channel = phone.channel(&offer).await;
    until("the leaf never joined its channel", || factoryd::user_apps::dms::kept_groups(&factory) == [(String::new(), String::new())]).await;
    phone.pin(&bo);
    bo.pin(&phone);
    let people = GroupKind::Conversation { people: [matt.clone(), "bo".to_owned()] };
    let group = phone
        .create(people.clone(), json!({"kind": "conversation", "conversation_id": conversation, "contact_generation": generation}))
        .await;
    let packages = phone.claim("bo", &[&bo.device_key]).await;
    phone.add(&group, packages).await;
    assert_eq!(bo.join_welcomes(|_| people.clone()).await, vec![group.clone()]);
    let (mut watcher, _, _) = Client::connect(addr).await;

    // Proposed: it waits in Needs you, with who and to whom, never the text.
    let args = json!({"conversation": conversation, "text": "See you at 6, canary-dm-send"});
    let said = call(&mut client, &mut view, "mike", "dm_send", &args).await;
    assert!(said.starts_with("Proposed your message to \"Bo\" · ") && said.contains("for the person's review"), "{said}");
    let id = proposal_in(&said);
    view.follow(&mut client, |view| view.workspace.dm_sends.len() == 1).await;
    let waiting = view.workspace.dm_sends[0].clone();
    assert_eq!((waiting.proposal_id.as_str(), waiting.agent_id.as_str(), waiting.conversation_id.as_str()), (id.as_str(), "mike", conversation.as_str()));
    assert_eq!(waiting.friend_name, "Bo");
    assert!(!waiting.friend_tag.is_empty() && waiting.expires_ms > waiting.created_ms, "{waiting:?}");
    assert!(!String::from_utf8_lossy(&prost::Message::encode_to_vec(&view.workspace)).contains("canary-dm-send"));
    // It notifies as Needs you, in Claktory's words.
    let note = tokio::time::timeout(std::time::Duration::from_secs(20), async {
        loop {
            if let Some(v1::server_message::Body::Notify(note)) = watcher.receive().await.body {
                return note;
            }
        }
    })
    .await
    .expect("a notification");
    assert_eq!((note.title.as_str(), note.body.as_str(), note.agent_id.as_str()), ("Mike", "Wants to send a message to a friend", "mike"));
    // The text is nowhere but sealed: not at the service, not in factory.db.
    for file in ["apps.db", "home/factory.db"] {
        let bytes = std::fs::read(dir.join(file)).unwrap_or_default();
        assert!(!String::from_utf8_lossy(&bytes).contains("canary-dm-send"), "the text is in {file}");
    }

    // Matt's phone opens it: from this Claktory's certified leaf, bound to
    // the conversation, its generation and Mike.
    let shown = phone.proposals().await;
    assert_eq!(shown.len(), 1);
    assert!(shown[0].get("text").is_none(), "{}", shown[0]);
    let opened = phone.open(&channel, &shown[0]);
    assert_eq!((opened.from.leaf.kind, claktory_of(&opened.from.leaf.leaf_id)), (LeafKind::Claktory, Some(offer.claktory_id.as_str())));
    assert_eq!((opened.content.kind.as_str(), opened.content.text.as_deref()), ("proposal", Some("See you at 6, canary-dm-send")));
    assert_eq!(
        (opened.extra("conversation_id"), opened.extra("contact_generation"), opened.extra("agent_id"), opened.extra("agent_name"), opened.extra("sequence")),
        (&json!(conversation), &json!(generation), &json!("mike"), &json!("Mike"), &json!(1))
    );
    assert!(opened.extra("incarnation").as_str().is_some_and(|incarnation| !incarnation.is_empty()));

    // Changed while it waits: the same proposal, a new revision and sequence.
    let args = json!({"conversation": conversation, "text": "See you at 7", "replaces": id});
    let said = call(&mut client, &mut view, "mike", "dm_send", &args).await;
    assert!(said.starts_with("Changed your message") && proposal_in(&said) == id, "{said}");
    let (status, stale) = phone.approve(&group, &opened).await;
    assert_eq!(status, 409, "what the phone opened isn't what waits now: {stale}");
    let shown = phone.proposals().await;
    let changed = phone.open(&channel, &shown[0]);
    assert_eq!((changed.content.text.as_deref(), changed.extra("sequence")), (Some("See you at 7"), &json!(2)));

    // Matt's tablet joins the channel: it couldn't open what was sent
    // before, so Mike's waiting proposal comes again, for every device.
    let mut tablet = phone.another(4).await;
    // The leaf trusts only the lists Matt's devices gave it: first the new one.
    phone.share_pins(&channel, 1).await;
    bo.pins.accept_list(&matt, &phone.signed_list).expect("Matt's new list");
    let packages = phone.claim(&matt, &[&tablet.device_key]).await;
    phone.add(&channel, packages).await;
    let kind = GroupKind::Channel { account: matt.clone(), claktory_id: offer.claktory_id.clone() };
    assert_eq!(tablet.join_welcomes(|_| kind.clone()).await, vec![channel.clone()]);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    let shown = loop {
        let shown = tablet.proposals().await;
        if shown.first().is_some_and(|shown| shown["revision"].as_i64() == Some(3)) {
            break shown;
        }
        assert!(std::time::Instant::now() < deadline, "never sent again: {shown:?}");
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    };
    let reposted = tablet.open(&channel, &shown[0]);
    assert_eq!((reposted.content.text.as_deref(), reposted.extra("sequence")), (Some("See you at 7"), &json!(3)));
    assert_eq!(reposted.shown["id"], json!(id), "the same proposal");

    // Matt approves exactly what his tablet shows: it encrypts it to Bo
    // itself, and Mike is told.
    let in_group = group_for(&mut tablet, &mut phone, &group, &people).await;
    let (status, sent) = tablet.approve(&in_group, &reposted).await;
    assert_eq!(status, 200, "{sent}");
    view.follow(&mut client, |view| view.workspace.dm_sends.is_empty() && told(view, "The person approved your message").is_some()).await;
    let event = told(&view, "The person approved your message").expect("told");
    assert!(event.contains("to \"Bo\" · ") && event.contains(&format!("(proposal {id}, message #")), "{event}");
    assert!(!event.contains("See you"), "never the text: {event}");
    let read = bo.catch_up(&group).await;
    let message = read.iter().find_map(|(_, content)| content.clone()).expect("Bo got it");
    let by = message.by_agent.clone().expect("by an agent");
    assert_eq!((message.text.as_deref(), by.agent_id.as_str(), by.name.as_str(), by.claktory_id.as_str()), (Some("See you at 7"), "mike", "Mike", offer.claktory_id.as_str()));

    // Taken back while it waits: it leaves Needs you, nothing is sent.
    view.follow(&mut client, |view| idle(view, "mike")).await;
    let said = call(&mut client, &mut view, "mike", "dm_send", &json!({"conversation": conversation, "text": "Actually, 8"})).await;
    let second = proposal_in(&said);
    view.follow(&mut client, |view| view.workspace.dm_sends.len() == 1).await;
    let said = call(&mut client, &mut view, "mike", "dm_withdraw", &json!({"proposal": second})).await;
    assert_eq!(said, "You took it back: it won't be sent.");
    view.follow(&mut client, |view| view.workspace.dm_sends.is_empty()).await;
    let said = call(&mut client, &mut view, "mike", "dm_withdraw", &json!({"proposal": second})).await;
    assert!(said.starts_with("That isn't one of your proposals still waiting"), "{said}");

    // Declined on Matt's device: Mike is told, nothing is sent.
    let said = call(&mut client, &mut view, "mike", "dm_send", &json!({"conversation": conversation, "text": "Or 9?"})).await;
    let third = proposal_in(&said);
    let (status, declined) = phone.call("POST", &format!("proposals/{third}/decline"), json!({})).await;
    assert_eq!(status, 200, "{declined}");
    view.follow(&mut client, |view| view.workspace.dm_sends.is_empty() && told(view, "The person declined your message").is_some()).await;
    assert!(bo.catch_up(&group).await.iter().all(|(_, content)| content.is_none()), "only the approved one reached Bo");

    // Mike's access to send ends while one waits (a new grant replaces it):
    // it leaves Needs you, and he's told it wasn't sent.
    let said = call(&mut client, &mut view, "mike", "dm_send", &json!({"conversation": conversation, "text": "Last one"})).await;
    let fourth = proposal_in(&said);
    view.follow(&mut client, |view| view.workspace.dm_sends.len() == 1).await;
    select(&mut client, &mut view, "rivet").await;
    let narrower = json!({
        "agent": "Mike",
        "app": "dms",
        "scopes": ["dm.read"],
        "resources": [{"kind": "conversation", "id": conversation}],
        "reason": "Only read now.",
    });
    call(&mut client, &mut view, "rivet", "app_grants_propose", &narrower).await;
    let seq =
        view.entries.iter().rev().find_map(|entry| matches!(entry.kind, Some(Kind::AppGrant(_))).then_some(entry.seq)).expect("a card");
    let decision = v1::DecideAppGrant { agent_id: "rivet".into(), seq, approve: true, comment: String::new(), ..Default::default() };
    apply_request(&mut client, &mut view, Request::DecideAppGrant(decision)).await;
    select(&mut client, &mut view, "mike").await;
    view.follow(&mut client, |view| {
        view.workspace.dm_sends.is_empty() && told(view, "Your access to these messages ended, so the message wasn't sent").is_some()
    })
    .await;
    assert!(told(&view, "Your access").expect("told").contains(&fourth));

    // Nothing that ended keeps its text, even sealed.
    let kept: i64 = rusqlite::Connection::open(dir.join("home/dm/leaf.db"))
        .expect("the leaf's store")
        .query_row("SELECT COUNT(*) FROM proposals", [], |row| row.get(0))
        .expect("counted");
    assert_eq!(kept, 0);
}

/// The conversation's group, for the tablet: the phone adds it there.
async fn group_for(tablet: &mut dm_device::Device, phone: &mut dm_device::Device, group: &str, people: &claktory_dm_crypto::validation::GroupKind) -> String {
    let packages = phone.claim(&tablet.account.clone(), &[&tablet.device_key.clone()]).await;
    phone.add(group, packages).await;
    let joined = tablet.join_welcomes(|_| people.clone()).await;
    assert_eq!(joined, [group.to_owned()]);
    group.to_owned()
}
