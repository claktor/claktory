use serde_json::Value;

use crate::types::{AssistantPart, ImageData, Message, Model, StopReason, ToolResultPart, UserContent, UserPart};
use crate::usage::calculate_context_tokens;

const CHARS_PER_TOKEN: u64 = 4;
/// An image whose size can't be read: about what a full-size one costs.
const ESTIMATED_IMAGE_TOKENS: u64 = 1200;

/// How a provider charges an image: by its size, never its bytes (#490: a
/// count of its base64 made three screenshots a million tokens).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ImageRule {
    /// Anthropic's: scaled to a long edge of 1568 and about 1600 tokens at
    /// most, then width × height / 750. The dearest, so the one used where
    /// the model isn't known.
    #[default]
    Anthropic,
    /// OpenAI's at high detail ("auto" for large images): fitted in
    /// 2048×2048, its short side to 768, then 170 a 512-pixel tile and 85.
    OpenAi,
    /// Google's: 258 for an image within 384×384, else 258 a 768-pixel tile.
    Gemini,
}

impl ImageRule {
    /// The rule of the provider `model` goes to: its own, or through
    /// OpenRouter, its maker's.
    pub fn of(model: &Model) -> ImageRule {
        ImageRule::for_model(&model.api, &model.id)
    }

    /// The rule for a model by its API and id.
    pub fn for_model(api: &str, id: &str) -> ImageRule {
        let id = id.to_ascii_lowercase();
        if api == "anthropic-messages" || id.contains("claude") {
            ImageRule::Anthropic
        } else if api.contains("google") || id.contains("gemini") {
            ImageRule::Gemini
        } else {
            ImageRule::OpenAi
        }
    }
}

/// The width and height of the image in `bytes` (PNG, JPEG, GIF or WebP),
/// from its header; None when it isn't one.
pub fn image_size(bytes: &[u8]) -> Option<(u32, u32)> {
    image::ImageReader::new(std::io::Cursor::new(bytes)).with_guessed_format().ok()?.into_dimensions().ok()
}

/// What an image `width` × `height` costs as input under `rule`, in tokens.
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss, clippy::cast_precision_loss)] // pixel counts
pub fn image_tokens(rule: ImageRule, width: u32, height: u32) -> u64 {
    let (mut width, mut height) = (f64::from(width.max(1)), f64::from(height.max(1)));
    match rule {
        ImageRule::Anthropic => {
            const LONG_EDGE: f64 = 1568.0;
            const MOST: f64 = 1600.0;
            let scale = (LONG_EDGE / width.max(height)).min((MOST * 750.0 / (width * height)).sqrt()).min(1.0);
            (width, height) = ((width * scale).floor().max(1.0), (height * scale).floor().max(1.0));
            ((width * height / 750.0).ceil() as u64).min(MOST as u64)
        }
        ImageRule::OpenAi => {
            let fit = (2048.0 / width.max(height)).min(1.0);
            (width, height) = (width * fit, height * fit);
            let short = (768.0 / width.min(height)).min(1.0);
            (width, height) = (width * short, height * short);
            let tiles = (width / 512.0).ceil() * (height / 512.0).ceil();
            85 + 170 * tiles as u64
        }
        ImageRule::Gemini => {
            if width <= 384.0 && height <= 384.0 {
                258
            } else {
                258 * ((width / 768.0).ceil() * (height / 768.0).ceil()) as u64
            }
        }
    }
}

/// What an image in a message costs under `rule`: by its size, or about a
/// full-size one's when that can't be read.
fn image_cost(data: &ImageData, rule: ImageRule) -> u64 {
    data.dimensions().map_or(ESTIMATED_IMAGE_TOKENS, |(width, height)| image_tokens(rule, width, height))
}

fn utf16_len(text: &str) -> u64 {
    crate::text::utf16_len(text) as u64
}

pub fn estimate_text_tokens(text: &str) -> u64 {
    utf16_len(text).div_ceil(CHARS_PER_TOKEN)
}

fn json_len(value: &Value) -> u64 {
    serde_json::to_string(value).map_or(0, |text| utf16_len(&text))
}

/// A message's tokens, its images at the dearest provider's rule.
pub fn estimate_message_tokens(message: &Message) -> u64 {
    estimate_message_tokens_for(message, ImageRule::default())
}

/// A message's tokens, its images under `rule`: text at four characters a
/// token, images by their size.
pub fn estimate_message_tokens_for(message: &Message, rule: ImageRule) -> u64 {
    let mut images = 0;
    let chars: u64 = match message {
        Message::System(note) => utf16_len(&note.text),
        Message::User(user) => match &user.content {
            UserContent::Text(text) => utf16_len(text),
            UserContent::Parts(parts) => parts
                .iter()
                .map(|part| match part {
                    UserPart::Text { text } => utf16_len(text),
                    UserPart::Image { data, .. } => {
                        images += image_cost(data, rule);
                        0
                    }
                })
                .sum(),
        },
        Message::ToolResult(result) => result
            .content
            .iter()
            .map(|part| match part {
                ToolResultPart::Text { text } => utf16_len(text),
                ToolResultPart::Image { data, .. } => {
                    images += image_cost(data, rule);
                    0
                }
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
    chars.div_ceil(CHARS_PER_TOKEN) + images
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

fn estimate_from_last(messages: &[Message], last: Option<(u64, usize)>, rule: ImageRule) -> Estimate {
    let tokens = |messages: &[Message]| messages.iter().map(|message| estimate_message_tokens_for(message, rule)).sum::<u64>();
    if let Some((usage_tokens, index)) = last {
        let trailing_tokens = tokens(&messages[index + 1..]);
        return Estimate { tokens: usage_tokens + trailing_tokens, usage_tokens, trailing_tokens, last_usage_index: Some(index) };
    }
    let tokens = tokens(messages);
    Estimate { tokens, usage_tokens: 0, trailing_tokens: tokens, last_usage_index: None }
}

pub fn estimate_messages(messages: &[Message]) -> Estimate {
    estimate_from_last(messages, last_assistant_usage(messages), ImageRule::default())
}

fn usable_usage(message: &Message) -> Option<u64> {
    let Message::Assistant(assistant) = message else { return None };
    let tokens = calculate_context_tokens(&assistant.usage);
    (!matches!(assistant.stop_reason, StopReason::Aborted | StopReason::Error) && tokens > 0).then_some(tokens)
}

/// The headless context limit's estimate: pi-agent-core's, which takes the last usable usage whatever its timestamp.
pub fn estimate_context_tokens(messages: &[Message]) -> Estimate {
    let last = messages.iter().enumerate().rev().find_map(|(index, message)| usable_usage(message).map(|tokens| (tokens, index)));
    estimate_from_last(messages, last, ImageRule::default())
}

pub fn estimate_context(system_prompt: Option<&str>, messages: &[Message], tools: Option<&Value>) -> Estimate {
    estimate_context_for(ImageRule::default(), system_prompt, messages, tools)
}

/// The context's tokens for a model whose provider charges images by `rule`.
pub fn estimate_context_for(rule: ImageRule, system_prompt: Option<&str>, messages: &[Message], tools: Option<&Value>) -> Estimate {
    let estimate = estimate_from_last(messages, last_assistant_usage(messages), rule);
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
