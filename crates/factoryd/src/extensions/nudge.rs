//! The Checklist's nudge: an agent that stopped with work outstanding, and
//! that nothing running will wake, is reminded once, so outstanding work
//! doesn't sit until the person notices. And the doctrine on blockers:
//! an item that blocks on the person, owned by an agent with no
//! question waiting for them, reminds that agent daily to ask, so work
//! never sits parked behind a question nobody filed (one whose question
//! the person dismissed is asked again on its own terms instead, see
//! `checklist::check_waits`). And the periodic
//! check-in (the `nudge_every` option, off unless chosen): while an
//! agent has open goals or outstanding items, it's reminded that often once
//! idle, even while others work on its items, to see what can move.
//!
//! It's reminded when, for a short while (`Factory::set_nudge_after`), it has
//! been idle with open or in-progress items, no messages waiting, no
//! background task running, no monitor due to wake it soon (see
//! `monitors::will_wake`), and no other agent working that owns one of them. After a reminder, it's not reminded again until something changes:
//! one of its items, a message for it, or an owner of its items working.
//! While an owner works, it's still reminded, at most daily, of the items
//! that haven't changed for a day: an owner busy with other work shouldn't
//! leave them unwatched.

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

/// How long, in minutes, an item goes unchanged before it counts as stuck,
/// and how long between reminders of stuck items while their owners work.
const STUCK_MINUTES: i64 = 24 * 60;

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

/// The longest the checks sleep while nothing happens: they still look
/// then, for check-ins and waits that come due on the clock.
const QUIET: Duration = Duration::from_secs(30);

/// Checks every agent now and then, for as long as Factory runs: every
/// `nudge_after / 2` (at most 2 s) while something happens and for
/// `nudge_after` after (an agent that stopped is reminded then); with
/// nothing happening, at the next change, a parked item's time, or half a
/// minute at most.
pub(crate) async fn monitor(factory: Weak<Factory>) {
    let mut reminded: HashMap<String, Situation> = HashMap::new();
    let mut stuck: HashMap<String, i64> = HashMap::new();
    let mut checked_in: HashMap<String, i64> = HashMap::new();
    let Some((mut woken, mut activity)) = factory.upgrade().map(|factory| (factory.loops_woken(), factory.activity())) else { return };
    // Claktory starting counts as something happening.
    let mut happened = tokio::time::Instant::now();
    loop {
        let Some((after, minute, parked)) =
            factory.upgrade().map(|factory| (factory.nudge_after(), factory.nudge_minute(), next_parked(&factory)))
        else {
            return;
        };
        let often = (after / 2).clamp(Duration::from_millis(50), Duration::from_secs(2));
        if happened.elapsed() <= after + often {
            tokio::time::sleep(often).await;
            if activity.has_changed().unwrap_or(false) {
                activity.borrow_and_update();
                happened = tokio::time::Instant::now();
            }
        } else {
            let mut nap = (Duration::from_millis(u64::try_from(minute).unwrap_or(0)) / 2).clamp(often, QUIET);
            if let Some(at) = parked {
                nap = nap.min(Duration::from_millis(u64::try_from(at - now_ms()).unwrap_or(0)));
            }
            tokio::select! {
                () = tokio::time::sleep(nap) => {}
                changed = activity.changed() => {
                    if changed.is_err() {
                        return;
                    }
                    happened = tokio::time::Instant::now();
                }
                changed = woken.changed() => if changed.is_err() { return },
            }
        }
        let Some(factory) = factory.upgrade() else { return };
        if let Err(error) = check(&factory, after, &mut reminded, &mut stuck) {
            tracing::error!(%error, "could not check for agents to remind");
        }
        if let Err(error) = check_person_blocked(&factory) {
            tracing::error!(%error, "could not check for unasked blockers");
        }
        if let Err(error) = check_stale(&factory) {
            tracing::error!(%error, "could not check for stale work");
        }
        if let Err(error) = check_in(&factory, &mut checked_in) {
            tracing::error!(%error, "could not check in on agents' goals and items");
        }
        if let Err(error) = checklist::check_waits(&factory) {
            tracing::error!(%error, "could not check the checklist's waits");
        }
    }
}

/// When the next parked item opens again (`checklist::check_waits`), if any.
fn next_parked(factory: &Factory) -> Option<i64> {
    let db = factory.store.db();
    db.query_row("SELECT MIN(retry_at) FROM checklist_items WHERE status = 'parked'", [], |row| row.get::<_, Option<i64>>(0)).ok().flatten()
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
        let outstanding = Query {
            agent: &id,
            statuses: Some(vec![Status::Open, Status::InProgress, Status::Blocked]),
            oldest_untouched: true,
            ..Query::default()
        };
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
        ". None of it shows in their Needs you until you ask: file ask_person now with item naming it, one request per \
         decision or action (secrets_request or propose_to_rivet with item, if one waits on a secret or on Rivet) — or note \
         on the item why you can't ask yet.",
    );
    let summary = format!("Reminder: {} blocked item{} with nothing asked", items.len(), if items.len() == 1 { "" } else { "s" });
    tracing::info!(agent = %owning, items = items.len(), "reminding an agent of person-blocked items with no ask");
    factory.deliver_event(owning, text, host_event(summary));
    factory.store.db().execute("INSERT OR REPLACE INTO checklist_nudges (agent, last) VALUES (?1, ?2)", rusqlite::params![owning, now])?;
    Ok(())
}

/// Items the daily list of stale work names; more are counted.
const STALE_NAMED: usize = 15;

/// How long, in minutes, an agent goes idle with nothing open before its
/// lead's list of stale work names it as free.
const FREE_AFTER_MINUTES: i64 = 2 * 60;

/// An item as the stale-work list reads it.
#[derive(Clone)]
struct Stale {
    agent: String,
    id: u32,
    text: String,
    status: Status,
    owner: String,
    updated: i64,
}

/// The daily list of stale work: each agent with the Checklist on is told,
/// at most once a day, of its items that haven't changed for its
/// `stale_after` (parked ones with no resume time, three times as long),
/// oldest first, whether it's idle or working and whatever may wake it.
/// Items untouched three times as long also go to its lead, in the lead's
/// own daily list, with the agents it leads that sit idle with nothing open.
fn check_stale(factory: &Arc<Factory>) -> rusqlite::Result<()> {
    let minute = factory.nudge_minute();
    let now = now_ms();
    let day = (24 * 60_i64).saturating_mul(minute);
    let shortest = checklist::STALE_AFTER.iter().map(|(_, _, minutes)| *minutes).filter(|minutes| *minutes > 0).min().unwrap_or(0);
    let candidates: Vec<Stale> = {
        let db = factory.store.db();
        let mut statement = db.prepare(
            "SELECT agent, id, text, status, owner, updated FROM checklist_items
             WHERE (status IN ('open', 'in_progress') OR (status = 'parked' AND retry_at IS NULL)) AND updated <= ?1
             ORDER BY updated, agent, id",
        )?;
        let rows = statement.query_map([now - shortest.saturating_mul(minute)], |row| {
            Ok(Stale {
                agent: row.get(0)?,
                id: row.get(1)?,
                text: row.get(2)?,
                status: Status::parse(&row.get::<_, String>(3)?).unwrap_or(Status::Open),
                owner: row.get(4)?,
                updated: row.get(5)?,
            })
        })?;
        rows.collect::<Result<_, _>>()?
    };
    if candidates.is_empty() {
        return Ok(());
    }
    // Each item stale on its agent's list, and those also for its lead.
    let mut own: HashMap<String, Vec<Stale>> = HashMap::new();
    let mut team: HashMap<String, Vec<Stale>> = HashMap::new();
    {
        let inner = factory.lock();
        for item in candidates {
            let Some(config) = inner.config.agent(&item.agent) else { continue };
            if inner.config.archived(config) || !super::enabled(config, checklist::ID) {
                continue;
            }
            let chosen = super::choice(config, checklist::ID, "stale_after");
            let Some((_, _, after)) = checklist::STALE_AFTER.iter().find(|(value, _, minutes)| *value == chosen && *minutes > 0) else {
                continue;
            };
            let after = after.saturating_mul(minute);
            let long = after.saturating_mul(3);
            let age = now - item.updated;
            if age < if item.status == Status::Parked { long } else { after } {
                continue;
            }
            let lead = inner.config.lead(config).filter(|lead| !inner.config.archived(lead)).map(|lead| lead.id.clone());
            if let Some(lead) = lead.filter(|_| age >= long) {
                team.entry(lead).or_default().push(item.clone());
            }
            own.entry(item.agent.clone()).or_default().push(item);
        }
    }
    let mut recipients: Vec<String> = own.keys().chain(team.keys()).cloned().collect();
    recipients.sort();
    recipients.dedup();
    for agent in recipients {
        let last: Option<i64> = factory
            .store
            .db()
            .query_row("SELECT last FROM checklist_stale_nudges WHERE agent = ?1", [&agent], |row| row.get(0))
            .optional()?;
        if last.is_some_and(|last| now - last < day) {
            continue;
        }
        let mine = own.remove(&agent).unwrap_or_default();
        let theirs = team.remove(&agent).unwrap_or_default();
        let free = free_agents(factory, &agent, now, minute)?;
        let (text, summary) = stale_text(factory, &mine, &theirs, &free, now, minute);
        tracing::info!(agent = %agent, own = mine.len(), team = theirs.len(), "listing an agent's stale work");
        factory.deliver_event(&agent, text, host_event(summary));
        factory
            .store
            .db()
            .execute("INSERT OR REPLACE INTO checklist_stale_nudges (agent, last) VALUES (?1, ?2)", rusqlite::params![agent, now])?;
    }
    Ok(())
}

/// The agents `lead` leads, directly or through others, that have been
/// idle for `FREE_AFTER_MINUTES` with nothing open or in progress, by name.
fn free_agents(factory: &Factory, lead: &str, now: i64, minute: i64) -> rusqlite::Result<Vec<String>> {
    let led: Vec<(String, String)> = {
        let inner = factory.lock();
        let Some(lead) = inner.config.agent(lead) else { return Ok(Vec::new()) };
        inner
            .config
            .agents
            .iter()
            .filter(|agent| inner.config.leads(lead, agent) && !inner.config.archived(agent))
            .filter(|agent| inner.agents.get(&agent.id).is_none_or(|runtime| !runtime.running && !runtime.has_ready()))
            .map(|agent| (agent.id.clone(), agent.name.clone()))
            .collect()
    };
    if led.is_empty() {
        return Ok(Vec::new());
    }
    let runs = factory.store.run_times()?;
    let open = checklist::working_counts(&factory.store)?;
    let since = now - FREE_AFTER_MINUTES.saturating_mul(minute);
    Ok(led
        .into_iter()
        .filter(|(id, _)| runs.get(id).is_none_or(|(_, ended)| ended.is_some_and(|ended| ended <= since)))
        .filter(|(id, _)| !open.contains_key(id))
        .map(|(_, name)| name)
        .collect())
}

/// How long ago, from `minutes`: "1 day 2 h", "5 h" or "40 min".
fn age(minutes: i64) -> String {
    let (days, hours) = (minutes / (24 * 60), minutes % (24 * 60) / 60);
    match (days, hours) {
        (0, 0) => format!("{minutes} min"),
        (0, hours) => format!("{hours} h"),
        (days, 0) => format!("{days} day{}", if days == 1 { "" } else { "s" }),
        (days, hours) => format!("{days} day{} {hours} h", if days == 1 { "" } else { "s" }),
    }
}

/// The stale-work event: its text and summary.
fn stale_text(factory: &Factory, mine: &[Stale], theirs: &[Stale], free: &[String], now: i64, minute: i64) -> (String, String) {
    let line = |item: &Stale, whose: bool| {
        let owner = if item.owner.is_empty() { "no owner".to_owned() } else { format!("owner {}", checklist::name_of(factory, &item.owner)) };
        let on = if whose { format!("{}'s ", checklist::name_of(factory, &item.agent)) } else { String::new() };
        format!(
            "{on}#{} {} ({}, {}, {owner})",
            item.id,
            clip(&item.text, 60),
            item.status.name().replace('_', " "),
            age((now - item.updated) / minute.max(1)),
        )
    };
    let list = |items: &[Stale], whose: bool| {
        let mut list: Vec<String> = items.iter().take(STALE_NAMED).map(|item| line(item, whose)).collect();
        if items.len() > STALE_NAMED {
            list.push(format!("+{} more", items.len() - STALE_NAMED));
        }
        list.join("; ")
    };
    let plural = |count: usize| if count == 1 { "item" } else { "items" };
    let mut parts = Vec::new();
    if !mine.is_empty() {
        parts.push(format!(
            "Stale work: {} {} on your checklist {} changed in a while, oldest first: {}. For each, start it, hand it to someone, \
             give it a date (parked with retry_at, or blocked with waits_on), mark it done if it shipped, or drop it with a reason.",
            mine.len(),
            plural(mine.len()),
            if mine.len() == 1 { "hasn't" } else { "haven't" },
            list(mine, false),
        ));
    }
    if !theirs.is_empty() {
        parts.push(format!(
            "On the checklists of agents you lead, {} {} untouched for longer still: {}. Settle with each agent what happens next.",
            theirs.len(),
            plural(theirs.len()),
            list(theirs, true),
        ));
    }
    if !free.is_empty() {
        parts.push(format!("Free now, idle with nothing open: {}.", free.join(", ")));
    }
    let mut summary = Vec::new();
    if !mine.is_empty() {
        summary.push(format!("{} {}", mine.len(), plural(mine.len())));
    }
    if !theirs.is_empty() {
        summary.push(format!("{} on your team", theirs.len()));
    }
    (parts.join("\n\n"), format!("Stale: {}", summary.join(", ")))
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

/// The reminders of outstanding items: see the module's doc. `stuck` is when
/// each agent was last reminded of its stuck items.
fn check(
    factory: &Arc<Factory>,
    after: Duration,
    reminded: &mut HashMap<String, Situation>,
    stuck: &mut HashMap<String, i64>,
) -> rusqlite::Result<()> {
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
    let now = now_ms();
    let settled = now - i64::try_from(after.as_millis()).unwrap_or(i64::MAX);
    let day = STUCK_MINUTES.saturating_mul(factory.nudge_minute());
    for id in idle {
        // Settled: it stopped a while ago.
        if !runs.get(&id).and_then(|(_, ended)| *ended).is_some_and(|ended| ended <= settled) {
            continue;
        }
        let outstanding =
            Query { agent: &id, statuses: Some(vec![Status::Open, Status::InProgress]), oldest_untouched: true, ..Query::default() };
        let (items, total) = checklist::query(&factory.store, &outstanding)?;
        if items.is_empty() {
            continue;
        }
        let mut owners: Vec<String> =
            items.iter().map(|item| item.owner.clone()).filter(|owner| !owner.is_empty() && *owner != id).collect();
        owners.sort();
        owners.dedup();
        if running_tasks(factory, &id)? || monitored(factory, &id)? {
            continue;
        }
        if owners.iter().any(|owner| working.contains(owner)) {
            remind_stuck(factory, &id, &items, now, day, stuck);
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
            format!("You have {total} outstanding item{} and nothing is due to wake you soon: ", if total == 1 { "" } else { "s" });
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

/// While an owner of its items works: reminds `agent` of the items that
/// haven't changed for `day`, if it wasn't reminded of them within `day`.
fn remind_stuck(factory: &Arc<Factory>, agent: &str, items: &[checklist::Item], now: i64, day: i64, stuck: &mut HashMap<String, i64>) {
    if stuck.get(agent).is_some_and(|last| now - last < day) {
        return;
    }
    let still: Vec<&checklist::Item> = items.iter().filter(|item| now - item.updated >= day).collect();
    if still.is_empty() {
        return;
    }
    let count = still.len();
    let named: Vec<String> = still.iter().take(NAMED).map(|item| format!("#{} {}", item.id, clip(&item.text, 60))).collect();
    let mut text = format!(
        "{count} of your outstanding items {} changed for a day, though their owners are working: {}",
        if count == 1 { "hasn't" } else { "haven't" },
        named.join(", ")
    );
    if count > NAMED {
        let _ = write!(text, ", +{} more", count - NAMED);
    }
    text.push_str(". Check on them: ask their owner, carry on yourself, or mark what's waiting with status blocked and a note saying on what.");
    let summary = format!("Reminder: {count} checklist item{} unchanged for a day", if count == 1 { "" } else { "s" });
    tracing::info!(agent, count, "reminding an agent of items unchanged for a day");
    factory.deliver_event(agent, text, host_event(summary));
    stuck.insert(agent.to_owned(), now);
}

/// A monitor will wake it: an at, every or cron monitor due within the hour.
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
