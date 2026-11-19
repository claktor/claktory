#![allow(dead_code)]

use std::sync::Arc;
use std::time::Duration;

use parking_lot::Mutex;
use serde_json::Value;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

/// How long a test waits for something that should end before calling it a
/// hang. Never a measure of speed: a test process starved of the processor
/// (agents' commands run throttled, beside builds) can stall for seconds, and
/// only a real hang should fail.
pub const HANG: Duration = Duration::from_secs(60);

/// Waits until `condition` holds, for what happens on its own time (a socket
/// closing, a session dropping); fails only after [`HANG`].
pub async fn eventually(what: &str, mut condition: impl FnMut() -> bool) {
    let deadline = tokio::time::Instant::now() + HANG;
    while !condition() {
        assert!(tokio::time::Instant::now() < deadline, "{what}: still not so after {HANG:?}");
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
}

#[derive(Clone, Debug)]
pub struct Served {
    pub status: u16,
    pub content_type: &'static str,
    pub chunks: Vec<String>,
    pub hold: bool,
    pub hold_before_headers: bool,
}

#[derive(Clone, Debug)]
pub struct Recorded {
    pub method: String,
    pub path: String,
    pub headers: Vec<(String, String)>,
    pub body: Value,
    pub raw: String,
}

/// An HTTP/1.1 responder: one request per connection, a chunked body with a pause between chunks.
pub struct Server {
    pub base_url: String,
    pub requests: Arc<Mutex<Vec<Recorded>>>,
    pub task: JoinHandle<()>,
}

impl Server {
    /// Cancels `cancel` once the server has read a request, so a cancellation
    /// meant for "before the response" can't land before the request is sent.
    pub fn cancel_once_requested(&self, cancel: CancellationToken) {
        let requests = self.requests.clone();
        tokio::spawn(async move {
            while requests.lock().is_empty() {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
            cancel.cancel();
        });
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.task.abort();
    }
}

pub fn served_json(status: u16, body: Value) -> Served {
    Served { status, content_type: "application/json", chunks: vec![body.to_string()], hold: false, hold_before_headers: false }
}

pub fn served_text(status: u16, body: &str) -> Served {
    Served { status, content_type: "text/plain", chunks: vec![body.to_owned()], hold: false, hold_before_headers: false }
}

/// The port of a sign-in flow's redirect URI, `http://localhost:<port><path>`:
/// flows given port 0 listen on a free one and name it there.
pub fn redirect_port(redirect: &str, path: &str) -> u16 {
    redirect
        .strip_prefix("http://localhost:")
        .and_then(|rest| rest.strip_suffix(path))
        .and_then(|port| port.parse().ok())
        .unwrap_or_else(|| panic!("redirect {redirect}"))
}

fn content_length(headers: &[(String, String)]) -> usize {
    headers.iter().find(|(name, _)| name == "content-length").and_then(|(_, value)| value.parse().ok()).unwrap_or(0)
}

fn parse_head(raw: &[u8]) -> Option<(Recorded, usize)> {
    let head_end = raw.windows(4).position(|window| window == b"\r\n\r\n")?;
    let head = String::from_utf8_lossy(&raw[..head_end]);
    let mut lines = head.lines();
    let mut parts = lines.next().unwrap_or_default().split(' ');
    let method = parts.next().unwrap_or_default().to_owned();
    let path = parts.next().unwrap_or_default().to_owned();
    let headers: Vec<(String, String)> = lines
        .filter_map(|line| line.split_once(':'))
        .map(|(name, value)| (name.trim().to_ascii_lowercase(), value.trim().to_owned()))
        .collect();
    Some((Recorded { method, path, headers, body: Value::Null, raw: String::new() }, head_end + 4))
}

pub async fn serve(responses: Vec<Served>) -> Server {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let base_url = format!("http://{}", listener.local_addr().expect("addr"));
    let requests = Arc::new(Mutex::new(Vec::new()));
    let recorded = requests.clone();
    let task = tokio::spawn(async move {
        let mut responses = responses.into_iter();
        let mut last: Option<Served> = None;
        loop {
            let Ok((mut socket, _)) = listener.accept().await else { return };
            let mut raw = Vec::new();
            let (request, body_start) = loop {
                let mut buffer = [0u8; 4096];
                let Ok(read) = socket.read(&mut buffer).await else { return };
                if read == 0 {
                    return;
                }
                raw.extend_from_slice(&buffer[..read]);
                if let Some((request, body_start)) = parse_head(&raw)
                    && raw.len() >= body_start + content_length(&request.headers)
                {
                    break (request, body_start);
                }
            };
            let body_end = body_start + content_length(&request.headers);
            let body = serde_json::from_slice(&raw[body_start..body_end]).unwrap_or(Value::Null);
            let raw = String::from_utf8_lossy(&raw[body_start..body_end]).into_owned();
            recorded.lock().push(Recorded { body, raw, ..request });
            let served = responses.next().or_else(|| last.clone()).expect("a response to serve");
            last = Some(served.clone());
            let head = format!(
                "HTTP/1.1 {} X\r\ncontent-type: {}\r\ntransfer-encoding: chunked\r\nconnection: close\r\n\r\n",
                served.status, served.content_type
            );
            // Held until the test drops the server.
            if served.hold_before_headers {
                std::future::pending::<()>().await;
            }
            if socket.write_all(head.as_bytes()).await.is_err() {
                continue;
            }
            for chunk in &served.chunks {
                let framed = format!("{:x}\r\n{chunk}\r\n", chunk.len());
                if socket.write_all(framed.as_bytes()).await.is_err() {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(1)).await;
            }
            if served.hold {
                std::future::pending::<()>().await;
            }
            let _ = socket.write_all(b"0\r\n\r\n").await;
            let _ = socket.shutdown().await;
        }
    });
    Server { base_url, requests, task }
}
