//! How much of their usage limits the providers signed in with a subscription
//! have used, and which plan each has, account by account. Read from each provider at sign-in,
//! often while its agents work and rarely while they're idle, so it stays
//! current without wasting requests.

use std::collections::{HashMap, HashSet};
use std::sync::Weak;
use std::time::Duration;

use factory_protocol::v1;

use crate::factory::{Factory, RequestError};
use crate::transcript::now_ms;

/// How often the schedule is checked; checking costs nothing.
const TICK: Duration = Duration::from_secs(15);
const MINUTE: i64 = 60_000;
/// Between reads while an agent on the provider works.
const WHILE_WORKING: i64 = 5 * MINUTE;
/// After the last read that saw agents working, once they've stopped, or
/// after a limit resets.
const AFTER_WORK: i64 = MINUTE;
/// Between reads while nothing runs on it: catches use elsewhere, such as the
/// person's own Claude or ChatGPT.
const WHILE_IDLE: i64 = 15 * MINUTE;
/// Between attempts after a read failed.
const AFTER_ERROR: i64 = 5 * MINUTE;

/// What the schedule knows about one signed-in provider.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Tracked {
    signed_in_with: String,
    /// When it was last read or tried, in ms since 1970; None before that.
    read: Option<i64>,
    /// It has no limits to read, such as a sign-in with an API key.
    unlimited: bool,
    /// An agent on it worked since the last read.
    worked: bool,
    /// An agent waits for its provider's usage to come back (`crate::waits`):
    /// every account is read each minute, in use or not, so room the
    /// provider reports wakes it soon.
    waiting: bool,
    failed: bool,
    /// The earliest reset after the last read; 0 when none is known.
    next_reset: i64,
}

/// An account a usage reset is being used on, until dropped.
struct Using<'a> {
    factory: &'a Factory,
    key: AccountKey,
}

impl<'a> Using<'a> {
    /// None when one is already being used on it.
    fn start(factory: &'a Factory, key: &AccountKey) -> Option<Using<'a>> {
        factory
            .resetting
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(key.clone())
            .then(|| Using { factory, key: key.clone() })
    }
}

impl Drop for Using<'_> {
    fn drop(&mut self) {
        self.factory.resetting.lock().unwrap_or_else(std::sync::PoisonError::into_inner).remove(&self.key);
    }
}

/// Whether to read the provider's usage now.
pub(crate) fn due(tracked: &Tracked, working: bool, now: i64) -> bool {
    if tracked.unlimited {
        return false;
    }
    let Some(read) = tracked.read else { return true };
    let mut wait = if working {
        WHILE_WORKING
    } else if tracked.worked {
        AFTER_WORK
    } else {
        WHILE_IDLE
    };
    if tracked.waiting || (tracked.next_reset != 0 && now >= tracked.next_reset) {
        wait = wait.min(AFTER_WORK);
    }
    if tracked.failed {
        wait = AFTER_ERROR;
    }
    now - read >= wait
}

/// Keeps every subscription's usage and plan current while the Factory lives.
pub(crate) async fn monitor(factory: Weak<Factory>) {
    let Some(wake) = factory.upgrade().map(|strong| strong.usage_wake.clone()) else { return };
    let mut tracked = HashMap::new();
    // At start with the computer, the first read waits for the network.
    crate::network::ready().await;
    loop {
        let Some(strong) = factory.upgrade() else { return };
        strong.check_usage(&mut tracked).await;
        // Switches whose provider has room again go back (friend bug #8 A1).
        crate::usage_switch::come_back(&strong).await;
        drop(strong);
        // A sign-in wakes it early.
        tokio::select! {
            () = tokio::time::sleep(TICK) => {}
            () = wake.notified() => {}
        }
    }
}

/// A provider's accounts by id, in order; one with no id ("") when it lists
/// none but may still be signed in (with an API key from the environment).
pub(crate) fn account_ids(provider: &v1::Provider) -> Vec<String> {
    if provider.accounts.is_empty() { vec![String::new()] } else { provider.accounts.iter().map(|account| account.id.clone()).collect() }
}

/// How a signed-in account's check went (`Factory::check_sign_ins`).
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct SignInCheck {
    /// The provider's name, such as "Anthropic".
    pub provider: String,
    /// The account's label, such as its email; empty when unknown.
    pub account: String,
    pub outcome: SignInOutcome,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum SignInOutcome {
    /// Its usage was read: the sign-in works.
    Works,
    /// It has no usage to read (an API key): nothing says it doesn't work.
    NoLimits,
    /// The provider refused it: it needs signing in again.
    Refused,
    /// The vault holding it is locked.
    Locked,
    /// The read failed otherwise: why.
    Failed(String),
}

/// An account's usage and plan, by provider and account id.
pub(crate) type AccountKey = (String, String);

impl Factory {
    async fn check_usage(&self, tracked: &mut HashMap<AccountKey, Tracked>) {
        let now = now_ms();
        let reread: HashSet<String> = std::mem::take(&mut *self.usage_reread.lock().unwrap_or_else(std::sync::PoisonError::into_inner));
        let (providers, working, waiting, reset) = {
            let inner = self.lock();
            let mut working = HashSet::new();
            // Providers an agent or a chat waits on.
            let mut waiting = HashSet::new();
            for (id, _) in inner.agents.iter().filter(|(_, runtime)| runtime.usage_wait.is_some()) {
                let model = inner
                    .config
                    .agent(id)
                    .map(|agent| agent.model.clone())
                    .or_else(|| self.chats.model_and_name(id).map(|(model, _)| model));
                if let Some((provider, _)) = model.as_deref().and_then(|model| model.split_once('/')) {
                    waiting.insert(provider.to_owned());
                }
            }
            for (id, runtime) in &inner.agents {
                if runtime.running
                    && let Some(agent) = inner.config.agent(id)
                    && let Some((provider, _)) = agent.model.split_once('/')
                {
                    working.insert(provider.to_owned());
                }
            }
            // An account at its limit until now has capacity again; a key out of
            // credit is tried again after a while, which may make it the one in use.
            let reset = inner
                .providers
                .iter()
                .flat_map(|provider| &provider.accounts)
                .any(|account| (account.exhausted_until != 0 && account.exhausted_until <= now) || account.out_of_credit);
            (inner.providers.clone(), working, waiting, reset)
        };
        if reset {
            self.refresh_models().await;
        }
        tracked.retain(|(id, account), _| {
            providers
                .iter()
                .any(|provider| provider.id == *id && !provider.signed_in_with.is_empty() && account_ids(provider).contains(account))
        });
        let mut named = false;
        for provider in providers {
            let id = provider.id.clone();
            if provider.signed_in_with.is_empty() {
                self.forget_usage(&id);
                continue;
            }
            // Locked away with the person's secrets (just after a start,
            // until the vault opens): nothing can be read yet, and nothing
            // is recorded, so it's read as soon as they open. Read as "no
            // limits", it would never have been read again.
            if self.models.sign_ins_locked(&id).await {
                continue;
            }
            // Requests go to the account in use; the others wait.
            let in_use = provider.accounts.iter().find(|account| account.in_use).map_or_else(String::new, |account| account.id.clone());
            for account in account_ids(&provider) {
                let key = (id.clone(), account.clone());
                let signed_in_with = provider.signed_in_with.clone();
                let entry = tracked.entry(key.clone()).or_default();
                if entry.signed_in_with != signed_in_with {
                    // A new sign-in: forget the old one's usage and plan and read them afresh.
                    *entry = Tracked { signed_in_with, ..Tracked::default() };
                    self.set_usage(&key, None);
                    self.set_plan(&key, None);
                } else if reread.contains(&id) {
                    // Signed in to again with the same method (a plan just
                    // upgraded, say), or Try now: read now, keeping what
                    // shows until then.
                    *entry = Tracked { signed_in_with, ..Tracked::default() };
                }
                entry.waiting = waiting.contains(&id);
                let is_working = working.contains(&id) && account == in_use;
                entry.worked |= is_working;
                if !due(entry, is_working, now_ms()) {
                    continue;
                }
                let (result, plan) = tokio::join!(self.models.usage(&id, &account), self.models.plan(&id, &account));
                named |= provider.accounts.iter().any(|known| known.id == account && known.label.is_empty());
                match plan {
                    // As the provider says it now: a new plan after an upgrade,
                    // none when it names none. A read that fails keeps the last.
                    Ok(plan) => self.set_plan(&key, Some(plan).filter(|plan| !plan.is_empty())),
                    Err(error) => tracing::warn!(provider = %id, %account, %error, "could not read the plan"),
                }
                let now = now_ms();
                entry.read = Some(now);
                entry.worked = is_working;
                match result {
                    Ok(None) => {
                        entry.unlimited = true;
                        self.set_usage(&key, None);
                        self.set_plan(&key, None);
                    }
                    Ok(Some(usage)) => {
                        entry.failed = false;
                        let resets = usage.windows.iter().map(|window| window.resets_at).filter(|at| *at > now);
                        entry.next_reset = resets.min().unwrap_or(0);
                        // The provider says the account has room for a
                        // pool's models (each window of their limits under
                        // 100%): its mark still there (a reset claimed on
                        // the provider's own site, or applied late) is
                        // forgotten, and waiting agents ask again. A model's
                        // own weekly limit at 100% (Opus's, U11) holds back
                        // only its own models.
                        let read = !usage.windows.is_empty();
                        let full: Vec<String> =
                            usage.windows.iter().filter(|window| window.used_percent >= 100.0).map(|window| window.id.clone()).collect();
                        self.set_usage(&key, Some(v1::ProviderUsage { updated: now, error: String::new(), ..usage }));
                        if read {
                            match self.models.forget_spent_with_room(&id, &account, &full).await {
                                Ok(true) => {
                                    // Shown at once, not at the old reset time:
                                    // agents switched away (#341) can go back.
                                    self.refresh_models().await;
                                    self.capacity_changed();
                                }
                                Ok(false) => {}
                                Err(error) => {
                                    tracing::warn!(provider = %id, %account, %error, "could not mark the account as having room");
                                }
                            }
                        }
                    }
                    Err(error) => {
                        entry.failed = true;
                        // A sign-in the provider won't refresh is marked so now:
                        // the account shows it, to sign in to again.
                        named |= error.is_refused();
                        self.usage_failed(&key, &error);
                    }
                }
            }
        }
        // Who an account is, unknown until now, was read with its plan; or
        // a sign-in was refused.
        if named {
            self.refresh_models().await;
        }
    }

    /// Reads every signed-in account's usage now, as a check that its
    /// sign-in works here (after a move, #243 S3): no model call, nothing
    /// spent. A sign-in the provider refuses is marked so, as any usage read
    /// marks it, and shows Sign in again. Each account's outcome, in order.
    pub(crate) async fn check_sign_ins(&self) -> Vec<SignInCheck> {
        let providers = self.lock().providers.clone();
        let mut checks = Vec::new();
        let mut refused = false;
        for provider in providers.iter().filter(|provider| !provider.signed_in_with.is_empty()) {
            for account in account_ids(provider) {
                let label = provider.accounts.iter().find(|known| known.id == account).map(|known| known.label.clone()).unwrap_or_default();
                let key = (provider.id.clone(), account.clone());
                let outcome = match self.models.usage(&provider.id, &account).await {
                    Ok(None) => SignInOutcome::NoLimits,
                    Ok(Some(usage)) => {
                        self.set_usage(&key, Some(v1::ProviderUsage { updated: now_ms(), error: String::new(), ..usage }));
                        SignInOutcome::Works
                    }
                    Err(error) if error.is_refused() => {
                        refused = true;
                        self.usage_failed(&key, &error);
                        SignInOutcome::Refused
                    }
                    Err(error) if error.is_locked() => SignInOutcome::Locked,
                    Err(error) => {
                        self.usage_failed(&key, &error);
                        SignInOutcome::Failed(error.to_string())
                    }
                };
                checks.push(SignInCheck { provider: provider.name.clone(), account: label, outcome });
            }
        }
        if refused {
            self.refresh_models().await;
        }
        checks
    }

    /// Keeps showing an account's last good read, with why it's not newer.
    fn usage_failed(&self, key: &AccountKey, error: &factory_agent::Error) {
        tracing::warn!(provider = %key.0, account = %key.1, %error, "could not read usage");
        let mut usage = self.lock().usage.get(key).cloned().unwrap_or_default();
        usage.error = error.to_string();
        self.set_usage(key, Some(usage));
    }

    /// Uses one of an account's usage resets, as its last usage read listed
    /// it (`ProviderUsage.resets`), and reads its usage again. Only the
    /// person does this, from the app: it can't be undone, so nothing in
    /// Claktory does it by itself. One at a time per account. The provider's
    /// answer, or a failure on the way, comes back as a sentence.
    pub async fn use_usage_reset(&self, provider_id: &str, account_id: &str, reset_id: &str) -> Result<v1::UsageResetAnswer, RequestError> {
        self.known_account(provider_id, account_id)?;
        let key = (provider_id.to_owned(), account_id.to_owned());
        let listed = self.lock().usage.get(&key).and_then(|usage| usage.resets.iter().find(|reset| reset.id == reset_id).cloned());
        let Some(reset) = listed else {
            return Err(RequestError::NoReset("That reset isn't available any more.".into()));
        };
        let Some(_using) = Using::start(self, &key) else {
            return Err(RequestError::Reset("A reset is already being used on this account.".into()));
        };
        tracing::info!(provider = provider_id, account = account_id, reset = %reset.label, "using a usage reset, as the person asked");
        let answered = self.models.use_reset(provider_id, account_id, reset).await;
        let answer = match answered {
            Ok(Some(answer)) => answer,
            Ok(None) => return Err(RequestError::NoReset("This account has no usage resets.".into())),
            Err(error) => {
                tracing::warn!(provider = provider_id, account = account_id, %error, "could not use a usage reset");
                let error = error.to_string();
                let error = error.trim_end_matches('.');
                v1::UsageResetAnswer {
                    reset: false,
                    message: format!("Couldn't use the reset: {error}. Nothing was used."),
                    ..v1::UsageResetAnswer::default()
                }
            }
        };
        tracing::info!(provider = provider_id, account = account_id, reset = answer.reset, message = %answer.message, "usage reset answered");
        // What it changed, whatever it said: the limits, the resets left and
        // whether requests go to the account.
        match self.models.usage(provider_id, account_id).await {
            Ok(Some(usage)) => self.set_usage(&key, Some(v1::ProviderUsage { updated: now_ms(), error: String::new(), ..usage })),
            Ok(None) => {}
            Err(error) => self.usage_failed(&key, &error),
        }
        if answer.reset {
            self.refresh_models().await;
            // Agents waiting out the limits ask again now. (Directly: the
            // refresh wakes them only when the providers listing changed,
            // and it carries the normal pool's marks alone, so a reset that
            // freed only the Fable pool looks like no change to it.)
            self.capacity_changed();
        }
        Ok(answer)
    }

    /// Forgets the usage and plan of all of a provider's accounts.
    fn forget_usage(&self, provider: &str) {
        let mut inner = self.lock();
        let before = (inner.usage.len(), inner.plans.len());
        inner.usage.retain(|(id, _), _| id != provider);
        inner.plans.retain(|(id, _), _| id != provider);
        if before != (inner.usage.len(), inner.plans.len()) {
            self.broadcast_workspace(&inner);
        }
    }

    /// Records an account's usage and tells clients if it changed.
    fn set_usage(&self, key: &AccountKey, usage: Option<v1::ProviderUsage>) {
        let mut inner = self.lock();
        let resets = |usage: Option<&v1::ProviderUsage>| {
            let resets = usage.map(|usage| usage.resets.iter().map(|reset| format!("{} ×{}", reset.label, reset.left)));
            resets.into_iter().flatten().collect::<Vec<_>>().join(", ")
        };
        // What the app's circles and cards show: each window's share used.
        let used = |usage: Option<&v1::ProviderUsage>| {
            let windows = usage.map(|usage| usage.windows.iter().map(|window| format!("{} {}%", window.id, window.used_percent)));
            windows.into_iter().flatten().collect::<Vec<_>>().join(", ")
        };
        let before = (resets(inner.usage.get(key)), used(inner.usage.get(key)));
        let changed = match usage {
            Some(usage) => inner.usage.insert(key.clone(), usage.clone()).as_ref() != Some(&usage),
            None => inner.usage.remove(key).is_some(),
        };
        if changed {
            let after = (resets(inner.usage.get(key)), used(inner.usage.get(key)));
            if after.0 != before.0 {
                tracing::info!(provider = %key.0, account = %key.1, resets = %after.0, "usage resets");
            }
            // Only when a share changes, not each read: a log shows what the
            // app does without a line every few minutes.
            if after.1 != before.1 && !after.1.is_empty() {
                tracing::info!(provider = %key.0, account = %key.1, used = %after.1, "usage");
            }
            self.broadcast_workspace(&inner);
        }
    }

    /// Records an account's plan and tells clients if it changed.
    fn set_plan(&self, key: &AccountKey, plan: Option<String>) {
        let mut inner = self.lock();
        let changed = match &plan {
            Some(plan) => inner.plans.insert(key.clone(), plan.clone()).as_ref() != Some(plan),
            None => inner.plans.remove(key).is_some(),
        };
        if changed {
            tracing::info!(provider = %key.0, account = %key.1, plan = plan.as_deref().unwrap_or(""), "subscription plan");
            self.broadcast_workspace(&inner);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: i64 = 1_800_000_000_000;

    fn read_at(read: i64) -> Tracked {
        Tracked { signed_in_with: "Anthropic (Claude Pro/Max)".into(), read: Some(read), ..Tracked::default() }
    }

    #[test]
    fn a_new_sign_in_is_read_at_once_and_one_without_limits_never_again() {
        assert!(due(&Tracked::default(), false, NOW));
        let unlimited = Tracked { unlimited: true, ..read_at(NOW - 60 * MINUTE) };
        assert!(!due(&unlimited, true, NOW));
    }

    #[test]
    fn idle_providers_are_read_every_fifteen_minutes_and_working_ones_every_five() {
        assert!(!due(&read_at(NOW - 14 * MINUTE), false, NOW));
        assert!(due(&read_at(NOW - 15 * MINUTE), false, NOW));
        assert!(!due(&read_at(NOW - 4 * MINUTE), true, NOW));
        assert!(due(&read_at(NOW - 5 * MINUTE), true, NOW));
    }

    #[test]
    fn work_that_ended_is_read_a_minute_later() {
        let worked = Tracked { worked: true, ..read_at(NOW - 30_000) };
        assert!(!due(&worked, false, NOW));
        assert!(due(&worked, false, NOW + 30_000));
    }

    #[test]
    fn a_limit_that_reset_is_read_again() {
        let resets = Tracked { next_reset: NOW - 1, ..read_at(NOW - 2 * MINUTE) };
        assert!(due(&resets, false, NOW));
        let later = Tracked { next_reset: NOW + 1, ..read_at(NOW - 2 * MINUTE) };
        assert!(!due(&later, false, NOW));
    }

    #[test]
    fn while_an_agent_waits_every_account_is_read_each_minute() {
        let waiting = Tracked { waiting: true, ..read_at(NOW - MINUTE + 1) };
        assert!(!due(&waiting, false, NOW));
        assert!(due(&Tracked { waiting: true, ..read_at(NOW - MINUTE) }, false, NOW));
        assert!(!due(&read_at(NOW - MINUTE), false, NOW));
    }

    #[test]
    fn a_failed_read_is_retried_after_five_minutes() {
        let failed = Tracked { failed: true, ..read_at(NOW - 4 * MINUTE) };
        assert!(!due(&failed, true, NOW));
        assert!(due(&failed, false, NOW + MINUTE));
    }
}
