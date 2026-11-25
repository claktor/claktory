pub mod auth;
mod binding;
pub mod files;
pub mod oauth;
#[cfg(any(test, feature = "qa-provider"))]
pub mod qa;

use futures_util::StreamExt;
use hashbrown::HashMap;
use serde::Serialize;
use serde_json::{Map, Value, json};

use crate::errors::Error;
use crate::json_parse::{parse_json_with_repair, parse_streaming_json};
use crate::providers::anthropic::auth::AnthropicApiKeyAuth;
use crate::providers::anthropic::oauth::AnthropicOAuth;
use crate::providers::openai_responses::{make_strict_json_schema, safe_id, strict_sampling, transform_messages};
use crate::providers::sdk::{api_error_message, send_request, truthy};
use crate::providers::{
    Provider, api_key, check_cancelled, clamp_max_tokens, client, client_api_key, count, ephemeral, merge_headers, model_headers,
    model_params, oauth, response_text, set_response_id, set_stop_reason, tool_text, user_agent, user_content,
};
use crate::sse::{SseDecoder, SseMessage};
use crate::stream::{CacheTtl, EventSink, EventStream, LlmContext, StreamOptions, ThinkingLevel, pending_message};
use crate::tools::ToolSpec;
use crate::types::{
    AssistantMessage, AssistantPart, Message, Model, ModelCost, StopReason, StreamEvent, ToolResultPart, UserContent, UserPart,
};
use crate::usage::calculate_cost;

pub(crate) const CLAUDE_CODE_VERSION: &str = "2.1.327";
const CLAUDE_CODE_TOOLS: [&str; 17] = [
    "Read",
    "Write",
    "Edit",
    "Bash",
    "Grep",
    "Glob",
    "AskUserQuestion",
    "EnterPlanMode",
    "ExitPlanMode",
    "KillShell",
    "NotebookEdit",
    "Skill",
    "Task",
    "TaskOutput",
    "TodoWrite",
    "WebFetch",
    "WebSearch",
];
const CLAUDE_CODE_IDENTITY: &str = "You are Claude Code, Anthropic's official CLI for Claude.";
const FINE_GRAINED_TOOL_STREAMING_BETA: &str = "fine-grained-tool-streaming-2025-05-14";
const INTERLEAVED_THINKING_BETA: &str = "interleaved-thinking-2025-05-14";
const SERVER_SIDE_FALLBACK_BETA: &str = "server-side-fallback-2026-07-01";
const CLAUDE_CODE_BETAS: [&str; 9] = [
    "claude-code-20250219",
    "interleaved-thinking-2025-05-14",
    "thinking-token-count-2026-05-13",
    "context-management-2025-06-27",
    "prompt-caching-scope-2026-01-05",
    "per-turn-control-2026-07-01",
    "mid-conversation-tool-changes-2026-07-01",
    "effort-2025-11-24",
    "thinking-display-updates-2026-08-18",
];
const MESSAGE_EVENTS: [&str; 6] =
    ["message_start", "message_delta", "message_stop", "content_block_start", "content_block_delta", "content_block_stop"];
const MIN_ANSWER_TOKENS: u64 = 1024;
const STRICT_NUMBER_BOUNDS: [(&str, &str); 5] = [
    ("minimum", "Minimum"),
    ("exclusiveMinimum", "Greater than"),
    ("maximum", "Maximum"),
    ("exclusiveMaximum", "Less than"),
    ("multipleOf", "Multiple of"),
];

pub fn provider() -> Provider {
    let mut provider = Provider::new(
        "anthropic",
        "Anthropic",
        "https://api.anthropic.com",
        api_key(AnthropicApiKeyAuth),
        oauth(AnthropicOAuth::new()),
        stream_simple,
    );
    provider.limits = Some(crate::limits::anthropic);
    provider.plan = Some(crate::limits::anthropic_plan);
    provider.identity = Some(crate::limits::anthropic_identity);
    provider.reset = Some(crate::limits::anthropic_reset);
    provider.check_key = Some(crate::providers::keys::anthropic);
    provider
}

fn compat_flag(model: &Model, key: &str, default: bool) -> bool {
    model.compat.as_ref().and_then(|compat| compat.get(key)).and_then(Value::as_bool).unwrap_or(default)
}

fn fallback_models(model: &Model) -> Vec<Value> {
    model.compat.as_ref().and_then(|compat| compat.get("allowedFallbackModels")).and_then(Value::as_array).cloned().unwrap_or_default()
}

/// Whether the model supports Anthropic's server-side fallback at all
/// (`serverSideFallback`): with no named list, `fallbacks: "default"` asks
/// for Anthropic's own per-category recommendation.
fn server_side_fallback(model: &Model) -> bool {
    compat_flag(model, "serverSideFallback", false) || !fallback_models(model).is_empty()
}

fn to_claude_code_name(name: &str) -> String {
    CLAUDE_CODE_TOOLS.iter().find(|tool| tool.eq_ignore_ascii_case(name)).map_or_else(|| name.to_owned(), |tool| (*tool).to_owned())
}

fn from_claude_code_name(name: &str, tools: &[ToolSpec]) -> String {
    tools.iter().find(|tool| tool.name.eq_ignore_ascii_case(name)).map_or_else(|| name.to_owned(), |tool| tool.name.to_string())
}

/// Anthropic tool ids allow `[a-zA-Z0-9_-]` up to 64 characters.
fn normalize_tool_call_id(id: &str) -> String {
    safe_id(id).chars().take(64).collect()
}

enum Thinking {
    Disabled,
    Adaptive(String),
    Budget(u64),
}

/// `mapThinkingLevelToEffort`: the model's own mapping, else Pi's level names.
fn effort_for(model: &Model, level: ThinkingLevel) -> String {
    if let Some(mapped) = model.thinking_level_map.as_ref().and_then(|map| map.get(level.as_str())).and_then(Value::as_str) {
        return mapped.to_owned();
    }
    match level {
        ThinkingLevel::Minimal | ThinkingLevel::Low => "low",
        ThinkingLevel::Medium => "medium",
        _ => "high",
    }
    .to_owned()
}

fn budget_for(level: ThinkingLevel) -> u64 {
    match level {
        ThinkingLevel::Minimal => 1024,
        ThinkingLevel::Low => 2048,
        ThinkingLevel::Medium => 8192,
        ThinkingLevel::Off => 0,
        ThinkingLevel::High | ThinkingLevel::Xhigh | ThinkingLevel::Max => 16384,
    }
}

fn text_block(text: &str) -> Value {
    json!({"type": "text", "text": text})
}

fn image_block(data: &str, mime_type: &str) -> Value {
    json!({"type": "image", "source": {"type": "base64", "media_type": mime_type, "data": data}})
}

/// The file id each image already at Anthropic goes as, by digest (#382,
/// `files`); the others go inline.
type FileRefs = HashMap<String, String>;

fn file_block(file_id: &str) -> Value {
    json!({"type": "image", "source": {"type": "file", "file_id": file_id}})
}

fn convert_content_blocks(content: &[ToolResultPart], refs: &FileRefs) -> Value {
    if !content.iter().any(|part| matches!(part, ToolResultPart::Image { .. })) {
        let (text, _) = tool_text(content);
        return json!(text);
    }
    let mut blocks: Vec<Value> = content
        .iter()
        .map(|part| match part {
            ToolResultPart::Text { text } => text_block(text),
            ToolResultPart::Image { data, mime_type, digest } => match digest.as_ref().and_then(|digest| refs.get(digest)) {
                Some(file_id) => file_block(file_id),
                None => image_block(data, mime_type),
            },
        })
        .collect();
    if !blocks.iter().any(|block| block["type"] == "text") {
        blocks.insert(0, text_block("(see attached image)"));
    }
    Value::Array(blocks)
}

fn assistant_blocks(content: &[AssistantPart], oauth: bool, allow_empty_signature: bool) -> Vec<Value> {
    let mut blocks = Vec::new();
    for part in content {
        match part {
            AssistantPart::Text { text, .. } => {
                if !text.trim().is_empty() {
                    blocks.push(text_block(text));
                }
            }
            AssistantPart::Thinking { thinking, thinking_signature, redacted } => {
                if redacted == &Some(true) {
                    blocks.push(json!({"type": "redacted_thinking", "data": thinking_signature}));
                    continue;
                }
                let signature = thinking_signature.as_deref().filter(|signature| !signature.trim().is_empty());
                if thinking.trim().is_empty() && signature.is_none() {
                    continue;
                }
                blocks.push(match signature {
                    Some(signature) => json!({"type": "thinking", "thinking": thinking, "signature": signature}),
                    None if allow_empty_signature => json!({"type": "thinking", "thinking": thinking, "signature": ""}),
                    None => text_block(thinking),
                });
            }
            AssistantPart::ToolCall { id, name, arguments, .. } => {
                let name = if oauth { to_claude_code_name(name) } else { name.clone() };
                let input = if arguments.is_null() { json!({}) } else { arguments.clone() };
                blocks.push(json!({"type": "tool_use", "id": id, "name": name, "input": input}));
            }
        }
    }
    blocks
}

#[derive(Serialize)]
struct Note<'a> {
    role: &'static str,
    content: [NoteText<'a>; 1],
}

#[derive(Serialize)]
struct NoteText<'a> {
    r#type: &'static str,
    text: &'a str,
}

#[derive(Serialize)]
struct AdaptiveThinking<'a> {
    r#type: &'static str,
    display: &'a str,
}

#[derive(Serialize)]
struct ThinkingContext {
    edits: [ThinkingEdit; 1],
}

#[derive(Serialize)]
struct ThinkingEdit {
    r#type: &'static str,
    keep: &'static str,
}

fn convert_messages(messages: &[Message], oauth: bool, allow_empty_signature: bool, refs: &FileRefs) -> Result<Vec<Value>, String> {
    let mut params = Vec::new();
    let mut index = 0;
    while index < messages.len() {
        match &messages[index] {
            Message::User(user) => match &user.content {
                UserContent::Text(text) => {
                    if !text.trim().is_empty() {
                        params.push(user_content(text));
                    }
                }
                UserContent::Parts(parts) => {
                    let blocks: Vec<Value> = parts
                        .iter()
                        .filter_map(|part| match part {
                            UserPart::Text { text } => (!text.trim().is_empty()).then(|| text_block(text)),
                            UserPart::Image { data, mime_type } => Some(image_block(data, mime_type)),
                        })
                        .collect();
                    if !blocks.is_empty() {
                        params.push(user_content(blocks));
                    }
                }
            },
            Message::Assistant(assistant) => {
                let blocks = assistant_blocks(&assistant.content, oauth, allow_empty_signature);
                if !blocks.is_empty() {
                    params.push(json!({"role": "assistant", "content": blocks}));
                }
            }
            Message::System(note) => params.push(
                serde_json::to_value(Note { role: "system", content: [NoteText { r#type: "text", text: &note.text }] })
                    .map_err(|error| error.to_string())?,
            ),
            Message::ToolResult(_) => {
                let mut results = Vec::new();
                while let Some(Message::ToolResult(result)) = messages.get(index) {
                    results.push(json!({"type": "tool_result", "tool_use_id": result.tool_call_id, "content": convert_content_blocks(&result.content, refs), "is_error": result.is_error}));
                    index += 1;
                }
                params.push(user_content(results));
                continue;
            }
        }
        index += 1;
    }
    if let Some(last) = params.last_mut().filter(|last| last["role"] == "user" || last["role"] == "system") {
        let control = ephemeral();
        match &mut last["content"] {
            Value::Array(blocks) => {
                if let Some(block) =
                    blocks.last_mut().filter(|block| matches!(block["type"].as_str(), Some("text" | "image" | "tool_result")))
                {
                    block["cache_control"] = control;
                }
            }
            Value::String(text) => {
                let text = text.clone();
                last["content"] = json!([{"type": "text", "text": text, "cache_control": control}]);
            }
            _ => {}
        }
    }
    Ok(params)
}

/// Anthropic strict tools reject numeric bounds, so each moves into its schema's description.
fn describe_bounds(schema: &mut Value) {
    let Some(object) = schema.as_object_mut() else { return };
    let notes: Vec<String> =
        STRICT_NUMBER_BOUNDS.iter().filter_map(|(key, label)| object.shift_remove(*key).map(|bound| format!("{label} {bound}."))).collect();
    if !notes.is_empty() {
        let description = object.get("description").and_then(Value::as_str).map(str::to_owned).into_iter().chain(notes).collect::<Vec<_>>();
        object.insert("description".into(), Value::String(description.join(" ")));
    }
    if let Some(Value::Object(properties)) = object.get_mut("properties") {
        properties.values_mut().for_each(describe_bounds);
    }
    if let Some(Value::Array(variants)) = object.get_mut("anyOf") {
        variants.iter_mut().for_each(describe_bounds);
    }
    if let Some(items) = object.get_mut("items") {
        describe_bounds(items);
    }
}

fn convert_tools(tools: &[ToolSpec], oauth: bool, model: &Model) -> Result<Vec<Value>, String> {
    let eager = compat_flag(model, "supportsEagerToolInputStreaming", true);
    let strict_tools = compat_flag(model, "supportsStrictTools", false);
    let cache_on_tools = compat_flag(model, "supportsCacheControlOnTools", true);
    let last = tools.len().saturating_sub(1);
    tools
        .iter()
        .enumerate()
        .map(|(index, tool)| {
            let strict = strict_sampling(tool, strict_tools)? == Some(true);
            let mut parameters = if strict { make_strict_json_schema(&tool.parameters)? } else { tool.parameters.clone() };
            if strict {
                describe_bounds(&mut parameters);
            }
            let properties = parameters.get("properties").filter(|value| !value.is_null()).cloned().unwrap_or_else(|| json!({}));
            let required = parameters.get("required").filter(|value| !value.is_null()).cloned().unwrap_or_else(|| json!([]));
            let mut schema = if strict { parameters.as_object().cloned().unwrap_or_default() } else { Map::new() };
            schema.insert("type".into(), json!("object"));
            schema.insert("properties".into(), properties);
            schema.insert("required".into(), required);
            let mut item = Map::new();
            item.insert("name".into(), json!(if oauth { to_claude_code_name(&tool.name) } else { tool.name.to_string() }));
            item.insert("description".into(), json!(tool.description));
            if eager {
                item.insert("eager_input_streaming".into(), json!(true));
            }
            if strict {
                item.insert("strict".into(), json!(true));
            }
            item.insert("input_schema".into(), Value::Object(schema));
            if cache_on_tools && index == last {
                item.insert("cache_control".into(), ephemeral());
            }
            Ok(Value::Object(item))
        })
        .collect()
}

/// Every cache marker in `params` asks for `ttl`; all alike, since a longer
/// TTL may not follow a shorter one.
fn set_cache_ttl(params: &mut Value, ttl: CacheTtl) {
    match params {
        Value::Object(object) => {
            for (key, value) in object.iter_mut() {
                if key == "cache_control" {
                    if let Value::Object(control) = value {
                        control.insert("ttl".into(), json!(ttl.as_str()));
                    }
                } else {
                    set_cache_ttl(value, ttl);
                }
            }
        }
        Value::Array(items) => items.iter_mut().for_each(|item| set_cache_ttl(item, ttl)),
        _ => {}
    }
}

/// `buildParams` with the default short cache retention, and what its
/// thinking is bound to (`binding`). `unbound`: every earlier thinking block
/// is left out, not only those made under another system prompt or tools.
fn build_params(
    model: &Model,
    context: &LlmContext,
    oauth: bool,
    max_tokens: u64,
    thinking: &Thinking,
    refs: &FileRefs,
    unbound: bool,
) -> Result<(Value, String), String> {
    let control = ephemeral();
    let mut params = model_params(model);
    params.insert("max_tokens".into(), json!(max_tokens));
    params.insert("stream".into(), json!(true));
    let prompt = context.system_prompt.as_deref().filter(|prompt| !prompt.is_empty());
    let mut system = Vec::new();
    if oauth {
        system.push(json!({"type": "text", "text": CLAUDE_CODE_IDENTITY, "cache_control": control}));
    }
    if let Some(prompt) = prompt {
        system.push(json!({"type": "text", "text": prompt, "cache_control": control}));
    }
    if !system.is_empty() {
        params.insert("system".into(), Value::Array(system));
    }
    if !context.tools.is_empty() {
        params.insert("tools".into(), Value::Array(convert_tools(&context.tools, oauth, model)?));
    }
    let bound = binding::digest(params.get("system"), params.get("tools"));
    let normalize = |id: &str, _source: &AssistantMessage| normalize_tool_call_id(id);
    let mut transformed = transform_messages(&context.messages, model, Some(&normalize));
    binding::leave_out(&mut transformed, model, &bound, unbound);
    params.insert(
        "messages".into(),
        Value::Array(convert_messages(&transformed, oauth, compat_flag(model, "allowEmptySignature", false), refs)?),
    );
    if model.reasoning {
        match thinking {
            Thinking::Adaptive(effort) => {
                let display = if compat_flag(model, "claudeCodeRequest", false) { "updates" } else { "summarized" };
                params.insert(
                    "thinking".into(),
                    serde_json::to_value(AdaptiveThinking { r#type: "adaptive", display }).map_err(|error| error.to_string())?,
                );
                params.insert("output_config".into(), json!({"effort": effort}));
            }
            Thinking::Budget(budget) => {
                let budget = if *budget == 0 { 1024 } else { *budget };
                params.insert("thinking".into(), json!({"type": "enabled", "budget_tokens": budget, "display": "summarized"}));
            }
            Thinking::Disabled => {
                if model.thinking_level_map.as_ref().and_then(|map| map.get("off")) != Some(&Value::Null) {
                    params.insert("thinking".into(), json!({"type": "disabled"}));
                }
            }
        }
    }
    if compat_flag(model, "claudeCodeRequest", false) {
        params.insert(
            "context_management".into(),
            serde_json::to_value(ThinkingContext { edits: [ThinkingEdit { r#type: "clear_thinking_20251015", keep: "all" }] })
                .map_err(|error| error.to_string())?,
        );
    }
    let fallbacks = fallback_models(model);
    if !fallbacks.is_empty() {
        params.insert("fallbacks".into(), Value::Array(fallbacks.iter().map(|fallback| json!({"model": fallback["model"]})).collect()));
    } else if server_side_fallback(model) {
        // Anthropic's per-category recommendation; a category with none, or
        // a rate-limited fallback model, returns the refusal as it is.
        params.insert("fallbacks".into(), json!("default"));
    }
    Ok((Value::Object(params), bound))
}

/// The client headers: Claude Code identity for an OAuth token, the API key otherwise; caller headers win.
fn request_headers(model: &Model, context: &LlmContext, options: &StreamOptions, oauth: bool) -> Vec<(String, String)> {
    let mut betas = Vec::new();
    if !context.tools.is_empty() && !compat_flag(model, "supportsEagerToolInputStreaming", true) {
        betas.push(FINE_GRAINED_TOOL_STREAMING_BETA);
    }
    if !compat_flag(model, "forceAdaptiveThinking", false) {
        betas.push(INTERLEAVED_THINKING_BETA);
    }
    if server_side_fallback(model) {
        betas.push(SERVER_SIDE_FALLBACK_BETA);
    }
    if compat_flag(model, "claudeCodeRequest", false) {
        for beta in CLAUDE_CODE_BETAS {
            if !betas.contains(&beta) {
                betas.push(beta);
            }
        }
    }
    let pair = |name: &str, value: &str| (name.to_owned(), value.to_owned());
    let mut headers = vec![pair("anthropic-version", "2023-06-01")];
    match (&options.api_key, oauth) {
        (Some(token), true) => headers.push(pair("Authorization", &format!("Bearer {token}"))),
        (Some(key), false) => headers.push(pair("x-api-key", key)),
        (None, _) => {}
    }
    let mut client_headers = vec![
        pair("User-Agent", user_agent()),
        pair("accept", "application/json"),
        pair("anthropic-dangerous-direct-browser-access", "true"),
    ];
    if oauth {
        let mut all = vec!["claude-code-20250219", "oauth-2025-04-20"];
        all.extend(betas.into_iter().filter(|beta| *beta != "claude-code-20250219"));
        client_headers.push(pair("anthropic-beta", &all.join(",")));
        client_headers.push(pair("user-agent", &format!("claude-cli/{CLAUDE_CODE_VERSION}")));
        client_headers.push(pair("x-app", "cli"));
    } else if !betas.is_empty() {
        client_headers.push(pair("anthropic-beta", &betas.join(",")));
    }
    let model_headers = model_headers(model);
    for extra in [&client_headers, &model_headers, &options.headers] {
        headers = merge_headers(&headers, extra);
    }
    headers
}

pub fn stream_simple(model: &Model, context: LlmContext, options: StreamOptions) -> Result<EventStream, Error> {
    client_api_key(model, &options, &["x-api-key"])?;
    let base_max = clamp_max_tokens(model, &context, model.max_tokens);
    let (max_tokens, thinking) = match options.reasoning {
        None => (base_max, Thinking::Disabled),
        Some(level) if compat_flag(model, "forceAdaptiveThinking", false) => (base_max, Thinking::Adaptive(effort_for(model, level))),
        Some(level) => {
            let mut budget = budget_for(level);
            let adjusted = (base_max + budget).min(model.max_tokens);
            if adjusted <= budget {
                budget = budget.min(adjusted.saturating_sub(MIN_ANSWER_TOKENS));
            }
            let max_tokens = clamp_max_tokens(model, &context, adjusted);
            (max_tokens, Thinking::Budget(budget.min(max_tokens.saturating_sub(MIN_ANSWER_TOKENS))))
        }
    };
    spawn_stream!(
        model,
        options,
        parser = Parser::new(pending_message(&model), &model),
        parser.output,
        request(&context, &options, max_tokens, &thinking)
    )
}

fn map_stop_reason(reason: &str, details: &Value) -> Result<(StopReason, Option<String>), String> {
    Ok(match reason {
        "end_turn" | "pause_turn" | "stop_sequence" => (StopReason::Stop, None),
        "max_tokens" => (StopReason::Length, None),
        "tool_use" => (StopReason::ToolUse, None),
        // A refusal stays StopReason::Error on the wire and on disk (an
        // older backend reads the conversation fine after a rollback); its
        // details ride in `extra` (`crate::refusal`), set by the caller.
        "refusal" => {
            let explanation =
                details["explanation"].as_str().filter(|text| !text.is_empty()).unwrap_or("The model refused to complete the request");
            (StopReason::Error, Some(explanation.to_owned()))
        }
        "sensitive" => (StopReason::Error, Some("Provider stopped with: sensitive".into())),
        other => return Err(format!("Unhandled stop reason: {other}")),
    })
}

fn update_count(count: &mut u64, value: &Value) {
    if let Some(value) = value.as_u64() {
        *count = value;
    }
}

fn owned_text(value: &Value, key: &str) -> String {
    value[key].as_str().unwrap_or_default().to_owned()
}

/// The message events of `anthropic-messages.js`; a block is addressed by its stream index until it stops.
struct Parser {
    output: AssistantMessage,
    stream_index: Vec<Option<u64>>,
    partial_json: HashMap<usize, String>,
    cost: ModelCost,
    fallbacks: Vec<Value>,
    saw_start: bool,
    saw_stop: bool,
}

impl Parser {
    fn new(output: AssistantMessage, model: &Model) -> Parser {
        Parser {
            output,
            stream_index: Vec::new(),
            partial_json: HashMap::new(),
            cost: model.cost.clone(),
            fallbacks: fallback_models(model),
            saw_start: false,
            saw_stop: false,
        }
    }

    fn position(&self, index: &Value) -> Option<usize> {
        let index = index.as_u64()?;
        self.stream_index.iter().position(|candidate| *candidate == Some(index))
    }

    /// Prices the message as `served`, another model than was asked for
    /// (the server's fallback answered, or an alias resolved): the named
    /// fallback entry's cost, or the catalog's for that model.
    fn reprice_as(&mut self, served: &str) {
        let named = self
            .fallbacks
            .iter()
            .find(|fallback| fallback["model"] == served)
            .and_then(|fallback| serde_json::from_value::<ModelCost>(fallback["cost"].clone()).ok());
        let from_catalog =
            || crate::providers::catalog::models("anthropic").into_iter().find(|model| model.id == served).map(|model| model.cost);
        if let Some(cost) = named.or_else(from_catalog) {
            self.cost = cost;
            self.reprice();
        }
    }

    fn reprice(&mut self) {
        let usage = &mut self.output.usage;
        usage.total_tokens = usage.input + usage.output + usage.cache_read + usage.cache_write;
        calculate_cost(&self.cost, usage);
    }

    fn handle(&mut self, event: &Value, model: &Model, tools: &[ToolSpec], oauth: bool, sink: &EventSink) -> Result<(), String> {
        match event["type"].as_str().unwrap_or_default() {
            "message_start" => {
                self.saw_start = true;
                let message = &event["message"];
                set_response_id(&mut self.output, message["id"].as_str());
                if let Some(served) = message["model"].as_str() {
                    self.output.model = served.to_owned();
                    if served != model.id {
                        self.reprice_as(served);
                    }
                }
                let usage = &message["usage"];
                self.output.usage.input = count(&usage["input_tokens"]);
                self.output.usage.output = count(&usage["output_tokens"]);
                self.output.usage.cache_read = count(&usage["cache_read_input_tokens"]);
                self.output.usage.cache_write = count(&usage["cache_creation_input_tokens"]);
                self.output.usage.cache_write_1h = Some(count(&usage["cache_creation"]["ephemeral_1h_input_tokens"]));
                self.reprice();
            }
            "content_block_start" => {
                let block = &event["content_block"];
                let part = match block["type"].as_str() {
                    Some("text") => AssistantPart::Text { text: owned_text(block, "text"), text_signature: None },
                    Some("thinking") => AssistantPart::Thinking {
                        thinking: owned_text(block, "thinking"),
                        thinking_signature: Some(owned_text(block, "signature")),
                        redacted: None,
                    },
                    Some("redacted_thinking") => AssistantPart::Thinking {
                        thinking: "[Reasoning redacted]".into(),
                        thinking_signature: block["data"].as_str().map(str::to_owned),
                        redacted: Some(true),
                    },
                    Some("tool_use") => {
                        let name = block["name"].as_str().unwrap_or_default();
                        AssistantPart::ToolCall {
                            id: owned_text(block, "id"),
                            name: if oauth { from_claude_code_name(name, tools) } else { name.to_owned() },
                            arguments: if block["input"].is_null() { json!({}) } else { block["input"].clone() },
                            thought_signature: None,
                        }
                    }
                    // The server's own fallback: this model declined and the
                    // next one answers from here. Mark the handoff (the
                    // transcript names who answered) and bill the server.
                    Some("fallback") => {
                        let from = block["from"]["model"].as_str().unwrap_or_default();
                        let to = block["to"]["model"].as_str().unwrap_or_default();
                        let category = block["trigger"]["category"].as_str().unwrap_or_default();
                        crate::refusal::mark_fallback_served(&mut self.output, from, to, category);
                        if !to.is_empty() {
                            self.output.model = to.to_owned();
                            self.reprice_as(to);
                        }
                        return Ok(());
                    }
                    _ => return Ok(()),
                };
                let content_index = self.output.content.len();
                self.output.content.push(part);
                self.stream_index.push(event["index"].as_u64());
                let partial = self.output.clone();
                sink.push(match &self.output.content[content_index] {
                    AssistantPart::Text { .. } => StreamEvent::TextStart { content_index, partial },
                    AssistantPart::Thinking { .. } => StreamEvent::ThinkingStart { content_index, partial },
                    AssistantPart::ToolCall { .. } => {
                        self.partial_json.insert(content_index, String::new());
                        StreamEvent::ToolcallStart { content_index, partial }
                    }
                });
            }
            "content_block_delta" => {
                let Some(index) = self.position(&event["index"]) else { return Ok(()) };
                let delta = &event["delta"];
                let pushed = match (delta["type"].as_str(), &mut self.output.content[index]) {
                    (Some("text_delta"), AssistantPart::Text { text, .. }) => {
                        let piece = owned_text(delta, "text");
                        text.push_str(&piece);
                        Some(StreamEvent::TextDelta { content_index: index, delta: piece, partial: self.output.clone() })
                    }
                    (Some("thinking_delta"), AssistantPart::Thinking { thinking, .. }) => {
                        let piece = owned_text(delta, "thinking");
                        thinking.push_str(&piece);
                        Some(StreamEvent::ThinkingDelta { content_index: index, delta: piece, partial: self.output.clone() })
                    }
                    (Some("input_json_delta"), AssistantPart::ToolCall { arguments, .. }) => {
                        let piece = owned_text(delta, "partial_json");
                        let partial_json = self.partial_json.entry(index).or_default();
                        partial_json.push_str(&piece);
                        *arguments = parse_streaming_json(partial_json);
                        Some(StreamEvent::ToolcallDelta { content_index: index, delta: piece, partial: self.output.clone() })
                    }
                    (Some("signature_delta"), AssistantPart::Thinking { thinking_signature, .. }) => {
                        thinking_signature.get_or_insert_with(String::new).push_str(delta["signature"].as_str().unwrap_or_default());
                        None
                    }
                    _ => None,
                };
                if let Some(event) = pushed {
                    sink.push(event);
                }
            }
            "content_block_stop" => {
                let Some(index) = self.position(&event["index"]) else { return Ok(()) };
                self.stream_index[index] = None;
                if let Some(partial_json) = self.partial_json.remove(&index)
                    && let AssistantPart::ToolCall { arguments, .. } = &mut self.output.content[index]
                {
                    *arguments = parse_streaming_json(&partial_json);
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
            "message_delta" => {
                if let Some(reason) = event["delta"]["stop_reason"].as_str().filter(|reason| !reason.is_empty()) {
                    set_stop_reason(&mut self.output, Some(reason), map_stop_reason(reason, &event["delta"]["stop_details"]), false)?;
                    if reason == "refusal" {
                        crate::refusal::mark(&mut self.output, &event["delta"]["stop_details"]);
                    }
                }
                let usage = &event["usage"];
                let usage_out = &mut self.output.usage;
                update_count(&mut usage_out.input, &usage["input_tokens"]);
                update_count(&mut usage_out.output, &usage["output_tokens"]);
                update_count(&mut usage_out.cache_read, &usage["cache_read_input_tokens"]);
                update_count(&mut usage_out.cache_write, &usage["cache_creation_input_tokens"]);
                if let Some(thinking) = usage["output_tokens_details"]["thinking_tokens"].as_u64() {
                    usage_out.reasoning = Some(thinking);
                }
                self.reprice();
            }
            "message_stop" => self.saw_stop = true,
            _ => {}
        }
        Ok(())
    }
}

fn raw_lines(message: &SseMessage) -> String {
    let mut lines: Vec<String> = message.event.iter().map(|event| format!("event: {event}")).collect();
    lines.extend(message.data.split('\n').map(|line| format!("data: {line}")));
    lines.join("\\n")
}

/// Pi's own SSE reader for Anthropic: an `error` event raises its data; other non-message events are skipped.
fn dispatch(
    messages: Vec<SseMessage>,
    parser: &mut Parser,
    model: &Model,
    context: &LlmContext,
    oauth: bool,
    sink: &EventSink,
) -> Result<(), String> {
    for message in messages {
        let event_name = message.event.as_deref().unwrap_or_default();
        if event_name == "error" {
            return Err(message.data);
        }
        if !MESSAGE_EVENTS.contains(&event_name) {
            continue;
        }
        let event = parse_json_with_repair(&message.data).map_err(|error| {
            format!("Could not parse Anthropic SSE event {event_name}: {error}; data={}; raw={}", message.data, raw_lines(&message))
        })?;
        parser.handle(&event, model, &context.tools, oauth, sink)?;
    }
    Ok(())
}

/// Whether a failed response is the subscription's usage limit (its 5-hour
/// or weekly one), not a passing rate limit or an overload: a 429 whose
/// `anthropic-ratelimit-unified-status` is "rejected". Some, with when it
/// resets (`anthropic-ratelimit-unified-reset`, seconds since 1970) if said.
pub fn usage_limit(status: u16, headers: &reqwest::header::HeaderMap) -> Option<Option<u64>> {
    let header = |name: &str| headers.get(name).and_then(|value| value.to_str().ok()).map(str::trim);
    if status != 429 || header("anthropic-ratelimit-unified-status") != Some("rejected") {
        return None;
    }
    Some(header("anthropic-ratelimit-unified-reset").and_then(|reset| reset.parse::<f64>().ok()).map(|seconds| (seconds * 1000.0) as u64))
}

/// Which limit a usage limit's refusal is (`usage_limit`), as Anthropic
/// names it: "five_hour", "seven_day", or a weekly limit of some models
/// alone, "seven_day_opus" or "seven_day_sonnet", which refuses only those
/// models' requests (as Claude Code reads it, and its stand-in for
/// Anthropic's limits sends it). When several are reached, the one that
/// resets last.
pub fn limit_claim(headers: &reqwest::header::HeaderMap) -> Option<&str> {
    headers.get("anthropic-ratelimit-unified-representative-claim").and_then(|value| value.to_str().ok()).map(str::trim)
}

/// The images of `context` that can go as files (#382): read since, in a
/// request to Anthropic that keeps files, for a model that takes images.
fn file_images<'a>(model: &Model, context: &'a LlmContext, options: &StreamOptions) -> Vec<files::Image<'a>> {
    if options.image_files.is_none() || !model.input.iter().any(|input| input == "image") {
        return Vec::new();
    }
    context
        .messages
        .iter()
        .filter_map(|message| match message {
            Message::ToolResult(result) => Some(&result.content),
            _ => None,
        })
        .flatten()
        .filter_map(|part| match part {
            ToolResultPart::Image { data, mime_type, digest: Some(digest) } if !data.is_empty() => {
                Some(files::Image { digest, data, mime_type })
            }
            _ => None,
        })
        .collect()
}

/// The file id each of `images` goes as, uploading the new ones first, and
/// whether any was uploaded now.
async fn image_refs(model: &Model, options: &StreamOptions, oauth: bool, images: &[files::Image<'_>]) -> (FileRefs, bool) {
    let Some(files) = options.image_files.as_ref().filter(|_| !images.is_empty()) else { return (FileRefs::new(), false) };
    let headers = file_headers(model, options, oauth);
    files.files.refs(&files.account, images, &model.base_url, &headers, &options.cancel).await
}

/// The headers of a request to Anthropic's Files API: a request's, with
/// no conversation.
pub fn file_headers(model: &Model, options: &StreamOptions, oauth: bool) -> Vec<(String, String)> {
    request_headers(model, &LlmContext::default(), options, oauth)
}

/// Whether Anthropic refused a request for a file it names: one it no
/// longer has, or files it won't read from this account. A limit, an
/// outage or anything else about the request isn't one.
fn file_refused(status: u16, text: &str) -> bool {
    matches!(status, 400 | 403 | 404) && text.to_ascii_lowercase().contains("file")
}

async fn request(
    model: &Model,
    context: &LlmContext,
    options: &StreamOptions,
    max_tokens: u64,
    thinking: &Thinking,
    parser: &mut Parser,
    sink: &EventSink,
) -> Result<(), String> {
    let oauth = options.api_key.as_deref().is_some_and(|key| key.contains("sk-ant-oat"));
    // Images read since #382 go as files Anthropic already has, uploaded
    // the first time; a request Anthropic refuses for one of them (another
    // conversation let go of it, or files refused) is sent again once, with
    // them inline.
    let images = file_images(model, context, options);
    let (mut refs, fresh) = image_refs(model, options, oauth, &images).await;
    check_cancelled(options)?;
    let mut retried = false;
    // Refused for thinking bound to another conversation: sent again once,
    // with none of it (`binding`).
    let mut unbound = false;
    let response = loop {
        let (mut params, bound) = build_params(model, context, oauth, max_tokens, thinking, &refs, unbound)?;
        binding::mark(&mut parser.output, &bound, unbound);
        // The only lifetime it can ask for beyond its own 5 minutes.
        if options.cache_ttl == CacheTtl::OneHour {
            set_cache_ttl(&mut params, options.cache_ttl);
        }
        let mut builder = client().post(format!("{}/v1/messages", model.base_url.trim_end_matches('/')));
        for (name, value) in request_headers(model, context, options, oauth) {
            builder = builder.header(name, value);
        }
        builder = builder.header("content-type", "application/json").body(params.to_string());
        let response = send_request(builder, options).await?;
        let status = response.status().as_u16();
        if (200..300).contains(&status) {
            break response;
        }
        let limit = usage_limit(status, response.headers()).map(|resets_at| (resets_at, limit_claim(response.headers()).map(str::to_owned)));
        let text = response_text(response).await;
        if let Some(files) = options.image_files.as_ref().filter(|_| !retried && !refs.is_empty() && file_refused(status, &text)) {
            tracing::info!(account = %files.account, status, "Anthropic refused a request naming images as files; sending them inline");
            files.files.refuse(&files.account, &refs, fresh);
            refs.clear();
            retried = true;
            continue;
        }
        if !unbound && binding::refused(status, &text) {
            tracing::warn!(model = %model.id, "Anthropic refused earlier thinking as bound to another conversation; sending it again without");
            unbound = true;
            continue;
        }
        if let Some((resets_at, claim)) = limit {
            crate::accounts::mark_usage_limit(&mut parser.output, resets_at);
            if let Some(claim) = claim {
                crate::accounts::mark_usage_limit_claim(&mut parser.output, &claim);
            }
        }
        if crate::accounts::credit_error(status, &text) {
            crate::accounts::mark_out_of_credit(&mut parser.output);
        } else if crate::accounts::refused_error(status, &text) {
            crate::accounts::mark_refused(&mut parser.output, &crate::accounts::Refused::default());
        }
        let parsed = serde_json::from_str::<Value>(&text).ok().filter(truthy);
        let fallback = if parsed.is_some() { None } else { Some(text.as_str()) };
        return Err(api_error_message(Some(status), &parsed.unwrap_or(Value::Null), fallback));
    };
    sink.push(StreamEvent::Start { partial: parser.output.clone() });
    let mut body = response.bytes_stream();
    let mut decoder = SseDecoder::new();
    loop {
        let chunk = tokio::select! {
            chunk = body.next() => chunk,
            () = options.cancel.cancelled() => return Err("Request was aborted".into()),
        };
        let Some(chunk) = chunk else { break };
        let chunk = chunk.map_err(|error| error.to_string())?;
        dispatch(decoder.push(&chunk), parser, model, context, oauth, sink)?;
    }
    dispatch(decoder.finish(), parser, model, context, oauth, sink)?;
    if parser.saw_start && !parser.saw_stop {
        return Err("Anthropic stream ended before message_stop".into());
    }
    check_cancelled(options)?;
    match parser.output.stop_reason {
        StopReason::Pending => Err("Anthropic stream ended without a stop reason".into()),
        StopReason::Error | StopReason::Aborted => {
            Err(parser.output.error_message.clone().unwrap_or_else(|| "An unknown error occurred".into()))
        }
        _ => Ok(()),
    }
}
