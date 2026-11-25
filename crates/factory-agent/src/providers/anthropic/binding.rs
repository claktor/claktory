//! Thinking bound to what came before it (Anthropic's preserved thinking:
//! Claude Fable 5.1 and later, Mythos, Opus 5.5, Sonnet 5.5, Haiku 5.5).
//! Those models record in each thinking block's signature the system
//! prompt, the tools and the messages before it, and check them when the
//! block comes back: a block made under another system prompt is refused
//! ("bound to a different conversation", Fable 5.2) or reaches the model
//! unchecked (Haiku 5.5).
//!
//! Claktory changes an agent's system prompt mid-conversation when the
//! person changes its instructions, name or project, and its tools when they
//! change its model or project (AGENTS.md, prompt caching). So each
//! response notes what it was made under (`BINDING`: a digest of the system
//! prompt and tools as sent), and a request leaves out the thinking of every
//! response made under other ones, and all thinking before it: the oldest
//! blocks may be left out, never one between others. The cache from there
//! on went with the system prompt anyway. A request refused all the same
//! (a conversation from before these notes, say) is sent again once with no
//! earlier thinking at all, and its response notes that (`CUT`), so what it
//! left out stays out.
//!
//! Every Anthropic model takes a turn without its earlier thinking, even
//! one whose tool call is still open (measured on Haiku 4.5, Sonnet 4.6 and
//! Haiku 5.5), so this needs no list of the models that bind.

use serde_json::{Map, Value};
use sha2::{Digest, Sha256};

use crate::types::{AssistantMessage, AssistantPart, Message, Model};

/// The digest of the system prompt and tools a response was made under.
const BINDING: &str = "anthropicBinding";
/// Its request left out every earlier thinking block.
const CUT: &str = "anthropicThinkingCut";

/// What a request's thinking is bound to: its system prompt and tools as
/// sent, cache markers aside (they aren't content) and keys in order (the
/// API compares parsed JSON).
pub(super) fn digest(system: Option<&Value>, tools: Option<&Value>) -> String {
    let mut both = Map::new();
    both.insert("system".into(), system.map_or(Value::Null, canonical));
    both.insert("tools".into(), tools.map_or(Value::Null, canonical));
    let bytes = serde_json::to_vec(&Value::Object(both)).unwrap_or_default();
    Sha256::digest(bytes).iter().take(16).map(|byte| format!("{byte:02x}")).collect()
}

fn canonical(value: &Value) -> Value {
    match value {
        Value::Object(object) => {
            let mut keys: Vec<&String> = object.keys().filter(|key| *key != "cache_control").collect();
            keys.sort();
            Value::Object(keys.into_iter().map(|key| (key.clone(), canonical(&object[key]))).collect())
        }
        Value::Array(items) => Value::Array(items.iter().map(canonical).collect()),
        other => other.clone(),
    }
}

/// Leaves out the thinking `model` can't take back under `binding`: every
/// block up to the last response of its own made under another, or before
/// the last one whose request left them all out; with `all`, every one.
pub(super) fn leave_out(messages: &mut [Message], model: &Model, binding: &str, all: bool) {
    let mut cut = if all { messages.len() } else { 0 };
    for (index, message) in messages.iter().enumerate() {
        let Message::Assistant(assistant) = message else { continue };
        if !same_model(assistant, model) {
            continue;
        }
        if assistant.extra.get(CUT).and_then(Value::as_bool) == Some(true) {
            cut = cut.max(index);
        }
        if assistant.extra.get(BINDING).and_then(Value::as_str).is_some_and(|made| made != binding) {
            cut = cut.max(index + 1);
        }
    }
    for message in &mut messages[..cut] {
        if let Message::Assistant(assistant) = message
            && same_model(assistant, model)
        {
            assistant.content.retain(|part| !matches!(part, AssistantPart::Thinking { .. }));
        }
    }
}

/// Notes on a response what it was made under, and whether its request
/// left out all earlier thinking.
pub(super) fn mark(output: &mut AssistantMessage, binding: &str, all: bool) {
    output.extra.insert(BINDING.into(), Value::String(binding.to_owned()));
    if all {
        output.extra.insert(CUT.into(), Value::Bool(true));
    }
}

/// Whether Anthropic refused a request for a thinking block bound to
/// another conversation: the one error to know by its words, as Anthropic
/// says, and only by these.
pub(super) fn refused(status: u16, text: &str) -> bool {
    status == 400 && text.contains("bound to a different conversation")
}

fn same_model(assistant: &AssistantMessage, model: &Model) -> bool {
    assistant.provider == model.provider && assistant.api == model.api && assistant.model == model.id
}
