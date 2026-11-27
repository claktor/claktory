use hashbrown::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, LazyLock};
use std::time::{Duration, Instant};

use futures_util::{SinkExt, StreamExt};
use parking_lot::Mutex;
use serde_json::Value;
use tokio::net::TcpStream;
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message as Frame;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::protocol::CloseFrame;
use tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};
use tokio_util::sync::CancellationToken;

use crate::providers::openai_codex::{CodexFailure, Mapped, Request, map_codex_event};
use crate::providers::openai_responses::{Parser, convert_input};
use crate::stream::{EventSink, LlmContext, StreamOptions};
use crate::types::{Message, Model, StreamEvent};

const CONNECT_TIMEOUT: Duration = Duration::from_millis(15_000);
const CACHE_TTL: Duration = Duration::from_secs(5 * 60);
const MAX_AGE: Duration = Duration::from_secs(55 * 60);
const MESSAGE_TOO_BIG: u16 = 1009;

type Socket = WebSocketStream<MaybeTlsStream<TcpStream>>;

struct Job {
    frame: String,
    events: mpsc::UnboundedSender<Result<Value, CodexFailure>>,
    cancel: CancellationToken,
    idle_timeout: Option<Duration>,
}

/// What the next request on the socket needs of the last one to send only
/// what's new: digests, not the request itself, which carries every
/// image of the conversation.
struct Continuation {
    /// The last request's body, all but its input.
    last_rest: Digest,
    /// How many items the last request's input and its response's own
    /// items make together, and their digest: the start the next request's
    /// input must have.
    baseline_len: usize,
    baseline: Digest,
    last_response_id: String,
}

type Digest = [u8; 32];

/// Feeds what serde_json writes into a SHA-256.
struct Hashing(sha2::Sha256);

impl std::io::Write for Hashing {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        sha2::Digest::update(&mut self.0, bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// The digest of `values` in order, each as its JSON.
fn digest<'a>(values: impl IntoIterator<Item = &'a Value>) -> Digest {
    let mut hashing = Hashing(<sha2::Sha256 as sha2::Digest>::new());
    for value in values {
        // A newline never occurs in compact JSON: it ends each one.
        let _ = serde_json::to_writer(&mut hashing, value);
        sha2::Digest::update(&mut hashing.0, b"\n");
    }
    sha2::Digest::finalize(hashing.0).into()
}

/// The digest of a request body without its input and continuation, the
/// part a delta must share with the last request.
fn rest_digest(body: &Value) -> Digest {
    let mut hashing = Hashing(<sha2::Sha256 as sha2::Digest>::new());
    for (key, value) in body.as_object().into_iter().flatten() {
        if key == "input" || key == "previous_response_id" {
            continue;
        }
        let _ = serde_json::to_writer(&mut hashing, key);
        let _ = serde_json::to_writer(&mut hashing, value);
        sha2::Digest::update(&mut hashing.0, b"\n");
    }
    sha2::Digest::finalize(hashing.0).into()
}

/// One task owns each socket; requests queue through `jobs`, the task notices server closes while idle.
pub(crate) struct Connection {
    jobs: mpsc::Sender<Job>,
    close: mpsc::Sender<(u16, String)>,
    created_at: Instant,
    busy: AtomicBool,
    closed: Arc<AtomicBool>,
    continuation: Mutex<Option<Continuation>>,
}

type Cache = HashMap<String, HashMap<String, Arc<Connection>>>;

static CACHE: LazyLock<Mutex<Cache>> = LazyLock::new(|| Mutex::new(HashMap::new()));

fn transport(message: impl Into<String>, close_code: Option<u16>) -> CodexFailure {
    CodexFailure::Transport { message: message.into(), close_code }
}

/// The browser socket's texts: an unclean end is an `error` event, a close frame without a status is code 1005.
fn close_error(frame: Option<CloseFrame>) -> CodexFailure {
    let code: u16 = frame.as_ref().map_or(1005, |frame| frame.code.into());
    let mut reason = frame.map(|frame| frame.reason.to_string()).unwrap_or_default();
    if reason.is_empty() && code == MESSAGE_TOO_BIG {
        reason = "message too big".into();
    }
    let reason = if reason.is_empty() { String::new() } else { format!(" {reason}") };
    transport(format!("WebSocket closed {code}{reason}"), Some(code))
}

async fn close_quietly(socket: &mut Socket, code: u16, reason: &str) {
    let _ = socket.send(Frame::Close(Some(CloseFrame { code: CloseCode::from(code), reason: reason.to_owned().into() }))).await;
    let _ = socket.close(None).await;
}

fn is_terminal(kind: Option<&str>) -> bool {
    matches!(kind, Some("response.completed" | "response.done" | "response.incomplete"))
}

/// Sends one `response.create` frame and relays events until the terminal one; false when the socket is no longer usable.
async fn serve(socket: &mut Socket, job: Job) -> bool {
    if let Err(error) = socket.send(Frame::Text(job.frame.into())).await {
        let _ = job.events.send(Err(transport(error.to_string(), None)));
        return false;
    }
    loop {
        let idle = job.idle_timeout.unwrap_or(Duration::from_secs(u64::MAX / 4));
        let next = tokio::select! {
            message = socket.next() => message,
            () = job.events.closed() => return true,
            () = job.cancel.cancelled() => {
                let _ = job.events.send(Err(transport("Request was aborted", None)));
                close_quietly(socket, 1000, "aborted").await;
                return false;
            }
            () = tokio::time::sleep(idle), if job.idle_timeout.is_some() => {
                let _ = job.events.send(Err(transport(format!("WebSocket idle timeout after {}ms", idle.as_millis()), None)));
                close_quietly(socket, 1000, "idle_timeout").await;
                return false;
            }
        };
        let text = match next {
            Some(Ok(Frame::Text(text))) => text.to_string(),
            Some(Ok(Frame::Binary(bytes))) => String::from_utf8_lossy(&bytes).into_owned(),
            Some(Ok(Frame::Ping(_))) => {
                let _ = socket.flush().await;
                continue;
            }
            Some(Ok(Frame::Pong(_) | Frame::Frame(_))) => continue,
            Some(Ok(Frame::Close(frame))) => {
                let _ = job.events.send(Err(close_error(frame)));
                return false;
            }
            Some(Err(_)) | None => {
                let _ = job.events.send(Err(transport("WebSocket error", None)));
                return false;
            }
        };
        match serde_json::from_str::<Value>(&text) {
            Ok(event) => {
                let terminal = is_terminal(event["type"].as_str());
                let _ = job.events.send(Ok(event));
                if terminal {
                    return true;
                }
            }
            Err(error) => {
                let _ = job.events.send(Err(CodexFailure::Protocol(format!("Invalid Codex WebSocket JSON: {error}"))));
                return true;
            }
        }
    }
}

/// Owns the socket until it closes; a cached connection then leaves the cache, so a socket the server closed
/// while idle is not kept around.
async fn drive(
    mut socket: Socket,
    mut jobs: mpsc::Receiver<Job>,
    mut close: mpsc::Receiver<(u16, String)>,
    closed: Arc<AtomicBool>,
    cached: Option<(String, String)>,
) {
    loop {
        tokio::select! {
            biased;
            request = close.recv() => {
                let (code, reason) = request.unwrap_or((1000, "done".into()));
                close_quietly(&mut socket, code, &reason).await;
                break;
            }
            job = jobs.recv() => match job {
                None => {
                    let (code, reason) = close.try_recv().unwrap_or((1000, "done".into()));
                    close_quietly(&mut socket, code, &reason).await;
                    break;
                }
                Some(job) => {
                    if !serve(&mut socket, job).await {
                        break;
                    }
                }
            },
            message = socket.next() => match message {
                Some(Ok(Frame::Ping(_))) => {
                    let _ = socket.flush().await;
                }
                Some(Ok(Frame::Pong(_) | Frame::Frame(_) | Frame::Text(_) | Frame::Binary(_))) => {}
                _ => break,
            },
            () = tokio::time::sleep(CACHE_TTL) => {
                close_quietly(&mut socket, 1000, "idle_timeout").await;
                break;
            }
        }
    }
    closed.store(true, Ordering::SeqCst);
    if let Some((session, account)) = cached {
        remove(&session, &account, &closed);
    }
}

/// `connect_async` with Factory's TLS settings (`providers::tls_config`).
async fn connect_async(
    request: tokio_tungstenite::tungstenite::handshake::client::Request,
) -> Result<
    (WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>, tokio_tungstenite::tungstenite::handshake::client::Response),
    tokio_tungstenite::tungstenite::Error,
> {
    let tls = tokio_tungstenite::Connector::Rustls(crate::providers::tls_config());
    tokio_tungstenite::connect_async_tls_with_config(request, None, false, Some(tls)).await
}

async fn connect(
    url: &str,
    headers: &[(String, String)],
    cancel: &CancellationToken,
    cached: Option<(String, String)>,
) -> Result<Arc<Connection>, CodexFailure> {
    let mut request = url.into_client_request().map_err(|error| transport(error.to_string(), None))?;
    for (name, value) in headers {
        let name = http::header::HeaderName::from_bytes(name.as_bytes()).map_err(|error| transport(error.to_string(), None))?;
        let value = http::header::HeaderValue::from_str(value).map_err(|error| transport(error.to_string(), None))?;
        request.headers_mut().insert(name, value);
    }
    let socket = tokio::select! {
        connected = tokio::time::timeout(CONNECT_TIMEOUT, connect_async(request)) => match connected {
            Ok(Ok((socket, _))) => socket,
            Ok(Err(error)) => return Err(transport(error.to_string(), None)),
            Err(_) => return Err(transport(format!("WebSocket connect timeout after {}ms", CONNECT_TIMEOUT.as_millis()), None)),
        },
        () = cancel.cancelled() => return Err(transport("Request was aborted", None)),
    };
    let (jobs, jobs_rx) = mpsc::channel(1);
    let (close, close_rx) = mpsc::channel(1);
    let closed = Arc::new(AtomicBool::new(false));
    tokio::spawn(drive(socket, jobs_rx, close_rx, closed.clone(), cached));
    Ok(Arc::new(Connection {
        jobs,
        close,
        created_at: Instant::now(),
        busy: AtomicBool::new(false),
        closed,
        continuation: Mutex::new(None),
    }))
}

/// Drops the cached connection whose `closed` flag this is, if it is still the cached one.
fn remove(session: &str, account: &str, closed: &Arc<AtomicBool>) {
    let mut cache = CACHE.lock();
    if let Some(accounts) = cache.get_mut(session) {
        if accounts.get(account).is_some_and(|cached| Arc::ptr_eq(&cached.closed, closed)) {
            accounts.remove(account);
        }
        if accounts.is_empty() {
            cache.remove(session);
        }
    }
}

impl Connection {
    fn request_close(&self, code: u16, reason: &str) {
        let _ = self.close.try_send((code, reason.to_owned()));
    }

    fn is_closed(&self) -> bool {
        self.closed.load(Ordering::SeqCst)
    }
}

struct Acquired {
    connection: Arc<Connection>,
    cached: Option<(String, String)>,
}

impl Acquired {
    fn release(self, keep: bool) {
        if !keep || self.connection.is_closed() {
            self.connection.request_close(1000, "done");
            if let Some((session, account)) = &self.cached {
                remove(session, account, &self.connection.closed);
            }
            return;
        }
        self.connection.busy.store(false, Ordering::SeqCst);
    }
}

/// Pi's `acquireWebSocket`: one cached socket per session and account, replaced when closed, expired or busy.
async fn acquire(
    url: &str,
    headers: &[(String, String)],
    session: Option<&str>,
    account: &str,
    cancel: &CancellationToken,
) -> Result<Acquired, CodexFailure> {
    let Some(session) = session else {
        return Ok(Acquired { connection: connect(url, headers, cancel, None).await?, cached: None });
    };
    let existing = CACHE.lock().get(session).and_then(|accounts| accounts.get(account)).cloned();
    if let Some(cached) = existing {
        let busy = cached.busy.load(Ordering::SeqCst);
        if !busy && cached.created_at.elapsed() >= MAX_AGE {
            cached.request_close(1000, "connection_age_limit");
            remove(session, account, &cached.closed);
        } else if !busy && !cached.is_closed() {
            cached.busy.store(true, Ordering::SeqCst);
            return Ok(Acquired { connection: cached, cached: Some((session.to_owned(), account.to_owned())) });
        } else if busy {
            return Ok(Acquired { connection: connect(url, headers, cancel, None).await?, cached: None });
        } else {
            cached.request_close(1000, "done");
            remove(session, account, &cached.closed);
        }
    }
    let key = (session.to_owned(), account.to_owned());
    let connection = connect(url, headers, cancel, Some(key)).await?;
    connection.busy.store(true, Ordering::SeqCst);
    CACHE.lock().entry(session.to_owned()).or_default().insert(account.to_owned(), connection.clone());
    Ok(Acquired { connection, cached: Some((session.to_owned(), account.to_owned())) })
}

fn input_items(body: &Value) -> &[Value] {
    body["input"].as_array().map_or(&[], Vec::as_slice)
}

/// The items after the cached baseline (the previous request's input plus its response items), when the rest of the body matches.
fn delta<'a>(body: &'a Value, continuation: &Continuation) -> Option<&'a [Value]> {
    if rest_digest(body) != continuation.last_rest {
        return None;
    }
    let current = input_items(body);
    if current.len() < continuation.baseline_len || digest(&current[..continuation.baseline_len]) != continuation.baseline {
        return None;
    }
    Some(&current[continuation.baseline_len..])
}

/// A `response.create` frame: the body's fields after its type, as they
/// are, or with only the input that's new since the response it continues.
struct CreateFrame<'a> {
    body: &'a Value,
    continues: Option<(&'a str, &'a [Value])>,
}

impl serde::Serialize for CreateFrame<'_> {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeMap;
        let mut frame = serializer.serialize_map(None)?;
        let fields = self.body.as_object();
        match fields.and_then(|fields| fields.get("type")) {
            Some(kind) => frame.serialize_entry("type", kind)?,
            None => frame.serialize_entry("type", "response.create")?,
        }
        for (key, value) in fields.into_iter().flatten() {
            match (key.as_str(), self.continues) {
                ("type", _) => {}
                ("input", Some((_, delta))) => frame.serialize_entry(key, delta)?,
                ("previous_response_id", Some((id, _))) => frame.serialize_entry(key, id)?,
                _ => frame.serialize_entry(key, value)?,
            }
        }
        if let Some((id, _)) = self.continues
            && !fields.is_some_and(|fields| fields.contains_key("previous_response_id"))
        {
            frame.serialize_entry("previous_response_id", id)?;
        }
        frame.end()
    }
}

/// The frame for `body`: a delta when the socket's last response is its
/// start, else the whole body (and the socket forgets that response).
fn frame_for(connection: &Connection, body: &Value) -> String {
    let mut guard = connection.continuation.lock();
    let continues = guard.as_ref().and_then(|continuation| {
        let delta = delta(body, continuation)?;
        (!continuation.last_response_id.is_empty()).then_some((continuation.last_response_id.as_str(), delta))
    });
    let frame = serde_json::to_string(&CreateFrame { body, continues }).unwrap_or_default();
    if continues.is_none() {
        *guard = None;
    }
    frame
}

fn response_items(model: &Model, output: &crate::types::AssistantMessage) -> Vec<Value> {
    let context = LlmContext { system_prompt: None, messages: vec![Message::Assistant(output.clone())], tools: Vec::new() };
    convert_input(model, &context, false)
        .unwrap_or_default()
        .into_iter()
        .filter(|item| item["type"] != "function_call_output" && item["type"] != "custom_tool_call_output")
        .collect()
}

/// `processWebSocketStream`: one request on a cached or fresh socket, with the continuation delta and the start event on the first mapped event.
#[allow(clippy::too_many_arguments)]
pub async fn process(
    url: &str,
    request: &Request,
    model: &Model,
    options: &StreamOptions,
    parser: &mut Parser,
    sink: &EventSink,
    started: &mut bool,
    start_emitted: &mut bool,
) -> Result<(), CodexFailure> {
    let acquired = acquire(url, &request.websocket_headers, request.session_id.as_deref(), &request.account_id, &options.cancel).await?;
    let frame = if acquired.cached.is_some() {
        frame_for(&acquired.connection, &request.body)
    } else {
        serde_json::to_string(&CreateFrame { body: &request.body, continues: None }).unwrap_or_default()
    };
    let (events, mut received) = mpsc::unbounded_channel();
    let job = Job {
        frame,
        events,
        cancel: options.cancel.clone(),
        idle_timeout: options.timeout_ms.filter(|ms| *ms > 0).map(Duration::from_millis),
    };
    let outcome: Result<(), CodexFailure> = async {
        acquired.connection.jobs.send(job).await.map_err(|_| transport("WebSocket closed", None))?;
        let mut finished = false;
        while let Some(event) = received.recv().await {
            let event = event?;
            let mapped = map_codex_event(event, &mut parser.output)?;
            if matches!(mapped, Mapped::Skip) {
                continue;
            }
            if !*started {
                *started = true;
                if !*start_emitted {
                    *start_emitted = true;
                    sink.push(StreamEvent::Start { partial: parser.output.clone() });
                }
            }
            if finished {
                continue;
            }
            match mapped {
                Mapped::Event(event) => parser.handle(&event, sink, model).map_err(CodexFailure::Protocol)?,
                Mapped::Terminal(event) => {
                    finished = true;
                    parser.handle(&event, sink, model).map_err(CodexFailure::Protocol)?;
                }
                Mapped::Skip => {}
            }
        }
        if options.cancel.is_cancelled() {
            return Err(transport("Request was aborted", None));
        }
        if !finished {
            return Err(transport("WebSocket stream closed before response.completed", None));
        }
        Ok(())
    }
    .await;
    let keep = outcome.is_ok() && !options.cancel.is_cancelled();
    if acquired.cached.is_some() {
        let continuation = match (&outcome, parser.output.extra.get("responseId").and_then(Value::as_str)) {
            (Ok(()), Some(id)) if keep => {
                let input = input_items(&request.body);
                let answered = response_items(model, &parser.output);
                Some(Continuation {
                    last_rest: rest_digest(&request.body),
                    baseline_len: input.len() + answered.len(),
                    baseline: digest(input.iter().chain(&answered)),
                    last_response_id: id.to_owned(),
                })
            }
            _ => None,
        };
        *acquired.connection.continuation.lock() = continuation;
    }
    acquired.release(keep);
    outcome
}

/// `closeOpenAICodexWebSocketSessions`: one session's sockets, or every session's.
pub fn close_sessions(session: Option<&str>) {
    let mut cache = CACHE.lock();
    let drained: Vec<Arc<Connection>> = match session {
        Some(session) => cache.remove(session).into_iter().flat_map(|accounts| accounts.into_values()).collect(),
        None => cache.drain().flat_map(|(_, accounts)| accounts.into_values()).collect(),
    };
    for connection in drained {
        connection.request_close(1000, "debug_close");
    }
}

pub fn connection_count(session: &str, account: &str) -> usize {
    CACHE.lock().get(session).and_then(|accounts| accounts.get(account)).map_or(0, |_| 1)
}
