//! The refusal fallback: a step the model's safety classifiers
//! declined is retried once on the configured fallback model, same
//! context, with the refusal kept in the conversation but out of the
//! model's view, and the agent told in a transient note.

use std::sync::Arc;

use crate::support::{collect, event_names, luna, reply};
use factory_agent::stream::event_stream;
use factory_agent::{
    Agent, AgentOptions, AssistantMessage, AssistantPart, EventSource, Message, Model, StopReason, StreamEvent, StreamFn, ThinkingLevel,
    UserContent,
};
use parking_lot::Mutex;
use serde_json::json;

/// Each request's model and how many messages it carried.
type Requests = Arc<Mutex<Vec<(String, usize)>>>;

fn fallback() -> Model {
    Model { id: "gpt-5.4-mini".into(), name: "GPT-5.4 mini".into(), ..luna() }
}

fn refuse(model: &Model, category: &str) -> AssistantMessage {
    let mut message = reply(model, Vec::new());
    message.stop_reason = StopReason::Error;
    message.error_message = Some("I can't help with this.".into());
    factory_agent::refusal::mark(
        &mut message,
        &json!({"category": category, "explanation": "I can't help with this.", "recommended_model": null}),
    );
    message
}

/// Scripted by model: requests to models in `refusing` are refused, the
/// rest answered (with a tool call first when `tool_turns` remain).
/// Records each request's model and how many messages it carried.
fn scripted(refusing: &'static [&'static str], tool_turns: usize) -> (StreamFn, Requests) {
    let requests = Arc::new(Mutex::new(Vec::new()));
    let seen = requests.clone();
    let answers = Arc::new(Mutex::new(0usize));
    let stream: StreamFn = Arc::new(move |model, context, _options| {
        seen.lock().push((model.id.clone(), context.messages.len()));
        let (sink, stream) = event_stream();
        if refusing.contains(&model.id.as_str()) {
            sink.push(StreamEvent::Error { reason: StopReason::Error, error: refuse(model, "cyber") });
            return Ok(stream);
        }
        let mut answered = answers.lock();
        *answered += 1;
        let content = if *answered <= tool_turns {
            vec![AssistantPart::ToolCall {
                id: format!("call{answered}"),
                name: "missing".into(),
                arguments: json!({}),
                thought_signature: None,
            }]
        } else {
            vec![AssistantPart::Text { text: "the answer".into(), text_signature: None }]
        };
        let message = reply(model, content);
        sink.push(StreamEvent::Start { partial: message.clone() });
        sink.push(StreamEvent::Done { reason: StopReason::Stop, message });
        Ok(stream)
    });
    (stream, requests)
}

fn agent(stream: StreamFn, fallback_model: Option<Model>) -> Agent {
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
        secrets: None,
        fallback_model,
    })
}

fn models(requests: &Mutex<Vec<(String, usize)>>) -> Vec<String> {
    requests.lock().iter().map(|(model, _)| model.clone()).collect()
}

#[tokio::test(start_paused = true)]
async fn a_refused_step_is_retried_on_the_fallback_model_with_the_same_context() {
    let (stream, requests) = scripted(&["gpt-5.6-luna"], 0);
    let mut agent = agent(stream, Some(fallback()));
    let events = collect(&mut agent);
    agent.prompt("hello").await.unwrap();
    // Same context both times: the refusal never enters the model's view.
    assert_eq!(*requests.lock(), [("gpt-5.6-luna".into(), 1), ("gpt-5.4-mini".into(), 1)]);
    assert_eq!(agent.state.error_message, None);
    // The conversation keeps the refusal, the answer and the note, in order.
    let [Message::User(_), Message::Assistant(refused), Message::Assistant(answer), Message::User(note)] = &agent.state.messages[..] else {
        panic!("{:?}", agent.state.messages)
    };
    let refusal = factory_agent::refusal::of(refused).expect("the refusal is marked");
    assert_eq!((refusal.category.as_str(), refused.stop_reason), ("cyber", StopReason::Error));
    assert_eq!((answer.model.as_str(), answer.stop_reason), ("gpt-5.4-mini", StopReason::Stop));
    let event = note.event.as_ref().expect("the note is an event");
    assert_eq!((event.source, event.from.as_str(), event.transient), (EventSource::Host, "Claktory", true));
    let UserContent::Text(text) = &note.content else { panic!("{:?}", note.content) };
    assert!(text.contains("GPT-5.6 Luna") && text.contains("cyber") && text.contains("GPT-5.4 mini"), "{text}");
    // No Retrying events: a refusal is not a transient failure.
    assert!(!event_names(&events.lock()).iter().any(|name| name == "retrying"));
}

#[tokio::test(start_paused = true)]
async fn a_refusal_by_the_fallback_too_stands_and_leaves_no_note() {
    let (stream, requests) = scripted(&["gpt-5.6-luna", "gpt-5.4-mini"], 0);
    let mut agent = agent(stream, Some(fallback()));
    agent.prompt("hello").await.unwrap();
    assert_eq!(models(&requests), ["gpt-5.6-luna", "gpt-5.4-mini"]);
    assert_eq!(agent.state.error_message.as_deref(), Some("I can't help with this."));
    // Both refusals kept; no note about an answer that never came.
    let [Message::User(_), Message::Assistant(first), Message::Assistant(second)] = &agent.state.messages[..] else {
        panic!("{:?}", agent.state.messages)
    };
    assert!(factory_agent::refusal::of(first).is_some() && factory_agent::refusal::of(second).is_some());
}

#[tokio::test(start_paused = true)]
async fn refused_turns_are_retried_at_most_twice_a_run_and_the_turn_after_runs_on_the_agents_model() {
    // The agent's model refuses every step; the fallback answers with tool
    // calls, so the run keeps going until the retries run out.
    let (stream, requests) = scripted(&["gpt-5.6-luna"], 2);
    let mut agent = agent(stream, Some(fallback()));
    agent.prompt("hello").await.unwrap();
    // Two refused turns retried; the third refusal stands and ends the run.
    assert_eq!(models(&requests), ["gpt-5.6-luna", "gpt-5.4-mini", "gpt-5.6-luna", "gpt-5.4-mini", "gpt-5.6-luna"]);
    assert_eq!(agent.state.error_message.as_deref(), Some("I can't help with this."));
}

#[tokio::test(start_paused = true)]
async fn without_a_fallback_or_with_the_agents_own_model_as_one_the_refusal_ends_the_run() {
    for fallback_model in [None, Some(luna())] {
        let (stream, requests) = scripted(&["gpt-5.6-luna"], 0);
        let mut agent = agent(stream, fallback_model);
        agent.prompt("hello").await.unwrap();
        assert_eq!(models(&requests), ["gpt-5.6-luna"]);
        assert_eq!(agent.state.error_message.as_deref(), Some("I can't help with this."));
    }
}

#[tokio::test(start_paused = true)]
async fn a_fallback_attempt_without_capacity_is_never_waited_out() {
    // The agent's model refuses; the fallback has no account with capacity.
    let requests = Arc::new(Mutex::new(Vec::new()));
    let seen = requests.clone();
    let stream: StreamFn = Arc::new(move |model, context, _options| {
        seen.lock().push((model.id.clone(), context.messages.len()));
        let (sink, stream) = event_stream();
        if model.id == "gpt-5.6-luna" {
            sink.push(StreamEvent::Error { reason: StopReason::Error, error: refuse(model, "") });
        } else {
            let mut message = reply(model, Vec::new());
            message.stop_reason = StopReason::Error;
            message.error_message = Some("Your account reached its usage limit.".into());
            factory_agent::accounts::mark_none_left(&mut message, Some(1_900_000_000_000));
            sink.push(StreamEvent::Error { reason: StopReason::Error, error: message });
        }
        Ok(stream)
    });
    let mut agent = agent(stream, Some(fallback()));
    let waited = Arc::new(Mutex::new(0usize));
    let count = waited.clone();
    agent.wait_out_with(Arc::new(move |_, _| {
        *count.lock() += 1;
        Box::pin(async { true })
    }));
    let events = collect(&mut agent);
    agent.prompt("hello").await.unwrap();
    // One try each; the fallback's missing capacity ends the run instead
    // of waiting for a reset that may be days away.
    assert_eq!(models(&requests), ["gpt-5.6-luna", "gpt-5.4-mini"]);
    assert_eq!(*waited.lock(), 0);
    assert!(!event_names(&events.lock()).iter().any(|name| name == "waiting"));
    assert_eq!(agent.state.error_message.as_deref(), Some("Your account reached its usage limit."));
}
