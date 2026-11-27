//! macOS's protected places (permissions.md P2, item 12): Documents,
//! Desktop, Downloads, iCloud Drive and volumes, which macOS keeps from
//! Claktory Agents until the person lets it in (Files and Folders). macOS
//! has no way to ask whether it may, so Claktory keeps what it last found
//! (Workspace.protected_places): a place a project or folder works in, as
//! a check found it, and any place an agent's tool was refused in.
//!
//! - An agent's tool result saying macOS refused it (the tools'
//!   `protected_refused`, factory-agent's `tools::protected`) is believed
//!   only for a place `protected_root` names again.
//! - Adding or pointing a project's folder at such a place reads it at once
//!   (`touch`), so macOS asks while the person is there, after the app's
//!   sheet said so; one refused already comes back refused.
//! - The app asks for a check (CheckProtectedPlaces): its one-time sheet for
//!   places never checked, and on coming back to the front for refused ones,
//!   so a place let in from System Settings shows On. Reading a place macOS
//!   has already decided on asks the person nothing.
//! - A refusal raises one Needs you item for its place, until the person's
//!   OK (DismissProtectedPlace) or a check that finds it let in; it comes
//!   again only after one found it let in and a later one refused it. Each
//!   is kept for History (NEEDS_YOU_RECORD_KIND_PROTECTED_PLACE).
//!
//! On any system but macOS no place is protected, and none is listed.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use factory_protocol::v1;
use rusqlite::{Connection, OptionalExtension as _, params};

use crate::config::Config;
use crate::factory::{Factory, RequestError};
use crate::transcript::now_ms;

pub(crate) const SCHEMA: &str = "
-- Places macOS keeps private (Files and Folders) as Claktory last found
-- them (permissions.md P2, item 12): one row per place.
CREATE TABLE IF NOT EXISTS protected_places (
    -- The place's root, absolute: /Users/ana/Documents, /Volumes/Backup.
    root TEXT PRIMARY KEY,
    -- allowed or refused
    state TEXT NOT NULL,
    checked INTEGER NOT NULL,
    -- The agent refused there most recently; '' once a check found it so.
    agent TEXT NOT NULL DEFAULT ''
);
-- Each refusal raised in Needs you, for History: waiting while ended is
-- NULL.
CREATE TABLE IF NOT EXISTS protected_refusals (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    root TEXT NOT NULL,
    agent TEXT NOT NULL DEFAULT '',
    -- The item's words when it was raised.
    title TEXT NOT NULL,
    text TEXT NOT NULL,
    raised INTEGER NOT NULL,
    ended INTEGER,
    -- dismissed (the person's OK) or allowed (a check found it let in).
    outcome TEXT NOT NULL DEFAULT ''
);
CREATE INDEX IF NOT EXISTS protected_refusals_by_end ON protected_refusals (ended);
";

/// The system's protected places: macOS's own, or a stand-in in tests.
pub trait Places: Send + Sync {
    /// The protected place `path` is in, as its root; None outside them.
    fn root(&self, path: &Path) -> Option<PathBuf>;
    /// Reads the place now: Some(true) when macOS lets Claktory in,
    /// Some(false) when it refuses (EPERM), None when it can't tell (a
    /// volume that isn't there, say).
    fn read(&self, root: &Path) -> Option<bool>;
    /// Every protected place on this computer, whatever uses it
    /// (Workspace.protected_roots); none unless it says.
    fn all(&self) -> Vec<PathBuf> {
        Vec::new()
    }
}

/// The places in the home folder macOS protects, as `protected_root` names
/// them (factory-agent's `tools::protected`; a test holds the two together).
const HOME_PLACES: [&str; 4] = ["Documents", "Desktop", "Downloads", "Library/Mobile Documents"];

/// The protected places under `home` and `volumes`: the home's, then each
/// volume mounted there, by name. The entry linking the startup disk to /
/// is a link, not a volume, and is left out; nothing in a volume is read,
/// so one that doesn't answer can't hold this up.
fn all_in(home: &Path, volumes: &Path) -> Vec<PathBuf> {
    let mut all: Vec<PathBuf> = HOME_PLACES.iter().map(|place| home.join(place)).collect();
    let mut mounted: Vec<PathBuf> = std::fs::read_dir(volumes)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
        .map(|entry| volumes.join(entry.file_name()))
        .collect();
    mounted.sort();
    all.extend(mounted);
    all
}

/// macOS's: none anywhere else.
struct System;

impl Places for System {
    fn root(&self, path: &Path) -> Option<PathBuf> {
        factory_agent::tools::protected::protected_root(path)
    }

    fn read(&self, root: &Path) -> Option<bool> {
        match std::fs::read_dir(root) {
            Ok(_) => Some(true),
            // EPERM: macOS's no; EACCES is the file's own permissions.
            Err(error) if error.raw_os_error() == Some(1) => Some(false),
            Err(_) => None,
        }
    }

    fn all(&self) -> Vec<PathBuf> {
        let home = std::env::var_os("HOME").map(PathBuf::from);
        match home {
            Some(home) if cfg!(target_os = "macos") => all_in(&home, Path::new("/Volumes")),
            _ => Vec::new(),
        }
    }
}

/// The places' system, on the Factory.
pub(crate) struct Protected {
    places: Mutex<Arc<dyn Places>>,
}

impl Default for Protected {
    fn default() -> Protected {
        Protected { places: Mutex::new(Arc::new(System)) }
    }
}

impl Protected {
    fn places(&self) -> Arc<dyn Places> {
        self.places.lock().unwrap_or_else(std::sync::PoisonError::into_inner).clone()
    }

    /// The protected place `path` is in, as its root.
    pub(crate) fn root(&self, path: &Path) -> Option<PathBuf> {
        self.places().root(path)
    }

    pub(crate) fn set(&self, places: Arc<dyn Places>) {
        *self.places.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = places;
    }
}

/// What a place was found.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum State {
    Allowed,
    Refused,
}

impl State {
    fn word(self) -> &'static str {
        match self {
            State::Allowed => "allowed",
            State::Refused => "refused",
        }
    }
}

/// A place as kept.
struct Kept {
    state: State,
    checked: i64,
    agent: String,
}

fn kept(db: &Connection) -> rusqlite::Result<BTreeMap<String, Kept>> {
    let mut statement = db.prepare("SELECT root, state, checked, agent FROM protected_places")?;
    let rows = statement.query_map([], |row| {
        let state = if row.get::<_, String>(1)? == "refused" { State::Refused } else { State::Allowed };
        Ok((row.get::<_, String>(0)?, Kept { state, checked: row.get(2)?, agent: row.get(3)? }))
    })?;
    rows.collect()
}

/// The places whose refusal waits in Needs you.
fn waiting(db: &Connection) -> rusqlite::Result<Vec<String>> {
    let mut statement = db.prepare("SELECT DISTINCT root FROM protected_refusals WHERE ended IS NULL")?;
    let rows = statement.query_map([], |row| row.get(0))?;
    rows.collect()
}

/// An agent's tool says macOS refused it `claimed` (its details'
/// `protected_refused`): believed only when it's a protected place's root.
pub(crate) fn refused_by_tool(factory: &Factory, agent: &str, claimed: &str) {
    let root = PathBuf::from(claimed);
    if factory.protected.places().root(&root).as_deref() != Some(root.as_path()) {
        tracing::warn!(agent, "a tool said macOS refused a place that isn't a protected one; not believed");
        return;
    }
    if found(factory, &root, State::Refused, agent) {
        factory.broadcast_workspace(&factory.lock());
    }
}

/// Reads each of `roots` (protected places' roots only; others are skipped)
/// now, off the runtime: macOS may ask the person first, and the read waits
/// for their answer.
pub(crate) async fn check(factory: &Factory, roots: Vec<PathBuf>) {
    let places = factory.protected.places();
    let roots: Vec<PathBuf> = roots.into_iter().filter(|root| places.root(root).as_deref() == Some(root.as_path())).collect();
    if roots.is_empty() {
        return;
    }
    let read = tokio::task::spawn_blocking(move || roots.into_iter().map(|root| (places.read(&root), root)).collect::<Vec<_>>()).await;
    let Ok(read) = read else { return };
    let mut changed = false;
    for (state, root) in read {
        let state = match state {
            Some(true) => State::Allowed,
            Some(false) => State::Refused,
            None => continue,
        };
        changed |= found(factory, &root, state, "");
    }
    if changed {
        factory.broadcast_workspace(&factory.lock());
    }
}

/// A project's folder at `path` was added or pointed elsewhere: reads its
/// protected place, if it's in one, so macOS asks now.
pub(crate) async fn touch(factory: &Factory, path: &Path) {
    if let Some(root) = factory.protected.root(path) {
        check(factory, vec![root]).await;
    }
}

/// The checks the app asked for (CheckProtectedPlaces): roots as the
/// Workspace lists them.
pub(crate) async fn checks_asked(factory: &Factory, roots: &[String]) {
    check(factory, roots.iter().map(PathBuf::from).collect()).await;
}

/// The person's OK on a refusal's Needs you item: it leaves Needs you for
/// History, and the place stays refused. One with nothing waiting is fine.
pub(crate) fn dismiss(factory: &Factory, root: &str) -> Result<(), RequestError> {
    let ended = factory.store.db().execute(
        "UPDATE protected_refusals SET ended = ?2, outcome = 'dismissed' WHERE root = ?1 AND ended IS NULL",
        params![root, now_ms()],
    );
    if ended? > 0 {
        factory.broadcast_workspace(&factory.lock());
    }
    Ok(())
}

/// Keeps what `root` was found: a refusal where it wasn't refused raises
/// its item; one found let in ends a waiting item. Whether anything the
/// Workspace shows changed.
fn found(factory: &Factory, root: &Path, state: State, agent: &str) -> bool {
    let text_root = root.display().to_string();
    let words = (state == State::Refused).then(|| {
        let inner = factory.lock();
        words(&inner.config, &factory.protected.places(), root, agent)
    });
    let now = now_ms();
    let mut db = factory.store.db();
    let kept = (|| -> rusqlite::Result<bool> {
        let transaction = db.transaction()?;
        let before: Option<String> =
            transaction.query_row("SELECT state FROM protected_places WHERE root = ?1", [&text_root], |row| row.get(0)).optional()?;
        transaction.execute(
            "INSERT INTO protected_places (root, state, checked, agent) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT (root) DO UPDATE SET state = ?2, checked = ?3,
                 agent = CASE WHEN ?2 = 'refused' AND ?4 != '' THEN ?4 WHEN ?2 = 'allowed' THEN '' ELSE agent END",
            params![text_root, state.word(), now, agent],
        )?;
        let refused_before = before.as_deref() == Some("refused");
        match (state, words) {
            (State::Refused, Some((title, text))) if !refused_before => {
                transaction.execute(
                    "INSERT INTO protected_refusals (root, agent, title, text, raised) VALUES (?1, ?2, ?3, ?4, ?5)",
                    params![text_root, agent, title, text, now],
                )?;
            }
            (State::Allowed, _) if refused_before => {
                transaction.execute(
                    "UPDATE protected_refusals SET ended = ?2, outcome = 'allowed' WHERE root = ?1 AND ended IS NULL",
                    params![text_root, now],
                )?;
            }
            _ => {}
        }
        transaction.commit()?;
        Ok(before.as_deref() != Some(state.word()))
    })();
    match kept {
        Ok(changed) => changed || agent_changed(state, agent),
        Err(error) => {
            tracing::error!(%error, "couldn't keep what a protected place was found");
            false
        }
    }
}

/// A refusal of an agent where one was already refused still names it.
fn agent_changed(state: State, agent: &str) -> bool {
    state == State::Refused && !agent.is_empty()
}

/// The projects and folders working in `root`, in the sidebar's order:
/// a project by its own folder, a folder by its name.
fn used_by(config: &Config, places: &Arc<dyn Places>, root: &Path) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    for project in &config.projects {
        let mut add = |name: &str| {
            if !names.iter().any(|known| known == name) {
                names.push(name.to_owned());
            }
        };
        if places.root(&project.path).as_deref() == Some(root) {
            add(&project.name);
        }
        for folder in &project.folders {
            if places.root(&folder.path).as_deref() == Some(root) {
                add(&folder.name);
            }
        }
    }
    names
}

/// The Needs you item's words: "Your agents can't work in Documents" /
/// "macOS hasn't let Claktory Agents into Documents, so commands in
/// nixos-site fail. Turn it on for Claktory Agents in System Settings."
/// (Trish 50063: a volume's switch in Files and Folders is "Removable
/// Volumes" or "Network Volumes", so the last sentence names no place.)
fn words(config: &Config, places: &Arc<dyn Places>, root: &Path, agent: &str) -> (String, String) {
    let name = factory_agent::tools::protected::place_name(root);
    let used = used_by(config, places, root);
    let agent = config.agent(agent).map(|agent| agent.name.clone());
    let why = match (used.is_empty(), agent) {
        (false, _) => format!("commands in {} fail", factory_agent::text::list_and(&used)),
        (true, Some(agent)) => format!("{agent}'s commands there fail"),
        (true, None) => "agents' commands there fail".to_owned(),
    };
    (
        format!("Your agents can't work in {name}"),
        format!("macOS hasn't let Claktory Agents into {name}, so {why}. Turn it on for Claktory Agents in System Settings."),
    )
}

/// Workspace.protected_places: the places projects and folders work in, in
/// the sidebar's order, then others an agent was refused in.
pub(crate) fn view(factory: &Factory, config: &Config) -> Vec<v1::ProtectedPlace> {
    let places = factory.protected.places();
    let mut roots: Vec<PathBuf> = Vec::new();
    for project in &config.projects {
        for path in std::iter::once(&project.path).chain(project.folders.iter().map(|folder| &folder.path)) {
            if let Some(root) = places.root(path)
                && !roots.contains(&root)
            {
                roots.push(root);
            }
        }
    }
    let (kept, waiting) = {
        let db = factory.store.db();
        (kept(&db).unwrap_or_default(), waiting(&db).unwrap_or_default())
    };
    for (root, place) in &kept {
        let root = PathBuf::from(root);
        if (place.state == State::Refused || waiting.iter().any(|waits| Path::new(waits) == root)) && !roots.contains(&root) {
            roots.push(root);
        }
    }
    roots
        .into_iter()
        .map(|root| {
            let text = root.display().to_string();
            let place = kept.get(&text);
            let state = match place.map(|place| place.state) {
                None => v1::ProtectedPlaceState::Unspecified,
                Some(State::Allowed) => v1::ProtectedPlaceState::Allowed,
                Some(State::Refused) => v1::ProtectedPlaceState::Refused,
            };
            let agent = place.map(|place| place.agent.clone()).unwrap_or_default();
            let (title, text_words) =
                if state == v1::ProtectedPlaceState::Refused { words(config, &places, &root, &agent) } else { Default::default() };
            v1::ProtectedPlace {
                name: factory_agent::tools::protected::place_name(&root),
                state: state.into(),
                checked_ms: place.map_or(0, |place| place.checked),
                used_by: used_by(config, &places, &root),
                needs_you: waiting.contains(&text),
                title,
                text: text_words,
                agent_id: agent,
                root: text,
            }
        })
        .collect()
}

/// Workspace.protected_roots: every protected place on this computer, for
/// the app to tell before anything is saved whether a path typed is in one,
/// and name it (Illuso 49904).
pub(crate) fn roots(factory: &Factory) -> Vec<v1::ProtectedRoot> {
    let places = factory.protected.places();
    places
        .all()
        .into_iter()
        .map(|root| v1::ProtectedRoot { name: factory_agent::tools::protected::place_name(&root), root: root.display().to_string() })
        .collect()
}

/// A refusal raised in Needs you that ended, for History.
pub(crate) struct Ended {
    pub id: i64,
    pub agent: String,
    pub title: String,
    pub text: String,
    pub raised: i64,
    pub ended: i64,
    /// The person's OK; else a check found it let in.
    pub dismissed: bool,
}

/// The refusals that ended in [since, before), newest first, at most
/// `limit`.
pub(crate) fn between(db: &Connection, since: i64, before: i64, limit: i64) -> rusqlite::Result<Vec<Ended>> {
    let mut statement = db.prepare(
        "SELECT id, agent, title, text, raised, ended, outcome FROM protected_refusals
         WHERE ended >= ?1 AND ended < ?2 ORDER BY ended DESC, id DESC LIMIT ?3",
    )?;
    let rows = statement.query_map(params![since, before, limit], |row| {
        Ok(Ended {
            id: row.get(0)?,
            agent: row.get(1)?,
            title: row.get(2)?,
            text: row.get(3)?,
            raised: row.get(4)?,
            ended: row.get(5)?,
            dismissed: row.get::<_, String>(6)? == "dismissed",
        })
    })?;
    rows.collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every place listed is one the tools' rule calls protected, as its
    /// own root, so the app's match agrees with factoryd's.
    #[test]
    fn every_protected_place_is_listed_as_the_tools_name_it() {
        let dir = std::env::temp_dir().join(format!("factoryd-protected-{}", std::process::id()));
        let volumes = dir.join("Volumes");
        std::fs::create_dir_all(volumes.join("Backup")).unwrap();
        std::fs::create_dir_all(volumes.join("Photos")).unwrap();
        std::os::unix::fs::symlink("/", volumes.join("Macintosh HD")).unwrap();
        std::fs::write(volumes.join(".DS_Store"), b"").unwrap();
        let home = Path::new("/Users/ana");
        let all = all_in(home, &volumes);
        let names: Vec<String> = all.iter().map(|root| factory_agent::tools::protected::place_name(root)).collect();
        assert_eq!(names, ["Documents", "Desktop", "Downloads", "iCloud Drive", "Backup", "Photos"], "no startup disk link, no file");
        for root in &all[..4] {
            assert_eq!(factory_agent::tools::protected::protected_root_in(home, &root.join("site")).as_deref(), Some(root.as_path()));
        }
        let real = Path::new("/Volumes/Backup");
        assert_eq!(factory_agent::tools::protected::protected_root_in(home, &real.join("x")).as_deref(), Some(real));
        assert_eq!(all_in(home, &dir.join("none")).len(), 4, "no volumes folder");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
