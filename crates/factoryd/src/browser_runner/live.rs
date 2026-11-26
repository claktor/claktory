//! The live view's side of the runner (#435 P2): the screen captured
//! (`capture`), the person's input applied (`input`) and the clipboard
//! both ways (`clipboard`), for one session's screen. Input and pastes are
//! applied in order on a thread of their own, so a long text being typed
//! never holds up the runner's control messages.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::thread::JoinHandle;

use super::Outbox;
use super::capture::Capture;
use super::clipboard::Clipboard;
use super::input::Input;
use super::wire::{self, Channel, InputEvent, Report};
use super::xvfb::Display;

/// What the input thread does, in order.
enum Job {
    Event(InputEvent),
    Paste(String),
    /// The view's new size, which the pointer stays within.
    Bound(u16, u16),
}

/// The live view of one screen.
pub struct Live {
    capture: Capture,
    /// Stops after the input thread, which may be pasting through it:
    /// `drop` ends that thread before the fields go.
    clipboard: Clipboard,
    jobs: Option<mpsc::Sender<Job>>,
    halt: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Live {
    /// Starts on `display`, its view `width` × `height` from the top left
    /// (the browser window's size); frames and what's copied go to `out`,
    /// once the view is on and the clipboard watched.
    pub fn start(display: &Display, width: u16, height: u16, out: &Outbox) -> Result<Live, String> {
        let capture = Capture::start(display, width, height, out.clone())?;
        let clipboard = Clipboard::start(display, out.clone())?;
        let mut input = Input::new(display)?;
        input.bound(width, height);
        let halt = input.halt();
        let paster = clipboard.paster();
        let (jobs, queue) = mpsc::channel();
        let out = out.clone();
        let thread = std::thread::Builder::new()
            .name("browser-input".to_owned())
            .spawn(move || {
                for job in queue {
                    let done = match job {
                        Job::Event(event) => input.apply(&event),
                        Job::Paste(text) => paster.paste(text, &mut input),
                        Job::Bound(width, height) => {
                            input.bound(width, height);
                            Ok(())
                        }
                    };
                    // Said once: what comes after would fail the same way.
                    if let Err(why) = done {
                        out.report(&Report::Error {
                            what: format!("the person's input stopped reaching the browser: {why}"),
                            fatal: false,
                        });
                        return;
                    }
                }
            })
            .map_err(|error| error.to_string())?;
        Ok(Live { capture, clipboard, jobs: Some(jobs), halt, thread: Some(thread) })
    }

    /// From factoryd, for the screen (C1 channels 2 to 4): a frame's ack,
    /// the person's input, or text to paste. What doesn't read is dropped.
    pub fn screen(&self, channel: Channel, payload: Vec<u8>) {
        let job = match channel {
            Channel::Frames => {
                if let Some(seq) = wire::read_ack(&payload) {
                    self.capture.ack(seq);
                }
                return;
            }
            Channel::Input => InputEvent::parse(&payload).map(Job::Event),
            Channel::Clipboard => String::from_utf8(payload).ok().map(Job::Paste),
            Channel::Control | Channel::Cdp | Channel::Files => None,
        };
        if let (Some(job), Some(jobs)) = (job, &self.jobs) {
            let _ = jobs.send(job);
        }
    }

    /// Frames on or off, at most `max_width` wide; `quality_hint` 1 to
    /// 100, 0 for the runner's own.
    pub fn view(&self, on: bool, max_width: u16, quality_hint: u8) {
        self.capture.set_view(on, max_width, quality_hint);
    }

    /// The view, and the browser's windows, `width` × `height` now.
    pub fn resize(&self, width: u16, height: u16) -> Result<(), String> {
        self.capture.resize(width, height)?;
        if let Some(jobs) = &self.jobs {
            let _ = jobs.send(Job::Bound(width, height));
        }
        Ok(())
    }

    /// The person's turn on the clipboard: what the browser copies goes
    /// out and pastes count; or neither, and the clipboard is emptied.
    pub fn watch_clipboard(&self, on: bool) {
        self.clipboard.watch(on);
    }

    /// Empties the clipboard and the primary selection, before Chrome
    /// starts again.
    pub fn forget_clipboard(&self) -> Result<(), String> {
        self.clipboard.forget()
    }
}

impl Drop for Live {
    fn drop(&mut self) {
        // Text half typed stops, and the thread ends with its queue.
        self.halt.store(true, Ordering::Relaxed);
        self.jobs = None;
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}
