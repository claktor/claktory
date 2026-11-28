//! Claktory's background service on Windows, for `crate::service`: the
//! supervisor its sign-in task starts (`sign_in`, `install`), which the
//! app stops with StopEngine and starts again with `factoryd install
//! --start`.
//!
//! A stop leaves `stopped.json` saying what stopped it, as on a Mac. The
//! person's switch also disables the task, so Claktory stays off at the
//! next sign-in until they start it. Then the backend asks its supervisor
//! to stop (its stop event), and the supervisor stops the backend first.
//! The app quitting leaves the task as the person chose. A stop that
//! doesn't happen within a while puts the task back as they chose.
//!
//! Only for this user's own Claktory, real or dev, in LocalAppData and run
//! by its supervisor: a backend started by hand, or one in a home of its
//! own (`FACTORY_HOME`), says nothing.

use std::time::Duration;

use super::sign_in::{Task, running};
use crate::home::Home;

/// This home's sign-in task, when it's this user's own Claktory, real or
/// dev, and its supervisor runs this backend.
pub(crate) fn here(home: &Home) -> Option<Task> {
    std::env::var_os(crate::update::SUPERVISED)?;
    let canonical = |path: &std::path::Path| super::fs::canonical(path).ok();
    let root = canonical(home.root())?;
    [false, true].into_iter().find_map(|dev| {
        let installed = canonical(&super::dirs::factory_home(dev).ok()?)?;
        if installed == root { Task::for_home(home, dev).ok() } else { None }
    })
}

/// Whether the task should start Claktory at sign-in: as the person last
/// chose.
pub(crate) fn chosen(home: &Home, task: &Task) -> bool {
    super::install::kept_choice(home).unwrap_or_else(|| task.query().is_none_or(|task| task.enabled))
}

/// Disables the task when the person's switch stops Claktory (`switch`),
/// then, a moment later so the answer reaches the app, asks this home's
/// supervisor to stop. Err when there's no supervisor to ask.
pub(crate) fn stop(home: &Home, task: &Task, switch: bool) -> std::io::Result<()> {
    let supervisors = running(&super::supervisor_copy::installed(home));
    if supervisors.is_empty() {
        return Err(std::io::Error::other("Claktory's supervisor isn't running"));
    }
    if switch && let Err(error) = task.set_enabled(false) {
        tracing::error!(%error, "could not keep Claktory from starting at sign-in");
    }
    std::thread::Builder::new().name("stop-supervisor".to_owned()).spawn(move || {
        std::thread::sleep(Duration::from_secs(1));
        for pid in supervisors {
            if let Err(error) = super::stop::ask_to_stop(pid) {
                tracing::error!(%error, pid, "could not ask the supervisor to stop");
            }
        }
    })?;
    Ok(())
}

/// The stop didn't happen: the task does as the person chose again.
pub(crate) fn stop_failed(home: &Home, task: &Task) {
    if let Err(error) = task.set_enabled(chosen(home, task)) {
        tracing::error!(%error, "could not set whether Claktory starts at sign-in");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::windows::sign_in::Action;

    /// With no supervisor to ask, a stop is refused, and the person's
    /// switch leaves the task as it was: Claktory isn't kept off at the
    /// next sign-in by a stop that never happened.
    #[test]
    fn a_stop_with_no_supervisor_is_refused_and_leaves_the_task_on() {
        let dir = std::env::temp_dir().join(format!("claktory-service-{}", std::process::id()));
        let home = Home::new(dir.join("home"));
        let sid = factory_agent::windows::system::user_sid().expect("SID");
        let supervisor = super::super::supervisor_copy::installed(&home).display().to_string();
        let task = Task::new(
            format!(r"\Claktory\Claktory test service {} {sid}", std::process::id()),
            Action::supervising(&supervisor, &home.root().display().to_string(), false),
        );
        task.register(true, &std::env::temp_dir()).expect("registered");
        assert!(stop(&home, &task, true).is_err());
        assert!(task.query().expect("task").enabled);
        assert!(chosen(&home, &task), "nothing kept: as the task is");
        task.set_enabled(false).expect("off");
        assert!(!chosen(&home, &task));
        task.delete().expect("deleted");
        let _ = std::fs::remove_dir_all(dir);
    }
}
