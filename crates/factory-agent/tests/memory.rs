//! What a model call holds in memory beyond the conversation itself (#454):
//! measured with a counting allocator, so a file of its own (the allocator
//! is the process's), its tests one at a time (`one_at_a_time`). Each bound
//! sits between what the code held before #454 and after, so a copy of the
//! conversation creeping back fails it; `--nocapture` prints the numbers.

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use base64::Engine;
use factory_agent::agent::{Agent, AgentOptions};
use factory_agent::providers::anthropic::files::{ImageFiles, ImageFilesFor};
use factory_agent::providers::catalog::models;
use factory_agent::stream::{LlmContext, StreamFn, StreamOptions, ThinkingLevel, pending_message};
use factory_agent::types::{AssistantMessage, AssistantPart, Message, Model, StopReason, ToolResultMessage, ToolResultPart, UserMessage};
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

struct Counting;

static LIVE: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);
static TOTAL: AtomicUsize = AtomicUsize::new(0);

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let pointer = unsafe { System.alloc(layout) };
        if !pointer.is_null() {
            TOTAL.fetch_add(layout.size(), Ordering::Relaxed);
            let live = LIVE.fetch_add(layout.size(), Ordering::Relaxed) + layout.size();
            PEAK.fetch_max(live, Ordering::Relaxed);
        }
        pointer
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        unsafe { System.dealloc(pointer, layout) };
        LIVE.fetch_sub(layout.size(), Ordering::Relaxed);
    }

    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        let moved = unsafe { System.realloc(pointer, layout, size) };
        if !moved.is_null() {
            if size > layout.size() {
                TOTAL.fetch_add(size - layout.size(), Ordering::Relaxed);
                let live = LIVE.fetch_add(size - layout.size(), Ordering::Relaxed) + size - layout.size();
                PEAK.fetch_max(live, Ordering::Relaxed);
            } else {
                LIVE.fetch_sub(layout.size() - size, Ordering::Relaxed);
            }
        }
        moved
    }
}

#[global_allocator]
static ALLOCATOR: Counting = Counting;

const MB: f64 = 1024.0 * 1024.0;

fn live() -> usize {
    LIVE.load(Ordering::Relaxed)
}

fn reset_peak() {
    PEAK.store(live(), Ordering::Relaxed);
}

fn peak() -> usize {
    PEAK.load(Ordering::Relaxed)
}

/// Answers Anthropic like the real one, reading each request's body and
/// keeping only its length, so what the test measures is the client's: a
/// file uploaded, and a message's answer started, then held.
struct Server {
    base_url: String,
    asked: Arc<AtomicUsize>,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for Server {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn serve() -> Server {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let base_url = format!("http://{}", listener.local_addr().expect("addr"));
    let asked = Arc::new(AtomicUsize::new(0));
    let body_bytes = asked.clone();
    let task = tokio::spawn(async move {
        loop {
            let Ok((mut socket, _)) = listener.accept().await else { return };
            let body_bytes = body_bytes.clone();
            tokio::spawn(async move {
                let mut head = Vec::new();
                let mut byte = [0u8; 1];
                while !head.ends_with(b"\r\n\r\n") {
                    if socket.read(&mut byte).await.unwrap_or(0) == 0 {
                        return;
                    }
                    head.push(byte[0]);
                }
                let head = String::from_utf8_lossy(&head).to_ascii_lowercase();
                let length: usize = head
                    .lines()
                    .find_map(|line| line.strip_prefix("content-length:"))
                    .and_then(|value| value.trim().parse().ok())
                    .unwrap_or(0);
                let mut left = length;
                let mut buffer = [0u8; 16 * 1024];
                while left > 0 {
                    let read = socket.read(&mut buffer[..left.min(16 * 1024)]).await.unwrap_or(0);
                    if read == 0 {
                        return;
                    }
                    left -= read;
                }
                if head.starts_with("post /v1/files") {
                    let body = json!({"type": "file", "id": format!("file_{length}"), "size_bytes": 4, "mime_type": "image/png", "downloadable": false}).to_string();
                    let answer = format!(
                        "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                        body.len()
                    );
                    let _ = socket.write_all(answer.as_bytes()).await;
                    return;
                }
                let events = [
                    json!({"type": "message_start", "message": {"id": "msg_1", "model": "claude-sonnet-4-5", "usage": {"input_tokens": 3, "output_tokens": 1}}}),
                    json!({"type": "content_block_start", "index": 0, "content_block": {"type": "text", "text": ""}}),
                    json!({"type": "content_block_delta", "index": 0, "delta": {"type": "text_delta", "text": "Working"}}),
                ];
                let mut answer = String::from(
                    "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ntransfer-encoding: chunked\r\nconnection: close\r\n\r\n",
                );
                for event in events {
                    let chunk = format!("event: {}\ndata: {event}\n\n", event["type"].as_str().expect("type"));
                    answer.push_str(&format!("{:x}\r\n{chunk}\r\n", chunk.len()));
                }
                let _ = socket.write_all(answer.as_bytes()).await;
                body_bytes.store(length, Ordering::Relaxed);
                std::future::pending::<()>().await;
            });
        }
    });
    Server { base_url, asked, task }
}

fn sonnet(server: &Server) -> Model {
    let mut model = models("anthropic").into_iter().find(|model| model.id == "claude-sonnet-4-5").expect("sonnet");
    model.base_url = server.base_url.clone();
    model
}

const TEXT_RESULTS: usize = 60;
const TEXT_RESULT_BYTES: usize = 30_000;
const IMAGES: usize = 20;
const IMAGE_BYTES: usize = 400_000;

/// A long working conversation: bash outputs and screenshots, as a QA
/// agent's reads after an hour (60 × 30 KB of text, 20 × 400 KB images).
fn conversation(model: &Model) -> Vec<Message> {
    let mut messages = vec![Message::User(UserMessage::text("Check the app", 1))];
    let mut call = |index: usize, name: &str, content: Vec<ToolResultPart>| {
        let id = format!("toolu_{index}");
        let part =
            AssistantPart::ToolCall { id: id.clone(), name: name.into(), arguments: json!({"command": "ls"}), thought_signature: None };
        messages.push(Message::Assistant(AssistantMessage {
            content: vec![AssistantPart::Text { text: "Next I'll look at it.".into(), text_signature: None }, part],
            stop_reason: StopReason::ToolUse,
            ..pending_message(model)
        }));
        messages.push(Message::ToolResult(ToolResultMessage {
            tool_call_id: id,
            tool_name: name.into(),
            content,
            details: Value::Null,
            usage: None,
            is_error: false,
            timestamp: 2,
            started: None,
        }));
    };
    for index in 0..TEXT_RESULTS {
        let text: String = (0..TEXT_RESULT_BYTES).map(|n| (b'a' + ((n * 7 + index) % 26) as u8) as char).collect();
        call(index, "bash", vec![ToolResultPart::Text { text }]);
    }
    for index in 0..IMAGES {
        let bytes: Vec<u8> = (0..IMAGE_BYTES * 3 / 4).map(|n| (n * 31 + index) as u8).collect();
        let image = ToolResultPart::Image {
            data: base64::engine::general_purpose::STANDARD.encode(&bytes).into(),
            mime_type: "image/png".into(),
            digest: Some(format!("digest{index}")),
        };
        call(TEXT_RESULTS + index, "read_image", vec![ToolResultPart::Text { text: "shot.png".into() }, image]);
    }
    messages
}

/// Until the message's answer has started; the request body's length.
async fn until_asked(server: &Server) -> usize {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(60);
    while server.asked.load(Ordering::Relaxed) == 0 {
        assert!(tokio::time::Instant::now() < deadline, "no request");
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    tokio::time::sleep(Duration::from_millis(300)).await;
    server.asked.load(Ordering::Relaxed)
}

/// The counters are the process's: one measurement at a time.
async fn one_at_a_time() -> tokio::sync::MutexGuard<'static, ()> {
    static ONE: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
    ONE.lock().await
}

/// Prints what a call held, and checks it against its bounds, as multiples
/// of the conversation.
fn report(what: &str, conversation: usize, extra: usize, peak: usize, bounds: (f64, f64)) {
    eprintln!(
        "{what}: conversation {:.1} MB; held while streaming {:.1} MB ({:.2}×); peak {:.1} MB ({:.2}×)",
        conversation as f64 / MB,
        extra as f64 / MB,
        extra as f64 / conversation as f64,
        peak as f64 / MB,
        peak as f64 / conversation as f64,
    );
    assert!((extra as f64) < bounds.0 * conversation as f64, "{what}: held {extra} bytes while streaming");
    assert!((peak as f64) < bounds.1 * conversation as f64, "{what}: peaked {peak} bytes over its conversation");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn what_a_model_call_holds_beyond_its_conversation() {
    let _one = one_at_a_time().await;
    let files = Arc::new(ImageFiles::new(None));
    let options = StreamOptions {
        api_key: Some("test-key".into()),
        image_files: Some(ImageFilesFor { files: files.clone(), account: "anthropic/1".into() }),
        ..StreamOptions::default()
    };

    // The provider alone: the request built from a context it owns.
    let server = serve().await;
    let model = sonnet(&server);
    let before = live();
    let messages = conversation(&model);
    let size = live() - before;
    let context = LlmContext { system_prompt: Some("You are a coding agent.".into()), messages, tools: vec![] };
    let base = live();
    reset_peak();
    let stream = factory_agent::providers::anthropic::stream_simple(&model, context, options.clone()).expect("stream");
    let body = until_asked(&server).await;
    // Before #454: 0.01× while streaming, 1.21× at its peak.
    report("provider", size, live().saturating_sub(base), peak() - base, (0.5, 0.8));
    eprintln!("provider: request body {:.1} MB", body as f64 / MB);
    options.cancel.cancel();
    drop(stream);
    drop(server);
    // The call's task lets go of its context as it ends.
    tokio::time::sleep(Duration::from_millis(300)).await;

    // An agent's call: its conversation, the run's, and the request's.
    let server = serve().await;
    let model = sonnet(&server);
    let files_for = options.image_files.clone();
    let provider: StreamFn = Arc::new(move |model, context, mut options| {
        options.api_key = Some("test-key".into());
        options.image_files = files_for.clone();
        factory_agent::providers::anthropic::stream_simple(model, context, options)
    });
    let before = live();
    let messages = conversation(&model);
    let size = live() - before;
    let mut agent = Agent::new(AgentOptions {
        system_prompt: "You are a coding agent.".into(),
        model: model.clone(),
        thinking_level: ThinkingLevel::Off,
        tools: Vec::new(),
        messages,
        stream: provider,
        session_id: None,
        transform_context: None,
        should_stop_after_turn: None,
        before_tool_call: None,
        compact_at: None,
        inbox: None,
        after_compaction: None,
        secrets: None,
        fallback_model: None,
    });
    let abort = agent.abort_handle();
    let base = live();
    reset_peak();
    let run = tokio::spawn(async move {
        let _ = agent.prompt("Go on").await;
        agent
    });
    until_asked(&server).await;
    // Before: 2.01× and 3.21×; the run's and the request's copies of the
    // conversation share its images now.
    report("agent", size, live().saturating_sub(base), peak() - base, (1.0, 1.6));
    abort.abort();
    drop(run.await.expect("run"));
    drop(server);
}

/// Answers Codex's WebSocket like the real one, keeping only each frame's
/// length: the first request answered whole, the next ones started, then held.
async fn serve_codex() -> Server {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let base_url = format!("http://{}", listener.local_addr().expect("addr"));
    let asked = Arc::new(AtomicUsize::new(0));
    let frame_bytes = asked.clone();
    let task = tokio::spawn(async move {
        loop {
            let Ok((stream, _)) = listener.accept().await else { return };
            let frame_bytes = frame_bytes.clone();
            tokio::spawn(async move {
                let Ok(mut socket) = tokio_tungstenite::accept_async(stream).await else { return };
                let mut frames = 0;
                while let Some(Ok(message)) = socket.next().await {
                    let tokio_tungstenite::tungstenite::Message::Text(text) = message else { continue };
                    frames += 1;
                    let id = format!("resp_{frames}");
                    let mut events = vec![
                        json!({"type": "response.created", "response": {"id": id, "status": "in_progress"}}),
                        json!({"type": "response.output_item.added", "output_index": 0, "item": {"type": "message", "id": format!("msg_{id}"), "role": "assistant", "status": "in_progress", "content": []}}),
                        json!({"type": "response.output_text.delta", "output_index": 0, "delta": "Seen."}),
                    ];
                    if frames == 1 {
                        events.push(json!({"type": "response.output_item.done", "output_index": 0, "item": {"type": "message", "id": format!("msg_{id}"), "role": "assistant", "status": "completed", "content": [{"type": "output_text", "text": "Seen.", "annotations": []}]}}));
                        events.push(json!({"type": "response.completed", "response": {"id": id, "status": "completed", "usage": {"input_tokens": 10, "output_tokens": 2, "total_tokens": 12}}}));
                    }
                    for event in events {
                        if socket.send(tokio_tungstenite::tungstenite::Message::text(event.to_string())).await.is_err() {
                            return;
                        }
                    }
                    frame_bytes.store(text.len(), Ordering::Relaxed);
                }
            });
        }
    });
    Server { base_url, asked, task }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn what_a_codex_call_holds_beyond_its_conversation() {
    let _one = one_at_a_time().await;
    let fixture: Value = serde_json::from_str(include_str!("fixtures/openai_codex_ws.json")).expect("fixture");
    let server = serve_codex().await;
    let mut model = models("openai-codex").into_iter().find(|model| model.id == "gpt-5.6-luna").expect("luna");
    model.base_url = server.base_url.clone();
    let options = StreamOptions {
        api_key: Some(fixture["token"].as_str().expect("token").to_owned()),
        session_id: Some("memory".into()),
        ..StreamOptions::default()
    };
    let before = live();
    let mut messages = conversation(&model);
    let size = live() - before;
    let context = |messages: &Vec<Message>| LlmContext {
        system_prompt: Some("You are a coding agent.".into()),
        messages: messages.clone(),
        tools: vec![],
    };

    // The first call, answered whole: what the socket keeps for the next.
    let base = live();
    reset_peak();
    let first = factory_agent::providers::openai_codex::stream_simple(&model, context(&messages), options.clone())
        .expect("stream")
        .result()
        .await
        .expect("an answer");
    assert_eq!(first.stop_reason, StopReason::Stop, "{:?}", first.error_message);
    let first_peak = peak() - base;
    tokio::time::sleep(Duration::from_millis(300)).await;
    let kept = live().saturating_sub(base);
    // Before: 3.0× kept (the whole request, beside the socket's buffers,
    // the server's included) and 6.78× at its peak.
    report("codex, first call", size, kept, first_peak, (2.5, 5.0));
    eprintln!(
        "codex: first frame {:.1} MB; kept by the socket after it {:.1} MB",
        server.asked.load(Ordering::Relaxed) as f64 / MB,
        kept as f64 / MB
    );

    // The next: a delta on the same socket.
    messages.push(Message::Assistant(first));
    messages.push(Message::User(UserMessage::text("And now?", 3)));
    server.asked.store(0, Ordering::Relaxed);
    let base = live();
    reset_peak();
    let stream = factory_agent::providers::openai_codex::stream_simple(&model, context(&messages), options.clone()).expect("stream");
    let frame = until_asked(&server).await;
    // Before: 2.77× and 4.78×.
    report("codex, next call", size, live().saturating_sub(base), peak() - base, (2.0, 3.0));
    eprintln!("codex: next frame {:.1} KB", frame as f64 / 1024.0);
    options.cancel.cancel();
    drop(stream);
    factory_agent::providers::openai_codex::websocket::close_sessions(Some("memory"));
}

/// A tool call written in `PIECES` pieces of `PIECE` bytes, each event with
/// the message so far, as a provider streams it.
const PIECES: usize = 1000;
const PIECE: usize = 100;

fn tool_call_stream(model: &Model) -> Vec<factory_agent::types::StreamEvent> {
    use factory_agent::types::StreamEvent;
    let call =
        |arguments: Value| AssistantPart::ToolCall { id: "toolu_1".into(), name: "write_file".into(), arguments, thought_signature: None };
    let mut partial = AssistantMessage { content: vec![call(json!({}))], stop_reason: StopReason::ToolUse, ..pending_message(model) };
    let mut events =
        vec![StreamEvent::Start { partial: partial.clone() }, StreamEvent::ToolcallStart { content_index: 0, partial: partial.clone() }];
    let mut written = String::new();
    for _ in 0..PIECES {
        let delta = "x".repeat(PIECE);
        written.push_str(&delta);
        partial.content[0] = call(json!({"path": "a.txt", "content": written}));
        events.push(StreamEvent::ToolcallDelta { content_index: 0, delta, partial: partial.clone() });
    }
    // Stops here: the test needs no tool to run.
    partial.content = vec![AssistantPart::Text { text: "Wrote it.".into(), text_signature: None }];
    partial.stop_reason = StopReason::Stop;
    events.push(StreamEvent::Done { reason: StopReason::Stop, message: partial });
    events
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn what_streaming_a_long_tool_call_copies() {
    let _one = one_at_a_time().await;
    let model = models("anthropic").into_iter().find(|model| model.id == "claude-sonnet-4-5").expect("sonnet");
    let events = Arc::new(parking_lot::Mutex::new(Some(tool_call_stream(&model))));
    let provider: StreamFn = Arc::new(move |_model, _context, _options| {
        let (sink, stream) = factory_agent::stream::event_stream();
        for event in events.lock().take().unwrap_or_default() {
            sink.push(event);
        }
        Ok(stream)
    });
    let mut agent = Agent::new(AgentOptions {
        system_prompt: "You are a coding agent.".into(),
        model: model.clone(),
        thinking_level: ThinkingLevel::Off,
        tools: Vec::new(),
        messages: Vec::new(),
        stream: provider,
        session_id: None,
        transform_context: None,
        should_stop_after_turn: None,
        before_tool_call: None,
        compact_at: None,
        inbox: None,
        after_compaction: None,
        secrets: None,
        fallback_model: None,
    });
    // One listener, as factoryd's runner.
    agent.subscribe(Arc::new(|_event| Box::pin(async { Ok(()) })));
    let total = TOTAL.load(Ordering::Relaxed);
    agent.prompt("Write it").await.expect("run");
    let copied = TOTAL.load(Ordering::Relaxed) - total;
    let message = PIECES * PIECE;
    eprintln!(
        "streaming a {} KB tool call in {PIECES} pieces: the agent allocated {:.1} MB ({:.0}× the call)",
        message / 1024,
        copied as f64 / MB,
        copied as f64 / message as f64
    );
    // Before #454: 3084×, each piece copied into the run's context and
    // for each listener besides its update; 1560× after.
    assert!(copied < 2200 * message, "copied {copied} bytes");
}
