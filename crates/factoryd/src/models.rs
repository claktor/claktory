//! Model providers: what agents can use and how Factory signs in to them.
//! Behind a trait so tests can script a provider.

use std::path::PathBuf;
use std::sync::Arc;

use factory_agent::auth::AuthInteraction;
use factory_agent::credentials::CredentialBacking;
use factory_agent::providers::models::Notes;
use factory_agent::{Error, Model, StreamFn};
use factory_protocol::v1;
use futures_util::future::BoxFuture;

pub const OAUTH: &str = "oauth";
pub const API_KEY: &str = "api_key";

pub trait Models: Send + Sync {
    /// Models whose provider is signed in: what agents can run on now.
    fn list(&self) -> BoxFuture<'_, Vec<v1::Model>>;
    /// Every provider, signed in or not, with its ways to sign in.
    fn providers(&self) -> BoxFuture<'_, Vec<v1::Provider>>;
    /// The model for a config id like "anthropic/claude-opus-5-5".
    fn resolve(&self, id: &str) -> Option<Model>;
    fn stream(&self) -> StreamFn;
    /// `stream`, telling `notes` what the person should know of a request,
    /// such as that it carried on with another account of its provider.
    fn stream_noting(&self, notes: Notes) -> StreamFn {
        let _ = notes;
        self.stream()
    }
    /// Runs a provider's sign-in, asking `interaction` for whatever it needs,
    /// and stores the credential: as another account with `add`, else in
    /// place of its first account (unless it's one already signed in).
    fn login<'a>(
        &'a self,
        provider: &'a str,
        method: &'a str,
        interaction: &'a dyn AuthInteraction,
        add: bool,
    ) -> BoxFuture<'a, Result<(), Error>>;
    /// Forgets the stored credential of one of its accounts; an empty
    /// `account`, all of them.
    fn logout<'a>(&'a self, provider: &'a str, account: &'a str) -> BoxFuture<'a, Result<(), Error>>;
    /// Moves one of a provider's accounts to `position` in the order
    /// requests try them.
    fn move_account<'a>(&'a self, provider: &'a str, account: &'a str, position: usize) -> BoxFuture<'a, Result<(), Error>> {
        let _ = (provider, account, position);
        Box::pin(async { Err(Error::Other("this provider has only one account".into())) })
    }
    /// Renames one of a provider's accounts (`label`; empty: the name
    /// Factory gives it) and, with `retry`, tries a key out of credit again.
    fn update_account<'a>(
        &'a self,
        provider: &'a str,
        account: &'a str,
        label: Option<&'a str>,
        retry: bool,
    ) -> BoxFuture<'a, Result<(), Error>> {
        let _ = (provider, account, label, retry);
        Box::pin(async { Err(Error::Other("this provider's accounts can't be changed".into())) })
    }
    /// Reads how much of its usage limits an account (`provider.accounts`'
    /// id; empty: the provider's first) signed in with a subscription has
    /// used; None when it isn't signed in that way.
    /// Its usage resets come with it; `updated` and `error` are left unset.
    fn usage<'a>(&'a self, _provider: &'a str, _account: &'a str) -> BoxFuture<'a, Result<Option<v1::ProviderUsage>, Error>> {
        Box::pin(async { Ok(None) })
    }
    /// Uses one of an account's usage resets, as its last usage read listed
    /// it, at the person's word: it can't be undone. A limit reset gives the
    /// account requests again at once. None when the account has no resets.
    fn use_reset<'a>(
        &'a self,
        _provider: &'a str,
        _account: &'a str,
        _reset: v1::UsageReset,
    ) -> BoxFuture<'a, Result<Option<v1::UsageResetAnswer>, Error>> {
        Box::pin(async { Ok(None) })
    }
    /// Reads which plan an account signed in with a subscription has, such
    /// as "Claude Max 20x"; empty when it isn't signed in that way or
    /// doesn't say.
    fn plan<'a>(&'a self, _provider: &'a str, _account: &'a str) -> BoxFuture<'a, Result<String, Error>> {
        Box::pin(async { Ok(String::new()) })
    }
    /// Whether the provider says what its keys have to spend (`account_credit`).
    fn reads_credit(&self, _provider: &str) -> bool {
        false
    }
    /// Reads what one of the provider's keys (`provider.accounts`' id;
    /// empty: its first) has to spend, as the provider says; None when it
    /// can't say (`crate::credit`).
    fn account_credit<'a>(
        &'a self,
        _provider: &'a str,
        _account: &'a str,
    ) -> BoxFuture<'a, Result<Option<factory_agent::providers::keys::KeyCredit>, Error>> {
        Box::pin(async { Ok(None) })
    }
    /// Makes one image on the provider (#403: `crate::extensions::images`),
    /// on the account its requests go to now.
    fn make_image<'a>(
        &'a self,
        _provider: &'a str,
        _request: factory_agent::images::ImageRequest,
    ) -> BoxFuture<'a, Result<factory_agent::images::MadeImage, factory_agent::images::ImageFailure>> {
        Box::pin(async { Err(factory_agent::images::ImageFailure::NotOffered) })
    }
    /// Forgets that one of the provider's accounts (empty: all of them) is
    /// at its usage limits or out of credit, so the next request asks the
    /// provider itself again: for when usage may be back without a sign-in
    /// changing. True when any account was marked.
    fn forget_spent<'a>(&'a self, _provider: &'a str, _account: &'a str) -> BoxFuture<'a, Result<bool, Error>> {
        Box::pin(async { Ok(false) })
    }
    /// Whether a request for `model` ("anthropic/claude-opus-4-8") would go
    /// to one of its provider's accounts now, judged by the model's own
    /// limits (an Opus-only one, say). False with no accounts listed.
    fn room_for<'a>(&'a self, _model: &'a str) -> BoxFuture<'a, bool> {
        Box::pin(async { false })
    }
    /// `forget_spent` for one account by its usage read: the marks of the
    /// pools whose limits aren't among `full` (the windows at 100%), so a
    /// model's own weekly limit doesn't hold the others back.
    fn forget_spent_with_room<'a>(
        &'a self,
        _provider: &'a str,
        _account: &'a str,
        _full: &'a [String],
    ) -> BoxFuture<'a, Result<bool, Error>> {
        Box::pin(async { Ok(false) })
    }
    /// The file its sign-ins are kept in, `auth.json`; None when they're
    /// kept no other way (a scripted provider).
    fn auth_path(&self) -> Option<PathBuf> {
        None
    }
    /// Keeps the sign-ins with `backing` from now on (the vault's).
    fn keep_sign_ins_with(&self, _backing: Arc<dyn CredentialBacking>) {}
    /// Takes in sign-ins kept elsewhere meanwhile (see
    /// `vault::signins`); whether any changed.
    fn take_in_sign_ins(&self) -> BoxFuture<'_, Result<bool, Error>> {
        Box::pin(async { Ok(false) })
    }
    /// Whether the provider's sign-ins are locked away with the person's
    /// secrets now, so its requests wait until they open.
    fn sign_ins_locked<'a>(&'a self, _provider: &'a str) -> BoxFuture<'a, bool> {
        Box::pin(async { false })
    }
    /// Whether the provider's model list is read from the provider itself
    /// (`crate::catalog`): at its own address, not a scripted one.
    fn fetches_models(&self, _provider: &str) -> bool {
        false
    }
    /// Reads the provider's model list from the provider, as it came; None
    /// when there's none to read (not signed in, or a scripted provider).
    /// A provider that didn't take the sign-in or key fails
    /// `is_unauthorized` or `is_refused`.
    fn fetch_models<'a>(&'a self, _provider: &'a str) -> BoxFuture<'a, Result<Option<Vec<u8>>, Error>> {
        Box::pin(async { Ok(None) })
    }
    /// Takes a model list `fetch_models` brought, checked whole, in place of
    /// the bundled one: whether the provider's models changed. One that
    /// fails its checks changes nothing.
    fn take_models(&self, _provider: &str, _body: &[u8]) -> Result<bool, String> {
        Ok(false)
    }
    /// Asks the provider whether it has the model `model` (its own id) for
    /// this sign-in and an agent can run on it: the model to add, as kept
    /// (`add_model`), or why not in a sentence to show. None when it can't
    /// be asked here.
    fn check_model<'a>(&'a self, _provider: &'a str, _model: &'a str) -> BoxFuture<'a, Result<Option<Vec<u8>>, String>> {
        Box::pin(async { Ok(None) })
    }
    /// Adds a model `check_model` found (or kept from before) to the
    /// provider's list; whether it's new.
    fn add_model(&self, _provider: &str, _model: &[u8]) -> Result<bool, String> {
        Ok(false)
    }
    /// When a request next tries one of the provider's keys out of credit
    /// again by itself, in ms since 1970; None when none is out of credit.
    fn credit_retry_at<'a>(&'a self, _provider: &'a str) -> BoxFuture<'a, Option<u64>> {
        Box::pin(async { None })
    }

    /// Deletes at the provider the files of the images of `digests`, which
    /// a conversation let go of (#382: Anthropic's).
    fn forget_images(&self, _digests: Vec<String>) -> BoxFuture<'_, ()> {
        Box::pin(async {})
    }
}

/// The digests of the images in `messages` that may have gone to a
/// provider as files (#382).
pub(crate) fn image_digests(messages: &[factory_agent::types::Message]) -> std::collections::HashSet<String> {
    messages
        .iter()
        .filter_map(|message| match message {
            factory_agent::types::Message::ToolResult(result) => Some(&result.content),
            _ => None,
        })
        .flatten()
        .filter_map(|part| match part {
            factory_agent::ToolResultPart::Image { digest, .. } => digest.clone(),
            factory_agent::ToolResultPart::Text { .. } => None,
        })
        .collect()
}

/// An account id as `Registry` takes it: None for the first.
fn account_arg(account: &str) -> Option<&str> {
    (!account.is_empty()).then_some(account)
}

pub use factory_agent::Registry;

impl Models for Registry {
    fn fetches_models(&self, provider: &str) -> bool {
        Registry::fetches_models(self, provider)
    }

    fn fetch_models<'a>(&'a self, provider: &'a str) -> BoxFuture<'a, Result<Option<Vec<u8>>, Error>> {
        Box::pin(Registry::fetch_models(self, provider))
    }

    fn take_models(&self, provider: &str, body: &[u8]) -> Result<bool, String> {
        Registry::take_models(self, provider, body)
    }

    fn check_model<'a>(&'a self, provider: &'a str, model: &'a str) -> BoxFuture<'a, Result<Option<Vec<u8>>, String>> {
        Box::pin(async move {
            let Some(model) = Registry::check_model(self, provider, model).await? else { return Ok(None) };
            serde_json::to_vec(&model).map(Some).map_err(|error| error.to_string())
        })
    }

    fn add_model(&self, provider: &str, model: &[u8]) -> Result<bool, String> {
        let model: Model = serde_json::from_slice(model).map_err(|error| format!("a kept model can't be read: {error}"))?;
        Ok(Registry::add_model(self, provider, model))
    }

    fn list(&self) -> BoxFuture<'_, Vec<v1::Model>> {
        Box::pin(async move {
            let available = match self.models().await {
                Ok(available) => available,
                Err(error) => {
                    tracing::warn!(%error, "could not read sign-ins");
                    return Vec::new();
                }
            };
            let mut models = Vec::new();
            for model in available {
                let mut efforts = Vec::new();
                for effort in model.efforts {
                    efforts.push(effort.as_str().to_owned());
                }
                models.push(v1::Model {
                    id: model.id,
                    name: model.name,
                    context_window: model.context_window,
                    efforts,
                    image_input: model.image_input,
                    added: model.added,
                    image_output: model.image_output,
                });
            }
            models
        })
    }

    fn providers(&self) -> BoxFuture<'_, Vec<v1::Provider>> {
        Box::pin(async move {
            let mut providers = Vec::new();
            for provider in Registry::providers(self) {
                let mut methods = Vec::new();
                if let Some(oauth) = &provider.oauth {
                    methods.push(v1::LoginMethod { id: OAUTH.into(), label: oauth.name().to_owned() });
                }
                if let Some(api_key) = &provider.api_key {
                    methods.push(v1::LoginMethod { id: API_KEY.into(), label: api_key.name().to_owned() });
                }
                let check = match self.check_auth(provider.id).await {
                    Ok(check) => check,
                    Err(error) => {
                        tracing::warn!(provider = provider.id, %error, "could not check sign-in");
                        None
                    }
                };
                // A subscription's usage is there from its sign-in on, unread
                // until the first read lands.
                let mut usage = None;
                // Stored credentials can be removed; environment variables can't.
                let (signed_in_with, can_sign_out) = match check {
                    None => (String::new(), false),
                    Some(check) if check.kind == OAUTH => {
                        if provider.limits.is_some() && provider.oauth.as_ref().is_some_and(|oauth| oauth.is_subscription()) {
                            usage = Some(v1::ProviderUsage::default());
                        }
                        (provider.oauth.as_ref().map_or_else(|| "OAuth".to_owned(), |oauth| oauth.name().to_owned()), true)
                    }
                    Some(check) => match check.source.as_deref() {
                        Some("stored credential") | None => {
                            (provider.api_key.as_ref().map_or_else(|| "API key".to_owned(), |key| key.name().to_owned()), true)
                        }
                        Some(variable) => (variable.to_owned(), false),
                    },
                };
                let stored = match self.accounts(provider.id).await {
                    Ok(stored) => stored,
                    Err(error) => {
                        tracing::warn!(provider = provider.id, %error, "could not read the accounts");
                        Vec::new()
                    }
                };
                let subscription = provider.limits.is_some() && provider.oauth.as_ref().is_some_and(|oauth| oauth.is_subscription());
                let mut accounts = Vec::new();
                for account in stored {
                    let key = account.kind == API_KEY;
                    accounts.push(v1::Account {
                        id: account.id,
                        label: account.label,
                        plan: if key { "API key".to_owned() } else { String::new() },
                        usage: (subscription && account.kind == OAUTH).then(v1::ProviderUsage::default),
                        can_sign_out: account.stored,
                        in_use: account.in_use,
                        exhausted_until: i64::try_from(account.exhausted_until).unwrap_or(0),
                        out_of_credit: account.out_of_credit,
                        method_id: account.kind.to_owned(),
                        sign_in_again: account.sign_in_again,
                        credit: None,
                        fable_exhausted_until: i64::try_from(account.fable_exhausted_until).unwrap_or(0),
                        fable_in_use: account.fable_in_use,
                    });
                }
                providers.push(v1::Provider {
                    id: provider.id.to_owned(),
                    name: provider.name.to_owned(),
                    signed_in_with,
                    can_sign_out,
                    methods,
                    usage,
                    plan: String::new(),
                    accounts,
                    cache_ttl: String::new(),
                    cache_lifetime: String::new(),
                    cache_ttl_choices: Vec::new(),
                    agents: 0,
                    // Filled in by the Workspace (`crate::catalog`).
                    ..Default::default()
                });
            }
            providers
        })
    }

    fn resolve(&self, id: &str) -> Option<Model> {
        Registry::resolve(self, id)
    }

    fn stream(&self) -> StreamFn {
        self.stream_fn()
    }

    fn stream_noting(&self, notes: Notes) -> StreamFn {
        self.stream_fn_with(Some(notes))
    }

    fn login<'a>(
        &'a self,
        provider: &'a str,
        method: &'a str,
        interaction: &'a dyn AuthInteraction,
        add: bool,
    ) -> BoxFuture<'a, Result<(), Error>> {
        Box::pin(async move {
            Registry::login_account(self, provider, method, interaction, add).await?;
            Ok(())
        })
    }

    fn logout<'a>(&'a self, provider: &'a str, account: &'a str) -> BoxFuture<'a, Result<(), Error>> {
        Box::pin(Registry::logout_account(self, provider, account))
    }

    fn move_account<'a>(&'a self, provider: &'a str, account: &'a str, position: usize) -> BoxFuture<'a, Result<(), Error>> {
        Box::pin(Registry::move_account(self, provider, account, position))
    }

    fn update_account<'a>(
        &'a self,
        provider: &'a str,
        account: &'a str,
        label: Option<&'a str>,
        retry: bool,
    ) -> BoxFuture<'a, Result<(), Error>> {
        Box::pin(Registry::update_account(self, provider, account, label, retry))
    }

    fn reads_credit(&self, provider: &str) -> bool {
        Registry::reads_credit(self, provider)
    }

    fn account_credit<'a>(
        &'a self,
        provider: &'a str,
        account: &'a str,
    ) -> BoxFuture<'a, Result<Option<factory_agent::providers::keys::KeyCredit>, Error>> {
        Box::pin(Registry::account_credit(self, provider, account))
    }

    fn make_image<'a>(
        &'a self,
        provider: &'a str,
        request: factory_agent::images::ImageRequest,
    ) -> BoxFuture<'a, Result<factory_agent::images::MadeImage, factory_agent::images::ImageFailure>> {
        Box::pin(Registry::make_image(self, provider, request))
    }

    fn usage<'a>(&'a self, provider: &'a str, account: &'a str) -> BoxFuture<'a, Result<Option<v1::ProviderUsage>, Error>> {
        Box::pin(async move {
            let Some(usage) = self.account_usage(provider, account_arg(account)).await? else { return Ok(None) };
            let mut windows = Vec::new();
            for limit in usage.limits {
                windows.push(v1::UsageWindow {
                    id: limit.id,
                    label: limit.label,
                    used_percent: limit.used_percent,
                    resets_at: i64::try_from(limit.resets_at).unwrap_or(0),
                });
            }
            let mut resets = Vec::new();
            for reset in usage.resets {
                resets.push(v1::UsageReset {
                    id: reset.id,
                    label: reset.label,
                    detail: reset.detail,
                    left: reset.left,
                    total: reset.total,
                    expires_at: i64::try_from(reset.expires_at).unwrap_or(0),
                    usable: reset.usable,
                    unavailable: reset.unavailable,
                    window_ids: reset.window_ids,
                });
            }
            Ok(Some(v1::ProviderUsage { windows, resets, ..v1::ProviderUsage::default() }))
        })
    }

    fn use_reset<'a>(
        &'a self,
        provider: &'a str,
        account: &'a str,
        reset: v1::UsageReset,
    ) -> BoxFuture<'a, Result<Option<v1::UsageResetAnswer>, Error>> {
        Box::pin(async move {
            let reset = factory_agent::limits::UsageReset {
                id: reset.id,
                label: reset.label,
                detail: reset.detail,
                left: reset.left,
                total: reset.total,
                expires_at: u64::try_from(reset.expires_at).unwrap_or(0),
                usable: reset.usable,
                unavailable: reset.unavailable,
                window_ids: reset.window_ids,
            };
            let answer = Registry::use_reset(self, provider, account_arg(account), reset).await?;
            Ok(answer.map(|answer| v1::UsageResetAnswer {
                reset: answer.reset,
                message: answer.message,
                fallback_url: answer.fallback_url,
            }))
        })
    }

    fn plan<'a>(&'a self, provider: &'a str, account: &'a str) -> BoxFuture<'a, Result<String, Error>> {
        Box::pin(async move { Ok(Registry::account_plan(self, provider, account_arg(account)).await?.unwrap_or_default()) })
    }

    fn forget_spent<'a>(&'a self, provider: &'a str, account: &'a str) -> BoxFuture<'a, Result<bool, Error>> {
        Box::pin(Registry::forget_spent(self, provider, account_arg(account)))
    }

    fn room_for<'a>(&'a self, model: &'a str) -> BoxFuture<'a, bool> {
        Box::pin(async move {
            let Some((provider, id)) = model.split_once('/') else { return false };
            Registry::room_for(self, provider, id).await
        })
    }

    fn forget_spent_with_room<'a>(&'a self, provider: &'a str, account: &'a str, full: &'a [String]) -> BoxFuture<'a, Result<bool, Error>> {
        Box::pin(Registry::forget_spent_with_room(self, provider, account_arg(account), full))
    }

    fn auth_path(&self) -> Option<PathBuf> {
        Some(Registry::auth_path(self).to_path_buf())
    }

    fn keep_sign_ins_with(&self, backing: Arc<dyn CredentialBacking>) {
        self.set_backing(backing);
    }

    fn take_in_sign_ins(&self) -> BoxFuture<'_, Result<bool, Error>> {
        Box::pin(self.take_in())
    }

    fn sign_ins_locked<'a>(&'a self, provider: &'a str) -> BoxFuture<'a, bool> {
        Box::pin(self.is_locked(provider))
    }

    fn forget_images(&self, digests: Vec<String>) -> BoxFuture<'_, ()> {
        Box::pin(async move { Registry::forget_images(self, &digests).await })
    }

    fn credit_retry_at<'a>(&'a self, provider: &'a str) -> BoxFuture<'a, Option<u64>> {
        Box::pin(async move {
            Registry::credit_retry_at(self, provider).await.unwrap_or_else(|error| {
                tracing::warn!(%provider, %error, "could not read when a key out of credit is tried again");
                None
            })
        })
    }
}
