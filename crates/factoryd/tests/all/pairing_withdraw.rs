//! A device waiting to be allowed takes back its own request when the
//! person taps Cancel on it (#603): sent on its waiting connection, it
//! leaves the pending devices at once, and its code waits again; sent on
//! any other connection it's refused.

use std::time::Duration;

use crate::common::{Client, View, apply_request, start, temp_dir};
use crate::remote::{hello, new_device, dial, next, open, waiting_device};
use factory_protocol::v1::{self, ClientMessage, ErrorCode, client_message, server_message::Body};
use futures_util::SinkExt;
use prost::Message as _;
use tokio_tungstenite::tungstenite::Message;

async fn follow(client: &mut Client, view: &mut View, done: impl Fn(&View) -> bool) {
    let waited = tokio::time::timeout(Duration::from_secs(30), async {
        while !done(view) {
            let message = client.receive().await;
            view.apply(message.body.as_ref().expect("body"));
        }
    })
    .await;
    assert!(waited.is_ok(), "still waiting: {:?}", view.workspace.remote);
}

fn pending(view: &View) -> Vec<String> {
    view.workspace.remote.as_ref().map(|remote| remote.pending_devices.iter().map(|device| device.device_id.clone()).collect()).unwrap_or_default()
}

fn withdraw() -> client_message::Body {
    client_message::Body::WithdrawPairing(v1::WithdrawPairing {})
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_waiting_device_takes_back_its_request_on_its_own_connection_only() {
    let dir = temp_dir();
    let (addr, factory) = start(&dir).await;
    factory.set_tunnel_online_wait(Duration::from_millis(100));
    let (mut client, workspace, _) = Client::connect(addr).await;
    let mut view = View { workspace, ..View::default() };
    let relays = v1::SetRemoteAccess { relays: vec!["http://127.0.0.1:9/".into()], set_relays: true, ..Default::default() };
    apply_request(&mut client, &mut view, client_message::Body::SetRemoteAccess(relays)).await;

    let (code, id, _device, _connection, mut socket) = waiting_device(&mut client, &mut view, "iPad").await;
    follow(&mut client, &mut view, |view| pending(view) == [id.clone()]).await;

    // From the person's own connection, it's refused: only the waiting
    // device takes back its request.
    match apply_request(&mut client, &mut view, withdraw()).await {
        Body::Error(error) if error.code() == ErrorCode::InvalidRequest => {}
        other => panic!("expected a refusal, got {other:?}"),
    }
    assert_eq!(pending(&view), std::slice::from_ref(&id));

    // On its waiting connection: Ack, the connection closes, and the
    // request is gone from the Workspace at once.
    let message = ClientMessage { id: 9, body: Some(withdraw()) };
    socket.send(Message::Binary(message.encode_to_vec().into())).await.expect("send");
    assert!(matches!(next(&mut socket).await, Some(Body::Ack(_))));
    assert!(next(&mut socket).await.is_none(), "closed");
    follow(&mut client, &mut view, |view| pending(view).is_empty()).await;
    let remote = view.workspace.remote.clone().expect("remote");
    assert!(remote.devices.iter().any(|device| device.id == id), "its code is kept");

    // Its code waits again: the device can ask again with it.
    let again = new_device().await;
    let link = dial(&again, &code, &code.addresses).await.expect("the code still lets a device ask");
    let mut socket = open(&link).await;
    assert!(matches!(hello(&mut socket, &code.token).await, Body::PairingPending(_)));
    follow(&mut client, &mut view, |view| pending(view) == [id.clone()]).await;
}
