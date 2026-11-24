//! Claktory's background service on a Mac (friend bug #24): whether the
//! LaunchAgent's supervisor runs this backend, and putting it back in charge
//! when it doesn't.
//!
//! A backend started some other way (`factoryd` in Terminal, through the
//! installer's link on PATH) works, but nothing starts it again if it stops
//! or the Mac restarts, and it can't stop and start itself for a move. The
//! app's own check at launch can't see it: the job's plist is there and
//! something answers on the port. So the backend says so itself, in the
//! Workspace (`BackendService`), once it's up, and RestoreBackendService
//! hands over: a helper in a session of its own waits for this backend to
//! exit, then loads the job (`launchctl bootstrap`), or restarts it when
//! it's loaded already (its supervisor can't start a backend while this one
//! holds the port); and this backend stops as SIGTERM stops it, finishing
//! what it's writing.
//!
//! Only for this user's real Claktory on a Mac: the dev Claktory and those
//! tests or QA run elsewhere (`FACTORY_HOME`) say nothing, unless
//! `FACTORY_SERVICE_LABEL` names a job of their own for QA to try this with
//! (`~/Library/LaunchAgents/<label>.plist`), never Claktory's.

use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use factory_protocol::v1::{self, BackendServiceState as State};

use crate::factory::{Factory, RequestError};
use crate::home::Home;
use crate::launch_agent::{self, Job};

/// What the Workspace shows of it.
#[derive(Default)]
pub(crate) struct Service {
    view: Mutex<v1::BackendService>,
}

impl Service {
    pub(crate) fn view(&self) -> v1::BackendService {
        self.view.lock().unwrap_or_else(std::sync::PoisonError::into_inner).clone()
    }

    fn set(&self, view: v1::BackendService) {
        *self.view.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = view;
    }
}

/// How long the backend has to stop before the service is loaded anyway
/// (its supervisor then starts it once the port is free).
const STOP_WITHIN_TENTHS: u32 = 600;

/// Waits for the backend (`pid`) to exit, then loads the job, or restarts
/// it when it's loaded already. In a session of its own, as launch_agent's
/// helpers.
const RESTORE: &str = r#"
lc="$1" domain="$2" label="$3" plist="$4" pid="$5" tries="$6"
i=0
while kill -0 "$pid" 2>/dev/null && [ $i -lt "$tries" ]; do sleep 0.1; i=$((i + 1)); done
if "$lc" print "$domain/$label" >/dev/null 2>&1; then
  "$lc" kickstart -k "$domain/$label" && echo "restarted $label, which runs Claktory again" && exit 0
else
  "$lc" bootstrap "$domain" "$plist" && echo "loaded $label, which runs Claktory again" && exit 0
fi
echo "couldn't put $label back in charge of Claktory"
exit 1
"#;

/// QA's job, standing in for Claktory's (`FACTORY_SERVICE_LABEL`).
fn qa_label() -> Option<String> {
    std::env::var("FACTORY_SERVICE_LABEL").ok().filter(|label| !label.is_empty() && label != launch_agent::LABEL)
}

/// Whether `home` is this user's real Claktory, as installed.
fn real(home: &Home) -> bool {
    let Some(user_home) = std::env::home_dir() else { return false };
    let installed = crate::home::instance(false, &user_home).home;
    let same = |a: &Path, b: &Path| a.canonicalize().ok().zip(b.canonicalize().ok()).is_some_and(|(a, b)| a == b);
    same(home.root(), installed.root())
}

/// The job's plist runs this Claktory's supervisor, which is there, and the
/// backend it starts is in place.
fn set_up(home: &Home, job: &Job, label: &str) -> bool {
    let supervisors = launch_agent::supervisors(home);
    let ours = launch_agent::read(&job.plist).is_ok_and(|plist| {
        let program = plist["ProgramArguments"][0].as_str().unwrap_or_default().to_owned();
        plist["Label"] == label && supervisors.iter().any(|supervisor| supervisor.to_string_lossy() == program && supervisor.is_file())
    });
    ours && std::fs::metadata(home.backend()).is_ok_and(|backend| backend.is_file() && backend.len() > 0)
}

/// Whether launchd has the job loaded.
fn loaded(job: &Job, label: &str) -> bool {
    std::process::Command::new(&job.launchctl)
        .arg("print")
        .arg(format!("{}/{label}", job.domain))
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

/// How a backend runs: `supervised` by its service or not, on `home`, with
/// the job at `job`.
pub(crate) fn examine(home: &Home, job: &Job, label: &str, supervised: bool) -> v1::BackendService {
    if supervised {
        return v1::BackendService { state: State::Running.into(), ..Default::default() };
    }
    let can_restore = set_up(home, job, label);
    let reason = if !can_restore {
        "Claktory was started by hand, and its background service isn't set up on this Mac, so Claktory won't start again by itself if it stops or this Mac restarts, and it can't move to another computer. Quit Claktory's app and open it again to repair it."
    } else if loaded(job, label) {
        "Claktory was started by hand while its background service runs too, so the service can't start it. Restore hands Claktory back to the service: it restarts once, and your agents carry on."
    } else {
        "Claktory was started by hand, and its background service isn't running, so Claktory won't start again by itself if it stops or this Mac restarts, and it can't move to another computer. Restore puts the service back in charge: Claktory restarts once, and your agents carry on."
    };
    v1::BackendService { state: State::ByHand.into(), reason: reason.to_owned(), can_restore, restoring: false }
}

/// This Mac's real Claktory: its job and label, if it's one to say
/// anything about; QA's, with FACTORY_SERVICE_LABEL.
fn job_here(home: &Home) -> Option<(Job, String)> {
    if !cfg!(target_os = "macos") {
        return None;
    }
    let mut job = Job::for_this_user()?;
    match qa_label() {
        Some(label) => {
            job.plist = job.plist.with_file_name(format!("{label}.plist"));
            Some((job, label))
        }
        None if real(home) => Some((job, launch_agent::LABEL.to_owned())),
        None => None,
    }
}

/// At start: how this backend runs, for the Workspace.
pub(crate) fn check(factory: &Arc<Factory>) {
    let factory = factory.clone();
    tokio::spawn(async move {
        let home = factory.home.clone();
        let supervised = std::env::var_os(crate::update::SUPERVISED).is_some();
        let view = tokio::task::spawn_blocking(move || job_here(&home).map(|(job, label)| examine(&home, &job, &label, supervised))).await;
        if let Ok(Some(view)) = view {
            if view.state == State::ByHand as i32 {
                tracing::warn!(can_restore = view.can_restore, "Claktory wasn't started by its background service");
            }
            factory.service.set(view);
            factory.broadcast_workspace(&factory.lock());
        }
    });
}

/// RestoreBackendService: the helper waits for this backend to stop and
/// puts the service back in charge; this backend stops as SIGTERM stops it.
pub(crate) async fn restore(factory: &Arc<Factory>) -> Result<(), RequestError> {
    let home = factory.home.clone();
    let supervised = std::env::var_os(crate::update::SUPERVISED).is_some();
    let found =
        tokio::task::spawn_blocking(move || job_here(&home).map(|(job, label)| (examine(&home, &job, &label, supervised), job, label)))
            .await
            .map_err(|error| RequestError::Usage(error.to_string()))?;
    let Some((view, job, label)) = found else {
        return Err(RequestError::Usage("Only Claktory on a Mac runs under a background service.".to_owned()));
    };
    if view.state != State::ByHand as i32 {
        return Err(RequestError::Usage("Claktory's background service already runs it.".to_owned()));
    }
    if !view.can_restore {
        return Err(RequestError::Usage(view.reason));
    }
    hand_over(&factory.home, &job, &label, std::process::id(), STOP_WITHIN_TENTHS).map_err(RequestError::Io)?;
    tracing::info!("handing Claktory back to its background service; stopping");
    factory.service.set(v1::BackendService { restoring: true, ..view });
    factory.broadcast_workspace(&factory.lock());
    // A moment for the answer to reach the app, then stop as the
    // supervisor would stop it.
    tokio::spawn(async {
        tokio::time::sleep(Duration::from_secs(1)).await;
        // SAFETY: sends SIGTERM to this process, whose handler stops it.
        unsafe { libc::kill(libc::getpid(), libc::SIGTERM) };
    });
    Ok(())
}

/// Starts the helper that waits for `pid` (`tries` tenths of a second at
/// most) and then loads or restarts the job.
fn hand_over(home: &Home, job: &Job, label: &str, pid: u32, tries: u32) -> std::io::Result<()> {
    let mut command = launch_agent::helper(home, RESTORE)?;
    command.arg(&job.launchctl).arg(&job.domain).arg(label).arg(&job.plist).arg(pid.to_string()).arg(tries.to_string());
    command.spawn().map(drop)
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;
    use crate::launch_agent::LABEL;
    use std::os::unix::fs::PermissionsExt;
    use std::path::PathBuf;

    fn temp() -> PathBuf {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let next = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("factoryd-service-{}-{next}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("home/bin")).expect("dir");
        dir
    }

    fn executable(path: &Path, text: &str) {
        std::fs::create_dir_all(path.parent().expect("parent")).expect("dir");
        std::fs::write(path, text).expect("write");
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).expect("chmod");
    }

    /// A home with its backend and supervisor, a plist that runs it, and a
    /// launchctl that logs what it's asked and says the job is `loaded`.
    fn setup(dir: &Path, loaded: bool) -> (Home, Job) {
        let home = Home::new(dir.join("home"));
        executable(&home.backend(), "#!/bin/sh\n");
        let supervisor = launch_agent::supervisors(&home)[0].clone();
        executable(&supervisor, "#!/bin/sh\n");
        let plist = dir.join(format!("{}.plist", launch_agent::LABEL));
        let text = format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
  <key>Label</key><string>{}</string>
  <key>ProgramArguments</key><array><string>{}</string><string>supervise</string></array>
</dict></plist>"#,
            launch_agent::LABEL,
            supervisor.display()
        );
        std::fs::write(&plist, text).expect("plist");
        let log = dir.join("launchctl.log");
        let print = if loaded { "exit 0" } else { "exit 113" };
        let launchctl = dir.join("launchctl");
        executable(&launchctl, &format!("#!/bin/sh\necho \"$@\" >> \"{}\"\n[ \"$1\" = print ] && {print}\nexit 0\n", log.display()));
        (home, Job { plist, launchctl, domain: "gui/501".into(), codesign: PathBuf::from("/usr/bin/true") })
    }

    fn asked(dir: &Path) -> String {
        std::fs::read_to_string(dir.join("launchctl.log")).unwrap_or_default()
    }

    /// The helper's log, once it says it's done.
    fn helper_done(home: &Home) -> String {
        let log = home.root().join("bin/launch-agent.log");
        for _ in 0..100 {
            let text = std::fs::read_to_string(&log).unwrap_or_default();
            if text.contains("Claktory again") || text.contains("couldn't") {
                return text;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        panic!("the helper didn't finish: {}", std::fs::read_to_string(&log).unwrap_or_default());
    }

    #[test]
    fn a_backend_its_service_runs_says_so_and_one_started_by_hand_offers_restore() {
        let dir = temp();
        let (home, job) = setup(&dir, false);
        let running = examine(&home, &job, LABEL, true);
        assert_eq!((running.state(), running.can_restore), (State::Running, false));

        let by_hand = examine(&home, &job, LABEL, false);
        assert_eq!((by_hand.state(), by_hand.can_restore, by_hand.restoring), (State::ByHand, true, false));
        assert!(by_hand.reason.contains("its background service isn't running"), "{}", by_hand.reason);

        // The job is loaded (its supervisor can't start a backend while
        // this one holds the port): Restore still hands over.
        let (home_loaded, job_loaded) = setup(&temp(), true);
        let both = examine(&home_loaded, &job_loaded, LABEL, false);
        assert!(both.can_restore && both.reason.contains("while its background service runs too"), "{}", both.reason);

        // A plist that runs something else, or no backend: nothing to
        // restore, and it says how to repair.
        std::fs::write(&job.plist, "not a plist").expect("plist");
        let broken = examine(&home, &job, LABEL, false);
        assert_eq!((broken.state(), broken.can_restore), (State::ByHand, false));
        assert!(broken.reason.contains("isn't set up on this Mac"), "{}", broken.reason);
    }

    #[test]
    fn restoring_waits_for_the_backend_to_stop_then_loads_the_job_or_restarts_it() {
        let dir = temp();
        let (home, job) = setup(&dir, false);
        // A stand-in for this backend, which stops in a moment.
        let mut backend = std::process::Command::new("/bin/sleep").arg("0.3").spawn().expect("sleep");
        hand_over(&home, &job, LABEL, backend.id(), 100).expect("helper");
        std::thread::sleep(Duration::from_millis(100));
        assert!(!asked(&dir).contains("bootstrap"), "loaded before the backend stopped: {}", asked(&dir));
        backend.wait().expect("stopped");
        let said = helper_done(&home);
        assert!(said.contains("loaded dev.factory.factoryd"), "{said}");
        assert!(asked(&dir).contains(&format!("bootstrap gui/501 {}", job.plist.display())), "{}", asked(&dir));

        let dir = temp();
        let (home, job) = setup(&dir, true);
        hand_over(&home, &job, LABEL, 999_999, 100).expect("helper");
        let said = helper_done(&home);
        assert!(said.contains("restarted dev.factory.factoryd"), "{said}");
        assert!(asked(&dir).contains("kickstart -k gui/501/dev.factory.factoryd"), "{}", asked(&dir));
    }
}
