//! The Checklist's nudge: an agent that stopped with work outstanding, and
//! that nothing running will wake, is reminded once, so outstanding work
//! doesn't sit until the person notices.
//!
//! It's reminded when, for a short while (`Factory::set_nudge_after`), it has
//! been idle with open or in-progress items, no messages waiting, no
//! background task running, no monitor that will wake it (see
//! `monitors::will_wake`), and no other agent working that owns one of them. After a reminder, it's not reminded again until something changes:
//! one of its items, a message for it, or an owner of its items working.

use std::collections::HashMap;
use std::fmt::Write as _;
use std::sync::{Arc, Weak};
use std::time::Duration;

use factory_agent::{Event, EventSource};
use rusqlite::OptionalExtension;

use super::checklist::{self, Query, Status};
use crate::factory::Factory;
use crate::transcript::now_ms;

/// Items a reminder names; more are counted.
const NAMED: usize = 8;

/// What a reminder was about: when it's the same, the agent isn't reminded again.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Situation {
    /// Its latest checklist event.
    item_events: i64,
    /// The latest message for it.
    message: i64,
    /// When each other owner of its items last finished working.
    owners: Vec<(String, Option<i64>)>,
}

/// Checks every agent now and then, for as long as Factory runs.
pub(crate) async fn monitor(factory: Weak<Factory>) {
    let mut reminded: HashMap<String, Situation> = HashMap::new();
    loop {
        let Some(after) = factory.upgrade().map(|factory| factory.nudge_after()) else { return };
        tokio::time::sleep((after / 2).clamp(Duration::from_millis(50), Duration::from_secs(2))).await;
        let Some(factory) = factory.upgrade() else { return };
        if let Err(error) = check(&factory, after, &mut reminded) {
            tracing::error!(%error, "could not check for agents to remind");
        }
    }
}

fn check(factory: &Arc<Factory>, after: Duration, reminded: &mut HashMap<String, Situation>) -> rusqlite::Result<()> {
    // Idle agents with the nudge on, and whose agents are working now.
    let (idle, working) = {
        let inner = factory.lock();
        let mut idle = Vec::new();
        let mut working = Vec::new();
        for agent in &inner.config.agents {
            let runtime = inner.agents.get(&agent.id);
            if runtime.is_some_and(|runtime| runtime.running) {
                working.push(agent.id.clone());
            } else if super::enabled(agent, checklist::ID)
                && super::switch(agent, checklist::ID, "nudge")
                && !inner.config.archived(agent)
                && runtime.is_none_or(|runtime| runtime.queue.is_empty())
            {
                idle.push(agent.id.clone());
            }
        }
        (idle, working)
    };
    reminded.retain(|id, _| idle.contains(id) || working.contains(id));
    if idle.is_empty() {
        return Ok(());
    }
    let runs = factory.store.run_times()?;
    let settled = now_ms() - i64::try_from(after.as_millis()).unwrap_or(i64::MAX);
    for id in idle {
        // Settled: it stopped a while ago.
        if !runs.get(&id).and_then(|(_, ended)| *ended).is_some_and(|ended| ended <= settled) {
            continue;
        }
        let outstanding = Query { agent: &id, statuses: Some(vec![Status::Open, Status::InProgress]), ..Query::default() };
        let (items, total) = checklist::query(&factory.store, &outstanding)?;
        if items.is_empty() {
            continue;
        }
        let mut owners: Vec<String> =
            items.iter().map(|item| item.owner.clone()).filter(|owner| !owner.is_empty() && *owner != id).collect();
        owners.sort();
        owners.dedup();
        if owners.iter().any(|owner| working.contains(owner)) || running_tasks(factory, &id)? || monitored(factory, &id)? {
            continue;
        }
        let situation = |factory: &Factory| -> rusqlite::Result<Situation> {
            let db = factory.store.db();
            let latest = |sql: &str| {
                db.query_row(sql, [&id], |row| row.get::<_, Option<i64>>(0)).optional().map(|value| value.flatten().unwrap_or(0))
            };
            Ok(Situation {
                item_events: latest("SELECT MAX(seq) FROM checklist_events WHERE agent = ?1")?,
                message: latest("SELECT MAX(id) FROM messages WHERE agent = ?1")?,
                owners: owners.iter().map(|owner| (owner.clone(), runs.get(owner).and_then(|(_, ended)| *ended))).collect(),
            })
        };
        if reminded.get(&id) == Some(&situation(factory)?) {
            continue;
        }
        let mut text =
            format!("You have {total} outstanding item{} and nothing running will wake you: ", if total == 1 { "" } else { "s" });
        let named: Vec<String> = items.iter().take(NAMED).map(|item| format!("#{} {}", item.id, clip(&item.text, 60))).collect();
        text.push_str(&named.join(", "));
        if total > NAMED {
            let _ = write!(text, ", +{} more", total - NAMED);
        }
        text.push_str(". Carry on, or mark what's waiting with status blocked and a note saying on what.");
        let summary = format!("Reminder: {total} outstanding checklist item{}", if total == 1 { "" } else { "s" });
        let event = Event { source: EventSource::Host, from: "Claktory".to_owned(), id: String::new(), summary, transient: false };
        tracing::info!(agent = %id, total, "reminding an idle agent of its outstanding items");
        factory.deliver_event(&id, text, event);
        reminded.insert(id.clone(), situation(factory)?);
    }
    Ok(())
}

/// A monitor will wake it: one that watches for something, or one due within the hour.
fn monitored(factory: &Factory, agent: &str) -> rusqlite::Result<bool> {
    let Some(config) = factory.lock().config.agent(agent).cloned() else { return Ok(false) };
    super::monitors::will_wake(factory, &config, factory.monitors.now())
}

fn running_tasks(factory: &Factory, agent: &str) -> rusqlite::Result<bool> {
    let running: i64 =
        factory.store.db().query_row("SELECT COUNT(*) FROM tasks WHERE agent = ?1 AND ended IS NULL", [agent], |row| row.get(0))?;
    Ok(running > 0)
}

fn clip(text: &str, chars: usize) -> String {
    match text.char_indices().nth(chars) {
        Some((end, _)) => format!("{}…", &text[..end]),
        None => text.to_owned(),
    }
}
