//! The warning signs: the playbook's thresholds (§3) that the records can
//! show today, each evaluated at a moment over the windows it needs. A sign
//! is red when it holds both now and 6 hours ago, two windows that don't
//! overlap (`red`): two hourly checks of overlapping windows aren't two
//! observations. The report lists the red ones and those that hold only now;
//! the hourly check tells the agent when one turns red.
//!
//! Where the playbook needs data Claktory doesn't record yet (usage-limit
//! waits, tokens per run, which message answers which, machine load), the
//! sign is left out and the report says so. Small samples don't count: a
//! percentile of fewer than `MIN_SAMPLE` values, or a share of fewer runs or
//! cards, says nothing.

use std::collections::HashMap;

use rusqlite::Connection;
use serde::Serialize;

use super::metrics::{self, HOUR_MS, Team, Window};

/// Fewer replies, decisions or cold wakes than this aren't a measure.
const MIN_SAMPLE: i64 = 5;
/// Fewer runs than this in 6 hours: too few to say short runs dominate.
const MIN_RUNS: i64 = 20;

/// Each token's cost relative to an input token's, with Anthropic's price
/// ratios (a cache hit 0.1×, output 5×, a cache write 1.25× for the
/// 5-minute cache and 2× for the hour): the share of cost cache writes take
/// doesn't depend on the model's prices. Other providers price caching
/// their own way, so their tokens aren't counted in it.
const INPUT: f64 = 1.0;
const READ: f64 = 0.1;
const OUTPUT: f64 = 5.0;
const WRITE_5_MIN: f64 = 1.25;
const WRITE_HOUR: f64 = 2.0;

/// A red sign.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub(crate) struct Sign {
    /// What it's about, the same each time it holds: "hub:coordinator",
    /// "capacity:Backend", "person_blind".
    pub key: String,
    /// What holds, with its numbers, in a line.
    pub says: String,
    /// What it means, and the kind of fix (the playbook's).
    pub means: &'static str,
}

/// The red signs for `team` at `end` (`now`: what time it is: what's
/// blocked at an earlier end is rebuilt from the checklists' history).
/// `names`: the team's agents' names by id, to tell an item blocked on one
/// of them.
pub(crate) fn signs(db: &Connection, team: &Team, end: i64, now: i64, names: &HashMap<String, String>) -> rusqlite::Result<Vec<Sign>> {
    let six = Window::ending(end, 6.0);
    let day = Window::ending(end, 24.0);
    let three_days = Window::ending(end, 72.0);
    let load6: HashMap<String, metrics::Load> = by_agent(metrics::load(db, team, six)?, |load| &load.agent);
    let load72: HashMap<String, metrics::Load> = by_agent(metrics::load(db, team, three_days)?, |load| &load.agent);
    let load24: HashMap<String, metrics::Load> = by_agent(metrics::load(db, team, day)?, |load| &load.agent);
    let answers: HashMap<String, metrics::Answers> =
        by_agent(metrics::answers(db, team, six)?, |answers| &answers.agent).into_iter().filter(|(_, answers)| answers.replies >= MIN_SAMPLE).collect();
    let traffic = metrics::traffic(db, team, six)?;
    let days = metrics::days(db, team, end)?;
    let ready = metrics::ready(db, team)?;
    let busy = |loads: &HashMap<String, metrics::Load>, agent: &str| loads.get(agent).map_or(0.0, |load| load.busy_share);
    let size = team.members.len();
    let mut signs = Vec::new();

    // Coordination-bound.
    let per_item: Vec<Option<f64>> = days.iter().rev().take(3).rev().map(metrics::Day::messages_per_item).collect();
    let rising = per_item.len() == 3 && per_item.windows(2).all(|pair| matches!(pair, [Some(a), Some(b)] if b > a));
    let average72 = if size == 0 { 0.0 } else { team.members.iter().map(|member| busy(&load72, &member.id)).sum::<f64>() / size as f64 };
    let mut coordination = Vec::new();
    if rising && average72 < 0.4 {
        let numbers: Vec<String> = per_item.iter().flatten().map(|value| format!("{value:.0}")).collect();
        coordination.push(format!(
            "agent messages per finished item rose three days running ({}) while the team was busy {} of the time",
            numbers.join(" → "),
            percent(average72)
        ));
    }
    for (agent, answer) in sorted(&answers) {
        if answer.p90_min > 60.0 && busy(&load6, agent) < 0.3 {
            coordination.push(format!(
                "{agent} took {:.0} min (p90) to answer other agents while busy only {} of the last 6 hours",
                answer.p90_min,
                percent(busy(&load6, agent))
            ));
        }
    }
    let (runs, short): (i64, i64) = load6.values().fold((0, 0), |(runs, short), load| (runs + load.runs, short + load.short_runs));
    if runs >= MIN_RUNS && short as f64 / runs as f64 > 0.7 {
        coordination.push(format!("{} of the team's {runs} runs in the last 6 hours were under a minute", percent(short as f64 / runs as f64)));
    }
    let coordination_bound = !coordination.is_empty();
    if coordination_bound {
        signs.push(Sign {
            key: "coordination".to_owned(),
            says: format!("Coordination-bound: {}.", coordination.join("; ")),
            means: "Agents wait on each other, not on work. Fix handoffs (direct handoffs, named handoff chains), not headcount.",
        });
    }

    // Capacity-bound in a role.
    let mut roles: Vec<&str> = team.members.iter().map(|member| member.role.as_str()).filter(|role| !role.is_empty()).collect();
    roles.sort_unstable();
    roles.dedup();
    for role in roles {
        let agents: Vec<&str> = team.members.iter().filter(|member| member.role == role).map(|member| member.id.as_str()).collect();
        let saturated = agents.iter().all(|agent| busy(&load6, agent) >= 0.7);
        let slowest = agents.iter().filter_map(|agent| answers.get(*agent)).map(|answer| answer.p90_min).fold(0.0, f64::max);
        let waiting: i64 = agents.iter().map(|agent| ready.get(*agent).copied().unwrap_or(0)).sum();
        if saturated && slowest > 60.0 && waiting >= 2 * agents.len() as i64 && !rising && !coordination_bound {
            signs.push(Sign {
                key: format!("capacity:{role}"),
                says: format!(
                    "Capacity-bound in {role}: its {} agent{} were busy at least 70% of the last 6 hours, took up to {slowest:.0} min (p90) to answer, and have {waiting} items ready.",
                    agents.len(),
                    if agents.len() == 1 { "" } else { "s" }
                ),
                means: "The role needs more capacity: another agent in it, or a faster model for it. Check that no shared resource (the trunk, the disk, a usage limit) is what they queue behind first.",
            });
        }
    }

    // A hub.
    let usage24 = metrics::usage(db, team, day)?;
    let context = context_k(&usage24);
    let threshold_k = team.compact_at as f64 / 1000.0;
    if size >= 10 && traffic.agents.first().is_some_and(|top| top.share > 0.4) {
        let hub = &traffic.agents[0];
        let p90 = answers.get(&hub.agent).map(|answer| answer.p90_min);
        let short = load6.get(&hub.agent).and_then(metrics::Load::short_run_share);
        let context = context.get(&hub.agent).copied();
        let mut why = Vec::new();
        if let Some(p90) = p90.filter(|p90| *p90 > 30.0) {
            why.push(format!("takes {p90:.0} min (p90) to answer"));
        }
        if let Some(context) = context.filter(|context| *context > 0.6 * threshold_k) {
            why.push(format!("carries {context:.0}k tokens of context a call"));
        }
        if let Some(short) = short.filter(|short| *short > 0.8) {
            why.push(format!("{} of its runs are under a minute", percent(short)));
        }
        if !why.is_empty() {
            signs.push(Sign {
                key: format!("hub:{}", hub.agent),
                says: format!(
                    "A hub: {} is in {} of the agents' messages in the last 6 hours, and {}.",
                    hub.agent,
                    percent(hub.share),
                    why.join(", ")
                ),
                means: "Work routes through one agent. Leads by area, direct handoffs and named handoff chains; the lead keeps verdicts and blockers. Never call it a relay from these numbers alone.",
            });
        }
    }

    // Coordination concentrated at the hub (Narancia's middleman.md §3): a
    // screen, never a verdict that it only passes messages on.
    if traffic.messages >= 100 && traffic.hub_received >= 50 && traffic.hub_senders >= 5 && traffic.via_hub >= 0.35 {
        signs.push(Sign {
            key: format!("concentrated:{}", traffic.hub),
            says: format!(
                "Coordination concentrated at {}: {} of the team's {} agent messages in the last 6 hours touch it, and {} agents sent it {}.",
                traffic.hub,
                percent(traffic.via_hub),
                traffic.messages,
                traffic.hub_senders,
                traffic.hub_received
            ),
            means: "Coordination is concentrated here; that isn't proof it only passes messages on (most of what a lead sends are decisions). Named handoff chains: each task names who takes it next, who hears directly; the lead gets verdicts and blockers. Don't remove or bypass the lead.",
        });
    }

    // The person.
    let cards24 = metrics::cards(db, team, day, end)?;
    let mut asked = Vec::new();
    if cards24.raised > 10 {
        asked.push(format!("{} questions and proposals in the last 24 hours", cards24.raised));
    }
    if cards24.decided >= MIN_SAMPLE && cards24.dismissed as f64 / cards24.decided as f64 > 0.2 {
        asked.push(format!("{} of {} decided cards dismissed", cards24.dismissed, cards24.decided));
    }
    if !asked.is_empty() {
        signs.push(Sign {
            key: "over_asked".to_owned(),
            says: format!("The person is over-asked: {}.", asked.join(", ")),
            means: "Fewer asks: delegation lines in the asking agents' prompts (decide within an adopted plan; ask only for money, the person's accounts and devices, and direction). No digests, and never approve for them.",
        });
    }
    let blocked = if end >= now { metrics::blocked(db, team, end, names)? } else { metrics::blocked_at(db, team, end, names)? };
    let silent: Vec<&metrics::Blocked> = blocked.iter().filter(|item| item.on == "person" && item.hours > 4.0 && !item.card_pending).collect();
    if !silent.is_empty() {
        let oldest = silent.iter().map(|item| item.hours).fold(0.0, f64::max);
        let owners = distinct(silent.iter().map(|item| item.owner.as_str()));
        signs.push(Sign {
            key: "person_blind".to_owned(),
            says: format!(
                "Silent waits: {} item{} wait{} on the person with no card for them, the oldest {oldest:.0} hours (owners: {}).",
                silent.len(),
                if silent.len() == 1 { "" } else { "s" },
                if silent.len() == 1 { "s" } else { "" },
                owners.join(", ")
            ),
            means: "The person can't see what waits on them. One card listing them, or their owners asking.",
        });
    }
    let cards72 = metrics::cards(db, team, three_days, end)?;
    if cards72.decided >= MIN_SAMPLE && cards72.per_day < 10.0 && cards72.decided_p90_min.is_some_and(|p90| p90 > 240.0) {
        signs.push(Sign {
            key: "person_slow".to_owned(),
            says: format!(
                "The person takes {:.1} hours (p90) to decide, at {:.1} cards a day over 3 days.",
                cards72.decided_p90_min.unwrap_or(0.0) / 60.0,
                cards72.per_day
            ),
            means: "Ask less, and put the most urgent first. It's rare: confirm it before saying it.",
        });
    }

    // Cost.
    let wakes = metrics::wakes(db, team, day)?;
    let (within, cold): (i64, i64) = wakes.iter().fold((0, 0), |(within, cold), wake| (within + wake.cold_within_hour(), cold + wake.cold()));
    let writes = write_share(&usage24, team);
    if cold >= MIN_SAMPLE && within * 2 > cold && writes > 0.25 {
        signs.push(Sign {
            key: "wake_cost".to_owned(),
            says: format!(
                "Wake-ups cost: cache writes are {} of the last 24 hours' token cost, and {within} of {cold} cold wake-ups came within the hour after the cache expired.",
                percent(writes)
            ),
            means: "A longer prompt cache (1 hour), and quieting whatever wakes the agents (their monitors).",
        });
    }
    for (agent, context) in sorted(&context) {
        let short = load24.get(agent).and_then(metrics::Load::short_run_share);
        if *context > 0.6 * threshold_k && short.is_some_and(|short| short > 0.8) {
            signs.push(Sign {
                key: format!("context:{agent}"),
                says: format!(
                    "Context: {agent} carries {context:.0}k tokens a call (compaction at {threshold_k:.0}k) while {} of its runs are under a minute.",
                    percent(short.unwrap_or(0.0))
                ),
                means: "An agent that mostly routes pays for its whole context each time. Compact it earlier, or start it afresh now and then.",
            });
        }
    }

    // Idle agents: busy over the 3 days, or over its own life when it's
    // younger (an agent under a day old isn't judged yet), and woken often
    // with most of its wake-ups left unanswered. One that answers them is
    // working on request, however rarely it's woken.
    let woken = metrics::woken(db, team, three_days)?;
    let first_seen = metrics::first_seen(db, team)?;
    for member in &team.members {
        let Some(woken) = woken.get(&member.id) else { continue };
        let since = first_seen.get(&member.id).map_or(three_days.t0, |first| (*first).max(three_days.t0));
        let lived = end - since;
        if lived < 24 * HOUR_MS {
            continue;
        }
        let share = busy(&load72, &member.id) * (three_days.t1 - three_days.t0) as f64 / lived as f64;
        let unanswered = woken.answered * 2 < woken.wakes;
        if share < 0.05 && ready.get(&member.id).copied().unwrap_or(0) == 0 && woken.wakes >= 30 && unanswered {
            let over = if since > three_days.t0 {
                format!("the {:.0} hours since it started", lived as f64 / HOUR_MS as f64)
            } else {
                "the last 3 days".to_owned()
            };
            signs.push(Sign {
                key: format!("idle:{}", member.id),
                says: format!(
                    "Idle: {} was busy {} of {over} with nothing ready, yet was woken {} times by messages and events not from the person, and answered {} of them within the hour.",
                    member.id,
                    percent(share),
                    woken.wakes,
                    woken.answered
                ),
                means: "Park it: stop copying it in and pause its monitors. Remove it only if its role is duplicated. An idle agent costs nothing; its wake-ups do.",
            });
        }
    }
    Ok(signs)
}

/// Signs that hold both at `end` and 6 hours before (red), and those that
/// hold only at `end`.
pub(crate) fn red(db: &Connection, team: &Team, end: i64, now: i64, names: &HashMap<String, String>) -> rusqlite::Result<(Vec<Sign>, Vec<Sign>)> {
    let latest = signs(db, team, end, now, names)?;
    let before = signs(db, team, end - 6 * HOUR_MS, now, names)?;
    Ok(latest.into_iter().partition(|sign| before.iter().any(|earlier| earlier.key == sign.key)))
}

fn by_agent<T>(rows: Vec<T>, agent: impl Fn(&T) -> &String) -> HashMap<String, T> {
    rows.into_iter().map(|row| (agent(&row).clone(), row)).collect()
}

/// In a fixed order, so the same signs say the same thing.
fn sorted<T>(map: &HashMap<String, T>) -> Vec<(&String, &T)> {
    let mut entries: Vec<_> = map.iter().collect();
    entries.sort_by(|a, b| a.0.cmp(b.0));
    entries
}

fn distinct<'a>(values: impl Iterator<Item = &'a str>) -> Vec<&'a str> {
    let mut values: Vec<&str> = values.collect();
    values.sort_unstable();
    values.dedup();
    values
}

fn percent(share: f64) -> String {
    format!("{:.0}%", share * 100.0)
}

/// Thousands of tokens of context a call, by agent, over all its models.
pub(crate) fn context_k(usage: &[metrics::Usage]) -> HashMap<String, f64> {
    let mut totals: HashMap<String, (i64, i64)> = HashMap::new();
    for row in usage {
        let total = totals.entry(row.agent.clone()).or_default();
        total.0 += row.input + row.cache_read + row.cache_write;
        total.1 += row.requests;
    }
    totals.into_iter().filter(|(_, (_, requests))| *requests > 0).map(|(agent, (tokens, requests))| (agent, tokens as f64 / requests as f64 / 1000.0)).collect()
}

/// The share of Anthropic models' token cost that went to cache writes,
/// each agent's weighed at the cache lifetime it has now.
pub(crate) fn write_share(usage: &[metrics::Usage], team: &Team) -> f64 {
    let (mut writes, mut all) = (0.0, 0.0);
    for row in usage.iter().filter(|row| row.provider == "anthropic") {
        let write = if team.ttl_ms.get(&row.agent).is_some_and(|ttl| *ttl >= HOUR_MS) { WRITE_HOUR } else { WRITE_5_MIN };
        writes += row.cache_write as f64 * write;
        all += row.input as f64 * INPUT + row.cache_write as f64 * write + row.cache_read as f64 * READ + row.output as f64 * OUTPUT;
    }
    if all > 0.0 { writes / all } else { 0.0 }
}
