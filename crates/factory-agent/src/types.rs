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
    /// The person's: files they attached, which the model reads as tags
    /// after the text (images, when it takes them, are in `content` too).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attachments: Vec<Attachment>,
    /// The person said it: a voice message, `attachments[0]` its recording,
    /// the text what was heard.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub voice: Option<Voice>,
}

/// A voice message, transcribed on the computer Claktory runs on.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Voice {
    /// How long the recording is.
    pub duration_ms: u64,
    /// The language heard, such as "pl", when the model said.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub language: String,
}

impl Voice {
    /// How the model reads its length: "42s", "3m 5s".
    fn length(&self) -> String {
        let seconds = self.duration_ms.div_ceil(1000).max(1);
        match (seconds / 60, seconds % 60) {
            (0, seconds) => format!("{seconds}s"),
            (minutes, 0) => format!("{minutes}m"),
            (minutes, seconds) => format!("{minutes}m {seconds}s"),
        }
    }
}

/// A file the person attached to a message, uploaded to the agent's uploads
/// folder.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Attachment {
    pub path: String,
    pub name: String,
    /// In bytes.
    pub size: u64,
    /// Such as "image/png"; empty when unknown.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub mime: String,
}

/// The largest image given to the model as an image: 3.75 MiB, 5 MiB in
/// base64, Anthropic's limit (OpenAI's is higher).
pub const MAX_IMAGE_BYTES: u64 = 3 * 1024 * 1024 + 768 * 1024;

impl Attachment {
    /// `<attachment path="…" name="…" size="…" type="…"/>`: how the model reads it.
    fn tag(&self) -> String {
        let kind = if self.mime.is_empty() { String::new() } else { format!(" type=\"{}\"", attribute(&self.mime)) };
        format!("<attachment path=\"{}\" name=\"{}\" size=\"{}\"{kind}/>", attribute(&self.path), attribute(&self.name), self.size)
    }

    /// The image type models read, from its type or, when the app gave none,
    /// its extension: PNG, JPEG, GIF or WebP; None for anything else.
    pub fn image_type(&self) -> Option<&'static str> {
        let known = ["image/png", "image/jpeg", "image/gif", "image/webp"];
        if let Some(mime) = known.iter().find(|known| self.mime.eq_ignore_ascii_case(known)) {
            return Some(mime);
        }
        // A type the app gave that isn't one of those wins over the name.
        if !self.mime.is_empty() && !self.mime.eq_ignore_ascii_case("application/octet-stream") {
            return None;
        }
        let extension = std::path::Path::new(&self.name).extension()?.to_str()?.to_ascii_lowercase();
        match extension.as_str() {
            "png" => Some("image/png"),
            "jpg" | "jpeg" => Some("image/jpeg"),
            "gif" => Some("image/gif"),
            "webp" => Some("image/webp"),
            _ => None,
        }
    }

    /// The file as an image for the model, when it's one models read and no
    /// bigger than `MAX_IMAGE_BYTES`; None otherwise, or if it can't be read.
    pub fn image(&self) -> Option<UserPart> {
        use base64::Engine as _;
        let mime_type = self.image_type()?;
        let file = std::fs::File::open(&self.path).ok()?;
        if file.metadata().ok()?.len() > MAX_IMAGE_BYTES {
            return None;
        }
        let mut bytes = Vec::new();
        std::io::Read::read_to_end(&mut std::io::Read::take(file, MAX_IMAGE_BYTES + 1), &mut bytes).ok()?;
        if bytes.len() as u64 > MAX_IMAGE_BYTES {
            return None;
        }
        Some(UserPart::Image { data: base64::engine::general_purpose::STANDARD.encode(&bytes), mime_type: mime_type.to_owned() })
    }
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

/// `<sent from="Matt's iPhone" device="phone">`, with `voice="42s"` when
/// they said it: how the model reads who sent the person's message.
fn sent_tag(sent_from: Option<&SentFrom>, voice: Option<&Voice>) -> String {
    let mut tag = String::from("<sent");
    if let Some(sent_from) = sent_from {
        tag.push_str(&format!(" from=\"{}\"", attribute(&sent_from.device)));
        if !sent_from.kind.is_empty() {
            tag.push_str(&format!(" device=\"{}\"", attribute(&sent_from.kind)));
        }
    }
    if let Some(voice) = voice {
        tag.push_str(&format!(" voice=\"{}\"", voice.length()));
    }
    tag.push_str(">\n");
    tag
}

/// A value as it goes between an attribute's quotes in a tag the model
/// reads: a double quote becomes a single one and a line break a space, so it
/// can't end the attribute, and `<` becomes `&lt;`, so it can't start a tag.
fn attribute(value: &str) -> String {
    value.replace('"', "'").replace(['\n', '\r'], " ").replace('<', "&lt;")
}

/// The tags Claktory wraps a message in, which text inside one must not
/// open or close.
const TAGS: [&str; 3] = ["event", "sent", "attachment"];

/// Text from outside, as it goes inside an `<event>` tag: every `<` that
/// would open or close one of Claktory's tags (`</event>`, `<sent from=…>`,
/// `< /Event >` and the like) becomes `&lt;`, so a message, a task's output
/// or an app's event can't end its event early and pass what follows off as
/// the person or as another event. Text without one is returned as it is,
/// byte for byte, so conversations keep their prompt cache.
fn defuse(text: &str) -> std::borrow::Cow<'_, str> {
    let opens_tag = |at: usize| {
        let rest = text[at + 1..].trim_start();
        let rest = rest.strip_prefix('/').unwrap_or(rest).trim_start();
        TAGS.iter().any(|tag| {
            rest.get(..tag.len()).is_some_and(|name| name.eq_ignore_ascii_case(tag))
                && !rest[tag.len()..].starts_with(|c: char| c.is_alphanumeric() || c == '-' || c == '_' || c == ':')
        })
    };
    let dangerous: Vec<usize> = text.match_indices('<').map(|(at, _)| at).filter(|&at| opens_tag(at)).collect();
    if dangerous.is_empty() {
        return std::borrow::Cow::Borrowed(text);
    }
    let mut defused = String::with_capacity(text.len() + dangerous.len() * 3);
    let mut from = 0;
    for at in dangerous {
        defused.push_str(&text[from..at]);
        defused.push_str("&lt;");
        from = at + 1;
    }
    defused.push_str(&text[from..]);
    std::borrow::Cow::Owned(defused)
}

impl UserMessage {
    pub fn text(text: impl Into<String>, timestamp: u64) -> UserMessage {
        UserMessage {
            content: UserContent::Parts(vec![UserPart::Text { text: text.into() }]),
            timestamp,
            event: None,
            sent_from: None,
            client_id: String::new(),
            attachments: Vec::new(),
            voice: None,
        }
    }

    pub fn event(text: impl Into<String>, event: Event, timestamp: u64) -> UserMessage {
        UserMessage { event: Some(event), ..UserMessage::text(text, timestamp) }
    }

    /// What the model reads: an event comes wrapped in a tag naming where it's
    /// from, so it's never mistaken for the person; the person's message, in
    /// one naming the device they sent it from, when known, and saying when
    /// they said it (a voice message), with a tag for each file attached
    /// after its text. A voice message's recording isn't one: the agent reads
    /// its words.
    pub fn for_model(&self) -> UserMessage {
        let attachments = self.attachments.get(usize::from(self.voice.is_some())..).unwrap_or_default();
        let (open, close) = match (&self.event, &self.sent_from) {
            (Some(event), _) => {
                // An agent's id too, when its name doesn't say it, so a reply reaches it.
                let id =
                    if event.id.is_empty() || event.id == event.from { String::new() } else { format!(" id=\"{}\"", attribute(&event.id)) };
                (format!("<event source=\"{}\" from=\"{}\"{id}>\n", event.source.name(), attribute(&event.from)), "\n</event>")
            }
            (None, sent_from) if sent_from.is_some() || self.voice.is_some() => {
                (sent_tag(sent_from.as_ref(), self.voice.as_ref()), "\n</sent>")
            }
            (None, _) if attachments.is_empty() => return self.clone(),
            (None, _) => (String::new(), ""),
        };
        let tags = if attachments.is_empty() {
            None
        } else {
            let tags: Vec<String> = attachments.iter().map(Attachment::tag).collect();
            let written = match &self.content {
                UserContent::Text(text) => !text.is_empty(),
                UserContent::Parts(parts) => parts.iter().any(|part| matches!(part, UserPart::Text { text } if !text.is_empty())),
            };
            Some(format!("{}{}", if written { "\n" } else { "" }, tags.join("\n")))
        };
        // An event's text comes from outside (another agent, a task, an app):
        // it can't open or close a tag. The person's own text is theirs.
        let inside = |text: &str| if self.event.is_some() { defuse(text).into_owned() } else { text.to_owned() };
        let content = match &self.content {
            UserContent::Text(text) if tags.is_none() => UserContent::Text(format!("{open}{}{close}", inside(text))),
            UserContent::Text(text) => UserContent::Text(format!("{open}{}{}{close}", inside(text), tags.unwrap_or_default())),
            UserContent::Parts(parts) => {
                let mut wrapped = Vec::with_capacity(parts.len() + 3);
                if !open.is_empty() {
                    wrapped.push(UserPart::Text { text: open });
                }
                wrapped.extend(parts.iter().map(|part| match part {
                    UserPart::Text { text } => UserPart::Text { text: inside(text) },
                    other => other.clone(),
                }));
                if let Some(tags) = tags {
                    wrapped.push(UserPart::Text { text: tags });
                }
                if !close.is_empty() {
                    wrapped.push(UserPart::Text { text: close.to_owned() });
                }
                UserContent::Parts(wrapped)
            }
        };
        UserMessage { content, timestamp: self.timestamp, ..UserMessage::text("", 0) }
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
    /// An app the agent hears (its events).
    App,
    /// A pipe to another Claktory: its messages, for its lead.
    Pipe,
}

impl EventSource {
    pub fn name(self) -> &'static str {
        match self {
            EventSource::Host => "host",
            EventSource::Agent => "agent",
            EventSource::Task => "task",
            EventSource::Monitor => "monitor",
            EventSource::App => "app",
            EventSource::Pipe => "pipe",
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
    /// The response that the last `MessageStart` began failed because none of
    /// the provider's accounts can take a request now (`accounts::none_left`).
    /// It ends that message in place of `MessageEnd` and is not part of the
    /// context; the run waits until `until_ms` (ms since 1970), or, when None,
    /// until the person changes something, then sends the request again.
    Waiting {
        message: Message,
        until_ms: Option<u64>,
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
