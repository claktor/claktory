//! `factoryd uninstall`: removes Claktory from a Mac, the reverse of
//! `factoryd install`, for "Remove Claktory from this Mac" in the app. The
//! app runs it as its own child (not the launchd job's), so it outlives the
//! backend it stops, and learns how it went from its one JSON line.
//!
//! It unloads the LaunchAgent (the supervisor stops the backend cleanly),
//! stops what the backend leaves running in process groups of their own
//! (agents' background tasks, monitors' watches), and removes the
//! LaunchAgent and `~/.factory/bin`. The person's data stays, unless
//! `--delete-data`: then everything else in `~/.factory` goes too, except
//! the workspace and any project folder inside it, which are their work.
//! Nothing outside `~/.factory` is touched, nor the dev Claktory.
//! `--dry-run` only says what it would do.

use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};
use std::time::{Duration, Instant};

use serde::Serialize;

use crate::install::Setup;

/// How long counting sizes may take, in all (`--dry-run`).
const COUNT_WITHIN: Duration = Duration::from_secs(5);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Outcome {
    /// Done; with `--delete-data`, the data too (or it's being deleted).
    Removed,
    /// Nothing changed; `reason` says why.
    Failed,
    /// No LaunchAgent and no backend: nothing to remove.
    NotInstalled,
    /// It stopped Claktory, but something couldn't be removed.
    PartlyRemoved,
    /// (`--dry-run`) What it would do.
    DryRun,
}

impl Outcome {
    fn exit_code(self) -> u8 {
        match self {
            Outcome::Removed => 0,
            Outcome::Failed => 1,
            Outcome::NotInstalled => 3,
            Outcome::PartlyRemoved => 4,
            Outcome::DryRun => 5,
        }
    }
}

/// One thing it removes, for `--dry-run`.
#[derive(Debug, Serialize, PartialEq)]
pub(crate) struct Item {
    /// "conversations", "settings", "records", "secrets", "sign_ins",
    /// "devices", "releases", "checkouts", "trash", "log", "backend" or
    /// "other".
    pub(crate) what: &'static str,
    pub(crate) path: String,
    pub(crate) bytes: u64,
    /// The count finished; otherwise `bytes` is at least that.
    pub(crate) complete: bool,
}

/// The JSON line `factoryd uninstall` answers with.
#[derive(Debug, Serialize)]
pub(crate) struct Report {
    pub(crate) result: Outcome,
    /// A sentence the app shows as it is.
    pub(crate) reason: String,
    /// Technical detail; empty when there's none.
    pub(crate) detail: String,
    pub(crate) version: &'static str,
    /// `~/.factory`.
    pub(crate) data_path: String,
    /// What it removes (or removed).
    pub(crate) removes: Vec<String>,
    /// What it keeps in `~/.factory`.
    pub(crate) keeps: Vec<String>,
    /// (`--dry-run`) Each thing it would remove, with its size.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(crate) items: Vec<Item>,
}

/// What `delete_data` decides it removes and keeps in the data folder.
struct Plan {
    /// The LaunchAgent's plist, when it's this Claktory's.
    plist: Option<PathBuf>,
    /// `~/.factory/bin`, when it's there.
    bin: Option<PathBuf>,
    /// The rest of the data folder that goes, with `--delete-data`.
    data: Vec<PathBuf>,
    /// What stays in the data folder.
    keeps: Vec<PathBuf>,
}

impl Plan {
    fn removes(&self) -> Vec<PathBuf> {
        self.plist.iter().chain(&self.bin).chain(&self.data).cloned().collect()
    }
}

fn plan(setup: &Setup, delete_data: bool) -> Plan {
    let root = setup.home.root();
    let ours = setup.plist.is_file() && crate::launch_agent::read(&setup.plist).is_ok_and(|plist| plist["Label"] == setup.label.as_str());
    let bin = root.join("bin");
    let mut plan = Plan { plist: ours.then(|| setup.plist.clone()), bin: bin.exists().then_some(bin), data: Vec::new(), keeps: Vec::new() };
    let entries: Vec<PathBuf> =
        std::fs::read_dir(root).map(|entries| entries.filter_map(Result::ok).map(|entry| entry.path()).collect()).unwrap_or_default();
    let kept = kept_folders(root);
    for entry in entries {
        let name = entry.file_name().and_then(|name| name.to_str()).unwrap_or_default();
        if name == "bin" {
            continue;
        }
        let keep = !delete_data || name == "workspace" || kept.iter().any(|folder| folder.starts_with(&entry));
        if keep {
            plan.keeps.push(entry);
        } else {
            plan.data.push(entry);
        }
    }
    plan.data.sort();
    plan.keeps.sort();
    plan
}

/// The projects' folders (and their folders') that lie in the data folder,
/// from config.json, read leniently: a broken file keeps only the workspace.
fn kept_folders(root: &Path) -> Vec<PathBuf> {
    let Ok(text) = std::fs::read_to_string(root.join("config.json")) else { return Vec::new() };
    let Ok(config) = serde_json::from_str::<serde_json::Value>(&text) else { return Vec::new() };
    let mut folders = Vec::new();
    for project in config["projects"].as_array().into_iter().flatten() {
        let paths =
            std::iter::once(&project["path"]).chain(project["folders"].as_array().into_iter().flatten().map(|folder| &folder["path"]));
        for path in paths.filter_map(serde_json::Value::as_str) {
            let path = PathBuf::from(path);
            if path.starts_with(root) {
                folders.push(path);
            }
        }
    }
    folders
}

/// What a path in the data folder holds, as the app names it.
fn what(path: &Path) -> &'static str {
    match path.file_name().and_then(|name| name.to_str()).unwrap_or_default() {
        "agents" => "conversations",
        "config.json" | "config.broken.json" => "settings",
        name if name.starts_with("factory.db") || name == "state.json" || name == "runs" => "records",
        "vault" => "secrets",
        "auth.json" => "sign_ins",
        "remote" => "devices",
        "releases" => "releases",
        "trees" => "checkouts",
        "trash" => "trash",
        name if name.starts_with("factoryd.log") => "log",
        "bin" => "backend",
        _ => "other",
    }
}

/// How many bytes `path` holds, counting until `deadline`; and whether it
/// finished.
fn size(path: &Path, deadline: Instant) -> (u64, bool) {
    let Ok(metadata) = std::fs::symlink_metadata(path) else { return (0, true) };
    if !metadata.is_dir() {
        return (metadata.len(), true);
    }
    let (mut bytes, mut pending) = (0, vec![path.to_owned()]);
    while let Some(dir) = pending.pop() {
        if Instant::now() >= deadline {
            return (bytes, false);
        }
        let Ok(entries) = std::fs::read_dir(&dir) else { continue };
        for entry in entries.filter_map(Result::ok) {
            match entry.metadata() {
                Ok(metadata) if metadata.is_dir() => pending.push(entry.path()),
                Ok(metadata) => bytes += metadata.len(),
                Err(_) => {}
            }
        }
    }
    (bytes, true)
}

fn report(setup: &Setup, result: Outcome, reason: &str, detail: impl Into<String>, plan: &Plan) -> Report {
    let shown = |paths: Vec<PathBuf>| paths.iter().map(|path| path.display().to_string()).collect();
    Report {
        result,
        reason: reason.to_owned(),
        detail: detail.into(),
        version: crate::update::VERSION,
        data_path: setup.home.root().display().to_string(),
        removes: shown(plan.removes()),
        keeps: shown(plan.keeps.clone()),
        items: Vec::new(),
    }
}

/// What it would do, with sizes.
pub(crate) fn dry_run(setup: &Setup, delete_data: bool) -> Report {
    let plan = plan(setup, delete_data);
    let deadline = Instant::now() + COUNT_WITHIN;
    let mut report = report(setup, Outcome::DryRun, "Nothing was changed.", "", &plan);
    for path in plan.bin.iter().chain(&plan.data) {
        let (bytes, complete) = size(path, deadline);
        report.items.push(Item { what: what(path), path: path.display().to_string(), bytes, complete });
    }
    report
}

/// Stops Claktory and removes it, and with `delete_data` the person's data
/// but for their work.
pub(crate) fn uninstall(setup: &Setup, delete_data: bool) -> Report {
    let plan = plan(setup, delete_data);
    if plan.plist.is_none() && plan.bin.is_none() && plan.data.is_empty() {
        return report(setup, Outcome::NotInstalled, "Claktory isn't set up on this Mac.", "", &plan);
    }
    if let Err(detail) = stop(setup) {
        return report(setup, Outcome::Failed, "macOS didn't stop Claktory, so nothing was removed.", detail, &plan);
    }
    stop_leftovers(setup);
    let mut failed = Vec::new();
    if let Some(plist) = &plan.plist
        && let Err(error) = std::fs::remove_file(plist)
    {
        failed.push(format!("{}: {error}", plist.display()));
    }
    if let Some(bin) = &plan.bin
        && let Err(error) = std::fs::remove_dir_all(bin)
    {
        failed.push(format!("{}: {error}", bin.display()));
    }
    if !plan.data.is_empty()
        && let Err(error) = delete(setup.home.root(), &plan)
    {
        failed.push(error);
    }
    if failed.is_empty() {
        let reason = if delete_data {
            "Claktory and its data were removed from this Mac."
        } else {
            "Claktory was removed from this Mac. Your data is kept."
        };
        return report(setup, Outcome::Removed, reason, "", &plan);
    }
    report(setup, Outcome::PartlyRemoved, "Claktory was stopped, but some of it couldn't be removed.", failed.join("; "), &plan)
}

/// Unloads the job, if it's loaded, and waits until launchd has stopped it
/// and nothing answers on the backend's port, for `answer_within` in all.
fn stop(setup: &Setup) -> Result<(), String> {
    let service = format!("{}/{}", setup.domain, setup.label);
    let loaded = || Command::new(&setup.launchctl).args(["print", &service]).output().is_ok_and(|output| output.status.success());
    let deadline = Instant::now() + setup.answer_within;
    if loaded() {
        eprintln!("stopping Claktory");
        let output =
            Command::new(&setup.launchctl).args(["bootout", &service]).output().map_err(|error| format!("Can't run launchctl: {error}"))?;
        while loaded() {
            if Instant::now() >= deadline {
                return Err(format!("{service} is still loaded: {}", String::from_utf8_lossy(&output.stderr).trim()));
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }
    while crate::install::answers(setup.port, Duration::from_millis(200)) {
        if Instant::now() >= deadline {
            return Err(format!("something still answers on port {}", setup.port));
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    Ok(())
}

/// Stops what the backend started in process groups of their own, which
/// outlive it: agents' background tasks and monitors' watches that factory.db
/// says still run. Only those started since this boot, whose pids can't be
/// another's.
fn stop_leftovers(setup: &Setup) {
    use factory_agent::tools::bash::{group_alive, started_before_boot};
    let flags = rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX;
    let Ok(db) = rusqlite::Connection::open_with_flags(setup.home.db_path(), flags) else { return };
    let mut groups: Vec<(u32, i64)> = Vec::new();
    if let Ok(mut statement) = db.prepare("SELECT pid, started FROM tasks WHERE ended IS NULL")
        && let Ok(rows) = statement.query_map([], crate::store::row2::<i64, i64>)
    {
        groups.extend(rows.filter_map(Result::ok).filter_map(|(pid, started)| Some((u32::try_from(pid).ok()?, started))));
    }
    if let Ok(mut statement) = db.prepare("SELECT cursor FROM monitors")
        && let Ok(rows) = statement.query_map([], |row| row.get::<_, String>(0))
    {
        for cursor in rows.filter_map(Result::ok) {
            let Ok(cursor) = serde_json::from_str::<serde_json::Value>(&cursor) else { continue };
            if let (Some(pid), Some(started)) = (cursor["pid"].as_u64().and_then(|pid| u32::try_from(pid).ok()), cursor["started"].as_i64())
            {
                groups.push((pid, started));
            }
        }
    }
    let running: Vec<u32> = groups
        .into_iter()
        .filter(|(pid, started)| *pid > 1 && !started_before_boot((*started).max(0) as u64) && group_alive(*pid))
        .map(|(pid, _)| pid)
        .collect();
    if running.is_empty() {
        return;
    }
    eprintln!("stopping {} background tasks and watches", running.len());
    for signal in [libc::SIGTERM, libc::SIGKILL] {
        for pid in &running {
            // SAFETY: a plain syscall on a process group factory.db says Claktory started.
            unsafe {
                libc::kill(-(*pid as i32), signal);
            }
        }
        let deadline = Instant::now() + Duration::from_secs(2);
        while running.iter().any(|pid| group_alive(*pid)) && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(50));
        }
    }
}

/// Removes the data that goes: moved aside at once (a rename, on the same
/// volume), then deleted by a process of its own, since agents' checkouts
/// can take minutes and the app quits meanwhile.
fn delete(root: &Path, plan: &Plan) -> Result<(), String> {
    let stamp = crate::transcript::now_ms();
    // Nothing of theirs stays: the whole folder goes.
    let (aside, moves): (PathBuf, Vec<&PathBuf>) = if plan.keeps.is_empty() {
        let aside = root.with_file_name(format!(".factory-removed-{stamp}"));
        std::fs::rename(root, &aside).map_err(|error| format!("{}: {error}", root.display()))?;
        (aside, Vec::new())
    } else {
        let aside = root.join(format!(".removing-{stamp}"));
        std::fs::create_dir_all(&aside).map_err(|error| format!("{}: {error}", aside.display()))?;
        (aside, plan.data.iter().collect())
    };
    let mut failed = Vec::new();
    for path in moves {
        let Some(name) = path.file_name() else { continue };
        if let Err(error) = std::fs::rename(path, aside.join(name)) {
            failed.push(format!("{}: {error}", path.display()));
        }
    }
    use std::os::unix::process::CommandExt;
    let mut remove = Command::new("/bin/rm");
    remove.arg("-rf").arg(&aside).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    // SAFETY: setsid is async-signal-safe and touches nothing else.
    unsafe {
        remove.pre_exec(|| if libc::setsid() < 0 { Err(std::io::Error::last_os_error()) } else { Ok(()) });
    }
    if let Err(error) = remove.spawn() {
        failed.push(format!("{}: {error}", aside.display()));
    }
    if failed.is_empty() { Ok(()) } else { Err(failed.join("; ")) }
}

/// `factoryd uninstall [--delete-data] [--dry-run]`.
pub fn main(args: &[String]) -> ExitCode {
    let (mut delete_data, mut dry) = (false, false);
    for arg in args {
        match arg.as_str() {
            "--delete-data" => delete_data = true,
            "--dry-run" => dry = true,
            "--dev" => {
                eprintln!("The dev Claktory isn't removed this way.");
                return ExitCode::from(2);
            }
            _ => {
                eprintln!("usage: factoryd uninstall [--delete-data] [--dry-run]");
                return ExitCode::from(2);
            }
        }
    }
    if !cfg!(target_os = "macos") {
        eprintln!("factoryd uninstall removes Claktory from a Mac.");
        return ExitCode::from(2);
    }
    let Some(setup) = crate::install::this_mac() else {
        eprintln!("HOME isn't set.");
        return ExitCode::from(2);
    };
    let report = if dry { dry_run(&setup, delete_data) } else { uninstall(&setup, delete_data) };
    println!("{}", serde_json::to_string(&report).unwrap_or_default());
    ExitCode::from(report.result.exit_code())
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;
    use crate::home::Home;
    use std::os::unix::fs::PermissionsExt;

    fn temp() -> PathBuf {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let next = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("factoryd-uninstall-{}-{next}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("dir");
        dir
    }

    fn write(path: &Path, text: &str) {
        std::fs::create_dir_all(path.parent().expect("parent")).expect("dir");
        std::fs::write(path, text).expect("write");
    }

    /// A launchctl that logs its calls; the job counts as loaded until it's
    /// booted out (a file says so).
    fn fake_launchctl(dir: &Path, stays: bool) -> PathBuf {
        let path = dir.join("launchctl");
        let loaded = dir.join("loaded");
        write(&loaded, "");
        let bootout = if stays { "exit 5".to_owned() } else { format!("rm -f \"{}\"", loaded.display()) };
        let script = format!(
            "#!/bin/sh\necho \"$@\" >> \"{calls}\"\ncase \"$1\" in\n  print) [ -e \"{loaded}\" ] ;;\n  bootout) {bootout} ;;\nesac\n",
            calls = dir.join("calls").display(),
            loaded = loaded.display(),
        );
        write(&path, &script);
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).expect("chmod");
        path
    }

    /// An installed Claktory with data and a project folder in the workspace.
    fn installed(dir: &Path) -> Setup {
        let home = Home::new(dir.join("home"));
        let root = home.root().to_owned();
        let plist = dir.join("LaunchAgents/dev.factory.test.plist");
        let job = serde_json::json!({"Label": "dev.factory.test", "ProgramArguments": ["supervisor", "supervise"]});
        write(&plist, &job.to_string());
        write(&root.join("bin/factoryd"), "backend");
        write(&root.join("bin/Claktory Agents.app/Contents/MacOS/factoryd-supervisor"), "supervisor");
        write(&root.join("agents/builder/transcript.jsonl"), "a conversation");
        write(&root.join("vault/vault.db"), "secrets");
        write(&root.join("factory.db"), "");
        write(&root.join("trees/builder/repo/file"), "a checkout");
        write(&root.join("workspace/site/index.html"), "the person's work");
        let elsewhere = dir.join("elsewhere");
        write(&elsewhere.join("app/main.rs"), "a project outside");
        let config = serde_json::json!({"projects": [
            {"id": "site", "name": "Site", "path": root.join("workspace/site")},
            {"id": "app", "name": "App", "path": elsewhere.join("app"), "folders": [{"id": "x", "name": "X", "path": root.join("projects/x")}]},
        ]});
        write(&root.join("config.json"), &config.to_string());
        write(&root.join("projects/x/notes.md"), "a project folder inside");
        Setup {
            home,
            helpers: PathBuf::new(),
            plist,
            label: "dev.factory.test".to_owned(),
            launchctl: fake_launchctl(dir, false),
            domain: "gui/501".to_owned(),
            codesign: "/usr/bin/true".into(),
            port: 0,
            answer_within: Duration::from_millis(100),
            path: None,
            env: Vec::new(),
        }
    }

    fn names(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(dir)
            .map(|entries| entries.filter_map(Result::ok).map(|entry| entry.file_name().to_string_lossy().into_owned()).collect())
            .unwrap_or_default();
        names.retain(|name| !name.starts_with(".removing-"));
        names.sort();
        names
    }

    #[test]
    fn removing_keeps_the_data_and_takes_the_job_and_the_backend() {
        let dir = temp();
        let setup = installed(&dir);
        let report = uninstall(&setup, false);
        assert_eq!(report.result, Outcome::Removed, "{report:?}");
        assert_eq!(report.reason, "Claktory was removed from this Mac. Your data is kept.");
        assert!(!setup.plist.exists() && !setup.home.root().join("bin").exists());
        assert_eq!(names(setup.home.root()), ["agents", "config.json", "factory.db", "projects", "trees", "vault", "workspace"]);
        assert_eq!(
            std::fs::read_to_string(dir.join("calls")).expect("calls"),
            "print gui/501/dev.factory.test\nbootout gui/501/dev.factory.test\nprint gui/501/dev.factory.test\n"
        );
        let line = serde_json::to_value(&report).expect("json");
        assert_eq!(
            line["removes"],
            serde_json::json!([setup.plist.display().to_string(), setup.home.root().join("bin").display().to_string()])
        );
        assert!(line.get("items").is_none());
        // Done again: nothing left to remove.
        assert_eq!(uninstall(&setup, false).result, Outcome::NotInstalled);
    }

    #[test]
    fn deleting_the_data_keeps_the_persons_work() {
        let dir = temp();
        let setup = installed(&dir);
        let dry = dry_run(&setup, true);
        assert_eq!(dry.result, Outcome::DryRun);
        let items: Vec<(&str, u64, bool)> = dry.items.iter().map(|item| (item.what, item.bytes, item.complete)).collect();
        assert_eq!(
            items,
            [
                ("backend", 17, true),
                ("conversations", 14, true),
                ("settings", items[2].1, true),
                ("records", 0, true),
                ("checkouts", 10, true),
                ("secrets", 7, true)
            ]
        );
        assert!(setup.plist.exists() && setup.home.root().join("vault").exists(), "a dry run changes nothing");

        let report = uninstall(&setup, true);
        assert_eq!(
            (report.result, report.reason.as_str()),
            (Outcome::Removed, "Claktory and its data were removed from this Mac."),
            "{report:?}"
        );
        assert_eq!(names(setup.home.root()), ["projects", "workspace"]);
        assert_eq!(std::fs::read_to_string(setup.home.root().join("workspace/site/index.html")).expect("kept"), "the person's work");
        assert!(dir.join("elsewhere/app/main.rs").exists(), "nothing outside is touched");
        // The rest goes in the background.
        for _ in 0..100 {
            let left = std::fs::read_dir(setup.home.root())
                .expect("dir")
                .filter_map(Result::ok)
                .any(|entry| entry.file_name().to_string_lossy().starts_with(".removing-"));
            if !left {
                return;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        panic!("the data set aside was never deleted");
    }

    #[test]
    fn with_nothing_of_theirs_inside_the_whole_folder_goes() {
        let dir = temp();
        let setup = installed(&dir);
        std::fs::remove_dir_all(setup.home.root().join("workspace")).expect("rm");
        std::fs::remove_dir_all(setup.home.root().join("projects")).expect("rm");
        std::fs::write(setup.home.root().join("config.json"), "{").expect("broken");
        let report = uninstall(&setup, true);
        assert_eq!(report.result, Outcome::Removed, "{report:?}");
        assert!(!setup.home.root().exists());
    }

    #[test]
    fn a_job_launchd_wont_stop_changes_nothing() {
        let dir = temp();
        let mut setup = installed(&dir);
        setup.launchctl = fake_launchctl(&dir, true);
        let started = Instant::now();
        let report = uninstall(&setup, true);
        assert_eq!(report.result, Outcome::Failed);
        assert_eq!(report.reason, "macOS didn't stop Claktory, so nothing was removed.");
        assert!(started.elapsed() >= setup.answer_within);
        assert!(setup.plist.exists() && setup.home.root().join("bin/factoryd").exists() && setup.home.root().join("vault").exists());
    }

    /// Installed and removed again with launchd itself, under a label, home
    /// and port of its own. Run by hand, with FACTORY_TEST_BACKEND the path
    /// of a built factoryd:
    /// `FACTORY_TEST_BACKEND=$PWD/target/debug/factoryd cargo test -p factoryd --lib uninstall::tests::with_launchd -- --ignored`
    #[test]
    #[ignore]
    fn with_launchd() {
        let Some(built) = std::env::var_os("FACTORY_TEST_BACKEND") else { return };
        let dir = temp();
        let helpers = dir.join("Claktory.app/Contents/Helpers");
        let program = helpers.join("Claktory Agents.app/Contents/MacOS/factoryd-supervisor");
        std::fs::create_dir_all(program.parent().expect("dir")).expect("dir");
        std::fs::copy(&built, helpers.join("factoryd")).expect("backend");
        std::fs::copy(&built, &program).expect("supervisor");
        let label = format!("dev.factory.uninstalltest-{}", std::process::id());
        let home = Home::new(dir.join("home"));
        let port = 7433;
        // SAFETY: getuid has no preconditions.
        let domain = format!("gui/{}", unsafe { libc::getuid() });
        let setup = Setup {
            home: home.clone(),
            helpers,
            plist: dir.join(format!("LaunchAgents/{label}.plist")),
            label: label.clone(),
            launchctl: "/bin/launchctl".into(),
            domain: domain.clone(),
            codesign: "/usr/bin/true".into(),
            port,
            answer_within: Duration::from_secs(60),
            path: None,
            env: vec![("FACTORY_HOME".into(), home.root().to_string_lossy().into_owned()), ("FACTORY_PORT".into(), port.to_string())],
        };
        let installed = crate::install::install(&setup, false);
        let removed = uninstall(&setup, false);
        let loaded =
            Command::new("/bin/launchctl").args(["print", &format!("{domain}/{label}")]).output().expect("launchctl").status.success();
        if loaded {
            let _ = Command::new("/bin/launchctl").args(["bootout", &format!("{domain}/{label}")]).status();
        }
        assert_eq!(installed.result, crate::install::Outcome::Installed, "{installed:?}");
        assert_eq!(removed.result, Outcome::Removed, "{removed:?}");
        assert!(!loaded, "the job is gone");
        assert!(!crate::install::answers(port, Duration::from_millis(300)), "the backend stopped");
        assert!(!setup.plist.exists() && !home.root().join("bin").exists());
        assert!(home.root().join("config.json").exists(), "the data is kept");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn background_tasks_and_watches_started_since_boot_are_stopped() {
        let dir = temp();
        let setup = installed(&dir);
        std::fs::remove_file(setup.home.db_path()).expect("rm");
        let db = rusqlite::Connection::open(setup.home.db_path()).expect("db");
        db.execute_batch("CREATE TABLE tasks (pid INTEGER, started INTEGER, ended INTEGER); CREATE TABLE monitors (cursor TEXT);")
            .expect("tables");
        use std::os::unix::process::CommandExt;
        let spawn = || Command::new("/bin/sleep").arg("60").process_group(0).spawn().expect("sleep");
        let (mut task, mut watch, mut old) = (spawn(), spawn(), spawn());
        let now = crate::transcript::now_ms();
        db.execute("INSERT INTO tasks VALUES (?1, ?2, NULL)", (task.id(), now)).expect("task");
        // From before the computer started: its pid could be anyone's now.
        db.execute("INSERT INTO tasks VALUES (?1, 1, NULL)", (old.id(),)).expect("old");
        db.execute("INSERT INTO monitors VALUES (?1)", (serde_json::json!({"pid": watch.id(), "started": now}).to_string(),))
            .expect("watch");
        drop(db);
        assert_eq!(uninstall(&setup, false).result, Outcome::Removed);
        assert!(task.try_wait().expect("wait").is_some() && watch.try_wait().expect("wait").is_some(), "stopped");
        assert!(old.try_wait().expect("wait").is_none(), "left alone");
        let _ = old.kill();
    }
}
