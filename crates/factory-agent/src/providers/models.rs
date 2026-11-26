use std::path::PathBuf;
use std::sync::Arc;

use serde_json::Map;
use tokio_util::sync::CancellationToken;

use hashbrown::HashMap;

use parking_lot::Mutex;

use crate::accounts::{
    Refused, account_name, account_noun, exhausted_message, key_label, mark_answered_by, mark_none_left, mark_refused, out_of_credit,
    refused, switch_note, usage_limit, usage_limit_claim,
};
use crate::auth::{AuthCheck, AuthInteraction, AuthResult, Env, RequestAuth, process_env, resolve_account_auth, resolve_provider_auth};
use crate::clock::now_ms;
use crate::credentials::{Account, Credential, CredentialBacking, FileCredentialStore};
use crate::errors::{Error, models_error};
use crate::limits::{AccountUsage, ResetAnswer, UsageLimit, UsageReset};
use crate::providers::anthropic::files::{Forgotten, ImageFiles, ImageFilesFor};
use crate::providers::keys::KeyCheck;
use crate::providers::{
    Provider, anthropic, grok, merge_headers, model_headers, openai, openai_codex, openrouter, supported_thinking_levels,
};
use crate::stream::{EventStream, LlmContext, StreamFn, StreamOptions, ThinkingLevel, event_stream};
use crate::types::{AssistantMessage, Model, StopReason, StreamEvent, Usage};

/// A credential store's answer, its failure said as the store's; sign-ins
/// locked away stay `LOCKED`, so the request waits for them.
fn credential_result<T>(result: Result<T, Error>, operation: &str, id: &str) -> Result<T, Error> {
    result.map_err(|error| {
        if error.is_locked() { error } else { models_error("auth", format!("Credential store {operation} failed for {id}"), Some(&error)) }
    })
}

/// `config.ts` registers the four providers in this order.
pub fn default_providers() -> Vec<Provider> {
    vec![anthropic::provider(), openai::provider(), openai_codex::provider(), openrouter::provider(), grok::provider()]
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
    /// An agent on it can make images (#403): its provider makes them on
    /// this sign-in (ChatGPT's subscription, `Provider::images`) and the
    /// model takes image input, as Codex's own rule has it. Whether the
    /// plan allows it is factoryd's to add, from the plan it reads.
    pub image_output: bool,
    /// Added by its id, as the provider's list doesn't have it.
    pub added: bool,
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
    /// The provider refused to refresh its sign-in: skipped until it's
    /// signed in to again.
    pub sign_in_again: bool,
    /// Factory stored it, so it can be signed out of; a key from the
    /// environment can't.
    pub stored: bool,
    /// Requests go to it now: the first with capacity.
    pub in_use: bool,
    /// Fable's own pool reached its limit on it, until then (ms since 1970);
    /// 0 when Fable has room on it.
    pub fable_exhausted_until: u64,
    /// Fable models' requests go to it now: the first with capacity in
    /// Fable's pool (Anthropic only).
    pub fable_in_use: bool,
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

/// Which of an account's limits a model's requests draw on. Anthropic keeps
/// weekly limits of some models' own on top of the shared 5-hour and weekly
/// ones: Fable's (`weekly_fable`), Opus's (`weekly_opus`) and Sonnet's
/// (`weekly_sonnet`). At one of them only those models wait; the others
/// carry on. Fable learns its state apart from the shared limits too (a
/// mark on one doesn't stop the other's requests: at worst the provider
/// refuses the probe and the pool is marked too); Opus and Sonnet wait for
/// the shared limits' mark as well as their own. Every other model, and
/// every other provider, draws on the normal pool.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Pool {
    Normal,
    Fable,
    Opus,
    Sonnet,
}

impl Pool {
    /// The pool of a provider's model id ("claude-opus-4-8").
    pub fn of(provider: &str, model: &str) -> Pool {
        if provider != "anthropic" {
            Pool::Normal
        } else if model.starts_with("claude-fable") {
            Pool::Fable
        } else if model.contains("opus") {
            Pool::Opus
        } else if model.contains("sonnet") {
            Pool::Sonnet
        } else {
            Pool::Normal
        }
    }

    fn of_model(model: &Model) -> Pool {
        Pool::of(&model.provider, &model.id)
    }

    /// The usage window of the pool's own weekly limit ("weekly_opus");
    /// None for the normal pool.
    pub fn own_window(self) -> Option<&'static str> {
        match self {
            Pool::Normal => None,
            Pool::Fable => Some("weekly_fable"),
            Pool::Opus => Some("weekly_opus"),
            Pool::Sonnet => Some("weekly_sonnet"),
        }
    }

    /// The name Anthropic gives the pool's own limit when it refuses a
    /// request for it (`anthropic::limit_claim`).
    fn claim(self) -> Option<&'static str> {
        match self {
            Pool::Opus => Some("seven_day_opus"),
            Pool::Sonnet => Some("seven_day_sonnet"),
            Pool::Normal | Pool::Fable => None,
        }
    }

    /// Whether a usage window (`UsageLimit.id`) holds the pool's requests
    /// back when it's full: the 5-hour and weekly ones every pool's, a
    /// model's own weekly one only its pool's, any other every pool's but
    /// Fable's.
    pub fn limited_by(self, window: &str) -> bool {
        match window {
            "five_hour" | "weekly" => true,
            "weekly_fable" | "weekly_opus" | "weekly_sonnet" => self.own_window() == Some(window),
            _ => self != Pool::Fable,
        }
    }
}

/// Whether the model's usage counts against Anthropic's Fable pool.
pub fn in_fable_pool(provider: &str, model: &str) -> bool {
    Pool::of(provider, model) == Pool::Fable
}

/// An account's limit mark for `pool` alone, when it resets (ms since
/// 1970); 0 when it isn't marked. The normal pool's mark is the shared
/// limits'.
fn mark_until(account: &Account, pool: Pool) -> u64 {
    match pool {
        Pool::Normal => account.exhausted_until,
        Pool::Fable => account.fable_exhausted_until,
        Pool::Opus => account.opus_exhausted_until,
        Pool::Sonnet => account.sonnet_exhausted_until,
    }
}

/// When the account's limits for the pool's requests reset; 0 (or past)
/// when it has capacity. Opus and Sonnet wait for the shared limits' mark
/// too; Fable learns its own.
fn pool_until(account: &Account, pool: Pool) -> u64 {
    match pool {
        Pool::Normal | Pool::Fable => mark_until(account, pool),
        Pool::Opus | Pool::Sonnet => account.exhausted_until.max(mark_until(account, pool)),
    }
}

/// Whether requests for the pool's models can go to the account at `now`:
/// past the pool's limit reset, with credit (or out of it long enough to try
/// again), and with a sign-in the provider still refreshes.
fn has_capacity(account: &Account, pool: Pool, now: u64) -> bool {
    pool_until(account, pool) <= now
        && (account.out_of_credit_since == 0 || now >= account.out_of_credit_since + CREDIT_RETRY_MS)
        && account.refused_since == 0
}

/// The first account with capacity for the pool at `now`, leaving out `tried`.
fn with_capacity<'a>(accounts: &'a [Account], tried: &[String], pool: Pool, now: u64) -> Option<(usize, &'a Account)> {
    accounts.iter().enumerate().find(|(_, account)| has_capacity(account, pool, now) && !tried.contains(&account.id))
}

/// How an account's request failed for good on that account.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Spent {
    /// A usage limit, resetting then (ms since 1970) if said: the limits
    /// of the pool it names (`Pool::Normal`: the shared ones).
    Limit(Option<u64>, Pool),
    /// An API key out of credit.
    Credit,
}

/// How a request for `pool` failed on its account, if it did: a refusal
/// naming the pool's own limit (Anthropic's "seven_day_opus" for an Opus
/// request) marks that pool alone; Fable's any refusal; others the shared
/// limits.
fn spent(message: &AssistantMessage, pool: Pool) -> Option<Spent> {
    match usage_limit(message) {
        Some(resets_at) => {
            let own = pool == Pool::Fable || (pool.claim().is_some() && usage_limit_claim(message) == pool.claim());
            Some(Spent::Limit(resets_at, if own { pool } else { Pool::Normal }))
        }
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

/// Why a request can't go to any of a provider's sign-ins: it refused to
/// refresh each of them.
fn refused_message(provider: &str, noun: &str, accounts: usize) -> String {
    if accounts == 1 {
        format!("{provider} no longer accepts your {noun} sign-in. Sign in again to use it.")
    } else {
        format!("{provider} no longer accepts any of your {noun} sign-ins. Sign in again to use them.")
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
    opus_exhausted_until: u64,
    sonnet_exhausted_until: u64,
    out_of_credit_since: u64,
}

/// Which images went to Anthropic as files, beside the sign-ins (#382).
pub const IMAGE_FILES: &str = "anthropic-files.jsonl";
/// How long whose subscription it is holds once read (`files_for`), and
/// how soon it's read again when it couldn't be.
const PERSONAL_KEEP_MS: u64 = 6 * 3_600_000;
const PERSONAL_RETRY_MS: u64 = 10 * 60_000;
/// How long a request waits to read it.
const PROFILE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

struct Inner {
    providers: Vec<Provider>,
    store: FileCredentialStore,
    env: Env,
    /// The state of the keys in the environment, by provider id.
    env_state: Mutex<HashMap<String, EnvState>>,
    /// Model lists fetched from a provider, in place of its bundled one, by
    /// provider id.
    fetched: Mutex<HashMap<String, Arc<Vec<Model>>>>,
    /// Models added by their id, as the provider's list didn't have them
    /// (friend bug #14), by provider id.
    added: Mutex<HashMap<String, Vec<Model>>>,
    /// The images Anthropic holds as files (#382); None sends them inline.
    image_files: Option<Arc<ImageFiles>>,
    /// Whether each Anthropic subscription sign-in is the person's own
    /// (`files_for`), by `provider/account`, and until when that holds.
    personal: Mutex<HashMap<String, (bool, u64)>>,
}

#[derive(Clone)]
pub struct Registry {
    inner: Arc<Inner>,
}

impl Registry {
    /// The four providers, with credentials stored at `auth_path` (created 0600 on first login) or taken from the environment.
    /// Which images went to Anthropic as files is kept beside them.
    pub fn new(auth_path: PathBuf) -> Registry {
        let files = auth_path.with_file_name(IMAGE_FILES);
        Registry::with_providers(FileCredentialStore::new(auth_path), process_env(), default_providers()).keeping_image_files(Some(files))
    }

    pub fn with_providers(store: FileCredentialStore, env: Env, providers: Vec<Provider>) -> Registry {
        Registry {
            inner: Arc::new(Inner {
                providers,
                store,
                env,
                env_state: Mutex::new(HashMap::new()),
                fetched: Mutex::new(HashMap::new()),
                added: Mutex::new(HashMap::new()),
                image_files: None,
                personal: Mutex::new(HashMap::new()),
            }),
        }
    }

    /// Sends images read since #382 to Anthropic once, as files, keeping
    /// which in `path` (None: only while it runs). Before it's cloned.
    #[must_use]
    pub fn keeping_image_files(mut self, path: Option<PathBuf>) -> Registry {
        if let Some(inner) = Arc::get_mut(&mut self.inner) {
            inner.image_files = Some(Arc::new(ImageFiles::new(path)));
        }
        self
    }

    /// Deletes at Anthropic the files of the images of `digests` (those of
    /// a conversation compacted or cleared), on every account that has
    /// them; the next request with one uploads it again.
    pub async fn forget_images(&self, digests: &[String]) {
        let Some(files) = &self.inner.image_files else { return };
        self.delete_files(files.forget(digests)).await;
    }

    /// Deletes files at Anthropic, each with its own account's sign-in.
    async fn delete_files(&self, forgotten: Vec<Forgotten>) {
        for forgotten in forgotten {
            let account = forgotten.account.as_str();
            let (provider_id, account_id) = account.split_once('/').unwrap_or((account, ""));
            let (Some(provider), Some(model)) = (self.provider(provider_id), self.get_models(Some(provider_id)).into_iter().next()) else {
                continue;
            };
            let account_id = Some(account_id).filter(|account_id| !account_id.is_empty());
            let resolved = match (account_id, &provider.api_key) {
                (Some(ENV_ACCOUNT), Some(api_key)) => api_key.resolve(&self.inner.env, None),
                _ => resolve_account_auth(provider, &self.inner.store, &self.inner.env, account_id, &CancellationToken::new()).await,
            };
            let Ok(Some(resolved)) = resolved else { continue };
            let oauth = resolved.auth.api_key.as_deref().is_some_and(|key| key.contains("sk-ant-oat"));
            let options = StreamOptions { api_key: resolved.auth.api_key, headers: resolved.auth.headers, ..StreamOptions::default() };
            let headers = crate::providers::anthropic::file_headers(&model, &options, oauth);
            if let Err(error) = crate::providers::anthropic::files::delete(&forgotten.base_url, &headers, &forgotten.id).await {
                tracing::info!(%account, %error, "could not delete an image's file at Anthropic; it expires by itself");
            }
        }
    }

    /// Deletes at Anthropic the files of the images sent to the provider's
    /// accounts (`account`, or all of them when None) while their sign-ins
    /// still can: before they're signed out of.
    async fn delete_account_files(&self, id: &str, account: Option<&str>) {
        let Some(files) = &self.inner.image_files else { return };
        let accounts: Vec<String> = match account {
            Some(account) => vec![format!("{id}/{account}")],
            None => files.accounts().into_iter().filter(|key| key.split_once('/').is_some_and(|(provider, _)| provider == id)).collect(),
        };
        let forgotten = accounts.iter().flat_map(|account| files.forget_account(account)).collect();
        self.delete_files(forgotten).await;
    }

    /// Forgets whose subscription each of the provider's accounts is (Fugo):
    /// an account signed out of and in again under another plan is read
    /// anew, not judged on the old one.
    fn forget_whose(&self, id: &str) {
        self.inner.personal.lock().retain(|key, _| key.split_once('/').is_none_or(|(provider, _)| provider != id));
    }

    /// Where a request to `provider`'s `account` keeps its images as files:
    /// only Anthropic's, and only on a subscription that's the person's
    /// own (Fugo, #382): a file is reachable to everyone with access to its
    /// organization's workspace, so an API key's (often a company's) and a
    /// Team's or Enterprise's send images inline, as does a plan it can't
    /// read.
    async fn files_for(&self, provider: &Provider, account: Option<&str>, base_url: &str, auth: &RequestAuth) -> Option<ImageFilesFor> {
        let files = self.inner.image_files.as_ref().filter(|_| provider.id == "anthropic")?;
        if !auth.api_key.as_deref().is_some_and(|key| key.contains("sk-ant-oat")) {
            return None;
        }
        let key = format!("{}/{}", provider.id, account.unwrap_or_default());
        let now = now_ms();
        let known = self.inner.personal.lock().get(&key).copied().filter(|(_, until)| *until > now);
        let personal = match known {
            Some((personal, _)) => personal,
            None => {
                let read = tokio::time::timeout(PROFILE_TIMEOUT, crate::limits::anthropic_personal(base_url.to_owned(), auth.clone())).await;
                let (personal, keep) = match read {
                    Ok(Ok(personal)) => (personal, PERSONAL_KEEP_MS),
                    Ok(Err(error)) => {
                        tracing::info!(account = %key, %error, "could not read whose subscription it is; its images go inline for now");
                        (false, PERSONAL_RETRY_MS)
                    }
                    Err(_) => (false, PERSONAL_RETRY_MS),
                };
                self.inner.personal.lock().insert(key.clone(), (personal, now + keep));
                personal
            }
        };
        personal.then(|| ImageFilesFor { files: files.clone(), account: key })
    }

    pub fn providers(&self) -> &[Provider] {
        &self.inner.providers
    }

    /// The credentials' file, `auth.json`.
    pub fn auth_path(&self) -> &std::path::Path {
        self.inner.store.path()
    }

    /// Keeps the credentials with `backing` from now on (a Claktory's vault).
    pub fn set_backing(&self, backing: Arc<dyn CredentialBacking>) {
        self.inner.store.set_backing(backing);
    }

    /// Takes in sign-ins kept elsewhere meanwhile; whether any changed.
    pub async fn take_in(&self) -> Result<bool, Error> {
        self.inner.store.take_in().await
    }

    /// Whether the provider's sign-ins are locked away now, with none made
    /// meanwhile: its requests wait until they open.
    pub async fn is_locked(&self, id: &str) -> bool {
        matches!(self.inner.store.accounts(id).await, Err(error) if error.is_locked())
    }

    pub fn provider(&self, id: &str) -> Option<&Provider> {
        self.inner.providers.iter().find(|provider| provider.id == id)
    }

    pub fn get_models(&self, provider: Option<&str>) -> Vec<Model> {
        match provider {
            Some(id) => self.provider(id).map(|provider| self.models_of(provider).to_vec()).unwrap_or_default(),
            None => self.inner.providers.iter().flat_map(|provider| self.models_of(provider).to_vec()).collect(),
        }
    }

    /// A provider's models: the list fetched from it, else its bundled one,
    /// then those added by their id that it doesn't have.
    fn models_of(&self, provider: &Provider) -> Arc<Vec<Model>> {
        let listed = match self.inner.fetched.lock().get(provider.id) {
            Some(models) => models.clone(),
            None => Arc::new(provider.models.clone()),
        };
        let added = self.inner.added.lock();
        let extra: Vec<Model> = added
            .get(provider.id)
            .into_iter()
            .flatten()
            .filter(|model| !listed.iter().any(|known| known.id == model.id))
            .cloned()
            .collect();
        if extra.is_empty() {
            return listed;
        }
        Arc::new(listed.iter().cloned().chain(extra).collect())
    }

    /// Whether the provider's model id was added by its id and isn't in the
    /// provider's own list.
    fn is_added(&self, provider: &Provider, id: &str) -> bool {
        let listed = match self.inner.fetched.lock().get(provider.id) {
            Some(models) => models.iter().any(|model| model.id == id),
            None => provider.models.iter().any(|model| model.id == id),
        };
        !listed && self.inner.added.lock().get(provider.id).is_some_and(|added| added.iter().any(|model| model.id == id))
    }

    /// Whether the provider's model list is to be fetched from the provider
    /// itself: Anthropic's, OpenAI's, Codex's and OpenRouter's, at their own
    /// address and not pointed elsewhere (`OPENROUTER_BASE_URL`, as QA's
    /// sandboxes do, or `ANTHROPIC_BASE_URL`), whose list wouldn't be theirs.
    pub fn fetches_models(&self, id: &str) -> bool {
        let Some(provider) = self.provider(id) else { return false };
        let elsewhere = |variable: &str| (self.inner.env)(variable).is_some();
        match id {
            "openrouter" => {
                (provider.base_url == crate::providers::catalog::OPENROUTER_BASE && !elsewhere("OPENROUTER_BASE_URL"))
                    || crate::providers::catalog::qa_models()
            }
            "anthropic" => provider.base_url == "https://api.anthropic.com" && !elsewhere("ANTHROPIC_BASE_URL"),
            "openai" => provider.base_url == "https://api.openai.com/v1" && !elsewhere("OPENAI_BASE_URL"),
            "openai-codex" => provider.base_url == "https://chatgpt.com/backend-api",
            "grok" => provider.base_url == grok::BASE_URL,
            _ => false,
        }
    }

    /// The base url and auth of the provider's first account, to read its
    /// model list with, and the account's id when it's a sign-in (OAuth);
    /// None when it isn't signed in (or its sign-ins are locked away), or
    /// it's signed in at another address.
    async fn listing_auth(&self, id: &str) -> Result<Option<(String, RequestAuth, Option<String>)>, String> {
        let Some(provider) = self.provider(id) else { return Ok(None) };
        let signed_in = match self.inner.store.accounts(id).await {
            Ok(accounts) => {
                accounts.into_iter().next().filter(|first| matches!(first.credential, Credential::Oauth { .. })).map(|first| first.id)
            }
            Err(_) => None,
        };
        let resolved = match resolve_account_auth(provider, &self.inner.store, &self.inner.env, None, &CancellationToken::new()).await {
            Err(error) if error.is_locked() => return Ok(None),
            resolved => resolved.map_err(|error| error.to_string())?,
        };
        let Some(resolved) = resolved else { return Ok(None) };
        let base_url = resolved.auth.base_url.clone().unwrap_or_else(|| provider.base_url.to_owned());
        if base_url.trim_end_matches('/') != provider.base_url.trim_end_matches('/') {
            return Ok(None);
        }
        Ok(Some((base_url, resolved.auth, signed_in)))
    }

    /// Reads the provider's model list from the provider, as it came; None
    /// when there's none to read (`fetches_models`, or not signed in).
    pub async fn fetch_models(&self, id: &str) -> Result<Option<Vec<u8>>, String> {
        if !self.fetches_models(id) {
            return Ok(None);
        }
        // A QA build's list from qa-model: public, like openrouter.ai's.
        if id == "openrouter" && crate::providers::catalog::qa_models() {
            return crate::providers::catalog::fetch_openrouter().await.map(Some);
        }
        let Some((base_url, auth, signed_in)) = self.listing_auth(id).await? else { return Ok(None) };
        let provider = self.provider(id).ok_or("no such provider")?;
        let listing = |base_url: String, auth: RequestAuth| -> futures_util::future::BoxFuture<'static, Result<Vec<u8>, Error>> {
            let id = provider.id;
            Box::pin(async move { crate::providers::listing::fetch(id, &base_url, &auth).await })
        };
        // A sign-in's token the provider didn't take is refreshed once, as for usage reads.
        let fetched = match signed_in {
            Some(account) => self.calling(provider, &account, base_url, auth, listing).await,
            None => listing(base_url, auth).await,
        };
        fetched.map(Some).map_err(|error| error.to_string())
    }

    /// Takes a list `fetch_models` brought, checked whole, in place of the
    /// bundled one: whether the provider's models changed. One that fails
    /// its checks changes nothing.
    pub fn take_models(&self, id: &str, body: &[u8]) -> Result<bool, String> {
        if !self.fetches_models(id) {
            return Ok(false);
        }
        let models = crate::providers::listing::parse(id, body)?;
        Ok(self.set_fetched_models(id, models))
    }

    /// Asks the provider whether it has the model `model` (its own id) for
    /// this sign-in, and an agent can run on it: the model to add, or why
    /// not in a sentence to show. None when it can't be asked here (a
    /// provider pointed elsewhere).
    pub async fn check_model(&self, id: &str, model: &str) -> Result<Option<Model>, String> {
        if !self.fetches_models(id) {
            return Ok(None);
        }
        let who = crate::providers::listing::name(id);
        if id == "openrouter" && crate::providers::catalog::qa_models() {
            let body = crate::providers::catalog::fetch_openrouter().await?;
            return crate::providers::catalog::openrouter_model(&body, model).map(Some);
        }
        let Some((base_url, auth, _)) = self.listing_auth(id).await? else {
            return Err(format!("Sign in to {who} first, in Settings → Providers."));
        };
        crate::providers::listing::check(id, &base_url, &auth, model).await.map(Some)
    }

    /// Adds a model `check_model` found to the provider's list; whether it's
    /// new.
    pub fn add_model(&self, provider: &str, model: Model) -> bool {
        let Some(provider) = self.provider(provider) else { return false };
        let mut added = self.inner.added.lock();
        let models = added.entry(provider.id.to_owned()).or_default();
        if models.contains(&model) {
            return false;
        }
        models.retain(|known| known.id != model.id);
        models.push(model);
        true
    }

    /// Takes `models`, fetched from the provider, in place of its bundled
    /// list (and of any fetched before); whether its models changed.
    pub fn set_fetched_models(&self, provider: &str, models: Vec<Model>) -> bool {
        let Some(provider) = self.provider(provider) else { return false };
        let same = match self.inner.fetched.lock().get(provider.id) {
            Some(fetched) => fetched.as_slice() == models.as_slice(),
            None => provider.models.as_slice() == models.as_slice(),
        };
        if same {
            return false;
        }
        self.inner.fetched.lock().insert(provider.id.to_owned(), Arc::new(models));
        true
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
            let subscribed = match self.check_auth(provider.id).await {
                Ok(Some(check)) => check.kind == "oauth",
                Ok(None) => continue,
                // One sign-in that can't be read (edited by hand, say) takes
                // only its own provider's models away.
                Err(error) => {
                    tracing::warn!(provider = provider.id, %error, "its sign-in can't be read: its models aren't offered");
                    continue;
                }
            };
            for model in self.models_of(provider).iter() {
                let image_input = model.input.iter().any(|input| input == "image");
                models.push(ModelInfo {
                    id: format!("{}/{}", provider.id, model.id),
                    name: model.name.clone(),
                    context_window: model.context_window,
                    efforts: supported_thinking_levels(model),
                    image_input,
                    image_output: image_input && subscribed && provider.images.is_some(),
                    added: self.is_added(provider, &model.id),
                });
            }
        }
        Ok(models)
    }

    /// The stored credential's type and source, without touching the network except for a due refresh.
    pub async fn check_auth(&self, id: &str) -> Result<Option<AuthCheck>, Error> {
        let Some(provider) = self.provider(id) else { return Ok(None) };
        let first = match self.inner.store.read(id).await {
            // Locked away: signed in all the same, as its outline says.
            Err(error) if error.is_locked() => {
                let outlined = credential_result(self.inner.store.locked_accounts(id).await, "read", id)?;
                return Ok(match outlined.first().map(|account| &account.credential) {
                    Some(Credential::Oauth { .. }) => {
                        provider.oauth.as_ref().map(|_| AuthCheck { source: Some("OAuth".into()), kind: "oauth" })
                    }
                    Some(Credential::ApiKey { .. }) => {
                        provider.api_key.as_ref().map(|_| AuthCheck { source: Some("stored credential".into()), kind: "api_key" })
                    }
                    None => None,
                });
            }
            read => credential_result(read, "read", id)?,
        };
        if let Some(Credential::Oauth { .. }) = first {
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
        let accounts = self.outlined_candidates(id).await?;
        // As shown: the normal pool, which is what most models draw on.
        let in_use = with_capacity(&accounts, &[], Pool::Normal, now).map(|(position, _)| position);
        let fable = in_fable_pool(id, "claude-fable");
        let fable_in_use = with_capacity(&accounts, &[], Pool::Fable, now).map(|(position, _)| position).filter(|_| fable);
        Ok(accounts
            .into_iter()
            .enumerate()
            .map(|(position, account)| AccountInfo {
                label: shown_label(&account),
                kind: account.credential.kind(),
                exhausted_until: if account.exhausted_until > now { account.exhausted_until } else { 0 },
                out_of_credit: account.out_of_credit_since != 0,
                sign_in_again: account.refused_since != 0,
                stored: account.id != ENV_ACCOUNT,
                in_use: in_use == Some(position),
                fable_exhausted_until: if fable && account.fable_exhausted_until > now { account.fable_exhausted_until } else { 0 },
                fable_in_use: fable_in_use == Some(position),
                id: account.id,
            })
            .collect())
    }

    /// Whether a request for `model` (an id of `provider`'s) would go to one
    /// of the provider's accounts now: it has accounts, and one with
    /// capacity for the model's pool. False with none listed, so a request
    /// refused there isn't asked again at once.
    pub async fn room_for(&self, provider: &str, model: &str) -> bool {
        let Ok(accounts) = self.candidates(provider).await else { return false };
        !accounts.is_empty() && with_capacity(&accounts, &[], Pool::of(provider, model), now_ms()).is_some()
    }

    /// When a request next tries one of the provider's keys out of credit
    /// again, by itself (`CREDIT_RETRY_MS` after it ran out): the earliest,
    /// in ms since 1970; None when none is out of credit.
    pub async fn credit_retry_at(&self, id: &str) -> Result<Option<u64>, Error> {
        let accounts = self.outlined_candidates(id).await?;
        Ok(accounts
            .iter()
            .filter(|account| account.out_of_credit_since != 0 && account.refused_since == 0)
            .map(|account| account.out_of_credit_since + CREDIT_RETRY_MS)
            .min())
    }

    /// The provider's accounts in the order requests try them: those stored,
    /// then a key in the environment.
    async fn candidates(&self, id: &str) -> Result<Vec<Account>, Error> {
        let stored = credential_result(self.inner.store.accounts(id).await, "read", id)?;
        Ok(self.with_env_key(id, stored))
    }

    /// `candidates` to show: with those locked away, as far as they're known.
    async fn outlined_candidates(&self, id: &str) -> Result<Vec<Account>, Error> {
        let stored = credential_result(self.inner.store.outlined_accounts(id).await, "read", id)?;
        Ok(self.with_env_key(id, stored))
    }

    /// `accounts`, then a key in the environment.
    fn with_env_key(&self, id: &str, mut accounts: Vec<Account>) -> Vec<Account> {
        if let Some((variable, key)) = self.env_key(id) {
            let state = self.inner.env_state.lock().get(id).copied().unwrap_or_default();
            accounts.push(Account {
                id: ENV_ACCOUNT.into(),
                label: variable,
                identity: String::new(),
                exhausted_until: state.exhausted_until,
                fable_exhausted_until: state.fable_exhausted_until,
                opus_exhausted_until: state.opus_exhausted_until,
                sonnet_exhausted_until: state.sonnet_exhausted_until,
                out_of_credit_since: state.out_of_credit_since,
                refused_since: 0,
                credential: Credential::ApiKey { key, extra: Map::new() },
            });
        }
        accounts
    }

    /// Refuses to change the provider's accounts as a whole while some are
    /// locked away, or `account` when it is: they're changed once they open.
    async fn not_locked(&self, id: &str, account: Option<&str>) -> Result<(), Error> {
        let locked = credential_result(self.inner.store.locked_accounts(id).await, "read", id)?;
        let held = match account {
            Some(account) => locked.iter().any(|stored| stored.id == account),
            None => !locked.is_empty(),
        };
        if held { Err(crate::credentials::locked_error(id)) } else { Ok(()) }
    }

    /// The provider's key in the environment: its variable and the key.
    fn env_key(&self, id: &str) -> Option<(String, String)> {
        let api_key = self.provider(id)?.api_key.as_ref()?;
        let resolved = api_key.resolve(&self.inner.env, None).ok()??;
        Some((resolved.source, resolved.auth.api_key?))
    }

    /// Changes the first stored account with this id under the store's lock;
    /// a missing account leaves the store untouched.
    async fn modify_account(&self, id: &str, account: String, change: impl FnOnce(&mut Account) + Send) -> Result<(), Error> {
        let changed = self
            .inner
            .store
            .modify_accounts(id, |mut accounts| async move {
                let Some(stored) = accounts.iter_mut().find(|stored| stored.id == account) else { return Ok(None) };
                change(stored);
                Ok(Some(accounts))
            })
            .await;
        credential_result(changed, "modify", id).map(drop)
    }

    /// Changes an account's limit mark for `pool` alone (`mark_until`) and
    /// its credit state, stored or not; returns the provider's accounts then.
    async fn set_state(
        &self,
        id: &str,
        account: &str,
        pool: Pool,
        change: impl FnOnce(&mut u64, &mut u64) + Send,
    ) -> Result<Vec<Account>, Error> {
        if account == ENV_ACCOUNT {
            {
                let mut states = self.inner.env_state.lock();
                let state = states.entry(id.to_owned()).or_default();
                let until = match pool {
                    Pool::Normal => &mut state.exhausted_until,
                    Pool::Fable => &mut state.fable_exhausted_until,
                    Pool::Opus => &mut state.opus_exhausted_until,
                    Pool::Sonnet => &mut state.sonnet_exhausted_until,
                };
                change(until, &mut state.out_of_credit_since);
            }
            return self.candidates(id).await;
        }
        let account = account.to_owned();
        self.modify_account(id, account, move |stored| {
            let until = match pool {
                Pool::Normal => &mut stored.exhausted_until,
                Pool::Fable => &mut stored.fable_exhausted_until,
                Pool::Opus => &mut stored.opus_exhausted_until,
                Pool::Sonnet => &mut stored.sonnet_exhausted_until,
            };
            change(until, &mut stored.out_of_credit_since);
        })
        .await?;
        self.candidates(id).await
    }

    /// Marks one of the provider's stored sign-ins as refused by the
    /// provider: skipped until it's signed in to again.
    async fn refuse(&self, id: &str, account: &str) -> Result<(), Error> {
        let account = account.to_owned();
        self.modify_account(id, account, |stored| stored.refused_since = now_ms()).await
    }

    /// Forgets that one of the provider's accounts (None: all of them) is at
    /// its usage limits (every pool's) or out of credit, so the next request
    /// asks the provider itself again: for when usage may be back without a
    /// sign-in changing, such as a reset claimed on the provider's own site
    /// or one the provider applied late. True when any account was marked. A
    /// wrong guess costs one refused request, which marks the account spent
    /// again with the provider's fresh reset time.
    pub async fn forget_spent(&self, id: &str, account: Option<&str>) -> Result<bool, Error> {
        self.forget_marks(id, account, |_| true, true).await
    }

    /// `forget_spent` for one account by its usage read now: each pool's
    /// mark whose limits have room in it (`full`: the ids of the windows at
    /// 100%) is forgotten, the others kept, so one model's weekly limit
    /// doesn't hold the rest of the account back. Its credit is left as it
    /// is: an account with usage limits has none to run out of.
    pub async fn forget_spent_with_room(&self, id: &str, account: Option<&str>, full: &[String]) -> Result<bool, Error> {
        let room = |pool: Pool| match pool {
            // A model's own mark: its own limit has room. The shared
            // limits' mark holds its requests back on its own.
            Pool::Opus | Pool::Sonnet => !full.iter().any(|window| Some(window.as_str()) == pool.own_window()),
            Pool::Normal | Pool::Fable => !full.iter().any(|window| pool.limited_by(window)),
        };
        self.forget_marks(id, account, room, false).await
    }

    async fn forget_marks(&self, id: &str, account: Option<&str>, room: impl Fn(Pool) -> bool, credit: bool) -> Result<bool, Error> {
        let now = now_ms();
        let accounts = self.candidates(id).await?;
        let mut any = false;
        for stored in accounts {
            if account.is_some_and(|wanted| wanted != stored.id) {
                continue;
            }
            if credit && stored.out_of_credit_since != 0 {
                self.set_state(id, &stored.id, Pool::Normal, |_, since| *since = 0).await?;
                any = true;
            }
            for pool in [Pool::Normal, Pool::Fable, Pool::Opus, Pool::Sonnet] {
                if mark_until(&stored, pool) > now && room(pool) {
                    self.set_state(id, &stored.id, pool, |until, _| *until = 0).await?;
                    any = true;
                }
            }
        }
        Ok(any)
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
            self.modify_account(id, wanted, move |stored| stored.label = label).await?;
        }
        if retry {
            self.set_state(id, account, Pool::Normal, |_, since| *since = 0).await?;
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
        let Some((base_url, auth, stored)) = self.subscription_auth(provider, account).await? else { return Ok(None) };
        self.calling(provider, &stored, base_url, auth, limits).await.map(Some)
    }

    /// Uses one of the usage resets of one of the provider's accounts (None:
    /// its first), as its last usage read listed it: only ever at the
    /// person's word, since it can't be undone. A limit reset gives the
    /// account requests again at once. None when it isn't signed in with a
    /// subscription that has resets.
    pub async fn use_reset(&self, id: &str, account: Option<&str>, reset: UsageReset) -> Result<Option<ResetAnswer>, Error> {
        let Some(provider) = self.provider(id) else { return Ok(None) };
        let Some(use_reset) = provider.reset else { return Ok(None) };
        let Some((base_url, auth, _)) = self.subscription_auth(provider, account).await? else { return Ok(None) };
        let answer = use_reset(base_url, auth, reset).await?;
        if answer.reset {
            let stored = credential_result(self.inner.store.accounts(id).await, "read", id)?;
            let reset_account = match account {
                Some(account) => stored.iter().find(|stored| stored.id == account),
                None => stored.first(),
            };
            if let Some(reset_account) = reset_account.map(|stored| stored.id.clone()) {
                // A reset refills limits; which pools have room again, the
                // next request finds out, so every pool gets to try.
                for pool in [Pool::Normal, Pool::Fable, Pool::Opus, Pool::Sonnet] {
                    self.set_state(id, &reset_account, pool, |until, _| *until = 0).await?;
                }
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
        let Some((base_url, auth, signed_in)) = self.subscription_auth(provider, account).await? else { return Ok(None) };
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
        self.calling(provider, &signed_in, base_url, auth, plan).await.map(Some)
    }

    async fn name_account(&self, id: &str, account: &str, identity: crate::limits::Identity) -> Result<(), Error> {
        let account = account.to_owned();
        self.modify_account(id, account, move |stored| {
            // Named meanwhile (the person renamed it while this asked): theirs stays.
            if stored.label.is_empty() {
                stored.label = identity.label;
            }
            stored.identity = identity.key;
        })
        .await
    }

    /// Whether the provider says what its keys have to spend (`account_credit`).
    pub fn reads_credit(&self, id: &str) -> bool {
        self.provider(id).is_some_and(|provider| provider.credit.is_some())
    }

    /// What one of the provider's accounts has to spend, as its provider
    /// says (#343 U8: OpenRouter's GET /key), read now: None when the
    /// provider can't say, it has no such account, or its sign-ins are
    /// locked away. `ENV_ACCOUNT` is the key in the environment. Why a read
    /// failed is a short sentence with no body of the provider's answer.
    pub async fn account_credit(&self, id: &str, account: &str) -> Result<Option<crate::providers::keys::KeyCredit>, Error> {
        let Some(provider) = self.provider(id) else { return Ok(None) };
        let Some(credit) = provider.credit else { return Ok(None) };
        let resolved = match (account, &provider.api_key) {
            (ENV_ACCOUNT, Some(api_key)) => api_key.resolve(&self.inner.env, None)?,
            _ => {
                let account = Some(account).filter(|account| !account.is_empty());
                match resolve_account_auth(provider, &self.inner.store, &self.inner.env, account, &CancellationToken::new()).await {
                    Err(error) if error.is_locked() => return Ok(None),
                    resolved => resolved?,
                }
            }
        };
        let Some(resolved) = resolved else { return Ok(None) };
        let base_url = resolved.auth.base_url.clone().unwrap_or_else(|| provider.base_url.to_owned());
        credit(base_url, resolved.auth).await.map(Some).map_err(Error::Other)
    }

    /// Makes one image on the provider (#403), on the account its requests
    /// go to now (the first with room, else its first), signed in with its
    /// subscription. Never sent again by itself.
    pub async fn make_image(
        &self,
        id: &str,
        request: crate::images::ImageRequest,
    ) -> Result<crate::images::MadeImage, crate::images::ImageFailure> {
        use crate::images::ImageFailure;
        let Some(provider) = self.provider(id) else { return Err(ImageFailure::NotOffered) };
        let Some(generate) = provider.images else { return Err(ImageFailure::NotOffered) };
        let accounts = match self.candidates(id).await {
            Ok(accounts) => accounts,
            Err(error) if error.is_locked() => return Err(ImageFailure::SignedOut),
            Err(error) => return Err(ImageFailure::Refused(error.to_string())),
        };
        let account = with_capacity(&accounts, &[], Pool::Normal, now_ms()).map(|(_, account)| account).or_else(|| accounts.first());
        let Some(account) = account else { return Err(ImageFailure::SignedOut) };
        if !matches!(account.credential, Credential::Oauth { .. }) {
            return Err(ImageFailure::NotOffered);
        }
        let resolved =
            resolve_account_auth(provider, &self.inner.store, &self.inner.env, Some(&account.id), &CancellationToken::new()).await;
        let resolved = match resolved {
            Ok(Some(resolved)) => resolved,
            Ok(None) => return Err(ImageFailure::SignedOut),
            Err(error) if error.is_locked() => return Err(ImageFailure::SignedOut),
            Err(error) => return Err(ImageFailure::Refused(error.to_string())),
        };
        let base_url = resolved.auth.base_url.clone().unwrap_or_else(|| provider.base_url.to_owned());
        generate(base_url, resolved.auth, request).await
    }

    /// The base url and auth of a provider's account signed in with a
    /// subscription, and the account's id; None when it isn't.
    async fn subscription_auth(&self, provider: &Provider, account: Option<&str>) -> Result<Option<(String, RequestAuth, String)>, Error> {
        let Some(oauth) = &provider.oauth else { return Ok(None) };
        let accounts = match self.inner.store.accounts(provider.id).await {
            // Nothing to read until they open.
            Err(error) if error.is_locked() => return Ok(None),
            read => credential_result(read, "read", provider.id)?,
        };
        let stored = match account {
            Some(account) => accounts.iter().find(|stored| stored.id == account),
            None => accounts.first(),
        };
        if !oauth.is_subscription() || !matches!(stored, Some(Account { credential: Credential::Oauth { .. }, .. })) {
            return Ok(None);
        }
        // Refused: nothing to read until it's signed in to again, and no
        // token endpoint asked again meanwhile.
        let Some(stored) = stored.filter(|stored| stored.refused_since == 0) else { return Ok(None) };
        let stored = stored.id.clone();
        let resolution = match resolve_account_auth(provider, &self.inner.store, &self.inner.env, account, &CancellationToken::new()).await
        {
            // Found by a usage read rather than a request: the same refusal.
            Err(error) if error.is_refused() => {
                tracing::warn!(provider = provider.id, account = %stored, %error, "the provider refused to refresh a sign-in; skipping it until it's signed in to again");
                self.refuse(provider.id, &stored).await?;
                return Err(error);
            }
            resolved => resolved?,
        };
        let Some(resolution) = resolution else { return Ok(None) };
        let base_url = resolution.auth.base_url.clone().unwrap_or_else(|| provider.base_url.to_owned());
        Ok(Some((base_url, resolution.auth, stored)))
    }

    /// Calls `call` with a sign-in's base url and auth. When the provider
    /// doesn't take its access token (401: expired before its time, or
    /// ended), the sign-in is refreshed once (`renew`) and `call` asked again
    /// with the new token; only a refresh the provider refuses, or a new
    /// token it doesn't take either, marks the sign-in to sign in to again.
    async fn calling<T>(
        &self,
        provider: &Provider,
        account: &str,
        base_url: String,
        auth: RequestAuth,
        call: impl Fn(String, RequestAuth) -> futures_util::future::BoxFuture<'static, Result<T, Error>>,
    ) -> Result<T, Error> {
        let used = auth.api_key.clone().unwrap_or_default();
        let error = match call(base_url, auth).await {
            Err(error) if error.is_unauthorized() => error,
            other => return other,
        };
        tracing::warn!(provider = provider.id, %account, %error, "the provider didn't take a sign-in's access token; refreshing it");
        let (base_url, auth) = self.renew(provider, account, &used, &error).await?;
        match call(base_url, auth).await {
            Err(again) if again.is_unauthorized() => {
                tracing::warn!(provider = provider.id, %account, error = %again, "the provider didn't take a refreshed sign-in either; it needs signing in again");
                self.refuse(provider.id, account).await?;
                let message = format!("{} didn't take this sign-in, even refreshed ({again}). Sign in again.", self.noun(provider.id));
                Err(Error::Models { code: crate::errors::REFUSED, message })
            }
            other => other,
        }
    }

    /// Refreshes a sign-in whose access token `used` the provider didn't
    /// take (`refused`, its answer), once, whatever its stored expiry
    /// (`auth::refresh_after_refusal`: single-flight, so parallel callers
    /// spend the refresh token once). Its new base url and auth; a refresh
    /// the provider refuses marks the sign-in to sign in to again, and says
    /// why (`REFUSED`). A refresh that fails otherwise (no network) changes
    /// nothing.
    async fn renew(&self, provider: &Provider, account: &str, used: &str, refused: &Error) -> Result<(String, RequestAuth), Error> {
        match crate::auth::refresh_after_refusal(provider, &self.inner.store, account, used, &CancellationToken::new()).await {
            Ok(Some(resolved)) => {
                let base_url = resolved.auth.base_url.clone().unwrap_or_else(|| provider.base_url.to_owned());
                Ok((base_url, resolved.auth))
            }
            // Signed out, or out of the store, meanwhile.
            Ok(None) => Err(Error::Models { code: crate::errors::UNAUTHORIZED, message: refused.to_string() }),
            Err(error) if error.is_refused() => {
                let outcome = crate::auth::refresh_outcome(&error);
                tracing::warn!(provider = provider.id, %account, %outcome, "the provider refused to refresh a sign-in; skipping it until it's signed in to again");
                self.refuse(provider.id, account).await?;
                let message = format!(
                    "{} stopped taking this sign-in, and refused to refresh it ({outcome}). Sign in again.",
                    self.noun(provider.id)
                );
                Err(Error::Models { code: crate::errors::REFUSED, message })
            }
            Err(error) => Err(error),
        }
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
        // Signed in again, perhaps on another plan: whose it is is read anew.
        self.forget_whose(id);
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
            .map_err(|error| match error {
                // Said for the person already (kept only in the vault, which is locked).
                Error::Models { .. } => error,
                other => models_error("auth", format!("Credential store modify failed for {id}"), Some(&other)),
            })?;
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
        self.delete_account_files(id, None).await;
        self.forget_whose(id);
        let signed_in = self.inner.store.accounts(id).await.unwrap_or_default();
        credential_result(self.inner.store.delete(id).await, "delete", id)?;
        self.revoke(id, signed_in);
        Ok(())
    }

    /// Ends `accounts`' sign-ins at the provider, signed out of here
    /// already, where it has a way (`OAuthFlow::revoke`): in the
    /// background, and one it can't end now is only logged.
    fn revoke(&self, id: &str, accounts: Vec<Account>) {
        let Some(oauth) = self.provider(id).and_then(|provider| provider.oauth.clone()) else { return };
        let Ok(runtime) = tokio::runtime::Handle::try_current() else { return };
        for account in accounts.into_iter().filter(|account| matches!(account.credential, Credential::Oauth { .. })) {
            let (oauth, id) = (oauth.clone(), id.to_owned());
            runtime.spawn(async move {
                match tokio::time::timeout(std::time::Duration::from_secs(30), oauth.revoke(&account.credential)).await {
                    Ok(Ok(())) => {}
                    Ok(Err(error)) => {
                        tracing::warn!(provider = %id, account = %account.id, %error, "could not end a sign-in at the provider")
                    }
                    Err(_) => tracing::warn!(provider = %id, account = %account.id, "ending a sign-in at the provider took too long"),
                }
            });
        }
    }

    /// Signs out of one of the provider's accounts; an empty `account`, all.
    pub async fn logout_account(&self, id: &str, account: &str) -> Result<(), Error> {
        if account.is_empty() {
            return self.logout(id).await;
        }
        self.not_locked(id, Some(account)).await?;
        self.delete_account_files(id, Some(account)).await;
        self.forget_whose(id);
        let account = account.to_owned();
        let signed_in: Vec<Account> =
            self.inner.store.accounts(id).await.unwrap_or_default().into_iter().filter(|stored| stored.id == account).collect();
        let removed = self
            .inner
            .store
            .modify_accounts(id, |mut accounts| async move {
                let before = accounts.len();
                accounts.retain(|stored| stored.id != account);
                Ok((accounts.len() != before).then_some(accounts))
            })
            .await;
        credential_result(removed, "modify", id)?;
        self.revoke(id, signed_in);
        Ok(())
    }

    /// Moves one of the provider's accounts to `position` (0 is first, past
    /// the end is last) in the order requests try them.
    pub async fn move_account(&self, id: &str, account: &str, position: usize) -> Result<(), Error> {
        if account == ENV_ACCOUNT {
            return Err(models_error("auth", "A key from the environment comes after the accounts signed in", None));
        }
        self.not_locked(id, None).await?;
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
    /// (`Pool::Normal`: the shared limits) until it resets: when the
    /// provider said, or else when its usage read now says the pool's full
    /// limits do, or else in a while. Returns the provider's accounts then.
    async fn exhaust(&self, id: &str, account: &str, pool: Pool, resets_at: Option<u64>) -> Result<Vec<Account>, Error> {
        let now = now_ms();
        let resets_at = match resets_at.filter(|at| *at > now) {
            Some(at) => Some(at),
            None => match self.account_usage(id, Some(account)).await {
                Ok(Some(usage)) => usage
                    .limits
                    .iter()
                    .filter(|limit| limit.used_percent >= 100.0)
                    // Only limits the mark stands for. Fable requests pass
                    // the 5-hour gate, the weekly one (it applies across
                    // all models, Anthropic says) and Fable's own; an Opus
                    // or Sonnet mark is its own weekly limit's alone; the
                    // shared mark every limit but the models' own.
                    .filter(|limit| match pool {
                        Pool::Opus | Pool::Sonnet => Some(limit.id.as_str()) == pool.own_window(),
                        Pool::Normal | Pool::Fable => pool.limited_by(&limit.id),
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
        self.set_state(id, account, pool, move |exhausted_until, _| *exhausted_until = until).await
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
        // Kept per account: a file is its account's alone.
        let image_files = match &options.image_files {
            Some(_) => None,
            None => self.files_for(provider, account, &request_model.base_url, &resolution.auth).await,
        };
        let request_options = StreamOptions {
            api_key: options.api_key.or(resolution.auth.api_key),
            headers: merge_headers(&resolution.auth.headers, &options.headers),
            image_files: options.image_files.or(image_files),
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
                let pool = Pool::of_model(&model);
                let mut tried = Vec::new();
                // Sign-ins refreshed after the provider didn't take their
                // token: each once per request.
                let mut renewed: Vec<String> = Vec::new();
                loop {
                    let accounts = match registry.candidates(&model.provider).await {
                        Ok(accounts) => accounts,
                        // The sign-ins are locked away with the person's
                        // secrets: the request waits until they open.
                        Err(error) if error.is_locked() => {
                            let mut event = failed(&model, error.to_string());
                            if let StreamEvent::Error { error, .. } = &mut event {
                                mark_none_left(error, None);
                            }
                            sink.push(event);
                            return;
                        }
                        Err(error) => {
                            sink.push(failed(&model, error.to_string()));
                            return;
                        }
                    };
                    // Signed out of every account, and no key from the
                    // environment: the request waits for a sign-in (the
                    // person signing in again, say) rather than failing.
                    if accounts.is_empty()
                        && registry.provider(&model.provider).is_some()
                        && matches!(registry.account_auth(&model, None).await, Ok(None))
                    {
                        let mut event = failed(&model, format!("You're signed out of {}.", registry.noun(&model.provider)));
                        if let StreamEvent::Error { error, .. } = &mut event {
                            mark_none_left(error, None);
                        }
                        sink.push(event);
                        return;
                    }
                    let chosen = with_capacity(&accounts, &tried, pool, now_ms()).map(|(_, account)| account.clone());
                    if chosen.is_none() && !accounts.is_empty() {
                        sink.push(registry.none_left(&model, &accounts));
                        return;
                    }
                    let account = chosen.as_ref().map(|account| account.id.clone());
                    let mut inner = match registry.open(&model, context.clone(), options.clone(), account.as_deref()).await {
                        Ok(inner) => inner,
                        // A stored sign-in whose token the provider won't refresh: only
                        // signing in again helps, so it's skipped until then, and the
                        // request goes to the next account (rivet-move.md §3 P2).
                        Err(error)
                            if error.is_refused()
                                && chosen.as_ref().is_some_and(|chosen| matches!(chosen.credential, Credential::Oauth { .. }))
                                && account.as_deref().is_some_and(|account| account != ENV_ACCOUNT) =>
                        {
                            let account = account.clone().unwrap_or_default();
                            tracing::warn!(provider = %model.provider, %account, %error, "the provider refused to refresh a sign-in; skipping it until it's signed in to again");
                            if let Err(error) = registry.refuse(&model.provider, &account).await {
                                tracing::warn!(provider = %model.provider, %error, "could not mark a sign-in as refused");
                                let mut event = failed(&model, error.to_string());
                                if let StreamEvent::Error { error, .. } = &mut event {
                                    mark_refused(error, &refused_by(chosen.as_ref()));
                                }
                                sink.push(event);
                                return;
                            }
                            if let Some(notes) = &notes {
                                notes(AccountNote::Changed);
                            }
                            tried.push(account);
                            continue;
                        }
                        Err(error) => {
                            let mut event = failed(&model, error.to_string());
                            // A key the provider won't take.
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
                    // The provider didn't take a sign-in's access token
                    // before anything came: refreshed once, and asked again.
                    let renewable = chosen
                        .as_ref()
                        .filter(|chosen| matches!(chosen.credential, Credential::Oauth { .. }) && !renewed.contains(&chosen.id))
                        .map(|chosen| chosen.id.clone());
                    let mut unauthorized = None;
                    while let Some(mut event) = inner.next().await {
                        if !forwarding {
                            match &event {
                                StreamEvent::Start { .. } => {
                                    held = Some(event);
                                    continue;
                                }
                                StreamEvent::Error { error, .. } if renewable.is_some() && refused(error).is_some() => {
                                    unauthorized = Some(event);
                                    break;
                                }
                                StreamEvent::Error { error, .. } if account.is_some() && spent(error, pool).is_some() => {
                                    gone = spent(error, pool);
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
                    if let (Some(mut event), Some(renewing), Some(chosen)) = (unauthorized, renewable, chosen.as_ref()) {
                        renewed.push(renewing.clone());
                        let Credential::Oauth { access, .. } = &chosen.credential else { return };
                        let provider = registry.provider(&model.provider).expect("the model's provider");
                        let said = match &event {
                            StreamEvent::Error { error, .. } => error.error_message.clone().unwrap_or_default(),
                            _ => String::new(),
                        };
                        tracing::warn!(provider = %model.provider, account = %renewing, error = %said, "the provider didn't take a sign-in's access token; refreshing it");
                        let refused_token = Error::Models { code: crate::errors::UNAUTHORIZED, message: said };
                        match registry.renew(provider, &renewing, access, &refused_token).await {
                            // Asked again on the same account, with its new token.
                            Ok(_) => continue,
                            // Refused: marked to sign in to again; the next account, if any.
                            Err(error) if error.is_refused() => {
                                if let Some(notes) = &notes {
                                    notes(AccountNote::Changed);
                                }
                                tried.push(renewing);
                                continue;
                            }
                            // No refresh to be had now (no network): the request fails as it came.
                            Err(error) => {
                                tracing::warn!(provider = %model.provider, account = %renewing, %error, "could not refresh a sign-in");
                                if let Some(start) = held {
                                    sink.push(start);
                                }
                                if let StreamEvent::Error { error, .. } = &mut event {
                                    mark_refused(error, &refused_by(Some(chosen)));
                                }
                                sink.push(event);
                                return;
                            }
                        }
                    }
                    let (Some(gone), Some(account)) = (gone, account) else {
                        if let Some(start) = held {
                            sink.push(start);
                        }
                        // A key out of credit that answered has some again.
                        if let (true, Some(chosen)) = (done, chosen.filter(|chosen| chosen.out_of_credit_since != 0)) {
                            match registry.set_state(&model.provider, &chosen.id, Pool::Normal, |_, since| *since = 0).await {
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
                        Spent::Limit(resets_at, marked) => registry.exhaust(&model.provider, &account, marked, resets_at).await,
                        Spent::Credit => registry.set_state(&model.provider, &account, Pool::Normal, |_, since| *since = now_ms()).await,
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
                        Spent::Limit(..) => {
                            let until = accounts.iter().find(|stored| stored.id == account).map_or(0, |stored| pool_until(stored, pool));
                            let resets = if until > now_ms() { crate::clock::iso_from_ms(until) } else { "unknown".to_owned() };
                            tracing::info!(provider = %model.provider, %account, %resets, "account reached its usage limit");
                            format!("usage limit, resets {resets}")
                        }
                        Spent::Credit => {
                            tracing::info!(provider = %model.provider, %account, "account ran out of credit");
                            "out of credit".to_owned()
                        }
                    };
                    let Some((position, next)) = with_capacity(&accounts, &tried, pool, now_ms()) else {
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
        // Every sign-in refused: only signing in again helps, and the
        // conversation offers it.
        if let Some(first) = accounts.first().filter(|_| accounts.iter().all(|account| account.refused_since != 0)) {
            let name = self.provider(&model.provider).map_or(model.provider.as_str(), |provider| provider.name);
            let mut event = failed(model, refused_message(name, &self.noun(&model.provider), accounts.len()));
            if let StreamEvent::Error { error, .. } = &mut event {
                mark_refused(error, &refused_by(Some(first)));
            }
            return event;
        }
        let now = now_ms();
        let pool = Pool::of_model(model);
        let until = accounts.iter().map(|account| pool_until(account, pool)).filter(|at| *at > now).min();
        let mut event = failed(model, self.exhausted(&model.provider, accounts, pool));
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
    fn exhausted(&self, id: &str, accounts: &[Account], pool: Pool) -> String {
        // A limit whose reset has passed holds nothing back: a key out of
        // credit since says only that.
        let now = now_ms();
        let first = accounts
            .iter()
            .enumerate()
            .filter(|(_, account)| pool_until(account, pool) > now)
            .min_by_key(|(_, account)| pool_until(account, pool));
        let first = first.map(|(position, account)| (account_name(&shown_label(account), position), pool_until(account, pool)));
        let credit = accounts.iter().any(|account| account.out_of_credit_since != 0);
        let keys = accounts.iter().all(|account| matches!(account.credential, Credential::ApiKey { .. }));
        // Said as the model's own weekly limit when that's what holds every
        // account back, not the shared ones.
        let own = match pool {
            Pool::Opus => "weekly Opus limit",
            Pool::Sonnet => "weekly Sonnet limit",
            Pool::Normal | Pool::Fable => "usage limit",
        };
        let held = accounts.iter().filter(|account| pool_until(account, pool) > now);
        let limit = if pool.claim().is_some() && held.clone().next().is_some() && held.clone().all(|account| account.exhausted_until <= now)
        {
            own
        } else {
            "usage limit"
        };
        exhausted_message(&self.noun(id), accounts.len(), first.as_ref().map(|(name, at)| (name.as_str(), *at)), credit, keys, limit)
    }
}
