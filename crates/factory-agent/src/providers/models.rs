use std::path::PathBuf;
use std::sync::Arc;

use serde_json::Map;
use tokio_util::sync::CancellationToken;

use crate::auth::{AuthCheck, AuthInteraction, AuthResult, Env, RequestAuth, process_env, resolve_provider_auth};
use crate::clock::now_ms;
use crate::credentials::{Credential, FileCredentialStore};
use crate::errors::{Error, models_error};
use crate::limits::UsageLimit;
use crate::providers::{Provider, anthropic, merge_headers, model_headers, openai, openai_codex, openrouter, supported_thinking_levels};
use crate::stream::{EventStream, LlmContext, StreamFn, StreamOptions, ThinkingLevel, event_stream};
use crate::types::{AssistantMessage, Model, StopReason, StreamEvent, Usage};

fn credential_result<T>(result: Result<T, Error>, operation: &str, id: &str) -> Result<T, Error> {
    result.map_err(|error| models_error("auth", format!("Credential store {operation} failed for {id}"), Some(&error)))
}

/// `config.ts` registers the four providers in this order.
pub fn default_providers() -> Vec<Provider> {
    vec![anthropic::provider(), openai::provider(), openai_codex::provider(), openrouter::provider()]
}

/// A model an agent can use, as a client lists it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModelInfo {
    /// `provider/model-id`, the form `Registry::resolve` takes.
    pub id: String,
    pub name: String,
    pub context_window: u64,
    /// Reasoning efforts the model accepts, weakest first. A model without reasoning accepts only `Off`.
    pub efforts: Vec<ThinkingLevel>,
}

struct Inner {
    providers: Vec<Provider>,
    store: FileCredentialStore,
    env: Env,
}

#[derive(Clone)]
pub struct Registry {
    inner: Arc<Inner>,
}

impl Registry {
    /// The four providers, with credentials stored at `auth_path` (created 0600 on first login) or taken from the environment.
    pub fn new(auth_path: PathBuf) -> Registry {
        Registry::with_providers(FileCredentialStore::new(auth_path), process_env(), default_providers())
    }

    pub fn with_providers(store: FileCredentialStore, env: Env, providers: Vec<Provider>) -> Registry {
        Registry { inner: Arc::new(Inner { providers, store, env }) }
    }

    pub fn providers(&self) -> &[Provider] {
        &self.inner.providers
    }

    pub fn provider(&self, id: &str) -> Option<&Provider> {
        self.inner.providers.iter().find(|provider| provider.id == id)
    }

    pub fn get_models(&self, provider: Option<&str>) -> Vec<Model> {
        match provider {
            Some(id) => self.provider(id).map(|provider| provider.models.clone()).unwrap_or_default(),
            None => self.inner.providers.iter().flat_map(|provider| provider.models.iter().cloned()).collect(),
        }
    }

    pub fn get_model(&self, provider: &str, id: &str) -> Option<Model> {
        self.get_models(Some(provider)).into_iter().find(|model| model.id == id)
    }

    /// A model by `provider/model-id`; the model id may itself contain slashes.
    pub fn resolve(&self, id: &str) -> Option<Model> {
        let (provider, model) = id.split_once('/')?;
        self.get_model(provider, model)
    }

    /// The models of every provider with usable credentials, stored or in the environment, in catalog order.
    /// Reads the credential file and environment only; nothing is refreshed or sent.
    pub async fn models(&self) -> Result<Vec<ModelInfo>, Error> {
        let mut models = Vec::new();
        for provider in &self.inner.providers {
            if self.check_auth(provider.id).await?.is_none() {
                continue;
            }
            for model in &provider.models {
                models.push(ModelInfo {
                    id: format!("{}/{}", provider.id, model.id),
                    name: model.name.clone(),
                    context_window: model.context_window,
                    efforts: supported_thinking_levels(model),
                });
            }
        }
        Ok(models)
    }

    /// The stored credential's type and source, without touching the network except for a due refresh.
    pub async fn check_auth(&self, id: &str) -> Result<Option<AuthCheck>, Error> {
        let Some(provider) = self.provider(id) else { return Ok(None) };
        if let Some(Credential::Oauth { .. }) = credential_result(self.inner.store.read(id).await, "read", id)? {
            return Ok(provider.oauth.as_ref().map(|_| AuthCheck { source: Some("OAuth".into()), kind: "oauth" }));
        }
        if provider.api_key.is_none() {
            return Ok(None);
        }
        let resolution = resolve_provider_auth(provider, &self.inner.store, &self.inner.env, &CancellationToken::new()).await?;
        Ok(resolution.map(|result| AuthCheck { source: Some(result.source), kind: "api_key" }))
    }

    /// The usage limits of a provider signed in with a subscription (Claude Pro/Max, ChatGPT), read from the
    /// provider; None when it isn't signed in that way or has no limits to read. Refreshes a token due for it.
    pub async fn usage_limits(&self, id: &str) -> Result<Option<Vec<UsageLimit>>, Error> {
        let Some(provider) = self.provider(id) else { return Ok(None) };
        let Some(limits) = provider.limits else { return Ok(None) };
        let Some((base_url, auth)) = self.subscription_auth(provider).await? else { return Ok(None) };
        limits(base_url, auth).await.map(Some)
    }

    /// The plan of a provider signed in with a subscription, such as "Claude Max 20x", read from the provider;
    /// None when it isn't signed in that way or can't say, and empty when it didn't say. Refreshes a token due for it.
    pub async fn plan(&self, id: &str) -> Result<Option<String>, Error> {
        let Some(provider) = self.provider(id) else { return Ok(None) };
        let Some(plan) = provider.plan else { return Ok(None) };
        let Some((base_url, auth)) = self.subscription_auth(provider).await? else { return Ok(None) };
        plan(base_url, auth).await.map(Some)
    }

    /// The base url and auth of a provider signed in with a subscription; None when it isn't.
    async fn subscription_auth(&self, provider: &Provider) -> Result<Option<(String, RequestAuth)>, Error> {
        let Some(oauth) = &provider.oauth else { return Ok(None) };
        if !oauth.is_subscription()
            || !matches!(credential_result(self.inner.store.read(provider.id).await, "read", provider.id)?, Some(Credential::Oauth { .. }))
        {
            return Ok(None);
        }
        let resolution = resolve_provider_auth(provider, &self.inner.store, &self.inner.env, &CancellationToken::new()).await?;
        let Some(resolution) = resolution else { return Ok(None) };
        let base_url = resolution.auth.base_url.clone().unwrap_or_else(|| provider.base_url.to_owned());
        Ok(Some((base_url, resolution.auth)))
    }

    pub async fn get_auth(&self, model: &Model) -> Result<Option<AuthResult>, Error> {
        let Some(provider) = self.provider(&model.provider) else { return Ok(None) };
        let Some(mut result) = resolve_provider_auth(provider, &self.inner.store, &self.inner.env, &CancellationToken::new()).await? else {
            return Ok(None);
        };
        if model.headers.is_some() {
            result.auth.headers = merge_headers(&result.auth.headers, &model_headers(model));
        }
        Ok(Some(result))
    }

    pub async fn login(&self, id: &str, kind: &str, interaction: &dyn AuthInteraction) -> Result<Credential, Error> {
        let provider = self.provider(id).ok_or_else(|| models_error("provider", format!("Unknown provider: {id}"), None))?;
        let login = match kind {
            "oauth" => provider.oauth.as_ref().map(|oauth| oauth.login(interaction)),
            _ => provider.api_key.as_ref().map(|api_key| api_key.login(interaction)),
        };
        let Some(login) = login else {
            return Err(models_error("auth", format!("{} does not support {kind} login", provider.name), None));
        };
        let cancel = interaction.cancel();
        let credential = tokio::select! {
            biased;
            () = cancel.cancelled() => return Err(Error::Aborted),
            credential = login => credential?,
        };
        let stored = credential.clone();
        self.inner
            .store
            .modify(id, |_| async move { Ok(Some(stored)) })
            .await
            .map_err(|error| models_error("auth", format!("Credential store modify failed for {id}"), Some(&error)))?;
        Ok(credential)
    }

    pub async fn logout(&self, id: &str) -> Result<(), Error> {
        credential_result(self.inner.store.delete(id).await, "delete", id)
    }

    async fn open(&self, model: &Model, context: LlmContext, options: StreamOptions) -> Result<EventStream, Error> {
        let provider = self
            .provider(&model.provider)
            .ok_or_else(|| models_error("provider", format!("Unknown provider: {}", model.provider), None))?;
        let resolution = self
            .get_auth(model)
            .await?
            .ok_or_else(|| models_error("auth", format!("Provider is not configured: {}", model.provider), None))?;
        let api = provider.api.as_ref().ok_or_else(|| {
            models_error("stream", format!("Provider {} has no API implementation for \"{}\"", provider.id, model.api), None)
        })?;
        let mut request_model = model.clone();
        if let Some(base_url) = options.base_url.as_ref().or(resolution.auth.base_url.as_ref()) {
            request_model.base_url.clone_from(base_url);
        }
        let request_options = StreamOptions {
            api_key: options.api_key.or(resolution.auth.api_key),
            headers: merge_headers(&resolution.auth.headers, &options.headers),
            ..options
        };
        api.stream_simple(&request_model, context, request_options)
    }

    /// Streams through the model's provider. Credentials resolve per request: an OAuth token close to expiry is
    /// refreshed and the rotated credential written back to the auth file before the request goes out.
    /// The stream exists at once; a setup failure ends it with an error event.
    pub fn stream_fn(&self) -> StreamFn {
        let registry = self.clone();
        Arc::new(move |model, context, options| {
            let (sink, stream) = event_stream();
            let (registry, model) = (registry.clone(), model.clone());
            tokio::spawn(async move {
                match registry.open(&model, context, options).await {
                    Ok(mut inner) => {
                        while let Some(event) = inner.next().await {
                            sink.push(event);
                        }
                    }
                    Err(error) => {
                        let message = AssistantMessage {
                            content: Vec::new(),
                            api: model.api.clone(),
                            provider: model.provider.clone(),
                            model: model.id.clone(),
                            usage: Usage::empty(),
                            stop_reason: StopReason::Error,
                            error_message: Some(error.to_string()),
                            timestamp: now_ms(),
                            extra: Map::new(),
                        };
                        sink.push(StreamEvent::Error { reason: StopReason::Error, error: message });
                    }
                }
            });
            Ok(stream)
        })
    }
}
