use crate::support::fixtures::{context, event_json, message_json, model, openai_responses, sse_text};
use factory_agent::providers::openai_responses::{
    Parser, convert_messages, convert_tools, make_strict_json_schema, short_hash, supports_strict_mode,
};
use factory_agent::sse::SseDecoder;
use factory_agent::stream::event_stream;
use factory_agent::types::{AssistantMessage, StopReason, Usage, same_json};
use serde_json::{Value, json};

#[test]
fn short_hash_is_pi_s_hash() {
    assert_eq!(format!("fc_{}", short_hash("fc_9")), "fc_1tzgwmiy87leg");
    assert_eq!(short_hash(""), short_hash(""));
    assert_ne!(short_hash("a"), short_hash("b"));
}

#[test]
fn message_and_tool_conversion_match_every_recorded_request() {
    for case in openai_responses().iter().filter(|case| !case["request"].is_null()) {
        let name = case["name"].as_str().expect("name");
        let model = model(case);
        let context = context(case);
        assert_eq!(Value::Array(convert_messages(&model, &context).expect("input")), case["request"]["body"]["input"], "{name}");
        let tools = convert_tools(&context.tools, supports_strict_mode(&model)).expect("tools");
        match case["request"]["body"].get("tools") {
            Some(expected) => assert_eq!(Value::Array(tools), *expected, "{name}"),
            None => assert!(tools.is_empty(), "{name}"),
        }
    }
}

#[test]
fn strict_schemas_refuse_what_pi_refuses() {
    assert_eq!(make_strict_json_schema(&json!(true)).unwrap_err(), "root schema must have type object");
    assert_eq!(make_strict_json_schema(&json!({"type": "string"})).unwrap_err(), "root schema must have type object");
    assert_eq!(
        make_strict_json_schema(&json!({"type": "object", "properties": {"a": {"$ref": "#/x"}}})).unwrap_err(),
        "$ref schemas are unsupported"
    );
    assert_eq!(
        make_strict_json_schema(&json!({"type": "object", "additionalProperties": true})).unwrap_err(),
        "schema-valued or true additionalProperties is unsupported"
    );
    assert_eq!(
        make_strict_json_schema(&json!({"type": "object", "required": ["b"], "properties": {"a": {"type": "string"}}})).unwrap_err(),
        "required contains an unknown property"
    );
    assert_eq!(
        make_strict_json_schema(
            &json!({"type": "object", "properties": {"a": {"type": "string"}, "b": {"type": ["string", "null"]}}, "required": ["a"]})
        )
        .unwrap(),
        json!({"type": "object", "properties": {"a": {"type": "string"}, "b": {"type": ["string", "null"]}}, "required": ["a", "b"], "additionalProperties": false})
    );
}

fn parse(case: &Value) -> (Vec<Value>, AssistantMessage, Result<(), String>) {
    let model = model(case);
    let (sink, mut stream) = event_stream();
    let mut parser = Parser::new(AssistantMessage {
        content: Vec::new(),
        api: model.api.clone(),
        provider: model.provider.clone(),
        model: model.id.clone(),
        usage: Usage::empty(),
        stop_reason: StopReason::Pending,
        error_message: None,
        timestamp: 0,
        extra: Default::default(),
    });
    let mut decoder = SseDecoder::new();
    let mut messages = decoder.push(sse_text(case).as_bytes());
    messages.extend(decoder.flush());
    let mut outcome = Ok(());
    for message in messages {
        if message.data.starts_with("[DONE]") {
            break;
        }
        let event: Value = serde_json::from_str(&message.data).expect("json");
        if let Some(error) = event.get("error").filter(|error| !error.is_null()) {
            outcome = Err(error["message"].as_str().expect("message").to_owned());
            break;
        }
        outcome = parser.handle(&event, &sink, &model);
        if outcome.is_err() {
            break;
        }
    }
    let outcome = outcome.and_then(|()| parser.finish());
    drop(sink);
    let mut events = Vec::new();
    while let Some(event) = stream.try_next() {
        events.push(event_json(&event));
    }
    (events, parser.output, outcome)
}

#[test]
fn the_parser_reproduces_every_recorded_stream() {
    for case in openai_responses().iter().filter(|case| case["http"]["status"] == 200 && !case["request"].is_null()) {
        let name = case["name"].as_str().expect("name");
        let (events, output, outcome) = parse(case);
        let recorded: Vec<Value> = case["events"].as_array().expect("events").iter().skip(1).cloned().collect();
        assert_eq!(events, recorded, "{name}");
        match case["message"]["errorMessage"].as_str() {
            Some(error) if case["message"]["stopReason"] == "error" && output.stop_reason != StopReason::Error => {
                assert_eq!(outcome, Err(error.to_owned()), "{name}");
            }
            _ => {
                assert_eq!(outcome, Ok(()), "{name}");
                let mut expected = case["message"].clone();
                let mut actual = message_json(&output);
                if output.stop_reason == StopReason::Error {
                    expected.as_object_mut().expect("object").remove("errorMessage");
                    actual.as_object_mut().expect("object").remove("errorMessage");
                }
                assert!(same_json(&actual, &expected), "{name}: {actual} != {expected}");
            }
        }
    }
}

/// Pi's `msgIndex` advances only for messages that produced items.
#[test]
fn fallback_message_ids_skip_messages_without_items() {
    use factory_agent::stream::LlmContext;
    use factory_agent::types::{AssistantMessage, AssistantPart, Message, StopReason, Usage, UserContent, UserMessage};
    let model = model(&openai_responses()[0]);
    let foreign = |content: Vec<AssistantPart>| {
        Message::Assistant(AssistantMessage {
            content,
            api: "openai-codex-responses".into(),
            provider: "openai-codex".into(),
            model: "gpt-5.6-luna".into(),
            usage: Usage::empty(),
            stop_reason: StopReason::Stop,
            error_message: None,
            timestamp: 2,
            extra: Default::default(),
        })
    };
    let text = |text: &str| AssistantPart::Text { text: text.into(), text_signature: None };
    let messages = vec![
        Message::User(UserMessage::text("Fix", 1)),
        foreign(Vec::new()),
        Message::User(UserMessage::text("More", 3)),
        foreign(vec![text("Done")]),
    ];
    let context = LlmContext { system_prompt: None, messages, tools: Vec::new() };
    let items = convert_messages(&model, &context).expect("items");
    assert_eq!(items[2]["id"], "msg_pi_2", "{items:?}");
    let messages = vec![
        Message::User(UserMessage::text("Fix", 1)),
        Message::User(UserMessage {
            content: UserContent::Parts(Vec::new()),
            timestamp: 2,
            event: None,
            sent_from: None,
            client_id: String::new(),
        }),
        foreign(vec![text("Done"), text("Again")]),
    ];
    let context = LlmContext { system_prompt: None, messages, tools: Vec::new() };
    let items = convert_messages(&model, &context).expect("items");
    assert_eq!((items[1]["id"].as_str(), items[2]["id"].as_str()), (Some("msg_pi_1"), Some("msg_pi_1_1")), "{items:?}");
}
