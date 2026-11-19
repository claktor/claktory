use std::sync::Arc;
use std::time::Duration;

use parking_lot::Mutex;
use serde_json::Value;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::clock::now_ms;
use crate::compaction::{COMPACTION_PROMPT, extract_summary, latest_context_tokens, summary_message};
use crate::errors::Error;
use crate::guard::{ResponsePolicy, guard_responses, recoverable_response};
use crate::reasoning::{coalesce_history, coalesce_reasoning};
use crate::schema::validate_tool_arguments;
use crate::stream::{BoxFuture, LlmContext, StreamFn, StreamOptions, ThinkingLevel};
use crate::tools::Tool;
use crate::types::{
    AgentEvent, AssistantMessage, AssistantPart, Message, Model, StopReason, StreamEvent, ToolResult, ToolResultMessage, Usage, UserMessage,
};

pub struct Block {
    pub reason: Option<String>,
    pub terminate: bool,
}

pub type BeforeToolCall = Arc<dyn Fn(String, Value) -> BoxFuture<Option<Block>> + Send + Sync>;
pub type Listener = Arc<dyn Fn(AgentEvent) -> BoxFuture<Result<(), Error>> + Send + Sync>;
pub type TransformContext = Arc<dyn Fn(Vec<Message>) -> Result<Vec<Message>, String> + Send + Sync>;
pub type ShouldStop = Arc<dyn Fn(&AssistantMessage) -> bool + Send + Sync>;
/// What arrived while the agent worked, taken between steps so its next
/// model call has it: messages (the person's, or events), and changes to its
/// own settings.
pub type Inbox = Arc<dyn Fn() -> BoxFuture<Arrived> + Send + Sync>;

#[derive(Default)]
pub struct Arrived {
    pub messages: Vec<UserMessage>,
    pub settings: Settings,
}

/// The agent's settings for its next model call; what's None stays as it is.
#[derive(Default)]
pub struct Settings {
    pub system_prompt: Option<String>,
    pub model: Option<Model>,
    pub thinking_level: Option<ThinkingLevel>,
    pub tools: Option<Vec<Tool>>,
}

/// What a new context starts with: its system prompt and tools.
pub struct NewContext {
    pub system_prompt: String,
    pub tools: Vec<Tool>,
}

/// Called right after the context is compacted; what it returns replaces the
/// system prompt and tools for the rest of the run.
pub type AfterCompaction = Arc<dyn Fn() -> std::pin::Pin<Box<dyn Future<Output = Option<NewContext>> + Send>> + Send + Sync>;

fn is_transient(message: &Message) -> bool {
    matches!(message, Message::User(UserMessage { event: Some(event), .. }) if event.transient)
}

#[allow(non_snake_case)]
pub fn TRUNCATED_TOOL_CALL(name: &str) -> String {
    format!(
        "Tool call \"{name}\" was not executed: the response hit the output token limit, so its arguments may be truncated. Re-issue the tool call with complete arguments."
    )
}

fn assistant(message: AssistantMessage) -> Message {
    Message::Assistant(message)
}

pub struct AgentOptions {
    pub system_prompt: String,
    pub model: Model,
    pub thinking_level: ThinkingLevel,
    pub tools: Vec<Tool>,
    pub messages: Vec<Message>,
    pub stream: StreamFn,
    pub session_id: Option<String>,
    pub transform_context: Option<TransformContext>,
    pub should_stop_after_turn: Option<ShouldStop>,
    pub before_tool_call: Option<BeforeToolCall>,
    /// Compact the context before a model call once the latest response reports at least this many context tokens.
    pub compact_at: Option<u64>,
    /// Checked after every step; what it returns joins the run.
    pub inbox: Option<Inbox>,
    pub after_compaction: Option<AfterCompaction>,
}

pub struct AgentState {
    pub system_prompt: String,
    pub model: Model,
    pub thinking_level: ThinkingLevel,
    pub tools: Vec<Tool>,
    pub messages: Vec<Message>,
    pub compact_at: Option<u64>,
    pub error_message: Option<String>,
}

/// Dropping a run, finished or not, releases the agent and cancels whatever the run was waiting on.
struct ActiveRun(AbortHandle);

impl Drop for ActiveRun {
    fn drop(&mut self) {
        if let Some(token) = self.0.0.lock().take() {
            token.cancel();
        }
    }
}

#[derive(Clone, Default)]
pub struct AbortHandle(Arc<Mutex<Option<CancellationToken>>>);

impl AbortHandle {
    pub fn abort(&self) {
        if let Some(token) = self.0.lock().as_ref() {
            token.cancel();
        }
    }
}

/// Stateful wrapper around the loop: owns the transcript, emits lifecycle events, runs tools one at a time.
pub struct Agent {
    pub state: AgentState,
    stream: StreamFn,
    session_id: Option<String>,
    transform_context: Option<TransformContext>,
    should_stop_after_turn: Option<ShouldStop>,
    before_tool_call: Option<BeforeToolCall>,
    inbox: Option<Inbox>,
    after_compaction: Option<AfterCompaction>,
    listeners: Vec<Listener>,
    active: AbortHandle,
    policy: ResponsePolicy,
}

enum Prepared {
    Immediate(ToolResult),
    Ready(usize, Value),
}

struct Finalized {
    id: String,
    name: String,
    result: ToolResult,
    is_error: bool,
    /// When the call began, in ms since 1970.
    started: u64,
}

type Call = (String, String, Value);

impl Agent {
    /// Responses that stall or carry malformed tool JSON end as recoverable errors, which the loop retries.
    pub fn new(options: AgentOptions) -> Agent {
        let policy = ResponsePolicy::default();
        Agent {
            state: AgentState {
                system_prompt: options.system_prompt,
                model: options.model,
                thinking_level: options.thinking_level,
                tools: options.tools,
                messages: coalesce_history(options.messages),
                compact_at: options.compact_at,
                error_message: None,
            },
            stream: coalesce_reasoning(guard_responses(options.stream, policy.clone())),
            session_id: options.session_id,
            transform_context: options.transform_context,
            should_stop_after_turn: options.should_stop_after_turn,
            before_tool_call: options.before_tool_call,
            inbox: options.inbox,
            after_compaction: options.after_compaction,
            listeners: Vec::new(),
            active: AbortHandle::default(),
            policy,
        }
    }

    pub fn subscribe(&mut self, listener: Listener) {
        self.listeners.push(listener);
    }

    pub fn abort_handle(&self) -> AbortHandle {
        self.active.clone()
    }

    pub fn abort(&self) {
        self.active.abort();
    }

    pub fn is_streaming(&self) -> bool {
        self.active.0.lock().is_some()
    }

    pub fn reset(&mut self) {
        drop(ActiveRun(self.active.clone()));
        self.state.messages.clear();
        self.state.error_message = None;
    }

    pub async fn prompt(&mut self, text: &str) -> Result<(), Error> {
        self.prompt_messages(vec![UserMessage::text(text, now_ms())]).await
    }

    /// Runs on several prompts at once, such as messages and events that
    /// queued up while the agent was busy. Each stays its own message.
    pub async fn prompt_messages(&mut self, prompts: Vec<UserMessage>) -> Result<(), Error> {
        let mut messages = Vec::with_capacity(prompts.len());
        for prompt in prompts {
            messages.push(Message::User(prompt));
        }
        self.run(messages).await
    }

    /// Continue from the current transcript. The last message must be a user or tool-result message.
    pub async fn continue_run(&mut self) -> Result<(), Error> {
        match self.state.messages.last() {
            None => return Err(Error::Other("No messages to continue from".into())),
            Some(Message::Assistant(_)) => return Err(Error::Other("Cannot continue from message role: assistant".into())),
            Some(_) => {}
        }
        self.run(Vec::new()).await
    }

    async fn run(&mut self, prompts: Vec<Message>) -> Result<(), Error> {
        if self.active.0.lock().is_some() {
            return Err(Error::Other("Agent is already processing.".into()));
        }
        let token = CancellationToken::new();
        *self.active.0.lock() = Some(token.clone());
        let _active = ActiveRun(self.active.clone());
        self.state.error_message = None;
        let outcome = self.run_loop(prompts, &token).await;
        if let Err(error) = outcome {
            self.handle_run_failure(error, token.is_cancelled()).await?;
        }
        Ok(())
    }

    /// A listener's error unwinds the run, as a throwing listener unwinds Pi's loop.
    async fn emit(&mut self, event: AgentEvent) -> Result<(), Error> {
        match &event {
            AgentEvent::MessageEnd { message } => self.state.messages.push(message.clone()),
            AgentEvent::Compacted { messages, .. } => self.state.messages.clone_from(messages),
            AgentEvent::TurnEnd { message: Message::Assistant(assistant), .. } => {
                if let Some(error) = assistant.error_message.as_deref().filter(|error| !error.is_empty()) {
                    self.state.error_message = Some(error.to_owned());
                }
            }
            _ => {}
        }
        for listener in &self.listeners {
            listener(event.clone()).await?;
        }
        Ok(())
    }

    async fn emit_message(&mut self, message: Message) -> Result<(), Error> {
        self.emit(AgentEvent::MessageStart { message: message.clone() }).await?;
        self.emit(AgentEvent::MessageEnd { message }).await
    }

    async fn handle_run_failure(&mut self, error: Error, aborted: bool) -> Result<(), Error> {
        let failure = AssistantMessage {
            content: vec![AssistantPart::Text { text: String::new(), text_signature: None }],
            api: self.state.model.api.clone(),
            provider: self.state.model.provider.clone(),
            model: self.state.model.id.clone(),
            usage: Usage::empty(),
            stop_reason: if aborted { StopReason::Aborted } else { StopReason::Error },
            error_message: Some(error.to_string()),
            timestamp: now_ms(),
            extra: Default::default(),
        };
        let message = assistant(failure);
        self.emit_message(message.clone()).await?;
        self.emit(AgentEvent::TurnEnd { message: message.clone(), tool_results: Vec::new() }).await?;
        self.emit(AgentEvent::AgentEnd { messages: vec![message] }).await
    }

    async fn run_loop(&mut self, prompts: Vec<Message>, token: &CancellationToken) -> Result<(), Error> {
        let mut context: Vec<Message> = self.state.messages.clone();
        let mut produced: Vec<Message> = Vec::new();
        self.emit(AgentEvent::AgentStart).await?;
        self.emit(AgentEvent::TurnStart).await?;
        // Prompts at the end of the context the model hasn't answered yet.
        let mut fresh = prompts.len();
        for prompt in prompts {
            context.push(prompt.clone());
            produced.push(prompt.clone());
            self.emit_message(prompt).await?;
        }
        let mut first_turn = true;
        // The last step called tools: the model is in the middle of something.
        let mut mid_task = false;
        loop {
            if !first_turn {
                self.emit(AgentEvent::TurnStart).await?;
            }
            self.compact_if_needed(&mut context, fresh, mid_task, token).await?;
            first_turn = false;
            let message = self.respond(&mut context, token).await?;
            produced.push(assistant(message.clone()));
            if matches!(message.stop_reason, StopReason::Error | StopReason::Aborted) {
                self.emit(AgentEvent::TurnEnd { message: assistant(message.clone()), tool_results: Vec::new() }).await?;
                self.emit(AgentEvent::AgentEnd { messages: produced }).await?;
                return Ok(());
            }
            let calls: Vec<Call> = message
                .content
                .iter()
                .filter_map(|part| match part {
                    AssistantPart::ToolCall { id, name, arguments, .. } => Some((id.clone(), name.clone(), arguments.clone())),
                    _ => None,
                })
                .collect();
            let mut tool_results: Vec<Message> = Vec::new();
            let mut terminate = false;
            if !calls.is_empty() {
                // A "length" stop means every call may carry truncated arguments: fail them all instead of executing them.
                let (results, stop) = self.run_tool_calls(&calls, token, message.stop_reason == StopReason::Length).await?;
                for result in results {
                    context.push(result.clone());
                    produced.push(result.clone());
                    tool_results.push(result);
                }
                terminate = stop;
            }
            self.emit(AgentEvent::TurnEnd { message: assistant(message.clone()), tool_results }).await?;
            if terminate || self.should_stop_after_turn.as_ref().is_some_and(|stop| stop(&message)) {
                self.emit(AgentEvent::AgentEnd { messages: produced }).await?;
                return Ok(());
            }
            // Between steps: what arrived while the model and tools worked
            // goes in now, so the next call sees it.
            let arrived = match &self.inbox {
                Some(inbox) => inbox().await,
                None => Arrived::default(),
            };
            self.apply_settings(arrived.settings);
            fresh = arrived.messages.len();
            for message in arrived.messages {
                let message = Message::User(message);
                context.push(message.clone());
                produced.push(message.clone());
                self.emit_message(message).await?;
            }
            mid_task = !calls.is_empty();
            if !mid_task && fresh == 0 {
                break;
            }
        }
        self.emit(AgentEvent::AgentEnd { messages: produced }).await
    }

    fn apply_settings(&mut self, settings: Settings) {
        let Settings { system_prompt, model, thinking_level, tools } = settings;
        if let Some(system_prompt) = system_prompt {
            self.state.system_prompt = system_prompt;
        }
        if let Some(model) = model {
            self.state.model = model;
        }
        if let Some(thinking_level) = thinking_level {
            self.state.thinking_level = thinking_level;
        }
        if let Some(tools) = tools {
            self.state.tools = tools;
        }
    }

    fn llm_context(&self, context: Vec<Message>) -> Result<LlmContext, Error> {
        let mut messages = match &self.transform_context {
            Some(transform) => transform(context).map_err(Error::Other)?,
            None => context,
        };
        for message in &mut messages {
            if let Message::User(user) = message
                && user.event.is_some()
            {
                *user = user.for_model();
            }
        }
        Ok(LlmContext {
            system_prompt: Some(self.state.system_prompt.clone()),
            messages,
            tools: self.state.tools.iter().map(|tool| tool.spec.clone()).collect(),
        })
    }

    fn stream_options(&self, token: &CancellationToken) -> StreamOptions {
        let reasoning = (self.state.thinking_level != ThinkingLevel::Off).then_some(self.state.thinking_level);
        StreamOptions { cancel: token.clone(), session_id: self.session_id.clone(), reasoning, ..StreamOptions::default() }
    }

    /// How long to wait before sending a failed request again, if its failure is transient and retries remain.
    fn retry_delay(&self, response: &AssistantMessage, attempt: u32, token: &CancellationToken) -> Option<Duration> {
        let transient = response.stop_reason == StopReason::Error && response.error_message.as_deref().is_some_and(recoverable_response);
        (transient && attempt <= self.policy.max_recoveries && !token.is_cancelled())
            .then(|| Duration::from_millis(self.policy.retry_delay_ms * u64::from(attempt)))
    }

    /// One model response. Every attempt streams to listeners; one that fails transiently ends with `Retrying`
    /// instead of `MessageEnd`, leaves the context, and the request is sent again.
    async fn respond(&mut self, context: &mut Vec<Message>, token: &CancellationToken) -> Result<AssistantMessage, Error> {
        let mut attempt = 0;
        loop {
            let message = self.stream_assistant_response(context, token).await?;
            attempt += 1;
            let Some(delay) = self.retry_delay(&message, attempt, token) else {
                self.emit(AgentEvent::MessageEnd { message: assistant(message.clone()) }).await?;
                return Ok(message);
            };
            context.pop();
            let delay_ms = u64::try_from(delay.as_millis()).unwrap_or(u64::MAX);
            tracing::warn!(attempt, delay_ms, error = message.error_message.as_deref(), "retrying model response");
            self.emit(AgentEvent::Retrying { message: assistant(message), attempt, delay_ms }).await?;
            pause(delay, token).await?;
        }
    }

    /// Compacts when the latest response shows the context reached `compact_at`. With `new_prompt`, the last
    /// message is the prompt that started this run: it stays verbatim after the summary. Otherwise the summary
    /// is the whole new context and tells the model to carry on with the task.
    /// `new_prompts` are the run's prompts at the end of the context: they're
    /// kept after the summary rather than summarized.
    /// `new_prompts` at the end of the context are kept as they are; `mid_task`
    /// tells the model to carry on with what it was doing.
    async fn compact_if_needed(
        &mut self,
        context: &mut Vec<Message>,
        new_prompts: usize,
        mid_task: bool,
        token: &CancellationToken,
    ) -> Result<(), Error> {
        let Some(compact_at) = self.state.compact_at else { return Ok(()) };
        let Some(tokens_before) = latest_context_tokens(context).filter(|tokens| *tokens >= compact_at) else { return Ok(()) };
        tracing::info!(tokens_before, compact_at, "compacting context");
        self.compact_context(context, new_prompts, mid_task, tokens_before, token).await
    }

    /// Summarizes the conversation now, whatever its size, so the next run
    /// carries on from the summary.
    pub async fn compact(&mut self) -> Result<(), Error> {
        if self.state.messages.is_empty() {
            return Err(Error::Other("Nothing to compact yet.".into()));
        }
        if self.active.0.lock().is_some() {
            return Err(Error::Other("Agent is already processing.".into()));
        }
        let token = CancellationToken::new();
        *self.active.0.lock() = Some(token.clone());
        let _active = ActiveRun(self.active.clone());
        let mut context = self.state.messages.clone();
        let tokens_before = latest_context_tokens(&context).unwrap_or(0);
        tracing::info!(tokens_before, "compacting context on request");
        self.compact_context(&mut context, 0, false, tokens_before, &token).await
    }

    async fn compact_context(
        &mut self,
        context: &mut Vec<Message>,
        new_prompts: usize,
        mid_task: bool,
        tokens_before: u64,
        token: &CancellationToken,
    ) -> Result<(), Error> {
        // On failure the run ends and this context is dropped; `state.messages` is untouched.
        let mut kept = context.split_off(context.len().saturating_sub(new_prompts));
        // Notes only for this context, such as what changed in the agent's
        // settings, which the next context's setup carries anyway.
        context.retain(|message| !is_transient(message));
        kept.retain(|message| !is_transient(message));
        let summary = self.summarize(context, token).await?;
        let mut messages = vec![summary_message(&summary, mid_task, now_ms())];
        messages.extend(kept);
        context.clone_from(&messages);
        self.emit(AgentEvent::Compacted { summary, tokens_before, messages }).await?;
        if let Some(after) = &self.after_compaction
            && let Some(NewContext { system_prompt, tools }) = after().await
        {
            self.state.system_prompt = system_prompt;
            self.state.tools = tools;
        }
        Ok(())
    }

    /// Asks the model for a summary of the context. The request keeps the system prompt and tools so the
    /// provider's prompt cache covers the whole context; the prompt forbids tool calls.
    async fn summarize(&self, context: &[Message], token: &CancellationToken) -> Result<String, Error> {
        let mut messages = context.to_vec();
        messages.push(Message::User(UserMessage::text(COMPACTION_PROMPT, now_ms())));
        let mut attempt = 0;
        loop {
            let response = self.complete(messages.clone(), token).await?;
            attempt += 1;
            if let Some(delay) = self.retry_delay(&response, attempt, token) {
                tracing::warn!(attempt, error = response.error_message.as_deref(), "retrying compaction");
                pause(delay, token).await?;
                continue;
            }
            if token.is_cancelled() || response.stop_reason == StopReason::Aborted {
                return Err(Error::Aborted);
            }
            if response.stop_reason == StopReason::Error {
                return Err(Error::Compaction(response.error_message.unwrap_or_else(|| "the model returned an error".into())));
            }
            return extract_summary(&response).ok_or_else(|| Error::Compaction("the model wrote no summary".into()));
        }
    }

    /// A response collected without streaming it to listeners.
    async fn complete(&self, messages: Vec<Message>, token: &CancellationToken) -> Result<AssistantMessage, Error> {
        let mut stream = (self.stream)(&self.state.model, self.llm_context(messages)?, self.stream_options(token))?;
        while let Some(event) = stream.next().await {
            if let StreamEvent::Done { message, .. } | StreamEvent::Error { error: message, .. } = event {
                return Ok(message);
            }
        }
        Err(Error::Other("stream ended without a terminal response".into()))
    }

    /// Streams a response into the context, emitting `MessageStart` and updates; the caller ends the message.
    async fn stream_assistant_response(
        &mut self,
        context: &mut Vec<Message>,
        token: &CancellationToken,
    ) -> Result<AssistantMessage, Error> {
        let llm = self.llm_context(context.clone())?;
        let mut stream = (self.stream)(&self.state.model, llm, self.stream_options(token))?;
        let mut added = false;
        while let Some(event) = stream.next().await {
            match event {
                StreamEvent::Start { partial } => {
                    context.push(assistant(partial.clone()));
                    added = true;
                    self.emit(AgentEvent::MessageStart { message: assistant(partial) }).await?;
                }
                StreamEvent::Done { message, .. } | StreamEvent::Error { error: message, .. } => {
                    if added {
                        if let Some(last) = context.last_mut() {
                            *last = assistant(message.clone());
                        }
                    } else {
                        context.push(assistant(message.clone()));
                        self.emit(AgentEvent::MessageStart { message: assistant(message.clone()) }).await?;
                    }
                    return Ok(message);
                }
                event => {
                    if added {
                        let partial = event.message().clone();
                        if let Some(last) = context.last_mut() {
                            *last = assistant(partial.clone());
                        }
                        self.emit(AgentEvent::MessageUpdate { message: assistant(partial), assistant_message_event: Box::new(event) })
                            .await?;
                    }
                }
            }
        }
        Err(Error::Other("stream ended without a terminal response".into()))
    }

    async fn run_tool_calls(&mut self, calls: &[Call], token: &CancellationToken, truncated: bool) -> Result<(Vec<Message>, bool), Error> {
        let mut messages = Vec::new();
        let mut terminates = Vec::new();
        for (id, name, arguments) in calls {
            let started = now_ms();
            self.emit(AgentEvent::ToolExecutionStart { tool_call_id: id.clone(), tool_name: name.clone(), args: arguments.clone() })
                .await?;
            let (result, is_error) = if truncated {
                (ToolResult::error(TRUNCATED_TOOL_CALL(name)), true)
            } else {
                match self.prepare(name, arguments, token).await {
                    Prepared::Immediate(result) => (result, true),
                    Prepared::Ready(index, validated) => self.execute(index, id, name, arguments, &validated, token).await?,
                }
            };
            if !truncated {
                terminates.push(result.terminate == Some(true));
            }
            messages.push(self.finish_tool_call(Finalized { id: id.clone(), name: name.clone(), result, is_error, started }).await?);
            if !truncated && token.is_cancelled() {
                break;
            }
        }
        let terminate = !terminates.is_empty() && terminates.iter().all(|flag| *flag);
        Ok((messages, terminate))
    }

    async fn prepare(&mut self, name: &str, arguments: &Value, token: &CancellationToken) -> Prepared {
        let Some(index) = self.state.tools.iter().position(|tool| tool.spec.name == name) else {
            return Prepared::Immediate(ToolResult::error(format!("Tool {name} not found")));
        };
        let validated = match validate_tool_arguments(&self.state.tools[index].spec.parameters, name, arguments) {
            Ok(validated) => validated,
            Err(message) => return Prepared::Immediate(ToolResult::error(message)),
        };
        let block = if let Some(before) = &self.before_tool_call { before(name.to_owned(), validated.clone()).await } else { None };
        if token.is_cancelled() {
            return Prepared::Immediate(ToolResult::error("Operation aborted"));
        }
        if let Some(block) = block {
            let mut result = ToolResult::error(block.reason.unwrap_or_else(|| "Tool execution was blocked".into()));
            if block.terminate {
                result.terminate = Some(true);
            }
            return Prepared::Immediate(result);
        }
        Prepared::Ready(index, validated)
    }

    async fn execute(
        &mut self,
        index: usize,
        id: &str,
        name: &str,
        arguments: &Value,
        validated: &Value,
        token: &CancellationToken,
    ) -> Result<(ToolResult, bool), Error> {
        let tool = self.state.tools[index].clone();
        let (tx, mut rx) = mpsc::unbounded_channel::<ToolResult>();
        let update = move |partial: ToolResult| {
            let _ = tx.send(partial);
        };
        let execution = tool.execute(validated, token, &update);
        tokio::pin!(execution);
        let outcome = loop {
            tokio::select! {
                outcome = &mut execution => break outcome,
                Some(partial) = rx.recv() => self.emit(AgentEvent::ToolExecutionUpdate { tool_call_id: id.to_owned(), tool_name: name.to_owned(), args: arguments.clone(), partial_result: partial }).await?,
            }
        };
        while let Ok(partial) = rx.try_recv() {
            self.emit(AgentEvent::ToolExecutionUpdate {
                tool_call_id: id.to_owned(),
                tool_name: name.to_owned(),
                args: arguments.clone(),
                partial_result: partial,
            })
            .await?;
        }
        Ok(match outcome {
            Ok(result) => (result, false),
            Err(error) => (ToolResult::error(error.to_string()), true),
        })
    }

    async fn finish_tool_call(&mut self, finalized: Finalized) -> Result<Message, Error> {
        self.emit(AgentEvent::ToolExecutionEnd {
            tool_call_id: finalized.id.clone(),
            tool_name: finalized.name.clone(),
            result: finalized.result.clone(),
            is_error: finalized.is_error,
        })
        .await?;
        let message = Message::ToolResult(ToolResultMessage {
            tool_call_id: finalized.id,
            tool_name: finalized.name,
            content: finalized.result.content,
            details: finalized.result.details,
            usage: finalized.result.usage,
            is_error: finalized.is_error,
            timestamp: now_ms(),
            started: Some(finalized.started),
        });
        self.emit_message(message.clone()).await?;
        Ok(message)
    }
}

async fn pause(delay: Duration, token: &CancellationToken) -> Result<(), Error> {
    tokio::select! {
        () = tokio::time::sleep(delay) => Ok(()),
        () = token.cancelled() => Err(Error::Aborted),
    }
}
