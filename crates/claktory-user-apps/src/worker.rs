//! The worker: serves the root's requests on a local socket, one per
//! connection, from the root's user only, with a database of its own.

use std::io;
use std::path::Path;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use rusqlite::Connection;
use serde::Serialize;
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::Semaphore;

use crate::grants;
use crate::ipc::{self, Caller, Request};

/// Requests served at once; more wait for a turn, up to `TURN_WAIT`.
const CONCURRENT: usize = 32;
const TURN_WAIT: Duration = Duration::from_secs(5);
/// How long the root has to send a request once connected.
const READ_TIMEOUT: Duration = Duration::from_secs(10);

pub struct Worker {
    db: Mutex<Connection>,
    turns: Semaphore,
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

/// The worker's database: its own file, never the root's.
pub fn open_db(path: &Path) -> rusqlite::Result<Connection> {
    let db = Connection::open(path)?;
    db.pragma_update(None, "journal_mode", "WAL")?;
    db.pragma_update(None, "synchronous", "NORMAL")?;
    db.busy_timeout(Duration::from_secs(5))?;
    db.execute_batch(grants::SCHEMA)?;
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
    pub fn open(db: &Path) -> Result<Arc<Worker>, String> {
        let db = open_db(db).map_err(|error| format!("can't open {}: {error}", db.display()))?;
        Ok(Arc::new(Worker { db: Mutex::new(db), turns: Semaphore::new(CONCURRENT) }))
    }

    fn db(&self) -> MutexGuard<'_, Connection> {
        self.db.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Answers one request.
    pub fn answer(&self, request: Request, body: &[u8]) -> Answer {
        match request {
            Request::Health => json(200, &serde_json::json!({"ok": true})),
            Request::PurgeAccount { account_id, .. } => match grants::purge_account(&mut self.db(), &account_id) {
                Ok(()) => (204, Vec::new()),
                Err(refused) => refused_answer(&refused),
            },
            Request::Call { method, path, query: _, time, caller } => self.call(&method, &path, time, &caller, body),
        }
    }

    fn call(&self, method: &str, path: &str, time: i64, caller: &Caller, body: &[u8]) -> Answer {
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
    let Ok(Ok(turn)) = tokio::time::timeout(TURN_WAIT, worker.turns.acquire()).await else {
        let (status, body) = error(503, "busy", "Claktory is busy. Try again in a moment.");
        return ipc::write_answer(&mut stream, status, &body).await;
    };
    let answering = worker.clone();
    let answered = tokio::task::spawn_blocking(move || answering.answer(request, &body)).await;
    drop(turn);
    let (status, body) = answered.unwrap_or_else(|_| error(503, "unavailable", "Something went wrong. Try again later."));
    ipc::write_answer(&mut stream, status, &body).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_backup_is_a_whole_copy_and_only_the_newest_are_kept() {
        let dir = tempfile::tempdir().expect("a folder");
        let db = dir.path().join("apps.db");
        let worker = Worker::open(&db).expect("the worker");
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
}
