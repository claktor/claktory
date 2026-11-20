use std::sync::Arc;
use std::time::Duration;

use factory_agent::errors::Error;
use factory_agent::guard::{ResponsePolicy, guard_responses, recoverable_response};
use factory_agent::stream::{LlmContext, StreamFn, StreamOptions, event_stream};
use factory_agent::types::{AssistantMessage, AssistantPart, Model, ModelCost, StopReason, StreamEvent, Usage};
use parking_lot::Mutex;
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

type Seen = Arc<Mutex<Option<(Option<u64>, Option<u32>, Option<String>)>>>;

fn model() -> Model {
    Model {
        id: "gpt-5.6-luna".into(),
        name: "GPT-5.6 Luna".into(),
        api: "openai-codex-responses".into(),
        provider: "openai-codex".into(),
        base_url: String::new(),
        reasoning: true,
        input: vec![],
        cost: ModelCost { input: 0.2, output: 1.2, cache_read: 0.02, cache_write: 0.25, tiers: None, extra: Default::default() },
        context_window: 272_000,
        max_tokens: 128_000,
        thinking_level_map: None,
        compat: None,
        headers: None,
        extra: Default::default(),
    }
}

fn reply(content: Vec<AssistantPart>) -> AssistantMessage {
    let tool = content.iter().any(|part| matches!(part, AssistantPart::ToolCall { .. }));
    AssistantMessage {
        content,
        api: "openai-codex-responses".into(),
        provider: "openai-codex".into(),
        model: "gpt-5.6-luna".into(),
        usage: Usage { input: 100, ..Usage::empty() },
        stop_reason: if tool { StopReason::ToolUse } else { StopReason::Stop },
        error_message: None,
        timestamp: 1,
        extra: Default::default(),
    }
}

fn fast() -> ResponsePolicy {
    ResponsePolicy { idle_ms: 100, retry_delay_ms: 1, ..ResponsePolicy::default() }
}

fn call(arguments: Value) -> AssistantPart {
    AssistantPart::ToolCall { id: "bad".into(), name: "bash".into(), arguments, thought_signature: None }
}

/// A provider that streams a tool call's JSON in the given fragments and records whether it was cancelled.
fn fragments(parts: Vec<String>, arguments: Value, cancelled: Arc<Mutex<bool>>) -> StreamFn {
    Arc::new(move |_model, _context, options| {
        let (sink, stream) = event_stream();
        let message = reply(vec![call(arguments.clone())]);
        let parts = parts.clone();
        let cancelled = cancelled.clone();
        tokio::spawn(async move {
            sink.push(StreamEvent::Start { partial: message.clone() });
            sink.push(StreamEvent::ToolcallStart { content_index: 0, partial: message.clone() });
            for delta in parts {
                sink.push(StreamEvent::ToolcallDelta { content_index: 0, delta, partial: message.clone() });
            }
            sink.push(StreamEvent::ToolcallEnd { content_index: 0, tool_call: message.content[0].clone(), partial: message.clone() });
            sink.push(StreamEvent::Done { reason: StopReason::ToolUse, message });
            options.cancel.cancelled().await;
            *cancelled.lock() = true;
        });
        Ok(stream)
    })
}

async fn guarded(stream: StreamFn, policy: ResponsePolicy, options: StreamOptions) -> Vec<StreamEvent> {
    let mut events = Vec::new();
    let mut source = guard_responses(stream, policy)(&model(), LlmContext::default(), options).expect("stream");
    while let Some(event) = source.next().await {
        events.push(event);
    }
    events
}

#[tokio::test]
async fn malformed_tool_json_never_reaches_a_done_event_and_cancels_the_provider() {
    let cases: [(&str, Vec<String>, &str); 3] = [
        (
            "whitespace runaway",
            vec!["{\"command\":\"touch bad\",".into(), " ".repeat(8192)],
            "Response guard: runaway whitespace in tool JSON",
        ),
        (
            "malformed recorded shape",
            vec!["{\"command\":\"echo test\",\"timeout\":120,\"} malformed".into(), "\n   ".into()],
            "Response guard: unescaped control character in tool JSON",
        ),
        ("salvaged truncated JSON", vec!["{\"command\":\"touch bad\"".into()], "Response guard: tool arguments are not complete JSON"),
    ];
    for (name, parts, reason) in cases {
        let cancelled = Arc::new(Mutex::new(false));
        let events = guarded(fragments(parts, json!({"command": "touch bad"}), cancelled.clone()), fast(), StreamOptions::default()).await;
        let last = events.last().unwrap();
        let StreamEvent::Error { reason: stop, error } = last else { panic!("{name}: {last:?}") };
        assert_eq!(*stop, StopReason::Error, "{name}");
        assert_eq!(error.error_message.as_deref(), Some(reason), "{name}");
        assert!(events.iter().any(|event| matches!(event, StreamEvent::ToolcallDelta { .. })), "{name}");
        assert!(!events.iter().any(|event| matches!(event, StreamEvent::Done { .. })), "{name}");
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert!(*cancelled.lock(), "{name}");
    }
    let cancelled = Arc::new(Mutex::new(false));
    let events =
        guarded(fragments(vec!["{\"command\":\"ls\"}".into()], json!({"command": "rm"}), cancelled), fast(), StreamOptions::default())
            .await;
    let StreamEvent::Error { error, .. } = events.last().unwrap() else { panic!("disagreement") };
    assert_eq!(error.error_message.as_deref(), Some("Response guard: tool arguments disagree with the streamed JSON object"));
}

#[tokio::test]
async fn a_call_without_arguments_may_stream_no_json_at_all() {
    for parts in [Vec::new(), vec![String::new()]] {
        let events = guarded(fragments(parts, json!({}), Arc::new(Mutex::new(false))), fast(), StreamOptions::default()).await;
        assert!(matches!(events.last(), Some(StreamEvent::Done { .. })), "{:?}", events.last());
    }
    // Arguments that did arrive must still be streamed.
    let events =
        guarded(fragments(Vec::new(), json!({"command": "ls"}), Arc::new(Mutex::new(false))), fast(), StreamOptions::default()).await;
    let Some(StreamEvent::Error { error, .. }) = events.last() else { panic!("{:?}", events.last()) };
    assert_eq!(error.error_message.as_deref(), Some("Response guard: tool arguments are not complete JSON"));
}

#[tokio::test]
async fn large_valid_escaped_file_contents_are_not_mistaken_for_runaway_formatting() {
    let arguments = json!({"path": "large.txt", "content": format!("quoted \" text\\\n{}", " ".repeat(20_000))});
    let text = serde_json::to_string(&arguments).unwrap();
    let parts: Vec<String> = text.chars().map(String::from).collect();
    let events =
        guarded(fragments(parts, arguments, Arc::new(Mutex::new(false))), ResponsePolicy::default(), StreamOptions::default()).await;
    let StreamEvent::Done { message, .. } = events.last().unwrap() else { panic!("done") };
    assert_eq!(message.stop_reason, StopReason::ToolUse);
}

#[tokio::test]
async fn deadlines_interrupts_and_silent_ends_stop_even_an_uncooperative_provider() {
    let idle: StreamFn = Arc::new(|_model, _context, options| {
        let (sink, stream) = event_stream();
        tokio::spawn(async move {
            options.cancel.cancelled().await;
            drop(sink);
        });
        Ok(stream)
    });
    let events = guarded(idle.clone(), fast(), StreamOptions::default()).await;
    let StreamEvent::Error { error, .. } = events.last().unwrap() else { panic!("idle") };
    assert_eq!(error.error_message.as_deref(), Some("Response guard: stream idle deadline exceeded"));
    assert_eq!(error.stop_reason, StopReason::Error);
    let token = CancellationToken::new();
    let cancel = token.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(10)).await;
        cancel.cancel();
    });
    let events = guarded(idle.clone(), ResponsePolicy::default(), StreamOptions { cancel: token, ..StreamOptions::default() }).await;
    let StreamEvent::Error { reason, error } = events.last().unwrap() else { panic!("interrupt") };
    assert_eq!(*reason, StopReason::Aborted);
    assert_eq!(error.stop_reason, StopReason::Aborted);
    assert_eq!(error.error_message.as_deref(), Some("Invocation interrupted during response"));
    let already = CancellationToken::new();
    already.cancel();
    let events = guarded(idle, ResponsePolicy::default(), StreamOptions { cancel: already, ..StreamOptions::default() }).await;
    assert_eq!(events.len(), 1);
    let silent: StreamFn = Arc::new(|_model, _context, _options| {
        let (sink, stream) = event_stream();
        sink.push(StreamEvent::Start { partial: reply(vec![]) });
        drop(sink);
        Ok(stream)
    });
    let events = guarded(silent, ResponsePolicy::default(), StreamOptions::default()).await;
    let StreamEvent::Error { error, .. } = events.last().unwrap() else { panic!("silent") };
    assert_eq!(error.error_message.as_deref(), Some("Response guard: stream ended without a terminal response"));
    let failing: StreamFn = Arc::new(|_model, _context, _options| Err(Error::Other("no api key".into())));
    let events = guarded(failing, ResponsePolicy::default(), StreamOptions::default()).await;
    let StreamEvent::Error { error, .. } = events.last().unwrap() else { panic!("failing") };
    assert_eq!(error.error_message.as_deref(), Some("no api key"));
}

#[tokio::test]
async fn the_inner_request_gets_the_guards_timeout_and_no_retries() {
    let seen: Seen = Arc::new(Mutex::new(None));
    let record = seen.clone();
    let probe: StreamFn = Arc::new(move |_model, _context, options| {
        *record.lock() = Some((options.timeout_ms, options.max_retries, options.session_id.clone()));
        let (sink, stream) = event_stream();
        sink.push(StreamEvent::Done { reason: StopReason::Stop, message: reply(vec![]) });
        Ok(stream)
    });
    let options = StreamOptions { session_id: Some("session".into()), ..StreamOptions::default() };
    let events = guarded(probe, fast(), options).await;
    assert!(matches!(events.last(), Some(StreamEvent::Done { .. })));
    assert_eq!(*seen.lock(), Some((Some(100), Some(0), Some("session".into()))));
}

#[test]
fn the_recovery_predicate_recognizes_transport_failures_and_keeps_terminal_errors() {
    for reason in [
        "WebSocket error",
        "The operation was aborted",
        "429 temporarily rate-limited upstream",
        "Provider finish_reason: error",
        "Upstream idle timeout exceeded",
        "WebSocket idle timeout after 120000ms",
        "Response guard: stream idle deadline exceeded",
        "fetch failed",
        "HTTP 503 Service Unavailable",
        "error decoding response body",
        "error reading a body from connection",
        "Connection error.",
        "Request timed out.",
        "Upstream error from Sail Research: the stream ended unexpectedly before the completion finished",
    ] {
        assert!(recoverable_response(reason), "{reason}");
    }
    for reason in [
        "429 insufficient_quota",
        "401 unauthorized",
        "Provider finish_reason: content_filter",
        "Invalid API key",
        "plain failure",
        "Usage limit reached",
        "Connection error: insufficient quota",
        "error decoding response body: invalid API key",
    ] {
        assert!(!recoverable_response(reason), "{reason}");
    }
}
