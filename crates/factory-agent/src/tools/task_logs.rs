//! Background tasks' logs over time: a chatty task (a dev server, a
//! watch build) keeps at most [`CAP`] of its output, its start and its most
//! recent, with a line where the rest was cut; logs of tasks that finished
//! [`KEPT_FOR`] ago go.

#[cfg(unix)]
use std::os::unix::fs::FileExt;
#[cfg(windows)]
use crate::windows::positioned::FileExt;
use std::path::Path;
use std::time::{Duration, SystemTime};

/// The most a background task's log keeps.
pub const CAP: u64 = 50 * 1024 * 1024;
/// What a cut keeps of its start: the first lines, up to this much.
const HEAD: u64 = 1024 * 1024;
/// And of its end: about this much, from a line's start, so the next cut
/// comes only after as much again.
const TAIL: u64 = 24 * 1024 * 1024;
/// How often a running task's log is measured.
pub const CHECK_EVERY: Duration = Duration::from_secs(2);
/// How long a finished task's log stays.
pub const KEPT_FOR: Duration = Duration::from_secs(7 * 24 * 3600);
/// The number of the newest task whose log went, in a task folder, so no
/// task's id is used again.
pub const PRUNED: &str = "pruned";

const CUT: &str = "[Claktory cut ";

/// The line a cut leaves, with all the output cut so far.
fn cut_line(cut: u64) -> String {
    format!("{CUT}{cut} bytes of this task's output here, to keep its log under 50 MB: its start is above, its most recent output below.]\n")
}

/// Cuts the middle of `log` once it's past [`CAP`], when the task writing it
/// (process group `pid`) appends, as every task this Claktory starts does:
/// one that writes at its own place (started by an older version) would
/// leave a hole as big as what was cut. Whether it cut. What the task
/// writes in the moment between the last copy and the end being set can be
/// lost.
pub fn cap(log: &Path, pid: u32) -> std::io::Result<bool> {
    let file = std::fs::OpenOptions::new().read(true).write(true).open(log)?;
    let length = file.metadata()?.len();
    if length <= CAP || !appends(pid) {
        return Ok(false);
    }
    cut(&file, length)?;
    Ok(true)
}

/// Cuts a finished task's log once it's past [`CAP`] (a task that writes
/// it all and ends between two checks was never cut): with its process
/// group gone (`pid` 0: unknown), nothing writes it any more. Whether it
/// cut.
pub fn cap_finished(log: &Path, pid: u32) -> std::io::Result<bool> {
    if pid != 0 && super::bash::group_alive(pid) {
        return Ok(false);
    }
    let file = std::fs::OpenOptions::new().read(true).write(true).open(log)?;
    let length = file.metadata()?.len();
    if length <= CAP {
        return Ok(false);
    }
    cut(&file, length)?;
    Ok(true)
}

/// Keeps `file`'s first lines and its end, with a line between saying how
/// much went.
fn cut(file: &std::fs::File, length: u64) -> std::io::Result<()> {
    let mut head = vec![0u8; HEAD as usize + 512];
    let read = file.read_at(&mut head, 0)?;
    head.truncate(read);
    // An earlier cut's line ends the start where it is (the start's last
    // line would otherwise fall after it): counted, and replaced. Else
    // the start is its last whole line in HEAD.
    let at_cut = (0..head.len()).filter(|at| *at == 0 || head[at - 1] == b'\n').find(|at| earlier_cut(&head[*at..]).1 > 0);
    let head_end = match at_cut {
        Some(at) => at,
        None => {
            let start = &head[..head.len().min(HEAD as usize)];
            start.iter().rposition(|byte| *byte == b'\n').map_or(start.len(), |at| at + 1)
        }
    } as u64;
    let (before, earlier) = at_cut.map_or((0, 0), |at| earlier_cut(&head[at..]));
    let mut tail_start = length.saturating_sub(TAIL);
    let mut line = vec![0u8; 64 * 1024];
    let read = file.read_at(&mut line, tail_start)?;
    if let Some(at) = line[..read].iter().position(|byte| *byte == b'\n') {
        tail_start += at as u64 + 1;
    }
    let cut_now = tail_start.saturating_sub(head_end + earlier);
    let marker = cut_line(before + cut_now);
    file.write_all_at(marker.as_bytes(), head_end)?;
    // The end, moved up: to the file's end as it is when read, so what the
    // task appends meanwhile comes too.
    let (mut from, mut to) = (tail_start, head_end + marker.len() as u64);
    let mut buffer = vec![0u8; 1 << 20];
    loop {
        let read = file.read_at(&mut buffer, from)?;
        if read == 0 {
            break;
        }
        file.write_all_at(&buffer[..read], to)?;
        from += read as u64;
        to += read as u64;
    }
    file.set_len(to)
}

/// An earlier cut's line at the start of `bytes`: (what it cut, its length).
fn earlier_cut(bytes: &[u8]) -> (u64, u64) {
    let Some(rest) = bytes.strip_prefix(CUT.as_bytes()) else { return (0, 0) };
    let digits = rest.iter().take_while(|byte| byte.is_ascii_digit()).count();
    let Some(cut) = std::str::from_utf8(&rest[..digits]).ok().and_then(|digits| digits.parse().ok()) else { return (0, 0) };
    match bytes.iter().position(|byte| *byte == b'\n') {
        Some(end) => (cut, end as u64 + 1),
        None => (0, 0),
    }
}

/// Whether the task's shell (`pid`, its group's leader) writes its output
/// at the log's end, wherever that is: its standard output opened to
/// append.
fn appends(pid: u32) -> bool {
    #[cfg(target_os = "macos")]
    {
        // proc_fileinfo starts with fi_openflags; PROC_PIDFDVNODEINFO's
        // answer (vnode_fdinfo) is 176 bytes.
        const PROC_PIDFDVNODEINFO: libc::c_int = 1;
        let mut answer = [0u64; 64];
        // SAFETY: the buffer is as big as it says, and bigger than the answer.
        let size = unsafe {
            libc::proc_pidfdinfo(pid as libc::c_int, 1, PROC_PIDFDVNODEINFO, answer.as_mut_ptr().cast(), std::mem::size_of_val(&answer) as libc::c_int)
        };
        size >= 4 && (answer[0] as u32) & (libc::O_APPEND as u32) != 0
    }
    // Every task's log is opened to append (FILE_APPEND_DATA) by this
    // backend, and there are no older Windows versions that didn't.
    #[cfg(windows)]
    {
        let _ = pid;
        true
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        let Ok(info) = std::fs::read_to_string(format!("/proc/{pid}/fdinfo/1")) else { return false };
        let flags = info.lines().find_map(|line| line.strip_prefix("flags:")).and_then(|flags| u32::from_str_radix(flags.trim(), 8).ok());
        flags.is_some_and(|flags| flags & (libc::O_APPEND as u32) != 0)
    }
}

/// Where a task stands, for [`prune`], as Claktory recorded it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Ended {
    /// It still runs: its log stays, however long it has been quiet.
    Running,
    /// It finished then.
    At(SystemTime),
    /// Not recorded: when its log was last written stands for when it
    /// finished.
    Unknown,
}

/// Removes, from a task folder, the logs (and what goes with them) of tasks
/// that finished more than [`KEPT_FOR`] ago, never a running one's:
/// `ended` says, by task id (task-N), when each finished. The newest number
/// removed is kept in [`PRUNED`]. A finished task's newer log still past
/// [`CAP`] is cut. How many went.
pub fn prune(dir: &Path, now: SystemTime, ended: impl Fn(&str) -> Ended) -> usize {
    let Ok(entries) = std::fs::read_dir(dir) else { return 0 };
    let mut removed = 0;
    let mut newest = pruned(dir);
    for entry in entries.flatten() {
        let path = entry.path();
        let Some(number) = task_number(&path).filter(|_| path.extension().is_some_and(|extension| extension == "log")) else { continue };
        let Some(id) = path.file_stem().and_then(|stem| stem.to_str()) else { continue };
        let finished = match ended(id) {
            Ended::Running => continue,
            Ended::At(at) => Some(at),
            Ended::Unknown => entry.metadata().and_then(|metadata| metadata.modified()).ok(),
        };
        let old = finished.is_some_and(|at| now.duration_since(at).is_ok_and(|age| age > KEPT_FOR));
        if !old {
            // A finished one (it recorded how it ended) still past the cap:
            // cut, as its end would have been.
            if path.with_extension("status").exists() {
                let _ = cap_finished(&path, 0);
            }
            continue;
        }
        if std::fs::remove_file(&path).is_ok() {
            removed += 1;
            newest = newest.max(number);
            let _ = std::fs::remove_file(path.with_extension("status"));
        }
    }
    if removed > 0 {
        let _ = std::fs::write(dir.join(PRUNED), format!("{newest}\n"));
    }
    removed
}

/// The newest task number whose log was removed from `dir`, or 0.
pub fn pruned(dir: &Path) -> u64 {
    std::fs::read_to_string(dir.join(PRUNED)).ok().and_then(|text| text.trim().parse().ok()).unwrap_or(0)
}

/// N, for a file named task-N.<anything>.
pub fn task_number(path: &Path) -> Option<u64> {
    path.file_stem()?.to_str()?.strip_prefix("task-")?.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(unix)]
    use std::os::unix::process::CommandExt;

    /// With short lines and long ones (with 1 KB lines the first cut's
    /// line ended up inside the kept start, uncounted).
    #[test]
    fn a_cut_keeps_the_start_and_the_end_and_counts_what_went() {
        for width in [84, 990] {
            cut_twice(width);
        }
    }

    fn cut_twice(width: usize) {
        let dir = std::env::temp_dir().join(format!("task-logs-{}-{width}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let log = dir.join("task-1.log");
        let line = |n: u64| format!("line {n:09} {}\n", "x".repeat(width));
        let mut text = String::new();
        let mut n = 0;
        while (text.len() as u64) < CAP + 1024 * 1024 {
            text.push_str(&line(n));
            n += 1;
        }
        std::fs::write(&log, &text).unwrap();
        let file = std::fs::OpenOptions::new().read(true).write(true).open(&log).unwrap();
        cut(&file, text.len() as u64).unwrap();
        let kept = std::fs::read_to_string(&log).unwrap();
        assert!(kept.len() as u64 <= HEAD + TAIL + 200, "{}", kept.len());
        assert!(kept.starts_with(&line(0)) && kept.ends_with(&line(n - 1)));
        let marker = kept.lines().find(|line| line.starts_with(CUT)).expect("the cut's line");
        let first_kept = kept.lines().skip_while(|line| !line.starts_with(CUT)).nth(1).unwrap();
        let lines_before = kept[..kept.find(CUT).unwrap()].lines().count() as u64;
        let resumed: u64 = first_kept[5..14].parse().unwrap();
        let went = (resumed - lines_before) * line(0).len() as u64;
        assert_eq!(marker, cut_line(went).trim_end(), "whole lines, counted");

        // More, and cut again: one line, counting both.
        let mut more = kept.clone();
        let mut added = 0;
        while (more.len() as u64) < CAP + 1024 * 1024 {
            more.push_str(&line(n));
            n += 1;
            added += 1;
        }
        std::fs::write(&log, &more).unwrap();
        cut(&file, more.len() as u64).unwrap();
        let again = std::fs::read_to_string(&log).unwrap();
        assert_eq!(again.lines().filter(|line| line.starts_with(CUT)).count(), 1);
        assert!(again.starts_with(&line(0)) && again.ends_with(&line(n - 1)), "{added}");
        let (total, _) = earlier_cut(&again.as_bytes()[again.find(CUT).unwrap()..]);
        let first_kept = again.lines().skip_while(|line| !line.starts_with(CUT)).nth(1).unwrap();
        let resumed: u64 = first_kept[5..14].parse().unwrap();
        assert_eq!(total, (resumed - lines_before) * line(0).len() as u64);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Only a log its task appends to is cut: one an older version's task
    /// writes at its own place never is. (Windows has no older tasks.)
    #[cfg(unix)]
    #[test]
    fn only_a_log_its_task_appends_to_is_cut() {
        let dir = std::env::temp_dir().join(format!("task-logs-own-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        for (name, append) in [("task-1.log", true), ("task-2.log", false)] {
            let log = dir.join(name);
            std::fs::write(&log, "start\n".repeat((CAP / 6 + 10) as usize)).unwrap();
            let file = std::fs::OpenOptions::new().write(true).append(append).open(&log).unwrap();
            let mut task = std::process::Command::new("/bin/sleep").arg("10").stdout(file).spawn().unwrap();
            assert_eq!(cap(&log, task.id()).unwrap(), append, "{name}");
            let length = std::fs::metadata(&log).unwrap().len();
            assert_eq!(length <= HEAD + TAIL + 200, append, "{name}: {length}");
            let _ = task.kill();
            let _ = task.wait();
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A task that wrote it all between two checks is cut when it ends,
    /// or at the next start; not while its group runs.
    #[cfg(unix)]
    #[test]
    fn a_finished_tasks_log_past_the_cap_is_cut() {
        let dir = std::env::temp_dir().join(format!("task-logs-finished-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let big = "a line of output\n".repeat((CAP / 17 + 100) as usize);
        let log = dir.join("task-1.log");
        std::fs::write(&log, &big).unwrap();
        // Its group still running: left alone.
        let mut running = std::process::Command::new("/bin/sleep").arg("10").process_group(0).spawn().unwrap();
        assert!(!cap_finished(&log, running.id()).unwrap());
        let _ = running.kill();
        let _ = running.wait();
        assert!(cap_finished(&log, running.id()).unwrap());
        assert!(std::fs::metadata(&log).unwrap().len() <= HEAD + TAIL + 200);
        // At start: a finished one (with its status) is cut, one without isn't.
        for (name, status) in [("task-2", true), ("task-3", false)] {
            std::fs::write(dir.join(format!("{name}.log")), &big).unwrap();
            if status {
                std::fs::write(dir.join(format!("{name}.status")), "0\n").unwrap();
            }
        }
        assert_eq!(prune(&dir, SystemTime::now(), |_| Ended::Unknown), 0);
        assert!(std::fs::metadata(dir.join("task-2.log")).unwrap().len() <= HEAD + TAIL + 200);
        assert_eq!(std::fs::metadata(dir.join("task-3.log")).unwrap().len(), big.len() as u64);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn old_logs_go_and_their_numbers_are_kept() {
        let dir = std::env::temp_dir().join(format!("task-logs-prune-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        for name in ["task-1.log", "task-1.status", "task-2.log", "task-3.log", "notes.txt"] {
            std::fs::write(dir.join(name), "x").unwrap();
        }
        let now = SystemTime::now();
        assert_eq!(prune(&dir, now, |_| Ended::Unknown), 0, "all new");
        let later = now + KEPT_FOR + Duration::from_secs(60);
        std::fs::OpenOptions::new().write(true).open(dir.join("task-3.log")).unwrap().set_modified(later).unwrap();
        assert_eq!(prune(&dir, later, |_| Ended::Unknown), 2);
        assert!(!dir.join("task-1.log").exists() && !dir.join("task-1.status").exists() && !dir.join("task-2.log").exists());
        assert!(dir.join("task-3.log").exists() && dir.join("notes.txt").exists());
        assert_eq!(pruned(&dir), 2);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A task that still runs keeps its log however
    /// long it has been quiet, and a finished one's week counts from when it
    /// finished, not from its log's last write.
    #[test]
    fn a_quiet_running_tasks_log_stays_and_a_week_counts_from_the_end() {
        let dir = std::env::temp_dir().join(format!("task-logs-running-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let now = SystemTime::now();
        let long_ago = now - KEPT_FOR - Duration::from_secs(3 * 24 * 3600);
        for name in ["task-1.log", "task-2.log", "task-2.status", "task-3.log", "task-4.log"] {
            std::fs::write(dir.join(name), "x").unwrap();
            std::fs::OpenOptions::new().write(true).open(dir.join(name)).unwrap().set_modified(long_ago).unwrap();
        }
        let ended = |id: &str| match id {
            // Quiet for ten days, still running.
            "task-1" => Ended::Running,
            // Quiet for ten days, finished yesterday.
            "task-2" => Ended::At(now - Duration::from_secs(24 * 3600)),
            // Finished eight days ago.
            "task-3" => Ended::At(now - KEPT_FOR - Duration::from_secs(24 * 3600)),
            _ => Ended::Unknown,
        };
        assert_eq!(prune(&dir, now, ended), 2);
        assert!(dir.join("task-1.log").exists(), "a running task's log stays");
        assert!(dir.join("task-2.log").exists() && dir.join("task-2.status").exists(), "a week from its end");
        assert!(!dir.join("task-3.log").exists() && !dir.join("task-4.log").exists());
        assert_eq!(pruned(&dir), 4);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
