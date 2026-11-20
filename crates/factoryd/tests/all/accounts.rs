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
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

/// Anthropic's endpoints: the account's token decides the answer.
async fn anthropic(resets_at_seconds: u64) -> (String, Arc<Mutex<Vec<(String, String)>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let base_url = format!("http://{}", listener.local_addr().expect("addr"));
    let seen = Arc::new(Mutex::new(Vec::new()));
    let recorded = seen.clone();
    tokio::spawn(async move {
        loop {
            let Ok((mut socket, _)) = listener.accept().await else { return };
            let recorded = recorded.clone();
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
                recorded.lock().expect("lock").push((path.clone(), account.clone()));
                let (status, extra, content_type, body) = answer(&path, &account, resets_at_seconds);
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

/// Account "a" is over its limit; "b" answers "hi". Each has its own usage
/// and plan, and says who it is.
fn answer(path: &str, account: &str, resets_at_seconds: u64) -> (u16, String, &'static str, String) {
    let json = |body: Value| (200, String::new(), "application/json", body.to_string());
    if path.ends_with("/api/oauth/usage") {
        let used = if account == "a" { 100.0 } else { 12.0 };
        return json(json!({"five_hour": {"utilization": used, "resets_at": "2031-01-01T05:00:00Z"}}));
    }
    if path.ends_with("/api/oauth/profile") {
        let tier = if account == "a" { "default_claude_max_20x" } else { "default_claude_max_5x" };
        return json(json!({
            "account": {"uuid": format!("uuid-{account}"), "email": format!("{account}@x.com")},
            "organization": {"organization_type": "claude_max", "rate_limit_tier": tier},
        }));
    }
    if account == "a" {
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
