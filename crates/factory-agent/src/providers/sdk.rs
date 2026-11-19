use std::time::Duration;

use futures_util::StreamExt;
use serde_json::Value;
use tokio_util::sync::CancellationToken;

use crate::sse::{SseDecoder, SseMessage};
use crate::stream::StreamOptions;

const MAX_ERROR_BODY_CHARS: usize = 4000;
/// The SDKs' default time until the response headers arrive.
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(600);

pub fn truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(flag) => *flag,
        Value::Number(number) => number.as_f64() != Some(0.0),
        Value::String(text) => !text.is_empty(),
        _ => true,
    }
}

fn truncate(text: String, max_chars: usize) -> String {
    let count = text.chars().count();
    if count <= max_chars {
        return text;
    }
    format!("{}... [truncated {} chars]", text.chars().take(max_chars).collect::<String>(), count - max_chars)
}

/// The SDKs' `APIError.makeMessage`: the error's message, else the error as JSON, else the raw text.
pub fn api_error_message(status: Option<u16>, error: &Value, fallback: Option<&str>) -> String {
    let message = match error.get("message") {
        Some(message) if truthy(message) => Some(message.as_str().map_or_else(|| message.to_string(), str::to_owned)),
        _ if truthy(error) => Some(error.to_string()),
        _ => fallback.filter(|text| !text.is_empty()).map(str::to_owned),
    };
    match (status, message) {
        (Some(status), Some(message)) => format!("{status} {message}"),
        (Some(status), None) => format!("{status} status code (no body)"),
        (None, Some(message)) => message,
        (None, None) => "(no status code or body)".into(),
    }
}

/// What the OpenAI SDK's `APIError` carries and Pi's `normalizeProviderError` reads back.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProviderError {
    pub status: Option<u16>,
    pub message: String,
    pub body: Option<String>,
    pub raw_metadata: Option<String>,
}

fn body_of(error: &Value) -> Option<String> {
    match error {
        Value::Object(object) if !object.is_empty() => Some(truncate(error.to_string(), MAX_ERROR_BODY_CHARS)),
        _ => None,
    }
}

fn raw_metadata(error: &Value) -> Option<String> {
    let raw = &error["metadata"]["raw"];
    truthy(raw).then(|| raw.as_str().map_or_else(|| raw.to_string(), str::to_owned))
}

/// An HTTP error response: the JSON body's `error`, or the raw text when the body is not JSON.
pub fn status_error(status: u16, text: &str) -> ProviderError {
    let parsed = serde_json::from_str::<Value>(text).ok().filter(truthy);
    let error = parsed.as_ref().map_or(Value::Null, |parsed| parsed["error"].clone());
    let fallback = if parsed.is_some() { None } else { Some(text) };
    ProviderError {
        status: Some(status),
        message: api_error_message(Some(status), &error, fallback),
        body: body_of(&error),
        raw_metadata: raw_metadata(&error),
    }
}

/// An `error` object inside the SSE data; the SDK raises it without a status.
pub fn stream_error(error: &Value) -> ProviderError {
    ProviderError { status: None, message: api_error_message(None, error, None), body: body_of(error), raw_metadata: raw_metadata(error) }
}

impl ProviderError {
    /// Pi's `formatProviderError`.
    pub fn format(&self, prefix: Option<&str>) -> String {
        let carries = self.body.as_ref().is_none_or(|body| self.message.contains(body.as_str()));
        match (self.status, &self.body) {
            (Some(status), Some(body)) if !carries => match prefix {
                Some(prefix) => format!("{prefix} ({status}): {body}"),
                None => format!("{status}: {body}"),
            },
            (Some(status), _) if prefix.is_some() => format!("{} ({status}): {}", prefix.unwrap_or_default(), self.message),
            _ => self.message.clone(),
        }
    }
}

pub fn request_timeout(options: &StreamOptions) -> Duration {
    options.timeout_ms.filter(|ms| *ms > 0).map_or(DEFAULT_TIMEOUT, Duration::from_millis)
}

pub fn transport_error(error: &reqwest::Error) -> String {
    if error.is_timeout() { "Request timed out.".into() } else { "Connection error.".into() }
}

pub async fn send_request(builder: reqwest::RequestBuilder, options: &StreamOptions) -> Result<reqwest::Response, String> {
    tokio::select! {
        sent = tokio::time::timeout(request_timeout(options), builder.send()) => match sent {
            Ok(Ok(response)) => Ok(response),
            Ok(Err(error)) => Err(transport_error(&error)),
            Err(_) => Err("Request timed out.".into()),
        },
        () = options.cancel.cancelled() => Err("Request aborted".into()),
    }
}

pub enum StreamFailure {
    Api(ProviderError),
    Other(String),
}

fn dispatch(messages: Vec<SseMessage>, done: &mut bool, handle: &mut impl FnMut(Value) -> Result<(), String>) -> Result<(), StreamFailure> {
    for message in messages {
        if *done {
            continue;
        }
        if message.data.starts_with("[DONE]") {
            *done = true;
            continue;
        }
        let event: Value = serde_json::from_str(&message.data).map_err(|error| StreamFailure::Other(error.to_string()))?;
        if let Some(error) = event.get("error").filter(|error| truthy(error)) {
            return Err(StreamFailure::Api(stream_error(error)));
        }
        handle(event).map_err(StreamFailure::Other)?;
    }
    Ok(())
}

/// The OpenAI SDK's stream: `[DONE]` ends it, a data `error` object raises, a cancel ends the body silently.
pub async fn read_sdk_stream(
    response: reqwest::Response,
    cancel: &CancellationToken,
    mut handle: impl FnMut(Value) -> Result<(), String>,
) -> Result<(), StreamFailure> {
    let mut body = response.bytes_stream();
    let mut decoder = SseDecoder::new();
    let mut done = false;
    loop {
        let chunk = tokio::select! {
            chunk = body.next() => chunk,
            () = cancel.cancelled() => break,
        };
        let Some(chunk) = chunk else { break };
        let chunk = chunk.map_err(|error| StreamFailure::Other(error.to_string()))?;
        dispatch(decoder.push(&chunk), &mut done, &mut handle)?;
    }
    dispatch(decoder.flush(), &mut done, &mut handle)
}
