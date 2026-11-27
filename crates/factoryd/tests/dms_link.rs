//! The link between homes, `claktory-dm/1` (#451 H2): the person's home
//! hears only the hellos of their own Claktories (Fugo 32512). A stranger,
//! a hello replayed from another endpoint, a leaf the current list no
//! longer names and a device's leaf are all turned away with only a close;
//! a Claktory once bound comes back without a hello, even while the hello
//! limits are spent, until a list without it drops it and its connection.
//! Over a relay and an address lookup on this machine, as on the internet.
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
use std::time::Duration;

use claktory_account::server::{self, Config, Secrets, Service, UserApps, mail};
use claktory_dm_crypto::certificate::LeafKind;
use claktory_dm_crypto::device_list::Listed;
use claktory_dm_crypto::group::LeafKey;
use claktory_user_apps::worker::{self, Worker};
use common::{Client, start, temp_dir};
use dm_device::{Device, b64};
use factory_protocol::v1::{self, client_message::Body as Request, server_message::Body};
use iroh::endpoint::{Connection, presets};
use iroh::test_utils::{DnsPkarrServer, run_relay_server};
use iroh::{Endpoint, EndpointAddr, EndpointId, RelayMode, RelayUrl};
use serde_json::{Value, json};
use tokio::io::AsyncReadExt as _;

const WAIT: Duration = Duration::from_secs(20);

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

async fn sign_in(client: &mut Client, mail: &mail::Memory, email: &str) {
    client.ok(Request::AccountSignIn(v1::AccountSignIn { email: email.into() })).await;
    let sent = mail.sent().into_iter().rev().find(|sent| sent.to == email).expect("an email");
    let code = sent.subject.rsplit(' ').next().expect("a code").to_owned();
    client.ok(Request::AccountVerify(v1::AccountVerify { code })).await;
}

/// An address lookup on this machine, in place of n0's.
type Lookup = Arc<DnsPkarrServer>;

/// Another endpoint on the same relay and lookup: one of the person's
/// Claktories, or anyone.
async fn endpoint_on(relay: &RelayUrl, lookup: &Lookup) -> Endpoint {
    Endpoint::builder(presets::Minimal)
        .secret_key(iroh::SecretKey::generate())
        .ca_tls_config(iroh::tls::CaTlsConfig::insecure_skip_verify())
        .relay_mode(RelayMode::Custom(relay.clone().into()))
        .preset(lookup.preset())
        .bind()
        .await
        .expect("an endpoint")
}

async fn dial(from: &Endpoint, home: EndpointId) -> Connection {
    let connecting = from.connect(EndpointAddr::new(home), b"claktory-dm/1");
    tokio::time::timeout(WAIT, connecting).await.expect("in time").expect("the gate lets it try")
}

/// One request on a stream of its own: the reply's code, or None when the
/// home answered nothing (closed, or no answer in time).
async fn ask(connection: &Connection, request: &Value) -> Option<String> {
    let talk = async {
        let (mut send, mut receive) = connection.open_bi().await.ok()?;
        let body = request.to_string().into_bytes();
        send.write_all(&(body.len() as u32).to_be_bytes()).await.ok()?;
        send.write_all(&body).await.ok()?;
        send.finish().ok()?;
        let length = receive.read_u32().await.ok()? as usize;
        let mut reply = vec![0; length];
        receive.read_exact(&mut reply).await.ok()?;
        let reply: Value = serde_json::from_slice(&reply).ok()?;
        reply["code"].as_str().map(str::to_owned)
    };
    tokio::time::timeout(WAIT, talk).await.ok().flatten()
}

/// Whether the home closed `connection` within the wait.
async fn closed(connection: &Connection) -> bool {
    tokio::time::timeout(WAIT, connection.closed()).await.is_ok()
}

fn bytes_of(id: EndpointId) -> [u8; 32] {
    *id.as_bytes()
}

/// The home's state, as a device asks it.
async fn home_state(client: &mut Client) -> v1::DmHome {
    match client.request(Request::DmRoutes(v1::DmRoutes { routes: Vec::new() })).await.body {
        Body::DmHome(home) => home,
        other => panic!("expected the home, got {other:?}"),
    }
}

/// Waits until `done`.
async fn until(what: &str, mut done: impl AsyncFnMut() -> bool) {
    let deadline = tokio::time::Instant::now() + WAIT;
    while !done().await {
        assert!(tokio::time::Instant::now() < deadline, "{what}");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_home_hears_only_the_persons_own_claktories() {
    factoryd::user_apps::dms::set_preview(true);
    let (_, relay, _relay) = run_relay_server().await.expect("a relay");
    let lookup: Lookup = Arc::new(DnsPkarrServer::run().await.expect("an address lookup"));
    let dir = temp_dir();
    let (url, mail) = service(&dir).await;
    std::fs::create_dir_all(dir.join("home")).expect("home");
    std::fs::write(dir.join("home/config.json"), json!({"account": {"enabled": true, "service": url}}).to_string()).expect("config");
    let (addr, factory) = start(&dir).await;
    let lookup_for = lookup.clone();
    factory.set_tunnel_network(vec![relay.to_string()], move |builder| {
        builder.preset(lookup_for.preset()).ca_tls_config(iroh::tls::CaTlsConfig::insecure_skip_verify())
    });
    let (mut link, _, _) = Client::connect(addr).await;
    sign_in(&mut link, &mail, "matt@example.com").await;
    let saved: Value = serde_json::from_slice(&std::fs::read(dir.join("home/account/account.json")).expect("account.json")).expect("JSON");
    let account = saved["account_id"].as_str().expect("an account").to_owned();

    // Matt's phone lists two other Claktories of his (a Mac and a PC), then
    // only the Mac and this one, enrolled; then names this one his home.
    let mut phone = Device::new(&dir, &account, 1);
    let (mac_key, pc_key) = (LeafKey::generate().expect("a key"), LeafKey::generate().expect("a key"));
    let (mac, pc) = (b64(&[0x4d; 32]), b64(&[0x50; 32]));
    phone.listed.push(Listed { kind: LeafKind::Claktory, leaf_id: mac.clone(), signature_key: mac_key.public() });
    phone.listed.push(Listed { kind: LeafKind::Claktory, leaf_id: pc.clone(), signature_key: pc_key.public() });
    phone.publish_keys().await;
    phone.listed.retain(|listed| listed.leaf_id != pc);
    phone.enroll(&mut link).await;
    let home = factoryd::dm_device::endpoint_for_test(&factory).expect("an endpoint");
    phone.home = Some(home);
    phone.publish_keys().await;
    until("this Claktory hears it's the home", async || home_state(&mut link).await.is_home).await;
    let home_id = EndpointId::from_bytes(&home).expect("an endpoint id");
    lookup.on_endpoint(&home_id, WAIT).await.expect("the home is published");

    let hello = |key: &LeafKey, from: EndpointId, leaf: &str| {
        let at = factoryd::transcript::now_ms();
        factoryd::dm_device::hello_for_test(key, (&home, &bytes_of(from)), at, &account, leaf)
    };

    // A stranger saying anything else: closed, no answer.
    let stranger = endpoint_on(&relay, &lookup).await;
    let connection = dial(&stranger, home_id).await;
    assert_eq!(ask(&connection, &json!({"kind": "knock"})).await, None);
    assert!(closed(&connection).await);
    // Refused, as the gate refuses a key: a friend's home that dialed this
    // one can tell it's turned away, not offline (row 567).
    let refused = matches!(
        connection.close_reason(),
        Some(iroh::endpoint::ConnectionError::ApplicationClosed(close)) if close.error_code == 1u32.into()
    );
    assert!(refused, "{:?}", connection.close_reason());
    // One saying nothing at all: closed after its 5 seconds.
    let connection = dial(&stranger, home_id).await;
    assert!(closed(&connection).await, "no hello, no link");

    // The Mac's hello, from its own endpoint: bound.
    let mac_link = endpoint_on(&relay, &lookup).await;
    let said = hello(&mac_key, mac_link.id(), &mac);
    let connection = dial(&mac_link, home_id).await;
    assert_eq!(ask(&connection, &said).await.as_deref(), Some("ok"));
    // Replayed from another endpoint: the signature names the Mac's.
    let thief = endpoint_on(&relay, &lookup).await;
    let stolen = dial(&thief, home_id).await;
    assert_eq!(ask(&stolen, &said).await, None);
    assert!(closed(&stolen).await);
    // A device's leaf, signing its own hello: not a Claktory.
    let device_link = endpoint_on(&relay, &lookup).await;
    let device = dial(&device_link, home_id).await;
    let as_device = hello(&phone.me.key, device_link.id(), &phone.device_key);
    assert_eq!(ask(&device, &as_device).await, None);
    assert!(closed(&device).await);
    // A field the hello doesn't have: read strictly, turned away.
    let mut padded = hello(&pc_key, device_link.id(), &pc);
    padded["extra"] = json!(1);
    let device = dial(&device_link, home_id).await;
    assert_eq!(ask(&device, &padded).await, None);

    // The PC, on an older list but not the current one: turned away.
    let pc_link = endpoint_on(&relay, &lookup).await;
    let connection = dial(&pc_link, home_id).await;
    assert_eq!(ask(&connection, &hello(&pc_key, pc_link.id(), &pc)).await, None);

    // Every hello of the minute spent (30 in all): a new one isn't heard,
    // and the Mac, bound already, still comes in without one.
    let spender = endpoint_on(&relay, &lookup).await;
    for _ in 0..30 {
        let connection = dial(&spender, home_id).await;
        let _ = ask(&connection, &json!({"kind": "hello"})).await;
    }
    let late = endpoint_on(&relay, &lookup).await;
    let connection = dial(&late, home_id).await;
    assert_eq!(ask(&connection, &hello(&mac_key, late.id(), &mac)).await, None, "past the minute's hellos");
    let again = dial(&mac_link, home_id).await;
    assert_eq!(ask(&again, &json!({"kind": "knock"})).await.as_deref(), Some("not_routed"), "heard, with no groups yet");

    // A list without the Mac lets go of it, and of its connection.
    phone.listed.retain(|listed| listed.leaf_id != mac);
    phone.publish_keys().await;
    assert!(closed(&again).await, "its link closes");
    let after = dial(&mac_link, home_id).await;
    assert_eq!(ask(&after, &json!({"kind": "knock"})).await, None, "a stranger again");
}
