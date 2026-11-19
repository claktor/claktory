#![allow(dead_code)]

use std::sync::Arc;
use std::time::Duration;

use factory_agent::agent::Agent;
use factory_agent::stream::{StreamFn, event_stream};
use factory_agent::types::{
    AgentEvent, AssistantMessage, AssistantPart, Cost, CostTier, Message, Model, ModelCost, StopReason, StreamEvent, Usage, UserContent,
    UserPart,
};
use factory_agent::usage::calculate_cost;
use parking_lot::Mutex;
use serde_json::json;

pub fn luna() -> Model {
    Model {
        id: "gpt-5.6-luna".into(),
        name: "GPT-5.6 Luna".into(),
        api: "openai-codex-responses".into(),
        provider: "openai-codex".into(),
        base_url: "https://chatgpt.com/backend-api".into(),
        reasoning: true,
        input: vec!["text".into(), "image".into()],
        cost: ModelCost {
            input: 0.2,
            output: 1.2,
            cache_read: 0.02,
            cache_write: 0.25,
            tiers: Some(vec![CostTier {
                input_tokens_above: 272_000,
                input: 0.4,
                output: 1.8,
                cache_read: 0.04,
                cache_write: 0.5,
                extra: Default::default(),
            }]),
            extra: Default::default(),
        },
        context_window: 272_000,
        max_tokens: 128_000,
        thinking_level_map: None,
        compat: None,
        headers: None,
        extra: Default::default(),
    }
}

pub fn reply(model: &Model, content: Vec<AssistantPart>) -> AssistantMessage {
    let tool = content.iter().any(|part| matches!(part, AssistantPart::ToolCall { .. }));
    let mut usage = Usage {
        input: 100,
        output: 20,
        cache_read: 10,
        cache_write: 2,
        cache_write_1h: None,
        reasoning: Some(5),
        total_tokens: 132,
        cost: Cost::ZERO,
    };
    calculate_cost(&model.cost, &mut usage);
    AssistantMessage {
        content,
        api: model.api.clone(),
        provider: model.provider.clone(),
        model: model.id.clone(),
        usage,
        stop_reason: if tool { StopReason::ToolUse } else { StopReason::Stop },
        error_message: None,
        timestamp: factory_agent::clock::now_ms(),
        extra: Default::default(),
    }
}

fn user_text(message: &Message) -> String {
    match message {
        Message::User(user) => match &user.content {
            UserContent::Text(text) => text.clone(),
            UserContent::Parts(parts) => serde_json::to_string(parts).expect("json"),
        },
        _ => String::new(),
    }
}

fn user_texts_contain(message: &Message, needle: &str) -> bool {
    match message {
        Message::User(user) => match &user.content {
            UserContent::Text(text) => text.contains(needle),
            UserContent::Parts(parts) => parts.iter().any(|part| matches!(part, UserPart::Text { text } if text.contains(needle))),
        },
        _ => false,
    }
}

/// Test-only provider: exercises the real loop and tools without model inference.
pub fn scripted_model() -> StreamFn {
    Arc::new(|model, context, options| {
        let (sink, stream) = event_stream();
        let last = context.messages.iter().rev().find(|message| !matches!(message, Message::System(_)));
        let prompt = context.messages.iter().rfind(|message| matches!(message, Message::User(_)));
        let text = prompt.map(user_text).unwrap_or_default();
        let prompts = context.messages.iter().filter(|message| matches!(message, Message::User(_))).count();
        let mut content = vec![AssistantPart::Text { text: format!("Done. Prior user prompts: {prompts}"), text_signature: None }];
        if matches!(last, Some(Message::User(_))) {
            let (name, arguments) = if text.contains("# Checkpoint 1") {
                ("write_file", json!({"path": "main.py", "content": "print(\"one\")\n"}))
            } else if text.contains("# Checkpoint 2") {
                (
                    "edit_file",
                    json!({"path": "main.py", "edits": [{"oldText": "print(\"one\")", "newText": "import sys\nprint(\"two\" if sys.argv[1:] == [\"two\"] else \"one\")"}]}),
                )
            } else if text.contains("PAUSE") {
                ("bash", json!({"command": "printf 'tool started\\n'; sleep 2"}))
            } else if text.contains("SLOW") {
                ("bash", json!({"command": "printf 'tool started\\n'; sleep 60"}))
            } else {
                ("bash", json!({"command": "printf '# reviewed\\n' >> main.py"}))
            };
            content = vec![AssistantPart::ToolCall {
                id: format!("test-{}", context.messages.len()),
                name: name.into(),
                arguments,
                thought_signature: None,
            }];
        }
        let slow_stream =
            prompt.is_some_and(|message| user_texts_contain(message, "SLOW_STREAM")) && matches!(last, Some(Message::ToolResult(_)));
        let message = reply(model, content);
        tokio::spawn(async move {
            sink.push(StreamEvent::Start { partial: message.clone() });
            sink.push(StreamEvent::TextDelta { content_index: 0, delta: "test delta".into(), partial: message.clone() });
            if slow_stream {
                tokio::select! {
                    () = tokio::time::sleep(Duration::from_secs(60)) => {}
                    () = options.cancel.cancelled() => {}
                }
            }
            if options.cancel.is_cancelled() {
                let mut aborted = message;
                aborted.stop_reason = StopReason::Aborted;
                sink.push(StreamEvent::Error { reason: StopReason::Aborted, error: aborted });
            } else {
                sink.push(StreamEvent::Done { reason: message.stop_reason, message });
            }
        });
        Ok(stream)
    })
}

pub fn collect(agent: &mut Agent) -> Arc<Mutex<Vec<AgentEvent>>> {
    let events = Arc::new(Mutex::new(Vec::new()));
    let sink = events.clone();
    agent.subscribe(Arc::new(move |event| {
        let sink = sink.clone();
        Box::pin(async move {
            sink.lock().push(event);
            Ok(())
        })
    }));
    events
}

pub fn event_names(events: &[AgentEvent]) -> Vec<String> {
    events.iter().map(|event| serde_json::to_value(event).expect("event")["type"].as_str().expect("type").to_owned()).collect()
}

pub fn deepseek() -> Model {
    Model {
        id: "deepseek/deepseek-v4-flash-0731".into(),
        name: "DeepSeek: DeepSeek V4 Flash 0731".into(),
        api: "openai-completions".into(),
        provider: "openrouter".into(),
        base_url: "https://openrouter.ai/api/v1".into(),
        reasoning: true,
        input: vec!["text".into()],
        cost: ModelCost { input: 0.03, output: 0.13, cache_read: 0.01, cache_write: 0.0, tiers: None, extra: Default::default() },
        context_window: 128_000,
        max_tokens: 16_000,
        thinking_level_map: None,
        compat: Some(json!({"thinkingFormat": "openrouter"}).as_object().expect("object").clone()),
        headers: None,
        extra: Default::default(),
    }
}

pub mod fixtures {
    use factory_agent::providers::catalog;
    use factory_agent::stream::LlmContext;
    use factory_agent::tools::coding_tools;
    use factory_agent::types::{AssistantMessage, Message, Model, StreamEvent};
    use serde_json::Value;

    pub fn openai_responses() -> Vec<Value> {
        serde_json::from_str(include_str!("../fixtures/openai_responses.json")).expect("fixture")
    }

    pub fn model(case: &Value) -> Model {
        catalog::models("openai").into_iter().find(|model| model.id == case["model"]).expect("catalog model")
    }

    pub fn context(case: &Value) -> LlmContext {
        let names: Vec<&str> = case["context"]["tools"].as_array().expect("tools").iter().filter_map(Value::as_str).collect();
        let tools = coding_tools(std::path::Path::new("/w"), &[], None)
            .into_iter()
            .map(|tool| tool.spec)
            .filter(|spec| names.contains(&spec.name.as_ref()))
            .collect();
        LlmContext {
            system_prompt: case["context"]["systemPrompt"].as_str().map(str::to_owned),
            messages: serde_json::from_value::<Vec<Message>>(case["context"]["messages"].clone()).expect("messages"),
            tools,
        }
    }

    /// The fixture records events without `partial` and messages without `timestamp`.
    pub fn event_json(event: &StreamEvent) -> Value {
        let mut value = serde_json::to_value(event).expect("event");
        value.as_object_mut().expect("object").remove("partial");
        value
    }

    pub fn message_json(message: &AssistantMessage) -> Value {
        let mut value = serde_json::to_value(Message::Assistant(message.clone())).expect("message");
        value.as_object_mut().expect("object").remove("timestamp");
        value
    }

    pub fn sse_text(case: &Value) -> String {
        case["http"]["chunks"].as_array().expect("chunks").iter().filter_map(Value::as_str).collect()
    }

    use factory_agent::stream::{StreamFn, StreamOptions, ThinkingLevel};
    use factory_agent::types::same_json;

    /// The catalog model a fixture case names, with the routing it recorded.
    pub fn fixture_model(provider: &str, case: &Value) -> Model {
        let mut model = catalog::models(provider).into_iter().find(|model| model.id == case["model"]).expect("catalog model");
        if let Some(routing) = case.get("routing").filter(|routing| !routing.is_null()) {
            model.compat.get_or_insert_with(Default::default).insert("openRouterRouting".into(), routing.clone());
        }
        model
    }

    /// The agent passes no reasoning level when thinking is off.
    pub fn fixture_options(case: &Value) -> StreamOptions {
        StreamOptions {
            api_key: case["apiKey"].as_str().map(str::to_owned),
            session_id: Some("session-abc".into()),
            reasoning: case["reasoning"].as_str().and_then(ThinkingLevel::parse).filter(|level| *level != ThinkingLevel::Off),
            ..StreamOptions::default()
        }
    }

    /// Replays one recorded exchange through `stream` against a local server and compares the request, events and message.
    pub async fn replay(case: &Value, base_path: &str, headers: &[&str], stream: StreamFn, provider: &str) {
        let name = case["name"].as_str().expect("name");
        let status = case["http"]["status"].as_u64().expect("status") as u16;
        let content_type = if status == 200 {
            "text/event-stream"
        } else if case["http"]["json"] == true {
            "application/json"
        } else {
            "text/plain"
        };
        let chunks = case["http"]["chunks"].as_array().expect("chunks").iter().filter_map(Value::as_str).map(str::to_owned).collect();
        let server =
            super::server::serve(vec![super::server::Served { status, content_type, chunks, hold: false, hold_before_headers: false }])
                .await;
        let mut model = fixture_model(provider, case);
        model.base_url = format!("{}{base_path}", server.base_url);
        let mut events = match stream(&model, context(case), fixture_options(case)) {
            Ok(events) => events,
            Err(error) => {
                assert_eq!(case["message"]["thrown"], error.to_string(), "{name}");
                return;
            }
        };
        let mut seen = Vec::new();
        while let Some(event) = events.next().await {
            seen.push(event);
        }
        let (last, body) = seen.split_last().expect("terminal event");
        assert_eq!(Value::Array(body.iter().map(event_json).collect()), case["events"], "{name}");
        let message = match last {
            StreamEvent::Done { message, .. } | StreamEvent::Error { error: message, .. } => message,
            other => panic!("{name}: {other:?}"),
        };
        let actual = message_json(message);
        assert!(same_json(&actual, &case["message"]), "{name}: {actual} != {}", case["message"]);
        if case["request"].is_null() {
            assert!(server.requests.lock().is_empty(), "{name}");
            return;
        }
        let request = server.requests.lock().first().cloned().expect("request");
        assert_eq!(
            (request.method.as_str(), request.path.clone()),
            ("POST", url_path(case["request"]["url"].as_str().expect("url"))),
            "{name}"
        );
        assert!(same_json(&request.body, &case["request"]["body"]), "{name}: {} != {}", request.body, case["request"]["body"]);
        let header = |name: &str| request.headers.iter().find(|(key, _)| key == name).map(|(_, value)| value.clone());
        for wanted in headers {
            let recorded = case["request"]["headers"][*wanted].as_str();
            let expected = match recorded {
                Some(agent) if *wanted == "user-agent" && agent.starts_with("pi (") => {
                    Some(factory_agent::providers::user_agent().to_owned())
                }
                other => other.map(str::to_owned),
            };
            assert_eq!(header(wanted), expected, "{name}: header {wanted}");
        }
    }

    fn url_path(url: &str) -> String {
        let after_scheme = url.split_once("://").map_or(url, |(_, rest)| rest);
        after_scheme.find('/').map_or_else(|| "/".to_owned(), |slash| after_scheme[slash..].to_owned())
    }
}

pub mod auth;
pub mod codex_server;
pub mod server;
