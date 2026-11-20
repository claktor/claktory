//! Idle compaction: an agent left idle with a long conversation compacts it
//! just before its prompt cache expires, as Claude Code does. Coming back to
//! it later then costs a short summary to cache again, not the whole
//! conversation, which the cache would have lost by then.
//!
//! It happens only when Claktory knows how long the cache lasts: an hour
//! (after 54 minutes idle), or 5 minutes when the person turned the short
//! cache on too (after 4). Not for a provider whose cache lifetime it can't
//! choose or know. Only for a conversation of at least `floor` tokens, with
//! nothing queued for the agent and its model unchanged since its last
//! response; once per response, and never late: a check that comes more than
//! a minute after the moment (the machine slept, Factory restarted) lets it
//! be, since the cache may be gone and summarizing then costs more than it
//! saves. New work for the agent cancels it (`Runtime::idle`).

use std::sync::Arc;
use std::sync::Weak;
use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};
use std::time::Duration;

use factory_agent::compaction::{Reason, latest_context_tokens};
use factory_agent::{CacheTtl, Message, StopReason};

use crate::factory::Factory;
use crate::transcript::now_ms;

/// A conversation must hold at least this many tokens…
const FLOOR_MAX: u64 = 200_000;
/// …or this share of a smaller model's window…
const FLOOR_SHARE: f64 = 0.75;
/// …and never less than this: smaller windows get no idle compaction.
const FLOOR_MIN: u64 = 100_000;

/// A check this long after the moment still compacts; later, it lets it be.
const LATE_MS: u64 = 60_000;

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

/// How long after a response's request started its conversation compacts
/// while idle: 90% of an hour (54 minutes), or a minute before 5 minutes run
/// out when `short` is on; None for a 5-minute cache otherwise.
pub(crate) fn delay(ttl: CacheTtl, short: bool) -> Option<u64> {
    if ttl == CacheTtl::FiveMinutes && !short {
        return None;
    }
    let ttl = ttl.millis();
    Some(ttl - (ttl / 10).max(60_000))
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
    }
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
        if runtime.running || !runtime.queue.is_empty() || runtime.compact.is_some() {
            continue;
        }
        let Some(transcript) = runtime.transcript.as_ref() else { continue };
        // Its last response, on the model it has now; a failed or stopped
        // one may not have cached anything.
        let Some(Message::Assistant(last)) = transcript.messages().last() else { continue };
        if matches!(last.stop_reason, StopReason::Error | StopReason::Aborted)
            || format!("{}/{}", last.provider, last.model) != agent.model
            || runtime.idle_tried == last.timestamp
        {
            continue;
        }
        let (stamp, tokens) = (last.timestamp, latest_context_tokens(transcript.messages()).unwrap_or(0));
        let Some(window) = windows.iter().find(|(id, _)| *id == agent.model).map(|(_, window)| *window) else { continue };
        let (Some(ttl), Some(floor)) = (config.cache_ttl(&agent.model), floor(window)) else { continue };
        let Some(delay) = delay(ttl, config.compaction.idle_short_cache) else { continue };
        let due = stamp + delay;
        if now < due {
            continue;
        }
        runtime.idle_tried = stamp;
        if now > due + LATE_MS || tokens < floor {
            continue;
        }
        tracing::info!(agent = %agent.id, tokens, ttl = ttl.as_str(), "compacting while idle, before the prompt cache expires");
        runtime.compact = Some(Reason::Idle);
        runtime.running = true;
        tokio::spawn(factory.clone().run(agent.id.clone()));
        started = true;
    }
    if started {
        factory.broadcast_workspace(&inner);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_hour_s_cache_compacts_after_54_minutes_and_5_minutes_only_when_asked() {
        assert_eq!(delay(CacheTtl::OneHour, false), Some(54 * 60_000));
        assert_eq!(delay(CacheTtl::OneHour, true), Some(54 * 60_000));
        assert_eq!(delay(CacheTtl::FiveMinutes, false), None);
        assert_eq!(delay(CacheTtl::FiveMinutes, true), Some(4 * 60_000));
    }

    #[test]
    fn the_floor_is_150k_on_200k_at_most_200k_and_none_below_100k() {
        assert_eq!(floor(200_000), Some(150_000));
        assert_eq!(floor(1_000_000), Some(200_000));
        assert_eq!(floor(140_000), Some(105_000));
        assert_eq!(floor(128_000), None);
    }
}
