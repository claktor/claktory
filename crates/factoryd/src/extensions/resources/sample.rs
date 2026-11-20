//! One sample of the machine and its processes, with Factory's told apart:
//! its backend, supervisor and app, and what agents started (background
//! tasks and monitors' watches by their process groups, anything else under
//! the backend as commands running now).

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use factory_protocol::v1;
use sysinfo::{ProcessRefreshKind, ProcessesToUpdate, System, UpdateKind};

/// How many other processes a sample keeps.
pub(crate) const OTHERS: usize = 10;

/// A process group of Factory's to tell apart: a task or a monitor's watch.
pub(crate) struct Group {
    pub pgid: u32,
    pub label: String,
    pub kind: &'static str,
    pub agent: String,
}

/// What Factory knows of its own when it samples.
pub(crate) struct Known {
    /// The app's bundle name, e.g. "Factory.app".
    pub app: &'static str,
    pub groups: Vec<Group>,
    /// Paths Factory uses, each with what it is: "Factory's data", a project's name.
    pub paths: Vec<(PathBuf, String)>,
}

pub(crate) struct Sample {
    pub machine: v1::Machine,
    /// Biggest first.
    pub factory: Vec<v1::ResourceProcess>,
    /// The top `OTHERS` by memory.
    pub others: Vec<v1::ResourceProcess>,
}

/// Keeps what CPU use is measured against between samples.
pub(crate) struct Sampler {
    system: System,
}

/// The volume `path` is on: where it's mounted, its size and what's free
/// for this user, in bytes.
#[cfg(target_os = "macos")]
fn volume_of(path: &Path) -> Option<(String, u64, u64)> {
    use std::os::unix::ffi::OsStrExt;
    let path = std::ffi::CString::new(path.as_os_str().as_bytes()).ok()?;
    // SAFETY: statfs fills the zeroed struct it's given, for a valid C string.
    let mut stats: libc::statfs = unsafe { std::mem::zeroed() };
    if unsafe { libc::statfs(path.as_ptr(), &raw mut stats) } != 0 {
        return None;
    }
    // SAFETY: f_mntonname is a NUL-terminated string.
    let mount = unsafe { std::ffi::CStr::from_ptr(stats.f_mntonname.as_ptr()) }.to_string_lossy().into_owned();
    // The startup disk's data half shares its space with "/".
    let mount = if mount == "/System/Volumes/Data" { "/".to_owned() } else { mount };
    let block = u64::from(stats.f_bsize);
    Some((mount, stats.f_blocks * block, stats.f_bavail * block))
}

#[cfg(not(target_os = "macos"))]
fn volume_of(path: &Path) -> Option<(String, u64, u64)> {
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::fs::MetadataExt;
    let c_path = std::ffi::CString::new(path.as_os_str().as_bytes()).ok()?;
    // SAFETY: statvfs fills the zeroed struct it's given, for a valid C string.
    let mut stats: libc::statvfs = unsafe { std::mem::zeroed() };
    if unsafe { libc::statvfs(c_path.as_ptr(), &raw mut stats) } != 0 {
        return None;
    }
    // Where it's mounted: the last folder up on the same device.
    let device = std::fs::metadata(path).ok()?.dev();
    let mut mount = path.to_owned();
    while let Some(parent) = mount.parent() {
        if std::fs::metadata(parent).map(|metadata| metadata.dev()).ok() != Some(device) {
            break;
        }
        mount = parent.to_owned();
    }
    #[allow(clippy::useless_conversion)] // u32 or u64 depending on the platform
    let block = u64::from(stats.f_frsize);
    #[allow(clippy::useless_conversion)]
    let (blocks, available) = (u64::from(stats.f_blocks), u64::from(stats.f_bavail));
    Some((mount.display().to_string(), blocks * block, available * block))
}

fn process_group(pid: u32) -> Option<u32> {
    let pid = libc::pid_t::try_from(pid).ok()?;
    // SAFETY: getpgid only reads the process table.
    let group = unsafe { libc::getpgid(pid) };
    u32::try_from(group).ok()
}

/// The outermost app bundle a program is in ("Google Chrome" for
/// ".../Google Chrome.app/.../Google Chrome Helper"), so an app's helpers add up.
fn bundle(exe: &Path) -> Option<String> {
    exe.components().filter_map(|part| part.as_os_str().to_str()).find_map(|part| part.strip_suffix(".app")).map(str::to_owned)
}

fn add(entry: &mut v1::ResourceProcess, process: &sysinfo::Process) {
    entry.processes += 1;
    entry.cpu_percent += f64::from(process.cpu_usage());
    entry.memory += process.memory();
}

fn entry(label: impl Into<String>, kind: &str, agent: &str, pid: u32) -> v1::ResourceProcess {
    v1::ResourceProcess { label: label.into(), kind: kind.to_owned(), agent_id: agent.to_owned(), pid, ..Default::default() }
}

impl Sampler {
    /// Err when this system can't be read.
    pub(crate) fn new() -> Result<Sampler, String> {
        let system = System::new().map_err(|error| error.to_string())?;
        let mut sampler = Sampler { system };
        // CPU use is measured from one refresh to the next.
        sampler.refresh();
        Ok(sampler)
    }

    fn refresh(&mut self) {
        self.system.refresh_memory();
        self.system.refresh_cpu_usage();
        let kind = ProcessRefreshKind::nothing().with_cpu().with_memory().with_exe(UpdateKind::OnlyIfNotSet);
        self.system.refresh_processes_specifics(ProcessesToUpdate::All, true, kind);
    }

    pub(crate) fn sample(&mut self, known: &Known) -> Sample {
        self.refresh();
        let me = std::process::id();
        let supervisor = std::env::var_os(crate::update::SUPERVISED).and_then(|_| {
            // SAFETY: getppid can't fail.
            u32::try_from(unsafe { libc::getppid() }).ok()
        });
        let processes = self.system.processes();
        let parents: HashMap<u32, u32> =
            processes.iter().filter_map(|(pid, process)| Some((pid.as_u32(), process.parent()?.as_u32()))).collect();
        let under_me = |pid: u32| {
            let mut at = pid;
            for _ in 0..64 {
                match parents.get(&at) {
                    Some(&parent) if parent == me => return true,
                    Some(&parent) if parent > 1 => at = parent,
                    _ => return false,
                }
            }
            false
        };
        let app_dir = format!("/{}/Contents/MacOS/", known.app);
        let mut backend = entry("Claktory service", "backend", "", me);
        let mut supervising = supervisor.map(|pid| entry("Supervisor", "supervisor", "", pid));
        let mut app: Option<v1::ResourceProcess> = None;
        let mut groups: Vec<v1::ResourceProcess> =
            known.groups.iter().map(|group| entry(group.label.clone(), group.kind, &group.agent, group.pgid)).collect();
        let mut commands: Option<v1::ResourceProcess> = None;
        let mut others: HashMap<String, v1::ResourceProcess> = HashMap::new();
        for (pid, process) in processes {
            let pid = pid.as_u32();
            if pid == me {
                add(&mut backend, process);
                continue;
            }
            if let Some(supervising) = supervising.as_mut().filter(|supervising| supervising.pid == pid) {
                add(supervising, process);
                continue;
            }
            let exe = process.exe();
            if exe.is_some_and(|exe| exe.to_string_lossy().contains(&app_dir)) {
                add(app.get_or_insert_with(|| entry("App", "app", "", pid)), process);
                continue;
            }
            if let Some(index) = process_group(pid).and_then(|pgid| known.groups.iter().position(|group| group.pgid == pgid)) {
                add(&mut groups[index], process);
                continue;
            }
            if under_me(pid) {
                add(commands.get_or_insert_with(|| entry("Commands running now", "command", "", pid)), process);
                continue;
            }
            let name = exe.and_then(bundle).unwrap_or_else(|| process.name().to_string_lossy().into_owned());
            add(others.entry(name.clone()).or_insert_with(|| entry(name, "other", "", pid)), process);
        }
        let mut factory: Vec<v1::ResourceProcess> = std::iter::once(backend)
            .chain(supervising)
            .chain(app)
            .chain(groups.into_iter().filter(|group| group.processes > 0))
            .chain(commands)
            .collect();
        factory.sort_by_key(|process| std::cmp::Reverse(process.memory));
        let mut others: Vec<v1::ResourceProcess> = others.into_values().collect();
        others.sort_by_key(|process| std::cmp::Reverse(process.memory));
        others.truncate(OTHERS);
        let machine = v1::Machine {
            cpu_percent: f64::from(self.system.global_cpu_usage()),
            cores: u32::try_from(self.system.cpus().len()).unwrap_or(u32::MAX),
            memory_total: self.system.total_memory(),
            memory_used: self.system.used_memory(),
            swap_total: self.system.total_swap(),
            swap_used: self.system.used_swap(),
            volumes: self.volumes(&known.paths),
        };
        Sample { machine, factory, others }
    }

    /// The volumes `paths` are on, each with what of Factory's it holds.
    fn volumes(&self, paths: &[(PathBuf, String)]) -> Vec<v1::Volume> {
        let mut volumes: Vec<v1::Volume> = Vec::new();
        for (path, what) in paths {
            let Some((mount, total, available)) = volume_of(path) else { continue };
            let volume = match volumes.iter_mut().find(|volume| volume.mount == mount) {
                Some(volume) => volume,
                None => {
                    volumes.push(v1::Volume { mount, total, available, uses: Vec::new() });
                    volumes.last_mut().expect("just pushed")
                }
            };
            if !volume.uses.contains(what) {
                volume.uses.push(what.clone());
            }
        }
        volumes
    }
}
