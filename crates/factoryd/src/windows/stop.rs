//! How the backend is asked to stop on Windows, where there's no SIGTERM:
//! what a Mac's supervisor and launchd send it, and what it sends itself
//! to hand over to its background service.
//!
//! - The supervisor sets an event named for the backend's process id
//!   (`ask_to_stop`), which only the backend makes, private to this user
//!   (this user and SYSTEM, as its job objects are), in this sign-in's own
//!   `Local\` namespace. One that's there before the backend makes it is
//!   another's, and the backend won't start on it.
//! - Ctrl+C or Ctrl+Break, the console closing, the person signing out and
//!   Windows shutting down, when it runs with a console.
//! - `request`, from inside the backend.
//!
//! Each ends it as SIGTERM does elsewhere: what's being written is
//! finished first.

use std::os::windows::io::{AsRawHandle as _, OwnedHandle};
use std::sync::{Arc, OnceLock};

use factory_agent::windows::system::{Protected, is_private_object};
use factory_agent::windows::wide::{check, owned, wide};
use windows_sys::Win32::Foundation::{ERROR_ALREADY_EXISTS, GetLastError, WAIT_OBJECT_0};
use windows_sys::Win32::System::Threading::{CreateEventW, EVENT_MODIFY_STATE, INFINITE, OpenEventW, SetEvent, WaitForSingleObject};

/// READ_CONTROL: to read the event's owner and DACL before setting it.
const READ_CONTROL: u32 = 0x0002_0000;

/// The event the supervisor sets to stop the backend with process `pid`.
pub fn event_name(pid: u32) -> String {
    format!("Local\\Claktory-factoryd-stop-{pid}")
}

fn own() -> &'static Arc<tokio::sync::Notify> {
    static OWN: OnceLock<Arc<tokio::sync::Notify>> = OnceLock::new();
    OWN.get_or_init(Default::default)
}

/// Asks this backend to stop, as `kill -TERM` on its own pid does elsewhere.
pub fn request() {
    own().notify_one();
}

/// What the backend listens to for being asked to stop (`listen`).
pub struct Stop {
    event: Arc<tokio::sync::Notify>,
    c: tokio::signal::windows::CtrlC,
    brk: tokio::signal::windows::CtrlBreak,
    close: tokio::signal::windows::CtrlClose,
    logoff: tokio::signal::windows::CtrlLogoff,
    shutdown: tokio::signal::windows::CtrlShutdown,
}

/// Starts listening for being asked to stop, by any of the ways above,
/// before the backend serves anything (inside a Tokio runtime). Fails when
/// its event can't be made private to it, or a console signal can't be
/// listened for: the supervisor couldn't stop it cleanly then.
pub fn listen() -> std::io::Result<Stop> {
    use tokio::signal::windows::{ctrl_break, ctrl_c, ctrl_close, ctrl_logoff, ctrl_shutdown};
    let handle = make_event(std::process::id())?;
    let (c, brk, close, logoff, shutdown) = (ctrl_c()?, ctrl_break()?, ctrl_close()?, ctrl_logoff()?, ctrl_shutdown()?);
    let event = Arc::new(tokio::sync::Notify::new());
    let told = event.clone();
    std::thread::Builder::new().name("stop-event".to_owned()).spawn(move || {
        // SAFETY: an event handle owned by this thread for the wait.
        if unsafe { WaitForSingleObject(handle.as_raw_handle(), INFINITE) } == WAIT_OBJECT_0 {
            told.notify_one();
        }
    })?;
    Ok(Stop { event, c, brk, close, logoff, shutdown })
}

impl Stop {
    /// Until the backend is asked to stop.
    pub async fn requested(mut self) {
        self.recv().await;
    }

    /// Until the backend is asked to stop, as a SIGTERM stream's `recv`
    /// does elsewhere (always `Some`), for a loop that waits on it with
    /// other things.
    pub async fn recv(&mut self) -> Option<()> {
        tokio::select! {
            () = self.event.notified() => tracing::info!("asked to stop through its stop event"),
            () = own().notified() => {}
            _ = self.c.recv() => tracing::info!("stopped with Ctrl+C"),
            _ = self.brk.recv() => tracing::info!("stopped with Ctrl+Break"),
            _ = self.close.recv() => tracing::info!("its console closed"),
            _ = self.logoff.recv() => tracing::info!("the person is signing out"),
            _ = self.shutdown.recv() => tracing::info!("Windows is shutting down"),
        }
        Some(())
    }
}

/// Makes the stop event for process `pid`, private to this user; refused
/// when there's one by that name already.
fn make_event(pid: u32) -> std::io::Result<OwnedHandle> {
    let protected = Protected::for_user()?;
    let attributes = protected.attributes();
    let name = wide(event_name(pid))?;
    // SAFETY: attributes that outlive the call and a NUL-terminated name;
    // manual reset, not set.
    let event = unsafe { owned(CreateEventW(&raw const attributes, 1, 0, name.as_ptr())) }?;
    // SAFETY: plain call, right after the one whose error it reads.
    if unsafe { GetLastError() } == ERROR_ALREADY_EXISTS {
        return Err(std::io::Error::new(
            std::io::ErrorKind::AlreadyExists,
            "another program made the event Claktory's backend is stopped with, so it won't start",
        ));
    }
    Ok(event)
}

/// Asks the backend with process `pid` to stop (the supervisor's SIGTERM):
/// sets its event, once it's checked to be the backend's own (this user's,
/// with the private DACL).
pub fn ask_to_stop(pid: u32) -> std::io::Result<()> {
    let name = wide(event_name(pid))?;
    // SAFETY: a NUL-terminated name; the handle is checked and owned.
    let event = unsafe { owned(OpenEventW(EVENT_MODIFY_STATE | READ_CONTROL, 0, name.as_ptr())) }?;
    if !is_private_object(&event)? {
        return Err(std::io::Error::new(std::io::ErrorKind::PermissionDenied, "the backend's stop event isn't Claktory's own"));
    }
    // SAFETY: an open event with EVENT_MODIFY_STATE.
    check(unsafe { SetEvent(event.as_raw_handle()) })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One test, since there's one event per process.
    #[tokio::test]
    async fn the_event_or_a_request_stops_it() {
        let waiting = tokio::spawn(listen().expect("listening").requested());
        // Made by `listen` above: there's one for this process now.
        let made = async {
            loop {
                match ask_to_stop(std::process::id()) {
                    Ok(()) => break,
                    Err(_) => tokio::time::sleep(std::time::Duration::from_millis(20)).await,
                }
            }
        };
        tokio::time::timeout(std::time::Duration::from_secs(5), made).await.expect("the event was made");
        tokio::time::timeout(std::time::Duration::from_secs(5), waiting).await.expect("it stopped").unwrap();
        assert_eq!(ask_to_stop(u32::MAX - 1).unwrap_err().kind(), std::io::ErrorKind::NotFound, "no backend there");
        // Its wait ended with it; then from inside.
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        request();
        let stop = listen().expect("listening again once the first is gone");
        tokio::time::timeout(std::time::Duration::from_secs(5), stop.requested()).await.expect("stopped");
    }

    #[test]
    fn an_event_someone_made_first_is_refused() {
        let pid = u32::MAX - 3;
        let first = make_event(pid).expect("the first");
        assert_eq!(make_event(pid).unwrap_err().kind(), std::io::ErrorKind::AlreadyExists);
        drop(first);
        drop(make_event(pid).expect("free again once it's gone"));
    }
}
