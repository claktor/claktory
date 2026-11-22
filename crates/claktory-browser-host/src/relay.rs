//! One session's relay: Chrome (this process's stdin and stdout) on one
//! side, the Claktory app's socket on the other. The helper connects to the
//! app (never starting it), checks its signature, says who it is, then
//! passes frames both ways, each checked by [`Session`] first. It ends at
//! the first of: Chrome closing the port, the app closing the socket, a
//! stop, the extension ending the session, or a frame it refuses; both
//! sides are told why where they can still hear it. Being connected grants
//! nothing: only the person's choice in the app binds this helper to a
//! session.

use std::io::{Read, Write};
use std::os::unix::fs::{FileTypeExt, MetadataExt, PermissionsExt};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, mpsc};
use std::time::Duration;

use serde_json::{Value, json};

use crate::frame::{self, Order};
use crate::peer;
use crate::wire::{self, FromApp, FromChrome, Refusal, Session};

/// The longest path a Unix socket takes (sun_path, less its NUL).
pub const SOCKET_PATH_LIMIT: usize = 103;
/// How long a write to the app may block before the app counts as gone.
const WRITE_TIMEOUT: Duration = Duration::from_secs(10);

/// What a helper is started with.
pub struct Setup {
    /// The extension origin Chrome must name; None refuses every caller.
    pub caller: Option<String>,
    /// The app's socket.
    pub socket: PathBuf,
    /// The code requirement the app must meet.
    pub requirement: String,
    /// This helper's version, for the app.
    pub version: String,
}

/// How a helper ended.
#[derive(Debug, PartialEq, Eq)]
pub enum Ended {
    /// Started for another caller, or by hand without one: nothing was
    /// connected or sent.
    WrongCaller,
    /// The app isn't open, or its socket isn't safe to use: why.
    NoApp(String),
    /// The process on the socket isn't the app: why.
    AppRefused(String),
    /// Chrome closed the port.
    ChromeClosed,
    /// The app closed the socket.
    AppClosed,
    /// The app stopped the session.
    Stopped,
    /// The extension ended the session.
    ExtensionEnded,
    /// The helper refused a frame.
    Refused(Refusal),
}

impl Ended {
    /// A clean end, rather than a refusal.
    pub fn clean(&self) -> bool {
        matches!(self, Ended::ChromeClosed | Ended::AppClosed | Ended::Stopped | Ended::ExtensionEnded)
    }
}

/// The app's socket folder: a folder, not a link, ours, closed to others.
pub fn safe_dir(dir: &Path) -> Result<(), String> {
    let meta = std::fs::symlink_metadata(dir).map_err(|_| "Claktory isn't set up for the browser on this Mac".to_owned())?;
    if meta.file_type().is_symlink() || !meta.is_dir() {
        return Err(format!("{} isn't a folder", dir.display()));
    }
    if meta.uid() != unsafe { libc::getuid() } {
        return Err(format!("{} belongs to another user", dir.display()));
    }
    if meta.permissions().mode() & 0o077 != 0 {
        return Err(format!("{} is open to other users", dir.display()));
    }
    Ok(())
}

/// Connects to the app's socket, after checking its folder and itself.
pub fn connect(socket: &Path) -> Result<UnixStream, String> {
    let dir = socket.parent().ok_or("the socket has no folder")?;
    safe_dir(dir)?;
    if socket.as_os_str().len() > SOCKET_PATH_LIMIT {
        return Err(format!("the socket's path is longer than {SOCKET_PATH_LIMIT} bytes"));
    }
    let meta = std::fs::symlink_metadata(socket).map_err(|_| "Claktory isn't open on this Mac".to_owned())?;
    if !meta.file_type().is_socket() || meta.uid() != unsafe { libc::getuid() } {
        return Err(format!("{} isn't the app's socket", socket.display()));
    }
    UnixStream::connect(socket).map_err(|_| "Claktory isn't open on this Mac".to_owned())
}

fn tell(output: &Mutex<impl Write>, order: Order, message: &Value) -> bool {
    let mut output = output.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    frame::write(&mut *output, order, message).is_ok()
}

fn lock(session: &Mutex<Session>) -> std::sync::MutexGuard<'_, Session> {
    session.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn ended(reason: &str) -> Value {
    json!({"type": "ended", "reason": reason})
}

/// Runs one helper: `caller` is what Chrome passed.
pub fn run(caller: &str, setup: &Setup, chrome_in: impl Read + Send + 'static, chrome_out: impl Write + Send + 'static) -> Ended {
    if setup.caller.as_deref() != Some(caller) {
        return Ended::WrongCaller;
    }
    let chrome_out = Arc::new(Mutex::new(chrome_out));
    let app = match connect(&setup.socket) {
        Ok(app) => app,
        Err(why) => {
            tell(&chrome_out, Order::Native, &ended("no_app"));
            return Ended::NoApp(why);
        }
    };
    if let Err(why) = peer::check(&app, &setup.requirement) {
        tell(&chrome_out, Order::Native, &ended("app_refused"));
        return Ended::AppRefused(why);
    }
    let (Ok(app_in), Ok(app_out)) = (app.try_clone(), app.try_clone()) else {
        tell(&chrome_out, Order::Native, &ended("no_app"));
        return Ended::NoApp("couldn't use the socket".into());
    };
    let _ = app.set_write_timeout(Some(WRITE_TIMEOUT));
    let app_out = Arc::new(Mutex::new(app_out));
    let first = json!({"type": "helper", "v": wire::VERSION, "version": setup.version, "caller": caller});
    if !tell(&app_out, Order::Little, &first) {
        tell(&chrome_out, Order::Native, &ended("app_closed"));
        return Ended::AppClosed;
    }

    let session = Arc::new(Mutex::new(Session::default()));
    let (done, end) = mpsc::channel::<Ended>();

    // Chrome to the app.
    {
        let (session, app_out, done) = (session.clone(), app_out.clone(), done.clone());
        let mut chrome_in = chrome_in;
        std::thread::spawn(move || {
            let why = loop {
                let frame = match frame::read(&mut chrome_in, Order::Native) {
                    Ok(Some(frame)) => frame,
                    Ok(None) => break Ended::ChromeClosed,
                    Err(bad) => break Ended::Refused(Refusal::BadFrame(bad.to_string())),
                };
                let checked = lock(&session).from_chrome(&frame);
                match checked {
                    Ok(then) => {
                        if !tell(&app_out, Order::Little, &Value::Object(frame)) {
                            break Ended::AppClosed;
                        }
                        if then == FromChrome::Ended {
                            break Ended::ExtensionEnded;
                        }
                    }
                    Err(refusal) => break Ended::Refused(refusal),
                }
            };
            let _ = done.send(why);
        });
    }

    // The app to Chrome.
    {
        let (session, chrome_out, done) = (session.clone(), chrome_out.clone(), done.clone());
        let mut app_in = app_in;
        std::thread::spawn(move || {
            let why = loop {
                let frame = match frame::read(&mut app_in, Order::Little) {
                    Ok(Some(frame)) => frame,
                    Ok(None) => break Ended::AppClosed,
                    // The socket shut down under it: the end was decided elsewhere.
                    Err(frame::Bad::Io(_) | frame::Bad::Truncated) => break Ended::AppClosed,
                    Err(bad) => break Ended::Refused(Refusal::BadFrame(bad.to_string())),
                };
                let checked = lock(&session).from_app(&frame);
                match checked {
                    Ok(FromApp::Send(message)) => {
                        if !tell(&chrome_out, Order::Native, &message) {
                            break Ended::ChromeClosed;
                        }
                    }
                    Ok(FromApp::Stop) => {
                        tell(&chrome_out, Order::Native, &json!({"type": "stop"}));
                        break Ended::Stopped;
                    }
                    Err(refusal) => break Ended::Refused(refusal),
                }
            };
            let _ = done.send(why);
        });
    }
    drop(done);

    let why = end.recv().unwrap_or(Ended::ChromeClosed);
    match &why {
        Ended::Refused(refusal) => {
            tell(&chrome_out, Order::Native, &ended(refusal.reason()));
            tell(&app_out, Order::Little, &ended(refusal.reason()));
        }
        Ended::AppClosed => {
            tell(&chrome_out, Order::Native, &ended("app_closed"));
        }
        _ => {}
    }
    let _ = app.shutdown(std::net::Shutdown::Both);
    why
}
