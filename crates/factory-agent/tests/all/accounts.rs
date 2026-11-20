//! Several accounts per provider, against mock Anthropic and Codex servers:
//! requests go to the first account with capacity, carry on with the next
//! when one reaches its usage limit (and only then), fail saying when the
//! first resets once all have, and come back after the reset; a credential
//! stored before accounts is the first one, still readable the old way.

use std::sync::Arc;
use std::time::Duration;

use base64::Engine;
use factory_agent::accounts::{Refused, refused, refused_error};
use factory_agent::auth::{AuthEvent, AuthFuture, AuthInteraction, AuthPrompt, Env, OAuthFlow, RequestAuth, refresh_refused, refusal};
use factory_agent::clock::now_ms;
use factory_agent::credentials::{Credential, FileCredentialStore};
use factory_agent::errors::Error;
use factory_agent::limits::{Identity, ResetAnswer, UsageReset};
use factory_agent::providers::models::{AccountNote, Notes, Registry, default_providers};
use factory_agent::providers::{Provider, catalog};
use factory_agent::stream::{LlmContext, StreamOptions, Transport};
use factory_agent::types::{AssistantMessage, AssistantPart, Message, Model, StopReason, StreamEvent, UserMessage};
use parking_lot::Mutex;
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio_util::sync::CancellationToken;

/// A request the mock server got: its path (with its query), bearer token
/// and body.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Seen {
    path: String,
    token: String,
    body: String,
}

/// An answer: status, headers, content type and body.
struct Answer {
    status: u16,
    headers: Vec<(String, String)>,
    content_type: &'static str,
    body: String,
}

type Route = Arc<dyn Fn(&Seen) -> Answer + Send + Sync>;

/// A provider's server: answers each request by its path and token, and keeps them.
struct Mock {
    base_url: String,
    seen: Arc<Mutex<Vec<Seen>>>,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for Mock {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl Mock {
    /// The tokens of the requests to `path`, in order.
    fn tokens(&self, path: &str) -> Vec<String> {
        self.seen.lock().iter().filter(|seen| seen.path.ends_with(path)).map(|seen| seen.token.clone()).collect()
    }
}

async fn mock(route: Route) -> Mock {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let base_url = format!("http://{}", listener.local_addr().expect("addr"));
    let seen = Arc::new(Mutex::new(Vec::new()));
    let recorded = seen.clone();
    let task = tokio::spawn(async move {
        loop {
            let Ok((mut socket, _)) = listener.accept().await else { return };
            let (route, recorded) = (route.clone(), recorded.clone());
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
                let header = |name: &str| {
                    head.lines()
                        .filter_map(|line| line.split_once(':'))
                        .find(|(key, _)| key.trim().eq_ignore_ascii_case(name))
                        .map(|(_, value)| value.trim().to_owned())
                };
                let length: usize = header("content-length").and_then(|length| length.parse().ok()).unwrap_or(0);
                while raw.len() < head_end + 4 + length {
                    let mut buffer = [0u8; 8192];
                    let Ok(read) = socket.read(&mut buffer).await else { return };
                    if read == 0 {
                        break;
                    }
                    raw.extend_from_slice(&buffer[..read]);
                }
                let path = head.split(' ').nth(1).unwrap_or_default().to_owned();
                let token = header("authorization")
                    .map(|value| value.trim_start_matches("Bearer ").to_owned())
                    .or_else(|| header("x-api-key"))
                    .unwrap_or_default();
                let body = String::from_utf8_lossy(&raw[(head_end + 4).min(raw.len())..]).into_owned();
                let seen = Seen { path, token, body };
                recorded.lock().push(seen.clone());
                let answer = route(&seen);
                let mut response = format!(
                    "HTTP/1.1 {} X\r\ncontent-type: {}\r\ncontent-length: {}\r\nconnection: close\r\n",
                    answer.status,
                    answer.content_type,
                    answer.body.len()
                );
                for (name, value) in &answer.headers {
                    response.push_str(&format!("{name}: {value}\r\n"));
                }
                response.push_str("\r\n");
                response.push_str(&answer.body);
                let _ = socket.write_all(response.as_bytes()).await;
                let _ = socket.shutdown().await;
            });
        }
    });
    Mock { base_url, seen, task }
}

fn json_answer(status: u16, headers: Vec<(String, String)>, body: Value) -> Answer {
    Answer { status, headers, content_type: "application/json", body: body.to_string() }
}

/// Anthropic's answer to a request over the subscription's usage limit.
fn claude_limit(resets_at_seconds: u64) -> Answer {
    json_answer(
        429,
        vec![
            ("anthropic-ratelimit-unified-status".into(), "rejected".into()),
            ("anthropic-ratelimit-unified-5h-status".into(), "rejected".into()),
            ("anthropic-ratelimit-unified-reset".into(), resets_at_seconds.to_string()),
            ("anthropic-ratelimit-unified-representative-claim".into(), "five_hour".into()),
        ],
        json!({"type": "error", "error": {"type": "rate_limit_error", "message": "Error"}}),
    )
}

/// A passing rate limit: a 429 that isn't the usage limit.
fn claude_rate_limit() -> Answer {
    json_answer(
        429,
        vec![("anthropic-ratelimit-unified-status".into(), "allowed".into()), ("retry-after".into(), "1".into())],
        json!({"type": "error", "error": {"type": "rate_limit_error", "message": "This request would exceed your account's rate limit. Please try again later."}}),
    )
}

/// A reply of "hi".
fn claude_reply() -> Answer {
    let events = [
        json!({"type": "message_start", "message": {"id": "msg_1", "model": "claude", "usage": {"input_tokens": 3, "output_tokens": 1}}}),
        json!({"type": "content_block_start", "index": 0, "content_block": {"type": "text", "text": ""}}),
        json!({"type": "content_block_delta", "index": 0, "delta": {"type": "text_delta", "text": "hi"}}),
        json!({"type": "content_block_stop", "index": 0}),
        json!({"type": "message_delta", "delta": {"stop_reason": "end_turn"}, "usage": {"output_tokens": 1}}),
        json!({"type": "message_stop"}),
    ];
    let body = events.iter().map(|event| format!("event: {}\ndata: {event}\n\n", event["type"].as_str().expect("type"))).collect();
    Answer { status: 200, headers: Vec::new(), content_type: "text/event-stream", body }
}

/// An Anthropic subscription's OAuth token, named after its account.
fn claude_token(account: &str) -> String {
    format!("sk-ant-oat01-{account}")
}

fn oauth(access: &str) -> Value {
    json!({"type": "oauth", "access": access, "refresh": format!("refresh-{access}"), "expires": now_ms() + 3_600_000})
}

/// The account's entry in the credentials file, as Factory writes it.
fn account(id: &str, label: &str, access: &str) -> Value {
    let mut value = oauth(access);
    value["factoryAccount"] = json!(id);
    value["factoryLabel"] = json!(label);
    value
}

/// A credentials file with the provider's `accounts`, in order.
fn store_accounts(file: &std::path::Path, provider: &str, accounts: Vec<Value>) {
    let mut accounts = accounts.into_iter();
    let mut first = accounts.next().expect("an account");
    let rest: Vec<Value> = accounts.collect();
    if !rest.is_empty() {
        first["factoryAccounts"] = Value::Array(rest);
    }
    std::fs::write(file, json!({ provider: first }).to_string()).expect("credentials");
}

fn no_env() -> Env {
    Arc::new(|_| None)
}

/// The real providers, Anthropic's and Codex's account endpoints on `base_url`.
fn registry(file: &std::path::Path, base_url: &str) -> Registry {
    let base: &'static str = Box::leak(base_url.to_owned().into_boxed_str());
    let mut providers = default_providers();
    for provider in &mut providers {
        provider.base_url = base;
    }
    Registry::with_providers(FileCredentialStore::new(file.to_path_buf()), no_env(), providers)
}

fn model(provider: &str, id: &str, base_url: &str) -> Model {
    let mut model = catalog::models(provider).into_iter().find(|model| model.id == id).expect("model");
    model.base_url = base_url.to_owned();
    model
}

fn claude(base_url: &str) -> Model {
    let id = catalog::models("anthropic").into_iter().next().expect("a Claude model").id;
    model("anthropic", &id, base_url)
}

fn context() -> LlmContext {
    LlmContext {
        system_prompt: Some("Be brief.".into()),
        messages: vec![Message::User(UserMessage::text("hello", now_ms()))],
        tools: Vec::new(),
    }
}

/// What the notes said.
fn recorder() -> (Notes, Arc<Mutex<Vec<AccountNote>>>) {
    let notes = Arc::new(Mutex::new(Vec::new()));
    let recorded = notes.clone();
    (Arc::new(move |note| recorded.lock().push(note)), notes)
}

/// Runs one request and returns how it ended: its reply's text, or its error.
async fn ask(registry: &Registry, notes: &Notes, model: &Model, options: StreamOptions) -> Result<String, String> {
    let stream = registry.stream_fn_with(Some(notes.clone()));
    let mut events = stream(model, context(), options).expect("stream");
    let mut last = None;
    let mut starts = 0;
    while let Some(event) = tokio::time::timeout(crate::support::server::HANG, events.next()).await.expect("the stream ends") {
        if matches!(event, StreamEvent::Start { .. }) {
            starts += 1;
        }
        last = Some(event);
    }
    match last.expect("an event") {
        StreamEvent::Done { message, .. } => {
            assert_eq!(starts, 1, "one response starts");
            let mut text = String::new();
            for part in &message.content {
                if let AssistantPart::Text { text: part, .. } = part {
                    text.push_str(part);
                }
            }
            Ok(text)
        }
        StreamEvent::Error { error, .. } => {
            assert_eq!(error.stop_reason, StopReason::Error);
            Err(error.error_message.unwrap_or_default())
        }
        other => panic!("{other:?}"),
    }
}

fn exhausted_until(registry_accounts: &[factory_agent::providers::models::AccountInfo]) -> Vec<(String, bool)> {
    registry_accounts.iter().map(|account| (account.label.clone(), account.exhausted_until > 0)).collect()
}

#[tokio::test]
async fn a_request_over_one_accounts_limit_carries_on_with_the_next_in_order() {
    let dir = tempfile::tempdir().expect("dir");
    let file = dir.path().join("auth.json");
    let resets = now_ms() / 1000 + 3 * 3600;
    let server = mock(Arc::new(move |seen| match seen.token.as_str() {
        token if token == claude_token("a") => claude_limit(resets),
        _ => claude_reply(),
    }))
    .await;
    store_accounts(
        &file,
        "anthropic",
        vec![
            account("a", "a@x.com", &claude_token("a")),
            account("b", "b@x.com", &claude_token("b")),
            account("c", "c@x.com", &claude_token("c")),
        ],
    );
    let registry = registry(&file, &server.base_url);
    let (notes, said) = recorder();

    let reply = ask(&registry, &notes, &claude(&server.base_url), StreamOptions::default()).await;
    assert_eq!(reply, Ok("hi".into()));
    // Straight on to the second, not the third.
    assert_eq!(server.tokens("/v1/messages"), [claude_token("a"), claude_token("b")]);
    assert_eq!(*said.lock(), [AccountNote::Switched("Claude account a@x.com reached its limit; continuing on b@x.com.".into())]);
    let accounts = registry.accounts("anthropic").await.expect("accounts");
    assert_eq!(accounts[0].exhausted_until, resets * 1000, "skipped until the reset Anthropic said");
    assert_eq!(exhausted_until(&accounts)[1..], [("b@x.com".into(), false), ("c@x.com".into(), false)]);

    // The next request goes to the second at once, saying nothing.
    let reply = ask(&registry, &notes, &claude(&server.base_url), StreamOptions::default()).await;
    assert_eq!(reply, Ok("hi".into()));
    assert_eq!(server.tokens("/v1/messages"), [claude_token("a"), claude_token("b"), claude_token("b")]);
    assert_eq!(said.lock().len(), 1);
}

#[tokio::test]
async fn a_passing_rate_limit_fails_the_request_as_before_without_changing_accounts() {
    let dir = tempfile::tempdir().expect("dir");
    let file = dir.path().join("auth.json");
    let server = mock(Arc::new(|seen| if seen.token == claude_token("a") { claude_rate_limit() } else { claude_reply() })).await;
    store_accounts(&file, "anthropic", vec![account("a", "a@x.com", &claude_token("a")), account("b", "b@x.com", &claude_token("b"))]);
    let registry = registry(&file, &server.base_url);
    let (notes, said) = recorder();

    let error = ask(&registry, &notes, &claude(&server.base_url), StreamOptions::default()).await.expect_err("a 429");
    assert!(error.starts_with("429 ") && error.contains("This request would exceed your account's rate limit"), "{error}");
    // Still the agent's retry to make: it's no usage limit.
    assert!(factory_agent::guard::recoverable_response(&error), "{error}");
    assert_eq!(server.tokens("/v1/messages"), [claude_token("a")]);
    assert!(said.lock().is_empty());
    let accounts = registry.accounts("anthropic").await.expect("accounts");
    assert_eq!(exhausted_until(&accounts), [("a@x.com".into(), false), ("b@x.com".into(), false)]);
}

#[tokio::test]
async fn with_every_account_at_its_limit_the_request_fails_saying_when_the_first_resets() {
    let dir = tempfile::tempdir().expect("dir");
    let file = dir.path().join("auth.json");
    let now = now_ms() / 1000;
    let server = mock(Arc::new(move |seen| {
        // b resets first.
        if seen.token == claude_token("a") { claude_limit(now + 4 * 3600) } else { claude_limit(now + 2 * 3600) }
    }))
    .await;
    store_accounts(&file, "anthropic", vec![account("a", "a@x.com", &claude_token("a")), account("b", "b@x.com", &claude_token("b"))]);
    let registry = registry(&file, &server.base_url);
    let (notes, said) = recorder();

    let error = ask(&registry, &notes, &claude(&server.base_url), StreamOptions::default()).await.expect_err("all out");
    assert!(error.starts_with("All your Claude accounts reached their usage limit; the first to reset is b@x.com, "), "{error}");
    assert!(error.ends_with("(in 2 h)."), "{error}");
    // Not retried: it's a usage limit.
    assert!(!factory_agent::guard::recoverable_response(&error), "{error}");
    assert_eq!(server.tokens("/v1/messages"), [claude_token("a"), claude_token("b")]);
    assert_eq!(
        *said.lock(),
        [AccountNote::Switched("Claude account a@x.com reached its limit; continuing on b@x.com.".into()), AccountNote::Exhausted]
    );

    // Until one resets, requests fail at once, without reaching Anthropic.
    let again = ask(&registry, &notes, &claude(&server.base_url), StreamOptions::default()).await.expect_err("still out");
    assert!(again.starts_with("All your Claude accounts reached their usage limit"), "{again}");
    assert_eq!(server.tokens("/v1/messages").len(), 2);
}

#[tokio::test]
async fn an_account_has_capacity_again_once_its_limit_resets() {
    let dir = tempfile::tempdir().expect("dir");
    let file = dir.path().join("auth.json");
    let limited = Arc::new(Mutex::new(true));
    // Far enough ahead that it is still to come when the account is marked.
    let resets = now_ms() / 1000 + 3;
    let still = limited.clone();
    let server =
        mock(Arc::new(move |seen| if seen.token == claude_token("a") && *still.lock() { claude_limit(resets) } else { claude_reply() }))
            .await;
    store_accounts(&file, "anthropic", vec![account("a", "a@x.com", &claude_token("a")), account("b", "b@x.com", &claude_token("b"))]);
    let registry = registry(&file, &server.base_url);
    let (notes, said) = recorder();

    assert_eq!(ask(&registry, &notes, &claude(&server.base_url), StreamOptions::default()).await, Ok("hi".into()));
    assert_eq!(server.tokens("/v1/messages"), [claude_token("a"), claude_token("b")]);
    *limited.lock() = false;
    while now_ms() <= resets * 1000 {
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let accounts = registry.accounts("anthropic").await.expect("accounts");
    assert_eq!(accounts[0].exhausted_until, 0, "past its reset, it has capacity");

    // Back to the first, the order's own, with nothing to say.
    assert_eq!(ask(&registry, &notes, &claude(&server.base_url), StreamOptions::default()).await, Ok("hi".into()));
    assert_eq!(server.tokens("/v1/messages"), [claude_token("a"), claude_token("b"), claude_token("a")]);
    assert_eq!(said.lock().len(), 1);
}

#[tokio::test]
async fn without_a_reset_in_the_answer_the_usage_says_when() {
    let dir = tempfile::tempdir().expect("dir");
    let file = dir.path().join("auth.json");
    let weekly = "2031-01-07T10:00:00Z";
    let server = mock(Arc::new(move |seen| {
        if seen.path.starts_with("/api/oauth/usage") {
            return json_answer(
                200,
                Vec::new(),
                json!({"five_hour": {"utilization": 30.0, "resets_at": "2031-01-01T05:00:00Z"}, "seven_day": {"utilization": 100.0, "resets_at": weekly}}),
            );
        }
        if seen.token == claude_token("a") {
            json_answer(
                429,
                vec![("anthropic-ratelimit-unified-status".into(), "rejected".into())],
                json!({"type": "error", "error": {"type": "rate_limit_error", "message": "Error"}}),
            )
        } else {
            claude_reply()
        }
    }))
    .await;
    store_accounts(&file, "anthropic", vec![account("a", "a@x.com", &claude_token("a")), account("b", "b@x.com", &claude_token("b"))]);
    let registry = registry(&file, &server.base_url);
    let (notes, _) = recorder();

    assert_eq!(ask(&registry, &notes, &claude(&server.base_url), StreamOptions::default()).await, Ok("hi".into()));
    assert_eq!(server.tokens(USAGE), [claude_token("a")]);
    let accounts = registry.accounts("anthropic").await.expect("accounts");
    assert_eq!(accounts[0].exhausted_until, factory_agent::clock::parse_iso_ms(weekly).expect("time"), "the full window's reset");
}

/// Anthropic's usage, read with its resets.
const USAGE: &str = "/api/oauth/usage?cedar_ember=1&skip_spend=1";

/// A Codex access token: unsigned, with the claims Codex reads.
fn codex_token(user: &str) -> String {
    let encode = |value: Value| base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(value.to_string());
    let claims = json!({
        "https://api.openai.com/auth": {"chatgpt_account_id": format!("account-{user}"), "chatgpt_user_id": format!("user-{user}"), "chatgpt_plan_type": "pro"},
        "https://api.openai.com/profile": {"email": format!("{user}@x.com")},
    });
    format!("{}.{}.sig", encode(json!({"alg": "none"})), encode(claims))
}

fn codex_reply() -> Answer {
    let cases: Vec<Value> = serde_json::from_str(include_str!("../fixtures/openai_codex.json")).expect("fixture");
    let case = cases.iter().find(|case| case["name"] == "thinking off sends no reasoning").expect("case");
    let body = case["http"]["chunks"].as_array().expect("chunks").iter().filter_map(Value::as_str).collect();
    Answer { status: 200, headers: Vec::new(), content_type: "text/event-stream", body }
}

#[tokio::test]
async fn a_chatgpt_account_over_its_usage_limit_hands_over_to_the_next() {
    let dir = tempfile::tempdir().expect("dir");
    let file = dir.path().join("auth.json");
    let resets = now_ms() / 1000 + 600;
    let (a, b) = (codex_token("a"), codex_token("b"));
    let limited = a.clone();
    let server = mock(Arc::new(move |seen| {
        if seen.token == limited {
            json_answer(
                429,
                Vec::new(),
                json!({"error": {"type": "usage_limit_reached", "message": "The usage limit has been reached", "plan_type": "pro", "resets_at": resets}}),
            )
        } else {
            codex_reply()
        }
    }))
    .await;
    store_accounts(&file, "openai-codex", vec![account("a", "a@x.com", &a), account("b", "b@x.com", &b)]);
    let registry = registry(&file, &server.base_url);
    let (notes, said) = recorder();
    let luna = model("openai-codex", "gpt-5.6-luna", &format!("{}/backend-api", server.base_url));
    let options = StreamOptions { transport: Transport::Sse, ..StreamOptions::default() };

    let reply = ask(&registry, &notes, &luna, options).await;
    assert!(reply.is_ok(), "{reply:?}");
    assert_eq!(server.tokens("/codex/responses"), [a, b]);
    assert_eq!(*said.lock(), [AccountNote::Switched("ChatGPT account a@x.com reached its limit; continuing on b@x.com.".into())]);
    assert_eq!(registry.accounts("openai-codex").await.expect("accounts")[0].exhausted_until, resets * 1000);
}

#[tokio::test]
async fn one_account_over_its_limit_fails_as_a_usage_limit_without_a_note() {
    let dir = tempfile::tempdir().expect("dir");
    let file = dir.path().join("auth.json");
    let resets = now_ms() / 1000 + 90 * 60;
    let server = mock(Arc::new(move |_| claude_limit(resets))).await;
    store_accounts(&file, "anthropic", vec![account("a", "a@x.com", &claude_token("a"))]);
    let registry = registry(&file, &server.base_url);
    let (notes, said) = recorder();

    let error = ask(&registry, &notes, &claude(&server.base_url), StreamOptions::default()).await.expect_err("out");
    assert!(error.starts_with("Your Claude account reached its usage limit; it resets "), "{error}");
    assert!(error.ends_with("(in 1 h 30 min)."), "{error}");
    assert_eq!(*said.lock(), [AccountNote::Exhausted]);
}

#[tokio::test]
async fn a_credential_stored_before_accounts_is_the_first_and_stays_readable_the_old_way() {
    let dir = tempfile::tempdir().expect("dir");
    let file = dir.path().join("auth.json");
    let old = json!({"type": "oauth", "access": claude_token("old"), "refresh": "r-old", "expires": now_ms() + 3_600_000, "kept": true});
    std::fs::write(&file, json!({"anthropic": old, "openai": {"type": "api_key", "key": "sk-1234abcd"}}).to_string()).expect("file");
    let server = mock(Arc::new(|seen| {
        if seen.path.ends_with("/api/oauth/profile") {
            let who = seen.token.trim_start_matches("sk-ant-oat01-");
            return json_answer(200, Vec::new(), json!({"account": {"uuid": format!("uuid-{who}"), "email": format!("{who}@x.com")}}));
        }
        claude_reply()
    }))
    .await;
    // Anthropic's sign-in, without the browser.
    let mut providers = default_providers();
    let base: &'static str = Box::leak(server.base_url.clone().into_boxed_str());
    for provider in &mut providers {
        provider.base_url = base;
        if provider.id == "anthropic" {
            provider.oauth = Some(Arc::new(Pasted));
        }
    }
    let registry = Registry::with_providers(FileCredentialStore::new(file.clone()), no_env(), providers);

    // It's the provider's one account, "1", unnamed until read.
    let accounts = registry.accounts("anthropic").await.expect("accounts");
    assert_eq!(accounts.len(), 1);
    assert_eq!((accounts[0].id.as_str(), accounts[0].label.as_str(), accounts[0].kind), ("1", "", "oauth"));
    assert_eq!(registry.accounts("openai").await.expect("accounts")[0].kind, "api_key");
    // Reading its plan names it.
    registry.account_plan("anthropic", None).await.expect("plan");
    assert_eq!(registry.accounts("anthropic").await.expect("accounts")[0].label, "old@x.com");

    // Another account joins it, after it.
    let login = Login::new(&claude_token("new"));
    registry.login_account("anthropic", "oauth", &login, true).await.expect("sign-in");
    let accounts = registry.accounts("anthropic").await.expect("accounts");
    let labels: Vec<_> = accounts.iter().map(|account| (account.id.as_str(), account.label.as_str())).collect();
    assert_eq!(labels[0], ("1", "old@x.com"));
    assert_eq!(labels[1].1, "new@x.com");

    // The file still has the first where the one credential was, with what
    // Factory doesn't model, so a Factory from before reads it.
    let stored: Value = serde_json::from_str(&std::fs::read_to_string(&file).expect("file")).expect("json");
    let first: Credential = serde_json::from_value(stored["anthropic"].clone()).expect("the old shape");
    let Credential::Oauth { access, refresh, extra, .. } = first else { panic!("oauth") };
    assert_eq!((access.as_str(), refresh.as_str(), &extra["kept"]), (claude_token("old").as_str(), "r-old", &json!(true)));
    assert_eq!(stored["anthropic"]["factoryAccounts"][0]["access"], json!(claude_token("new")));
    assert_eq!(stored["openai"], json!({"type": "api_key", "key": "sk-1234abcd"}), "other providers untouched");
}

/// Signs in with an OAuth token of the test's choosing.
struct Login {
    access: String,
    cancel: CancellationToken,
}

impl Login {
    fn new(access: &str) -> Login {
        Login { access: access.to_owned(), cancel: CancellationToken::new() }
    }
}

impl AuthInteraction for Login {
    fn prompt<'a>(&'a self, _prompt: AuthPrompt) -> AuthFuture<'a, Result<String, Error>> {
        let access = self.access.clone();
        Box::pin(async move { Ok(access) })
    }

    fn notify(&self, _event: AuthEvent) {}

    fn cancel(&self) -> CancellationToken {
        self.cancel.clone()
    }
}

/// A subscription whose sign-in asks for the token itself.
struct Pasted;

impl OAuthFlow for Pasted {
    fn name(&self) -> &str {
        "Pasted (subscription)"
    }

    fn is_subscription(&self) -> bool {
        true
    }

    fn login<'a>(&'a self, interaction: &'a dyn AuthInteraction) -> AuthFuture<'a, Result<Credential, Error>> {
        Box::pin(async move {
            let access = interaction.prompt(AuthPrompt::Text { message: "token".into(), placeholder: None }).await?;
            Ok(Credential::Oauth {
                access,
                refresh: "r".into(),
                expires: now_ms() + 3_600_000,
                account_id: None,
                extra: Default::default(),
            })
        })
    }

    fn refresh<'a>(&'a self, credential: Credential, _cancel: CancellationToken) -> AuthFuture<'a, Result<Credential, Error>> {
        Box::pin(async move { Ok(credential) })
    }

    fn to_auth(&self, credential: &Credential) -> Result<RequestAuth, Error> {
        let Credential::Oauth { access, .. } = credential else { return Err(Error::Other("not oauth".into())) };
        Ok(RequestAuth { api_key: Some(access.clone()), ..RequestAuth::default() })
    }
}

/// Who a token is: the part before its first dash ("ann-2" is ann).
fn who(_base_url: String, auth: RequestAuth) -> futures_util::future::BoxFuture<'static, Result<Identity, Error>> {
    let token = auth.api_key.unwrap_or_default();
    let person = token.split('-').next().unwrap_or_default().to_owned();
    Box::pin(async move { Ok(Identity { key: person.clone(), label: format!("{person}@x.com") }) })
}

fn pasted_registry(file: &std::path::Path) -> Registry {
    let mut model = crate::support::luna();
    model.provider = "pasted".into();
    let provider = Provider {
        id: "pasted",
        name: "Pasted",
        base_url: "http://127.0.0.1:9",
        api_key: None,
        oauth: Some(Arc::new(Pasted)),
        models: vec![model],
        api: None,
        limits: None,
        plan: None,
        identity: Some(who),
        reset: None,
        check_key: None,
    };
    Registry::with_providers(FileCredentialStore::new(file.to_path_buf()), no_env(), vec![provider])
}

async fn labels(registry: &Registry) -> Vec<String> {
    registry.accounts("pasted").await.expect("accounts").into_iter().map(|account| account.label).collect()
}

#[tokio::test]
async fn accounts_are_added_signed_in_again_reordered_and_signed_out_of() {
    let dir = tempfile::tempdir().expect("dir");
    let registry = pasted_registry(&dir.path().join("auth.json"));
    registry.login_account("pasted", "oauth", &Login::new("ann-1"), false).await.expect("ann");
    registry.login_account("pasted", "oauth", &Login::new("bob-1"), true).await.expect("bob");
    registry.login_account("pasted", "oauth", &Login::new("cat-1"), true).await.expect("cat");
    assert_eq!(labels(&registry).await, ["ann@x.com", "bob@x.com", "cat@x.com"]);
    let ids: Vec<String> = registry.accounts("pasted").await.expect("accounts").into_iter().map(|account| account.id).collect();

    // Signing in to one already there renews it in place, keeping its id.
    registry.login_account("pasted", "oauth", &Login::new("bob-2"), true).await.expect("bob again");
    let accounts = registry.accounts("pasted").await.expect("accounts");
    assert_eq!(accounts.iter().map(|account| account.id.clone()).collect::<Vec<_>>(), ids);
    // Without adding, a new one takes the first's place.
    registry.login_account("pasted", "oauth", &Login::new("dan-1"), false).await.expect("dan");
    assert_eq!(labels(&registry).await, ["dan@x.com", "bob@x.com", "cat@x.com"]);

    registry.move_account("pasted", &ids[2], 0).await.expect("cat first");
    assert_eq!(labels(&registry).await, ["cat@x.com", "dan@x.com", "bob@x.com"]);
    registry.move_account("pasted", &ids[2], 99).await.expect("cat last");
    assert_eq!(labels(&registry).await, ["dan@x.com", "bob@x.com", "cat@x.com"]);
    assert!(registry.move_account("pasted", "nope", 0).await.is_err());

    registry.logout_account("pasted", &ids[1]).await.expect("bob out");
    assert_eq!(labels(&registry).await, ["dan@x.com", "cat@x.com"]);
    registry.logout_account("pasted", "").await.expect("all out");
    assert!(labels(&registry).await.is_empty());
}

/// Anthropic's answer to a key without credit.
fn no_credit() -> Answer {
    json_answer(
        400,
        Vec::new(),
        json!({"type": "error", "error": {"type": "invalid_request_error", "message": "Your credit balance is too low to access the Anthropic API. Please go to Plans & Billing to upgrade or purchase credits."}}),
    )
}

fn key(key: &str) -> Value {
    json!({"type": "api_key", "key": key})
}

fn keyed(id: &str, secret: &str) -> Value {
    let mut value = key(secret);
    value["factoryAccount"] = json!(id);
    value
}

const KEY_1: &str = "sk-ant-api03-first-1111";
const KEY_2: &str = "sk-ant-api03-second-2222";

#[tokio::test]
async fn a_subscription_at_its_limit_carries_on_with_an_api_key() {
    let dir = tempfile::tempdir().expect("dir");
    let file = dir.path().join("auth.json");
    let resets = now_ms() / 1000 + 3600;
    let server = mock(Arc::new(move |seen| if seen.token == claude_token("a") { claude_limit(resets) } else { claude_reply() })).await;
    store_accounts(&file, "anthropic", vec![account("a", "a@x.com", &claude_token("a")), keyed("k", KEY_1)]);
    let registry = registry(&file, &server.base_url);
    let (notes, said) = recorder();

    assert_eq!(ask(&registry, &notes, &claude(&server.base_url), StreamOptions::default()).await, Ok("hi".into()));
    assert_eq!(server.tokens("/v1/messages"), [claude_token("a"), KEY_1.to_owned()]);
    assert_eq!(*said.lock(), [AccountNote::Switched("Claude account a@x.com reached its limit; continuing on sk-…1111.".into())]);
    let accounts = registry.accounts("anthropic").await.expect("accounts");
    let shown: Vec<_> = accounts.iter().map(|account| (account.label.as_str(), account.kind, account.in_use)).collect();
    assert_eq!(shown, [("a@x.com", "oauth", false), ("sk-…1111", "api_key", true)]);
}

#[tokio::test]
async fn a_key_out_of_credit_carries_on_with_the_next_and_is_tried_again_when_asked() {
    let dir = tempfile::tempdir().expect("dir");
    let file = dir.path().join("auth.json");
    let broke = Arc::new(Mutex::new(true));
    let still = broke.clone();
    let server = mock(Arc::new(move |seen| if seen.token == KEY_1 && *still.lock() { no_credit() } else { claude_reply() })).await;
    store_accounts(&file, "anthropic", vec![keyed("k1", KEY_1), keyed("k2", KEY_2)]);
    let registry = registry(&file, &server.base_url);
    registry.update_account("anthropic", "k1", Some("Credits"), false).await.expect("named");
    let (notes, said) = recorder();

    assert_eq!(ask(&registry, &notes, &claude(&server.base_url), StreamOptions::default()).await, Ok("hi".into()));
    assert_eq!(server.tokens("/v1/messages"), [KEY_1, KEY_2]);
    assert_eq!(*said.lock(), [AccountNote::Switched("Claude account Credits ran out of credit; continuing on sk-…2222.".into())]);
    let accounts = registry.accounts("anthropic").await.expect("accounts");
    let shown: Vec<_> = accounts.iter().map(|account| (account.out_of_credit, account.exhausted_until, account.in_use)).collect();
    assert_eq!(shown, [(true, 0, false), (false, 0, true)], "out of credit, with nothing to say when it resets");

    // Skipped from then on, until it's tried again.
    assert_eq!(ask(&registry, &notes, &claude(&server.base_url), StreamOptions::default()).await, Ok("hi".into()));
    assert_eq!(server.tokens("/v1/messages"), [KEY_1, KEY_2, KEY_2]);
    *broke.lock() = false;
    registry.update_account("anthropic", "k1", None, true).await.expect("retry");
    assert_eq!(ask(&registry, &notes, &claude(&server.base_url), StreamOptions::default()).await, Ok("hi".into()));
    assert_eq!(server.tokens("/v1/messages"), [KEY_1, KEY_2, KEY_2, KEY_1]);
    assert!(!registry.accounts("anthropic").await.expect("accounts")[0].out_of_credit);
}

#[tokio::test]
async fn a_key_out_of_credit_that_answers_when_tried_again_has_credit() {
    let dir = tempfile::tempdir().expect("dir");
    let file = dir.path().join("auth.json");
    let server = mock(Arc::new(|_| claude_reply())).await;
    // Out of credit for over an hour: tried again.
    let mut tired = keyed("k1", KEY_1);
    tired["factoryOutOfCredit"] = json!(now_ms() - factory_agent::providers::models::CREDIT_RETRY_MS - 1000);
    store_accounts(&file, "anthropic", vec![tired, keyed("k2", KEY_2)]);
    let registry = registry(&file, &server.base_url);
    let (notes, said) = recorder();
    assert!(registry.accounts("anthropic").await.expect("accounts")[0].in_use);

    assert_eq!(ask(&registry, &notes, &claude(&server.base_url), StreamOptions::default()).await, Ok("hi".into()));
    assert_eq!(server.tokens("/v1/messages"), [KEY_1]);
    assert!(!registry.accounts("anthropic").await.expect("accounts")[0].out_of_credit);
    assert_eq!(*said.lock(), [AccountNote::Changed]);
}

#[tokio::test]
async fn every_key_out_of_credit_fails_saying_so_without_retrying() {
    let dir = tempfile::tempdir().expect("dir");
    let file = dir.path().join("auth.json");
    let server = mock(Arc::new(|_| no_credit())).await;
    store_accounts(&file, "anthropic", vec![keyed("k1", KEY_1), keyed("k2", KEY_2)]);
    let registry = registry(&file, &server.base_url);
    let (notes, _) = recorder();

    let error = ask(&registry, &notes, &claude(&server.base_url), StreamOptions::default()).await.expect_err("broke");
    assert_eq!(error, "All your Claude accounts ran out of credit.");
    assert!(!factory_agent::guard::recoverable_response(&error));
}

#[test]
fn credit_errors_are_told_apart_from_rate_limits() {
    use factory_agent::accounts::credit_error;
    let openai = json!({"error": {"message": "You exceeded your current quota, please check your plan and billing details.", "type": "insufficient_quota", "code": "insufficient_quota"}});
    assert!(credit_error(429, &openai.to_string()));
    let rate = json!({"error": {"message": "Rate limit reached for requests", "type": "requests", "code": "rate_limit_exceeded"}});
    assert!(!credit_error(429, &rate.to_string()));
    let openrouter = json!({"error": {"code": 402, "message": "Insufficient credits. Add more using https://openrouter.ai/credits"}});
    assert!(credit_error(402, &openrouter.to_string()));
    assert!(!credit_error(529, &json!({"type": "error", "error": {"type": "overloaded_error", "message": "Overloaded"}}).to_string()));
}

#[tokio::test]
async fn keys_are_added_beside_the_others_and_the_environments_comes_last() {
    let dir = tempfile::tempdir().expect("dir");
    let file = dir.path().join("auth.json");
    store_accounts(&file, "anthropic", vec![account("a", "a@x.com", &claude_token("a"))]);
    let env: Env = Arc::new(|name| (name == "ANTHROPIC_API_KEY").then(|| "sk-ant-api03-from-env-9999".to_owned()));
    // Anthropic, which accepts every key when asked.
    let server = mock(Arc::new(|_| json_answer(200, Vec::new(), json!({"data": []})))).await;
    let registry = Registry::with_providers(FileCredentialStore::new(file.clone()), env, on(&server.base_url));

    registry.login_account("anthropic", "api_key", &Login::new(KEY_1), true).await.expect("first key");
    registry.login_account("anthropic", "api_key", &Login::new(KEY_2), true).await.expect("second key");
    // The same key again is the same account.
    registry.login_account("anthropic", "api_key", &Login::new(KEY_1), true).await.expect("first key again");
    let accounts = registry.accounts("anthropic").await.expect("accounts");
    let shown: Vec<_> = accounts.iter().map(|account| (account.label.as_str(), account.kind, account.stored)).collect();
    assert_eq!(
        shown,
        [("a@x.com", "oauth", true), ("sk-…1111", "api_key", true), ("sk-…2222", "api_key", true), ("ANTHROPIC_API_KEY", "api_key", false)]
    );
    assert_eq!(accounts[3].id, "env");
    assert!(registry.move_account("anthropic", "env", 0).await.is_err(), "it stays last");
    assert!(registry.update_account("anthropic", "env", Some("Mine"), false).await.is_err(), "it's named by its variable");
    registry.move_account("anthropic", &accounts[2].id, 0).await.expect("second key first");
    let labels: Vec<String> = registry.accounts("anthropic").await.expect("accounts").into_iter().map(|account| account.label).collect();
    assert_eq!(labels, ["sk-…2222", "a@x.com", "sk-…1111", "ANTHROPIC_API_KEY"]);
}

/// An upgraded ChatGPT plan shows at the next read, as Codex says it now,
/// though the token from the sign-in still claims the old one; the claim
/// stands in only while Codex can't say.
#[tokio::test]
async fn a_chatgpt_plan_is_read_as_codex_says_it_now() {
    let dir = tempfile::tempdir().expect("dir");
    let file = dir.path().join("auth.json");
    let answering = Arc::new(std::sync::atomic::AtomicBool::new(true));
    let up = answering.clone();
    let server = mock(Arc::new(move |seen| {
        assert!(seen.path.ends_with("/api/codex/usage"), "{}", seen.path);
        if up.load(std::sync::atomic::Ordering::SeqCst) {
            json_answer(200, Vec::new(), json!({"plan_type": "prolite", "rate_limit": null}))
        } else {
            json_answer(503, Vec::new(), json!({"error": "unavailable"}))
        }
    }))
    .await;
    store_accounts(&file, "openai-codex", vec![account("a", "a@x.com", &codex_token("a"))]);
    let registry = registry(&file, &server.base_url);
    assert_eq!(registry.account_plan("openai-codex", Some("a")).await.expect("plan").as_deref(), Some("ChatGPT Prolite"));
    answering.store(false, std::sync::atomic::Ordering::SeqCst);
    assert_eq!(registry.account_plan("openai-codex", Some("a")).await.expect("plan").as_deref(), Some("ChatGPT Pro"), "the token's claim");
}

fn reset(id: &str, label: &str, left: u32) -> UsageReset {
    UsageReset { id: id.into(), label: label.into(), left, usable: true, ..UsageReset::default() }
}

/// The JSON body of the requests to `path`.
fn bodies(server: &Mock, path: &str) -> Vec<Value> {
    server.seen.lock().iter().filter(|seen| seen.path == path).map(|seen| serde_json::from_str(&seen.body).expect("a JSON body")).collect()
}

/// Using a Claude reset claims it as Claude Code does, for the account's
/// organization, and the account at its limit gets requests again at once.
#[tokio::test]
async fn a_claude_reset_is_claimed_for_its_organization_and_the_account_gets_requests_again() {
    let dir = tempfile::tempdir().expect("dir");
    let file = dir.path().join("auth.json");
    let resets = now_ms() / 1000 + 3600;
    let server = mock(Arc::new(move |seen| match seen.path.as_str() {
        "/api/oauth/profile" => json_answer(200, Vec::new(), json!({"organization": {"uuid": "org-1"}})),
        "/api/organizations/org-1/reset_rate_limits" => {
            json_answer(200, Vec::new(), json!({"result": "reset", "resets_left": 1, "cleared": ["five_hour"]}))
        }
        _ if seen.token == claude_token("a") => claude_limit(resets),
        _ => claude_reply(),
    }))
    .await;
    store_accounts(&file, "anthropic", vec![account("a", "a@x.com", &claude_token("a")), account("b", "b@x.com", &claude_token("b"))]);
    let registry = registry(&file, &server.base_url);
    let (notes, _) = recorder();
    assert_eq!(ask(&registry, &notes, &claude(&server.base_url), StreamOptions::default()).await, Ok("hi".into()));
    assert!(registry.accounts("anthropic").await.expect("accounts")[0].exhausted_until > 0, "a is at its limit");

    let answer =
        registry.use_reset("anthropic", Some("a"), reset("grant_1", "5-hour reset", 2)).await.expect("used").expect("a subscription");
    assert_eq!(
        answer,
        ResetAnswer { reset: true, message: "Your 5-hour limit is reset. 1 five-hour reset left.".into(), fallback_url: String::new() }
    );
    let claims = bodies(&server, "/api/organizations/org-1/reset_rate_limits");
    assert_eq!(claims.len(), 1);
    assert_eq!((claims[0]["program"].as_str(), claims[0]["grant_id"].as_str()), (Some("cedar_ember"), Some("grant_1")));
    let request_id = claims[0]["request_id"].as_str().expect("a request id");
    assert!(request_id.len() == 32 && request_id.bytes().all(|byte| byte.is_ascii_hexdigit()), "{request_id}");
    assert_eq!(server.tokens("/reset_rate_limits"), [claude_token("a")]);
    assert_eq!(registry.accounts("anthropic").await.expect("accounts")[0].exhausted_until, 0, "requests go to it again");
}

/// A claim Anthropic refuses, or one it answers without a reset, uses
/// nothing and leaves the account as it was; a refusal of Claktory points
/// to claude.ai.
#[tokio::test]
async fn a_claude_reset_anthropic_refuses_uses_nothing_and_points_to_claude_ai() {
    let dir = tempfile::tempdir().expect("dir");
    let file = dir.path().join("auth.json");
    let answers = Arc::new(Mutex::new(vec![
        json!({"result": "not_limited", "reason": "not_limited"}),
        json!({"result": "ineligible", "reason": "surface"}),
    ]));
    let next = answers.clone();
    let server = mock(Arc::new(move |seen| match seen.path.as_str() {
        "/api/oauth/profile" => json_answer(200, Vec::new(), json!({"organization": {"uuid": "org-1"}})),
        _ => match next.lock().pop() {
            Some(answer) => json_answer(200, Vec::new(), answer),
            None => json_answer(403, Vec::new(), json!({"error": {"message": "Forbidden"}})),
        },
    }))
    .await;
    store_accounts(&file, "anthropic", vec![account("a", "a@x.com", &claude_token("a"))]);
    let registry = registry(&file, &server.base_url);
    let use_one = || registry.use_reset("anthropic", Some("a"), reset("grant_1", "Fable reset", 1));

    let ineligible = use_one().await.expect("used").expect("a subscription");
    assert!(!ineligible.reset);
    assert_eq!(ineligible.fallback_url, "https://clau.de/reset");
    let not_limited = use_one().await.expect("used").expect("a subscription");
    assert_eq!(
        not_limited,
        ResetAnswer {
            reset: false,
            message: "You're not at a limit this reset refills, so nothing was used.".into(),
            fallback_url: String::new()
        }
    );
    let forbidden = use_one().await.expect("used").expect("a subscription");
    assert!(!forbidden.reset && forbidden.message.contains("Nothing was used"), "{}", forbidden.message);
    assert_eq!(forbidden.fallback_url, "https://clau.de/reset");
    assert_eq!(server.tokens("/reset_rate_limits").len(), 3, "each sent once");
}

/// ChatGPT's resets are counted with its usage and told apart by their
/// list; using one names it and gives ChatGPT's answer.
#[tokio::test]
async fn chatgpt_resets_are_read_with_the_usage_and_used_by_id() {
    let dir = tempfile::tempdir().expect("dir");
    let file = dir.path().join("auth.json");
    let server = mock(Arc::new(|seen| match seen.path.as_str() {
        "/api/codex/usage" => json_answer(
            200,
            Vec::new(),
            json!({
                "plan_type": "pro",
                "rate_limit": {"primary_window": {"used_percent": 95.0, "limit_window_seconds": 18_000, "reset_at": 1_900_000_000}},
                "rate_limit_reset_credits": {"available_count": 2}
            }),
        ),
        "/api/codex/rate-limit-reset-credits" => json_answer(
            200,
            Vec::new(),
            json!({"available_count": 2, "credits": [
                {"id": "later", "reset_type": "codex_rate_limits", "status": "available", "granted_at": "2030-01-01T00:00:00Z", "expires_at": "2031-02-01T00:00:00Z", "title": null, "description": null},
                {"id": "used", "reset_type": "codex_rate_limits", "status": "redeemed", "granted_at": "2030-01-01T00:00:00Z", "expires_at": "2030-02-01T00:00:00Z"},
                {"id": "sooner", "reset_type": "codex_rate_limits", "status": "available", "granted_at": "2030-01-01T00:00:00Z", "expires_at": "2031-01-01T00:00:00Z"}
            ]}),
        ),
        "/api/codex/rate-limit-reset-credits/consume" => json_answer(200, Vec::new(), json!({"code": "nothing_to_reset", "windows_reset": 0})),
        _ => json_answer(404, Vec::new(), json!({})),
    }))
    .await;
    store_accounts(&file, "openai-codex", vec![account("a", "a@x.com", &codex_token("a"))]);
    let registry = registry(&file, &server.base_url);

    let usage = registry.account_usage("openai-codex", Some("a")).await.expect("read").expect("a subscription");
    assert_eq!(usage.resets.len(), 1, "{:?}", usage.resets);
    let full = &usage.resets[0];
    assert_eq!((full.id.as_str(), full.label.as_str(), full.left), ("sooner", "Full reset", 2));
    assert_eq!(full.expires_at, factory_agent::clock::parse_iso_ms("2031-01-01T00:00:00Z").expect("time"));
    assert_eq!(full.window_ids, ["five_hour"]);

    let answer = registry.use_reset("openai-codex", Some("a"), full.clone()).await.expect("used").expect("a subscription");
    assert!(!answer.reset && answer.message.contains("Nothing was used"), "{}", answer.message);
    let consumed = bodies(&server, "/api/codex/rate-limit-reset-credits/consume");
    assert_eq!(consumed.len(), 1);
    assert_eq!(consumed[0]["credit_id"], "sooner");
    assert!(consumed[0]["redeem_request_id"].as_str().is_some_and(|id| !id.is_empty()));
}

/// What this test binary has logged: tests run in parallel, on threads of
/// their own, so one subscriber takes it all and each test looks for its own
/// accounts' lines.
fn logged() -> String {
    static LOGGED: std::sync::OnceLock<Arc<Mutex<Vec<u8>>>> = std::sync::OnceLock::new();
    let buffer = LOGGED.get_or_init(|| {
        struct Writer(Arc<Mutex<Vec<u8>>>);
        impl std::io::Write for Writer {
            fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
                self.0.lock().extend_from_slice(bytes);
                Ok(bytes.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let buffer = Arc::new(Mutex::new(Vec::new()));
        let shared = buffer.clone();
        let subscriber = tracing_subscriber::fmt().with_writer(move || Writer(shared.clone())).with_ansi(false).finish();
        tracing::subscriber::set_global_default(subscriber).expect("the only subscriber");
        buffer
    });
    String::from_utf8_lossy(&buffer.lock()).into_owned()
}

/// #183: moving to another account was only a notice in one agent's
/// conversation, so nothing watching the log ever saw it.
#[tokio::test]
async fn reaching_a_limit_and_moving_to_the_next_account_are_logged() {
    logged();
    let dir = tempfile::tempdir().expect("dir");
    let file = dir.path().join("auth.json");
    let resets = now_ms() / 1000 + 2 * 3600;
    let server = mock(Arc::new(move |seen| match seen.token.as_str() {
        token if token == claude_token("first-183") => claude_limit(resets),
        _ => claude_reply(),
    }))
    .await;
    store_accounts(
        &file,
        "anthropic",
        vec![account("first-183", "a@x.com", &claude_token("first-183")), account("second-183", "b@x.com", &claude_token("second-183"))],
    );
    let registry = registry(&file, &server.base_url);
    let (notes, _) = recorder();
    let reply = ask(&registry, &notes, &claude(&server.base_url), StreamOptions::default()).await;
    assert_eq!(reply, Ok("hi".into()));
    let resets = factory_agent::clock::iso_from_ms(resets * 1000);
    let logged = logged();
    assert!(logged.contains(&format!("account reached its usage limit provider=anthropic account=first-183 resets={resets}")), "{logged}");
    assert!(
        logged.contains(&format!("moved from account first-183 to second-183 (usage limit, resets {resets}) provider=anthropic")),
        "{logged}"
    );
}

/// The real providers, on `base_url`.
fn on(base_url: &str) -> Vec<Provider> {
    let base: &'static str = Box::leak(base_url.to_owned().into_boxed_str());
    let mut providers = default_providers();
    for provider in &mut providers {
        provider.base_url = base;
    }
    providers
}

/// Anthropic's answer to a key it doesn't know.
fn invalid_key() -> Answer {
    json_answer(
        401,
        Vec::new(),
        json!({"type": "error", "error": {"type": "authentication_error", "message": "invalid x-api-key"}, "request_id": "req_1"}),
    )
}

/// Onboarding rehearsal 2: a dummy key read as "Connected". A new key is
/// checked with one call that costs nothing before it's kept; one the
/// provider refuses isn't kept, and says so. One that can't be checked (the
/// provider is down) is kept.
#[tokio::test]
async fn a_new_key_is_checked_and_one_the_provider_refuses_is_not_kept() {
    let dir = tempfile::tempdir().expect("dir");
    let file = dir.path().join("auth.json");
    let server = mock(Arc::new(|seen| match seen.token.as_str() {
        KEY_1 => json_answer(200, Vec::new(), json!({"data": [{"id": "claude"}]})),
        KEY_2 => json_answer(503, Vec::new(), json!({"type": "error", "error": {"type": "overloaded_error", "message": "Overloaded"}})),
        _ => invalid_key(),
    }))
    .await;
    let registry = Registry::with_providers(FileCredentialStore::new(file), no_env(), on(&server.base_url));

    let refused = registry.login_account("anthropic", "api_key", &Login::new("sk-ant-dummy"), true).await.expect_err("refused");
    assert_eq!(refused.to_string(), "Anthropic didn't accept this key.");
    assert!(refused.is_refused());
    assert!(registry.accounts("anthropic").await.expect("accounts").is_empty(), "not kept");

    registry.login_account("anthropic", "api_key", &Login::new(KEY_1), true).await.expect("accepted");
    registry.login_account("anthropic", "api_key", &Login::new(KEY_2), true).await.expect("unchecked, kept");
    let labels: Vec<String> = registry.accounts("anthropic").await.expect("accounts").into_iter().map(|account| account.label).collect();
    assert_eq!(labels, ["sk-…1111", "sk-…2222"]);
    let asked: Vec<(String, String)> = server.seen.lock().iter().map(|seen| (seen.path.clone(), seen.token.clone())).collect();
    assert_eq!(
        asked,
        [
            ("/v1/models?limit=1".to_owned(), "sk-ant-dummy".to_owned()),
            ("/v1/models?limit=1".to_owned(), KEY_1.to_owned()),
            ("/v1/models?limit=1".to_owned(), KEY_2.to_owned())
        ]
    );
}

/// How a request ended, when it failed: its error's message.
async fn failure(registry: &Registry, model: &Model) -> AssistantMessage {
    let stream = registry.stream_fn_with(None);
    let mut events = stream(model, context(), StreamOptions::default()).expect("stream");
    let mut last = None;
    while let Some(event) = tokio::time::timeout(crate::support::server::HANG, events.next()).await.expect("the stream ends") {
        last = Some(event);
    }
    match last.expect("an event") {
        StreamEvent::Error { error, .. } => error,
        other => panic!("{other:?}"),
    }
}

/// Rivet's first turn on a refused key: the failure says the provider
/// refused the credentials, and whose, so the app can offer to change them.
#[tokio::test]
async fn a_request_the_provider_refuses_the_key_for_says_whose_it_was() {
    let dir = tempfile::tempdir().expect("dir");
    let file = dir.path().join("auth.json");
    let server = mock(Arc::new(|seen| if seen.token == KEY_1 { invalid_key() } else { claude_reply() })).await;
    store_accounts(&file, "anthropic", vec![keyed("k", KEY_1)]);
    let registry = registry(&file, &server.base_url);
    let failed = failure(&registry, &claude(&server.base_url)).await;
    assert_eq!(refused(&failed), Some(Refused { account: "k".into(), method: "api_key".into() }));
    assert!(failed.error_message.as_deref().is_some_and(|text| text.contains("invalid x-api-key")), "{failed:?}");

    // Anything else isn't a refusal: a usage limit, a rate limit, no credit.
    let server = mock(Arc::new(|_| no_credit())).await;
    let failed = failure(&registry, &claude(&server.base_url)).await;
    assert_eq!(refused(&failed), None);
    assert!(refused_error(401, ""));
    assert!(refused_error(403, r#"{"error": {"type": "authentication_error", "message": "OAuth token has been revoked."}}"#));
    assert!(!refused_error(403, r#"{"error": {"type": "permission_error", "message": "Your key can't use this model."}}"#));
    assert!(!refused_error(429, r#"{"error": {"type": "rate_limit_error"}}"#));
}

/// A subscription whose refresh token the provider no longer takes.
struct Revoked;

impl OAuthFlow for Revoked {
    fn name(&self) -> &str {
        "Revoked (subscription)"
    }

    fn is_subscription(&self) -> bool {
        true
    }

    fn login<'a>(&'a self, _interaction: &'a dyn AuthInteraction) -> AuthFuture<'a, Result<Credential, Error>> {
        Box::pin(async { Err(Error::Aborted) })
    }

    fn refresh<'a>(&'a self, _credential: Credential, _cancel: CancellationToken) -> AuthFuture<'a, Result<Credential, Error>> {
        let refused = refresh_refused(400, r#"{"error": "invalid_grant", "error_description": "Refresh token not found or invalid"}"#);
        Box::pin(async move { Err(refusal(refused, "HTTP request failed. status=400".into())) })
    }

    fn to_auth(&self, _credential: &Credential) -> Result<RequestAuth, Error> {
        Ok(RequestAuth::default())
    }
}

/// A sign-in whose token can't be refreshed any more fails the request as
/// refused, naming the account and that it signed in with OAuth.
#[tokio::test]
async fn a_sign_in_the_provider_wont_refresh_is_refused() {
    let dir = tempfile::tempdir().expect("dir");
    let file = dir.path().join("auth.json");
    let mut model = crate::support::luna();
    model.provider = "revoked".into();
    let provider = Provider {
        id: "revoked",
        name: "Revoked",
        base_url: "http://127.0.0.1:9",
        api_key: None,
        oauth: Some(Arc::new(Revoked)),
        models: vec![model.clone()],
        api: None,
        limits: None,
        plan: None,
        identity: None,
        reset: None,
        check_key: None,
    };
    let mut stale = account("s", "s@x.com", "stale");
    stale["expires"] = json!(0);
    store_accounts(&file, "revoked", vec![stale]);
    let registry = Registry::with_providers(FileCredentialStore::new(file), no_env(), vec![provider]);
    let failed = failure(&registry, &model).await;
    assert_eq!(refused(&failed), Some(Refused { account: "s".into(), method: "oauth".into() }));
    assert!(!refresh_refused(500, "invalid_grant") && !refresh_refused(429, ""), "a passing failure isn't a refusal");
}
