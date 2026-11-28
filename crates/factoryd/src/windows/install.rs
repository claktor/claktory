//! `factoryd install` and `factoryd uninstall` on Windows: what they are on
//! a Mac (`crate::install`, `crate::uninstall`), with the sign-in task
//! (`sign_in`) for the LaunchAgent. The installer runs the factoryd it lays
//! down beside the app (`Programs\Claktory\backend`), and the app runs the
//! home's own once it's set up (`<home>\bin\factoryd.exe`), else that one.
//!
//! `install` copies the backend and what comes with it (the helpers, the
//! agents' tools, Git, Claktory's own apps) into the home, the backend a
//! second time as the supervisor's copy (`supervisor_copy`), writes the
//! sign-in task and runs it, then waits until the backend answers. As on
//! a Mac it never replaces what's there: a Claktory already set up is left
//! alone, and `--repair` fills in what's missing and writes the task again,
//! still never replacing a backend that's there.
//!
//! Whether Claktory starts when the person signs in to Windows is theirs to
//! choose (`--consent=yes|no`: the installer's page, and Settings →
//! General); without one, it does. The choice is kept in the home
//! (`windows\sign-in.json`) and is the task's Enabled, except while the
//! person's switch holds Claktory off (`service::held`), when the task is
//! disabled whatever they chose. Every answer says it
//! (`starts_at_sign_in`). `--start` runs the task whatever it's set to,
//! leaving it so, and starts a Claktory the person stopped.
//!
//! `uninstall` stops the supervisor (its stop event, `stop::ask_to_stop`),
//! which stops the backend, deletes the task and removes `bin` and `tools`;
//! the person's data stays. Removing the data with it (`--delete-data`),
//! and saying first what it would remove (`--dry-run`), aren't on Windows
//! yet.
//!
//! Each is the real Claktory of this user (`--dev`, the dev one), in
//! LocalAppData (`dirs::factory_home`), whatever `FACTORY_HOME` says: the
//! task starts the supervisor without the environment this runs with.

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::{Duration, Instant};

use super::sign_in::{Registered, Task, running};
use crate::home::{Home, program};
use crate::install::{Outcome, Report, answers};
use crate::uninstall::{Outcome as Removal, Report as RemovalReport};

/// How long the backend has to answer once its task runs.
const ANSWER_WITHIN: Duration = Duration::from_secs(30);
/// How long the supervisor has to stop, its backend first. It gives the
/// backend 10 s before it ends it.
const STOP_WITHIN: Duration = Duration::from_secs(20);

/// Where everything goes: this user's Claktory, or a test's.
pub(crate) struct Setup {
    pub(crate) home: Home,
    /// What comes with this factoryd: the folder it's in (the installer's
    /// `backend`, or the home's `bin`).
    pub(crate) helpers: PathBuf,
    pub(crate) task: Task,
    pub(crate) port: u16,
    pub(crate) answer_within: Duration,
}

/// What's on this PC now.
struct Found {
    /// The task of this Claktory's name, as Task Scheduler has it.
    task: Option<Registered>,
    /// It runs this home's supervisor.
    ours: bool,
    backend: bool,
    supervisor: bool,
}

impl Found {
    fn installed(&self) -> bool {
        self.ours && self.backend && self.supervisor
    }

    fn nothing(&self) -> bool {
        self.task.is_none() && !self.backend && !self.supervisor
    }

    fn missing(&self) -> String {
        let mut missing = Vec::new();
        if !self.ours {
            missing.push(if self.task.is_some() {
                "the sign-in task doesn't run Claktory's supervisor"
            } else {
                "the sign-in task is missing"
            });
        }
        if !self.backend {
            missing.push("the backend is missing");
        }
        if !self.supervisor {
            missing.push("the supervisor is missing");
        }
        missing.join("; ")
    }
}

fn examine(setup: &Setup) -> Found {
    let task = setup.task.query();
    let ours = task.as_ref().is_some_and(|task| task.runs.as_ref() == Some(&setup.task.action));
    let present = |path: &Path| std::fs::metadata(path).is_ok_and(|file| file.is_file() && file.len() > 0);
    Found { task, ours, backend: present(&setup.home.backend()), supervisor: present(&supervisor(&setup.home)) }
}

fn supervisor(home: &Home) -> PathBuf {
    super::supervisor_copy::installed(home)
}

/// The person's choice, kept in the home.
fn choice_file(home: &Home) -> PathBuf {
    super::dirs::windows_records(home.root()).join("sign-in.json")
}

pub(crate) fn kept_choice(home: &Home) -> Option<bool> {
    let bytes = std::fs::read(choice_file(home)).ok()?;
    serde_json::from_slice::<serde_json::Value>(&bytes).ok()?["starts_at_sign_in"].as_bool()
}

/// Keeps the choice, making the home (private, as the backend makes it)
/// if it isn't there yet.
fn keep_choice(home: &Home, starts: bool) -> std::io::Result<()> {
    super::prepare_home(home.root())?;
    factory_agent::windows::private::create_dir_all(&super::dirs::windows_records(home.root()))?;
    let choice = serde_json::json!({"starts_at_sign_in": starts});
    factory_agent::windows::private::write(&choice_file(home), choice.to_string().as_bytes())
}

/// What the person chose: kept, else what the task does, else yes.
fn starts_at_sign_in(setup: &Setup) -> bool {
    super::service::chosen(&setup.home, &setup.task)
}

/// Whether the task should start Claktory at sign-in now: as chosen,
/// unless the person's switch holds it off.
fn enabled(setup: &Setup) -> bool {
    starts_at_sign_in(setup) && !crate::service::held(&setup.home)
}

/// Something of another user's answers on the port.
fn taken_by_another(port: u16) -> bool {
    use super::ports::{Owner, listening, owner};
    answers(port, Duration::from_millis(300))
        && listening(port).is_ok_and(|sockets| sockets.iter().any(|socket| owner(socket.pid) == Some(Owner::Others)))
}

fn runs(setup: &Setup) -> bool {
    answers(setup.port, Duration::from_millis(300)) && !taken_by_another(setup.port)
}

fn another_users(setup: &Setup) -> Report {
    Report::new(
        Outcome::Failed,
        "Another Claktory on this PC already uses its port, so this one can't start. It's probably another user's: quit Claktory in that account, then try again.",
        format!("Something of another user's answers on port {}", setup.port),
    )
}

/// How things stand, changing nothing.
pub(crate) fn check(setup: &Setup) -> Report {
    let found = examine(setup);
    if found.installed() {
        let answering = runs(setup);
        if !answering && running(&supervisor(&setup.home)).is_empty() {
            let held = crate::service::held(&setup.home);
            let quit = std::fs::read(crate::service::stopped_marker(&setup.home)).is_ok();
            let reason = if held {
                "You turned Claktory off, so your agents aren't working."
            } else if quit {
                "Claktory stopped when its app quit, so your agents aren't working."
            } else {
                "Claktory isn't running, so your agents aren't working."
            };
            let mut report = Report::new(Outcome::Stopped, reason, "");
            (report.running, report.held) = (Some(false), Some(held));
            return report;
        }
        let mut report = Report::new(Outcome::AlreadyInstalled, "Claktory is already set up on this PC.", "");
        report.running = Some(answering);
        return report;
    }
    if found.nothing() {
        return Report::new(Outcome::NotInstalled, "Claktory isn't set up on this PC yet.", "");
    }
    Report::new(Outcome::Incomplete, "Part of Claktory is missing on this PC. Repair puts it back.", found.missing())
}

/// Sets Claktory up, or (`repair`) completes it, writes its task again and
/// starts it.
pub(crate) fn install(setup: &Setup, repair: bool) -> Report {
    let found = examine(setup);
    if found.installed() && (!repair || runs(setup)) {
        return check(setup);
    }
    if !found.nothing() && !repair {
        return check(setup);
    }
    put_in(setup, &found).unwrap_or_else(|report| report)
}

/// Starts a Claktory that's set up: runs its task, and waits for the
/// backend. One that runs already is left as it is.
pub(crate) fn start(setup: &Setup) -> Report {
    let found = examine(setup);
    if !found.installed() {
        return check(setup);
    }
    if runs(setup) {
        let _ = std::fs::remove_file(crate::service::stopped_marker(&setup.home));
        return check(setup);
    }
    if taken_by_another(setup.port) {
        return another_users(setup);
    }
    // One starting, or whose backend keeps stopping: it's waited for.
    if running(&supervisor(&setup.home)).is_empty()
        && let Err(detail) = setup.task.run()
    {
        return Report::new(Outcome::Failed, "Windows didn't start Claktory.", detail);
    }
    // Started: whatever held it off is over, and it starts at sign-in as
    // the person chose.
    let _ = std::fs::remove_file(crate::service::stopped_marker(&setup.home));
    if let Err(detail) = setup.task.set_enabled(starts_at_sign_in(setup)) {
        eprintln!("could not set whether Claktory starts at sign-in: {detail}");
    }
    answered(setup, Outcome::Started, "Claktory is running.", "Claktory didn't start.")
}

/// Keeps the person's choice, and has the task follow it if it's there.
pub(crate) fn choose(setup: &Setup, starts: bool) -> Result<(), Report> {
    let failed =
        |detail: String| Report::new(Outcome::Failed, "Claktory couldn't change whether it starts when you sign in to Windows.", detail);
    keep_choice(&setup.home, starts).map_err(|error| failed(format!("{}: {error}", choice_file(&setup.home).display())))?;
    if examine(setup).ours {
        setup.task.set_enabled(enabled(setup)).map_err(failed)?;
    }
    Ok(())
}

/// Waits for the backend to answer: `outcome` when it does, NotRunning
/// (`not_running`) when it doesn't in time.
fn answered(setup: &Setup, outcome: Outcome, reason: &str, not_running: &str) -> Report {
    eprintln!("waiting for the backend");
    let deadline = Instant::now() + setup.answer_within;
    loop {
        if answers(setup.port, Duration::from_millis(200)) {
            let mut report = Report::new(outcome, reason, "");
            report.running = Some(true);
            return report;
        }
        if Instant::now() >= deadline {
            let log = setup.home.root().join("factoryd.log");
            return Report::new(
                Outcome::NotRunning,
                not_running,
                format!(
                    "Nothing answered on port {} within {}s. Windows runs Claktory's sign-in task only while you're signed in to Windows; see {}",
                    setup.port,
                    setup.answer_within.as_secs(),
                    log.display()
                ),
            );
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// Puts in what's missing (all of it on a new PC), writes the task, runs
/// it and waits for the backend.
fn put_in(setup: &Setup, found: &Found) -> Result<Report, Report> {
    let backend = setup.helpers.join(program("factoryd"));
    if !backend.is_file() {
        return Err(Report::new(
            Outcome::Damaged,
            "This copy of Claktory is damaged. Download Claktory again.",
            format!("{} has no factoryd", setup.helpers.display()),
        ));
    }
    let copying = |error: std::io::Error| {
        Report::new(
            Outcome::Failed,
            "Claktory couldn't copy its files into your user folder.",
            format!("{}: {error}", setup.home.root().display()),
        )
    };
    super::prepare_home(setup.home.root()).map_err(copying)?;
    let records = super::dirs::windows_records(setup.home.root());
    factory_agent::windows::private::create_dir_all(&records).map_err(copying)?;
    copy_programs(setup, found).map_err(copying)?;
    if found.nothing() {
        // A new install isn't an update on trial.
        let _ = std::fs::remove_file(setup.home.trial_path());
    }
    // Started now, so not held off: the task does as the person chose.
    let starts = starts_at_sign_in(setup);
    keep_choice(&setup.home, starts).map_err(copying)?;
    setup
        .task
        .register(starts, &records)
        .map_err(|detail| Report::new(Outcome::Failed, "Claktory couldn't set up its sign-in task.", detail))?;
    if taken_by_another(setup.port) {
        return Err(another_users(setup));
    }
    // A supervisor that runs, but whose backend doesn't answer, is started
    // again from the task.
    stop_supervisors(setup);
    setup.task.run().map_err(|detail| Report::new(Outcome::Failed, "Windows didn't start Claktory.", detail))?;
    let _ = std::fs::remove_file(crate::service::stopped_marker(&setup.home));
    Ok(answered(setup, Outcome::Installed, "Claktory is set up.", "Claktory is set up, but its backend didn't start."))
}

/// The backend, the supervisor's copy of it, the helpers, the agents'
/// tools and Git, and Claktory's own apps, each that's missing.
fn copy_programs(setup: &Setup, found: &Found) -> std::io::Result<()> {
    let backend = std::fs::read(setup.helpers.join(program("factoryd")))?;
    let bin = setup.home.root().join("bin");
    std::fs::create_dir_all(&bin)?;
    // A backend that's there stays: a newer one may have moved its data on.
    if !found.backend {
        eprintln!("copying the backend");
        crate::update::write_executable(&setup.home.backend(), &backend)?;
    }
    if !found.supervisor {
        eprintln!("copying the supervisor");
        crate::update::write_executable(&supervisor(&setup.home), &backend)?;
    }
    for name in crate::update::HELPERS {
        copy_missing(&setup.helpers.join(program(name)), &bin.join(program(name)))?;
    }
    let tools = crate::update::tools_dir(&setup.home);
    let mut tool_put = false;
    for name in crate::update::TOOLS {
        let at = Path::new("bin").join(program(name));
        tool_put |= copy_missing(&setup.helpers.join("tools").join(&at), &tools.join(&at))?;
    }
    if tool_put || !tools.join("LICENSES").exists() {
        crate::update::put_tool_licenses(&setup.helpers.join("tools").join("LICENSES"), &setup.home)?;
    }
    super::tools::put_in_unless_named(&setup.home, &setup.helpers)?;
    let apps = setup.helpers.join("apps");
    if apps.is_dir() && !bin.join("apps").exists() {
        copy_apps(&apps, &bin.join("apps"))?;
    }
    Ok(())
}

/// Copies `from` to `to` when `from` is there and `to` isn't; whether it did.
fn copy_missing(from: &Path, to: &Path) -> std::io::Result<bool> {
    if !from.is_file() || to.exists() {
        return Ok(false);
    }
    if let Some(folder) = to.parent() {
        std::fs::create_dir_all(folder)?;
    }
    crate::update::write_executable(to, &std::fs::read(from)?)?;
    Ok(true)
}

/// Copies the `apps` folder (its .clapp files) into place whole.
fn copy_apps(from: &Path, to: &Path) -> std::io::Result<()> {
    let staged = to.with_extension("new");
    let _ = std::fs::remove_dir_all(&staged);
    std::fs::create_dir_all(&staged)?;
    for entry in std::fs::read_dir(from)? {
        let path = entry?.path();
        if path.is_file()
            && let Some(name) = path.file_name()
        {
            std::fs::copy(&path, staged.join(name))?;
        }
    }
    std::fs::rename(&staged, to)
}

/// Asks the home's supervisors to stop (each stops its backend first), and
/// then any of its backends still running without one; ends those that
/// don't stop in time.
fn stop_supervisors(setup: &Setup) {
    let deadline = Instant::now() + STOP_WITHIN;
    for program in [supervisor(&setup.home), setup.home.backend()] {
        let pids = running(&program);
        if pids.is_empty() {
            continue;
        }
        eprintln!("stopping Claktory");
        for pid in &pids {
            if let Err(error) = super::stop::ask_to_stop(*pid) {
                eprintln!("could not ask process {pid} to stop, so it's ended: {error}");
                factory_agent::windows::system::terminate_process(*pid);
            }
        }
        while pids.iter().any(|pid| running(&program).contains(pid)) && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(100));
        }
        for pid in running(&program).into_iter().filter(|pid| pids.contains(pid)) {
            factory_agent::windows::system::terminate_process(pid);
        }
    }
}

/// Stops Claktory, and waits until nothing of this user's answers on its
/// port.
fn stop(setup: &Setup) -> Result<(), String> {
    stop_supervisors(setup);
    let deadline = Instant::now() + Duration::from_secs(3);
    while answers(setup.port, Duration::from_millis(200)) && !taken_by_another(setup.port) {
        if Instant::now() >= deadline {
            return Err(format!("something still answers on port {}", setup.port));
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    Ok(())
}

fn removal(setup: &Setup, result: Removal, reason: &str, detail: impl Into<String>, removes: &[PathBuf]) -> RemovalReport {
    let root = setup.home.root();
    let keeps = std::fs::read_dir(root)
        .map(|entries| {
            entries
                .filter_map(Result::ok)
                .map(|entry| entry.path())
                .filter(|path| {
                    !removes.contains(path) && !path.file_name().is_some_and(|name| name.to_string_lossy().starts_with(".removing-"))
                })
                .map(|path| path.display().to_string())
                .collect()
        })
        .unwrap_or_default();
    RemovalReport {
        result,
        reason: reason.to_owned(),
        detail: detail.into(),
        version: crate::update::VERSION,
        data_path: root.display().to_string(),
        removes: std::iter::once(setup.task.name.clone()).chain(removes.iter().map(|path| path.display().to_string())).collect(),
        keeps,
        items: Vec::new(),
    }
}

/// Stops Claktory and removes it; the person's data stays.
pub(crate) fn uninstall(setup: &Setup) -> RemovalReport {
    let removes: Vec<PathBuf> =
        [setup.home.root().join("bin"), crate::update::tools_dir(&setup.home)].into_iter().filter(|path| path.exists()).collect();
    if setup.task.query().is_none() && removes.is_empty() {
        return removal(setup, Removal::NotInstalled, "Claktory isn't set up on this PC.", "", &removes);
    }
    if let Err(detail) = stop(setup) {
        return removal(setup, Removal::Failed, "Windows didn't stop Claktory, so nothing was removed.", detail, &removes);
    }
    let mut failed = Vec::new();
    if let Err(detail) = setup.task.delete() {
        failed.push(detail);
    }
    crate::uninstall::stop_leftovers(&setup.home);
    for folder in &removes {
        if let Err(error) = remove_folder(setup.home.root(), folder) {
            failed.push(format!("{}: {error}", folder.display()));
        }
    }
    if failed.is_empty() {
        return removal(setup, Removal::Removed, "Claktory was removed from this PC. Your data is kept.", "", &removes);
    }
    removal(setup, Removal::PartlyRemoved, "Claktory was stopped, but some of it couldn't be removed.", failed.join("; "), &removes)
}

/// Removes `folder`, trying again for a few seconds while what ran from it
/// (the helpers its backend started) ends. When this factoryd runs from it,
/// as the installer's uninstaller runs the home's, it's moved aside first
/// (Windows lets a running program be renamed, never deleted), and deleted
/// once it's ended by a process of its own.
fn remove_folder(root: &Path, folder: &Path) -> std::io::Result<()> {
    if let Ok(me) = std::env::current_exe()
        && super::fs::canonical(&me).ok().zip(super::fs::canonical(folder).ok()).is_some_and(|(me, folder)| me.starts_with(folder))
    {
        let name = format!(".removing-{}", crate::transcript::now_ms());
        let aside = root.join(&name);
        std::fs::create_dir_all(&aside)?;
        std::fs::rename(&me, aside.join(program("factoryd")))?;
        delete_later(root, &name)?;
    }
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        match std::fs::remove_dir_all(folder) {
            Ok(()) => return Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(_) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(200)),
            Err(error) => return Err(error),
        }
    }
}

/// Deletes `name`, a folder in `root`, a few seconds from now, from
/// Windows' own `cmd`, in a process group of its own with no console, so it
/// carries on once this ends. Run in `root` and given only the folder's
/// name, which is Claktory's own: cmd expands `%…%` even inside quotes,
/// and the home's path holds the user's name.
fn delete_later(root: &Path, name: &str) -> std::io::Result<()> {
    use std::os::windows::process::CommandExt;
    /// CREATE_NEW_PROCESS_GROUP | CREATE_NO_WINDOW.
    const DETACHED: u32 = 0x0000_0200 | 0x0800_0000;
    if !name.bytes().all(|byte| byte.is_ascii_alphanumeric() || byte == b'.' || byte == b'-') {
        return Err(std::io::Error::other(format!("{name} isn't a folder name Claktory makes")));
    }
    let system = factory_agent::windows::system::system_folder().unwrap_or_else(|| PathBuf::from(r"C:\Windows\System32"));
    std::process::Command::new(system.join("cmd.exe"))
        .raw_arg(format!("/D /C \"ping -n 6 127.0.0.1 >NUL & rmdir /S /Q {name}\""))
        .current_dir(root)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .creation_flags(DETACHED)
        .spawn()
        .map(drop)
}

/// This user's real Claktory (`dev`, the dev one), set up from the folder
/// this factoryd is in.
fn this_pc(dev: bool) -> std::io::Result<Setup> {
    let home = Home::new(super::dirs::factory_home(dev)?);
    let exe = std::env::current_exe()?;
    let helpers = exe.parent().ok_or_else(|| std::io::Error::other("factoryd isn't in a folder"))?.to_owned();
    let port = crate::home::instance_in(dev, home.root().to_owned()).addr.port();
    let task = Task::for_home(&home, dev)?;
    Ok(Setup { home, helpers, task, port, answer_within: ANSWER_WITHIN })
}

/// `factoryd install [--dev] [--check | --repair | --start] [--consent=yes|no]`.
pub fn install_main(args: &[String]) -> ExitCode {
    let (mut check_only, mut repair, mut start_only, mut dev, mut consent) = (false, false, false, false, None);
    for arg in args {
        match arg.as_str() {
            "--check" => check_only = true,
            "--repair" => repair = true,
            "--start" => start_only = true,
            "--dev" => dev = true,
            "--consent=yes" => consent = Some(true),
            "--consent=no" => consent = Some(false),
            _ => {
                eprintln!("usage: factoryd install [--dev] [--check | --repair | --start] [--consent=yes|no]");
                return ExitCode::from(2);
            }
        }
    }
    let report = match this_pc(dev) {
        Ok(setup) => {
            let mut report = match consent.map(|starts| choose(&setup, starts)) {
                Some(Err(report)) => report,
                _ if check_only => check(&setup),
                _ if start_only => start(&setup),
                _ => install(&setup, repair),
            };
            report.starts_at_sign_in = Some(starts_at_sign_in(&setup));
            report
        }
        Err(error) => Report::new(Outcome::Failed, "Claktory couldn't find where it keeps its files on this PC.", error.to_string()),
    };
    println!("{}", serde_json::to_string(&report).unwrap_or_default());
    ExitCode::from(report.result.exit_code())
}

/// `factoryd uninstall [--dev]`.
pub fn uninstall_main(args: &[String]) -> ExitCode {
    let mut dev = false;
    for arg in args {
        match arg.as_str() {
            "--dev" => dev = true,
            "--delete-data" | "--dry-run" => {
                eprintln!("{arg} isn't on Windows yet: factoryd uninstall removes Claktory and keeps your data.");
                return ExitCode::from(2);
            }
            _ => {
                eprintln!("usage: factoryd uninstall [--dev]");
                return ExitCode::from(2);
            }
        }
    }
    let setup = match this_pc(dev) {
        Ok(setup) => setup,
        Err(error) => {
            eprintln!("Claktory couldn't find where it keeps its files on this PC: {error}");
            return ExitCode::from(2);
        }
    };
    let report = uninstall(&setup);
    println!("{}", serde_json::to_string(&report).unwrap_or_default());
    ExitCode::from(report.result.exit_code())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::windows::sign_in::Action;

    /// A release as the installer lays it down, its programs stand-ins; a
    /// home to set up; and a task of the test's own, on a port where
    /// nothing answers.
    fn setup(name: &str) -> (PathBuf, Setup) {
        let dir = std::env::temp_dir().join(format!("claktory-install-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let helpers = dir.join("backend");
        let file = |path: PathBuf, text: &str| {
            std::fs::create_dir_all(path.parent().expect("folder")).expect("folder");
            std::fs::write(path, text).expect("file");
        };
        file(helpers.join(program("factoryd")), "factoryd v2");
        file(helpers.join(program(crate::update::APPS_HOST)), "apps host");
        file(helpers.join("tools").join("bin").join(program("rg")), "rg");
        file(helpers.join("tools").join("LICENSES").join("ripgrep.txt"), "MIT");
        file(helpers.join("tools").join("git.txt"), "git-2.51.0\r\n");
        file(helpers.join("tools").join("git-2.51.0").join("usr").join("bin").join("bash.exe"), "bash");
        file(helpers.join("apps").join("charts.clapp"), "charts");
        let home = Home::new(dir.join("home"));
        let sid = factory_agent::windows::system::user_sid().expect("SID");
        let action = Action::supervising(&supervisor(&home).display().to_string(), &home.root().display().to_string(), false);
        let task = Task::new(format!(r"\Claktory\Claktory test {name} {} {sid}", std::process::id()), action);
        let _ = task.delete();
        let port = std::net::TcpListener::bind("127.0.0.1:0").expect("port").local_addr().expect("port").port();
        (dir, Setup { home, helpers, task, port, answer_within: Duration::from_millis(300) })
    }

    fn read(path: PathBuf) -> String {
        std::fs::read_to_string(&path).unwrap_or_else(|error| panic!("{}: {error}", path.display()))
    }

    #[test]
    fn a_new_pc_gets_everything_and_a_task_that_starts_it_at_sign_in_as_chosen() {
        let (dir, setup) = setup("new");
        assert_eq!(check(&setup).result, Outcome::NotInstalled);
        let report = install(&setup, false);
        // Nothing runs the stand-in, so nothing answers.
        assert_eq!(report.result, Outcome::NotRunning, "{report:?}");
        let root = setup.home.root();
        assert_eq!(read(setup.home.backend()), "factoryd v2");
        assert_eq!(read(supervisor(&setup.home)), "factoryd v2", "the supervisor's own copy");
        assert_eq!(read(root.join("bin").join(program(crate::update::APPS_HOST))), "apps host");
        assert_eq!(read(root.join("tools").join("bin").join(program("rg"))), "rg");
        assert_eq!(read(root.join("tools").join("LICENSES").join("ripgrep.txt")), "MIT");
        assert_eq!(read(root.join("tools").join("git.txt")).trim(), "git-2.51.0");
        assert!(root.join("tools").join("git-2.51.0").join("usr").join("bin").join("bash.exe").is_file());
        assert_eq!(read(root.join("bin").join("apps").join("charts.clapp")), "charts");
        assert!(factory_agent::windows::system::is_private_path(root).expect("home"), "the home is private");
        let task = setup.task.query().expect("the task");
        assert_eq!((task.enabled, task.runs.as_ref()), (true, Some(&setup.task.action)), "no choice made: it starts at sign-in");
        assert_eq!(kept_choice(&setup.home), Some(true));

        // Set up: nothing runs it now, so it's stopped, and the app starts it.
        let report = check(&setup);
        assert_eq!((report.result, report.held), (Outcome::Stopped, Some(false)), "{report:?}");
        assert_eq!(install(&setup, false).result, Outcome::Stopped, "set up already: left alone");

        // The person's choice, and their switch holding it off.
        choose(&setup, false).expect("chosen");
        assert!(!setup.task.query().expect("task").enabled);
        assert_eq!(kept_choice(&setup.home), Some(false));
        choose(&setup, true).expect("chosen");
        assert!(setup.task.query().expect("task").enabled);
        std::fs::write(crate::service::stopped_marker(&setup.home), r#"{"by":"switch","time":1}"#).expect("marker");
        let report = check(&setup);
        assert_eq!((report.result, report.held), (Outcome::Stopped, Some(true)));
        choose(&setup, true).expect("chosen");
        assert!(!setup.task.query().expect("task").enabled, "held off, whatever they chose");
        assert!(starts_at_sign_in(&setup), "what they chose stands");
        assert_eq!(start(&setup).result, Outcome::NotRunning);
        assert!(!crate::service::stopped_marker(&setup.home).exists(), "started: no longer held");
        assert!(setup.task.query().expect("task").enabled, "and it starts at sign-in again");

        setup.task.delete().expect("deleted");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn repair_puts_back_what_is_missing_and_never_replaces_a_backend() {
        let (dir, setup) = setup("repair");
        choose(&setup, false).expect("chosen before it's set up, as the installer does");
        install(&setup, false);
        assert!(!setup.task.query().expect("task").enabled, "set up as chosen");
        crate::update::write_executable(&setup.home.backend(), b"factoryd v3, from an update").expect("update");
        std::fs::remove_file(supervisor(&setup.home)).expect("supervisor gone");
        std::fs::remove_file(setup.home.root().join("tools").join("bin").join(program("rg"))).expect("rg gone");
        setup.task.delete().expect("task gone");
        let report = check(&setup);
        assert_eq!(report.result, Outcome::Incomplete);
        assert_eq!(report.detail, "the sign-in task is missing; the supervisor is missing");
        assert_eq!(install(&setup, false).result, Outcome::Incomplete, "only Repair repairs");

        assert_eq!(install(&setup, true).result, Outcome::NotRunning);
        assert_eq!(read(setup.home.backend()), "factoryd v3, from an update", "the newer backend stays");
        assert_eq!(read(supervisor(&setup.home)), "factoryd v2");
        assert_eq!(read(setup.home.root().join("tools").join("bin").join(program("rg"))), "rg");
        assert!(!setup.task.query().expect("task").enabled, "still as chosen");

        // A task of its name that runs something else is no longer ours.
        let other = Task::new(setup.task.name.clone(), Action::supervising(r"C:\elsewhere.exe", r"C:\", false));
        other.register(true, &std::env::temp_dir()).expect("registered");
        assert_eq!(check(&setup).detail, "the sign-in task doesn't run Claktory's supervisor");
        install(&setup, true);
        assert_eq!(setup.task.query().and_then(|task| task.runs), Some(setup.task.action.clone()));

        setup.task.delete().expect("deleted");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_damaged_copy_sets_nothing_up() {
        let (dir, setup) = setup("damaged");
        std::fs::remove_file(setup.helpers.join(program("factoryd"))).expect("gone");
        assert_eq!(install(&setup, false).result, Outcome::Damaged);
        assert_eq!(setup.task.query(), None);
        assert!(!setup.home.backend().exists());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn uninstalling_removes_the_task_and_the_programs_and_keeps_the_data() {
        let (dir, setup) = setup("uninstall");
        assert_eq!(uninstall(&setup).result, Removal::NotInstalled);
        install(&setup, false);
        std::fs::write(setup.home.root().join("config.json"), "{}").expect("data");
        let report = uninstall(&setup);
        assert_eq!(report.result, Removal::Removed, "{report:?}");
        assert_eq!(setup.task.query(), None);
        assert!(!setup.home.root().join("bin").exists() && !setup.home.root().join("tools").exists());
        assert!(setup.home.root().join("config.json").is_file());
        assert!(report.keeps.iter().any(|kept| kept.ends_with("config.json")), "{:?}", report.keeps);
        assert_eq!(report.removes[0], setup.task.name);
        assert_eq!(uninstall(&setup).result, Removal::NotInstalled);
        let _ = std::fs::remove_dir_all(dir);
    }

    /// A home whose path cmd would expand (`%…%` in the user's name) still
    /// has its own folder deleted, and only that.
    #[test]
    fn a_folder_left_to_delete_later_goes_whatever_the_homes_path() {
        let root = std::env::temp_dir().join(format!("claktory-later-%PATH%-{}", std::process::id()));
        let name = ".removing-1";
        std::fs::create_dir_all(root.join(name).join("inside")).expect("folder");
        std::fs::create_dir_all(root.join("kept")).expect("kept");
        delete_later(&root, name).expect("started");
        wait_until("the deletion", || !root.join(name).exists());
        assert!(root.join("kept").is_dir());
        assert!(delete_later(&root, "x & del %TEMP%").is_err(), "only a name of Claktory's");
        let _ = std::fs::remove_dir_all(root);
    }

    fn wait_until(what: &str, done: impl Fn() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(60);
        while !done() {
            assert!(Instant::now() < deadline, "{what} didn't happen within a minute");
            std::thread::sleep(Duration::from_millis(200));
        }
    }

    /// End to end, Task Scheduler running the task and the real supervisor
    /// and backend (this build's `factoryd.exe`): set up, stopped by the
    /// person's switch, started, and removed, data kept.
    #[test]
    #[ignore = "sets up and removes this user's dev Claktory, and needs a signed-in session for Task Scheduler to run its task"]
    fn the_dev_claktory_is_set_up_stopped_started_and_removed() {
        let built = std::env::current_exe().expect("exe").parent().and_then(Path::parent).expect("target").join(program("factoryd"));
        let helpers = std::env::temp_dir().join(format!("claktory-install-e2e-{}", std::process::id()));
        std::fs::create_dir_all(&helpers).expect("helpers");
        std::fs::copy(&built, helpers.join(program("factoryd"))).unwrap_or_else(|error| panic!("{}: {error}", built.display()));
        let mut setup = this_pc(true).expect("this PC");
        setup.helpers = helpers.clone();
        assert!(!setup.home.backend().exists(), "the dev Claktory is set up already: {}", setup.home.root().display());

        choose(&setup, false).expect("chosen");
        let report = install(&setup, false);
        assert_eq!(report.result, Outcome::Installed, "{report:?}");
        assert!(!setup.task.query().expect("task").enabled, "not at sign-in, as chosen");
        let report = check(&setup);
        assert_eq!((report.result, report.running), (Outcome::AlreadyInstalled, Some(true)), "{report:?}");

        // StopEngine, by the person's switch.
        std::fs::write(crate::service::stopped_marker(&setup.home), r#"{"by":"switch","time":1}"#).expect("marker");
        choose(&setup, true).expect("chosen");
        super::super::service::stop(&setup.home, &setup.task, true).expect("stopping");
        wait_until("the stop", || running(&supervisor(&setup.home)).is_empty() && !answers(setup.port, Duration::from_millis(200)));
        let report = check(&setup);
        assert_eq!((report.result, report.held), (Outcome::Stopped, Some(true)), "{report:?}");
        assert!(!setup.task.query().expect("task").enabled, "held off at sign-in");

        let report = start(&setup);
        assert_eq!(report.result, Outcome::Started, "{report:?}");
        assert!(setup.task.query().expect("task").enabled, "at sign-in again, as chosen");
        assert!(!crate::service::stopped_marker(&setup.home).exists());

        let report = uninstall(&setup);
        assert_eq!(report.result, Removal::Removed, "{report:?}");
        assert_eq!(setup.task.query(), None);
        assert!(!setup.home.root().join("bin").exists());
        assert!(!answers(setup.port, Duration::from_millis(200)));
        let _ = std::fs::remove_file(choice_file(&setup.home));
        let _ = std::fs::remove_dir_all(helpers);
    }
}
