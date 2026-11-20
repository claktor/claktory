use std::sync::Arc;
use std::time::Duration;

use tokio_util::sync::CancellationToken;

use crate::clock::now_ms;
use crate::credentials::{Credential, FileCredentialStore};
use crate::errors::{Error, models_error};
use crate::providers::Provider;

pub type AuthFuture<'a, T> = std::pin::Pin<Box<dyn std::future::Future<Output = T> + Send + 'a>>;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SelectOption {
    pub id: String,
    pub label: String,
    pub description: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AuthPrompt {
    Text { message: String, placeholder: Option<String> },
    Secret { message: String, placeholder: Option<String> },
    Select { message: String, options: Vec<SelectOption> },
    ManualCode { message: String, placeholder: Option<String> },
}

impl AuthPrompt {
    pub fn message(&self) -> &str {
        match self {
            AuthPrompt::Text { message, .. }
            | AuthPrompt::Secret { message, .. }
            | AuthPrompt::Select { message, .. }
            | AuthPrompt::ManualCode { message, .. } => message,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AuthLink {
    pub url: String,
    pub label: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AuthEvent {
    Info { message: String, links: Vec<AuthLink> },
    AuthUrl { url: String, instructions: Option<String> },
    DeviceCode { user_code: String, verification_uri: String, interval_seconds: Option<u64>, expires_in_seconds: Option<u64> },
    Progress { message: String },
}

/// `prompt` answers with the entered text or the selected option id; `cancel` aborts the whole login.
pub trait AuthInteraction: Send + Sync {
    fn prompt<'a>(&'a self, prompt: AuthPrompt) -> AuthFuture<'a, Result<String, Error>>;
    fn notify(&self, event: AuthEvent);
    fn cancel(&self) -> CancellationToken;
}

/// Request auth: a bearer key, extra headers, or a base url the credential dictates.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RequestAuth {
    pub api_key: Option<String>,
    pub headers: Vec<(String, String)>,
    pub base_url: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AuthResult {
    pub auth: RequestAuth,
    pub source: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AuthCheck {
    pub source: Option<String>,
    pub kind: &'static str,
}

pub type Env = Arc<dyn Fn(&str) -> Option<String> + Send + Sync>;

/// `process.env` with blank values treated as unset.
pub fn process_env() -> Env {
    Arc::new(|name| std::env::var(name).ok().filter(|value| !value.trim().is_empty()))
}

/// Prompts for a secret, guarding against cancellation before and after the prompt.
pub fn secret_login<'a>(interaction: &'a dyn AuthInteraction, message: String) -> AuthFuture<'a, Result<Credential, Error>> {
    let prompt = interaction.prompt(AuthPrompt::Secret { message, placeholder: None });
    let cancel = interaction.cancel();
    Box::pin(async move {
        if cancel.is_cancelled() {
            return Err(Error::Aborted);
        }
        let key = prompt.await?;
        if cancel.is_cancelled() {
            return Err(Error::Aborted);
        }
        Ok(Credential::ApiKey { key, extra: Default::default() })
    })
}

pub trait ApiKeyAuth: Send + Sync {
    fn name(&self) -> &str;
    fn login<'a>(&'a self, interaction: &'a dyn AuthInteraction) -> AuthFuture<'a, Result<Credential, Error>>;
    fn resolve(&self, env: &Env, credential: Option<&Credential>) -> Result<Option<AuthResult>, Error>;
}

/// A stored key wins, otherwise the first set environment variable. `base_url_var`, when set,
/// replaces the provider's base url.
pub struct EnvApiKeyAuth {
    pub name: &'static str,
    pub env_vars: &'static [&'static str],
    pub base_url_var: Option<&'static str>,
}

impl ApiKeyAuth for EnvApiKeyAuth {
    fn name(&self) -> &str {
        self.name
    }

    fn login<'a>(&'a self, interaction: &'a dyn AuthInteraction) -> AuthFuture<'a, Result<Credential, Error>> {
        secret_login(interaction, format!("Enter {}", self.name))
    }

    fn resolve(&self, env: &Env, credential: Option<&Credential>) -> Result<Option<AuthResult>, Error> {
        let base_url = self.base_url_var.and_then(|name| env(name));
        if let Some(Credential::ApiKey { key, .. }) = credential
            && !key.is_empty()
        {
            let auth = RequestAuth { api_key: Some(key.clone()), base_url, ..RequestAuth::default() };
            return Ok(Some(AuthResult { auth, source: "stored credential".into() }));
        }
        for name in self.env_vars {
            if let Some(value) = env(name) {
                let auth = RequestAuth { api_key: Some(value), base_url, ..RequestAuth::default() };
                return Ok(Some(AuthResult { auth, source: (*name).to_owned() }));
            }
        }
        Ok(None)
    }
}

/// Whether a token endpoint's error answer refuses the refresh token itself
/// (revoked, expired or used up), so only signing in again helps, rather than
/// failing for now: OAuth's `invalid_grant`, ChatGPT's `refresh_token_…`
/// codes, or a 401.
pub fn refresh_refused(status: u16, body: &str) -> bool {
    status == 401 || ((400..500).contains(&status) && (body.contains("invalid_grant") || body.contains("refresh_token_")))
}

/// A token endpoint's refusal of the refresh token, as `Error::Models` with
/// the `REFUSED` code; any other failure as it is.
pub fn refusal(refused: bool, message: String) -> Error {
    if refused { Error::Models { code: crate::errors::REFUSED, message } } else { Error::Other(message) }
}

pub trait OAuthFlow: Send + Sync {
    fn name(&self) -> &str;
    fn is_subscription(&self) -> bool;
    fn login<'a>(&'a self, interaction: &'a dyn AuthInteraction) -> AuthFuture<'a, Result<Credential, Error>>;
    fn refresh<'a>(&'a self, credential: Credential, cancel: CancellationToken) -> AuthFuture<'a, Result<Credential, Error>>;
    fn to_auth(&self, credential: &Credential) -> Result<RequestAuth, Error>;
}

const OAUTH_MINIMUM_VALIDITY_MS: u64 = 5 * 60 * 1000;
const OAUTH_REFRESH_TIMEOUT: Duration = Duration::from_secs(15);

fn expires_soon(credential: &Credential) -> bool {
    matches!(credential, Credential::Oauth { expires, .. } if now_ms() + OAUTH_MINIMUM_VALIDITY_MS >= *expires)
}

/// A stored credential owns the provider; the environment is consulted only when nothing is stored.
/// The provider's first account's, when it has several.
pub async fn resolve_provider_auth(
    provider: &Provider,
    store: &FileCredentialStore,
    env: &Env,
    cancel: &CancellationToken,
) -> Result<Option<AuthResult>, Error> {
    resolve_account_auth(provider, store, env, None, cancel).await
}

/// `resolve_provider_auth` for one of the provider's accounts (`account`, its
/// id), or its first (None). None when it has no such account.
pub async fn resolve_account_auth(
    provider: &Provider,
    store: &FileCredentialStore,
    env: &Env,
    account: Option<&str>,
    cancel: &CancellationToken,
) -> Result<Option<AuthResult>, Error> {
    if cancel.is_cancelled() {
        return Err(Error::Aborted);
    }
    let accounts = store
        .accounts(provider.id)
        .await
        .map_err(|error| models_error("auth", format!("Credential store read failed for {}", provider.id), Some(&error)))?;
    let stored = match account {
        Some(id) => match accounts.into_iter().find(|stored| stored.id == id) {
            Some(stored) => Some(stored),
            None => return Ok(None),
        },
        None => accounts.into_iter().next(),
    };
    let (account_id, stored) = stored.map(|stored| (stored.id, stored.credential)).unzip();
    match (stored, &provider.oauth, &provider.api_key) {
        (Some(credential @ Credential::Oauth { .. }), Some(oauth), _) => {
            resolve_stored_oauth(store, provider.id, account_id.as_deref().unwrap_or_default(), oauth.as_ref(), credential, cancel).await
        }
        (Some(credential @ Credential::ApiKey { .. }), _, Some(api_key)) => {
            resolve_api_key(provider.id, api_key.as_ref(), env, Some(&credential))
        }
        (Some(_), _, _) => Ok(None),
        (None, _, Some(api_key)) => resolve_api_key(provider.id, api_key.as_ref(), env, None),
        (None, _, None) => Ok(None),
    }
}

fn resolve_api_key(id: &str, api_key: &dyn ApiKeyAuth, env: &Env, credential: Option<&Credential>) -> Result<Option<AuthResult>, Error> {
    api_key.resolve(env, credential).map_err(|error| models_error("auth", format!("API key auth failed for provider {id}"), Some(&error)))
}

/// Tokens within five minutes of expiry refresh once under the store lock; the rotated credential is written before release.
async fn resolve_stored_oauth(
    store: &FileCredentialStore,
    id: &str,
    account: &str,
    oauth: &dyn OAuthFlow,
    stored: Credential,
    cancel: &CancellationToken,
) -> Result<Option<AuthResult>, Error> {
    let mut credential = stored;
    if expires_soon(&credential) {
        let post = store
            .modify_account(id, account, |current| async move {
                let Some(current) = current.filter(|current| matches!(current, Credential::Oauth { .. }) && expires_soon(current)) else {
                    return Ok(None);
                };
                match tokio::time::timeout(OAUTH_REFRESH_TIMEOUT, oauth.refresh(current, cancel.clone())).await {
                    Ok(Ok(refreshed)) => Ok(Some(refreshed)),
                    Ok(Err(error)) => {
                        let code = if error.is_refused() { crate::errors::REFUSED } else { "oauth" };
                        Err(models_error(code, format!("OAuth refresh failed for {id}"), Some(&error)))
                    }
                    Err(_) => {
                        let timeout = Error::Other("The operation was aborted due to timeout".into());
                        Err(models_error("oauth", format!("OAuth refresh failed for {id}"), Some(&timeout)))
                    }
                }
            })
            .await
            .map_err(|error| match error {
                Error::Models { .. } => error,
                other => models_error("auth", format!("Credential store modify failed for {id}"), Some(&other)),
            })?;
        let Some(post @ Credential::Oauth { .. }) = post else { return Ok(None) };
        credential = post;
    }
    let auth = oauth
        .to_auth(&credential)
        .map_err(|error| models_error("oauth", format!("OAuth auth derivation failed for {id}"), Some(&error)))?;
    Ok(Some(AuthResult { auth, source: "OAuth".into() }))
}
