//! `factoryd export` (#217): writes this Claktory into a move bundle
//! (docs/MOVE.md, `move_bundle`) for `factoryd import` on another computer.
//!
//! - A plain export reads the Claktory as it runs: files are replaced whole
//!   when they change (so each is read as it was at one moment) and SQLite
//!   databases are copied with `VACUUM INTO`, which reads one consistent
//!   snapshot. It's for a rehearsal or a backup, and nothing here changes.
//! - `--final` is the move: it stops Claktory (turning its service off, so
//!   nothing starts it meanwhile) and its background tasks, writes the
//!   bundle, reads it back whole, and only then tombstones this copy:
//!   `moved.json`, and the fence (`auth.json`, `vault/`, `account/` and
//!   `remote/` into `moved/`), so no backend, of any version, can run its
//!   agents or use its identity here again. Nothing is deleted. If anything
//!   fails before the tombstone, Claktory starts again as it was.
//! - `--restore` undoes a tombstone; `--delete-moved` deletes the old copy
//!   for good, once the person has seen the moved one work.
//!
//! Every check that can fail runs before anything changes, and says what to
//! do in a sentence.

use std::collections::BTreeMap;
use std::fs::File;
use std::io::{self, IsTerminal, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};
use std::time::{Duration, Instant};

use serde::Serialize;

use crate::home::Home;
use crate::move_bundle::{self, Checkout, Folder, Manifest, Remote, Requires, Skipped, Source, Task, Writer};

/// What's fenced at `--final`: moved into `moved/` as they are.
pub const FENCED: [&str; 4] = ["auth.json", "vault", "account", "remote"];
/// Room left beside the bundle, over its estimate.
const HEADROOM: u64 = 256 << 20;
/// The memory exporting and importing need at most (Argon2id takes 64 MiB).
const MEMORY: u64 = 256 << 20;
/// How long Claktory has to stop, or start answering again.
const STOP_WITHIN: Duration = Duration::from_secs(40);
/// The home's entries a bundle never holds.
const LEFT_OUT: [&str; 12] = [
    "bin",
    "releases",
    "trees",
    "models",
    "trash",
    "moved",
    "moved.json",
    "move-epoch",
    "moving",
    "move-failed.json",
    "factoryd.pid",
    "update-failed.json",
];
/// Names left out of the workspace and plain folders: build output and the like.
const BUILD_OUTPUT: [&str; 2] = ["target", "node_modules"];

/// Where the Claktory is and what keeps it running.
#[derive(Clone)]
pub struct Place {
    pub home: Home,
    pub port: u16,
    pub user_home: PathBuf,
    pub dev: bool,
    pub service: Service,
}

/// What keeps Claktory running here, so `--final` can stop it and turn it off.
#[derive(Clone, Debug)]
pub enum Service {
    /// A LaunchAgent (macOS).
    Launchd { launchctl: PathBuf, domain: String, label: String, plist: PathBuf },
    /// A systemd user service (Linux).
    Systemd { systemctl: PathBuf, unit: String },
    /// Nothing this knows: Claktory must already be stopped for `--final`.
    None,
}

impl Place {
    /// This user's Claktory, as factoryd's other commands find it:
    /// `--dev`, `FACTORY_HOME` and `FACTORY_PORT` included.
    pub fn this_user(dev: bool) -> Option<Place> {
        let user_home = std::env::home_dir()?;
        let mut instance = crate::home::instance(dev, &user_home);
        let custom = std::env::var_os("FACTORY_HOME").is_some();
        if let Some(root) = std::env::var_os("FACTORY_HOME") {
            instance.home = Home::new(root.into());
        }
        if let Some(port) = std::env::var("FACTORY_PORT").ok().and_then(|port| port.parse().ok()) {
            instance.addr.set_port(port);
        }
        // Only the installed Claktory has a service; a dev or custom one runs
        // however it was started.
        let service = if dev || custom { Service::None } else { installed_service(&user_home) };
        Some(Place { home: instance.home, port: instance.addr.port(), user_home, dev, service })
    }
}

fn installed_service(user_home: &Path) -> Service {
    if cfg!(target_os = "macos") {
        let Some(job) = crate::launch_agent::Job::for_this_user() else { return Service::None };
        if job.plist.is_file() {
            return Service::Launchd {
                launchctl: job.launchctl,
                domain: job.domain,
                label: crate::launch_agent::LABEL.to_owned(),
                plist: job.plist,
            };
        }
    } else if std::env::var_os("XDG_CONFIG_HOME")
        .filter(|config| !config.is_empty())
        .map_or_else(|| user_home.join(".config"), PathBuf::from)
        .join("systemd/user/factoryd.service")
        .is_file()
    {
        return Service::Systemd { systemctl: PathBuf::from("systemctl"), unit: "factoryd.service".into() };
    }
    Service::None
}

/// What `export` was asked.
#[derive(Clone, Debug, Default)]
pub struct Options {
    pub is_final: bool,
    /// The bundle's path; a new one in the current folder when None.
    pub out: Option<PathBuf>,
    /// Project folders to leave out.
    pub skip: Vec<PathBuf>,
}

/// Why it didn't go: a sentence for the person, and whether anything changed.
#[derive(Debug)]
pub struct Refused {
    pub reason: String,
}

fn refuse(reason: impl Into<String>) -> Refused {
    Refused { reason: reason.into() }
}

/// A bundle written and read back whole.
#[derive(Debug, Serialize)]
pub struct Exported {
    pub bundle: PathBuf,
    /// Printed once, never kept.
    pub passphrase: String,
    pub manifest: Manifest,
    /// Size of the bundle file.
    pub size: u64,
    /// What wasn't carried, file by file (a link that leads outside, say).
    pub warnings: Vec<String>,
}

/// One thing to write from the home, a plain folder or untracked files.
#[derive(Debug)]
pub(crate) struct Entry {
    /// Its path in the bundle.
    path: String,
    source: PathBuf,
    kind: EntryKind,
}

#[derive(Debug)]
pub(crate) enum EntryKind {
    File {
        size: u64,
    },
    /// A SQLite database: copied as a snapshot.
    Database {
        size: u64,
    },
    Dir,
    Symlink {
        target: String,
    },
}

/// A project folder as planned.
pub(crate) struct FolderPlan {
    folder: Folder,
    /// Where it is (the person's spelling, as config.json has it).
    root: PathBuf,
    /// Untracked files for git, every file for plain.
    entries: Vec<Entry>,
    /// Git's own folder's size: the bundle's estimate.
    git_bytes: u64,
    /// Files git tracks, checked out on the target.
    tracked_bytes: u64,
}

pub(crate) struct CheckoutPlan {
    checkout: Checkout,
    entries: Vec<Entry>,
}

pub(crate) struct Plan {
    home: Vec<Entry>,
    folders: Vec<FolderPlan>,
    skipped: Vec<Skipped>,
    checkouts: Vec<CheckoutPlan>,
    tasks: Vec<Task>,
    warnings: Vec<String>,
}

impl Plan {
    fn content_bytes(&self) -> u64 {
        let entries = |entries: &[Entry]| entries.iter().map(Entry::size).sum::<u64>();
        entries(&self.home)
            + self.folders.iter().map(|folder| entries(&folder.entries) + folder.git_bytes).sum::<u64>()
            + self.checkouts.iter().map(|checkout| entries(&checkout.entries)).sum::<u64>()
    }

    /// What's staged beside the bundle while it's written: databases'
    /// snapshots and git bundles.
    pub(crate) fn staged_estimate(&self) -> u64 {
        let databases: u64 = self.home.iter().filter(|entry| matches!(entry.kind, EntryKind::Database { .. })).map(Entry::size).sum();
        databases + self.folders.iter().map(|folder| folder.git_bytes).sum::<u64>()
    }
}

impl Entry {
    fn size(&self) -> u64 {
        match self.kind {
            EntryKind::File { size } | EntryKind::Database { size } => size,
            _ => 0,
        }
    }
}

/// Writes the bundle; with `is_final`, the move: see the module's header.
pub fn export(place: &Place, options: &Options) -> Result<Exported, Refused> {
    let home = place.home.root();
    if !place.home.config_path().is_file() {
        return Err(refuse(format!("There's no Claktory at {}.", home.display())));
    }
    if let Some(moved) = read_moved(&place.home) {
        return Err(refuse(format!(
            "This Claktory already moved to another computer ({}). To bring it back here, run factoryd export --restore.",
            when(moved.at_ms)
        )));
    }
    let plan = plan(place, options)?;
    let out = out_path(place, options)?;
    precheck(place, options, &plan, &out)?;
    if !options.is_final {
        return write_checked(place, options, plan, &out);
    }
    // The move: Claktory stops, and starts again unless it's tombstoned.
    stop(place)?;
    crate::uninstall::stop_leftovers(&place.home);
    let written = write_checked(place, options, plan, &out).and_then(|exported| {
        let moved = Moved {
            export_id: exported.manifest.export_id.clone(),
            epoch: exported.manifest.epoch,
            machine: exported.manifest.source.machine.clone(),
            bundle: exported.bundle.display().to_string(),
            ..Moved::default()
        };
        tombstone(place, moved).map(|()| exported).inspect_err(|_| {
            let _ = std::fs::remove_file(&out);
        })
    });
    if written.is_err()
        && let Err(problem) = start(place)
    {
        let reason = written.err().map(|refused| refused.reason).unwrap_or_default();
        return Err(refuse(format!("{reason} Claktory didn't start again by itself ({problem}): start it from the Claktory app.")));
    }
    written
}

/// The bundle's place: free, outside the Claktory and the folders it holds.
fn out_path(place: &Place, options: &Options) -> Result<PathBuf, Refused> {
    let out = match &options.out {
        Some(out) => absolute(out),
        None => {
            let stamp = jiff::Timestamp::now().to_zoned_datetime(jiff::tz::TimeZone::system()).strftime("%Y%m%d-%H%M").to_string();
            let machine: String = crate::remote::machine_name()
                .chars()
                .map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_lowercase() } else { '-' })
                .collect::<String>()
                .split('-')
                .filter(|part| !part.is_empty())
                .collect::<Vec<_>>()
                .join("-");
            absolute(Path::new(&format!("claktory-{machine}-{stamp}.claktory")))
        }
    };
    if out.exists() {
        return Err(refuse(format!("{} already exists. Choose another name with --out.", out.display())));
    }
    let parent = out.parent().unwrap_or(Path::new("/"));
    if !parent.is_dir() {
        return Err(refuse(format!("There's no folder {} to put the bundle in.", parent.display())));
    }
    let probe = parent.join(format!(".claktory-export-{}", std::process::id()));
    if File::create(&probe).and_then(|_| std::fs::remove_file(&probe)).is_err() {
        return Err(refuse(format!("The bundle can't be written in {}. Choose another place with --out.", parent.display())));
    }
    let real_parent = parent.canonicalize().unwrap_or_else(|_| parent.to_path_buf());
    let real_home = place.home.root().canonicalize().unwrap_or_else(|_| place.home.root().to_path_buf());
    if real_parent.starts_with(&real_home) {
        return Err(refuse(format!(
            "The bundle can't go inside Claktory's own folder ({}): choose a place outside it with --out.",
            place.home.root().display()
        )));
    }
    Ok(out)
}

fn absolute(path: &Path) -> PathBuf {
    if path.is_absolute() { path.to_path_buf() } else { std::env::current_dir().unwrap_or_default().join(path) }
}

/// Checks everything that can be checked before anything changes.
fn precheck(place: &Place, options: &Options, plan: &Plan, out: &Path) -> Result<(), Refused> {
    let parent = out.parent().unwrap_or(Path::new("/"));
    let real_parent = parent.canonicalize().unwrap_or_else(|_| parent.to_path_buf());
    for folder in &plan.folders {
        let root = folder.root.canonicalize().unwrap_or_else(|_| folder.root.clone());
        if real_parent.starts_with(&root) {
            return Err(refuse(format!(
                "The bundle can't go inside {}, which it carries: choose a place outside it with --out.",
                folder.root.display()
            )));
        }
    }
    let gits = plan.folders.iter().filter(|folder| folder.folder.kind == "git").count() + plan.checkouts.len();
    if gits > 0 && !git_works() {
        return Err(refuse("git isn't installed on this computer, and the projects' folders need it. Install git and try again."));
    }
    let estimate = plan.content_bytes();
    let needed = estimate + plan.staged_estimate() + HEADROOM;
    if let Some(free) = free_space(parent)
        && free < needed
    {
        return Err(refuse(format!(
            "There isn't enough free space in {} for the bundle: it needs about {} and {} is free. Free some up, or choose another place with --out.",
            parent.display(),
            size_text(needed),
            size_text(free)
        )));
    }
    if let Some(available) = available_memory()
        && available < MEMORY
    {
        return Err(refuse(format!(
            "This computer has too little free memory to make the bundle: it needs {}, and {} is free. Quit some apps and try again.",
            size_text(MEMORY),
            size_text(available)
        )));
    }
    if options.is_final {
        stoppable(place, "factoryd export --final")?;
    }
    Ok(())
}

/// Whether `command` can stop this Claktory: it doesn't run inside it, and
/// either a service keeps it running or it's stopped already.
pub(crate) fn stoppable(place: &Place, command: &str) -> Result<(), Refused> {
    if inside_this_claktory(&place.home) {
        return Err(refuse(format!(
            "Moving stops this Claktory, and this command runs inside it (an agent's command), so it would stop too. Run {command} in Terminal instead."
        )));
    }
    if matches!(place.service, Service::None) && crate::install::answers(place.port, Duration::from_millis(300)) {
        return Err(refuse(format!(
            "This Claktory is running (on port {}), and this can't stop it. Stop it first, then try again.",
            place.port
        )));
    }
    Ok(())
}

/// Writes the bundle beside its final name, reads it back whole, then gives
/// it its name. Leaves nothing behind when it fails.
fn write_checked(place: &Place, options: &Options, plan: Plan, out: &Path) -> Result<Exported, Refused> {
    let name = out.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
    let parent = out.parent().unwrap_or(Path::new("/"));
    let partial = parent.join(format!(".{name}.partial"));
    let staging = parent.join(format!(".{name}.staging"));
    let cleanup = || {
        let _ = std::fs::remove_file(&partial);
        let _ = std::fs::remove_dir_all(&staging);
    };
    let (passphrase, key) = move_bundle::new_passphrase();
    let written = (|| -> Result<(Manifest, Vec<String>), String> {
        std::fs::create_dir_all(&staging).map_err(|error| format!("{}: {error}", staging.display()))?;
        let file = File::create(&partial).map_err(|error| format!("{}: {error}", partial.display()))?;
        let mut writer = Writer::new(io::BufWriter::new(file), &key).map_err(|error| error.to_string())?;
        let (manifest, warnings) = write(place, options, plan, &mut writer, &staging)?;
        let file = writer.finish().map_err(|error| error.to_string())?;
        let file = file.into_inner().map_err(|error| error.into_error().to_string())?;
        file.sync_all().map_err(|error| error.to_string())?;
        Ok((manifest, warnings))
    })();
    let _ = std::fs::remove_dir_all(&staging);
    let (manifest, warnings) = match written {
        Ok(written) => written,
        Err(problem) => {
            cleanup();
            return Err(refuse(format!("The bundle couldn't be written: {problem}. Nothing was changed.")));
        }
    };
    // Read back whole before anything relies on it.
    match move_bundle::check(&partial, &passphrase) {
        Ok(read) if read.export_id == manifest.export_id => {}
        Ok(_) => {
            cleanup();
            return Err(refuse("The bundle read back as another one. Nothing was changed."));
        }
        Err(error) => {
            cleanup();
            return Err(refuse(format!("The bundle didn't read back: {error} Nothing was changed.")));
        }
    }
    if let Err(error) = std::fs::rename(&partial, out) {
        cleanup();
        return Err(refuse(format!("The bundle couldn't be named {}: {error}. Nothing was changed.", out.display())));
    }
    let size = std::fs::metadata(out).map(|meta| meta.len()).unwrap_or(0);
    Ok(Exported { bundle: out.to_path_buf(), passphrase, manifest, size, warnings })
}

/// One path of the tree a bundle (or a move) carries, and where its
/// content is now: the Claktory's own file (read live), or one staged.
#[derive(Clone, Debug)]
pub(crate) struct Item {
    /// Its path in the tree (`home/config.json`).
    pub(crate) path: String,
    pub(crate) kind: ItemKind,
}

#[derive(Clone, Debug)]
pub(crate) enum ItemKind {
    Dir {
        mode: u32,
    },
    /// `live`: the Claktory's own file, which may change or go while it's read.
    File {
        source: PathBuf,
        live: bool,
    },
    Symlink {
        target: String,
    },
}

/// What's carried, ready to read: the manifest (staged as manifest.json,
/// the first item), every item, and what couldn't be carried.
pub(crate) struct Tree {
    pub(crate) manifest: Manifest,
    pub(crate) items: Vec<Item>,
    pub(crate) warnings: Vec<String>,
}

/// Makes what isn't a plain file of the Claktory's into one in `staging`
/// (databases' snapshots, git bundles and patches, the manifest), and lists
/// the whole tree. Each call stages afresh: a move's every round.
pub(crate) fn stage(place: &Place, export_id: &str, is_final: bool, plan: &Plan, staging: &Path) -> Result<Tree, String> {
    let Plan { home, folders, skipped, checkouts, tasks, warnings } = plan;
    let mut warnings = warnings.clone();
    let mut folders: Vec<Folder> = folders.iter().map(|folder| folder.folder.clone()).collect();
    let mut checkout_records: Vec<Checkout> = checkouts.iter().map(|checkout| checkout.checkout.clone()).collect();
    // Git bundles and patches first, so their sizes are known.
    let mut staged: BTreeMap<String, PathBuf> = BTreeMap::new();
    for (n, (folder, planned)) in folders.iter_mut().zip(&plan.folders).enumerate() {
        if folder.kind != "git" || folder.head.is_empty() {
            continue;
        }
        let root = &planned.root;
        let bundle = staging.join(format!("folder-{n}.bundle"));
        git(root, &["bundle".as_ref(), "create".as_ref(), bundle.as_os_str(), "--all".as_ref()])?;
        staged.insert(format!("folders/{n}/repo.bundle"), bundle);
        folder.bundle = Some(format!("folders/{n}/repo.bundle"));
        let patch = staging.join(format!("folder-{n}.patch"));
        if stage_patch(root, &patch)? {
            staged.insert(format!("folders/{n}/changes.patch"), patch);
            folder.patch = Some(format!("folders/{n}/changes.patch"));
        }
    }
    for checkout in &mut checkout_records {
        let dir = format!("checkouts/{}/{}", checkout.agent, checkout.name);
        let patch = staging.join(format!("checkout-{}-{}.patch", checkout.agent, checkout.name));
        if stage_patch(Path::new(&checkout.tree), &patch)? {
            staged.insert(format!("{dir}/changes.patch"), patch);
            checkout.patch = Some(format!("{dir}/changes.patch"));
        }
    }
    // Databases as snapshots, and everything else as it is.
    let mut sqlite = 0;
    let mut listed = |entries: &[Entry], items: &mut Vec<Item>| -> Result<(), String> {
        for entry in entries {
            let kind = match &entry.kind {
                EntryKind::Dir => ItemKind::Dir { mode: mode_of(&entry.source).unwrap_or(0o755) },
                EntryKind::Symlink { target } => ItemKind::Symlink { target: target.clone() },
                EntryKind::File { .. } => ItemKind::File { source: entry.source.clone(), live: true },
                EntryKind::Database { .. } => {
                    sqlite += 1;
                    let copy = staging.join(format!("database-{sqlite}.sqlite"));
                    snapshot(&entry.source, &copy)?;
                    set_mode(&copy, mode_of(&entry.source).unwrap_or(0o600));
                    ItemKind::File { source: copy, live: false }
                }
            };
            items.push(Item { path: entry.path.clone(), kind });
        }
        Ok(())
    };
    let mut items =
        vec![Item { path: "manifest.json".into(), kind: ItemKind::File { source: staging.join("manifest.json"), live: false } }];
    listed(home, &mut items)?;
    items.extend(staged.iter().map(|(path, file)| Item { path: path.clone(), kind: ItemKind::File { source: file.clone(), live: false } }));
    for folder in &plan.folders {
        listed(&folder.entries, &mut items)?;
    }
    for checkout in checkouts {
        listed(&checkout.entries, &mut items)?;
    }
    let mut manifest = Manifest {
        format: move_bundle::FORMAT,
        export_id: export_id.to_owned(),
        created_ms: crate::transcript::now_ms(),
        is_final,
        version: crate::update::VERSION.to_owned(),
        epoch: epoch(&place.home) + 1,
        source: Source {
            machine: crate::remote::machine_name().to_owned(),
            os: std::env::consts::OS.to_owned(),
            arch: std::env::consts::ARCH.to_owned(),
            user: std::env::var("USER")
                .ok()
                .filter(|user| !user.is_empty())
                .or_else(|| place.user_home.file_name().map(|name| name.to_string_lossy().into_owned()))
                .unwrap_or_default(),
            user_home: place.user_home.display().to_string(),
            home: place.home.root().display().to_string(),
            dev: place.dev,
        },
        files: 0,
        bytes: 0,
        requires: Requires::default(),
        folders,
        skipped: skipped.clone(),
        checkouts: checkout_records,
        tasks: tasks.clone(),
    };
    let size_of = |item: &Item| match &item.kind {
        ItemKind::File { source, .. } => Some(file_size(source)),
        _ => None,
    };
    let files: Vec<u64> = items[1..].iter().filter_map(size_of).collect();
    manifest.files = files.len() as u64;
    manifest.bytes = files.iter().sum();
    for (n, folder) in manifest.folders.iter_mut().enumerate() {
        let prefix = format!("folders/{n}/");
        folder.bytes = items.iter().filter(|item| item.path.starts_with(&prefix)).filter_map(size_of).sum();
    }
    // Unpacked, then the folders checked out again beside their history.
    let tracked: u64 = plan.folders.iter().map(|folder| folder.tracked_bytes).sum();
    manifest.requires = Requires { disk_bytes: (manifest.bytes + tracked) / 10 * 11 + HEADROOM, memory_bytes: MEMORY };
    let json = serde_json::to_vec_pretty(&manifest).map_err(|error| error.to_string())?;
    std::fs::write(staging.join("manifest.json"), &json).map_err(|error| format!("{}: {error}", staging.display()))?;
    // Counted with it: the manifest itself.
    warnings.dedup();
    Ok(Tree { manifest, items, warnings })
}

/// A new export's id.
pub(crate) fn new_export_id() -> String {
    crate::encoding::hex(&crate::vault::crypto::random::<16>())
}

fn set_mode(path: &Path, mode: u32) {
    use std::os::unix::fs::PermissionsExt;
    let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode & 0o777));
}

/// Everything into `writer`, the manifest first. Returns the manifest and
/// what couldn't be carried.
fn write(
    place: &Place,
    options: &Options,
    plan: Plan,
    writer: &mut Writer<io::BufWriter<File>>,
    staging: &Path,
) -> Result<(Manifest, Vec<String>), String> {
    let Tree { manifest, items, mut warnings } = stage(place, &new_export_id(), options.is_final, &plan, staging)?;
    for item in &items {
        put(writer, item, &mut warnings)?;
    }
    Ok((manifest, warnings))
}

fn file_size(path: &Path) -> u64 {
    std::fs::metadata(path).map(|meta| meta.len()).unwrap_or(0)
}

/// One item into the bundle, as it is now. A file of the Claktory's that's
/// gone meanwhile (a live export) is noted, not an error.
fn put(writer: &mut Writer<io::BufWriter<File>>, item: &Item, warnings: &mut Vec<String>) -> Result<(), String> {
    match &item.kind {
        ItemKind::Dir { mode } => writer.dir(&item.path, *mode).map_err(|error| format!("{}: {error}", item.path)),
        ItemKind::Symlink { target } => writer.symlink(&item.path, target).map_err(|error| format!("{}: {error}", item.path)),
        ItemKind::File { source, live } => {
            let failed = |error: io::Error| format!("{}: {error}", source.display());
            let file = match File::open(source) {
                Ok(file) => file,
                Err(error) if *live && error.kind() == io::ErrorKind::NotFound => {
                    warnings.push(format!("{} was deleted while the bundle was made", source.display()));
                    return Ok(());
                }
                Err(error) => return Err(failed(error)),
            };
            let meta = file.metadata().map_err(failed)?;
            writer.file(&item.path, permissions(&meta), modified_ms(&meta), meta.len(), file).map_err(failed)
        }
    }
}

/// A consistent copy of a SQLite database, live or not.
fn snapshot(database: &Path, copy: &Path) -> Result<(), String> {
    let flags = rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX;
    let failed = |error: rusqlite::Error| format!("{}: {error}", database.display());
    let db = rusqlite::Connection::open_with_flags(database, flags).map_err(failed)?;
    db.busy_timeout(Duration::from_secs(30)).map_err(failed)?;
    db.execute("VACUUM INTO ?1", [copy.display().to_string()]).map_err(failed)?;
    Ok(())
}

fn permissions(meta: &std::fs::Metadata) -> u32 {
    use std::os::unix::fs::PermissionsExt;
    meta.permissions().mode() & 0o777
}

fn mode_of(path: &Path) -> Option<u32> {
    std::fs::metadata(path).ok().map(|meta| permissions(&meta))
}

fn modified_ms(meta: &std::fs::Metadata) -> i64 {
    meta.modified().ok().and_then(|at| at.duration_since(std::time::UNIX_EPOCH).ok()).map_or(0, |at| at.as_millis() as i64)
}

/// The move epoch in `<home>/move-epoch`; 0 when there's none.
fn epoch(home: &Home) -> u64 {
    std::fs::read_to_string(home.root().join("move-epoch")).ok().and_then(|text| text.trim().parse().ok()).unwrap_or(0)
}

/// What goes into the bundle, from what's on disk and in config.json.
pub(crate) fn plan(place: &Place, options: &Options) -> Result<Plan, Refused> {
    let home = place.home.root();
    let config: serde_json::Value =
        std::fs::read_to_string(place.home.config_path()).ok().and_then(|text| serde_json::from_str(&text).ok()).ok_or_else(|| {
            refuse("Claktory's settings (config.json) can't be read, so it can't be moved. Open Claktory once to repair them.")
        })?;
    let mut warnings = Vec::new();
    let skip: Vec<PathBuf> = options.skip.iter().map(|path| absolute(path)).collect();
    let (folders, skipped) = plan_folders(place, &config, &skip, &mut warnings);
    let roots: Vec<PathBuf> = folders.iter().map(|folder| folder.root.clone()).collect();
    let mut home_entries = Vec::new();
    walk(home, "home", &mut home_entries, &mut warnings, &|relative, name, is_dir| {
        // Top level: what's regenerable or this machine's.
        if !relative.contains('/') && (LEFT_OUT.contains(&name) || name.starts_with("factoryd.log") || name.starts_with(".removing-")) {
            return false;
        }
        if name == ".DS_Store" || (is_dir && name == ".partial") {
            return false;
        }
        if !is_dir && sqlite_sidecar(&home.join(relative)) {
            return false;
        }
        // Project folders go as folders, wherever they are.
        let path = home.join(relative);
        if roots.contains(&path) {
            return false;
        }
        !(relative.starts_with("workspace/") && is_dir && BUILD_OUTPUT.contains(&name))
    });
    let checkouts = plan_checkouts(place, &mut warnings);
    let tasks = running_tasks(&place.home);
    Ok(Plan { home: home_entries, folders, skipped, checkouts, tasks, warnings })
}

/// A database's `-wal`, `-shm` or `-journal`, beside the database.
fn sqlite_sidecar(path: &Path) -> bool {
    let name = path.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
    ["-wal", "-shm", "-journal"].iter().any(|suffix| name.strip_suffix(suffix).is_some_and(|base| path.with_file_name(base).is_file()))
}

fn is_sqlite(path: &Path) -> bool {
    let mut magic = [0u8; 16];
    File::open(path).and_then(|mut file| file.read_exact(&mut magic)).is_ok() && &magic == b"SQLite format 3\0"
}

/// Lists `root` into `entries` under `prefix`: every folder, file and link
/// that `keep(relative path, name, is a folder)` keeps, links not followed.
fn walk(root: &Path, prefix: &str, entries: &mut Vec<Entry>, warnings: &mut Vec<String>, keep: &dyn Fn(&str, &str, bool) -> bool) {
    fn visit(
        root: &Path,
        dir: &Path,
        prefix: &str,
        entries: &mut Vec<Entry>,
        warnings: &mut Vec<String>,
        keep: &dyn Fn(&str, &str, bool) -> bool,
    ) {
        let Ok(read) = std::fs::read_dir(dir) else {
            warnings.push(format!("{} couldn't be read", dir.display()));
            return;
        };
        let mut children: Vec<_> = read.filter_map(Result::ok).collect();
        children.sort_by_key(|child| child.file_name());
        for child in children {
            let path = child.path();
            let Ok(relative) = path.strip_prefix(root) else { continue };
            let Some(relative) = relative.to_str() else {
                warnings.push(format!("{} has a name that isn't UTF-8", path.display()));
                continue;
            };
            let name = child.file_name().to_string_lossy().into_owned();
            let Ok(meta) = std::fs::symlink_metadata(&path) else { continue };
            if !keep(relative, &name, meta.is_dir()) {
                continue;
            }
            let bundle_path = format!("{prefix}/{relative}");
            if meta.is_dir() {
                entries.push(Entry { path: bundle_path, source: path.clone(), kind: EntryKind::Dir });
                visit(root, &path, prefix, entries, warnings, keep);
            } else if meta.file_type().is_symlink() {
                link(&path, bundle_path, entries, warnings);
            } else if meta.is_file() {
                let size = meta.len();
                let kind = if is_sqlite(&path) { EntryKind::Database { size } } else { EntryKind::File { size } };
                entries.push(Entry { path: bundle_path, source: path, kind });
            }
        }
    }
    visit(root, root, prefix, entries, warnings, keep);
}

/// A link, kept when it stays inside what's carried.
fn link(path: &Path, bundle_path: String, entries: &mut Vec<Entry>, warnings: &mut Vec<String>) {
    let target = std::fs::read_link(path).ok().and_then(|target| target.to_str().map(str::to_owned));
    let inside = target.as_deref().is_some_and(|target| {
        let mut depth: Vec<&str> = bundle_path.split('/').collect();
        depth.pop();
        // Not into another part of the bundle: within its own top two levels.
        let floor = 2.min(depth.len());
        !target.starts_with('/')
            && target.split('/').all(|part| match part {
                "" | "." => true,
                ".." => depth.len() > floor && depth.pop().is_some(),
                part => {
                    depth.push(part);
                    true
                }
            })
    });
    match target {
        Some(target) if inside => {
            entries.push(Entry { path: bundle_path, source: path.to_path_buf(), kind: EntryKind::Symlink { target } })
        }
        _ => warnings.push(format!("{} is a link to somewhere that doesn't move with it, so it was left out", path.display())),
    }
}

/// The folders the projects use, each once: a git repository's top folder,
/// or a plain folder; one inside another goes with it.
fn plan_folders(
    place: &Place,
    config: &serde_json::Value,
    skip: &[PathBuf],
    warnings: &mut Vec<String>,
) -> (Vec<FolderPlan>, Vec<Skipped>) {
    let home = place.home.root();
    let real_home = home.canonicalize().unwrap_or_else(|_| home.to_path_buf());
    let mut used: BTreeMap<PathBuf, Vec<String>> = BTreeMap::new();
    for project in config["projects"].as_array().into_iter().flatten() {
        let id = project["id"].as_str().unwrap_or_default().to_owned();
        let paths =
            std::iter::once(&project["path"]).chain(project["folders"].as_array().into_iter().flatten().map(|folder| &folder["path"]));
        for path in paths.filter_map(serde_json::Value::as_str).filter(|path| !path.is_empty()) {
            used.entry(PathBuf::from(path)).or_default().push(id.clone());
        }
    }
    let mut skipped = Vec::new();
    // Each path's root: its repository's top folder, or itself.
    let mut roots: BTreeMap<PathBuf, (bool, Vec<String>)> = BTreeMap::new();
    for (path, projects) in used {
        let skip_it =
            |why: &str, skipped: &mut Vec<Skipped>| skipped.push(Skipped { path: path.display().to_string(), why: why.to_owned() });
        if skip.iter().any(|skipped| path.starts_with(skipped)) {
            skip_it("left out (--skip)", &mut skipped);
            continue;
        }
        if !path.is_dir() {
            skip_it("missing", &mut skipped);
            continue;
        }
        let real = path.canonicalize().unwrap_or_else(|_| path.clone());
        if real_home.starts_with(&real) {
            skip_it("it holds Claktory's own folder; move it yourself", &mut skipped);
            continue;
        }
        if real.starts_with(real_home.join("trees")) {
            skip_it("an agent's checkout, which goes with its agent", &mut skipped);
            continue;
        }
        let (root, git) = match git_top(&path) {
            // A repository around Claktory's folder is carried as plain folders.
            Some(top) if !real_home.starts_with(top.canonicalize().unwrap_or_else(|_| top.clone())) => (top, true),
            _ => (path.clone(), false),
        };
        if skip.iter().any(|skipped| root.starts_with(skipped)) {
            skip_it("left out (--skip)", &mut skipped);
            continue;
        }
        let entry = roots.entry(root).or_insert((git, Vec::new()));
        for project in projects {
            if !entry.1.contains(&project) {
                entry.1.push(project);
            }
        }
    }
    // One inside another goes with it (BTreeMap order puts parents first).
    let mut kept: Vec<(PathBuf, bool, Vec<String>)> = Vec::new();
    for (root, (git, projects)) in roots {
        if let Some(outer) = kept.iter_mut().find(|(outer, _, _)| root.starts_with(outer)) {
            for project in projects {
                if !outer.2.contains(&project) {
                    outer.2.push(project);
                }
            }
            continue;
        }
        kept.push((root, git, projects));
    }
    let mut folders = Vec::new();
    for (n, (root, git, projects)) in kept.into_iter().enumerate() {
        let mut folder =
            Folder { path: root.display().to_string(), kind: if git { "git" } else { "plain" }.to_owned(), projects, ..Folder::default() };
        let mut entries = Vec::new();
        let (mut git_bytes, mut tracked_bytes) = (0, 0);
        if git {
            folder.head = git_out(&root, &["rev-parse", "-q", "--verify", "HEAD"]).unwrap_or_default();
            folder.branch = git_out(&root, &["symbolic-ref", "--short", "-q", "HEAD"]).unwrap_or_default();
            folder.remotes = remotes(&root);
            let prefix = format!("folders/{n}/untracked");
            // With no commit yet, what's staged has no history to come from.
            let listed: &[&str] = if folder.head.is_empty() { &["--cached", "--others"] } else { &["--others"] };
            untracked(&root, listed, &prefix, &mut entries, warnings);
            if !entries.is_empty() {
                folder.untracked = Some(prefix);
            }
            git_bytes = dir_size(&root.join(".git"));
            tracked_bytes = tracked_size(&root);
        } else {
            let prefix = format!("folders/{n}/files");
            walk(&root, &prefix, &mut entries, warnings, &|_, name, is_dir| {
                name != ".DS_Store" && !(is_dir && BUILD_OUTPUT.contains(&name))
            });
            folder.files = Some(prefix);
        }
        folders.push(FolderPlan { folder, root, entries, git_bytes, tracked_bytes });
    }
    (folders, skipped)
}

/// The top folder of the git repository `path` is in, in `path`'s spelling.
fn git_top(path: &Path) -> Option<PathBuf> {
    let up = git_out(path, &["rev-parse", "--show-cdup"])?;
    let mut top = path.to_path_buf();
    for _ in up.split('/').filter(|part| *part == "..") {
        top.pop();
    }
    top.join(".git").exists().then_some(top)
}

fn remotes(root: &Path) -> Vec<Remote> {
    let names = git_out(root, &["remote"]).unwrap_or_default();
    names
        .lines()
        .filter(|name| !name.is_empty())
        .map(|name| Remote { name: name.to_owned(), url: git_out(root, &["remote", "get-url", name]).unwrap_or_default() })
        .collect()
}

/// Files git lists in `root` (untracked and not ignored, by default) as entries under `prefix`.
fn untracked(root: &Path, which: &[&str], prefix: &str, entries: &mut Vec<Entry>, warnings: &mut Vec<String>) {
    let mut args = vec!["ls-files", "-z", "--exclude-standard"];
    args.extend_from_slice(which);
    let Ok(output) = git_command(root).args(&args).output() else { return };
    for name in output.stdout.split(|byte| *byte == 0).filter(|name| !name.is_empty()) {
        let Ok(name) = std::str::from_utf8(name) else {
            warnings.push(format!("a file in {} has a name that isn't UTF-8", root.display()));
            continue;
        };
        let path = root.join(name);
        let Ok(meta) = std::fs::symlink_metadata(&path) else { continue };
        let bundle_path = format!("{prefix}/{name}");
        if meta.file_type().is_symlink() {
            link(&path, bundle_path, entries, warnings);
        } else if meta.is_file() {
            entries.push(Entry { path: bundle_path, source: path, kind: EntryKind::File { size: meta.len() } });
        }
    }
}

/// The size of the files git tracks in `root`.
fn tracked_size(root: &Path) -> u64 {
    let Ok(output) = git_command(root).args(["ls-files", "-z"]).output() else { return 0 };
    output
        .stdout
        .split(|byte| *byte == 0)
        .filter_map(|name| std::str::from_utf8(name).ok())
        .filter_map(|name| std::fs::symlink_metadata(root.join(name)).ok())
        .map(|meta| meta.len())
        .sum()
}

fn dir_size(dir: &Path) -> u64 {
    let Ok(read) = std::fs::read_dir(dir) else { return 0 };
    read.filter_map(Result::ok)
        .filter_map(|entry| Some((entry.path(), std::fs::symlink_metadata(entry.path()).ok()?)))
        .map(|(path, meta)| if meta.is_dir() { dir_size(&path) } else { meta.len() })
        .sum()
}

/// Agents' own checkouts, from their records in `<home>/trees`.
fn plan_checkouts(place: &Place, warnings: &mut Vec<String>) -> Vec<CheckoutPlan> {
    let trees = crate::trees::trees_dir(&place.home);
    let mut checkouts = Vec::new();
    let Ok(agents) = std::fs::read_dir(&trees) else { return checkouts };
    let mut agents: Vec<_> = agents.filter_map(Result::ok).filter(|entry| entry.path().is_dir()).collect();
    agents.sort_by_key(|entry| entry.file_name());
    for agent in agents {
        let agent_id = agent.file_name().to_string_lossy().into_owned();
        let Ok(records) = std::fs::read_dir(agent.path()) else { continue };
        let mut records: Vec<_> = records.filter_map(Result::ok).map(|entry| entry.path()).collect();
        records.sort();
        for record in records.into_iter().filter(|path| path.extension().is_some_and(|ext| ext == "json")) {
            let Some(saved) = std::fs::read_to_string(&record).ok().and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok())
            else {
                continue;
            };
            let tree = record.with_extension("");
            if !tree.join(".git").exists() {
                continue;
            }
            let name = tree.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
            let prefix = format!("checkouts/{agent_id}/{name}/untracked");
            let mut entries = Vec::new();
            untracked(&tree, &["--others"], &prefix, &mut entries, warnings);
            let checkout = Checkout {
                agent: agent_id.clone(),
                name,
                tree: tree.display().to_string(),
                repo: saved["repo"].as_str().unwrap_or_default().to_owned(),
                branch: saved["branch"].as_str().unwrap_or_default().to_owned(),
                head: git_out(&tree, &["rev-parse", "-q", "--verify", "HEAD"]).unwrap_or_default(),
                patch: None,
                untracked: (!entries.is_empty()).then_some(prefix),
            };
            checkouts.push(CheckoutPlan { checkout, entries });
        }
    }
    checkouts
}

/// Background tasks factory.db says are still running.
fn running_tasks(home: &Home) -> Vec<Task> {
    let flags = rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX;
    let Ok(db) = rusqlite::Connection::open_with_flags(home.db_path(), flags) else { return Vec::new() };
    let Ok(mut statement) = db.prepare("SELECT agent, task, command, started FROM tasks WHERE ended IS NULL ORDER BY id") else {
        return Vec::new();
    };
    statement
        .query_map([], |row| Ok(Task { agent: row.get(0)?, task: row.get(1)?, command: row.get(2)?, started_ms: row.get(3)? }))
        .map(|rows| rows.filter_map(Result::ok).collect())
        .unwrap_or_default()
}

/// The uncommitted changes to tracked files in `root`, into `patch`; false
/// when there are none.
fn stage_patch(root: &Path, patch: &Path) -> Result<bool, String> {
    let file = File::create(patch).map_err(|error| format!("{}: {error}", patch.display()))?;
    let status = git_command(root)
        .args(["diff", "--binary", "--no-color", "--no-ext-diff", "--full-index", "HEAD"])
        .stdout(file)
        .stderr(Stdio::piped())
        .output()
        .map_err(|error| format!("git: {error}"))?;
    if !status.status.success() {
        return Err(format!("git diff in {}: {}", root.display(), String::from_utf8_lossy(&status.stderr).trim()));
    }
    Ok(file_size(patch) > 0)
}

fn git_command(root: &Path) -> Command {
    let mut command = Command::new("git");
    command.arg("-C").arg(root).env("GIT_TERMINAL_PROMPT", "0").env("GIT_OPTIONAL_LOCKS", "0").stdin(Stdio::null());
    command
}

fn git(root: &Path, args: &[&std::ffi::OsStr]) -> Result<(), String> {
    let output = git_command(root).args(args).output().map_err(|error| format!("git: {error}"))?;
    if output.status.success() {
        Ok(())
    } else {
        Err(format!("git in {}: {}", root.display(), String::from_utf8_lossy(&output.stderr).trim()))
    }
}

fn git_out(root: &Path, args: &[&str]) -> Option<String> {
    let output = git_command(root).args(args).stderr(Stdio::null()).output().ok()?;
    output.status.success().then(|| String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

fn git_works() -> bool {
    Command::new("git").arg("--version").stdout(Stdio::null()).stderr(Stdio::null()).status().is_ok_and(|status| status.success())
}

fn free_space(dir: &Path) -> Option<u64> {
    use std::os::unix::ffi::OsStrExt;
    let path = std::ffi::CString::new(dir.as_os_str().as_bytes()).ok()?;
    // SAFETY: statvfs writes into the struct it's given, for a valid C string.
    let mut stat: libc::statvfs = unsafe { std::mem::zeroed() };
    if unsafe { libc::statvfs(path.as_ptr(), &mut stat) } != 0 {
        return None;
    }
    #[allow(clippy::unnecessary_cast)]
    Some(stat.f_bavail as u64 * stat.f_frsize as u64)
}

fn available_memory() -> Option<u64> {
    let mut system = sysinfo::System::new().ok()?;
    system.refresh_memory();
    let available = system.available_memory();
    (available > 0).then_some(available)
}

/// Whether this process runs under this Claktory's backend (an agent's
/// command), which `--final` stops.
fn inside_this_claktory(home: &Home) -> bool {
    use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, UpdateKind};
    let backend = home.backend().canonicalize().unwrap_or_else(|_| home.backend());
    let Ok(mut system) = sysinfo::System::new() else { return false };
    let mut pid = Pid::from_u32(std::process::id());
    for _ in 0..64 {
        system.refresh_processes_specifics(
            ProcessesToUpdate::Some(&[pid]),
            true,
            ProcessRefreshKind::nothing().with_exe(UpdateKind::Always),
        );
        let Some(process) = system.process(pid) else { return false };
        if process.exe().is_some_and(|exe| exe.canonicalize().unwrap_or_else(|_| exe.to_path_buf()) == backend) {
            return true;
        }
        match process.parent() {
            Some(parent) if parent != pid && parent.as_u32() > 1 => pid = parent,
            _ => return false,
        }
    }
    false
}

/// Stops Claktory and turns its service off, until `start`.
pub(crate) fn stop(place: &Place) -> Result<(), Refused> {
    let stopped = match &place.service {
        Service::Launchd { launchctl, domain, label, .. } => {
            let service = format!("{domain}/{label}");
            let run = |args: &[&str]| Command::new(launchctl).args(args).output();
            // Off first, so nothing starts it again meanwhile.
            match run(&["disable", &service]) {
                Ok(output) if output.status.success() => {}
                Ok(output) => {
                    return Err(refuse(format!(
                        "macOS didn't turn Claktory off ({}), so nothing was changed.",
                        String::from_utf8_lossy(&output.stderr).trim()
                    )));
                }
                Err(error) => return Err(refuse(format!("launchctl can't run ({error}), so nothing was changed."))),
            }
            let _ = run(&["bootout", &service]);
            let deadline = Instant::now() + STOP_WITHIN;
            let mut stopped = true;
            while run(&["print", &service]).is_ok_and(|output| output.status.success()) {
                if Instant::now() >= deadline {
                    stopped = false;
                    break;
                }
                std::thread::sleep(Duration::from_millis(100));
            }
            stopped
        }
        Service::Systemd { systemctl, unit } => {
            Command::new(systemctl).args(["--user", "disable", "--now", unit]).output().is_ok_and(|output| output.status.success())
        }
        Service::None => true,
    };
    let deadline = Instant::now() + STOP_WITHIN;
    let mut quiet = stopped;
    while quiet && crate::install::answers(place.port, Duration::from_millis(200)) {
        if Instant::now() >= deadline {
            quiet = false;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    if quiet {
        return Ok(());
    }
    let started = start(place);
    Err(refuse(match started {
        Ok(()) => "Claktory didn't stop in time, so nothing was changed, and it's running again.".to_owned(),
        Err(problem) => {
            format!("Claktory didn't stop in time, and didn't start again by itself ({problem}): start it from the Claktory app.")
        }
    }))
}

/// Turns the service on again and starts it.
pub(crate) fn start(place: &Place) -> Result<(), String> {
    match &place.service {
        Service::Launchd { launchctl, domain, label, plist } => {
            let service = format!("{domain}/{label}");
            let enabled = Command::new(launchctl).args(["enable", &service]).output().map_err(|error| error.to_string())?;
            if !enabled.status.success() {
                return Err(String::from_utf8_lossy(&enabled.stderr).trim().to_owned());
            }
            let loaded = Command::new(launchctl).arg("bootstrap").arg(domain).arg(plist).output().map_err(|error| error.to_string())?;
            // Already loaded counts: it runs.
            let running = Command::new(launchctl).args(["print", &service]).output().is_ok_and(|output| output.status.success());
            if loaded.status.success() || running { Ok(()) } else { Err(String::from_utf8_lossy(&loaded.stderr).trim().to_owned()) }
        }
        Service::Systemd { systemctl, unit } => {
            let output = Command::new(systemctl).args(["--user", "enable", "--now", unit]).output().map_err(|error| error.to_string())?;
            if output.status.success() { Ok(()) } else { Err(String::from_utf8_lossy(&output.stderr).trim().to_owned()) }
        }
        Service::None => Ok(()),
    }
}

/// What `<home>/moved.json` says.
#[derive(Clone, Debug, Default, serde::Deserialize, Serialize)]
#[serde(default)]
pub struct Moved {
    pub format: u32,
    pub export_id: String,
    pub at_ms: i64,
    pub epoch: u64,
    pub machine: String,
    /// Where it moved: the target's computer name, and its system; empty
    /// after a bundle.
    pub to: String,
    pub to_os: String,
    pub version: String,
    pub bundle: String,
    /// When this copy was deleted (`--delete-moved`); 0 while it's kept.
    #[serde(skip_serializing_if = "is_zero")]
    pub deleted_ms: i64,
}

fn is_zero(value: &i64) -> bool {
    *value == 0
}

/// The tombstone, if this copy moved. One that can't be read still counts,
/// as moved mode reads it (`move_import::moved`).
pub fn read_moved(home: &Home) -> Option<Moved> {
    let text = match std::fs::read_to_string(home.root().join("moved.json")) {
        Ok(text) => text,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return None,
        Err(_) => return Some(Moved::default()),
    };
    Some(serde_json::from_str(&text).unwrap_or_default())
}

fn write_moved(home: &Home, moved: &Moved) -> io::Result<()> {
    let path = home.root().join("moved.json");
    let new = home.root().join("moved.json.new");
    let mut file = File::create(&new)?;
    file.write_all(&serde_json::to_vec_pretty(moved).map_err(io::Error::other)?)?;
    file.sync_all()?;
    std::fs::rename(&new, &path)
}

/// Fences this copy, then writes `moved` as `moved.json`, stamped now.
/// Undoes what it did when it fails.
pub(crate) fn tombstone(place: &Place, moved: Moved) -> Result<(), Refused> {
    let home = place.home.root();
    let fence = home.join("moved");
    let mut done: Vec<&str> = Vec::new();
    let undo = |done: &[&str]| {
        for name in done {
            let _ = std::fs::rename(fence.join(name), home.join(name));
        }
        let _ = std::fs::remove_dir(&fence);
    };
    if let Err(error) = std::fs::create_dir_all(&fence) {
        return Err(refuse(format!("{}: {error}. Nothing was changed.", fence.display())));
    }
    for name in FENCED {
        if !home.join(name).exists() {
            continue;
        }
        if let Err(error) = std::fs::rename(home.join(name), fence.join(name)) {
            undo(&done);
            return Err(refuse(format!("{} couldn't be put away ({error}), so nothing was changed.", home.join(name).display())));
        }
        done.push(name);
    }
    let moved = Moved { format: 1, at_ms: crate::transcript::now_ms(), version: crate::update::VERSION.to_owned(), deleted_ms: 0, ..moved };
    if let Err(error) = write_moved(&place.home, &moved) {
        undo(&done);
        return Err(refuse(format!("moved.json couldn't be written ({error}), so nothing was changed.")));
    }
    Ok(())
}

/// `--restore`: this copy comes back as it was before `--final`.
pub fn restore(place: &Place) -> Result<String, Refused> {
    let home = place.home.root();
    let Some(moved) = read_moved(&place.home) else {
        return Err(refuse("This Claktory hasn't moved, so there's nothing to bring back."));
    };
    if moved.deleted_ms != 0 {
        return Err(refuse(format!(
            "This copy was deleted ({}), so it can't come back. Move the other one back here instead.",
            when(moved.deleted_ms)
        )));
    }
    let fence = home.join("moved");
    // Anything made here since (a backend that started without them) is kept aside.
    let aside = fence.join(format!("replaced-{}", crate::transcript::now_ms()));
    for name in FENCED {
        if !fence.join(name).exists() {
            continue;
        }
        if home.join(name).exists() {
            std::fs::create_dir_all(&aside).map_err(|error| refuse(format!("{}: {error}", aside.display())))?;
            std::fs::rename(home.join(name), aside.join(name))
                .map_err(|error| refuse(format!("{} couldn't be put aside: {error}", home.join(name).display())))?;
        }
        std::fs::rename(fence.join(name), home.join(name))
            .map_err(|error| refuse(format!("{} couldn't be brought back: {error}", fence.join(name).display())))?;
    }
    let _ = std::fs::remove_dir(&fence);
    std::fs::rename(home.join("moved.json"), home.join(format!("moved-{}.json", crate::transcript::now_ms())))
        .map_err(|error| refuse(format!("moved.json couldn't be put away: {error}")))?;
    // A backend in moved mode holds the port: stopped, it gives way to the
    // Claktory that's back (its supervisor starts that one by itself).
    if crate::move_import::moved::stop_held(&place.home) {
        let deadline = Instant::now() + STOP_WITHIN;
        while crate::move_import::moved::held(&place.home) && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(100));
        }
    }
    match start(place) {
        Ok(()) if matches!(place.service, Service::None) => Ok("This Claktory is back as it was. Start it again to carry on.".to_owned()),
        Ok(()) => Ok("This Claktory is back as it was, and it's starting.".to_owned()),
        Err(problem) => {
            Ok(format!("This Claktory is back as it was, but it didn't start by itself ({problem}): start it from the Claktory app."))
        }
    }
}

/// What `--delete-moved` deletes: everything of the old copy but the
/// person's work (the workspace and project folders inside the home), the
/// program, and `moved.json`, which keeps saying it moved.
pub fn deleted_by(place: &Place) -> Vec<PathBuf> {
    let home = place.home.root();
    let config: serde_json::Value =
        std::fs::read_to_string(place.home.config_path()).ok().and_then(|text| serde_json::from_str(&text).ok()).unwrap_or_default();
    let mut work: Vec<PathBuf> = vec![place.home.workspace()];
    for project in config["projects"].as_array().into_iter().flatten() {
        let paths =
            std::iter::once(&project["path"]).chain(project["folders"].as_array().into_iter().flatten().map(|folder| &folder["path"]));
        work.extend(paths.filter_map(serde_json::Value::as_str).map(PathBuf::from).filter(|path| path.starts_with(home)));
    }
    let Ok(read) = std::fs::read_dir(home) else { return Vec::new() };
    let mut gone: Vec<PathBuf> = read
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            let name = path.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
            !matches!(name.as_str(), "bin" | "releases" | "moved.json") && !work.iter().any(|kept| kept.starts_with(path))
        })
        .collect();
    gone.sort();
    gone
}

/// `--delete-moved`: deletes the old copy for good.
pub fn delete_moved(place: &Place) -> Result<String, Refused> {
    let Some(mut moved) = read_moved(&place.home) else {
        return Err(refuse(
            "This Claktory hasn't moved, so there's no old copy to delete. To remove Claktory, use Remove Claktory in the app.",
        ));
    };
    let mut failed = Vec::new();
    for path in deleted_by(place) {
        let removed = if std::fs::symlink_metadata(&path).is_ok_and(|meta| meta.is_dir()) {
            std::fs::remove_dir_all(&path)
        } else {
            std::fs::remove_file(&path)
        };
        if let Err(error) = removed {
            failed.push(format!("{}: {error}", path.display()));
        }
    }
    moved.deleted_ms = crate::transcript::now_ms();
    let _ = write_moved(&place.home, &moved);
    if failed.is_empty() {
        Ok("The old copy of this Claktory was deleted. Your workspace is kept.".to_owned())
    } else {
        Err(refuse(format!("Some of the old copy couldn't be deleted: {}", failed.join("; "))))
    }
}

/// A time as the person reads it.
fn when(ms: i64) -> String {
    jiff::Timestamp::from_millisecond(ms)
        .map(|at| at.to_zoned_datetime(jiff::tz::TimeZone::system()).strftime("%B %-d at %H:%M").to_string())
        .unwrap_or_default()
}

/// "812 MB", "3.4 GB".
pub(crate) fn size_text(bytes: u64) -> String {
    const GB: f64 = 1_000_000_000.0;
    let bytes = bytes as f64;
    if bytes >= GB { format!("{:.1} GB", bytes / GB) } else { format!("{:.0} MB", (bytes / 1_000_000.0).max(1.0)) }
}

const USAGE: &str = "usage: factoryd export [--final] [--out FILE] [--skip FOLDER]… [--json]
       factoryd export --restore [--confirm restore]
       factoryd export --delete-moved [--confirm delete]";

/// Asks the person to type `word`, unless `--confirm word` said it.
fn confirmed(word: &str, given: Option<&str>, question: &str) -> bool {
    if let Some(given) = given {
        return given == word;
    }
    if !io::stdin().is_terminal() {
        return false;
    }
    print!("{question}\nType {word} to go ahead: ");
    let _ = io::stdout().flush();
    let mut line = String::new();
    io::stdin().read_line(&mut line).is_ok() && line.trim() == word
}

/// `factoryd [--dev] export …`.
pub fn main(dev: bool, args: &[String]) -> ExitCode {
    let mut options = Options::default();
    let (mut json, mut restoring, mut deleting) = (false, false, false);
    let mut confirm: Option<String> = None;
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--final" => options.is_final = true,
            "--json" => json = true,
            "--restore" => restoring = true,
            "--delete-moved" => deleting = true,
            "--out" => match args.next() {
                Some(out) => options.out = Some(PathBuf::from(out)),
                None => return usage(),
            },
            "--skip" => match args.next() {
                Some(folder) => options.skip.push(PathBuf::from(folder)),
                None => return usage(),
            },
            "--confirm" => match args.next() {
                Some(word) => confirm = Some(word.clone()),
                None => return usage(),
            },
            _ => return usage(),
        }
    }
    if u8::from(restoring) + u8::from(deleting) + u8::from(options.is_final) > 1 {
        return usage();
    }
    let Some(place) = Place::this_user(dev) else {
        eprintln!("HOME isn't set.");
        return ExitCode::from(2);
    };
    let say = |ok: bool, text: &str, extra: serde_json::Value| {
        if json {
            let mut line = serde_json::json!({"ok": ok, "message": text});
            if let (Some(line), Some(extra)) = (line.as_object_mut(), extra.as_object()) {
                line.extend(extra.clone());
            }
            println!("{line}");
        } else if ok {
            println!("{text}");
        } else {
            eprintln!("{text}");
        }
        ExitCode::from(if ok { 0 } else { 3 })
    };
    if restoring {
        let question = "This brings this Claktory back here, as it was before it moved. Only do it if the moved Claktory never ran, or is stopped for good: two copies running would both use your sign-ins and devices.";
        if !confirmed("restore", confirm.as_deref(), question) {
            return say(
                false,
                "Nothing was changed. To bring this Claktory back, run factoryd export --restore and type restore, or add --confirm restore.",
                serde_json::json!({}),
            );
        }
        return match restore(&place) {
            Ok(text) => say(true, &text, serde_json::json!({})),
            Err(refused) => say(false, &refused.reason, serde_json::json!({})),
        };
    }
    if deleting {
        if read_moved(&place.home).is_none() {
            return say(false, "This Claktory hasn't moved, so there's no old copy to delete.", serde_json::json!({}));
        }
        let gone = deleted_by(&place);
        let listed: Vec<String> = gone.iter().map(|path| format!("  {}", path.display())).collect();
        let question = format!(
            "This deletes the old copy of this Claktory for good: its conversations, records, settings, secrets and sign-ins:\n{}\nYour workspace stays. Do this only once the moved Claktory works.",
            listed.join("\n")
        );
        if !confirmed("delete", confirm.as_deref(), &question) {
            return say(
                false,
                "Nothing was deleted. To delete the old copy, run factoryd export --delete-moved and type delete, or add --confirm delete.",
                serde_json::json!({}),
            );
        }
        return match delete_moved(&place) {
            Ok(text) => say(true, &text, serde_json::json!({})),
            Err(refused) => say(false, &refused.reason, serde_json::json!({})),
        };
    }
    if !json {
        println!("{}", if options.is_final { "Stopping Claktory and writing the bundle…" } else { "Writing the bundle…" });
    }
    match export(&place, &options) {
        Ok(exported) => {
            let manifest = &exported.manifest;
            let mut text = format!(
                "Wrote {} ({}: {} files, {}).\n\nPassphrase: {}\n\nWrite it down: the bundle can't be opened without it, and it isn't kept anywhere.",
                exported.bundle.display(),
                size_text(exported.size),
                manifest.files,
                match manifest.folders.len() {
                    1 => "1 project folder".to_owned(),
                    n => format!("{n} project folders"),
                },
                exported.passphrase,
            );
            if !manifest.skipped.is_empty() {
                let skipped: Vec<String> = manifest.skipped.iter().map(|skipped| format!("  {} ({})", skipped.path, skipped.why)).collect();
                text.push_str(&format!("\n\nNot in the bundle:\n{}", skipped.join("\n")));
            }
            if !exported.warnings.is_empty() {
                text.push_str(&format!("\n\nLeft out:\n  {}", exported.warnings.join("\n  ")));
            }
            if options.is_final {
                text.push_str("\n\nThis Claktory is stopped here for good, with everything kept. On the new computer, import the bundle. If that fails, run factoryd export --restore to bring it back here. Once the moved Claktory works, delete this copy with factoryd export --delete-moved.");
            }
            let extra = serde_json::json!({
                "bundle": exported.bundle,
                "passphrase": exported.passphrase,
                "size": exported.size,
                "export_id": manifest.export_id,
                "final": manifest.is_final,
                "skipped": manifest.skipped,
                "warnings": exported.warnings,
            });
            say(true, &text, extra)
        }
        Err(refused) => say(false, &refused.reason, serde_json::json!({})),
    }
}

fn usage() -> ExitCode {
    eprintln!("{USAGE}");
    ExitCode::from(2)
}
