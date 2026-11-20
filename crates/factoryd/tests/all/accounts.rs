//! Several accounts per provider, through the app's protocol, against a mock
//! Anthropic: an agent's request over one account's usage limit carries on
//! with the next and says so in its transcript; the providers list every
//! account with its own usage, plan and state; accounts are reordered and
//! signed out of one by one.

use std::sync::Arc;

use crate::common::{Client, View, apply_request, describe, send_message, temp_dir};
use factory_agent::auth::Env;
use factory_agent::clock::now_ms;
use factory_agent::credentials::FileCredentialStore;
use factory_agent::providers::models::{Registry, default_providers};
use factory_protocol::v1::{self, ErrorCode, client_message::Body as Request, server_message::Body};
use factoryd::factory::Factory;
use serde_json::{Value, json};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

/// Anthropic's endpoints: the account's token decides the answer.
async fn anthropic(resets_at_seconds: u64) -> (String, Arc<Mutex<Vec<(String, String)>>>) {
    anthropic_with(resets_at_seconds, Arc::new(AtomicBool::new(true))).await
}

/// `anthropic`, with account "a" over its limit only while `limited`.
async fn anthropic_with(resets_at_seconds: u64, limited: Arc<AtomicBool>) -> (String, Arc<Mutex<Vec<(String, String)>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let base_url = format!("http://{}", listener.local_addr().expect("addr"));
    let seen = Arc::new(Mutex::new(Vec::new()));
    let recorded = seen.clone();
    tokio::spawn(async move {
        loop {
            let Ok((mut socket, _)) = listener.accept().await else { return };
            let (recorded, limited) = (recorded.clone(), limited.load(Ordering::SeqCst));
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
                        .unwrap_or_default()
                };
                let length: usize = header("content-length").parse().unwrap_or(0);
                while raw.len() < head_end + 4 + length {
                    let mut buffer = [0u8; 8192];
                    match socket.read(&mut buffer).await {
                        Ok(0) | Err(_) => break,
                        Ok(read) => raw.extend_from_slice(&buffer[..read]),
                    }
                }
                let path = head.split(' ').nth(1).unwrap_or_default().to_owned();
                let account = header("authorization").trim_start_matches("Bearer sk-ant-oat01-").to_owned();
                let claimed = {
                    let mut recorded = recorded.lock().expect("lock");
                    recorded.push((path.clone(), account.clone()));
                    recorded.iter().any(|(path, _)| path.ends_with("/reset_rate_limits"))
                };
                let (status, extra, content_type, body) = answer(&path, &account, resets_at_seconds, claimed, limited);
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

/// Account "a" is over its limit, with a full reset to use; "b" answers
/// "hi". Each has its own usage and plan, and says who it is. Once the reset
/// is `claimed`, or when it's not `limited`, a has no limit reached (and no
/// reset left once claimed).
fn answer(path: &str, account: &str, resets_at_seconds: u64, claimed: bool, limited: bool) -> (u16, String, &'static str, String) {
    let json = |body: Value| (200, String::new(), "application/json", body.to_string());
    if path == "/api/oauth/usage?cedar_ember=1&skip_spend=1" {
        let used = if account == "b" {
            12.0
        } else if claimed {
            0.0
        } else {
            100.0
        };
        let grants = if account == "a" && !claimed {
            json!([{"id": "full_1", "resets_total": 1, "resets_left": 1, "ends_at": "2031-01-10T22:00:00Z",
                    "clears": ["five_hour", "seven_day"], "type": "full", "paused": false, "usable_now": true}])
        } else {
            json!([])
        };
        return json(json!({
            "five_hour": {"utilization": used, "resets_at": "2031-01-01T05:00:00Z"},
            "cedar_ember": {"eligible": true, "at_limit": account == "a" && !claimed, "grants": grants},
        }));
    }
    if path == "/api/organizations/org-a/reset_rate_limits" {
        return json(json!({"result": "reset", "resets_left": 0, "cleared": ["five_hour", "seven_day"]}));
    }
    if path.ends_with("/api/oauth/profile") {
        let tier = if account == "a" { "default_claude_max_20x" } else { "default_claude_max_5x" };
        return json(json!({
            "account": {"uuid": format!("uuid-{account}"), "email": format!("{account}@x.com")},
            "organization": {"uuid": format!("org-{account}"), "organization_type": "claude_max", "rate_limit_tier": tier},
        }));
    }
    if account == "a" && limited && !claimed {
        let headers = format!("anthropic-ratelimit-unified-status: rejected\r\nanthropic-ratelimit-unified-reset: {resets_at_seconds}\r\n");
        let body = json!({"type": "error", "error": {"type": "rate_limit_error", "message": "Error"}});
        return (429, headers, "application/json", body.to_string());
    }
    let events = [
        json!({"type": "message_start", "message": {"id": "msg_1", "model": "claude", "usage": {"input_tokens": 3, "output_tokens": 1}}}),
        json!({"type": "content_block_start", "index": 0, "content_block": {"type": "text", "text": ""}}),
        json!({"type": "content_block_delta", "index": 0, "delta": {"type": "text_delta", "text": "hi"}}),
        json!({"type": "content_block_stop", "index": 0}),
        json!({"type": "message_delta", "delta": {"stop_reason": "end_turn"}, "usage": {"output_tokens": 1}}),
        json!({"type": "message_stop"}),
    ];
    let body = events.iter().map(|event| format!("event: {}\ndata: {event}\n\n", event["type"].as_str().expect("type"))).collect();
    (200, String::new(), "text/event-stream", body)
}

fn account(id: &str) -> Value {
    json!({"type": "oauth", "access": format!("sk-ant-oat01-{id}"), "refresh": "r", "expires": now_ms() + 3_600_000, "factoryAccount": id})
}

/// The real providers, all on `base_url`, with Anthropic signed in to
/// accounts a and b, in that order; a signed in before accounts had names.
fn registry(dir: &std::path::Path, base_url: &str) -> Registry {
    let file = dir.join("auth.json");
    let mut first = account("a");
    first["factoryAccounts"] = json!([account("b")]);
    std::fs::write(&file, json!({"anthropic": first}).to_string()).expect("credentials");
    let base: &'static str = Box::leak(base_url.to_owned().into_boxed_str());
    let mut providers = default_providers();
    for provider in &mut providers {
        provider.base_url = base;
        for model in &mut provider.models {
            model.base_url = base_url.to_owned();
        }
    }
    let env: Env = Arc::new(|_| None);
    Registry::with_providers(FileCredentialStore::new(file), env, providers)
}

fn claude(view: &View) -> v1::Provider {
    view.workspace.providers.iter().find(|provider| provider.id == "anthropic").cloned().expect("anthropic")
}

fn order(view: &View) -> Vec<String> {
    claude(view).accounts.iter().map(|account| account.id.clone()).collect()
}

#[tokio::test]
async fn an_agent_carries_on_with_the_next_account_and_every_account_shows_its_own_state() {
    let dir = temp_dir();
    let resets = now_ms() / 1000 + 3 * 3600;
    let (base_url, seen) = anthropic(resets).await;
    let factory = Factory::open(crate::common::home(&dir), Arc::new(registry(&dir, &base_url))).await.expect("open");
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("addr");
    crate::common::serves(addr, crate::common::home(&dir));
    tokio::spawn(factoryd::serve(listener, factory.clone()));
    let (mut client, workspace, _) = Client::connect(addr).await;
    let mut view = View { workspace, ..View::default() };

    // Both accounts, the first in use, each read for its own usage, plan and name.
    view.follow(&mut client, |view| {
        claude(view)
            .accounts
            .iter()
            .all(|account| !account.plan.is_empty() && account.usage.as_ref().is_some_and(|usage| usage.updated > 0))
            && claude(view).accounts.iter().all(|account| !account.label.is_empty())
    })
    .await;
    let accounts = claude(&view).accounts;
    let shown: Vec<_> =
        accounts.iter().map(|account| (account.id.as_str(), account.label.as_str(), account.plan.as_str(), account.in_use)).collect();
    assert_eq!(shown, [("a", "a@x.com", "Claude Max 20x", true), ("b", "b@x.com", "Claude Max 5x", false)]);
    let used: Vec<f64> = accounts.iter().map(|account| account.usage.as_ref().expect("usage").windows[0].used_percent).collect();
    assert_eq!(used, [100.0, 12.0]);
    assert!(accounts.iter().all(|account| account.can_sign_out && account.exhausted_until == 0));
    // The provider's own fields are its first account's, for an app from before.
    assert_eq!(claude(&view).plan, "Claude Max 20x");
    assert_eq!(claude(&view).usage.expect("usage").windows[0].used_percent, 100.0);

    let model = factory_agent::providers::catalog::models("anthropic").into_iter().next().expect("a Claude model").id;
    let create = Request::CreateAgent(v1::CreateAgent {
        name: "Builder".into(),
        model: format!("anthropic/{model}"),
        effort: "off".into(),
        ..Default::default()
    });
    assert!(matches!(apply_request(&mut client, &mut view, create).await, Body::Ack(_)));
    apply_request(&mut client, &mut view, send_message("builder", "hello")).await;
    view.follow(&mut client, |view| describe(&view.entries).iter().any(|entry| entry == "assistant: hi")).await;
    assert_eq!(
        describe(&view.entries),
        ["user: hello", "notice: Claude account a@x.com reached its limit; continuing on b@x.com.", "assistant: hi"]
    );
    let requests: Vec<String> =
        seen.lock().expect("lock").iter().filter(|(path, _)| path.ends_with("/v1/messages")).map(|(_, account)| account.clone()).collect();
    assert_eq!(requests, ["a", "b"]);
    // The Usage pane counts the answer as the account's that gave it.
    let load = Request::LoadUsage(v1::LoadUsage::default());
    let Body::UsageReport(report) = apply_request(&mut client, &mut view, load).await else { panic!("expected a usage report") };
    let rows: Vec<_> =
        report.rows.iter().map(|row| (row.agent_id.as_str(), row.account_id.as_str(), row.account_label.as_str(), row.requests)).collect();
    assert_eq!(rows, [("builder", "b", "b@x.com", 1)]);

    // The first is skipped until its reset; requests go to the second.
    view.follow(&mut client, |view| claude(view).accounts[1].in_use).await;
    let accounts = claude(&view).accounts;
    assert_eq!((accounts[0].in_use, accounts[0].exhausted_until), (false, i64::try_from(resets * 1000).expect("ms")));

    // Reordered: b first.
    let move_b = Request::MoveAccount(v1::MoveAccount { provider_id: "anthropic".into(), account_id: "b".into(), position: 0 });
    assert!(matches!(apply_request(&mut client, &mut view, move_b).await, Body::Ack(_)));
    view.follow(&mut client, |view| order(view) == ["b", "a"]).await;
    let unknown = Request::MoveAccount(v1::MoveAccount { provider_id: "anthropic".into(), account_id: "zz".into(), position: 0 });
    match apply_request(&mut client, &mut view, unknown).await {
        Body::Error(error) => assert_eq!(error.code(), ErrorCode::NotFound),
        other => panic!("expected an error, got {other:?}"),
    }

    // Renamed.
    let rename = Request::UpdateAccount(v1::UpdateAccount {
        provider_id: "anthropic".into(),
        account_id: "b".into(),
        label: Some("Team".into()),
        retry: false,
    });
    assert!(matches!(apply_request(&mut client, &mut view, rename).await, Body::Ack(_)));
    view.follow(&mut client, |view| claude(view).accounts[0].label == "Team").await;
    assert_eq!(claude(&view).accounts[0].method_id, "oauth");

    // Signed out of one, the other stays.
    let sign_out = Request::SignOut(v1::SignOut { provider_id: "anthropic".into(), account_id: "a".into() });
    assert!(matches!(apply_request(&mut client, &mut view, sign_out).await, Body::Ack(_)));
    view.follow(&mut client, |view| order(view) == ["b"]).await;
    assert_eq!(claude(&view).signed_in_with, "Anthropic (Claude Pro/Max)");
    assert!(claude(&view).accounts[0].in_use);
    let all = Request::SignOut(v1::SignOut { provider_id: "anthropic".into(), account_id: String::new() });
    assert!(matches!(apply_request(&mut client, &mut view, all).await, Body::Ack(_)));
    view.follow(&mut client, |view| claude(view).accounts.is_empty()).await;
    assert_eq!(claude(&view).signed_in_with, "");
}

/// An account's usage resets show with its usage; the person uses one, gets
/// Anthropic's answer in a sentence, and the account's usage is read again
/// at once. A reset used, or an account that isn't there, is refused.
#[tokio::test]
async fn the_person_uses_a_usage_reset_and_the_usage_is_read_again() {
    let dir = temp_dir();
    let (base_url, seen) = anthropic(now_ms() / 1000 + 3 * 3600).await;
    let factory = Factory::open(crate::common::home(&dir), Arc::new(registry(&dir, &base_url))).await.expect("open");
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("addr");
    crate::common::serves(addr, crate::common::home(&dir));
    tokio::spawn(factoryd::serve(listener, factory.clone()));
    let (mut client, workspace, _) = Client::connect(addr).await;
    let mut view = View { workspace, ..View::default() };

    let usage = |view: &View, account: usize| claude(view).accounts.get(account).and_then(|account| account.usage.clone());
    view.follow(&mut client, |view| usage(view, 0).is_some_and(|usage| !usage.resets.is_empty())).await;
    let resets = usage(&view, 0).expect("usage").resets;
    assert_eq!(
        resets,
        [v1::UsageReset {
            id: "full_1".into(),
            label: "Full reset".into(),
            detail: "Refills all your limits.".into(),
            left: 1,
            total: 1,
            expires_at: i64::try_from(factory_agent::clock::parse_iso_ms("2031-01-10T22:00:00Z").expect("time")).expect("ms"),
            usable: true,
            unavailable: String::new(),
            window_ids: vec!["five_hour".into(), "weekly".into()],
        }]
    );
    view.follow(&mut client, |view| usage(view, 1).is_some()).await;
    assert_eq!(usage(&view, 1).expect("b's usage").resets, [], "b has none");

    let use_it = |account: &str| {
        Request::UseUsageReset(v1::UseUsageReset { provider_id: "anthropic".into(), account_id: account.into(), reset_id: "full_1".into() })
    };
    match apply_request(&mut client, &mut view, use_it("a")).await {
        Body::UsageResetAnswer(answer) => assert_eq!(
            answer,
            v1::UsageResetAnswer {
                reset: true,
                message: "Your limits are reset. No full resets left.".into(),
                fallback_url: String::new()
            }
        ),
        other => panic!("expected an answer, got {other:?}"),
    }
    let claims: Vec<String> = seen
        .lock()
        .expect("lock")
        .iter()
        .filter(|(path, _)| path.ends_with("/reset_rate_limits"))
        .map(|(path, account)| format!("{account} {path}"))
        .collect();
    assert_eq!(claims, ["a /api/organizations/org-a/reset_rate_limits"]);
    view.follow(&mut client, |view| usage(view, 0).is_some_and(|usage| usage.resets.is_empty() && usage.windows[0].used_percent == 0.0))
        .await;

    // Used, it's gone: nothing is sent again.
    for (account, code) in [("a", ErrorCode::NotFound), ("zz", ErrorCode::NotFound)] {
        match apply_request(&mut client, &mut view, use_it(account)).await {
            Body::Error(error) => assert_eq!(error.code(), code, "{account}: {}", error.message),
            other => panic!("expected an error, got {other:?}"),
        }
    }
    assert_eq!(seen.lock().expect("lock").iter().filter(|(path, _)| path.ends_with("/reset_rate_limits")).count(), 1);
}

/// The real providers on `base_url`, with Anthropic signed in to account a
/// alone.
fn only_a(dir: &std::path::Path, base_url: &str) -> Registry {
    let registry = registry(dir, base_url);
    std::fs::write(dir.join("auth.json"), json!({"anthropic": account("a")}).to_string()).expect("credentials");
    registry
}

fn builder(view: &View) -> v1::Agent {
    view.agent("builder").clone()
}

async fn serve(factory: &Arc<Factory>, dir: &std::path::Path) -> (Client, View) {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("addr");
    crate::common::serves(addr, crate::common::home(dir));
    tokio::spawn(factoryd::serve(listener, factory.clone()));
    let (client, workspace, transcript) = Client::connect(addr).await;
    let entries = transcript.map(|transcript| transcript.entries).unwrap_or_default();
    (client, View { workspace, entries, ..View::default() })
}

async fn create_builder(client: &mut Client, view: &mut View) {
    let model = factory_agent::providers::catalog::models("anthropic").into_iter().next().expect("a Claude model").id;
    let create = Request::CreateAgent(v1::CreateAgent {
        name: "Builder".into(),
        model: format!("anthropic/{model}"),
        effort: "off".into(),
        ..Default::default()
    });
    assert!(matches!(apply_request(client, view, create).await, Body::Ack(_)));
    let select = Request::SelectAgent(v1::SelectAgent { agent_id: "builder".into() });
    apply_request(client, view, select).await;
}

fn model_requests(seen: &Mutex<Vec<(String, String)>>) -> usize {
    seen.lock().expect("lock").iter().filter(|(path, _)| path.ends_with("/v1/messages")).count()
}

/// With its only account at its usage limit, an agent doesn't fail: it
/// waits, says so in its conversation and its status, sleeps, takes messages
/// meanwhile, and carries on by itself once the limit resets, with them.
#[tokio::test]
async fn an_agent_waits_out_its_usage_limit_and_carries_on_by_itself() {
    let dir = temp_dir();
    // Far enough ahead that a busy machine still asks before it.
    let resets = now_ms() / 1000 + 5;
    let limited = Arc::new(AtomicBool::new(true));
    let (base_url, seen) = anthropic_with(resets, limited.clone()).await;
    let factory = Factory::open(crate::common::home(&dir), Arc::new(only_a(&dir, &base_url))).await.expect("open");
    factory.set_usage_wait_margin(Duration::ZERO);
    let (mut client, mut view) = serve(&factory, &dir).await;
    create_builder(&mut client, &mut view).await;

    apply_request(&mut client, &mut view, send_message("builder", "hello")).await;
    view.follow(&mut client, |view| !builder(view).usage_wait.is_empty()).await;
    let agent = builder(&view);
    assert!(agent.usage_wait.starts_with("Waiting for your Claude usage to reset at "), "{}", agent.usage_wait);
    assert_eq!(agent.usage_wait_until_ms, i64::try_from(resets * 1000).expect("ms"));
    assert_eq!((agent.status(), agent.character_mood()), (v1::AgentStatus::Working, v1::CharacterMood::Asleep));
    view.follow(&mut client, |view| describe(&view.entries).len() == 2).await;
    let shown = describe(&view.entries);
    assert_eq!(shown[0], "user: hello");
    assert!(
        shown[1].starts_with("notice: Your Claude account reached its usage limit; it resets ")
            && shown[1].ends_with("Builder will carry on by itself then."),
        "{}",
        shown[1]
    );

    // A message meanwhile waits with it; then the limit resets.
    apply_request(&mut client, &mut view, send_message("builder", "meanwhile")).await;
    limited.store(false, Ordering::SeqCst);
    view.follow(&mut client, |view| describe(&view.entries).iter().any(|entry| entry == "assistant: hi")).await;
    view.follow(&mut client, |view| builder(view).status() == v1::AgentStatus::Idle).await;
    assert_eq!(describe(&view.entries)[2..], ["user: meanwhile", "assistant: hi"]);
    assert_eq!((builder(&view).usage_wait.as_str(), builder(&view).usage_wait_until_ms), ("", 0));
    assert_eq!(model_requests(&seen), 2, "one at the limit, one after it reset");
    assert!(now_ms() >= resets * 1000, "not before the reset");
}

/// A wait ends early when the person uses a usage reset, and a backend
/// restart in the middle of one resumes the run, which waits again.
#[tokio::test]
async fn a_wait_survives_a_restart_and_ends_early_with_a_usage_reset() {
    let dir = temp_dir();
    let resets = now_ms() / 1000 + 3 * 3600;
    let (base_url, seen) = anthropic(resets).await;
    let factory = Factory::open(crate::common::home(&dir), Arc::new(only_a(&dir, &base_url))).await.expect("open");
    let (mut client, mut view) = serve(&factory, &dir).await;
    create_builder(&mut client, &mut view).await;
    apply_request(&mut client, &mut view, send_message("builder", "hello")).await;
    view.follow(&mut client, |view| builder(view).usage_wait_until_ms == i64::try_from(resets * 1000).expect("ms")).await;

    // Restarted: the open run resumes, finds no account free, and waits again, asking nothing.
    let factory = Factory::open(crate::common::home(&dir), Arc::new(only_a_again(&dir, &base_url))).await.expect("open again");
    let (mut client, mut view) = serve(&factory, &dir).await;
    view.follow(&mut client, |view| !builder(view).usage_wait.is_empty()).await;
    assert_eq!(model_requests(&seen), 1);

    // The person uses a reset: it asks again at once, and the answer comes.
    view.follow(&mut client, |view| claude(view).accounts[0].usage.as_ref().is_some_and(|usage| !usage.resets.is_empty())).await;
    let use_it =
        Request::UseUsageReset(v1::UseUsageReset { provider_id: "anthropic".into(), account_id: "a".into(), reset_id: "full_1".into() });
    assert!(matches!(apply_request(&mut client, &mut view, use_it).await, Body::UsageResetAnswer(_)));
    let select = Request::SelectAgent(v1::SelectAgent { agent_id: "builder".into() });
    apply_request(&mut client, &mut view, select).await;
    view.follow(&mut client, |view| describe(&view.entries).iter().any(|entry| entry == "assistant: hi")).await;
    view.follow(&mut client, |view| builder(view).status() == v1::AgentStatus::Idle && builder(view).usage_wait.is_empty()).await;
    assert_eq!(model_requests(&seen), 2);
}

/// `only_a`'s registry, over the credentials file as it is now (account a
/// marked as at its limit).
fn only_a_again(dir: &std::path::Path, base_url: &str) -> Registry {
    let credentials = std::fs::read_to_string(dir.join("auth.json")).expect("credentials");
    let registry = registry(dir, base_url);
    std::fs::write(dir.join("auth.json"), credentials).expect("credentials");
    registry
}
