//! What agents' commands may not start (Bruno 36035, Fugo 36039 and 36040):
//! a program that asks the person who they are before acting as someone
//! else: sudo, doas, su, pkexec, authopen, or osascript "with administrator
//! privileges". On a Mac with Touch ID for sudo, sudo puts up a system
//! dialog that a habitual touch satisfies, which would give the agent's
//! command root. Each command's process group is looked at every 100 ms
//! while it runs; any such program is stopped at once (the dialog goes with
//! it), and the agent is told in its command's output.
//!
//! Commands the person runs from a card (Execute) aren't guarded: they
//! started those themselves.

use std::sync::Arc;

use parking_lot::Mutex;
use std::time::Duration;

/// How often a command's process group is looked at.
pub const EVERY: Duration = Duration::from_millis(100);

/// A process in a group.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Process {
    pub pid: i32,
    /// Its name, as the system keeps it (the file it runs, or a name it set).
    pub name: String,
    /// The file it runs, absolute; '' when it can't be told.
    pub path: String,
}

/// The processes in group `pgid`.
pub fn processes(pgid: i32) -> Vec<Process> {
    if pgid <= 1 {
        return Vec::new();
    }
    #[cfg(target_os = "macos")]
    {
        let mut pids = vec![0 as libc::pid_t; 1024];
        let size = i32::try_from(std::mem::size_of_val(pids.as_slice())).unwrap_or(i32::MAX);
        // SAFETY: pids holds `size` bytes.
        let listed = unsafe { libc::proc_listpgrppids(pgid, pids.as_mut_ptr().cast(), size) };
        if listed <= 0 {
            return Vec::new();
        }
        pids.iter()
            .filter(|pid| **pid > 0)
            .filter_map(|pid| {
                // Its name can't be read once it runs as root (sudo, su),
                // but the file it runs can.
                let mut name = [0_u8; 256];
                // SAFETY: name holds 256 bytes.
                let length = unsafe { libc::proc_name(*pid, name.as_mut_ptr().cast(), 256) };
                let name = usize::try_from(length).map_or_else(|_| String::new(), |length| String::from_utf8_lossy(&name[..length.min(name.len())]).into_owned());
                let mut path = vec![0_u8; libc::PROC_PIDPATHINFO_MAXSIZE as usize];
                // SAFETY: path holds PROC_PIDPATHINFO_MAXSIZE bytes.
                let length = unsafe { libc::proc_pidpath(*pid, path.as_mut_ptr().cast(), libc::PROC_PIDPATHINFO_MAXSIZE as u32) };
                let path = usize::try_from(length).map_or_else(|_| String::new(), |length| String::from_utf8_lossy(&path[..length.min(path.len())]).into_owned());
                (!name.is_empty() || !path.is_empty()).then_some(Process { pid: *pid, name, path })
            })
            .collect()
    }
    #[cfg(not(target_os = "macos"))]
    {
        let Ok(entries) = std::fs::read_dir("/proc") else { return Vec::new() };
        entries
            .flatten()
            .filter_map(|entry| {
                let pid: i32 = entry.file_name().to_str()?.parse().ok()?;
                let stat = std::fs::read_to_string(entry.path().join("stat")).ok()?;
                // "pid (name) state ppid pgrp …"
                let (head, rest) = stat.rsplit_once(')')?;
                let name = head.split_once('(')?.1.to_owned();
                let group: i32 = rest.split_whitespace().nth(2)?.parse().ok()?;
                let path = std::fs::read_link(entry.path().join("exe")).map(|path| path.display().to_string()).unwrap_or_default();
                (group == pgid).then_some(Process { pid, name, path })
            })
            .collect()
    }
}

/// Process `pid`'s arguments, its own name first, joined with spaces; None
/// when they can't be read.
fn arguments(pid: i32) -> Option<String> {
    #[cfg(target_os = "macos")]
    {
        let mut mib = [libc::CTL_KERN, libc::KERN_PROCARGS2, pid];
        let mut size: libc::size_t = 0;
        // SAFETY: a null buffer asks only for the size.
        if unsafe { libc::sysctl(mib.as_mut_ptr(), 3, std::ptr::null_mut(), &raw mut size, std::ptr::null_mut(), 0) } != 0 || size < 4 {
            return None;
        }
        let mut buffer = vec![0_u8; size];
        // SAFETY: buffer holds `size` bytes.
        if unsafe { libc::sysctl(mib.as_mut_ptr(), 3, buffer.as_mut_ptr().cast(), &raw mut size, std::ptr::null_mut(), 0) } != 0 {
            return None;
        }
        buffer.truncate(size);
        // argc, the file it runs, padding, then argc arguments.
        let count = usize::try_from(i32::from_ne_bytes(buffer.get(..4)?.try_into().ok()?)).ok()?;
        let rest = &buffer[4..];
        let after_path = rest.iter().position(|byte| *byte == 0)?;
        let start = rest[after_path..].iter().position(|byte| *byte != 0).map(|at| after_path + at)?;
        let words: Vec<String> = rest[start..].split(|byte| *byte == 0).take(count).map(|word| String::from_utf8_lossy(word).into_owned()).collect();
        Some(words.join(" "))
    }
    #[cfg(not(target_os = "macos"))]
    {
        let line = std::fs::read(format!("/proc/{pid}/cmdline")).ok()?;
        Some(line.split(|byte| *byte == 0).filter(|word| !word.is_empty()).map(|word| String::from_utf8_lossy(word).into_owned()).collect::<Vec<_>>().join(" "))
    }
}

/// The program `process` is, by the name the agent is told, when it asks
/// who you are.
pub fn asking(process: &Process) -> Option<&'static str> {
    let file = process.path.rsplit('/').next().unwrap_or_default();
    let is = |name: &str| process.name == name || file == name;
    if is("sudo") {
        return Some("sudo");
    }
    for name in ["doas", "su", "pkexec"] {
        if is(name) {
            return Some(name);
        }
    }
    if is("authopen") || process.path == "/usr/libexec/authopen" {
        return Some("authopen");
    }
    if is("osascript") && arguments(process.pid).is_some_and(|arguments| arguments.to_lowercase().contains("administrator privileges")) {
        return Some("osascript with administrator privileges");
    }
    None
}

/// What the agent is told when `program` was stopped.
pub fn told(program: &str) -> String {
    format!("{program} isn't for agents: it was stopped. Ask the person to run it themselves.")
}

/// Watches one command's process group until it's gone, stopping what asks
/// who you are.
#[derive(Clone, Default)]
pub struct Guard {
    /// What it stopped, each once.
    stopped: Arc<Mutex<Vec<&'static str>>>,
}

impl Guard {
    /// Starts watching group `pgid`, of `agent`'s command. `log`: the
    /// command's log, which hears what was stopped (a handle sharing the
    /// command's own offset, so nothing is written over); without one, the
    /// caller adds `said` to the output itself.
    pub fn watch(pgid: u32, agent: &str, log: Option<std::fs::File>) -> Guard {
        let guard = Guard::default();
        let stopped = guard.stopped.clone();
        let agent = agent.to_owned();
        let Ok(group) = i32::try_from(pgid) else { return guard };
        tokio::spawn(async move {
            let mut log = log;
            let mut ticks = tokio::time::interval(EVERY);
            loop {
                ticks.tick().await;
                if !super::bash::group_alive(pgid) {
                    break;
                }
                for process in processes(group) {
                    let Some(program) = asking(&process) else { continue };
                    // SAFETY: a plain signal to one process of the command's group.
                    unsafe { libc::kill(process.pid, libc::SIGKILL) };
                    let first = {
                        let mut stopped = stopped.lock();
                        let first = !stopped.contains(&program);
                        if first {
                            stopped.push(program);
                        }
                        first
                    };
                    if first {
                        tracing::warn!(%agent, program, "stopped a program that asks who you are in an agent's command");
                        let failed = log.as_mut().is_some_and(|file| {
                            use std::io::Write;
                            file.write_all(format!("\n{}\n", told(program)).as_bytes()).is_err()
                        });
                        if failed {
                            log = None;
                        }
                    }
                }
            }
        });
        guard
    }

    /// What the agent is told of what was stopped: a line each, or ''.
    pub fn said(&self) -> String {
        let stopped = self.stopped.lock();
        stopped.iter().map(|program| told(program)).collect::<Vec<_>>().join("\n")
    }
}
