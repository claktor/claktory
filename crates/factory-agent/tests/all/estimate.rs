use factory_agent::estimate::{
    Estimate, estimate_context, estimate_context_tokens, estimate_message_tokens, estimate_messages, estimate_text_tokens,
};
use factory_agent::types::{AssistantMessage, AssistantPart, Message, StopReason, ToolResultMessage, ToolResultPart, Usage, UserMessage};
use serde_json::json;

fn assistant(parts: Vec<AssistantPart>, total_tokens: u64, stop_reason: StopReason, timestamp: u64) -> Message {
    let usage = Usage { total_tokens, ..Usage::empty() };
    Message::Assistant(AssistantMessage {
        content: parts,
        api: "a".into(),
        provider: "p".into(),
        model: "m".into(),
        usage,
        stop_reason,
        error_message: None,
        timestamp,
        extra: Default::default(),
    })
}

fn tool_result(text: &str, timestamp: u64) -> Message {
    Message::ToolResult(ToolResultMessage {
        tool_call_id: "c".into(),
        tool_name: "bash".into(),
        content: vec![ToolResultPart::Text { text: text.into() }],
        details: json!({}),
        usage: None,
        is_error: false,
        timestamp,
        started: None,
    })
}

#[test]
fn text_tokens_count_utf16_units_in_fours() {
    assert_eq!(estimate_text_tokens(""), 0);
    assert_eq!(estimate_text_tokens("abcd"), 1);
    assert_eq!(estimate_text_tokens("abcde"), 2);
    assert_eq!(estimate_text_tokens("😀😀😀😀"), 2);
    let call =
        AssistantPart::ToolCall { id: "1".into(), name: "bash".into(), arguments: json!({"command": "ls"}), thought_signature: None };
    assert_eq!(estimate_message_tokens(&assistant(vec![call], 0, StopReason::ToolUse, 1)), (4 + 16_u64).div_ceil(4));
    assert_eq!(estimate_message_tokens(&Message::User(UserMessage::text("12345", 1))), 2);
}

#[test]
fn messages_after_the_last_usable_usage_are_estimated_on_top_of_it() {
    let messages = vec![
        Message::User(UserMessage::text("x".repeat(400), 1)),
        assistant(vec![AssistantPart::Text { text: "y".repeat(40), text_signature: None }], 1000, StopReason::Stop, 2),
        tool_result(&"z".repeat(80), 3),
        assistant(vec![AssistantPart::Text { text: "aborted".into(), text_signature: None }], 2000, StopReason::Aborted, 4),
    ];
    assert_eq!(estimate_messages(&messages), Estimate { tokens: 1022, usage_tokens: 1000, trailing_tokens: 22, last_usage_index: Some(1) });
    assert_eq!(estimate_context(Some("prompt"), &messages, Some(&json!([{"name": "bash"}]))).tokens, 1022);
    let fresh = vec![Message::User(UserMessage::text("x".repeat(400), 1))];
    assert_eq!(estimate_messages(&fresh), Estimate { tokens: 100, usage_tokens: 0, trailing_tokens: 100, last_usage_index: None });
    let with_prefix = estimate_context(Some("12345678"), &fresh, Some(&json!([{"name": "bash"}])));
    assert_eq!(with_prefix.tokens, 100 + 2 + estimate_text_tokens(r#"[{"name":"bash"}]"#));
    assert_eq!(estimate_context(Some(""), &fresh, Some(&json!([]))).tokens, 100);
    let stale = vec![
        assistant(vec![], 500, StopReason::Stop, 1),
        Message::User(UserMessage::text("later prefix", 5)),
        assistant(vec![], 700, StopReason::Stop, 3),
    ];
    assert_eq!(estimate_messages(&stale).last_usage_index, Some(0));
}

#[test]
fn the_headless_estimate_takes_the_last_usable_usage_regardless_of_timestamps() {
    let stale = vec![
        assistant(vec![], 500, StopReason::Stop, 1),
        Message::User(UserMessage::text("later prefix", 5)),
        assistant(vec![], 700, StopReason::Stop, 3),
        tool_result("zzzz", 6),
    ];
    assert_eq!(estimate_context_tokens(&stale), Estimate { tokens: 701, usage_tokens: 700, trailing_tokens: 1, last_usage_index: Some(2) });
    assert_eq!(estimate_messages(&stale).last_usage_index, Some(0));
    let unusable = vec![assistant(vec![AssistantPart::Text { text: "abcd".into(), text_signature: None }], 900, StopReason::Aborted, 1)];
    assert_eq!(estimate_context_tokens(&unusable), Estimate { tokens: 1, usage_tokens: 0, trailing_tokens: 1, last_usage_index: None });
}
