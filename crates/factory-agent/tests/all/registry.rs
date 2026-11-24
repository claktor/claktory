use std::sync::Arc;

use crate::support::{luna, scripted_model};
use factory_agent::auth::{AuthCheck, AuthEvent, AuthFuture, AuthInteraction, AuthPrompt, Env, EnvApiKeyAuth, OAuthFlow, RequestAuth};
use factory_agent::clock::now_ms;
use factory_agent::credentials::{Credential, FileCredentialStore};
use factory_agent::errors::Error;
use factory_agent::providers::models::{Registry, default_providers};
use factory_agent::providers::{Api, Provider};
use factory_agent::stream::{EventStream, LlmContext, StreamOptions};
use factory_agent::types::{AssistantPart, Message, Model, StopReason, StreamEvent, UserMessage};
use parking_lot::Mutex;
use serde_json::json;
use tokio_util::sync::CancellationToken;

fn env(pairs: &[(&str, &str)]) -> Env {
    let pairs: Vec<(String, String)> = pairs.iter().map(|(name, value)| ((*name).to_owned(), (*value).to_owned())).collect();
    Arc::new(move |name| pairs.iter().find(|(key, _)| key == name).map(|(_, value)| value.clone()))
}

fn registry(dir: &std::path::Path, pairs: &[(&str, &str)]) -> Registry {
    Registry::with_providers(FileCredentialStore::new(dir.join("auth.json")), env(pairs), default_providers())
}

fn stored(dir: &std::path::Path, provider: &str, credential: serde_json::Value) {
    std::fs::write(dir.join("auth.json"), json!({provider: credential}).to_string()).expect("auth file");
}

struct Recording {
    seen: Arc<Mutex<Vec<StreamOptions>>>,
}

impl Api for Recording {
    fn stream_simple(&self, model: &Model, context: LlmContext, options: StreamOptions) -> Result<EventStream, Error> {
        self.seen.lock().push(options.clone());
        scripted_model()(model, context, options)
    }
}

struct FakeOAuth {
    refreshes: Arc<Mutex<u32>>,
    fail: bool,
}

impl OAuthFlow for FakeOAuth {
    fn name(&self) -> &str {
        "Fake (subscription)"
    }

    fn is_subscription(&self) -> bool {
        true
    }

    fn login<'a>(&'a self, _interaction: &'a dyn AuthInteraction) -> AuthFuture<'a, Result<Credential, Error>> {
        Box::pin(async {
            Ok(Credential::Oauth {
                access: "login-access".into(),
                refresh: "login-refresh".into(),
                expires: now_ms() + 3_600_000,
                account_id: None,
                extra: Default::default(),
            })
        })
    }

    fn refresh<'a>(&'a self, credential: Credential, _cancel: CancellationToken) -> AuthFuture<'a, Result<Credential, Error>> {
        *self.refreshes.lock() += 1;
        let fail = self.fail;
        Box::pin(async move {
            if fail {
                return Err(Error::Other("boom".into()));
            }
            let Credential::Oauth { refresh, account_id, extra, .. } = credential else { return Err(Error::Other("not oauth".into())) };
            Ok(Credential::Oauth { access: "refreshed-access".into(), refresh, expires: now_ms() + 3_600_000, account_id, extra })
        })
    }

    fn to_auth(&self, credential: &Credential) -> Result<RequestAuth, Error> {
        let Credential::Oauth { access, .. } = credential else { return Err(Error::Other("not oauth".into())) };
        Ok(RequestAuth {
            api_key: Some(access.clone()),
            headers: vec![("x-account".into(), "acct".into())],
            base_url: Some("http://fake.local/v1".into()),
        })
    }
}

fn fake_provider(oauth: Option<Arc<dyn OAuthFlow>>, api: Option<Arc<dyn Api>>) -> Provider {
    let mut model = luna();
    model.provider = "fake".into();
    model.api = "fake-api".into();
    Provider {
        id: "fake",
        name: "Fake",
        base_url: "https://fake.example",
        api_key: Some(Arc::new(EnvApiKeyAuth { name: "Fake API key", env_vars: &["FAKE_API_KEY"], base_url_var: None })),
        oauth,
        models: vec![model],
        api,
        limits: None,
        plan: None,
        identity: None,
        reset: None,
        check_key: None,
        credit: None,
        images: None,
    }
}

struct Typing {
    answer: String,
    prompts: Arc<Mutex<Vec<AuthPrompt>>>,
}

impl AuthInteraction for Typing {
    fn prompt<'a>(&'a self, prompt: AuthPrompt) -> AuthFuture<'a, Result<String, Error>> {
        self.prompts.lock().push(prompt);
        let answer = self.answer.clone();
        Box::pin(async move { Ok(answer) })
    }

    fn notify(&self, _event: AuthEvent) {}

    fn cancel(&self) -> CancellationToken {
        CancellationToken::new()
    }
}

fn catalog_luna() -> Model {
    let mut model = luna();
    model.thinking_level_map = Some(json!({"xhigh": "xhigh", "max": "max", "minimal": "low"}).as_object().expect("map").clone());
    model.compat = Some(
        json!({"supportsOpenAIGrammarTools": true, "supportsAdditionalTools": true, "supportsToolSearch": true})
            .as_object()
            .expect("map")
            .clone(),
    );
    model
}

#[tokio::test]
async fn the_registry_lists_the_four_providers_and_their_models() {
    let dir = tempfile::tempdir().unwrap();
    let registry = registry(dir.path(), &[]);
    let ids: Vec<&str> = registry.providers().iter().map(|provider| provider.id).collect();
    assert_eq!(ids, ["anthropic", "openai", "openai-codex", "openrouter"]);
    assert_eq!(registry.get_model("openai", "gpt-5.5").map(|model| model.max_tokens), Some(128_000));
    assert_eq!(registry.get_model("openai-codex", "gpt-5.6-luna"), Some(catalog_luna()));
    assert!(registry.get_model("openai", "nope").is_none());
    assert!(registry.get_models(Some("nope")).is_empty());
    assert_eq!(registry.get_models(None).len(), 38 + 8 + 356 + 15);
    let codex = registry.provider("openai-codex").unwrap();
    assert_eq!(codex.oauth.as_ref().map(|oauth| oauth.name()), Some("OpenAI (ChatGPT Plus/Pro)"));
    assert!(codex.api.is_some() && codex.api_key.is_none());
    let openrouter = registry.provider("openrouter").unwrap();
    assert_eq!(openrouter.oauth.as_ref().map(|oauth| oauth.name()), Some("OpenRouter OAuth"));
    assert_eq!(openrouter.api_key.as_ref().map(|key| key.name().to_owned()).as_deref(), Some("OpenRouter API key"));
    let anthropic = registry.provider("anthropic").unwrap();
    assert_eq!(anthropic.oauth.as_ref().map(|oauth| oauth.name()), Some("Anthropic (Claude Pro/Max)"));
    assert_eq!(anthropic.api_key.as_ref().map(|key| key.name().to_owned()).as_deref(), Some("Anthropic API key"));
    assert!(registry.providers().iter().all(|provider| provider.api.is_some()));
}

#[tokio::test]
async fn a_stored_key_wins_over_the_environment_and_a_stored_oauth_owns_the_provider() {
    let dir = tempfile::tempdir().unwrap();
    let registry = registry(dir.path(), &[("OPENAI_API_KEY", "env-key")]);
    let model = registry.get_model("openai", "gpt-5.5").unwrap();
    assert_eq!(registry.check_auth("openai").await.unwrap(), Some(AuthCheck { source: Some("OPENAI_API_KEY".into()), kind: "api_key" }));
    assert_eq!(registry.get_auth(&model).await.unwrap().unwrap().auth.api_key.as_deref(), Some("env-key"));
    stored(dir.path(), "openai", json!({"type": "api_key", "key": "stored-key"}));
    assert_eq!(registry.check_auth("openai").await.unwrap(), Some(AuthCheck { source: Some("stored credential".into()), kind: "api_key" }));
    assert_eq!(registry.get_auth(&model).await.unwrap().unwrap().auth.api_key.as_deref(), Some("stored-key"));
    stored(dir.path(), "openai", json!({"type": "oauth", "access": "a", "refresh": "r", "expires": 0}));
    assert_eq!(registry.check_auth("openai").await.unwrap(), None);
    assert_eq!(registry.get_auth(&model).await.unwrap(), None);
    assert_eq!(registry.check_auth("ghost").await.unwrap(), None);
    let empty = Registry::with_providers(FileCredentialStore::new(dir.path().join("none.json")), env(&[]), default_providers());
    assert_eq!(empty.check_auth("openai").await.unwrap(), None);
    assert_eq!(empty.check_auth("openai-codex").await.unwrap(), None);
    std::fs::write(dir.path().join("auth.json"), "{not json").unwrap();
    let error = registry.check_auth("openai").await.unwrap_err().to_string();
    assert!(error.starts_with("Credential store read failed for openai: "), "{error}");
}

#[tokio::test]
async fn an_expiring_oauth_credential_is_refreshed_once_and_written_back() {
    let dir = tempfile::tempdir().unwrap();
    let refreshes = Arc::new(Mutex::new(0));
    let oauth: Arc<dyn OAuthFlow> = Arc::new(FakeOAuth { refreshes: refreshes.clone(), fail: false });
    let registry =
        Registry::with_providers(FileCredentialStore::new(dir.path().join("auth.json")), env(&[]), vec![fake_provider(Some(oauth), None)]);
    let model = registry.get_model("fake", "gpt-5.6-luna").unwrap();
    stored(dir.path(), "fake", json!({"type": "oauth", "access": "fresh", "refresh": "r", "expires": now_ms() + 3_600_000}));
    let result = registry.get_auth(&model).await.unwrap().unwrap();
    assert_eq!(
        (result.auth.api_key.as_deref(), result.source.as_str(), result.auth.base_url.as_deref()),
        (Some("fresh"), "OAuth", Some("http://fake.local/v1"))
    );
    assert_eq!(registry.check_auth("fake").await.unwrap(), Some(AuthCheck { source: Some("OAuth".into()), kind: "oauth" }));
    assert_eq!(*refreshes.lock(), 0);
    stored(
        dir.path(),
        "fake",
        json!({"type": "oauth", "access": "stale", "refresh": "r", "expires": now_ms() + 60_000, "accountId": "acct-1"}),
    );
    let result = registry.get_auth(&model).await.unwrap().unwrap();
    assert_eq!(result.auth.api_key.as_deref(), Some("refreshed-access"));
    assert_eq!(*refreshes.lock(), 1);
    let on_disk: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(dir.path().join("auth.json")).unwrap()).unwrap();
    assert_eq!(on_disk["fake"]["access"], "refreshed-access");
    assert_eq!(on_disk["fake"]["accountId"], "acct-1");
    assert!(on_disk["fake"]["expires"].as_u64().unwrap() > now_ms() + 3_000_000);
    registry.get_auth(&model).await.unwrap().unwrap();
    assert_eq!(*refreshes.lock(), 1);
    let failing: Arc<dyn OAuthFlow> = Arc::new(FakeOAuth { refreshes: refreshes.clone(), fail: true });
    let registry = Registry::with_providers(
        FileCredentialStore::new(dir.path().join("auth.json")),
        env(&[]),
        vec![fake_provider(Some(failing), None)],
    );
    stored(dir.path(), "fake", json!({"type": "oauth", "access": "stale", "refresh": "r", "expires": 0}));
    assert_eq!(registry.get_auth(&model).await.unwrap_err().to_string(), "OAuth refresh failed for fake: boom");
    let on_disk: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(dir.path().join("auth.json")).unwrap()).unwrap();
    assert_eq!(on_disk["fake"]["access"], "stale");
}

#[tokio::test]
async fn login_stores_the_credential_and_logout_removes_it() {
    let dir = tempfile::tempdir().unwrap();
    // OpenAI, which accepts the key when asked.
    let server = crate::support::server::serve(vec![crate::support::server::served_json(200, json!({"data": []}))]).await;
    let registry = registry(dir.path(), &[("OPENAI_BASE_URL", &server.base_url)]);
    let prompts = Arc::new(Mutex::new(Vec::new()));
    let typing = Typing { answer: "typed-key".into(), prompts: prompts.clone() };
    let credential = registry.login("openai", "api_key", &typing).await.unwrap();
    assert_eq!(credential, Credential::ApiKey { key: "typed-key".into(), extra: Default::default() });
    let asked = server.requests.lock().clone();
    assert_eq!(asked.iter().map(|request| (request.method.as_str(), request.path.as_str())).collect::<Vec<_>>(), [("GET", "/models")]);
    assert!(asked[0].headers.contains(&("authorization".into(), "Bearer typed-key".into())), "{:?}", asked[0].headers);
    assert_eq!(prompts.lock().clone(), vec![AuthPrompt::Secret { message: "Enter OpenAI API key".into(), placeholder: None }]);
    assert_eq!(registry.check_auth("openai").await.unwrap(), Some(AuthCheck { source: Some("stored credential".into()), kind: "api_key" }));
    assert_eq!(registry.login("openai", "oauth", &typing).await.unwrap_err().to_string(), "OpenAI does not support oauth login");
    assert_eq!(registry.login("nope", "api_key", &typing).await.unwrap_err().to_string(), "Unknown provider: nope");
    registry.logout("openai").await.unwrap();
    assert_eq!(registry.check_auth("openai").await.unwrap(), None);
    registry.logout("openai").await.unwrap();
    let oauth: Arc<dyn OAuthFlow> = Arc::new(FakeOAuth { refreshes: Arc::new(Mutex::new(0)), fail: false });
    let fake =
        Registry::with_providers(FileCredentialStore::new(dir.path().join("fake.json")), env(&[]), vec![fake_provider(Some(oauth), None)]);
    let credential = fake.login("fake", "oauth", &typing).await.unwrap();
    assert!(matches!(credential, Credential::Oauth { ref access, .. } if access == "login-access"));
    assert_eq!(fake.check_auth("fake").await.unwrap(), Some(AuthCheck { source: Some("OAuth".into()), kind: "oauth" }));
}

#[tokio::test]
async fn stream_fn_resolves_auth_lazily_and_reports_setup_failures_as_error_events() {
    let dir = tempfile::tempdir().unwrap();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let api: Arc<dyn Api> = Arc::new(Recording { seen: seen.clone() });
    let oauth: Arc<dyn OAuthFlow> = Arc::new(FakeOAuth { refreshes: Arc::new(Mutex::new(0)), fail: false });
    let mut with_headers = fake_provider(Some(oauth.clone()), Some(api.clone()));
    with_headers.models[0].headers = Some(json!({"X-Account": "from-model", "extra": "1"}).as_object().unwrap().clone());
    let mut unimplemented = fake_provider(Some(oauth), None);
    unimplemented.id = "pending";
    unimplemented.models[0].provider = "pending".into();
    let registry =
        Registry::with_providers(FileCredentialStore::new(dir.path().join("auth.json")), env(&[]), vec![with_headers, unimplemented]);
    let stream = registry.stream_fn();
    let context =
        LlmContext { system_prompt: Some("x".into()), messages: vec![Message::User(UserMessage::text("hi", 1))], tools: Vec::new() };
    let model = registry.get_model("fake", "gpt-5.6-luna").unwrap();
    let result = stream(&model, context.clone(), StreamOptions::default()).unwrap().result().await.unwrap();
    assert_eq!(result.stop_reason, StopReason::Error);
    // Signed in to no account: the request fails marked as having none
    // left, which an agent waits out until a sign-in.
    assert_eq!(result.error_message.as_deref(), Some("You're signed out of Fake."));
    assert_eq!(factory_agent::accounts::none_left(&result), Some(None));
    assert!(result.content.is_empty());
    stored(dir.path(), "fake", json!({"type": "oauth", "access": "tok", "refresh": "r", "expires": now_ms() + 3_600_000}));
    let options = StreamOptions { session_id: Some("s".into()), headers: vec![("extra".into(), "2".into())], ..StreamOptions::default() };
    let result = stream(&model, context.clone(), options).unwrap().result().await.unwrap();
    assert_eq!(result.stop_reason, StopReason::ToolUse);
    assert!(matches!(result.content.first(), Some(AssistantPart::ToolCall { .. })));
    let options = seen.lock().last().cloned().unwrap();
    assert_eq!(options.api_key.as_deref(), Some("tok"));
    assert_eq!(options.session_id.as_deref(), Some("s"));
    assert_eq!(options.headers, vec![("X-Account".to_owned(), "from-model".to_owned()), ("extra".to_owned(), "2".to_owned())]);
    let mut pending = model.clone();
    pending.provider = "pending".into();
    stored(dir.path(), "pending", json!({"type": "oauth", "access": "tok", "refresh": "r", "expires": now_ms() + 3_600_000}));
    let result = stream(&pending, context.clone(), StreamOptions::default()).unwrap().result().await.unwrap();
    assert_eq!(result.error_message.as_deref(), Some("Provider pending has no API implementation for \"fake-api\""));
    let mut ghost = model;
    ghost.provider = "ghost".into();
    let mut events = stream(&ghost, context, StreamOptions::default()).unwrap();
    let Some(StreamEvent::Error { error, .. }) = events.next().await else { panic!("error event") };
    assert_eq!(error.error_message.as_deref(), Some("Unknown provider: ghost"));
    assert!(events.next().await.is_none());
}

#[tokio::test]
async fn the_anthropic_auth_token_reaches_the_request_as_a_bearer_header() {
    let dir = tempfile::tempdir().unwrap();
    let registry = registry(dir.path(), &[("ANTHROPIC_AUTH_TOKEN", "env-token")]);
    assert_eq!(
        registry.check_auth("anthropic").await.unwrap(),
        Some(AuthCheck { source: Some("ANTHROPIC_AUTH_TOKEN".into()), kind: "api_key" })
    );
    let body = "event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"id\":\"m\",\"model\":\"claude-fable-5\",\"usage\":{\"input_tokens\":1,\"output_tokens\":1}}}\n\nevent: message_delta\ndata: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\"},\"usage\":{\"output_tokens\":2}}\n\nevent: message_stop\ndata: {\"type\":\"message_stop\"}\n\n";
    let server = crate::support::server::serve(vec![crate::support::server::Served {
        status: 200,
        content_type: "text/event-stream",
        chunks: vec![body.into()],
        hold: false,
        hold_before_headers: false,
    }])
    .await;
    let mut model = registry.get_model("anthropic", "claude-fable-5").unwrap();
    model.base_url = server.base_url.clone();
    let context =
        LlmContext { system_prompt: Some("x".into()), messages: vec![Message::User(UserMessage::text("hi", 1))], tools: Vec::new() };
    let result = registry.stream_fn()(&model, context, StreamOptions::default()).unwrap().result().await.unwrap();
    assert_eq!(result.stop_reason, StopReason::Stop, "{:?}", result.error_message);
    let request = server.requests.lock()[0].clone();
    let header = |name: &str| request.headers.iter().find(|(key, _)| key == name).map(|(_, value)| value.clone());
    assert_eq!((header("authorization").as_deref(), header("x-api-key")), (Some("Bearer env-token"), None));
}

struct Waiting {
    cancel: CancellationToken,
}

impl AuthInteraction for Waiting {
    fn prompt<'a>(&'a self, _prompt: AuthPrompt) -> AuthFuture<'a, Result<String, Error>> {
        Box::pin(std::future::pending())
    }

    fn notify(&self, _event: AuthEvent) {}

    fn cancel(&self) -> CancellationToken {
        self.cancel.clone()
    }
}

struct CancelledFlow;

impl OAuthFlow for CancelledFlow {
    fn name(&self) -> &str {
        "Cancelled"
    }

    fn is_subscription(&self) -> bool {
        false
    }

    fn login<'a>(&'a self, interaction: &'a dyn AuthInteraction) -> AuthFuture<'a, Result<Credential, Error>> {
        Box::pin(async move {
            interaction.cancel().cancelled().await;
            Err(Error::Other("Login cancelled".into()))
        })
    }

    fn refresh<'a>(&'a self, credential: Credential, _cancel: CancellationToken) -> AuthFuture<'a, Result<Credential, Error>> {
        Box::pin(async move { Ok(credential) })
    }

    fn to_auth(&self, _credential: &Credential) -> Result<RequestAuth, Error> {
        Ok(RequestAuth::default())
    }
}

fn cancel_soon(cancel: CancellationToken) {
    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        cancel.cancel();
    });
}

#[tokio::test]
async fn a_cancelled_login_reports_the_abort_signal_text() {
    let dir = tempfile::tempdir().unwrap();
    let registry = registry(dir.path(), &[]);
    let waiting = Waiting { cancel: CancellationToken::new() };
    cancel_soon(waiting.cancel.clone());
    let login = tokio::time::timeout(crate::support::server::HANG, registry.login("openai", "api_key", &waiting))
        .await
        .expect("the cancel ends the login");
    assert_eq!(login.unwrap_err().to_string(), "This operation was aborted");
    let flow: Arc<dyn OAuthFlow> = Arc::new(CancelledFlow);
    let provider = Provider {
        id: "fake",
        name: "Fake",
        base_url: "https://fake.example",
        api_key: None,
        oauth: Some(flow),
        models: Vec::new(),
        api: None,
        limits: None,
        plan: None,
        identity: None,
        reset: None,
        check_key: None,
        credit: None,
        images: None,
    };
    let registry = Registry::with_providers(FileCredentialStore::new(dir.path().join("auth.json")), env(&[]), vec![provider]);
    let waiting = Waiting { cancel: CancellationToken::new() };
    cancel_soon(waiting.cancel.clone());
    let login = tokio::time::timeout(crate::support::server::HANG, registry.login("fake", "oauth", &waiting))
        .await
        .expect("the cancel ends the login");
    assert_eq!(login.unwrap_err().to_string(), "This operation was aborted");
    assert!(!dir.path().join("auth.json").exists());
}

#[tokio::test]
async fn models_lists_only_providers_with_usable_credentials_with_their_efforts_weakest_first() {
    let dir = tempfile::tempdir().unwrap();
    assert!(registry(dir.path(), &[]).models().await.unwrap().is_empty());
    stored(dir.path(), "anthropic", json!({"type": "oauth", "access": "a", "refresh": "r", "expires": now_ms() + 3_600_000}));
    let models = registry(dir.path(), &[("OPENROUTER_API_KEY", "key")]).models().await.unwrap();
    let mut providers: Vec<&str> = Vec::new();
    for model in &models {
        let provider = model.id.split_once('/').unwrap().0;
        if providers.last() != Some(&provider) {
            providers.push(provider);
        }
    }
    assert_eq!(providers, ["anthropic", "openrouter"]);
    let efforts = |id: &str| models.iter().find(|model| model.id == id).unwrap().efforts.clone();
    use factory_agent::ThinkingLevel::*;
    // Thinking can't be turned off; xhigh and max are opted into.
    assert_eq!(efforts("anthropic/claude-opus-5-5"), [Minimal, Low, Medium, High, Xhigh, Max]);
    assert_eq!(efforts("anthropic/claude-haiku-4-5"), [Off, Minimal, Low, Medium, High]);
    assert_eq!(efforts("openrouter/amazon/nova-lite-v1"), [Off]);
    let opus = models.iter().find(|model| model.id == "anthropic/claude-opus-5-5").unwrap();
    assert_eq!((opus.name.as_str(), opus.context_window), ("Claude Opus 5.5", 1_000_000));
}

#[tokio::test]
async fn resolve_takes_provider_slash_model_and_model_ids_may_contain_slashes() {
    let dir = tempfile::tempdir().unwrap();
    let registry = registry(dir.path(), &[]);
    let model = registry.resolve("openrouter/deepseek/deepseek-v4-flash-0731").unwrap();
    assert_eq!((model.provider.as_str(), model.id.as_str()), ("openrouter", "deepseek/deepseek-v4-flash-0731"));
    assert_eq!(registry.resolve("anthropic/claude-opus-5-5").map(|model| model.api), Some("anthropic-messages".into()));
    for unknown in ["claude-opus-5-5", "anthropic/nope", "nope/claude-opus-5-5", ""] {
        assert_eq!(registry.resolve(unknown), None, "{unknown}");
    }
}
