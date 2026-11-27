//! A liaison's daily model spend (pipes-automation §7, H4; in dollars). An
//! agent quarantined by a pipe talks with someone outside the person's
//! Claktory, who decides how often it's woken: so what it spends a day is
//! capped. The spend is estimated from what its model calls used today
//! (`agent_usage`, since local midnight) at its models' prices; calls on a
//! model with no price count in tokens instead, against a cap of their own.
//! Past either cap its pipes' messages wait, untold, rather than wake it; at
//! local midnight, or when the person raises its cap, they're delivered as
//! usual. The person's own messages always reach it. The app shows the
//! spend, the caps and how many messages wait (Agent.spend): a quiet count,
//! never a card.
//!
//! The caps are the person's (SetSpendCap), kept in factory.db, not
//! config.json, beside the pipes they're about.

use std::sync::{Arc, Weak};
use std::time::Duration;

use factory_protocol::v1;
use rusqlite::{Connection, OptionalExtension, params};

use super::{MessageState, State};
use crate::factory::{Factory, RequestError};
use crate::transcript::now_ms;

pub(crate) const SCHEMA: &str = "
-- The person's daily caps on a liaison's model spend (pipes-automation H4),
-- where they aren't the defaults: dollars at its models' prices, and tokens
-- on models with no price.
CREATE TABLE pipe_spend_caps (
    agent TEXT PRIMARY KEY,
    usd REAL NOT NULL,
    tokens INTEGER NOT NULL
);
";

/// A liaison's daily cap in dollars, unless the person set another.
pub(crate) const DEFAULT_USD: f64 = 10.0;
/// Its daily cap in tokens on models with no price (input, output and
/// cache writes; cache reads, re-reading what it already has, don't count).
pub(crate) const DEFAULT_TOKENS: u64 = 1_000_000;

/// What a liaison spent today, and its caps.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Spend {
    /// Estimated, at its models' prices.
    pub usd: f64,
    /// On models with no price.
    pub tokens: u64,
    pub cap_usd: f64,
    pub cap_tokens: u64,
    /// The next local midnight, ms since 1970: when today's spend is over.
    pub resets_ms: i64,
}

impl Spend {
    pub(crate) fn over(&self) -> bool {
        self.usd >= self.cap_usd || self.tokens >= self.cap_tokens
    }
}

/// Today's local midnight and the next, ms since 1970, at `now` (ms).
fn day(db: &Connection, now: i64) -> rusqlite::Result<(i64, i64)> {
    db.query_row(
        "SELECT CAST(strftime('%s', ?1 / 1000, 'unixepoch', 'localtime', 'start of day', 'utc') AS INTEGER) * 1000,
                CAST(strftime('%s', ?1 / 1000, 'unixepoch', 'localtime', 'start of day', '+1 day', 'utc') AS INTEGER) * 1000",
        [now],
        crate::store::row2,
    )
}

/// The person's caps for `agent`: theirs, or the defaults.
pub(crate) fn caps(db: &Connection, agent: &str) -> rusqlite::Result<(f64, u64)> {
    let set: Option<(f64, i64)> = db
        .query_row("SELECT usd, tokens FROM pipe_spend_caps WHERE agent = ?1", [agent], crate::store::row2)
        .optional()?;
    Ok(set.map_or((DEFAULT_USD, DEFAULT_TOKENS), |(usd, tokens)| (usd, u64::try_from(tokens).unwrap_or(DEFAULT_TOKENS))))
}

/// What `agent` spent today at `now` (ms), and its caps. A model's price is
/// per million tokens; usage is counted by UTC hour, so on a half-hour
/// time zone the hour that holds midnight counts whole.
pub(crate) fn spent(factory: &Factory, agent: &str, now: i64) -> rusqlite::Result<Spend> {
    factory.store.write_calls();
    let (rows, (cap_usd, cap_tokens), resets_ms) = {
        let db = factory.store.db();
        let (midnight, next) = day(&db, now)?;
        let rows = crate::store::query(
            &db,
            "SELECT provider, model, SUM(input), SUM(output), SUM(cache_read), SUM(cache_write) FROM agent_usage
             WHERE agent = ?1 AND hour >= ?2 GROUP BY provider, model",
            params![agent, midnight / 1000 / 3600 * 3600],
            |row| {
                let count = |index| row.get::<_, i64>(index).map(|value| u64::try_from(value).unwrap_or(0));
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?, [count(2)?, count(3)?, count(4)?, count(5)?]))
            },
        )?;
        (rows, caps(&db, agent)?, next)
    };
    let mut spend = Spend { cap_usd, cap_tokens, resets_ms, ..Spend::default() };
    for (provider, model, [input, output, cache_read, cache_write]) in rows {
        let cost = factory.models.resolve(&format!("{provider}/{model}")).map(|model| model.cost);
        match cost.filter(|cost| cost.input > 0.0 || cost.output > 0.0) {
            Some(cost) => {
                spend.usd += (cost.input * input as f64
                    + cost.output * output as f64
                    + cost.cache_read * cache_read as f64
                    + cost.cache_write * cache_write as f64)
                    / 1_000_000.0;
            }
            None => spend.tokens += input + output + cache_write,
        }
    }
    Ok(spend)
}

/// What a liaison spent today, for the app; None for any other agent.
pub(crate) fn of(factory: &Factory, agent: &str) -> Option<Spend> {
    if !factory.pipes.quarantined(agent) {
        return None;
    }
    spent(factory, agent, now_ms()).inspect_err(|error| tracing::error!(%agent, %error, "could not work out a liaison's spend")).ok()
}

/// Whether `agent`, woken for a pipe's messages, waits instead: a liaison
/// past its cap for today.
pub(crate) fn holds(factory: &Factory, agent: &str) -> bool {
    of(factory, agent).is_some_and(|spend| spend.over())
}

/// The pipe messages waiting for `agent` because it's past its cap: the
/// incoming ones on the open pipes it leads that it hasn't been told of.
pub(crate) fn held(factory: &Factory, agent: &str) -> usize {
    factory
        .pipes
        .all()
        .into_iter()
        .filter(|record| record.lead == agent && record.state == State::Open)
        .map(|record| {
            super::messages(factory, &record.id, 0, 1000)
                .0
                .iter()
                .filter(|message| {
                    (message.seq > record.told || record.untold.contains(&message.seq))
                        && message.author.incoming()
                        && message.state == MessageState::Received
                })
                .count()
        })
        .sum()
}

/// The person sets an agent's caps (SetSpendCap); 0 keeps a default. Its
/// held messages go to it now if it's under them.
pub(crate) fn set_caps(factory: &Arc<Factory>, request: &v1::SetSpendCap) -> Result<(), RequestError> {
    factory.known_agent(&request.agent_id)?;
    if !request.cap_usd.is_finite() || request.cap_usd < 0.0 {
        return Err(RequestError::Pipe("A spend cap is a number of dollars, 0 or more.".to_owned()));
    }
    let usd = if request.cap_usd > 0.0 { request.cap_usd } else { DEFAULT_USD };
    let tokens = if request.cap_tokens > 0 { request.cap_tokens } else { DEFAULT_TOKENS };
    factory
        .store
        .db()
        .execute(
            "INSERT INTO pipe_spend_caps (agent, usd, tokens) VALUES (?1, ?2, ?3)
             ON CONFLICT (agent) DO UPDATE SET usd = excluded.usd, tokens = excluded.tokens",
            params![request.agent_id, usd, i64::try_from(tokens).unwrap_or(i64::MAX)],
        )
        .map_err(|error| RequestError::Pipe(format!("The cap couldn't be kept: {error}")))?;
    release(factory);
    factory.broadcast_workspace(&factory.lock());
    Ok(())
}

/// The app's view of a liaison's day; None for any other agent.
pub(crate) fn proto(factory: &Factory, agent: &str) -> Option<v1::ContainedSpend> {
    let spend = of(factory, agent)?;
    let over = spend.over();
    Some(v1::ContainedSpend {
        spent_usd: spend.usd,
        cap_usd: spend.cap_usd,
        spent_tokens: spend.tokens,
        cap_tokens: spend.cap_tokens,
        over,
        held_messages: if over { u32::try_from(held(factory, agent)).unwrap_or(u32::MAX) } else { 0 },
        resumes_ms: spend.resets_ms,
    })
}

/// Wakes each liaison under its cap for the messages held for it.
pub(crate) fn release(factory: &Arc<Factory>) {
    for record in factory.pipes.all() {
        if record.state == State::Open && factory.pipes.quarantined(&record.lead) && !holds(factory, &record.lead) {
            super::agents::wake(factory, &record.id);
        }
    }
}

/// From start: what waited through a restart goes now if it may, and every
/// local midnight after, what waited for the new day.
pub(crate) async fn monitor(factory: Weak<Factory>) {
    let Some(strong) = factory.upgrade() else { return };
    release(&strong);
    drop(strong);
    loop {
        let next = {
            let Some(factory) = factory.upgrade() else { return };
            let now = now_ms();
            let next = day(&factory.store.db(), now).map_or(now + 3_600_000, |(_, next)| next);
            next - now
        };
        // A second past midnight, so the new day has begun.
        tokio::time::sleep(Duration::from_millis(u64::try_from(next).unwrap_or(0)) + Duration::from_secs(1)).await;
        let Some(factory) = factory.upgrade() else { return };
        release(&factory);
        factory.broadcast_workspace(&factory.lock());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_day_runs_from_local_midnight_to_the_next() {
        let db = Connection::open_in_memory().expect("db");
        let now = 1_795_347_117_055;
        let (midnight, next) = day(&db, now).expect("day");
        assert!(midnight <= now && now < next, "{midnight} {now} {next}");
        // 23, 24 or 25 hours, across a change of clocks.
        assert!((23 * 3_600_000..=25 * 3_600_000).contains(&(next - midnight)), "{}", next - midnight);
        assert_eq!(day(&db, next).expect("day").0, next);
    }
}
