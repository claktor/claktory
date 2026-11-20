use crate::common::{Client, start, temp_dir};
use factory_protocol::PROTOCOL_VERSION;
use factory_protocol::v1::{ClientMessage, ErrorCode, Hello, ServerMessage, client_message, server_message::Body};
use prost::Message as _;

fn hello(id: u64, protocol_version: u32) -> ClientMessage {
    let hello = Hello { protocol_version, client_version: "test".into(), token: String::new(), device_kind: 0, workspace_patches: false };
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
    client.send(hello(7, PROTOCOL_VERSION)).await;
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
    client.send(hello(3, PROTOCOL_VERSION + 1)).await;
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
