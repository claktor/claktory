//! Devices reach Factory through its private tunnel (iroh): pairing, keys
//! and tokens, the gate after the handshake, Unpair; and this machine's app,
//! through its local pairing code.

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
struct Code {
    token: String,
    endpoint: EndpointId,
    relays: Vec<String>,
    addresses: Vec<SocketAddr>,
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

fn parse(code: &str) -> Code {
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

/// A device: a key of its own, no relays.
async fn new_device() -> Endpoint {
    Endpoint::builder(presets::Minimal).relay_mode(RelayMode::Disabled).bind().await.expect("device endpoint")
}

/// Connects to Factory at the code's addresses; Err with why when it's
/// turned away (or nothing answers).
async fn dial(device: &Endpoint, code: &Code, addresses: &[SocketAddr]) -> Result<Connection, String> {
    let mut addr = EndpointAddr::new(code.endpoint);
    for address in addresses {
        addr = addr.with_ip_addr(*address);
    }
    let connection = tokio::time::timeout(WAIT, device.connect(addr, factoryd::remote::ALPN))
        .await
        .map_err(|_| "timed out".to_owned())?
        .map_err(|error| format!("{error:#}"))?;
    // A connection the gate turns away is closed just after the handshake.
    match tokio::time::timeout(Duration::from_millis(300), connection.closed()).await {
        Ok(ConnectionError::ApplicationClosed(close)) => Err(String::from_utf8_lossy(&close.reason).into_owned()),
        Ok(other) => Err(other.to_string()),
        Err(_) => Ok(connection),
    }
}

type Socket = WebSocketStream<tokio::io::Join<RecvStream, SendStream>>;

/// A client on the loopback port, as scripts and the app from before the
/// tunnel connect.
async fn loopback(addr: SocketAddr) -> WebSocketStream<tokio::net::TcpStream> {
    let tcp = tokio::net::TcpStream::connect(addr).await.expect("connect");
    tokio_tungstenite::client_async("ws://factory/", tcp).await.expect("websocket").0
}

/// A client on a new stream of the connection, as the app's tunnel makes one.
async fn open(connection: &Connection) -> Socket {
    let (send, receive) = connection.open_bi().await.expect("stream");
    let (socket, _) = tokio_tungstenite::client_async("ws://factory/", tokio::io::join(receive, send)).await.expect("websocket");
    socket
}

/// Says hello with `token`; returns the answer.
async fn hello<S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin>(socket: &mut WebSocketStream<S>, token: &str) -> Body {
    let hello = v1::Hello {
        protocol_version: PROTOCOL_VERSION,
        client_version: "test".into(),
        token: token.into(),
        device_kind: v1::DeviceKind::Phone.into(),
    };
    let message = ClientMessage { id: 1, body: Some(client_message::Body::Hello(hello)) };
    socket.send(Message::Binary(message.encode_to_vec().into())).await.expect("send");
    next(socket).await.expect("an answer")
}

/// The next message; None once the connection closes.
async fn next<S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin>(socket: &mut WebSocketStream<S>) -> Option<Body> {
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

/// This machine's pairing code, once Factory has written it.
async fn local_pairing(dir: &Path) -> String {
    let path = dir.join("home/remote/local-pairing");
    for _ in 0..200 {
        if let Ok(code) = std::fs::read_to_string(&path) {
            return code;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("no local pairing code");
}

#[tokio::test]
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

    // Until this machine's app has used the tunnel, the loopback port lets in
    // a client without a token: the app from before the tunnel, during an
    // update.
    let mut old_app = loopback(addr).await;
    assert!(matches!(hello(&mut old_app, "").await, Body::Welcome(_)));

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
    // From now on the loopback port wants the token: an agent on this
    // machine can't drive Factory without it.
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
    drop((old_app, script));

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

#[tokio::test]
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

    // The phone binds its key with the code's token, over the network.
    let phone = new_device().await;
    let connection = dial(&phone, &code, &code.addresses).await.expect("a code is waiting: let in");
    let mut socket = open(&connection).await;
    match hello(&mut socket, &code.token).await {
        Body::Welcome(welcome) => assert_eq!(welcome.device_id, "phone"),
        other => panic!("expected welcome, got {other:?}"),
    }
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
    let message = client_message::Body::SendMessage(v1::SendMessage { agent_id: "ops".into(), text: "From the train".into() });
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

    // Another key is turned away right after the handshake, before any of
    // Factory's protocol: the code is used, and nothing else is waiting.
    let stranger = new_device().await;
    let refused = dial(&stranger, &code, &code.addresses).await.expect_err("turned away");
    assert!(refused.contains("isn't paired"), "{refused}");
    // Nor does a code that expired let one in.
    factoryd::remote::pair(&home(&dir), "Old").expect("paired");
    let mut devices = factoryd::remote::devices(&home(&dir)).expect("devices");
    devices.iter_mut().find(|device| device.id == "old").expect("old").code_expires = Some(1);
    std::fs::write(dir.join("home/remote/devices.json"), serde_json::to_string(&devices).expect("json")).expect("write");
    let refused = dial(&stranger, &code, &code.addresses).await.expect_err("turned away");
    assert!(refused.contains("isn't paired"), "{refused}");
    view.follow(&mut client, |view| remote(view).devices.iter().any(|device| device.id == "old" && device.code_expires_ms == 1)).await;

    // Unpaired from the app: its session and its connection close at once.
    for id in ["old", "phone"] {
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
    let unknown = client_message::Body::UnpairDevice(v1::UnpairDevice { device_id: "phone".into() });
    assert!(matches!(apply_request(&mut client, &mut view, unknown).await, Body::Error(error) if error.code() == ErrorCode::NotFound));
}

#[tokio::test]
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

#[tokio::test]
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

#[tokio::test]
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
    assert!(matches!(hello(&mut phone_socket, &code.token).await, Body::Welcome(_)));
    assert!(matches!(ask(&mut socket, 4, list_home()).await, Body::FolderList(_)), "still connected");

    // Unpairing it turns remote access off; this machine's app stays.
    let unpair = client_message::Body::UnpairDevice(v1::UnpairDevice { device_id: "phone".into() });
    assert!(matches!(ask(&mut socket, 5, unpair).await, Body::Ack(_)));
    loop {
        match next(&mut socket).await.expect("still connected") {
            Body::Workspace(workspace) if workspace.remote.as_ref().is_some_and(|remote| !remote.on) => break,
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
