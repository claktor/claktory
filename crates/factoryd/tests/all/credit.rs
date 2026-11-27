//! What OpenRouter keys have to spend: read at sign-in, so
//! the step can say a new key has no credit yet, and kept on the account
//! (Account.credit), from OpenRouter's GET /key, numbers only.

use std::sync::{Arc, Mutex};

use crate::accounts::{registry_over, serve};
use crate::common::{View, apply_request, temp_dir};
use factory_protocol::v1::{self, LoginPromptKind, client_message::Body as Request, server_message::Body};
use factoryd::factory::Factory;
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

/// What the mock was asked: (path, its Authorization header).
type Seen = Arc<Mutex<Vec<(String, String)>>>;

/// OpenRouter's GET /key, answering `answer` (status, body).
async fn openrouter(answer: Arc<Mutex<(u16, Value)>>) -> (String, Seen) {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let base_url = format!("http://{}", listener.local_addr().expect("addr"));
    let seen: Seen = Arc::new(Mutex::new(Vec::new()));
    let recorded = seen.clone();
    tokio::spawn(async move {
        loop {
            let Ok((mut socket, _)) = listener.accept().await else { return };
            let (recorded, answer) = (recorded.clone(), answer.clone());
            tokio::spawn(async move {
                let mut raw = Vec::new();
                loop {
                    let mut buffer = [0u8; 8192];
                    let Ok(read) = socket.read(&mut buffer).await else { return };
                    if read == 0 {
                        return;
                    }
                    raw.extend_from_slice(&buffer[..read]);
                    if raw.windows(4).any(|window| window == b"\r\n\r\n") {
                        break;
                    }
                }
                let head = String::from_utf8_lossy(&raw).into_owned();
                let path = head.split(' ').nth(1).unwrap_or_default().to_owned();
                let authorization = head
                    .lines()
                    .filter_map(|line| line.split_once(':'))
                    .find(|(key, _)| key.trim().eq_ignore_ascii_case("authorization"))
                    .map(|(_, value)| value.trim().to_owned())
                    .unwrap_or_default();
                recorded.lock().expect("lock").push((path, authorization));
                let (status, body) = answer.lock().expect("lock").clone();
                let body = body.to_string();
                let response = format!(
                    "HTTP/1.1 {status} X\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = socket.write_all(response.as_bytes()).await;
                let _ = socket.shutdown().await;
            });
        }
    });
    (base_url, seen)
}

fn key(limit: Value, left: Value, daily: f64, free: bool) -> (u16, Value) {
    let data = json!({"label": "sk-or-v1-new...a1b2", "limit": limit, "limit_remaining": left, "limit_reset": "weekly",
        "usage": 12.5, "usage_daily": daily, "is_free_tier": free});
    (200, json!({ "data": data }))
}

fn credit(view: &View) -> Option<v1::AccountCredit> {
    let provider = view.workspace.providers.iter().find(|provider| provider.id == "openrouter")?;
    provider.accounts.first()?.credit.clone()
}

#[tokio::test]
async fn a_new_key_with_no_credit_says_so_when_its_sign_in_ends() {
    let dir = temp_dir();
    let answer = Arc::new(Mutex::new(key(Value::Null, Value::Null, 0.0, true)));
    let (base_url, seen) = openrouter(answer.clone()).await;
    let file = dir.join("auth.json");
    std::fs::write(&file, "{}").expect("credentials");
    let factory = Factory::open(crate::common::home(&dir), Arc::new(registry_over(file, &base_url))).await.expect("open");
    let (mut client, mut view) = serve(&factory, &dir).await;

    let start = Request::StartLogin(v1::StartLogin { provider_id: "openrouter".into(), method_id: "api_key".into(), ..Default::default() });
    assert!(matches!(apply_request(&mut client, &mut view, start).await, Body::Ack(_)));
    view.follow(&mut client, |view| {
        view.workspace.login.as_ref().and_then(|flow| flow.prompt.as_ref()).is_some_and(|prompt| prompt.kind() == LoginPromptKind::Secret)
    })
    .await;
    apply_request(&mut client, &mut view, Request::AnswerLogin(v1::AnswerLogin { text: "sk-or-v1-newkey".into() })).await;
    view.follow(&mut client, |view| view.workspace.login.is_none() && credit(view).is_some()).await;

    // The credit is there in the same Workspace that ends the sign-in.
    let read = credit(&view).expect("credit");
    assert_eq!(read.warning, "This key has no credit yet. Add credit at OpenRouter.");
    assert!(read.never_paid && !read.limited && read.read_ms > 0 && read.error.is_empty(), "{read:?}");
    // The check and the read: GET /key, the key only in its Authorization header.
    for (path, authorization) in seen.lock().expect("lock").iter() {
        assert_eq!((path.as_str(), authorization.as_str()), ("/key", "Bearer sk-or-v1-newkey"));
    }

    // Credit added on OpenRouter, with a limit on the key: the next read
    // shows it, here a sign-in again of the same key.
    *answer.lock().expect("lock") = key(json!(10), json!(4.2), 0.8, false);
    factory.read_provider_credit_for_test("openrouter").await;
    view.follow(&mut client, |view| credit(view).is_some_and(|credit| credit.warning.is_empty())).await;
    let read = credit(&view).expect("credit");
    assert_eq!(
        (read.limited, read.limit, read.left, read.limit_reset.as_str(), read.spent_today, read.never_paid),
        (true, 10.0, 4.2, "weekly", 0.8, false)
    );

    // A refusal: the last good numbers stay, with a sentence that carries
    // no more of OpenRouter's answer than its message, and never the key.
    let refusal = json!({"error": {"code": 401, "message": "sk-or-v1-newkey was revoked"}, "user_id": "user_1"});
    *answer.lock().expect("lock") = (401, refusal);
    factory.read_provider_credit_for_test("openrouter").await;
    view.follow(&mut client, |view| credit(view).is_some_and(|credit| !credit.error.is_empty())).await;
    let read = credit(&view).expect("credit");
    assert_eq!(read.error, "OpenRouter answered 401: [key] was revoked.");
    assert_eq!((read.limit, read.left), (10.0, 4.2));
}
