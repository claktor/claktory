use std::sync::Arc;

use crate::support::fixtures::{context, fixture_model, fixture_options, replay};
use crate::support::server::{HANG, Served, serve};
use factory_agent::providers::anthropic::stream_simple;
use factory_agent::providers::catalog::models;
use factory_agent::stream::{StreamFn, StreamOptions, ThinkingLevel};
use factory_agent::tools::ToolSpec;
use factory_agent::types::{AssistantPart, StopReason, StreamEvent};
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

fn cases() -> Vec<Value> {
    serde_json::from_str(include_str!("../fixtures/anthropic.json")).expect("fixture")
}

fn stream() -> StreamFn {
    Arc::new(stream_simple)
}

const HEADERS: [&str; 10] = [
    "x-api-key",
    "authorization",
    "anthropic-version",
    "anthropic-beta",
    "anthropic-dangerous-direct-browser-access",
    "accept",
    "content-type",
    "user-agent",
    "x-app",
    "content-encoding",
];

#[tokio::test]
async fn every_recorded_exchange_is_reproduced() {
    for case in cases() {
        replay(&case, "", &HEADERS, stream(), "anthropic").await;
    }
}

fn sse(events: &[Value]) -> Vec<String> {
    events.iter().map(|event| format!("event: {}\ndata: {event}\n\n", event["type"].as_str().expect("type"))).collect()
}

fn served(chunks: Vec<String>, hold: bool, hold_before_headers: bool) -> Served {
    Served { status: 200, content_type: "text/event-stream", chunks, hold, hold_before_headers }
}

#[tokio::test]
async fn cancellation_before_headers_and_during_the_body_use_pis_texts() {
    let all = cases();
    let case = all.iter().find(|case| case["name"] == "thinking off").expect("case");
    let start = json!({"type": "message_start", "message": {"id": "msg_9", "model": "claude-fable-5", "usage": {"input_tokens": 3, "output_tokens": 1}}});
    for (before_headers, chunks, expected) in [(true, Vec::new(), "Request aborted"), (false, sse(&[start]), "Request was aborted")] {
        let server = serve(vec![served(chunks, true, before_headers)]).await;
        let mut model = fixture_model("anthropic", case);
        model.base_url = server.base_url.clone();
        let cancel = CancellationToken::new();
        let mut events =
            stream_simple(&model, context(case), StreamOptions { cancel: cancel.clone(), ..fixture_options(case) }).expect("stream");
        // Before headers: once the request is in. During the body: once the
        // client has read message_start, its only event.
        if before_headers {
            server.cancel_once_requested(cancel.clone());
        }
        let mut last = None;
        while let Some(event) = tokio::time::timeout(HANG, events.next()).await.expect("the provider task ends") {
            if matches!(event, StreamEvent::Start { .. }) {
                cancel.cancel();
            }
            last = Some(event);
        }
        let Some(StreamEvent::Error { reason, error }) = last else { panic!("{last:?}") };
        assert_eq!((reason, error.error_message.as_deref()), (StopReason::Aborted, Some(expected)), "{before_headers}");
    }
}

#[tokio::test]
async fn an_oauth_token_maps_claude_code_tool_names_back() {
    let all = cases();
    let case = all.iter().find(|case| case["name"] == "oauth token sends claude code identity and tool names").expect("case");
    let events = sse(&[
        json!({"type": "message_start", "message": {"id": "msg_8", "model": "claude-fable-5", "usage": {"input_tokens": 3, "output_tokens": 1}}}),
        json!({"type": "content_block_start", "index": 0, "content_block": {"type": "tool_use", "id": "toolu_9", "name": "Bash", "input": {}}}),
        json!({"type": "content_block_delta", "index": 0, "delta": {"type": "input_json_delta", "partial_json": "{\"command\":\"ls\"}"}}),
        json!({"type": "content_block_stop", "index": 0}),
        json!({"type": "message_delta", "delta": {"stop_reason": "tool_use"}, "usage": {"output_tokens": 9}}),
        json!({"type": "message_stop"}),
    ]);
    let server = serve(vec![served(events, false, false)]).await;
    let mut model = fixture_model("anthropic", case);
    model.base_url = server.base_url.clone();
    let message = stream_simple(&model, context(case), fixture_options(case)).expect("stream").result().await.expect("message");
    assert_eq!(message.stop_reason, StopReason::ToolUse);
    assert!(
        matches!(&message.content[0], AssistantPart::ToolCall { name, arguments, .. } if name == "bash" && *arguments == json!({"command": "ls"}))
    );
    assert_eq!(message.usage.output, 9);
}

#[tokio::test]
async fn an_auth_token_header_replaces_the_api_key() {
    let all = cases();
    let case = all.iter().find(|case| case["name"] == "thinking off").expect("case");
    let server = serve(vec![served(
        case["http"]["chunks"].as_array().expect("chunks").iter().filter_map(Value::as_str).map(str::to_owned).collect(),
        false,
        false,
    )])
    .await;
    let mut model = fixture_model("anthropic", case);
    model.base_url = server.base_url.clone();
    let options =
        StreamOptions { api_key: None, headers: vec![("Authorization".into(), "Bearer env-token".into())], ..fixture_options(case) };
    let message = stream_simple(&model, context(case), options).expect("stream").result().await.expect("message");
    assert_eq!(message.stop_reason, StopReason::Stop);
    let request = server.requests.lock()[0].clone();
    let header = |name: &str| request.headers.iter().find(|(key, _)| key == name).map(|(_, value)| value.clone());
    assert_eq!((header("authorization").as_deref(), header("x-api-key")), (Some("Bearer env-token"), None));
}

#[tokio::test]
async fn opus_5_5_never_sends_disabled_thinking() {
    let all = cases();
    let case = all.iter().find(|case| case["name"] == "thinking off").expect("case");
    let chunks: Vec<String> =
        case["http"]["chunks"].as_array().expect("chunks").iter().filter_map(Value::as_str).map(str::to_owned).collect();
    let server = serve(vec![served(chunks.clone(), false, false), served(chunks, false, false)]).await;
    let mut model = models("anthropic").into_iter().find(|model| model.id == "claude-opus-5-5").expect("opus 5.5");
    model.base_url = server.base_url.clone();
    for reasoning in [None, Some(ThinkingLevel::Xhigh)] {
        let options = StreamOptions { reasoning, ..fixture_options(case) };
        let message = stream_simple(&model, context(case), options).expect("stream").result().await.expect("message");
        assert_eq!(message.stop_reason, StopReason::Stop, "{:?}", message.error_message);
    }
    let requests = server.requests.lock().clone();
    assert_eq!(requests[0].body.get("thinking"), None);
    assert_eq!(requests[1].body["thinking"]["type"], "adaptive");
    assert_eq!(requests[1].body["output_config"]["effort"], "xhigh");
}

#[tokio::test]
async fn strict_tools_move_numeric_bounds_into_descriptions() {
    let all = cases();
    let case = all.iter().find(|case| case["name"] == "first turn with tools and medium thinking on an adaptive model").expect("case");
    let chunks: Vec<String> =
        case["http"]["chunks"].as_array().expect("chunks").iter().filter_map(Value::as_str).map(str::to_owned).collect();
    let server = serve(vec![served(chunks.clone(), false, false), served(chunks, false, false)]).await;
    let strict = models("anthropic").into_iter().find(|model| model.id == "claude-opus-5-5").expect("opus 5.5");
    let mut loose = strict.clone();
    loose.compat.get_or_insert_with(Default::default).insert("supportsStrictTools".into(), json!(false));
    let mut context = context(case);
    context.tools.push(ToolSpec {
        name: "pick".into(),
        label: "pick".into(),
        description: "Pick counts.".into(),
        parameters: json!({"type": "object", "required": ["counts"], "properties": {"counts": {"type": "array", "items": {"type": "integer", "description": "A count.", "exclusiveMaximum": 10, "multipleOf": 2}, "minItems": 1}}, "additionalProperties": false}),
        constrained_sampling: json!({"type": "json_schema", "strict": "prefer"}),
    });
    for mut model in [strict, loose] {
        model.base_url = server.base_url.clone();
        let message = stream_simple(&model, context.clone(), fixture_options(case)).expect("stream").result().await.expect("message");
        assert_eq!(message.error_message, None);
    }
    let requests = server.requests.lock().clone();
    let tools = |index: usize| requests[index].body["tools"].as_array().expect("tools").clone();
    let (strict, loose) = (tools(0), tools(1));
    assert!(strict.iter().all(|tool| tool["strict"] == true) && loose.iter().all(|tool| tool.get("strict").is_none()));
    let sent = serde_json::to_string(&strict).expect("tools");
    for keyword in ["minimum", "exclusiveMinimum", "maximum", "exclusiveMaximum", "multipleOf"] {
        assert!(!sent.contains(&format!("\"{keyword}\"")), "{keyword}");
    }
    let limit = |tools: &[Value]| tools[0]["input_schema"]["properties"]["limit"]["anyOf"][0].clone();
    assert_eq!(limit(&strict), json!({"type": "integer", "description": "Minimum 1. Maximum 2000."}));
    assert_eq!(limit(&loose), json!({"type": "integer", "minimum": 1, "maximum": 2000}));
    assert_eq!(
        strict[3]["input_schema"]["properties"]["timeout"]["anyOf"][0],
        json!({"type": "number", "description": "Greater than 0. Maximum 7200."})
    );
    assert_eq!(
        strict[4]["input_schema"]["properties"]["counts"],
        json!({"type": "array", "items": {"type": "integer", "description": "A count. Less than 10. Multiple of 2."}, "minItems": 1})
    );
    assert_eq!(strict[2]["input_schema"]["properties"]["edits"]["minItems"], 1);
}

#[derive(serde::Deserialize)]
struct BudgetFixture {
    name: String,
    http: BudgetHttp,
}

#[derive(serde::Deserialize)]
struct BudgetHttp {
    chunks: Vec<String>,
}

#[derive(serde::Deserialize)]
struct BudgetRequest {
    thinking: Option<BudgetThinking>,
    output_config: Option<BudgetEffort>,
    context_management: Option<BudgetContext>,
    messages: Vec<BudgetMessage>,
}

#[derive(serde::Deserialize)]
struct BudgetThinking {
    r#type: String,
    display: String,
}

#[derive(serde::Deserialize)]
struct BudgetEffort {
    effort: String,
}

#[derive(serde::Deserialize)]
struct BudgetContext {
    edits: Vec<BudgetEdit>,
}

#[derive(serde::Deserialize)]
struct BudgetEdit {
    r#type: String,
    keep: String,
}

#[derive(serde::Deserialize)]
struct BudgetMessage {
    role: String,
    content: Vec<BudgetText>,
}

#[derive(serde::Deserialize)]
struct BudgetText {
    r#type: String,
    text: String,
    cache_control: Option<BudgetCache>,
}

#[derive(serde::Deserialize)]
struct BudgetCache {
    r#type: String,
}

#[tokio::test]
async fn budget_notes_reach_anthropic_with_a_cache_breakpoint_and_the_selected_request_settings() {
    use factory_agent::stream::LlmContext;
    use factory_agent::types::{Message, SystemMessage};
    for claude_request in [false, true] {
        let fixture = serde_json::from_str::<Vec<BudgetFixture>>(include_str!("../fixtures/anthropic.json"))
            .unwrap()
            .into_iter()
            .find(|case| case.name == "thinking off")
            .unwrap();
        let server = serve(vec![served(fixture.http.chunks, false, false)]).await;
        let mut model = models("anthropic").into_iter().find(|model| model.id == "claude-sonnet-5-5").unwrap();
        model.base_url = server.base_url.clone();
        if claude_request {
            model.compat.as_mut().unwrap().insert("claudeCodeRequest".into(), true.into());
        }
        let context = LlmContext {
            system_prompt: None,
            messages: vec![Message::System(SystemMessage { text: "USD budget: $0/$0.3; $0.3 remaining".into(), timestamp: 1 })],
            tools: vec![],
        };
        let options = StreamOptions { api_key: Some("test-key".into()), reasoning: Some(ThinkingLevel::High), ..Default::default() };
        let result = stream_simple(&model, context, options).unwrap().result().await.unwrap();
        assert_eq!(result.stop_reason, StopReason::Stop, "{:?}", result.error_message);
        let request = server.requests.lock()[0].clone();
        let body: BudgetRequest = serde_json::from_str(&request.body.to_string()).unwrap();
        let thinking = body.thinking.unwrap();
        assert_eq!(thinking.r#type, "adaptive");
        assert_eq!(thinking.display, if claude_request { "updates" } else { "summarized" });
        assert_eq!(body.output_config.unwrap().effort, "high");
        assert_eq!(body.context_management.is_some(), claude_request);
        if let Some(context) = body.context_management {
            assert_eq!(context.edits.len(), 1);
            assert_eq!(context.edits[0].r#type, "clear_thinking_20251015");
            assert_eq!(context.edits[0].keep, "all");
        }
        let last = body.messages.last().unwrap();
        assert_eq!(last.role, "system");
        assert_eq!(last.content.len(), 1);
        assert_eq!(last.content[0].r#type, "text");
        assert_eq!(last.content[0].text, "USD budget: $0/$0.3; $0.3 remaining");
        assert_eq!(last.content[0].cache_control.as_ref().unwrap().r#type, "ephemeral");
        if claude_request {
            let betas = &request.headers.iter().find(|(name, _)| name == "anthropic-beta").unwrap().1;
            for beta in ["claude-code-20250219", "interleaved-thinking-2025-05-14", "effort-2025-11-24", "context-management-2025-06-27"] {
                assert!(betas.split(',').any(|listed| listed == beta), "{betas}");
            }
        }
    }
}
