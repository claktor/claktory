//! Model lists from the providers themselves: OpenRouter's (U4), and
//! Anthropic's, OpenAI's and Codex's, so new models show
//! up without waiting for a Claktory update. The list bundled with Claktory
//! stays until a fetched one passes its checks whole, and again whenever
//! none has (`factory_agent::providers::listing`). The last one fetched is
//! kept in the home folder, so its models are there from the start, before
//! the provider answers. Read every 12 hours while the provider is signed
//! in, or now when the person asks (RefreshModels), and when they look at
//! the models or accounts (ProvidersShown) if it's more than 15 minutes old
//! .
//!
//! After each read, and as Claktory starts, a newer version of a model
//! agents run on is announced to Rivet once (`crate::upgrades`).
//!
//! A model the list doesn't have can be added by its id (AddModel, or an
//! agent's model set to it), once the provider says the sign-in has it and
//! an agent can run on it; it's kept in factory.db (`added_models`).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, Weak};
use std::time::{Duration, Instant, SystemTime};

use factory_protocol::v1;

use crate::factory::{Factory, write_atomic};
use crate::home::Home;
use crate::models::Models;

/// How old the kept list may get before it's fetched again.
const REFRESH: Duration = Duration::from_secs(12 * 60 * 60);
/// How often it looks whether a fetch is due (a provider signed in to
/// meanwhile, say).
const CHECK: Duration = Duration::from_secs(10 * 60);
/// How long after a fetch that failed it tries again by itself.
const RETRY: Duration = Duration::from_secs(60 * 60);
/// How old a list may be when the person looks at the models
/// (ProvidersShown) before it's read again.
const SHOWN: Duration = Duration::from_secs(15 * 60);
/// How long after a failed read a look tries again.
const SHOWN_RETRY: Duration = Duration::from_secs(60);

pub(crate) const SCHEMA: &str = "
-- Models added by their id, as their provider's list didn't have them
-- (friend bug #14): the model as Claktory runs it, as JSON.
CREATE TABLE added_models (
    provider TEXT NOT NULL,
    id TEXT NOT NULL,
    model TEXT NOT NULL,
    added_ms INTEGER NOT NULL,
    PRIMARY KEY (provider, id)
);
";

/// Where a provider's last list fetched is kept, as it came.
fn path(home: &Home, provider: &str) -> PathBuf {
    home.root().join(format!("{provider}-models.json"))
}

/// How a provider's list was last read.
#[derive(Clone, Debug, Default)]
struct Status {
    /// When a list was last taken in, in ms since 1970; 0 never.
    fetched_ms: u64,
    /// Why the last read failed; empty when it worked.
    error: String,
    /// The last read failed because the provider didn't take the sign-in
    /// or key: a hint only.
    signed_out: bool,
    fetching: bool,
    /// When the last read failed, so the next try waits `RETRY`.
    failed_at: Option<Instant>,
}

#[derive(Default)]
pub(crate) struct Catalog {
    status: Mutex<HashMap<String, Status>>,
    /// One read at a time.
    reading: tokio::sync::Mutex<()>,
}

impl Catalog {
    fn status(&self) -> std::sync::MutexGuard<'_, HashMap<String, Status>> {
        self.status.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Fills in how the provider's list was read.
    pub(crate) fn view(&self, provider: &mut v1::Provider, fetchable: bool) {
        let status = self.status().get(&provider.id).cloned().unwrap_or_default();
        provider.models_fetched_ms = status.fetched_ms;
        provider.models_fetch_error = status.error;
        provider.models_fetching = status.fetching;
        provider.models_fetchable = fetchable;
        provider.models_fetch_signed_out = status.signed_out;
    }
}

/// Takes in the lists kept from the last fetches that pass their checks,
/// and the models added by their id: before anything reads the models.
pub(crate) fn load(home: &Home, store: &crate::store::Store, models: &dyn Models) -> Catalog {
    let catalog = Catalog::default();
    for provider in factory_agent::providers::listing::PROVIDERS {
        let path = path(home, provider);
        let Ok(body) = std::fs::read(&path) else { continue };
        match models.take_models(provider, &body) {
            Ok(_) => {
                let modified = std::fs::metadata(&path).and_then(|metadata| metadata.modified()).ok();
                let fetched_ms =
                    modified.and_then(|at| at.duration_since(SystemTime::UNIX_EPOCH).ok()).map_or(0, |at| at.as_millis() as u64);
                catalog.status().entry(provider.to_owned()).or_default().fetched_ms = fetched_ms;
            }
            Err(error) => tracing::warn!(%error, path = %path.display(), "a kept model list isn't used"),
        }
    }
    let db = store.db();
    let added = db.prepare("SELECT provider, id, model FROM added_models ORDER BY added_ms").and_then(|mut statement| {
        statement
            .query_map([], |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?, row.get::<_, String>(2)?)))?
            .collect::<Result<Vec<_>, _>>()
    });
    match added {
        Ok(added) => {
            for (provider, id, model) in added {
                if let Err(error) = models.add_model(&provider, model.as_bytes()) {
                    tracing::warn!(%error, provider, id, "a model added by its id isn't used");
                }
            }
        }
        Err(error) => tracing::warn!(%error, "could not read the models added by their id"),
    }
    catalog
}

/// The providers Claktory has.
fn providers(factory: &Factory) -> Vec<String> {
    factory.lock().providers.iter().map(|provider| provider.id.clone()).collect()
}

/// Whether the provider is signed in and its list is read from it.
fn fetchable(factory: &Factory, provider: &str) -> bool {
    factory.models.fetches_models(provider)
        && factory.lock().providers.iter().any(|known| known.id == provider && !known.signed_in_with.is_empty())
}

/// Fetches each provider's list while it's signed in: when the kept one is
/// older than `REFRESH`, or there's none.
pub(crate) async fn monitor(factory: Weak<Factory>) {
    loop {
        let Some(strong) = factory.upgrade() else { return };
        // Newer models among those kept, from the start.
        crate::upgrades::check(&strong);
        for provider in providers(&strong) {
            let retry_due =
                strong.catalog.status().get(&provider).and_then(|status| status.failed_at).is_none_or(|at| at.elapsed() >= RETRY);
            if fetchable(&strong, &provider) && retry_due && is_due(&path(&strong.home, &provider)).await {
                let _ = fetch(&strong, &provider).await;
            }
        }
        drop(strong);
        tokio::time::sleep(CHECK).await;
    }
}

/// Whether the kept list is older than `REFRESH`, or missing.
async fn is_due(path: &Path) -> bool {
    match tokio::fs::metadata(path).await.and_then(|metadata| metadata.modified()) {
        Ok(modified) => SystemTime::now().duration_since(modified).is_ok_and(|age| age >= REFRESH),
        Err(_) => true,
    }
}

/// Reads the provider's list now, takes it in and keeps it, saying so in
/// Workspace while it reads; why not, in a sentence, when it failed.
async fn fetch(factory: &Arc<Factory>, provider: &str) -> Result<(), String> {
    let _reading = factory.catalog.reading.lock().await;
    let set = |change: &dyn Fn(&mut Status)| {
        change(factory.catalog.status().entry(provider.to_owned()).or_default());
        factory.broadcast_workspace(&factory.lock());
    };
    set(&|status| status.fetching = true);
    let path = path(&factory.home, provider);
    // Whether a read failed because the provider didn't take the sign-in
    // or key (it answered 401): only a hint for the app, which changes
    // nothing about the sign-in itself.
    let refused = std::sync::atomic::AtomicBool::new(false);
    let read = async {
        let fetched = factory.models.fetch_models(provider).await.map_err(|error| {
            refused.store(error.is_unauthorized() || error.is_refused(), std::sync::atomic::Ordering::Relaxed);
            error.to_string()
        })?;
        let Some(body) = fetched else {
            return Err(format!("Sign in to {} first, in Settings → Providers.", factory_agent::providers::listing::name(provider)));
        };
        let changed = factory
            .models
            .take_models(provider, &body)
            .map_err(|error| format!("{}'s answer wasn't its model list: {error}.", factory_agent::providers::listing::name(provider)))?;
        write_atomic(&path, &body).await.map_err(|error| format!("The model list couldn't be kept: {error}."))?;
        Ok(changed)
    };
    let result = read.await;
    if let Ok(true) = result {
        factory.refresh_models().await;
    }
    if result.is_ok() {
        crate::upgrades::check(factory);
    }
    let now = factory_agent::clock::now_ms();
    match &result {
        Ok(_) => set(&|status| *status = Status { fetched_ms: now, ..Status::default() }),
        Err(error) => {
            tracing::warn!(provider, %error, "could not read the provider's model list: the one there is stays");
            let signed_out = refused.load(std::sync::atomic::Ordering::Relaxed);
            set(&|status| {
                status.fetching = false;
                status.error.clone_from(error);
                status.signed_out = signed_out;
                status.failed_at = Some(Instant::now());
            });
        }
    }
    result.map(|_| ())
}

/// ProvidersShown: the person looks at the models, so each
/// signed-in provider's list read more than `SHOWN` ago is read again now,
/// unless it's being read or a read failed within `SHOWN_RETRY`. Each read
/// shows in Workspace as it goes.
pub(crate) async fn shown(factory: &Arc<Factory>) {
    let now = factory_agent::clock::now_ms();
    for provider in providers(factory) {
        let status = factory.catalog.status().get(&provider).cloned().unwrap_or_default();
        let stale = now.saturating_sub(status.fetched_ms) >= SHOWN.as_millis() as u64;
        let retry_due = status.failed_at.is_none_or(|at| at.elapsed() >= SHOWN_RETRY);
        if stale && retry_due && !status.fetching && fetchable(factory, &provider) {
            let _ = fetch(factory, &provider).await;
        }
    }
}

/// RefreshModels: reads the provider's list now (every fetchable one's for
/// an empty id).
pub(crate) async fn refresh(factory: &Arc<Factory>, provider: &str) -> Result<(), String> {
    if provider.is_empty() {
        let mut failed = Vec::new();
        for provider in providers(factory) {
            if fetchable(factory, &provider)
                && let Err(error) = fetch(factory, &provider).await
            {
                failed.push(error);
            }
        }
        return if failed.is_empty() { Ok(()) } else { Err(failed.join(" ")) };
    }
    let who = factory.lock().providers.iter().find(|known| known.id == provider).map(|known| known.name.clone());
    let Some(who) = who else { return Err(format!("There's no provider “{provider}”.")) };
    if !factory.models.fetches_models(provider) {
        return Err(format!("{who}'s models can't be read from {who} here."));
    }
    if !fetchable(factory, provider) {
        return Err(format!("Sign in to {who} first, in Settings → Providers."));
    }
    fetch(factory, provider).await
}

/// Whether to check a model before it's set: AddModel always; an agent's
/// model only when its provider's list can be asked (as before otherwise).
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Check {
    Always,
    WhenItCan,
}

/// Makes sure `model` ("<provider>/<model id>") is in the list: one that
/// isn't is checked with its provider and added. Why not, in a sentence to
/// show.
pub(crate) async fn ensure(factory: &Arc<Factory>, model: &str, check: Check) -> Result<(), String> {
    let model = model.trim();
    if factory.lock().models.iter().any(|known| known.id == model) {
        return Ok(());
    }
    let Some((provider, id)) = model.split_once('/').filter(|(provider, id)| !provider.is_empty() && !id.is_empty()) else {
        return match check {
            Check::Always => Err("Give the model as <provider>/<model id>, such as anthropic/claude-opus-5.".to_owned()),
            Check::WhenItCan => Ok(()),
        };
    };
    let known = factory.lock().providers.iter().any(|known| known.id == provider);
    if !known || !factory.models.fetches_models(provider) {
        return match check {
            Check::Always if !known => Err(format!("There's no provider “{provider}”.")),
            Check::Always => Err(format!("Claktory can't ask {provider} about its models here.")),
            Check::WhenItCan => Ok(()),
        };
    }
    let Some(found) = factory.models.check_model(provider, id).await? else {
        return match check {
            Check::Always => Err(format!("Claktory can't ask {provider} about its models here.")),
            Check::WhenItCan => Ok(()),
        };
    };
    factory.models.add_model(provider, &found)?;
    let kept = String::from_utf8_lossy(&found).into_owned();
    factory
        .store
        .db()
        .execute(
            "INSERT INTO added_models (provider, id, model, added_ms) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT (provider, id) DO UPDATE SET model = excluded.model",
            rusqlite::params![provider, id, kept, factory_agent::clock::now_ms() as i64],
        )
        .map_err(|error| format!("The model couldn't be kept: {error}."))?;
    factory.refresh_models().await;
    Ok(())
}
