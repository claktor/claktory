//! `factoryd import BUNDLE`: makes this computer's Claktory from a bundle
//! `factoryd export` wrote on another (docs/MOVE.md, #217).
//!
//! An import is resumable and verifying, and never half-starts a Claktory:
//! while `<home>/import/state.json` exists the backend refuses to start (no
//! endpoint, no agent runs), and running the import again carries on where
//! it stopped. Its steps, each safe to repeat:
//!
//! 1. **Unpack** the bundle into `<home>/import/bundle` (`move_bundle`),
//!    checking every file's hash; a folder already unpacked is used as it is.
//! 2. **Check** it can go here: its format and version, a home on a local
//!    disk, no Claktory with work in this home, and free places for the
//!    folders outside it.
//! 3. **Prepare**, in `<home>/import/work`, copies of what changes: settings
//!    and records re-pathed (and migrated), the vault's key on disk rotated,
//!    the old computer's own app made an ordinary paired device. Redone from
//!    the bundle when it didn't finish.
//! 4. **Switch** the home over, one top-level name at a time: what this home
//!    had under that name is put aside in `<home>/import/replaced`.
//! 5. **Folders**: each project folder made again where the map puts it,
//!    with its uncommitted changes; one made by an earlier try is made again.
//! 6. **Checkouts**: each agent's own checkout made again on its branch,
//!    with its uncommitted changes (a patch that doesn't apply is kept).
//! 7. **Tell** each agent where it is now, in one event.
//! 8. **Verify**, then remove the import's folder: from then on Claktory
//!    starts as usual.
//!
//! `--abandon` undoes an unfinished import: what it put in the home goes,
//! what it put aside comes back, and the folders it made are removed.

mod map;
pub mod moved;
pub mod receive;

use std::collections::BTreeSet;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::Command;

use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::config::Config;
use crate::home::Home;
use crate::transcript::now_ms;

pub(crate) use map::PathMap;
pub use map::parse_pair;

/// The bundle formats this reads (`manifest.json`'s `format`).
const FORMAT: u32 = 1;
/// How long the code for the old computer's app works, when that app never
/// connected through the tunnel: it's allowed like any new device.
const OLD_APP_CODE_DAYS: i64 = 7;

/// Records of the moves this Claktory came through, in factory.db.
pub(crate) const SCHEMA: &str = "
-- Moves this Claktory came through (crate::move_import): one row per bundle
-- imported, which is never imported again.
CREATE TABLE moves (
    export_id TEXT PRIMARY KEY,
    -- The computer it moved from, and its version of Claktory.
    machine TEXT NOT NULL,
    version TEXT NOT NULL,
    epoch INTEGER NOT NULL,
    -- The paths it was moved by, as JSON [[from, to], …].
    map TEXT NOT NULL,
    imported INTEGER NOT NULL,
    -- When each agent was told, and when the import was verified complete.
    told INTEGER,
    finished INTEGER
);
";

// ---- the manifest (docs/MOVE.md) ----

#[derive(Clone, Debug, Deserialize)]
pub(crate) struct Manifest {
    pub format: u32,
    pub export_id: String,
    /// The source was tombstoned after writing it (`export --final`).
    #[serde(default, rename = "final")]
    pub is_final: bool,
    pub version: String,
    #[serde(default)]
    pub epoch: u64,
    pub source: Source,
    #[serde(default)]
    pub requires: Requires,
    #[serde(default)]
    pub folders: Vec<Folder>,
    #[serde(default)]
    pub skipped: Vec<Skipped>,
    #[serde(default)]
    pub checkouts: Vec<Checkout>,
    #[serde(default)]
    pub tasks: Vec<Task>,
}

#[derive(Clone, Debug, Deserialize)]
pub(crate) struct Source {
    pub machine: String,
    #[serde(default)]
    pub os: String,
    #[serde(default)]
    pub arch: String,
    pub user_home: PathBuf,
    pub home: PathBuf,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub(crate) struct Requires {
    #[serde(default)]
    pub disk_bytes: u64,
}

#[derive(Clone, Debug, Deserialize)]
pub(crate) struct Folder {
    pub path: PathBuf,
    pub kind: String,
    #[serde(default)]
    pub branch: String,
    #[serde(default)]
    pub head: String,
    #[serde(default)]
    pub remotes: Vec<Remote>,
    pub bundle: Option<String>,
    pub patch: Option<String>,
    pub untracked: Option<String>,
    pub files: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
pub(crate) struct Remote {
    pub name: String,
    pub url: String,
}

#[derive(Clone, Debug, Deserialize)]
pub(crate) struct Skipped {
    pub path: PathBuf,
    #[serde(default)]
    pub why: String,
}

#[derive(Clone, Debug, Deserialize)]
pub(crate) struct Checkout {
    pub agent: String,
    pub repo: PathBuf,
    pub branch: String,
    pub patch: Option<String>,
    pub untracked: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
pub(crate) struct Task {
    pub agent: String,
    pub task: String,
    pub command: String,
    #[serde(default)]
    pub started_ms: i64,
}

// ---- what's asked, and what comes of it ----

/// What `factoryd import` was asked.
pub struct Request {
    pub bundle: PathBuf,
    /// The bundle's passphrase; not needed for a folder already unpacked.
    pub passphrase: Option<String>,
    /// `--map FROM=TO`, in the order given.
    pub map: Vec<(PathBuf, PathBuf)>,
    /// `--same-identity`: import a bundle made without `--final` all the
    /// same, a second Claktory with the first one's identity.
    pub same_identity: bool,
}

/// What an import did, for the person.
#[derive(Debug, Default)]
pub struct Imported {
    pub text: String,
    /// The pairing code for the old computer's own app, if it had one.
    pub old_app_code: Option<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum ImportError {
    /// Something the person can act on, in words for them.
    #[error("{0}")]
    Refused(String),
    #[error("{0}")]
    Failed(String),
}

fn failed(what: &str, error: impl std::fmt::Display) -> ImportError {
    ImportError::Failed(format!("{what}: {error}"))
}

// ---- the import's own state ----

fn dir(home: &Home) -> PathBuf {
    home.root().join("import")
}

fn state_path(home: &Home) -> PathBuf {
    dir(home).join("state.json")
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Step {
    #[default]
    Unpack,
    Prepare,
    Switch,
    Folders,
    Checkouts,
    Tell,
    Verify,
    /// Verified, waiting for the source to commit (a move by `receive`
    /// only): it's finished once the source has moved away.
    Commit,
}

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(default)]
struct State {
    export_id: String,
    bundle: PathBuf,
    /// The bundle was a folder of the person's: read, never moved or changed.
    external: bool,
    /// The bundle comes over the network (`receive`), into `import/bundle`.
    received: bool,
    map: Vec<(PathBuf, PathBuf)>,
    /// The next step to do.
    step: Step,
    /// Top-level names switched into the home, in order.
    switched: Vec<String>,
    /// Folders this import made (and may remove again), and those done.
    made: Vec<PathBuf>,
    done: Vec<PathBuf>,
    /// Agents whose checkouts are made, and what each needs to know.
    checkouts: Vec<(String, String)>,
    old_app_code: Option<String>,
}

fn load_state(home: &Home) -> Result<Option<State>, ImportError> {
    match std::fs::read_to_string(state_path(home)) {
        Ok(text) => serde_json::from_str(&text).map(Some).map_err(|error| failed("The import's state can't be read", error)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(failed("The import's state can't be read", error)),
    }
}

fn save_state(home: &Home, state: &State) -> Result<(), ImportError> {
    let text = serde_json::to_string_pretty(state).map_err(|error| failed("Can't save the import's state", error))?;
    crate::remote::write_private(&state_path(home), text.as_bytes()).map_err(|error| failed("Can't save the import's state", error))
}

/// Why this home's Claktory can't start now: an import into it hasn't
/// finished. None when it can.
pub fn unfinished(home: &Home) -> Option<String> {
    if !state_path(home).exists() {
        return None;
    }
    let received = load_state(home).ok().flatten().is_some_and(|state| state.received);
    Some(if received {
        format!(
            "A move into this Claktory ({}) hasn't finished, so it can't start: run factoryd receive again to finish it, or factoryd import --abandon to undo it.",
            home.root().display()
        )
    } else {
        format!(
            "An import into this Claktory ({}) hasn't finished, so it can't start: run factoryd import again to finish it, or factoryd import --abandon to undo it.",
            home.root().display()
        )
    })
}

/// Whether importing `bundle` into `home` needs its passphrase: a bundle
/// file that isn't unpacked yet.
pub fn needs_passphrase(home: &Home, bundle: &Path) -> bool {
    bundle.is_file() && load_state(home).ok().flatten().is_none_or(|state| state.step == Step::Unpack)
}

/// Holds `<home>/import/lock` while it lives, so two imports never run at once.
struct Lock(#[allow(dead_code)] std::fs::File);

fn lock(home: &Home) -> Result<Lock, ImportError> {
    use std::os::fd::AsRawFd;
    std::fs::create_dir_all(dir(home)).map_err(|error| failed("Can't make the import's folder", error))?;
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(dir(home).join("lock"))
        .map_err(|error| failed("Can't lock the import", error))?;
    // SAFETY: flock(2) on a file this holds open.
    if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
        return Err(ImportError::Refused("Another factoryd import is running for this Claktory.".to_owned()));
    }
    Ok(Lock(file))
}

// ---- the import ----

/// Imports `request.bundle` into `home`, or carries on an import into it
/// that didn't finish. `addr` is where its backend would listen, to make
/// sure none is running.
pub fn import(home: &Home, addr: std::net::SocketAddr, request: &Request) -> Result<Imported, ImportError> {
    let _lock = lock(home)?;
    let imported = carry_on(home, addr, request);
    if imported.is_err() {
        // Nothing in the home changed until it's switched over: an import
        // that stops before then leaves no trace, and Claktory starts as it
        // was. (One that's killed carries on from its state when run again.)
        let untouched = load_state(home).ok().flatten().is_none_or(|state| state.switched.is_empty());
        if untouched {
            let _ = std::fs::remove_file(state_path(home));
            let _ = std::fs::remove_dir_all(dir(home));
        }
    }
    imported
}

fn carry_on(home: &Home, addr: std::net::SocketAddr, request: &Request) -> Result<Imported, ImportError> {
    let mut state = match load_state(home)? {
        Some(state) if state.received => return Err(ImportError::Refused(RECEIVING.to_owned())),
        Some(state) => {
            let same = std::fs::canonicalize(&request.bundle).ok() == std::fs::canonicalize(&state.bundle).ok();
            if !same {
                return Err(ImportError::Refused(format!(
                    "An import of {} into this Claktory hasn't finished. Finish it first (factoryd import with that bundle), or undo it with factoryd import --abandon.",
                    state.bundle.display()
                )));
            }
            state
        }
        None => {
            if running(addr) && !moved::held(home) {
                return Err(running_here(addr));
            }
            fresh(home)?;
            local_disk(home.root())?;
            // A wrong passphrase, or a bundle this can't read, says so now.
            if request.bundle.is_file() {
                let passphrase = request.passphrase.as_deref().unwrap_or_default();
                let manifest = crate::move_bundle::read_manifest(&request.bundle, passphrase)
                    .map_err(|error| ImportError::Refused(error.to_string()))?;
                room(home.root(), manifest.requires.disk_bytes, 0)?;
                if !manifest.is_final && !request.same_identity {
                    return Err(ImportError::Refused(SAME_IDENTITY.to_owned()));
                }
            } else if !request.same_identity && read_manifest(&request.bundle).is_ok_and(|manifest| !manifest.is_final) {
                return Err(ImportError::Refused(SAME_IDENTITY.to_owned()));
            }
            let state = State {
                bundle: std::fs::canonicalize(&request.bundle)
                    .map_err(|error| ImportError::Refused(format!("Can't read {}: {error}", request.bundle.display())))?,
                external: request.bundle.is_dir(),
                ..State::default()
            };
            save_state(home, &state)?;
            state
        }
    };
    not_running(home, addr)?;
    if state.step == Step::Unpack {
        if !state.external {
            unpack(&state.bundle, request.passphrase.as_deref(), &dir(home).join("bundle"))?;
        }
        state.step = Step::Prepare;
        save_state(home, &state)?;
    }
    steps(home, state, &request.map, false, &mut |_, _| {})
}

/// Why a bundle made without `--final` isn't imported unless asked.
const SAME_IDENTITY: &str = "This bundle was made without --final, so the Claktory it came from still runs. Importing it makes a second Claktory with the same identity: the same tunnel key (so your paired devices could reach either copy), sign-ins and account. To move a Claktory, export it with --final. To make the copy anyway, import with --same-identity, and keep only one of them running.";

/// Why `factoryd import` won't touch a move `receive` hasn't finished.
const RECEIVING: &str = "A move into this Claktory over the network hasn't finished: run factoryd receive again to finish it, or factoryd import --abandon to undo it.";

/// What an import is doing, for the person: its step and a sentence.
type Progress<'a> = &'a mut (dyn FnMut(&str, &str) + Send);

/// The import from its unpacked bundle on, step by step from where `state`
/// is. With `hold` (a move by `receive`) it stops once verified, at
/// `Step::Commit`, and `commit` finishes it.
fn steps(home: &Home, mut state: State, given: &[(PathBuf, PathBuf)], hold: bool, progress: Progress) -> Result<Imported, ImportError> {
    let bundle = if state.external { state.bundle.clone() } else { dir(home).join("bundle") };
    let manifest = read_manifest(&bundle)?;
    state.export_id.clone_from(&manifest.export_id);
    // The paths settle when the home is switched; until then a new --map
    // starts the preparation again.
    let pairs = path_map(home, &manifest, given).pairs().to_vec();
    if state.map.is_empty() {
        state.map = pairs;
    } else if !given.is_empty() && state.map != pairs {
        if !state.switched.is_empty() || state.step > Step::Switch {
            return Err(ImportError::Refused(
                "The paths were settled when this import switched the home over. Run it again without --map, or undo it with factoryd import --abandon."
                    .to_owned(),
            ));
        }
        state.map = pairs;
        state.step = Step::Prepare;
    }
    let map = PathMap::new(state.map.clone());
    if state.step == Step::Prepare {
        progress("prepare", "Checking and preparing the settings and records…");
        check(home, &manifest, &map, &bundle)?;
        state.old_app_code = prepare(home, &manifest, &map, &bundle)?;
        state.step = Step::Switch;
        save_state(home, &state)?;
    }
    if state.step == Step::Switch {
        progress("switch", "Putting the Claktory in place…");
        switch(home, &mut state, &bundle)?;
        state.step = Step::Folders;
        save_state(home, &state)?;
    }
    if state.step == Step::Folders {
        progress("folders", "Making the project folders again…");
        for folder in &manifest.folders {
            restore_folder(home, &mut state, folder, &map, &bundle)?;
        }
        state.step = Step::Checkouts;
        save_state(home, &state)?;
    }
    if state.step == Step::Checkouts {
        progress("checkouts", "Making the agents' checkouts again…");
        for checkout in &manifest.checkouts {
            if state.checkouts.iter().any(|(agent, _)| *agent == checkout.agent) {
                continue;
            }
            let said = restore_checkout(home, checkout, &map, &bundle)?;
            state.checkouts.push((checkout.agent.clone(), said));
            save_state(home, &state)?;
        }
        state.step = Step::Tell;
        save_state(home, &state)?;
    }
    if state.step == Step::Tell {
        tell(home, &manifest, &map, &state)?;
        state.step = Step::Verify;
        save_state(home, &state)?;
    }
    progress("verify", "Verifying…");
    verify(home, &manifest, &map)?;
    let imported = report(home, &manifest, &map, &state);
    if hold {
        state.step = Step::Commit;
        save_state(home, &state)?;
    } else {
        finish(home, &manifest, &state)?;
    }
    Ok(imported)
}

fn running(addr: std::net::SocketAddr) -> bool {
    std::net::TcpStream::connect_timeout(&addr, std::time::Duration::from_millis(500)).is_ok()
}

fn running_here(addr: std::net::SocketAddr) -> ImportError {
    ImportError::Refused(format!(
        "Claktory is running here (port {}): stop it first, then import. The installer imports before it starts Claktory.",
        addr.port()
    ))
}

/// Refuses while a Claktory runs here: it would hold the files being
/// replaced. A backend in moved mode holds nothing, so it's stopped (once
/// the import's state exists, what starts again waits for it).
fn not_running(home: &Home, addr: std::net::SocketAddr) -> Result<(), ImportError> {
    if !running(addr) {
        return Ok(());
    }
    if moved::stop_held(home) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while running(addr) && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(200));
        }
        if !running(addr) {
            return Ok(());
        }
    }
    Err(running_here(addr))
}

/// Refuses a home whose Claktory ran and holds work: only a fresh one is
/// replaced, or one that moved away (moving back).
fn fresh(home: &Home) -> Result<(), ImportError> {
    if !home.db_path().exists() || home.root().join("moved.json").exists() {
        return Ok(());
    }
    let db = Connection::open_with_flags(home.db_path(), rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(|error| failed("Can't read this Claktory's records", error))?;
    let sent: i64 = db.query_row("SELECT COUNT(*) FROM messages", [], |row| row.get(0)).unwrap_or(0);
    if sent > 0 {
        return Err(ImportError::Refused(format!(
            "The Claktory in {} already has work in it, so an import won't replace it. Import into a new Claktory instead.",
            home.root().display()
        )));
    }
    Ok(())
}

fn unpack(bundle: &Path, passphrase: Option<&str>, into: &Path) -> Result<(), ImportError> {
    let Some(passphrase) = passphrase else {
        return Err(ImportError::Refused(
            "This bundle needs its passphrase: give it with --passphrase-stdin, or type it when asked.".to_owned(),
        ));
    };
    crate::move_bundle::unpack(bundle, passphrase, into).map(drop).map_err(|error| ImportError::Refused(error.to_string()))
}

fn read_manifest(bundle: &Path) -> Result<Manifest, ImportError> {
    let path = bundle.join("manifest.json");
    let text =
        std::fs::read_to_string(&path).map_err(|error| ImportError::Refused(format!("{} has no manifest: {error}", bundle.display())))?;
    let manifest: Manifest =
        serde_json::from_str(&text).map_err(|error| ImportError::Refused(format!("The bundle's manifest can't be read: {error}")))?;
    if manifest.format != FORMAT {
        return Err(ImportError::Refused(format!(
            "This bundle is in format {} (made by Claktory {}), which this version of Claktory ({}) can't read. Install the latest Claktory here, then import.",
            manifest.format,
            manifest.version,
            crate::update::VERSION
        )));
    }
    Ok(manifest)
}

/// The paths: the source's home to this one, its user's home to this
/// user's, and then the person's own, which win.
fn path_map(home: &Home, manifest: &Manifest, given: &[(PathBuf, PathBuf)]) -> PathMap {
    let user_home = std::env::home_dir().unwrap_or_else(|| home.root().parent().map(Path::to_path_buf).unwrap_or_default());
    let mut pairs = vec![(manifest.source.user_home.clone(), user_home), (manifest.source.home.clone(), home.root().to_path_buf())];
    pairs.extend(given.iter().cloned());
    PathMap::new(pairs)
}

// ---- 2. checks ----

fn check(home: &Home, manifest: &Manifest, map: &PathMap, bundle: &Path) -> Result<(), ImportError> {
    match problems(home, manifest, map, Some(bundle), 0)?.into_iter().next() {
        Some(problem) => Err(ImportError::Refused(problem)),
        None => Ok(()),
    }
}

/// What stops `manifest` from going here, in sentences: its version, its
/// databases' schemas and whether it came before (with the bundle at
/// hand), the folders' places, and the disk, with `staged` bytes of it
/// here already.
fn problems(home: &Home, manifest: &Manifest, map: &PathMap, bundle: Option<&Path>, staged: u64) -> Result<Vec<String>, ImportError> {
    let mut problems = Vec::new();
    let here = crate::update::VERSION;
    if too_new(&manifest.version, here) {
        problems.push(format!(
            "This bundle is from Claktory {}, newer than this one ({here}). Install Claktory {} or later here, then import.",
            manifest.version, manifest.version
        ));
    }
    if let Some(bundle) = bundle {
        problems.extend(bundle_problems(manifest, bundle)?);
    }
    for folder in &manifest.folders {
        let Some(to) = map.path(&folder.path) else {
            problems.push(format!(
                "{} isn't in your home folder, so Claktory can't tell where it goes here: say with --map {}=/where/it/goes.",
                folder.path.display(),
                folder.path.display()
            ));
            continue;
        };
        // Folders in this home are checked once it's switched over.
        if !to.starts_with(home.root()) && !empty_or_missing(&to) {
            problems.push(format!(
                "{} is already there and isn't empty, so {} can't go there. Move it away, or put the folder elsewhere with --map {}=/elsewhere.",
                to.display(),
                folder.path.display(),
                folder.path.display()
            ));
        }
    }
    if let Err(ImportError::Refused(problem) | ImportError::Failed(problem)) = room(home.root(), manifest.requires.disk_bytes, staged) {
        problems.push(problem);
    }
    Ok(problems)
}

/// Refuses when `root`'s disk has less than `needed` bytes free, counting
/// `staged` bytes already there as free.
fn room(root: &Path, needed: u64, staged: u64) -> Result<(), ImportError> {
    let free = free_bytes(root).map(|free| free.saturating_add(staged));
    if needed > 0 && free.is_some_and(|free| free < needed) {
        return Err(ImportError::Refused(format!(
            "There isn't room: this needs {} MB free, and {} has {} MB. Make room, then try again.",
            needed.div_ceil(1 << 20),
            root.display(),
            free.unwrap_or_default() >> 20
        )));
    }
    Ok(())
}

/// What the bundle's own databases say against it.
fn bundle_problems(manifest: &Manifest, bundle: &Path) -> Result<Vec<String>, ImportError> {
    let here = crate::update::VERSION;
    let mut problems = Vec::new();
    for (name, path, known) in [
        ("factory.db", bundle.join("home/factory.db"), crate::store::schema_version()),
        ("the vault", bundle.join("home/vault/vault.db"), crate::vault::schema_version()),
    ] {
        if let Some(version) = user_version(&path)?
            && version > known
        {
            problems.push(format!(
                "This bundle's {name} was made by a newer Claktory than this one ({here}). Install the latest Claktory here, then import."
            ));
        }
    }
    let imported: Option<i64> = Connection::open_with_flags(bundle.join("home/factory.db"), rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
        .ok()
        .and_then(|db| db.query_row("SELECT imported FROM moves WHERE export_id = ?1", [&manifest.export_id], |row| row.get(0)).ok());
    if imported.is_some() {
        problems.push("This bundle was imported before: a bundle is imported once.".to_owned());
    }
    Ok(problems)
}

/// A bundle from `source` is newer than this Claktory (`here`). A build
/// from source ("dev") takes any, and any takes one: their versions say
/// nothing; the databases' schemas are checked all the same.
fn too_new(source: &str, here: &str) -> bool {
    here != "dev" && source != "dev" && crate::update::newer(source, here)
}

fn user_version(path: &Path) -> Result<Option<i64>, ImportError> {
    if !path.exists() {
        return Ok(None);
    }
    let db = Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(|error| failed(&format!("Can't read {}", path.display()), error))?;
    db.pragma_query_value(None, "user_version", |row| row.get(0))
        .map(Some)
        .map_err(|error| failed(&format!("Can't read {}", path.display()), error))
}

fn empty_or_missing(path: &Path) -> bool {
    std::fs::read_dir(path).map_or(true, |mut entries| entries.next().is_none())
}

/// Refuses a home on a network or FUSE disk: SQLite's WAL needs a local one.
fn local_disk(root: &Path) -> Result<(), ImportError> {
    use std::os::unix::ffi::OsStrExt;
    std::fs::create_dir_all(root).map_err(|error| failed(&format!("Can't make {}", root.display()), error))?;
    let Ok(path) = std::ffi::CString::new(root.as_os_str().as_bytes()) else { return Ok(()) };
    // SAFETY: statfs(2) into a zeroed struct of its own type.
    let mut stat: libc::statfs = unsafe { std::mem::zeroed() };
    if unsafe { libc::statfs(path.as_ptr(), &raw mut stat) } != 0 {
        return Ok(());
    }
    if let Some(kind) = network_kind(&stat) {
        return Err(ImportError::Refused(format!(
            "{} is on a network disk ({kind}). Claktory's records need a disk of this computer's own: SQLite doesn't work over a network. Use a home on a local disk.",
            root.display()
        )));
    }
    Ok(())
}

#[cfg(target_os = "macos")]
fn network_kind(stat: &libc::statfs) -> Option<String> {
    // SAFETY: f_fstypename is a NUL-terminated name statfs filled in.
    let name = unsafe { std::ffi::CStr::from_ptr(stat.f_fstypename.as_ptr()) }.to_string_lossy().into_owned();
    ["nfs", "smbfs", "afpfs", "webdav", "cifs", "osxfuse", "macfuse", "fuse"].iter().any(|kind| name.starts_with(kind)).then_some(name)
}

#[cfg(not(target_os = "macos"))]
fn network_kind(stat: &libc::statfs) -> Option<String> {
    #[allow(clippy::unnecessary_cast)]
    let kind = stat.f_type as i64 & 0xffff_ffff;
    match kind {
        0x6969 => Some("NFS".to_owned()),
        0x517B | 0xFE53_4D42 | 0xFF53_4D42 => Some("SMB".to_owned()),
        0x6573_5546 => Some("FUSE".to_owned()),
        0x0102_1997 => Some("9P".to_owned()),
        _ => None,
    }
}

fn free_bytes(root: &Path) -> Option<u64> {
    use std::os::unix::ffi::OsStrExt;
    let path = std::ffi::CString::new(root.as_os_str().as_bytes()).ok()?;
    // SAFETY: statvfs(3) into a zeroed struct of its own type.
    let mut stat: libc::statvfs = unsafe { std::mem::zeroed() };
    if unsafe { libc::statvfs(path.as_ptr(), &raw mut stat) } != 0 {
        return None;
    }
    #[allow(clippy::useless_conversion, clippy::unnecessary_cast)]
    Some(u64::from(stat.f_bavail as u64) * u64::from(stat.f_frsize as u64))
}

// ---- 3. prepare ----

/// What prepare changes, copied from the bundle into `<home>/import/work`.
const CHANGED: [&str; 4] = ["config.json", "factory.db", "vault", "remote"];

/// Makes the changed copies in `<home>/import/work` from the bundle, anew;
/// returns the code for the old computer's app, if it had one.
fn prepare(home: &Home, manifest: &Manifest, map: &PathMap, bundle: &Path) -> Result<Option<String>, ImportError> {
    let work = dir(home).join("work");
    if work.exists() {
        std::fs::remove_dir_all(&work).map_err(|error| failed("Can't clear the import's work folder", error))?;
    }
    std::fs::create_dir_all(&work).map_err(|error| failed("Can't make the import's work folder", error))?;
    for name in CHANGED {
        let from = bundle.join("home").join(name);
        if from.exists() {
            copy_all(&from, &work.join(name)).map_err(|error| failed(&format!("Can't copy {name}"), error))?;
        }
    }
    let work_home = Home::new(work.clone());
    repath_config(&work_home, map)?;
    repath_records(&work_home, manifest, map)?;
    if work.join("vault/vault.db").exists() {
        let vault = crate::vault::Vault::open(work.join("vault"));
        vault.rotate_auto_key().map_err(|error| failed("Can't give the vault a new key", error))?;
    }
    let code = old_app(&work_home)?;
    crate::remote::write_private(&work.join("move-epoch"), format!("{}\n", manifest.epoch).as_bytes())
        .map_err(|error| failed("Can't write the move's epoch", error))?;
    Ok(code)
}

fn copy_all(from: &Path, to: &Path) -> std::io::Result<()> {
    let meta = std::fs::symlink_metadata(from)?;
    if meta.is_dir() {
        std::fs::create_dir_all(to)?;
        std::fs::set_permissions(to, meta.permissions())?;
        for entry in std::fs::read_dir(from)? {
            let entry = entry?;
            copy_all(&entry.path(), &to.join(entry.file_name()))?;
        }
    } else if meta.file_type().is_symlink() {
        std::os::unix::fs::symlink(std::fs::read_link(from)?, to)?;
    } else {
        std::fs::copy(from, to)?;
    }
    Ok(())
}

/// Project and folder paths in config.json.
fn repath_config(home: &Home, map: &PathMap) -> Result<(), ImportError> {
    let path = home.config_path();
    let Ok(text) = std::fs::read_to_string(&path) else { return Ok(()) };
    let mut config = Config::parse(&text).map_err(|error| failed("The bundle's settings can't be read", error))?;
    for project in &mut config.projects {
        project.path = map.path_or_same(&project.path);
        for folder in &mut project.folders {
            folder.path = map.path_or_same(&folder.path);
        }
    }
    let text = config.to_json().map_err(|error| failed("Can't write the settings", error))?;
    crate::remote::write_private(&path, text.as_bytes()).map_err(|error| failed("Can't write the settings", error))
}

/// factory.db, migrated to this version, with its paths moved: monitors'
/// commands and files, tasks' logs, comments' files. Background tasks the
/// source was running are over. The move is recorded.
fn repath_records(home: &Home, manifest: &Manifest, map: &PathMap) -> Result<(), ImportError> {
    if !home.db_path().exists() {
        return Ok(());
    }
    let store = crate::store::Store::open(&home.db_path()).map_err(|error| failed("The bundle's records can't be opened", error))?;
    let mut db = store.db();
    let records = |error: rusqlite::Error| failed("Can't move the records' paths", error);
    let transaction = db.transaction().map_err(records)?;
    let rows: Vec<(String, i64, String, String)> = {
        let mut statement = transaction.prepare("SELECT agent, id, spec, cursor FROM monitors").map_err(records)?;
        statement.query_map([], crate::store::row4).map_err(records)?.collect::<Result<_, _>>().map_err(records)?
    };
    for (agent, id, spec, cursor) in rows {
        let moved = match serde_json::from_str::<serde_json::Value>(&spec) {
            Ok(value) => repath_json(value, map).to_string(),
            Err(_) => map.text(&spec),
        };
        // A watch's process stayed on the old computer: its pid here is
        // nobody's, or another process's, so it's started again.
        let cursor = match serde_json::from_str::<serde_json::Value>(&cursor) {
            Ok(serde_json::Value::Object(mut fields)) => {
                fields.remove("pid");
                fields.remove("started");
                repath_json(serde_json::Value::Object(fields), map).to_string()
            }
            _ => cursor,
        };
        transaction
            .execute("UPDATE monitors SET spec = ?3, cursor = ?4 WHERE agent = ?1 AND id = ?2", params![agent, id, moved, cursor])
            .map_err(records)?;
    }
    let logs: Vec<(i64, String)> = {
        let mut statement = transaction.prepare("SELECT id, log FROM tasks").map_err(records)?;
        statement.query_map([], crate::store::row2).map_err(records)?.collect::<Result<_, _>>().map_err(records)?
    };
    for (id, log) in logs {
        let moved = map.path_or_same(Path::new(&log)).display().to_string();
        if moved != log {
            transaction.execute("UPDATE tasks SET log = ?2 WHERE id = ?1", params![id, moved]).map_err(records)?;
        }
    }
    transaction.execute("UPDATE tasks SET ended = ?1, status = 'stopped by the move' WHERE ended IS NULL", [now_ms()]).map_err(records)?;
    for table in ["threads", "thread_files"] {
        let paths: Vec<String> = {
            let mut statement = transaction.prepare(&format!("SELECT DISTINCT path FROM {table}")).map_err(records)?;
            statement.query_map([], |row| row.get(0)).map_err(records)?.collect::<Result<_, _>>().map_err(records)?
        };
        for path in paths {
            let moved = map.path_or_same(Path::new(&path)).display().to_string();
            if moved != path {
                transaction.execute(&format!("UPDATE {table} SET path = ?2 WHERE path = ?1"), params![path, moved]).map_err(records)?;
            }
        }
    }
    let pairs: Vec<(String, String)> =
        map.pairs().iter().map(|(from, to)| (from.display().to_string(), to.display().to_string())).collect();
    transaction
        .execute(
            "INSERT OR REPLACE INTO moves (export_id, machine, version, epoch, map, imported) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                manifest.export_id,
                manifest.source.machine,
                manifest.version,
                i64::try_from(manifest.epoch).unwrap_or(i64::MAX),
                serde_json::to_string(&pairs).unwrap_or_default(),
                now_ms()
            ],
        )
        .map_err(records)?;
    transaction.commit().map_err(records)?;
    // One file, without its WAL, to move into place.
    db.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);").map_err(records)?;
    Ok(())
}

fn repath_json(value: serde_json::Value, map: &PathMap) -> serde_json::Value {
    use serde_json::Value;
    match value {
        Value::String(text) => Value::String(map.text(&text)),
        Value::Array(items) => Value::Array(items.into_iter().map(|item| repath_json(item, map)).collect()),
        Value::Object(fields) => Value::Object(fields.into_iter().map(|(key, value)| (key, repath_json(value, map))).collect()),
        other => other,
    }
}

/// The old computer's own app becomes an ordinary paired device: its token
/// (from the old local pairing code) is kept, so its saved connection keeps
/// working through the tunnel, and a code with it is returned to give it.
/// This computer's app gets a new local pairing when Claktory starts.
fn old_app(work: &Home) -> Result<Option<String>, ImportError> {
    let devices_path = work.root().join("remote/devices.json");
    let Ok(bytes) = std::fs::read(&devices_path) else { return Ok(None) };
    let mut devices: Vec<crate::remote::Device> =
        serde_json::from_slice(&bytes).map_err(|error| failed("The bundle's devices can't be read", error))?;
    let pairing = work.root().join("remote/local-pairing");
    let token = std::fs::read_to_string(&pairing)
        .ok()
        .and_then(|code| crate::remote::code_values(&code, "t").next())
        .filter(|token| !token.is_empty());
    let mut code = None;
    if let Some(index) = devices.iter().position(|device| device.local) {
        let mut device = devices.remove(index);
        let base = {
            let base: String = device.name.to_lowercase().chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '-' }).collect();
            let base = base.trim_matches('-').to_owned();
            if base.is_empty() || base == crate::remote::LOCAL_DEVICE { "old-computer".to_owned() } else { base }
        };
        let mut id = base.clone();
        let mut n = 2;
        while devices.iter().any(|known| known.id == id) {
            id = format!("{base}-{n}");
            n += 1;
        }
        device.id = id;
        device.local = false;
        match &token {
            Some(token) if sha256_hex(token) == device.token_sha256 => {
                if device.endpoint_id.is_empty() {
                    // Never connected through the tunnel: its code lets it in,
                    // allowed like any new device's.
                    device.code_expires = Some(now_ms() + OLD_APP_CODE_DAYS * 24 * 60 * 60 * 1000);
                }
                let key = crate::remote::secret_key(work).map_err(|error| failed("The bundle's tunnel key can't be read", error))?;
                let config =
                    std::fs::read_to_string(work.config_path()).ok().and_then(|text| Config::parse(&text).ok()).unwrap_or_default();
                let relays = config.remote.as_ref().map(|remote| remote.relays.clone()).unwrap_or_default();
                let identity = crate::remote::Identity {
                    name: config.claktory_name(),
                    color: config.claktory_color(crate::remote::key_color(work)).to_owned(),
                };
                code = Some(crate::remote::pairing_code(token, &key.public().to_string(), &relays, &[], &identity));
            }
            // No token to give it: it's paired again like any device.
            _ => {}
        }
        if code.is_some() {
            devices.push(device);
        }
    }
    let mut json = serde_json::to_string_pretty(&devices).map_err(|error| failed("Can't write the devices", error))?;
    json.push('\n');
    crate::remote::write_private(&devices_path, json.as_bytes()).map_err(|error| failed("Can't write the devices", error))?;
    // This computer's own pairing and port are made again here.
    for name in ["local-pairing", "remote-port"] {
        let _ = std::fs::remove_file(work.root().join("remote").join(name));
    }
    Ok(code)
}

// ---- 4. switch ----

/// Never put aside: this computer's own Claktory programs and the import.
const KEPT: [&str; 4] = ["bin", "releases", "models", "import"];

/// Moves the bundle's home into this one, name by name; what was here
/// under each name goes to `<home>/import/replaced`. Safe to run again.
fn switch(home: &Home, state: &mut State, bundle: &Path) -> Result<(), ImportError> {
    let work = dir(home).join("work");
    let replaced = dir(home).join("replaced");
    std::fs::create_dir_all(&replaced).map_err(|error| failed("Can't make the import's folder", error))?;
    let mut names: BTreeSet<String> = BTreeSet::new();
    for from in [bundle.join("home"), work.clone()] {
        if let Ok(entries) = std::fs::read_dir(&from) {
            names.extend(entries.filter_map(|entry| entry.ok()?.file_name().into_string().ok()));
        }
    }
    names.retain(|name| !KEPT.contains(&name.as_str()));
    // What this home had that's stale with the bundle's: the records' WAL,
    // and a tombstone when it's moving back.
    for name in ["factory.db-wal", "factory.db-shm", "moved.json", "moved"] {
        names.insert(name.to_owned());
    }
    for name in &names {
        let target = home.root().join(name);
        let source = if work.join(name).exists() { work.join(name) } else { bundle.join("home").join(name) };
        let switched = state.switched.contains(name);
        if switched && !source.exists() {
            continue;
        }
        if !switched && std::fs::symlink_metadata(&target).is_ok() {
            let aside = replaced.join(name);
            if aside.exists() {
                return Err(ImportError::Failed(format!("{} is in the way of {}", aside.display(), target.display())));
            }
            std::fs::rename(&target, &aside).map_err(|error| failed(&format!("Can't put {} aside", target.display()), error))?;
        }
        state.switched.push(name.clone());
        state.switched.dedup();
        save_state(home, state)?;
        if !source.exists() {
            continue;
        }
        if state.external && !source.starts_with(&work) {
            // The person's folder stays as it is: a copy goes in.
            if target.exists() {
                std::fs::remove_dir_all(&target)
                    .or_else(|_| std::fs::remove_file(&target))
                    .map_err(|error| failed("Can't clear a copy", error))?;
            }
            copy_all(&source, &target).map_err(|error| failed(&format!("Can't copy {name}"), error))?;
        } else {
            if std::fs::symlink_metadata(&target).is_ok() {
                return Err(ImportError::Failed(format!("{} is in the way", target.display())));
            }
            std::fs::rename(&source, &target).map_err(|error| failed(&format!("Can't move {name} into place"), error))?;
        }
    }
    Ok(())
}

// ---- 5. folders ----

fn git(dir: &Path, args: &[&str]) -> Result<String, String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["-c", "core.hooksPath=/dev/null"])
        .args(args)
        .output()
        .map_err(|error| format!("git can't run: {error}"))?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
    } else {
        Err(format!("git {}: {}", args.join(" "), String::from_utf8_lossy(&output.stderr).trim()))
    }
}

fn restore_folder(home: &Home, state: &mut State, folder: &Folder, map: &PathMap, bundle: &Path) -> Result<(), ImportError> {
    let to = map.path_or_same(&folder.path);
    if state.done.contains(&to) {
        return Ok(());
    }
    let ours = state.made.contains(&to);
    if ours {
        // Made by a try that didn't finish: made again.
        std::fs::remove_dir_all(&to).or_else(ignore_missing).map_err(|error| failed(&format!("Can't clear {}", to.display()), error))?;
    } else if !empty_or_missing(&to) {
        return Err(ImportError::Refused(format!(
            "{} is already there and isn't empty, so {} can't go there. Move it away, or undo this import with factoryd import --abandon and import again with --map {}=/elsewhere.",
            to.display(),
            folder.path.display(),
            folder.path.display()
        )));
    } else {
        state.made.push(to.clone());
        save_state(home, state)?;
    }
    std::fs::create_dir_all(&to).map_err(|error| failed(&format!("Can't make {}", to.display()), error))?;
    let part = |name: &Option<String>| name.as_deref().map(|name| bundle.join(name)).filter(|path| path.exists());
    let made = |error: String| ImportError::Failed(format!("Can't make {} again: {error}", to.display()));
    if folder.kind == "git" {
        git(&to, &["init", "-q"]).map_err(made)?;
        if let Some(repo) = part(&folder.bundle) {
            let repo = repo.to_string_lossy();
            git(&to, &["fetch", "-q", "--update-head-ok", &repo, "refs/*:refs/*"]).map_err(made)?;
            if folder.branch.is_empty() {
                git(&to, &["checkout", "-q", "--detach", &folder.head]).map_err(made)?;
            } else {
                git(&to, &["symbolic-ref", "HEAD", &format!("refs/heads/{}", folder.branch)]).map_err(made)?;
                git(&to, &["reset", "-q", "--hard"]).map_err(made)?;
            }
        } else if !folder.branch.is_empty() {
            git(&to, &["symbolic-ref", "HEAD", &format!("refs/heads/{}", folder.branch)]).map_err(made)?;
        }
        for remote in &folder.remotes {
            // A remote the bundle's refs named is there already.
            let _ = git(&to, &["remote", "add", &remote.name, &remote.url]);
        }
        if let Some(patch) = part(&folder.patch) {
            git(&to, &["apply", "--binary", "--whitespace=nowarn", &patch.to_string_lossy()]).map_err(made)?;
        }
        if let Some(untracked) = part(&folder.untracked) {
            copy_into(&untracked, &to).map_err(|error| made(error.to_string()))?;
        }
    } else if let Some(files) = part(&folder.files) {
        copy_into(&files, &to).map_err(|error| made(error.to_string()))?;
    }
    state.done.push(to);
    save_state(home, state)
}

fn ignore_missing(error: std::io::Error) -> std::io::Result<()> {
    if error.kind() == std::io::ErrorKind::NotFound { Ok(()) } else { Err(error) }
}

/// Copies what's in `from` into `to`, over what's there.
fn copy_into(from: &Path, to: &Path) -> std::io::Result<()> {
    for entry in std::fs::read_dir(from)? {
        let entry = entry?;
        let target = to.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            std::fs::create_dir_all(&target)?;
            copy_into(&entry.path(), &target)?;
        } else {
            if std::fs::symlink_metadata(&target).is_ok() {
                std::fs::remove_file(&target)?;
            }
            copy_all(&entry.path(), &target)?;
        }
    }
    Ok(())
}

// ---- 6. checkouts ----

/// Makes the agent's checkout again on its branch, with its uncommitted
/// work; says what the agent needs to know of it.
fn restore_checkout(home: &Home, checkout: &Checkout, map: &PathMap, bundle: &Path) -> Result<String, ImportError> {
    let repo = map.path_or_same(&checkout.repo);
    if !repo.join(".git").exists() {
        return Ok(format!("Your own checkout wasn't made again: its repository, {}, didn't move.", repo.display()));
    }
    let Ok(plan) = crate::trees::plan(home, &checkout.agent, &tree_source(home, checkout, &repo)) else {
        return Ok(format!("Your own checkout wasn't made again: {} isn't a repository here.", repo.display()));
    };
    // A home of another name has other branch names: the agent's branch
    // carries on under this home's.
    if plan.branch != checkout.branch && git(&repo, &["rev-parse", "--verify", "-q", &format!("refs/heads/{}", plan.branch)]).is_err() {
        let _ = git(&repo, &["branch", "-m", &checkout.branch, &plan.branch]);
    }
    let tree = plan.tree.clone();
    // Made again whole, so a patch is never applied twice.
    let _ = git(&repo, &["worktree", "remove", "--force", &tree.to_string_lossy()]);
    let _ = std::fs::remove_dir_all(&tree);
    let _ = std::fs::remove_file(tree.with_extension("json"));
    let _ = git(&repo, &["worktree", "prune"]);
    crate::trees::ensure(&plan).map_err(|error| ImportError::Failed(format!("Can't make {}'s checkout again: {error}", checkout.agent)))?;
    let part = |name: &Option<String>| name.as_deref().map(|name| bundle.join(name)).filter(|path| path.exists());
    let mut said = format!("Your checkout is at {}, on {}", tree.display(), plan.branch);
    if let Some(patch) = part(&checkout.patch) {
        match git(&tree, &["apply", "--binary", "--whitespace=nowarn", &patch.to_string_lossy()]) {
            Ok(_) => said.push_str(", with your uncommitted changes"),
            Err(error) => {
                let kept = tree.with_extension("moved.patch");
                std::fs::copy(&patch, &kept).map_err(|error| failed("Can't keep the checkout's changes", error))?;
                tracing::warn!(agent = %checkout.agent, %error, "a checkout's changes didn't apply");
                let _ = write!(said, ". Your uncommitted changes didn't apply to it; they're kept at {}", kept.display());
            }
        }
    }
    if let Some(untracked) = part(&checkout.untracked) {
        copy_into(&untracked, &tree).map_err(|error| failed("Can't put the checkout's untracked files back", error))?;
        said.push_str(if said.contains("uncommitted changes didn't") {
            ", and your untracked files are in it"
        } else {
            " and untracked files"
        });
    }
    said.push('.');
    Ok(said)
}

/// The folder the agent works in, from the moved settings: its checkout's
/// source; the repository itself if the settings don't say.
fn tree_source(home: &Home, checkout: &Checkout, repo: &Path) -> PathBuf {
    let config = std::fs::read_to_string(home.config_path()).ok().and_then(|text| Config::parse(&text).ok());
    config
        .as_ref()
        .and_then(|config| config.agent(&checkout.agent).map(|agent| crate::runner::source_folder(home, config, agent)))
        .filter(|source| source.starts_with(repo))
        .unwrap_or_else(|| repo.to_path_buf())
}

// ---- 7. tell ----

/// One event for each agent: where it is now, the paths, its checkout, the
/// tasks the move stopped, and the tools missing here. Once.
fn tell(home: &Home, manifest: &Manifest, map: &PathMap, state: &State) -> Result<(), ImportError> {
    let config = std::fs::read_to_string(home.config_path()).ok().and_then(|text| Config::parse(&text).ok()).unwrap_or_default();
    let mut db = Connection::open(home.db_path()).map_err(|error| failed("Can't open the records", error))?;
    let records = |error: rusqlite::Error| failed("Can't tell the agents", error);
    let transaction = db.transaction().map_err(records)?;
    let told: Option<Option<i64>> = transaction
        .query_row("SELECT told FROM moves WHERE export_id = ?1", [&manifest.export_id], |row| row.get(0))
        .optional()
        .map_err(records)?;
    if matches!(told, Some(Some(_))) {
        return Ok(());
    }
    let here = format!("{} ({})", crate::remote::machine_name(), platform(std::env::consts::OS, std::env::consts::ARCH));
    let from = format!("{} ({})", manifest.source.machine, platform(&manifest.source.os, &manifest.source.arch));
    let at = jiff::ZonedDateTime::now().strftime("%H:%M on %-d %b").to_string();
    let now = now_ms();
    for agent in &config.agents {
        let mut text = format!("Claktory moved from {from} to {here} at {at}.\n");
        let said = map.said();
        if !said.is_empty() {
            let _ = writeln!(text, "Paths: {said}. Your conversation is as it was: paths in it are the old computer's.");
        }
        if let Some((_, checkout)) = state.checkouts.iter().find(|(id, _)| *id == agent.id) {
            let _ = writeln!(text, "{checkout}");
        }
        let stopped: Vec<String> = manifest
            .tasks
            .iter()
            .filter(|task| task.agent == agent.id)
            .map(|task| {
                let started = jiff::Timestamp::from_millisecond(task.started_ms)
                    .map(|at| at.to_zoned_datetime(jiff::tz::TimeZone::system()).strftime("%H:%M").to_string())
                    .unwrap_or_default();
                format!("{} ({}), started {started}", task.task, short(&task.command))
            })
            .collect();
        if !stopped.is_empty() {
            let _ = writeln!(text, "Stopped by the move: {}. Start them again if you still need them.", stopped.join("; "));
        }
        let missing = missing_tools(&transaction, &agent.id);
        if !missing.is_empty() {
            let _ = writeln!(text, "Missing on this computer: {}.", missing.join(", "));
        }
        text.push_str("Build output and caches weren't moved: the first build will be slow.");
        let summary = format!("Claktory moved to {}.", crate::remote::machine_name());
        transaction
            .execute(
                "INSERT INTO messages (agent, source, sender, sender_id, summary, text, created) VALUES (?1, 'factory', 'Claktory', '', ?2, ?3, ?4)",
                params![agent.id, summary, text, now],
            )
            .map_err(records)?;
    }
    transaction.execute("UPDATE moves SET told = ?2 WHERE export_id = ?1", params![manifest.export_id, now]).map_err(records)?;
    transaction.commit().map_err(records)
}

fn short(command: &str) -> String {
    let line = command.lines().next().unwrap_or_default();
    if line.chars().count() > 60 { format!("{}…", line.chars().take(60).collect::<String>()) } else { line.to_owned() }
}

/// "macOS arm64", "Linux x86_64".
fn platform(os: &str, arch: &str) -> String {
    let os = match os {
        "macos" => "macOS",
        "linux" => "Linux",
        "" => "an unknown system",
        other => other,
    };
    let arch = if arch == "aarch64" && os == "macOS" { "arm64" } else { arch };
    if arch.is_empty() { os.to_owned() } else { format!("{os} {arch}") }
}

/// Tools the agent's background tasks used that this computer hasn't got.
fn missing_tools(db: &Connection, agent: &str) -> Vec<String> {
    const TOOLS: [&str; 20] = [
        "brew",
        "cargo",
        "codesign",
        "docker",
        "flutter",
        "gh",
        "git",
        "go",
        "node",
        "npm",
        "notarytool",
        "pnpm",
        "podman",
        "python3",
        "rustup",
        "security",
        "sqlite3",
        "swift",
        "xcodebuild",
        "xcrun",
    ];
    let commands: Vec<String> = db
        .prepare("SELECT command FROM tasks WHERE agent = ?1 ORDER BY id DESC LIMIT 500")
        .and_then(|mut statement| statement.query_map([agent], |row| row.get(0))?.collect())
        .unwrap_or_default();
    let used: BTreeSet<&str> = commands
        .iter()
        .flat_map(|command| command.split(|c: char| c.is_whitespace() || matches!(c, ';' | '&' | '|' | '(' | ')' | '`')))
        .filter_map(|word| word.rsplit('/').next())
        .filter(|word| TOOLS.contains(word))
        .collect();
    used.into_iter().filter(|tool| !on_path(tool)).map(str::to_owned).collect()
}

fn on_path(tool: &str) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::env::var_os("PATH").is_some_and(|path| {
        std::env::split_paths(&path)
            .any(|dir| std::fs::metadata(dir.join(tool)).is_ok_and(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0))
    })
}

// ---- 8. verify and finish ----

fn verify(home: &Home, manifest: &Manifest, map: &PathMap) -> Result<(), ImportError> {
    let broken = |what: String| ImportError::Failed(format!("The import didn't verify: {what}. Run factoryd import again."));
    if home.config_path().exists() {
        let text = std::fs::read_to_string(home.config_path()).map_err(|error| broken(format!("the settings can't be read ({error})")))?;
        Config::parse(&text).map_err(|error| broken(format!("the settings can't be read ({error})")))?;
    }
    let db = Connection::open(home.db_path()).map_err(|error| broken(format!("the records can't be opened ({error})")))?;
    let check: String = db.query_row("PRAGMA quick_check", [], |row| row.get(0)).map_err(|error| broken(error.to_string()))?;
    if check != "ok" {
        return Err(broken(format!("the records are damaged ({check})")));
    }
    let known: Option<i64> =
        db.query_row("SELECT imported FROM moves WHERE export_id = ?1", [&manifest.export_id], |row| row.get(0)).optional().unwrap_or(None);
    if known.is_none() {
        return Err(broken("the records don't hold the move".to_owned()));
    }
    let vault = home.root().join("vault");
    if vault.join("vault.db").exists() && vault.join("auto.key").exists() {
        let opened = crate::vault::Vault::open(vault);
        if !opened.is_open() {
            return Err(broken("the vault doesn't open with its new key".to_owned()));
        }
    }
    for folder in &manifest.folders {
        let to = map.path_or_same(&folder.path);
        if !to.is_dir() {
            return Err(broken(format!("{} isn't there", to.display())));
        }
    }
    Ok(())
}

fn finish(home: &Home, manifest: &Manifest, state: &State) -> Result<(), ImportError> {
    if let Ok(db) = Connection::open(home.db_path()) {
        let _ =
            db.execute("UPDATE moves SET finished = ?2 WHERE export_id = ?1 AND finished IS NULL", params![manifest.export_id, now_ms()]);
    }
    // What this home had before goes to the trash, never deleted.
    let replaced = dir(home).join("replaced");
    if !empty_or_missing(&replaced) {
        let trash = home.root().join("trash").join(format!("replaced-by-import-{}", now_ms()));
        if let Some(parent) = trash.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        std::fs::rename(&replaced, &trash).map_err(|error| failed("Can't put the replaced files in the trash", error))?;
    }
    let _ = state;
    // Last: from now on Claktory starts as usual.
    let _ = std::fs::remove_file(state_path(home));
    std::fs::remove_dir_all(dir(home)).map_err(|error| failed("Can't remove the import's folder", error))
}

fn report(home: &Home, manifest: &Manifest, map: &PathMap, state: &State) -> Imported {
    let mut text =
        format!("Imported the Claktory from {} (Claktory {}) into {}.\n", manifest.source.machine, manifest.version, home.root().display());
    let said = map.said();
    if !said.is_empty() {
        let _ = writeln!(text, "Paths: {said}.");
    }
    for folder in &manifest.folders {
        let _ = writeln!(text, "Folder: {}", map.path_or_same(&folder.path).display());
    }
    for skipped in &manifest.skipped {
        let _ = writeln!(text, "Not moved: {} ({})", skipped.path.display(), skipped.why);
    }
    for (agent, said) in &state.checkouts {
        let _ = writeln!(text, "{agent}: {said}");
    }
    if !manifest.tasks.is_empty() {
        let _ = writeln!(text, "Background tasks stopped by the move: {}. Each agent was told.", manifest.tasks.len());
    }
    if let Some(code) = &state.old_app_code {
        let _ = writeln!(
            text,
            "\nOn {}, Claktory's app can reach this Claktory with this code (Add a Claktory, the + at the far left of the window):\n\n{code}\n",
            manifest.source.machine
        );
    }
    if !state.received {
        text.push_str(
            "Run this Claktory in one place only: if this bundle was imported elsewhere too, or the Claktory it came from still runs, keep one and delete the other.\n",
        );
    }
    text.push_str("Start Claktory to carry on: its agents are told where they are now.");
    Imported { text, old_app_code: state.old_app_code.clone() }
}

/// Undoes an unfinished import: what it put in the home goes, what it put
/// aside comes back, the folders it made are removed, and its folder goes.
pub fn abandon(home: &Home) -> Result<String, ImportError> {
    let _lock = lock(home)?;
    undo(home)
}

/// `abandon`, with the import's lock held.
fn undo(home: &Home) -> Result<String, ImportError> {
    let Some(state) = load_state(home)? else {
        let _ = std::fs::remove_dir_all(dir(home));
        return Ok("There's no unfinished import here.".to_owned());
    };
    let replaced = dir(home).join("replaced");
    for name in state.switched.iter().rev() {
        let target = home.root().join(name);
        if std::fs::symlink_metadata(&target).is_ok() {
            std::fs::remove_dir_all(&target)
                .or_else(|_| std::fs::remove_file(&target))
                .map_err(|error| failed(&format!("Can't remove {}", target.display()), error))?;
        }
        let aside = replaced.join(name);
        if std::fs::symlink_metadata(&aside).is_ok() {
            std::fs::rename(&aside, &target).map_err(|error| failed(&format!("Can't put {} back", target.display()), error))?;
        }
    }
    for folder in state.made.iter().rev() {
        std::fs::remove_dir_all(folder)
            .or_else(ignore_missing)
            .map_err(|error| failed(&format!("Can't remove {}", folder.display()), error))?;
    }
    // The checkouts it made (their repositories, which it made too, are gone).
    let trees = crate::trees::trees_dir(home);
    for (agent, _) in &state.checkouts {
        let _ = std::fs::remove_dir_all(trees.join(agent));
    }
    let _ = std::fs::remove_file(state_path(home));
    std::fs::remove_dir_all(dir(home)).map_err(|error| failed("Can't remove the import's folder", error))?;
    if state.received {
        return Ok("Undid the unfinished move: this Claktory is as it was before it.".to_owned());
    }
    let of = if state.bundle.as_os_str().is_empty() { String::new() } else { format!(" of {}", state.bundle.display()) };
    Ok(format!("Undid the unfinished import{of}: this Claktory is as it was before it."))
}

/// Asks for the bundle's passphrase in the terminal, without showing it.
pub fn ask_passphrase() -> Result<String, String> {
    use std::io::{BufRead, Write};
    use std::os::fd::AsRawFd;
    let tty = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open("/dev/tty")
        .map_err(|_| "This bundle needs its passphrase: give it with --passphrase-stdin.".to_owned())?;
    let fd = tty.as_raw_fd();
    // SAFETY: termios of the terminal this holds open, put back below.
    let mut before: libc::termios = unsafe { std::mem::zeroed() };
    let hidden = unsafe { libc::tcgetattr(fd, &raw mut before) } == 0 && {
        let mut quiet = before;
        quiet.c_lflag &= !libc::ECHO;
        unsafe { libc::tcsetattr(fd, libc::TCSANOW, &raw const quiet) == 0 }
    };
    let mut out = &tty;
    let _ = write!(out, "The bundle's passphrase (shown when it was made): ");
    let _ = out.flush();
    let mut line = String::new();
    let read = std::io::BufReader::new(&tty).read_line(&mut line);
    if hidden {
        // SAFETY: as above.
        unsafe { libc::tcsetattr(fd, libc::TCSANOW, &raw const before) };
        let _ = writeln!(out);
    }
    read.map_err(|error| format!("Can't read the passphrase: {error}"))?;
    Ok(line.trim().to_owned())
}

/// The SHA-256 of `text`, in hex.
pub(crate) fn sha256_hex(text: &str) -> String {
    Sha256::digest(text.as_bytes()).iter().map(|byte| format!("{byte:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn platforms_read_as_people_say_them() {
        assert_eq!(platform("macos", "aarch64"), "macOS arm64");
        assert_eq!(platform("linux", "x86_64"), "Linux x86_64");
        assert_eq!(short("cargo test --workspace\nmore"), "cargo test --workspace");
    }

    #[test]
    fn a_bundle_from_a_newer_claktory_is_too_new() {
        assert!(too_new("v0.1.15", "v0.1.14"));
        assert!(!too_new("v0.1.14", "v0.1.14") && !too_new("v0.1.13", "v0.1.14"));
        assert!(too_new("v0.2.0", "v0.1.99"));
        assert!(!too_new("v9.0.0", "dev") && !too_new("dev", "v0.1.14"));
    }
}
