use factory_agent::providers::openai_codex::{Transport, resolve_codex_url, resolve_websocket_url, stream_simple_with};
use factory_agent::providers::user_agent;

use crate::support::fixtures::{context, event_json, message_json};
use crate::support::server::{HANG, Served, serve};
use factory_agent::stream::{EventStream, StreamOptions, ThinkingLevel};
use factory_agent::types::{StopReason, StreamEvent, same_json};
use serde_json::Value;
use tokio_util::sync::CancellationToken;

fn cases() -> Vec<Value> {
    serde_json::from_str(include_str!("../fixtures/openai_codex.json")).expect("fixture")
}

fn luna() -> factory_agent::types::Model {
    factory_agent::providers::catalog::models("openai-codex").into_iter().find(|model| model.id == "gpt-5.6-luna").expect("luna")
}

async fn collect(stream: &mut EventStream) -> Vec<StreamEvent> {
    let mut events = Vec::new();
    while let Some(event) = stream.next().await {
        events.push(event);
    }
    events
}

#[test]
fn urls_follow_pi() {
    assert_eq!(resolve_codex_url("https://chatgpt.com/backend-api"), "https://chatgpt.com/backend-api/codex/responses");
    assert_eq!(resolve_codex_url("https://x.test/backend-api/"), "https://x.test/backend-api/codex/responses");
    assert_eq!(resolve_codex_url("https://x.test/codex"), "https://x.test/codex/responses");
    assert_eq!(resolve_codex_url("https://x.test/codex/responses"), "https://x.test/codex/responses");
    assert_eq!(resolve_codex_url("  "), "https://chatgpt.com/backend-api/codex/responses");
    assert_eq!(resolve_websocket_url("https://chatgpt.com/backend-api"), "wss://chatgpt.com/backend-api/codex/responses");
    assert_eq!(resolve_websocket_url("http://127.0.0.1:9/backend-api"), "ws://127.0.0.1:9/backend-api/codex/responses");
}

#[tokio::test]
async fn every_recorded_sse_exchange_is_reproduced() {
    for case in cases() {
        let name = case["name"].as_str().expect("name");
        let status = case["http"]["status"].as_u64().expect("status") as u16;
        let chunks: Vec<String> =
            case["http"]["chunks"].as_array().expect("chunks").iter().filter_map(Value::as_str).map(str::to_owned).collect();
        let server = serve(vec![Served {
            status,
            content_type: if status == 200 { "text/event-stream" } else { "application/json" },
            chunks: chunks.clone(),
            hold: false,
            hold_before_headers: false,
        }])
        .await;
        let mut model = luna();
        model.base_url = format!("{}/backend-api", server.base_url);
        let options = StreamOptions {
            api_key: case["apiKey"].as_str().map(str::to_owned),
            session_id: Some("session-abc".into()),
            reasoning: case["reasoning"].as_str().and_then(ThinkingLevel::parse),
            ..StreamOptions::default()
        };
        let mut stream = match stream_simple_with(&model, context(&case), options, Transport::Sse) {
            Ok(stream) => stream,
            Err(error) => {
                assert_eq!(case["message"]["thrown"], error.to_string(), "{name}");
                continue;
            }
        };
        let events = collect(&mut stream).await;
        let (last, body) = events.split_last().expect("terminal event");
        assert_eq!(Value::Array(body.iter().map(event_json).collect()), case["events"], "{name}");
        let message = match last {
            StreamEvent::Done { message, .. } | StreamEvent::Error { error: message, .. } => message,
            other => panic!("{name}: {other:?}"),
        };
        let mut expected = case["message"].clone();
        if status != 200 && chunks.is_empty() {
            expected["errorMessage"] =
                Value::String(reqwest::StatusCode::from_u16(status).expect("status").canonical_reason().expect("reason").to_owned());
        }
        // Factory marks a usage limit (not Pi): its account is skipped until it resets.
        let limit = serde_json::from_str::<Value>(chunks.first().map_or("", String::as_str)).ok().and_then(|body| {
            (body["error"]["type"] == "usage_limit_reached")
                .then(|| serde_json::json!({"resetsAt": body["error"]["resets_at"].as_u64().map(|at| at * 1000)}))
        });
        if let Some(limit) = limit {
            expected["usageLimit"] = limit;
        }
        let actual = message_json(message);
        assert!(same_json(&actual, &expected), "{name}: {actual} != {expected}");
        if case["request"].is_null() {
            assert!(server.requests.lock().is_empty(), "{name}");
            continue;
        }
        let request = server.requests.lock().first().cloned().expect("request");
        assert_eq!((request.method.as_str(), request.path.as_str()), ("POST", "/backend-api/codex/responses"), "{name}");
        assert_eq!(request.body, case["request"]["body"], "{name}");
        let header = |name: &str| request.headers.iter().find(|(key, _)| key == name).map(|(_, value)| value.clone());
        for name in [
            "authorization",
            "chatgpt-account-id",
            "originator",
            "openai-beta",
            "accept",
            "content-type",
            "session-id",
            "x-client-request-id",
        ] {
            assert_eq!(header(name).as_deref(), case["request"]["headers"][name].as_str(), "{name}: header {name}");
        }
        assert_eq!(header("user-agent").as_deref(), Some(user_agent()), "{name}");
        assert_eq!(header("content-encoding"), None, "{name}");
    }
}

fn completed_case() -> Value {
    cases().into_iter().find(|case| case["http"]["status"] == 200 && case["message"]["stopReason"] == "stop").expect("a completed case")
}

/// Streams `served` from a server that holds the response open; `cancel`,
/// when given, is cancelled once the server has the request.
async fn held_stream(case: &Value, served: Served, cancel: Option<CancellationToken>) -> Vec<StreamEvent> {
    let server = serve(vec![served]).await;
    if let Some(cancel) = &cancel {
        server.cancel_once_requested(cancel.clone());
    }
    let mut model = luna();
    model.base_url = format!("{}/backend-api", server.base_url);
    let options = StreamOptions {
        api_key: case["apiKey"].as_str().map(str::to_owned),
        session_id: Some("session-abc".into()),
        cancel: cancel.unwrap_or_default(),
        ..StreamOptions::default()
    };
    let mut stream = stream_simple_with(&model, context(case), options, Transport::Sse).expect("stream");
    tokio::time::timeout(HANG, collect(&mut stream)).await.expect("the stream ends")
}

#[tokio::test]
async fn the_sse_stream_ends_at_the_terminal_event_while_the_body_stays_open() {
    let case = completed_case();
    let chunks = case["http"]["chunks"].as_array().expect("chunks").iter().filter_map(Value::as_str).map(str::to_owned).collect();
    let served = Served { status: 200, content_type: "text/event-stream", chunks, hold: true, hold_before_headers: false };
    let events = held_stream(&case, served, None).await;
    assert!(matches!(events.last(), Some(StreamEvent::Done { reason: StopReason::Stop, .. })), "{:?}", events.last());
}

#[tokio::test]
async fn cancelling_while_waiting_for_sse_headers_aborts_at_once() {
    let case = completed_case();
    let served = Served { status: 200, content_type: "text/event-stream", chunks: Vec::new(), hold: true, hold_before_headers: true };
    let events = held_stream(&case, served, Some(CancellationToken::new())).await;
    let Some(StreamEvent::Error { error, .. }) = events.last() else { panic!("{:?}", events.last()) };
    assert_eq!((error.stop_reason, error.error_message.as_deref()), (StopReason::Aborted, Some("Request was aborted")));
}
