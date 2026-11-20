//! A stand-in for a service's HTTP API in tests (connectors): each request
//! is answered by the test's own function, and every request is recorded.

use std::sync::{Arc, Mutex};

use serde_json::Value;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

#[derive(Clone, Debug)]
pub struct Request {
    pub method: String,
    /// The path and query, as sent.
    pub target: String,
    pub headers: Vec<(String, String)>,
    pub body: String,
}

impl Request {
    pub fn path(&self) -> &str {
        self.target.split('?').next().unwrap_or_default()
    }

    pub fn query(&self, name: &str) -> Option<String> {
        let query = self.target.split_once('?')?.1;
        reqwest::Url::parse(&format!("http://x/?{query}"))
            .ok()?
            .query_pairs()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.into_owned())
    }

    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.iter().find(|(key, _)| key.eq_ignore_ascii_case(name)).map(|(_, value)| value.as_str())
    }

    pub fn json(&self) -> Value {
        serde_json::from_str(&self.body).unwrap_or_default()
    }

    pub fn form(&self, name: &str) -> Option<String> {
        reqwest::Url::parse(&format!("http://x/?{}", self.body))
            .ok()?
            .query_pairs()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.into_owned())
    }
}

pub type Handler = Arc<dyn Fn(&Request) -> (u16, Value) + Send + Sync>;

pub struct StandIn {
    pub base: String,
    pub requests: Arc<Mutex<Vec<Request>>>,
}

impl StandIn {
    pub async fn start(handler: impl Fn(&Request) -> (u16, Value) + Send + Sync + 'static) -> StandIn {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let base = format!("http://{}", listener.local_addr().expect("addr"));
        let requests = Arc::new(Mutex::new(Vec::new()));
        let handler: Handler = Arc::new(handler);
        let recorded = requests.clone();
        tokio::spawn(async move {
            while let Ok((mut socket, _)) = listener.accept().await {
                let handler = handler.clone();
                let recorded = recorded.clone();
                tokio::spawn(async move {
                    let Some(request) = read(&mut socket).await else { return };
                    let (status, body) = handler(&request);
                    recorded.lock().expect("lock").push(request);
                    let body = body.to_string();
                    let head = format!(
                        "HTTP/1.1 {status} X\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
                        body.len()
                    );
                    let _ = socket.write_all(head.as_bytes()).await;
                    let _ = socket.write_all(body.as_bytes()).await;
                });
            }
        });
        StandIn { base, requests }
    }

    pub fn requests(&self) -> Vec<Request> {
        self.requests.lock().expect("lock").clone()
    }
}

async fn read(socket: &mut tokio::net::TcpStream) -> Option<Request> {
    let mut buffer = Vec::new();
    let mut chunk = [0u8; 8192];
    let head_end = loop {
        let read = socket.read(&mut chunk).await.ok()?;
        if read == 0 {
            return None;
        }
        buffer.extend_from_slice(&chunk[..read]);
        if let Some(at) = buffer.windows(4).position(|window| window == b"\r\n\r\n") {
            break at + 4;
        }
    };
    let head = String::from_utf8_lossy(&buffer[..head_end]).into_owned();
    let mut lines = head.split("\r\n");
    let mut first = lines.next()?.split(' ');
    let (method, target) = (first.next()?.to_owned(), first.next()?.to_owned());
    let headers: Vec<(String, String)> =
        lines.filter_map(|line| line.split_once(':')).map(|(key, value)| (key.trim().to_owned(), value.trim().to_owned())).collect();
    let length: usize =
        headers.iter().find(|(key, _)| key.eq_ignore_ascii_case("content-length")).and_then(|(_, value)| value.parse().ok()).unwrap_or(0);
    while buffer.len() < head_end + length {
        let read = socket.read(&mut chunk).await.ok()?;
        if read == 0 {
            break;
        }
        buffer.extend_from_slice(&chunk[..read]);
    }
    let body = String::from_utf8_lossy(&buffer[head_end..(head_end + length).min(buffer.len())]).into_owned();
    Some(Request { method, target, headers, body })
}
