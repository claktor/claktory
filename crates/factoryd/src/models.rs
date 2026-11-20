//! Model providers: what agents can use and how Factory signs in to them.
//! Behind a trait so tests can script a provider.

use factory_agent::auth::AuthInteraction;
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
}

/// An account id as `Registry` takes it: None for the first.
fn account_arg(account: &str) -> Option<&str> {
    (!account.is_empty()).then_some(account)
}

pub use factory_agent::Registry;

impl Models for Registry {
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
                models.push(v1::Model { id: model.id, name: model.name, context_window: model.context_window, efforts });
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
}
