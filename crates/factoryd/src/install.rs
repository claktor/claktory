//! `factoryd install`: sets Claktory up on a Mac from the app, with no
//! Terminal. The app carries the backend in its bundle (`Contents/Helpers`:
//! factoryd, the apps host and the supervisor's app); on first launch it runs
//! this factoryd, which puts them where updates expect them
//! (`~/.factory/bin`), writes the LaunchAgent the installers write
//! (`~/Library/LaunchAgents/dev.factory.factoryd.plist`, running the
//! supervisor) and loads it, then waits until the backend answers.
//!
//! It never replaces what's there: a Claktory already set up (the
//! LaunchAgent runs this Factory's supervisor, and the backend is in place)
//! is left alone, whatever its version, running or not. `--repair`, which
//! the person asks for, fills in what's missing and reloads the LaunchAgent,
//! and still never replaces a backend that's there: a newer one may have
//! moved factory.db on already. `--check` only says how things are.
//!
//! `--start` starts a Claktory the person stopped (friend bug #27,
//! `crate::service`): it enables the job, loads it, or starts it if it's
//! loaded, and waits until the backend answers. `--check` says `stopped`
//! when the job is disabled and not loaded, and `held` when the person's
//! switch stopped it, so the app leaves it stopped when it opens.
//!
//! It answers with one JSON line on stdout (`Report`) and an exit code for
//! each result (`Outcome::exit_code`); anything else goes to stderr.

use std::io::Read as _;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};
use std::time::{Duration, Instant};

use serde::Serialize;

use crate::home::Home;
use crate::launch_agent;

/// The supervisor's app, in the bundle's Helpers and in `~/.factory/bin`.
const SUPERVISOR_APP: &str = "Claktory Agents.app";
/// The supervisor's program in its app.
const SUPERVISOR_PROGRAM: &str = "Contents/MacOS/factoryd-supervisor";
/// Where agents look for programs when the login shell can't say.
const FALLBACK_PATH: &str = "/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin";
/// How long the login shell has to say its PATH.
const SHELL_WITHIN: Duration = Duration::from_secs(5);
/// Launch Services' tool.
const LSREGISTER: &str = "/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister";
/// How long the backend has to answer once its job is loaded.
const ANSWER_WITHIN: Duration = Duration::from_secs(30);

/// How things stand, or what was done.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Outcome {
    /// (`--check`) Nothing of Claktory's is on this Mac.
    NotInstalled,
    /// Set up (or repaired), and the backend answers.
    Installed,
    /// The LaunchAgent runs this Factory's supervisor and the backend is in
    /// place; nothing was touched.
    AlreadyInstalled,
    /// Some of it is there and some isn't; `--repair` completes it.
    Incomplete,
    /// All in place, but the backend didn't answer in time.
    NotRunning,
    /// It couldn't be done; `reason` says why.
    Failed,
    /// The bundle's helpers are missing or aren't signed as they should be:
    /// only downloading Claktory again helps.
    Damaged,
    /// (`--check`) The person stopped it (`held` says how): its job is
    /// disabled and not loaded. `--start` starts it.
    Stopped,
    /// (`--start`) Started, and the backend answers.
    Started,
}

impl Outcome {
    fn exit_code(self) -> u8 {
        match self {
            Outcome::Installed => 0,
            Outcome::Failed => 1,
            Outcome::NotInstalled => 3,
            Outcome::AlreadyInstalled => 4,
            Outcome::Incomplete => 5,
            Outcome::NotRunning => 6,
            Outcome::Damaged => 7,
            Outcome::Stopped => 8,
            Outcome::Started => 9,
        }
    }
}

/// The JSON line `factoryd install` answers with.
#[derive(Debug, Serialize)]
pub(crate) struct Report {
    pub(crate) result: Outcome,
    /// A sentence the app shows as it is.
    pub(crate) reason: String,
    /// Technical detail (an error, a log's path); empty when there's none.
    pub(crate) detail: String,
    /// The version of the backend in this bundle.
    pub(crate) version: &'static str,
    /// (already_installed) Whether the backend answered just then.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) running: Option<bool>,
    /// (stopped) The person's switch stopped it, so opening the app leaves
    /// it stopped; false when the app quitting did.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) held: Option<bool>,
}

impl Report {
    fn new(result: Outcome, reason: &str, detail: impl Into<String>) -> Report {
        Report { result, reason: reason.to_owned(), detail: detail.into(), version: crate::update::VERSION, running: None, held: None }
    }
}

/// Where everything goes, and how to reach launchd: this user's, or a test's.
pub(crate) struct Setup {
    pub(crate) home: Home,
    /// The bundle's Helpers: factoryd, claktory-apps, Claktory Agents.app.
    pub(crate) helpers: PathBuf,
    pub(crate) plist: PathBuf,
    pub(crate) label: String,
    pub(crate) launchctl: PathBuf,
    /// `gui/<uid>`.
    pub(crate) domain: String,
    pub(crate) codesign: PathBuf,
    /// Registers the app with Launch Services.
    pub(crate) lsregister: PathBuf,
    /// The backend's port.
    pub(crate) port: u16,
    /// How long the backend has to answer once its job is loaded; for
    /// `uninstall`, how long launchd and the backend have to stop.
    pub(crate) answer_within: Duration,
    /// The job's PATH; None asks the login shell.
    pub(crate) path: Option<String>,
    /// More of the job's environment (a test's FACTORY_HOME and FACTORY_PORT).
    pub(crate) env: Vec<(String, String)>,
}

/// What's on this Mac now.
struct Found {
    /// A plist with the job's label that runs this Factory's supervisor,
    /// whose program is there.
    job: bool,
    /// Any plist where the job's goes.
    plist: bool,
    backend: bool,
    /// Either supervisor, the app's or the bare one of older installs.
    supervisor: bool,
}

impl Found {
    fn installed(&self) -> bool {
        self.job && self.backend
    }

    fn nothing(&self) -> bool {
        !self.plist && !self.backend && !self.supervisor
    }

    /// What's missing, for `detail`.
    fn missing(&self) -> String {
        let mut missing = Vec::new();
        if !self.job {
            missing.push(if self.plist { "the LaunchAgent doesn't run Claktory's supervisor" } else { "the LaunchAgent is missing" });
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
    let supervisors = launch_agent::supervisors(&setup.home);
    let job = setup.plist.is_file()
        && launch_agent::read(&setup.plist).is_ok_and(|plist| {
            let program = plist["ProgramArguments"][0].as_str().unwrap_or_default().to_owned();
            plist["Label"] == setup.label.as_str()
                && supervisors.iter().any(|supervisor| supervisor.to_string_lossy() == program && supervisor.is_file())
        });
    let backend = std::fs::metadata(setup.home.backend()).is_ok_and(|backend| backend.is_file() && backend.len() > 0);
    Found { job, plist: setup.plist.exists(), backend, supervisor: supervisors.iter().any(|supervisor| supervisor.is_file()) }
}

/// Whether something answers on the backend's port, within `within`.
pub(crate) fn answers(port: u16, within: Duration) -> bool {
    let addr = std::net::SocketAddr::from(([127, 0, 0, 1], port));
    port != 0 && std::net::TcpStream::connect_timeout(&addr, within).is_ok()
}

/// Something of another user's answers on `port`: it answers, and no
/// process of this user's listens there (lsof sees only this user's). False
/// when that can't be told.
fn taken_by_another(port: u16) -> bool {
    if !answers(port, Duration::from_millis(300)) {
        return false;
    }
    let uid = unsafe { libc::getuid() };
    let listening = Command::new("/usr/sbin/lsof")
        .args(["-nP", "-a", "-t", "-sTCP:LISTEN", &format!("-iTCP:{port}"), "-u", &uid.to_string()])
        .stderr(std::process::Stdio::null())
        .output();
    match listening {
        // lsof exits 1, printing nothing, when nothing of this user's matches.
        Ok(output) => output.stdout.iter().all(u8::is_ascii_whitespace) && output.status.code() == Some(1),
        Err(_) => false,
    }
}

/// How things stand, changing nothing.
pub(crate) fn check(setup: &Setup) -> Report {
    let found = examine(setup);
    if found.installed() {
        let running = answers(setup.port, Duration::from_millis(300)) && !taken_by_another(setup.port);
        if !running && disabled(setup) && !loaded(setup) {
            let held = crate::service::held(&setup.home);
            let reason = if held {
                "You turned Claktory off, so your agents aren't working."
            } else {
                "Claktory stopped when its app quit, so your agents aren't working."
            };
            let mut report = Report::new(Outcome::Stopped, reason, "");
            (report.running, report.held) = (Some(false), Some(held));
            return report;
        }
        let mut report = Report::new(Outcome::AlreadyInstalled, "Claktory is already set up on this Mac.", "");
        report.running = Some(running);
        return report;
    }
    if found.nothing() {
        return Report::new(Outcome::NotInstalled, "Claktory isn't set up on this Mac yet.", "");
    }
    Report::new(Outcome::Incomplete, "Part of Claktory is missing on this Mac. Repair puts it back.", found.missing())
}

/// Sets Claktory up, or (`repair`) completes it and reloads its job.
pub(crate) fn install(setup: &Setup, repair: bool) -> Report {
    let found = examine(setup);
    if found.installed() && (!repair || (answers(setup.port, Duration::from_millis(300)) && !taken_by_another(setup.port))) {
        return check(setup);
    }
    if !found.nothing() && !repair {
        return check(setup);
    }
    put_in(setup, &found).unwrap_or_else(|report| report)
}

/// Starts a Claktory that's set up: enables its job, loads it, or starts it
/// when it's loaded, and waits for the backend. One that runs already is
/// left as it is (already_installed, running).
pub(crate) fn start(setup: &Setup) -> Report {
    let found = examine(setup);
    if !found.installed() {
        return check(setup);
    }
    if answers(setup.port, Duration::from_millis(300)) && !taken_by_another(setup.port) {
        let _ = std::fs::remove_file(crate::service::stopped_marker(&setup.home));
        return check(setup);
    }
    if taken_by_another(setup.port) {
        return another_users(setup);
    }
    let service = format!("{}/{}", setup.domain, setup.label);
    let started = launchctl(setup, &["enable", &service]).and_then(|()| {
        if loaded(setup) {
            launchctl(setup, &["kickstart", &service])
        } else {
            launchctl(setup, &["bootstrap", &setup.domain, &setup.plist.to_string_lossy()])
        }
    });
    if let Err(detail) = started {
        return Report::new(Outcome::Failed, "macOS didn't start Claktory's background service.", detail);
    }
    let _ = std::fs::remove_file(crate::service::stopped_marker(&setup.home));
    answered(setup, Outcome::Started, "Claktory is running.", "Claktory didn't start.")
}

/// Runs launchctl with `args`; Err with what it said when it fails.
fn launchctl(setup: &Setup, args: &[&str]) -> Result<(), String> {
    let output = Command::new(&setup.launchctl).args(args).output().map_err(|error| format!("Can't run launchctl: {error}"))?;
    if output.status.success() {
        Ok(())
    } else {
        Err(format!("launchctl {}: {}", args.join(" "), String::from_utf8_lossy(&output.stderr).trim()))
    }
}

/// Whether launchd has the job loaded.
fn loaded(setup: &Setup) -> bool {
    Command::new(&setup.launchctl)
        .arg("print")
        .arg(format!("{}/{}", setup.domain, setup.label))
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

/// Whether the job is disabled (`launchctl print-disabled`: `"<label>" =>
/// disabled`, or `=> true` before macOS 13).
fn disabled(setup: &Setup) -> bool {
    let Ok(output) = Command::new(&setup.launchctl).arg("print-disabled").arg(&setup.domain).stderr(Stdio::null()).output() else {
        return false;
    };
    let quoted = format!("\"{}\"", setup.label);
    String::from_utf8_lossy(&output.stdout).lines().any(|line| {
        let mut parts = line.trim().splitn(2, "=>");
        parts.next().is_some_and(|label| label.trim() == quoted)
            && parts.next().is_some_and(|state| matches!(state.trim(), "disabled" | "true"))
    })
}

fn another_users(setup: &Setup) -> Report {
    Report::new(
        Outcome::Failed,
        "Another Claktory on this Mac already uses its port, so this one can't start. It's probably another user's: quit Claktory in that account, then try again.",
        format!("Something of another user's answers on port {}", setup.port),
    )
}

/// Waits for the backend to answer once its job is loaded: `outcome` when
/// it does, NotRunning (`not_running`) when it doesn't in time.
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
                format!("Nothing answered on port {} within {}s; see {}", setup.port, setup.answer_within.as_secs(), log.display()),
            );
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// Puts in what's missing (all of it on a new Mac), writes the job's plist,
/// loads it and waits for the backend.
fn put_in(setup: &Setup, found: &Found) -> Result<Report, Report> {
    let damaged = |detail: String| Report::new(Outcome::Damaged, "This copy of Claktory is damaged. Download Claktory again.", detail);
    let backend = setup.helpers.join("factoryd");
    let helpers: Vec<std::path::PathBuf> = crate::update::HELPERS.iter().map(|name| setup.helpers.join(name)).collect();
    // The tools every agent gets (ripgrep, jq), in Helpers/tools.
    let tools: Vec<std::path::PathBuf> = crate::update::TOOLS.iter().map(|name| setup.helpers.join("tools").join(name)).collect();
    let app = setup.helpers.join(SUPERVISOR_APP);
    if !backend.is_file() || !app.join(SUPERVISOR_PROGRAM).is_file() {
        return Err(damaged(format!("{} has no factoryd or {SUPERVISOR_APP}", setup.helpers.display())));
    }
    let pieces = helpers.iter().chain(&tools).map(|helper| (helper, false));
    for (piece, deep) in std::iter::once((&backend, false)).chain(pieces).chain([(&app, true)]) {
        if piece.exists() {
            verify(&setup.codesign, piece, deep).map_err(damaged)?;
        }
    }
    let copying = |error: std::io::Error| {
        Report::new(
            Outcome::Failed,
            "Claktory couldn't copy its files into your home folder.",
            format!("{}: {error}", setup.home.root().display()),
        )
    };
    let bin = setup.home.root().join("bin");
    std::fs::create_dir_all(&bin).map_err(copying)?;
    // A backend that's there stays: a newer one may have moved its data on.
    if !found.backend {
        eprintln!("copying the backend");
        let bytes = std::fs::read(&backend).map_err(copying)?;
        crate::update::write_executable(&setup.home.backend(), &bytes).map_err(copying)?;
    }
    for (name, bundled) in crate::update::HELPERS.iter().zip(&helpers) {
        let installed = bin.join(name);
        if bundled.is_file() && !installed.exists() {
            let bytes = std::fs::read(bundled).map_err(copying)?;
            crate::update::write_executable(&installed, &bytes).map_err(copying)?;
        }
    }
    let installed_tools = crate::update::tools_dir(&setup.home);
    let mut tool_put = false;
    for (name, bundled) in crate::update::TOOLS.iter().zip(&tools) {
        let installed = installed_tools.join(name);
        if bundled.is_file() && !installed.exists() {
            std::fs::create_dir_all(&installed_tools).map_err(copying)?;
            let bytes = std::fs::read(bundled).map_err(copying)?;
            crate::update::write_executable(&installed, &bytes).map_err(copying)?;
            tool_put = true;
        }
    }
    // Their licenses, in Resources: codesign refuses data in Helpers. A tool
    // put in beside others already there (jq beside rg) brings its own.
    if let Some(contents) = setup.helpers.parent()
        && installed_tools.is_dir()
        && (tool_put || !installed_tools.join("LICENSES").exists())
    {
        crate::update::put_tool_licenses(&contents.join("Resources/tools/LICENSES"), &setup.home).map_err(copying)?;
    }
    // Claktory's own apps (`apps/*.clapp`), for the Marketplace's Apps tab to offer
    // (`apps::bundled`): data, so in the bundle's Resources (codesign
    // refuses data in Helpers); updates bring theirs in their release
    // folders.
    let apps = setup.helpers.parent().map_or_else(|| setup.helpers.join("apps"), |contents| contents.join("Resources/apps"));
    let installed_apps = bin.join("apps");
    if apps.is_dir() && !installed_apps.exists() {
        copy_apps(&apps, &installed_apps).map_err(copying)?;
    }
    let supervisor = launch_agent::supervisors(&setup.home)[0].clone();
    if !supervisor.is_file() {
        eprintln!("copying the supervisor");
        copy_app(&app, &bin.join(SUPERVISOR_APP)).map_err(copying)?;
    }
    if found.nothing() {
        // A new install isn't an update on trial.
        let _ = std::fs::remove_file(setup.home.trial_path());
    }
    let path = setup.path.clone().unwrap_or_else(login_path);
    write_plist(setup, &supervisor, &path).map_err(|detail| {
        Report::new(Outcome::Failed, "Claktory couldn't set up its background service.", format!("{}: {detail}", setup.plist.display()))
    })?;
    if taken_by_another(setup.port) {
        return Err(another_users(setup));
    }
    // macOS names the job's Login Items entry after its
    // AssociatedBundleIdentifiers' app (friend bug #28) only if Launch
    // Services knows the app when the job is first loaded, and keeps that
    // name for good.
    if let Some(app) = setup.helpers.parent().and_then(Path::parent).filter(|app| app.extension().is_some_and(|ext| ext == "app")) {
        let _ = Command::new(&setup.lsregister).arg("-f").arg(app).stdout(Stdio::null()).stderr(Stdio::null()).status();
    }
    load(setup).map_err(|detail| Report::new(Outcome::Failed, "macOS didn't start Claktory's background service.", detail))?;
    // Running again: whatever stopped it (friend bug #27) is over.
    let _ = std::fs::remove_file(crate::service::stopped_marker(&setup.home));
    Ok(answered(setup, Outcome::Installed, "Claktory is set up.", "Claktory is set up, but its backend didn't start."))
}

/// Checks a helper's signature; `deep` for an app.
fn verify(codesign: &Path, piece: &Path, deep: bool) -> Result<(), String> {
    let mut command = Command::new(codesign);
    command.arg("--verify").arg("--strict");
    if deep {
        command.arg("--deep");
    }
    let output = command.arg(piece).output().map_err(|error| format!("Can't run codesign: {error}"))?;
    if output.status.success() {
        Ok(())
    } else {
        Err(format!("{} isn't signed as it should be: {}", piece.display(), String::from_utf8_lossy(&output.stderr).trim()))
    }
}

/// Copies the bundle's `apps` folder (its .clapp files) into place whole.
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

/// Copies the supervisor's app into place without the download's quarantine
/// (which launchd would otherwise stop at), moving aside a broken one there.
fn copy_app(app: &Path, to: &Path) -> std::io::Result<()> {
    let staged = to.with_extension("app.new");
    let _ = std::fs::remove_dir_all(&staged);
    let status = Command::new("/usr/bin/ditto").arg("--noqtn").arg(app).arg(&staged).status()?;
    if !status.success() {
        return Err(std::io::Error::other(format!("ditto couldn't copy {}", app.display())));
    }
    // ditto's --noqtn leaves an approved download's flag ("01c3;…") on
    // what it copies; it goes, as it never was on the backend.
    let _ = Command::new("/usr/bin/xattr").args(["-dr", "com.apple.quarantine"]).arg(&staged).stderr(Stdio::null()).status();
    let aside = to.with_extension("app.old");
    let _ = std::fs::remove_dir_all(&aside);
    if to.exists() {
        std::fs::rename(to, &aside)?;
    }
    std::fs::rename(&staged, to)?;
    let _ = std::fs::remove_dir_all(&aside);
    Ok(())
}

/// The job's plist, as scripts/install-release.sh writes it: the supervisor,
/// kept alive, Interactive (launchd throttles anything else, and every
/// agent's build with it), with the login shell's PATH and the log.
fn write_plist(setup: &Setup, supervisor: &Path, path: &str) -> Result<(), String> {
    let log = setup.home.root().join("factoryd.log").to_string_lossy().into_owned();
    let mut env = serde_json::Map::new();
    env.insert("PATH".to_owned(), path.into());
    for (key, value) in &setup.env {
        env.insert(key.clone(), value.as_str().into());
    }
    let plist = serde_json::json!({
        "Label": setup.label,
        "ProgramArguments": [supervisor.to_string_lossy(), "supervise"],
        "EnvironmentVariables": env,
        "RunAtLoad": true,
        "KeepAlive": true,
        // So Login Items shows it as Claktory, with its icon (friend bug #28).
        "AssociatedBundleIdentifiers": [launch_agent::APP_BUNDLE_ID],
        "ProcessType": "Interactive",
        "StandardOutPath": log,
        "StandardErrorPath": log,
    });
    let dir = setup.plist.parent().ok_or("no folder")?;
    std::fs::create_dir_all(dir).map_err(|error| error.to_string())?;
    let new = setup.plist.with_extension("plist.new");
    std::fs::write(&new, plist.to_string()).map_err(|error| error.to_string())?;
    let converted = Command::new("/usr/bin/plutil").args(["-convert", "xml1"]).arg(&new).output().map_err(|error| error.to_string())?;
    if !converted.status.success() {
        let _ = std::fs::remove_file(&new);
        return Err(format!("plutil: {}", String::from_utf8_lossy(&converted.stderr).trim()));
    }
    std::fs::rename(&new, &setup.plist).map_err(|error| error.to_string())
}

/// Unloads the job if it's loaded, then enables it (a stopped Claktory's is
/// disabled, and launchd won't load it so) and loads its plist.
fn load(setup: &Setup) -> Result<(), String> {
    let service = format!("{}/{}", setup.domain, setup.label);
    let launchctl = |args: &[&std::ffi::OsStr]| Command::new(&setup.launchctl).args(args).output();
    let _ = launchctl(&["bootout".as_ref(), service.as_ref()]);
    let deadline = Instant::now() + Duration::from_secs(25);
    while launchctl(&["print".as_ref(), service.as_ref()]).is_ok_and(|output| output.status.success()) && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(100));
    }
    let _ = launchctl(&["enable".as_ref(), service.as_ref()]);
    let output = launchctl(&["bootstrap".as_ref(), setup.domain.as_ref(), setup.plist.as_os_str()])
        .map_err(|error| format!("Can't run launchctl: {error}"))?;
    if output.status.success() {
        Ok(())
    } else {
        Err(format!("launchctl bootstrap {} {}: {}", setup.domain, setup.plist.display(), String::from_utf8_lossy(&output.stderr).trim()))
    }
}

/// The PATH this user's login shell sets up (Homebrew's, cargo's…), which
/// a Mac app opened from Finder doesn't have; the usual places if it can't
/// say within a few seconds.
fn login_path() -> String {
    login_shell().and_then(|shell| path_from(&shell, SHELL_WITHIN)).unwrap_or_else(|| FALLBACK_PATH.to_owned())
}

/// The user's shell, from the user database (an app has no $SHELL).
fn login_shell() -> Option<PathBuf> {
    // SAFETY: getpwuid returns a pointer into static storage or null; it's
    // read at once, before anything else could call it.
    unsafe {
        let entry = libc::getpwuid(libc::getuid());
        if entry.is_null() || (*entry).pw_shell.is_null() {
            return None;
        }
        let shell = std::ffi::CStr::from_ptr((*entry).pw_shell).to_str().ok()?;
        (!shell.is_empty()).then(|| PathBuf::from(shell))
    }
}

/// What PATH `shell`, as an interactive login shell, sets, between markers
/// so whatever else its startup files print is left out.
fn path_from(shell: &Path, within: Duration) -> Option<String> {
    use std::os::unix::process::CommandExt;
    const MARK: &str = "__CLAKTORY_PATH__";
    let mut child = Command::new(shell)
        .args(["-l", "-i", "-c", &format!("printf '\\n{MARK}%s{MARK}\\n' \"$PATH\"")])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        // Its own group, so anything its startup files leave behind goes too.
        .process_group(0)
        .spawn()
        .ok()?;
    let mut stdout = child.stdout.take()?;
    let reader = std::thread::spawn(move || {
        let mut text = String::new();
        let _ = stdout.read_to_string(&mut text);
        text
    });
    let deadline = Instant::now() + within;
    let finished = loop {
        match child.try_wait() {
            Ok(Some(_)) => break true,
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(20)),
            _ => break false,
        }
    };
    // SAFETY: kills the shell's own process group, made above.
    unsafe {
        libc::killpg(child.id() as libc::pid_t, libc::SIGKILL);
    }
    let _ = child.wait();
    if !finished {
        return None;
    }
    let text = reader.join().ok()?;
    let path = text.split(MARK).nth(1)?.trim();
    (!path.is_empty() && path.contains('/')).then(|| path.to_owned())
}

/// `factoryd install [--check | --repair | --start]`.
pub fn main(args: &[String]) -> ExitCode {
    let (mut check_only, mut repair, mut start_only) = (false, false, false);
    for arg in args {
        match arg.as_str() {
            "--check" => check_only = true,
            "--repair" => repair = true,
            "--start" => start_only = true,
            "--dev" => {
                eprintln!("The dev Claktory isn't installed this way: scripts/dev.sh runs it.");
                return ExitCode::from(2);
            }
            _ => {
                eprintln!("usage: factoryd install [--check | --repair | --start]");
                return ExitCode::from(2);
            }
        }
    }
    if !cfg!(target_os = "macos") {
        eprintln!("factoryd install sets Claktory up on a Mac; on Linux, use the installer.");
        return ExitCode::from(2);
    }
    let report = match this_mac() {
        Some(setup) if check_only => check(&setup),
        Some(setup) if start_only => start(&setup),
        Some(setup) => install(&setup, repair),
        None => Report::new(Outcome::Failed, "Claktory couldn't find your home folder.", "HOME isn't set"),
    };
    println!("{}", serde_json::to_string(&report).unwrap_or_default());
    ExitCode::from(report.result.exit_code())
}

/// The real Claktory on this Mac, for this user, from the bundle this
/// factoryd is in.
pub(crate) fn this_mac() -> Option<Setup> {
    let user_home = std::env::home_dir()?;
    let instance = crate::home::instance(false, &user_home);
    let job = launch_agent::Job::for_this_user()?;
    let exe = std::env::current_exe().ok()?;
    let helpers = exe.canonicalize().unwrap_or(exe).parent()?.to_owned();
    Some(Setup {
        home: instance.home,
        helpers,
        plist: job.plist,
        label: launch_agent::LABEL.to_owned(),
        launchctl: job.launchctl,
        domain: job.domain,
        codesign: job.codesign,
        lsregister: PathBuf::from(LSREGISTER),
        port: instance.addr.port(),
        answer_within: ANSWER_WITHIN,
        path: None,
        env: Vec::new(),
    })
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn temp() -> PathBuf {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let next = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("factoryd-install-{}-{next}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("dir");
        dir
    }

    fn executable(path: &Path, text: &str) {
        std::fs::create_dir_all(path.parent().expect("parent")).expect("dir");
        std::fs::write(path, text).expect("write");
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).expect("chmod");
    }

    /// A launchctl that logs its calls; `print` says the job's loaded only
    /// when `<dir>/loaded` is there, `print-disabled` says what
    /// `<dir>/disabled` does, and `bootstrap` fails with `refuse`.
    fn fake_launchctl(dir: &Path, refuse: bool) -> PathBuf {
        let path = dir.join("launchctl");
        let bootstrap = if refuse { "echo 'Bootstrap failed: 5: Input/output error' >&2; exit 5" } else { "exit 0" };
        let script = format!(
            "#!/bin/sh\necho \"$@\" >> \"{calls}\"\ncase \"$1\" in\n  print) [ -f \"{dir}/loaded\" ] && exit 0; exit 1 ;;\n  print-disabled) cat \"{dir}/disabled\" 2>/dev/null; exit 0 ;;\n  bootstrap) {bootstrap} ;;\nesac\nexit 0\n",
            calls = dir.join("calls").display(),
            dir = dir.display(),
        );
        executable(&path, &script);
        path
    }

    /// What was asked of launchd, past reading how things are.
    fn changes(dir: &Path) -> Vec<String> {
        calls(dir).lines().filter(|line| !line.starts_with("print")).map(str::to_owned).collect()
    }

    /// A bundle's Helpers, with stand-ins.
    fn helpers(dir: &Path) -> PathBuf {
        let helpers = dir.join("Claktory.app/Contents/Helpers");
        executable(&helpers.join("factoryd"), "the bundle's backend");
        executable(&helpers.join("claktory-apps"), "the bundle's host");
        executable(&helpers.join("tools/rg"), "the bundle's rg");
        executable(&helpers.join("tools/jq"), "the bundle's jq");
        executable(&helpers.join(SUPERVISOR_APP).join(SUPERVISOR_PROGRAM), "the bundle's supervisor");
        let resources = dir.join("Claktory.app/Contents/Resources");
        std::fs::create_dir_all(resources.join("tools/LICENSES/ripgrep")).expect("licenses");
        std::fs::write(resources.join("tools/LICENSES/ripgrep/UNLICENSE"), "ripgrep's license").expect("license");
        std::fs::create_dir_all(resources.join("apps")).expect("apps");
        std::fs::write(resources.join("apps/charts.clapp"), "the bundle's Charts").expect("Charts");
        helpers
    }

    fn setup(dir: &Path, port: u16) -> Setup {
        Setup {
            home: Home::new(dir.join("home")),
            helpers: helpers(dir),
            plist: dir.join("LaunchAgents/dev.factory.test.plist"),
            label: "dev.factory.test".to_owned(),
            launchctl: fake_launchctl(dir, false),
            domain: "gui/501".to_owned(),
            codesign: "/usr/bin/true".into(),
            lsregister: "/usr/bin/true".into(),
            port,
            answer_within: Duration::from_millis(500),
            path: Some("/opt/homebrew/bin:/usr/bin:/bin".to_owned()),
            env: Vec::new(),
        }
    }

    fn calls(dir: &Path) -> String {
        std::fs::read_to_string(dir.join("calls")).unwrap_or_default()
    }

    fn read(path: &Path) -> String {
        std::fs::read_to_string(path).expect("read")
    }

    #[test]
    fn a_port_only_counts_as_another_users_when_something_of_theirs_answers() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
        let port = listener.local_addr().expect("addr").port();
        assert!(!taken_by_another(port), "this user's own listener");
        drop(listener);
        // Another test's listener can land on the freed port for a moment
        // between the check's connect and its lsof: ask again, not once.
        assert!((0..5).any(|_| !taken_by_another(port)), "nothing there");
    }

    #[test]
    fn a_new_mac_gets_the_backend_the_supervisor_and_a_job_that_runs_it() {
        let dir = temp();
        let backend = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
        let setup = setup(&dir, backend.local_addr().expect("addr").port());
        assert_eq!(check(&setup).result, Outcome::NotInstalled);

        // As a download the person opened: flagged, and approved.
        let flag = |path: &Path| {
            Command::new("/usr/bin/xattr").args(["-p", "com.apple.quarantine"]).arg(path).output().expect("xattr").status.success()
        };
        let program = setup.helpers.join(SUPERVISOR_APP).join(SUPERVISOR_PROGRAM);
        let flagged = Command::new("/usr/bin/xattr").args(["-w", "com.apple.quarantine", "01c3;6740a000;Safari;"]).arg(&program).status();
        assert!(flagged.expect("xattr").success() && flag(&program));

        let report = install(&setup, false);
        assert_eq!((report.result, report.running), (Outcome::Installed, Some(true)), "{report:?}");
        let bin = setup.home.root().join("bin");
        assert_eq!(read(&bin.join("factoryd")), "the bundle's backend");
        assert_eq!(read(&bin.join("claktory-apps")), "the bundle's host");
        assert_eq!(read(&bin.join("apps/charts.clapp")), "the bundle's Charts");
        let tools = setup.home.root().join("tools");
        assert_eq!(read(&tools.join("rg")), "the bundle's rg", "every agent's ripgrep, off bin");
        assert_eq!(read(&tools.join("jq")), "the bundle's jq");
        assert_eq!(read(&tools.join("LICENSES/ripgrep/UNLICENSE")), "ripgrep's license");
        let supervisor = launch_agent::supervisors(&setup.home)[0].clone();
        assert_eq!(read(&supervisor), "the bundle's supervisor");
        assert!(!flag(&supervisor), "no quarantine on the installed supervisor");
        let plist = launch_agent::read(&setup.plist).expect("plist");
        assert_eq!(plist["Label"], "dev.factory.test");
        assert_eq!(plist["ProgramArguments"], serde_json::json!([supervisor.to_string_lossy(), "supervise"]));
        assert_eq!(plist["EnvironmentVariables"]["PATH"], "/opt/homebrew/bin:/usr/bin:/bin");
        assert_eq!((plist["ProcessType"].as_str(), plist["KeepAlive"].as_bool()), (Some("Interactive"), Some(true)));
        assert_eq!(plist["AssociatedBundleIdentifiers"], serde_json::json!(["dev.factory.app"]), "Login Items names it Claktory");
        assert_eq!(plist["StandardOutPath"], setup.home.root().join("factoryd.log").to_string_lossy().as_ref());
        assert_eq!(
            calls(&dir),
            format!(
                "bootout gui/501/dev.factory.test\nprint gui/501/dev.factory.test\nenable gui/501/dev.factory.test\nbootstrap gui/501 {}\n",
                setup.plist.display()
            )
        );

        // Now it's there: told so, and nothing is touched again.
        let again = check(&setup);
        assert_eq!((again.result, again.running), (Outcome::AlreadyInstalled, Some(true)));
        assert_eq!(install(&setup, false).result, Outcome::AlreadyInstalled);
        assert_eq!(install(&setup, true).result, Outcome::AlreadyInstalled, "running: nothing to repair");
        assert_eq!(calls(&dir).lines().count(), 4);
        let line = serde_json::to_value(&again).expect("json");
        assert_eq!(line["result"], "already_installed");
        assert_eq!(line["version"], crate::update::VERSION);
    }

    #[test]
    fn an_existing_install_is_left_alone_whatever_its_version() {
        let dir = temp();
        let setup = setup(&dir, 0);
        // A curl install from before the supervisor's app, not running now.
        let bare = launch_agent::supervisors(&setup.home)[1].clone();
        executable(&bare, "an older supervisor");
        executable(&setup.home.backend(), "a newer backend");
        let plist = serde_json::json!({"Label": "dev.factory.test", "ProgramArguments": [bare.to_string_lossy(), "supervise"]});
        std::fs::create_dir_all(setup.plist.parent().expect("dir")).expect("dir");
        std::fs::write(&setup.plist, plist.to_string()).expect("plist");
        let before = std::fs::read(&setup.plist).expect("read");

        let report = install(&setup, false);
        assert_eq!((report.result, report.running), (Outcome::AlreadyInstalled, Some(false)), "{report:?}");
        assert_eq!(read(&setup.home.backend()), "a newer backend");
        assert_eq!(std::fs::read(&setup.plist).expect("read"), before);
        assert!(!launch_agent::supervisors(&setup.home)[0].exists());
        assert_eq!(changes(&dir), Vec::<String>::new(), "launchd untouched");
    }

    #[test]
    fn a_half_done_install_is_reported_and_repair_completes_it_keeping_its_backend() {
        let dir = temp();
        let backend = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
        let setup = setup(&dir, backend.local_addr().expect("addr").port());
        executable(&setup.home.backend(), "the backend already there");

        let report = check(&setup);
        assert_eq!(report.result, Outcome::Incomplete);
        assert_eq!(report.detail, "the LaunchAgent is missing; the supervisor is missing");
        assert_eq!(install(&setup, false).result, Outcome::Incomplete, "only a repair changes it");
        assert!(!setup.plist.exists() && calls(&dir).is_empty());

        let repaired = install(&setup, true);
        assert_eq!(repaired.result, Outcome::Installed, "{repaired:?}");
        assert_eq!(read(&setup.home.backend()), "the backend already there", "never replaced");
        assert_eq!(read(&launch_agent::supervisors(&setup.home)[0]), "the bundle's supervisor");
        assert_eq!(check(&setup).result, Outcome::AlreadyInstalled);
    }

    #[test]
    fn a_job_whose_backend_never_answers_is_set_up_and_says_so() {
        let dir = temp();
        let setup = setup(&dir, 0);
        let report = install(&setup, false);
        assert_eq!(report.result, Outcome::NotRunning, "{report:?}");
        assert!(report.detail.ends_with("factoryd.log"), "{}", report.detail);
        // A repair reloads it.
        assert_eq!(install(&setup, true).result, Outcome::NotRunning);
        assert_eq!(calls(&dir).lines().filter(|line| line.starts_with("bootstrap")).count(), 2);
    }

    /// Friend bug #27: a job the person stopped (disabled, not loaded) reads
    /// as stopped, held when their switch stopped it; `--start` enables it
    /// and loads it, or starts it when it's loaded, and it's no longer held.
    #[test]
    fn a_stopped_claktory_says_so_and_start_enables_and_loads_it() {
        let dir = temp();
        let backend = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
        let port = backend.local_addr().expect("addr").port();
        let setup = setup(&dir, port);
        assert_eq!(start(&setup).result, Outcome::NotInstalled, "nothing to start");
        assert_eq!(install(&setup, false).result, Outcome::Installed);
        drop(backend);
        // Not running and enabled: broken, not stopped.
        let report = check(&setup);
        assert_eq!((report.result, report.running), (Outcome::AlreadyInstalled, Some(false)), "{report:?}");

        std::fs::write(
            dir.join("disabled"),
            "disabled services = {\n\t\"com.apple.x\" => enabled\n\t\"dev.factory.test\" => disabled\n}\n",
        )
        .expect("disabled");
        let marker = crate::service::stopped_marker(&setup.home);
        std::fs::write(&marker, r#"{"by":"switch","time":1}"#).expect("marker");
        let report = check(&setup);
        assert_eq!((report.result, report.held), (Outcome::Stopped, Some(true)), "{report:?}");
        assert_eq!(report.reason, "You turned Claktory off, so your agents aren't working.");
        let line = serde_json::to_value(&report).expect("json");
        assert_eq!((line["result"].as_str(), line["held"].as_bool()), (Some("stopped"), Some(true)));
        std::fs::write(&marker, r#"{"by":"quit","time":1}"#).expect("marker");
        assert_eq!(check(&setup).held, Some(false));
        // Before macOS 13, print-disabled said true.
        std::fs::write(dir.join("disabled"), "\t\"dev.factory.test\" => true\n").expect("disabled");
        assert_eq!(check(&setup).result, Outcome::Stopped);

        // Started: enabled, then loaded, and the backend answers.
        let before = changes(&dir).len();
        // As launchd would: the backend answers once the job's loaded.
        let calls_file = dir.join("calls");
        let starts = std::thread::spawn(move || {
            while std::fs::read_to_string(&calls_file).unwrap_or_default().matches("bootstrap gui/501").count() < 2 {
                std::thread::sleep(Duration::from_millis(10));
            }
            std::net::TcpListener::bind(("127.0.0.1", port)).expect("bind again")
        });
        let report = start(&setup);
        let backend = starts.join().expect("listener");
        assert_eq!((report.result, report.running), (Outcome::Started, Some(true)), "{report:?}");
        assert_eq!(
            changes(&dir)[before..],
            ["enable gui/501/dev.factory.test".to_owned(), format!("bootstrap gui/501 {}", setup.plist.display())]
        );
        assert!(!marker.exists(), "no longer stopped");
        // Running already: left as it is.
        let report = start(&setup);
        assert_eq!((report.result, report.running), (Outcome::AlreadyInstalled, Some(true)));
        assert_eq!(changes(&dir).len(), before + 2);
        drop(backend);

        // Loaded but not running: started where it is, not loaded again.
        std::fs::write(dir.join("loaded"), "").expect("loaded");
        let report = start(&setup);
        assert_eq!(report.result, Outcome::NotRunning, "nothing answers here: {report:?}");
        assert_eq!(changes(&dir)[before + 2..], ["enable gui/501/dev.factory.test", "kickstart gui/501/dev.factory.test"]);
    }

    #[test]
    fn a_start_launchd_refuses_fails_with_what_it_said() {
        let dir = temp();
        let mut setup = setup(&dir, 0);
        assert_eq!(install(&setup, false).result, Outcome::NotRunning);
        setup.launchctl = fake_launchctl(&dir, true);
        let report = start(&setup);
        assert_eq!((report.result, report.reason.as_str()), (Outcome::Failed, "macOS didn't start Claktory's background service."));
        assert!(report.detail.contains("Input/output error"), "{}", report.detail);
    }

    #[test]
    fn a_damaged_bundle_or_a_refused_job_fails_with_why() {
        let dir = temp();
        let mut unsigned = setup(&dir, 0);
        unsigned.codesign = "/usr/bin/false".into();
        let report = install(&unsigned, false);
        assert_eq!(
            (report.result, report.reason.as_str()),
            (Outcome::Damaged, "This copy of Claktory is damaged. Download Claktory again.")
        );
        assert!(!unsigned.home.backend().exists(), "nothing copied");

        let dir = temp();
        let mut refused = setup(&dir, 0);
        refused.launchctl = fake_launchctl(&dir, true);
        let report = install(&refused, false);
        assert_eq!(report.result, Outcome::Failed);
        assert_eq!(report.reason, "macOS didn't start Claktory's background service.");
        assert!(report.detail.contains("Input/output error"), "{}", report.detail);
    }

    /// With launchd itself, under a label, home and port of its own: the
    /// supervisor starts the backend and it answers. Run by hand, with
    /// FACTORY_TEST_BACKEND the path of a built factoryd:
    /// `FACTORY_TEST_BACKEND=$PWD/target/debug/factoryd cargo test -p factoryd --lib install::tests::with_launchd -- --ignored`
    #[test]
    #[ignore]
    fn with_launchd() {
        let Some(built) = std::env::var_os("FACTORY_TEST_BACKEND") else { return };
        let dir = temp();
        let helpers = dir.join("Claktory.app/Contents/Helpers");
        std::fs::create_dir_all(helpers.join(SUPERVISOR_APP).join("Contents/MacOS")).expect("dir");
        std::fs::copy(&built, helpers.join("factoryd")).expect("backend");
        std::fs::copy(&built, helpers.join(SUPERVISOR_APP).join(SUPERVISOR_PROGRAM)).expect("supervisor");
        let label = format!("dev.factory.installtest-{}", std::process::id());
        let home = Home::new(dir.join("home"));
        let port = 7432;
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
            lsregister: "/usr/bin/true".into(),
            port,
            answer_within: Duration::from_secs(60),
            path: None,
            env: vec![("FACTORY_HOME".into(), home.root().to_string_lossy().into_owned()), ("FACTORY_PORT".into(), port.to_string())],
        };
        let report = install(&setup, false);
        let again = install(&setup, false);
        let _ = Command::new("/bin/launchctl").args(["bootout", &format!("{domain}/{label}")]).status();
        assert_eq!(report.result, Outcome::Installed, "{report:?}");
        assert_eq!((again.result, again.running), (Outcome::AlreadyInstalled, Some(true)));
        let path = launch_agent::read(&setup.plist).expect("plist")["EnvironmentVariables"]["PATH"].as_str().unwrap_or_default().to_owned();
        assert!(path.contains("/usr/bin"), "{path}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Friend bug #27 with launchd itself, under a label, home and port of
    /// its own (QA's FACTORY_SERVICE_LABEL, so its plist goes in
    /// ~/Library/LaunchAgents for the test's length): stop mode keeps the
    /// job disabled while it runs; StopEngine from the app stops it, and it
    /// stays stopped; `--start` starts it. Run by hand, as `with_launchd`:
    /// `FACTORY_TEST_BACKEND=$PWD/target/debug/factoryd cargo test -p factoryd --lib install::tests::stop_and_start_with_launchd -- --ignored --nocapture`
    #[tokio::test(flavor = "multi_thread")]
    #[ignore]
    async fn stop_and_start_with_launchd() {
        use factory_protocol::v1::{self, client_message::Body as Request, server_message::Body};
        use futures_util::{SinkExt, StreamExt};
        use prost::Message as _;
        let Some(built) = std::env::var_os("FACTORY_TEST_BACKEND") else { return };
        let dir = temp();
        let helpers = dir.join("Claktory.app/Contents/Helpers");
        std::fs::create_dir_all(helpers.join(SUPERVISOR_APP).join("Contents/MacOS")).expect("dir");
        std::fs::copy(&built, helpers.join("factoryd")).expect("backend");
        std::fs::copy(&built, helpers.join(SUPERVISOR_APP).join(SUPERVISOR_PROGRAM)).expect("supervisor");
        let label = format!("dev.factory.stoptest-{}", std::process::id());
        let home = Home::new(dir.join("home"));
        let port = 7435;
        let domain = format!("gui/{}", unsafe { libc::getuid() });
        let service = format!("{domain}/{label}");
        let agents = std::env::home_dir().expect("home").join("Library/LaunchAgents");
        let setup = Setup {
            home: home.clone(),
            helpers,
            plist: agents.join(format!("{label}.plist")),
            label: label.clone(),
            launchctl: "/bin/launchctl".into(),
            domain: domain.clone(),
            codesign: "/usr/bin/true".into(),
            lsregister: "/usr/bin/true".into(),
            port,
            answer_within: Duration::from_secs(60),
            path: None,
            env: vec![
                ("FACTORY_HOME".into(), home.root().to_string_lossy().into_owned()),
                ("FACTORY_PORT".into(), port.to_string()),
                ("FACTORY_SERVICE_LABEL".into(), label.clone()),
            ],
        };
        let disabled = || {
            let out = Command::new("/bin/launchctl").args(["print-disabled", &domain]).output().expect("launchctl").stdout;
            String::from_utf8_lossy(&out).lines().any(|line| line.contains(&format!("\"{label}\" => disabled")))
        };
        let loaded = || Command::new("/bin/launchctl").args(["print", &service]).output().is_ok_and(|out| out.status.success());
        // The app's request, over the loopback port with this home's token.
        let ask = |request: Request| {
            let home = home.clone();
            async move {
                let pairing = home.root().join("remote/local-pairing");
                for _ in 0..100 {
                    if pairing.exists() {
                        break;
                    }
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
                let code = std::fs::read_to_string(&pairing).expect("pairing");
                let token = code.trim().split(['?', '&']).find_map(|part| part.strip_prefix("t=")).expect("token").to_owned();
                let stream = tokio::net::TcpStream::connect(("127.0.0.1", port)).await.expect("connect");
                let (mut socket, _) = tokio_tungstenite::client_async(format!("ws://127.0.0.1:{port}"), stream).await.expect("ws");
                let hello = v1::Hello { protocol_version: factory_protocol::PROTOCOL_VERSION, token, ..Default::default() };
                for (id, body) in [(1, Request::Hello(hello)), (2, request)] {
                    let message = v1::ClientMessage { id, body: Some(body) };
                    socket.send(tokio_tungstenite::tungstenite::Message::Binary(message.encode_to_vec().into())).await.expect("send");
                }
                loop {
                    let frame = socket.next().await.expect("open").expect("frame");
                    if let tokio_tungstenite::tungstenite::Message::Binary(bytes) = frame {
                        let message = v1::ServerMessage::decode(bytes).expect("decode");
                        if message.reply_to == 2 {
                            return message.body.expect("body");
                        }
                    }
                }
            }
        };
        let until = |what: &str, done: &dyn Fn() -> bool| {
            let deadline = Instant::now() + Duration::from_secs(60);
            while !done() {
                assert!(Instant::now() < deadline, "never: {what}");
                std::thread::sleep(Duration::from_millis(200));
            }
        };
        let finish = || {
            let _ = Command::new("/bin/launchctl").args(["bootout", &service]).status();
            let _ = Command::new("/bin/launchctl").args(["enable", &service]).status();
            let _ = std::fs::remove_file(agents.join(format!("{label}.plist")));
        };

        let result = std::panic::AssertUnwindSafe(async {
            assert_eq!(install(&setup, false).result, Outcome::Installed);
            assert!(!disabled(), "keep mode: enabled");
            let mode = |mode: v1::QuitMode| {
                Request::UpdateSettings(v1::UpdateSettings { when_app_quits: Some(mode.into()), ..Default::default() })
            };
            assert!(matches!(ask(mode(v1::QuitMode::Stop)).await, Body::Ack(_)));
            until("stop mode disables the running job", &|| disabled());
            assert!(loaded() && answers(port, Duration::from_millis(300)), "and it keeps running");

            // The app quits: stopped, and stays stopped.
            assert!(matches!(ask(Request::StopEngine(v1::StopEngine { quitting: true })).await, Body::Ack(_)));
            until("stopped", &|| !loaded() && !answers(port, Duration::from_millis(300)));
            std::thread::sleep(Duration::from_secs(3));
            assert!(!answers(port, Duration::from_millis(300)), "KeepAlive didn't bring it back");
            let report = check(&setup);
            assert_eq!((report.result, report.held), (Outcome::Stopped, Some(false)), "{report:?}");

            // The app opens: started, and still disabled in stop mode.
            let report = start(&setup);
            assert_eq!(report.result, Outcome::Started, "{report:?}");
            until("disabled again as it starts", &|| disabled());

            // The person's switch: held.
            assert!(matches!(ask(Request::StopEngine(v1::StopEngine { quitting: false })).await, Body::Ack(_)));
            until("stopped by the switch", &|| !loaded() && !answers(port, Duration::from_millis(300)));
            assert_eq!(check(&setup).held, Some(true));
            assert_eq!(start(&setup).result, Outcome::Started);

            // Keep mode: enabled, and quitting the app leaves it running.
            assert!(matches!(ask(mode(v1::QuitMode::KeepRunning)).await, Body::Ack(_)));
            until("keep mode enables it", &|| !disabled());
            assert!(matches!(ask(Request::StopEngine(v1::StopEngine { quitting: true })).await, Body::Ack(_)));
            std::thread::sleep(Duration::from_secs(3));
            assert!(answers(port, Duration::from_millis(300)), "still running");
        });
        let outcome = futures_util::FutureExt::catch_unwind(result).await;
        finish();
        if let Err(panic) = outcome {
            eprintln!(
                "log: {}",
                std::fs::read_to_string(home.root().join("factoryd.log"))
                    .unwrap_or_default()
                    .lines()
                    .rev()
                    .take(30)
                    .collect::<Vec<_>>()
                    .join("\n")
            );
            std::panic::resume_unwind(panic);
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_path_comes_from_the_login_shell_and_a_shell_that_hangs_is_given_up_on() {
        let path = path_from(Path::new("/bin/sh"), Duration::from_secs(5)).expect("a PATH");
        assert!(path.contains("/usr/bin"), "{path}");
        // Startup files that print, and then a PATH.
        let dir = temp();
        let chatty = dir.join("chatty");
        executable(&chatty, "#!/bin/sh\necho 'Welcome!'\nPATH=/chatty/bin:/usr/bin\nexport PATH\nexec /bin/sh -c \"$4\"\n");
        assert_eq!(path_from(&chatty, Duration::from_secs(5)).as_deref(), Some("/chatty/bin:/usr/bin"));
        let hangs = dir.join("hangs");
        executable(&hangs, "#!/bin/sh\nsleep 30\n");
        let started = Instant::now();
        assert_eq!(path_from(&hangs, Duration::from_millis(300)), None);
        assert!(started.elapsed() < Duration::from_secs(5));
    }
}
