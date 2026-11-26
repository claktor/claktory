//! Localhost forwarding (docs: private-tunnel.md): when a sign-in opened in
//! this device's browser comes back to http://127.0.0.1:<port>, the program
//! waiting for it runs on the backend's machine. [forward] listens on this
//! device's 127.0.0.1:<port> and [::1]:<port> and carries each connection,
//! over the tunnel (ALPN `claktory-forward/1`), to the same port there.
//!
//! Each connection is one stream on its own iroh connection to the
//! backend: the device writes `port <N>\n`, the backend answers `ok\n` and
//! then both sides pass the bytes as they are, or `no <sentence>\n` and ends
//! the stream (nothing listens there, a port of Claktory's own, a device
//! that isn't paired).
//!
//! A website a pipe's peer shares, opened here (#503), is forwarded the same
//! way from a port of this device's own ([forward_to]): the share's port on
//! the backend's machine is often one this device uses itself (3000, say).

use std::{
    net::{Ipv4Addr, Ipv6Addr, SocketAddr},
    sync::{Arc, Mutex},
};

use iroh::endpoint::Connection;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt, copy},
    net::{TcpListener, TcpStream},
    task::JoinHandle,
};

use crate::tunnel::{Dialer, Tunnel};

/// The backend's localhost forwarding, both sides.
pub const ALPN_FORWARD: &[u8] = b"claktory-forward/1";

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ForwardingFailure {
    /// Something on this device already listens on the port (on 127.0.0.1
    /// or ::1): the browser would reach it instead.
    PortTaken,
    /// The backend is from before forwarding.
    Unsupported,
    /// Anything else, said as it is.
    Failed(String),
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ForwardingStatus {
    /// Connections carried so far: the browser came back once it's above 0.
    pub connections: u32,
    /// Why the last one couldn't be carried (the backend's sentence, or the
    /// tunnel's); empty while all's well.
    pub last_error: String,
}

/// A port forwarded until [Forward::stop] (or drop).
pub struct Forward {
    port: u16,
    status: Arc<Mutex<ForwardingStatus>>,
    listeners: Vec<JoinHandle<()>>,
}

impl Forward {
    pub fn port(&self) -> u16 {
        self.port
    }

    pub fn status(&self) -> ForwardingStatus {
        self.status.lock().unwrap().clone()
    }

    /// Stops listening; connections being carried finish on their own.
    pub fn stop(&self) {
        for listener in &self.listeners {
            listener.abort();
        }
    }
}

impl Drop for Forward {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Starts forwarding [port] to the same port on the backend's machine. The
/// forwarding connection is dialled first, so a backend from before it
/// fails here.
pub async fn forward(tunnel: &Tunnel, port: u16) -> Result<Forward, ForwardingFailure> {
    forward_to(tunnel, port, port).await
}

/// Starts forwarding this device's [port] to [target] on the backend's
/// machine.
pub async fn forward_to(tunnel: &Tunnel, port: u16, target: u16) -> Result<Forward, ForwardingFailure> {
    if port == 0 || target == 0 {
        return Err(ForwardingFailure::Failed("Port 0 can't be forwarded.".to_owned()));
    }
    let mut listeners = Vec::new();
    for ip in [SocketAddr::from((Ipv4Addr::LOCALHOST, port)), SocketAddr::from((Ipv6Addr::LOCALHOST, port))] {
        match TcpListener::bind(ip).await {
            Ok(listener) => listeners.push(listener),
            Err(error) if error.kind() == std::io::ErrorKind::AddrInUse => return Err(ForwardingFailure::PortTaken),
            // No IPv6 on this device: 127.0.0.1 alone, then.
            Err(_) if ip.is_ipv6() => {}
            Err(error) => return Err(ForwardingFailure::Failed(error.to_string())),
        }
    }
    let tunnel = tunnel.dialer();
    tunnel.forward_connection().await?;
    let status = Arc::new(Mutex::new(ForwardingStatus::default()));
    let listeners = listeners
        .into_iter()
        .map(|listener| {
            let status = status.clone();
            let tunnel = tunnel.clone();
            tokio::spawn(async move {
                while let Ok((socket, _)) = listener.accept().await {
                    let _ = socket.set_nodelay(true);
                    tokio::spawn(carry(socket, target, tunnel.clone(), status.clone()));
                }
            })
        })
        .collect();
    Ok(Forward { port, status, listeners })
}

/// One connection from the browser, to the backend's machine and back.
async fn carry(socket: TcpStream, port: u16, tunnel: Dialer, status: Arc<Mutex<ForwardingStatus>>) {
    let failed = |said: String| status.lock().unwrap().last_error = said;
    let connection = match tunnel.forward_connection().await {
        Ok(connection) => connection,
        Err(error) => return failed(said(&error)),
    };
    let (mut send, mut receive) = match open(&connection, port).await {
        Ok(streams) => streams,
        Err(said) => return failed(said),
    };
    {
        let mut status = status.lock().unwrap();
        status.connections += 1;
        status.last_error.clear();
    }
    let (mut read, mut write) = socket.into_split();
    let up = async {
        let _ = copy(&mut read, &mut send).await;
        let _ = send.finish();
    };
    let down = async {
        let _ = copy(&mut receive, &mut write).await;
        let _ = write.shutdown().await;
    };
    tokio::join!(up, down);
}

/// A stream to [port] on the backend's machine, once it says `ok`; else its
/// sentence.
async fn open(connection: &Connection, port: u16) -> Result<(iroh::endpoint::SendStream, iroh::endpoint::RecvStream), String> {
    let (mut send, mut receive) = connection.open_bi().await.map_err(|error| error.to_string())?;
    send.write_all(format!("port {port}\n").as_bytes()).await.map_err(|error| error.to_string())?;
    let mut line = Vec::new();
    loop {
        match receive.read_u8().await {
            Ok(b'\n') => break,
            Ok(byte) if line.len() < 1024 => line.push(byte),
            Ok(_) => return Err("The backend's answer was too long.".to_owned()),
            Err(_) => return Err("The backend closed the forward.".to_owned()),
        }
    }
    let line = String::from_utf8_lossy(&line).into_owned();
    match line.as_str() {
        "ok" => Ok((send, receive)),
        _ => Err(line.strip_prefix("no ").unwrap_or(&line).to_owned()),
    }
}

fn said(error: &ForwardingFailure) -> String {
    match error {
        ForwardingFailure::PortTaken => "The port is in use on this device.".to_owned(),
        ForwardingFailure::Unsupported => "This Claktory can't forward sign-ins yet: update it.".to_owned(),
        ForwardingFailure::Failed(said) => said.clone(),
    }
}

#[cfg(test)]
mod tests {
    use iroh::{Endpoint, RelayMode, endpoint::presets};

    use super::*;
    use crate::tunnel::{ALPN, generate_key, parse_key};

    async fn device() -> Endpoint {
        Endpoint::builder(presets::Minimal)
            .relay_mode(RelayMode::Disabled)
            .secret_key(parse_key(&generate_key()).unwrap())
            .bind()
            .await
            .unwrap()
    }

    /// A free port on this machine.
    async fn free_port() -> u16 {
        TcpListener::bind("127.0.0.1:0").await.unwrap().local_addr().unwrap().port()
    }

    /// A backend that forwards as factoryd does, except that on one machine
    /// it must reach another port than the device listens on: `target`. Port
    /// 1 it refuses.
    async fn forwarding_backend(target: u16) -> Endpoint {
        let endpoint = Endpoint::builder(presets::Minimal)
            .relay_mode(RelayMode::Disabled)
            .alpns(vec![ALPN.to_vec(), ALPN_FORWARD.to_vec()])
            .bind()
            .await
            .unwrap();
        let server = endpoint.clone();
        tokio::spawn(async move {
            while let Some(incoming) = server.accept().await {
                tokio::spawn(async move {
                    let connection = incoming.await.unwrap();
                    assert_eq!(connection.alpn(), ALPN_FORWARD);
                    while let Ok((mut send, mut receive)) = connection.accept_bi().await {
                        tokio::spawn(async move {
                            let mut line = Vec::new();
                            loop {
                                match receive.read_u8().await.unwrap() {
                                    b'\n' => break,
                                    byte => line.push(byte),
                                }
                            }
                            if line == b"port 1" {
                                send.write_all(b"no Port 1 is Claktory's own.\n").await.unwrap();
                                let _ = send.finish();
                                return;
                            }
                            let socket = TcpStream::connect(("127.0.0.1", target)).await.unwrap();
                            send.write_all(b"ok\n").await.unwrap();
                            let (mut read, mut write) = socket.into_split();
                            let up = async {
                                let _ = copy(&mut receive, &mut write).await;
                                let _ = write.shutdown().await;
                            };
                            let down = async {
                                let _ = copy(&mut read, &mut send).await;
                                let _ = send.finish();
                            };
                            tokio::join!(up, down);
                        });
                    }
                });
            }
        });
        endpoint
    }

    /// What a sign-in's callback server does: answers with what it got.
    async fn callback_server() -> u16 {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            while let Ok((mut socket, _)) = listener.accept().await {
                tokio::spawn(async move {
                    let mut asked = Vec::new();
                    socket.read_to_end(&mut asked).await.unwrap();
                    socket.write_all(b"got: ").await.unwrap();
                    socket.write_all(&asked).await.unwrap();
                });
            }
        });
        port
    }

    async fn ask(port: u16, what: &[u8]) -> Vec<u8> {
        let mut socket = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
        socket.write_all(what).await.unwrap();
        socket.shutdown().await.unwrap();
        let mut got = Vec::new();
        tokio::time::timeout(std::time::Duration::from_secs(10), socket.read_to_end(&mut got)).await.expect("in time").unwrap();
        got
    }

    #[tokio::test]
    async fn the_browser_reaches_the_backends_localhost() {
        let target = callback_server().await;
        let backend = forwarding_backend(target).await;
        let tunnel = Tunnel::open(device().await, backend.addr()).await.unwrap();
        let port = free_port().await;
        let forward = forward(&tunnel, port).await.expect("forwarding");
        assert_eq!(forward.port(), port);
        assert_eq!(forward.status(), ForwardingStatus::default());
        for _ in 0..2 {
            assert_eq!(ask(port, b"GET /callback?code=abc HTTP/1.1\r\n\r\n").await, b"got: GET /callback?code=abc HTTP/1.1\r\n\r\n");
        }
        assert_eq!(forward.status().connections, 2);

        // Stopped, the port is free again.
        forward.stop();
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        TcpListener::bind(("127.0.0.1", port)).await.expect("free after stop");
        tunnel.close().await;
    }

    #[tokio::test]
    async fn a_port_of_its_own_reaches_another_there() {
        // The backend's test double connects every stream to `target`; what
        // it was asked for is the device's word, checked by the backend's
        // own tests. Here: the device listens on a port other than the one
        // it names, and the line says the one it names.
        let target = callback_server().await;
        let backend = forwarding_backend(target).await;
        let tunnel = Tunnel::open(device().await, backend.addr()).await.unwrap();
        let port = free_port().await;
        let forward = forward_to(&tunnel, port, 1).await.expect("forwarding");
        assert_eq!(forward.port(), port);
        let _ = TcpStream::connect(("127.0.0.1", port)).await;
        for _ in 0..100 {
            if !forward.status().last_error.is_empty() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        assert_eq!(forward.status().last_error, "Port 1 is Claktory's own.", "it named port 1 there");
        let other = free_port().await;
        let forward = forward_to(&tunnel, other, target).await.expect("forwarding");
        assert_eq!(ask(other, b"GET / HTTP/1.1\r\n\r\n").await, b"got: GET / HTTP/1.1\r\n\r\n");
        assert_eq!(forward.status().connections, 1);
        tunnel.close().await;
    }

    #[tokio::test]
    async fn a_port_in_use_a_refusal_and_an_old_backend_say_so() {
        let target = callback_server().await;
        let backend = forwarding_backend(target).await;
        let tunnel = Tunnel::open(device().await, backend.addr()).await.unwrap();

        // In use on this device: the browser would reach that instead.
        let taken = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = taken.local_addr().unwrap().port();
        assert_eq!(forward(&tunnel, port).await.err(), Some(ForwardingFailure::PortTaken));

        // The backend turns one down: the connection closes, and the status
        // says why.
        if let Ok(forward) = forward(&tunnel, 1).await {
            let _ = TcpStream::connect(("127.0.0.1", 1)).await;
            for _ in 0..100 {
                if !forward.status().last_error.is_empty() {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            }
            assert_eq!(forward.status().last_error, "Port 1 is Claktory's own.");
        } // Port 1 needs privileges to listen on; nothing to check then.
        tunnel.close().await;

        // A backend from before forwarding.
        let old = Endpoint::builder(presets::Minimal).relay_mode(RelayMode::Disabled).alpns(vec![ALPN.to_vec()]).bind().await.unwrap();
        let server = old.clone();
        tokio::spawn(async move {
            while let Some(incoming) = server.accept().await {
                let _ = incoming.await;
            }
        });
        let tunnel = Tunnel::open(device().await, old.addr()).await.unwrap();
        assert_eq!(forward(&tunnel, free_port().await).await.err(), Some(ForwardingFailure::Unsupported));
        tunnel.close().await;
    }
}
