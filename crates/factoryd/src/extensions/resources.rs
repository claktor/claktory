//! The Resources extension: how the machine and Factory use its processors,
//! memory and disk, for admin agents such as Rivet to look into and tell the
//! person about. Never to act on: the alert says so.
//!
//! A sampler started with Factory reads the machine and its processes every
//! minute (`sample`) and keeps the last hour in memory; what Factory's data and
//! projects take on disk is measured in the background every 20 minutes
//! (`disk`). When memory stays above `resources.ram_percent`, a volume Factory
//! uses runs short of space, or (if set) the processors stay busier than
//! `cpu_percent`, the admin agents with the extension on hear it once, as a
//! Factory event; it's said again only after the machine recovered. What was
//! said and still holds is kept in factory.db, so a restart doesn't say it again
//! while the machine is still short. A process that uses a full core in every
//! sample for an hour, owned by no agent's task, watch or running command (Part
//! 2 #4: a leftover `cat /dev/zero` did for 37 hours), is told the same way,
//! once while it lives. Other agents with it on can look, but aren't told (one
//! owner for the machine's health). When no admin agent can take it, Claktory
//! notifies the person itself.

mod disk;
mod sample;

use std::collections::{HashMap, HashSet, VecDeque};
use std::fmt::Write as _;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::{Duration, Instant};

use factory_agent::{Tool, ToolResult};
use factory_protocol::v1;
use serde::Deserialize;
use serde_json::{Value, json};

use super::{Extension, Manifest, PromptSection, enabled, factory_tool};
use crate::config::{AgentConfig, Config, ResourcesConfig};
use crate::factory::{Factory, Inner, host_event};
use crate::transcript::now_ms;

pub(crate) const ID: &str = "resources";

/// What was said about the machine and still holds, by what it's
/// about, so a restart doesn't say it again.
pub(crate) const SCHEMA: &str = "
-- Alerts about this machine that were given and still hold (#455), by what
-- they're about (\"ram\", \"cpu\" or \"disk:<mount>\"), with what was said:
-- a restart doesn't give them again. Deleted once it recovers, so a later
-- crossing alerts again.
CREATE TABLE resource_alerts (about TEXT PRIMARY KEY, text TEXT NOT NULL, told INTEGER NOT NULL);
";

static MANIFEST: Manifest = Manifest {
    id: ID,
    name: "Resources",
    icon: "gauge",
    description: "Shows how this computer's processors, memory and disk are used, and how much of them Claktory takes. Rivet hears when memory or disk runs low, or a program keeps a processor core busy for over an hour, and tells you.",
    default_enabled: false,
    options: &[],
};

const PROMPT: &str = "## Resources

The resources tool shows how this machine and Claktory use its processors, memory and disk: the whole machine, Claktory's own processes (its backend and app, agents' background tasks and monitors' watches), the other programs using the most memory, and what Claktory's data and projects take on disk. Claktory samples it every minute and tells you when memory or disk runs low, or when a process no agent's task owns has used a full processor core for over an hour. Then look into it with the tool and tell the person what you found and what they could do. Never stop, kill or delete anything yourself unless the person asks you to, and never change the system, its disks or virtual machines to make room or keep it running: tell the person what they could do.
";

/// Its prompt for an agent that isn't an admin agent: it isn't told.
const PROMPT_LOOK: &str = "## Resources

The resources tool shows how this machine and Claktory use its processors, memory and disk: the whole machine, Claktory's own processes (its backend and app, agents' background tasks and monitors' watches), the other programs using the most memory, and what Claktory's data and projects take on disk. Use it when the machine matters to your task. Rivet watches the machine's health and tells the person when memory or disk runs short, so you aren't told; raise it only when it blocks your own work or you caused it. Never stop, kill or delete anything yourself unless the person asks you to.
";

const ABOUT: &str = "How this machine and Claktory use its resources, in a few lines: processors, memory, swap and free disk; Claktory's processes; the other programs using the most memory; alerts in effect. `detail`: \"processes\" (every process group of Claktory's and the top others), \"disk\" (each volume, and what Claktory's data, agents and projects take) or \"history\" (the last hour, every 5 minutes). `clear_cache`: the path of a build cache (shown under detail \"disk\") to clear now, only when the person asks: it's moved aside and deleted, and the next build there is cold.";

/// Check for updates now, as Settings' button does.
pub(crate) const UPDATES_TOOL: &str = "updates_check";
const UPDATES: &str = "Check now for a newer version of Claktory, as the person's Check for updates in Settings does, and say what it found: up to date, a new version downloading, or one ready to install. Only the person installs it, from the Update banner in the app, when they choose.";
/// How long updates_check waits for the check to finish.
const UPDATES_WAIT: Duration = Duration::from_secs(60);

#[derive(Deserialize)]
struct NoArgs {}

/// updates_check: checks the release channel now (`Factory::check_for_updates`,
/// which a check run lately answers without asking again) and says what it
/// found, once it's done or after a minute.
async fn check_updates(factory: Arc<Factory>, agent: String, _args: NoArgs) -> Result<ToolResult, String> {
    let person = {
        let inner = factory.lock();
        super::caller(&inner.config, &agent)?;
        inner.config.person_capitalized().to_owned()
    };
    factory.check_for_updates().map_err(|error| error.to_string())?;
    let started = Instant::now();
    let check = loop {
        let check = factory.lock().update_check.clone();
        if check.state() != v1::UpdateCheckState::Checking || started.elapsed() > UPDATES_WAIT {
            break check;
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    };
    let running = crate::update::VERSION;
    let said = match check.state() {
        v1::UpdateCheckState::Ready => format!(
            "Claktory {} is ready to install (this one is {running}). {person} installs it from the Update banner in the app when they choose; you can't install it.",
            check.version
        ),
        v1::UpdateCheckState::Downloading => {
            format!("Claktory {} is downloading. Once it's ready, {person} sees the Update banner in the app and installs it when they choose.", check.version)
        }
        v1::UpdateCheckState::UpToDate => format!("This Claktory is up to date: {running} is the latest."),
        v1::UpdateCheckState::Failed => format!("Claktory couldn't check for updates: {}", check.error),
        v1::UpdateCheckState::Checking | v1::UpdateCheckState::None => {
            "The check is still going after a minute; Settings shows what it finds.".to_owned()
        }
    };
    Ok(ToolResult::text(said, json!({"state": check.state().as_str_name(), "version": check.version})))
}

/// Samples kept: an hour's.
const HISTORY: usize = 60;
/// Samples in a row above the line before memory or the processors alert.
const RAM_SAMPLES: u32 = 3;
const CPU_SAMPLES: u32 = 5;
/// How long a process uses a full core's worth of processor time
/// (`sample::FULL_CORE` on average), no agent's task owning it, before it's
/// told.
const STRAY: Duration = Duration::from_secs(3600);
/// Points kept of a watched process's processor time over `STRAY`: one
/// every twelfth of it, so it's judged over an hour and at most 5 minutes.
const STRAY_POINTS: i64 = 12;
/// The other processes the panel shows.
const OTHERS_SHOWN: usize = 5;
/// How long one disk measurement may walk.
const DISK_BUDGET: Duration = Duration::from_secs(300);

pub(crate) struct Resources;

/// A process busy in some sample, watched: its processor time (ms) at
/// times (ms) over the last `STRAY` and a little more, when it was last
/// busy, and whether it was told.
#[derive(Default)]
struct Stray {
    points: VecDeque<(i64, u64)>,
    seen: i64,
    told: bool,
}

/// Over a line: samples in a row, and whether the agents were told.
#[derive(Default)]
struct Crossing {
    over: u32,
    told: bool,
}

#[derive(Default)]
struct Now {
    sampled: i64,
    machine: v1::Machine,
    factory: Vec<v1::ResourceProcess>,
    others: Vec<v1::ResourceProcess>,
    history: VecDeque<v1::ResourceSample>,
    measured: i64,
    disk: Vec<v1::DiskUsage>,
    /// Why the machine can't be read, if it can't.
    error: Option<String>,
    ram: Crossing,
    cpu: Crossing,
    /// By mount point.
    disks: HashMap<String, Crossing>,
    /// Processes busy in a sample over the last `STRAY`, by pid and start:
    /// told once each, kept while they live.
    strays: HashMap<(u32, u64), Stray>,
    /// What the agents were told, while it still holds, by what it's about.
    alerts: Vec<(String, String)>,
    /// The last save of `alerts` to factory.db failed: the next sample
    /// saves them again.
    unkept: bool,
}

pub(crate) struct State {
    every_ms: AtomicU64,
    disk_every_ms: AtomicU64,
    stray_ms: AtomicU64,
    /// Percent of one core that counts as a full one, as f32 bits.
    full_core: std::sync::atomic::AtomicU32,
    now: Mutex<Now>,
}

impl Default for State {
    fn default() -> State {
        State {
            every_ms: AtomicU64::new(60_000),
            disk_every_ms: AtomicU64::new(20 * 60_000),
            stray_ms: AtomicU64::new(u64::try_from(STRAY.as_millis()).unwrap_or(u64::MAX)),
            full_core: std::sync::atomic::AtomicU32::new(sample::FULL_CORE.to_bits()),
            now: Mutex::default(),
        }
    }
}

impl State {
    fn now(&self) -> std::sync::MutexGuard<'_, Now> {
        self.now.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn every(&self) -> Duration {
        Duration::from_millis(self.every_ms.load(Ordering::Relaxed))
    }

    fn disk_every(&self) -> Duration {
        Duration::from_millis(self.disk_every_ms.load(Ordering::Relaxed))
    }

    /// How long a process uses a full core's worth before it's told, in
    /// ms: `STRAY`, by the clock, however often samples come.
    fn stray_ms(&self) -> i64 {
        i64::try_from(self.stray_ms.load(Ordering::Relaxed)).unwrap_or(i64::MAX)
    }
}

impl Factory {
    /// How often the machine is sampled (a minute) and the disk measured (20
    /// minutes); tests make them shorter.
    pub fn set_resources_every(&self, sample: Duration, disk: Duration) {
        let ms = |every: Duration| u64::try_from(every.as_millis()).unwrap_or(u64::MAX);
        self.resources.every_ms.store(ms(sample), Ordering::Relaxed);
        self.resources.disk_every_ms.store(ms(disk), Ordering::Relaxed);
    }

    /// How long a process uses a full core's worth before it's told (an
    /// hour), and what percent of one core on average counts as full (90);
    /// tests make them smaller, since a busy machine gives no process a
    /// whole core.
    pub fn set_resources_stray(&self, after: Duration, core_percent: f32) {
        self.resources.stray_ms.store(u64::try_from(after.as_millis()).unwrap_or(u64::MAX), Ordering::Relaxed);
        self.resources.full_core.store(core_percent.to_bits(), Ordering::Relaxed);
    }
}

fn gb(bytes: u64) -> String {
    #[allow(clippy::cast_precision_loss)] // shown to one decimal
    let gb = bytes as f64 / 1e9;
    if gb >= 100.0 { format!("{gb:.0} GB") } else { format!("{gb:.1} GB") }
}

/// Bytes as people read them: MB under a GB, KB under a MB.
fn size(bytes: u64) -> String {
    match bytes {
        1_000_000_000.. => gb(bytes),
        1_000_000.. => format!("{} MB", bytes / 1_000_000),
        _ => format!("{} KB", bytes.div_ceil(1_000)),
    }
}

#[allow(clippy::cast_precision_loss)] // a percentage
fn percent(part: u64, whole: u64) -> f64 {
    if whole == 0 { 0.0 } else { part as f64 * 100.0 / whole as f64 }
}

fn short(command: &str) -> String {
    let line = command.lines().next().unwrap_or_default();
    if line.chars().count() > 60 || command.contains('\n') {
        format!("{}…", line.chars().take(60).collect::<String>())
    } else {
        line.to_owned()
    }
}

fn ago(ms: i64) -> String {
    let seconds = (now_ms() - ms).max(0) / 1000;
    match seconds {
        0..60 => format!("{seconds}s ago"),
        60..3600 => format!("{} min ago", seconds / 60),
        _ => format!("{}h ago", seconds / 3600),
    }
}

fn clock(ms: i64) -> String {
    crate::transcript::local_time(ms, "%H:%M")
}

/// What a process is, with whose it is.
fn named(config: &Config, process: &v1::ResourceProcess) -> String {
    match config.agent(&process.agent_id) {
        Some(agent) => format!("{}'s {}", agent.name, process.label),
        None => process.label.clone(),
    }
}

/// The first few, as "name size".
fn few(config: &Config, processes: &[v1::ResourceProcess], count: usize) -> String {
    processes
        .iter()
        .take(count)
        .map(|process| format!("{} {}", named(config, process), size(process.memory)))
        .collect::<Vec<_>>()
        .join(", ")
}

/// What Factory knows of its own processes and paths now.
fn known(factory: &Factory) -> sample::Known {
    let app = if factory.home.root().file_name().is_some_and(|name| name == ".factory-dev") { "Factory Dev.app" } else { "Factory.app" };
    let mut groups = Vec::new();
    match factory.store.running_tasks() {
        Ok(tasks) => groups.extend(tasks.into_iter().map(|(agent, task)| sample::Group {
            pgid: task.pid,
            label: format!("{} `{}`", task.id, short(&task.command)),
            kind: "task",
            agent,
        })),
        Err(error) => tracing::error!(%error, "could not read the running tasks"),
    }
    let active = super::monitors::Query { active: true, ..Default::default() };
    match super::monitors::query(&factory.store, &active) {
        Ok(monitors) => groups.extend(monitors.into_iter().filter_map(|monitor| {
            let pgid = monitor.cursor.pid?;
            Some(sample::Group { pgid, label: format!("monitor m{} {}", monitor.id, monitor.name), kind: "monitor", agent: monitor.agent })
        })),
        Err(error) => tracing::error!(%error, "could not read the monitors"),
    }
    let inner = factory.lock();
    let mut paths = vec![(factory.home.root().to_owned(), "Claktory's data".to_owned())];
    for project in &inner.config.projects {
        paths.push((project.path.clone(), project.name.clone()));
        paths.extend(project.folders.iter().map(|folder| (folder.path.clone(), project.name.clone())));
    }
    let full_core = f32::from_bits(factory.resources.full_core.load(Ordering::Relaxed));
    sample::Known { app, groups, paths, full_core }
}

fn plan(factory: &Factory) -> disk::Plan {
    let inner = factory.lock();
    disk::Plan {
        home: factory.home.root().to_owned(),
        agents: inner.config.agents.iter().map(|agent| (agent.id.clone(), agent.name.clone())).collect(),
        projects: inner
            .config
            .projects
            .iter()
            .map(|project| disk::Project {
                id: project.id.clone(),
                name: project.name.clone(),
                path: project.path.clone(),
                folders: project
                    .folders
                    .iter()
                    .filter(|folder| !folder.path.starts_with(&project.path))
                    .map(|folder| (folder.name.clone(), folder.path.clone()))
                    .collect(),
            })
            .collect(),
    }
}

/// A disk as the person knows it: the startup disk, or the name the Mac
/// shows for another (its folder in /Volumes), never a bare path.
fn disk_name(mount: &str) -> String {
    match std::path::Path::new(mount).file_name() {
        Some(name) if mount != "/" => format!("The disk \u{201c}{}\u{201d}", name.to_string_lossy()),
        _ => "This computer's disk".to_owned(),
    }
}

/// An alert to tell: what it's about ("ram", "cpu", "disk:<mount>"), what
/// an admin agent is told, and Claktory's own sentence for the person when
/// no admin agent can take it.
struct Said {
    about: String,
    text: String,
    person: String,
}

/// Checks a new sample against the lines; returns what to tell. Each is
/// said once per crossing.
fn check(
    now: &mut Now,
    lines: &ResourcesConfig,
    config: &Config,
    hot: &[sample::Busy],
    alive: &HashSet<(u32, u64)>,
    (stray_ms, full_core): (i64, f32),
) -> Vec<Said> {
    let mut said = Vec::new();
    let advice = format!(
        "Look into it with the resources tool and tell {} what you find and what they could do. Don't stop, kill or delete anything yourself.",
        config.person()
    );
    let machine = now.machine.clone();
    let memory = percent(machine.memory_used, machine.memory_total);
    let busy = |crossing: &mut Crossing, over: bool, samples: u32| {
        if !over {
            *crossing = Crossing::default();
            return false;
        }
        crossing.over += 1;
        let tell = crossing.over >= samples && !crossing.told;
        crossing.told |= tell;
        tell
    };
    if busy(&mut now.ram, memory > f64::from(lines.ram_percent), RAM_SAMPLES) {
        let mut text = format!(
            "This machine is running low on memory: {} of {} used ({memory:.0}%, above {}%) for the last {RAM_SAMPLES} minutes",
            gb(machine.memory_used),
            gb(machine.memory_total),
            lines.ram_percent
        );
        if machine.swap_used > 0 {
            let _ = write!(text, ", and {} of swap", gb(machine.swap_used));
        }
        let factory_memory: u64 = now.factory.iter().map(|process| process.memory).sum();
        let _ = write!(
            text,
            ". Claktory's processes use {} (most: {}); the other programs using the most: {}. {advice}",
            size(factory_memory),
            few(config, &now.factory, 3),
            few(config, &now.others, 3)
        );
        let person = format!(
            "This computer is running low on memory: {} of {} used ({memory:.0}%).",
            gb(machine.memory_used),
            gb(machine.memory_total)
        );
        said.push(Said { about: "ram".to_owned(), text, person });
    }
    if lines.cpu_percent == 0 {
        // Off: nothing holds, kept or not.
        now.cpu = Crossing::default();
    }
    if lines.cpu_percent > 0 && busy(&mut now.cpu, machine.cpu_percent > f64::from(lines.cpu_percent), CPU_SAMPLES) {
        let mut busiest: Vec<&v1::ResourceProcess> = now.factory.iter().chain(&now.others).collect();
        busiest.sort_by(|a, b| b.cpu_percent.total_cmp(&a.cpu_percent));
        let busiest: Vec<String> =
            busiest.iter().take(3).map(|process| format!("{} {:.0}%", named(config, process), process.cpu_percent)).collect();
        said.push(Said {
            about: "cpu".to_owned(),
            text: format!(
                "This machine's processors have been busy: {:.0}% of {} cores (above {}%) for the last {CPU_SAMPLES} minutes. Busiest: {}. {advice}",
                machine.cpu_percent,
                machine.cores,
                lines.cpu_percent,
                busiest.join(", ")
            ),
            person: format!(
                "This computer's processors have been busy for {CPU_SAMPLES} minutes: {:.0}% of {} cores. Busiest: {}.",
                machine.cpu_percent,
                machine.cores,
                busiest.join(", ")
            ),
        });
    }
    for volume in &machine.volumes {
        let limit = (volume.total / 100 * u64::from(lines.disk_free_percent)).min(u64::from(lines.disk_free_gb) * 1_000_000_000);
        let crossing = now.disks.entry(volume.mount.clone()).or_default();
        if !busy(crossing, volume.available < limit, 1) {
            continue;
        }
        let mut text = format!(
            "The volume {} is running out of space: {} free of {} ({:.1}%). It holds {}.",
            volume.mount,
            gb(volume.available),
            gb(volume.total),
            percent(volume.available, volume.total),
            volume.uses.join(", ")
        );
        if now.measured > 0 {
            let biggest: Vec<String> =
                now.disk.iter().filter(|row| row.kind != "home").take(3).map(|row| format!("{} {}", row.label, size(row.bytes))).collect();
            let _ = write!(text, " Of Claktory's, the most ({}): {}.", ago(now.measured), biggest.join(", "));
        }
        let _ = write!(text, " {advice}");
        let person = format!("{} is running out of space: {} free of {}.", disk_name(&volume.mount), gb(volume.available), gb(volume.total));
        said.push(Said { about: format!("disk:{}", volume.mount), text, person });
    }
    // A process that used a full core's worth of processor time over the
    // last hour, owned by no agent's task (Part 2 #4): told once while it
    // lives, whatever it does after. Judged by its processor time, never
    // sample by sample, so a loaded machine that starves it now and then
    // doesn't hide it, and by the clock, so samples far
    // apart (a machine asleep) don't make one.
    let at = now.sampled;
    now.strays.retain(|id, stray| alive.contains(id) && (stray.told || at.saturating_sub(stray.seen) <= stray_ms));
    for process in hot {
        let stray = now.strays.entry(process.id).or_default();
        stray.seen = at;
        if stray.told {
            continue;
        }
        if stray.points.back().is_none_or(|(then, _)| at.saturating_sub(*then) >= stray_ms / STRAY_POINTS) {
            stray.points.push_back((at, process.cpu_ms));
        }
        // From the last point at least `STRAY` ago, the ones before it gone.
        while stray.points.get(1).is_some_and(|(then, _)| at.saturating_sub(*then) >= stray_ms) {
            stray.points.pop_front();
        }
        let Some(&(then, cpu_then)) = stray.points.front().filter(|(then, _)| at.saturating_sub(*then) >= stray_ms) else { continue };
        #[allow(clippy::cast_precision_loss)] // a share of a core
        let average = process.cpu_ms.saturating_sub(cpu_then) as f64 / (at - then) as f64 * 100.0;
        if average < f64::from(full_core) {
            continue;
        }
        stray.told = true;
        stray.points = VecDeque::new();
        let (pid, started) = process.id;
        let command = if process.command.is_empty() { String::new() } else { format!(", `{}`", short(&process.command)) };
        let since = i64::try_from(started).map_or_else(|_| String::new(), |started| {
            format!(", started {}", crate::transcript::local_time(started * 1000, "%b %-d %H:%M"))
        });
        said.push(Said {
            about: format!("process:{pid}:{started}"),
            text: format!(
                "A process has used a full processor core for over an hour: {} (pid {pid}{command}{since}), {average:.0}% of a core on average over the last hour, {:.0}% now. No agent's task owns it. {advice}",
                process.name, process.cpu_percent
            ),
            person: format!("A program has used a full processor core on this computer for over an hour: {} (pid {pid}).", process.name),
        });
    }
    // What's no longer over its line is over.
    let holding = |about: &str| match about {
        "ram" => now.ram.told,
        "cpu" => now.cpu.told,
        process if process.starts_with("process:") => stray(process).is_some_and(|id| now.strays.get(&id).is_some_and(|stray| stray.told)),
        disk => now.disks.get(disk.trim_start_matches("disk:")).is_some_and(|crossing| crossing.told),
    };
    let mut alerts: Vec<(String, String)> = now.alerts.drain(..).filter(|(about, _)| holding(about)).collect();
    alerts.extend(said.iter().map(|said| (said.about.clone(), said.text.clone())));
    now.alerts = alerts;
    said
}

/// A process alert's pid and start ("process:<pid>:<start>").
fn stray(about: &str) -> Option<(u32, u64)> {
    let (pid, started) = about.strip_prefix("process:")?.split_once(':')?;
    Some((pid.parse().ok()?, started.parse().ok()?))
}

/// Takes a sample in: history, alerts and the panel. An alert goes to the
/// agents that look after the machine (`keepers`: the admin agents unless
/// the person chose otherwise, and those they chose); with none, Claktory
/// tells the person itself.
fn record(factory: &Arc<Factory>, sampled: sample::Sample) {
    let time = now_ms();
    let (lines, config) = {
        let inner = factory.lock();
        (inner.config.resources.clone(), inner.config.clone())
    };
    let (said, kept) = {
        let mut now = factory.resources.now();
        let before: Vec<String> = now.alerts.iter().map(|(about, _)| about.clone()).collect();
        let factory_memory = sampled.factory.iter().map(|process| process.memory).sum();
        now.history.push_back(v1::ResourceSample {
            time,
            cpu_percent: sampled.machine.cpu_percent,
            memory_used: sampled.machine.memory_used,
            swap_used: sampled.machine.swap_used,
            factory_memory,
        });
        while now.history.len() > HISTORY {
            now.history.pop_front();
        }
        now.sampled = time;
        now.machine = sampled.machine;
        now.factory = sampled.factory;
        now.others = sampled.others;
        let stray = (factory.resources.stray_ms(), f32::from_bits(factory.resources.full_core.load(Ordering::Relaxed)));
        let said = check(&mut now, &lines, &config, &sampled.busy, &sampled.alive, stray);
        let changed = now.alerts.len() != before.len() || now.alerts.iter().zip(&before).any(|((about, _), was)| about != was);
        let keep = changed || std::mem::take(&mut now.unkept);
        (said, keep.then(|| now.alerts.clone()))
    };
    if let Some(alerts) = kept
        && !keep(factory, &alerts, time)
    {
        factory.resources.now().unkept = true;
    }
    if said.is_empty() {
        return refresh_panel(factory);
    }
    let told = keepers(factory);
    for said in said {
        if told.is_empty() {
            tell_person(factory, &config, said.person);
            continue;
        }
        for agent in &told {
            let event = host_event(String::new());
            factory.deliver_event(agent, said.text.clone(), event);
        }
    }
    refresh_panel(factory);
}

/// Keeps the alerts that hold now in factory.db, in place of the last;
/// false if it couldn't.
fn keep(factory: &Factory, alerts: &[(String, String)], time: i64) -> bool {
    let mut db = factory.store.db();
    let kept = db.transaction().and_then(|transaction| {
        // Those still holding keep when they were told.
        let holding: Vec<&str> = alerts.iter().map(|(about, _)| about.as_str()).collect();
        let gone: Vec<String> = {
            let mut statement = transaction.prepare("SELECT about FROM resource_alerts")?;
            let rows = statement.query_map([], |row| row.get::<_, String>(0))?.collect::<Result<Vec<_>, _>>()?;
            rows.into_iter().filter(|about| !holding.contains(&about.as_str())).collect()
        };
        for about in gone {
            transaction.execute("DELETE FROM resource_alerts WHERE about = ?1", [about])?;
        }
        for (about, text) in alerts {
            transaction.execute(
                "INSERT INTO resource_alerts (about, text, told) VALUES (?1, ?2, ?3) ON CONFLICT (about) DO NOTHING",
                rusqlite::params![about, text, time],
            )?;
        }
        transaction.commit()
    });
    if let Err(error) = &kept {
        tracing::error!(%error, "could not keep the machine's alerts; trying again at the next sample");
    }
    kept.is_ok()
}

/// The alerts that held when Factory last stopped: told already, so not
/// said again until the machine recovers.
fn kept(factory: &Factory) {
    let rows = {
        let db = factory.store.db();
        let rows = db.prepare("SELECT about, text FROM resource_alerts ORDER BY told").and_then(|mut statement| {
            statement.query_map([], |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)))?.collect::<Result<Vec<_>, _>>()
        });
        match rows {
            Ok(rows) => rows,
            Err(error) => return tracing::error!(%error, "could not read the machine's alerts"),
        }
    };
    let mut now = factory.resources.now();
    for (about, text) in rows {
        match about.as_str() {
            "ram" => now.ram.told = true,
            "cpu" => now.cpu.told = true,
            process if process.starts_with("process:") => {
                let Some(id) = stray(process) else { continue };
                now.strays.insert(id, Stray { told: true, ..Stray::default() });
            }
            disk => now.disks.entry(disk.trim_start_matches("disk:").to_owned()).or_default().told = true,
        }
        now.alerts.push((about, text));
    }
}

/// The agents that look after the machine (`hearers`) and are told now.
fn keepers(factory: &Factory) -> Vec<String> {
    let inner = factory.lock();
    hearers(&inner.config, &inner.models).into_iter().filter(|hears| hears.told).map(|hears| hears.agent_id).collect()
}

/// Whether `agent` is one that hears the alerts (`hearers`), told or not:
/// its prompt says so.
fn hears(config: &Config, agent: &AgentConfig) -> bool {
    (config.resources.alerts_to_admins && super::switch(agent, super::agents::ID, "admin"))
        || config.resources.alert_agents.contains(&agent.id)
}

/// Everyone who hears the alerts, as Settings shows them:
/// the admin agents (such as Rivet) while `resources.alerts_to_admins` is
/// on, then the agents chosen in `resources.alert_agents`, each once; told
/// when it has the extension on, is in no archived project, and its model
/// can run.
pub(crate) fn hearers(config: &Config, models: &[v1::Model]) -> Vec<v1::ResourceAlertAgent> {
    use v1::ResourceAlertHeld as Held;
    let admins = config
        .agents
        .iter()
        .filter(|agent| config.resources.alerts_to_admins && super::switch(agent, super::agents::ID, "admin"))
        .map(|agent| (agent.id.clone(), true));
    let chosen = config.resources.alert_agents.iter().map(|id| (id.clone(), false));
    let mut listed: Vec<v1::ResourceAlertAgent> = Vec::new();
    for (id, admin) in admins.chain(chosen) {
        if listed.iter().any(|hears| hears.agent_id == id) {
            continue;
        }
        let held = match config.agent(&id) {
            None => Held::NoAgent,
            Some(agent) if !enabled(agent, ID) => Held::ResourcesOff,
            Some(agent) if config.archived(agent) => Held::Archived,
            Some(agent) if !models.iter().any(|model| model.id == agent.model) => Held::NoModel,
            Some(_) => Held::Unspecified,
        };
        listed.push(v1::ResourceAlertAgent { agent_id: id, told: held == Held::Unspecified, held: held.into(), admin });
    }
    listed
}

/// No agent can take an alert: Claktory notifies the person itself, as an
/// alert, in its own words.
fn tell_person(factory: &Factory, config: &Config, what: String) {
    let delivered = crate::notify::host_alert(factory, config, &what);
    tracing::info!(?delivered, "told the person about the machine itself: no admin agent can take it");
}

/// The selected agent's panel, if it's this one.
fn refresh_panel(factory: &Factory) {
    let selected = {
        let inner = factory.lock();
        inner.state.selected_agent.clone().filter(|agent| inner.state.open_panels.get(agent).is_some_and(|panel| panel == ID))
    };
    if let Some(agent) = selected {
        factory.refresh_view(&agent);
    }
}

/// Samples the machine every minute, for as long as Factory runs.
pub(crate) async fn monitor(factory: Weak<Factory>) {
    if let Some(factory) = factory.upgrade() {
        kept(&factory);
    }
    let mut sampler = match tokio::task::spawn_blocking(sample::Sampler::new).await {
        Ok(Ok(sampler)) => sampler,
        Ok(Err(error)) => return unreadable(&factory, &error),
        Err(error) => return unreadable(&factory, &error.to_string()),
    };
    tokio::spawn(measure(factory.clone()));
    // CPU use needs a moment between readings.
    let mut wait = sysinfo::MINIMUM_CPU_UPDATE_INTERVAL.max(Duration::from_millis(500));
    loop {
        tokio::time::sleep(wait).await;
        let Some(strong) = factory.upgrade() else { return };
        wait = strong.resources.every();
        let known = known(&strong);
        drop(strong);
        let Ok((back, sampled)) = tokio::task::spawn_blocking(move || {
            let sampled = sampler.sample(&known);
            (sampler, sampled)
        })
        .await
        else {
            return;
        };
        sampler = back;
        let Some(strong) = factory.upgrade() else { return };
        record(&strong, sampled);
    }
}

fn unreadable(factory: &Weak<Factory>, error: &str) {
    tracing::error!(%error, "can't read this machine's resources");
    if let Some(factory) = factory.upgrade() {
        factory.resources.now().error = Some(error.to_owned());
    }
}

/// Shows the build caches as last found (`tree::caches`) among the disk's
/// rows: each agent's checkouts' and each project folder's, as parts of
/// its row, and refreshes the panel.
pub(crate) fn show_caches(factory: &Factory) {
    let found = factory.build_caches.found();
    {
        let mut now = factory.resources.now();
        for row in &mut now.disk {
            row.parts.retain(|part| part.kind != "build_cache");
            for place in &found {
                let ours = match row.kind.as_str() {
                    "agent" => place.agent.as_deref() == Some(row.id.as_str()),
                    "project" | "folder" => place.agent.is_none() && std::path::Path::new(&row.path) == place.path,
                    _ => false,
                };
                if !ours {
                    continue;
                }
                for cache in &place.caches {
                    let inside = cache.path.strip_prefix(&place.path).unwrap_or(&cache.path);
                    row.parts.push(v1::DiskUsage {
                        label: format!("Build cache {}", inside.display()),
                        kind: "build_cache".to_owned(),
                        id: row.id.clone(),
                        path: cache.path.display().to_string(),
                        bytes: cache.bytes,
                        parts: Vec::new(),
                        partial: cache.partial,
                    });
                }
            }
            row.parts.sort_by_key(|part| std::cmp::Reverse(part.bytes));
        }
    }
    refresh_panel(factory);
}

/// Measures the disk in the background: shortly after start, then every
/// 20 minutes.
async fn measure(factory: Weak<Factory>) {
    let mut wait = Duration::from_secs(60);
    loop {
        let first = factory.upgrade().map(|factory| factory.resources.disk_every());
        let Some(every) = first else { return };
        tokio::time::sleep(wait.min(every)).await;
        wait = every;
        let Some(plan) = factory.upgrade().map(|factory| plan(&factory)) else { return };
        let Ok(rows) = tokio::task::spawn_blocking(move || disk::measure(&plan, Instant::now() + DISK_BUDGET)).await else { return };
        let Some(factory) = factory.upgrade() else { return };
        {
            let mut now = factory.resources.now();
            now.disk = rows;
            now.measured = now_ms();
        }
        show_caches(&factory);
    }
}

#[derive(Deserialize)]
struct Args {
    #[serde(default)]
    detail: Option<String>,
    #[serde(default)]
    clear_cache: Option<String>,
}

fn machine_line(machine: &v1::Machine) -> String {
    let mut line = format!(
        "Machine: CPU {:.0}% of {} cores, memory {} of {} used ({:.0}%), swap {} of {}.",
        machine.cpu_percent,
        machine.cores,
        gb(machine.memory_used),
        gb(machine.memory_total),
        percent(machine.memory_used, machine.memory_total),
        gb(machine.swap_used),
        gb(machine.swap_total)
    );
    for volume in &machine.volumes {
        let _ = write!(
            line,
            " Disk {}: {} free of {} ({:.0}%).",
            volume.mount,
            gb(volume.available),
            gb(volume.total),
            percent(volume.available, volume.total)
        );
    }
    line
}

fn process_line(config: &Config, process: &v1::ResourceProcess) -> String {
    let count = if process.processes > 1 { format!(", {} processes", process.processes) } else { String::new() };
    format!("- {}: {}, CPU {:.0}%{count} (pid {})", named(config, process), size(process.memory), process.cpu_percent, process.pid)
}

fn disk_row(out: &mut String, row: &v1::DiskUsage, indent: &str) {
    let partial = if row.partial { " or more (the walk took too long)" } else { "" };
    let _ = writeln!(out, "{indent}- {}: {}{partial} ({})", row.label, size(row.bytes), row.path);
}

fn run(factory: &Factory, _agent: &str, args: Value) -> Result<String, String> {
    let args: Args = serde_json::from_value(args).map_err(|error| error.to_string())?;
    if let Some(path) = args.clear_cache.as_deref().filter(|path| !path.trim().is_empty()) {
        return super::tree::caches::clear_on_request(factory, path);
    }
    let config = factory.lock().config.clone();
    let now = factory.resources.now();
    if let Some(error) = &now.error {
        return Err(format!("This machine's resources can't be read: {error}"));
    }
    if now.sampled == 0 {
        return Err("Claktory hasn't sampled the machine yet; ask again in a few seconds.".to_owned());
    }
    let mut out = String::new();
    match args.detail.as_deref().map(str::trim).unwrap_or_default() {
        "" => {
            let _ = writeln!(out, "{}", machine_line(&now.machine));
            let factory_memory: u64 = now.factory.iter().map(|process| process.memory).sum();
            let factory_cpu: f64 = now.factory.iter().map(|process| process.cpu_percent).sum();
            let _ = writeln!(out, "Claktory: {}, CPU {factory_cpu:.0}%: {}.", size(factory_memory), few(&config, &now.factory, 4));
            let _ = writeln!(out, "Others by memory: {}.", few(&config, &now.others, OTHERS_SHOWN));
            if now.alerts.is_empty() {
                let _ = writeln!(out, "No alerts.");
            } else {
                for (_, text) in &now.alerts {
                    let _ = writeln!(out, "Alert: {}.", text.split(". ").next().unwrap_or(text).trim_end_matches('.'));
                }
            }
            let _ = write!(out, "Sampled {}. More: detail \"processes\", \"disk\" or \"history\".", ago(now.sampled));
        }
        "processes" => {
            let _ = writeln!(out, "{}", machine_line(&now.machine));
            let _ = writeln!(out, "Claktory's processes (CPU: 100% is one core, over the last minute):");
            for process in &now.factory {
                let _ = writeln!(out, "{}", process_line(&config, process));
            }
            let _ = writeln!(out, "Other programs using the most memory:");
            for process in &now.others {
                let _ = writeln!(out, "{}", process_line(&config, process));
            }
            let _ = write!(out, "Sampled {}.", ago(now.sampled));
        }
        "disk" => {
            for volume in &now.machine.volumes {
                let _ = writeln!(
                    out,
                    "Volume {}: {} free of {} ({:.0}%); holds {}.",
                    volume.mount,
                    gb(volume.available),
                    gb(volume.total),
                    percent(volume.available, volume.total),
                    volume.uses.join(", ")
                );
            }
            if now.measured == 0 {
                let _ = write!(
                    out,
                    "What Claktory's data and projects take isn't measured yet: it's measured in the background, first a minute after Claktory starts."
                );
                return Ok(out);
            }
            let _ = writeln!(
                out,
                "Measured {} (every 20 minutes). Checkouts are copy-on-write clones, so most of their bytes are shared. Build caches over {} in a checkout or project folder are cleared on their own while nothing works there; clear_cache clears one now.",
                ago(now.measured),
                if config.resources.build_cache_gb > 0.0 {
                    gb((config.resources.build_cache_gb * 1e9) as u64)
                } else {
                    "the budget (none set)".to_owned()
                }
            );
            for row in &now.disk {
                disk_row(&mut out, row, "");
                for part in &row.parts {
                    disk_row(&mut out, part, "  ");
                }
            }
        }
        "history" => {
            let _ = writeln!(out, "The last hour, every 5 minutes (memory used, swap used, Claktory's memory, CPU):");
            for sample in now.history.iter().rev().step_by(5).collect::<Vec<_>>().into_iter().rev() {
                let _ = writeln!(
                    out,
                    "- {}: memory {}, swap {}, Claktory {}, CPU {:.0}%",
                    clock(sample.time),
                    gb(sample.memory_used),
                    gb(sample.swap_used),
                    size(sample.factory_memory),
                    sample.cpu_percent
                );
            }
        }
        other => return Err(format!("detail is \"processes\", \"disk\" or \"history\", or left out; got \"{other}\".")),
    }
    Ok(out.trim_end().to_owned())
}

impl Extension for Resources {
    fn manifest(&self) -> &'static Manifest {
        &MANIFEST
    }

    fn admin_default(&self) -> bool {
        true
    }

    fn prompt(&self, _factory: &Factory, inner: &Inner, agent: &AgentConfig) -> Vec<PromptSection> {
        let text = if hears(&inner.config, agent) { PROMPT } else { PROMPT_LOOK };
        vec![PromptSection::static_text(text)]
    }

    fn tools(&self, factory: &Arc<Factory>, agent: &str) -> Vec<Tool> {
        let parameters = json!({
            "type": "object",
            "properties": {
                "detail": {"type": "string", "enum": ["processes", "disk", "history"]},
                "clear_cache": {"type": "string"},
            },
            "additionalProperties": false,
        });
        let nothing = json!({"type": "object", "properties": {}, "additionalProperties": false});
        vec![
            factory_tool(factory, agent, "resources", "Resources", ABOUT, parameters, run),
            super::async_tool(factory, agent, UPDATES_TOOL, "Check for updates", UPDATES, nothing, check_updates),
        ]
    }

    fn owns_tool(&self, name: &str) -> bool {
        name == ID || name == UPDATES_TOOL
    }

    fn view(&self, factory: &Factory, _inner: &Inner, agent: &str) -> Option<v1::ExtensionView> {
        let now = factory.resources.now();
        let resources = v1::Resources {
            machine: Some(now.machine.clone()),
            factory: now.factory.clone(),
            others: now.others.iter().take(OTHERS_SHOWN).cloned().collect(),
            disk: now.disk.clone(),
            history: now.history.iter().cloned().collect(),
            sampled: now.sampled,
            measured: now.measured,
            alerts: now.alerts.iter().map(|(_, text)| text.clone()).collect(),
        };
        Some(v1::ExtensionView {
            extension_id: ID.to_owned(),
            agent_id: agent.to_owned(),
            title: MANIFEST.name.to_owned(),
            body: Some(v1::extension_view::Body::Resources(resources)),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{Config, Now, ResourcesConfig, check, disk_name, sample};
    use std::collections::HashSet;

    const HOUR: (i64, f32) = (3_600_000, sample::FULL_CORE);

    /// A process `cat` busy in a sample at `minute` with `cpu_ms` used so
    /// far, or (None) not busy in it; what Rivet was told.
    fn at_minute(now: &mut Now, alive: &HashSet<(u32, u64)>, minute: i64, cpu_ms: Option<u64>) -> Vec<String> {
        let (config, lines) = (Config::default(), ResourcesConfig { ram_percent: 100, disk_free_gb: 0, cpu_percent: 0, ..ResourcesConfig::default() });
        now.sampled = minute * 60_000;
        let hot: Vec<sample::Busy> = cpu_ms
            .map(|cpu_ms| sample::Busy { id: CAT, name: "cat".into(), command: "cat /dev/zero".into(), cpu_percent: 99.6, cpu_ms })
            .into_iter()
            .collect();
        check(now, &lines, &config, &hot, alive, HOUR).into_iter().map(|said| said.text).collect()
    }

    const CAT: (u32, u64) = (3063, 1_795_000_000);

    /// A process is told once it used a full
    /// core's worth of processor time over an hour by the clock, though a
    /// loaded machine starved it in some samples (here 1.4 cores most
    /// minutes, a twentieth of one every fifth), once while it lives; gone,
    /// its alert goes.
    #[test]
    fn a_runaway_starved_now_and_then_is_told_once_after_an_hour_of_processor_time() {
        let (mut now, alive) = (Now::default(), HashSet::from([CAT]));
        let mut cpu_ms = 0;
        let mut told = Vec::new();
        for minute in 0..=70 {
            let starved = minute % 5 == 0;
            cpu_ms += if starved { 3_000 } else { 84_000 };
            let said = at_minute(&mut now, &alive, minute, (!starved).then_some(cpu_ms));
            assert!(minute >= 61 || said.is_empty(), "told at minute {minute}: {said:?}");
            told.extend(said.into_iter().map(|text| (minute, text)));
        }
        assert_eq!(told.len(), 1, "{told:?}");
        let (minute, text) = &told[0];
        assert!(*minute <= 66, "told only at minute {minute}");
        assert!(text.starts_with("A process has used a full processor core for over an hour: cat (pid 3063, `cat /dev/zero`, started "), "{text}");
        assert!(text.contains("% of a core on average over the last hour, 100% now. No agent's task owns it."), "{text}");
        let average: f64 = text.split("), ").nth(1).and_then(|rest| rest.split('%').next()).and_then(|n| n.parse().ok()).expect("average");
        assert!((100.0..=140.0).contains(&average), "{text}");
        // Once, busy or not, while it lives; its alert holds meanwhile.
        assert!(at_minute(&mut now, &alive, 71, Some(cpu_ms + 84_000)).is_empty() && at_minute(&mut now, &alive, 72, None).is_empty());
        assert!(at_minute(&mut now, &alive, 200, Some(cpu_ms + 9_000_000)).is_empty());
        assert_eq!(now.alerts.iter().map(|(about, _)| about.as_str()).collect::<Vec<_>>(), ["process:3063:1795000000"]);
        // Gone: its alert goes.
        assert!(at_minute(&mut now, &HashSet::new(), 201, None).is_empty());
        assert!(now.alerts.is_empty() && now.strays.is_empty());
    }

    /// Under a full core's worth over the hour, it's never told, however
    /// long it runs; nor across samples far apart (the machine asleep),
    /// busy as it is in each.
    #[test]
    fn half_a_core_or_a_machine_asleep_is_never_told() {
        let (mut now, alive) = (Now::default(), HashSet::from([CAT]));
        for minute in 0..=240 {
            assert!(at_minute(&mut now, &alive, minute, Some(u64::try_from(minute).unwrap_or(0) * 30_000)).is_empty(), "minute {minute}");
        }
        let mut now = Now::default();
        assert!(at_minute(&mut now, &alive, 0, Some(1_000)).is_empty());
        assert!(at_minute(&mut now, &alive, 90, Some(121_000)).is_empty(), "two busy minutes 90 apart");
        assert!(at_minute(&mut now, &alive, 91, Some(181_000)).is_empty());
    }

    #[test]
    fn a_disk_is_named_as_the_mac_shows_it_never_by_its_path() {
        assert_eq!(disk_name("/"), "This computer's disk");
        assert_eq!(disk_name("/Volumes/Backup"), "The disk \u{201c}Backup\u{201d}");
        assert_eq!(disk_name("/mnt/data"), "The disk \u{201c}data\u{201d}");
    }
}
