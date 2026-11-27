//! Model calls counted: the tokens by hour and model for the
//! account's check-ins (`account::analytics`, `model_usage`), by hour, model,
//! account and agent for the Usage pane (`usage_report`, `agent_usage`), and
//! each run's last call (`runs.last_call`). Kept here and written together,
//! in one transaction, a minute after the first call counted, and before
//! anything reads them, instead of three writes at every call. A crash loses
//! at most that minute's counts, never a conversation.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use rusqlite::{Connection, params};

/// How long counts wait to be written.
pub(crate) const WRITE_AFTER: Duration = Duration::from_secs(60);

/// A call's tokens.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Tokens {
    pub input: i64,
    pub output: i64,
    pub cache_read: i64,
    pub cache_write: i64,
}

impl Tokens {
    fn add(&mut self, other: Tokens) {
        self.input = self.input.saturating_add(other.input);
        self.output = self.output.saturating_add(other.output);
        self.cache_read = self.cache_read.saturating_add(other.cache_read);
        self.cache_write = self.cache_write.saturating_add(other.cache_write);
    }
}

/// `model_usage`'s key: hour (unix seconds), provider, model.
type ModelKey = (i64, String, String);
/// `agent_usage`'s key: hour, provider, model, account, agent.
type AgentKey = (i64, String, String, String, String);

#[derive(Default)]
struct Counted {
    models: BTreeMap<ModelKey, (i64, Tokens)>,
    /// With the agent's name at its latest call.
    agents: BTreeMap<AgentKey, (String, i64, Tokens)>,
    /// Each run's latest call, ms.
    runs: BTreeMap<i64, i64>,
}

impl Counted {
    fn is_empty(&self) -> bool {
        self.models.is_empty() && self.agents.is_empty() && self.runs.is_empty()
    }
}

#[derive(Default)]
pub(crate) struct Calls {
    counted: Mutex<Counted>,
    /// Notified at the first count since the last write.
    pub(crate) first: Arc<tokio::sync::Notify>,
}

impl Calls {
    fn lock(&self) -> std::sync::MutexGuard<'_, Counted> {
        self.counted.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn counting(&self, count: impl FnOnce(&mut Counted)) {
        let first = {
            let mut counted = self.lock();
            let first = counted.is_empty();
            count(&mut counted);
            first
        };
        if first {
            self.first.notify_one();
        }
    }

    /// A call by `provider`/`model` in `hour`, for check-ins.
    pub(crate) fn model(&self, hour: i64, provider: &str, model: &str, tokens: Tokens) {
        self.counting(|counted| {
            let (requests, sum) = counted.models.entry((hour, provider.to_owned(), model.to_owned())).or_default();
            *requests += 1;
            sum.add(tokens);
        });
    }

    /// A call by an agent, for the Usage pane.
    pub(crate) fn agent(&self, key: (i64, &str, &str, &str, &str), name: &str, tokens: Tokens) {
        let (hour, provider, model, account, agent) = key;
        self.counting(|counted| {
            let key = (hour, provider.to_owned(), model.to_owned(), account.to_owned(), agent.to_owned());
            let (kept, requests, sum) = counted.agents.entry(key).or_default();
            name.clone_into(kept);
            *requests += 1;
            sum.add(tokens);
        });
    }

    /// Run `run` had a call at `at` (ms).
    pub(crate) fn last_call(&self, run: i64, at: i64) {
        self.counting(|counted| {
            let latest = counted.runs.entry(run).or_insert(at);
            *latest = (*latest).max(at);
        });
    }

    /// Writes what's counted, in one transaction. What can't be written is
    /// kept for the next time.
    pub(crate) fn write(&self, db: &mut Connection) {
        let counted = std::mem::take(&mut *self.lock());
        if counted.is_empty() {
            return;
        }
        if let Err(error) = write(db, &counted) {
            tracing::warn!(%error, "could not count model calls");
            let mut kept = self.lock();
            let newer = std::mem::replace(&mut *kept, counted);
            merge(&mut kept, newer);
        }
    }
}

fn merge(into: &mut Counted, from: Counted) {
    for (key, (requests, tokens)) in from.models {
        let (kept, sum) = into.models.entry(key).or_default();
        *kept += requests;
        sum.add(tokens);
    }
    for (key, (name, requests, tokens)) in from.agents {
        let (kept_name, kept, sum) = into.agents.entry(key).or_default();
        *kept_name = name;
        *kept += requests;
        sum.add(tokens);
    }
    for (run, at) in from.runs {
        let latest = into.runs.entry(run).or_insert(at);
        *latest = (*latest).max(at);
    }
}

fn write(db: &mut Connection, counted: &Counted) -> rusqlite::Result<()> {
    let transaction = db.transaction()?;
    for ((hour, provider, model), (requests, tokens)) in &counted.models {
        transaction.execute(
            "INSERT INTO model_usage (hour, provider, model, requests, input, output, cache_read, cache_write)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
             ON CONFLICT (hour, provider, model) DO UPDATE SET requests = requests + excluded.requests, input = input + excluded.input,
                 output = output + excluded.output, cache_read = cache_read + excluded.cache_read,
                 cache_write = cache_write + excluded.cache_write",
            params![hour, provider, model, requests, tokens.input, tokens.output, tokens.cache_read, tokens.cache_write],
        )?;
    }
    for ((hour, provider, model, account, agent), (name, requests, tokens)) in &counted.agents {
        transaction.execute(
            "INSERT INTO agent_usage (hour, provider, model, account, agent, agent_name, requests, input, output, cache_read, cache_write)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
             ON CONFLICT (hour, provider, model, account, agent) DO UPDATE SET agent_name = excluded.agent_name,
                 requests = requests + excluded.requests, input = input + excluded.input, output = output + excluded.output,
                 cache_read = cache_read + excluded.cache_read, cache_write = cache_write + excluded.cache_write",
            params![hour, provider, model, account, agent, name, requests, tokens.input, tokens.output, tokens.cache_read, tokens.cache_write],
        )?;
    }
    for (run, at) in &counted.runs {
        transaction.execute("UPDATE runs SET last_call = MAX(COALESCE(last_call, 0), ?2) WHERE id = ?1", params![run, at])?;
    }
    transaction.commit()
}

/// Writes what's counted a minute after the first count since the last
/// write, for as long as Factory runs; nothing counted, it sleeps.
pub(crate) async fn monitor(factory: std::sync::Weak<crate::factory::Factory>) {
    loop {
        let Some(first) = factory.upgrade().map(|factory| factory.store.calls.first.clone()) else { return };
        first.notified().await;
        tokio::time::sleep(WRITE_AFTER).await;
        let Some(factory) = factory.upgrade() else { return };
        factory.store.write_calls();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn db() -> Connection {
        let db = Connection::open_in_memory().unwrap();
        db.execute_batch("CREATE TABLE runs (id INTEGER PRIMARY KEY); INSERT INTO runs (id) VALUES (7);").unwrap();
        db.execute_batch(crate::account::analytics::SCHEMA).unwrap();
        db.execute_batch(crate::usage_report::SCHEMA).unwrap();
        db
    }

    #[test]
    fn calls_are_written_together_as_they_were_counted() {
        let mut db = db();
        let calls = Calls::default();
        let tokens = |n: i64| Tokens { input: n, output: 2 * n, cache_read: 3 * n, cache_write: 4 * n };
        for (n, at) in [(1, 100), (10, 300), (100, 200)] {
            calls.model(3600, "anthropic", "opus", tokens(n));
            calls.agent((3600, "anthropic", "opus", "me", "ana"), &format!("Ana {n}"), tokens(n));
            calls.last_call(7, at);
        }
        calls.model(7200, "anthropic", "opus", tokens(1000));
        calls.write(&mut db);
        let models: Vec<(i64, i64, i64, i64)> = db
            .prepare("SELECT hour, requests, input, cache_write FROM model_usage ORDER BY hour")
            .unwrap()
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        assert_eq!(models, vec![(3600, 3, 111, 444), (7200, 1, 1000, 4000)]);
        let agent: (String, i64, i64) =
            db.query_row("SELECT agent_name, requests, output FROM agent_usage WHERE agent = 'ana'", [], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?))).unwrap();
        assert_eq!(agent, ("Ana 100".to_owned(), 3, 222));
        let last: i64 = db.query_row("SELECT last_call FROM runs WHERE id = 7", [], |row| row.get(0)).unwrap();
        assert_eq!(last, 300);

        // More later adds to the same rows; nothing counted writes nothing.
        calls.model(3600, "anthropic", "opus", tokens(1));
        calls.last_call(7, 250);
        calls.write(&mut db);
        calls.write(&mut db);
        let requests: i64 = db.query_row("SELECT requests FROM model_usage WHERE hour = 3600", [], |row| row.get(0)).unwrap();
        assert_eq!(requests, 4);
        let last: i64 = db.query_row("SELECT last_call FROM runs WHERE id = 7", [], |row| row.get(0)).unwrap();
        assert_eq!(last, 300, "never earlier than written before");
    }
}
