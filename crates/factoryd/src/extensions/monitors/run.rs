//! The scheduler: once a second it fires the monitors that are due, runs
//! checks, polls files and reads watches' output. Every fire goes through
//! `trigger`, which keeps the rules that stop monitors from looping: one fire
//! waiting per monitor (what happens meanwhile is carried into the next), a
//! least gap between a watch's fires, and an hourly budget per agent.

use std::collections::{BTreeMap, HashMap};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

use factory_agent::{Event, EventSource};
use serde::{Deserialize, Serialize};

use super::when::{self, FireOn, MINUTE, Repeat, SECOND, When};
use super::{Monitor, Query, Status, fired_since, query, save, waiting};
use crate::factory::Factory;
use crate::transcript::now_ms;

/// Fires an agent's monitors may make in `BUDGET_WINDOW` before the one
/// going over is paused.
const BUDGET: u32 = 30;
const BUDGET_WINDOW: i64 = 60 * MINUTE;
/// How much of what happened a fire carries.
const WHAT_CHARS: usize = 4_000;
/// A time monitor this late says when it was due.
const LATE: i64 = 90 * SECOND;

const CHECKS_AT_ONCE: usize = 3;
const CHECK_TIMEOUT: Duration = Duration::from_secs(60);
const CHECK_OUTPUT: u64 = 64 * 1024;
/// Checks in a row that can't run before the monitor stops with an error.
const CHECK_FAILURES: u32 = 5;

const FILES_EVERY: i64 = 2 * SECOND;
const FILES_QUIET: i64 = 2 * SECOND;
const FILES_ENTRIES: usize = 20_000;
const PATHS_SHOWN: usize = 20;

/// A watch's batch of lines closes after this long without new ones…
const BATCH_QUIET: i64 = SECOND;
/// …or at this many lines or bytes.
const BATCH_LINES: usize = 50;
const BATCH_BYTES: usize = 8 * 1024;
const LINE_CHARS: usize = 2_000;
const READ_CHUNK: u64 = 256 * 1024;
/// A watch's process that ends this soon after starting, this many times in
/// a row, stops the monitor with an error.
const QUICK_RUN: i64 = MINUTE;
const QUICK_RUNS: u32 = 5;
const MAX_BACKOFF: i64 = 5 * MINUTE;
const MAX_LOG: u64 = 50 * 1024 * 1024;

/// The scheduler's own state, kept with the Factory.
pub(crate) struct State {
    /// Added to the clock, for tests.
    offset: AtomicI64,
    tick_ms: AtomicU64,
    /// When this Factory started, so a late fire can say Factory wasn't running.
    started: i64,
    live: Mutex<HashMap<(String, u32), Live>>,
    /// Held while a watch's process is started and recorded, or stopped
    /// and its monitor changed, so neither happens halfway through the other.
    processes: Mutex<()>,
}

impl Default for State {
    fn default() -> State {
        State {
            offset: AtomicI64::new(0),
            tick_ms: AtomicU64::new(1000),
            started: now_ms(),
            live: Mutex::default(),
            processes: Mutex::default(),
        }
    }
}

impl State {
    /// The monitors' clock: now, in ms since 1970.
    pub(crate) fn now(&self) -> i64 {
        now_ms() + self.offset.load(Ordering::Relaxed)
    }

    fn tick(&self) -> Duration {
        Duration::from_millis(self.tick_ms.load(Ordering::Relaxed))
    }

    fn live(&self) -> std::sync::MutexGuard<'_, HashMap<(String, u32), Live>> {
        self.live.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    pub(crate) fn processes(&self) -> std::sync::MutexGuard<'_, ()> {
        self.processes.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

impl Factory {
    /// Moves the monitors' clock forward, as if that much time had passed; for tests.
    pub fn advance_monitor_clock(&self, by: Duration) {
        self.monitors.offset.fetch_add(i64::try_from(by.as_millis()).unwrap_or(i64::MAX), Ordering::Relaxed);
    }

    /// How often the monitors' scheduler runs (every second); tests make it shorter.
    pub fn set_monitor_tick(&self, tick: Duration) {
        self.monitors.tick_ms.store(u64::try_from(tick.as_millis()).unwrap_or(1000).max(1), Ordering::Relaxed);
    }
}

/// What the scheduler keeps in memory about a monitor between runs.
#[derive(Default)]
pub(super) struct Live {
    checking: bool,
    last_poll: i64,
    /// The files as the last poll saw them.
    seen: Option<Snapshot>,
    /// When they last changed, while they're still changing.
    changed_at: Option<i64>,
    /// How far the watch's log has been read (into `batch`).
    read_to: Option<u64>,
    batch: Vec<String>,
    batch_bytes: usize,
    last_line: i64,
}

/// A file's size and modification time, by path.
type Snapshot = BTreeMap<String, (u64, i64)>;

/// What the scheduler needs to know about an agent.
struct Agent {
    /// It's in the Factory with Monitors on.
    on: bool,
    running: bool,
    folder: PathBuf,
}

/// Runs the monitors for as long as Factory runs.
pub(crate) async fn monitor(factory: Weak<Factory>) {
    loop {
        let Some(tick) = factory.upgrade().map(|factory| factory.monitors.tick()) else { return };
        tokio::time::sleep(tick).await;
        let Some(factory) = factory.upgrade() else { return };
        if let Err(error) = tick_once(&factory).await {
            tracing::error!(%error, "could not run monitors");
        }
        super::refresh_view(&factory);
    }
}

async fn tick_once(factory: &Arc<Factory>) -> rusqlite::Result<()> {
    let now = factory.monitors.now();
    let monitors = query(&factory.store, &Query { active: true, ..Query::default() })?;
    factory.monitors.live().retain(|key, _| monitors.iter().any(|monitor| (&monitor.agent, monitor.id) == (&key.0, key.1)));
    let agents: HashMap<String, Agent> = {
        let inner = factory.lock();
        let mut agents = HashMap::new();
        for monitor in &monitors {
            if agents.contains_key(&monitor.agent) {
                continue;
            }
            let agent = match inner.config.agent(&monitor.agent) {
                Some(config) => Agent {
                    on: crate::extensions::enabled(config, super::ID),
                    running: inner.agents.get(&config.id).is_some_and(|runtime| runtime.running),
                    folder: crate::runner::places(&factory.home, &inner.config, config).folder,
                },
                None => Agent { on: false, running: false, folder: PathBuf::new() },
            };
            agents.insert(monitor.agent.clone(), agent);
        }
        agents
    };
    for mut monitor in monitors {
        let Some(agent) = agents.get(&monitor.agent) else { continue };
        if !agent.on {
            // Kept, but quiet: its process stops until it's back.
            if monitor.cursor.pid.is_some() {
                stop_process(factory, &monitor);
                monitor.cursor.pid = None;
                save(&factory.store, &monitor, now, "", None, true)?;
            }
            continue;
        }
        if monitor.until.is_some_and(|until| now >= until) {
            end(factory, &mut monitor, Status::Done, "its until passed", now)?;
            continue;
        }
        if !monitor.carry.is_empty() && gap_passed(&monitor, now) && !waiting(&factory.store, &monitor.agent, monitor.id)? {
            flush(factory, &mut monitor, now)?;
            if monitor.status != Status::Active {
                continue;
            }
        }
        match monitor.when.clone() {
            When::At { .. } | When::Every { .. } | When::Cron { .. } => time(factory, &mut monitor, agent, now)?,
            When::Check { .. } => check(factory, &monitor, agent, now),
            When::Files { paths, glob } => files(factory, &mut monitor, &paths, glob.as_deref(), now).await?,
            When::Watch { command, pattern, .. } => watch(factory, &mut monitor, agent, &command, pattern.as_deref(), now)?,
        }
    }
    Ok(())
}

/// What made a monitor fire.
struct Trigger {
    /// "check matched \"success\"", "3 new lines".
    cause: String,
    /// What happened: output, paths, lines. Cut to `WHAT_CHARS`.
    what: String,
    /// "Was due 09:00; Factory wasn't running then."
    late: Option<String>,
    /// This fire is its last (an `at`, or a check that fires once).
    last: bool,
}

/// Whether a watch's least gap since its last fire has passed (always, for other kinds).
fn gap_passed(monitor: &Monitor, now: i64) -> bool {
    match (&monitor.when, monitor.last_fired) {
        (When::Watch { gap, .. }, Some(fired)) => now - fired >= *gap,
        _ => true,
    }
}

/// Fires the monitor, or carries what happened into its next fire while its
/// last one waits to be read; pauses it instead when its agent's monitors
/// are over the budget.
fn trigger(factory: &Arc<Factory>, monitor: &mut Monitor, trigger: Trigger, now: i64) -> rusqlite::Result<()> {
    if !gap_passed(monitor, now) || waiting(&factory.store, &monitor.agent, monitor.id)? {
        let mut carry = std::mem::take(&mut monitor.carry);
        if !carry.is_empty() {
            carry.push('\n');
        }
        let _ = write!(carry, "{}", if trigger.what.is_empty() { trigger.cause } else { format!("{}:\n{}", trigger.cause, trigger.what) });
        monitor.carry = tail(&carry, WHAT_CHARS);
        monitor.cursor.last_after_carry |= trigger.last;
        return save(&factory.store, monitor, now, "", None, true).map(drop);
    }
    if fired_since(&factory.store, &monitor.agent, now - BUDGET_WINDOW)? >= BUDGET {
        let reason = format!("your monitors fired {BUDGET} times in the last hour");
        stop_process(factory, monitor);
        monitor.cursor.pid = None;
        monitor.status = Status::Paused;
        monitor.reason.clone_from(&reason);
        monitor.carry.clear();
        save(&factory.store, monitor, now, "", Some(("paused", &reason)), true)?;
        notice(
            factory,
            monitor,
            "paused",
            format!(
                "Monitor m{} \"{}\" paused: {reason}. Resume it with monitors_change when it's fixed.",
                monitor.id, monitor.name
            ),
        );
        return Ok(());
    }
    monitor.fires += 1;
    let last = trigger.last || monitor.max_fires.is_some_and(|max| monitor.fires >= max);
    let text = fire_text(factory, monitor, &trigger, last);
    let summary = format!("m{} {} fired", monitor.id, monitor.name);
    let event = Event { source: EventSource::Monitor, from: monitor.name.clone(), id: format!("m{}", monitor.id), summary, transient: false };
    factory.deliver_event(&monitor.agent, text, event);
    monitor.last_fired = Some(now);
    monitor.carry.clear();
    monitor.cursor.last_after_carry = false;
    save(&factory.store, monitor, now, "", Some(("fired", &trigger.cause)), true)?;
    if last {
        let reason = if monitor.max_fires.is_some_and(|max| monitor.fires >= max) { "it fired max_fires times" } else { "it fired" };
        end(factory, monitor, Status::Done, reason, now)?;
    }
    Ok(())
}

/// What happened while the last fire waited to be read, as one fire.
fn flush(factory: &Arc<Factory>, monitor: &mut Monitor, now: i64) -> rusqlite::Result<()> {
    let what = std::mem::take(&mut monitor.carry);
    let last = monitor.cursor.last_after_carry;
    trigger(factory, monitor, Trigger { cause: "while your last fire waited".to_owned(), what, late: None, last }, now)
}

/// The event's text: what fired, why, what happened, and the agent's prompt.
fn fire_text(factory: &Factory, monitor: &Monitor, trigger: &Trigger, last: bool) -> String {
    let done = if last { ", now done" } else { "" };
    let mut text = format!("Monitor m{} \"{}\" fired ({}; fire {}{done}).", monitor.id, monitor.name, trigger.cause, monitor.fires);
    if monitor.created_by != monitor.agent {
        let by = if monitor.created_by.is_empty() {
            "the person".to_owned()
        } else {
            factory.lock().config.agent(&monitor.created_by).map_or_else(|| monitor.created_by.clone(), |agent| agent.name.clone())
        };
        let _ = write!(text, " Set by {by}.");
    }
    if let Some(late) = &trigger.late {
        let _ = write!(text, " {late}");
    }
    if !trigger.what.is_empty() {
        let _ = write!(text, "\n{}", trigger.what);
    }
    let _ = write!(text, "\n\nYour prompt: {}", monitor.prompt);
    text
}

/// One Factory note to the agent about its monitor, such as why it stopped.
fn notice(factory: &Arc<Factory>, monitor: &Monitor, what: &str, text: String) {
    let summary = format!("m{} {} {what}", monitor.id, monitor.name);
    let event = Event { source: EventSource::Host, from: "Factory".to_owned(), id: String::new(), summary, transient: false };
    factory.deliver_event(&monitor.agent, text, event);
}

/// Ends the monitor (done, or in error with a notice saying why).
fn end(factory: &Arc<Factory>, monitor: &mut Monitor, status: Status, reason: &str, now: i64) -> rusqlite::Result<()> {
    stop_process(factory, monitor);
    monitor.cursor.pid = None;
    monitor.status = status;
    reason.clone_into(&mut monitor.reason);
    monitor.next_due = None;
    save(&factory.store, monitor, now, "", Some((status.name(), reason)), true)?;
    forget_live(factory, monitor);
    if status == Status::Error {
        notice(
            factory,
            monitor,
            "stopped",
            format!(
                "Monitor m{} \"{}\" stopped: {reason}. Fix it, then resume it with monitors_change, or cancel it.",
                monitor.id, monitor.name
            ),
        );
    }
    Ok(())
}

/// The last `chars` characters of `text`, saying when some were cut.
fn tail(text: &str, chars: usize) -> String {
    let count = text.chars().count();
    if count <= chars {
        return text.to_owned();
    }
    let kept: String = text.chars().skip(count - chars).collect();
    format!("[earlier output cut]\n{kept}")
}

fn time(factory: &Arc<Factory>, monitor: &mut Monitor, agent: &Agent, now: i64) -> rusqlite::Result<()> {
    let Some(due) = monitor.next_due else {
        // Nothing due, which shouldn't happen: schedule it afresh.
        monitor.next_due = monitor.when.next(now);
        return save(&factory.store, monitor, now, "", None, true).map(drop);
    };
    if due > now || (monitor.idle_only && agent.running) {
        return Ok(());
    }
    let late = (now - due > LATE).then(|| {
        if due < factory.monitors.started {
            format!("Was due {}; Factory wasn't running then.", when::local(due, now))
        } else {
            format!("Was due {}.", when::local(due, now))
        }
    });
    let cause = match &monitor.when {
        When::At { at } => format!("at {}", when::local(*at, now)),
        when => when.describe(now),
    };
    // Counted from this fire, so missed times fire once, not once each.
    monitor.next_due = monitor.when.next(now);
    let last = matches!(monitor.when, When::At { .. });
    trigger(factory, monitor, Trigger { cause, what: String::new(), late, last }, now)
}

/// The result of running a check's command.
enum CheckRun {
    Ran { code: Option<i32>, output: String },
    /// It couldn't run, or timed out.
    Failed(String),
}

fn check(factory: &Arc<Factory>, monitor: &Monitor, agent: &Agent, now: i64) {
    let When::Check { command, .. } = &monitor.when else { return };
    if monitor.next_due.is_some_and(|due| due > now) || (monitor.idle_only && agent.running) {
        return;
    }
    {
        let mut live = factory.monitors.live();
        if live.values().filter(|live| live.checking).count() >= CHECKS_AT_ONCE {
            return;
        }
        let entry = live.entry((monitor.agent.clone(), monitor.id)).or_default();
        if entry.checking {
            return;
        }
        entry.checking = true;
    }
    let factory = factory.clone();
    let (agent_id, id, command, folder) = (monitor.agent.clone(), monitor.id, command.clone(), agent.folder.clone());
    let since = monitor.last_fired.unwrap_or(monitor.created) / 1000;
    tokio::spawn(async move {
        let run = run_check(&folder, &command, since).await;
        if let Some(live) = factory.monitors.live().get_mut(&(agent_id.clone(), id)) {
            live.checking = false;
        }
        if let Err(error) = checked(&factory, &agent_id, id, run) {
            tracing::error!(%error, agent = %agent_id, monitor = id, "could not record a check");
        }
    });
}

/// Runs a check's command in the agent's folder, as bash runs commands,
/// with FACTORY_MONITOR_LAST_FIRED set to when the monitor last fired (or
/// was set), in seconds since 1970.
async fn run_check(folder: &Path, command: &str, last_fired: i64) -> CheckRun {
    use tokio::io::AsyncReadExt;
    let spawned = tokio::process::Command::new("bash")
        .args(["--noprofile", "--norc", "-o", "pipefail", "-c", &format!("exec 2>&1\n{command}")])
        .env("FACTORY_MONITOR_LAST_FIRED", last_fired.to_string())
        .current_dir(folder)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .process_group(0)
        .kill_on_drop(true)
        .spawn();
    let mut child = match spawned {
        Ok(child) => child,
        Err(error) => return CheckRun::Failed(format!("it couldn't start: {error}")),
    };
    let pid = child.id();
    let Some(stdout) = child.stdout.take() else { return CheckRun::Failed("it has no output".to_owned()) };
    let ran = tokio::time::timeout(CHECK_TIMEOUT, async move {
        let mut output = Vec::new();
        let read = stdout.take(CHECK_OUTPUT).read_to_end(&mut output).await;
        let status = child.wait().await;
        (read, status, output)
    })
    .await;
    match ran {
        Ok((_, Ok(status), output)) => CheckRun::Ran { code: status.code(), output: String::from_utf8_lossy(&output).into_owned() },
        Ok((_, Err(error), _)) => CheckRun::Failed(format!("it failed: {error}")),
        Err(_) => {
            if let Some(pid) = pid {
                factory_agent::tools::bash::stop_group(pid);
            }
            CheckRun::Failed("it timed out after 60s".to_owned())
        }
    }
}

/// A check ran: fires when its condition became true (or, with repeat
/// "always", holds), and schedules the next run.
fn checked(factory: &Arc<Factory>, agent: &str, id: u32, run: CheckRun) -> rusqlite::Result<()> {
    let found = query(&factory.store, &Query { agent: Some(agent), id: Some(id), active: true, ..Query::default() })?;
    let Some(mut monitor) = found.into_iter().next() else { return Ok(()) };
    let When::Check { every, fire_on, pattern, repeat, .. } = monitor.when.clone() else { return Ok(()) };
    let now = factory.monitors.now();
    monitor.next_due = Some(now + every);
    let (code, output) = match run {
        CheckRun::Failed(reason) => {
            monitor.cursor.failures += 1;
            if monitor.cursor.failures >= CHECK_FAILURES {
                let reason = format!("its check couldn't run {CHECK_FAILURES} times in a row (last: {reason})");
                return end(factory, &mut monitor, Status::Error, &reason, now);
            }
            return save(&factory.store, &monitor, now, "", None, true).map(drop);
        }
        CheckRun::Ran { code, output } => (code, output),
    };
    monitor.cursor.failures = 0;
    let exit = code.map_or_else(|| "killed".to_owned(), |code| format!("exit {code}"));
    let (holds, cause) = match fire_on {
        FireOn::Success => (code == Some(0), "check succeeded".to_owned()),
        FireOn::Failure => (code != Some(0), format!("check failed ({exit})")),
        FireOn::Match => {
            let pattern = pattern.unwrap_or_default();
            let matched = regex::Regex::new(&pattern).is_ok_and(|regex| regex.is_match(&output));
            (matched, format!("check matched \"{pattern}\""))
        }
        FireOn::Changed => {
            let hash = hash(&output);
            let changed = monitor.cursor.hash.as_ref().is_some_and(|before| *before != hash);
            monitor.cursor.hash = Some(hash);
            (changed, "check's output changed".to_owned())
        }
    };
    let before = monitor.cursor.last_true.unwrap_or(false);
    monitor.cursor.last_true = Some(holds);
    let fires = holds && (!before || fire_on == FireOn::Changed || repeat == Repeat::Always);
    if !fires {
        return save(&factory.store, &monitor, now, "", None, true).map(drop);
    }
    let what = if output.trim().is_empty() { String::new() } else { format!("Output (end):\n{}", tail(output.trim_end(), WHAT_CHARS)) };
    let last = repeat == Repeat::Once;
    trigger(factory, &mut monitor, Trigger { cause, what, late: None, last }, now)
}

fn hash(text: &str) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(text.as_bytes()).iter().take(16).map(|byte| format!("{byte:02x}")).collect()
}

/// Where a files monitor keeps the files as it last fired on them.
fn baseline_path(factory: &Factory, monitor: &Monitor) -> PathBuf {
    factory.home.agent_dir(&monitor.agent).join("monitors").join(format!("m{}.files.json", monitor.id))
}

#[derive(Serialize, Deserialize)]
struct Baseline {
    files: Snapshot,
}

fn read_baseline(path: &Path) -> Option<Snapshot> {
    let bytes = std::fs::read(path).ok()?;
    serde_json::from_slice::<Baseline>(&bytes).ok().map(|baseline| baseline.files)
}

fn write_baseline(path: &Path, files: &Snapshot) {
    let write = || -> std::io::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let bytes = serde_json::to_vec(&Baseline { files: files.clone() }).map_err(std::io::Error::other)?;
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, bytes)?;
        std::fs::rename(tmp, path)
    };
    if let Err(error) = write() {
        tracing::warn!(%error, path = %path.display(), "could not save a files monitor's baseline");
    }
}

/// The files under `paths` (folders recursively, leaving out what
/// .gitignore and .factoryignore list), at most `FILES_ENTRIES`.
fn snapshot(paths: &[String], glob: Option<&str>) -> Snapshot {
    let matcher = glob.and_then(|glob| globset::Glob::new(glob).ok()).map(|glob| glob.compile_matcher());
    let mut files = Snapshot::new();
    let stamp = |metadata: &std::fs::Metadata| {
        let modified = metadata.modified().ok().and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok());
        (metadata.len(), modified.map_or(0, |time| i64::try_from(time.as_millis()).unwrap_or(0)))
    };
    for root in paths {
        let root_path = Path::new(root);
        let Ok(metadata) = std::fs::metadata(root_path) else { continue };
        if !metadata.is_dir() {
            files.insert(root.clone(), stamp(&metadata));
            continue;
        }
        let walk = ignore::WalkBuilder::new(root_path).add_custom_ignore_filename(".factoryignore").require_git(false).build();
        for entry in walk.flatten() {
            if files.len() >= FILES_ENTRIES {
                return files;
            }
            if !entry.file_type().is_some_and(|kind| kind.is_file()) {
                continue;
            }
            let path = entry.path();
            let relative = path.strip_prefix(root_path).unwrap_or(path);
            if matcher.as_ref().is_some_and(|matcher| !matcher.is_match(relative) && !path.file_name().is_some_and(|name| matcher.is_match(name))) {
                continue;
            }
            if let Ok(metadata) = entry.metadata() {
                files.insert(path.to_string_lossy().into_owned(), stamp(&metadata));
            }
        }
    }
    files
}

/// "Changed: a, b\nCreated: c\nRemoved: d", at most `PATHS_SHOWN` paths.
fn differences(before: &Snapshot, after: &Snapshot) -> String {
    let changed: Vec<&String> = after.iter().filter(|(path, stamp)| before.get(*path).is_some_and(|old| old != *stamp)).map(|(path, _)| path).collect();
    let created: Vec<&String> = after.keys().filter(|path| !before.contains_key(*path)).collect();
    let removed: Vec<&String> = before.keys().filter(|path| !after.contains_key(*path)).collect();
    let mut shown = 0;
    let mut text = Vec::new();
    for (label, paths) in [("Changed", changed), ("Created", created), ("Removed", removed)] {
        if paths.is_empty() {
            continue;
        }
        let room = PATHS_SHOWN.saturating_sub(shown);
        let mut line = format!("{label}: {}", paths.iter().take(room).map(|path| path.as_str()).collect::<Vec<_>>().join(", "));
        shown += paths.len().min(room);
        if paths.len() > room {
            let _ = write!(line, " +{} more", paths.len() - room);
        }
        text.push(line);
    }
    text.join("\n")
}

async fn files(factory: &Arc<Factory>, monitor: &mut Monitor, paths: &[String], glob: Option<&str>, now: i64) -> rusqlite::Result<()> {
    let key = (monitor.agent.clone(), monitor.id);
    {
        let mut live = factory.monitors.live();
        let entry = live.entry(key.clone()).or_default();
        if now - entry.last_poll < FILES_EVERY {
            return Ok(());
        }
        entry.last_poll = now;
    }
    let (paths, glob) = (paths.to_vec(), glob.map(str::to_owned));
    let Ok(current) = tokio::task::spawn_blocking(move || snapshot(&paths, glob.as_deref())).await else { return Ok(()) };
    let path = baseline_path(factory, monitor);
    let Some(baseline) = read_baseline(&path) else {
        write_baseline(&path, &current);
        return Ok(());
    };
    let quiet = {
        let mut live = factory.monitors.live();
        let entry = live.entry(key).or_default();
        if current == baseline {
            entry.changed_at = None;
            entry.seen = Some(current);
            return Ok(());
        }
        if entry.seen.as_ref() != Some(&current) {
            entry.changed_at = Some(now);
            entry.seen = Some(current.clone());
        }
        let quiet = entry.changed_at.is_some_and(|at| now - at >= FILES_QUIET);
        if quiet {
            entry.changed_at = None;
        }
        quiet
    };
    if !quiet {
        return Ok(());
    }
    write_baseline(&path, &current);
    let what = differences(&baseline, &current);
    trigger(factory, monitor, Trigger { cause: "files changed".to_owned(), what, late: None, last: false }, now)
}

/// Where a watch's process writes its output.
fn watch_log(factory: &Factory, monitor: &Monitor) -> PathBuf {
    factory.home.agent_dir(&monitor.agent).join("monitors").join(format!("m{}.log", monitor.id))
}

/// Whether the watch's process has ended.
fn process_ended(pid: u32, log: &Path) -> bool {
    log.with_extension("status").exists() || !factory_agent::tools::bash::group_alive(pid)
}

fn watch(factory: &Arc<Factory>, monitor: &mut Monitor, agent: &Agent, command: &str, pattern: Option<&str>, now: i64) -> rusqlite::Result<()> {
    let log = watch_log(factory, monitor);
    match monitor.cursor.pid {
        Some(pid) if process_ended(pid, &log) => {
            let ran = now - monitor.cursor.started.unwrap_or(now);
            monitor.cursor.quick_runs = if ran < QUICK_RUN { monitor.cursor.quick_runs + 1 } else { 0 };
            monitor.cursor.pid = None;
            if monitor.cursor.quick_runs >= QUICK_RUNS {
                let reason = format!("its command ended {QUICK_RUNS} times within a minute of starting (see {})", log.display());
                return end(factory, monitor, Status::Error, &reason, now);
            }
            let backoff = (SECOND << monitor.cursor.quick_runs.min(16)).min(MAX_BACKOFF);
            monitor.cursor.restart_at = Some(now + backoff);
            save(&factory.store, monitor, now, "", None, true)?;
        }
        Some(_) => {}
        None if monitor.cursor.restart_at.is_none_or(|at| at <= now) => {
            if let Some(dir) = log.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            let processes = factory.monitors.processes();
            match factory_agent::tools::bash::spawn_detached(&agent.folder, command, &log) {
                Ok((mut child, pid)) => {
                    // Reaped here; whether it runs is read from its group and status file.
                    tokio::spawn(async move {
                        let _ = child.wait().await;
                    });
                    monitor.cursor.pid = Some(pid);
                    monitor.cursor.started = Some(now);
                    monitor.cursor.restart_at = None;
                    if !save(&factory.store, monitor, now, "", None, true)? {
                        // Cancelled, paused or deleted meanwhile.
                        factory_agent::tools::bash::stop_group(pid);
                        return Ok(());
                    }
                    drop(processes);
                }
                Err(error) => return end(factory, monitor, Status::Error, &format!("its command couldn't start: {error}"), now),
            }
        }
        None => {}
    }
    let size = std::fs::metadata(&log).map_or(0, |metadata| metadata.len());
    if size > MAX_LOG {
        let reason = format!("its command wrote over 50 MB; filter its output, e.g. with `grep --line-buffered` (log: {})", log.display());
        return end(factory, monitor, Status::Error, &reason, now);
    }
    let key = (monitor.agent.clone(), monitor.id);
    let closed = {
        let mut live = factory.monitors.live();
        let entry = live.entry(key).or_default();
        let mut from = entry.read_to.unwrap_or(monitor.cursor.offset);
        if size < from {
            // The log was cut short; start over.
            from = 0;
            monitor.cursor.offset = 0;
        }
        let (lines, read_to) = read_lines(&log, from, size);
        entry.read_to = Some(read_to);
        let matcher = pattern.and_then(|pattern| regex::Regex::new(pattern).ok());
        for line in lines {
            if matcher.as_ref().is_some_and(|matcher| !matcher.is_match(&line)) {
                continue;
            }
            let line: String = line.chars().take(LINE_CHARS).collect();
            entry.batch_bytes += line.len() + 1;
            entry.batch.push(line);
            entry.last_line = now;
        }
        let full = entry.batch.len() >= BATCH_LINES || entry.batch_bytes >= BATCH_BYTES;
        if !entry.batch.is_empty() && (full || now - entry.last_line >= BATCH_QUIET) {
            entry.batch_bytes = 0;
            Some((std::mem::take(&mut entry.batch), read_to))
        } else if entry.batch.is_empty() && read_to != monitor.cursor.offset {
            // Nothing to fire on: remember how far it was read.
            Some((Vec::new(), read_to))
        } else {
            None
        }
    };
    let Some((lines, read_to)) = closed else { return Ok(()) };
    monitor.cursor.offset = read_to;
    if lines.is_empty() {
        return save(&factory.store, monitor, now, "", None, true).map(drop);
    }
    let count = lines.len();
    let cause = match pattern {
        Some(pattern) => format!("{count} new line{} matching \"{pattern}\"", if count == 1 { "" } else { "s" }),
        None => format!("{count} new line{}", if count == 1 { "" } else { "s" }),
    };
    let what = format!("{}\nFull log: {}", tail(&lines.join("\n"), WHAT_CHARS), log.display());
    trigger(factory, monitor, Trigger { cause, what, late: None, last: false }, now)
}

/// The whole lines of `log` from byte `from`, at most `READ_CHUNK` bytes,
/// and where they end.
fn read_lines(log: &Path, from: u64, size: u64) -> (Vec<String>, u64) {
    use std::io::{Read, Seek, SeekFrom};
    if size <= from {
        return (Vec::new(), from);
    }
    let read = || -> std::io::Result<Vec<u8>> {
        let mut file = std::fs::File::open(log)?;
        file.seek(SeekFrom::Start(from))?;
        let mut bytes = Vec::new();
        file.take(READ_CHUNK.min(size - from)).read_to_end(&mut bytes)?;
        Ok(bytes)
    };
    let Ok(bytes) = read() else { return (Vec::new(), from) };
    // Up to the last newline; a whole chunk without one counts as a line.
    let end = match bytes.iter().rposition(|byte| *byte == b'\n') {
        Some(newline) => newline + 1,
        None if bytes.len() as u64 >= READ_CHUNK => bytes.len(),
        None => return (Vec::new(), from),
    };
    let text = String::from_utf8_lossy(&bytes[..end]);
    let lines = text.lines().filter(|line| !line.trim().is_empty()).map(str::to_owned).collect();
    (lines, from + end as u64)
}

/// A new monitor: a files monitor notes the files as they are now, so what
/// changes from here on fires it.
pub(super) fn added(factory: &Factory, agent: &str, id: u32) {
    let Ok(found) = query(&factory.store, &Query { agent: Some(agent), id: Some(id), active: true, ..Query::default() }) else { return };
    let Some(monitor) = found.into_iter().next() else { return };
    if let When::Files { paths, glob } = &monitor.when {
        write_baseline(&baseline_path(factory, &monitor), &snapshot(paths, glob.as_deref()));
    }
}

/// Fires the monitor now, as asked (`monitors_change` fire), without ending it.
pub(super) fn fire_by_hand(factory: &Arc<Factory>, monitor: &mut Monitor, actor: &str, now: i64) -> rusqlite::Result<()> {
    let by = if actor.is_empty() { "the person".to_owned() } else { actor.to_owned() };
    trigger(factory, monitor, Trigger { cause: format!("fired by {by}"), what: String::new(), late: None, last: false }, now)
}

/// Stops a watch's process, if it has one running.
pub(crate) fn stop_process(_factory: &Factory, monitor: &Monitor) {
    if let (When::Watch { .. }, Some(pid)) = (&monitor.when, monitor.cursor.pid) {
        factory_agent::tools::bash::stop_group(pid);
    }
}

/// Forgets what the scheduler kept in memory about the monitor.
pub(super) fn forget_live(factory: &Factory, monitor: &Monitor) {
    factory.monitors.live().remove(&(monitor.agent.clone(), monitor.id));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_changes_name_paths_up_to_a_limit() {
        let before: Snapshot = [("/a".to_owned(), (1, 1)), ("/b".to_owned(), (1, 1))].into();
        let after: Snapshot = [("/a".to_owned(), (2, 2)), ("/c".to_owned(), (1, 1))].into();
        assert_eq!(differences(&before, &after), "Changed: /a\nCreated: /c\nRemoved: /b");
        let many: Snapshot = (0..25).map(|n| (format!("/f{n:02}"), (1, 1))).collect();
        let text = differences(&Snapshot::new(), &many);
        assert!(text.ends_with(" +5 more"), "{text}");
    }

    #[test]
    fn a_long_output_keeps_its_end() {
        let text = "x".repeat(10) + "end";
        assert_eq!(tail(&text, 3), "[earlier output cut]\nend");
        assert_eq!(tail("short", 10), "short");
    }
}
