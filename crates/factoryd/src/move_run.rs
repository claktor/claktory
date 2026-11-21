//! Moving this Claktory from the app (#217, phase 2; docs/MOVE.md, "The
//! handshake"). StartMove dials the computer that shows a receive code, the
//! person confirms the digits (ConfirmMove), and the copy runs by itself
//! while agents keep working (`move_source::copy`). Once it settles (or on
//! FreezeMove), the backend writes `<home>/moving/state.json` and exits as
//! an update does; the supervisor starts it again in **frozen mode**
//! (`serve_frozen`): no Factory and no agents, only the move's progress for
//! the app, while it sends the last round, waits for the target to import
//! it, fences this copy and commits. Then it exits again, and the
//! supervisor finds `moved.json` (moved mode), or, if the move failed
//! before the commit, no `moving/` (the Claktory as it was, which shows why
//! from `move-failed.json`).
//!
//! The view (`Workspace.move`) lives in `Moving`; state that must outlive a
//! restart lives in `moving/state.json`.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, Weak};
use std::time::{Duration, Instant};

use factory_protocol::v1;
use futures_util::{SinkExt, StreamExt};
use prost::Message as _;
use serde::{Deserialize, Serialize};
use tokio::net::TcpListener;
use tokio::sync::{mpsc, watch};
use tokio_tungstenite::tungstenite::Message;

use crate::factory::{Factory, RequestError};
use crate::home::Home;
use crate::move_export::{self, Moved, Options, Place, Service};
use crate::move_source::{self, Checked, Link, Mover, Person, Problem, Rounded};
use crate::move_wire::ReceiveCode;

use v1::MovePhase as Phase;

/// The phase `moving/state.json` keeps.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Stage {
    /// Pre-copy, in the normal backend: a restart carries on copying.
    #[default]
    Copying,
    /// The backend restarts into frozen mode for the last round.
    Frozen,
    /// Fenced, with the commit said or about to be: frozen mode says it again
    /// until the target answers.
    Committing,
}

/// `<home>/moving/state.json`: a move past pairing.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Saved {
    pub stage: Stage,
    /// The receive code, as given.
    pub code: String,
    pub export_id: String,
    /// The next round's number.
    pub round: u32,
    pub epoch: u64,
    /// The commit's time, once committing.
    pub at_ms: i64,
    /// What the app was shown, to show again after the restart.
    pub target_machine: String,
    pub target_os: String,
    pub target_arch: String,
    pub target_version: String,
    pub map: Vec<(String, String)>,
    pub missing: Vec<String>,
    pub plan_bytes: u64,
    pub bytes_have: u64,
    pub bytes_total: u64,
}

fn state_path(home: &Home) -> PathBuf {
    move_source::moving_dir(home).join("state.json")
}

fn failed_path(home: &Home) -> PathBuf {
    home.root().join("move-failed.json")
}

/// The move's saved state, if one is past pairing.
pub fn saved(home: &Home) -> Option<Saved> {
    let text = std::fs::read_to_string(state_path(home)).ok()?;
    serde_json::from_str(&text).ok()
}

fn save(home: &Home, saved: &Saved) -> Result<(), String> {
    let dir = move_source::moving_dir(home);
    crate::remote::private_dir(&dir).map_err(|error| format!("{}: {error}", dir.display()))?;
    let json = serde_json::to_vec_pretty(saved).map_err(|error| error.to_string())?;
    crate::remote::write_private(&state_path(home), &json).map_err(|error| format!("The move's state couldn't be saved: {error}"))
}

/// Why the last move failed: `<home>/move-failed.json`.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
struct Failed {
    at_ms: i64,
    why: String,
    target_machine: String,
}

/// Keeps why the move failed, so the app shows it after a restart too,
/// until it's dismissed.
fn remember_failed(home: &Home, why: &str, target_machine: &str) {
    let failed = Failed { at_ms: crate::transcript::now_ms(), why: why.to_owned(), target_machine: target_machine.to_owned() };
    let written = serde_json::to_vec_pretty(&failed)
        .map_err(|error| error.to_string())
        .and_then(|json| crate::remote::write_private(&failed_path(home), &json).map_err(|error| error.to_string()));
    if let Err(error) = written {
        tracing::warn!(%error, "could not keep why the move failed");
    }
}

/// Frozen mode, at start: a move frozen or committing. It comes before
/// everything else, `moved.json` included.
pub fn frozen(home: &Home) -> Option<Saved> {
    saved(home).filter(|saved| saved.stage != Stage::Copying)
}

/// For tests, which run Factory in their own process: a freeze stops the
/// move there instead of exiting, and StartMove needs no supervisor.
static HOLD_AT_FREEZE: AtomicBool = AtomicBool::new(false);

#[doc(hidden)]
pub fn hold_at_freeze() {
    HOLD_AT_FREEZE.store(true, Ordering::Relaxed);
}

enum Control {
    Confirm,
    Cancel,
}

/// The move as the app sees it, and how to steer the one under way.
#[derive(Default)]
pub(crate) struct Moving {
    view: Mutex<Option<v1::Move>>,
    control: Mutex<Option<mpsc::UnboundedSender<Control>>>,
    freeze: Arc<AtomicBool>,
}

fn locked<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

impl Moving {
    /// What a start finds: a move that failed before (shown until
    /// dismissed).
    pub(crate) fn new(home: &Home) -> Moving {
        let failed: Option<Failed> = std::fs::read_to_string(failed_path(home)).ok().and_then(|text| serde_json::from_str(&text).ok());
        let view = failed.map(|failed| v1::Move {
            phase: Phase::Failed.into(),
            target_machine: failed.target_machine,
            error: failed.why,
            can_cancel: true,
            updated_ms: failed.at_ms,
            ..v1::Move::default()
        });
        Moving { view: Mutex::new(view), ..Moving::default() }
    }

    /// `Workspace.move`.
    pub(crate) fn view(&self) -> Option<v1::Move> {
        locked(&self.view).clone()
    }

    fn phase(&self) -> Option<Phase> {
        locked(&self.view).as_ref().map(v1::Move::phase)
    }

    fn set(&self, change: impl FnOnce(&mut v1::Move)) {
        let mut view = locked(&self.view);
        let view = view.get_or_insert_with(v1::Move::default);
        change(view);
        view.can_cancel = can_cancel(view.phase());
        view.updated_ms = crate::transcript::now_ms();
    }

    fn clear(&self) {
        *locked(&self.view) = None;
        *locked(&self.control) = None;
    }
}

fn can_cancel(phase: Phase) -> bool {
    matches!(phase, Phase::Connecting | Phase::Confirm | Phase::Checking | Phase::Copying | Phase::Frozen | Phase::Failed)
}

fn refused(why: impl Into<String>) -> RequestError {
    RequestError::Account(v1::ErrorCode::InvalidRequest, why.into())
}

fn place(home: &Home) -> Place {
    Place {
        home: home.clone(),
        port: 0,
        user_home: std::env::home_dir().unwrap_or_default(),
        dev: home.root().file_name().is_some_and(|name| name == ".factory-dev"),
        // The supervisor stops and starts it: the move only ever exits.
        service: Service::None,
    }
}

/// StartMove.
pub(crate) fn start(factory: &Arc<Factory>, code: &str) -> Result<(), RequestError> {
    if factory.moving.phase().is_some_and(|phase| phase != Phase::Failed) {
        return Err(refused("A move is already under way."));
    }
    let Some(code) = ReceiveCode::parse(code) else {
        return Err(refused("That isn't a receive code. Run factoryd receive on the other computer, and copy the code it shows."));
    };
    if !code.version.is_empty() && crate::update::newer(crate::update::VERSION, &code.version) {
        return Err(refused(format!(
            "{} has Claktory {}, older than this one ({}). Update Claktory there, run factoryd receive again, and use its new code.",
            code.machine,
            code.version,
            crate::update::VERSION
        )));
    }
    if std::env::var_os(crate::update::SUPERVISED).is_none() && !HOLD_AT_FREEZE.load(Ordering::Relaxed) {
        return Err(refused(
            "This Claktory wasn't started by its service, so it can't stop and start itself for the move. Run factoryd move in Terminal instead.",
        ));
    }
    let _ = std::fs::remove_file(failed_path(&factory.home));
    factory.moving.clear();
    begin(factory, code, None);
    Ok(())
}

/// Starts the driver: a new move, or one a restart interrupted while copying.
fn begin(factory: &Arc<Factory>, code: ReceiveCode, resumed: Option<Saved>) {
    let (control, controls) = mpsc::unbounded_channel();
    *locked(&factory.moving.control) = Some(control);
    factory.moving.freeze.store(false, Ordering::Relaxed);
    factory.moving.set(|view| {
        *view = v1::Move {
            phase: Phase::Connecting.into(),
            target_machine: code.machine.clone(),
            target_os: code.os.clone(),
            target_arch: code.arch.clone(),
            target_version: code.version.clone(),
            step_text: format!("Connecting to {}…", code.machine),
            ..v1::Move::default()
        };
        if let Some(saved) = &resumed {
            shown(view, saved);
        }
    });
    factory.broadcast_workspace(&factory.lock());
    let weak = Arc::downgrade(factory);
    tokio::spawn(async move {
        drive(weak, code, resumed, controls).await;
    });
}

/// What a saved move showed.
fn shown(view: &mut v1::Move, saved: &Saved) {
    view.target_machine.clone_from(&saved.target_machine);
    view.target_os.clone_from(&saved.target_os);
    view.target_arch.clone_from(&saved.target_arch);
    view.target_version.clone_from(&saved.target_version);
    view.map = saved.map.iter().map(|(from, to)| v1::MovePath { from: from.clone(), to: to.clone() }).collect();
    view.missing_tools.clone_from(&saved.missing);
    view.plan_bytes = saved.plan_bytes;
    view.bytes_have = saved.bytes_have;
    view.bytes_total = saved.bytes_total;
    view.round = saved.round;
    view.target_confirmed = true;
}

/// ConfirmMove.
pub(crate) fn confirm(factory: &Arc<Factory>) -> Result<(), RequestError> {
    if factory.moving.phase() != Some(Phase::Confirm) {
        return Err(refused("There are no digits to confirm now."));
    }
    send(factory, Control::Confirm)
}

/// CancelMove.
pub(crate) fn cancel(factory: &Arc<Factory>) -> Result<(), RequestError> {
    match factory.moving.phase() {
        None => Err(refused("There's no move to call off.")),
        Some(Phase::Failed) => {
            let _ = std::fs::remove_file(failed_path(&factory.home));
            factory.moving.clear();
            factory.broadcast_workspace(&factory.lock());
            Ok(())
        }
        Some(phase) if can_cancel(phase) => send(factory, Control::Cancel),
        Some(_) => Err(refused("The move can't be called off any more: the other computer is taking over.")),
    }
}

/// FreezeMove.
pub(crate) fn freeze(factory: &Arc<Factory>) -> Result<(), RequestError> {
    if factory.moving.phase() != Some(Phase::Copying) {
        return Err(refused("The move isn't copying, so there's nothing to hurry."));
    }
    factory.moving.freeze.store(true, Ordering::Relaxed);
    factory.moving.set(|view| view.step_text = "Stopping agents for the last round after this one…".to_owned());
    factory.broadcast_workspace(&factory.lock());
    Ok(())
}

fn send(factory: &Arc<Factory>, control: Control) -> Result<(), RequestError> {
    let sent = locked(&factory.moving.control).as_ref().is_some_and(|sender| sender.send(control).is_ok());
    if sent { Ok(()) } else { Err(refused("The move has ended.")) }
}

/// At start: a move a restart interrupted while copying carries on.
pub fn resume(factory: &Arc<Factory>) {
    let Some(saved) = saved(&factory.home).filter(|saved| saved.stage == Stage::Copying) else { return };
    let Some(code) = ReceiveCode::parse(&saved.code) else {
        move_source::forget(&factory.home);
        return;
    };
    tracing::info!(target = %saved.target_machine, round = saved.round, "carrying on moving this Claktory");
    begin(factory, code, Some(saved));
}

/// The app's side of the person, in the normal backend.
struct App {
    factory: Weak<Factory>,
    freeze: Arc<AtomicBool>,
    saved: Saved,
    /// When the view last showed a round's progress.
    shown_at: Option<Instant>,
}

/// How often progress within a round reaches the app.
const PROGRESS_EVERY: Duration = Duration::from_secs(1);

/// Whether progress is due again, and if so, notes it's shown now.
fn progress_due(shown_at: &mut Option<Instant>, have: u64, total: u64) -> bool {
    if have < total && shown_at.is_some_and(|at| at.elapsed() < PROGRESS_EVERY) {
        return false;
    }
    *shown_at = Some(Instant::now());
    true
}

impl App {
    fn update(&self, change: impl FnOnce(&mut v1::Move)) {
        if let Some(factory) = self.factory.upgrade() {
            factory.moving.set(change);
            factory.broadcast_workspace(&factory.lock());
        }
    }

    fn home(&self) -> Option<Home> {
        self.factory.upgrade().map(|factory| factory.home.clone())
    }
}

impl Person for App {
    fn say(&mut self, _text: &str) {}

    fn checked(&mut self, checked: &Checked, plan_bytes: u64) {
        self.saved.map.clone_from(&checked.map);
        self.saved.missing.clone_from(&checked.missing);
        self.saved.plan_bytes = plan_bytes;
        let saved = self.saved.clone();
        self.update(|view| {
            shown(view, &saved);
            view.phase = Phase::Copying.into();
            view.step_text = copying_text(&saved.target_machine);
        });
    }

    fn round_started(&mut self, n: u32) {
        let machine = self.saved.target_machine.clone();
        self.update(|view| {
            view.round = n;
            if view.phase() == Phase::Copying {
                view.step_text = copying_text(&machine);
            }
        });
    }

    fn round_done(&mut self, n: u32, rounded: &Rounded) {
        self.saved.round = n + 1;
        self.saved.bytes_have = rounded.have;
        self.saved.bytes_total = rounded.total;
        if let Some(home) = self.home()
            && let Err(error) = save(&home, &self.saved)
        {
            tracing::warn!(%error, "could not save the move's progress");
        }
        let rounded = *rounded;
        self.update(|view| {
            view.bytes_have = rounded.have;
            view.bytes_total = rounded.total;
        });
    }

    fn freeze_now(&self) -> bool {
        self.freeze.load(Ordering::Relaxed)
    }

    fn sending(&mut self, have: u64, total: u64) {
        if progress_due(&mut self.shown_at, have, total) {
            self.update(|view| {
                view.bytes_have = have;
                view.bytes_total = total;
            });
        }
    }

    fn redialing(&mut self, machine: &str) {
        let text = redialing_text(machine);
        self.update(|view| view.step_text = text);
    }

    fn redialed(&mut self, machine: &str) {
        let text = copying_text(machine);
        let freezing = self.freeze_now();
        self.update(|view| {
            view.step_text = if freezing { "Stopping agents for the last round after this one…".to_owned() } else { text };
        });
    }
}

fn copying_text(machine: &str) -> String {
    format!("Copying to {machine} while your agents keep working…")
}

fn redialing_text(machine: &str) -> String {
    format!("Lost the connection to {machine}. Trying to reach it again…")
}

/// The move in the normal backend: pairing and pre-copy, then the freeze.
async fn drive(weak: Weak<Factory>, code: ReceiveCode, resumed: Option<Saved>, mut controls: mpsc::UnboundedReceiver<Control>) {
    let Some(factory) = weak.upgrade() else { return };
    let home = factory.home.clone();
    let freeze = factory.moving.freeze.clone();
    drop(factory);
    let fail = |why: String| {
        move_source::forget(&home);
        let Some(factory) = weak.upgrade() else { return };
        *locked(&factory.moving.control) = None;
        let mut machine = String::new();
        factory.moving.set(|view| {
            view.phase = Phase::Failed.into();
            view.error.clone_from(&why);
            machine.clone_from(&view.target_machine);
        });
        remember_failed(&home, &why, &machine);
        factory.broadcast_workspace(&factory.lock());
    };
    let called_off = || {
        move_source::forget(&home);
        if let Some(factory) = weak.upgrade() {
            factory.moving.clear();
            factory.broadcast_workspace(&factory.lock());
        }
    };
    let mover = match Mover::new(&home, code.clone()).await {
        Ok(mover) => mover,
        Err(why) => return fail(why),
    };
    let mut mover = mover;
    let machine = code.machine.clone();
    let connected = if resumed.is_some() {
        // A restart while copying: the target may still hold the connection
        // that dropped, and turns this one away until it lets go of it.
        let redialing = mover.redial(move_source::REDIAL);
        tokio::pin!(redialing);
        loop {
            tokio::select! {
                linked = &mut redialing => break linked,
                control = controls.recv() => match control {
                    Some(Control::Cancel) | None => return called_off(),
                    Some(Control::Confirm) => {}
                },
            }
        }
    } else {
        mover.connect().await
    };
    let mut link = match connected {
        Ok(link) => link,
        Err(Problem::Lost(detail)) => {
            return fail(format!("Can't reach {machine} ({detail}). Check that factoryd receive still runs there, and try again."));
        }
        Err(problem) => return fail(problem.text(&machine)),
    };
    let target = link.target.clone();
    let machine = target.machine.clone();
    let mut saved =
        resumed.unwrap_or_else(|| Saved { code: code.text(), export_id: move_export::new_export_id(), round: 1, ..Saved::default() });
    saved.target_machine.clone_from(&machine);
    saved.target_os.clone_from(&target.os);
    saved.target_arch.clone_from(&target.arch);
    saved.target_version.clone_from(&target.version);
    if !target.resumed {
        let app = App { factory: weak.clone(), freeze: freeze.clone(), saved: saved.clone(), shown_at: None };
        app.update(|view| {
            view.phase = Phase::Confirm.into();
            view.target_machine.clone_from(&machine);
            view.target_os.clone_from(&target.os);
            view.target_arch.clone_from(&target.arch);
            view.target_version.clone_from(&target.version);
            view.digits.clone_from(&target.digits);
            view.step_text = format!("Check that {machine} shows {}.", target.digits);
        });
        let (mut ours, mut theirs) = (false, false);
        while !(ours && theirs) {
            enum Heard {
                Person(Option<Control>),
                Target(Result<(), Problem>),
            }
            let heard = tokio::select! {
                control = controls.recv() => Heard::Person(control),
                confirmed = link.target_confirmed(), if !theirs => Heard::Target(confirmed),
            };
            match heard {
                Heard::Person(Some(Control::Confirm)) if !ours => {
                    if let Err(problem) = link.send_confirmed().await {
                        return fail(problem.text(&machine));
                    }
                    ours = true;
                    app.update(|view| view.step_text = format!("Now confirm the same digits on {machine}."));
                }
                Heard::Person(Some(Control::Confirm)) => {}
                Heard::Person(Some(Control::Cancel) | None) => {
                    let source = crate::remote::machine_name();
                    link.fail(&format!("The digits weren't confirmed on {source}, so nothing was moved.")).await;
                    return called_off();
                }
                Heard::Target(Ok(())) => {
                    theirs = true;
                    app.update(|view| view.target_confirmed = true);
                }
                Heard::Target(Err(problem)) => return fail(problem.text(&machine)),
            }
        }
    }
    if let Err(why) = save(&home, &saved) {
        link.cancel(&why).await;
        return fail(why);
    }
    let mut app = App { factory: weak.clone(), freeze: freeze.clone(), saved: saved.clone(), shown_at: None };
    app.update(|view| {
        view.phase = if view.phase() == Phase::Copying { Phase::Copying } else { Phase::Checking }.into();
        view.step_text = format!("{machine} is checking it can take this Claktory…");
    });
    let place = place(&home);
    let first = saved.round.max(1);
    let copied = {
        let options = Options::default();
        let copying = move_source::copy(&place, &options, &mut mover, &mut link, &saved.export_id, first, &mut app);
        tokio::pin!(copying);
        loop {
            tokio::select! {
                copied = &mut copying => break Some(copied),
                control = controls.recv() => match control {
                    Some(Control::Cancel) | None => break None,
                    Some(Control::Confirm) => {}
                },
            }
        }
    };
    let copied = match copied {
        Some(Ok(copied)) => copied,
        Some(Err(problem)) => {
            let why = problem.text(&machine);
            link.cancel(&why).await;
            return fail(format!("{why} Nothing was moved."));
        }
        None => {
            link.cancel("The move was called off.").await;
            return called_off();
        }
    };
    // The freeze: the next start is frozen mode.
    let mut saved = app.saved.clone();
    saved.stage = Stage::Frozen;
    saved.round = copied.round + 1;
    saved.epoch = copied.epoch;
    if let Err(why) = save(&home, &saved) {
        link.cancel(&why).await;
        return fail(why);
    }
    link.close().await;
    mover.close().await;
    app.update(|view| {
        view.phase = Phase::Frozen.into();
        view.round = saved.round;
        view.step_text = "Stopping agents for the last round…".to_owned();
    });
    if HOLD_AT_FREEZE.load(Ordering::Relaxed) {
        return;
    }
    let Some(factory) = weak.upgrade() else { return };
    tracing::info!(target = %machine, "freezing for the move's last round");
    // As an update does: what's being written is finished, the vault's key
    // goes to the supervisor, and it starts this backend again.
    tokio::time::sleep(Duration::from_millis(300)).await;
    factory.finish_writes(Duration::from_secs(5)).await;
    factory.vault.hand_over(false);
    std::process::exit(0);
}

// ---- frozen mode ----

/// Frozen mode: serves the move's progress on `listener` while the move's
/// last part runs, then returns, for the supervisor to start the backend
/// again (moved mode, or the Claktory as it was).
pub async fn serve_frozen(listener: TcpListener, home: &Home, saved: Saved, handed: Option<crate::vault::handoff::Handed>) {
    tracing::warn!(target = %saved.target_machine, stage = ?saved.stage, "this Claktory is moving; frozen mode only");
    let mut view = v1::Move::default();
    shown(&mut view, &saved);
    view.phase = if saved.stage == Stage::Committing { Phase::Committing } else { Phase::Frozen }.into();
    view.step_text = format!("Sending the last changes to {}…", saved.target_machine);
    view.can_cancel = can_cancel(view.phase());
    view.updated_ms = crate::transcript::now_ms();
    let (shown_tx, shown_rx) = watch::channel(view);
    let (cancel_tx, cancel_rx) = watch::channel(false);
    let welcome = Arrival::read(home);
    let server = tokio::spawn(serve_clients(listener, welcome, shown_rx, cancel_tx));
    let outcome = finish_frozen(home, saved.clone(), &shown_tx, cancel_rx).await;
    let unfreeze = |why: Option<String>| {
        if let Some(why) = why {
            remember_failed(home, &why, &saved.target_machine);
        }
        move_source::forget(home);
        // The Claktory starts again here: its vault's key goes back to the
        // supervisor for it.
        if let Some(handed) = &handed {
            crate::vault::handoff::keep(&handed.vault_id, &handed.vmk);
        }
    };
    match outcome {
        Frozen::Moved => {
            move_source::forget(home);
            shown_tx.send_modify(|view| {
                view.phase = Phase::Done.into();
                view.can_cancel = false;
                view.step_text = format!("Moved to {}.", saved.target_machine);
            });
        }
        Frozen::Failed(why) => {
            shown_tx.send_modify(|view| {
                view.phase = Phase::Failed.into();
                view.error.clone_from(&why);
            });
            unfreeze(Some(why));
        }
        Frozen::CalledOff => unfreeze(None),
    }
    // A moment for the app to see how it ended.
    tokio::time::sleep(Duration::from_secs(2)).await;
    server.abort();
}

enum Frozen {
    Moved,
    Failed(String),
    CalledOff,
}

/// Frozen mode's person: the app's view.
struct Watched<'a> {
    shown: &'a watch::Sender<v1::Move>,
    shown_at: Option<Instant>,
    /// Import's last step, shown again once the connection is back.
    step: String,
}

impl Person for Watched<'_> {
    fn say(&mut self, _text: &str) {}

    fn importing(&mut self, _step: &str, text: &str) {
        let text = text.to_owned();
        if !text.is_empty() {
            self.step.clone_from(&text);
        }
        self.shown.send_modify(|view| {
            view.phase = Phase::Importing.into();
            if !text.is_empty() {
                view.step_text = text;
            }
            view.updated_ms = crate::transcript::now_ms();
        });
    }

    fn round_done(&mut self, _n: u32, rounded: &Rounded) {
        let rounded = *rounded;
        self.shown.send_modify(|view| {
            view.bytes_have = rounded.have;
            view.bytes_total = rounded.total;
            view.updated_ms = crate::transcript::now_ms();
        });
    }

    fn sending(&mut self, have: u64, total: u64) {
        if progress_due(&mut self.shown_at, have, total) {
            self.shown.send_modify(|view| {
                view.bytes_have = have;
                view.bytes_total = total;
                view.updated_ms = crate::transcript::now_ms();
            });
        }
    }

    fn redialing(&mut self, machine: &str) {
        let text = redialing_text(machine);
        self.shown.send_modify(|view| {
            view.step_text = text;
            view.updated_ms = crate::transcript::now_ms();
        });
    }

    fn redialed(&mut self, machine: &str) {
        let text = if self.step.is_empty() { format!("Sending the last changes to {machine}…") } else { self.step.clone() };
        self.shown.send_modify(|view| {
            view.step_text = text;
            view.updated_ms = crate::transcript::now_ms();
        });
    }
}

/// The last round, import on the target, the fence and the commit.
async fn finish_frozen(home: &Home, mut saved: Saved, shown: &watch::Sender<v1::Move>, mut cancel: watch::Receiver<bool>) -> Frozen {
    let machine = saved.target_machine.clone();
    let Some(code) = ReceiveCode::parse(&saved.code) else { return Frozen::Failed("The move's state is damaged.".to_owned()) };
    let place = place(home);
    {
        let home = home.clone();
        let _ = tokio::task::spawn_blocking(move || crate::uninstall::stop_leftovers(&home)).await;
    }
    let mut mover = match Mover::new(home, code).await {
        Ok(mover) => mover,
        Err(why) => return Frozen::Failed(why),
    };
    let mut person = Watched { shown, shown_at: None, step: String::new() };
    let mut link: Link;
    if saved.stage == Stage::Frozen {
        link = match mover.redial(Duration::from_secs(120)).await {
            Ok(link) => link,
            Err(problem) => return Frozen::Failed(format!("{} Nothing was moved.", problem.text(&machine))),
        };
        let options = Options::default();
        let finishing = move_source::finish(&place, &options, &mut mover, &mut link, &saved.export_id, saved.round, &mut person);
        let finished = tokio::select! {
            finished = finishing => Some(finished),
            _ = cancel.wait_for(|cancelled| *cancelled) => None,
        };
        match finished {
            Some(Ok(())) => {}
            Some(Err(problem)) => {
                let why = problem.text(&machine);
                link.cancel(&why).await;
                return Frozen::Failed(format!("{why} Nothing was moved."));
            }
            None => {
                link.cancel("The move was called off.").await;
                return Frozen::CalledOff;
            }
        }
        // No way back from here: the fence, then the commit.
        shown.send_modify(|view| {
            view.phase = Phase::Committing.into();
            view.can_cancel = false;
            view.step_text = format!("Handing over to {machine}…");
        });
        let moved = Moved {
            export_id: saved.export_id.clone(),
            epoch: saved.epoch,
            machine: crate::remote::machine_name().to_owned(),
            to: machine.clone(),
            to_os: saved.target_os.clone(),
            ..Moved::default()
        };
        if let Err(refused) = move_export::tombstone(&place, moved) {
            link.cancel(&refused.reason).await;
            return Frozen::Failed(format!("{} Nothing was moved.", refused.reason));
        }
        saved.stage = Stage::Committing;
        saved.at_ms = crate::transcript::now_ms();
        if let Err(error) = save(home, &saved) {
            tracing::error!(%error, "could not note the move's commit; saying it anyway");
        }
    } else {
        shown.send_modify(|view| view.step_text = format!("Handing over to {machine}…"));
        link = loop {
            match mover.redial(Duration::from_secs(120)).await {
                Ok(link) => break link,
                Err(problem) => {
                    tracing::info!(problem = %problem.text(&machine), "the move's target can't be reached to commit; trying again");
                    tokio::time::sleep(Duration::from_secs(5)).await;
                }
            }
        };
    }
    let committed = move_source::commit(&mover, &mut link, saved.at_ms, &saved.export_id, &mut person).await;
    mover.close().await;
    match committed {
        Ok(()) => Frozen::Moved,
        Err(why) => {
            // It refused the commit, so it never started: this copy comes back.
            match move_export::restore(&place) {
                Ok(_) => Frozen::Failed(format!("{why} Nothing was moved.")),
                Err(refused) => Frozen::Failed(format!(
                    "{why} This copy couldn't come back by itself ({}): run factoryd export --restore.",
                    refused.reason
                )),
            }
        }
    }
}

/// What frozen mode greets this computer's app with.
#[derive(Clone)]
struct Arrival {
    name: String,
    color: String,
    /// The token of this machine's own pairing code: only its app gets in.
    token: String,
}

impl Arrival {
    fn read(home: &Home) -> Arrival {
        let config: serde_json::Value =
            std::fs::read_to_string(home.config_path()).ok().and_then(|text| serde_json::from_str(&text).ok()).unwrap_or_default();
        let name = config["name"].as_str().map(str::trim).filter(|name| !name.is_empty()).unwrap_or(crate::remote::machine_name());
        let color = config["color"].as_str().filter(|color| !color.is_empty()).unwrap_or(crate::remote::key_color(home));
        let token = std::fs::read_to_string(home.root().join("remote/local-pairing"))
            .ok()
            .and_then(|code| code.trim().split(['?', '&']).find_map(|part| part.strip_prefix("t=")).map(str::to_owned))
            .unwrap_or_default();
        Arrival { name: name.to_owned(), color: color.to_owned(), token }
    }
}

async fn serve_clients(listener: TcpListener, arrival: Arrival, shown: watch::Receiver<v1::Move>, cancel: watch::Sender<bool>) {
    let cancel = Arc::new(cancel);
    loop {
        let Ok((stream, peer)) = listener.accept().await else { continue };
        let (arrival, shown, cancel) = (arrival.clone(), shown.clone(), cancel.clone());
        tokio::spawn(async move {
            if let Err(error) = serve_client(stream, arrival, shown, &cancel).await {
                tracing::debug!(%peer, %error, "a client of frozen mode left");
            }
        });
    }
}

type Socket = tokio_tungstenite::WebSocketStream<tokio::net::TcpStream>;

async fn answer(socket: &mut Socket, reply_to: u64, body: v1::server_message::Body) -> Result<(), String> {
    let message = v1::ServerMessage { reply_to, body: Some(body) };
    socket.send(Message::Binary(message.encode_to_vec().into())).await.map_err(|error| error.to_string())
}

fn error(why: impl Into<String>) -> v1::server_message::Body {
    v1::server_message::Body::Error(v1::Error { code: v1::ErrorCode::InvalidRequest.into(), message: why.into() })
}

fn workspace(view: &v1::Move) -> v1::server_message::Body {
    v1::server_message::Body::Workspace(v1::Workspace {
        version: crate::update::VERSION.to_owned(),
        r#move: Some(view.clone()),
        ..v1::Workspace::default()
    })
}

/// One client: this computer's app, by its token; the move's view, and
/// CancelMove while there's still a way back.
async fn serve_client(
    stream: tokio::net::TcpStream,
    arrival: Arrival,
    mut shown: watch::Receiver<v1::Move>,
    cancel: &watch::Sender<bool>,
) -> Result<(), String> {
    use tokio_tungstenite::tungstenite::handshake::server::{ErrorResponse, Request, Response};
    // As the normal backend: never a web page.
    #[allow(clippy::result_large_err)]
    fn no_pages(request: &Request, response: Response) -> Result<Response, ErrorResponse> {
        if request.headers().contains_key("origin") {
            let mut refusal = ErrorResponse::new(Some("Claktory doesn't accept connections from web pages.".into()));
            *refusal.status_mut() = tokio_tungstenite::tungstenite::http::StatusCode::FORBIDDEN;
            return Err(refusal);
        }
        Ok(response)
    }
    let mut socket = tokio_tungstenite::accept_hdr_async(stream, no_pages).await.map_err(|error| error.to_string())?;
    let hello = loop {
        match tokio::time::timeout(Duration::from_secs(10), socket.next()).await {
            Ok(Some(Ok(Message::Binary(bytes)))) => break v1::ClientMessage::decode(bytes).map_err(|error| error.to_string())?,
            Ok(Some(Ok(Message::Close(_))) | None) | Err(_) => return Ok(()),
            Ok(Some(Ok(_))) => continue,
            Ok(Some(Err(error))) => return Err(error.to_string()),
        }
    };
    let Some(v1::client_message::Body::Hello(knock)) = hello.body else {
        return answer(&mut socket, hello.id, error("first message must be hello")).await;
    };
    if knock.protocol_version != factory_protocol::PROTOCOL_VERSION {
        let why = format!("server speaks protocol {}, client sent {}", factory_protocol::PROTOCOL_VERSION, knock.protocol_version);
        return answer(&mut socket, hello.id, error(why)).await;
    }
    if arrival.token.is_empty() || !crate::move_wire::same(&knock.token, &arrival.token) {
        tokio::time::sleep(Duration::from_secs(1)).await;
        let why = "This Claktory is moving to another computer; only this computer's own app can follow it.";
        let refusal = v1::server_message::Body::Error(v1::Error { code: v1::ErrorCode::Unauthorized.into(), message: why.into() });
        return answer(&mut socket, hello.id, refusal).await;
    }
    let welcome = v1::Welcome {
        protocol_version: factory_protocol::PROTOCOL_VERSION,
        server_version: env!("CARGO_PKG_VERSION").to_owned(),
        device_id: "local".to_owned(),
        name: arrival.name.clone(),
        color: arrival.color.clone(),
        confirm_by_ms: 0,
    };
    answer(&mut socket, hello.id, v1::server_message::Body::Welcome(welcome)).await?;
    let first = shown.borrow_and_update().clone();
    answer(&mut socket, 0, workspace(&first)).await?;
    loop {
        tokio::select! {
            changed = shown.changed() => {
                if changed.is_err() {
                    return Ok(());
                }
                let view = shown.borrow_and_update().clone();
                answer(&mut socket, 0, workspace(&view)).await?;
            }
            frame = socket.next() => {
                let message = match frame {
                    Some(Ok(Message::Binary(bytes))) => v1::ClientMessage::decode(bytes).map_err(|error| error.to_string())?,
                    Some(Ok(Message::Close(_))) | None => return Ok(()),
                    Some(Ok(_)) => continue,
                    Some(Err(error)) => return Err(error.to_string()),
                };
                let reply = match message.body {
                    Some(v1::client_message::Body::CancelMove(_)) => {
                        if shown.borrow().can_cancel {
                            cancel.send_replace(true);
                            v1::server_message::Body::Ack(v1::Ack {})
                        } else {
                            error("The move can't be called off any more: the other computer is taking over.")
                        }
                    }
                    _ => error(format!(
                        "This Claktory is moving to {}: it does nothing else until the move is done.",
                        shown.borrow().target_machine
                    )),
                };
                answer(&mut socket, message.id, reply).await?;
            }
        }
    }
}
