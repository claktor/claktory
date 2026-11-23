//! Continue on another provider (#341): agents and a chat waiting out their
//! Claude usage limit offer to continue on OpenRouter (the same model, or
//! the closest), switch at the person's tap, carry on there, and offer to
//! go back once Claude has room again. Against one mock server for both
//! providers.

use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex};

use crate::accounts::{account, registry_over, serve};
use crate::common::{Client, View, apply_request, send_message, temp_dir};
use factory_agent::clock::now_ms;
use factory_protocol::v1::{self, ErrorCode, client_message::Body as Request, server_message::Body};
use factoryd::factory::Factory;
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

/// What the mock was asked: (path, the request's model).
type Seen = Arc<Mutex<Vec<(String, String)>>>;

/// Claude (account "a") at its limit until `reset` (unix seconds, 0 for
/// never), OpenRouter answering every request.
async fn providers(reset: Arc<AtomicI64>) -> (String, Seen) {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let base_url = format!("http://{}", listener.local_addr().expect("addr"));
    let seen: Seen = Arc::new(Mutex::new(Vec::new()));
    let recorded = seen.clone();
    tokio::spawn(async move {
        loop {
            let Ok((mut socket, _)) = listener.accept().await else { return };
            let (recorded, reset) = (recorded.clone(), reset.clone());
            tokio::spawn(async move {
                let mut raw = Vec::new();
                let head_end = loop {
                    let mut buffer = [0u8; 8192];
                    let Ok(read) = socket.read(&mut buffer).await else { return };
                    if read == 0 {
                        return;
                    }
                    raw.extend_from_slice(&buffer[..read]);
                    if let Some(end) = raw.windows(4).position(|window| window == b"\r\n\r\n") {
                        break end;
                    }
                };
                let head = String::from_utf8_lossy(&raw[..head_end]).into_owned();
                let length: usize = head
                    .lines()
                    .filter_map(|line| line.split_once(':'))
                    .find(|(key, _)| key.trim().eq_ignore_ascii_case("content-length"))
                    .and_then(|(_, value)| value.trim().parse().ok())
                    .unwrap_or(0);
                while raw.len() < head_end + 4 + length {
                    let mut buffer = [0u8; 8192];
                    match socket.read(&mut buffer).await {
                        Ok(0) | Err(_) => break,
                        Ok(read) => raw.extend_from_slice(&buffer[..read]),
                    }
                }
                let path = head.split(' ').nth(1).unwrap_or_default().to_owned();
                let body: Value = serde_json::from_slice(&raw[head_end + 4..]).unwrap_or_default();
                recorded.lock().expect("lock").push((path.clone(), body["model"].as_str().unwrap_or_default().to_owned()));
                let (status, extra, content_type, body) = answer(&path, reset.load(Ordering::SeqCst));
                let mut response = format!(
                    "HTTP/1.1 {status} X\r\ncontent-type: {content_type}\r\ncontent-length: {}\r\nconnection: close\r\n",
                    body.len()
                );
                response.push_str(&extra);
                response.push_str("\r\n");
                response.push_str(&body);
                let _ = socket.write_all(response.as_bytes()).await;
                let _ = socket.shutdown().await;
            });
        }
    });
    (base_url, seen)
}

fn answer(path: &str, reset: i64) -> (u16, String, &'static str, String) {
    let limited = reset == 0 || now_ms() / 1000 < u64::try_from(reset).unwrap_or(0);
    let resets_at = if reset == 0 { now_ms() / 1000 + 3 * 3600 } else { u64::try_from(reset).unwrap_or(0) };
    let json = |body: Value| (200, String::new(), "application/json", body.to_string());
    if path.starts_with("/api/oauth/usage") {
        let at = jiff::Timestamp::from_second(i64::try_from(resets_at).expect("seconds")).expect("a time").to_string();
        return json(json!({"five_hour": {"utilization": if limited { 100.0 } else { 3.0 }, "resets_at": at}}));
    }
    if path.ends_with("/api/oauth/profile") {
        return json(json!({"account": {"uuid": "uuid-a", "email": "a@x.com"}, "organization": {"uuid": "org-a"}}));
    }
    if path.ends_with("/chat/completions") {
        let chunks = [
            json!({"id": "c1", "choices": [{"index": 0, "delta": {"role": "assistant", "content": "hi from openrouter"}}]}),
            json!({"id": "c1", "choices": [{"index": 0, "delta": {}, "finish_reason": "stop"}], "usage": {"prompt_tokens": 3, "completion_tokens": 2}}),
        ];
        let mut body: String = chunks.iter().map(|chunk| format!("data: {chunk}\n\n")).collect();
        body.push_str("data: [DONE]\n\n");
        return (200, String::new(), "text/event-stream", body);
    }
    if limited {
        let headers = format!("anthropic-ratelimit-unified-status: rejected\r\nanthropic-ratelimit-unified-reset: {resets_at}\r\n");
        let body = json!({"type": "error", "error": {"type": "rate_limit_error", "message": "Error"}});
        return (429, headers, "application/json", body.to_string());
    }
    let events = [
        json!({"type": "message_start", "message": {"id": "msg_1", "model": "claude", "usage": {"input_tokens": 3, "output_tokens": 1}}}),
        json!({"type": "content_block_start", "index": 0, "content_block": {"type": "text", "text": ""}}),
        json!({"type": "content_block_delta", "index": 0, "delta": {"type": "text_delta", "text": "hi from claude"}}),
        json!({"type": "content_block_stop", "index": 0}),
        json!({"type": "message_delta", "delta": {"stop_reason": "end_turn"}, "usage": {"output_tokens": 1}}),
        json!({"type": "message_stop"}),
    ];
    let body = events.iter().map(|event| format!("event: {}\ndata: {event}\n\n", event["type"].as_str().expect("type"))).collect();
    (200, String::new(), "text/event-stream", body)
}

fn models_asked(seen: &Seen, path_ends: &str) -> Vec<String> {
    seen.lock().expect("lock").iter().filter(|(path, _)| path.ends_with(path_ends)).map(|(_, model)| model.clone()).collect()
}

/// A Claktory signed in to Claude (account a) and, with `openrouter`, to
/// OpenRouter with a key.
async fn claktory(openrouter: bool, reset: Arc<AtomicI64>) -> (std::path::PathBuf, Arc<Factory>, Client, View, Seen) {
    let dir = temp_dir();
    let (base_url, seen) = providers(reset).await;
    let file = dir.join("auth.json");
    let mut auth = json!({"anthropic": account("a")});
    if openrouter {
        auth["openrouter"] = json!({"type": "api_key", "key": "sk-or-test"});
    }
    std::fs::write(&file, auth.to_string()).expect("credentials");
    let factory = Factory::open(crate::common::home(&dir), Arc::new(registry_over(file, &base_url))).await.expect("open");
    let (client, view) = serve(&factory, &dir).await;
    (dir, factory, client, view, seen)
}

async fn create(client: &mut Client, view: &mut View, name: &str, model: &str, effort: &str) {
    let create =
        Request::CreateAgent(v1::CreateAgent { name: name.into(), model: model.into(), effort: effort.into(), ..Default::default() });
    assert!(matches!(apply_request(client, view, create).await, Body::Ack(_)));
}

/// Follows `view` until `done`, failing with what it waited for and where
/// the agents and chats stood instead of a bare timeout.
async fn until(client: &mut Client, view: &mut View, what: &str, done: impl FnMut(&View) -> bool) {
    if tokio::time::timeout(std::time::Duration::from_secs(45), view.follow(client, done)).await.is_err() {
        let agents: Vec<String> = view
            .workspace
            .agents
            .iter()
            .map(|agent| {
                format!("{} {:?} {} wait {:?} switch {:?}", agent.id, agent.status(), agent.model, agent.usage_wait, agent.usage_switch)
            })
            .collect();
        let chats: Vec<String> = view
            .workspace
            .chats
            .iter()
            .map(|chat| format!("{} {:?} {} wait {:?}", chat.id, chat.status(), chat.model, chat.usage_wait))
            .collect();
        panic!("still waiting for {what}: {agents:#?} {chats:#?}");
    }
}

fn chat(view: &View) -> v1::Chat {
    view.workspace.chats.first().cloned().expect("a chat")
}

fn waiting(agent: &v1::Agent) -> bool {
    agent.usage_wait.starts_with("Waiting for your Claude usage to reset at ")
}

#[tokio::test]
async fn waiting_agents_and_a_chat_continue_on_openrouter_and_go_back_once_claude_has_room() {
    let reset = Arc::new(AtomicI64::new(0));
    let (dir, _factory, mut client, mut view, seen) = claktory(true, reset.clone()).await;
    create(&mut client, &mut view, "Builder", "anthropic/claude-opus-4-8", "max").await;
    create(&mut client, &mut view, "Helper", "anthropic/claude-opus-4-8", "low").await;
    apply_request(&mut client, &mut view, send_message("builder", "hello")).await;
    apply_request(&mut client, &mut view, send_message("helper", "hello")).await;
    let ask = Request::CreateChat(v1::CreateChat { text: "hello".into(), client_id: "c1".into() });
    apply_request(&mut client, &mut view, ask).await;
    until(&mut client, &mut view, "all three waiting with alternatives", |view| {
        waiting(view.agent("builder"))
            && waiting(view.agent("helper"))
            && view.workspace.chats.first().is_some_and(|chat| !chat.usage_wait.is_empty())
            && view.agent("builder").usage_wait_alternative.as_ref().is_some_and(|alternative| alternative.others_waiting == 2)
    })
    .await;

    // The same model on OpenRouter, the effort as close as it has; the chat
    // on the default model (the closest one when a model isn't there is
    // the unit tests').
    let builder = view.agent("builder").clone();
    let offered = builder.usage_wait_alternative.clone().expect("an alternative");
    assert_eq!(
        (offered.provider_name.as_str(), offered.model.as_str(), offered.model_name.as_str(), offered.same_model),
        ("OpenRouter", "openrouter/anthropic/claude-opus-4.8", "Claude Opus 4.8", true)
    );
    assert_eq!(offered.effort_changed, offered.effort != "max", "{offered:?}");
    assert!(!builder.usage_wait_no_other_provider);
    let in_chat = chat(&view).usage_wait_alternative.expect("the chat's alternative");
    assert_eq!((in_chat.model.as_str(), in_chat.same_model), ("openrouter/anthropic/claude-opus-5.5", true));
    assert_eq!(models_asked(&seen, "/chat/completions"), Vec::<String>::new(), "nothing on OpenRouter before the person taps");

    // All of them, from Builder's wait line: as many as the person saw, or
    // nothing switches.
    let stale =
        Request::ContinueOnAlternative(v1::ContinueOnAlternative { agent_id: "builder".into(), all_waiting: true, expected_others: 1 });
    let refused = client.request(stale).await;
    assert!(
        matches!(refused.body, Body::Error(ref error) if error.code() == ErrorCode::InvalidRequest && error.message == "3 are waiting now, not 2. Choose again."),
        "{:?}",
        refused.body
    );
    assert_eq!(models_asked(&seen, "/chat/completions"), Vec::<String>::new());
    let all =
        Request::ContinueOnAlternative(v1::ContinueOnAlternative { agent_id: "builder".into(), all_waiting: true, expected_others: 2 });
    assert!(matches!(apply_request(&mut client, &mut view, all).await, Body::Ack(_)));
    until(&mut client, &mut view, "all three answered on OpenRouter", |view| {
        ["builder", "helper"].iter().all(|id| view.agent(id).status() == v1::AgentStatus::Idle && view.agent(id).usage_wait.is_empty())
            && chat(view).status() == v1::AgentStatus::Idle
    })
    .await;
    let asked = models_asked(&seen, "/chat/completions");
    assert_eq!(asked.iter().filter(|model| *model == "anthropic/claude-opus-4.8").count(), 2, "{asked:?}");
    assert!(asked.iter().any(|model| model == "anthropic/claude-opus-5.5"), "{asked:?}");
    let builder = view.agent("builder").clone();
    assert_eq!((builder.model.as_str(), builder.effort.as_str()), ("openrouter/anthropic/claude-opus-4.8", offered.effort.as_str()));
    assert!(builder.usage_wait_alternative.is_none());
    let switched = builder.usage_switch.clone().expect("where it came from");
    assert_eq!(
        (switched.provider_name.as_str(), switched.model.as_str(), switched.model_name.as_str(), switched.ready),
        ("Claude", "anthropic/claude-opus-4-8", "Claude Opus 4.8", false)
    );
    assert_eq!(chat(&view).model, "openrouter/anthropic/claude-opus-5.5");

    // Claude has room again, seen as Claktory sees an early reset: signed
    // in to again (an upgraded plan, say). Back to Claude, one, then all.
    reset.store(i64::try_from(now_ms() / 1000).expect("now") - 1, Ordering::SeqCst);
    let mut again = account("a");
    again["factoryLabel"] = json!("a@x.com");
    std::fs::write(dir.join("auth.json"), json!({"anthropic": again, "openrouter": {"type": "api_key", "key": "sk-or-test"}}).to_string())
        .expect("credentials");
    let try_claude = Request::SwitchBack(v1::SwitchBack { agent_id: "helper".into(), all: false, expected_others: 0 });
    assert!(matches!(apply_request(&mut client, &mut view, try_claude).await, Body::Ack(_)));
    assert_eq!((view.agent("helper").model.as_str(), view.agent("helper").effort.as_str()), ("anthropic/claude-opus-4-8", "low"));
    assert!(view.agent("helper").usage_switch.is_none());
    until(&mut client, &mut view, "builder's switch ready", |view| {
        view.agent("builder").usage_switch.as_ref().is_some_and(|switch| switch.ready)
    })
    .await;
    assert_eq!(view.agent("builder").usage_switch.clone().expect("switched").others_ready, 1, "the chat too");
    let stale = client.request(Request::SwitchBack(v1::SwitchBack { agent_id: "builder".into(), all: true, expected_others: 2 })).await;
    assert!(
        matches!(stale.body, Body::Error(ref error) if error.message == "2 are ready to go back now, not 3. Choose again."),
        "{:?}",
        stale.body
    );
    let back = Request::SwitchBack(v1::SwitchBack { agent_id: "builder".into(), all: true, expected_others: 1 });
    assert!(matches!(apply_request(&mut client, &mut view, back).await, Body::Ack(_)));
    assert_eq!((view.agent("builder").model.as_str(), view.agent("builder").effort.as_str()), ("anthropic/claude-opus-4-8", "max"));
    assert_eq!(chat(&view).model, "anthropic/claude-opus-5-5");
    assert!(view.agent("builder").usage_switch.is_none() && chat(&view).usage_switch.is_none());

    // Nothing to go back from, nobody waiting, nobody there.
    let again = client.request(Request::SwitchBack(v1::SwitchBack { agent_id: "builder".into(), all: false, expected_others: 0 })).await;
    assert!(matches!(again.body, Body::Error(ref error) if error.code() == ErrorCode::InvalidRequest), "{:?}", again.body);
    let idle = client
        .request(Request::ContinueOnAlternative(v1::ContinueOnAlternative {
            agent_id: "builder".into(),
            all_waiting: false,
            expected_others: 0,
        }))
        .await;
    assert!(
        matches!(idle.body, Body::Error(ref error) if error.code() == ErrorCode::InvalidRequest && error.message == "Builder isn't waiting anymore."),
        "{:?}",
        idle.body
    );
    let nobody = client
        .request(Request::ContinueOnAlternative(v1::ContinueOnAlternative {
            agent_id: "nobody".into(),
            all_waiting: false,
            expected_others: 0,
        }))
        .await;
    assert!(matches!(nobody.body, Body::Error(ref error) if error.code() == ErrorCode::NotFound), "{:?}", nobody.body);
}

#[tokio::test]
async fn a_switch_is_kept_across_a_restart_and_the_persons_own_model_change_ends_it() {
    let reset = Arc::new(AtomicI64::new(0));
    let (dir, factory, mut client, mut view, _seen) = claktory(true, reset.clone()).await;
    create(&mut client, &mut view, "Builder", "anthropic/claude-sonnet-4-6", "medium").await;
    apply_request(&mut client, &mut view, send_message("builder", "hello")).await;
    until(&mut client, &mut view, "builder's alternative", |view| view.agent("builder").usage_wait_alternative.is_some()).await;
    let one =
        Request::ContinueOnAlternative(v1::ContinueOnAlternative { agent_id: "builder".into(), all_waiting: false, expected_others: 0 });
    assert!(matches!(apply_request(&mut client, &mut view, one).await, Body::Ack(_)));
    until(&mut client, &mut view, "builder answered", |view| view.agent("builder").status() == v1::AgentStatus::Idle).await;
    assert_eq!(view.agent("builder").model, "openrouter/anthropic/claude-sonnet-4.6");
    drop(client);
    drop(factory);

    let file = dir.join("auth.json");
    let (base_url, _seen) = providers(reset).await;
    let factory = Factory::open(crate::common::home(&dir), Arc::new(registry_over(file, &base_url))).await.expect("open");
    let (mut client, mut view) = serve(&factory, &dir).await;
    let kept = view.agent("builder").usage_switch.clone().expect("kept");
    assert_eq!(kept.model, "anthropic/claude-sonnet-4-6");

    let chosen =
        v1::UpdateAgent { agent_id: "builder".into(), model: Some("openrouter/anthropic/claude-opus-4.8".into()), ..Default::default() };
    assert!(matches!(apply_request(&mut client, &mut view, Request::UpdateAgent(chosen)).await, Body::Ack(_)));
    assert!(view.agent("builder").usage_switch.is_none(), "the person chose its model");
}

#[tokio::test]
async fn with_no_other_provider_signed_in_the_wait_says_another_would_carry_it() {
    let (_dir, _factory, mut client, mut view, _seen) = claktory(false, Arc::new(AtomicI64::new(0))).await;
    create(&mut client, &mut view, "Builder", "anthropic/claude-opus-4-8", "low").await;
    apply_request(&mut client, &mut view, send_message("builder", "hello")).await;
    until(&mut client, &mut view, "builder waiting", |view| waiting(view.agent("builder"))).await;
    let builder = view.agent("builder");
    assert!(builder.usage_wait_no_other_provider && builder.usage_wait_alternative.is_none());
    let refused = client
        .request(Request::ContinueOnAlternative(v1::ContinueOnAlternative {
            agent_id: "builder".into(),
            all_waiting: false,
            expected_others: 0,
        }))
        .await;
    assert!(
        matches!(refused.body, Body::Error(ref error) if error.message == "No other provider can run Builder's model right now."),
        "{:?}",
        refused.body
    );
}
