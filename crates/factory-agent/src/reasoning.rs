use std::sync::Arc;

use serde_json::{Map, Value};

use crate::guard::pending_message;
use crate::stream::{StreamFn, event_stream};
use crate::types::{AssistantMessage, AssistantPart, Message, StopReason, StreamEvent};

fn is_text_detail(detail: &Map<String, Value>) -> bool {
    detail.get("type").and_then(Value::as_str) == Some("reasoning.text") && detail.get("text").is_some_and(Value::is_string)
}

/// Fragments of one reasoning block, streamed a token at a time, share these.
fn same_block(a: &Map<String, Value>, b: &Map<String, Value>) -> bool {
    let signed = |detail: &Map<String, Value>| detail.get("signature").is_some_and(|value| !value.is_null());
    a.get("id") == b.get("id") && a.get("format") == b.get("format") && a.get("index") == b.get("index") && !(signed(a) && signed(b))
}

/// Consecutive same-kind reasoning details replay as one, so the model never sees its own reasoning split per token.
pub fn coalesce_reasoning_details(signature: Option<&str>) -> Option<String> {
    let signature = signature?;
    if signature.is_empty() {
        return Some(String::new());
    }
    let unchanged = Some(signature.to_owned());
    let Ok(Value::Array(details)) = serde_json::from_str::<Value>(signature) else { return unchanged };
    if details.len() < 2 {
        return unchanged;
    }
    let Some(objects) = details
        .iter()
        .map(|detail| detail.as_object().filter(|object| object.get("type").is_some_and(Value::is_string)))
        .collect::<Option<Vec<_>>>()
    else {
        return unchanged;
    };
    let mut merged: Vec<Map<String, Value>> = Vec::new();
    for detail in objects {
        match merged.last_mut() {
            Some(last) if is_text_detail(last) && is_text_detail(detail) && same_block(last, detail) => {
                let text = format!(
                    "{}{}",
                    last.get("text").and_then(Value::as_str).unwrap_or(""),
                    detail.get("text").and_then(Value::as_str).unwrap_or("")
                );
                let signature =
                    last.get("signature").filter(|value| !value.is_null()).cloned().or_else(|| detail.get("signature").cloned());
                for (key, value) in detail {
                    last.insert(key.clone(), value.clone());
                }
                last.insert("text".into(), Value::String(text));
                match signature {
                    Some(value) => {
                        last.insert("signature".into(), value);
                    }
                    None => {
                        last.remove("signature");
                    }
                }
            }
            _ => merged.push(detail.clone()),
        }
    }
    if merged.len() == details.len() { unchanged } else { serde_json::to_string(&merged).ok() }
}

pub fn coalesce_message(message: &mut AssistantMessage) {
    for part in &mut message.content {
        if let AssistantPart::Thinking { thinking_signature, .. } = part {
            *thinking_signature = coalesce_reasoning_details(thinking_signature.as_deref());
        }
    }
}

/// A journal written before this normalisation, or by another process, still holds exploded details.
pub fn coalesce_history(mut messages: Vec<Message>) -> Vec<Message> {
    for message in &mut messages {
        if let Message::Assistant(assistant) = message {
            coalesce_message(assistant);
        }
    }
    messages
}

/// Everything a provider replays as the model's own reasoning: thinking parts, and the signatures its tool calls carry for them.
pub fn drop_reasoning(messages: &[Message]) -> Vec<Message> {
    messages
        .iter()
        .map(|message| match message {
            Message::Assistant(assistant) => {
                let content = assistant
                    .content
                    .iter()
                    .filter(|part| !matches!(part, AssistantPart::Thinking { .. }))
                    .map(|part| match part {
                        AssistantPart::ToolCall { id, name, arguments, .. } => AssistantPart::ToolCall {
                            id: id.clone(),
                            name: name.clone(),
                            arguments: arguments.clone(),
                            thought_signature: None,
                        },
                        other => other.clone(),
                    })
                    .collect();
                Message::Assistant(AssistantMessage { content, ..assistant.clone() })
            }
            other => other.clone(),
        })
        .collect()
}

/// Wrap a guarded stream: the completed message is stored and replayed, so normalise it there.
pub fn coalesce_reasoning(inner: StreamFn) -> StreamFn {
    Arc::new(move |model, context, options| {
        let (sink, stream) = event_stream();
        match inner(model, context, options) {
            Ok(mut source) => {
                tokio::spawn(async move {
                    while let Some(mut event) = source.next().await {
                        match &mut event {
                            StreamEvent::Done { message, .. } => coalesce_message(message),
                            StreamEvent::Error { error, .. } => coalesce_message(error),
                            _ => {}
                        }
                        sink.push(event);
                    }
                });
            }
            Err(error) => {
                let mut failure = pending_message(model);
                failure.stop_reason = StopReason::Error;
                failure.error_message = Some(error.to_string());
                sink.push(StreamEvent::Error { reason: StopReason::Error, error: failure });
            }
        }
        Ok(stream)
    })
}
