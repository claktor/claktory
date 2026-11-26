//! Grok on a subscription (#493): xAI's device sign-in, its refresh and
//! revoke, the proxy's requests and refusals, its model list and who a
//! sign-in is.

use factory_agent::auth::{AuthEvent, OAuthFlow};
use factory_agent::credentials::Credential;
use factory_agent::providers::grok::{self, GrokOAuth};
use factory_agent::stream::{LlmContext, StreamOptions};
use factory_agent::types::{Message, StopReason, UserMessage};
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

use crate::support::auth::{login_error, scripted};
use crate::support::server::{Served, serve, served_json};

fn form(raw: &str) -> Vec<(String, String)> {
    let body = raw.split("\r\n\r\n").nth(1).unwrap_or(raw);
    url::form_urlencoded::parse(body.as_bytes()).into_owned().collect()
}

fn field(raw: &str, name: &str) -> Option<String> {
    form(raw).into_iter().find(|(key, _)| key == name).map(|(_, value)| value)
}

/// A JWT-shaped token with these claims (unsigned: only read, never checked).
fn token(claims: Value) -> String {
    use base64::Engine;
    let encode = |value: &Value| base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(value.to_string());
    format!("{}.{}.sig", encode(&json!({"alg": "none"})), encode(&claims))
}

fn device(uri: &str) -> Served {
    served_json(200, json!({"device_code": "dev-1", "user_code": "AB12-CD34", "verification_uri": uri, "expires_in": 600, "interval": 1}))
}

/// The sign-in: the narrowest scopes with the tool's client, xAI's page and
/// code shown, polling until it's confirmed, the token kept with the
/// scope xAI granted.
#[tokio::test]
async fn a_device_sign_in_asks_for_the_narrowest_scopes_and_keeps_the_grant() {
    let access = token(json!({"sub": "user-0000abcd", "tier": "SuperGrok"}));
    let server = serve(vec![
        device("http://127.0.0.1:9/device"),
        served_json(400, json!({"error": "authorization_pending"})),
        served_json(
            200,
            json!({"access_token": access, "refresh_token": "refresh-1", "expires_in": 21600, "scope": grok::oauth::SCOPE, "token_type": "Bearer"}),
        ),
    ])
    .await;
    let flow = GrokOAuth { issuer: server.base_url.clone() };
    let interaction = scripted(|_, _| None);
    let credential = flow.login(&interaction).await.expect("signed in");
    let Credential::Oauth { access: kept, refresh, expires, extra, .. } = credential else { panic!("a sign-in") };
    assert_eq!((kept.as_str(), refresh.as_str()), (access.as_str(), "refresh-1"));
    let left = expires.saturating_sub(factory_agent::clock::now_ms());
    assert!((21_500_000..=21_600_000).contains(&left), "{left}");
    assert_eq!(extra["scope"], grok::oauth::SCOPE);

    let requests = server.requests.lock().clone();
    assert_eq!(
        requests.iter().map(|request| request.path.as_str()).collect::<Vec<_>>(),
        ["/oauth2/device/code", "/oauth2/token", "/oauth2/token"]
    );
    assert_eq!(field(&requests[0].raw, "scope").as_deref(), Some("openid offline_access grok-cli:access api:access"));
    assert_eq!(field(&requests[0].raw, "client_id").as_deref(), Some(grok::oauth::CLIENT_ID));
    assert_eq!(field(&requests[1].raw, "grant_type").as_deref(), Some("urn:ietf:params:oauth:grant-type:device_code"));
    assert_eq!(field(&requests[1].raw, "device_code").as_deref(), Some("dev-1"));
    let events = interaction.events.lock().clone();
    assert!(
        matches!(&events[..], [AuthEvent::DeviceCode { user_code, verification_uri, .. }] if user_code == "AB12-CD34" && verification_uri == "http://127.0.0.1:9/device"),
        "{events:?}"
    );
}

/// A refusal is final: what xAI said, and nothing asked again another way.
/// A page the answer names elsewhere is never shown.
#[tokio::test]
async fn a_refusal_or_a_page_elsewhere_ends_the_sign_in() {
    let server = serve(vec![served_json(403, json!({"error": "unauthorized_client", "error_description": "Client not allowed"}))]).await;
    let flow = GrokOAuth { issuer: server.base_url.clone() };
    let interaction = scripted(|_, _| None);
    assert_eq!(login_error(&flow, &interaction).await, "xAI refused this sign-in: Client not allowed (403)");
    assert_eq!(server.requests.lock().len(), 1);
    assert!(interaction.events.lock().is_empty());

    for uri in ["https://x.ai.evil.example/device", "http://example.com/device"] {
        let server = serve(vec![device(uri)]).await;
        let flow = GrokOAuth { issuer: server.base_url.clone() };
        let interaction = scripted(|_, _| None);
        assert_eq!(login_error(&flow, &interaction).await, "xAI refused this sign-in: the page it named isn't xAI's.", "{uri}");
        assert!(interaction.events.lock().is_empty(), "{uri} never shown");
        assert_eq!(server.requests.lock().len(), 1, "{uri}: no token asked for");
    }

    let server = serve(vec![device("http://127.0.0.1:9/device"), served_json(400, json!({"error": "access_denied"}))]).await;
    let flow = GrokOAuth { issuer: server.base_url.clone() };
    assert_eq!(login_error(&flow, &scripted(|_, _| None)).await, "The sign-in was declined at xAI.");
}

/// Refreshing takes the new refresh token xAI gives each time; one xAI
/// refuses is a refusal (sign in again), not a failure for now. Signing
/// out revokes the refresh token.
#[tokio::test]
async fn refresh_rotates_and_sign_out_revokes() {
    let server = serve(vec![
        served_json(200, json!({"access_token": "access-2", "refresh_token": "refresh-2", "expires_in": 21600})),
        served_json(400, json!({"error": "invalid_grant", "error_description": "Refresh token reused"})),
        served_json(200, json!({})),
    ])
    .await;
    let flow = GrokOAuth { issuer: server.base_url.clone() };
    let old = Credential::Oauth {
        access: "access-1".into(),
        refresh: "refresh-1".into(),
        expires: 0,
        account_id: None,
        extra: Default::default(),
    };
    let refreshed = flow.refresh(old.clone(), CancellationToken::new()).await.expect("refreshed");
    assert!(matches!(&refreshed, Credential::Oauth { access, refresh, .. } if access == "access-2" && refresh == "refresh-2"));
    let refused = flow.refresh(old, CancellationToken::new()).await.expect_err("refused");
    assert!(refused.is_refused(), "{refused}");
    assert!(!refused.to_string().contains("refresh-1"), "{refused}");
    flow.revoke(&refreshed).await.expect("revoked");

    let requests = server.requests.lock().clone();
    assert_eq!(field(&requests[0].raw, "grant_type").as_deref(), Some("refresh_token"));
    assert_eq!(field(&requests[0].raw, "refresh_token").as_deref(), Some("refresh-1"));
    assert_eq!(requests[2].path, "/oauth2/revoke");
    assert_eq!(field(&requests[2].raw, "token").as_deref(), Some("refresh-2"));
    assert_eq!(field(&requests[2].raw, "token_type_hint").as_deref(), Some("refresh_token"));
}

fn model(base_url: &str) -> factory_agent::types::Model {
    let mut model = factory_agent::providers::catalog::models("grok").into_iter().find(|model| model.id == "grok-4.7").expect("Grok 4.7");
    model.base_url = base_url.to_owned();
    model
}

fn hello() -> LlmContext {
    LlmContext { system_prompt: Some("Be brief.".into()), messages: vec![Message::User(UserMessage::text("hi", 1))], tools: Vec::new() }
}

fn sse(events: &[Value]) -> Served {
    let body: String =
        events.iter().map(|event| format!("event: {}\ndata: {event}\n\n", event["type"].as_str().unwrap_or_default())).collect();
    Served { status: 200, content_type: "text/event-stream", chunks: vec![body], hold: false, hold_before_headers: false }
}

/// Requests go to the proxy's Responses API with the sign-in's token, as
/// the tool's sign-in at the tool's one fixed version, routed by the model,
/// with none of OpenAI's session headers.
#[tokio::test]
async fn a_request_goes_through_the_proxy_as_the_tool() {
    let reply = || {
        sse(&[
            json!({"type": "response.created", "response": {"id": "r1", "status": "in_progress"}}),
            json!({"type": "response.output_item.added", "output_index": 0, "item": {"type": "message", "id": "m1", "role": "assistant", "status": "in_progress", "content": []}}),
            json!({"type": "response.output_text.delta", "output_index": 0, "delta": "hi from grok"}),
            json!({"type": "response.output_item.done", "output_index": 0, "item": {"type": "message", "id": "m1", "role": "assistant", "status": "completed", "content": [{"type": "output_text", "text": "hi from grok", "annotations": []}]}}),
            json!({"type": "response.completed", "response": {"id": "r1", "status": "completed", "usage": {"input_tokens": 10, "output_tokens": 3, "total_tokens": 13}}}),
        ])
    };
    let server = serve(vec![reply(), reply(), reply()]).await;
    let model = model(&server.base_url);
    let auth = GrokOAuth::default()
        .to_auth(&Credential::Oauth {
            access: "access-1".into(),
            refresh: "r".into(),
            expires: u64::MAX,
            account_id: None,
            extra: Default::default(),
        })
        .expect("auth");
    let options =
        StreamOptions { api_key: auth.api_key, headers: auth.headers, session_id: Some("session-1".into()), ..StreamOptions::default() };
    let message = grok::stream_simple(&model, hello(), options).expect("stream").result().await.expect("an end");
    assert_eq!(message.stop_reason, StopReason::Stop, "{message:?}");

    let request = server.requests.lock()[0].clone();
    assert_eq!((request.method.as_str(), request.path.as_str()), ("POST", "/responses"));
    let header = |name: &str| request.headers.iter().find(|(key, _)| key == name).map(|(_, value)| value.clone());
    assert_eq!(header("authorization").as_deref(), Some("Bearer access-1"));
    assert_eq!(header("x-xai-token-auth").as_deref(), Some("xai-grok-cli"));
    assert_eq!(header("x-grok-client-version").as_deref(), Some(grok::CLIENT_VERSION));
    assert_eq!(header("x-grok-model-override").as_deref(), Some("grok-4.7"));
    assert_eq!((header("session_id"), header("x-client-request-id")), (None, None));
    assert_eq!(request.body["model"], "grok-4.7");
    assert_eq!(request.body["stream"], true);
    // The conversation: a random id, nothing of the agent's, the same while
    // its context goes on and a new one when it starts again.
    let conversation = header("x-grok-conv-id").expect("a conversation id");
    assert!(uuid::Uuid::parse_str(&conversation).is_ok() && !conversation.contains("session-1"), "{conversation}");
    assert_eq!(request.body["prompt_cache_key"], conversation.as_str(), "the agent's session goes no further");

    let options = || StreamOptions {
        api_key: Some("access-1".into()),
        headers: grok::proxy_headers(),
        session_id: Some("session-1".into()),
        ..StreamOptions::default()
    };
    let mut further = hello();
    further.messages.push(Message::User(UserMessage::text("and more", 2)));
    grok::stream_simple(&model, further, options()).expect("stream").result().await.expect("an end");
    let mut again = hello();
    again.messages[0] = Message::User(UserMessage::text("a summary of before", 3));
    grok::stream_simple(&model, again, options()).expect("stream").result().await.expect("an end");
    let conversations: Vec<String> = server
        .requests
        .lock()
        .iter()
        .map(|request| request.headers.iter().find(|(key, _)| key == "x-grok-conv-id").map(|(_, value)| value.clone()).unwrap_or_default())
        .collect();
    assert_eq!(conversations[1], conversation, "the same context");
    assert_ne!(conversations[2], conversation, "a context started again");
}

/// A Grok sign-in's token goes to the proxy alone: any other address,
/// xAI's API above all, is refused before anything is sent.
#[tokio::test]
async fn a_grok_token_goes_only_to_the_proxy() {
    assert!(grok::allowed("https://cli-chat-proxy.grok.com/v1"));
    for elsewhere in
        ["https://api.x.ai/v1", "http://cli-chat-proxy.grok.com/v1", "https://cli-chat-proxy.grok.com.evil.test/v1", "https://x.ai"]
    {
        assert!(!grok::allowed(elsewhere), "{elsewhere}");
        let options = StreamOptions { api_key: Some("access-1".into()), ..StreamOptions::default() };
        let refused = grok::stream_simple(&model(elsewhere), hello(), options).expect_err("refused before sending");
        assert!(refused.to_string().contains("used only with Grok's own service"), "{refused}");
        let auth = factory_agent::auth::RequestAuth { api_key: Some("access-1".into()), ..Default::default() };
        assert!(factory_agent::limits::grok(elsewhere.to_owned(), auth.clone()).await.is_err());
        assert!(factory_agent::providers::listing::fetch("grok", elsewhere, &auth).await.is_err());
    }
}

/// The proxy turning the client away (426, 403) is xAI's refusal, said as
/// such: a failure no wait or retry changes.
#[tokio::test]
async fn the_proxy_turning_the_tool_away_is_a_refusal() {
    let said = "Your Grok CLI version (none) is outdated. Please update to version 1.0.13 or later.";
    let server = serve(vec![served_json(426, json!({"error": said}))]).await;
    let options = StreamOptions { api_key: Some("access-1".into()), ..StreamOptions::default() };
    let error = grok::stream_simple(&model(&server.base_url), hello(), options).expect("stream").result().await.expect("an end");
    assert_eq!(error.stop_reason, StopReason::Error, "{error:?}");
    let message = error.error_message.clone().unwrap_or_default();
    assert_eq!(message, format!("xAI refused this sign-in (426): {said}"));
    assert!(!factory_agent::guard::recoverable_response(&message), "never retried");
}

/// The plan's usage used up (402) is a usage limit, said plainly; any other
/// 4xx but an expired token or too many requests is xAI's refusal; those,
/// and 5xx, are left to the usual renewal and waits.
#[tokio::test]
async fn a_used_up_plan_is_a_usage_limit_and_other_refusals_stop() {
    let used_up = "Grok Build usage balance exhausted";
    let server = serve(vec![
        served_json(402, json!({"error": used_up})),
        served_json(400, json!({"error": {"message": "Unknown parameter: store"}})),
        served_json(429, json!({"error": "Too many requests"})),
    ])
    .await;
    let ask = || {
        let options = StreamOptions { api_key: Some("access-1".into()), ..StreamOptions::default() };
        grok::stream_simple(&model(&server.base_url), hello(), options).expect("stream").result()
    };
    let error = ask().await.expect("an end");
    assert_eq!(error.error_message.as_deref(), Some(format!("Your Grok usage is used up for now (402): {used_up}").as_str()));
    assert_eq!(factory_agent::accounts::usage_limit(&error), Some(None), "a usage limit, its reset read from the meter");
    assert!(!factory_agent::accounts::out_of_credit(&error), "not a key out of credit");

    let error = ask().await.expect("an end");
    let message = error.error_message.clone().unwrap_or_default();
    assert_eq!(message, "xAI refused this sign-in (400): Unknown parameter: store");
    assert!(!factory_agent::guard::recoverable_response(&message));

    let error = ask().await.expect("an end");
    assert!(error.error_message.as_deref().is_some_and(|message| !message.starts_with("xAI refused")), "{error:?}");
}

/// The plan meter: the period's usage and when it resets, read with the
/// sign-in as the tool reads it.
#[tokio::test]
async fn the_plan_meter_is_the_accounts_usage() {
    let meter = json!({"config": {
        "currentPeriod": {"type": "USAGE_PERIOD_TYPE_WEEKLY", "start": "2026-11-20T10:00:00+00:00", "end": "2026-11-27T10:00:00+00:00"},
        "creditUsagePercent": 42.5,
        "productUsage": [{"product": "GrokBuild", "usagePercent": 42.5}],
    }});
    let server = serve(vec![served_json(200, meter)]).await;
    let auth = GrokOAuth::default()
        .to_auth(&Credential::Oauth {
            access: "access-1".into(),
            refresh: "r".into(),
            expires: u64::MAX,
            account_id: None,
            extra: Default::default(),
        })
        .expect("auth");
    let usage = factory_agent::limits::grok(server.base_url.clone(), auth).await.expect("usage");
    assert_eq!(usage.limits.len(), 1);
    let weekly = &usage.limits[0];
    assert_eq!((weekly.id.as_str(), weekly.label.as_str(), weekly.used_percent), ("weekly", "Weekly", 42.5));
    assert_eq!(weekly.resets_at, factory_agent::clock::parse_iso_ms("2026-11-27T10:00:00+00:00").expect("a time"));
    let request = server.requests.lock()[0].clone();
    assert_eq!((request.method.as_str(), request.path.as_str()), ("GET", "/billing?format=credits"));
    let header = |name: &str| request.headers.iter().find(|(key, _)| key == name).map(|(_, value)| value.clone());
    assert_eq!(header("authorization").as_deref(), Some("Bearer access-1"));
    assert_eq!(header("x-grok-client-version").as_deref(), Some(grok::CLIENT_VERSION));
    assert_eq!(factory_agent::limits::parse_grok(&json!({"config": {}})), Vec::new());
}

/// Who a sign-in is, from its token: shown as Grok and the end of its user
/// id (no email is asked for), and the plan its token names.
#[test]
fn a_sign_in_is_known_by_its_user_and_plan() {
    let access = token(json!({"sub": "user-0000abcd", "tier": "SuperGrok"}));
    let identity = grok::identity_of(&access);
    assert_eq!((identity.key.as_str(), identity.label.as_str()), ("user-0000abcd", "Grok …abcd"));
    assert_eq!(grok::plan_of(&access).as_deref(), Some("SuperGrok"));
    assert_eq!(grok::identity_of("not a token").label, "Grok");
    assert_eq!(grok::plan_of("not a token"), None);
}

/// The proxy's list: its models, with their context and efforts; a new one
/// is made from the closest it has.
#[test]
fn the_proxy_lists_its_models() {
    let efforts = json!([{"id": "high", "value": "high"}, {"id": "medium", "value": "medium"}, {"id": "low", "value": "low"}]);
    let listing = json!({"object": "list", "data": [
        {"id": "grok-4.7", "name": "Grok 4.7", "context_window": 500000, "reasoning_efforts": efforts},
        {"id": "grok-5", "name": "Grok 5", "context_window": 256000, "reasoning_efforts": efforts},
    ]});
    let models = factory_agent::providers::listing::parse("grok", listing.to_string().as_bytes()).expect("Grok's list");
    let grok_47 = models.iter().find(|model| model.id == "grok-4.7").expect("4.7");
    assert_eq!(grok_47.context_window, 500_000);
    let levels = factory_agent::providers::supported_thinking_levels(grok_47);
    assert!(
        !levels.contains(&factory_agent::stream::ThinkingLevel::Off) && !levels.contains(&factory_agent::stream::ThinkingLevel::Xhigh),
        "{levels:?}"
    );
    let grok_5 = models.iter().find(|model| model.id == "grok-5").expect("a new one");
    assert_eq!((grok_5.name.as_str(), grok_5.provider.as_str(), grok_5.api.as_str()), ("Grok 5", "grok", "openai-responses"));
}
