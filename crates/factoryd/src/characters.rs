//! Agents' characters: who each one looks like (its shape and seed; the
//! color is the agent's, and the eyes are the same for every agent)
//! and what it shows now (its mood). The app only draws them; Factory
//! decides both, so every device agrees.
//!
//! A mood comes from what Factory knows of the agent: whether it's running
//! and running a tool, whether its model can run, its presence (when it last
//! became idle, and whether its last run failed, kept in `agent_presence`),
//! and whether it has something unseen. One sweep (`monitor`) turns idle
//! agents asleep when their time comes.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

use factory_protocol::v1;
use rusqlite::{Connection, OptionalExtension, params};

use crate::config::AgentConfig;
use crate::factory::{Factory, Inner};
use crate::transcript::now_ms;

/// Body shapes, in catalog order: an automatic shape is `seed % 6`.
pub(crate) const SHAPES: [&str; 6] = ["circle", "square", "triangle", "diamond", "pentagon", "hexagon"];
/// An idle agent falls asleep after this long.
pub(crate) const SLEEP_AFTER: Duration = Duration::from_secs(15 * 60);
/// The sweep looks at least this often.
const SWEEP: Duration = Duration::from_secs(30);

pub(crate) const SCHEMA: &str = "
-- What agents' characters show that outlasts a run: when each agent last
-- became idle (a run ended, /clear, or it was first seen), and whether its
-- last run failed.
CREATE TABLE agent_presence (
    agent TEXT PRIMARY KEY,
    idle_since INTEGER NOT NULL,
    failed INTEGER NOT NULL DEFAULT 0
);
";

/// 32-bit FNV-1a of the agent's id: the same everywhere and for good.
pub(crate) fn seed(id: &str) -> u32 {
    id.bytes().fold(0x811c_9dc5_u32, |hash, byte| (hash ^ u32::from(byte)).wrapping_mul(0x0100_0193))
}

/// What the agent's character looks like.
pub(crate) struct Identity {
    pub shape: &'static str,
    pub seed: u32,
}

/// Its chosen shape, or else (and for one this Factory doesn't know, such
/// as the organic shapes of before) the one its id gives it.
pub(crate) fn identity(agent: &AgentConfig) -> Identity {
    let seed = seed(&agent.id);
    let chosen = |catalog: &[&'static str], choice: &str| catalog.iter().copied().find(|known| *known == choice);
    Identity { shape: chosen(&SHAPES, &agent.avatar.shape).unwrap_or(SHAPES[seed as usize % SHAPES.len()]), seed }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Mood {
    Idle,
    Thinking,
    Working,
    News,
    Failed,
    NoModel,
    Asleep,
}

impl Mood {
    pub(crate) fn proto(self) -> v1::CharacterMood {
        match self {
            Mood::Idle => v1::CharacterMood::Idle,
            Mood::Thinking => v1::CharacterMood::Thinking,
            Mood::Working => v1::CharacterMood::Working,
            Mood::News => v1::CharacterMood::News,
            Mood::Failed => v1::CharacterMood::Failed,
            Mood::NoModel => v1::CharacterMood::NoModel,
            Mood::Asleep => v1::CharacterMood::Asleep,
        }
    }
}

/// What outlasts a run, kept in `agent_presence`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Presence {
    /// When it last became idle, in ms since 1970.
    pub idle_since: i64,
    /// Its last run failed.
    pub failed: bool,
}

/// What an agent's mood is decided from.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Facts {
    pub running: bool,
    /// A tool of its (not a background task) is running.
    pub tool: bool,
    pub model_available: bool,
    pub unseen: bool,
    pub presence: Presence,
}

/// The first that applies: running (working with a tool, thinking without),
/// no model, failed, news, asleep, idle.
pub(crate) fn mood(facts: Facts, now: i64, sleep_after: Duration) -> Mood {
    if facts.running {
        if facts.tool { Mood::Working } else { Mood::Thinking }
    } else if !facts.model_available {
        Mood::NoModel
    } else if facts.presence.failed {
        Mood::Failed
    } else if facts.unseen {
        Mood::News
    } else if now >= asleep_at(facts.presence, sleep_after) {
        Mood::Asleep
    } else {
        Mood::Idle
    }
}

fn asleep_at(presence: Presence, sleep_after: Duration) -> i64 {
    presence.idle_since.saturating_add(i64::try_from(sleep_after.as_millis()).unwrap_or(i64::MAX))
}

/// What each agent's character was last shown with: its mood and since
/// when. A view shows the same `since` until the mood changes, so a new
/// connection doesn't look like a change.
#[derive(Default)]
pub(crate) struct Shown(Mutex<HashMap<String, (Mood, i64)>>);

impl Shown {
    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<String, (Mood, i64)>> {
        self.0.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Since when the agent's mood has been `mood`, recording it if new. A
    /// mood found in place (Factory just started) dates from when it began,
    /// where that's known: asleep from when it fell asleep, idle, failed and
    /// news from when it became idle.
    fn since(&self, id: &str, mood: Mood, presence: Presence, now: i64, sleep_after: Duration) -> i64 {
        let mut shown = self.lock();
        match shown.get(id) {
            Some((was, since)) if *was == mood => *since,
            found => {
                let since = match mood {
                    Mood::Asleep => asleep_at(presence, sleep_after),
                    Mood::Idle | Mood::Failed | Mood::News if found.is_none() => presence.idle_since,
                    _ => now,
                };
                shown.insert(id.to_owned(), (mood, since));
                since
            }
        }
    }

    fn differs(&self, id: &str, mood: Mood) -> bool {
        self.lock().get(id).is_none_or(|(was, _)| *was != mood)
    }

    fn forget(&self, id: &str) {
        self.lock().remove(id);
    }
}

/// Every agent's presence, cached from the store.
#[derive(Default)]
pub(crate) struct Presences(Mutex<HashMap<String, Presence>>);

impl Presences {
    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<String, Presence>> {
        self.0.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    pub(crate) fn get(&self, id: &str) -> Option<Presence> {
        self.lock().get(id).copied()
    }
}

/// What Factory keeps for agents' characters.
pub(crate) struct State {
    /// Agents' presence, as recorded.
    presences: Presences,
    /// The mood each agent's character was last shown with, and since when.
    shown: Shown,
    /// How long an idle agent takes to fall asleep, in ms.
    sleep_after: std::sync::atomic::AtomicU64,
    /// Wakes the sweep to look again now.
    wake: Arc<tokio::sync::Notify>,
}

impl State {
    pub(crate) fn new(presences: Presences) -> State {
        State {
            presences,
            shown: Shown::default(),
            sleep_after: std::sync::atomic::AtomicU64::new(u64::try_from(SLEEP_AFTER.as_millis()).unwrap_or(u64::MAX)),
            wake: Arc::default(),
        }
    }
}

/// The agent's character fields of its `v1::Agent`.
pub(crate) struct View {
    pub shape: String,
    pub seed: u32,
    pub mood: v1::CharacterMood,
    pub since: i64,
    pub shape_override: Option<String>,
}

impl Factory {
    fn facts(&self, inner: &Inner, agent: &AgentConfig, now: i64) -> Facts {
        let runtime = inner.agents.get(&agent.id);
        Facts {
            running: runtime.is_some_and(|runtime| runtime.running),
            tool: runtime.is_some_and(|runtime| runtime.doing.is_some()),
            model_available: inner.models.iter().any(|model| model.id == agent.model),
            unseen: inner.state.unseen.contains(&agent.id),
            // One not seen yet (just added) became idle now.
            presence: self.characters.presences.get(&agent.id).unwrap_or(Presence { idle_since: now, failed: false }),
        }
    }

    /// The agent's character as it is now, for a view.
    pub(crate) fn character(&self, inner: &Inner, agent: &AgentConfig, now: i64) -> View {
        let identity = identity(agent);
        let facts = self.facts(inner, agent, now);
        let sleep_after = self.sleep_after();
        let mood = mood(facts, now, sleep_after);
        let chosen = |choice: &str| (!choice.is_empty()).then(|| choice.to_owned());
        View {
            shape: identity.shape.to_owned(),
            seed: identity.seed,
            mood: mood.proto(),
            since: self.characters.shown.since(&agent.id, mood, facts.presence, now, sleep_after),
            shape_override: chosen(&agent.avatar.shape),
        }
    }

    /// Whether any agent's mood isn't what it was last shown with, and
    /// how long until an idle one falls asleep.
    fn moods_changed(&self, inner: &Inner, now: i64) -> (bool, Option<Duration>) {
        let sleep_after = self.sleep_after();
        let mut changed = false;
        let mut next: Option<i64> = None;
        for agent in &inner.config.agents {
            let facts = self.facts(inner, agent, now);
            let mood = mood(facts, now, sleep_after);
            changed |= self.characters.shown.differs(&agent.id, mood);
            if mood == Mood::Idle {
                let at = asleep_at(facts.presence, sleep_after);
                next = Some(next.map_or(at, |next| next.min(at)));
            }
        }
        (changed, next.map(|at| Duration::from_millis(u64::try_from(at - now).unwrap_or(0))))
    }

    /// Sets how long an idle agent takes to fall asleep (for tests).
    pub fn set_sleep_after(&self, after: Duration) {
        self.characters.sleep_after.store(u64::try_from(after.as_millis()).unwrap_or(u64::MAX), std::sync::atomic::Ordering::Relaxed);
        self.characters.wake.notify_one();
    }

    pub(crate) fn sleep_after(&self) -> Duration {
        Duration::from_millis(self.characters.sleep_after.load(std::sync::atomic::Ordering::Relaxed))
    }

    /// Every agent in the config has a presence: one first seen now gets
    /// it from its last run that ended, or else from now.
    pub(crate) fn add_presences(&self, agents: &[AgentConfig]) {
        let missing: Vec<&str> =
            agents.iter().map(|agent| agent.id.as_str()).filter(|id| self.characters.presences.get(id).is_none()).collect();
        if missing.is_empty() {
            return;
        }
        let now = now_ms();
        let db = self.store.db();
        for id in missing {
            match add_presence(&db, id, now) {
                Ok(presence) => {
                    self.characters.presences.lock().insert(id.to_owned(), presence);
                }
                Err(error) => tracing::error!(agent = %id, %error, "could not record the agent's presence"),
            }
        }
    }

    /// A run of the agent ended (`store::end_run` saved it).
    pub(crate) fn ran(&self, id: &str, presence: Presence) {
        self.characters.presences.lock().insert(id.to_owned(), presence);
        self.characters.wake.notify_one();
    }

    /// The agent failed without a run to record it in (it couldn't even load).
    pub(crate) fn failed_now(&self, id: &str) {
        let presence = Presence { idle_since: now_ms(), failed: true };
        if let Err(error) = set_presence(&self.store.db(), id, presence) {
            tracing::error!(agent = %id, %error, "could not record the agent's presence");
        }
        self.characters.presences.lock().insert(id.to_owned(), presence);
        self.characters.wake.notify_one();
    }

    /// /clear: the agent starts over idle, with no failure.
    pub(crate) fn reset_presence(&self, id: &str) {
        let presence = Presence { idle_since: now_ms(), failed: false };
        if let Err(error) = set_presence(&self.store.db(), id, presence) {
            tracing::error!(agent = %id, %error, "could not record the agent's presence");
        }
        self.characters.presences.lock().insert(id.to_owned(), presence);
        self.characters.wake.notify_one();
    }

    /// The agent is deleted.
    pub(crate) fn forget_presence(&self, id: &str) -> rusqlite::Result<()> {
        self.store.db().execute("DELETE FROM agent_presence WHERE agent = ?1", [id])?;
        self.characters.presences.lock().remove(id);
        self.characters.shown.forget(id);
        Ok(())
    }
}

/// Every presence recorded.
pub(crate) fn presences(db: &Connection) -> rusqlite::Result<Presences> {
    let mut statement = db.prepare("SELECT agent, idle_since, failed FROM agent_presence")?;
    let rows = statement.query_map([], |row| Ok((row.get::<_, String>(0)?, Presence { idle_since: row.get(1)?, failed: row.get(2)? })))?;
    Ok(Presences(Mutex::new(rows.collect::<rusqlite::Result<_>>()?)))
}

/// Records the agent's presence unless it has one: from its last run that
/// ended (by run, not by time), or else from `now`. Returns what's recorded.
fn add_presence(db: &Connection, agent: &str, now: i64) -> rusqlite::Result<Presence> {
    let last: Option<(i64, Option<String>)> = db
        .query_row("SELECT ended, outcome FROM runs WHERE agent = ?1 AND ended IS NOT NULL ORDER BY id DESC LIMIT 1", [agent], |row| {
            Ok((row.get(0)?, row.get(1)?))
        })
        .optional()?;
    let (idle_since, failed) = last.map_or((now, false), |(ended, outcome)| (ended, outcome.as_deref() == Some("failed")));
    db.execute("INSERT OR IGNORE INTO agent_presence (agent, idle_since, failed) VALUES (?1, ?2, ?3)", params![agent, idle_since, failed])?;
    db.query_row("SELECT idle_since, failed FROM agent_presence WHERE agent = ?1", [agent], |row| {
        Ok(Presence { idle_since: row.get(0)?, failed: row.get(1)? })
    })
}

pub(crate) fn set_presence(db: &Connection, agent: &str, presence: Presence) -> rusqlite::Result<()> {
    db.execute(
        "INSERT INTO agent_presence (agent, idle_since, failed) VALUES (?1, ?2, ?3)
         ON CONFLICT (agent) DO UPDATE SET idle_since = excluded.idle_since, failed = excluded.failed",
        params![agent, presence.idle_since, presence.failed],
    )?;
    Ok(())
}

/// Turns idle agents asleep when their time comes: one sweep for every
/// agent, waking when the next one is due, or every `SWEEP` at most. Moods
/// that changed some other way unnoticed are sent too.
pub(crate) async fn monitor(factory: Weak<Factory>) {
    loop {
        let Some(strong) = factory.upgrade() else { return };
        let wait = sweep(&strong).min(SWEEP).min(strong.sleep_after()).max(Duration::from_millis(10));
        let wake = strong.characters.wake.clone();
        drop(strong);
        tokio::select! {
            () = tokio::time::sleep(wait) => {}
            () = wake.notified() => {}
        }
    }
}

fn sweep(factory: &Arc<Factory>) -> Duration {
    let inner = factory.lock();
    let (changed, next) = factory.moods_changed(&inner, now_ms());
    if changed {
        factory.broadcast_workspace(&inner);
    }
    next.unwrap_or(SWEEP)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_seed_is_fnv_1a_of_the_id() {
        // Known 32-bit FNV-1a values.
        assert_eq!(seed(""), 0x811c_9dc5);
        assert_eq!(seed("a"), 0xe40c_292c);
        assert_eq!(seed("foobar"), 0xbf9c_f968);
    }

    #[test]
    fn moods_go_by_precedence() {
        let now = 100 * 60_000;
        let sleep = SLEEP_AFTER;
        let idle = Presence { idle_since: now - 60_000, failed: false };
        let long_idle = Presence { idle_since: now - 20 * 60_000, failed: false };
        let failed = Presence { failed: true, ..idle };
        let facts = |running, tool, model_available, unseen, presence| Facts { running, tool, model_available, unseen, presence };
        for (facts, expected) in [
            (facts(false, false, true, false, idle), Mood::Idle),
            (facts(true, false, true, false, idle), Mood::Thinking),
            (facts(true, true, true, false, idle), Mood::Working),
            // Running wins over an old failure, no model, news and sleep.
            (facts(true, false, false, true, failed), Mood::Thinking),
            (facts(false, false, false, false, failed), Mood::NoModel),
            (facts(false, false, true, true, failed), Mood::Failed),
            (facts(false, false, true, false, failed), Mood::Failed),
            (facts(false, false, true, true, idle), Mood::News),
            // Unread news doesn't sleep.
            (facts(false, false, true, true, long_idle), Mood::News),
            (facts(false, false, true, false, long_idle), Mood::Asleep),
            (facts(false, false, false, false, long_idle), Mood::NoModel),
        ] {
            assert_eq!(mood(facts, now, sleep), expected, "{facts:?}");
        }
        // Asleep at 15:00 idle, not at 14:59.
        let at = |idle_for: i64| mood(facts(false, false, true, false, Presence { idle_since: now - idle_for, failed: false }), now, sleep);
        assert_eq!((at(14 * 60_000 + 59_000), at(15 * 60_000)), (Mood::Idle, Mood::Asleep));
    }

    #[test]
    fn a_mood_keeps_its_since_until_it_changes() {
        let shown = Shown::default();
        let presence = Presence { idle_since: 1_000, failed: false };
        // Found in place: from when it became idle; asleep from when it fell asleep.
        assert_eq!(shown.since("a", Mood::Idle, presence, 5_000, SLEEP_AFTER), 1_000);
        assert_eq!(shown.since("b", Mood::Asleep, presence, 5_000_000, SLEEP_AFTER), 1_000 + 15 * 60_000);
        assert_eq!(shown.since("a", Mood::Idle, presence, 9_000, SLEEP_AFTER), 1_000);
        // A change dates from now.
        assert_eq!(shown.since("a", Mood::Thinking, presence, 9_000, SLEEP_AFTER), 9_000);
        assert_eq!(shown.since("a", Mood::Thinking, presence, 12_000, SLEEP_AFTER), 9_000);
        assert!(shown.differs("a", Mood::Idle) && !shown.differs("a", Mood::Thinking));
    }
}
