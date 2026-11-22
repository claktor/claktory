//! Apps' events, to the agents subscribed to them that have the event's
//! scope. Bursts are folded: an agent hears an app at most once per window
//! (5 s), what came meanwhile in one event, a line each; an event whose
//! dedupe key was seen lately is dropped.

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use factory_protocol::apps::Emit;

use crate::factory::Factory;

/// The dedupe keys remembered per app.
const KEYS_KEPT: usize = 512;
/// The most an event's text may be.
const TEXT_LIMIT: usize = 8_000;

pub(crate) struct Events {
    state: Mutex<State>,
    /// How long an agent's window lasts, in ms.
    window_ms: AtomicU64,
}

impl Default for Events {
    fn default() -> Self {
        Events { state: Mutex::default(), window_ms: AtomicU64::new(5_000) }
    }
}

#[derive(Default)]
struct State {
    /// Each app's recent dedupe keys.
    seen: HashMap<String, VecDeque<String>>,
    /// The texts waiting for each (app, agent) window to end; a window is
    /// open while its entry is there.
    waiting: HashMap<(String, String), Vec<String>>,
}

impl Factory {
    /// How long agents' windows for app events last (5 s); tests make it
    /// shorter.
    pub fn set_app_event_window(&self, window: Duration) {
        self.apps.events.window_ms.store(u64::try_from(window.as_millis()).unwrap_or(u64::MAX), Ordering::Relaxed);
    }
}

/// An app emitted an event.
pub(super) fn emit(factory: &Arc<Factory>, emitted: Emit) {
    let Some(manifest) = factory.apps.manifest(&emitted.app_id) else { return };
    // Its kind and scope are the manifest's, whatever the app says.
    let Some(spec) = manifest.event(&emitted.kind) else {
        tracing::warn!(app = %emitted.app_id, kind = %emitted.kind, "an app emitted an event its manifest doesn't declare");
        return;
    };
    let events = &factory.apps.events;
    if !emitted.dedupe_key.is_empty() {
        let mut state = events.state.lock().expect("events");
        let seen = state.seen.entry(emitted.app_id.clone()).or_default();
        if seen.contains(&emitted.dedupe_key) {
            return;
        }
        if seen.len() >= KEYS_KEPT {
            seen.pop_front();
        }
        seen.push_back(emitted.dedupe_key.clone());
    }
    let text: String = emitted.text.trim().chars().take(TEXT_LIMIT).collect();
    if text.is_empty() {
        return;
    }
    let agents: Vec<String> = {
        let inner = factory.lock();
        let Some(app) = inner.config.apps.iter().find(|app| app.id == emitted.app_id) else { return };
        app.subscriptions
            .get(&spec.kind)
            .into_iter()
            .flatten()
            .filter(|agent| inner.config.agent(agent).is_some())
            .filter(|agent| app.grants.get(*agent).is_some_and(|scopes| scopes.contains(&spec.scope)))
            .cloned()
            .collect()
    };
    for agent in agents {
        let key = (emitted.app_id.clone(), agent.clone());
        let open = {
            let mut state = events.state.lock().expect("events");
            match state.waiting.get_mut(&key) {
                Some(waiting) => {
                    waiting.push(text.clone());
                    true
                }
                None => {
                    state.waiting.insert(key.clone(), Vec::new());
                    false
                }
            }
        };
        if open {
            continue;
        }
        deliver(factory, &key.0, &manifest.name, &agent, &text);
        window(factory, key, manifest.name.clone());
    }
}

/// Waits out an agent's window, then delivers what came meanwhile in one
/// event (opening another window), or closes it.
fn window(factory: &Arc<Factory>, key: (String, String), name: String) {
    let weak = Arc::downgrade(factory);
    let wait = Duration::from_millis(factory.apps.events.window_ms.load(Ordering::Relaxed));
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(wait).await;
            let Some(factory) = weak.upgrade() else { return };
            let texts = {
                let mut state = factory.apps.events.state.lock().expect("events");
                let texts = state.waiting.get_mut(&key).map(std::mem::take).unwrap_or_default();
                if texts.is_empty() {
                    state.waiting.remove(&key);
                }
                texts
            };
            if texts.is_empty() {
                return;
            }
            deliver(&factory, &key.0, &name, &key.1, &texts.join("\n"));
        }
    });
}

fn deliver(factory: &Arc<Factory>, app: &str, name: &str, agent: &str, text: &str) {
    let first = text.lines().next().unwrap_or_default();
    let summary: String =
        if first.chars().count() > 120 { format!("{}…", first.chars().take(119).collect::<String>()) } else { first.to_owned() };
    let event = factory_agent::Event {
        source: factory_agent::EventSource::App,
        from: name.to_owned(),
        id: app.to_owned(),
        summary,
        transient: false,
        pipe_seqs: Vec::new(),
    };
    factory.deliver_event(agent, text.to_owned(), event);
}
