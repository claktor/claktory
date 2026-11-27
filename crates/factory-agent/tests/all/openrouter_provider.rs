use std::sync::Arc;
use std::time::Duration;

use crate::support::fixtures::{context, fixture_model, fixture_options, replay};
use crate::support::server::{HANG, Served, serve};
use factory_agent::providers::openrouter::stream_simple;
use factory_agent::stream::StreamFn;
use factory_agent::types::{StopReason, StreamEvent};
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

fn cases() -> Vec<Value> {
    serde_json::from_str(include_str!("../fixtures/openrouter.json")).expect("fixture")
}

fn stream() -> StreamFn {
    Arc::new(stream_simple)
}

#[tokio::test]
async fn every_recorded_exchange_is_reproduced() {
    for case in cases() {
        replay(&case, "/api/v1", &["authorization", "content-type", "accept", "user-agent"], stream(), "openrouter").await;
    }
}

fn sse(chunks: &[Value]) -> Vec<String> {
    chunks.iter().map(|chunk| format!("data: {chunk}\n\n")).collect()
}

#[tokio::test]
async fn cancellation_before_headers_and_during_the_body_use_pis_texts() {
    let all = cases();
    let case = all.iter().find(|case| case["name"] == "thinking off without routing").expect("case");
    let partial = sse(&[json!({"id": "gen-9", "choices": [{"index": 0, "delta": {"content": "Hel"}, "finish_reason": null}]})]);
    for (before_headers, chunks, expected) in [(true, Vec::new(), "Request aborted"), (false, partial, "Request was aborted")] {
        let server =
            serve(vec![Served { status: 200, content_type: "text/event-stream", chunks, hold: true, hold_before_headers: before_headers }])
                .await;
        let mut model = fixture_model("openrouter", case);
        model.base_url = format!("{}/api/v1", server.base_url);
        let cancel = CancellationToken::new();
        let options = factory_agent::stream::StreamOptions { cancel: cancel.clone(), ..fixture_options(case) };
        let mut events = stream_simple(&model, context(case), options).expect("stream");
        // Before headers: once the request is in. During the body: once the
        // client has read "Hel", the last thing the server sends.
        if before_headers {
            server.cancel_once_requested(cancel.clone());
        }
        let mut seen = Vec::new();
        while let Some(event) = tokio::time::timeout(HANG, events.next()).await.expect("the provider task ends") {
            if matches!(event, StreamEvent::TextDelta { .. }) {
                cancel.cancel();
            }
            seen.push(event);
        }
        let Some(StreamEvent::Error { reason, error }) = seen.last() else { panic!("{seen:?}") };
        assert_eq!((*reason, error.error_message.as_deref()), (StopReason::Aborted, Some(expected)), "{before_headers}");
        if !before_headers {
            assert!(matches!(seen[seen.len() - 2], StreamEvent::TextEnd { ref content, .. } if content == "Hel"), "{seen:?}");
        }
    }
}

#[tokio::test]
async fn tool_calls_split_across_chunks_by_index_and_id_become_one_call() {
    let all = cases();
    let case = all.iter().find(|case| case["name"] == "thinking off without routing").expect("case");
    let chunks = sse(&[
        json!({"id": "gen-7", "choices": [{"index": 0, "delta": {"tool_calls": [{"index": 0, "id": "call_a", "function": {"name": "bash"}}]}}]}),
        json!({"id": "gen-7", "choices": [{"index": 0, "delta": {"tool_calls": [{"index": 0, "function": {"arguments": "{\"command\""}}]}}]}),
        json!({"id": "gen-7", "choices": [{"index": 0, "delta": {"tool_calls": [{"id": "call_a", "function": {"arguments": ":\"ls\"}"}}]}}]}),
        json!({"id": "gen-7", "choices": [{"index": 0, "delta": {"tool_calls": [{"index": 1, "id": "call_b", "type": "function", "function": {"name": "read_file", "arguments": "{\"path\":\"a\"}"}}]}}]}),
        json!({"id": "gen-7", "choices": [{"index": 0, "delta": {}, "finish_reason": "tool_calls"}], "usage": {"prompt_tokens": 10, "completion_tokens": 5}}),
    ]);
    let server =
        serve(vec![Served { status: 200, content_type: "text/event-stream", chunks, hold: false, hold_before_headers: false }]).await;
    let mut model = fixture_model("openrouter", case);
    model.base_url = format!("{}/api/v1", server.base_url);
    let message = stream_simple(&model, context(case), fixture_options(case)).expect("stream").result().await.expect("message");
    assert_eq!(message.stop_reason, StopReason::ToolUse);
    let calls: Vec<(String, String, Value)> = message
        .content
        .iter()
        .filter_map(|part| match part {
            factory_agent::types::AssistantPart::ToolCall { id, name, arguments, .. } => {
                Some((id.clone(), name.clone(), arguments.clone()))
            }
            _ => None,
        })
        .collect();
    assert_eq!(
        calls,
        vec![("call_a".into(), "bash".into(), json!({"command": "ls"})), ("call_b".into(), "read_file".into(), json!({"path": "a"}))]
    );
    assert_eq!((message.usage.input, message.usage.output, message.usage.total_tokens), (10, 5, 15));
}

#[tokio::test]
async fn the_charge_openrouter_reports_replaces_the_table_estimate() {
    let all = cases();
    let case = all.iter().find(|case| case["name"] == "thinking off without routing").expect("case");
    let details = json!({"upstream_inference_cost": 0.5, "upstream_inference_completions_cost": 0.125});
    for (charge, total, fixed) in [
        (json!({"cost": 0.5, "is_byok": false, "cost_details": details}), 0.5, false),
        (json!({"cost": 0.25, "is_byok": true, "cost_details": details}), 0.75, false),
        (json!({"cost": 0.5, "is_byok": false, "cost_details": details}), 1.0, true),
    ] {
        let mut usage = json!({"prompt_tokens": 1000, "completion_tokens": 10});
        usage.as_object_mut().expect("usage").extend(charge.as_object().expect("charge").clone());
        let chunks =
            sse(&[json!({"id": "gen-1", "choices": [{"index": 0, "delta": {"content": "ok"}, "finish_reason": "stop"}], "usage": usage})]);
        let server =
            serve(vec![Served { status: 200, content_type: "text/event-stream", chunks, hold: false, hold_before_headers: false }]).await;
        let mut model = fixture_model("openrouter", case);
        model.base_url = format!("{}/api/v1", server.base_url);
        if fixed {
            model.cost =
                serde_json::from_value(json!({"input": 1000.0, "output": 0.0, "cacheRead": 0.0, "cacheWrite": 0.0})).expect("cost");
            model.compat.get_or_insert_with(Default::default).insert("fixedPricing".into(), json!(true));
        }
        let message = stream_simple(&model, context(case), fixture_options(case)).expect("stream").result().await.expect("message");
        let cost = &message.usage.cost;
        let output = if fixed { 0.0 } else { 0.125 };
        assert_eq!((cost.total, cost.input, cost.output, cost.cache_read), (total, total - output, output, 0.0));
    }
}

#[tokio::test]
async fn a_long_reasoning_details_stream_accumulates_in_linear_time() {
    let all = cases();
    let case = all.iter().find(|case| case["name"] == "thinking off without routing").expect("case");
    let detail = json!({"type": "reasoning.text", "text": "r", "format": "unknown", "index": 0});
    let mut events: Vec<Value> = (0..5000)
        .map(|_| json!({"id": "gen-1", "choices": [{"index": 0, "delta": {"reasoning": "r", "reasoning_details": [detail]}, "finish_reason": null}]}))
        .collect();
    events.push(json!({"id": "gen-1", "choices": [{"index": 0, "delta": {"content": "done"}, "finish_reason": "stop"}], "usage": {"prompt_tokens": 1, "completion_tokens": 1}}));
    let chunks = sse(&events).chunks(100).map(|group| group.concat()).collect();
    let server =
        serve(vec![Served { status: 200, content_type: "text/event-stream", chunks, hold: false, hold_before_headers: false }]).await;
    let mut model = fixture_model("openrouter", case);
    model.base_url = format!("{}/api/v1", server.base_url);
    // The processor time this thread spends, not the time on the clock: a
    // starved test process can wait seconds, but quadratic work burns them.
    // The server, the connection and the provider all run on this thread.
    let started = thread_cpu_time();
    let message = stream_simple(&model, context(case), fixture_options(case)).expect("stream").result().await.expect("message");
    let spent = thread_cpu_time() - started;
    assert!(spent < Duration::from_secs(5), "{spent:?}");
    let Some(factory_agent::types::AssistantPart::Thinking { thinking, thinking_signature, .. }) = message.content.first() else {
        panic!("{:?}", message.content)
    };
    assert_eq!(thinking.len(), 5000);
    assert_eq!(thinking_signature.as_deref(), Some(Value::Array(vec![detail; 5000]).to_string().as_str()));
}

#[tokio::test]
async fn sol_medium_sends_tools_and_preserves_the_returned_call() {
    #[derive(serde::Deserialize)]
    struct Reasoning {
        effort: factory_agent::stream::ThinkingLevel,
    }
    #[derive(serde::Deserialize)]
    struct Function {
        name: String,
    }
    #[derive(serde::Deserialize)]
    struct Tool {
        function: Function,
    }
    #[derive(serde::Deserialize)]
    struct Request {
        model: String,
        reasoning: Reasoning,
        tools: Vec<Tool>,
    }
    #[derive(serde::Deserialize)]
    struct BashArguments {
        command: String,
    }
    let chunks = vec![concat!(
        "data: ",
        r#"{"choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"id":"probe","type":"function","function":{"name":"bash","arguments":"{\"command\":\"pwd\"}"}}]},"finish_reason":"tool_calls"}]}"#,
        "\n\n"
    ).into()];
    let server =
        serve(vec![Served { status: 200, content_type: "text/event-stream", chunks, hold: false, hold_before_headers: false }]).await;
    let mut model = factory_agent::providers::catalog::models("openrouter").into_iter().find(|m| m.id == "openai/gpt-6-sol").expect("Sol");
    model.base_url = server.base_url.clone();
    let options = factory_agent::stream::StreamOptions {
        reasoning: Some(factory_agent::stream::ThinkingLevel::Medium),
        api_key: Some("test".into()),
        ..Default::default()
    };
    let context = factory_agent::stream::LlmContext {
        messages: vec![factory_agent::types::Message::User(factory_agent::types::UserMessage::text("Run pwd", 1))],
        tools: factory_agent::tools::coding_tools(std::path::Path::new("/w"), &[], None).into_iter().map(|tool| tool.spec).collect(),
        ..Default::default()
    };
    let message = stream_simple(&model, context, options).expect("stream").result().await.expect("message");
    assert_eq!(message.stop_reason, StopReason::ToolUse);
    let [factory_agent::types::AssistantPart::ToolCall { id, name, arguments, .. }] = message.content.as_slice() else {
        panic!("{:?}", message.content)
    };
    let arguments: BashArguments = serde_json::from_str(&arguments.to_string()).expect("bash arguments");
    assert_eq!((id.as_str(), name.as_str(), arguments.command.as_str()), ("probe", "bash", "pwd"));
    let requests = server.requests.lock();
    assert_eq!(requests.len(), 1);
    let request: Request = serde_json::from_str(&requests[0].raw).expect("request");
    assert_eq!(request.model, "openai/gpt-6-sol");
    assert_eq!(request.reasoning.effort, factory_agent::stream::ThinkingLevel::Medium);
    assert!(request.tools.iter().any(|tool| tool.function.name == "bash"));
}

#[cfg(windows)]
fn thread_cpu_time() -> Duration {
    factory_agent::windows::system::thread_cpu_time().expect("this thread's processor time")
}

#[cfg(unix)]
fn thread_cpu_time() -> Duration {
    let mut time = libc::timespec { tv_sec: 0, tv_nsec: 0 };
    // SAFETY: clock_gettime only writes the timespec it's given.
    let done = unsafe { libc::clock_gettime(libc::CLOCK_THREAD_CPUTIME_ID, &raw mut time) };
    assert_eq!(done, 0, "clock_gettime");
    Duration::new(time.tv_sec.try_into().expect("seconds"), time.tv_nsec.try_into().expect("nanoseconds"))
}
