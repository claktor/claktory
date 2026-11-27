//! The supervisor's own copy on Windows: the
//! sign-in task runs `bin\factoryd-supervisor.exe supervise`, a copy of
//! factoryd that an update leaves alone, as the Mac's LaunchAgent runs
//! `Claktory Agents.app`. A backend update replaces only
//! `bin\factoryd.exe`, so the program the supervisor runs from, which is
//! what undoes a bad update, never changes under it.
//!
//! Only when `update::SUPERVISOR_VERSION` goes up does the backend, once
//! it's past its update's trial, write the release's factoryd.exe over the
//! copy, once per version: staged beside it, checked as Claktory's own
//! (`authenticode::check_program`, when this backend is signed), then put
//! in by moving the running one aside, which Windows allows while it runs.
//! The new copy runs from the supervisor's next start (the next sign-in);
//! the one moved aside is deleted once nothing runs from it.

use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::home::{Home, program};

/// The supervisor's copy, in `bin\`.
pub fn installed(home: &Home) -> PathBuf {
    home.root().join("bin").join(program("factoryd-supervisor"))
}

/// Where the copy being replaced goes, until nothing runs from it.
fn aside(home: &Home) -> PathBuf {
    home.root().join("bin").join(format!("{}.previous", program("factoryd-supervisor")))
}

/// The new copy, before it's checked and put in.
fn staged(home: &Home) -> PathBuf {
    home.root().join("bin").join(format!("{}.new", program("factoryd-supervisor")))
}

/// Records which version's supervisor was tried, so each is tried once.
fn marker(home: &Home) -> PathBuf {
    home.root().join("bin").join("supervisor.json")
}

/// What the supervisor at `program` does, as a number
/// (`update::SUPERVISOR_VERSION`): 0 when it can't be run, doesn't answer
/// within 5 s, or answers anything else.
pub fn supervisor_version(program: &Path) -> u32 {
    let Ok(mut child) = std::process::Command::new(program)
        .arg("supervisor-version")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
    else {
        return 0;
    };
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    loop {
        match child.try_wait() {
            Ok(Some(status)) if status.success() => break,
            Ok(None) if std::time::Instant::now() < deadline => std::thread::sleep(Duration::from_millis(20)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return 0;
            }
        }
    }
    let mut said = String::new();
    if let Some(mut stdout) = child.stdout.take() {
        let _ = std::io::Read::read_to_string(&mut stdout, &mut said);
    }
    said.trim().parse().unwrap_or(0)
}

fn same_file(a: &Path, b: &Path) -> bool {
    match (std::fs::read(a), std::fs::read(b)) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    }
}

/// What `refresh` did.
#[derive(Debug, PartialEq)]
pub enum Refreshed {
    /// Nothing to do: no copy installed (a dev home, or the app starts the
    /// backend itself), no release file, tried already, or the copy is as
    /// new.
    Nothing,
    /// The copy is the release's factoryd now, from the supervisor's next
    /// start.
    Replaced,
}

/// Writes `release`'s factoryd over the supervisor's copy when the copy is
/// older (by `version_of`), once per `version`. `check` says whether the
/// staged copy is Claktory's own.
pub fn refresh(
    home: &Home,
    version: &str,
    release: &Path,
    version_of: impl Fn(&Path) -> u32,
    check: impl Fn(&Path) -> Result<(), String>,
) -> Result<Refreshed, String> {
    let installed = installed(home);
    // An earlier copy moved aside, once nothing runs from it.
    let _ = std::fs::remove_file(aside(home));
    let new = release.join(program("factoryd"));
    if !installed.is_file() || !new.is_file() {
        return Ok(Refreshed::Nothing);
    }
    let tried = std::fs::read_to_string(marker(home)).ok().and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok());
    if tried.is_some_and(|tried| tried["version"] == version) {
        return Ok(Refreshed::Nothing);
    }
    if same_file(&installed, &new) || version_of(&installed) >= crate::update::SUPERVISOR_VERSION {
        return Ok(Refreshed::Nothing);
    }
    let record = serde_json::json!({"version": version, "time": crate::transcript::now_ms()});
    std::fs::write(marker(home), record.to_string()).map_err(|error| format!("Can't write {}: {error}", marker(home).display()))?;
    let staged = staged(home);
    let _ = std::fs::remove_file(&staged);
    std::fs::copy(&new, &staged).map_err(|error| format!("Can't copy {}: {error}", new.display()))?;
    let put_in = check(&staged).and_then(|()| {
        if !same_file(&staged, &new) {
            return Err(format!("the copy of {} isn't the same", new.display()));
        }
        // Windows lets a running program be renamed, never replaced.
        std::fs::rename(&installed, aside(home)).map_err(|error| format!("Can't move {} aside: {error}", installed.display()))?;
        std::fs::rename(&staged, &installed).map_err(|error| {
            // The old copy goes back, so the task still finds its program.
            let _ = std::fs::rename(aside(home), &installed);
            format!("Can't put the new supervisor in: {error}")
        })
    });
    if let Err(error) = put_in {
        let _ = std::fs::remove_file(&staged);
        return Err(error);
    }
    // Deleted now if the old copy isn't running (never in a dev home).
    let _ = std::fs::remove_file(aside(home));
    Ok(Refreshed::Replaced)
}

/// Once the backend is past its update's trial, under the supervisor:
/// brings the supervisor's copy up to date (`refresh`).
pub async fn update(home: Home) {
    if std::env::var_os(crate::update::SUPERVISED).is_none() {
        return;
    }
    loop {
        tokio::time::sleep(Duration::from_secs(30)).await;
        if !home.trial_path().exists() {
            break;
        }
    }
    let done = tokio::task::spawn_blocking(move || {
        let release = home.releases().join(crate::update::VERSION);
        refresh(&home, crate::update::VERSION, &release, supervisor_version, super::authenticode::check_program)
    })
    .await;
    match done {
        Ok(Ok(Refreshed::Replaced)) => {
            tracing::info!("the supervisor's copy is this release's now; it runs from the supervisor's next start")
        }
        Ok(Ok(Refreshed::Nothing)) => {}
        Ok(Err(error)) => tracing::error!(%error, "could not update the supervisor's copy"),
        Err(error) => tracing::error!(%error, "could not update the supervisor's copy"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup(name: &str) -> (PathBuf, Home, PathBuf) {
        let dir = std::env::temp_dir().join(format!("claktory-supervisor-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let home = Home::new(dir.join("home"));
        std::fs::create_dir_all(home.root().join("bin")).expect("bin");
        let release = home.releases().join("v0.2.0");
        std::fs::create_dir_all(&release).expect("release");
        std::fs::write(release.join(program("factoryd")), b"the release's factoryd").expect("release factoryd");
        (dir, home, release)
    }

    #[test]
    fn an_older_supervisor_copy_becomes_the_releases_factoryd_byte_for_byte_once() {
        let (dir, home, release) = setup("older");
        std::fs::write(installed(&home), b"the old supervisor").expect("installed");
        let older = |_: &Path| crate::update::SUPERVISOR_VERSION - 1;
        assert_eq!(refresh(&home, "v0.2.0", &release, older, |_| Ok(())), Ok(Refreshed::Replaced));
        assert_eq!(std::fs::read(installed(&home)).expect("copy"), std::fs::read(release.join(program("factoryd"))).expect("release"));
        assert!(!aside(&home).exists() && !staged(&home).exists());
        // Tried once per version, whatever the copy says.
        std::fs::write(installed(&home), b"put back by hand").expect("installed");
        assert_eq!(refresh(&home, "v0.2.0", &release, older, |_| Ok(())), Ok(Refreshed::Nothing));
        assert_eq!(std::fs::read(installed(&home)).expect("copy"), b"put back by hand");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_copy_as_new_or_none_at_all_is_left_alone() {
        let (dir, home, release) = setup("same");
        assert_eq!(refresh(&home, "v0.2.0", &release, |_| 0, |_| Ok(())), Ok(Refreshed::Nothing));
        assert!(!installed(&home).exists(), "no copy installed: none made");
        std::fs::write(installed(&home), b"the current supervisor").expect("installed");
        let current = |_: &Path| crate::update::SUPERVISOR_VERSION;
        assert_eq!(refresh(&home, "v0.2.0", &release, current, |_| Ok(())), Ok(Refreshed::Nothing));
        assert_eq!(std::fs::read(installed(&home)).expect("copy"), b"the current supervisor");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_copy_that_isnt_claktorys_own_is_never_put_in() {
        let (dir, home, release) = setup("unsigned");
        std::fs::write(installed(&home), b"the old supervisor").expect("installed");
        let refused = refresh(&home, "v0.2.0", &release, |_| 0, |_| Err("it isn't signed".into()));
        assert_eq!(refused, Err("it isn't signed".into()));
        assert_eq!(std::fs::read(installed(&home)).expect("copy"), b"the old supervisor");
        assert!(!staged(&home).exists() && !aside(&home).exists());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_running_copy_is_moved_aside_and_replaced() {
        let (dir, home, release) = setup("running");
        // A real program, running from the copy's path.
        let ping = factory_agent::windows::system::system_folder().expect("System32").join("ping.exe");
        std::fs::copy(&ping, installed(&home)).expect("installed");
        let mut running = std::process::Command::new(installed(&home))
            .args(["-n", "30", "127.0.0.1"])
            .stdout(std::process::Stdio::null())
            .spawn()
            .expect("run the copy");
        let replaced = refresh(&home, "v0.2.0", &release, |_| 0, |_| Ok(()));
        let _ = running.kill();
        let _ = running.wait();
        assert_eq!(replaced, Ok(Refreshed::Replaced));
        assert_eq!(std::fs::read(installed(&home)).expect("copy"), b"the release's factoryd");
        // Moved aside while it ran; deleted at the next try.
        assert!(aside(&home).exists());
        let _ = refresh(&home, "v0.2.0", &release, |_| 0, |_| Ok(()));
        assert!(!aside(&home).exists());
        let _ = std::fs::remove_dir_all(dir);
    }
}
