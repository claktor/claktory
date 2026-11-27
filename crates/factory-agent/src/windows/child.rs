//! A command's shell while it runs: what `tokio::process::Child` is on a
//! Mac or Linux, for a process `spawn` made itself (tokio's Command can't
//! put a process in a job as it's created, nor limit what it inherits).
//!
//! Waiting is the thread pool's (RegisterWaitForSingleObject): no thread
//! of our own sits blocked per command, and `wait` can be raced in a
//! `select!` again and again, as tokio's can.

use parking_lot::Mutex;
use std::os::windows::io::{AsRawHandle, OwnedHandle};
use std::process::ExitStatus;

use tokio::sync::oneshot;
use windows_sys::Win32::Foundation::{HANDLE, INVALID_HANDLE_VALUE};
use windows_sys::Win32::System::Threading::{
    GetExitCodeProcess, INFINITE, RegisterWaitForSingleObject, TerminateProcess, UnregisterWaitEx, WT_EXECUTEINWAITTHREAD,
    WT_EXECUTEONLYONCE,
};

use super::system::exit_status;
use super::wide::check;

/// What the wait's callback is handed: whom to tell.
type Waker = Mutex<Option<oneshot::Sender<()>>>;

/// A running process this backend started.
pub struct Child {
    process: OwnedHandle,
    pid: u32,
    /// Its output, when it goes to a pipe (read through the blocking pool).
    pub stdout: Option<tokio::fs::File>,
    pub stderr: Option<tokio::fs::File>,
    /// Hears when it ends; None once it has.
    ended: Option<oneshot::Receiver<()>>,
    /// The registered wait and its callback's waker, until dropped.
    wait: Option<(HANDLE, Box<Waker>)>,
    status: Option<ExitStatus>,
    /// Ended when dropped while it runs (`kill_on_drop`), as tokio's.
    kill_on_drop: bool,
}

// SAFETY: the wait handle is only used to unregister, from whichever thread drops it.
unsafe impl Send for Child {}
unsafe impl Sync for Child {}

unsafe extern "system" fn exited(context: *mut core::ffi::c_void, _timed_out: bool) {
    // SAFETY: the context is the Child's Box<Waker>, alive until the wait is unregistered.
    let waker = unsafe { &*context.cast::<Waker>() };
    if let Some(sender) = waker.lock().take() {
        let _ = sender.send(());
    }
}

impl Child {
    pub(crate) fn new(
        process: OwnedHandle,
        pid: u32,
        stdout: Option<tokio::fs::File>,
        stderr: Option<tokio::fs::File>,
    ) -> std::io::Result<Child> {
        let (sender, receiver) = oneshot::channel();
        let waker: Box<Waker> = Box::new(Mutex::new(Some(sender)));
        let mut wait: HANDLE = std::ptr::null_mut();
        // SAFETY: the process handle and the waker outlive the wait (Drop unregisters it first).
        check(unsafe {
            RegisterWaitForSingleObject(
                &raw mut wait,
                process.as_raw_handle(),
                Some(exited),
                (&raw const *waker).cast(),
                INFINITE,
                WT_EXECUTEONLYONCE | WT_EXECUTEINWAITTHREAD,
            )
        })?;
        Ok(Child { process, pid, stdout, stderr, ended: Some(receiver), wait: Some((wait, waker)), status: None, kill_on_drop: false })
    }

    /// Its pid; Some while it's ours, as tokio's.
    pub fn id(&self) -> Option<u32> {
        Some(self.pid)
    }

    /// Waits for it to end. Cancel-safe: dropping the future and calling
    /// again waits on.
    pub async fn wait(&mut self) -> std::io::Result<ExitStatus> {
        if let Some(status) = self.status {
            return Ok(status);
        }
        if let Some(ended) = self.ended.as_mut() {
            // A closed channel means the waker was dropped without firing, which
            // only Drop does; read the code either way.
            let _ = ended.await;
            self.ended = None;
        }
        let status = self.exit_code()?.ok_or_else(|| std::io::Error::other("the process was said to end but is still running"))?;
        self.status = Some(status);
        super::job::settle(self.pid);
        Ok(status)
    }

    /// How it ended, if it has.
    pub fn try_wait(&mut self) -> std::io::Result<Option<ExitStatus>> {
        if self.status.is_none() {
            self.status = self.exit_code()?;
        }
        Ok(self.status)
    }

    fn exit_code(&self) -> std::io::Result<Option<ExitStatus>> {
        const STILL_ACTIVE: u32 = 259;
        let mut code = 0u32;
        // SAFETY: an open process handle.
        check(unsafe { GetExitCodeProcess(self.process.as_raw_handle(), &raw mut code) })?;
        // A process can exit with 259 itself; then it's signalled, which the wait tells.
        if code == STILL_ACTIVE && self.ended.is_some() {
            return Ok(None);
        }
        Ok(Some(exit_status(code)))
    }

    /// Ends it at once (TerminateProcess, as SIGKILL elsewhere) and waits
    /// for it, as tokio's `kill`.
    pub async fn kill(&mut self) -> std::io::Result<()> {
        if self.try_wait()?.is_none() {
            // SAFETY: an open process handle; one that ended meanwhile refuses, and the wait below says how it ended.
            let ended = unsafe { TerminateProcess(self.process.as_raw_handle(), 1) };
            if ended == 0 && self.try_wait()?.is_none() {
                return Err(std::io::Error::last_os_error());
            }
        }
        self.wait().await.map(|_| ())
    }

    /// Whether dropping it while it runs ends it, as tokio's.
    pub fn kill_on_drop(&mut self, on: bool) {
        self.kill_on_drop = on;
    }

    /// The process's handle, for asking about it.
    pub fn handle(&self) -> &OwnedHandle {
        &self.process
    }
}

impl Drop for Child {
    fn drop(&mut self) {
        if self.kill_on_drop && self.status.is_none() && matches!(self.exit_code(), Ok(None)) {
            // SAFETY: an open process handle, still running a moment ago.
            unsafe { TerminateProcess(self.process.as_raw_handle(), 1) };
        }
        if let Some((wait, waker)) = self.wait.take() {
            // SAFETY: INVALID_HANDLE_VALUE waits for a callback that's running to
            // finish, so the waker is no longer used once this returns.
            unsafe { UnregisterWaitEx(wait, INVALID_HANDLE_VALUE) };
            drop(waker);
        }
    }
}
