use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Cost {
    pub input: f64,
    pub output: f64,
    pub cache_read: f64,
    pub cache_write: f64,
    pub total: f64,
}

impl Cost {
    pub const ZERO: Cost = Cost { input: 0.0, output: 0.0, cache_read: 0.0, cache_write: 0.0, total: 0.0 };
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Usage {
    pub input: u64,
    pub output: u64,
    pub cache_read: u64,
    pub cache_write: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_write_1h: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning: Option<u64>,
    pub total_tokens: u64,
    pub cost: Cost,
}

impl Usage {
    pub fn empty() -> Usage {
        Usage::default()
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CostTier {
    pub input_tokens_above: u64,
    pub input: f64,
    pub output: f64,
    pub cache_read: f64,
    pub cache_write: f64,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelCost {
    pub input: f64,
    pub output: f64,
    pub cache_read: f64,
    pub cache_write: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tiers: Option<Vec<CostTier>>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Model {
    pub id: String,
    pub name: String,
    pub api: String,
    pub provider: String,
    #[serde(default)]
    pub base_url: String,
    #[serde(default)]
    pub reasoning: bool,
    #[serde(default)]
    pub input: Vec<String>,
    pub cost: ModelCost,
    pub context_window: u64,
    pub max_tokens: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thinking_level_map: Option<Map<String, Value>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compat: Option<Map<String, Value>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub headers: Option<Map<String, Value>>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "role", rename_all = "camelCase")]
pub enum Message {
    User(UserMessage),
    Assistant(AssistantMessage),
    ToolResult(ToolResultMessage),
    /// A harness note, such as the remaining budget, sent as a system turn where the provider has one.
    System(SystemMessage),
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SystemMessage {
    #[serde(rename = "content", alias = "text")]
    pub text: String,
    pub timestamp: u64,
}

impl Message {
    pub fn timestamp(&self) -> u64 {
        match self {
            Message::User(message) => message.timestamp,
            Message::Assistant(message) => message.timestamp,
            Message::ToolResult(message) => message.timestamp,
            Message::System(message) => message.timestamp,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UserMessage {
    pub content: UserContent,
    pub timestamp: u64,
    /// Set when the person didn't write it: it's an event the harness delivered.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub event: Option<Event>,
    /// The person's: the device they sent it from, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sent_from: Option<SentFrom>,
    /// The person's: the id their app made up for this send, so it can match
    /// the message it showed at once to this one.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub client_id: String,
}

/// The device the person sent a message from, which the model reads with it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SentFrom {
    /// "Matt's iPhone", "Studio".
    pub device: String,
    /// "desktop", "laptop", "phone" or "tablet"; empty when the app didn't say.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub kind: String,
}

impl SentFrom {
    /// `<sent from="Matt's iPhone" device="phone">`: how the model reads it.
    fn tag(&self) -> String {
        let attribute = |value: &str| value.replace('"', "'").replace(['\n', '\r'], " ");
        let kind = if self.kind.is_empty() { String::new() } else { format!(" device=\"{}\"", attribute(&self.kind)) };
        format!("<sent from=\"{}\"{kind}>\n", attribute(&self.device))
    }
}

impl UserMessage {
    pub fn text(text: impl Into<String>, timestamp: u64) -> UserMessage {
        UserMessage {
            content: UserContent::Parts(vec![UserPart::Text { text: text.into() }]),
            timestamp,
            event: None,
            sent_from: None,
            client_id: String::new(),
        }
    }

    pub fn event(text: impl Into<String>, event: Event, timestamp: u64) -> UserMessage {
        UserMessage { event: Some(event), ..UserMessage::text(text, timestamp) }
    }

    /// What the model reads: an event comes wrapped in a tag naming where it's
    /// from, so it's never mistaken for the person; the person's message, in
    /// one naming the device they sent it from, when known.
    pub fn for_model(&self) -> UserMessage {
        let (open, close) = match (&self.event, &self.sent_from) {
            (Some(event), _) => {
                // An agent's id too, when its name doesn't say it, so a reply reaches it.
                let id = if event.id.is_empty() || event.id == event.from { String::new() } else { format!(" id=\"{}\"", event.id) };
                (format!("<event source=\"{}\" from=\"{}\"{id}>\n", event.source.name(), event.from), "\n</event>")
            }
            (None, Some(sent_from)) => (sent_from.tag(), "\n</sent>"),
            (None, None) => return self.clone(),
        };
        let content = match &self.content {
            UserContent::Text(text) => UserContent::Text(format!("{open}{text}{close}")),
            UserContent::Parts(parts) => {
                let mut wrapped = Vec::with_capacity(parts.len() + 2);
                wrapped.push(UserPart::Text { text: open });
                wrapped.extend(parts.iter().cloned());
                wrapped.push(UserPart::Text { text: close.to_owned() });
                UserContent::Parts(wrapped)
            }
        };
        UserMessage { content, timestamp: self.timestamp, event: None, sent_from: None, client_id: String::new() }
    }
}

/// Something other than the person prompting the agent: the program running
/// it, another agent, or a background task finishing.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Event {
    pub source: EventSource,
    /// Who it's from, for the model and the person: "Factory", an agent's name, a task's id.
    pub from: String,
    /// The sending agent's id, or the task's.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub id: String,
    /// One line for people; the text is for the model.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub summary: String,
    /// Only for the context it's in: compaction leaves it out, because the
    /// next context's setup says the same (such as a change to the agent's
    /// settings).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub transient: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum EventSource {
    /// The program running the agent.
    Host,
    Agent,
    Task,
    /// A monitor the agent (or another agent) set, firing.
    Monitor,
}

impl EventSource {
    pub fn name(self) -> &'static str {
        match self {
            EventSource::Host => "host",
            EventSource::Agent => "agent",
            EventSource::Task => "task",
            EventSource::Monitor => "monitor",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum UserContent {
    Text(String),
    Parts(Vec<UserPart>),
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum UserPart {
    Text { text: String },
    Image { data: String, mime_type: String },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AssistantMessage {
    pub content: Vec<AssistantPart>,
    pub api: String,
    pub provider: String,
    pub model: String,
    pub usage: Usage,
    pub stop_reason: StopReason,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_message: Option<String>,
    pub timestamp: u64,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum StopReason {
    Stop,
    Length,
    ToolUse,
    Error,
    Aborted,
    Pending,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum AssistantPart {
    Text {
        text: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        text_signature: Option<String>,
    },
    Thinking {
        thinking: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        thinking_signature: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        redacted: Option<bool>,
    },
    ToolCall {
        id: String,
        name: String,
        arguments: Value,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        thought_signature: Option<String>,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolResultMessage {
    pub tool_call_id: String,
    pub tool_name: String,
    pub content: Vec<ToolResultPart>,
    #[serde(default, skip_serializing_if = "Value::is_null")]
    pub details: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<Value>,
    pub is_error: bool,
    /// When it ended, in ms since 1970.
    pub timestamp: u64,
    /// When the tool began running, in ms since 1970; None in transcripts
    /// written before it was recorded.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum ToolResultPart {
    Text { text: String },
    Image { data: String, mime_type: String },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolResult {
    pub content: Vec<ToolResultPart>,
    #[serde(default, skip_serializing_if = "Value::is_null")]
    pub details: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub terminate: Option<bool>,
}

impl ToolResult {
    pub fn text(text: impl Into<String>, details: Value) -> ToolResult {
        ToolResult { content: vec![ToolResultPart::Text { text: text.into() }], details, usage: None, terminate: None }
    }

    pub fn error(message: impl Into<String>) -> ToolResult {
        ToolResult::text(message, Value::Object(Map::new()))
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", rename_all_fields = "camelCase")]
pub enum StreamEvent {
    Start { partial: AssistantMessage },
    TextStart { content_index: usize, partial: AssistantMessage },
    TextDelta { content_index: usize, delta: String, partial: AssistantMessage },
    TextEnd { content_index: usize, content: String, partial: AssistantMessage },
    ThinkingStart { content_index: usize, partial: AssistantMessage },
    ThinkingDelta { content_index: usize, delta: String, partial: AssistantMessage },
    ThinkingEnd { content_index: usize, content: String, partial: AssistantMessage },
    ToolcallStart { content_index: usize, partial: AssistantMessage },
    ToolcallDelta { content_index: usize, delta: String, partial: AssistantMessage },
    ToolcallEnd { content_index: usize, tool_call: AssistantPart, partial: AssistantMessage },
    Done { reason: StopReason, message: AssistantMessage },
    Error { reason: StopReason, error: AssistantMessage },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", rename_all_fields = "camelCase")]
pub enum AgentEvent {
    AgentStart,
    TurnStart,
    MessageStart {
        message: Message,
    },
    MessageUpdate {
        message: Message,
        assistant_message_event: Box<StreamEvent>,
    },
    MessageEnd {
        message: Message,
    },
    ToolExecutionStart {
        tool_call_id: String,
        tool_name: String,
        args: Value,
    },
    ToolExecutionUpdate {
        tool_call_id: String,
        tool_name: String,
        args: Value,
        partial_result: ToolResult,
    },
    ToolExecutionEnd {
        tool_call_id: String,
        tool_name: String,
        result: ToolResult,
        is_error: bool,
    },
    TurnEnd {
        message: Message,
        tool_results: Vec<Message>,
    },
    AgentEnd {
        messages: Vec<Message>,
    },
    /// The response that the last `MessageStart` began failed transiently. It ends that message in place of
    /// `MessageEnd` and is not part of the context; the same request is sent again after `delay_ms`.
    Retrying {
        message: Message,
        attempt: u32,
        delay_ms: u64,
    },
    /// The context was replaced by a summary of it. `messages` is the new context, which `state.messages` now
    /// holds: the summary as a user message, followed by the run's prompt when it was the latest message.
    Compacted {
        summary: String,
        tokens_before: u64,
        messages: Vec<Message>,
        /// What writing the summary took: a model request like any other.
        #[serde(default)]
        usage: Usage,
        #[serde(default)]
        reason: crate::compaction::Reason,
    },
}

/// `JSON.stringify` equality for values that may have gone through `f64`.
pub fn same_json(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Number(x), Value::Number(y)) => x.as_f64() == y.as_f64(),
        (Value::Array(x), Value::Array(y)) => x.len() == y.len() && x.iter().zip(y).all(|(x, y)| same_json(x, y)),
        (Value::Object(x), Value::Object(y)) => x.len() == y.len() && x.iter().all(|(key, x)| y.get(key).is_some_and(|y| same_json(x, y))),
        _ => a == b,
    }
}
