use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use crate::support::{collect, event_names, luna, reply};
use factory_agent::compaction::{COMPACTION_PROMPT, IDLE_PREAMBLE, REQUESTED_PREAMBLE, Reason, SUMMARY_PREAMBLE};
use factory_agent::stream::{LlmContext, event_stream};
use factory_agent::{
    Agent, AgentEvent, AgentOptions, AssistantMessage, AssistantPart, Message, StopReason, StreamEvent, StreamFn, ThinkingLevel,
    UserContent, UserPart, coding_tools,
};
use parking_lot::Mutex;
use serde_json::json;

type Requests = Arc<Mutex<Vec<Vec<Message>>>>;

/// A provider stand-in: answers each request with `respond` and records every request's messages.
fn scripted(respond: impl Fn(usize, &LlmContext) -> AssistantMessage + Send + Sync + 'static) -> (StreamFn, Requests) {
    let requests: Requests = Arc::new(Mutex::new(Vec::new()));
    let seen = requests.clone();
    let count = AtomicUsize::new(0);
    let stream: StreamFn = Arc::new(move |_model, context, _options| {
        seen.lock().push(context.messages.clone());
        let message = respond(count.fetch_add(1, Ordering::SeqCst), &context);
        let (sink, stream) = event_stream();
        sink.push(StreamEvent::Start { partial: message.clone() });
        match message.stop_reason {
            StopReason::Error => sink.push(StreamEvent::Error { reason: StopReason::Error, error: message }),
            reason => sink.push(StreamEvent::Done { reason, message }),
        }
        Ok(stream)
    });
    (stream, requests)
}

fn options(cwd: &Path, stream: StreamFn, compact_at: u64) -> AgentOptions {
    AgentOptions {
        system_prompt: "You are a coding agent.".into(),
        model: luna(),
        thinking_level: ThinkingLevel::Medium,
        tools: coding_tools(cwd, &[], None),
        messages: Vec::new(),
        stream,
        session_id: None,
        transform_context: None,
        should_stop_after_turn: None,
        before_tool_call: None,
        compact_at: Some(compact_at),
        inbox: None,
        after_compaction: None,
        secrets: None,
    }
}

/// `reply` reports 100 input, 10 cache-read, 2 cache-write and 20 output tokens.
const REPLY_CONTEXT_TOKENS: u64 = 132;

fn text(text: &str) -> AssistantMessage {
    reply(&luna(), vec![AssistantPart::Text { text: text.into(), text_signature: None }])
}

fn user_text(message: &Message) -> Option<String> {
    let Message::User(user) = message else { return None };
    Some(match &user.content {
        UserContent::Text(text) => text.clone(),
        UserContent::Parts(parts) => parts.iter().map(|part| if let UserPart::Text { text } = part { text.as_str() } else { "" }).collect(),
    })
}

fn is_compaction(context: &LlmContext) -> bool {
    context.messages.last().and_then(user_text).as_deref() == Some(COMPACTION_PROMPT)
}

fn compacted(events: &[AgentEvent]) -> Vec<(String, u64, Vec<Message>)> {
    let mut found = Vec::new();
    for event in events {
        if let AgentEvent::Compacted { summary, tokens_before, messages, .. } = event {
            found.push((summary.clone(), *tokens_before, messages.clone()));
        }
    }
    found
}

#[tokio::test]
async fn reaching_the_threshold_compacts_before_the_next_model_call_and_keeps_the_new_prompt_verbatim() {
    for (compact_at, compacts) in [(REPLY_CONTEXT_TOKENS + 1, false), (REPLY_CONTEXT_TOKENS, true)] {
        let dir = tempfile::tempdir().unwrap();
        let (stream, requests) =
            scripted(
                |_, context| {
                    if is_compaction(context) { text("Notes first.\n<summary>The user said hello.</summary>") } else { text("Hi.") }
                },
            );
        let mut agent = Agent::new(options(dir.path(), stream, compact_at));
        let events = collect(&mut agent);
        agent.prompt("hello").await.unwrap();
        agent.prompt("what next?").await.unwrap();
        let requests = requests.lock().clone();
        let events = events.lock().clone();
        if !compacts {
            assert_eq!(requests.len(), 2);
            assert!(compacted(&events).is_empty());
            assert_eq!(agent.state.messages.len(), 4);
            continue;
        }
        // The summary request carries the whole context, then the instructions.
        assert_eq!(requests.len(), 3);
        let summarized: Vec<Option<String>> = requests[1].iter().map(user_text).collect();
        assert_eq!(summarized, [Some("hello".to_owned()), None, Some(COMPACTION_PROMPT.to_owned())]);
        let summary_text = format!("{SUMMARY_PREAMBLE}\n\nThe user said hello.");
        let sent: Vec<Option<String>> = requests[2].iter().map(user_text).collect();
        assert_eq!(sent, [Some(summary_text.clone()), Some("what next?".to_owned())]);
        let [(summary, tokens_before, messages)] = compacted(&events).try_into().unwrap();
        assert_eq!((summary.as_str(), tokens_before), ("The user said hello.", REPLY_CONTEXT_TOKENS));
        assert_eq!(messages, requests[2]);
        assert_eq!(agent.state.messages[..2], messages[..]);
        assert!(matches!(&agent.state.messages[2..], [Message::Assistant(answer)] if answer.stop_reason == StopReason::Stop));
    }
}

#[tokio::test]
async fn compaction_between_tool_rounds_replaces_the_context_with_the_summary_and_the_loop_continues() {
    let dir = tempfile::tempdir().unwrap();
    let (stream, requests) = scripted(|index, context| match index {
        0 => reply(
            &luna(),
            vec![AssistantPart::ToolCall {
                id: "call-1".into(),
                name: "write_file".into(),
                arguments: json!({"path": "notes.txt", "content": "draft"}),
                thought_signature: None,
            }],
        ),
        1 => {
            assert!(is_compaction(context));
            text("<summary>notes.txt holds the draft; next, review it.</summary>")
        }
        _ => text("Reviewed."),
    });
    let mut agent = Agent::new(options(dir.path(), stream, REPLY_CONTEXT_TOKENS));
    let events = collect(&mut agent);
    agent.prompt("write a draft to notes.txt, then review it").await.unwrap();
    assert_eq!(std::fs::read_to_string(dir.path().join("notes.txt")).unwrap(), "draft");
    let requests = requests.lock().clone();
    assert_eq!(requests.len(), 3);
    // The tool round is summarized, including its result.
    assert!(matches!(requests[1][..], [Message::User(_), Message::Assistant(_), Message::ToolResult(_), Message::User(_)]));
    // The next model call sees exactly the summary, which tells it to carry on.
    let [summary] = &requests[2][..] else { panic!("{:?}", requests[2]) };
    let summary = user_text(summary).unwrap();
    assert!(summary.starts_with(&format!("{SUMMARY_PREAMBLE}\n\nnotes.txt holds the draft; next, review it.\n\n")), "{summary}");
    assert!(summary.contains("Continue the task"), "{summary}");
    let events = events.lock().clone();
    let names = event_names(&events);
    let at = names.iter().position(|name| name == "compacted").unwrap();
    assert_eq!(names[at - 3..at + 4], ["message_end", "turn_end", "turn_start", "compacted", "message_start", "message_end", "turn_end"]);
    let [(_, tokens_before, messages)] = compacted(&events).try_into().unwrap();
    assert_eq!(tokens_before, REPLY_CONTEXT_TOKENS);
    assert_eq!(messages, requests[2]);
    assert_eq!(agent.state.messages.len(), 2);
    assert_eq!(agent.state.messages[0], messages[0]);
    assert!(matches!(&agent.state.messages[1], Message::Assistant(done) if done.stop_reason == StopReason::Stop));
    assert_eq!(agent.state.error_message, None);
}

#[tokio::test]
async fn a_failed_compaction_ends_the_run_like_a_model_failure_and_keeps_the_context() {
    let dir = tempfile::tempdir().unwrap();
    let (stream, requests) = scripted(|_, context| {
        if is_compaction(context) {
            let mut failed = text("");
            failed.stop_reason = StopReason::Error;
            failed.error_message = Some("400 prompt is too long".into());
            failed
        } else {
            text("Hi.")
        }
    });
    let mut agent = Agent::new(options(dir.path(), stream, REPLY_CONTEXT_TOKENS));
    agent.prompt("hello").await.unwrap();
    let before = agent.state.messages.clone();
    let events = collect(&mut agent);
    agent.prompt("again").await.unwrap();
    assert_eq!(requests.lock().len(), 2);
    assert_eq!(agent.state.error_message.as_deref(), Some("Compaction failed: 400 prompt is too long"));
    let events = events.lock().clone();
    assert!(compacted(&events).is_empty());
    assert_eq!(event_names(&events).last().map(String::as_str), Some("agent_end"));
    // The old context and the new prompt stay, followed by the failure.
    assert_eq!(agent.state.messages[..2], before[..]);
    assert_eq!(user_text(&agent.state.messages[2]).as_deref(), Some("again"));
    assert!(matches!(&agent.state.messages[3], Message::Assistant(failure) if failure.stop_reason == StopReason::Error));
    assert_eq!(agent.state.messages.len(), 4);
}

#[tokio::test]
async fn after_compaction_the_run_goes_on_with_the_system_prompt_the_hook_gives() {
    let dir = tempfile::tempdir().unwrap();
    let prompts = Arc::new(Mutex::new(Vec::new()));
    let seen = prompts.clone();
    let (stream, _) = scripted(move |_, context| {
        seen.lock().push(context.system_prompt.clone().unwrap_or_default());
        if is_compaction(context) { text("<summary>The user said hello.</summary>") } else { text("Hi.") }
    });
    let mut options = options(dir.path(), stream, REPLY_CONTEXT_TOKENS);
    let calls = Arc::new(AtomicUsize::new(0));
    let counted = calls.clone();
    options.after_compaction = Some(Arc::new(move || {
        counted.fetch_add(1, Ordering::SeqCst);
        Box::pin(async { Some(factory_agent::NewContext { system_prompt: "Fresh prompt.".to_owned(), tools: Vec::new() }) })
    }));
    let mut agent = Agent::new(options);
    agent.prompt("hello").await.unwrap();
    agent.prompt("what next?").await.unwrap();
    // The summary is written under the old prompt, so its cache still applies.
    assert_eq!(*prompts.lock(), ["You are a coding agent.", "You are a coding agent.", "Fresh prompt."]);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(agent.state.system_prompt, "Fresh prompt.");
}

#[tokio::test]
async fn compact_summarizes_now_whatever_the_size() {
    let dir = tempfile::tempdir().unwrap();
    let (stream, requests) =
        scripted(|_, context| if is_compaction(context) { text("<summary>The user said hello.</summary>") } else { text("Hi.") });
    let mut agent = Agent::new(options(dir.path(), stream, 1_000_000));
    assert_eq!(agent.compact().await.unwrap_err().to_string(), "Nothing to compact yet.");
    let events = collect(&mut agent);
    agent.prompt("hello").await.unwrap();
    agent.compact().await.unwrap();
    let [(summary, tokens_before, messages)] = compacted(&events.lock()).try_into().unwrap();
    assert_eq!((summary.as_str(), tokens_before), ("The user said hello.", REPLY_CONTEXT_TOKENS));
    assert_eq!(agent.state.messages, messages);
    assert_eq!(requests.lock().len(), 2);
    agent.prompt("what next?").await.unwrap();
    let sent: Vec<Option<String>> = requests.lock()[2].iter().map(user_text).collect();
    assert_eq!(sent, [Some(format!("{REQUESTED_PREAMBLE}\n\nThe user said hello.")), Some("what next?".to_owned())]);
}

#[tokio::test]
async fn compacting_while_idle_says_so_carries_its_usage_and_asks_nothing_more() {
    let dir = tempfile::tempdir().unwrap();
    let (stream, requests) =
        scripted(|_, context| if is_compaction(context) { text("<summary>The user said hello.</summary>") } else { text("Hi.") });
    let mut agent = Agent::new(options(dir.path(), stream, 1_000_000));
    agent.prompt("hello").await.unwrap();
    let events = collect(&mut agent);
    agent.compact_for(Reason::Idle).await.unwrap();
    let events = events.lock().clone();
    assert_eq!(event_names(&events), ["compacted"], "no run: nothing else happens");
    let Some(AgentEvent::Compacted { messages, usage, reason, .. }) = events.first() else { panic!("{events:?}") };
    assert_eq!(*reason, Reason::Idle);
    // What writing the summary took, as the provider reported it.
    assert_eq!(factory_agent::compaction::context_tokens(usage), REPLY_CONTEXT_TOKENS);
    let [summary] = &messages[..] else { panic!("{messages:?}") };
    assert_eq!(user_text(summary), Some(format!("{IDLE_PREAMBLE}\n\nThe user said hello.")), "not told to carry on");
    assert_eq!(requests.lock().len(), 2);
}

#[tokio::test]
async fn notes_only_for_this_context_are_summarized_as_sent_but_not_kept() {
    let dir = tempfile::tempdir().unwrap();
    let (stream, requests) =
        scripted(|_, context| if is_compaction(context) { text("<summary>The user said hello.</summary>") } else { text("Hi.") });
    let mut agent = Agent::new(options(dir.path(), stream, 1_000_000));
    let note = factory_agent::Event {
        source: factory_agent::EventSource::Host,
        from: "Factory".into(),
        id: String::new(),
        summary: String::new(),
        transient: true,
    };
    agent
        .prompt_messages(vec![
            factory_agent::UserMessage::event("Your settings changed.", note, 1),
            factory_agent::UserMessage::text("hello", 2),
        ])
        .await
        .unwrap();
    agent.compact().await.unwrap();
    // The summary is asked for after the context exactly as it was last
    // sent, so the provider's prompt cache covers all of it…
    let requests = requests.lock().clone();
    assert_eq!(requests[1][..2], requests[0][..]);
    assert_eq!(user_text(&requests[1][3]).as_deref(), Some(COMPACTION_PROMPT));
    // …and the note isn't in the new context: the next one's setup says it.
    let kept: Vec<String> = agent.state.messages.iter().filter_map(user_text).collect();
    assert_eq!(kept, [format!("{REQUESTED_PREAMBLE}\n\nThe user said hello.")]);
}
