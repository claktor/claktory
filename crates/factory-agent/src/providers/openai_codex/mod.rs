pub mod oauth;
pub mod websocket;

use hashbrown::HashSet;
use std::sync::LazyLock;
use std::time::Duration;

use futures_util::StreamExt;
use parking_lot::Mutex;
use serde_json::{Map, Value, json};

use crate::clock::now_ms;
use crate::errors::Error;
use crate::providers::openai_codex::oauth::{CodexOAuth, account_id};
use crate::providers::openai_responses::{Parser, convert_input, convert_tools_with};
use crate::providers::{
    Provider, check_cancelled, client, model_headers, model_params, no_api_key, oauth, prompt_cache_key, response_text, stream_effort,
    user_agent,
};
use crate::stream::{EventSink, EventStream, LlmContext, StreamOptions, ThinkingLevel, pending_message};
use crate::types::{AssistantMessage, Model, StopReason, StreamEvent};

const DEFAULT_CODEX_BASE_URL: &str = "https://chatgpt.com/backend-api";
const RESPONSE_STATUSES: [&str; 6] = ["completed", "incomplete", "failed", "cancelled", "queued", "in_progress"];
const USAGE_LIMIT_CODES: [&str; 3] = ["usage_limit_reached", "usage_not_included", "rate_limit_exceeded"];

pub use crate::stream::Transport;

/// Pi's `CodexApiError`, `CodexProtocolError` and transport failures; only the first two skip the SSE fallback.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CodexFailure {
    Api { code: Option<String>, message: String },
    Protocol(String),
    Transport { message: String, close_code: Option<u16> },
}

fn check_transport_cancelled(options: &StreamOptions) -> Result<(), CodexFailure> {
    if options.cancel.is_cancelled() {
        return Err(CodexFailure::Transport { message: "Request was aborted".into(), close_code: None });
    }
    Ok(())
}

impl CodexFailure {
    pub fn message(&self) -> &str {
        match self {
            CodexFailure::Api { message, .. } | CodexFailure::Protocol(message) | CodexFailure::Transport { message, .. } => message,
        }
    }

    pub fn code(&self) -> Option<&str> {
        match self {
            CodexFailure::Api { code, .. } => code.as_deref(),
            _ => None,
        }
    }
}

pub fn provider() -> Provider {
    let mut provider = Provider::new("openai-codex", "OpenAI Codex", DEFAULT_CODEX_BASE_URL, None, oauth(CodexOAuth::new()), stream_simple);
    provider.limits = Some(crate::limits::codex);
    provider.plan = Some(crate::limits::codex_plan);
    provider.identity = Some(crate::limits::codex_identity);
    provider.reset = Some(crate::limits::codex_reset);
    provider
}

pub fn resolve_codex_url(base_url: &str) -> String {
    let raw = if base_url.trim().is_empty() { DEFAULT_CODEX_BASE_URL } else { base_url };
    let normalized = raw.trim_end_matches('/');
    if normalized.ends_with("/codex/responses") {
        normalized.to_owned()
    } else if normalized.ends_with("/codex") {
        format!("{normalized}/responses")
    } else {
        format!("{normalized}/codex/responses")
    }
}

pub fn resolve_websocket_url(base_url: &str) -> String {
    let url = resolve_codex_url(base_url);
    if let Some(rest) = url.strip_prefix("https:") {
        format!("wss:{rest}")
    } else if let Some(rest) = url.strip_prefix("http:") {
        format!("ws:{rest}")
    } else {
        url
    }
}

fn set_header(headers: &mut Vec<(String, String)>, name: &str, value: &str) {
    headers.retain(|(key, _)| !key.eq_ignore_ascii_case(name));
    headers.push((name.to_owned(), value.to_owned()));
}

fn delete_header(headers: &mut Vec<(String, String)>, name: &str) {
    headers.retain(|(key, _)| !key.eq_ignore_ascii_case(name));
}

fn base_headers(model: &Model, options: &StreamOptions, account: &str, token: &str) -> Vec<(String, String)> {
    let mut headers = Vec::new();
    for (name, value) in model_headers(model) {
        set_header(&mut headers, &name, &value);
    }
    for (name, value) in &options.headers {
        set_header(&mut headers, name, value);
    }
    set_header(&mut headers, "Authorization", &format!("Bearer {token}"));
    set_header(&mut headers, "chatgpt-account-id", account);
    set_header(&mut headers, "originator", "pi");
    set_header(&mut headers, "User-Agent", user_agent());
    headers
}

pub struct Request {
    pub body: Value,
    pub body_json: String,
    pub sse_headers: Vec<(String, String)>,
    pub websocket_headers: Vec<(String, String)>,
    pub session_id: Option<String>,
    pub account_id: String,
    pub request_id: String,
}

/// `buildRequestBody` plus both header sets; the reasoning effort is already clamped (Pi's `streamSimple`).
pub fn build_request(
    model: &Model,
    context: &LlmContext,
    options: &StreamOptions,
    effort: Option<ThinkingLevel>,
) -> Result<Request, String> {
    let token = options.api_key.clone().ok_or_else(|| format!("No API key for provider: {}", model.provider))?;
    let account = account_id(&token).ok_or_else(|| "Failed to extract accountId from token".to_owned())?;
    let session_id = options.session_id.as_deref().map(prompt_cache_key);
    let supports_strict_mode =
        model.compat.as_ref().and_then(|compat| compat.get("supportsStrictMode")).and_then(Value::as_bool).unwrap_or(true);
    let mut body = model_params(model);
    body.insert("store".into(), json!(false));
    body.insert("stream".into(), json!(true));
    body.insert(
        "instructions".into(),
        json!(context.system_prompt.as_deref().filter(|prompt| !prompt.is_empty()).unwrap_or("You are a helpful assistant.")),
    );
    body.insert("input".into(), Value::Array(convert_input(model, context, false)?));
    body.insert("text".into(), json!({"verbosity": "low"}));
    body.insert("include".into(), json!(["reasoning.encrypted_content"]));
    if let Some(session) = &session_id {
        body.insert("prompt_cache_key".into(), json!(session));
    }
    body.insert("tool_choice".into(), json!("auto"));
    body.insert("parallel_tool_calls".into(), json!(true));
    let tools = convert_tools_with(&context.tools, supports_strict_mode, Value::Null)?;
    if !tools.is_empty() {
        body.insert("tools".into(), Value::Array(tools));
    }
    if let Some(effort) = effort {
        let mapped = model.thinking_level_map.as_ref().and_then(|map| map.get(effort.as_str()));
        let effort = match mapped {
            Some(Value::Null) => None,
            Some(Value::String(mapped)) => Some(mapped.clone()),
            _ => Some(effort.as_str().to_owned()),
        };
        if let Some(effort) = effort {
            body.insert("reasoning".into(), json!({"effort": effort, "summary": "auto"}));
        }
    }
    let body = Value::Object(body);
    let body_json = body.to_string();
    let request_id = session_id.clone().unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
    let mut sse_headers = base_headers(model, options, &account, &token);
    set_header(&mut sse_headers, "OpenAI-Beta", "responses=experimental");
    set_header(&mut sse_headers, "accept", "text/event-stream");
    set_header(&mut sse_headers, "content-type", "application/json");
    if let Some(session) = &session_id {
        set_header(&mut sse_headers, "session-id", session);
        set_header(&mut sse_headers, "x-client-request-id", session);
    }
    let mut websocket_headers = base_headers(model, options, &account, &token);
    delete_header(&mut websocket_headers, "accept");
    delete_header(&mut websocket_headers, "content-type");
    delete_header(&mut websocket_headers, "OpenAI-Beta");
    set_header(&mut websocket_headers, "OpenAI-Beta", "responses_websockets=2026-02-06");
    set_header(&mut websocket_headers, "x-client-request-id", &request_id);
    set_header(&mut websocket_headers, "session-id", &request_id);
    Ok(Request { body, body_json, sse_headers, websocket_headers, session_id, account_id: account, request_id })
}

fn truthy_str(value: &Value) -> Option<&str> {
    value.as_str().filter(|text| !text.is_empty())
}

/// Whether a Codex error object is the account's usage limit:
/// `usage_limit_reached`, or `usage_not_included` (a plan without Codex).
/// Some, with when it resets (`resets_at`, seconds since 1970, or
/// `resets_in_seconds`) if said.
fn usage_limit(error: &Value) -> Option<Option<u64>> {
    let code = truthy_str(&error["code"]).or_else(|| truthy_str(&error["type"]))?;
    if !["usage_limit_reached", "usage_not_included"].iter().any(|limit| code.eq_ignore_ascii_case(limit)) {
        return None;
    }
    let at = error["resets_at"].as_f64().filter(|at| *at > 0.0).map(|at| (at * 1000.0) as u64);
    let after = error["resets_in_seconds"].as_f64().filter(|after| *after > 0.0).map(|after| now_ms() + (after * 1000.0) as u64);
    Some(at.or(after))
}

/// Marks `output` when a failed response's body says the account reached its usage limit.
fn mark_if_usage_limit(output: &mut AssistantMessage, error: &Value) {
    if let Some(resets_at) = usage_limit(error) {
        crate::accounts::mark_usage_limit(output, resets_at);
    }
}

/// `parseErrorResponse` followed by `throw new Error(info.friendlyMessage || info.message)`.
pub fn parse_error_response(status: u16, status_text: Option<&str>, raw: &str) -> String {
    let mut message =
        if !raw.is_empty() { raw.to_owned() } else { status_text.filter(|text| !text.is_empty()).unwrap_or("Request failed").to_owned() };
    let mut friendly = None;
    if let Ok(parsed) = serde_json::from_str::<Value>(raw) {
        let error = &parsed["error"];
        if !error.is_null() && error != &json!(false) && error != &json!(0) && error != &json!("") {
            let code = truthy_str(&error["code"]).or_else(|| truthy_str(&error["type"])).unwrap_or("");
            if USAGE_LIMIT_CODES.iter().any(|limit| code.to_ascii_lowercase().contains(limit)) || status == 429 {
                let plan = truthy_str(&error["plan_type"]).map(|plan| format!(" ({} plan)", plan.to_lowercase())).unwrap_or_default();
                let when = error["resets_at"].as_f64().filter(|resets| *resets != 0.0).map(|resets| {
                    let minutes = ((resets * 1000.0 - now_ms() as f64) / 60_000.0).round().max(0.0);
                    format!(" Try again in ~{minutes} min.")
                });
                friendly = Some(format!("You have hit your ChatGPT usage limit{plan}.{}", when.unwrap_or_default()).trim().to_owned());
            }
            if let Some(text) = truthy_str(&error["message"]) {
                message = text.to_owned();
            } else if let Some(friendly) = &friendly {
                message = friendly.clone();
            }
        }
    }
    friendly.unwrap_or(message)
}

pub enum Mapped {
    Event(Value),
    Terminal(Value),
    Skip,
}

fn js_json(value: &Value) -> String {
    value.to_string()
}

/// `mapCodexEvents`: Codex error shapes become failures, `response.done` becomes `response.completed`, unknown statuses are dropped.
pub fn map_codex_event(event: Value, output: &mut AssistantMessage) -> Result<Mapped, CodexFailure> {
    let Some(kind) = event["type"].as_str().map(str::to_owned) else { return Ok(Mapped::Skip) };
    match kind.as_str() {
        "error" => {
            let nested = event.get("error").filter(|nested| nested.is_object());
            mark_if_usage_limit(output, nested.unwrap_or(&event));
            let code = event["code"].as_str().or_else(|| nested.and_then(|nested| nested["code"].as_str())).map(str::to_owned);
            let message = event["message"].as_str().or_else(|| nested.and_then(|nested| nested["message"].as_str())).map(str::to_owned);
            let text = message
                .clone()
                .filter(|text| !text.is_empty())
                .or_else(|| code.clone().filter(|code| !code.is_empty()))
                .unwrap_or_else(|| js_json(&event));
            Err(CodexFailure::Api { code, message: format!("Codex error: {text}") })
        }
        "response.failed" => {
            let error = &event["response"]["error"];
            mark_if_usage_limit(output, error);
            let message = truthy_str(&error["message"]).unwrap_or("Codex response failed").to_owned();
            Err(CodexFailure::Api { code: error["code"].as_str().map(str::to_owned), message })
        }
        "response.done" | "response.completed" | "response.incomplete" => {
            let mut mapped = event.clone();
            mapped["type"] = json!("response.completed");
            if let Some(response) = mapped.get_mut("response").and_then(Value::as_object_mut) {
                if let Some(end_turn) = response.get("end_turn").and_then(Value::as_bool) {
                    output.extra.insert("endTurn".into(), json!(end_turn));
                }
                let status =
                    response.get("status").and_then(Value::as_str).filter(|status| RESPONSE_STATUSES.contains(status)).map(str::to_owned);
                match status {
                    Some(status) => {
                        response.insert("status".into(), json!(status));
                    }
                    None => {
                        response.remove("status");
                    }
                }
            }
            Ok(Mapped::Terminal(mapped))
        }
        _ => Ok(Mapped::Event(event)),
    }
}

/// Pi's own SSE reader: frames end at a blank line, `data:` lines are trimmed and joined, `[DONE]` and empties are skipped.
pub fn sse_frames(buffer: &mut Vec<u8>) -> Vec<String> {
    let mut frames = Vec::new();
    while let Some(index) = buffer.windows(2).position(|window| window == b"\n\n") {
        let chunk = String::from_utf8_lossy(&buffer[..index]).into_owned();
        buffer.drain(..index + 2);
        let data: Vec<&str> = chunk.split('\n').filter(|line| line.starts_with("data:")).map(|line| line[5..].trim()).collect();
        if data.is_empty() {
            continue;
        }
        let joined = data.join("\n").trim().to_owned();
        if !joined.is_empty() && joined != "[DONE]" {
            frames.push(joined);
        }
    }
    frames
}

fn handle_frame(frame: &str, parser: &mut Parser, sink: &EventSink, model: &Model, finished: &mut bool) -> Result<(), CodexFailure> {
    if *finished {
        return Ok(());
    }
    let event: Value = serde_json::from_str(frame).map_err(|error| CodexFailure::Protocol(format!("Invalid Codex SSE JSON: {error}")))?;
    match map_codex_event(event, &mut parser.output)? {
        Mapped::Skip => Ok(()),
        Mapped::Event(event) => parser.handle(&event, sink, model).map_err(CodexFailure::Protocol),
        Mapped::Terminal(event) => {
            *finished = true;
            parser.handle(&event, sink, model).map_err(CodexFailure::Protocol)
        }
    }
}

async fn sse_request(
    request: &Request,
    model: &Model,
    options: &StreamOptions,
    parser: &mut Parser,
    sink: &EventSink,
    start_emitted: &mut bool,
) -> Result<(), CodexFailure> {
    check_transport_cancelled(options)?;
    let mut builder = client().post(resolve_codex_url(&model.base_url));
    for (name, value) in &request.sse_headers {
        builder = builder.header(name, value);
    }
    builder = builder.body(request.body_json.clone());
    let send = async {
        match options.timeout_ms {
            Some(timeout) if timeout > 0 => tokio::time::timeout(Duration::from_millis(timeout), builder.send())
                .await
                .map_err(|_| format!("Codex SSE response headers timed out after {timeout}ms")),
            _ => Ok(builder.send().await),
        }
    };
    let response = tokio::select! {
        sent = send => match sent {
            Ok(Ok(response)) => response,
            Ok(Err(error)) => return Err(CodexFailure::Transport { message: error.to_string(), close_code: None }),
            Err(message) => return Err(CodexFailure::Transport { message, close_code: None }),
        },
        () = options.cancel.cancelled() => return Err(CodexFailure::Transport { message: "Request was aborted".into(), close_code: None }),
    };
    let status = response.status();
    if !status.is_success() {
        let text = response_text(response).await;
        if let Ok(body) = serde_json::from_str::<Value>(&text) {
            mark_if_usage_limit(&mut parser.output, &body["error"]);
        }
        return Err(CodexFailure::Api { code: None, message: parse_error_response(status.as_u16(), status.canonical_reason(), &text) });
    }
    if !*start_emitted {
        *start_emitted = true;
        sink.push(StreamEvent::Start { partial: parser.output.clone() });
    }
    let mut body = response.bytes_stream();
    let mut buffer = Vec::new();
    let mut finished = false;
    while !finished {
        let chunk = tokio::select! {
            chunk = body.next() => chunk,
            () = options.cancel.cancelled() => return Err(CodexFailure::Transport { message: "Request was aborted".into(), close_code: None }),
        };
        let Some(chunk) = chunk else { break };
        let chunk = chunk.map_err(|error| CodexFailure::Transport { message: error.to_string(), close_code: None })?;
        buffer.extend_from_slice(&chunk);
        for frame in sse_frames(&mut buffer) {
            handle_frame(&frame, parser, sink, model, &mut finished)?;
        }
    }
    check_transport_cancelled(options)?;
    parser.finish().map_err(CodexFailure::Protocol)
}

fn assert_successful(output: &AssistantMessage) -> Result<(), String> {
    match output.stop_reason {
        StopReason::Pending => Err("Codex stream ended without a stop reason".into()),
        StopReason::Error | StopReason::Aborted => Err(output.error_message.clone().unwrap_or_else(|| "An unknown error occurred".into())),
        _ => Ok(()),
    }
}

impl CodexFailure {
    fn is_non_transport(&self) -> bool {
        matches!(self, CodexFailure::Api { .. } | CodexFailure::Protocol(_))
    }
}

/// Pi's `provider_transport_failure` diagnostic on the message; the JavaScript stack is the one field not reproduced.
fn append_diagnostic(output: &mut AssistantMessage, failure: &CodexFailure, started: bool, request_bytes: usize) {
    let (name, code) = match failure {
        CodexFailure::Transport { close_code: Some(code), .. } => ("WebSocketCloseError", Some(json!(code))),
        CodexFailure::Transport { .. } => ("Error", None),
        CodexFailure::Api { code, .. } => ("CodexApiError", code.as_ref().map(|code| json!(code))),
        CodexFailure::Protocol(_) => ("CodexProtocolError", None),
    };
    let mut error = json!({"name": name, "message": failure.message()});
    if let Some(code) = code {
        error["code"] = code;
    }
    let mut details = Map::new();
    details.insert("configuredTransport".into(), json!("auto"));
    if !started {
        details.insert("fallbackTransport".into(), json!("sse"));
    }
    details.insert("eventsEmitted".into(), json!(started));
    details.insert("phase".into(), json!(if started { "after_message_stream_start" } else { "before_message_stream_start" }));
    details.insert("requestBytes".into(), json!(request_bytes));
    let diagnostic = json!({"type": "provider_transport_failure", "timestamp": now_ms(), "error": error, "details": details});
    match output.extra.get_mut("diagnostics").and_then(Value::as_array_mut) {
        Some(diagnostics) => diagnostics.push(diagnostic),
        None => {
            output.extra.insert("diagnostics".into(), json!([diagnostic]));
        }
    }
}

static SSE_ONLY_SESSIONS: LazyLock<Mutex<HashSet<String>>> = LazyLock::new(|| Mutex::new(HashSet::new()));

pub fn sse_fallback_active(session_id: Option<&str>) -> bool {
    session_id.is_some_and(|session| SSE_ONLY_SESSIONS.lock().contains(session))
}

pub fn record_websocket_failure(session_id: Option<&str>) {
    if let Some(session) = session_id {
        SSE_ONLY_SESSIONS.lock().insert(session.to_owned());
    }
}

pub fn stream_simple(model: &Model, context: LlmContext, options: StreamOptions) -> Result<EventStream, Error> {
    let transport = options.transport;
    stream_simple_with(model, context, options, transport)
}

pub fn stream_simple_with(model: &Model, context: LlmContext, options: StreamOptions, transport: Transport) -> Result<EventStream, Error> {
    if options.api_key.is_none() {
        return Err(no_api_key(model));
    }
    let effort = stream_effort(model, &options);
    spawn_stream!(model, options, parser = Parser::new(pending_message(&model)), parser.output, run(&context, &options, effort, transport))
}

async fn run(
    model: &Model,
    context: &LlmContext,
    options: &StreamOptions,
    effort: Option<ThinkingLevel>,
    transport: Transport,
    parser: &mut Parser,
    sink: &EventSink,
) -> Result<(), String> {
    let request = build_request(model, context, options, effort)?;
    let mut start_emitted = false;
    let session = request.session_id.as_deref();
    if transport == Transport::Auto && !sse_fallback_active(session) {
        let url = resolve_websocket_url(&model.base_url);
        let (mut retried_limit, mut retried_missing) = (false, false);
        loop {
            let mut started = false;
            let outcome =
                websocket::process(&url, &request, model, options, parser, sink, &mut started, &mut start_emitted).await.and_then(|()| {
                    check_transport_cancelled(options)?;
                    assert_successful(&parser.output).map_err(|message| CodexFailure::Transport { message, close_code: None })
                });
            match outcome {
                Ok(()) => return Ok(()),
                Err(failure) => {
                    let aborted = options.cancel.is_cancelled();
                    let limit_before_start = !started && failure.code() == Some("websocket_connection_limit_reached");
                    if !aborted && failure.code() == Some("previous_response_not_found") && !retried_missing {
                        retried_missing = true;
                        continue;
                    }
                    if !aborted && limit_before_start && !retried_limit {
                        retried_limit = true;
                        continue;
                    }
                    if aborted || (failure.is_non_transport() && !limit_before_start) {
                        return Err(failure.message().to_owned());
                    }
                    append_diagnostic(&mut parser.output, &failure, started, request.body_json.len());
                    record_websocket_failure(session);
                    if started {
                        return Err(failure.message().to_owned());
                    }
                    break;
                }
            }
        }
    }
    sse_request(&request, model, options, parser, sink, &mut start_emitted).await.map_err(|failure| failure.message().to_owned())?;
    check_cancelled(options)?;
    assert_successful(&parser.output)
}
