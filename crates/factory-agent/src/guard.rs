use std::sync::{Arc, LazyLock};
use std::time::Duration;

use regex::Regex;
use serde_json::Value;
use tokio::time::{Instant, sleep};
use tokio_util::sync::CancellationToken;

pub use crate::stream::pending_message;
use crate::stream::{EventSink, LlmContext, StreamFn, StreamOptions, event_stream};
use crate::types::{AssistantMessage, AssistantPart, Model, StopReason, StreamEvent, same_json};

/// How the agent guards and retries responses. There is no overall deadline: a long response is fine as long
/// as events keep arriving. The idle limit matches Codex CLI's stream idle timeout, since reasoning models can
/// stream nothing for minutes while they think.
#[derive(Clone, Debug, PartialEq)]
pub struct ResponsePolicy {
    pub idle_ms: u64,
    pub max_json_whitespace: usize,
    pub max_recoveries: u32,
    pub retry_delay_ms: u64,
}

impl Default for ResponsePolicy {
    fn default() -> ResponsePolicy {
        ResponsePolicy { idle_ms: 300_000, max_json_whitespace: 8192, max_recoveries: 2, retry_delay_ms: 1000 }
    }
}

static TERMINAL: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)quota|usage limit|out of credit|out of budget|billing|insufficient|unauthori[sz]ed|invalid api key|authentication|invalid_grant|invalid_client|refresh_token_(?:reused|expired|invalidated)|app_session_terminated")
        .unwrap_or_else(|_| unreachable!())
});
static RECOVERABLE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)^Response guard:|websocket (error|closed|idle timeout)|socket hang up|connection (reset|closed|error)|request timed out|error decoding response body|error reading a body from connection|stream ended unexpectedly|ECONNRESET|ETIMEDOUT|fetch failed|operation was aborted|overloaded|temporarily unavailable|service unavailable|internal server error|provider finish_reason: (?:error|network_error)|upstream idle timeout|\b(?:429|500|502|503|504|529)\b|stream (?:ended|closed)|without a terminal response|OAuth refresh failed")
        .unwrap_or_else(|_| unreachable!())
});

pub fn recoverable_response(reason: &str) -> bool {
    !TERMINAL.is_match(reason) && RECOVERABLE.is_match(reason)
}

/// A failure worth waiting out at length (#490): one that passes, unless the
/// provider refused the request as it is (its text starts with a 4xx status
/// other than 408, 409 or 429), which no wait changes, whatever number its
/// text mentions.
pub fn lasting_outage(reason: &str) -> bool {
    let status = reason.trim_start().get(..3).filter(|code| code.bytes().all(|byte| byte.is_ascii_digit()));
    let refused = status.is_some_and(|code| code.starts_with('4') && !matches!(code, "408" | "409" | "429"))
        && !reason.trim_start()[3..].starts_with(|c: char| c.is_ascii_digit());
    recoverable_response(reason) && !refused
}

/// Observe raw JSON before a permissive partial-JSON parser can salvage a broken call.
#[derive(Default)]
struct ToolArguments {
    text: String,
    quoted: bool,
    escaped: bool,
    whitespace: usize,
}

impl ToolArguments {
    fn append(&mut self, delta: &str, max_whitespace: usize) -> Result<(), String> {
        self.text.push_str(delta);
        for ch in delta.chars() {
            if self.quoted {
                if (ch as u32) < 32 {
                    return Err("unescaped control character in tool JSON".into());
                }
                if self.escaped {
                    self.escaped = false;
                } else if ch == '\\' {
                    self.escaped = true;
                } else if ch == '"' {
                    self.quoted = false;
                }
            } else {
                if ch == '"' {
                    self.quoted = true;
                }
                self.whitespace = if matches!(ch, ' ' | '\t' | '\r' | '\n') { self.whitespace + 1 } else { 0 };
                if self.whitespace >= max_whitespace {
                    return Err("runaway whitespace in tool JSON".into());
                }
            }
        }
        Ok(())
    }

    fn verify(&self, args: &Value) -> Result<(), String> {
        // A call without arguments may stream none at all.
        if self.text.trim().is_empty() && args.as_object().is_some_and(serde_json::Map::is_empty) {
            return Ok(());
        }
        let parsed: Value = serde_json::from_str(&self.text).map_err(|_| "tool arguments are not complete JSON".to_owned())?;
        if !parsed.is_object() || !same_json(&parsed, args) {
            return Err("tool arguments disagree with the streamed JSON object".into());
        }
        Ok(())
    }
}

fn fail(sink: &EventSink, controller: &CancellationToken, partial: &AssistantMessage, reason: String, aborted: bool) {
    let mut error = partial.clone();
    error.stop_reason = if aborted { StopReason::Aborted } else { StopReason::Error };
    error.error_message = Some(reason);
    sink.push(StreamEvent::Error { reason: error.stop_reason, error });
    controller.cancel();
}

async fn guard_task(inner: StreamFn, model: Model, context: LlmContext, request: StreamOptions, policy: ResponsePolicy, sink: EventSink) {
    let controller = CancellationToken::new();
    let mut partial = pending_message(&model);
    if request.cancel.is_cancelled() {
        return fail(&sink, &controller, &partial, "Invocation interrupted during response".into(), true);
    }
    let options = StreamOptions { cancel: controller.clone(), timeout_ms: Some(policy.idle_ms), max_retries: Some(0), ..request.clone() };
    let mut source = match inner(&model, context, options) {
        Ok(source) => source,
        Err(error) => return fail(&sink, &controller, &partial, error.to_string(), false),
    };
    let mut arguments: Vec<(usize, ToolArguments)> = Vec::new();
    let idle = sleep(Duration::from_millis(policy.idle_ms));
    tokio::pin!(idle);
    loop {
        let event = tokio::select! {
            () = &mut idle => return fail(&sink, &controller, &partial, "Response guard: stream idle deadline exceeded".into(), false),
            () = request.cancel.cancelled() => return fail(&sink, &controller, &partial, "Invocation interrupted during response".into(), true),
            event = source.next() => event,
        };
        let Some(event) = event else {
            return fail(&sink, &controller, &partial, "Response guard: stream ended without a terminal response".into(), false);
        };
        idle.as_mut().reset(Instant::now() + Duration::from_millis(policy.idle_ms));
        partial = event.message().clone();
        // Keep the offending delta in the durable transcript too.
        if !event.is_terminal() {
            sink.push(event.clone());
        }
        let checked = match &event {
            StreamEvent::ToolcallStart { content_index, .. } => {
                arguments.retain(|(index, _)| index != content_index);
                arguments.push((*content_index, ToolArguments::default()));
                Ok(())
            }
            StreamEvent::ToolcallDelta { content_index, delta, .. } => match arguments.iter_mut().find(|(index, _)| index == content_index)
            {
                Some((_, args)) => args.append(delta, policy.max_json_whitespace),
                None => {
                    let mut args = ToolArguments::default();
                    let result = args.append(delta, policy.max_json_whitespace);
                    arguments.push((*content_index, args));
                    result
                }
            },
            StreamEvent::ToolcallEnd { content_index, tool_call: AssistantPart::ToolCall { arguments: call, .. }, .. } => {
                arguments.iter().find(|(index, _)| index == content_index).map_or(Ok(()), |(_, args)| args.verify(call))
            }
            StreamEvent::Done { message, .. } => arguments.iter().try_for_each(|(index, args)| match message.content.get(*index) {
                Some(AssistantPart::ToolCall { arguments: call, .. }) => args.verify(call),
                _ => Err("streamed tool call missing from final response".to_owned()),
            }),
            _ => Ok(()),
        };
        if let Err(reason) = checked {
            return fail(&sink, &controller, &partial, format!("Response guard: {reason}"), false);
        }
        if event.is_terminal() {
            sink.push(event);
            return;
        }
    }
}

/// A stalled provider must not prevent cancellation or hold the invocation open indefinitely.
pub fn guard_responses(inner: StreamFn, policy: ResponsePolicy) -> StreamFn {
    Arc::new(move |model, context, request| {
        let (sink, stream) = event_stream();
        let (inner, model, policy) = (inner.clone(), model.clone(), policy.clone());
        tokio::spawn(guard_task(inner, model, context, request, policy, sink));
        Ok(stream)
    })
}
