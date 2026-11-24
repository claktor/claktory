//! Agents wait out their accounts' usage limits. When none of the accounts
//! that can serve an agent's model can take a request (all at their usage
//! limit, or out of credit), its run doesn't fail: it waits, says so in its
//! conversation and its status, and asks again when the first limit resets
//! (with a small margin), or sooner when something changes that may give it
//! one: the person signs in (again) or out, adds an account, uses a usage
//! reset, tries a key again, chooses Try now, messages it or changes the
//! agent's model, or the usage read sees room. An account out of credit
//! doesn't reset by itself, so then it waits for the person, as it does
//! when every account was signed out while it ran. Messages and events that
//! come meanwhile queue as for any working agent; the run stays open, so a
//! backend restart resumes it, and it waits again.
//!
//! A run whose model calls can't connect waits the same way, for the
//! network (#364 O2): after the quick retries, it looks every few seconds
//! until names resolve again, then asks again, each time a little later
//! while it keeps failing. It waits for as long as the network is down (a
//! night, say); only a provider that can't be reached for an hour while the
//! network works ends the run.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Weak};
use std::time::Duration;

use factory_protocol::v1::NoticeLevel;

use crate::factory::{Factory, RequestError};

/// How long after a limit's reset the request goes again, so the provider
/// has reset it too, in ms (15 s; tests make it shorter).
static MARGIN_MS: AtomicU64 = AtomicU64::new(15_000);

/// How long an agent waits for the vault to open before Needs you offers
/// to open it, in ms (a minute; tests make it shorter).
static LOCKED_WAIT_MS: AtomicU64 = AtomicU64::new(60_000);

/// The shortest wait, whatever the reset time says, so a clock that
/// disagrees with the provider's never makes it ask over and over (or the
/// margin, when that's shorter).
const SHORTEST: Duration = Duration::from_secs(10);

/// How often a wait for the network looks whether it's back.
const NETWORK_LOOK: Duration = Duration::from_secs(5);
/// The least a wait for the network lasts, doubling with each one in a row
/// up to `NETWORK_MOST`, so a provider that can't be reached while the
/// network is fine is asked less and less often (ms; tests make it shorter).
static NETWORK_LEAST_MS: AtomicU64 = AtomicU64::new(5_000);
const NETWORK_MOST: Duration = Duration::from_secs(5 * 60);
/// A usage wait at most this long is brief: see `wait_out`.
const BRIEF_MS: u64 = 2 * 60 * 1000;
/// How long a provider can't be reached while the network works before
/// the run gives up, in ms (an hour; tests make it shorter).
pub(crate) const NETWORK_GIVE_UP_MS: i64 = 60 * 60 * 1000;

/// What an agent waits for, while it does: shown with it in the app.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct UsageWait {
    /// In words, one line: "Waiting for your Claude usage to reset at Fri 18:57".
    pub text: String,
    /// When it asks again by itself, in ms since 1970; 0 when it waits for
    /// the person.
    pub until_ms: i64,
    /// It waits on a usage limit or on credit, not for a sign-in or the
    /// secrets: another provider could carry it (`crate::usage_switch`).
    pub switchable: bool,
    /// The provider it waits on: its model's when the wait began.
    pub provider: String,
}

/// Clears the agent's wait however the wait ends: over, or stopped.
struct Clear {
    factory: Arc<Factory>,
    id: String,
    epoch: u64,
}

impl Drop for Clear {
    fn drop(&mut self) {
        let mut inner = self.factory.lock();
        let Some(runtime) = inner.agents.get_mut(&self.id).filter(|runtime| runtime.epoch == self.epoch) else { return };
        if runtime.usage_wait.take().is_some() {
            self.factory.broadcast_workspace(&inner);
        }
    }
}

/// The agent's wait: what its run calls when no account can take its
/// request (see `factory_agent::WaitOut`).
pub(crate) fn wait_out(factory: Weak<Factory>, id: String, epoch: u64) -> factory_agent::WaitOut {
    Arc::new(move |until, why| {
        let (factory, id) = (factory.clone(), id.clone());
        Box::pin(async move {
            let Some(factory) = factory.upgrade() else { return false };
            factory.wait_out(&id, epoch, until, why).await
        })
    })
}

/// The agent's wait for the network: what its run calls when its model
/// calls can't connect (see `factory_agent::WaitNetwork`).
pub(crate) fn wait_network(factory: Weak<Factory>, id: String, epoch: u64) -> factory_agent::WaitNetwork {
    Arc::new(move |why| {
        let (factory, id) = (factory.clone(), id.clone());
        Box::pin(async move {
            let Some(factory) = factory.upgrade() else { return Err(None) };
            factory.wait_network(&id, epoch, why).await
        })
    })
}

impl Factory {
    /// Waits for the network (#364 O2): until names resolve again, and at
    /// least a while that grows with each wait in a row, or sooner when
    /// something changes (Try now, a message). However long the network is
    /// down; once the provider couldn't be reached for an hour while it was
    /// up, the run fails, saying so in place of the connection error.
    async fn wait_network(self: Arc<Self>, id: &str, epoch: u64, why: String) -> Result<(), Option<String>> {
        let changed = self.capacity.clone();
        let changed = changed.notified();
        tokio::pin!(changed);
        changed.as_mut().enable();
        let model_and_name = {
            let inner = self.lock();
            inner.config.agent(id).map(|agent| (agent.model.clone(), agent.name.clone())).or_else(|| self.chats.model_and_name(id))
        };
        let Some((model, name)) = model_and_name else { return Err(None) };
        let provider = model.split_once('/').map_or(model.as_str(), |(provider, _)| provider).to_owned();
        // With no network, it waits as long as that lasts; with one, the
        // provider itself can't be reached, which ends the run in time.
        let up = self.network_up().await;
        let now = crate::transcript::now_ms();
        let give_up = self.network_give_up_ms.load(Ordering::Relaxed);
        let waits = {
            let mut inner = self.lock();
            let provider_name =
                inner.providers.iter().find(|known| known.id == provider).map_or(provider.clone(), |known| known.name.clone());
            let noun = factory_agent::accounts::account_noun(&provider, &provider_name);
            let Some(runtime) = inner.agents.get_mut(id).filter(|runtime| runtime.epoch == epoch) else { return Err(None) };
            if runtime.idle {
                return Err(None);
            }
            if !up {
                runtime.network_since = 0;
            } else if runtime.network_since == 0 {
                runtime.network_since = now;
            }
            runtime.network_waits += 1;
            let waits = runtime.network_waits;
            if up && now - runtime.network_since >= give_up {
                (runtime.network_since, runtime.network_waits) = (0, 0);
                tracing::warn!(agent = %id, %why, "the provider couldn't be reached for an hour with the network up: the run ends");
                // Said in place of the connection error: one notice for the stop.
                return Err(Some(format!(
                    "Couldn't reach {noun} for an hour, though the network works. {name} stopped; send a message to carry on."
                )));
            }
            runtime.usage_wait =
                Some(UsageWait {
                    text: "Waiting for the network…".to_owned(), until_ms: 0, switchable: false, provider: provider.clone()
                });
            // Said once per outage, not on each look.
            if waits == 1
                && let Some(transcript) = runtime.transcript.as_mut()
            {
                let entry = transcript
                    .notice(format!("Lost the connection to {noun}. {name} carries on by itself once it's back."), NoticeLevel::Info);
                self.send_change(&inner, id, crate::transcript::Change::Entry(entry));
            }
            self.broadcast_workspace(&inner);
            waits
        };
        let _clear = Clear { factory: self.clone(), id: id.to_owned(), epoch };
        tracing::info!(agent = %id, %why, waits, "waiting for the network");
        let least =
            Duration::from_millis(NETWORK_LEAST_MS.load(Ordering::Relaxed)).saturating_mul(1 << (waits - 1).min(10)).min(NETWORK_MOST);
        let started = tokio::time::Instant::now();
        loop {
            let look = if started.elapsed() >= least { NETWORK_LOOK.min(least) } else { least - started.elapsed() };
            tokio::select! {
                () = tokio::time::sleep(look) => {}
                () = changed.as_mut() => return Ok(()),
            }
            if started.elapsed() >= least && self.network_up().await {
                return Ok(());
            }
        }
    }

    /// Whether names resolve, as they do with a network (`crate::network`).
    async fn network_up(&self) -> bool {
        match self.network_says.load(Ordering::Relaxed) {
            1 => true,
            2 => false,
            _ => crate::network::resolves().await,
        }
    }

    /// How long a provider can't be reached while the network works before
    /// the run gives up (an hour); tests make it shorter.
    pub fn set_network_give_up(&self, after: Duration) {
        self.network_give_up_ms.store(i64::try_from(after.as_millis()).unwrap_or(i64::MAX), Ordering::Relaxed);
    }

    /// The least a wait for the network lasts (5 s, doubling with each in a
    /// row), for every Factory in this process; tests make it shorter.
    pub fn set_network_wait(&self, least: Duration) {
        NETWORK_LEAST_MS.store(u64::try_from(least.as_millis()).unwrap_or(u64::MAX), Ordering::Relaxed);
    }

    /// Tests: whether the network is up for the waits on it, in place of
    /// looking (None: look).
    pub fn set_network_up(&self, up: Option<bool>) {
        self.network_says.store(
            match up {
                None => 0,
                Some(true) => 1,
                Some(false) => 2,
            },
            Ordering::Relaxed,
        );
    }

    /// Waits until `until` (ms since 1970) plus a margin, or, when None,
    /// until a key out of credit is tried again or the person changes
    /// something; or sooner, when the accounts or
    /// the agent's settings change. False when the agent doesn't wait: its
    /// run only compacts it while idle, which nobody waits for.
    async fn wait_out(self: Arc<Self>, id: &str, epoch: u64, until: Option<u64>, why: String) -> bool {
        // Woken by anything that changes from here on.
        let changed = self.capacity.clone();
        let changed = changed.notified();
        tokio::pin!(changed);
        changed.as_mut().enable();
        // A chat waits as an agent does, as "The chat".
        let model_and_name = {
            let inner = self.lock();
            inner.config.agent(id).map(|agent| (agent.model.clone(), agent.name.clone())).or_else(|| self.chats.model_and_name(id))
        };
        let Some((model, name)) = model_and_name else { return false };
        let provider = model.split_once('/').map_or(model.as_str(), |(provider, _)| provider).to_owned();
        // Its model moved to another provider after the answer it waits
        // after: the wait was the other's, so it asks again now, on its new
        // model, rather than wait under this one's name.
        let asked_on = self.lock().agents.get_mut(id).map(|runtime| std::mem::take(&mut runtime.asked_on)).unwrap_or_default();
        if !asked_on.is_empty() && asked_on != provider {
            tracing::info!(agent = %id, from = %asked_on, to = %provider, "its model moved to another provider while it asked: asking again");
            return true;
        }
        // Its model moved to a provider with room while it asked (Continue
        // for all, say, whose first change woke it before its own): the
        // limit was another provider's, so it asks again now, on its new
        // model, rather than wait under this one's name (row 736).
        // Judged by the registry, which knows every pool's limits (an Opus
        // one alone, U11), not only those the providers' listing shows.
        if until.is_some() && self.models.room_for(&model).await {
            tracing::info!(agent = %id, %provider, "its model moved to a provider with room while it asked: asking again");
            return true;
        }
        // The sign-ins are locked away with the person's secrets: it waits
        // for them to open (`vault::signins`).
        let locked = until.is_none() && self.models.sign_ins_locked(&provider).await;
        // Out of credit: a request tries the key again by itself after an
        // hour (`CREDIT_RETRY_MS`), so the wait ends then too, credit added
        // or not.
        let retry_at = if until.is_none() && !locked { self.models.credit_retry_at(&provider).await } else { None };
        let brief = {
            let mut inner = self.lock();
            let known = inner.providers.iter().find(|known| known.id == provider);
            let provider_name = known.map_or(provider.clone(), |known| known.name.clone());
            let signed_out = known.is_none_or(|known| known.signed_in_with.is_empty());
            // Signed in with API keys only: their limit is one set on spending.
            let keys_only = known
                .is_some_and(|known| !known.accounts.is_empty() && known.accounts.iter().all(|account| account.method_id == "api_key"));
            let noun = factory_agent::accounts::account_noun(&provider, &provider_name);
            // A key's limit that lifts within a couple of minutes, as
            // OpenRouter's in-flight budget does (its Retry-After): waited
            // out quietly, with no notice, no Needs you item and nothing
            // switched to another provider.
            let brief = keys_only && until.is_some_and(|at| at <= factory_agent::clock::now_ms() + BRIEF_MS);
            let Some(runtime) = inner.agents.get_mut(id).filter(|runtime| runtime.epoch == epoch) else { return false };
            if runtime.idle {
                return false;
            }
            if !locked {
                runtime.locked_since = 0;
            } else if runtime.locked_since == 0 {
                runtime.locked_since = crate::transcript::now_ms();
                // After a minute, Needs you offers to open the vault
                // (VaultState.agents_waiting).
                let factory = Arc::downgrade(&self);
                tokio::spawn(async move {
                    tokio::time::sleep(Duration::from_millis(LOCKED_WAIT_MS.load(Ordering::Relaxed))).await;
                    if let Some(factory) = factory.upgrade() {
                        factory.broadcast_workspace(&factory.lock());
                    }
                });
            }
            let (text, notice) = match until {
                Some(at) if brief => (
                    format!(
                        "Waiting a moment for {noun}, until {}",
                        factory_agent::accounts::local_time(at).unwrap_or_else(|| factory_agent::accounts::when(at))
                    ),
                    String::new(),
                ),
                None if locked => (
                    format!("Waiting to use {provider_name} until your secrets unlock"),
                    format!("{why} {name} carries on by itself once they're unlocked."),
                ),
                Some(at) => (
                    format!(
                        "Waiting for your {noun} {} to reset at {}",
                        if keys_only { "spending limit" } else { "usage" },
                        factory_agent::accounts::local_time(at).unwrap_or_else(|| factory_agent::accounts::when(at))
                    ),
                    if keys_only {
                        format!("{why} {name} will carry on by itself then. If you've raised the limit, choose Try now to carry on now.")
                    } else {
                        format!("{why} {name} will carry on by itself then. If you've upgraded your plan, choose Try now to carry on now.")
                    },
                ),
                // Signed out of every account while it ran: it carries on
                // once the person signs in again.
                None if signed_out => (
                    format!("Waiting for you to sign in to {noun}"),
                    format!("{why} {name} will carry on once you sign in under Settings → Providers."),
                ),
                None => (
                    format!("Waiting for you to add credit or another {noun} account"),
                    format!("{why} {name} will carry on once you add credit or another account. After adding credit, choose Try now."),
                ),
            };
            let switchable = !brief && (until.is_some() || (!locked && !signed_out));
            let wait = UsageWait {
                text,
                until_ms: until.and_then(|at| i64::try_from(at).ok()).unwrap_or(0),
                switchable,
                provider: provider.clone(),
            };
            // Said once per reset time, not again on every look.
            let say = !brief && runtime.wait_said != Some(until);
            runtime.wait_said = Some(until);
            runtime.usage_wait = Some(wait);
            if say && let Some(transcript) = runtime.transcript.as_mut() {
                let entry = transcript.notice(notice, NoticeLevel::Info);
                self.send_change(&inner, id, crate::transcript::Change::Entry(entry));
            }
            self.broadcast_workspace(&inner);
            brief
        };
        // Continue automatically (friend bug #8 A2): a wait on a usage limit
        // goes on elsewhere at once, when the person turned it on and
        // another provider can run the model; the switch wakes the wait.
        if until.is_some() && !brief {
            crate::usage_switch::continue_by_itself(&self, id).await;
        }
        let _clear = Clear { factory: self.clone(), id: id.to_owned(), epoch };
        tracing::info!(agent = %id, until, "waiting out the accounts' usage limits");
        match until.or(retry_at) {
            Some(at) => {
                let left = Duration::from_millis(at.saturating_sub(factory_agent::clock::now_ms()));
                let margin = Duration::from_millis(MARGIN_MS.load(Ordering::Relaxed));
                let wait = (left + margin).max(SHORTEST.min(margin));
                tokio::select! {
                    () = tokio::time::sleep(wait) => {}
                    () = changed => {}
                }
            }
            None => changed.await,
        }
        true
    }

    /// How long after a limit's reset a waiting agent asks again (15 s), for
    /// every Factory in this process; tests make it shorter.
    pub fn set_usage_wait_margin(&self, margin: Duration) {
        MARGIN_MS.store(u64::try_from(margin.as_millis()).unwrap_or(u64::MAX), Ordering::Relaxed);
    }

    /// How long an agent waits for the vault to open before Needs you
    /// offers to open it (a minute), for every Factory in this process;
    /// tests make it shorter.
    pub fn set_locked_wait(&self, wait: Duration) {
        LOCKED_WAIT_MS.store(u64::try_from(wait.as_millis()).unwrap_or(u64::MAX), Ordering::Relaxed);
    }

    /// Whether an agent or chat has waited for the vault to open, to use
    /// sign-ins kept in it, for a minute or more (VaultState.agents_waiting).
    pub(crate) fn agents_waiting_for_vault(&self, inner: &crate::factory::Inner) -> bool {
        let since = crate::transcript::now_ms() - i64::try_from(LOCKED_WAIT_MS.load(Ordering::Relaxed)).unwrap_or(i64::MAX);
        !self.vault.is_open()
            && inner
                .agents
                .values()
                .any(|runtime| runtime.usage_wait.is_some() && runtime.locked_since != 0 && runtime.locked_since <= since)
    }

    /// Something changed that may give waiting agents an account to use:
    /// they ask again now.
    pub(crate) fn capacity_changed(&self) {
        self.capacity.notify_waiters();
    }

    /// TryAgainNow: Try now, beside an agent's or a chat's usage wait. Its
    /// provider's spent marks are forgotten and their usage read again, and
    /// every agent waiting on it asks again now (`probe_accounts`).
    pub(crate) async fn try_again_now(self: &Arc<Self>, id: &str) -> Result<(), RequestError> {
        let (model, name, waiting) = {
            let inner = self.lock();
            let model_and_name =
                inner.config.agent(id).map(|agent| (agent.model.clone(), agent.name.clone())).or_else(|| self.chats.model_and_name(id));
            let Some((model, name)) = model_and_name else { return Err(RequestError::UnknownAgent(id.to_owned())) };
            (model, name, inner.agents.get(id).is_some_and(|runtime| runtime.usage_wait.is_some()))
        };
        if !waiting {
            return Err(RequestError::Wait(format!("{name} isn't waiting anymore.")));
        }
        let provider = model.split_once('/').map_or(model.as_str(), |(provider, _)| provider).to_owned();
        tracing::info!(agent = %id, %provider, "Try now: asking the provider again");
        self.usage_reread.lock().unwrap_or_else(std::sync::PoisonError::into_inner).insert(provider.clone());
        self.usage_wake.notify_one();
        self.clone().probe_accounts(provider).await;
        Ok(())
    }

    /// The person messaged an agent waiting out the provider's usage: they
    /// may just have brought usage back in a way nothing here can see (a
    /// reset claimed on the provider's own site, credit added), so the
    /// accounts' spent marks are forgotten and the waiters ask the provider
    /// itself again. A wrong guess costs one refused request, which marks
    /// the account spent again with the provider's fresh reset time.
    pub(crate) async fn probe_accounts(self: Arc<Self>, provider: String) {
        match self.models.forget_spent(&provider, "").await {
            // Shown at once: its accounts have room, and agents switched
            // away from it (#341) can go back.
            Ok(true) => self.refresh_models().await,
            Ok(false) => {}
            Err(error) => tracing::warn!(%provider, %error, "could not forget the accounts' spent marks"),
        }
        self.capacity_changed();
    }
}
