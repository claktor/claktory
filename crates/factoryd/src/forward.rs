//! Localhost forwarding through the private tunnel (`claktory-forward/1`):
//! a paired device on another machine reaches what listens on this
//! machine's localhost, so a sign-in whose browser runs on the device can
//! come back to the program waiting for it here (a model provider's sign-in
//! in factoryd, or a command line tool an agent runs, many of which have no
//! way to paste a code instead).
//!
//! Each bidirectional stream on such a connection is one TCP connection.
//! The device sends `port <N>\n`; Claktory answers `ok\n` and then passes
//! the bytes both ways untouched, half-closes included, or `no <sentence>\n`
//! and ends the stream. Only a paired device that was allowed may forward
//! (checked again for every stream, so unpairing stops it), never to
//! Claktory's own port, and only to 127.0.0.1 or [::1]. Each forward is
//! logged with its device, port and size, never what it carried.

use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::Weak;
use std::time::Duration;

use iroh::endpoint::{Connection, RecvStream, SendStream};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

use crate::factory::Factory;

/// How long connecting to the port may take.
const CONNECT: Duration = Duration::from_secs(3);
/// How long a device may take to say which port.
const HEADER: Duration = Duration::from_secs(10);
/// The longest header: "port 65535\n".
const HEADER_MAX: usize = 16;

/// Serves a device's forwarding connection until it ends.
pub(crate) async fn serve(connection: Connection, factory: Weak<Factory>, endpoint_id: String) {
    while let Ok((send, receive)) = connection.accept_bi().await {
        let factory = factory.clone();
        let endpoint_id = endpoint_id.clone();
        tokio::spawn(async move { forward(factory, &endpoint_id, send, receive).await });
    }
}

/// Why a forward was turned down, as the device is told.
fn refusal(factory: &Factory, endpoint_id: &str, port: Option<u16>) -> Result<(String, u16), String> {
    let Some(device) = crate::remote::paired_device(&factory.home, endpoint_id) else {
        return Err("This device isn't paired with this computer.".to_owned());
    };
    let Some(port) = port.filter(|port| *port != 0) else {
        return Err("That isn't a port.".to_owned());
    };
    if factory.remote.listener_port() == Some(port) {
        return Err(format!("Port {port} is Claktory's own."));
    }
    Ok((device, port))
}

async fn forward(factory: Weak<Factory>, endpoint_id: &str, mut send: SendStream, mut receive: RecvStream) {
    let port = match tokio::time::timeout(HEADER, header(&mut receive)).await {
        Ok(Some(port)) => Some(port),
        _ => None,
    };
    let Some(strong) = factory.upgrade() else { return };
    let checked = refusal(&strong, endpoint_id, port);
    let machine = crate::remote::machine_name();
    drop(strong);
    let (device, port) = match checked {
        Ok(found) => found,
        Err(why) => {
            tracing::info!(endpoint = %endpoint_id, ?port, %why, "refused to forward localhost");
            return no(send, &why).await;
        }
    };
    let Some(socket) = connect(port).await else {
        tracing::info!(%device, port, "forwarding localhost: nothing listens there");
        return no(send, &format!("Nothing is listening on port {port} on {machine}.")).await;
    };
    if send.write_all(b"ok\n").await.is_err() {
        return;
    }
    tracing::info!(%device, port, "forwarding localhost");
    let (mut read, mut write) = socket.into_split();
    let up = async {
        let copied = tokio::io::copy(&mut receive, &mut write).await.unwrap_or(0);
        let _ = write.shutdown().await;
        copied
    };
    let down = async {
        let copied = tokio::io::copy(&mut read, &mut send).await.unwrap_or(0);
        let _ = send.finish();
        copied
    };
    let (up, down) = tokio::join!(up, down);
    tracing::info!(%device, port, bytes_in = up, bytes_out = down, "forward ended");
}

/// The port in `port <N>\n`, read a byte at a time so nothing after it is
/// taken.
async fn header(receive: &mut RecvStream) -> Option<u16> {
    let mut line = Vec::new();
    loop {
        let byte = receive.read_u8().await.ok()?;
        if byte == b'\n' {
            break;
        }
        line.push(byte);
        if line.len() > HEADER_MAX {
            return None;
        }
    }
    std::str::from_utf8(&line).ok()?.strip_prefix("port ")?.trim().parse().ok()
}

async fn no(mut send: SendStream, why: &str) {
    let line = format!("no {}\n", why.replace('\n', " "));
    let _ = send.write_all(line.as_bytes()).await;
    let _ = send.finish();
}

/// What listens on `port`, on 127.0.0.1 or else [::1].
async fn connect(port: u16) -> Option<TcpStream> {
    for ip in [Ipv4Addr::LOCALHOST.into(), Ipv6Addr::LOCALHOST.into()] {
        if let Ok(Ok(socket)) = tokio::time::timeout(CONNECT, TcpStream::connect(SocketAddr::new(ip, port))).await {
            let _ = socket.set_nodelay(true);
            return Some(socket);
        }
    }
    None
}
