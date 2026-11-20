pub mod oauth;

use hashbrown::HashMap;
use serde_json::{Map, Value, json};

use crate::auth::EnvApiKeyAuth;
use crate::errors::Error;
use crate::json_parse::parse_streaming_json;
use crate::providers::openai_responses::{
    Note, UserMessageDialect, chat_image, convert_tools_with, safe_id, short_hash, transform_messages, user_message,
};
use crate::providers::openrouter::oauth::OpenRouterOAuth;
use crate::providers::sdk::{ProviderError, StreamFailure, read_sdk_stream, send_request, status_error, truthy};
use crate::providers::{
    Provider, api_key, check_cancelled, clamp_max_tokens, client, client_api_key, count, ephemeral, insert_reasoning_effort, merge_headers,
    model_headers, model_params, oauth, push, response_text, set_stop_reason, stream_effort, tool_text, toolcall_delta, user_agent_header,
    user_content,
};
use crate::stream::{EventSink, EventStream, LlmContext, StreamOptions, ThinkingLevel, pending_message};
use crate::tools::ToolSpec;
use crate::types::{AssistantMessage, AssistantPart, Cost, Message, Model, StopReason, StreamEvent, ToolResultPart, Usage};
use crate::usage::calculate_cost;

const REASONING_FIELDS: [&str; 3] = ["reasoning_content", "reasoning", "reasoning_text"];
const TOOL_ID_LIMIT: usize = 40;

pub fn provider() -> Provider {
    Provider::new(
        "openrouter",
        "OpenRouter",
        "https://openrouter.ai/api/v1",
        api_key(EnvApiKeyAuth { name: "OpenRouter API key", env_vars: &["OPENROUTER_API_KEY"], base_url_var: Some("OPENROUTER_BASE_URL") }),
        oauth(OpenRouterOAuth::new()),
        stream_simple,
    )
}

/// `getCompat` for the fields an OpenRouter model reaches: detection by provider or base URL, then `model.compat`.
struct Compat {
    supports_store: bool,
    supports_developer_role: bool,
    supports_reasoning_effort: bool,
    supports_usage_in_streaming: bool,
    supports_finish_reason: bool,
    max_tokens_field: String,
    requires_reasoning_content: bool,
    thinking_format: String,
    supports_strict_mode: bool,
    anthropic_cache_control: bool,
}

fn compat(model: &Model) -> Compat {
    let overrides = model.compat.as_ref();
    let flag = |key: &str, detected: bool| overrides.and_then(|compat| compat.get(key)).and_then(Value::as_bool).unwrap_or(detected);
    let text =
        |key: &str, detected: &str| overrides.and_then(|compat| compat.get(key)).and_then(Value::as_str).unwrap_or(detected).to_owned();
    let openrouter = model.provider == "openrouter" || model.base_url.contains("openrouter.ai");
    let developer_model = openrouter && (model.id.starts_with("anthropic/") || model.id.starts_with("openai/"));
    let cache_control = if model.provider == "openrouter" && model.id.starts_with("anthropic/") { "anthropic" } else { "" };
    Compat {
        supports_store: flag("supportsStore", true),
        supports_developer_role: flag("supportsDeveloperRole", developer_model || !openrouter),
        supports_reasoning_effort: flag("supportsReasoningEffort", true),
        supports_usage_in_streaming: flag("supportsUsageInStreaming", true),
        supports_finish_reason: flag("supportsFinishReason", true),
        max_tokens_field: text("maxTokensField", "max_completion_tokens"),
        requires_reasoning_content: flag("requiresReasoningContentOnAssistantMessages", false),
        thinking_format: text("thinkingFormat", if openrouter { "openrouter" } else { "openai" }),
        supports_strict_mode: flag("supportsStrictMode", true),
        anthropic_cache_control: text("cacheControlFormat", cache_control) == "anthropic",
    }
}

/// Responses-style `call|item` ids keep item-level uniqueness within Chat Completions' 40-character limit.
fn normalize_tool_call_id(model: &Model, id: &str) -> String {
    if let Some((call, item)) = id.split_once('|') {
        let (call, item) = (safe_id(call), safe_id(item));
        let combined = if item.is_empty() { call.clone() } else { format!("{call}_{item}") };
        if combined.len() <= TOOL_ID_LIMIT {
            return combined;
        }
        let hash: String = short_hash(id).chars().take(8).collect();
        let prefix: String = call.chars().take(TOOL_ID_LIMIT.saturating_sub(hash.len() + 1).max(1)).collect();
        return format!("{prefix}_{hash}");
    }
    if model.provider == "openai" && id.encode_utf16().count() > TOOL_ID_LIMIT {
        return String::from_utf16_lossy(&id.encode_utf16().take(TOOL_ID_LIMIT).collect::<Vec<u16>>());
    }
    id.to_owned()
}

fn reasoning_detail_valid(detail: &Value) -> bool {
    let Some(object) = detail.as_object() else { return false };
    let common = matches!(object.get("id"), None | Some(Value::Null | Value::String(_)))
        && matches!(object.get("format"), None | Some(Value::String(_)))
        && matches!(object.get("index"), None | Some(Value::Number(_)));
    common
        && match object.get("type").and_then(Value::as_str) {
            Some("reasoning.summary") => object.get("summary").is_some_and(Value::is_string),
            Some("reasoning.encrypted") => object.get("data").is_some_and(Value::is_string),
            Some("reasoning.text") => {
                object.get("text").is_some_and(Value::is_string)
                    && matches!(object.get("signature"), None | Some(Value::Null | Value::String(_)))
            }
            _ => false,
        }
}

fn parse_reasoning_details(signature: Option<&str>) -> Option<Vec<Value>> {
    let parsed: Value = serde_json::from_str(signature.filter(|signature| !signature.is_empty())?).ok()?;
    let details = parsed.as_array()?;
    (!details.is_empty() && details.iter().all(reasoning_detail_valid)).then(|| details.clone())
}

fn parse_legacy_encrypted(signature: Option<&str>) -> Option<Value> {
    let parsed: Value = serde_json::from_str(signature.filter(|signature| !signature.is_empty())?).ok()?;
    let valid = reasoning_detail_valid(&parsed)
        && parsed["type"] == "reasoning.encrypted"
        && parsed["id"].as_str().is_some_and(|id| !id.is_empty())
        && parsed["data"].as_str().is_some_and(|data| !data.is_empty());
    valid.then_some(parsed)
}

fn assistant_message(model: &Model, compat: &Compat, assistant: &AssistantMessage) -> Option<Value> {
    let mut message = Map::new();
    message.insert("role".into(), json!("assistant"));
    message.insert("content".into(), Value::Null);
    let text: String = assistant
        .content
        .iter()
        .filter_map(|part| match part {
            AssistantPart::Text { text, .. } if !text.trim().is_empty() => Some(text.as_str()),
            _ => None,
        })
        .collect();
    let thinking: Vec<(&str, Option<&str>)> = assistant
        .content
        .iter()
        .filter_map(|part| match part {
            AssistantPart::Thinking { thinking, thinking_signature, .. } => Some((thinking.as_str(), thinking_signature.as_deref())),
            _ => None,
        })
        .collect();
    let calls: Vec<Value> = assistant
        .content
        .iter()
        .filter_map(|part| match part {
            AssistantPart::ToolCall { id, name, arguments, .. } => {
                Some(json!({"id": id, "type": "function", "function": {"name": name, "arguments": arguments.to_string()}}))
            }
            _ => None,
        })
        .collect();
    let legacy: Vec<Value> = assistant
        .content
        .iter()
        .filter_map(|part| match part {
            AssistantPart::ToolCall { thought_signature, .. } => parse_legacy_encrypted(thought_signature.as_deref()),
            _ => None,
        })
        .collect();
    let preserved =
        thinking.iter().find_map(|(_, signature)| parse_reasoning_details(*signature)).or_else(|| (!legacy.is_empty()).then_some(legacy));
    let non_empty: Vec<&(&str, Option<&str>)> = thinking.iter().filter(|(text, _)| !text.trim().is_empty()).collect();
    if !text.is_empty() {
        message.insert("content".into(), json!(text));
    }
    if let (Some((_, signature)), None) = (non_empty.first(), &preserved)
        && let Some(field) = signature.filter(|signature| REASONING_FIELDS.contains(signature))
    {
        let joined = non_empty.iter().map(|(text, _)| *text).collect::<Vec<_>>().join("\n");
        message.insert(field.to_owned(), json!(joined));
    }
    let has_calls = !calls.is_empty();
    if has_calls {
        message.insert("tool_calls".into(), Value::Array(calls));
    }
    if let Some(details) = preserved {
        message.insert("reasoning_details".into(), Value::Array(details));
    }
    if compat.requires_reasoning_content && model.reasoning && !message.contains_key("reasoning_content") {
        message.insert("reasoning_content".into(), json!(""));
    }
    (truthy(&message["content"]) || has_calls).then_some(Value::Object(message))
}

/// `convertMessages`: the system prompt, user parts, one assistant message per turn, and a tool message per result.
fn convert_messages(model: &Model, context: &LlmContext, compat: &Compat) -> Result<Vec<Value>, String> {
    let normalize = |id: &str, _source: &AssistantMessage| normalize_tool_call_id(model, id);
    let transformed = transform_messages(&context.messages, model, Some(&normalize));
    let mut params = Vec::new();
    if let Some(prompt) = context.system_prompt.as_deref().filter(|prompt| !prompt.is_empty()) {
        let role = if model.reasoning && compat.supports_developer_role { "developer" } else { "system" };
        params.push(json!({"role": role, "content": prompt}));
    }
    let images_allowed = model.input.iter().any(|input| input == "image");
    let mut index = 0;
    while index < transformed.len() {
        match &transformed[index] {
            Message::User(user) => params.extend(user_message(user, UserMessageDialect::Chat)),
            Message::System(note) => {
                params.push(serde_json::to_value(Note { role: "system", content: &note.text }).map_err(|error| error.to_string())?)
            }
            Message::Assistant(assistant) => params.extend(assistant_message(model, compat, assistant)),
            Message::ToolResult(_) => {
                let mut images = Vec::new();
                while let Some(Message::ToolResult(result)) = transformed.get(index) {
                    let (text, has_images) = tool_text(&result.content);
                    let content = if !text.is_empty() {
                        text
                    } else if has_images {
                        "(see attached image)".into()
                    } else {
                        "(no tool output)".into()
                    };
                    params.push(json!({"role": "tool", "content": content, "tool_call_id": result.tool_call_id}));
                    if has_images && images_allowed {
                        images.extend(result.content.iter().filter_map(|part| match part {
                            ToolResultPart::Image { data, mime_type } => Some(chat_image(data, mime_type)),
                            ToolResultPart::Text { .. } => None,
                        }));
                    }
                    index += 1;
                }
                if !images.is_empty() {
                    let mut content = vec![json!({"type": "text", "text": "Attached image(s) from tool result:"})];
                    content.extend(images);
                    params.push(user_content(content));
                }
                continue;
            }
        }
        index += 1;
    }
    Ok(params)
}

fn convert_tools(tools: &[ToolSpec], compat: &Compat) -> Result<Vec<Value>, String> {
    convert_tools_with(tools, compat.supports_strict_mode, json!(false)).map(|items| {
        items
            .into_iter()
            .map(|mut function| {
                drop(function.as_object_mut().and_then(|object| object.remove("type")));
                json!({"type": "function", "function": function})
            })
            .collect()
    })
}

fn add_cache_control(message: &mut Value, control: &Value) -> bool {
    let replacement = match message.get_mut("content") {
        Some(Value::String(text)) if !text.is_empty() => json!([{"type": "text", "text": text.clone(), "cache_control": control}]),
        Some(Value::Array(parts)) => {
            return match parts.iter_mut().rev().find(|part| part["type"] == "text") {
                Some(part) => {
                    part["cache_control"] = control.clone();
                    true
                }
                None => false,
            };
        }
        _ => return false,
    };
    message["content"] = replacement;
    true
}

/// Anthropic models behind OpenRouter cache the system prompt, the last tool and the last conversation text.
fn apply_anthropic_cache_control(messages: &mut [Value], tools: Option<&mut Vec<Value>>) {
    let control = ephemeral();
    if let Some(system) = messages.iter_mut().find(|message| message["role"] == "system" || message["role"] == "developer") {
        add_cache_control(system, &control);
    }
    if let Some(last) = tools.and_then(|tools| tools.last_mut()) {
        last["cache_control"] = control.clone();
    }
    for message in messages.iter_mut().rev() {
        if matches!(message["role"].as_str(), Some("user" | "assistant" | "tool")) && add_cache_control(message, &control) {
            break;
        }
    }
}

fn has_tool_history(messages: &[Message]) -> bool {
    messages.iter().any(|message| match message {
        Message::ToolResult(_) => true,
        Message::Assistant(assistant) => assistant.content.iter().any(|part| matches!(part, AssistantPart::ToolCall { .. })),
        Message::User(_) | Message::System(_) => false,
    })
}

fn reasoning_params(model: &Model, compat: &Compat, effort: Option<ThinkingLevel>, params: &mut Map<String, Value>) {
    if !model.reasoning {
        return;
    }
    let mapped = |level: &str| model.thinking_level_map.as_ref().and_then(|map| map.get(level)).cloned();
    if compat.thinking_format == "openrouter" {
        match effort {
            Some(effort) => {
                let value = mapped(effort.as_str()).filter(|value| !value.is_null()).unwrap_or_else(|| json!(effort.as_str()));
                insert_reasoning_effort(params, value);
            }
            None if mapped("off") != Some(Value::Null) => {
                let off = mapped("off").filter(|value| !value.is_null()).unwrap_or_else(|| json!("none"));
                insert_reasoning_effort(params, off);
            }
            None => {}
        }
    } else if compat.supports_reasoning_effort {
        match effort {
            Some(effort) => {
                let value = mapped(effort.as_str()).filter(|value| !value.is_null()).unwrap_or_else(|| json!(effort.as_str()));
                params.insert("reasoning_effort".into(), value);
            }
            None => {
                if let Some(off) = mapped("off").filter(Value::is_string) {
                    params.insert("reasoning_effort".into(), off);
                }
            }
        }
    }
}

/// `buildParams` for Chat Completions as OpenRouter receives it.
fn build_params(
    model: &Model,
    context: &LlmContext,
    compat: &Compat,
    max_tokens: u64,
    effort: Option<ThinkingLevel>,
) -> Result<Value, String> {
    let mut messages = convert_messages(model, context, compat)?;
    let mut tools = if !context.tools.is_empty() {
        Some(convert_tools(&context.tools, compat)?)
    } else if has_tool_history(&context.messages) {
        Some(Vec::new())
    } else {
        None
    };
    if compat.anthropic_cache_control {
        apply_anthropic_cache_control(&mut messages, tools.as_mut());
    }
    let mut params = model_params(model);
    params.insert("messages".into(), Value::Array(messages));
    params.insert("stream".into(), json!(true));
    if compat.supports_usage_in_streaming {
        params.insert("stream_options".into(), json!({"include_usage": true}));
    }
    if compat.supports_store {
        params.insert("store".into(), json!(false));
    }
    if max_tokens > 0 {
        params.insert(compat.max_tokens_field.clone(), json!(max_tokens));
    }
    if let Some(tools) = tools {
        params.insert("tools".into(), Value::Array(tools));
    }
    reasoning_params(model, compat, effort, &mut params);
    if let Some(routing) = model.compat.as_ref().and_then(|compat| compat.get("openRouterRouting")).filter(|routing| truthy(routing)) {
        params.insert("provider".into(), routing.clone());
    }
    Ok(Value::Object(params))
}

fn map_finish_reason(reason: &str) -> (StopReason, Option<String>) {
    match reason {
        "stop" | "end" => (StopReason::Stop, None),
        "length" => (StopReason::Length, None),
        "function_call" | "tool_calls" => (StopReason::ToolUse, None),
        other => (StopReason::Error, Some(format!("Provider finish_reason: {other}"))),
    }
}

/// `parseChunkUsage`: cached tokens from OpenRouter's details, DeepSeek's hit count or Kimi's top-level count.
fn parse_usage(raw: &Value, model: &Model) -> Usage {
    let prompt = count(&raw["prompt_tokens"]);
    let cache_read = raw["prompt_tokens_details"]["cached_tokens"]
        .as_u64()
        .or_else(|| raw["prompt_cache_hit_tokens"].as_u64())
        .or_else(|| raw["cached_tokens"].as_u64())
        .unwrap_or(0);
    let cache_write = count(&raw["prompt_tokens_details"]["cache_write_tokens"]);
    let input = prompt.saturating_sub(cache_read).saturating_sub(cache_write);
    let output = count(&raw["completion_tokens"]);
    let mut usage = Usage {
        input,
        output,
        cache_read,
        cache_write,
        cache_write_1h: None,
        reasoning: Some(count(&raw["completion_tokens_details"]["reasoning_tokens"])),
        total_tokens: input + output + cache_read + cache_write,
        cost: Cost::ZERO,
    };
    calculate_cost(&model.cost, &mut usage);
    let fixed = model.compat.as_ref().and_then(|compat| compat.get("fixedPricing")).is_some_and(truthy);
    if let Some(charged) = charged(raw).filter(|_| !fixed) {
        usage.cost = charged;
    }
    usage
}

/// OpenRouter's own charge for a response, split into prompt and completion; BYOK adds the provider's.
fn charged(raw: &Value) -> Option<Cost> {
    let details = &raw["cost_details"];
    let upstream = if raw["is_byok"] == true { details["upstream_inference_cost"].as_f64().unwrap_or(0.0) } else { 0.0 };
    let total = raw["cost"].as_f64()? + upstream;
    let output = details["upstream_inference_completions_cost"].as_f64().unwrap_or(0.0).min(total);
    Some(Cost { input: total - output, output, cache_read: 0.0, cache_write: 0.0, total })
}

/// The chunk loop of `openai-completions.js`: blocks open lazily and all end together after the stream.
struct Parser {
    output: AssistantMessage,
    text: Option<usize>,
    thinking: Option<usize>,
    by_index: HashMap<u64, usize>,
    by_id: HashMap<String, usize>,
    stream_index_of: HashMap<usize, u64>,
    partial_args: HashMap<usize, String>,
    has_finish_reason: bool,
    signature_holds_details: bool,
}

impl Parser {
    fn new(output: AssistantMessage) -> Parser {
        Parser {
            output,
            text: None,
            thinking: None,
            by_index: HashMap::new(),
            by_id: HashMap::new(),
            stream_index_of: HashMap::new(),
            partial_args: HashMap::new(),
            has_finish_reason: false,
            signature_holds_details: false,
        }
    }

    /// The text part's index, or with `signature` the thinking part's, creating the part on first use.
    fn ensure_part(&mut self, signature: Option<&str>, sink: &EventSink) -> usize {
        let slot = if signature.is_some() { &mut self.thinking } else { &mut self.text };
        if let Some(index) = *slot {
            return index;
        }
        let index = self.output.content.len();
        *slot = Some(index);
        self.output.content.push(match signature {
            None => AssistantPart::Text { text: String::new(), text_signature: None },
            _ => AssistantPart::Thinking { thinking: String::new(), thinking_signature: signature.map(str::to_owned), redacted: None },
        });
        push(&self.output, sink, |partial| match signature {
            None => StreamEvent::TextStart { content_index: index, partial },
            _ => StreamEvent::ThinkingStart { content_index: index, partial },
        });
        index
    }

    fn ensure_tool_call(&mut self, call: &Value, sink: &EventSink) -> usize {
        let stream_index = call["index"].as_u64();
        let id = call["id"].as_str().filter(|id| !id.is_empty());
        let name = call["function"]["name"].as_str().or_else(|| call["custom"]["name"].as_str()).unwrap_or("");
        let existing =
            stream_index.and_then(|index| self.by_index.get(&index).copied()).or_else(|| id.and_then(|id| self.by_id.get(id).copied()));
        let index = match existing {
            Some(index) => index,
            None => {
                let index = self.output.content.len();
                self.output.content.push(AssistantPart::ToolCall {
                    id: id.unwrap_or_default().to_owned(),
                    name: name.to_owned(),
                    arguments: Value::Object(Map::new()),
                    thought_signature: None,
                });
                self.partial_args.insert(index, String::new());
                push(&self.output, sink, |partial| StreamEvent::ToolcallStart { content_index: index, partial });
                index
            }
        };
        if let Some(stream_index) = stream_index.filter(|_| !self.stream_index_of.contains_key(&index)) {
            self.stream_index_of.insert(index, stream_index);
            self.by_index.insert(stream_index, index);
        }
        if let Some(id) = id {
            self.by_id.insert(id.to_owned(), index);
        }
        if let Some(AssistantPart::ToolCall { id: block_id, name: block_name, .. }) = self.output.content.get_mut(index) {
            if block_id.is_empty()
                && let Some(id) = id
            {
                *block_id = id.to_owned();
            }
            if block_name.is_empty() && !name.is_empty() {
                *block_name = name.to_owned();
            }
        }
        index
    }

    fn handle(&mut self, chunk: &Value, model: &Model, sink: &EventSink) {
        if !chunk.is_object() {
            return;
        }
        if !self.output.extra.get("responseId").is_some_and(truthy) && !chunk["id"].is_null() {
            self.output.extra.insert("responseId".into(), chunk["id"].clone());
        }
        if let Some(name) = chunk["model"].as_str().filter(|name| !name.is_empty() && *name != model.id)
            && !self.output.extra.get("responseModel").is_some_and(truthy)
        {
            self.output.extra.insert("responseModel".into(), json!(name));
        }
        if truthy(&chunk["usage"]) {
            self.output.usage = parse_usage(&chunk["usage"], model);
        }
        let Some(choice) = chunk["choices"].as_array().and_then(|choices| choices.first()) else { return };
        if !truthy(&chunk["usage"]) && truthy(&choice["usage"]) {
            self.output.usage = parse_usage(&choice["usage"], model);
        }
        if let Some(reason) = choice["finish_reason"].as_str().filter(|reason| !reason.is_empty()) {
            set_stop_reason(&mut self.output, Some(reason), Ok(map_finish_reason(reason)), false).ok();
            self.has_finish_reason = true;
        }
        let delta = &choice["delta"];
        if !delta.is_object() {
            return;
        }
        if let Some(content) = delta["content"].as_str().filter(|content| !content.is_empty()) {
            let index = self.ensure_part(None, sink);
            if let Some(AssistantPart::Text { text, .. }) = self.output.content.get_mut(index) {
                text.push_str(content);
            }
            push(&self.output, sink, |partial| StreamEvent::TextDelta { content_index: index, delta: content.to_owned(), partial });
        }
        if let Some((field, reasoning)) =
            REASONING_FIELDS.iter().find_map(|field| delta[*field].as_str().filter(|text| !text.is_empty()).map(|text| (*field, text)))
        {
            let index = self.ensure_part(Some(field), sink);
            if let Some(AssistantPart::Thinking { thinking, .. }) = self.output.content.get_mut(index) {
                thinking.push_str(reasoning);
            }
            push(&self.output, sink, |partial| StreamEvent::ThinkingDelta { content_index: index, delta: reasoning.to_owned(), partial });
        }
        for call in delta["tool_calls"].as_array().into_iter().flatten() {
            let index = self.ensure_tool_call(call, sink);
            let arguments = call["function"]["arguments"].as_str().filter(|arguments| !arguments.is_empty()).unwrap_or("");
            if !arguments.is_empty() {
                let partial = self.partial_args.entry(index).or_default();
                partial.push_str(arguments);
                let parsed = parse_streaming_json(partial);
                if let Some(AssistantPart::ToolCall { arguments: slot, .. }) = self.output.content.get_mut(index) {
                    *slot = parsed;
                }
            }
            toolcall_delta(&self.output, sink, index, arguments);
        }
        for detail in delta["reasoning_details"].as_array().into_iter().flatten().filter(|detail| reasoning_detail_valid(detail)) {
            let index = self.ensure_part(Some(""), sink);
            if let Some(AssistantPart::Thinking { thinking_signature, .. }) = self.output.content.get_mut(index) {
                let signature = thinking_signature.get_or_insert_with(String::new);
                if self.signature_holds_details && signature.pop() == Some(']') {
                    signature.push(',');
                } else {
                    signature.clear();
                    signature.push('[');
                }
                signature.push_str(&detail.to_string());
                signature.push(']');
                self.signature_holds_details = true;
            }
        }
    }

    fn finish_blocks(&mut self, sink: &EventSink) {
        for index in 0..self.output.content.len() {
            if let Some(partial) = self.partial_args.remove(&index)
                && let Some(AssistantPart::ToolCall { arguments, .. }) = self.output.content.get_mut(index)
            {
                *arguments = parse_streaming_json(&partial);
            }
            let partial = self.output.clone();
            sink.push(match &self.output.content[index] {
                AssistantPart::Text { text, .. } => StreamEvent::TextEnd { content_index: index, content: text.clone(), partial },
                AssistantPart::Thinking { thinking, .. } => {
                    StreamEvent::ThinkingEnd { content_index: index, content: thinking.clone(), partial }
                }
                call @ AssistantPart::ToolCall { .. } => {
                    StreamEvent::ToolcallEnd { content_index: index, tool_call: call.clone(), partial }
                }
            });
        }
    }
}

fn completion_error(error: ProviderError) -> String {
    let text = error.format(None);
    match error.raw_metadata {
        Some(raw) if !text.contains(&raw) => format!("{text}\n{raw}"),
        _ => text,
    }
}

pub fn stream_simple(model: &Model, context: LlmContext, options: StreamOptions) -> Result<EventStream, Error> {
    client_api_key(model, &options, &[])?;
    let max_tokens = clamp_max_tokens(model, &context, model.max_tokens);
    let effort = stream_effort(model, &options);
    spawn_stream!(
        model,
        options,
        parser = Parser::new(pending_message(&model)),
        parser.output,
        request(&context, &options, max_tokens, effort)
    )
}

async fn request(
    model: &Model,
    context: &LlmContext,
    options: &StreamOptions,
    max_tokens: u64,
    effort: Option<ThinkingLevel>,
    parser: &mut Parser,
    sink: &EventSink,
) -> Result<(), String> {
    let compat = compat(model);
    let params = build_params(model, context, &compat, max_tokens, effort)?;
    let mut headers = vec![user_agent_header()];
    let model_headers = model_headers(model);
    headers = merge_headers(&headers, &model_headers);
    headers = merge_headers(&headers, &options.headers);
    let key = options.api_key.clone().unwrap_or_else(|| "unused".into());
    let mut builder = client().post(format!("{}/chat/completions", model.base_url.trim_end_matches('/')));
    for (name, value) in merge_headers(&[("Authorization".to_owned(), format!("Bearer {key}"))], &headers) {
        builder = builder.header(name, value);
    }
    builder = builder.header("Content-Type", "application/json").header("Accept", "application/json").body(params.to_string());
    let response = send_request(builder, options).await?;
    let status = response.status().as_u16();
    if !(200..300).contains(&status) {
        let body = response_text(response).await;
        if crate::accounts::credit_error(status, &body) {
            crate::accounts::mark_out_of_credit(&mut parser.output);
        }
        return Err(completion_error(status_error(status, &body)));
    }
    sink.push(StreamEvent::Start { partial: parser.output.clone() });
    let outcome = read_sdk_stream(response, &options.cancel, |chunk| {
        parser.handle(&chunk, model, sink);
        Ok(())
    })
    .await;
    match outcome {
        Err(StreamFailure::Api(error)) => return Err(completion_error(error)),
        Err(StreamFailure::Other(message)) => return Err(message),
        Ok(()) => {}
    }
    parser.finish_blocks(sink);
    check_cancelled(options)?;
    if !parser.has_finish_reason && !compat.supports_finish_reason {
        let tool = parser.output.content.iter().any(|part| matches!(part, AssistantPart::ToolCall { .. }));
        parser.output.stop_reason = if tool { StopReason::ToolUse } else { StopReason::Stop };
    }
    if parser.output.stop_reason == StopReason::Error {
        return Err(parser.output.error_message.clone().unwrap_or_else(|| "Provider returned an error stop reason".into()));
    }
    if (compat.supports_finish_reason && !parser.has_finish_reason) || parser.output.stop_reason == StopReason::Pending {
        return Err("Stream ended without finish_reason".into());
    }
    Ok(())
}
