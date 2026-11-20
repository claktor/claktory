use serde_json::{Value, json};

use crate::auth::EnvApiKeyAuth;
use crate::errors::Error;
use crate::providers::openai_responses::{Parser, convert_messages, convert_tools, supports_strict_mode};
use crate::providers::sdk::{StreamFailure, read_sdk_stream, send_request, status_error};
use crate::providers::{
    Provider, api_key, check_cancelled, clamp_max_tokens, client, client_api_key, insert_reasoning_effort, model_params, prompt_cache_key,
    response_text, stream_effort, user_agent_header,
};
use crate::stream::{EventSink, EventStream, LlmContext, StreamOptions, ThinkingLevel, pending_message};
use crate::types::{AssistantMessage, Model, StopReason, StreamEvent};

const MIN_OUTPUT_TOKENS: u64 = 16;
const ERROR_PREFIX: &str = "OpenAI API error";

pub fn provider() -> Provider {
    let mut provider = Provider::new(
        "openai",
        "OpenAI",
        "https://api.openai.com/v1",
        api_key(EnvApiKeyAuth { name: "OpenAI API key", env_vars: &["OPENAI_API_KEY"], base_url_var: Some("OPENAI_BASE_URL") }),
        None,
        stream_simple,
    );
    provider.check_key = Some(crate::providers::keys::openai);
    provider
}

fn assign(headers: &mut Vec<(String, String)>, name: &str, value: &str) {
    match headers.iter_mut().find(|(key, _)| key == name) {
        Some((_, slot)) => *slot = value.to_owned(),
        None => headers.push((name.to_owned(), value.to_owned())),
    }
}

fn compat_str<'a>(model: &'a Model, key: &str) -> Option<&'a str> {
    model.compat.as_ref().and_then(|compat| compat.get(key)).and_then(Value::as_str)
}

fn request_headers(model: &Model, options: &StreamOptions) -> Vec<(String, String)> {
    let mut headers = vec![user_agent_header()];
    for (name, value) in model.headers.iter().flatten() {
        if let Some(value) = value.as_str() {
            assign(&mut headers, name, value);
        }
    }
    if let Some(session) = &options.session_id {
        let detected = if model.provider == "openrouter" || model.base_url.contains("openrouter.ai") { "openrouter" } else { "openai" };
        let format = compat_str(model, "sessionAffinityFormat").unwrap_or(detected);
        if format == "openrouter" {
            assign(&mut headers, "x-session-id", session);
        } else {
            if format == "openai" {
                assign(&mut headers, "session_id", session);
            }
            assign(&mut headers, "x-client-request-id", session);
        }
    }
    for (name, value) in &options.headers {
        assign(&mut headers, name, value);
    }
    headers
}

fn build_params(
    model: &Model,
    context: &LlmContext,
    options: &StreamOptions,
    max_tokens: u64,
    effort: Option<ThinkingLevel>,
) -> Result<Value, String> {
    let mut params = model_params(model);
    params.insert("input".into(), Value::Array(convert_messages(model, context)?));
    params.insert("stream".into(), json!(true));
    if let Some(session) = &options.session_id {
        params.insert("prompt_cache_key".into(), json!(prompt_cache_key(session)));
    }
    params.insert("store".into(), json!(false));
    params.insert("max_output_tokens".into(), json!(max_tokens.max(MIN_OUTPUT_TOKENS)));
    let tools = convert_tools(&context.tools, supports_strict_mode(model))?;
    if !tools.is_empty() {
        params.insert("tools".into(), Value::Array(tools));
    }
    if model.reasoning {
        let map = model.thinking_level_map.as_ref();
        if let Some(effort) = effort {
            let mapped = map.and_then(|map| map.get(effort.as_str())).and_then(Value::as_str).unwrap_or(effort.as_str());
            params.insert("reasoning".into(), json!({"effort": mapped, "summary": "auto"}));
            params.insert("include".into(), json!(["reasoning.encrypted_content"]));
        } else if model.provider != "github-copilot" && map.and_then(|map| map.get("off")) != Some(&Value::Null) {
            let off = map.and_then(|map| map.get("off")).and_then(Value::as_str).unwrap_or("none");
            insert_reasoning_effort(&mut params, off.into());
        }
    }
    Ok(Value::Object(params))
}

pub fn stream_simple(model: &Model, context: LlmContext, options: StreamOptions) -> Result<EventStream, Error> {
    client_api_key(model, &options, &[])?;
    let max_tokens = clamp_max_tokens(model, &context, model.max_tokens);
    let effort = stream_effort(model, &options);
    spawn_stream!(model, options, output = pending_message(&model), output, request(&context, &options, max_tokens, effort))
}

async fn request(
    model: &Model,
    context: &LlmContext,
    options: &StreamOptions,
    max_tokens: u64,
    effort: Option<ThinkingLevel>,
    output: &mut AssistantMessage,
    sink: &EventSink,
) -> Result<(), String> {
    let api_key = client_api_key(model, options, &[]).map_err(|error| error.to_string())?;
    let params = build_params(model, context, options, max_tokens, effort)?;
    let mut builder = client().post(format!("{}/responses", model.base_url.trim_end_matches('/')));
    for (name, value) in request_headers(model, options) {
        builder = builder.header(name, value);
    }
    builder = builder
        .header("Authorization", format!("Bearer {api_key}"))
        .header("Content-Type", "application/json")
        .header("Accept", "application/json")
        .body(params.to_string());
    let response = send_request(builder, options).await?;
    let status = response.status().as_u16();
    if !(200..300).contains(&status) {
        let body = response_text(response).await;
        if crate::accounts::credit_error(status, &body) {
            crate::accounts::mark_out_of_credit(output);
        } else if crate::accounts::refused_error(status, &body) {
            crate::accounts::mark_refused(output, &crate::accounts::Refused::default());
        }
        return Err(status_error(status, &body).format(Some(ERROR_PREFIX)));
    }
    sink.push(StreamEvent::Start { partial: output.clone() });
    let mut parser = Parser::new(output.clone());
    let outcome = read_sdk_stream(response, &options.cancel, |event| parser.handle(&event, sink, model)).await;
    let outcome = match outcome {
        Ok(()) => parser.finish(),
        Err(StreamFailure::Api(error)) => Err(error.format(Some(ERROR_PREFIX))),
        Err(StreamFailure::Other(message)) => Err(message),
    };
    *output = parser.output;
    outcome?;
    check_cancelled(options)?;
    if output.stop_reason == StopReason::Pending {
        return Err("OpenAI Responses stream ended without a stop reason".into());
    }
    if matches!(output.stop_reason, StopReason::Aborted | StopReason::Error) {
        return Err(output.error_message.clone().unwrap_or_else(|| "An unknown error occurred".into()));
    }
    Ok(())
}
