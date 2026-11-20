use std::net::SocketAddr;

use crate::common::{start, temp_dir};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

/// Sends a WebSocket handshake, with `extra` headers, and returns the status line of the answer.
async fn handshake(addr: SocketAddr, extra: &str) -> String {
    let mut stream = TcpStream::connect(addr).await.expect("connect");
    let request = format!(
        "GET / HTTP/1.1\r\nHost: {addr}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n\
         Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\nSec-WebSocket-Version: 13\r\n{extra}\r\n"
    );
    stream.write_all(request.as_bytes()).await.expect("write");
    let mut answer = Vec::new();
    let mut buffer = [0; 1024];
    while !answer.windows(4).any(|end| end == b"\r\n\r\n") {
        let read = stream.read(&mut buffer).await.expect("read");
        if read == 0 {
            break;
        }
        answer.extend_from_slice(&buffer[..read]);
    }
    String::from_utf8_lossy(&answer).lines().next().unwrap_or_default().to_owned()
}

#[tokio::test]
async fn a_web_page_cannot_connect_but_the_app_can() {
    let (addr, _factory) = start(&temp_dir()).await;
    assert_eq!(handshake(addr, "Origin: https://evil.example\r\n").await, "HTTP/1.1 403 Forbidden");
    assert_eq!(handshake(addr, "Origin: null\r\n").await, "HTTP/1.1 403 Forbidden");
    assert_eq!(handshake(addr, "").await, "HTTP/1.1 101 Switching Protocols");
}
