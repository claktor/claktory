//! Seals on files (the Seal extension): an agent records a file's SHA-256
//! at a moment, with a note, so anyone can later tell whether the file is
//! still what it was then (a pass line written before an experiment ran,
//! say). The records are Claktory's, in factory.db: added, never changed or
//! removed (the table refuses both), and no agent tool writes there. A copy
//! of the version sealed is kept with ink's blobs (`crate::ink`), named by
//! its SHA-256 and out of every agent's file tools, so the person can open
//! it after the file changed.
//!
//! What holds it: agents' own tools can't change or remove a seal. Agents'
//! commands run as the person, so one that set out to could still edit
//! factory.db; the seal records drift, not a determined agent.
//!
//! Each seal shows in Needs you's History (NEEDS_YOU_RECORD_KIND_SEALED), a
//! viewer showing the file says whether it still matches (FileContent.seals,
//! ServerMessage.file_seals), and ReadFile.sealed_sha256 opens the copy.

use std::collections::HashMap;
use std::io::{Read as _, Write as _};
use std::path::{Path, PathBuf};

use factory_protocol::v1::{self, server_message::Body};
use rusqlite::{Connection, OpenFlags, params};
use sha2::{Digest, Sha256};

use crate::factory::Factory;
use crate::transcript::now_ms;

pub(crate) const SCHEMA: &str = "
-- Seals on files (crate::seals): a file's SHA-256 at a moment, recorded by
-- an agent with a note. Added, never changed or removed.
CREATE TABLE IF NOT EXISTS seals (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    -- The file, absolute, links resolved.
    path TEXT NOT NULL,
    -- Its SHA-256 then, lowercase hex.
    sha256 TEXT NOT NULL,
    agent TEXT NOT NULL,
    -- The agent's name then.
    agent_name TEXT NOT NULL,
    note TEXT NOT NULL,
    -- 1: a copy of that version is kept (crate::ink's blobs).
    kept INTEGER NOT NULL,
    at INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS seals_by_path ON seals (path, id);
CREATE INDEX IF NOT EXISTS seals_at ON seals (at);
CREATE TRIGGER IF NOT EXISTS seals_never_changed BEFORE UPDATE ON seals
BEGIN SELECT RAISE(ABORT, 'a seal is never changed'); END;
CREATE TRIGGER IF NOT EXISTS seals_never_removed BEFORE DELETE ON seals
BEGIN SELECT RAISE(ABORT, 'a seal is never removed'); END;
";

/// The copies the person deleted (DeleteSealedCopy): each takes the copy
/// from every seal of that content made before it.
pub(crate) const DELETED: &str = "
-- Copies of sealed versions the person deleted (crate::seals): by content,
-- for the seals made before.
CREATE TABLE IF NOT EXISTS seal_copies_deleted (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    sha256 TEXT NOT NULL,
    at INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS seal_copies_deleted_by_sha ON seal_copies_deleted (sha256, at);
";

pub(crate) const TOOL: &str = "seal";
/// The key of a seal's result details naming its record.
pub(crate) const DETAIL: &str = "seal";
/// Files up to this size have a copy of the version sealed kept.
pub(crate) const KEEP_MOST: u64 = 64 * 1024 * 1024;
/// Files over this can't be sealed: viewers don't hash them.
const SEAL_MOST: u64 = 2 * 1024 * 1024 * 1024;
/// Seals an agent may make in an hour.
pub(crate) const PER_HOUR: i64 = 60;
/// The longest note kept.
pub(crate) const NOTE_MOST: usize = 300;
pub(crate) const NOT_KEPT: &str = "This sealed version wasn't kept.";

/// A seal as kept.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Seal {
    pub id: i64,
    pub path: String,
    pub sha256: String,
    pub agent: String,
    pub agent_name: String,
    pub note: String,
    pub kept: bool,
    pub at: i64,
}

impl Seal {
    fn of(row: &rusqlite::Row<'_>) -> rusqlite::Result<Seal> {
        Ok(Seal {
            id: row.get(0)?,
            path: row.get(1)?,
            sha256: row.get(2)?,
            agent: row.get(3)?,
            agent_name: row.get(4)?,
            note: row.get(5)?,
            kept: row.get(6)?,
            at: row.get(7)?,
        })
    }

    /// As clients see it: kept only while its copy is there.
    fn proto(&self, home: &crate::home::Home) -> v1::FileSeal {
        v1::FileSeal {
            sha256: self.sha256.clone(),
            agent_id: self.agent.clone(),
            agent_name: self.agent_name.clone(),
            sealed_ms: self.at,
            note: self.note.clone(),
            kept: self.kept && crate::ink::blob(home, &self.sha256).is_some(),
        }
    }

    /// Claktory's sentence for it, with the agent's name now (or then):
    /// "Tomás sealed passline.md".
    pub(crate) fn title(&self, names: &HashMap<String, String>) -> String {
        let agent = names.get(&self.agent).cloned().unwrap_or_else(|| self.agent_name.clone());
        format!("{agent} sealed {}", file_name(&self.path))
    }
}

/// A seal's columns, `kept` only while the person hasn't deleted its copy.
const COLUMNS: &str = "id, path, sha256, agent, agent_name, note, kept = 1 AND NOT EXISTS (SELECT 1 FROM seal_copies_deleted AS deleted WHERE deleted.sha256 = seals.sha256 AND deleted.at >= seals.at), at";

fn file_name(path: &str) -> String {
    Path::new(path).file_name().map_or_else(|| path.to_owned(), |name| name.to_string_lossy().into_owned())
}

/// `path`'s seals, newest first.
pub(crate) fn of_path(db: &Connection, path: &str) -> rusqlite::Result<Vec<Seal>> {
    crate::store::query(db, &format!("SELECT {COLUMNS} FROM seals WHERE path = ?1 ORDER BY id DESC"), [path], Seal::of)
}

/// The seals from `since` to before `before`, newest first, at most `limit`.
pub(crate) fn between(db: &Connection, since: i64, before: i64, limit: i64) -> rusqlite::Result<Vec<Seal>> {
    crate::store::query(
        db,
        &format!("SELECT {COLUMNS} FROM seals WHERE at >= ?1 AND at < ?2 ORDER BY at DESC, id DESC LIMIT ?3"),
        params![since, before, limit],
        Seal::of,
    )
}

/// Whether a seal keeps blob `sha` (`crate::ink::referenced`).
pub(crate) fn refers_to(db: &Connection, sha: &str) -> rusqlite::Result<bool> {
    db.query_row(
        "SELECT EXISTS (SELECT 1 FROM seals WHERE sha256 = ?1 AND kept = 1
            AND NOT EXISTS (SELECT 1 FROM seal_copies_deleted AS deleted WHERE deleted.sha256 = seals.sha256 AND deleted.at >= seals.at))",
        [sha],
        |row| row.get(0),
    )
}

/// Whether the file at `path` is now the version `seals`' latest names.
fn state(path: &Path, seals: &[Seal]) -> v1::FileSealState {
    let Some(latest) = seals.first() else { return v1::FileSealState::Unspecified };
    match crate::extensions::viewer::whole_sha256(path) {
        Ok(sha) if sha == latest.sha256 => v1::FileSealState::Matches,
        _ => v1::FileSealState::Changed,
    }
}

fn file_seals(home: &crate::home::Home, path: &str, seals: &[Seal]) -> v1::FileSeals {
    let shown = seals.iter().map(|seal| seal.proto(home)).collect();
    v1::FileSeals { path: path.to_owned(), seals: shown, state: state(Path::new(path), seals).into() }
}

/// `path` (absolute, or relative to `folder`) as seals keep it: "~"
/// expanded, links resolved.
fn resolve(folder: &Path, path: &str) -> Result<String, String> {
    let path = path.trim();
    if path.is_empty() {
        return Err("Say which file, in path.".to_owned());
    }
    let asked = Path::new(path);
    let path = if asked.is_absolute() || path == "~" || path.starts_with("~/") {
        path.to_owned()
    } else {
        folder.join(asked).display().to_string()
    };
    crate::annotations::absolute(&path).map_err(|error| error.to_string())
}

/// The folders no agent's file tools reach, which no seal reads either.
fn private(factory: &Factory) -> Vec<PathBuf> {
    crate::ink::private(&factory.home)
        .into_iter()
        .chain(crate::browser_host::confine::denied(true).into_iter().map(|denied| denied.dir))
        .collect()
}

/// A file sealed: its seal, and the file's seals now.
pub(crate) struct Sealed {
    pub seal: Seal,
    pub seals: v1::FileSeals,
}

/// Copies `file` into ink's staging as it hashes it: the copy, and the
/// SHA-256 of exactly what was copied.
fn copy_hashed(factory: &Factory, file: &Path) -> std::io::Result<(PathBuf, String)> {
    let staging = crate::ink::staging(&factory.home)?;
    let partial = staging.join(format!("seal-{}-{}.partial", std::process::id(), now_ms()));
    let mut out = std::fs::File::create(&partial)?;
    let mut input = std::fs::File::open(file)?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0; 1 << 20];
    let copied = (|| {
        loop {
            let read = input.read(&mut buffer)?;
            if read == 0 {
                break;
            }
            hasher.update(&buffer[..read]);
            out.write_all(&buffer[..read])?;
        }
        out.flush()
    })();
    if let Err(error) = copied {
        let _ = std::fs::remove_file(&partial);
        return Err(error);
    }
    Ok((partial, crate::encoding::hex(&hasher.finalize())))
}

/// `agent` seals `path` (absolute, or relative to `folder`) with `note`:
/// records its SHA-256, keeps a copy of it when it's small enough, and
/// tells every client.
pub(crate) fn seal(factory: &Factory, agent: &str, folder: &Path, path: &str, note: &str) -> Result<Sealed, String> {
    // One line, as History and viewers show it, and nothing in it that
    // reads other than it shows.
    let note = note.split_whitespace().collect::<Vec<_>>().join(" ");
    if claktory_account::protocol::without_hidden(&note) != note {
        return Err("note can't hold hidden, control or direction characters: write it as plain text. Nothing was sealed.".to_owned());
    }
    let note = note.as_str();
    if note.is_empty() {
        return Err("Say what the seal fixes, in note (\"pass line for run 12, before it runs\"). Nothing was sealed.".to_owned());
    }
    if note.chars().count() > NOTE_MOST {
        return Err(format!("note is at most {NOTE_MOST} characters. Nothing was sealed."));
    }
    let path = resolve(folder, path)?;
    let real = Path::new(&path);
    if private(factory).iter().any(|dir| real.starts_with(std::fs::canonicalize(dir).unwrap_or_else(|_| dir.clone()))) {
        return Err(format!("{path} is in one of Claktory's own folders, which no agent's tools read. Nothing was sealed."));
    }
    let metadata = std::fs::metadata(real).map_err(|error| match error.kind() {
        std::io::ErrorKind::NotFound => format!("No such file: {path}"),
        _ => format!("Can't read {path}: {error}"),
    })?;
    if !metadata.is_file() {
        return Err(format!("{path} isn't a file: only a file can be sealed."));
    }
    if metadata.len() > SEAL_MOST {
        return Err(format!("{path} is over 2 GB, too big to seal."));
    }
    let name = {
        let inner = factory.lock();
        crate::extensions::caller(&inner.config, agent)?.name.clone()
    };
    {
        let db = factory.store.db();
        let recent: i64 = db
            .query_row("SELECT COUNT(*) FROM seals WHERE agent = ?1 AND at > ?2", params![agent, now_ms() - 3_600_000], |row| row.get(0))
            .map_err(|error| format!("Claktory couldn't read its seals: {error}"))?;
        if recent >= PER_HOUR {
            return Err(format!("You've made {PER_HOUR} seals in the last hour, the most there can be. Nothing was sealed."));
        }
    }
    let unreadable = |error: std::io::Error| format!("Can't read {path}: {error}");
    let (sha256, kept) = if metadata.len() <= KEEP_MOST {
        let (partial, sha) = copy_hashed(factory, real).map_err(unreadable)?;
        crate::ink::keep(&factory.home, &partial, &sha, "application/octet-stream")
            .map_err(|error| format!("Claktory couldn't keep a copy: {error}"))?;
        (sha, true)
    } else {
        (crate::extensions::viewer::whole_sha256(real).map_err(unreadable)?, false)
    };
    let at = now_ms();
    let seal = {
        let db = factory.store.db();
        db.execute(
            "INSERT INTO seals (path, sha256, agent, agent_name, note, kept, at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![path, sha256, agent, name, note, kept, at],
        )
        .map_err(|error| format!("Claktory couldn't record the seal: {error}"))?;
        Seal {
            id: db.last_insert_rowid(),
            path: path.clone(),
            sha256,
            agent: agent.to_owned(),
            agent_name: name,
            note: note.to_owned(),
            kept,
            at,
        }
    };
    let seals = of_path(&factory.store.db(), &path).map_err(|error| format!("Claktory couldn't read its seals: {error}"))?;
    let seals = file_seals(&factory.home, &path, &seals);
    factory.send(Body::FileSeals(seals.clone()));
    Ok(Sealed { seal, seals })
}

/// `path` (absolute, or relative to `folder`), and its seals now.
pub(crate) fn check(factory: &Factory, folder: &Path, path: &str) -> Result<(String, Vec<Seal>, v1::FileSealState), String> {
    let path = resolve(folder, path)?;
    let seals = of_path(&factory.store.db(), &path).map_err(|error| format!("Claktory couldn't read its seals: {error}"))?;
    let state = state(Path::new(&path), &seals);
    Ok((path, seals, state))
}

/// The seal a seal call's result names (its details), for History to find
/// the call.
pub(crate) fn sealed_of(call: &v1::ToolCall) -> Option<String> {
    if call.name != TOOL {
        return None;
    }
    let details: serde_json::Value = serde_json::from_str(&call.details).ok()?;
    details.get(DETAIL)?.as_i64().map(|id| id.to_string())
}

/// factory.db in `data`, to read seals apart from the store's connection;
/// None when there's none to read.
fn reader(data: &Path) -> Option<Connection> {
    let db =
        Connection::open_with_flags(data.join("factory.db"), OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX).ok()?;
    db.busy_timeout(std::time::Duration::from_secs(5)).ok()?;
    Some(db)
}

/// ReadFile: the file (or, with `sealed_sha256`, a version of it kept when
/// it was sealed), with its seals on a read from the start.
pub(crate) fn read_requested(data: &Path, request: &v1::ReadFile) -> v1::FileContent {
    let sealed = request.sealed_sha256.trim();
    let mut content = if sealed.is_empty() { crate::previews::read_requested(data, request) } else { read_sealed(data, request, sealed) };
    if content.error.is_empty() && request.offset == 0 {
        let path = crate::annotations::absolute(&content.path).unwrap_or_else(|_| content.path.clone());
        let seals = reader(data).and_then(|db| of_path(&db, &path).ok()).unwrap_or_default();
        content.seals = Some(file_seals(&crate::home::Home::new(data.to_owned()), &path, &seals));
    }
    content
}

/// ReadFile.sealed_sha256: the copy kept when `request.path` was sealed
/// at `sha`, read as the file.
fn read_sealed(data: &Path, request: &v1::ReadFile, sha: &str) -> v1::FileContent {
    let named = crate::extensions::viewer::expand(request.path.trim());
    let not_kept = || v1::FileContent { path: named.display().to_string(), error: NOT_KEPT.to_owned(), ..v1::FileContent::default() };
    let path = crate::annotations::absolute(&named.display().to_string()).unwrap_or_else(|_| named.display().to_string());
    let seal = reader(data)
        .and_then(|db| of_path(&db, &path).ok())
        .and_then(|seals| seals.into_iter().find(|seal| seal.kept && seal.sha256.eq_ignore_ascii_case(sha)));
    let Some(seal) = seal else { return not_kept() };
    let home = crate::home::Home::new(data.to_owned());
    let Some(copy) = crate::ink::blob(&home, &seal.sha256) else { return not_kept() };
    let mut content = crate::extensions::viewer::read_copy(&copy.path, &named, request.offset, request.max_bytes);
    if content.error.is_empty() {
        content.modified_ms = seal.at;
    }
    content
}

/// DeleteSealedCopy: the person deletes the copy kept of a sealed version,
/// for every seal of that content; the seals stay as they were made.
pub(crate) fn delete_copy(factory: &Factory, request: &v1::DeleteSealedCopy) -> Result<(), crate::factory::RequestError> {
    use crate::factory::RequestError;
    let failed = |error: rusqlite::Error| RequestError::Command(format!("Claktory couldn't delete the copy: {error}"));
    let path = crate::annotations::absolute(&request.path)?;
    let sha = request.sha256.trim().to_ascii_lowercase();
    let paths = {
        let db = factory.store.db();
        let keeps = of_path(&db, &path).map_err(failed)?.iter().any(|seal| seal.kept && seal.sha256 == sha);
        if !keeps {
            return Err(RequestError::Command(NOT_KEPT.to_owned()));
        }
        db.execute("INSERT INTO seal_copies_deleted (sha256, at) VALUES (?1, ?2)", params![sha, now_ms()]).map_err(failed)?;
        // Kept while anything else (ink) still names it; with the records
        // held, so no save can come to name it meanwhile.
        if !crate::ink::referenced(&db, &sha).unwrap_or(true)
            && let Some(blob) = crate::ink::blob(&factory.home, &sha)
            && let Err(error) = std::fs::remove_file(&blob.path)
        {
            tracing::warn!(%error, "couldn't remove a sealed version's copy");
        }
        crate::store::query(&db, "SELECT DISTINCT path FROM seals WHERE sha256 = ?1", [&sha], |row| row.get::<_, String>(0))
            .map_err(failed)?
    };
    for path in paths {
        let seals = of_path(&factory.store.db(), &path).map_err(failed)?;
        factory.send(Body::FileSeals(file_seals(&factory.home, &path, &seals)));
    }
    Ok(())
}
