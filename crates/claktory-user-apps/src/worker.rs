//! The worker: serves the root's requests on a local socket, one per
//! connection, from the root's user only, with a database of its own.

use std::collections::HashMap;
use std::io;
use std::path::Path;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use rusqlite::Connection;
use serde::Serialize;
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::Semaphore;

use crate::dms::keys::Keys;
use crate::grants;
use crate::ipc::{self, Caller, Request};

/// The worker database's schema version: one made by a newer worker is
/// refused, rather than written to by code that doesn't know its records.
pub const SCHEMA_VERSION: i64 = 1;

/// How often the worker deletes what's past its time.
const SWEEP_EVERY: Duration = Duration::from_secs(3600);

/// Requests served at once; more wait for a turn, up to `TURN_WAIT`.
const CONCURRENT: usize = 32;
const TURN_WAIT: Duration = Duration::from_secs(5);
/// How long the root has to send a request once connected.
const READ_TIMEOUT: Duration = Duration::from_secs(10);
/// Long polls waiting at once, per device (or Claktory) and per account;
/// more are answered without waiting.
const WAITS_PER_KEY: usize = 4;
const WAITS_PER_ACCOUNT: usize = 8;

pub struct Worker {
    db: Mutex<Connection>,
    /// The DM wrapping key: what opens DM fields at rest.
    keys: Keys,
    turns: Semaphore,
    /// The accounts with long polls waiting: each wakes only for its own
    /// account's events, holding neither the database nor a turn.
    waits: Mutex<HashMap<String, Waiting>>,
}

/// An account's long polls: what wakes them, and how many wait, by key.
struct Waiting {
    changes: tokio::sync::watch::Sender<u64>,
    keys: HashMap<String, usize>,
    total: usize,
}

/// A long poll's place among its account's: it wakes on `changes`, and
/// leaves when dropped.
pub struct Slot {
    worker: Arc<Worker>,
    account: String,
    key: String,
    changes: tokio::sync::watch::Receiver<u64>,
}

impl Drop for Slot {
    fn drop(&mut self) {
        let mut waits = self.worker.waits.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(waiting) = waits.get_mut(&self.account) {
            waiting.total -= 1;
            if let Some(count) = waiting.keys.get_mut(&self.key) {
                *count -= 1;
                if *count == 0 {
                    waiting.keys.remove(&self.key);
                }
            }
            if waiting.total == 0 {
                waits.remove(&self.account);
            }
        }
    }
}

/// An answer: its status and JSON body.
pub type Answer = (u16, Vec<u8>);

fn json(status: u16, value: &impl Serialize) -> Answer {
    (status, serde_json::to_vec(value).unwrap_or_default())
}

fn error(status: u16, code: &str, message: &str) -> Answer {
    json(status, &serde_json::json!({"error": code, "message": message}))
}

fn not_found() -> Answer {
    error(404, "not_found", "There's nothing here.")
}

/// The worker's database: its own file, never the root's, bound to the DM
/// wrapping key `keys`. One a newer worker made, or whose DM data keys are
/// sealed with another wrapping key, is refused.
pub fn open_db(path: &Path, keys: &Keys) -> Result<Connection, String> {
    let failed = |error: rusqlite::Error| format!("can't open {}: {error}", path.display());
    let db = Connection::open(path).map_err(failed)?;
    db.busy_timeout(Duration::from_secs(5)).map_err(failed)?;
    let version: i64 = db.pragma_query_value(None, "user_version", |row| row.get(0)).map_err(failed)?;
    if version > SCHEMA_VERSION {
        return Err(format!(
            "{} was made by a newer claktory-user-apps (schema {version}, this one knows {SCHEMA_VERSION}): run that one",
            path.display()
        ));
    }
    db.pragma_update(None, "journal_mode", "WAL").map_err(failed)?;
    db.pragma_update(None, "synchronous", "NORMAL").map_err(failed)?;
    // What's deleted (a forgotten data key, a purged account) is zeroed, not
    // left in free pages.
    db.pragma_update(None, "secure_delete", "ON").map_err(failed)?;
    db.execute_batch(grants::SCHEMA).map_err(failed)?;
    db.execute_batch(crate::dms::identity::SCHEMA).map_err(failed)?;
    db.execute_batch(crate::dms::contacts::SCHEMA).map_err(failed)?;
    db.execute_batch(crate::dms::reports::SCHEMA).map_err(failed)?;
    db.execute_batch(crate::dms::messages::SCHEMA).map_err(failed)?;
    db.execute_batch(crate::dms::sync::SCHEMA).map_err(failed)?;
    db.execute_batch(crate::dms::agents::SCHEMA).map_err(failed)?;
    db.execute_batch(crate::dms::review::SCHEMA).map_err(failed)?;
    db.execute_batch(crate::dms::leaves::SCHEMA).map_err(failed)?;
    db.execute_batch(crate::dms::groups::SCHEMA).map_err(failed)?;
    db.execute_batch(crate::dms::handoffs::SCHEMA).map_err(failed)?;
    // OpenMLS's public state of each group (`dms::groups`), sealed.
    db.execute_batch(claktory_dm_crypto::storage::SCHEMA).map_err(failed)?;
    keys.bind(&db).map_err(|refused| format!("{}: {refused}", path.display()))?;
    // End to end began: D2's worker-sealed bodies go (`dms::bodies`).
    let mut db = db;
    match crate::dms::bodies::forget_format_1_in(&mut db) {
        Ok(0) => {}
        Ok(gone) => tracing::info!(gone, "deleted the worker-sealed DM bodies: messages are end to end now"),
        Err(error) => return Err(format!("{}: {error}", path.display())),
    }
    db.pragma_update(None, "user_version", SCHEMA_VERSION).map_err(failed)?;
    Ok(db)
}

/// Writes a consistent copy of the worker's database at `db` to `dir` (made
/// if missing), while the worker uses it, as `apps-<UTC time>.db`, and keeps
/// the newest `keep` copies there. The copy's path.
pub fn backup(db: &Path, dir: &Path, keep: usize, now: i64) -> Result<std::path::PathBuf, String> {
    std::fs::create_dir_all(dir).map_err(|error| format!("can't make {}: {error}", dir.display()))?;
    let stamp = jiff::Timestamp::from_second(now).map_err(|error| error.to_string())?.strftime("%Y%m%d-%H%M%S").to_string();
    let path = dir.join(format!("apps-{stamp}.db"));
    let conn = Connection::open_with_flags(db, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(|error| format!("can't open {}: {error}", db.display()))?;
    conn.busy_timeout(Duration::from_secs(30)).map_err(|error| error.to_string())?;
    let target = path.to_str().ok_or("the backup folder's path isn't UTF-8")?;
    conn.execute("VACUUM INTO ?1", [target]).map_err(|error| format!("can't write {}: {error}", path.display()))?;
    let mut copies: Vec<std::path::PathBuf> = std::fs::read_dir(dir)
        .map_err(|error| error.to_string())?
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| {
            path.file_name().and_then(|name| name.to_str()).is_some_and(|name| name.starts_with("apps-") && name.ends_with(".db"))
        })
        .collect();
    copies.sort();
    for old in copies.iter().rev().skip(keep) {
        let _ = std::fs::remove_file(old);
    }
    Ok(path)
}

impl Worker {
    pub fn open(db: &Path, keys: Keys) -> Result<Arc<Worker>, String> {
        let db = open_db(db, &keys)?;
        let worker = Arc::new(Worker { db: Mutex::new(db), keys, turns: Semaphore::new(CONCURRENT), waits: Mutex::new(HashMap::new()) });
        worker.sweep();
        Ok(worker)
    }

    /// Deletes what's past its time with nothing else to delete it (reports
    /// after their 30 days, with their keys): at start, then every hour.
    pub fn sweep(&self) {
        let now_ms = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |since| since.as_millis() as i64);
        match crate::dms::reports::sweep(&mut self.db(), now_ms) {
            Ok(0) => {}
            Ok(deleted) => tracing::info!(deleted, "deleted reports past their 30 days"),
            Err(refused) => tracing::error!("could not delete old reports: {refused}"),
        }
        match crate::dms::sync::prune(&mut self.db(), now_ms) {
            Ok(0) => {}
            Ok(pruned) => tracing::info!(pruned, "pruned DM events past their 30 days"),
            Err(refused) => tracing::error!("could not prune old DM events: {refused}"),
        }
        match crate::dms::review::sweep(&mut self.db(), now_ms) {
            Ok(0) => {}
            Ok(deleted) => tracing::info!(deleted, "deleted agents' proposals a week after they were decided"),
            Err(refused) => tracing::error!("could not delete old proposals: {}", refused.2),
        }
        match crate::dms::groups::sweep(&mut self.db(), now_ms) {
            Ok(0) => {}
            Ok(deleted) => tracing::info!(deleted, "deleted group messages and Welcomes past their time"),
            Err(error) => tracing::error!("could not delete old group messages: {error}"),
        }
        if let Err(error) = crate::dms::handoffs::sweep(&self.db(), now_ms) {
            tracing::error!("could not delete old hand-offs: {error}");
        }
        match crate::dms::leaves::sweep(&self.db(), now_ms) {
            Ok(0) => {}
            Ok(deleted) => tracing::info!(deleted, "forgot KeyPackage claims past their 30 days"),
            Err(error) => tracing::error!("could not forget old KeyPackage claims: {error}"),
        }
    }

    /// The DM wrapping key.
    pub fn keys(&self) -> &Keys {
        &self.keys
    }

    fn db(&self) -> MutexGuard<'_, Connection> {
        self.db.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Answers one request.
    pub fn answer(&self, request: Request, body: &[u8]) -> Answer {
        match request {
            Request::Health => json(200, &serde_json::json!({"ok": true})),
            Request::PurgeAccount { account_id, time } => {
                let mut db = self.db();
                if let Err(refused) = grants::purge_account(&mut db, &account_id) {
                    return refused_answer(&refused);
                }
                // Its identity goes; its friend IDs stay retired, owned by no
                // one, so none is found or handed out again; its requests,
                // friendships and blocks end.
                if let Err(failure) = crate::dms::identity::purge(&mut db, &self.keys, &account_id, time.saturating_mul(1000)) {
                    tracing::error!("purging an account's DM identity: {failure}");
                    return error(503, "unavailable", "Something went wrong. Try again later.");
                }
                // Its data keys go, so nothing of its opens again.
                let purpose = crate::dms::keys::account_purpose(&account_id);
                let forgotten =
                    crate::dms::keys::forget(&db, &purpose).and_then(|_| crate::dms::keys::forget_all(&db, &format!("{purpose}:")));
                match forgotten {
                    Ok(_) => (204, Vec::new()),
                    Err(refused) => {
                        tracing::error!("purging an account's DM keys: {refused}");
                        error(503, "unavailable", "Something went wrong. Try again later.")
                    }
                }
            }
            Request::Call { method, path, query, time, caller } => self.call(&method, &path, &query, time, &caller, body),
        }
    }

    fn call(&self, method: &str, path: &str, query: &str, time: i64, caller: &Caller, body: &[u8]) -> Answer {
        let now_ms = time.saturating_mul(1000);
        let Some(route) = path.strip_prefix("/v1/apps/") else { return not_found() };
        match (method, route) {
            // Who the root says the caller is: for apps (and tests) to see
            // what reaches the worker.
            ("GET", "whoami") => json(200, caller),
            ("PUT", "grants") => {
                let Ok(put) = serde_json::from_slice::<grants::Put>(body) else {
                    return error(400, "invalid_request", "The request isn't what this endpoint takes.");
                };
                match grants::put(&mut self.db(), caller, put, now_ms) {
                    Ok(grant) => json(200, &grant),
                    Err(refused) => refused_answer(&refused),
                }
            }
            ("GET", "grants") => match grants::list(&mut self.db(), caller, now_ms) {
                Ok(grants) => json(200, &grants::Grants { grants }),
                Err(refused) => refused_answer(&refused),
            },
            ("DELETE", route) if route.starts_with("grants/") => {
                let id = &route["grants/".len()..];
                match grants::revoke(&mut self.db(), caller, id) {
                    Ok(()) => (204, Vec::new()),
                    Err(refused) => refused_answer(&refused),
                }
            }
            (method, route) if route.starts_with("dms/") => {
                // The query rides with the route, for the routes that read one.
                let route = if query.is_empty() { route["dms/".len()..].to_owned() } else { format!("{}?{query}", &route["dms/".len()..]) };
                // Whose outboxes this call adds to: theirs alone wake.
                let _ = crate::dms::sync::touched();
                let answered = crate::dms::identity::route(&mut self.db(), &self.keys, method, &route, caller, body, now_ms);
                self.wake(&crate::dms::sync::touched());
                match answered {
                    Some((204, _)) => (204, Vec::new()),
                    Some((status, value)) => json(status, &value),
                    None => not_found(),
                }
            }
            _ => not_found(),
        }
    }
}

fn refused_answer(refused: &grants::Refused) -> Answer {
    let (status, body) = refused.answer();
    json(status, &body)
}

/// Serves until the listener fails: only connections from `root_uid`, the
/// account service's user, are answered.
pub async fn serve(listener: UnixListener, worker: Arc<Worker>, root_uid: u32) -> io::Result<()> {
    let sweeper = worker.clone();
    tokio::spawn(async move {
        let mut hourly = tokio::time::interval(SWEEP_EVERY);
        hourly.tick().await;
        loop {
            hourly.tick().await;
            let worker = sweeper.clone();
            let _ = tokio::task::spawn_blocking(move || worker.sweep()).await;
        }
    });
    loop {
        let (stream, _) = match listener.accept().await {
            Ok(accepted) => accepted,
            Err(error) => {
                tracing::warn!("accept: {error}");
                tokio::time::sleep(Duration::from_millis(100)).await;
                continue;
            }
        };
        let worker = worker.clone();
        tokio::spawn(async move {
            if let Err(error) = connection(stream, worker, root_uid).await {
                tracing::debug!("a connection: {error}");
            }
        });
    }
}

async fn connection(mut stream: UnixStream, worker: Arc<Worker>, root_uid: u32) -> io::Result<()> {
    let peer = stream.peer_cred()?;
    if peer.uid() != root_uid {
        tracing::warn!(uid = peer.uid(), "refused a connection from a user other than the account service's");
        return Ok(());
    }
    let (request, body) = tokio::time::timeout(READ_TIMEOUT, ipc::read_request(&mut stream))
        .await
        .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "no request"))??;
    // A long poll asks again each time its account's outbox changed, until
    // it has something to answer or its wait is over; one past its device's
    // or account's waits answers at once.
    let slot = long_poll(&request).and_then(|wait| Some((wait, worker.enter(&request)?)));
    let wait = slot.as_ref().map(|(wait, _)| *wait);
    let mut changes = slot.map(|(_, slot)| slot);
    let deadline = tokio::time::Instant::now() + wait.unwrap_or_default();
    let (request, body) = (Arc::new(request), Arc::new(body));
    loop {
        // Watched before asking, so a change made meanwhile isn't missed.
        if let Some(slot) = changes.as_mut() {
            slot.changes.borrow_and_update();
        }
        let Ok(Ok(turn)) = tokio::time::timeout(TURN_WAIT, worker.turns.acquire()).await else {
            let (status, body) = error(503, "busy", "Claktory is busy. Try again in a moment.");
            return ipc::write_answer(&mut stream, status, &body).await;
        };
        let (answering, asked, bytes) = (worker.clone(), request.clone(), body.clone());
        let answered = tokio::task::spawn_blocking(move || answering.answer((*asked).clone(), &bytes)).await;
        drop(turn);
        let (status, answer) = answered.unwrap_or_else(|_| error(503, "unavailable", "Something went wrong. Try again later."));
        let nothing = wait.is_some()
            && status == 200
            && serde_json::from_slice::<serde_json::Value>(&answer)
                .is_ok_and(|value| value["events"].as_array().is_some_and(Vec::is_empty) || value["waiting"] == true);
        let left = deadline.saturating_duration_since(tokio::time::Instant::now());
        if let Some(slot) = changes.as_mut()
            && nothing
            && !left.is_zero()
            && tokio::time::timeout(left, slot.changes.changed()).await.is_ok_and(|changed| changed.is_ok())
        {
            continue;
        }
        return ipc::write_answer(&mut stream, status, &answer).await;
    }
}

impl Worker {
    /// A place for a long poll of `request`'s caller, or None when its
    /// device (or Claktory) or account has as many waiting as it may.
    pub fn enter(self: &Arc<Self>, request: &Request) -> Option<Slot> {
        let Request::Call { caller, .. } = request else { return None };
        let (account, key) = match caller {
            Caller::Device { account_id, device_key, .. } => (account_id.clone(), format!("device:{device_key}")),
            Caller::Backend { account_id, claktory_id, .. } => (account_id.clone(), format!("claktory:{claktory_id}")),
            Caller::Account { .. } => return None,
        };
        let mut waits = self.waits.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let waiting = waits.entry(account.clone()).or_insert_with(|| Waiting {
            changes: tokio::sync::watch::channel(0).0,
            keys: HashMap::new(),
            total: 0,
        });
        let count = waiting.keys.get(&key).copied().unwrap_or(0);
        if count >= WAITS_PER_KEY || waiting.total >= WAITS_PER_ACCOUNT {
            if waiting.total == 0 {
                waits.remove(&account);
            }
            return None;
        }
        waiting.keys.insert(key.clone(), count + 1);
        waiting.total += 1;
        let changes = waiting.changes.subscribe();
        Some(Slot { worker: self.clone(), account, key, changes })
    }

    /// Wakes the long polls of `accounts`, whose outboxes changed.
    fn wake(&self, accounts: &[String]) {
        let waits = self.waits.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        for account in accounts {
            if let Some(waiting) = waits.get(account) {
                waiting.changes.send_modify(|count| *count += 1);
            }
        }
    }
}

/// How long a call may wait for something to happen, with `wait` seconds
/// (at most 25): a DM sync, or the agents' events (agents.rs), after a
/// cursor, answering `"events"`, empty while there's nothing new; or a
/// proposal's status (review.rs), `"waiting"` while the person hasn't
/// decided.
fn long_poll(request: &Request) -> Option<Duration> {
    let Request::Call { method, path, query, .. } = request else { return None };
    let pairs: Vec<(&str, &str)> = query.split('&').filter_map(|pair| pair.split_once('=')).collect();
    let since = pairs.iter().any(|(name, _)| *name == "since");
    let feed = matches!(path.as_str(), "/v1/apps/dms/sync" | "/v1/apps/dms/agent/events" | "/v1/apps/dms/claktory/events") && since;
    let proposal = path.strip_prefix("/v1/apps/dms/proposals/").is_some_and(|id| !id.is_empty() && !id.contains('/'));
    if method != "GET" || !(feed || proposal) {
        return None;
    }
    let seconds: u64 = pairs.iter().find(|(name, _)| *name == "wait")?.1.parse().ok()?;
    (1..=crate::dms::sync::MAX_WAIT).contains(&seconds).then(|| Duration::from_secs(seconds))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test(flavor = "multi_thread")]
    async fn a_sync_waits_for_a_change_and_answers_it_at_once() {
        let dir = tempfile::tempdir().expect("a folder");
        let worker = Worker::open(&dir.path().join("apps.db"), Keys::from_bytes([7; 32])).expect("the worker");
        let socket = dir.path().join("apps.sock");
        let listener = UnixListener::bind(&socket).expect("a socket");
        let uid = {
            let (stream, _other) = UnixStream::pair().expect("a pair");
            stream.peer_cred().expect("credentials").uid()
        };
        tokio::spawn(serve(listener, worker.clone(), uid));
        let ask = |method: &str, path: &str, query: &str, body: serde_json::Value| {
            let caller = Caller::Device { account_id: "ana".into(), device_key: "k".into(), registrations: Vec::new() };
            let request = Request::Call {
                method: method.into(),
                path: format!("/v1/apps/dms/{path}"),
                query: query.into(),
                time: 1_795_183_320,
                caller,
            };
            let (socket, body) = (socket.clone(), if body.is_null() { Vec::new() } else { body.to_string().into_bytes() });
            async move {
                let (status, answer) = ipc::ask(&socket, uid, &request, &body).await.expect("an answer");
                (status, serde_json::from_slice::<serde_json::Value>(&answer).unwrap_or_default())
            }
        };
        let (status, snapshot) = ask("GET", "sync", "", serde_json::Value::Null).await;
        assert_eq!(status, 200, "{snapshot}");
        let cursor = snapshot["cursor"].as_str().expect("a cursor").to_owned();

        // Nothing happens: it answers nothing once its wait is over.
        let started = tokio::time::Instant::now();
        let (status, nothing) = ask("GET", "sync", &format!("since={cursor}&wait=1"), serde_json::Value::Null).await;
        assert_eq!((status, nothing["events"].clone()), (200, serde_json::json!([])));
        assert!(started.elapsed() >= Duration::from_secs(1), "{:?}", started.elapsed());

        // Something does, on another device: the waiting one hears at once.
        let started = tokio::time::Instant::now();
        let waiting = tokio::spawn(ask("GET", "sync", &format!("since={cursor}&wait=20"), serde_json::Value::Null));
        tokio::time::sleep(Duration::from_millis(300)).await;
        let (_, me) = ask("GET", "me", "", serde_json::Value::Null).await;
        let privacy = serde_json::json!({"requests_on": false, "revision": me["privacy"]["revision"]});
        assert_eq!(ask("PUT", "me/privacy", "", privacy).await.0, 200);
        let (status, changes) = waiting.await.expect("the poll");
        assert_eq!(status, 200, "{changes}");
        assert!(changes["events"].as_array().is_some_and(|events| events.iter().any(|event| event["kind"] == "me")), "{changes}");
        let took = started.elapsed();
        assert!(took >= Duration::from_millis(300) && took < Duration::from_secs(10), "{took:?}");

        // A fifth wait from one device holds no place: it answers at once.
        let (_, latest) = ask("GET", "sync", &format!("since={cursor}"), serde_json::Value::Null).await;
        let latest = latest["next_cursor"].as_str().expect("a cursor").to_owned();
        let waiting = Request::Call {
            method: "GET".into(),
            path: "/v1/apps/dms/sync".into(),
            query: format!("since={latest}&wait=20"),
            time: 1_795_183_320,
            caller: Caller::Device { account_id: "ana".into(), device_key: "k".into(), registrations: Vec::new() },
        };
        let held: Vec<Slot> = (0..WAITS_PER_KEY).map(|_| worker.enter(&waiting).expect("a place")).collect();
        let started = tokio::time::Instant::now();
        let (status, nothing) = ask("GET", "sync", &format!("since={latest}&wait=20"), serde_json::Value::Null).await;
        assert_eq!((status, nothing["events"].clone()), (200, serde_json::json!([])));
        assert!(started.elapsed() < Duration::from_secs(2), "{:?}", started.elapsed());
        drop(held);
    }

    #[test]
    fn only_a_feed_after_a_cursor_or_a_proposal_s_status_waits() {
        let call = |method: &str, path: &str, query: &str| Request::Call {
            method: method.into(),
            path: path.into(),
            query: query.into(),
            time: 1_795_183_320,
            caller: Caller::Device { account_id: "ana".into(), device_key: "k".into(), registrations: Vec::new() },
        };
        let waits = |method: &str, path: &str, query: &str| long_poll(&call(method, path, query)).map(|wait| wait.as_secs());
        assert_eq!(waits("GET", "/v1/apps/dms/sync", "since=c1.0&wait=25"), Some(25));
        assert_eq!(waits("GET", "/v1/apps/dms/sync", "wait=25"), None, "a snapshot answers at once");
        assert_eq!(waits("GET", "/v1/apps/dms/agent/events", "since=c1.0&wait=5"), Some(5));
        assert_eq!(waits("GET", "/v1/apps/dms/proposals/abc", "wait=10"), Some(10));
        assert_eq!(waits("GET", "/v1/apps/dms/proposals", "wait=10"), None);
        assert_eq!(waits("GET", "/v1/apps/dms/proposals/abc/approve", "wait=10"), None);
        assert_eq!(waits("POST", "/v1/apps/dms/proposals/abc", "wait=10"), None);
        assert_eq!(waits("GET", "/v1/apps/dms/sync", "since=c1.0&wait=26"), None);
    }

    #[test]
    fn a_long_poll_wakes_only_for_its_own_account_and_a_device_or_account_waits_a_few_at_once() {
        let dir = tempfile::tempdir().expect("a folder");
        let worker = Worker::open(&dir.path().join("apps.db"), Keys::from_bytes([7; 32])).expect("the worker");
        let call = |account: &str, key: &str, method: &str, path: &str, query: &str| Request::Call {
            method: method.into(),
            path: format!("/v1/apps/dms/{path}"),
            query: query.into(),
            time: 1_795_183_320,
            caller: Caller::Device { account_id: account.into(), device_key: key.into(), registrations: Vec::new() },
        };
        let wait = |account: &str, key: &str| call(account, key, "GET", "sync", "since=c1.0&wait=25");
        let mut ana = worker.enter(&wait("ana", "phone")).expect("a place");
        let mut bo = worker.enter(&wait("bo", "phone")).expect("a place");

        // Ana changes something: her waits wake, Bo's don't.
        let (_, me) = worker.answer(call("ana", "phone", "GET", "me", ""), b"");
        let me: serde_json::Value = serde_json::from_slice(&me).expect("json");
        let privacy = serde_json::json!({"requests_on": false, "revision": me["privacy"]["revision"]}).to_string();
        assert_eq!(worker.answer(call("ana", "phone", "PUT", "me/privacy", ""), privacy.as_bytes()).0, 200);
        assert!(ana.changes.has_changed().expect("open"));
        assert!(!bo.changes.has_changed().expect("open"), "another account's change");
        ana.changes.borrow_and_update();
        // A refused change wakes no one.
        assert_eq!(worker.answer(call("ana", "phone", "PUT", "me/privacy", ""), privacy.as_bytes()).0, 409);
        assert!(!ana.changes.has_changed().expect("open"));
        bo.changes.borrow_and_update();

        // Four at once from a device, eight from an account.
        let mut held: Vec<Slot> = (1..WAITS_PER_KEY).map(|_| worker.enter(&wait("ana", "phone")).expect("a place")).collect();
        assert!(worker.enter(&wait("ana", "phone")).is_none(), "the device's fifth");
        held.extend((0..WAITS_PER_KEY).map(|_| worker.enter(&wait("ana", "mac")).expect("a place")));
        assert!(worker.enter(&wait("ana", "ipad")).is_none(), "the account's ninth");
        assert!(worker.enter(&wait("bo", "mac")).is_some(), "another account's own");
        held.pop();
        assert!(worker.enter(&wait("ana", "ipad")).is_some(), "one left, one comes");
        drop((ana, bo, held));
        assert!(worker.waits.lock().expect("waits").is_empty(), "nothing left behind");
    }

    #[test]
    fn a_backup_is_a_whole_copy_and_only_the_newest_are_kept() {
        let dir = tempfile::tempdir().expect("a folder");
        let db = dir.path().join("apps.db");
        let worker = Worker::open(&db, Keys::from_bytes([7; 32])).expect("the worker");
        let caller = Caller::Backend { account_id: "acct".into(), claktory_id: "mac".into(), epoch: 1, agent: None };
        let put = serde_json::json!({"app": "dms", "agent": {"id": "june", "incarnation": "i"}, "scopes": ["dm.read"],
                                     "resources": [{"kind": "conversation", "id": "c1"}]});
        let request =
            Request::Call { method: "PUT".into(), path: "/v1/apps/grants".into(), query: String::new(), time: 1_795_183_320, caller };
        let (status, body) = worker.answer(request, put.to_string().as_bytes());
        assert_eq!(status, 200, "{}", String::from_utf8_lossy(&body));
        let backups = dir.path().join("backups");
        let mut copies = Vec::new();
        for at in 0..3 {
            copies.push(backup(&db, &backups, 2, 1_795_183_320 + at).expect("a copy"));
        }
        assert!(copies[2].ends_with("apps-20261120-140202.db"), "{:?}", copies[2]);
        let kept: Vec<_> = std::fs::read_dir(&backups).expect("the folder").collect();
        assert_eq!(kept.len(), 2);
        assert!(!copies[0].exists(), "the oldest goes");
        let copy = Connection::open(&copies[2]).expect("the copy");
        let grants: i64 = copy.query_row("SELECT COUNT(*) FROM grants", [], |row| row.get(0)).expect("grants");
        assert_eq!(grants, 1);
    }

    #[test]
    fn a_purged_account_s_data_keys_go_so_nothing_of_its_opens() {
        use crate::dms::keys::{Field, account_purpose};
        let dir = tempfile::tempdir().expect("a folder");
        let worker = Worker::open(&dir.path().join("apps.db"), Keys::from_bytes([7; 32])).expect("the worker");
        let field = Field { table: "dm_profiles", record: "acct", column: "profile" };
        let (mine, theirs) = {
            let conn = worker.db();
            let keys = worker.keys();
            (
                keys.seal(&conn, &account_purpose("acct"), field, b"Matt", 1).expect("sealed"),
                keys.seal(&conn, &account_purpose("other"), field, b"Eve", 1).expect("sealed"),
            )
        };
        let (status, _) = worker.answer(Request::PurgeAccount { account_id: "acct".into(), time: 1 }, &[]);
        assert_eq!(status, 204);
        let conn = worker.db();
        assert!(worker.keys().open(&conn, field, &mine).is_err(), "gone with its key");
        assert_eq!(worker.keys().open(&conn, field, &theirs).expect("another account's stays").bytes(), b"Eve");
    }

    #[test]
    fn a_restored_copy_opens_only_with_its_own_key_and_by_a_worker_that_knows_it() {
        use crate::dms::keys::Field;
        let dir = tempfile::tempdir().expect("a folder");
        let db = dir.path().join("apps.db");
        let field = Field { table: "dm_messages", record: "m1", column: "body" };
        let sealed = {
            let worker = Worker::open(&db, Keys::from_bytes([7; 32])).expect("the worker");
            let conn = worker.db();
            worker.keys().seal(&conn, "conversation:c1", field, b"the plan for friday", 1).expect("sealed")
        };
        let copy = backup(&db, &dir.path().join("backups"), 1, 1_795_183_320).expect("a copy");
        // The copy alone: no plaintext anywhere in it.
        let bytes = std::fs::read(&copy).expect("the copy's bytes");
        assert!(!bytes.windows(10).any(|window| window == b"the plan f"), "no DM text in a backup");
        // Restored with another key: refused, and nothing written to it.
        let refused = Worker::open(&copy, Keys::from_bytes([8; 32])).err().expect("refused");
        assert!(refused.contains("sealed with the wrapping key"), "{refused}");
        let restored = Worker::open(&copy, Keys::from_bytes([7; 32])).expect("its own key");
        let conn = restored.db();
        assert_eq!(restored.keys().open(&conn, field, &sealed).expect("opens").bytes(), b"the plan for friday");
        drop(conn);
        // Made by a newer worker: refused.
        Connection::open(&copy).expect("the copy").pragma_update(None, "user_version", SCHEMA_VERSION + 1).expect("newer");
        let refused = Worker::open(&copy, Keys::from_bytes([7; 32])).err().expect("refused");
        assert!(refused.contains("newer claktory-user-apps"), "{refused}");
    }
}
