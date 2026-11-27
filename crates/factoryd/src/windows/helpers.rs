//! The helpers factoryd starts for itself on Windows (the apps host, speech
//! recognition) end with it, however it ends: each is in one job, unnamed
//! and held only by this process, that kills what's in it when its last
//! handle closes (JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE). `kill_on_drop` alone
//! fires only when factoryd drops the child normally; a crash or
//! TerminateProcess would leave them running.
//!
//! Each starts suspended, goes into the job, and only then runs its first
//! instruction, so nothing of it is ever outside the job. The job's handle
//! isn't inheritable: no child holds it open. Agents' commands aren't
//! helpers: their own jobs outlive the backend (`factory_agent::windows::job`).

use std::os::windows::io::{AsRawHandle, OwnedHandle};
use std::sync::OnceLock;

use factory_agent::windows::wide::{check, owned};
use windows_sys::Win32::Foundation::HANDLE;
use windows_sys::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, TH32CS_SNAPTHREAD, THREADENTRY32, Thread32First, Thread32Next,
};
use windows_sys::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_DIE_ON_UNHANDLED_EXCEPTION, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation, SetInformationJobObject,
};
use windows_sys::Win32::System::Threading::{CREATE_SUSPENDED, OpenThread, ResumeThread, THREAD_SUSPEND_RESUME};

/// The helpers' job, made at the first helper; Err when it couldn't be.
static JOB: OnceLock<Result<OwnedHandle, String>> = OnceLock::new();

fn job() -> std::io::Result<HANDLE> {
    match JOB.get_or_init(|| make_job().map_err(|error| error.to_string())) {
        Ok(job) => Ok(job.as_raw_handle()),
        Err(error) => Err(std::io::Error::other(format!("the helpers' job couldn't be made: {error}"))),
    }
}

fn make_job() -> std::io::Result<OwnedHandle> {
    // SAFETY: no name, default security (not inheritable); the handle is checked and owned.
    let job = unsafe { owned(CreateJobObjectW(std::ptr::null(), std::ptr::null())) }?;
    // SAFETY: an all-zero JOBOBJECT_EXTENDED_LIMIT_INFORMATION is valid (no limits).
    let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { std::mem::zeroed() };
    // No breakaway: what a helper starts stays in the job too.
    limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE | JOB_OBJECT_LIMIT_DIE_ON_UNHANDLED_EXCEPTION;
    // SAFETY: the structure and its size.
    check(unsafe {
        SetInformationJobObject(
            job.as_raw_handle(),
            JobObjectExtendedLimitInformation,
            (&raw const limits).cast(),
            std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
        )
    })?;
    Ok(job)
}

/// Starts `command` as a helper: suspended, in the helpers' job, then
/// running. One that can't go into the job is ended, never left running
/// outside it.
pub fn spawn(command: &mut tokio::process::Command) -> std::io::Result<tokio::process::Child> {
    let job = job()?;
    command.creation_flags(CREATE_SUSPENDED);
    let mut child = command.spawn()?;
    let admitted = (|| {
        let process = child.raw_handle().ok_or_else(|| std::io::Error::other("the helper ended at once"))?;
        // SAFETY: our job and the suspended child's process handle.
        check(unsafe { AssignProcessToJobObject(job, process) })?;
        resume(child.id().ok_or_else(|| std::io::Error::other("the helper ended at once"))?)
    })();
    if let Err(error) = admitted {
        // Suspended still, it ran nothing: ended here.
        let _ = child.start_kill();
        return Err(error);
    }
    Ok(child)
}

/// Resumes the one thread of a process started suspended.
fn resume(pid: u32) -> std::io::Result<()> {
    // SAFETY: a snapshot of the system's threads, checked and owned.
    let snapshot = unsafe { owned(CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0)) }?;
    // SAFETY: an all-zero THREADENTRY32 is valid once its size is set.
    let mut entry: THREADENTRY32 = unsafe { std::mem::zeroed() };
    entry.dwSize = std::mem::size_of::<THREADENTRY32>() as u32;
    let mut resumed = 0;
    // SAFETY: the snapshot and the entry to fill.
    let mut more = unsafe { Thread32First(snapshot.as_raw_handle(), &raw mut entry) } != 0;
    while more {
        if entry.th32OwnerProcessID == pid {
            // SAFETY: the thread's id from the snapshot; the handle is checked and owned.
            let thread = unsafe { owned(OpenThread(THREAD_SUSPEND_RESUME, 0, entry.th32ThreadID)) }?;
            // SAFETY: an open thread handle with the right to resume it.
            if unsafe { ResumeThread(thread.as_raw_handle()) } == u32::MAX {
                return Err(std::io::Error::last_os_error());
            }
            resumed += 1;
        }
        // SAFETY: as above.
        more = unsafe { Thread32Next(snapshot.as_raw_handle(), &raw mut entry) } != 0;
    }
    if resumed == 0 {
        return Err(std::io::Error::other("the helper's thread wasn't found"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows_sys::Win32::System::Threading::{
        OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SYNCHRONIZE, PROCESS_TERMINATE, TerminateProcess, WaitForSingleObject,
    };

    /// A process opened to wait on and end.
    fn open_process(pid: u32) -> std::io::Result<OwnedHandle> {
        // SAFETY: plain call; the handle is checked and owned.
        unsafe { owned(OpenProcess(PROCESS_SYNCHRONIZE | PROCESS_TERMINATE | PROCESS_QUERY_LIMITED_INFORMATION, 0, pid)) }
    }

    /// Set in the test's child, the "backend": it starts two long helpers,
    /// one as a helper and one plainly, says their pids, and waits.
    const BACKEND: &str = "FACTORY_TEST_HELPERS_BACKEND";

    fn long_command() -> tokio::process::Command {
        // Waits 100 s for a signal no one sends.
        let mut command = tokio::process::Command::new("waitfor.exe");
        command.args(["/t", "100", "claktory-helpers-test-never"]).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null());
        command
    }

    #[test]
    fn child_backend_starts_helpers() {
        if std::env::var_os(BACKEND).is_none() {
            return;
        }
        let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().expect("runtime");
        runtime.block_on(async {
            let helper = spawn(&mut long_command()).expect("helper");
            let mut plain = long_command();
            let plain = plain.spawn().expect("plain");
            println!("HELPERS {} {}", helper.id().unwrap(), plain.id().unwrap());
            tokio::time::sleep(std::time::Duration::from_secs(100)).await;
            drop((helper, plain));
        });
    }

    fn gone_within(pid: u32, millis: u32) -> bool {
        let Ok(process) = open_process(pid) else { return true };
        // SAFETY: an open process handle; waits at most `millis`.
        unsafe { WaitForSingleObject(process.as_raw_handle(), millis) == 0 }
    }

    #[test]
    fn helpers_end_when_the_backend_is_killed_hard() {
        use std::io::BufRead;
        let exe = std::env::current_exe().expect("test binary");
        let mut backend = std::process::Command::new(exe)
            .args(["windows::helpers::tests::child_backend_starts_helpers", "--exact", "--nocapture", "--test-threads=1"])
            .env(BACKEND, "1")
            .stdout(std::process::Stdio::piped())
            .spawn()
            .expect("backend");
        let mut lines = std::io::BufReader::new(backend.stdout.take().unwrap()).lines();
        let pids = lines
            .by_ref()
            .map_while(Result::ok)
            .find_map(|line| line.split_once("HELPERS ").map(|(_, pids)| pids.to_owned()))
            .expect("pids");
        let (helper, plain) = pids.split_once(' ').map(|(a, b)| (a.parse::<u32>().unwrap(), b.trim().parse::<u32>().unwrap())).unwrap();
        assert!(!gone_within(helper, 0) && !gone_within(plain, 0), "both run");

        // As a crash would: no drop, no clean exit.
        let process = open_process(backend.id()).expect("backend's handle");
        // SAFETY: our child's process handle.
        assert!(unsafe { TerminateProcess(process.as_raw_handle(), 1) } != 0);
        let _ = backend.wait();

        assert!(gone_within(helper, 5000), "the helper outlived the backend");
        // The control: a child started plainly outlives it, which is why
        // helpers are in the job.
        assert!(!gone_within(plain, 0), "the plain child ended too, so the test proves nothing");
        if let Ok(plain) = open_process(plain) {
            // SAFETY: cleaning up the control.
            unsafe { TerminateProcess(plain.as_raw_handle(), 1) };
        }
    }
}
