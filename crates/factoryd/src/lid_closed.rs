//! Keeping a laptop's agents working with its lid closed.
//!
//! macOS sleeps a laptop when its lid closes unless a bit of the kernel's
//! power manager says not to: IOPMrootDomain's clamshell sleep disable
//! (`kPMSetClamshellSleepState`), which any process of the person's may set,
//! so no root helper and nothing to install. The bit lives only in the
//! kernel's memory: a restart always clears it, and nothing of it is kept
//! on disk. factoryd sets it while the person has it on, agents work and the
//! Mac is plugged in (`crate::keep_awake`'s `caffeinate -s` holds idle sleep
//! off meanwhile), sets it again at every 30-second look (powerd may set it
//! back), and clears it when that ends, the Mac gets too hot (until 10
//! minutes after it cools), or factoryd stops.
//!
//! factory.db records that factoryd set the bit before it does, and forgets
//! only once it's cleared, so a factoryd that died holding clears it at its
//! next start. The person's own SleepDisabled (`pmset disablesleep`) holds
//! the same bit through powerd, so it's read just before the first set: if
//! it's on, factoryd sets nothing and leaves it to them; otherwise the bit
//! is its own and always cleared, whatever `pmset` says later,
//! which may be factoryd's own bit showing. Should clearing fail, or an old rehearsal's SleepDisabled be
//! on while it's on, the setting says the Mac can't sleep (STUCK) with what
//! fixes it, whatever else it says, and Needs you tells the person
//! (`crate::notify`). Only the real Claktory sets the bit: Claktory Dev and
//! tests never do, so they can't undo its hold.
//!
//! Off unless the person turns it on, in the Mac app on that Mac, after a
//! question about it alone. Only the serve path changes it: no tool, no
//! command.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use factory_protocol::v1::{self, LidClosedState as State};
use rusqlite::{OptionalExtension, params};

use crate::factory::{Factory, RequestError};
use crate::store::Store;

pub(crate) const SCHEMA: &str = "
-- Keeping a laptop's agents working with its lid closed (#371): whether the
-- person turned it on, and what macOS last told the Mac app of its helper
-- (LidClosedRegistration, and its error), for what the setting shows.
CREATE TABLE lid_closed (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    turned_on INTEGER NOT NULL,
    registration INTEGER NOT NULL,
    error TEXT NOT NULL
);
";

/// factoryd set the kernel's clamshell bit and hasn't cleared it yet: what
/// the next start clears (the helper's columns are unused).
/// 0 none; 1 factoryd's own (`OURS`), always cleared; 2 the person's own
/// SleepDisabled was on before (`THEIRS`): nothing set, nothing cleared.
pub(crate) const SET_BIT: &str = "
ALTER TABLE lid_closed ADD COLUMN set_bit INTEGER NOT NULL DEFAULT 0;
";

/// `set_bit`: factoryd set the bit, its own to clear.
const OURS: i64 = 1;
/// `set_bit`: the person's own SleepDisabled was on when factoryd would
/// have set it, so it didn't, and never clears it.
const THEIRS: i64 = 2;

/// How long the Mac must have cooled before it's held again.
const COOL: Duration = Duration::from_secs(10 * 60);

/// What fixes a bit that couldn't be cleared, `{machine}` its name: Needs
/// you shows it on every device, so "this Mac" would point at the wrong one.
const RESTART: &str = "Restart {machine} to let it sleep with the lid closed.";
/// What fixes an old rehearsal's, or the person's own, SleepDisabled.
const PMSET: &str = "In Terminal on {machine}, run `sudo pmset -a disablesleep 0`, or restart it.";

/// The kernel's power manager, as far as the lid goes: the real one, or a
/// test's.
pub trait Clamshell: Send + Sync {
    /// Sets (`true`) or clears the bit that keeps a closed lid from
    /// sleeping the Mac; why not, when the kernel refused.
    fn keep_awake(&self, on: bool) -> Result<(), String>;
    /// Whether the system's SleepDisabled setting is on (`pmset -g`; not
    /// listed is off); None when it can't be read.
    fn sleep_disabled(&self) -> Option<bool>;
    /// Whether the Mac is too hot: thermal pressure heavy or worse.
    fn hot(&self) -> bool;
}

/// The real one (native/clamshell.c).
struct Kernel;

#[cfg(target_os = "macos")]
mod native {
    unsafe extern "C" {
        pub(super) fn claktory_clamshell_sleep_disable(disable: i32) -> i32;
        pub(super) fn claktory_thermal_pressure() -> i32;
    }
}

impl Clamshell for Kernel {
    #[cfg(target_os = "macos")]
    fn keep_awake(&self, on: bool) -> Result<(), String> {
        // SAFETY: takes an int, returns a kern_return_t; nothing shared.
        let result = unsafe { native::claktory_clamshell_sleep_disable(i32::from(on)) };
        match result {
            0 => Ok(()),
            code => Err(format!("macOS refused to keep the lid from sleeping the Mac (0x{code:x}).")),
        }
    }

    #[cfg(not(target_os = "macos"))]
    fn keep_awake(&self, _on: bool) -> Result<(), String> {
        Err("Only a Mac can be kept awake with its lid closed.".into())
    }

    fn sleep_disabled(&self) -> Option<bool> {
        let output = std::process::Command::new("/usr/bin/pmset").arg("-g").output().ok().filter(|output| output.status.success())?;
        sleep_disabled_in(&String::from_utf8_lossy(&output.stdout))
    }

    #[cfg(target_os = "macos")]
    fn hot(&self) -> bool {
        // SAFETY: no arguments; reads a notify state.
        let native = unsafe { native::claktory_thermal_pressure() };
        // `FACTORY_THERMAL_PRESSURE`, a file's path, says a level for QA.
        let file = std::env::var_os("FACTORY_THERMAL_PRESSURE")
            .filter(|file| !file.is_empty())
            .map_or(-1, |file| level_in(std::path::Path::new(&file)));
        let level = effective_level(native, file);
        if level < 0 {
            static SAID: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
            if !SAID.swap(true, std::sync::atomic::Ordering::Relaxed) {
                tracing::warn!("can't read the Mac's thermal pressure, so the lid isn't kept from sleeping it");
            }
        }
        too_hot(level)
    }

    #[cfg(not(target_os = "macos"))]
    fn hot(&self) -> bool {
        false
    }
}

/// Whether `pmset -g`'s output says SleepDisabled is on: not listed is
/// off; None when its value isn't 0 or 1.
fn sleep_disabled_in(output: &str) -> Option<bool> {
    let value = output.lines().find_map(|line| {
        let mut words = line.split_whitespace();
        (words.next() == Some("SleepDisabled")).then(|| words.next().unwrap_or_default().to_owned())
    });
    match value.as_deref() {
        None | Some("0") => Some(false),
        Some("1") => Some(true),
        Some(_) => None,
    }
}

/// The thermal pressure level a QA file says; -1, unreadable, when it says
/// none.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn level_in(file: &std::path::Path) -> i32 {
    std::fs::read_to_string(file).ok().and_then(|said| said.trim().parse().ok()).unwrap_or(-1)
}

/// The level to go by: the Mac's own, which QA's file can only raise, so
/// it never switches the heat guard off; the file's alone only where the
/// Mac has none to read (a VM).
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn effective_level(native: i32, file: i32) -> i32 {
    if native >= 0 { native.max(file) } else { file }
}

/// Whether a thermal pressure level is too hot to hold: 0 nominal, 1
/// moderate, 2 heavy, 3 trapping, 4 sleeping; -1, unreadable, counts as
/// hot, so nothing is held blind.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn too_hot(level: i32) -> bool {
    !(0..2).contains(&level)
}

#[derive(Default)]
struct Inner {
    /// The person turned it on.
    on: bool,
    /// factoryd set the bit and hasn't cleared it (also in factory.db).
    set_bit: bool,
    /// The person's own SleepDisabled was on when it would have set the
    /// bit, so it didn't (also in factory.db, as `THEIRS`).
    theirs: bool,
    /// The bit is set for agents now.
    holding: bool,
    /// Why setting the bit failed; empty when it didn't.
    error: String,
    /// The Mac is too hot now.
    hot: bool,
    /// When it last cooled down after being too hot.
    cooled: Option<Instant>,
    /// What fixes the Mac's being unable to sleep; None when it can.
    stuck: Option<&'static str>,
}

impl Inner {
    fn paused_for_heat(&self) -> bool {
        self.hot || self.cooled.is_some_and(|at| at.elapsed() < COOL)
    }
}

pub(crate) struct Lid {
    /// A laptop on macOS 14 or later, in the real Claktory.
    available: AtomicBool,
    inner: Mutex<Inner>,
    clamshell: Mutex<Arc<dyn Clamshell>>,
}

impl Lid {
    /// Reads the setting, and clears a bit a factoryd before this one set
    /// and didn't clear. `available`: this factoryd may set the bit;
    /// `real`: it's the real Claktory, whose record is its own.
    pub(crate) fn load(store: &Store, available: bool, real: bool) -> Lid {
        Lid::load_on(store, available, real, Arc::new(Kernel))
    }

    fn load_on(store: &Store, available: bool, real: bool, clamshell: Arc<dyn Clamshell>) -> Lid {
        let read = store
            .db()
            .query_row("SELECT turned_on, set_bit FROM lid_closed WHERE id = 1", [], |row| {
                Ok((row.get::<_, bool>(0)?, row.get::<_, i64>(1)?))
            })
            .optional();
        let (on, set_bit) = match read {
            Ok(found) => found.unwrap_or_default(),
            Err(error) => {
                tracing::error!(%error, "could not read whether agents keep working with the lid closed");
                (false, 0)
            }
        };
        let theirs = set_bit == THEIRS;
        let set_bit = set_bit == OURS;
        let lid = Lid {
            available: AtomicBool::new(available),
            inner: Mutex::new(Inner { on: on && available, set_bit: set_bit && real, ..Inner::default() }),
            clamshell: Mutex::new(clamshell),
        };
        // The real Claktory's record is its own: cleared even if this start
        // doesn't take the Mac for a laptop (a misread `pmset`), as clearing
        // its own bit is harmless anywhere. Anywhere else (Claktory Dev, a
        // test, a home copied from the real one) never set it, and clearing
        // would undo the real one's hold, so only the record goes.
        if set_bit && real {
            tracing::info!("the lid was kept from sleeping the Mac when Claktory last stopped: letting go");
            lid.let_go(store);
        } else if set_bit {
            tracing::info!("forgetting a lid-closed hold this Claktory can't have made, without touching the Mac's sleep");
            record(store, 0);
        } else if theirs {
            // The person's own SleepDisabled: the kernel is left alone.
            record(store, 0);
        }
        lid
    }

    fn inner(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn clamshell(&self) -> Arc<dyn Clamshell> {
        self.clamshell.lock().unwrap_or_else(std::sync::PoisonError::into_inner).clone()
    }

    /// A test's kernel in place of the real one, on a laptop.
    pub(crate) fn set_clamshell(&self, clamshell: Arc<dyn Clamshell>) {
        *self.clamshell.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = clamshell;
        self.available.store(true, Ordering::Relaxed);
    }

    pub(crate) fn available(&self) -> bool {
        self.available.load(Ordering::Relaxed)
    }

    /// Whether it's to be looked at every 30 seconds: on, or the Mac can't
    /// sleep.
    pub(crate) fn watched(&self) -> bool {
        let inner = self.inner();
        inner.on || inner.set_bit || inner.stuck.is_some()
    }

    /// KeepAwake's lid-closed fields.
    /// `machine`: this Claktory's name, in the fix.
    pub(crate) fn view(&self, keep: &mut v1::KeepAwake, machine: &str) {
        let inner = self.inner();
        keep.lid_closed_available = self.available();
        let state = state(&inner);
        keep.lid_closed = state.into();
        keep.lid_closed_error = if state == State::LidClosedFailed { inner.error.clone() } else { String::new() };
        keep.lid_closed_holding = state == State::LidClosedOn && inner.holding;
        keep.lid_closed_fix = inner.stuck.unwrap_or_default().replace("{machine}", machine);
        keep.lid_closed_turned_on = inner.on;
    }

    /// SetLidClosed: kept.
    pub(crate) fn set(&self, store: &Store, on: bool) {
        let mut inner = self.inner();
        inner.on = on;
        inner.error.clear();
        let saved = store.db().execute(
            "INSERT INTO lid_closed (id, turned_on, registration, error) VALUES (1, ?1, 0, '')
             ON CONFLICT (id) DO UPDATE SET turned_on = excluded.turned_on",
            params![on],
        );
        if let Err(error) = saved {
            tracing::error!(%error, "could not keep whether agents keep working with the lid closed");
        }
    }

    /// Sets the bit while `working` (agents work, on power), it's on and
    /// the Mac isn't too hot, else clears one it set; and looks whether the
    /// Mac can sleep. Never under Factory's lock. Whether anything shown
    /// changed.
    pub(crate) fn step(&self, store: &Store, working: bool) -> bool {
        let clamshell = self.clamshell();
        let before = self.shown();
        let (on, set_bit) = {
            let inner = self.inner();
            (inner.on, inner.set_bit)
        };
        if on {
            let hot = clamshell.hot();
            let mut inner = self.inner();
            if inner.hot && !hot {
                inner.cooled = Some(Instant::now());
            }
            inner.hot = hot;
        }
        let hold = on && working && !self.inner().paused_for_heat();
        // Before the first set: the person's own SleepDisabled, read now,
        // says whether the bit would be theirs; unreadable counts as off,
        // so the bit is factoryd's and cleared later. Recorded before it's
        // set, so a factoryd that dies holding has the next one clear it.
        let mark = (hold && !set_bit).then(|| if clamshell.sleep_disabled() == Some(true) { THEIRS } else { OURS });
        if mark.is_some_and(|mark| !record(store, mark)) {
            let mut inner = self.inner();
            inner.holding = false;
            inner.error = "Claktory couldn't note that it keeps the lid from sleeping the Mac, so it doesn't.".to_owned();
        } else if mark == Some(THEIRS) {
            // Their setting already keeps the lid from sleeping the Mac:
            // nothing set, and nothing of theirs ever cleared.
            let mut inner = self.inner();
            inner.theirs = true;
            inner.holding = false;
            inner.error.clear();
        } else if hold {
            if mark.is_some() {
                let mut inner = self.inner();
                inner.set_bit = true;
                inner.theirs = false;
            }
            // Set again at every look: powerd may have cleared it.
            let result = clamshell.keep_awake(true);
            let mut inner = self.inner();
            inner.holding = result.is_ok();
            inner.error = result.err().unwrap_or_default();
        } else if set_bit {
            self.let_go(store);
        } else if std::mem::take(&mut self.inner().theirs) {
            record(store, 0);
        }
        // SleepDisabled (an old rehearsal's, or the person's own), while
        // it's on and not holding, or still after it's turned off, so it
        // never shows off while the Mac can't sleep with its lid closed.
        let look = {
            let inner = self.inner();
            (inner.on || inner.stuck == Some(PMSET)) && !inner.holding
        };
        let sleep_disabled = look && clamshell.sleep_disabled() == Some(true);
        {
            let mut inner = self.inner();
            match inner.stuck {
                Some(RESTART) => {}
                _ => inner.stuck = sleep_disabled.then_some(PMSET),
            }
        }
        let after = self.shown();
        if before != after {
            tracing::info!(state = ?after.0, holding = after.1, "keeping the lid from sleeping the Mac");
        }
        before != after
    }

    /// Clears the bit factoryd set, and forgets it once cleared; a clear
    /// the kernel refuses leaves the Mac unable to sleep with its lid
    /// closed until a restart (STUCK).
    /// Always cleared, whatever `pmset` says now: SleepDisabled was off
    /// before factoryd set the bit, so on now may be that bit showing (one
    /// that was the person's own was never set: `THEIRS`).
    pub(crate) fn let_go(&self, store: &Store) {
        let result = self.clamshell().keep_awake(false);
        let mut inner = self.inner();
        inner.holding = false;
        match result {
            Ok(()) => {
                if record(store, 0) {
                    inner.set_bit = false;
                }
                if inner.stuck == Some(RESTART) {
                    inner.stuck = None;
                }
            }
            Err(error) => {
                tracing::error!(%error, "could not let the lid sleep the Mac again");
                inner.stuck = Some(RESTART);
            }
        }
    }

    /// Lets go before factoryd stops.
    pub(crate) fn stop(&self, store: &Store) {
        if self.inner().set_bit {
            self.let_go(store);
        }
    }

    /// What the setting shows: its state, holding and the fix.
    fn shown(&self) -> (State, bool, Option<&'static str>) {
        let inner = self.inner();
        (state(&inner), inner.holding, inner.stuck)
    }
}

/// Records in factory.db whether factoryd set the bit (0, `OURS` or
/// `THEIRS`); whether it's kept.
fn record(store: &Store, set: i64) -> bool {
    let saved = store.db().execute(
        "INSERT INTO lid_closed (id, turned_on, registration, error, set_bit) VALUES (1, 0, 0, '', ?1)
         ON CONFLICT (id) DO UPDATE SET set_bit = excluded.set_bit",
        params![set],
    );
    match saved {
        Ok(_) => true,
        Err(error) => {
            tracing::error!(%error, set, "could not note whether the lid is kept from sleeping the Mac");
            false
        }
    }
}

/// SetLidClosed. Turning it on is the person's, in the Mac app on this Mac
/// (`local`: a connection authenticated as this machine's own device, by
/// the local pairing code's token or key, whether on the loopback port or
/// through this machine's own tunnel endpoint as the Mac app connects; never
/// a paired device), after its own question; turning it off is taken from
/// anywhere. What older apps send of a helper (registration, error) is
/// ignored: there's none.
pub(crate) fn set(factory: &Arc<Factory>, local: bool, request: &v1::SetLidClosed) -> Result<(), RequestError> {
    let refuse = |why: &str| Err(RequestError::Account(v1::ErrorCode::InvalidRequest, why.to_owned()));
    let lid = &factory.keep_awake.lid;
    if request.on && !lid.available() {
        return refuse("Only a laptop on macOS 14 or later can keep agents working with its lid closed.");
    }
    if request.on && !local {
        return refuse("Turn it on from the Mac.");
    }
    lid.set(&factory.store, request.on);
    // Held or let go now, not at the next look.
    factory.keep_awake.runs_changed();
    factory.broadcast_workspace(&factory.lock());
    Ok(())
}

/// Where it stands: unable to sleep first, whatever else.
fn state(inner: &Inner) -> State {
    if inner.stuck.is_some() {
        State::LidClosedStuck
    } else if !inner.on {
        State::LidClosedOff
    } else if !inner.error.is_empty() {
        State::LidClosedFailed
    } else if inner.paused_for_heat() {
        State::LidClosedPausedHeat
    } else {
        State::LidClosedOn
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A kernel that does as told, and keeps what it was asked.
    #[derive(Default)]
    struct Fake {
        /// The bit as set.
        bit: Mutex<bool>,
        asked: Mutex<Vec<bool>>,
        refuses: AtomicBool,
        hot: AtomicBool,
        /// What `pmset` says of SleepDisabled; None, unreadable.
        sleep_disabled: Mutex<Option<bool>>,
    }

    impl Clamshell for Fake {
        fn keep_awake(&self, on: bool) -> Result<(), String> {
            self.asked.lock().unwrap().push(on);
            if self.refuses.load(Ordering::Relaxed) {
                return Err("refused".into());
            }
            *self.bit.lock().unwrap() = on;
            Ok(())
        }
        fn sleep_disabled(&self) -> Option<bool> {
            *self.sleep_disabled.lock().unwrap()
        }
        fn hot(&self) -> bool {
            self.hot.load(Ordering::Relaxed)
        }
    }

    fn store() -> Store {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let next = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!("lid-closed-test-{}-{next}.db", std::process::id()));
        let _ = std::fs::remove_file(&path);
        Store::open(&path).expect("store")
    }

    fn view(lid: &Lid) -> v1::KeepAwake {
        let mut keep = v1::KeepAwake::default();
        lid.view(&mut keep, "Studio");
        keep
    }

    fn recorded(store: &Store) -> i64 {
        store.db().query_row("SELECT set_bit FROM lid_closed WHERE id = 1", [], |row| row.get(0)).optional().unwrap().unwrap_or(0)
    }

    impl Fake {
        fn pmset(&self, said: Option<bool>) {
            *self.sleep_disabled.lock().unwrap() = said;
        }
    }

    #[test]
    fn it_sets_the_bit_only_while_agents_work_with_it_on_and_records_it_first() {
        let store = store();
        let lid = Lid::load(&store, true, false);
        let fake = Arc::new(Fake::default());
        lid.set_clamshell(fake.clone());
        assert_eq!(view(&lid).lid_closed(), State::LidClosedOff);
        assert!(!lid.step(&store, true), "off: nothing set");
        assert!(fake.asked.lock().unwrap().is_empty());

        lid.set(&store, true);
        lid.step(&store, false);
        assert!(fake.asked.lock().unwrap().is_empty() && !view(&lid).lid_closed_holding, "no agent works");
        assert!(lid.step(&store, true));
        assert!(*fake.bit.lock().unwrap() && view(&lid).lid_closed_holding && recorded(&store) == OURS);
        // Set again at every look, as powerd may clear it.
        lid.step(&store, true);
        assert_eq!(*fake.asked.lock().unwrap(), [true, true]);
        // Agents stop: cleared and forgotten.
        assert!(lid.step(&store, false));
        assert!(!*fake.bit.lock().unwrap() && recorded(&store) == 0 && view(&lid).lid_closed() == State::LidClosedOn);

        // Hot: let go, and held again only 10 minutes after it cools.
        lid.step(&store, true);
        fake.hot.store(true, Ordering::Relaxed);
        lid.step(&store, true);
        assert_eq!(view(&lid).lid_closed(), State::LidClosedPausedHeat);
        assert!(!*fake.bit.lock().unwrap());
        fake.hot.store(false, Ordering::Relaxed);
        lid.step(&store, true);
        assert_eq!(view(&lid).lid_closed(), State::LidClosedPausedHeat, "cooling down");
        lid.inner().cooled = Some(Instant::now() - COOL);
        lid.step(&store, true);
        assert!(*fake.bit.lock().unwrap() && view(&lid).lid_closed_holding);

        // Off while holding: let go.
        lid.set(&store, false);
        lid.step(&store, true);
        assert!(!*fake.bit.lock().unwrap() && view(&lid).lid_closed() == State::LidClosedOff);
    }

    #[test]
    fn a_bit_left_set_is_cleared_at_the_next_start_and_one_that_cant_be_is_stuck() {
        let store = store();
        record(&store, OURS);
        // Where it can't have been set (Claktory Dev, a home copied from
        // the real one): only the record goes, the kernel untouched, so
        // the real Claktory's hold stays.
        let fake = Arc::new(Fake::default());
        let lid = Lid::load_on(&store, false, false, fake.clone());
        assert!(recorded(&store) == 0 && !lid.watched());
        assert!(fake.asked.lock().unwrap().is_empty());
        // The real one's, even when this start doesn't take the Mac for a
        // laptop: cleared.
        record(&store, OURS);
        *fake.bit.lock().unwrap() = true;
        let lid = Lid::load_on(&store, false, true, fake.clone());
        assert_eq!(*fake.asked.lock().unwrap(), [false]);
        assert!(!*fake.bit.lock().unwrap() && recorded(&store) == 0);
        // A clear the kernel refuses at start: stuck, the record kept.
        record(&store, OURS);
        fake.refuses.store(true, Ordering::Relaxed);
        drop(lid);
        let lid = Lid::load_on(&store, true, true, fake.clone());
        let keep = view(&lid);
        assert_eq!(keep.lid_closed(), State::LidClosedStuck);
        assert_eq!(keep.lid_closed_fix, "Restart Studio to let it sleep with the lid closed.");
        assert!(recorded(&store) == OURS, "still to clear");
        assert!(lid.watched());
        // It clears at a later look: no longer stuck.
        fake.refuses.store(false, Ordering::Relaxed);
        lid.step(&store, false);
        assert_eq!(view(&lid).lid_closed(), State::LidClosedOff);
        assert!(recorded(&store) == 0);

        // An old rehearsal's SleepDisabled, with it on: stuck, with pmset.
        lid.set(&store, true);
        fake.pmset(Some(true));
        lid.step(&store, false);
        assert_eq!(view(&lid).lid_closed(), State::LidClosedStuck);
        assert_eq!(view(&lid).lid_closed_fix, "In Terminal on Studio, run `sudo pmset -a disablesleep 0`, or restart it.");
        assert!(view(&lid).lid_closed_turned_on, "the switch shows the setting");
        // Turned off while stuck: still stuck until it can sleep, then off.
        lid.set(&store, false);
        lid.step(&store, false);
        assert_eq!(view(&lid).lid_closed(), State::LidClosedStuck);
        assert!(!view(&lid).lid_closed_turned_on && lid.watched());
        fake.pmset(Some(false));
        lid.step(&store, false);
        assert_eq!(view(&lid).lid_closed(), State::LidClosedOff);
        assert!(!lid.watched());
        // On again while stuck is taken.
        fake.pmset(Some(true));
        lid.set(&store, true);
        lid.step(&store, false);
        assert_eq!(view(&lid).lid_closed(), State::LidClosedStuck);
        fake.pmset(Some(false));
        lid.step(&store, false);
        assert_eq!(view(&lid).lid_closed(), State::LidClosedOn);

        // SleepDisabled on while holding may be factoryd's own bit
        // showing: letting go clears it all the same.
        lid.step(&store, true);
        assert!(*fake.bit.lock().unwrap() && recorded(&store) == OURS);
        fake.pmset(Some(true));
        lid.step(&store, false);
        assert_eq!(fake.asked.lock().unwrap().last(), Some(&false), "cleared");
        assert!(!*fake.bit.lock().unwrap() && recorded(&store) == 0 && !view(&lid).lid_closed_holding);
    }

    /// SleepDisabled is read just before the first set, and
    /// recorded with it; the bit is cleared unless it was the person's own
    /// before.
    #[test]
    fn the_persons_own_sleep_disabled_is_read_before_the_first_set() {
        // Off before: set, recorded as factoryd's, cleared on letting go.
        let store = store();
        let lid = Lid::load(&store, true, false);
        let fake = Arc::new(Fake::default());
        lid.set_clamshell(fake.clone());
        fake.pmset(Some(false));
        lid.set(&store, true);
        lid.step(&store, true);
        assert!(*fake.bit.lock().unwrap() && recorded(&store) == OURS);
        // pmset now says on (perhaps the bit itself): still factoryd's.
        fake.pmset(Some(true));
        lid.step(&store, true);
        assert!(recorded(&store) == OURS && view(&lid).lid_closed_holding);
        lid.step(&store, false);
        assert!(!*fake.bit.lock().unwrap() && recorded(&store) == 0);
        // Cleared, pmset no longer says on: it was the bit showing.
        fake.pmset(Some(false));
        // Letting go twice is harmless.
        lid.let_go(&store);
        lid.let_go(&store);
        assert!(fake.asked.lock().unwrap().ends_with(&[false, false, false]));
        lid.step(&store, false);
        assert!(!*fake.bit.lock().unwrap() && recorded(&store) == 0 && view(&lid).lid_closed() == State::LidClosedOn);
        // ...and at the next start, after one that died holding.
        lid.step(&store, true);
        assert_eq!(recorded(&store), OURS);
        drop(lid);
        let asked = fake.asked.lock().unwrap().len();
        let _lid = Lid::load_on(&store, true, true, fake.clone());
        assert_eq!(fake.asked.lock().unwrap()[asked..], [false]);
        assert!(!*fake.bit.lock().unwrap() && recorded(&store) == 0);

        // On before: the person's own. Never set, never cleared, at a look
        // or at the next start; recorded as theirs while it would hold.
        let store = self::store();
        let lid = Lid::load(&store, true, false);
        let fake = Arc::new(Fake::default());
        lid.set_clamshell(fake.clone());
        fake.pmset(Some(true));
        lid.set(&store, true);
        lid.step(&store, true);
        lid.step(&store, true);
        assert!(fake.asked.lock().unwrap().is_empty() && recorded(&store) == THEIRS);
        assert!(!view(&lid).lid_closed_holding);
        drop(lid);
        let lid = Lid::load_on(&store, true, true, fake.clone());
        assert!(fake.asked.lock().unwrap().is_empty() && recorded(&store) == 0);
        // Once they turn theirs off, the next look sets factoryd's own.
        lid.step(&store, true);
        assert!(fake.asked.lock().unwrap().is_empty());
        fake.pmset(Some(false));
        lid.step(&store, true);
        assert!(*fake.bit.lock().unwrap() && recorded(&store) == OURS);
        lid.stop(&store);
        assert!(!*fake.bit.lock().unwrap() && recorded(&store) == 0);
        // Theirs, then no agent works: only the record goes.
        fake.pmset(Some(true));
        lid.step(&store, true);
        assert_eq!(recorded(&store), THEIRS);
        let asked = fake.asked.lock().unwrap().len();
        lid.step(&store, false);
        assert!(fake.asked.lock().unwrap().len() == asked && recorded(&store) == 0);

        // Unreadable: counts as factoryd's own, so it's cleared later.
        let store = self::store();
        let lid = Lid::load(&store, true, false);
        let fake = Arc::new(Fake::default());
        lid.set_clamshell(fake.clone());
        fake.pmset(None);
        lid.set(&store, true);
        lid.step(&store, true);
        assert!(*fake.bit.lock().unwrap() && recorded(&store) == OURS);
        lid.step(&store, false);
        assert!(!*fake.bit.lock().unwrap() && recorded(&store) == 0);
    }

    /// Recorded, then died before the kernel call: the next start clears.
    #[test]
    fn a_record_without_its_set_is_cleared_at_the_next_start() {
        let store = store();
        record(&store, OURS);
        let fake = Arc::new(Fake::default());
        let _lid = Lid::load_on(&store, true, true, fake.clone());
        assert_eq!(*fake.asked.lock().unwrap(), [false]);
        assert_eq!(recorded(&store), 0);
    }

    #[test]
    fn pmset_says_sleep_disabled_on_off_or_unreadable() {
        assert_eq!(sleep_disabled_in(" SleepDisabled\t\t1\n hibernatemode 3\n"), Some(true));
        assert_eq!(sleep_disabled_in(" SleepDisabled\t\t0\n"), Some(false));
        assert_eq!(sleep_disabled_in(" hibernatemode 3\n"), Some(false), "not listed is off");
        assert_eq!(sleep_disabled_in(" SleepDisabled\n"), None);
        assert_eq!(sleep_disabled_in(" SleepDisabled yes\n"), None);
    }

    #[test]
    fn an_unreadable_thermal_pressure_counts_as_hot() {
        assert!(too_hot(-1) && too_hot(2) && too_hot(4));
        assert!(!too_hot(0) && !too_hot(1));
        // QA's file only adds heat where the Mac reads its own.
        assert_eq!(effective_level(1, 0), 1);
        assert_eq!(effective_level(0, 2), 2);
        assert_eq!(effective_level(1, -1), 1);
        assert_eq!(effective_level(-1, 0), 0);
        assert_eq!(effective_level(-1, -1), -1);
        // QA's file: a level, or unreadable.
        let file = std::env::temp_dir().join(format!("lid-closed-thermal-{}", std::process::id()));
        std::fs::write(&file, "2\n").unwrap();
        assert_eq!(level_in(&file), 2);
        std::fs::write(&file, "warm").unwrap();
        assert_eq!(level_in(&file), -1);
        std::fs::remove_file(&file).unwrap();
        assert_eq!(level_in(&file), -1);
    }

    #[test]
    fn a_refused_set_says_why() {
        let store = store();
        let lid = Lid::load(&store, true, false);
        let fake = Arc::new(Fake::default());
        lid.set_clamshell(fake.clone());
        fake.refuses.store(true, Ordering::Relaxed);
        lid.set(&store, true);
        lid.step(&store, true);
        let keep = view(&lid);
        assert_eq!(keep.lid_closed(), State::LidClosedFailed);
        assert_eq!(keep.lid_closed_error, "refused");
        assert!(!keep.lid_closed_holding);
    }
}
