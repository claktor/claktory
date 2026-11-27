//! Direct messages between two Claktories, end to end (#451 H8): Matt's and
//! Bo's homes, each its person's, signed in to one account service, find
//! each other on a relay and an address lookup on this machine, as on the
//! internet. Their phones send and read only through their own home, and
//! no application message ever reaches the service. Both homes away, then
//! back; a message from a stale epoch; Matt's home moved to another of his
//! Claktories; and a flood, capped.
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
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use claktory_account::server::{self, Config, Secrets, Service, UserApps, mail};
use claktory_dm_crypto::group::Group;
use claktory_dm_crypto::validation::GroupKind;
use claktory_user_apps::worker::{self, Worker};
use common::{Client, start, temp_dir};
use dm_device::Device;
use factory_protocol::v1::{self, client_message::Body as Request, server_message::Body};
use factoryd::factory::Factory;
use serde_json::{Value, json};

const WAIT: Duration = Duration::from_secs(90);

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

/// The account service, its mail and the network the homes meet on.
struct World {
    root: PathBuf,
    url: String,
    mail: Arc<mail::Memory>,
    network: dm_device::Network,
}

/// A Claktory of someone's.
struct Claktory {
    factory: Arc<Factory>,
    addr: std::net::SocketAddr,
    client: Client,
}

/// Someone with a phone and their home Claktory.
struct Person {
    home: Claktory,
    phone: Device,
}

impl World {
    async fn new() -> World {
        factoryd::user_apps::dms::set_preview(true);
        let root = temp_dir();
        let (url, mail) = service(&root).await;
        World { root, url, mail, network: dm_device::Network::start().await }
    }

    /// A Claktory in its own folder `name`, signed in as `email`: its
    /// account.
    async fn claktory(&self, name: &str, email: &str) -> (Claktory, String) {
        let dir = self.root.join(name);
        std::fs::create_dir_all(dir.join("home")).expect("home");
        std::fs::write(dir.join("home/config.json"), json!({"account": {"enabled": true, "service": self.url}}).to_string())
            .expect("config");
        let (addr, factory) = start(&dir).await;
        self.network.serve(&factory);
        let (mut client, _, _) = Client::connect(addr).await;
        client.ok(Request::AccountSignIn(v1::AccountSignIn { email: email.into() })).await;
        let sent = self.mail.sent().into_iter().rev().find(|sent| sent.to == email).expect("an email");
        let code = sent.subject.rsplit(' ').next().expect("a code").to_owned();
        client.ok(Request::AccountVerify(v1::AccountVerify { code })).await;
        let saved: Value =
            serde_json::from_slice(&std::fs::read(dir.join("home/account/account.json")).expect("account.json")).expect("JSON");
        (Claktory { factory, addr, client }, saved["account_id"].as_str().expect("an account").to_owned())
    }

    /// Someone signed in on a Claktory of their own, which their phone
    /// enrolls and names their home.
    async fn person(&self, name: &str, seed: u8) -> Person {
        let (mut home, account) = self.claktory(name, &format!("{name}@example.com")).await;
        let mut phone = Device::new(&self.root, &account, seed);
        phone.publish_keys().await;
        phone.stock(3).await;
        phone.enroll(&mut home.client).await;
        move_home(&self.network, &mut phone, &mut home).await;
        Person { home, phone }
    }
}

/// `phone`'s list names `to` its person's home, which hears it is.
async fn move_home(network: &dm_device::Network, phone: &mut Device, to: &mut Claktory) {
    let deadline = Instant::now() + WAIT;
    let endpoint = loop {
        if let Some(endpoint) = factoryd::dm_device::endpoint_for_test(&to.factory) {
            break endpoint;
        }
        assert!(Instant::now() < deadline, "the Claktory never had an endpoint");
        tokio::time::sleep(Duration::from_millis(100)).await;
    };
    phone.home = Some(endpoint);
    phone.publish_keys().await;
    phone.set_home_addr(to.addr);
    while !home_state(&mut to.client).await.is_home {
        assert!(Instant::now() < deadline, "the Claktory never heard it's the home");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let id = iroh::EndpointId::from_bytes(&endpoint).expect("an endpoint id");
    network.lookup.on_endpoint(&id, WAIT).await.expect("the home is published");
}

/// The home's state, as a device asks it.
async fn home_state(client: &mut Client) -> v1::DmHome {
    match client.request(Request::DmRoutes(v1::DmRoutes { routes: Vec::new() })).await.body {
        Body::DmHome(home) => home,
        other => panic!("expected the home, got {other:?}"),
    }
}

/// The home's state, as `device` asks it with the routes it gave.
async fn routed_state(client: &mut Client, device: &Device) -> v1::DmHome {
    match client.request(Request::DmRoutes(v1::DmRoutes { routes: device.routes.clone() })).await.body {
        Body::DmHome(home) => home,
        other => panic!("expected the home, got {other:?}"),
    }
}

/// `friend` (calling itself `name`) and `person` become friends: their
/// conversation and its generation.
async fn befriend(person: &Device, friend: &Device, name: &str, n: u32) -> (String, i64) {
    let (_, me) = friend.call("GET", "me", Value::Null).await;
    let (status, shown) = friend.call("PUT", "me/profile", json!({"name": name, "revision": me["profile"]["revision"]})).await;
    assert_eq!(status, 200, "{shown}");
    let (status, code) = friend.call("POST", "me/friend-id/reveal", Value::Null).await;
    assert_eq!(status, 200, "{code}");
    let ask = json!({"code": code["code"], "action_id": format!("action-{n:018}")});
    let (status, sent) = person.call("POST", "requests", ask).await;
    assert_eq!(status, 202, "{sent}");
    let request = sent["receipt"]["id"].as_str().expect("a receipt");
    let (status, contact) = friend.call("POST", &format!("requests/{request}/accept"), Value::Null).await;
    assert_eq!(status, 200, "{contact}");
    (contact["conversation_id"].as_str().expect("a conversation").to_owned(), contact["generation"].as_i64().expect("a generation"))
}

/// Matt and Bo, friends, with their conversation's group made by Matt's
/// phone, Bo's phone in it, and each home routed to the other's for it.
async fn conversation(world: &World) -> (Person, Person, String) {
    let mut matt = world.person("matt", 3).await;
    let mut bo = world.person("bo", 9).await;
    let (conversation, generation) = befriend(&matt.phone, &bo.phone, "Bo", 1).await;
    matt.phone.pin(&bo.phone);
    bo.phone.pin(&matt.phone);
    let kind = GroupKind::Conversation { people: [matt.phone.account.clone(), bo.phone.account.clone()] };
    let body = json!({"kind": "conversation", "conversation_id": conversation, "contact_generation": generation});
    let group = matt.phone.create(kind.clone(), body).await;
    let packages = matt.phone.claim(&bo.phone.account.clone(), &[&bo.phone.device_key.clone()]).await;
    matt.phone.add(&group, packages).await;
    assert_eq!(bo.phone.join_welcomes(|_| kind.clone()).await, std::slice::from_ref(&group));
    matt.phone.route(&bo.phone, &[&group]).await;
    bo.phone.route(&matt.phone, &[&group]).await;
    (matt, bo, group)
}

/// Reads on at `device`'s home until it has read `text`.
async fn hears(device: &mut Device, group: &str, text: &str) {
    let deadline = Instant::now() + WAIT;
    while !heard(device, text) {
        assert!(Instant::now() < deadline, "{} never read {text:?}", device.account);
        device.catch_up(group).await;
        if !heard(device, text) {
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
    }
}

fn heard(device: &Device, text: &str) -> bool {
    device.opened.values().any(|(_, content)| content.text.as_deref() == Some(text))
}

/// How many application messages the service keeps: none, ever (#451).
fn kept_at_the_service(world: &World) -> i64 {
    let db = rusqlite::Connection::open(world.root.join("apps.db")).expect("the worker's store");
    db.query_row("SELECT COUNT(*) FROM dm_mailbox WHERE kind = 'application'", [], |row| row.get(0)).expect("counted")
}

/// Matt and Bo talk through their homes; then both homes go away, each
/// person writes meanwhile, and once both are back each reads the other.
/// The service never holds a message.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn two_homes_carry_a_conversation_and_catch_up_after_both_were_away() {
    let world = World::new().await;
    let (mut matt, mut bo, group) = conversation(&world).await;

    matt.phone.post(&group, "", 0, 1, "Hi Bo").await;
    hears(&mut bo.phone, &group, "Hi Bo").await;
    bo.phone.post(&group, "", 0, 1, "Hi Matt").await;
    hears(&mut matt.phone, &group, "Hi Matt").await;

    // Both homes away: each takes its own person's message and keeps it.
    factoryd::dm_device::set_offline_for_test(&matt.home.factory, true);
    factoryd::dm_device::set_offline_for_test(&bo.home.factory, true);
    matt.phone.post(&group, "", 0, 2, "Are you there?").await;
    bo.phone.post(&group, "", 0, 2, "Back soon").await;
    tokio::time::sleep(Duration::from_secs(2)).await;
    bo.phone.catch_up(&group).await;
    assert!(!heard(&bo.phone, "Are you there?"), "nothing crosses while both are away");

    // Bo's comes back first, finds Matt's still away; then Matt's.
    factoryd::dm_device::set_offline_for_test(&bo.home.factory, false);
    tokio::time::sleep(Duration::from_secs(3)).await;
    factoryd::dm_device::set_offline_for_test(&matt.home.factory, false);
    hears(&mut bo.phone, &group, "Are you there?").await;
    hears(&mut matt.phone, &group, "Back soon").await;

    assert_eq!(kept_at_the_service(&world), 0, "no message ever reached the service");
}

/// Bo's phone, six commits behind, writes: its home won't take a message
/// from that far back and says the epoch to write at. Caught up, it seals
/// the message again and Matt reads it, never the stale one.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_message_from_a_stale_epoch_is_sealed_again_and_arrives() {
    let world = World::new().await;
    let (mut matt, mut bo, group) = conversation(&world).await;
    matt.phone.post(&group, "", 0, 1, "Hi Bo").await;
    hears(&mut bo.phone, &group, "Hi Bo").await;
    let before = bo.phone.epoch(&group);
    for _ in 0..6 {
        matt.phone.commit(&group, |group: &mut Group, provider, me, _| group.update(provider, me).expect("an update")).await;
    }
    let now = matt.phone.epoch(&group);
    assert_eq!(now, before + 6);

    // Bo's home takes Matt's commits from the service (his home's hints,
    // or the service's word) and is at Matt's epoch.
    let deadline = Instant::now() + WAIT;
    while bo.phone.home_epoch(&group).await != Some(now) {
        assert!(Instant::now() < deadline, "Bo's home never took Matt's commits");
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    let (stale, id) = bo.phone.seal(&group, 1, "Sorry, I was away");
    let receipt = bo.phone.send_once(&group, stale.clone(), &id).await;
    assert_eq!(receipt.result(), v1::DmSendResult::TooOld, "{receipt:?}");
    assert_eq!(receipt.epoch, now, "{receipt:?}");

    // Caught up, the phone seals it again, under a new id.
    bo.phone.catch_up(&group).await;
    assert_eq!(bo.phone.epoch(&group), now);
    bo.phone.post(&group, "", 0, 2, "Sorry, I was away").await;
    hears(&mut matt.phone, &group, "Sorry, I was away").await;
    let stale_digest = claktory_dm_crypto::envelope::Envelope::new(stale).expect("an envelope").digest();
    let stale_hex: String = stale_digest.iter().map(|byte| format!("{byte:02x}")).collect();
    assert!(!matt.phone.opened.contains_key(&stale_hex), "the stale one never arrived");
    assert_eq!(kept_at_the_service(&world), 0);
}

/// Matt makes another of his Claktories his home: his phone's new list
/// names it, Bo's phone takes the list in and routes Bo's home there.
/// What was waiting when it moved isn't lost: what Bo's home kept for the
/// old one, and what the old one kept for Bo's (both away then), reach
/// their people through the new home. What either says from then on goes
/// through it too.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_moved_home_takes_the_conversation_over() {
    let world = World::new().await;
    let (mut matt, mut bo, group) = conversation(&world).await;
    matt.phone.post(&group, "", 0, 1, "Hi Bo").await;
    hears(&mut bo.phone, &group, "Hi Bo").await;

    // Bo's home is away when Matt writes: Matt's old home keeps it. Then
    // the old home is away too when Bo writes: Bo's keeps that.
    factoryd::dm_device::set_offline_for_test(&bo.home.factory, true);
    let (packing, id) = matt.phone.seal(&group, 2, "Packing up");
    matt.phone.send(&group, packing.clone(), &id).await;
    factoryd::dm_device::set_offline_for_test(&matt.home.factory, true);
    bo.phone.post(&group, "", 0, 1, "Did you move?").await;

    // Matt's Mac mini: signed in, enrolled, named his home.
    let (mut mini, _) = world.claktory("matt-mini", "matt@example.com").await;
    matt.phone.enroll(&mut mini.client).await;
    move_home(&world.network, &mut matt.phone, &mut mini).await;
    matt.phone.positions.remove(&group);
    matt.phone.route(&bo.phone, &[&group]).await;
    bo.phone.refresh(&matt.phone);
    bo.phone.route(&matt.phone, &[&group]).await;

    // Bo's home back: what it kept for the old home goes to the new one.
    factoryd::dm_device::set_offline_for_test(&bo.home.factory, false);
    hears(&mut matt.phone, &group, "Did you move?").await;
    // The old one back, no longer the home: it hands what it kept for
    // Bo's home to the new one, and dials nothing else.
    factoryd::dm_device::set_offline_for_test(&matt.home.factory, false);
    let mini_id = matt.phone.home.expect("the new home").to_vec();
    let deadline = Instant::now() + WAIT;
    loop {
        let old = home_state(&mut matt.home.client).await;
        assert!(old.peers.iter().all(|peer| peer.home_endpoint == mini_id), "{old:?}");
        if !old.is_home {
            break;
        }
        assert!(Instant::now() < deadline, "the old home never heard it isn't any more");
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    hears(&mut bo.phone, &group, "Packing up").await;
    let digest = claktory_dm_crypto::envelope::Envelope::new(packing).expect("an envelope").digest().to_vec();
    let bo_home = rusqlite::Connection::open(world.root.join("bo/home/dm/home.db")).expect("Bo's home");
    let times: i64 = bo_home.query_row("SELECT COUNT(*) FROM log WHERE digest = ?1", [&digest], |row| row.get(0)).expect("counted");
    assert_eq!(times, 1, "handed on exactly once");

    bo.phone.post(&group, "", 0, 2, "Nice place").await;
    hears(&mut matt.phone, &group, "Nice place").await;
    matt.phone.post(&group, "", 0, 3, "Thanks").await;
    hears(&mut bo.phone, &group, "Thanks").await;
    assert_eq!(kept_at_the_service(&world), 0);
}

/// Row 574: a device's place is in the numbering of the home it read.
/// Matt's phone read his old home well into its log, then he moves his home
/// to his Mac mini: the mini numbers past every seq the old home gave, so
/// what comes there next reaches the phone from the place it had, with no
/// reset (Zucchero saw new messages count as read).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_device_reads_the_new_home_on_from_its_place_at_the_old_one() {
    let world = World::new().await;
    let (mut matt, mut bo, group) = conversation(&world).await;
    for n in 1..=3 {
        let text = format!("Before the move {n}");
        bo.phone.post(&group, "", 0, n, &text).await;
        hears(&mut matt.phone, &group, &text).await;
    }
    let before = matt.phone.positions[&group];
    assert!(before >= (1 << 32) + 3, "past the first seqs a home gives: {before}");

    let (mut mini, _) = world.claktory("matt-mini", "matt@example.com").await;
    matt.phone.enroll(&mut mini.client).await;
    move_home(&world.network, &mut matt.phone, &mut mini).await;
    matt.phone.route(&bo.phone, &[&group]).await;
    bo.phone.refresh(&matt.phone);
    bo.phone.route(&matt.phone, &[&group]).await;

    bo.phone.post(&group, "", 0, 4, "After the move").await;
    hears(&mut matt.phone, &group, "After the move").await;
    assert!(matt.phone.positions[&group] > before);
    matt.phone.post(&group, "", 0, 1, "Settled in").await;
    hears(&mut bo.phone, &group, "Settled in").await;
}

/// Row 595 (Fugo 41473): Bo moves his home while every device of Matt's is
/// off, so none gives Matt's home the new route. Bo's new home shows Matt's
/// home Bo's signed device list naming it; Matt's home takes it under the
/// pin Matt's devices gave its leaf, so what Bo says reaches it, and what
/// Matt's home had queued for Bo's old home goes to the new one.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_friends_home_takes_their_new_home_from_their_signed_list() {
    let world = World::new().await;
    let (mut matt, mut bo, group) = conversation(&world).await;
    // Matt's Claktory's leaf has Bo's pin from Matt's phone.
    matt.phone.enroll(&mut matt.home.client).await;
    matt.phone.post(&group, "", 0, 1, "Hi Bo").await;
    hears(&mut bo.phone, &group, "Hi Bo").await;

    // Bo's old home is off when Matt writes: Matt's home keeps it for it.
    factoryd::dm_device::set_offline_for_test(&bo.home.factory, true);
    matt.phone.post(&group, "", 0, 2, "Are you there?").await;

    // Bo's Mac mini becomes his home; only Bo's phone hears of it.
    let (mut mini, _) = world.claktory("bo-mini", "bo@example.com").await;
    bo.phone.enroll(&mut mini.client).await;
    move_home(&world.network, &mut bo.phone, &mut mini).await;
    bo.phone.route(&matt.phone, &[&group]).await;

    bo.phone.post(&group, "", 0, 2, "Moved in").await;
    hears(&mut matt.phone, &group, "Moved in").await;
    hears(&mut bo.phone, &group, "Are you there?").await;
    // Matt's home routes Bo's group to the new home now, from that list.
    let mini_endpoint: String = bo.phone.home.expect("Bo's new home").iter().map(|byte| format!("{byte:02x}")).collect();
    let matt_home = rusqlite::Connection::open(world.root.join("matt/home/dm/home.db")).expect("Matt's home");
    let routed: Vec<String> = {
        let mut statement = matt_home.prepare("SELECT endpoint FROM routes WHERE account = ?1").expect("prepared");
        statement.query_map([&bo.phone.account], |row| row.get(0)).expect("routes").collect::<Result<_, _>>().expect("read")
    };
    assert_eq!(routed, vec![mini_endpoint.clone()]);
    let taken: (String, i64) = matt_home
        .query_row("SELECT endpoint, version FROM friend_homes WHERE account = ?1", [&bo.phone.account], |row| {
            Ok((row.get(0)?, row.get(1)?))
        })
        .expect("the floor kept");
    assert_eq!(taken.0, mini_endpoint);
    matt.phone.post(&group, "", 0, 3, "Welcome").await;
    hears(&mut bo.phone, &group, "Welcome").await;
}

/// Matt moves his home while Bo's phone hasn't read his new list yet
/// (Zucchero's row 567: its app was closed when the list changed). Bo's
/// home still sends to Matt's old one, which isn't his home any more and
/// turns it away: Bo's devices hear that, not that it's offline, and once
/// Bo's phone follows the list the message reaches Matt's new home.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_friends_home_that_turns_this_one_away_says_so() {
    let world = World::new().await;
    let (mut matt, mut bo, group) = conversation(&world).await;
    matt.phone.post(&group, "", 0, 1, "Hi Bo").await;
    hears(&mut bo.phone, &group, "Hi Bo").await;
    let old = matt.phone.home.expect("Matt's home").to_vec();

    let (mut mini, _) = world.claktory("matt-mini", "matt@example.com").await;
    matt.phone.enroll(&mut mini.client).await;
    move_home(&world.network, &mut matt.phone, &mut mini).await;
    matt.phone.positions.remove(&group);
    matt.phone.route(&bo.phone, &[&group]).await;
    let deadline = Instant::now() + WAIT;
    while home_state(&mut matt.home.client).await.is_home {
        assert!(Instant::now() < deadline, "the old home never heard it isn't any more");
        tokio::time::sleep(Duration::from_millis(200)).await;
    }

    // Bo writes, his home still routed to the old one.
    bo.phone.post(&group, "", 0, 1, "Where are you?").await;
    let deadline = Instant::now() + WAIT;
    loop {
        let state = routed_state(&mut bo.home.client, &bo.phone).await;
        let peer = state.peers.iter().find(|peer| peer.home_endpoint == old).expect("Matt's old home, still routed");
        if peer.refused == "turned_away" {
            assert!(!peer.reachable, "{peer:?}");
            assert_eq!(peer.outbox_count, 1, "{peer:?}");
            break;
        }
        assert!(Instant::now() < deadline, "Bo's home never said the old one turns it away: {peer:?}");
        tokio::time::sleep(Duration::from_millis(200)).await;
    }

    // Bo's phone follows Matt's list: the message goes to the new home.
    bo.phone.refresh(&matt.phone);
    bo.phone.route(&matt.phone, &[&group]).await;
    hears(&mut matt.phone, &group, "Where are you?").await;
    let mini_id = matt.phone.home.expect("the new home").to_vec();
    let state = routed_state(&mut bo.home.client, &bo.phone).await;
    assert!(!state.peers.is_empty(), "{state:?}");
    assert!(state.peers.iter().all(|peer| peer.home_endpoint == mini_id && peer.refused.is_empty()), "{state:?}");
    assert_eq!(kept_at_the_service(&world), 0);
}

/// Bo moves his home while his tablet hasn't read Matt's message
/// (Zucchero's row 567), nor the one Bo's phone sent (row 591): the old
/// home passes both on to the new one, which serves them to Bo's leaves
/// only. The tablet, already past a commit it made
/// after the message, reads it there once; Bo's phone, which read it at the
/// old home, never opens it twice. Once taken, it leaves the old home's
/// queue.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_moved_home_passes_on_what_the_persons_devices_hadnt_read() {
    let world = World::new().await;
    let (mut matt, mut bo, group) = conversation(&world).await;
    let mut tablet = bo.phone.another(11).await;
    matt.phone.refresh(&bo.phone);
    let packages = bo.phone.claim(&bo.phone.account.clone(), &[&tablet.device_key.clone()]).await;
    bo.phone.add(&group, packages).await;
    let kind = GroupKind::Conversation { people: [matt.phone.account.clone(), bo.phone.account.clone()] };
    assert_eq!(tablet.join_welcomes(|_| kind.clone()).await, std::slice::from_ref(&group));
    let deadline = Instant::now() + WAIT;
    while matt.phone.epoch(&group) != bo.phone.epoch(&group) {
        assert!(Instant::now() < deadline, "Matt's phone never took the tablet's add");
        matt.phone.catch_up(&group).await;
        tokio::time::sleep(Duration::from_millis(200)).await;
    }

    // Matt writes; Bo's phone reads it and says so, the tablet doesn't. The
    // tablet commits after it, so it reads it later from an earlier epoch.
    matt.phone.post(&group, "", 0, 1, "Before you move").await;
    hears(&mut bo.phone, &group, "Before you move").await;
    bo.phone.ack(&group).await;
    bo.phone.post(&group, "", 0, 1, "Sent while the tablet was off").await;
    hears(&mut matt.phone, &group, "Sent while the tablet was off").await;
    tablet.commit(&group, |group: &mut Group, provider, me, _| group.update(provider, me).expect("an update")).await;

    // Bo's Mac mini becomes his home; his phone routes it to Matt's.
    let (mut mini, _) = world.claktory("bo-mini", "bo@example.com").await;
    bo.phone.enroll(&mut mini.client).await;
    move_home(&world.network, &mut bo.phone, &mut mini).await;
    bo.phone.route(&matt.phone, &[&group]).await;
    matt.phone.refresh(&bo.phone);
    matt.phone.route(&bo.phone, &[&group]).await;

    // The tablet reads both at the new home: Bo's own as his.
    tablet.positions.remove(&group);
    hears(&mut tablet, &group, "Before you move").await;
    hears(&mut tablet, &group, "Sent while the tablet was off").await;
    let (from, _) =
        tablet.opened.values().find(|(_, content)| content.text.as_deref() == Some("Sent while the tablet was off")).expect("opened");
    assert_eq!((from.leaf.account_id.as_str(), from.leaf.leaf_id.as_str()), (bo.phone.account.as_str(), bo.phone.device_key.as_str()));
    let digest = tablet
        .opened
        .iter()
        .find(|(_, (_, content))| content.text.as_deref() == Some("Before you move"))
        .map(|(digest, _)| digest.clone())
        .expect("opened");
    // Bo's phone gets it there too, and opens nothing twice.
    bo.phone.positions.remove(&group);
    let read = bo.phone.catch_up(&group).await;
    assert!(!read.iter().any(|(_, content)| content.as_ref().is_some_and(|content| content.text.as_deref() == Some("Before you move"))));

    // Once at the new home, out of the old one's queue; at the new home once.
    let unhex = |hex: &str| (0..hex.len()).step_by(2).map(|at| u8::from_str_radix(&hex[at..at + 2], 16).expect("hex")).collect::<Vec<u8>>();
    let old = rusqlite::Connection::open(world.root.join("bo/home/dm/home.db")).expect("Bo's old home");
    let deadline = Instant::now() + WAIT;
    while old.query_row("SELECT COUNT(*) FROM pass_on", [], |row| row.get::<_, i64>(0)).expect("counted") > 0 {
        assert!(Instant::now() < deadline, "the old home never let go of what it passed on");
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    let new = rusqlite::Connection::open(world.root.join("bo-mini/home/dm/home.db")).expect("Bo's new home");
    let times: i64 = new.query_row("SELECT COUNT(*) FROM log WHERE digest = ?1", [unhex(&digest)], |row| row.get(0)).expect("counted");
    assert_eq!(times, 1, "passed on exactly once");
    let passed: i64 = new.query_row("SELECT COUNT(*) FROM passed_in", [], |row| row.get(0)).expect("counted");
    assert_eq!(passed, 2, "both marked, never to be passed on again");
    let own_rows = "SELECT COUNT(*), COALESCE(SUM(mine), 0) FROM log WHERE account = ?1 AND kind = 'application'";
    let own: (i64, i64) = new.query_row(own_rows, [&bo.phone.account], |row| Ok((row.get(0)?, row.get(1)?))).expect("counted");
    assert_eq!(own, (1, 0), "Bo's own, as a friend's passed message lands");
}

/// Matt's phone commits while it's away from his home (Zucchero's row
/// 568): only claktory.com has the commit, and nothing comes after it. Both
/// homes hear of it from claktory.com's feed and pull it, so Bo's phone,
/// reading only Bo's home, takes it and reads what Matt says next.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_commit_made_away_from_the_home_reaches_both_homes() {
    let world = World::new().await;
    let (mut matt, mut bo, group) = conversation(&world).await;
    matt.phone.post(&group, "", 0, 1, "Hi Bo").await;
    hears(&mut bo.phone, &group, "Hi Bo").await;

    matt.phone.commit_away(&group, |group: &mut Group, provider, me, _| group.update(provider, me).expect("an update")).await;
    let now = matt.phone.epoch(&group);
    let deadline = Instant::now() + WAIT;
    while matt.phone.home_epoch(&group).await != Some(now) || bo.phone.home_epoch(&group).await != Some(now) {
        assert!(Instant::now() < deadline, "a home never took the commit made away from Matt's");
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    bo.phone.catch_up(&group).await;
    assert_eq!(bo.phone.epoch(&group), now, "Bo's phone took it from his home");
    matt.phone.post(&group, "", 0, 2, "Back on my phone").await;
    hears(&mut bo.phone, &group, "Back on my phone").await;
    assert_eq!(kept_at_the_service(&world), 0);
}

/// Bo floods Matt. Bo's own home takes 60 a minute from his phone and
/// says when to try again; Matt's home takes 60 a minute from Cy's home,
/// a friend's with no Claktory in the test, and answers busy after. Matt
/// reads what was taken.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_flood_is_capped_at_both_homes() {
    let world = World::new().await;
    let (mut matt, mut bo, group) = conversation(&world).await;
    matt.phone.post(&group, "", 0, 1, "Hi Bo").await;
    hears(&mut bo.phone, &group, "Hi Bo").await;

    let mut stored = 0;
    let mut limited = None;
    for n in 1..=61 {
        let (bytes, id) = bo.phone.seal(&group, n, &format!("flood {n}"));
        let receipt = bo.phone.send_once(&group, bytes, &id).await;
        match receipt.result() {
            v1::DmSendResult::Stored => stored += 1,
            v1::DmSendResult::RateLimited => {
                limited = Some(receipt);
                break;
            }
            other => panic!("unexpected {other:?}: {receipt:?}"),
        }
    }
    assert_eq!(stored, 60);
    let limited = limited.expect("the 61st is held back");
    assert!(limited.retry_after_ms > 0, "{limited:?}");
    hears(&mut matt.phone, &group, "flood 60").await;
    assert!(!heard(&matt.phone, "flood 61"));

    // Cy's home floods Matt's straight over the link.
    let mut cy = Device::new(&world.root, "cy", 21);
    cy.friend_home = Some(dm_device::FriendHome::start(&world.network).await);
    cy.publish_keys().await;
    cy.stock(2).await;
    let (conversation, generation) = befriend(&matt.phone, &cy, "Cy", 2).await;
    matt.phone.pin(&cy);
    cy.pin(&matt.phone);
    let kind = GroupKind::Conversation { people: [matt.phone.account.clone(), "cy".to_owned()] };
    let body = json!({"kind": "conversation", "conversation_id": conversation, "contact_generation": generation});
    let with_cy = matt.phone.create(kind.clone(), body).await;
    let packages = matt.phone.claim("cy", &[&cy.device_key.clone()]).await;
    matt.phone.add(&with_cy, packages).await;
    cy.join_welcomes(|_| kind.clone()).await;
    matt.phone.route(&cy, &[&with_cy]).await;
    let home = cy.friend_home.clone().expect("Cy's home");
    let mut codes = Vec::new();
    for n in 1..=61 {
        let (bytes, _) = cy.seal(&with_cy, n, &format!("cy {n}"));
        codes.push(home.deliver_once(&with_cy, &bytes).await.unwrap_or_default());
    }
    assert!(codes[..60].iter().all(|code| code == "took" || code == "held"), "{codes:?}");
    assert_eq!(codes[60], "busy");
    hears(&mut matt.phone, &with_cy, "cy 60").await;
    assert!(!heard(&matt.phone, "cy 61"));
    assert_eq!(kept_at_the_service(&world), 0);
}
