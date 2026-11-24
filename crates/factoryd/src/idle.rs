//! Idle compaction: an agent left idle with a long conversation compacts it
//! just before its prompt cache expires, as Claude Code does. Coming back to
//! it later then costs a short summary to cache again, not the whole
//! conversation, which the cache would have lost by then.
//!
//! It happens only when Claktory knows how long the cache lasts
//! (`Config::cache_ttl`), as late as is safe (`delay`): after 58 minutes of
//! Anthropic's hour, 28 of OpenAI's 30 minutes, or 4 of Anthropic's 5 when
//! the person turned the short cache on too. Not for a provider whose cache
//! lifetime it can't choose or know. Only for a conversation of at least
//! `floor` tokens, with nothing queued for the agent and its model unchanged
//! since its last response; once per response, and never late: a check that
//! comes more than `late` after the moment (the machine slept, Factory
//! restarted) lets it be, since the cache may be gone and summarizing then
//! costs more than it saves. New work for the agent cancels it
//! (`Runtime::idle`).

use std::sync::Arc;
use std::sync::Weak;
use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};
use std::time::Duration;

use factory_agent::compaction::{Reason, latest_context_tokens};
use factory_agent::{Message, StopReason};

use crate::factory::{Factory, Runtime};
use crate::transcript::now_ms;

/// A conversation must hold at least this many tokens…
const FLOOR_MAX: u64 = 200_000;
/// …or this share of a smaller model's window…
const FLOOR_SHARE: f64 = 0.75;
/// …and never less than this: smaller windows get no idle compaction.
const FLOOR_MIN: u64 = 100_000;

pub(crate) struct State {
    every_ms: AtomicU64,
    /// Added to the clock, for tests.
    offset: AtomicI64,
    /// Wakes the check when its interval changes.
    wake: Arc<tokio::sync::Notify>,
}

impl Default for State {
    fn default() -> State {
        State { every_ms: AtomicU64::new(15_000), offset: AtomicI64::new(0), wake: Arc::default() }
    }
}

impl State {
    fn now(&self) -> u64 {
        u64::try_from(now_ms() + self.offset.load(Ordering::Relaxed)).unwrap_or(0)
    }
}

impl Factory {
    /// How often idle agents are checked (15 seconds); tests make it shorter.
    pub fn set_idle_check_every(&self, every: Duration) {
        self.idle.every_ms.store(u64::try_from(every.as_millis()).unwrap_or(u64::MAX), Ordering::Relaxed);
        self.idle.wake.notify_one();
    }

    /// Sets the clock idle compaction goes by `ahead` of the real one, for
    /// tests: responses are stamped with the real time.
    pub fn set_idle_clock_ahead(&self, ahead: Duration) {
        self.idle.offset.store(i64::try_from(ahead.as_millis()).unwrap_or(i64::MAX), Ordering::Relaxed);
        self.idle.wake.notify_one();
    }
}

/// The longest margin before a cache expires: time for a check that comes
/// late (`late`) and for the summary's request to start before it does.
const MARGIN_MAX_MS: u64 = 2 * 60_000;
/// A cache this short compacts only when the person asked for it
/// (`compaction.idle_short_cache`): it would compact far more often.
const SHORT_MS: u64 = 5 * 60_000;

/// How long before a cache of `lifetime` expires its idle conversation
/// compacts: 2 minutes, or a fifth of a short lifetime.
fn margin(lifetime: u64) -> u64 {
    MARGIN_MAX_MS.min(lifetime / 5)
}

/// How late a check may come after the moment and still compact: half the
/// margin, so the summary's request has the other half to start in.
pub(crate) fn late(lifetime: u64) -> u64 {
    margin(lifetime) / 2
}

/// How long after a response's request started its conversation compacts
/// while idle, for a prompt cache that lasts `lifetime` (ms) from then: as
/// late as can be, `margin` before it expires (58 minutes of an hour, 28 of
/// 30, 4 of 5). None for a cache of 5 minutes or less unless `short`.
pub(crate) fn delay(lifetime: u64, short: bool) -> Option<u64> {
    if lifetime == 0 || (lifetime <= SHORT_MS && !short) {
        return None;
    }
    Some(lifetime - margin(lifetime))
}

/// The fewest tokens a conversation on a model with `window` tokens must
/// hold to compact while idle; None when its window is too small.
pub(crate) fn floor(window: u64) -> Option<u64> {
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss, clippy::cast_precision_loss)] // tokens
    let share = (window as f64 * FLOOR_SHARE) as u64;
    Some(share.min(FLOOR_MAX)).filter(|floor| *floor >= FLOOR_MIN)
}

/// Checks idle agents now and then, for as long as Factory runs.
pub(crate) async fn monitor(factory: Weak<Factory>) {
    loop {
        let Some((every, wake)) =
            factory.upgrade().map(|factory| (factory.idle.every_ms.load(Ordering::Relaxed), factory.idle.wake.clone()))
        else {
            return;
        };
        tokio::select! {
            () = tokio::time::sleep(Duration::from_millis(every)) => {}
            () = wake.notified() => {}
        }
        let Some(factory) = factory.upgrade() else { return };
        check(&factory);
        crate::unload::sweep(&factory);
    }
}

/// When an agent's conversation compacts while idle, from its last
/// response; None when it never will for that response.
struct Due {
    /// The response's timestamp.
    stamp: u64,
    /// The moment, on the idle clock.
    at: u64,
    /// How late after it a check may still compact.
    late: u64,
    /// Its conversation's size, and whether that's enough to.
    tokens: u64,
    long: bool,
}

/// When `agent`'s conversation compacts while idle, if it hasn't been
/// tried for its last response yet.
fn due(config: &crate::config::Config, windows: &[(String, u64)], agent: &crate::config::AgentConfig, runtime: &Runtime) -> Option<Due> {
    let transcript = runtime.transcript.as_ref()?;
    // Its last response, on the model it has now; a failed or stopped
    // one may not have cached anything.
    let Some(Message::Assistant(last)) = transcript.messages().last() else { return None };
    if matches!(last.stop_reason, StopReason::Error | StopReason::Aborted)
        || format!("{}/{}", last.provider, last.model) != agent.model
        || runtime.idle_tried == last.timestamp
    {
        return None;
    }
    let (stamp, tokens) = (last.timestamp, latest_context_tokens(transcript.messages()).unwrap_or(0));
    let window = windows.iter().find(|(id, _)| *id == agent.model).map(|(_, window)| *window)?;
    let (Some(ttl), Some(floor)) = (config.cache_ttl(&agent.model), floor(window)) else { return None };
    let lifetime = ttl.millis();
    let delay = delay(lifetime, config.compaction.idle_short_cache)?;
    Some(Due { stamp, at: stamp + delay, late: late(lifetime), tokens, long: tokens >= floor })
}

/// Starts compacting every agent that's due to now.
fn check(factory: &Arc<Factory>) {
    let now = factory.idle.now();
    let mut inner = factory.lock();
    let config = inner.config.clone();
    if !config.compaction.idle {
        return;
    }
    let windows: Vec<(String, u64)> = inner.models.iter().map(|model| (model.id.clone(), model.context_window)).collect();
    let mut started = false;
    for agent in &config.agents {
        let Some(runtime) = inner.agents.get_mut(&agent.id) else { continue };
        if runtime.running || runtime.has_ready() || runtime.compact.is_some() {
            continue;
        }
        let Some(due) = due(&config, &windows, agent, runtime) else { continue };
        if now < due.at {
            continue;
        }
        runtime.idle_tried = due.stamp;
        if now > due.at + due.late || !due.long {
            continue;
        }
        let ttl = config.cache_ttl(&agent.model).map(|ttl| ttl.as_str()).unwrap_or_default();
        tracing::info!(agent = %agent.id, tokens = due.tokens, ttl, "compacting while idle, before the prompt cache expires");
        runtime.compact = Some(Reason::Idle);
        runtime.running = true;
        tokio::spawn(factory.clone().run(agent.id.clone()));
        started = true;
    }
    if started {
        factory.broadcast_workspace(&inner);
    }
}

/// Whether `agent`'s conversation may still compact while idle: it's
/// long enough and the moment hasn't passed. It stays in memory until
/// then (`crate::unload`).
pub(crate) fn may_compact(factory: &Factory, inner: &crate::factory::Inner, agent: &crate::config::AgentConfig) -> bool {
    if !inner.config.compaction.idle {
        return false;
    }
    let Some(runtime) = inner.agents.get(&agent.id) else { return false };
    let windows: Vec<(String, u64)> = inner.models.iter().map(|model| (model.id.clone(), model.context_window)).collect();
    due(&inner.config, &windows, agent, runtime).is_some_and(|due| due.long && factory.idle.now() <= due.at + due.late)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_cache_compacts_as_late_as_is_safe_and_5_minutes_only_when_asked() {
        const MINUTE: u64 = 60_000;
        assert_eq!(delay(60 * MINUTE, false), Some(58 * MINUTE));
        assert_eq!(delay(60 * MINUTE, true), Some(58 * MINUTE));
        assert_eq!(late(60 * MINUTE), MINUTE);
        assert_eq!(delay(30 * MINUTE, false), Some(28 * MINUTE));
        assert_eq!(delay(12 * 60 * MINUTE, false), Some((12 * 60 - 2) * MINUTE));
        assert_eq!(delay(5 * MINUTE, false), None);
        assert_eq!(delay(5 * MINUTE, true), Some(4 * MINUTE));
        assert_eq!(late(5 * MINUTE), 30_000);
        assert_eq!(delay(0, true), None);
    }

    #[test]
    fn the_floor_is_150k_on_200k_at_most_200k_and_none_below_100k() {
        assert_eq!(floor(200_000), Some(150_000));
        assert_eq!(floor(1_000_000), Some(200_000));
        assert_eq!(floor(140_000), Some(105_000));
        assert_eq!(floor(128_000), None);
    }
}
