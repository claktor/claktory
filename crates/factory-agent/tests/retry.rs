mod support;

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use factory_agent::stream::event_stream;
use factory_agent::{
    Agent, AgentEvent, AgentOptions, AssistantMessage, AssistantPart, Message, StopReason, StreamEvent, StreamFn, ThinkingLevel,
};
use parking_lot::Mutex;
use support::{collect, event_names, luna, reply};

/// Streams partial text, then fails with `failures[n]` on the n-th request while there is one, and succeeds after.
/// Records how many messages each request carried.
fn flaky(failures: &'static [&'static str]) -> (StreamFn, Arc<Mutex<Vec<usize>>>) {
    let requests = Arc::new(Mutex::new(Vec::new()));
    let seen = requests.clone();
    let count = AtomicUsize::new(0);
    let stream: StreamFn = Arc::new(move |model, context, _options| {
        seen.lock().push(context.messages.len());
        let attempt = count.fetch_add(1, Ordering::SeqCst);
        let mut message = reply(model, vec![AssistantPart::Text { text: format!("attempt {attempt}"), text_signature: None }]);
        let (sink, stream) = event_stream();
        sink.push(StreamEvent::Start { partial: message.clone() });
        match failures.get(attempt) {
            Some(failure) => {
                message.stop_reason = StopReason::Error;
                message.error_message = Some((*failure).into());
                sink.push(StreamEvent::Error { reason: StopReason::Error, error: message });
            }
            None => sink.push(StreamEvent::Done { reason: StopReason::Stop, message }),
        }
        Ok(stream)
    });
    (stream, requests)
}

fn agent(stream: StreamFn) -> Agent {
    Agent::new(AgentOptions {
        system_prompt: "You are a coding agent.".into(),
        model: luna(),
        thinking_level: ThinkingLevel::Off,
        tools: Vec::new(),
        messages: Vec::new(),
        stream,
        session_id: None,
        transform_context: None,
        should_stop_after_turn: None,
        before_tool_call: None,
        compact_at: None,
        inbox: None,
        after_compaction: None,
    })
}

fn retries(events: &[AgentEvent]) -> Vec<(u32, u64)> {
    let mut found = Vec::new();
    for event in events {
        if let AgentEvent::Retrying { attempt, delay_ms, .. } = event {
            found.push((*attempt, *delay_ms));
        }
    }
    found
}

fn assistants(messages: &[Message]) -> Vec<&AssistantMessage> {
    let mut found = Vec::new();
    for message in messages {
        if let Message::Assistant(assistant) = message {
            found.push(assistant);
        }
    }
    found
}

#[tokio::test(start_paused = true)]
async fn a_transient_failure_is_retried_with_backoff_and_leaves_no_trace_in_the_context() {
    let (stream, requests) = flaky(&["503 Service Unavailable", "Response guard: no response events for 120000ms"]);
    let mut agent = agent(stream);
    let events = collect(&mut agent);
    agent.prompt("hello").await.unwrap();
    // Every attempt is sent the same context: the failed ones never enter it.
    assert_eq!(*requests.lock(), [1, 1, 1]);
    let events = events.lock().clone();
    assert_eq!(retries(&events), [(1, 1000), (2, 2000)]);
    let names = event_names(&events);
    assert_eq!(names.iter().filter(|name| *name == "retrying").count(), 2);
    assert_eq!(names.iter().filter(|name| *name == "message_start").count(), 4);
    assert_eq!(names.iter().filter(|name| *name == "message_end").count(), 2);
    let [answer] = assistants(&agent.state.messages)[..] else { panic!("{:?}", agent.state.messages) };
    assert_eq!(
        (answer.stop_reason, answer.content.clone()),
        (StopReason::Stop, vec![AssistantPart::Text { text: "attempt 2".into(), text_signature: None }])
    );
    assert_eq!(agent.state.error_message, None);
}

#[tokio::test(start_paused = true)]
async fn retries_stop_after_two_and_terminal_failures_are_not_retried() {
    let cases: [(&'static [&'static str], usize, &str); 2] = [
        (&["overloaded", "overloaded", "overloaded", "overloaded"], 3, "overloaded"),
        (&["401 invalid api key"], 1, "401 invalid api key"),
    ];
    for (failures, attempts, error) in cases {
        let (stream, requests) = flaky(failures);
        let mut agent = agent(stream);
        agent.prompt("hello").await.unwrap();
        assert_eq!(requests.lock().len(), attempts, "{error}");
        assert_eq!(agent.state.error_message.as_deref(), Some(error));
        let [failure] = assistants(&agent.state.messages)[..] else { panic!("{:?}", agent.state.messages) };
        assert_eq!(failure.stop_reason, StopReason::Error);
    }
}

#[tokio::test(start_paused = true)]
async fn aborting_during_the_backoff_ends_the_run_as_aborted() {
    let (stream, requests) = flaky(&["503", "503"]);
    let mut agent = agent(stream);
    let abort = agent.abort_handle();
    let events = collect(&mut agent);
    let run = tokio::spawn(async move {
        agent.prompt("hello").await.unwrap();
        agent
    });
    while !event_names(&events.lock()).iter().any(|name| name == "retrying") {
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    abort.abort();
    let agent = run.await.unwrap();
    assert_eq!(requests.lock().len(), 1);
    let [aborted] = assistants(&agent.state.messages)[..] else { panic!("{:?}", agent.state.messages) };
    assert_eq!(aborted.stop_reason, StopReason::Aborted);
}
