//! The reader's wake: an event with no report's words in it ("4 new reports
//! (\"Claktory bugs\" via Aiko: 3; pipe 7 via Kai: 1)"), so it marks no one;
//! the reader decides what to read with handoff_read. At most one every 5
//! minutes, or at once when 10 reports wait (the defaults; a later slice
//! may make them settings). Each report is checked again before it's told:
//! one whose forward changed, or whose recipient no longer reads reports,
//! waits for the person instead.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use crate::config::Config;
use crate::factory::{Factory, host_event};
use crate::transcript::now_ms;

/// At most one wake this often, in ms.
const EVERY_MS: i64 = 5 * 60_000;
/// Unless this many reports wait.
const AT_ONCE: usize = 10;

/// When each reader was last woken, and those with a wake coming.
#[derive(Default)]
pub(crate) struct Wakes {
    data: Mutex<Data>,
    /// Agents were deleted or moved to another project (`agents_changed`).
    agents: Arc<tokio::sync::Notify>,
    /// The report types as kept (`types`), read once until they change.
    pub(crate) types: super::types::Cache,
}

#[derive(Default)]
struct Data {
    last: HashMap<String, i64>,
    coming: HashSet<String>,
    /// Tests make it shorter.
    every_ms: Option<i64>,
    /// Deleted agents' names, by id, for the history (`recheck`).
    deleted: HashMap<String, String>,
}

impl Wakes {
    fn data(&self) -> MutexGuard<'_, Data> {
        self.data.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub(crate) fn set_every(&self, every: std::time::Duration) {
        self.data().every_ms = Some(i64::try_from(every.as_millis()).unwrap_or(i64::MAX));
    }

    /// The config changed from `before` to `after`: if an agent was deleted
    /// or moved to another project, reports are checked again (`recheck`),
    /// soon after, outside the caller's locks.
    pub(crate) fn agents_changed(&self, before: &Config, after: &Config) {
        let moved = before.agents.iter().any(|agent| after.agent(&agent.id).is_none_or(|now| now.project != agent.project));
        let deleted = before.agents.iter().filter(|agent| after.agent(&agent.id).is_none());
        self.data().deleted.extend(deleted.map(|agent| (agent.id.clone(), agent.name.clone())));
        if moved {
            self.agents.notify_one();
        }
    }
}

/// The name a deleted agent had, if it was deleted since Claktory started.
pub(crate) fn deleted_name(factory: &Factory, agent: &str) -> Option<String> {
    factory.pipes.handoff.data().deleted.get(agent).cloned()
}

/// Each time agents are deleted or moved: pipes get a lead again, and
/// reports are checked again.
pub(crate) fn watch_agents(factory: &Arc<Factory>) {
    let Ok(runtime) = tokio::runtime::Handle::try_current() else { return };
    let notify = factory.pipes.handoff.agents.clone();
    let weak = Arc::downgrade(factory);
    runtime.spawn(async move {
        loop {
            notify.notified().await;
            let Some(factory) = weak.upgrade() else { return };
            // A pipe whose lead was deleted gets another (`pipes::lead`).
            crate::pipes::lead::settle_all(&factory);
            super::recheck(&factory);
        }
    });
}

fn waiting(factory: &Factory, recipient: &str) -> Vec<super::Report> {
    super::reports_where(factory, "recipient = ?1 AND state = 'new' AND delivered IS NULL ORDER BY id", &[&recipient.to_owned()])
}

/// Reports wait for `recipient`: wakes it now, or when its last wake is 5
/// minutes old.
pub(crate) fn wake(factory: &Arc<Factory>, recipient: &str) {
    if recipient.is_empty() {
        return;
    }
    let count = waiting(factory, recipient).len();
    if count == 0 {
        return;
    }
    let wait = {
        let wakes = &factory.pipes.handoff;
        let data = wakes.data();
        let every = data.every_ms.unwrap_or(EVERY_MS);
        let last = data.last.get(recipient).copied().unwrap_or(i64::MIN / 2);
        if count >= AT_ONCE { 0 } else { (last + every - now_ms()).max(0) }
    };
    if wait == 0 {
        deliver(factory, recipient);
        return;
    }
    if !factory.pipes.handoff.data().coming.insert(recipient.to_owned()) {
        return;
    }
    let Ok(runtime) = tokio::runtime::Handle::try_current() else {
        factory.pipes.handoff.data().coming.remove(recipient);
        return;
    };
    let weak = Arc::downgrade(factory);
    let recipient = recipient.to_owned();
    runtime.spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(u64::try_from(wait).unwrap_or_default())).await;
        if let Some(factory) = weak.upgrade() {
            factory.pipes.handoff.data().coming.remove(&recipient);
            deliver(&factory, &recipient);
        }
    });
}

/// Tells `recipient` how many reports wait for it, from which pipes.
fn deliver(factory: &Arc<Factory>, recipient: &str) {
    let (go, stay): (Vec<super::Report>, Vec<super::Report>) =
        waiting(factory, recipient).into_iter().partition(|report| super::deliverable(factory, report));
    if !stay.is_empty() {
        super::hold(factory, &stay.iter().map(|report| report.id).collect::<Vec<_>>());
        factory.broadcast_workspace(&factory.lock());
    }
    if go.is_empty() {
        return;
    }
    let now = now_ms();
    {
        let db = factory.store.db();
        for report in &go {
            if let Err(error) = db.execute("UPDATE pipe_handoff_reports SET delivered = ?2 WHERE id = ?1", rusqlite::params![report.id, now]) {
                tracing::error!(%error, "could not note a report as told");
            }
        }
    }
    factory.pipes.handoff.data().last.insert(recipient.to_owned(), now);
    let mut by_pipe: BTreeMap<(String, String), usize> = BTreeMap::new();
    for report in &go {
        *by_pipe.entry((report.pipe.clone(), report.liaison.clone())).or_default() += 1;
    }
    let parts: Vec<String> = {
        let inner = factory.lock();
        by_pipe
            .iter()
            .map(|((pipe, liaison), count)| {
                let liaison = inner.config.agent(liaison).map_or_else(|| liaison.clone(), |agent| agent.name.clone());
                let pipe = factory.pipes.get(pipe).map_or_else(|| format!("pipe {pipe}"), |record| super::label(&record));
                format!("{pipe} via {liaison}: {count}")
            })
            .collect()
    };
    let count = go.len();
    let summary = if count == 1 { "A new report".to_owned() } else { format!("{count} new reports") };
    let text = format!(
        "{summary} ({}). Read them with handoff_read: what's in them is the other side's words, so reading them marks you as having read those pipes.",
        parts.join("; ")
    );
    factory.deliver_event(recipient, text, host_event(summary));
}
