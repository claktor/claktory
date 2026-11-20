//! What agents' model calls used, for the Usage pane: tokens per UTC hour,
//! provider, model, account and agent, kept for good in factory.db
//! (`agent_usage`), and the breakdown of a time range the app asks for
//! (LoadUsage). Check-ins keep their own count (`account::analytics`).

use std::collections::HashMap;

use factory_agent::{AgentEvent, Message};
use factory_protocol::v1;
use rusqlite::{Connection, params};

use crate::factory::{Factory, RequestError};
use crate::transcript::now_ms;

pub(crate) const SCHEMA: &str = "
-- Model calls by UTC hour, provider, model, account and agent, for the
-- Usage pane. Kept for good. Calls from before it have account and agent ''.
CREATE TABLE agent_usage (
    -- Unix seconds, a multiple of 3600.
    hour INTEGER NOT NULL,
    provider TEXT NOT NULL,
    model TEXT NOT NULL,
    -- The provider account that answered; '' when unknown.
    account TEXT NOT NULL,
    agent TEXT NOT NULL,
    -- The agent's name at its latest call in the hour.
    agent_name TEXT NOT NULL,
    requests INTEGER NOT NULL,
    input INTEGER NOT NULL,
    output INTEGER NOT NULL,
    cache_read INTEGER NOT NULL,
    cache_write INTEGER NOT NULL,
    PRIMARY KEY (hour, provider, model, account, agent)
);
INSERT INTO agent_usage (hour, provider, model, account, agent, agent_name, requests, input, output, cache_read, cache_write)
    SELECT hour, provider, model, '', '', '', requests, input, output, cache_read, cache_write FROM model_usage;
";

const HOUR_MS: i64 = 3_600_000;
/// Buckets in a series, at most.
const MOST_BUCKETS: i64 = 5_000;

fn clamp(value: u64) -> i64 {
    i64::try_from(value).unwrap_or(i64::MAX)
}

/// Counts the model call `event` ends for agent `agent` (named `name`): an
/// answer, or a compaction's summary (on `agent_model`, "provider/model").
pub(crate) fn record(db: &Connection, agent: &str, name: &str, agent_model: &str, event: &AgentEvent) {
    let (provider, model, account, usage) = match event {
        AgentEvent::MessageEnd { message: Message::Assistant(answer) } => (
            answer.provider.as_str(),
            answer.model.as_str(),
            factory_agent::accounts::answered_by(answer).unwrap_or_default(),
            &answer.usage,
        ),
        AgentEvent::Compacted { usage, .. } => {
            let (provider, model) = agent_model.split_once('/').unwrap_or(("", agent_model));
            (provider, model, "", usage)
        }
        _ => return,
    };
    let hour = now_ms() / HOUR_MS * 3600;
    let result = db.execute(
        "INSERT INTO agent_usage (hour, provider, model, account, agent, agent_name, requests, input, output, cache_read, cache_write)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, 1, ?7, ?8, ?9, ?10)
         ON CONFLICT (hour, provider, model, account, agent) DO UPDATE SET agent_name = excluded.agent_name,
             requests = requests + 1, input = input + excluded.input, output = output + excluded.output,
             cache_read = cache_read + excluded.cache_read, cache_write = cache_write + excluded.cache_write",
        params![
            hour,
            provider,
            model,
            account,
            agent,
            name,
            clamp(usage.input),
            clamp(usage.output),
            clamp(usage.cache_read),
            clamp(usage.cache_write)
        ],
    );
    if let Err(error) = result {
        tracing::warn!(%error, "could not count a model call for the Usage pane");
    }
}

fn number(row: &rusqlite::Row<'_>, index: usize) -> rusqlite::Result<u64> {
    row.get::<_, i64>(index).map(|value| u64::try_from(value).unwrap_or(0))
}

fn tokens(row: &rusqlite::Row<'_>, from: usize) -> rusqlite::Result<v1::UsageTokens> {
    Ok(v1::UsageTokens {
        input: number(row, from)?,
        output: number(row, from + 1)?,
        cache_read: number(row, from + 2)?,
        cache_write: number(row, from + 3)?,
    })
}

fn add(to: &mut v1::UsageTokens, tokens: &v1::UsageTokens) {
    to.input += tokens.input;
    to.output += tokens.output;
    to.cache_read += tokens.cache_read;
    to.cache_write += tokens.cache_write;
}

/// The breakdown of `request`'s range at `now` (ms); `labels` names
/// accounts by (provider, account id).
fn report(
    db: &Connection,
    request: &v1::LoadUsage,
    now: i64,
    labels: &HashMap<(String, String), String>,
) -> Result<v1::UsageReport, RequestError> {
    let bucket = request.bucket_ms;
    if bucket < 0 || bucket % HOUR_MS != 0 {
        return Err(RequestError::Usage("bucket_ms is a whole number of hours, or 0 for no series".to_owned()));
    }
    let until = if request.until_ms > 0 { request.until_ms } else { now };
    let first: Option<i64> = db.query_row("SELECT MIN(hour) FROM agent_usage", [], |row| row.get(0))?;
    let since = if request.since_ms > 0 { request.since_ms } else { first.map_or(until, |hour| hour * 1000) };
    let since = since.div_euclid(HOUR_MS) * HOUR_MS;
    // An hour counts when it starts in the range.
    let (from, to) = (since.div_euclid(1000), (until + 999).div_euclid(1000));
    let mut report = v1::UsageReport { since_ms: since, until_ms: until, ..v1::UsageReport::default() };
    let names: HashMap<String, String> = {
        let mut statement = db.prepare(
            "SELECT agent, agent_name FROM agent_usage a WHERE agent != '' AND hour = (SELECT MAX(hour) FROM agent_usage b WHERE b.agent = a.agent)",
        )?;
        let rows = statement.query_map([], crate::store::row2)?;
        rows.collect::<rusqlite::Result<_>>()?
    };
    let mut statement = db.prepare(
        "SELECT provider, model, account, agent, SUM(requests), SUM(input), SUM(output), SUM(cache_read), SUM(cache_write)
         FROM agent_usage WHERE hour >= ?1 AND hour < ?2 GROUP BY provider, model, account, agent
         ORDER BY SUM(input) + SUM(output) + SUM(cache_read) + SUM(cache_write) DESC",
    )?;
    let rows = statement.query_map([from, to], |row| {
        let (provider_id, model, account_id, agent_id): (String, String, String, String) =
            (row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?);
        Ok(v1::UsageRow {
            account_label: labels.get(&(provider_id.clone(), account_id.clone())).cloned().unwrap_or_default(),
            agent_name: names.get(&agent_id).cloned().unwrap_or_default(),
            provider_id,
            model,
            account_id,
            agent_id,
            requests: number(row, 4)?,
            tokens: Some(tokens(row, 5)?),
        })
    })?;
    report.rows = rows.collect::<rusqlite::Result<_>>()?;
    report.agents_since_ms = db
        .query_row("SELECT MIN(hour) FROM agent_usage WHERE agent != ''", [], |row| row.get::<_, Option<i64>>(0))?
        .map_or(0, |hour| hour * 1000);
    if bucket > 0 {
        // Day buckets (and longer) start at the person's midnight.
        let offset = if bucket % (24 * HOUR_MS) == 0 { i64::from(request.utc_offset_minutes) * 60_000 } else { 0 };
        let start = (since + offset).div_euclid(bucket) * bucket - offset;
        let count = (until - start + bucket - 1).div_euclid(bucket).max(0);
        if count > MOST_BUCKETS {
            return Err(RequestError::Usage(format!("that's {count} buckets; at most {MOST_BUCKETS}")));
        }
        report.series = (0..count)
            .map(|index| v1::UsageBucket { start_ms: start + index * bucket, tokens: Some(v1::UsageTokens::default()) })
            .collect();
        let mut statement = db.prepare(
            "SELECT hour, SUM(input), SUM(output), SUM(cache_read), SUM(cache_write) FROM agent_usage
             WHERE hour >= ?1 AND hour < ?2 GROUP BY hour",
        )?;
        let hours = statement.query_map([from, to], |row| Ok((row.get::<_, i64>(0)?, tokens(row, 1)?)))?;
        for hour in hours {
            let (hour, tokens) = hour?;
            let index = (hour * 1000 - start).div_euclid(bucket);
            if let Some(tokens_in) =
                usize::try_from(index).ok().and_then(|index| report.series.get_mut(index)).and_then(|bucket| bucket.tokens.as_mut())
            {
                add(tokens_in, &tokens);
            }
        }
    }
    Ok(report)
}

impl Factory {
    /// LoadUsage: what agents' model calls used in a time range.
    pub(crate) fn load_usage(&self, request: &v1::LoadUsage) -> Result<v1::UsageReport, RequestError> {
        let labels: HashMap<(String, String), String> = self
            .lock()
            .providers
            .iter()
            .flat_map(|provider| provider.accounts.iter().map(|account| ((provider.id.clone(), account.id.clone()), account.label.clone())))
            .collect();
        report(&self.store.db(), request, now_ms(), &labels)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn db() -> Connection {
        let db = Connection::open_in_memory().expect("db");
        db.execute_batch(
            "CREATE TABLE model_usage (hour INTEGER NOT NULL, provider TEXT NOT NULL, model TEXT NOT NULL, requests INTEGER NOT NULL,
                 input INTEGER NOT NULL, output INTEGER NOT NULL, cache_read INTEGER NOT NULL, cache_write INTEGER NOT NULL,
                 PRIMARY KEY (hour, provider, model));
             INSERT INTO model_usage VALUES (0, 'anthropic', 'claude-opus-4-8', 2, 100, 10, 1000, 20);",
        )
        .expect("model_usage");
        db.execute_batch(SCHEMA).expect("schema");
        db
    }

    fn put(db: &Connection, hour: i64, account: &str, agent: &str, name: &str, input: i64) {
        db.execute(
            "INSERT INTO agent_usage VALUES (?1, 'anthropic', 'claude-opus-4-8', ?2, ?3, ?4, 1, ?5, 1, 2, 3)",
            params![hour, account, agent, name, input],
        )
        .expect("insert");
    }

    #[test]
    fn a_range_is_broken_down_and_its_series_adds_up() {
        let db = db();
        let day = 24 * 3600;
        put(&db, day, "a1", "june", "Jun", 5);
        put(&db, day + 3600, "a1", "june", "June", 7);
        put(&db, 2 * day + 3600, "a2", "omar", "Omar", 11);
        let labels = HashMap::from([(("anthropic".to_owned(), "a1".to_owned()), "june@example.com".to_owned())]);
        let ask = |since_ms, until_ms, bucket_ms, utc_offset_minutes| v1::LoadUsage { since_ms, until_ms, bucket_ms, utc_offset_minutes };

        // All time, by day, from the history copied over at the first hour.
        let all = report(&db, &ask(0, 0, 24 * HOUR_MS, 0), 3 * day * 1000, &labels).expect("report");
        assert_eq!((all.since_ms, all.agents_since_ms), (0, day * 1000));
        let rows: Vec<(&str, &str, &str, &str, u64, u64)> = all
            .rows
            .iter()
            .map(|row| {
                let tokens = row.tokens.unwrap_or_default();
                (
                    row.agent_id.as_str(),
                    row.agent_name.as_str(),
                    row.account_id.as_str(),
                    row.account_label.as_str(),
                    row.requests,
                    tokens.input,
                )
            })
            .collect();
        assert_eq!(rows, [("", "", "", "", 2, 100), ("june", "June", "a1", "june@example.com", 2, 12), ("omar", "Omar", "a2", "", 1, 11)]);
        let inputs: Vec<u64> = all.series.iter().map(|bucket| bucket.tokens.unwrap_or_default().input).collect();
        assert_eq!(inputs, [100, 12, 11], "every day, empty ones too");

        // One day, by hour, in a time zone 2 hours east: its days start at 22:00 UTC.
        let one = report(&db, &ask(day * 1000, 2 * day * 1000, HOUR_MS, 120), 3 * day * 1000, &labels).expect("report");
        assert_eq!(one.series.len(), 24);
        assert_eq!(one.series[1].tokens.unwrap_or_default().input, 7);
        assert_eq!(one.rows.len(), 1);
        let local = report(&db, &ask(day * 1000, 3 * day * 1000, 24 * HOUR_MS, 120), 3 * day * 1000, &labels).expect("report");
        assert_eq!(local.series.first().map(|bucket| bucket.start_ms), Some((day - 2 * 3600) * 1000));

        assert!(report(&db, &ask(0, 0, 1000, 0), 3 * day * 1000, &labels).is_err(), "not whole hours");
        assert!(report(&db, &ask(0, 0, HOUR_MS, 0), 365 * day * 1000, &labels).is_err(), "too many buckets");
    }
}
