mod support;

use std::time::Duration;

use factory_agent::auth::{AuthEvent, AuthPrompt, OAuthFlow, RequestAuth};
use factory_agent::clock::now_ms;
use factory_agent::credentials::Credential;
use factory_agent::providers::openai_codex::oauth::{CodexOAuth, account_id};
use serde_json::json;
use support::auth::{login_error, pkce_challenge, scripted};
use support::server::{HANG, redirect_port, serve, served_json, served_text};
use tokio_util::sync::CancellationToken;

fn b64url(text: &str) -> String {
    use base64::Engine;
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(text)
}

fn token(account: &str) -> String {
    format!(
        "{}.{}.sig",
        b64url("{\"alg\":\"none\"}"),
        b64url(&format!("{{\"https://api.openai.com/auth\":{{\"chatgpt_account_id\":\"{account}\"}}}}"))
    )
}

fn token_json(account: &str) -> serde_json::Value {
    json!({"access_token": token(account), "refresh_token": "refresh-1", "expires_in": 3600})
}

fn codex_flow(server: &support::server::Server, callback_port: u16) -> CodexOAuth {
    CodexOAuth { auth_base_url: server.base_url.clone(), callback_port }
}

fn form(raw: &str) -> Vec<(String, String)> {
    url::form_urlencoded::parse(raw.as_bytes()).map(|(key, value)| (key.into_owned(), value.into_owned())).collect()
}

fn field<'a>(pairs: &'a [(String, String)], name: &str) -> Option<&'a str> {
    pairs.iter().find(|(key, _)| key == name).map(|(_, value)| value.as_str())
}

#[test]
fn the_account_id_comes_from_the_jwt_claim() {
    assert_eq!(account_id(&token("acct_123")).as_deref(), Some("acct_123"));
    assert_eq!(account_id("not.a.jwt.at.all"), None);
    assert_eq!(account_id(&format!("a.{}.c", b64url("{\"sub\":\"x\"}"))), None);
    let standard = format!(
        "a.{}.c",
        base64::Engine::encode(
            &base64::engine::general_purpose::STANDARD,
            "{\"https://api.openai.com/auth\":{\"chatgpt_account_id\":\"std\"}}"
        )
    );
    assert_eq!(account_id(&standard).as_deref(), Some("std"));
}

#[tokio::test]
async fn device_code_login_polls_backs_off_and_exchanges_with_the_device_redirect() {
    let server = serve(vec![
        served_json(200, json!({"device_auth_id": "dev-1", "user_code": "ABCD-EFGH", "interval": "0"})),
        served_text(404, "pending"),
        served_json(400, json!({"error": {"code": "slow_down"}})),
        served_json(400, json!({"error": "deviceauth_authorization_pending"})),
        served_json(200, json!({"authorization_code": "auth-code", "code_verifier": "server-verifier"})),
        served_json(200, token_json("acct_dev")),
    ])
    .await;
    let flow = codex_flow(&server, 0);
    let interaction = scripted(|prompt, _| matches!(prompt, AuthPrompt::Select { .. }).then(|| "device_code".to_owned()));
    let started = std::time::Instant::now();
    let credential = flow.login(&interaction).await.unwrap();
    assert!(started.elapsed() >= Duration::from_secs(3), "{:?}", started.elapsed());
    let Credential::Oauth { access, refresh, expires, account_id, .. } = credential else { panic!("oauth") };
    assert_eq!((access, refresh, account_id.as_deref()), (token("acct_dev"), "refresh-1".into(), Some("acct_dev")));
    assert!(expires > now_ms() + 3_500_000 && expires <= now_ms() + 3_600_000);
    let events = interaction.events.lock().clone();
    assert_eq!(
        events,
        vec![AuthEvent::DeviceCode {
            user_code: "ABCD-EFGH".into(),
            verification_uri: format!("{}/codex/device", server.base_url),
            interval_seconds: Some(0),
            expires_in_seconds: Some(900),
        }]
    );
    let requests = server.requests.lock().clone();
    assert_eq!(requests.len(), 6);
    assert_eq!(requests[0].path, "/api/accounts/deviceauth/usercode");
    assert_eq!(requests[0].body, json!({"client_id": "app_EMoamEEZ73f0CkXaXp7hrann"}));
    assert!(requests[1..5].iter().all(|request| request.path == "/api/accounts/deviceauth/token"
        && request.body == json!({"device_auth_id": "dev-1", "user_code": "ABCD-EFGH"})));
    assert_eq!(requests[5].path, "/oauth/token");
    let exchange = form(&requests[5].raw);
    assert_eq!(field(&exchange, "grant_type"), Some("authorization_code"));
    assert_eq!(field(&exchange, "client_id"), Some("app_EMoamEEZ73f0CkXaXp7hrann"));
    assert_eq!(field(&exchange, "code"), Some("auth-code"));
    assert_eq!(field(&exchange, "code_verifier"), Some("server-verifier"));
    assert_eq!(field(&exchange, "redirect_uri"), Some(format!("{}/deviceauth/callback", server.base_url).as_str()));
    assert_eq!(
        requests[5].headers.iter().find(|(name, _)| name == "content-type").map(|(_, value)| value.as_str()),
        Some("application/x-www-form-urlencoded")
    );
}

#[tokio::test]
async fn device_code_failures_use_pis_texts() {
    let server = serve(vec![served_text(404, "")]).await;
    let flow = codex_flow(&server, 0);
    let interaction = scripted(|_, _| Some("device_code".into()));
    assert_eq!(
        login_error(&flow, &interaction).await,
        "OpenAI Codex device code login is not enabled for this server. Use browser login or verify the server URL."
    );
    let server = serve(vec![served_text(500, "down")]).await;
    let flow = codex_flow(&server, 0);
    assert_eq!(login_error(&flow, &interaction).await, "OpenAI Codex device code request failed with status 500: down");
    let server = serve(vec![served_json(200, json!({"device_auth_id": "dev-1"}))]).await;
    let flow = codex_flow(&server, 0);
    assert_eq!(login_error(&flow, &interaction).await, "Invalid OpenAI Codex device code response: {\"device_auth_id\":\"dev-1\"}");
    let server = serve(vec![
        served_json(200, json!({"device_auth_id": "dev-1", "user_code": "X", "interval": 0})),
        served_json(401, json!({"error": {"code": "expired_token"}})),
    ])
    .await;
    let flow = codex_flow(&server, 0);
    assert_eq!(
        login_error(&flow, &interaction).await,
        "OpenAI Codex device auth failed with status 401: {\"error\":{\"code\":\"expired_token\"}}"
    );
    let server = serve(vec![
        served_json(200, json!({"device_auth_id": "dev-1", "user_code": "X", "interval": 0})),
        served_json(200, json!({"authorization_code": "c"})),
    ])
    .await;
    let flow = codex_flow(&server, 0);
    assert_eq!(login_error(&flow, &interaction).await, "Invalid OpenAI Codex device auth token response: {\"authorization_code\":\"c\"}");
    let unknown = scripted(|_, _| Some("carrier-pigeon".into()));
    assert_eq!(login_error(&flow, &unknown).await, "Unknown OpenAI Codex login method: carrier-pigeon");
    let cancelled = scripted(|_, _| Some("device_code".into()));
    cancelled.cancel.cancel();
    let server = serve(vec![served_json(200, json!({"device_auth_id": "dev-1", "user_code": "X", "interval": 0}))]).await;
    let flow = codex_flow(&server, 0);
    assert_eq!(login_error(&flow, &cancelled).await, "Login cancelled");
}

#[tokio::test]
async fn browser_login_takes_the_callback_and_exchanges_the_code() {
    let server = serve(vec![served_json(200, token_json("acct_browser"))]).await;
    // Port 0: the flow listens on a free port and names it in the redirect.
    let flow = codex_flow(&server, 0);
    let interaction = scripted(|prompt, _| matches!(prompt, AuthPrompt::Select { .. }).then(|| "browser".to_owned()));
    let events = interaction.events.clone();
    let callback = tokio::spawn(async move {
        loop {
            let url = events.lock().iter().find_map(|event| match event {
                AuthEvent::AuthUrl { url, .. } => Some(url.clone()),
                _ => None,
            });
            let Some(url) = url else {
                tokio::time::sleep(Duration::from_millis(10)).await;
                continue;
            };
            let parsed = url::Url::parse(&url).expect("url");
            let query: Vec<(String, String)> = parsed.query_pairs().map(|(key, value)| (key.into_owned(), value.into_owned())).collect();
            assert_eq!(parsed.path(), "/oauth/authorize");
            assert_eq!(field(&query, "response_type"), Some("code"));
            assert_eq!(field(&query, "client_id"), Some("app_EMoamEEZ73f0CkXaXp7hrann"));
            let port = redirect_port(field(&query, "redirect_uri").expect("redirect_uri"), "/auth/callback");
            assert_ne!(port, 0);
            assert_eq!(field(&query, "scope"), Some("openid profile email offline_access"));
            assert_eq!(field(&query, "code_challenge_method"), Some("S256"));
            assert_eq!(field(&query, "id_token_add_organizations"), Some("true"));
            assert_eq!(field(&query, "codex_cli_simplified_flow"), Some("true"));
            assert_eq!(field(&query, "originator"), Some("pi"));
            let state = field(&query, "state").expect("state").to_owned();
            let challenge = field(&query, "code_challenge").expect("challenge").to_owned();
            let wrong = reqwest::get(format!("http://127.0.0.1:{port}/auth/callback?code=x&state=wrong")).await.expect("wrong state");
            assert_eq!(wrong.status().as_u16(), 400);
            assert!(wrong.text().await.expect("html").contains("State mismatch."));
            let missing = reqwest::get(format!("http://127.0.0.1:{port}/nope")).await.expect("404");
            assert_eq!(missing.status().as_u16(), 404);
            let ok =
                reqwest::get(format!("http://127.0.0.1:{port}/auth/callback?code=browser-code&state={state}")).await.expect("callback");
            assert_eq!(ok.status().as_u16(), 200);
            assert!(ok.text().await.expect("html").contains("OpenAI authentication completed. You can close this window."));
            return (challenge, port);
        }
    });
    let credential = flow.login(&interaction).await.unwrap();
    let (challenge, port) = callback.await.unwrap();
    let Credential::Oauth { account_id, .. } = credential else { panic!("oauth") };
    assert_eq!(account_id.as_deref(), Some("acct_browser"));
    let prompts = interaction.prompts.lock().clone();
    assert!(
        matches!(&prompts[1], AuthPrompt::ManualCode { message, placeholder } if message == "Complete login in your browser, or paste the authorization code / redirect URL here:" && placeholder.as_deref() == Some("http://localhost:1455/auth/callback"))
    );
    let requests = server.requests.lock().clone();
    assert_eq!(requests.len(), 1);
    let exchange = form(&requests[0].raw);
    assert_eq!(field(&exchange, "code"), Some("browser-code"));
    assert_eq!(field(&exchange, "redirect_uri"), Some(format!("http://localhost:{port}/auth/callback").as_str()));
    let verifier = field(&exchange, "code_verifier").expect("verifier");
    assert_eq!(pkce_challenge(verifier), challenge);
    assert!(
        matches!(interaction.events.lock()[0], AuthEvent::AuthUrl { ref instructions, .. } if instructions.as_deref() == Some("A browser window should open. Complete login to finish."))
    );
}

#[tokio::test]
async fn browser_login_accepts_a_pasted_code_or_url_and_rejects_a_state_mismatch() {
    let server = serve(vec![served_json(200, token_json("acct_paste")), served_json(200, token_json("acct_paste"))]).await;
    let paste = scripted(move |prompt, events| match prompt {
        AuthPrompt::Select { .. } => Some("browser".into()),
        AuthPrompt::ManualCode { .. } => {
            let state = events
                .iter()
                .find_map(|event| match event {
                    AuthEvent::AuthUrl { url, .. } => {
                        url::Url::parse(url).ok()?.query_pairs().find(|(key, _)| key == "state").map(|(_, value)| value.into_owned())
                    }
                    _ => None,
                })
                .unwrap_or_default();
            Some(format!("http://localhost:1455/auth/callback?code=pasted-code&state={state}"))
        }
        _ => None,
    });
    let flow = codex_flow(&server, 0);
    let credential = flow.login(&paste).await.unwrap();
    assert!(matches!(credential, Credential::Oauth { ref account_id, .. } if account_id.as_deref() == Some("acct_paste")));
    assert_eq!(field(&form(&server.requests.lock()[0].raw), "code"), Some("pasted-code"));
    let bare = scripted(|prompt, _| match prompt {
        AuthPrompt::Select { .. } => Some("browser".into()),
        AuthPrompt::ManualCode { .. } => Some("  bare-code  ".into()),
        _ => None,
    });
    let flow = codex_flow(&server, 0);
    flow.login(&bare).await.unwrap();
    assert_eq!(field(&form(&server.requests.lock()[1].raw), "code"), Some("bare-code"));
    let mismatch = scripted(|prompt, _| match prompt {
        AuthPrompt::Select { .. } => Some("browser".into()),
        AuthPrompt::ManualCode { .. } => Some("code#not-the-state".into()),
        _ => None,
    });
    let flow = codex_flow(&server, 0);
    assert_eq!(login_error(&flow, &mismatch).await, "State mismatch");
    let empty = scripted(|prompt, _| match prompt {
        AuthPrompt::Select { .. } => Some("browser".into()),
        AuthPrompt::ManualCode { .. } => Some("   ".into()),
        _ => None,
    });
    let flow = codex_flow(&server, 0);
    assert_eq!(login_error(&flow, &empty).await, "Missing authorization code");
}

#[tokio::test]
async fn refresh_rotates_the_credential_and_token_errors_use_pis_texts() {
    let server = serve(vec![
        served_json(200, json!({"access_token": token("acct_new"), "refresh_token": "refresh-2", "expires_in": 60})),
        served_text(401, "nope"),
        served_json(200, json!({"access_token": token("acct_new")})),
        served_json(200, json!({"access_token": "no.claim.here", "refresh_token": "r", "expires_in": 1})),
    ])
    .await;
    let flow = codex_flow(&server, 0);
    let stale = Credential::Oauth {
        access: "old".into(),
        refresh: "refresh-1".into(),
        expires: 0,
        account_id: Some("acct_old".into()),
        extra: Default::default(),
    };
    let fresh = flow.refresh(stale.clone(), CancellationToken::new()).await.unwrap();
    let Credential::Oauth { access, refresh, expires, account_id, .. } = fresh.clone() else { panic!("oauth") };
    assert_eq!((access.as_str(), refresh.as_str(), account_id.as_deref()), (token("acct_new").as_str(), "refresh-2", Some("acct_new")));
    assert!(expires > now_ms() + 50_000 && expires <= now_ms() + 60_000);
    let exchange = form(&server.requests.lock()[0].raw);
    assert_eq!(field(&exchange, "grant_type"), Some("refresh_token"));
    assert_eq!(field(&exchange, "refresh_token"), Some("refresh-1"));
    assert_eq!(field(&exchange, "client_id"), Some("app_EMoamEEZ73f0CkXaXp7hrann"));
    assert_eq!(flow.to_auth(&fresh).unwrap(), RequestAuth { api_key: Some(token("acct_new")), headers: Vec::new(), base_url: None });
    assert_eq!(
        flow.refresh(stale.clone(), CancellationToken::new()).await.unwrap_err().to_string(),
        "OpenAI Codex token refresh failed (401): nope"
    );
    assert_eq!(
        flow.refresh(stale.clone(), CancellationToken::new()).await.unwrap_err().to_string(),
        format!("OpenAI Codex token refresh response missing fields: {{\"access_token\":\"{}\"}}", token("acct_new"))
    );
    assert_eq!(flow.refresh(stale, CancellationToken::new()).await.unwrap_err().to_string(), "Failed to extract accountId from token");
    assert_eq!(flow.name(), "OpenAI (ChatGPT Plus/Pro)");
    assert!(flow.is_subscription());
}

#[tokio::test]
async fn a_silent_connection_does_not_hold_up_the_browser_callback() {
    let server = serve(vec![served_json(200, token_json("acct_browser"))]).await;
    let flow = codex_flow(&server, 0);
    let interaction = scripted(|prompt, _| matches!(prompt, AuthPrompt::Select { .. }).then(|| "browser".to_owned()));
    let events = interaction.events.clone();
    let callback = tokio::spawn(async move {
        let url = loop {
            let url = events.lock().iter().find_map(|event| match event {
                AuthEvent::AuthUrl { url, .. } => Some(url.clone()),
                _ => None,
            });
            match url {
                Some(url) => break url,
                None => tokio::time::sleep(Duration::from_millis(10)).await,
            }
        };
        let parsed = url::Url::parse(&url).expect("url");
        let state = parsed.query_pairs().find(|(key, _)| key == "state").map(|(_, value)| value.into_owned()).expect("state");
        let redirect = parsed.query_pairs().find(|(key, _)| key == "redirect_uri").map(|(_, value)| value.into_owned()).expect("redirect");
        let port = redirect_port(&redirect, "/auth/callback");
        let silent = tokio::net::TcpStream::connect(("127.0.0.1", port)).await.expect("preconnect");
        let ok = reqwest::get(format!("http://127.0.0.1:{port}/auth/callback?code=browser-code&state={state}")).await.expect("callback");
        assert_eq!(ok.status().as_u16(), 200);
        drop(silent);
    });
    let credential = tokio::time::timeout(HANG, flow.login(&interaction)).await.expect("the callback is served").unwrap();
    callback.await.unwrap();
    assert!(matches!(credential, Credential::Oauth { .. }));
}
