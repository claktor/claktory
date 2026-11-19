use factory_agent::types::{
    AgentEvent, AssistantMessage, AssistantPart, Cost, Message, StopReason, StreamEvent, ToolResult, ToolResultPart, Usage, UserMessage,
    same_json,
};
use serde_json::{Value, json};

const ASSISTANT: &str = r#"{"role":"assistant","content":[{"type":"thinking","thinking":"Let me create the hello.py file and run it.","thinkingSignature":"[{\"type\":\"reasoning.text\",\"text\":\"Let\",\"format\":\"unknown\",\"index\":0}]"},{"type":"toolCall","id":"chatcmpl-tool-aa75b85fb586ae21","name":"write_file","arguments":{"path":"hello.py","content":"print('hello ladder')\n"}}],"api":"openai-completions","provider":"openrouter","model":"deepseek/deepseek-v4-flash-0731","usage":{"input":0,"output":28,"cacheRead":1208,"cacheWrite":0,"reasoning":0,"totalTokens":1236,"cost":{"input":0,"output":3.64e-06,"cacheRead":1.208e-05,"cacheWrite":0,"total":1.572e-05}},"stopReason":"toolUse","timestamp":1789661424790,"responseId":"gen-1789661424-O3e3keC3jhRuipGF5iXE","rawStopReason":"tool_calls"}"#;

const TOOL_RESULT: &str = r#"{"role":"toolResult","toolCallId":"chatcmpl-tool-aa75b85fb586ae21","toolName":"write_file","content":[{"type":"text","text":"Created hello.py (22 characters)."}],"details":{"path":"hello.py"},"isError":false,"timestamp":1789661418654}"#;

const USER: &str = r#"{"role":"user","content":[{"type":"text","text":"Create hello.py\n"}],"timestamp":1789661414274}"#;

fn round_trip(line: &str) -> Message {
    let original: Value = serde_json::from_str(line).expect("json");
    let message: Message = serde_json::from_str(line).expect("json");
    let again = serde_json::to_value(&message).expect("json");
    assert!(same_json(&original, &again), "{original}\n{again}");
    message
}

#[test]
fn journal_messages_round_trip_including_keys_this_crate_does_not_model() {
    let Message::Assistant(assistant) = round_trip(ASSISTANT) else { panic!("role") };
    assert_eq!(assistant.stop_reason, StopReason::ToolUse);
    assert_eq!(assistant.usage.cache_read, 1208);
    assert_eq!(assistant.usage.reasoning, Some(0));
    assert_eq!(assistant.extra.get("responseId"), Some(&json!("gen-1789661424-O3e3keC3jhRuipGF5iXE")));
    assert!(matches!(&assistant.content[1], AssistantPart::ToolCall { name, .. } if name == "write_file"));
    let Message::ToolResult(result) = round_trip(TOOL_RESULT) else { panic!("role") };
    assert!(!result.is_error);
    assert_eq!(result.details, json!({"path": "hello.py"}));
    let user = round_trip(USER);
    assert_eq!(user.timestamp(), 1789661414274);
    let keys: Vec<String> = serde_json::to_value(&user).unwrap().as_object().unwrap().keys().cloned().collect();
    assert_eq!(keys, ["role", "content", "timestamp"]);
}

#[test]
fn optional_fields_are_omitted_and_events_carry_the_journal_names() {
    let message = AssistantMessage {
        content: vec![AssistantPart::Text { text: "hi".into(), text_signature: None }],
        api: "openai-codex-responses".into(),
        provider: "openai-codex".into(),
        model: "gpt-5.6-luna".into(),
        usage: Usage::empty(),
        stop_reason: StopReason::Stop,
        error_message: None,
        timestamp: 1,
        extra: Default::default(),
    };
    let value = serde_json::to_value(&message).unwrap();
    assert_eq!(value["content"], json!([{"type": "text", "text": "hi"}]));
    assert!(value.get("errorMessage").is_none());
    assert_eq!(
        value["usage"],
        json!({"input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0, "totalTokens": 0, "cost": {"input": 0.0, "output": 0.0, "cacheRead": 0.0, "cacheWrite": 0.0, "total": 0.0}})
    );
    let event = StreamEvent::TextDelta { content_index: 0, delta: "test delta".into(), partial: message.clone() };
    let value = serde_json::to_value(&event).unwrap();
    assert_eq!(value["type"], "text_delta");
    assert_eq!(value["contentIndex"], 0);
    assert_eq!(value["partial"]["model"], "gpt-5.6-luna");
    let end = AgentEvent::ToolExecutionEnd {
        tool_call_id: "call".into(),
        tool_name: "bash".into(),
        result: ToolResult::error("Tool bash not found"),
        is_error: true,
    };
    assert_eq!(
        serde_json::to_value(&end).unwrap(),
        json!({"type": "tool_execution_end", "toolCallId": "call", "toolName": "bash", "result": {"content": [{"type": "text", "text": "Tool bash not found"}], "details": {}}, "isError": true})
    );
    assert_eq!(serde_json::to_value(AgentEvent::AgentStart).unwrap(), json!({"type": "agent_start"}));
    let user = Message::User(UserMessage::text("hello", 5));
    assert_eq!(
        serde_json::to_value(&user).unwrap(),
        json!({"role": "user", "content": [{"type": "text", "text": "hello"}], "timestamp": 5})
    );
    assert_eq!(ToolResult::text("ok", json!({"path": "a"})).content, vec![ToolResultPart::Text { text: "ok".into() }]);
    assert_eq!(Cost::ZERO.total, 0.0);
}

#[test]
fn same_json_compares_numbers_by_value() {
    assert!(same_json(&json!({"a": [1, {"b": 0}]}), &json!({"a": [1.0, {"b": 0.0}]})));
    assert!(!same_json(&json!({"a": 1}), &json!({"a": 1, "b": 2})));
    assert!(!same_json(&json!([1, 2]), &json!([2, 1])));
    assert!(!same_json(&json!("1"), &json!(1)));
}

#[test]
fn an_event_reaches_the_model_tagged_with_where_it_is_from_and_reloads_as_an_event() {
    use factory_agent::types::{Event, EventSource};
    let event =
        Event { source: EventSource::Agent, from: "Builder".into(), id: "builder".into(), summary: String::new(), transient: false };
    let message = UserMessage::event("The tests pass.", event.clone(), 7);
    let seen = message.for_model();
    assert_eq!(seen.event, None);
    assert_eq!(
        serde_json::to_value(&seen.content).unwrap(),
        json!([
            {"type": "text", "text": "<event source=\"agent\" from=\"Builder\" id=\"builder\">\n"},
            {"type": "text", "text": "The tests pass."},
            {"type": "text", "text": "\n</event>"},
        ])
    );
    let saved = serde_json::to_string(&Message::User(message.clone())).unwrap();
    assert_eq!(serde_json::from_str::<Message>(&saved).unwrap(), Message::User(message));
    // Messages the person wrote are saved as before, without an event.
    assert!(!serde_json::to_string(&Message::User(UserMessage::text("hi", 1))).unwrap().contains("event"));
}
