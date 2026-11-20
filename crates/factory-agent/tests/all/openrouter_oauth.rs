use std::time::Duration;

use crate::support::auth::{login_error, pkce_challenge, scripted};
use crate::support::server::{serve, served_json, served_text};
use factory_agent::auth::{AuthEvent, AuthPrompt, OAuthFlow, RequestAuth};
use factory_agent::credentials::Credential;
use factory_agent::providers::openrouter::oauth::OpenRouterOAuth;
use serde_json::json;
use tokio_util::sync::CancellationToken;

fn flow(server: &crate::support::server::Server) -> OpenRouterOAuth {
    OpenRouterOAuth { authorize_url: format!("{}/auth", server.base_url), token_url: format!("{}/api/v1/auth/keys", server.base_url) }
}

fn callback_url(events: &[AuthEvent]) -> Option<String> {
    events.iter().find_map(|event| match event {
        AuthEvent::Progress { message } => message.strip_prefix("Listening for OpenRouter OAuth callback on ").map(str::to_owned),
        _ => None,
    })
}

fn auth_url(events: &[AuthEvent]) -> Option<String> {
    events.iter().find_map(|event| match event {
        AuthEvent::AuthUrl { url, .. } => Some(url.clone()),
        _ => None,
    })
}

#[tokio::test]
async fn the_callback_exchanges_the_code_for_a_permanent_key() {
    let server = serve(vec![served_json(200, json!({"key": "sk-or-v1-new"}))]).await;
    let flow = flow(&server);
    let interaction = scripted(|_, _| None);
    let events = interaction.events.clone();
    let browser = tokio::spawn(async move {
        loop {
            let seen = {
                let events = events.lock();
                (callback_url(&events), auth_url(&events))
            };
            let (Some(callback), Some(authorize)) = seen else {
                tokio::time::sleep(Duration::from_millis(10)).await;
                continue;
            };
            let authorize = url::Url::parse(&authorize).expect("url");
            let query: Vec<(String, String)> = authorize.query_pairs().map(|(key, value)| (key.into_owned(), value.into_owned())).collect();
            assert_eq!(authorize.path(), "/auth");
            assert_eq!(
                query.iter().map(|(key, _)| key.as_str()).collect::<Vec<_>>(),
                ["callback_url", "code_challenge", "code_challenge_method"]
            );
            assert_eq!(query[0].1, callback);
            assert_eq!(query[2].1, "S256");
            let missing = reqwest::get(format!("{callback}?nothing=1")).await.expect("no code");
            assert_eq!(missing.status().as_u16(), 400);
            assert!(missing.text().await.expect("html").contains("OpenRouter returned no authorization code."));
            let wrong = reqwest::get(callback.replace("/oauth/callback/", "/elsewhere/")).await.expect("404");
            assert_eq!(wrong.status().as_u16(), 404);
            let ok = reqwest::get(format!("{callback}?code=abc")).await.expect("callback");
            assert_eq!(ok.status().as_u16(), 200);
            assert!(ok.text().await.expect("html").contains("Signed in to OpenRouter. You may now close this page."));
            return query[1].1.clone();
        }
    });
    let credential = flow.login(&interaction).await.unwrap();
    let challenge = browser.await.unwrap();
    assert_eq!(
        credential,
        Credential::Oauth {
            access: "sk-or-v1-new".into(),
            refresh: String::new(),
            expires: 9_007_199_254_740_991,
            account_id: None,
            extra: Default::default()
        }
    );
    let request = server.requests.lock()[0].clone();
    assert_eq!(request.path, "/api/v1/auth/keys");
    assert_eq!(request.body["code"], "abc");
    assert_eq!(request.body["code_challenge_method"], "S256");
    let verifier = request.body["code_verifier"].as_str().expect("verifier");
    assert_eq!(pkce_challenge(verifier), challenge);
    assert!(
        matches!(&interaction.events.lock()[1], AuthEvent::AuthUrl { instructions, .. } if instructions.as_deref() == Some("Complete sign-in in your browser. If the browser is on another machine, paste the final redirect URL here."))
    );
    assert_eq!(flow.refresh(credential.clone(), CancellationToken::new()).await.unwrap(), credential);
    assert_eq!(
        flow.to_auth(&credential).unwrap(),
        RequestAuth { api_key: Some("sk-or-v1-new".into()), headers: Vec::new(), base_url: None }
    );
    assert_eq!((flow.name(), flow.is_subscription()), ("OpenRouter OAuth", false));
}

#[tokio::test]
async fn a_denied_callback_fails_the_login_with_the_description() {
    let server = serve(Vec::new()).await;
    let flow = flow(&server);
    let interaction = scripted(|_, _| None);
    let events = interaction.events.clone();
    tokio::spawn(async move {
        loop {
            let callback = callback_url(&events.lock());
            if let Some(callback) = callback {
                let denied =
                    reqwest::get(format!("{callback}?error=access_denied&error_description=user%20said%20no")).await.expect("denied");
                assert_eq!(denied.status().as_u16(), 400);
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    });
    assert_eq!(login_error(&flow, &interaction).await, "OpenRouter authorization failed: user said no");
}

#[tokio::test]
async fn a_pasted_url_or_code_is_exchanged_and_exchange_failures_use_pis_texts() {
    let server = serve(vec![
        served_json(200, json!({"key": "sk-or-v1-pasted"})),
        served_json(403, json!({"error": {"message": "forbidden"}})),
        served_json(200, json!({"nokey": true})),
        served_text(200, "{not json"),
        served_json(401, json!({"error_description": "expired"})),
    ])
    .await;
    let flow = flow(&server);
    let paste = scripted(|prompt, events| match prompt {
        AuthPrompt::ManualCode { placeholder, .. } => {
            assert_eq!(placeholder.as_deref(), callback_url(events).as_deref());
            Some(format!("{}?code=pasted", placeholder.clone().expect("placeholder")))
        }
        _ => None,
    });
    let credential = flow.login(&paste).await.unwrap();
    assert!(matches!(credential, Credential::Oauth { ref access, .. } if access == "sk-or-v1-pasted"));
    assert_eq!(server.requests.lock()[0].body["code"], "pasted");
    assert!(
        paste
            .events
            .lock()
            .iter()
            .any(|event| matches!(event, AuthEvent::Progress { message } if message == "Exchanging authorization code for an API key..."))
    );
    let bare = || scripted(|prompt, _| matches!(prompt, AuthPrompt::ManualCode { .. }).then(|| "  bare-code  ".to_owned()));
    assert_eq!(login_error(&flow, &bare()).await, "OpenRouter OAuth key exchange failed (HTTP 403): forbidden");
    assert_eq!(login_error(&flow, &bare()).await, "OpenRouter OAuth response carries no \"key\"");
    assert_eq!(login_error(&flow, &bare()).await, "OpenRouter OAuth returned invalid JSON");
    assert_eq!(login_error(&flow, &bare()).await, "OpenRouter OAuth key exchange failed (HTTP 401): expired");
    assert_eq!(server.requests.lock()[1].body["code"], "bare-code");
    let empty = scripted(|prompt, _| matches!(prompt, AuthPrompt::ManualCode { .. }).then(|| "   ".to_owned()));
    assert_eq!(login_error(&flow, &empty).await, "Missing authorization code");
    let cancelled = scripted(|_, _| None);
    cancelled.cancel.cancel();
    assert_eq!(login_error(&flow, &cancelled).await, "Login cancelled");
}
