//! What agents' commands may not start (Bruno 36035, Fugo 36039 and 36040):
//! a program that asks the person who they are before acting as someone
//! else: sudo, doas, su, pkexec, run0 (systemd's, through polkit), authopen,
//! or osascript asking for administrator privileges, in its arguments or in
//! the script file it runs (AppleScript's words, JXA's
//! `administratorPrivileges`, compiled AppleScript's `badm`). And Apple's
//! `container` when its arguments name Claktory's browser (a
//! `claktory-browser…` VM or volume), which would reach the VM
//! and its kept profile, cookies and all. On a Mac with Touch ID for sudo, sudo puts up a system
//! dialog that a habitual touch satisfies, which would give the agent's
//! command root (polkit's dialog on Linux is the same risk). Any such
//! program in a guarded command is stopped within about 100 ms (the dialog
//! goes with it), and the agent is told in its command's output.
//!
//! A command's processes are its process group, every descendant of its
//! first process (by parent, whatever their group or session), and every
//! process already seen in it, known by pid and start time, so one that
//! moves to a session of its own (`setsid sudo`) stays watched. How they're
//! found (Polpo's sudo-guard.md, Fugo 46005):
//! - On Linux, one thread walks /proc once every 100 ms for every guarded
//!   command together, reading each process's stat, and the file it runs
//!   only for a command's own.
//! - On a Mac, one thread is told by a kqueue (EVFILT_PROC: fork, exec and
//!   exit) of each watched process. A new process is registered first and
//!   checked after, so one that ran sudo before it was registered is caught
//!   by the check and one that runs it after by its exec. Each command's
//!   group is listed again every 500 ms in case a registration was missed;
//!   between events an idle command costs nothing.
//! - On Windows, a command is a job, which holds every process it starts;
//!   one thread looks at each guarded command's job every 100 ms, and what
//!   it stops is sudo, gsudo and runas (crate::windows::guard).
//!
//! Accepted (Fugo 46319): a double fork whose middle process exits within
//! one look (on Linux, between two walks 100 ms apart; on a Mac, before its
//! fork is handled) leaves its child outside, in a session of its own, and
//! can escape. And osascript is judged by what its arguments and its
//! script file say (Fugo 49010): a script that builds the words as it runs
//! (`run script`, joined strings) or reads them from elsewhere gets through,
//! and a script piped to it on stdin (`osascript <<EOF`, `osascript -`) is
//! never seen at all: the easiest way past it. Under containment osascript
//! can't run at all, so this gap is only for agents that aren't contained.
//! `container` is known by its name or its file's, and stopped as it
//! starts, a few milliseconds after its exec: a copy of it under another
//! name, a command quicker than that, or talking to its service directly
//! would get through, which only a sandbox that keeps agents from the
//! service closes: on a Mac with Claktory's browser set up, agents'
//! commands run under one (`confine`), which denies them the service and
//! its folder whatever runs them. On Linux, what a stopped program started as another user (sudo
//! with nothing to ask, as a NOPASSWD line allows) can't be stopped by this
//! user: its descendants are stopped only while they're still this user's.
//! Programs that run as another user but ask nothing (fusermount,
//! newuidmap) are left alone on purpose. Nothing that can't be traced to a
//! guarded command is ever stopped: the person's own commands, run from a
//! card (Execute), aren't guarded, since they started those themselves.

use std::sync::Arc;

use parking_lot::Mutex;
use std::time::Duration;

/// How often Linux looks at the guarded commands' processes.
pub const EVERY: Duration = Duration::from_millis(100);

/// How often a Mac lists each guarded command's group again, in case an
/// event was missed.
pub const BACKSTOP: Duration = Duration::from_millis(500);

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
        mac::group(pgid).into_iter().filter_map(mac::process).collect()
    }
    // The command's job, named by its shell's pid (crate::windows::job).
    #[cfg(windows)]
    {
        windows::processes(pgid)
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        linux::walk().into_iter().filter(|stat| stat.pgrp == pgid).map(|stat| linux::process(&stat)).collect()
    }
}

/// Process `pid`'s arguments, its own name first; None when they can't be
/// read.
#[cfg(unix)]
fn arguments(pid: i32) -> Option<Vec<String>> {
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
        Some(rest[start..].split(|byte| *byte == 0).take(count).map(|word| String::from_utf8_lossy(word).into_owned()).collect())
    }
    #[cfg(not(target_os = "macos"))]
    {
        let line = std::fs::read(format!("/proc/{pid}/cmdline")).ok()?;
        Some(line.split(|byte| *byte == 0).filter(|word| !word.is_empty()).map(|word| String::from_utf8_lossy(word).into_owned()).collect())
    }
}

/// Process `pid`'s working folder; None when it can't be read.
#[cfg(unix)]
fn working_folder(pid: i32) -> Option<std::path::PathBuf> {
    #[cfg(target_os = "macos")]
    {
        // SAFETY: plain data, filled in by proc_pidinfo.
        let mut info: libc::proc_vnodepathinfo = unsafe { std::mem::zeroed() };
        let size = i32::try_from(std::mem::size_of::<libc::proc_vnodepathinfo>()).ok()?;
        // SAFETY: info is `size` bytes.
        let read = unsafe { libc::proc_pidinfo(pid, libc::PROC_PIDVNODEPATHINFO, 0, (&raw mut info).cast(), size) };
        if read != size {
            return None;
        }
        // SAFETY: vip_path is MAXPATHLEN bytes, a C string.
        let path = unsafe { std::ffi::CStr::from_ptr(info.pvi_cdir.vip_path.as_ptr().cast()) };
        use std::os::unix::ffi::OsStrExt;
        Some(std::path::PathBuf::from(std::ffi::OsStr::from_bytes(path.to_bytes())))
    }
    #[cfg(not(target_os = "macos"))]
    {
        std::fs::read_link(format!("/proc/{pid}/cwd")).ok()
    }
}

/// How much of a script file osascript runs is read for what it asks.
#[cfg(unix)]
const SCRIPT_MOST: u64 = 64 * 1024;

/// What a script asks for administrator privileges with, in osascript's
/// arguments or its script, lowercased: AppleScript's words and JXA's
/// option.
#[cfg(unix)]
const ADMINISTRATOR: [&str; 2] = ["administrator privileges", "administratorprivileges"];

/// The script file osascript runs, from its arguments: the first that isn't
/// an option or an option's value, unless it runs statements (-e) instead.
#[cfg(unix)]
fn osascript_file(arguments: &[String]) -> Option<&str> {
    let mut words = arguments.iter().skip(1);
    while let Some(word) = words.next() {
        match word.as_str() {
            "--" => return words.next().map(String::as_str),
            "-e" => return None,
            "-l" | "-s" => {
                words.next();
            }
            "-" => return None,
            option if option.starts_with('-') => {
                if option.starts_with("-e") {
                    return None;
                }
            }
            file => return Some(file),
        }
    }
    None
}

/// Whether the script file osascript at `pid` runs (`file`, as its
/// arguments name it) asks for administrator privileges: its first 64 KB,
/// as text (AppleScript or JXA, and compiled JXA, which keeps its source)
/// or compiled AppleScript, where `with administrator privileges` is the
/// parameter `badm`. A bundle's script is its main.scpt.
#[cfg(unix)]
fn script_asks(pid: i32, file: &str) -> bool {
    use std::io::Read;
    let mut path = std::path::PathBuf::from(file);
    if path.is_relative() {
        let Some(folder) = working_folder(pid) else { return false };
        path = folder.join(path);
    }
    if path.is_dir() {
        path = path.join("Contents/Resources/Scripts/main.scpt");
    }
    let mut bytes = Vec::new();
    let Ok(opened) = std::fs::File::open(&path) else { return false };
    if opened.take(SCRIPT_MOST).read_to_end(&mut bytes).is_err() {
        return false;
    }
    if bytes.starts_with(b"FasdUAS") && bytes.windows(4).any(|window| window == b"badm") {
        return true;
    }
    let text = String::from_utf8_lossy(&bytes).to_lowercase();
    ADMINISTRATOR.iter().any(|words| text.contains(words))
}

/// The program `process` is, by the name the agent is told, when it asks
/// who you are.
pub fn asking(process: &Process) -> Option<&'static str> {
    #[cfg(windows)]
    {
        crate::windows::guard::asking(&process.path)
    }
    #[cfg(unix)]
    {
        asking_unix(process)
    }
}

#[cfg(unix)]
fn asking_unix(process: &Process) -> Option<&'static str> {
    let is = |name: &str| known_as(process, name);
    if is("sudo") {
        return Some("sudo");
    }
    // run0 is systemd's sudo, which asks through polkit.
    for name in ["doas", "su", "pkexec", "run0"] {
        if is(name) {
            return Some(name);
        }
    }
    if is("authopen") || process.path == "/usr/libexec/authopen" {
        return Some("authopen");
    }
    if is("osascript") && osascript_asks(process.pid) {
        return Some("osascript with administrator privileges");
    }
    if is("container") && arguments(process.pid).is_some_and(|arguments| names_the_browser(&arguments)) {
        return Some("container on Claktory's browser");
    }
    None
}

/// Whether `process` is the program `name`, by its name or its file's:
/// once it runs as root its name can't be read, but the file it runs can.
/// In debug builds with tests' stand-ins on (`stand_ins_for_test`),
/// "asks-like-sudo" is sudo too.
pub fn known_as(process: &Process, name: &str) -> bool {
    let file = process.path.rsplit('/').next().unwrap_or_default();
    if process.name == name || file == name {
        return true;
    }
    cfg!(debug_assertions)
        && STAND_INS.get().is_some()
        && [file, process.name.as_str()].iter().any(|named| named.strip_prefix(STAND_IN) == Some(name))
}

/// Whether `container`'s arguments (its own name first) name Claktory's
/// browser: a VM or volume whose name starts `claktory-browser`, in any
/// subcommand and option (`exec`, `-v claktory-browser-x:/p`, `--mount
/// source=…`, `volume rm`), whatever the case.
#[cfg(unix)]
fn names_the_browser(arguments: &[String]) -> bool {
    arguments.iter().skip(1).any(|word| word.to_lowercase().contains(BROWSER))
}

/// What Claktory's browser's VMs and volumes are named with.
#[cfg(unix)]
const BROWSER: &str = "claktory-browser";

/// Whether osascript at `pid` asks for administrator privileges, in its
/// arguments (`-e` statements) or in the script file it runs.
#[cfg(unix)]
fn osascript_asks(pid: i32) -> bool {
    let Some(arguments) = arguments(pid) else { return false };
    let joined = arguments.join(" ").to_lowercase();
    ADMINISTRATOR.iter().any(|words| joined.contains(words)) || osascript_file(&arguments).is_some_and(|file| script_asks(pid, file))
}

/// What tests' stand-ins are named with (`stand_ins_for_test`).
pub const STAND_IN: &str = "asks-like-";

/// Tests' stand-ins are on (`stand_ins_for_test`).
static STAND_INS: std::sync::OnceLock<()> = std::sync::OnceLock::new();

/// For tests: a program named `asks-like-<name>` is judged as `<name>`
/// ("asks-like-sudo" is stopped as sudo), in debug builds only. An agent
/// runs the tests, and Claktory's own guard stops sudo and the rest in its
/// commands at once, before the guard under test can see them or where they
/// should be left alone, so the tests run stand-ins under names only the
/// guard under test knows. It adds what's stopped, never takes anything
/// away.
#[doc(hidden)]
pub fn stand_ins_for_test() {
    let _ = STAND_INS.set(());
}

/// What the agent is told when `program` was stopped.
pub fn told(program: &str) -> String {
    format!("{program} isn't for agents: it was stopped. Ask the person to run it themselves.")
}

/// One guarded command, as the guard's thread keeps it.
struct Command {
    /// Its first process, the leader of its group.
    root: i32,
    agent: String,
    /// Its log, which hears what was stopped; None without one, or once
    /// writing to it failed.
    log: Option<std::fs::File>,
    stopped: Arc<Mutex<Vec<&'static str>>>,
}

impl Command {
    /// Stops `process`, which is `program`, and says so the first time.
    fn stop(&mut self, process: &Process, program: &'static str) {
        #[cfg(windows)]
        crate::windows::system::terminate_process(process.pid as u32);
        // SAFETY: a plain signal to one process of the command's.
        #[cfg(unix)]
        unsafe {
            libc::kill(process.pid, libc::SIGKILL)
        };
        let first = {
            let mut stopped = self.stopped.lock();
            let first = !stopped.contains(&program);
            if first {
                stopped.push(program);
            }
            first
        };
        if first {
            tracing::warn!(agent = %self.agent, program, "stopped a program that asks who you are in an agent's command");
            let failed = self.log.as_mut().is_some_and(|file| {
                use std::io::Write;
                file.write_all(format!("\n{}\n", told(program)).as_bytes()).is_err()
            });
            if failed {
                self.log = None;
            }
        }
    }

    /// Stops `process` if it asks who you are; whether it did.
    fn check(&mut self, process: &Process) -> bool {
        let Some(program) = asking(process) else { return false };
        self.stop(process, program);
        true
    }
}

/// Watches one command's processes until they're all gone, stopping what
/// asks who you are.
#[derive(Clone, Default)]
pub struct Guard {
    /// What it stopped, each once.
    stopped: Arc<Mutex<Vec<&'static str>>>,
}

impl Guard {
    /// Starts watching the command whose first process (and group) is
    /// `pgid`, `agent`'s. `log`: the command's log, which hears what was
    /// stopped (a handle sharing the command's own offset, so nothing is
    /// written over); without one, the caller adds `said` to the output
    /// itself.
    pub fn watch(pgid: u32, agent: &str, log: Option<std::fs::File>) -> Guard {
        let guard = Guard::default();
        let Ok(root) = i32::try_from(pgid) else { return guard };
        if root <= 1 {
            return guard;
        }
        let command = Command { root, agent: agent.to_owned(), log, stopped: guard.stopped.clone() };
        #[cfg(target_os = "macos")]
        mac::watch(command);
        #[cfg(windows)]
        windows::watch(command);
        #[cfg(all(unix, not(target_os = "macos")))]
        linux::watch(command);
        guard
    }

    /// What the agent is told of what was stopped: a line each, or ''.
    pub fn said(&self) -> String {
        let stopped = self.stopped.lock();
        stopped.iter().map(|program| told(program)).collect::<Vec<_>>().join("\n")
    }
}

/// Windows: a command is a job (crate::windows::job), which holds every
/// process it starts, so one thread looks at each guarded command's job
/// every 100 ms, as Linux walks /proc, until the job is empty.
#[cfg(windows)]
mod windows {
    use super::{Command, EVERY, Process};
    use parking_lot::Mutex;

    /// The processes of the job of the command whose shell is `root`.
    pub(super) fn processes(root: i32) -> Vec<Process> {
        let Ok(root) = u32::try_from(root) else { return Vec::new() };
        crate::windows::job::processes(root)
            .into_iter()
            .map(|(pid, path)| {
                let name = path.rsplit('\\').next().unwrap_or_default().to_owned();
                Process { pid: pid as i32, name, path }
            })
            .collect()
    }

    struct State {
        watched: Vec<Command>,
        /// The thread looking at them is running.
        running: bool,
    }

    static STATE: Mutex<State> = Mutex::new(State { watched: Vec::new(), running: false });

    pub(super) fn watch(mut command: Command) {
        // A first look at once, so what the shell started first is caught
        // without waiting for the thread.
        for process in processes(command.root) {
            command.check(&process);
        }
        let mut state = STATE.lock();
        state.watched.push(command);
        if !state.running {
            state.running = true;
            let spawned = std::thread::Builder::new().name("asking-guard".into()).spawn(run);
            if let Err(error) = spawned {
                tracing::warn!(%error, "couldn't start the guard against programs that ask who you are");
                state.running = false;
            }
        }
    }

    /// Looks every 100 ms while a command is guarded, then ends.
    fn run() {
        loop {
            std::thread::sleep(EVERY);
            let mut state = STATE.lock();
            // A command whose job is empty is over: it's dropped.
            state.watched.retain_mut(|command| {
                let processes = processes(command.root);
                for process in &processes {
                    command.check(process);
                }
                !processes.is_empty()
            });
            if state.watched.is_empty() {
                state.running = false;
                return;
            }
        }
    }
}

#[cfg(all(unix, not(target_os = "macos")))]
mod linux {
    use super::{Command, EVERY, Process};
    use hashbrown::{HashMap, HashSet};
    use parking_lot::Mutex;

    /// A process as its stat says.
    pub(super) struct Stat {
        pub pid: i32,
        pub name: String,
        pub ppid: i32,
        pub pgrp: i32,
        /// When it started, in clock ticks since boot: with the pid, which
        /// process it is.
        pub start: u64,
    }

    /// Every process on the machine, from one walk of /proc.
    pub(super) fn walk() -> Vec<Stat> {
        let Ok(entries) = std::fs::read_dir("/proc") else { return Vec::new() };
        entries.flatten().filter_map(|entry| stat(entry.file_name().to_str()?.parse().ok()?)).collect()
    }

    /// Process `pid`'s stat; None once it's gone.
    fn stat(pid: i32) -> Option<Stat> {
        let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
        // "pid (name) state ppid pgrp session tty tpgid flags minflt
        // cminflt majflt cmajflt utime stime cutime cstime priority nice
        // threads itrealvalue starttime …"
        let (head, rest) = stat.rsplit_once(')')?;
        let name = head.split_once('(')?.1.to_owned();
        let fields: Vec<&str> = rest.split_whitespace().collect();
        Some(Stat {
            pid,
            name,
            ppid: fields.get(1)?.parse().ok()?,
            pgrp: fields.get(2)?.parse().ok()?,
            start: fields.get(19)?.parse().ok()?,
        })
    }

    /// Stops every descendant of `pid` (in `table`, by parent): what a
    /// stopped program started. Only those still this user's can be: one
    /// running as another user refuses the signal.
    fn stop_descendants(table: &[Stat], children: &HashMap<i32, Vec<usize>>, pid: i32) {
        let mut next = vec![pid];
        while let Some(parent) = next.pop() {
            for &child in children.get(&parent).map(Vec::as_slice).unwrap_or_default() {
                kill_if_still(&table[child]);
                next.push(table[child].pid);
            }
        }
    }

    /// Kills the process `seen` is, only while its pid is still that
    /// process: one that ended since the walk may have left
    /// its pid to the person's own work. A pidfd holds the process itself,
    /// so the start time read after opening it is the one signalled; on
    /// kernels without pidfds (before 5.3), the start time is read just
    /// before the signal.
    fn kill_if_still(seen: &Stat) {
        let still = || stat(seen.pid).is_some_and(|now| now.start == seen.start);
        // SAFETY: pidfd_open takes a pid and flags, and returns a new fd or -1.
        let pidfd = unsafe { libc::syscall(libc::SYS_pidfd_open, seen.pid, 0) };
        match i32::try_from(pidfd) {
            Ok(fd) if fd >= 0 => {
                if still() {
                    // SAFETY: a signal through our own pidfd, with no info.
                    unsafe { libc::syscall(libc::SYS_pidfd_send_signal, fd, libc::SIGKILL, std::ptr::null::<libc::siginfo_t>(), 0) };
                }
                // SAFETY: closes the fd opened above, once.
                unsafe { libc::close(fd) };
            }
            _ => {
                if still() {
                    // SAFETY: a plain signal to one process.
                    unsafe { libc::kill(seen.pid, libc::SIGKILL) };
                }
            }
        }
    }

    /// The process `stat` is, with the file it runs.
    pub(super) fn process(stat: &Stat) -> Process {
        let path = std::fs::read_link(format!("/proc/{}/exe", stat.pid)).map(|path| path.display().to_string()).unwrap_or_default();
        Process { pid: stat.pid, name: stat.name.clone(), path }
    }

    struct Watched {
        command: Command,
        /// Its processes at the last walk, by pid and start time.
        seen: HashSet<(i32, u64)>,
        /// Not looked at yet: its first process may have gone before its
        /// stat was read, and its group is all there is to go by.
        first: bool,
    }

    struct State {
        watched: Vec<Watched>,
        /// The thread walking for them is running.
        running: bool,
    }

    static STATE: Mutex<State> = Mutex::new(State { watched: Vec::new(), running: false });

    pub(super) fn watch(command: Command) {
        // Its first process by pid and start time, so a pid used again
        // later is never taken for it.
        let root = command.root;
        let seen: HashSet<(i32, u64)> = stat(root).map(|stat| (stat.pid, stat.start)).into_iter().collect();
        let mut state = STATE.lock();
        state.watched.push(Watched { command, seen, first: true });
        if !state.running {
            state.running = true;
            let spawned = std::thread::Builder::new().name("asking-guard".into()).spawn(run);
            if let Err(error) = spawned {
                tracing::warn!(%error, "couldn't start the guard against programs that ask who you are");
                state.running = false;
            }
        }
    }

    /// Walks every 100 ms while a command is guarded, then ends.
    fn run() {
        loop {
            std::thread::sleep(EVERY);
            if !tick() {
                return;
            }
        }
    }

    /// One look at every guarded command; false once there are none, and
    /// the thread ends.
    fn tick() -> bool {
        let mut state = STATE.lock();
        if state.watched.is_empty() {
            state.running = false;
            return false;
        }
        let table = walk();
        let mut children: HashMap<i32, Vec<usize>> = HashMap::new();
        for (at, stat) in table.iter().enumerate() {
            children.entry(stat.ppid).or_default().push(at);
        }
        for watched in &mut state.watched {
            let root = watched.command.root;
            // None of its processes left: it's over, and its group's id may
            // be another group's by now (Fugo 46798), so it isn't matched.
            if !watched.first && !table.iter().any(|stat| watched.seen.contains(&(stat.pid, stat.start))) {
                watched.seen.clear();
                continue;
            }
            watched.first = false;
            let mut members: Vec<usize> = table
                .iter()
                .enumerate()
                .filter(|(_, stat)| stat.pgrp == root || watched.seen.contains(&(stat.pid, stat.start)))
                .map(|(at, _)| at)
                .collect();
            let mut taken: HashSet<usize> = members.iter().copied().collect();
            let mut next = 0;
            while next < members.len() {
                let pid = table[members[next]].pid;
                for &child in children.get(&pid).map(Vec::as_slice).unwrap_or_default() {
                    if taken.insert(child) {
                        members.push(child);
                    }
                }
                next += 1;
            }
            watched.seen = members.iter().map(|&at| (table[at].pid, table[at].start)).collect();
            for &at in &members {
                if watched.command.check(&process(&table[at])) {
                    stop_descendants(&table, &children, table[at].pid);
                }
            }
        }
        state.watched.retain(|watched| !watched.seen.is_empty());
        true
    }
}

#[cfg(target_os = "macos")]
mod mac {
    use super::{BACKSTOP, Command, Process};
    use hashbrown::HashMap;
    use parking_lot::Mutex;
    use std::sync::OnceLock;
    use std::time::{Duration, Instant};

    /// Children of one process listed at most this often for a command,
    /// however fast it forks.
    const RELIST: Duration = Duration::from_millis(100);

    /// The ident of the event that wakes the thread for a new command.
    const WAKE: usize = 1;

    /// Pids at most in one listing.
    const MOST: usize = 4096;

    struct Watched {
        command: Command,
        /// Parents whose new children are still to be listed, once
        /// `listed` is RELIST ago.
        forked: Vec<i32>,
        listed: Option<Instant>,
    }

    struct State {
        kq: i32,
        commands: HashMap<u64, Watched>,
        /// Each watched process: its command and its start time.
        owner: HashMap<i32, (u64, (u64, u64))>,
        next: u64,
        backstop: Instant,
    }

    fn state() -> Option<&'static Mutex<State>> {
        static STATE: OnceLock<Option<Mutex<State>>> = OnceLock::new();
        STATE
            .get_or_init(|| {
                // SAFETY: a new kqueue, ours alone.
                let kq = unsafe { libc::kqueue() };
                if kq < 0 {
                    tracing::warn!(error = %std::io::Error::last_os_error(), "couldn't start the guard against programs that ask who you are");
                    return None;
                }
                let wake = event(WAKE, libc::EVFILT_USER, libc::EV_ADD | libc::EV_CLEAR, 0);
                change(kq, &wake);
                let spawned = std::thread::Builder::new().name("asking-guard".into()).spawn(run);
                if let Err(error) = spawned {
                    tracing::warn!(%error, "couldn't start the guard against programs that ask who you are");
                    return None;
                }
                Some(Mutex::new(State { kq, commands: HashMap::new(), owner: HashMap::new(), next: 0, backstop: Instant::now() }))
            })
            .as_ref()
    }

    fn event(ident: usize, filter: i16, flags: u16, fflags: u32) -> libc::kevent {
        libc::kevent { ident, filter, flags, fflags, data: 0, udata: std::ptr::null_mut() }
    }

    /// Adds `change` to the kqueue; false when it couldn't be (the process
    /// is gone).
    fn change(kq: i32, change: &libc::kevent) -> bool {
        // SAFETY: one change, no events asked for.
        unsafe { libc::kevent(kq, change, 1, std::ptr::null_mut(), 0, std::ptr::null()) == 0 }
    }

    pub(super) fn watch(command: Command) {
        let Some(state) = state() else { return };
        let mut state = state.lock();
        let id = state.next;
        state.next += 1;
        let root = command.root;
        let was_idle = state.commands.is_empty();
        state.commands.insert(id, Watched { command, forked: Vec::new(), listed: None });
        if was_idle {
            state.backstop = Instant::now();
        }
        adopt(&mut state, id, root);
        for pid in group(root) {
            adopt(&mut state, id, pid);
        }
        // Wakes the thread, which waits for nothing while no command is
        // guarded, so the backstop runs from now on.
        let kq = state.kq;
        change(kq, &event(WAKE, libc::EVFILT_USER, 0, libc::NOTE_TRIGGER));
    }

    /// The pids a listing call wrote.
    fn listed(call: impl FnOnce(*mut libc::c_void, i32) -> i32) -> Vec<i32> {
        let mut pids = vec![0 as libc::pid_t; MOST];
        let size = i32::try_from(std::mem::size_of_val(pids.as_slice())).unwrap_or(i32::MAX);
        if call(pids.as_mut_ptr().cast(), size) <= 0 {
            return Vec::new();
        }
        pids.retain(|pid| *pid > 0);
        pids
    }

    /// The processes in group `pgid`.
    pub(super) fn group(pgid: i32) -> Vec<i32> {
        // SAFETY: the buffer holds `size` bytes.
        listed(|buffer, size| unsafe { libc::proc_listpgrppids(pgid, buffer, size) })
    }

    fn children(pid: i32) -> Vec<i32> {
        // SAFETY: the buffer holds `size` bytes.
        listed(|buffer, size| unsafe { libc::proc_listchildpids(pid, buffer, size) })
    }

    /// When `pid` started: with the pid, which process it is. None once
    /// it's gone. From sysctl's kern.proc.pid, as ps reads it, since
    /// proc_pidinfo refuses a process that runs set-uid (a real sudo, as
    /// root) and the guard would then lose it (Abbacchio 47134).
    fn started(pid: i32) -> Option<(u64, u64)> {
        // kinfo_proc (648 bytes) begins with extern_proc, whose first
        // member is a union holding p_starttime, a timeval: tv_sec (64
        // bits), then tv_usec (32).
        let mut info = [0_u64; 128];
        let mut size = std::mem::size_of_val(&info);
        let mut name = [libc::CTL_KERN, libc::KERN_PROC, libc::KERN_PROC_PID, pid];
        // SAFETY: name holds 4 ints; info holds `size` bytes, more than a
        // kinfo_proc; size says how much was written.
        let read = unsafe { libc::sysctl(name.as_mut_ptr(), 4, info.as_mut_ptr().cast(), &raw mut size, std::ptr::null_mut(), 0) };
        (read == 0 && size >= 16 && info[0] > 0).then_some((info[0], info[1] & 0xffff_ffff))
    }

    /// The process `pid` is, with its name and the file it runs.
    pub(super) fn process(pid: i32) -> Option<Process> {
        // Its name can't be read once it runs as root (sudo, su), but the
        // file it runs can.
        let mut name = [0_u8; 256];
        // SAFETY: name holds 256 bytes.
        let length = unsafe { libc::proc_name(pid, name.as_mut_ptr().cast(), 256) };
        let name = usize::try_from(length)
            .map_or_else(|_| String::new(), |length| String::from_utf8_lossy(&name[..length.min(name.len())]).into_owned());
        let mut path = vec![0_u8; libc::PROC_PIDPATHINFO_MAXSIZE as usize];
        // SAFETY: path holds PROC_PIDPATHINFO_MAXSIZE bytes.
        let length = unsafe { libc::proc_pidpath(pid, path.as_mut_ptr().cast(), libc::PROC_PIDPATHINFO_MAXSIZE as u32) };
        let path = usize::try_from(length)
            .map_or_else(|_| String::new(), |length| String::from_utf8_lossy(&path[..length.min(path.len())]).into_owned());
        (!name.is_empty() || !path.is_empty()).then_some(Process { pid, name, path })
    }

    /// Takes `pid` into command `id`: registered first, then checked, then
    /// its children (which it may have made before it was registered) the
    /// same way.
    fn adopt(state: &mut State, id: u64, pid: i32) {
        let mut pending = vec![pid];
        while let Some(pid) = pending.pop() {
            if pid <= 1 || state.owner.contains_key(&pid) {
                continue;
            }
            let Ok(ident) = usize::try_from(pid) else { continue };
            let watching = event(ident, libc::EVFILT_PROC, libc::EV_ADD, libc::NOTE_FORK | libc::NOTE_EXEC | libc::NOTE_EXIT);
            if !change(state.kq, &watching) {
                // Gone already, or not ours to watch: checked all the same.
                if let (Some(process), Some(watched)) = (process(pid), state.commands.get_mut(&id)) {
                    watched.command.check(&process);
                }
                continue;
            }
            // Checked even when its start can't be read: never let go
            // unchecked.
            if let Some(start) = started(pid) {
                state.owner.insert(pid, (id, start));
            }
            if let (Some(process), Some(watched)) = (process(pid), state.commands.get_mut(&id)) {
                watched.command.check(&process);
            }
            pending.extend(children(pid));
        }
    }

    /// Lists `pid`'s children now, or once RELIST has passed for its
    /// command, however fast it forks.
    fn forked(state: &mut State, pid: i32, now: Instant) {
        let Some(&(id, _)) = state.owner.get(&pid) else { return };
        let Some(watched) = state.commands.get_mut(&id) else { return };
        if watched.listed.is_some_and(|listed| now.duration_since(listed) < RELIST) {
            if !watched.forked.contains(&pid) {
                watched.forked.push(pid);
            }
            return;
        }
        watched.listed = Some(now);
        for child in children(pid) {
            adopt(state, id, child);
        }
    }

    /// The forks waiting on RELIST whose time has come.
    fn relist(state: &mut State, now: Instant) {
        let due: Vec<(u64, Vec<i32>)> = state
            .commands
            .iter_mut()
            .filter(|(_, watched)| !watched.forked.is_empty() && watched.listed.is_none_or(|listed| now.duration_since(listed) >= RELIST))
            .map(|(id, watched)| {
                watched.listed = Some(now);
                (*id, std::mem::take(&mut watched.forked))
            })
            .collect();
        for (id, parents) in due {
            for parent in parents {
                for child in children(parent) {
                    adopt(state, id, child);
                }
            }
        }
    }

    /// Every 500 ms: each command's group listed again and each of its
    /// processes checked, in case an event was missed; processes and
    /// commands that are gone are let go.
    fn backstop(state: &mut State) {
        let gone: Vec<i32> = state.owner.iter().filter(|(pid, (_, start))| started(**pid) != Some(*start)).map(|(pid, _)| *pid).collect();
        for pid in gone {
            state.owner.remove(&pid);
        }
        // None of a command's processes left: it's over, and its group's id
        // may be another group's by now (Fugo 46798), so it isn't listed.
        let alive: hashbrown::HashSet<u64> = state.owner.values().map(|(id, _)| *id).collect();
        state.commands.retain(|id, _| alive.contains(id));
        let ids: Vec<(u64, i32)> = state.commands.iter().map(|(id, watched)| (*id, watched.command.root)).collect();
        for (id, root) in ids {
            for pid in group(root) {
                adopt(state, id, pid);
            }
        }
        let owned: Vec<(i32, u64)> = state.owner.iter().map(|(pid, (id, _))| (*pid, *id)).collect();
        for (pid, id) in owned {
            if let (Some(process), Some(watched)) = (process(pid), state.commands.get_mut(&id)) {
                watched.command.check(&process);
            }
        }
    }

    /// The guard's thread: events as they come, the forks waiting on
    /// RELIST, and the backstop every 500 ms while a command is guarded.
    fn run() {
        let Some(state) = state() else { return };
        let kq = state.lock().kq;
        let mut events = vec![event(0, 0, 0, 0); 64];
        loop {
            let timeout = {
                let state = state.lock();
                if state.commands.is_empty() {
                    None
                } else {
                    let now = Instant::now();
                    let mut wait = BACKSTOP.saturating_sub(now.duration_since(state.backstop));
                    for watched in state.commands.values().filter(|watched| !watched.forked.is_empty()) {
                        let since = watched.listed.map_or(RELIST, |listed| now.duration_since(listed));
                        wait = wait.min(RELIST.saturating_sub(since));
                    }
                    Some(libc::timespec { tv_sec: wait.as_secs() as libc::time_t, tv_nsec: libc::c_long::from(wait.subsec_nanos()) })
                }
            };
            let pointer = timeout.as_ref().map_or(std::ptr::null(), std::ptr::from_ref);
            // SAFETY: events holds 64 kevents.
            let count = unsafe { libc::kevent(kq, std::ptr::null(), 0, events.as_mut_ptr(), 64, pointer) };
            let mut state = state.lock();
            let now = Instant::now();
            for event in events.iter().take(usize::try_from(count).unwrap_or(0)) {
                if event.filter != libc::EVFILT_PROC {
                    continue;
                }
                let Ok(pid) = i32::try_from(event.ident) else { continue };
                if event.fflags & libc::NOTE_EXEC != 0
                    && let Some(&(id, _)) = state.owner.get(&pid)
                    && let (Some(process), Some(watched)) = (process(pid), state.commands.get_mut(&id))
                {
                    watched.command.check(&process);
                }
                if event.fflags & libc::NOTE_FORK != 0 {
                    forked(&mut state, pid, now);
                }
                if event.fflags & libc::NOTE_EXIT != 0 {
                    state.owner.remove(&pid);
                }
            }
            relist(&mut state, now);
            if now.duration_since(state.backstop) >= BACKSTOP {
                state.backstop = now;
                backstop(&mut state);
            }
        }
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::osascript_file;

    #[test]
    fn osascript_runs_the_first_word_that_isnt_an_option_unless_it_runs_statements() {
        let words = |line: &str| line.split(' ').map(str::to_owned).collect::<Vec<_>>();
        assert_eq!(osascript_file(&words("osascript a.scpt x")), Some("a.scpt"));
        assert_eq!(osascript_file(&words("osascript -l JavaScript -s eh a.js")), Some("a.js"));
        assert_eq!(osascript_file(&words("osascript -i -sh a.scpt")), Some("a.scpt"));
        assert_eq!(osascript_file(&words("osascript -- -odd.scpt")), Some("-odd.scpt"));
        assert_eq!(osascript_file(&words("osascript -e beep a.scpt")), None);
        assert_eq!(osascript_file(&words("osascript -")), None);
        assert_eq!(osascript_file(&words("osascript")), None);
    }
}
