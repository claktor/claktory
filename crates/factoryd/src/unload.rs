//! Unloading idle agents' conversations (#377 step 3, Polpo's
//! factoryd-memory.md): an agent whose conversation nothing has needed for
//! an hour lets it go from memory, and it's read again from its files when
//! something needs it (`Factory::ensure_loaded`), as after a restart.
//!
//! Only when that changes nothing anyone sees or the agent does: it isn't
//! running, nothing queued for it can go to it now (a quiet message waits
//! for something else to wake it) or is being handed to it, it doesn't wait
//! (for an account, the network, the vault), nothing of it waits for the
//! person, no device shows it, it may not compact while idle any more, and
//! what it holds reads back the same (`Transcript::reads_back_the_same`:
//! all saved, nothing shown for display only). Checked under the state
//! lock, with no save of it in progress.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use crate::config::AgentConfig;
use crate::factory::{Factory, Inner, Runtime};
use crate::transcript::now_ms;

pub(crate) struct State {
    after_ms: AtomicU64,
}

impl Default for State {
    fn default() -> State {
        State { after_ms: AtomicU64::new(60 * 60_000) }
    }
}

impl Factory {
    /// How long an idle agent's conversation stays in memory unused (an
    /// hour); tests make it shorter.
    pub fn set_unload_after(&self, after: Duration) {
        self.unload.after_ms.store(u64::try_from(after.as_millis()).unwrap_or(u64::MAX), Ordering::Relaxed);
    }

    /// Whether the agent's conversation is in memory now.
    pub fn conversation_held(&self, id: &str) -> bool {
        self.lock().agents.get(id).is_some_and(|runtime| runtime.transcript.is_some())
    }
}

/// Whether `agent`'s conversation can go, as far as the state shows.
fn idle(factory: &Factory, inner: &Inner, agent: &AgentConfig, now: i64, after: i64) -> bool {
    let Some(runtime) = inner.agents.get(&agent.id) else { return false };
    let Some(transcript) = runtime.transcript.as_ref() else { return false };
    now.saturating_sub(runtime.used_ms) > after
        && quiet(runtime)
        && transcript.waiting_seqs().is_empty()
        && transcript.reads_back_the_same()
        && inner.state.selected_agent.as_deref() != Some(agent.id.as_str())
        && !crate::idle::may_compact(factory, inner, agent)
}

/// Nothing runs or waits, and nothing queued for it can go to it now.
fn quiet(runtime: &Runtime) -> bool {
    !runtime.running
        && !runtime.stopped
        && !runtime.forking
        && runtime.abort.is_none()
        && runtime.run.is_none()
        // Quiet messages wait for something else to wake it.
        && !runtime.has_ready()
        && runtime.taken.is_empty()
        && runtime.compact.is_none()
        && runtime.usage_wait.is_none()
        && runtime.locked_since == 0
        && runtime.network_since == 0
}

/// Unloads the conversations of agents idle for long enough.
pub(crate) fn sweep(factory: &Factory) {
    let after = i64::try_from(factory.unload.after_ms.load(Ordering::Relaxed)).unwrap_or(i64::MAX);
    let now = now_ms();
    let candidates: Vec<String> = {
        let inner = factory.lock();
        inner.config.agents.iter().filter(|agent| idle(factory, &inner, agent, now, after)).map(|agent| agent.id.clone()).collect()
    };
    for id in candidates {
        // Read from the store, without the state lock.
        if factory.has_waiting(&id) {
            continue;
        }
        let mut inner = factory.lock();
        // Again under the lock: it may have changed meanwhile.
        let Some(agent) = inner.config.agent(&id) else { continue };
        if !idle(factory, &inner, agent, now_ms(), after) {
            continue;
        }
        let Some(runtime) = inner.agents.get_mut(&id) else { continue };
        // Never while its files are written.
        let writes = runtime.writes.clone();
        let Ok(_writing) = writes.try_lock() else { continue };
        let Some(transcript) = runtime.transcript.take() else { continue };
        runtime.held_tokens = transcript.context_tokens();
        drop(inner);
        drop(transcript);
        tracing::info!(agent = %id, "unloaded its conversation: idle and unused");
    }
}
