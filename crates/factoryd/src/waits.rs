//! Agents wait out their accounts' usage limits. When none of the accounts
//! that can serve an agent's model can take a request (all at their usage
//! limit, or out of credit), its run doesn't fail: it waits, says so in its
//! conversation and its status, and asks again when the first limit resets
//! (with a small margin), or sooner when something changes that may give it
//! one: the person adds an account, uses a usage reset, tries a key again or
//! changes the agent's model. An account out of credit doesn't reset by
//! itself, so then it waits for the person. Messages and events that come
//! meanwhile queue as for any working agent; the run stays open, so a
//! backend restart resumes it, and it waits again.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Weak};
use std::time::Duration;

use factory_protocol::v1::NoticeLevel;

use crate::factory::Factory;

/// How long after a limit's reset the request goes again, so the provider
/// has reset it too, in ms (15 s; tests make it shorter).
static MARGIN_MS: AtomicU64 = AtomicU64::new(15_000);

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
        {
            let mut inner = self.lock();
            let noun = inner.config.agent(id).map(|agent| {
                let provider = agent.model.split_once('/').map_or(agent.model.as_str(), |(provider, _)| provider);
                let name = inner.providers.iter().find(|known| known.id == provider).map_or(provider, |known| known.name.as_str());
                (factory_agent::accounts::account_noun(provider, name), agent.name.clone())
            });
            let Some((noun, name)) = noun else { return false };
            let Some(runtime) = inner.agents.get_mut(id).filter(|runtime| runtime.epoch == epoch) else { return false };
            if runtime.idle {
                return false;
            }
            let (text, notice) = match until {
                Some(at) => (
                    format!(
                        "Waiting for your {noun} usage to reset at {}",
                        factory_agent::accounts::local_time(at).unwrap_or_else(|| factory_agent::accounts::when(at))
                    ),
                    format!("{why} {name} will carry on by itself then."),
                ),
                None => (
                    format!("Waiting for you to add credit or another {noun} account"),
                    format!(
                        "{why} {name} will carry on once you add credit or another account. After adding credit, use Try again on the key in Settings → Providers."
                    ),
                ),
            };
            let wait = UsageWait { text, until_ms: until.and_then(|at| i64::try_from(at).ok()).unwrap_or(0) };
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

    /// Something changed that may give waiting agents an account to use:
    /// they ask again now.
    pub(crate) fn capacity_changed(&self) {
        self.capacity.notify_waiters();
    }
}
