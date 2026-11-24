//! Keeping a laptop's agents working with its lid closed (#371; design:
//! coordination/security/lid-closed.md). macOS lets only root stop a
//! closed-lid sleep, so a helper does it: a root launchd daemon the Mac app
//! installs with SMAppService, whose whole power is the system's
//! SleepDisabled setting, within limits it enforces itself (low battery,
//! heat, the lid closed on battery). factoryd asks it, over XPC, to hold the
//! Mac awake for a 10-minute lease, renewed every 4 minutes while agents
//! work and the person has it on, and lets go when the last one stops.
//!
//! Off unless the person turns it on, in the Mac app on that Mac, from a
//! question about it alone. What macOS told the app of the helper
//! (SetLidClosed.registration) is shown, never trusted: holding, and the
//! setting's being on, come only from the helper's own answers. Only the
//! serve path reaches the helper: no tool, no command.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use factory_protocol::v1::{self, LidClosedRegistration as Registration, LidClosedState as State};
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

/// The contract factoryd and the helper speak, and the helper's build it
/// was written with (app/macos/LidHelper/main.swift's `contract`, `build`).
const CONTRACT: i64 = 1;
const HELPER_BUILD: i64 = 1;

/// A hold is renewed this often while agents work; the helper's lease is
/// 10 minutes.
const RENEW: Duration = Duration::from_secs(4 * 60);
/// How often, at most, the helper is asked how it is while nothing is held:
/// longer than it stays up idle (60 s), so asking doesn't keep starting it.
/// The Mac app's word of a change (SetLidClosed) asks at once.
const STATUS_EVERY: Duration = Duration::from_secs(5 * 60);

/// What's asked of the helper.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Op {
    Status = 1,
    Hold = 2,
    Release = 3,
}

/// The helper's answer.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Answer {
    /// Its contract version.
    pub contract: i64,
    /// Its own build.
    pub version: i64,
    pub holding: bool,
    /// 0 none, 1 battery, 2 heat, 3 the lid closed on battery.
    pub limit: i64,
    pub lease_ms_left: i64,
    /// Non-zero: it didn't take the request (another contract version).
    pub error: i64,
}

/// Who answers for the helper: XPC to the real one, or a test's.
pub trait Helper: Send + Sync {
    /// Its answer; None when no helper of ours answered.
    fn call(&self, op: Op) -> Option<Answer>;
}

/// The real helper, over XPC (native/lid_xpc.c), checked by its signature.
struct Xpc;

#[cfg(target_os = "macos")]
mod native {
    #[repr(C)]
    #[derive(Default)]
    pub(super) struct Reply {
        pub contract: i64,
        pub version: i64,
        pub limit: i64,
        pub lease_ms_left: i64,
        pub error: i64,
        pub holding: i32,
    }

    unsafe extern "C" {
        pub(super) fn claktory_lid_call(contract: i64, op: i64, reply: *mut Reply) -> i32;
    }
}

impl Helper for Xpc {
    #[cfg(target_os = "macos")]
    fn call(&self, op: Op) -> Option<Answer> {
        let mut reply = native::Reply::default();
        // SAFETY: the reply is ours to fill, and lives through the call.
        let result = unsafe { native::claktory_lid_call(CONTRACT, op as i64, &raw mut reply) };
        (result == 0).then_some(Answer {
            contract: reply.contract,
            version: reply.version,
            holding: reply.holding != 0,
            limit: reply.limit,
            lease_ms_left: reply.lease_ms_left,
            error: reply.error,
        })
    }

    #[cfg(not(target_os = "macos"))]
    fn call(&self, _op: Op) -> Option<Answer> {
        None
    }
}

struct Inner {
    /// The person turned it on.
    on: bool,
    registration: Registration,
    error: String,
    /// The helper's last answer; None when none came.
    answer: Option<Answer>,
    /// When it was last asked.
    asked: Option<Instant>,
    /// When the hold was last taken or renewed; None while nothing's held.
    held: Option<Instant>,
}

pub(crate) struct Lid {
    /// A laptop on macOS 14 or later.
    available: AtomicBool,
    inner: Mutex<Inner>,
    helper: Mutex<Arc<dyn Helper>>,
    /// A step is asking the helper: a hung one ties up one thread, not one
    /// a look.
    asking: AtomicBool,
}

impl Lid {
    pub(crate) fn load(store: &Store, available: bool) -> Lid {
        let read = store
            .db()
            .query_row("SELECT turned_on, registration, error FROM lid_closed WHERE id = 1", [], |row| {
                Ok((row.get::<_, bool>(0)?, row.get::<_, i32>(1)?, row.get::<_, String>(2)?))
            })
            .optional();
        let (on, registration, error) = match read {
            Ok(found) => found.unwrap_or_default(),
            Err(error) => {
                tracing::error!(%error, "could not read whether agents keep working with the lid closed");
                (false, 0, String::new())
            }
        };
        let inner = Inner {
            on: on && available,
            registration: Registration::try_from(registration).unwrap_or_default(),
            error,
            answer: None,
            asked: None,
            held: None,
        };
        Lid {
            available: AtomicBool::new(available),
            inner: Mutex::new(inner),
            helper: Mutex::new(Arc::new(Xpc)),
            asking: AtomicBool::new(false),
        }
    }

    fn inner(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn helper(&self) -> Arc<dyn Helper> {
        self.helper.lock().unwrap_or_else(std::sync::PoisonError::into_inner).clone()
    }

    /// A test's helper in place of the real one, on a laptop.
    pub(crate) fn set_helper(&self, helper: Arc<dyn Helper>) {
        *self.helper.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = helper;
        self.available.store(true, Ordering::Relaxed);
    }

    pub(crate) fn available(&self) -> bool {
        self.available.load(Ordering::Relaxed)
    }

    /// Whether it's on: then it's looked at every 30 seconds.
    pub(crate) fn on(&self) -> bool {
        self.inner().on
    }

    /// KeepAwake's lid-closed fields: available, state, error, holding.
    pub(crate) fn view(&self, keep: &mut v1::KeepAwake) {
        let inner = self.inner();
        keep.lid_closed_available = self.available();
        let state = state(&inner);
        keep.lid_closed = state.into();
        keep.lid_closed_error = if state == State::LidClosedFailed { inner.error.clone() } else { String::new() };
        keep.lid_closed_holding = state == State::LidClosedOn && inner.answer.is_some_and(|answer| answer.holding);
    }

    /// SetLidClosed: kept, with what macOS told the app.
    pub(crate) fn set(&self, store: &Store, on: bool, registration: Registration, error: &str) {
        let mut inner = self.inner();
        inner.on = on;
        inner.registration = registration;
        inner.error = if registration == Registration::Failed { clean(error) } else { String::new() };
        // Asked again at the next step.
        inner.asked = None;
        let saved = store.db().execute(
            "INSERT OR REPLACE INTO lid_closed (id, turned_on, registration, error) VALUES (1, ?1, ?2, ?3)",
            params![inner.on, inner.registration as i32, inner.error],
        );
        if let Err(error) = saved {
            tracing::error!(%error, "could not keep whether agents keep working with the lid closed");
        }
    }

    /// Holds, renews, lets go or asks how it is, as `working` and the
    /// setting say; blocking (XPC), so never under Factory's lock. Whether
    /// anything shown changed.
    pub(crate) fn step(&self, working: bool) -> bool {
        let (op, before) = {
            let inner = self.inner();
            let before = (state(&inner), inner.answer.is_some_and(|answer| answer.holding));
            let due = |since: Option<Instant>, every: Duration| since.is_none_or(|at| at.elapsed() >= every);
            let op = if !inner.on {
                // Off: let go of what's held, once.
                inner.held.is_some().then_some(Op::Release)
            } else if working {
                // Renewed on time; retried at each step while it's refused.
                due(inner.held, RENEW).then_some(Op::Hold)
            } else if inner.held.is_some() {
                Some(Op::Release)
            } else {
                due(inner.asked, STATUS_EVERY).then_some(Op::Status)
            };
            (op, before)
        };
        let Some(mut op) = op else { return false };
        if self.asking.swap(true, Ordering::AcqRel) {
            return false;
        }
        let _asked = Asking(&self.asking);
        let helper = self.helper();
        let ask = |op| helper.call(op).filter(|answer| answer.error == 0);
        let fresh = op == Op::Hold && self.inner().held.is_none();
        let mut answer = if fresh {
            // A helper from another version holds nothing of ours: asked
            // how it is first.
            let status = ask(Op::Status);
            if status.is_some_and(compatible) {
                ask(Op::Hold)
            } else {
                op = Op::Status;
                status
            }
        } else {
            ask(op)
        };
        if op == Op::Hold && answer.is_some_and(|answer| answer.holding && !compatible(answer)) {
            // Replaced meanwhile: let go.
            answer = ask(Op::Release);
            op = Op::Release;
        }
        let mut inner = self.inner();
        inner.asked = Some(Instant::now());
        inner.answer = answer;
        inner.held = match op {
            Op::Hold if answer.is_some_and(|answer| answer.holding) => Some(Instant::now()),
            Op::Hold | Op::Release => None,
            Op::Status => inner.held,
        };
        if op != Op::Status {
            tracing::info!(
                ?op,
                holding = answer.is_some_and(|answer| answer.holding),
                limit = answer.map(|answer| answer.limit),
                "the lid-closed helper"
            );
        }
        before != (state(&inner), inner.answer.is_some_and(|answer| answer.holding))
    }
}

/// SetLidClosed. Turning it on is the person's, in the Mac app on this Mac
/// (`local`: a connection authenticated as this machine's own device, by
/// the local pairing code's token or key, whether on the loopback port or
/// through this machine's own tunnel endpoint as the Mac app connects; never
/// a paired device), after its own question; turning it off is taken from
/// anywhere.
pub(crate) fn set(factory: &Arc<Factory>, local: bool, request: &v1::SetLidClosed) -> Result<(), RequestError> {
    let refuse = |why: &str| Err(RequestError::Account(v1::ErrorCode::InvalidRequest, why.to_owned()));
    let lid = &factory.keep_awake.lid;
    if request.on && !lid.available() {
        return refuse("Only a laptop on macOS 14 or later can keep agents working with its lid closed.");
    }
    if request.on && !local {
        return refuse("Turn it on from the Mac.");
    }
    lid.set(&factory.store, request.on, request.registration(), &request.error);
    // Held or let go now, not at the next look.
    factory.keep_awake.runs_changed();
    factory.broadcast_workspace(&factory.lock());
    Ok(())
}

/// Marks the helper free to ask again when a step's done with it.
struct Asking<'a>(&'a AtomicBool);

impl Drop for Asking<'_> {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}

/// The helper this factoryd was written for.
fn compatible(answer: Answer) -> bool {
    answer.contract == CONTRACT && answer.version == HELPER_BUILD
}

/// Where it stands, from the person's setting and the helper's answer,
/// in the design's order (lid-closed.md §C2).
fn state(inner: &Inner) -> State {
    if !inner.on {
        return State::LidClosedOff;
    }
    if inner.registration == Registration::Failed {
        return State::LidClosedFailed;
    }
    match inner.answer {
        Some(answer) if !compatible(answer) => State::LidClosedOutdated,
        Some(answer) => match answer.limit {
            1 => State::LidClosedPausedBattery,
            2 => State::LidClosedPausedHeat,
            3 => State::LidClosedPausedLidOnBattery,
            _ => State::LidClosedOn,
        },
        None if inner.registration == Registration::RequiresApproval => State::LidClosedNeedsApproval,
        None => State::LidClosedMissing,
    }
}

/// macOS's error as one short line for the person.
fn clean(error: &str) -> String {
    let line: String = error.chars().map(|c| if c.is_control() { ' ' } else { c }).collect();
    line.split_whitespace().collect::<Vec<_>>().join(" ").chars().take(300).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A helper that answers as told, and counts what it was asked.
    #[derive(Default)]
    struct Fake {
        answer: Mutex<Option<Answer>>,
        asked: Mutex<Vec<Op>>,
    }

    impl Helper for Fake {
        fn call(&self, op: Op) -> Option<Answer> {
            self.asked.lock().unwrap().push(op);
            let mut answer = (*self.answer.lock().unwrap())?;
            answer.holding = op == Op::Hold && answer.limit == 0 || op == Op::Status && answer.holding;
            Some(answer)
        }
    }

    const OK: Answer = Answer { contract: CONTRACT, version: HELPER_BUILD, holding: false, limit: 0, lease_ms_left: 0, error: 0 };

    fn store() -> Store {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let next = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!("lid-closed-test-{}-{next}.db", std::process::id()));
        let _ = std::fs::remove_file(&path);
        Store::open(&path).expect("store")
    }

    fn view(lid: &Lid) -> v1::KeepAwake {
        let mut keep = v1::KeepAwake::default();
        lid.view(&mut keep);
        keep
    }

    #[test]
    fn off_by_default_and_it_holds_only_while_agents_work_with_it_on() {
        let store = store();
        let lid = Lid::load(&store, true);
        let fake = Arc::new(Fake::default());
        lid.set_helper(fake.clone());
        assert_eq!(view(&lid).lid_closed(), State::LidClosedOff);
        assert!(!lid.step(true), "off: nothing asked");
        assert!(fake.asked.lock().unwrap().is_empty());

        // On, approval pending: no helper answers yet.
        lid.set(&store, true, Registration::RequiresApproval, "");
        lid.step(false);
        assert_eq!(view(&lid).lid_closed(), State::LidClosedNeedsApproval);
        // Approved: it answers; agents work, so it holds.
        *fake.answer.lock().unwrap() = Some(OK);
        lid.set(&store, true, Registration::Enabled, "");
        assert!(lid.step(true));
        let held = view(&lid);
        assert!(held.lid_closed() == State::LidClosedOn && held.lid_closed_holding, "{held:?}");
        assert!(!lid.step(true), "renewed only every 4 minutes");
        // They stop: it lets go.
        assert!(lid.step(false));
        assert!(!view(&lid).lid_closed_holding);
        assert_eq!(*fake.asked.lock().unwrap(), [Op::Status, Op::Status, Op::Hold, Op::Release]);

        // Kept across a restart; off lets go of a hold at once.
        let again = Lid::load(&store, true);
        again.set_helper(fake.clone());
        assert!(again.on());
        again.step(true);
        again.set(&store, false, Registration::NotRegistered, "");
        again.step(true);
        assert_eq!(fake.asked.lock().unwrap().last(), Some(&Op::Release));
        assert_eq!(view(&again).lid_closed(), State::LidClosedOff);
        // A desktop never has it on, whatever was kept.
        assert!(!Lid::load(&store, false).on());
    }

    #[test]
    fn the_helpers_own_answer_says_where_it_stands() {
        let store = store();
        let lid = Lid::load(&store, true);
        let fake = Arc::new(Fake::default());
        lid.set_helper(fake.clone());
        lid.set(&store, true, Registration::Enabled, "");
        lid.step(false);
        assert_eq!(view(&lid).lid_closed(), State::LidClosedMissing, "enabled, said the app; no helper answered");
        for (limit, state) in [(1, State::LidClosedPausedBattery), (2, State::LidClosedPausedHeat), (3, State::LidClosedPausedLidOnBattery)]
        {
            *fake.answer.lock().unwrap() = Some(Answer { limit, ..OK });
            lid.step(true);
            let paused = view(&lid);
            assert!(paused.lid_closed() == state && !paused.lid_closed_holding, "{paused:?}");
        }
        *fake.answer.lock().unwrap() = Some(Answer { version: HELPER_BUILD + 1, ..OK });
        let before = fake.asked.lock().unwrap().len();
        lid.step(true);
        let outdated = view(&lid);
        assert!(outdated.lid_closed() == State::LidClosedOutdated && !outdated.lid_closed_holding, "{outdated:?}");
        assert_eq!(fake.asked.lock().unwrap()[before..], [Op::Status], "another version is never asked to hold");
        lid.set(&store, true, Registration::Failed, "The operation couldn’t be completed.\n(Operation not permitted)");
        let failed = view(&lid);
        assert_eq!(failed.lid_closed(), State::LidClosedFailed);
        assert_eq!(failed.lid_closed_error, "The operation couldn’t be completed. (Operation not permitted)");
    }
}
