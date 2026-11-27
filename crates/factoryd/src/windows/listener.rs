//! The backend's loopback listener on Windows, bound for itself alone
//! (Fugo 48392 c). With SO_EXCLUSIVEADDRUSE no other process, another
//! account's included, can bind the same address and port while the
//! backend holds it, so nothing can listen beside it and take the app's
//! connections; the app's own check of whom it reached (the server end of
//! its connection runs as its user) covers the time the backend is down.
//! Not inherited by any program the backend starts.

use std::net::{SocketAddr, TcpListener};
use std::os::windows::io::{FromRawSocket, RawSocket};

use windows_sys::Win32::Networking::WinSock::{
    AF_INET, AF_INET6, IN_ADDR, IN_ADDR_0, IN6_ADDR, IN6_ADDR_0, INVALID_SOCKET, IPPROTO_TCP, SO_EXCLUSIVEADDRUSE, SOCK_STREAM, SOCKADDR,
    SOCKADDR_IN, SOCKADDR_IN6, SOCKADDR_IN6_0, SOCKET, SOCKET_ERROR, SOL_SOCKET, SOMAXCONN, WSA_FLAG_NO_HANDLE_INHERIT, WSA_FLAG_OVERLAPPED,
    WSADATA, WSAGetLastError, WSASocketW, WSAStartup, bind, closesocket, listen, setsockopt,
};

/// A listener on `address` that only this process can hold, for
/// `tokio::net::TcpListener::from_std` (it's non-blocking already).
pub fn bind_exclusive(address: SocketAddr) -> std::io::Result<TcpListener> {
    start_winsock()?;
    let family = if address.is_ipv4() { AF_INET } else { AF_INET6 };
    // SAFETY: plain call; the socket is checked, and closed on every error.
    let socket = unsafe { WSASocketW(i32::from(family), SOCK_STREAM, IPPROTO_TCP, std::ptr::null(), 0, WSA_FLAG_OVERLAPPED | WSA_FLAG_NO_HANDLE_INHERIT) };
    if socket == INVALID_SOCKET {
        return Err(last_error());
    }
    let socket = Owned(socket);
    let on: i32 = 1;
    // SAFETY: one i32 option, by pointer and size.
    if unsafe { setsockopt(socket.0, SOL_SOCKET, SO_EXCLUSIVEADDRUSE, (&raw const on).cast(), size_of::<i32>() as i32) } == SOCKET_ERROR {
        return Err(last_error());
    }
    let bound = match address {
        SocketAddr::V4(v4) => {
            let raw = SOCKADDR_IN {
                sin_family: AF_INET,
                sin_port: v4.port().to_be(),
                sin_addr: IN_ADDR { S_un: IN_ADDR_0 { S_addr: u32::from_ne_bytes(v4.ip().octets()) } },
                sin_zero: [0; 8],
            };
            // SAFETY: the address and its size.
            unsafe { bind(socket.0, (&raw const raw).cast::<SOCKADDR>(), size_of::<SOCKADDR_IN>() as i32) }
        }
        SocketAddr::V6(v6) => {
            let raw = SOCKADDR_IN6 {
                sin6_family: AF_INET6,
                sin6_port: v6.port().to_be(),
                sin6_flowinfo: v6.flowinfo(),
                sin6_addr: IN6_ADDR { u: IN6_ADDR_0 { Byte: v6.ip().octets() } },
                Anonymous: SOCKADDR_IN6_0 { sin6_scope_id: v6.scope_id() },
            };
            // SAFETY: the address and its size.
            unsafe { bind(socket.0, (&raw const raw).cast::<SOCKADDR>(), size_of::<SOCKADDR_IN6>() as i32) }
        }
    };
    if bound == SOCKET_ERROR {
        return Err(last_error());
    }
    // SAFETY: a bound socket.
    if unsafe { listen(socket.0, SOMAXCONN as i32) } == SOCKET_ERROR {
        return Err(last_error());
    }
    // SAFETY: an open socket this takes over; Owned no longer closes it.
    let listener = unsafe { TcpListener::from_raw_socket(socket.take() as RawSocket) };
    listener.set_nonblocking(true)?;
    Ok(listener)
}

/// Windows Sockets started for this process; counted, so starting it again
/// where std already has is harmless.
fn start_winsock() -> std::io::Result<()> {
    static STARTED: std::sync::OnceLock<i32> = std::sync::OnceLock::new();
    let result = *STARTED.get_or_init(|| {
        // SAFETY: an all-zero WSADATA is valid to write over.
        let mut data: WSADATA = unsafe { std::mem::zeroed() };
        // SAFETY: version 2.2 and the structure to fill.
        unsafe { WSAStartup(0x0202, &raw mut data) }
    });
    if result == 0 { Ok(()) } else { Err(std::io::Error::from_raw_os_error(result)) }
}

fn last_error() -> std::io::Error {
    // SAFETY: plain call.
    std::io::Error::from_raw_os_error(unsafe { WSAGetLastError() })
}

/// A socket closed when dropped, until it's taken.
struct Owned(SOCKET);

impl Owned {
    fn take(mut self) -> SOCKET {
        std::mem::replace(&mut self.0, INVALID_SOCKET)
    }
}

impl Drop for Owned {
    fn drop(&mut self) {
        if self.0 != INVALID_SOCKET {
            // SAFETY: a socket this owns, closed once.
            unsafe { closesocket(self.0) };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{Ipv4Addr, SocketAddrV4};

    #[test]
    fn no_one_else_can_bind_beside_it() {
        let listener = bind_exclusive(SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0))).expect("bound");
        let address = listener.local_addr().unwrap();
        assert_ne!(address.port(), 0);
        // Plain binds of the same address and port are refused, as another
        // program's would be.
        assert!(std::net::TcpListener::bind(address).is_err());
        assert!(bind_exclusive(address).is_err());
        // It accepts.
        let client = std::net::TcpStream::connect(address).expect("connects");
        let mut accepted = None;
        for _ in 0..100 {
            match listener.accept() {
                Ok((stream, _)) => {
                    accepted = Some(stream);
                    break;
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => std::thread::sleep(std::time::Duration::from_millis(10)),
                Err(error) => panic!("{error}"),
            }
        }
        assert!(accepted.is_some());
        drop(client);
        // Free again once it's closed.
        drop(listener);
        drop(accepted);
        bind_exclusive(address).expect("bound again after it closed");
    }
}
