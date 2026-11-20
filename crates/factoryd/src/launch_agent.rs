//! Brings the LaunchAgent that runs Factory's supervisor on a Mac up to date
//! after an update, once. scripts/install-release.sh writes it at install
//! time, and an update never runs the installer, so what a newer installer
//! writes (`ProcessType Interactive`: without it launchd throttles Factory and
//! everything agents run) reaches older installs only here.
//!
//! The backend checks the plist once it's past any update's trial. Only a
//! plist that runs this Factory's own supervisor counts (the dev Factory isn't
//! a launchd job, so there it does nothing). The new plist is made beside it
//! and checked with `plutil`, a marker says it was tried (so it's tried once,
//! whatever happens), and a helper in its own session, which outlives the job,
//! swaps it in and reloads the job: `launchctl bootout`, then `bootstrap`. That
//! restarts the supervisor and backend once, as an update does. If the new
//! plist won't load, the old one goes back and is loaded again.
//!
//! The supervisor itself is updated here too, since an update replaces only
//! the backend: when the release the backend came from carries the
//! supervisor's app (`supervisor/Claktory Agents.app`) and the job runs
//! another supervisor (the bare `factoryd-supervisor` installs had before
//! the app, or an older app), the release's app goes in, checked with
//! `codesign`, and the plist runs it. Tried once per version. The helper
//! keeps the old app and plist until the backend answers on its port again
//! under the new supervisor, and puts them back if it doesn't within 30 s.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use serde_json::Value;

use crate::home::Home;

pub(crate) const LABEL: &str = "dev.factory.factoryd";

/// What the plist must say, as installers write it now.
const WANTED: [(&str, &str); 1] = [("ProcessType", "Interactive")];

/// Where the job lives and how to reach launchd.
pub(crate) struct Job {
    pub(crate) plist: PathBuf,
    pub(crate) launchctl: PathBuf,
    /// `gui/<uid>`.
    pub(crate) domain: String,
    /// Checks a supervisor app's signature.
    pub(crate) codesign: PathBuf,
}

impl Job {
    fn for_this_user() -> Option<Job> {
        let user_home = std::env::var_os("HOME")?;
        // SAFETY: getuid has no preconditions.
        let uid = unsafe { libc::getuid() };
        Some(Job {
            plist: Path::new(&user_home).join("Library/LaunchAgents").join(format!("{LABEL}.plist")),
            launchctl: PathBuf::from("/bin/launchctl"),
            domain: format!("gui/{uid}"),
            codesign: PathBuf::from("/usr/bin/codesign"),
        })
    }
}

/// Where scripts/install-release.sh puts the supervisor: in its app (an app,
/// so it can be added to Developer Tools), or, before that, bare.
pub(crate) fn supervisors(home: &Home) -> [PathBuf; 2] {
    let bin = home.root().join("bin");
    [bin.join("Claktory Agents.app/Contents/MacOS/factoryd-supervisor"), bin.join("factoryd-supervisor")]
}

/// Where the marker and the helper's files go.
fn marker(home: &Home) -> PathBuf {
    home.root().join("bin/launch-agent.json")
}

fn plutil(args: &[&std::ffi::OsStr]) -> Result<Vec<u8>, String> {
    let output = Command::new("/usr/bin/plutil").args(args).output().map_err(|error| format!("Can't run plutil: {error}"))?;
    if output.status.success() { Ok(output.stdout) } else { Err(format!("plutil: {}", String::from_utf8_lossy(&output.stderr).trim())) }
}

/// The plist as JSON.
fn read(plist: &Path) -> Result<Value, String> {
    let json = plutil(&["-convert".as_ref(), "json".as_ref(), "-o".as_ref(), "-".as_ref(), plist.as_os_str()])?;
    serde_json::from_slice(&json).map_err(|error| error.to_string())
}

/// Makes the up-to-date plist beside the job's and marks the update tried;
/// Ok(None) when there's nothing to do.
pub(crate) fn prepare(home: &Home, job: &Job) -> Result<Option<PathBuf>, String> {
    if !job.plist.is_file() || marker(home).exists() {
        return Ok(None);
    }
    let current = read(&job.plist)?;
    // Only the job that runs this Factory's own supervisor: its app, or
    // the bare binary installers put there before.
    let program = current["ProgramArguments"][0].as_str().unwrap_or_default();
    let ours = supervisors(home).iter().any(|supervisor| supervisor.to_string_lossy() == program);
    if current["Label"] != LABEL || !ours {
        return Ok(None);
    }
    let missing: Vec<(&str, &str)> = WANTED.into_iter().filter(|(key, value)| current[*key].as_str() != Some(*value)).collect();
    if missing.is_empty() {
        return Ok(None);
    }
    let new = home.root().join("bin/launch-agent.plist");
    std::fs::copy(&job.plist, &new).map_err(|error| format!("Can't copy {}: {error}", job.plist.display()))?;
    for (key, value) in &missing {
        plutil(&["-replace".as_ref(), key.as_ref(), "-string".as_ref(), value.as_ref(), new.as_os_str()])?;
    }
    plutil(&["-lint".as_ref(), "-s".as_ref(), new.as_os_str()])?;
    let keys: Vec<&str> = missing.iter().map(|(key, _)| *key).collect();
    let record = serde_json::json!({"version": crate::update::VERSION, "time": crate::transcript::now_ms(), "added": keys});
    std::fs::write(marker(home), record.to_string()).map_err(|error| format!("Can't write {}: {error}", marker(home).display()))?;
    Ok(Some(new))
}

/// The supervisor's app as installers put it, and its program.
fn supervisor_app(home: &Home) -> PathBuf {
    home.root().join("bin/Claktory Agents.app")
}

/// The supervisor's app in the release this backend came from.
fn release_supervisor(home: &Home) -> PathBuf {
    home.releases().join(crate::update::VERSION).join("supervisor/Claktory Agents.app")
}

/// Records which version's supervisor was tried, so each is tried once.
fn supervisor_marker(home: &Home) -> PathBuf {
    home.root().join("bin/supervisor.json")
}

/// What `prepare_supervisor` made ready: the plist that runs the new
/// supervisor, and its app, staged beside the installed one.
#[derive(Debug, PartialEq)]
pub(crate) struct NewSupervisor {
    pub(crate) plist: PathBuf,
    pub(crate) staged: PathBuf,
}

fn same_file(a: &Path, b: &Path) -> bool {
    match (std::fs::read(a), std::fs::read(b)) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    }
}

/// Stages the release's supervisor app and a plist that runs it, when the
/// job runs another supervisor; Ok(None) when there's nothing to do.
pub(crate) fn prepare_supervisor(home: &Home, job: &Job) -> Result<Option<NewSupervisor>, String> {
    let release = release_supervisor(home);
    let program_in = |app: &Path| app.join("Contents/MacOS/factoryd-supervisor");
    if !job.plist.is_file() || !program_in(&release).is_file() {
        return Ok(None);
    }
    let tried = std::fs::read_to_string(supervisor_marker(home)).ok().and_then(|text| serde_json::from_str::<Value>(&text).ok());
    if tried.is_some_and(|tried| tried["version"] == crate::update::VERSION) {
        return Ok(None);
    }
    let current = read(&job.plist)?;
    let program = current["ProgramArguments"][0].as_str().unwrap_or_default();
    let ours = supervisors(home).iter().any(|supervisor| supervisor.to_string_lossy() == program);
    if current["Label"] != LABEL || !ours {
        return Ok(None);
    }
    let installed = supervisors(home)[0].clone();
    if program == installed.to_string_lossy() && same_file(&installed, &program_in(&release)) {
        return Ok(None);
    }
    let record = serde_json::json!({"version": crate::update::VERSION, "time": crate::transcript::now_ms(), "from": program});
    std::fs::write(supervisor_marker(home), record.to_string())
        .map_err(|error| format!("Can't write {}: {error}", supervisor_marker(home).display()))?;
    let staged = home.root().join("bin/Claktory Agents.app.new");
    let _ = std::fs::remove_dir_all(&staged);
    let copied = Command::new("/usr/bin/ditto").arg(&release).arg(&staged).status().map_err(|error| format!("Can't run ditto: {error}"))?;
    if !copied.success() {
        return Err(format!("ditto couldn't copy {}", release.display()));
    }
    let signed = Command::new(&job.codesign)
        .args(["--verify", "--deep", "--strict"])
        .arg(&staged)
        .output()
        .map_err(|error| format!("Can't run codesign: {error}"))?;
    if !signed.status.success() {
        let _ = std::fs::remove_dir_all(&staged);
        return Err(format!("{} isn't signed as it should be: {}", release.display(), String::from_utf8_lossy(&signed.stderr).trim()));
    }
    let plist = home.root().join("bin/launch-agent.plist");
    std::fs::copy(&job.plist, &plist).map_err(|error| format!("Can't copy {}: {error}", job.plist.display()))?;
    let mut arguments = current["ProgramArguments"].as_array().cloned().unwrap_or_default();
    arguments[0] = Value::String(installed.to_string_lossy().into_owned());
    let arguments = Value::Array(arguments).to_string();
    plutil(&["-replace".as_ref(), "ProgramArguments".as_ref(), "-json".as_ref(), arguments.as_ref(), plist.as_os_str()])?;
    for (key, value) in WANTED {
        plutil(&["-replace".as_ref(), key.as_ref(), "-string".as_ref(), value.as_ref(), plist.as_os_str()])?;
    }
    plutil(&["-lint".as_ref(), "-s".as_ref(), plist.as_os_str()])?;
    Ok(Some(NewSupervisor { plist, staged }))
}

/// Puts the new supervisor's app and plist in and reloads the job; keeps
/// them if the backend answers on its port within 30 s, and otherwise puts
/// the old ones back. In a session of its own, like RELOAD.
const SWITCH: &str = r#"
lc="$1" domain="$2" label="$3" plist="$4" new="$5" app="$6" staged="$7" port="$8" bare="$9" tries="${10:-300}"
sleep 1
stopped() {
  i=0
  while "$lc" print "$domain/$label" >/dev/null 2>&1 && [ $i -lt 250 ]; do sleep 0.1; i=$((i + 1)); done
}
cp -p "$plist" "$plist.previous" || exit 1
"$lc" bootout "$domain/$label"
stopped
rm -rf "$app.previous"
if [ -d "$app" ]; then mv "$app" "$app.previous" || exit 1; fi
if ! { mv "$staged" "$app" && cp "$new" "$plist.next" && mv -f "$plist.next" "$plist"; }; then
  echo "the new supervisor couldn't be put in; going back"
elif "$lc" bootstrap "$domain" "$plist"; then
  i=0
  while [ $i -lt "$tries" ]; do
    if /usr/bin/nc -z 127.0.0.1 "$port" >/dev/null 2>&1; then
      rm -rf "$app.previous"
      rm -f "$plist.previous" "$new" "$bare"
      echo "switched $label to the supervisor in $app"
      exit 0
    fi
    sleep 0.1
    i=$((i + 1))
  done
  echo "the backend didn't answer under the new supervisor; going back"
  "$lc" bootout "$domain/$label"
  stopped
else
  echo "the new supervisor's plist didn't load; going back"
fi
rm -rf "$app"
if [ -d "$app.previous" ]; then mv "$app.previous" "$app"; fi
mv -f "$plist.previous" "$plist"
"$lc" bootstrap "$domain" "$plist"
"#;

pub(crate) fn switch_supervisor(home: &Home, job: &Job, new: &NewSupervisor, port: u16) -> std::io::Result<()> {
    switch_within(home, job, new, port, 300)
}

/// As `switch_supervisor`, giving the backend `tries` tenths of a second.
fn switch_within(home: &Home, job: &Job, new: &NewSupervisor, port: u16, tries: u32) -> std::io::Result<()> {
    let bare = &supervisors(home)[1];
    let mut command = helper(home, SWITCH)?;
    command
        .arg(&job.launchctl)
        .arg(&job.domain)
        .arg(LABEL)
        .arg(&job.plist)
        .arg(&new.plist)
        .arg(supervisor_app(home))
        .arg(&new.staged)
        .arg(port.to_string())
        .arg(bare)
        .arg(tries.to_string());
    command.spawn().map(drop)
}

/// A shell running `script` in a session of its own (which `bootout`
/// doesn't stop), its output going to `<home>/bin/launch-agent.log`.
fn helper(home: &Home, script: &str) -> std::io::Result<Command> {
    use std::os::unix::process::CommandExt;
    let log = std::fs::OpenOptions::new().create(true).append(true).open(home.root().join("bin/launch-agent.log"))?;
    let mut command = Command::new("/bin/sh");
    command.args(["-c", script, "sh"]).stdin(std::process::Stdio::null()).stdout(log.try_clone()?).stderr(log);
    // SAFETY: setsid is async-signal-safe and touches nothing else.
    unsafe {
        command.pre_exec(|| if libc::setsid() < 0 { Err(std::io::Error::last_os_error()) } else { Ok(()) });
    }
    Ok(command)
}

/// Swaps the new plist in and reloads the job, from a helper in a session of
/// its own, which `bootout` doesn't stop. Its output goes to
/// `<home>/bin/launch-agent.log`.
const RELOAD: &str = r#"
lc="$1" domain="$2" label="$3" plist="$4" new="$5"
sleep 1
cp -p "$plist" "$plist.previous" || exit 1
cp "$new" "$plist.next" && mv -f "$plist.next" "$plist" || { rm -f "$plist.next"; exit 1; }
"$lc" bootout "$domain/$label"
i=0
while "$lc" print "$domain/$label" >/dev/null 2>&1 && [ $i -lt 100 ]; do sleep 0.1; i=$((i + 1)); done
if "$lc" bootstrap "$domain" "$plist"; then
  rm -f "$plist.previous" "$new"
  echo "reloaded $label with the updated plist"
else
  echo "the updated plist didn't load; going back to the previous one"
  mv -f "$plist.previous" "$plist"
  "$lc" bootstrap "$domain" "$plist"
fi
"#;

pub(crate) fn reload(home: &Home, job: &Job, new: &Path) -> std::io::Result<()> {
    let mut command = helper(home, RELOAD)?;
    command.arg(&job.launchctl).arg(&job.domain).arg(LABEL).arg(&job.plist).arg(new);
    command.spawn().map(drop)
}

/// Runs once the backend is up: waits out an update's trial (so a reload
/// never stands in for a failed update), then moves the job to the
/// release's supervisor, or updates its plist, if it needs it. `port`: the
/// backend's, which the new supervisor's backend must answer on.
pub async fn update(home: Home, port: u16) {
    if !cfg!(target_os = "macos") || std::env::var_os(crate::update::SUPERVISED).is_none() {
        return;
    }
    let Some(job) = Job::for_this_user() else { return };
    loop {
        tokio::time::sleep(Duration::from_secs(30)).await;
        if !home.trial_path().exists() {
            break;
        }
    }
    let supervisor = {
        let home = home.clone();
        tokio::task::spawn_blocking(move || {
            let job = Job::for_this_user()?;
            Some(prepare_supervisor(&home, &job))
        })
        .await
    };
    match supervisor {
        Ok(Some(Ok(Some(new)))) => {
            tracing::info!(plist = %job.plist.display(), "moving Factory's LaunchAgent to the release's supervisor; the backend restarts once");
            if let Err(error) = switch_supervisor(&home, &job, &new, port) {
                tracing::error!(%error, "could not move Factory's LaunchAgent to the new supervisor");
            }
            return;
        }
        Ok(Some(Err(error))) => tracing::error!(%error, "could not prepare the release's supervisor"),
        _ => {}
    }
    let prepared = {
        let home = home.clone();
        tokio::task::spawn_blocking(move || {
            let job = Job::for_this_user()?;
            Some(prepare(&home, &job))
        })
        .await
    };
    match prepared {
        Ok(Some(Ok(Some(new)))) => {
            tracing::info!(plist = %job.plist.display(), "updating Factory's LaunchAgent; the backend restarts once");
            if let Err(error) = reload(&home, &job, &new) {
                tracing::error!(%error, "could not reload Factory's LaunchAgent");
            }
        }
        Ok(Some(Err(error))) => tracing::error!(%error, "could not update Factory's LaunchAgent"),
        _ => {}
    }
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;

    fn temp() -> PathBuf {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let next = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("factoryd-launch-agent-{}-{next}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("home/bin")).expect("dir");
        dir
    }

    /// A plist as the installer wrote it before ProcessType.
    fn old_plist(dir: &Path, supervisor: &Path) -> PathBuf {
        let plist = dir.join(format!("{LABEL}.plist"));
        let text = format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key><string>{LABEL}</string>
  <key>ProgramArguments</key><array><string>{}</string><string>supervise</string></array>
  <key>EnvironmentVariables</key><dict><key>PATH</key><string>/opt/homebrew/bin:/usr/bin:/bin</string></dict>
  <key>RunAtLoad</key><true/>
  <key>KeepAlive</key><true/>
</dict>
</plist>
"#,
            supervisor.display()
        );
        std::fs::write(&plist, text).expect("plist");
        plist
    }

    /// A launchctl that logs its calls, and fails `bootstrap` of a plist with
    /// ProcessType when `refuse_new`.
    fn fake_launchctl(dir: &Path, refuse_new: bool) -> PathBuf {
        let path = dir.join("launchctl");
        let script = format!(
            r#"#!/bin/sh
echo "$@" >> "{log}"
case "$1" in
  print) exit 1 ;;
  bootstrap) if {refuse} && /usr/bin/plutil -extract ProcessType raw "$3" >/dev/null 2>&1; then exit 5; fi ;;
esac
exit 0
"#,
            log = dir.join("calls").display(),
            refuse = refuse_new
        );
        std::fs::write(&path, script).expect("launchctl");
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).expect("chmod");
        path
    }

    fn wait_for(path: &Path, text: &str) -> String {
        wait_until(path, |found| found.contains(text))
    }

    fn wait_until(path: &Path, done: impl Fn(&str) -> bool) -> String {
        let mut found = String::new();
        for _ in 0..600 {
            found = std::fs::read_to_string(path).unwrap_or_default();
            if done(&found) {
                return found;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        panic!("{} never got there; it said:\n{found}", path.display());
    }

    #[test]
    fn an_old_plist_gets_process_type_and_the_job_is_reloaded_once() {
        let dir = temp();
        let home = Home::new(dir.join("home"));
        let plist = old_plist(&dir, &home.root().join("bin/factoryd-supervisor"));
        let job = Job {
            plist: plist.clone(),
            launchctl: fake_launchctl(&dir, false),
            domain: "gui/501".into(),
            codesign: PathBuf::from("/usr/bin/true"),
        };

        let new = prepare(&home, &job).expect("prepared").expect("something to do");
        reload(&home, &job, &new).expect("reload");
        let calls = wait_for(&dir.join("calls"), "bootstrap");
        assert_eq!(calls, format!("bootout gui/501/{LABEL}\nprint gui/501/{LABEL}\nbootstrap gui/501 {}\n", plist.display()));
        wait_for(&home.root().join("bin/launch-agent.log"), "reloaded");
        let updated = read(&plist).expect("read");
        assert_eq!(updated["ProcessType"], "Interactive");
        assert_eq!(updated["EnvironmentVariables"]["PATH"], "/opt/homebrew/bin:/usr/bin:/bin", "the rest is kept");
        assert!(!dir.join(format!("{LABEL}.plist.previous")).exists());

        // Once: tried, it's never tried again, whatever the plist says.
        old_plist(&dir, &home.root().join("bin/factoryd-supervisor"));
        assert_eq!(prepare(&home, &job).expect("again"), None);
    }

    #[test]
    fn a_plist_that_runs_the_supervisors_app_is_ours_too() {
        let dir = temp();
        let home = Home::new(dir.join("home"));
        let plist = old_plist(&dir, &supervisors(&home)[0]);
        let job =
            Job { plist, launchctl: PathBuf::from("/nonexistent"), domain: "gui/501".into(), codesign: PathBuf::from("/usr/bin/true") };
        let new = prepare(&home, &job).expect("prepared").expect("something to do");
        assert_eq!(read(&new).expect("read")["ProgramArguments"][0], supervisors(&home)[0].to_string_lossy().as_ref());
    }

    #[test]
    fn a_plist_that_wont_load_is_put_back() {
        let dir = temp();
        let home = Home::new(dir.join("home"));
        let plist = old_plist(&dir, &home.root().join("bin/factoryd-supervisor"));
        let before = std::fs::read(&plist).expect("read");
        let job = Job {
            plist: plist.clone(),
            launchctl: fake_launchctl(&dir, true),
            domain: "gui/501".into(),
            codesign: PathBuf::from("/usr/bin/true"),
        };
        let new = prepare(&home, &job).expect("prepared").expect("something to do");
        reload(&home, &job, &new).expect("reload");
        wait_for(&home.root().join("bin/launch-agent.log"), "going back");
        let calls = wait_for(&dir.join("calls"), &format!("bootstrap gui/501 {}\nbootstrap", plist.display()));
        assert_eq!(calls.lines().filter(|line| line.starts_with("bootstrap")).count(), 2, "{calls}");
        // The second bootstrap ran after the old plist was back.
        std::thread::sleep(Duration::from_millis(100));
        assert_eq!(std::fs::read(&plist).expect("read"), before);
    }

    /// A release with the supervisor's app (a stand-in) for this version.
    fn release_app(home: &Home, program: &str) -> PathBuf {
        let app = release_supervisor(home);
        std::fs::create_dir_all(app.join("Contents/MacOS")).expect("dir");
        std::fs::write(app.join("Contents/MacOS/factoryd-supervisor"), program).expect("program");
        app
    }

    /// Something answering on a port, as a backend does.
    fn answering() -> (std::net::TcpListener, u16) {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
        let port = listener.local_addr().expect("addr").port();
        (listener, port)
    }

    #[test]
    fn a_bare_supervisor_moves_to_the_releases_app_once_the_backend_answers_under_it() {
        let dir = temp();
        let home = Home::new(dir.join("home"));
        let bare = supervisors(&home)[1].clone();
        std::fs::write(&bare, "old").expect("bare");
        let plist = old_plist(&dir, &bare);
        release_app(&home, "new");
        let job = Job {
            plist: plist.clone(),
            launchctl: fake_launchctl(&dir, false),
            domain: "gui/501".into(),
            codesign: "/usr/bin/true".into(),
        };

        let new = prepare_supervisor(&home, &job).expect("prepared").expect("something to do");
        let (_backend, port) = answering();
        switch_within(&home, &job, &new, port, 50).expect("switch");
        wait_for(&home.root().join("bin/launch-agent.log"), "switched");
        let updated = read(&plist).expect("read");
        assert_eq!(updated["ProgramArguments"][0], supervisors(&home)[0].to_string_lossy().as_ref());
        assert_eq!(updated["ProgramArguments"][1], "supervise");
        assert_eq!(updated["ProcessType"], "Interactive");
        assert_eq!(std::fs::read_to_string(&supervisors(&home)[0]).expect("installed"), "new");
        assert!(!bare.exists(), "the bare supervisor goes once the app runs");
        assert!(!dir.join(format!("{LABEL}.plist.previous")).exists());
        let calls = std::fs::read_to_string(dir.join("calls")).expect("calls");
        assert_eq!(calls.lines().filter(|line| line.starts_with("bootstrap")).count(), 1, "{calls}");

        // Once per version, and nothing to do once it runs the release's app.
        assert_eq!(prepare_supervisor(&home, &job).expect("again"), None);
        std::fs::remove_file(supervisor_marker(&home)).expect("forget");
        assert_eq!(prepare_supervisor(&home, &job).expect("up to date"), None);
    }

    #[test]
    fn a_new_supervisor_whose_backend_doesnt_answer_is_undone() {
        let dir = temp();
        let home = Home::new(dir.join("home"));
        // An older app installed, run by the plist.
        let installed = supervisors(&home)[0].clone();
        std::fs::create_dir_all(installed.parent().expect("parent")).expect("dir");
        std::fs::write(&installed, "older").expect("older");
        let plist = old_plist(&dir, &installed);
        let before = std::fs::read(&plist).expect("read");
        release_app(&home, "newer");
        let job = Job {
            plist: plist.clone(),
            launchctl: fake_launchctl(&dir, false),
            domain: "gui/501".into(),
            codesign: "/usr/bin/true".into(),
        };

        let new = prepare_supervisor(&home, &job).expect("prepared").expect("a newer app");
        // Port 0: nothing ever answers there, whatever else is running.
        switch_within(&home, &job, &new, 0, 5).expect("switch");
        wait_for(&home.root().join("bin/launch-agent.log"), "going back");
        // The new plist loaded, then unloaded, then the old one loaded again
        // (with launchctl prints, waiting for it to stop, in between).
        let bootstraps = |calls: &str| calls.lines().filter(|line| line.starts_with("bootstrap")).count();
        let calls = wait_until(&dir.join("calls"), |calls| bootstraps(calls) == 2 && calls.ends_with('\n'));
        let order: Vec<&str> = calls.lines().filter_map(|line| line.split(' ').next()).filter(|call| *call != "print").collect();
        assert_eq!(order, ["bootout", "bootstrap", "bootout", "bootstrap"], "{calls}");
        std::thread::sleep(Duration::from_millis(100));
        assert_eq!(std::fs::read(&plist).expect("read"), before, "the old plist is back");
        assert_eq!(std::fs::read_to_string(&installed).expect("installed"), "older", "and the old app");
    }

    #[test]
    fn a_supervisor_app_that_isnt_signed_isnt_installed() {
        let dir = temp();
        let home = Home::new(dir.join("home"));
        let plist = old_plist(&dir, &supervisors(&home)[1]);
        let before = std::fs::read(&plist).expect("read");
        release_app(&home, "unsigned");
        let job = Job {
            plist: plist.clone(),
            launchctl: PathBuf::from("/nonexistent"),
            domain: "gui/501".into(),
            codesign: "/usr/bin/false".into(),
        };
        let refused = prepare_supervisor(&home, &job).expect_err("refused");
        assert!(refused.contains("isn't signed as it should be"), "{refused}");
        assert!(!home.root().join("bin/Claktory Agents.app.new").exists());
        assert_eq!(std::fs::read(&plist).expect("read"), before);
        assert_eq!(prepare_supervisor(&home, &job).expect("tried"), None, "tried once for this version");
    }

    #[test]
    fn nothing_is_done_for_another_factorys_job_or_an_up_to_date_one() {
        let dir = temp();
        let home = Home::new(dir.join("home"));
        let job = |plist: PathBuf| Job {
            plist,
            launchctl: PathBuf::from("/nonexistent"),
            domain: "gui/501".into(),
            codesign: PathBuf::from("/usr/bin/true"),
        };
        // The dev Factory has no plist of its own; one running another home's supervisor isn't its.
        let other = old_plist(&dir, Path::new("/somewhere/else/bin/factoryd-supervisor"));
        assert_eq!(prepare(&home, &job(other.clone())).expect("other"), None);
        assert_eq!(prepare(&home, &job(dir.join("missing.plist"))).expect("missing"), None);
        let current = old_plist(&dir, &home.root().join("bin/factoryd-supervisor"));
        plutil(&["-replace".as_ref(), "ProcessType".as_ref(), "-string".as_ref(), "Interactive".as_ref(), current.as_os_str()])
            .expect("edit");
        assert_eq!(prepare(&home, &job(current)).expect("current"), None);
        assert!(!marker(&home).exists(), "nothing tried");
    }
}
