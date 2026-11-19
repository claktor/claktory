use serde_json::Value;

use crate::types::{AssistantPart, Message, StopReason, ToolResultPart, UserContent, UserPart};
use crate::usage::calculate_context_tokens;

const CHARS_PER_TOKEN: u64 = 4;
const ESTIMATED_IMAGE_CHARS: u64 = 4800;

fn utf16_len(text: &str) -> u64 {
    crate::text::utf16_len(text) as u64
}

pub fn estimate_text_tokens(text: &str) -> u64 {
    utf16_len(text).div_ceil(CHARS_PER_TOKEN)
}

fn json_len(value: &Value) -> u64 {
    serde_json::to_string(value).map_or(0, |text| utf16_len(&text))
}

pub fn estimate_message_tokens(message: &Message) -> u64 {
    let chars: u64 = match message {
        Message::System(note) => utf16_len(&note.text),
        Message::User(user) => match &user.content {
            UserContent::Text(text) => utf16_len(text),
            UserContent::Parts(parts) => parts
                .iter()
                .map(|part| match part {
                    UserPart::Text { text } => utf16_len(text),
                    UserPart::Image { .. } => ESTIMATED_IMAGE_CHARS,
                })
                .sum(),
        },
        Message::ToolResult(result) => result
            .content
            .iter()
            .map(|part| match part {
                ToolResultPart::Text { text } => utf16_len(text),
                ToolResultPart::Image { .. } => ESTIMATED_IMAGE_CHARS,
            })
            .sum(),
        Message::Assistant(assistant) => assistant
            .content
            .iter()
            .map(|part| match part {
                AssistantPart::Text { text, .. } => utf16_len(text),
                AssistantPart::Thinking { thinking, .. } => utf16_len(thinking),
                AssistantPart::ToolCall { name, arguments, .. } => utf16_len(name) + json_len(arguments),
            })
            .sum(),
    };
    chars.div_ceil(CHARS_PER_TOKEN)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Estimate {
    pub tokens: u64,
    pub usage_tokens: u64,
    pub trailing_tokens: u64,
    pub last_usage_index: Option<usize>,
}

fn last_assistant_usage(messages: &[Message]) -> Option<(u64, usize)> {
    let mut latest_prefix: Option<u64> = None;
    let mut found = None;
    for (index, message) in messages.iter().enumerate() {
        if let Message::Assistant(assistant) = message {
            let applies = latest_prefix.is_none_or(|timestamp| assistant.timestamp >= timestamp);
            let usage = if applies { usable_usage(message) } else { None };
            if let Some(tokens) = usage {
                found = Some((tokens, index));
            }
        }
        latest_prefix = Some(latest_prefix.map_or(message.timestamp(), |timestamp| timestamp.max(message.timestamp())));
    }
    found
}

fn estimate_from_last(messages: &[Message], last: Option<(u64, usize)>) -> Estimate {
    if let Some((usage_tokens, index)) = last {
        let trailing_tokens: u64 = messages[index + 1..].iter().map(estimate_message_tokens).sum();
        return Estimate { tokens: usage_tokens + trailing_tokens, usage_tokens, trailing_tokens, last_usage_index: Some(index) };
    }
    let tokens = messages.iter().map(estimate_message_tokens).sum();
    Estimate { tokens, usage_tokens: 0, trailing_tokens: tokens, last_usage_index: None }
}

pub fn estimate_messages(messages: &[Message]) -> Estimate {
    estimate_from_last(messages, last_assistant_usage(messages))
}

fn usable_usage(message: &Message) -> Option<u64> {
    let Message::Assistant(assistant) = message else { return None };
    let tokens = calculate_context_tokens(&assistant.usage);
    (!matches!(assistant.stop_reason, StopReason::Aborted | StopReason::Error) && tokens > 0).then_some(tokens)
}

/// The headless context limit's estimate: pi-agent-core's, which takes the last usable usage whatever its timestamp.
pub fn estimate_context_tokens(messages: &[Message]) -> Estimate {
    let last = messages.iter().enumerate().rev().find_map(|(index, message)| usable_usage(message).map(|tokens| (tokens, index)));
    estimate_from_last(messages, last)
}

pub fn estimate_context(system_prompt: Option<&str>, messages: &[Message], tools: Option<&Value>) -> Estimate {
    let estimate = estimate_messages(messages);
    if estimate.last_usage_index.is_some() {
        return estimate;
    }
    let prompt_tokens = system_prompt.filter(|prompt| !prompt.is_empty()).map_or(0, estimate_text_tokens);
    let tool_tokens =
        tools.filter(|tools| !tools.as_array().is_some_and(Vec::is_empty)).map_or(0, |tools| json_len(tools).div_ceil(CHARS_PER_TOKEN));
    let prefix = prompt_tokens + tool_tokens;
    Estimate {
        tokens: estimate.tokens + prefix,
        usage_tokens: 0,
        trailing_tokens: estimate.trailing_tokens + prefix,
        last_usage_index: None,
    }
}
