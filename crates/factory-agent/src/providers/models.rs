use std::path::PathBuf;
use std::sync::Arc;

use serde_json::Map;
use tokio_util::sync::CancellationToken;

use hashbrown::HashMap;

use parking_lot::Mutex;

use crate::accounts::{
    Refused, account_name, account_noun, exhausted_message, key_label, mark_answered_by, mark_none_left, mark_refused, out_of_credit,
    refused, switch_note, usage_limit,
};
use crate::auth::{AuthCheck, AuthInteraction, AuthResult, Env, RequestAuth, process_env, resolve_account_auth, resolve_provider_auth};
use crate::clock::now_ms;
use crate::credentials::{Account, Credential, FileCredentialStore};
use crate::errors::{Error, models_error};
use crate::limits::{AccountUsage, ResetAnswer, UsageLimit, UsageReset};
use crate::providers::keys::KeyCheck;
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
    /// The model takes image input (the catalog's input list, the same
    /// source as read_image's gate).
    pub image_input: bool,
}

/// One of a provider's accounts, as a client shows it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AccountInfo {
    pub id: String,
    /// A subscription's email or name (empty until known); an API key's
    /// name, or else its end ("sk-…a1b2"); a key from the environment, its
    /// variable.
    pub label: String,
    /// "oauth" or "api_key".
    pub kind: &'static str,
    /// It reached a usage limit and is skipped until then, in ms since 1970;
    /// 0 when it has capacity.
    pub exhausted_until: u64,
    /// An API key that ran out of credit, until a request on it succeeds.
    pub out_of_credit: bool,
    /// Factory stored it, so it can be signed out of; a key from the
    /// environment can't.
    pub stored: bool,
    /// Requests go to it now: the first with capacity.
    pub in_use: bool,
}

/// The account a key in the environment is, after those stored.
pub const ENV_ACCOUNT: &str = "env";

/// How long a key out of credit is skipped before a request tries it again.
pub const CREDIT_RETRY_MS: u64 = 60 * 60 * 1000;

/// What a stream tells its caller of the provider's accounts.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AccountNote {
    /// An account reached its limit and the request carried on with the
    /// next: a line for the agent's transcript.
    Switched(String),
    /// An account reached its limit and none is left; the request fails
    /// saying so.
    Exhausted,
    /// An account's state changed otherwise: a key out of credit has some again.
    Changed,
}

/// Where a stream tells its caller of the provider's accounts.
pub type Notes = Arc<dyn Fn(AccountNote) + Send + Sync>;

/// How long an account that reached its limit is skipped when neither the
/// provider nor its usage says when the limit resets.
const UNKNOWN_RESET_MS: u64 = 15 * 60 * 1000;

/// Whether the model's usage counts against Anthropic's Fable pool. Fable
/// has a weekly limit of its own (`weekly_fable`) on top of the shared
/// ones, and the Opus and Sonnet weekly caps don't cover it, so each pool
/// learns its own state: a mark on one doesn't stop the other's requests
/// (at worst the provider refuses the probe and the pool is marked too).
/// Every other model, and every other provider, draws on the normal pool.
fn fable_pool(model: &Model) -> bool {
    model.provider == "anthropic" && model.id.starts_with("claude-fable")
}

/// When the account's limit for the pool resets; 0 when it has capacity.
fn pool_until(account: &Account, fable: bool) -> u64 {
    if fable { account.fable_exhausted_until } else { account.exhausted_until }
}

/// Whether requests for the pool's models can go to the account at `now`:
/// past the pool's limit reset, and with credit (or out of it long enough
/// to try again).
fn has_capacity(account: &Account, fable: bool, now: u64) -> bool {
    pool_until(account, fable) <= now && (account.out_of_credit_since == 0 || now >= account.out_of_credit_since + CREDIT_RETRY_MS)
}

/// The first account with capacity for the pool at `now`, leaving out `tried`.
fn with_capacity<'a>(accounts: &'a [Account], tried: &[String], fable: bool, now: u64) -> Option<(usize, &'a Account)> {
    accounts.iter().enumerate().find(|(_, account)| has_capacity(account, fable, now) && !tried.contains(&account.id))
}

/// How an account's request failed for good on that account.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Spent {
    /// Its usage limit, resetting then (ms since 1970) if said.
    Limit(Option<u64>),
    /// An API key out of credit.
    Credit,
}

fn spent(message: &AssistantMessage) -> Option<Spent> {
    match usage_limit(message) {
        Some(resets_at) => Some(Spent::Limit(resets_at)),
        None => out_of_credit(message).then_some(Spent::Credit),
    }
}

/// How people see an account: a key unnamed by its end.
fn shown_label(account: &Account) -> String {
    match &account.credential {
        Credential::ApiKey { key, .. } if account.label.is_empty() => key_label(key),
        _ => account.label.clone(),
    }
}

/// The error a stream ends with when it can't go on, without a response.
/// Whose credentials a request went with: the account chosen, and how it
/// signed in.
fn refused_by(chosen: Option<&Account>) -> Refused {
    let Some(account) = chosen else { return Refused::default() };
    let method = match account.credential {
        Credential::ApiKey { .. } => "api_key",
        Credential::Oauth { .. } => "oauth",
    };
    Refused { account: account.id.clone(), method: method.to_owned() }
}

fn failed(model: &Model, text: String) -> StreamEvent {
    let message = AssistantMessage {
        content: Vec::new(),
        api: model.api.clone(),
        provider: model.provider.clone(),
        model: model.id.clone(),
        usage: Usage::empty(),
        stop_reason: StopReason::Error,
        error_message: Some(text),
        timestamp: now_ms(),
        extra: Map::new(),
    };
    StreamEvent::Error { reason: StopReason::Error, error: message }
}

/// The state of a key in the environment (it isn't stored): when each
/// pool's limit resets and since when out of credit.
#[derive(Clone, Copy, Default)]
struct EnvState {
    exhausted_until: u64,
    fable_exhausted_until: u64,
    out_of_credit_since: u64,
}

struct Inner {
    providers: Vec<Provider>,
    store: FileCredentialStore,
    env: Env,
    /// The state of the keys in the environment, by provider id.
    env_state: Mutex<HashMap<String, EnvState>>,
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
        Registry { inner: Arc::new(Inner { providers, store, env, env_state: Mutex::new(HashMap::new()) }) }
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
                    image_input: model.input.iter().any(|input| input == "image"),
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

    /// The provider's accounts, in the order requests try them.
    pub async fn accounts(&self, id: &str) -> Result<Vec<AccountInfo>, Error> {
        let now = now_ms();
        let accounts = self.candidates(id).await?;
        // As shown: the normal pool, which is what most models draw on.
        let in_use = with_capacity(&accounts, &[], false, now).map(|(position, _)| position);
        Ok(accounts
            .into_iter()
            .enumerate()
            .map(|(position, account)| AccountInfo {
                label: shown_label(&account),
                kind: account.credential.kind(),
                exhausted_until: if account.exhausted_until > now { account.exhausted_until } else { 0 },
                out_of_credit: account.out_of_credit_since != 0,
                stored: account.id != ENV_ACCOUNT,
                in_use: in_use == Some(position),
                id: account.id,
            })
            .collect())
    }

    /// The provider's accounts in the order requests try them: those stored,
    /// then a key in the environment.
    async fn candidates(&self, id: &str) -> Result<Vec<Account>, Error> {
        let mut accounts = credential_result(self.inner.store.accounts(id).await, "read", id)?;
        if let Some((variable, key)) = self.env_key(id) {
            let state = self.inner.env_state.lock().get(id).copied().unwrap_or_default();
            accounts.push(Account {
                id: ENV_ACCOUNT.into(),
                label: variable,
                identity: String::new(),
                exhausted_until: state.exhausted_until,
                fable_exhausted_until: state.fable_exhausted_until,
                out_of_credit_since: state.out_of_credit_since,
                credential: Credential::ApiKey { key, extra: Map::new() },
            });
        }
        Ok(accounts)
    }

    /// The provider's key in the environment: its variable and the key.
    fn env_key(&self, id: &str) -> Option<(String, String)> {
        let api_key = self.provider(id)?.api_key.as_ref()?;
        let resolved = api_key.resolve(&self.inner.env, None).ok()??;
        Some((resolved.source, resolved.auth.api_key?))
    }

    /// Changes an account's limit state (the pool `fable` names) and credit
    /// state, stored or not; returns the provider's accounts then.
    async fn set_state(
        &self,
        id: &str,
        account: &str,
        fable: bool,
        change: impl FnOnce(&mut u64, &mut u64) + Send,
    ) -> Result<Vec<Account>, Error> {
        if account == ENV_ACCOUNT {
            {
                let mut states = self.inner.env_state.lock();
                let state = states.entry(id.to_owned()).or_default();
                let until = if fable { &mut state.fable_exhausted_until } else { &mut state.exhausted_until };
                change(until, &mut state.out_of_credit_since);
            }
            return self.candidates(id).await;
        }
        let account = account.to_owned();
        let changed = self
            .inner
            .store
            .modify_accounts(id, |mut accounts| async move {
                let Some(stored) = accounts.iter_mut().find(|stored| stored.id == account) else { return Ok(None) };
                let until = if fable { &mut stored.fable_exhausted_until } else { &mut stored.exhausted_until };
                change(until, &mut stored.out_of_credit_since);
                Ok(Some(accounts))
            })
            .await;
        credential_result(changed, "modify", id)?;
        self.candidates(id).await
    }

    /// Renames one of the provider's accounts (an empty `label`: back to the
    /// name Factory gives it) and, with `retry`, tries a key out of credit
    /// again from the next request.
    pub async fn update_account(&self, id: &str, account: &str, label: Option<&str>, retry: bool) -> Result<(), Error> {
        let accounts = self.candidates(id).await?;
        if !accounts.iter().any(|stored| stored.id == account) {
            return Err(models_error("auth", format!("No such account: {account}"), None));
        }
        if let Some(label) = label {
            if account == ENV_ACCOUNT {
                return Err(models_error("auth", "A key from the environment is named by its variable", None));
            }
            let (wanted, label) = (account.to_owned(), label.trim().to_owned());
            let renamed = self
                .inner
                .store
                .modify_accounts(id, |mut accounts| async move {
                    let Some(stored) = accounts.iter_mut().find(|stored| stored.id == wanted) else { return Ok(None) };
                    stored.label = label;
                    Ok(Some(accounts))
                })
                .await;
            credential_result(renamed, "modify", id)?;
        }
        if retry {
            self.set_state(id, account, false, |_, since| *since = 0).await?;
        }
        Ok(())
    }

    /// The usage limits of a provider signed in with a subscription (Claude Pro/Max, ChatGPT), read from the
    /// provider; None when it isn't signed in that way or has no limits to read. Refreshes a token due for it.
    /// Its first account's.
    pub async fn usage_limits(&self, id: &str) -> Result<Option<Vec<UsageLimit>>, Error> {
        Ok(self.account_usage(id, None).await?.map(|usage| usage.limits))
    }

    /// The usage limits and usage resets of one of the provider's accounts
    /// (None: its first), as `usage_limits`.
    pub async fn account_usage(&self, id: &str, account: Option<&str>) -> Result<Option<AccountUsage>, Error> {
        let Some(provider) = self.provider(id) else { return Ok(None) };
        let Some(limits) = provider.limits else { return Ok(None) };
        let Some((base_url, auth)) = self.subscription_auth(provider, account).await? else { return Ok(None) };
        limits(base_url, auth).await.map(Some)
    }

    /// Uses one of the usage resets of one of the provider's accounts (None:
    /// its first), as its last usage read listed it: only ever at the
    /// person's word, since it can't be undone. A limit reset gives the
    /// account requests again at once. None when it isn't signed in with a
    /// subscription that has resets.
    pub async fn use_reset(&self, id: &str, account: Option<&str>, reset: UsageReset) -> Result<Option<ResetAnswer>, Error> {
        let Some(provider) = self.provider(id) else { return Ok(None) };
        let Some(use_reset) = provider.reset else { return Ok(None) };
        let Some((base_url, auth)) = self.subscription_auth(provider, account).await? else { return Ok(None) };
        let answer = use_reset(base_url, auth, reset).await?;
        if answer.reset {
            let stored = credential_result(self.inner.store.accounts(id).await, "read", id)?;
            let reset_account = match account {
                Some(account) => stored.iter().find(|stored| stored.id == account),
                None => stored.first(),
            };
            if let Some(reset_account) = reset_account.map(|stored| stored.id.clone()) {
                // A reset refills limits; which pools have room again, the
                // next request finds out, so both get to try.
                self.set_state(id, &reset_account, false, |until, _| *until = 0).await?;
                self.set_state(id, &reset_account, true, |until, _| *until = 0).await?;
            }
        }
        Ok(Some(answer))
    }

    /// The plan of a provider signed in with a subscription, such as "Claude Max 20x", read from the provider;
    /// None when it isn't signed in that way or can't say, and empty when it didn't say. Refreshes a token due for it.
    /// Its first account's.
    pub async fn plan(&self, id: &str) -> Result<Option<String>, Error> {
        self.account_plan(id, None).await
    }

    /// `plan` of one of the provider's accounts (None: its first). An account
    /// not yet known by name (one signed in before accounts were) is read
    /// for who it is too.
    pub async fn account_plan(&self, id: &str, account: Option<&str>) -> Result<Option<String>, Error> {
        let Some(provider) = self.provider(id) else { return Ok(None) };
        let Some(plan) = provider.plan else { return Ok(None) };
        let Some((base_url, auth)) = self.subscription_auth(provider, account).await? else { return Ok(None) };
        if let Some(identify) = provider.identity {
            let stored = credential_result(self.inner.store.accounts(id).await, "read", id)?;
            let unnamed = match account {
                Some(account) => stored.iter().find(|stored| stored.id == account),
                None => stored.first(),
            }
            .filter(|stored| stored.label.is_empty())
            .map(|stored| stored.id.clone());
            if let Some(account) = unnamed {
                match identify(base_url.clone(), auth.clone()).await {
                    Ok(identity) if !identity.label.is_empty() || !identity.key.is_empty() => {
                        self.name_account(id, &account, identity).await?;
                    }
                    Ok(_) => {}
                    Err(error) => tracing::warn!(provider = id, %error, "could not read who the account is"),
                }
            }
        }
        plan(base_url, auth).await.map(Some)
    }

    async fn name_account(&self, id: &str, account: &str, identity: crate::limits::Identity) -> Result<(), Error> {
        let account = account.to_owned();
        let named = self
            .inner
            .store
            .modify_accounts(id, |mut accounts| async move {
                let Some(stored) = accounts.iter_mut().find(|stored| stored.id == account) else { return Ok(None) };
                stored.label = identity.label;
                stored.identity = identity.key;
                Ok(Some(accounts))
            })
            .await;
        credential_result(named, "modify", id).map(|_| ())
    }

    /// The base url and auth of a provider's account signed in with a subscription; None when it isn't.
    async fn subscription_auth(&self, provider: &Provider, account: Option<&str>) -> Result<Option<(String, RequestAuth)>, Error> {
        let Some(oauth) = &provider.oauth else { return Ok(None) };
        let accounts = credential_result(self.inner.store.accounts(provider.id).await, "read", provider.id)?;
        let stored = match account {
            Some(account) => accounts.iter().find(|stored| stored.id == account),
            None => accounts.first(),
        };
        if !oauth.is_subscription() || !matches!(stored, Some(Account { credential: Credential::Oauth { .. }, .. })) {
            return Ok(None);
        }
        let resolution = resolve_account_auth(provider, &self.inner.store, &self.inner.env, account, &CancellationToken::new()).await?;
        let Some(resolution) = resolution else { return Ok(None) };
        let base_url = resolution.auth.base_url.clone().unwrap_or_else(|| provider.base_url.to_owned());
        Ok(Some((base_url, resolution.auth)))
    }

    pub async fn get_auth(&self, model: &Model) -> Result<Option<AuthResult>, Error> {
        self.account_auth(model, None).await
    }

    /// `get_auth` with one of the provider's accounts (None: its first).
    async fn account_auth(&self, model: &Model, account: Option<&str>) -> Result<Option<AuthResult>, Error> {
        let Some(provider) = self.provider(&model.provider) else { return Ok(None) };
        let resolved = match (account, &provider.api_key) {
            (Some(ENV_ACCOUNT), Some(api_key)) => api_key.resolve(&self.inner.env, None)?,
            _ => resolve_account_auth(provider, &self.inner.store, &self.inner.env, account, &CancellationToken::new()).await?,
        };
        let Some(mut result) = resolved else {
            return Ok(None);
        };
        if model.headers.is_some() {
            result.auth.headers = merge_headers(&result.auth.headers, &model_headers(model));
        }
        Ok(Some(result))
    }

    /// Signs in and stores the credential as the provider's first account,
    /// replacing it, unless it's an account already signed in, which is
    /// signed in again.
    pub async fn login(&self, id: &str, kind: &str, interaction: &dyn AuthInteraction) -> Result<Credential, Error> {
        self.login_account(id, kind, interaction, false).await
    }

    /// `login`; with `add`, the account is added after the provider's others
    /// instead of replacing its first (unless it's one of them).
    pub async fn login_account(&self, id: &str, kind: &str, interaction: &dyn AuthInteraction, add: bool) -> Result<Credential, Error> {
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
        tokio::select! {
            biased;
            () = cancel.cancelled() => return Err(Error::Aborted),
            checked = self.check_key(provider, &credential) => checked?,
        }
        let identity = self.identify(provider, &credential).await;
        let stored = credential.clone();
        self.inner
            .store
            .modify_accounts(id, |mut accounts| async move {
                let same = accounts.iter().position(|account| !identity.key.is_empty() && account.identity == identity.key);
                let mut account = Account::new(stored);
                account.label = identity.label;
                account.identity = identity.key;
                match same {
                    // Signed in again: the same account, with its new credential and its limit forgotten.
                    Some(position) => {
                        let label = if account.label.is_empty() { accounts[position].label.clone() } else { account.label };
                        accounts[position] = Account { id: accounts[position].id.clone(), label, ..account }
                    }
                    None if add || accounts.is_empty() => accounts.push(account),
                    None => accounts[0] = account,
                }
                Ok(Some(accounts))
            })
            .await
            .map_err(|error| models_error("auth", format!("Credential store modify failed for {id}"), Some(&error)))?;
        Ok(credential)
    }

    /// Asks the provider whether it accepts a new API key, so one it refuses
    /// is never kept. A key that can't be checked (the provider is down, or
    /// the server at its base URL has no such call) is kept: its first
    /// request says what's wrong.
    async fn check_key(&self, provider: &Provider, credential: &Credential) -> Result<(), Error> {
        let (Credential::ApiKey { .. }, Some(check), Some(api_key)) = (credential, provider.check_key, &provider.api_key) else {
            return Ok(());
        };
        let Some(resolved) = api_key.resolve(&self.inner.env, Some(credential))? else { return Ok(()) };
        let base_url = resolved.auth.base_url.clone().unwrap_or_else(|| provider.base_url.to_owned());
        match check(base_url, resolved.auth).await {
            KeyCheck::Accepted => Ok(()),
            KeyCheck::Refused(detail) => {
                tracing::info!(provider = provider.id, %detail, "the provider refused a new API key");
                Err(Error::Models { code: crate::errors::REFUSED, message: format!("{} didn't accept this key.", provider.name) })
            }
            KeyCheck::Unchecked(why) => {
                tracing::warn!(provider = provider.id, %why, "could not check a new API key; keeping it");
                Ok(())
            }
        }
    }

    /// Who a new sign-in is: read from a subscription, or the end of an API
    /// key; nothing when that fails.
    async fn identify(&self, provider: &Provider, credential: &Credential) -> crate::limits::Identity {
        match credential {
            // A key is named by its end until given a name; the same key added again is the same account.
            Credential::ApiKey { key, .. } => {
                use sha2::Digest;
                let digest = sha2::Sha256::digest(key.trim().as_bytes());
                let hex: String = digest.iter().take(8).map(|byte| format!("{byte:02x}")).collect();
                crate::limits::Identity { key: format!("key:{hex}"), label: String::new() }
            }
            Credential::Oauth { .. } => {
                let (Some(identify), Some(oauth)) = (provider.identity, &provider.oauth) else { return Default::default() };
                let auth = match oauth.to_auth(credential) {
                    Ok(auth) => auth,
                    Err(error) => {
                        tracing::warn!(provider = provider.id, %error, "could not read who the account is");
                        return Default::default();
                    }
                };
                let base_url = auth.base_url.clone().unwrap_or_else(|| provider.base_url.to_owned());
                identify(base_url, auth).await.unwrap_or_else(|error| {
                    tracing::warn!(provider = provider.id, %error, "could not read who the account is");
                    Default::default()
                })
            }
        }
    }

    /// Signs out of all of the provider's accounts.
    pub async fn logout(&self, id: &str) -> Result<(), Error> {
        credential_result(self.inner.store.delete(id).await, "delete", id)
    }

    /// Signs out of one of the provider's accounts; an empty `account`, all.
    pub async fn logout_account(&self, id: &str, account: &str) -> Result<(), Error> {
        if account.is_empty() {
            return self.logout(id).await;
        }
        let account = account.to_owned();
        let removed = self
            .inner
            .store
            .modify_accounts(id, |mut accounts| async move {
                let before = accounts.len();
                accounts.retain(|stored| stored.id != account);
                Ok((accounts.len() != before).then_some(accounts))
            })
            .await;
        credential_result(removed, "modify", id).map(|_| ())
    }

    /// Moves one of the provider's accounts to `position` (0 is first, past
    /// the end is last) in the order requests try them.
    pub async fn move_account(&self, id: &str, account: &str, position: usize) -> Result<(), Error> {
        if account == ENV_ACCOUNT {
            return Err(models_error("auth", "A key from the environment comes after the accounts signed in", None));
        }
        let wanted = account.to_owned();
        let mut found = false;
        let moved = self
            .inner
            .store
            .modify_accounts(id, |mut accounts| {
                let from = accounts.iter().position(|stored| stored.id == wanted);
                found = from.is_some();
                async move {
                    let Some(from) = from else { return Ok(None) };
                    let moving = accounts.remove(from);
                    accounts.insert(position.min(accounts.len()), moving);
                    Ok(Some(accounts))
                }
            })
            .await;
        credential_result(moved, "modify", id)?;
        if !found {
            return Err(models_error("auth", format!("No such account: {account}"), None));
        }
        Ok(())
    }

    /// Marks an account as having reached its usage limit for the pool
    /// (`fable` or not) until it resets: when the provider said, or else
    /// when its usage read now says the pool's full limits do, or else in a
    /// while. Returns the provider's accounts then.
    async fn exhaust(&self, id: &str, account: &str, fable: bool, resets_at: Option<u64>) -> Result<Vec<Account>, Error> {
        let now = now_ms();
        let resets_at = match resets_at.filter(|at| *at > now) {
            Some(at) => Some(at),
            None => match self.account_usage(id, Some(account)).await {
                Ok(Some(usage)) => usage
                    .limits
                    .iter()
                    .filter(|limit| limit.used_percent >= 100.0)
                    // Only limits the pool draws on. Fable requests pass
                    // the 5-hour gate, the weekly one (it applies across
                    // all models, Anthropic says) and Fable's own; the
                    // other models everything but Fable's.
                    .filter(|limit| {
                        if fable {
                            matches!(limit.id.as_str(), "five_hour" | "weekly" | "weekly_fable")
                        } else {
                            limit.id != "weekly_fable"
                        }
                    })
                    .map(|limit| limit.resets_at)
                    .filter(|at| *at > now)
                    .max(),
                Ok(None) => None,
                Err(error) => {
                    tracing::warn!(provider = id, %error, "could not read the usage of an account at its limit");
                    None
                }
            },
        };
        let until = resets_at.unwrap_or(now + UNKNOWN_RESET_MS);
        self.set_state(id, account, fable, move |exhausted_until, _| *exhausted_until = until).await
    }

    async fn open(&self, model: &Model, context: LlmContext, options: StreamOptions, account: Option<&str>) -> Result<EventStream, Error> {
        let provider = self
            .provider(&model.provider)
            .ok_or_else(|| models_error("provider", format!("Unknown provider: {}", model.provider), None))?;
        let resolution = self
            .account_auth(model, account)
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
        self.stream_fn_with(None)
    }

    /// `stream_fn`, over the provider's accounts: a request goes to the first
    /// with capacity, and when it fails on that account's usage limit, the
    /// account is skipped until the limit resets and the request goes again,
    /// at once, to the next, which `notes` is told. With none left, it fails
    /// saying when the first resets. Nothing else changes accounts.
    pub fn stream_fn_with(&self, notes: Option<Notes>) -> StreamFn {
        let registry = self.clone();
        Arc::new(move |model, context, options| {
            let (sink, stream) = event_stream();
            let (registry, model, notes) = (registry.clone(), model.clone(), notes.clone());
            tokio::spawn(async move {
                let fable = fable_pool(&model);
                let mut tried = Vec::new();
                loop {
                    let accounts = match registry.candidates(&model.provider).await {
                        Ok(accounts) => accounts,
                        Err(error) => {
                            sink.push(failed(&model, error.to_string()));
                            return;
                        }
                    };
                    let chosen = with_capacity(&accounts, &tried, fable, now_ms()).map(|(_, account)| account.clone());
                    if chosen.is_none() && !accounts.is_empty() {
                        sink.push(registry.none_left(&model, &accounts));
                        return;
                    }
                    let account = chosen.as_ref().map(|account| account.id.clone());
                    let mut inner = match registry.open(&model, context.clone(), options.clone(), account.as_deref()).await {
                        Ok(inner) => inner,
                        Err(error) => {
                            let mut event = failed(&model, error.to_string());
                            // A sign-in whose token the provider won't refresh.
                            if let (true, StreamEvent::Error { error, .. }) = (error.is_refused(), &mut event) {
                                mark_refused(error, &refused_by(chosen.as_ref()));
                            }
                            sink.push(event);
                            return;
                        }
                    };
                    // A response that fails on the account's limit or credit does so
                    // before anything but its start: held until something else comes.
                    let (mut held, mut forwarding) = (None, false);
                    let (mut gone, mut done) = (None, false);
                    while let Some(mut event) = inner.next().await {
                        if !forwarding {
                            match &event {
                                StreamEvent::Start { .. } => {
                                    held = Some(event);
                                    continue;
                                }
                                StreamEvent::Error { error, .. } if account.is_some() && spent(error).is_some() => {
                                    gone = spent(error);
                                    held = Some(event);
                                    break;
                                }
                                _ => {
                                    forwarding = true;
                                    if let Some(start) = held.take() {
                                        sink.push(start);
                                    }
                                }
                            }
                        }
                        done = matches!(event, StreamEvent::Done { .. });
                        match &mut event {
                            StreamEvent::Done { message, .. } => {
                                if let Some(account) = &account {
                                    mark_answered_by(message, account);
                                }
                            }
                            // The provider says whether it refused the credentials; whose they were is known here.
                            StreamEvent::Error { error, .. } if refused(error).is_some() => {
                                let by = refused_by(chosen.as_ref());
                                tracing::info!(provider = %model.provider, account = %by.account, method = %by.method, "the provider refused the credentials");
                                mark_refused(error, &by);
                            }
                            _ => {}
                        }
                        sink.push(event);
                    }
                    let (Some(gone), Some(account)) = (gone, account) else {
                        if let Some(start) = held {
                            sink.push(start);
                        }
                        // A key out of credit that answered has some again.
                        if let (true, Some(chosen)) = (done, chosen.filter(|chosen| chosen.out_of_credit_since != 0)) {
                            match registry.set_state(&model.provider, &chosen.id, fable, |_, since| *since = 0).await {
                                Ok(_) => {
                                    if let Some(notes) = &notes {
                                        notes(AccountNote::Changed);
                                    }
                                }
                                Err(error) => tracing::warn!(provider = %model.provider, %error, "could not mark a key as having credit"),
                            }
                        }
                        return;
                    };
                    tried.push(account.clone());
                    let marked = match gone {
                        Spent::Limit(resets_at) => registry.exhaust(&model.provider, &account, fable, resets_at).await,
                        Spent::Credit => registry.set_state(&model.provider, &account, fable, |_, since| *since = now_ms()).await,
                    };
                    let accounts = match marked {
                        Ok(accounts) => accounts,
                        Err(error) => {
                            tracing::warn!(provider = %model.provider, %error, "could not mark an account as spent");
                            accounts
                        }
                    };
                    // In the log too, so it can be seen and watched for.
                    let why = match gone {
                        Spent::Limit(_) => {
                            let until = accounts.iter().find(|stored| stored.id == account).map_or(0, |stored| pool_until(stored, fable));
                            let resets = if until > now_ms() { crate::clock::iso_from_ms(until) } else { "unknown".to_owned() };
                            tracing::info!(provider = %model.provider, %account, %resets, "account reached its usage limit");
                            format!("usage limit, resets {resets}")
                        }
                        Spent::Credit => {
                            tracing::info!(provider = %model.provider, %account, "account ran out of credit");
                            "out of credit".to_owned()
                        }
                    };
                    let Some((position, next)) = with_capacity(&accounts, &tried, fable, now_ms()) else {
                        tracing::info!(provider = %model.provider, "no account left with capacity ({why} on account {account})");
                        if let Some(notes) = &notes {
                            notes(AccountNote::Exhausted);
                        }
                        sink.push(registry.none_left(&model, &accounts));
                        return;
                    };
                    tracing::info!(provider = %model.provider, "moved from account {account} to {} ({why})", next.id);
                    if let Some(notes) = &notes {
                        let noun = registry.noun(&model.provider);
                        let from = accounts.iter().position(|stored| stored.id == account);
                        let from = from.map_or_else(|| account.clone(), |from| account_name(&shown_label(&accounts[from]), from));
                        let to = account_name(&shown_label(next), position);
                        notes(AccountNote::Switched(switch_note(&noun, &from, &to, gone == Spent::Credit)));
                    }
                }
            });
            Ok(stream)
        })
    }

    /// The error a request ends with when no account can take it: saying
    /// why, marked with when the first account to reset does (none when
    /// they're out of credit, which waits for the person). Judged for the
    /// model's pool: another pool's limit doesn't hold its requests back.
    fn none_left(&self, model: &Model, accounts: &[Account]) -> StreamEvent {
        let now = now_ms();
        let fable = fable_pool(model);
        let until = accounts.iter().map(|account| pool_until(account, fable)).filter(|at| *at > now).min();
        let mut event = failed(model, self.exhausted(&model.provider, accounts, fable));
        if let StreamEvent::Error { error, .. } = &mut event {
            mark_none_left(error, until);
        }
        event
    }

    /// What people call the provider's accounts: "Claude".
    fn noun(&self, id: &str) -> String {
        account_noun(id, self.provider(id).map_or(id, |provider| provider.name))
    }

    /// Why a request can't go to any of the provider's `accounts`: all are at
    /// their limit for the model's pool, and when the first resets.
    fn exhausted(&self, id: &str, accounts: &[Account], fable: bool) -> String {
        let first = accounts
            .iter()
            .enumerate()
            .filter(|(_, account)| pool_until(account, fable) > 0)
            .min_by_key(|(_, account)| pool_until(account, fable));
        let first = first.map(|(position, account)| (account_name(&shown_label(account), position), pool_until(account, fable)));
        let credit = accounts.iter().any(|account| account.out_of_credit_since != 0);
        exhausted_message(&self.noun(id), accounts.len(), first.as_ref().map(|(name, at)| (name.as_str(), *at)), credit)
    }
}
