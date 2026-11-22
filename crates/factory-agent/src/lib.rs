//! Factory's coding agent: a sequential tool loop over streaming model providers, with
//! per-agent model choice, coding tools, credential storage and automatic context compaction.

pub mod accounts;
pub mod agent;
pub mod auth;
pub mod clock;
pub mod compaction;
pub mod credentials;
pub mod errors;
pub mod estimate;
pub mod guard;
pub mod json_parse;
pub mod limits;
pub mod login;
pub mod oauth;
pub mod providers;
pub mod reasoning;
pub mod refusal;
pub mod schema;
pub mod secrets;
pub mod sse;
pub mod stream;
pub mod text;
pub mod tools;
pub mod types;
pub mod usage;

pub use agent::{AbortHandle, AfterCompaction, Agent, AgentOptions, Arrived, DetachHandle, Inbox, Listener, NewContext, Settings, WaitOut};
pub use errors::Error;
pub use login::login_terminal;
pub use providers::models::{ModelInfo, Registry};
pub use secrets::{Redaction, Redactor, SecretUse, SecretVariable, Secrets};
pub use stream::{CacheTtl, StreamFn, ThinkingLevel};
pub use tools::bash::{Background, Finished, Task};
pub use tools::{Handler, Tool, ToolSpec, coding_prompt, coding_tools};
pub use types::{
    AgentEvent, AssistantMessage, AssistantPart, Attachment, Event, EventSource, Message, Model, Provenance, SentFrom, StopReason, StreamEvent,
    SystemMessage, ToolResult, ToolResultMessage, ToolResultPart, Usage, UserContent, UserMessage, UserPart, Voice,
};
