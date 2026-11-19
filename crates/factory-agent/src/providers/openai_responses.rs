use hashbrown::HashMap;
use serde::Serialize;
use serde_json::{Map, Value, json};

use crate::clock::now_ms;
use crate::json_parse::parse_streaming_json;
use crate::providers::{push, set_response_id, set_stop_reason, tool_text, toolcall_delta, user_content};
use crate::stream::{EventSink, LlmContext};
use crate::tools::ToolSpec;
use crate::types::{
    AssistantMessage, AssistantPart, Cost, Message, Model, StopReason, StreamEvent, ToolResultMessage, ToolResultPart, Usage, UserContent,
    UserMessage, UserPart,
};
use crate::usage::calculate_cost;

const OPENAI_TOOL_CALL_PROVIDERS: [&str; 3] = ["openai", "openai-codex", "opencode"];
const USER_IMAGE_PLACEHOLDER: &str = "(image omitted: model does not support images)";
const TOOL_IMAGE_PLACEHOLDER: &str = "(tool image omitted: model does not support images)";
const UNSUPPORTED_STRICT_KEYS: [&str; 16] = [
    "$ref",
    "$defs",
    "definitions",
    "allOf",
    "oneOf",
    "patternProperties",
    "dependentSchemas",
    "dependencies",
    "unevaluatedProperties",
    "propertyNames",
    "contains",
    "prefixItems",
    "not",
    "if",
    "then",
    "else",
];

fn base36(mut value: u32) -> String {
    let mut out = Vec::new();
    loop {
        out.push(b"0123456789abcdefghijklmnopqrstuvwxyz"[(value % 36) as usize]);
        value /= 36;
        if value == 0 {
            break;
        }
    }
    out.reverse();
    String::from_utf8(out).unwrap_or_default()
}

/// Pi's `shortHash`: two `Math.imul` mixers over UTF-16 units, base 36.
fn mix(a: u32, b: u32) -> u32 {
    (a ^ (a >> 16)).wrapping_mul(2_246_822_507) ^ (b ^ (b >> 13)).wrapping_mul(3_266_489_909)
}

pub fn short_hash(text: &str) -> String {
    let mut h1: u32 = 0xdead_beef;
    let mut h2: u32 = 0x41c6_ce57;
    for unit in text.encode_utf16() {
        h1 = (h1 ^ u32::from(unit)).wrapping_mul(2_654_435_761);
        h2 = (h2 ^ u32::from(unit)).wrapping_mul(1_597_334_677);
    }
    h1 = mix(h1, h2);
    h2 = mix(h2, h1);
    format!("{}{}", base36(h2), base36(h1))
}

pub(super) fn safe_id(part: &str) -> String {
    part.encode_utf16()
        .map(|unit| match char::from_u32(u32::from(unit)) {
            Some(c) if c.is_ascii_alphanumeric() || c == '_' || c == '-' => c,
            _ => '_',
        })
        .collect()
}

fn normalize_id_part(part: &str) -> String {
    let mut sanitized = safe_id(part);
    sanitized.truncate(64);
    sanitized.trim_end_matches('_').to_owned()
}

fn foreign_item_id(item_id: &str) -> String {
    let mut id = format!("fc_{}", short_hash(item_id));
    id.truncate(64);
    id
}

fn same_model(assistant: &AssistantMessage, model: &Model) -> bool {
    assistant.provider == model.provider && assistant.api == model.api && assistant.model == model.id
}

macro_rules! text_parts {
    ($name:ident, $part:ident) => {
        fn $name(parts: &[$part], placeholder: &str) -> Vec<$part> {
            let mut out = Vec::new();
            let mut previous_placeholder = false;
            for part in parts {
                match part {
                    $part::Image { .. } => {
                        if !previous_placeholder {
                            out.push($part::Text { text: placeholder.to_owned() });
                        }
                        previous_placeholder = true;
                    }
                    $part::Text { text } => {
                        out.push(part.clone());
                        previous_placeholder = text == placeholder;
                    }
                }
            }
            out
        }
    };
}
text_parts!(tool_text_parts, ToolResultPart);
text_parts!(user_text_parts, UserPart);

type Normalize<'a> = &'a dyn Fn(&str, &AssistantMessage) -> String;

fn transform_assistant(
    assistant: &AssistantMessage,
    model: &Model,
    normalize: Option<Normalize>,
    id_map: &mut HashMap<String, String>,
) -> Message {
    let same = same_model(assistant, model);
    let content = assistant
        .content
        .iter()
        .filter_map(|part| match part {
            AssistantPart::Thinking { thinking, thinking_signature, redacted } => {
                if redacted == &Some(true) {
                    return same.then(|| part.clone());
                }
                if same && thinking_signature.is_some() {
                    return Some(part.clone());
                }
                if thinking.trim().is_empty() {
                    return None;
                }
                Some(if same { part.clone() } else { AssistantPart::Text { text: thinking.clone(), text_signature: None } })
            }
            AssistantPart::Text { text, .. } => {
                Some(if same { part.clone() } else { AssistantPart::Text { text: text.clone(), text_signature: None } })
            }
            AssistantPart::ToolCall { id, name, arguments, thought_signature } => {
                let mut normalized_id = id.clone();
                if let (false, Some(normalize)) = (same, normalize) {
                    let normalized = normalize(id, assistant);
                    if normalized != *id {
                        id_map.insert(id.clone(), normalized.clone());
                        normalized_id = normalized;
                    }
                }
                Some(AssistantPart::ToolCall {
                    id: normalized_id,
                    name: name.clone(),
                    arguments: arguments.clone(),
                    thought_signature: if same { thought_signature.clone() } else { None },
                })
            }
        })
        .collect();
    Message::Assistant(AssistantMessage { content, ..assistant.clone() })
}

fn synthesize(result: &mut Vec<Message>, pending: &mut Vec<(String, String)>, answered: &mut Vec<String>) {
    for (id, name) in pending.drain(..) {
        if !answered.contains(&id) {
            result.push(Message::ToolResult(ToolResultMessage {
                tool_call_id: id,
                tool_name: name,
                content: vec![ToolResultPart::Text { text: "No result provided".into() }],
                details: Value::Null,
                usage: None,
                is_error: true,
                timestamp: now_ms(),
                started: None,
            }));
        }
    }
    answered.clear();
}

/// `transform-messages.js`: image downgrade, cross-model thinking and id normalization, errored turns dropped, orphaned calls answered.
pub fn transform_messages(messages: &[Message], model: &Model, normalize: Option<Normalize>) -> Vec<Message> {
    let images = model.input.iter().any(|input| input == "image");
    let mut id_map: HashMap<String, String> = HashMap::new();
    let transformed: Vec<Message> = messages
        .iter()
        .map(|message| match message {
            Message::User(user) => match (&user.content, images) {
                (UserContent::Parts(parts), false) => Message::User(UserMessage {
                    content: UserContent::Parts(user_text_parts(parts, USER_IMAGE_PLACEHOLDER)),
                    ..user.clone()
                }),
                _ => message.clone(),
            },
            Message::ToolResult(result) => {
                let mut result = result.clone();
                if !images {
                    result.content = tool_text_parts(&result.content, TOOL_IMAGE_PLACEHOLDER);
                }
                if let Some(mapped) = id_map.get(&result.tool_call_id) {
                    result.tool_call_id = mapped.clone();
                }
                Message::ToolResult(result)
            }
            Message::Assistant(assistant) => transform_assistant(assistant, model, normalize, &mut id_map),
            Message::System(_) => message.clone(),
        })
        .collect();
    let mut result = Vec::new();
    let mut pending: Vec<(String, String)> = Vec::new();
    let mut answered: Vec<String> = Vec::new();
    for message in transformed {
        match &message {
            Message::Assistant(assistant) => {
                synthesize(&mut result, &mut pending, &mut answered);
                if matches!(assistant.stop_reason, StopReason::Error | StopReason::Aborted) {
                    continue;
                }
                let calls: Vec<(String, String)> = assistant
                    .content
                    .iter()
                    .filter_map(|part| match part {
                        AssistantPart::ToolCall { id, name, .. } => Some((id.clone(), name.clone())),
                        _ => None,
                    })
                    .collect();
                if !calls.is_empty() {
                    pending = calls;
                    answered.clear();
                }
                result.push(message);
            }
            Message::ToolResult(tool) => {
                answered.push(tool.tool_call_id.clone());
                result.push(message);
            }
            Message::User(_) => {
                synthesize(&mut result, &mut pending, &mut answered);
                result.push(message);
            }
            Message::System(_) => result.push(message),
        }
    }
    synthesize(&mut result, &mut pending, &mut answered);
    result
}

fn encode_text_signature(id: &str, phase: Option<&str>) -> String {
    let mut payload = json!({"v": 1, "id": id});
    if let Some(phase) = phase {
        payload["phase"] = json!(phase);
    }
    payload.to_string()
}

fn parse_text_signature(signature: Option<&str>) -> Option<(String, Option<String>)> {
    let signature = signature?;
    if signature.starts_with('{')
        && let Ok(parsed) = serde_json::from_str::<Value>(signature)
        && parsed["v"] == 1
        && let Some(id) = parsed["id"].as_str()
    {
        let phase = parsed["phase"].as_str().filter(|phase| *phase == "commentary" || *phase == "final_answer").map(str::to_owned);
        return Some((id.to_owned(), phase));
    }
    Some((signature.to_owned(), None))
}

fn responses_image(data: &str, mime_type: &str) -> Value {
    json!({"type": "input_image", "detail": "auto", "image_url": format!("data:{mime_type};base64,{data}")})
}

fn tool_result_output(model: &Model, content: &[ToolResultPart]) -> Value {
    let (text, has_images) = tool_text(content);
    let images: Vec<&ToolResultPart> = content.iter().filter(|part| matches!(part, ToolResultPart::Image { .. })).collect();
    if !has_images || !model.input.iter().any(|input| input == "image") {
        return Value::String(if !text.is_empty() {
            text
        } else if !has_images {
            "(no tool output)".into()
        } else {
            "(see attached image)".into()
        });
    }
    let mut output = Vec::new();
    if !text.is_empty() {
        output.push(json!({"type": "input_text", "text": text}));
    }
    for image in images {
        if let ToolResultPart::Image { data, mime_type } = image {
            output.push(responses_image(data, mime_type));
        }
    }
    Value::Array(output)
}

pub fn supports_strict_mode(model: &Model) -> bool {
    model.compat.as_ref().and_then(|compat| compat.get("supportsStrictMode")).and_then(Value::as_bool).unwrap_or(false)
}

fn supports_developer_role(model: &Model) -> bool {
    model.compat.as_ref().and_then(|compat| compat.get("supportsDeveloperRole")).and_then(Value::as_bool) != Some(false)
}

#[derive(Clone, Copy)]
pub(super) enum UserMessageDialect {
    Responses,
    Chat,
}

pub(super) fn chat_image(data: &str, mime_type: &str) -> Value {
    json!({"type": "image_url", "image_url": {"url": format!("data:{mime_type};base64,{data}")}})
}

pub(super) fn user_message(user: &UserMessage, dialect: UserMessageDialect) -> Option<Value> {
    let content = match &user.content {
        UserContent::Text(text) => match dialect {
            UserMessageDialect::Responses => json!([{"type": "input_text", "text": text}]),
            UserMessageDialect::Chat => json!(text),
        },
        UserContent::Parts(parts) => {
            if parts.is_empty() {
                return None;
            }
            Value::Array(
                parts
                    .iter()
                    .map(|part| match (part, dialect) {
                        (UserPart::Text { text }, UserMessageDialect::Responses) => json!({"type": "input_text", "text": text}),
                        (UserPart::Text { text }, UserMessageDialect::Chat) => json!({"type": "text", "text": text}),
                        (UserPart::Image { data, mime_type }, UserMessageDialect::Responses) => responses_image(data, mime_type),
                        (UserPart::Image { data, mime_type }, UserMessageDialect::Chat) => chat_image(data, mime_type),
                    })
                    .collect(),
            )
        }
    };
    Some(user_content(content))
}

fn assistant_items(assistant: &AssistantMessage, model: &Model, index: usize) -> Result<Vec<Value>, String> {
    let same_provider = assistant.provider == model.provider && assistant.api == model.api;
    let different_model = same_provider && assistant.model != model.id;
    let mut text_index = 0;
    let mut output = Vec::new();
    for part in &assistant.content {
        match part {
            AssistantPart::Thinking { thinking_signature: Some(signature), .. } => {
                output.push(serde_json::from_str::<Value>(signature).map_err(|error| error.to_string())?);
            }
            AssistantPart::Thinking { .. } => {}
            AssistantPart::Text { text, text_signature } => {
                let parsed = parse_text_signature(text_signature.as_deref());
                let fallback = if text_index == 0 { format!("msg_pi_{index}") } else { format!("msg_pi_{index}_{text_index}") };
                text_index += 1;
                let id = match parsed.as_ref().map(|(id, _)| id.as_str()).filter(|id| !id.is_empty()) {
                    None => fallback,
                    Some(id) if id.encode_utf16().count() > 64 => format!("msg_{}", short_hash(id)),
                    Some(id) => id.to_owned(),
                };
                let mut item = json!({
                    "type": "message", "role": "assistant",
                    "content": [{"type": "output_text", "text": text, "annotations": []}],
                    "status": "completed", "id": id,
                });
                if let Some((_, Some(phase))) = parsed {
                    item["phase"] = json!(phase);
                }
                output.push(item);
            }
            AssistantPart::ToolCall { id, name, arguments, .. } => {
                let (call_id, item_id) =
                    id.split_once('|').map_or((id.as_str(), None), |(call, item)| (call, Some(item.split('|').next().unwrap_or_default())));
                let item_id = item_id.filter(|item| item.starts_with("fc_") && !different_model);
                let mut item = Map::new();
                item.insert("type".into(), json!("function_call"));
                if let Some(item_id) = item_id {
                    item.insert("id".into(), json!(item_id));
                }
                item.insert("call_id".into(), json!(call_id));
                item.insert("name".into(), json!(name));
                item.insert("arguments".into(), json!(arguments.to_string()));
                output.push(Value::Object(item));
            }
        }
    }
    Ok(output)
}

/// `convertResponsesMessages` for function tools (no grammar or deferred tools, which this agent never defines).
pub fn convert_messages(model: &Model, context: &LlmContext) -> Result<Vec<Value>, String> {
    convert_input(model, context, true)
}

#[derive(Serialize)]
pub(crate) struct Note<'a> {
    pub role: &'static str,
    pub content: &'a str,
}

/// The Codex request carries the system prompt as `instructions`, so its input omits it.
pub fn convert_input(model: &Model, context: &LlmContext, include_system_prompt: bool) -> Result<Vec<Value>, String> {
    let normalize = |id: &str, source: &AssistantMessage| -> String {
        if !OPENAI_TOOL_CALL_PROVIDERS.contains(&model.provider.as_str()) {
            return normalize_id_part(id);
        }
        let Some((call_id, item_id)) = id.split_once('|') else { return normalize_id_part(id) };
        let item_id = item_id.split('|').next().unwrap_or_default();
        let foreign = source.provider != model.provider || source.api != model.api;
        let mut item = if foreign { foreign_item_id(item_id) } else { normalize_id_part(item_id) };
        if !item.starts_with("fc_") {
            item = normalize_id_part(&format!("fc_{item}"));
        }
        format!("{}|{item}", normalize_id_part(call_id))
    };
    let transformed = transform_messages(&context.messages, model, Some(&normalize));
    let mut items = Vec::new();
    if let Some(prompt) = context.system_prompt.as_deref().filter(|prompt| include_system_prompt && !prompt.is_empty()) {
        let role = if model.reasoning && supports_developer_role(model) { "developer" } else { "system" };
        items.push(json!({"role": role, "content": prompt}));
    }
    let mut index = 0;
    for message in &transformed {
        let before = items.len();
        match message {
            Message::User(user) => items.extend(user_message(user, UserMessageDialect::Responses)),
            Message::System(note) => items.push(
                serde_json::to_value(Note {
                    role: if model.reasoning && supports_developer_role(model) { "developer" } else { "system" },
                    content: &note.text,
                })
                .map_err(|error| error.to_string())?,
            ),
            Message::Assistant(assistant) => items.extend(assistant_items(assistant, model, index)?),
            Message::ToolResult(result) => {
                let call_id = result.tool_call_id.split('|').next().unwrap_or_default();
                items.push(
                    json!({"type": "function_call_output", "call_id": call_id, "output": tool_result_output(model, &result.content)}),
                );
            }
        }
        if items.len() > before {
            index += 1;
        }
    }
    Ok(items)
}

fn is_structured(schema: &Value) -> bool {
    let Some(object) = schema.as_object() else { return false };
    let types: Vec<&str> = match object.get("type") {
        Some(Value::String(one)) => vec![one.as_str()],
        Some(Value::Array(many)) => many.iter().filter_map(Value::as_str).collect(),
        _ => Vec::new(),
    };
    types.contains(&"object") || types.contains(&"array") || object.contains_key("properties") || object.contains_key("items")
}

fn allows_null(schema: &Value) -> bool {
    let Some(object) = schema.as_object() else { return false };
    let typed_null = match object.get("type") {
        Some(Value::String(one)) => one == "null",
        Some(Value::Array(many)) => many.iter().any(|entry| entry == "null"),
        _ => false,
    };
    typed_null
        || object.get("const").is_some_and(Value::is_null)
        || object.get("enum").and_then(Value::as_array).is_some_and(|values| values.iter().any(Value::is_null))
        || object.get("anyOf").and_then(Value::as_array).is_some_and(|variants| variants.iter().any(allows_null))
}

fn make_node_strict(schema: &mut Value) -> Result<(), String> {
    let Some(object) = schema.as_object_mut() else { return Err("boolean schemas are unsupported".into()) };
    for key in UNSUPPORTED_STRICT_KEYS {
        if object.contains_key(key) {
            return Err(format!("{key} schemas are unsupported"));
        }
    }
    if let Some(any_of) = object.get_mut("anyOf") {
        let Some(variants) = any_of.as_array_mut().filter(|variants| !variants.is_empty()) else {
            return Err("anyOf must contain at least one schema".into());
        };
        for variant in variants.iter_mut() {
            if is_structured(variant) {
                return Err("object and array unions are unsupported".into());
            }
            make_node_strict(variant)?;
        }
    }
    if let Some(items) = object.get_mut("items") {
        if items.is_array() {
            return Err("tuple schemas are unsupported".into());
        }
        make_node_strict(items)?;
    }
    let is_object = object.get("type") == Some(&json!("object"));
    if object.contains_key("properties") && !is_object {
        return Err("properties require type object".into());
    }
    if !is_object {
        return Ok(());
    }
    if object.get("additionalProperties").is_some_and(|value| value != &json!(false)) {
        return Err("schema-valued or true additionalProperties is unsupported".into());
    }
    if object.get("properties").is_some_and(|properties| !properties.is_object()) {
        return Err("object properties must be a schema map".into());
    }
    let required: Vec<String> = match object.get("required") {
        None => Vec::new(),
        Some(Value::Array(keys)) if keys.iter().all(Value::is_string) => keys.iter().filter_map(Value::as_str).map(str::to_owned).collect(),
        Some(_) => return Err("object required must be a string array".into()),
    };
    let mut properties = object.get("properties").and_then(Value::as_object).cloned().unwrap_or_default();
    let names: Vec<String> = properties.keys().cloned().collect();
    if required.iter().any(|key| !names.contains(key)) {
        return Err("required contains an unknown property".into());
    }
    for (key, property) in properties.iter_mut() {
        make_node_strict(property)?;
        if !required.contains(key) && !allows_null(property) {
            *property = json!({"anyOf": [property.clone(), {"type": "null"}]});
        }
    }
    if object.contains_key("properties") {
        object.insert("properties".into(), Value::Object(properties));
    }
    object.insert("required".into(), json!(names));
    object.insert("additionalProperties".into(), json!(false));
    Ok(())
}

/// The strict subset provider constrained sampling accepts; the error is Pi's reason.
pub fn make_strict_json_schema(schema: &Value) -> Result<Value, String> {
    let mut strict = schema.clone();
    if !strict.is_object() {
        return Err("root schema must have type object".into());
    }
    make_node_strict(&mut strict)?;
    if strict.get("type") != Some(&json!("object")) {
        return Err("root schema must have type object".into());
    }
    Ok(strict)
}

pub fn strict_sampling(tool: &ToolSpec, supports_strict_mode: bool) -> Result<Option<bool>, String> {
    let config = &tool.constrained_sampling;
    if config.get("type") != Some(&json!("json_schema")) {
        return Ok(None);
    }
    let required = config.get("strict") == Some(&json!("require"));
    if supports_strict_mode {
        return match make_strict_json_schema(&tool.parameters) {
            Ok(_) => Ok(Some(true)),
            Err(_) if !required => Ok(None),
            Err(reason) => Err(format!("Tool \"{}\" requires JSON-schema constrained sampling, but {reason}.", tool.name)),
        };
    }
    if required {
        return Err(format!("Tool \"{}\" requires JSON-schema constrained sampling, but strict tools are unsupported.", tool.name));
    }
    Ok(None)
}

pub fn convert_tools(tools: &[ToolSpec], supports_strict_mode: bool) -> Result<Vec<Value>, String> {
    convert_tools_with(tools, supports_strict_mode, json!(false))
}

/// `convertResponsesTools`: `default_strict` is the `strict` value for tools without constrained sampling (`false` or `null`).
pub fn convert_tools_with(tools: &[ToolSpec], supports_strict_mode: bool, default_strict: Value) -> Result<Vec<Value>, String> {
    tools
        .iter()
        .map(|tool| {
            let strict = strict_sampling(tool, supports_strict_mode)?.map_or(default_strict.clone(), |strict| json!(strict));
            let parameters = if strict == json!(true) { make_strict_json_schema(&tool.parameters)? } else { tool.parameters.clone() };
            let mut item = json!({"type": "function", "name": tool.name, "description": tool.description, "parameters": parameters});
            if supports_strict_mode {
                item["strict"] = strict;
            }
            Ok(item)
        })
        .collect()
}

enum Slot {
    Thinking(usize),
    Text(usize),
    ToolCall { index: usize, partial_json: String },
}

/// `processResponsesStream`: output items become content parts by output index; usage and stop reason come from the terminal response.
pub struct Parser {
    pub output: AssistantMessage,
    slots: HashMap<u64, Slot>,
    reasoning_blocks: HashMap<String, usize>,
    saw_terminal: bool,
}

fn text(value: &Value) -> &str {
    value.as_str().unwrap_or_default()
}

fn js_string(value: &Value) -> String {
    match value {
        Value::Null => "undefined".into(),
        Value::String(text) => text.clone(),
        other => other.to_string(),
    }
}

fn map_stop_reason(status: Option<&str>, incomplete: Option<&str>) -> Result<(StopReason, Option<String>), String> {
    Ok(match status {
        None | Some("") => (StopReason::Stop, None),
        Some("completed" | "in_progress" | "queued") => (StopReason::Stop, None),
        Some("incomplete") => match incomplete {
            Some("max_output_tokens") => (StopReason::Length, None),
            Some(reason) => (StopReason::Error, Some(format!("Response incomplete: {reason}"))),
            None => (StopReason::Error, Some("Response incomplete without a provider reason".into())),
        },
        Some("failed" | "cancelled") => (StopReason::Error, None),
        Some(other) => return Err(format!("Unhandled stop reason: {other}")),
    })
}

fn joined_texts(item: &Value, key: &str) -> String {
    item[key].as_array().map(|parts| parts.iter().map(|part| text(&part["text"])).collect::<Vec<_>>().join("\n\n")).unwrap_or_default()
}

impl Parser {
    pub fn new(output: AssistantMessage) -> Parser {
        Parser { output, slots: HashMap::new(), reasoning_blocks: HashMap::new(), saw_terminal: false }
    }

    fn apply_phase(&mut self, item: &Value) {
        if item["type"] == "message" && item["phase"] == "final_answer" {
            self.output.stop_reason = StopReason::Stop;
        }
    }

    fn create_slot(&mut self, output_index: u64, item: &Value, sink: &EventSink) {
        let index = self.output.content.len();
        match text(&item["type"]) {
            "reasoning" => {
                self.output.content.push(AssistantPart::Thinking { thinking: String::new(), thinking_signature: None, redacted: None });
                self.slots.insert(output_index, Slot::Thinking(index));
                push(&self.output, sink, |partial| StreamEvent::ThinkingStart { content_index: index, partial });
            }
            "message" => {
                self.apply_phase(item);
                self.output.content.push(AssistantPart::Text { text: String::new(), text_signature: None });
                self.slots.insert(output_index, Slot::Text(index));
                push(&self.output, sink, |partial| StreamEvent::TextStart { content_index: index, partial });
            }
            "function_call" => {
                self.output.content.push(AssistantPart::ToolCall {
                    id: format!("{}|{}", js_string(&item["call_id"]), js_string(&item["id"])),
                    name: text(&item["name"]).to_owned(),
                    arguments: Value::Object(Map::new()),
                    thought_signature: None,
                });
                self.slots.insert(output_index, Slot::ToolCall { index, partial_json: text(&item["arguments"]).to_owned() });
                push(&self.output, sink, |partial| StreamEvent::ToolcallStart { content_index: index, partial });
            }
            _ => {}
        }
    }

    fn delta(&mut self, output_index: u64, delta: &str, thinking: bool, sink: &EventSink) {
        let index = match (thinking, self.slots.get(&output_index)) {
            (true, Some(Slot::Thinking(index))) | (false, Some(Slot::Text(index))) => *index,
            _ => return,
        };
        match (thinking, self.output.content.get_mut(index)) {
            (true, Some(AssistantPart::Thinking { thinking: text, .. })) | (false, Some(AssistantPart::Text { text, .. })) => {
                text.push_str(delta)
            }
            _ => {}
        }
        push(&self.output, sink, |partial| {
            if thinking {
                StreamEvent::ThinkingDelta { content_index: index, delta: delta.to_owned(), partial }
            } else {
                StreamEvent::TextDelta { content_index: index, delta: delta.to_owned(), partial }
            }
        });
    }

    fn set_arguments(&mut self, index: usize, arguments: Value) {
        if let Some(AssistantPart::ToolCall { arguments: slot, .. }) = self.output.content.get_mut(index) {
            *slot = arguments;
        }
    }

    fn backfill_signatures(&mut self, response: &Value) {
        for item in response["output"].as_array().into_iter().flatten() {
            let Some(encrypted) = item["encrypted_content"].as_str().filter(|_| item["type"] == "reasoning") else { continue };
            let Some(index) = self.reasoning_blocks.get(text(&item["id"])) else { continue };
            let Some(AssistantPart::Thinking { thinking_signature: Some(signature), .. }) = self.output.content.get_mut(*index) else {
                continue;
            };
            let Ok(Value::Object(mut stored)) = serde_json::from_str::<Value>(signature) else { continue };
            if stored.get("encrypted_content").is_some_and(|value| !value.is_null() && value != "") {
                continue;
            }
            stored.insert("encrypted_content".into(), json!(encrypted));
            *signature = Value::Object(stored).to_string();
        }
    }

    fn finalize(&mut self, response: &Value, model: &Model) -> Result<(), String> {
        self.saw_terminal = true;
        self.backfill_signatures(response);
        set_response_id(&mut self.output, response["id"].as_str());
        if let Some(usage) = response.get("usage").filter(|usage| usage.is_object()) {
            let cached = usage["input_tokens_details"]["cached_tokens"].as_u64().unwrap_or(0);
            let written = usage["input_tokens_details"]["cache_write_tokens"].as_u64().unwrap_or(0);
            self.output.usage = Usage {
                input: usage["input_tokens"].as_u64().unwrap_or(0).saturating_sub(cached).saturating_sub(written),
                output: usage["output_tokens"].as_u64().unwrap_or(0),
                cache_read: cached,
                cache_write: written,
                cache_write_1h: None,
                reasoning: Some(usage["output_tokens_details"]["reasoning_tokens"].as_u64().unwrap_or(0)),
                total_tokens: usage["total_tokens"].as_u64().unwrap_or(0),
                cost: Cost::ZERO,
            };
        }
        calculate_cost(&model.cost, &mut self.output.usage);
        let status = response["status"].as_str();
        let incomplete = response["incomplete_details"]["reason"].as_str();
        let raw = status.map(|status| match incomplete {
            Some(reason) => format!("{status}.{reason}"),
            None => status.to_owned(),
        });
        set_stop_reason(&mut self.output, raw.as_deref(), map_stop_reason(status, incomplete), true)?;
        if self.output.stop_reason == StopReason::Stop
            && self.output.content.iter().any(|part| matches!(part, AssistantPart::ToolCall { .. }))
        {
            self.output.stop_reason = StopReason::ToolUse;
        }
        Ok(())
    }

    fn item_done(&mut self, output_index: u64, item: &Value, sink: &EventSink) {
        self.apply_phase(item);
        if !self.slots.contains_key(&output_index) {
            self.create_slot(output_index, item, sink);
        }
        match (text(&item["type"]), self.slots.remove(&output_index)) {
            ("reasoning", Some(Slot::Thinking(index))) => {
                let (summary, content) = (joined_texts(item, "summary"), joined_texts(item, "content"));
                let mut final_text = String::new();
                if let Some(AssistantPart::Thinking { thinking, thinking_signature, .. }) = self.output.content.get_mut(index) {
                    if !summary.is_empty() {
                        *thinking = summary;
                    } else if !content.is_empty() {
                        *thinking = content;
                    }
                    *thinking_signature = Some(item.to_string());
                    final_text = thinking.clone();
                }
                self.reasoning_blocks.insert(text(&item["id"]).to_owned(), index);
                push(&self.output, sink, |partial| StreamEvent::ThinkingEnd { content_index: index, content: final_text, partial });
            }
            ("message", Some(Slot::Text(index))) => {
                let joined: String = item["content"]
                    .as_array()
                    .map(|parts| {
                        parts
                            .iter()
                            .map(|part| if part["type"] == "output_text" { text(&part["text"]) } else { text(&part["refusal"]) })
                            .collect()
                    })
                    .unwrap_or_default();
                if let Some(AssistantPart::Text { text: slot, text_signature }) = self.output.content.get_mut(index) {
                    *slot = joined.clone();
                    *text_signature = Some(encode_text_signature(text(&item["id"]), item["phase"].as_str()));
                }
                push(&self.output, sink, |partial| StreamEvent::TextEnd { content_index: index, content: joined, partial });
            }
            ("function_call", Some(Slot::ToolCall { index, partial_json })) => {
                let raw = match item["arguments"].as_str().filter(|raw| !raw.is_empty()) {
                    Some(raw) => raw.to_owned(),
                    None if partial_json.is_empty() => "{}".to_owned(),
                    None => partial_json,
                };
                self.set_arguments(index, parse_streaming_json(&raw));
                let call = self.output.content[index].clone();
                push(&self.output, sink, |partial| StreamEvent::ToolcallEnd { content_index: index, tool_call: call, partial });
            }
            (_, Some(slot)) => {
                self.slots.insert(output_index, slot);
            }
            (_, None) => {}
        }
    }

    pub fn handle(&mut self, event: &Value, sink: &EventSink, model: &Model) -> Result<(), String> {
        let output_index = event["output_index"].as_u64().unwrap_or(0);
        match text(&event["type"]) {
            "response.created" => {
                set_response_id(&mut self.output, event["response"]["id"].as_str());
            }
            "response.output_item.added" => self.create_slot(output_index, &event["item"], sink),
            "response.reasoning_summary_text.delta" | "response.reasoning_text.delta" => {
                self.delta(output_index, text(&event["delta"]), true, sink)
            }
            "response.reasoning_summary_part.done" => self.delta(output_index, "\n\n", true, sink),
            "response.output_text.delta" | "response.refusal.delta" => self.delta(output_index, text(&event["delta"]), false, sink),
            "response.function_call_arguments.delta" => {
                let Some(Slot::ToolCall { index, partial_json }) = self.slots.get_mut(&output_index) else { return Ok(()) };
                let index = *index;
                partial_json.push_str(text(&event["delta"]));
                let arguments = parse_streaming_json(partial_json);
                self.set_arguments(index, arguments);
                toolcall_delta(&self.output, sink, index, text(&event["delta"]));
            }
            "response.function_call_arguments.done" => {
                let Some(Slot::ToolCall { index, partial_json }) = self.slots.get_mut(&output_index) else { return Ok(()) };
                let index = *index;
                let previous = std::mem::replace(partial_json, text(&event["arguments"]).to_owned());
                let arguments = parse_streaming_json(text(&event["arguments"]));
                self.set_arguments(index, arguments);
                if let Some(delta) = text(&event["arguments"]).strip_prefix(previous.as_str()).filter(|delta| !delta.is_empty()) {
                    toolcall_delta(&self.output, sink, index, delta);
                }
            }
            "response.output_item.done" => self.item_done(output_index, &event["item"], sink),
            "response.completed" | "response.incomplete" => self.finalize(&event["response"], model)?,
            "error" => return Err(format!("Error Code {}: {}", js_string(&event["code"]), js_string(&event["message"]))),
            "response.failed" => {
                self.saw_terminal = true;
                if let Some(status) = event["response"]["status"].as_str() {
                    self.output.extra.insert("rawStopReason".into(), json!(status));
                }
                let error = &event["response"]["error"];
                let reason = event["response"]["incomplete_details"]["reason"].as_str();
                return Err(if error.is_object() {
                    format!(
                        "{}: {}",
                        error["code"].as_str().filter(|code| !code.is_empty()).unwrap_or("unknown"),
                        error["message"].as_str().filter(|message| !message.is_empty()).unwrap_or("no message")
                    )
                } else if let Some(reason) = reason {
                    format!("incomplete: {reason}")
                } else {
                    "Unknown error (no error details in response)".into()
                });
            }
            _ => {}
        }
        Ok(())
    }

    pub fn finish(&self) -> Result<(), String> {
        if self.saw_terminal { Ok(()) } else { Err("OpenAI Responses stream ended before a terminal response event".into()) }
    }
}
