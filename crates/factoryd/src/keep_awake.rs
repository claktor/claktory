//! Keeping the Mac awake while agents work (P1, O1), so they carry on while
//! the person is away: only while an agent works and the Mac is plugged in,
//! as `caffeinate -s` holds it, so a laptop on battery sleeps as usual.
//! A desktop Mac does it unless the person says no; a laptop asks, the first
//! time agents work on it (KeepAwake.ask). Closing a laptop's lid still puts
//! it to sleep. The answer is kept in factory.db.

use std::sync::{Arc, Mutex, MutexGuard, Weak};

use factory_protocol::v1;
use rusqlite::{OptionalExtension, params};

use crate::factory::{Factory, Inner, RequestError};
use crate::store::Store;

pub(crate) const SCHEMA: &str = "
-- Keeping the Mac awake while agents work (KeepAwake): the person's answer
-- (NULL until they give one; 1 yes, 0 no), and whether agents have worked
-- on this laptop, so Needs you asks.
CREATE TABLE keep_awake (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    answer INTEGER,
    asked INTEGER NOT NULL DEFAULT 0
);
";

/// What this machine is, as far as keeping it awake goes.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Machine {
    /// A Mac: it can be kept awake.
    pub available: bool,
    /// It has a battery.
    pub laptop: bool,
    /// Where whether it's plugged in is read: `pmset`, or for QA a file.
    pub power: Power,
    /// A laptop on macOS 14 or later: keeping agents working with its lid
    /// closed can be offered (`crate::lid_closed`), in the real Claktory.
    pub lid: bool,
}

/// How a machine says whether it's plugged in.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) enum Power {
    /// Always: a desktop, or nothing to keep awake.
    #[default]
    Mains,
    /// `pmset -g ps`: "AC Power" or "Battery Power".
    Pmset,
    /// A laptop QA simulates: plugged in while the file says "ac".
    File(std::path::PathBuf),
}

/// macOS 14 or later (Fugo, 23220): the floor the lid-closed setting kept from its helper.
#[cfg(target_os = "macos")]
fn macos_14() -> bool {
    let mut buffer = [0u8; 64];
    let mut size = buffer.len();
    // SAFETY: sysctlbyname writes at most `size` bytes into the buffer.
    let read = unsafe {
        libc::sysctlbyname(c"kern.osproductversion".as_ptr(), buffer.as_mut_ptr().cast(), &raw mut size, std::ptr::null_mut(), 0)
    } == 0;
    let version =
        if read { String::from_utf8_lossy(&buffer[..size.min(buffer.len())]).trim_end_matches('\0').to_owned() } else { String::new() };
    version.split('.').next().and_then(|major| major.parse::<u32>().ok()).is_some_and(|major| major >= 14)
}

impl Machine {
    /// This computer: a Mac, and whether `pmset` names an internal battery.
    /// `FACTORY_KEEP_AWAKE_POWER`, a file's path, makes it a laptop that's
    /// plugged in while the file says "ac", for QA.
    pub(crate) fn this() -> Machine {
        #[cfg(target_os = "macos")]
        {
            if let Some(file) = std::env::var_os("FACTORY_KEEP_AWAKE_POWER").filter(|file| !file.is_empty()) {
                return Machine { available: true, laptop: true, power: Power::File(file.into()), lid: macos_14() };
            }
            let batteries = std::process::Command::new("/usr/bin/pmset").args(["-g", "batt"]).output();
            let laptop = batteries.is_ok_and(|output| String::from_utf8_lossy(&output.stdout).contains("InternalBattery"));
            Machine { available: true, laptop, power: if laptop { Power::Pmset } else { Power::Mains }, lid: laptop && macos_14() }
        }
        #[cfg(not(target_os = "macos"))]
        Machine::default()
    }

    /// Whether it's plugged in now. Unknown counts as not, so a laptop isn't
    /// held awake on battery by mistake.
    pub(crate) fn plugged_in(&self) -> bool {
        match &self.power {
            Power::Mains => true,
            Power::Pmset => std::process::Command::new("/usr/bin/pmset")
                .args(["-g", "ps"])
                .output()
                .is_ok_and(|output| String::from_utf8_lossy(&output.stdout).contains("'AC Power'")),
            Power::File(file) => std::fs::read_to_string(file).is_ok_and(|said| said.trim() == "ac"),
        }
    }
}

#[derive(Default)]
struct State {
    answer: Option<bool>,
    asked: bool,
    /// Plugged in, when last looked at.
    plugged_in: bool,
    /// `caffeinate -s`, while it holds the Mac awake.
    holding: Option<std::process::Child>,
}

pub(crate) struct KeepAwake {
    machine: Machine,
    state: Mutex<State>,
    /// A run started or ended: whether to hold the Mac awake is looked at again.
    changed: Arc<tokio::sync::Notify>,
    /// Keeping agents working with the lid closed (#371).
    pub(crate) lid: Arc<crate::lid_closed::Lid>,
}

impl Drop for KeepAwake {
    fn drop(&mut self) {
        if let Some(mut child) = self.state().holding.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

impl KeepAwake {
    /// `real`: this is the real Claktory, the only one that may keep a
    /// closed lid from sleeping the Mac (never Claktory Dev, whose clearing
    /// would undo the real one's hold).
    pub(crate) fn load(store: &Store, machine: Machine, real: bool) -> KeepAwake {
        let read = store
            .db()
            .query_row("SELECT answer, asked FROM keep_awake WHERE id = 1", [], |row| {
                Ok((row.get::<_, Option<bool>>(0)?, row.get::<_, bool>(1)?))
            })
            .optional();
        let (answer, asked) = match read {
            Ok(found) => found.unwrap_or_default(),
            Err(error) => {
                tracing::error!(%error, "could not read whether to keep the Mac awake");
                (None, false)
            }
        };
        let plugged_in = !machine.laptop;
        let lid = Arc::new(crate::lid_closed::Lid::load(store, machine.available && machine.lid && real, real));
        KeepAwake { machine, state: Mutex::new(State { answer, asked, plugged_in, holding: None }), changed: Arc::default(), lid }
    }

    fn state(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// A run started or ended.
    pub(crate) fn runs_changed(&self) {
        self.changed.notify_one();
    }

    /// On: the person said yes, or this is a desktop they haven't answered for.
    fn on(&self, state: &State) -> bool {
        state.answer.unwrap_or(!self.machine.laptop)
    }

    /// `machine`: this Claktory's name, for the lid-closed fix.
    pub(crate) fn view(&self, machine: &str) -> v1::KeepAwake {
        let state = self.state();
        let mut keep = v1::KeepAwake {
            available: self.machine.available,
            laptop: self.machine.laptop,
            on: self.machine.available && self.on(&state),
            answered: state.answer.is_some(),
            ask: self.machine.available && self.machine.laptop && state.asked && state.answer.is_none(),
            holding: state.holding.is_some(),
            plugged_in: self.machine.available && state.plugged_in,
            ..Default::default()
        };
        self.lid.view(&mut keep, machine);
        keep
    }

    /// Whether the power source is worth watching now: a laptop it would
    /// hold awake once plugged in, or holds until it's unplugged.
    fn watching_power(&self, working: bool) -> bool {
        self.machine.laptop && working && self.on(&self.state())
    }

    /// Holds the Mac awake while `working`, it's on and `plugged_in`, else
    /// lets it sleep; the first work on a laptop asks. Whether anything
    /// shown changed.
    fn follow(&self, store: &Store, working: bool, plugged_in: bool) -> bool {
        if !self.machine.available {
            return false;
        }
        let mut state = self.state();
        let mut changed = false;
        if working && self.machine.laptop && !state.asked {
            state.asked = true;
            changed = true;
            save(store, &state);
        }
        if state.plugged_in != plugged_in {
            state.plugged_in = plugged_in;
            changed = true;
        }
        let hold = working && plugged_in && self.on(&state);
        match (&mut state.holding, hold) {
            (None, true) => match hold_awake() {
                Ok(child) => {
                    tracing::info!("keeping the Mac awake while agents work");
                    state.holding = Some(child);
                    changed = true;
                }
                Err(error) => tracing::warn!(%error, "could not keep the Mac awake"),
            },
            (Some(child), false) => {
                let _ = child.kill();
                let _ = child.wait();
                state.holding = None;
                tracing::info!(plugged_in, "no agent works, or it's on battery: the Mac may sleep");
                changed = true;
            }
            _ => {}
        }
        changed
    }

    fn answer(&self, store: &Store, on: bool) {
        let mut state = self.state();
        state.answer = Some(on);
        save(store, &state);
    }
}

fn save(store: &Store, state: &State) {
    let saved =
        store.db().execute("INSERT OR REPLACE INTO keep_awake (id, answer, asked) VALUES (1, ?1, ?2)", params![state.answer, state.asked]);
    if let Err(error) = saved {
        tracing::error!(%error, "could not keep whether to keep the Mac awake");
    }
}

/// `caffeinate -s` (system sleep held off, only while on power), ending by
/// itself with this process, should it end without letting go.
fn hold_awake() -> std::io::Result<std::process::Child> {
    std::process::Command::new("/usr/bin/caffeinate")
        .args(["-s", "-w", &std::process::id().to_string()])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
}

/// Whether an agent or chat works now: it's in a run.
fn working(inner: &Inner) -> bool {
    inner.agents.values().any(|runtime| runtime.run.is_some())
}

/// How long a look at the lid may take (the kernel, `pmset`).
const LID_ANSWER: std::time::Duration = std::time::Duration::from_secs(10);

/// How often a laptop's power source is looked at while it matters.
const POWER_EVERY: std::time::Duration = std::time::Duration::from_secs(30);

/// Follows runs as they start and end, and a laptop's power source while
/// it matters, for as long as Claktory runs.
pub(crate) async fn monitor(factory: Weak<Factory>) {
    loop {
        let Some(strong) = factory.upgrade() else { return };
        // Read before the lock: it runs `pmset`.
        let plugged_in = strong.keep_awake.machine.plugged_in();
        let (watching, working) = {
            let inner = strong.lock();
            let working = working(&inner);
            if strong.keep_awake.follow(&strong.store, working, plugged_in) {
                strong.broadcast_workspace(&inner);
            }
            (strong.keep_awake.watching_power(working), working)
        };
        let changed = Arc::clone(&strong.keep_awake.changed);
        let watching = watching || strong.keep_awake.lid.watched();
        // The lid is looked at outside every lock: it asks the kernel and
        // runs `pmset`. Only on power, as keeping the Mac awake: never a
        // laptop on battery.
        let stepped = tokio::task::spawn_blocking({
            let factory = Arc::clone(&strong);
            move || factory.keep_awake.lid.step(&factory.store, working && plugged_in)
        });
        drop(strong);
        if tokio::time::timeout(LID_ANSWER, stepped).await.is_ok_and(|stepped| stepped.unwrap_or(false))
            && let Some(strong) = factory.upgrade()
        {
            strong.broadcast_workspace(&strong.lock());
        }
        if watching {
            let _ = tokio::time::timeout(POWER_EVERY, changed.notified()).await;
        } else {
            changed.notified().await;
        }
    }
}

/// SetKeepAwake: the person's answer, from the question or Settings.
pub(crate) fn set(factory: &Arc<Factory>, on: bool) -> Result<(), RequestError> {
    if !factory.keep_awake.machine.available {
        return Err(RequestError::Account(v1::ErrorCode::InvalidRequest, "Claktory can keep only a Mac awake.".into()));
    }
    factory.keep_awake.answer(&factory.store, on);
    let plugged_in = factory.keep_awake.machine.plugged_in();
    let inner = factory.lock();
    factory.keep_awake.follow(&factory.store, working(&inner), plugged_in);
    factory.broadcast_workspace(&inner);
    // A laptop's power source is watched from now on, or no more.
    factory.keep_awake.runs_changed();
    Ok(())
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;

    fn store() -> (std::path::PathBuf, Store) {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let next = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!("keep-awake-test-{}-{next}.db", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let store = Store::open(&path).expect("store");
        (path, store)
    }

    const LAPTOP: Machine = Machine { available: true, laptop: true, power: Power::Mains, lid: false };
    const DESKTOP: Machine = Machine { available: true, laptop: false, power: Power::Mains, lid: false };

    #[test]
    fn a_laptop_asks_once_agents_work_and_a_yes_holds_it_awake_only_while_they_do_on_power() {
        let (_dir, store) = store();
        let keep = KeepAwake::load(&store, LAPTOP, false);
        assert!(!keep.view("Studio").ask && !keep.view("Studio").on, "nothing has worked yet, and a laptop is off until asked");
        assert!(keep.follow(&store, false, true), "plugged in, as looked at");
        assert!(keep.view("Studio").plugged_in && !keep.view("Studio").ask && !keep.view("Studio").holding);
        assert!(keep.follow(&store, true, true));
        let asking = keep.view("Studio");
        assert!(asking.ask && !asking.answered && !asking.holding, "{asking:?}");
        assert!(!keep.follow(&store, true, true), "asked once");
        // Asked across a restart too, until answered.
        assert!(KeepAwake::load(&store, LAPTOP, false).view("Studio").ask);

        keep.answer(&store, true);
        assert!(keep.follow(&store, true, true));
        let held = keep.view("Studio");
        assert!(held.on && held.answered && held.holding && held.plugged_in && !held.ask, "{held:?}");
        assert!(keep.watching_power(true), "unplugged, it lets go");
        // On battery it lets go, and holds again once plugged in.
        assert!(keep.follow(&store, true, false));
        let battery = keep.view("Studio");
        assert!(battery.on && !battery.holding && !battery.plugged_in, "{battery:?}");
        assert!(keep.follow(&store, true, true));
        assert!(keep.view("Studio").holding);
        assert!(keep.follow(&store, false, true));
        assert!(!keep.view("Studio").holding, "no agent works");
        assert!(!keep.watching_power(false));

        keep.answer(&store, false);
        assert!(!keep.follow(&store, true, true), "a no holds nothing");
        assert!(!keep.watching_power(true));
        let kept = KeepAwake::load(&store, LAPTOP, false).view("Studio");
        assert!(kept.answered && !kept.on && !kept.ask, "{kept:?}");
    }

    #[test]
    fn a_desktop_holds_by_default_never_asks_and_a_no_is_kept() {
        let (_dir, store) = store();
        let desktop = KeepAwake::load(&store, DESKTOP, false);
        let first = desktop.view("Studio");
        assert!(first.on && !first.answered && !first.ask && first.plugged_in, "{first:?}");
        assert!(desktop.follow(&store, true, true));
        assert!(desktop.view("Studio").holding && !desktop.view("Studio").ask);
        assert!(!desktop.watching_power(true), "a desktop's power isn't watched");
        desktop.answer(&store, false);
        assert!(desktop.follow(&store, true, true));
        assert!(!desktop.view("Studio").holding);
        drop(desktop);
        let kept = KeepAwake::load(&store, DESKTOP, false).view("Studio");
        assert!(!kept.on && kept.answered, "{kept:?}");
        let elsewhere = KeepAwake::load(&store, Machine::default(), false);
        assert!(!elsewhere.follow(&store, true, true));
        let nothing = elsewhere.view("Studio");
        assert!(!nothing.available && !nothing.on && !nothing.holding && !nothing.plugged_in, "{nothing:?}");
    }

    #[test]
    fn a_simulated_laptop_is_plugged_in_while_its_file_says_ac() {
        let file = std::env::temp_dir().join(format!("keep-awake-power-{}", std::process::id()));
        let laptop = Machine { available: true, laptop: true, power: Power::File(file.clone()), lid: false };
        assert!(!laptop.plugged_in(), "no file: on battery");
        std::fs::write(&file, "ac\n").expect("write");
        assert!(laptop.plugged_in());
        std::fs::write(&file, "battery").expect("write");
        assert!(!laptop.plugged_in());
        let _ = std::fs::remove_file(&file);
    }
}
