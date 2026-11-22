//! A fake HTTP server: what it was sent, and what it answers.

use std::sync::{Arc, Mutex};

use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// One request as the fake server read it.
#[derive(Clone, Debug)]
pub struct Seen {
    pub head: String,
    pub body: String,
}

impl Seen {
    pub fn header(&self, name: &str) -> Option<String> {
        let prefix = format!("{}:", name.to_ascii_lowercase());
        self.head.lines().find_map(|line| line.to_ascii_lowercase().starts_with(&prefix).then(|| line[prefix.len()..].trim().to_owned()))
    }

    pub fn json(&self) -> serde_json::Value {
        serde_json::from_str(&self.body).unwrap_or_default()
    }
}

/// Answers every request with `answer(request)`: a status and a body.
pub struct Fake {
    pub url: String,
    pub seen: Arc<Mutex<Vec<Seen>>>,
}

pub async fn fake(answer: impl Fn(&Seen) -> (u16, String) + Send + Sync + 'static) -> Fake {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let url = format!("http://127.0.0.1:{}", listener.local_addr().expect("addr").port());
    let seen = Arc::new(Mutex::new(Vec::new()));
    let (log, answer) = (seen.clone(), Arc::new(answer));
    tokio::spawn(async move {
        loop {
            let Ok((mut stream, _)) = listener.accept().await else { return };
            let (log, answer) = (log.clone(), answer.clone());
            tokio::spawn(async move {
                let mut request = Vec::new();
                let mut buffer = [0u8; 8192];
                let seen = loop {
                    let Ok(read) = stream.read(&mut buffer).await else { return };
                    if read == 0 {
                        return;
                    }
                    request.extend_from_slice(&buffer[..read]);
                    let text = String::from_utf8_lossy(&request).to_string();
                    if let Some((head, body)) = text.split_once("\r\n\r\n") {
                        let seen = Seen { head: head.to_owned(), body: body.to_owned() };
                        let length: usize = seen.header("content-length").and_then(|value| value.parse().ok()).unwrap_or(0);
                        if body.len() >= length {
                            break seen;
                        }
                    }
                };
                let (status, body) = answer(&seen);
                log.lock().expect("lock").push(seen);
                let response = format!(
                    "HTTP/1.1 {status} X\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = stream.write_all(response.as_bytes()).await;
            });
        }
    });
    Fake { url, seen }
}

/// `der` as a PEM file's text.
pub fn pem(label: &str, der: &[u8]) -> String {
    use base64::Engine;
    let body = base64::engine::general_purpose::STANDARD.encode(der);
    let lines: Vec<&str> = body.as_bytes().chunks(64).map(|line| std::str::from_utf8(line).expect("ascii")).collect();
    format!("-----BEGIN {label}-----\n{}\n-----END {label}-----\n", lines.join("\n"))
}
