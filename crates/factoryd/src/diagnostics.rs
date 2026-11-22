//! Advanced diagnostics: with them on (SetDiagnostics, off by default),
//! the person's devices record what happens in areas Claktory knows trouble
//! in and send it to this Claktory (SendDiagnostics), the backend records
//! its own side (`record`), and this Claktory's agents read it in
//! factory.db with database:factory, to find out what went wrong. The first
//! area is "connection": app launches, network changes, connect attempts
//! and how they went. A new area needs no new table or message.
//!
//! The setting is kept in factory.db, not config.json, so a backend from
//! before it still reads the settings after a rollback. Only a device let
//! in (this computer's own app, or a paired device) can send: who it is,
//! its kind, system and app version come from its connection, never from
//! what it sends. Each source keeps 14 days or 4 MB, oldest out first;
//! turning diagnostics off deletes everything.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock, Weak};
use std::time::Duration;

use factory_protocol::v1;
use rusqlite::{Connection, OptionalExtension, Transaction, params};
use tokio::sync::mpsc;

use crate::factory::{Factory, RequestError};
use crate::transcript::now_ms;

/// Where the first, connection-only version's tables were made (never
/// released): a database that has them gets them dropped by `SCHEMA`.
pub(crate) const CONNECTION: &str = "
-- Connection diagnostics, before diagnostics were general: the next
-- migration drops their tables where an early build made them.
";

pub(crate) const SCHEMA: &str = "
DROP TABLE IF EXISTS connection_diagnostics;
DROP TABLE IF EXISTS connection_diagnostics_devices;
DROP TABLE IF EXISTS diagnostics_settings;
-- Settings of advanced diagnostics (crate::diagnostics): share is '1'
-- while they're on; no row, off.
CREATE TABLE diagnostics_settings (name TEXT PRIMARY KEY, value TEXT NOT NULL);
-- What happened, as the person's devices and this Claktory recorded it
-- while diagnostics were on, by area (connection…).
CREATE TABLE diagnostics (
    id INTEGER PRIMARY KEY,
    -- device:<paired device id> (device:local, this computer's own app), or
    -- backend: this Claktory itself.
    source TEXT NOT NULL,
    -- What it's about: connection…
    area TEXT NOT NULL,
    -- Goes up every time the app's (or backend's) process starts.
    session INTEGER NOT NULL,
    -- Goes up with every event of the app's install (or backend's process).
    seq INTEGER NOT NULL,
    -- When, by the source's clock, in ms since 1970.
    at INTEGER NOT NULL,
    -- For connection: lifecycle, network, connect, connected, welcome,
    -- disconnected, relay, error…
    kind TEXT NOT NULL,
    -- key=value tokens: path=relay ms=812 err=timeout net=wifi
    detail TEXT NOT NULL,
    -- How long it took, in ms; 0 when it doesn't apply.
    duration INTEGER NOT NULL,
    -- The version of the app (or backend) that recorded it.
    app_version TEXT NOT NULL,
    -- When it arrived, by this Claktory's clock.
    received INTEGER NOT NULL,
    -- What it weighs against the source's 4 MB.
    bytes INTEGER NOT NULL,
    UNIQUE (source, area, seq, at)
);
CREATE INDEX diagnostics_by_received ON diagnostics (received);
-- Each source that recorded diagnostics: as it last said, and what's kept.
CREATE TABLE diagnostics_sources (
    source TEXT PRIMARY KEY,
    -- A device's name; '' for the backend.
    name TEXT NOT NULL,
    -- A DeviceKind: 1 desktop, 2 laptop, 3 phone, 4 tablet; 0 the backend.
    kind INTEGER NOT NULL,
    platform TEXT NOT NULL,
    app_version TEXT NOT NULL,
    events INTEGER NOT NULL,
    bytes INTEGER NOT NULL,
    oldest INTEGER NOT NULL,
    newest INTEGER NOT NULL,
    received INTEGER NOT NULL
);
-- What's kept of each source, by area.
CREATE TABLE diagnostics_areas (
    source TEXT NOT NULL,
    area TEXT NOT NULL,
    events INTEGER NOT NULL,
    bytes INTEGER NOT NULL,
    oldest INTEGER NOT NULL,
    newest INTEGER NOT NULL,
    PRIMARY KEY (source, area)
);
";

/// How long events are kept.
pub(crate) const KEEP_DAYS: u32 = 14;
const KEEP_MS: i64 = KEEP_DAYS as i64 * 24 * 3600 * 1000;
/// The most a source keeps here.
pub(crate) const SOURCE_LIMIT_BYTES: i64 = 4 * 1024 * 1024;
/// The most events a request brings.
const MOST_EVENTS: usize = 2000;
const KIND_CHARS: usize = 40;
const AREA_CHARS: usize = 32;
const DETAIL_CHARS: usize = 500;
/// What a row weighs besides its area, kind and detail.
const ROW_BYTES: i64 = 64;
const SHARE: &str = "share";
/// The backend's own source.
pub(crate) const BACKEND: &str = "backend";
/// The area of connections.
pub(crate) const CONNECTION_AREA: &str = "connection";

/// Who recorded events, as diagnostics keep it: a device from its
/// connection, never from what it sends; or the backend.
#[derive(Clone, Debug, Default)]
pub(crate) struct Source {
    /// `device:<id>`, or `backend`.
    pub id: String,
    pub name: String,
    pub kind: i32,
    pub platform: String,
    pub app_version: String,
}

impl Source {
    /// A device of the person's, by its paired id.
    pub(crate) fn device(id: &str, name: String, kind: i32, platform: String, app_version: String) -> Source {
        Source { id: format!("device:{id}"), name, kind, platform, app_version }
    }

    fn backend() -> Source {
        Source {
            id: BACKEND.to_owned(),
            name: String::new(),
            kind: 0,
            platform: std::env::consts::OS.to_owned(),
            app_version: crate::update::VERSION.to_owned(),
        }
    }
}

/// One event, cleaned: what's kept.
struct Event {
    area: String,
    session: i64,
    seq: i64,
    at: i64,
    kind: String,
    detail: String,
    duration: i64,
}

fn sharing(db: &Connection) -> bool {
    db.query_row("SELECT value FROM diagnostics_settings WHERE name = ?1", [SHARE], |row| row.get::<_, String>(0))
        .optional()
        .ok()
        .flatten()
        .is_some_and(|value| value == "1")
}

/// Workspace.diagnostics.
pub(crate) fn view(factory: &Factory) -> v1::Diagnostics {
    let db = factory.store.db();
    let share = sharing(&db);
    let sources = if share { sources(&db).unwrap_or_default() } else { Vec::new() };
    v1::Diagnostics { share, sources, keep_days: KEEP_DAYS, source_limit_bytes: SOURCE_LIMIT_BYTES }
}

fn sources(db: &Connection) -> rusqlite::Result<Vec<v1::DiagnosticsSource>> {
    let mut sources = crate::store::query(
        db,
        "SELECT source, name, kind, platform, app_version, events, bytes, oldest, newest, received
         FROM diagnostics_sources WHERE events > 0 ORDER BY received DESC, source",
        [],
        |row| {
            Ok(v1::DiagnosticsSource {
                id: row.get(0)?,
                name: row.get(1)?,
                kind: row.get(2)?,
                platform: row.get(3)?,
                app_version: row.get(4)?,
                events: count(row.get(5)?),
                bytes: row.get(6)?,
                oldest_ms: row.get(7)?,
                newest_ms: row.get(8)?,
                received_ms: row.get(9)?,
                areas: Vec::new(),
            })
        },
    )?;
    let areas = crate::store::query(
        db,
        "SELECT source, area, events, bytes, oldest, newest FROM diagnostics_areas WHERE events > 0 ORDER BY source, area",
        [],
        |row| {
            let area = v1::DiagnosticsArea {
                area: row.get(1)?,
                events: count(row.get(2)?),
                bytes: row.get(3)?,
                oldest_ms: row.get(4)?,
                newest_ms: row.get(5)?,
            };
            Ok((row.get::<_, String>(0)?, area))
        },
    )?;
    for (source, area) in areas {
        if let Some(shown) = sources.iter_mut().find(|shown| shown.id == source) {
            shown.areas.push(area);
        }
    }
    Ok(sources)
}

fn count(events: i64) -> u32 {
    u32::try_from(events).unwrap_or(u32::MAX)
}

/// SetDiagnostics.
pub(crate) fn set(factory: &Factory, request: &v1::SetDiagnostics) -> Result<(), RequestError> {
    {
        let mut db = factory.store.db();
        let transaction = db.transaction()?;
        match request.share {
            Some(true) => {
                transaction.execute("INSERT OR REPLACE INTO diagnostics_settings (name, value) VALUES (?1, '1')", [SHARE])?;
            }
            Some(false) => {
                transaction.execute("DELETE FROM diagnostics_settings WHERE name = ?1", [SHARE])?;
                transaction.execute_batch("DELETE FROM diagnostics; DELETE FROM diagnostics_sources; DELETE FROM diagnostics_areas;")?;
            }
            None => {}
        }
        if request.clear {
            let (source, area) = (request.source_id.trim(), request.area.trim());
            transaction.execute(
                "DELETE FROM diagnostics WHERE (?1 = '' OR source = ?1) AND (?2 = '' OR area = ?2)",
                params![source, area],
            )?;
            summed(&transaction, None)?;
        }
        transaction.commit()?;
    }
    factory.broadcast_workspace(&factory.lock());
    Ok(())
}

/// A short token of `a-z 0-9` and `also` (lowercased), at most `most`.
fn token(text: &str, also: &[char], most: usize) -> String {
    text.chars()
        .map(|c| c.to_ascii_lowercase())
        .filter(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || also.contains(c))
        .take(most)
        .collect()
}

fn kind_of(kind: &str) -> String {
    let kind = token(kind, &['_', '.'], KIND_CHARS);
    if kind.is_empty() { "event".to_owned() } else { kind }
}

fn area_of(area: &str) -> String {
    let area = token(area, &['_'], AREA_CHARS);
    if area.is_empty() { "general".to_owned() } else { area }
}

/// One line, at most 500 characters: controls become spaces.
fn detail_of(detail: &str) -> String {
    detail.chars().take(DETAIL_CHARS).map(|c| if c.is_control() { ' ' } else { c }).collect::<String>().trim().to_owned()
}

/// SendDiagnostics, from a device's connection.
pub(crate) fn receive(factory: &Factory, source: &Source, request: v1::SendDiagnostics) -> Result<(), RequestError> {
    if request.events.len() > MOST_EVENTS {
        return Err(RequestError::Command(format!("Send at most {MOST_EVENTS} diagnostic events at a time.")));
    }
    let events = request
        .events
        .iter()
        .map(|event| Event {
            area: area_of(&event.area),
            session: i64::try_from(event.session).unwrap_or(i64::MAX),
            seq: i64::try_from(event.seq).unwrap_or(i64::MAX),
            at: event.at_ms,
            kind: kind_of(&event.kind),
            detail: detail_of(&event.detail),
            duration: event.duration_ms.max(0),
        })
        .collect::<Vec<_>>();
    if !keep(factory, source, &events)? {
        return Err(RequestError::Command("Diagnostics are off on this Claktory.".to_owned()));
    }
    factory.broadcast_workspace(&factory.lock());
    Ok(())
}

/// When this backend started, for its events' session.
static STARTED: std::sync::OnceLock<i64> = std::sync::OnceLock::new();
static BACKEND_SEQ: AtomicU64 = AtomicU64::new(0);
/// The backend's events waiting for factory.db: past this many, new ones
/// are dropped.
const WAITING: usize = 256;
/// The most written in one transaction.
const BATCH: usize = 200;
/// Turned-away knocks alike are one row per this long, with their count.
const TURNED_WINDOW: Duration = Duration::from_secs(10);

/// The backend's own events, on their way to factory.db: never written on
/// the path of a request, one writer at a time, dropped when it can't keep
/// up; and turned-away knocks counted rather than written one by one, since
/// a stranger can make them.
pub(crate) struct Recorder {
    sender: OnceLock<mpsc::Sender<Event>>,
    /// Turned-away knocks seen since their row, by what they share.
    turned: Mutex<HashMap<String, u64>>,
    window: Mutex<Duration>,
}

impl Default for Recorder {
    fn default() -> Recorder {
        Recorder { sender: OnceLock::new(), turned: Mutex::default(), window: Mutex::new(TURNED_WINDOW) }
    }
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// The backend records an event of its own in `area`, kept while
/// diagnostics are on. The same rules as a device's: key=value tokens,
/// never content, names, addresses or codes. Shown in the Workspace with
/// the next change.
pub(crate) fn record(factory: &Arc<Factory>, area: &str, kind: &str, detail: &str, duration_ms: i64) {
    let Ok(runtime) = tokio::runtime::Handle::try_current() else { return };
    let event = Event {
        area: area_of(area),
        session: *STARTED.get_or_init(now_ms),
        seq: i64::try_from(BACKEND_SEQ.fetch_add(1, Ordering::Relaxed)).unwrap_or(i64::MAX),
        at: now_ms(),
        kind: kind_of(kind),
        detail: detail_of(detail),
        duration: duration_ms.max(0),
    };
    let sender = factory.diagnostics.sender.get_or_init(|| {
        let (sender, receiver) = mpsc::channel(WAITING);
        runtime.spawn(write(Arc::downgrade(factory), receiver));
        sender
    });
    // Full: it can't keep up, and diagnostics never slow Claktory down.
    let _ = sender.try_send(event);
}

/// The one writer of the backend's events.
async fn write(factory: Weak<Factory>, mut receiver: mpsc::Receiver<Event>) {
    while let Some(first) = receiver.recv().await {
        let mut batch = vec![first];
        while batch.len() < BATCH {
            match receiver.try_recv() {
                Ok(event) => batch.push(event),
                Err(_) => break,
            }
        }
        let Some(factory) = factory.upgrade() else { return };
        let kept = tokio::task::spawn_blocking(move || keep(&factory, &Source::backend(), &batch)).await;
        if let Ok(Err(error)) = kept {
            tracing::error!(%error, "could not record the backend's diagnostic events");
        }
    }
}

/// A connection turned away (`via`, `why`: tokens): one row per 10 seconds
/// of those alike, `n=` how many it stands for.
pub(crate) fn turned_away(factory: &Arc<Factory>, via: &str, why: &str) {
    let key = format!("via={via} why={why}");
    {
        let mut turned = lock(&factory.diagnostics.turned);
        if let Some(count) = turned.get_mut(&key) {
            *count += 1;
            return;
        }
        if tokio::runtime::Handle::try_current().is_err() {
            return;
        }
        turned.insert(key.clone(), 0);
    }
    record(factory, CONNECTION_AREA, "turned_away", &format!("{key} n=1"), 0);
    let window = *lock(&factory.diagnostics.window);
    let factory = Arc::downgrade(factory);
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(window).await;
            let Some(factory) = factory.upgrade() else { return };
            let count = {
                let mut turned = lock(&factory.diagnostics.turned);
                let count = turned.get(&key).copied().unwrap_or_default();
                if count == 0 {
                    turned.remove(&key);
                    return;
                }
                turned.insert(key.clone(), 0);
                count
            };
            record(&factory, CONNECTION_AREA, "turned_away", &format!("{key} n={count}"), 0);
        }
    });
}

/// Tests: a connection turned away, as connection.rs records it.
#[doc(hidden)]
pub fn turned_away_for_test(factory: &Arc<Factory>, via: &str, why: &str) {
    turned_away(factory, via, why);
}

/// Tests: turned-away knocks alike are one row per `window`.
#[doc(hidden)]
pub fn set_turned_window_for_test(factory: &Factory, window: Duration) {
    *lock(&factory.diagnostics.window) = window;
}

/// Keeps `events` from `source`, when diagnostics are on (false when off),
/// then drops what's past 14 days, and past 4 MB of this source.
fn keep(factory: &Factory, source: &Source, events: &[Event]) -> rusqlite::Result<bool> {
    let now = now_ms();
    let mut db = factory.store.db();
    if !sharing(&db) {
        return Ok(false);
    }
    let transaction = db.transaction()?;
    {
        let mut insert = transaction.prepare(
            "INSERT OR IGNORE INTO diagnostics (source, area, session, seq, at, kind, detail, duration, app_version, received, bytes)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
        )?;
        for event in events.iter().filter(|event| event.at >= now - KEEP_MS) {
            let bytes = ROW_BYTES + (event.area.len() + event.kind.len() + event.detail.len()) as i64;
            insert.execute(params![
                source.id,
                event.area,
                event.session,
                event.seq,
                event.at,
                event.kind,
                event.detail,
                event.duration,
                source.app_version,
                now,
                bytes
            ])?;
        }
    }
    let aged = transaction.execute("DELETE FROM diagnostics WHERE received < ?1 OR at < ?1", [now - KEEP_MS])?;
    let total: i64 =
        transaction.query_row("SELECT COALESCE(SUM(bytes), 0) FROM diagnostics WHERE source = ?1", [&source.id], |row| row.get(0))?;
    if total > SOURCE_LIMIT_BYTES {
        let rows = crate::store::query(
            &transaction,
            "SELECT id, bytes FROM diagnostics WHERE source = ?1 ORDER BY at, id",
            [&source.id],
            crate::store::row2::<i64, i64>,
        )?;
        let mut over = total - SOURCE_LIMIT_BYTES;
        let mut delete = transaction.prepare("DELETE FROM diagnostics WHERE id = ?1")?;
        for (id, bytes) in rows {
            if over <= 0 {
                break;
            }
            delete.execute([id])?;
            over -= bytes;
        }
    }
    transaction.execute(
        "INSERT INTO diagnostics_sources (source, name, kind, platform, app_version, events, bytes, oldest, newest, received)
         VALUES (?1, ?2, ?3, ?4, ?5, 0, 0, 0, 0, ?6)
         ON CONFLICT (source) DO UPDATE SET name = ?2, kind = ?3, platform = ?4, app_version = ?5, received = ?6",
        params![source.id, source.name, source.kind, source.platform, source.app_version, now],
    )?;
    // Every source's sums when 14 days took some of others'.
    summed(&transaction, (aged == 0).then_some(source.id.as_str()))?;
    transaction.commit()?;
    Ok(true)
}

/// Sums what's kept, by area, of `source` (of every source when None),
/// into diagnostics_areas and diagnostics_sources; drops sources with
/// nothing kept.
fn summed(transaction: &Transaction<'_>, source: Option<&str>) -> rusqlite::Result<()> {
    let source = source.unwrap_or("");
    transaction.execute("DELETE FROM diagnostics_areas WHERE ?1 = '' OR source = ?1", [source])?;
    transaction.execute(
        "INSERT INTO diagnostics_areas (source, area, events, bytes, oldest, newest)
         SELECT source, area, COUNT(*), SUM(bytes), MIN(at), MAX(at) FROM diagnostics
         WHERE ?1 = '' OR source = ?1 GROUP BY source, area",
        [source],
    )?;
    transaction.execute(
        "UPDATE diagnostics_sources SET
            events = (SELECT COALESCE(SUM(events), 0) FROM diagnostics_areas AS kept WHERE kept.source = diagnostics_sources.source),
            bytes = (SELECT COALESCE(SUM(bytes), 0) FROM diagnostics_areas AS kept WHERE kept.source = diagnostics_sources.source),
            oldest = (SELECT COALESCE(MIN(oldest), 0) FROM diagnostics_areas AS kept WHERE kept.source = diagnostics_sources.source),
            newest = (SELECT COALESCE(MAX(newest), 0) FROM diagnostics_areas AS kept WHERE kept.source = diagnostics_sources.source)
         WHERE ?1 = '' OR source = ?1",
        [source],
    )?;
    transaction.execute("DELETE FROM diagnostics_sources WHERE events = 0", [])?;
    Ok(())
}

#[cfg(test)]
mod tests {
    #[test]
    fn kinds_and_areas_are_tokens_and_details_one_bounded_line() {
        assert_eq!(super::kind_of("Connect.Attempt_2"), "connect.attempt_2");
        assert_eq!(super::kind_of("<script> 💥"), "script");
        assert_eq!(super::kind_of("💥"), "event");
        assert_eq!(super::kind_of(&"a".repeat(90)).len(), 40);
        assert_eq!(super::area_of("Connection"), "connection");
        assert_eq!(super::area_of("a.b/c d"), "abcd");
        assert_eq!(super::area_of(""), "general");
        assert_eq!(super::area_of(&"z".repeat(90)).len(), 32);
        assert_eq!(super::detail_of(" path=relay\nms=812\u{1b}[31m "), "path=relay ms=812 [31m");
        assert_eq!(super::detail_of(&"é".repeat(900)).chars().count(), 500);
    }
}
