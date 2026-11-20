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
    endpoint: EndpointId,
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
    tokio_tungstenite::client_async("ws://factory/", tcp).await.expect("websocket").0
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

async fn workspace(socket: &mut Socket) -> v1::Workspace {
    loop {
        if let Some(Body::Workspace(workspace)) = next(socket).await {
            return workspace;
        }
    }
}

/// This machine's pairing code, once Factory has written it with the
/// tunnel's address (a new home's first one has none until it binds).
async fn local_pairing(dir: &Path) -> String {
    let path = dir.join("home/remote/local-pairing");
    for _ in 0..200 {
        if let Ok(code) = std::fs::read_to_string(&path)
            && code.contains("&a=")
        {
            return code;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("no local pairing code");
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

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn this_machines_app_connects_through_the_tunnel_with_remote_access_off() {
    let dir = temp_dir();
    let (addr, factory) = start(&dir).await;
    let written = local_pairing(&dir).await;
    let local = parse(&written);
    use std::os::unix::fs::PermissionsExt;
    let mode = std::fs::metadata(dir.join("home/remote/local-pairing")).expect("file").permissions().mode();
    assert_eq!(mode & 0o777, 0o600, "only this user reads it");
    assert!(local.relays.is_empty(), "no relays while remote access is off: {written}");
    assert!(matches!(local.addresses.as_slice(), [address] if address.ip().is_loopback()), "{written}");

    // The loopback port never lets in a client without the token: any user
    // of this machine can reach it.
    let mut stranger = loopback(addr).await;
    assert!(matches!(hello(&mut stranger, "").await, Body::Error(error) if error.code() == ErrorCode::Unauthorized));

    // The app, with a key of its own, gets Factory as on the loopback port.
    let app = new_device().await;
    let connection = dial(&app, &local, &local.addresses).await.expect("let in");
    let mut socket = open(&connection).await;
    match hello(&mut socket, &local.token).await {
        Body::Welcome(welcome) => assert_eq!(welcome.device_id, factoryd::remote::LOCAL_DEVICE),
        other => panic!("expected welcome, got {other:?}"),
    }
    let shown = workspace(&mut socket).await.remote.expect("remote access");
    assert!(!shown.on && shown.devices.is_empty() && shown.relays.is_empty(), "{shown:?}");
    assert_eq!(shown.endpoint_id, local.endpoint.to_string());
    // A second window is a second stream on the same connection.
    let mut second = open(&connection).await;
    assert!(matches!(hello(&mut second, &local.token).await, Body::Welcome(_)));
    // A wrong token from this machine is refused all the same.
    let mut wrong = open(&connection).await;
    assert!(matches!(hello(&mut wrong, "0000").await, Body::Error(error) if error.code() == ErrorCode::Unauthorized));
    // Nor after this machine's app has used the tunnel.
    let mut unknown = loopback(addr).await;
    match hello(&mut unknown, "").await {
        Body::Error(error) => {
            assert_eq!(error.code(), ErrorCode::Unauthorized);
            assert!(error.message.contains("remote/local-pairing"), "{}", error.message);
        }
        other => panic!("expected unauthorized, got {other:?}"),
    }
    let mut wrong_token = loopback(addr).await;
    assert!(matches!(hello(&mut wrong_token, &"0".repeat(64)).await, Body::Error(error) if error.code() == ErrorCode::Unauthorized));
    let mut script = loopback(addr).await;
    assert!(matches!(hello(&mut script, &local.token).await, Body::Welcome(welcome) if welcome.device_id == "local"));
    // Another device's token isn't this machine's.
    let (token, _) = factoryd::remote::pair(&home(&dir), "Laptop").expect("paired");
    let mut laptop = loopback(addr).await;
    assert!(matches!(hello(&mut laptop, &token).await, Body::Error(error) if error.code() == ErrorCode::Unauthorized));
    factoryd::remote::unpair(&home(&dir), "laptop").expect("unpaired");
    // Deleting the code's file makes a new token, and keeps the port closed.
    drop((stranger, script));

    // A new backend on the same data keeps the key and the token.
    drop((socket, second, wrong, unknown, laptop, connection));
    drop(factory);
    let (_addr, _factory) = start(&dir).await;
    let again = parse(&local_pairing(&dir).await);
    assert_eq!((again.endpoint, &again.token, &again.addresses), (local.endpoint, &local.token, &local.addresses));
    drop(_factory);
    std::fs::remove_file(dir.join("home/remote/local-pairing")).expect("deleted");
    let (addr, _factory) = start(&dir).await;
    assert_ne!(parse(&local_pairing(&dir).await).token, local.token);
    let mut unknown = loopback(addr).await;
    assert!(matches!(hello(&mut unknown, "").await, Body::Error(error) if error.code() == ErrorCode::Unauthorized));
}

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
    assert_eq!(asking.number, number(&code, &phone.id().to_string()), "worked out the same on both sides");
    assert!(asking.requested_ms > 0 && asking.expires_ms > now_ms());
    // While it waits, another key that copied the code is turned away.
    let copier = new_device().await;
    let refused = dial(&copier, &code, &code.addresses).await.expect_err("the code is taken");
    assert!(refused.contains("isn't paired"), "{refused}");
    let allow = client_message::Body::ApproveDevice(v1::ApproveDevice { device_id: "phone".into() });
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
    let unknown = client_message::Body::UnpairDevice(v1::UnpairDevice { device_id: "phone".into() });
    assert!(matches!(apply_request(&mut client, &mut view, unknown).await, Body::Error(error) if error.code() == ErrorCode::NotFound));
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
    std::os::unix::fs::symlink(browse.join("beta"), browse.join("linked")).expect("link");
    let list = |path: &str| client_message::Body::ListFolders(v1::ListFolders { path: path.into() });

    // Folders only, links to folders too, sorted, hidden ones flagged.
    let Body::FolderList(listed) = client.request(list(&browse.display().to_string())).await.body else { panic!("no list") };
    let browse = std::fs::canonicalize(&browse).expect("real path");
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
        ("/no/such/folder", "doesn't exist"),
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

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn remote_access_turning_on_and_off_keeps_this_machines_app_connected() {
    let dir = temp_dir();
    let (_addr, factory) = start(&dir).await;
    factory.set_tunnel_online_wait(Duration::from_millis(100));
    let local = parse(&local_pairing(&dir).await);
    let app = new_device().await;
    let connection = dial(&app, &local, &local.addresses).await.expect("let in");
    let mut socket = open(&connection).await;
    assert!(matches!(hello(&mut socket, &local.token).await, Body::Welcome(_)));
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
    let allow = client_message::Body::ApproveDevice(v1::ApproveDevice { device_id: "phone".into() });
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
    assert!(connection.close_reason().is_none(), "the tunnel connection never closed");
    assert_eq!(parse(&local_pairing(&dir).await).addresses, local.addresses, "the same port throughout");

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
async fn waiting_device(client: &mut Client, view: &mut View, name: &str) -> (Code, String, Endpoint, Connection, Socket) {
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
async fn turned_away(socket: &mut Socket, case: &str) -> String {
    match next(socket).await {
        Some(Body::Error(error)) if error.code() == ErrorCode::Unauthorized => error.message,
        other => panic!("{case}: expected to be turned away, got {other:?}"),
    }
}

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
    let why = turned_away(&mut socket, "expired").await;
    assert!(why.contains("expired"), "{why}");
    let allow = client_message::Body::ApproveDevice(v1::ApproveDevice { device_id: id.clone() });
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
    let app = new_device().await;
    let both: &[&[u8]] = &[factoryd::remote::ALPN_COMPRESSED, factoryd::remote::ALPN];
    let connection = dial_offering(&app, &local, &local.addresses, both).await.expect("let in");
    assert_eq!(connection.alpn(), factoryd::remote::ALPN_COMPRESSED);
    let (send, receive) = connection.open_bi().await.expect("stream");
    let (mut socket, _) =
        tokio_tungstenite::client_async("ws://factory/", factoryd::remote::compressed(receive, send)).await.expect("websocket");
    assert!(matches!(hello(&mut socket, &local.token).await, Body::Welcome(_)));
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
    let old = new_device().await;
    let connection = dial(&old, &local, &local.addresses).await.expect("let in");
    assert_eq!(connection.alpn(), factoryd::remote::ALPN);
    let mut socket = open(&connection).await;
    assert!(matches!(hello(&mut socket, &local.token).await, Body::Welcome(_)));
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

    let allow = client_message::Body::ApproveDevice(v1::ApproveDevice { device_id: id.clone() });
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
    let closed = tokio::time::timeout(WAIT, forwards.closed()).await.expect("closed in time");
    assert!(matches!(closed, ConnectionError::ApplicationClosed(_)), "{closed:?}");
}
