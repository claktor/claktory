//! The playbook's ten measures (Melone's optimize-swarm-playbook.md §2,
//! Q1–Q10), over Claktory's records, read only. Each is its SQL as the
//! playbook has it, but with the window given as times (so a fixed clock
//! gives fixed numbers) and every row scoped to the team's agents. They
//! read counts, times, states and token totals: never what anyone wrote.

use std::collections::{BTreeMap, HashMap};
use std::time::{Duration, Instant};

use rusqlite::{Connection, OpenFlags, params};
use serde::Serialize;

use crate::home::Home;

pub(crate) const HOUR_MS: i64 = 3_600_000;
/// A run shorter than this is a short one: coordination rather than work.
const SHORT_MS: i64 = 60_000;
/// How long one report may take before it's cut off.
const TIME_LIMIT: Duration = Duration::from_secs(20);

/// The agents a report is about.
#[derive(Clone, Debug, Default)]
pub(crate) struct Team {
    /// "everyone", or the project's name.
    pub name: String,
    pub members: Vec<Member>,
    /// How long each member's prompt cache lasts now, in ms, for those on
    /// a provider whose lifetime Claktory sets (Anthropic): a run that
    /// starts later than that after its last one writes its context again.
    pub ttl_ms: BTreeMap<String, i64>,
    /// Contexts are summarized at this many tokens.
    pub compact_at: u64,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct Member {
    pub id: String,
    pub role: String,
    /// Someone reports to it.
    pub leads: bool,
}

impl Team {
    /// Its ids as JSON, for `json_each`.
    fn ids(&self) -> String {
        serde_json::to_string(&self.members.iter().map(|member| &member.id).collect::<Vec<_>>()).unwrap_or_default()
    }

}

/// Claktory's records, read only, with a time limit.
pub(crate) fn open(home: &Home) -> Result<Connection, String> {
    let failed = |error: rusqlite::Error| format!("Claktory's records couldn't be opened: {error}");
    let flags = OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX;
    let mut db = Connection::open_with_flags(home.db_path(), flags).map_err(failed)?;
    db.pragma_update(None, "query_only", true).map_err(failed)?;
    db.busy_timeout(Duration::from_secs(5)).map_err(failed)?;
    let deadline = Instant::now() + TIME_LIMIT;
    db.progress_handler(10_000, Some(move || Instant::now() > deadline)).map_err(failed)?;
    Ok(db)
}

/// Collects an already prepared cached query, stopping at the first row error.
fn query<T, C: FromIterator<T>>(
    mut statement: rusqlite::CachedStatement<'_>,
    parameters: impl rusqlite::Params,
    read: impl FnMut(&rusqlite::Row<'_>) -> rusqlite::Result<T>,
) -> rusqlite::Result<C> {
    statement.query_map(parameters, read)?.collect()
}

/// A window of time: from `t0` to `t1`, in ms since 1970.
#[derive(Clone, Copy, Debug, Serialize)]
pub(crate) struct Window {
    pub t0: i64,
    pub t1: i64,
}

impl Window {
    /// `hours` long, ending `end`.
    pub(crate) fn ending(end: i64, hours: f64) -> Window {
        Window { t0: end - (hours * HOUR_MS as f64) as i64, t1: end }
    }

    pub(crate) fn hours(&self) -> f64 {
        (self.t1 - self.t0) as f64 / HOUR_MS as f64
    }
}

// ---- Q1: how busy each agent is ----

#[derive(Clone, Debug, Serialize)]
pub(crate) struct Load {
    pub agent: String,
    /// The share of the window spent in runs.
    pub busy_share: f64,
    /// Runs started in the window, and how many of them were short.
    pub runs: i64,
    pub short_runs: i64,
}

impl Load {
    pub(crate) fn short_run_share(&self) -> Option<f64> {
        (self.runs > 0).then(|| self.short_runs as f64 / self.runs as f64)
    }
}

const Q1: &str = "
WITH w(t0, t1) AS (SELECT ?1, ?2),
r AS (SELECT agent, started AS s,
  CASE WHEN outcome = 'interrupted' THEN MAX(started, MIN(COALESCE(last_call, started), COALESCE(ended, started)))
       ELSE COALESCE(ended, (SELECT t1 FROM w)) END AS e
  FROM runs, w WHERE started < w.t1 AND (ended IS NULL OR ended > w.t0 OR outcome = 'interrupted')
  AND agent IN (SELECT value FROM json_each(?3)))
SELECT agent, SUM(MAX(0, MIN(e, t1) - MAX(s, t0))) * 1.0 / (t1 - t0) AS busy_share,
  SUM(s >= t0) AS runs, SUM(s >= t0 AND e - s < ?4) AS short_runs
FROM r, w GROUP BY agent HAVING busy_share > 0 ORDER BY busy_share DESC";

pub(crate) fn load(db: &Connection, team: &Team, window: Window) -> rusqlite::Result<Vec<Load>> {
    query(db.prepare_cached(Q1)?, params![window.t0, window.t1, team.ids(), SHORT_MS], |row| {
        Ok(Load { agent: row.get(0)?, busy_share: row.get(1)?, runs: row.get(2)?, short_runs: row.get(3)? })
    })
}

// ---- Q2: how long an agent takes to answer another ----

#[derive(Clone, Debug, Serialize)]
pub(crate) struct Answers {
    pub agent: String,
    pub replies: i64,
    pub p50_min: f64,
    pub p90_min: f64,
}

const Q2: &str = "
WITH m AS (SELECT agent AS receiver, sender_id AS sender, created, MIN(agent, sender_id) || '|' || MAX(agent, sender_id) AS pair
  FROM messages WHERE source = 'agent' AND created >= ?1 AND created < ?2
  AND agent IN (SELECT value FROM json_each(?3)) AND sender_id IN (SELECT value FROM json_each(?3))),
n AS (SELECT receiver, created, LEAD(sender) OVER (PARTITION BY pair ORDER BY created) AS next_sender,
  LEAD(created) OVER (PARTITION BY pair ORDER BY created) AS next_created FROM m),
d AS (SELECT receiver AS responder, next_created - created AS wait FROM n WHERE next_sender = receiver AND next_created - created < 43200000),
k AS (SELECT responder, wait, ROW_NUMBER() OVER (PARTITION BY responder ORDER BY wait) AS i, COUNT(*) OVER (PARTITION BY responder) AS n FROM d)
SELECT responder, n AS replies, MIN(CASE WHEN i >= 0.5*n THEN wait END) / 60000.0 AS p50_min, MIN(CASE WHEN i >= 0.9*n THEN wait END) / 60000.0 AS p90_min
FROM k GROUP BY responder ORDER BY p90_min DESC";

pub(crate) fn answers(db: &Connection, team: &Team, window: Window) -> rusqlite::Result<Vec<Answers>> {
    query(db.prepare_cached(Q2)?, params![window.t0, window.t1, team.ids()], |row| {
        Ok(Answers { agent: row.get(0)?, replies: row.get(1)?, p50_min: row.get(2)?, p90_min: row.get(3)? })
    })
}

// ---- Q3: who's in the middle of the traffic ----

#[derive(Clone, Debug, Default, Serialize)]
pub(crate) struct Traffic {
    /// Agent-to-agent messages within the team.
    pub messages: i64,
    /// Each agent's, most first.
    pub agents: Vec<Share>,
    /// The busiest agent, and the shares of the messages touching it, touching
    /// another lead (an agent someone reports to), and between the rest.
    pub hub: String,
    /// What the hub sent, what it was sent, and by how many agents.
    pub hub_sent: i64,
    pub hub_received: i64,
    pub hub_senders: i64,
    pub via_hub: f64,
    pub via_leads: f64,
    pub between_peers: f64,
    /// Where messages stand (#285), for those recorded with it; None when
    /// the records have no such columns.
    pub provenance: Option<Provenance>,
}

/// Of the team's agent messages recorded with provenance: how many, in how
/// many sends, how many say which message they answer (as their sender
/// said, checked), how many were sent as handoffs, and the quiet copies to
/// leads. Messages a run read before sending are context, never counted as
/// answered.
#[derive(Clone, Debug, Default, Serialize)]
pub(crate) struct Provenance {
    pub recorded: i64,
    pub sends: i64,
    pub answers: i64,
    pub handoffs: i64,
    pub quiet_copies: i64,
}

const PROVENANCE: &str = "
SELECT COUNT(*), COUNT(DISTINCT send_id), COALESCE(SUM(reply_to != '[]'), 0), COALESCE(SUM(intent = 'handoff'), 0) FROM messages
WHERE source = 'agent' AND send_id IS NOT NULL AND created >= ?1 AND created < ?2
  AND agent IN (SELECT value FROM json_each(?3)) AND sender_id IN (SELECT value FROM json_each(?3))";

const QUIET_COPIES: &str = "
SELECT COUNT(*) FROM quiet_copies WHERE created >= ?1 AND created < ?2
  AND agent IN (SELECT value FROM json_each(?3)) AND sender_id IN (SELECT value FROM json_each(?3))";

/// `Provenance` over the window; None from records before it.
fn provenance(db: &Connection, team: &Team, window: Window) -> Option<Provenance> {
    let ids = team.ids();
    let (recorded, sends, answers, handoffs) = db
        .query_row(PROVENANCE, params![window.t0, window.t1, ids], crate::store::row4)
        .ok()?;
    let quiet_copies = db.query_row(QUIET_COPIES, params![window.t0, window.t1, ids], |row| row.get(0)).ok()?;
    Some(Provenance { recorded, sends, answers, handoffs, quiet_copies })
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct Share {
    pub agent: String,
    pub sent: i64,
    pub received: i64,
    pub share: f64,
}

const Q3: &str = "
SELECT sender_id, agent, COUNT(*) FROM messages
WHERE source = 'agent' AND created >= ?1 AND created < ?2
  AND agent IN (SELECT value FROM json_each(?3)) AND sender_id IN (SELECT value FROM json_each(?3))
GROUP BY sender_id, agent";

pub(crate) fn traffic(db: &Connection, team: &Team, window: Window) -> rusqlite::Result<Traffic> {
    let mut statement = db.prepare_cached(Q3)?;
    let pairs: Vec<(String, String, i64)> =
        statement.query_map(params![window.t0, window.t1, team.ids()], crate::store::row3)?.collect::<Result<_, _>>()?;
    let messages: i64 = pairs.iter().map(|(_, _, count)| count).sum();
    if messages == 0 {
        return Ok(Traffic { provenance: provenance(db, team, window), ..Traffic::default() });
    }
    let mut by_agent: HashMap<&str, (i64, i64)> = HashMap::new();
    for (from, to, count) in &pairs {
        by_agent.entry(from).or_default().0 += count;
        by_agent.entry(to).or_default().1 += count;
    }
    let mut agents: Vec<Share> = by_agent
        .into_iter()
        .map(|(agent, (sent, received))| Share { agent: agent.to_owned(), sent, received, share: (sent + received) as f64 / messages as f64 })
        .collect();
    // A tie goes to the one sent more: a hub is who the others write to.
    agents.sort_by(|a, b| b.share.total_cmp(&a.share).then_with(|| b.received.cmp(&a.received)).then_with(|| a.agent.cmp(&b.agent)));
    let hub = agents[0].agent.clone();
    let hub_sent = agents[0].sent;
    let lead = |id: &str| team.members.iter().any(|member| member.id == id && member.leads);
    let (mut via_hub, mut via_leads) = (0, 0);
    let (mut hub_received, mut hub_senders) = (0, 0);
    for (from, to, count) in &pairs {
        if *to == hub {
            hub_received += count;
            hub_senders += 1;
        }
        if *from == hub || *to == hub {
            via_hub += count;
        } else if lead(from) || lead(to) {
            via_leads += count;
        }
    }
    let share = |count: i64| count as f64 / messages as f64;
    Ok(Traffic {
        messages,
        agents,
        hub,
        hub_sent,
        hub_received,
        hub_senders,
        via_hub: share(via_hub),
        via_leads: share(via_leads),
        between_peers: share(messages - via_hub - via_leads),
        provenance: provenance(db, team, window),
    })
}

// ---- Q4: the trend by day ----

#[derive(Clone, Debug, Serialize)]
pub(crate) struct Day {
    /// YYYY-MM-DD, local time.
    pub day: String,
    pub agent_messages: i64,
    pub person_messages: i64,
    pub agents_active: i64,
    pub runs: i64,
    pub short_run_share: Option<f64>,
    pub items_done: i64,
    pub cards: i64,
}

impl Day {
    /// Agent messages for each finished item.
    pub(crate) fn messages_per_item(&self) -> Option<f64> {
        (self.items_done > 0).then(|| self.agent_messages as f64 / self.items_done as f64)
    }
}

const Q4: &str = "
WITH d AS (SELECT date(created/1000, 'unixepoch', 'localtime') AS day, source FROM messages
  WHERE created >= ?1 AND created < ?2 AND agent IN (SELECT value FROM json_each(?3)))
SELECT day, SUM(source = 'agent') AS agent_messages, SUM(source = 'person') AS person_messages,
  (SELECT COUNT(DISTINCT agent) FROM runs WHERE date(started/1000, 'unixepoch', 'localtime') = d.day AND agent IN (SELECT value FROM json_each(?3))) AS agents_active,
  (SELECT COUNT(*) FROM runs WHERE date(started/1000, 'unixepoch', 'localtime') = d.day AND agent IN (SELECT value FROM json_each(?3))) AS runs,
  (SELECT AVG(ended - started < ?4) FROM runs WHERE ended IS NOT NULL AND date(started/1000, 'unixepoch', 'localtime') = d.day AND agent IN (SELECT value FROM json_each(?3))) AS short_run_share,
  (SELECT COUNT(*) FROM checklist_events WHERE kind = 'status' AND value = 'done' AND date(time/1000, 'unixepoch', 'localtime') = d.day AND agent IN (SELECT value FROM json_each(?3))) AS items_done,
  (SELECT COUNT(*) FROM asks WHERE date(created/1000, 'unixepoch', 'localtime') = d.day AND agent IN (SELECT value FROM json_each(?3)))
    + (SELECT COUNT(*) FROM proposals WHERE date(created/1000, 'unixepoch', 'localtime') = d.day AND agent IN (SELECT value FROM json_each(?3))) AS cards
FROM d GROUP BY day ORDER BY day";

/// The 7 days up to `end`.
pub(crate) fn days(db: &Connection, team: &Team, end: i64) -> rusqlite::Result<Vec<Day>> {
    query(db.prepare_cached(Q4)?, params![end - 7 * 24 * HOUR_MS, end, team.ids(), SHORT_MS], |row| {
        Ok(Day {
            day: row.get(0)?,
            agent_messages: row.get(1)?,
            person_messages: row.get(2)?,
            agents_active: row.get(3)?,
            runs: row.get(4)?,
            short_run_share: row.get(5)?,
            items_done: row.get(6)?,
            cards: row.get(7)?,
        })
    })
}

// ---- Q5: flow ----

#[derive(Clone, Debug, Default, Serialize)]
pub(crate) struct Flow {
    pub finished: i64,
    pub finished_per_hour: f64,
    pub cycle_p50_h: Option<f64>,
    pub cycle_p90_h: Option<f64>,
    /// Open, in progress or blocked now.
    pub open_now: i64,
    /// What Little's law makes of the open items: open ÷ finished an hour.
    pub littles_law_cycle_h: Option<f64>,
}

const Q5: &str = "
WITH done AS (SELECT e.agent, e.item, MAX(e.time) AS done_at FROM checklist_events e
  WHERE e.kind = 'status' AND e.value = 'done' AND e.time >= ?1 AND e.time < ?2 AND e.agent IN (SELECT value FROM json_each(?3))
  GROUP BY e.agent, e.item),
c AS (SELECT (done_at - i.created) AS cycle, ROW_NUMBER() OVER (ORDER BY done_at - i.created) AS k, COUNT(*) OVER () AS n
  FROM done JOIN checklist_items i ON i.agent = done.agent AND i.id = done.item)
SELECT (SELECT COUNT(*) FROM done) AS finished,
  MIN(CASE WHEN k >= 0.5*n THEN cycle END) / 3600000.0 AS cycle_p50_h, MIN(CASE WHEN k >= 0.9*n THEN cycle END) / 3600000.0 AS cycle_p90_h,
  (SELECT COUNT(*) FROM checklist_items WHERE status IN ('open', 'in_progress', 'blocked') AND agent IN (SELECT value FROM json_each(?3))) AS open_now
FROM c";

pub(crate) fn flow(db: &Connection, team: &Team, window: Window) -> rusqlite::Result<Flow> {
    let (finished, p50, p90, open_now): (i64, Option<f64>, Option<f64>, i64) =
        db.prepare_cached(Q5)?.query_row(params![window.t0, window.t1, team.ids()], crate::store::row4)?;
    let per_hour = finished as f64 / window.hours();
    Ok(Flow {
        finished,
        finished_per_hour: per_hour,
        cycle_p50_h: p50,
        cycle_p90_h: p90,
        open_now,
        littles_law_cycle_h: (finished > 0).then(|| open_now as f64 / per_hour),
    })
}

// ---- Q6: what's blocked, how long, and on what ----

#[derive(Clone, Debug, Serialize)]
pub(crate) struct Blocked {
    /// Whose checklist it's on, and its number there.
    pub list: String,
    pub item: i64,
    /// Its owner; the list's agent when it has none.
    pub owner: String,
    pub hours: f64,
    /// What it waits on, as the checklist types it: "person", "agent" (an
    /// agent of the team named), "other" (something named outside), or
    /// "unstated" (blocked before it was typed, or with nothing said).
    pub on: &'static str,
    /// The owner has a question or proposal waiting for the person.
    pub card_pending: bool,
}

const Q6: &str = "
SELECT i.agent, i.id, i.owner,
  (?1 - COALESCE((SELECT MAX(time) FROM checklist_events e WHERE e.agent = i.agent AND e.item = i.id AND e.kind = 'status' AND e.value = 'blocked'), i.updated)) / 3600000.0 AS blocked_h,
  i.blocked_on,
  EXISTS (SELECT 1 FROM asks a WHERE a.state = 'pending' AND a.agent = CASE WHEN i.owner = '' THEN i.agent ELSE i.owner END)
    OR EXISTS (SELECT 1 FROM proposals p WHERE p.state = 'pending' AND p.agent = CASE WHEN i.owner = '' THEN i.agent ELSE i.owner END)
FROM checklist_items i WHERE i.status = 'blocked' AND i.agent IN (SELECT value FROM json_each(?2)) ORDER BY blocked_h DESC";

/// What's blocked now (`now`: when that is).
pub(crate) fn blocked(db: &Connection, team: &Team, now: i64, names: &HashMap<String, String>) -> rusqlite::Result<Vec<Blocked>> {
    query(db.prepare_cached(Q6)?, params![now, team.ids()], |row| blocked_row(row, names))
}

/// What was blocked at `t`, rebuilt from the checklists' history: items
/// whose last status before `t` was blocked, since then, with their owner
/// and what they waited on as recorded by then ("unstated" from before it
/// was typed), and whether the owner had a card waiting at `t`.
const Q6_AT: &str = "
WITH last AS (SELECT agent, item, MAX(seq) AS seq FROM checklist_events
  WHERE kind = 'status' AND time < ?1 AND agent IN (SELECT value FROM json_each(?2)) GROUP BY agent, item),
b AS (SELECT e.agent, e.item, e.time AS since FROM last JOIN checklist_events e ON e.seq = last.seq WHERE e.value = 'blocked'),
o AS (SELECT b.*, COALESCE((SELECT value FROM checklist_events e WHERE e.agent = b.agent AND e.item = b.item AND e.kind = 'owner' AND e.time < ?1 ORDER BY seq DESC LIMIT 1),
    (SELECT owner FROM checklist_items i WHERE i.agent = b.agent AND i.id = b.item), '') AS owner FROM b)
SELECT o.agent, o.item, o.owner, (?1 - o.since) / 3600000.0 AS blocked_h,
  COALESCE((SELECT value FROM checklist_events e WHERE e.agent = o.agent AND e.item = o.item AND e.kind = 'blocked_on' AND e.time < ?1 AND e.time >= o.since - 1000 ORDER BY seq DESC LIMIT 1), ''),
  EXISTS (SELECT 1 FROM asks a WHERE a.agent = CASE WHEN o.owner = '' THEN o.agent ELSE o.owner END AND a.created < ?1 AND (a.decided IS NULL OR a.decided >= ?1))
    OR EXISTS (SELECT 1 FROM proposals p WHERE p.agent = CASE WHEN o.owner = '' THEN o.agent ELSE o.owner END AND p.created < ?1 AND (p.decided IS NULL OR p.decided >= ?1))
FROM o ORDER BY blocked_h DESC";

/// What was blocked at `t`, in the past (`Q6_AT`).
pub(crate) fn blocked_at(db: &Connection, team: &Team, t: i64, names: &HashMap<String, String>) -> rusqlite::Result<Vec<Blocked>> {
    query(db.prepare_cached(Q6_AT)?, params![t, team.ids()], |row| blocked_row(row, names))
}

fn blocked_row(row: &rusqlite::Row<'_>, names: &HashMap<String, String>) -> rusqlite::Result<Blocked> {
    let list: String = row.get(0)?;
    let owner: String = row.get(2)?;
    let on: String = row.get(4)?;
    Ok(Blocked {
        owner: if owner.is_empty() { list.clone() } else { owner },
        list,
        item: row.get(1)?,
        hours: row.get(3)?,
        on: waits_on(&on, names),
        card_pending: row.get(5)?,
    })
}

/// What an item's `blocked_on` says it waits on, without repeating it:
/// it's free text an agent wrote.
fn waits_on(on: &str, names: &HashMap<String, String>) -> &'static str {
    let on = on.trim();
    if on.is_empty() {
        "unstated"
    } else if on == "person" {
        "person"
    } else if names.iter().any(|(id, name)| on.eq_ignore_ascii_case(id) || on.eq_ignore_ascii_case(name)) {
        "agent"
    } else {
        "other"
    }
}

// ---- Q7: the person's cards ----

#[derive(Clone, Debug, Default, Serialize)]
pub(crate) struct Cards {
    /// Questions and proposals raised in the window.
    pub raised: i64,
    pub per_day: f64,
    /// Of those, the ones the person answered, approved or dismissed by now.
    /// Questions the agent's /clear closed unseen (state `cleared`) are
    /// neither decided nor dismissed: the person never saw them.
    pub decided: i64,
    /// Of the decided, the ones the person dismissed.
    pub dismissed: i64,
    pub decided_p50_min: Option<f64>,
    pub decided_p90_min: Option<f64>,
    /// Those waiting now (raised whenever): by agent, how long, in hours.
    pub pending: Vec<(String, f64)>,
}

const Q7: &str = "
SELECT 'ask', agent, state, created, decided FROM asks
  WHERE ((created >= ?1 AND created < ?2) OR state = 'pending') AND agent IN (SELECT value FROM json_each(?3))
UNION ALL
SELECT 'proposal', agent, state, created, decided FROM proposals
  WHERE ((created >= ?1 AND created < ?2) OR state = 'pending') AND agent IN (SELECT value FROM json_each(?3))";

pub(crate) fn cards(db: &Connection, team: &Team, window: Window, now: i64) -> rusqlite::Result<Cards> {
    let mut statement = db.prepare_cached(Q7)?;
    let rows: Vec<(String, String, i64, Option<i64>)> = statement
        .query_map(params![window.t0, window.t1, team.ids()], |row| Ok((row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?)))?
        .collect::<Result<_, _>>()?;
    let in_window: Vec<_> = rows.iter().filter(|(_, _, created, _)| *created >= window.t0 && *created < window.t1).collect();
    let mut times: Vec<i64> = in_window
        .iter()
        // Closed without the person: /clear, or withdrawn by its agent.
        .filter(|(_, state, _, _)| state != "cleared" && state != "withdrawn")
        .filter_map(|(_, _, created, decided)| decided.map(|decided| decided - created))
        .collect();
    times.sort_unstable();
    let raised = in_window.len() as i64;
    let mut pending: Vec<(String, f64)> = rows
        .iter()
        .filter(|(_, state, _, _)| state == "pending")
        .map(|(agent, _, created, _)| (agent.clone(), (now - created) as f64 / HOUR_MS as f64))
        .collect();
    pending.sort_by(|a, b| b.1.total_cmp(&a.1));
    Ok(Cards {
        raised,
        per_day: raised as f64 / (window.hours() / 24.0),
        decided: times.len() as i64,
        dismissed: in_window.iter().filter(|(_, state, _, _)| state == "dismissed").count() as i64,
        decided_p50_min: nearest_rank(&times, 0.5).map(|ms| ms as f64 / 60_000.0),
        decided_p90_min: nearest_rank(&times, 0.9).map(|ms| ms as f64 / 60_000.0),
        pending,
    })
}

/// The playbook's percentile: the first sorted value whose 1-based rank is
/// at least `q` of the count.
fn nearest_rank(sorted: &[i64], q: f64) -> Option<i64> {
    let n = sorted.len() as f64;
    sorted.iter().enumerate().find(|(i, _)| (*i + 1) as f64 >= q * n).map(|(_, value)| *value)
}

// ---- Q8: wake-ups and the prompt cache ----

/// Runs by how long after the agent's last one they started: the
/// playbook's fixed gaps. Whether a gap found the cache gone depends on
/// its lifetime then, which the records don't keep: `Team::ttl_ms` is
/// today's.
#[derive(Clone, Debug, Serialize)]
pub(crate) struct Wakes {
    pub agent: String,
    /// The prompt cache's lifetime it has now, in minutes.
    pub cache_min: i64,
    pub within_5_min: i64,
    pub from_5_to_60_min: i64,
    pub over_60_min: i64,
    /// More than 5 minutes after the last one, and shorter than a minute.
    pub late_and_short: i64,
}

impl Wakes {
    /// Runs that found the cache gone, at today's lifetime.
    pub(crate) fn cold(&self) -> i64 {
        self.cold_within_hour() + self.over_60_min
    }

    /// Of those, the ones within the hour: what a 1-hour cache keeps warm.
    pub(crate) fn cold_within_hour(&self) -> i64 {
        if self.cache_min >= 60 { 0 } else { self.from_5_to_60_min }
    }
}

const Q8: &str = "
WITH r AS (SELECT agent, started, ended, LAG(ended) OVER (PARTITION BY agent ORDER BY started) AS prev_end
  FROM runs WHERE started >= ?1 - 86400000 AND started < ?2 AND agent IN (SELECT value FROM json_each(?3)))
SELECT agent, SUM(started - prev_end <= 300000) AS within_5_min,
  SUM(started - prev_end > 300000 AND started - prev_end <= 3600000) AS from_5_to_60_min,
  SUM(started - prev_end > 3600000) AS over_60_min,
  SUM(started - prev_end > 300000 AND ended - started < ?4) AS late_and_short
FROM r WHERE started >= ?1 AND started < ?2 AND prev_end IS NOT NULL GROUP BY agent
ORDER BY from_5_to_60_min + over_60_min DESC, agent";

pub(crate) fn wakes(db: &Connection, team: &Team, window: Window) -> rusqlite::Result<Vec<Wakes>> {
    query(db.prepare_cached(Q8)?, params![window.t0, window.t1, team.ids(), SHORT_MS], |row| {
        let agent: String = row.get(0)?;
        Ok(Wakes {
            cache_min: team.ttl_ms.get(&agent).copied().unwrap_or(5 * 60_000) / 60_000,
            agent,
            within_5_min: row.get(1)?,
            from_5_to_60_min: row.get(2)?,
            over_60_min: row.get(3)?,
            late_and_short: row.get::<_, Option<i64>>(4)?.unwrap_or(0),
        })
    })
}

// ---- Q9: tokens and context size ----

#[derive(Clone, Debug, Serialize)]
pub(crate) struct Usage {
    pub agent: String,
    pub provider: String,
    pub model: String,
    pub requests: i64,
    pub input: i64,
    pub cache_write: i64,
    pub cache_read: i64,
    pub output: i64,
}

impl Usage {
    /// Thousands of tokens of context each call.
    pub(crate) fn context_k(&self) -> f64 {
        if self.requests == 0 {
            return 0.0;
        }
        (self.input + self.cache_read + self.cache_write) as f64 / self.requests as f64 / 1000.0
    }
}

const Q9: &str = "
SELECT agent, provider, model, SUM(requests), SUM(input), SUM(cache_write), SUM(cache_read), SUM(output)
FROM agent_usage WHERE hour >= ?1 / 1000 AND hour < ?2 / 1000 AND agent IN (SELECT value FROM json_each(?3))
GROUP BY agent, provider, model ORDER BY SUM(cache_read) + SUM(cache_write) DESC, agent";

pub(crate) fn usage(db: &Connection, team: &Team, window: Window) -> rusqlite::Result<Vec<Usage>> {
    query(db.prepare_cached(Q9)?, params![window.t0, window.t1, team.ids()], |row| {
        Ok(Usage {
            agent: row.get(0)?,
            provider: row.get(1)?,
            model: row.get(2)?,
            requests: row.get(3)?,
            input: row.get(4)?,
            cache_write: row.get(5)?,
            cache_read: row.get(6)?,
            output: row.get(7)?,
        })
    })
}

// ---- Q10: monitors that wake agents ----

#[derive(Clone, Debug, Serialize)]
pub(crate) struct Waker {
    pub agent: String,
    /// m1, m2…
    pub monitor: String,
    pub kind: String,
    pub idle_only: bool,
    pub fires_ever: i64,
    pub fires_in_window: i64,
}

const Q10: &str = "
SELECT m.agent, m.id, m.kind, m.idle_only, m.fires,
  (SELECT COUNT(*) FROM monitor_events e WHERE e.agent = m.agent AND e.monitor = m.id AND e.kind = 'fired' AND e.time >= ?1 AND e.time < ?2) AS fires_in_window
FROM monitors m WHERE m.state = 'active' AND m.agent IN (SELECT value FROM json_each(?3))
ORDER BY fires_in_window DESC, m.fires DESC LIMIT 10";

pub(crate) fn wakers(db: &Connection, team: &Team, window: Window) -> rusqlite::Result<Vec<Waker>> {
    query(db.prepare_cached(Q10)?, params![window.t0, window.t1, team.ids()], |row| {
        Ok(Waker {
            agent: row.get(0)?,
            monitor: format!("m{}", row.get::<_, i64>(1)?),
            kind: row.get(2)?,
            idle_only: row.get(3)?,
            fires_ever: row.get(4)?,
            fires_in_window: row.get(5)?,
        })
    })
}

// ---- What the signs need besides ----

/// Items each agent could take up now: open or in progress, by owner (the
/// list's agent when there's none).
pub(crate) fn ready(db: &Connection, team: &Team) -> rusqlite::Result<HashMap<String, i64>> {
    query(
        db.prepare_cached(
            "SELECT CASE WHEN owner = '' THEN agent ELSE owner END AS who, COUNT(*) FROM checklist_items
         WHERE status IN ('open', 'in_progress') AND agent IN (SELECT value FROM json_each(?1)) GROUP BY who",
        )?,
        params![team.ids()],
        crate::store::row2,
    )
}

/// A wake-up answered within this long by a message to another agent did
/// its job.
pub(crate) const ANSWER_MS: i64 = HOUR_MS;

/// What woke an agent in a window without the person, and how much of it
/// the agent answered.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize)]
pub(crate) struct Woken {
    /// Messages and events it was sent, but the person's and those sent
    /// quietly (which don't wake it): agents copying it in, monitors, tasks.
    pub wakes: i64,
    /// Of them, those it followed within `ANSWER_MS` with a message to
    /// another agent.
    pub answered: i64,
}

const WAKES: &str = "
SELECT agent, created FROM messages WHERE created >= ?1 AND created < ?2 AND source != 'person' AND quiet IS NULL
  AND agent IN (SELECT value FROM json_each(?3))";

const ANSWERS: &str = "
SELECT sender_id, created FROM messages WHERE source = 'agent' AND created >= ?1 AND created < ?2 AND agent != sender_id
  AND sender_id IN (SELECT value FROM json_each(?3))";

/// `Woken` for each agent woken in the window.
pub(crate) fn woken(db: &Connection, team: &Team, window: Window) -> rusqlite::Result<HashMap<String, Woken>> {
    let wakes: Vec<(String, i64)> = query(db.prepare_cached(WAKES)?, params![window.t0, window.t1, team.ids()], crate::store::row2)?;
    let sent: Vec<(String, i64)> =
        query(db.prepare_cached(ANSWERS)?, params![window.t0, window.t1 + ANSWER_MS, team.ids()], crate::store::row2)?;
    let mut sent_by: HashMap<String, Vec<i64>> = HashMap::new();
    for (agent, created) in sent {
        sent_by.entry(agent).or_default().push(created);
    }
    sent_by.values_mut().for_each(|times| times.sort_unstable());
    let mut woken: HashMap<String, Woken> = HashMap::new();
    for (agent, created) in wakes {
        let answered = sent_by.get(&agent).is_some_and(|times| {
            let next = times.partition_point(|time| *time < created);
            times.get(next).is_some_and(|time| *time < created + ANSWER_MS)
        });
        let entry = woken.entry(agent).or_default();
        entry.wakes += 1;
        entry.answered += i64::from(answered);
    }
    Ok(woken)
}

/// When each agent first shows in the records: the first message it was
/// sent or its first run, whichever came first. Claktory doesn't record
/// when an agent was made, and a new one is sent its first message as it
/// starts. An agent with no records is left out.
pub(crate) fn first_seen(db: &Connection, team: &Team) -> rusqlite::Result<HashMap<String, i64>> {
    let rows: Vec<(String, Option<i64>, Option<i64>)> = query(
        db.prepare_cached(
            "SELECT value, (SELECT created FROM messages WHERE agent = value ORDER BY id LIMIT 1),
               (SELECT started FROM runs WHERE agent = value ORDER BY id LIMIT 1) FROM json_each(?1)",
        )?,
        params![team.ids()],
        crate::store::row3,
    )?;
    Ok(rows
        .into_iter()
        .filter_map(|(agent, message, run)| {
            let first = match (message, run) {
                (Some(message), Some(run)) => message.min(run),
                (first, None) | (None, first) => first?,
            };
            Some((agent, first))
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_percentile_is_the_playbooks_nearest_rank() {
        let values = [1, 2, 3, 4, 5, 6, 7, 8, 9, 10];
        assert_eq!(nearest_rank(&values, 0.5), Some(5));
        assert_eq!(nearest_rank(&values, 0.9), Some(9));
        assert_eq!(nearest_rank(&[7], 0.9), Some(7));
        assert_eq!(nearest_rank(&[], 0.5), None);
    }
}
