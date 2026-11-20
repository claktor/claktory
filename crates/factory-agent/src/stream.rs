use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::Map;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::clock::now_ms;
use crate::errors::Error;
use crate::tools::ToolSpec;
use crate::types::{AssistantMessage, Message, Model, StopReason, StreamEvent, Usage};

pub type BoxFuture<T> = Pin<Box<dyn Future<Output = T> + Send>>;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ThinkingLevel {
    Off,
    Minimal,
    Low,
    Medium,
    High,
    Xhigh,
    Max,
}

impl ThinkingLevel {
    pub const ALL: [ThinkingLevel; 7] = [
        ThinkingLevel::Off,
        ThinkingLevel::Minimal,
        ThinkingLevel::Low,
        ThinkingLevel::Medium,
        ThinkingLevel::High,
        ThinkingLevel::Xhigh,
        ThinkingLevel::Max,
    ];

    pub fn parse(text: &str) -> Option<ThinkingLevel> {
        ThinkingLevel::ALL.into_iter().find(|level| level.as_str() == text)
    }

    pub fn as_str(self) -> &'static str {
        match self {
            ThinkingLevel::Off => "off",
            ThinkingLevel::Minimal => "minimal",
            ThinkingLevel::Low => "low",
            ThinkingLevel::Medium => "medium",
            ThinkingLevel::High => "high",
            ThinkingLevel::Xhigh => "xhigh",
            ThinkingLevel::Max => "max",
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct LlmContext {
    pub system_prompt: Option<String>,
    pub messages: Vec<Message>,
    pub tools: Vec<ToolSpec>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Transport {
    #[default]
    Auto,
    Sse,
}

/// How long the provider keeps a request's prompt cache, where a request can
/// choose (Anthropic's `cache_control.ttl`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum CacheTtl {
    /// The provider's default: 5 minutes on Anthropic. Nothing is sent.
    #[default]
    #[serde(rename = "5m")]
    FiveMinutes,
    /// An hour, at a higher price for writing the cache.
    #[serde(rename = "1h")]
    OneHour,
}

impl CacheTtl {
    pub fn as_str(self) -> &'static str {
        match self {
            CacheTtl::FiveMinutes => "5m",
            CacheTtl::OneHour => "1h",
        }
    }

    pub fn parse(text: &str) -> Option<CacheTtl> {
        match text.trim() {
            "5m" => Some(CacheTtl::FiveMinutes),
            "1h" => Some(CacheTtl::OneHour),
            _ => None,
        }
    }

    pub fn millis(self) -> u64 {
        match self {
            CacheTtl::FiveMinutes => 5 * 60_000,
            CacheTtl::OneHour => 60 * 60_000,
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct StreamOptions {
    pub cancel: CancellationToken,
    pub session_id: Option<String>,
    pub reasoning: Option<ThinkingLevel>,
    pub timeout_ms: Option<u64>,
    pub max_retries: Option<u32>,
    pub api_key: Option<String>,
    pub base_url: Option<String>,
    pub headers: Vec<(String, String)>,
    pub transport: Transport,
    /// How long the prompt cache is kept, where the provider lets a request ask.
    pub cache_ttl: CacheTtl,
}

#[derive(Clone, Debug)]
pub struct EventSink {
    tx: mpsc::UnboundedSender<StreamEvent>,
}

impl EventSink {
    pub fn push(&self, event: StreamEvent) {
        let _ = self.tx.send(event);
    }

    pub fn finish(&self, mut output: AssistantMessage, outcome: Result<(), String>, cancelled: bool) {
        match outcome {
            Ok(()) => self.push(StreamEvent::Done { reason: output.stop_reason, message: output }),
            Err(message) => {
                output.stop_reason = if cancelled { StopReason::Aborted } else { StopReason::Error };
                output.error_message = Some(message);
                self.push(StreamEvent::Error { reason: output.stop_reason, error: output });
            }
        }
    }
}

pub fn pending_message(model: &Model) -> AssistantMessage {
    AssistantMessage {
        content: Vec::new(),
        api: model.api.clone(),
        provider: model.provider.clone(),
        model: model.id.clone(),
        usage: Usage::empty(),
        stop_reason: StopReason::Pending,
        error_message: None,
        timestamp: now_ms(),
        extra: Map::new(),
    }
}

#[derive(Debug)]
pub struct EventStream {
    rx: mpsc::UnboundedReceiver<StreamEvent>,
}

impl EventStream {
    pub async fn next(&mut self) -> Option<StreamEvent> {
        self.rx.recv().await
    }

    pub fn try_next(&mut self) -> Option<StreamEvent> {
        self.rx.try_recv().ok()
    }

    /// The `done` or `error` event's message; None when the producer stopped without one.
    pub async fn result(mut self) -> Option<AssistantMessage> {
        while let Some(event) = self.next().await {
            match event {
                StreamEvent::Done { message, .. } => return Some(message),
                StreamEvent::Error { error, .. } => return Some(error),
                _ => {}
            }
        }
        None
    }
}

pub fn event_stream() -> (EventSink, EventStream) {
    let (tx, rx) = mpsc::unbounded_channel();
    (EventSink { tx }, EventStream { rx })
}

pub type StreamFn = Arc<dyn Fn(&Model, LlmContext, StreamOptions) -> Result<EventStream, Error> + Send + Sync>;

impl StreamEvent {
    /// The assistant message the event carries: the partial for deltas, the final one for `done` and `error`.
    pub fn message(&self) -> &AssistantMessage {
        match self {
            StreamEvent::Start { partial }
            | StreamEvent::TextStart { partial, .. }
            | StreamEvent::TextDelta { partial, .. }
            | StreamEvent::TextEnd { partial, .. }
            | StreamEvent::ThinkingStart { partial, .. }
            | StreamEvent::ThinkingDelta { partial, .. }
            | StreamEvent::ThinkingEnd { partial, .. }
            | StreamEvent::ToolcallStart { partial, .. }
            | StreamEvent::ToolcallDelta { partial, .. }
            | StreamEvent::ToolcallEnd { partial, .. } => partial,
            StreamEvent::Done { message, .. } => message,
            StreamEvent::Error { error, .. } => error,
        }
    }

    pub fn is_terminal(&self) -> bool {
        matches!(self, StreamEvent::Done { .. } | StreamEvent::Error { .. })
    }
}
