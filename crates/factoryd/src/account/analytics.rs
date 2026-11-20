//! What check-ins report besides the version and platform (Matt's call,
//! API.md's check-in): how many agents, projects and paired devices there
//! are, and, per period since the last accepted check-in, the tokens of
//! model calls per provider and model, and agents' work per UTC hour.
//!
//! Model calls are counted as they end, per hour, in factory.db
//! (`model_usage`); agents' work comes from the runs factory.db records. A
//! period runs from one whole hour to another; each check-in makes the
//! periods since the last one's and keeps them (account.json) until the
//! service accepts them, so none is lost while it can't be reached, and one
//! sent twice is recorded once, by its id.

use claktory_account::protocol::{self as api, Activity, Counts, Period, Usage};
use factory_agent::{AgentEvent, Message};
use rusqlite::{Connection, params};

use crate::factory::Factory;

pub(crate) const SCHEMA: &str = "
-- Model calls by UTC hour, provider and model: the tokens check-ins at
-- claktory.com report (`crate::account`).
CREATE TABLE model_usage (
    -- Unix seconds, a multiple of 3600.
    hour INTEGER NOT NULL,
    provider TEXT NOT NULL,
    model TEXT NOT NULL,
    requests INTEGER NOT NULL,
    input INTEGER NOT NULL,
    output INTEGER NOT NULL,
    cache_read INTEGER NOT NULL,
    cache_write INTEGER NOT NULL,
    PRIMARY KEY (hour, provider, model)
);
-- When a run last had a model's answer, in ms since 1970: where a run
-- Claktory stopped in (interrupted) really ended.
ALTER TABLE runs ADD COLUMN last_call INTEGER;
";

const HOUR: i64 = 3600;
const DAY: i64 = 24 * HOUR;
/// Periods start at most this long ago (the service takes 90 days).
const OLDEST: i64 = 89 * DAY;
/// A period made after time offline covers a day at most.
const LONGEST: i64 = DAY;
/// Usage kept this long, then deleted.
const KEEP: i64 = 100 * DAY;
/// Check-in bodies stay under this many bytes (the service takes 64 KiB).
const BODY_BUDGET: usize = 48 * 1024;
/// Periods per check-in, at most (the service takes 400).
const MOST_PERIODS: usize = 400;
/// Usage entries per period, at most (the service takes 200).
const MOST_USAGE: usize = 200;

fn clamp(value: u64) -> i64 {
    i64::try_from(value).unwrap_or(i64::MAX)
}

/// A provider or model name the service takes: 1–100 of `A-Z a-z 0-9 . _ : / @ + -`.
fn valid_name(name: &str) -> bool {
    (1..=100).contains(&name.len()) && name.bytes().all(|byte| byte.is_ascii_alphanumeric() || b"._:/@+-".contains(&byte))
}

/// Counts a model call that `event` ends: an answer, or a compaction's
/// summary (by `agent_model`, "provider/model"). `run`: the agent's run.
pub(crate) fn record(db: &Connection, run: Option<i64>, agent_model: &str, event: &AgentEvent) {
    let (provider, model, usage) = match event {
        AgentEvent::MessageEnd { message: Message::Assistant(answer) } => (answer.provider.as_str(), answer.model.as_str(), &answer.usage),
        AgentEvent::Compacted { usage, .. } => {
            let (provider, model) = agent_model.split_once('/').unwrap_or(("", agent_model));
            (provider, model, usage)
        }
        _ => return,
    };
    let now = crate::transcript::now_ms();
    let hour = now / 1000 / HOUR * HOUR;
    let result = db
        .execute(
            "INSERT INTO model_usage (hour, provider, model, requests, input, output, cache_read, cache_write)
             VALUES (?1, ?2, ?3, 1, ?4, ?5, ?6, ?7)
             ON CONFLICT (hour, provider, model) DO UPDATE SET requests = requests + 1, input = input + excluded.input,
                 output = output + excluded.output, cache_read = cache_read + excluded.cache_read,
                 cache_write = cache_write + excluded.cache_write",
            params![hour, provider, model, clamp(usage.input), clamp(usage.output), clamp(usage.cache_read), clamp(usage.cache_write)],
        )
        .and_then(|_| match run {
            Some(run) => db.execute("UPDATE runs SET last_call = ?2 WHERE id = ?1", params![run, now]),
            None => Ok(0),
        });
    if let Err(error) = result {
        tracing::warn!(%error, "could not count a model call");
    }
}

/// How many agents, projects and paired devices there are now.
pub(super) fn counts(factory: &Factory) -> Counts {
    let (agents, projects) = {
        let inner = factory.lock();
        (inner.config.agents.len(), inner.config.projects.len())
    };
    let devices = crate::remote::devices(&factory.home)
        .map(|devices| devices.iter().filter(|device| !device.local && !device.endpoint_id.is_empty()).count())
        .unwrap_or(0);
    let count = |n: usize| u32::try_from(n).unwrap_or(u32::MAX);
    Counts { agents: count(agents), projects: count(projects), devices: count(devices) }
}

/// The periods from `since` (unix seconds, a whole hour; 0 for none yet)
/// to the last whole hour before `now`: none when no hour has passed. Also
/// deletes usage too old to report.
pub(super) fn new_periods(db: &Connection, since: i64, now: i64) -> rusqlite::Result<Vec<Period>> {
    db.execute("DELETE FROM model_usage WHERE hour < ?1", [now - KEEP])?;
    let to = now / HOUR * HOUR;
    let mut from = since.max(to - OLDEST);
    let mut periods = Vec::new();
    while from < to {
        let end = (from + LONGEST).min(to);
        periods.push(Period {
            id: api::b64(&api::random_32()[..18]),
            from,
            to: end,
            usage: usage(db, from, end)?,
            activity: activity(db, from, end, now)?,
        });
        from = end;
    }
    Ok(periods)
}

fn usage(db: &Connection, from: i64, to: i64) -> rusqlite::Result<Vec<Usage>> {
    let mut statement = db.prepare(
        "SELECT provider, model, SUM(requests), SUM(input), SUM(output), SUM(cache_read), SUM(cache_write) FROM model_usage
         WHERE hour >= ?1 AND hour < ?2 GROUP BY provider, model ORDER BY SUM(requests) DESC",
    )?;
    let rows = statement.query_map([from, to], |row| {
        let number = |index: usize| row.get::<_, i64>(index).map(|value| u64::try_from(value).unwrap_or(0));
        Ok(Usage {
            provider: row.get(0)?,
            model: row.get(1)?,
            model_name: None,
            requests: number(2)?,
            input_tokens: number(3)?,
            output_tokens: number(4)?,
            cache_read_tokens: number(5)?,
            cache_write_tokens: number(6)?,
        })
    })?;
    let mut usage = Vec::new();
    for row in rows {
        let row = row?;
        if valid_name(&row.provider) && valid_name(&row.model) && usage.len() < MOST_USAGE {
            usage.push(row);
        }
    }
    Ok(usage)
}

/// Agents' work per UTC hour of [from, to), from their runs; hours with none
/// are left out. A run Claktory stopped in ends at its last model answer.
fn activity(db: &Connection, from: i64, to: i64, now: i64) -> rusqlite::Result<Vec<Activity>> {
    let mut statement = db.prepare(
        "SELECT started, ended, outcome, last_call FROM runs WHERE started < ?2 AND (ended IS NULL OR ended > ?1 OR outcome = 'interrupted')",
    )?;
    let rows = statement.query_map([from * 1000, to * 1000], |row| {
        Ok((row.get::<_, i64>(0)?, row.get::<_, Option<i64>>(1)?, row.get::<_, Option<String>>(2)?, row.get::<_, Option<i64>>(3)?))
    })?;
    let mut runs: Vec<(i64, i64)> = Vec::new();
    for row in rows {
        let (started, ended, outcome, last_call) = row?;
        let ended = match outcome.as_deref() {
            Some("interrupted") => last_call.unwrap_or(started).clamp(started, ended.unwrap_or(started)),
            _ => ended.unwrap_or(now * 1000),
        };
        if ended > from * 1000 {
            runs.push((started, ended));
        }
    }
    let mut hours = Vec::new();
    let mut hour = from;
    while hour < to {
        let (start, end) = (hour * 1000, (hour + HOUR) * 1000);
        let mut edges: Vec<(i64, i32)> = Vec::new();
        let mut worked = 0i64;
        let mut started = 0u32;
        for &(run_start, run_end) in &runs {
            if (start..end).contains(&run_start) {
                started += 1;
            }
            let (a, b) = (run_start.max(start), run_end.min(end));
            if a < b {
                worked += b - a;
                edges.push((a, 1));
                edges.push((b, -1));
            }
        }
        // Ends before starts at the same moment: back to back isn't at once.
        edges.sort_unstable();
        let (mut working, mut most) = (0i32, 0i32);
        for (_, change) in edges {
            working += change;
            most = most.max(working);
        }
        let activity = Activity {
            hour,
            agent_minutes: u32::try_from(worked / 60_000).unwrap_or(u32::MAX),
            agents_working: u32::try_from(most).unwrap_or(0),
            runs: started,
        };
        if activity.agent_minutes > 0 || activity.runs > 0 || activity.agents_working > 0 {
            hours.push(activity);
        }
        hour += HOUR;
    }
    Ok(hours)
}

/// The oldest of `periods` that fit one check-in, after `base` (the body
/// without periods); none older than the service takes.
pub(super) fn to_send(periods: &[Period], base: usize, now: i64) -> Vec<Period> {
    let mut size = base;
    let mut sending = Vec::new();
    for period in periods.iter().filter(|period| period.from >= now - OLDEST - DAY) {
        let length = serde_json::to_string(period).map_or(usize::MAX, |json| json.len() + 1);
        if sending.len() >= MOST_PERIODS || size.saturating_add(length) > BODY_BUDGET {
            break;
        }
        size += length;
        sending.push(period.clone());
    }
    sending
}

#[cfg(test)]
mod tests {
    use super::*;

    fn db() -> Connection {
        let db = Connection::open_in_memory().expect("db");
        db.execute_batch(
            "CREATE TABLE runs (id INTEGER PRIMARY KEY, agent TEXT, version TEXT, started INTEGER NOT NULL, ended INTEGER, outcome TEXT);",
        )
        .expect("runs");
        db.execute_batch(SCHEMA).expect("schema");
        db
    }

    #[test]
    fn periods_count_tokens_per_model_and_work_per_hour() {
        let db = db();
        let hour = 1_795_183_200; // a whole hour
        for (h, provider, model, input) in [
            (hour, "anthropic", "claude-opus-4-8", 100),
            (hour, "anthropic", "claude-opus-4-8", 50),
            (hour + HOUR, "openai-codex", "gpt-5.5", 7),
            (hour, "bad name", "x", 1),
        ] {
            db.execute(
                "INSERT INTO model_usage VALUES (?1, ?2, ?3, 1, ?4, 10, 1000, 20)
                 ON CONFLICT (hour, provider, model) DO UPDATE SET requests = requests + 1, input = input + excluded.input",
                params![h, provider, model, input],
            )
            .expect("usage");
        }
        let ms = |seconds: i64| seconds * 1000;
        // Two agents at once for 30 minutes; one run across the hour; one
        // interrupted run that really ended at its last answer.
        db.execute_batch(&format!(
            "INSERT INTO runs (started, ended, outcome) VALUES ({}, {}, 'finished');
             INSERT INTO runs (started, ended, outcome) VALUES ({}, {}, 'finished');
             INSERT INTO runs (started, ended, outcome, last_call) VALUES ({}, {}, 'interrupted', {});",
            ms(hour + 600),
            ms(hour + 2400),
            ms(hour + 1200),
            ms(hour + HOUR + 1800),
            ms(hour + HOUR + 2000),
            ms(hour + 5 * HOUR),
            ms(hour + HOUR + 2600),
        ))
        .expect("runs");

        let periods = new_periods(&db, hour, hour + 2 * HOUR + 59).expect("periods");
        assert_eq!(periods.len(), 1, "one period, up to the last whole hour");
        let period = &periods[0];
        assert_eq!((period.from, period.to), (hour, hour + 2 * HOUR));
        assert!(api::valid_claktory_id(&period.id), "an id the service takes");
        let usage: Vec<(&str, u64, u64)> =
            period.usage.iter().map(|usage| (usage.model.as_str(), usage.requests, usage.input_tokens)).collect();
        assert_eq!(usage, [("claude-opus-4-8", 2, 150), ("gpt-5.5", 1, 7)], "names the service wouldn't take are left out");
        assert_eq!(
            period.activity,
            [
                Activity { hour, agent_minutes: 30 + 40, agents_working: 2, runs: 2 },
                Activity { hour: hour + HOUR, agent_minutes: 30 + 10, agents_working: 1, runs: 1 },
            ]
        );
        for activity in &period.activity {
            assert!(activity.agent_minutes <= activity.agents_working * 60);
        }

        assert!(new_periods(&db, hour + 2 * HOUR, hour + 2 * HOUR + 3000).expect("periods").is_empty(), "no whole hour yet");
        // After two days offline: a period a day.
        let later = new_periods(&db, hour, hour + 2 * DAY + 10).expect("periods");
        assert_eq!(later.iter().map(|period| period.to - period.from).collect::<Vec<_>>(), [DAY, DAY]);
        assert_eq!(to_send(&later, 100, hour + 2 * DAY).len(), 2);
        assert_eq!(to_send(&later, BODY_BUDGET, hour + 2 * DAY).len(), 0, "kept for the next check-in");
    }
}
