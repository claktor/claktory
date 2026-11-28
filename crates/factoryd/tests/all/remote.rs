//! Devices reach Factory through its private tunnel (iroh): pairing, keys
//! and tokens, the gate after the handshake, Unpair; and this machine's app,
//! through its local pairing code.
//!
//! The tests run on several threads: the tunnel's endpoints (Claktory's and
//! the devices') keep exchanging packets while anything else in the test
//! blocks a thread, so a busy machine (the whole suite in parallel) can't
//! starve a connection into its idle timeout (ConnectionLost(TimedOut)).

use std::net::SocketAddr;
use std::path::Path;
use std::time::Duration;

use crate::common::{Client, View, apply_request, home, start, temp_dir};
use factory_protocol::PROTOCOL_VERSION;
use factory_protocol::v1::{self, ClientMessage, ErrorCode, ServerMessage, client_message, server_message::Body};
use futures_util::{SinkExt, StreamExt};
use iroh::endpoint::{Connection, ConnectionError, RecvStream, SendStream, presets};
use iroh::{Endpoint, EndpointAddr, EndpointId, RelayMode};
use prost::Message as _;
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::tungstenite::Message;

const WAIT: Duration = Duration::from_secs(10);

fn now_ms() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).expect("time").as_millis() as i64
}

/// What a pairing code holds.
#[derive(Debug)]
pub(crate) struct Code {
    pub(crate) token: String,
    pub(crate) endpoint: EndpointId,
    relays: Vec<String>,
    pub(crate) addresses: Vec<SocketAddr>,
}

fn decode(text: &str) -> String {
    let mut out = Vec::new();
    let bytes = text.as_bytes();
    let mut at = 0;
    while at < bytes.len() {
        if bytes[at] == b'%' {
            out.push(u8::from_str_radix(&text[at + 1..at + 3], 16).expect("hex"));
            at += 3;
        } else {
            out.push(bytes[at]);
            at += 1;
        }
    }
    String::from_utf8(out).expect("utf-8")
}

pub(crate) fn parse(code: &str) -> Code {
    let query = code.trim().strip_prefix("factory://?").expect("factory://? (no host: only the tunnel)");
    let values =
        |name: &str| -> Vec<String> { query.split('&').filter_map(|pair| pair.strip_prefix(&format!("{name}="))).map(decode).collect() };
    Code {
        token: values("t").pop().expect("t="),
        endpoint: values("e").pop().expect("e=").parse().expect("an endpoint id"),
        relays: values("r"),
        addresses: values("a").iter().map(|address| address.parse().expect("ip:port")).collect(),
    }
}

/// The four digits a device waiting to be allowed shows, worked out as the
/// protocol says (PendingDevice.number), from its side.
pub(crate) fn number(code: &Code, device: &str) -> String {
    use sha2::Digest as _;
    let token: String = sha2::Sha256::digest(code.token.as_bytes()).iter().map(|byte| format!("{byte:02x}")).collect();
    let digest = sha2::Sha256::digest(format!("claktory pairing\n{}\n{device}\n{token}", code.endpoint).as_bytes());
    format!("{:04}", u32::from_be_bytes(digest[..4].try_into().expect("4 bytes")) % 10_000)
}

/// A device: a key of its own, no relays.
pub(crate) async fn new_device() -> Endpoint {
    Endpoint::builder(presets::Minimal).relay_mode(RelayMode::Disabled).bind().await.expect("device endpoint")
}

/// Connects to Factory at the code's addresses; Err with why when it's
/// turned away (or nothing answers).
pub(crate) async fn dial(device: &Endpoint, code: &Code, addresses: &[SocketAddr]) -> Result<Connection, String> {
    dial_offering(device, code, addresses, &[factoryd::remote::ALPN]).await
}

/// Dials offering the ALPNs given, in order of preference.
async fn dial_offering(device: &Endpoint, code: &Code, addresses: &[SocketAddr], alpns: &[&[u8]]) -> Result<Connection, String> {
    let mut addr = EndpointAddr::new(code.endpoint);
    for address in addresses {
        addr = addr.with_ip_addr(*address);
    }
    let options = iroh::endpoint::ConnectOptions::new().with_additional_alpns(alpns[1..].iter().map(|alpn| alpn.to_vec()).collect());
    let dial = async {
        device
            .connect_with_opts(addr, alpns[0], options)
            .await
            .map_err(|error| format!("{error:#}"))?
            .await
            .map_err(|error| format!("{error:#}"))
    };
    let connection = tokio::time::timeout(WAIT, dial).await.map_err(|_| "timed out".to_owned())??;
    // A connection the gate turns away is closed just after the handshake.
    match tokio::time::timeout(Duration::from_millis(300), connection.closed()).await {
        Ok(ConnectionError::ApplicationClosed(close)) => Err(String::from_utf8_lossy(&close.reason).into_owned()),
        Ok(other) => Err(other.to_string()),
        Err(_) => Ok(connection),
    }
}

pub(crate) type Socket = WebSocketStream<tokio::io::Join<RecvStream, SendStream>>;

/// A client on the loopback port, as the CLI and this machine's app when
/// the tunnel fails connect.
async fn loopback(addr: SocketAddr) -> WebSocketStream<tokio::net::TcpStream> {
    let tcp = tokio::net::TcpStream::connect(addr).await.expect("connect");
    tokio_tungstenite::client_async(format!("ws://{addr}/"), tcp).await.expect("websocket").0
}

/// A client on a new stream of the connection, as the app's tunnel makes one.
pub(crate) async fn open(connection: &Connection) -> Socket {
    let (send, receive) = connection.open_bi().await.expect("stream");
    let (socket, _) = tokio_tungstenite::client_async("ws://factory/", tokio::io::join(receive, send)).await.expect("websocket");
    socket
}

/// Says hello with `token`; returns the answer.
pub(crate) async fn hello<S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin>(socket: &mut WebSocketStream<S>, token: &str) -> Body {
    hello_as(socket, token, "").await
}

/// `hello` from a device that gives its own name.
pub(crate) async fn hello_as<S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin>(
    socket: &mut WebSocketStream<S>,
    token: &str,
    name: &str,
) -> Body {
    let hello = v1::Hello {
        protocol_version: PROTOCOL_VERSION,
        client_version: "test".into(),
        token: token.into(),
        device_kind: v1::DeviceKind::Phone.into(),
        workspace_patches: false,
        device_name: name.into(),
        platform: "ios".into(),
        ..Default::default()
    };
    let message = ClientMessage { id: 1, body: Some(client_message::Body::Hello(hello)) };
    socket.send(Message::Binary(message.encode_to_vec().into())).await.expect("send");
    next(socket).await.expect("an answer")
}

/// The next message; None once the connection closes.
pub(crate) async fn next<S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin>(socket: &mut WebSocketStream<S>) -> Option<Body> {
    loop {
        match tokio::time::timeout(WAIT, socket.next()).await.expect("in time") {
            Some(Ok(Message::Binary(bytes))) => return ServerMessage::decode(bytes).expect("decode").body,
            Some(Ok(Message::Close(_)) | Err(_)) | None => return None,
            Some(Ok(_)) => {}
        }
    }
}

/// A code that dials this machine's end of the tunnel (its key and
/// 127.0.0.1 address, with no token: this machine's app has none, since
/// it connects over its socket), once it's up.
pub(crate) async fn local_pairing(dir: &Path) -> String {
    for _ in 0..200 {
        if let Some(code) = factoryd::remote::local_code_for_test(&dir.join("home")) {
            return code;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("no loopback endpoint");
}

/// Another device of the person's, paired with a code and allowed, as an
/// app through the private tunnel (from this machine, so remote access
/// can stay off; a relay of its own, so pairing needs no network).
pub(crate) async fn another_device(addr: SocketAddr, name: &str) -> (Client, v1::Workspace) {
    let home = crate::common::home_of(addr);
    let local = parse(&local_pairing(home.root().parent().expect("the test's folder")).await);
    let (mut setup, _, _) = Client::connect(addr).await;
    let relays = v1::SetRemoteAccess { relays: vec!["http://127.0.0.1:9/".into()], set_relays: true, ..Default::default() };
    setup.ok(client_message::Body::SetRemoteAccess(relays)).await;
    let (token, _) = factoryd::remote::pair(&home, name).expect("paired");
    let device = new_device().await;
    let connection = dial(&device, &local, &local.addresses).await.expect("let in");
    let (send, receive) = connection.open_bi().await.expect("stream");
    let mut client = Client::over(Box::new(tokio::io::join(receive, send)), Some(Box::new((device, connection)))).await;
    let hello = v1::Hello { protocol_version: PROTOCOL_VERSION, client_version: "test".into(), token, ..Default::default() };
    client.send(v1::ClientMessage { id: 1, body: Some(client_message::Body::Hello(hello)) }).await;
    assert!(matches!(client.receive().await.body, Some(Body::PairingPending(_))));
    factoryd::remote::approve(&home, name).expect("allowed");
    let (workspace, _) = client.welcomed().await;
    (client, workspace)
}

/// Jotaro 51447: a backend stopping closes its tunnel, so a device sees
/// its connection end at once, not after a timeout.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_stopping_backend_closes_the_tunnel() {
    let dir = temp_dir();
    let (_addr, factory) = start(&dir).await;
    let local = parse(&local_pairing(&dir).await);
    let (token, _) = factoryd::remote::pair(&home(&dir), "Laptop").expect("paired");
    let laptop = new_device().await;
    let connection = dial(&laptop, &local, &local.addresses).await.expect("let in");
    let mut socket = open(&connection).await;
    let waiting = hello(&mut socket, &token).await;
    assert!(matches!(&waiting, Body::PairingPending(_)), "connected, waiting to be allowed: {waiting:?}");
    factory.close_tunnel(std::time::Duration::from_secs(2)).await;
    tokio::time::timeout(std::time::Duration::from_secs(3), connection.closed()).await.expect("closed at once, not timed out");
}

/// This machine's end of the tunnel, with remote access off, is on
/// 127.0.0.1 only and takes no app's token: this machine's app connects over
/// its socket. The loopback port takes only the tool token (agents'
/// commands), welcomed as a tool, never as this machine's app; and a new
/// backend on the same data keeps the tunnel's key and the tool token.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn this_machines_end_of_the_tunnel_takes_no_apps_token_and_the_port_only_the_tool_token() {
    let dir = temp_dir();
    let (addr, factory) = start(&dir).await;
    let written = local_pairing(&dir).await;
    let local = parse(&written);
    assert!(local.relays.is_empty(), "no relays while remote access is off: {written}");
    assert!(matches!(local.addresses.as_slice(), [address] if address.ip().is_loopback()), "{written}");
    assert!(!dir.join("home/remote/local-pairing").exists(), "no pairing code of this machine's app");

    // The loopback port never lets in a client without the token: any user
    // of this machine can reach it. One with none at all (an app from
    // before, whose pairing code is gone) hears only Claktory's version, so
    // it shows that it's too old, and is refused everything.
    let mut stranger = loopback(addr).await;
    let welcome = hello(&mut stranger, "").await;
    assert!(matches!(&welcome, Body::Welcome(welcome) if welcome.device_id.is_empty()), "{welcome:?}");
    match next(&mut stranger).await {
        Some(Body::Workspace(workspace)) => {
            assert_eq!(workspace, v1::Workspace { version: factoryd::update::VERSION.to_owned(), ..v1::Workspace::default() })
        }
        other => panic!("expected only the version, got {other:?}"),
    }
    let refused = ask(&mut stranger, 2, list_home()).await;
    assert!(
        matches!(&refused, Body::Error(error) if error.code() == ErrorCode::Unauthorized && error.message.starts_with("This copy of Claktory is out of date")),
        "{refused:?}"
    );

    // An app dialling from this machine gets nothing without a paired
    // device's token, the tests' person token included.
    let app = new_device().await;
    let connection = dial(&app, &local, &local.addresses).await.expect("this machine reaches its end of the tunnel");
    for token in [String::new(), crate::common::local_token(addr), crate::common::tool_token(addr)] {
        let mut socket = open(&connection).await;
        let answer = hello(&mut socket, &token).await;
        assert!(matches!(&answer, Body::Error(error) if error.code() == ErrorCode::Unauthorized), "{answer:?}");
    }

    // The tool token, on the port only: a tool, not this machine's app.
    let tool = crate::common::tool_token(addr);
    let mut script = loopback(addr).await;
    assert!(matches!(hello(&mut script, &tool).await, Body::Welcome(welcome) if welcome.device_id == factoryd::authority::TOOL_DEVICE));
    let mut wrong_token = loopback(addr).await;
    assert!(matches!(hello(&mut wrong_token, &"0".repeat(64)).await, Body::Error(error) if error.code() == ErrorCode::Unauthorized));
    // Another device's token isn't let in on the port.
    let (token, _) = factoryd::remote::pair(&home(&dir), "Laptop").expect("paired");
    let mut laptop = loopback(addr).await;
    assert!(matches!(hello(&mut laptop, &token).await, Body::Error(error) if error.code() == ErrorCode::Unauthorized));
    factoryd::remote::unpair(&home(&dir), "laptop").expect("unpaired");

    // A new backend on the same data keeps the key and the tool token.
    drop((stranger, script, wrong_token, laptop, connection));
    drop(factory);
    let (addr, _factory) = start(&dir).await;
    let again = parse(&local_pairing(&dir).await);
    assert_eq!(again.endpoint, local.endpoint);
    assert_eq!(crate::common::tool_token(addr), tool);
}

/// On Windows remote access goes through the relays only: listening on a
/// network address would raise the firewall's "Allow access" prompt, which
/// a Standard account can only cancel. So a pairing code names its relay
/// and no address.
#[cfg(windows)]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn on_windows_remote_access_goes_through_the_relays_only() {
    let dir = temp_dir();
    let (addr, _factory) = start(&dir).await;
    let (mut client, workspace, _) = Client::connect(addr).await;
    let mut view = View { workspace, ..View::default() };
    let relay = "http://127.0.0.1:9/";
    let relays = v1::SetRemoteAccess { relays: vec![relay.into()], set_relays: true, ..Default::default() };
    assert!(matches!(apply_request(&mut client, &mut view, client_message::Body::SetRemoteAccess(relays)).await, Body::Ack(_)));
    let pairing = client_message::Body::CreatePairing(v1::CreatePairing { name: "Phone".into(), host: String::new() });
    let Body::PairingCode(made) = apply_request(&mut client, &mut view, pairing).await else { panic!("a pairing code") };
    let code = parse(&made.code);
    assert_eq!(code.relays, [relay], "{}", made.code);
    assert!(code.addresses.is_empty(), "{}", made.code);
    view.follow(&mut client, |view| view.workspace.remote.as_ref().is_some_and(|remote| remote.on)).await;
}

#[cfg_attr(windows, ignore = "its device reaches this machine directly, and on Windows remote access goes through the relays only")]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn paired_devices_connect_through_the_tunnel_unknown_keys_are_turned_away_and_unpair_cuts_them_off() {
    let dir = temp_dir();
    let (addr, factory) = start(&dir).await;
    factory.set_tunnel_online_wait(Duration::from_millis(100));
    let (mut client, workspace, _) = Client::connect(addr).await;
    let mut view = View { workspace, ..View::default() };
    let remote = |view: &View| view.workspace.remote.clone().expect("remote access");
    assert!(!remote(&view).on && remote(&view).devices.is_empty() && !remote(&view).machine.is_empty());
    // A relay of its own (nothing there), so the test needs no network.
    let relay = "http://127.0.0.1:9/";
    let relays = v1::SetRemoteAccess { relays: vec![relay.into()], set_relays: true, ..Default::default() };
    assert!(matches!(apply_request(&mut client, &mut view, client_message::Body::SetRemoteAccess(relays)).await, Body::Ack(_)));
    // The switch from before is harmless: access follows the devices.
    let on = v1::SetRemoteAccess { on: true, ..Default::default() };
    assert!(matches!(apply_request(&mut client, &mut view, client_message::Body::SetRemoteAccess(on)).await, Body::Ack(_)));
    assert!(!remote(&view).on);

    // Pairing turns remote access on; the code goes only to the asker.
    let pairing = client_message::Body::CreatePairing(v1::CreatePairing { name: "Phone".into(), host: String::new() });
    let made = match apply_request(&mut client, &mut view, pairing).await {
        Body::PairingCode(code) => code,
        other => panic!("expected a pairing code, got {other:?}"),
    };
    assert_eq!(made.device_id, "phone");
    assert_eq!(made.link, factoryd::remote::pairing_link(&made.code), "the same code, as a link once links are on");
    let code = parse(&made.code);
    assert_eq!(code.relays, [relay], "{}", made.code);
    assert!(!code.addresses.is_empty() && code.addresses.iter().all(|address| !address.ip().is_loopback()), "{}", made.code);
    view.follow(&mut client, |view| remote(view).on && remote(view).devices.len() == 1).await;
    // This machine's app keeps its code and its port.
    let local = parse(&local_pairing(&dir).await);
    assert!(local.addresses[0].ip().is_loopback() && local.relays.is_empty());
    let shown = remote(&view);
    assert_eq!((shown.relays.as_slice(), shown.custom_relays), ([relay.to_owned()].as_slice(), true));
    assert!(shown.devices[0].code_expires_ms > now_ms() && !shown.devices[0].connected, "{shown:?}");

    // The phone uses the code's token, over the network, and waits for the
    // person to allow it, showing the same number as the app.
    let phone = new_device().await;
    let connection = dial(&phone, &code, &code.addresses).await.expect("a code is waiting: let in");
    let mut socket = open(&connection).await;
    match hello(&mut socket, &code.token).await {
        Body::PairingPending(pending) => {
            assert_eq!((pending.device_id.as_str(), pending.name.as_str()), ("phone", "Phone"));
            assert_eq!(pending.machine, remote(&view).machine);
            assert!(pending.expires_ms > now_ms());
        }
        other => panic!("expected to wait, got {other:?}"),
    }
    view.follow(&mut client, |view| !remote(view).pending_devices.is_empty()).await;
    let asking = remote(&view).pending_devices[0].clone();
    assert_eq!((asking.device_id.as_str(), asking.name.as_str(), asking.kind()), ("phone", "Phone", v1::DeviceKind::Phone));
    // Its digits are never sent, not even to the person's app: they
    // read them on the device, which works them out itself.
    assert_eq!(asking.number, "", "never sent");
    let shown = number(&code, &phone.id().to_string());
    assert!(asking.requested_ms > 0 && asking.expires_ms > now_ms());
    // While it waits, another key that copied the code is turned away.
    let copier = new_device().await;
    let refused = dial(&copier, &code, &code.addresses).await.expect_err("the code is taken");
    assert!(refused.contains("isn't paired"), "{refused}");
    let allow =
        client_message::Body::ApproveDevice(v1::ApproveDevice { device_id: "phone".into(), number: shown.clone(), ..Default::default() });
    assert!(matches!(apply_request(&mut client, &mut view, allow).await, Body::Ack(_)));
    match next(&mut socket).await {
        Some(Body::Welcome(welcome)) => assert_eq!(welcome.device_id, "phone"),
        other => panic!("expected welcome, got {other:?}"),
    }
    view.follow(&mut client, |view| remote(view).pending_devices.is_empty()).await;
    let saved = factoryd::remote::devices(&home(&dir)).expect("devices");
    let bound = saved.iter().find(|device| device.id == "phone").expect("phone");
    assert_eq!((bound.endpoint_id.clone(), bound.code_expires), (phone.id().to_string(), None));
    let file = std::fs::read_to_string(dir.join("home/remote/devices.json")).expect("devices.json");
    assert!(!file.contains(&code.token), "only the token's hash is kept");
    view.follow(&mut client, |view| remote(view).devices[0].connected && remote(view).devices[0].path() == v1::DevicePath::Direct).await;
    let device = &remote(&view).devices[0];
    assert!(device.code_expires_ms == 0 && device.last_seen_ms > 0 && !device.last_address.is_empty(), "{device:?}");
    assert_eq!(device.platform, "ios", "its system, as its app said");
    // What the person sends from it says it came from it.
    apply_request(&mut client, &mut view, crate::common::create_agent("", "Ops", "")).await;
    let message =
        client_message::Body::SendMessage(v1::SendMessage { agent_id: "ops".into(), text: "From the train".into(), ..Default::default() });
    socket.send(Message::Binary(ClientMessage { id: 2, body: Some(message) }.encode_to_vec().into())).await.expect("send");
    let select = client_message::Body::SelectAgent(v1::SelectAgent { agent_id: "ops".into() });
    apply_request(&mut client, &mut view, select).await;
    let from = |view: &View| {
        view.entries.iter().find_map(|entry| match &entry.kind {
            Some(v1::entry::Kind::User(user)) if user.text == "From the train" => user.sent_from.clone(),
            _ => None,
        })
    };
    view.follow(&mut client, |view| from(view).is_some()).await;
    assert_eq!(from(&view), Some(v1::SentFrom { device_name: "Phone".into(), kind: v1::DeviceKind::Phone.into() }));
    // Its zoom is its own, and goes when it's unpaired.
    let zoom = client_message::Body::SetZoom(v1::SetZoom { zoom: 0.8 });
    socket.send(Message::Binary(ClientMessage { id: 3, body: Some(zoom) }.encode_to_vec().into())).await.expect("send");
    let zooms = |view: &View| view.workspace.settings.clone().expect("settings").zoom;
    view.follow(&mut client, |view| !zooms(view).is_empty()).await;
    assert_eq!(zooms(&view), [("phone".to_owned(), 0.8)].into());
    apply_request(&mut client, &mut view, client_message::Body::SetZoom(v1::SetZoom { zoom: 1.5 })).await;
    assert_eq!(zooms(&view), [("local".to_owned(), 1.5), ("phone".to_owned(), 0.8)].into());
    // So is how much it keeps of the files (#446).
    let cache = client_message::Body::SetFileCache(v1::SetFileCache { bytes: 5_000_000_000 });
    socket.send(Message::Binary(ClientMessage { id: 4, body: Some(cache) }.encode_to_vec().into())).await.expect("send");
    let caches = |view: &View| view.workspace.settings.clone().expect("settings").file_cache;
    view.follow(&mut client, |view| !caches(view).is_empty()).await;
    assert_eq!(caches(&view), [("phone".to_owned(), 5_000_000_000)].into());

    // Another key is turned away right after the handshake, before any of
    // Factory's protocol: the code is used, and nothing else is waiting.
    let stranger = new_device().await;
    let refused = dial(&stranger, &code, &code.addresses).await.expect_err("turned away");
    assert!(refused.contains("isn't paired"), "{refused}");
    // Nor does a code that expired let one in, and it's forgotten.
    factoryd::remote::pair(&home(&dir), "Old").expect("paired");
    let mut devices = factoryd::remote::devices(&home(&dir)).expect("devices");
    devices.iter_mut().find(|device| device.id == "old").expect("old").code_expires = Some(1);
    std::fs::write(dir.join("home/remote/devices.json"), serde_json::to_string(&devices).expect("json")).expect("write");
    let refused = dial(&stranger, &code, &code.addresses).await.expect_err("turned away");
    assert!(refused.contains("isn't paired"), "{refused}");
    view.follow(&mut client, |view| remote(view).devices.iter().all(|device| device.id != "old")).await;

    // Unpaired from the app: its session and its connection close at once.
    for id in ["phone"] {
        let unpair = client_message::Body::UnpairDevice(v1::UnpairDevice { device_id: id.into() });
        assert!(matches!(apply_request(&mut client, &mut view, unpair).await, Body::Ack(_)));
    }
    loop {
        if next(&mut socket).await.is_none() {
            break;
        }
    }
    match tokio::time::timeout(WAIT, connection.closed()).await.expect("closed in time") {
        ConnectionError::ApplicationClosed(close) => assert_eq!(String::from_utf8_lossy(&close.reason), "this device was unpaired"),
        other => panic!("expected an unpaired close, got {other:?}"),
    }
    // The last device gone, remote access is off again.
    view.follow(&mut client, |view| !remote(view).on && remote(view).devices.is_empty()).await;
    view.follow(&mut client, |view| !zooms(view).contains_key("phone")).await;
    assert_eq!(zooms(&view), [("local".to_owned(), 1.5)].into());
    assert!(caches(&view).is_empty(), "its file cache went with it");
    let unknown = client_message::Body::UnpairDevice(v1::UnpairDevice { device_id: "phone".into() });
    assert!(matches!(apply_request(&mut client, &mut view, unknown).await, Body::Error(error) if error.code() == ErrorCode::NotFound));
}

/// A device that checks none of its paths itself, so that every ping it
/// hears is Claktory's own keep-alive.
async fn device_that_never_checks() -> Endpoint {
    let transport = iroh::endpoint::QuicTransportConfig::builder().keep_alive_interval(Duration::from_secs(600)).build();
    Endpoint::builder(presets::Minimal).relay_mode(RelayMode::Disabled).transport_config(transport).bind().await.expect("device endpoint")
}

/// A device still waiting to be allowed keeps iroh's keep-alives, so Claktory pings it every 5 s as
/// before; let in, it's checked only every 25 s, which leaves an idle phone's
/// radio alone; and unpaired then, its session and connection still close at
/// once, without waiting for any timeout.
#[cfg_attr(windows, ignore = "its device reaches this machine directly, and on Windows remote access goes through the relays only")]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn claktory_checks_a_device_rarely_only_once_its_let_in_and_still_closes_it_at_once_when_unpaired() {
    let dir = temp_dir();
    let (addr, factory) = start(&dir).await;
    factory.set_tunnel_online_wait(Duration::from_millis(100));
    let (mut client, workspace, _) = Client::connect(addr).await;
    let mut view = View { workspace, ..View::default() };
    let relays = v1::SetRemoteAccess { relays: vec!["http://127.0.0.1:9/".into()], set_relays: true, ..Default::default() };
    assert!(matches!(apply_request(&mut client, &mut view, client_message::Body::SetRemoteAccess(relays)).await, Body::Ack(_)));
    let pairing = client_message::Body::CreatePairing(v1::CreatePairing { name: "Phone".into(), host: String::new() });
    let Body::PairingCode(made) = apply_request(&mut client, &mut view, pairing).await else { panic!("a pairing code") };
    let code = parse(&made.code);

    let phone = device_that_never_checks().await;
    let connection = dial(&phone, &code, &code.addresses).await.expect("a code is waiting: let in");
    for path in &connection.paths() {
        assert!(path.set_keep_alive_interval(None), "the path is open");
    }
    let pings = || connection.stats().frame_rx.ping;
    let mut socket = open(&connection).await;
    assert!(matches!(hello(&mut socket, &code.token).await, Body::PairingPending(_)));
    let before = pings();
    let checked = tokio::time::timeout(Duration::from_secs(30), async {
        while pings() == before {
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    });
    checked.await.expect("waiting to be allowed: Claktory checks it every 5 s, as before");

    let remote = |view: &View| view.workspace.remote.clone().expect("remote access");
    view.follow(&mut client, |view| !remote(view).pending_devices.is_empty()).await;
    let number = number(&code, &phone.id().to_string());
    let allow = client_message::Body::ApproveDevice(v1::ApproveDevice { device_id: "phone".into(), number, ..Default::default() });
    assert!(matches!(apply_request(&mut client, &mut view, allow).await, Body::Ack(_)));
    assert!(matches!(next(&mut socket).await, Some(Body::Welcome(_))));
    // The check already due may go once more; past it, the line stays quiet
    // for far longer than 5 s, which checks every 5 s would never leave.
    let quiet = Duration::from_secs(10);
    let mut last = (pings(), std::time::Instant::now());
    let stays_quiet = tokio::time::timeout(Duration::from_secs(40), async {
        while last.1.elapsed() < quiet {
            tokio::time::sleep(Duration::from_millis(100)).await;
            if pings() != last.0 {
                last = (pings(), std::time::Instant::now());
            }
        }
    });
    stays_quiet.await.expect("let in: Claktory leaves the checking to the device");

    let unpair = client_message::Body::UnpairDevice(v1::UnpairDevice { device_id: "phone".into() });
    let asked = std::time::Instant::now();
    assert!(matches!(apply_request(&mut client, &mut view, unpair).await, Body::Ack(_)));
    match tokio::time::timeout(WAIT, connection.closed()).await.expect("closed in time") {
        ConnectionError::ApplicationClosed(close) => assert_eq!(String::from_utf8_lossy(&close.reason), "this device was unpaired"),
        other => panic!("expected an unpaired close, got {other:?}"),
    }
    assert!(asked.elapsed() < Duration::from_secs(2), "at once: {:?}", asked.elapsed());
}

/// Friend bug #38: a disk that turns read-only or fails unpairs nobody. A
/// paired device still comes in (only when it came isn't noted), one that
/// would be new is told it's the disk, a devices file that can't be read
/// keeps the devices connected, and none of it says "not paired".
// The disk turns read-only by modes; on Windows that would take DACLs.
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_disk_that_fails_unpairs_nobody() {
    use std::os::unix::fs::PermissionsExt;
    let dir = temp_dir();
    let (addr, factory) = start(&dir).await;
    factory.set_tunnel_online_wait(Duration::from_millis(100));
    let (mut client, workspace, _) = Client::connect(addr).await;
    let mut view = View { workspace, ..View::default() };
    let remote = |view: &View| view.workspace.remote.clone().expect("remote access");
    let relays = v1::SetRemoteAccess { relays: vec!["http://127.0.0.1:9/".into()], set_relays: true, ..Default::default() };
    apply_request(&mut client, &mut view, client_message::Body::SetRemoteAccess(relays)).await;
    let make = |name: &str| client_message::Body::CreatePairing(v1::CreatePairing { name: name.into(), host: String::new() });
    let Body::PairingCode(made) = apply_request(&mut client, &mut view, make("Phone")).await else { panic!("a code") };
    let code = parse(&made.code);
    let phone = new_device().await;
    let connection = dial(&phone, &code, &code.addresses).await.expect("let in");
    let mut socket = open(&connection).await;
    assert!(matches!(hello(&mut socket, &code.token).await, Body::PairingPending(_)));
    factoryd::remote::approve(&home(&dir), "phone").expect("allowed");
    assert!(matches!(next(&mut socket).await, Some(Body::Welcome(_))));
    // A second code, waiting for its device.
    let Body::PairingCode(second) = apply_request(&mut client, &mut view, make("Tablet")).await else { panic!("a code") };
    let second = parse(&second.code);

    // The disk turns read-only (as far as Claktory can tell).
    let folder = dir.join("home/remote");
    std::fs::set_permissions(&folder, std::fs::Permissions::from_mode(0o500)).expect("read-only");
    if std::fs::write(folder.join("probe"), b"").is_ok() {
        eprintln!("skipped: this user writes to read-only folders (root)");
        std::fs::set_permissions(&folder, std::fs::Permissions::from_mode(0o700)).expect("writable");
        return;
    }
    let last_seen = |dir: &Path| factoryd::remote::devices(&home(dir)).expect("devices")[1].last_seen;
    let before = last_seen(&dir);
    let mut again = open(&connection).await;
    match hello(&mut again, &code.token).await {
        Body::Welcome(welcome) => assert_eq!(welcome.device_id, "phone"),
        other => panic!("a paired device still comes in, got {other:?}"),
    }
    assert_eq!(last_seen(&dir), before, "only when it came isn't noted");
    let tablet = new_device().await;
    let to_tablet = dial(&tablet, &second, &second.addresses).await.expect("its code is waiting");
    let mut waiting = open(&to_tablet).await;
    match hello(&mut waiting, &second.token).await {
        Body::Error(error) => {
            assert_eq!(error.code(), ErrorCode::Internal);
            assert!(
                error.message.contains("can't use its disk") && error.message.contains("You don't need to add this device again"),
                "{}",
                error.message
            );
        }
        other => panic!("a new device is told it's the disk, got {other:?}"),
    }

    // The devices file can't even be read: the devices stay connected and
    // shown, and one connecting again hears why it can't, not that it isn't
    // paired.
    let file = folder.join("devices.json");
    std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o000)).expect("unreadable");
    tokio::time::sleep(Duration::from_secs(3)).await;
    assert!(remote(&view).devices.iter().any(|device| device.id == "phone"), "still shown");
    assert!(connection.close_reason().is_none(), "its connection stays");
    let ping = client_message::Body::SelectAgent(v1::SelectAgent { agent_id: String::new() });
    again.send(Message::Binary(ClientMessage { id: 2, body: Some(ping) }.encode_to_vec().into())).await.expect("send");
    assert!(next(&mut again).await.is_some(), "its session still answers");
    let phone_again = dial(&phone, &code, &code.addresses).await.expect("the gate lets the Hello say why");
    let mut third = open(&phone_again).await;
    match hello(&mut third, &code.token).await {
        Body::Error(error) => {
            assert!(error.code() == ErrorCode::Internal && error.message.contains("can't use its disk"), "{}", error.message)
        }
        other => panic!("expected the disk's reason, got {other:?}"),
    }

    // Once the disk works again, nothing needs pairing again.
    std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o600)).expect("readable");
    std::fs::set_permissions(&folder, std::fs::Permissions::from_mode(0o700)).expect("writable");
    let mut fourth = open(&phone_again).await;
    assert!(matches!(hello(&mut fourth, &code.token).await, Body::Welcome(_)));
    assert!(last_seen(&dir) > before, "noted again");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn factoryd_pair_makes_a_code_for_the_running_factory() {
    let dir = temp_dir();
    let (_addr, _factory) = start(&dir).await;
    let local = parse(&local_pairing(&dir).await);
    let (code, device) = factoryd::remote::pair_offline(&home(&dir), "Laptop").expect("paired");
    let code = parse(&code);
    assert_eq!((device.id.as_str(), code.endpoint), ("laptop", local.endpoint), "the same key as the running Factory");
    assert!(code.relays.is_empty() && code.addresses.is_empty(), "found by its key");
    assert!(device.code_expires.is_some_and(|expires| expires > now_ms()));
    factoryd::remote::unpair(&home(&dir), "Laptop").expect("unpaired");
    assert!(factoryd::remote::unpair(&home(&dir), "local").is_err(), "this machine's app isn't a device to unpair");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_app_browses_this_machines_folders() {
    let dir = temp_dir();
    let (addr, _factory) = start(&dir).await;
    let (mut client, _, _) = Client::connect(addr).await;
    let browse = dir.join("browse");
    for folder in ["beta", "Alpha", ".hidden", "gamma/inner"] {
        std::fs::create_dir_all(browse.join(folder)).expect("folder");
    }
    std::fs::write(browse.join("notes.txt"), "a file").expect("file");
    crate::common::symlink(&browse.join("beta"), &browse.join("linked")).expect("link");
    let list = |path: &str| client_message::Body::ListFolders(v1::ListFolders { path: path.into() });

    // Folders only, links to folders too, sorted, hidden ones flagged.
    let Body::FolderList(listed) = client.request(list(&browse.display().to_string())).await.body else { panic!("no list") };
    #[cfg(unix)]
    let browse = std::fs::canonicalize(&browse).expect("real path");
    #[cfg(windows)]
    let browse = factoryd::windows::fs::canonical(&browse).expect("real path");
    assert_eq!(
        (listed.path.as_str(), listed.parent.as_str()),
        (browse.to_str().expect("utf-8"), browse.parent().expect("parent").to_str().expect("utf-8"))
    );
    let names: Vec<(&str, bool)> = listed.folders.iter().map(|folder| (folder.name.as_str(), folder.hidden)).collect();
    assert_eq!(names, [(".hidden", true), ("Alpha", false), ("beta", false), ("gamma", false), ("linked", false)]);
    assert!(!listed.truncated);

    // "" is the home folder, and ".." is resolved.
    let Body::FolderList(home) = client.request(list("")).await.body else { panic!("no list") };
    assert_eq!(home.path, home.home);
    let up = format!("{}/gamma/inner/..", browse.display());
    let Body::FolderList(gamma) = client.request(list(&up)).await.body else { panic!("no list") };
    assert_eq!(gamma.path, browse.join("gamma").display().to_string());

    // What can't be listed is refused, saying why.
    for (path, why) in [
        ("relative/path", "isn't a full path"),
        (if cfg!(windows) { r"C:\no\such\folder" } else { "/no/such/folder" }, "doesn't exist"),
        (&*browse.join("notes.txt").display().to_string(), "isn't a folder"),
    ] {
        match client.request(list(path)).await.body {
            Body::Error(error) => assert!(error.message.contains(why), "{path}: {}", error.message),
            other => panic!("{path}: {other:?}"),
        }
    }
}

fn list_home() -> client_message::Body {
    client_message::Body::ListFolders(v1::ListFolders { path: String::new() })
}

/// Sends a request on a socket; the first answer that isn't a Workspace.
async fn ask<S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin>(
    socket: &mut WebSocketStream<S>,
    id: u64,
    body: client_message::Body,
) -> Body {
    let message = ClientMessage { id, body: Some(body) };
    socket.send(Message::Binary(message.encode_to_vec().into())).await.expect("send");
    loop {
        if let answer @ (Body::Ack(_) | Body::Error(_) | Body::PairingCode(_) | Body::FolderList(_)) =
            next(socket).await.expect("an answer, on a connection still open")
        {
            return answer;
        }
    }
}

#[cfg_attr(windows, ignore = "its device reaches this machine directly, and on Windows remote access goes through the relays only")]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn remote_access_turning_on_and_off_keeps_the_persons_connection_and_this_machines_end_of_the_tunnel() {
    let dir = temp_dir();
    let (addr, factory) = start(&dir).await;
    factory.set_tunnel_online_wait(Duration::from_millis(100));
    let local = parse(&local_pairing(&dir).await);
    // The person's connection (this machine's app's, as tests make it).
    let mut socket = loopback(addr).await;
    let answer = hello(&mut socket, &crate::common::local_token(addr)).await;
    assert!(matches!(answer, Body::Welcome(_)), "{answer:?}");
    let relays = v1::SetRemoteAccess { relays: vec!["http://127.0.0.1:9/".into()], set_relays: true, ..Default::default() };
    assert!(matches!(ask(&mut socket, 2, client_message::Body::SetRemoteAccess(relays)).await, Body::Ack(_)));

    // Adding the first device turns remote access on: the answer comes on
    // the same connection, which stays open.
    let pairing = client_message::Body::CreatePairing(v1::CreatePairing { name: "Phone".into(), host: String::new() });
    let code = match ask(&mut socket, 3, pairing).await {
        Body::PairingCode(made) => parse(&made.code),
        other => panic!("expected a pairing code, got {other:?}"),
    };
    let phone = new_device().await;
    let phone_link = dial(&phone, &code, &code.addresses).await.expect("the phone reaches the remote endpoint");
    let mut phone_socket = open(&phone_link).await;
    assert!(matches!(hello(&mut phone_socket, &code.token).await, Body::PairingPending(_)));
    let allow = client_message::Body::ApproveDevice(v1::ApproveDevice {
        device_id: "phone".into(),
        number: number(&code, &phone.id().to_string()),
        ..Default::default()
    });
    assert!(matches!(ask(&mut socket, 4, allow).await, Body::Ack(_)));
    assert!(matches!(next(&mut phone_socket).await, Some(Body::Welcome(_))));
    assert!(matches!(ask(&mut socket, 4, list_home()).await, Body::FolderList(_)), "still connected");

    // Unpairing it turns remote access off; this machine's app stays.
    let unpair = client_message::Body::UnpairDevice(v1::UnpairDevice { device_id: "phone".into() });
    let message = ClientMessage { id: 5, body: Some(unpair) };
    socket.send(Message::Binary(message.encode_to_vec().into())).await.expect("send");
    let (mut acked, mut off) = (false, false);
    while !(acked && off) {
        match next(&mut socket).await.expect("still connected") {
            Body::Ack(_) => acked = true,
            Body::Workspace(workspace) => off = workspace.remote.as_ref().is_some_and(|remote| !remote.on),
            _ => {}
        }
    }
    // A moment for the remote endpoint to close.
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert!(matches!(ask(&mut socket, 6, list_home()).await, Body::FolderList(_)), "still connected");
    assert_eq!(parse(&local_pairing(&dir).await).addresses, local.addresses, "this machine's end of the tunnel stayed up throughout");

    // Turned on again, the remote endpoint takes the port it had.
    let again = client_message::Body::CreatePairing(v1::CreatePairing { name: "Tablet".into(), host: String::new() });
    let second = match ask(&mut socket, 7, again).await {
        Body::PairingCode(made) => parse(&made.code),
        other => panic!("expected a pairing code, got {other:?}"),
    };
    assert_eq!(second.addresses.first().map(SocketAddr::port), code.addresses.first().map(SocketAddr::port));
}

/// Makes a code from the app and has a new device use it: the code, the
/// device's id, its endpoint (kept, or its connection closes), its
/// connection and the socket it waits on.
pub(crate) async fn waiting_device(client: &mut Client, view: &mut View, name: &str) -> (Code, String, Endpoint, Connection, Socket) {
    let pairing = client_message::Body::CreatePairing(v1::CreatePairing { name: name.into(), host: String::new() });
    let made = match apply_request(client, view, pairing).await {
        Body::PairingCode(code) => code,
        other => panic!("expected a pairing code, got {other:?}"),
    };
    let code = parse(&made.code);
    let device = new_device().await;
    let connection = dial(&device, &code, &code.addresses).await.expect("a code is waiting: let in");
    let mut socket = open(&connection).await;
    assert!(matches!(hello(&mut socket, &code.token).await, Body::PairingPending(_)));
    (code, made.device_id, device, connection, socket)
}

/// The waiting device is turned away: why.
pub(crate) async fn turned_away(socket: &mut Socket, case: &str) -> String {
    match next(socket).await {
        Some(Body::Error(error)) if error.code() == ErrorCode::Unauthorized => error.message,
        other => panic!("{case}: expected to be turned away, got {other:?}"),
    }
}

#[cfg_attr(windows, ignore = "its device reaches this machine directly, and on Windows remote access goes through the relays only")]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_code_lets_a_device_in_only_once_the_person_allows_it_and_ends_when_they_stop_pairing() {
    let dir = temp_dir();
    let (addr, factory) = start(&dir).await;
    factory.set_tunnel_online_wait(Duration::from_millis(100));
    let (mut client, workspace, _) = Client::connect(addr).await;
    let mut view = View { workspace, ..View::default() };
    let remote = |view: &View| view.workspace.remote.clone().expect("remote access");
    let relays = v1::SetRemoteAccess { relays: vec!["http://127.0.0.1:9/".into()], set_relays: true, ..Default::default() };
    apply_request(&mut client, &mut view, client_message::Body::SetRemoteAccess(relays)).await;

    // Turned away: it hears so, and its device and code are forgotten.
    let (code, id, stranger, _link, mut socket) = waiting_device(&mut client, &mut view, "Tablet").await;
    let deny = client_message::Body::DenyDevice(v1::DenyDevice { device_id: id.clone() });
    assert!(matches!(apply_request(&mut client, &mut view, deny.clone()).await, Body::Ack(_)));
    let why = turned_away(&mut socket, "denied").await;
    assert!(why.contains("didn't allow this device"), "{why}");
    view.follow(&mut client, |view| remote(view).devices.is_empty() && remote(view).pending_devices.is_empty()).await;
    assert!(dial(&stranger, &code, &code.addresses).await.is_err(), "the code is gone");
    assert!(matches!(apply_request(&mut client, &mut view, deny).await, Body::Error(error) if error.code() == ErrorCode::NotFound));

    // Add a device closed while a device waits: the code ends.
    let (_, id, _device, _link, mut socket) = waiting_device(&mut client, &mut view, "Laptop").await;
    let cancel = client_message::Body::CancelPairing(v1::CancelPairing { device_id: id.clone() });
    assert!(matches!(apply_request(&mut client, &mut view, cancel.clone()).await, Body::Ack(_)));
    let why = turned_away(&mut socket, "cancelled").await;
    assert!(why.contains("cancelled"), "{why}");
    assert!(matches!(apply_request(&mut client, &mut view, cancel).await, Body::Ack(_)), "already gone: nothing to do");
    // And one nobody used yet.
    let pairing = client_message::Body::CreatePairing(v1::CreatePairing { name: "Spare".into(), host: String::new() });
    let Body::PairingCode(spare) = apply_request(&mut client, &mut view, pairing).await else { panic!("a code") };
    let cancel = client_message::Body::CancelPairing(v1::CancelPairing { device_id: spare.device_id.clone() });
    assert!(matches!(apply_request(&mut client, &mut view, cancel).await, Body::Ack(_)));
    view.follow(&mut client, |view| remote(view).devices.is_empty() && !remote(view).on).await;

    // Its 10 minutes run out while it waits.
    let (_, id, _device, _link, mut socket) = waiting_device(&mut client, &mut view, "Watch").await;
    let mut devices = factoryd::remote::devices(&home(&dir)).expect("devices");
    devices.iter_mut().find(|device| device.id == id).expect("waiting").code_expires = Some(now_ms() - 1);
    std::fs::write(dir.join("home/remote/devices.json"), serde_json::to_string(&devices).expect("json")).expect("write");
    // Allow goes by the file at once, not by what the Workspace showed a
    // moment ago.
    let allow = client_message::Body::ApproveDevice(v1::ApproveDevice { device_id: id.clone(), ..Default::default() });
    assert!(
        matches!(apply_request(&mut client, &mut view, allow.clone()).await, Body::Error(error) if error.code() == ErrorCode::NotFound)
    );
    let why = turned_away(&mut socket, "expired").await;
    assert!(why.contains("expired"), "{why}");
    assert!(matches!(apply_request(&mut client, &mut view, allow).await, Body::Error(error) if error.code() == ErrorCode::NotFound));

    // Allowed in a terminal (factoryd allow), it's let in all the same;
    // cancelling it afterwards leaves it paired.
    let (_, id, phone, _link, mut socket) = waiting_device(&mut client, &mut view, "Phone").await;
    factoryd::remote::approve(&home(&dir), "Phone").expect("allowed");
    assert!(matches!(next(&mut socket).await, Some(Body::Welcome(_))));
    let cancel = client_message::Body::CancelPairing(v1::CancelPairing { device_id: id.clone() });
    assert!(matches!(apply_request(&mut client, &mut view, cancel).await, Body::Ack(_)));
    let saved = factoryd::remote::devices(&home(&dir)).expect("devices");
    let paired = saved.iter().find(|device| device.id == id).expect("still paired");
    assert_eq!((paired.endpoint_id.clone(), paired.pending_endpoint_id.as_str()), (phone.id().to_string(), ""));
    view.follow(&mut client, |view| remote(view).devices.iter().any(|device| device.id == id && device.connected)).await;
}

/// An app that offers `claktory/2` gets it: the same protocol, compressed
/// both ways, each message arriving as soon as it's sent. One that offers
/// only `claktory/1` (every app before it) gets that, as before.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_app_that_offers_compression_gets_it_and_one_that_doesnt_still_connects() {
    let dir = temp_dir();
    let (_addr, _factory) = start(&dir).await;
    let local = parse(&local_pairing(&dir).await);
    // A device paired with a code and allowed (this machine's app connects
    // over its socket), dialling this machine's end.
    let (token, _) = factoryd::remote::pair(&home(&dir), "Laptop").expect("paired");
    let app = new_device().await;
    let both: &[&[u8]] = &[factoryd::remote::ALPN_COMPRESSED, factoryd::remote::ALPN];
    let connection = dial_offering(&app, &local, &local.addresses, both).await.expect("let in");
    assert_eq!(connection.alpn(), factoryd::remote::ALPN_COMPRESSED);
    let (send, receive) = connection.open_bi().await.expect("stream");
    let (mut socket, _) =
        tokio_tungstenite::client_async("ws://factory/", factoryd::remote::compressed(receive, send)).await.expect("websocket");
    assert!(matches!(hello(&mut socket, &token).await, Body::PairingPending(_)));
    factoryd::remote::approve(&home(&dir), "laptop").expect("allowed");
    assert!(matches!(next(&mut socket).await, Some(Body::Welcome(_))));
    let workspace = match next(&mut socket).await {
        Some(Body::Workspace(workspace)) => workspace,
        other => panic!("expected the workspace, got {other:?}"),
    };
    assert!(workspace.agents.iter().any(|agent| agent.id == "rivet"));
    // Requests and their answers go both ways, one at a time.
    for id in 2..5 {
        assert!(matches!(ask(&mut socket, id, list_home()).await, Body::FolderList(_)));
    }
    socket.close(None).await.expect("close");

    // An app from before compression.
    let (token, _) = factoryd::remote::pair(&home(&dir), "Old laptop").expect("paired");
    let old = new_device().await;
    let connection = dial(&old, &local, &local.addresses).await.expect("let in");
    assert_eq!(connection.alpn(), factoryd::remote::ALPN);
    let mut socket = open(&connection).await;
    assert!(matches!(hello(&mut socket, &token).await, Body::PairingPending(_)));
    factoryd::remote::approve(&home(&dir), "old-laptop").expect("allowed");
    assert!(matches!(next(&mut socket).await, Some(Body::Welcome(_))));
    assert!(matches!(ask(&mut socket, 2, list_home()).await, Body::FolderList(_)));
}

/// Opens a forward to `port` on `connection` (claktory-forward/1): the
/// answer line and the stream.
async fn forward_to(connection: &Connection, port: u16) -> (String, SendStream, RecvStream) {
    use tokio::io::AsyncReadExt;
    let (mut send, mut receive) = connection.open_bi().await.expect("stream");
    send.write_all(format!("port {port}\n").as_bytes()).await.expect("header");
    let mut line = Vec::new();
    loop {
        match tokio::time::timeout(WAIT, receive.read_u8()).await.expect("an answer in time") {
            Ok(b'\n') | Err(_) => break,
            Ok(byte) => line.push(byte),
        }
    }
    (String::from_utf8(line).expect("utf-8"), send, receive)
}

/// A paired device reaches what listens on this machine's localhost, as a
/// browser on the device coming back from a sign-in does; nobody else does,
/// nor Claktory's own port, and unpairing ends it.
#[cfg_attr(windows, ignore = "its device reaches this machine directly, and on Windows remote access goes through the relays only")]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_paired_device_forwards_to_this_machines_localhost() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let dir = temp_dir();
    let (addr, factory) = start(&dir).await;
    factory.set_tunnel_online_wait(Duration::from_millis(100));
    let (mut client, workspace, _) = Client::connect(addr).await;
    let mut view = View { workspace, ..View::default() };
    let relays = v1::SetRemoteAccess { relays: vec!["http://127.0.0.1:9/".into()], set_relays: true, ..Default::default() };
    apply_request(&mut client, &mut view, client_message::Body::SetRemoteAccess(relays)).await;

    // Something waiting for a sign-in's callback: it answers what it got.
    let waiting = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let port = waiting.local_addr().expect("addr").port();
    tokio::spawn(async move {
        while let Ok((mut socket, _)) = waiting.accept().await {
            tokio::spawn(async move {
                let mut asked = Vec::new();
                socket.read_to_end(&mut asked).await.expect("read");
                socket.write_all(b"got: ").await.expect("write");
                socket.write_all(&asked).await.expect("write");
            });
        }
    });

    // A device waiting to be allowed can't yet.
    let (code, id, device, _link, mut socket) = waiting_device(&mut client, &mut view, "Laptop").await;
    let early = dial_offering(&device, &code, &code.addresses, &[factoryd::remote::ALPN_FORWARD]).await.expect("the gate lets it in");
    let (answer, ..) = forward_to(&early, port).await;
    assert!(answer.starts_with("no ") && answer.contains("isn't paired"), "{answer}");

    let allow = client_message::Body::ApproveDevice(v1::ApproveDevice {
        device_id: id.clone(),
        number: number(&code, &device.id().to_string()),
        ..Default::default()
    });
    assert!(matches!(apply_request(&mut client, &mut view, allow).await, Body::Ack(_)));
    assert!(matches!(next(&mut socket).await, Some(Body::Welcome(_))));

    // Allowed, it forwards: the bytes both ways, its half-close passed on.
    let forwards = dial_offering(&device, &code, &code.addresses, &[factoryd::remote::ALPN_FORWARD]).await.expect("let in");
    assert_eq!(forwards.alpn(), factoryd::remote::ALPN_FORWARD);
    for _ in 0..2 {
        let (answer, mut send, mut receive) = forward_to(&forwards, port).await;
        assert_eq!(answer, "ok");
        send.write_all(b"GET /callback?code=abc HTTP/1.1\r\n\r\n").await.expect("write");
        send.finish().expect("finish");
        let mut got = Vec::new();
        tokio::time::timeout(WAIT, AsyncReadExt::read_to_end(&mut receive, &mut got)).await.expect("in time").expect("read");
        assert_eq!(got, b"got: GET /callback?code=abc HTTP/1.1\r\n\r\n");
    }
    // Nothing listening, Claktory's own port, or no port at all.
    let free = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("bind").local_addr().expect("addr").port();
    let (answer, ..) = forward_to(&forwards, free).await;
    assert!(answer.starts_with("no ") && answer.contains(&format!("port {free}")), "{answer}");
    let (answer, ..) = forward_to(&forwards, addr.port()).await;
    assert!(answer.starts_with("no ") && answer.contains("Claktory's own"), "{answer}");
    let (answer, ..) = forward_to(&forwards, 0).await;
    assert!(answer.starts_with("no "), "{answer}");

    // A key that isn't a paired device's, even from this machine.
    let stranger = new_device().await;
    let local = parse(&local_pairing(&dir).await);
    let theirs = dial_offering(&stranger, &local, &local.addresses, &[factoryd::remote::ALPN_FORWARD]).await.expect("this machine");
    let (answer, ..) = forward_to(&theirs, port).await;
    assert!(answer.contains("isn't paired"), "{answer}");

    // Unpaired, its forwarding connection closes.
    let unpair = client_message::Body::UnpairDevice(v1::UnpairDevice { device_id: id });
    assert!(matches!(apply_request(&mut client, &mut view, unpair).await, Body::Ack(_)));
    // Closed by Claktory; under load the device may hear only a reset, when
    // Claktory's side has dropped the connection before the close reached
    // it. Either is the end, and nothing gets through after it.
    let closed = tokio::time::timeout(WAIT, forwards.closed()).await.expect("closed in time");
    assert!(matches!(closed, ConnectionError::ApplicationClosed(_) | ConnectionError::Reset), "{closed:?}");
    assert!(forwards.open_bi().await.is_err(), "nothing opens on it");
}

/// Dials `addr` (a relay and no address, as from away), offering the
/// tunnel's ALPN; Err with why within `wait`.
async fn dial_at(device: &Endpoint, addr: EndpointAddr, wait: Duration) -> Result<Connection, String> {
    let dial = device.connect(addr, factoryd::remote::ALPN);
    tokio::time::timeout(wait, dial).await.map_err(|_| "timed out".to_owned())?.map_err(|error| format!("{error:#}"))
}

/// An address lookup on this machine, in place of n0's.
type Lookup = std::sync::Arc<iroh::test_utils::DnsPkarrServer>;

/// Claktory's tunnel on `relays` (the default ones) and `lookup`, as on the
/// internet with Claktory's relays and n0's lookup.
fn on_local_network(factory: &factoryd::Factory, relays: &[&iroh::RelayUrl], lookup: &Lookup) {
    let lookup = lookup.clone();
    factory.set_tunnel_network(relays.iter().map(ToString::to_string).collect(), move |builder| {
        builder.preset(lookup.preset()).ca_tls_config(iroh::tls::CaTlsConfig::insecure_skip_verify())
    });
}

/// A device homed on `relay`, with `lookup` (`None`: none at all) and `key`.
async fn device_on(relay: &iroh::RelayUrl, lookup: Option<&Lookup>, key: iroh::SecretKey) -> Endpoint {
    let mut builder = Endpoint::builder(presets::Minimal)
        .secret_key(key)
        .ca_tls_config(iroh::tls::CaTlsConfig::insecure_skip_verify())
        .relay_mode(RelayMode::Custom(relay.clone().into()));
    if let Some(lookup) = lookup {
        builder = builder.preset(lookup.preset());
    }
    builder.bind().await.expect("device endpoint")
}

/// Its code's relays, and no address (it's away from home).
fn from_away(code: &Code) -> EndpointAddr {
    code.relays.iter().fold(EndpointAddr::new(code.endpoint), |addr, relay| addr.with_relay_url(relay.parse().expect("a relay URL")))
}

/// Pairs "Phone" from the app, and gives its code once it names a relay the
/// tunnel reached, which is published.
async fn pair_phone(client: &mut Client, view: &mut View, lookup: &Lookup) -> Code {
    let pairing = client_message::Body::CreatePairing(v1::CreatePairing { name: "Phone".into(), host: String::new() });
    let made = match apply_request(client, view, pairing).await {
        Body::PairingCode(code) => code,
        other => panic!("expected a pairing code, got {other:?}"),
    };
    let code = parse(&made.code);
    lookup.on_endpoint(&code.endpoint, WAIT).await.expect("published");
    code
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_default_relays_are_claktorys_and_n0s_us_east_and_a_device_whose_code_names_n0s_still_connects() {
    use iroh::test_utils::{DnsPkarrServer, run_relay_server};

    let defaults = factoryd::remote::default_relays();
    let us_east = iroh::defaults::prod::default_na_east_relay().url.to_string();
    assert_eq!(defaults, ["https://relay.claktory.com/", us_east.as_str()]);
    assert_eq!(factoryd::remote::DEFAULT_RELAY, "https://relay.claktory.com/", "as the apps write it");
    assert_eq!(us_east, "https://use1-1.relay.n0.iroh.link./", "as iroh writes it, for devices homed there");
    // Relays on this machine: Claktory's, the fallback, and one of n0's
    // others, which were the default before; and an address lookup.
    let (_, ours, _ours) = run_relay_server().await.expect("a relay");
    let (_, fallback, _fallback) = run_relay_server().await.expect("a relay");
    let (_, n0s, _n0s) = run_relay_server().await.expect("a relay");
    let lookup: Lookup = std::sync::Arc::new(DnsPkarrServer::run().await.expect("an address lookup"));
    let dir = temp_dir();
    let (addr, factory) = start(&dir).await;
    factory.set_tunnel_online_wait(WAIT);
    on_local_network(&factory, &[&ours, &fallback], &lookup);
    let (mut client, workspace, _) = Client::connect(addr).await;
    let mut view = View { workspace, ..View::default() };
    let remote = |view: &View| view.workspace.remote.clone().expect("remote access");

    // config.json names no relays: the tunnel uses the default ones, homes
    // on one, and a new code names that one.
    let code = pair_phone(&mut client, &mut view, &lookup).await;
    view.follow(&mut client, |view| remote(view).on && remote(view).relay_connected).await;
    let shown = remote(&view);
    assert_eq!((shown.relays.as_slice(), shown.custom_relays), ([ours.to_string(), fallback.to_string()].as_slice(), false));
    assert_eq!(code.relays, std::slice::from_ref(&shown.home_relay));
    // A new phone, homed on Claktory's relay as the apps are, uses it from
    // away; the person allows it.
    let key = iroh::SecretKey::generate();
    let phone = device_on(&ours, Some(&lookup), key.clone()).await;
    let connection = dial_at(&phone, from_away(&code), WAIT).await.expect("reached through the code's relay");
    let mut socket = open(&connection).await;
    assert!(matches!(hello(&mut socket, &code.token).await, Body::PairingPending(_)));
    factoryd::remote::approve(&home(&dir), "phone").expect("allowed");
    assert!(matches!(next(&mut socket).await, Some(Body::Welcome(welcome)) if welcome.device_id == "phone"));
    drop(socket);
    connection.close(0u32.into(), b"bye");
    drop(connection);
    phone.close().await;

    // A device paired while n0's relays were the default: homed on one of
    // them, and its code names it and no address. It finds this Claktory
    // through the address lookup.
    let old = EndpointAddr::new(code.endpoint).with_relay_url(n0s.clone());
    let phone = device_on(&n0s, Some(&lookup), key).await;
    let connection = dial_at(&phone, old.clone(), WAIT).await.expect("found through the address lookup");
    let mut socket = open(&connection).await;
    assert!(matches!(hello(&mut socket, &code.token).await, Body::Welcome(welcome) if welcome.device_id == "phone"));
    view.follow(&mut client, |view| remote(view).devices.iter().any(|device| device.id == "phone" && device.connected)).await;
    // Without the lookup the old code alone leads nowhere: that relay
    // doesn't have this Claktory any more.
    let blind = device_on(&n0s, None, iroh::SecretKey::generate()).await;
    assert!(dial_at(&blind, old, Duration::from_secs(3)).await.is_err(), "only the lookup finds it");

    // config.json's relays still override the default ones.
    let relay = "http://127.0.0.1:9/";
    let relays = v1::SetRemoteAccess { relays: vec![relay.into()], set_relays: true, ..Default::default() };
    assert!(matches!(apply_request(&mut client, &mut view, client_message::Body::SetRemoteAccess(relays)).await, Body::Ack(_)));
    view.follow(&mut client, |view| remote(view).custom_relays).await;
    assert_eq!(remote(&view).relays, [relay]);
    factory.set_tunnel_online_wait(Duration::from_millis(100));
    let pairing = client_message::Body::CreatePairing(v1::CreatePairing { name: "Laptop".into(), host: String::new() });
    let made = match apply_request(&mut client, &mut view, pairing).await {
        Body::PairingCode(code) => code,
        other => panic!("expected a pairing code, got {other:?}"),
    };
    assert_eq!(parse(&made.code).relays, [relay], "{}", made.code);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn with_claktorys_relay_down_the_tunnel_homes_on_the_fallback_and_devices_still_connect() {
    use iroh::test_utils::{DnsPkarrServer, run_relay_server};

    // Claktory's relay is down (nothing answers at its address); the
    // fallback is up.
    let (_, ours, server) = run_relay_server().await.expect("a relay");
    drop(server);
    let (_, fallback, _fallback) = run_relay_server().await.expect("a relay");
    let lookup: Lookup = std::sync::Arc::new(DnsPkarrServer::run().await.expect("an address lookup"));
    let dir = temp_dir();
    let (addr, factory) = start(&dir).await;
    factory.set_tunnel_online_wait(WAIT);
    on_local_network(&factory, &[&ours, &fallback], &lookup);
    let (mut client, workspace, _) = Client::connect(addr).await;
    let mut view = View { workspace, ..View::default() };
    let remote = |view: &View| view.workspace.remote.clone().expect("remote access");

    let code = pair_phone(&mut client, &mut view, &lookup).await;
    assert_eq!(code.relays, [fallback.to_string()], "homed on the fallback");
    view.follow(&mut client, |view| remote(view).relay_connected).await;
    assert_eq!(remote(&view).home_relay, fallback.to_string());
    // A phone homed on Claktory's relay alone, as the apps are, still
    // reaches it through the fallback the code names.
    let phone = device_on(&ours, Some(&lookup), iroh::SecretKey::generate()).await;
    let connection = dial_at(&phone, from_away(&code), WAIT).await.expect("reached through the fallback");
    let mut socket = open(&connection).await;
    assert!(matches!(hello(&mut socket, &code.token).await, Body::PairingPending(_)));
    factoryd::remote::approve(&home(&dir), "phone").expect("allowed");
    assert!(matches!(next(&mut socket).await, Some(Body::Welcome(welcome)) if welcome.device_id == "phone"));
    // And one whose code names no relay at all finds it through the lookup.
    let bare = Code { token: code.token.clone(), endpoint: code.endpoint, relays: Vec::new(), addresses: Vec::new() };
    let again = device_on(&ours, Some(&lookup), phone.secret_key().clone()).await;
    let connection = dial_at(&again, from_away(&bare), WAIT).await.expect("found through the address lookup");
    let mut socket = open(&connection).await;
    assert!(matches!(hello(&mut socket, &code.token).await, Body::Welcome(welcome) if welcome.device_id == "phone"));
}

/// One of this machine's private network (LAN) IPv4 addresses in `code`, if
/// it has one: reached from 127.0.0.1 over loopback, unlike a VPN's.
fn lan_ipv4(code: &Code) -> Option<SocketAddr> {
    code.addresses.iter().find(|address| matches!(address.ip(), std::net::IpAddr::V4(ip) if ip.is_private())).copied()
}

/// #392: a device that reaches Claktory at an address that isn't this
/// machine's route back to the device (its second network, a port
/// forward) gets its answers from that same address. macOS sent them from
/// its route's address instead (noq-udp's IPv4 source went in a control
/// message macOS ignores on send, vendor/README.md), so the device's
/// handshake followed the answers there and Claktory's, pinned to the
/// address dialled, never completed: a direct path that stalled for good.
/// Here the device sends from 127.0.0.1 to this machine's network address:
/// the route back to 127.0.0.1 is 127.0.0.1's.
/// Not on Windows: a socket bound to 127.0.0.1 can't send to the machine's
/// own network address there (WSAENETUNREACH), so the device can't dial.
#[cfg_attr(windows, ignore = "a socket on 127.0.0.1 can't reach this machine's network address on Windows")]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_device_reaching_claktory_at_an_address_other_than_its_route_back_gets_answers_from_it() {
    let dir = temp_dir();
    let (addr, factory) = start(&dir).await;
    factory.set_tunnel_online_wait(Duration::from_millis(100));
    let (mut setup, _, _) = Client::connect(addr).await;
    let relays = v1::SetRemoteAccess { relays: vec!["http://127.0.0.1:9/".into()], set_relays: true, ..Default::default() };
    setup.ok(client_message::Body::SetRemoteAccess(relays)).await;
    let made = factory.create_pairing("Phone").await.expect("a code");
    let code = parse(&made.code);
    let Some(lan) = lan_ipv4(&code) else {
        eprintln!("skipped: this machine has no private network IPv4 address");
        return;
    };
    let device = Endpoint::builder(presets::Minimal)
        .relay_mode(RelayMode::Disabled)
        .clear_ip_transports()
        .bind_addr(SocketAddr::from(([127, 0, 0, 1], 0)))
        .expect("loopback")
        .bind()
        .await
        .expect("device endpoint");
    let connection = dial(&device, &code, &[lan]).await.expect("answered from the address it dialled");
    let mut socket = open(&connection).await;
    assert!(matches!(hello(&mut socket, &code.token).await, Body::PairingPending(_)));
}

/// Who reached a relay, and the token each showed (#450).
#[derive(Debug, Default)]
struct Heard(std::sync::Mutex<Vec<(EndpointId, Option<String>)>>);

impl iroh_relay::server::AccessControl for Heard {
    async fn on_connect(&self, request: &iroh_relay::server::ClientRequest) -> iroh_relay::server::Access {
        self.0.lock().unwrap().push((request.endpoint_id(), request.auth_token()));
        iroh_relay::server::Access::Allow
    }
}

/// #450: Claktory's relay forwards only to or from a signed-in Claktory,
/// which shows it a token from the account service. Signed out, the tunnel
/// is on n0's relays; signed in, it gets a token at its first check-in and
/// moves to Claktory's with it, giving it to Claktory's alone; signed out
/// again, back to n0's, the token forgotten. The Workspace names the relays
/// each time, for devices to follow, and a device's session stays up
/// throughout.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_signed_in_claktory_takes_its_relay_token_to_claktorys_relay_alone_and_signed_out_uses_n0s() {
    use iroh::test_utils::{DnsPkarrServer, run_relay_server_with_access};

    let heard = std::sync::Arc::new(Heard::default());
    let (_, ours, _ours) = run_relay_server_with_access(false, heard.clone()).await.expect("Claktory's relay");
    let n0_heard = std::sync::Arc::new(Heard::default());
    let (_, n0s, _n0s) = run_relay_server_with_access(false, n0_heard.clone()).await.expect("n0's");
    let lookup: Lookup = std::sync::Arc::new(DnsPkarrServer::run().await.expect("an address lookup"));
    let dir = temp_dir();
    let service = crate::claktory_account::Instance::start(&dir).await;
    let (addr, factory) = crate::claktory_account::open(&dir, &service.url).await;
    factory.set_tunnel_online_wait(WAIT);
    factory.set_tunnel_relay_grace(Duration::from_millis(500));
    on_local_network(&factory, &[&ours], &lookup);
    factory.set_tunnel_public_relays(vec![n0s.to_string()]);
    let (mut client, workspace, _) = Client::connect(addr).await;
    let mut view = View { workspace, ..View::default() };
    let remote = |view: &View| view.workspace.remote.clone().expect("remote access");

    // Not signed in: n0's relays, and a phone paired through them.
    let code = pair_phone(&mut client, &mut view, &lookup).await;
    assert_eq!(remote(&view).relays, [n0s.to_string()]);
    assert_eq!(code.relays, [n0s.to_string()]);
    let phone = device_on(&n0s, Some(&lookup), iroh::SecretKey::generate()).await;
    let connection = dial_at(&phone, from_away(&code), WAIT).await.expect("reached through n0's relay");
    let mut socket = open(&connection).await;
    assert!(matches!(hello(&mut socket, &code.token).await, Body::PairingPending(_)));
    factoryd::remote::approve(&home(&dir), "phone").expect("allowed");
    assert!(matches!(next(&mut socket).await, Some(Body::Welcome(welcome)) if welcome.device_id == "phone"));
    let backend: EndpointId = code.endpoint;
    assert!(heard.0.lock().unwrap().iter().all(|(who, _)| *who != backend), "never on Claktory's relay unsigned");

    // Signed in: the check-in brings a token, and the tunnel moves.
    crate::claktory_account::sign_in(&mut client, &service, "ana@example.com").await;
    view.follow(&mut client, |view| remote(view).relays == [ours.to_string()]).await;
    view.follow(&mut client, |view| remote(view).home_relay == ours.to_string() && remote(view).relay_connected).await;
    let token = tokio::time::timeout(WAIT, async {
        loop {
            if let Some(token) = heard.0.lock().unwrap().iter().find(|(who, _)| *who == backend).map(|(_, token)| token.clone()) {
                return token;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("the tunnel reaches Claktory's relay")
    .expect("with its token");
    let keys = claktory_account::server::relay_token::Keys::derive(&service.secrets.0);
    let public: Vec<u8> = (0..64).step_by(2).map(|at| u8::from_str_radix(&keys.public_hex()[at..at + 2], 16).expect("hex")).collect();
    let key = ed25519_dalek::VerifyingKey::from_bytes(&public.try_into().expect("32 bytes")).expect("the relay-token key");
    let payload = claktory_account::protocol::open_relay_token(&key, &token).expect("signed by the account service");
    let payload: serde_json::Value = serde_json::from_slice(&payload).expect("JSON");
    assert_eq!(payload["e"], backend.to_string(), "bound to the tunnel key");
    // Never to another relay.
    assert!(n0_heard.0.lock().unwrap().iter().all(|(_, token)| token.is_none()));
    // The phone's session is still up: it hears the sign-in.
    assert!(next(&mut socket).await.is_some(), "still connected after the move");

    // Signed out: back to n0's, the token forgotten.
    client.ok(client_message::Body::AccountSignOut(v1::AccountSignOut {})).await;
    view.follow(&mut client, |view| remote(view).relays == [n0s.to_string()]).await;
    let kept: i64 = rusqlite::Connection::open(dir.join("home/factory.db"))
        .expect("factory.db")
        .query_row("SELECT count(*) FROM relay_token", [], |row| row.get(0))
        .expect("the table");
    assert_eq!(kept, 0);
    assert!(next(&mut socket).await.is_some(), "still connected after the move back");
}

/// #450: a relay token replaced at runtime (a renewal, `insert_relay`) goes
/// with the next connection to that relay, the home relay's too, whose
/// connection lasts as long as the endpoint (vendor/iroh reads the token at
/// every dial).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_renewed_relay_token_goes_with_the_next_connection_to_the_home_relay() {
    assert_eq!(redialled_with(Some("two")).await, [Some("one".to_owned()), Some("two".to_owned())]);
}

/// #450: a relay config put back without a token (signed out, with
/// Claktory's relay named in config.json) dials without one: the relay map is
/// the only place the token comes from, not the one the connection started
/// with.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_relay_config_without_a_token_dials_without_one() {
    assert_eq!(redialled_with(None).await, [Some("one".to_owned()), None]);
}

/// The tokens a relay heard from an endpoint homed on it with the token
/// "one": at its first connection, then at the next, after its config was
/// replaced with one holding `token` and the relay dropped it.
async fn redialled_with(token: Option<&str>) -> Vec<Option<String>> {
    let heard = std::sync::Arc::new(Heard::default());
    let (_, url, server) = iroh::test_utils::run_relay_server_with_access(false, heard.clone()).await.expect("a relay");
    let first = iroh::RelayConfig::from(url.clone()).with_auth_token("one");
    let endpoint = Endpoint::builder(presets::Minimal)
        .ca_tls_config(iroh::tls::CaTlsConfig::insecure_skip_verify())
        .relay_mode(RelayMode::Custom(iroh::RelayMap::from(first)))
        .bind()
        .await
        .expect("an endpoint");
    tokio::time::timeout(WAIT, endpoint.online()).await.expect("homed on the relay");
    let tokens =
        || heard.0.lock().unwrap().iter().filter(|(who, _)| *who == endpoint.id()).map(|(_, token)| token.clone()).collect::<Vec<_>>();
    let mut replaced = iroh::RelayConfig::from(url.clone());
    if let Some(token) = token {
        replaced = replaced.with_auth_token(token);
    }
    endpoint.insert_relay(url, std::sync::Arc::new(replaced)).await;
    // The relay drops it, once it holds the connection: the relay confirms
    // the handshake (and the client counts itself connected) a moment before
    // it registers the client, and dropping an endpoint it doesn't hold yet
    // does nothing, so a single try could miss it and nothing would redial.
    let clients = server.relay_service().expect("a relay").clients();
    tokio::time::timeout(WAIT, async {
        while !clients.disconnect(endpoint.id(), None) {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("the relay holds the connection");
    tokio::time::timeout(WAIT, async {
        while tokens().len() < 2 {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("connected again");
    tokens()[..2].to_vec()
}
