use std::sync::Arc;
use std::time::Duration;

use crate::support::{collect, event_names, luna, reply, scripted_model};
use factory_agent::agent::{Agent, AgentOptions, Block, TRUNCATED_TOOL_CALL};
use factory_agent::guard::{ResponsePolicy, guard_responses};
use factory_agent::stream::{StreamFn, ThinkingLevel, event_stream};
use factory_agent::tools::coding_tools;
use factory_agent::types::{AgentEvent, AssistantMessage, AssistantPart, Message, StopReason, StreamEvent, ToolResultPart};
use parking_lot::Mutex;
use serde_json::{Value, json};

fn options(cwd: &std::path::Path, stream: StreamFn) -> AgentOptions {
    AgentOptions {
        system_prompt: format!("You are a coding agent. Work in {}.", cwd.display()),
        model: luna(),
        thinking_level: ThinkingLevel::Medium,
        tools: coding_tools(cwd, &[], None),
        messages: Vec::new(),
        stream,
        session_id: Some("session".into()),
        transform_context: None,
        should_stop_after_turn: None,
        before_tool_call: None,
        compact_at: None,
        inbox: None,
        after_compaction: None,
        secrets: None,
        fallback_model: None,
    }
}

/// Answers the first request with `content`, later ones with a plain text, and honors cancellation like a provider.
fn single(content: Vec<AssistantPart>, stop_reason: Option<StopReason>) -> StreamFn {
    let calls = Arc::new(Mutex::new(0));
    Arc::new(move |model, _context, options| {
        let (sink, stream) = event_stream();
        *calls.lock() += 1;
        let mut message = if *calls.lock() == 1 {
            reply(model, content.clone())
        } else {
            reply(model, vec![AssistantPart::Text { text: "Done".into(), text_signature: None }])
        };
        if let (Some(stop), 1) = (stop_reason, *calls.lock()) {
            message.stop_reason = stop;
        }
        if options.cancel.is_cancelled() {
            message.stop_reason = StopReason::Aborted;
            sink.push(StreamEvent::Error { reason: StopReason::Aborted, error: message });
            return Ok(stream);
        }
        sink.push(StreamEvent::Start { partial: message.clone() });
        sink.push(StreamEvent::Done { reason: message.stop_reason, message });
        Ok(stream)
    })
}

fn tool_result_text(message: &Message) -> String {
    let Message::ToolResult(result) = message else { panic!("tool result") };
    result
        .content
        .iter()
        .map(|part| match part {
            ToolResultPart::Text { text } => text.clone(),
            ToolResultPart::Image { .. } => String::new(),
        })
        .collect()
}

#[tokio::test]
async fn a_prompt_runs_tools_sequentially_and_emits_the_journal_events_in_order() {
    let dir = tempfile::tempdir().unwrap();
    let mut agent = Agent::new(options(dir.path(), scripted_model()));
    let events = collect(&mut agent);
    agent.prompt("# Checkpoint 1").await.unwrap();
    assert!(!agent.is_streaming());
    assert_eq!(agent.state.error_message, None);
    assert_eq!(std::fs::read_to_string(dir.path().join("main.py")).unwrap(), "print(\"one\")\n");
    let names = event_names(&events.lock());
    assert_eq!(
        names,
        [
            "agent_start",
            "turn_start",
            "message_start",
            "message_end",
            "message_start",
            "message_update",
            "message_end",
            "tool_execution_start",
            "tool_execution_end",
            "message_start",
            "message_end",
            "turn_end",
            "turn_start",
            "message_start",
            "message_update",
            "message_end",
            "turn_end",
            "agent_end",
        ]
    );
    let roles: Vec<String> =
        agent.state.messages.iter().map(|message| serde_json::to_value(message).unwrap()["role"].as_str().unwrap().to_owned()).collect();
    assert_eq!(roles, ["user", "assistant", "toolResult", "assistant"]);
    assert_eq!(tool_result_text(&agent.state.messages[2]), "Created main.py (13 characters).");
    let Message::Assistant(last) = agent.state.messages.last().unwrap() else { panic!("assistant") };
    assert_eq!(last.content, vec![AssistantPart::Text { text: "Done. Prior user prompts: 1".into(), text_signature: None }]);
    let update = events.lock().iter().find(|event| matches!(event, AgentEvent::MessageUpdate { .. })).cloned().unwrap();
    let AgentEvent::MessageUpdate { assistant_message_event, message } = update else { panic!("update") };
    assert!(matches!(*assistant_message_event, StreamEvent::TextDelta { ref delta, .. } if delta == "test delta"));
    assert!(matches!(message, Message::Assistant(_)));
    let end = events.lock().iter().find(|event| matches!(event, AgentEvent::ToolExecutionEnd { .. })).cloned().unwrap();
    let AgentEvent::ToolExecutionEnd { tool_name, is_error, result, .. } = end else { panic!("end") };
    assert_eq!((tool_name.as_str(), is_error), ("write_file", false));
    assert_eq!(result.details["existed"], false);
    agent.prompt("Review this").await.unwrap();
    assert_eq!(std::fs::read_to_string(dir.path().join("main.py")).unwrap(), "print(\"one\")\n# reviewed\n");
    assert_eq!(agent.state.messages.len(), 8);
    agent.reset();
    assert!(agent.state.messages.is_empty());
}

#[tokio::test]
async fn unknown_tools_invalid_arguments_and_truncated_calls_become_error_results() {
    let dir = tempfile::tempdir().unwrap();
    let calls = vec![
        AssistantPart::ToolCall { id: "a".into(), name: "missing".into(), arguments: json!({}), thought_signature: None },
        AssistantPart::ToolCall {
            id: "b".into(),
            name: "edit_file".into(),
            arguments: json!({"command": "touch bad"}),
            thought_signature: None,
        },
        AssistantPart::ToolCall {
            id: "c".into(),
            name: "read_file".into(),
            arguments: json!({"path": "missing.txt"}),
            thought_signature: None,
        },
    ];
    let mut agent = Agent::new(options(dir.path(), single(calls.clone(), None)));
    let events = collect(&mut agent);
    agent.prompt("go").await.unwrap();
    assert_eq!(tool_result_text(&agent.state.messages[2]), "Tool missing not found");
    assert!(tool_result_text(&agent.state.messages[3]).starts_with("Validation failed for tool \"edit_file\":\n  - path: must have required properties path, edits\n  - root: must not have additional properties"));
    assert_eq!(
        tool_result_text(&agent.state.messages[4]),
        format!("ENOENT: no such file or directory, open '{}/missing.txt'", dir.path().display())
    );
    let ends: Vec<bool> = events
        .lock()
        .iter()
        .filter_map(|event| match event {
            AgentEvent::ToolExecutionEnd { is_error, .. } => Some(*is_error),
            _ => None,
        })
        .collect();
    assert_eq!(ends, [true, true, true]);
    let end = events.lock().iter().find(|event| matches!(event, AgentEvent::ToolExecutionEnd { .. })).cloned().unwrap();
    let AgentEvent::ToolExecutionEnd { result, .. } = end else { panic!("end") };
    assert_eq!(result.details, json!({}));
    let mut agent = Agent::new(options(dir.path(), single(calls[2..].to_vec(), Some(StopReason::Length))));
    agent.prompt("go").await.unwrap();
    assert_eq!(tool_result_text(&agent.state.messages[2]), TRUNCATED_TOOL_CALL("read_file"));
    assert_eq!(
        TRUNCATED_TOOL_CALL("x"),
        "Tool call \"x\" was not executed: the response hit the output token limit, so its arguments may be truncated. Re-issue the tool call with complete arguments."
    );
}

#[tokio::test]
async fn before_tool_call_can_block_and_abort_stops_the_batch() {
    let dir = tempfile::tempdir().unwrap();
    let call = AssistantPart::ToolCall {
        id: "w".into(),
        name: "write_file".into(),
        arguments: json!({"path": "x", "content": "y"}),
        thought_signature: None,
    };
    let mut blocking = options(dir.path(), single(vec![call.clone()], None));
    blocking.before_tool_call = Some(Arc::new(|name, args| {
        Box::pin(async move {
            (name == "write_file" && args["path"] == "x").then_some(Block {
                reason: Some("Denied by user".into()),
                terminate: false,
                details: serde_json::Value::Null,
            })
        })
    }));
    let mut agent = Agent::new(blocking);
    agent.prompt("go").await.unwrap();
    assert_eq!(tool_result_text(&agent.state.messages[2]), "Denied by user");
    assert!(!dir.path().join("x").exists());
    let mut agent = Agent::new(options(dir.path(), single(vec![call], None)));
    let handle = agent.abort_handle();
    agent.subscribe(Arc::new(move |event| {
        let handle = handle.clone();
        Box::pin(async move {
            if matches!(event, AgentEvent::ToolExecutionStart { .. }) {
                handle.abort();
            }
            Ok(())
        })
    }));
    let events = collect(&mut agent);
    agent.prompt("go").await.unwrap();
    assert_eq!(tool_result_text(&agent.state.messages[2]), "Operation aborted");
    assert!(!dir.path().join("x").exists());
    let names = event_names(&events.lock());
    assert_eq!(&names[names.len() - 5..], ["turn_start", "message_start", "message_end", "turn_end", "agent_end"]);
    let Message::Assistant(last) = agent.state.messages.last().unwrap() else { panic!("assistant") };
    assert_eq!(last.stop_reason, StopReason::Aborted);
    assert_eq!(agent.state.messages.len(), 4);
}

#[tokio::test]
async fn a_failing_context_transform_becomes_a_synthetic_error_message() {
    let dir = tempfile::tempdir().unwrap();
    let mut failing = options(dir.path(), scripted_model());
    failing.transform_context = Some(Arc::new(|_messages| Err("Context limit reached; session saved".into())));
    let mut agent = Agent::new(failing);
    let events = collect(&mut agent);
    agent.prompt("go").await.unwrap();
    assert_eq!(
        event_names(&events.lock()),
        ["agent_start", "turn_start", "message_start", "message_end", "message_start", "message_end", "turn_end", "agent_end"]
    );
    assert_eq!(agent.state.error_message.as_deref(), Some("Context limit reached; session saved"));
    let Message::Assistant(failure) = agent.state.messages.last().unwrap() else { panic!("assistant") };
    assert_eq!(failure.content, vec![AssistantPart::Text { text: String::new(), text_signature: None }]);
    assert_eq!(failure.stop_reason, StopReason::Error);
    assert_eq!(failure.usage.total_tokens, 0);
    assert_eq!(
        (failure.api.as_str(), failure.provider.as_str(), failure.model.as_str()),
        ("openai-codex-responses", "openai-codex", "gpt-5.6-luna")
    );
    let error = agent.continue_run().await.unwrap_err();
    assert_eq!(error.to_string(), "Cannot continue from message role: assistant");
    agent.reset();
    assert_eq!(agent.continue_run().await.unwrap_err().to_string(), "No messages to continue from");
}

#[tokio::test]
async fn continue_resumes_after_a_tool_result_and_should_stop_ends_the_run() {
    let dir = tempfile::tempdir().unwrap();
    let stops = Arc::new(Mutex::new(0));
    let counter = stops.clone();
    let mut stopping = options(dir.path(), scripted_model());
    stopping.should_stop_after_turn = Some(Arc::new(move |message: &AssistantMessage| {
        *counter.lock() += 1;
        message.content.iter().any(|part| matches!(part, AssistantPart::ToolCall { .. }))
    }));
    let mut agent = Agent::new(stopping);
    let events = collect(&mut agent);
    agent.prompt("# Checkpoint 1").await.unwrap();
    assert_eq!(*stops.lock(), 1);
    assert_eq!(agent.state.messages.len(), 3);
    assert_eq!(event_names(&events.lock()).last().map(String::as_str), Some("agent_end"));
    agent.continue_run().await.unwrap();
    assert_eq!(agent.state.messages.len(), 4);
    assert_eq!(*stops.lock(), 2);
    let value: Value = serde_json::to_value(agent.state.messages.last().unwrap()).unwrap();
    assert_eq!(value["content"][0]["text"], "Done. Prior user prompts: 1");
}

#[tokio::test]
async fn a_dropped_run_future_leaves_the_agent_usable_and_cancels_its_provider() {
    let dir = tempfile::tempdir().unwrap();
    let calls = Arc::new(Mutex::new(0));
    let cancelled = Arc::new(Mutex::new(false));
    let (counter, flag) = (calls.clone(), cancelled.clone());
    let stalling: StreamFn = Arc::new(move |model, _context, options| {
        *counter.lock() += 1;
        let (sink, stream) = event_stream();
        if *counter.lock() == 1 {
            let flag = flag.clone();
            tokio::spawn(async move {
                options.cancel.cancelled().await;
                *flag.lock() = true;
                drop(sink);
            });
        } else {
            let message = reply(model, vec![AssistantPart::Text { text: "Done".into(), text_signature: None }]);
            sink.push(StreamEvent::Done { reason: StopReason::Stop, message });
        }
        Ok(stream)
    });
    let mut agent = Agent::new(options(dir.path(), stalling));
    assert!(tokio::time::timeout(Duration::from_millis(50), agent.prompt("go")).await.is_err());
    assert!(!agent.is_streaming());
    tokio::time::sleep(Duration::from_millis(20)).await;
    assert!(*cancelled.lock());
    agent.prompt("again").await.unwrap();
    assert_eq!(*calls.lock(), 2);
    let Message::Assistant(last) = agent.state.messages.last().unwrap() else { panic!("assistant") };
    assert_eq!(last.stop_reason, StopReason::Stop);
}

#[tokio::test]
async fn a_guarded_malformed_tool_call_never_executes_through_the_loop() {
    let dir = tempfile::tempdir().unwrap();
    let cancelled = Arc::new(Mutex::new(false));
    let flag = cancelled.clone();
    let fragments: StreamFn = Arc::new(move |model, _context, options| {
        let (sink, stream) = event_stream();
        let message = reply(
            model,
            vec![AssistantPart::ToolCall {
                id: "bad".into(),
                name: "bash".into(),
                arguments: json!({"command": "touch bad"}),
                thought_signature: None,
            }],
        );
        let flag = flag.clone();
        tokio::spawn(async move {
            sink.push(StreamEvent::Start { partial: message.clone() });
            sink.push(StreamEvent::ToolcallStart { content_index: 0, partial: message.clone() });
            sink.push(StreamEvent::ToolcallDelta {
                content_index: 0,
                delta: "{\"command\":\"touch bad\"".into(),
                partial: message.clone(),
            });
            sink.push(StreamEvent::ToolcallEnd { content_index: 0, tool_call: message.content[0].clone(), partial: message.clone() });
            sink.push(StreamEvent::Done { reason: StopReason::ToolUse, message });
            options.cancel.cancelled().await;
            *flag.lock() = true;
        });
        Ok(stream)
    });
    let mut agent = Agent::new(options(dir.path(), guard_responses(fragments, ResponsePolicy::default())));
    let events = collect(&mut agent);
    agent.prompt("go").await.unwrap();
    assert_eq!(agent.state.error_message.as_deref(), Some("Response guard: tool arguments are not complete JSON"));
    assert!(!event_names(&events.lock()).iter().any(|name| name == "tool_execution_start"));
    assert!(!dir.path().join("bad").exists());
    tokio::time::sleep(Duration::from_millis(20)).await;
    assert!(*cancelled.lock());
}

#[tokio::test]
async fn an_empty_error_message_does_not_mark_the_run_failed() {
    let dir = tempfile::tempdir().unwrap();
    let empty: StreamFn = Arc::new(|model, _context, _options| {
        let (sink, stream) = event_stream();
        let mut message = reply(model, vec![AssistantPart::Text { text: "ok".into(), text_signature: None }]);
        message.error_message = Some(String::new());
        sink.push(StreamEvent::Done { reason: StopReason::Stop, message });
        Ok(stream)
    });
    let mut agent = Agent::new(options(dir.path(), empty));
    agent.prompt("go").await.unwrap();
    assert_eq!(agent.state.error_message, None);
}

#[tokio::test]
async fn what_arrives_while_the_agent_works_is_read_between_steps() {
    use factory_agent::types::{Event, EventSource, UserMessage};
    let dir = tempfile::tempdir().expect("tempdir");
    // What each request ends with, as the model reads it.
    let seen: Arc<Mutex<Vec<String>>> = Arc::default();
    let stream: StreamFn = {
        let seen = seen.clone();
        Arc::new(move |model, context, _options| {
            let last = match context.messages.last() {
                Some(Message::User(user)) => serde_json::to_string(&user.content).expect("json"),
                Some(Message::ToolResult(_)) => "tool result".to_owned(),
                other => format!("{other:?}"),
            };
            let request = {
                let mut seen = seen.lock();
                seen.push(last);
                seen.len()
            };
            let content = if request == 1 {
                vec![AssistantPart::ToolCall {
                    id: "c1".into(),
                    name: "bash".into(),
                    arguments: json!({"command": "echo hi"}),
                    thought_signature: None,
                }]
            } else {
                vec![AssistantPart::Text { text: format!("reply {request}"), text_signature: None }]
            };
            let message = reply(model, content);
            let (sink, stream) = event_stream();
            sink.push(StreamEvent::Start { partial: message.clone() });
            sink.push(StreamEvent::Done { reason: message.stop_reason, message });
            Ok(stream)
        })
    };
    let event = Event {
        source: EventSource::Task,
        from: "task-1".into(),
        id: "task-1".into(),
        summary: String::new(),
        transient: false,
        pipe_seqs: Vec::new(),
    };
    let mut arrivals = vec![Vec::new(), vec![UserMessage::text("and also this", 2)], vec![UserMessage::event("task-1 finished", event, 1)]];
    let arrivals = Arc::new(Mutex::new(std::mem::take(&mut arrivals)));
    let mut options = options(dir.path(), stream);
    options.inbox = Some(Arc::new(move || {
        let messages = arrivals.lock().pop().unwrap_or_default();
        Box::pin(async move { factory_agent::Arrived { messages, ..Default::default() } })
    }));
    let mut agent = Agent::new(options);
    agent.prompt("start").await.expect("run");

    let seen = seen.lock().clone();
    assert_eq!(seen.len(), 3, "the run went on for what arrived: {seen:?}");
    assert_eq!(seen[0], r#"[{"type":"text","text":"start"}]"#);
    // After the tool ran, the task's event joined before the next call…
    assert!(seen[1].contains(r#"<event source=\"task\" from=\"task-1\">"#) && seen[1].contains("task-1 finished"), "{}", seen[1]);
    // …and a message that came during the last reply kept the run going.
    assert_eq!(seen[2], r#"[{"type":"text","text":"and also this"}]"#);
    let kinds: Vec<&str> = agent
        .state
        .messages
        .iter()
        .map(|message| match message {
            Message::User(user) if user.event.is_some() => "event",
            Message::User(_) => "user",
            Message::Assistant(_) => "assistant",
            Message::ToolResult(_) => "tool",
            Message::System(_) => "system",
        })
        .collect();
    assert_eq!(kinds, ["user", "assistant", "tool", "event", "assistant", "user", "assistant"]);
}
