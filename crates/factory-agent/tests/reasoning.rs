use std::sync::Arc;

use factory_agent::errors::Error;
use factory_agent::reasoning::{coalesce_history, coalesce_reasoning, coalesce_reasoning_details, drop_reasoning};
use factory_agent::stream::{LlmContext, StreamFn, StreamOptions, event_stream};
use factory_agent::types::{
    AssistantMessage, AssistantPart, Message, Model, ModelCost, StopReason, StreamEvent, ToolResultMessage, ToolResultPart, Usage,
    UserMessage,
};
use serde_json::{Value, json};

fn detail(text: &str, extra: Value) -> Value {
    let mut value = json!({"type": "reasoning.text", "text": text, "format": "unknown", "index": 0});
    for (key, item) in extra.as_object().into_iter().flatten() {
        value[key] = item.clone();
    }
    value
}

fn model() -> Model {
    Model {
        id: "m".into(),
        name: "m".into(),
        api: "openai-completions".into(),
        provider: "openrouter".into(),
        base_url: String::new(),
        reasoning: true,
        input: vec![],
        cost: ModelCost { input: 0.0, output: 0.0, cache_read: 0.0, cache_write: 0.0, tiers: None, extra: Default::default() },
        context_window: 0,
        max_tokens: 0,
        thinking_level_map: None,
        compat: None,
        headers: None,
        extra: Default::default(),
    }
}

fn reply(content: Vec<AssistantPart>) -> AssistantMessage {
    let tool = content.iter().any(|part| matches!(part, AssistantPart::ToolCall { .. }));
    AssistantMessage {
        content,
        api: "openai-completions".into(),
        provider: "openrouter".into(),
        model: "m".into(),
        usage: Usage { input: 100, ..Usage::empty() },
        stop_reason: if tool { StopReason::ToolUse } else { StopReason::Stop },
        error_message: None,
        timestamp: 1,
        extra: Default::default(),
    }
}

#[test]
fn streamed_reasoning_fragments_become_one_detail() {
    let streamed = serde_json::to_string(&["Let", " me", " read", " the", " spec"].map(|text| detail(text, json!({})))).unwrap();
    assert_eq!(
        coalesce_reasoning_details(Some(&streamed)),
        Some(serde_json::to_string(&[detail("Let me read the spec", json!({}))]).unwrap())
    );
}

#[test]
fn details_that_differ_in_index_format_or_id_stay_separate() {
    let streamed = serde_json::to_string(&[
        detail("a", json!({})),
        detail("b", json!({"index": 1})),
        detail("c", json!({"format": "openai"})),
        detail("d", json!({"id": "r1"})),
    ])
    .unwrap();
    let merged: Vec<Value> = serde_json::from_str(&coalesce_reasoning_details(Some(&streamed)).unwrap()).unwrap();
    assert_eq!(merged.len(), 4);
}

#[test]
fn a_signature_is_carried_onto_the_merged_detail_and_never_erased() {
    let streamed = serde_json::to_string(&[detail("half ", json!({})), detail("done", json!({"signature": "sig"}))]).unwrap();
    let merged: Vec<Value> = serde_json::from_str(&coalesce_reasoning_details(Some(&streamed)).unwrap()).unwrap();
    assert_eq!(merged, vec![detail("half done", json!({"signature": "sig"}))]);
    let streamed =
        serde_json::to_string(&[detail("half ", json!({"signature": "sig"})), detail("done", json!({"signature": null}))]).unwrap();
    let merged: Vec<Value> = serde_json::from_str(&coalesce_reasoning_details(Some(&streamed)).unwrap()).unwrap();
    assert_eq!(merged, vec![detail("half done", json!({"signature": "sig"}))]);
}

#[test]
fn encrypted_and_summary_details_pass_through_and_other_signatures_are_left_alone() {
    let streamed = serde_json::to_string(&[
        json!({"type": "reasoning.encrypted", "data": "x"}),
        json!({"type": "reasoning.summary", "summary": "s"}),
        detail("a", json!({})),
        detail("b", json!({})),
    ])
    .unwrap();
    let merged: Vec<Value> = serde_json::from_str(&coalesce_reasoning_details(Some(&streamed)).unwrap()).unwrap();
    assert_eq!(
        merged,
        vec![
            json!({"type": "reasoning.encrypted", "data": "x"}),
            json!({"type": "reasoning.summary", "summary": "s"}),
            detail("ab", json!({}))
        ]
    );
    assert_eq!(coalesce_reasoning_details(Some("reasoning_content")), Some("reasoning_content".into()));
    assert_eq!(coalesce_reasoning_details(None), None);
    assert_eq!(coalesce_reasoning_details(Some("")), Some(String::new()));
    let single = serde_json::to_string(&[detail("only", json!({}))]).unwrap();
    assert_eq!(coalesce_reasoning_details(Some(&single)), Some(single.clone()));
}

#[tokio::test]
async fn the_completed_message_carries_coalesced_details_downstream() {
    let fragments = ["Let", " me", " look"];
    let signature = serde_json::to_string(&fragments.map(|text| detail(text, json!({})))).unwrap();
    let source: StreamFn = Arc::new(move |_model, _context, _options| {
        let (sink, stream) = event_stream();
        let message = reply(vec![
            AssistantPart::Thinking { thinking: fragments.concat(), thinking_signature: Some(signature.clone()), redacted: None },
            AssistantPart::Text { text: "done".into(), text_signature: None },
        ]);
        sink.push(StreamEvent::Start { partial: message.clone() });
        sink.push(StreamEvent::Done { reason: StopReason::Stop, message });
        Ok(stream)
    });
    let seen = coalesce_reasoning(source)(&model(), LlmContext::default(), StreamOptions::default()).unwrap().result().await.unwrap();
    let Some(AssistantPart::Thinking { thinking, thinking_signature, .. }) = seen.content.first() else { panic!("thinking") };
    assert_eq!(thinking, "Let me look");
    assert_eq!(thinking_signature.as_deref(), Some(serde_json::to_string(&[detail("Let me look", json!({}))]).unwrap().as_str()));
}

#[tokio::test]
async fn a_source_that_fails_to_start_ends_with_an_error_event() {
    let source: StreamFn = Arc::new(|_model, _context, _options| Err(Error::Other("connection reset".into())));
    let mut stream = coalesce_reasoning(source)(&model(), LlmContext::default(), StreamOptions::default()).unwrap();
    let only = stream.next().await.unwrap();
    let StreamEvent::Error { reason, error } = only else { panic!("error event") };
    assert_eq!(reason, StopReason::Error);
    assert_eq!(error.stop_reason, StopReason::Error);
    assert_eq!(error.error_message.as_deref(), Some("connection reset"));
    assert_eq!((error.api.as_str(), error.provider.as_str(), error.model.as_str()), ("openai-completions", "openrouter", "m"));
    assert!(error.content.is_empty());
    assert_eq!(stream.next().await, None);
}

fn history() -> Vec<Message> {
    vec![
        Message::User(UserMessage::text("Fix the parser", 1)),
        Message::Assistant(reply(vec![
            AssistantPart::Thinking {
                thinking: "Let me read the spec".into(),
                thinking_signature: Some(serde_json::to_string(&[detail("a", json!({}))]).expect("json")),
                redacted: None,
            },
            AssistantPart::Text { text: "Reading.".into(), text_signature: None },
            AssistantPart::ToolCall {
                id: "c1".into(),
                name: "read_file".into(),
                arguments: json!({"path": "a.ts"}),
                thought_signature: Some("encrypted".into()),
            },
        ])),
        Message::ToolResult(ToolResultMessage {
            tool_call_id: "c1".into(),
            tool_name: "read_file".into(),
            content: vec![ToolResultPart::Text { text: "1 line".into() }],
            details: Value::Null,
            usage: None,
            is_error: false,
            timestamp: 2,
            started: None,
        }),
    ]
}

#[test]
fn dropping_reasoning_keeps_text_tool_calls_and_every_other_message() {
    let messages = history();
    let stripped = drop_reasoning(&messages);
    assert_eq!(stripped.len(), 3);
    assert_eq!(stripped[0], messages[0]);
    assert_eq!(stripped[2], messages[2]);
    let Message::Assistant(assistant) = &stripped[1] else { panic!("assistant") };
    assert_eq!(
        assistant.content,
        vec![
            AssistantPart::Text { text: "Reading.".into(), text_signature: None },
            AssistantPart::ToolCall {
                id: "c1".into(),
                name: "read_file".into(),
                arguments: json!({"path": "a.ts"}),
                thought_signature: None
            }
        ]
    );
    let Message::Assistant(original) = &messages[1] else { panic!("assistant") };
    assert_eq!(assistant.usage, original.usage);
    assert_eq!(original.content.len(), 3);
}

#[test]
fn a_journal_written_before_coalescing_replays_coalesced_details() {
    let exploded = serde_json::to_string(&["Let", " me", " read", " the", " spec"].map(|text| detail(text, json!({})))).unwrap();
    let stale = Message::Assistant(reply(vec![AssistantPart::Thinking {
        thinking: "Let me read the spec".into(),
        thinking_signature: Some(exploded),
        redacted: None,
    }]));
    let replayed = coalesce_history(vec![Message::User(UserMessage::text("Earlier", 1)), stale]);
    let Message::Assistant(assistant) = &replayed[1] else { panic!("assistant") };
    let Some(AssistantPart::Thinking { thinking_signature, .. }) = assistant.content.first() else { panic!("thinking") };
    assert_eq!(thinking_signature.as_deref(), Some(serde_json::to_string(&[detail("Let me read the spec", json!({}))]).unwrap().as_str()));
}
