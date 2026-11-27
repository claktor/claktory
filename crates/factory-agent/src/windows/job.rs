//! A named Job Object per command: Windows' process group for Claktory.
//!
//! Each command the bash tool, a monitor's check or a command card starts
//! goes into a job of its own as it's created (`spawn`), so everything it
//! starts is in that job too, and stopping it ends them all
//! (TerminateJobObject), not just the shell. Jobs aren't killed on close, so
//! a command outlives the backend that started it, as a process group does
//! on a Mac: an update or a restart of the backend doesn't end agents' work.
//!
//! Fugo's conditions (44974), each held here:
//! 1. Each job's name is random (128 bits), never made from the task's id,
//!    and kept only in this Claktory's records (`keep_records`). It's
//!    created with a protected DACL for this user and SYSTEM only, and a name
//!    that already exists is refused rather than joined.
//! 2. After a restart a job is found again by its name, and trusted only if
//!    this user owns it with the private DACL (45257 (a)) and, while the
//!    process it was made for still runs (pid and creation time, never a pid
//!    alone), that process is in it (IsProcessInJob).
//! 3. No breakaway: neither JOB_OBJECT_LIMIT_BREAKAWAY_OK nor
//!    JOB_OBJECT_LIMIT_SILENT_BREAKAWAY_OK is ever set.
//! 4. (`spawn`) Only the command's own handles are inherited.
//! 5. Stopping ends the whole job.
//!
//! Elsewhere a task is known by its process group's id, its shell's pid; the
//! same pid names its job here, so `tools::bash` keeps one number.
//!
//! A job is lifetime bookkeeping, not a boundary: any of this user's
//! programs can open it, as any of them can signal a process group.

use hashbrown::HashMap;
use parking_lot::Mutex;
use std::os::windows::io::{AsRawHandle, OwnedHandle};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use windows_sys::Win32::Foundation::{ERROR_ALREADY_EXISTS, ERROR_FILE_NOT_FOUND, GetLastError};
use windows_sys::Win32::System::JobObjects::{
    CreateJobObjectW, IsProcessInJob, JOB_OBJECT_LIMIT_DIE_ON_UNHANDLED_EXCEPTION, JOBOBJECT_BASIC_ACCOUNTING_INFORMATION,
    JOBOBJECT_BASIC_PROCESS_ID_LIST, JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectBasicAccountingInformation,
    JobObjectBasicProcessIdList, JobObjectExtendedLimitInformation, OpenJobObjectW, QueryInformationJobObject, SetInformationJobObject,
    TerminateJobObject,
};

use super::system::{
    Protected, STOPPED_CODE, creation_time, hex, image_path, is_console_host, is_private_object, open_process, random_bytes,
};
use super::wide::{check, owned, wide};

/// Where job names live: this session's namespace, so they're this
/// sign-in's alone.
const PREFIX: &str = "Local\\claktory-job-";

/// The access a reopened job is asked for (winnt.h's job rights).
const JOB_OBJECT_ASSIGN_PROCESS: u32 = 0x0001;
const JOB_OBJECT_QUERY: u32 = 0x0004;
const JOB_OBJECT_TERMINATE: u32 = 0x0008;
/// To read its owner and DACL (`find`), which Fugo 45353 caught missing.
const READ_CONTROL: u32 = 0x0002_0000;

/// A job, open.
pub struct Job {
    handle: OwnedHandle,
    name: String,
}

impl Job {
    /// A new job with a random name and a protected DACL. Its processes may
    /// not leave it, and one that crashes ends rather than waiting on an
    /// error report no one will see.
    pub fn create() -> std::io::Result<Job> {
        let name = format!("{PREFIX}{}", hex(&random_bytes::<16>()?));
        let protected = Protected::for_user()?;
        let attributes = protected.attributes();
        let text = wide(&name)?;
        // SAFETY: attributes and name outlive the call; the handle is checked and owned.
        let handle = owned(unsafe { CreateJobObjectW(&raw const attributes, text.as_ptr()) })?;
        // SAFETY: plain call, right after the one whose error it reads.
        if unsafe { GetLastError() } == ERROR_ALREADY_EXISTS {
            // 128 random bits taken already: someone made it on purpose.
            return Err(std::io::Error::new(std::io::ErrorKind::AlreadyExists, format!("a job named {name} already exists")));
        }
        let job = Job { handle, name };
        // SAFETY: an all-zero JOBOBJECT_EXTENDED_LIMIT_INFORMATION is valid (no limits).
        let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { std::mem::zeroed() };
        limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_DIE_ON_UNHANDLED_EXCEPTION;
        // SAFETY: the structure and its size.
        check(unsafe {
            SetInformationJobObject(
                job.raw(),
                JobObjectExtendedLimitInformation,
                (&raw const limits).cast(),
                std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            )
        })?;
        Ok(job)
    }

    /// The job named `name`, if it's still there (its processes, or a
    /// handle to it, keep it).
    fn open(name: &str) -> std::io::Result<Job> {
        let text = wide(name)?;
        // SAFETY: a NUL-terminated name; the handle is checked and owned.
        let handle = owned(unsafe {
            OpenJobObjectW(JOB_OBJECT_QUERY | JOB_OBJECT_TERMINATE | JOB_OBJECT_ASSIGN_PROCESS | READ_CONTROL, 0, text.as_ptr())
        })?;
        Ok(Job { handle, name: name.to_owned() })
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub(crate) fn raw(&self) -> windows_sys::Win32::Foundation::HANDLE {
        self.handle.as_raw_handle()
    }

    /// How many of its processes are running.
    pub fn active(&self) -> std::io::Result<u32> {
        // SAFETY: an all-zero accounting structure is valid to write over.
        let mut info: JOBOBJECT_BASIC_ACCOUNTING_INFORMATION = unsafe { std::mem::zeroed() };
        // SAFETY: the structure and its size.
        check(unsafe {
            QueryInformationJobObject(
                self.raw(),
                JobObjectBasicAccountingInformation,
                (&raw mut info).cast(),
                std::mem::size_of::<JOBOBJECT_BASIC_ACCOUNTING_INFORMATION>() as u32,
                std::ptr::null_mut(),
            )
        })?;
        Ok(info.ActiveProcesses)
    }

    /// Its running processes but the console host (`is_console_host`),
    /// each with its program's path: what still runs of its command. A
    /// process that ended since the list was read is left out.
    pub fn running(&self) -> std::io::Result<Vec<(u32, String)>> {
        let mut running = Vec::new();
        for pid in self.pids()? {
            let image = match open_process(pid) {
                Ok(process) => image_path(&process),
                // ERROR_INVALID_PARAMETER: no process by that pid any more.
                Err(error) if error.raw_os_error() == Some(87) => continue,
                Err(_) => String::new(),
            };
            if !is_console_host(&image) {
                running.push((pid, image));
            }
        }
        Ok(running)
    }

    /// The pids of its running processes.
    pub fn pids(&self) -> std::io::Result<Vec<u32>> {
        let mut room = 64usize;
        loop {
            // The list's header (two u32s), then `room` pids (usize each).
            let words = 1 + room;
            let mut buffer = vec![0usize; words];
            let size = words * std::mem::size_of::<usize>();
            // SAFETY: `size` bytes to write.
            let done = unsafe {
                QueryInformationJobObject(
                    self.raw(),
                    JobObjectBasicProcessIdList,
                    buffer.as_mut_ptr().cast(),
                    size as u32,
                    std::ptr::null_mut(),
                )
            };
            // SAFETY: the buffer starts with the list's header, whatever happened.
            let list = unsafe { &*buffer.as_ptr().cast::<JOBOBJECT_BASIC_PROCESS_ID_LIST>() };
            if done != 0 && list.NumberOfAssignedProcesses <= list.NumberOfProcessIdsInList {
                let count = list.NumberOfProcessIdsInList as usize;
                return Ok(buffer[1..1 + count.min(room)].iter().map(|pid| *pid as u32).collect());
            }
            if done == 0 && std::io::Error::last_os_error().raw_os_error() != Some(234 /* ERROR_MORE_DATA */) {
                return Err(std::io::Error::last_os_error());
            }
            room = (list.NumberOfAssignedProcesses as usize).max(room * 2) + 8;
        }
    }

    /// Whether `process` is in this job.
    fn contains(&self, process: &OwnedHandle) -> std::io::Result<bool> {
        let mut inside = 0;
        // SAFETY: both handles are open.
        check(unsafe { IsProcessInJob(process.as_raw_handle(), self.raw(), &raw mut inside) })?;
        Ok(inside != 0)
    }

    /// Ends every process in it.
    pub fn terminate(&self) -> std::io::Result<()> {
        // SAFETY: an open job handle with JOB_OBJECT_TERMINATE.
        check(unsafe { TerminateJobObject(self.raw(), STOPPED_CODE) })
    }
}

/// What's kept of a job so the next backend can find it again.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Record {
    /// Its name (random; secret from other users, not from this one).
    pub name: String,
    /// The process it was made for: the command's shell.
    pub pid: u32,
    /// That process's creation time (FILETIME units).
    pub created: u64,
}

#[derive(Default, Serialize, Deserialize)]
struct Saved {
    jobs: Vec<Record>,
}

#[derive(Default)]
struct Jobs {
    /// The records' file; None until the backend says where it is.
    file: Option<PathBuf>,
    records: HashMap<u32, Record>,
    /// Jobs this process has open: the ones it made, and ones it found again.
    open: HashMap<u32, Job>,
}

static JOBS: Mutex<Option<Jobs>> = Mutex::new(None);

fn with<T>(f: impl FnOnce(&mut Jobs) -> T) -> T {
    let mut jobs = JOBS.lock();
    f(jobs.get_or_insert_with(Jobs::default))
}

impl Jobs {
    fn save(&self) {
        let Some(file) = &self.file else { return };
        let mut jobs: Vec<Record> = self.records.values().cloned().collect();
        jobs.sort_by_key(|record| record.pid);
        if let Err(error) = write_records(file, &Saved { jobs }) {
            tracing::warn!(file = %file.display(), %error, "could not save the commands' jobs");
        }
    }

    fn forget(&mut self, pid: u32) {
        self.open.remove(&pid);
        if self.records.remove(&pid).is_some() {
            self.save();
        }
    }
}

fn write_records(file: &Path, saved: &Saved) -> std::io::Result<()> {
    let text = serde_json::to_vec_pretty(saved).map_err(std::io::Error::other)?;
    super::private::write(file, &text)
}

/// Keeps the records of jobs in `file` (the backend's, under its home), and
/// reads those an earlier backend left. Until it's called, jobs are only
/// remembered in memory.
pub fn keep_records(file: PathBuf) -> std::io::Result<()> {
    let saved: Saved = match std::fs::read(&file) {
        Ok(bytes) => serde_json::from_slice(&bytes).map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Saved::default(),
        Err(error) => return Err(error),
    };
    // Those whose jobs have emptied since go; the rest are found again
    // when asked about.
    let left: Vec<Record> = saved.jobs.into_iter().filter(|record| !matches!(find(record), Found::Gone)).collect();
    with(|jobs| {
        for record in left {
            jobs.records.entry(record.pid).or_insert(record);
        }
        jobs.file = Some(file);
        jobs.save();
    });
    Ok(())
}

/// Remembers a job made for a command whose shell is `pid`.
pub(crate) fn remember(job: Job, pid: u32, created: u64) {
    with(|jobs| {
        jobs.records.insert(pid, Record { name: job.name.clone(), pid, created });
        jobs.open.insert(pid, job);
        jobs.save();
    });
}

/// The command whose shell is `pid` has ended: its job is forgotten once
/// nothing in it runs. One whose shell left something running (`server &`)
/// stays, found by `alive` and `stop` like any other, until it empties.
pub(crate) fn settle(pid: u32) {
    let empty = with(|jobs| jobs.open.get(&pid).map(|job| job.running().is_ok_and(|running| running.is_empty())));
    if empty == Some(true) {
        with(|jobs| jobs.forget(pid));
    }
}

/// What finding a recorded job again came to.
enum Found {
    /// It's there and it's this Claktory's: owned by this user with the
    /// private DACL, and holding its shell when that still runs.
    Ours,
    /// It's gone: all its processes ended.
    Gone,
    /// Something by its name isn't the job Claktory made.
    Foreign,
}

/// Finds a recorded job again (Fugo 44974 condition 2, and 45257 (a)): by
/// its name; then only if this user owns it and its DACL is the private one
/// (another account in this session, under runas say, shares `Local\`, so
/// a leaked name alone gives nothing); then, while its shell still runs
/// with the recorded creation time, only if the shell is in it. A job
/// whose shell has ended (a `server &` it left) is trusted on the first two,
/// so the person can still stop it after a restart.
fn find(record: &Record) -> Found {
    let job = match Job::open(&record.name) {
        Ok(job) => job,
        Err(error) if error.raw_os_error() == Some(ERROR_FILE_NOT_FOUND as i32) => return Found::Gone,
        Err(error) => {
            tracing::warn!(job = %record.name, %error, "could not open a command's job");
            return Found::Foreign;
        }
    };
    match is_private_object(&job.handle) {
        Ok(true) => {}
        Ok(false) => {
            tracing::warn!(job = %record.name, "a command's job isn't owned by this user with Claktory's DACL; leaving it alone");
            return Found::Foreign;
        }
        Err(error) => {
            tracing::warn!(job = %record.name, %error, "could not read a command's job's security");
            return Found::Foreign;
        }
    }
    let shell = open_process(record.pid).ok().filter(|process| creation_time(process).ok() == Some(record.created));
    if let Some(process) = shell
        && !matches!(job.contains(&process), Ok(true))
    {
        tracing::warn!(job = %record.name, pid = record.pid, "a command's job doesn't hold its shell; leaving it alone");
        return Found::Foreign;
    }
    with(|jobs| jobs.open.insert(record.pid, job));
    Found::Ours
}

/// The job of the command whose shell is `pid`, open, if it's one this
/// Claktory made.
fn checked(pid: u32) -> Option<()> {
    if with(|jobs| jobs.open.contains_key(&pid)) {
        return Some(());
    }
    let record = with(|jobs| jobs.records.get(&pid).cloned())?;
    matches!(find(&record), Found::Ours).then_some(())
}

/// Whether any process of the command whose shell is `pid` is running:
/// `group_alive` on Windows. A job that's gone is forgotten.
pub fn alive(pid: u32) -> bool {
    let active = checked(pid).is_some()
        && with(|jobs| jobs.open.get(&pid).map(Job::running)).is_some_and(|running| running.is_ok_and(|running| !running.is_empty()));
    if !active {
        with(|jobs| jobs.forget(pid));
    }
    active
}

/// Ends every process of the command whose shell is `pid`: `stop_group`
/// on Windows. Only a job this Claktory made (`find`) is stopped; false
/// otherwise, or when it had already ended.
pub fn stop(pid: u32) -> bool {
    if checked(pid).is_none() {
        return false;
    }
    let ended = with(|jobs| jobs.open.get(&pid).map(Job::terminate));
    match ended {
        Some(Ok(())) => true,
        Some(Err(error)) => {
            tracing::warn!(pid, %error, "could not stop a command's job");
            false
        }
        None => false,
    }
}

/// The processes of the command whose shell is `pid`, as (pid, the file it
/// runs): what the guard against prompts to act as someone else looks at.
pub fn processes(pid: u32) -> Vec<(u32, String)> {
    if checked(pid).is_none() {
        return Vec::new();
    }
    with(|jobs| jobs.open.get(&pid).map(Job::running)).and_then(Result::ok).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_new_job_is_empty_and_named_at_random() {
        let one = Job::create().expect("a job");
        let two = Job::create().expect("another");
        assert!(one.name().starts_with(PREFIX));
        assert_eq!(one.name().len(), PREFIX.len() + 32);
        assert_ne!(one.name(), two.name());
        assert_eq!(one.active().unwrap(), 0);
        assert!(one.pids().unwrap().is_empty());
    }

    #[test]
    fn an_unknown_pid_is_neither_alive_nor_stopped() {
        assert!(!alive(u32::MAX - 3));
        assert!(!stop(u32::MAX - 3));
        assert!(processes(u32::MAX - 3).is_empty());
    }

    #[test]
    fn a_new_job_is_private_and_found_again_by_its_record() {
        let job = Job::create().expect("a job");
        assert!(is_private_object(&job.handle).unwrap(), "owned by this user, user and SYSTEM only");
        // Reopened by name, as after a restart (the first handle keeps the job there).
        let reopened = Job::open(job.name()).expect("reopens by name");
        assert!(is_private_object(&reopened.handle).expect("a reopened job's security can be read"));
        assert_eq!(reopened.active().unwrap(), 0);
        drop(reopened);
        let me = open_process(std::process::id()).unwrap();
        // This test process isn't in the job: a record naming it as the shell is refused.
        let record = Record { name: job.name().to_owned(), pid: std::process::id(), created: creation_time(&me).unwrap() };
        assert!(matches!(find(&record), Found::Foreign));
        // A shell that's gone (another creation time) leaves it trusted by owner and DACL.
        let record = Record { created: 1, ..record };
        assert!(matches!(find(&record), Found::Ours));
        with(|jobs| jobs.forget(record.pid));
    }

    #[test]
    fn a_recorded_job_that_is_gone_reads_as_gone() {
        let record = Record { name: format!("{PREFIX}{}", "0".repeat(32)), pid: 4, created: 1 };
        assert!(matches!(find(&record), Found::Gone));
    }
}
