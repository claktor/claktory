//! What API keys have to spend, as their provider says (#343 U8 and U14,
//! Account.credit): OpenRouter's GET /key, read at sign-in, then every few
//! minutes while agents work on the provider and every half hour while
//! they don't, and when the person looks at the accounts (ProvidersShown)
//! if it's more than a minute old. Only numbers are kept, never what the
//! provider wrote.

use std::collections::HashSet;
use std::sync::Weak;
use std::time::Duration;

use factory_agent::providers::keys::KeyCredit;
use factory_protocol::v1;

use crate::factory::Factory;
use crate::transcript::now_ms;
use crate::usage::{AccountKey, account_ids};

/// How often the schedule is checked; checking costs nothing.
const TICK: Duration = Duration::from_secs(15);
const MINUTE: i64 = 60_000;
/// Between reads while an agent on the provider works: today's spend moves.
const WHILE_WORKING: i64 = 5 * MINUTE;
/// Between reads while nothing runs on it: catches spending elsewhere and
/// credit added.
const WHILE_IDLE: i64 = 30 * MINUTE;
/// Between attempts after a read failed.
const AFTER_ERROR: i64 = 5 * MINUTE;
/// How old a read may be when the person looks at the accounts
/// (ProvidersShown) before it's read again.
const SHOWN: i64 = MINUTE;

/// Keeps every key's credit current while the Factory lives.
pub(crate) async fn monitor(factory: Weak<Factory>) {
    crate::network::ready().await;
    loop {
        let Some(strong) = factory.upgrade() else { return };
        strong.check_credit().await;
        drop(strong);
        tokio::time::sleep(TICK).await;
    }
}

/// Whether to read an account's credit now: last tried at `tried` (None:
/// never), which `failed`.
fn due(tried: Option<i64>, failed: bool, working: bool, now: i64) -> bool {
    let Some(tried) = tried else { return true };
    let wait = if failed {
        AFTER_ERROR
    } else if working {
        WHILE_WORKING
    } else {
        WHILE_IDLE
    };
    now - tried >= wait
}

impl Factory {
    async fn check_credit(&self) {
        let now = now_ms();
        let (accounts, working) = {
            let inner = self.lock();
            let mut working = HashSet::new();
            for (id, runtime) in &inner.agents {
                let model = inner
                    .config
                    .agent(id)
                    .map(|agent| agent.model.clone())
                    .or_else(|| self.chats.model_and_name(id).map(|(model, _)| model));
                if runtime.running
                    && let Some((provider, _)) = model.as_deref().and_then(|model| model.split_once('/'))
                {
                    working.insert(provider.to_owned());
                }
            }
            (self.credit_accounts(&inner), working)
        };
        self.forget_credit(&accounts);
        for key in accounts {
            let tried = self.credit_tried.lock().unwrap_or_else(std::sync::PoisonError::into_inner).get(&key).copied();
            let failed = self.lock().credit.get(&key).is_some_and(|last| !last.error.is_empty());
            if due(tried, failed, working.contains(&key.0), now) {
                self.read_credit(&key).await;
            }
        }
    }

    /// The signed-in accounts whose credit their provider tells.
    fn credit_accounts(&self, inner: &crate::factory::Inner) -> Vec<AccountKey> {
        inner
            .providers
            .iter()
            .filter(|provider| !provider.signed_in_with.is_empty() && self.models.reads_credit(&provider.id))
            .flat_map(|provider| account_ids(provider).into_iter().map(|account| (provider.id.clone(), account)))
            .collect()
    }

    /// ProvidersShown (friend bug #41): the person looks at the accounts, so
    /// each one's credit read more than `SHOWN` ago is read again now.
    pub(crate) async fn read_shown_credit(&self) {
        let now = now_ms();
        let accounts = self.credit_accounts(&self.lock());
        for key in accounts {
            let tried = self.credit_tried.lock().unwrap_or_else(std::sync::PoisonError::into_inner).get(&key).copied();
            if tried.is_none_or(|tried| now - tried >= SHOWN) {
                self.read_credit(&key).await;
            }
        }
    }

    /// Reads every account's credit of `provider` now (a sign-in, #343 U14:
    /// before the sign-in shows as done, so the step can say a new key has
    /// none).
    pub(crate) async fn read_provider_credit(&self, provider: &str) {
        if !self.models.reads_credit(provider) {
            return;
        }
        let accounts: Vec<AccountKey> = {
            let inner = self.lock();
            let Some(known) = inner.providers.iter().find(|known| known.id == provider) else { return };
            account_ids(known).into_iter().map(|account| (provider.to_owned(), account)).collect()
        };
        self.forget_credit_of(provider, &accounts);
        for key in accounts {
            self.read_credit(&key).await;
        }
    }

    /// Tests: `read_provider_credit`, as a sign-in does.
    #[doc(hidden)]
    pub async fn read_provider_credit_for_test(&self, provider: &str) {
        self.read_provider_credit(provider).await;
    }

    async fn read_credit(&self, key: &AccountKey) {
        let now = now_ms();
        self.credit_tried.lock().unwrap_or_else(std::sync::PoisonError::into_inner).insert(key.clone(), now);
        let (provider, account) = key;
        let read = self.models.account_credit(provider, account).await;
        let mut inner = self.lock();
        let name = inner.providers.iter().find(|known| known.id == *provider).map_or_else(|| provider.clone(), |known| known.name.clone());
        let credit = match read {
            // Nothing to say: locked away, or no such account any more.
            Ok(None) => return,
            Ok(Some(credit)) => view(&credit, &name, now),
            Err(error) => {
                let error = error.to_string();
                tracing::warn!(%provider, %account, %error, "could not read the key's credit");
                let last = inner.credit.get(key).cloned().unwrap_or_default();
                v1::AccountCredit { error, ..last }
            }
        };
        if inner.credit.get(key) != Some(&credit) {
            let before = inner.credit.insert(key.clone(), credit.clone());
            if before.as_ref().map(|before| &before.warning) != Some(&credit.warning) && !credit.warning.is_empty() {
                tracing::info!(%provider, %account, warning = %credit.warning, "key credit");
            }
            self.broadcast_workspace(&inner);
        }
    }

    /// Forgets the credit of accounts no longer signed in.
    fn forget_credit(&self, accounts: &[AccountKey]) {
        let mut inner = self.lock();
        let before = inner.credit.len();
        inner.credit.retain(|key, _| accounts.contains(key));
        self.credit_tried.lock().unwrap_or_else(std::sync::PoisonError::into_inner).retain(|key, _| accounts.contains(key));
        if inner.credit.len() != before {
            self.broadcast_workspace(&inner);
        }
    }

    /// `forget_credit` for one provider's accounts.
    fn forget_credit_of(&self, provider: &str, accounts: &[AccountKey]) {
        let mut inner = self.lock();
        let before = inner.credit.len();
        inner.credit.retain(|key, _| key.0 != provider || accounts.contains(key));
        if inner.credit.len() != before {
            self.broadcast_workspace(&inner);
        }
    }
}

/// Account.credit from what the provider said, read at `now`.
fn view(credit: &KeyCredit, provider: &str, now: i64) -> v1::AccountCredit {
    let limited = credit.limit.is_some();
    let left = credit.left.unwrap_or(0.0);
    let warning = if limited && left <= 0.0 {
        let refills = refill_ms(&credit.limit_reset, now)
            .and_then(|at| u64::try_from(at).ok())
            .and_then(factory_agent::accounts::local_time)
            .map(|at| format!(", or wait until it refills at {at}"))
            .unwrap_or_default();
        format!("This key's spending limit is used up. Raise it at {provider}{refills}.")
    } else if credit.never_paid {
        format!("This key has no credit yet. Add credit at {provider}.")
    } else {
        String::new()
    };
    v1::AccountCredit {
        limited,
        limit: credit.limit.unwrap_or(0.0),
        left,
        limit_reset: credit.limit_reset.clone(),
        spent_today: credit.spent_today,
        never_paid: credit.never_paid,
        warning,
        read_ms: now,
        error: String::new(),
        left_shown: if limited && left > 0.0 { format!("{} left", dollars(left)) } else { String::new() },
    }
}

/// "$1,234.50": dollars to the cent, with thousands set apart.
fn dollars(amount: f64) -> String {
    let cents = (amount * 100.0).round() as u64;
    let whole = (cents / 100).to_string();
    let mut grouped = String::new();
    for (index, digit) in whole.chars().enumerate() {
        if index > 0 && (whole.len() - index).is_multiple_of(3) {
            grouped.push(',');
        }
        grouped.push(digit);
    }
    format!("${grouped}.{:02}", cents % 100)
}

/// When a limit that refills `reset` ("daily", "weekly" or "monthly", in
/// UTC) next does, after `now`; None when it never does.
fn refill_ms(reset: &str, now: i64) -> Option<i64> {
    use jiff::{ToSpan, civil::Weekday, tz::TimeZone};
    let today = jiff::Timestamp::from_millisecond(now).ok()?.to_zoned_datetime(TimeZone::UTC).date();
    let next = match reset {
        "daily" => today.checked_add(1.day()).ok()?,
        "weekly" => today.nth_weekday(1, Weekday::Monday).ok()?,
        "monthly" => today.first_of_month().checked_add(1.month()).ok()?,
        _ => return None,
    };
    Some(next.to_zoned_datetime(TimeZone::UTC).ok()?.timestamp().as_millisecond())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Wednesday 2026-11-18 15:00 UTC.
    const NOW: i64 = 1_795_014_000_000;

    fn at(text: &str) -> i64 {
        text.parse::<jiff::Timestamp>().expect("a time").as_millisecond()
    }

    #[test]
    fn what_is_left_reads_in_dollars_only_while_a_limit_has_some() {
        let key = |limit: Option<f64>, left: Option<f64>| KeyCredit { limit, left, ..KeyCredit::default() };
        assert_eq!(view(&key(Some(50.0), Some(12.4)), "OpenRouter", NOW).left_shown, "$12.40 left");
        assert_eq!(view(&key(Some(5000.0), Some(1234.505)), "OpenRouter", NOW).left_shown, "$1,234.51 left");
        assert_eq!(view(&key(Some(50.0), Some(0.0)), "OpenRouter", NOW).left_shown, "");
        assert_eq!(view(&key(None, None), "OpenRouter", NOW).left_shown, "");
        assert_eq!(dollars(0.004), "$0.00");
        assert_eq!(dollars(999.999), "$1,000.00");
    }

    #[test]
    fn limits_refill_at_the_next_utc_boundary() {
        assert_eq!(NOW, at("2026-11-18T15:00:00Z"));
        assert_eq!(refill_ms("daily", NOW), Some(at("2026-11-19T00:00:00Z")));
        assert_eq!(refill_ms("weekly", NOW), Some(at("2026-11-23T00:00:00Z")));
        assert_eq!(refill_ms("monthly", NOW), Some(at("2026-12-01T00:00:00Z")));
        assert_eq!(refill_ms("", NOW), None);
    }

    #[test]
    fn a_key_that_cant_run_paid_models_says_why() {
        let none_yet = view(&KeyCredit { never_paid: true, ..KeyCredit::default() }, "OpenRouter", NOW);
        assert_eq!(none_yet.warning, "This key has no credit yet. Add credit at OpenRouter.");
        assert!(!none_yet.limited && none_yet.read_ms == NOW);

        let spent = KeyCredit { limit: Some(10.0), left: Some(-0.01), limit_reset: "weekly".into(), spent_today: 3.5, never_paid: false };
        let spent = view(&spent, "OpenRouter", NOW);
        let monday = factory_agent::accounts::local_time(u64::try_from(at("2026-11-23T00:00:00Z")).expect("ms")).expect("local");
        assert_eq!(
            spent.warning,
            format!("This key's spending limit is used up. Raise it at OpenRouter, or wait until it refills at {monday}.")
        );
        assert_eq!((spent.limited, spent.limit, spent.left, spent.spent_today), (true, 10.0, -0.01, 3.5));

        let fine = view(&KeyCredit { limit: Some(10.0), left: Some(4.2), ..KeyCredit::default() }, "OpenRouter", NOW);
        assert_eq!(fine.warning, "");
    }

    #[test]
    fn reads_come_often_while_working_and_soon_after_a_failure() {
        assert!(due(None, false, false, NOW));
        assert!(!due(Some(NOW), false, true, NOW + 4 * MINUTE));
        assert!(due(Some(NOW), false, true, NOW + 5 * MINUTE));
        assert!(!due(Some(NOW), false, false, NOW + 29 * MINUTE));
        assert!(due(Some(NOW), false, false, NOW + 30 * MINUTE));
        assert!(due(Some(NOW), true, false, NOW + 5 * MINUTE));
    }
}
