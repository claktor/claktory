//! Continue on another provider: agents and a chat waiting out their
//! Claude usage limit offer to continue on OpenRouter (the same model, or
//! the closest), switch at the person's tap, carry on there, and offer to
//! go back once Claude has room again. Against one mock server for both
//! providers.

use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};
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
pub(crate) type Seen = Arc<Mutex<Vec<(String, String)>>>;

/// Claude (account "a") at its limit until `reset` (unix seconds, 0 for
/// never), OpenRouter answering every request.
pub(crate) async fn providers(reset: Arc<AtomicI64>) -> (String, Seen) {
    providers_on(TcpListener::bind("127.0.0.1:0").await.expect("bind"), reset)
}

/// `providers`, on `listener`.
pub(crate) fn providers_on(listener: TcpListener, reset: Arc<AtomicI64>) -> (String, Seen) {
    serve_providers(listener, reset, Arc::new(AtomicU64::new(0)))
}

/// `providers`, Claude's refusals answered `slow` ms late.
fn serve_providers(listener: TcpListener, reset: Arc<AtomicI64>, slow: Arc<AtomicU64>) -> (String, Seen) {
    serve_slowly(listener, Arc::new(move |path, _| answer(path, reset.load(Ordering::SeqCst))), slow)
}

/// An answer: status, more header lines ("name: value\r\n" each), content
/// type and body.
pub(crate) type Answer = (u16, String, &'static str, String);

/// What a mock answers a request: its path and JSON body in, its answer out.
pub(crate) type Route = Arc<dyn Fn(&str, &Value) -> Answer + Send + Sync>;

/// A mock of the providers on `listener`, answering by `route`.
pub(crate) fn serve_routes(listener: TcpListener, route: Route) -> (String, Seen) {
    serve_slowly(listener, route, Arc::new(AtomicU64::new(0)))
}

/// `serve_routes`, its refusals (429) answered `slow` ms late.
fn serve_slowly(listener: TcpListener, route: Route, slow: Arc<AtomicU64>) -> (String, Seen) {
    let base_url = format!("http://{}", listener.local_addr().expect("addr"));
    let seen: Seen = Arc::new(Mutex::new(Vec::new()));
    let recorded = seen.clone();
    tokio::spawn(async move {
        loop {
            let Ok((mut socket, _)) = listener.accept().await else { return };
            let (recorded, route, slow) = (recorded.clone(), route.clone(), slow.clone());
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
                let (status, extra, content_type, body) = route(&path, &body);
                if status == 429 {
                    tokio::time::sleep(std::time::Duration::from_millis(slow.load(Ordering::SeqCst))).await;
                }
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

fn answer(path: &str, reset: i64) -> Answer {
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
        return openrouter_reply();
    }
    if limited {
        let headers = format!("anthropic-ratelimit-unified-status: rejected\r\nanthropic-ratelimit-unified-reset: {resets_at}\r\n");
        let body = json!({"type": "error", "error": {"type": "rate_limit_error", "message": "Error"}});
        return (429, headers, "application/json", body.to_string());
    }
    claude_reply()
}

/// OpenRouter's answer: "hi from openrouter".
pub(crate) fn openrouter_reply() -> Answer {
    let chunks = [
        json!({"id": "c1", "choices": [{"index": 0, "delta": {"role": "assistant", "content": "hi from openrouter"}}]}),
        json!({"id": "c1", "choices": [{"index": 0, "delta": {}, "finish_reason": "stop"}], "usage": {"prompt_tokens": 3, "completion_tokens": 2}}),
    ];
    let mut body: String = chunks.iter().map(|chunk| format!("data: {chunk}\n\n")).collect();
    body.push_str("data: [DONE]\n\n");
    (200, String::new(), "text/event-stream", body)
}

/// Claude's answer: "hi from claude".
pub(crate) fn claude_reply() -> Answer {
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

pub(crate) fn models_asked(seen: &Seen, path_ends: &str) -> Vec<String> {
    seen.lock().expect("lock").iter().filter(|(path, _)| path.ends_with(path_ends)).map(|(_, model)| model.clone()).collect()
}

/// A Claktory signed in to Claude (account a) and, with `openrouter`, to
/// OpenRouter with a key.
pub(crate) async fn claktory(openrouter: bool, reset: Arc<AtomicI64>) -> (std::path::PathBuf, Arc<Factory>, Client, View, Seen) {
    let dir = temp_dir();
    let (base_url, seen) = providers(reset).await;
    let file = dir.join("auth.json");
    // Named already, as after its first usage read, so that reading it
    // again changes nothing in the providers' listing.
    let mut claude = account("a");
    claude["factoryLabel"] = json!("a@x.com");
    let mut auth = json!({"anthropic": claude});
    if openrouter {
        auth["openrouter"] = json!({"type": "api_key", "key": "sk-or-test"});
    }
    std::fs::write(&file, auth.to_string()).expect("credentials");
    let factory = Factory::open(crate::common::home(&dir), Arc::new(registry_over(file, &base_url))).await.expect("open");
    let (client, view) = serve(&factory, &dir).await;
    (dir, factory, client, view, seen)
}

pub(crate) async fn create(client: &mut Client, view: &mut View, name: &str, model: &str, effort: &str) {
    let create =
        Request::CreateAgent(v1::CreateAgent { name: name.into(), model: model.into(), effort: effort.into(), ..Default::default() });
    assert!(matches!(apply_request(client, view, create).await, Body::Ack(_)));
}

/// Follows `view` until `done`, failing with what it waited for and where
/// the agents and chats stood instead of a bare timeout.
pub(crate) async fn until(client: &mut Client, view: &mut View, what: &str, done: impl FnMut(&View) -> bool) {
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
    let (dir, factory, mut client, mut view, seen) = claktory(true, reset.clone()).await;
    // Going back by itself is `goes_back_by_itself_once_claude_has_room…`.
    factory.switch_back_only_by_tap();
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
async fn a_waiting_chat_put_on_another_model_carries_on_at_once_and_a_switch_ends_when_the_person_chooses() {
    let (_dir, _factory, mut client, mut view, seen) = claktory(true, Arc::new(AtomicI64::new(0))).await;
    let ask = Request::CreateChat(v1::CreateChat { text: "hello".into(), client_id: "c1".into() });
    apply_request(&mut client, &mut view, ask).await;
    until(&mut client, &mut view, "the chat waiting", |view| view.workspace.chats.first().is_some_and(|chat| !chat.usage_wait.is_empty()))
        .await;
    let id = chat(&view).id;

    // The person picks a model of OpenRouter's: it asks again at once.
    let chosen = Request::SetChatModel(v1::SetChatModel { chat_id: id.clone(), model: "openrouter/anthropic/claude-opus-4.8".into() });
    assert!(matches!(apply_request(&mut client, &mut view, chosen).await, Body::Ack(_)));
    until(&mut client, &mut view, "the chat answered on OpenRouter", |view| {
        chat(view).status() == v1::AgentStatus::Idle && chat(view).usage_wait.is_empty()
    })
    .await;
    let asked = models_asked(&seen, "/chat/completions");
    assert!(!asked.is_empty() && asked.iter().all(|model| model == "anthropic/claude-opus-4.8"), "its reply and its title: {asked:?}");
    assert!(chat(&view).usage_switch.is_none(), "chosen, not switched");

    // Switched by Continue on OpenRouter, then chosen by the person: the
    // switch is over.
    let back = Request::SetChatModel(v1::SetChatModel { chat_id: id.clone(), model: "anthropic/claude-opus-4-8".into() });
    assert!(matches!(apply_request(&mut client, &mut view, back).await, Body::Ack(_)));
    apply_request(
        &mut client,
        &mut view,
        Request::SendChatMessage(v1::SendChatMessage { chat_id: id.clone(), text: "again".into(), ..Default::default() }),
    )
    .await;
    until(&mut client, &mut view, "the chat's alternative", |view| chat(view).usage_wait_alternative.is_some()).await;
    let one = Request::ContinueOnAlternative(v1::ContinueOnAlternative { agent_id: id.clone(), all_waiting: false, expected_others: 0 });
    assert!(matches!(apply_request(&mut client, &mut view, one).await, Body::Ack(_)));
    until(&mut client, &mut view, "the switched chat answered", |view| chat(view).status() == v1::AgentStatus::Idle).await;
    assert!(chat(&view).usage_switch.is_some());
    let mine = Request::SetChatModel(v1::SetChatModel { chat_id: id, model: "openrouter/anthropic/claude-sonnet-4.6".into() });
    assert!(matches!(apply_request(&mut client, &mut view, mine).await, Body::Ack(_)));
    assert_eq!(chat(&view).model, "openrouter/anthropic/claude-sonnet-4.6");
    assert!(chat(&view).usage_switch.is_none(), "the person chose its model");
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

/// Once Claude answers again (Try now here; a usage read showing
/// room does the same), its account's limit mark is gone from the
/// workspace at once, so agents switched away are offered Back to Claude
/// then, not at the old reset time.
#[tokio::test]
async fn back_to_claude_is_offered_as_soon_as_claude_answers_again() {
    let reset = Arc::new(AtomicI64::new(0));
    let (_dir, factory, mut client, mut view, _seen) = claktory(true, reset.clone()).await;
    factory.switch_back_only_by_tap();
    create(&mut client, &mut view, "Builder", "anthropic/claude-opus-4-8", "low").await;
    create(&mut client, &mut view, "Helper", "anthropic/claude-opus-4-8", "low").await;
    apply_request(&mut client, &mut view, send_message("builder", "hello")).await;
    apply_request(&mut client, &mut view, send_message("helper", "hello")).await;
    until(&mut client, &mut view, "both waiting", |view| waiting(view.agent("builder")) && waiting(view.agent("helper"))).await;
    let one =
        Request::ContinueOnAlternative(v1::ContinueOnAlternative { agent_id: "builder".into(), all_waiting: false, expected_others: 0 });
    assert!(matches!(apply_request(&mut client, &mut view, one).await, Body::Ack(_)));
    until(&mut client, &mut view, "builder answered on OpenRouter", |view| view.agent("builder").status() == v1::AgentStatus::Idle).await;
    assert!(!view.agent("builder").usage_switch.clone().expect("switched").ready);

    // Claude has room again, unseen until Helper's Try now asks it.
    reset.store(i64::try_from(now_ms() / 1000).expect("now") - 1, Ordering::SeqCst);
    let try_now = Request::TryAgainNow(v1::TryAgainNow { agent_id: "helper".into() });
    assert!(matches!(apply_request(&mut client, &mut view, try_now).await, Body::Ack(_)));
    until(&mut client, &mut view, "builder's switch ready", |view| {
        view.agent("helper").status() == v1::AgentStatus::Idle
            && view.agent("builder").usage_switch.as_ref().is_some_and(|switch| switch.ready)
    })
    .await;
    let claude = view.workspace.providers.iter().find(|provider| provider.id == "anthropic").expect("Claude").clone();
    assert_eq!(claude.accounts[0].exhausted_until, 0, "{claude:?}");
}

fn notices(view: &View) -> Vec<String> {
    crate::common::describe(&view.entries).into_iter().filter(|entry| entry.starts_with("notice: ")).collect()
}

/// A switched agent goes back to Claude by itself once
/// Claude has room, and says so; one that reaches the limit again within
/// minutes of going back stays on OpenRouter until that limit resets.
#[tokio::test]
async fn goes_back_by_itself_once_claude_has_room_and_stays_when_it_bounces() {
    let reset = Arc::new(AtomicI64::new(0));
    let (_dir, _factory, mut client, mut view, seen) = claktory(true, reset.clone()).await;
    create(&mut client, &mut view, "Builder", "anthropic/claude-opus-4-8", "low").await;
    create(&mut client, &mut view, "Helper", "anthropic/claude-opus-4-8", "low").await;
    apply_request(&mut client, &mut view, Request::SelectAgent(v1::SelectAgent { agent_id: "builder".into() })).await;
    apply_request(&mut client, &mut view, send_message("builder", "hello")).await;
    apply_request(&mut client, &mut view, send_message("helper", "hello")).await;
    until(&mut client, &mut view, "both waiting", |view| waiting(view.agent("builder")) && waiting(view.agent("helper"))).await;
    let resets_at = view.agent("builder").usage_wait_until_ms;
    let one =
        Request::ContinueOnAlternative(v1::ContinueOnAlternative { agent_id: "builder".into(), all_waiting: false, expected_others: 0 });
    assert!(matches!(apply_request(&mut client, &mut view, one).await, Body::Ack(_)));
    until(&mut client, &mut view, "builder answered on OpenRouter", |view| view.agent("builder").status() == v1::AgentStatus::Idle).await;
    let switched = view.agent("builder").usage_switch.clone().expect("switched");
    assert!(switched.goes_back && !switched.ready, "{switched:?}");
    assert_eq!(switched.goes_back_at_ms, resets_at, "when Claude's limit resets");
    // It hears who moved it, and why.
    let told = || crate::common::describe(&view.entries).into_iter().filter(|line| line.starts_with("event from Claktory")).collect::<Vec<_>>();
    assert!(
        told().iter().any(|line| line.contains(" (the person chose OpenRouter on ") && line.contains(", while your Claude usage is at its limit)")),
        "{:?}",
        told()
    );

    // Claude has room again, as Helper's Try now finds: Builder goes back.
    reset.store(i64::try_from(now_ms() / 1000).expect("now") - 1, Ordering::SeqCst);
    let try_now = Request::TryAgainNow(v1::TryAgainNow { agent_id: "helper".into() });
    assert!(matches!(apply_request(&mut client, &mut view, try_now).await, Body::Ack(_)));
    until(&mut client, &mut view, "builder back on Claude", |view| {
        view.agent("builder").model == "anthropic/claude-opus-4-8" && view.agent("builder").usage_switch.is_none()
    })
    .await;
    assert_eq!(view.agent("builder").effort, "low");
    view.follow(&mut client, |view| notices(view).iter().any(|notice| notice.starts_with("notice: Back on "))).await;
    assert!(
        notices(&view).contains(&"notice: Back on Claude Opus 4.8: your Claude usage has room again.".to_owned()),
        "{:?}",
        notices(&view)
    );
    apply_request(&mut client, &mut view, send_message("builder", "back?")).await;
    let back = "(Claktory switched you back by itself, since your Claude usage has room again)";
    until(&mut client, &mut view, "builder told why it's back", |view| {
        crate::common::describe(&view.entries).iter().any(|line| line.starts_with("event from Claktory") && line.contains(back))
    })
    .await;
    until(&mut client, &mut view, "builder answered on Claude", |view| view.agent("builder").status() == v1::AgentStatus::Idle).await;

    // At the limit again at once: continued again, it stays there until
    // this limit resets.
    reset.store(0, Ordering::SeqCst);
    apply_request(&mut client, &mut view, send_message("builder", "again")).await;
    until(&mut client, &mut view, "builder waiting again", |view| waiting(view.agent("builder"))).await;
    let again =
        Request::ContinueOnAlternative(v1::ContinueOnAlternative { agent_id: "builder".into(), all_waiting: false, expected_others: 0 });
    assert!(matches!(apply_request(&mut client, &mut view, again).await, Body::Ack(_)));
    until(&mut client, &mut view, "builder answered on OpenRouter again", |view| {
        view.agent("builder").status() == v1::AgentStatus::Idle && view.agent("builder").usage_switch.is_some()
    })
    .await;
    assert!(!view.agent("builder").usage_switch.clone().expect("switched").goes_back);
    assert_eq!(models_asked(&seen, "/chat/completions").len(), 2);
}

/// With Continue automatically on (kept in factory.db),
/// an agent whose wait on Claude's limit starts continues on OpenRouter by
/// itself, says so, and is a switch like any other; off, nothing moves.
#[tokio::test]
async fn with_continue_automatically_on_a_wait_on_the_limit_continues_by_itself() {
    let reset = Arc::new(AtomicI64::new(0));
    let (dir, factory, mut client, mut view, seen) = claktory(true, reset.clone()).await;
    factory.switch_back_only_by_tap();
    assert!(!view.workspace.usage_auto_continue, "off unless turned on");
    let on = Request::SetUsageAutoContinue(v1::SetUsageAutoContinue { on: true });
    assert!(matches!(apply_request(&mut client, &mut view, on).await, Body::Ack(_)));
    assert!(view.workspace.usage_auto_continue);

    create(&mut client, &mut view, "Builder", "anthropic/claude-opus-4-8", "low").await;
    apply_request(&mut client, &mut view, Request::SelectAgent(v1::SelectAgent { agent_id: "builder".into() })).await;
    apply_request(&mut client, &mut view, send_message("builder", "hello")).await;
    until(&mut client, &mut view, "builder answered on OpenRouter", |view| {
        view.agent("builder").status() == v1::AgentStatus::Idle && view.agent("builder").usage_switch.is_some()
    })
    .await;
    assert_eq!(view.agent("builder").model, "openrouter/anthropic/claude-opus-4.8");
    assert_eq!(models_asked(&seen, "/chat/completions"), vec!["anthropic/claude-opus-4.8".to_owned()]);
    view.follow(&mut client, |view| notices(view).iter().any(|notice| notice.starts_with("notice: Continued on "))).await;
    assert!(
        notices(&view).contains(&"notice: Continued on OpenRouter automatically: your Claude usage reached its limit.".to_owned()),
        "{:?}",
        notices(&view)
    );
    drop(client);
    drop(factory);

    // Kept across a restart; turned off, a new wait stays put.
    let (base_url, seen) = providers(reset).await;
    let factory = Factory::open(crate::common::home(&dir), Arc::new(registry_over(dir.join("auth.json"), &base_url))).await.expect("open");
    let (mut client, mut view) = serve(&factory, &dir).await;
    assert!(view.workspace.usage_auto_continue);
    let off = Request::SetUsageAutoContinue(v1::SetUsageAutoContinue { on: false });
    assert!(matches!(apply_request(&mut client, &mut view, off).await, Body::Ack(_)));
    create(&mut client, &mut view, "Helper", "anthropic/claude-opus-4-8", "low").await;
    apply_request(&mut client, &mut view, send_message("helper", "hello")).await;
    until(&mut client, &mut view, "helper waiting", |view| waiting(view.agent("helper"))).await;
    assert_eq!(view.agent("helper").model, "anthropic/claude-opus-4-8");
    assert!(view.agent("helper").usage_switch.is_none());
    assert_eq!(models_asked(&seen, "/chat/completions"), Vec::<String>::new());
}

/// U19: while agents are switched away from Claude and it still has
/// no room, a new chat, and a new agent on the default model, start where
/// they are, each as a switch that goes back with them; an agent given
/// another model starts on it as asked.
#[tokio::test]
async fn new_chats_and_agents_start_where_the_switched_ones_are() {
    let reset = Arc::new(AtomicI64::new(0));
    let (_dir, factory, mut client, mut view, _seen) = claktory(true, reset.clone()).await;
    factory.switch_back_only_by_tap();
    create(&mut client, &mut view, "Builder", "anthropic/claude-opus-4-8", "low").await;
    apply_request(&mut client, &mut view, send_message("builder", "hello")).await;
    until(&mut client, &mut view, "builder waiting", |view| waiting(view.agent("builder"))).await;
    let one =
        Request::ContinueOnAlternative(v1::ContinueOnAlternative { agent_id: "builder".into(), all_waiting: false, expected_others: 0 });
    assert!(matches!(apply_request(&mut client, &mut view, one).await, Body::Ack(_)));

    let ask = Request::CreateChat(v1::CreateChat { text: String::new(), client_id: "c1".into() });
    apply_request(&mut client, &mut view, ask).await;
    let new_chat = chat(&view);
    assert_eq!(new_chat.model, "openrouter/anthropic/claude-opus-5.5");
    assert_eq!(new_chat.usage_switch.expect("a switch").model, "anthropic/claude-opus-5-5");

    let default = view.workspace.settings.as_ref().map(|settings| settings.default_model.clone()).unwrap_or_default();
    assert_eq!(default, "anthropic/claude-opus-5-5");
    create(&mut client, &mut view, "Newbie", &default, "high").await;
    let newbie = view.agent("newbie").clone();
    assert_eq!(newbie.model, "openrouter/anthropic/claude-opus-5.5");
    assert_eq!(newbie.usage_switch.expect("a switch").model, default);
    create(&mut client, &mut view, "Chosen", "anthropic/claude-sonnet-4-6", "low").await;
    assert_eq!(view.agent("chosen").model, "anthropic/claude-sonnet-4-6", "a model chosen for it stays");
    assert!(view.agent("chosen").usage_switch.is_none());
}

/// A chat put on another provider's model while its request to Claude is
/// still out (Continue for all wakes it with the first one's change,
/// before its own): refused by Claude once its model is OpenRouter's, it
/// carries on there at once, rather than wait out Claude's limit under
/// OpenRouter's name.
#[tokio::test]
async fn a_chat_moved_while_it_asks_carries_on_without_a_false_wait() {
    let dir = temp_dir();
    let slow = Arc::new(AtomicU64::new(0));
    let (base_url, seen) =
        serve_providers(TcpListener::bind("127.0.0.1:0").await.expect("bind"), Arc::new(AtomicI64::new(0)), slow.clone());
    let file = dir.join("auth.json");
    let mut claude = account("a");
    claude["factoryLabel"] = json!("a@x.com");
    std::fs::write(&file, json!({"anthropic": claude, "openrouter": {"type": "api_key", "key": "sk-or-test"}}).to_string())
        .expect("credentials");
    let factory = Factory::open(crate::common::home(&dir), Arc::new(registry_over(file, &base_url))).await.expect("open");
    factory.switch_back_only_by_tap();
    let (mut client, mut view) = serve(&factory, &dir).await;
    let ask = Request::CreateChat(v1::CreateChat { text: "hello".into(), client_id: "c1".into() });
    apply_request(&mut client, &mut view, ask).await;
    until(&mut client, &mut view, "the chat waiting", |view| view.workspace.chats.first().is_some_and(|chat| !chat.usage_wait.is_empty()))
        .await;

    // Try now: it asks Claude again, whose refusal takes a while; meanwhile
    // the person puts it on OpenRouter.
    let id = chat(&view).id;
    let claude_asked = |seen: &Seen| seen.lock().expect("lock").iter().filter(|(path, _)| path.ends_with("/v1/messages")).count();
    let before = claude_asked(&seen);
    slow.store(1500, Ordering::SeqCst);
    assert!(matches!(
        apply_request(&mut client, &mut view, Request::TryAgainNow(v1::TryAgainNow { agent_id: id.clone() })).await,
        Body::Ack(_)
    ));
    let asked = tokio::time::timeout(std::time::Duration::from_secs(20), async {
        while claude_asked(&seen) == before {
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    })
    .await;
    assert!(asked.is_ok(), "the chat asked Claude again");
    let moved = Request::SetChatModel(v1::SetChatModel { chat_id: id.clone(), model: "openrouter/anthropic/claude-opus-5.5".into() });
    assert!(matches!(apply_request(&mut client, &mut view, moved).await, Body::Ack(_)));
    let mut elsewhere = Vec::new();
    until(&mut client, &mut view, "the chat answered on OpenRouter", |view| {
        elsewhere.extend(view.workspace.provider_waits.iter().filter(|wait| wait.provider_id != "anthropic").cloned());
        chat(view).status() == v1::AgentStatus::Idle && chat(view).usage_wait.is_empty()
    })
    .await;
    assert!(elsewhere.is_empty(), "{elsewhere:?}");
    let asked = models_asked(&seen, "/chat/completions");
    assert!(asked.iter().any(|model| model == "anthropic/claude-opus-5.5"), "{asked:?}");
}
