//! The Checklist's nudge: an agent that stopped with work outstanding, and
//! that nothing running will wake, is reminded once, so outstanding work
//! doesn't sit until the person notices. And the doctrine on blockers
//! (#255): an item that blocks on the person, owned by an agent with no
//! question waiting for them, reminds that agent daily to ask, so work
//! never sits parked behind a question nobody filed (one whose question
//! the person dismissed is asked again on its own terms instead, see
//! `checklist::check_waits`). And the periodic
//! check-in (#270, the `nudge_every` option, off unless chosen): while an
//! agent has open goals or outstanding items, it's reminded that often once
//! idle, even while others work on its items, to see what can move.
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

use rusqlite::OptionalExtension;

use super::checklist::{self, Query, Status};
use crate::factory::{Factory, host_event};
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
    let mut checked_in: HashMap<String, i64> = HashMap::new();
    loop {
        let Some(after) = factory.upgrade().map(|factory| factory.nudge_after()) else { return };
        tokio::time::sleep((after / 2).clamp(Duration::from_millis(50), Duration::from_secs(2))).await;
        let Some(factory) = factory.upgrade() else { return };
        if let Err(error) = check(&factory, after, &mut reminded) {
            tracing::error!(%error, "could not check for agents to remind");
        }
        if let Err(error) = check_person_blocked(&factory) {
            tracing::error!(%error, "could not check for unasked blockers");
        }
        if let Err(error) = check_in(&factory, &mut checked_in) {
            tracing::error!(%error, "could not check in on agents' goals and items");
        }
        if let Err(error) = checklist::check_waits(&factory) {
            tracing::error!(%error, "could not check the checklist's waits");
        }
    }
}

/// The periodic check-in: each idle agent whose `nudge_every` is set, with
/// open goals or outstanding items, is reminded once that long has passed
/// since it last worked and since its last check-in (counted from when
/// Claktory started, the first time).
fn check_in(factory: &Arc<Factory>, checked_in: &mut HashMap<String, i64>) -> rusqlite::Result<()> {
    let minute = factory.nudge_minute();
    let now = now_ms();
    let due: Vec<(String, i64, &'static str)> = {
        let inner = factory.lock();
        let mut due = Vec::new();
        for agent in &inner.config.agents {
            if !super::enabled(agent, checklist::ID) || inner.config.archived(agent) {
                continue;
            }
            let chosen = super::choice(agent, checklist::ID, "nudge_every");
            let Some((_, label, minutes)) = checklist::EVERY.iter().find(|(value, _, minutes)| *value == chosen && *minutes > 0) else {
                checked_in.remove(&agent.id);
                continue;
            };
            // From when it's first seen with the option on.
            checked_in.entry(agent.id.clone()).or_insert(now);
            let runtime = inner.agents.get(&agent.id);
            if runtime.is_some_and(|runtime| runtime.running || runtime.has_ready()) {
                continue;
            }
            due.push((agent.id.clone(), minutes.saturating_mul(minute), *label));
        }
        due
    };
    if due.is_empty() {
        return Ok(());
    }
    let runs = factory.store.run_times()?;
    let person = factory.lock().config.person().to_owned();
    for (id, every, label) in due {
        let ended = runs.get(&id).and_then(|(_, ended)| *ended).unwrap_or(0);
        let since = checked_in.get(&id).copied().unwrap_or(now).max(ended);
        if now - since < every {
            continue;
        }
        let open_goals: Vec<checklist::Goal> =
            checklist::goals(&factory.store, &id, false)?.into_iter().filter(|goal| goal.status == Status::Open).collect();
        let outstanding = Query { agent: &id, statuses: Some(vec![Status::Open, Status::InProgress, Status::Blocked]), ..Query::default() };
        let (items, total) = checklist::query(&factory.store, &outstanding)?;
        if open_goals.is_empty() && total == 0 {
            continue;
        }
        let plural = |count: usize, one: &str| format!("{count} {one}{}", if count == 1 { "" } else { "s" });
        let mut parts = Vec::new();
        if !open_goals.is_empty() {
            let progress = checklist::progress(&factory.store, &id)?;
            let named: Vec<String> = open_goals
                .iter()
                .take(NAMED)
                .map(|goal| match progress.get(&goal.id).filter(|(_, all)| *all > 0) {
                    Some((done, all)) => format!("G{} {} ({done}/{all} done)", goal.id, clip(&goal.text, 60)),
                    None => format!("G{} {} (no items yet)", goal.id, clip(&goal.text, 60)),
                })
                .collect();
            let more = if open_goals.len() > NAMED { format!(", +{} more", open_goals.len() - NAMED) } else { String::new() };
            parts.push(format!("{}: {}{more}", plural(open_goals.len(), "open goal"), named.join(", ")));
        }
        if total > 0 {
            let named: Vec<String> = items
                .iter()
                .take(NAMED)
                .map(|item| {
                    let state = match item.status {
                        Status::InProgress => " (in progress)",
                        Status::Blocked => " (blocked)",
                        _ => "",
                    };
                    format!("#{} {}{state}", item.id, clip(&item.text, 60))
                })
                .collect();
            let more = if total > NAMED { format!(", +{} more", total - NAMED) } else { String::new() };
            parts.push(format!("{}: {}{more}", plural(total, "outstanding item"), named.join(", ")));
        }
        let text = format!(
            "Check-in ({}): {}. See whether anything can be driven further: a step you can take now, an item to hand to someone, \
             or a question for {person} (ask_person). If nothing can move yet, stop without a reply.",
            label.to_lowercase(),
            parts.join("; "),
        );
        let mut summary = vec![];
        if !open_goals.is_empty() {
            summary.push(plural(open_goals.len(), "goal"));
        }
        if total > 0 {
            summary.push(plural(total, "item"));
        }
        tracing::info!(agent = %id, goals = open_goals.len(), items = total, "checking in on an agent's goals and items");
        factory.deliver_event(&id, text, host_event(format!("Check-in: {}", summary.join(", "))));
        checked_in.insert(id, now);
    }
    Ok(())
}

/// How long between reminders of the same agent's unasked blockers.
const ASK_EVERY_MS: i64 = 24 * 60 * 60 * 1000;

/// A person-blocked item as read: the checklist it's on, its owner, id
/// and text.
type Blocked = (String, String, u32, String);

/// How long a decision waits on the person before its owner is reminded
/// to ask, in minutes: time to file the card in the same step.
const ASK_WITHIN_MINUTES: i64 = 60;

/// The daily reminder: items with `blocked_on` "person", blocked an hour,
/// whose owning agent (the owner, or the checklist's agent) has no pending
/// ask about them get that agent told to file one, at most once a day: a
/// decision or something only the person can do alike, since neither shows
/// in Needs you until a question names it. Not once they dismissed a
/// question about it (it's asked again on its own terms, see
/// `checklist::check_waits`).
fn check_person_blocked(factory: &Arc<Factory>) -> rusqlite::Result<()> {
    sweep_once(factory)?;
    let rows: Vec<Blocked> = {
        let db = factory.store.db();
        let mut statement = db.prepare(
            "SELECT agent, owner, id, text FROM checklist_items WHERE status = 'blocked' AND blocked_on = 'person'
             AND dismissed_at IS NULL AND blocked_since <= ?1 ORDER BY agent, id",
        )?;
        let since = now_ms() - ASK_WITHIN_MINUTES.saturating_mul(factory.nudge_minute());
        let rows = statement.query_map([since], crate::store::row4)?;
        rows.collect::<Result<_, _>>()?
    };
    if rows.is_empty() {
        return Ok(());
    }
    let person = factory.lock().config.person().to_owned();
    for (owning, items) in by_owner(rows) {
        remind_owner(factory, &owning, &items, &person, "")?;
    }
    Ok(())
}

/// The items by their owning agent: the owner when one is set, else the
/// agent whose checklist they're on.
fn by_owner(rows: Vec<Blocked>) -> HashMap<String, Vec<Blocked>> {
    let mut groups: HashMap<String, Vec<Blocked>> = HashMap::new();
    for row in rows {
        let owning = if row.1.is_empty() { row.0.clone() } else { row.1.clone() };
        groups.entry(owning).or_default().push(row);
    }
    groups
}

/// Tells `owning` its person-blocked items have no question filed, if
/// that holds and a day has passed since it was last told. `lead` opens
/// the text (the sweep's explanation), empty for the daily reminder.
fn remind_owner(factory: &Arc<Factory>, owning: &str, items: &[Blocked], person: &str, lead: &str) -> rusqlite::Result<()> {
    {
        let inner = factory.lock();
        let Some(config) = inner.config.agent(owning) else { return Ok(()) };
        if inner.config.archived(config) || (super::enabled(config, checklist::ID) && !super::switch(config, checklist::ID, "nudge")) {
            return Ok(());
        }
    }
    let items: Vec<&Blocked> = items
        .iter()
        .filter(|(agent, _, id, _)| !checklist::asked_about(factory, owning, &checklist::item_key(agent, *id)))
        .collect();
    if items.is_empty() {
        return Ok(());
    }
    let now = now_ms();
    let last: Option<i64> =
        factory.store.db().query_row("SELECT last FROM checklist_nudges WHERE agent = ?1", [owning], |row| row.get(0)).optional()?;
    if last.is_some_and(|last| now - last < ASK_EVERY_MS) {
        return Ok(());
    }
    let named: Vec<String> = items
        .iter()
        .take(NAMED)
        .map(|(agent, _, id, text)| {
            let whose = if agent == owning { String::new() } else { format!(" on {}'s checklist", checklist::name_of(factory, agent)) };
            format!("#{id}{whose} {}", clip(text, 60))
        })
        .collect();
    let mut text = format!(
        "{lead}{count} checklist item{s} block{third} on {person} and you have no question waiting for them: {list}",
        count = items.len(),
        s = if items.len() == 1 { "" } else { "s" },
        third = if items.len() == 1 { "s" } else { "" },
        list = named.join(", "),
    );
    if items.len() > NAMED {
        let _ = write!(text, ", +{} more", items.len() - NAMED);
    }
    text.push_str(
        ". None of it shows in their Needs you until you ask: file ask_person now with item naming it, one card per \
         decision or action (secrets_request or propose_to_rivet with item, if one waits on a secret or on Rivet) — or note \
         on the item why you can't ask yet.",
    );
    let summary = format!("Reminder: {} blocked item{} with nothing asked", items.len(), if items.len() == 1 { "" } else { "s" });
    tracing::info!(agent = %owning, items = items.len(), "reminding an agent of person-blocked items with no ask");
    factory.deliver_event(owning, text, host_event(summary));
    factory.store.db().execute("INSERT OR REPLACE INTO checklist_nudges (agent, last) VALUES (?1, ?2)", rusqlite::params![owning, now])?;
    Ok(())
}

/// Once, after the update that brought `blocked_on`: blocked items from
/// before it whose notes read as waiting on the person get their owners
/// told, so old parked blockers surface too.
fn sweep_once(factory: &Arc<Factory>) -> rusqlite::Result<()> {
    let done: bool =
        factory.store.db().query_row("SELECT EXISTS (SELECT 1 FROM checklist_nudges WHERE agent = '')", [], |row| row.get(0))?;
    if done {
        return Ok(());
    }
    let agents: Vec<String> = {
        let db = factory.store.db();
        let mut statement = db.prepare("SELECT DISTINCT agent FROM checklist_items WHERE status = 'blocked' AND blocked_on = ''")?;
        let rows = statement.query_map([], |row| row.get(0))?;
        rows.collect::<Result<_, _>>()?
    };
    let person = factory.lock().config.person().to_owned();
    let mut rows: Vec<Blocked> = Vec::new();
    for agent in agents {
        let blocked = Query { agent: &agent, statuses: Some(vec![Status::Blocked]), with_log: true, ..Query::default() };
        let (items, _) = checklist::query(&factory.store, &blocked)?;
        for item in items {
            if item.blocked_on.is_empty() && looks_person_blocked(&item.blocked_note, &person) {
                rows.push((agent.clone(), item.owner.clone(), item.id, item.text.clone()));
            }
        }
    }
    for (owning, items) in by_owner(rows) {
        let lead = "Some blocked items on the books look like they wait on the person. If so, set blocked_on: \"person\" on each with checklist_update. ";
        remind_owner(factory, &owning, &items, &person, lead)?;
    }
    factory.store.db().execute("INSERT OR REPLACE INTO checklist_nudges (agent, last) VALUES ('', ?1)", [now_ms()])?;
    Ok(())
}

/// Whether a blocked item's note reads as waiting on the person.
fn looks_person_blocked(note: &str, person: &str) -> bool {
    let note = note.to_ascii_lowercase();
    let person = person.to_ascii_lowercase();
    let marks = ["person", "approv", "decision", "decide", "confirm", "permission", "answer", "waiting for", "ask"];
    !note.is_empty() && (note.contains(&person) || marks.iter().any(|mark| note.contains(mark)))
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
                && runtime.is_none_or(|runtime| !runtime.has_ready())
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
        let event = host_event(summary);
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
