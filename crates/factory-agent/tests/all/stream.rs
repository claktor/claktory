use factory_agent::errors::Error;
use factory_agent::stream::{StreamOptions, ThinkingLevel, event_stream};
use factory_agent::types::{AssistantMessage, AssistantPart, StopReason, StreamEvent, Usage};
use serde_json::json;

fn message(text: &str) -> AssistantMessage {
    AssistantMessage {
        content: vec![AssistantPart::Text { text: text.into(), text_signature: None }],
        api: "a".into(),
        provider: "p".into(),
        model: "m".into(),
        usage: Usage::empty(),
        stop_reason: StopReason::Stop,
        error_message: None,
        timestamp: 1,
        extra: Default::default(),
    }
}

#[tokio::test]
async fn a_stream_yields_its_events_and_the_terminal_message() {
    let (sink, mut stream) = event_stream();
    sink.push(StreamEvent::Start { partial: message("") });
    sink.push(StreamEvent::TextDelta { content_index: 0, delta: "hi".into(), partial: message("hi") });
    sink.push(StreamEvent::Done { reason: StopReason::Stop, message: message("hi") });
    drop(sink);
    let first = stream.next().await.unwrap();
    assert!(matches!(first, StreamEvent::Start { .. }));
    assert_eq!(first.message().content.len(), 1);
    assert!(!first.is_terminal());
    assert_eq!(stream.result().await.map(|m| m.content), Some(message("hi").content));
    let (sink, stream) = event_stream();
    sink.push(StreamEvent::TextDelta { content_index: 0, delta: "x".into(), partial: message("x") });
    drop(sink);
    assert_eq!(stream.result().await, None);
    let (sink, stream) = event_stream();
    let mut failed = message("");
    failed.stop_reason = StopReason::Error;
    failed.error_message = Some("boom".into());
    let event = StreamEvent::Error { reason: StopReason::Error, error: failed.clone() };
    assert!(event.is_terminal());
    sink.push(event);
    assert_eq!(stream.result().await, Some(failed));
}

#[test]
fn thinking_levels_parse_and_serialize_lowercase() {
    assert_eq!(ThinkingLevel::parse("xhigh"), Some(ThinkingLevel::Xhigh));
    assert_eq!(ThinkingLevel::parse("none"), None);
    assert_eq!(serde_json::to_value(ThinkingLevel::Medium).unwrap(), json!("medium"));
    assert_eq!(serde_json::from_value::<ThinkingLevel>(json!("off")).unwrap(), ThinkingLevel::Off);
    assert_eq!(
        ThinkingLevel::ALL.iter().map(|level| level.as_str()).collect::<Vec<_>>(),
        ["off", "minimal", "low", "medium", "high", "xhigh", "max"]
    );
    let options = StreamOptions::default();
    assert!(!options.cancel.is_cancelled());
    assert_eq!((options.session_id, options.reasoning, options.timeout_ms, options.max_retries), (None, None, None, None));
    assert_eq!(Error::Other("plain".into()).to_string(), "plain");
    assert_eq!(Error::Other("plain".into()).failure(), None);
}
