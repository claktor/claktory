//! The Resources extension: how the machine and Factory use its processors,
//! memory and disk, for admin agents such as Rivet to look into and tell the
//! person about. Never to act on: the alert says so.
//!
//! A sampler started with Factory reads the machine and its processes every
//! minute (`sample`) and keeps the last hour in memory; what Factory's data
//! and projects take on disk is measured in the background every 20 minutes
//! (`disk`). When memory stays above `resources.ram_percent`, a volume
//! Factory uses runs short of space, or (if set) the processors stay busier
//! than `cpu_percent`, every agent with the extension on hears it once, as a
//! Factory event; it's said again only after the machine recovered.

mod disk;
mod sample;

use std::collections::{HashMap, VecDeque};
use std::fmt::Write as _;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::{Duration, Instant};

use factory_agent::Tool;
use factory_protocol::v1;
use serde::Deserialize;
use serde_json::{Value, json};

use super::{Extension, Manifest, PromptKind, PromptSection, enabled, factory_tool};
use crate::config::{AgentConfig, Config, ResourcesConfig};
use crate::factory::{Factory, Inner, host_event};
use crate::transcript::now_ms;

pub(crate) const ID: &str = "resources";

static MANIFEST: Manifest = Manifest {
    id: ID,
    name: "Resources",
    icon: "gauge",
    description: "Shows how this computer's processor, memory and disk are used, and how much Claktory takes. It hears when memory or disk runs low, and tells you. Rivet has it.",
    default_enabled: false,
    options: &[],
};

const PROMPT: &str = "## Resources

The resources tool shows how this machine and Claktory use its processors, memory and disk: the whole machine, Claktory's own processes (its backend and app, agents' background tasks and monitors' watches), the other programs using the most memory, and what Claktory's data and projects take on disk. Claktory samples it every minute and tells you when memory or disk runs low. Then look into it with the tool and tell the person what you found and what they could do. Never stop, kill or delete anything yourself unless the person asks you to.
";

const ABOUT: &str = "How this machine and Claktory use its resources, in a few lines: processors, memory, swap and free disk; Claktory's processes; the other programs using the most memory; alerts in effect. `detail`: \"processes\" (every process group of Claktory's and the top others), \"disk\" (each volume, and what Claktory's data, agents and projects take) or \"history\" (the last hour, every 5 minutes). `clear_cache`: the path of a build cache (shown under detail \"disk\") to clear now, only when the person asks: it's moved aside and deleted, and the next build there is cold.";

/// Samples kept: an hour's.
const HISTORY: usize = 60;
/// Samples in a row above the line before memory or the processors alert.
const RAM_SAMPLES: u32 = 3;
const CPU_SAMPLES: u32 = 5;
/// The other processes the panel shows.
const OTHERS_SHOWN: usize = 5;
/// How long one disk measurement may walk.
const DISK_BUDGET: Duration = Duration::from_secs(300);

pub(crate) struct Resources;

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
    /// What the agents were told, while it still holds, by what it's about.
    alerts: Vec<(String, String)>,
}

pub(crate) struct State {
    every_ms: AtomicU64,
    disk_every_ms: AtomicU64,
    now: Mutex<Now>,
}

impl Default for State {
    fn default() -> State {
        State { every_ms: AtomicU64::new(60_000), disk_every_ms: AtomicU64::new(20 * 60_000), now: Mutex::default() }
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
}

impl Factory {
    /// How often the machine is sampled (a minute) and the disk measured (20
    /// minutes); tests make them shorter.
    pub fn set_resources_every(&self, sample: Duration, disk: Duration) {
        let ms = |every: Duration| u64::try_from(every.as_millis()).unwrap_or(u64::MAX);
        self.resources.every_ms.store(ms(sample), Ordering::Relaxed);
        self.resources.disk_every_ms.store(ms(disk), Ordering::Relaxed);
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
    jiff::Timestamp::from_millisecond(ms)
        .map(|time| time.to_zoned_datetime(jiff::tz::TimeZone::system()).strftime("%H:%M").to_string())
        .unwrap_or_default()
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
    sample::Known { app, groups, paths }
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

/// Checks a new sample against the lines; returns what to tell, by what
/// it's about. Each is said once per crossing.
fn check(now: &mut Now, lines: &ResourcesConfig, config: &Config) -> Vec<(String, String)> {
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
        said.push(("ram".to_owned(), text));
    }
    if lines.cpu_percent > 0 && busy(&mut now.cpu, machine.cpu_percent > f64::from(lines.cpu_percent), CPU_SAMPLES) {
        let mut busiest: Vec<&v1::ResourceProcess> = now.factory.iter().chain(&now.others).collect();
        busiest.sort_by(|a, b| b.cpu_percent.total_cmp(&a.cpu_percent));
        let busiest: Vec<String> =
            busiest.iter().take(3).map(|process| format!("{} {:.0}%", named(config, process), process.cpu_percent)).collect();
        said.push((
            "cpu".to_owned(),
            format!(
                "This machine's processors have been busy: {:.0}% of {} cores (above {}%) for the last {CPU_SAMPLES} minutes. Busiest: {}. {advice}",
                machine.cpu_percent,
                machine.cores,
                lines.cpu_percent,
                busiest.join(", ")
            ),
        ));
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
        said.push((format!("disk:{}", volume.mount), text));
    }
    // What's no longer over its line is over.
    let holding = |about: &str| match about {
        "ram" => now.ram.told,
        "cpu" => now.cpu.told,
        disk => now.disks.get(disk.trim_start_matches("disk:")).is_some_and(|crossing| crossing.told),
    };
    let mut alerts: Vec<(String, String)> = now.alerts.drain(..).filter(|(about, _)| holding(about)).collect();
    alerts.extend(said.iter().cloned());
    now.alerts = alerts;
    said
}

/// Takes a sample in: history, alerts (told to every agent with the
/// extension on) and the panel.
fn record(factory: &Arc<Factory>, sampled: sample::Sample) {
    let time = now_ms();
    let (lines, config) = {
        let inner = factory.lock();
        (inner.config.resources.clone(), inner.config.clone())
    };
    let said = {
        let mut now = factory.resources.now();
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
        check(&mut now, &lines, &config)
    };
    let told: Vec<String> =
        config.agents.iter().filter(|agent| enabled(agent, ID) && !config.archived(agent)).map(|agent| agent.id.clone()).collect();
    for (_, text) in said {
        for agent in &told {
            let event = host_event(String::new());
            factory.deliver_event(agent, text.clone(), event);
        }
    }
    refresh_panel(factory);
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

    fn prompt(&self, _factory: &Factory, _inner: &Inner, _agent: &AgentConfig) -> Vec<PromptSection> {
        vec![PromptSection { kind: PromptKind::Static, text: PROMPT.to_owned() }]
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
        vec![factory_tool(factory, agent, "resources", "Resources", ABOUT, parameters, run)]
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
