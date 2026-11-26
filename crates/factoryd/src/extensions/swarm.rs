//! Team health (`swarm`): how the agents work together, from Claktory's
//! records. `swarm_report` runs the playbook's measures (Melone's
//! optimize-swarm-playbook.md, shipped as the Claktory manual's note
//! "swarm-playbook") over a window and says which warning signs are red; the
//! agent reads the playbook and proposes changes the person approves on one
//! card. It reads counts, times, states and token totals only, never what
//! anyone wrote, and changes nothing.
//!
//! Its option "watch" (on unless turned off) checks every hour, with no
//! model: when a sign turns red (it holds in the last 6 hours and in the 6
//! before them), the agent is told once in an event, and not again about
//! that sign until it clears. Admin agents (Rivet) have it unless it's
//! turned off; they look at everyone, other agents at their own project.

mod metrics;
mod signs;
#[cfg(test)]
mod tests;

use std::collections::{BTreeSet, HashMap};
use std::fmt::Write as _;
use std::sync::{Arc, Weak};
use std::time::Duration;

use factory_agent::{Tool, ToolResult};
use serde::Deserialize;
use serde_json::{Value, json};

use super::{Extension, Manifest, OptionKind, OptionSpec, PromptSection, agents, enabled, switch};
use crate::config::{AgentConfig, Config};
use crate::factory::{Factory, Inner, host_event};
use crate::transcript::now_ms;
use metrics::{HOUR_MS, Member, Team, Window};
use signs::Sign;

pub(crate) const ID: &str = "swarm";

static MANIFEST: Manifest = Manifest {
    id: ID,
    name: "Team health",
    icon: "chart-column",
    description: "Lets it check how your agents work together: who's busy, who waits on whom, what waits on you and what costs the most, and suggest changes for you to approve. It looks only at counts and times, never at what agents wrote.",
    default_enabled: false,
    options: &[OptionSpec {
        id: "watch",
        label: "Speak up when something's wrong",
        description: "Once an hour, Claktory checks for warning signs, such as work waiting on you that no one asked about, and tells it when one has lasted 12 hours. Checking doesn't use the model.",
        kind: OptionKind::Switch,
        default_on: true,
        includes_project: false,
    }],
};

const PROMPT: &str = "## Team health

swarm_report measures how agents work together, from Claktory's records: how busy each agent is, how long agents take to answer each other, who's in the middle of the traffic, finished and blocked items, the person's cards, wake-ups and tokens, and which warning signs are red. Use it when the person asks why things are slow, or to improve how the team works, and when Claktory tells you a sign turned red. Before you propose anything, follow the playbook (the Claktory manual's note \"swarm-playbook\", with memory_read, if you have the manual):
- Look at two windows, 6 hours and 72 hours; a sign counts only when it holds in the last 6 hours and in the 6 before them.
- Find which constraint binds before proposing more agents: more agents make coordination problems worse.
- Propose at most five changes, each tied to a sign, in one ask_person card, and apply nothing until the person approves; never delete an agent or give a secret that way. Measure again a day later.
- It reads counts and times only. Don't read what agents wrote unless the person asks about a specific exchange, and never a pipe's messages.
";

const ABOUT: &str = "How the agents work together, from Claktory's records, over a window: each agent's busy share and runs (Q1), how long agents take to answer each other (Q2), the traffic and its hub (Q3), the last 7 days (Q4), finished items and cycle times (Q5), what's blocked and on what (Q6), the person's cards (Q7), wake-ups against the prompt cache (Q8), tokens and context size (Q9), the monitors that wake agents most (Q10), and the warning signs that are red. Counts, times and states only. window: hours (6 unless given; 72 for the trend); offset: hours before now the window ends (0 unless given; 6 for the 6 hours before); project: a project's id or name, for its agents and the admin agents (everyone unless given).";

pub(crate) struct Swarm;

/// The table that keeps which signs each agent was told about, so it's told
/// once while a sign stays red, across restarts.
pub(crate) const WATCH: &str = "
-- Warning signs (Team health) each agent was told about while they're red.
CREATE TABLE swarm_watch (
    agent TEXT NOT NULL,
    -- The sign's key, e.g. 'person_blind' or 'hub:coordinator'.
    sign TEXT NOT NULL,
    -- When the agent was told, in ms since 1970.
    told INTEGER NOT NULL,
    PRIMARY KEY (agent, sign)
);
";

/// How often the watch checks.
const EVERY: Duration = Duration::from_secs(3600);
/// The first check after Claktory starts.
const FIRST: Duration = Duration::from_secs(300);

/// The agents `project` (an id or a name) is about, with the admin agents;
/// everyone when it's None. Archived agents aren't on it.
fn team(config: &Config, project: Option<&str>) -> Result<(Team, HashMap<String, String>), String> {
    let project = match project.map(str::trim).filter(|project| !project.is_empty()) {
        None => None,
        Some(wanted) => {
            let found = config.projects.iter().find(|project| project.id == wanted || project.name.eq_ignore_ascii_case(wanted));
            let Some(found) = found else {
                let names: Vec<&str> = config.projects.iter().map(|project| project.name.as_str()).collect();
                return Err(format!("There's no project \"{wanted}\". Projects: {}.", if names.is_empty() { "none".to_owned() } else { names.join(", ") }));
            };
            Some(found)
        }
    };
    let agents: Vec<&AgentConfig> = config
        .agents
        .iter()
        .filter(|agent| !config.archived(agent))
        .filter(|agent| project.is_none_or(|project| agent.project.as_deref() == Some(project.id.as_str()) || switch(agent, agents::ID, "admin")))
        .collect();
    let members = agents
        .iter()
        .map(|agent| Member {
            id: agent.id.clone(),
            role: agent.role.clone(),
            leads: config.agents.iter().any(|other| other.lead.as_deref() == Some(agent.id.as_str())),
        })
        .collect();
    let ttl_ms = agents
        .iter()
        .filter_map(|agent| {
            let ttl = config.cache_ttl(&agent.model)?;
            Some((agent.id.clone(), if ttl == factory_agent::CacheTtl::OneHour { HOUR_MS } else { 5 * 60_000 }))
        })
        .collect();
    let names = agents.iter().map(|agent| (agent.id.clone(), agent.name.clone())).collect();
    let name = project.map_or_else(|| "everyone".to_owned(), |project| project.name.clone());
    Ok((Team { name, members, ttl_ms, compact_at: config.compact_at_tokens }, names))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Args {
    #[serde(default)]
    window: Option<f64>,
    #[serde(default)]
    offset: Option<f64>,
    #[serde(default)]
    project: Option<String>,
}

/// Everything a report says.
#[derive(serde::Serialize)]
struct Report {
    team: String,
    agents: usize,
    window: Window,
    load: Vec<metrics::Load>,
    answers: Vec<metrics::Answers>,
    traffic: metrics::Traffic,
    days: Vec<metrics::Day>,
    flow: metrics::Flow,
    blocked: Vec<metrics::Blocked>,
    cards: metrics::Cards,
    wakes: Vec<metrics::Wakes>,
    usage: Vec<metrics::Usage>,
    cache_write_share: f64,
    monitors: Vec<metrics::Waker>,
    red: Vec<Sign>,
    only_now: Vec<Sign>,
}

fn measure(db: &rusqlite::Connection, team: &Team, names: &HashMap<String, String>, window: Window, now: i64) -> rusqlite::Result<Report> {
    let usage = metrics::usage(db, team, window)?;
    let (red, only_now) = signs::red(db, team, window.t1, now, names)?;
    Ok(Report {
        team: team.name.clone(),
        agents: team.members.len(),
        window,
        load: metrics::load(db, team, window)?,
        answers: metrics::answers(db, team, window)?,
        traffic: metrics::traffic(db, team, window)?,
        days: metrics::days(db, team, window.t1)?,
        flow: metrics::flow(db, team, window)?,
        blocked: metrics::blocked(db, team, now, names)?,
        cards: metrics::cards(db, team, window, now)?,
        wakes: metrics::wakes(db, team, window)?,
        cache_write_share: signs::write_share(&usage, team),
        usage,
        monitors: metrics::wakers(db, team, window)?,
        red,
        only_now,
    })
}

/// A report on Claktory's records at `home`, for `team`, over `window`
/// (`now`: what time it is). Blocking.
fn report(home: &crate::home::Home, team: &Team, names: &HashMap<String, String>, window: Window, now: i64) -> Result<(String, Value), String> {
    let db = metrics::open(home)?;
    let report = measure(&db, team, names, window, now).map_err(|error| format!("Claktory's records couldn't be read: {error}"))?;
    let details = serde_json::to_value(&report).unwrap_or(Value::Null);
    Ok((text(&report), details))
}

fn local(ms: i64) -> String {
    crate::transcript::local_time(ms, "%Y-%m-%d %H:%M")
}

fn pct(share: f64) -> String {
    format!("{:.0}%", share * 100.0)
}

fn opt(value: Option<f64>, digits: usize) -> String {
    value.map_or_else(|| "–".to_owned(), |value| format!("{value:.digits$}"))
}

fn millions(tokens: i64) -> String {
    format!("{:.1}", tokens as f64 / 1e6)
}

/// The report as the agent reads it.
fn text(report: &Report) -> String {
    let mut out = String::new();
    let window = report.window;
    let _ = writeln!(
        out,
        "Team health: {} ({} agent{}), {:.0} hours from {} to {}. Counts and times only.\n",
        report.team,
        report.agents,
        if report.agents == 1 { "" } else { "s" },
        window.hours(),
        local(window.t0),
        local(window.t1)
    );
    if report.red.is_empty() {
        out.push_str("No warning sign is red (held in the last 6 hours and the 6 before them).\n");
    } else {
        out.push_str("Red (held in the last 6 hours and the 6 before them):\n");
        for sign in &report.red {
            let _ = writeln!(out, "- {} {}", sign.says, sign.means);
        }
    }
    if !report.only_now.is_empty() {
        out.push_str("Only in the last 6 hours, not yet a trend:\n");
        for sign in &report.only_now {
            let _ = writeln!(out, "- {}", sign.says);
        }
    }

    out.push_str("\nQ1 Busy (share of the window in runs) | runs started | short (<1 min):\n");
    if report.load.is_empty() {
        out.push_str("No runs.\n");
    }
    for load in &report.load {
        let _ = writeln!(out, "{} | {} | {} | {}", load.agent, pct(load.busy_share), load.runs, load.short_run_share().map_or("–".to_owned(), pct));
    }
    let total: f64 = report.load.iter().map(|load| load.busy_share).sum();
    let _ = writeln!(out, "On average {total:.1} agents worked at once.");

    out.push_str("\nQ2 Answering other agents, minutes: agent | replies | p50 | p90 (any reply counts, acknowledgements too):\n");
    if report.answers.is_empty() {
        out.push_str("No replies.\n");
    }
    for answer in &report.answers {
        let _ = writeln!(out, "{} | {} | {:.1} | {:.1}", answer.agent, answer.replies, answer.p50_min, answer.p90_min);
    }

    let traffic = &report.traffic;
    let _ = writeln!(out, "\nQ3 Traffic: {} agent-to-agent messages.", traffic.messages);
    if traffic.messages > 0 {
        let _ = writeln!(
            out,
            "Busiest: {}, in {} of them (it sent {}, and received {} from {} agent{}); {} touch it, {} touch another lead, {} are between the rest.",
            traffic.hub,
            traffic.hub_sent + traffic.hub_received,
            traffic.hub_sent,
            traffic.hub_received,
            traffic.hub_senders,
            if traffic.hub_senders == 1 { "" } else { "s" },
            pct(traffic.via_hub),
            pct(traffic.via_leads),
            pct(traffic.between_peers)
        );
        out.push_str("agent | sent | received | share\n");
        for share in traffic.agents.iter().take(15) {
            let _ = writeln!(out, "{} | {} | {} | {}", share.agent, share.sent, share.received, pct(share.share));
        }
    }
    if let Some(known) = traffic.provenance.as_ref().filter(|known| known.recorded > 0 || known.quiet_copies > 0) {
        let _ = writeln!(
            out,
            "Provenance, for the {} recorded with it: {} sends; {} say which message they answer; {} sent as handoffs (as their senders said); {} quiet copies to leads. A message its sender's run read before sending is context, not a confirmed answer.",
            known.recorded, known.sends, known.answers, known.handoffs, known.quiet_copies
        );
    }

    out.push_str("\nQ4 By day: day | agent messages | person's | agents active | runs | short | items done | cards | agent messages per item done:\n");
    for day in &report.days {
        let _ = writeln!(
            out,
            "{} | {} | {} | {} | {} | {} | {} | {} | {}",
            day.day,
            day.agent_messages,
            day.person_messages,
            day.agents_active,
            day.runs,
            day.short_run_share.map_or("–".to_owned(), pct),
            day.items_done,
            day.cards,
            opt(day.messages_per_item(), 1)
        );
    }

    let flow = &report.flow;
    let _ = writeln!(
        out,
        "\nQ5 Flow: {} items finished ({:.2} an hour), cycle p50 {} h, p90 {} h; {} open now (open, in progress or blocked); Little's law gives {} h. When that's several times the p90, most open items aren't flowing work.",
        flow.finished,
        flow.finished_per_hour,
        opt(flow.cycle_p50_h, 1),
        opt(flow.cycle_p90_h, 1),
        flow.open_now,
        opt(flow.littles_law_cycle_h, 1)
    );

    let _ = writeln!(out, "\nQ6 Blocked now: {} items. list #item | owner | hours | waits on | owner's card waiting:", report.blocked.len());
    for item in report.blocked.iter().take(30) {
        let _ = writeln!(
            out,
            "{} #{} | {} | {:.1} | {} | {}",
            item.list,
            item.item,
            item.owner,
            item.hours,
            item.on,
            if item.card_pending { "yes" } else { "no" }
        );
    }
    if report.blocked.len() > 30 {
        let _ = writeln!(out, "+{} more", report.blocked.len() - 30);
    }

    let cards = &report.cards;
    let _ = writeln!(
        out,
        "\nQ7 The person's cards: {} raised ({:.1} a day), {} decided, {} dismissed (questions a /clear closed unseen count as neither); decided in {} min (p50), {} min (p90).",
        cards.raised,
        cards.per_day,
        cards.decided,
        cards.dismissed,
        opt(cards.decided_p50_min, 0),
        opt(cards.decided_p90_min, 0)
    );
    if !cards.pending.is_empty() {
        let waiting: Vec<String> = cards.pending.iter().map(|(agent, hours)| format!("{agent} ({hours:.1} h)")).collect();
        let _ = writeln!(out, "Waiting now: {}.", waiting.join(", "));
    }

    out.push_str(
        "\nQ8 Wake-ups, runs by how long after the agent's last one they started: agent | its cache now | within 5 min | 5 to 60 min | over 60 min | over 5 min and under a minute long. A run later than the cache lasts writes its whole context again; the records don't keep when the cache's lifetime changed, so an earlier window is read against today's:\n",
    );
    if report.wakes.is_empty() {
        out.push_str("None.\n");
    }
    for wake in report.wakes.iter().take(15) {
        let cache = if wake.cache_min >= 60 { "1 h".to_owned() } else { format!("{} min", wake.cache_min) };
        let _ = writeln!(
            out,
            "{} | {cache} | {} | {} | {} | {}",
            wake.agent, wake.within_5_min, wake.from_5_to_60_min, wake.over_60_min, wake.late_and_short
        );
    }

    let _ = writeln!(
        out,
        "\nQ9 Tokens, millions: agent | model | calls | context a call (k) | input | cache write | cache read | output. Cache writes are {} of Anthropic models' token cost, at Anthropic's price ratios and each agent's cache lifetime now:",
        pct(report.cache_write_share)
    );
    for usage in report.usage.iter().take(20) {
        let _ = writeln!(
            out,
            "{} | {} | {} | {:.0} | {} | {} | {} | {}",
            usage.agent,
            usage.model,
            usage.requests,
            usage.context_k(),
            millions(usage.input),
            millions(usage.cache_write),
            millions(usage.cache_read),
            millions(usage.output)
        );
    }

    out.push_str("\nQ10 Monitors that fire most: agent monitor | kind | waits until idle | fires in the window | ever:\n");
    if report.monitors.is_empty() {
        out.push_str("None active.\n");
    }
    for waker in &report.monitors {
        let _ = writeln!(
            out,
            "{} {} | {} | {} | {} | {}",
            waker.agent,
            waker.monitor,
            waker.kind,
            if waker.idle_only { "yes" } else { "no" },
            waker.fires_in_window,
            waker.fires_ever
        );
    }
    out.push_str(
        "\nNot measured (say so when it matters): time runs waited on usage limits (busy counts it), tokens per run, model time against tool time, which message answers which (Q2 pairs them by order), the machine's load (the Resources extension), and why an item is blocked beyond what it waits on.",
    );
    out
}

fn swarm_report(factory: &Arc<Factory>, agent: &str, args: Args) -> Result<ToolResult, String> {
    let window = args.window.unwrap_or(6.0);
    let offset = args.offset.unwrap_or(0.0);
    if !(1.0..=24.0 * 30.0).contains(&window) {
        return Err("window is hours, 1 to 720.".to_owned());
    }
    if !(0.0..=24.0 * 30.0).contains(&offset) {
        return Err("offset is hours before now, 0 to 720.".to_owned());
    }
    let (team, names) = {
        let inner = factory.lock();
        super::caller(&inner.config, agent)?;
        team(&inner.config, args.project.as_deref())?
    };
    let now = now_ms();
    let window = Window::ending(now - (offset * HOUR_MS as f64) as i64, window);
    factory.store.write_calls();
    let (text, details) = report(&factory.home, &team, &names, window, now)?;
    Ok(ToolResult::text(text, details))
}

impl Extension for Swarm {
    fn manifest(&self) -> &'static Manifest {
        &MANIFEST
    }

    fn prompt(&self, _factory: &Factory, _inner: &Inner, _agent: &AgentConfig) -> Vec<PromptSection> {
        vec![PromptSection::static_text(PROMPT)]
    }

    fn tools(&self, factory: &Arc<Factory>, agent: &str) -> Vec<Tool> {
        let parameters = json!({
            "type": "object",
            "properties": {
                "window": {"type": "number", "minimum": 1, "maximum": 720},
                "offset": {"type": "number", "minimum": 0, "maximum": 720},
                "project": {"type": "string"},
            },
            "additionalProperties": false,
        });
        vec![super::tool("swarm_report", "Team health", ABOUT, parameters, super::typed_handler(factory, agent, swarm_report))]
    }

    fn admin_default(&self) -> bool {
        true
    }
}

// ---- The watch ----

/// Checks every hour, for as long as Claktory runs.
pub(crate) async fn monitor(factory: Weak<Factory>) {
    let mut wait = FIRST;
    loop {
        tokio::time::sleep(wait).await;
        wait = EVERY;
        let Some(factory) = factory.upgrade() else { return };
        let checked = tokio::task::spawn_blocking(move || check(&factory, now_ms())).await;
        if let Ok(Err(error)) = checked {
            tracing::error!(%error, "could not check the team's health");
        }
    }
}

/// Who watches, and the team each looks at: admin agents everyone, others
/// their own project.
fn watchers(config: &Config) -> Vec<(String, Option<String>)> {
    config
        .agents
        .iter()
        .filter(|agent| !config.archived(agent) && enabled(agent, ID) && switch(agent, ID, "watch"))
        .map(|agent| {
            let project = if switch(agent, agents::ID, "admin") { None } else { agent.project.clone() };
            (agent.id.clone(), project)
        })
        .collect()
}

/// One check at `now`: each watching agent is told about the signs that
/// turned red since it was last told, and forgets those that cleared.
/// Blocking.
fn check(factory: &Arc<Factory>, now: i64) -> Result<(), String> {
    let (watchers, person, teams) = {
        let inner = factory.lock();
        let watchers = watchers(&inner.config);
        let mut teams = HashMap::new();
        for (_, project) in &watchers {
            if !teams.contains_key(project) {
                teams.insert(project.clone(), team(&inner.config, project.as_deref())?);
            }
        }
        (watchers, inner.config.person().to_owned(), teams)
    };
    if watchers.is_empty() {
        return Ok(());
    }
    factory.store.write_calls();
    let db = metrics::open(&factory.home)?;
    let mut red: HashMap<Option<String>, Vec<Sign>> = HashMap::new();
    for (project, (team, names)) in &teams {
        let (signs, _) = signs::red(&db, team, now, now, names).map_err(|error| error.to_string())?;
        red.insert(project.clone(), signs);
    }
    drop(db);
    for (agent, project) in watchers {
        let signs = &red[&project];
        let told: BTreeSet<String> = {
            let db = factory.store.db();
            let mut statement = db.prepare("SELECT sign FROM swarm_watch WHERE agent = ?1").map_err(|error| error.to_string())?;
            statement.query_map([&agent], |row| row.get(0)).and_then(Iterator::collect).map_err(|error| error.to_string())?
        };
        let new: Vec<&Sign> = signs.iter().filter(|sign| !told.contains(&sign.key)).collect();
        {
            let db = factory.store.db();
            for key in told.iter().filter(|key| !signs.iter().any(|sign| &sign.key == *key)) {
                db.execute("DELETE FROM swarm_watch WHERE agent = ?1 AND sign = ?2", rusqlite::params![agent, key]).map_err(|error| error.to_string())?;
            }
            for sign in &new {
                db.execute("INSERT OR REPLACE INTO swarm_watch (agent, sign, told) VALUES (?1, ?2, ?3)", rusqlite::params![agent, sign.key, now])
                    .map_err(|error| error.to_string())?;
            }
        }
        if new.is_empty() {
            continue;
        }
        let mut text = format!(
            "Team health: {} turned red in Claktory's records (held in the last 6 hours and the 6 before them):\n",
            if new.len() == 1 { "a warning sign" } else { "warning signs" }
        );
        for sign in &new {
            let _ = writeln!(text, "- {} {}", sign.says, sign.means);
        }
        let _ = write!(
            text,
            "Run swarm_report (6 hours, then 72) and follow the playbook before proposing anything: at most five changes on one ask_person card, applied only once {person} approves. Claktory won't mention {} again until {} clears.",
            if new.len() == 1 { "it" } else { "them" },
            if new.len() == 1 { "it" } else { "each" }
        );
        let summary = if new.len() == 1 { format!("Team health: {}", short(&new[0].says)) } else { format!("Team health: {} warning signs turned red", new.len()) };
        tracing::info!(%agent, signs = new.len(), "told an agent the team's warning signs");
        factory.deliver_event(&agent, text, host_event(summary));
    }
    Ok(())
}

/// A sign's line up to its colon: "Silent waits", "A hub".
fn short(says: &str) -> String {
    says.split(':').next().unwrap_or(says).trim().to_owned()
}

/// Tests: one hourly check, as if it were `now`.
#[doc(hidden)]
pub fn check_for_test(factory: &Arc<Factory>, now: i64) -> Result<(), String> {
    check(factory, now)
}
