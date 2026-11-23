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

impl Factory {
    /// Waits until `until` (ms since 1970) plus a margin, or, when None,
    /// until the person changes something; or sooner, when the accounts or
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
        // The sign-ins are locked away with the person's secrets: it waits
        // for them to open (`vault::signins`).
        let locked = until.is_none() && self.models.sign_ins_locked(&provider).await;
        {
            let mut inner = self.lock();
            let known = inner.providers.iter().find(|known| known.id == provider);
            let provider_name = known.map_or(provider.clone(), |known| known.name.clone());
            let signed_out = known.is_none_or(|known| known.signed_in_with.is_empty());
            let noun = factory_agent::accounts::account_noun(&provider, &provider_name);
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
                None if locked => (
                    format!("Waiting to use {provider_name} until your secrets unlock"),
                    format!("{why} {name} carries on by itself once they're unlocked."),
                ),
                Some(at) => (
                    format!(
                        "Waiting for your {noun} usage to reset at {}",
                        factory_agent::accounts::local_time(at).unwrap_or_else(|| factory_agent::accounts::when(at))
                    ),
                    format!("{why} {name} will carry on by itself then."),
                ),
                // Signed out of every account while it ran: it carries on
                // once the person signs in again.
                None if signed_out => (
                    format!("Waiting for you to sign in to {noun}"),
                    format!("{why} {name} will carry on once you sign in under Settings → Providers."),
                ),
                None => (
                    format!("Waiting for you to add credit or another {noun} account"),
                    format!(
                        "{why} {name} will carry on once you add credit or another account. After adding credit, use Try again on the key in Settings → Providers."
                    ),
                ),
            };
            let switchable = until.is_some() || (!locked && !signed_out);
            let wait = UsageWait { text, until_ms: until.and_then(|at| i64::try_from(at).ok()).unwrap_or(0), switchable };
            // Said once per reset time, not again on every look.
            let say = runtime.wait_said != Some(until);
            runtime.wait_said = Some(until);
            runtime.usage_wait = Some(wait);
            if say && let Some(transcript) = runtime.transcript.as_mut() {
                let entry = transcript.notice(notice, NoticeLevel::Info);
                self.send_change(&inner, id, crate::transcript::Change::Entry(entry));
            }
            self.broadcast_workspace(&inner);
        }
        let _clear = Clear { factory: self.clone(), id: id.to_owned(), epoch };
        tracing::info!(agent = %id, until, "waiting out the accounts' usage limits");
        match until {
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
        if let Err(error) = self.models.forget_spent(&provider, "").await {
            tracing::warn!(%provider, %error, "could not forget the accounts' spent marks");
        }
        self.capacity_changed();
    }
}
