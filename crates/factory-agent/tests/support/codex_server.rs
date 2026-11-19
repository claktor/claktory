#![allow(dead_code)]

use std::sync::Arc;
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use parking_lot::Mutex;
use serde_json::Value;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::task::JoinHandle;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::protocol::CloseFrame;
use tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode;

#[derive(Clone, Debug)]
pub enum Action {
    Send(Value),
    SendText(String),
    Close(u16, String),
    CloseLater(u64, u16, String),
    CloseWithoutStatus,
    Drop,
}

/// What the server does with the `frame_index`-th frame on the `connection`-th socket.
pub type Script = Arc<dyn Fn(usize, usize, &Value) -> Vec<Action> + Send + Sync>;

#[derive(Clone, Debug, Default)]
pub struct Connection {
    pub headers: Vec<(String, String)>,
    pub frames: Vec<Value>,
    pub closed: Option<(u16, String)>,
}

#[derive(Clone, Debug)]
pub struct SseRequest {
    pub headers: Vec<(String, String)>,
    pub body: Value,
}

#[derive(Default)]
pub struct Recorded {
    pub connections: Vec<Connection>,
    pub sse: Vec<SseRequest>,
}

pub struct CodexServer {
    pub base_url: String,
    pub recorded: Arc<Mutex<Recorded>>,
    pub task: JoinHandle<()>,
}

impl Drop for CodexServer {
    fn drop(&mut self) {
        self.task.abort();
    }
}

fn close_frame(code: u16, reason: &str) -> Message {
    Message::Close(Some(CloseFrame { code: CloseCode::from(code), reason: reason.to_owned().into() }))
}

#[allow(clippy::result_large_err)]
async fn serve_socket(stream: TcpStream, index: usize, script: Script, recorded: Arc<Mutex<Recorded>>) {
    let headers = Arc::new(Mutex::new(Vec::new()));
    let seen = headers.clone();
    let Ok(mut socket) = tokio_tungstenite::accept_hdr_async(
        stream,
        move |request: &tokio_tungstenite::tungstenite::handshake::server::Request, response| {
            *seen.lock() = request
                .headers()
                .iter()
                .map(|(name, value)| (name.as_str().to_owned(), value.to_str().unwrap_or_default().to_owned()))
                .collect();
            Ok(response)
        },
    )
    .await
    else {
        return;
    };
    recorded.lock().connections[index].headers = headers.lock().clone();
    let mut frame_index = 0;
    while let Some(Ok(message)) = socket.next().await {
        match message {
            Message::Text(text) => {
                let frame: Value = serde_json::from_str(&text).expect("frame json");
                recorded.lock().connections[index].frames.push(frame.clone());
                for action in script(index, frame_index, &frame) {
                    match action {
                        Action::Send(value) => socket.send(Message::Text(value.to_string().into())).await.expect("send"),
                        Action::SendText(text) => socket.send(Message::Text(text.into())).await.expect("send"),
                        Action::Close(code, reason) => {
                            let _ = socket.send(close_frame(code, &reason)).await;
                        }
                        Action::CloseLater(delay, code, reason) => {
                            tokio::time::sleep(Duration::from_millis(delay)).await;
                            let _ = socket.send(close_frame(code, &reason)).await;
                        }
                        Action::CloseWithoutStatus => {
                            let _ = socket.send(Message::Close(None)).await;
                        }
                        Action::Drop => return,
                    }
                }
                frame_index += 1;
            }
            Message::Close(frame) => {
                recorded.lock().connections[index].closed =
                    Some(frame.map_or((1005, String::new()), |frame| (frame.code.into(), frame.reason.to_string())));
                break;
            }
            _ => {}
        }
    }
}

async fn serve_http(mut stream: TcpStream, sse_events: Vec<Value>, recorded: Arc<Mutex<Recorded>>) {
    let mut raw = Vec::new();
    let mut buffer = [0u8; 4096];
    let (head_end, length) = loop {
        let Ok(read) = stream.read(&mut buffer).await else { return };
        if read == 0 {
            return;
        }
        raw.extend_from_slice(&buffer[..read]);
        if let Some(head_end) = raw.windows(4).position(|window| window == b"\r\n\r\n") {
            let head = String::from_utf8_lossy(&raw[..head_end]).to_ascii_lowercase();
            let length: usize =
                head.lines().find_map(|line| line.strip_prefix("content-length:")).and_then(|value| value.trim().parse().ok()).unwrap_or(0);
            if raw.len() >= head_end + 4 + length {
                break (head_end + 4, length);
            }
        }
    };
    let head = String::from_utf8_lossy(&raw[..head_end]).into_owned();
    let headers = head
        .lines()
        .skip(1)
        .filter_map(|line| line.split_once(':'))
        .map(|(name, value)| (name.trim().to_ascii_lowercase(), value.trim().to_owned()))
        .collect();
    let body = serde_json::from_slice(&raw[head_end..head_end + length]).unwrap_or(Value::Null);
    recorded.lock().sse.push(SseRequest { headers, body });
    let text: String =
        sse_events.iter().map(|event| format!("event: {}\ndata: {}\n\n", event["type"].as_str().unwrap_or_default(), event)).collect();
    let response =
        format!("HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{text}", text.len());
    let _ = stream.write_all(response.as_bytes()).await;
    let _ = stream.shutdown().await;
}

/// `sse_events` answers every SSE fallback request; `script` answers socket frames.
pub async fn serve(script: Script, sse_events: Vec<Value>) -> CodexServer {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let base_url = format!("http://{}/backend-api", listener.local_addr().expect("addr"));
    let recorded = Arc::new(Mutex::new(Recorded::default()));
    let shared = recorded.clone();
    let task = tokio::spawn(async move {
        loop {
            let Ok((stream, _)) = listener.accept().await else { return };
            let mut peek = [0u8; 1024];
            let Ok(read) = stream.peek(&mut peek).await else { continue };
            let head = String::from_utf8_lossy(&peek[..read]).to_ascii_lowercase();
            if head.contains("upgrade: websocket") {
                let index = {
                    let mut recorded = shared.lock();
                    recorded.connections.push(Connection::default());
                    recorded.connections.len() - 1
                };
                tokio::spawn(serve_socket(stream, index, script.clone(), shared.clone()));
            } else {
                tokio::spawn(serve_http(stream, sse_events.clone(), shared.clone()));
            }
        }
    });
    CodexServer { base_url, recorded, task }
}
