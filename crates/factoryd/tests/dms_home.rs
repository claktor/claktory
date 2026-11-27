//! The person's devices and their home Claktory (#451 H3): a device hands
//! its MLS messages to the home over its own connection (DmSend), follows
//! its groups there (DmFollow: what it missed, then each new entry as it
//! comes), lets the home forget what it kept (DmAck) and gives it the
//! friends' homes to route to (DmRoutes, answered with DmHome). Over real
//! MLS groups of the person's devices, against the worker for the commits.
//!
//! A file of its own: direct messages are a process-wide preview until
//! People ships.
//!
//! DMs reach the user-apps worker over a Unix socket: not on Windows.
#![cfg(unix)]

#[path = "common/mod.rs"]
mod common;
#[path = "common/dm_device.rs"]
mod dm_device;

use std::os::unix::fs::MetadataExt as _;
use std::path::Path;
use std::sync::Arc;

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use claktory_account::server::{self, Config, Secrets, Service, UserApps, mail};
use claktory_dm_crypto::content::Content;
use claktory_dm_crypto::group::{Group, Incoming};
use claktory_dm_crypto::validation::GroupKind;
use claktory_user_apps::worker::{self, Worker};
use common::{Client, last_workspace, start, temp_dir};
use dm_device::{Device, envelope};
use factory_protocol::v1::{self, client_message::Body as Request, server_message::Body};
use serde_json::json;

/// The first seq a home gives a group's entries.
const FIRST: u64 = (1 << 32) + 1;

/// An account service with its user-apps worker; its sent mail.
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

async fn sign_in(client: &mut Client, mail: &mail::Memory, email: &str) -> v1::Workspace {
    client.ok(Request::AccountSignIn(v1::AccountSignIn { email: email.into() })).await;
    let sent = mail.sent().into_iter().rev().find(|sent| sent.to == email).expect("an email");
    let code = sent.subject.rsplit(' ').next().expect("a code").to_owned();
    let events = client.ok(Request::AccountVerify(v1::AccountVerify { code })).await;
    last_workspace(&events).expect("workspace").clone()
}

/// The group's state on this device.
fn load(device: &Device, group: &str) -> (GroupKind, Vec<u8>) {
    (device.groups.get(group).expect("its group").clone(), URL_SAFE_NO_PAD.decode(group).expect("an id"))
}

/// Seals `text` in the group on `device`, as the app does before DmSend.
fn seal(device: &Device, group: &str, id: &str, text: &str) -> Vec<u8> {
    let (kind, group_id) = load(device, group);
    let content = Content::text(id, text).encode().expect("encoded");
    device.with(|provider, me| {
        Group::load(provider, kind, &group_id).expect("loads").expect("kept").send(provider, me, &device.pins, &content).expect("sealed")
    })
}

/// What `device` reads in an entry: the text of a friend's or another
/// device's message.
fn open(device: &Device, group: &str, bytes: &[u8]) -> Option<String> {
    let (kind, group_id) = load(device, group);
    device.with(|provider, _| {
        match Group::load(provider, kind, &group_id).expect("loads").expect("kept").process(provider, &device.pins, bytes) {
            Ok(Incoming::Application { content, .. }) => Content::decode(&content).expect("content").text,
            _ => None,
        }
    })
}

/// An update commit, taken by the worker and merged: its bytes, for the
/// home.
async fn update(device: &mut Device, group: &str) -> Vec<u8> {
    let (kind, group_id) = load(device, group);
    let out = device.with(|provider, me| {
        Group::load(provider, kind.clone(), &group_id).expect("loads").expect("kept").update(provider, me).expect("an update")
    });
    let body = json!({"commit": envelope(out.commit.clone()), "group_info": envelope(out.group_info.expect("a GroupInfo"))});
    let (status, taken) = device.call("POST", &format!("groups/{group}/commits"), body).await;
    assert_eq!(status, 200, "{taken}");
    device
        .with(|provider, _| Group::load(provider, kind, &group_id).expect("loads").expect("kept").merge_pending(provider).expect("merged"));
    out.commit
}

async fn send(client: &mut Client, leaf: &str, group: &[u8], envelope: Vec<u8>, id: &str) -> Result<v1::DmReceipt, String> {
    let request = v1::DmSend { group_id: group.to_vec(), envelope, client_message_id: id.to_owned(), leaf_id: leaf.to_owned() };
    match client.request(Request::DmSend(request)).await.body {
        Body::DmFeed(v1::DmFeed { sent: Some(receipt), .. }) => Ok(receipt),
        Body::Error(error) => Err(error.message),
        other => panic!("expected a receipt, got {other:?}"),
    }
}

fn result(receipt: &v1::DmReceipt) -> v1::DmSendResult {
    v1::DmSendResult::try_from(receipt.result).expect("a result")
}

async fn follow(client: &mut Client, leaf: &str, cursors: Vec<v1::DmCursor>) -> Result<v1::DmFeed, String> {
    match client.request(Request::DmFollow(v1::DmFollow { leaf_id: leaf.to_owned(), cursors })).await.body {
        Body::DmFeed(feed) => Ok(feed),
        Body::Error(error) => Err(error.message),
        other => panic!("expected a feed, got {other:?}"),
    }
}

/// The entries pushed to `client` until a page says there are no more.
async fn pushed(client: &mut Client) -> Vec<v1::DmEntry> {
    let mut entries = Vec::new();
    loop {
        let feed = client.wait_for(|body| if let Body::DmFeed(feed) = body { Some(feed.clone()) } else { None }).await;
        entries.extend(feed.entries);
        if !feed.has_more {
            return entries;
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn the_persons_devices_send_follow_and_ack_through_their_home() {
    factoryd::user_apps::dms::set_preview(true);
    let dir = temp_dir();
    let (url, mail) = service(&dir).await;
    std::fs::create_dir_all(dir.join("home")).expect("home");
    std::fs::write(dir.join("home/config.json"), json!({"account": {"enabled": true, "service": url}}).to_string()).expect("config");
    let (addr, factory) = start(&dir).await;
    let (mut phone_link, _, _) = Client::connect(addr).await;
    sign_in(&mut phone_link, &mail, "matt@example.com").await;
    let saved: serde_json::Value =
        serde_json::from_slice(&std::fs::read(dir.join("home/account/account.json")).expect("account.json")).expect("JSON");
    let account = saved["account_id"].as_str().expect("an account").to_owned();

    // Matt's phone and iPad in their self group, at epoch 1.
    let mut phone = Device::new(&dir, &account, 1);
    phone.publish_keys().await;
    let mut ipad = phone.another(2).await;
    let group = phone.create(GroupKind::SelfGroup { account: account.clone() }, json!({"kind": "self"})).await;
    let packages = phone.claim(&account, &[&ipad.device_key]).await;
    phone.add(&group, packages).await;
    assert_eq!(ipad.join_welcomes(|_| GroupKind::SelfGroup { account: account.clone() }).await, vec![group.clone()]);
    let group_id = URL_SAFE_NO_PAD.decode(&group).expect("an id");
    let (phone_leaf, ipad_leaf) = (phone.device_key.clone(), ipad.device_key.clone());
    let hello = seal(&phone, &group, "message-000000000000000001", "Hello from my phone");

    // Not the home yet: nothing taken, nothing followed.
    let receipt = send(&mut phone_link, &phone_leaf, &group_id, hello.clone(), "message-000000000000000001").await.expect("a receipt");
    assert_eq!(result(&receipt), v1::DmSendResult::NotHome);
    let (mut ipad_link, _, _) = Client::connect(addr).await;
    assert!(follow(&mut ipad_link, &ipad_leaf, Vec::new()).await.expect_err("refused").contains("isn't your home"));

    // Its home now, holding the group as claktory.com has it.
    factoryd::dm_device::set_home_for_test(&factory, true);
    let (kind, _) = load(&phone, &group);
    let info = phone.with(|provider, me| {
        Group::load(provider, kind, &group_id).expect("loads").expect("kept").group_info(provider, me).expect("a GroupInfo")
    });
    factoryd::dm_device::seed_for_test(&factory, &info).expect("seeded");
    let home = match phone_link.request(Request::DmRoutes(v1::DmRoutes { routes: Vec::new() })).await.body {
        Body::DmHome(home) => home,
        other => panic!("expected the home, got {other:?}"),
    };
    assert!(home.is_home);
    assert_eq!(home.endpoint_id.len(), 32);
    assert!(home.peers.is_empty() && !home.intake_paused);

    // The iPad follows: its group, nothing to catch up on.
    let feed = follow(&mut ipad_link, &ipad_leaf, Vec::new()).await.expect("followed");
    assert_eq!(feed.groups.len(), 1, "{feed:?}");
    assert_eq!((feed.groups[0].group_id.as_slice(), feed.groups[0].epoch, feed.groups[0].behind), (group_id.as_slice(), 1, false));
    assert!(feed.entries.is_empty() && !feed.has_more);

    // The phone sends; the home keeps it once, under that id.
    let receipt = send(&mut phone_link, &phone_leaf, &group_id, hello.clone(), "message-000000000000000001").await.expect("a receipt");
    assert_eq!((result(&receipt), receipt.seq), (v1::DmSendResult::Stored, FIRST), "{receipt:?}");
    assert!(receipt.accepted_ms > 0);
    let again = send(&mut phone_link, &phone_leaf, &group_id, hello.clone(), "message-000000000000000001").await.expect("a receipt");
    assert_eq!((result(&again), again.seq), (v1::DmSendResult::Duplicate, FIRST));
    let other = seal(&phone, &group, "message-000000000000000001", "Something else");
    let conflict = send(&mut phone_link, &phone_leaf, &group_id, other, "message-000000000000000001").await.expect("a receipt");
    assert_eq!(result(&conflict), v1::DmSendResult::Conflict);
    let refused = send(&mut phone_link, &phone_leaf, &group_id, hello.clone(), "short").await.expect_err("refused");
    assert!(refused.contains("client_message_id"), "{refused}");

    // The iPad hears it as it comes, and reads it.
    let entries = pushed(&mut ipad_link).await;
    assert_eq!(entries.len(), 1, "{entries:?}");
    let entry = &entries[0];
    assert_eq!((entry.seq, entry.kind(), entry.epoch, entry.from_me), (FIRST, v1::DmEntryKind::Application, 1, true));
    assert_eq!(entry.client_message_id, "", "only the sending leaf sees its id");
    assert_eq!(open(&ipad, &group, &entry.envelope).as_deref(), Some("Hello from my phone"));

    // A commit claktory.com took goes to the home too, and on to the iPad.
    let commit = update(&mut phone, &group).await;
    let receipt = send(&mut phone_link, &phone_leaf, &group_id, commit, "").await.expect("a receipt");
    assert_eq!((result(&receipt), receipt.seq), (v1::DmSendResult::Stored, FIRST + 1), "{receipt:?}");
    let entries = pushed(&mut ipad_link).await;
    assert_eq!((entries.len(), entries[0].kind(), entries[0].epoch), (1, v1::DmEntryKind::Commit, 1));
    open(&ipad, &group, &entries[0].envelope);

    // Away for a while: it reads on from its cursor when it comes back.
    drop(ipad_link);
    for n in 2..=4 {
        let id = format!("message-{n:018}");
        let sealed = seal(&phone, &group, &id, &format!("Message {n}"));
        let receipt = send(&mut phone_link, &phone_leaf, &group_id, sealed, &id).await.expect("a receipt");
        assert_eq!(result(&receipt), v1::DmSendResult::Stored);
    }
    let (mut ipad_link, _, _) = Client::connect(addr).await;
    let cursor = v1::DmCursor { group_id: group_id.clone(), seq: FIRST + 1 };
    let feed = follow(&mut ipad_link, &ipad_leaf, vec![cursor]).await.expect("followed");
    assert!(feed.has_more, "{feed:?}");
    assert_eq!(feed.groups[0].last_seq, FIRST + 4);
    let entries = pushed(&mut ipad_link).await;
    assert_eq!(entries.iter().map(|entry| entry.seq).collect::<Vec<_>>(), vec![FIRST + 2, FIRST + 3, FIRST + 4]);
    let read: Vec<Option<String>> = entries.iter().map(|entry| open(&ipad, &group, &entry.envelope)).collect();
    assert_eq!(read, vec![Some("Message 2".to_owned()), Some("Message 3".to_owned()), Some("Message 4".to_owned())]);

    // It kept them all; a cursor from before the home counts for nothing,
    // so it reads on from where it acked.
    let through = vec![v1::DmCursor { group_id: group_id.clone(), seq: FIRST + 4 }];
    ipad_link.ok(Request::DmAck(v1::DmAck { leaf_id: ipad_leaf.clone(), through })).await;
    let (mut ipad_link, _, _) = Client::connect(addr).await;
    let feed = follow(&mut ipad_link, &ipad_leaf, vec![v1::DmCursor { group_id: group_id.clone(), seq: 3 }]).await.expect("followed");
    assert!(!feed.has_more, "{feed:?}");

    // A leaf sends at most 60 a minute.
    for n in 0..60 {
        let refused = send(&mut phone_link, "busy-leaf", &group_id, vec![0; 8], &format!("message-{n:018}")).await;
        assert!(refused.is_err(), "not an MLS message");
    }
    let limited = send(&mut phone_link, "busy-leaf", &group_id, hello, "message-000000000000000099").await.expect("a receipt");
    assert_eq!(result(&limited), v1::DmSendResult::RateLimited);
    assert!(limited.retry_after_ms >= 1000, "{limited:?}");

    // Routes: friends' homes only, never this Claktory itself.
    let own = v1::DmRoute { account_id: "bo".into(), home_endpoint: home.endpoint_id.clone(), group_ids: vec![group_id.clone()] };
    match phone_link.request(Request::DmRoutes(v1::DmRoutes { routes: vec![own] })).await.body {
        Body::Error(error) => assert!(error.message.contains("itself"), "{}", error.message),
        other => panic!("expected a refusal, got {other:?}"),
    }
    let bo = v1::DmRoute { account_id: "bo".into(), home_endpoint: vec![0xb0; 32], group_ids: vec![group_id.clone()] };
    let home = match phone_link.request(Request::DmRoutes(v1::DmRoutes { routes: vec![bo] })).await.body {
        Body::DmHome(home) => home,
        other => panic!("expected the home, got {other:?}"),
    };
    assert_eq!(home.peers.len(), 1, "{home:?}");
    assert_eq!((home.peers[0].account_id.as_str(), home.peers[0].home_endpoint.as_slice()), ("bo", [0xb0; 32].as_slice()));
}

/// Row 548: a device that asked how the home is doing before the person's
/// list named this Claktory hears it once the list watcher places it,
/// without asking again; so does a connection that never asked.
#[tokio::test(flavor = "multi_thread")]
async fn the_persons_devices_hear_when_their_list_makes_this_claktory_the_home() {
    factoryd::user_apps::dms::set_preview(true);
    let dir = temp_dir();
    let (url, mail) = service(&dir).await;
    std::fs::create_dir_all(dir.join("home")).expect("home");
    std::fs::write(dir.join("home/config.json"), json!({"account": {"enabled": true, "service": url}}).to_string()).expect("config");
    let (addr, factory) = start(&dir).await;
    let (mut phone_link, _, _) = Client::connect(addr).await;
    sign_in(&mut phone_link, &mail, "matt@example.com").await;
    let saved: serde_json::Value =
        serde_json::from_slice(&std::fs::read(dir.join("home/account/account.json")).expect("account.json")).expect("JSON");
    let account = saved["account_id"].as_str().expect("an account").to_owned();

    // Matt's phone enrolls this Claktory, and asks: not the home yet.
    let mut phone = Device::new(&dir, &account, 1);
    phone.publish_keys().await;
    phone.enroll(&mut phone_link).await;
    let home = match phone_link.request(Request::DmRoutes(v1::DmRoutes { routes: Vec::new() })).await.body {
        Body::DmHome(home) => home,
        other => panic!("expected the home, got {other:?}"),
    };
    assert!(!home.is_home);
    let (mut mac_link, _, _) = Client::connect(addr).await;

    // His list names it his home, and the phone gives its routes, as the app
    // does: the Mac, which never asked, hears once this Claktory places
    // itself, and so does the phone, whatever its answer said.
    phone.home = Some(factoryd::dm_device::endpoint_for_test(&factory).expect("an endpoint"));
    phone.publish_keys().await;
    phone_link.send(v1::ClientMessage { id: 1000, body: Some(Request::DmRoutes(v1::DmRoutes { routes: Vec::new() })) }).await;
    for link in [&mut mac_link, &mut phone_link] {
        let home = link.wait_for(|body| if let Body::DmHome(home) = body { home.is_home.then(|| home.clone()) } else { None }).await;
        assert_eq!(home.endpoint_id.len(), 32);
    }
}
