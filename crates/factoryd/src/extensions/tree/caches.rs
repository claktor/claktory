//! Build caches stay bounded on their own. Builds fill disks: a Rust target/
//! grows by gigabytes a day in each checkout an agent builds in. So Factory
//! looks at every agent's checkout, and every project folder, every few
//! minutes and finds the build caches in them. While nothing works there,
//! it first deletes what Rust builds leave behind and never use again:
//! object files (`*.rcgu.o`) in `target/**/deps` older than a few hours. On
//! macOS, rustc 1.88 and later leaves a new set at every rebuild with debug
//! info and never deletes the old (rust-lang/rust#161824), about a GB per
//! test run of a large crate; cargo still finds everything fresh without
//! them. Then, when the place's caches together still take more than the
//! budget (`resources.build_cache_gb`, 25 GB unless changed), it clears
//! them until it's under. First it trims Rust's targets, largest first:
//! their incremental state (`<profile>/incremental`, most of a target) and
//! test binaries a newer build of the same test replaced. The next build
//! takes longer than a warm one but far less than a cold one (#454). Only
//! when that isn't enough does it clear whole caches, largest first. Each
//! is moved aside at once and deleted in the background. The agents that
//! work there hear of a cache cleared: one working now after its current
//! step, an idle one at its next run (the stale objects and a trim go
//! unannounced: nothing starts cold).
//!
//! Only folders known to be build caches are ever touched: a `target/` next
//! to a Cargo.toml, a Flutter `build/` or `.dart_tool/` next to a
//! pubspec.yaml, `node_modules/` next to a package.json and the like, or a
//! folder tagged as a cache (CACHEDIR.TAG). Never one git tracks a file in,
//! and never anything else git ignores (.env files, local data).
//!
//! A trim, and the stale objects, need only the target to be quiet: no Rust
//! build holding its lock, and nothing written in it for `TRIM_QUIET`
//! (two minutes), whatever its agents are doing, background tasks and
//! watches included. Busy agents nearly always have one running, and their
//! caches grow most (Polpo's disk.md, item 1); nothing running uses what a
//! trim takes (incremental state, test binaries a newer build replaced, and
//! on macOS a running executable keeps working once unlinked). A whole
//! cache is cleared only while nothing works there: no agent of the folder
//! running, nor with a background task or watch running, nothing written
//! in it for a few minutes, and no Rust build holding its lock, since its
//! next build is cold. The last checks and the clearing itself happen under
//! Factory's lock, which a tool call's start takes before the tool runs, so
//! none starts in between.
//!
//! The dev Claktory (`~/.factory-dev`) looks only inside its own data
//! folder: its agents' checkouts and its projects there. A project folder
//! elsewhere, such as the repository it's built from, is usually the real
//! Claktory's too, whose agents it can't see working there.

use std::collections::{HashMap, HashSet};
use std::io::Read as _;
#[cfg(unix)]
use std::os::unix::fs::MetadataExt;
#[cfg(windows)]
use crate::windows::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crate::factory::{Factory, Inner, Queued, host_event};
use crate::transcript::now_ms;

/// Cache folders by name, each with the files beside it that make it one.
const KNOWN: &[(&str, &[&str])] = &[
    ("target", &["Cargo.toml"]),
    ("build", &["pubspec.yaml", "build.gradle", "build.gradle.kts"]),
    (".dart_tool", &["pubspec.yaml"]),
    ("node_modules", &["package.json"]),
    (".next", &["package.json"]),
    (".turbo", &["package.json"]),
    (".parcel-cache", &["package.json"]),
    (".build", &["Package.swift"]),
    (".gradle", &["build.gradle", "build.gradle.kts", "settings.gradle", "settings.gradle.kts"]),
];

/// What a CACHEDIR.TAG starts with (https://bford.info/cachedir/).
const CACHEDIR_SIGNATURE: &[u8] = b"Signature: 8a477f597d28d172789f06886806bc55";

/// How deep below a checkout or project folder caches are looked for:
/// enough for app/rust/target in a repository.
const DEPTH: usize = 4;

/// A cache written this recently is in use.
const RECENT: Duration = Duration::from_secs(5 * 60);

/// A Rust target written this recently isn't trimmed: a build may be
/// between two of its steps.
const TRIM_QUIET: Duration = Duration::from_secs(2 * 60);

/// Rust's object files older than this are left over from earlier builds.
const STALE_OBJECTS: Duration = Duration::from_secs(3 * 3600);

/// A test binary a newer build of the same test replaced is no longer run
/// once it's this old: a busy checkout makes gigabytes of them within hours.
const SUPERSEDED: Duration = Duration::from_secs(15 * 60);

/// How long one sweep may walk.
const SWEEP_BUDGET: Duration = Duration::from_secs(300);

/// What a cleared cache's folder is renamed to while it's deleted, when it
/// can't go to `<home>/trash` (another volume).
const TRASH_PREFIX: &str = ".factory-trash-";

/// A build cache as last measured.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Cache {
    pub path: PathBuf,
    /// What it takes, in bytes; at least this when `partial`.
    pub bytes: u64,
    pub partial: bool,
    /// When anything in it was last written, in seconds since 1970.
    newest: i64,
}

/// A checkout or project folder, and the caches found in it.
#[derive(Clone, Debug)]
pub(crate) struct Place {
    pub path: PathBuf,
    /// The agent's whose checkout it is; None for a project's folder.
    pub agent: Option<String>,
    pub caches: Vec<Cache>,
}

pub(crate) struct State {
    every_ms: AtomicU64,
    /// The last sweep's places.
    found: Mutex<Vec<Place>>,
    /// What each agent hears at its next run.
    notes: Mutex<HashMap<String, Vec<String>>>,
    /// Wakes the sweep when its interval changes.
    wake: std::sync::Arc<tokio::sync::Notify>,
}

impl Default for State {
    fn default() -> State {
        State { every_ms: AtomicU64::new(10 * 60_000), found: Mutex::default(), notes: Mutex::default(), wake: std::sync::Arc::default() }
    }
}

impl State {
    fn every(&self) -> Duration {
        Duration::from_millis(self.every_ms.load(Ordering::Relaxed))
    }

    /// The last sweep's places.
    pub(crate) fn found(&self) -> Vec<Place> {
        self.found.lock().unwrap_or_else(std::sync::PoisonError::into_inner).clone()
    }

    fn note(&self, agent: &str, text: String) {
        self.notes.lock().unwrap_or_else(std::sync::PoisonError::into_inner).entry(agent.to_owned()).or_default().push(text);
    }
}

impl Factory {
    /// How often build caches are checked (10 minutes); tests make it shorter.
    pub fn set_build_cache_every(&self, every: Duration) {
        self.build_caches.every_ms.store(u64::try_from(every.as_millis()).unwrap_or(u64::MAX), Ordering::Relaxed);
        self.build_caches.wake.notify_one();
    }
}

/// Whether `dir` is a build cache: a known name beside its marker, or tagged.
pub(crate) fn is_cache(dir: &Path) -> bool {
    let Ok(metadata) = std::fs::symlink_metadata(dir) else { return false };
    if !metadata.is_dir() {
        return false;
    }
    let name = dir.file_name().and_then(|name| name.to_str()).unwrap_or_default();
    let parent = dir.parent().unwrap_or(Path::new("/"));
    let known = KNOWN.iter().any(|(known, markers)| *known == name && markers.iter().any(|marker| parent.join(marker).is_file()));
    known || tagged(dir)
}

fn tagged(dir: &Path) -> bool {
    let mut start = [0; CACHEDIR_SIGNATURE.len()];
    std::fs::File::open(dir.join("CACHEDIR.TAG")).and_then(|mut file| file.read_exact(&mut start)).is_ok() && start == CACHEDIR_SIGNATURE
}

/// Whether git tracks a file in `dir`; false outside a repository.
fn tracked(dir: &Path) -> bool {
    let Some(parent) = dir.parent() else { return false };
    let Some(name) = dir.file_name() else { return false };
    let output = crate::trees::git_command(parent).args(["ls-files", "-z", "--"]).arg(name).output();
    // Not a repository (or no git): nothing is tracked.
    output.is_ok_and(|output| output.status.success() && !output.stdout.is_empty())
}

/// The build caches below `root`, and leftovers of clearing to delete.
fn find(root: &Path) -> (Vec<PathBuf>, Vec<PathBuf>) {
    let (mut caches, mut leftovers) = (Vec::new(), Vec::new());
    let mut stack = vec![(root.to_path_buf(), 0)];
    while let Some((dir, depth)) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else { continue };
        let mut entries: Vec<_> = entries.filter_map(Result::ok).collect();
        entries.sort_by_key(std::fs::DirEntry::file_name);
        for entry in entries {
            let Ok(kind) = entry.file_type() else { continue };
            if !kind.is_dir() {
                continue;
            }
            let path = entry.path();
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if name.starts_with(TRASH_PREFIX) {
                leftovers.push(path);
            } else if name == ".git" {
            } else if is_cache(&path) {
                if !tracked(&path) {
                    caches.push(path);
                }
            } else if depth + 1 < DEPTH {
                stack.push((path, depth + 1));
            }
        }
    }
    caches.sort();
    (caches, leftovers)
}

/// What `dir` takes on disk, when it was last written, and whether the walk
/// stopped at `deadline`. Symlinks aren't followed.
fn measure(dir: &Path, deadline: Instant) -> Cache {
    let (mut bytes, mut newest, mut partial) = (0, 0, false);
    let mut stack = vec![dir.to_path_buf()];
    while let Some(at) = stack.pop() {
        if Instant::now() > deadline {
            partial = true;
            break;
        }
        let Ok(metadata) = std::fs::symlink_metadata(&at) else { continue };
        bytes += metadata.blocks() * 512;
        newest = newest.max(metadata.mtime());
        if metadata.is_dir()
            && let Ok(entries) = std::fs::read_dir(&at)
        {
            stack.extend(entries.filter_map(Result::ok).map(|entry| entry.path()));
        }
    }
    Cache { path: dir.to_path_buf(), bytes, partial, newest }
}

/// Whether something is building in the cache now: written within
/// `recent`, or a Rust build holds its lock.
fn in_use(cache: &Cache, recent: Duration) -> bool {
    let now = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |elapsed| elapsed.as_secs() as i64);
    if now - cache.newest < recent.as_secs() as i64 {
        return true;
    }
    cargo_locked(&cache.path)
}

/// A Rust build holds a lock file in the target folder (`debug/.cargo-lock`,
/// `<triple>/release/.cargo-lock`…) while it runs.
fn cargo_locked(dir: &Path) -> bool {
    let mut stack = vec![(dir.to_path_buf(), 0)];
    while let Some((at, depth)) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&at) else { continue };
        for entry in entries.filter_map(Result::ok) {
            let path = entry.path();
            if entry.file_name().to_string_lossy().ends_with(".cargo-lock")
                && let Ok(file) = std::fs::File::open(&path)
                && matches!(file.try_lock(), Err(std::fs::TryLockError::WouldBlock))
            {
                return true;
            }
            if depth < 2 && entry.file_type().is_ok_and(|kind| kind.is_dir()) {
                stack.push((path, depth + 1));
            }
        }
    }
    false
}

/// The folders of a target that may be a profile's (`debug`, `release`,
/// `<triple>/debug`…): its folders, and theirs.
fn profiles(target: &Path) -> Vec<PathBuf> {
    let subdirs = |dir: &Path| -> Vec<PathBuf> {
        std::fs::read_dir(dir)
            .map(|entries| {
                entries
                    .filter_map(Result::ok)
                    .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
                    .map(|entry| entry.path())
                    .collect()
            })
            .unwrap_or_default()
    };
    let mut profiles = Vec::new();
    for dir in subdirs(target) {
        profiles.extend(subdirs(&dir));
        profiles.push(dir);
    }
    profiles.sort();
    profiles
}

/// Deletes the object files Rust builds leave in a target folder's `deps`
/// (`<profile>/deps`, `<triple>/<profile>/deps`) that are older than
/// `STALE_OBJECTS`; returns how many and their bytes.
fn sweep_objects(target: &Path) -> (usize, u64) {
    let Some(stale) = SystemTime::now().checked_sub(STALE_OBJECTS) else { return (0, 0) };
    let (mut count, mut bytes) = (0, 0);
    for dir in profiles(target).into_iter().map(|profile| profile.join("deps")) {
        let Ok(entries) = std::fs::read_dir(&dir) else { continue };
        for entry in entries.filter_map(Result::ok) {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if !(name.ends_with(".rcgu.o") || name.ends_with(".rcgu.dwo")) {
                continue;
            }
            let Ok(metadata) = entry.metadata() else { continue };
            if metadata.is_file()
                && metadata.modified().is_ok_and(|modified| modified < stale)
                && std::fs::remove_file(entry.path()).is_ok()
            {
                count += 1;
                bytes += metadata.blocks() * 512;
            }
        }
    }
    (count, bytes)
}

/// Whether a file in `deps` is an executable. On Windows those end in
/// `.exe`, which the names looked at here never do.
#[cfg(unix)]
fn executable(metadata: &std::fs::Metadata) -> bool {
    metadata.mode() & 0o111 != 0
}

#[cfg(windows)]
fn executable(_metadata: &std::fs::Metadata) -> bool {
    false
}

/// Whether `dir` is a Rust build's target folder.
fn rust_target(dir: &Path) -> bool {
    dir.join(".rustc_info.json").is_file() || profiles(dir).iter().any(|profile| profile.join(".fingerprint").is_dir())
}

/// What a trim of a Rust target takes away, with what each takes: its
/// profiles' incremental state, and the executables in their `deps` (test
/// binaries, mostly) that a newer build of the same name replaced and that
/// are older than `SUPERSEDED`. The newest of each name stays, so the
/// tests built last run without linking again. None of it makes the next
/// build a cold one.
fn trimmable(target: &Path, deadline: Instant) -> Vec<(PathBuf, u64)> {
    let Some(stale) = SystemTime::now().checked_sub(SUPERSEDED) else { return Vec::new() };
    let mut found = Vec::new();
    for profile in profiles(target) {
        let incremental = profile.join("incremental");
        if std::fs::symlink_metadata(&incremental).is_ok_and(|metadata| metadata.is_dir()) {
            let size = measure(&incremental, deadline).bytes;
            found.push((incremental, size));
        }
        let Ok(entries) = std::fs::read_dir(profile.join("deps")) else { continue };
        // By name (`all` of `all-0123456789abcdef`): the executables, newest first.
        let mut builds: HashMap<String, Vec<(SystemTime, PathBuf, u64)>> = HashMap::new();
        for entry in entries.filter_map(Result::ok) {
            let file = entry.file_name();
            let file = file.to_string_lossy();
            let Some((name, hash)) = file.rsplit_once('-') else { continue };
            if file.contains('.') || hash.len() != 16 || !hash.chars().all(|c| c.is_ascii_hexdigit()) {
                continue;
            }
            let Ok(metadata) = entry.metadata() else { continue };
            if !metadata.is_file() || !executable(&metadata) {
                continue;
            }
            let Ok(modified) = metadata.modified() else { continue };
            builds.entry(name.to_owned()).or_default().push((modified, entry.path(), metadata.blocks() * 512));
        }
        for mut versions in builds.into_values() {
            versions.sort_by_key(|version| std::cmp::Reverse(version.0));
            found.extend(versions.into_iter().skip(1).filter(|(modified, ..)| *modified < stale).map(|(_, path, bytes)| (path, bytes)));
        }
    }
    found
}

/// Opens a folder to set its time: Windows wants write-attributes access.
fn date_folder(folder: &Path) -> std::io::Result<std::fs::File> {
    #[cfg(unix)]
    return std::fs::File::open(folder);
    #[cfg(windows)]
    return crate::windows::fs::open_to_date(folder);
}

/// Takes away what `trimmable` found in `target`: a folder is moved aside
/// and deleted in the background, a file deleted. Returns the bytes freed.
fn trim(factory: &Factory, target: &Path, found: &[(PathBuf, u64)]) -> u64 {
    let mut freed = 0;
    for (path, bytes) in found {
        // Its folder keeps its time: what a trim takes isn't a build
        // writing there (`in_use`, `written_lately`).
        let parent = path.parent().and_then(|parent| Some((parent, std::fs::metadata(parent).ok()?.modified().ok()?)));
        let gone = if path.is_dir() { move_aside(factory, path, target) } else { std::fs::remove_file(path) };
        if let Some((parent, modified)) = parent
            && let Err(error) = date_folder(parent).and_then(|folder| folder.set_modified(modified))
        {
            tracing::warn!(path = %parent.display(), %error, "could not keep a trimmed folder's time");
        }
        match gone {
            Ok(()) => freed += bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => tracing::warn!(path = %path.display(), %error, "could not trim a build cache"),
        }
    }
    freed
}

fn gb(bytes: u64) -> String {
    #[allow(clippy::cast_precision_loss)] // shown to one decimal
    let gb = bytes as f64 / 1e9;
    if gb >= 10.0 { format!("{gb:.0} GB") } else { format!("{gb:.1} GB") }
}

/// Whether this is the dev Claktory, which keeps to its own data folder.
fn dev(factory: &Factory) -> bool {
    factory.home.root().file_name().is_some_and(|name| name == DEV_HOME)
}

/// The dev Claktory's data folder's name (`home::instance`).
const DEV_HOME: &str = ".factory-dev";

/// The places to look in: agents' checkouts, then projects' folders, with
/// whose they are. A project folder within another, or holding the
/// checkouts, isn't looked in twice; the dev Claktory leaves those outside
/// its data folder.
fn places(factory: &Factory) -> Vec<Place> {
    let outside = |path: &Path| dev(factory) && !path.starts_with(factory.home.root());
    let inner = factory.lock();
    let trees = crate::trees::trees_dir(&factory.home);
    let mut places = Vec::new();
    for agent in &inner.config.agents {
        let Ok(entries) = std::fs::read_dir(trees.join(&agent.id)) else { continue };
        let mut checkouts: Vec<PathBuf> =
            entries.filter_map(Result::ok).map(|entry| entry.path()).filter(|path| path.join(".git").is_file()).collect();
        checkouts.sort();
        for path in checkouts {
            places.push(Place { path, agent: Some(agent.id.clone()), caches: Vec::new() });
        }
    }
    let mut folders: Vec<PathBuf> = Vec::new();
    for project in &inner.config.projects {
        folders.extend(std::iter::once(&project.path).chain(project.folders.iter().map(|folder| &folder.path)).cloned());
    }
    for path in &folders {
        let within = folders.iter().any(|other| other != path && path.starts_with(other));
        if within || outside(path) || trees.starts_with(path) || places.iter().any(|place: &Place| place.path == *path) {
            continue;
        }
        places.push(Place { path: path.clone(), agent: None, caches: Vec::new() });
    }
    places
}

/// The agents that work in `place`: a checkout's agent, or those whose tools
/// start in a project folder (not in a checkout of their own).
fn workers(factory: &Factory, place: &Place) -> Vec<String> {
    if let Some(agent) = &place.agent {
        return vec![agent.clone()];
    }
    let inner = factory.lock();
    let config = &inner.config;
    config
        .agents
        .iter()
        .filter(|agent| crate::runner::places(&factory.home, config, agent, &[]).folder.starts_with(&place.path))
        .map(|agent| agent.id.clone())
        .collect()
}

/// Agents working now: running, or with a background task or a watch running.
fn working(factory: &Factory) -> HashSet<String> {
    let mut busy = occupied(factory);
    let inner = factory.lock();
    busy.extend(inner.agents.iter().filter(|(_, runtime)| runtime.running).map(|(id, _)| id.clone()));
    busy
}

/// Whether `agent` is running a tool call now (not between its steps).
fn mid_tool(inner: &Inner, agent: &str) -> bool {
    inner.agents.get(agent).is_some_and(|runtime| runtime.running && runtime.doing.is_some())
}

/// Agents with a background task or a watch running, whether or not they
/// run themselves.
fn occupied(factory: &Factory) -> HashSet<String> {
    let mut busy: HashSet<String> = HashSet::new();
    match factory.store.running_tasks() {
        Ok(tasks) => busy.extend(tasks.into_iter().map(|(agent, _)| agent)),
        // Unknown: everyone counts as working.
        Err(error) => {
            tracing::error!(%error, "could not read running tasks; no build cache is cleared");
            busy.extend(factory.lock().config.agents.iter().map(|agent| agent.id.clone()));
        }
    }
    let active = crate::extensions::monitors::Query { active: true, ..Default::default() };
    if let Ok(monitors) = crate::extensions::monitors::query(&factory.store, &active) {
        busy.extend(monitors.into_iter().filter(|monitor| monitor.cursor.pid.is_some()).map(|monitor| monitor.agent));
    }
    busy
}

/// Whether a build wrote in `dir` within `recent`, by its folders' times
/// down to `target/debug/deps` (a file made or removed in a folder changes
/// its time): quick enough to ask under Factory's lock, just before
/// clearing.
fn written_lately(dir: &Path, recent: Duration) -> bool {
    let Some(lately) = SystemTime::now().checked_sub(recent) else { return true };
    let mut stack = vec![(dir.to_path_buf(), 0)];
    while let Some((at, depth)) = stack.pop() {
        let Ok(metadata) = std::fs::symlink_metadata(&at) else { continue };
        if metadata.modified().is_ok_and(|modified| modified > lately) {
            return true;
        }
        if depth < 2
            && let Ok(entries) = std::fs::read_dir(&at)
        {
            stack.extend(
                entries
                    .filter_map(Result::ok)
                    .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
                    .map(|entry| (entry.path(), depth + 1)),
            );
        }
    }
    false
}

/// Moves the cache aside at once and deletes it in the background: into
/// `<home>/trash`, or beside it when that's on another volume.
fn clear(factory: &Factory, cache: &Path) -> std::io::Result<()> {
    move_aside(factory, cache, cache)
}

/// Moves `path` aside at once and deletes it in the background: into
/// `<home>/trash`, or beside `cache` (where a sweep finds what a restart
/// left) when that's on another volume.
fn move_aside(factory: &Factory, path: &Path, cache: &Path) -> std::io::Result<()> {
    static MOVED: AtomicU64 = AtomicU64::new(0);
    let trash = factory.home.root().join("trash");
    std::fs::create_dir_all(&trash)?;
    let name = path.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
    let stamp = format!("{}-{}-{name}", now_ms(), MOVED.fetch_add(1, Ordering::Relaxed));
    #[cfg(unix)]
    let same_volume = std::fs::metadata(&trash).ok().zip(std::fs::metadata(path).ok()).is_some_and(|(a, b)| a.dev() == b.dev());
    #[cfg(windows)]
    let same_volume = crate::windows::fs::identity(&trash).ok().zip(crate::windows::fs::identity(path).ok()).is_some_and(|(a, b)| a.volume == b.volume);
    let aside = if same_volume { trash.join(stamp) } else { cache.with_file_name(format!("{TRASH_PREFIX}{stamp}")) };
    std::fs::rename(path, &aside)?;
    delete_later(aside);
    Ok(())
}

fn delete_later(path: PathBuf) {
    std::thread::spawn(move || {
        if let Err(error) = std::fs::remove_dir_all(&path) {
            tracing::warn!(path = %path.display(), %error, "could not delete a cleared build cache");
        }
    });
}

/// What's left in `<home>/trash` from before a restart goes.
fn empty_trash(factory: &Factory) {
    let Ok(entries) = std::fs::read_dir(factory.home.root().join("trash")) else { return };
    for entry in entries.filter_map(Result::ok) {
        delete_later(entry.path());
    }
}

/// Tells the place's agents that a cache was cleared: one working now (it
/// was between steps) after its current step, an idle one at its next run.
/// `now` is Factory and its state, when the caller holds its lock.
fn tell(factory: &Factory, mut now: Option<(&Arc<Factory>, &mut Inner)>, place: &Place, workers: &[String], cache: &Cache) {
    let size = gb(cache.bytes);
    for agent in workers {
        let text = if place.agent.is_some() {
            format!("Your checkout's build cache ({size}, {}) was cleared; the next build is cold.", cache.path.display())
        } else {
            format!("The build cache {} ({size}) was cleared; the next build there is cold.", cache.path.display())
        };
        match now.as_mut() {
            Some((factory, inner)) if inner.agents.get(agent).is_some_and(|runtime| runtime.running) => {
                let note = Queued::event(text, host_event("A build cache was cleared".to_owned()));
                if let Err(error) = factory.enqueue(inner, agent, note) {
                    tracing::error!(%agent, %error, "could not tell an agent a build cache was cleared");
                }
            }
            _ => factory.build_caches.note(agent, text),
        }
    }
}

/// What the agent hears of cleared caches before its run, recorded like any
/// message; None if nothing was cleared.
pub(crate) fn take_note(factory: &Factory, agent: &str) -> Option<Queued> {
    let texts = factory.build_caches.notes.lock().unwrap_or_else(std::sync::PoisonError::into_inner).remove(agent)?;
    let summary =
        if texts.len() == 1 { "A build cache was cleared".to_owned() } else { format!("{} build caches were cleared", texts.len()) };
    let event = host_event(summary);
    let mut note = Queued::event(texts.join("\n"), event);
    match factory.store.add_message(agent, &note) {
        Ok(id) => note.id = id,
        Err(error) => tracing::error!(%agent, %error, "could not record that a build cache was cleared"),
    }
    Some(note)
}

/// Looks at every place once: measures its caches and, for a place over the
/// budget, trims its Rust targets while nothing works there (its agents may
/// be between steps), then clears whole caches while its agents are idle.
pub(crate) fn sweep(factory: &Arc<Factory>) {
    let budget_gb = factory.lock().config.resources.build_cache_gb;
    let deadline = Instant::now() + SWEEP_BUDGET;
    let mut places = places(factory);
    let mut seen = HashSet::new();
    for place in &mut places {
        let (caches, leftovers) = find(&place.path);
        for leftover in leftovers {
            delete_later(leftover);
        }
        // Stale objects first, whatever the agents do: they cost no
        // rebuild, and no build uses them, only one running now might.
        for cache in caches.iter().filter(|cache| !cargo_locked(cache)) {
            let (count, bytes) = sweep_objects(cache);
            if count > 0 {
                tracing::info!(cache = %cache.display(), count, bytes = %gb(bytes), "deleted stale build objects");
            }
        }
        place.caches = caches.into_iter().filter(|cache| seen.insert(cache.clone())).map(|cache| measure(&cache, deadline)).collect();
        place.caches.sort_by_key(|cache| std::cmp::Reverse(cache.bytes));
    }
    // 0: never cleared on its own.
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)] // a budget in bytes
    let budget = (budget_gb * 1e9) as u64;
    if budget > 0 {
        for place in &mut places {
            let mut total: u64 = place.caches.iter().map(|cache| cache.bytes).sum();
            if total <= budget {
                continue;
            }
            let workers = workers(factory, place);
            // Trims first, largest first, whatever the place's agents are
            // doing: only a quiet target, and the next build isn't a cold
            // one (Polpo's disk.md, item 1).
            for cache in &mut place.caches {
                if total <= budget {
                    break;
                }
                if in_use(cache, TRIM_QUIET) || !rust_target(&cache.path) {
                    continue;
                }
                let found = trimmable(&cache.path, deadline);
                if found.is_empty() {
                    continue;
                }
                // Asked again just before: a build may have started.
                if written_lately(&cache.path, TRIM_QUIET) || cargo_locked(&cache.path) {
                    continue;
                }
                let freed = trim(factory, &cache.path, &found);
                if freed > 0 {
                    tracing::info!(cache = %cache.path.display(), size = %gb(cache.bytes), freed = %gb(freed), budget = %gb(budget), "trimmed a build cache over budget");
                    cache.bytes = cache.bytes.saturating_sub(freed);
                    total = total.saturating_sub(freed);
                }
            }
            if total <= budget {
                place.caches.sort_by_key(|cache| std::cmp::Reverse(cache.bytes));
                continue;
            }
            // Whole caches only while nothing works there: the next build
            // is cold. Not while one of its agents runs, nor has a
            // background task or a watch running.
            let tasks = occupied(factory);
            if let Some(agent) = {
                let inner = factory.lock();
                workers
                    .iter()
                    .find(|agent| tasks.contains(*agent) || inner.agents.get(agent.as_str()).is_some_and(|runtime| runtime.running))
                    .cloned()
            } {
                tracing::info!(place = %place.path.display(), %agent, total = %gb(total), budget = %gb(budget), "build caches over budget after a trim; left while an agent works or its task or watch runs");
                place.caches.sort_by_key(|cache| std::cmp::Reverse(cache.bytes));
                continue;
            }
            let mut kept = Vec::new();
            for cache in std::mem::take(&mut place.caches) {
                if total <= budget || in_use(&cache, RECENT) {
                    kept.push(cache);
                    continue;
                }
                // The last checks, and the clearing, under the lock a tool
                // call's start waits for: none of the place's agents is in
                // one, and none starts until the cache is moved aside.
                let mut inner = factory.lock();
                if let Some(agent) = workers.iter().find(|agent| mid_tool(&inner, agent)) {
                    tracing::info!(place = %place.path.display(), %agent, total = %gb(total), "build caches over budget; left while an agent runs a tool");
                    kept.push(cache);
                    continue;
                }
                if written_lately(&cache.path, RECENT) || cargo_locked(&cache.path) {
                    kept.push(cache);
                    continue;
                }
                match clear(factory, &cache.path) {
                    Ok(()) => {
                        tracing::info!(cache = %cache.path.display(), size = %gb(cache.bytes), budget = %gb(budget), "cleared a build cache over budget");
                        total -= cache.bytes;
                        tell(factory, Some((factory, &mut inner)), place, &workers, &cache);
                    }
                    Err(error) => {
                        tracing::warn!(cache = %cache.path.display(), %error, "could not clear a build cache");
                        kept.push(cache);
                    }
                }
            }
            place.caches = kept;
        }
    }
    *factory.build_caches.found.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = places;
    crate::extensions::resources::show_caches(factory);
}

/// Clears one build cache when asked (Rivet, the Resources panel): it must
/// be a cache in an agent's checkout or a project's folder, with nothing
/// working there. Says what was cleared, or why not.
pub(crate) fn clear_on_request(factory: &Factory, path: &str) -> Result<String, String> {
    let path = Path::new(path.trim());
    if !path.is_absolute() {
        return Err(format!("\"{}\" isn't an absolute path to a build cache.", path.display()));
    }
    let Some(place) = places(factory).into_iter().find(|place| path.starts_with(&place.path) && path != place.path) else {
        if dev(factory) && !path.starts_with(factory.home.root()) {
            return Err(format!(
                "{} is outside this Claktory's data folder; the dev Claktory clears only caches inside it.",
                path.display()
            ));
        }
        return Err(format!("{} isn't in an agent's checkout or a project's folder.", path.display()));
    };
    if !is_cache(path) || tracked(path) {
        return Err(format!(
            "{} isn't a build cache Claktory knows (such as a target/ beside a Cargo.toml, or node_modules/ beside a package.json); only those are cleared.",
            path.display()
        ));
    }
    let workers = workers(factory, &place);
    let busy = working(factory);
    if let Some(agent) = workers.iter().find(|agent| busy.contains(*agent)) {
        let name = factory.agent_name(agent);
        return Err(format!("{name} is working there now; clear it once {name} is idle."));
    }
    let cache = measure(path, Instant::now() + SWEEP_BUDGET);
    if cargo_locked(path) {
        return Err(format!("A build is running in {} now; clear it once it's done.", path.display()));
    }
    clear(factory, path).map_err(|error| format!("Couldn't clear {}: {error}", path.display()))?;
    tracing::info!(cache = %path.display(), bytes = cache.bytes, "cleared a build cache on request");
    tell(factory, None, &place, &workers, &cache);
    {
        let mut found = factory.build_caches.found.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        for place in found.iter_mut() {
            place.caches.retain(|known| known.path != path);
        }
    }
    crate::extensions::resources::show_caches(factory);
    Ok(format!("Cleared {} ({}); it's deleted in the background, and the next build there is cold.", path.display(), gb(cache.bytes)))
}

/// Checks build caches shortly after Factory starts, then every 10 minutes.
pub(crate) async fn monitor(factory: Weak<Factory>) {
    if let Some(factory) = factory.upgrade() {
        empty_trash(&factory);
    }
    let mut wait = Duration::from_secs(120);
    loop {
        let Some((every, wake)) = factory.upgrade().map(|factory| (factory.build_caches.every(), factory.build_caches.wake.clone())) else {
            return;
        };
        tokio::select! {
            () = tokio::time::sleep(wait.min(every)) => {}
            () = wake.notified() => {}
        }
        wait = every;
        let Some(strong) = factory.upgrade() else { return };
        let swept = tokio::task::spawn_blocking(move || sweep(&strong)).await;
        if let Err(error) = swept {
            tracing::error!(%error, "the build cache sweep failed");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("factory-caches-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("dir");
        dir
    }

    #[test]
    fn only_known_caches_beside_their_markers_are_found() {
        let root = dir("find");
        for file in [
            "Cargo.toml",
            "target/debug/app",
            "app/pubspec.yaml",
            "app/build/app.dill",
            "app/.dart_tool/package_config.json",
            "app/rust/Cargo.toml",
            "app/rust/target/debug/lib",
            "web/package.json",
            "web/node_modules/left-pad/index.js",
            "docs/build/index.html",
            "data/target/keep.csv",
            "tagged/CACHEDIR.TAG",
            ".env",
        ] {
            let path = root.join(file);
            std::fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
            let text: &[u8] = if file.ends_with("CACHEDIR.TAG") { CACHEDIR_SIGNATURE } else { b"x" };
            std::fs::write(path, text).expect("write");
        }
        std::fs::create_dir_all(root.join(".factory-trash-1-target")).expect("leftover");
        let (caches, leftovers) = find(&root);
        let found: Vec<String> = caches.iter().map(|path| path.strip_prefix(&root).expect("inside").display().to_string()).collect();
        let expected = ["app/.dart_tool", "app/build", "app/rust/target", "tagged", "target", "web/node_modules"];
        assert_eq!(found, expected.map(|path| path.replace('/', std::path::MAIN_SEPARATOR_STR)));
        assert_eq!(leftovers, [root.join(".factory-trash-1-target")]);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_cache_git_tracks_files_in_is_left() {
        let root = dir("tracked");
        std::fs::write(root.join("package.json"), "{}").expect("package");
        std::fs::create_dir_all(root.join("node_modules/vendored")).expect("mkdir");
        std::fs::write(root.join("node_modules/vendored/index.js"), "x").expect("write");
        let git = |args: &[&str]| std::process::Command::new("git").arg("-C").arg(&root).args(args).output().expect("git");
        git(&["init", "-q"]);
        git(&["add", "-f", "node_modules/vendored/index.js"]);
        let (caches, _) = find(&root);
        assert!(caches.is_empty(), "{caches:?}");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn only_stale_object_files_in_deps_are_swept() {
        let root = dir("objects");
        let target = root.join("target");
        let old = SystemTime::now() - Duration::from_secs(4 * 3600);
        let mut files = Vec::new();
        for file in [
            "debug/deps/factoryd-1a.factoryd.abc.rcgu.o",
            "debug/deps/libfactoryd-1a.rlib",
            "aarch64-apple-darwin/release/deps/x-2b.x.def.rcgu.o",
            "debug/incremental/x/s-1/x.rcgu.o",
            "debug/deps/fresh-3c.fresh.ghi.rcgu.o",
        ] {
            let path = target.join(file);
            std::fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
            std::fs::write(&path, b"object").expect("write");
            if !file.starts_with("debug/deps/fresh") {
                std::fs::File::options().write(true).open(&path).expect("open").set_modified(old).expect("age");
            }
            files.push(path);
        }
        assert_eq!(sweep_objects(&target).0, 2);
        let left: Vec<bool> = files.iter().map(|path| path.exists()).collect();
        assert_eq!(left, [false, true, false, true, true], "old objects in deps go; libraries, incremental state and new objects stay");
        let _ = std::fs::remove_dir_all(&root);
    }

    // Test binaries on Windows end in .exe, which a trim leaves alone.
    #[cfg(unix)]
    #[test]
    fn a_trim_takes_incremental_state_and_superseded_binaries_only() {
        let root = dir("trim");
        let target = root.join("target");
        let old = SystemTime::now() - Duration::from_secs(3600);
        let lately = SystemTime::now() - Duration::from_secs(60);
        let executable = |file: &str, modified: SystemTime| {
            let path = target.join(file);
            std::fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
            std::fs::write(&path, b"binary").expect("write");
            std::fs::set_permissions(&path, std::os::unix::fs::PermissionsExt::from_mode(0o755)).expect("mode");
            std::fs::File::options().write(true).open(&path).expect("open").set_modified(modified).expect("age");
            path
        };
        for file in ["debug/incremental/all-1/s-1/x.o", "aarch64-apple-darwin/release/incremental/x-2/s-2/y.o", "debug/deps/libx-0123456789abcdef.rlib"] {
            let path = target.join(file);
            std::fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
            std::fs::write(path, b"x").expect("write");
        }
        // Three builds of the tests: the newest stays, the oldest goes, and
        // the one replaced lately stays too.
        let oldest = executable("debug/deps/all-0000000000000001", old - Duration::from_secs(60));
        let recent = executable("debug/deps/all-0000000000000002", lately);
        let newest = executable("debug/deps/all-0000000000000003", SystemTime::now());
        // The only build of its name: stays, however old.
        let only = executable("debug/deps/factoryd-0000000000000004", old);
        assert!(!rust_target(&target), "no sign of Rust yet");
        std::fs::write(target.join(".rustc_info.json"), "{}").expect("rustc info");
        assert!(rust_target(&target));

        let found: Vec<PathBuf> = trimmable(&target, Instant::now() + SWEEP_BUDGET).into_iter().map(|(path, _)| path).collect();
        let mut found: Vec<String> = found.iter().map(|path| path.strip_prefix(&target).expect("inside").display().to_string()).collect();
        found.sort();
        assert_eq!(found, ["aarch64-apple-darwin/release/incremental", "debug/deps/all-0000000000000001", "debug/incremental"]);
        assert!(oldest.exists() && recent.exists() && newest.exists() && only.exists(), "only found, nothing deleted");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_build_holding_its_lock_is_in_use() {
        let root = dir("lock");
        std::fs::create_dir_all(root.join("target/debug")).expect("mkdir");
        let lock = root.join("target/debug/.cargo-lock");
        std::fs::write(&lock, "").expect("lock");
        assert!(!cargo_locked(&root.join("target")));
        let held = std::fs::File::open(&lock).expect("open");
        held.lock().expect("lock");
        assert!(cargo_locked(&root.join("target")));
        drop(held);
        let _ = std::fs::remove_dir_all(&root);
    }
}
