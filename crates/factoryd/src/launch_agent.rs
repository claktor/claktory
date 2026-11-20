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
    use std::os::unix::process::CommandExt;
    let log = std::fs::OpenOptions::new().create(true).append(true).open(home.root().join("bin/launch-agent.log"))?;
    let mut command = Command::new("/bin/sh");
    command
        .args(["-c", RELOAD, "sh"])
        .arg(&job.launchctl)
        .arg(&job.domain)
        .arg(LABEL)
        .arg(&job.plist)
        .arg(new)
        .stdin(std::process::Stdio::null())
        .stdout(log.try_clone()?)
        .stderr(log);
    // SAFETY: setsid is async-signal-safe and touches nothing else.
    unsafe {
        command.pre_exec(|| if libc::setsid() < 0 { Err(std::io::Error::last_os_error()) } else { Ok(()) });
    }
    command.spawn().map(drop)
}

/// Runs once the backend is up: waits out an update's trial (so a reload
/// never stands in for a failed update), then updates the job if it needs it.
pub async fn update(home: Home) {
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
        for _ in 0..600 {
            let found = std::fs::read_to_string(path).unwrap_or_default();
            if found.contains(text) {
                return found;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        panic!("{} never said {text}", path.display());
    }

    #[test]
    fn an_old_plist_gets_process_type_and_the_job_is_reloaded_once() {
        let dir = temp();
        let home = Home::new(dir.join("home"));
        let plist = old_plist(&dir, &home.root().join("bin/factoryd-supervisor"));
        let job = Job { plist: plist.clone(), launchctl: fake_launchctl(&dir, false), domain: "gui/501".into() };

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
        let job = Job { plist, launchctl: PathBuf::from("/nonexistent"), domain: "gui/501".into() };
        let new = prepare(&home, &job).expect("prepared").expect("something to do");
        assert_eq!(read(&new).expect("read")["ProgramArguments"][0], supervisors(&home)[0].to_string_lossy().as_ref());
    }

    #[test]
    fn a_plist_that_wont_load_is_put_back() {
        let dir = temp();
        let home = Home::new(dir.join("home"));
        let plist = old_plist(&dir, &home.root().join("bin/factoryd-supervisor"));
        let before = std::fs::read(&plist).expect("read");
        let job = Job { plist: plist.clone(), launchctl: fake_launchctl(&dir, true), domain: "gui/501".into() };
        let new = prepare(&home, &job).expect("prepared").expect("something to do");
        reload(&home, &job, &new).expect("reload");
        wait_for(&home.root().join("bin/launch-agent.log"), "going back");
        let calls = wait_for(&dir.join("calls"), &format!("bootstrap gui/501 {}\nbootstrap", plist.display()));
        assert_eq!(calls.lines().filter(|line| line.starts_with("bootstrap")).count(), 2, "{calls}");
        // The second bootstrap ran after the old plist was back.
        std::thread::sleep(Duration::from_millis(100));
        assert_eq!(std::fs::read(&plist).expect("read"), before);
    }

    #[test]
    fn nothing_is_done_for_another_factorys_job_or_an_up_to_date_one() {
        let dir = temp();
        let home = Home::new(dir.join("home"));
        let job = |plist: PathBuf| Job { plist, launchctl: PathBuf::from("/nonexistent"), domain: "gui/501".into() };
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
