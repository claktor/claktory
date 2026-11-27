//! Tests of iroh's UDP socket, Claktory's copy of netwatch
//! (vendor/netwatch): after a network change iroh rebinds its sockets,
//! and a rebind whose bind fails (the port held for a moment by something
//! else) must leave the socket to try again at its next send, not closed for
//! good. Upstream, every send then failed with "socket closed" until the
//! process restarted: the backend and the app went through the relay only,
//! logging a warning per packet (n0-computer/net-tools#200).

use std::net::{Ipv4Addr, SocketAddr};
use std::time::Duration;

use netwatch::UdpSocket;

/// What a send or a receive may take once the port is free again.
const HEALED_WITHIN: Duration = Duration::from_secs(1);

// Holding the port: another socket on one of this machine's addresses, at
// the port the socket listens on everywhere (0.0.0.0), with SO_REUSEADDR,
// which macOS allows beside it. The socket's own bind (without it) then fails
// with AddrInUse until the other one goes. Linux refuses the holder itself.
#[cfg(target_vendor = "apple")]
#[tokio::test]
async fn a_rebind_that_fails_while_the_port_is_held_heals_at_the_next_send() {
    let socket = UdpSocket::bind_full((Ipv4Addr::UNSPECIFIED, 0)).unwrap();
    let port = socket.local_addr().unwrap().port();
    let peer = std::net::UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    peer.set_read_timeout(Some(HEALED_WITHIN)).unwrap();
    let peer_addr = peer.local_addr().unwrap();

    let holder = hold(port);
    let failed = socket.rebind().expect_err("the port is held");
    assert_eq!(failed.kind(), std::io::ErrorKind::AddrInUse);
    assert!(socket.is_broken(), "a failed rebind leaves the socket to try again");
    // Still held: the send tries the bind again and fails, without the
    // socket giving up.
    assert!(socket.send_to(b"held", peer_addr).await.is_err());
    assert!(socket.is_broken());

    drop(holder);
    let sent = tokio::time::timeout(HEALED_WITHIN, socket.send_to(b"back", peer_addr))
        .await
        .expect("the send doesn't hang");
    sent.expect("the port is free again: the send binds it and goes through");
    assert!(!socket.is_broken());
    assert_eq!(socket.local_addr().unwrap().port(), port, "the same port as before");

    let mut buf = [0u8; 16];
    let (n, from) = peer.recv_from(&mut buf).unwrap();
    assert_eq!(&buf[..n], b"back");
    assert_eq!(from.port(), port);

    // And it hears again.
    peer.send_to(b"hello", (Ipv4Addr::LOCALHOST, port)).unwrap();
    let mut buf = [0u8; 16];
    let (n, _) = tokio::time::timeout(HEALED_WITHIN, socket.recv_from(&mut buf))
        .await
        .expect("the answer arrives")
        .unwrap();
    assert_eq!(&buf[..n], b"hello");
}

/// The fix's boundary: a socket closed on purpose (iroh closing its
/// endpoint) stays closed, and a send on it doesn't open it again.
#[tokio::test]
async fn a_socket_closed_on_purpose_stays_closed() {
    let socket = UdpSocket::bind_full((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let peer = std::net::UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    socket.close().await;
    assert!(!socket.is_broken());

    let err = socket
        .send_to(b"x", peer.local_addr().unwrap())
        .await
        .expect_err("a closed socket doesn't send");
    assert_eq!(err.kind(), std::io::ErrorKind::BrokenPipe);
    assert!(!socket.is_broken());
    assert!(socket.is_closed());
}

#[cfg(target_vendor = "apple")]
fn hold(port: u16) -> socket2::Socket {
    use socket2::{Domain, Protocol, Socket, Type};
    let holder = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP)).unwrap();
    holder.set_reuse_address(true).unwrap();
    let addr: SocketAddr = (Ipv4Addr::LOCALHOST, port).into();
    holder.bind(&addr.into()).unwrap();
    holder
}
