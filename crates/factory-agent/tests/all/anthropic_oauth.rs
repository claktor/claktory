use std::time::Duration;

use crate::support::auth::{login_error, pkce_challenge, scripted};
use crate::support::server::{redirect_port, serve, served_json, served_text};
use factory_agent::auth::{AuthEvent, AuthPrompt, OAuthFlow, RequestAuth};
use factory_agent::clock::now_ms;
use factory_agent::credentials::Credential;
use factory_agent::providers::anthropic::oauth::{AnthropicOAuth, CLIENT_ID};
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

/// Port 0: the flow listens on a free port and names it in its redirect.
fn flow(server: &crate::support::server::Server) -> AnthropicOAuth {
    AnthropicOAuth {
        authorize_url: format!("{}/oauth/authorize", server.base_url),
        token_url: format!("{}/v1/oauth/token", server.base_url),
        callback_port: 0,
    }
}

/// The redirect URI of the sign-in URL a flow published.
fn redirect_of(events: &[AuthEvent]) -> Option<String> {
    events.iter().find_map(|event| match event {
        AuthEvent::AuthUrl { url, .. } => {
            url::Url::parse(url).ok()?.query_pairs().find(|(key, _)| key == "redirect_uri").map(|(_, value)| value.into_owned())
        }
        _ => None,
    })
}

fn state_of(events: &[AuthEvent]) -> Option<String> {
    events.iter().find_map(|event| match event {
        AuthEvent::AuthUrl { url, .. } => {
            url::Url::parse(url).ok()?.query_pairs().find(|(key, _)| key == "state").map(|(_, value)| value.into_owned())
        }
        _ => None,
    })
}

fn token_json() -> Value {
    json!({"access_token": "sk-ant-oat01-access", "refresh_token": "sk-ant-ort01-refresh", "expires_in": 3600})
}

#[test]
fn the_client_id_is_decoded() {
    assert_eq!(CLIENT_ID.as_str(), "9d1c250a-e61b-44d9-88ed-5944d1962f5e");
}

#[tokio::test]
async fn the_callback_with_the_verifier_as_state_exchanges_the_code() {
    let server = serve(vec![served_json(200, token_json())]).await;
    let flow = flow(&server);
    let interaction = scripted(|_, _| None);
    let events = interaction.events.clone();
    let browser = tokio::spawn(async move {
        loop {
            let Some(AuthEvent::AuthUrl { url, instructions }) = events.lock().first().cloned() else {
                tokio::time::sleep(Duration::from_millis(10)).await;
                continue;
            };
            assert_eq!(
                instructions.as_deref(),
                Some("Complete login in your browser. If the browser is on another machine, paste the final redirect URL here.")
            );
            let parsed = url::Url::parse(&url).expect("url");
            let query: Vec<(String, String)> = parsed.query_pairs().map(|(key, value)| (key.into_owned(), value.into_owned())).collect();
            assert_eq!(parsed.path(), "/oauth/authorize");
            assert_eq!(
                query.iter().map(|(key, _)| key.as_str()).collect::<Vec<_>>(),
                ["code", "client_id", "response_type", "redirect_uri", "scope", "code_challenge", "code_challenge_method", "state"]
            );
            assert_eq!(query[0].1, "true");
            assert_eq!(query[1].1, CLIENT_ID.as_str());
            let port = redirect_port(&query[3].1, "/callback");
            assert_ne!(port, 0);
            assert_eq!(
                query[4].1,
                "org:create_api_key user:profile user:inference user:sessions:claude_code user:mcp_servers user:file_upload"
            );
            let state = query[7].1.clone();
            let mismatch = reqwest::get(format!("http://127.0.0.1:{port}/callback?code=x&state=nope")).await.expect("mismatch");
            assert_eq!(mismatch.status().as_u16(), 400);
            assert!(mismatch.text().await.expect("html").contains("State mismatch."));
            let denied = reqwest::get(format!("http://127.0.0.1:{port}/callback?error=access_denied")).await.expect("denied");
            assert_eq!(denied.status().as_u16(), 400);
            assert!(denied.text().await.expect("html").contains("Anthropic authentication did not complete."));
            let missing = reqwest::get(format!("http://127.0.0.1:{port}/callback?code=x")).await.expect("missing");
            assert_eq!(missing.status().as_u16(), 400);
            assert!(missing.text().await.expect("html").contains("Missing code or state parameter."));
            let ok = reqwest::get(format!("http://127.0.0.1:{port}/callback?code=the-code&state={state}")).await.expect("callback");
            assert_eq!(ok.status().as_u16(), 200);
            assert!(ok.text().await.expect("html").contains(
                "Claktory is finishing your Anthropic sign-in. You can close this tab and go back to Claktory to see how it went."
            ));
            return (state, query[5].1.clone(), port);
        }
    });
    let credential = flow.login(&interaction).await.unwrap();
    let (state, challenge, port) = browser.await.unwrap();
    let Credential::Oauth { access, refresh, expires, account_id, .. } = credential.clone() else { panic!("oauth") };
    assert_eq!((access.as_str(), refresh.as_str(), account_id), ("sk-ant-oat01-access", "sk-ant-ort01-refresh", None));
    assert!(expires > now_ms() + 3_200_000 && expires <= now_ms() + 3_300_000);
    let request = server.requests.lock()[0].clone();
    assert_eq!(request.path, "/v1/oauth/token");
    assert_eq!(request.body["grant_type"], "authorization_code");
    assert_eq!(request.body["client_id"], CLIENT_ID.as_str());
    assert_eq!(request.body["code"], "the-code");
    assert_eq!(request.body["state"], state);
    assert_eq!(request.body["redirect_uri"], format!("http://localhost:{port}/callback"));
    assert_eq!(request.body["code_verifier"], state);
    assert_eq!(pkce_challenge(&state), challenge);
    assert!(
        interaction
            .events
            .lock()
            .iter()
            .any(|event| matches!(event, AuthEvent::Progress { message } if message == "Exchanging authorization code for tokens..."))
    );
    assert_eq!(
        flow.to_auth(&credential).unwrap(),
        RequestAuth { api_key: Some("sk-ant-oat01-access".into()), headers: Vec::new(), base_url: None }
    );
    assert_eq!((flow.name(), flow.is_subscription()), ("Anthropic (Claude Pro/Max)", true));
}

#[tokio::test]
async fn pasted_input_and_exchange_failures_use_pis_texts() {
    let server = serve(vec![
        served_json(200, token_json()),
        served_json(200, token_json()),
        served_text(400, "bad request"),
        served_text(200, "{not json"),
    ])
    .await;
    let flow = flow(&server);
    let with_state = scripted(|prompt, events| match prompt {
        AuthPrompt::ManualCode { .. } => {
            Some(format!("http://localhost:1/callback?code=pasted&state={}", state_of(events).expect("state")))
        }
        _ => None,
    });
    flow.login(&with_state).await.unwrap();
    assert_eq!(server.requests.lock()[0].body["code"], "pasted");
    let bare = scripted(|prompt, _| matches!(prompt, AuthPrompt::ManualCode { .. }).then(|| "bare".to_owned()));
    flow.login(&bare).await.unwrap();
    let request = server.requests.lock()[1].clone();
    assert_eq!(request.body["code"], "bare");
    assert_eq!(request.body["state"], request.body["code_verifier"]);
    let mismatch = scripted(|prompt, _| matches!(prompt, AuthPrompt::ManualCode { .. }).then(|| "code#wrong".to_owned()));
    assert_eq!(login_error(&flow, &mismatch).await, "OAuth state mismatch");
    let empty = scripted(|prompt, _| matches!(prompt, AuthPrompt::ManualCode { .. }).then(|| " ".to_owned()));
    assert_eq!(login_error(&flow, &empty).await, "Missing authorization code");
    let bare = || scripted(|prompt, _| matches!(prompt, AuthPrompt::ManualCode { .. }).then(|| "bare".to_owned()));
    let failing = bare();
    let failed = login_error(&flow, &failing).await;
    let redirect = redirect_of(&failing.events.lock()).expect("redirect");
    assert_ne!(redirect_port(&redirect, "/callback"), 0);
    assert_eq!(
        failed,
        format!(
            "Token exchange request failed. url={0}; redirect_uri={redirect}; response_type=authorization_code; details=Error: HTTP request failed. status=400; url={0}; body=bad request",
            flow.token_url
        )
    );
    let invalid = login_error(&flow, &bare()).await;
    assert!(
        invalid.starts_with(&format!("Token exchange returned invalid JSON. url={}; body={{not json; details=Error: ", flow.token_url)),
        "{invalid}"
    );
}

#[tokio::test]
async fn a_busy_callback_port_still_lets_the_person_paste_the_redirect() {
    let server = serve(vec![served_json(200, token_json())]).await;
    let taken = std::net::TcpListener::bind(("127.0.0.1", 0)).expect("hold a port");
    let flow = AnthropicOAuth { callback_port: taken.local_addr().expect("address").port(), ..flow(&server) };
    let pasting = scripted(|prompt, events| match prompt {
        AuthPrompt::ManualCode { .. } => {
            Some(format!("http://localhost:1/callback?code=pasted&state={}", state_of(events).expect("state")))
        }
        _ => None,
    });
    flow.login(&pasting).await.unwrap();
    assert_eq!(server.requests.lock()[0].body["code"], "pasted");
}

#[tokio::test]
async fn refresh_rotates_the_credential_five_minutes_early_and_reports_failures() {
    let server = serve(vec![
        served_json(200, json!({"access_token": "a2", "refresh_token": "r2", "expires_in": 600})),
        served_text(500, "down"),
        served_text(200, "nope"),
    ])
    .await;
    let flow = flow(&server);
    let stale = Credential::Oauth { access: "a1".into(), refresh: "r1".into(), expires: 0, account_id: None, extra: Default::default() };
    let fresh = flow.refresh(stale.clone(), CancellationToken::new()).await.unwrap();
    let Credential::Oauth { access, refresh, expires, .. } = fresh else { panic!("oauth") };
    assert_eq!((access.as_str(), refresh.as_str()), ("a2", "r2"));
    assert!(expires > now_ms() + 290_000 && expires <= now_ms() + 300_000);
    let request = server.requests.lock()[0].clone();
    assert_eq!(request.body, json!({"grant_type": "refresh_token", "client_id": CLIENT_ID.as_str(), "refresh_token": "r1"}));
    assert_eq!(request.headers.iter().find(|(name, _)| name == "content-type").map(|(_, value)| value.as_str()), Some("application/json"));
    assert_eq!(
        flow.refresh(stale.clone(), CancellationToken::new()).await.unwrap_err().to_string(),
        format!(
            "Anthropic token refresh request failed. url={0}; details=Error: HTTP request failed. status=500; url={0}; body=down",
            flow.token_url
        )
    );
    let invalid = flow.refresh(stale, CancellationToken::new()).await.unwrap_err().to_string();
    assert!(
        invalid.starts_with(&format!("Anthropic token refresh returned invalid JSON. url={}; body=nope; details=Error: ", flow.token_url)),
        "{invalid}"
    );
}
