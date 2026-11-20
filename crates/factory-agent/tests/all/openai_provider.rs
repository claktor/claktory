use crate::support::fixtures::{context, event_json, message_json, model, openai_responses};
use crate::support::server::{HANG, Served, serve};
use factory_agent::providers::openai::stream_simple;
use factory_agent::providers::{clamp_max_tokens, clamp_thinking_level, supported_thinking_levels, user_agent};
use factory_agent::stream::{EventStream, LlmContext, StreamOptions, ThinkingLevel};
use factory_agent::types::{StopReason, StreamEvent, same_json};
use serde_json::Value;
use tokio_util::sync::CancellationToken;

fn served(case: &Value, hold: bool) -> Served {
    let status = case["http"]["status"].as_u64().expect("status") as u16;
    let content_type = if status == 200 {
        "text/event-stream"
    } else if case["http"]["json"] == true {
        "application/json"
    } else {
        "text/plain"
    };
    let chunks = case["http"]["chunks"].as_array().expect("chunks").iter().filter_map(Value::as_str).map(str::to_owned).collect();
    Served { status, content_type, chunks, hold, hold_before_headers: false }
}

fn options(case: &Value) -> StreamOptions {
    StreamOptions {
        api_key: case["apiKey"].as_str().map(str::to_owned),
        session_id: Some("session-abc".into()),
        reasoning: case["reasoning"].as_str().and_then(ThinkingLevel::parse),
        ..StreamOptions::default()
    }
}

async fn collect(stream: &mut EventStream) -> Vec<StreamEvent> {
    let mut events = Vec::new();
    while let Some(event) = stream.next().await {
        events.push(event);
    }
    events
}

#[tokio::test]
async fn every_recorded_exchange_is_reproduced() {
    for case in openai_responses() {
        let name = case["name"].as_str().expect("name");
        let server = serve(vec![served(&case, false)]).await;
        let mut model = model(&case);
        model.base_url = format!("{}/v1", server.base_url);
        let mut stream = match stream_simple(&model, context(&case), options(&case)) {
            Ok(stream) => stream,
            Err(error) => {
                assert_eq!(case["message"]["thrown"], error.to_string(), "{name}");
                continue;
            }
        };
        let events = collect(&mut stream).await;
        let (last, body) = events.split_last().expect("terminal event");
        let recorded: Vec<Value> = body.iter().map(event_json).collect();
        assert_eq!(Value::Array(recorded), case["events"], "{name}");
        let message = match last {
            StreamEvent::Done { message, .. } | StreamEvent::Error { error: message, .. } => message,
            other => panic!("{name}: {other:?}"),
        };
        let actual = message_json(message);
        assert!(same_json(&actual, &case["message"]), "{name}: {actual} != {}", case["message"]);
        if case["request"].is_null() {
            assert!(server.requests.lock().is_empty(), "{name}");
            continue;
        }
        let request = server.requests.lock().first().cloned().expect("request");
        assert_eq!((request.method.as_str(), request.path.as_str()), ("POST", "/v1/responses"), "{name}");
        assert_eq!(request.body, case["request"]["body"], "{name}");
        let header = |name: &str| request.headers.iter().find(|(key, _)| key == name).map(|(_, value)| value.clone());
        assert_eq!(header("authorization").as_deref(), case["request"]["headers"]["authorization"].as_str(), "{name}");
        assert_eq!(header("content-type").as_deref(), Some("application/json"), "{name}");
        assert_eq!(header("accept").as_deref(), Some("application/json"), "{name}");
        assert_eq!(header("session_id").as_deref(), Some("session-abc"), "{name}");
        assert_eq!(header("x-client-request-id").as_deref(), Some("session-abc"), "{name}");
        assert_eq!(header("user-agent").as_deref(), Some(user_agent()), "{name}");
    }
}

/// Pi's texts: an abort during `send` is `Request aborted`; one while reading ends the body silently, so the terminal event decides the text.
#[tokio::test]
async fn cancellation_aborts_the_request_and_the_body() {
    let cases = openai_responses();
    let case = cases.iter().find(|case| case["name"] == "non-reasoning model without tools").expect("case");
    for (before_headers, chunks, expected) in [
        (true, 0, "Request aborted"),
        (false, 0, "OpenAI Responses stream ended before a terminal response event"),
        (false, 2, "Request was aborted"),
    ] {
        let mut served = served(case, true);
        served.hold_before_headers = before_headers;
        served.chunks.truncate(chunks);
        let server = serve(vec![served]).await;
        let mut model = model(case);
        model.base_url = format!("{}/v1", server.base_url);
        let cancel = CancellationToken::new();
        let mut stream = stream_simple(&model, context(case), StreamOptions { cancel: cancel.clone(), ..options(case) }).expect("stream");
        // Before headers: once the request is in. During the body: once the
        // client has read all the server sends, the headers (Start) or the
        // whole reply (its text ends), whose body then stays open.
        if before_headers {
            server.cancel_once_requested(cancel.clone());
        }
        let last = if chunks == 0 { "start" } else { "text_end" };
        let read = async {
            let mut events = Vec::new();
            while let Some(event) = stream.next().await {
                if event_json(&event)["type"] == last {
                    cancel.cancel();
                }
                events.push(event);
            }
            events
        };
        let events = tokio::time::timeout(HANG, read).await.expect("the provider task ends");
        let Some(StreamEvent::Error { reason, error }) = events.last() else { panic!("{events:?}") };
        assert_eq!(*reason, StopReason::Aborted, "{before_headers} {chunks}");
        assert_eq!(error.stop_reason, StopReason::Aborted, "{before_headers} {chunks}");
        assert_eq!(error.error_message.as_deref(), Some(expected), "{before_headers} {chunks}");
        assert_eq!(events.iter().filter(|event| matches!(event, StreamEvent::Start { .. })).count(), usize::from(!before_headers));
    }
}

#[tokio::test]
async fn a_refused_connection_is_a_connection_error() {
    let cases = openai_responses();
    let case = cases.iter().find(|case| case["name"] == "non-reasoning model without tools").expect("case");
    let closed = serve(Vec::new()).await;
    let base_url = closed.base_url.clone();
    drop(closed);
    let mut model = model(case);
    model.base_url = format!("{base_url}/v1");
    let mut stream = stream_simple(&model, context(case), options(case)).expect("stream");
    let events = collect(&mut stream).await;
    let Some(StreamEvent::Error { error, .. }) = events.last() else { panic!("{events:?}") };
    assert_eq!(error.error_message.as_deref(), Some("Connection error."));
    assert_eq!(error.stop_reason, StopReason::Error);
}

#[test]
fn thinking_levels_clamp_through_the_model_map() {
    let gpt = model(&openai_responses()[0]);
    assert_eq!(
        supported_thinking_levels(&gpt),
        [ThinkingLevel::Off, ThinkingLevel::Low, ThinkingLevel::Medium, ThinkingLevel::High, ThinkingLevel::Xhigh]
    );
    assert_eq!(clamp_thinking_level(&gpt, ThinkingLevel::Max), ThinkingLevel::Xhigh);
    assert_eq!(clamp_thinking_level(&gpt, ThinkingLevel::Minimal), ThinkingLevel::Low);
    let mut plain = gpt.clone();
    plain.reasoning = false;
    assert_eq!(supported_thinking_levels(&plain), [ThinkingLevel::Off]);
    assert_eq!(clamp_thinking_level(&plain, ThinkingLevel::High), ThinkingLevel::Off);
    let mut no_xhigh = gpt.clone();
    no_xhigh.thinking_level_map = None;
    assert_eq!(clamp_thinking_level(&no_xhigh, ThinkingLevel::Xhigh), ThinkingLevel::High);
    assert_eq!(clamp_thinking_level(&no_xhigh, ThinkingLevel::Max), ThinkingLevel::High);
}

#[test]
fn max_tokens_leave_room_in_the_context_window() {
    let mut gpt = model(&openai_responses()[0]);
    let context = LlmContext { system_prompt: Some("x".repeat(40_000)), messages: Vec::new(), tools: Vec::new() };
    assert_eq!(clamp_max_tokens(&gpt, &context, gpt.max_tokens), 128_000);
    gpt.context_window = 20_000;
    assert_eq!(clamp_max_tokens(&gpt, &context, gpt.max_tokens), 5_904);
    gpt.context_window = 4_000;
    assert_eq!(clamp_max_tokens(&gpt, &context, gpt.max_tokens), 1);
    gpt.context_window = 0;
    assert_eq!(clamp_max_tokens(&gpt, &context, 0), 1);
}
