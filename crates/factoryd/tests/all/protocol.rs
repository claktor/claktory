use crate::common::{Client, home, local_token, start, temp_dir};
use factory_protocol::PROTOCOL_VERSION;
use factory_protocol::v1::{ClientMessage, ErrorCode, Hello, ServerMessage, client_message, server_message::Body};
use prost::Message as _;

fn hello(id: u64, protocol_version: u32, token: String) -> ClientMessage {
    let hello =
        Hello { protocol_version, client_version: "test".into(), token, device_kind: 0, workspace_patches: false, ..Default::default() };
    ClientMessage { id, body: Some(client_message::Body::Hello(hello)) }
}

fn error_code(message: &ServerMessage) -> ErrorCode {
    match &message.body {
        Some(Body::Error(error)) => error.code(),
        other => panic!("expected an error, got {other:?}"),
    }
}

#[tokio::test]
async fn welcome_echoes_the_request_id_and_is_followed_by_the_workspace() {
    let (addr, _factory) = start(&temp_dir()).await;
    let mut client = Client::open(addr).await;
    client.send(hello(7, PROTOCOL_VERSION, local_token(addr))).await;
    let welcome = client.receive().await;
    assert_eq!(welcome.reply_to, 7);
    match welcome.body {
        Some(Body::Welcome(welcome)) => {
            assert_eq!(welcome.protocol_version, PROTOCOL_VERSION);
            assert_eq!(welcome.server_version, env!("CARGO_PKG_VERSION"));
        }
        other => panic!("expected welcome, got {other:?}"),
    }
    let workspace = client.receive().await;
    assert_eq!(workspace.reply_to, 0);
    match workspace.body {
        Some(Body::Workspace(workspace)) => assert_eq!(workspace.models.len(), 2),
        other => panic!("expected workspace, got {other:?}"),
    }
}

#[tokio::test]
async fn unsupported_version_is_rejected_and_closed() {
    let (addr, _factory) = start(&temp_dir()).await;
    let mut client = Client::open(addr).await;
    client.send(hello(3, PROTOCOL_VERSION + 1, local_token(addr))).await;
    let reply = client.receive().await;
    assert_eq!(reply.reply_to, 3);
    assert_eq!(error_code(&reply), ErrorCode::UnsupportedProtocolVersion);
    assert!(client.closed().await);
}

#[tokio::test]
async fn first_message_must_be_hello() {
    let (addr, _factory) = start(&temp_dir()).await;
    let mut client = Client::open(addr).await;
    let select = client_message::Body::SelectAgent(Default::default());
    client.send(ClientMessage { id: 5, body: Some(select) }).await;
    let reply = client.receive().await;
    assert_eq!(reply.reply_to, 5);
    assert_eq!(error_code(&reply), ErrorCode::HandshakeRequired);
    assert!(client.closed().await);
}

#[tokio::test]
async fn malformed_frame_is_reported_and_the_connection_survives() {
    let (addr, _factory) = start(&temp_dir()).await;
    let (mut client, _, _) = Client::connect(addr).await;
    client.send_bytes(vec![0xff, 0xff, 0xff]).await;
    assert_eq!(error_code(&client.receive().await), ErrorCode::MalformedMessage);
    let reply = client.request(crate::common::create_project("After", &temp_dir())).await;
    assert!(matches!(reply.body, Body::Ack(_)), "got {:?}", reply.body);
}

#[tokio::test]
async fn message_from_a_newer_client_is_answered_not_fatal() {
    let (addr, _factory) = start(&temp_dir()).await;
    let (mut client, _, _) = Client::connect(addr).await;
    // A request in field 999, which this server doesn't know: it decodes to an empty body.
    let mut bytes = ClientMessage { id: 9, body: None }.encode_to_vec();
    prost::encoding::encode_key(999, prost::encoding::WireType::LengthDelimited, &mut bytes);
    prost::encoding::encode_varint(0, &mut bytes);
    client.send_bytes(bytes).await;
    let reply = client.receive().await;
    assert_eq!(reply.reply_to, 9);
    assert_eq!(error_code(&reply), ErrorCode::UnknownMessage);
    // And the connection keeps working.
    let reply = client.request(crate::common::create_project("After", &temp_dir())).await;
    assert!(matches!(reply.body, Body::Ack(_)), "got {:?}", reply.body);
}

/// The loopback port lets in only this machine's app, by the token in its
/// local pairing code, which only this user can read: any other user's
/// program, with no token or another, is turned away, and its Hello is
/// answered so a CLI can say why.
#[tokio::test]
async fn the_loopback_port_wants_this_users_token() {
    let dir = temp_dir();
    let (addr, factory) = start(&dir).await;
    for token in [String::new(), "not-the-token".to_owned()] {
        let mut client = Client::open(addr).await;
        client.send(hello(4, PROTOCOL_VERSION, token)).await;
        let reply = client.receive().await;
        assert_eq!(reply.reply_to, 4);
        assert_eq!(error_code(&reply), ErrorCode::Unauthorized);
        assert!(client.closed().await);
    }
    // Not even once this machine's app has used the tunnel and gone.
    Client::connect(addr).await;
    let mut client = Client::open(addr).await;
    client.send(hello(5, PROTOCOL_VERSION, String::new())).await;
    assert_eq!(error_code(&client.receive().await), ErrorCode::Unauthorized);
    drop(factory);

    // Only this user can open the home, its remote folder and the code.
    let root = home(&dir).root().to_owned();
    let mode = crate::common::mode;
    assert_eq!(mode(&root), 0o700);
    assert_eq!(mode(&root.join("remote")), 0o700);
    assert_eq!(mode(&root.join("remote/local-pairing")), 0o600);
}

/// A home made before, open to every user, is made private when Factory
/// opens, and its code still lets this machine's app in.
#[tokio::test]
async fn an_open_home_is_made_private() {
    let dir = temp_dir();
    let root = home(&dir).root().to_owned();
    std::fs::create_dir_all(root.join("remote")).expect("remote");
    // On Windows, folders made in the temporary folder take its DACL, open
    // to the administrators.
    #[cfg(unix)]
    for path in [&root, &root.join("remote")] {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).expect("permissions");
    }
    assert_eq!(crate::common::mode(&root), if cfg!(unix) { 0o755 } else { 0o777 }, "open to start with");
    let (addr, _factory) = start(&dir).await;
    let mode = crate::common::mode;
    assert_eq!(mode(&root), 0o700);
    assert_eq!(mode(&root.join("remote")), 0o700);
    Client::connect(addr).await;
}
