//! Devices on other machines: the remote listener, pairing and tokens.

mod common;

use std::net::SocketAddr;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use common::{Client, View, apply_request, home, start, temp_dir};
use factory_protocol::PROTOCOL_VERSION;
use factory_protocol::v1::{self, ClientMessage, ErrorCode, ServerMessage, client_message, server_message::Body};
use futures_util::{SinkExt, StreamExt};
use prost::Message as _;
use sha2::{Digest, Sha256};
use tokio_rustls::rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use tokio_rustls::rustls::crypto::{CryptoProvider, aws_lc_rs, verify_tls12_signature, verify_tls13_signature};
use tokio_rustls::rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use tokio_rustls::rustls::{self, ClientConfig, DigitallySignedStruct, SignatureScheme};
use tokio_tungstenite::tungstenite::Message;

/// Trusts only the certificate with this SHA-256, as the app does.
#[derive(Debug)]
struct Pinned {
    fingerprint: String,
    provider: Arc<CryptoProvider>,
}

impl ServerCertVerifier for Pinned {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _name: &ServerName<'_>,
        _ocsp: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        let seen: String = Sha256::digest(end_entity.as_ref()).iter().map(|byte| format!("{byte:02x}")).collect();
        if seen == self.fingerprint {
            Ok(ServerCertVerified::assertion())
        } else {
            Err(rustls::Error::General("not the pinned certificate".into()))
        }
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        signed: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        verify_tls12_signature(message, cert, signed, &self.provider.signature_verification_algorithms)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        signed: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        verify_tls13_signature(message, cert, signed, &self.provider.signature_verification_algorithms)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.provider.signature_verification_algorithms.supported_schemes()
    }
}

/// What a pairing code holds.
struct Code {
    port: u16,
    token: String,
    fingerprint: String,
}

fn parse(code: &str) -> Code {
    let rest = code.strip_prefix("factory://").expect("factory://");
    let (address, query) = rest.split_once('?').expect("query");
    let port = address.rsplit_once(':').expect("port").1.parse().expect("number");
    let field = |name: &str| query.split('&').find_map(|pair| pair.strip_prefix(&format!("{name}="))).expect(name).to_owned();
    Code { port, token: field("t"), fingerprint: field("fp") }
}

type Socket = tokio_tungstenite::WebSocketStream<tokio_rustls::client::TlsStream<tokio::net::TcpStream>>;

/// Connects over TLS, trusting only `fingerprint`; None if the TLS handshake fails.
async fn connect(addr: SocketAddr, fingerprint: &str) -> Option<Socket> {
    let provider = Arc::new(aws_lc_rs::default_provider());
    let verifier = Arc::new(Pinned { fingerprint: fingerprint.to_owned(), provider: provider.clone() });
    let config = ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .expect("versions")
        .dangerous()
        .with_custom_certificate_verifier(verifier)
        .with_no_client_auth();
    let tcp = tokio::net::TcpStream::connect(addr).await.ok()?;
    let tls = tokio_rustls::TlsConnector::from(Arc::new(config)).connect(ServerName::try_from("factory").expect("name"), tcp).await.ok()?;
    let (socket, _) = tokio_tungstenite::client_async("ws://factory/", tls).await.expect("websocket");
    Some(socket)
}

/// Says hello with `token`; returns the answer.
async fn hello(socket: &mut Socket, token: &str) -> Body {
    let hello = v1::Hello { protocol_version: PROTOCOL_VERSION, client_version: "test".into(), token: token.into() };
    let message = ClientMessage { id: 1, body: Some(client_message::Body::Hello(hello)) };
    socket.send(Message::Binary(message.encode_to_vec().into())).await.expect("send");
    loop {
        let frame = tokio::time::timeout(Duration::from_secs(10), socket.next()).await.expect("answer").expect("frame").expect("ok");
        if let Message::Binary(bytes) = frame {
            return ServerMessage::decode(bytes).expect("decode").body.expect("body");
        }
    }
}

fn set_remote(dir: &Path, remote: serde_json::Value) {
    let path = dir.join("home/config.json");
    let mut config: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&path).expect("config")).expect("json");
    config["remote"] = remote;
    std::fs::write(&path, serde_json::to_string_pretty(&config).expect("json")).expect("write");
}

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0").expect("bind").local_addr().expect("addr").port()
}

#[tokio::test]
async fn paired_devices_connect_over_pinned_tls_and_others_are_refused() {
    let dir = temp_dir();
    let (_local, _factory) = start(&dir).await;
    // Off until config.json turns it on; with no config.json, there's nothing to turn on.
    assert!(factoryd::remote::set_remote(&home(&temp_dir()), true, None).is_err());
    assert!(factoryd::remote::pair(&home(&dir), "laptop", "").expect_err("off").to_string().contains("\"remote\""));
    let port = free_port();
    set_remote(&dir, serde_json::json!({"port": port, "listen": "127.0.0.1"}));
    let (code, _) = factoryd::remote::pair(&home(&dir), "Laptop", "127.0.0.1").expect("paired");
    let paired = parse(&code);
    assert!(code.starts_with(&format!("factory://127.0.0.1:{port}?t=")), "{code}");
    assert_eq!((paired.port, paired.token.len(), paired.fingerprint.len()), (port, 64, 64));
    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    let mut socket = None;
    for _ in 0..100 {
        socket = connect(addr, &paired.fingerprint).await;
        if socket.is_some() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let mut socket = socket.expect("the remote listener started");

    // The right token gets Factory, as the local app does.
    assert!(matches!(hello(&mut socket, &paired.token).await, Body::Welcome(_)));
    let workspace = loop {
        let frame = socket.next().await.expect("frame").expect("ok");
        if let Message::Binary(bytes) = frame
            && let Some(Body::Workspace(workspace)) = ServerMessage::decode(bytes).expect("decode").body
        {
            break workspace;
        }
    };
    assert!(workspace.agents.iter().any(|agent| agent.id == "rivet"));
    let devices = factoryd::remote::devices(&home(&dir)).expect("devices");
    assert_eq!((devices[0].id.as_str(), devices[0].name.as_str()), ("laptop", "Laptop"));
    assert!(devices[0].last_seen.is_some(), "it connected");
    let saved = std::fs::read_to_string(dir.join("home/remote/devices.json")).expect("devices.json");
    assert!(!saved.contains(&paired.token), "only the token's hash is kept");

    // No token, or a wrong one: refused.
    for token in ["", "0000"] {
        let mut socket = connect(addr, &paired.fingerprint).await.expect("tls");
        match hello(&mut socket, token).await {
            Body::Error(error) => assert_eq!(error.code(), ErrorCode::Unauthorized),
            other => panic!("expected unauthorized, got {other:?}"),
        }
    }
    // Another certificate isn't Factory's.
    assert!(connect(addr, &"0".repeat(64)).await.is_none());

    // The certificate stays, so codes made later pin the same one.
    let other = parse(&factoryd::remote::pair(&home(&dir), "Laptop", "").expect("paired").0);
    assert_eq!(other.fingerprint, paired.fingerprint);
    assert_eq!(factoryd::remote::devices(&home(&dir)).expect("devices")[1].id, "laptop-2");

    // Unpaired, its token is refused at once.
    factoryd::remote::unpair(&home(&dir), "laptop").expect("unpaired");
    let mut socket = connect(addr, &paired.fingerprint).await.expect("tls");
    assert!(matches!(hello(&mut socket, &paired.token).await, Body::Error(error) if error.code() == ErrorCode::Unauthorized));
    let mut socket = connect(addr, &other.fingerprint).await.expect("tls");
    assert!(matches!(hello(&mut socket, &other.token).await, Body::Welcome(_)));

    // `factoryd remote on` keeps the port and address set; `remote off`
    // takes it out of config.json, and nothing listens any more.
    let kept = factoryd::remote::set_remote(&home(&dir), true, None).expect("on").expect("remote");
    assert_eq!((kept.port, kept.listen.as_str()), (port, "127.0.0.1"));
    assert_eq!(factoryd::remote::set_remote(&home(&dir), false, None).expect("off"), None);
    for _ in 0..100 {
        if tokio::net::TcpStream::connect(addr).await.is_err() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("the remote listener kept listening");
}

#[tokio::test]
async fn the_app_turns_remote_access_on_pairs_devices_and_unpairs_them() {
    let dir = temp_dir();
    let (local, _factory) = start(&dir).await;
    let (mut client, workspace, _) = Client::connect(local).await;
    let mut view = View { workspace, ..View::default() };
    let remote = |view: &View| view.workspace.remote.clone().expect("remote access");
    assert!(!remote(&view).on && remote(&view).devices.is_empty());
    assert!(!remote(&view).machine.is_empty());
    let pairing = |name: &str| client_message::Body::CreatePairing(v1::CreatePairing { name: name.into(), host: String::new() });
    match apply_request(&mut client, &mut view, pairing("Phone")).await {
        Body::Error(error) => assert!(error.message.contains("Remote access is off"), "{}", error.message),
        other => panic!("expected an error, got {other:?}"),
    }

    // On, from the app: config.json gets `remote`, and the listener starts.
    let port = free_port();
    let on = v1::SetRemoteAccess { on: true, port: u32::from(port), listen: Some("127.0.0.1".into()) };
    assert!(matches!(apply_request(&mut client, &mut view, client_message::Body::SetRemoteAccess(on)).await, Body::Ack(_)));
    let config = std::fs::read_to_string(dir.join("home/config.json")).expect("config");
    assert!(config.contains(&format!("\"port\": {port}")), "{config}");
    view.follow(&mut client, |view| !remote(view).fingerprint.is_empty()).await;
    let shown = remote(&view);
    assert_eq!((shown.on, shown.port, shown.listen.as_str(), shown.error.as_str()), (true, u32::from(port), "127.0.0.1", ""));
    assert_eq!(shown.addresses, ["127.0.0.1"]);

    // Paired from the app: the code goes only to the asker, the device to everyone.
    let code = match apply_request(&mut client, &mut view, pairing("Phone")).await {
        Body::PairingCode(code) => code,
        other => panic!("expected a pairing code, got {other:?}"),
    };
    assert_eq!(code.device_id, "phone");
    let paired = parse(&code.code);
    assert_eq!(paired.fingerprint, shown.fingerprint);
    view.follow(&mut client, |view| remote(view).devices.len() == 1).await;
    assert_eq!((remote(&view).devices[0].last_seen_ms, remote(&view).devices[0].connected), (0, false));

    // Connected, it knows which device it is, and every client sees it connected.
    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    let mut phone = connect(addr, &paired.fingerprint).await.expect("tls");
    match hello(&mut phone, &paired.token).await {
        Body::Welcome(welcome) => assert_eq!(welcome.device_id, "phone"),
        other => panic!("expected welcome, got {other:?}"),
    }
    view.follow(&mut client, |view| remote(view).devices.first().is_some_and(|device| device.connected)).await;
    let device = &remote(&view).devices[0];
    assert!(device.last_seen_ms > 0 && device.last_address == "127.0.0.1", "{device:?}");

    // Unpaired from the app: its connection closes, and it's gone from the list.
    let unpair = client_message::Body::UnpairDevice(v1::UnpairDevice { device_id: "phone".into() });
    assert!(matches!(apply_request(&mut client, &mut view, unpair).await, Body::Ack(_)));
    let closed = tokio::time::timeout(Duration::from_secs(10), async {
        while let Some(Ok(frame)) = phone.next().await {
            if frame.is_close() {
                return;
            }
        }
    })
    .await;
    assert!(closed.is_ok(), "the unpaired device's connection closed");
    view.follow(&mut client, |view| remote(view).devices.is_empty()).await;
    let unknown = client_message::Body::UnpairDevice(v1::UnpairDevice { device_id: "phone".into() });
    assert!(matches!(apply_request(&mut client, &mut view, unknown).await, Body::Error(error) if error.code() == ErrorCode::NotFound));

    // Off again.
    let off = v1::SetRemoteAccess { on: false, port: 0, listen: None };
    apply_request(&mut client, &mut view, client_message::Body::SetRemoteAccess(off)).await;
    assert!(!remote(&view).on);
    assert!(!std::fs::read_to_string(dir.join("home/config.json")).expect("config").contains("\"remote\""));
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
