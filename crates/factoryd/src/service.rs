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
//! Stopping and starting it from the Mac app (friend bug #27) uses
//! launchd's own enabled flag: a stopped Claktory's job is disabled and
//! booted out, so KeepAlive can't bring it back and launchd doesn't load it
//! at login, until the app's `factoryd install --start` enables and loads it
//! again. StopEngine disables it, and a helper in a session of its own
//! boots it out: launchd stops the supervisor, which stops this backend as
//! SIGTERM does. A disabled job that's loaded keeps running (KeepAlive
//! too), so in stop mode (Settings.when_app_quits) the job stays disabled
//! while it runs: nothing starts it at login, even after a crash or a power
//! cut with the app open. `stopped.json` in the home says what stopped it
//! (the app quitting, or the person's switch, which holds it off when the
//! app opens). Never while an update goes in or is on trial: a stop then
//! would read as a failed update and roll it back.
//!
//! Only for this user's real Claktory on a Mac: the dev Claktory and those
//! tests or QA run elsewhere (`FACTORY_HOME`) say nothing, unless
//! `FACTORY_SERVICE_LABEL` names a job of their own for QA to try this with
//! (`~/Library/LaunchAgents/<label>.plist`), never Claktory's.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use factory_protocol::v1::{self, BackendServiceState as State, QuitMode};
use rusqlite::OptionalExtension;

use crate::locking::lock as guard;
use crate::factory::{Factory, RequestError};
use crate::home::Home;
use crate::launch_agent::{self, Job};
use crate::store::Store;

pub(crate) const SCHEMA: &str = "
-- What happens to Claktory on its Mac when the Mac app quits (friend bug
-- #27): 'keep' (it keeps running) or 'stop'. No row: keep.
CREATE TABLE engine (id INTEGER PRIMARY KEY CHECK (id = 1), when_app_quits TEXT NOT NULL);
";

/// What the Workspace shows of it, and what's under way.
#[derive(Default)]
pub(crate) struct Service {
    view: Mutex<v1::BackendService>,
    /// Settings.when_app_quits: true for stop.
    stop_on_quit: Mutex<bool>,
    phase: Mutex<Phase>,
}

/// An update going in and a stop shut each other out.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Phase {
    #[default]
    Running,
    Updating,
    Stopping,
}

impl Service {
    /// With the setting factory.db has.
    pub(crate) fn load(store: &Store) -> Service {
        let mode: Option<String> =
            store.db().query_row("SELECT when_app_quits FROM engine WHERE id = 1", [], |row| row.get(0)).optional().unwrap_or_else(
                |error| {
                    tracing::error!(%error, "could not read what happens when the app quits");
                    None
                },
            );
        Service { stop_on_quit: Mutex::new(mode.as_deref() == Some("stop")), ..Service::default() }
    }

    pub(crate) fn view(&self) -> v1::BackendService {
        guard(&self.view).clone()
    }

    fn set(&self, view: v1::BackendService) {
        *guard(&self.view) = view;
    }

    /// Settings.when_app_quits.
    pub(crate) fn when_app_quits(&self) -> QuitMode {
        if *guard(&self.stop_on_quit) { QuitMode::Stop } else { QuitMode::KeepRunning }
    }

    /// An update starts going in; refused while Claktory is stopping.
    pub(crate) fn begin_update(&self) -> Result<(), RequestError> {
        let mut phase = guard(&self.phase);
        if *phase == Phase::Stopping {
            return Err(RequestError::Usage("Claktory is stopping, so it can't update now.".to_owned()));
        }
        *phase = Phase::Updating;
        Ok(())
    }

    /// The update didn't go in: Claktory carries on as it was.
    pub(crate) fn update_failed(&self) {
        let mut phase = guard(&self.phase);
        if *phase == Phase::Updating {
            *phase = Phase::Running;
        }
    }
}

/// What stopped Claktory, for `factoryd install --check`: `{"by": "quit" |
/// "switch", "time": ms}`. Gone once it starts again.
pub(crate) fn stopped_marker(home: &Home) -> PathBuf {
    home.root().join("stopped.json")
}

/// Whether the person's switch stopped it (`stopped.json` says "switch"),
/// so opening the app leaves it stopped.
pub(crate) fn held(home: &Home) -> bool {
    std::fs::read(stopped_marker(home))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
        .is_some_and(|marker| marker["by"] == "switch")
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
# A stopped Claktory's job is disabled (friend bug #27), and launchd won't
# load a disabled job; in stop mode the backend disables it again as it starts.
"$lc" enable "$domain/$label"
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
        return v1::BackendService { state: State::Running.into(), can_stop: true, ..Default::default() };
    }
    let can_restore = set_up(home, job, label);
    let reason = if !can_restore {
        "Claktory was started by hand, and its background service isn't set up on this Mac, so Claktory won't start again by itself if it stops or this Mac restarts, and it can't move to another computer. Quit Claktory's app and open it again to repair it."
    } else if loaded(job, label) {
        "Claktory was started by hand while its background service runs too, so the service can't start it. Restore hands Claktory back to the service: it restarts once, and your agents carry on."
    } else {
        "Claktory was started by hand, and its background service isn't running, so Claktory won't start again by itself if it stops or this Mac restarts, and it can't move to another computer. Restore puts the service back in charge: Claktory restarts once, and your agents carry on."
    };
    v1::BackendService { state: State::ByHand.into(), reason: reason.to_owned(), can_restore, ..Default::default() }
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

/// Whether this backend runs under this Mac's background service, which
/// the Mac app can stop and start (BackendService.can_stop).
pub(crate) fn runs_here(home: &Home) -> bool {
    std::env::var_os(crate::update::SUPERVISED).is_some() && job_here(home).is_some()
}

/// At start: how this backend runs, for the Workspace; and in stop mode,
/// the job disabled, so nothing starts it at login.
pub(crate) fn check(factory: &Arc<Factory>) {
    let factory = factory.clone();
    tokio::spawn(async move {
        let home = factory.home.clone();
        let supervised = std::env::var_os(crate::update::SUPERVISED).is_some();
        let stop = factory.service.when_app_quits() == QuitMode::Stop;
        let view = tokio::task::spawn_blocking(move || {
            job_here(&home).map(|(job, label)| {
                let view = examine(&home, &job, &label, supervised);
                // Running, so started: whatever stopped it is over.
                let _ = std::fs::remove_file(stopped_marker(&home));
                if view.can_stop {
                    set_enabled(&job, &label, !stop);
                }
                view
            })
        })
        .await;
        if let Ok(Some(view)) = view {
            if view.state == State::ByHand as i32 {
                tracing::warn!(can_restore = view.can_restore, "Claktory wasn't started by its background service");
            }
            factory.service.set(view);
            factory.broadcast_workspace(&factory.lock());
        }
    });
}

/// `launchctl enable` or `disable`: whether launchd loads the job at login
/// and lets it be bootstrapped. A loaded job keeps running either way.
fn set_enabled(job: &Job, label: &str, enabled: bool) {
    let verb = if enabled { "enable" } else { "disable" };
    let done = std::process::Command::new(&job.launchctl)
        .arg(verb)
        .arg(format!("{}/{label}", job.domain))
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .output();
    match done {
        Ok(output) if output.status.success() => {}
        Ok(output) => tracing::error!(verb, error = %String::from_utf8_lossy(&output.stderr).trim(), "launchctl refused"),
        Err(error) => tracing::error!(verb, %error, "could not run launchctl"),
    }
}

/// UpdateSettings.when_app_quits: kept, and the job enabled for keep or
/// disabled for stop. Answers Onboarding.when_app_quits too.
pub(crate) async fn set_when_app_quits(factory: &Factory, mode: QuitMode) -> Result<(), RequestError> {
    let stop = match mode {
        QuitMode::Stop => true,
        QuitMode::KeepRunning => false,
        QuitMode::Unspecified => return Err(RequestError::Usage("when_app_quits must be KEEP_RUNNING or STOP".to_owned())),
    };
    factory
        .store
        .db()
        .execute("INSERT OR REPLACE INTO engine (id, when_app_quits) VALUES (1, ?1)", [if stop { "stop" } else { "keep" }])?;
    *guard(&factory.service.stop_on_quit) = stop;
    crate::onboarding::when_app_quits_answered(factory)?;
    if factory.service.view().can_stop && !factory.service.view().stopping {
        let home = factory.home.clone();
        tokio::task::spawn_blocking(move || {
            if let Some((job, label)) = job_here(&home) {
                set_enabled(&job, &label, !stop);
            }
        })
        .await
        .map_err(|error| RequestError::Usage(error.to_string()))?;
    }
    factory.broadcast_workspace(&factory.lock());
    Ok(())
}

/// Disables the job, then boots it out: launchd stops the supervisor and,
/// through it, this backend. In a session of its own, as RESTORE.
const STOP: &str = r#"
lc="$1" domain="$2" label="$3"
sleep 1
"$lc" disable "$domain/$label" || echo "couldn't disable $label"
if "$lc" bootout "$domain/$label"; then
  echo "stopped $label, which stays stopped until Claktory's app starts it"
else
  echo "couldn't stop $label"
fi
"#;

/// How long this backend waits to be stopped before it takes it that the
/// stop failed and carries on.
const STOPPED_WITHIN: Duration = Duration::from_secs(45);

/// Why StopEngine can't stop Claktory now, if it can't.
fn stop_refused(local: bool, view: &v1::BackendService, on_trial: bool, phase: Phase) -> Option<&'static str> {
    if !local {
        Some("Stop Claktory from the Mac it runs on.")
    } else if !view.can_stop {
        Some("Only Claktory on a Mac, run by its background service, can be stopped from its app.")
    } else if on_trial || phase == Phase::Updating {
        Some("Claktory is finishing an update. Try again in a minute.")
    } else {
        None
    }
}

/// StopEngine: from the app on this Mac (`local`), when it quits in stop
/// mode (`quitting`) or by the person's switch.
pub(crate) async fn stop(factory: &Arc<Factory>, local: bool, quitting: bool) -> Result<(), RequestError> {
    if local && quitting && factory.service.when_app_quits() != QuitMode::Stop {
        return Ok(());
    }
    let view = factory.service.view();
    {
        let mut phase = guard(&factory.service.phase);
        if let Some(why) = stop_refused(local, &view, crate::update::on_trial(&factory.home), *phase) {
            return Err(RequestError::Usage(why.to_owned()));
        }
        if *phase == Phase::Stopping {
            return Ok(());
        }
        *phase = Phase::Stopping;
    }
    let home = factory.home.clone();
    let by = if quitting { "quit" } else { "switch" };
    let started = tokio::task::spawn_blocking(move || -> std::io::Result<()> {
        let Some((job, label)) = job_here(&home) else {
            return Err(std::io::Error::other("Claktory's background service isn't on this Mac"));
        };
        let marker = serde_json::json!({"by": by, "time": crate::transcript::now_ms()});
        std::fs::write(stopped_marker(&home), marker.to_string())?;
        let mut command = launch_agent::helper(&home, STOP)?;
        command.arg(&job.launchctl).arg(&job.domain).arg(&label);
        command.spawn().map(drop)
    })
    .await
    .map_err(|error| RequestError::Usage(error.to_string()))
    .and_then(|started| started.map_err(RequestError::Io));
    if let Err(error) = started {
        *guard(&factory.service.phase) = Phase::Running;
        let _ = std::fs::remove_file(stopped_marker(&factory.home));
        return Err(error);
    }
    tracing::info!(by, "stopping Claktory: its background service won't start it again until the app does");
    factory.service.set(v1::BackendService { stopping: true, ..view.clone() });
    factory.broadcast_workspace(&factory.lock());
    // Still here long after: the stop failed. Claktory carries on as it
    // was, and the job is enabled again unless stop mode keeps it disabled.
    let factory = factory.clone();
    tokio::spawn(async move {
        tokio::time::sleep(STOPPED_WITHIN).await;
        tracing::error!("Claktory's background service didn't stop it; carrying on (see bin/launch-agent.log)");
        let _ = std::fs::remove_file(stopped_marker(&factory.home));
        let stop = factory.service.when_app_quits() == QuitMode::Stop;
        let home = factory.home.clone();
        let _ = tokio::task::spawn_blocking(move || {
            if let Some((job, label)) = job_here(&home) {
                set_enabled(&job, &label, !stop);
            }
        })
        .await;
        *guard(&factory.service.phase) = Phase::Running;
        factory.service.set(view);
        factory.broadcast_workspace(&factory.lock());
    });
    Ok(())
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
        assert!(asked(&dir).contains("enable gui/501/dev.factory.factoryd\nprint"), "enabled first: {}", asked(&dir));
    }

    /// Friend bug #27, Fugo's requirements: only this Mac's own app, only
    /// where its service runs it, and never while an update goes in or is
    /// on trial.
    #[test]
    fn a_stop_is_refused_from_elsewhere_where_no_service_runs_it_and_during_an_update() {
        let (home, job) = setup(&temp(), false);
        let running = examine(&home, &job, LABEL, true);
        assert!(running.can_stop);
        assert!(!examine(&home, &job, LABEL, false).can_stop, "started by hand");
        assert_eq!(stop_refused(true, &running, false, Phase::Running), None);
        assert_eq!(stop_refused(false, &running, false, Phase::Running), Some("Stop Claktory from the Mac it runs on."));
        let updating = Some("Claktory is finishing an update. Try again in a minute.");
        assert_eq!(stop_refused(true, &running, true, Phase::Running), updating, "on trial");
        assert_eq!(stop_refused(true, &running, false, Phase::Updating), updating, "going in");
        let nowhere = v1::BackendService::default();
        assert!(stop_refused(true, &nowhere, false, Phase::Running).is_some_and(|why| why.starts_with("Only Claktory on a Mac")));

        // And an update isn't started while it stops.
        let service = Service::default();
        *guard(&service.phase) = Phase::Stopping;
        assert!(service.begin_update().is_err());
        *guard(&service.phase) = Phase::Running;
        service.begin_update().expect("an update");
        service.update_failed();
        assert_eq!(*guard(&service.phase), Phase::Running);
    }

    #[test]
    fn stopping_disables_the_job_then_boots_it_out_from_a_helper() {
        let dir = temp();
        let (home, job) = setup(&dir, true);
        let mut command = launch_agent::helper(&home, STOP).expect("helper");
        command.arg(&job.launchctl).arg(&job.domain).arg(LABEL);
        command.spawn().expect("spawn").wait().expect("helper");
        let log = std::fs::read_to_string(home.root().join("bin/launch-agent.log")).expect("log");
        assert!(log.contains("stopped dev.factory.factoryd"), "{log}");
        assert_eq!(asked(&dir), "disable gui/501/dev.factory.factoryd\nbootout gui/501/dev.factory.factoryd\n");

        // What stopped it, for `install --check`.
        assert!(!held(&home));
        std::fs::write(stopped_marker(&home), r#"{"by":"switch","time":1}"#).expect("marker");
        assert!(held(&home));
        std::fs::write(stopped_marker(&home), r#"{"by":"quit","time":1}"#).expect("marker");
        assert!(!held(&home));
    }
}
