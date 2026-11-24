//! Work in progress survives restarts. Every message for an agent is recorded
//! in the store when it arrives and when it's delivered, and every run when it
//! starts and ends (see `store`). When Factory starts again (after an update,
//! say), agents whose run never ended carry on: they're told Factory
//! restarted, then get what was waiting. Background tasks that were running
//! are watched again, so their agents still hear when they finish.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use factory_agent::{Finished, Task};
use serde::Deserialize;

use crate::config::State;
use crate::factory::{Factory, Inner, OpenError, Queued, Runtime, host_event};
use crate::home::Home;
use crate::store::{Outcome, Store};
use crate::update::VERSION;

impl Factory {
    /// The agent's current run, started now if it isn't in one.
    pub(crate) fn begin_run(&self, id: &str, runtime: &mut Runtime) -> Option<i64> {
        if runtime.run.is_none() {
            match self.store.start_run(id, VERSION) {
                Ok(run) => {
                    runtime.run = Some(run);
                    self.keep_awake.runs_changed();
                }
                Err(error) => tracing::error!(agent = %id, %error, "could not record a run"),
            }
        }
        runtime.run
    }

    /// Marks messages handed to the agent delivered, once its saved
    /// conversation holds them (`count` of them, oldest first), or all of them.
    pub(crate) fn delivered(&self, runtime: &mut Runtime, count: Option<usize>) {
        let count = count.unwrap_or(runtime.taken.len()).min(runtime.taken.len());
        for (queued, run) in runtime.taken.drain(..count) {
            self.settle(std::slice::from_ref(&queued), Outcome::Delivered { run });
        }
    }

    /// Stops the current reply without dropping what's queued, then records its end.
    pub(crate) fn stop_run(&self, runtime: &mut Runtime) {
        runtime.stopped = true;
        if let Some(abort) = runtime.abort.take() {
            abort.abort();
        }
        self.end_run(runtime, "stopped");
    }

    /// Ends the agent's run, if it's in one; returns whether it was.
    pub(crate) fn end_run(&self, runtime: &mut Runtime, outcome: &str) -> bool {
        // The run reached them, whether or not it got to save them.
        self.delivered(runtime, None);
        runtime.failed = false;
        runtime.used_ms = crate::transcript::now_ms();
        let Some(run) = runtime.run.take() else { return false };
        self.keep_awake.runs_changed();
        match self.store.end_run(run, outcome) {
            Ok(Some((agent, presence))) => self.ran(&agent, presence),
            Ok(None) => {}
            Err(error) => tracing::error!(run, %error, "could not record the end of a run"),
        }
        true
    }

    /// A run of the agent finished or failed: unless the person is looking
    /// at it, its entry shows there's something new until they open it.
    pub(crate) fn mark_unseen(&self, inner: &mut Inner, id: &str) {
        if inner.state.selected_agent.as_deref() == Some(id) || !inner.state.unseen.insert(id.to_owned()) {
            return;
        }
        if let Err(error) = self.store.save_state(&inner.state) {
            tracing::error!(agent = %id, %error, "could not record that the agent has something new");
        }
    }

    /// Records that messages reached their agent, or never will.
    pub(crate) fn settle(&self, messages: &[Queued], outcome: Outcome) {
        if !messages.is_empty()
            && let Err(error) = self.store.settle(messages, outcome)
        {
            tracing::error!(%error, "could not record what became of messages");
        }
    }

    pub(crate) fn task_started(&self, agent: &str, task: &Task) {
        if let Err(error) = self.store.start_task(agent, task) {
            tracing::error!(agent, task = %task.id, %error, "could not record a background task");
        }
        crate::extensions::tasks::refresh_view(self);
    }

    /// A background task ended: recorded, and its agent gets an event.
    pub(crate) fn task_finished(self: &Arc<Self>, agent: &str, mut done: Finished) {
        let stopped_by = self.lock().stopping.remove(&(agent.to_owned(), done.id.clone()));
        if let Some(by) = stopped_by {
            done.status = format!("stopped by {by}");
        }
        if let Err(error) = self.store.end_task(agent, &done) {
            tracing::error!(agent, task = %done.id, %error, "could not record the end of a background task");
        }
        let event = factory_agent::Event {
            source: factory_agent::EventSource::Task,
            from: done.id.clone(),
            id: done.id.clone(),
            summary: done.summary(),
            transient: false,
            pipe_seqs: Vec::new(),
            provenance: None,
        };
        self.deliver_event(agent, done.text(), event);
        crate::extensions::tasks::refresh_view(self);
    }

    /// Before the backend stops (on SIGTERM, from its supervisor at a
    /// restart or shutdown): lets the transcript writes under way finish,
    /// for up to `within`. What's left is picked up at the next start.
    pub async fn finish_writes(&self, within: std::time::Duration) {
        let writes: Vec<_> = self.lock().agents.values().map(|runtime| runtime.writes.clone()).collect();
        let deadline = tokio::time::Instant::now() + within;
        for write in writes {
            if tokio::time::timeout_at(deadline, write.lock()).await.is_err() {
                tracing::warn!("stopping before every transcript was written");
                return;
            }
        }
    }

    /// Picks up where the last Factory stopped: agents carry on with what
    /// they were doing and what was waiting for them, and background tasks
    /// are watched until they end.
    pub(crate) fn resume(self: &Arc<Self>) -> Result<(), rusqlite::Error> {
        let interrupted = self.store.interrupted_runs()?;
        let mut waiting = self.store.waiting()?;
        let mut agents: BTreeSet<String> = interrupted.keys().cloned().collect();
        agents.extend(waiting.keys().cloned());
        let mut inner = self.lock();
        let mut resuming = Vec::new();
        for id in agents {
            let messages = waiting.remove(&id).unwrap_or_default();
            if inner.config.agent(&id).is_none() && !crate::chats::kept(self, &id) {
                self.settle(&messages, Outcome::Dropped);
                continue;
            }
            let mut queue = Vec::with_capacity(messages.len() + 1);
            if let Some((stopped_on, started)) = interrupted.get(&id) {
                let rebooted = factory_agent::tools::bash::started_before_boot(u64::try_from(*started).unwrap_or(0));
                let (text, event) = resume_note(stopped_on, rebooted);
                let mut note = Queued::event(text, event);
                note.id = self.store.add_message(&id, &note)?;
                queue.push(note);
            }
            queue.extend(messages);
            tracing::info!(agent = %id, waiting = queue.len(), "resuming");
            let runtime = inner.agents.entry(id.clone()).or_default();
            runtime.queue = queue;
            runtime.running = true;
            resuming.push(id);
        }
        drop(inner);
        // Started with the computer, the network may not be up yet: they
        // carry on once it is, rather than fail their first call.
        let factory = self.clone();
        tokio::spawn(async move {
            crate::network::ready().await;
            for id in resuming {
                tokio::spawn(factory.clone().run(id));
            }
        });
        for (agent, task) in self.store.running_tasks()? {
            let factory = Arc::downgrade(self);
            let redaction = self.redaction.clone();
            tokio::spawn(async move {
                let done = task.wait_detached(&redaction).await;
                if let Some(factory) = factory.upgrade() {
                    factory.task_finished(&agent, done);
                }
            });
        }
        Ok(())
    }
}

/// What a resumed agent is told first: an event from Factory. `rebooted`:
/// its run began before the computer last started.
fn resume_note(stopped_on: &str, rebooted: bool) -> (String, factory_agent::Event) {
    let summary = if rebooted {
        "The computer restarted".to_owned()
    } else if stopped_on.is_empty() || stopped_on == VERSION {
        "Claktory restarted".to_owned()
    } else {
        format!("Claktory restarted: updated from {stopped_on} to {VERSION}")
    };
    let text = format!(
        "{summary} while you were working. Whatever you were doing when it stopped may not have finished (a tool call shows \"Interrupted\" if so), so check before relying on it, then carry on with your task."
    );
    let event = host_event(summary);
    (text, event)
}

/// What Factories before the store kept in runs.json.
#[derive(Deserialize)]
struct Runs {
    #[serde(default)]
    version: String,
    #[serde(default)]
    agents: BTreeMap<String, Vec<Queued>>,
}

/// Factories before the store kept which agents were working in runs.json
/// and the app's state in state.json. Moves them into the store, once.
pub(crate) async fn import_files(home: &Home, store: &Store) -> Result<(), OpenError> {
    if let Ok(bytes) = tokio::fs::read(home.runs_path()).await {
        match serde_json::from_slice::<Runs>(&bytes) {
            Ok(runs) => {
                for (agent, queue) in runs.agents {
                    // An open run: the agent was working, so it resumes.
                    store.start_run(&agent, &runs.version)?;
                    for queued in queue {
                        store.add_message(&agent, &queued)?;
                    }
                }
            }
            Err(error) => tracing::warn!(%error, "ignoring unreadable runs.json"),
        }
        tokio::fs::remove_file(home.runs_path()).await?;
    }
    if let Ok(bytes) = tokio::fs::read(home.state_path()).await {
        match serde_json::from_slice::<State>(&bytes) {
            Ok(state) => store.save_state(&state)?,
            Err(error) => tracing::warn!(%error, "ignoring unreadable state.json"),
        }
        tokio::fs::remove_file(home.state_path()).await?;
    }
    Ok(())
}
