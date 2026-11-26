//! The clipboard between the person and the browser's screen (#435 P2,
//! C1's channel 4). In: text the person pastes becomes the screen's
//! clipboard (this owns the CLIPBOARD selection and serves it as UTF-8),
//! then Ctrl+V pastes it where the browser's focus is. Out: what the
//! browser copies (a new owner of CLIPBOARD, XFixes says) is read as UTF-8
//! and sent, only while watched: factoryd watches only during the person's
//! turn or a take-over (Fugo 36540), and treats what comes as a hostile
//! page's.
//!
//! What the person pastes or copies is theirs: a password or a code from
//! a sign-in, say. So a paste counts only while watched, and when the
//! watch ends, or Chrome starts again, the clipboard and the primary
//! selection are emptied (no owner), whoever had them: an agent driving
//! afterwards pastes nothing of the person's (Fugo 36828).

use std::sync::mpsc;
use std::thread::JoinHandle;
use std::time::Duration;

use x11rb::connection::{Connection as _, RequestConnection as _};
use x11rb::protocol::Event;
use x11rb::protocol::xfixes::{self, ConnectionExt as _};
use x11rb::protocol::xproto::{self, ConnectionExt as _};
use x11rb::rust_connection::RustConnection;
use x11rb::wrapper::ConnectionExt as _;

use super::Outbox;
use super::input::Input;
use super::keysym::CONTROL_L;
use super::wire::{Channel, MAX_CLIPBOARD, Report};
use super::xvfb::Display;

enum Control {
    /// Own the clipboard with this text, while watched; answered once the
    /// server says so: false when it wasn't watched (and nothing changed).
    Own(String, mpsc::Sender<Result<bool, String>>),
    /// Watched or not; not watched empties it.
    Watch(bool),
    /// Empties it, answered once done.
    Forget(mpsc::Sender<Result<(), String>>),
    Stop,
}

/// The clipboard's thread, and how to steer it.
pub struct Clipboard {
    control: mpsc::Sender<Control>,
    thread: Option<JoinHandle<()>>,
}

impl Clipboard {
    /// Starts serving and watching the screen's clipboard; what the
    /// browser copies goes out on C1's channel 4 while watched (off at
    /// first).
    pub fn start(display: &Display, out: Outbox) -> Result<Clipboard, String> {
        let keeper = Keeper::new(display)?;
        let (control, controls) = mpsc::channel();
        let thread =
            std::thread::Builder::new().name("browser-clipboard".to_owned()).spawn(move || run(keeper, &controls, &out)).map_err(fail)?;
        Ok(Clipboard { control, thread: Some(thread) })
    }

    /// Sends what the browser copies from now on, and takes pastes; or
    /// stops, and empties the clipboard.
    pub fn watch(&self, on: bool) {
        let _ = self.control.send(Control::Watch(on));
    }

    /// Empties the clipboard and the primary selection, whoever has them,
    /// and waits until it's done.
    pub fn forget(&self) -> Result<(), String> {
        let (done, answer) = mpsc::channel();
        self.control.send(Control::Forget(done)).map_err(|_| "the clipboard stopped".to_owned())?;
        answer.recv_timeout(Duration::from_secs(5)).map_err(|_| "the screen didn't answer".to_owned())?
    }

    /// What pastes, for the thread that types.
    pub fn paster(&self) -> Paster {
        Paster { control: self.control.clone() }
    }
}

impl Drop for Clipboard {
    /// Stops the thread, and waits for it.
    fn drop(&mut self) {
        let _ = self.control.send(Control::Stop);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// Pastes through the clipboard's thread.
pub struct Paster {
    control: mpsc::Sender<Control>,
}

impl Paster {
    /// Pastes `text` where the browser's focus is: the clipboard is it,
    /// then Ctrl+V. Text past C1's cap is refused; text while the
    /// clipboard isn't watched (not the person's turn) is dropped, and
    /// said.
    pub fn paste(&self, text: String, input: &mut Input) -> Result<(), String> {
        if text.len() > MAX_CLIPBOARD {
            return Err(format!("{} bytes to paste, more than {MAX_CLIPBOARD}", text.len()));
        }
        let (done, answer) = mpsc::channel();
        self.control.send(Control::Own(text, done)).map_err(|_| "the clipboard stopped".to_owned())?;
        let owned = answer.recv_timeout(Duration::from_secs(5)).map_err(|_| "the screen didn't answer".to_owned())??;
        if !owned {
            return Ok(());
        }
        input.chord(&[CONTROL_L], u32::from(b'v'))
    }
}

/// The atoms the clipboard speaks.
struct Atoms {
    clipboard: xproto::Atom,
    targets: xproto::Atom,
    utf8: xproto::Atom,
    text: xproto::Atom,
    incr: xproto::Atom,
    /// Where an owner puts what this asked it for.
    landing: xproto::Atom,
}

/// Holds the clipboard on the screen: a window of its own, unmapped.
struct Keeper {
    connection: RustConnection,
    window: xproto::Window,
    atoms: Atoms,
    /// What this serves while it owns the clipboard.
    text: Option<String>,
    /// The most one request may carry, so a paste too large for one
    /// property is refused rather than half sent.
    max_request: usize,
}

impl Keeper {
    fn new(display: &Display) -> Result<Keeper, String> {
        let connection = display.connect()?;
        let root = connection.setup().roots.first().ok_or("the screen has no root window")?.root;
        let window = connection.generate_id().map_err(fail)?;
        let attributes = xproto::CreateWindowAux::new().event_mask(xproto::EventMask::PROPERTY_CHANGE);
        connection.create_window(0, window, root, 0, 0, 1, 1, 0, xproto::WindowClass::INPUT_ONLY, 0, &attributes).map_err(fail)?;
        let atom = |name: &[u8]| -> Result<xproto::Atom, String> {
            Ok(connection.intern_atom(false, name).map_err(fail)?.reply().map_err(fail)?.atom)
        };
        let atoms = Atoms {
            clipboard: atom(b"CLIPBOARD")?,
            targets: atom(b"TARGETS")?,
            utf8: atom(b"UTF8_STRING")?,
            text: atom(b"TEXT")?,
            incr: atom(b"INCR")?,
            landing: atom(b"CLAKTORY_CLIPBOARD")?,
        };
        connection.xfixes_query_version(5, 0).map_err(fail)?.reply().map_err(|_| "the screen has no XFixes".to_owned())?;
        let changes = xfixes::SelectionEventMask::SET_SELECTION_OWNER;
        connection.xfixes_select_selection_input(window, atoms.clipboard, changes).map_err(fail)?;
        let max_request = connection.maximum_request_bytes();
        connection.flush().map_err(fail)?;
        Ok(Keeper { connection, window, atoms, text: None, max_request })
    }

    /// Takes the clipboard, to serve `text`.
    fn own(&mut self, text: String) -> Result<(), String> {
        self.text = Some(text);
        self.connection.set_selection_owner(self.window, self.atoms.clipboard, x11rb::CURRENT_TIME).map_err(fail)?;
        let owner = self.connection.get_selection_owner(self.atoms.clipboard).map_err(fail)?.reply().map_err(fail)?.owner;
        if owner != self.window {
            self.text = None;
            return Err("the screen didn't give the clipboard".to_owned());
        }
        Ok(())
    }

    /// Empties CLIPBOARD and PRIMARY: no owner, whoever had them (any
    /// client may), and nothing kept here.
    fn forget(&mut self) -> Result<(), String> {
        self.text = None;
        for selection in [self.atoms.clipboard, xproto::AtomEnum::PRIMARY.into()] {
            self.connection.set_selection_owner(x11rb::NONE, selection, x11rb::CURRENT_TIME).map_err(fail)?;
        }
        // A round trip: done once it's answered.
        self.connection.get_selection_owner(self.atoms.clipboard).map_err(fail)?.reply().map_err(fail)?;
        Ok(())
    }

    /// Answers a request for what this serves: the targets it offers or
    /// the text, written on the requestor's property; refused (no
    /// property) for anything else, or while it serves nothing.
    fn serve(&self, request: &xproto::SelectionRequestEvent) -> Result<(), String> {
        // Old clients name no property: the target is the property then.
        let property = if request.property == x11rb::NONE { request.target } else { request.property };
        let atoms = &self.atoms;
        let served = match &self.text {
            Some(text) if request.selection == atoms.clipboard => {
                if request.target == atoms.targets {
                    let offered = [atoms.targets, atoms.utf8, xproto::AtomEnum::STRING.into(), atoms.text];
                    self.connection
                        .change_property32(xproto::PropMode::REPLACE, request.requestor, property, xproto::AtomEnum::ATOM, &offered)
                        .map_err(fail)?;
                    true
                } else if [atoms.utf8, atoms.text, xproto::AtomEnum::STRING.into()].contains(&request.target) {
                    let (kind, bytes) = match request.target == xproto::Atom::from(xproto::AtomEnum::STRING) {
                        true => (xproto::Atom::from(xproto::AtomEnum::STRING), latin1(text)),
                        false => (atoms.utf8, text.as_bytes().to_vec()),
                    };
                    // Room for the request's own 24 bytes.
                    if bytes.len() + 64 > self.max_request {
                        false
                    } else {
                        self.connection
                            .change_property8(xproto::PropMode::REPLACE, request.requestor, property, kind, &bytes)
                            .map_err(fail)?;
                        true
                    }
                } else {
                    false
                }
            }
            _ => false,
        };
        let notify = xproto::SelectionNotifyEvent {
            response_type: xproto::SELECTION_NOTIFY_EVENT,
            sequence: 0,
            time: request.time,
            requestor: request.requestor,
            selection: request.selection,
            target: request.target,
            property: if served { property } else { x11rb::NONE },
        };
        self.connection.send_event(false, request.requestor, xproto::EventMask::NO_EVENT, notify).map_err(fail)?;
        Ok(())
    }

    /// Asks the clipboard's new owner for its text, as UTF-8.
    fn ask(&self, time: xproto::Timestamp) -> Result<(), String> {
        self.connection.convert_selection(self.window, self.atoms.clipboard, self.atoms.utf8, self.atoms.landing, time).map_err(fail)?;
        Ok(())
    }

    /// The text an owner answered with; None when it gave nothing, gave
    /// something else, or more than C1 carries (never read in pieces).
    fn answer(&self, notify: &xproto::SelectionNotifyEvent) -> Result<Option<String>, String> {
        if notify.property == x11rb::NONE || notify.selection != self.atoms.clipboard {
            return Ok(None);
        }
        let longs = u32::try_from(MAX_CLIPBOARD / 4 + 1).map_err(fail)?;
        let reply = self
            .connection
            .get_property(true, self.window, notify.property, xproto::AtomEnum::ANY, 0, longs)
            .map_err(fail)?
            .reply()
            .map_err(fail)?;
        let text_kinds = [self.atoms.utf8, xproto::Atom::from(xproto::AtomEnum::STRING), self.atoms.text];
        if reply.type_ == self.atoms.incr || !text_kinds.contains(&reply.type_) || reply.format != 8 || reply.bytes_after > 0 {
            if reply.bytes_after > 0 {
                // What's left of a property too large to read.
                self.connection.delete_property(self.window, notify.property).map_err(fail)?;
            }
            return Ok(None);
        }
        if reply.value.len() > MAX_CLIPBOARD {
            return Ok(None);
        }
        Ok(String::from_utf8(reply.value).ok())
    }
}

/// Latin-1 for STRING: a character past it is a question mark.
fn latin1(text: &str) -> Vec<u8> {
    text.chars().map(|char| u8::try_from(u32::from(char)).unwrap_or(b'?')).collect()
}

fn fail(error: impl std::fmt::Display) -> String {
    error.to_string()
}

/// The clipboard's loop: steered by `controls`, serving requests and
/// reading what's copied, sent to `out` while watched.
fn run(mut keeper: Keeper, controls: &mpsc::Receiver<Control>, out: &Outbox) {
    let mut watching = false;
    loop {
        match controls.recv_timeout(Duration::from_millis(20)) {
            Ok(Control::Stop) | Err(mpsc::RecvTimeoutError::Disconnected) => return,
            Ok(Control::Own(text, done)) => {
                if watching {
                    let _ = done.send(keeper.own(text).map(|()| true));
                } else {
                    out.report(&Report::Error {
                        what: "a paste came outside the person's turn, so it was dropped".to_owned(),
                        fatal: false,
                    });
                    let _ = done.send(Ok(false));
                }
            }
            Ok(Control::Watch(on)) => {
                watching = on;
                if !on && let Err(why) = keeper.forget() {
                    tracing::warn!(%why, "the browser's clipboard stopped");
                    return;
                }
            }
            Ok(Control::Forget(done)) => {
                let forgot = keeper.forget();
                let failed = forgot.is_err();
                let _ = done.send(forgot);
                if failed {
                    return;
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
        }
        loop {
            let event = match keeper.connection.poll_for_event() {
                Ok(Some(event)) => event,
                Ok(None) => break,
                Err(why) => {
                    tracing::warn!(%why, "the browser's clipboard lost the screen");
                    return;
                }
            };
            let handled = match event {
                Event::SelectionRequest(request) => keeper.serve(&request),
                Event::SelectionClear(clear) if clear.selection == keeper.atoms.clipboard => {
                    keeper.text = None;
                    Ok(())
                }
                // Copied in the browser: someone else owns the clipboard now.
                Event::XfixesSelectionNotify(notify) if watching && notify.owner != keeper.window && notify.owner != x11rb::NONE => {
                    keeper.ask(notify.selection_timestamp)
                }
                Event::SelectionNotify(notify) if notify.requestor == keeper.window => match keeper.answer(&notify) {
                    Ok(Some(text)) if watching => {
                        out.send(Channel::Clipboard, text.into_bytes());
                        Ok(())
                    }
                    Ok(_) => Ok(()),
                    Err(why) => Err(why),
                },
                _ => Ok(()),
            };
            if let Err(why) = handled.and_then(|()| keeper.connection.flush().map_err(fail)) {
                tracing::warn!(%why, "the browser's clipboard stopped");
                return;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_string_target_gets_latin1() {
        assert_eq!(latin1("café €"), b"caf\xe9 ?");
    }
}
