//! Model providers: what agents can use and how Factory signs in to them.
//! Behind a trait so tests can script a provider.

use factory_agent::auth::AuthInteraction;
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
    /// Runs a provider's sign-in, asking `interaction` for whatever it needs,
    /// and stores the credential.
    fn login<'a>(&'a self, provider: &'a str, method: &'a str, interaction: &'a dyn AuthInteraction) -> BoxFuture<'a, Result<(), Error>>;
    /// Forgets the stored credential.
    fn logout<'a>(&'a self, provider: &'a str) -> BoxFuture<'a, Result<(), Error>>;
    /// Reads how much of its usage limits a provider signed in with a
    /// subscription has used; None when it isn't signed in that way.
    fn usage<'a>(&'a self, _provider: &'a str) -> BoxFuture<'a, Result<Option<Vec<v1::UsageWindow>>, Error>> {
        Box::pin(async { Ok(None) })
    }
    /// Reads which plan a provider signed in with a subscription has, such as
    /// "Claude Max 20x"; empty when it isn't signed in that way or doesn't say.
    fn plan<'a>(&'a self, _provider: &'a str) -> BoxFuture<'a, Result<String, Error>> {
        Box::pin(async { Ok(String::new()) })
    }
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
                providers.push(v1::Provider {
                    id: provider.id.to_owned(),
                    name: provider.name.to_owned(),
                    signed_in_with,
                    can_sign_out,
                    methods,
                    usage,
                    plan: String::new(),
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

    fn login<'a>(&'a self, provider: &'a str, method: &'a str, interaction: &'a dyn AuthInteraction) -> BoxFuture<'a, Result<(), Error>> {
        Box::pin(async move {
            Registry::login(self, provider, method, interaction).await?;
            Ok(())
        })
    }

    fn logout<'a>(&'a self, provider: &'a str) -> BoxFuture<'a, Result<(), Error>> {
        Box::pin(Registry::logout(self, provider))
    }

    fn usage<'a>(&'a self, provider: &'a str) -> BoxFuture<'a, Result<Option<Vec<v1::UsageWindow>>, Error>> {
        Box::pin(async move {
            let Some(limits) = self.usage_limits(provider).await? else { return Ok(None) };
            let mut windows = Vec::new();
            for limit in limits {
                windows.push(v1::UsageWindow {
                    id: limit.id,
                    label: limit.label,
                    used_percent: limit.used_percent,
                    resets_at: i64::try_from(limit.resets_at).unwrap_or(0),
                });
            }
            Ok(Some(windows))
        })
    }

    fn plan<'a>(&'a self, provider: &'a str) -> BoxFuture<'a, Result<String, Error>> {
        Box::pin(async move { Ok(Registry::plan(self, provider).await?.unwrap_or_default()) })
    }
}
